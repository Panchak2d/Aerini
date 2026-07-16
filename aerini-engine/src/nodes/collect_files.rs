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
use std::collections::HashSet;

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortDefinition, PortPosition};

/// Caps applied while merging files across sources (S3-13): `merged_files`
/// previously grew without bound, and each entry typically embeds a file's
/// full contents inline as base64. Both caps are checked as files are
/// collected — the node fails fast rather than silently truncating the
/// batch (a truncated file set handed to a downstream Save-to-Folder/S3
/// node would look like a successful, complete run).
const MAX_MERGED_FILES: usize = 10_000;
const MAX_MERGED_BYTES: usize = 10 * 1024 * 1024;

pub struct CollectFilesNode;

#[async_trait]
impl Node for CollectFilesNode {
    fn type_id(&self) -> &'static str { "collect_files" }
    fn display_name(&self) -> &'static str { "Collect Files" }
    fn node_type(&self) -> NodeType { NodeType::Logic }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Gather files from multiple upstream sources into a single list for downstream processing." }

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
                return NodeOutput::success_with_logs(
                    json!({ "files": [], "count": 0, "source": "collect_files" }),
                    vec!["No sources configured".to_string()],
                );
            }
        };

        let mut merged_files: Vec<Value> = Vec::new();
        let mut seen_filenames: HashSet<String> = HashSet::new();
        let mut logs: Vec<String> = Vec::new();
        let mut total_bytes: usize = 0;

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
                let file_bytes = file.get("data").and_then(Value::as_str).map(str::len).unwrap_or(0);

                if merged_files.len() + 1 > MAX_MERGED_FILES {
                    return NodeOutput::failure_with_logs(
                        NodeError::unrecoverable(
                            "TOO_MANY_FILES",
                            format!("Collect Files exceeds {} file limit", MAX_MERGED_FILES),
                        ),
                        logs,
                    );
                }
                if total_bytes + file_bytes > MAX_MERGED_BYTES {
                    return NodeOutput::failure_with_logs(
                        NodeError::unrecoverable(
                            "TOTAL_SIZE_EXCEEDED",
                            format!(
                                "Collect Files exceeds {} MB combined file-data limit",
                                MAX_MERGED_BYTES / (1024 * 1024)
                            ),
                        ),
                        logs,
                    );
                }
                total_bytes += file_bytes;

                let original_name = file["filename"].as_str().unwrap_or("file.bin").to_string();
                let deduped = deduplicate_filename(&original_name, source_index, &seen_filenames);
                seen_filenames.insert(deduped.clone());

                let mut f = file;
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

pub fn derive_ports(config: &Value) -> NodePorts {
    let mut inputs: Vec<PortDefinition> = Vec::new();

    if let Some(sources) = config["sources"].as_array() {
        if !sources.is_empty() {
            for source in sources {
                let id    = source["id"].as_str().unwrap_or("input").to_string();
                let label = source["name"].as_str().unwrap_or("Source").to_string();
                inputs.push(PortDefinition { id, label, position: PortPosition::Left, port_type: Some("files".to_string()) });
            }
        }
    }

    // Always show at least 1 input port (PLAN: "Minimum 1 input port always shown")
    if inputs.is_empty() {
        inputs.push(PortDefinition {
            id:        "input".to_string(),
            label:     "Source".to_string(),
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
    seen: &HashSet<String>,
) -> String {
    if !seen.contains(filename) {
        return filename.to_string();
    }
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


// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExecutionContext;
    use serde_json::json;

    fn make_input(sources_json: Value) -> NodeInput {
        NodeInput {
            node_id: "n1".to_string(),
            workflow_id: "wf".to_string(),
            execution_id: "exec".to_string(),
            input: json!({ "sources": sources_json }),
            context: ExecutionContext::default(),
        }
    }

    fn file(name: &str) -> Value {
        json!({ "filename": name, "data": "abc", "mime_type": "text/plain" })
    }

    fn media_contract(files: Vec<Value>) -> String {
        serde_json::to_string(&json!({ "files": files, "count": files.len() })).unwrap()
    }

    fn bare_array(files: Vec<Value>) -> String {
        serde_json::to_string(&Value::Array(files)).unwrap()
    }

    // ── No sources configured ───────────────────────────────────────────────

    #[tokio::test]
    async fn no_sources_returns_empty_files() {
        let input = NodeInput {
            node_id: "n1".to_string(),
            workflow_id: "wf".to_string(),
            execution_id: "exec".to_string(),
            input: json!({}), // no "sources" key
            context: ExecutionContext::default(),
        };
        let out = CollectFilesNode.execute(input).await;
        assert!(out.success);
        let data = out.output.unwrap();
        assert_eq!(data["count"], json!(0));
        assert_eq!(data["files"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn empty_sources_array_returns_empty_files() {
        let out = CollectFilesNode.execute(make_input(json!([]))).await;
        assert!(out.success);
        assert_eq!(out.output.unwrap()["count"], json!(0));
    }

    // ── Media contract shape ────────────────────────────────────────────────

    #[tokio::test]
    async fn single_source_media_contract_shape() {
        let contract = media_contract(vec![file("a.txt"), file("b.txt")]);
        let out = CollectFilesNode.execute(make_input(json!([
            { "id": "s1", "name": "Source 1", "source_expr": contract }
        ]))).await;
        assert!(out.success);
        let data = out.output.unwrap();
        assert_eq!(data["count"], json!(2));
    }

    // ── Bare array shape ────────────────────────────────────────────────────

    #[tokio::test]
    async fn single_source_bare_array_shape() {
        let arr = bare_array(vec![file("x.txt")]);
        let out = CollectFilesNode.execute(make_input(json!([
            { "id": "s1", "name": "S1", "source_expr": arr }
        ]))).await;
        assert!(out.success);
        assert_eq!(out.output.unwrap()["count"], json!(1));
    }

    // ── Accumulate across multiple sources ─────────────────────────────────

    #[tokio::test]
    async fn two_sources_files_accumulated() {
        let c1 = media_contract(vec![file("p.txt")]);
        let c2 = media_contract(vec![file("q.txt"), file("r.txt")]);
        let out = CollectFilesNode.execute(make_input(json!([
            { "id": "s1", "name": "A", "source_expr": c1 },
            { "id": "s2", "name": "B", "source_expr": c2 }
        ]))).await;
        assert!(out.success);
        assert_eq!(out.output.unwrap()["count"], json!(3));
    }

    // ── Filename deduplication ──────────────────────────────────────────────

    #[tokio::test]
    async fn duplicate_filename_gets_suffix() {
        let c1 = media_contract(vec![file("dup.txt")]);
        let c2 = media_contract(vec![file("dup.txt")]); // same name
        let out = CollectFilesNode.execute(make_input(json!([
            { "id": "s1", "name": "A", "source_expr": c1 },
            { "id": "s2", "name": "B", "source_expr": c2 }
        ]))).await;
        assert!(out.success);
        let data = out.output.unwrap();
        let files = data["files"].as_array().unwrap();
        assert_eq!(files.len(), 2);
        let names: Vec<&str> = files.iter()
            .map(|f| f["filename"].as_str().unwrap())
            .collect();
        assert!(names[0] != names[1], "filenames should differ after dedup: {:?}", names);
    }

    // ── Empty source_expr skipped ───────────────────────────────────────────

    #[tokio::test]
    async fn source_with_empty_expr_skipped() {
        let c = media_contract(vec![file("ok.txt")]);
        let out = CollectFilesNode.execute(make_input(json!([
            { "id": "empty", "name": "Empty", "source_expr": "" },
            { "id": "good",  "name": "Good",  "source_expr": c }
        ]))).await;
        assert!(out.success);
        assert_eq!(out.output.unwrap()["count"], json!(1));
    }

    // ── Memory caps (S3-13) ─────────────────────────────────────────────────

    #[tokio::test]
    async fn file_count_over_cap_fails_cleanly() {
        let files: Vec<Value> = (0..=MAX_MERGED_FILES).map(|i| file(&format!("f{i}.txt"))).collect();
        let c = media_contract(files);
        let out = CollectFilesNode.execute(make_input(json!([
            { "id": "s1", "name": "S1", "source_expr": c }
        ]))).await;
        assert!(!out.success, "expected failure once file count exceeds MAX_MERGED_FILES");
        assert_eq!(out.error.unwrap().code, "TOO_MANY_FILES");
    }

    #[tokio::test]
    async fn file_count_at_cap_succeeds() {
        let files: Vec<Value> = (0..MAX_MERGED_FILES).map(|i| file(&format!("f{i}.txt"))).collect();
        let c = media_contract(files);
        let out = CollectFilesNode.execute(make_input(json!([
            { "id": "s1", "name": "S1", "source_expr": c }
        ]))).await;
        assert!(out.success, "MAX_MERGED_FILES itself must still be accepted");
        assert_eq!(out.output.unwrap()["count"], json!(MAX_MERGED_FILES));
    }

    #[tokio::test]
    async fn total_size_over_cap_fails_cleanly() {
        let big = "a".repeat(MAX_MERGED_BYTES); // exactly at the cap, alone
        let f1 = json!({ "filename": "big.bin", "data": big, "mime_type": "application/octet-stream" });
        let f2 = file("tips_it_over.txt"); // any additional bytes push total over
        let c = media_contract(vec![f1, f2]);
        let out = CollectFilesNode.execute(make_input(json!([
            { "id": "s1", "name": "S1", "source_expr": c }
        ]))).await;
        assert!(!out.success, "expected failure once combined data size exceeds MAX_MERGED_BYTES");
        assert_eq!(out.error.unwrap().code, "TOTAL_SIZE_EXCEEDED");
    }

    #[tokio::test]
    async fn total_size_at_cap_succeeds() {
        let big = "a".repeat(MAX_MERGED_BYTES); // exactly at the cap, no more
        let f1 = json!({ "filename": "big.bin", "data": big, "mime_type": "application/octet-stream" });
        let c = media_contract(vec![f1]);
        let out = CollectFilesNode.execute(make_input(json!([
            { "id": "s1", "name": "S1", "source_expr": c }
        ]))).await;
        assert!(out.success, "exactly MAX_MERGED_BYTES must still be accepted");
    }

    // ── parse_source_expr unit tests ────────────────────────────────────────

    #[test]
    fn parse_source_expr_object() {
        let v = parse_source_expr("{\"files\":[]}");
        assert!(v.is_object());
    }

    #[test]
    fn parse_source_expr_array() {
        let v = parse_source_expr("[1,2,3]");
        assert!(v.is_array());
    }

    #[test]
    fn parse_source_expr_non_json_returns_null() {
        let v = parse_source_expr("{{not.json}}");
        assert!(v.is_null());
    }

    // ── deduplicate_filename unit tests ─────────────────────────────────────

    #[test]
    fn dedup_filename_no_collision() {
        let seen = std::collections::HashSet::new();
        let result = deduplicate_filename("file.txt", 1, &seen);
        assert_eq!(result, "file.txt");
    }

    #[test]
    fn dedup_filename_with_extension() {
        let mut seen = std::collections::HashSet::new();
        seen.insert("file.txt".to_string());
        let result = deduplicate_filename("file.txt", 1, &seen);
        assert_eq!(result, "file_1.txt");
    }

    #[test]
    fn dedup_filename_no_extension() {
        let mut seen = std::collections::HashSet::new();
        seen.insert("README".to_string());
        let result = deduplicate_filename("README", 2, &seen);
        assert_eq!(result, "README_2");
    }

    // ── derive_ports ────────────────────────────────────────────────────────

    #[test]
    fn derive_ports_no_config_gives_one_input() {
        let ports = derive_ports(&Value::Null);
        assert_eq!(ports.inputs.len(), 1);
        assert_eq!(ports.inputs[0].id, "input");
    }

    #[test]
    fn derive_ports_with_sources_gives_matching_inputs() {
        let cfg = json!({ "sources": [
            { "id": "s1", "name": "Source A" },
            { "id": "s2", "name": "Source B" }
        ]});
        let ports = derive_ports(&cfg);
        assert_eq!(ports.inputs.len(), 2);
        assert_eq!(ports.inputs[0].id, "s1");
        assert_eq!(ports.inputs[1].id, "s2");
    }
}
