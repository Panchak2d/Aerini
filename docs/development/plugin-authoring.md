# Writing a Plugin Node

A plugin is a node type distributed as a compiled `.wasm` file instead of Rust source merged into `aerini-engine`. You write it in any language that targets `wasm32-wasip2`, build it, and drop the file into a plugin directory; Aerini loads it at startup or on demand and it behaves like any other node in the palette, on the canvas, and in a running workflow. This page covers that path end to end: the interface you implement, what the optional extensions cost, the sandbox you run inside, and how installation and signing actually work. It assumes you've read [Architecture](architecture.md), which covers `plugin_loader.rs` in outline, and [Nodes Reference](../guide/nodes.md), for what a node looks like from a workflow author's side. Adding a node compiled directly into the engine instead is [Adding a Built-in Node](node-authoring.md)'s job, a different artifact with a different build and registration story.

## The node interface

Every plugin implements one required WIT interface, declared in `aerini-engine/wit/node.wit`:

```wit
interface node {
    describe: func() -> node-descriptor;
    execute: func(input: node-input) -> node-output;
}
```

`describe()` runs once, at load time, and its return value (`type_id`, `display_name`, `category`, `description`, and the two JSON Schema strings) is cached for the process's whole lifetime. Nothing you return from it can change without a reload. `category` is one of `"action"`, `"ai"`, `"logic"`, or `"utility"`, the same four `NodeType` variants built-in nodes use; anything else falls back to `"action"` with a warning logged, not a load failure.

`execute()` runs once per workflow run that reaches the node. `node-input.params` carries your node's resolved configuration as a flat list of key/value pairs, already past credential resolution: if a workflow author connects a saved credential to one of your fields, you receive the plaintext value directly, the same as a built-in node would, never an id you have to look up. A nested object or array in the config comes through as its JSON text under that key, which is awkward to reconstruct field by field, so the host also always includes one extra entry, key `__aerini_input_json`, whose value is the entire merged config as one JSON object string. Parse that once instead of walking the flat list if your schema has anything beyond flat scalars. On the rare chance your own schema already defines a field literally named `__aerini_input_json`, the host skips synthesizing its own entry rather than overwriting yours, so you always get your own value back and never host-synthesized data standing in for it silently. The `credentials` field on `node-input` exists in the WIT type but the host never populates it; every credential value you need already arrives through `params`.

Return `node-output` with `success` and `data` set for a normal result, or `success: false` with `error-code`/`error-message` for an expected failure, the same distinction `NodeOutput::failure` and `NodeOutput::success` make for a built-in node. Set `recoverable` when the executor's retry policy should get another attempt at the same input; don't set it for a failure that will produce the identical result every time. Never rely on a Rust panic, or your host language's equivalent, to signal failure: a trap is treated as unrecoverable no matter what actually went wrong, and the workflow just sees `wasm_trap` with no detail from inside your code.

## Building the template

`examples/plugin-template/` is a complete, minimal plugin: an Echo node that returns whatever you pass into a `message` field.

```toml
[package]
name = "aerini-plugin-echo"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
wit-bindgen = "0.57.1"
```

```rust
wit_bindgen::generate!({ world: "aerini-node" });

use exports::aerini::plugin::node::{Guest, NodeDescriptor, NodeInput, NodeOutput};

struct EchoPlugin;

impl Guest for EchoPlugin {
    fn describe() -> NodeDescriptor {
        NodeDescriptor {
            type_id: "com.example.echo".to_string(),
            display_name: "Echo".to_string(),
            category: "utility".to_string(),
            description: "Echoes the message parameter to output.".to_string(),
            input_schema: r#"{"type":"object","properties":{"message":{"type":"string"}},"required":["message"]}"#.to_string(),
            output_schema: r#"{"type":"object","properties":{"echo":{"type":"string"}}}"#.to_string(),
        }
    }

    fn execute(input: NodeInput) -> NodeOutput {
        let message = input.params.iter().find(|p| p.key == "message").map(|p| p.value.as_str()).unwrap_or("");
        NodeOutput {
            success: true,
            data: format!(r#"{{"echo":"{message}"}}"#),
            error_code: String::new(),
            error_message: String::new(),
            recoverable: false,
        }
    }
}

export!(EchoPlugin);
```

Build it with the `wasm32-wasip2` target, added once via `rustup target add wasm32-wasip2`:

```bash
cargo build --target wasm32-wasip2 --release
```

The output lands at `target/wasm32-wasip2/release/aerini_plugin_echo.wasm`. Use `type_id` in reverse-domain form (`com.yourname.your-node`) so it doesn't collide with a future built-in or someone else's plugin; see [Installing your plugin](#installing-your-plugin) for what actually happens on a collision. Rename the crate, fill in your own `describe()` and `execute()`, and you have a working node. Everything past this point is optional.

## Icon and identity metadata

By default a plugin gets no icon (a generic glyph is shown instead), an empty author field, and version `"1.0.0"`. To set your own, export a second, additive interface alongside `node`:

```wit
interface metadata {
    record plugin-metadata {
        version: string,
        author: string,
        icon: string,
    }
    describe-metadata: func() -> plugin-metadata;
}
```

Target the `aerini-node-with-metadata` world in your `generate!` call instead of plain `aerini-node`; the host tries this shape first when loading any plugin and falls back to the base interface automatically, so this costs you nothing in compatibility either direction. `icon` is inner SVG shape markup only, not a full `<svg>` document: `<path>`, `<circle>`, `<rect>`, `<line>`, `<ellipse>`, `<polygon>`, `<polyline>`, and `<g>` are allowed, rendered stroke-only inside a fixed 24x24 viewBox to match the built-in node icon style. Anything outside that allowlist, including `<script>`, `<style>`, `<image>`, `<use>`, event-handler attributes, and a `style` attribute, is stripped by the host before it ever reaches the page; an empty or invalid icon just falls back to the generic glyph rather than failing the load. Leaving any field in `plugin-metadata` empty falls back to the same default that field would have gotten without the interface at all, so you can implement only the parts you care about.

## Trigger plugins

A plugin can supply its own trigger instead of running once per execution: export `trigger` alongside `node`, targeting the `aerini-node-with-trigger` world.

```wit
interface trigger {
    record trigger-event {
        data: string,
    }
    events: async func(config: string) -> stream<trigger-event>;
}
```

The host keeps one instance of a trigger-capable plugin alive per placement and reads events from the stream `events()` returns; each event starts a new workflow run with that event's `data` as input. This is genuinely a different build, not just a different function: `events` is async, which requires the WASI Preview 3 / component-model-async ABI instead of the synchronous one `describe`/`execute` use, so a trigger plugin needs the `wasip3` crate (which re-exports its own `wit_bindgen::generate!`) rather than plain `wit-bindgen`. The host detects trigger capability by actually compiling and link-checking your file against a second, async-only engine at load time, not by anything you declare in `describe()`; there's no `"trigger"` category, and `category` in `describe()` stays one of the four normal values regardless. `examples/plugin-template/trigger-example/` is a complete reference implementation of a fixed-interval heartbeat trigger, built the same way (`cargo build --target wasm32-wasip2 --release` from inside that directory).

The 30-second deadline described below still bounds the initial `events(config)` call itself, the same figure and same reason: the host has to know the call started, not just handed back an inert stream. Once that stream is returned, though, it stays open indefinitely with no further deadline, since a trigger is meant to keep running rather than return; the 64 MiB memory cap and SSRF-filtered outbound HTTP still apply throughout. As of this page, the host-side event pump that drains a trigger plugin's stream has open bugs: the example above builds and traces correctly as a reference for the guest-side pattern, but isn't yet runnable end to end inside a live Aerini instance. Don't ship a trigger plugin expecting it to fire until that's resolved.

## Storage

A plugin can also import a small key-value store the host provides, scoped per plugin `type_id` rather than per placement, so every workflow using your plugin shares one namespace:

```wit
interface storage {
    get: func(key: string) -> option<string>;
    set: func(key: string, value: string) -> result<_, storage-error>;
    delete: func(key: string);
    list-keys: func(prefix: string) -> result<list<string>, storage-error>;
}
```

This is an import, not an export: the host links it into every plugin's linker unconditionally, so a plugin built before this interface existed is unaffected, and one that wants it just declares the import in its own hand-written world (or targets `aerini-node-with-storage`, the convenience world that pairs it with `node`). It's meant for small durable notes, not a general-purpose database: hosts enforce a per-plugin quota on total bytes and key count, and `set`/`list-keys` return a `storage-error` on `key-too-long`, `value-too-large`, `quota-exceeded`, or a backend `unavailable`. `get` and `delete` never fail; a backend problem there just looks like "key absent" rather than an error, since neither can lose data you've already confirmed was written. Every call is a no-op during `describe()`, before your plugin's own `type_id` is even known to scope storage by; it's only backed by a real store once `execute()` runs.

## The plugin sandbox

Each `execute()` call gets a fresh Wasmtime instance, never reused between calls, capped at 64 MiB of linear memory and a 30-second wall-clock deadline; a trigger plugin's `events()` call is bound by the same 30 seconds to start its stream, but the stream itself has no further deadline once returned (more on this above). Filesystem access is fully denied; there are no preopened directories, so any file operation your plugin attempts fails. Outbound HTTP is the only network access available, and every request goes through the same SSRF policy the built-in HTTP Request and Database nodes enforce, with no per-plugin opt-out. [Security](../guide/security.md) covers this sandbox boundary as part of Aerini's overall security model, including how it compares to the Code (JS) node's much looser default; this page only restates the parts that change what you can build against.

## What happens to a bad plugin

A file that fails to compile against the Component Model, fails to link (missing an import the host doesn't provide), or doesn't export the `node` interface at all is skipped and logged as a warning; it never reaches the registry, and it doesn't stop any other plugin in the same directory from loading. A plugin that compiles and loads correctly but then panics, exceeds the 64 MiB memory cap, or runs past its deadline during `execute()` traps: the executor sees that as an unrecoverable failure with error code `wasm_trap`, with no way to recover the actual panic message or line from inside the sandbox. Design your `execute()` to return a normal `success: false` output for anything you can anticipate, and treat a trap as a bug to fix in your own code, not a condition a workflow author can work around.

## Signing your plugin

`examples/plugin-template/sign_plugin.py` generates and checks an optional `<file>.wasm.sig` sidecar next to your `.wasm` file: an Ed25519 signature over a BLAKE3 hash of the file's contents, under a keypair you generate and hold yourself.

```bash
python sign_plugin.py keygen --out publisher.key
python sign_plugin.py sign --key publisher.key --wasm aerini_plugin_echo.wasm
python sign_plugin.py verify --sig aerini_plugin_echo.wasm.sig
```

There's no central registry or certificate authority behind this: signing proves the file matches what you published under a given key and hasn't been altered since, not that anyone has reviewed it. An unsigned plugin loads exactly the same as a signed one; nothing here is required. What signing does change is what happens if the file changes later: if the content no longer matches the hash the sidecar declares, or the signature itself doesn't verify against the declared key, the plugin is rejected outright at load time rather than silently loaded anyway. The desktop app's Plugins panel shows a signature status per installed file and remembers the public key the first time it sees a valid one for a given `type_id`, so a later install claiming that same `type_id` under a different key is visibly a key change, not a silent takeover. Keep `publisher.key` private; anyone who has it can sign updates that your users' trust store will accept as coming from you.

## Installing your plugin

In the desktop app, **Settings → Plugins** sets a plugin directory (a folder picker), then installs a `.wasm` file into it either through the file picker or by dragging the file onto the window; both paths end up at the same install step, and Aerini reloads the plugin registry live afterward, no restart needed. `aerini-server` instead reads every `.wasm` file in the directory passed via `--plugin-dir` at startup. A `type_id` that collides with a built-in node, or with another already-installed plugin, doesn't crash anything: the file stays on disk and shows up in the Plugins list, but with a warning that it isn't active until the id is changed. Multiple plugins can also be bundled and signed together as one `.aerinipkg` package for distribution; that packaging format is a separate concern from writing a single node and isn't covered here.

## The danger badge

Every plugin node is treated as dangerous in the UI, the same one-time confirmation dialog and canvas badge as Shell Command, Code (JS), and Database, covered in [Nodes Reference](../guide/nodes.md). This is set by the host, not something your `describe()` or `execute()` can influence either way; there's no field to opt out of it, and there shouldn't be, since the warning exists for whoever's running a workflow, not for you as the plugin's author.

## What this page doesn't cover

[Adding a Built-in Node](node-authoring.md) covers the Rust-compiled-into-`aerini-engine` path, the same `Node` trait from a different, non-sandboxed side. [Desktop IPC Reference](desktop-ipc-reference.md) covers the Tauri commands behind the Plugins panel itself (`install_plugin_from_path`, `reload_plugins`, and the rest), if you need the exact request/response shape rather than the UI flow described above. [Security](../guide/security.md) is the full account of the sandbox this page only summarizes, including how it compares to every other node type Aerini treats as dangerous.
