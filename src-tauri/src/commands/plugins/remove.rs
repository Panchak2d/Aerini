use std::path::PathBuf;

use aerini_engine::plugin_loader::signature::sig_sidecar_path;

use super::pack::{is_valid_pack_id, pack_manifest_path, parse_pack_manifest};
use super::trust::{load_trust_store, save_trust_store, TRUST_STORE_LOCK};

pub(super) fn remove_pack_files(pack_id: &str, plugin_dir: &str) -> Result<(), String> {
    if !is_valid_pack_id(pack_id) {
        return Err("Invalid pack_id".to_string());
    }

    let dir = PathBuf::from(plugin_dir);
    let manifest_path = pack_manifest_path(&dir, pack_id);

    if !manifest_path.is_file() {
        return Err(format!("Pack '{pack_id}' not found"));
    }

    // Confirm the resolved path is actually inside plugin_dir (defense in
    // depth, matching `remove_plugin`'s identical check).
    let canonical_dir = std::fs::canonicalize(&dir).map_err(|e| format!("Invalid plugin directory: {e}"))?;
    let canonical_target = std::fs::canonicalize(&manifest_path).map_err(|e| format!("Invalid pack manifest path: {e}"))?;
    if !canonical_target.starts_with(&canonical_dir) {
        return Err("Invalid pack_id".to_string());
    }

    if let Ok(_guard) = TRUST_STORE_LOCK.lock() {
        let mut store = load_trust_store(&dir);
        if store.remove(&format!("pack:{pack_id}")).is_some() {
            let _ = save_trust_store(&dir, &store);
        }
    }

    // Best-effort: remove every member this pack's manifest lists, plus any
    // per-member `.wasm.sig` sidecar. A manifest that fails to parse (corrupt,
    // hand-edited) leaves its members orphaned rather than guessed at — this
    // command only ever acts on filenames the manifest itself names.
    if let Ok(raw) = std::fs::read(&manifest_path) {
        if let Ok(manifest) = parse_pack_manifest(&raw) {
            for member in &manifest.files {
                let member_path = dir.join(member);
                let _ = std::fs::remove_file(sig_sidecar_path(&member_path));
                let _ = std::fs::remove_file(&member_path);
            }
        }
    }
    let _ = std::fs::remove_file(sig_sidecar_path(&manifest_path));

    std::fs::remove_file(&manifest_path).map_err(|e| format!("Failed to remove pack manifest: {e}"))
}
