# Testing

Aerini has no single test command. Each of the three Rust crates carries its own tests, run with its own `cargo test -p` invocation, and the frontend has a separate suite run with `npm test`. This page is the map: where each suite lives, what it actually covers, and which of them CI actually runs on your behalf versus which you'd need to run yourself. It assumes you've read [Architecture](architecture.md) for the three-crate shape; this page doesn't re-explain that split, only where the tests inside each piece sit.

`CONTRIBUTING.md` has its own shorter "Testing" section. It covers `aerini-engine`'s expression tests and the `cargo test -p aerini-engine` command, but doesn't mention the frontend suite, `aerini-server`, or the desktop shell's own tests at all, and its claim that there's no end-to-end test harness is out of date. The differences are covered below as they come up.

## Where everything lives

| Crate / area | Package name | Test location | Command |
|---|---|---|---|
| Engine | `aerini-engine` | inline `#[cfg(test)]` modules, plus `aerini-engine/tests/` | `cargo test -p aerini-engine` |
| Server | `aerini-server` | inline `#[cfg(test)]` modules only | `cargo test -p aerini-server` |
| Desktop shell | `aerini` | inline `#[cfg(test)]` modules only | `cargo test -p aerini` |
| Frontend | (n/a, `package.json`) | `src/__tests__/*.test.ts` | `npm test` |

The desktop shell's package name is worth calling out on its own: the crate that lives under `src-tauri/` is named `aerini` in its own `Cargo.toml`, not `src-tauri`. `cargo test -p src-tauri` fails with "package not found"; `cargo test -p aerini` is the one that actually runs its tests.

`cargo test --workspace`, run from the repo root, runs all three Rust crates' tests in one pass. On Linux this requires the same GUI system libraries the desktop app itself needs to compile at all (`libwebkit2gtk-4.1-dev`, `libappindicator3-dev`, `librsvg2-dev`, `patchelf`, `xdg-utils`), since `aerini`'s crate pulls in `tauri` directly. Without them, `--workspace` fails at the `aerini` crate's build step before any test runs, for a reason that has nothing to do with the tests themselves. `CONTRIBUTING.md`'s dev environment table doesn't currently list these packages.

## The engine (`aerini-engine`)

61 files carry their own `#[cfg(test)]` module. The largest concentration is the 39 files implementing the `Node` trait, and the split there, 26 with their own test module against 13 relying on integration coverage instead, is [Adding a Built-in Node](node-authoring.md#tests)'s own territory; that page names which 13 and why, and isn't repeated here.

Outside the node files, `expression/resolver.rs` alone carries 52 tests, the largest single-file suite in the crate. `CONTRIBUTING.md` refers to this as "a unit test suite for `expression.rs`", but there is no file by that name; the expression engine is a module directory (`expression/mod.rs`, `parser.rs`, `functions.rs`, `resolver.rs`), and every one of its tests lives in `resolver.rs` specifically, not spread across the other three files.

`plugin_loader.rs` has two separate `#[cfg(test)]` modules. One covers `engine_input_to_wit`'s config-flattening logic in isolation. The other covers `PluginLoader` itself: load/describe error classification, the compile-result cache, and signature verification, all against a hand-written minimal WebAssembly component (`(component)`, the smallest valid one) built inline with `tempfile`, never a real `.wasm` file. The module's own comment on one of these tests says as much directly: there's no fixture in this crate that exercises a component actually exporting the `aerini-node` interface. `examples/plugin-template/` (the echo and trigger-example plugins) has no tests of its own either. Between the two, there's currently no test anywhere in the repository that compiles and loads a real, interface-exporting plugin the way a running Aerini install would; the sandbox and interface-detection logic are covered, but only against synthetic, empty components.

Two integration-test files live under `aerini-engine/tests/`, separate from the inline `#[cfg(test)]` modules and run by the same `cargo test -p aerini-engine` command:

- `workflow_integration.rs`, 6 tests, each building a small `Workflow` by hand and running it through the real `WorkflowExecutor`: a full trigger-to-transform-to-output chain, both branches of an If node, loop iteration counts, cycle detection, and a disconnected node. No network, database, or filesystem access; every node used is pure in-process.
- `schema_validation.rs`, 4 tests, a sweep across every registered built-in node asserting that `input_schema()` and `output_schema()` each return either nothing or a well-formed JSON Schema object, that `required` arrays only contain strings named in `properties`, that no two nodes share a `type_id`, and that every node has a non-empty display name and version.

Neither file shares helpers with the other or with any `#[cfg(test)]` module elsewhere. `workflow_integration.rs` builds its own `make_registry`, `make_executor`, `node`, and `edge` functions at the top of the file; `schema_validation.rs` does the same with its own `make_registry`. There's no `test_helpers.rs` or `fixtures/` directory anywhere in the crate, or in `aerini-server` or `aerini`.

## The server (`aerini-server`)

Seven files carry a `#[cfg(test)]` module: `api_server/mod.rs`, `api_server/routes/memory.rs`, `api_server/routes/performance.rs`, `api_server/routes/workflows.rs`, `env_credentials.rs`, `status_server.rs`, and `token_store.rs`. No separate `tests/` directory exists for this crate. The split between plain `#[test]` and `#[tokio::test]` runs by file, not by category: `mod.rs`, `workflows.rs`, and `token_store.rs` use plain `#[test]` throughout; `memory.rs`, `performance.rs`, `env_credentials.rs`, and `status_server.rs` use `#[tokio::test]` throughout. No file in this crate mixes the two; which one a given file uses tracks whether the functions it calls are themselves async, not what kind of file it is.

## The desktop shell (`aerini`, under `src-tauri/`)

Three files carry a `#[cfg(test)]` module: `commands/export/templates.rs`, `commands/plugins.rs`, and `lib.rs`. `commands/plugins.rs` has the largest share, covering plugin-install collision handling and signature-status reporting with both sync and async (`#[tokio::test]`) cases. As with the other two crates, there's no separate `tests/` directory here.

## The frontend (`src/`)

66 files, every one of them under `src/__tests__/`, every one named `*.test.ts`. There's no `.spec.ts` file anywhere and no test file outside that directory. `vitest.config.ts` points its `include` pattern at exactly `src/__tests__/**/*.test.ts`, which is the actual enforcement behind that convention, not just a habit.

Run with `npm test` (`vitest run` under the hood, per `package.json`'s `scripts`), or `npm run test:watch` for watch mode. The default test environment is `node`; 59 of the 66 files opt into `jsdom` instead with a `// @vitest-environment jsdom` (or block-comment equivalent) docblock at the top of the file, for anything touching the DOM. `vitest.config.ts` also carries a Node-version-gated `execArgv` entry: Node 25 and later ships its own global `localStorage`, which shadows jsdom's copy in any file using the `jsdom` environment, so the config passes `--no-experimental-webstorage` on Node 25+ to keep jsdom's `localStorage` the one in effect.

Same pattern as the Rust side: no shared fixtures or mock module. Every file that needs `@tauri-apps/api/core` mocked calls `vi.mock` at its own top, and object factories like `workflow-manager.test.ts`'s `makeNode` are written locally in the file that needs them, not imported from anywhere shared.

## Continuous integration

`ci.yml` runs on every push and pull request against `main`, as three separate jobs:

- **`test`**: builds `aerini-engine` and `aerini-server` (not `aerini`, the desktop crate), then runs `cargo test -p aerini-engine` and `cargo clippy -p aerini-engine -p aerini-server -- -D warnings`. `aerini-server`'s own test suite is built but not run here.
- **`node-checks`**: `npm audit`, then the full frontend suite via `npm test`, then a check that a committed `dist/` rebuild produces no diff.
- **`tauri-build`**: builds the desktop app across four platform targets (`ubuntu-22.04`, `ubuntu-22.04-arm`, `windows-latest`, `macos-latest`) via `npm run build`. This job compiles the `aerini` crate but never runs `cargo test` against it.

Put together, no job in this workflow runs `aerini-server`'s or `aerini`'s own `#[cfg(test)]` suites, 27 and 50 tests respectively, unless a contributor runs `cargo test -p aerini-server` or `cargo test -p aerini` (or `cargo test --workspace`) themselves. This matches `CONTRIBUTING.md`'s own "Opening a pull request" checklist, which also only asks for `cargo test -p aerini-engine` before a PR. A change confined to `aerini-engine` or the frontend gets full CI coverage; a change to `aerini-server` or the desktop shell's own command handlers does not, today, without that extra manual step.

`security-audit.yml` is a separate concern from any of the above: it runs `cargo audit` against every crate's dependency tree on a daily schedule and whenever a `Cargo.toml` or `Cargo.lock` changes, checking for known vulnerability advisories, not application behavior. `release.yml` runs on a version tag push and builds and publishes installers; it has no test step of its own; a tag is expected to already be off a commit that passed `ci.yml`.

## What this page doesn't cover

[Adding a Built-in Node](node-authoring.md#tests) covers what a new node's own `#[cfg(test)]` block should contain. [Writing a Plugin Node](plugin-authoring.md) covers building and signing a `.wasm` plugin; it doesn't currently discuss testing one, and as noted above, there's no working example of that in this repository yet. [Architecture](architecture.md) is where the three-crate split itself is explained; this page only says where the tests inside that split live.
