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
//! Each Store is limited to [`PLUGIN_MEMORY_LIMIT`] bytes (64 MiB by default).
//! The limit is enforced by [`wasmtime::StoreLimitsBuilder`] via
//! [`wasmtime::Store::limiter`].
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
//! Database and HTTP nodes enforce — before `wasmtime_wasi_http`'s default send
//! path is allowed to run it. Action plugins go through
//! [`PluginHttpHooks::send_request`] (`wasi:http` p2); trigger plugins go through
//! [`TriggerHttpHooks::send_request`] (p3, `trigger_engine`'s async ABI) — both
//! delegate the actual policy to the same [`check_ssrf_uri`], so the two engines
//! can't drift apart on what's blocked. See that function for exactly what is and
//! isn't caught.
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
//! interface alongside `node`. It is never used for the `node`/`describe`/`execute`
//! path above, which stays fully synchronous; see `wit/node.wit`'s doc comment on
//! `interface trigger` for why the two can't share one engine. Trigger plugins get
//! SSRF-filtered `wasi:http` (p3) linked the same way action plugins get p2 — see
//! "Outbound HTTP" above — but no raw-socket grant, the same effective position
//! action plugins are in absent an explicit grant. See [`PluginLoader::start_trigger`].

use std::collections::HashMap;
use std::net::ToSocketAddrs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use async_trait::async_trait;
use bytes::Bytes;
use http_body_util::combinators::UnsyncBoxBody;
use hyper::Request;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::OptionalExtension;
use serde_json::Value;
use wasmtime::component::{ResourceTable, Source, StreamConsumer, StreamResult};
use wasmtime::{Config, Engine, StoreLimitsBuilder};
use std::pin::Pin;
use std::task::{Context, Poll};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};
use wasmtime_wasi_http::p2::bindings::http::types::ErrorCode;
use wasmtime_wasi_http::p2::body::HyperOutgoingBody;
use wasmtime_wasi_http::p2::types::{HostFutureIncomingResponse, OutgoingRequestConfig};
use wasmtime_wasi_http::{WasiHttpCtx, p2::{HttpResult, WasiHttpCtxView, WasiHttpHooks, WasiHttpView, default_send_request}};
use wasmtime_wasi_http::p3::bindings::http::types::ErrorCode as P3ErrorCode;
use wasmtime_wasi_http::p3::{
    RequestOptions as P3RequestOptions,
    WasiHttpCtxView as P3WasiHttpCtxView,
    WasiHttpHooks as P3WasiHttpHooks,
    WasiHttpView as P3WasiHttpView,
    default_send_request as p3_default_send_request,
};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodeRegistry};
use crate::nodes::util::{SsrfPolicy, check_ssrf_ip};

// ── WIT bindings ──────────────────────────────────────────────────────────────

/// Bindings generated by `bindgen!` from `wit/node.wit`.
///
/// Wrapped in a private module because `bindgen!` generates types named `NodeInput`
/// and `NodeOutput` that would collide with the identically named types in
/// [`crate::model`] if expanded at the top level.
///
/// The `pub use` re-exports normalize the type path to `wit::Param`, `wit::NodeInput`,
/// etc. `bindgen!` places these types under the full package path
/// (`aerini::plugin::types`), not at the top level.
mod wit {
    wasmtime::component::bindgen!({
        world: "aerini-node",
        path: "wit",
    });

    pub use aerini::plugin::types::{NodeInput, NodeOutput, Param};
}

/// Bindings for the optional `aerini-node-with-metadata` world (`node` + `metadata`
/// exports). Generated separately from `wit` above because `bindgen!` cannot express
/// "this world's exports are a superset of that one" -- the two worlds get
/// independent, structurally-identical Rust types. Only `AeriniNodeWithMetadataPre`
/// and the `describe-metadata` call are used from here; `execute`/`describe` always
/// go through `wit::AeriniNodePre`, which every plugin -- with or without metadata --
/// satisfies.
mod wit_metadata {
    wasmtime::component::bindgen!({
        world: "aerini-node-with-metadata",
        path: "wit",
    });
}

/// Bindings for the `aerini-node-with-trigger` world (`node` + `trigger`
/// exports), generated with `bindgen!`'s async support auto-enabled by the
/// presence of `trigger.events`'s `async func` in the WIT source -- no
/// explicit `async: true` option is needed in the macro invocation itself.
///
/// Kept entirely separate from [`wit`]/[`wit_metadata`] above: those are
/// only ever instantiated on [`PluginLoader`]'s synchronous `engine`, this
/// one only ever on `PluginLoader::trigger_engine` -- see `wit/node.wit`'s
/// own doc comment on `interface trigger` for why the same plugin file is
/// compiled and instantiated separately per engine rather than once.
/// Nothing from this module is re-exported at the file level (unlike
/// `wit`'s `pub use`): every caller goes through [`PluginLoader::start_trigger`]
/// and the plain [`String`] events it yields, never these generated types
/// directly.
mod wit_trigger {
    wasmtime::component::bindgen!({
        world: "aerini-node-with-trigger",
        path: "wit",
    });
}

/// Bindings for the `aerini-node-with-storage` world (`node` export +
/// `storage` import), used only for the `storage::Host` trait and its
/// generated `add_to_linker` -- the `node`-export half of this world's
/// generated bindings goes unused here since `execute`/`describe` always
/// go through [`wit::AeriniNodePre`] regardless of which world a given
/// plugin's own build actually targeted (see `wit/node.wit`'s own doc
/// comment on `interface storage` for why an import needs no such
/// per-world detection: the linker satisfies whatever a compiled
/// component actually imports, not what a nominal "world" could import).
/// `storage` is linked into every plugin's [`Linker`](wasmtime::component::Linker)
/// unconditionally in [`PluginLoader::compile_and_link`] -- a plugin
/// compiled before this existed simply has no such import to satisfy, so
/// linking it in changes nothing for already-published plugins.
mod wit_storage {
    wasmtime::component::bindgen!({
        world: "aerini-node-with-storage",
        path: "wit",
    });

    pub use aerini::plugin::storage::{add_to_linker, Host as StorageHost, StorageError};
}

/// A plugin's pre-instantiated bindings, in whichever of the two `aerini:plugin`
/// world shapes it actually exports. Produced by
/// [`PluginLoader::detect_node_pre`](PluginLoader::detect_node_pre).
enum NodePre {
    /// Exports only `node` (`describe`/`execute`) -- the pre-`metadata` shape.
    Plain(wit::AeriniNodePre<PluginState>),
    /// Exports `node` and `metadata` (`describe-metadata`).
    WithMetadata(wit_metadata::AeriniNodeWithMetadataPre<PluginState>),
}

/// World-agnostic result of calling `describe()` (and `describe-metadata()`, when
/// the plugin exports it) once against an instantiated plugin.
struct RawDescribe {
    type_id: String,
    display_name: String,
    category: String,
    description: String,
    input_schema: String,
    output_schema: String,
    /// Empty when the plugin doesn't export `metadata`, or exports it but left
    /// this field unset -- both mean "use the host default" (Constraint 1 /
    /// `wit/node.wit`'s doc comment).
    version: String,
    author: String,
    icon: String,
}

/// Instantiates `node_pre` and extracts everything callers need from `describe()`
/// (and `describe-metadata()`, when the world supports it) into one owned,
/// world-agnostic result. `describe()` is called through `node_pre`'s own bindings
/// rather than a separately-constructed plain `AeriniNodePre` so a `WithMetadata`
/// plugin is instantiated exactly once for this call, not twice.
fn describe_via_pre(
    node_pre: &NodePre,
    store: &mut wasmtime::Store<PluginState>,
) -> Result<RawDescribe, PluginLoadError> {
    match node_pre {
        NodePre::Plain(pre) => {
            let bindings = pre.instantiate(&mut *store)
                .map_err(|e| PluginLoadError::WasmLink(e.to_string()))?;
            let d = bindings.aerini_plugin_node().call_describe(&mut *store)
                .map_err(|e| PluginLoadError::WasmLink(e.to_string()))?;
            Ok(RawDescribe {
                type_id: d.type_id,
                display_name: d.display_name,
                category: d.category,
                description: d.description,
                input_schema: d.input_schema,
                output_schema: d.output_schema,
                version: String::new(),
                author: String::new(),
                icon: String::new(),
            })
        }
        NodePre::WithMetadata(pre) => {
            let bindings = pre.instantiate(&mut *store)
                .map_err(|e| PluginLoadError::WasmLink(e.to_string()))?;
            let d = bindings.aerini_plugin_node().call_describe(&mut *store)
                .map_err(|e| PluginLoadError::WasmLink(e.to_string()))?;
            let m = bindings.aerini_plugin_metadata().call_describe_metadata(&mut *store)
                .map_err(|e| PluginLoadError::WasmLink(e.to_string()))?;
            Ok(RawDescribe {
                type_id: d.type_id,
                display_name: d.display_name,
                category: d.category,
                description: d.description,
                input_schema: d.input_schema,
                output_schema: d.output_schema,
                version: m.version,
                author: m.author,
                icon: m.icon,
            })
        }
    }
}

// ── Error type ────────────────────────────────────────────────────────────────

/// Errors that can occur while loading a WASM plugin.
#[derive(Debug, thiserror::Error)]
pub enum PluginLoadError {
    #[error("I/O error reading plugin: {0}")]
    Io(#[from] std::io::Error),

    #[error("WASM component compilation failed: {0}")]
    WasmCompile(String),

    #[error("WASM linker setup or pre-instantiation failed: {0}")]
    WasmLink(String),

    #[error("plugin does not export the `aerini-node` world")]
    MissingInterface,

    #[error("no plugin exporting `aerini-node-with-trigger` with type_id \"{0}\" found in the plugin directory")]
    NoSuchTriggerPlugin(String),

    #[error("plugin signature check failed: {0}")]
    SignatureRejected(String),

    /// Kept for API compatibility; not returned by any current code path.
    #[error("not yet implemented")]
    #[allow(dead_code)]
    NotImplemented,
}

// ── Plugin signature verification ───────────────────────────────────────────

/// Publisher signature verification for a single `.wasm` file against its
/// optional `<name>.wasm.sig` sidecar. The single load-time enforcement
/// point every [`load_plugins`] caller shares — see
/// [`PluginLoader::load_plugin`]. Also reused by `src-tauri` for its
/// desktop-only trust store and multi-file pack signing, which have no
/// equivalent here (`aerini-engine` has no concept of a "pack").
pub mod signature {
    use std::path::{Path, PathBuf};

    use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
    use ed25519_dalek::{Signature, VerifyingKey};
    use serde::Deserialize;

    pub const SIG_SCHEMA_VERSION: u32 = 1;
    pub const SIG_ALGORITHM: &str = "ed25519";
    const SIG_DOMAIN: &[u8] = b"aerini-plugin-sig-v1\n";

    /// One `{name, blake3}` entry inside a [`PluginSignatureFile`]. A list
    /// (not a single hash) so a multi-file package manifest can reuse this
    /// exact shape with more than one entry under one signature.
    #[derive(Deserialize)]
    pub struct SignedFileEntry {
        pub name: String,
        pub blake3: String,
    }

    /// Shape of an optional `<name>.wasm.sig` sidecar:
    /// `{schema_version, algorithm, public_key (base64, 32 bytes),
    /// files: [{name, blake3}], signature (base64, 64 bytes)}`. `name`
    /// inside each entry is informational only — matching is by content
    /// hash.
    #[derive(Deserialize)]
    pub struct PluginSignatureFile {
        pub schema_version: u32,
        pub algorithm: String,
        pub public_key: String,
        pub files: Vec<SignedFileEntry>,
        pub signature: String,
    }

    /// Outcome of checking a `.wasm` file against its sidecar, if any. Pure
    /// and read-only.
    #[derive(Debug, Clone)]
    pub enum SigCheckResult {
        /// No `.sig` sidecar next to the file.
        Unsigned,
        /// Sidecar present but declares a `schema_version`/`algorithm`/`files`
        /// shape this build doesn't understand — e.g. from a newer app
        /// version. Informational, not a failure.
        Unrecognized,
        /// Sidecar present but malformed: bad JSON, or a key/signature that
        /// doesn't decode to the expected byte length.
        Malformed(String),
        /// The sidecar's declared content hash doesn't match the actual file.
        IntegrityMismatch,
        /// Hash matched but the Ed25519 signature itself doesn't verify.
        SignatureInvalid,
        /// Hash and signature both verify, under this public key.
        Valid(VerifyingKey),
    }

    /// Path of the optional signature sidecar for a given `.wasm` path.
    pub fn sig_sidecar_path(wasm_path: &Path) -> PathBuf {
        let mut p = wasm_path.as_os_str().to_owned();
        p.push(".sig");
        PathBuf::from(p)
    }

    /// Deterministic message the signature is computed over: a
    /// domain-separation prefix followed by each file entry's
    /// `name`/`blake3` hash, name-sorted so the result doesn't depend on the
    /// order `files` happens to be written in.
    pub fn signed_message(entries: &[SignedFileEntry]) -> Vec<u8> {
        let mut sorted: Vec<&SignedFileEntry> = entries.iter().collect();
        sorted.sort_by(|a, b| a.name.cmp(&b.name));
        let mut msg = SIG_DOMAIN.to_vec();
        for e in sorted {
            msg.extend_from_slice(e.name.as_bytes());
            msg.push(0);
            msg.extend_from_slice(e.blake3.as_bytes());
            msg.push(b'\n');
        }
        msg
    }

    /// Checks `wasm_bytes` (the file's actual current content) against the
    /// optional sidecar at `sig_sidecar_path(wasm_path)`.
    pub fn check_signature(wasm_path: &Path, wasm_bytes: &[u8]) -> SigCheckResult {
        let raw = match std::fs::read_to_string(sig_sidecar_path(wasm_path)) {
            Ok(s) => s,
            Err(_) => return SigCheckResult::Unsigned,
        };
        let sig: PluginSignatureFile = match serde_json::from_str(&raw) {
            Ok(s) => s,
            Err(e) => return SigCheckResult::Malformed(e.to_string()),
        };
        if sig.schema_version != SIG_SCHEMA_VERSION || sig.algorithm != SIG_ALGORITHM || sig.files.len() != 1 {
            return SigCheckResult::Unrecognized;
        }

        let actual_hash = blake3::hash(wasm_bytes).to_hex().to_string();
        if sig.files[0].blake3 != actual_hash {
            return SigCheckResult::IntegrityMismatch;
        }

        let Some(public_key) = BASE64.decode(&sig.public_key).ok().and_then(|b| <[u8; 32]>::try_from(b).ok()) else {
            return SigCheckResult::Malformed("public_key is not valid base64 for a 32-byte Ed25519 key".to_string());
        };
        let Some(signature_bytes) = BASE64.decode(&sig.signature).ok().and_then(|b| <[u8; 64]>::try_from(b).ok()) else {
            return SigCheckResult::Malformed("signature is not valid base64 for a 64-byte Ed25519 signature".to_string());
        };
        let Ok(verifying_key) = VerifyingKey::from_bytes(&public_key) else {
            return SigCheckResult::Malformed("public_key is not a valid Ed25519 point".to_string());
        };
        let signature = Signature::from_bytes(&signature_bytes);

        match verifying_key.verify_strict(&signed_message(&sig.files), &signature) {
            Ok(()) => SigCheckResult::Valid(verifying_key),
            Err(_) => SigCheckResult::SignatureInvalid,
        }
    }
}

// ── WASM Store state ──────────────────────────────────────────────────────────

/// Per-execution store state.
///
/// A fresh `PluginState` (and a fresh `Store<PluginState>`) is created for every
/// `execute()` call. This ensures complete isolation between executions.
///
/// Holds:
/// - [`WasiCtx`] — WASIp2 context (sandboxed filesystem, clocks, random, stdio).
/// - [`WasiHttpCtx`] — WASI HTTP context for outbound requests.
/// - [`ResourceTable`] — tracks host-owned resources (shared by WASI and HTTP).
/// - [`wasmtime::StoreLimits`] — enforces the 64 MiB memory cap per execution.
/// - [`PluginHttpHooks`] — enforces this crate's SSRF policy on every outbound
///   request before `wasi:http`'s default send path is allowed to run it.
/// - `storage` — this call's `storage` import backing, if any. `None` for
///   every `describe()`-time `Store` (no plugin type-id is known yet to
///   scope by) and for a `Store` built after [`PluginStorage::open`]
///   failed for this plugin's directory; every `storage.*` guest call then
///   degrades per `wit/node.wit`'s own doc comment instead of panicking.
struct PluginState {
    wasi_ctx: WasiCtx,
    http_ctx: WasiHttpCtx,
    table: ResourceTable,
    limits: wasmtime::StoreLimits,
    http_hooks: PluginHttpHooks,
    storage: Option<PluginStorageHandle>,
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

/// A [`PluginStorage`] handle plus the key-space `scope` this particular
/// `Store` reads and writes under -- see [`PluginStorage`]'s own doc
/// comment for what `scope` is derived from.
#[derive(Clone)]
struct PluginStorageHandle {
    db: Arc<PluginStorage>,
    scope: String,
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

// ── Trigger plugin store state ─────────────────────────────────────────────────

/// Per-instance store state for a trigger-plugin component, run on
/// [`PluginLoader::trigger_engine`].
///
/// HTTP is linked via `wasi:http` p3 ([`TriggerHttpHooks`]), SSRF-filtered
/// identically to [`PluginState`]'s p2 HTTP -- see "Outbound HTTP" in this
/// module's own doc comment. Raw `wasi:sockets` capability is left at
/// `WasiCtxBuilder`'s own default (no `allow_tcp`/`socket_addr_check`
/// override), which denies every address absent an explicit grant -- the
/// same effective position [`make_plugin_state`] leaves action plugins in
/// for raw sockets. The `storage` import is linked identically to
/// [`PluginState`]'s -- see `storage` field below.
///
/// One `TriggerPluginState` (and one `Store`) per running trigger instance,
/// held for that instance's whole lifetime -- unlike [`PluginState`], which
/// is recreated per `execute()` call, a trigger instance's `events()` stream
/// must stay backed by the same store for as long as it's being drained.
struct TriggerPluginState {
    wasi_ctx: WasiCtx,
    http_ctx: WasiHttpCtx,
    table: ResourceTable,
    limits: wasmtime::StoreLimits,
    http_hooks: TriggerHttpHooks,
    /// This instance's `storage` import backing, if any -- same shape and
    /// same "scoped by the resolved `type_id`, `None` if the directory's
    /// storage database couldn't be opened" contract as [`PluginState::storage`].
    storage: Option<PluginStorageHandle>,
}

impl WasiView for TriggerPluginState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView { ctx: &mut self.wasi_ctx, table: &mut self.table }
    }
}

impl P3WasiHttpView for TriggerPluginState {
    fn http(&mut self) -> P3WasiHttpCtxView<'_> {
        P3WasiHttpCtxView {
            ctx: &mut self.http_ctx,
            table: &mut self.table,
            hooks: &mut self.http_hooks,
        }
    }
}

impl wit_storage::StorageHost for TriggerPluginState {
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

fn make_trigger_plugin_state(storage: Option<PluginStorageHandle>) -> TriggerPluginState {
    TriggerPluginState {
        wasi_ctx: WasiCtx::builder().build(),
        http_ctx: WasiHttpCtx::new(),
        table: ResourceTable::new(),
        limits: StoreLimitsBuilder::new().memory_size(PLUGIN_MEMORY_LIMIT).build(),
        http_hooks: TriggerHttpHooks,
        storage,
    }
}

/// Bridges a guest's `trigger-event` stream to an mpsc channel of plain
/// JSON strings. `poll_consume` waits for channel capacity before pulling
/// from `source`, so a slow receiver applies backpressure to the guest
/// instead of buffering unboundedly on the host side; if the receiver is
/// dropped mid-stream, the stream is reported as `Dropped` so the guest's
/// writer stops.
struct TriggerEventConsumer {
    tx: tokio_util::sync::PollSender<Result<String, String>>,
}

impl StreamConsumer<TriggerPluginState> for TriggerEventConsumer {
    type Item = wit_trigger::exports::aerini::plugin::trigger::TriggerEvent;

    fn poll_consume(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: wasmtime::StoreContextMut<TriggerPluginState>,
        mut source: Source<'_, Self::Item>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let this = self.get_mut();
        match this.tx.poll_reserve(cx) {
            Poll::Pending => {
                if finish {
                    Poll::Ready(Ok(StreamResult::Cancelled))
                } else {
                    Poll::Pending
                }
            }
            Poll::Ready(Err(_)) => Poll::Ready(Ok(StreamResult::Dropped)),
            Poll::Ready(Ok(())) => {
                if source.remaining(&mut store) == 0 {
                    // poll_reserve already committed a slot; nothing to send yet, so give it back.
                    this.tx.abort_send();
                    return Poll::Ready(Ok(StreamResult::Completed));
                }

                let mut buf: Vec<Self::Item> = Vec::with_capacity(1);
                if let Err(e) = source.read(&mut store, &mut buf) {
                    return Poll::Ready(Err(e));
                }

                let Some(event) = buf.into_iter().next() else {
                    return Poll::Ready(Ok(StreamResult::Completed));
                };

                if this.tx.send_item(Ok(event.data)).is_err() {
                    return Poll::Ready(Ok(StreamResult::Dropped));
                }

                Poll::Ready(Ok(StreamResult::Completed))
            }
        }
    }
}

/// No epoch deadline is set on trigger stores (contrast [`make_store`]):
/// a trigger instance is meant to stay alive and idle between events
/// indefinitely, so the same fixed wall-clock deadline that bounds one
/// `execute()` call would misfire on a trigger that's simply waiting for
/// its next event. Cancellation for the trigger path is `tokio::time::timeout`
/// plus the owning job task's abort, both handled by the caller
/// (`scheduler/runner.rs`), not by this store.
fn make_trigger_store(
    engine: &Engine,
    state: TriggerPluginState,
) -> wasmtime::Store<TriggerPluginState> {
    let mut store = wasmtime::Store::new(engine, state);
    store.limiter(|s| &mut s.limits);
    store
}

// ── Outbound HTTP SSRF enforcement ─────────────────────────────────────────────

/// `wasi:http/outgoing-handler`'s default send path (`default_send_request`) connects
/// straight to whatever host the guest asks for — it has no knowledge of this
/// application's SSRF policy. `WasiHttpHooks::send_request` is the documented
/// interception point, so plugin traffic is now checked exactly like any other
/// node's HTTP/DB egress before `default_send_request` is allowed to run.
struct PluginHttpHooks;

impl WasiHttpHooks for PluginHttpHooks {
    fn send_request(
        &mut self,
        request: Request<HyperOutgoingBody>,
        config: OutgoingRequestConfig,
    ) -> HttpResult<HostFutureIncomingResponse> {
        if let Err(code) = check_plugin_request_ssrf(request.uri()) {
            return Ok(HostFutureIncomingResponse::ready(Ok(Err(code))));
        }
        Ok(default_send_request(request, config))
    }
}

/// Version-agnostic outcome of [`check_ssrf_uri`] — mapped to the caller's own
/// protocol-version `error-code` type by each thin wrapper
/// ([`check_plugin_request_ssrf`] for p2, [`check_plugin_request_ssrf_p3`] for
/// p3). `p2::bindings::http::types::ErrorCode` and
/// `p3::bindings::http::types::ErrorCode` are distinct Rust types generated
/// from two different WIT package versions, even though both declare the same
/// `destination-not-found` / `destination-IP-prohibited` /
/// `HTTP-request-URI-invalid` variants this enum mirrors.
enum SsrfRejection {
    UriInvalid,
    Prohibited,
    NotFound,
}

/// Applies this crate's standard SSRF policy (`SsrfPolicy::Strict` — the same
/// policy the Database and HTTP nodes enforce, see `nodes::util::check_ssrf_ip`)
/// to a WASM plugin's outbound request URI before it is sent. Shared by both
/// [`PluginHttpHooks::send_request`] (p2, action plugins) and
/// [`TriggerHttpHooks::send_request`] (p3, trigger plugins) via their thin
/// wrappers below, so the two engines can't drift apart on what's blocked.
///
/// IP-literal hosts are checked directly (IPv6 authority brackets are stripped
/// first — `hyper::Uri::host()` keeps them). Domain names are resolved with a
/// blocking DNS lookup and every returned address is checked; safe to block on
/// here since both callers only ever run this inside a blocking context —
/// `tokio::task::spawn_blocking` for `WasmPluginNode::execute` (p2), the
/// trigger engine's own blocking-pool-backed I/O for `send_request` (p3, per
/// `wasmtime_wasi_http::p3`'s own `send_request` contract) — never on an async
/// worker thread. A missing or unparsable host, a failed resolution, or an
/// empty result set all reject the request — fail closed, matching this
/// crate's existing SSRF-check convention (`nodes::util::check_host_ssrf`).
///
/// Residual limits, same caveat `check_host_ssrf` itself documents:
/// - A DNS-rebinding TOCTOU gap remains between this check and the connect
///   `default_send_request` performs a moment later.
/// - No explicit timeout on the resolution call, consistent with every other
///   SSRF check in this crate — a stalled resolver holds one blocking-pool
///   thread, not an async worker.
///
/// Both require network-level egress filtering to close fully — see the
/// startup warning in `load_plugins`.
fn check_ssrf_uri(uri: &hyper::Uri) -> Result<(), SsrfRejection> {
    let host = uri.host().ok_or(SsrfRejection::UriInvalid)?;
    let port = uri
        .port_u16()
        .unwrap_or(if uri.scheme_str() == Some("https") { 443 } else { 80 });

    // `hyper::Uri::host()` keeps the `[...]` brackets around an IPv6 literal;
    // strip them before attempting to parse as an IP address.
    let ip_candidate = host.strip_prefix('[').and_then(|s| s.strip_suffix(']')).unwrap_or(host);
    if let Ok(ip) = ip_candidate.parse::<std::net::IpAddr>() {
        return check_ssrf_ip(ip, SsrfPolicy::Strict).map_err(|_| SsrfRejection::Prohibited);
    }

    let lower = host.to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".localhost") || lower == "metadata.google.internal" {
        return Err(SsrfRejection::Prohibited);
    }

    let addrs = (host, port).to_socket_addrs().map_err(|_| SsrfRejection::NotFound)?;
    let mut resolved_any = false;
    for addr in addrs {
        resolved_any = true;
        check_ssrf_ip(addr.ip(), SsrfPolicy::Strict).map_err(|_| SsrfRejection::Prohibited)?;
    }
    if !resolved_any {
        return Err(SsrfRejection::NotFound);
    }
    Ok(())
}

/// p2 wrapper around [`check_ssrf_uri`] — see that function's doc comment for
/// the policy and its residual limits.
fn check_plugin_request_ssrf(uri: &hyper::Uri) -> Result<(), ErrorCode> {
    check_ssrf_uri(uri).map_err(|rejection| match rejection {
        SsrfRejection::UriInvalid => ErrorCode::HttpRequestUriInvalid,
        SsrfRejection::Prohibited => ErrorCode::DestinationIpProhibited,
        SsrfRejection::NotFound => ErrorCode::DestinationNotFound,
    })
}

/// p3 wrapper around [`check_ssrf_uri`] — see that function's doc comment for
/// the policy and its residual limits. Used by
/// [`TriggerHttpHooks::send_request`].
fn check_plugin_request_ssrf_p3(uri: &hyper::Uri) -> Result<(), P3ErrorCode> {
    check_ssrf_uri(uri).map_err(|rejection| match rejection {
        SsrfRejection::UriInvalid => P3ErrorCode::HttpRequestUriInvalid,
        SsrfRejection::Prohibited => P3ErrorCode::DestinationIpProhibited,
        SsrfRejection::NotFound => P3ErrorCode::DestinationNotFound,
    })
}

/// Trigger-plugin counterpart to [`PluginHttpHooks`], wired through
/// `wasmtime_wasi_http::p3`'s own `WasiHttpHooks`/`WasiHttpView` traits
/// instead of `p2`'s — `TriggerPluginState` runs on `trigger_engine`
/// (`wasm_component_model_async`), and `p3::add_to_linker`'s bound is
/// `p3::WasiHttpView`, a distinct trait from `p2::WasiHttpView` despite the
/// identical name (`wasmtime_wasi_http` gives p2 and p3 fully separate
/// `WasiHttpHooks`/`WasiHttpView`/`WasiHttpCtxView` types; only `WasiHttpCtx`
/// itself is shared — see `TriggerPluginState::http_ctx`). Same SSRF policy
/// as `PluginHttpHooks`, via [`check_plugin_request_ssrf_p3`]'s shared
/// [`check_ssrf_uri`] — the two engines can't drift apart on what's blocked.
struct TriggerHttpHooks;

impl P3WasiHttpHooks for TriggerHttpHooks {
    fn send_request(
        &mut self,
        request: hyper::Request<UnsyncBoxBody<Bytes, P3ErrorCode>>,
        options: Option<P3RequestOptions>,
        fut: Box<dyn std::future::Future<Output = Result<(), P3ErrorCode>> + Send>,
    ) -> Box<
        dyn std::future::Future<
                Output = Result<
                    (
                        hyper::Response<UnsyncBoxBody<Bytes, P3ErrorCode>>,
                        Box<dyn std::future::Future<Output = Result<(), P3ErrorCode>> + Send>,
                    ),
                    wasmtime_wasi::TrappableError<P3ErrorCode>,
                >,
            > + Send,
    > {
        // Not used on either path below: nothing is sent on rejection, and
        // `wasmtime_wasi_http::p3`'s own default `send_request` implementation
        // (which the success path mirrors) discards this identically.
        _ = fut;
        Box::new(async move {
            // `check_ssrf_uri` does a blocking DNS lookup for a non-IP-literal
            // host. Unlike the p2 path -- whose caller, `WasmPluginNode::execute`,
            // already runs entirely inside a `tokio::task::spawn_blocking`
            // closure -- this future runs on `trigger_engine`'s ordinary async
            // task (`start_trigger`'s `store.run_concurrent`), so the check
            // itself has to be the thing offloaded here, not assumed already
            // off the async reactor thread.
            let uri = request.uri().clone();
            let rejection = tokio::task::spawn_blocking(move || check_plugin_request_ssrf_p3(&uri))
                .await
                .map_err(|e| {
                    wasmtime_wasi::TrappableError::trap(wasmtime::Error::msg(format!(
                        "SSRF check task panicked: {e}"
                    )))
                })?;
            if let Err(code) = rejection {
                return Err(code.into());
            }
            use http_body_util::BodyExt;
            let (res, io) = p3_default_send_request(request, options).await?;
            Ok((res.map(BodyExt::boxed_unsync), Box::new(io) as Box<dyn std::future::Future<Output = _> + Send>))
        })
    }
}

#[cfg(test)]
mod plugin_http_hooks_tests {
    use super::{
        Bytes, ErrorCode, P3ErrorCode, P3WasiHttpHooks, UnsyncBoxBody, check_plugin_request_ssrf,
        check_plugin_request_ssrf_p3,
    };

    #[test]
    fn public_ip_allowed() {
        let uri: hyper::Uri = "http://93.184.216.34/".parse().unwrap();
        assert!(check_plugin_request_ssrf(&uri).is_ok());
    }

    #[test]
    fn private_ip_blocked() {
        let v4: hyper::Uri = "http://192.168.1.1/".parse().unwrap();
        assert!(matches!(check_plugin_request_ssrf(&v4), Err(ErrorCode::DestinationIpProhibited)));

        // IPv6 loopback via bracketed authority -- `hyper::Uri::host()` keeps the
        // brackets, exercising the strip-before-parse path above.
        let v6: hyper::Uri = "http://[::1]:8080/".parse().unwrap();
        assert!(matches!(check_plugin_request_ssrf(&v6), Err(ErrorCode::DestinationIpProhibited)));
    }

    #[test]
    fn relative_uri_without_host_rejected() {
        let uri: hyper::Uri = "/no-authority".parse().unwrap();
        assert!(matches!(check_plugin_request_ssrf(&uri), Err(ErrorCode::HttpRequestUriInvalid)));
    }

    // p3 (trigger-plugin) wrapper: same shared `check_ssrf_uri`, only the
    // outer `error-code` type differs from the p2 tests above -- one normal
    // case and the one edge case that exercises the mapping (not a re-proof
    // of `check_ssrf_uri` itself, already fully covered above).
    #[test]
    fn public_ip_allowed_p3() {
        let uri: hyper::Uri = "http://93.184.216.34/".parse().unwrap();
        assert!(check_plugin_request_ssrf_p3(&uri).is_ok());
    }

    #[test]
    fn private_ip_blocked_p3() {
        let uri: hyper::Uri = "http://192.168.1.1/".parse().unwrap();
        assert!(matches!(check_plugin_request_ssrf_p3(&uri), Err(P3ErrorCode::DestinationIpProhibited)));
    }

    // Exercises `TriggerHttpHooks::send_request` itself, not just the pure
    // `check_plugin_request_ssrf_p3` mapping above -- the SSRF check has to
    // survive being offloaded through `spawn_blocking` and converted back
    // into a `TrappableError` without losing its `error-code`; a test on
    // the pure mapping function alone can't prove that plumbing compiles
    // or behaves correctly end to end.
    #[tokio::test]
    async fn trigger_http_hooks_send_request_rejects_private_ip() {
        use http_body_util::{BodyExt, Empty};
        use std::future::Future;

        let body: UnsyncBoxBody<Bytes, P3ErrorCode> = Empty::new().map_err(|_| unreachable!()).boxed_unsync();
        let request = hyper::Request::builder()
            .uri("http://192.168.1.1/")
            .body(body)
            .expect("request build failed");
        let fut: Box<dyn Future<Output = Result<(), P3ErrorCode>> + Send> = Box::new(async { Ok(()) });

        let mut hooks = super::TriggerHttpHooks;
        let result = Box::into_pin(hooks.send_request(request, None, fut)).await;

        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("expected the private-IP request to be rejected"),
        };
        let code = err.downcast().expect("expected a guest-visible error-code, not a host trap");
        assert!(matches!(code, P3ErrorCode::DestinationIpProhibited));
    }
}

// ── Memory limit ──────────────────────────────────────────────────────────────

/// Maximum linear memory per plugin execution: 64 MiB.
///
/// Plugin instances that attempt to grow beyond this limit receive a WASM
/// `memory.grow` failure surfacing as a trap in the guest.
const PLUGIN_MEMORY_LIMIT: usize = 64 * 1024 * 1024;

/// CPU time limit for plugin execution, expressed in epoch ticks.
///
/// The epoch ticker thread (started in [`PluginLoader::new`]) increments the
/// engine epoch every 10 ms. A deadline of 3 000 ticks therefore limits each
/// plugin call to approximately 30 seconds of wall-clock time before it is
/// interrupted with a trap. Applies to both `describe()` at load time and
/// `execute()` per workflow run.
const PLUGIN_EPOCH_DEADLINE: u64 = 3_000;

/// Per-call wall-clock budget for any single guest call made while resolving
/// or starting a trigger-plugin instance (`describe`, and the initial
/// `events(config)` call) — the async trigger engine's counterpart to
/// [`PLUGIN_EPOCH_DEADLINE`]'s role above, using the same ~30s budget for
/// consistency. Deliberately not applied to *draining* an already-started
/// event stream afterward: waiting indefinitely between events is the
/// normal, expected state for a trigger (unlike a bounded `describe`/`events`
/// call, which should always return promptly) — timing that out would
/// misfire on every healthy, simply-quiet trigger.
const PLUGIN_TRIGGER_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Max chars of a malformed plugin `data` string echoed into the error log
/// (`wit_output_to_engine`) — avoids dumping an arbitrarily large/binary
/// blob into logs while still giving an operator enough to diagnose.
const INVALID_OUTPUT_LOG_PREVIEW_CHARS: usize = 200;

fn make_plugin_state(storage: Option<PluginStorageHandle>) -> PluginState {
    PluginState {
        // No preopened directories, no inherited stdio — sandboxed context.
        // Filesystem syscalls succeed at the API level but return "not found" /
        // "permission denied" since no paths are mounted.
        wasi_ctx: WasiCtx::builder().build(),
        http_ctx: WasiHttpCtx::new(),
        table: ResourceTable::new(),
        limits: StoreLimitsBuilder::new().memory_size(PLUGIN_MEMORY_LIMIT).build(),
        http_hooks: PluginHttpHooks,
        storage,
    }
}

// ── Plugin storage (WIT `storage` import backing) ──────────────────────────────

/// File name of the per-`plugin_dir` storage database — a dotfile sidecar
/// matching the `.aerini-plugin-trust.json` naming convention exactly (same
/// directory, same "hidden management file living next to the `.wasm`s"
/// pattern), scanned past by every existing `.wasm`-extension-filtered
/// directory walk in this crate and in `commands/plugins.rs` without any
/// change needed there.
const STORAGE_DB_FILENAME: &str = ".aerini-plugin-storage.db";

/// Max bytes for a single storage key.
const STORAGE_MAX_KEY_BYTES: usize = 256;
/// Max bytes for a single storage value.
const STORAGE_MAX_VALUE_BYTES: usize = 64 * 1024;
/// Max distinct keys one plugin (one `scope`) may hold at once — bounds
/// row/index growth independently of the byte quota below (many tiny keys
/// would otherwise sail under a bytes-only cap).
const STORAGE_MAX_KEYS_PER_SCOPE: i64 = 10_000;
/// Max total bytes (sum of all value lengths) one plugin (one `scope`) may
/// hold at once. A durable-notes budget, not a database's — see
/// `interface storage`'s own doc comment in `wit/node.wit`.
const STORAGE_MAX_SCOPE_BYTES: i64 = 5 * 1024 * 1024;

/// Bounded, host-provided key-value storage backing the WIT `storage`
/// import — one `PluginStorage` per `plugin_dir`, opened lazily and cached
/// (see [`PluginLoader::plugin_storage`]), shared via `Arc` across every
/// [`WasmPluginNode`] loaded from that directory. `r2d2::Pool` is
/// `Send + Sync` and cheap to clone, matching `Engine`'s own sharing model
/// elsewhere in this file — the same `rusqlite` + `r2d2`/`r2d2_sqlite`
/// stack already used by `crate::db::WorkflowDb`, reused here rather than
/// adding a new storage-backend dependency.
///
/// **Scoped per plugin type-id**, not per placed node instance: every
/// execution of a given plugin, across every workflow and every
/// placement, reads and writes the same key space. Resolved this way over
/// per-node-instance scoping because (a) it needs zero new plumbing —
/// `WasmPluginNode` already carries `type_id`, while node-instance scoping
/// would need additions to `node-input.params`'s shape that nothing else
/// here requires, and (b) it matches this capability's own framing as "a
/// safe substitute for real filesystem access" (see this file's own
/// module-level doc comment) — a plugin's own app-data directory, on a
/// real filesystem, wouldn't vary by which workflow node happens to be
/// calling into it either. A plugin author who wants isolation between
/// two placements of their own node type must namespace their own keys
/// (e.g. fold a config value into the key) — documented on `interface
/// storage`. Revisit as per-node-instance scoping if shared state across
/// placements proves to cause real collisions in practice, not guessed at
/// here.
struct PluginStorage {
    pool: r2d2::Pool<SqliteConnectionManager>,
}

impl PluginStorage {
    /// Opens (creating if absent) the storage database at
    /// `plugin_dir/`[`STORAGE_DB_FILENAME`]. Does not create `plugin_dir`
    /// itself — every real caller only reaches this after a `.wasm` file
    /// was already successfully read from that same directory (see
    /// [`PluginLoader::plugin_storage`]'s doc comment), so it's already
    /// known to exist.
    fn open(plugin_dir: &Path) -> Result<Self, String> {
        let path = plugin_dir.join(STORAGE_DB_FILENAME);
        let manager = SqliteConnectionManager::file(&path).with_init(|conn| {
            conn.execute_batch(
                "PRAGMA busy_timeout=5000;
                 PRAGMA journal_mode=WAL;
                 PRAGMA synchronous=NORMAL;"
            )
        });
        let pool = r2d2::Pool::builder().max_size(4).build(manager).map_err(|e| e.to_string())?;

        let conn = pool.get().map_err(|e| e.to_string())?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS plugin_storage (
                scope TEXT NOT NULL,
                key   TEXT NOT NULL,
                value BLOB NOT NULL,
                PRIMARY KEY (scope, key)
            );"
        ).map_err(|e| e.to_string())?;
        drop(conn);

        Ok(Self { pool })
    }

    fn get(&self, scope: &str, key: &str) -> Option<String> {
        let conn = self.pool.get().map_err(|e| {
            tracing::warn!("plugin storage: pool unavailable on get: {e}");
        }).ok()?;
        conn.query_row(
            "SELECT value FROM plugin_storage WHERE scope = ?1 AND key = ?2",
            rusqlite::params![scope, key],
            |row| row.get::<_, String>(0),
        ).ok()
    }

    /// Checks the key/value size limits and the per-scope key-count/byte
    /// quotas, then writes — all inside one `IMMEDIATE` transaction so a
    /// concurrent `set` on the same scope (two workflow runs of the same
    /// plugin type in parallel) can't race the quota check against the
    /// write (`BEGIN IMMEDIATE` takes SQLite's write lock upfront, closing
    /// the check-then-write TOCTOU gap a plain `BEGIN` would leave open).
    /// Known, accepted trade-off: under real contention on the same scope,
    /// a losing transaction waits on `busy_timeout` (5s, set in `open`)
    /// rather than queuing indefinitely — a `set` can surface `unavailable`
    /// under sustained concurrent writes to one plugin's key space, not
    /// just on a genuine backend failure. Not expected to matter at this
    /// capability's intended scale (a few small notes, not a write-heavy
    /// database); documented rather than engineered around.
    fn set(&self, scope: &str, key: &str, value: &str) -> Result<(), wit_storage::StorageError> {
        if key.len() > STORAGE_MAX_KEY_BYTES {
            return Err(wit_storage::StorageError::KeyTooLong);
        }
        if value.len() > STORAGE_MAX_VALUE_BYTES {
            return Err(wit_storage::StorageError::ValueTooLarge);
        }

        let mut conn = self.pool.get().map_err(|e| {
            tracing::warn!("plugin storage: pool unavailable on set: {e}");
            wit_storage::StorageError::Unavailable
        })?;
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| {
                tracing::warn!("plugin storage: failed to start transaction: {e}");
                wit_storage::StorageError::Unavailable
            })?;

        let existing_len: Option<i64> = tx
            .query_row(
                "SELECT LENGTH(value) FROM plugin_storage WHERE scope = ?1 AND key = ?2",
                rusqlite::params![scope, key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| {
                tracing::warn!("plugin storage: quota pre-check failed: {e}");
                wit_storage::StorageError::Unavailable
            })?;

        if existing_len.is_none() {
            let key_count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM plugin_storage WHERE scope = ?1",
                    rusqlite::params![scope],
                    |row| row.get(0),
                )
                .map_err(|e| {
                    tracing::warn!("plugin storage: key-count check failed: {e}");
                    wit_storage::StorageError::Unavailable
                })?;
            if key_count >= STORAGE_MAX_KEYS_PER_SCOPE {
                return Err(wit_storage::StorageError::QuotaExceeded);
            }
        }

        let scope_total: i64 = tx
            .query_row(
                "SELECT COALESCE(SUM(LENGTH(value)), 0) FROM plugin_storage WHERE scope = ?1",
                rusqlite::params![scope],
                |row| row.get(0),
            )
            .map_err(|e| {
                tracing::warn!("plugin storage: total-bytes check failed: {e}");
                wit_storage::StorageError::Unavailable
            })?;

        let projected = scope_total - existing_len.unwrap_or(0) + value.len() as i64;
        if projected > STORAGE_MAX_SCOPE_BYTES {
            return Err(wit_storage::StorageError::QuotaExceeded);
        }

        tx.execute(
            "INSERT INTO plugin_storage (scope, key, value) VALUES (?1, ?2, ?3)
             ON CONFLICT(scope, key) DO UPDATE SET value = excluded.value",
            rusqlite::params![scope, key, value],
        ).map_err(|e| {
            tracing::warn!("plugin storage: write failed: {e}");
            wit_storage::StorageError::Unavailable
        })?;

        tx.commit().map_err(|e| {
            tracing::warn!("plugin storage: commit failed: {e}");
            wit_storage::StorageError::Unavailable
        })?;

        Ok(())
    }

    fn delete(&self, scope: &str, key: &str) {
        let Ok(conn) = self.pool.get().map_err(|e| tracing::warn!("plugin storage: pool unavailable on delete: {e}")) else {
            return;
        };
        if let Err(e) = conn.execute(
            "DELETE FROM plugin_storage WHERE scope = ?1 AND key = ?2",
            rusqlite::params![scope, key],
        ) {
            tracing::warn!("plugin storage: delete failed: {e}");
        }
    }

    /// Lists keys under `scope` starting with `prefix`. `prefix` is
    /// caller-supplied (guest-controlled) content, not a trusted pattern —
    /// `%`/`_`/`\` are escaped to literals before being used in a SQL
    /// `LIKE ... ESCAPE '\'` clause, so a key containing those characters
    /// can't be used to widen the match beyond a literal prefix.
    fn list_keys(&self, scope: &str, prefix: &str) -> Result<Vec<String>, wit_storage::StorageError> {
        let conn = self.pool.get().map_err(|e| {
            tracing::warn!("plugin storage: pool unavailable on list_keys: {e}");
            wit_storage::StorageError::Unavailable
        })?;

        let mut escaped = String::with_capacity(prefix.len());
        for c in prefix.chars() {
            if c == '%' || c == '_' || c == '\\' {
                escaped.push('\\');
            }
            escaped.push(c);
        }
        let pattern = format!("{escaped}%");

        let mut stmt = conn
            .prepare("SELECT key FROM plugin_storage WHERE scope = ?1 AND key LIKE ?2 ESCAPE '\\' ORDER BY key")
            .map_err(|e| {
                tracing::warn!("plugin storage: list_keys prepare failed: {e}");
                wit_storage::StorageError::Unavailable
            })?;
        let rows = stmt
            .query_map(rusqlite::params![scope, pattern], |row| row.get::<_, String>(0))
            .map_err(|e| {
                tracing::warn!("plugin storage: list_keys query failed: {e}");
                wit_storage::StorageError::Unavailable
            })?;

        let mut keys = Vec::new();
        for row in rows {
            keys.push(row.map_err(|e| {
                tracing::warn!("plugin storage: list_keys row read failed: {e}");
                wit_storage::StorageError::Unavailable
            })?);
        }
        Ok(keys)
    }
}

#[cfg(test)]
mod plugin_storage_tests {
    use super::*;

    fn storage() -> PluginStorage {
        let dir = tempfile::tempdir().expect("tempdir create failed");
        // Leak the TempDir so it outlives this test's `PluginStorage` --
        // acceptable in a `#[cfg(test)]`-only helper (mirrors this file's
        // own `NamedTempFile` test fixtures elsewhere, which rely on the
        // guard staying alive for the test's duration rather than cleaning
        // up immediately).
        let path = dir.keep();
        PluginStorage::open(&path).expect("PluginStorage::open failed")
    }

    /// Normal case: a value written by `set` is returned by `get`, scoped
    /// under the same `scope` it was written under.
    #[test]
    fn set_then_get_round_trips() {
        let db = storage();
        assert_eq!(db.get("plugin-a", "k1"), None, "unset key must read as absent");
        db.set("plugin-a", "k1", "hello").expect("set failed");
        assert_eq!(db.get("plugin-a", "k1"), Some("hello".to_string()));
        // A different scope must not see it.
        assert_eq!(db.get("plugin-b", "k1"), None, "scopes must not leak into each other");
    }

    /// Edge case: oversized keys/values are rejected before ever reaching
    /// SQLite, with the specific error the size limit that was exceeded.
    #[test]
    fn set_rejects_oversized_key_and_value() {
        let db = storage();
        let long_key = "k".repeat(STORAGE_MAX_KEY_BYTES + 1);
        assert!(matches!(db.set("plugin-a", &long_key, "v"), Err(wit_storage::StorageError::KeyTooLong)));

        let long_value = "v".repeat(STORAGE_MAX_VALUE_BYTES + 1);
        assert!(matches!(db.set("plugin-a", "k1", &long_value), Err(wit_storage::StorageError::ValueTooLarge)));
    }

    /// Edge case: the per-scope byte quota is enforced across multiple
    /// keys, and an update to an *existing* key correctly nets out its own
    /// prior size rather than double-counting it.
    #[test]
    fn set_enforces_scope_byte_quota() {
        let db = storage();
        let chunk = "x".repeat(STORAGE_MAX_VALUE_BYTES);
        let mut written = 0i64;
        let mut i = 0;
        loop {
            let key = format!("k{i}");
            match db.set("plugin-a", &key, &chunk) {
                Ok(()) => { written += chunk.len() as i64; i += 1; }
                Err(wit_storage::StorageError::QuotaExceeded) => break,
                Err(e) => panic!("unexpected error: {e:?}"),
            }
            assert!(written <= STORAGE_MAX_SCOPE_BYTES, "wrote past the quota before it was enforced");
        }
        assert!(written > 0, "quota must allow at least one write before rejecting");

        // Overwriting an already-stored key with an equal-sized value must
        // still succeed -- it doesn't add new net bytes to the scope.
        db.set("plugin-a", "k0", &chunk).expect("overwrite of existing key must not double-count its own prior size");
    }

    #[test]
    fn delete_removes_key() {
        let db = storage();
        db.set("plugin-a", "k1", "v").expect("set failed");
        assert_eq!(db.get("plugin-a", "k1"), Some("v".to_string()));
        db.delete("plugin-a", "k1");
        assert_eq!(db.get("plugin-a", "k1"), None);
        // Deleting an already-absent key is a no-op, not an error.
        db.delete("plugin-a", "does-not-exist");
    }

    /// `%`/`_` in a caller-supplied prefix must be treated as literal
    /// characters, not SQL `LIKE` wildcards -- otherwise a plugin could
    /// list keys outside the prefix it actually asked for.
    #[test]
    fn list_keys_escapes_like_metacharacters() {
        let db = storage();
        db.set("plugin-a", "a%b", "v1").expect("set failed");
        db.set("plugin-a", "aXb", "v2").expect("set failed");
        db.set("plugin-a", "a%bc", "v3").expect("set failed");

        let matches = db.list_keys("plugin-a", "a%b").expect("list_keys failed");
        assert_eq!(
            matches,
            vec!["a%b".to_string(), "a%bc".to_string()],
            "prefix 'a%b' must match only keys literally starting with 'a%b', not 'aXb' via wildcard expansion"
        );
    }
}

fn make_store(engine: &Engine, state: PluginState) -> wasmtime::Store<PluginState> {
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

// ── PluginLoader ──────────────────────────────────────────────────────────────

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
    /// share one. No epoch ticker: see `make_trigger_store`'s doc comment
    /// for why.
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

/// Maps a plugin-supplied category string to a [`NodeType`], defaulting to
/// [`NodeType::Action`] (with a warning) for any unrecognised value. Shared
/// by [`PluginLoader::load_plugin`] and [`PluginLoader::describe_plugin`] so
/// the mapping — and its warning — can't drift between the two call paths.
fn category_to_node_type(category: &str, type_id_for_log: &str) -> NodeType {
    match category {
        "ai" => NodeType::Ai,
        "logic" => NodeType::Logic,
        "utility" => NodeType::Utility,
        "action" => NodeType::Action,
        other => {
            tracing::warn!(
                "plugin '{}': unrecognised category '{}' — defaulting to Action",
                type_id_for_log,
                other
            );
            NodeType::Action
        }
    }
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
        let ticker_engine = engine.clone();
        std::thread::Builder::new()
            .name("wasm-epoch-ticker".to_string())
            .spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_millis(10));
                ticker_engine.increment_epoch();
            })
            .expect("failed to spawn wasm epoch ticker thread");

        // Second engine for trigger-plugin components. `wasm_component_model_async`
        // is what lets this engine compile and instantiate a component using
        // `trigger.events`'s async ABI; `engine` above doesn't set it, and per
        // `wit/node.wit`'s own doc comment the two configurations cannot be
        // combined on one `Engine`. No `epoch_interruption` here — see
        // `make_trigger_store`.
        let mut trigger_config = Config::new();
        trigger_config.wasm_component_model(true);
        trigger_config.wasm_component_model_async(true);
        let trigger_engine = Engine::new(&trigger_config)?;

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

    /// Checks `path`'s signature sidecar (if any), reusing the cached result
    /// when the file's `(mtime, len)` fingerprint hasn't changed since the
    /// last check — same fingerprint shape `compile_and_link` uses, so
    /// repeated hot-reload polling doesn't re-hash an unchanged file.
    fn verify_signature(&self, path: &Path) -> Result<signature::SigCheckResult, PluginLoadError> {
        let meta = std::fs::metadata(path)?;
        let fingerprint = (meta.modified().ok(), meta.len());

        let cached = self
            .signature_cache
            .lock()
            .expect("signature cache mutex poisoned")
            .get(path)
            .filter(|entry| entry.fingerprint == fingerprint)
            .map(|entry| entry.result.clone());

        if let Some(result) = cached {
            return Ok(result);
        }

        let bytes = std::fs::read(path)?;
        let result = signature::check_signature(path, &bytes);
        self.signature_check_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        self.signature_cache
            .lock()
            .expect("signature cache mutex poisoned")
            .insert(path.to_path_buf(), CachedSignature { fingerprint, result: result.clone() });

        Ok(result)
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

    /// Steps shared by [`load_plugin`](Self::load_plugin) and
    /// [`describe_plugin`](Self::describe_plugin): read the file, compile it
    /// to a component, build a linker with the full WASIp2 + HTTP surface,
    /// and pre-instantiate. Does not validate which world is exported --
    /// that's [`detect_node_pre`](Self::detect_node_pre), since both call
    /// paths need to try the same `InstancePre` against two possible world
    /// shapes. Extracted so the two call paths — one that goes on to leak
    /// `'static` strings and register a live `Node`, one that doesn't —
    /// can't drift apart on the compile/link logic itself.
    fn compile_and_link(&self, path: &Path) -> Result<wasmtime::component::InstancePre<PluginState>, PluginLoadError> {
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
        // Three calls are required:
        // (a) `wasmtime_wasi::p2::add_to_linker_sync` — satisfies all `wasi:cli/command`
        //     imports (filesystem, random, clocks, stdio) that `wasm32-wasip2` binaries
        //     unconditionally import via Rust's standard library.
        // (b) `wasmtime_wasi_http::p2::add_only_http_to_linker_sync` — adds only the
        //     `wasi:http/outgoing-handler` interface without duplicating the WASI
        //     interfaces already registered in (a).
        // (c) this package's own `storage` import (below) — see its own comment.
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
        // no-op for it, not a compatibility risk (Constraint 1).
        wit_storage::add_to_linker::<PluginState, wasmtime::component::HasSelf<PluginState>>(
            &mut linker,
            |state| state,
        ).map_err(|e| PluginLoadError::WasmLink(e.to_string()))?;

        linker.instantiate_pre(&component)
            .map_err(|e| PluginLoadError::WasmLink(e.to_string()))
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

    /// Load a single WASM plugin from `path`.
    ///
    /// Steps:
    /// 0. Check the signature sidecar, if any (see
    ///    [`verify_signature`](Self::verify_signature)) — a file whose
    ///    content doesn't match its declared hash, or whose signature
    ///    doesn't verify, is rejected outright
    ///    (→ [`PluginLoadError::SignatureRejected`]) before any compilation
    ///    is attempted. Unsigned, unrecognized-format, and validly-signed
    ///    files all proceed normally — publisher-key pinning/trust is a
    ///    desktop-UI concern layered on top, not enforced here.
    /// 1. Read the file (→ [`PluginLoadError::Io`] on failure).
    /// 2. Compile to a component (→ [`PluginLoadError::WasmCompile`] on failure).
    /// 3. Build a linker with full WASIp2 + HTTP + `storage` and pre-instantiate
    ///    (→ [`PluginLoadError::WasmLink`] on failure).
    /// 4. Detect whether the component exports `aerini-node` or
    ///    `aerini-node-with-metadata` (→ [`PluginLoadError::MissingInterface`]
    ///    if neither).
    /// 5. Call `describe()` (and `describe-metadata()`, when exported) once to
    ///    populate the [`NodeDescriptor`](crate::node::NodeDescriptor). A plugin
    ///    that doesn't export `metadata` gets the host defaults from
    ///    `wit/node.wit`'s doc comment: empty author, `"1.0.0"` version, no icon.
    /// 6. Open (or reuse) this plugin's directory's storage database (see
    ///    [`PluginLoader::plugin_storage`]) — never fails the load; a plugin
    ///    whose storage database couldn't be opened just gets `storage: None`.
    /// 7. Probe whether the same file also exports `aerini-node-with-trigger`
    ///    (see [`PluginLoader::probe_trigger_capable`]) — type-level only, no
    ///    guest code runs; never fails the load.
    ///
    /// The describe result is cached in [`WasmPluginNode`] for the lifetime
    /// of the process — it is never called again after load time.
    pub fn load_plugin(&self, path: &Path) -> Result<Arc<dyn Node>, PluginLoadError> {
        match self.verify_signature(path)? {
            signature::SigCheckResult::IntegrityMismatch => {
                return Err(PluginLoadError::SignatureRejected(
                    "file does not match its signed checksum — may be corrupted or tampered".to_string(),
                ));
            }
            signature::SigCheckResult::SignatureInvalid => {
                return Err(PluginLoadError::SignatureRejected(
                    "signature sidecar present but does not verify".to_string(),
                ));
            }
            signature::SigCheckResult::Malformed(reason) => {
                return Err(PluginLoadError::SignatureRejected(format!(
                    "signature sidecar present but malformed: {reason}"
                )));
            }
            signature::SigCheckResult::Unsigned
            | signature::SigCheckResult::Unrecognized
            | signature::SigCheckResult::Valid(_) => {}
        }

        let instance_pre = self.compile_and_link(path)?;
        let node_pre = self.detect_node_pre(instance_pre.clone())?;

        // Runtime `execute()` always goes through the plain `node`-only Pre --
        // every valid plugin exports it, with or without `metadata`. Cannot
        // fail: `detect_node_pre` above already proved `node` is exported by
        // this exact component.
        let exec_pre = wit::AeriniNodePre::new(instance_pre)
            .expect("node export already confirmed by detect_node_pre");

        let mut store = make_store(&self.engine, make_plugin_state(None));
        let raw = describe_via_pre(&node_pre, &mut store)?;

        // Leak type_id/display_name/description/icon/author/version once per plugin load —
        // bounded intentional leak: plugins are loaded once at process start and
        // live for the process lifetime. Only safe because this method is reached
        // once per plugin per process (via `load_plugins_from_dir`, called from
        // `load_plugins` through `shared()`) — a caller needing repeatable
        // metadata lookups must use `describe_plugin` instead, which runs the
        // identical steps but leaks nothing.
        let type_id: &'static str = Box::leak(raw.type_id.into_boxed_str());
        let display_name: &'static str = Box::leak(raw.display_name.into_boxed_str());
        let description: &'static str = Box::leak(raw.description.into_boxed_str());
        let icon: &'static str = Box::leak(raw.icon.into_boxed_str());
        let author: &'static str = Box::leak(raw.author.into_boxed_str());
        let version: &'static str = Box::leak(
            if raw.version.is_empty() { "1.0.0".to_string() } else { raw.version }
                .into_boxed_str(),
        );

        let node_type = category_to_node_type(&raw.category, type_id);

        // JSON Schema strings from the plugin — fall back to empty schema on parse failure.
        let input_schema: Value = serde_json::from_str(&raw.input_schema)
            .unwrap_or_else(|_| Value::Object(serde_json::Map::new()));
        let output_schema: Value = serde_json::from_str(&raw.output_schema)
            .unwrap_or_else(|_| Value::Object(serde_json::Map::new()));

        // `path`'s parent is `plugin_dir` in every real call path (see
        // `plugin_storage`'s own doc comment) — falls back to "." rather
        // than panicking on the near-impossible case of a bare relative
        // filename with no parent component.
        let storage_dir = path.parent().unwrap_or_else(|| Path::new("."));
        let storage = self.plugin_storage(storage_dir);
        let trigger_capable = self.probe_trigger_capable(path);

        Ok(Arc::new(WasmPluginNode {
            engine: self.engine.clone(),
            pre: exec_pre,
            type_id,
            display_name,
            description,
            node_type,
            input_schema,
            output_schema,
            icon,
            author,
            version,
            storage,
            trigger_capable,
        }))
    }

    /// Returns `type_id`/`display_name`/`node_type` for the plugin at `path`
    /// without registering it as a live [`Node`] and without leaking any
    /// memory — every string in the returned [`PluginDescriptor`] is owned.
    ///
    /// Runs the identical compile/link/detect/describe steps as
    /// [`load_plugin`](Self::load_plugin), but never calls [`Box::leak`] and
    /// never constructs a [`WasmPluginNode`] — the compiled component, linker,
    /// and store are all dropped when this method returns. Safe to call as
    /// many times as a caller needs, unlike `load_plugin`, whose leak is only
    /// bounded when called once per plugin per process. Discards the
    /// `metadata`-only fields (`icon`/`author`/`version`) — no current caller
    /// of `describe_plugin` needs them; see `PluginInfo` (Settings-tab
    /// listing), which doesn't carry an icon on either the Rust or TS side.
    pub fn describe_plugin(&self, path: &Path) -> Result<PluginDescriptor, PluginLoadError> {
        let instance_pre = self.compile_and_link(path)?;
        let node_pre = self.detect_node_pre(instance_pre)?;

        let mut store = make_store(&self.engine, make_plugin_state(None));
        let raw = describe_via_pre(&node_pre, &mut store)?;

        let node_type = category_to_node_type(&raw.category, &raw.type_id);

        Ok(PluginDescriptor {
            type_id: raw.type_id,
            display_name: raw.display_name,
            node_type,
        })
    }

    /// Load all `.wasm` files from `dir` as plugins.
    ///
    /// Files that fail to load are logged as warnings and skipped — they do not
    /// prevent other plugins from loading or crash the process.
    ///
    /// Returns an empty `Vec` if the directory cannot be read.
    pub fn load_plugins_from_dir(&self, dir: &Path) -> Vec<Arc<dyn Node>> {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(err) => {
                tracing::warn!(
                    "plugin_loader: cannot read plugin directory {}: {}",
                    dir.display(),
                    err
                );
                return vec![];
            }
        };

        let mut nodes: Vec<Arc<dyn Node>> = Vec::new();

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(err) => {
                    tracing::warn!("plugin_loader: error reading directory entry: {}", err);
                    continue;
                }
            };

            let path = entry.path();

            if path.extension().and_then(|e| e.to_str()) != Some("wasm") {
                continue;
            }

            match self.load_plugin(&path) {
                Ok(node) => {
                    tracing::info!(
                        "plugin_loader: loaded plugin '{}' from {}",
                        node.type_id(),
                        path.display()
                    );
                    nodes.push(node);
                }
                Err(err) => {
                    tracing::warn!(
                        "plugin_loader: skipping {}: {}",
                        path.display(),
                        err
                    );
                }
            }
        }

        nodes
    }

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
    async fn compile_trigger_component(&self, path: &Path) -> Result<wasmtime::component::Component, PluginLoadError> {
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
    fn compile_trigger_component_sync(&self, path: &Path) -> Result<wasmtime::component::Component, PluginLoadError> {
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
    fn probe_trigger_capable(&self, path: &Path) -> bool {
        let Ok(component) = self.compile_trigger_component_sync(path) else { return false };

        let mut linker = wasmtime::component::Linker::<TriggerPluginState>::new(&self.trigger_engine);
        if wasmtime_wasi::p3::add_to_linker(&mut linker).is_err() {
            return false;
        }
        if wasmtime_wasi_http::p3::add_to_linker(&mut linker).is_err() {
            return false;
        }
        if wit_storage::add_to_linker::<TriggerPluginState, wasmtime::component::HasSelf<TriggerPluginState>>(
            &mut linker,
            |state| state,
        ).is_err() {
            return false;
        }

        let Ok(instance_pre) = linker.instantiate_pre(&component) else { return false };
        wit_trigger::AeriniNodeWithTriggerPre::new(instance_pre).is_ok()
    }

    /// Scans `dir` for a `.wasm` file exporting `aerini-node-with-trigger`
    /// whose `describe().type_id` matches `type_id`, and returns it
    /// instantiated and ready for `events()` to be called on it.
    ///
    /// Every file that fails to compile or link against `trigger_engine` is
    /// skipped, not logged as a warning -- unlike `load_plugins_from_dir`'s
    /// directory scan, a "miss" here is the expected outcome for the
    /// majority of files in a plugin directory (every action-only plugin,
    /// built against WASIp2, cannot satisfy `wasmtime_wasi::p3`'s linker
    /// imports and will fail here every time this is called; that is not a
    /// problem to surface, just a non-match).
    async fn resolve_trigger_instance(
        &self,
        dir: &Path,
        type_id: &str,
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

        for entry in entries {
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("wasm") {
                continue;
            }

            let Ok(component) = self.compile_trigger_component(&path).await else { continue };

            let mut linker = wasmtime::component::Linker::<TriggerPluginState>::new(&self.trigger_engine);
            if wasmtime_wasi::p3::add_to_linker(&mut linker).is_err() {
                continue;
            }
            if wasmtime_wasi_http::p3::add_to_linker(&mut linker).is_err() {
                continue;
            }
            if wit_storage::add_to_linker::<TriggerPluginState, wasmtime::component::HasSelf<TriggerPluginState>>(
                &mut linker,
                |state| state,
            ).is_err() {
                continue;
            }

            let mut store = make_trigger_store(&self.trigger_engine, make_trigger_plugin_state(storage_handle.clone()));

            let Ok(bindings) = wit_trigger::AeriniNodeWithTrigger::instantiate_async(&mut store, &component, &linker).await else {
                continue;
            };

            // `describe` is declared as a plain (non-`async`) function in
            // `wit/node.wit`. bindgen's async support is per-function, not
            // per-world: only a WIT function actually marked `async func`
            // (`events`, below) gets an `Accessor`-based async call form --
            // `describe` still generates a synchronous call that returns its
            // `Result` directly rather than a future, even on an instance
            // whose world also happens to export an async interface.
            let Ok(descriptor) = bindings.aerini_plugin_node().call_describe(&mut store) else { continue };

            if descriptor.type_id == type_id {
                return Ok((store, bindings));
            }
        }

        Err(PluginLoadError::NoSuchTriggerPlugin(type_id.to_string()))
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
    pub async fn start_trigger(
        &self,
        dir: &Path,
        type_id: &str,
        config: &str,
    ) -> Result<(tokio::task::JoinHandle<()>, tokio::sync::mpsc::Receiver<Result<String, String>>), PluginLoadError> {
        let (mut store, bindings) = self.resolve_trigger_instance(dir, type_id).await?;
        let config = config.to_string();
        let (tx, rx) = tokio::sync::mpsc::channel(32);

        let handle = tokio::spawn(async move {
            let pump_tx = tx.clone();
            let outcome = store
                .run_concurrent(async move |accessor| -> wasmtime::Result<()> {
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

                    let consumer = TriggerEventConsumer { tx: tokio_util::sync::PollSender::new(pump_tx.clone()) };
                    if let Err(e) = accessor.with(|access| reader.pipe(access, consumer)) {
                        let _ = pump_tx.send(Err(e.to_string())).await;
                    }
                    Ok(())
                })
                .await;

            if let Err(e) = outcome.and_then(|inner| inner) {
                let _ = tx.send(Err(e.to_string())).await;
            }
        });

        Ok((handle, rx))
    }
}

// ── WasmPluginNode ────────────────────────────────────────────────────────────

/// A node implementation backed by a WASM component.
///
/// Registered in [`NodeRegistry`] identically to built-in nodes and appears in the
/// UI palette via the cached [`NodeDescriptor`](crate::node::NodeDescriptor) with
/// no frontend changes required.
///
/// # Execution isolation
///
/// Each `execute()` call creates a fresh [`wasmtime::Store<PluginState>`] from the
/// shared engine and the pre-compiled [`wit::AeriniNodePre`]. The `Store` is created
/// and consumed entirely within a `spawn_blocking` closure — it never crosses an
/// `await` point and is dropped at the end of the call.
///
/// # Input serialization
///
/// The executor provides `input.input` as a merged JSON object (config + resolved
/// credentials). This is serialized to `Vec<wit::Param>` key-value pairs and passed
/// to the plugin as `NodeInput.params`. `credentials` is left empty because the
/// executor has already resolved and merged credential values into `input.input`.
///
/// # Output deserialization
///
/// `NodeOutput.data` is a JSON string. On success it is parsed to `serde_json::Value`;
/// a parse failure is treated as a genuine, logged node failure (`wasm_invalid_output`),
/// not silently substituted with empty output.
///
/// # Static string fields
///
/// `type_id`, `display_name`, `icon`, `author`, and `version` are [`Box::leak`]ed
/// once at load time. This is a bounded, intentional leak: plugins are loaded
/// once at process start and never unloaded.
///
/// # Storage
///
/// `storage`, if present (see [`PluginLoader::plugin_storage`]), backs the
/// optional WIT `storage` import — scoped by this node's own `type_id` at
/// each `execute()` call (`wit/node.wit`'s `interface storage` doc comment
/// covers the scoping choice). `None` means the storage database for this
/// plugin's directory failed to open; every `storage.*` guest call then
/// degrades per that same doc comment rather than failing the node.
pub struct WasmPluginNode {
    /// Shared engine — cheap to clone (internally ref-counted).
    engine: Engine,
    /// Pre-compiled component with import-satisfaction already verified.
    /// `Clone + Send + Sync`. Re-instantiated per `execute()` call.
    pre: wit::AeriniNodePre<PluginState>,
    type_id: &'static str,
    display_name: &'static str,
    description: &'static str,
    node_type: NodeType,
    input_schema: Value,
    output_schema: Value,
    icon: &'static str,
    author: &'static str,
    version: &'static str,
    storage: Option<Arc<PluginStorage>>,
    trigger_capable: bool,
}

#[async_trait]
impl Node for WasmPluginNode {
    fn type_id(&self) -> &'static str {
        self.type_id
    }

    fn display_name(&self) -> &'static str {
        self.display_name
    }

    fn description(&self) -> &'static str {
        self.description
    }

    fn node_type(&self) -> NodeType {
        self.node_type.clone()
    }

    fn version(&self) -> &'static str {
        self.version
    }

    fn icon(&self) -> &'static str {
        self.icon
    }

    fn author(&self) -> &'static str {
        self.author
    }

    fn is_plugin(&self) -> bool {
        true
    }

    fn is_trigger_capable(&self) -> bool {
        self.trigger_capable
    }

    fn input_schema(&self) -> Value {
        self.input_schema.clone()
    }

    fn output_schema(&self) -> Value {
        self.output_schema.clone()
    }

    /// Execute the plugin node.
    ///
    /// Runs synchronous Wasmtime instantiation + function call in
    /// `tokio::task::spawn_blocking` so the async executor is never blocked.
    /// A fresh `Store` is created per call for full execution isolation.
    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let engine = self.engine.clone();
        let pre = self.pre.clone();
        let wit_input = engine_input_to_wit(&input);
        // Scoped by this node's own type_id, not by `input.node_id`/
        // `input.workflow_id` — see `wit/node.wit`'s `interface storage`
        // doc comment for why storage is per-plugin, not per-placement.
        let storage_handle = self.storage.clone().map(|db| PluginStorageHandle {
            db,
            scope: self.type_id.to_string(),
        });

        let result = tokio::task::spawn_blocking(move || {
            let mut store = make_store(&engine, make_plugin_state(storage_handle));

            let bindings = pre.instantiate(&mut store).map_err(|e| e.to_string())?;

            bindings
                .aerini_plugin_node()
                .call_execute(&mut store, &wit_input)
                .map_err(|e| e.to_string())
        })
        .await;

        match result {
            Ok(Ok(out)) => wit_output_to_engine(out),
            // Traps are unrecoverable: retrying the same input produces the same trap.
            Ok(Err(trap_msg)) => NodeOutput::failure(NodeError::unrecoverable(
                "wasm_trap",
                format!("plugin execution trapped: {trap_msg}"),
            )),
            Err(join_err) => NodeOutput::failure(NodeError::unrecoverable(
                "wasm_panic",
                format!("plugin task panicked: {join_err}"),
            )),
        }
    }
}

// ── Input / output conversion ─────────────────────────────────────────────────

/// Reserved `Param` key carrying the entire merged input as one JSON-encoded
/// object string (see `wit/node.wit`'s `node-input.params` doc comment). Must
/// match that doc comment exactly.
const FULL_INPUT_JSON_PARAM_KEY: &str = "__aerini_input_json";

/// Flatten the merged JSON input into a `Vec<Param>` for the WIT interface.
///
/// The executor merges config and resolved credentials into `input.input` as a
/// JSON object. Each top-level key becomes a `Param`. Non-string values are
/// serialized to their JSON text representation (e.g. numbers, booleans, objects) --
/// kept for plugins written before structured access existed. One additional
/// `FULL_INPUT_JSON_PARAM_KEY` entry carries the whole object as a single JSON
/// string, so a plugin can parse nested config once instead of re-parsing each
/// individually flattened field.
fn engine_input_to_wit(input: &NodeInput) -> wit::NodeInput {
    let params = match &input.input {
        Value::Object(map) => {
            let mut params: Vec<wit::Param> = map
                .iter()
                .map(|(k, v)| wit::Param {
                    key: k.clone(),
                    value: match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    },
                })
                .collect();

            if map.contains_key(FULL_INPUT_JSON_PARAM_KEY) {
                tracing::warn!(
                    "plugin_loader: node config already defines a field named {:?}; \
                     skipping the host-synthesized full-input JSON param to avoid \
                     shadowing the author's own value",
                    FULL_INPUT_JSON_PARAM_KEY
                );
            } else {
                params.push(wit::Param {
                    key: FULL_INPUT_JSON_PARAM_KEY.to_string(),
                    value: input.input.to_string(),
                });
            }

            params
        }
        _ => vec![],
    };

    wit::NodeInput {
        params,
        credentials: vec![], // already merged into input.input by the executor
    }
}

#[cfg(test)]
mod engine_input_to_wit_tests {
    use super::{engine_input_to_wit, FULL_INPUT_JSON_PARAM_KEY};
    use crate::model::{ExecutionContext, NodeInput};
    use serde_json::json;

    fn make_input(input: serde_json::Value) -> NodeInput {
        NodeInput {
            cancel_token: None,
            node_id: "n1".to_string(),
            workflow_id: "wf".to_string(),
            execution_id: "exec".to_string(),
            input,
            context: ExecutionContext::default(),
        }
    }

    /// Nested config must survive intact in the reserved full-JSON param,
    /// not just as the pre-existing per-key flattened text.
    #[test]
    fn nested_config_available_as_single_json_param() {
        let input = make_input(json!({
            "url": "https://example.com",
            "options": { "retries": 3, "headers": { "x-a": "b" } },
        }));

        let wit_input = engine_input_to_wit(&input);

        let full = wit_input
            .params
            .iter()
            .find(|p| p.key == FULL_INPUT_JSON_PARAM_KEY)
            .expect("reserved full-input param must be present");
        let parsed: serde_json::Value =
            serde_json::from_str(&full.value).expect("reserved param value must be valid JSON");
        assert_eq!(parsed, input.input, "reserved param must round-trip the whole merged input");

        // Pre-existing flattened behavior is unchanged for backward compat.
        let url = wit_input.params.iter().find(|p| p.key == "url").expect("url param missing");
        assert_eq!(url.value, "https://example.com");
    }

    /// A node config that happens to already use the reserved key must not be
    /// shadowed by the host-synthesized value -- the author's own field wins.
    #[test]
    fn existing_field_with_reserved_key_is_not_overwritten() {
        let input = make_input(json!({ FULL_INPUT_JSON_PARAM_KEY: "author-value" }));

        let wit_input = engine_input_to_wit(&input);

        let matches: Vec<_> =
            wit_input.params.iter().filter(|p| p.key == FULL_INPUT_JSON_PARAM_KEY).collect();
        assert_eq!(matches.len(), 1, "must not add a second entry under the same key");
        assert_eq!(matches[0].value, "author-value", "author's own value must win, not the synthesized JSON blob");
    }
}

/// Convert a WIT `NodeOutput` record to the engine's [`NodeOutput`].
fn wit_output_to_engine(out: wit::NodeOutput) -> NodeOutput {
    if out.success {
        match serde_json::from_str::<Value>(&out.data) {
            Ok(value) => NodeOutput::success(value),
            Err(parse_err) => {
                // A plugin reporting success with non-JSON `data` is a hard failure,
                // not an empty result — logged here so it's distinguishable from a
                // plugin that legitimately returns no data.
                let preview: String = out.data.chars().take(INVALID_OUTPUT_LOG_PREVIEW_CHARS).collect();
                tracing::error!(
                    "plugin_loader: plugin reported success but `data` is not valid JSON: {} (data preview: {:?})",
                    parse_err,
                    preview
                );
                NodeOutput::failure(NodeError::unrecoverable(
                    "wasm_invalid_output",
                    format!(
                        "plugin reported success but its output was not valid JSON: {parse_err}"
                    ),
                ))
            }
        }
    } else {
        NodeOutput::failure(NodeError {
            code: out.error_code,
            message: out.error_message,
            recoverable: out.recoverable,
        })
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Outcome of one [`load_plugins`] call, keyed by `type_id` — lets a caller
/// surface what happened (e.g. in a UI) instead of only reading the log.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct PluginLoadReport {
    /// `type_id`s that ended up registered (including the winner of a collision).
    pub loaded: Vec<String>,
    /// `type_id`s rejected outright for colliding with a built-in node.
    pub builtin_rejected: Vec<String>,
    /// `type_id`s claimed by more than one plugin file; the last one loaded won.
    pub plugin_collisions: Vec<String>,
}

/// Load all WASM plugins from `plugin_dir` and register them in `registry`.
///
/// Constructs a [`PluginLoader`] (and a Wasmtime engine) internally.
/// Called by `aerini-server` and `src-tauri` after [`crate::nodes::register_builtins`].
/// Seals the built-in namespace before registering any plugins so that a plugin
/// whose `type_id` collides with a built-in is rejected rather than silently replacing it.
///
/// If the Wasmtime engine fails to initialise, an error is logged and the function
/// returns an empty report without registering any plugins — the process continues normally.
pub fn load_plugins(registry: &mut NodeRegistry, plugin_dir: &Path) -> PluginLoadReport {
    // Seal the built-in namespace before loading any plugins so that a plugin
    // cannot shadow a built-in node type (e.g. "http_request", "shell_exec").
    registry.seal_builtins();

    // SECURITY NOTICE: WASM plugins' outbound HTTP (wasi:http/outgoing-handler,
    // both action and trigger plugins) is checked against the same SSRF policy
    // as the Database and HTTP nodes before any request is sent (see
    // `check_ssrf_uri`) — RFC 1918, loopback, link-local, and cloud metadata
    // addresses are rejected. That check has the
    // same DNS-rebinding TOCTOU gap documented on `nodes::util::check_host_ssrf`,
    // and a plugin still runs with the full trust of whatever else this process
    // can reach once a request clears it. Treat the plugin directory as a trust
    // boundary equivalent to running arbitrary native code, and enforce
    // network-level egress filtering as defence-in-depth for the TOCTOU gap in
    // server/API deployments.
    tracing::warn!(
        "plugin_loader: loading WASM plugins from '{}'. Outbound HTTP is SSRF-filtered (RFC1918/loopback/link-local/cloud-metadata blocked), but only load plugins from trusted sources — a DNS-rebinding TOCTOU gap remains; enforce network-level egress filtering as defence-in-depth.",
        plugin_dir.display()
    );

    let loader = match PluginLoader::shared() {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(
                "plugin_loader: failed to initialise Wasmtime engine: {}",
                e
            );
            return PluginLoadReport::default();
        }
    };

    let nodes = loader.load_plugins_from_dir(plugin_dir);
    let mut report = PluginLoadReport::default();
    // register_plugin already emits a WARN log for both branches below.
    for node in nodes {
        let id = node.type_id().to_string();
        match registry.register_plugin(node) {
            Ok(true) => report.loaded.push(id),
            Ok(false) => {
                report.loaded.push(id.clone());
                report.plugin_collisions.push(id);
            }
            Err(_) => report.builtin_rejected.push(id),
        }
    }

    if !report.plugin_collisions.is_empty() {
        tracing::warn!(
            "plugin_loader: {} plugin node(s) had a type_id collision with another plugin (last-loaded wins) from {}",
            report.plugin_collisions.len(),
            plugin_dir.display()
        );
    }
    if !report.builtin_rejected.is_empty() {
        tracing::warn!(
            "plugin_loader: {} plugin node(s) rejected (built-in type_id collision) from {}",
            report.builtin_rejected.len(),
            plugin_dir.display()
        );
    }
    tracing::info!(
        "plugin_loader: loaded {} plugin node(s) from {}",
        report.loaded.len(),
        plugin_dir.display()
    );
    report
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn loader() -> PluginLoader {
        PluginLoader::new().expect("Wasmtime engine init failed in test")
    }

    /// Non-existent path must return `PluginLoadError::Io`.
    #[test]
    fn load_plugin_nonexistent_file_returns_io_error() {
        let l = loader();
        let result = l.load_plugin(Path::new("/nonexistent/path/to/plugin.wasm"));
        assert!(result.is_err(), "expected Io error, got Ok");
        let err = result.err().unwrap();
        assert!(
            matches!(err, PluginLoadError::Io(_)),
            "expected Io, got: {err:?}"
        );
    }

    /// A valid WASM component that does NOT export the `aerini-node` world must
    /// return `PluginLoadError::MissingInterface`.
    ///
    /// Uses `(component)` — the smallest valid Component Model component. It has no
    /// exports and no imports, so `linker.instantiate_pre` succeeds (no unsatisfied
    /// imports) but `AeriniNodePre::new` fails (no `aerini:plugin/node` export).
    #[test]
    fn load_plugin_empty_component_returns_missing_interface() {
        let l = loader();

        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile create failed");
        tmp.write_all(b"(component)").expect("tempfile write failed");
        tmp.flush().expect("tempfile flush failed");

        let result = l.load_plugin(tmp.path());
        assert!(result.is_err(), "expected MissingInterface error, got Ok");
        let err = result.err().unwrap();
        assert!(
            matches!(err, PluginLoadError::MissingInterface),
            "expected MissingInterface, got: {err:?}"
        );
    }

    /// Normal case: `load_plugin` must reject a `.wasm` file whose sidecar
    /// signature declares a content hash that doesn't match the file —
    /// before any compilation is attempted, not just logged and let through.
    #[test]
    fn load_plugin_rejects_tampered_signature() {
        use base64::{engine::general_purpose::STANDARD as B64, Engine as _};

        let l = loader();
        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile create failed");
        tmp.write_all(b"(component)").expect("tempfile write failed");
        tmp.flush().expect("tempfile flush failed");

        let sig_path = signature::sig_sidecar_path(tmp.path());
        let sig_json = serde_json::json!({
            "schema_version": 1,
            "algorithm": "ed25519",
            "public_key": B64.encode([0u8; 32]),
            "files": [{"name": "plugin.wasm", "blake3": "0".repeat(64)}],
            "signature": B64.encode([0u8; 64]),
        })
        .to_string();
        std::fs::write(&sig_path, sig_json).expect("sig sidecar write failed");

        let result = l.load_plugin(tmp.path());
        let err = result.err().expect("expected the tampered signature to reject the load");
        assert!(
            matches!(err, PluginLoadError::SignatureRejected(_)),
            "expected SignatureRejected, got: {err:?}"
        );

        let _ = std::fs::remove_file(&sig_path);
    }

    /// Edge case: repeated `verify_signature` calls against the same,
    /// unchanged file must hash it exactly once, not on every call — same
    /// fingerprint-cache contract `component_cache` already provides,
    /// checked here via `signature_check_count` instead of timing.
    #[test]
    fn verify_signature_reuses_cached_result_for_unchanged_file() {
        let l = loader();
        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile create failed");
        tmp.write_all(b"(component)").expect("tempfile write failed");
        tmp.flush().expect("tempfile flush failed");

        for _ in 0..3 {
            let _ = l.verify_signature(tmp.path());
        }

        assert_eq!(
            l.signature_check_count(),
            1,
            "expected exactly one real signature check across three verify_signature calls on an unchanged file"
        );
    }

    /// `describe_plugin` must classify errors identically to `load_plugin`
    /// for the same non-existent-path input — the non-leaking path must not
    /// silently swallow or misclassify an error `load_plugin` already
    /// handles correctly.
    #[test]
    fn describe_plugin_nonexistent_file_returns_io_error() {
        let l = loader();
        let result = l.describe_plugin(Path::new("/nonexistent/path/to/plugin.wasm"));
        assert!(result.is_err(), "expected Io error, got Ok");
        let err = result.err().unwrap();
        assert!(
            matches!(err, PluginLoadError::Io(_)),
            "expected Io, got: {err:?}"
        );
    }

    /// `describe_plugin` must classify a world-less component identically to
    /// `load_plugin` (same mechanism as `load_plugin_empty_component_returns_missing_interface`
    /// above — see that test's doc comment for why `(component)` triggers this path).
    #[test]
    fn describe_plugin_empty_component_returns_missing_interface() {
        let l = loader();

        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile create failed");
        tmp.write_all(b"(component)").expect("tempfile write failed");
        tmp.flush().expect("tempfile flush failed");

        let result = l.describe_plugin(tmp.path());
        assert!(result.is_err(), "expected MissingInterface error, got Ok");
        let err = result.err().unwrap();
        assert!(
            matches!(err, PluginLoadError::MissingInterface),
            "expected MissingInterface, got: {err:?}"
        );
    }

    /// `PluginLoader::shared()` must return the same
    /// process-wide instance on every call, not construct a fresh `Engine` +
    /// epoch-ticker thread each time (each such construction is a leak if
    /// nothing ever tears it down — the exact failure mode a call site using
    /// its own `PluginLoader::new()` instead of `shared()` would hit).
    /// Pointer equality on the returned `&'static PluginLoader` is a direct
    /// proxy for "no second construction happened": `OnceLock::get_or_init`
    /// only ever runs its initializer once, so two `Ok` results can only
    /// share an address if they came from the same underlying
    /// `PluginLoader`.
    #[test]
    fn shared_returns_same_instance_across_repeated_calls() {
        let a = PluginLoader::shared().expect("shared() should succeed on a normal host");
        let b = PluginLoader::shared().expect("shared() should succeed on a normal host");
        let c = PluginLoader::shared().expect("shared() should succeed on a normal host");
        assert!(
            std::ptr::eq(a, b) && std::ptr::eq(b, c),
            "shared() must return the same PluginLoader on every call, not construct a new one"
        );
    }

    /// `describe_plugin` must agree with `load_plugin` on the metadata it
    /// extracts for the same input — this only has an error path to compare
    /// in this test module (no valid `aerini-node`-exporting fixture is
    /// available without a real compiled plugin), so this asserts the two
    /// paths at least agree on failure classification for a second distinct
    /// error shape (`WasmCompile`, via a byte string that is neither valid
    /// WAT text nor a valid WASM binary).
    #[test]
    fn describe_plugin_and_load_plugin_agree_on_invalid_bytes() {
        let l = loader();

        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile create failed");
        tmp.write_all(b"not a wasm module or wat text").expect("tempfile write failed");
        tmp.flush().expect("tempfile flush failed");

        let load_err = l.load_plugin(tmp.path()).err().expect("load_plugin should fail");
        let describe_err = l.describe_plugin(tmp.path()).err().expect("describe_plugin should fail");

        assert!(matches!(load_err, PluginLoadError::WasmCompile(_)));
        assert!(matches!(describe_err, PluginLoadError::WasmCompile(_)));
    }

    /// repeated `describe_plugin` calls against the same, unchanged file must
    /// compile the `.wasm` bytes exactly once, not on every call. Uses a
    /// per-instance compile counter (see `compile_count`) rather than
    /// timing, so this can't be flaky.
    #[test]
    fn describe_plugin_reuses_cached_component_for_unchanged_file() {
        let l = loader();

        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile create failed");
        tmp.write_all(b"(component)").expect("tempfile write failed");
        tmp.flush().expect("tempfile flush failed");

        for _ in 0..3 {
            // Every call returns MissingInterface (no export) — only the
            // compile *count*, not the result, is under test here.
            let _ = l.describe_plugin(tmp.path());
        }

        assert_eq!(
            l.compile_count(),
            1,
            "expected exactly one real compile across three describe_plugin calls on an unchanged file"
        );
    }

    /// Companion to the above: if the file at the same path actually
    /// changes (different length — covers a plugin removed and a
    /// different one reinstalled under the same filename), the cache must
    /// not serve stale bytes — a fresh compile is required.
    #[test]
    fn describe_plugin_recompiles_when_file_content_changes() {
        let l = loader();

        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile create failed");
        tmp.write_all(b"(component)").expect("tempfile write failed");
        tmp.flush().expect("tempfile flush failed");

        let _ = l.describe_plugin(tmp.path());
        assert_eq!(l.compile_count(), 1);

        // Truncate and rewrite with different (longer) content so the
        // (mtime, len) fingerprint changes regardless of filesystem mtime
        // resolution.
        use std::io::{Seek, SeekFrom};
        tmp.as_file_mut().set_len(0).expect("truncate failed");
        tmp.as_file_mut().seek(SeekFrom::Start(0)).expect("seek failed");
        tmp.write_all(b"(component (core module (memory 1) (data (i32.const 100) \"abcd\")))")
            .expect("tempfile rewrite failed");
        tmp.flush().expect("tempfile flush failed");

        let _ = l.describe_plugin(tmp.path());
        assert_eq!(
            l.compile_count(),
            2,
            "file content/length changed at the same path — must recompile, not reuse the stale cached Component"
        );
    }

    /// Async entry point must classify a missing file identically to the
    /// sync action-plugin path — `Io`, not `WasmCompile` — even though the
    /// read now happens inside a spawned blocking task.
    #[tokio::test]
    async fn compile_trigger_component_nonexistent_file_returns_io_error() {
        let l = loader();
        let result = l.compile_trigger_component(Path::new("/nonexistent/path/to/plugin.wasm")).await;
        assert!(result.is_err(), "expected Io error, got Ok");
        assert!(
            matches!(result.err().unwrap(), PluginLoadError::Io(_)),
            "expected Io error classification to survive the spawn_blocking offload"
        );
    }

    /// The async (`spawn_blocking`-backed) and sync compile entry points
    /// share one cache: compiling via one and then the other for the same
    /// unchanged file must not trigger a second real compile.
    #[tokio::test]
    async fn compile_trigger_component_reuses_cache_across_sync_and_async_paths() {
        let l = loader();
        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile create failed");
        tmp.write_all(b"(component)").expect("tempfile write failed");
        tmp.flush().expect("tempfile flush failed");

        let _ = l.compile_trigger_component_sync(tmp.path());
        let _ = l.compile_trigger_component(tmp.path()).await;

        assert_eq!(
            l.trigger_compile_count(),
            1,
            "expected exactly one real compile across the sync and async entry points for an unchanged file"
        );
    }

    /// Normal case: `TriggerPluginState`'s `StorageHost` delegates to the
    /// backing `PluginStorage` exactly like `PluginState`'s does — a value
    /// set through the handle is visible through a `get` on the same
    /// instance.
    #[test]
    fn trigger_plugin_state_storage_round_trips_through_handle() {
        use wit_storage::StorageHost as _;
        let dir = tempfile::tempdir().expect("tempdir create failed");
        let db = Arc::new(PluginStorage::open(&dir.keep()).expect("PluginStorage::open failed"));
        let mut state = make_trigger_plugin_state(Some(PluginStorageHandle {
            db,
            scope: "trigger-plugin-a".to_string(),
        }));

        assert_eq!(state.get("k1".to_string()), None);
        state.set("k1".to_string(), "v1".to_string()).expect("set failed");
        assert_eq!(state.get("k1".to_string()), Some("v1".to_string()));
    }

    /// Edge case: a `None` storage handle (directory's storage DB
    /// unavailable) must degrade exactly per `wit/node.wit`'s documented
    /// contract — `get` empty, `set`/`list_keys` return `Unavailable`,
    /// `delete` a silent no-op — never panic.
    #[test]
    fn trigger_plugin_state_storage_degrades_when_handle_is_none() {
        use wit_storage::StorageHost as _;
        let mut state = make_trigger_plugin_state(None);

        assert_eq!(state.get("k1".to_string()), None);
        assert!(matches!(
            state.set("k1".to_string(), "v1".to_string()),
            Err(wit_storage::StorageError::Unavailable)
        ));
        assert!(matches!(
            state.list_keys("".to_string()),
            Err(wit_storage::StorageError::Unavailable)
        ));
        state.delete("k1".to_string()); // must not panic
    }

    /// `probe_trigger_capable`'s throwaway linker also links `storage` and
    /// `wasi:http` p3 alongside `wasi:p3` — this must not change its answer
    /// for a component that imports none of them: still `false`, since it
    /// doesn't export `trigger` either.
    #[test]
    fn probe_trigger_capable_returns_false_for_component_without_trigger_export() {
        let l = loader();
        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile create failed");
        tmp.write_all(b"(component)").expect("tempfile write failed");
        tmp.flush().expect("tempfile flush failed");

        assert!(!l.probe_trigger_capable(tmp.path()));
    }
}
