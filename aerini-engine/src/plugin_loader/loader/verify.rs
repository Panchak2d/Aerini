use std::path::Path;

use crate::plugin_loader::signature;
use crate::plugin_loader::error::PluginLoadError;
use super::{CachedSignature, PluginLoader};

impl PluginLoader {
    /// Checks `path`'s signature sidecar (if any), reusing the cached result
    /// when the file's `(mtime, len)` fingerprint hasn't changed since the
    /// last check — same fingerprint shape `compile_and_link` uses, so
    /// repeated hot-reload polling doesn't re-hash an unchanged file.
    pub(super) fn verify_signature(&self, path: &Path) -> Result<signature::SigCheckResult, PluginLoadError> {
        let meta = std::fs::metadata(path)?;
        let fingerprint = (meta.modified().ok(), meta.len());

        let cached = self
            .signature_cache
            .lock()
            .expect("signature cache mutex poisoned")
            .get(path)
            .filter(|entry| entry.fingerprint == fingerprint)
            .map(|entry| entry.result.clone());

        if let Some(result) = cached {
            return Ok(result);
        }

        let bytes = std::fs::read(path)?;
        let result = signature::check_signature(path, &bytes);
        self.signature_check_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        self.signature_cache
            .lock()
            .expect("signature cache mutex poisoned")
            .insert(path.to_path_buf(), CachedSignature { fingerprint, result: result.clone() });

        Ok(result)
    }

    /// Rejects `path` if its signature sidecar is present but bad (content
    /// doesn't match the signed hash, signature doesn't verify, or sidecar
    /// malformed). Unsigned files and files signed under any key pass. Runs
    /// before any compilation or guest code.
    pub(super) fn require_acceptable_signature(&self, path: &Path) -> Result<(), PluginLoadError> {
        match self.verify_signature(path)? {
            signature::SigCheckResult::IntegrityMismatch => Err(PluginLoadError::SignatureRejected(
                "file does not match its signed checksum — may be corrupted or tampered".to_string(),
            )),
            signature::SigCheckResult::SignatureInvalid => Err(PluginLoadError::SignatureRejected(
                "signature sidecar present but does not verify".to_string(),
            )),
            signature::SigCheckResult::Malformed(reason) => Err(PluginLoadError::SignatureRejected(format!(
                "signature sidecar present but malformed: {reason}"
            ))),
            signature::SigCheckResult::Unsigned
            | signature::SigCheckResult::Unrecognized
            | signature::SigCheckResult::Valid(_) => Ok(()),
        }
    }
}
