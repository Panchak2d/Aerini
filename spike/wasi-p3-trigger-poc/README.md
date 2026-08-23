# WASI P3 Trigger Plugin Spike

An experimental proof of concept. It shows a WebAssembly component that can keep running in the background and send a stream of events back to the program that started it — and that the same running component can be asked for another stream a second time, instead of having to be restarted for every request.

If you're new to WebAssembly outside the browser, the "Background" section below explains the terms used in this document. If you just want to run it, skip to "How to use it."

## What it is

### Background, in plain terms

- **WebAssembly (Wasm)** is a small, fast, sandboxed program format. It's best known for running in web browsers, but it can also run standalone, outside a browser, using a runtime like [Wasmtime](https://wasmtime.dev/).
- **WASI** ("WebAssembly System Interface") is a standard set of APIs that let a Wasm program do things like tell the time or read a file, without being tied to one specific operating system. **WASI 0.3**, also called **"Preview 3"** or **"P3"**, is a version of that standard that adds native support for asynchronous operations — things like "start a background task" or "give me a stream of results over time," which earlier versions of WASI couldn't express.
- A **guest** is the Wasm program being run. A **host** is the program that loads and runs it. In this project, `guest/` is the Wasm program, and `host/` is a normal, native Rust program that loads and calls it.
- A **stream** here is exactly what it sounds like: instead of the guest handing back one value and finishing, it hands back a channel that the host can keep reading from, receiving new values as they become available.

### The two parts

This project has two independent pieces:

- **`guest/`** — a Rust project that compiles into a Wasm component (built for the `wasm32-wasip2` target). It exports one function, `events()`, which returns a stream of events instead of a single value.
- **`host/`** — a plain Rust program that loads that Wasm component and calls `events()` on it.

They are separate Rust crates with no shared build — you build and run them one after the other, as shown below.

This is a standalone spike: it isn't part of, or connected to, any other project or application.

## What it does

Here's the flow when you run it, step by step:

```
 host                              guest (Wasm component)
 ─────                              ───────────────────────
 1. loads the component
 2. calls events() ───────────────► starts a background task
                                     inside the component
 3. gets back a stream        ◄──── returns immediately
 4. reads events as they                (doesn't wait for the
    arrive, prints each one)             background task to finish)
                                     background task, every ~50ms:
                                       emits one event
 5. stream ends after 3 events ◄──── after 3 events, task stops
                                     and the stream is closed
 6. calls events() again ─────────► same component instance,
    (same running instance)          starts a new background task
 7. reads and prints 3 more   ◄──── ...and the cycle repeats
    events, same as before
```

In more detail:

1. **The host calls `events()` on the guest.** This doesn't block — the guest hands back a stream to read from right away, even before any events exist yet.
2. **Inside the guest, a background task starts** and produces 3 events, one every 50 milliseconds. Each event has:
   - a sequence number (`0`, `1`, `2`),
   - a short message (e.g. `spike-event-0`),
   - and a timestamp (in nanoseconds, taken at the moment the event was created).
3. **The host reads events off the stream as they arrive** and prints each one to the console.
4. **Once all 3 events have been sent, the stream closes**, and the host's read loop ends.
5. **The host calls `events()` a second time — on the exact same, still-running component instance.** This repeats steps 2–4. The point of doing this twice is to show that one component instance can be reused for multiple requests, rather than needing to be thrown away and reloaded after each one.

## How to use it

### Prerequisites

You need the Rust toolchain installed. If you don't have it yet, install it from [rustup.rs](https://rustup.rs/) — this gives you `rustc` (the compiler) and `cargo` (the build tool/package manager) used below.

You also need the `wasm32-wasip2` compilation target, which lets Rust compile to a Wasm component. Install it with:

```bash
rustup target add wasm32-wasip2
```

You only need to run this once per machine — it's not something you repeat for every build.

### Step 1: Build the guest component

The guest has to be compiled to Wasm *before* the host can load it.

```bash
cd guest
cargo build --release --target wasm32-wasip2
```

This produces a `.wasm` file at `guest/target/wasm32-wasip2/release/aerini_spike_trigger_guest.wasm`. The host (next step) expects to find it at exactly that path, relative to where the host is run from — that's why the folder layout matters and why step 2 below says to run from `host/`.

### Step 2: Run the host

```bash
cd ../host
cargo run
```

`cargo run` both builds and runs the host program in one command. The first time you run it, it will also download and compile the host's own dependencies, which can take a minute or two.

### What you should see

Output will look something like this (the exact timestamps will differ every time you run it, since they're taken live):

```
poll 1: seq=0 message="spike-event-0" emitted_at_ns=1234500000000
poll 1: seq=1 message="spike-event-1" emitted_at_ns=1234550000000
poll 1: seq=2 message="spike-event-2" emitted_at_ns=1234600000000
poll 1 drained in 104.32ms
poll 2: seq=0 message="spike-event-0" emitted_at_ns=1234700000000
poll 2: seq=1 message="spike-event-1" emitted_at_ns=1234750000000
poll 2: seq=2 message="spike-event-2" emitted_at_ns=1234800000000
poll 2 drained in 103.87ms
```

Two "polls" (rounds), 3 events each, roughly 100ms apart per round (3 events spaced 50ms apart) — matching the two `events()` calls described above.

### Troubleshooting

- **`error: target 'wasm32-wasip2' not found`** — you skipped the `rustup target add wasm32-wasip2` step, or it failed silently. Run it again and check for errors.
- **Host can't find the `.wasm` file / file-not-found style error** — you're running `cargo run` from the wrong folder, or you haven't completed Step 1 yet. The host looks for the guest binary using a path relative to the `host/` folder, so it must be built first and the host must be run from inside `host/`.
- **Build errors when compiling the guest or host** — this is an experimental spike using an early, evolving part of the WebAssembly ecosystem (WASI 0.3 / Preview 3 async support). If a dependency has moved on to a newer version with breaking changes since this was written, you may need to adjust version numbers in `guest/Cargo.toml` or `host/Cargo.toml`.
