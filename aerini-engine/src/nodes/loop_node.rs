use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortDefinition, PortPosition};
use super::util::traverse_dotpath;

/// Loop node — iterates over an array, emitting one item at a time.
///
/// Because the executor runs nodes in topological order (not as a true runtime loop),
/// the loop node outputs ALL items and marks each one via the execution context.
/// The executor calls loop_body successors once per item by re-activating them.
///
/// Implementation note: the node outputs a cursor into the array. The executor
/// is responsible for re-running the loop body for each item.
pub struct LoopNode;

/// Fixed keys the per-iteration output always sets (see the `json!{}` block in
/// `execute()`) — `item_var`/`index_var` must not collide with any of these
const RESERVED_LOOP_OUTPUT_KEYS: &[&str] = &[
    "total", "item", "index", "all_results", "done",
    "__loop_node_id", "__loop_next_index", "__loop_total",
];

#[async_trait]
impl Node for LoopNode {
    fn type_id(&self) -> &'static str { "loop" }
    fn display_name(&self) -> &'static str { "Loop (For Each)" }
    fn node_type(&self) -> NodeType { NodeType::Logic }
    fn version(&self) -> &'static str { "2.0.0" }
    fn description(&self) -> &'static str { "Run a downstream branch once for each item in a list, then continue with the collected results." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["array_field", "source_node"],
            "properties": {
                "array_field": {
                    "type": "string",
                    "description": "Dot-path to the array to iterate, e.g. 'items' or 'response.results'"
                },
                "source_node": {
                    "type": "string",
                    "description": "Node ID to read the array from."
                },
                "item_var": {
                    "type": "string",
                    "description": "Variable name for the current item (default: 'item')"
                },
                "index_var": {
                    "type": "string",
                    "description": "Variable name for the current index (default: 'index')"
                }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "items":       { "type": "array", "description": "The full array being iterated" },
                "total":       { "type": "number", "description": "Total number of items" },
                "item":        { "description": "Current item in the iteration" },
                "index":       { "type": "number", "description": "Current zero-based index" },
                "all_results": { "type": "array", "description": "Collected results from loop body executions" },
                "done":        { "type": "boolean", "description": "True when all items have been processed" }
            }
        })
    }

    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![PortDefinition {
                id: "input".to_string(), label: "In".to_string(), position: PortPosition::Left, port_type: None,
            }],
            outputs: vec![
                PortDefinition { id: "loop_body".to_string(), label: "Each Item".to_string(), position: PortPosition::Right, port_type: None },
                PortDefinition { id: "done".to_string(),      label: "Done".to_string(),      position: PortPosition::Right, port_type: None },
            ],
        }
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let array_field = match input.input["array_field"].as_str() {
            Some(f) if !f.is_empty() => f.to_string(),
            _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_FIELD", "array_field is required")),
        };

        let item_var  = input.input["item_var"].as_str().unwrap_or("item").to_string();
        let index_var = input.input["index_var"].as_str().unwrap_or("index").to_string();

        // item_var/index_var are user-configurable and land as top-level
        // keys in the same json! object as the loop's own fixed keys below —
        // later keys silently overwrite earlier ones in serde_json::Map insert
        // order. A collision with "__loop_next_index" in particular corrupts
        // loop control flow (loop_executor.rs reads it back to advance the
        // iteration), not just the visible output. Reject before that can
        // happen instead of silently overwriting.
        // item_var's own natural key is "item" (its default, and the fixed
        // key the json! block below also writes); index_var's is "index".
        // Landing on that one key with the same value is a self-overwrite,
        // not a collision — only a var name resolving to a *different*
        // reserved key corrupts something else, so each var is exempted
        // from the one reserved key that is its own default.
        if (item_var != "item" && RESERVED_LOOP_OUTPUT_KEYS.contains(&item_var.as_str()))
            || (index_var != "index" && RESERVED_LOOP_OUTPUT_KEYS.contains(&index_var.as_str()))
        {
            return NodeOutput::failure(NodeError::unrecoverable(
                "RESERVED_VAR_NAME",
                format!(
                    "item_var/index_var cannot use a reserved name ({}). Choose a different name.",
                    RESERVED_LOOP_OUTPUT_KEYS.join(", ")
                ),
            ));
        }
        if item_var == index_var {
            return NodeOutput::failure(NodeError::unrecoverable(
                "RESERVED_VAR_NAME",
                "item_var and index_var must be different names.",
            ));
        }

        // T1-1i-followup: unify further — an explicit source_node string
        // that names no node in context used to silently resolve to
        // Value::Null, inconsistent with transform.rs's SOURCE_NOT_FOUND
        // for the identical shape. A typo'd or stale source_node should
        // fail loudly, not silently degrade to "no array, NOT_ARRAY".
        let source_node_value = &input.input["source_node"];
        let source_data: Value = if source_node_value.is_null() {
            return NodeOutput::failure(NodeError::unrecoverable(
                "SOURCE_NODE_REQUIRED",
                "source_node is required. Leaving it blank previously searched every upstream node's output for the first array match at array_field, which could iterate the wrong node's data whenever more than one predecessor exposed an array at the same field path. Set source_node to the specific node whose array you want to iterate.",
            ));
        } else if let Some(source_node) = source_node_value.as_str() {
            match input.context.node_outputs.get(source_node) {
                Some(v) => v.clone(),
                None => return NodeOutput::failure(NodeError::unrecoverable(
                    "SOURCE_NOT_FOUND",
                    format!("Node '{}' has no output in context", source_node),
                )),
            }
        } else {
            return NodeOutput::failure(NodeError::unrecoverable(
                "INVALID_SOURCE_NODE",
                format!(
                    "source_node is set to a non-string value ({}), which is not a valid node ID",
                    source_node_value
                ),
            ));
        };

        let array = traverse_dotpath(&source_data, &array_field);
        let items = match array.as_array() {
            Some(a) => a.clone(),
            None => return NodeOutput::failure(NodeError::unrecoverable(
                "NOT_ARRAY",
                format!("Field '{}' is not an array or was not found. Make sure the source node ran successfully and the field path is correct.", array_field),
            )),
        };

        let total = items.len();

        // MEDIUM fix: prevent O(n²) memory usage on large arrays.
        // 10,000 items is a generous cap for a desktop automation tool.
        if total > 10_000 {
            return NodeOutput::failure(NodeError::unrecoverable(
                "ARRAY_TOO_LARGE",
                format!("Array has {} items — maximum is 10,000 per loop. Split your data into smaller batches.", total),
            ));
        }

        if total == 0 {
            return NodeOutput::success_with_logs(
                json!({ "items": [], "total": 0, "all_results": [], "done": true, "item": null, "index": 0 }),
                vec!["Loop: empty array, nothing to iterate".to_string()],
            );
        }

        // Read the current iteration index from context metadata (loop_state is
        // merged into metadata by ExecutionState::snapshot(), keeping it out of
        // user-visible variables).
        let current_index = input.context.metadata
            .get(&format!("__loop_{}_index", input.node_id))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;

        if current_index >= total {
            // all_results is populated by the executor (loop_executor.rs) from its
            // dedicated loop_results store, which is excluded from snapshot() cloning.
            // Return [] as a placeholder — the executor overwrites it before returning
            // the done output to the caller.
            return NodeOutput::success_with_logs(
                json!({
                    "items": items,
                    "total": total,
                    "all_results": [],
                    "done": true,
                    "item": null,
                    "index": current_index
                }),
                vec![format!("Loop complete: processed {} items", total)],
            );
        }

        let current_item = items[current_index].clone();

        // Emit current item — the executor will activate loop_body successors.
        // MEDIUM fix: omit "items" from per-iteration output. Serializing the
        // full array on every iteration causes O(n²) memory usage. Downstream
        // nodes that need the full array should reference the loop node's
        // initial output from workflow context instead.
        NodeOutput::success_with_logs(
            json!({
                "total": total,
                "item": current_item,
                item_var: current_item,
                "index": current_index,
                index_var: current_index,
                "all_results": [],
                "done": false,
                "__loop_node_id": input.node_id,
                "__loop_next_index": current_index + 1,
                "__loop_total": total
            }),
            vec![format!("Loop: item {}/{}", current_index + 1, total)],
        )
    }
}

// ---------------------------------------------------------------------------
// Tests cover the source_node validation added in Batch A and unified to a
// hard requirement in Batch W (T1-1i) — not a full suite for pre-existing
// logic.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExecutionContext;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn make_input(input: Value, outputs: HashMap<String, Value>) -> NodeInput {
        NodeInput {
            node_id:      "test".to_string(),
            workflow_id:  "wf".to_string(),
            execution_id: "exec".to_string(),
            input,
            context: ExecutionContext {
                variables:    HashMap::new(),
                node_outputs: Arc::new(outputs),
                metadata:     HashMap::new(),
                ..Default::default()
            },
        }
    }

    #[tokio::test]
    async fn non_string_source_node_returns_error() {
        // Batch A / T1-1g pattern (unchanged by T1-1i): a bare number for
        // "source_node" is a malformed value, not "unset" — rejected with a
        // distinct code from the is_null() reject path (SOURCE_NODE_REQUIRED).
        let input = make_input(
            json!({ "array_field": "items", "source_node": 5 }),
            HashMap::new(),
        );
        let out = LoopNode.execute(input).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "INVALID_SOURCE_NODE");
    }

    #[tokio::test]
    async fn nonexistent_string_source_node_returns_error() {
        // T1-1i followup: a syntactically-valid string that names no node in
        // context now errors (SOURCE_NOT_FOUND), matching transform.rs.
        // Previously resolved to Value::Null and fell through to NOT_ARRAY,
        // silently masking a typo'd/stale source_node.
        let input = make_input(
            json!({ "array_field": "items", "source_node": "ghost" }),
            HashMap::new(),
        );
        let out = LoopNode.execute(input).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "SOURCE_NOT_FOUND");
    }

    #[tokio::test]
    async fn missing_source_node_key_is_rejected() {
        // T1-1i: unify to reject — the key omitted entirely (the shape the
        // canvas most commonly sends for an unconfigured field) must be
        // rejected. Previously this searched all upstream outputs and
        // succeeded.
        let mut outputs = HashMap::new();
        outputs.insert("n_a".to_string(), json!({ "items": [1, 2, 3] }));
        let input = make_input(
            json!({ "array_field": "items" }),
            outputs,
        );
        let out = LoopNode.execute(input).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "SOURCE_NODE_REQUIRED");
    }

    #[tokio::test]
    async fn explicit_null_source_node_is_rejected() {
        // T1-1i: explicit JSON null is treated the same as an absent key —
        // both are rejected now, unlike a non-string value which was
        // already rejected before this fix.
        let mut outputs = HashMap::new();
        outputs.insert("n_a".to_string(), json!({ "items": [1, 2, 3] }));
        let input = make_input(
            json!({ "array_field": "items", "source_node": null }),
            outputs,
        );
        let out = LoopNode.execute(input).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "SOURCE_NODE_REQUIRED");
    }

    #[tokio::test]
    async fn null_source_node_is_rejected_even_with_execution_order_present() {
        // T1-1i: the removed search fallback used to pick a deterministic
        // "first completed match" when execution_order was
        // populated and two upstream outputs both carried an "items" array.
        // That search is gone entirely — a null source_node must be
        // rejected regardless of whether a deterministic candidate would
        // have been resolvable, guarding against the search silently
        // creeping back in as a "helpful" fallback later.
        let mut outputs = HashMap::new();
        outputs.insert("z_second".to_string(), json!({ "items": [9, 9] }));
        outputs.insert("a_first".to_string(), json!({ "items": [1, 2, 3] }));
        let input = NodeInput {
            node_id: "test".to_string(),
            workflow_id: "wf".to_string(),
            execution_id: "exec".to_string(),
            input: json!({ "array_field": "items", "source_node": null }),
            context: ExecutionContext {
                variables: HashMap::new(),
                node_outputs: Arc::new(outputs),
                metadata: HashMap::new(),
                execution_order: Arc::new(vec!["a_first".to_string(), "z_second".to_string()]),
            },
        };
        let out = LoopNode.execute(input).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "SOURCE_NODE_REQUIRED");
    }

    /// S4-12 (edge case): naming item_var after the internal key
    /// loop_executor.rs reads to advance iteration must be rejected, not
    /// silently overwrite it.
    #[tokio::test]
    async fn item_var_colliding_with_loop_next_index_is_rejected() {
        let mut outputs = HashMap::new();
        outputs.insert("n_a".to_string(), json!({ "items": [1, 2, 3] }));
        let input = make_input(
            json!({
                "array_field": "items",
                "source_node": "n_a",
                "item_var": "__loop_next_index"
            }),
            outputs,
        );
        let out = LoopNode.execute(input).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "RESERVED_VAR_NAME");
    }

    /// S4-12 (edge case): index_var colliding with a reserved key is rejected
    /// the same way item_var is.
    #[tokio::test]
    async fn index_var_colliding_with_reserved_key_is_rejected() {
        let mut outputs = HashMap::new();
        outputs.insert("n_a".to_string(), json!({ "items": [1, 2, 3] }));
        let input = make_input(
            json!({
                "array_field": "items",
                "source_node": "n_a",
                "index_var": "done"
            }),
            outputs,
        );
        let out = LoopNode.execute(input).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "RESERVED_VAR_NAME");
    }

    /// S4-12 (edge case): item_var and index_var set to the same name collide
    /// with each other, not just with a fixed key.
    #[tokio::test]
    async fn item_var_equal_to_index_var_is_rejected() {
        let mut outputs = HashMap::new();
        outputs.insert("n_a".to_string(), json!({ "items": [1, 2, 3] }));
        let input = make_input(
            json!({
                "array_field": "items",
                "source_node": "n_a",
                "item_var": "x",
                "index_var": "x"
            }),
            outputs,
        );
        let out = LoopNode.execute(input).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "RESERVED_VAR_NAME");
    }

    /// S4-12 (normal case): distinct, non-reserved item_var/index_var names
    /// still work exactly as before and appear in the output under both
    /// their custom name and the fixed "item"/"index" keys.
    #[tokio::test]
    async fn distinct_non_reserved_vars_still_work() {
        let mut outputs = HashMap::new();
        outputs.insert("n_a".to_string(), json!({ "items": [10, 20, 30] }));
        let input = make_input(
            json!({
                "array_field": "items",
                "source_node": "n_a",
                "item_var": "row",
                "index_var": "row_num"
            }),
            outputs,
        );
        let out = LoopNode.execute(input).await;
        assert!(out.success, "expected success, got: {:?}", out.error);
        let data = out.output.unwrap();
        assert_eq!(data["item"], json!(10));
        assert_eq!(data["row"], json!(10));
        assert_eq!(data["index"], json!(0));
        assert_eq!(data["row_num"], json!(0));
    }

    /// Regression for the reserved-name false positive: item_var/index_var
    /// resolve to their own defaults ("item"/"index") when omitted, and
    /// "item"/"index" are themselves in RESERVED_LOOP_OUTPUT_KEYS. That must
    /// not be treated as a collision — the fixed "item"/"index" keys and the
    /// defaulted item_var/index_var keys are the same key holding the same
    /// value, not two different keys stomping each other.
    #[tokio::test]
    async fn default_item_index_vars_succeed_without_override() {
        let mut outputs = HashMap::new();
        outputs.insert("n_a".to_string(), json!({ "items": [10, 20, 30] }));
        let input = make_input(
            json!({ "array_field": "items", "source_node": "n_a" }),
            outputs,
        );
        let out = LoopNode.execute(input).await;
        assert!(out.success, "expected success, got: {:?}", out.error);
        let data = out.output.unwrap();
        assert_eq!(data["item"], json!(10));
        assert_eq!(data["index"], json!(0));
    }
}

