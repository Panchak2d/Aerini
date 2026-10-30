use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use crate::plugin_loader::bindings::wit_trigger;
use crate::plugin_loader::error::PluginLoadError;
use crate::plugin_loader::fs_watch::{FsWatchScope, PluginFsWatchHandle};
use crate::plugin_loader::limits::{PLUGIN_TRIGGER_CALL_TIMEOUT, TRIGGER_SCAN_LIMIT};
use crate::plugin_loader::loader::load::is_wasm_file_name;
use crate::plugin_loader::storage::PluginStorageHandle;
use crate::plugin_loader::trigger::{NextFireWatch, TriggerPre};
use crate::plugin_loader::trigger::describe::instantiate_and_describe_trigger;
use crate::plugin_loader::trigger::execute::trigger_linker;
use crate::plugin_loader::trigger::stall::{install_stall_watch, stall_message, StallWatch};
use crate::plugin_loader::trigger::state::{make_trigger_plugin_state, make_trigger_store, TriggerPluginState};
use crate::plugin_loader::trigger::stream::TriggerEventConsumer;
use super::{CachedComponent, PluginLoader};

impl PluginLoader {
    /// Cache-only half of the trigger-engine compile path -- `get` (with the
    /// fingerprint filter) and `insert`, factored out so the async and sync
    /// compile entry points below share one lock/get/insert implementation
    /// and can't drift apart on caching semantics even though they differ
    /// in how they get from "cache miss" to "compiled `Component`".
    fn cached_trigger_component(
        &self,
        path: &Path,
        fingerprint: (Option<SystemTime>, u64),
    ) -> Option<wasmtime::component::Component> {
        self.trigger_component_cache
            .lock()
            .expect("trigger component cache mutex poisoned")
            .get(path)
            .filter(|entry| entry.fingerprint == fingerprint)
            .map(|entry| entry.component.clone())
    }

    fn insert_trigger_component(
        &self,
        path: &Path,
        fingerprint: (Option<SystemTime>, u64),
        component: wasmtime::component::Component,
    ) {
        self.trigger_component_cache
            .lock()
            .expect("trigger component cache mutex poisoned")
            .insert(path.to_path_buf(), CachedComponent { fingerprint, component });
        self.trigger_compile_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    /// Trigger-engine counterpart to the read+compile+cache half of
    /// `compile_and_link` -- same fingerprint-checked cache contract, against
    /// `trigger_component_cache`/`trigger_engine` instead of
    /// `component_cache`/`engine`. Stops short of linking: unlike the
    /// sync path, a trigger-plugin instantiation attempt is inherently
    /// speculative (any given `.wasm` file in the directory may not export
    /// `aerini-node-with-trigger` at all), so the linker + `instantiate_async`
    /// step lives in `resolve_trigger_instance` right where that outcome is
    /// decided, not bundled in here.
    ///
    /// For [`resolve_trigger_instance`](Self::resolve_trigger_instance)'s
    /// already-async call path, run concurrently with every other trigger's
    /// live event stream on the same runtime: a cache miss offloads the
    /// actual `read` + `Component::new` (CPU-bound, can take tens of
    /// milliseconds for a real component) onto the blocking thread pool via
    /// `spawn_blocking`, cloning `trigger_engine` into the closure (cheap --
    /// internally `Arc`-backed) rather than borrowing `self`, so compiling
    /// one trigger plugin never stalls the async reactor thread everything
    /// else is scheduled on. The cache lookup and insert stay on the
    /// calling task either side of the offloaded work -- both are fast and
    /// don't need to move. See
    /// [`compile_trigger_component_sync`](Self::compile_trigger_component_sync)
    /// for the counterpart [`load_plugin`](Self::load_plugin)'s synchronous
    /// call path uses instead, since it cannot `.await` this one.
    pub(super) async fn compile_trigger_component(&self, path: &Path) -> Result<wasmtime::component::Component, PluginLoadError> {
        let meta = std::fs::metadata(path)?;
        let fingerprint = (meta.modified().ok(), meta.len());

        if let Some(c) = self.cached_trigger_component(path, fingerprint) {
            return Ok(c);
        }

        let engine = self.trigger_engine.clone();
        let owned_path = path.to_path_buf();
        let component = tokio::task::spawn_blocking(move || -> Result<wasmtime::component::Component, PluginLoadError> {
            let bytes = std::fs::read(&owned_path)?;
            wasmtime::component::Component::new(&engine, &bytes)
                .map_err(|e| PluginLoadError::WasmCompile(e.to_string()))
        })
        .await
        .map_err(|e| PluginLoadError::WasmCompile(format!("compile task panicked: {e}")))??;

        self.insert_trigger_component(path, fingerprint, component.clone());

        Ok(component)
    }

    /// Synchronous counterpart to
    /// [`compile_trigger_component`](Self::compile_trigger_component),
    /// sharing its cache via [`cached_trigger_component`](Self::cached_trigger_component)/
    /// [`insert_trigger_component`](Self::insert_trigger_component). Used
    /// only by [`probe_trigger_capable`](Self::probe_trigger_capable),
    /// reached from [`load_plugin`](Self::load_plugin)'s synchronous public
    /// signature -- kept synchronous because that signature is depended on
    /// by a caller that is itself not `async` (the desktop app's startup
    /// sequence). Not `spawn_blocking`-wrapped: there is no async reactor to
    /// protect here, the same accepted trade-off `compile_and_link`'s own
    /// synchronous read+compile already makes for ordinary action plugins.
    pub(super) fn compile_trigger_component_sync(&self, path: &Path) -> Result<wasmtime::component::Component, PluginLoadError> {
        let meta = std::fs::metadata(path)?;
        let fingerprint = (meta.modified().ok(), meta.len());

        if let Some(c) = self.cached_trigger_component(path, fingerprint) {
            return Ok(c);
        }

        let bytes = std::fs::read(path)?;
        let component = wasmtime::component::Component::new(&self.trigger_engine, &bytes)
            .map_err(|e| PluginLoadError::WasmCompile(e.to_string()))?;

        self.insert_trigger_component(path, fingerprint, component.clone());

        Ok(component)
    }

    /// Type-level check for whether the `.wasm` file at `path` exports
    /// `aerini-node-with-trigger`, run once at [`load_plugin`](Self::load_plugin)
    /// time. Reuses `compile_trigger_component_sync`'s cache -- the same
    /// `trigger_component_cache` a later `resolve_trigger_instance` call for
    /// this same file will also hit. The import check against a throwaway
    /// linker and `AeriniNodeWithTriggerPre::new` are both type-level only
    /// -- no guest code executes, unlike `resolve_trigger_instance`'s
    /// `instantiate_async` + `describe()` call. Links the same import
    /// surface `resolve_trigger_instance` links (`wasi:p3` + `wasi:http` p3 +
    /// `storage`) so a plugin that imports `http` or `storage` isn't
    /// undercounted here relative to what actually resolves later.
    pub(super) fn probe_trigger_capable(&self, path: &Path) -> bool {
        self.trigger_pre(path).is_ok()
    }

    /// Scans `dir` for a `.wasm` file exporting `aerini-node-with-trigger`
    /// whose `describe().type_id` matches `type_id`, and returns it
    /// instantiated and ready for `events()` to be called on it.
    ///
    /// A file whose signature sidecar is bad is skipped before it is compiled
    /// or run. Each file gets [`PLUGIN_TRIGGER_CALL_TIMEOUT`] to instantiate and
    /// `describe()`, and no further file is started once the scan has run for
    /// [`TRIGGER_SCAN_LIMIT`]; a file that times out, or a scan cut short, is
    /// named in [`PluginLoadError::TriggerScanIncomplete`].
    ///
    /// Every file that fails to compile or link against `trigger_engine` is
    /// skipped, not logged as a warning -- unlike `load_plugins_from_dir`'s
    /// directory scan, a "miss" here is the expected outcome for the
    /// majority of files in a plugin directory (every action-only plugin,
    /// built against WASIp2, cannot satisfy `wasmtime_wasi::p3`'s linker
    /// imports and will fail here every time this is called; that is not a
    /// problem to surface, just a non-match).
    pub(super) async fn resolve_trigger_instance(
        &self,
        dir: &Path,
        type_id: &str,
        sandbox_root: Option<&Path>,
    ) -> Result<(wasmtime::Store<TriggerPluginState>, wit_trigger::AeriniNodeWithTrigger), PluginLoadError> {
        let entries = std::fs::read_dir(dir).map_err(PluginLoadError::Io)?;

        // Looked up once per resolve, not once per candidate file: every
        // store this loop builds is trying to match this same `type_id`,
        // so one handle (or `None`, if `dir`'s storage database couldn't be
        // opened) covers every candidate -- same scoping convention as
        // `WasmPluginNode::execute`'s own `storage_handle` (scoped by
        // `type_id`, not by placement).
        let storage_handle = self.plugin_storage(dir).map(|db| PluginStorageHandle {
            db,
            scope: type_id.to_string(),
        });
        // Same "looked up once per resolve" reasoning as `storage_handle`
        // above -- `FsWatchRegistry` construction cannot fail, so this is
        // always `Some`, unlike `storage_handle`.
        let fs_watch_handle = Some(PluginFsWatchHandle {
            registry: self.fs_watch_registry.clone(),
            plugin_type_id: type_id.to_string(),
            scope: FsWatchScope::from_root(sandbox_root),
        });

        let scan_started = std::time::Instant::now();
        let mut incomplete: Vec<String> = Vec::new();

        for entry in entries {
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if !is_wasm_file_name(&path) {
                continue;
            }

            if scan_started.elapsed() >= TRIGGER_SCAN_LIMIT {
                incomplete.push(format!(
                    "scan stopped after the {}s limit before every file was tried",
                    TRIGGER_SCAN_LIMIT.as_secs()
                ));
                break;
            }

            if self.require_acceptable_signature(&path).is_err() {
                continue;
            }

            let Ok(component) = self.compile_trigger_component(&path).await else { continue };
            let Ok(linker) = trigger_linker(&self.trigger_engine) else { continue };
            let Ok(instance_pre) = linker.instantiate_pre(&component) else { continue };
            let Ok(pre) = TriggerPre::new(instance_pre) else { continue };

            let mut store = make_trigger_store(
                &self.trigger_engine,
                make_trigger_plugin_state(storage_handle.clone(), fs_watch_handle.clone()),
            );

            // `describe` is a plain (non-`async`) WIT function, but this store
            // has async-typed imports linked, so it must still be called through
            // `call_async`: the sync `call_describe` rejects such a store.
            let (bindings, raw) = match instantiate_and_describe_trigger(&pre, &mut store).await {
                Ok(v) => v,
                Err(PluginLoadError::CallTimedOut(secs)) => {
                    let name = path
                        .file_name()
                        .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
                    incomplete.push(format!("{name} (did not finish within the {secs}s per-file limit)"));
                    continue;
                }
                Err(_) => continue,
            };

            if raw.type_id == type_id {
                return Ok((store, bindings));
            }
        }

        if incomplete.is_empty() {
            Err(PluginLoadError::NoSuchTriggerPlugin(type_id.to_string()))
        } else {
            Err(PluginLoadError::TriggerScanIncomplete {
                type_id: type_id.to_string(),
                detail: incomplete.join("; "),
            })
        }
    }

    /// Resolves `type_id` to a trigger-capable plugin under `dir`, starts its
    /// event stream (`trigger.events(config)`, called exactly once, per
    /// `wit/node.wit`'s own contract), and hands events back as plain JSON
    /// strings (`trigger-event.data`) over a channel -- no wasmtime or
    /// `bindgen!`-generated type crosses this boundary.
    ///
    /// The event pump runs on its own spawned task, returned alongside the
    /// receiver so the caller can bind its lifetime to whatever owns the
    /// receiver (e.g. abort it together with the job task that's draining
    /// the channel) -- this function does not itself decide when the stream
    /// should stop being read.
    ///
    /// The channel yields `Err(message)` for a single read failure (the
    /// plugin instance is left as-is; a further `recv()` may still succeed)
    /// and closes (`recv()` returns `None`) when the guest closes the
    /// stream or the instance traps -- the caller decides whether to treat
    /// closure as fatal for this job (see `scheduler/runner.rs`).
    ///
    /// Once `events()` has returned, a guest that executes for
    /// [`PLUGIN_TRIGGER_STALL_LIMIT`] without a host call or a delivered event
    /// is stopped and reported as an `Err` (see [`StallTracker`]); a trigger
    /// idle between events is unaffected.
    ///
    /// The third element carries the latest next-fire time the guest reported
    /// through `trigger-schedule` (validated by [`validate_next_fire`]), or
    /// `None` if it has reported none. It is created fresh for each call, so a
    /// restarted instance never inherits the previous instance's value, and it
    /// is display-only: nothing here reads it to decide when to run.
    pub async fn start_trigger(
        &self,
        dir: &Path,
        type_id: &str,
        config: &str,
        sandbox_root: Option<&Path>,
    ) -> Result<(
        tokio::task::JoinHandle<()>,
        tokio::sync::mpsc::Receiver<Result<String, String>>,
        NextFireWatch,
    ), PluginLoadError> {
        let (mut store, bindings) = self.resolve_trigger_instance(dir, type_id, sandbox_root).await?;
        let (next_fire_tx, next_fire_rx) = tokio::sync::watch::channel(None);
        store.data_mut().next_fire = Some(next_fire_tx);
        let watch = install_stall_watch(&mut store);
        let activity = Arc::clone(&store.data().activity);
        let config = config.to_string();
        let (tx, rx) = tokio::sync::mpsc::channel(32);

        let handle = tokio::spawn(async move {
            let StallWatch { armed, stalled, signal } = watch;
            let pump_tx = tx.clone();
            let run = store.run_concurrent(async move |accessor| -> wasmtime::Result<()> {
                let events_call = bindings.aerini_plugin_trigger().call_events(accessor, config);
                let reader = match tokio::time::timeout(PLUGIN_TRIGGER_CALL_TIMEOUT, events_call).await {
                    Ok(Ok(r)) => r,
                    Ok(Err(e)) => {
                        let _ = pump_tx.send(Err(e.to_string())).await;
                        return Ok(());
                    }
                    Err(_elapsed) => {
                        let _ = pump_tx.send(Err(format!(
                            "events() did not return within {}s",
                            PLUGIN_TRIGGER_CALL_TIMEOUT.as_secs()
                        ))).await;
                        return Ok(());
                    }
                };
                armed.store(true, std::sync::atomic::Ordering::Release);

                let consumer = TriggerEventConsumer {
                    tx: tokio_util::sync::PollSender::new(pump_tx.clone()),
                    activity,
                };
                if let Err(e) = accessor.with(|access| reader.pipe(access, consumer)) {
                    let _ = pump_tx.send(Err(e.to_string())).await;
                }
                Ok(())
            });

            tokio::select! {
                biased;
                outcome = run => {
                    if stalled.load(std::sync::atomic::Ordering::Acquire) {
                        let _ = tx.send(Err(stall_message())).await;
                    } else if let Err(e) = outcome.and_then(|inner| inner) {
                        let _ = tx.send(Err(e.to_string())).await;
                    }
                }
                _ = signal.notified() => {
                    let _ = tx.send(Err(stall_message())).await;
                }
            }
        });

        Ok((handle, rx, next_fire_rx))
    }
}
