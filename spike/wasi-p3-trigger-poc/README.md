# P8.0 spike — WASI P3 async trigger-plugin feasibility

**Status: throwaway, not shipped, not wired into `aerini-engine` or any Tauri/server command** (per `PLAN.md`'s own P8.0 description). This directory exists to answer one question and then be deleted or ignored — it is not a plugin-system feature branch.

**Overall verdict: UNVERIFIED — pending a real `cargo build`.** No Rust toolchain in this environment (Constraint 7); every line below was traced manually against documentation, not compiled or run. See "What a real build needs to confirm" at the end.

**Feasibility verdict (the actual deliverable): yes, a persistent async trigger-plugin Store can coexist with the existing per-call sync `spawn_blocking` Store pattern in `plugin_loader.rs` — but only if they run on two separate `wasmtime::Engine`s, not one.** Details and reasoning below.

---

## 1. Research — re-verified, not reused blind

`PLAN.md`'s existing P8 grounding note is **HIGH but incomplete**, not wrong. Re-checked at this batch's start per Rule 12/14:

| Claim | Status |
|---|---|
| wasmtime 46 ships WASI 0.3 (P3) with native async by default | `VERIFIED` — confirmed, and stronger than PLAN.md stated: the `component-model-async` **wasm feature** is enabled by default as of 46.0.0 (GitHub release notes), not just "available." |
| WASI 0.3.0 ratified 2026-06-11 | `VERIFIED`, unchanged. |
| `wasip3` crate + wit-bindgen is the workable Rust guest path, targeting `wasm32-wasip2` | `VERIFIED`. Current release is **0.7.1+wasi-0.3.0** (crates.io/docs.rs, checked today) — final WASI 0.3.0, not an `-rc` build; PLAN.md's note predates this and only knew of the ratification date. |
| `wasm32-wasip3` native rustc target is Tier 3, avoid | `VERIFIED`, still holds — a Tier-2 promotion proposal is open (`rust-lang/compiler-team#1001`, filed 2026-06-12) but not landed. |
| `wit-bindgen` async-capable version | `VERIFIED` — current standalone release is 0.60.0, but **not used directly by this spike** (see §2 below): `wasip3` 0.7.1 pins `wit-bindgen ^0.57.1` as a normal dependency and re-exports it, so this spike's guest crate uses that re-export instead of adding a second, incompatible copy to the dependency graph. `examples/plugin-template` already pins `wit-bindgen = "0.57.1"` for its own (sync) world — same version, unrelated reason. |
| `wasmtime_wasi::p3` module | **New finding, not in PLAN.md's grounding note:** exists in `wasmtime-wasi` 46.0.1, but is gated behind a `p3` Cargo feature the crate does not enable by default. `aerini-engine/Cargo.toml`'s current `wasmtime-wasi = "46.0.1"` line (no `features`) does **not** get it. Concrete P8.1/P8.2 prerequisite, not spike-only. |
| Required `Config` flags for the host | `VERIFIED` (docs.wasmtime.dev code sample): `wasm_component_model_async(true)` **and** `async_support(true)` together, in addition to the already-familiar `wasm_component_model(true)`. A third flag, `Config::concurrency_support`, is referenced by `GuardedStreamReader`'s own panic message ("Panics if `Config::concurrency_support` is not enabled") but no verified sample actually sets it — `UNCERTAIN` whether this spike's single-task read loop needs it; see §4. |
| `wasmtime_wasi`'s own P3 maturity | `VERIFIED` — the crate's own top-level docs literally say "WASIp3 support is experimental, unstable and incomplete." Not a PLAN.md-copied assumption. |

---

## 2. What's in this directory

- `guest/` — a `wasm32-wasip2` Rust component (`wit/trigger.wit` + `src/lib.rs`) exporting one `trigger` interface with one function, `events: async func() -> stream<event>`, that spawns a background task emitting 3 timestamped events 50ms apart, then closes the stream. Timestamps come from a real WASI 0.3 import (`wasip3::clocks::monotonic_clock::now`/`wait_for`), not a stub — this is a genuine (if minimal) P3 async guest, not a sync plugin with a different file extension.
- `host/` — a `tokio`-based async harness (`src/main.rs`) that builds an async-enabled `Engine`, links `wasmtime_wasi::p3`, instantiates the component **once** into a single `Store`, and calls `events()` **twice** against that same store/instance to demonstrate the instance surviving across polls (the actual question this spike exists to answer).
- Both crates carry their own empty `[workspace]` table so neither is swept into the repo's root workspace — zero edits to any existing file, matching P8.0's "throwaway, own scratch location" framing exactly.

---

## 3. The coexistence question — direct answer

`plugin_loader.rs` today builds **one process-wide `Engine`** (`Config::epoch_interruption(true)`, no `async_support`), and every `execute()` call creates a **fresh, synchronous `Store`** inside `spawn_blocking`, instantiated via `wasmtime_wasi::p2::add_to_linker_sync`. This is correct and should not change for action plugins — full per-call isolation is the point.

A trigger plugin is the opposite shape: one `Store` per **installed trigger plugin instance**, created once (e.g. at trigger-registration time) and kept alive for the process's lifetime (or until the trigger is disabled/reloaded), so the guest's spawned background task can keep running and the host can poll/drain its stream repeatedly.

**These two shapes cannot share one `Engine`.** Per Wasmtime's own embedding-API docs (verified, §1): once `Config::async_support(true)` is set, *every* store built from that `Engine` must use the async API surface — `add_to_linker_sync` and synchronous `Func::call` against such a store are documented to return an error, not silently work. `plugin_loader.rs`'s existing sync path and a new async trigger path are therefore mutually exclusive on one `Engine`.

**Recommendation for P8.1/P8.2: two `Engine`s in one process**, not a shared one:

- The existing `PluginLoader::shared()` engine is untouched — action plugins keep working exactly as today, zero regression risk to P1–P6's shipped behavior.
- A second, new `Engine` (own `Config`: `wasm_component_model(true)`, `wasm_component_model_async(true)`, `async_support(true)`; likely **no** `epoch_interruption`, see next point) is constructed once, at process startup, dedicated to trigger plugins.
- Trigger execution runs as an ordinary `tokio` task on the existing runtime — **not** `spawn_blocking`. `spawn_blocking` exists specifically to keep a *blocking* call off the async executor; an async guest export doing real async I/O has no blocking call to hide, and wrapping it in `spawn_blocking` anyway would waste a blocking-pool thread for the trigger's entire lifetime (which, for a trigger, is "indefinitely" — exactly the resource this repo's own `spawn_blocking` doc comment implies is meant to be scarce and per-call).
- Timeout/cancellation for a stuck trigger poll: prefer `tokio::time::timeout` wrapping the drive loop over the sync path's epoch-ticker-thread trap. This is a genuine advantage of the async shape, not just a style choice — a synchronous guest stuck in a tight loop can only be stopped by the epoch trap (the thread itself can't be interrupted), but dropping a `tokio::time::timeout`'d future for an async guest task actually cancels it. A second epoch-ticker OS thread for the trigger engine is avoidable this way.
- File format, install/signing/packaging (P2/P5/P6) need **no changes** for trigger plugins: a P3-targeting guest is still an ordinary `wasm32-wasip2` Component Model binary (native `wasm32-wasip3` remains avoided, §1) — only the WIT world it targets and which of the two engines/linkers loads it differ. This is a materially better answer than PLAN.md's framing implied might be needed.
- `WasiCtx`/`WasiCtxView`/`WasiView` plumbing is identical in shape to the existing p2 code (same trait, defined once in `wasmtime_wasi`, version-independent) — confirmed directly against `plugin_loader.rs`'s own `PluginState`/`impl WasiView` block, which this spike's `HostState` mirrors exactly. No new host-state design needed, just a second implementing struct (or the same `PluginState` shape reused) paired with the new engine.

**Named blocker for P8.1 (not this spike's job to resolve, flagged per Rule 3):** whether trigger-plugin `WasiCtx`s get any capabilities (network, in particular) beyond the current action-plugin default of none. `wasi:clocks` needs no such grant (informational, not gated by `WasiCtxBuilder` the way filesystem/network are — `HIGH`, matches long-standing WASIp2 precedent, not independently re-verified for P3's clocks specifically). A real trigger plugin (webhook poller, file watcher, etc.) will likely need more; that's a P8.1 design call, not an engine-plumbing one.

---

## 4. Manual trace (Rule 16 / AGENT_PROTOCOL step 7 — not compiled, no toolchain here)

**Guest, normal case:** `events()` is called once by the host. It creates `(tx, rx)` via `wasip3::wit_stream::new::<Event>()`, spawns a task that writes 3 events 50ms apart via `monotonic_clock::wait_for`, and returns `rx` immediately — before the spawned task has written anything. Host receives a valid, empty-so-far reader and begins reading; elements arrive as the spawned task produces them. After the third `write_all`, the loop ends, `tx` is dropped, the stream closes.

**Guest, edge case (host drops `rx` early):** the spawned task's next `tx.write_all(...).await` should fail or be cancelled since there's no reader left. This code does not inspect `write_all`'s return value (§5, weakness 1) — a real trigger plugin would need to; this echo-stream spike does not, since the fixed 3-event sequence is short enough that a dropped reader mid-sequence is not a scenario worth handling for a throwaway spike, but it is a real gap a shipped trigger-plugin SDK must close.

**Host, normal case:** builds an async-enabled `Engine`, links WASI P3, instantiates the component once, calls `events()`, drains the returned reader in a loop, prints each event, then calls `events()` **again on the same `store`/`bindings`** and repeats. If this compiles and runs, it directly answers the coexistence question: the same `Store`/instance survives being asked for a second, independent event sequence, proving a long-lived trigger `Store` is a viable pattern, not just a per-call one dressed up in `async fn`.

**Host, edge case (second `call_events` before first poll's spawned guest task has been reaped):** genuinely `UNCERTAIN` — not found in any verified source. Component-model-async's task/subtask model (§ "Canonical ABI" doc, verified) suggests a completed subtask is disposed once its result is delivered, which should make a second call clean, but this spike found no documentation confirming there is no other cleanup step needed between calls on one store. Flagged, not resolved — first thing to watch for in a real build.

---

## 5. Self-audit (Rule 16) — weaknesses found

1. **Host `StreamReader::read` call (`host/src/main.rs`) is the single least-verified line in this spike.** Documentation search confirmed `StreamReader<T>` exists with `close`/`close_with`/`into_guarded` (the last requiring an `Accessor` and, per its own panic message, possibly `Config::concurrency_support(true)`), but did not turn up a plain, ungated read method's exact name or signature. The `reader.read(&mut store, buf).await?` call here is this spike's best-effort reconstruction from the verified shape of the guest-side `StreamWriter::write_all` and general Wasmtime buffer-based read/write conventions (`ReadBuffer`/`WriteBuffer` types, verified to exist) — not independently confirmed. **Not fixed** — this is exactly the kind of unknown a real `cargo build` resolves in minutes with compiler feedback, and no amount of further doc searching in this environment would responsibly close it without guessing further. Flagged, not hidden.
2. **Guest `write_all`'s return value is discarded** (`guest/src/lib.rs`) — an earlier draft invented a specific `Option`-returning check with no source backing it; removed during self-audit in favor of the one verified call shape (`tx.write_all(...).await;`, no result inspected), which is honest but means a dropped-reader scenario mid-sequence isn't handled. Documented above (§4) rather than papered over with fabricated error-handling.

---

## 6. What a real build needs to confirm (top of the LAUNCH GATE list for P8.x)

1. `reader.read(...)`'s actual name/signature (§5.1) — first compile error will show this immediately.
2. Whether `Config::concurrency_support(true)` is needed for a single-task, non-concurrent stream read, or only for the `Accessor`/`GuardedStreamReader` multi-task path.
3. Whether `async func` is strictly required in WIT for a guest-exported function returning `stream<T>`, or merely conventional — `component-model/design/mvp/Concurrency.md` (verified) states functions are async-by-default at the ABI level in P3, which suggests the keyword may be about binding-generator ergonomics rather than a hard requirement, but this spike's WIT uses `async func` regardless since that's what every real sample found uses.
4. Whether a second `call_events` against the same store genuinely requires no extra cleanup step (§4).
5. Whether `wasmtime-wasi`'s "p3" feature has any transitive dependency conflicts against the rest of `aerini-engine/Cargo.toml`'s already-resolved `Cargo.lock` — untested, this spike's `host/Cargo.toml` is fully standalone and shares no lockfile with the workspace.

None of the above block the §3 architectural verdict (two engines, async task not `spawn_blocking`, no file-format changes) — they're implementation-detail risks inside that verdict, not challenges to it.
