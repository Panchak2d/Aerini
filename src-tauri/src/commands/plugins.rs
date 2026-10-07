use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aerini_engine::db::WorkflowDb;
use aerini_engine::model::NodeType;
use aerini_engine::node::{NodeRegistry, Reloadable};
use aerini_engine::nodes::register_builtins;
use aerini_engine::plugin_loader::signature::{
    check_signature, sig_sidecar_path, signed_message, PluginSignatureFile, SigCheckResult, SignedFileEntry,
    SIG_ALGORITHM, SIG_SCHEMA_VERSION,
};
use aerini_engine::plugin_loader::{load_plugins, PluginLoadReport, PluginLoader};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use tauri::Manager;
use zip::ZipArchive;

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
// consistent with the type_id-keyed identity used elsewhere in this file,
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

const PACK_SCHEMA_VERSION: u32 = 1;
const PACK_MAX_ENTRIES: usize = 64;
const PACK_MAX_MEMBER_BYTES: u64 = 64 * 1024 * 1024;
const PACK_MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
const PACK_MANIFEST_MAX_BYTES: u64 = 64 * 1024;
const PACK_MANIFEST_ENTRY_NAME: &str = "pack.json";
const PACK_SIG_ENTRY_NAME: &str = "pack.json.sig";

#[derive(Deserialize, Clone, Debug)]
struct PackManifest {
    schema_version: u32,
    pack_id: String,
    display_name: String,
    files: Vec<String>,
}

/// `pack_id` must match `^[A-Za-z0-9][A-Za-z0-9._-]*$` — same discipline as
/// a plugin `type_id`, reverse-domain recommended. Hand-rolled rather than
/// pulling in a `regex` dependency for one fixed, simple charset check.
fn is_valid_pack_id(id: &str) -> bool {
    let mut chars = id.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Parses and validates `raw` as a `pack.json` manifest. Fails closed on
/// anything structurally wrong — unsupported schema version, an invalid
/// `pack_id`, an empty/oversized/duplicate member list, or a member
/// filename that isn't a bare `.wasm` name — rather than sanitizing or
/// mangling any of it.
fn parse_pack_manifest(raw: &[u8]) -> Result<PackManifest, String> {
    if raw.len() as u64 > PACK_MANIFEST_MAX_BYTES {
        return Err(format!("pack_invalid: {PACK_MANIFEST_ENTRY_NAME} exceeds the maximum manifest size"));
    }
    let manifest: PackManifest = serde_json::from_slice(raw)
        .map_err(|e| format!("pack_invalid: {PACK_MANIFEST_ENTRY_NAME} is not valid JSON ({e})"))?;

    if manifest.schema_version != PACK_SCHEMA_VERSION {
        return Err(format!(
            "pack_invalid: unsupported {PACK_MANIFEST_ENTRY_NAME} schema_version {}",
            manifest.schema_version
        ));
    }
    if !is_valid_pack_id(&manifest.pack_id) {
        return Err(format!(
            "pack_invalid: '{}' is not a valid pack_id (must match ^[A-Za-z0-9][A-Za-z0-9._-]*$)",
            manifest.pack_id
        ));
    }
    if manifest.display_name.trim().is_empty() {
        return Err("pack_invalid: display_name must not be empty".to_string());
    }
    if manifest.files.is_empty() {
        return Err(format!("pack_invalid: {PACK_MANIFEST_ENTRY_NAME} lists no member files"));
    }
    if manifest.files.len() > PACK_MAX_ENTRIES {
        return Err(format!("pack_invalid: {PACK_MANIFEST_ENTRY_NAME} lists more than {PACK_MAX_ENTRIES} member files"));
    }
    let mut seen = HashSet::new();
    for name in &manifest.files {
        if name.contains('/') || name.contains('\\') || name.contains("..") {
            return Err(format!("pack_invalid: member filename '{name}' must be a bare name with no path separators"));
        }
        if !name.to_lowercase().ends_with(".wasm") {
            return Err(format!("pack_invalid: member filename '{name}' must end in .wasm"));
        }
        if !seen.insert(name.clone()) {
            return Err(format!("pack_invalid: member filename '{name}' is listed more than once"));
        }
    }
    Ok(manifest)
}

/// Path of `<pack_id>.aerini-pack.json` in `plugin_dir`. The optional
/// signature sidecar is `sig_sidecar_path` applied to this path, exactly
/// like the single-file `.wasm` -> `.wasm.sig` convention — no new sidecar
/// helper needed.
fn pack_manifest_path(plugin_dir: &Path, pack_id: &str) -> PathBuf {
    plugin_dir.join(format!("{pack_id}.aerini-pack.json"))
}

/// Scans `plugin_dir` for `*.aerini-pack.json` manifests and returns every
/// one that parses successfully, keyed by `pack_id`. A manifest that fails
/// to parse (corrupt, hand-edited, from a newer app version) is silently
/// skipped rather than surfaced as an error here — mirrors `scan_type_ids`'s
/// treatment of a `.wasm` file that fails to describe.
fn scan_installed_packs(plugin_dir: &Path) -> HashMap<String, PackManifest> {
    let mut out = HashMap::new();
    let Ok(entries) = std::fs::read_dir(plugin_dir) else { return out };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(filename) = path.file_name().and_then(|f| f.to_str()) else { continue };
        if !filename.ends_with(".aerini-pack.json") {
            continue;
        }
        let Ok(raw) = std::fs::read(&path) else { continue };
        if let Ok(manifest) = parse_pack_manifest(&raw) {
            out.insert(manifest.pack_id.clone(), manifest);
        }
    }
    out
}

/// Reads all of `r` into memory, erroring the moment more than `cap` bytes
/// have been seen. Never trusts a zip entry's declared (attacker-controlled)
/// size — this is what actually bounds a zip-bomb member, not the size
/// field in the archive's central directory.
fn read_bounded(mut r: impl std::io::Read, cap: u64) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    let mut total: u64 = 0;
    loop {
        let n = r.read(&mut chunk).map_err(|e| format!("pack_invalid: failed to read archive entry: {e}"))?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > cap {
            return Err("pack_invalid: archive entry exceeds its maximum allowed size".to_string());
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    Ok(buf)
}

/// Extracts and structurally validates a `.aerinipkg` zip: exactly one
/// `pack.json`, at most one `pack.json.sig`, and every other entry a bare
/// `.wasm` file — nothing else, fail closed. Returns the raw manifest
/// bytes (not re-serialized — the signature, if any, was computed over
/// these exact bytes), the raw sig bytes if present, and every member's
/// raw bytes keyed by filename. Caller still owns cross-checking the
/// member set against `pack.json`'s own `files` list (this function has no
/// opinion on manifest content, only on the archive's shape).
///
/// Zip-slip is closed by construction, not sanitization alone: every entry
/// is rejected unless it is a non-directory, single-path-component name —
/// `enclosed_name()` handles `..`/absolute paths, and the explicit
/// component-count and raw-separator checks catch any nested path this
/// flat-by-definition format should never contain in the first place.
type PackZipContents = (Vec<u8>, Option<Vec<u8>>, HashMap<String, Vec<u8>>);

fn extract_pack_zip(src: &Path) -> Result<PackZipContents, String> {
    let file = std::fs::File::open(src).map_err(|e| format!("Failed to open package: {e}"))?;
    let reader = std::io::BufReader::new(file);
    let mut archive = ZipArchive::new(reader).map_err(|e| format!("pack_invalid: not a valid zip archive ({e})"))?;

    if archive.len() > PACK_MAX_ENTRIES {
        return Err(format!("pack_invalid: archive contains more than {PACK_MAX_ENTRIES} entries"));
    }

    let mut manifest_bytes: Option<Vec<u8>> = None;
    let mut sig_bytes: Option<Vec<u8>> = None;
    let mut members: HashMap<String, Vec<u8>> = HashMap::new();
    let mut total_bytes: u64 = 0;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| format!("pack_invalid: corrupt archive entry: {e}"))?;

        if entry.is_dir() {
            return Err("pack_invalid: archive contains a directory entry — packages must be flat".to_string());
        }
        let raw_name = entry.name().to_string();
        if raw_name.contains('/') || raw_name.contains('\\') {
            return Err("pack_invalid: archive entries must be flat (no directories)".to_string());
        }
        let Some(enclosed) = entry.enclosed_name() else {
            return Err("pack_invalid: archive entry has an unsafe path".to_string());
        };
        if enclosed.components().count() != 1 {
            return Err("pack_invalid: archive entries must be flat (no directories)".to_string());
        }
        let Some(name) = enclosed.file_name().and_then(|f| f.to_str()) else {
            return Err("pack_invalid: archive entry has an unreadable filename".to_string());
        };
        let name = name.to_string();

        let cap = if name == PACK_MANIFEST_ENTRY_NAME || name == PACK_SIG_ENTRY_NAME {
            PACK_MANIFEST_MAX_BYTES
        } else if name.to_lowercase().ends_with(".wasm") {
            PACK_MAX_MEMBER_BYTES
        } else {
            return Err(format!("pack_invalid: unexpected entry '{name}' in package"));
        };

        let bytes = read_bounded(&mut entry, cap)?;
        total_bytes += bytes.len() as u64;
        if total_bytes > PACK_MAX_TOTAL_BYTES {
            return Err("pack_invalid: package exceeds its maximum total size".to_string());
        }

        if name == PACK_MANIFEST_ENTRY_NAME {
            if manifest_bytes.is_some() {
                return Err(format!("pack_invalid: package contains more than one {PACK_MANIFEST_ENTRY_NAME}"));
            }
            manifest_bytes = Some(bytes);
        } else if name == PACK_SIG_ENTRY_NAME {
            if sig_bytes.is_some() {
                return Err(format!("pack_invalid: package contains more than one {PACK_SIG_ENTRY_NAME}"));
            }
            sig_bytes = Some(bytes);
        } else if members.insert(name.clone(), bytes).is_some() {
            return Err(format!("pack_invalid: duplicate entry '{name}' in package"));
        }
    }

    let manifest_bytes = manifest_bytes.ok_or_else(|| format!("pack_invalid: package is missing {PACK_MANIFEST_ENTRY_NAME}"))?;
    Ok((manifest_bytes, sig_bytes, members))
}

/// Checks a pack signature payload — already-read `pack.json.sig` bytes, or
/// `None` when no sidecar was present — against `actual`: the manifest's
/// own content hash paired with every member `.wasm`'s content hash, as
/// computed by the caller from what's about to be installed (or, from
/// `warn_on_untrusted_plugins`, what's already on disk). Pure and
/// read-only, exactly like `check_signature` above, which this generalizes
/// to N files under one signature — `PluginSignatureFile.files` was already
/// a list for exactly this reason (see that struct's own doc comment), so
/// no sidecar shape changed here, only that N can now be greater than one.
/// `check_signature` itself is untouched; this is a separate function so
/// its single-file behavior can't drift.
fn check_pack_signature(sig_raw: Option<&[u8]>, actual: &[SignedFileEntry]) -> SigCheckResult {
    let Some(sig_raw) = sig_raw else { return SigCheckResult::Unsigned };
    let sig: PluginSignatureFile = match serde_json::from_slice(sig_raw) {
        Ok(s) => s,
        Err(e) => return SigCheckResult::Malformed(e.to_string()),
    };
    if sig.schema_version != SIG_SCHEMA_VERSION || sig.algorithm != SIG_ALGORITHM {
        return SigCheckResult::Unrecognized;
    }

    // The signed set must match the actual set exactly, by name and hash —
    // not just by count. A name the signature doesn't cover, or a hash that
    // doesn't match what's actually about to be installed, means this
    // signature wasn't produced for this exact bundle.
    if sig.files.len() != actual.len() {
        return SigCheckResult::IntegrityMismatch;
    }
    let expected: HashMap<&str, &str> = sig.files.iter().map(|e| (e.name.as_str(), e.blake3.as_str())).collect();
    for a in actual {
        match expected.get(a.name.as_str()) {
            Some(hash) if **hash == a.blake3 => {}
            _ => return SigCheckResult::IntegrityMismatch,
        }
    }

    let Some(public_key) = BASE64.decode(&sig.public_key).ok().and_then(|b| <[u8; 32]>::try_from(b).ok()) else {
        return SigCheckResult::Malformed("public_key is not valid base64 for a 32-byte Ed25519 key".to_string());
    };
    let Some(signature_bytes) = BASE64.decode(&sig.signature).ok().and_then(|b| <[u8; 64]>::try_from(b).ok()) else {
        return SigCheckResult::Malformed("signature is not valid base64 for a 64-byte Ed25519 signature".to_string());
    };
    let Ok(verifying_key) = VerifyingKey::from_bytes(&public_key) else {
        return SigCheckResult::Malformed("public_key is not a valid Ed25519 point".to_string());
    };
    let signature = Signature::from_bytes(&signature_bytes);

    match verifying_key.verify_strict(&signed_message(&sig.files), &signature) {
        Ok(()) => SigCheckResult::Valid(verifying_key),
        Err(_) => SigCheckResult::SignatureInvalid,
    }
}

fn trust_store_path(plugin_dir: &Path) -> PathBuf {
    plugin_dir.join(".aerini-plugin-trust.json")
}

/// Serializes trust-store read-modify-write across concurrent
/// `install_plugin_from_path`/`remove_plugin` calls in this process. Without
/// this, two simultaneous installs of different plugins could each read the
/// store before the other's write lands, and one's freshly-pinned key would
/// be silently dropped. A plain in-process static, not a `reload_lock`-style
/// Tauri-managed mutex — this file has no state-registration setup of its
/// own to extend that pattern into.
static TRUST_STORE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
fn load_trust_store(plugin_dir: &Path) -> HashMap<String, String> {
    std::fs::read_to_string(trust_store_path(plugin_dir))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save_trust_store(plugin_dir: &Path, store: &HashMap<String, String>) -> Result<(), String> {
    let raw = serde_json::to_string_pretty(store).map_err(|e| format!("Failed to serialize plugin trust store: {e}"))?;
    std::fs::write(trust_store_path(plugin_dir), raw).map_err(|e| format!("Failed to write plugin trust store: {e}"))
}

/// Human-readable status for `list_installed_plugins`'s `signature_status`
/// field. Read-only — reflects `trust` as given, never pins or mutates it
/// (only `install_plugin_from_path` does that; a listing command staying
/// side-effect-free is deliberate).
fn signature_status_message(wasm_path: &Path, wasm_bytes: &[u8], trust: &HashMap<String, String>, type_id: &str) -> String {
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
fn warn_on_untrusted_plugins(dir: &Path) {
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

/// Lists all `.wasm` files in `plugin_dir`. Files that fail to load are still
/// returned with `load_error` set so the UI can surface broken plugins.
/// A non-existent `plugin_dir` returns an empty list, not an error.
#[tauri::command]
pub async fn list_installed_plugins(
    plugin_dir: String,
    load_report: tauri::State<'_, Arc<Reloadable<PluginLoadReport>>>,
) -> Result<Vec<PluginInfo>, String> {
    let dir = PathBuf::from(&plugin_dir);
    let load_report = load_report.current();

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

/// Scans `plugin_dir` for `.wasm` files and returns `(filename, type_id)` for
/// every one that loads successfully. A file that fails to describe is
/// omitted — it has no `type_id` to match against.
fn scan_type_ids(dir: &Path, loader: &PluginLoader) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("wasm")) != Some(true) {
            continue;
        }
        let Some(filename) = path.file_name().and_then(|f| f.to_str()) else { continue };
        if let Ok(desc) = loader.describe_plugin(&path) {
            out.push((filename.to_string(), desc.type_id));
        }
    }
    out
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
) -> Result<String, String> {
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

    let loader = PluginLoader::shared()?;
    let incoming_type_id = loader.describe_plugin(src).ok().map(|d| d.type_id);

    // Signature/integrity verification, before any filesystem mutation below.
    // Already-published plugins with no signature at all still install
    // (Unsigned/Unrecognized fall through) — only a positively bad signal
    // (corrupted content, an invalid signature, or a publisher key that
    // doesn't match what was previously trusted for this type_id) blocks.
    let wasm_bytes = std::fs::read(src).map_err(|e| format!("Failed to read plugin file: {e}"))?;
    let sig_check = check_signature(src, &wasm_bytes);
    {
        // Holds TRUST_STORE_LOCK for this whole read-decide-write section —
        // see the lock's own doc comment. Everything inside is synchronous
        // std::fs/serde_json work, no `.await` point, so holding a
        // std::sync::Mutex guard across it is safe.
        let _trust_guard = TRUST_STORE_LOCK
            .lock()
            .map_err(|_| "Plugin trust store lock was poisoned by an earlier failure".to_string())?;
        match sig_check {
            SigCheckResult::IntegrityMismatch => {
                return Err(
                    "plugin_integrity_failed: the plugin file does not match its signed checksum (corrupted or tampered download)"
                        .to_string(),
                );
            }
            SigCheckResult::SignatureInvalid => {
                return Err("plugin_signature_invalid: the plugin's signature file is present but does not verify".to_string());
            }
            SigCheckResult::Malformed(reason) => {
                return Err(format!("plugin_signature_invalid: the plugin's signature file is malformed ({reason})"));
            }
            SigCheckResult::Valid(verifying_key) => {
                if let Some(type_id) = &incoming_type_id {
                    let mut store = load_trust_store(&dir);
                    let incoming_key_b64 = BASE64.encode(verifying_key.as_bytes());
                    match store.get(type_id) {
                        Some(pinned) if *pinned != incoming_key_b64 => {
                            return Err(format!(
                                "plugin_key_mismatch: '{type_id}' is signed by a different publisher key than the one \
                                 previously trusted for this plugin. Remove the existing install first if you intend to \
                                 trust the new key."
                            ));
                        }
                        Some(_) => {}
                        None => {
                            // First valid signature seen for this type_id — trust on first use.
                            store.insert(type_id.clone(), incoming_key_b64);
                            save_trust_store(&dir, &store)?;
                        }
                    }
                }
                // No type_id (plugin failed to describe): signature verified
                // but there's no identity to pin it to — proceed without
                // touching the trust store.
            }
            SigCheckResult::Unsigned | SigCheckResult::Unrecognized => {
                // Downgrade guard: a type_id previously trusted with a
                // verified signature can't silently update to an
                // unsigned/unrecognized file — that would let an attacker
                // who can't forge a new signature just ship a plain
                // unsigned "update" instead.
                if let Some(type_id) = &incoming_type_id {
                    if load_trust_store(&dir).contains_key(type_id) {
                        return Err(format!(
                            "plugin_signature_downgrade: '{type_id}' was previously installed with a verified publisher \
                             signature; this file has none. Remove the existing install first if this is intentional."
                        ));
                    }
                }
            }
        }
    }

    let existing_filename = match &incoming_type_id {
        Some(id) => scan_type_ids(&dir, loader).into_iter().find(|(_, tid)| tid == id).map(|(f, _)| f),
        None => dir.join(&filename).is_file().then(|| filename.clone()),
    };

    let dest = dir.join(&filename);
    let conflict_err = || Err(format!(
        "A different plugin file already exists at '{filename}'. Rename the source file and try again."
    ));

    match existing_filename {
        Some(existing_filename) if !overwrite => {
            let label = incoming_type_id.as_deref().unwrap_or(&filename);
            if label == existing_filename {
                return Err(format!("plugin_already_installed: '{existing_filename}' is already installed"));
            }
            return Err(format!(
                "plugin_already_installed: '{label}' is already installed as '{existing_filename}'"
            ));
        }
        Some(existing_filename) => {
            let existing_path = dir.join(&existing_filename);
            // dest may differ from existing_path (identity matched by type_id under a
            // different filename) — if a *third*, unrelated file already sits at dest,
            // this is a genuine name clash and must not be silently overwritten.
            if dest.exists() && dest != existing_path {
                return conflict_err();
            }
            if existing_path != dest {
                std::fs::remove_file(&existing_path)
                    .map_err(|e| format!("Failed to remove previous version '{existing_filename}': {e}"))?;
                let _ = std::fs::remove_file(sig_sidecar_path(&existing_path));
            }
        }
        None if dest.exists() => return conflict_err(),
        None => {}
    }

    // `src` and `dest` can resolve to the same file: native file dialogs
    // remember their last-browsed folder, so re-selecting a plugin that
    // already lives in `plugin_dir` points `src_path` straight at `dest`.
    // std::fs::copy(p, p) truncates the destination before reading from the
    // (identical) source and "succeeds" with 0 bytes copied — it does not
    // detect or reject a same-path copy. Content is already in place in
    // that case, so skip both copies entirely rather than self-destruct.
    let same_file = std::fs::canonicalize(src)
        .ok()
        .zip(std::fs::canonicalize(&dest).ok())
        .is_some_and(|(a, b)| a == b);

    if !same_file {
        std::fs::copy(src, &dest).map_err(|e| format!("Failed to copy plugin: {e}"))?;

        let sig_src = sig_sidecar_path(src);
        let sig_dest = sig_sidecar_path(&dest);
        if sig_src.is_file() {
            std::fs::copy(&sig_src, &sig_dest).map_err(|e| format!("Failed to copy plugin signature file: {e}"))?;
        } else if sig_dest.is_file() {
            // No sidecar for the incoming file, but a stale one from a previous
            // install sits at dest — drop it so it doesn't get misattributed to
            // this file (the checks above already refused this path if it would
            // have been a signed-to-unsigned downgrade for a trusted type_id).
            let _ = std::fs::remove_file(&sig_dest);
        }
    }

    Ok(filename)
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
) -> Result<String, String> {
    let src = Path::new(&src_path);

    if src.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("aerinipkg")) != Some(true) {
        return Err("Selected file is not a .aerinipkg package".to_string());
    }
    if !src.is_file() {
        return Err(format!("Source file not found: {src_path}"));
    }

    let (manifest_bytes, sig_bytes, members) = extract_pack_zip(src)?;
    let manifest = parse_pack_manifest(&manifest_bytes)?;
    let expected_files: HashSet<&str> = manifest.files.iter().map(String::as_str).collect();
    let actual_files: HashSet<&str> = members.keys().map(String::as_str).collect();
    if expected_files != actual_files {
        return Err(format!(
            "pack_invalid: package contents do not match its manifest for '{}'",
            manifest.pack_id
        ));
    }

    // Entries the signature (if any) must cover: the manifest itself, plus
    // every member — same `{name, blake3}` shape as the single-file path,
    // just N of them instead of one.
    let mut actual_entries = vec![SignedFileEntry {
        name: PACK_MANIFEST_ENTRY_NAME.to_string(),
        blake3: blake3::hash(&manifest_bytes).to_hex().to_string(),
    }];
    for (name, bytes) in &members {
        actual_entries.push(SignedFileEntry { name: name.clone(), blake3: blake3::hash(bytes).to_hex().to_string() });
    }
    let sig_check = check_pack_signature(sig_bytes.as_deref(), &actual_entries);

    let dir = PathBuf::from(&plugin_dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create plugin directory: {e}"))?;

    let trust_key = format!("pack:{}", manifest.pack_id);
    {
        // Same lock, same read-decide-write shape as `install_plugin_from_path`
        // above — see `TRUST_STORE_LOCK`'s doc comment. A `pack:` prefix on
        // the trust-store key keeps pack identity in a disjoint namespace
        // from plain `type_id` pins sharing the same flat store, since a
        // `pack_id` and a `type_id` both pass the same charset check and
        // could otherwise collide on the same string.
        let _trust_guard = TRUST_STORE_LOCK
            .lock()
            .map_err(|_| "Plugin trust store lock was poisoned by an earlier failure".to_string())?;
        match sig_check {
            SigCheckResult::IntegrityMismatch => {
                return Err(
                    "pack_integrity_failed: the package's contents do not match its signed checksums (corrupted or tampered download)"
                        .to_string(),
                );
            }
            SigCheckResult::SignatureInvalid => {
                return Err("pack_signature_invalid: the package's signature file is present but does not verify".to_string());
            }
            SigCheckResult::Malformed(reason) => {
                return Err(format!("pack_signature_invalid: the package's signature file is malformed ({reason})"));
            }
            SigCheckResult::Valid(verifying_key) => {
                let mut store = load_trust_store(&dir);
                let incoming_key_b64 = BASE64.encode(verifying_key.as_bytes());
                match store.get(&trust_key) {
                    Some(pinned) if *pinned != incoming_key_b64 => {
                        return Err(format!(
                            "pack_key_mismatch: '{}' is signed by a different publisher key than the one previously \
                             trusted for this pack. Remove the existing install first if you intend to trust the new key.",
                            manifest.pack_id
                        ));
                    }
                    Some(_) => {}
                    None => {
                        store.insert(trust_key.clone(), incoming_key_b64);
                        save_trust_store(&dir, &store)?;
                    }
                }
            }
            SigCheckResult::Unsigned | SigCheckResult::Unrecognized => {
                if load_trust_store(&dir).contains_key(&trust_key) {
                    return Err(format!(
                        "pack_signature_downgrade: '{}' was previously installed with a verified publisher signature; \
                         this package has none. Remove the existing install first if this is intentional.",
                        manifest.pack_id
                    ));
                }
            }
        }
    }

    let existing_packs = scan_installed_packs(&dir);
    let existing_manifest = existing_packs.get(&manifest.pack_id).cloned();
    if existing_manifest.is_some() && !overwrite {
        return Err(format!("pack_already_installed: '{}' is already installed", manifest.pack_id));
    }

    // Everything past this point needs to instantiate each member to read
    // its `type_id` for conflict checking, which requires a real path on
    // disk (`PluginLoader::describe_plugin` takes `&Path`, and
    // `aerini-engine`/`plugin_loader.rs` are out of this batch's scope to
    // extend with an in-memory-bytes entry point). Staged into a tempdir
    // rather than `plugin_dir` itself so a member that fails every check
    // below never touches the real install directory.
    let loader = PluginLoader::shared()?;
    let staging = tempfile::tempdir().map_err(|e| format!("Failed to create a staging directory: {e}"))?;
    let mut member_type_ids: HashMap<String, String> = HashMap::new();
    for (name, bytes) in &members {
        let staged_path = staging.path().join(name);
        std::fs::write(&staged_path, bytes).map_err(|e| format!("Failed to stage package member '{name}': {e}"))?;
        if let Ok(desc) = loader.describe_plugin(&staged_path) {
            member_type_ids.insert(name.clone(), desc.type_id);
        }
        // A member that fails to describe is still installed (matching
        // `install_plugin_from_path`'s "install a broken plugin so the UI
        // can surface why" behavior) — it just has no `type_id` to run a
        // conflict check against below.
    }

    // Internal duplicate: two members of *this same* incoming pack claiming
    // the same `type_id`. Not a soft, last-loaded-wins collision like a
    // plugin-directory-wide `type_id` clash — a self-consistent package
    // should never ship this, so it's rejected as malformed rather than
    // silently resolved by load order.
    let mut seen_type_ids: HashMap<&str, &str> = HashMap::new();
    for (name, type_id) in &member_type_ids {
        if let Some(other_name) = seen_type_ids.insert(type_id.as_str(), name.as_str()) {
            return Err(format!(
                "pack_invalid: members '{other_name}' and '{name}' both export type_id '{type_id}'"
            ));
        }
    }

    // filename -> owning pack_id, across every *other* installed pack (this
    // pack's own prior install, if any, is expected to already own its own
    // member filenames/type_ids — that's an update, not a conflict).
    let mut filename_owner: HashMap<&str, &str> = HashMap::new();
    for (pid, m) in &existing_packs {
        for f in &m.files {
            filename_owner.insert(f.as_str(), pid.as_str());
        }
    }
    let is_own_pack = |owner: Option<&&str>| owner == Some(&manifest.pack_id.as_str());

    let existing_type_ids = scan_type_ids(&dir, loader);
    let mut type_id_owner: HashMap<&str, &str> = HashMap::new();
    for (f, tid) in &existing_type_ids {
        type_id_owner.insert(tid.as_str(), f.as_str());
    }

    for (name, type_id) in &member_type_ids {
        if let Some(owner_filename) = type_id_owner.get(type_id.as_str()) {
            if !is_own_pack(filename_owner.get(owner_filename)) {
                return Err(format!(
                    "pack_member_conflict: '{type_id}' (member '{name}') is already provided by another installed plugin or pack"
                ));
            }
        }
    }
    for name in members.keys() {
        if dir.join(name).exists() && !is_own_pack(filename_owner.get(name.as_str())) {
            return Err(format!(
                "pack_member_conflict: filename '{name}' is already used by another installed plugin or pack"
            ));
        }
    }

    // All checks passed — mutate. If this is an update, drop members the
    // new manifest no longer lists first (best-effort; the manifest/member
    // writes below are what must succeed for the install to count as done).
    if let Some(old) = &existing_manifest {
        for old_file in &old.files {
            if !expected_files.contains(old_file.as_str()) {
                let _ = std::fs::remove_file(dir.join(old_file));
                let _ = std::fs::remove_file(sig_sidecar_path(&dir.join(old_file)));
            }
        }
    }
    for name in members.keys() {
        std::fs::copy(staging.path().join(name), dir.join(name))
            .map_err(|e| format!("Failed to install package member '{name}': {e}"))?;
    }
    let manifest_dest = pack_manifest_path(&dir, &manifest.pack_id);
    std::fs::write(&manifest_dest, &manifest_bytes).map_err(|e| format!("Failed to write pack manifest: {e}"))?;

    let sig_dest = sig_sidecar_path(&manifest_dest);
    match &sig_bytes {
        Some(bytes) => std::fs::write(&sig_dest, bytes).map_err(|e| format!("Failed to write pack signature file: {e}"))?,
        None if sig_dest.is_file() => {
            let _ = std::fs::remove_file(&sig_dest);
        }
        None => {}
    }

    Ok(manifest.pack_id)
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
pub async fn remove_plugin_pack(pack_id: String, plugin_dir: String) -> Result<(), String> {
    if !is_valid_pack_id(&pack_id) {
        return Err("Invalid pack_id".to_string());
    }

    let dir = PathBuf::from(&plugin_dir);
    let manifest_path = pack_manifest_path(&dir, &pack_id);

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

    let data_dir = app.path().app_data_dir()
        .map_err(|e| format!("Failed to resolve app data directory: {e}"))?;

    let mut new_registry = NodeRegistry::new();
    register_builtins(&mut new_registry, &data_dir, Some(Arc::clone(&db)));

    let dir = PathBuf::from(&plugin_dir);
    let report = if !dir.exists() {
        PluginLoadReport::default()
    } else {
        warn_on_untrusted_plugins(&dir);
        load_plugins(&mut new_registry, &dir)
    };

    registry.reload(new_registry);
    load_report.reload(report.clone());
    Ok(report)
}

#[cfg(test)]
mod tests {
    //! Coverage here targets the pure validation/extraction/signature logic
    //! that doesn't require an actual `aerini-node`-exporting component —
    //! same limitation `plugin_loader.rs`'s own test module documents (no
    //! Rust toolchain in this environment to build one). `install_plugin_pack_from_path`'s
    //! happy path — reaching `PluginLoader::describe_plugin` on a real
    //! staged member and its downstream member-conflict checks — is
    //! therefore `UNVERIFIED` by these tests; every early-exit failure path
    //! that returns before that point (extension/existence checks, manifest
    //! parsing, signature verification, the already-installed/downgrade
    //! gates) is covered directly.

    use super::*;
    use std::io::Write;

    fn write_test_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let buf = std::io::Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(buf);
        let options = zip::write::SimpleFileOptions::default();
        for (name, data) in entries {
            zip.start_file(*name, options).expect("start_file failed");
            zip.write_all(data).expect("write_all failed");
        }
        zip.finish().expect("zip finish failed").into_inner()
    }

    fn write_pack_file(entries: &[(&str, &[u8])]) -> tempfile::NamedTempFile {
        let zip_bytes = write_test_zip(entries);
        let mut tmp = tempfile::Builder::new()
            .suffix(".aerinipkg")
            .tempfile()
            .expect("tempfile create failed");
        tmp.write_all(&zip_bytes).expect("tempfile write failed");
        tmp.flush().expect("tempfile flush failed");
        tmp
    }

    fn sample_manifest_json(pack_id: &str, files: &[&str]) -> Vec<u8> {
        serde_json::json!({
            "schema_version": PACK_SCHEMA_VERSION,
            "pack_id": pack_id,
            "display_name": "Sample Pack",
            "files": files,
        })
        .to_string()
        .into_bytes()
    }

    fn signing_key_fixture() -> ed25519_dalek::SigningKey {
        // Fixed, deterministic bytes — this is a test fixture, never a real
        // publisher key, so reproducibility matters more than secrecy here.
        ed25519_dalek::SigningKey::from_bytes(&[7u8; 32])
    }

    fn sig_file_json(signing_key: &ed25519_dalek::SigningKey, entries: &[SignedFileEntry]) -> Vec<u8> {
        use ed25519_dalek::Signer;
        let signature = signing_key.sign(&signed_message(entries));
        let files: Vec<_> = entries.iter().map(|e| serde_json::json!({"name": e.name, "blake3": e.blake3})).collect();
        serde_json::json!({
            "schema_version": SIG_SCHEMA_VERSION,
            "algorithm": SIG_ALGORITHM,
            "public_key": BASE64.encode(signing_key.verifying_key().to_bytes()),
            "files": files,
            "signature": BASE64.encode(signature.to_bytes()),
        })
        .to_string()
        .into_bytes()
    }

    // --- is_valid_pack_id ---

    #[test]
    fn is_valid_pack_id_accepts_reverse_domain_style_ids() {
        for id in ["com.example.mypack", "a", "A1", "pack-name_v2.1"] {
            assert!(is_valid_pack_id(id), "expected '{id}' to be accepted as a valid pack_id");
        }
    }

    #[test]
    fn is_valid_pack_id_rejects_bad_starts_and_disallowed_chars() {
        for id in ["", ".starts-with-dot", "-starts-with-dash", "has space", "has/slash", "has\\backslash", "has:colon"] {
            assert!(!is_valid_pack_id(id), "expected '{id}' to be rejected as an invalid pack_id");
        }
    }

    // --- parse_pack_manifest ---

    #[test]
    fn parse_pack_manifest_accepts_well_formed_manifest() {
        let raw = sample_manifest_json("com.example.pack", &["a.wasm", "b.wasm"]);
        let m = parse_pack_manifest(&raw).expect("well-formed manifest should parse");
        assert_eq!(m.pack_id, "com.example.pack");
        assert_eq!(m.files, vec!["a.wasm".to_string(), "b.wasm".to_string()]);
    }

    #[test]
    fn parse_pack_manifest_rejects_unsupported_schema_version() {
        let raw = serde_json::json!({
            "schema_version": 2, "pack_id": "com.example.pack", "display_name": "P", "files": ["a.wasm"]
        }).to_string().into_bytes();
        let err = parse_pack_manifest(&raw).expect_err("schema_version 2 should be rejected");
        assert!(err.starts_with("pack_invalid:"), "expected pack_invalid prefix, got: {err}");
    }

    #[test]
    fn parse_pack_manifest_rejects_invalid_pack_id() {
        let raw = sample_manifest_json(".bad-id", &["a.wasm"]);
        let err = parse_pack_manifest(&raw).expect_err("invalid pack_id should be rejected");
        assert!(err.starts_with("pack_invalid:"));
    }

    #[test]
    fn parse_pack_manifest_rejects_empty_files_list() {
        let raw = sample_manifest_json("com.example.pack", &[]);
        let err = parse_pack_manifest(&raw).expect_err("empty files list should be rejected");
        assert!(err.starts_with("pack_invalid:"));
    }

    #[test]
    fn parse_pack_manifest_rejects_bad_member_filenames() {
        // Two distinct guard clauses in one table: a duplicate name, and a
        // name that isn't a bare filename.
        for files in [vec!["a.wasm", "a.wasm"], vec!["../evil.wasm"]] {
            let raw = sample_manifest_json("com.example.pack", &files);
            let err = parse_pack_manifest(&raw).expect_err(&format!("{files:?} should be rejected"));
            assert!(err.starts_with("pack_invalid:"));
        }
    }

    #[test]
    fn parse_pack_manifest_rejects_oversized_manifest() {
        let raw = vec![b' '; (PACK_MANIFEST_MAX_BYTES + 1) as usize];
        let err = parse_pack_manifest(&raw).expect_err("oversized manifest should be rejected");
        assert!(err.starts_with("pack_invalid:"));
    }

    // --- read_bounded ---

    #[test]
    fn read_bounded_reads_all_bytes_under_cap() {
        let data = b"hello world";
        let result = read_bounded(&data[..], 100).expect("should read fully under cap");
        assert_eq!(result, data);
    }

    #[test]
    fn read_bounded_errors_when_exceeding_cap() {
        let data = [0u8; 20];
        let err = read_bounded(&data[..], 10).expect_err("should reject data over cap");
        assert!(err.starts_with("pack_invalid:"));
    }

    // --- extract_pack_zip ---

    #[test]
    fn extract_pack_zip_reads_manifest_and_members() {
        let zip_bytes = write_test_zip(&[("pack.json", b"manifest-bytes"), ("a.wasm", b"wasm-a"), ("b.wasm", b"wasm-b")]);
        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile create failed");
        tmp.write_all(&zip_bytes).expect("tempfile write failed");
        tmp.flush().expect("tempfile flush failed");

        let (manifest, sig, members) = extract_pack_zip(tmp.path()).expect("valid zip should extract");
        assert_eq!(manifest, b"manifest-bytes");
        assert!(sig.is_none());
        assert_eq!(members.len(), 2);
        assert_eq!(members.get("a.wasm").map(Vec::as_slice), Some(&b"wasm-a"[..]));
    }

    #[test]
    fn extract_pack_zip_rejects_directory_entry() {
        let buf = std::io::Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(buf);
        zip.add_directory("subdir/", zip::write::SimpleFileOptions::default()).expect("add_directory failed");
        let zip_bytes = zip.finish().expect("zip finish failed").into_inner();
        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile create failed");
        tmp.write_all(&zip_bytes).expect("tempfile write failed");
        tmp.flush().expect("tempfile flush failed");

        let err = extract_pack_zip(tmp.path()).expect_err("a directory entry should be rejected");
        assert!(err.starts_with("pack_invalid:"));
    }

    #[test]
    fn extract_pack_zip_rejects_nested_path_entry() {
        let zip_bytes = write_test_zip(&[("sub/evil.wasm", b"x")]);
        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile create failed");
        tmp.write_all(&zip_bytes).expect("tempfile write failed");
        tmp.flush().expect("tempfile flush failed");

        let err = extract_pack_zip(tmp.path()).expect_err("a nested path entry should be rejected");
        assert!(err.starts_with("pack_invalid:"));
    }

    #[test]
    fn extract_pack_zip_rejects_unexpected_entry_type() {
        let zip_bytes = write_test_zip(&[("pack.json", b"m"), ("readme.txt", b"hi")]);
        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile create failed");
        tmp.write_all(&zip_bytes).expect("tempfile write failed");
        tmp.flush().expect("tempfile flush failed");

        let err = extract_pack_zip(tmp.path()).expect_err("a non-manifest, non-.wasm entry should be rejected");
        assert!(err.starts_with("pack_invalid:"));
    }

    #[test]
    fn extract_pack_zip_rejects_missing_manifest() {
        let zip_bytes = write_test_zip(&[("a.wasm", b"x")]);
        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile create failed");
        tmp.write_all(&zip_bytes).expect("tempfile write failed");
        tmp.flush().expect("tempfile flush failed");

        let err = extract_pack_zip(tmp.path()).expect_err("an archive with no pack.json should be rejected");
        assert!(err.starts_with("pack_invalid:"));
    }

    #[test]
    fn extract_pack_zip_rejects_entry_count_over_cap() {
        let mut entries: Vec<(String, Vec<u8>)> =
            (0..(PACK_MAX_ENTRIES + 1)).map(|i| (format!("m{i}.wasm"), Vec::new())).collect();
        entries.push(("pack.json".to_string(), Vec::new()));
        let entry_refs: Vec<(&str, &[u8])> = entries.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
        let zip_bytes = write_test_zip(&entry_refs);
        let mut tmp = tempfile::NamedTempFile::new().expect("tempfile create failed");
        tmp.write_all(&zip_bytes).expect("tempfile write failed");
        tmp.flush().expect("tempfile flush failed");

        let err = extract_pack_zip(tmp.path()).expect_err("an archive over the entry cap should be rejected");
        assert!(err.starts_with("pack_invalid:"));
    }

    // --- check_pack_signature ---

    #[test]
    fn check_pack_signature_returns_unsigned_when_no_sidecar() {
        let actual = vec![SignedFileEntry { name: "pack.json".to_string(), blake3: "abc".to_string() }];
        assert!(matches!(check_pack_signature(None, &actual), SigCheckResult::Unsigned));
    }

    #[test]
    fn check_pack_signature_returns_valid_for_a_correctly_signed_pack() {
        let signing_key = signing_key_fixture();
        let actual = vec![
            SignedFileEntry { name: "pack.json".to_string(), blake3: blake3::hash(b"manifest").to_hex().to_string() },
            SignedFileEntry { name: "a.wasm".to_string(), blake3: blake3::hash(b"wasm-a").to_hex().to_string() },
        ];
        let sig_raw = sig_file_json(&signing_key, &actual);
        let result = check_pack_signature(Some(&sig_raw), &actual);
        assert!(matches!(result, SigCheckResult::Valid(_)), "expected a valid signature check result");
    }

    #[test]
    fn check_pack_signature_detects_integrity_mismatch_on_tampered_content() {
        let signing_key = signing_key_fixture();
        let signed = vec![SignedFileEntry { name: "pack.json".to_string(), blake3: blake3::hash(b"manifest").to_hex().to_string() }];
        let sig_raw = sig_file_json(&signing_key, &signed);
        let actual = vec![SignedFileEntry { name: "pack.json".to_string(), blake3: blake3::hash(b"tampered-manifest").to_hex().to_string() }];
        let result = check_pack_signature(Some(&sig_raw), &actual);
        assert!(matches!(result, SigCheckResult::IntegrityMismatch));
    }

    #[test]
    fn check_pack_signature_detects_a_file_count_mismatch() {
        let signing_key = signing_key_fixture();
        let signed = vec![SignedFileEntry { name: "pack.json".to_string(), blake3: blake3::hash(b"manifest").to_hex().to_string() }];
        let sig_raw = sig_file_json(&signing_key, &signed);
        let actual = vec![
            SignedFileEntry { name: "pack.json".to_string(), blake3: blake3::hash(b"manifest").to_hex().to_string() },
            SignedFileEntry { name: "a.wasm".to_string(), blake3: blake3::hash(b"wasm-a").to_hex().to_string() },
        ];
        let result = check_pack_signature(Some(&sig_raw), &actual);
        assert!(matches!(result, SigCheckResult::IntegrityMismatch));
    }

    #[test]
    fn check_pack_signature_rejects_tampered_signature_bytes() {
        let signing_key = signing_key_fixture();
        let actual = vec![SignedFileEntry { name: "pack.json".to_string(), blake3: blake3::hash(b"manifest").to_hex().to_string() }];
        let sig_raw = sig_file_json(&signing_key, &actual);
        let mut sig_val: serde_json::Value = serde_json::from_slice(&sig_raw).expect("sig json should parse");
        sig_val["signature"] = serde_json::Value::String(BASE64.encode([0u8; 64]));
        let tampered = serde_json::to_vec(&sig_val).expect("sig json should serialize");

        let result = check_pack_signature(Some(&tampered), &actual);
        assert!(matches!(result, SigCheckResult::SignatureInvalid));
    }

    #[test]
    fn check_pack_signature_rejects_broken_json_and_unknown_schema_version() {
        assert!(matches!(check_pack_signature(Some(b"not json"), &[]), SigCheckResult::Malformed(_)));

        let raw = serde_json::json!({"schema_version": 99, "algorithm": "ed25519", "public_key": "", "files": [], "signature": ""})
            .to_string()
            .into_bytes();
        assert!(matches!(check_pack_signature(Some(&raw), &[]), SigCheckResult::Unrecognized));
    }

    // --- scan_installed_packs ---

    #[test]
    fn scan_installed_packs_finds_and_parses_a_valid_manifest() {
        let dir = tempfile::tempdir().expect("tempdir create failed");
        let manifest = sample_manifest_json("com.example.pack", &["a.wasm"]);
        std::fs::write(dir.path().join("com.example.pack.aerini-pack.json"), &manifest).expect("manifest write failed");

        let packs = scan_installed_packs(dir.path());
        assert_eq!(packs.len(), 1, "expected exactly one installed pack to be found");
        assert!(packs.contains_key("com.example.pack"));
    }

    #[test]
    fn scan_installed_packs_skips_a_corrupt_manifest_without_erroring() {
        let dir = tempfile::tempdir().expect("tempdir create failed");
        std::fs::write(dir.path().join("broken.aerini-pack.json"), b"not json").expect("manifest write failed");

        let packs = scan_installed_packs(dir.path());
        assert!(packs.is_empty(), "a corrupt manifest should be skipped, not surfaced as a pack");
    }

    // --- remove_plugin_pack ---

    #[tokio::test]
    async fn remove_plugin_pack_removes_manifest_members_and_sig() {
        let dir = tempfile::tempdir().expect("tempdir create failed");
        let pack_id = "com.example.pack";
        let manifest = sample_manifest_json(pack_id, &["a.wasm", "b.wasm"]);
        std::fs::write(dir.path().join(format!("{pack_id}.aerini-pack.json")), &manifest).expect("manifest write failed");
        std::fs::write(dir.path().join(format!("{pack_id}.aerini-pack.json.sig")), b"sig").expect("sig write failed");
        std::fs::write(dir.path().join("a.wasm"), b"wasm-a").expect("member write failed");
        std::fs::write(dir.path().join("b.wasm"), b"wasm-b").expect("member write failed");

        let result = remove_plugin_pack(pack_id.to_string(), dir.path().display().to_string()).await;
        assert!(result.is_ok(), "expected removal to succeed, got: {result:?}");
        assert!(!dir.path().join(format!("{pack_id}.aerini-pack.json")).exists());
        assert!(!dir.path().join(format!("{pack_id}.aerini-pack.json.sig")).exists());
        assert!(!dir.path().join("a.wasm").exists());
        assert!(!dir.path().join("b.wasm").exists());
    }

    #[tokio::test]
    async fn remove_plugin_pack_errors_when_pack_not_found() {
        let dir = tempfile::tempdir().expect("tempdir create failed");
        let result = remove_plugin_pack("com.example.missing".to_string(), dir.path().display().to_string()).await;
        assert!(result.is_err(), "expected an error for a pack that was never installed");
    }

    #[tokio::test]
    async fn remove_plugin_pack_rejects_invalid_pack_id() {
        let dir = tempfile::tempdir().expect("tempdir create failed");
        let result = remove_plugin_pack("../escape".to_string(), dir.path().display().to_string()).await;
        assert!(result.is_err(), "expected an invalid pack_id to be rejected");
    }

    // --- install_plugin_pack_from_path (early-exit paths only — see module doc) ---

    #[tokio::test]
    async fn install_plugin_pack_from_path_rejects_bad_source_file() {
        let dir = tempfile::tempdir().expect("tempdir create failed");

        let mut wrong_ext = tempfile::NamedTempFile::new().expect("tempfile create failed");
        wrong_ext.write_all(b"not a real package").expect("tempfile write failed");
        wrong_ext.flush().expect("tempfile flush failed");
        let result = install_plugin_pack_from_path(wrong_ext.path().display().to_string(), dir.path().display().to_string(), false).await;
        assert!(result.is_err(), "expected a non-.aerinipkg file to be rejected");

        let result = install_plugin_pack_from_path("/nonexistent/path/pack.aerinipkg".to_string(), dir.path().display().to_string(), false).await;
        assert!(result.is_err(), "expected a missing source file to be rejected");
    }

    #[tokio::test]
    async fn install_plugin_pack_from_path_rejects_malformed_manifest() {
        let manifest = serde_json::json!({
            "schema_version": 2, "pack_id": "com.example.pack", "display_name": "P", "files": ["a.wasm"]
        }).to_string();
        let tmp = write_pack_file(&[("pack.json", manifest.as_bytes()), ("a.wasm", b"x")]);
        let dir = tempfile::tempdir().expect("tempdir create failed");

        let result = install_plugin_pack_from_path(tmp.path().display().to_string(), dir.path().display().to_string(), false).await;
        let err = result.expect_err("unsupported schema_version should be rejected");
        assert!(err.starts_with("pack_invalid:"));
    }

    #[tokio::test]
    async fn install_plugin_pack_from_path_rejects_content_not_matching_manifest() {
        let manifest = sample_manifest_json("com.example.pack", &["a.wasm", "b.wasm"]);
        let tmp = write_pack_file(&[("pack.json", &manifest), ("a.wasm", b"x")]);
        let dir = tempfile::tempdir().expect("tempdir create failed");

        let result = install_plugin_pack_from_path(tmp.path().display().to_string(), dir.path().display().to_string(), false).await;
        let err = result.expect_err("a member set that doesn't match the manifest should be rejected");
        assert!(err.starts_with("pack_invalid:"));
    }

    #[tokio::test]
    async fn install_plugin_pack_from_path_rejects_when_already_installed_without_overwrite() {
        let pack_id = "com.example.pack";
        let manifest = sample_manifest_json(pack_id, &["a.wasm"]);
        let dir = tempfile::tempdir().expect("tempdir create failed");
        std::fs::write(dir.path().join(format!("{pack_id}.aerini-pack.json")), &manifest).expect("manifest write failed");
        std::fs::write(dir.path().join("a.wasm"), b"existing-wasm").expect("member write failed");

        let tmp = write_pack_file(&[("pack.json", &manifest), ("a.wasm", b"new-wasm")]);
        let result = install_plugin_pack_from_path(tmp.path().display().to_string(), dir.path().display().to_string(), false).await;
        let err = result.expect_err("reinstalling without overwrite should be rejected");
        assert!(err.starts_with("pack_already_installed:"));
    }

    #[tokio::test]
    async fn install_plugin_pack_from_path_rejects_integrity_mismatch() {
        let signing_key = signing_key_fixture();
        let manifest = sample_manifest_json("com.example.pack", &["a.wasm"]);
        let signed = vec![
            SignedFileEntry { name: "pack.json".to_string(), blake3: blake3::hash(&manifest).to_hex().to_string() },
            SignedFileEntry { name: "a.wasm".to_string(), blake3: blake3::hash(b"wasm-a").to_hex().to_string() },
        ];
        let sig_raw = sig_file_json(&signing_key, &signed);
        // Ships different member content than what the signature covers.
        let tmp = write_pack_file(&[("pack.json", &manifest), ("a.wasm", b"tampered"), ("pack.json.sig", &sig_raw)]);
        let dir = tempfile::tempdir().expect("tempdir create failed");

        let result = install_plugin_pack_from_path(tmp.path().display().to_string(), dir.path().display().to_string(), false).await;
        let err = result.expect_err("tampered member content should fail the integrity check");
        assert!(err.starts_with("pack_integrity_failed:"));
    }

    #[tokio::test]
    async fn install_plugin_pack_from_path_rejects_invalid_signature() {
        let manifest = sample_manifest_json("com.example.pack", &["a.wasm"]);
        let bad_sig = serde_json::json!({
            "schema_version": SIG_SCHEMA_VERSION,
            "algorithm": SIG_ALGORITHM,
            "public_key": BASE64.encode([1u8; 32]),
            "files": [
                {"name": "pack.json", "blake3": blake3::hash(&manifest).to_hex().to_string()},
                {"name": "a.wasm", "blake3": blake3::hash(b"wasm-a").to_hex().to_string()},
            ],
            "signature": BASE64.encode([0u8; 64]),
        }).to_string();
        let tmp = write_pack_file(&[("pack.json", &manifest), ("a.wasm", b"wasm-a"), ("pack.json.sig", bad_sig.as_bytes())]);
        let dir = tempfile::tempdir().expect("tempdir create failed");

        let result = install_plugin_pack_from_path(tmp.path().display().to_string(), dir.path().display().to_string(), false).await;
        let err = result.expect_err("a signature that doesn't verify should be rejected");
        assert!(err.starts_with("pack_signature_invalid:"));
    }

    #[tokio::test]
    async fn install_plugin_pack_from_path_rejects_signature_downgrade_even_with_overwrite() {
        let pack_id = "com.example.pack";
        let dir = tempfile::tempdir().expect("tempdir create failed");

        // Pre-seed the trust store as if a prior install had a verified signature.
        let signing_key = signing_key_fixture();
        let mut store = HashMap::new();
        store.insert(format!("pack:{pack_id}"), BASE64.encode(signing_key.verifying_key().to_bytes()));
        save_trust_store(dir.path(), &store).expect("trust store write failed");

        let manifest = sample_manifest_json(pack_id, &["a.wasm"]);
        let tmp = write_pack_file(&[("pack.json", &manifest), ("a.wasm", b"wasm-a")]); // no pack.json.sig

        // overwrite: true — proves the downgrade guard isn't bypassable by
        // the same flag that authorizes a legitimate update.
        let result = install_plugin_pack_from_path(tmp.path().display().to_string(), dir.path().display().to_string(), true).await;
        let err = result.expect_err("an unsigned update to a previously-trusted pack should be rejected");
        assert!(err.starts_with("pack_signature_downgrade:"));
    }
}
