# Contributing to Flowo

Flowo is a Tauri 2.0 desktop app for building visual automation workflows.
The codebase is split into three Cargo crates and a TypeScript frontend.

---

## Architecture

```
flowo-engine/     — core library: Node trait, executor, scheduler, DB, 34 nodes
src-tauri/        — Tauri shell: IPC commands, tray, app lifecycle
flowo-server/     — headless binary for server-side workflow execution
src/              — TypeScript/Vite frontend (vanilla TS, no framework)
```

`flowo-engine` has zero Tauri dependency. Both `src-tauri` and `flowo-server` depend on it.
The decoupling point is the `EventSink` trait — the Tauri app and the server binary implement
it differently, but the executor and scheduler use only the trait.

---

## Dev environment

| Tool | Version |
|------|---------|
| Rust (stable) | 1.77+ |
| Node.js | 18+ |
| Tauri CLI | 2.x (`cargo install tauri-cli --version "^2"`) |
| Node.js on PATH | Required for the Code (JS) node at runtime |

```bash
npm install
npm run dev       # Vite dev server + Tauri window
npm run build     # production bundle → src-tauri/target/release/bundle/
```

---

## Adding a new node

All 34 built-in nodes follow the same pattern. Copy any existing node file as a starting point
(e.g. `flowo-engine/src/nodes/delay.rs` for a simple action node).

### 1 — Create the file

`flowo-engine/src/nodes/your_node.rs`

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

In `flowo-engine/src/nodes/mod.rs`, add to `register_builtins()`:

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

**`async_trait` impls** — all 34 `impl Node for X`, both `CredentialResolver` impls,
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

`flowo-engine` has a unit test suite for `expression.rs` (30+ tests). Run with:

```bash
cargo test -p flowo-engine
```

For new nodes, add at least one `#[cfg(test)]` block covering:
- Normal execution path
- Missing/null input handling
- Any credential resolution path (mock the resolver)

There is no end-to-end test harness yet. Manual verification steps go in your PR description.

---

## Opening a pull request

1. Fork, branch from `main`, keep the branch focused on one change.
2. Run `cargo clippy -p flowo-engine -- -D warnings` and fix all warnings before opening.
3. Run `cargo test -p flowo-engine` — all tests must pass.
4. If you touch any IPC command or serde type — call it out explicitly in the PR description.
5. If you add a node — include a short description of what it does and what credentials it needs.
6. Keep the PR title in the form `[node] Add YourNode` / `[fix] Description` / `[refactor] Scope`.

---

## Data directory (local dev)

All user data is written to the OS application data directory:

| Platform | Path |
|----------|------|
| macOS | `~/Library/Application Support/com.flowo.app/` |
| Windows | `%APPDATA%\com.flowo.app\` |
| Linux | `~/.local/share/com.flowo.app/` |

Delete this directory to reset all workflows, credentials, and run history during development.
