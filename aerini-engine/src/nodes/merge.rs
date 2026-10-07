use std::collections::HashSet;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortArity};
use super::util::ordered_node_outputs_where;

/// `context.metadata` key under which the executor passes a Merge node the ids
/// of the upstream nodes it merges: those with a connection into this node,
/// taken on the port they actually left through, that produced an output.
pub const MERGE_UPSTREAM_IDS_KEY: &str = "__merge_upstream_ids";

/// Merge node — collects the outputs of the nodes wired into it into a single value.
///
/// Only nodes that feed this node directly and whose connection fired in this
/// run contribute. A branch that was not taken, a skipped node, or a node that
/// is not wired in does not appear. With no contributing nodes the result is
/// an empty `{}` / `[]`, and the same happens when the executor did not supply
/// the upstream list.
///
/// Config shape:
/// {
///   "mode": "object" | "array"   (default: "object")
///     "object" -> { "node_id_1": <o>, "node_id_2": <o>, ... }
///     "array"  -> [ <output1>, <output2>, ... ]
///       Order: completion order (context.execution_order — see
///       `ExecutionState::mark_succeeded` in `context.rs`). execution_order
///       uses move-to-end semantics: a node that completes more than once
///       (e.g. inside a loop body) reflects its most recent completion
///       position, not its first — the same semantic output_node.rs's "most
///       recent" behavior depends on. For Merge's actual use — waiting on
///       independent parallel branches that each complete once before the
///       merge fires — this is indistinguishable from true insertion order;
///       it only diverges for a predecessor that re-completes before the
///       merge runs, in which case "most recent position" is the more
///       useful behavior anyway (deterministic and reflects the freshest
///       value), not a special case worth a second ordering mechanism.
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
            "description": "The outputs of the upstream nodes wired into this one, merged into one value"
        })
    }

    fn ports(&self) -> NodePorts {
        let mut ports = NodePorts::default();
        if let Some(input) = ports.inputs.first_mut() {
            input.arity = PortArity::Multi;
        }
        ports
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let mode = input.input["mode"].as_str().unwrap_or("object");

        let upstream: HashSet<&str> = input.context.metadata
            .get(MERGE_UPSTREAM_IDS_KEY)
            .and_then(Value::as_array)
            .map(|ids| ids.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();

        let outputs = ordered_node_outputs_where(&input.context, |id| upstream.contains(id));
        let count = outputs.len();

        let merged: Value = match mode {
            "array" => Value::Array(outputs.into_iter().map(|(_, v)| v).collect()),
            _       => Value::Object(outputs.into_iter().collect()),
        };

        NodeOutput::success_with_logs(
            merged,
            vec![format!("Merged {} upstream output(s) as {}", count, mode)],
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

    #[test]
    fn input_port_accepts_multiple_wires() {
        let ports = MergeNode.ports();
        assert_eq!(ports.inputs.len(), 1);
        assert_eq!(ports.inputs[0].id, "input");
        assert_eq!(ports.inputs[0].arity, PortArity::Multi);
        assert_eq!(ports.outputs[0].arity, PortArity::Single);
    }

    /// Builds a Merge input whose upstream list is exactly `upstream`.
    fn make_input_with_upstream(
        mode: Option<&str>,
        node_outputs: HashMap<String, Value>,
        order: Vec<&str>,
        upstream: Option<Vec<&str>>,
    ) -> NodeInput {
        let input = match mode {
            Some(m) => json!({ "mode": m }),
            None    => json!({}),
        };
        let mut metadata = HashMap::new();
        if let Some(ids) = upstream {
            metadata.insert(MERGE_UPSTREAM_IDS_KEY.to_string(), json!(ids));
        }
        NodeInput {
            resolved_credentials: std::collections::HashMap::new(),
            cancel_token: None,
            node_id: "n1".to_string(),
            workflow_id: "wf".to_string(),
            execution_id: "exec".to_string(),
            input,
            context: ExecutionContext {
                variables: HashMap::new(),
                node_outputs: Arc::new(node_outputs),
                metadata,
                execution_order: Arc::new(order.into_iter().map(String::from).collect()),
            },
        }
    }

    /// Every key in `node_outputs` is an upstream node of this Merge.
    fn make_input(mode: Option<&str>, node_outputs: HashMap<String, Value>) -> NodeInput {
        let ids: Vec<String> = node_outputs.keys().cloned().collect();
        let upstream = Some(ids.iter().map(String::as_str).collect());
        make_input_with_upstream(mode, node_outputs, vec![], upstream)
    }

    /// Same as make_input, but seeds execution_order too — needed for tests
    /// that assert on the exact resulting order, not just membership.
    fn make_input_ordered(mode: Option<&str>, node_outputs: HashMap<String, Value>, order: Vec<&str>) -> NodeInput {
        let ids: Vec<String> = node_outputs.keys().cloned().collect();
        let upstream = Some(ids.iter().map(String::as_str).collect());
        make_input_with_upstream(mode, node_outputs, order, upstream)
    }

    // ── Empty context ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn empty_context_object_mode_returns_empty_object() {
        // Object mode's non-empty path returns the merged value unwrapped (see
        // object_mode_keys_are_node_ids below) — the empty case must match
        // that shape, not a `{"merged": {}}` wrapper that no other path in
        // this node ever produces.
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
        // A Merge node in "array" mode with zero upstream outputs (e.g. every
        // incoming branch was disabled/skipped) must still produce an array,
        // not an object — the same JSON type every other "array" mode run
        // produces.
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
        // array mode order is deterministic (ordered_node_outputs), but
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

    // ── upstream restriction ───────────────────────────────────────────────

    #[tokio::test]
    async fn object_mode_excludes_nodes_that_are_not_upstream() {
        let mut outputs = HashMap::new();
        outputs.insert("wired_a".to_string(),  json!({ "x": 1 }));
        outputs.insert("wired_b".to_string(),  json!({ "y": 2 }));
        outputs.insert("unrelated".to_string(), json!({ "secret": true }));
        let out = MergeNode.execute(make_input_with_upstream(
            Some("object"), outputs, vec![], Some(vec!["wired_a", "wired_b"]),
        )).await;
        assert!(out.success);
        let data = out.output.unwrap();
        assert_eq!(data.as_object().unwrap().len(), 2);
        assert!(data.get("unrelated").is_none());
        assert!(out.logs.iter().any(|l| l.contains("Merged 2 upstream")), "logs: {:?}", out.logs);
    }

    #[tokio::test]
    async fn array_mode_orders_the_selected_subset_by_completion_order() {
        let mut outputs = HashMap::new();
        outputs.insert("z".to_string(), json!(20));
        outputs.insert("m".to_string(), json!(99));
        outputs.insert("a".to_string(), json!(10));
        let out = MergeNode.execute(make_input_with_upstream(
            Some("array"), outputs, vec!["z", "m", "a"], Some(vec!["a", "z"]),
        )).await;
        assert!(out.success);
        assert_eq!(out.output.unwrap(), json!([20, 10]));
    }

    #[tokio::test]
    async fn no_upstream_nodes_returns_an_empty_value_even_when_other_outputs_exist() {
        for (mode, empty) in [("object", json!({})), ("array", json!([]))] {
            for upstream in [None, Some(vec![])] {
                let mut outputs = HashMap::new();
                outputs.insert("other".to_string(), json!(1));
                let out = MergeNode.execute(make_input_with_upstream(
                    Some(mode), outputs, vec![], upstream,
                )).await;
                assert!(out.success);
                assert_eq!(out.output.unwrap(), empty, "mode {mode}");
            }
        }
    }
}
