use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::fs_sandbox;
use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::Node;

const MAX_READ_BYTES: u64 = 50 * 1024 * 1024;

pub struct FileNode;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Utf8,
    Base64,
}

fn parse_encoding(v: &Value) -> Result<Encoding, NodeOutput> {
    match v {
        Value::Null => Ok(Encoding::Utf8),
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "" | "utf8" => Ok(Encoding::Utf8),
            "base64" => Ok(Encoding::Base64),
            other => Err(NodeOutput::failure(NodeError::unrecoverable(
                "INVALID_ENCODING",
                format!("encoding must be 'utf8' or 'base64', got '{other}'"),
            ))),
        },
        _ => Err(NodeOutput::failure(NodeError::unrecoverable(
            "INVALID_ENCODING",
            "encoding must be 'utf8' or 'base64'",
        ))),
    }
}

/// Bytes to write for `write` / `append`. An absent or null `content` is an
/// error: treating it as empty would silently truncate the target on `write`.
fn content_bytes(content: &Value, encoding: Encoding) -> Result<Vec<u8>, NodeOutput> {
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => {
            return Err(NodeOutput::failure(NodeError::unrecoverable(
                "MISSING_CONTENT",
                "content is required for write and append (use an empty string for an empty file)",
            )))
        }
        _ => {
            return Err(NodeOutput::failure(NodeError::unrecoverable(
                "INVALID_CONTENT",
                "content must be text; convert objects and arrays to a string first",
            )))
        }
    };
    decode_write_content(&text, encoding).map_err(|e| {
        NodeOutput::failure(NodeError::unrecoverable(
            "INVALID_BASE64",
            format!("content is not valid base64: {e}"),
        ))
    })
}

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
                "content":   { "type": "string", "description": "Content to write (write/append only; required, use an empty string for an empty file)" },
                "encoding":  { "type": "string", "enum": ["utf8", "base64"], "description": "Default: utf8. Reading a file that is not valid UTF-8 as utf8 fails; use base64 for binary files." }
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

        if Path::new(&raw_path)
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return NodeOutput::failure(NodeError::unrecoverable(
                "INVALID_PATH",
                "Path traversal sequences (..) are not permitted",
            ));
        }

        let sandbox = match fs_sandbox::root_from_metadata(&input.context.metadata) {
            Ok(root) => root,
            Err(e) => return e.into_output("path", None),
        };
        let path: PathBuf = match &sandbox {
            None => PathBuf::from(&raw_path),
            Some(root) if operation == "delete" => match resolve_entry(root, &raw_path) {
                Ok(p) => p,
                Err(failure) => return failure,
            },
            Some(root) => match fs_sandbox::resolve(root, Path::new(&raw_path)) {
                Ok(p) => p,
                Err(e) => return e.into_output("path", Some(root.as_path())),
            },
        };
        let shown = path.to_string_lossy().into_owned();

        match operation.as_str() {
            "read" => {
                let encoding = match parse_encoding(&input.input["encoding"]) {
                    Ok(e) => e,
                    Err(failure) => return failure,
                };
                let bytes = match read_capped(&path).await {
                    Ok(b) => b,
                    Err(failure) => return failure,
                };
                let byte_len = bytes.len();
                let content = match encoding {
                    Encoding::Base64 => {
                        use base64::Engine;
                        base64::engine::general_purpose::STANDARD.encode(&bytes)
                    }
                    Encoding::Utf8 => match String::from_utf8(bytes) {
                        Ok(text) => text,
                        Err(_) => return NodeOutput::failure(NodeError::unrecoverable(
                            "NOT_UTF8",
                            "File is not valid UTF-8; set encoding to base64 to read binary content",
                        )),
                    },
                };
                NodeOutput::success(json!({ "content": content, "bytes": byte_len, "path": shown }))
            }
            "write" => {
                let encoding = match parse_encoding(&input.input["encoding"]) {
                    Ok(e) => e,
                    Err(failure) => return failure,
                };
                let bytes = match content_bytes(&input.input["content"], encoding) {
                    Ok(b) => b,
                    Err(failure) => return failure,
                };
                if let Err(failure) = ensure_parent_dir(&path, "WRITE_ERR").await {
                    return failure;
                }
                match fs::write(&path, &bytes).await {
                    Err(e) => NodeOutput::failure(NodeError::unrecoverable("WRITE_ERR", e.to_string())),
                    Ok(_)  => NodeOutput::success(json!({ "path": shown, "bytes": bytes.len() })),
                }
            }
            "append" => {
                let encoding = match parse_encoding(&input.input["encoding"]) {
                    Ok(e) => e,
                    Err(failure) => return failure,
                };
                let bytes = match content_bytes(&input.input["content"], encoding) {
                    Ok(b) => b,
                    Err(failure) => return failure,
                };
                if let Err(failure) = ensure_parent_dir(&path, "APPEND_ERR").await {
                    return failure;
                }
                let mut file = match fs::OpenOptions::new().create(true).append(true).open(&path).await {
                    Ok(f) => f,
                    Err(e) => return NodeOutput::failure(NodeError::unrecoverable("APPEND_ERR", e.to_string())),
                };
                // tokio's File can return from write_all before the bytes reach
                // the OS, so the explicit flush is what guarantees delivery.
                if let Err(e) = file.write_all(&bytes).await {
                    return NodeOutput::failure(NodeError::unrecoverable("APPEND_ERR", e.to_string()));
                }
                if let Err(e) = file.flush().await {
                    return NodeOutput::failure(NodeError::unrecoverable("APPEND_ERR", e.to_string()));
                }
                NodeOutput::success(json!({ "path": shown, "bytes": bytes.len() }))
            }
            "delete" => match fs::remove_file(&path).await {
                Err(e) => NodeOutput::failure(NodeError::unrecoverable("DELETE_ERR", e.to_string())),
                Ok(_)  => NodeOutput::success(json!({ "path": shown, "deleted": true })),
            },
            "exists" => {
                let exists = fs::try_exists(&path).await.unwrap_or(false);
                NodeOutput::success(json!({ "exists": exists, "path": shown }))
            }
            _ => NodeOutput::failure(NodeError::unrecoverable("INVALID_OP", format!("Unknown operation: {}", operation))),
        }
    }
}

/// Reads a regular file of at most `MAX_READ_BYTES`. The stat happens before the
/// open so a FIFO or device node is refused instead of blocking or streaming
/// forever, and the read itself is bounded in case the file grows meanwhile.
async fn read_capped(path: &Path) -> Result<Vec<u8>, NodeOutput> {
    let fail = |code: &str, msg: String| NodeOutput::failure(NodeError::unrecoverable(code, msg));
    let meta = fs::metadata(path).await.map_err(|e| fail("READ_ERR", e.to_string()))?;
    if !meta.is_file() {
        return Err(fail("NOT_A_FILE", "path is not a regular file".to_string()));
    }
    let too_large = |size: String| {
        fail(
            "FILE_TOO_LARGE",
            format!(
                "File exceeds {}MB read limit ({} bytes). Use a streaming approach for large files.",
                MAX_READ_BYTES / (1024 * 1024),
                size
            ),
        )
    };
    if meta.len() > MAX_READ_BYTES {
        return Err(too_large(meta.len().to_string()));
    }
    let file = fs::File::open(path).await.map_err(|e| fail("READ_ERR", e.to_string()))?;
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(MAX_READ_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|e| fail("READ_ERR", e.to_string()))?;
    if bytes.len() as u64 > MAX_READ_BYTES {
        return Err(too_large(format!("more than {MAX_READ_BYTES}")));
    }
    Ok(bytes)
}

async fn ensure_parent_dir(path: &Path, code: &str) -> Result<(), NodeOutput> {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => fs::create_dir_all(parent).await.map_err(|e| {
            NodeOutput::failure(NodeError::unrecoverable(
                code,
                format!("Cannot create directory '{}': {}", parent.display(), e),
            ))
        }),
        _ => Ok(()),
    }
}

/// Sandboxed delete targets the directory entry itself: the parent is resolved
/// and the final name re-attached, so deleting a symlink removes the link and
/// never the file it points to.
fn resolve_entry(root: &Path, raw_path: &str) -> Result<PathBuf, NodeOutput> {
    let requested = Path::new(raw_path);
    let Some(name) = requested.file_name() else {
        return Err(NodeOutput::failure(NodeError::unrecoverable(
            "INVALID_PATH",
            "path must name a file",
        )));
    };
    let parent = requested.parent().unwrap_or_else(|| Path::new(""));
    fs_sandbox::resolve(root, parent)
        .map(|p| p.join(name))
        .map_err(|e| e.into_output("path", Some(root)))
}

/// Decode `content` per `encoding`: base64, or utf8 passthrough.
fn decode_write_content(content: &str, encoding: Encoding) -> Result<Vec<u8>, String> {
    match encoding {
        Encoding::Base64 => {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .decode(content)
                .map_err(|e| e.to_string())
        }
        Encoding::Utf8 => Ok(content.as_bytes().to_vec()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExecutionContext;
    use std::collections::HashMap;

    fn input_with(cfg: Value, sandbox: Option<&Path>) -> NodeInput {
        let mut metadata: HashMap<String, Value> = HashMap::new();
        if let Some(root) = sandbox {
            metadata.insert(
                fs_sandbox::SANDBOX_KEY.to_string(),
                Value::String(root.to_str().unwrap().to_string()),
            );
        }
        NodeInput {
            resolved_credentials: HashMap::new(),
            cancel_token: None,
            node_id:      "test-node".to_string(),
            workflow_id:  "test-wf".to_string(),
            execution_id: "test-exec".to_string(),
            input: cfg,
            context: ExecutionContext {
                variables:    HashMap::new(),
                node_outputs: std::sync::Arc::new(HashMap::new()),
                metadata,
                ..Default::default()
            },
        }
    }

    async fn run(cfg: Value) -> NodeOutput {
        FileNode.execute(input_with(cfg, None)).await
    }

    async fn run_sandboxed(root: &Path, cfg: Value) -> NodeOutput {
        FileNode.execute(input_with(cfg, Some(root))).await
    }

    fn code(out: NodeOutput) -> String {
        out.error.expect("expected a failure").code
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn sandbox_refuses_symlinks_that_resolve_outside() {
        let sandbox = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "secret data").unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret.txt"), sandbox.path().join("file_link")).unwrap();
        std::os::unix::fs::symlink(outside.path(), sandbox.path().join("dir_link")).unwrap();

        let read = run_sandboxed(sandbox.path(), json!({ "operation": "read", "path": "file_link" })).await;
        assert_eq!(code(read), "PATH_OUTSIDE_SANDBOX");

        let exists = run_sandboxed(sandbox.path(), json!({ "operation": "exists", "path": "dir_link/nonexistent.txt" })).await;
        assert_eq!(code(exists), "PATH_OUTSIDE_SANDBOX");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn sandbox_refuses_write_through_a_dangling_symlink() {
        let sandbox = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().join("created_by_attacker.txt");
        std::os::unix::fs::symlink(&victim, sandbox.path().join("trap")).unwrap();

        for op in ["write", "append"] {
            let out = run_sandboxed(
                sandbox.path(),
                json!({ "operation": op, "path": "trap", "content": "pwned" }),
            )
            .await;
            assert_eq!(code(out), "INVALID_PATH", "{op} must be refused");
        }
        assert!(!victim.exists(), "nothing may be created outside the sandbox");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn sandboxed_delete_removes_the_link_not_its_target() {
        let sandbox = tempfile::tempdir().unwrap();
        let keep = sandbox.path().join("keep.txt");
        std::fs::write(&keep, "data").unwrap();
        let link = sandbox.path().join("alias");
        std::os::unix::fs::symlink(&keep, &link).unwrap();

        let out = run_sandboxed(sandbox.path(), json!({ "operation": "delete", "path": "alias" })).await;
        assert!(out.success, "{:?}", out.error);
        assert!(keep.exists(), "the target must survive");
        assert!(std::fs::symlink_metadata(&link).is_err(), "the link must be gone");
    }

    #[tokio::test]
    async fn sandboxed_write_creates_missing_nested_parent_dirs() {
        let sandbox = tempfile::tempdir().unwrap();
        let out = run_sandboxed(
            sandbox.path(),
            json!({ "operation": "write", "path": "new/nested/file.txt", "content": "hello" }),
        )
        .await;
        assert!(out.success, "{:?}", out.error);
        assert_eq!(std::fs::read_to_string(sandbox.path().join("new/nested/file.txt")).unwrap(), "hello");
    }

    #[tokio::test]
    async fn sandboxed_exists_missing_file_in_real_dir_returns_false() {
        let sandbox = tempfile::tempdir().unwrap();
        std::fs::create_dir(sandbox.path().join("realdir")).unwrap();
        let out = run_sandboxed(sandbox.path(), json!({ "operation": "exists", "path": "realdir/missing.txt" })).await;
        assert!(out.success, "{:?}", out.error);
        assert_eq!(out.output.unwrap()["exists"], false);
    }

    #[tokio::test]
    async fn unusable_sandbox_root_fails_closed() {
        let out = FileNode
            .execute(input_with(
                json!({ "operation": "exists", "path": "x" }),
                Some(Path::new("/no/such/sandbox/root")),
            ))
            .await;
        assert_eq!(code(out), "INVALID_PATH");
    }

    #[tokio::test]
    async fn write_and_append_decode_base64_and_reject_malformed_input() {
        use base64::Engine;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.bin");
        let encode = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);

        let w = run(json!({
            "operation": "write", "path": path.to_str().unwrap(),
            "content": encode(&[0x00, 0xFF, 0x10]), "encoding": "base64"
        })).await;
        assert!(w.success, "{:?}", w.error);
        assert_eq!(w.output.unwrap()["bytes"], 3);

        let a = run(json!({
            "operation": "append", "path": path.to_str().unwrap(),
            "content": encode(&[0xFE, 0xAB]), "encoding": "base64"
        })).await;
        assert!(a.success, "{:?}", a.error);
        assert_eq!(std::fs::read(&path).unwrap(), vec![0x00, 0xFF, 0x10, 0xFE, 0xAB]);

        let bad = run(json!({
            "operation": "write", "path": path.to_str().unwrap(),
            "content": "not-valid-base64!!!", "encoding": "base64"
        })).await;
        assert_eq!(code(bad), "INVALID_BASE64");
        assert_eq!(std::fs::read(&path).unwrap().len(), 5, "a failed decode must not touch the file");
    }

    #[tokio::test]
    async fn write_without_content_is_refused_and_leaves_the_file_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keep.txt");
        std::fs::write(&path, "precious").unwrap();

        for cfg in [
            json!({ "operation": "write", "path": path.to_str().unwrap() }),
            json!({ "operation": "write", "path": path.to_str().unwrap(), "content": null }),
        ] {
            assert_eq!(code(run(cfg).await), "MISSING_CONTENT");
        }
        let object = run(json!({ "operation": "append", "path": path.to_str().unwrap(), "content": { "a": 1 } })).await;
        assert_eq!(code(object), "INVALID_CONTENT");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "precious");

        let empty = run(json!({ "operation": "write", "path": path.to_str().unwrap(), "content": "" })).await;
        assert!(empty.success, "an explicit empty string is a valid empty file");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");

        let number = run(json!({ "operation": "write", "path": path.to_str().unwrap(), "content": 42 })).await;
        assert!(number.success, "{:?}", number.error);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "42");
    }

    #[tokio::test]
    async fn read_reports_binary_files_instead_of_corrupting_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blob.bin");
        std::fs::write(&path, [0xFFu8, 0xFE, 0x00, 0x41]).unwrap();
        let p = path.to_str().unwrap();

        assert_eq!(code(run(json!({ "operation": "read", "path": p })).await), "NOT_UTF8");

        let b64 = run(json!({ "operation": "read", "path": p, "encoding": "base64" })).await;
        let out = b64.output.expect("base64 read must succeed");
        assert_eq!(out["content"], "//4AQQ==");
        assert_eq!(out["bytes"], 4);

        assert_eq!(code(run(json!({ "operation": "read", "path": p, "encoding": "hex" })).await), "INVALID_ENCODING");
    }

    #[tokio::test]
    async fn read_refuses_directories_and_oversized_files() {
        let dir = tempfile::tempdir().unwrap();
        let out = run(json!({ "operation": "read", "path": dir.path().to_str().unwrap() })).await;
        assert_eq!(code(out), "NOT_A_FILE");

        let big = dir.path().join("big.bin");
        std::fs::File::create(&big).unwrap().set_len(MAX_READ_BYTES + 1).unwrap();
        let out = run(json!({ "operation": "read", "path": big.to_str().unwrap() })).await;
        assert_eq!(code(out), "FILE_TOO_LARGE");
    }

    #[tokio::test]
    async fn dots_inside_a_filename_are_allowed_but_parent_components_are_not() {
        let dir = tempfile::tempdir().unwrap();
        let ok_path = dir.path().join("report..final.txt");
        let ok = run(json!({ "operation": "write", "path": ok_path.to_str().unwrap(), "content": "x" })).await;
        assert!(ok.success, "a filename containing '..' is not traversal: {:?}", ok.error);

        let bad_path = format!("{}/sub/../escape.txt", dir.path().to_str().unwrap());
        let bad = run(json!({ "operation": "write", "path": bad_path, "content": "x" })).await;
        assert_eq!(code(bad), "INVALID_PATH");
    }

    #[tokio::test]
    async fn write_reports_directory_creation_failure() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, "a file, not a directory").unwrap();
        let target = blocker.join("child").join("out.txt");

        let out = run(json!({ "operation": "write", "path": target.to_str().unwrap(), "content": "x" })).await;
        let err = out.error.expect("write under a regular file must fail");
        assert_eq!(err.code, "WRITE_ERR");
        assert!(err.message.contains("Cannot create directory"), "got: {}", err.message);
    }
}
