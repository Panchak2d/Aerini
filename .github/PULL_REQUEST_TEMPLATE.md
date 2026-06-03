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

- [ ] `cargo clippy -p flowo-engine -p flowo-server -- -D warnings` passes
- [ ] `cargo test -p flowo-engine` passes
- [ ] No `.unwrap()` in production paths
- [ ] `dist/` not committed
- [ ] **New node only:** registered in `nodes/mod.rs`, icon added in `src/utils.ts`, at least one `#[cfg(test)]` block added
- [ ] **Protected zone change** (IPC command, serde type, `async_trait` impl): described in section below
- [ ] **First PR:** signed the [Contributor License Agreement](CLA.md)

## Protected zone changes

<!-- IPC commands, serde types, or async_trait impls touched. Delete if none. -->

## Breaking changes

<!-- Schema, config, or wire format changes that affect existing workflows.
     Delete if none. -->
