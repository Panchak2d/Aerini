//! Publisher signature verification for a single `.wasm` file against its
//! optional `<name>.wasm.sig` sidecar. The single load-time enforcement
//! point every [`load_plugins`] caller shares — see
//! [`PluginLoader::load_plugin`]. Also reused by `src-tauri` for its
//! desktop-only trust store and multi-file pack signing, which have no
//! equivalent here (`aerini-engine` has no concept of a "pack").

use std::path::{Path, PathBuf};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::Deserialize;

pub const SIG_SCHEMA_VERSION: u32 = 1;
pub const SIG_ALGORITHM: &str = "ed25519";
const SIG_DOMAIN: &[u8] = b"aerini-plugin-sig-v1\n";

/// One `{name, blake3}` entry inside a [`PluginSignatureFile`]. A list
/// (not a single hash) so a multi-file package manifest can reuse this
/// exact shape with more than one entry under one signature.
#[derive(Deserialize)]
pub struct SignedFileEntry {
    pub name: String,
    pub blake3: String,
}

/// Shape of an optional `<name>.wasm.sig` sidecar:
/// `{schema_version, algorithm, public_key (base64, 32 bytes),
/// files: [{name, blake3}], signature (base64, 64 bytes)}`. `name`
/// inside each entry is informational only — matching is by content
/// hash.
#[derive(Deserialize)]
pub struct PluginSignatureFile {
    pub schema_version: u32,
    pub algorithm: String,
    pub public_key: String,
    pub files: Vec<SignedFileEntry>,
    pub signature: String,
}

/// Outcome of checking a `.wasm` file against its sidecar, if any. Pure
/// and read-only.
#[derive(Debug, Clone)]
pub enum SigCheckResult {
    /// No `.sig` sidecar next to the file.
    Unsigned,
    /// Sidecar present but declares a `schema_version`/`algorithm`/`files`
    /// shape this build doesn't understand — e.g. from a newer app
    /// version. Informational, not a failure.
    Unrecognized,
    /// Sidecar present but malformed: bad JSON, or a key/signature that
    /// doesn't decode to the expected byte length.
    Malformed(String),
    /// The sidecar's declared content hash doesn't match the actual file.
    IntegrityMismatch,
    /// Hash matched but the Ed25519 signature itself doesn't verify.
    SignatureInvalid,
    /// Hash and signature both verify, under this public key.
    Valid(VerifyingKey),
}

/// Path of the optional signature sidecar for a given `.wasm` path.
pub fn sig_sidecar_path(wasm_path: &Path) -> PathBuf {
    let mut p = wasm_path.as_os_str().to_owned();
    p.push(".sig");
    PathBuf::from(p)
}

/// Deterministic message the signature is computed over: a
/// domain-separation prefix followed by each file entry's
/// `name`/`blake3` hash, name-sorted so the result doesn't depend on the
/// order `files` happens to be written in.
pub fn signed_message(entries: &[SignedFileEntry]) -> Vec<u8> {
    let mut sorted: Vec<&SignedFileEntry> = entries.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    let mut msg = SIG_DOMAIN.to_vec();
    for e in sorted {
        msg.extend_from_slice(e.name.as_bytes());
        msg.push(0);
        msg.extend_from_slice(e.blake3.as_bytes());
        msg.push(b'\n');
    }
    msg
}

/// Checks `wasm_bytes` (the file's actual current content) against the
/// optional sidecar at `sig_sidecar_path(wasm_path)`.
pub fn check_signature(wasm_path: &Path, wasm_bytes: &[u8]) -> SigCheckResult {
    let raw = match std::fs::read_to_string(sig_sidecar_path(wasm_path)) {
        Ok(s) => s,
        Err(_) => return SigCheckResult::Unsigned,
    };
    let sig: PluginSignatureFile = match serde_json::from_str(&raw) {
        Ok(s) => s,
        Err(e) => return SigCheckResult::Malformed(e.to_string()),
    };
    if sig.schema_version != SIG_SCHEMA_VERSION || sig.algorithm != SIG_ALGORITHM || sig.files.len() != 1 {
        return SigCheckResult::Unrecognized;
    }

    let actual_hash = blake3::hash(wasm_bytes).to_hex().to_string();
    if sig.files[0].blake3 != actual_hash {
        return SigCheckResult::IntegrityMismatch;
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
