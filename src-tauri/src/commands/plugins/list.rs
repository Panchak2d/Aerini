use std::collections::HashMap;
use std::path::PathBuf;

use aerini_engine::model::NodeType;
use aerini_engine::plugin_loader::{PluginLoader, PluginLoadReport};
use serde::Serialize;

use super::pack::scan_installed_packs;
use super::trust::{load_trust_store, signature_status_message};

#[derive(Serialize)]
pub struct PluginInfo {
    pub filename: String,
    pub display_name: String,
    pub type_id: String,
    pub category: String,
    pub load_error: Option<String>,
    /// What happened when this plugin's `type_id` was registered into the live
    /// engine at last startup — `None` when it registered cleanly. Independent
    /// of `load_error`: a file can describe fine here and still have lost a
    /// registry-level collision, or been rejected for shadowing a built-in.
    pub registry_warning: Option<String>,
    /// Human-readable publisher-signature status for this installed file —
    /// e.g. "Verified — trusted publisher" or "Unsigned — no publisher
    /// signature". `None` only when the file failed to describe at all (see
    /// `load_error`), so there was nothing to check a signature against.
    pub signature_status: Option<String>,
    /// `pack_id` of the multi-node package this file is a member of, if any
    /// — cross-referenced from every `*.aerini-pack.json` manifest in
    /// `plugin_dir`, independent of whether this file itself describes
    /// successfully. `None` for a standalone (non-packaged) plugin file.
    pub pack_id: Option<String>,
    /// Display name of the owning package, paired with `pack_id` above.
    pub pack_display_name: Option<String>,
}

fn node_type_to_category(nt: &NodeType) -> &'static str {
    match nt {
        NodeType::Action => "action",
        NodeType::Ai => "ai",
        NodeType::Logic => "logic",
        NodeType::Utility => "utility",
    }
}

pub(super) fn list_installed_plugins_blocking(plugin_dir: String, load_report: &PluginLoadReport) -> Result<Vec<PluginInfo>, String> {
    let dir = PathBuf::from(&plugin_dir);

    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Ok(Vec::new()),
    };

    // `PluginLoader::shared()` reuses the process-wide `Engine` and epoch-ticker
    // thread rather than constructing a fresh pair per call, since this command
    // runs every time a user opens or refreshes the Plugins settings panel.
    // `describe_plugin` (not `load_plugin`) is used because it never leaks the
    // `type_id`/`display_name` strings that `load_plugin` intentionally leaks
    // for its one-per-process caller.
    let loader = PluginLoader::shared()?;
    let trust = load_trust_store(&dir);
    let mut out = Vec::new();

    // filename -> (pack_id, display_name), cross-referenced from every
    // installed pack's manifest so a member's `PluginInfo` can carry its
    // owning pack regardless of whether the member itself describes
    // successfully.
    let mut pack_membership: HashMap<String, (String, String)> = HashMap::new();
    for (pack_id, manifest) in scan_installed_packs(&dir) {
        for member in &manifest.files {
            pack_membership.insert(member.clone(), (pack_id.clone(), manifest.display_name.clone()));
        }
    }

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("wasm")) != Some(true) {
            continue;
        }
        let filename = match path.file_name().and_then(|f| f.to_str()) {
            Some(f) => f.to_string(),
            None => continue,
        };
        let (pack_id, pack_display_name) = match pack_membership.get(&filename) {
            Some((id, name)) => (Some(id.clone()), Some(name.clone())),
            None => (None, None),
        };

        match loader.describe_plugin(&path) {
            Ok(desc) => {
                let registry_warning = if load_report.builtin_rejected.contains(&desc.type_id) {
                    Some(format!("'{}' conflicts with a built-in node — not active until renamed", desc.type_id))
                } else if load_report.plugin_collisions.contains(&desc.type_id) {
                    Some(format!("'{}' is also claimed by another installed plugin", desc.type_id))
                } else {
                    None
                };
                let signature_status = std::fs::read(&path)
                    .ok()
                    .map(|bytes| signature_status_message(&path, &bytes, &trust, &desc.type_id));
                out.push(PluginInfo {
                    filename,
                    display_name: desc.display_name,
                    type_id: desc.type_id,
                    category: node_type_to_category(&desc.node_type).to_string(),
                    load_error: None,
                    registry_warning,
                    signature_status,
                    pack_id,
                    pack_display_name,
                });
            }
            Err(e) => out.push(PluginInfo {
                filename: filename.clone(),
                display_name: filename,
                type_id: String::new(),
                category: String::new(),
                load_error: Some(e.to_string()),
                registry_warning: None,
                signature_status: None,
                pack_id,
                pack_display_name,
            }),
        }
    }

    out.sort_by(|a, b| a.display_name.cmp(&b.display_name));
    Ok(out)
}
