//! Scratch files for media the webview plays or saves (video and audio
//! outputs), kept in a private directory under the OS temp directory.

use std::io::Write as _;
use std::path::{Path, PathBuf};

const DIR_NAME: &str = "aerini_media";
const MAX_STEM_BYTES: usize = 100;
const MAX_EXT_CHARS: usize = 8;
const HASH_PREFIX_HEX_CHARS: usize = 32;

pub(crate) fn dir_path() -> PathBuf {
    std::env::temp_dir().join(DIR_NAME)
}

/// Reduces `filename` to one portable file name: only its last path segment,
/// with every character other than letters, digits, `.`, `-`, `_` and space
/// replaced by `_`, no leading or trailing dots or spaces (Windows drops
/// trailing ones, so the file would not be where the caller expects), and a
/// bounded length. `None` when nothing is left of the last segment.
pub(crate) fn sanitize_file_name(filename: &str) -> Option<String> {
    let last = filename.rsplit(['/', '\\']).next().unwrap_or("").trim();
    if last.is_empty() {
        return None;
    }
    let cleaned: String = last
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ') { c } else { '_' })
        .collect();
    let (stem, ext) = match cleaned.rfind('.') {
        Some(i) if i > 0 && is_plain_extension(&cleaned[i + 1..]) => (&cleaned[..i], &cleaned[i + 1..]),
        _ => (cleaned.as_str(), ""),
    };
    let stem = stem.trim_matches(['.', ' ']);
    let stem = truncate_to_bytes(stem, MAX_STEM_BYTES).trim_end_matches(['.', ' ']);
    let stem = if stem.is_empty() { "media" } else { stem };
    Some(if ext.is_empty() { stem.to_string() } else { format!("{stem}.{ext}") })
}

/// A short ASCII-alphanumeric extension such as `mp4`; anything else stays
/// part of the stem rather than becoming a misleading extension.
fn is_plain_extension(ext: &str) -> bool {
    !ext.is_empty() && ext.len() <= MAX_EXT_CHARS && ext.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn truncate_to_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Creates `dir` if needed and confirms it is a real directory (not a
/// symlink) that only this user can use. On Unix the system temp directory is
/// shared, so another user could have created the path first; `chmod` fails
/// unless this process owns the directory, which rejects that case.
fn prepare_dir_at(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(not(unix))]
    let builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let meta = std::fs::symlink_metadata(dir)?;
    if !meta.is_dir() {
        return Err(std::io::Error::other(format!("{} is not a directory", dir.display())));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

fn target_path(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let hash = blake3::hash(bytes).to_hex();
    dir.join(format!("{}-{name}", &hash.as_str()[..HASH_PREFIX_HEX_CHARS]))
}

/// Writes `bytes` to `dir` under a name made from their hash and the
/// sanitized `filename`, so the same media is stored once however often it is
/// rendered, and returns the path. The bytes go to a fresh private file that
/// is then renamed into place; a rename replaces whatever entry is at the
/// target (a symlink included) instead of writing through it.
fn store_in(dir: &Path, filename: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let name = sanitize_file_name(filename)
        .ok_or_else(|| "Invalid filename: empty after sanitisation".to_string())?;
    let target = target_path(dir, &name, bytes);
    if std::fs::symlink_metadata(&target)
        .is_ok_and(|m| m.is_file() && m.len() == bytes.len() as u64)
    {
        return Ok(target);
    }
    let mut tmp = tempfile::NamedTempFile::new_in(dir)
        .map_err(|e| format!("Write failed: {e}"))?;
    tmp.write_all(bytes)
        .and_then(|()| tmp.flush())
        .map_err(|e| format!("Write failed: {e}"))?;
    tmp.persist(&target).map_err(|e| format!("Write failed: {}", e.error))?;
    Ok(target)
}

/// Stores `bytes` in the media scratch directory and returns the file's path.
pub(crate) fn store(filename: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let dir = dir_path();
    prepare_dir_at(&dir).map_err(|e| format!("Failed to prepare temp dir: {e}"))?;
    store_in(&dir, filename, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_keeps_an_ordinary_name() {
        assert_eq!(sanitize_file_name("clip.mp4").as_deref(), Some("clip.mp4"));
        assert_eq!(sanitize_file_name("my clip-1_a.tar.gz").as_deref(), Some("my clip-1_a.tar.gz"));
    }

    #[test]
    fn sanitize_keeps_only_the_last_path_segment() {
        assert_eq!(sanitize_file_name("../../etc/passwd.mp4").as_deref(), Some("passwd.mp4"));
        assert_eq!(sanitize_file_name("a\\b\\c.wav").as_deref(), Some("c.wav"));
    }

    #[test]
    fn sanitize_neutralises_drive_prefixes_and_stream_markers() {
        assert_eq!(sanitize_file_name("C:evil.mp4").as_deref(), Some("C_evil.mp4"));
        assert_eq!(sanitize_file_name("a.mp4:stream").as_deref(), Some("a.mp4_stream"));
        assert_eq!(sanitize_file_name("x\0y.mp4").as_deref(), Some("x_y.mp4"));
    }

    #[test]
    fn sanitize_drops_trailing_dots_and_spaces() {
        assert_eq!(sanitize_file_name("clip.mp4. .").as_deref(), Some("clip.mp4"));
        assert_eq!(sanitize_file_name("clip. ").as_deref(), Some("clip"));
    }

    #[test]
    fn sanitize_falls_back_to_a_placeholder_stem_or_none() {
        assert_eq!(sanitize_file_name("....").as_deref(), Some("media"));
        assert_eq!(sanitize_file_name(".mp4").as_deref(), Some("mp4"));
        assert_eq!(sanitize_file_name("dir/").as_deref(), None);
        assert_eq!(sanitize_file_name("   ").as_deref(), None);
        assert_eq!(sanitize_file_name("").as_deref(), None);
    }

    #[test]
    fn sanitize_bounds_the_length_on_a_char_boundary() {
        let long = format!("{}.mp4", "é".repeat(200));
        let out = sanitize_file_name(&long).unwrap();
        assert!(out.ends_with(".mp4"));
        assert!(out.len() <= MAX_STEM_BYTES + 1 + 4);
        assert!(out.is_char_boundary(out.len() - 4));
    }

    #[test]
    fn same_media_is_stored_once_and_different_media_separately() {
        let dir = tempfile::tempdir().unwrap();
        let a = store_in(dir.path(), "out.mp4", b"first").unwrap();
        let again = store_in(dir.path(), "out.mp4", b"first").unwrap();
        let other = store_in(dir.path(), "out.mp4", b"second").unwrap();
        assert_eq!(a, again);
        assert_ne!(a, other);
        assert_eq!(std::fs::read(&a).unwrap(), b"first");
        assert_eq!(std::fs::read(&other).unwrap(), b"second");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn stored_path_stays_inside_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = store_in(dir.path(), "../../escape.mp4", b"x").unwrap();
        assert_eq!(path.parent(), Some(dir.path()));
    }

    #[test]
    fn an_unusable_name_is_refused_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(store_in(dir.path(), "", b"x").is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_at_the_target_is_replaced_not_written_through() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim.txt");
        std::fs::write(&victim, b"keep me").unwrap();
        let planted = target_path(dir.path(), "out.mp4", b"payload");
        std::os::unix::fs::symlink(&victim, &planted).unwrap();

        let path = store_in(dir.path(), "out.mp4", b"payload").unwrap();

        assert_eq!(path, planted);
        assert_eq!(std::fs::read(&victim).unwrap(), b"keep me");
        assert_eq!(std::fs::read(&path).unwrap(), b"payload");
        assert!(!std::fs::symlink_metadata(&path).unwrap().file_type().is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn prepare_dir_tightens_a_permissive_directory_we_own() {
        use std::os::unix::fs::PermissionsExt;
        let parent = tempfile::tempdir().unwrap();
        let dir = parent.path().join("media");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();

        prepare_dir_at(&dir).unwrap();

        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn prepare_dir_refuses_a_symlink() {
        let parent = tempfile::tempdir().unwrap();
        let real = parent.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = parent.path().join("media");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        assert!(prepare_dir_at(&link).is_err());
    }

    #[test]
    fn prepare_dir_creates_a_missing_directory() {
        let parent = tempfile::tempdir().unwrap();
        let dir = parent.path().join("media");
        prepare_dir_at(&dir).unwrap();
        assert!(dir.is_dir());
    }
}
