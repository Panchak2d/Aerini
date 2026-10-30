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

    #[error("no plugin exporting `aerini-node-with-trigger` with type_id \"{type_id}\" found in the plugin directory; scan incomplete: {detail}")]
    TriggerScanIncomplete { type_id: String, detail: String },

    #[error("plugin ran for {0}s without returning and was stopped")]
    CallTimedOut(u64),

    #[error("plugin signature check failed: {0}")]
    SignatureRejected(String),

    /// Kept for API compatibility; not returned by any current code path.
    #[error("not yet implemented")]
    #[allow(dead_code)]
    NotImplemented,
}
