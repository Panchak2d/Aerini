use std::path::Path;

use crate::node::NodeRegistry;
use crate::plugin_loader::loader::PluginLoader;

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
    // both action and trigger plugins) resolves the host once, rejects any
    // RFC 1918, loopback, link-local or cloud-metadata address, and connects to
    // the validated addresses (see `plugin_http`). A plugin still runs with the
    // full trust of whatever else this process can reach once a request clears
    // that filter, so treat the plugin directory as a trust boundary equivalent
    // to running arbitrary native code.
    tracing::warn!(
        "plugin_loader: loading WASM plugins from '{}'. Outbound HTTP is SSRF-filtered and pinned to the validated address (RFC1918/loopback/link-local/cloud-metadata blocked), but only load plugins from trusted sources.",
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
