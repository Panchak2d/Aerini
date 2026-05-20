# Architecture

How Flowo is structured, why it's split the way it is, and how execution actually works.

## The three-crate split

```
flowo-engine/     — core library: workflow model, executor, scheduler, node registry, 34 nodes
src-tauri/        — Tauri shell: IPC commands, system tray, app lifecycle
flowo-server/     — headless binary: HTTP API, serve mode CLI, control commands
```

`flowo-engine` has no Tauri dependency. It's a pure Rust library. Both `src-tauri` and `flowo-server` depend on it, but neither appears in the other's dependency tree. This matters because it keeps the execution engine testable in isolation and makes the server binary possible without bundling a WebView runtime.

The TypeScript frontend (under `src/`) is a vanilla TS/Vite application. No frontend framework. It communicates with the Tauri shell via IPC commands defined in `src-tauri/src/commands/`.

## The EventSink trait

The decoupling point between the engine and the two runtimes is `EventSink`:

```rust
// The executor and scheduler call this during execution.
// The Tauri shell and the server binary each implement it differently.
trait EventSink: Send + Sync {
    fn emit_node_status(&self, workflow_id: &str, node_id: &str, status: &str);
    fn emit_scheduler_status(&self, event: SchedulerStatusEvent);
}
```

The Tauri shell's implementation emits events to the frontend window via Tauri's event system. The frontend's `listenNodeStatus` subscription picks them up and turns the node green/red on the canvas in real time.

`flowo-server`'s implementation broadcasts the same events over the SSE endpoint at `/api/events`. A client watching that stream sees the exact same event shape.

The executor doesn't know which runtime it's in. It calls `EventSink` and the runtime decides what to do with the event. This is what makes scheduled runs animate on the canvas exactly like manual runs — same executor, same events, same handler.

## Workflow execution

A workflow is a directed acyclic graph. `WorkflowNode` holds the node type ID and its static config. `WorkflowEdge` holds the connection between two nodes, including optional `condition`, `on_success`, and `on_failure` routing.

When you press Run, `WorkflowExecutor::run()`:

1. Builds a topologically sorted execution order using `petgraph`.
2. Walks the graph. For each node:
   - Resolves all `{{...}}` expressions in the node's config against the current `ExecutionContext` (which accumulates node outputs as the run progresses).
   - Resolves credential IDs to actual values via `CredentialResolver`.
   - Calls `node.execute(NodeInput)` — the async dispatch through `Arc<dyn Node>`.
   - Stores the `NodeOutput` in the context for downstream nodes.
   - Emits a node-status event before and after execution.
3. Returns a `WorkflowResult` with all node outputs, all log entries, and a success flag.

The executor itself is linear. Parallelism within a single workflow run isn't currently implemented — nodes execute one at a time in topological order. The scheduler runs multiple workflows concurrently (each in its own Tokio task), but a single workflow's nodes are sequential.

## The scheduler daemon

`SchedulerDaemon` is the background run engine. It lives as Tauri state, initialized at app startup. When you start a background run, `start_job()` spawns a Tokio task for that workflow. The task owns a loop:

1. Extract the trigger (Schedule or Webhook) from the workflow.
2. Sleep the configured interval, or bind a TCP port and wait for a request.
3. Wake up. Build an executor with the production `CredentialResolver` and `EventSink`.
4. Run the workflow.
5. Record the result in `WorkflowDb`.
6. Emit a `scheduler-status` event.
7. Go back to step 2.

`stop_job()` cancels the Tokio task via a cancellation token. `always_on` is a flag stored in `WorkflowDb` — on app startup, the daemon reads all always-on jobs and restarts their tasks automatically.

`flowo-server`'s API mode has its own scheduler with the same logic, but without Tauri state — it uses a `DashMap<WorkflowId, TaskHandle>` instead.

## Credential store

The `CredentialStore` wraps `credentials.db` and handles the encryption lifecycle. On first open it generates a random 32-byte key, writes it to the key file with `chmod 600`, and derives an AES-256-GCM cipher. Subsequent opens read the key file and reconstruct the cipher.

Each credential entry in the database stores: ID, name, a random nonce, and the GCM ciphertext. The raw value is never stored. Decryption happens in `CredentialResolver::resolve()`, called per-node at execution time. The decrypted value lives only in memory for the duration of the node's `execute()` call.

The key file is not the OS keychain. This is intentional — it makes the store work identically on all three platforms without platform-specific keychain APIs. The tradeoff is that the key is on disk, so physical access to the machine defeats the encryption. Back up the key file alongside the database.

## The expression resolver

The expression resolver in `flowo-engine/src/expression.rs` is a single-pass character scanner. It finds `{{...}}` spans, resolves each expression, and builds the result string. It never panics — any unresolvable expression produces an empty string and a warning entry in the log.

`$env.VAR_NAME` resolution goes through an allowlist (`env_allowlist: Option<&HashSet<String>>`). `None` = disabled. `Some(set)` = only variables in the set resolve. The server passes the `--allow-env-vars` set. The desktop app always passes `None`.

The fast path short-circuits immediately if the template contains no `{{`. Static strings have zero resolution overhead.

## Frontend architecture

The frontend is deliberately framework-free. The canvas (`src/canvas/`) is a `<canvas>` element with a custom renderer — nodes are drawn as rectangles, connectors as quadratic bezier curves. The minimap is a second smaller canvas updated every frame.

`WorkflowManager` owns save/load/duplicate/version history. `RunManager` owns execution, the output drawer, and background run lifecycle. They communicate through callbacks rather than shared state — each manager exposes event hooks (`onRunStateChange`, `onNavigate`, etc.) that `app.ts` wires together at startup.

Workflow data is stored in SQLite via Tauri IPC (in the desktop app) or in `localStorage` (in browser dev mode only — when running `vite dev` without Tauri). The `isTauri()` guard in `utils.ts` controls which path is taken.

Browser mode disables all execution. The Run button is inert and shows a notice explaining that execution requires the desktop app. The scheduler daemon, the Code (JS) subprocess, and the IPC credential layer are all unavailable in this mode. `localStorage` is used purely to make the canvas editable during frontend development — it is not a supported data store for real workflows.

## Workflow schema versioning

Every `.flowo` file and workflow stored in the database carries a `schema_version` field on the `Workflow` struct. The current version is `"1.0"`.

```rust
pub struct Workflow {
    // ...
    #[serde(default = "default_schema_version")]
    pub schema_version: String,  // always "1.0" for now
}
```

**How loading works.** `Workflow::from_json()` now runs in three steps:

1. Parse the raw JSON into an untyped `serde_json::Value`.
2. Pass it to `MigrationEngine::apply()`, which reads `schema_version`, applies any pending migration functions in sequence, and updates the version field after each step.
3. Deserialize the (now-current) value tree into `Workflow`.

For version `"1.0"` workflows — which is all existing workflows — step 2 is a no-op that returns immediately. There is no performance cost.

**No migration functions exist yet.** The framework shipped in this release. When the schema needs to change in a future release, the migration function is registered in `MigrationEngine::new()` and `CURRENT_VERSION` is incremented. The change is invisible to users — old files load and migrate automatically.

**If a workflow was created by a newer Flowo build** and this build has no migration path to that version, `from_json()` returns an error rather than silently loading malformed data. The error message tells the user to upgrade Flowo.

See [`docs/schema-migrations.md`](schema-migrations.md) for the full migration contract, how to write a migration function, and backup guidance for production deployments.

## What's not in the engine

A few things that might seem like they'd be in `flowo-engine` but aren't:

**The export package builder** lives in `src-tauri/src/commands/export.rs`. It builds the deployment zip including the systemd service file, install script, and `.env.example`. It needs access to Tauri's resource directory (to bundle the Linux binary), which ties it to the Tauri runtime.

**The n8n import converter** is pure TypeScript in `src/modal-manager.ts`. It runs entirely in the frontend — no Rust involved. The converted workflow is passed to the normal save flow once the user confirms the import preview.

**The status page** (the read-only HTTP page served by `flowo-server` in serve mode) is in `flowo-server/src/status_server.rs`. It serves HTML directly from the binary — no separate frontend assets needed on the server.
