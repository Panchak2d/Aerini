//! Loop node execution for [`super::WorkflowExecutor`].

use std::collections::{HashMap, HashSet};


use crate::context::LogLevel;
use crate::model::Workflow;

use crate::context::SharedExecutionState;
use super::WorkflowExecutor;
use crate::model::NodeOutput;

impl WorkflowExecutor {
    // Collect all node IDs in the loop body — nodes reachable from the
    // loop node via loop_body edges, transitively — returned in topo order.
    // Nodes reachable via the loop node's "done" port are explicitly excluded so
    // post-loop nodes are never accidentally pulled into the body set.
    pub(super) fn collect_loop_body_nodes(
        &self,
        loop_node_id: &str,
        workflow:     &Workflow,
        topo_order:   &[String],
    ) -> Vec<String> {
        // Collect nodes reachable from the loop's "done" port so we can exclude them.
        let mut done_set: HashSet<String> = HashSet::new();
        let mut done_queue: Vec<String> = Vec::new();
        for edge in &workflow.edges {
            if edge.from_node == loop_node_id && edge.from_port == "done"
                && !done_set.contains(&edge.to_node) {
                    done_set.insert(edge.to_node.clone());
                    done_queue.push(edge.to_node.clone());
                }
        }
        let mut di = 0;
        while di < done_queue.len() {
            let current = done_queue[di].clone();
            di += 1;
            for edge in &workflow.edges {
                if edge.from_node == current && !done_set.contains(&edge.to_node) {
                    done_set.insert(edge.to_node.clone());
                    done_queue.push(edge.to_node.clone());
                }
            }
        }

        let mut body_set: HashSet<String> = HashSet::new();
        let mut queue: Vec<String> = Vec::new();

        // Seed: direct loop_body successors of the loop node.
        for edge in &workflow.edges {
            if edge.from_node == loop_node_id && edge.from_port == "loop_body"
                && !body_set.contains(&edge.to_node) && !done_set.contains(&edge.to_node) {
                    body_set.insert(edge.to_node.clone());
                    queue.push(edge.to_node.clone());
                }
        }

        // BFS: follow all outgoing edges from body nodes, skipping done-port nodes.
        let mut i = 0;
        while i < queue.len() {
            let current = queue[i].clone();
            i += 1;
            for edge in &workflow.edges {
                if edge.from_node == current
                    && !body_set.contains(&edge.to_node)
                    && !done_set.contains(&edge.to_node)
                {
                    body_set.insert(edge.to_node.clone());
                    queue.push(edge.to_node.clone());
                }
            }
        }

        // Return body nodes in their original topo order so execution is sequenced correctly.
        topo_order.iter()
            .filter(|id| body_set.contains(*id))
            .cloned()
            .collect()
    }

    // Execute a loop node and its body nodes for every iteration.
    // Returns the final "done: true" NodeOutput, or an error string if any
    // node in the loop fails.
    pub(super) async fn execute_loop_node(
        &self,
        loop_node_id: &str,
        loop_node_def: &crate::model::WorkflowNode,
        workflow:      &Workflow,
        topo_order:    &[String],
        state:         &SharedExecutionState,
    ) -> Result<NodeOutput, String> {
        // Safety net against executor re-entry bugs — should never be reached in
        // normal operation because LoopNode hard-caps arrays at 10,000 items and
        // routes to "done" once current_index >= total. Set to 10,001 so it sits
        // above the node's own guard and only fires on a real executor bug.
        const MAX_LOOP_ITERATIONS: u64 = 10_001;

        // Build a local O(1) lookup map. Built once per loop execution — not per iteration.
        // Replaces the O(n) Workflow::node() scan on every body node on every iteration.
        let node_map: HashMap<&str, &crate::model::WorkflowNode> =
            workflow.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

        let loop_node_impl = self
            .registry
            .get(&loop_node_def.node_type_id)
            .ok_or_else(|| format!(
                "Loop node type '{}' not registered", loop_node_def.node_type_id
            ))?;

        let body_nodes = self.collect_loop_body_nodes(loop_node_id, workflow, topo_order);

        state.write().await.mark_running(loop_node_id);
        self.emit_node_status(&workflow.id, loop_node_id, "running");

        for iteration in 0..=MAX_LOOP_ITERATIONS {
            // Cancel check — exits the loop immediately between iterations.
            if self.cancel_token.as_ref().map(|t| t.is_cancelled()).unwrap_or(false) {
                return Err("Run cancelled by user".to_string());
            }

            if iteration == MAX_LOOP_ITERATIONS {
                return Err(format!(
                    "Loop node '{}' exceeded maximum of {} iterations — \
                     possible infinite loop. Check that the array is finite and \
                     the loop index advances correctly.",
                    loop_node_id, MAX_LOOP_ITERATIONS
                ));
            }

            // Build input with current state (includes __loop_{id}_index variable
            // written at end of previous iteration, or absent on iteration 0).
            let loop_input = match self.build_input(&workflow, loop_node_def, state).await {
                Ok(input) => input,
                Err(failure) => {
                    let msg = failure.error.as_ref().map(|e| e.message.clone()).unwrap_or_else(|| "unknown error".to_string());
                    return Err(format!("Loop node '{}' build_input failed: {}", loop_node_id, msg));
                }
            };
            let loop_output = loop_node_impl.execute(loop_input).await;

            if !loop_output.success {
                let msg = loop_output
                    .error
                    .as_ref()
                    .map(|e| e.message.clone())
                    .unwrap_or_else(|| "unknown error".to_string());
                return Err(format!("Loop node '{}' failed on iteration {}: {}", loop_node_id, iteration, msg));
            }

            let done = loop_output
                .output
                .as_ref()
                .and_then(|v| v.get("done"))
                .and_then(|d| d.as_bool())
                .unwrap_or(false);

            // Write loop node's current-iteration output into state so body nodes
            // can reference {{LoopNodeName.output.item}} via the expression resolver.
            {
                let mut s = state.write().await;
                s.mark_succeeded(loop_node_id, loop_output.clone());
            }
            self.emit_node_status(&workflow.id, loop_node_id, if done { "success" } else { "running" });

            if done {
                // All items processed — return the summary output.
                return Ok(loop_output);
            }

            // Read the next index to set from the loop node's output field.
            let next_index = loop_output
                .output
                .as_ref()
                .and_then(|v| v.get("__loop_next_index"))
                .and_then(|v| v.as_u64())
                .unwrap_or(iteration + 1);

            // Execute each body node in topo order for this iteration.
            for body_id in &body_nodes {
                let body_def = match node_map.get(body_id.as_str()).copied() {
                    Some(n) => n,
                    None => return Err(format!(
                        "Loop body node '{}' not found in workflow — this is a bug", body_id
                    )),
                };

                // Honour disabled flag inside the loop body too.
                if body_def.disabled {
                    {
                        let mut s = state.write().await;
                        s.log(Some(body_id), LogLevel::Info, format!("Node '{}' is disabled — skipped", body_def.name));
                        s.mark_skipped(body_id);
                    }
                    self.emit_node_status(&workflow.id, body_id, "skipped");
                    continue;
                }

                let body_impl = self.registry.get(&body_def.node_type_id).ok_or_else(|| {
                    format!("Body node type '{}' not registered", body_def.node_type_id)
                })?;

                let body_input = match self.build_input(&workflow, body_def, state).await {
                    Ok(input) => input,
                    Err(failure) => {
                        let msg = failure.error.as_ref().map(|e| e.message.clone()).unwrap_or_else(|| "unknown error".to_string());
                        state.write().await.mark_failed(body_id, failure, 1);
                        self.emit_node_status(&workflow.id, body_id, "error");
                        return Err(format!("Loop body node '{}' build_input failed: {}", body_id, msg));
                    }
                };

                // Validate body node input. In strict mode, fail the loop iteration.
                // In non-strict mode, log a warning to the execution log and continue.
                let schema_errors = Self::validate_node_input(&body_def.input_schema, &body_input.input);
                if !schema_errors.is_empty() {
                    if self.strict_schema_validation {
                        let reason = schema_errors.join("; ");
                        let fail_output = crate::model::NodeOutput::failure(
                            crate::error::NodeError::unrecoverable(
                                "SCHEMA_VIOLATION",
                                format!("Input schema validation failed for loop body node '{}': {}", body_id, reason),
                            ),
                        );
                        state.write().await.mark_failed(body_id, fail_output, 1);
                        self.emit_node_status(&workflow.id, body_id, "error");
                        return Err(format!(
                            "Loop body node '{}' failed schema validation on iteration {}: {}",
                            body_id, iteration, reason
                        ));
                    } else {
                        let mut s = state.write().await;
                        for msg in &schema_errors {
                            s.log(
                                Some(body_id),
                                LogLevel::Warn,
                                format!("[Schema] node '{}': {}", body_id, msg),
                            );
                        }
                    }
                }

                state.write().await.mark_running(body_id);
                self.emit_node_status(&workflow.id, body_id, "running");

                let (body_output, _attempts) =
                    self.execute_with_retry(body_impl, body_input, body_def, state).await;

                if !body_output.success {
                    let msg = body_output
                        .error
                        .as_ref()
                        .map(|e| e.message.clone())
                        .unwrap_or_else(|| "unknown error".to_string());
                    state.write().await.mark_failed(body_id, body_output, _attempts);
                    self.emit_node_status(&workflow.id, body_id, "error");
                    return Err(format!(
                        "Loop body node '{}' failed on iteration {}: {}", body_id, iteration, msg
                    ));
                }

                // Store body result in loop_state (not user variables) so loop
                // indices and results cannot collide with user-defined variable names.
                {
                    let mut s = state.write().await;
                    if let Some(ref val) = body_output.output {
                        let result_key = format!("__loop_{}_result_{}", loop_node_id, iteration);
                        s.set_loop_var(result_key, val.clone());
                    }
                    s.mark_succeeded(body_id, body_output);
                }
                self.emit_node_status(&workflow.id, body_id, "success");
            }

            // Advance the loop index for the next iteration.
            let index_key = format!("__loop_{}_index", loop_node_id);
            state.write().await.set_loop_var(
                index_key,
                serde_json::Value::Number(serde_json::Number::from(next_index)),
            );
        }

        // Unreachable: the iteration == MAX_LOOP_ITERATIONS guard above returns Err first.
        Err(format!("Loop node '{}': iteration limit logic error — this is a bug", loop_node_id))
    }
}
