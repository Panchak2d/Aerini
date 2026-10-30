use crate::plugin_loader::bindings::{wit, wit_metadata};
use crate::plugin_loader::error::PluginLoadError;
use crate::plugin_loader::state::PluginState;

/// A plugin's pre-instantiated bindings, in whichever of the two `aerini:plugin`
/// world shapes it actually exports. Produced by
/// [`PluginLoader::detect_node_pre`](PluginLoader::detect_node_pre).
pub(super) enum NodePre {
    /// Exports only `node` (`describe`/`execute`) -- the pre-`metadata` shape.
    Plain(wit::AeriniNodePre<PluginState>),
    /// Exports `node` and `metadata` (`describe-metadata`).
    WithMetadata(wit_metadata::AeriniNodeWithMetadataPre<PluginState>),
}

/// World-agnostic result of calling `describe()` (and `describe-metadata()`, when
/// the plugin exports it) once against an instantiated plugin.
pub(super) struct RawDescribe {
    pub(super) type_id: String,
    pub(super) display_name: String,
    pub(super) category: String,
    pub(super) description: String,
    pub(super) input_schema: String,
    pub(super) output_schema: String,
    /// Empty when the plugin doesn't export `metadata`, or exports it but left
    /// this field unset -- both mean "use the host default" (see
    /// `wit/node.wit`'s doc comment).
    pub(super) version: String,
    pub(super) author: String,
    pub(super) icon: String,
}

/// Instantiates `node_pre` and extracts everything callers need from `describe()`
/// (and `describe-metadata()`, when the world supports it) into one owned,
/// world-agnostic result. `describe()` is called through `node_pre`'s own bindings
/// rather than a separately-constructed plain `AeriniNodePre` so a `WithMetadata`
/// plugin is instantiated exactly once for this call, not twice.
pub(super) fn describe_via_pre(
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
