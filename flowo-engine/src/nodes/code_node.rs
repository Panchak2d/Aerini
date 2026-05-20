use async_trait::async_trait;
use serde_json::{json, Value};
use std::process::Stdio;
use tokio::process::Command;

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortDefinition, PortPosition};

/// Code node — runs a JavaScript snippet using the system Node.js installation.
/// The snippet has access to `input` (the incoming data) and `context` (all node outputs).
/// Return a value by calling `output(value)` — whatever you pass becomes the node output.
///
/// Example:
///   const temp = input.current?.temperature_2m ?? 0;
///   output({ celsius: temp, fahrenheit: temp * 9/5 + 32 });
pub struct CodeNode;

#[async_trait]
impl Node for CodeNode {
    fn type_id(&self)      -> &'static str { "code" }
    fn display_name(&self) -> &'static str { "Code (JS)" }
    fn node_type(&self)    -> NodeType     { NodeType::Action }
    fn version(&self)      -> &'static str { "1.0.0" }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["code"],
            "properties": {
                "code": {
                    "type": "string",
                    "description": "JavaScript to run. Use output(value) to return a result. Has access to `input` and `context`."
                },
                "timeout_secs": {
                    "type": "number",
                    "description": "Max execution time in seconds (default 10, max 60)"
                }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "result":   { "description": "Value passed to output()" },
                "stdout":   { "type": "string" },
                "duration_ms": { "type": "number" }
            }
        })
    }

    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![
                PortDefinition { id: "input".to_string(),    label: "In".to_string(),    position: PortPosition::Left  },
            ],
            outputs: vec![
                PortDefinition { id: "output".to_string(),   label: "Out".to_string(),   position: PortPosition::Right },
                PortDefinition { id: "on_error".to_string(), label: "Error".to_string(), position: PortPosition::Right },
            ],
        }
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        if input.context.metadata.get("__code_disabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            return NodeOutput::failure(NodeError::unrecoverable(
                "CODE_DISABLED",
                "Code (JS) node is disabled in this deployment. \
                 Pass --disable-code=false to the server to enable it.",
            ));
        }

        let code = match input.input["code"].as_str() {
            Some(c) if !c.trim().is_empty() => c.to_string(),
            _ => return NodeOutput::failure(
                NodeError::unrecoverable("MISSING_CODE", "code field is required")
            ),
        };

        let timeout_secs = input.input["timeout_secs"]
            .as_u64().unwrap_or(10).min(60);

        // Inject context and input as globals, wrap user code so output() captures the result
        let wrapper = format!(r#"
const input   = {};
const context = {};
let   __result  = undefined;

function output(v) {{ __result = v; }}

// User code runs here
(async () => {{
  {}
}})().then(() => {{
  process.stdout.write(JSON.stringify({{ ok: true, result: __result ?? null }}));
}}).catch(err => {{
  process.stdout.write(JSON.stringify({{ ok: false, error: err.message ?? String(err) }}));
}});
"#,
            serde_json::to_string(&input.input).unwrap_or_else(|_| "{}".to_string()),
            serde_json::to_string(&input.context.node_outputs).unwrap_or_else(|_| "{}".to_string()),
            code
        );

        let start = std::time::Instant::now();

        // Try node, then nodejs as fallback (some Linux distros name it differently)
        let node_bin = if which_node("node").await { "node" } else { "nodejs" };

        let child = Command::new(node_bin)
            .arg("--input-type=module")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();

        let mut child = match child {
            Ok(c)  => c,
            Err(e) => return NodeOutput::failure(
                NodeError::unrecoverable(
                    "NODE_NOT_FOUND",
                    format!("Node.js not found on this system: {}. Install Node.js to use the Code node.", e)
                )
            ),
        };

        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            let _ = stdin.write_all(wrapper.as_bytes()).await;
        }

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            child.wait_with_output()
        ).await;

        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Err(_) => NodeOutput::failure(
                NodeError::unrecoverable("TIMEOUT", format!("Code exceeded {}s timeout", timeout_secs))
            ),
            Ok(Err(e)) => NodeOutput::failure(
                NodeError::unrecoverable("EXEC_ERROR", e.to_string())
            ),
            Ok(Ok(out)) => {
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();

                if !out.status.success() && stdout.is_empty() {
                    return NodeOutput::failure(
                        NodeError::unrecoverable("RUNTIME_ERROR",
                            if stderr.is_empty() { "Code exited with non-zero status".to_string() }
                            else { stderr }
                        )
                    );
                }

                // Parse the JSON output written by the wrapper
                match serde_json::from_str::<Value>(stdout.trim()) {
                    Err(_) => NodeOutput::failure(
                        NodeError::unrecoverable("PARSE_ERROR",
                            format!("Could not parse code output. stderr: {}", stderr)
                        )
                    ),
                    Ok(parsed) => {
                        if parsed["ok"].as_bool() == Some(false) {
                            let raw_msg = parsed["error"].as_str().unwrap_or("Runtime error").to_string();
                            // Improve ESM import error message — Node.js runs code as ESM
                            // (--input-type=module), so `require()` is not available.
                            let msg = if raw_msg.contains("require is not defined")
                                || raw_msg.contains("ERR_REQUIRE_ESM")
                            {
                                format!(
                                    "{}\\n\\nNote: Code nodes run as ES modules. \
                                     Use `import` instead of `require()`. \
                                     Example: import fs from 'fs/promises'; \
                                     Only built-in Node.js modules are available — \
                                     npm packages are not supported.",
                                    raw_msg
                                )
                            } else {
                                raw_msg
                            };
                            return NodeOutput::failure(
                                NodeError::unrecoverable("RUNTIME_ERROR", msg)
                            );
                        }
                        let result_val = parsed["result"].clone();
                        NodeOutput::success_with_logs(
                            json!({
                                "result":      result_val,
                                "stdout":      stdout,
                                "duration_ms": duration_ms
                            }),
                            vec![format!("Code executed in {}ms", duration_ms)],
                        )
                    }
                }
            }
        }
    }
}

async fn which_node(bin: &str) -> bool {
    Command::new(if cfg!(windows) { "where" } else { "which" })
        .arg(bin)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}
