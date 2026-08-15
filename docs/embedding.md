# Embedding aerini-engine

This guide is for developers who want to run Aerini workflows from inside their own
Rust application — instead of calling the server over HTTP or using the desktop app.

If that's not what you're after, you probably want the
[server deployment guide](./server-deploy.md) or the [API reference](./api-reference.md).

---

## What "embedding" means

Normally you run Aerini as a separate process — the desktop app or `aerini-server` —
and your code talks to it over HTTP. Embedding means you skip that entirely.
`aerini-engine` becomes a library inside your application. Workflows execute in the
same process as your code, with no network call in between.

**Why you'd want this:**
- No extra process to manage or keep running.
- Lower latency — no HTTP roundtrip on every workflow run.
- Full control over where credentials come from and where events go.
- You can ship a product that runs Aerini workflows without exposing any server.

**The tradeoff:** you have to write a little Rust glue code to wire things up. This
guide shows you exactly what that looks like, step by step.

---

## Before you start

You need:

- **Rust installed.** The official installer at [rustup.rs](https://rustup.rs) takes
  about two minutes. Run `rustc --version` afterwards to confirm it worked.
- **A Rust project.** If you don't have one, run `cargo new my-app` in a terminal.
  This creates a new folder called `my-app` with everything you need to get started.
- **A workflow JSON file.** You can export any workflow from the Aerini desktop app
  via File → Export. Save it somewhere in your project — we'll load it in the example.

You don't need deep Rust knowledge. The guide explains each concept when it first
appears.

**Platform note:** `aerini-engine` targets native platforms only. As pinned
today it depends on `tokio` (full feature set), `rusqlite`, and
`reqwest`/`hyper`, none of which build for `wasm32-unknown-unknown` — so it
cannot run inside a browser tab as-is. For browser-facing use, see the
[chat widget](widget-embedding.md) instead.

---

## A note on a few Rust concepts you'll see

If you're new to Rust, three things in this guide might look unfamiliar:

**`Arc::new(...)`** — this is just Rust's way of sharing a piece of data safely across
different parts of your program. Whenever the code says `Arc::new(something)`, think
of it as "wrap this so it can be shared." You don't need to understand how it works
internally — just follow the pattern.

**`async` and `await`** — these keywords mean the engine can run multiple things
without blocking. When you see `.await` on a function call, it means "wait for this
to finish before continuing." The `#[tokio::main]` attribute on `main` is what makes
async code work at all — it sets up the async runtime for you.

**`#[async_trait]`** — a helper that makes async functions work inside Rust traits
(a limitation of the language). You don't need to understand why it exists. Just
put it above any `impl CredentialResolver` block and it'll work.

---

## Adding aerini-engine to your project

Open your project's `Cargo.toml` and add these lines under `[dependencies]`:

```toml
[dependencies]
aerini-engine = { git = "https://github.com/Panchak2d/aerini", tag = "vX.Y.Z" }
tokio        = { version = "1", features = ["full"] }
async-trait  = "0.1"
serde_json   = "1"
```

Replace `vX.Y.Z` with the latest release tag, then check the
[releases page](https://github.com/Panchak2d/aerini/releases) to see if there's a
newer version. Always pin to a specific tag — using `branch = "main"` means your
build can break unexpectedly when the library changes.

After saving `Cargo.toml`, run `cargo build` once. It downloads and compiles
everything. The first build takes a couple of minutes because it's compiling the
engine and all its dependencies from scratch. Subsequent builds are much faster.

---

## Complete working example

Here's a full `src/main.rs` you can copy, run, and then adapt. It loads a workflow
from a file and runs it.

```rust
use async_trait::async_trait;
use aerini_engine::{
    executor::{CredentialResolver, WorkflowExecutor},
    model::Workflow,
    nodes::register_builtins,
    EventSink, NodeRegistry,
};
use serde_json::Value;
use std::{collections::HashMap, path::Path, sync::Arc};

// ── 1. Tell the engine what to do with run events ────────────────────────────

struct StdoutSink;

impl EventSink for StdoutSink {
    fn emit(&self, event: &str, payload: Value) {
        println!("[{}] {}", event, payload);
    }
}

// ── 2. Tell the engine where to find credentials ─────────────────────────────

struct EnvResolver;

#[async_trait]
impl CredentialResolver for EnvResolver {
    async fn resolve(&self, credential_id: &str) -> Option<String> {
        std::env::var(credential_id).ok()
    }
}

// ── 3. Set up the registry, executor, and run ────────────────────────────────

#[tokio::main]
async fn main() {
    // Load a workflow exported from the desktop app.
    let json = std::fs::read_to_string("my-workflow.json")
        .expect("couldn't read my-workflow.json — is it in the right place?");

    let workflow = Workflow::from_json(&json)
        .expect("workflow JSON is invalid");

    // Build the node registry with all built-in node types.
    let mut registry = NodeRegistry::new();
    register_builtins(&mut registry, Path::new("./aerini_data"), None);

    // Create the executor.
    let executor = WorkflowExecutor::new(
        Arc::new(registry),
        Arc::new(EnvResolver),
    )
    .with_event_sink(Arc::new(StdoutSink));

    // Run the workflow with no initial variables.
    let result = executor
        .run(Arc::new(workflow), HashMap::new())
        .await
        .expect("the executor itself failed — this usually means the workflow JSON is malformed");

    // Check what happened.
    if result.success {
        println!("\n✓ Workflow completed successfully.");
    } else {
        println!("\n✗ Workflow failed.");
        if let Some(err) = &result.error {
            println!("  Error: {}", err.message);
        }
    }

    // Print every node's output.
    println!("\nNode outputs:");
    for (node_id, output) in &result.node_outputs {
        println!("  {}: {}", node_id, output);
    }

    // Print the execution log.
    println!("\nExecution log:");
    for entry in &result.logs {
        println!("  {}", entry.message);
    }
}
```

Save it, put a `my-workflow.json` file next to it, and run:

```
cargo run
```

---

## Understanding each piece

### EventSink — what happens when a node finishes

Every time a node completes (success or failure), the engine calls your `emit`
function with two arguments:

- `event` — a string describing what happened. The most common value is
  `"node-status"`.
- `payload` — a JSON object with details. For a `"node-status"` event it looks like
  this:

```json
{
    "workflow_id": "wf_abc123",
    "node_id":     "n1",
    "status":      "success"
}
```

You can do whatever you want with these — write to a log file, push to a database,
send to a WebSocket, stream to a UI. The only rule is: **don't do anything slow
inside `emit`**. The engine calls it synchronously, so a slow `emit` blocks the
entire workflow.

If you need to do something slow (like write to a database), use a channel:

```rust
use tokio::sync::mpsc;
use std::sync::Mutex;

struct ChannelSink {
    // A sender that we can call from the sync emit() function.
    tx: Mutex<mpsc::UnboundedSender<(String, Value)>>,
}

impl EventSink for ChannelSink {
    fn emit(&self, event: &str, payload: Value) {
        // This is instant — we just drop the event into the channel and return.
        let _ = self.tx.lock().unwrap().send((event.to_string(), payload));
    }
}

// Somewhere else in your async code, you have a receiver that processes
// the events at whatever pace it needs.
```

If you don't need events at all:

```rust
use aerini_engine::NoopEventSink;

let executor = WorkflowExecutor::new(Arc::new(registry), Arc::new(EnvResolver))
    .with_event_sink(Arc::new(NoopEventSink));
```

---

### CredentialResolver — providing secrets

Workflows don't store raw passwords or API keys. Instead, a node config references
a credential by a name you choose, like `"openai_key"` or `"stripe_prod"`. When the
engine is about to run that node, it asks your `CredentialResolver` to look up the
actual value.

The engine calls `resolve("openai_key")` and expects you to return either:
- `Some("sk-abc123...")` — the actual secret value
- `None` — you don't have it, in which case the engine passes an empty string to
  the node

Here are three common patterns:

**Reading from environment variables** (good for local development):

```rust
struct EnvResolver;

#[async_trait]
impl CredentialResolver for EnvResolver {
    async fn resolve(&self, credential_id: &str) -> Option<String> {
        std::env::var(credential_id).ok()
    }
}

// Usage: set OPENAI_KEY=sk-abc123 before running your app.
// The workflow must use "OPENAI_KEY" as the credential name.
```

**Reading from a config file or hard-coded map** (useful for testing):

```rust
use std::collections::HashMap;

struct MapResolver {
    secrets: HashMap<String, String>,
}

#[async_trait]
impl CredentialResolver for MapResolver {
    async fn resolve(&self, credential_id: &str) -> Option<String> {
        self.secrets.get(credential_id).cloned()
    }
}

// Build it like this:
let mut secrets = HashMap::new();
secrets.insert("openai_key".to_string(), "sk-abc123".to_string());
let resolver = MapResolver { secrets };
```

**Always returning nothing** (useful for workflows that don't use credentials):

```rust
struct NoopResolver;

#[async_trait]
impl CredentialResolver for NoopResolver {
    async fn resolve(&self, _credential_id: &str) -> Option<String> {
        None
    }
}
```

---

### NodeRegistry — the table of node types

The registry is a lookup table: given a `node_type_id` string from the workflow JSON,
it finds the code that knows how to run that node. `register_builtins` fills it with
every built-in node type.

```rust
let mut registry = NodeRegistry::new();
register_builtins(&mut registry, Path::new("./aerini_data"), None);
```

The two extra arguments to `register_builtins`:

**First argument — the data directory:**
This is a folder where the AI Memory node stores its local SQLite database. The
engine creates it automatically if it doesn't exist, so you don't need to create
it yourself. If none of your workflows use the AI Memory node, this path doesn't
matter — just pass `Path::new(".")` to use the current directory.

**Second argument — the workflow database:**
This is `None` in most cases. Passing `None` means the Set Variable and Get Variable
nodes are disabled. If you need those nodes, you'll need to open a `WorkflowDb` and
pass it here — but that's an advanced use case not covered in this guide.

**What node type IDs look like:**
Every node type has a string ID that the engine uses to look it up in the registry.
These IDs are written into the workflow JSON when you build a workflow in the desktop
app. You never normally need to type them by hand, but here are some common ones in
case you're constructing workflows programmatically:

| Node name in the UI     | `node_type_id` in JSON |
|-------------------------|------------------------|
| HTTP Request            | `http_request`         |
| Transform Data          | `transform`            |
| If / Else               | `if_condition`         |
| Run Code (JS)           | `code`                 |
| Send Email              | `email`                |
| Slack Message           | `slack`                |
| Schedule (Trigger)      | `schedule`             |
| AI Prompt               | `ai_prompt`            |
| Output                  | `output`               |

---

### Loading and running a workflow

**Loading from a file** (the most common case):

```rust
let json = std::fs::read_to_string("path/to/my-workflow.json")
    .expect("file not found");

let workflow = Workflow::from_json(&json)
    .expect("invalid workflow JSON");
```

`from_json` automatically handles version migrations. If someone exports a workflow
from an older version of Aerini, it gets upgraded silently before running — you don't
have to do anything.

**Loading from a string** (useful when the JSON comes from a database or API):

```rust
let json = fetch_workflow_from_db(workflow_id).await; // your own function
let workflow = Workflow::from_json(&json).expect("invalid workflow JSON");
```

**Passing variables into a workflow:**

Variables let you inject dynamic values at run time. Any node in the workflow can
read them using `{{$vars.key}}` in its config fields. For example, if you pass
a variable called `"target_url"`, an HTTP Request node can use
`{{$vars.target_url}}` as its URL.

```rust
let mut vars = HashMap::new();
vars.insert("target_url".to_string(), serde_json::json!("https://api.example.com/data"));
vars.insert("user_id".to_string(),    serde_json::json!("user_42"));

let result = executor.run(Arc::new(workflow), vars).await.unwrap();
```

---

### Reading the result

`executor.run()` returns a `WorkflowResult` with everything that happened during
the run.

**Did it succeed?**

```rust
if result.success {
    println!("Workflow finished successfully.");
} else {
    println!("Workflow failed.");
}
```

**What went wrong?**

```rust
if let Some(error) = &result.error {
    println!("Error message: {}", error.message);
    println!("Error code:    {}", error.code);

    // error.recoverable tells you whether retrying might help.
    if error.recoverable {
        println!("This error might go away if you try again.");
    }
}
```

**What did each node output?**

`result.node_outputs` is a map from node ID to that node's output value. The node
IDs come from your workflow JSON — look for the `"id"` field on each node.

```rust
// Get the output from a specific node.
if let Some(output) = result.node_outputs.get("n1") {
    // output is a serde_json::Value — access fields like this:
    if let Some(body) = output.get("body") {
        println!("Response body: {}", body);
    }
}

// Or print everything.
for (node_id, output) in &result.node_outputs {
    println!("{}: {}", node_id, output);
}
```

**The execution log:**

The log contains one entry per event during the run — node starts, completions,
warnings, and errors. It's useful for debugging when something goes wrong.

```rust
for entry in &result.logs {
    println!("[{}] {}", entry.level, entry.message);
    // entry.level is something like "info", "warn", or "error"
    // entry.node_id tells you which node the message came from (if any)
}
```

**Validation warnings:**

If any node's input didn't match its declared schema, warnings end up here. With
default settings these are non-fatal — the workflow continues. You can make them
fatal with `.with_strict_schema_validation(true)`.

```rust
for warning in &result.validation_errors {
    println!("Schema warning: {}", warning);
}
```

---

## Handling errors

There are two different kinds of failure to handle.

**Kind 1 — the executor itself fails.** This happens when the workflow JSON is
structurally broken (a cycle in the graph, a node referencing a type that isn't
registered, etc.). `run()` returns `Err(...)` in this case.

```rust
match executor.run(Arc::new(workflow), HashMap::new()).await {
    Ok(result) => {
        // Workflow ran (but may have failed internally — check result.success)
        println!("Ran. Success: {}", result.success);
    }
    Err(e) => {
        // The executor couldn't run the workflow at all.
        println!("Executor error: {}", e);
    }
}
```

**Kind 2 — a node inside the workflow fails.** This shows up inside `result`, not
as an `Err`. The run returns `Ok(result)` with `result.success == false`. Check
`result.error` and `result.logs` to understand what happened.

The distinction matters: if `run()` returns `Err`, something is wrong with your
setup or the workflow structure. If it returns `Ok` with `success == false`,
the workflow ran correctly but a node hit an error (network failure, bad API key,
etc.).

---

## Locking things down for production

The default settings are permissive, which is fine while you're developing. Before
you ship something user-facing, consider restricting what workflows can do.

```rust
use std::path::PathBuf;

let executor = WorkflowExecutor::new(Arc::new(registry), Arc::new(EnvResolver))
    .with_event_sink(Arc::new(StdoutSink))

    // Restrict the File node to a specific folder.
    // Without this, the File node can read and write anywhere on the filesystem.
    // If users can supply their own workflows, this is not optional.
    .with_file_sandbox_dir(PathBuf::from("/var/myapp/uploads"))

    // Disable the Shell Command node.
    // This node runs arbitrary shell commands. Disable it unless you specifically
    // need it and trust the workflow authors completely.
    .with_shell_disabled(true)

    // Disable the Code (JavaScript) node.
    // This node runs arbitrary JavaScript via Node.js. Same reasoning as above.
    .with_code_disabled(true)

    // Disable the Database node.
    // This node can connect to any Postgres or MySQL URL in the workflow config.
    // Disable it to prevent users from connecting to databases you don't control.
    .with_database_disabled(true)

    // Set a maximum run time for any workflow.
    // If a workflow takes longer than this, it's killed and returns a timeout error.
    // The value is in seconds. This example sets a 5-minute limit.
    .with_server_max_duration_secs(Some(300))

    // Fail a node immediately if its input doesn't match its schema.
    // The default (false) logs a warning and continues running.
    // Set to true if you want strict validation.
    .with_strict_schema_validation(true);
```

---

## Common problems

**`cargo build` fails with "package not found" or a git error.**
Make sure you have git installed and can reach GitHub. Run
`git clone https://github.com/Panchak2d/aerini` in a terminal to verify. If the
tag you specified doesn't exist yet, check the
[releases page](https://github.com/Panchak2d/aerini/releases).

**`Workflow::from_json` panics with "invalid workflow JSON".**
The most likely cause is that the JSON file is corrupted or was exported from a
version of Aerini that introduced an incompatible format. Try re-exporting from the
desktop app. If it still fails, print the raw JSON and look for obvious issues
(missing braces, trailing commas, etc.).

**The executor returns `Err(UnknownNodeType("..."))`.**
A node in your workflow uses a `node_type_id` that isn't in the registry. This
usually means the workflow was built with a plugin or custom node that you haven't
registered. If you're using only built-in nodes, make sure `register_builtins` was
called before `executor.run()`.

**A node fails with an empty string instead of the credential value.**
Your `CredentialResolver` returned `None` for that credential. The credential name
in your resolver must match the name stored in the workflow exactly — it's
case-sensitive. Check what name the workflow uses by opening the workflow JSON and
searching for `"credentials"`.

**The workflow runs but `result.node_outputs` is empty.**
This can happen if the workflow has no nodes, or if all nodes were skipped (e.g.
a conditional branch that wasn't taken). Check `result.logs` for a trace of what
the engine actually did.

---

## Stable API surface (v1.0+)

The items below won't change in a backward-incompatible way without a major version
bump. We can always add new things — new builder methods, new error variants — but
we won't remove or rename anything in this list.

| What | Where |
|------|-------|
| `Node` trait | `aerini_engine::node` |
| `NodeRegistry` — `new()`, `register()`, `get()`, `all_descriptors()` | `aerini_engine::node` |
| `NodeDescriptor`, `NodePorts`, `PortDefinition`, `PortPosition` | `aerini_engine::node` |
| `WorkflowExecutor` — constructor, all `with_*()` methods, `run()` | `aerini_engine::executor` |
| `CredentialResolver` trait | `aerini_engine::executor` |
| `WorkflowResult` — all fields | `aerini_engine::executor` |
| `EventSink` trait, `NoopEventSink`, `ENGINE_VERSION` | `aerini_engine` |
| All types in `model` — `Workflow`, `WorkflowNode`, `WorkflowEdge`, `RetryPolicy`, etc. | `aerini_engine::model` |
| `EngineError` — existing variants only (we can add new ones) | `aerini_engine::error` |
| `NodeError` | `aerini_engine::error` |

---

## Internal modules (not stable)

These modules are technically accessible from outside the crate but are not part
of the embedding API. Their internals can change in any minor release without
warning. Don't import from them directly.

| Module | What it does |
|--------|-------------|
| `graph` | Builds and walks the execution graph (cycle detection, topological sort) |
| `expression` | Evaluates `{{...}}` template expressions inside node configs |
| `context` | Manages per-run execution state |
| `scheduler` | Background daemon that fires scheduled and webhook triggers |
| `db` | SQLite persistence for workflows, run history, and settings |
| `migration` | Upgrades older workflow JSON to the current schema |
| `store` | AES-256-GCM encrypted credential store used by the desktop app |
| `cron` | Parses cron expressions like `"0 9 * * MON"` |

---

## Licensing

`aerini-engine` is released under **AGPL-3.0**.

The important part: if you embed `aerini-engine` in an application and make that
application available as a network service to others, AGPL-3.0 requires you to
publish the complete source code of your application under the same license. This
applies even if you only modified a small part of it.

**A commercial license removes this requirement entirely.** Your application code
stays private, your modifications stay proprietary, no source disclosure needed.

You almost certainly need a commercial license if you are:

- Building a SaaS or hosted product powered by Aerini
- Shipping a modified version of Aerini to paying customers
- Running `aerini-server` (or your own binary that embeds `aerini-engine`) as a
  service for other people
- Subject to a legal or procurement policy that prohibits AGPL software

If you're building an internal tool that only your team uses and you're okay with
AGPL terms, you're probably fine without it — but check with your legal team if
you're unsure.

[View pricing and license terms →](https://panchak2d.github.io/aerini/pricing)

Still have questions? Open a thread in
[GitHub Discussions](https://github.com/Panchak2d/aerini/discussions).
