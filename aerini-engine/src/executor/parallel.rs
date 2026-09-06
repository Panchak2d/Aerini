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
/// for the caller to apply. Shared by the four failure-outcome arms below so
/// this routing logic exists in exactly one place.
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
                let is_trigger_plugin = executor.registry.get(&node_def.node_type_id)
                    .map(|n| n.is_trigger_capable())
                    .unwrap_or(false);
                if !TRIGGER_TYPES.contains(&node_def.node_type_id.as_str()) && !is_trigger_plugin {
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

    let semaphore = Arc::new(tokio::sync::Semaphore::new(executor.config.max_concurrent_nodes));

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
        if executor.config.cancel_token.as_ref().map(|t| t.is_cancelled()).unwrap_or(false) {
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

                    let resolved_input = match exec.build_input(&wf, &ndef, &st, None).await {
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
                        if exec.config.strict_schema_validation {
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
                let is_cancelled = output.error.as_ref()
                    .map(|e| e.code == super::CANCEL_ERROR_CODE)
                    .unwrap_or(false);
                state.write().await.mark_failed(&node_id, output, attempts);
                executor.emit_node_status(&workflow.id, &node_id, "error");

                if is_cancelled {
                    while join_set.join_next().await.is_some() {}
                    return Err(EngineError::ExecutionCancelled);
                }

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

                // execute_loop_node returns a plain String on error — no structured
                // error code survives to check against CANCEL_ERROR_CODE here, so
                // (matching sequential.rs's equivalent loop-Err handling) fall back
                // to the token's own state directly.
                if executor.config.cancel_token.as_ref().map(|t| t.is_cancelled()).unwrap_or(false) {
                    while join_set.join_next().await.is_some() {}
                    return Err(EngineError::ExecutionCancelled);
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

// ── parallel executor tests ──────────────────────────────────────────────
//
// These tests run with parallel_execution = true to exercise the JoinSet-based
// concurrent path. They use the same real WorkflowExecutor as integration tests,
// but with in-process mock nodes so no real I/O occurs.
#[cfg(test)]
mod tests {
    use super::super::{CredentialResolveError, CredentialResolver, WorkflowExecutor};
    use crate::error::EngineError;
    use crate::migration::CURRENT_VERSION;
    use crate::model::{NodeInput, NodeOutput, NodeType, Workflow, WorkflowEdge, WorkflowNode};
    use crate::node::{Node, NodeRegistry};
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    struct NoopCreds;
    #[async_trait::async_trait]
    impl CredentialResolver for NoopCreds {
        async fn resolve(&self, _: &str) -> Result<String, CredentialResolveError> {
            Err(CredentialResolveError::NotFound)
        }
    }

    struct InstantNode;
    #[async_trait::async_trait]
    impl Node for InstantNode {
        fn type_id(&self)        -> &'static str { "parallel_instant_test" }
        fn display_name(&self)   -> &'static str { "Instant" }
        fn node_type(&self)      -> NodeType     { NodeType::Utility }
        fn version(&self)        -> &'static str { "1.0" }
        fn input_schema(&self)   -> serde_json::Value { serde_json::json!({}) }
        fn output_schema(&self)  -> serde_json::Value { serde_json::json!({}) }
        async fn execute(&self, _: NodeInput) -> NodeOutput {
            NodeOutput::success(serde_json::json!({}))
        }
    }

    struct FailingNode;
    #[async_trait::async_trait]
    impl Node for FailingNode {
        fn type_id(&self)        -> &'static str { "parallel_failing_test" }
        fn display_name(&self)   -> &'static str { "Failing" }
        fn node_type(&self)      -> NodeType     { NodeType::Utility }
        fn version(&self)        -> &'static str { "1.0" }
        fn input_schema(&self)   -> serde_json::Value { serde_json::json!({}) }
        fn output_schema(&self)  -> serde_json::Value { serde_json::json!({}) }
        async fn execute(&self, _: NodeInput) -> NodeOutput {
            NodeOutput::failure(
                crate::error::NodeError::unrecoverable("PARALLEL_FAIL", "parallel test failure"),
            )
        }
    }

    // Sleeps 60s — only used to prove a mid-execute cancel interrupts it long
    // before that, not to exercise the sleep duration itself.
    struct SlowNode;
    #[async_trait::async_trait]
    impl Node for SlowNode {
        fn type_id(&self)        -> &'static str { "parallel_slow_test" }
        fn display_name(&self)   -> &'static str { "Slow" }
        fn node_type(&self)      -> NodeType     { NodeType::Utility }
        fn version(&self)        -> &'static str { "1.0" }
        fn input_schema(&self)   -> serde_json::Value { serde_json::json!({}) }
        fn output_schema(&self)  -> serde_json::Value { serde_json::json!({}) }
        async fn execute(&self, _: NodeInput) -> NodeOutput {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            NodeOutput::success(serde_json::json!({}))
        }
    }

    fn make_node(id: &str, type_id: &str) -> WorkflowNode {
        WorkflowNode {
            id: id.to_string(),
            node_type_id: type_id.to_string(),
            node_type: NodeType::Utility,
            name: id.to_string(),
            config: serde_json::json!({}),
            credentials: HashMap::new(),
            input_schema: serde_json::json!({}),
            output_schema: serde_json::json!({}),
            retry: Default::default(),
            fallback_node: None,
            disabled: false,
            position: Default::default(),
        }
    }

    fn make_edge(from: &str, to: &str) -> WorkflowEdge {
        WorkflowEdge {
            id: format!("e_{}_to_{}", from, to),
            from_node: from.to_string(),
            from_port: "output".to_string(),
            to_node: to.to_string(),
            to_port: "input".to_string(),
            condition: None,
            on_success: None,
            on_failure: None,
        }
    }

    // All nodes complete: three independent (no-edge) entry nodes run in parallel.
    // Every node must appear in node_outputs when all succeed.
    #[tokio::test]
    async fn parallel_all_nodes_complete_every_result_present() {
        let mut registry = NodeRegistry::new();
        registry.register(Arc::new(InstantNode));

        let executor = WorkflowExecutor::new(Arc::new(registry), Arc::new(NoopCreds))
            .with_parallel_execution(true)
            .with_max_concurrent_nodes(4);

        let workflow = Workflow {
            schema_version: CURRENT_VERSION.to_string(),
            id: "wf_parallel_all".to_string(),
            name: "Parallel All".to_string(),
            description: String::new(),
            nodes: vec![
                make_node("n_a", "parallel_instant_test"),
                make_node("n_b", "parallel_instant_test"),
                make_node("n_c", "parallel_instant_test"),
            ],
            // No edges — all three are independent entry nodes.
            edges: vec![],
            metadata: Default::default(),
            max_duration_secs: None,
            unlimited_duration: false,
            parallel_execution: true,
            max_concurrent_nodes: Some(4),
            settings: Default::default(),
        };

        let result = executor.run(Arc::new(workflow), HashMap::new()).await.unwrap();

        assert!(result.success, "all instant nodes must succeed; got: {:?}", result.error);
        assert!(result.node_outputs.contains_key("n_a"), "n_a must be in node_outputs");
        assert!(result.node_outputs.contains_key("n_b"), "n_b must be in node_outputs");
        assert!(result.node_outputs.contains_key("n_c"), "n_c must be in node_outputs");
    }

    // One node errors: failure is captured in WorkflowResult, no panic.
    // Topology: FailingNode (entry) → InstantNode.
    // FailingNode has no on_error route → abort → success=false with error message.
    // InstantNode (downstream) was never activated → must not appear in node_outputs.
    #[tokio::test]
    async fn parallel_one_node_errors_failure_captured_no_panic() {
        let mut registry = NodeRegistry::new();
        registry.register(Arc::new(FailingNode));
        registry.register(Arc::new(InstantNode));

        let executor = WorkflowExecutor::new(Arc::new(registry), Arc::new(NoopCreds))
            .with_parallel_execution(true);

        let workflow = Workflow {
            schema_version: CURRENT_VERSION.to_string(),
            id: "wf_parallel_fail".to_string(),
            name: "Parallel Fail".to_string(),
            description: String::new(),
            nodes: vec![
                make_node("n_fail", "parallel_failing_test"),
                make_node("n_after", "parallel_instant_test"),
            ],
            edges: vec![make_edge("n_fail", "n_after")],
            metadata: Default::default(),
            max_duration_secs: None,
            unlimited_duration: false,
            parallel_execution: true,
            max_concurrent_nodes: None,
            settings: Default::default(),
        };

        let result = executor.run(Arc::new(workflow), HashMap::new()).await;

        assert!(result.is_ok(), "run() must not return Err for a node failure");
        let r = result.unwrap();
        assert!(!r.success, "workflow must be marked failed");
        assert!(r.error.is_some(), "error must be captured in WorkflowResult");
        // n_after was never activated (n_fail had no on_error edge).
        assert!(
            !r.node_outputs.contains_key("n_after"),
            "downstream node must not appear in node_outputs when upstream failed without routing"
        );
    }

    // Cancel fired mid-execute in parallel mode, on a node with an on_error
    // edge wired: the run must stop (Err(ExecutionCancelled)), not silently
    // route through on_error and keep going. No prior test in this module
    // exercised parallel-mode cancellation at all.
    #[tokio::test]
    async fn parallel_cancel_mid_execute_bypasses_on_error_route() {
        let mut registry = NodeRegistry::new();
        registry.register(Arc::new(SlowNode));
        registry.register(Arc::new(InstantNode));

        let token = CancellationToken::new();

        let executor = WorkflowExecutor::new(Arc::new(registry), Arc::new(NoopCreds))
            .with_parallel_execution(true)
            .with_cancel_token(token.clone());

        let workflow = Workflow {
            schema_version: CURRENT_VERSION.to_string(),
            id: "wf_parallel_cancel".to_string(),
            name: "Parallel Cancel".to_string(),
            description: String::new(),
            nodes: vec![
                make_node("n_slow", "parallel_slow_test"),
                make_node("n_recover", "parallel_instant_test"),
            ],
            edges: vec![WorkflowEdge {
                id: "e_slow_on_error".to_string(),
                from_node: "n_slow".to_string(),
                from_port: "on_error".to_string(),
                to_node: "n_recover".to_string(),
                to_port: "input".to_string(),
                condition: None, on_success: None, on_failure: None,
            }],
            metadata: Default::default(),
            max_duration_secs: None,
            unlimited_duration: false,
            parallel_execution: true,
            max_concurrent_nodes: None,
            settings: Default::default(),
        };

        let cancel_token = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            cancel_token.cancel();
        });

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            executor.run(Arc::new(workflow), HashMap::new()),
        ).await;

        let result = result.expect(
            "run() did not return within 5s — cancellation did not interrupt the in-flight node"
        );
        assert!(
            matches!(result, Err(EngineError::ExecutionCancelled)),
            "expected ExecutionCancelled (on_error route must not mask a cancel); got {:?}", result
        );
    }
}
