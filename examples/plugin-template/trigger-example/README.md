# Aerini Trigger-Plugin Example — Heartbeat

A minimal trigger plugin: starts a workflow run every `interval_secs` seconds. Demonstrates the `trigger` interface alongside the `node` interface every plugin already implements — see [docs/development/plugin-authoring.md](../../../docs/development/plugin-authoring.md#trigger-plugins) for the concepts.

> **Status:** builds with `cargo build --target wasm32-wasip2 --release`. Aerini's host-side trigger event pump still has open implementation bugs — don't expect this to fire inside a running Aerini instance.

This is a separate, self-contained crate from the base `../` echo template — it targets the `aerini-node-with-trigger-and-next-fire` world instead of plain `aerini-node`, and needs the `wasip3` crate for async guest bindings. It does not affect or depend on the base template.

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
- **`trigger-schedule.report-next-fire`** — called when the stream starts and after each beat with the time the next one is due, so the Background Runs panel shows "next in Ns" for this trigger. See [Showing a countdown to the next event](../../../docs/development/plugin-authoring.md#showing-a-countdown-to-the-next-event).

## Customising

Same shape as the base template: rename the crate, edit `describe()`, and change what `events()` emits and on what condition. If you don't need a fixed interval — e.g. a webhook poller or file watcher instead — the `events()` signature and stream-writing pattern here are still the starting point; only the fire condition and payload change.
