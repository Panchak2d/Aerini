# Architecture

This page explains how Aerini's codebase fits together: three Rust crates, one plugin system, and a vanilla TypeScript frontend. By the end you should be able to tell which crate a change belongs in and why the engine has no idea whether it's running inside the desktop app or on a headless server. It assumes you've read [Concepts](../getting-started/concepts.md); everything here is about the code, not the workflow model a user sees.

## Three crates, one workspace

The repository's root `Cargo.toml` declares a workspace with three members: `aerini-engine`, `aerini-server`, and `src-tauri`. Each has a different job.

| Crate | What it produces | What it depends on |
|---|---|---|
| `aerini-engine` | A library only, no binary | Nothing in this workspace |
| `aerini-server` | A binary named `aerini-server` | `aerini-engine`, as a path dependency |
| `src-tauri` | Both a library (`aerini_lib`) and a binary named `aerini` | `aerini-engine`, plus the Tauri framework |

`aerini-engine` has no `[[bin]]` target at all: it's pure library code, meant to be linked into something else. `src-tauri` is the one crate that builds both ways at once, a `staticlib`/`cdylib`/`rlib` library that Tauri's own build tooling links against, and a thin binary (`src/main.rs`) that just calls into it. `aerini-server` is a conventional single-binary crate.

`examples/plugin-template` sits inside the repository tree but is explicitly excluded from this workspace, not a member of it. Its own `Cargo.toml` builds a `cdylib` targeting `wasm32-wasip2`, a different compilation target than anything else here, and it's built with its own `cargo build --target wasm32-wasip2` invocation rather than `cargo build --workspace`. Without the exclusion, a plain `cargo build` run from the repository root would try to compile it for the host platform and fail.

This split isn't just organizational. Aerini's CI pipeline builds and tests `aerini-engine` and `aerini-server` together on a plain Linux runner with no GUI dependencies installed, and hands `src-tauri` to a separate job that has the platform toolchains a Tauri build actually needs. The workspace boundary maps directly onto how the project gets built and tested in practice, not just onto how the source happens to be arranged on disk.

## Inside aerini-engine

`aerini-engine` is where a workflow actually runs. Its crate-root file declares seventeen public modules; a few carry most of the weight:

- **`node`** defines the `Node` trait every node type implements, and `NodeRegistry`, the map from a node's `type_id` string to the object that runs it.
- **`nodes`** holds the built-in node implementations themselves (40 of them, registered by `register_builtins`) and is the source [Nodes Reference](../guide/nodes.md) is written from.
- **`model`** is the serde layer: `Workflow`, `WorkflowNode`, `WorkflowEdge`, and the rest of the types that cross the Rust/TypeScript wire contract.
- **`graph`** wraps petgraph to turn a `Workflow` into a topologically sorted execution order and to catch cycles before a run starts.
- **`executor`** walks that order, resolving `{{...}}` expressions before each node runs, handling retries and disabled nodes, and routing along `on_success`/`on_failure` edges.
- **`expression`** is the resolver behind that syntax, covered on its own terms in [Expressions](../guide/expressions.md).
- **`context`** holds `ExecutionState`, the mutable per-run state (node outputs, statuses, log entries) that the executor and event-emitting tasks share.
- **`scheduler`** runs the background daemon behind schedules, cron jobs, one-shot timers, and webhook triggers, the mechanism behind [Background Runs](../guide/background-runs.md).
- **`db`**, **`store`**, and **`migration`** cover persistence: workflow and run-history storage, the AES-256-GCM encrypted credential store, and the schema migration engine documented in full at [Schema Migrations](../reference/schema-migrations.md).
- **`mem_tracking`** and **`perf_monitor`** sit on top of a custom global allocator to attribute live memory and produce the per-run performance reports the desktop app's Monitor panel and `aerini-server`'s performance routes both read from.
- **`plugin_loader`** is the WASM plugin host, covered in outline below and in depth in `plugin-authoring.md`.
- **`cron`**, **`error`**, and **`provider`** round the list out: a 5-field cron parser, the engine's error types, and AI provider metadata.

None of this imports anything UI-specific. That constraint is deliberate and it's what makes the rest of this page's answer to "how do the three crates share code" as simple as: they don't reimplement any of the above, they just call into it.

## The EventSink decoupling point

The executor needs to tell something outside itself when a node starts, finishes, or fails, but `aerini-engine` can't know in advance whether that something is a Tauri window or an HTTP client waiting on a stream. The crate solves this with one trait, declared right in its crate root:

```rust
pub trait EventSink: Send + Sync + 'static {
    fn emit(&self, event: &str, payload: serde_json::Value);
}
```

The executor and scheduler hold only an `Arc<dyn EventSink>`. Neither one knows or cares what's on the other side of that call, which is exactly why `aerini-engine` can compile with no `tauri` dependency at all: nothing in the engine names a Tauri type.

Three implementations are actually wired into a running binary, and they don't map one-to-one with the three crates the way you might expect:

- **`TauriEventSink`** (`src-tauri/src/lib.rs`) wraps a `tauri::AppHandle` and forwards every event through `tauri::Emitter::emit`. On the frontend side, `src/ipc/events.ts` listens for those same named events through the webview's own event API, which is how a node turning green on the canvas actually reaches the screen.
- **`EventBridge`** (`aerini-server/src/event_bridge.rs`) is wired up by `main.rs`'s `serve` subcommand. It writes structured log lines to an in-memory `LogBuffer` and updates a `RunState` struct that the single-workflow daemon's status page reads from, and it's also where a completed run gets persisted to SQLite history.
- **`BroadcastEventSink`** (also `aerini-server/src/event_bridge.rs`) is wired up separately, by `api_server/mod.rs`'s `api` subcommand. It serializes each event to a JSON string and sends it over a `tokio::sync::broadcast` channel, the channel that feeds the REST API's SSE run-event streams documented in [Server API Reference](../operations/server-api-reference.md).

So `aerini-server` doesn't have one `EventSink`, it has two, chosen by which subcommand you run: `serve` mode needs a status page and persisted history, `api` mode needs a live event stream fanned out to however many clients are currently watching a run. Both still compile against the exact same trait the desktop app uses. `aerini-engine` also ships a fourth, trivial implementation of its own, `NoopEventSink`, for unit tests and library consumers that don't care about events and just need something to hand the executor.

## The Tauri shell (src-tauri)

`src-tauri`'s `run()` function, called from a two-line `main.rs`, is where the desktop app actually starts. In order, it: installs the memory-tracking allocator hook, opens the workflow database and the encrypted credential store, builds a `NodeRegistry` from the built-in nodes plus whatever `.wasm` plugins are configured, starts the `SchedulerDaemon` with `caller_is_admin` set (the desktop app is single-tenant, so whoever configured a schedule is the same person whose machine runs it), sets up the system tray and the close-to-tray window behavior, and registers every command the frontend is allowed to call.

That command surface is organized by domain under `src-tauri/src/commands/`: `workflow`, `chat`, `credentials`, `oauth`, `scheduler`, `export`, `plugins`, `memory`, `performance`, and `update`, plus a handful of top-level commands like `read_text_file` and `save_file_dialog` that don't belong to any one domain. Each module maps closely to one file under the frontend's `src/ipc/` directory (`workflow.ts`, `credentials.ts`, and so on), which wraps Tauri's `invoke()` calls in typed functions the rest of the UI code calls instead of touching IPC directly. The full list of commands, their arguments, and what each one returns is `desktop-ipc-reference.md`'s job, not this page's. What matters here is that everything the frontend does eventually funnels through this one registered set, and every one of those commands is free to call straight into `aerini-engine` because `src-tauri` already depends on it.

## The aerini-server binary

`aerini-server` is one binary with two subcommands, chosen at startup and never switched between at runtime:

- **`serve`** runs a single exported workflow (the file produced by the desktop app's Export for Server panel) on its own schedule and serves a small status page. This is the path `EventBridge` feeds, described above.
- **`api`** runs many workflows behind a REST API with bearer-token authentication, meant for someone managing several automations from one machine. This is the path `BroadcastEventSink` feeds.

The `api` subcommand's HTTP surface lives under `aerini-server/src/api_server/`, with routes split by resource into separate files (`workflows`, `scheduler`, `memory`, `performance`, `credentials`, `plugins`, `tokens`, `widget`, plus a shared `state` module the handlers pull from). Every endpoint, its method, path, and payload shape, is already covered exhaustively in [Server API Reference](../operations/server-api-reference.md); every CLI flag on both subcommands is covered the same way in [Server CLI Reference](../operations/server-cli-reference.md). This page's job is just to place those two documents in the larger picture: they describe the surface of one binary that, underneath both subcommands, is calling the exact same `aerini-engine` the desktop app calls.

## The plugin system, in outline

A `.wasm` plugin is a WebAssembly component compiled against the Component Model, and the interface it has to implement is small: `aerini-engine/wit/node.wit` declares a `node` interface with exactly two functions, `describe()` (called once, at load time, to get the plugin's metadata) and `execute()` (called once per workflow run that reaches it). `plugin_loader.rs`'s `PluginLoader` compiles and links a `.wasm` file against that interface using `wasmtime`, wraps the result in a `WasmPluginNode`, and that struct implements the exact same `Node` trait a built-in node implements. Once it's registered in the `NodeRegistry`, the executor has no way to tell a plugin node from a compiled-in one, both are just an `Arc<dyn Node>` it calls through.

Everything past that summary, the sandbox and memory limits a plugin runs under, the optional metadata/trigger/storage extensions to the base interface, and how to actually write and sign one, belongs to `plugin-authoring.md` and to [Security](../guide/security.md)'s existing coverage of the sandbox boundary, not to this page.

## What this page doesn't cover

[Concepts](../getting-started/concepts.md) deliberately stays above this level of detail; it explains workflow, node, canvas, and run without ever mentioning that three separate crates exist, because a user building a workflow never needs to know that. This page is the place that split becomes visible, and only from here on do the docs start talking about crates at all.

Three already-shipped reference pages go deeper than this page does on purpose: [Server CLI Reference](../operations/server-cli-reference.md) and [Server API Reference](../operations/server-api-reference.md) cover every flag and every route, and [Schema Migrations](../reference/schema-migrations.md) covers the migration engine's own mechanics. Nothing here duplicates them.

## What's next

- [Embedding aerini-engine](embedding.md), for using `aerini-engine` as a library inside your own Rust program instead of through either binary
- [Adding a Built-in Node](node-authoring.md), for adding a new built-in node to the `nodes` module
- [Writing a Plugin Node](plugin-authoring.md), for writing a `.wasm` plugin against the WIT interface outlined above
- [Desktop IPC Reference](desktop-ipc-reference.md), for the complete list of commands the frontend can call into `src-tauri`
- [Testing](testing.md), for where this codebase's tests live and how to run them
