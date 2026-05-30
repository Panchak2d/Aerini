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
                    "write" | "append" => {
                        // The target file may not exist yet. Canonicalize its parent directory
                        // instead, then rejoin the filename. If the parent does not exist on
                        // disk, INVALID_PATH is returned — create_dir_all only runs after this
                        // check passes, never before.
                        let fname = match abs.file_name() {
                            Some(f) => f.to_owned(),
                            None => return NodeOutput::failure(NodeError::unrecoverable(
                                "INVALID_PATH", "Path has no filename component",
                            )),
                        };
                        let parent = abs.parent().unwrap_or(sandbox.as_path());
                        match std::fs::canonicalize(parent) {
                            Ok(cp) if cp.starts_with(&sandbox) => {
                                cp.join(fname).to_string_lossy().into_owned()
                            }
                            Ok(_)  => return outside(),
                            Err(_) => return NodeOutput::failure(NodeError::unrecoverable(
                                "INVALID_PATH",
                                "Path parent does not exist or cannot be resolved",
                            )),
                        }
                    }
                    "exists" => {
                        // Try full canonicalize first: if the file exists, this dereferences
                        // all symlinks and we can check containment precisely.
                        // If the file does not exist, canonicalize returns Err (nothing to
                        // dereference). In that case fall back to starts_with on abs: safe
                        // because ".." is already rejected above, and if the final target
                        // existed a symlink would have made canonicalize succeed.
                        match std::fs::canonicalize(&abs) {
                            Ok(cp) if cp.starts_with(&sandbox) => {
                                cp.to_string_lossy().into_owned()
                            }
                            Ok(_)  => return outside(),
                            Err(_) => {
                                if !abs.starts_with(&sandbox) {
                                    return outside();
                                }
                                abs.to_string_lossy().into_owned()
                            }
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
                if let Some(parent) = std::path::Path::new(&path).parent() {
                    let _ = fs::create_dir_all(parent).await;
                }
                match fs::write(&path, content).await {
                    Err(e) => NodeOutput::failure(NodeError::unrecoverable("WRITE_ERR", e.to_string())),
                    Ok(_)  => NodeOutput::success(json!({ "path": path, "bytes": content.len() })),
                }
            }
            "append" => {
                use tokio::io::AsyncWriteExt;
                let content = input.input["content"].as_str().unwrap_or("");
                match tokio::fs::OpenOptions::new().create(true).append(true).open(&path).await {
                    Err(e) => NodeOutput::failure(NodeError::unrecoverable("APPEND_ERR", e.to_string())),
                    Ok(mut f) => match f.write_all(content.as_bytes()).await {
                        Err(e) => NodeOutput::failure(NodeError::unrecoverable("APPEND_ERR", e.to_string())),
                        Ok(_)  => NodeOutput::success(json!({ "path": path, "bytes": content.len() })),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ExecutionContext, NodeInput};
    use crate::node::Node;
    use serde_json::{json, Value};
    use std::collections::HashMap;

    /// A symlink inside the sandbox whose target resolves outside must be rejected.
    /// The canonicalize-based check in Patch 1 makes this detectable.
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
            node_id:      "test-node".to_string(),
            workflow_id:  "test-wf".to_string(),
            execution_id: "test-exec".to_string(),
            input: json!({ "operation": "read", "path": "escape" }),
            context: ExecutionContext {
                variables:    HashMap::new(),
                node_outputs: std::sync::Arc::new(HashMap::new()),
                metadata,
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
}
