# Contributing to Aerini

Aerini is a Tauri 2.0 desktop app for building visual automation workflows.
The codebase is split into three Cargo crates and a TypeScript frontend.

---

## Contributor License Agreement

Before your first pull request can be merged, you must sign the
[Contributor License Agreement](CLA.md).

**Individual contributors:** This project uses [CLA Assistant](https://cla-assistant.io). On your first PR it posts a comment with a one-click sign link — GitHub OAuth, done in seconds.

**Corporate contributors (contributing on behalf of your employer):**
Your company must sign the Corporate CLA before any code from your employees
can be merged. See [CLA.md](CLA.md) Section C8 for instructions. See
[CONTACT.md](CONTACT.md) for the contact address before opening a PR.

---

## Architecture

```
aerini-engine/     — core library: Node trait, executor, scheduler, DB, 39 nodes
src-tauri/        — Tauri shell: IPC commands, tray, app lifecycle
aerini-server/     — headless binary for server-side workflow execution
src/              — TypeScript/Vite frontend (vanilla TS, no framework)
```

`aerini-engine` has zero Tauri dependency. Both `src-tauri` and `aerini-server` depend on it.
The decoupling point is the `EventSink` trait — the Tauri app and the server binary implement
it differently, but the executor and scheduler use only the trait.

---

## Dev environment

| Tool | Version |
|------|---------|
| Rust (stable) | 1.77+ |
| Node.js | 18+ (only to run `npm`/Vite — not needed to use the Code (JS) node, see below) |
| Tauri CLI | 2.x (`cargo install tauri-cli --version "^2"`) |

The Code (JS) node runs on a Node.js runtime bundled into Aerini, not your system install. Before the first `npm run dev`, stage it:

```bash
npm install
./scripts/fetch-node-binaries.sh   # downloads + checksum-verifies bundled Node.js, one time per clone
npm run dev       # Vite dev server + Tauri window
npm run build     # production bundle → src-tauri/target/release/bundle/
```

`fetch-node-binaries.sh` needs `curl`, `tar`, `unzip` (or `powershell.exe`), and `sha256sum`/`shasum` on PATH, plus network access to nodejs.org. Skipping it fails the Rust build with `resource path 'binaries/node-bundled-...' doesn't exist`.

> ⚠️ **`dist/` is intentionally committed — do not delete it or add it to `.gitignore`.**
>
> Tauri reads compiled frontend assets from `dist/` at build time. Without it, `cargo tauri build` fails.
> Only sourcemaps (`dist/**/*.map`) are gitignored — `dist/` itself is not.
>
> When contributing: rebuild with `npm run build` before `cargo tauri build` if you changed the frontend,
> but **do not commit the rebuilt `dist/`** unless you are a maintainer cutting a release.

---

## Adding a new node

All 39 built-in nodes follow the same pattern. Copy any existing node file as a starting point
(e.g. `aerini-engine/src/nodes/delay.rs` for a simple action node).

### 1 — Create the file

`aerini-engine/src/nodes/your_node.rs`

Implement the `Node` trait:

```rust
use async_trait::async_trait;
use serde_json::{json, Value};
use crate::model::{NodeInput, NodeOutput};
use crate::node::{Node, NodeType};

pub struct YourNode;

#[async_trait]
impl Node for YourNode {
    fn type_id(&self)        -> &'static str { "your_node" }
    fn display_name(&self)   -> &'static str { "Your Node" }
    fn node_type(&self)      -> NodeType     { NodeType::Action }
    fn version(&self)        -> &'static str { "1.0" }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "param_name": { "type": "string", "title": "Label shown in UI" }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "result": { "type": "string" }
            }
        })
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let param = input.params.get("param_name")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        NodeOutput::success(json!({ "result": param }))
    }
}
```

### 2 — Register it

In `aerini-engine/src/nodes/mod.rs`, add to `register_builtins()`:

```rust
pub mod your_node;
// ...
registry.register(Arc::new(your_node::YourNode));
```

### 3 — Add the icon

In `src/utils.ts`, add an entry to `NODE_ICONS`:

```typescript
your_node: "🔧",   // single emoji or SVG string
```

### 4 — Done

The node appears in the canvas palette automatically. No other files require changes.

---

## Node authoring rules

- **No `.unwrap()` in production paths.** Use `?`, `if let`, or `.expect("reason")`.
- **No noise comments.** Comment only non-obvious logic — never restate what the code does.
- **Implicit returns.** No `return` at end of function body.
- **Error propagation via `?`**, not manual `match`, where semantics are equivalent.
- **HTTP clients:** use the crate-level `OnceLock<reqwest::Client>` pattern from existing
  integration nodes (see `slack.rs`, `github.rs`). Do not create a new client per request.
- **Secrets:** receive credential IDs, not values. Resolve via `input.resolve_credential()`.
  Never log or expose raw credential values.
- **SSRF:** if your node makes outbound HTTP requests, wrap with `check_ssrf()` from `http.rs`
  or implement equivalent protection.

---

## Protected zones — do not touch

These interfaces are frozen. Any change breaks the IPC contract, serde wire format, or
`generate_handler!` registry and will silently fail at runtime.

**Tauri IPC commands** — all entries in `generate_handler!` in `src-tauri/src/lib.rs`.
Adding is fine. Renaming or removing breaks the TypeScript frontend.

**Serde wire contracts** — field names on these types are JSON API:
`Workflow`, `WorkflowNode`, `WorkflowEdge`, `NodeInput`, `NodeOutput`, `WorkflowResult`,
`NodeDescriptor`, `ScheduledJobRow`, `SchedulerStatusEvent`, `SchedulerError`, `PortConflict`,
`CredentialEntry`, `RunRecord`, `VersionRow`, `TriggerKind`.

**`async_trait` impls** — all 39 `impl Node for X`, both `CredentialResolver` impls,
both `SchedulerDb` impls, all `EventSink` impls. Zero direct call sites is intentional
(dyn dispatch). Do not remove any impl even if nothing visibly calls it.

**`#[allow(dead_code)]` items** — these are author-intentional. Never remove the annotation
or the item without confirming the intent with a maintainer.

**`subtle::ConstantTimeEq` usage** — `webhook.rs`, `scheduler/mod.rs`, `status_server.rs`.
Do not replace with `==`. Timing-safe comparison is required for secret validation.

**`#[cfg(unix)]` in `store.rs`** — sets `chmod 600` on the key file. Do not remove.

---

## Code conventions

### Rust
- No `.unwrap()` in production paths — `?`, `expect("descriptive reason")`, or `if let`
- Implicit returns (no trailing `return`)
- `async_trait` stays — MSRV not confirmed for native async traits
- `spawn_blocking` in Tauri commands stays — SQLite is sync, pattern is correct
- `thiserror` for all public error types — do not add `anyhow` errors to public APIs

### TypeScript
- No `any` where a type can be inferred or declared
- camelCase functions/vars, PascalCase classes/types/interfaces
- Magic strings used in 3+ files belong in `src/utils.ts`
- Event listeners must be paired with cleanup (`removeEventListener` or `{ once: true }`)

### CSS
- CSS custom properties over repeated literal values
- Backward-compat aliases (`--blue`, `--green`, etc.) in `main.css` must stay — used in TS

---

## Testing

`aerini-engine` has a unit test suite for `expression.rs` (30+ tests). Run with:

```bash
cargo test -p aerini-engine
```

For new nodes, add at least one `#[cfg(test)]` block covering:
- Normal execution path
- Missing/null input handling
- Any credential resolution path (mock the resolver)

There is no end-to-end test harness yet. Manual verification steps go in your PR description.

---

## Opening a pull request

1. Fork, branch from `main`, keep the branch focused on one change.
2. Run `cargo clippy -p aerini-engine -p aerini-server -- -D warnings` and fix all warnings before opening.
3. Run `cargo test -p aerini-engine` — all tests must pass.
4. If you touch any IPC command or serde type — call it out explicitly in the PR description.
5. If you add a node — include a short description of what it does and what credentials it needs.
6. Keep the PR title in the form `[node] Add YourNode` / `[fix] Description` / `[refactor] Scope`.

---

## Data directory (local dev)

All user data is written to the OS application data directory:

| Platform | Path |
|----------|------|
| macOS | `~/Library/Application Support/com.aerini.app/` |
| Windows | `%APPDATA%\com.aerini.app\` |
| Linux | `~/.local/share/com.aerini.app/` |

Delete this directory to reset all workflows, credentials, and run history during development.

---

## Building from Source

### Requirements

| Tool | Version | Install |
|------|---------|---------|
| Rust | 1.77+ | [rustup.rs](https://rustup.rs/) |
| Node.js | 18+ | [nodejs.org](https://nodejs.org/) — required to build the frontend (`npm`/Vite) only |
| Tauri CLI | 2.x | `cargo install tauri-cli --version "^2" --locked` |

### Steps

```bash
git clone https://github.com/Panchak2d/aerini
cd aerini
npm install
./scripts/fetch-node-binaries.sh
npm run dev
```

`fetch-node-binaries.sh` downloads and checksum-verifies the Node.js runtime that gets bundled into Aerini for the Code (JS) node, staging it under `src-tauri/binaries/`. One-time per clone; the build fails without it.

`npm run dev` compiles the Rust code on the first run, which takes 2–5 minutes. After that, the Aerini window opens.

### Building an installer

```bash
npm run build
```

The installer appears in `src-tauri/target/release/bundle/`:

| Platform | File | How to install |
|---|---|---|
| macOS | `.dmg` in `macos/` | Open it, drag Aerini to Applications |
| Windows | `.exe` or `.msi` in `msi/` | Run the installer |
| Linux | `.AppImage` in `appimage/` | `chmod +x Aerini*.AppImage` then run it |

### Server binary only (no desktop)

To build `aerini-server` without the Tauri/desktop toolchain:

```bash
cargo build --release -p aerini-server
```

For a static Linux binary (recommended for server deployments):

```bash
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl -p aerini-server
```

The Docker image (`Dockerfile`) stages its own bundled Node.js at build time — no extra step. A locally-built `aerini-server` binary run outside Docker does **not** get one staged automatically; the Code (JS) node will fail with "Bundled Node.js runtime is missing or corrupt" until you either copy a `node-bundled` binary next to the built executable or set `AERINI_NODE_BIN` to a Node.js path.

---

## Troubleshooting

### AppImage build fails: `failed to run linuxdeploy`

Only affects `npm run build` on Linux (the Tauri step that produces an `.AppImage`). `npm run dev` and `npm run vite:build` are unaffected.

**Cause:** Missing `libfuse2` — AppImages require FUSE to mount themselves, and Ubuntu 22.04+ / Fedora / Arch do not ship it by default.

**Fix:**

```bash
# Ubuntu 22.04+
sudo apt install libfuse2t64

# Ubuntu 20.04 / older distros
sudo apt install libfuse2
```

Retry `npm run build` after installing. If the error persists, the full output just above the `failed to run linuxdeploy` line will contain a more specific message.
