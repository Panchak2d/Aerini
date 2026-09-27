# Embedding aerini-engine

`aerini-engine` is a plain Rust library. Nothing in it depends on Tauri or on either binary in this workspace, so you can add it to your own program, register whichever built-in nodes you want, supply your own credential resolver and event sink, and run workflows with no desktop app and no `aerini-server` process anywhere in the picture. This page assumes you've read [Architecture](architecture.md); it covers the same crate from the outside, as a dependency, rather than as part of this repository's own workspace.

## License

`aerini-engine` is AGPL-3.0 licensed, same as the rest of this repository. Embedding it in your own program and then distributing that program, or running it as a network-accessible service for others, brings your program under the same obligation described in [Why Aerini is dual-licensed](../dual-licensing.md): publish your modifications under AGPL-3.0, including the code you wrote around the crate, or get a commercial license if you need to keep it closed.

## Adding the dependency

`aerini-engine` isn't published on crates.io. Depend on it directly from the repository, either as a git dependency or, if you've cloned the repository locally, as a path dependency:

```toml
[dependencies]
aerini-engine = { git = "https://github.com/Panchak2d/aerini" }
tokio         = { version = "1", features = ["full"] }
async-trait   = "0.1"
serde_json    = "1.0"
```

The package name has a hyphen; the crate's own `[lib]` declaration renames it to `aerini_engine` for `use` paths, so every import below reads `aerini_engine::...`, not `aerini-engine::...`.

## What's actually there to use

The crate root declares seventeen public modules, the same list [Architecture](architecture.md#inside-aerini-engine) already walks through. Two items sit at the crate root itself rather than inside a module: `aerini_engine::Workflow` and `aerini_engine::NodeRegistry`, re-exported because they're the two types almost every caller needs first. `EventSink`, `NoopEventSink`, and `ENGINE_VERSION` also live at the root.

Not every `pub` item in those seventeen modules is something an embedder has a reason to reach for. `provider::ProviderRegistry` exists so the AI Prompt and AI Agent nodes can share provider-detection logic between themselves; both call it through associated functions with no setup on your part, and there's nothing to configure there directly. `db::WorkflowDb` is genuinely one struct, but its methods span several unrelated concerns collected under one type: workflow storage and versioning, run history, performance reports, and the desktop app's Chat Panel sessions all live on the same handle. Opening a `WorkflowDb` for run history, covered below, hands you all of it, whether or not you touch the rest.

The modules you'll actually work with for a basic embedding are `node` (`NodeRegistry`, the `Node` trait), `nodes` (`register_builtins`, the 40 built-in implementations), `model` (`Workflow` and its parts), and `executor` (`WorkflowExecutor`, `CredentialResolver`). `graph` is public but you won't call into it: `WorkflowExecutor::run` builds and discards an `ExecutionGraph` internally on every call, so there's no reason to construct one yourself.

## A minimal embedding

This runs a two-node workflow (a manual trigger feeding an output node) with no database, no stored credentials, and nothing but stdout for events.

```rust
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use aerini_engine::executor::{CredentialResolveError, CredentialResolver, WorkflowExecutor};
use aerini_engine::model::Workflow;
use aerini_engine::node::NodeRegistry;
use aerini_engine::nodes::register_builtins;
use aerini_engine::EventSink;

const WORKFLOW_JSON: &str = r#"
{
  "id": "wf-embed-demo",
  "name": "Embed Demo",
  "nodes": [
    { "id": "n_trigger", "node_type_id": "manual_trigger", "node_type": "action",
      "name": "Start",  "config": {}, "input_schema": {}, "output_schema": {} },
    { "id": "n_output",  "node_type_id": "output",         "node_type": "utility",
      "name": "Result", "config": {}, "input_schema": {}, "output_schema": {} }
  ],
  "edges": [
    { "id": "e1", "from_node": "n_trigger", "from_port": "output",
      "to_node": "n_output", "to_port": "input" }
  ]
}
"#;

struct StdoutSink;

impl EventSink for StdoutSink {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        println!("[{event}] {payload}");
    }
}

struct NoCredentials;

#[async_trait::async_trait]
impl CredentialResolver for NoCredentials {
    async fn resolve(&self, _credential_id: &str) -> Result<String, CredentialResolveError> {
        Err(CredentialResolveError::NotFound)
    }
}

#[tokio::main]
async fn main() {
    let mut registry = NodeRegistry::new();
    register_builtins(&mut registry, Path::new("."), None);
    registry.seal_builtins();

    let executor = WorkflowExecutor::new(Arc::new(registry), Arc::new(NoCredentials))
        .with_event_sink(Arc::new(StdoutSink));

    let workflow = Workflow::from_json(WORKFLOW_JSON).expect("workflow JSON is valid");

    let result = executor
        .run(Arc::new(workflow), HashMap::new())
        .await
        .expect("run must not error for a valid, acyclic workflow");

    println!("success: {}", result.success);
    println!("output: {:?}", result.node_outputs.get("n_output"));
}
```

A few things worth calling out, since they're easy to get wrong on a first attempt:

- `input_schema` and `output_schema` are required fields on every node in the JSON, not optional ones. An empty object skips validation entirely, but leaving the key out fails deserialization.
- `node_type_id` selects which registered implementation runs (`"manual_trigger"`, `"output"`); `node_type` is a separate, cosmetic category field the palette uses for grouping and has no effect on execution.
- `schema_version` can be left out of hand-written JSON. `Workflow::from_json` treats a missing version as already current, not as an old file needing every migration run against it.
- `register_builtins`'s `data_dir` argument is only touched if a workflow actually uses the AI Memory node, which opens its database there lazily on first execution. Passing `Path::new(".")`, as above, is fine for a workflow that never uses it.
- `seal_builtins` only matters once you start registering `.wasm` plugins on top of the built-ins (`register_plugin` checks against it). Skipping it here changes nothing, since this example never calls `register_plugin`.

The event sink receives a `"node-status"` event each time a node's status changes during a run (`running`, then `success`, `error`, or `skipped`), each carrying `{"workflow_id", "node_id", "status"}`. That's the entire event vocabulary this trait produces on the executor's own account; a scheduler, if you were also running one, emits several more of its own (`scheduler-ready`, `scheduler-status`, and so on), out of scope for this page.

## Tokio runtime requirements

`aerini-engine`'s own end-to-end tests exercise chains, branches, and loops entirely under plain `#[tokio::test]`, meaning the default current-thread flavor, not the multi-threaded one. A current-thread runtime is enough to run a workflow correctly. Where a multi-threaded runtime actually matters is `parallel_execution: true` workflows: those still run correctly on a single thread, cooperatively, but only get real concurrency across CPU cores under a multi-threaded runtime (`#[tokio::main]`'s default, or `Builder::new_multi_thread()`).

What isn't optional is having a Tokio runtime already running before you call `.run()`. Several built-in nodes use `tokio::spawn` or `tokio::task::spawn_blocking` internally (HTTP requests, the Code and Shell nodes, database nodes), and the executor's own performance sampling uses `tokio::time::interval`. Calling `.run()` from outside any runtime panics the same way any other Tokio call outside a runtime does.

## Running without Tauri or aerini-server

Nothing in `aerini-engine` reads a Tauri-specific environment variable, expects a directory either binary would have created, or otherwise notices whether it's embedded, running inside the desktop shell, or running inside `aerini-server`. The desktop app and `aerini-server`'s `api` mode both call `std::fs::create_dir_all` on their data directory explicitly, before opening anything. `serve` mode doesn't need to: its data directory is wherever the workflow config file already lives, and that path is guaranteed to exist since the config was just read from it. An embedder should do the same as the two binaries that create their directory explicitly, for whatever directory it wants to use, and needs nothing else installed or running alongside it.

## Persisting workflows and credentials

The example above needs none of this. It applies once you want run history, saved workflow variables, or centrally stored credentials instead of a resolver you write yourself.

`WorkflowDb::open(path, pool_size)` and `CredentialStore::open(db_path, key_source)` are both synchronous, blocking calls. Call them before your runtime starts, or hand them to `tokio::task::spawn_blocking` if you're already inside one. `aerini-server` does exactly this for its own credential store, with a documented reason: `KeySource::OsKeychain` makes a blocking OS call, and running that directly on a thread that's also driving a Tokio runtime can deadlock the runtime.

`KeySource` has two variants. `KeySource::File(path)` reads or generates a local AES key file and creates the file's parent directory itself if it doesn't exist; it never touches the OS keychain. `KeySource::OsKeychain { fallback }` tries the platform keychain first (Keychain, Credential Manager, Secret Service) and only falls back to a file if that's unavailable. For a bare embedding with no desktop integration, `KeySource::File` is the simpler choice and matches what `aerini-server` defaults to.

Neither `open` call creates its own top-level data directory; the desktop app and `aerini-server`'s `api` mode both call `create_dir_all` on that directory themselves first. Do the same for whatever path you choose.

If you want the exact credential model the desktop app uses rather than writing your own resolver, `aerini_engine::store::StoreCredentialResolver` is public and takes an `Arc<CredentialStore>` directly:

```rust
use aerini_engine::store::{CredentialStore, KeySource, StoreCredentialResolver};

let store = CredentialStore::open(&creds_path, KeySource::File(key_path))?;
let resolver = Arc::new(StoreCredentialResolver { store: Arc::new(store) });
```

Its `resolve` implementation already offloads the blocking store lookup to `spawn_blocking` on every call, so it's safe to use from inside an already-running executor without any extra wrapping on your side.

For run history and saved workflow variables, pass `Some(Arc::new(workflow_db))` as `register_builtins`'s third argument instead of `None`. With `None`, the Set Variable and Get Variable nodes still run; persisting a variable becomes a no-op with a logged warning instead of a write.

## Memory and performance stats

`aerini_engine::mem_tracking::install()` installs the global allocation tracker that per-run memory and performance figures are built on. It's meant to be called once, at process startup. Skipping it doesn't break anything: workflows still run, `mem_tracking::snapshot()` and the performance-report machinery just stay empty, since nothing is recording allocations for them to report on.

## What this page doesn't cover

Running workflows on a schedule or from a webhook without a UI in front of them means wiring up `aerini-engine`'s `scheduler` module yourself; nothing here does that for you. Loading `.wasm` plugins into a `NodeRegistry` you built yourself works the same way `plugin_loader.rs` already does it for the desktop app, covered in `plugin-authoring.md`. Adding an entirely new built-in node type, as opposed to embedding the 40 that already exist, is `node-authoring.md`'s job.
