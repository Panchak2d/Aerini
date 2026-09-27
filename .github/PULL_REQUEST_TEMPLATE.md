<!--
PR title format: [node] Add YourNode | [fix] Short description | [refactor] Scope | [docs] What
-->

Closes #<!-- issue number, or delete this line -->

## What

<!-- One paragraph. -->

## Why

<!-- What does it fix or enable? -->

## Testing

<!-- Unit tests / manual steps. If no tests, say why. -->

## Checklist

- [ ] `cargo clippy -p aerini-engine -p aerini-server -- -D warnings` passes
- [ ] `cargo test -p aerini-engine` and `cargo test -p aerini-server` pass
- [ ] No `.unwrap()` in production paths
- [ ] **Frontend change only:** `npm run typecheck` and `npm test` pass; `dist/` rebuilt with `npm run vite:build` and committed (CI fails on a stale one)
- [ ] **New node only:** registered in `nodes/mod.rs`, icon added in `src/utils.ts`, at least one `#[cfg(test)]` block added
- [ ] **Protected zone change** (IPC command, serde type, `async_trait` impl): described in section below
- [ ] **First PR:** signed the [Contributor License Agreement](CLA.md)

## Protected zone changes

<!-- IPC commands, serde types, or async_trait impls touched. Delete if none. -->

## Breaking changes

<!-- Schema, config, or wire format changes that affect existing workflows.
     Delete if none. -->
