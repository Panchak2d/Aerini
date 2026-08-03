use async_trait::async_trait;
use serde_json::{json, Value};

use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::Node;
use super::util::ordered_node_outputs;

/// Merge node — collects all upstream node outputs into a single object.
///
/// Config shape:
/// {
///   "mode": "object" | "array"   (default: "object")
///     "object" -> { "node_id_1": <output>, "node_id_2": <output>, ... }
///     "array"  -> [ <output1>, <output2>, ... ]
///       Order: completion order (context.execution_order — see
///       `ExecutionState::mark_succeeded` in `context.rs`). T2-5 design
///       decision, stated once: this was previously documented as
///       "insertion order" but implemented as raw, non-deterministic
/// HashMap iteration — neither was true. execution_order uses
///       move-to-end semantics (a node that completes more than once, e.g.
///       inside a loop body, reflects its most recent completion position,
///       not its first) rather than literal first-insertion order, the same
///       semantic Batch J's output_node.rs "most recent" fix already
///       depends on. For Merge's actual use — waiting on independent
///       parallel branches that each complete once before the merge fires
///       — this is indistinguishable from true insertion order; it only
///       diverges for a predecessor that re-completes before the merge
///       runs, in which case "most recent position" is the more useful
///       behavior anyway (deterministic and reflects the freshest value),
///       not a special case worth a second ordering mechanism.
/// }
pub struct MergeNode;

#[async_trait]
impl Node for MergeNode {
    fn type_id(&self) -> &'static str { "merge" }
    fn display_name(&self) -> &'static str { "Merge" }
    fn node_type(&self) -> NodeType { NodeType::Logic }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Wait for all incoming parallel branches to complete, then continue as a single execution path." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "mode": {
                    "type": "string",
                    "enum": ["object", "array"],
                    "description": "How to combine inputs: object (keyed by node ID) or array (values only)"
                }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "description": "All upstream node outputs merged into one value"
        })
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let mode = input.input["mode"].as_str().unwrap_or("object");

        let outputs = &input.context.node_outputs;

        // T2-5 residual: the zero-outputs case used to short-circuit to a
        // hardcoded `{"merged": {}}` object *regardless of mode*, while the
        // non-empty path below always returns the merged value unwrapped
        // (a bare array for "array" mode, a bare object for "object" mode —
        // see the tests, which assert on `data` directly, never
        // `data["merged"]`). Net effect: a Merge node with zero upstream
        // outputs — e.g. every incoming branch was disabled or skipped —
        // returned an *object* even when configured for "array" mode,
        // silently handing a downstream node the wrong JSON type for the
        // one case where mode-correctness matters most (an empty result is
        // exactly the case a caller is most likely to check the shape of).
        // Removing the special case lets the match below produce a
        // mode-correct, unwrapped empty value (`[]` or `{}`) the same way
        // it already does for the non-empty case — no behavior change for
        // any non-empty input.
        let merged: Value = match mode {
            "array" => {
                Value::Array(
                    ordered_node_outputs(&input.context)
                        .into_iter()
                        .map(|(_, v)| v)
                        .collect()
                )
            }
            _ => {
                // "object" — key each output by its source node ID
                let mut map = serde_json::Map::new();
                for (node_id, output) in outputs.iter() {
                    map.insert(node_id.clone(), output.clone());
                }
                Value::Object(map)
            }
        };

        NodeOutput::success_with_logs(
            merged,
            vec![format!("Merged {} upstream output(s) as {}", outputs.len(), mode)],
        )
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
    use std::sync::Arc;
    use std::collections::HashMap;

    fn make_input(mode: Option<&str>, node_outputs: HashMap<String, Value>) -> NodeInput {
        let input = match mode {
            Some(m) => json!({ "mode": m }),
            None    => json!({}),
        };
        NodeInput {
            node_id: "n1".to_string(),
            workflow_id: "wf".to_string(),
            execution_id: "exec".to_string(),
            input,
            context: ExecutionContext {
                variables: HashMap::new(),
                node_outputs: Arc::new(node_outputs),
                metadata: HashMap::new(),
                ..Default::default()
            },
        }
    }

    /// Same as make_input, but seeds execution_order too — needed for T2-5
    /// tests that assert on the exact resulting order, not just membership.
    fn make_input_ordered(mode: Option<&str>, node_outputs: HashMap<String, Value>, order: Vec<&str>) -> NodeInput {
        let input = match mode {
            Some(m) => json!({ "mode": m }),
            None    => json!({}),
        };
        NodeInput {
            node_id: "n1".to_string(),
            workflow_id: "wf".to_string(),
            execution_id: "exec".to_string(),
            input,
            context: ExecutionContext {
                variables: HashMap::new(),
                node_outputs: Arc::new(node_outputs),
                metadata: HashMap::new(),
                execution_order: Arc::new(order.into_iter().map(String::from).collect()),
            },
        }
    }

    // ── Empty context ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn empty_context_object_mode_returns_empty_object() {
        // T2-5 residual: object mode's non-empty path already returns the
        // merged value unwrapped (see object_mode_keys_are_node_ids below) —
        // the empty case must match that shape, not a `{"merged": {}}`
        // wrapper that no other path in this node ever produces.
        let out = MergeNode.execute(make_input(Some("object"), HashMap::new())).await;
        assert!(out.success);
        let data = out.output.unwrap();
        assert!(data.is_object(), "expected object output, got {:?}", data);
        assert!(data.as_object().unwrap().is_empty());
    }

    #[tokio::test]
    async fn empty_context_default_mode_returns_empty_object() {
        let out = MergeNode.execute(make_input(None, HashMap::new())).await;
        assert!(out.success);
        let data = out.output.unwrap();
        assert!(data.is_object(), "expected object output, got {:?}", data);
        assert!(data.as_object().unwrap().is_empty());
    }

    #[tokio::test]
    async fn empty_context_array_mode_returns_empty_array_not_object() {
        // T2-5 residual, the actual bug: pre-fix, this returned
        // `{"merged": {}}` (an object) regardless of `mode`, so a Merge node
        // in "array" mode with zero upstream outputs (e.g. every incoming
        // branch was disabled/skipped) silently handed downstream nodes the
        // wrong JSON type — an object where every other "array" mode run
        // produces an array.
        let out = MergeNode.execute(make_input(Some("array"), HashMap::new())).await;
        assert!(out.success);
        let data = out.output.unwrap();
        assert!(data.is_array(), "expected array output for empty array-mode merge, got {:?}", data);
        assert!(data.as_array().unwrap().is_empty());
    }

    // ── object mode ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn object_mode_keys_are_node_ids() {
        let mut outputs = HashMap::new();
        outputs.insert("node_a".to_string(), json!({ "x": 1 }));
        outputs.insert("node_b".to_string(), json!({ "y": 2 }));
        let out = MergeNode.execute(make_input(Some("object"), outputs)).await;
        assert!(out.success);
        let data = out.output.unwrap();
        assert_eq!(data["node_a"]["x"], json!(1));
        assert_eq!(data["node_b"]["y"], json!(2));
    }

    #[tokio::test]
    async fn default_mode_is_object() {
        let mut outputs = HashMap::new();
        outputs.insert("n1".to_string(), json!(42));
        let out = MergeNode.execute(make_input(None, outputs)).await;
        assert!(out.success);
        let data = out.output.unwrap();
        assert_eq!(data["n1"], json!(42));
    }

    // ── array mode ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn array_mode_produces_values_only() {
        let mut outputs = HashMap::new();
        outputs.insert("a".to_string(), json!(10));
        outputs.insert("b".to_string(), json!(20));
        let out = MergeNode.execute(make_input(Some("array"), outputs)).await;
        assert!(out.success);
        let data = out.output.unwrap();
        assert!(data.is_array(), "expected array output, got {:?}", data);
        let arr = data.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        // array mode is now deterministic (ordered_node_outputs), but
        // this fixture has no execution_order seeded, so it falls back to
        // sorted-by-key order — check membership here, exact order below.
        let set: std::collections::HashSet<i64> =
            arr.iter().filter_map(|v| v.as_i64()).collect();
        assert!(set.contains(&10));
        assert!(set.contains(&20));
    }

    #[tokio::test]
    async fn array_mode_order_follows_execution_order_deterministically() {
        // array mode must reflect real completion order, not
        // whichever order the raw HashMap happened to enumerate — same
        // input, run twice, must produce the same order every time.
        let mut outputs = HashMap::new();
        outputs.insert("z".to_string(), json!(20));
        outputs.insert("a".to_string(), json!(10));
        let out = MergeNode.execute(make_input_ordered(Some("array"), outputs, vec!["z", "a"])).await;
        assert!(out.success);
        let arr = out.output.unwrap().as_array().unwrap().clone();
        assert_eq!(arr, vec![json!(20), json!(10)]);
    }

    // ── conflicting keys in object mode ────────────────────────────────────

    #[tokio::test]
    async fn object_mode_different_node_ids_no_key_collision() {
        // Object mode keys by node_id, so two nodes with different IDs never collide
        let mut outputs = HashMap::new();
        outputs.insert("alpha".to_string(), json!({ "val": 1 }));
        outputs.insert("beta".to_string(),  json!({ "val": 2 }));
        let out = MergeNode.execute(make_input(Some("object"), outputs)).await;
        assert!(out.success);
        let data = out.output.unwrap();
        assert_eq!(data["alpha"]["val"], json!(1));
        assert_eq!(data["beta"]["val"],  json!(2));
    }

    // ── log line ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn log_line_contains_output_count() {
        let mut outputs = HashMap::new();
        outputs.insert("x".to_string(), json!(1));
        outputs.insert("y".to_string(), json!(2));
        let out = MergeNode.execute(make_input(Some("object"), outputs)).await;
        assert!(out.success);
        let has_log = out.logs.iter().any(|l| l.contains('2'));
        assert!(has_log, "expected log with count, got: {:?}", out.logs);
    }

    // ── unknown mode falls back to object ──────────────────────────────────

    #[tokio::test]
    async fn unknown_mode_falls_back_to_object() {
        let mut outputs = HashMap::new();
        outputs.insert("src".to_string(), json!({ "k": "v" }));
        let out = MergeNode.execute(make_input(Some("invalid"), outputs)).await;
        assert!(out.success);
        let data = out.output.unwrap();
        // object-mode result: key is node_id "src"
        assert_eq!(data["src"]["k"], json!("v"));
    }
}
