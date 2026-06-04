//! Parallel execution path for [`super::WorkflowExecutor`].

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::Value;

use crate::context::{new_shared_state, LogLevel};
use crate::error::{EngineError, NodeError};
use crate::graph::ExecutionGraph;
use crate::model::{NodeOutput, Workflow};

use super::{WorkflowExecutor, WorkflowResult};

// ── Parallel execution types ──────────────────────────────────────────────────

pub(super) enum NodeOutcome {
    Succeeded {
        output:         NodeOutput,
        taken_port:     String,
        variable_key:   Option<String>,
        variable_value: Option<Value>,
    },
    Failed {
        output:   NodeOutput,
        attempts: u32,
        err_msg:  String,
    },
    LoopCompleted { output: NodeOutput },
    LoopFailed    { err_msg: String },
    SchemaFailed  { failure: NodeOutput, err_msg: String },
    Unregistered  { err_msg: String },
}

pub(super) struct NodeTaskResult {
    pub(super) node_id:         String,
    pub(super) outcome:         NodeOutcome,
    /// Body node IDs managed by a loop task. Empty for non-loop tasks.
    pub(super) loop_body_nodes: Vec<String>,
}

// ── Parallel executor ─────────────────────────────────────────────────────────

/// Shared failure-routing logic for the parallel executor.
///
/// After a node fails, activates the `on_error` port, falls back to
/// `find_failure_route`, and returns `(should_abort, Option<failure_result>)`
/// for the caller to apply. Extracted to eliminate the four identical routing
/// blocks that previously appeared in each failure-outcome arm.
async fn parallel_route_failure(
    node_id:      &str,
    err_msg:      String,
    executor:     &WorkflowExecutor,
    workflow:     &Workflow,
    graph:        &ExecutionGraph,
    active_nodes: &mut HashSet<String>,
    state:        &crate::context::SharedExecutionState,
) -> (bool, Option<WorkflowResult>) {
    let before = active_nodes.len();
    let (_, drop_warn) = executor.activate_successors(node_id, "on_error", workflow, active_nodes);
    let on_error_wired = active_nodes.len() > before;
    if let Some(msg) = drop_warn {
        state.write().await.log(Some(node_id), crate::context::LogLevel::Error, msg);
    }

    if on_error_wired {
        return (false, None);
    }

    if let Some(fallback_id) = executor.find_failure_route(node_id, graph) {
        active_nodes.insert(fallback_id);
        return (false, None);
    }

    let s = state.read().await;
    (true, Some(WorkflowResult {
        execution_id:      s.execution_id.clone(),
        workflow_id:       workflow.id.clone(),
        success:           false,
        node_outputs:      HashMap::new(),
        logs:              s.logs.clone(),
        error:             Some(err_msg),
        validation_errors: vec![],
    }))
}

/// Parallel implementation of [`WorkflowExecutor::run_inner`].
///
/// Runs whenever `WorkflowExecutor.parallel_execution == true`.
/// Independent branches (nodes whose predecessors have all completed) are
/// spawned concurrently as tokio tasks, bounded by `max_concurrent_nodes`.
///
/// The sequential path in `run_inner` is untouched.
pub(super) async fn run_inner_parallel(
    executor:          Arc<WorkflowExecutor>,
    workflow:          Arc<Workflow>,
    initial_variables: HashMap<String, Value>,
) -> Result<WorkflowResult, EngineError> {
    let graph = ExecutionGraph::build(&workflow)?;
    let state = new_shared_state(&workflow.id, initial_variables);

    // Shared accumulator for non-strict schema validation warnings from parallel tasks.
    // Tasks push into this instead of the (unshareable) per-function Vec used in sequential mode.
    let validation_acc: Arc<std::sync::Mutex<Vec<String>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));

    // O(1) node lookup.
    let node_map: HashMap<&str, &crate::model::WorkflowNode> =
        workflow.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

    // predecessor_map[N] = set of node IDs with edges pointing TO N.
    // A node is "ready" when all its predecessors are in `completed`.
    let predecessor_map: HashMap<String, HashSet<String>> = workflow.nodes.iter()
        .map(|n| {
            let preds: HashSet<String> = graph.predecessors(&n.id).into_iter().collect();
            (n.id.clone(), preds)
        })
        .collect();

    {
        let mut s = state.write().await;
        s.log(None, LogLevel::Info,
            format!("Workflow '{}' started (parallel mode)", workflow.name));
    }

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

    let semaphore = Arc::new(tokio::sync::Semaphore::new(executor.max_concurrent_nodes));

    let mut active_nodes: HashSet<String> = graph.entry_nodes.iter().cloned().collect();
    let mut completed:    HashSet<String> = HashSet::new();
    let mut in_flight:    HashSet<String> = HashSet::new();
    // Nodes owned by a loop — never independently scheduled by the parallel executor.
    let mut loop_managed: HashSet<String> = HashSet::new();
    let mut join_set: tokio::task::JoinSet<NodeTaskResult> = tokio::task::JoinSet::new();
    let mut abort = false;
    let mut failure_result: Option<WorkflowResult> = None;

    loop {
        // Cancel check — highest priority; drains in-flight tasks and returns Err.
        // Runs before the abort check so a cancelled loop node (which sets abort=true)
        // is correctly surfaced as ExecutionCancelled, not as a workflow failure.
        if executor.cancel_token.as_ref().map(|t| t.is_cancelled()).unwrap_or(false) {
            for id in active_nodes.iter() {
                if !in_flight.contains(id) && !completed.contains(id) {
                    executor.emit_node_status(&workflow.id, id, "skipped");
                }
            }
            while join_set.join_next().await.is_some() {}
            return Err(EngineError::ExecutionCancelled);
        }

        // ── 1. Disabled nodes: handle synchronously (no task spawn needed). ───
        let disabled_ready: Vec<String> = active_nodes.iter()
            .filter(|id| {
                !in_flight.contains(*id)
                    && !completed.contains(*id)
                    && !loop_managed.contains(*id)
                    && node_map.get(id.as_str()).map(|n| n.disabled).unwrap_or(false)
                    && predecessor_map.get(*id)
                        .map(|preds| preds.iter().all(|p| completed.contains(p)))
                        .unwrap_or(true)
            })
            .cloned()
            .collect();

        for node_id in disabled_ready {
            let node_def = match node_map.get(node_id.as_str()).copied() {
                Some(n) => n,
                None    => { completed.insert(node_id); continue; }
            };
            {
                let mut s = state.write().await;
                s.log(Some(&node_id), LogLevel::Info,
                    format!("Node '{}' is disabled — skipped", node_def.name));
                s.mark_skipped(&node_id);
            }
            executor.emit_node_status(&workflow.id, &node_id, "skipped");
            let (_, drop_warn) = executor.activate_successors(
                &node_id, "output", &workflow, &mut active_nodes,
            );
            if let Some(msg) = drop_warn {
                if node_def.node_type_id != "output" {
                    let user_msg = msg.replacen(&node_id, &node_def.name, 1);
                    state.write().await.log(Some(&node_id), LogLevel::Error, user_msg);
                }
            }
            completed.insert(node_id);
        }

        // ── 2. Find ready executable nodes. ──────────────────────────────────
        let ready: Vec<String> = active_nodes.iter()
            .filter(|id| {
                !in_flight.contains(*id)
                    && !completed.contains(*id)
                    && !loop_managed.contains(*id)
                    && !node_map.get(id.as_str()).map(|n| n.disabled).unwrap_or(false)
                    && predecessor_map.get(*id)
                        .map(|preds| preds.iter().all(|p| completed.contains(p)))
                        .unwrap_or(true)
            })
            .cloned()
            .collect();

        if ready.is_empty() && join_set.is_empty() {
            break; // All done — nothing running, nothing left to do.
        }

        if abort && join_set.is_empty() {
            break; // Failure captured; in-flight tasks drained.
        }

        // ── 3. Spawn all ready nodes (skip if aborting). ─────────────────────
        if !abort {
            for node_id in ready {
                let node_def = match node_map.get(node_id.as_str()).copied() {
                    Some(n) => n,
                    None    => continue,
                };

                in_flight.insert(node_id.clone());

                // Loop node: collect body nodes before spawning so the parallel
                // scheduler never independently schedules them.
                if node_def.node_type_id == "loop" {
                    let body_nodes =
                        executor.collect_loop_body_nodes(&node_id, &workflow, &graph.topo_order);
                    for id in &body_nodes {
                        loop_managed.insert(id.clone());
                    }

                    let nid   = node_id.clone();
                    let exec  = Arc::clone(&executor);
                    let wf    = Arc::clone(&workflow);
                    let st    = Arc::clone(&state);
                    let ndef  = node_def.clone();
                    let topo  = graph.topo_order.clone();
                    let permit = Arc::clone(&semaphore)
                        .acquire_owned().await
                        .expect("semaphore is never closed — this is a bug");

                    join_set.spawn(async move {
                        let _permit = permit;
                        let result = exec
                            .execute_loop_node(&nid, &ndef, &wf, &topo, &st)
                            .await;
                        match result {
                            Ok(output) => NodeTaskResult {
                                node_id:         nid,
                                outcome:         NodeOutcome::LoopCompleted { output },
                                loop_body_nodes: body_nodes,
                            },
                            Err(err_msg) => NodeTaskResult {
                                node_id:         nid,
                                outcome:         NodeOutcome::LoopFailed { err_msg },
                                loop_body_nodes: body_nodes,
                            },
                        }
                    });
                    continue;
                }

                // Normal node.
                let node_impl = match executor.registry.get(&node_def.node_type_id) {
                    Some(n) => n,
                    None    => {
                        let nid = node_id.clone();
                        let type_id = node_def.node_type_id.clone();
                        join_set.spawn(async move {
                            NodeTaskResult {
                                node_id:         nid,
                                outcome:         NodeOutcome::Unregistered {
                                    err_msg: format!(
                                        "Node type '{}' is not registered", type_id
                                    ),
                                },
                                loop_body_nodes: vec![],
                            }
                        });
                        continue;
                    }
                };

                let nid    = node_id.clone();
                let exec   = Arc::clone(&executor);
                let wf     = Arc::clone(&workflow);
                let st     = Arc::clone(&state);
                let ndef   = node_def.clone();
                let vacc   = Arc::clone(&validation_acc);
                let permit = Arc::clone(&semaphore)
                    .acquire_owned().await
                    .expect("semaphore is never closed — this is a bug");

                join_set.spawn(async move {
                    let _permit = permit;

                    let resolved_input = match exec.build_input(&wf, &ndef, &st).await {
                        Ok(input) => input,
                        Err(failure) => {
                            let err_msg = failure.error.as_ref().map(|e| e.message.clone()).unwrap_or_default();
                            return NodeTaskResult {
                                node_id:         nid,
                                outcome:         NodeOutcome::Failed { output: failure, attempts: 1, err_msg },
                                loop_body_nodes: vec![],
                            };
                        }
                    };

                    // Schema validation: strict → fail; non-strict → warn and accumulate.
                    let schema_errors = WorkflowExecutor::validate_node_input(
                        &ndef.input_schema,
                        &resolved_input.input,
                    );
                    if !schema_errors.is_empty() {
                        if exec.strict_schema_validation {
                            let reason  = schema_errors.join("; ");
                            let err_msg = format!(
                                "Input schema validation failed for node '{}': {}",
                                nid, reason
                            );
                            let failure = NodeOutput::failure(
                                NodeError::unrecoverable("SCHEMA_VIOLATION", err_msg.clone())
                            );
                            return NodeTaskResult {
                                node_id:         nid,
                                outcome:         NodeOutcome::SchemaFailed { failure, err_msg },
                                loop_body_nodes: vec![],
                            };
                        } else {
                            // Collect messages without holding two locks simultaneously.
                            // Holding a tokio async write lock while blocking on a sync
                            // mutex would stall the runtime if vacc is ever contended.
                            let warnings: Vec<String> = {
                                let mut s = st.write().await;
                                schema_errors.iter().map(|msg| {
                                    let w = format!("[Schema] node '{}': {}", nid, msg);
                                    s.log(Some(&nid), LogLevel::Warn, w.clone());
                                    w
                                }).collect()
                            }; // st write lock released here
                            vacc.lock()
                                .expect("validation_acc mutex poisoned")
                                .extend(warnings);
                        }
                    }

                    st.write().await.mark_running(&nid);
                    exec.emit_node_status(&wf.id, &nid, "running");

                    let (output, attempts) = exec
                        .execute_with_retry(node_impl, resolved_input, &ndef, &st)
                        .await;

                    if output.success {
                        let taken_port = WorkflowExecutor::resolve_taken_port(&output);
                        let variable_key = output.output.as_ref()
                            .and_then(|v| v["_variable_key"].as_str())
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty());
                        let variable_value = variable_key.as_ref()
                            .and(output.output.as_ref())
                            .and_then(|v| v.get("_variable_value").cloned());
                        NodeTaskResult {
                            node_id:         nid,
                            outcome:         NodeOutcome::Succeeded {
                                output, taken_port, variable_key, variable_value,
                            },
                            loop_body_nodes: vec![],
                        }
                    } else {
                        let err_msg = output.error.as_ref()
                            .map(|e| e.message.clone())
                            .unwrap_or_default();
                        NodeTaskResult {
                            node_id:         nid,
                            outcome:         NodeOutcome::Failed { output, attempts, err_msg },
                            loop_body_nodes: vec![],
                        }
                    }
                });
            }
        }

        // ── 4. Wait for one task to finish. ───────────────────────────────────
        if join_set.is_empty() {
            // Nothing in flight and nothing ready (both guards above passed) — done.
            break;
        }

        let task_result = match join_set.join_next().await {
            Some(Ok(r))  => r,
            Some(Err(e)) => {
                // A spawned task panicked. This is an internal bug — surface it.
                return Err(EngineError::Internal(
                    format!("Parallel executor task panicked: {}", e)
                ));
            }
            None => break,
        };

        let node_id         = task_result.node_id;
        let loop_body_nodes = task_result.loop_body_nodes;
        in_flight.remove(&node_id);
        completed.insert(node_id.clone());

        // ── 5. Process result and update routing state. ───────────────────────
        match task_result.outcome {
            NodeOutcome::Succeeded { output, taken_port, variable_key, variable_value } => {
                if let (Some(key), Some(val)) = (variable_key, variable_value) {
                    state.write().await.set_variable(key, val);
                }
                state.write().await.mark_succeeded(&node_id, output);
                executor.emit_node_status(&workflow.id, &node_id, "success");

                let (fallback_fired, drop_warn) = executor.activate_successors(
                    &node_id, &taken_port, &workflow, &mut active_nodes,
                );
                if let Some(msg) = drop_warn {
                    let node_type_id = node_map.get(node_id.as_str()).map(|n| n.node_type_id.as_str()).unwrap_or("");
                    if node_type_id != "output" {
                        let node_name = node_map.get(node_id.as_str()).map(|n| n.name.as_str()).unwrap_or(node_id.as_str());
                        let user_msg = msg.replacen(&node_id, node_name, 1);
                        state.write().await.log(Some(&node_id), LogLevel::Error, user_msg);
                    }
                }
                if fallback_fired && taken_port != "output" {
                    let node_name = node_map.get(node_id.as_str()).map(|n| n.name.as_str()).unwrap_or(node_id.as_str());
                    state.write().await.log(
                        Some(&node_id),
                        LogLevel::Warn,
                        format!(
                            "Node '{}': no edge found on port '{}' — fell back to 'output' \
                             port routing. Connect the '{}' port explicitly to suppress this.",
                            node_name, taken_port, taken_port
                        ),
                    );
                }
            }

            NodeOutcome::Failed { output, attempts, err_msg } => {
                state.write().await.mark_failed(&node_id, output, attempts);
                executor.emit_node_status(&workflow.id, &node_id, "error");

                let (should_abort, result) = parallel_route_failure(
                    &node_id, format!("Node '{}' failed: {}", node_id, err_msg),
                    &executor, &workflow, &graph, &mut active_nodes, &state,
                ).await;
                if should_abort { abort = true; failure_result = result; }
            }

            NodeOutcome::SchemaFailed { failure, err_msg } => {
                state.write().await.mark_failed(&node_id, failure, 1);
                executor.emit_node_status(&workflow.id, &node_id, "error");

                let (should_abort, result) = parallel_route_failure(
                    &node_id, err_msg,
                    &executor, &workflow, &graph, &mut active_nodes, &state,
                ).await;
                if should_abort { abort = true; failure_result = result; }
            }

            NodeOutcome::Unregistered { err_msg } => {
                let failure = NodeOutput::failure(
                    NodeError::unrecoverable("NODE_TYPE_NOT_REGISTERED", err_msg.clone())
                );
                state.write().await.mark_failed(&node_id, failure, 1);
                executor.emit_node_status(&workflow.id, &node_id, "error");

                let (should_abort, result) = parallel_route_failure(
                    &node_id, err_msg,
                    &executor, &workflow, &graph, &mut active_nodes, &state,
                ).await;
                if should_abort { abort = true; failure_result = result; }
            }

            NodeOutcome::LoopCompleted { output } => {
                state.write().await.mark_succeeded(&node_id, output);
                executor.emit_node_status(&workflow.id, &node_id, "success");

                let (_, drop_warn) = executor.activate_successors(
                    &node_id, "done", &workflow, &mut active_nodes,
                );
                if let Some(msg) = drop_warn {
                    let node_name = node_map.get(node_id.as_str()).map(|n| n.name.as_str()).unwrap_or(node_id.as_str());
                    let user_msg = msg.replacen(&node_id, node_name, 1);
                    state.write().await.log(Some(&node_id), LogLevel::Error, user_msg);
                }
                for body_id in loop_body_nodes {
                    completed.insert(body_id);
                }
            }

            NodeOutcome::LoopFailed { err_msg } => {
                let fail_output = NodeOutput::failure(
                    NodeError::unrecoverable("LOOP_ERROR", err_msg.clone())
                );
                state.write().await.mark_failed(&node_id, fail_output, 1);
                executor.emit_node_status(&workflow.id, &node_id, "error");

                // Mark body nodes complete so they are never scheduled.
                for body_id in loop_body_nodes {
                    completed.insert(body_id);
                }

                let (should_abort, result) = parallel_route_failure(
                    &node_id, format!("Node '{}' failed: {}", node_id, err_msg),
                    &executor, &workflow, &graph, &mut active_nodes, &state,
                ).await;
                if should_abort { abort = true; failure_result = result; }
            }
        }
    }

    let validation_errors = validation_acc
        .lock()
        .expect("validation_acc mutex poisoned")
        .clone();

    if abort {
        let mut result = failure_result.unwrap_or_else(|| {
            // Should not reach here: abort=true is always paired with failure_result=Some.
            let s = state.try_read().map(|s| s.execution_id.clone()).unwrap_or_default();
            WorkflowResult {
                execution_id:      s,
                workflow_id:       workflow.id.clone(),
                success:           false,
                node_outputs:      HashMap::new(),
                logs:              vec![],
                error:             Some("Workflow aborted — this is a bug, please report it".to_string()),
                validation_errors: vec![],
            }
        });
        result.validation_errors = validation_errors;
        return Ok(result);
    }

    let s = state.read().await;
    Ok(WorkflowResult {
        execution_id:      s.execution_id.clone(),
        workflow_id:       workflow.id.clone(),
        success:           true,
        node_outputs:      (*s.snapshot().node_outputs).clone(),
        logs:              s.logs.clone(),
        error:             None,
        validation_errors,
    })
}
