use std::path::Path;

use crate::plugin_loader::bindings::{wit, wit_fs_watch, wit_metadata, wit_storage};
use crate::plugin_loader::describe::{describe_via_pre, NodePre, RawDescribe};
use crate::plugin_loader::error::PluginLoadError;
use crate::plugin_loader::node::NodeBackend;
use crate::plugin_loader::state::{make_plugin_state, make_store, PluginState};
use crate::plugin_loader::trigger::TriggerPre;
use crate::plugin_loader::trigger::describe::{block_on_scratch_runtime, describe_trigger_for_load};
use crate::plugin_loader::trigger::execute::trigger_linker;
use crate::plugin_loader::trigger::state::{make_trigger_plugin_state, make_trigger_store};
use super::{CachedComponent, PluginLoader};

/// What [`PluginLoader::compile_and_link`] produced for a file.
enum LinkedPlugin {
    /// Every import is satisfied on the synchronous engine.
    Sync(wasmtime::component::InstancePre<PluginState>),
    /// Only satisfiable on `trigger_engine` (the component imports async-typed
    /// WASI 0.3 interfaces) and exports `aerini-node-with-trigger`.
    Trigger(TriggerPre),
}

impl PluginLoader {
    /// Steps shared by [`load_plugin`](Self::load_plugin) and
    /// [`describe_plugin`](Self::describe_plugin): read the file, compile it
    /// to a component, build a linker with the full WASIp2 + HTTP surface,
    /// and pre-instantiate. Does not validate which world is exported --
    /// that's [`detect_node_pre`](Self::detect_node_pre), since both call
    /// paths need to try the same `InstancePre` against two possible world
    /// shapes. Extracted so the two call paths — one that goes on to leak
    /// `'static` strings and register a live `Node`, one that doesn't —
    /// can't drift apart on the compile/link logic itself.
    fn compile_and_link(&self, path: &Path) -> Result<LinkedPlugin, PluginLoadError> {
        // Steps 1-2: read + compile — or reuse a cached `Component` for this
        // exact path if its (mtime, len) fingerprint matches what's cached.
        // `std::fs::metadata` (not `std::fs::read`) is the first fallible
        // call so a missing/unreadable file still produces `Io`, not
        // `WasmCompile` — matching the pre-cache error classification the
        // existing tests assert on. A changed fingerprint (file replaced,
        // e.g. via remove+reinstall under the same filename) forces a fresh
        // compile rather than serving stale bytes.
        let meta = std::fs::metadata(path)?;
        let fingerprint = (meta.modified().ok(), meta.len());

        let cached = self
            .component_cache
            .lock()
            .expect("component cache mutex poisoned")
            .get(path)
            .filter(|entry| entry.fingerprint == fingerprint)
            .map(|entry| entry.component.clone());

        let component = match cached {
            Some(c) => c,
            None => {
                // Step 1: read — produces Io error (not WasmCompile) for missing files.
                let bytes = std::fs::read(path)?;

                // Step 2: compile. Accepts WAT text or WASM binary — wasmtime detects by magic.
                let component = wasmtime::component::Component::new(&self.engine, &bytes)
                    .map_err(|e| PluginLoadError::WasmCompile(e.to_string()))?;

                self.compile_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

                self.component_cache
                    .lock()
                    .expect("component cache mutex poisoned")
                    .insert(path.to_path_buf(), CachedComponent { fingerprint, component: component.clone() });

                component
            }
        };

        // Step 3: linker + pre-instantiation.
        //
        // Four calls are required:
        // (a) `wasmtime_wasi::p2::add_to_linker_sync` — satisfies all `wasi:cli/command`
        //     imports (filesystem, random, clocks, stdio) that `wasm32-wasip2` binaries
        //     unconditionally import via Rust's standard library.
        // (b) `wasmtime_wasi_http::p2::add_only_http_to_linker_sync` — adds only the
        //     `wasi:http/outgoing-handler` interface without duplicating the WASI
        //     interfaces already registered in (a).
        // (c) this package's own `storage` import (below) — see its own comment.
        // (d) this package's own `fs-watch` import (below) — see its own comment.
        //
        // Using just `wasmtime_wasi_http::p2::add_to_linker_sync` (the proxy world bundle)
        // instead of (a)+(b) would omit `wasi:filesystem`, `wasi:random`, etc., causing
        // `linker.instantiate_pre` to fail for every standard `wasm32-wasip2` plugin.
        let mut linker = wasmtime::component::Linker::<PluginState>::new(&self.engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
            .map_err(|e| PluginLoadError::WasmLink(e.to_string()))?;
        wasmtime_wasi_http::p2::add_only_http_to_linker_sync(&mut linker)
            .map_err(|e| PluginLoadError::WasmLink(e.to_string()))?;

        // (c) the `storage` import (`PluginState` implements its generated
        // `Host` trait directly, so `HasSelf<PluginState>` — the "my data
        // IS &mut T" convenience `HasData` impl — is the right `D` here,
        // matching `wasmtime_wasi::p3::bindings`' own documented example
        // for a plain custom host-implemented interface). Linked
        // unconditionally, like (a)/(b) above: a plugin compiled before
        // `storage` existed has no such import to satisfy, so this is a
        // no-op for it, not a compatibility risk.
        wit_storage::add_to_linker::<PluginState, wasmtime::component::HasSelf<PluginState>>(
            &mut linker,
            |state| state,
        ).map_err(|e| PluginLoadError::WasmLink(e.to_string()))?;

        // (d) the `fs-watch` import -- same unconditional-link rationale as
        // (c) above; a plugin compiled before `fs-watch` existed has no
        // such import to satisfy, so this changes nothing for it.
        wit_fs_watch::add_to_linker::<PluginState, wasmtime::component::HasSelf<PluginState>>(
            &mut linker,
            |state| state,
        ).map_err(|e| PluginLoadError::WasmLink(e.to_string()))?;

        match linker.instantiate_pre(&component) {
            Ok(pre) => Ok(LinkedPlugin::Sync(pre)),
            // A plugin built with `wasip3` imports async-typed WASI 0.3
            // interfaces, which a store on the synchronous engine can never
            // host. If the component links and exports the trigger world on
            // `trigger_engine`, it is served from there instead; anything else
            // keeps the strict sync-link error.
            Err(sync_err) => match self.trigger_pre(path) {
                Ok(pre) => Ok(LinkedPlugin::Trigger(pre)),
                Err(_) => Err(PluginLoadError::WasmLink(sync_err.to_string())),
            },
        }
    }

    /// Compiles `path` for `trigger_engine` (cached) and returns its
    /// pre-instantiated trigger bindings, or the reason it isn't one. Type-level
    /// only: no guest code runs.
    pub(super) fn trigger_pre(&self, path: &Path) -> Result<TriggerPre, String> {
        let component = self.compile_trigger_component_sync(path).map_err(|e| e.to_string())?;
        let linker = trigger_linker(&self.trigger_engine)?;
        let instance_pre = linker.instantiate_pre(&component).map_err(|e| e.to_string())?;
        TriggerPre::new(instance_pre).map_err(|e| e.to_string())
    }

    /// Links `path` and calls `describe()` once, on whichever engine the
    /// component can be hosted on, returning the result with the backend
    /// `execute()` must use. Shared by `load_plugin` and `describe_plugin` so
    /// the two can't drift apart.
    pub(super) fn describe_linked(&self, path: &Path) -> Result<(RawDescribe, NodeBackend), PluginLoadError> {
        match self.compile_and_link(path)? {
            LinkedPlugin::Sync(instance_pre) => {
                let node_pre = self.detect_node_pre(instance_pre.clone())?;

                // Runtime `execute()` always goes through the plain `node`-only Pre --
                // every valid plugin exports it, with or without `metadata`. Cannot
                // fail: `detect_node_pre` above already proved `node` is exported by
                // this exact component.
                let exec_pre = wit::AeriniNodePre::new(instance_pre)
                    .expect("node export already confirmed by detect_node_pre");

                let mut store = make_store(&self.engine, make_plugin_state(None, None));
                let raw = describe_via_pre(&node_pre, &mut store)?;
                Ok((raw, NodeBackend::Sync { engine: self.engine.clone(), pre: exec_pre }))
            }
            LinkedPlugin::Trigger(pre) => {
                let pre_ref = &pre;
                let raw = block_on_scratch_runtime(move || async move {
                    let mut store = make_trigger_store(
                        pre_ref.engine(),
                        make_trigger_plugin_state(None, None),
                    );
                    describe_trigger_for_load(pre_ref, &mut store).await
                })?;
                Ok((raw, NodeBackend::Trigger { pre }))
            }
        }
    }

    /// Determines which world `instance_pre` exports and returns the matching
    /// bindings, trying the superset (`aerini-node-with-metadata`) first and
    /// falling back to the plain `aerini-node` world -- per `wit/node.wit`'s
    /// own doc comment, both are permanent, equally-valid shapes, not a
    /// migration path. `InstancePre::new` on the `bindgen!`-generated `*Pre`
    /// types is a type-level check against the component's export signature;
    /// it runs no guest code, so trying the superset first and falling back
    /// costs nothing beyond the check itself.
    fn detect_node_pre(
        &self,
        instance_pre: wasmtime::component::InstancePre<PluginState>,
    ) -> Result<NodePre, PluginLoadError> {
        match wit_metadata::AeriniNodeWithMetadataPre::new(instance_pre.clone()) {
            Ok(pre) => Ok(NodePre::WithMetadata(pre)),
            Err(_) => wit::AeriniNodePre::new(instance_pre)
                .map(NodePre::Plain)
                .map_err(|_| PluginLoadError::MissingInterface),
        }
    }
}
