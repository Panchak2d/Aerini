use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use aerini_engine::plugin_loader::signature::{PluginSignatureFile, SIG_ALGORITHM, SIG_SCHEMA_VERSION, SigCheckResult, signed_message, SignedFileEntry};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::Deserialize;
use zip::ZipArchive;

pub(super) const PACK_SCHEMA_VERSION: u32 = 1;
pub(super) const PACK_MAX_ENTRIES: usize = 64;
pub(super) const PACK_MAX_MEMBER_BYTES: u64 = 64 * 1024 * 1024;
const PACK_MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
pub(super) const PACK_MANIFEST_MAX_BYTES: u64 = 64 * 1024;
pub(super) const PACK_MANIFEST_ENTRY_NAME: &str = "pack.json";
const PACK_SIG_ENTRY_NAME: &str = "pack.json.sig";

#[derive(Deserialize, Clone, Debug)]
pub(super) struct PackManifest {
    pub(super) schema_version: u32,
    pub(super) pack_id: String,
    pub(super) display_name: String,
    pub(super) files: Vec<String>,
}

/// `pack_id` must match `^[A-Za-z0-9][A-Za-z0-9._-]*$` — same discipline as
/// a plugin `type_id`, reverse-domain recommended. Hand-rolled rather than
/// pulling in a `regex` dependency for one fixed, simple charset check.
pub(super) fn is_valid_pack_id(id: &str) -> bool {
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
fn is_portable_filename(name: &str) -> bool {
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
        "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if name.chars().any(|c| c.is_control() || matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|')) {
        return false;
    }
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    if stem.is_empty() || stem.ends_with([' ', '.']) {
        return false;
    }
    let device = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    !RESERVED.contains(&device.as_str())
}

pub(super) fn parse_pack_manifest(raw: &[u8]) -> Result<PackManifest, String> {
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
        if !is_portable_filename(name) {
            return Err(format!(
                "pack_invalid: member filename '{name}' uses a character or name that is not valid on every platform"
            ));
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
pub(super) fn pack_manifest_path(plugin_dir: &Path, pack_id: &str) -> PathBuf {
    plugin_dir.join(format!("{pack_id}.aerini-pack.json"))
}

/// Scans `plugin_dir` for `*.aerini-pack.json` manifests and returns every
/// one that parses successfully, keyed by `pack_id`. A manifest that fails
/// to parse (corrupt, hand-edited, from a newer app version) is silently
/// skipped rather than surfaced as an error here — mirrors `scan_type_ids`'s
/// treatment of a `.wasm` file that fails to describe.
pub(super) fn scan_installed_packs(plugin_dir: &Path) -> HashMap<String, PackManifest> {
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
pub(super) fn read_entry_bounded(mut r: impl std::io::Read, cap: u64) -> Result<Vec<u8>, String> {
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
pub(super) type PackZipContents = (Vec<u8>, Option<Vec<u8>>, HashMap<String, Vec<u8>>);

pub(super) fn extract_pack_zip(src: &Path) -> Result<PackZipContents, String> {
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
        let raw_name = entry
            .name()
            .map_err(|e| format!("pack_invalid: archive entry has an unreadable name ({e})"))?
            .into_owned();
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

        let bytes = read_entry_bounded(&mut entry, cap)?;
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
pub(super) fn check_pack_signature(sig_raw: Option<&[u8]>, actual: &[SignedFileEntry]) -> SigCheckResult {
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
