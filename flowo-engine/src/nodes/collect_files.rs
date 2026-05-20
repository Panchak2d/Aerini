// CollectFiles Node
//
// Fan-in merger: accepts multiple media contract sources (one dynamic input port each)
// and emits a single merged media contract batch.
//
// Dynamic ports: YES — implements is_dynamic_ports() + ports_from_config().
// Config holds a "sources" array. Each source has {id, name, source_expr}.
// After expression resolution by the executor, source_expr is a JSON string
// containing a serialized media contract object or files array.
//
// Output: standard media contract (PLAN.md DATA CONTRACT)
//   { "files": [...merged], "count": N, "source": "collect_files" }
//
// Filename collision: files from source N that collide with earlier filenames
// get a "_{source_index}" suffix inserted before the extension.
//
// Minimum 1 input port always shown (even with 0 sources configured).

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortDefinition, PortPosition};

pub struct CollectFilesNode;

#[async_trait]
impl Node for CollectFilesNode {
    fn type_id(&self) -> &'static str { "collect_files" }
    fn display_name(&self) -> &'static str { "Collect Files" }
    fn node_type(&self) -> NodeType { NodeType::Logic }
    fn version(&self) -> &'static str { "1.0.0" }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "sources": {
                    "type": "array",
                    "description": "Source slots — each has id, name, source_expr (auto-populated by canvas wire)",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id":          { "type": "string" },
                            "name":        { "type": "string" },
                            "source_expr": { "type": "string", "description": "Resolved to JSON string of media contract" }
                        }
                    }
                }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "files":  { "type": "array",  "description": "Merged files from all sources" },
                "count":  { "type": "number" },
                "source": { "type": "string" }
            }
        })
    }

    fn is_dynamic_ports(&self) -> bool { true }

    fn ports_from_config(&self, config: &Value) -> Option<NodePorts> {
        Some(derive_ports(config))
    }

    // Static ports() is never called for dynamic-port nodes, but we define it
    // as the minimum-state ports (1 empty input) for defensive correctness.
    fn ports(&self) -> NodePorts {
        derive_ports(&Value::Null)
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let cfg = &input.input;

        let sources = match cfg["sources"].as_array() {
            Some(s) => s.clone(),
            None => {
                // No sources configured — return empty batch
                return NodeOutput::success_with_logs(
                    json!({ "files": [], "count": 0, "source": "collect_files" }),
                    vec!["No sources configured".to_string()],
                );
            }
        };

        let mut merged_files: Vec<Value> = Vec::new();
        let mut seen_filenames: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut logs: Vec<String> = Vec::new();

        for (source_index, source) in sources.iter().enumerate() {
            let name = source["name"].as_str().unwrap_or("unnamed");
            let expr_val = source["source_expr"].as_str().unwrap_or("").trim();

            if expr_val.is_empty() {
                logs.push(format!("Source '{}' (index {}) has no expression — skipped", name, source_index));
                continue;
            }

            // After executor expression resolution, source_expr is a JSON string
            // representing the serialized media contract or files array.
            let parsed = parse_source_expr(expr_val);
            let files = extract_files_array(&parsed);

            if files.is_empty() {
                logs.push(format!("Source '{}' resolved to 0 files — skipped", name));
                continue;
            }

            logs.push(format!("Source '{}': {} file(s)", name, files.len()));

            for file in files {
                let original_name = file["filename"].as_str().unwrap_or("file.bin").to_string();
                let deduped = deduplicate_filename(&original_name, source_index, &seen_filenames);
                seen_filenames.insert(deduped.clone());

                // Rebuild with (possibly renamed) filename
                let mut f = file.clone();
                if let Some(obj) = f.as_object_mut() {
                    obj.insert("filename".to_string(), Value::String(deduped));
                }
                merged_files.push(f);
            }
        }

        let count = merged_files.len();
        NodeOutput::success_with_logs(
            json!({ "files": merged_files, "count": count, "source": "collect_files" }),
            logs,
        )
    }
}

// ── Port derivation ───────────────────────────────────────────────────────────

pub fn derive_ports(config: &Value) -> NodePorts {
    let mut inputs: Vec<PortDefinition> = Vec::new();

    if let Some(sources) = config["sources"].as_array() {
        if !sources.is_empty() {
            for source in sources {
                let id    = source["id"].as_str().unwrap_or("input").to_string();
                let label = source["name"].as_str().unwrap_or("Source").to_string();
                inputs.push(PortDefinition { id, label, position: PortPosition::Left });
            }
        }
    }

    // Always show at least 1 input port (PLAN: "Minimum 1 input port always shown")
    if inputs.is_empty() {
        inputs.push(PortDefinition {
            id:       "input".to_string(),
            label:    "Source".to_string(),
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

// ── Utilities ─────────────────────────────────────────────────────────────────

/// Parse source_expr string:
/// - If it looks like a JSON object/array: parse as JSON
/// - Otherwise: treat as empty (unresolved expression)
fn parse_source_expr(expr: &str) -> Value {
    let trimmed = expr.trim();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        serde_json::from_str(trimmed).unwrap_or(Value::Null)
    } else {
        Value::Null
    }
}

/// Extract a files array from a resolved source expression.
/// Accepts two shapes:
///   1. Media contract object: { "files": [...], ... }
///   2. Bare files array: [{ "filename": ..., "data": ..., "mime_type": ... }, ...]
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
        _ => vec![],
    }
}

/// If filename already seen, insert "_{source_index}" before the extension.
fn deduplicate_filename(
    filename: &str,
    source_index: usize,
    seen: &std::collections::HashSet<String>,
) -> String {
    if !seen.contains(filename) {
        return filename.to_string();
    }
    // Insert suffix before extension
    let dot = filename.rfind('.');
    let renamed = match dot {
        Some(pos) => format!("{}_{}{}", &filename[..pos], source_index, &filename[pos..]),
        None      => format!("{}_{}", filename, source_index),
    };
    // If renamed also collides (extremely unlikely), append again
    if seen.contains(&renamed) {
        format!("{}_{}", renamed, source_index)
    } else {
        renamed
    }
}
