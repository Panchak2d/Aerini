use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortDefinition, PortPosition};
use super::util::{ordered_node_outputs, traverse_dotpath};

/// Loop node — iterates over an array, emitting one item at a time.
///
/// Because the executor runs nodes in topological order (not as a true runtime loop),
/// the loop node outputs ALL items and marks each one via the execution context.
/// The executor calls loop_body successors once per item by re-activating them.
///
/// Implementation note: the node outputs a cursor into the array. The executor
/// is responsible for re-running the loop body for each item.
pub struct LoopNode;

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
            "required": ["array_field"],
            "properties": {
                "array_field": {
                    "type": "string",
                    "description": "Dot-path to the array to iterate, e.g. 'items' or 'response.results'"
                },
                "source_node": {
                    "type": "string",
                    "description": "Node ID to read the array from. Leave blank to search all previous outputs."
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

        // Batch A: source_node is schema-typed as a string (line 37), so a
        // non-string, non-null value is a malformed config, not "unset" — it
        // must not silently fall through to the search-all-outputs fallback
        // below, which is reserved for a genuinely absent/null source_node.
        // Mirrors switch.rs's T1-1g is_null/as_str/reject pattern.
        let source_node_value = &input.input["source_node"];
        let source_data: Value = if source_node_value.is_null() {
            // Search all node outputs for the array field. T2-5 / S4-7:
            // search in real completion order (via ordered_node_outputs),
            // not the raw HashMap's unspecified order — previously, which
            // upstream node "won" when two shared an array-valued field at
            // this path was non-deterministic across runs.
            let mut found = Value::Null;
            for (_, output_val) in ordered_node_outputs(&input.context) {
                let candidate = traverse_dotpath(&output_val, &array_field);
                if candidate.is_array() {
                    found = output_val;
                    break;
                }
            }
            found
        } else if let Some(source_node) = source_node_value.as_str() {
            input.context.node_outputs.get(source_node).cloned().unwrap_or(Value::Null)
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
// Tests — Batch A only: covers the source_node validation this batch added.
// No test module existed in this file before this batch; scope is limited
// to what this fix changes (Rule 7), not a full suite for pre-existing logic.
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
        // Batch A / T1-1g pattern: a bare number for "source_node" must not
        // silently fall through to the search-all-outputs fallback the way
        // an absent key correctly does.
        let input = make_input(
            json!({ "array_field": "items", "source_node": 5 }),
            HashMap::new(),
        );
        let out = LoopNode.execute(input).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "INVALID_SOURCE_NODE");
    }

    #[tokio::test]
    async fn explicit_null_source_node_still_searches_all_outputs() {
        // Explicit JSON null is treated the same as an absent key (both are
        // "genuinely unset"), unlike a non-string value — unchanged from
        // pre-batch behavior.
        let mut outputs = HashMap::new();
        outputs.insert("n_a".to_string(), json!({ "items": [1, 2, 3] }));
        let input = make_input(
            json!({ "array_field": "items", "source_node": null }),
            outputs,
        );
        let out = LoopNode.execute(input).await;
        assert!(out.success);
        assert_eq!(out.output.unwrap()["total"], json!(3));
    }

    #[tokio::test]
    async fn null_source_node_search_deterministically_picks_first_completed_match() {
        // T2-5 / S4-7: two upstream nodes both carry an "items" array —
        // resolution must be deterministic (first in execution_order), not
        // whichever the raw HashMap happened to enumerate first.
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
        assert!(out.success);
        assert_eq!(out.output.unwrap()["total"], json!(3));
    }
}

