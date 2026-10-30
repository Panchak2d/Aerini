use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use wasmtime::{Config, Engine};

use crate::model::NodeType;
use crate::plugin_loader::signature;
use crate::plugin_loader::fs_watch::FsWatchRegistry;
use crate::plugin_loader::storage::PluginStorage;

mod link;
mod load;
mod trigger;
mod verify;

#[cfg(test)]
mod tests;

/// Loads WASM plugin components and registers them as [`Node`] implementations.
///
/// `Engine` is expensive to construct and designed to be shared across threads.
/// Build one `PluginLoader` at startup and reuse it for all plugin loading calls —
/// in practice, call [`PluginLoader::shared`] rather than [`PluginLoader::new`]
/// directly, so this invariant is enforced by construction rather than by
/// caller discipline.
pub struct PluginLoader {
    engine: Engine,
    /// Compiled-`Component` cache keyed by plugin path — see
    /// `compile_and_link` for the caching/invalidation contract.
    component_cache: Mutex<HashMap<PathBuf, CachedComponent>>,
    /// Counts real (cache-miss) compiles — lets tests assert cache-hit
    /// behavior directly instead of only inferring it from timing.
    /// Per-instance (not a shared global) so tests running in parallel
    /// can't interfere with each other's counts. Always present (one
    /// `AtomicUsize`, one relaxed-cost increment per real compile — compile
    /// itself dwarfs this) rather than `#[cfg(test)]`-gated, since only the
    /// *definition* form of field-level `cfg` is a confirmed-safe pattern;
    /// only the read accessor below needs to be test-only.
    #[allow(dead_code)]
    compile_count: std::sync::atomic::AtomicUsize,
    /// Second, dedicated engine for trigger-plugin components (the
    /// `aerini-node-with-trigger` world). Configured for
    /// `wasm_component_model_async`, which `engine` above is not -- the two
    /// configurations are mutually exclusive on one `Engine`/`Config`, so
    /// action-plugin execution and trigger-plugin event streaming never
    /// share one. Epoch-interrupted like `engine`; see `make_trigger_store`.
    trigger_engine: Engine,
    /// Compiled-`Component` cache for `trigger_engine`, keyed and
    /// invalidated identically to `component_cache` — kept as a separate
    /// field (not merged into it) because a `Component` compiled against
    /// one engine cannot be instantiated on the other; sharing one map
    /// keyed only by path would risk serving a component compiled for the
    /// wrong engine on a fingerprint match.
    trigger_component_cache: Mutex<HashMap<PathBuf, CachedComponent>>,
    /// Counts real (cache-miss) compiles against `trigger_component_cache`,
    /// across both entry points that can populate it
    /// ([`compile_trigger_component`](PluginLoader::compile_trigger_component)
    /// and [`compile_trigger_component_sync`](PluginLoader::compile_trigger_component_sync))
    /// -- both funnel through the one `insert_trigger_component` call site,
    /// so one counter covers both without the two ever being able to drift
    /// apart. Same not-`#[cfg(test)]`-gated-at-the-field-level rationale as
    /// `compile_count` above.
    #[allow(dead_code)]
    trigger_compile_count: std::sync::atomic::AtomicUsize,
    /// Opened, cached [`PluginStorage`] databases, keyed by the `plugin_dir`
    /// they back — see [`PluginLoader::plugin_storage`]. A `HashMap` (not a
    /// single `Option`) for the same reason `component_cache` is keyed by
    /// path rather than being one bare field: nothing today calls
    /// `load_plugin`/`load_plugins_from_dir` against more than one
    /// `plugin_dir` in one process, but keying by path costs nothing and
    /// doesn't assume that stays true.
    storage_pools: Mutex<HashMap<PathBuf, Arc<PluginStorage>>>,
    /// Signature-verification result cache, keyed by plugin path and
    /// fingerprinted identically to `component_cache` — see
    /// `verify_signature` for the caching/invalidation contract.
    signature_cache: Mutex<HashMap<PathBuf, CachedSignature>>,
    /// Counts real (cache-miss) signature checks — same not-`#[cfg(test)]`-
    /// gated-at-the-field-level rationale as `compile_count`.
    #[allow(dead_code)]
    signature_check_count: std::sync::atomic::AtomicUsize,
    /// Backing for the WIT `fs-watch` import — one per process, unlike
    /// `storage_pools` above, since it holds no on-disk file to key by
    /// `plugin_dir`: every real `notify` watcher it starts lives only in
    /// memory, for this process's lifetime. See [`FsWatchRegistry`].
    fs_watch_registry: Arc<FsWatchRegistry>,
}

/// One cached, already-compiled [`wasmtime::component::Component`] plus the
/// file fingerprint (mtime, length) it was compiled from.
///
/// `Component::clone` is a cheap, `Arc`-backed shallow copy, not a
/// recompilation — it creates a new reference to the existing component
/// rather than an entirely new one, so serving a cache hit costs one
/// `stat()` plus one cheap clone, versus a full read + Cranelift compile
/// on a miss.
struct CachedComponent {
    fingerprint: (Option<SystemTime>, u64),
    component: wasmtime::component::Component,
}

/// One cached [`signature::SigCheckResult`] plus the file fingerprint
/// (mtime, length) it was computed from — same shape as [`CachedComponent`],
/// so repeated hot-reload polling doesn't re-hash an unchanged file.
struct CachedSignature {
    fingerprint: (Option<SystemTime>, u64),
    result: signature::SigCheckResult,
}

/// Process-wide, lazily-constructed [`PluginLoader`].
///
/// [`PluginLoader::new`] builds a `wasmtime::Engine` and spawns a permanent,
/// process-lifetime epoch-ticker OS thread — both are meant to exist at most
/// once per process (see the struct-level doc comment above). Every call site
/// that needs a loader — [`load_plugins`] at startup, and any UI-facing
/// listing/inspection command — goes through [`PluginLoader::shared`] instead
/// of calling `new()` itself, so a caller invoked repeatedly within one
/// process (e.g. a Plugins settings panel re-listing installed plugins) can
/// never construct a second engine/thread.
static SHARED_LOADER: OnceLock<Result<PluginLoader, String>> = OnceLock::new();

/// Metadata about a plugin, obtained without registering it as a live
/// [`Node`] and without leaking any memory.
///
/// [`PluginLoader::load_plugin`] returns an `Arc<dyn Node>` whose `type_id`
/// and `display_name` are [`Box::leak`]ed, because the [`Node`] trait
/// requires `&'static str` — correct and bounded when called once per
/// plugin at process startup (see [`load_plugins`]), but not safe to call
/// repeatedly. [`PluginLoader::describe_plugin`] runs the identical
/// compile/link/describe steps but returns owned data instead, so it is
/// safe to call as many times as a caller needs (e.g. every time a UI
/// panel lists installed plugins).
pub struct PluginDescriptor {
    pub type_id: String,
    pub display_name: String,
    pub node_type: NodeType,
}

impl PluginLoader {
    /// Construct a new `PluginLoader` with a Wasmtime engine configured for the
    /// Component Model.
    ///
    /// Returns an error if the engine cannot be initialised (e.g. unsupported CPU
    /// features on the host).
    pub fn new() -> wasmtime::Result<Self> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        // Enable epoch-based CPU time limiting. All stores created from this
        // engine must call set_epoch_deadline() before executing WASM — handled
        // in make_store(). Without a deadline set, stores trap immediately.
        config.epoch_interruption(true);
        let engine = Engine::new(&config)?;

        // Spawn a process-lifetime background thread that advances the epoch
        // counter every 10 ms. The thread holds a clone of the engine (cheap —
        // Engine is Arc-backed) and runs until the process exits. Combined with
        // PLUGIN_EPOCH_DEADLINE = 3 000, this gives plugins ~30 s per call.
        //
        // Second engine for trigger-plugin components. `wasm_component_model_async`
        // is what lets this engine compile and instantiate a component using
        // `trigger.events`'s async ABI; `engine` above doesn't set it, and per
        // `wit/node.wit`'s own doc comment the two configurations cannot be
        // combined on one `Engine`. Epoch interruption is enabled here too, so
        // a trigger plugin's `describe`/`execute` is bounded exactly like an
        // action plugin's — see `make_trigger_store`.
        let mut trigger_config = Config::new();
        trigger_config.wasm_component_model(true);
        trigger_config.wasm_component_model_async(true);
        trigger_config.epoch_interruption(true);
        let trigger_engine = Engine::new(&trigger_config)?;

        // One ticker advances both engines' epochs.
        let ticker_engine = engine.clone();
        let ticker_trigger_engine = trigger_engine.clone();
        std::thread::Builder::new()
            .name("wasm-epoch-ticker".to_string())
            .spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_millis(10));
                ticker_engine.increment_epoch();
                ticker_trigger_engine.increment_epoch();
            })
            .expect("failed to spawn wasm epoch ticker thread");

        Ok(Self {
            engine,
            component_cache: Mutex::new(HashMap::new()),
            compile_count: std::sync::atomic::AtomicUsize::new(0),
            trigger_engine,
            trigger_component_cache: Mutex::new(HashMap::new()),
            trigger_compile_count: std::sync::atomic::AtomicUsize::new(0),
            storage_pools: Mutex::new(HashMap::new()),
            signature_cache: Mutex::new(HashMap::new()),
            signature_check_count: std::sync::atomic::AtomicUsize::new(0),
            fs_watch_registry: Arc::new(FsWatchRegistry::new()),
        })
    }

    /// Returns the shared [`PluginStorage`] backing the WIT `storage`
    /// import for every plugin loaded from `plugin_dir`, opening and
    /// caching it on first use (a `plugin_dir` this loader hasn't seen
    /// before), and simply returning the cached handle on every call
    /// after that — matching `compile_and_link`'s own cache-then-reuse
    /// shape for `component_cache`, including *not* holding the mutex
    /// during the actual (I/O-bound) open: released after the initial
    /// miss check, re-acquired only to insert, so two threads racing to
    /// open the same never-before-seen `plugin_dir` concurrently don't
    /// serialize behind one lock for the whole open — the same tolerated
    /// "whichever insert wins" race `component_cache` already accepts for
    /// a concurrent first compile of the same file.
    ///
    /// `plugin_dir` here is always a loaded plugin's own file's parent
    /// directory (see [`load_plugin`](Self::load_plugin)'s call site),
    /// which is the same path as the `plugin_dir` passed to
    /// [`load_plugins`]/[`load_plugins_from_dir`](Self::load_plugins_from_dir)
    /// in every real call path in this crate — installs place `.wasm`
    /// files flat, directly in `plugin_dir`, with no subdirectories —
    /// true for both single-file installs and pack members, which are
    /// placed flat in the same directory as single-file installs.
    ///
    /// A failure to open the database (e.g. a read-only `plugin_dir`) is
    /// logged once per distinct `plugin_dir` and does not fail plugin
    /// loading — the affected [`WasmPluginNode`]s simply get no storage
    /// handle, and every `storage.*` guest call degrades per
    /// `wit/node.wit`'s own doc comment on `interface storage` instead of
    /// blocking the plugin from loading and running otherwise.
    fn plugin_storage(&self, plugin_dir: &Path) -> Option<Arc<PluginStorage>> {
        if let Some(existing) = self
            .storage_pools
            .lock()
            .expect("storage pool mutex poisoned")
            .get(plugin_dir)
        {
            return Some(Arc::clone(existing));
        }

        match PluginStorage::open(plugin_dir) {
            Ok(db) => {
                let db = Arc::new(db);
                self.storage_pools
                    .lock()
                    .expect("storage pool mutex poisoned")
                    .insert(plugin_dir.to_path_buf(), Arc::clone(&db));
                Some(db)
            }
            Err(e) => {
                tracing::warn!(
                    "plugin_loader: failed to open plugin storage database in {}: {} — \
                     plugins loaded from this directory get no `storage` import backing",
                    plugin_dir.display(),
                    e
                );
                None
            }
        }
    }

    /// Number of real (cache-miss) compiles performed by this instance so
    /// far. Test-only — see the `compile_count` field doc above.
    #[cfg(test)]
    fn compile_count(&self) -> usize {
        self.compile_count.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Real (cache-miss) compiles against `trigger_component_cache` so far
    /// -- see the `trigger_compile_count` field doc above. Test-only, same
    /// rationale as `compile_count` above.
    #[cfg(test)]
    fn trigger_compile_count(&self) -> usize {
        self.trigger_compile_count.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Real (cache-miss) signature checks performed by this instance so far
    /// -- see the `signature_check_count` field doc above. Test-only, same
    /// rationale as `compile_count` above.
    #[cfg(test)]
    fn signature_check_count(&self) -> usize {
        self.signature_check_count.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Drops every cached component and signature result held for `path`.
    /// For a file that was only read once, such as a staged install, so its
    /// compiled code is not kept for the rest of the process.
    pub fn forget_path(&self, path: &Path) {
        self.component_cache.lock().unwrap_or_else(|e| e.into_inner()).remove(path);
        self.trigger_component_cache.lock().unwrap_or_else(|e| e.into_inner()).remove(path);
        self.signature_cache.lock().unwrap_or_else(|e| e.into_inner()).remove(path);
    }

    /// Returns the process-wide shared [`PluginLoader`], constructing it on
    /// the first call and reusing it — and its one `Engine` and one
    /// epoch-ticker thread — for every later call, from any caller, for the
    /// rest of the process's lifetime.
    ///
    /// If [`PluginLoader::new`] fails (e.g. unsupported CPU features on the
    /// host), the error is cached and returned again on every subsequent
    /// call rather than retried — a construction failure here reflects a
    /// host/environment condition that will not change between calls within
    /// the same process.
    pub fn shared() -> Result<&'static PluginLoader, String> {
        SHARED_LOADER
            .get_or_init(|| PluginLoader::new().map_err(|e| e.to_string()))
            .as_ref()
            .map_err(|e| e.clone())
    }
}
