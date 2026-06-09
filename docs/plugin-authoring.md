# Writing a Flowo Plugin Node

Flowo's WASM plugin system lets you ship new node types as `.wasm` files. Once installed, plugin nodes appear in the palette and execute identically to built-in nodes — in the desktop app and `flowo-server` alike.

---

## What plugins can do

A plugin node receives its configuration parameters and resolved credential values, runs arbitrary logic, and returns a JSON object as output. Downstream nodes can reference that output using the normal `{{...}}` expression syntax.

From inside the plugin sandbox, you can:
- Make outbound HTTP requests (WASI HTTP is available).
- Use Rust's standard library (strings, collections, JSON parsing via any pure-Rust crate).
- Perform CPU-bound computation.

You cannot:
- Access the local filesystem (no preopened directories — all filesystem calls return errors).
- Bind ports or accept inbound connections.
- Access Flowo's SQLite database or credential store directly.
- Call back into Flowo's Rust API (the interface is one-directional: host calls guest).

See [Security model](#security-model) for details.

---

## Prerequisites

- **Rust stable 1.82 or later.** Earlier versions do not include the `wasm32-wasip2` target.
- **The WASM target:**
  ```bash
  rustup target add wasm32-wasip2
  ```

No other tooling is needed. You do not need `cargo-component` or `wasm-opt`. The `wasm32-wasip2` target compiles directly to a WASM Component using the standard Rust toolchain.

---

## Starting from the template

Clone the Flowo repository and copy the template:

```bash
cp -r examples/plugin-template my-plugin
cd my-plugin
```

The template is a self-contained Rust crate. It already has the correct `crate-type`, `wit-bindgen` dependency, and WIT file. Rename the crate in `Cargo.toml`, then edit `src/lib.rs`.

To verify the template builds before modifying it:

```bash
cargo build --target wasm32-wasip2 --release
```

---

## The WIT interface

Every Flowo plugin must export the `flowo-node` world defined in `wit/node.wit`. The full annotated interface:

```wit
package flowo:plugin@0.1.0;

interface types {
    /// A key-value parameter passed to a node.
    record param {
        key: string,
        value: string,
    }

    /// Input passed to a plugin node's execute function.
    record node-input {
        /// Node configuration parameters (from the workflow JSON).
        params: list<param>,
        /// Resolved credential values (secrets, never credential IDs).
        credentials: list<param>,
    }

    /// Output returned from a plugin node's execute function.
    record node-output {
        success: bool,
        /// JSON-encoded output data. Must be a valid JSON object string.
        data: string,
        /// Short machine-readable error code. Empty string if success.
        error-code: string,
        /// Human-readable error message. Empty string if success.
        error-message: string,
        /// If true, the executor may retry this node per its RetryPolicy.
        recoverable: bool,
    }

    /// Metadata describing this node type. Returned by describe().
    record node-descriptor {
        /// Unique identifier. Use reverse-domain format: "com.example.my-node"
        type-id: string,
        /// Display name shown in the UI palette.
        display-name: string,
        /// Category for palette grouping: "action", "ai", "logic", "utility"
        category: string,
        /// Short description shown in the UI.
        description: string,
        /// JSON Schema string for node inputs (shown in config panel).
        input-schema: string,
        /// JSON Schema string for node outputs (used in expression picker).
        output-schema: string,
    }
}

/// The interface every Flowo plugin node must implement.
interface node {
    use types.{node-input, node-output, node-descriptor};

    /// Return metadata about this node type. Called once at load time.
    describe: func() -> node-descriptor;

    /// Execute the node. Called for every workflow run that reaches this node.
    execute: func(input: node-input) -> node-output;
}

/// The world exported by every Flowo plugin component.
world flowo-node {
    export node;
}
```

WIT field names use kebab-case. `wit-bindgen` maps them to snake_case in Rust: `type-id` becomes `type_id`, `error-code` becomes `error_code`, and so on.

---

## Implementing `describe()`

`describe()` is called once when Flowo loads the plugin. The returned `NodeDescriptor` is cached for the lifetime of the process — it is not called again during normal operation.

```rust
fn describe() -> NodeDescriptor {
    NodeDescriptor {
        type_id: "com.example.my-node".to_string(),
        display_name: "My Node".to_string(),
        category: "utility".to_string(),
        description: "A short description shown in the UI.".to_string(),
        input_schema: r#"{
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch"
                }
            },
            "required": ["url"]
        }"#.to_string(),
        output_schema: r#"{
            "type": "object",
            "properties": {
                "body": { "type": "string" }
            }
        }"#.to_string(),
    }
}
```

**`type_id`** must be unique across all installed plugins. Use reverse-domain notation: `com.yourname.node-name`. Flowo uses this ID to match nodes in saved workflows; changing it after publishing breaks existing workflows that use your node.

**`category`** controls which palette group the node appears in. Accepted values: `"trigger"`, `"action"`, `"ai"`, `"logic"`, `"utility"`, `"other"`. Nodes with an unrecognised category are silently omitted from the palette — they load successfully and execute correctly if referenced in a workflow, but they will not be visible in the UI.

**`input_schema` and `output_schema`** are JSON Schema strings. They control the node configuration panel and the expression picker, respectively. Both must be valid JSON objects. If you have no inputs or outputs, use `"{}"`.

---

## Implementing `execute()`

`execute()` receives a `NodeInput` and must return a `NodeOutput`. The function is synchronous from the guest's perspective.

```rust
fn execute(input: NodeInput) -> NodeOutput {
    // Look up a parameter by key.
    let url = input
        .params
        .iter()
        .find(|p| p.key == "url")
        .map(|p| p.value.as_str())
        .unwrap_or("");

    if url.is_empty() {
        return NodeOutput {
            success: false,
            data: "{}".to_string(),
            error_code: "missing_url".to_string(),
            error_message: "url parameter is required".to_string(),
            recoverable: false,
        };
    }

    // ... do work ...

    NodeOutput {
        success: true,
        data: r#"{"result":"ok"}"#.to_string(),
        error_code: String::new(),
        error_message: String::new(),
        recoverable: false,
    }
}
```

**`data`** must be a valid JSON object string (`"{"key":"value"}`). If it is not valid JSON, Flowo treats the output as empty (`{}`). It is not treated as an error.

**`recoverable`** tells the executor whether to retry this failure according to the node's `RetryPolicy` in the workflow. Set it to `true` for transient errors (network timeouts, rate limits). Set it to `false` for permanent errors (invalid input, authentication failure).

**Parameters vs. credentials.** Flowo merges resolved credential values into `input.params` before calling `execute()`. Your code does not need to look in `input.credentials` separately — everything arrives in `params`.

---

## Making HTTP requests from a plugin

WASI HTTP is available. You can use any pure-Rust HTTP client that targets `wasm32-wasip2`. The [`wasi`](https://crates.io/crates/wasi) crate exposes the raw WASI interfaces. Higher-level crates like `reqwest` do not yet support `wasm32-wasip2`, so use the WASI bindings directly or a crate that wraps them.

Example using raw WASI HTTP:

```rust
// Add to Cargo.toml: wasi = "0.14"
use wasi::http::outgoing_handler;
// ... (see wasi crate docs for full example)
```

**Important:** Flowo cannot apply SSRF protection inside the WASM sandbox. If you are running `flowo-server` in a multi-tenant or publicly accessible environment, use a host-level egress firewall to restrict what plugins can reach. This is the same recommendation made for the built-in HTTP node in [`docs/security.md`](security.md).

---

## Testing locally

Plugins are reactor components (no `main`, no `_start`). They cannot be run directly with the `wasmtime` CLI — the CLI only runs command components. To verify a plugin before loading it into Flowo:

**Inspect the component exports** using `wasm-tools`:

```bash
cargo install wasm-tools
cargo build --target wasm32-wasip2 --release
wasm-tools component wit target/wasm32-wasip2/release/my_plugin.wasm
```

This prints the resolved WIT interface exported by the component. Confirm that `flowo:plugin/node` appears in the output and that `describe` and `execute` are listed as exports.

**Integration test** — the most reliable approach:

1. Copy the `.wasm` to your plugin directory.
2. Start Flowo (desktop) or `flowo-server --plugin-dir <dir>`.
3. Confirm the node appears in the palette.
4. Create a workflow using the node and run it.
5. Check the run history for output and error details.

---

## Distributing your plugin

There is no central registry. Distribute your plugin by sharing the compiled `.wasm` file. Users place it in their plugin directory and restart Flowo.

Recommended distribution practices:
- Include the source code. Your plugin inherits Flowo's AGPL-3.0 obligations unless it is entirely independent logic (i.e., it does not incorporate any Flowo source code). If uncertain, consult the [Flowo dual-licensing guide](dual-licensing.md).
- Publish the `type_id` you use publicly so it can be coordinated across the community and avoid collisions.
- Never change the `type_id` of a published plugin. Users' saved workflows reference it. Rename by publishing a new `type_id` and deprecating the old one in your documentation.

---

## Security model

Each plugin execution runs in an isolated Wasmtime `Store` with:
- **64 MiB memory limit.** Attempts to allocate beyond this cause the plugin to receive a WASM trap.
- **No filesystem access.** Filesystem syscalls succeed at the WASM API level but all paths return "not found" or "permission denied" — no directories are mounted.
- **Outbound HTTP allowed.** Plugins can make HTTP requests via WASI HTTP.
- **No shared state.** Each `execute()` call gets a fresh store. One call cannot observe or corrupt the state of another.
- **No CPU time limit in v1.** A plugin that loops forever blocks the worker thread indefinitely. This will be addressed in a future release using Wasmtime's epoch-based interruption.

Plugin `.wasm` files are loaded only from operator-configured directories. Remote loading is not supported. Trust your plugin sources.

---

## Versioning and compatibility

The `flowo-node` world is versioned at `flowo:plugin@0.1.0`. Flowo guarantees the following stability:

- `describe()` and `execute()` function signatures will not change in a breaking way without a major version bump on the WIT package.
- WIT records have no optional fields — all fields are required and positionally encoded in the canonical ABI. Any change to record fields (`NodeInput`, `NodeOutput`, `NodeDescriptor`) is a breaking change, regardless of whether fields are added or removed, and will be signalled by a major version bump on the WIT package.
- Plugins compiled against an older version of the WIT file are binary-incompatible with a newer host that changed a record definition. Always use the WIT file that matches the Flowo version you are targeting.

If Flowo cannot load a plugin (because the WIT interface is incompatible), it logs a warning and skips the plugin. The process does not crash.
