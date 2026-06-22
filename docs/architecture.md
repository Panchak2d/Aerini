# Architecture

How Aerini is structured internally, why it's split the way it is, and how execution actually works.

This page is for contributors and developers embedding `aerini-engine`. It is not required reading for general use.

---

## The three-crate split

```
aerini-engine/   — core library: workflow model, executor, scheduler, node registry, 39 built-in nodes, WASM plugin loader
src-tauri/      — Tauri shell: IPC commands, system tray, app lifecycle
aerini-server/   — headless binary: HTTP API, serve mode CLI
```

`aerini-engine` has no Tauri dependency. It's a pure Rust library. Both `src-tauri` and `aerini-server` depend on it, but neither appears in the other's dependency tree. This makes the execution engine independently testable and makes the server binary possible without bundling a WebView runtime.

The TypeScript frontend (under `src/`) is a vanilla TS/Vite application with no frontend framework. It communicates with the Tauri shell via IPC commands defined in `src-tauri/src/commands/` — see the [Desktop IPC Reference](desktop-ipc-reference.md) for the full list.

---

## The EventSink trait

The decoupling point between the engine and both runtimes is `EventSink`:

```rust
trait EventSink: Send + Sync {
    fn emit_node_status(&self, workflow_id: &str, node_id: &str, status: &str);
    fn emit_scheduler_status(&self, event: SchedulerStatusEvent);
}
```

The Tauri shell implements `EventSink` by emitting events to the frontend window. The frontend's `listenNodeStatus` subscription picks them up and turns nodes green or red on the canvas in real time.

`aerini-server` implements the same trait by broadcasting events over the SSE endpoint at `/api/events`. A client subscribed to that stream sees the same event shape.

The executor has no idea which runtime it's in — it calls `EventSink` and the runtime decides what happens. This is why scheduled background runs animate on the canvas exactly like manual runs: same executor, same events, same handler.

---

## Workflow execution

A workflow is a directed acyclic graph. `WorkflowNode` holds the node type ID and its static config. `WorkflowEdge` holds the connection between nodes, including optional `on_success` and `on_failure` routing.

When a run is triggered, `WorkflowExecutor::run()` does the following:

1. Builds a topologically sorted execution order using `petgraph`. Validates for cycles, dangling edge references, and the presence of at least one trigger node.
2. Walks the graph. For each node:
   - Resolves all `{{...}}` expressions in the node's config against the current `ExecutionContext`, which accumulates node outputs as the run progresses.
   - Resolves credential IDs to decrypted values via `CredentialResolver`.
   - Calls `node.execute(NodeInput)` — an async dispatch through `Arc<dyn Node>`.
   - Stores the `NodeOutput` in the context for downstream nodes.
   - Emits a node-status event before and after execution.
3. Returns a `WorkflowResult` containing all node outputs, all log entries, and a success flag.

The executor supports two modes. By default, nodes execute one at a time in topological order. When `parallel_execution: true` is set on a workflow, independent branches — nodes whose upstream dependencies have all completed — run concurrently via Tokio tasks, bounded by `max_concurrent_nodes` (default: 8).

Concurrent-run control uses two guards acquired before any executor is constructed:
- **Global semaphore** (`run_semaphore`) — limits total simultaneous executions. Default: 16. Configurable with `--max-concurrent-runs`. Acquiring this semaphore queues the run rather than rejecting it.
- **Per-workflow mutex** (`exec_lock`) — ensures only one instance of a given workflow runs at a time. If the lock can't be acquired, the run is skipped with a "previous run in progress" log entry.

---

## The scheduler daemon

`SchedulerDaemon` is the background run engine. In the desktop app, it lives as Tauri state, initialized at startup. Starting a background run calls `start_job()`, which spawns a Tokio task for that workflow. The task owns a loop:

1. Extract the trigger (Schedule or Webhook) from the workflow.
2. Sleep the configured interval, or bind a TCP port and wait for a request.
3. Build an executor with the production `CredentialResolver` and `EventSink`.
4. Run the workflow.
5. Record the result in `WorkflowDb`.
6. Emit a `scheduler-status` event.
7. Return to step 2.

`stop_job()` cancels the task via a cancellation token. `always_on` is stored in `WorkflowDb` — on startup, the daemon reads all always-on jobs and restarts their tasks automatically.

On startup, the daemon also calls `mark_interrupted_runs()`, which relabels any run left in `status = 'running'` from a previous session as `interrupted` — see [Execution Flow — graceful shutdown](execution-flow.md#10-graceful-shutdown).

`SchedulerDaemon` also exposes `drain_all()`, used for graceful shutdown in `aerini-server`. It sets a `shutting_down` flag (checked cooperatively by every trigger loop), then polls an `active_runs` counter until it reaches zero or a timeout elapses. `stop_job()` and `stop_all()` remain immediate hard stops — `drain_all()` is a separate, additive code path used only on process shutdown.

`aerini-server` in API mode runs the same logic, but without Tauri state. It uses a `DashMap<WorkflowId, TaskHandle>` instead.

---

## Credential store

`CredentialStore` wraps `credentials.db` and manages the encryption lifecycle. On first open, it generates a random 32-byte key via `OsRng`, stores it (keychain or file depending on `KeySource`), and derives an AES-256-GCM cipher. Subsequent opens load the key and reconstruct the cipher.

The desktop app passes `KeySource::OsKeychain` — macOS Keychain, Windows Credential Manager, or Linux SecretService, with `.cred.key` as a fallback. The server passes `KeySource::File` by default; `--keychain` switches it to `KeySource::OsKeychain`.

Each credential entry stores: ID, name, a random nonce, and the GCM ciphertext. The plaintext value is never stored. Decryption happens in `CredentialResolver::resolve()`, called per-node at execution time. The decrypted value lives in memory only for the duration of that node's `execute()` call.

---

## The expression resolver

The expression resolver in `aerini-engine/src/expression/resolver.rs` is a single-pass character scanner. It finds `{{...}}` spans, resolves each expression against the execution context, and builds the output string. It never panics — any unresolvable expression produces an empty string and a warning entry in the run log.

`$env.VAR_NAME` resolution goes through an allowlist (`env_allowlist: Option<&HashSet<String>>`). `None` disables all env access. `Some(set)` allows only listed variables. The server passes the `--allow-env-vars` set; the desktop app always passes `None`.

The fast path short-circuits immediately if the template string contains no `{{`. Static strings have zero resolution overhead.

---

## Frontend architecture

The frontend is deliberately framework-free. The canvas (`src/canvas/`) is a `<canvas>` element with a custom renderer — nodes are rectangles, connectors are quadratic Bézier curves. The minimap is a second smaller canvas updated each frame.

`WorkflowManager` handles save, load, duplicate, and version history. `RunManager` handles execution, the output drawer, and background run lifecycle. They communicate through callbacks rather than shared state — each manager exposes event hooks (`onRunStateChange`, `onNavigate`, etc.) that `app.ts` wires together at startup.

In the desktop app, workflow data flows through Tauri IPC to SQLite. In browser dev mode (running `vite dev` without Tauri), `localStorage` is used for the canvas state only — no execution is available in that mode.

Browser dev mode disables all execution. The Run button is inert. The scheduler daemon, Code (JS) subprocess, and IPC credential layer are all unavailable. `localStorage` in this mode is strictly for frontend development convenience, not a supported data store.

---

## WASM plugin loader

`aerini-engine/src/plugin_loader.rs` loads third-party node types from `.wasm` files at startup. It is the only part of the engine that depends on Wasmtime.

`PluginLoader` holds a shared `wasmtime::Engine` (constructed once per process). `load_plugins(registry, dir)` iterates `.wasm` files in the plugin directory, compiles each with the Component Model enabled, validates that it exports the `aerini-node` world, calls `describe()` once to populate its `NodeDescriptor`, and registers the resulting `WasmPluginNode` in `NodeRegistry`. Failed files are logged as warnings and skipped — they don't crash the process.

Each `execute()` call creates a fresh `wasmtime::Store` for isolation. The pre-linked `InstancePre` (stored per node) is re-instantiated on each call — fast, because recompilation and re-linking are done once at load time. Execution runs inside `tokio::task::spawn_blocking` to avoid blocking the async executor.

Plugins receive WASI clocks, randomness, and stdio, plus outbound HTTP via `wasmtime-wasi-http`. Filesystem access is provided at the WASI API level but all paths return errors — no directories are mounted.

---

## What's not in the engine

A few things that might seem like engine concerns but aren't:

**Export package builder** — in `src-tauri/src/commands/export.rs`. It needs access to Tauri's resource directory to bundle the Linux binary, which ties it to the Tauri runtime.

**n8n import converter** — pure TypeScript in `src/modal-manager.ts`. Runs entirely in the frontend. The converted workflow is passed to the normal save flow after the user confirms the import preview.

**Status page** — in `aerini-server/src/status_server.rs`. Serves HTML directly from the binary, with no external frontend assets required on the server.
