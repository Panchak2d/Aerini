use async_trait::async_trait;
use serde_json::{json, Value};
use std::process::Stdio;
use std::sync::OnceLock;
use tokio::process::Command;

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortDefinition, PortPosition};

static NODE_BIN: OnceLock<&'static str> = OnceLock::new();
/// Guards the macOS partial-sandbox warning so it fires once per process, not once per execution.
#[cfg(target_os = "macos")]
static MACOS_SANDBOX_PARTIAL_WARNED: OnceLock<()> = OnceLock::new();

/// ESM loader script injected when sandbox mode is active.
/// Intercepts `import` resolution and blocks dangerous built-in modules.
/// Written to a temp file because Node.js loaders cannot be passed inline.
///
/// Blocked modules: child_process, fs, fs/promises, net, http, https, dgram, dns, os
/// These cover: subprocess spawning, filesystem access, raw network access.
/// Allowed: crypto, util, path, stream, events, url, buffer, string_decoder, querystring
///
/// Compatibility: --experimental-loader works on all Node.js 18+ versions.
/// The API moved to a worker thread in 18.19 but the flag is not removed.
const SANDBOX_LOADER_CONTENT: &str = r#"
// Flowo Code Node sandbox loader.
// Blocks import of dangerous built-in modules. Do not modify — auto-generated.
const BLOCKED = new Set([
  'node:child_process', 'child_process',
  'node:fs',            'fs',
  'node:fs/promises',   'fs/promises',
  'node:net',           'net',
  'node:http',          'http',
  'node:https',         'https',
  'node:http2',         'http2',
  'node:dgram',         'dgram',
  'node:dns',           'dns',
  'node:dns/promises',  'dns/promises',
  'node:os',            'os',
  'node:cluster',       'cluster',
  'node:worker_threads','worker_threads',
  'node:vm',            'vm',
  'node:repl',          'repl',
  'node:domain',        'domain',
]);

export async function resolve(specifier, context, nextResolve) {
  if (BLOCKED.has(specifier)) {
    throw new Error(
      `[Flowo sandbox] Import of '${specifier}' is blocked. ` +
      `Filesystem, network, and subprocess access are not available in sandboxed Code nodes. ` +
      `Use the HTTP Request node for outbound HTTP, or disable sandboxing for trusted deployments.`
    );
  }
  return nextResolve(specifier, context);
}
"#;

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
                "result":           { "description": "Value passed to output()" },
                "stdout":           { "type": "string" },
                "duration_ms":      { "type": "number" },
                "_sandbox_partial": {
                    "type": "boolean",
                    "description": "Present and true on macOS when --code-sandbox is active. ESM module import restrictions are enforced but CPU/memory resource limits (setrlimit) are Linux-only and not applied."
                }
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
                 Pass --allow-code to the server to enable it.",
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

        let sandbox_enabled = input.context.metadata.get("__code_sandbox")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // On Windows, neither the ESM module loader nor setrlimit() resource
        // limits are implemented. Reporting "sandbox active" while providing no
        // actual isolation is a security lie. Fail loudly so operators know their
        // deployment is not protected, rather than silently running unsandboxed.
        #[cfg(windows)]
        if sandbox_enabled {
            return NodeOutput::failure(NodeError::unrecoverable(
                "SANDBOX_NOT_SUPPORTED",
                "--code-sandbox is not supported on Windows. \
                 The ESM module loader (which blocks fs/net/child_process imports) \
                 and OS-level resource limits (setrlimit) are both unavailable on \
                 this platform. Run without --code-sandbox for trusted deployments, \
                 or deploy on Linux where full sandboxing is implemented.",
            ));
        }

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

        // Detect node binary once per process lifetime; cached via OnceLock.
        let node_bin = *NODE_BIN.get_or_init(|| {
            if std::process::Command::new(if cfg!(windows) { "where" } else { "which" })
                .arg("node")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
            { "node" } else { "nodejs" }
        });

        // In sandbox mode, write the loader to a temp file.
        // Temp file is cleaned up when the guard drops at end of scope.
        // If the write fails (read-only /tmp, disk full, restrictive umask — all
        // common in hardened containers), hard-fail rather than silently running
        // without module restrictions. The caller explicitly requested sandboxing;
        // proceeding unsandboxed violates that contract.
        #[cfg(not(windows))]
        let loader_tempfile: Option<tempfile::NamedTempFile> = if sandbox_enabled {
            match write_sandbox_loader() {
                Ok(f)  => Some(f),
                Err(e) => {
                    return NodeOutput::failure(NodeError::unrecoverable(
                        "SANDBOX_INIT_FAILED",
                        format!(
                            "Code sandbox loader could not be written to temp directory: {}. \
                             Cannot proceed unsandboxed when --code-sandbox is active. \
                             Ensure /tmp is writable, or remove --code-sandbox for trusted deployments.",
                            e
                        ),
                    ));
                }
            }
        } else {
            None
        };
        #[cfg(windows)]
        let _loader_tempfile: Option<()> = None;

        let mut cmd = Command::new(node_bin);
        cmd.arg("--input-type=module");
        if sandbox_enabled {
            // Blocks eval() and new Function() from generating executable code.
            cmd.arg("--disallow-code-generation-from-strings");
        }
        #[cfg(not(windows))]
        if let Some(ref lf) = loader_tempfile {
            // ESM loader intercepts import resolution to block dangerous modules.
            cmd.arg(format!("--experimental-loader=file://{}", lf.path().display()));
            // Suppress the loader experimental warning — it's noise for end users.
            cmd.arg("--no-warnings");
        }
        cmd.stdin(Stdio::piped())
           .stdout(Stdio::piped())
           .stderr(Stdio::piped());

        // Apply OS-level resource limits on Linux in sandbox mode.
        // RLIMIT_AS (virtual address space): configurable via __code_max_memory_mb (default 512 MB).
        // RLIMIT_CPU (CPU seconds): timeout_secs + 5 — backstop for busy-loops.
        // Safety: pre_exec runs between fork() and exec(). setrlimit(2) is listed
        // in POSIX as async-signal-safe. No allocations are made in the closure.
        #[cfg(target_os = "linux")]
        let spawn_result = if sandbox_enabled {
            let cpu_limit = (timeout_secs + 5) as libc::rlim_t;
            let mem_limit = input.context.metadata.get("__code_max_memory_mb")
                .and_then(|v| v.as_u64())
                .unwrap_or(512)
                .min(16_384) // cap at 16 TB — prevents u64 overflow on * 1024 * 1024
                * 1024 * 1024;
            let mem_limit = mem_limit as libc::rlim_t;
            unsafe {
                cmd.pre_exec(move || {
                    let r1 = libc::setrlimit(libc::RLIMIT_CPU, &libc::rlimit {
                        rlim_cur: cpu_limit,
                        rlim_max: cpu_limit,
                    });
                    if r1 != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    let r2 = libc::setrlimit(libc::RLIMIT_AS, &libc::rlimit {
                        rlim_cur: mem_limit,
                        rlim_max: mem_limit,
                    });
                    if r2 != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                })
            }.spawn()
        } else {
            cmd.spawn()
        };
        // Track whether the sandbox is partial (macOS: module restrictions apply
        // but setrlimit CPU/memory caps are Linux-only). Surfaced in node output
        // as `_sandbox_partial: true` so API callers can detect and alert.
        #[cfg(target_os = "linux")]
        let sandbox_partial = false;
        #[cfg(not(target_os = "linux"))]
        let sandbox_partial = sandbox_enabled;

        #[cfg(not(target_os = "linux"))]
        let spawn_result = {
            // On macOS, the ESM module loader applies but setrlimit() is Linux-only —
            // CPU/memory are uncapped. Windows is rejected above with SANDBOX_NOT_SUPPORTED.
            // Warn once per process so operators know the sandbox is partial without
            // flooding logs on every workflow execution.
            #[cfg(target_os = "macos")]
            if sandbox_enabled {
                MACOS_SANDBOX_PARTIAL_WARNED.get_or_init(|| {
                    tracing::warn!(
                        "Code node sandbox is PARTIAL on macOS. \
                         ESM module import restrictions (fs/net/child_process) are enforced, \
                         but CPU and memory resource limits (setrlimit) are Linux-only and \
                         are NOT applied. A runaway script can exhaust system resources. \
                         The per-node timeout_secs (max 60 s) is the only effective resource \
                         cap on this platform. For full sandboxing, deploy on Linux."
                    );
                });
            }
            cmd.spawn()
        };

        let mut child = match spawn_result {
            Ok(c)  => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return NodeOutput::failure(
                NodeError::unrecoverable(
                    "NODE_NOT_FOUND",
                    format!("Node.js not found on this system: {}. Install Node.js to use the Code node.", e)
                )
            ),
            Err(e) => return NodeOutput::failure(
                NodeError::unrecoverable(
                    "SANDBOX_INIT_FAILED",
                    format!("Code node sandbox failed to initialise: {}. \
                             Resource limits could not be applied — check that the process \
                             hard limits (RLIMIT_CPU, RLIMIT_AS) allow the requested values.", e)
                )
            ),
        };

        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            let _ = stdin.write_all(wrapper.as_bytes()).await;
        }

        const MAX_OUTPUT_BYTES: u64 = 10 * 1024 * 1024; // 10 MB per stream
        let stdout_task = tokio::spawn({
            use tokio::io::AsyncReadExt;
            let mut pipe = child.stdout.take();
            async move {
                let mut buf = Vec::new();
                if let Some(ref mut h) = pipe {
                    let _ = h.take(MAX_OUTPUT_BYTES).read_to_end(&mut buf).await;
                }
                buf
            }
        });
        let stderr_task = tokio::spawn({
            use tokio::io::AsyncReadExt;
            let mut pipe = child.stderr.take();
            async move {
                let mut buf = Vec::new();
                if let Some(ref mut h) = pipe {
                    let _ = h.take(MAX_OUTPUT_BYTES).read_to_end(&mut buf).await;
                }
                buf
            }
        });

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            child.wait(),
        ).await;

        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Err(_) => {
                let _ = child.kill().await;
                stdout_task.abort();
                stderr_task.abort();
                NodeOutput::failure(
                    NodeError::unrecoverable("TIMEOUT", format!("Code exceeded {}s timeout", timeout_secs))
                )
            }
            Ok(Err(e)) => {
                stdout_task.abort();
                stderr_task.abort();
                NodeOutput::failure(NodeError::unrecoverable("EXEC_ERROR", e.to_string()))
            }
            Ok(Ok(status)) => {
                let stdout_bytes = stdout_task.await.unwrap_or_default();
                let stderr_bytes = stderr_task.await.unwrap_or_default();
                let stdout = String::from_utf8_lossy(&stdout_bytes).to_string();
                let stderr = String::from_utf8_lossy(&stderr_bytes).trim().to_string();

                if !status.success() && stdout.is_empty() {
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
                        let mut out = json!({
                            "result":      result_val,
                            "stdout":      stdout,
                            "duration_ms": duration_ms
                        });
                        if sandbox_partial {
                            out["_sandbox_partial"] = json!(true);
                        }
                        NodeOutput::success_with_logs(
                            out,
                            vec![format!("Code executed in {}ms", duration_ms)],
                        )
                    }
                }
            }
        }
    }
}

// ── Sandbox helpers ────────────────────────────────────────────────────────────

/// Write the sandbox ESM loader script to a named temp file.
/// The caller is responsible for keeping the returned `NamedTempFile` alive until
/// the Node.js subprocess exits — drop = delete.
#[cfg(not(windows))]
fn write_sandbox_loader() -> std::io::Result<tempfile::NamedTempFile> {
    use std::io::Write;
    let mut f = tempfile::Builder::new()
        .prefix("flowo-sandbox-")
        .suffix(".mjs")
        .tempfile()?;
    f.write_all(SANDBOX_LOADER_CONTENT.as_bytes())?;
    f.flush()?;
    Ok(f)
}

// tokio::process::Command exposes pre_exec() directly on Unix — no CommandExt import needed.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExecutionContext;
    use std::collections::HashMap;

    fn make_input(code: &str, disabled: bool) -> NodeInput {
        let mut metadata = HashMap::new();
        if disabled {
            metadata.insert("__code_disabled".to_string(), serde_json::Value::Bool(true));
        }
        NodeInput {
            node_id:      "test".to_string(),
            workflow_id:  "wf".to_string(),
            execution_id: "exec".to_string(),
            input: json!({ "code": code }),
            context: ExecutionContext {
                variables:    HashMap::new(),
                node_outputs: std::sync::Arc::new(HashMap::new()),
                metadata,
            },
        }
    }

    #[tokio::test]
    async fn code_disabled_flag_returns_error() {
        let out = CodeNode.execute(make_input("console.log('hi')", true)).await;
        assert!(!out.success);
        let err = out.error.expect("expected NodeError");
        assert_eq!(err.code, "CODE_DISABLED");
        assert!(!err.recoverable);
    }

    #[tokio::test]
    async fn missing_code_returns_error() {
        let input = NodeInput {
            node_id:      "test".to_string(),
            workflow_id:  "wf".to_string(),
            execution_id: "exec".to_string(),
            input: json!({}),
            context: ExecutionContext {
                variables:    HashMap::new(),
                node_outputs: std::sync::Arc::new(HashMap::new()),
                metadata:     HashMap::new(),
            },
        };
        let out = CodeNode.execute(input).await;
        assert!(!out.success);
        let err = out.error.expect("expected NodeError");
        assert_eq!(err.code, "MISSING_CODE");
    }
}
