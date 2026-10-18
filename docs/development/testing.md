# Testing

Aerini has no single test command. Each of the three Rust crates carries its own tests, run with its own `cargo test -p` invocation, and the frontend has a separate suite run with `npm test`. This page is the map: where each suite lives, what it actually covers, and which of them CI actually runs on your behalf versus which you'd need to run yourself. It assumes you've read [Architecture](architecture.md) for the three-crate shape; this page doesn't re-explain that split, only where the tests inside each piece sit.

`CONTRIBUTING.md` has its own shorter "Testing" section. It covers `aerini-engine`'s expression tests and the `cargo test -p aerini-engine` command, but doesn't mention the frontend suite, `aerini-server`, or the desktop shell's own tests at all, and its claim that there's no end-to-end test harness is out of date. The differences are covered below as they come up.

## Where everything lives

| Crate / area | Package name | Test location | Command |
|---|---|---|---|
| Engine | `aerini-engine` | inline `#[cfg(test)]` modules, plus `aerini-engine/tests/` | `cargo test -p aerini-engine` |
| Server | `aerini-server` | inline `#[cfg(test)]` modules only | `cargo test -p aerini-server` |
| Desktop shell | `aerini` | inline `#[cfg(test)]` modules only | `cargo test -p aerini` |
| MCP server | `aerini-mcp` | inline `#[cfg(test)]` modules, plus `aerini-mcp/tests/` | `cargo test -p aerini-mcp` (and `--features offline` for the offline-mode tests) |
| Frontend | (n/a, `package.json`) | `src/__tests__/*.test.ts` | `npm test` |

The desktop shell's package name is worth calling out on its own: the crate that lives under `src-tauri/` is named `aerini` in its own `Cargo.toml`, not `src-tauri`. `cargo test -p src-tauri` fails with "package not found"; `cargo test -p aerini` is the one that actually runs its tests.

`cargo test --workspace`, run from the repo root, runs all three Rust crates' tests in one pass. On Linux this requires the same GUI system libraries the desktop app itself needs to compile at all (`libwebkit2gtk-4.1-dev`, `libappindicator3-dev`, `librsvg2-dev`, `patchelf`, `xdg-utils`), since `aerini`'s crate pulls in `tauri` directly. Without them, `--workspace` fails at the `aerini` crate's build step before any test runs, for a reason that has nothing to do with the tests themselves. `CONTRIBUTING.md`'s dev environment table doesn't currently list these packages.

## The engine (`aerini-engine`)

61 files carry their own `#[cfg(test)]` module. The largest concentration is the 39 files implementing the `Node` trait, and the split there, 26 with their own test module against 13 relying on integration coverage instead, is [Adding a Built-in Node](node-authoring.md#tests)'s own territory; that page names which 13 and why, and isn't repeated here.

Outside the node files, `expression/resolver.rs` alone carries 52 tests, the largest single-file suite in the crate. `CONTRIBUTING.md` refers to this as "a unit test suite for `expression.rs`", but there is no file by that name; the expression engine is a module directory (`expression/mod.rs`, `parser.rs`, `functions.rs`, `resolver.rs`), and every one of its tests lives in `resolver.rs` specifically, not spread across the other three files.

`plugin_loader.rs` has two separate `#[cfg(test)]` modules. One covers `engine_input_to_wit`'s config-flattening logic in isolation. The other covers `PluginLoader` itself: load/describe error classification, the compile-result cache, and signature verification, all against a hand-written minimal WebAssembly component (`(component)`, the smallest valid one) built inline with `tempfile`, never a real `.wasm` file. The module's own comment on one of these tests says as much directly: there's no fixture in this crate that exercises a component actually exporting the `aerini-node` interface. `examples/plugin-template/` (the echo and trigger-example plugins) has no tests of its own either. Between the two, there's currently no test anywhere in the repository that compiles and loads a real, interface-exporting plugin the way a running Aerini install would; the sandbox and interface-detection logic are covered, but only against synthetic, empty components.

Four integration-test files live under `aerini-engine/tests/`, separate from the inline `#[cfg(test)]` modules. Three run with `cargo test -p aerini-engine`; `workflow_schema.rs` is compiled only with the `schema` feature, so it needs `cargo test -p aerini-engine --features schema`:

- `workflow_integration.rs`, 6 tests, each building a small `Workflow` by hand and running it through the real `WorkflowExecutor`: a full trigger-to-transform-to-output chain, both branches of an If node, loop iteration counts, cycle detection, and a disconnected node. No network, database, or filesystem access; every node used is pure in-process.
- `schema_validation.rs`, 4 tests, a sweep across every registered built-in node asserting that `input_schema()` and `output_schema()` each return either nothing or a well-formed JSON Schema object, that `required` arrays only contain strings named in `properties`, that no two nodes share a `type_id`, and that every node has a non-empty display name and version.
- `reference_docs.rs`, 3 tests, builds every built-in node descriptor (sorted by `type_id`) and compares `docs/reference/node-schemas.json` and `docs/reference/node-catalog.md` with what the code generates. The third test checks that the catalog has no `{{` or raw angle bracket outside a code span, either of which would break the docs build.
- `workflow_schema.rs`, 3 tests, compares `docs/reference/workflow.schema.json` with the schema generated from the workflow types in `model.rs`; checks that every workflow shipped under `examples/` passes that schema and parses with `Workflow::from_json`; and runs a fixed set of valid and invalid documents through both the schema and `Workflow::from_json`, requiring the same verdict from each. It reads the example `.zip` package with the `zip` crate, which is a dev-dependency.

`workflow_integration.rs` builds its own `make_registry`, `make_executor`, `node`, and `edge` functions at the top of the file; `schema_validation.rs` does the same with its own `make_registry`. `reference_docs.rs` and `workflow_schema.rs` share one helper, `tests/common/mod.rs`, which compares a generated file with its copy under `docs/reference/` or rewrites it. There's no `test_helpers.rs` or `fixtures/` directory anywhere in the crate, or in `aerini-server` or `aerini`.

### Regenerating the reference files

The three generated files under `docs/reference/` (`node-schemas.json`, `node-catalog.md`, `workflow.schema.json`) must match the code, so the tests above fail after any change to a node, a node's schema or description, a workflow type in `model.rs`, or the engine version. Regenerate them and commit the result:

```bash
AERINI_REGEN_DOCS=1 cargo test -p aerini-engine --features schema --test reference_docs --test workflow_schema
```

In PowerShell, run `$env:AERINI_REGEN_DOCS = 1` first and remove it afterwards with `Remove-Item Env:AERINI_REGEN_DOCS`. The comparison ignores line endings, so a Windows checkout does not cause a false failure.

## The server (`aerini-server`)

Seven files carry a `#[cfg(test)]` module: `api_server/mod.rs`, `api_server/routes/memory.rs`, `api_server/routes/performance.rs`, `api_server/routes/workflows.rs`, `env_credentials.rs`, `status_server.rs`, and `token_store.rs`. No separate `tests/` directory exists for this crate. The split between plain `#[test]` and `#[tokio::test]` runs by file, not by category: `mod.rs`, `workflows.rs`, and `token_store.rs` use plain `#[test]` throughout; `memory.rs`, `performance.rs`, `env_credentials.rs`, and `status_server.rs` use `#[tokio::test]` throughout. No file in this crate mixes the two; which one a given file uses tracks whether the functions it calls are themselves async, not what kind of file it is.

`scripts/smoke-test-docker.sh` is the end-to-end check of the Docker image. `docker-smoke.yml` and the `smoke` job of `release.yml` run it; you can also run it yourself. Run `./scripts/smoke-test-docker.sh [image-name]` from the repo root. It builds the image, confirms `node-bundled` starts on its own inside it, then runs the container in `api` mode with `--allow-code` on `127.0.0.1:7799`, waits up to 30 seconds for `GET /api/health`, saves a one-node Code workflow, runs it through the API, and checks that the result is `4`. It needs Docker, `curl`, and `python3` on the host, requires port 7799 to be free, and prints the container's logs if a step fails.

## The desktop shell (`aerini`, under `src-tauri/`)

Three files carry a `#[cfg(test)]` module: `commands/export/templates.rs`, `commands/plugins.rs`, and `lib.rs`. `commands/plugins.rs` has the largest share, covering plugin-install collision handling and signature-status reporting with both sync and async (`#[tokio::test]`) cases. As with the other two crates, there's no separate `tests/` directory here.

## The frontend (`src/`)

115 files, every one of them under `src/__tests__/`, every one named `*.test.ts`. There's no `.spec.ts` file anywhere and no test file outside that directory. `vitest.config.ts` points its `include` pattern at exactly `src/__tests__/**/*.test.ts`, which is the actual enforcement behind that convention, not just a habit.

Run with `npm test` (`vitest run` under the hood, per `package.json`'s `scripts`), or `npm run test:watch` for watch mode. The default test environment is `node`; 99 of the 115 files opt into `jsdom` instead with a `// @vitest-environment jsdom` (or block-comment equivalent) docblock at the top of the file, for anything touching the DOM. `vitest.config.ts` also carries a Node-version-gated `execArgv` entry: Node 25 and later ships its own global `localStorage`, which shadows jsdom's copy in any file using the `jsdom` environment, so the config passes `--no-experimental-webstorage` on Node 25+ to keep jsdom's `localStorage` the one in effect.

Same pattern as the Rust side: no shared fixtures or mock module. Every file that needs `@tauri-apps/api/core` mocked calls `vi.mock` at its own top, and object factories like `workflow-manager.test.ts`'s `makeNode` are written locally in the file that needs them, not imported from anywhere shared.

## Continuous integration

`ci.yml` runs on every push and pull request against `main`, as ten separate jobs:

- **`test`**: builds `aerini-engine` and `aerini-server` (not `aerini`, the desktop crate), then runs `cargo test -p aerini-engine --features schema`, `cargo test -p aerini-server`, and `cargo clippy -p aerini-engine -p aerini-server --features aerini-engine/schema -- -D warnings`. It then runs `cargo test -p aerini-mcp` and `cargo test -p aerini-mcp --features offline`, and clippy over `aerini-mcp` with and without the `offline` feature. The `aerini-mcp` tests use a local mock of the server API, and one of them starts the built `aerini-mcp` binary and talks to it over stdio.
- **`test-windows`**: on `windows-latest`, runs `cargo test -p aerini-engine -- nodes::shell nodes::notification --include-ignored --skip the_windows_toast_command_runs_to_completion`, the tests for Windows-specific engine code, including the ones that need a real Windows shell. The one test that shows a real toast needs a desktop session, so the job skips it; run it with `cargo test -p aerini-engine -- the_windows_toast_command_runs_to_completion --ignored` on a Windows desktop. The rest of the engine suite assumes a Unix host and runs only in `test`.
- **`release-scripts`**: runs the Python tests for the release scripts under `scripts/tests/` (`python3 -m unittest discover -s scripts/tests`): the updater manifest builder, the updater configuration check, the release tag check, the `aerini-mcp` packaging script and the asset-name rules shared with the upload script, a check that the advisory ignores in `deny.toml` match `.cargo/audit.toml`, and a check that the `Dockerfile`'s edit of the workspace members leaves exactly the crates the image copies. They use only the standard library. The same job runs `shellcheck` over every script in `scripts/`.
- **`template-wit-sync`**: fails if the two copies of `node.wit` under `examples/plugin-template/` have drifted from `aerini-engine/wit/node.wit`.
- **`trigger-example-build`**: builds `examples/plugin-template` (the echo template, without `--locked` because it ships no `Cargo.lock`) and `examples/plugin-template/trigger-example` (with `--locked`) for `wasm32-wasip2` with `cargo build --target wasm32-wasip2 --release`, so a plugin template that no longer compiles fails the pull request.
- **`cargo-deny`**: runs `cargo deny check licenses bans sources` against `deny.toml`. A dependency whose license is not in the allow list, or that comes from anywhere but crates.io, fails the pull request. The allow list is permissive licenses only; `aerini-server` ships the MPL-2.0 crate `tracking-allocator`, and the desktop app adds five more MPL-2.0 crates (all listed as per-crate exceptions), so a new copyleft dependency needs an explicit decision. Duplicate crate versions are reported as warnings, not failures. Advisories are not part of this job; `security-audit.yml` covers them.
- **`node-checks`**: `npm audit`, then `npm run typecheck` (`tsc --noEmit`), then the full frontend suite via `npm test`, then a check that rebuilding `dist/` changes no committed file and creates no uncommitted one.
- **`docs-build`**: runs `npm run docs:build`, so a broken docs page fails the pull request instead of failing the Pages deploy after merge.
- **`tauri-build`**: builds the desktop app across four platform targets (`ubuntu-22.04`, `ubuntu-22.04-arm`, `windows-latest`, `macos-latest`) via `npm run build:unsigned`, which skips signing the update artifacts because CI has no signing key. The Linux runners match the ones `release.yml` builds on.
- **`tauri-test`**: on `ubuntu-22.04`, runs `cargo test -p aerini --locked`, the desktop crate's own `#[cfg(test)]` suite, then `cargo clippy -p aerini --locked -- -D warnings`.

`docker-smoke.yml` is a separate workflow that builds the official `Dockerfile` and runs `scripts/smoke-test-docker.sh` against the image. It runs only when the Dockerfile, `.dockerignore`, the two server-side crates, `Cargo.*`, `NODE_VERSION`, the smoke script, or the workflow itself changes.

Between `test` and `tauri-test`, CI runs the `#[cfg(test)]` suite of all three Rust crates. Clippy runs on all three crates: `aerini-engine` and `aerini-server` in `test`, `aerini` in `tauri-test`.

`codeql.yml` runs CodeQL code scanning on JavaScript/TypeScript, Rust and the workflow files themselves, on every push and pull request against `main` and weekly. `dependency-review.yml` fails a pull request that adds a dependency with a known high-severity advisory.

`security-audit.yml` is a separate concern from any of the above: it runs `cargo audit` against every crate's dependency tree on a daily schedule and whenever a `Cargo.toml` or `Cargo.lock` changes, checking for known vulnerability advisories, not application behavior. Its ignore list in `.cargo/audit.toml` is mirrored in `deny.toml`. `release.yml` runs on a version tag push and builds and publishes installers and the MCP server programs; it has no test step of its own. Its `verify` job fails the run unless the tagged commit is on `main` and has a successful `ci.yml` run, so a tag pushed before CI finishes fails and can be re-run afterwards. The same job requires the tag (`vMAJOR.MINOR.PATCH`, optionally with a `-prerelease` suffix) to equal the version in `package.json`, `src-tauri/tauri.conf.json`, and the `[workspace.package]` table of the root `Cargo.toml`, and, for a tag without a `-` suffix, `CHANGELOG.md` to have a `## [X.Y.Z]` section; a mismatch fails the run before anything is published. A tag with a `-` suffix is published as a prerelease. Run `bash scripts/verify-release-tag.sh <tag>` locally to check a tag before pushing it. The same job runs `scripts/verify-updater-config.py`, which fails the release if update artifacts, the public key, the update endpoint or `requireSignedVersion` are missing or wrong. Each platform's build signs its installers with the updater key, and a final `manifest` job writes `latest.json` from the signed assets and fails the run if any expected platform is missing. After the desktop build, an `mcp` job per platform builds `aerini-mcp` and `aerini-mcp-offline` with `--locked`, packages each with `scripts/package-mcp.py` and uploads them, with a `.sha256` file each, to the draft release with `scripts/upload-release-assets.sh`; it runs no tests, because `ci.yml` already ran them on the tagged commit. The container image is published to GHCR, with provenance and SBOM attestations, only after the Docker smoke test and the approved desktop build have both succeeded. The maintainer steps are in the "Releasing (maintainers)" section of `CONTRIBUTING.md`.

## What this page doesn't cover

[Adding a Built-in Node](node-authoring.md#tests) covers what a new node's own `#[cfg(test)]` block should contain. [Writing a Plugin Node](plugin-authoring.md) covers building and signing a `.wasm` plugin; it doesn't currently discuss testing one, and as noted above, there's no working example of that in this repository yet. [Architecture](architecture.md) is where the three-crate split itself is explained; this page only says where the tests inside that split live.
