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
// Path traversal protection: all '/', '\', '..' stripped from every filename
// before any path is constructed. folder_path is user-selected and not sanitized.
//
// Concurrent writes: tokio::spawn per file (all spawned before any awaited).
//
// Output: { saved, count, folder, skipped, errors }

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tokio::fs;

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortDefinition, PortPosition};

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
                    "description": "Optional prefix prepended to every saved filename"
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

        // Server mode: enforce __file_sandbox_dir if set, matching the same policy as FileNode.
        // The resolved path produced below is what every subsequent operation (create_dir_all,
        // file writes, the echoed "folder" output field) uses — raw_folder_path is never
        // touched again after this block, mirroring file.rs's resolve-once pattern.
        let folder_path: String = if let Some(sandbox_val) = input.context.metadata.get("__file_sandbox_dir") {
            if let Some(sandbox_str) = sandbox_val.as_str() {
                // Canonicalize the sandbox root so symlinks in the operator-supplied path
                // don't defeat the containment check.
                let sandbox = match std::fs::canonicalize(sandbox_str) {
                    Ok(p) => p,
                    Err(_) => return NodeOutput::failure(NodeError::unrecoverable(
                        "INVALID_PATH",
                        "Configured sandbox directory does not exist or cannot be resolved",
                    )),
                };
                // Resolve relative paths against the canonical sandbox root.
                let abs: std::path::PathBuf = if raw_folder_path.starts_with('/') {
                    std::path::PathBuf::from(&raw_folder_path)
                } else {
                    sandbox.join(&raw_folder_path)
                };
                let outside = || NodeOutput::failure(NodeError::unrecoverable(
                    "PATH_OUTSIDE_SANDBOX",
                    format!("folder_path '{}' is outside the permitted sandbox directory '{}'",
                        raw_folder_path, sandbox_str),
                ));
                // The target directory may not exist yet (create_dir_all runs later).
                // Canonicalize the deepest existing ancestor and check containment.
                // This also dereferences any symlinks inside the sandbox that point outside.
                let mut check = abs.as_path();
                let canonical_parent = loop {
                    match std::fs::canonicalize(check) {
                        Ok(p) => break p,
                        Err(_) => match check.parent() {
                            Some(p) => check = p,
                            None => return NodeOutput::failure(NodeError::unrecoverable(
                                "INVALID_PATH",
                                "folder_path cannot be resolved to an existing ancestor",
                            )),
                        },
                    }
                };
                if !canonical_parent.starts_with(&sandbox) {
                    return outside();
                }
                // Rejoin whatever suffix of `abs` doesn't exist yet onto the canonicalized
                // (symlink-dereferenced) existing ancestor, so the value threaded through to
                // flat_mode/subfolder_mode is exactly the path just validated above — not the
                // raw, unresolved folder_path string the old code discarded here.
                let suffix = match abs.strip_prefix(check) {
                    Ok(s) => s,
                    Err(_) => return NodeOutput::failure(NodeError::unrecoverable(
                        "INVALID_PATH",
                        "folder_path could not be resolved relative to its existing ancestor",
                    )),
                };
                // A ".." can only survive into suffix when the component it would walk back
                // through never existed on disk (canonicalize couldn't resolve it away above),
                // so its real target is unverified — reject rather than let create_dir_all/
                // fs::write resolve it past the already-validated ancestor at write time.
                if suffix.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
                    return outside();
                }
                canonical_parent.join(suffix).to_string_lossy().into_owned()
            } else {
                raw_folder_path
            }
        } else {
            raw_folder_path
        };

        let overwrite = cfg["overwrite"].as_bool().unwrap_or(true);
        // Sanitize prefix: strip path separators to prevent directory traversal.
        let raw_prefix = cfg["filename_prefix"].as_str().unwrap_or("").to_string();
        let prefix: String = raw_prefix.chars().filter(|&c| c != '/' && c != '\\').collect();

        let subfolders = cfg["subfolders"].as_array().cloned().unwrap_or_default();

        if subfolders.is_empty() {
            let direct_input = input.context.metadata.get("__direct_input");
            flat_mode(cfg, direct_input, &folder_path, &prefix, overwrite).await
        } else {
            subfolder_mode(&subfolders, &folder_path, &prefix, overwrite).await
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
            files = extract_files_array(direct);
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

    let results = write_files_concurrent(&files, &base, prefix, overwrite).await;
    build_output(results, folder_path)
}

async fn subfolder_mode(
    subfolders: &[Value],
    folder_path: &str,
    prefix: &str,
    overwrite: bool,
) -> NodeOutput {
    let mut all_results: Vec<Result<Value, Value>> = Vec::new();
    let mut logs: Vec<String> = Vec::new();

    for sf in subfolders {
        let sf_name      = sf["name"].as_str().unwrap_or("unnamed");
        // Sanitize subfolder name the same way individual filenames are sanitized
        // (sanitize_filename strips / \ and .. sequences). Without this, a
        // name like "../../escape" resolves outside the sandbox after PathBuf::join.
        let sf_name = {
            let no_sep: String = sf_name.chars()
                .filter(|&c| c != '/' && c != '\\')
                .collect();
            let no_dotdot = no_sep.split("..").collect::<Vec<_>>().join("");
            if no_dotdot.trim().is_empty() { "unnamed".to_string() } else { no_dotdot }
        };
        let source_expr  = sf["source_expr"].as_str().unwrap_or("").trim();

        if source_expr.is_empty() {
            logs.push(format!("Subfolder '{}': no source_expr — skipped", sf_name));
            continue;
        }

        let parsed = parse_source_expr(source_expr);
        let files  = extract_files_array(&parsed);

        // Create directory before the files.is_empty() guard so that configured
        // subfolders always exist on disk even when upstream produces no output.
        let target_dir = PathBuf::from(folder_path).join(&sf_name);
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
        let results = write_files_concurrent(&files, &target_dir, prefix, overwrite).await;
        all_results.extend(results);
    }

    let (ok, errors): (Vec<_>, Vec<_>) = all_results.into_iter().partition(|r| r.is_ok());
    let ok: Vec<Value>     = ok.into_iter().filter_map(|r| r.ok()).collect();
    let mut errors: Vec<Value> = errors.into_iter().map(|r| r.unwrap_err()).collect();
    let (skipped_files, saved): (Vec<Value>, Vec<Value>) =
        ok.into_iter().partition(|v| v["skipped"].as_bool().unwrap_or(false));
    let count         = saved.len();
    let skipped_count = skipped_files.len();

    // Surface zero-output explicitly — prevents a green checkmark when nothing was written.
    if count == 0 && errors.is_empty() && skipped_count == 0 {
        errors.push(json!({
            "filename": "<no input>",
            "reason": "No files were saved — upstream produced no file output"
        }));
    }

    if count == 0 && !errors.is_empty() {
        return NodeOutput::failure(NodeError::unrecoverable(
            "SAVE_FAILED",
            format!("Save to Folder: no files were written. First error: {}",
                errors[0]["reason"].as_str().unwrap_or("unknown")),
        ));
    }

    NodeOutput::success_with_logs(
        json!({ "saved": saved, "count": count, "folder": folder_path, "skipped": skipped_count, "errors": errors }),
        logs,
    )
}

// ── Port derivation ────────────────────────────────────────────────────────────

pub fn derive_ports(config: &Value) -> NodePorts {
    let mut inputs: Vec<PortDefinition> = Vec::new();

    if let Some(subfolders) = config["subfolders"].as_array() {
        if !subfolders.is_empty() {
            for sf in subfolders {
                let id    = sf["id"].as_str().unwrap_or("input").to_string();
                let label = sf["name"].as_str().unwrap_or("Subfolder").to_string();
                inputs.push(PortDefinition { id, label, position: PortPosition::Left, port_type: Some("files".to_string()) });
            }
        }
    }

    if inputs.is_empty() {
        inputs.push(PortDefinition {
            id:        "input".to_string(),
            label:     "In".to_string(),
            position:  PortPosition::Left,
            port_type: Some("files".to_string()),
        });
    }

    NodePorts {
        inputs,
        outputs: vec![PortDefinition {
            id:        "output".to_string(),
            label:     "Out".to_string(),
            position:  PortPosition::Right,
            port_type: None,
        }],
    }
}

// ── Concurrent file writing ───────────────────────────────────────────────────

/// Spawn one task per file. All tasks run concurrently (all spawned before any awaited).
async fn write_files_concurrent(
    files: &[Value],
    dir: &Path,
    prefix: &str,
    overwrite: bool,
) -> Vec<Result<Value, Value>> {
    let handles: Vec<_> = files.iter().map(|file| {
        let dir      = dir.to_path_buf();
        let prefix   = prefix.to_string();
        let file     = file.clone();
        tokio::spawn(async move {
            write_single_file(&file, &dir, &prefix, overwrite).await
        })
    }).collect();

    let mut results = Vec::with_capacity(handles.len());
    for handle in handles {
        match handle.await {
            Ok(r) => results.push(r),
            Err(e) => results.push(Err(json!({
                "filename": "unknown",
                "reason": format!("task panicked: {}", e)
            }))),
        }
    }
    results
}

async fn write_single_file(
    file: &Value,
    dir: &Path,
    prefix: &str,
    overwrite: bool,
) -> Result<Value, Value> {
    let raw_name  = file["filename"].as_str().unwrap_or("file.bin");
    let sanitized = sanitize_filename(raw_name);
    let filename  = if prefix.is_empty() {
        sanitized.clone()
    } else {
        format!("{}{}", prefix, sanitized)
    };

    let path = dir.join(&filename);

    if !overwrite && path.exists() {
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

    let bytes = match decode_base64(data_str) {
        Ok(b)  => b,
        Err(e) => return Err(json!({ "filename": filename, "reason": e })),
    };

    let byte_count = bytes.len();
    if let Err(e) = fs::write(&path, &bytes).await {
        return Err(json!({ "filename": filename, "reason": format!("write failed: {}", e) }));
    }

    Ok(json!({
        "filename": filename,
        "path":     path.display().to_string(),
        "bytes":    byte_count
    }))
}

fn build_output(results: Vec<Result<Value, Value>>, folder: &str) -> NodeOutput {
    let (ok, errors): (Vec<_>, Vec<_>) = results.into_iter().partition(|r| r.is_ok());
    let ok: Vec<Value>     = ok.into_iter().filter_map(|r| r.ok()).collect();
    let errors: Vec<Value> = errors.into_iter().map(|r| r.unwrap_err()).collect();
    // Separate skipped files (overwrite=false, file existed) from actually written files.
    let (skipped, saved): (Vec<Value>, Vec<Value>) =
        ok.into_iter().partition(|v| v["skipped"].as_bool().unwrap_or(false));
    let count         = saved.len();
    let skipped_count = skipped.len();
    NodeOutput::success(json!({
        "saved":   saved,
        "count":   count,
        "folder":  folder,
        "skipped": skipped_count,
        "errors":  errors
    }))
}

// ── Utilities ─────────────────────────────────────────────────────────────────

/// Strip path separators, `..` sequences, null bytes, and Windows reserved device names
/// from a filename. An empty result falls back to "file.bin".
fn sanitize_filename(name: &str) -> String {
    // Windows reserved device names. On Windows, CreateFile("NUL") silently discards
    // all written data; CreateFile("CON") writes to the console. Block all 22 names
    // regardless of extension or case so "NUL.txt" and "nul" are both rejected.
    const WINDOWS_RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL",
        "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
        "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];

    // Strip null bytes (cause ENAMETOOLONG / confusing OS errors), path separators,
    // and .. sequences.
    let cleaned: String = name.chars()
        .filter(|&c| c != '\0' && c != '/' && c != '\\')
        .collect();
    let no_dotdot = cleaned.split("..").collect::<Vec<_>>().join("");

    if no_dotdot.trim().is_empty() {
        return "file.bin".to_string();
    }

    // Check stem (part before first '.') against reserved names, case-insensitively.
    let stem = no_dotdot.split('.').next().unwrap_or("").to_uppercase();
    if WINDOWS_RESERVED.contains(&stem.as_str()) {
        return "file.bin".to_string();
    }

    no_dotdot
}

/// Decode base64. Strips a `data:<mime>;base64,` prefix if present.
fn decode_base64(data: &str) -> Result<Vec<u8>, String> {
    let raw = if let Some(pos) = data.find(',') {
        &data[pos + 1..]
    } else {
        data
    };
    general_purpose::STANDARD
        .decode(raw.trim())
        .map_err(|e| format!("base64 decode failed: {}", e))
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
fn extract_files_array(val: &Value) -> Vec<Value> {
    match val {
        Value::Object(obj) => {
            if let Some(Value::Array(files)) = obj.get("files") {
                files.clone()
            } else {
                vec![]
            }
        }
        Value::Array(arr) => arr.clone(),
        _                 => vec![],
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
        Value::String(s)  => extract_files_array(&parse_source_expr(s)),
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

    // ── sandbox containment (T0-3 / S3-1) ────────────────────────────────────

    // With __file_sandbox_dir set and a RELATIVE folder_path, the write must land
    // inside the sandbox. Before the fix, the containment check validated a
    // sandbox-resolved path that was then discarded — the actual write used the
    // raw, unresolved folder_path, which resolves relative to the process's CWD
    // instead of the sandbox root. This test exercises the full execute() path
    // (not flat_mode directly) since the sandbox resolution lives there.
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

    // P2 regression: empty files array must surface NO_FILES_RECEIVED, not succeed silently.
    #[tokio::test]
    async fn flat_mode_empty_files_array_returns_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_str().unwrap();
        let cfg = json!({ "files": [] });
        let out = flat_mode(&cfg, None, path, "", true).await;
        assert!(!out.success, "expected failure for zero files");
        assert_eq!(out.error.unwrap().code, "NO_FILES_RECEIVED");
    }

    // P2 regression: missing target directory must be created, not error.
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
        // build_output always returns success for flat_mode; errors live in output JSON.
        assert!(out.success, "flat_mode must not panic on partial failure");
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
        let out = subfolder_mode(&subfolders, path, "", true).await;
        assert!(out.success, "{:?}", out.error);
        let o = out.output.unwrap();
        assert_eq!(o["count"], 1);
        assert!(dir.path().join("images").join("img.txt").exists());
    }

    // P2 ordering regression: create_dir_all must fire BEFORE the files.is_empty() guard.
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
        let out = subfolder_mode(&subfolders, path, "", true).await;
        // NodeOutput may be failure (SAVE_FAILED: no files written) — that is expected.
        // The critical assertion is that the directory was created before the guard fired.
        let subdir = dir.path().join("empty_sub");
        assert!(
            subdir.exists(),
            "directory must be created even when source resolves to 0 files (P2 ordering fix)"
        );
        // Confirm the output surfaces the zero-file condition rather than silently succeeding.
        assert!(
            !out.success || out.output.as_ref().map(|o| o["count"] == 0).unwrap_or(false),
            "zero files must not produce a silent success with count > 0"
        );
    }
}
