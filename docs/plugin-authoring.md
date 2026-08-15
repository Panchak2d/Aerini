# Writing a Aerini Plugin Node

Aerini's plugin system lets you ship new node types as `.wasm` files. Once installed, plugin nodes appear in the palette and behave identically to built-in nodes — in both the desktop app and `aerini-server`. The only visible difference is a small grey **"P"** badge next to the node's name in the palette and search results, so users can tell at a glance which nodes came from a plugin versus shipped with Aerini. It's purely cosmetic — wiring, execution, retries, and expressions all work exactly the same.

This guide assumes you know some Rust. If you're new to Rust, the [Rust Book](https://doc.rust-lang.org/book/) covers the fundamentals; most of what you need for a plugin is covered in the first ten chapters.

---

## What a plugin can do

A plugin node receives its configuration parameters and any resolved credential values, runs your logic, and returns a JSON object. Downstream nodes can reference that output using the normal `{{...}}` expression syntax.

From inside the plugin sandbox, you can:
- Make outbound HTTP requests (WASI HTTP is available)
- Use Rust's standard library and any pure-Rust crates
- Perform CPU-bound computation

You cannot:
- Read or write the local filesystem (all filesystem calls return errors — no directories are mounted)
- Bind ports or accept inbound connections
- Access Aerini's SQLite database or credential store directly
- Call back into Aerini's Rust runtime (communication is one-directional: host calls plugin, not the other way around)

---

## Prerequisites

- **Rust 1.82 or later** — earlier versions don't include the `wasm32-wasip2` target
- **The WASM target:**
  ```bash
  rustup target add wasm32-wasip2
  ```

That's it. You don't need `cargo-component`, `wasm-opt`, or any other tooling. The `wasm32-wasip2` target produces a WASM Component directly using the standard Rust toolchain.

---

## Start from the template

Copy the included template rather than starting from scratch:

```bash
# From the Aerini repo root
cp -r examples/plugin-template my-plugin
cd my-plugin
```

The template has everything pre-configured: the correct `crate-type`, the `wit-bindgen` dependency, and the WIT file. Rename the crate in `Cargo.toml`, then edit `src/lib.rs`.

Verify the template builds before you touch anything:

```bash
cargo build --target wasm32-wasip2 --release
```

If that succeeds, your environment is set up correctly.

---

## The WIT interface

Every Aerini plugin implements the `aerini-node` world defined in `wit/node.wit`. You won't usually need to edit this file — it's included in the template and you implement the functions it defines.

The full annotated interface:

```wit
package aerini:plugin@0.1.0;

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
        /// Resolved credential values — actual secrets, never IDs.
        credentials: list<param>,
    }

    /// Output returned from a plugin node's execute function.
    record node-output {
        success: bool,
        /// JSON-encoded output object. Must be a valid JSON object string.
        data: string,
        /// Short machine-readable error code. Empty string on success.
        error-code: string,
        /// Human-readable error message. Empty string on success.
        error-message: string,
        /// If true, the executor may retry this node per its RetryPolicy.
        recoverable: bool,
    }

    /// Metadata about this node type. Returned by describe().
    record node-descriptor {
        /// Unique ID. Use reverse-domain format: "com.example.my-node"
        type-id: string,
        /// Display name shown in the palette.
        display-name: string,
        /// Palette category: "trigger", "action", "ai", "logic", "utility", "other"
        category: string,
        /// Short description shown in the UI.
        description: string,
        /// JSON Schema string for node inputs (drives the config panel).
        input-schema: string,
        /// JSON Schema string for node outputs (drives the expression picker).
        output-schema: string,
    }
}

interface node {
    use types.{node-input, node-output, node-descriptor};

    /// Return metadata about this node type. Called once at load time.
    describe: func() -> node-descriptor;

    /// Execute the node. Called on every workflow run that reaches this node.
    execute: func(input: node-input) -> node-output;
}

world aerini-node {
    export node;
}
```

WIT uses kebab-case for field names. `wit-bindgen` maps them to snake_case in Rust: `type-id` → `type_id`, `error-code` → `error_code`, and so on.

---

## Implementing `describe()`

`describe()` is called once when Aerini loads your plugin. The returned descriptor is cached — it won't be called again during normal operation.

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

**Key decisions:**

**`type_id`** — must be unique across all installed plugins. Use reverse-domain notation: `com.yourname.node-name`. This ID is stored in workflow files; changing it after publishing breaks every saved workflow that uses your node. Treat it as permanent.

**`category`** — controls palette grouping. Accepted values are `"trigger"`, `"action"`, `"ai"`, `"logic"`, `"utility"`, and `"other"`. If you put anything else here, Aerini doesn't reject the plugin — it falls back to `"action"` and writes a warning to the log so you notice during testing. Stick to the accepted list; the fallback exists so a typo doesn't take your node out of the palette entirely, not as a second supported value.

**`input_schema` / `output_schema`** — JSON Schema (draft-07) object strings. `input_schema` drives the config panel UI; `output_schema` populates the expression picker for downstream nodes. Both must be valid JSON. If your node has no inputs or outputs, use `"{}"`.

---

## Implementing `execute()`

`execute()` receives a `NodeInput` and returns a `NodeOutput`. It's synchronous from the guest side — WASI handles the async I/O internally.

```rust
fn execute(input: NodeInput) -> NodeOutput {
    // Find a parameter by key
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

**`data`** must be a valid JSON object string. If your node reports `success: true` but `data` isn't valid JSON, Aerini treats it as a failure: the run logs an error and the node fails with error code `wasm_invalid_output`, rather than silently substituting `{}`.

**`recoverable`** tells the executor whether to retry on failure (based on the node's retry settings in the workflow). Use `true` for transient problems like network timeouts and rate limits; use `false` for permanent failures like missing required input or authentication errors.

**Parameters and credentials both arrive in `params`.** Aerini merges resolved credential values into `input.params` before calling `execute()`. You don't need to check `input.credentials` separately — everything is in one place.

---

## Declaring credential fields

By default, the config UI treats any field literally named `api_key` or `password` in your `input-schema` as a credential field — it's hidden from the generic config form and shown in the "Connection" section with a saved-credential picker instead. This still works and isn't going away.

For anything else — multiple credential fields on one node, or a field name that isn't `api_key`/`password` — opt in explicitly with the `x-aerini-credential` JSON Schema annotation:

```json
{
  "properties": {
    "access_key_id":     { "type": "string", "x-aerini-credential": true },
    "secret_access_key": { "type": "string", "x-aerini-credential": { "cred_type": "api_key" } }
  }
}
```

- `"x-aerini-credential": true` marks the field as a credential field with no type constraint — the picker shows every saved credential.
- `"x-aerini-credential": { "cred_type": "..." }` filters the picker to only saved credentials of that type. Valid values: `"api_key"`, `"bearer"`, `"basic"`, `"oauth"`, `"other"` — the type a user picks when saving a credential in Aerini's Credentials panel, not a name you invent.
- A node can declare as many credential fields as it needs; each gets its own picker.
- If a `cred_type` is declared and the user has no saved credential of that type, the picker shows no options rather than falling back to an unrelated credential type — they'll need to save one of the right type first.

Either convention resolves the same way: the secret arrives merged into `params` as described above, keyed by the field name.

---

## Making HTTP requests

WASI HTTP is available. Use any pure-Rust HTTP client that targets `wasm32-wasip2`. The [`wasi`](https://crates.io/crates/wasi) crate exposes the raw interfaces. Higher-level crates like `reqwest` don't yet support `wasm32-wasip2`.

```toml
# Cargo.toml
[dependencies]
wasi = "0.14"
```

See the [wasi crate documentation](https://docs.rs/wasi) for usage examples.

**Security note:** Aerini cannot apply SSRF protection inside the WASM sandbox. If you're running `aerini-server` in a multi-tenant or publicly accessible environment, configure a host-level egress firewall to restrict what plugins can connect to. See [Security — SSRF protection](security.md#5-http-node--ssrf-protection).

---

## Testing your plugin

Plugins are reactor WASM components, not commands — you can't run them directly with the `wasmtime` CLI. There are two approaches for testing.

**Inspect exports first:**

```bash
cargo install wasm-tools
cargo build --target wasm32-wasip2 --release
wasm-tools component wit target/wasm32-wasip2/release/my_plugin.wasm
```

Confirm that `aerini:plugin/node` appears in the output and that both `describe` and `execute` are listed as exports. If they're missing, the component isn't implementing the interface correctly.

**Integration test in Aerini:**

1. Copy the `.wasm` file to your plugin directory
2. Start Aerini (desktop) or `aerini-server --plugin-dir <path>`
3. Confirm the node appears in the palette
4. Build a workflow using your node and run it
5. Check the run history for output and any error details

The integration test is the definitive check — it catches ABI mismatches that wasm-tools can't.

---

## Installing and managing plugins (desktop app)

This part is for whoever is *using* the plugin, not writing it — point your users at this section.

Open **Settings → Plugins**. The panel has three parts:

- **Plugin folder** — a path on disk, set once via the **Browse** button. This is the folder Aerini scans for `.wasm` files. If it's not set yet, the panel says so and the rest of the panel is inactive until you pick one.
- **Installed plugins** — a list of every `.wasm` file in that folder, showing the node's display name and its `type_id`. If a file is in the folder but fails to load (wrong WIT version, corrupted build, etc.), it still shows up in the list with an error message instead of just vanishing — so a broken plugin doesn't silently disappear and leave you wondering where it went. Each row has a **Remove** button.
- **Install .wasm** — opens a file picker restricted to `.wasm` files. Pick the file, Aerini copies it into the plugin folder, and it appears in the list.

**Drag-and-drop also works.** Drag a `.wasm` file straight onto the Aerini window. If a plugin folder is already set, it's installed immediately with a success toast. If no folder is set yet, you get a toast pointing you to Settings → Plugins to set one first.

> If you're running Aerini in a browser during development (not the packaged desktop app), drag-and-drop install for `.wasm` files doesn't work — the browser sandbox doesn't give web pages access to dropped files' real paths. Use the **Install .wasm** button in Settings instead; that goes through the desktop file picker and isn't affected.

**Restart required.** After any install or remove, a dismissible banner appears reminding you to restart Aerini. Plugins are loaded once at startup — installing or removing a file doesn't hot-swap the running palette. The banner doesn't restart anything for you and disappears if you dismiss it or close Aerini; it's just a reminder for that session.

**Removing a plugin** deletes the `.wasm` file from the plugin folder. Any saved workflows that still reference that plugin's `type_id` will fail to load that node after restart — Aerini doesn't quietly substitute anything in its place.

`aerini-server` uses the same plugin folder mechanism but has no Settings UI — point it at the folder with `--plugin-dir <path>` (or the equivalent config key) and restart the process to pick up changes.

---

## Distributing your plugin

There's no central registry. Share the compiled `.wasm` file — your users install it via Settings → Plugins or drag-and-drop, as described above.

Practices worth following:

- **Publish the source.** Your plugin may inherit Aerini's AGPL-3.0 obligations depending on how it uses Aerini code. If uncertain, see [dual-licensing.md](dual-licensing.md).
- **Never change a published `type_id`.** Users' saved workflows reference it by this string. If you need to rename, publish a new `type_id` alongside the old one and document the transition.
- **Document your `type_id` publicly** to avoid collisions with other plugin authors.

---

## Security model

Each plugin execution runs in an isolated Wasmtime `Store`:

- **64 MiB memory limit.** Allocation beyond this causes a WASM trap.
- **No filesystem access.** All filesystem calls return errors — no directories are mounted.
- **Outbound HTTP allowed** via WASI HTTP.
- **No shared state between calls.** Each `execute()` call gets a fresh store. One call can't read or corrupt another's state.
- **No CPU time limit (v1).** A plugin that loops indefinitely blocks the worker thread. This will be addressed in a future release using Wasmtime epoch-based interruption. Don't ship plugins with unbounded loops.

Plugin `.wasm` files are loaded only from operator-configured directories. Remote plugin loading is not supported.

---

## Versioning and compatibility

The `aerini-node` world is versioned at `aerini:plugin@0.1.0`. Stability guarantees:

- `describe()` and `execute()` signatures won't change without a major WIT package version bump.
- All record fields (`NodeInput`, `NodeOutput`, `NodeDescriptor`) are required and positionally encoded in the canonical ABI. Any field change — addition or removal — is a breaking change and will bump the major version.
- Plugins compiled against an older WIT version are binary-incompatible with a host that changed a record definition. Always target the WIT file that matches your Aerini version.

If Aerini can't load a plugin due to an incompatible interface, it logs a warning and skips the plugin. The process continues normally.
