use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use wasmtime::Engine;

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::Node;
use crate::plugin_loader::bindings::wit;
use crate::plugin_loader::convert::{engine_input_to_wit, wit_output_to_engine};
use crate::plugin_loader::fs_watch::{FsWatchRegistry, FsWatchScope, PluginFsWatchHandle};
use crate::plugin_loader::limits::{PLUGIN_EXECUTE_TIMEOUT, PLUGIN_TRIGGER_CALL_TIMEOUT};
use crate::plugin_loader::state::{make_plugin_state, make_store, PluginState};
use crate::plugin_loader::storage::{PluginStorage, PluginStorageHandle};
use crate::plugin_loader::trigger::TriggerPre;
use crate::plugin_loader::trigger::execute::{run_trigger_execute, wit_input_to_trigger};
use crate::plugin_loader::trigger::state::{make_trigger_plugin_state, make_trigger_store};

/// Where a loaded plugin's `execute()` runs.
pub(super) enum NodeBackend {
    Sync { engine: Engine, pre: wit::AeriniNodePre<PluginState> },
    Trigger { pre: TriggerPre },
}

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
/// to the plugin as `NodeInput.params` -- kept for backward compatibility with
/// existing plugins. The same resolved secrets are also passed separately, keyed
/// by config field name, as `NodeInput.credentials`, via `input.resolved_credentials`.
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
    /// Pre-compiled component with import-satisfaction already verified, on
    /// whichever engine can host it. `Clone + Send + Sync`. Re-instantiated per
    /// `execute()` call.
    pub(super) backend: NodeBackend,
    pub(super) type_id: &'static str,
    pub(super) display_name: &'static str,
    pub(super) description: &'static str,
    pub(super) node_type: NodeType,
    pub(super) input_schema: Value,
    pub(super) output_schema: Value,
    pub(super) icon: &'static str,
    pub(super) author: &'static str,
    pub(super) version: &'static str,
    pub(super) storage: Option<Arc<PluginStorage>>,
    /// Backing for the WIT `fs-watch` import -- see [`PluginFsWatchHandle`]
    /// and `wit/node.wit`'s `interface fs-watch` doc comment. Unlike
    /// `storage`, always present: constructing [`FsWatchRegistry`] cannot
    /// fail the way opening `storage`'s SQLite file can.
    pub(super) fs_watch_registry: Arc<FsWatchRegistry>,
    pub(super) trigger_capable: bool,
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
    /// On the synchronous engine, runs Wasmtime instantiation + the function
    /// call in `tokio::task::spawn_blocking` so the async executor is never
    /// blocked. A trigger plugin served from `trigger_engine` is awaited
    /// directly through the `_async` APIs, bounded by the epoch deadline and
    /// [`PLUGIN_TRIGGER_CALL_TIMEOUT`]. A fresh `Store` is created per call
    /// either way, for full execution isolation.
    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let wit_input = engine_input_to_wit(&input);
        // Scoped by this node's own type_id, not by `input.node_id`/
        // `input.workflow_id` — see `wit/node.wit`'s `interface storage`
        // doc comment for why storage is per-plugin, not per-placement.
        let storage_handle = self.storage.clone().map(|db| PluginStorageHandle {
            db,
            scope: self.type_id.to_string(),
        });
        // Same type_id scoping as `storage_handle` above — see `wit/node.wit`'s
        // `interface fs-watch` doc comment.
        let fs_watch_handle = Some(PluginFsWatchHandle {
            registry: self.fs_watch_registry.clone(),
            plugin_type_id: self.type_id.to_string(),
            scope: FsWatchScope::from_metadata(&input.context.metadata),
        });

        let result = match &self.backend {
            NodeBackend::Sync { engine, pre } => {
                let engine = engine.clone();
                let pre = pre.clone();
                let call = tokio::task::spawn_blocking(move || {
                    let mut store = make_store(&engine, make_plugin_state(storage_handle, fs_watch_handle));

                    let bindings = pre.instantiate(&mut store).map_err(|e| e.to_string())?;

                    bindings
                        .aerini_plugin_node()
                        .call_execute(&mut store, &wit_input)
                        .map_err(|e| e.to_string())
                });
                // The epoch deadline cannot interrupt a blocking host call, so
                // the wait is bounded here too. The thread itself ends when its
                // current host call returns and the guest hits the deadline.
                match tokio::time::timeout(PLUGIN_EXECUTE_TIMEOUT, call).await {
                    Ok(joined) => joined,
                    Err(_elapsed) => Ok(Err(format!(
                        "plugin ran for {}s without returning and was stopped",
                        PLUGIN_EXECUTE_TIMEOUT.as_secs()
                    ))),
                }
            }
            NodeBackend::Trigger { pre } => {
                let trigger_input = wit_input_to_trigger(&wit_input);
                let mut store = make_trigger_store(
                    pre.engine(),
                    make_trigger_plugin_state(storage_handle, fs_watch_handle),
                );
                let outcome = match tokio::time::timeout(
                    PLUGIN_TRIGGER_CALL_TIMEOUT,
                    run_trigger_execute(pre, &mut store, &trigger_input),
                )
                .await
                {
                    Ok(Ok(out)) => Ok(out),
                    Ok(Err(e)) => Err(e.to_string()),
                    Err(_elapsed) => Err(format!(
                        "plugin ran for {}s without returning and was stopped",
                        PLUGIN_TRIGGER_CALL_TIMEOUT.as_secs()
                    )),
                };
                Ok(outcome)
            }
        };

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
