use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use tokio::process::Command;

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortArity, PortDefinition, PortPosition};

use super::shell::failure_stderr;
use super::util::{cfg_u64_opt, drain_capped, kill_group_on_drop, FloodLimit, Keep, OUTPUT_CAP_BYTES, OUTPUT_FLOOD_BYTES};

#[cfg(target_os = "linux")]
const DEFAULT_CODE_MEMORY_MB: u64 = 512;
#[cfg(target_os = "linux")]
const MAX_CODE_MEMORY_MB: u64 = 16_384;
/// Address space Node.js reserves at startup regardless of what the script uses.
/// Node 24 aborts at startup with less than about 1.1 GB of RLIMIT_AS and peaks near 1.3 GB.
#[cfg(all(target_os = "linux", not(target_arch = "aarch64")))]
const NODE_ADDRESS_SPACE_OVERHEAD_MB: u64 = 2048;
/// Not measured on aarch64. RLIMIT_AS caps address space, not memory in use, so too much
/// headroom only loosens the cap on runaway allocations while too little aborts every
/// sandboxed Code node at startup; hence double the x86_64 figure.
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const NODE_ADDRESS_SPACE_OVERHEAD_MB: u64 = 4096;

/// How much of the end of stderr is kept; failure messages show less than this.
const STDERR_KEEP_BYTES: usize = 64 * 1024;

static NODE_BIN: OnceLock<Result<PathBuf, String>> = OnceLock::new();
/// Guards the macOS partial-sandbox warning so it fires once per process, not once per execution.
#[cfg(target_os = "macos")]
static MACOS_SANDBOX_PARTIAL_WARNED: OnceLock<()> = OnceLock::new();

/// ESM loader script injected when sandbox mode is active.
/// Intercepts `import` resolution and blocks dangerous built-in modules.
/// Written to a temp file because Node.js loaders cannot be passed inline.
///
/// Blocked modules: child_process, fs, fs/promises, net, http, https, dgram, dns, os, module,
/// v8, inspector
/// These cover: subprocess spawning, filesystem access, raw network access, and — via
/// `module`'s `createRequire()` — a CommonJS `require()` that would otherwise reach every
/// other entry on this list. `v8` is blocked because `v8.writeHeapSnapshot(path)` writes a
/// heap dump — containing arbitrary script-controlled string data — to any filesystem path,
/// an arbitrary-file-write primitive independent of `fs`. `inspector` is blocked because
/// `inspector.open()` starts a debugger session whose `Runtime.evaluate` executes code
/// without going through `eval`/`new Function`, bypassing `--disallow-code-generation-from-strings`.
/// `process.getBuiltinModule()` reaches the same builtins without an `import` and so never
/// hits this hook; `execute()`'s sandbox prelude overrides it with the same blocklist.
/// Allowed: crypto, util, path, stream, events, url, buffer, string_decoder, querystring
///
/// Compatibility: --experimental-loader works on all Node.js 18+ versions.
/// The API moved to a worker thread in 18.19 but the flag is not removed.
#[cfg(not(windows))]
const SANDBOX_LOADER_CONTENT: &str = r#"
// Aerini Code Node sandbox loader.
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
  'node:module',        'module',
  'node:v8',            'v8',
  'node:inspector',     'inspector',
  'node:inspector/promises', 'inspector/promises',
]);

export async function resolve(specifier, context, nextResolve) {
  if (BLOCKED.has(specifier)) {
    throw new Error(
      `[Aerini sandbox] Import of '${specifier}' is blocked. ` +
      `Filesystem, module-imported network access, subprocess access, CommonJS require() ` +
      `(via the 'module' builtin), heap-snapshot file writes (via 'v8'), and debugger-based ` +
      `code execution (via 'inspector') are all blocked in sandboxed Code nodes. ` +
      `Global fetch() and WebSocket are also disabled. ` +
      `Use the HTTP Request node for outbound HTTP, or disable sandboxing for trusted deployments.`
    );
  }
  return nextResolve(specifier, context);
}
"#;

/// Prelude injected into the wrapper script before user code, when sandbox mode
/// is active. Removes globals that reach the same capabilities
/// `SANDBOX_LOADER_CONTENT`'s resolve hook blocks, without going through an
/// `import` at all: `fetch`/`WebSocket`/`XMLHttpRequest` are built-in globals
/// since Node 18, and `process.getBuiltinModule()` returns any builtin module
/// directly from the `process` global. Blocking `module` here (mirroring the
/// loader's own list) is what matters most: `module.createRequire()` hands
/// back a full CommonJS `require()`, which reaches every other blocked
/// builtin regardless of what the loader hook does.
///
/// `process.binding()`, `process._linkedBinding()`, and `process.dlopen()` are deleted
/// outright rather than blocklisted by name: they hand back raw internal Node bindings
/// (`process.binding('fs')`, `process.binding('spawn_sync')`) or load native addons
/// directly, bypassing the ESM loader hook entirely regardless of which module names it
/// blocks. Verified: with only the module-import blocklist in place, sandboxed code could
/// call `process.binding('spawn_sync').spawn(...)` to execute arbitrary OS commands with
/// the host process's own privileges. No sandboxed workflow snippet has a legitimate use
/// for any of the three, so they are removed unconditionally rather than name-filtered.
/// Node writes to a pipe asynchronously on Unix: a write the pipe cannot take is queued in
/// Node's memory, and anything still queued is lost when the script calls `process.exit()`. Blocking mode
/// makes each write wait for this process to read it, so a script cannot grow Node's memory faster than it
/// is drained and a failing script's last stderr lines always arrive.
const STDIO_BLOCKING: &str = "for (const s of [process.stdout, process.stderr]) { try { s._handle?.setBlocking?.(true); } catch {} }\n";

const SANDBOX_GLOBALS_HARDENING: &str = r#"delete globalThis.fetch;
delete globalThis.WebSocket;
delete globalThis.XMLHttpRequest;
delete process.binding;
delete process._linkedBinding;
delete process.dlopen;
{
  const __sandboxBlockedBuiltins = new Set([
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
    'node:module',        'module',
    'node:v8',            'v8',
    'node:inspector',     'inspector',
    'node:inspector/promises', 'inspector/promises',
  ]);
  const __sandboxOriginalGetBuiltinModule = process.getBuiltinModule.bind(process);
  process.getBuiltinModule = (id) => {
    if (__sandboxBlockedBuiltins.has(id)) {
      throw new Error(`[Aerini sandbox] Access to '${id}' via process.getBuiltinModule is blocked.`);
    }
    return __sandboxOriginalGetBuiltinModule(id);
  };
}
"#;

/// Code node — runs a JavaScript snippet using Aerini's bundled Node.js runtime.
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
    fn description(&self)  -> &'static str { "Execute a JavaScript snippet and return its result. Full Node.js runtime available." }

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
                    "description": "Max execution time in seconds (1 to 60, default 10)"
                }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "result":           { "description": "Value passed to output()" },
                "stdout":           { "type": "string", "description": "Text the code printed, such as console.log output" },
                "truncated":        { "type": "boolean", "description": "True when printed output exceeded 10 MB and only the last 10 MB is kept" },
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
                PortDefinition { id: "input".to_string(),    label: "In".to_string(),    position: PortPosition::Left  , port_type: None, arity: PortArity::Single },
            ],
            outputs: vec![
                PortDefinition { id: "output".to_string(),   label: "Out".to_string(),   position: PortPosition::Right , port_type: None, arity: PortArity::Single },
                PortDefinition { id: "on_error".to_string(), label: "Error".to_string(), position: PortPosition::Right, port_type: None, arity: PortArity::Single },
            ],
        }
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        self.execute_with(input, None).await
    }
}

impl CodeNode {
    /// `node_bin` overrides the bundled-runtime lookup; tests use it to supply a Node.js binary.
    async fn execute_with(&self, input: NodeInput, node_bin: Option<&Path>) -> NodeOutput {
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

        let timeout_secs = match cfg_u64_opt(&input.input["timeout_secs"], "timeout_secs") {
            Ok(v) => v.unwrap_or(10).clamp(1, 60),
            Err(e) => return NodeOutput::failure(e),
        };

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

        // When sandbox is enabled, remove globals that reach the same capabilities the
        // ESM loader hook blocks, without going through it at all.
        let sandbox_globals_hardening = if sandbox_enabled {
            SANDBOX_GLOBALS_HARDENING
        } else {
            ""
        };

        let empty_obj = serde_json::Value::Object(Default::default());
        let direct_input = input.context.metadata.get("__direct_input").unwrap_or(&empty_obj);
        let name_outputs = input.context.metadata.get("__node_name_outputs").unwrap_or(&empty_obj);
        let marker = format!("__AERINI_RESULT_{}__", uuid::Uuid::new_v4().simple());
        let wrapper = format!(r#"
// input   — output of the directly-wired upstream node.
//   HTTP Request: {{ status, body, headers }}   e.g. input.body.id
//   Schedule:     {{ triggered_at, mode, interval_secs }}
// context — all upstream outputs keyed by node name.
//   e.g. context["HTTP Request"].body.id
{}{}const input   = {};
const context = {};
let   __result  = undefined;

function output(v) {{ __result = v; }}

// User code runs here
(async () => {{
  {}
}})().then(() => {{
  process.stdout.write("\n{marker}" + JSON.stringify({{ ok: true, result: __result ?? null }}) + "\n");
}}).catch(err => {{
  process.stdout.write("\n{marker}" + JSON.stringify({{ ok: false, error: err?.message ?? String(err) }}) + "\n");
}});
"#,
            sandbox_globals_hardening,
            STDIO_BLOCKING,
            serde_json::to_string(direct_input).unwrap_or_else(|_| "{}".to_string()),
            serde_json::to_string(name_outputs).unwrap_or_else(|_| "{}".to_string()),
            code,
        );

        let start = std::time::Instant::now();

        // Resolve the bundled node binary once per process lifetime; cached via OnceLock.
        let node_bin = match node_bin {
            Some(path) => path,
            None => match NODE_BIN.get_or_init(resolve_node_bin) {
                Ok(path) => path.as_path(),
                Err(msg) => return NodeOutput::failure(
                    NodeError::unrecoverable("NODE_NOT_FOUND", msg.clone())
                ),
            },
        };

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
        // kill_on_drop ensures the subprocess is killed on ANY drop path
        // (workflow-level timeout, abort, panic-unwind), not just the per-node timeout branch.
        cmd.kill_on_drop(true);
        #[cfg(unix)]
        cmd.process_group(0);
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
        // In sandbox mode, clear the inherited environment so process.env
        // cannot expose server secrets. Re-add PATH (required for Node to find its own
        // binaries) and any operator-approved vars from --allow-env-vars.
        if sandbox_enabled {
            cmd.env_clear();
            if let Ok(path_val) = std::env::var("PATH") {
                cmd.env("PATH", path_val);
            }
            // __allowed_env_vars is injected by the executor from --allow-env-vars.
            if let Some(Value::Array(allowed)) = input.context.metadata.get("__allowed_env_vars") {
                for entry in allowed {
                    if let Some(var_name) = entry.as_str() {
                        if let Ok(val) = std::env::var(var_name) {
                            cmd.env(var_name, val);
                        }
                    }
                }
            }
        }
        cmd.stdin(Stdio::piped())
           .stdout(Stdio::piped())
           .stderr(Stdio::piped());

        // Apply OS-level resource limits on Linux in sandbox mode.
        // RLIMIT_AS: see sandbox_address_space_limit. RLIMIT_CPU: timeout_secs + 5, a backstop for busy-loops.
        // Safety: pre_exec runs between fork() and exec(). setrlimit(2) is listed
        // in POSIX as async-signal-safe. No allocations are made in the closure.
        #[cfg(target_os = "linux")]
        let spawn_result = if sandbox_enabled {
            let cpu_limit = (timeout_secs + 5) as libc::rlim_t;
            let configured_mb = input.context.metadata.get("__code_max_memory_mb")
                .and_then(|v| v.as_u64());
            let mem_limit = libc::rlim_t::try_from(sandbox_address_space_limit(configured_mb))
                .unwrap_or(libc::rlim_t::MAX);
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
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::error!(
                    "code_node: bundled Node.js binary did not spawn (ENOENT) at the resolved \
                     path: {}. The runtime is bundled with this install; this indicates a \
                     missing or corrupt bundled binary, not an absent system Node.js.",
                    e
                );
                return NodeOutput::failure(NodeError::unrecoverable(
                    "NODE_NOT_FOUND",
                    format!(
                        "Bundled Node.js runtime is missing or corrupt: {}. Reinstall or redeploy this application to restore it.",
                        e
                    )
                ));
            }
            Err(e) => return NodeOutput::failure(
                NodeError::unrecoverable(
                    "SANDBOX_INIT_FAILED",
                    format!("Code node sandbox failed to initialise: {}. \
                             Resource limits could not be applied — check that the process \
                             hard limits (RLIMIT_CPU, RLIMIT_AS) allow the requested values.", e)
                )
            ),
        };
        let group_guard = kill_group_on_drop(child.id());

        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            let _ = stdin.write_all(wrapper.as_bytes()).await;
        }

        // The result sits at the end of stdout, so the tail is kept when printed output overflows the cap.
        // stderr only ever feeds failure messages, which show its last few KB.
        let flood = FloodLimit::new(OUTPUT_FLOOD_BYTES);
        let mut stdout_task = tokio::spawn({
            let (reader, flood) = (child.stdout.take(), flood.clone());
            async move { drain_capped(reader, OUTPUT_CAP_BYTES, Keep::Tail, &flood).await }
        });
        let mut stderr_task = tokio::spawn({
            let (reader, flood) = (child.stderr.take(), flood.clone());
            async move { drain_capped(reader, STDERR_KEEP_BYTES, Keep::Tail, &flood).await }
        });

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            async {
                let status = tokio::select! {
                    s = child.wait() => s,
                    _ = flood.tripped() => return None,
                };
                let stdout_bytes = (&mut stdout_task).await.unwrap_or_default();
                let stderr_bytes = (&mut stderr_task).await.unwrap_or_default();
                if stdout_bytes.flooded || stderr_bytes.flooded { return None; }
                Some((status, stdout_bytes, stderr_bytes))
            },
        ).await;

        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(None) => {
                drop(group_guard);
                let _ = child.kill().await;
                stdout_task.abort();
                stderr_task.abort();
                NodeOutput::failure(NodeError::unrecoverable(
                    "OUTPUT_FLOOD",
                    format!(
                        "Code wrote more than {} MB to one output stream and was stopped. \
                         Print less, or return the data with output().",
                        OUTPUT_FLOOD_BYTES / (1024 * 1024)
                    ),
                ))
            }
            Err(_) => {
                drop(group_guard);
                let _ = child.kill().await;
                stdout_task.abort();
                stderr_task.abort();
                NodeOutput::failure(
                    NodeError::unrecoverable("TIMEOUT", format!("Code exceeded {}s timeout", timeout_secs))
                )
            }
            Ok(Some((Err(e), _, _))) => {
                group_guard.disarm();
                stdout_task.abort();
                stderr_task.abort();
                NodeOutput::failure(NodeError::unrecoverable("EXEC_ERROR", e.to_string()))
            }
            Ok(Some((Ok(status), stdout_cap, stderr_cap))) => {
                group_guard.disarm();
                let stdout = String::from_utf8_lossy(&stdout_cap.bytes).to_string();
                let stderr = failure_stderr(String::from_utf8_lossy(&stderr_cap.bytes).trim());

                let Some((printed, envelope)) = split_result(&stdout, &marker) else {
                    return NodeOutput::failure(if !status.success() {
                        NodeError::unrecoverable("RUNTIME_ERROR",
                            if stderr.is_empty() { "Code exited with non-zero status".to_string() }
                            else { stderr }
                        )
                    } else if stdout_cap.truncated {
                        NodeError::unrecoverable("RESULT_TOO_LARGE",
                            "The result and printed output together exceeded 10 MB, so the result was cut off. \
                             Return a smaller value from output() or print less."
                        )
                    } else {
                        NodeError::unrecoverable("NO_RESULT",
                            "Code ended without returning a result. If it calls process.exit(), remove that call: \
                             output(value) is only delivered when the script finishes on its own."
                        )
                    });
                };

                match serde_json::from_str::<Value>(envelope) {
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
                                    "{}\n\nNote: Code nodes run as ES modules. \
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
                            "stdout":      printed,
                            "truncated":   stdout_cap.truncated,
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

/// Splits captured stdout at the last result marker into the text the code printed
/// and the JSON envelope that follows. `None` when the marker is absent.
fn split_result<'a>(stdout: &'a str, marker: &str) -> Option<(&'a str, &'a str)> {
    let at = stdout.rfind(marker)?;
    let printed = &stdout[..at];
    let printed = printed.strip_suffix('\n').unwrap_or(printed);
    Some((printed, stdout[at + marker.len()..].trim()))
}

/// RLIMIT_AS for a sandboxed Code node: the memory the script may use plus the
/// address space Node.js itself reserves at startup.
#[cfg(target_os = "linux")]
fn sandbox_address_space_limit(configured_mb: Option<u64>) -> u64 {
    let script_mb = configured_mb.unwrap_or(DEFAULT_CODE_MEMORY_MB).min(MAX_CODE_MEMORY_MB);
    (script_mb + NODE_ADDRESS_SPACE_OVERHEAD_MB) * 1024 * 1024
}

// ── Node binary resolution ───────────────────────────────────────────────────────
// Bundled-only: no PATH search, no `which`/`where`, ever. The desktop app's Tauri
// sidecar and the server's Docker image both place the bundled runtime next to
// their own executable under this same filename, so one resolver covers both.

/// Filename of the bundled runtime next to the current executable.
fn bundled_bin_name() -> &'static str {
    if cfg!(windows) { "node-bundled.exe" } else { "node-bundled" }
}

/// Resolves the bundled node binary's path: an explicit override env var if set,
/// otherwise the bundled binary next to the running executable. Never touches PATH.
fn resolve_node_bin() -> Result<PathBuf, String> {
    resolve_node_bin_from(std::env::var("AERINI_NODE_BIN").ok(), std::env::current_exe())
}

/// Cached outcome of `bundled_node_health_check`. A container image or installed
/// app doesn't repair itself mid-process, so a spawn failure or success is stable
/// for the process's lifetime — cached once to avoid spawning a subprocess on
/// every caller (e.g. a polled `/api/health` route).
static NODE_HEALTH: OnceLock<Result<String, String>> = OnceLock::new();

/// Confirms the bundled Node.js runtime actually spawns and reports a version,
/// beyond just resolving a path for it (`resolve_node_bin` never touches disk).
/// Returns the reported version on success, or a message describing why the
/// spawn failed — a resolvable-but-unspawnable binary means the bundle itself
/// is missing or corrupt, since resolution never depends on PATH.
pub fn bundled_node_health_check() -> Result<String, String> {
    NODE_HEALTH.get_or_init(|| {
        let path = resolve_node_bin()?;
        bundled_node_health_check_from(&path)
    }).clone()
}

/// Spawns the given path with `--version` and reports the outcome. Split out from
/// `bundled_node_health_check` so tests can exercise it without the process-wide
/// `OnceLock` cache above.
fn bundled_node_health_check_from(path: &std::path::Path) -> Result<String, String> {
    let output = std::process::Command::new(path)
        .arg("--version")
        .output()
        .map_err(|e| format!(
            "Bundled Node.js runtime is missing or corrupt: {}. Reinstall or redeploy this application to restore it.",
            e
        ))?;
    if !output.status.success() {
        return Err(format!(
            "Bundled Node.js runtime exited with a non-zero status while checking its version ({}). Reinstall or redeploy this application to restore it.",
            output.status
        ));
    }
    String::from_utf8(output.stdout)
        .map(|s| s.trim().to_string())
        .map_err(|e| format!("Bundled Node.js runtime returned non-UTF8 version output: {}.", e))
}

fn resolve_node_bin_from(
    env_override: Option<String>,
    current_exe: std::io::Result<PathBuf>,
) -> Result<PathBuf, String> {
    if let Some(path) = env_override.filter(|p| !p.trim().is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let exe_path = current_exe.map_err(|e| {
        format!(
            "Could not determine the running executable's location to find the bundled \
             Node.js runtime: {}. This is an environment/packaging problem, not a missing \
             Node.js install.",
            e
        )
    })?;
    let exe_dir = exe_path.parent().ok_or_else(|| {
        format!(
            "Running executable path '{}' has no parent directory; cannot locate the \
             bundled Node.js runtime next to it.",
            exe_path.display()
        )
    })?;
    Ok(exe_dir.join(bundled_bin_name()))
}

// ── Sandbox helpers ────────────────────────────────────────────────────────────

/// Write the sandbox ESM loader script to a named temp file.
/// The caller is responsible for keeping the returned `NamedTempFile` alive until
/// the Node.js subprocess exits — drop = delete.
#[cfg(not(windows))]
fn write_sandbox_loader() -> std::io::Result<tempfile::NamedTempFile> {
    use std::io::Write;
    let mut f = tempfile::Builder::new()
        .prefix("aerini-sandbox-")
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
            resolved_credentials: std::collections::HashMap::new(),
            cancel_token: None,
            node_id:      "test".to_string(),
            workflow_id:  "wf".to_string(),
            execution_id: "exec".to_string(),
            input: json!({ "code": code }),
            context: ExecutionContext {
                variables:    HashMap::new(),
                node_outputs: std::sync::Arc::new(HashMap::new()),
                metadata,
                ..Default::default()
            },
        }
    }

    #[test]
    #[cfg(not(windows))]
    fn sandbox_loader_blocks_module_builtin() {
        assert!(SANDBOX_LOADER_CONTENT.contains("'node:module'"));
        assert!(SANDBOX_LOADER_CONTENT.contains("'module'"));
    }

    #[test]
    #[cfg(not(windows))]
    fn sandbox_loader_blocks_v8_and_inspector() {
        // v8.writeHeapSnapshot(path) writes an attacker-influenced file to an
        // arbitrary path; inspector.open() starts a debugger whose Runtime.evaluate
        // bypasses --disallow-code-generation-from-strings. Both bypass the loader
        // hook's module-name blocklist without touching fs/child_process at all,
        // so both must be blocked by name like every other dangerous builtin.
        assert!(SANDBOX_LOADER_CONTENT.contains("'node:v8'"));
        assert!(SANDBOX_LOADER_CONTENT.contains("'v8'"));
        assert!(SANDBOX_LOADER_CONTENT.contains("'node:inspector'"));
        assert!(SANDBOX_LOADER_CONTENT.contains("'inspector'"));
    }

    #[test]
    fn sandbox_globals_hardening_overrides_get_builtin_module() {
        assert!(SANDBOX_GLOBALS_HARDENING.contains("process.getBuiltinModule = "));
        assert!(SANDBOX_GLOBALS_HARDENING.contains("'module'"));
        assert!(SANDBOX_GLOBALS_HARDENING.contains("'node:child_process'"));
    }

    #[test]
    fn sandbox_globals_hardening_deletes_raw_binding_access() {
        // process.binding('spawn_sync').spawn(...) and process.binding('fs') hand back
        // raw internal Node bindings that bypass the ESM loader hook entirely -- verified
        // empirically to execute arbitrary OS commands when only the module-import
        // blocklist was in place. process.dlopen/_linkedBinding load native addons
        // directly, same bypass class. All three must be removed unconditionally,
        // since neither reaches the module-name blocklist above.
        assert!(SANDBOX_GLOBALS_HARDENING.contains("delete process.binding;"));
        assert!(SANDBOX_GLOBALS_HARDENING.contains("delete process._linkedBinding;"));
        assert!(SANDBOX_GLOBALS_HARDENING.contains("delete process.dlopen;"));
        assert!(SANDBOX_GLOBALS_HARDENING.contains("'node:v8'"));
        assert!(SANDBOX_GLOBALS_HARDENING.contains("'node:inspector'"));
    }

    #[tokio::test]
    async fn code_disabled_flag_returns_error() {
        let out = CodeNode.execute(make_input("console.log('hi')", true)).await;
        assert!(!out.success);
        let err = out.error.expect("expected NodeError");
        assert_eq!(err.code, "CODE_DISABLED");
        assert!(!err.recoverable);
    }

    #[test]
    fn resolve_node_bin_prefers_env_override() {
        let resolved = resolve_node_bin_from(
            Some("/custom/path/my-node".to_string()),
            Ok(PathBuf::from("/usr/local/bin/aerini-server")),
        );
        assert_eq!(resolved, Ok(PathBuf::from("/custom/path/my-node")));
    }

    #[test]
    fn resolve_node_bin_falls_back_to_bundled_name_next_to_exe() {
        let resolved = resolve_node_bin_from(None, Ok(PathBuf::from("/usr/local/bin/aerini-server")));
        assert_eq!(
            resolved,
            Ok(PathBuf::from("/usr/local/bin").join(bundled_bin_name()))
        );
    }

    #[test]
    fn resolve_node_bin_errors_when_exe_path_unavailable_rather_than_falling_back_to_path_search() {
        // A bare filename here would let Command::new()'s own OS-level exec fall back to
        // a PATH search, which is the one thing this bundled-only resolver must never do.
        let err = std::io::Error::other("current_exe unavailable");
        let resolved = resolve_node_bin_from(None, Err(err));
        assert!(resolved.is_err());
    }

    // No normal-case (successful spawn) test: it would require a real executable
    // present in the test environment, which is exactly the kind of
    // environment-dependency this bundled-only design exists to avoid. The
    // NotFound case below is this function's entire reason for existing —
    // distinguishing "resolves to a path" from "actually runs."
    #[test]
    fn bundled_node_health_check_reports_missing_or_corrupt_on_spawn_failure() {
        let err = bundled_node_health_check_from(std::path::Path::new(
            "/definitely/does/not/exist/node-bundled",
        ));
        assert!(err.is_err());
        assert!(err.unwrap_err().contains("missing or corrupt"));
    }

    #[tokio::test]
    async fn missing_code_returns_error() {
        let input = NodeInput {
            resolved_credentials: std::collections::HashMap::new(),
            cancel_token: None,
            node_id:      "test".to_string(),
            workflow_id:  "wf".to_string(),
            execution_id: "exec".to_string(),
            input: json!({}),
            context: ExecutionContext {
                variables:    HashMap::new(),
                node_outputs: std::sync::Arc::new(HashMap::new()),
                metadata:     HashMap::new(),
                ..Default::default()
            },
        };
        let out = CodeNode.execute(input).await;
        assert!(!out.success);
        let err = out.error.expect("expected NodeError");
        assert_eq!(err.code, "MISSING_CODE");
    }

    macro_rules! require_node {
        () => {
            match test_node_bin() {
                Some(path) => path,
                None => {
                    eprintln!("skipped: no Node.js binary (set AERINI_NODE_BIN)");
                    return;
                }
            }
        };
    }

    fn test_node_bin() -> Option<PathBuf> {
        if let Some(path) = std::env::var_os("AERINI_NODE_BIN").filter(|p| !p.is_empty()) {
            return Some(PathBuf::from(path));
        }
        std::process::Command::new("node")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
            .then(|| PathBuf::from("node"))
    }

    async fn run_code(node: &Path, config: Value, metadata: HashMap<String, Value>) -> NodeOutput {
        let input = NodeInput {
            resolved_credentials: HashMap::new(),
            cancel_token: None,
            node_id:      "test".to_string(),
            workflow_id:  "wf".to_string(),
            execution_id: "exec".to_string(),
            input: config,
            context: ExecutionContext { metadata, ..Default::default() },
        };
        CodeNode.execute_with(input, Some(node)).await
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failure_message_shows_a_redacted_tail_of_stderr_not_all_of_it() {
        let node = require_node!();
        let code = "console.error('x'.repeat(300000)); \
                    console.error('cannot reach postgres://app:hunter2@db/prod'); \
                    console.error('real error'); process.exit(1);";
        let out = run_code(&node, json!({ "code": code }), HashMap::new()).await;
        let err = out.error.expect("expected NodeError");
        assert_eq!(err.code, "RUNTIME_ERROR");
        assert!(err.message.len() < 5_000, "len {}", err.message.len());
        assert!(!err.message.contains("hunter2"), "got: {}", err.message);
        assert!(err.message.contains("[REDACTED]"), "got: {}", err.message);
        assert!(err.message.trim_end().ends_with("real error"), "got: {}", err.message);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn endless_output_is_stopped_with_output_flood_before_the_timeout() {
        let node = require_node!();
        let started = std::time::Instant::now();
        let code = "const chunk = 'x'.repeat(65536); while (true) process.stdout.write(chunk);";
        let out = run_code(&node, json!({ "code": code, "timeout_secs": 60 }), HashMap::new()).await;
        let err = out.error.expect("expected NodeError");
        assert_eq!(err.code, "OUTPUT_FLOOD", "{}", err.message);
        assert!(started.elapsed() < std::time::Duration::from_secs(45));
    }

    #[tokio::test]
    async fn printing_before_and_after_output_keeps_the_result_and_the_printed_text() {
        let node = require_node!();
        let code = "console.log('before'); output({ a: 1 }); console.log('after');";
        let out = run_code(&node, json!({ "code": code }), HashMap::new()).await;
        assert!(out.success, "{:?}", out.error);
        let o = out.output.unwrap();
        assert_eq!(o["result"], json!({ "a": 1 }));
        assert_eq!(o["stdout"], "before\nafter\n");
        assert_eq!(o["truncated"], false);
    }

    #[tokio::test]
    async fn exiting_before_the_script_ends_reports_no_result() {
        let node = require_node!();
        let out = run_code(&node, json!({ "code": "output(1); process.exit(0);" }), HashMap::new()).await;
        let err = out.error.expect("expected NodeError");
        assert_eq!(err.code, "NO_RESULT");
        assert!(!err.recoverable);
    }

    #[tokio::test]
    async fn printed_output_beyond_the_cap_keeps_the_result_and_flags_truncated() {
        let node = require_node!();
        let code = "console.log('x'.repeat(11 * 1024 * 1024)); output(7);";
        let out = run_code(&node, json!({ "code": code, "timeout_secs": 30 }), HashMap::new()).await;
        assert!(out.success, "{:?}", out.error);
        let o = out.output.unwrap();
        assert_eq!(o["result"], 7);
        assert_eq!(o["truncated"], true);
        assert!(o["stdout"].as_str().unwrap().len() <= OUTPUT_CAP_BYTES);
    }

    #[tokio::test]
    async fn result_beyond_the_cap_reports_result_too_large() {
        let node = require_node!();
        let code = "output('x'.repeat(11 * 1024 * 1024));";
        let out = run_code(&node, json!({ "code": code, "timeout_secs": 30 }), HashMap::new()).await;
        assert_eq!(out.error.map(|e| e.code), Some("RESULT_TOO_LARGE".to_string()));
    }

    #[tokio::test]
    async fn thrown_values_become_runtime_errors() {
        let node = require_node!();
        for (code, want) in [("throw new Error('boom')", "boom"), ("throw null", "null")] {
            let out = run_code(&node, json!({ "code": code }), HashMap::new()).await;
            let err = out.error.expect("expected NodeError");
            assert_eq!(err.code, "RUNTIME_ERROR", "{code}");
            assert_eq!(err.message, want, "{code}");
        }
    }

    #[tokio::test]
    async fn timeout_of_zero_is_raised_to_one_second() {
        let node = require_node!();
        let code = "await new Promise(r => setTimeout(r, 100)); output(1);";
        let out = run_code(&node, json!({ "code": code, "timeout_secs": 0 }), HashMap::new()).await;
        assert!(out.success, "{:?}", out.error);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn sandboxed_node_starts_within_the_address_space_limit() {
        let node = require_node!();
        let mut metadata = HashMap::new();
        metadata.insert("__code_sandbox".to_string(), Value::Bool(true));
        let out = run_code(&node, json!({ "code": "output(1 + 1);" }), metadata).await;
        assert!(out.success, "{:?}", out.error);
        assert_eq!(out.output.unwrap()["result"], 2);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn sandbox_address_space_limit_adds_runtime_overhead_and_caps_configured_memory() {
        let mib = 1024 * 1024;
        let overhead: u64 = if cfg!(target_arch = "aarch64") { 4096 } else { 2048 };
        assert_eq!(sandbox_address_space_limit(None), (512 + overhead) * mib);
        assert_eq!(sandbox_address_space_limit(Some(100)), (100 + overhead) * mib);
        assert_eq!(sandbox_address_space_limit(Some(u64::MAX)), (16_384 + overhead) * mib);
    }
}
