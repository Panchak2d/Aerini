# Aerini Trigger-Plugin Example — Heartbeat

A minimal trigger plugin: starts a workflow run every `interval_secs` seconds. Demonstrates the `trigger` interface alongside the `node` interface every plugin already implements — see [docs/plugin-authoring.md](../../../docs/plugin-authoring.md#trigger-plugins) for the concepts.

> **Status: not build-verified.** A missing `wasip3` feature flag (`wit-bindgen-macros`) that would have blocked this crate from compiling at all was fixed in a later batch, but no real `cargo build` has run against it since — this environment has no `wasm32-wasip2`-capable toolchain. Treat this as an unverified guest-side pattern reference, not a confirmed build, until compiled on real hardware. Separately, Aerini's host-side trigger event pump has open implementation bugs — don't expect this to fire inside a running Aerini instance even once it compiles.

This is a separate, self-contained crate from the base `../` echo template — it targets the `aerini-node-with-trigger` world instead of plain `aerini-node`, and needs the `wasip3` crate for async guest bindings. It does not affect or depend on the base template.

---

## Prerequisites

Same as the base template, plus:

- **Rust 1.82 or later**, `wasm32-wasip2` target (`rustup target add wasm32-wasip2`)

No extra tooling beyond that — `wasip3` is a normal Rust dependency, resolved by Cargo like any other.

## Build

```bash
cd trigger-example
cargo build --target wasm32-wasip2 --release
```

The compiled plugin is at:
```
target/wasm32-wasip2/release/aerini_plugin_heartbeat_trigger.wasm
```

## What it does

- **`describe()`** — registers as `com.example.heartbeat-trigger`, category `"utility"` (there is no `"trigger"` category — see the linked docs section for why).
- **`execute()`** — no-op, returns trivial success immediately. Required by the WIT world; not where this plugin's real behavior lives.
- **`events(config)`** — reads `interval_secs` from `config` (defaults to 60 if missing or non-positive), then emits one JSON event — `{"tick": N, "emitted_at_ns": ...}` — per interval, indefinitely.

## Customising

Same shape as the base template: rename the crate, edit `describe()`, and change what `events()` emits and on what condition. If you don't need a fixed interval — e.g. a webhook poller or file watcher instead — the `events()` signature and stream-writing pattern here are still the starting point; only the fire condition and payload change.
