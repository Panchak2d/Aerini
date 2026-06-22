//! Sequential execution path for [`super::WorkflowExecutor`].

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::Value;

use crate::context::{new_shared_state, LogLevel};
use crate::error::EngineError;
use crate::graph::ExecutionGraph;
use crate::model::{NodeOutput, Workflow};

use super::{WorkflowExecutor, WorkflowResult};
use super::parallel::run_inner_parallel;

impl WorkflowExecutor {
    pub(super) async fn run_inner(
        &self,
        workflow: Arc<Workflow>,
        initial_variables: HashMap<String, Value>,
    ) -> Result<WorkflowResult, EngineError> {
        if self.parallel_execution {
            let executor_arc = Arc::new(self.clone());
            return run_inner_parallel(executor_arc, workflow, initial_variables).await;
        }

        let graph = ExecutionGraph::build(&workflow)?;
        let state = new_shared_state(&workflow.id, initial_variables);

        // Accumulates schema validation warnings (warn mode) or errors (strict mode).
        let mut validation_warnings: Vec<String> = Vec::new();

        // O(1) node lookup for the hot path. The existing Workflow::node() method
        // is kept for Tauri / test callers outside the executor.
        let node_map: HashMap<&str, &crate::model::WorkflowNode> =
            workflow.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

        {
            let mut s = state.write().await;
            s.log(None, LogLevel::Info, format!("Workflow '{}' started", workflow.name));
        }

        let mut active_nodes: HashSet<String> = graph.entry_nodes.iter().cloned().collect();

        {
            const TRIGGER_TYPES: &[&str] = &["manual_trigger", "schedule", "webhook"];
            let mut s = state.write().await;
            for entry_id in &graph.entry_nodes {
                if let Some(node_def) = node_map.get(entry_id.as_str()).copied() {
                    if !TRIGGER_TYPES.contains(&node_def.node_type_id.as_str()) {
                        s.log(
                            Some(entry_id),
                            LogLevel::Warn,
                            format!(
                                "Node '{}' (type: '{}') has no incoming connections and will \
                                 execute as an entry point. This is likely a disconnected node \
                                 — connect it or remove it from the canvas.",
                                node_def.name, node_def.node_type_id
                            ),
                        );
                    }
                }
            }
        }

        for node_id in &graph.topo_order {
            // Cancel check — runs between every node; skips all remaining active nodes on cancel.
            if self.cancel_token.as_ref().map(|t| t.is_cancelled()).unwrap_or(false) {
                {
                    let mut s = state.write().await;
                    for id in &active_nodes { s.mark_skipped(id); }
                }
                for id in &active_nodes {
                    self.emit_node_status(&workflow.id, id, "skipped");
                }
                return Err(EngineError::ExecutionCancelled);
            }

            if !active_nodes.contains(node_id) {
                state.write().await.mark_skipped(node_id);
                self.emit_node_status(&workflow.id, node_id, "skipped");
                continue;
            }

            let node_def = match node_map.get(node_id.as_str()).copied() {
                Some(n) => n,
                None => {
                    let s = state.read().await;
                    return Ok(WorkflowResult {
                        execution_id: s.execution_id.clone(),
                        workflow_id:  workflow.id.clone(),
                        success:      false,
                        node_outputs: (*s.snapshot().node_outputs).clone(),
                        logs:         s.logs.clone(),
                        error: Some(format!(
                            "Internal error: node '{}' in topo_order but not in workflow nodes \
                             — this is a bug, please report it",
                            node_id
                        )),
                        validation_errors: validation_warnings.clone(),
                    });
                }
            };

            // Disabled node: skip execution, pass output to successors.
            if node_def.disabled {
                {
                    let mut s = state.write().await;
                    s.log(Some(node_id), LogLevel::Info, format!("Node '{}' is disabled — skipped", node_def.name));
                    s.mark_skipped(node_id);
                }
                self.emit_node_status(&workflow.id, node_id, "skipped");
                // Activate output successors so the chain continues through the disabled node.
                let (_, drop_warn) = self.activate_successors(node_id, "output", &workflow, &mut active_nodes);
                if let Some(msg) = drop_warn {
                    if node_def.node_type_id != "output" {
                        let user_msg = msg.replacen(node_id, &node_def.name, 1);
                        state.write().await.log(Some(node_id), LogLevel::Error, user_msg);
                    }
                }
                continue;
            }

            // Loop node: run the full iteration inline so body nodes
            // are not re-visited by the outer topo loop.
            if node_def.node_type_id == "loop" {
                let loop_result = self
                    .execute_loop_node(node_id, node_def, &workflow, &graph.topo_order, &state)
                    .await;
                match loop_result {
                    Ok(done_output) => {
                        state.write().await.mark_succeeded(node_id, done_output);
                        self.emit_node_status(&workflow.id, node_id, "success");
                        // Activate only the "done" port successors; body nodes were
                        // already executed inside execute_loop_node.
                        let (_, drop_warn) = self.activate_successors(node_id, "done", &workflow, &mut active_nodes);
                        if let Some(msg) = drop_warn {
                            if node_def.node_type_id != "output" {
                                let user_msg = msg.replacen(node_id, &node_def.name, 1);
                                state.write().await.log(Some(node_id), LogLevel::Error, user_msg);
                            }
                        }
                        // Mark all body nodes as handled so the outer loop skips them.
                        let body_node_ids =
                            self.collect_loop_body_nodes(node_id, &workflow, &graph.topo_order);
                        for body_id in &body_node_ids {
                            active_nodes.remove(body_id);
                        }
                    }
                    Err(err_msg) => {
                        // If a loop node was cancelled, propagate as ExecutionCancelled
                        // rather than treating as a workflow failure.
                        if self.cancel_token.as_ref().map(|t| t.is_cancelled()).unwrap_or(false) {
                            return Err(EngineError::ExecutionCancelled);
                        }
                        let fail_output = NodeOutput::failure(
                            crate::error::NodeError::unrecoverable("LOOP_ERROR", err_msg.clone()),
                        );
                        state.write().await.mark_failed(node_id, fail_output, 1);
                        self.emit_node_status(&workflow.id, node_id, "error");

                        let before = active_nodes.len();
                        let (_, _) = self.activate_successors(node_id, "on_error", &workflow, &mut active_nodes);
                        let on_error_wired = active_nodes.len() > before;

                        if !on_error_wired {
                            let failure_route = self.find_failure_route(node_id, &graph);
                            if let Some(ref failure_node_id) = failure_route {
                                active_nodes.insert(failure_node_id.clone());
                            } else {
                                let s = state.read().await;
                                return Ok(WorkflowResult {
                                    execution_id: s.execution_id.clone(),
                                    workflow_id:  workflow.id.clone(),
                                    success:      false,
                                    node_outputs: (*s.snapshot().node_outputs).clone(),
                                    logs:         s.logs.clone(),
                                    error: Some(format!("Node '{}' failed: {}", node_id, err_msg)),
                                    validation_errors: validation_warnings.clone(),
                                });
                            }
                        }
                    }
                }
                continue;
            }

            let node_impl = self.registry.get(&node_def.node_type_id).ok_or_else(|| {
                EngineError::NodeTypeNotRegistered { type_id: node_def.node_type_id.clone() }
            })?;

            let resolved_input = match self.build_input(&workflow, node_def, &state).await {
                Ok(input) => input,
                Err(failure) => {
                    let err_msg = failure.error.as_ref().map(|e| e.message.clone()).unwrap_or_default();
                    state.write().await.mark_failed(node_id, failure, 1);
                    self.emit_node_status(&workflow.id, node_id, "error");
                    let before = active_nodes.len();
                    let (_, _) = self.activate_successors(node_id, "on_error", &workflow, &mut active_nodes);
                    if active_nodes.len() == before {
                        let failure_route = self.find_failure_route(node_id, &graph);
                        if let Some(ref fid) = failure_route {
                            active_nodes.insert(fid.clone());
                        } else {
                            let s = state.read().await;
                            return Ok(WorkflowResult {
                                execution_id: s.execution_id.clone(),
                                workflow_id:  workflow.id.clone(),
                                success:      false,
                                node_outputs: (*s.snapshot().node_outputs).clone(),
                                logs:         s.logs.clone(),
                                error:        Some(err_msg),
                                validation_errors: validation_warnings.clone(),
                            });
                        }
                    }
                    continue;
                }
            };

            // Validate resolved input against the node's declared input schema.
            // In non-strict mode: logs warnings and continues.
            // In strict mode: returns Some(failure) → treated identically to a node execution failure.
            if let Some(validation_failure) = self
                .check_schema(&node_def.input_schema, &resolved_input.input, node_id, &state, &mut validation_warnings)
                .await
            {
                let err_msg = validation_failure
                    .error
                    .as_ref()
                    .map(|e| e.message.clone())
                    .unwrap_or_default();

                state.write().await.mark_failed(node_id, validation_failure, 1);
                self.emit_node_status(&workflow.id, node_id, "error");

                let before = active_nodes.len();
                let (_, _) = self.activate_successors(node_id, "on_error", &workflow, &mut active_nodes);
                let on_error_wired = active_nodes.len() > before;

                if !on_error_wired {
                    let failure_route = self.find_failure_route(node_id, &graph);
                    if let Some(ref failure_node_id) = failure_route {
                        active_nodes.insert(failure_node_id.clone());
                    } else {
                        let s = state.read().await;
                        return Ok(WorkflowResult {
                            execution_id: s.execution_id.clone(),
                            workflow_id:  workflow.id.clone(),
                            success:      false,
                            node_outputs: (*s.snapshot().node_outputs).clone(),
                            logs:         s.logs.clone(),
                            error:        Some(err_msg),
                            validation_errors: validation_warnings.clone(),
                        });
                    }
                }
                continue;
            }

            state.write().await.mark_running(node_id);
            self.emit_node_status(&workflow.id, node_id, "running");

            let (output, actual_attempts) = self
                .execute_with_retry(node_impl, resolved_input, node_def, &state)
                .await;

            if output.success {
                let taken_port = Self::resolve_taken_port(&output);

                {
                    let mut s = state.write().await;
                    if let Some(ref val) = output.output {
                        let key = val["_variable_key"].as_str().unwrap_or("").trim();
                        if !key.is_empty() {
                            if let Some(v) = val.get("_variable_value") {
                                s.set_variable(key.to_string(), v.clone());
                            }
                        }
                    }
                    s.mark_succeeded(node_id, output);
                }
                self.emit_node_status(&workflow.id, node_id, "success");

                let (fallback_fired, drop_warn) = self.activate_successors(
                    node_id, &taken_port, &workflow, &mut active_nodes,
                );
                if drop_warn.is_some() || (fallback_fired && taken_port != "output") {
                    let mut s = state.write().await;
                    if let Some(msg) = drop_warn {
                        if node_def.node_type_id != "output" {
                            s.log(Some(node_id), LogLevel::Error, msg.replacen(node_id, &node_def.name, 1));
                        }
                    }
                    if fallback_fired && taken_port != "output" {
                        s.log(
                            Some(node_id),
                            LogLevel::Warn,
                            format!(
                                "Node '{}': no edge found on port '{}' — fell back to 'output' \
                                 port routing. Connect the '{}' port explicitly to suppress this.",
                                node_def.name, taken_port, taken_port
                            ),
                        );
                    }
                }
            } else {
                let err_msg = output
                    .error
                    .as_ref()
                    .map(|e| e.message.clone())
                    .unwrap_or_default();

                state.write().await.mark_failed(node_id, output, actual_attempts);
                self.emit_node_status(&workflow.id, node_id, "error");

                let before = active_nodes.len();
                let (_, _) = self.activate_successors(node_id, "on_error", &workflow, &mut active_nodes);
                let on_error_wired = active_nodes.len() > before;

                if !on_error_wired {
                    let failure_route = self.find_failure_route(node_id, &graph);
                    if let Some(ref failure_node_id) = failure_route {
                        active_nodes.insert(failure_node_id.clone());
                    } else {
                        let s = state.read().await;
                        return Ok(WorkflowResult {
                            execution_id: s.execution_id.clone(),
                            workflow_id:  workflow.id.clone(),
                            success:      false,
                            node_outputs: (*s.snapshot().node_outputs).clone(),
                            logs:         s.logs.clone(),
                            error: Some(format!("Node '{}' failed: {}", node_id, err_msg)),
                            validation_errors: validation_warnings.clone(),
                        });
                    }
                }
            }
        }

        let s = state.read().await;
        Ok(WorkflowResult {
            execution_id: s.execution_id.clone(),
            workflow_id:  workflow.id.clone(),
            success:      true,
            node_outputs: (*s.snapshot().node_outputs).clone(),
            logs:         s.logs.clone(),
            error:        None,
            validation_errors: validation_warnings,
        })
    }
}
