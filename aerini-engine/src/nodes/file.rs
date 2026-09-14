use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::fs;

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::Node;

pub struct FileNode;

#[async_trait]
impl Node for FileNode {
    fn type_id(&self) -> &'static str { "file" }
    fn display_name(&self) -> &'static str { "File" }
    fn node_type(&self) -> NodeType { NodeType::Action }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Read, write, or delete a file at a given path on the host machine." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["operation", "path"],
            "properties": {
                "operation": { "type": "string", "enum": ["read", "write", "append", "delete", "exists"] },
                "path":      { "type": "string", "description": "Absolute or relative file path" },
                "content":   { "type": "string", "description": "Content to write (write/append only)" },
                "encoding":  { "type": "string", "enum": ["utf8", "base64"], "description": "Default: utf8" }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "content": { "type": "string" },
                "exists":  { "type": "boolean" },
                "bytes":   { "type": "number" },
                "path":    { "type": "string" }
            }
        })
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let operation = match input.input["operation"].as_str() {
            Some(op) => op.to_string(),
            None => return NodeOutput::failure(NodeError::unrecoverable("MISSING_OP", "operation is required")),
        };
        let raw_path = match input.input["path"].as_str() {
            Some(p) if !p.is_empty() => p.to_string(),
            _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_PATH", "path is required")),
        };

        if raw_path.contains("..") {
            return NodeOutput::failure(NodeError::unrecoverable(
                "INVALID_PATH",
                "Path traversal sequences (..) are not permitted",
            ));
        }

        // In server mode the executor injects __file_sandbox_dir into context metadata.
        // fs::canonicalize fully dereferences all symlinks and requires the target
        // (or its parent, for write/append) to exist on disk. A symlink inside the
        // sandbox whose ultimate target resolves outside is detected and rejected.
        //
        // Without a sandbox the raw_path is used unchanged — no behaviour change for
        // desktop/no-sandbox deployments.
        let path: String = if let Some(sandbox_val) = input.context.metadata.get("__file_sandbox_dir") {
            if let Some(sandbox_str) = sandbox_val.as_str() {
                // Canonicalize the sandbox root itself so that starts_with comparisons
                // work correctly even when the operator's --file-sandbox-dir path
                // contains symlinks. Hard-fail if it can't be resolved — misconfiguration
                // means we cannot make a correct containment decision.
                let sandbox = match std::fs::canonicalize(sandbox_str) {
                    Ok(p) => p,
                    Err(_) => return NodeOutput::failure(NodeError::unrecoverable(
                        "INVALID_PATH",
                        "Configured sandbox directory does not exist or cannot be resolved",
                    )),
                };

                let abs: std::path::PathBuf = if raw_path.starts_with('/') {
                    std::path::PathBuf::from(&raw_path)
                } else {
                    sandbox.join(&raw_path)
                };

                let outside = || NodeOutput::failure(NodeError::unrecoverable(
                    "PATH_OUTSIDE_SANDBOX",
                    format!("File access is restricted to '{}'", sandbox_str),
                ));

                match operation.as_str() {
                    // write/append: the target file may not exist yet.
                    // exists: the target may legitimately not exist (that's a valid,
                    //   non-error "exists: false" result) — but the old lexical fallback
                    //   here trusted an unresolved suffix past a symlinked *intermediate*
                    // directory. All three now resolve through the same
                    //   walk-up-to-nearest-existing-ancestor helper, which also lets
                    //   write/append reach nested, not-yet-created directories inside the
                    // sandbox — create_dir_all runs later, after this check.
                    "write" | "append" | "exists" => {
                        match resolve_within_sandbox(&sandbox, sandbox_str, &abs) {
                            Ok(resolved) => resolved,
                            Err(failure) => return failure,
                        }
                    }
                    _ => {
                        // read, delete: canonicalize the full path.
                        // Returns INVALID_PATH if the file does not exist.
                        match std::fs::canonicalize(&abs) {
                            Ok(cp) if cp.starts_with(&sandbox) => {
                                cp.to_string_lossy().into_owned()
                            }
                            Ok(_)  => return outside(),
                            Err(_) => return NodeOutput::failure(NodeError::unrecoverable(
                                "INVALID_PATH",
                                "Path does not exist or cannot be resolved",
                            )),
                        }
                    }
                }
            } else {
                raw_path
            }
        } else {
            raw_path
        };

        match operation.as_str() {
            "read" => {
                const MAX_READ_BYTES: u64 = 50 * 1024 * 1024;
                match fs::metadata(&path).await {
                    Err(e) => return NodeOutput::failure(NodeError::unrecoverable("READ_ERR", e.to_string())),
                    Ok(meta) if meta.len() > MAX_READ_BYTES => {
                        return NodeOutput::failure(NodeError::unrecoverable(
                            "FILE_TOO_LARGE",
                            format!(
                                "File exceeds {}MB read limit ({} bytes). Use a streaming approach for large files.",
                                MAX_READ_BYTES / (1024 * 1024),
                                meta.len()
                            ),
                        ));
                    }
                    Ok(_) => {}
                }
                match fs::read(&path).await {
                    Err(e) => NodeOutput::failure(NodeError::unrecoverable("READ_ERR", e.to_string())),
                    Ok(bytes) => {
                        let encoding = input.input["encoding"].as_str().unwrap_or("utf8");
                        let content = if encoding == "base64" {
                            use base64::Engine;
                            base64::engine::general_purpose::STANDARD.encode(&bytes)
                        } else {
                            String::from_utf8_lossy(&bytes).to_string()
                        };
                        NodeOutput::success(json!({ "content": content, "bytes": bytes.len(), "path": path }))
                    }
                }
            }
            "write" => {
                let content = input.input["content"].as_str().unwrap_or("");
                let encoding = input.input["encoding"].as_str().unwrap_or("utf8");
                let bytes = match decode_write_content(content, encoding) {
                    Ok(b) => b,
                    Err(e) => return NodeOutput::failure(NodeError::unrecoverable(
                        "INVALID_BASE64",
                        format!("content is not valid base64: {}", e),
                    )),
                };
                if let Some(parent) = std::path::Path::new(&path).parent() {
                    let _ = fs::create_dir_all(parent).await;
                }
                match fs::write(&path, &bytes).await {
                    Err(e) => NodeOutput::failure(NodeError::unrecoverable("WRITE_ERR", e.to_string())),
                    Ok(_)  => NodeOutput::success(json!({ "path": path, "bytes": bytes.len() })),
                }
            }
            "append" => {
                use tokio::io::AsyncWriteExt;
                let content = input.input["content"].as_str().unwrap_or("");
                let encoding = input.input["encoding"].as_str().unwrap_or("utf8");
                let bytes = match decode_write_content(content, encoding) {
                    Ok(b) => b,
                    Err(e) => return NodeOutput::failure(NodeError::unrecoverable(
                        "INVALID_BASE64",
                        format!("content is not valid base64: {}", e),
                    )),
                };
                // Append must create missing parent directories too, matching "write" above.
                if let Some(parent) = std::path::Path::new(&path).parent() {
                    let _ = fs::create_dir_all(parent).await;
                }
                match tokio::fs::OpenOptions::new().create(true).append(true).open(&path).await {
                    Err(e) => NodeOutput::failure(NodeError::unrecoverable("APPEND_ERR", e.to_string())),
                    Ok(mut f) => match f.write_all(&bytes).await {
                        Err(e) => NodeOutput::failure(NodeError::unrecoverable("APPEND_ERR", e.to_string())),
                        // tokio::fs::File's write_all can return before the write has
                        // actually reached the OS; without an explicit flush, dropping
                        // `f` here is not guaranteed to deliver the bytes (per tokio's
                        // own docs). This flush is what "write" above gets for free
                        // from fs::write's single atomic blocking call.
                        Ok(_) => match f.flush().await {
                            Err(e) => NodeOutput::failure(NodeError::unrecoverable("APPEND_ERR", e.to_string())),
                            Ok(_)  => NodeOutput::success(json!({ "path": path, "bytes": bytes.len() })),
                        }
                    }
                }
            }
            "delete" => {
                match fs::remove_file(&path).await {
                    Err(e) => NodeOutput::failure(NodeError::unrecoverable("DELETE_ERR", e.to_string())),
                    Ok(_)  => NodeOutput::success(json!({ "path": path, "deleted": true })),
                }
            }
            "exists" => {
                let exists = std::path::Path::new(&path).exists();
                NodeOutput::success(json!({ "exists": exists, "path": path }))
            }
            _ => NodeOutput::failure(NodeError::unrecoverable("INVALID_OP", format!("Unknown operation: {}", operation))),
        }
    }
}

/// Resolve `abs` to a canonical path guaranteed to be inside `sandbox`, even when
/// `abs` (or trailing components of it) don't exist on disk yet.
///
/// Walks up to the nearest existing ancestor, canonicalizes *that* (dereferencing
/// any symlink along the way — including a symlinked *intermediate* directory, not
/// just the final component), verifies it is contained within `sandbox`, then
/// rejoins the non-existent suffix lexically. That rejoin is safe: a path component
/// that doesn't exist yet cannot itself be a symlink pointing elsewhere.
///
/// Mirrors save_to_folder.rs's canonical_parent/suffix pattern exactly — keep the
/// two in sync if either changes.
fn resolve_within_sandbox(
    sandbox: &std::path::Path,
    sandbox_str: &str,
    abs: &std::path::Path,
) -> Result<String, NodeOutput> {
    let outside = || NodeOutput::failure(NodeError::unrecoverable(
        "PATH_OUTSIDE_SANDBOX",
        format!("File access is restricted to '{}'", sandbox_str),
    ));

    let mut check: &std::path::Path = abs;
    let canonical_ancestor = loop {
        match std::fs::canonicalize(check) {
            Ok(p) => break p,
            Err(_) => match check.parent() {
                Some(p) => check = p,
                None => return Err(NodeOutput::failure(NodeError::unrecoverable(
                    "INVALID_PATH",
                    "Path cannot be resolved to an existing ancestor",
                ))),
            },
        }
    };

    if !canonical_ancestor.starts_with(sandbox) {
        return Err(outside());
    }

    let suffix = match abs.strip_prefix(check) {
        Ok(s) => s,
        Err(_) => return Err(NodeOutput::failure(NodeError::unrecoverable(
            "INVALID_PATH",
            "Path could not be resolved relative to its existing ancestor",
        ))),
    };

    // raw_path already rejects any ".." up in execute() before this fn is ever
    // reached, so suffix can't legitimately contain one — kept as defense in
    // depth (matching save_to_folder.rs's own belt-and-suspenders check) rather
    // than relying solely on that earlier, separate call site.
    if suffix.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err(outside());
    }

    Ok(canonical_ancestor.join(suffix).to_string_lossy().into_owned())
}

/// Decode `content` per `encoding` ("base64" or anything else = utf8 passthrough).
/// Shared by the "write" and "append" arms so both honor `encoding` identically.
fn decode_write_content(content: &str, encoding: &str) -> Result<Vec<u8>, String> {
    if encoding == "base64" {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .decode(content)
            .map_err(|e| e.to_string())
    } else {
        Ok(content.as_bytes().to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ExecutionContext, NodeInput};
    use crate::node::Node;
    use serde_json::{json, Value};
    use std::collections::HashMap;

    /// A symlink inside the sandbox whose target resolves outside must be rejected.
    /// The canonicalize-based check makes this detectable.
    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_inside_sandbox_is_rejected() {
        let sandbox_dir = tempfile::tempdir().unwrap();
        let outside_dir = tempfile::tempdir().unwrap();

        // Target file lives outside the sandbox.
        let target = outside_dir.path().join("secret.txt");
        std::fs::write(&target, "secret data").unwrap();

        // Symlink lives inside the sandbox but points outside.
        let link = sandbox_dir.path().join("escape");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let mut metadata: HashMap<String, Value> = HashMap::new();
        metadata.insert(
            "__file_sandbox_dir".to_string(),
            Value::String(sandbox_dir.path().to_str().unwrap().to_string()),
        );

        let input = NodeInput {
            resolved_credentials: std::collections::HashMap::new(),
            cancel_token: None,
            node_id:      "test-node".to_string(),
            workflow_id:  "test-wf".to_string(),
            execution_id: "test-exec".to_string(),
            input: json!({ "operation": "read", "path": "escape" }),
            context: ExecutionContext {
                variables:    HashMap::new(),
                node_outputs: std::sync::Arc::new(HashMap::new()),
                metadata,
                ..Default::default()
            },
        };

        let result = FileNode.execute(input).await;
        assert!(!result.success, "Expected failure for symlink escaping sandbox");
        let err = result.error.expect("Expected NodeError");
        assert_eq!(
            err.code, "PATH_OUTSIDE_SANDBOX",
            "Expected PATH_OUTSIDE_SANDBOX, got: {}",
            err.code
        );
    }

    fn no_sandbox_input(op_json: Value) -> NodeInput {
        NodeInput {
            resolved_credentials: std::collections::HashMap::new(),
            cancel_token: None,
            node_id:      "test-node".to_string(),
            workflow_id:  "test-wf".to_string(),
            execution_id: "test-exec".to_string(),
            input: op_json,
            context: ExecutionContext {
                variables:    HashMap::new(),
                node_outputs: std::sync::Arc::new(HashMap::new()),
                metadata:     HashMap::new(),
                ..Default::default()
            },
        }
    }

    // Normal case: base64-encoded, non-UTF8 binary content must round-trip
    // through "write" as decoded bytes, not literal base64 text.
    #[tokio::test]
    async fn write_base64_encoding_decodes_before_write() {
        use base64::Engine;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.bin");
        let raw_bytes: &[u8] = &[0x00, 0xFF, 0x10, 0xAB, 0xCD, 0xEF];
        let encoded = base64::engine::general_purpose::STANDARD.encode(raw_bytes);

        let input = no_sandbox_input(json!({
            "operation": "write",
            "path": path.to_str().unwrap(),
            "content": encoded,
            "encoding": "base64"
        }));

        let result = FileNode.execute(input).await;
        assert!(result.success, "write should succeed: {:?}", result.error);
        assert_eq!(result.output.as_ref().unwrap()["bytes"], raw_bytes.len());

        let on_disk = std::fs::read(&path).unwrap();
        assert_eq!(
            on_disk, raw_bytes,
            "file must contain decoded binary bytes, not literal base64 text"
        );
    }

    // Normal case: base64-encoded content must be decoded on "append" too.
    #[tokio::test]
    async fn append_base64_encoding_decodes_before_append() {
        use base64::Engine;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.bin");
        std::fs::write(&path, [0x01u8, 0x02]).unwrap();

        let more: &[u8] = &[0xFE, 0xFF];
        let encoded = base64::engine::general_purpose::STANDARD.encode(more);
        let input = no_sandbox_input(json!({
            "operation": "append",
            "path": path.to_str().unwrap(),
            "content": encoded,
            "encoding": "base64"
        }));

        let result = FileNode.execute(input).await;
        assert!(result.success, "append should succeed: {:?}", result.error);

        let on_disk = std::fs::read(&path).unwrap();
        assert_eq!(on_disk, vec![0x01, 0x02, 0xFE, 0xFF]);
    }

    // Edge case: malformed base64 content must fail cleanly, not silently
    // write garbage or panic.
    #[tokio::test]
    async fn write_invalid_base64_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.bin");

        let input = no_sandbox_input(json!({
            "operation": "write",
            "path": path.to_str().unwrap(),
            "content": "not-valid-base64!!!",
            "encoding": "base64"
        }));

        let result = FileNode.execute(input).await;
        assert!(!result.success);
        assert_eq!(result.error.unwrap().code, "INVALID_BASE64");
        assert!(!path.exists(), "no file should be written on decode failure");
    }

    // Normal case: sandboxed write to a relative path whose parent directories
    // don't exist yet must create them, matching desktop/no-sandbox behaviour,
    // instead of failing with INVALID_PATH.
    #[tokio::test]
    async fn sandboxed_write_creates_missing_nested_parent_dirs() {
        let sandbox_dir = tempfile::tempdir().unwrap();

        let mut metadata: HashMap<String, Value> = HashMap::new();
        metadata.insert(
            "__file_sandbox_dir".to_string(),
            Value::String(sandbox_dir.path().to_str().unwrap().to_string()),
        );

        let input = NodeInput {
            resolved_credentials: std::collections::HashMap::new(),
            cancel_token: None,
            node_id:      "test-node".to_string(),
            workflow_id:  "test-wf".to_string(),
            execution_id: "test-exec".to_string(),
            input: json!({
                "operation": "write",
                "path": "new/nested/file.txt",
                "content": "hello"
            }),
            context: ExecutionContext {
                variables:    HashMap::new(),
                node_outputs: std::sync::Arc::new(HashMap::new()),
                metadata,
                ..Default::default()
            },
        };

        let result = FileNode.execute(input).await;
        assert!(result.success, "expected success, got: {:?}", result.error);
        let on_disk = sandbox_dir.path().join("new/nested/file.txt");
        assert_eq!(std::fs::read_to_string(&on_disk).unwrap(), "hello");
    }

    // Edge case: a symlink inside the sandbox pointing outside it, used as an
    // *intermediate* directory (the final path component under it does NOT
    // exist), must be rejected by "exists" rather than falling back to a
    // lexical starts_with check that trusts the unresolved suffix.
    #[cfg(unix)]
    #[tokio::test]
    async fn sandboxed_exists_symlinked_intermediate_dir_is_rejected() {
        let sandbox_dir = tempfile::tempdir().unwrap();
        let outside_dir = tempfile::tempdir().unwrap();

        let link = sandbox_dir.path().join("escapelink");
        std::os::unix::fs::symlink(outside_dir.path(), &link).unwrap();

        let mut metadata: HashMap<String, Value> = HashMap::new();
        metadata.insert(
            "__file_sandbox_dir".to_string(),
            Value::String(sandbox_dir.path().to_str().unwrap().to_string()),
        );

        let input = NodeInput {
            resolved_credentials: std::collections::HashMap::new(),
            cancel_token: None,
            node_id:      "test-node".to_string(),
            workflow_id:  "test-wf".to_string(),
            execution_id: "test-exec".to_string(),
            // "nonexistent.txt" doesn't exist under the symlinked dir, so a full
            // canonicalize(abs) fails; the check must not fall back to a lexical
            // starts_with(sandbox) check that trusts the unresolved suffix.
            input: json!({ "operation": "exists", "path": "escapelink/nonexistent.txt" }),
            context: ExecutionContext {
                variables:    HashMap::new(),
                node_outputs: std::sync::Arc::new(HashMap::new()),
                metadata,
                ..Default::default()
            },
        };

        let result = FileNode.execute(input).await;
        assert!(!result.success, "expected rejection, got success: {:?}", result.output);
        assert_eq!(result.error.unwrap().code, "PATH_OUTSIDE_SANDBOX");
    }

    // Normal case, guards against over-rejection: a genuinely missing file
    // inside a real (non-symlinked) sandboxed directory must still return
    // exists:false, not an error.
    #[tokio::test]
    async fn sandboxed_exists_missing_file_in_real_dir_returns_false() {
        let sandbox_dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(sandbox_dir.path().join("realdir")).unwrap();

        let mut metadata: HashMap<String, Value> = HashMap::new();
        metadata.insert(
            "__file_sandbox_dir".to_string(),
            Value::String(sandbox_dir.path().to_str().unwrap().to_string()),
        );

        let input = NodeInput {
            resolved_credentials: std::collections::HashMap::new(),
            cancel_token: None,
            node_id:      "test-node".to_string(),
            workflow_id:  "test-wf".to_string(),
            execution_id: "test-exec".to_string(),
            input: json!({ "operation": "exists", "path": "realdir/missing.txt" }),
            context: ExecutionContext {
                variables:    HashMap::new(),
                node_outputs: std::sync::Arc::new(HashMap::new()),
                metadata,
                ..Default::default()
            },
        };

        let result = FileNode.execute(input).await;
        assert!(result.success, "expected success, got: {:?}", result.error);
        assert_eq!(result.output.as_ref().unwrap()["exists"], false);
    }
}

