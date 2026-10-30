use wasmtime::Engine;
use wasmtime::component::ResourceTable;
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpCtxView, WasiHttpView};

use crate::plugin_loader::bindings::{wit_fs_watch, wit_storage};
use crate::plugin_loader::fs_watch::PluginFsWatchHandle;
use crate::plugin_loader::http_hooks::PluginHttpHooks;
use crate::plugin_loader::limits::{PLUGIN_EPOCH_DEADLINE, PluginLimits};
use crate::plugin_loader::storage::PluginStorageHandle;

/// Per-execution store state.
///
/// A fresh `PluginState` (and a fresh `Store<PluginState>`) is created for every
/// `execute()` call. This ensures complete isolation between executions.
///
/// Holds:
/// - [`WasiCtx`] — WASIp2 context (sandboxed filesystem, clocks, random, stdio).
/// - [`WasiHttpCtx`] — WASI HTTP context for outbound requests.
/// - [`ResourceTable`] — tracks host-owned resources (shared by WASI and HTTP).
/// - [`PluginLimits`] — enforces the 64 MiB memory cap per execution.
/// - [`PluginHttpHooks`] — enforces this crate's SSRF policy on every outbound
///   request and connects only to the validated address.
/// - `storage` — this call's `storage` import backing, if any. `None` for
///   every `describe()`-time `Store` (no plugin type-id is known yet to
///   scope by) and for a `Store` built after [`PluginStorage::open`]
///   failed for this plugin's directory; every `storage.*` guest call then
///   degrades per `wit/node.wit`'s own doc comment instead of panicking.
pub(super) struct PluginState {
    wasi_ctx: WasiCtx,
    http_ctx: WasiHttpCtx,
    table: ResourceTable,
    limits: PluginLimits,
    http_hooks: PluginHttpHooks,
    storage: Option<PluginStorageHandle>,
    fs_watch: Option<PluginFsWatchHandle>,
}

impl WasiView for PluginState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView { ctx: &mut self.wasi_ctx, table: &mut self.table }
    }
}

impl WasiHttpView for PluginState {
    fn http(&mut self) -> WasiHttpCtxView<'_> {
        WasiHttpCtxView {
            ctx: &mut self.http_ctx,
            table: &mut self.table,
            hooks: &mut self.http_hooks,
        }
    }
}

impl wit_storage::StorageHost for PluginState {
    fn get(&mut self, key: String) -> Option<String> {
        let handle = self.storage.as_ref()?;
        handle.db.get(&handle.scope, &key)
    }

    fn set(&mut self, key: String, value: String) -> Result<(), wit_storage::StorageError> {
        let handle = self.storage.as_ref().ok_or(wit_storage::StorageError::Unavailable)?;
        handle.db.set(&handle.scope, &key, &value)
    }

    fn delete(&mut self, key: String) {
        if let Some(handle) = self.storage.as_ref() {
            handle.db.delete(&handle.scope, &key);
        }
    }

    fn list_keys(&mut self, prefix: String) -> Result<Vec<String>, wit_storage::StorageError> {
        let handle = self.storage.as_ref().ok_or(wit_storage::StorageError::Unavailable)?;
        handle.db.list_keys(&handle.scope, &prefix)
    }
}

impl wit_fs_watch::FsWatchHost for PluginState {
    fn poll(&mut self, target: wit_fs_watch::WatchTarget) -> Result<wit_fs_watch::PollResult, wit_fs_watch::WatchError> {
        let handle = self.fs_watch.as_ref().ok_or(wit_fs_watch::WatchError::Unavailable)?;
        handle.poll(&target)
    }

    fn unwatch(&mut self, target: wit_fs_watch::WatchTarget) {
        if let Some(handle) = self.fs_watch.as_ref() {
            handle.registry.unwatch(&handle.plugin_type_id, &target);
        }
    }
}

pub(super) fn make_plugin_state(
    storage: Option<PluginStorageHandle>,
    fs_watch: Option<PluginFsWatchHandle>,
) -> PluginState {
    PluginState {
        // No preopened directories, no inherited stdio — sandboxed context.
        // Filesystem syscalls succeed at the API level but return "not found" /
        // "permission denied" since no paths are mounted. `fs-watch` (above) is
        // how a plugin observes the real filesystem instead.
        wasi_ctx: WasiCtx::builder().build(),
        http_ctx: WasiHttpCtx::new(),
        table: ResourceTable::new(),
        limits: PluginLimits::default(),
        http_hooks: PluginHttpHooks,
        storage,
        fs_watch,
    }
}

pub(super) fn make_store(engine: &Engine, state: PluginState) -> wasmtime::Store<PluginState> {
    let mut store = wasmtime::Store::new(engine, state);
    store.limiter(|s| &mut s.limits);
    // Trap when the epoch deadline is reached. This is the wasmtime default,
    // but set explicitly so the intent is clear. Requires epoch_interruption
    // to be enabled on the Engine (configured in PluginLoader::new).
    store.epoch_deadline_trap();
    // Set the deadline N ticks in the future. The background ticker thread
    // increments the engine epoch every 10 ms, so this is ~30 s wall-clock.
    store.set_epoch_deadline(PLUGIN_EPOCH_DEADLINE);
    store
}
