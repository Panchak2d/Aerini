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

#[async_trait]
impl Node for LoopNode {
    fn type_id(&self) -> &'static str { "loop" }
    fn display_name(&self) -> &'static str { "Loop (For Each)" }
    fn node_type(&self) -> NodeType { NodeType::Logic }
    fn version(&self) -> &'static str { "2.0.0" }

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
                id: "input".to_string(), label: "In".to_string(), position: PortPosition::Left,
            }],
            outputs: vec![
                PortDefinition { id: "loop_body".to_string(), label: "Each Item".to_string(), position: PortPosition::Right },
                PortDefinition { id: "done".to_string(),      label: "Done".to_string(),      position: PortPosition::Right },
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

        // Resolve source data
        let source_data: Value = if let Some(source_node) = input.input["source_node"].as_str() {
            input.context.node_outputs.get(source_node).cloned().unwrap_or(Value::Null)
        } else {
            // Search all node outputs for the array field
            let mut found = Value::Null;
            for output_val in input.context.node_outputs.values() {
                let candidate = traverse_dotpath(output_val, &array_field);
                if candidate.is_array() {
                    found = output_val.clone();
                    break;
                }
            }
            found
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
            // All items processed — route to "done"
            let all_results: Vec<Value> = (0..total)
                .filter_map(|i| {
                    input.context.metadata
                        .get(&format!("__loop_{}_result_{}", input.node_id, i))
                        .cloned()
                })
                .collect();

            return NodeOutput::success_with_logs(
                json!({
                    "items": items,
                    "total": total,
                    "all_results": all_results,
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
                item_var.clone(): current_item,
                "index": current_index,
                index_var.clone(): current_index,
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

