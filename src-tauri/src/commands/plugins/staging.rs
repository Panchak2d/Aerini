use std::path::{Path, PathBuf};

use aerini_engine::plugin_loader::PluginLoader;

pub(super) const STAGED_PREFIX: &str = ".aerini-install-";
const STAGED_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(60 * 60);
pub(super) const SIDECAR_MAX_BYTES: u64 = 64 * 1024;

/// Writes `bytes` to a hidden file beside `dest`, flushes it to disk and
/// renames it over `dest`, so a reader sees the old file or the whole new one.
pub(super) fn write_atomic(dest: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = dest.parent().ok_or_else(|| "Destination has no parent folder".to_string())?;
    let mut tmp = tempfile::Builder::new()
        .prefix(STAGED_PREFIX)
        .suffix(".part")
        .tempfile_in(dir)
        .map_err(|e| format!("Failed to stage '{}': {e}", dest.display()))?;
    std::io::Write::write_all(&mut tmp, bytes).map_err(|e| format!("Failed to stage '{}': {e}", dest.display()))?;
    tmp.as_file().sync_all().map_err(|e| format!("Failed to stage '{}': {e}", dest.display()))?;
    tmp.persist(dest).map_err(|e| format!("Failed to write '{}': {}", dest.display(), e.error))?;
    Ok(())
}

/// Reads a whole file, failing when it is longer than `limit` bytes.
pub(super) fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("Failed to read '{}': {e}", path.display()))?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut std::io::Read::take(file, limit + 1), &mut bytes)
        .map_err(|e| format!("Failed to read '{}': {e}", path.display()))?;
    if bytes.len() as u64 > limit {
        return Err(format!("'{}' is larger than {} MiB", path.display(), limit / (1024 * 1024)));
    }
    Ok(bytes)
}

/// Removes staging files that an interrupted install left in `dir`.
pub(super) fn sweep_stale_installs(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with(STAGED_PREFIX) {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > STAGED_MAX_AGE);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

pub(super) struct StagedSidecar(pub(super) PathBuf);

impl Drop for StagedSidecar {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Scans `plugin_dir` for `.wasm` files and returns `(filename, type_id)` for
/// every one that loads successfully. A file that fails to describe is
/// omitted — it has no `type_id` to match against.
pub(super) fn scan_type_ids(dir: &Path, loader: &PluginLoader) -> Vec<(String, String)> {
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
