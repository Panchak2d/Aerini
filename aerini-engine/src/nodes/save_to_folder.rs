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

        let folder_path = match cfg["folder_path"].as_str() {
            Some(p) if !p.is_empty() => p.to_string(),
            _ => return NodeOutput::failure(NodeError::unrecoverable(
                "MISSING_FOLDER_PATH",
                "folder_path is required — set it using the Choose Folder button",
            )),
        };

        // Server mode: enforce __file_sandbox_dir if set, matching the same policy as FileNode.
        if let Some(sandbox_val) = input.context.metadata.get("__file_sandbox_dir") {
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
                let abs: std::path::PathBuf = if folder_path.starts_with('/') {
                    std::path::PathBuf::from(&folder_path)
                } else {
                    sandbox.join(&folder_path)
                };
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
                    return NodeOutput::failure(NodeError::unrecoverable(
                        "PATH_OUTSIDE_SANDBOX",
                        format!("folder_path '{}' is outside the permitted sandbox directory '{}'",
                            folder_path, sandbox_str),
                    ));
                }
            }
        }

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
                inputs.push(PortDefinition { id, label, position: PortPosition::Left });
            }
        }
    }

    if inputs.is_empty() {
        inputs.push(PortDefinition {
            id:       "input".to_string(),
            label:    "In".to_string(),
            position: PortPosition::Left,
        });
    }

    NodePorts {
        inputs,
        outputs: vec![PortDefinition {
            id:       "output".to_string(),
            label:    "Out".to_string(),
            position: PortPosition::Right,
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

