use std::path::{Path, PathBuf};
use std::sync::Arc;

use aerini_engine::db::WorkflowDb;
use aerini_engine::node::{NodeRegistry, Reloadable};
use aerini_engine::nodes::register_builtins;
use aerini_engine::plugin_loader::{load_plugins, PluginLoader, PluginLoadReport};
use aerini_engine::plugin_loader::signature::sig_sidecar_path;
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;

use install::{install_pack_blocking, install_plugin_blocking};
use list::list_installed_plugins_blocking;
use remove::remove_pack_files;
use trust::{load_trust_store, save_trust_store, TRUST_STORE_LOCK, warn_on_untrusted_plugins};

mod install;
mod list;
mod pack;
mod remove;
mod staging;
#[cfg(test)]
mod tests;
mod trust;

pub use list::PluginInfo;

// --- Plugin signature / integrity verification ---
//
// A publisher may ship an optional sidecar file, `<name>.wasm.sig`, next to
// `<name>.wasm`. It is never itself executed or introspected via the WIT
// interface — it's read as plain bytes from disk before the `.wasm` is
// touched, so a signature can't attest to its own validity from inside the
// code it's supposed to be vouching for. Shape:
//
// {
//   "schema_version": 1,
//   "algorithm": "ed25519",
//   "public_key": "<base64, 32 raw bytes>",
//   "files": [ { "name": "plugin.wasm", "blake3": "<hex>" } ],
//   "signature": "<base64, 64 raw bytes>"
// }
//
// `files` is a list (not a single hash) so a multi-file package manifest can
// reuse this exact format with more entries under one signature, without a
// schema change (`check_pack_signature` below does exactly that). `name`
// inside each entry is informational only — matching is by content hash,
// consistent with the type_id-keyed identity used elsewhere in this module,
// not by filename.
//
// `SigCheckResult`/`PluginSignatureFile`/`SignedFileEntry`/`signed_message`/
// `sig_sidecar_path`/`check_signature` live in
// `aerini_engine::plugin_loader::signature` (imported above) — the single
// verification path `load_plugins` itself now enforces on every load,
// shared by `aerini-server` and desktop alike. This file reuses those same
// types for two things `aerini-engine` has no concept of: the desktop-only
// trust store below, and multi-file `.aerinipkg` package signing
// (`check_pack_signature`), which generalizes the same sidecar shape to N
// files under one signature.

// --- Multi-node plugin packages (`.aerinipkg`) ---
//
// A pack bundles N already-buildable single-node `.wasm` plugin outputs
// under one install/remove/update unit. The artifact is a flat zip
// (`.aerinipkg`): a `pack.json` manifest, the member `.wasm` files it lists,
// and an optional `pack.json.sig` covering the manifest and every member
// under one signature — the same `PluginSignatureFile`/`SignedFileEntry`
// shape as the single-file `.wasm.sig` above, just with more than one entry
// (that shape was already a list for exactly this reason).
//
// On disk in `plugin_dir` (flat, same directory as single-file installs):
// `<pack_id>.aerini-pack.json`, member `.wasm` files under their own names,
// and `<pack_id>.aerini-pack.json.sig` when signed. `aerini-engine` never
// sees a "pack" concept at all — `load_plugins_from_dir` already loads
// every `.wasm` in the directory independently, so a pack's members are
// ordinary single-node components that happen to be installed, removed,
// and updated as a group by the commands below.

pub(crate) const PLUGIN_DIR_SETTING: &str = "plugin_dir";

/// Runs blocking filesystem and plugin work off the async executor.
async fn run_blocking<T, F>(work: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tokio::task::spawn_blocking(work).await.map_err(|e| format!("Plugin task failed: {e}"))?
}

/// Rejects a `plugin_dir` sent by the webview that is not the folder saved in
/// Settings, so these commands cannot be pointed at another directory.
fn require_configured_dir(db: &WorkflowDb, requested: &str) -> Result<(), String> {
    let configured = super::scheduler::configured_plugin_dir(db)
        .ok_or_else(|| "No plugin folder is set in Settings".to_string())?;
    let same = match (std::fs::canonicalize(&configured), std::fs::canonicalize(requested.trim())) {
        (Ok(a), Ok(b)) => a == b,
        _ => configured == Path::new(requested.trim()),
    };
    if same {
        Ok(())
    } else {
        Err("plugin_dir does not match the plugin folder set in Settings".to_string())
    }
}

/// Lists all `.wasm` files in `plugin_dir`. Files that fail to load are still
/// returned with `load_error` set so the UI can surface broken plugins.
/// A non-existent `plugin_dir` returns an empty list, not an error.
#[tauri::command]
pub async fn list_installed_plugins(
    plugin_dir: String,
    load_report: tauri::State<'_, Arc<Reloadable<PluginLoadReport>>>,
) -> Result<Vec<PluginInfo>, String> {
    let load_report = load_report.current();
    run_blocking(move || list_installed_plugins_blocking(plugin_dir, &load_report)).await
}

/// Opens a native folder picker and saves the chosen folder as the plugin
/// folder. `set_setting` refuses `plugin_dir`, so only a folder picked in this
/// dialog can become the folder the install commands accept. Returns `None`
/// when the user cancels.
#[tauri::command]
pub async fn choose_plugin_dir(
    app: tauri::AppHandle,
    db: tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<Option<String>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<tauri_plugin_dialog::FilePath>>();
    app.dialog().file().pick_folder(move |path| {
        let _ = tx.send(path);
    });
    let Ok(Some(picked)) = rx.await else { return Ok(None) };
    let dir = picked
        .as_path()
        .ok_or_else(|| "Invalid folder".to_string())?
        .display()
        .to_string();
    let db = Arc::clone(&db);
    let saved = dir.clone();
    run_blocking(move || db.set_setting(PLUGIN_DIR_SETTING, &saved)).await?;
    Ok(Some(dir))
}

/// Copies the `.wasm` file at `src_path` into `plugin_dir`, creating the
/// directory if needed. Returns the destination filename.
///
/// Identity for update detection is the incoming file's `type_id` when it
/// loads successfully — this matches any existing installed file claiming
/// that `type_id`, regardless of filename. If the incoming file fails to
/// load, identity falls back to filename, preserving the existing behavior
/// of installing a broken plugin so the UI can surface why (see
/// `list_installed_plugins`'s `load_error`).
///
/// Without `overwrite`, a matching existing plugin returns an error prefixed
/// `plugin_already_installed:` — the frontend matches on this prefix to offer
/// an update confirmation rather than parsing the full message. With
/// `overwrite`, the existing file for that identity is replaced. A
/// destination filename already occupied by an unrelated plugin (different
/// identity) is always a hard error, never silently replaced.
#[tauri::command]
pub async fn install_plugin_from_path(
    src_path: String,
    plugin_dir: String,
    overwrite: bool,
    db: tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<String, String> {
    let db = Arc::clone(&db);
    run_blocking(move || {
        require_configured_dir(&db, &plugin_dir)?;
        install_plugin_blocking(src_path, plugin_dir, overwrite)
    })
    .await
}

/// Installs a multi-node `.aerinipkg` package: parses and validates its
/// `pack.json` manifest, extends signature verification to the manifest +
/// every member `.wasm` under one signature (see `check_pack_signature`),
/// and — only once every check below has passed — writes the manifest and
/// its members into `plugin_dir` as a group. Returns the installed
/// `pack_id`.
///
/// Mirrors `install_plugin_from_path`'s ordering exactly: signature/trust
/// checks happen first, entirely before any filesystem mutation; identity
/// for update detection is `pack_id`, matched against an existing
/// `<pack_id>.aerini-pack.json` in `plugin_dir`. Without `overwrite`, a
/// matching existing pack returns `pack_already_installed:`. With it, the
/// pack's entire member set is replaced — members no longer listed are
/// removed, new ones added, applying the same update semantics as a
/// single-file install at the pack level.
///
/// A member `type_id` — or member filename — already owned by a *different*
/// pack or a standalone plugin is always a hard `pack_member_conflict:`,
/// never silently adopted, regardless of `overwrite` (that flag only ever
/// authorizes replacing this same pack's own previous install).
#[tauri::command]
pub async fn install_plugin_pack_from_path(
    src_path: String,
    plugin_dir: String,
    overwrite: bool,
    db: tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<String, String> {
    let db = Arc::clone(&db);
    run_blocking(move || {
        require_configured_dir(&db, &plugin_dir)?;
        install_pack_blocking(src_path, plugin_dir, overwrite)
    })
    .await
}

/// Deletes `filename` from `plugin_dir`. `filename` must be a bare name with
/// no path separators, must resolve to a path within `plugin_dir`, and must
/// have a `.wasm` extension.
#[tauri::command]
pub async fn remove_plugin(
    filename: String,
    plugin_dir: String,
    db: tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<(), String> {
    let (db, dir_arg) = (Arc::clone(&db), plugin_dir.clone());
    run_blocking(move || require_configured_dir(&db, &dir_arg)).await?;
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

    // Best-effort: clear this plugin's pinned publisher key, if any, so a
    // later reinstall under the same type_id starts trust fresh (this is
    // the recovery path for a legitimate publisher key rotation — see
    // `install_plugin_from_path`'s key-mismatch/downgrade checks). Getting
    // the type_id means calling the plugin's own describe() — time-boxed
    // and run off the async task via spawn_blocking, so a slow or
    // deliberately hostile plugin can't delay the actual removal below,
    // which is what actually matters when someone is trying to get rid of
    // a broken or malicious plugin. Neither this nor the sidecar cleanup
    // that follows blocks removal on failure.
    let target_for_describe = target.clone();
    let type_id_for_cleanup = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        tokio::task::spawn_blocking(move || {
            PluginLoader::shared()
                .ok()
                .and_then(|loader| loader.describe_plugin(&target_for_describe).ok())
                .map(|d| d.type_id)
        }),
    )
    .await
    .ok()
    .and_then(|join_result| join_result.ok())
    .flatten();

    if let Some(type_id) = type_id_for_cleanup {
        if let Ok(_guard) = TRUST_STORE_LOCK.lock() {
            let mut store = load_trust_store(&dir);
            if store.remove(&type_id).is_some() {
                let _ = save_trust_store(&dir, &store);
            }
        }
    }
    let _ = std::fs::remove_file(sig_sidecar_path(&target));

    std::fs::remove_file(&target).map_err(|e| format!("Failed to remove plugin: {e}"))
}

/// Deletes a multi-node package: its manifest, every member `.wasm` it
/// lists, and any signature sidecars. `pack_id` must pass the same
/// `is_valid_pack_id` charset check enforced at install time — it is used
/// to build a filesystem path, so this is not optional defense-in-depth,
/// it is the only thing standing between an arbitrary caller-supplied
/// string and `pack_manifest_path`.
///
/// Unlike `remove_plugin`, no time-boxed `describe()` call is needed to
/// discover an identity to clear from the trust store — a pack's identity
/// (`pack_id`) is already the input, not something that has to be read back
/// out of the plugin itself. Member removal and sidecar cleanup are
/// best-effort; only the manifest file's own removal is load-bearing for
/// this command's `Result`, matching `remove_plugin`'s "removal must not
/// stall on anything but the file itself" shape.
#[tauri::command]
pub async fn remove_plugin_pack(
    pack_id: String,
    plugin_dir: String,
    db: tauri::State<'_, Arc<WorkflowDb>>,
) -> Result<(), String> {
    let (db, dir_arg) = (Arc::clone(&db), plugin_dir.clone());
    run_blocking(move || require_configured_dir(&db, &dir_arg)).await?;
    remove_pack_files(&pack_id, &plugin_dir)
}

/// Rebuilds the node registry from scratch (built-ins + every `.wasm` in
/// `plugin_dir`) and swaps it in atomically — no restart required.
///
/// A workflow run already in progress keeps executing against the registry
/// snapshot it started with; only runs (and scheduled/webhook fires)
/// starting after this returns see the reloaded set. See
/// [`Reloadable`]'s doc comment for the underlying guarantee.
///
/// `reload_lock` serializes overlapping reload calls so a slower-but-earlier
/// one can't finish after, and clobber, a faster-but-later one with stale
/// data — the registry swap itself is atomic, but that alone doesn't order
/// two independent scan+compile+swap sequences against each other.
#[tauri::command]
pub async fn reload_plugins(
    plugin_dir:  String,
    app:         tauri::AppHandle,
    db:          tauri::State<'_, Arc<WorkflowDb>>,
    registry:    tauri::State<'_, Arc<Reloadable<NodeRegistry>>>,
    load_report: tauri::State<'_, Arc<Reloadable<PluginLoadReport>>>,
    reload_lock: tauri::State<'_, Arc<tokio::sync::Mutex<()>>>,
) -> Result<PluginLoadReport, String> {
    let _guard = reload_lock.lock().await;
    {
        let (db, dir_arg) = (Arc::clone(&db), plugin_dir.clone());
        run_blocking(move || require_configured_dir(&db, &dir_arg)).await?;
    }

    let data_dir = app.path().app_data_dir()
        .map_err(|e| format!("Failed to resolve app data directory: {e}"))?;

    let mut new_registry = NodeRegistry::new();
    register_builtins(&mut new_registry, &data_dir, Some(Arc::clone(&db)));

    let dir = PathBuf::from(&plugin_dir);
    let (new_registry, report) = run_blocking(move || {
        let report = if !dir.exists() {
            PluginLoadReport::default()
        } else {
            warn_on_untrusted_plugins(&dir);
            load_plugins(&mut new_registry, &dir)
        };
        Ok((new_registry, report))
    })
    .await?;

    registry.reload(new_registry);
    load_report.reload(report.clone());
    Ok(report)
}
