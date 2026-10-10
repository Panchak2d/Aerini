# Contributing to Aerini

Aerini is a Tauri 2 desktop app for building visual automation workflows.
The codebase is split into three Cargo crates and a TypeScript frontend.

---

## Contributor License Agreement

Before your first pull request can be merged, you must sign the
[Contributor License Agreement](CLA.md).

**Individual contributors:** This project uses [CLA Assistant](https://cla-assistant.io). On your first PR it posts a comment with a one-click sign link: GitHub OAuth, done in seconds.

**Corporate contributors (contributing on behalf of your employer):**
Your company must sign the Corporate CLA before any code from your employees
can be merged. See [CLA.md](CLA.md) Section C8 for instructions. See
[CONTACT.md](CONTACT.md) for the contact address before opening a PR.

---

## Architecture

```
aerini-engine/     core library: Node trait, executor, scheduler, DB, 40 nodes
src-tauri/        Tauri shell: IPC commands, tray, app lifecycle
aerini-server/     headless binary for server-side workflow execution
src/              TypeScript/Vite frontend (vanilla TS, no framework)
```

`aerini-engine` has zero Tauri dependency. Both `src-tauri` and `aerini-server` depend on it.
The decoupling point is the `EventSink` trait. The Tauri app and the server binary implement
it differently, but the executor and scheduler use only the trait.

---

## Dev environment

| Tool | Version |
|------|---------|
| Rust (stable) | 1.96+ |
| Node.js | 20.19+, 22.13+, or 24+ (only to run `npm`/Vite, not needed to use the Code (JS) node, see below) |
| Tauri CLI | 2.x (`cargo install tauri-cli --version "^2"`) |
| Linux system packages | `libwebkit2gtk-4.1-dev`, `libappindicator3-dev`, `librsvg2-dev`, `patchelf`, `xdg-utils` (only needed to build or test the `aerini` desktop crate on Linux; not needed for `aerini-engine` or `aerini-server` alone) |

The Code (JS) node runs on a Node.js runtime bundled into Aerini, not your system install. Before the first `npm run dev`, stage it:

```bash
npm install
./scripts/fetch-node-binaries.sh   # downloads + checksum-verifies bundled Node.js, one time per clone
npm run dev       # Vite dev server + Tauri window
npm run build:unsigned   # installers → src-tauri/target/release/bundle/
```

`npm run build` is the release form: it also signs the update artifacts and fails without the maintainers' `TAURI_SIGNING_PRIVATE_KEY`. Use `build:unsigned` for local builds.

`fetch-node-binaries.sh` needs `curl`, `tar`, `unzip` (or `powershell.exe`), and `sha256sum`/`shasum` on PATH, plus network access to nodejs.org. Skipping it fails the Rust build with `resource path 'binaries/node-bundled-...' doesn't exist`. `strip` is optional: when present, the script uses it to remove the ~14% of each Linux/macOS binary that's just an embedded debug symbol table (Windows builds don't have one to remove); its absence is not an error, the binary is staged unstripped instead.

`npm run vite:dev` serves the frontend on its own at `http://localhost:1420`, with no Rust backend behind it, so you can work on the UI in an ordinary browser without the Tauri window ("browser mode"). The port is fixed, so stop `npm run dev` first if it's running. The workflow editor works, and workflows save to that browser's `localStorage`, separate from the desktop app's database. Anything that needs the backend doesn't: Run shows a "requires the desktop app" notice, and the Monitor panel, performance monitoring, and the memory chip are disabled and say they need the desktop app.

> ⚠️ **`dist/` is intentionally committed: do not delete it or add it to `.gitignore`.**
>
> Tauri reads compiled frontend assets from `dist/` at build time. Without it, `cargo tauri build` fails.
> Only sourcemaps (`dist/**/*.map`) are gitignored; `dist/` itself is not.
>
> When contributing: if you changed the frontend, run `npm run vite:build` and commit the rebuilt `dist/`
> in the same pull request. CI rebuilds the bundle on every pull request and fails if `dist/` differs from
> what is committed. Use `vite:build`, not `build`: `npm run build` runs the full Tauri build.

---

## Adding a new node

All 40 built-in nodes follow the same pattern. Copy any existing node file as a starting point
(e.g. `aerini-engine/src/nodes/delay.rs` for a simple action node).

### 1. Create the file

`aerini-engine/src/nodes/your_node.rs`

Implement the `Node` trait:

```rust
use async_trait::async_trait;
use serde_json::{json, Value};
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::Node;

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
        let param = input.input.get("param_name")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        NodeOutput::success(json!({ "result": param }))
    }
}
```

### 2. Register it

In `aerini-engine/src/nodes/mod.rs`, add to `register_builtins()`:

```rust
pub mod your_node;
// ...
registry.register(Arc::new(your_node::YourNode));
```

### 3. Add the icon

In `src/utils.ts`, add an entry to `NODE_ICONS`:

```typescript
your_node: "🔧",   // single emoji or SVG string
```

### 4. Done

The node appears in the canvas palette automatically. No other files require changes.

---

## Node authoring rules

- **No `.unwrap()` in production paths.** Use `?`, `if let`, or `.expect("reason")`.
- **No noise comments.** Comment only non-obvious logic; never restate what the code does.
- **Implicit returns.** No `return` at end of function body.
- **Error propagation via `?`**, not manual `match`, where semantics are equivalent.
- **HTTP clients:** use the crate-level `OnceLock<reqwest::Client>` pattern from existing
  integration nodes (see `slack.rs`, `github.rs`). Do not create a new client per request.
- **Secrets:** the executor resolves any credential-backed config field before `execute()`
  runs; the plaintext value is already in `input.input` like any other field, there is no
  `resolve_credential()` method to call. Never log or echo that value back into `logs` or
  an error message.
- **SSRF:** `check_ssrf()` in `http.rs` is private to that module. For a node that requests
  a user-supplied host, build the client with `nodes::util::guarded_client_builder()` (it
  filters addresses at connect time) and call `nodes::util::check_host_ssrf_from_url()` first
  for a clear error. Classify a failed `send()` with `util::http_err_output(replay, &e)` or
  `util::is_retryable_network_error(replay, &e)`; pass `Replay::Never` for a call that sends or
  creates something, `Replay::Safe` for a read.

---

## Protected zones: do not touch

These interfaces are frozen. Any change breaks the IPC contract, serde wire format, or
`generate_handler!` registry and will silently fail at runtime.

**Tauri IPC commands:** all entries in `generate_handler!` in `src-tauri/src/lib.rs`.
Adding is fine. Renaming or removing breaks the TypeScript frontend.

**Serde wire contracts.** Field names on these types are JSON API:
`Workflow`, `WorkflowNode`, `WorkflowEdge`, `NodeInput`, `NodeOutput`, `WorkflowResult`,
`NodeDescriptor`, `ScheduledJobRow`, `SchedulerStatusEvent`, `SchedulerError`, `PortConflict`,
`CredentialEntry`, `RunRecord`, `VersionRow`, `TriggerKind`.

**`async_trait` impls:** all 40 `impl Node for X`, both `CredentialResolver` impls,
both `SchedulerDb` impls, all `EventSink` impls. Zero direct call sites is intentional
(dyn dispatch). Do not remove any impl even if nothing visibly calls it.

**`#[allow(dead_code)]` items.** These are author-intentional. Never remove the annotation
or the item without confirming the intent with a maintainer.

**`subtle::ConstantTimeEq` usage:** `webhook.rs`, `scheduler/mod.rs`, `status_server.rs`.
Do not replace with `==`. Timing-safe comparison is required for secret validation.

**`#[cfg(unix)]` in `store.rs`.** Sets `chmod 600` on the key file. Do not remove.

---

## Code conventions

### Rust
- No `.unwrap()` in production paths: use `?`, `expect("descriptive reason")`, or `if let`
- Implicit returns (no trailing `return`)
- `async_trait` stays (MSRV not confirmed for native async traits)
- `spawn_blocking` in Tauri commands stays (SQLite is sync; the pattern is correct)
- `thiserror` for all public error types: do not add `anyhow` errors to public APIs

### TypeScript
- No `any` where a type can be inferred or declared
- camelCase functions/vars, PascalCase classes/types/interfaces
- Magic strings used in 3+ files belong in `src/utils.ts`
- Event listeners must be paired with cleanup (`removeEventListener` or `{ once: true }`)

### CSS
- CSS custom properties over repeated literal values
- Backward-compat aliases (`--blue`, `--green`, etc.) in `main.css` must stay; they're used in TS

---

## Testing

`aerini-engine`'s expression engine lives under `expression/` (`mod.rs`, `parser.rs`, `functions.rs`, `resolver.rs`), not a single `expression.rs` file. All of its tests, 52 in total, live in `resolver.rs`. Run with:

```bash
cargo test -p aerini-engine
```

For new nodes, add at least one `#[cfg(test)]` block covering:
- Normal execution path
- Missing/null input handling
- Any credential-backed field, using a `NodeInput` with the field already populated
  (there is no resolver to mock, see the Secrets rule above)

`aerini-engine/tests/workflow_integration.rs` is an end-to-end harness: it builds a `Workflow` from scratch and runs it through the real executor, covering a full trigger-to-output chain, both branches of an If node, loop iteration counts, cycle detection, and a disconnected node. Manual verification steps for anything it doesn't cover still go in your PR description.

The release scripts have their own tests: `python3 -m unittest discover -s scripts/tests`. Run them if you change anything under `scripts/`.

See [Testing](docs/development/testing.md) for how all three Rust crates and the frontend suite fit together, and which of them CI runs for you automatically versus which need a manual `cargo test -p <crate>`.

---

## Opening a pull request

1. Fork, branch from `main`, keep the branch focused on one change.
2. Run `cargo clippy -p aerini-engine -p aerini-server -- -D warnings` and fix all warnings before opening.
3. Run `cargo test -p aerini-engine` and `cargo test -p aerini-server`. All tests must pass.
4. If you changed the frontend, run `npm run typecheck` and `npm test`, then `npm run vite:build` and commit the rebuilt `dist/`. CI fails on a stale one.
5. If you touch any IPC command or serde type, call it out explicitly in the PR description.
6. If you add a node, include a short description of what it does and what credentials it needs.
7. Keep the PR title in the form `[node] Add YourNode` / `[fix] Description` / `[refactor] Scope`.

---

## Data directory (local dev)

All user data is written to the OS application data directory:

| Platform | Path |
|----------|------|
| macOS | `~/Library/Application Support/org.aerini.desktop/` |
| Windows | `%APPDATA%\org.aerini.desktop\` |
| Linux | `~/.local/share/org.aerini.desktop/` |

Delete this directory to reset all workflows, credentials, and run history during development.

---

## Building from Source

### Requirements

| Tool | Version | Install |
|------|---------|---------|
| Rust | 1.96+ | [rustup.rs](https://rustup.rs/) |
| Node.js | 20.19+, 22.13+, or 24+ | [nodejs.org](https://nodejs.org/), required to build the frontend (`npm`/Vite) only |
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
npm run build:unsigned
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

The Docker image (`Dockerfile`) stages its own bundled Node.js at build time. No extra step is required. A locally-built `aerini-server` binary run outside Docker does **not** get one staged automatically; the Code (JS) node will fail with "Bundled Node.js runtime is missing or corrupt" until you either copy a `node-bundled` binary next to the built executable or set `AERINI_NODE_BIN` to a Node.js path.

---

## Releasing (maintainers)

Desktop releases are built by `release.yml` when a `vX.Y.Z` tag is pushed. The update packages are signed with a private key that only the build jobs can read.

### One-time setup

1. Generate the key pair with a password, outside the repository: `npx tauri signer generate -w <path>`. Put the **public** key in `plugins.updater.pubkey` in `src-tauri/tauri.conf.json`. Back up the private key and its password in two separate places. If the key is lost, installed copies can never update again and users must reinstall by hand. If it leaks, anyone holding it can push malicious updates.
2. In GitHub, create an environment named `release` (Settings → Environments). Require a reviewer, restrict deployments to tags matching `v*`, and add the secrets `TAURI_SIGNING_PRIVATE_KEY` (the key file's contents) and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
3. Add a tag ruleset for `v*` (Settings → Rules → Rulesets) that blocks deleting or moving release tags.

### Cutting a release

1. Set the new version in `package.json`, `src-tauri/tauri.conf.json` and the root `Cargo.toml` (`bash scripts/verify-release-tag.sh vX.Y.Z` checks all three), update `CHANGELOG.md`, and push the tag.
2. Approve the pending `release` deployment. The `verify` job has already checked the updater configuration; the build jobs sign each installer.
3. When the run finishes, open the summary of the **Publish updater manifest** job. It lists every platform in `latest.json`. Expect eight: AppImage, deb and rpm for x86_64 and aarch64, plus NSIS and MSI for Windows x86_64. macOS is built and signed but deliberately not listed, so macOS users update by downloading the release.
4. Edit the draft's notes and publish it. Publishing is what releases the update: only a published release that is not a prerelease is served as "latest", so drafts and `-rc` tags are never offered. If the manifest job failed, do not publish. Fix the cause and re-run the failed jobs; re-running is safe.
5. Smoke test: install the previous version and update to the new one through **Settings → About → Updates**.

### Rules

- **Never change `pubkey` directly.** Installed apps only trust the key compiled into them. To rotate: release N with the *new* `pubkey` in its config but signed with the *old* key; once users are on N, switch the two secrets and release N+1 signed with the new key.
- Do not remove `bundle.createUpdaterArtifacts` or `plugins.updater.requireSignedVersion`. The `verify` job fails the release if either is gone, because a build without them can never self-update, or can be forced to an older version.
- To list another platform in `latest.json`, add its key to `EXPECTED_KEYS` in `scripts/build-updater-manifest.py`. To enable macOS in-app updates, move `darwin-aarch64-app` from `HELD_BACK_KEYS` to `EXPECTED_KEYS`, but only after the macOS update path has been tested on a Mac.
- The `verify` job requires the first `plugins.updater` endpoint to point at this repository's own `releases/latest/download/latest.json`. A fork that wants to test releases must set its own public key and endpoint in `tauri.conf.json`, its own `release` environment and its own test key.

---

## Troubleshooting

### AppImage build fails: `failed to run linuxdeploy`

Only affects `npm run build:unsigned` (and `npm run build`) on Linux (the Tauri step that produces an `.AppImage`). `npm run dev` and `npm run vite:build` are unaffected.

**Cause:** Missing `libfuse2`. AppImages require FUSE to mount themselves, and Ubuntu 22.04+ / Fedora / Arch do not ship it by default.

**Fix:**

```bash
# Ubuntu 22.04+
sudo apt install libfuse2t64

# Ubuntu 20.04 / older distros
sudo apt install libfuse2
```

Retry `npm run build:unsigned` after installing. If the error persists, the full output just above the `failed to run linuxdeploy` line will contain a more specific message.
