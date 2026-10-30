use std::collections::HashMap;
use std::path::{Path, PathBuf};

use aerini_engine::plugin_loader::PluginLoader;
use aerini_engine::plugin_loader::signature::{check_signature, sig_sidecar_path, SigCheckResult, SignedFileEntry};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

use super::pack::{check_pack_signature, PACK_MANIFEST_ENTRY_NAME, pack_manifest_path, scan_installed_packs};

pub(super) fn trust_store_path(plugin_dir: &Path) -> PathBuf {
    plugin_dir.join(".aerini-plugin-trust.json")
}

/// Serializes trust-store read-modify-write across concurrent
/// `install_plugin_from_path`/`remove_plugin` calls in this process. Without
/// this, two simultaneous installs of different plugins could each read the
/// store before the other's write lands, and one's freshly-pinned key would
/// be silently dropped. A plain in-process static, not a `reload_lock`-style
/// Tauri-managed mutex — this module has no state-registration setup of its
/// own to extend that pattern into.
pub(super) static TRUST_STORE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Pinned publisher keys: `type_id -> base64 Ed25519 public key`, the first
/// key ever seen (and accepted) for that `type_id`. Missing or
/// unreadable/corrupt is treated as "no pins known yet" rather than a hard
/// error — this file lives in the same user-writable `plugin_dir` as the
/// plugins it protects, so it carries no stronger guarantee than they do.
/// It defends against a compromised *download* (a malicious update
/// delivered through a publisher's real distribution channel, or a MITM'd
/// transfer), not a local attacker who already has write access to
/// `plugin_dir` — that attacker could bypass this file, or the plugins
/// themselves, by other means regardless.
pub(super) fn read_trust_store(plugin_dir: &Path) -> Result<HashMap<String, String>, String> {
    let path = trust_store_path(plugin_dir);
    match std::fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw).map_err(|e| {
            format!("Plugin trust store {} is unreadable ({e}). Repair or delete it to continue.", path.display())
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(e) => Err(format!("Failed to read plugin trust store: {e}")),
    }
}

pub(super) fn load_trust_store(plugin_dir: &Path) -> HashMap<String, String> {
    read_trust_store(plugin_dir).unwrap_or_else(|e| {
        tracing::warn!("{e}");
        HashMap::new()
    })
}

pub(super) fn save_trust_store(plugin_dir: &Path, store: &HashMap<String, String>) -> Result<(), String> {
    let raw = serde_json::to_string_pretty(store).map_err(|e| format!("Failed to serialize plugin trust store: {e}"))?;
    let path = trust_store_path(plugin_dir);
    let staged = path.with_extension("json.tmp");
    std::fs::write(&staged, raw).map_err(|e| format!("Failed to write plugin trust store: {e}"))?;
    std::fs::rename(&staged, &path).map_err(|e| {
        let _ = std::fs::remove_file(&staged);
        format!("Failed to write plugin trust store: {e}")
    })
}

/// Human-readable status for `list_installed_plugins`'s `signature_status`
/// field. Read-only — reflects `trust` as given, never pins or mutates it
/// (only `install_plugin_from_path` does that; a listing command staying
/// side-effect-free is deliberate).
pub(super) fn signature_status_message(wasm_path: &Path, wasm_bytes: &[u8], trust: &HashMap<String, String>, type_id: &str) -> String {
    match check_signature(wasm_path, wasm_bytes) {
        SigCheckResult::Unsigned => "Unsigned — no publisher signature".to_string(),
        SigCheckResult::Unrecognized => "Signature present but in an unrecognized format".to_string(),
        SigCheckResult::Malformed(_) => "Signature file present but malformed".to_string(),
        SigCheckResult::IntegrityMismatch => "File does not match its signed checksum — may be corrupted".to_string(),
        SigCheckResult::SignatureInvalid => "Signature present but does not verify".to_string(),
        SigCheckResult::Valid(key) => {
            let key_b64 = BASE64.encode(key.as_bytes());
            match trust.get(type_id) {
                Some(pinned) if *pinned == key_b64 => "Verified — trusted publisher".to_string(),
                Some(_) => "Verified signature, but from a different key than the one currently trusted for this plugin".to_string(),
                None => "Verified signature (not yet trusted — reinstall via Plugins tab to establish trust)".to_string(),
            }
        }
    }
}

/// Best-effort, log-only signature audit of every `.wasm`/pack in `dir`,
/// run immediately before a reload. Never blocks anything itself — for
/// standalone `.wasm` files, the actual blocking now happens one step
/// later, in `load_plugins` → `load_plugin`
/// (`aerini_engine::plugin_loader`), which rejects a tampered/invalid/
/// malformed file outright before compiling it, and whose caller
/// (`load_plugins_from_dir`) already logs that rejection by path. This
/// function's own warning for the same file, when it fires, duplicates
/// that rejection but adds the plugin's declared `type_id` (via
/// `describe_plugin`, which — unlike `load_plugin` — doesn't check the
/// signature, so it still succeeds against a tampered file) for easier
/// identification in logs. Two things here have no equivalent in the
/// shared load path, so this function remains their sole enforcement
/// point: publisher-key trust pinning (a signature that verifies, but
/// under a different key than previously trusted for that `type_id`/pack),
/// and pack-level signatures (`check_pack_signature`, below) — a pack's
/// aggregate `.sig` covers a manifest plus member set as one unit, which
/// `load_plugin`'s per-file check has no concept of. Plain "unsigned" is
/// expected and common (every pre-signing plugin) and isn't logged here to
/// avoid noise; see `list_installed_plugins`'s `signature_status` for the
/// full per-plugin picture in the UI.
pub(super) fn warn_on_untrusted_plugins(dir: &Path) {
    let Ok(loader) = PluginLoader::shared() else { return };
    let trust = load_trust_store(dir);
    let Ok(entries) = std::fs::read_dir(dir) else { return };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("wasm")) != Some(true) {
            continue;
        }
        let Ok(desc) = loader.describe_plugin(&path) else { continue };
        let Ok(bytes) = std::fs::read(&path) else { continue };

        match check_signature(&path, &bytes) {
            SigCheckResult::IntegrityMismatch => tracing::warn!(
                "plugin '{}' ({}): file does not match its signed checksum — may be corrupted or tampered",
                desc.type_id, path.display()
            ),
            SigCheckResult::SignatureInvalid | SigCheckResult::Malformed(_) => tracing::warn!(
                "plugin '{}' ({}): signature sidecar present but invalid",
                desc.type_id, path.display()
            ),
            SigCheckResult::Valid(key) => {
                let key_b64 = BASE64.encode(key.as_bytes());
                if trust.get(&desc.type_id).is_some_and(|pinned| *pinned != key_b64) {
                    tracing::warn!(
                        "plugin '{}' ({}): signed by a different key than the one previously trusted for this plugin",
                        desc.type_id, path.display()
                    );
                }
            }
            SigCheckResult::Unsigned | SigCheckResult::Unrecognized => {}
        }
    }

    // Same audit, extended to installed packs: each pack's manifest + member
    // set is re-hashed from what's actually on disk right now and checked
    // against its `.aerini-pack.json.sig`, if any. A pack with no sidecar is
    // exactly as expected/unlogged as an unsigned standalone `.wasm` above.
    for (pack_id, manifest) in scan_installed_packs(dir) {
        let manifest_path = pack_manifest_path(dir, &pack_id);
        let Ok(manifest_bytes) = std::fs::read(&manifest_path) else { continue };
        let mut actual = vec![SignedFileEntry {
            name: PACK_MANIFEST_ENTRY_NAME.to_string(),
            blake3: blake3::hash(&manifest_bytes).to_hex().to_string(),
        }];
        let mut all_members_readable = true;
        for member in &manifest.files {
            match std::fs::read(dir.join(member)) {
                Ok(bytes) => actual.push(SignedFileEntry { name: member.clone(), blake3: blake3::hash(&bytes).to_hex().to_string() }),
                Err(_) => {
                    all_members_readable = false;
                    break;
                }
            }
        }
        if !all_members_readable {
            // A listed member is missing/unreadable right now — nothing
            // reliable to check a signature against; skip rather than
            // report a false integrity mismatch caused by our own read
            // failure, not the package's content.
            continue;
        }

        let sig_raw = std::fs::read(sig_sidecar_path(&manifest_path)).ok();
        match check_pack_signature(sig_raw.as_deref(), &actual) {
            SigCheckResult::IntegrityMismatch => tracing::warn!(
                "plugin pack '{pack_id}' ({}): contents do not match its signed checksums — may be corrupted or tampered",
                manifest_path.display()
            ),
            SigCheckResult::SignatureInvalid | SigCheckResult::Malformed(_) => tracing::warn!(
                "plugin pack '{pack_id}' ({}): signature sidecar present but invalid",
                manifest_path.display()
            ),
            SigCheckResult::Valid(key) => {
                let key_b64 = BASE64.encode(key.as_bytes());
                if trust.get(&format!("pack:{pack_id}")).is_some_and(|pinned| *pinned != key_b64) {
                    tracing::warn!(
                        "plugin pack '{pack_id}' ({}): signed by a different key than the one previously trusted for this pack",
                        manifest_path.display()
                    );
                }
            }
            SigCheckResult::Unsigned | SigCheckResult::Unrecognized => {}
        }
    }
}
