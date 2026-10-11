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
// Output: standard media contract
//   { "files": [...merged], "count": N, "source": "collect_files" }
//
// Filename collision: a file whose name (compared case-insensitively) is already
// taken gets "_{source_index}" inserted before the extension, then
// "_{source_index}_2", "_{source_index}_3", ... until it is unique.
//
// Minimum 1 input port always shown (even with 0 sources configured).

use async_trait::async_trait;
use serde_json::{json, Value};
use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortArity, PortDefinition, PortPosition};
use super::util::FilenameSet;

/// Caps applied while merging files across sources: `merged_files` must stay
/// bounded since each entry typically embeds a file's full contents inline
/// as base64. Both caps are checked as files are collected — the node fails
/// fast rather than silently truncating the batch (a truncated file set
/// handed to a downstream Save-to-Folder/S3 node would look like a
/// successful, complete run). The byte cap counts base64 characters.
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
            Some(s) => s,
            None => {
                return NodeOutput::success_with_logs(
                    json!({ "files": [], "count": 0, "source": "collect_files" }),
                    vec!["No sources configured".to_string()],
                );
            }
        };

        let mut merged_files: Vec<Value> = Vec::new();
        let mut names = FilenameSet::default();
        let mut logs: Vec<String> = Vec::new();
        let mut total_bytes: usize = 0;

        for (source_index, source) in sources.iter().enumerate() {
            let name = source["name"].as_str().unwrap_or("unnamed");
            let expr_val = source["source_expr"].as_str().unwrap_or("").trim();

            if expr_val.is_empty() {
                logs.push(format!("Source '{}' (index {}) has no expression — skipped", name, source_index));
                continue;
            }

            let parsed = match parse_source_expr(expr_val) {
                Ok(Some(v)) => v,
                Ok(None) => {
                    logs.push(format!("Source '{}' did not resolve to a file list — skipped", name));
                    continue;
                }
                Err(e) => {
                    return NodeOutput::failure_with_logs(
                        NodeError::unrecoverable(
                            "INVALID_SOURCE",
                            format!("Source '{}' (index {}) is not valid JSON: {}", name, source_index, e),
                        ),
                        logs,
                    );
                }
            };
            let had_files_key = parsed.is_array() || parsed.get("files").is_some();
            let files = extract_files_array(parsed);

            if files.is_empty() {
                if had_files_key {
                    logs.push(format!("Source '{}' resolved to 0 files — skipped", name));
                } else {
                    logs.push(format!("Source '{}' has no 'files' array — skipped", name));
                }
                continue;
            }

            logs.push(format!("Source '{}': {} file(s)", name, files.len()));

            for (file_index, mut file) in files.into_iter().enumerate() {
                let Some(entry) = file.as_object_mut() else {
                    return NodeOutput::failure_with_logs(
                        NodeError::unrecoverable(
                            "INVALID_FILE_ENTRY",
                            format!("Source '{}' file #{} is not an object with filename and data", name, file_index),
                        ),
                        logs,
                    );
                };
                let file_bytes = entry.get("data").and_then(Value::as_str).map(str::len).unwrap_or(0);

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
                                "Collect Files exceeds {} MB combined base64 file-data limit",
                                MAX_MERGED_BYTES / (1024 * 1024)
                            ),
                        ),
                        logs,
                    );
                }
                total_bytes += file_bytes;

                let original_name = entry.get("filename").and_then(Value::as_str).unwrap_or("file.bin");
                let unique = names.claim(original_name, Some(source_index));
                entry.insert("filename".to_string(), Value::String(unique));
                merged_files.push(file);
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
                inputs.push(PortDefinition { id, label, position: PortPosition::Left, port_type: Some("files".to_string()), arity: PortArity::Single });
            }
        }
    }

    // At least one input port is always shown, even with no sources configured.
    if inputs.is_empty() {
        inputs.push(PortDefinition {
            id:        "input".to_string(),
            label:     "Source".to_string(),
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

/// Parses a resolved source expression. `Ok(None)` means it is not a JSON
/// object or array (an empty or unresolved value, which callers skip);
/// `Err` means it looks like JSON but is malformed.
fn parse_source_expr(expr: &str) -> Result<Option<Value>, String> {
    let trimmed = expr.trim();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        serde_json::from_str(trimmed).map(Some).map_err(|e| e.to_string())
    } else {
        Ok(None)
    }
}

/// Extract a files array from a resolved source expression.
/// Accepts two shapes:
///   1. Media contract object: { "files": [...], ... }
///   2. Bare files array: [{ "filename": ..., "data": ..., "mime_type": ... }, ...]
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
            resolved_credentials: std::collections::HashMap::new(),
            cancel_token: None,
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
            resolved_credentials: std::collections::HashMap::new(),
            cancel_token: None,
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

    // ── Memory caps ─────────────────────────────────────────────────

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

    // ── source parsing ──────────────────────────────────────────────────────

    #[test]
    fn parse_source_expr_distinguishes_unusable_from_malformed() {
        assert!(parse_source_expr("{\"files\":[]}").unwrap().unwrap().is_object());
        assert!(parse_source_expr("[1,2,3]").unwrap().unwrap().is_array());
        assert_eq!(parse_source_expr("undefined").unwrap(), None);
        assert!(parse_source_expr("{{not.json}}").is_err());
        assert!(parse_source_expr("[{\"filename\":").is_err());
    }

    #[tokio::test]
    async fn malformed_json_source_fails_instead_of_being_skipped() {
        let good = media_contract(vec![file("ok.txt")]);
        let out = CollectFilesNode.execute(make_input(json!([
            { "id": "s1", "name": "Good", "source_expr": good },
            { "id": "s2", "name": "Broken", "source_expr": "[{\"filename\": \"cut" }
        ]))).await;
        let err = out.error.expect("a half-delivered batch must not look complete");
        assert_eq!(err.code, "INVALID_SOURCE");
        assert!(err.message.contains("Broken"), "{}", err.message);
    }

    #[tokio::test]
    async fn non_object_file_entry_fails_naming_the_source() {
        let out = CollectFilesNode.execute(make_input(json!([
            { "id": "s1", "name": "Mixed", "source_expr": "[\"just-a-string\"]" }
        ]))).await;
        assert_eq!(out.error.expect("must fail").code, "INVALID_FILE_ENTRY");
    }

    // ── filename uniqueness ─────────────────────────────────────────────────

    #[tokio::test]
    async fn every_merged_filename_is_unique_even_for_repeats_inside_one_source() {
        let c1 = media_contract(vec![file("a.txt"), file("a.txt"), file("a.txt"), file("A.TXT")]);
        let c2 = media_contract(vec![file("a.txt"), json!({ "data": "x" }), json!({ "data": "y" })]);
        let out = CollectFilesNode.execute(make_input(json!([
            { "id": "s1", "name": "A", "source_expr": c1 },
            { "id": "s2", "name": "B", "source_expr": c2 }
        ]))).await;
        assert!(out.success, "{:?}", out.error);
        let data = out.output.unwrap();
        let names: Vec<String> = data["files"].as_array().unwrap().iter()
            .map(|f| f["filename"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(names, [
            "a.txt", "a_0.txt", "a_0_2.txt", "A_0_3.TXT",
            "a_1.txt", "file.bin", "file_1.bin",
        ]);
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
