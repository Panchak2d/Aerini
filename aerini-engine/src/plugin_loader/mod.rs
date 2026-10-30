//! WASM plugin loader — loads third-party `.wasm` components as node implementations.
//!
//! # Architecture
//!
//! [`PluginLoader`] holds a shared [`wasmtime::Engine`] (expensive to construct; built
//! once per process via [`PluginLoader::shared`], which every call site — startup
//! loading and later, repeatable inspection/listing calls alike — goes through
//! instead of [`PluginLoader::new`] directly).
//! [`load_plugins_from_dir`](PluginLoader::load_plugins_from_dir)
//! iterates a directory, calls [`load_plugin`](PluginLoader::load_plugin) for each
//! `.wasm` file, logs warnings for failures, and returns all successfully loaded nodes.
//! [`describe_plugin`](PluginLoader::describe_plugin) does the same compile/link/describe
//! work as `load_plugin` for a single file but returns owned metadata instead of a
//! registered, `'static`-leaking [`Node`] — the path for callers (e.g. a UI listing
//! command) that need to call this repeatedly rather than once per process.
//!
//! Each [`WasmPluginNode`] is registered in [`crate::node::NodeRegistry`] identically
//! to built-in nodes and appears in the UI palette automatically.
//!
//! # Isolation model
//!
//! A fresh [`wasmtime::Store`] is created for every `execute()` call. Stores are not
//! reused across calls: one failed execution cannot corrupt the next, and there is no
//! shared mutable state between concurrent executions of the same plugin.
//!
//! The shared [`wasmtime::Engine`] and the pre-compiled
//! [`AeriniNodePre`](wit::AeriniNodePre) (which wraps [`wasmtime::component::InstancePre`])
//! are both `Clone + Send + Sync` and are cheaply shared across threads.
//!
//! # Memory limit
//!
//! Each Store is limited to [`PLUGIN_MEMORY_LIMIT`] bytes (64 MiB) of linear memory
//! in total, summed over every memory the component creates, and to
//! [`PLUGIN_TABLE_ELEMENT_LIMIT`] elements per table. Both are enforced by
//! [`PluginLimits`] via [`wasmtime::Store::limiter`].
//!
//! # WASI surface
//!
//! Two linker calls together cover the full surface a `wasm32-wasip2` plugin needs:
//!
//! - `wasmtime_wasi::p2::add_to_linker_sync` — full WASIp2 (`wasi:cli/command` world):
//!   filesystem (sandboxed, no preopened dirs), random, clocks, stdio, exit.
//! - `wasmtime_wasi_http::p2::add_only_http_to_linker_sync` — adds `wasi:http/outgoing-handler`
//!   without duplicating the WASI interfaces already added above.
//!
//! Using just `wasmtime_wasi_http::p2::add_to_linker_sync` (the proxy world bundle) would
//! omit `wasi:filesystem`, `wasi:random`, etc., causing `linker.instantiate_pre` to fail
//! for any plugin compiled with the standard `wasm32-wasip2` target.
//!
//! Plugins cannot access the real filesystem: `WasiCtx::builder().build()` creates an
//! empty context with no preopened directories. Filesystem syscalls return errors.
//!
//! # Outbound HTTP
//!
//! Every request a plugin sends through `wasi:http/outgoing-handler` is checked
//! against this crate's standard SSRF policy — the same `SsrfPolicy::Strict` the
//! Database and HTTP nodes enforce. The host name is resolved once, every
//! address must pass, and the connection goes to those validated addresses
//! while the TLS server name stays the original host (see `plugin_http`), so
//! DNS rebinding cannot swap the destination between check and connect.
//! Action plugins (`wasi:http` p2) and trigger plugins (p3, `trigger_engine`'s
//! async ABI) share one [`PluginHttpHooks`] implementation, so the two engines
//! can't drift apart on what's blocked. A plugin that follows a redirect issues
//! a new request, which goes through the same path.
//!
//! # Plugin storage
//!
//! The optional WIT `storage` import (`wit/node.wit`'s `interface storage`)
//! gives a plugin a small, bounded, host-provided key-value store, scoped
//! per plugin type-id — see that interface's own doc comment for the
//! scoping rationale and [`PluginStorage`]'s for the backend. Backed by
//! the same `rusqlite`/`r2d2` stack `crate::db::WorkflowDb` already uses,
//! in a dotfile database inside `plugin_dir` (opened lazily,
//! [`PluginLoader::plugin_storage`]). Linked into every plugin
//! unconditionally, like WASI/HTTP above — already-published plugins that
//! don't import it are unaffected.
//!
//! # Trigger plugins
//!
//! A second, dedicated `wasmtime::Engine` (`PluginLoader::trigger_engine`, configured
//! with `wasm_component_model_async`) hosts trigger-plugin
//! components — those exporting `aerini-node-with-trigger`'s optional `trigger`
//! interface alongside `node`. Action plugins' `node`/`describe`/`execute` path
//! above stays fully synchronous on the other engine; see `wit/node.wit`'s doc
//! comment on `interface trigger` for why the two can't share one engine. A trigger
//! plugin that imports WASI 0.3 cannot be hosted on the synchronous engine at all
//! (those imports are async-typed), so its own `describe`/`execute` run here too,
//! through the `*_async` APIs: `load_plugin`/`describe_plugin` keep their
//! synchronous signatures by driving that work on a scratch thread with its own
//! runtime, and `execute` simply awaits it. Trigger plugins get
//! SSRF-filtered `wasi:http` (p3) linked the same way action plugins get p2 — see
//! "Outbound HTTP" above — but no raw-socket grant, the same effective position
//! action plugins are in absent an explicit grant. See [`PluginLoader::start_trigger`].
//!
//! # Filesystem watching
//!
//! The optional WIT `fs-watch` import (`wit/node.wit`'s `interface fs-watch`) is a
//! plugin's only way to observe the real filesystem, since the sandbox above denies
//! it outright. [`FsWatchRegistry`] runs real `notify`-backed watchers continuously
//! in the host process, keyed per plugin type-id like `storage`, and buffers events
//! for whichever plugin calls `poll()` next — no relation to `trigger`/the async
//! event pump above; `poll` is a normal synchronous import call, satisfiable from an
//! ordinary `execute()`, so it doesn't inherit that pump's open bugs. Linked into
//! every plugin's linker unconditionally, exactly like `storage`.

mod bindings;
mod convert;
mod describe;
mod error;
mod fs_watch;
mod http_hooks;
mod limits;
mod loader;
mod node;
mod register;
pub mod signature;
mod state;
mod storage;
mod trigger;

pub use error::PluginLoadError;
pub use loader::{PluginDescriptor, PluginLoader};
pub use node::WasmPluginNode;
pub use register::{load_plugins, PluginLoadReport};
pub use trigger::NextFireWatch;
