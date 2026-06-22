use std::path::{Path, PathBuf};

use aerini_engine::model::NodeType;
use aerini_engine::plugin_loader::PluginLoader;
use serde::Serialize;

#[derive(Serialize)]
pub struct PluginInfo {
    pub filename: String,
    pub display_name: String,
    pub type_id: String,
    pub category: String,
    pub load_error: Option<String>,
}

fn node_type_to_category(nt: &NodeType) -> &'static str {
    match nt {
        NodeType::Action => "action",
        NodeType::Ai => "ai",
        NodeType::Logic => "logic",
        NodeType::Utility => "utility",
    }
}

/// Lists all `.wasm` files in `plugin_dir`. Files that fail to load are still
/// returned with `load_error` set so the UI can surface broken plugins.
/// A non-existent `plugin_dir` returns an empty list, not an error.
#[tauri::command]
pub async fn list_installed_plugins(plugin_dir: String) -> Result<Vec<PluginInfo>, String> {
    let dir = PathBuf::from(&plugin_dir);

    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Ok(Vec::new()),
    };

    let loader = PluginLoader::new().map_err(|e| e.to_string())?;
    let mut out = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("wasm")) != Some(true) {
            continue;
        }
        let filename = match path.file_name().and_then(|f| f.to_str()) {
            Some(f) => f.to_string(),
            None => continue,
        };

        match loader.load_plugin(&path) {
            Ok(node) => out.push(PluginInfo {
                filename,
                display_name: node.display_name().to_string(),
                type_id: node.type_id().to_string(),
                category: node_type_to_category(&node.node_type()).to_string(),
                load_error: None,
            }),
            Err(e) => out.push(PluginInfo {
                filename: filename.clone(),
                display_name: filename,
                type_id: String::new(),
                category: String::new(),
                load_error: Some(e.to_string()),
            }),
        }
    }

    out.sort_by(|a, b| a.display_name.cmp(&b.display_name));
    Ok(out)
}

/// Copies the `.wasm` file at `src_path` into `plugin_dir`, creating the
/// directory if needed. Refuses to overwrite an existing file with the same
/// name. Returns the destination filename.
#[tauri::command]
pub async fn install_plugin_from_path(src_path: String, plugin_dir: String) -> Result<String, String> {
    let src = Path::new(&src_path);

    if src.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("wasm")) != Some(true) {
        return Err("Selected file is not a .wasm file".to_string());
    }
    if !src.is_file() {
        return Err(format!("Source file not found: {src_path}"));
    }

    // Strip any directory components — only the bare filename is used for the
    // destination, preventing path traversal via a crafted src_path.
    let filename = src
        .file_name()
        .and_then(|f| f.to_str())
        .ok_or_else(|| "Source path has no valid filename".to_string())?
        .to_string();

    let dir = PathBuf::from(&plugin_dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create plugin directory: {e}"))?;

    let dest = dir.join(&filename);
    if dest.exists() {
        let name = dest.file_stem().and_then(|s| s.to_str()).unwrap_or(&filename);
        return Err(format!("Plugin '{name}' already installed. Remove it first."));
    }

    std::fs::copy(src, &dest).map_err(|e| format!("Failed to copy plugin: {e}"))?;
    Ok(filename)
}

/// Deletes `filename` from `plugin_dir`. `filename` must be a bare name with
/// no path separators, must resolve to a path within `plugin_dir`, and must
/// have a `.wasm` extension.
#[tauri::command]
pub async fn remove_plugin(filename: String, plugin_dir: String) -> Result<(), String> {
    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return Err("Invalid plugin filename".to_string());
    }
    if !filename.to_lowercase().ends_with(".wasm") {
        return Err("Invalid plugin filename".to_string());
    }

    let dir = PathBuf::from(&plugin_dir);
    let target = dir.join(&filename);

    if !target.is_file() {
        return Err(format!("Plugin '{filename}' not found"));
    }

    // Confirm the resolved path is actually inside plugin_dir (defense in
    // depth against the join above producing an unexpected path).
    let canonical_dir = std::fs::canonicalize(&dir).map_err(|e| format!("Invalid plugin directory: {e}"))?;
    let canonical_target = std::fs::canonicalize(&target).map_err(|e| format!("Invalid plugin path: {e}"))?;
    if !canonical_target.starts_with(&canonical_dir) {
        return Err("Invalid plugin filename".to_string());
    }

    std::fs::remove_file(&target).map_err(|e| format!("Failed to remove plugin: {e}"))
}
