# Writing a Aerini Plugin Node

Aerini's plugin system lets you ship new node types as `.wasm` files. Once installed, plugin nodes appear in the palette and behave identically to built-in nodes — in both the desktop app and `aerini-server`. The only visible difference is a small badge next to the node's name in the palette and search results, so users can tell at a glance which nodes came from a plugin versus shipped with Aerini — your declared icon if you have one (see [Icon and identity metadata](#icon-and-identity-metadata) below), otherwise a generic glyph. It's purely cosmetic — wiring, execution, retries, and expressions all work exactly the same.

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
        /// Node configuration parameters (from the workflow JSON), one `param`
        /// per top-level key. A non-string value is JSON-encoded into `value`
        /// (e.g. a nested object becomes its JSON text) -- re-parse per field
        /// to recover structure, or use the single reserved-key shortcut
        /// below. The host also always includes one extra entry, key
        /// `"__aerini_input_json"`, whose value is the entire merged input
        /// JSON-encoded as one object -- parse that once instead of
        /// re-parsing individual fields. Skipped (with a host-side warning)
        /// on the rare chance your own config already defines a field with
        /// that literal name, so you never silently receive host-synthesized
        /// data standing in for your own value.
        params: list<param>,
        /// Resolved credential values -- actual secrets, never IDs.
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
        /// Palette category: "action", "ai", "logic", "utility"
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

/// Optional plugin identity/branding metadata. Not required -- deliberately
/// kept out of the `aerini-node` world so plugins compiled against any
/// earlier revision of this package keep loading unchanged. A plugin that
/// does not export this interface gets host-side defaults: no icon (generic
/// glyph), empty author, "1.0.0" version.
interface metadata {
    record plugin-metadata {
        /// Semver string, e.g. "1.2.0". Empty string -> host defaults to "1.0.0".
        version: string,
        /// Author or organization name shown in the plugin's detail view. Empty string if unset.
        author: string,
        /// Icon markup: zero or more SVG shape elements only -- <path>, <circle>,
        /// <rect>, <line>, <ellipse>, <polygon>, <polyline>, <g> -- no <script>,
        /// <style>, <foreignObject>, <image>, <use>, <a>, event-handler attributes,
        /// or a `style` attribute. Rendered inside a fixed 24x24 viewBox, stroke-only,
        /// single theme-driven color -- matching the built-in node icon style exactly.
        /// Anything outside this allowlist is stripped by the host before rendering
        /// (see docs/plugin-authoring.md). Empty string -> generic plugin glyph.
        icon: string,
    }

    describe-metadata: func() -> plugin-metadata;
}

/// Extends `aerini-node` with the optional `metadata` export. Plugin authors who
/// want a custom icon/author/version target THIS world when generating guest
/// bindings, instead of `aerini-node`. Old plugins built against `aerini-node`
/// (with or without `metadata` existing in this file) keep loading unchanged --
/// the host tries this world's shape and falls back to plain `aerini-node`.
world aerini-node-with-metadata {
    export node;
    export metadata;
}

/// Optional trigger-plugin interface. A plugin exporting this alongside `node`
/// can act as a trigger source: instead of running once per workflow
/// execution like `node.execute`, the host keeps one instance alive and reads
/// a stream of events from it, each one starting a new workflow run.
///
/// Every other function in this package is synchronous; `events` is not -- it
/// requires an async-capable host runtime (WASI 0.3 / Preview 3
/// component-model-async).
interface trigger {
    /// One event produced by a trigger's `events` stream.
    record trigger-event {
        /// JSON-encoded event payload. Must be a valid JSON object string --
        /// becomes the input to the workflow run this event starts.
        data: string,
    }

    /// Begin emitting trigger events. Called once per running trigger-plugin
    /// instance, at trigger-registration time; the returned stream stays
    /// open and is drained for that instance's whole lifetime.
    ///
    /// `config` is the trigger node's resolved configuration, JSON-encoded
    /// as a single object string (nested objects/arrays intact) -- see
    /// "Implementing `events()`" below.
    events: async func(config: string) -> stream<trigger-event>;
}

/// Extends `aerini-node` with the optional `trigger` export, additive in the
/// same way `aerini-node-with-metadata` is.
world aerini-node-with-trigger {
    export node;
    export trigger;
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

**`category`** — controls palette grouping. Accepted values are `"action"`, `"ai"`, `"logic"`, and `"utility"`. If you put anything else here (including `"trigger"` or `"other"`), Aerini doesn't reject the plugin — it falls back to `"action"` and writes a warning to the log so you notice during testing. Stick to the accepted list; the fallback exists so a typo doesn't take your node out of the palette entirely, not as a second supported value.

**`input_schema` / `output_schema`** — JSON Schema (draft-07) object strings. `input_schema` drives the config panel UI; `output_schema` populates the expression picker for downstream nodes. Both must be valid JSON. If your node has no inputs or outputs, use `"{}"`.

---

## Icon and identity metadata

`metadata` is a separate, **optional** WIT interface — export it only if you want a custom icon, author name, or version shown for your plugin. Nodes that don't export it work exactly as described above; the palette shows a generic plugin glyph and no author/version, with no other behavior change.

To use it, target the `aerini-node-with-metadata` world instead of `aerini-node` when generating your guest bindings, and implement `describe-metadata()`:

```rust
fn describe_metadata() -> PluginMetadata {
    PluginMetadata {
        version: "1.2.0".to_string(),
        author: "Your Name".to_string(),
        icon: r#"<circle cx="12" cy="12" r="9" /><path d="M8 12h8" />"#.to_string(),
    }
}
```

**`icon`** is inner SVG shape markup, rendered inside a fixed 24×24 `viewBox`, stroke-only, in a single theme-driven color — the same visual language as Aerini's built-in node icons. Only these elements and attributes are allowed:

- **Elements:** `path`, `circle`, `rect`, `line`, `ellipse`, `polygon`, `polyline`, `g`
- **Attributes:** `d`, `cx`, `cy`, `r`, `rx`, `ry`, `x`, `y`, `x1`, `y1`, `x2`, `y2`, `width`, `height`, `points`, `transform`

Anything outside this allowlist — `<script>`, `<style>`, `<foreignObject>`, `<image>`, `<use>`, `<a>`, a `style`/`class`/`id`/`href` attribute, event-handler attributes, or any other tag — is stripped by the host before rendering, not rejected at install time. An empty or fully-stripped `icon` falls back to the generic glyph. Don't include your own `<svg>` wrapper, `fill`, or `stroke` color — the host supplies those so your icon matches the current theme.

Aerini's plugin loader tries the `aerini-node-with-metadata` world first and falls back to plain `aerini-node` automatically — export `metadata` and your icon, author, and version appear in the palette and plugin list with no other setup needed.

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

## Trigger plugins

> **Status: interface shipped, host not yet runnable end-to-end.** Everything in this section — the WIT interface, the config contract, event delivery via `$vars` — is final and safe to build against. But Aerini's host-side event pump for trigger plugins currently has unresolved implementation bugs, so a trigger plugin that builds correctly and passes `wasm-tools component wit` inspection still won't run to completion inside Aerini yet. Write and ship the plugin; don't promise your users it works until you've confirmed it against a released Aerini build.

A **trigger plugin** starts workflow runs itself, instead of running once per run like an ordinary node. It exports a second, optional interface — `trigger` — alongside the `node` interface every plugin already implements. Target the `aerini-node-with-trigger` world (instead of plain `aerini-node`) when generating your guest bindings.

### `execute()` is still required

WIT worlds can't partially export an interface, so a trigger-only plugin must still implement `node.execute()` — it's just never meaningfully called for that node's own logic. The convention: return trivial success immediately and do no real work, the same pattern Aerini's own built-in `Schedule` node follows for the same reason.

```rust
fn execute(_input: NodeInput) -> NodeOutput {
    NodeOutput {
        success: true,
        data: "{}".to_string(),
        error_code: String::new(),
        error_message: String::new(),
        recoverable: false,
    }
}
```

### Implementing `events()`

```rust
async fn events(config: String) -> StreamReader<TriggerEvent> {
    // ...
}
```

**`config`** is your trigger node's own config object — exactly what a workflow author sets in its config panel, with any `{{...}}` expressions already evaluated — JSON-encoded as a single object string. Parse it once with nested structure intact; unlike `node.execute`'s `params`, there's no flat-list/reserved-key split to work around here, since this interface has no published-plugin history to stay compatible with.

**`events()` is called once** per running trigger instance, at registration time. Return your stream immediately — write events to it from a spawned background task, the way you'd produce any other async stream. The call itself is time-boxed (~30s); waiting between events on the stream you've already returned is not — a trigger that goes quiet for hours between events is healthy, not stuck.

**Each `trigger-event.data`** must be a valid JSON string, matching `node.execute`'s own `data` contract. A JSON **object**'s top-level keys become the started run's `$vars` directly — so `{"order_id": "abc"}` lets downstream nodes read `{{$vars.order_id}}`. Anything else (a bare string, number, array, or invalid JSON) is wrapped under a single `"data"` key instead, so `{{$vars.data}}` always works regardless of what your plugin emits.

### What's different from an action plugin

- **No network capability.** Unlike `execute()`, which can make outbound WASI HTTP requests, a trigger plugin's instance is not linked with `wasi:http` at all today — a webhook-poller-style trigger that needs outbound HTTP isn't supported yet.
- **No filesystem access**, same as `execute()`.
- **No `category: "trigger"`.** The `category` field only accepts `"action"`, `"ai"`, `"logic"`, `"utility"` — the same list as any other plugin. Aerini recognizes a plugin as trigger-capable by where it sits in the workflow graph (no incoming connections), not by a declared category.
- **A known cosmetic issue:** placing a trigger-plugin node as a workflow's entry point currently logs a "likely a disconnected node" warning on every run — the entry-node warning's allowlist predates trigger plugins and hasn't been updated yet. The warning is spurious for a genuine trigger plugin; safe to ignore until a future release fixes it.

### Example

See [`examples/plugin-template/trigger-example`](../examples/plugin-template/trigger-example) — a heartbeat trigger that emits one timestamped event on a configurable interval. It implements both `execute()` (no-op, per above) and `events()`, and is a working reference for the stream-writing pattern even though — per the status note at the top of this section — it can't be run end-to-end inside Aerini yet.

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

- **Plugin folder** — a path on disk, set once via the **Browse** button. This is the folder Aerini scans for plugin files. If it's not set yet, the panel says so and the rest of the panel is inactive until you pick one.
- **Installed plugins** — a list of every installed plugin in that folder, showing the node's display name and its `type_id`. If a file is in the folder but fails to load (wrong WIT version, corrupted build, etc.), it still shows up in the list with an error message instead of just vanishing — so a broken plugin doesn't silently disappear and leave you wondering where it went. A [multi-node package](#multi-node-packages) is shown as a group with its own **Remove pack** action, above the individual **Remove** button each of its member plugins still has — removing just one member that way leaves the rest of the pack installed but its manifest referencing a file that's no longer there, so prefer **Remove pack** unless you specifically want to break one member out on its own.
- **Install** — opens a file picker accepting either a single `.wasm` plugin or a `.aerinipkg` multi-node package. Pick the file, Aerini copies it into the plugin folder, and it appears in the list — as a single row for a `.wasm`, or as a pack group for a `.aerinipkg`.

**Drag-and-drop also works.** Drag a `.wasm` or `.aerinipkg` file straight onto the Aerini window. If a plugin folder is already set, it's installed immediately with a success toast. If no folder is set yet, you get a toast pointing you to Settings → Plugins to set one first.

> If you're running Aerini in a browser during development (not the packaged desktop app), drag-and-drop install doesn't work — the browser sandbox doesn't give web pages access to dropped files' real paths. Use the **Install** button in Settings instead; that goes through the desktop file picker and isn't affected.

**No restart needed.** After any install or remove, Aerini reloads the plugin registry live — the palette updates immediately, with no app restart. A dismissible banner asking you to restart only appears as a fallback, if that live reload call itself fails; installation or removal has already succeeded on disk either way.

**Removing a plugin** deletes the `.wasm` file from the plugin folder. **Removing a pack** deletes its manifest and every member `.wasm` file it lists. Either way, any saved workflows that still reference the removed `type_id`(s) will fail to load that node — Aerini doesn't quietly substitute anything in its place.

`aerini-server` uses the same plugin folder mechanism but has no Settings UI. Point it at the folder with `--plugin-dir <path>` (or the equivalent config key). It also reloads live, without a restart: `serve` mode picks up a `SIGHUP` signal, and `api` mode exposes an admin-scoped `POST /api/plugins/reload` endpoint.

---

## Distributing your plugin

There's no central registry. Share the compiled `.wasm` file — your users install it via Settings → Plugins or drag-and-drop, as described above.

Practices worth following:

- **Publish the source.** Your plugin may inherit Aerini's AGPL-3.0 obligations depending on how it uses Aerini code. If uncertain, see [dual-licensing.md](dual-licensing.md).
- **Never change a published `type_id`.** Users' saved workflows reference it by this string. If you need to rename, publish a new `type_id` alongside the old one and document the transition.
- **Document your `type_id` publicly** to avoid collisions with other plugin authors.

---

## Signing your plugin

Signing is optional — an unsigned `.wasm` still installs (see [Distributing your plugin](#distributing-your-plugin) above) — but it lets Aerini detect a tampered file or a swapped publisher key on update, via the `<name>.wasm.sig` sidecar `check_signature` reads (`src-tauri/src/commands/plugins.rs`).

**Format**, one `.wasm.sig` per `.wasm`:

```json
{
  "schema_version": 1,
  "algorithm": "ed25519",
  "public_key": "<base64, 32 raw bytes>",
  "files": [ { "name": "plugin.wasm", "blake3": "<hex>" } ],
  "signature": "<base64, 64 raw bytes>"
}
```

`files` must contain exactly one entry for a standalone plugin — Aerini rejects any other count as an unrecognized sidecar. (A [package](#multi-node-packages)'s `pack.json.sig` reuses this exact shape with more entries, one per member plus the manifest — see [Signing a package](#signing-a-package) below.) The signature doesn't cover the raw JSON above; it covers a deterministic byte string built from `files` alone (a domain-separation prefix, then each entry's name and hash) — re-serializing the JSON differently never invalidates a valid signature, and there's nothing in `.wasm.sig` itself for Aerini to trust before that verification passes.

### Generating a key and signing

A companion script, `examples/plugin-template/sign_plugin.py`, produces this exact format. Needs `pip install blake3 cryptography`.

```bash
# Once, per publisher identity -- writes a 32-byte raw private key.
# Keep this file secret: anyone holding it can sign updates Aerini will
# accept for any type_id already pinned to its public key.
python sign_plugin.py keygen --out publisher.key

# Per release.
python sign_plugin.py sign --key publisher.key --wasm plugin.wasm
# -> writes plugin.wasm.sig next to it

# Optional self-check before publishing.
python sign_plugin.py verify --sig plugin.wasm.sig
```

Ship `plugin.wasm.sig` alongside `plugin.wasm` — same install/distribution path as an unsigned plugin, described above.

### Key handling and updates

- **Aerini trusts on first install (TOFU), not on any external channel.** The first time a `type_id` installs with a valid signature, its public key is pinned then and there — there's no registry to publish the key to beyond that.
- **A key rotation looks like an attack from Aerini's side.** Signing an update with a new key, for a `type_id` already pinned to an older one, is rejected as a downgrade/key-swap rather than trusted. Users have to remove the plugin and reinstall fresh under the new key. Plan around this — don't rotate keys casually.
- **Losing the key** means future updates can still ship unsigned (they install, just without the signature check) — but you can never again sign as that publisher identity for a `type_id` already pinned to it.

---

## Multi-node packages

If your plugin ships more than one node type, bundle them into a single **`.aerinipkg`** package instead of asking users to install each `.wasm` file separately. A pack installs, updates, and removes all of its member nodes as one unit.

### Building a package

A `.aerinipkg` is a flat zip — no subdirectories — containing:

- `pack.json` — the manifest (see below)
- Every member `.wasm` file it lists, already built exactly as you would for a standalone plugin
- Optionally, `pack.json.sig` — a publisher signature covering the manifest and every member (see [Signing a package](#signing-a-package) below)

```bash
zip -j my-pack.aerinipkg pack.json node-a.wasm node-b.wasm
```

**`pack.json` schema (`schema_version: 1`):**

```json
{
  "schema_version": 1,
  "pack_id": "com.example.mypack",
  "display_name": "My Pack",
  "files": ["node-a.wasm", "node-b.wasm"]
}
```

- `pack_id` must match `^[A-Za-z0-9][A-Za-z0-9._-]*$` — same discipline as `type_id`. Reverse-domain format is recommended, and it's your package's identity for updates and removal, so treat it with the same "never change it after publishing" discipline as a `type_id`.
- `files` must list every `.wasm` member's exact filename; the package's actual contents must match this list exactly, or the install is rejected.

Caps: at most 64 files per package, 64 MiB per member file, 256 MiB total, 64 KiB for `pack.json` itself. A package exceeding any of these is rejected outright.

### Installing, updating, and removing

From the user's side this is the same **Install** flow described above — the file picker and drag-drop both accept `.aerinipkg` alongside `.wasm` and dispatch to the right install path automatically. A few things behave differently from a single-plugin install because a pack is a group:

- **Updating** a pack (installing a new `.aerinipkg` with a `pack_id` already installed) replaces its entire member set: files the new manifest no longer lists are removed, new ones are added.
- **Conflicts are never silently adopted.** If a member's `type_id` or filename is already owned by a *different*, already-installed plugin or pack, the install is rejected rather than overwriting it — remove the conflicting install first if that's genuinely what you want.
- **Removing** a pack removes its manifest and every member file it lists, in one action.

### Signing a package

A publisher may ship an optional `pack.json.sig` sidecar alongside the `.aerinipkg`'s contents, covering the manifest and every member file under one ed25519 signature. On first install of a given `pack_id`, Aerini pins the signing key (TOFU); a later update signed by a *different* key is rejected rather than silently trusted, so a compromised or swapped publisher key on an update is caught rather than silently accepted. An unsigned package still installs — signing is a trust signal, not a requirement — but an update can't silently drop a signature a previous version had.

This sidecar reuses the same signature format as a standalone plugin's `.wasm.sig` file (see [Signing your plugin](#signing-your-plugin) above), just covering N files (the manifest plus every member) instead of one. `sign_plugin.py` only produces the single-file form described there; a companion script for the multi-entry `pack.json.sig` shape doesn't exist yet.

---

## Security model

Each plugin execution runs in an isolated Wasmtime `Store`:

- **64 MiB memory limit.** Allocation beyond this causes a WASM trap.
- **No filesystem access.** All filesystem calls return errors — no directories are mounted.
- **Outbound HTTP allowed** via WASI HTTP.
- **No shared state between calls.** Each `execute()` call gets a fresh store. One call can't read or corrupt another's state.
- **~30s CPU time limit** on `describe()` and `execute()` calls, enforced by Wasmtime epoch-based interruption — a call that runs past the deadline is trapped, not left to block the worker thread indefinitely. A trigger plugin's `events()` call is bound by the same ~30s deadline only until it *returns* its stream; time spent waiting between events afterward is unbounded by design (see [Trigger plugins](#trigger-plugins)).

Plugin `.wasm` files are loaded only from operator-configured directories. Remote plugin loading is not supported.

---

## Versioning and compatibility

The `aerini-node` world is versioned at `aerini:plugin@0.1.0`. Stability guarantees:

- `describe()` and `execute()` signatures won't change without a major WIT package version bump.
- All record fields (`NodeInput`, `NodeOutput`, `NodeDescriptor`) are required and positionally encoded in the canonical ABI. Any field change — addition or removal — is a breaking change and will bump the major version.
- Plugins compiled against an older WIT version are binary-incompatible with a host that changed a record definition. Always target the WIT file that matches your Aerini version.

If Aerini can't load a plugin due to an incompatible interface, it logs a warning and skips the plugin. The process continues normally.
