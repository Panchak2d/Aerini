// SaveToFolder Node
//
// Writes media contract files to disk. Two modes:
//
//   Subfolder mode (config.subfolders non-empty):
//     One dynamic input port per subfolder. Each port's source_expr is resolved
//     by the executor to a JSON string containing a media contract or files array.
//     Files are written to folder_path/<subfolder.name>/.
//
//   Flat mode (no subfolders):
//     Single static input port "input". config["files"] holds the files array
//     (resolved from a user-specified expression) and written directly to folder_path.
//
// Dynamic ports: YES — implements is_dynamic_ports() + ports_from_config().
//
// Names: path separators, NUL and trailing dots/spaces are stripped from every
// filename and subfolder name before any path is built, and names that collide
// within one run get a numeric suffix instead of overwriting each other.
// folder_path itself is user-selected and not sanitized; under a server file
// sandbox it, and every subfolder, must resolve inside the sandbox root.
//
// Writes: sequential, one file at a time, on the node's own async task — no
// per-file spawn. Each file lands via a same-directory temp file + rename, so
// an interrupted run leaves at most one stray temp file, never a truncated
// file at its real target path.
//
// Output: { saved, count, folder, skipped, errors }. A run that writes nothing
// and skips nothing fails with SAVE_FAILED.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::fs;
use uuid::Uuid;

use super::fs_sandbox;
use super::util::{cfg_bool_opt, decode_file_data, FilenameSet};
use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortArity, PortDefinition, PortPosition};

pub struct SaveToFolderNode;

#[async_trait]
impl Node for SaveToFolderNode {
    fn type_id(&self) -> &'static str { "save_to_folder" }
    fn display_name(&self) -> &'static str { "Save to Folder" }
    fn node_type(&self) -> NodeType { NodeType::Action }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Save one or more files to a folder on disk with optional subfolder organisation." }

    fn input_schema(&self) -> Value {
        // folder_path and overwrite are excluded from properties — the custom UI
        // in popover-config.ts renders them (folder picker + checkbox). Including
        // them here would produce duplicate generic text inputs.
        json!({
            "type": "object",
            "properties": {
                "filename_prefix": {
                    "type": "string",
                    "description": "Optional prefix prepended to every saved filename",
                    // Only catches a literal value typed directly into the field.
                    // filename_prefix resolves through the expression pipeline, so an
                    // {{expression}}-driven value isn't evaluated client-side and can
                    // still exceed this cap — the truncation in execute() below (also
                    // MAX_PREFIX_LEN) is the actual enforcement backstop.
                    "maxLength": MAX_PREFIX_LEN
                }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "saved":   { "type": "array",  "description": "Successfully saved file records" },
                "count":   { "type": "number", "description": "Number of files saved" },
                "folder":  { "type": "string", "description": "Destination folder path" },
                "skipped": { "type": "number", "description": "Files skipped (overwrite=false)" },
                "errors":  { "type": "array",  "description": "Files that failed to write" }
            }
        })
    }

    fn is_dynamic_ports(&self) -> bool { true }

    fn ports_from_config(&self, config: &Value) -> Option<NodePorts> {
        Some(derive_ports(config))
    }

    // Static ports() returns minimum-state ports for defensive correctness
    // (called only for descriptor generation, not during execution).
    fn ports(&self) -> NodePorts {
        derive_ports(&Value::Null)
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let cfg = &input.input;

        let raw_folder_path = match cfg["folder_path"].as_str() {
            Some(p) if !p.is_empty() => p.to_string(),
            _ => return NodeOutput::failure(NodeError::unrecoverable(
                "MISSING_FOLDER_PATH",
                "folder_path is required — set it using the Choose Folder button",
            )),
        };

        let sandbox = match fs_sandbox::root_from_metadata(&input.context.metadata) {
            Ok(root) => root,
            Err(e) => return e.into_output("folder_path", None),
        };
        let folder_path: String = match &sandbox {
            None => raw_folder_path,
            Some(root) => match fs_sandbox::resolve(root, Path::new(&raw_folder_path)) {
                Ok(p) => p.to_string_lossy().into_owned(),
                Err(e) => return e.into_output(&format!("folder_path '{raw_folder_path}'"), Some(root.as_path())),
            },
        };

        let overwrite = match cfg_bool_opt(&cfg["overwrite"], "overwrite") {
            Ok(v) => v.unwrap_or(true),
            Err(e) => return NodeOutput::failure(e),
        };
        let raw_prefix = cfg["filename_prefix"].as_str().unwrap_or("");
        let prefix: String = truncate_to_byte_len(&strip_separators(raw_prefix), MAX_PREFIX_LEN).to_string();

        let subfolders = cfg["subfolders"].as_array().cloned().unwrap_or_default();

        if subfolders.is_empty() {
            let direct_input = input.context.metadata.get("__direct_input");
            flat_mode(cfg, direct_input, &folder_path, &prefix, overwrite).await
        } else {
            subfolder_mode(&subfolders, &folder_path, sandbox.as_deref(), &prefix, overwrite).await
        }
    }
}

// ── Execution modes ────────────────────────────────────────────────────────────

async fn flat_mode(
    cfg: &Value,
    direct_input: Option<&Value>,
    folder_path: &str,
    prefix: &str,
    overwrite: bool,
) -> NodeOutput {
    let mut files = extract_config_files(&cfg["files"]);

    // config["files"] is only populated when the canvas wired a subfolder/source
    // slot's source_expr. The flat-mode default "input" port has no such slot
    // (see Canvas.ts finishConn), so a direct upstream connection alone leaves
    // this field empty. Fall back to the upstream node's raw output, captured
    // by the executor as __direct_input (same fallback code_node.rs uses).
    if files.is_empty() {
        if let Some(direct) = direct_input {
            files = extract_files_array(direct.clone());
        }
    }

    if files.is_empty() {
        return NodeOutput::failure(NodeError::unrecoverable(
            "NO_FILES_RECEIVED",
            "Save to Folder received no files. Connect a Text to File or other file-producing node to its input first.",
        ));
    }

    let base = PathBuf::from(folder_path);
    if let Err(e) = fs::create_dir_all(&base).await {
        return NodeOutput::failure(NodeError::unrecoverable(
            "CREATE_DIR_FAILED",
            format!("Could not create '{}': {}", folder_path, e),
        ));
    }

    let results = write_files(&files, &base, prefix, overwrite, &mut FilenameSet::default()).await;
    finish(results, folder_path, Vec::new())
}

async fn subfolder_mode(
    subfolders: &[Value],
    folder_path: &str,
    sandbox: Option<&Path>,
    prefix: &str,
    overwrite: bool,
) -> NodeOutput {
    let mut all_results: Vec<Result<Value, Value>> = Vec::new();
    let mut logs: Vec<String> = Vec::new();
    let mut names_by_dir: HashMap<String, FilenameSet> = HashMap::new();

    for sf in subfolders {
        let sf_name = sanitize_component(sf["name"].as_str().unwrap_or(""), "unnamed");
        let source_expr  = sf["source_expr"].as_str().unwrap_or("").trim();

        if source_expr.is_empty() {
            logs.push(format!("Subfolder '{}': no source_expr — skipped", sf_name));
            continue;
        }

        let files = extract_files_array(parse_source_expr(source_expr));

        // The directory is created before the empty check so configured
        // subfolders exist on disk even when upstream produces no output.
        let joined = PathBuf::from(folder_path).join(&sf_name);
        let target_dir = match sandbox {
            Some(root) => match fs_sandbox::resolve(root, &joined) {
                Ok(p) => p,
                Err(_) => {
                    all_results.push(Err(json!({
                        "filename": format!("<{}/...>", sf_name),
                        "reason": "Subfolder resolves outside the permitted directory"
                    })));
                    continue;
                }
            },
            None => joined,
        };
        if let Err(e) = fs::create_dir_all(&target_dir).await {
            all_results.push(Err(json!({
                "filename": format!("<{}/...>", sf_name),
                "reason": format!("Could not create directory: {}", e)
            })));
            continue;
        }

        if files.is_empty() {
            logs.push(format!("Subfolder '{}': resolved to 0 files — skipped", sf_name));
            continue;
        }

        logs.push(format!("Subfolder '{}': {} file(s)", sf_name, files.len()));
        let names = names_by_dir.entry(target_dir.to_string_lossy().to_lowercase()).or_default();
        let results = write_files(&files, &target_dir, prefix, overwrite, names).await;
        all_results.extend(results);
    }

    finish(all_results, folder_path, logs)
}

// ── Port derivation ────────────────────────────────────────────────────────────

pub fn derive_ports(config: &Value) -> NodePorts {
    let mut inputs: Vec<PortDefinition> = Vec::new();

    if let Some(subfolders) = config["subfolders"].as_array() {
        if !subfolders.is_empty() {
            for sf in subfolders {
                let id    = sf["id"].as_str().unwrap_or("input").to_string();
                let label = sf["name"].as_str().unwrap_or("Subfolder").to_string();
                inputs.push(PortDefinition { id, label, position: PortPosition::Left, port_type: Some("files".to_string()), arity: PortArity::Single });
            }
        }
    }

    if inputs.is_empty() {
        inputs.push(PortDefinition {
            id:        "input".to_string(),
            label:     "In".to_string(),
            position:  PortPosition::Left,
            port_type: Some("files".to_string()),
            arity: PortArity::Single,
        });
    }

    NodePorts {
        inputs,
        outputs: vec![PortDefinition {
            id:        "output".to_string(),
            label:     "Out".to_string(),
            position:  PortPosition::Right,
            port_type: None,
            arity: PortArity::Single,
        }],
    }
}

// ── File writing ───────────────────────────────────────────────────────────────

async fn write_files(
    files: &[Value],
    dir: &Path,
    prefix: &str,
    overwrite: bool,
    names: &mut FilenameSet,
) -> Vec<Result<Value, Value>> {
    let mut results = Vec::with_capacity(files.len());
    for file in files {
        results.push(write_single_file(file, dir, prefix, overwrite, names).await);
    }
    results
}

async fn write_single_file(
    file: &Value,
    dir: &Path,
    prefix: &str,
    overwrite: bool,
    names: &mut FilenameSet,
) -> Result<Value, Value> {
    let raw_name  = file["filename"].as_str().unwrap_or("file.bin");
    let sanitized = sanitize_filename(raw_name);
    let wanted    = if prefix.is_empty() {
        sanitized
    } else {
        format!("{}{}", prefix, sanitized)
    };
    let filename = names.claim(&wanted, None);

    let path = dir.join(&filename);

    if !overwrite && fs::try_exists(&path).await.unwrap_or(false) {
        return Ok(json!({
            "filename": filename,
            "path":     path.display().to_string(),
            "bytes":    0,
            "skipped":  true
        }));
    }

    let data_str = match file["data"].as_str() {
        Some(d) => d,
        None => return Err(json!({ "filename": filename, "reason": "missing data field" })),
    };

    let bytes = match decode_file_data(data_str) {
        Ok(b)  => b,
        Err(e) => return Err(json!({ "filename": filename, "reason": e })),
    };

    let byte_count = bytes.len();
    // `fs::write` straight onto `path` truncates an existing file before the new
    // bytes land, so a failure partway through (disk full, permissions) would
    // leave it corrupt. rename() is one directory-entry swap: the visible file
    // is always the old complete one or the new complete one. The staging name
    // is fixed-length so it can never exceed the OS component limit.
    let tmp_path = dir.join(format!(".{}.tmp", Uuid::new_v4().simple()));

    if let Err(e) = fs::write(&tmp_path, &bytes).await {
        let _ = fs::remove_file(&tmp_path).await;
        return Err(json!({ "filename": filename, "reason": format!("write failed: {}", e) }));
    }
    if let Err(e) = fs::rename(&tmp_path, &path).await {
        let _ = fs::remove_file(&tmp_path).await;
        return Err(json!({ "filename": filename, "reason": format!("write failed: {}", e) }));
    }

    Ok(json!({
        "filename": filename,
        "path":     path.display().to_string(),
        "bytes":    byte_count
    }))
}

fn finish(results: Vec<Result<Value, Value>>, folder: &str, logs: Vec<String>) -> NodeOutput {
    let (ok, errors): (Vec<_>, Vec<_>) = results.into_iter().partition(|r| r.is_ok());
    let ok: Vec<Value>         = ok.into_iter().filter_map(|r| r.ok()).collect();
    let mut errors: Vec<Value> = errors.into_iter().filter_map(|r| r.err()).collect();
    let (skipped, saved): (Vec<Value>, Vec<Value>) =
        ok.into_iter().partition(|v| v["skipped"].as_bool().unwrap_or(false));
    let count         = saved.len();
    let skipped_count = skipped.len();

    // Nothing written and nothing deliberately skipped must not look like a success.
    if count == 0 && skipped_count == 0 {
        if errors.is_empty() {
            errors.push(json!({
                "filename": "<no input>",
                "reason": "No files were saved — upstream produced no file output"
            }));
        }
        let first = &errors[0];
        return NodeOutput::failure_with_logs(
            NodeError::unrecoverable(
                "SAVE_FAILED",
                format!(
                    "Save to Folder: no files were written ({} error(s)). First: {} — {}",
                    errors.len(),
                    first["filename"].as_str().unwrap_or("?"),
                    first["reason"].as_str().unwrap_or("unknown"),
                ),
            ),
            logs,
        );
    }

    NodeOutput::success_with_logs(
        json!({ "saved": saved, "count": count, "folder": folder, "skipped": skipped_count, "errors": errors }),
        logs,
    )
}

// ── Utilities ─────────────────────────────────────────────────────────────────

// Longest final filename is prefix (MAX_PREFIX_LEN) + sanitized name
// (MAX_SANITIZED_LEN) = 217 bytes; a uniqueness suffix adds a few more and
// stays well under the 255-byte component limit of NTFS/ext4/APFS.
const MAX_SANITIZED_LEN: usize = 167;

// filename_prefix resolves through the same expression pipeline as any other
// config string field, so it isn't guaranteed short — cap it independently
// of MAX_SANITIZED_LEN.
const MAX_PREFIX_LEN: usize = 50;

/// Removes NUL and both path separators.
fn strip_separators(name: &str) -> String {
    name.chars().filter(|&c| c != '\0' && c != '/' && c != '\\').collect()
}

fn sanitize_filename(name: &str) -> String {
    sanitize_component(name, "file.bin")
}

/// Turns arbitrary text into one safe path component: separators and NUL are
/// removed, trailing dots and whitespace are dropped (Windows silently strips
/// them, so they would otherwise make the reported path wrong), Windows
/// reserved device names are replaced, and the length is capped while keeping
/// the extension. A name that ends up empty becomes `fallback`.
fn sanitize_component(name: &str, fallback: &str) -> String {
    // On Windows, CreateFile("NUL") silently discards all written data and
    // CreateFile("CON") writes to the console. All 22 names are blocked
    // regardless of extension or case, so "NUL.txt" and "nul" are both rejected.
    const WINDOWS_RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL",
        "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
        "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let is_reserved = |s: &str| {
        let stem = s.split('.').next().unwrap_or("").to_uppercase();
        WINDOWS_RESERVED.contains(&stem.as_str())
    };

    let stripped = strip_separators(name);
    let cleaned = stripped.trim_end_matches(|c: char| c == '.' || c.is_whitespace());

    if cleaned.is_empty() || is_reserved(cleaned) {
        return fallback.to_string();
    }

    let capped = cap_sanitized_length(cleaned, MAX_SANITIZED_LEN);
    // A truncated stem could coincidentally land on a reserved name if the
    // untruncated original started with one followed by more text (e.g.
    // "CON-notes-from-a-very-long-title...").
    if is_reserved(&capped) {
        return fallback.to_string();
    }

    capped
}

/// Cap `name` to `max_bytes` bytes, preserving the extension (text from the
/// last '.' onward) by truncating the stem first. Falls back to a flat
/// truncation of the whole name when the extension alone is at or over the
/// byte budget.
fn cap_sanitized_length(name: &str, max_bytes: usize) -> String {
    if name.len() <= max_bytes {
        return name.to_string();
    }
    if let Some(dot) = name.rfind('.') {
        let ext = &name[dot..];
        if dot > 0 && ext.len() <= max_bytes {
            let stem = truncate_to_byte_len(&name[..dot], max_bytes - ext.len());
            return format!("{}{}", stem, ext);
        }
    }
    truncate_to_byte_len(name, max_bytes).to_string()
}

/// Truncate `s` to at most `max_bytes` bytes without splitting a UTF-8
/// character, stepping back to the nearest char boundary.
fn truncate_to_byte_len(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Parse a resolved source_expr string into a Value.
/// After executor expression resolution, source_expr is a JSON string whose
/// content is either a media contract object or a files array.
fn parse_source_expr(expr: &str) -> Value {
    let trimmed = expr.trim();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        serde_json::from_str(trimmed).unwrap_or(Value::Null)
    } else {
        Value::Null
    }
}

/// Extract a files array from a resolved media contract or bare files array.
///   { "files": [...], ... }  →  the files array
///   [...]                    →  the array itself
fn extract_files_array(val: Value) -> Vec<Value> {
    match val {
        Value::Object(mut obj) => match obj.remove("files") {
            Some(Value::Array(files)) => files,
            _ => vec![],
        },
        Value::Array(arr) => arr,
        _ => vec![],
    }
}

/// Extract files from config["files"].
/// Handles:
///   - Value::Array: direct files array (rare; usually config holds strings)
///   - Value::String: resolved expression JSON string (common path)
///   - Everything else: empty
fn extract_config_files(val: &Value) -> Vec<Value> {
    match val {
        Value::Array(arr) => arr.clone(),
        Value::String(s)  => extract_files_array(parse_source_expr(s)),
        _                 => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExecutionContext;
    use serde_json::json;
    use std::collections::HashMap;
    use tempfile::TempDir;

    // "Hello World" → SGVsbG8gV29ybGQ=
    // "AAA"         → QUFB
    // "BBB"         → QkJC
    // "good"        → Z29vZA==
    // "img-data"    → aW1nLWRhdGE=

    fn file_entry(name: &str, b64: &str) -> Value {
        json!({ "filename": name, "data": b64 })
    }

    // ── sandbox containment ────────────────────────────────────

    // A relative folder_path under a sandbox must be written inside the
    // sandbox root, not relative to the process's working directory.
    #[tokio::test]
    async fn sandbox_relative_folder_path_writes_inside_sandbox() {
        let sandbox_dir = TempDir::new().unwrap();
        let canonical_sandbox = std::fs::canonicalize(sandbox_dir.path()).unwrap();

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
                "folder_path": "relative/sub",
                "files": [file_entry("hello.txt", "SGVsbG8gV29ybGQ=")]
            }),
            context: ExecutionContext {
                variables:    HashMap::new(),
                node_outputs: std::sync::Arc::new(HashMap::new()),
                metadata,
                ..Default::default()
            },
        };

        let out = SaveToFolderNode.execute(input).await;
        assert!(out.success, "expected success: {:?}", out.error);

        let expected_dir = canonical_sandbox.join("relative/sub");
        assert!(
            expected_dir.join("hello.txt").exists(),
            "file must be written inside the sandbox for a relative folder_path, \
             not at a CWD-relative location"
        );
    }

    // ── flat_mode ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn flat_mode_single_file_success() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_str().unwrap();
        let cfg = json!({
            "files": [file_entry("hello.txt", "SGVsbG8gV29ybGQ=")]
        });
        let out = flat_mode(&cfg, None, path, "", true).await;
        assert!(out.success, "expected success: {:?}", out.error);
        let o = out.output.unwrap();
        assert_eq!(o["count"], 1);
        assert_eq!(o["errors"].as_array().unwrap().len(), 0);
        assert!(dir.path().join("hello.txt").exists());
    }

    #[tokio::test]
    async fn flat_mode_multiple_files_success() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_str().unwrap();
        let cfg = json!({
            "files": [
                file_entry("a.txt", "QUFB"),
                file_entry("b.txt", "QkJC"),
            ]
        });
        let out = flat_mode(&cfg, None, path, "", true).await;
        assert!(out.success, "{:?}", out.error);
        let o = out.output.unwrap();
        assert_eq!(o["count"], 2);
        assert!(dir.path().join("a.txt").exists());
        assert!(dir.path().join("b.txt").exists());
    }

    // Empty files array must surface NO_FILES_RECEIVED, not succeed silently.
    #[tokio::test]
    async fn flat_mode_empty_files_array_returns_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_str().unwrap();
        let cfg = json!({ "files": [] });
        let out = flat_mode(&cfg, None, path, "", true).await;
        assert!(!out.success, "expected failure for zero files");
        assert_eq!(out.error.unwrap().code, "NO_FILES_RECEIVED");
    }

    // Missing target directory must be created, not error.
    #[tokio::test]
    async fn flat_mode_missing_dir_is_created_by_create_dir_all() {
        let base = TempDir::new().unwrap();
        let new_dir = base.path().join("nonexistent_sub").join("deeper");
        let path = new_dir.to_str().unwrap();
        assert!(!new_dir.exists(), "pre-condition: dir must not exist");
        let cfg = json!({
            "files": [file_entry("out.txt", "Z29vZA==")]
        });
        let out = flat_mode(&cfg, None, path, "", true).await;
        assert!(out.success, "{:?}", out.error);
        assert!(new_dir.exists(), "create_dir_all must have created the directory");
        assert!(new_dir.join("out.txt").exists());
    }

    // One file with missing data field must error; the other must still be written.
    #[tokio::test]
    async fn flat_mode_partial_failure_accumulates_errors() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_str().unwrap();
        let cfg = json!({
            "files": [
                file_entry("good.txt", "Z29vZA=="),
                json!({ "filename": "bad.txt" }),  // missing data → write_single_file returns Err
            ]
        });
        let out = flat_mode(&cfg, None, path, "", true).await;
        assert!(out.success, "a partial failure still succeeds; the errors are listed in the output");
        let o = out.output.unwrap();
        assert_eq!(o["count"], 1, "one file should succeed");
        assert_eq!(
            o["errors"].as_array().unwrap().len(),
            1,
            "one error entry expected"
        );
        assert!(dir.path().join("good.txt").exists(), "good file must be written");
        assert!(!dir.path().join("bad.txt").exists(), "bad file must not be created");
    }

    // overwrite=true must replace an existing file's on-disk content.
    #[tokio::test]
    async fn flat_mode_overwrite_true_replaces_existing_file_content() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_str().unwrap();

        let first = json!({ "files": [file_entry("out.txt", "QUFB")] }); // "AAA"
        let out1 = flat_mode(&first, None, path, "", true).await;
        assert!(out1.success, "{:?}", out1.error);

        let second = json!({ "files": [file_entry("out.txt", "QkJC")] }); // "BBB"
        let out2 = flat_mode(&second, None, path, "", true).await;
        assert!(out2.success, "{:?}", out2.error);

        let content = std::fs::read_to_string(dir.path().join("out.txt")).unwrap();
        assert_eq!(content, "BBB", "overwrite=true must replace the file's content");
    }

    // The temp-file-then-rename write must not leave its staging file behind
    // once the run completes successfully.
    #[tokio::test]
    async fn flat_mode_write_leaves_no_stray_temp_files() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_str().unwrap();
        let cfg = json!({
            "files": [
                file_entry("a.txt", "QUFB"),
                file_entry("b.txt", "QkJC"),
            ]
        });
        let out = flat_mode(&cfg, None, path, "", true).await;
        assert!(out.success, "{:?}", out.error);

        let leftover: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftover.is_empty(), "no .tmp staging files should remain: {:?}", leftover);
    }

    // ── subfolder_mode ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn subfolder_mode_success() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_str().unwrap();
        // source_expr must be a JSON string starting with '[' or '{' for parse_source_expr.
        let source_expr = serde_json::to_string(&json!([
            { "filename": "img.txt", "data": "aW1nLWRhdGE=" }
        ]))
        .unwrap();
        let subfolders = vec![json!({
            "id":          "sf1",
            "name":        "images",
            "source_expr": source_expr,
        })];
        let out = subfolder_mode(&subfolders, path, None, "", true).await;
        assert!(out.success, "{:?}", out.error);
        let o = out.output.unwrap();
        assert_eq!(o["count"], 1);
        assert!(dir.path().join("images").join("img.txt").exists());
    }

    // create_dir_all must fire before the files.is_empty() guard.
    // When source_expr resolves to an empty array, the subfolder directory must still
    // exist on disk even though no files are written.
    #[tokio::test]
    async fn subfolder_mode_dir_created_before_empty_check() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_str().unwrap();
        // "[]" is a valid non-empty source_expr that resolves to zero files.
        let source_expr = serde_json::to_string(&json!([])).unwrap();
        let subfolders = vec![json!({
            "id":          "sf1",
            "name":        "empty_sub",
            "source_expr": source_expr,
        })];
        let out = subfolder_mode(&subfolders, path, None, "", true).await;
        // NodeOutput may be failure (SAVE_FAILED: no files written) — that is expected.
        // The critical assertion is that the directory was created before the guard fired.
        let subdir = dir.path().join("empty_sub");
        assert!(
            subdir.exists(),
            "directory must be created even when source resolves to 0 files"
        );
        // Confirm the output surfaces the zero-file condition rather than silently succeeding.
        assert!(
            !out.success || out.output.as_ref().map(|o| o["count"] == 0).unwrap_or(false),
            "zero files must not produce a silent success with count > 0"
        );
    }

    // ── sanitize_filename length cap ────────────────────────────────────────

    #[test]
    fn sanitize_filename_short_name_unaffected() {
        assert_eq!(sanitize_filename("report.pdf"), "report.pdf");
    }

    #[test]
    fn sanitize_filename_long_name_truncates_preserving_extension() {
        let name = format!("{}.jpg", "a".repeat(300));
        let result = sanitize_filename(&name);
        assert_eq!(
            result,
            format!("{}.jpg", "a".repeat(163)),
            "stem must truncate to fit MAX_SANITIZED_LEN while the extension survives intact"
        );
    }

    #[test]
    fn sanitize_filename_oversized_extension_falls_back_to_flat_truncation() {
        // The ".xxx...xxx" extension alone (251 bytes) exceeds the 167-byte cap.
        let name = format!("a.{}", "x".repeat(250));
        let result = sanitize_filename(&name);
        assert_eq!(
            result,
            format!("a.{}", "x".repeat(165)),
            "extension budget exceeded — must flat-truncate instead of preserving it"
        );
    }

    // ── filename_prefix length cap ──────────────────────────────────────────

    fn exec_input(folder_path: &str, prefix: &str, filename: &str, b64: &str) -> NodeInput {
        NodeInput {
            resolved_credentials: std::collections::HashMap::new(),
            cancel_token: None,
            node_id:      "test-node".to_string(),
            workflow_id:  "test-wf".to_string(),
            execution_id: "test-exec".to_string(),
            input: json!({
                "folder_path": folder_path,
                "filename_prefix": prefix,
                "files": [file_entry(filename, b64)]
            }),
            context: ExecutionContext {
                variables:    HashMap::new(),
                node_outputs: std::sync::Arc::new(HashMap::new()),
                metadata:     HashMap::new(),
                ..Default::default()
            },
        }
    }

    #[tokio::test]
    async fn filename_prefix_within_cap_unaffected() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_str().unwrap();
        let prefix = "p".repeat(10);
        let input = exec_input(path, &prefix, "out.txt", "Z29vZA==");
        let out = SaveToFolderNode.execute(input).await;
        assert!(out.success, "{:?}", out.error);
        let expected = format!("{}out.txt", "p".repeat(10));
        assert!(dir.path().join(&expected).exists());
    }

    #[tokio::test]
    async fn filename_prefix_over_cap_truncates_to_max_prefix_len() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_str().unwrap();
        let prefix = "p".repeat(80);
        let input = exec_input(path, &prefix, "out.txt", "Z29vZA==");
        let out = SaveToFolderNode.execute(input).await;
        assert!(out.success, "{:?}", out.error);
        let capped = format!("{}out.txt", "p".repeat(MAX_PREFIX_LEN));
        assert!(dir.path().join(&capped).exists(), "prefix must be capped to MAX_PREFIX_LEN bytes");
        let uncapped = format!("{}out.txt", "p".repeat(80));
        assert!(!dir.path().join(&uncapped).exists(), "uncapped 80-byte prefix must not appear on disk");
    }

    // ── input_schema() ────────────────────────────────────────────────────────

    #[test]
    fn input_schema_filename_prefix_has_max_length() {
        let schema = SaveToFolderNode.input_schema();
        assert_eq!(
            schema["properties"]["filename_prefix"]["maxLength"],
            json!(MAX_PREFIX_LEN)
        );
    }

    // ── names, sandbox, overwrite ─────────────────────────────────────────────

    fn sandboxed_input(root: &Path, cfg: Value) -> NodeInput {
        let mut input = exec_input("", "", "unused", "");
        input.input = cfg;
        input.context.metadata.insert(
            fs_sandbox::SANDBOX_KEY.to_string(),
            Value::String(root.to_str().unwrap().to_string()),
        );
        input
    }

    #[tokio::test]
    async fn colliding_names_in_one_run_get_numeric_suffixes_instead_of_overwriting() {
        let dir = TempDir::new().unwrap();
        let cfg = json!({
            "files": [
                file_entry("a.txt", "QUFB"),
                file_entry("A.TXT", "QkJC"),
                json!({ "data": "Z29vZA==" }),
                json!({ "data": "aW1nLWRhdGE=" }),
            ]
        });
        let out = flat_mode(&cfg, None, dir.path().to_str().unwrap(), "", true).await;
        assert!(out.success, "{:?}", out.error);
        assert_eq!(out.output.unwrap()["count"], 4);
        let read = |n: &str| std::fs::read_to_string(dir.path().join(n)).unwrap();
        assert_eq!(read("a.txt"), "AAA");
        assert_eq!(read("A_2.TXT"), "BBB");
        assert_eq!(read("file.bin"), "good");
        assert_eq!(read("file_2.bin"), "img-data");
    }

    #[tokio::test]
    async fn overwrite_given_as_text_is_honoured_and_garbage_is_rejected() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_str().unwrap();
        std::fs::write(dir.path().join("out.txt"), "original").unwrap();

        let mut input = exec_input(path, "", "out.txt", "QkJC");
        input.input["overwrite"] = json!("false");
        let out = SaveToFolderNode.execute(input).await;
        assert!(out.success, "{:?}", out.error);
        let o = out.output.unwrap();
        assert_eq!((o["count"].as_u64(), o["skipped"].as_u64()), (Some(0), Some(1)));
        assert_eq!(std::fs::read_to_string(dir.path().join("out.txt")).unwrap(), "original");

        let mut input = exec_input(path, "", "out.txt", "QkJC");
        input.input["overwrite"] = json!("maybe");
        assert_eq!(SaveToFolderNode.execute(input).await.error.unwrap().code, "INVALID_CONFIG");
        assert_eq!(std::fs::read_to_string(dir.path().join("out.txt")).unwrap(), "original");
    }

    #[tokio::test]
    async fn run_where_every_file_fails_is_a_failure_in_flat_mode_too() {
        let dir = TempDir::new().unwrap();
        let cfg = json!({ "files": [json!({ "filename": "a.txt" }), file_entry("b.txt", "!!!")] });
        let out = flat_mode(&cfg, None, dir.path().to_str().unwrap(), "", true).await;
        assert_eq!(out.error.expect("must fail").code, "SAVE_FAILED");
    }

    #[tokio::test]
    async fn data_uri_and_wrapped_base64_are_decoded() {
        let dir = TempDir::new().unwrap();
        let cfg = json!({ "files": [
            file_entry("uri.txt", "data:text/plain;base64,SGVsbG8gV29ybGQ="),
            file_entry("wrapped.txt", "SGVsbG8g\nV29ybGQ="),
        ] });
        let out = flat_mode(&cfg, None, dir.path().to_str().unwrap(), "", true).await;
        assert!(out.success, "{:?}", out.error);
        for name in ["uri.txt", "wrapped.txt"] {
            assert_eq!(std::fs::read_to_string(dir.path().join(name)).unwrap(), "Hello World");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn sandbox_refuses_a_subfolder_that_is_a_symlink_out_of_it() {
        let sandbox = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        std::os::unix::fs::symlink(outside.path(), sandbox.path().join("images")).unwrap();

        let source_expr = serde_json::to_string(&json!([file_entry("img.txt", "aW1nLWRhdGE=")])).unwrap();
        let input = sandboxed_input(sandbox.path(), json!({
            "folder_path": ".",
            "subfolders": [{ "id": "sf1", "name": "images", "source_expr": source_expr }],
        }));
        let out = SaveToFolderNode.execute(input).await;
        assert_eq!(out.error.expect("must fail").code, "SAVE_FAILED");
        assert!(!outside.path().join("img.txt").exists(), "nothing may land outside the sandbox");
    }

    #[tokio::test]
    async fn sandbox_refuses_folder_paths_outside_the_root() {
        let sandbox = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let input = sandboxed_input(sandbox.path(), json!({
            "folder_path": outside.path().to_str().unwrap(),
            "files": [file_entry("a.txt", "QUFB")],
        }));
        assert_eq!(SaveToFolderNode.execute(input).await.error.unwrap().code, "PATH_OUTSIDE_SANDBOX");
        assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
    }

    // ── sanitize_component ───────────────────────────────────────────────────

    #[test]
    fn sanitize_filename_keeps_inner_dots_and_replaces_unusable_names() {
        let cases = [
            ("report..final.txt", "report..final.txt"),
            (".env", ".env"),
            ("../../etc/passwd", "....etcpasswd"),
            ("a.txt. ", "a.txt"),
            (".", "file.bin"),
            ("..", "file.bin"),
            (" . ", "file.bin"),
            ("", "file.bin"),
            ("nul.txt", "file.bin"),
            ("CON", "file.bin"),
            ("a\0b/c.txt", "abc.txt"),
        ];
        for (input, expected) in cases {
            assert_eq!(sanitize_filename(input), expected, "input: {input:?}");
        }
        assert_eq!(sanitize_component("", "unnamed"), "unnamed");
    }
}
