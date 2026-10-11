use std::collections::HashMap;
use std::path::{Component, Path, PathBuf, Prefix};

use serde_json::Value;

use crate::error::NodeError;
use crate::model::NodeOutput;

pub(crate) const SANDBOX_KEY: &str = "__file_sandbox_dir";

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SandboxError {
    BadRoot,
    Outside,
    Unresolvable,
}

impl SandboxError {
    pub(crate) fn into_output(self, subject: &str, root: Option<&Path>) -> NodeOutput {
        let err = match self {
            SandboxError::BadRoot => NodeError::unrecoverable(
                "INVALID_PATH",
                "Configured sandbox directory does not exist or cannot be resolved",
            ),
            SandboxError::Outside => NodeError::unrecoverable(
                "PATH_OUTSIDE_SANDBOX",
                match root {
                    Some(r) => format!("{subject} is outside the permitted directory '{}'", r.display()),
                    None => format!("{subject} is outside the permitted directory"),
                },
            ),
            SandboxError::Unresolvable => NodeError::unrecoverable(
                "INVALID_PATH",
                format!("{subject} cannot be verified to stay inside the permitted directory"),
            ),
        };
        NodeOutput::failure(err)
    }
}

/// Reads the server's file sandbox root from node metadata. `Ok(None)` means no
/// sandbox is configured. A configured root that is not a usable directory
/// fails closed rather than lifting the restriction.
pub(crate) fn root_from_metadata(
    metadata: &HashMap<String, Value>,
) -> Result<Option<PathBuf>, SandboxError> {
    let Some(value) = metadata.get(SANDBOX_KEY) else {
        return Ok(None);
    };
    let configured = value
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or(SandboxError::BadRoot)?;
    std::fs::canonicalize(configured)
        .map(Some)
        .map_err(|_| SandboxError::BadRoot)
}

/// Resolves `requested` (absolute, or relative to `root`) to a path that is
/// guaranteed to sit inside the canonical `root`, even when the tail of it does
/// not exist yet.
///
/// The deepest existing ancestor is canonicalized (dereferencing every symlink
/// on the way, intermediate directories included) and checked for containment;
/// the missing tail is then re-joined. A tail component that exists but could
/// not be canonicalized is a dangling symlink (or unreadable entry): writing
/// through one would create its target outside the root, so it is refused.
pub(crate) fn resolve(root: &Path, requested: &Path) -> Result<PathBuf, SandboxError> {
    for component in requested.components() {
        match component {
            Component::ParentDir => return Err(SandboxError::Outside),
            Component::Prefix(p)
                if matches!(
                    p.kind(),
                    Prefix::UNC(..) | Prefix::VerbatimUNC(..) | Prefix::DeviceNS(..)
                ) =>
            {
                return Err(SandboxError::Outside);
            }
            _ => {}
        }
    }

    let abs = root.join(requested);
    let mut existing: &Path = &abs;
    let canonical = loop {
        match std::fs::canonicalize(existing) {
            Ok(p) => break p,
            Err(_) => existing = existing.parent().ok_or(SandboxError::Unresolvable)?,
        }
    };
    if !canonical.starts_with(root) {
        return Err(SandboxError::Outside);
    }

    let suffix = abs
        .strip_prefix(existing)
        .map_err(|_| SandboxError::Unresolvable)?;
    if let Some(first) = suffix.components().next() {
        if std::fs::symlink_metadata(existing.join(first)).is_ok() {
            return Err(SandboxError::Unresolvable);
        }
    }
    Ok(canonical.join(suffix))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canonical_tmp() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        (dir, root)
    }

    #[test]
    fn relative_and_missing_paths_resolve_inside_root() {
        let (_guard, root) = canonical_tmp();
        std::fs::create_dir(root.join("real")).unwrap();

        assert_eq!(resolve(&root, Path::new("a/b/c.txt")).unwrap(), root.join("a/b/c.txt"));
        assert_eq!(resolve(&root, Path::new("real/new.txt")).unwrap(), root.join("real/new.txt"));
        assert_eq!(resolve(&root, Path::new("")).unwrap(), root);
        let absolute = root.join("real").join("x.txt");
        assert_eq!(resolve(&root, &absolute).unwrap(), absolute);
    }

    #[test]
    fn paths_outside_root_are_refused() {
        let (_guard, root) = canonical_tmp();
        let (_other_guard, other) = canonical_tmp();

        assert_eq!(resolve(&root, &other.join("x.txt")), Err(SandboxError::Outside));
        assert_eq!(resolve(&root, Path::new("a/../../x")), Err(SandboxError::Outside));
        assert_eq!(resolve(&root, Path::new("/definitely/not/here/x")), Err(SandboxError::Outside));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escapes_are_refused() {
        let (_guard, root) = canonical_tmp();
        let (_other_guard, other) = canonical_tmp();

        std::os::unix::fs::symlink(&other, root.join("dir_link")).unwrap();
        assert_eq!(resolve(&root, Path::new("dir_link/new.txt")), Err(SandboxError::Outside));

        std::fs::write(other.join("secret"), "x").unwrap();
        std::os::unix::fs::symlink(other.join("secret"), root.join("file_link")).unwrap();
        assert_eq!(resolve(&root, Path::new("file_link")), Err(SandboxError::Outside));
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_is_refused_even_though_its_parent_is_inside() {
        let (_guard, root) = canonical_tmp();
        let (_other_guard, other) = canonical_tmp();

        std::os::unix::fs::symlink(other.join("not_yet"), root.join("dangling")).unwrap();
        assert_eq!(resolve(&root, Path::new("dangling")), Err(SandboxError::Unresolvable));
        assert_eq!(resolve(&root, Path::new("dangling/deeper.txt")), Err(SandboxError::Unresolvable));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_to_an_existing_file_inside_root_is_followed() {
        let (_guard, root) = canonical_tmp();
        std::fs::write(root.join("target.txt"), "x").unwrap();
        std::os::unix::fs::symlink(root.join("target.txt"), root.join("alias")).unwrap();
        assert_eq!(resolve(&root, Path::new("alias")).unwrap(), root.join("target.txt"));
    }

    #[test]
    fn root_from_metadata_fails_closed() {
        let mut meta: HashMap<String, Value> = HashMap::new();
        assert_eq!(root_from_metadata(&meta), Ok(None));

        meta.insert(SANDBOX_KEY.into(), Value::from(7));
        assert_eq!(root_from_metadata(&meta), Err(SandboxError::BadRoot));

        meta.insert(SANDBOX_KEY.into(), Value::from("/no/such/dir/for/sandbox"));
        assert_eq!(root_from_metadata(&meta), Err(SandboxError::BadRoot));

        let (_guard, root) = canonical_tmp();
        meta.insert(SANDBOX_KEY.into(), Value::from(root.to_str().unwrap()));
        assert_eq!(root_from_metadata(&meta), Ok(Some(root)));
    }
}
