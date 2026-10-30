use std::path::Path;
use std::sync::Arc;

use serde_json::Value;

use crate::model::NodeType;
use crate::node::Node;
use crate::plugin_loader::error::PluginLoadError;
use crate::plugin_loader::node::{NodeBackend, WasmPluginNode};
use super::{PluginDescriptor, PluginLoader};

/// True for a path with a `.wasm` extension in any letter case, matching how
/// the install and listing commands recognise plugin files.
pub(super) fn is_wasm_file_name(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("wasm"))
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
        self.require_acceptable_signature(path)?;

        let (raw, backend) = self.describe_linked(path)?;

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
        let trigger_capable =
            matches!(backend, NodeBackend::Trigger { .. }) || self.probe_trigger_capable(path);

        Ok(Arc::new(WasmPluginNode {
            backend,
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
            fs_watch_registry: self.fs_watch_registry.clone(),
            trigger_capable,
        }))
    }

    /// Returns `type_id`/`display_name`/`node_type` for the plugin at `path`
    /// without registering it as a live [`Node`] and without leaking any
    /// memory — every string in the returned [`PluginDescriptor`] is owned.
    ///
    /// Checks the signature sidecar first, like [`load_plugin`](Self::load_plugin),
    /// so a rejected file is never compiled or run. Runs the identical
    /// compile/link/detect/describe steps as `load_plugin`, but never calls [`Box::leak`] and
    /// never constructs a [`WasmPluginNode`] — the compiled component, linker,
    /// and store are all dropped when this method returns. Safe to call as
    /// many times as a caller needs, unlike `load_plugin`, whose leak is only
    /// bounded when called once per plugin per process. Discards the
    /// `metadata`-only fields (`icon`/`author`/`version`) — no current caller
    /// of `describe_plugin` needs them; see `PluginInfo` (Settings-tab
    /// listing), which doesn't carry an icon on either the Rust or TS side.
    pub fn describe_plugin(&self, path: &Path) -> Result<PluginDescriptor, PluginLoadError> {
        self.require_acceptable_signature(path)?;
        let (raw, _backend) = self.describe_linked(path)?;

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

            if !is_wasm_file_name(&path) {
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
}
