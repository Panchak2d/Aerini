use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;
use tokio::process::Command;

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortArity, PortDefinition, PortPosition};

use super::shell::failure_stderr;
use super::util::{cfg_u64_opt, drain_capped, kill_tree_on_drop, FloodLimit, Keep, OUTPUT_CAP_BYTES, OUTPUT_FLOOD_BYTES};

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
/// Guards the partial-sandbox warning so it fires once per process, not once per execution.
#[cfg(not(target_os = "linux"))]
static PARTIAL_SANDBOX_WARNED: OnceLock<()> = OnceLock::new();
/// Set once the sandbox flags have been seen to enforce their restrictions on this host.
static SANDBOX_VERIFIED: AtomicBool = AtomicBool::new(false);

/// Node writes to a pipe asynchronously on Unix: a write the pipe cannot take is queued in
/// Node's memory, and anything still queued is lost when the script calls `process.exit()`. Blocking mode
/// makes each write wait for this process to read it, so a script cannot grow Node's memory faster than it
/// is drained and a failing script's last stderr lines always arrive.
const STDIO_BLOCKING: &str = "for (const s of [process.stdout, process.stderr]) { try { s._handle?.setBlocking?.(true); } catch {} }\n";

/// Script text injected ahead of user code when sandbox mode is active. It is the same on every
/// platform: it travels over stdin with the script, so there is no temp file, file URL or path quoting.
///
/// Blocked builtins, by `import`, `import()` and `process.getBuiltinModule()`: `child_process`, `fs`,
/// `net`, `http*`, `dgram`, `dns`, `os`, `cluster`, `worker_threads`, `vm`, `repl`, `domain`, `tls`,
/// `wasi`, `trace_events`, `sqlite` and `test` reach subprocesses, files and raw sockets.
/// `module` hands back `createRequire()`, a `require()` that reaches every other entry. `v8` writes
/// heap snapshots to any path. `inspector` runs code through a debugger session, bypassing
/// `--disallow-code-generation-from-strings`. The resolve hook judges both the specifier and the
/// resolved `node:` URL, so alternate spellings cannot slip past a name match.
///
/// `fetch`, `WebSocket` and `XMLHttpRequest` are globals that reach the network without an import.
/// `process.binding()`, `process._linkedBinding()` and `process.dlopen()` return raw internal
/// bindings or native addons, so they are deleted rather than name-filtered, as are `process.kill`
/// and `process.report`.
///
/// The hooks are registered inside a block so no binding for `registerHooks` is left in scope for
/// user code, and `module` itself is unreachable from the script.
///
/// Node's permission model (`--permission`, see `apply_sandbox_flags`) enforces the file system,
/// child process, worker, native addon and WASI limits again in Node itself. It does not restrict
/// the network, which is why the import and global blocks above remain.
const SANDBOX_PRELUDE: &str = r#"{
  const blockedModules = new Set([
    'child_process', 'fs', 'fs/promises', 'net', 'http', 'https', 'http2', 'dgram',
    'dns', 'dns/promises', 'os', 'cluster', 'worker_threads', 'vm', 'repl', 'domain',
    'module', 'v8', 'inspector', 'inspector/promises', 'tls', 'wasi', 'trace_events',
    'sqlite', 'test', 'test/reporters',
  ]);
  const isBlocked = (id) => blockedModules.has(id.startsWith('node:') ? id.slice(5) : id);
  const refusal = (id, how) => new Error(
    `[Aerini sandbox] ${how} of '${id}' is blocked. ` +
    `Filesystem, module-imported network access, subprocess access, CommonJS require() ` +
    `(via the 'module' builtin), heap-snapshot file writes (via 'v8'), and debugger-based ` +
    `code execution (via 'inspector') are all blocked in sandboxed Code nodes. ` +
    `Global fetch() and WebSocket are also disabled. ` +
    `Use the HTTP Request node for outbound HTTP, or disable sandboxing for trusted deployments.`
  );
  const { registerHooks } = process.getBuiltinModule('node:module');
  registerHooks({
    resolve(specifier, context, nextResolve) {
      if (isBlocked(specifier)) throw refusal(specifier, 'Import');
      const resolved = nextResolve(specifier, context);
      if (typeof resolved.url === 'string' && isBlocked(resolved.url)) throw refusal(specifier, 'Import');
      return resolved;
    },
  });
  const originalGetBuiltinModule = process.getBuiltinModule.bind(process);
  process.getBuiltinModule = (id) => {
    if (typeof id === 'string' && isBlocked(id)) throw refusal(id, 'Access');
    return originalGetBuiltinModule(id);
  };
}
delete globalThis.fetch;
delete globalThis.WebSocket;
delete globalThis.XMLHttpRequest;
delete process.binding;
delete process._linkedBinding;
delete process.dlopen;
delete process.kill;
delete process.report;
{
  const refuseConnect = function connect() {
    throw new Error(
      "[Aerini sandbox] Outbound sockets are blocked in sandboxed Code nodes. " +
      "Use the HTTP Request node for outbound HTTP, or disable sandboxing for trusted deployments."
    );
  };
  for (const stream of [process.stdout, process.stderr]) {
    for (let proto = Object.getPrototypeOf(stream); proto && proto !== Object.prototype; proto = Object.getPrototypeOf(proto)) {
      if (Object.prototype.hasOwnProperty.call(proto, 'connect') && proto.connect !== refuseConnect) {
        Object.defineProperty(proto, 'connect', { value: refuseConnect, writable: false, configurable: false });
      }
    }
  }
}
"#;

const SANDBOX_PROBE_MARKER: &str = "__AERINI_SANDBOX_PROBE__";

/// Runs after `SANDBOX_PRELUDE` with the real sandbox flags and reports every restriction that
/// failed to hold, as a JSON array. An empty array means the sandbox is enforced on this host.
const SANDBOX_PROBE: &str = r#"
const unenforced = [];
for (const id of ['node:child_process', 'node:fs', 'fs', 'node:module', 'node:net']) {
  try { await import(id); unenforced.push('import ' + id); } catch {}
}
for (const id of ['node:fs', 'child_process']) {
  try { process.getBuiltinModule(id); unenforced.push('getBuiltinModule ' + id); } catch {}
}
try {
  const probeSocket = new (process.stdout.constructor)();
  probeSocket.on('error', () => {});
  probeSocket.connect(1, '127.0.0.1');
  probeSocket.destroy();
  unenforced.push('socket connect');
} catch {}
if (typeof fetch !== 'undefined') unenforced.push('fetch');
if (typeof process.binding !== 'undefined') unenforced.push('process.binding');
if (process.permission?.has('fs.read') !== false) unenforced.push('permission model');
process.stdout.write('\n__AERINI_SANDBOX_PROBE__' + JSON.stringify(unenforced) + '\n');
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
                    "description": "Present and true on macOS and Windows when --code-sandbox is active. Module import and permission restrictions are enforced but CPU/memory resource limits (setrlimit) are Linux-only and not applied."
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

        let sandbox_prelude = if sandbox_enabled { SANDBOX_PRELUDE } else { "" };

        let empty_obj = serde_json::Value::Object(Default::default());
        let direct_input = input.context.metadata.get("__direct_input").unwrap_or(&empty_obj);
        let name_outputs = input.context.metadata.get("__node_name_outputs").unwrap_or(&empty_obj);
        let unsafe_integers = count_unsafe_integers(direct_input) + count_unsafe_integers(name_outputs);
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
            sandbox_prelude,
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

        if sandbox_enabled {
            if let Err(reason) = verify_sandbox(node_bin).await {
                return NodeOutput::failure(NodeError::unrecoverable(
                    "SANDBOX_UNAVAILABLE",
                    format!(
                        "The Code node sandbox cannot be enforced on this host: {}. \
                         Refusing to run unsandboxed while --code-sandbox is active. \
                         Remove --code-sandbox only for trusted deployments.",
                        reason
                    ),
                ));
            }
        }

        let mut cmd = Command::new(node_bin);
        // kill_on_drop ensures the subprocess is killed on ANY drop path
        // (workflow-level timeout, abort, panic-unwind), not just the per-node timeout branch.
        cmd.kill_on_drop(true);
        #[cfg(unix)]
        cmd.process_group(0);
        cmd.arg("--input-type=module");
        if sandbox_enabled {
            apply_sandbox_flags(&mut cmd);
            apply_sandbox_env(&mut cmd);
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
        } else {
            super::util::withhold_server_env(&mut cmd, &input.context.metadata);
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
        // Off Linux the module and permission restrictions apply but the setrlimit CPU/memory
        // caps do not. Surfaced in node output as `_sandbox_partial: true` so API callers can
        // detect and alert.
        #[cfg(target_os = "linux")]
        let sandbox_partial = false;
        #[cfg(not(target_os = "linux"))]
        let sandbox_partial = sandbox_enabled;

        #[cfg(not(target_os = "linux"))]
        let spawn_result = {
            if sandbox_enabled {
                PARTIAL_SANDBOX_WARNED.get_or_init(|| {
                    tracing::warn!(
                        "Code node sandbox is PARTIAL on {}. \
                         Module import and permission restrictions (fs/net/child_process) are \
                         enforced, but CPU and memory resource limits (setrlimit) are Linux-only \
                         and are NOT applied. A runaway script can exhaust system resources. \
                         The per-node timeout_secs (max 60 s) is the only effective resource \
                         cap on this platform. For full sandboxing, deploy on Linux.",
                        std::env::consts::OS
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
                    format!("Code node sandbox failed to initialise: {}.{}", e,
                        if cfg!(target_os = "linux") {
                            " Resource limits could not be applied — check that the process \
                             hard limits (RLIMIT_CPU, RLIMIT_AS) allow the requested values."
                        } else {
                            ""
                        })
                )
            ),
        };
        let group_guard = kill_tree_on_drop(&child);

        if let Some(mut stdin) = child.stdin.take() {
            let script = wrapper.into_bytes();
            tokio::spawn(async move {
                use tokio::io::AsyncWriteExt;
                let _ = stdin.write_all(&script).await;
            });
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
                        let mut logs = vec![format!("Code executed in {}ms", duration_ms)];
                        if unsafe_integers > 0 {
                            logs.push(format!(
                                "PRECISION: {unsafe_integers} integer(s) in the input or context are beyond 2^53 and JavaScript rounds them; pass such values as strings to keep every digit"
                            ));
                        }
                        NodeOutput::success_with_logs(out, logs)
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

const JS_SAFE_INTEGER: u64 = 1 << 53;

/// Counts integers whose magnitude is above 2^53, which JavaScript rounds to
/// the nearest double when the script reads them.
fn count_unsafe_integers(value: &Value) -> usize {
    let mut count = 0;
    let mut stack = vec![value];
    while let Some(v) = stack.pop() {
        match v {
            Value::Number(n) => {
                let beyond = n.as_u64().is_some_and(|u| u > JS_SAFE_INTEGER)
                    || n.as_i64().is_some_and(|i| i.unsigned_abs() > JS_SAFE_INTEGER);
                count += usize::from(beyond);
            }
            Value::Array(items) => stack.extend(items.iter()),
            Value::Object(map) => stack.extend(map.values()),
            _ => {}
        }
    }
    count
}

/// Flags that make a Node.js process enforce the sandbox: no string-to-code generation, and the
/// permission model with nothing granted, which denies file system access, child processes, workers,
/// native addons and WASI inside Node itself. The same flags apply on every platform.
fn apply_sandbox_flags(cmd: &mut Command) {
    cmd.arg("--disallow-code-generation-from-strings");
    cmd.arg("--permission");
}

/// Clears the inherited environment so `process.env` cannot expose server secrets, then restores
/// `PATH`, which Node needs to find its own binaries, and on Windows `SystemRoot` and `windir`,
/// without which Node cannot start its networking and crypto stacks.
fn apply_sandbox_env(cmd: &mut Command) {
    cmd.env_clear();
    #[cfg(windows)]
    for name in ["SystemRoot", "windir"] {
        if let Ok(value) = std::env::var(name) {
            cmd.env(name, value);
        }
    }
    if let Ok(path_val) = std::env::var("PATH") {
        cmd.env("PATH", path_val);
    }
}

/// Runs the sandbox prelude with the real sandbox flags and confirms the restrictions hold, once
/// per process. A host where they do not hold (an unsupported `--permission` flag, a hook that
/// does not run) gets an error instead of a Code node that runs unsandboxed. Failures are not
/// cached, so a transient spawn error does not disable the sandbox until restart.
async fn verify_sandbox(node_bin: &Path) -> Result<(), String> {
    if SANDBOX_VERIFIED.load(Ordering::Acquire) {
        return Ok(());
    }
    probe_sandbox(node_bin).await?;
    SANDBOX_VERIFIED.store(true, Ordering::Release);
    Ok(())
}

async fn probe_sandbox(node_bin: &Path) -> Result<(), String> {
    let mut cmd = Command::new(node_bin);
    cmd.kill_on_drop(true).arg("--input-type=module");
    apply_sandbox_flags(&mut cmd);
    apply_sandbox_env(&mut cmd);
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("Node.js did not start for the sandbox check ({e})"))?;
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let script = format!("{SANDBOX_PRELUDE}{SANDBOX_PROBE}");
        let _ = stdin.write_all(script.as_bytes()).await;
    }
    let output = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output())
        .await
        .map_err(|_| "the sandbox check timed out".to_string())?
        .map_err(|e| format!("the sandbox check could not be read ({e})"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    match stdout.lines().find_map(|line| line.strip_prefix(SANDBOX_PROBE_MARKER)) {
        Some("[]") => Ok(()),
        Some(unenforced) => Err(format!("these restrictions did not hold: {unenforced}")),
        None => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let tail: String = stderr.trim().chars().rev().take(300).collect::<Vec<_>>().into_iter().rev().collect();
            Err(format!("the sandbox check did not finish (exit {:?}): {}", output.status.code(), tail))
        }
    }
}

// tokio::process::Command exposes pre_exec() directly on Unix — no CommandExt import needed.

#[cfg(test)]
mod tests {
    #[test]
    fn only_integers_past_2_pow_53_are_counted() {
        let value = json!({"a": 42, "b": [9007199254740992u64, 9007199254740993u64], "c": {"d": -9007199254740993i64, "e": 1.5e300}});
        assert_eq!(count_unsafe_integers(&value), 2);
        assert_eq!(count_unsafe_integers(&json!({"n": 7, "list": [1, 2.5]})), 0);
    }

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
    fn sandbox_prelude_blocks_every_dangerous_builtin_by_name() {
        let list_start = SANDBOX_PRELUDE.find("new Set([").expect("blocklist start");
        let list = &SANDBOX_PRELUDE[list_start..list_start + SANDBOX_PRELUDE[list_start..].find("]);").expect("blocklist end")];
        for id in [
            "child_process", "fs", "fs/promises", "net", "http", "https", "http2", "dgram", "dns",
            "dns/promises", "os", "cluster", "worker_threads", "vm", "repl", "domain", "module", "v8",
            "inspector", "inspector/promises", "tls", "wasi", "trace_events", "sqlite", "test",
            "test/reporters",
        ] {
            assert!(list.contains(&format!("'{id}'")), "blocklist misses {id}");
        }
        assert!(SANDBOX_PRELUDE.contains("registerHooks("));
        assert!(SANDBOX_PRELUDE.contains("process.getBuiltinModule = "));
    }

    #[test]
    fn sandbox_prelude_deletes_raw_binding_and_network_globals() {
        for removed in [
            "fetch", "WebSocket", "XMLHttpRequest", "process.binding", "process._linkedBinding",
            "process.dlopen", "process.kill", "process.report",
        ] {
            let removal = if removed.starts_with("process.") {
                format!("delete {removed};")
            } else {
                format!("delete globalThis.{removed};")
            };
            assert!(SANDBOX_PRELUDE.contains(&removal), "prelude keeps {removed}");
        }
    }

    #[test]
    fn sandbox_flags_apply_the_same_arguments_on_every_platform() {
        let mut cmd = Command::new("node");
        apply_sandbox_flags(&mut cmd);
        let args: Vec<_> = cmd.as_std().get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args, ["--disallow-code-generation-from-strings", "--permission"]);
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
        let code = "const w = (s) => new Promise((r) => process.stderr.write(s + '\\n', r)); \
                    await w('x'.repeat(300000)); \
                    await w('cannot reach postgres://app:hunter2@db/prod'); \
                    await w('real error'); process.exit(1);";
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
        let code = "const chunk = 'x'.repeat(65536); \
                    while (true) await new Promise((r) => process.stdout.write(chunk, r));";
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

    #[tokio::test]
    async fn sandboxed_code_blocks_imports_and_reports_partial_off_linux() {
        let node = require_node!();
        let mut metadata = HashMap::new();
        metadata.insert("__code_sandbox".to_string(), Value::Bool(true));
        let code = r#"
            const blocked = [];
            for (const id of ['node:fs', 'fs', 'node:child_process', 'node:module', 'node:net']) {
              try { await import(id); } catch { blocked.push(id); }
            }
            try { process.getBuiltinModule('node:fs'); } catch { blocked.push('getBuiltinModule'); }
            output({ blocked: blocked.length, fetch: typeof fetch });
        "#;
        let out = run_code(&node, json!({ "code": code }), metadata).await;
        assert!(out.success, "{:?}", out.error);
        let data = out.output.unwrap();
        assert_eq!(data["result"], json!({ "blocked": 6, "fetch": "undefined" }));
        assert_eq!(data.get("_sandbox_partial").is_some(), !cfg!(target_os = "linux"));
    }

    #[tokio::test]
    async fn sandbox_probe_accepts_a_node_that_enforces_and_rejects_one_that_does_not() {
        let node = require_node!();
        probe_sandbox(&node).await.expect("real Node.js enforces the sandbox flags");
        #[cfg(unix)]
        {
            let err = probe_sandbox(Path::new("/bin/true")).await.unwrap_err();
            assert!(err.contains("did not finish"), "{err}");
        }
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
