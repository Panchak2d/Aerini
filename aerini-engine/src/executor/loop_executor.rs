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
        // Safety net against executor re-entry bugs should never be reached in
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

        // Direct loop_body successors of the loop node, these are the only body
        // nodes that unconditionally run every iteration. Everything else reachable
        // inside the body (e.g. downstream of an If/Switch node) only runs this
        // iteration if that branching node's taken port actually activates it,
        // see `active_body` below. Mirrors how sequential.rs seeds `active_nodes`
        // from `graph.entry_nodes` for the outer topo walk.
        let body_set: HashSet<&str> = body_nodes.iter().map(String::as_str).collect();
        let seed_body: HashSet<String> = workflow.edges.iter()
            .filter(|e| e.from_node == loop_node_id
                && e.from_port == "loop_body"
                && body_set.contains(e.to_node.as_str()))
            .map(|e| e.to_node.clone())
            .collect();

        // Body nodes that have at least one outgoing edge to anything. Used below
        // to tell "this branch genuinely goes nowhere for the port that fired.
        // worth a warning" (e.g. an If node with only on_true wired) apart from
        // "this is an ordinary leaf action node with nothing further wired,"
        // which is the common case for a loop body and must not warn every
        // single iteration.
        let body_ids_with_any_outgoing_edge: HashSet<&str> = workflow.edges.iter()
            .filter(|e| body_set.contains(e.from_node.as_str()))
            .map(|e| e.from_node.as_str())
            .collect();

        state.write().await.mark_running(loop_node_id);
        self.emit_node_status(&workflow.id, loop_node_id, "running");

        for iteration in 0..=MAX_LOOP_ITERATIONS {
            // Cancel check — exits the loop immediately between iterations.
            if self.config.cancel_token.as_ref().map(|t| t.is_cancelled()).unwrap_or(false) {
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
            let loop_input = match self.build_input(workflow, loop_node_def, state, None).await {
                Ok(input) => input,
                Err(failure) => {
                    let msg = failure.error.as_ref().map(|e| e.message.clone()).unwrap_or_else(|| "unknown error".to_string());
                    return Err(format!("Loop node '{}' build_input failed: {}", loop_node_id, msg));
                }
            };
            // This call bypasses `execute_with_retry` entirely
            // (sequential.rs special-cases `node_type_id == "loop"` to drive
            // iteration here instead), so it needs its own node-level group
            // rather than inheriting one from there — one fresh group per
            // iteration, matching `execute_with_retry`'s own per-attempt
            // granularity. Built before `loop_input` moves into `execute()`.
            let mem_meta = crate::mem_tracking::GroupMeta::node(
                loop_input.workflow_id.clone(),
                loop_node_id.to_string(),
                loop_node_def.node_type_id.clone(),
            );
            let loop_output = if let Some(ref token) = self.config.cancel_token {
                tokio::select! {
                    output = crate::mem_tracking::run_tracked(mem_meta, loop_node_impl.execute(loop_input)) => output,
                    _ = token.cancelled() => return Err("Run cancelled by user".to_string()),
                }
            } else {
                crate::mem_tracking::run_tracked(mem_meta, loop_node_impl.execute(loop_input)).await
            };

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
                // Collect results from the dedicated store (not from snapshot metadata).
                // loop_node.rs returns all_results: [] as a placeholder on the done path;
                // we override it here with the real accumulated results.
                let all_results = state.write().await.take_loop_results(loop_node_id);
                let final_output = if let Some(mut data) = loop_output.output.clone() {
                    data["all_results"] = serde_json::Value::Array(all_results);
                    NodeOutput { output: Some(data), ..loop_output }
                } else {
                    loop_output
                };
                return Ok(final_output);
            }

            // Read the next index to set from the loop node's output field.
            let next_index = loop_output
                .output
                .as_ref()
                .and_then(|v| v.get("__loop_next_index"))
                .and_then(|v| v.as_u64())
                .unwrap_or(iteration + 1);

            // Tracks this iteration's result for all_results — one entry per
            // iteration via overwrite-then-push-once, not push-per-body-node
            // (which would produce k×n entries for a k-body-node loop).
            let mut iteration_result: Option<serde_json::Value> = None;

            // Per-iteration active-node set, reset from the seed every iteration.
            // A body node only executes this iteration if it's in the seed set or
            // was activated by a branching node's (If/Switch) taken port below —
            // this is what makes branch selection actually gate execution inside
            // a loop body, instead of every reachable body node running every
            // iteration regardless of which branch the condition took.
            let mut active_body: HashSet<String> = seed_body.clone();

            // Execute each body node in topo order for this iteration.
            for body_id in &body_nodes {
                let body_def = match node_map.get(body_id.as_str()).copied() {
                    Some(n) => n,
                    None => return Err(format!(
                        "Loop body node '{}' not found in workflow — this is a bug", body_id
                    )),
                };

                // Not activated by this iteration's branch selection — skip without
                // executing (mirrors sequential.rs's `!active_nodes.contains(node_id)`).
                if !active_body.contains(body_id) {
                    state.write().await.mark_skipped(body_id);
                    self.emit_node_status(&workflow.id, body_id, "skipped");
                    continue;
                }

                // Honour disabled flag inside the loop body too.
                if body_def.disabled {
                    {
                        let mut s = state.write().await;
                        s.log(Some(body_id), LogLevel::Info, format!("Node '{}' is disabled — skipped", body_def.name));
                        s.mark_skipped(body_id);
                    }
                    self.emit_node_status(&workflow.id, body_id, "skipped");
                    // Pass through to "output" successors so the chain continues
                    // through the disabled node, matching sequential.rs's handling
                    // of a disabled node at the top level.
                    let (_, drop_warn) = self.activate_successors(body_id, "output", workflow, &mut active_body);
                    if let Some(msg) = drop_warn {
                        if body_def.node_type_id != "output"
                            && body_ids_with_any_outgoing_edge.contains(body_id.as_str())
                        {
                            let mut s = state.write().await;
                            s.log(Some(body_id), LogLevel::Error, msg.replacen(body_id.as_str(), &body_def.name, 1));
                        }
                    }
                    continue;
                }

                let body_impl = self.registry.get(&body_def.node_type_id).ok_or_else(|| {
                    format!("Body node type '{}' not registered", body_def.node_type_id)
                })?;

                let body_input = match self.build_input(workflow, body_def, state, Some(iteration)).await {
                    Ok(input) => input,
                    Err(failure) => {
                        let msg = failure.error.as_ref().map(|e| e.message.clone()).unwrap_or_else(|| "unknown error".to_string());
                        state.write().await.mark_failed(body_id, failure, 1);
                        self.emit_node_status(&workflow.id, body_id, "error");
                        // try on_error/on_failure routing before aborting the whole loop.
                        let (routed, warn) = self.route_loop_body_failure(
                            body_id, &body_def.name, workflow, &body_set, &mut active_body,
                        );
                        if let Some(w) = warn {
                            state.write().await.log(Some(body_id), LogLevel::Warn, w);
                        }
                        if routed {
                            continue;
                        }
                        return Err(format!("Loop body node '{}' build_input failed: {}", body_id, msg));
                    }
                };

                // Validate body node input. In strict mode, fail the loop iteration.
                // In non-strict mode, log a warning to the execution log and continue.
                let schema_errors = Self::validate_node_input(&body_def.input_schema, &body_input.input);
                if !schema_errors.is_empty() {
                    if self.config.strict_schema_validation {
                        let reason = schema_errors.join("; ");
                        let fail_output = crate::model::NodeOutput::failure(
                            crate::error::NodeError::unrecoverable(
                                "SCHEMA_VIOLATION",
                                format!("Input schema validation failed for loop body node '{}': {}", body_id, reason),
                            ),
                        );
                        state.write().await.mark_failed(body_id, fail_output, 1);
                        self.emit_node_status(&workflow.id, body_id, "error");
                        // try on_error/on_failure routing before aborting the whole loop.
                        let (routed, warn) = self.route_loop_body_failure(
                            body_id, &body_def.name, workflow, &body_set, &mut active_body,
                        );
                        if let Some(w) = warn {
                            state.write().await.log(Some(body_id), LogLevel::Warn, w);
                        }
                        if routed {
                            continue;
                        }
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
                    let is_cancelled = body_output.error.as_ref()
                        .map(|e| e.code == super::CANCEL_ERROR_CODE)
                        .unwrap_or(false);
                    let msg = body_output
                        .error
                        .as_ref()
                        .map(|e| e.message.clone())
                        .unwrap_or_else(|| "unknown error".to_string());
                    state.write().await.mark_failed(body_id, body_output, _attempts);
                    self.emit_node_status(&workflow.id, body_id, "error");

                    // A cancelled body node must stop the run, not be routed like an
                    // ordinary failure — an on_error edge here would otherwise mask
                    // the cancel and let the loop keep going. No structured error
                    // code survives past this fn's `Result<_, String>` return, so
                    // the caller (sequential.rs) re-derives the cancellation from
                    // the token's own state once it sees any Err here.
                    if is_cancelled {
                        return Err(msg);
                    }

                    // Mirror the on_error/on_failure routing every top-level node
                    // failure already gets (sequential.rs/parallel.rs) — scoped to this
                    // iteration's active_body set — so a workflow author who wires an
                    // explicit on_error edge or on_failure fallback on a body node (e.g.
                    // "for each item, try X, on failure log and continue to the next
                    // item") gets that routing honoured instead of the whole loop dying
                    // on the first failing item.
                    let (routed, warn) = self.route_loop_body_failure(
                        body_id, &body_def.name, workflow, &body_set, &mut active_body,
                    );
                    if let Some(w) = warn {
                        state.write().await.log(Some(body_id), LogLevel::Warn, w);
                    }
                    if routed {
                        continue;
                    }
                    return Err(format!(
                        "Loop body node '{}' failed on iteration {}: {}", body_id, iteration, msg
                    ));
                }

                // Track this iteration's result (overwrite — last body node wins,
                // matching the original per-iteration key-collision semantics).
                if let Some(ref val) = body_output.output {
                    iteration_result = Some(val.clone());
                }
                let taken_port = Self::resolve_taken_port(&body_output);
                state.write().await.mark_succeeded(body_id, body_output);
                self.emit_node_status(&workflow.id, body_id, "success");

                // Only the edge(s) on the node's actual taken port
                // (on_true/on_false, case_N/default, done/loop_body, or the plain
                // "output" port) become active for the rest of this iteration —
                // an If/Switch node's untaken branch does not run.
                let (fallback_fired, drop_warn) =
                    self.activate_successors(body_id, &taken_port, workflow, &mut active_body);
                if let Some(msg) = drop_warn {
                    if body_def.node_type_id != "output"
                        && body_ids_with_any_outgoing_edge.contains(body_id.as_str())
                    {
                        let mut s = state.write().await;
                        s.log(Some(body_id), LogLevel::Error, msg.replacen(body_id.as_str(), &body_def.name, 1));
                    }
                } else if fallback_fired && taken_port != "output" {
                    let mut s = state.write().await;
                    s.log(
                        Some(body_id),
                        LogLevel::Warn,
                        format!(
                            "Node '{}': no edge found on port '{}' inside loop body — fell back \
                             to 'output' port routing. Connect the '{}' port explicitly to \
                             suppress this.",
                            body_def.name, taken_port, taken_port
                        ),
                    );
                }
            }

            // Push exactly one entry for this iteration (the last body node's
            // output), preserving the original n-entries-total semantics while
            // avoiding the O(k·n²) clone cost of the old loop_state-based approach.
            if let Some(val) = iteration_result {
                state.write().await.push_loop_result(loop_node_id, val);
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

    // routes a failing loop-body node's failure through the same
    // two mechanisms a top-level node failure already gets in
    // sequential.rs/parallel.rs — an `on_error`-port edge (activate_successors),
    // falling back to a `WorkflowEdge.on_failure` pointer (the field
    // executor/mod.rs::find_failure_route resolves via ExecutionGraph at the
    // top level). Reimplemented directly against `workflow.edges` here rather
    // than threading an `ExecutionGraph` into `execute_loop_node` — body nodes
    // are ordinary members of `workflow.edges`, so a direct scan finds the same
    // edges, and this keeps the logic self-contained inside loop_executor.rs
    // rather than widening it into sequential.rs's and parallel.rs's call
    // sites (the latter would additionally require Arc-wrapping
    // ExecutionGraph to cross into parallel.rs's spawned per-loop tokio
    // task).
    //
    // A routed target must itself be a loop-body node (`body_set` — the same
    // membership `collect_loop_body_nodes` already computed for this loop) to
    // be actionable: `active_body` is only ever consulted by the `for body_id
    // in &body_nodes` loop above, so a target outside the body has no way to
    // run this iteration no matter what is inserted into the set. Rather than
    // silently no-op in that case — indistinguishable, from the workflow
    // author's side, from "no route configured at all" — this returns a
    // warning to log and falls through to the abort-the-loop outcome below.
    //
    // Returns `(routed, warning)`. `routed == true` means the caller should
    // `continue` to the next body node instead of aborting the loop.
    // `routed == false` means any loop that does not wire on_error/on_failure
    // on its body nodes aborts on first failure.
    pub(super) fn route_loop_body_failure(
        &self,
        body_id:     &str,
        body_name:   &str,
        workflow:    &Workflow,
        body_set:    &HashSet<&str>,
        active_body: &mut HashSet<String>,
    ) -> (bool, Option<String>) {
        // 1. on_error port.
        let on_error_targets: Vec<&str> = workflow.edges.iter()
            .filter(|e| e.from_node == body_id && e.from_port == "on_error")
            .map(|e| e.to_node.as_str())
            .collect();
        let on_error_in_body: Vec<&str> = on_error_targets.iter()
            .copied()
            .filter(|t| body_set.contains(t))
            .collect();
        if !on_error_in_body.is_empty() {
            for t in &on_error_in_body {
                active_body.insert((*t).to_string());
            }
            // Some on_error targets may still be outside the loop body even
            // though at least one usable (in-body) target was found — route
            // via the usable one(s), but don't silently drop the rest.
            let skipped: Vec<&str> = on_error_targets.iter()
                .copied()
                .filter(|t| !body_set.contains(t))
                .collect();
            let warn = if skipped.is_empty() {
                None
            } else {
                Some(format!(
                    "Node '{}': on_error also routes to {} outside this loop's body — \
                     those target(s) are skipped (only in-body targets can run \
                     per-iteration); the in-body on_error target(s) still ran.",
                    body_name,
                    skipped.iter().map(|t| format!("'{}'", t)).collect::<Vec<_>>().join(", "),
                ))
            };
            return (true, warn);
        }

        // 2. WorkflowEdge.on_failure pointer.
        if let Some(target) = workflow.edges.iter()
            .find(|e| e.from_node == body_id && e.on_failure.is_some())
            .and_then(|e| e.on_failure.clone())
        {
            if body_set.contains(target.as_str()) {
                active_body.insert(target);
                return (true, None);
            }
            return (false, Some(format!(
                "Node '{}': on_failure routes to '{}', which is outside this loop's body — \
                 the fallback cannot run per-iteration inside the loop, so this failure still \
                 aborts the loop. Wire the fallback node inside the loop body to keep the loop \
                 running past this failure.",
                body_name, target
            )));
        }

        if !on_error_targets.is_empty() {
            // on_error was wired but every target is outside the loop body.
            return (false, Some(format!(
                "Node '{}': on_error routes outside this loop's body — the fallback cannot run \
                 per-iteration inside the loop, so this failure still aborts the loop. Wire the \
                 fallback node inside the loop body to keep the loop running past this failure.",
                body_name
            )));
        }

        (false, None)
    }
}

// ── Loop executor regression tests ───────────────────────────────────────────
//
// Harness note: a general executor test harness already exists in executor/mod.rs.
// These tests live here because they are specific to loop execution semantics and
// require the real LoopNode + execute_loop_node path.
//
// The three cases covered:
//   (1) Multi-body-node overwrite: all_results must have exactly one entry per
//       iteration (= last body node's output), never k*n entries.
//   (2) Disabled body node: disabled node is skipped; enabled node's output is
//       captured.
//   (3) None-output body node: a body returning output:None must not push a null
//       entry; a prior body's output for the same iteration must be preserved.
#[cfg(test)]
mod tests {
    use super::super::{CredentialResolveError, CredentialResolver, WorkflowExecutor};
    use crate::error::EngineError;
    use crate::migration::CURRENT_VERSION;
    use crate::model::{NodeInput, NodeOutput, NodeType, Workflow, WorkflowEdge, WorkflowNode};
    use crate::node::{Node, NodeRegistry};
    use crate::nodes::loop_node::LoopNode;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    // ── Credentials stub ──────────────────────────────────────────────────────

    struct NoopCreds;
    #[async_trait::async_trait]
    impl CredentialResolver for NoopCreds {
        async fn resolve(&self, _: &str) -> Result<String, CredentialResolveError> {
            Err(CredentialResolveError::NotFound)
        }
    }

    // ── Test node types ───────────────────────────────────────────────────────

    // DataSource: returns {"items": <array>}. The array is baked in via the
    // `output` field at registry construction time, not stored in the Workflow.
    struct DataSourceNode {
        output: serde_json::Value,
    }
    #[async_trait::async_trait]
    impl Node for DataSourceNode {
        fn type_id(&self) -> &'static str { "data_source_loop_test" }
        fn display_name(&self) -> &'static str { "Data Source" }
        fn node_type(&self) -> NodeType { NodeType::Utility }
        fn version(&self) -> &'static str { "1.0" }
        fn input_schema(&self) -> serde_json::Value { serde_json::json!({}) }
        fn output_schema(&self) -> serde_json::Value { serde_json::json!({}) }
        async fn execute(&self, _: NodeInput) -> NodeOutput {
            NodeOutput::success(self.output.clone())
        }
    }

    // FixedOutputNode: returns a fixed JSON value. `tid` is a &'static str so
    // multiple distinct body node types (body1, body2, …) can be registered in
    // the same NodeRegistry without type_id collisions.
    struct FixedOutputNode {
        tid: &'static str,
        out: serde_json::Value,
    }
    #[async_trait::async_trait]
    impl Node for FixedOutputNode {
        fn type_id(&self) -> &'static str { self.tid }
        fn display_name(&self) -> &'static str { "Fixed Output" }
        fn node_type(&self) -> NodeType { NodeType::Utility }
        fn version(&self) -> &'static str { "1.0" }
        fn input_schema(&self) -> serde_json::Value { serde_json::json!({}) }
        fn output_schema(&self) -> serde_json::Value { serde_json::json!({}) }
        async fn execute(&self, _: NodeInput) -> NodeOutput {
            NodeOutput::success(self.out.clone())
        }
    }

    // NullOutputNode: succeeds but returns output: None. Used to verify that a
    // None-output body does not push a null entry into all_results.
    struct NullOutputNode;
    #[async_trait::async_trait]
    impl Node for NullOutputNode {
        fn type_id(&self) -> &'static str { "null_output_loop_test" }
        fn display_name(&self) -> &'static str { "Null Output" }
        fn node_type(&self) -> NodeType { NodeType::Utility }
        fn version(&self) -> &'static str { "1.0" }
        fn input_schema(&self) -> serde_json::Value { serde_json::json!({}) }
        fn output_schema(&self) -> serde_json::Value { serde_json::json!({}) }
        async fn execute(&self, _: NodeInput) -> NodeOutput {
            NodeOutput { success: true, output: None, error: None, logs: vec![] }
        }
    }

    // ── Workflow builder ──────────────────────────────────────────────────────

    // Builds: data_source → loop_node → body_0 … body_N (loop_body edges).
    //
    // `bodies` — (node_type_id, disabled) for each body node in execution order.
    //
    // The DataSource node is always type "data_source_loop_test" and its actual
    // output is set at registry construction time (see each test below).
    // The loop node config points at "data_source" and reads the "items" field.
    fn loop_workflow_with_bodies(bodies: Vec<(&str, bool)>) -> Workflow {
        let data_source = WorkflowNode {
            id:            "data_source".to_string(),
            node_type_id:  "data_source_loop_test".to_string(),
            node_type:     NodeType::Utility,
            name:          "Data Source".to_string(),
            config:        serde_json::json!({}),
            credentials:   HashMap::new(),
            input_schema:  serde_json::json!({}),
            output_schema: serde_json::json!({}),
            retry:         Default::default(),
            fallback_node: None,
            disabled:      false,
            position:      Default::default(),
        };

        let loop_node = WorkflowNode {
            id:            "loop_node".to_string(),
            node_type_id:  "loop".to_string(),
            node_type:     NodeType::Logic,
            name:          "Loop".to_string(),
            config:        serde_json::json!({
                "array_field": "items",
                "source_node": "data_source"
            }),
            credentials:   HashMap::new(),
            input_schema:  serde_json::json!({}),
            output_schema: serde_json::json!({}),
            retry:         Default::default(),
            fallback_node: None,
            disabled:      false,
            position:      Default::default(),
        };

        let mut nodes = vec![data_source, loop_node];
        let mut edges = vec![WorkflowEdge {
            id:         "e_src_loop".to_string(),
            from_node:  "data_source".to_string(),
            from_port:  "output".to_string(),
            to_node:    "loop_node".to_string(),
            to_port:    "input".to_string(),
            condition:  None,
            on_success: None,
            on_failure: None,
        }];

        for (i, (type_id, disabled)) in bodies.iter().enumerate() {
            let body_id = format!("body_{}", i);
            nodes.push(WorkflowNode {
                id:            body_id.clone(),
                node_type_id:  type_id.to_string(),
                node_type:     NodeType::Utility,
                name:          format!("Body {}", i),
                config:        serde_json::json!({}),
                credentials:   HashMap::new(),
                input_schema:  serde_json::json!({}),
                output_schema: serde_json::json!({}),
                retry:         Default::default(),
                fallback_node: None,
                disabled:      *disabled,
                position:      Default::default(),
            });
            edges.push(WorkflowEdge {
                id:         format!("e_loop_body_{}", i),
                from_node:  "loop_node".to_string(),
                from_port:  "loop_body".to_string(),
                to_node:    body_id,
                to_port:    "input".to_string(),
                condition:  None,
                on_success: None,
                on_failure: None,
            });
        }

        Workflow {
            schema_version:       CURRENT_VERSION.to_string(),
            id:                   "wf_loop_test".to_string(),
            name:                 "Loop Regression Test Workflow".to_string(),
            description:          String::new(),
            nodes,
            edges,
            metadata:             Default::default(),
            max_duration_secs:    None,
            unlimited_duration:   false,
            parallel_execution:   false,
            max_concurrent_nodes: None,
            settings:             Default::default(),
        }
    }

    // ── Regression test 1: one entry per iteration, not per body node ─────────
    //
    // Before the fix: each body node called push_loop_result, producing k×n
    // entries (k=2 bodies, n=3 iterations → 6 entries). After the fix:
    // overwrite-then-push-once per iteration → 3 entries, each equal to the
    // last body node's output (whichever the executor's topo order puts last).
    //
    // The count assertion (len == 3, not 6) is the core regression check.
    // The value assertion confirms overwrite semantics: whichever body node ran
    // last wins every iteration consistently — we don't assert which one because
    // petgraph's toposort order between peers at the same depth is an
    // implementation detail, not a contract.
    #[tokio::test]
    async fn loop_all_results_one_entry_per_iteration_not_per_body_node() {
        let mut registry = NodeRegistry::new();
        registry.register(Arc::new(DataSourceNode {
            output: serde_json::json!({ "items": [1, 2, 3] }),
        }));
        registry.register(Arc::new(LoopNode));
        registry.register(Arc::new(FixedOutputNode {
            tid: "body1_loop_test",
            out: serde_json::json!({ "r": "body1" }),
        }));
        registry.register(Arc::new(FixedOutputNode {
            tid: "body2_loop_test",
            out: serde_json::json!({ "r": "body2" }),
        }));

        let workflow = loop_workflow_with_bodies(vec![
            ("body1_loop_test", false),
            ("body2_loop_test", false),
        ]);

        let result = WorkflowExecutor::new(Arc::new(registry), Arc::new(NoopCreds))
            .run(Arc::new(workflow), HashMap::new())
            .await
            .unwrap();

        assert!(result.success, "workflow failed: {:?}", result.error);

        let loop_out = result.node_outputs.get("loop_node")
            .expect("loop_node must have output");
        let all_results = loop_out["all_results"].as_array()
            .expect("all_results must be an array");

        // Primary regression check: 3 iterations → exactly 3 entries, not 6.
        assert_eq!(
            all_results.len(), 3,
            "expected one entry per iteration; got {} — \
             likely regression: push_loop_result called per body node rather than per iteration",
            all_results.len()
        );

        // Overwrite semantics: all entries must be equal (the same body node won
        // every iteration). We don't assert which body node won; topo order
        // between peers is an implementation detail.
        let known_outputs = [
            serde_json::json!({ "r": "body1" }),
            serde_json::json!({ "r": "body2" }),
        ];
        let first = &all_results[0];
        assert!(
            known_outputs.iter().any(|k| k == first),
            "all_results[0] is not from a known body node: {:?}", first
        );
        for (i, entry) in all_results.iter().enumerate() {
            assert_eq!(
                entry, first,
                "iteration {}: inconsistent all_results entries — \
                 different body nodes won different iterations (overwrite semantics broken)",
                i
            );
        }
    }

    // ── Regression test 2: disabled body node is skipped ─────────────────────
    //
    // The disabled body node (body_0) must be skipped each iteration.
    // The enabled body node (body_1) must produce the captured output.
    #[tokio::test]
    async fn loop_disabled_body_skipped_enabled_body_output_captured() {
        let mut registry = NodeRegistry::new();
        registry.register(Arc::new(DataSourceNode {
            output: serde_json::json!({ "items": ["x", "y"] }),
        }));
        registry.register(Arc::new(LoopNode));
        registry.register(Arc::new(FixedOutputNode {
            tid: "body_disabled_test",
            out: serde_json::json!({ "r": "should_not_appear" }),
        }));
        registry.register(Arc::new(FixedOutputNode {
            tid: "body_enabled_test",
            out: serde_json::json!({ "r": "enabled" }),
        }));

        let workflow = loop_workflow_with_bodies(vec![
            ("body_disabled_test", true),  // disabled — must be skipped
            ("body_enabled_test",  false),
        ]);

        let result = WorkflowExecutor::new(Arc::new(registry), Arc::new(NoopCreds))
            .run(Arc::new(workflow), HashMap::new())
            .await
            .unwrap();

        assert!(result.success);

        let loop_out = result.node_outputs.get("loop_node").unwrap();
        let all_results = loop_out["all_results"].as_array().unwrap();

        assert_eq!(all_results.len(), 2, "expected one entry per iteration");
        let expected = serde_json::json!({ "r": "enabled" });
        for (i, entry) in all_results.iter().enumerate() {
            assert_eq!(
                entry, &expected,
                "iteration {}: disabled body's output leaked into all_results: {:?}",
                i, entry
            );
        }
    }

    // ── Regression test 3: None-output body does not push null into all_results ─
    //
    // A body node that returns output: None must never push a null entry.
    // Simplest unambiguous form: single null-output body over 2 iterations →
    // iteration_result stays None every iteration → push_loop_result never fires
    // → all_results must be [] (empty), not [null, null].
    //
    // This also confirms that the `if let Some(val) = iteration_result` guard
    // is behaving correctly: only an iteration that actually produced a value
    // pushes an entry.
    #[tokio::test]
    async fn loop_none_output_body_produces_no_null_entries_in_all_results() {
        let mut registry = NodeRegistry::new();
        registry.register(Arc::new(DataSourceNode {
            output: serde_json::json!({ "items": ["a", "b"] }),
        }));
        registry.register(Arc::new(LoopNode));
        registry.register(Arc::new(NullOutputNode));

        let workflow = loop_workflow_with_bodies(vec![
            ("null_output_loop_test", false),
        ]);

        let result = WorkflowExecutor::new(Arc::new(registry), Arc::new(NoopCreds))
            .run(Arc::new(workflow), HashMap::new())
            .await
            .unwrap();

        assert!(result.success);

        let loop_out = result.node_outputs.get("loop_node").unwrap();
        let all_results = loop_out["all_results"].as_array().unwrap();

        // No body ever produced output → push_loop_result must never have fired.
        // An unconditional push (ignoring output:None) would yield [null, null].
        assert!(
            all_results.is_empty(),
            "expected empty all_results when every body returns output:None; \
             got {} entries — null values are being pushed incorrectly: {:?}",
            all_results.len(), all_results
        );
    }

    // ── loop_executor.rs tests ───────────────────────────────────────────

    // Max iterations enforced: LoopNode hard-caps at 10,000 items (ARRAY_TOO_LARGE).
    // This is the raw array-size cap, separate from the optional per-node
    // max_iterations config field (which can only lower the effective bound,
    // never raise it past this one). This test exercises the raw cap itself:
    // a 10,001-item array → ARRAY_TOO_LARGE, regardless of max_iterations.
    // The error propagates as success=false in WorkflowResult (not Err from run()).
    #[tokio::test]
    async fn loop_max_iterations_enforced_via_array_size_cap() {
        let items: Vec<serde_json::Value> = (0u32..10_001).map(|i| serde_json::json!(i)).collect();

        let mut registry = NodeRegistry::new();
        registry.register(Arc::new(DataSourceNode {
            output: serde_json::json!({ "items": items }),
        }));
        registry.register(Arc::new(LoopNode));
        // No body node — loop fails on first LoopNode call before any body runs.

        let workflow = loop_workflow_with_bodies(vec![]);

        let result = WorkflowExecutor::new(Arc::new(registry), Arc::new(NoopCreds))
            .run(Arc::new(workflow), HashMap::new())
            .await
            .unwrap();

        assert!(!result.success, "oversized array must cause workflow failure");
        let err = result.error.expect("error field must be populated");
        assert!(
            err.contains("10,000") || err.contains("ARRAY_TOO_LARGE"),
            "error must reference the array size limit; got: {}", err
        );
    }

    // max_iterations config (distinct from the raw array-size cap above): a
    // 5-item array with max_iterations: 2 must run the body exactly twice,
    // not five times, and the loop node's own "total" output must show 2.
    #[tokio::test]
    async fn loop_max_iterations_config_stops_body_execution_early() {
        let mut registry = NodeRegistry::new();
        registry.register(Arc::new(DataSourceNode {
            output: serde_json::json!({ "items": [1, 2, 3, 4, 5] }),
        }));
        registry.register(Arc::new(LoopNode));
        registry.register(Arc::new(FixedOutputNode {
            tid: "body_max_iter_test",
            out: serde_json::json!({ "ran": true }),
        }));

        let data_source = WorkflowNode {
            id: "data_source".to_string(), node_type_id: "data_source_loop_test".to_string(),
            node_type: NodeType::Utility, name: "Data Source".to_string(),
            config: serde_json::json!({}), credentials: HashMap::new(),
            input_schema: serde_json::json!({}), output_schema: serde_json::json!({}),
            retry: Default::default(), fallback_node: None, disabled: false,
            position: Default::default(),
        };
        let loop_node = WorkflowNode {
            id: "loop_node".to_string(), node_type_id: "loop".to_string(),
            node_type: NodeType::Logic, name: "Loop".to_string(),
            config: serde_json::json!({
                "array_field": "items", "source_node": "data_source", "max_iterations": 2
            }),
            credentials: HashMap::new(),
            input_schema: serde_json::json!({}), output_schema: serde_json::json!({}),
            retry: Default::default(), fallback_node: None, disabled: false,
            position: Default::default(),
        };
        let body = WorkflowNode {
            id: "body_0".to_string(), node_type_id: "body_max_iter_test".to_string(),
            node_type: NodeType::Utility, name: "Body".to_string(),
            config: serde_json::json!({}), credentials: HashMap::new(),
            input_schema: serde_json::json!({}), output_schema: serde_json::json!({}),
            retry: Default::default(), fallback_node: None, disabled: false,
            position: Default::default(),
        };

        let edges = vec![
            WorkflowEdge {
                id: "e_src_loop".to_string(), from_node: "data_source".to_string(), from_port: "output".to_string(),
                to_node: "loop_node".to_string(), to_port: "input".to_string(),
                condition: None, on_success: None, on_failure: None,
            },
            WorkflowEdge {
                id: "e_loop_body".to_string(), from_node: "loop_node".to_string(), from_port: "loop_body".to_string(),
                to_node: "body_0".to_string(), to_port: "input".to_string(),
                condition: None, on_success: None, on_failure: None,
            },
        ];

        let workflow = Workflow {
            schema_version: CURRENT_VERSION.to_string(), id: "wf_loop_max_iterations_test".to_string(),
            name: "Loop Max Iterations Test".to_string(), description: String::new(),
            nodes: vec![data_source, loop_node, body],
            edges, metadata: Default::default(), max_duration_secs: None, unlimited_duration: false,
            parallel_execution: false, max_concurrent_nodes: None, settings: Default::default(),
        };

        let result = WorkflowExecutor::new(Arc::new(registry), Arc::new(NoopCreds))
            .run(Arc::new(workflow), HashMap::new())
            .await
            .unwrap();

        assert!(result.success, "workflow failed: {:?}", result.error);
        let loop_out = result.node_outputs.get("loop_node").unwrap();
        assert_eq!(
            loop_out["total"], serde_json::json!(2),
            "total must reflect the max_iterations cap (2), not the array's full length (5)"
        );
        let all_results = loop_out["all_results"].as_array().unwrap();
        assert_eq!(
            all_results.len(), 2,
            "body must run exactly max_iterations times (2), not once per array item (5)"
        );
    }

    // Break condition true on iteration 1: single-item array → LoopNode emits the
    // item on iteration 0 (done:false), body runs once, then on iteration 1
    // LoopNode sees current_index >= total and emits done:true.
    // Exactly 1 body execution → all_results.len() == 1.
    #[tokio::test]
    async fn loop_break_condition_true_on_iteration_1_single_item() {
        let mut registry = NodeRegistry::new();
        registry.register(Arc::new(DataSourceNode {
            output: serde_json::json!({ "items": ["only"] }),
        }));
        registry.register(Arc::new(LoopNode));
        registry.register(Arc::new(FixedOutputNode {
            tid: "body_single_test",
            out: serde_json::json!({ "ran": true }),
        }));

        let workflow = loop_workflow_with_bodies(vec![("body_single_test", false)]);

        let result = WorkflowExecutor::new(Arc::new(registry), Arc::new(NoopCreds))
            .run(Arc::new(workflow), HashMap::new())
            .await
            .unwrap();

        assert!(result.success, "workflow failed: {:?}", result.error);
        let loop_out = result.node_outputs.get("loop_node").unwrap();
        let all_results = loop_out["all_results"].as_array().unwrap();
        assert_eq!(
            all_results.len(), 1,
            "single-item array must produce exactly 1 result; break fires on iteration 1"
        );
        assert_eq!(all_results[0], serde_json::json!({ "ran": true }));
    }

    // Output accumulation: all per-iteration outputs present in final result, in order.
    // Uses a CounterNode whose output increments each call so ordering is verifiable.
    #[tokio::test]
    async fn loop_output_accumulation_all_iterations_present_in_order() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct CounterNode { counter: Arc<AtomicUsize> }
        #[async_trait::async_trait]
        impl crate::node::Node for CounterNode {
            fn type_id(&self)        -> &'static str { "counter_loop_test" }
            fn display_name(&self)   -> &'static str { "Counter" }
            fn node_type(&self)      -> crate::model::NodeType { crate::model::NodeType::Utility }
            fn version(&self)        -> &'static str { "1.0" }
            fn input_schema(&self)   -> serde_json::Value { serde_json::json!({}) }
            fn output_schema(&self)  -> serde_json::Value { serde_json::json!({}) }
            async fn execute(&self, _: crate::model::NodeInput) -> crate::model::NodeOutput {
                let n = self.counter.fetch_add(1, Ordering::SeqCst);
                crate::model::NodeOutput::success(serde_json::json!({ "seq": n }))
            }
        }

        let counter = Arc::new(AtomicUsize::new(0));
        let mut registry = NodeRegistry::new();
        registry.register(Arc::new(DataSourceNode {
            output: serde_json::json!({ "items": ["a", "b", "c"] }),
        }));
        registry.register(Arc::new(LoopNode));
        registry.register(Arc::new(CounterNode { counter: counter.clone() }));

        let workflow = loop_workflow_with_bodies(vec![("counter_loop_test", false)]);

        let result = WorkflowExecutor::new(Arc::new(registry), Arc::new(NoopCreds))
            .run(Arc::new(workflow), HashMap::new())
            .await
            .unwrap();

        assert!(result.success, "workflow failed: {:?}", result.error);
        let loop_out = result.node_outputs.get("loop_node").unwrap();
        let all_results = loop_out["all_results"].as_array().unwrap();

        assert_eq!(all_results.len(), 3, "3-item array must produce 3 accumulated results");
        // CounterNode increments per call: seq 0, 1, 2 — order must be preserved.
        assert_eq!(all_results[0]["seq"], 0, "iteration 0 output must be seq=0");
        assert_eq!(all_results[1]["seq"], 1, "iteration 1 output must be seq=1");
        assert_eq!(all_results[2]["seq"], 2, "iteration 2 output must be seq=2");
    }

    // ── If-style branch gating inside a loop body ──────
    //
    // An If/Switch node's branch selection must be honoured for every node in
    // a loop body, not just top-level nodes: only the edge matching the taken
    // branch/port may execute, once per iteration.
    //
    // Wires: loop_body -> cond (always emits branch:"on_true") ->
    //   on_true  -> true_body  (increments true_counter)
    //   on_false -> false_body (increments false_counter)
    // Over 3 iterations, true_counter must be 3 and false_counter must be 0.
    // If branch gating were broken, false_counter would also be 3, since
    // both branches would execute unconditionally every iteration.
    #[tokio::test]
    async fn loop_body_if_branch_gating_only_taken_branch_executes() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct CondNode;
        #[async_trait::async_trait]
        impl Node for CondNode {
            fn type_id(&self) -> &'static str { "cond_branch_loop_test" }
            fn display_name(&self) -> &'static str { "Cond" }
            fn node_type(&self) -> NodeType { NodeType::Logic }
            fn version(&self) -> &'static str { "1.0" }
            fn input_schema(&self) -> serde_json::Value { serde_json::json!({}) }
            fn output_schema(&self) -> serde_json::Value { serde_json::json!({}) }
            async fn execute(&self, _: NodeInput) -> NodeOutput {
                NodeOutput::success(serde_json::json!({ "branch": "on_true" }))
            }
        }

        struct CounterBranchNode { tid: &'static str, counter: Arc<AtomicUsize> }
        #[async_trait::async_trait]
        impl Node for CounterBranchNode {
            fn type_id(&self) -> &'static str { self.tid }
            fn display_name(&self) -> &'static str { "Counter Branch" }
            fn node_type(&self) -> NodeType { NodeType::Utility }
            fn version(&self) -> &'static str { "1.0" }
            fn input_schema(&self) -> serde_json::Value { serde_json::json!({}) }
            fn output_schema(&self) -> serde_json::Value { serde_json::json!({}) }
            async fn execute(&self, _: NodeInput) -> NodeOutput {
                self.counter.fetch_add(1, Ordering::SeqCst);
                NodeOutput::success(serde_json::json!({}))
            }
        }

        let true_counter = Arc::new(AtomicUsize::new(0));
        let false_counter = Arc::new(AtomicUsize::new(0));

        let mut registry = NodeRegistry::new();
        registry.register(Arc::new(DataSourceNode {
            output: serde_json::json!({ "items": [1, 2, 3] }),
        }));
        registry.register(Arc::new(LoopNode));
        registry.register(Arc::new(CondNode));
        registry.register(Arc::new(CounterBranchNode {
            tid: "true_body_branch_test", counter: true_counter.clone(),
        }));
        registry.register(Arc::new(CounterBranchNode {
            tid: "false_body_branch_test", counter: false_counter.clone(),
        }));

        // loop_workflow_with_bodies only wires flat loop_body edges — this test
        // needs cond's on_true/on_false edges, so the workflow is built by hand.
        let data_source = WorkflowNode {
            id: "data_source".to_string(), node_type_id: "data_source_loop_test".to_string(),
            node_type: NodeType::Utility, name: "Data Source".to_string(),
            config: serde_json::json!({}), credentials: HashMap::new(),
            input_schema: serde_json::json!({}), output_schema: serde_json::json!({}),
            retry: Default::default(), fallback_node: None, disabled: false,
            position: Default::default(),
        };
        let loop_node = WorkflowNode {
            id: "loop_node".to_string(), node_type_id: "loop".to_string(),
            node_type: NodeType::Logic, name: "Loop".to_string(),
            config: serde_json::json!({ "array_field": "items", "source_node": "data_source" }),
            credentials: HashMap::new(),
            input_schema: serde_json::json!({}), output_schema: serde_json::json!({}),
            retry: Default::default(), fallback_node: None, disabled: false,
            position: Default::default(),
        };
        let cond = WorkflowNode {
            id: "cond".to_string(), node_type_id: "cond_branch_loop_test".to_string(),
            node_type: NodeType::Logic, name: "Cond".to_string(),
            config: serde_json::json!({}), credentials: HashMap::new(),
            input_schema: serde_json::json!({}), output_schema: serde_json::json!({}),
            retry: Default::default(), fallback_node: None, disabled: false,
            position: Default::default(),
        };
        let true_body = WorkflowNode {
            id: "true_body".to_string(), node_type_id: "true_body_branch_test".to_string(),
            node_type: NodeType::Utility, name: "True Body".to_string(),
            config: serde_json::json!({}), credentials: HashMap::new(),
            input_schema: serde_json::json!({}), output_schema: serde_json::json!({}),
            retry: Default::default(), fallback_node: None, disabled: false,
            position: Default::default(),
        };
        let false_body = WorkflowNode {
            id: "false_body".to_string(), node_type_id: "false_body_branch_test".to_string(),
            node_type: NodeType::Utility, name: "False Body".to_string(),
            config: serde_json::json!({}), credentials: HashMap::new(),
            input_schema: serde_json::json!({}), output_schema: serde_json::json!({}),
            retry: Default::default(), fallback_node: None, disabled: false,
            position: Default::default(),
        };

        let edges = vec![
            WorkflowEdge {
                id: "e1".to_string(), from_node: "data_source".to_string(), from_port: "output".to_string(),
                to_node: "loop_node".to_string(), to_port: "input".to_string(),
                condition: None, on_success: None, on_failure: None,
            },
            WorkflowEdge {
                id: "e2".to_string(), from_node: "loop_node".to_string(), from_port: "loop_body".to_string(),
                to_node: "cond".to_string(), to_port: "input".to_string(),
                condition: None, on_success: None, on_failure: None,
            },
            WorkflowEdge {
                id: "e3".to_string(), from_node: "cond".to_string(), from_port: "on_true".to_string(),
                to_node: "true_body".to_string(), to_port: "input".to_string(),
                condition: None, on_success: None, on_failure: None,
            },
            WorkflowEdge {
                id: "e4".to_string(), from_node: "cond".to_string(), from_port: "on_false".to_string(),
                to_node: "false_body".to_string(), to_port: "input".to_string(),
                condition: None, on_success: None, on_failure: None,
            },
        ];

        let workflow = Workflow {
            schema_version: CURRENT_VERSION.to_string(), id: "wf_loop_branch_test".to_string(),
            name: "Loop Branch Gating Test".to_string(), description: String::new(),
            nodes: vec![data_source, loop_node, cond, true_body, false_body],
            edges, metadata: Default::default(), max_duration_secs: None, unlimited_duration: false,
            parallel_execution: false, max_concurrent_nodes: None, settings: Default::default(),
        };

        let result = WorkflowExecutor::new(Arc::new(registry), Arc::new(NoopCreds))
            .run(Arc::new(workflow), HashMap::new())
            .await
            .unwrap();

        assert!(result.success, "workflow failed: {:?}", result.error);
        assert_eq!(
            true_counter.load(Ordering::SeqCst), 3,
            "on_true branch must fire every iteration (3 items)"
        );
        assert_eq!(
            false_counter.load(Ordering::SeqCst), 0,
            "on_false branch must never fire — cond always takes on_true; broken branch \
             gating would put this at 3 too (both branches running unconditionally every iteration)"
        );
    }

    // ── Switch-style ("port" field) gating in a loop ────
    //
    // Same property, Switch's port shape instead of If's branch shape: a "switch" node
    // emitting `{"port": "case_2"}` must activate only its case_2 edge inside a
    // loop body, not case_1 or default as well.
    #[tokio::test]
    async fn loop_body_switch_port_gating_only_matching_case_executes() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct SwitchNode;
        #[async_trait::async_trait]
        impl Node for SwitchNode {
            fn type_id(&self) -> &'static str { "switch_branch_loop_test" }
            fn display_name(&self) -> &'static str { "Switch" }
            fn node_type(&self) -> NodeType { NodeType::Logic }
            fn version(&self) -> &'static str { "1.0" }
            fn input_schema(&self) -> serde_json::Value { serde_json::json!({}) }
            fn output_schema(&self) -> serde_json::Value { serde_json::json!({}) }
            async fn execute(&self, _: NodeInput) -> NodeOutput {
                NodeOutput::success(serde_json::json!({ "port": "case_2" }))
            }
        }

        struct CounterBranchNode { tid: &'static str, counter: Arc<AtomicUsize> }
        #[async_trait::async_trait]
        impl Node for CounterBranchNode {
            fn type_id(&self) -> &'static str { self.tid }
            fn display_name(&self) -> &'static str { "Counter Branch" }
            fn node_type(&self) -> NodeType { NodeType::Utility }
            fn version(&self) -> &'static str { "1.0" }
            fn input_schema(&self) -> serde_json::Value { serde_json::json!({}) }
            fn output_schema(&self) -> serde_json::Value { serde_json::json!({}) }
            async fn execute(&self, _: NodeInput) -> NodeOutput {
                self.counter.fetch_add(1, Ordering::SeqCst);
                NodeOutput::success(serde_json::json!({}))
            }
        }

        let case1_counter = Arc::new(AtomicUsize::new(0));
        let case2_counter = Arc::new(AtomicUsize::new(0));
        let default_counter = Arc::new(AtomicUsize::new(0));

        let mut registry = NodeRegistry::new();
        registry.register(Arc::new(DataSourceNode {
            output: serde_json::json!({ "items": [1, 2] }),
        }));
        registry.register(Arc::new(LoopNode));
        registry.register(Arc::new(SwitchNode));
        registry.register(Arc::new(CounterBranchNode {
            tid: "case1_branch_test", counter: case1_counter.clone(),
        }));
        registry.register(Arc::new(CounterBranchNode {
            tid: "case2_branch_test", counter: case2_counter.clone(),
        }));
        registry.register(Arc::new(CounterBranchNode {
            tid: "default_branch_test", counter: default_counter.clone(),
        }));

        let data_source = WorkflowNode {
            id: "data_source".to_string(), node_type_id: "data_source_loop_test".to_string(),
            node_type: NodeType::Utility, name: "Data Source".to_string(),
            config: serde_json::json!({}), credentials: HashMap::new(),
            input_schema: serde_json::json!({}), output_schema: serde_json::json!({}),
            retry: Default::default(), fallback_node: None, disabled: false,
            position: Default::default(),
        };
        let loop_node = WorkflowNode {
            id: "loop_node".to_string(), node_type_id: "loop".to_string(),
            node_type: NodeType::Logic, name: "Loop".to_string(),
            config: serde_json::json!({ "array_field": "items", "source_node": "data_source" }),
            credentials: HashMap::new(),
            input_schema: serde_json::json!({}), output_schema: serde_json::json!({}),
            retry: Default::default(), fallback_node: None, disabled: false,
            position: Default::default(),
        };
        let switch = WorkflowNode {
            id: "switch".to_string(), node_type_id: "switch_branch_loop_test".to_string(),
            node_type: NodeType::Logic, name: "Switch".to_string(),
            config: serde_json::json!({}), credentials: HashMap::new(),
            input_schema: serde_json::json!({}), output_schema: serde_json::json!({}),
            retry: Default::default(), fallback_node: None, disabled: false,
            position: Default::default(),
        };
        let mut nodes = vec![data_source, loop_node, switch];
        let mut edges = vec![
            WorkflowEdge {
                id: "e1".to_string(), from_node: "data_source".to_string(), from_port: "output".to_string(),
                to_node: "loop_node".to_string(), to_port: "input".to_string(),
                condition: None, on_success: None, on_failure: None,
            },
            WorkflowEdge {
                id: "e2".to_string(), from_node: "loop_node".to_string(), from_port: "loop_body".to_string(),
                to_node: "switch".to_string(), to_port: "input".to_string(),
                condition: None, on_success: None, on_failure: None,
            },
        ];
        for (case_port, node_id, tid) in [
            ("case_1", "case1_body", "case1_branch_test"),
            ("case_2", "case2_body", "case2_branch_test"),
            ("default", "default_body", "default_branch_test"),
        ] {
            nodes.push(WorkflowNode {
                id: node_id.to_string(), node_type_id: tid.to_string(),
                node_type: NodeType::Utility, name: node_id.to_string(),
                config: serde_json::json!({}), credentials: HashMap::new(),
                input_schema: serde_json::json!({}), output_schema: serde_json::json!({}),
                retry: Default::default(), fallback_node: None, disabled: false,
                position: Default::default(),
            });
            edges.push(WorkflowEdge {
                id: format!("e_{}", case_port),
                from_node: "switch".to_string(), from_port: case_port.to_string(),
                to_node: node_id.to_string(), to_port: "input".to_string(),
                condition: None, on_success: None, on_failure: None,
            });
        }

        let workflow = Workflow {
            schema_version: CURRENT_VERSION.to_string(), id: "wf_loop_switch_branch_test".to_string(),
            name: "Loop Switch Gating Test".to_string(), description: String::new(),
            nodes, edges, metadata: Default::default(), max_duration_secs: None, unlimited_duration: false,
            parallel_execution: false, max_concurrent_nodes: None, settings: Default::default(),
        };

        let result = WorkflowExecutor::new(Arc::new(registry), Arc::new(NoopCreds))
            .run(Arc::new(workflow), HashMap::new())
            .await
            .unwrap();

        assert!(result.success, "workflow failed: {:?}", result.error);
        assert_eq!(case1_counter.load(Ordering::SeqCst), 0, "case_1 must never fire — switch always takes case_2");
        assert_eq!(
            case2_counter.load(Ordering::SeqCst), 2,
            "case_2 must fire every iteration (2 items); broken case gating would have \
             case_1/default fire every iteration too"
        );
        assert_eq!(default_counter.load(Ordering::SeqCst), 0, "default must never fire — switch always takes case_2");
    }

    // ── regression: on_error-port routing keeps the loop alive ────
    //
    // A body node wired with an `on_error` edge to an in-body recovery node
    // must have that edge honoured on failure: the recovery node runs THIS
    // iteration and the loop continues to the next item, instead of the whole
    // loop aborting on the first failure.
    #[tokio::test]
    async fn loop_body_failure_routes_via_on_error_edge_and_loop_completes() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct FailingNode { counter: Arc<AtomicUsize> }
        #[async_trait::async_trait]
        impl Node for FailingNode {
            fn type_id(&self) -> &'static str { "failing_body_on_error_test" }
            fn display_name(&self) -> &'static str { "Failing Body" }
            fn node_type(&self) -> NodeType { NodeType::Utility }
            fn version(&self) -> &'static str { "1.0" }
            fn input_schema(&self) -> serde_json::Value { serde_json::json!({}) }
            fn output_schema(&self) -> serde_json::Value { serde_json::json!({}) }
            async fn execute(&self, _: NodeInput) -> NodeOutput {
                self.counter.fetch_add(1, Ordering::SeqCst);
                NodeOutput::failure(crate::error::NodeError::unrecoverable(
                    "SIMULATED_FAILURE", "intentional test failure",
                ))
            }
        }

        struct RecoveryNode { counter: Arc<AtomicUsize> }
        #[async_trait::async_trait]
        impl Node for RecoveryNode {
            fn type_id(&self) -> &'static str { "recovery_body_on_error_test" }
            fn display_name(&self) -> &'static str { "Recovery" }
            fn node_type(&self) -> NodeType { NodeType::Utility }
            fn version(&self) -> &'static str { "1.0" }
            fn input_schema(&self) -> serde_json::Value { serde_json::json!({}) }
            fn output_schema(&self) -> serde_json::Value { serde_json::json!({}) }
            async fn execute(&self, _: NodeInput) -> NodeOutput {
                self.counter.fetch_add(1, Ordering::SeqCst);
                NodeOutput::success(serde_json::json!({ "recovered": true }))
            }
        }

        let fail_counter = Arc::new(AtomicUsize::new(0));
        let recovery_counter = Arc::new(AtomicUsize::new(0));

        let mut registry = NodeRegistry::new();
        registry.register(Arc::new(DataSourceNode {
            output: serde_json::json!({ "items": [1, 2, 3] }),
        }));
        registry.register(Arc::new(LoopNode));
        registry.register(Arc::new(FailingNode { counter: fail_counter.clone() }));
        registry.register(Arc::new(RecoveryNode { counter: recovery_counter.clone() }));

        let data_source = WorkflowNode {
            id: "data_source".to_string(), node_type_id: "data_source_loop_test".to_string(),
            node_type: NodeType::Utility, name: "Data Source".to_string(),
            config: serde_json::json!({}), credentials: HashMap::new(),
            input_schema: serde_json::json!({}), output_schema: serde_json::json!({}),
            retry: Default::default(), fallback_node: None, disabled: false,
            position: Default::default(),
        };
        let loop_node = WorkflowNode {
            id: "loop_node".to_string(), node_type_id: "loop".to_string(),
            node_type: NodeType::Logic, name: "Loop".to_string(),
            config: serde_json::json!({ "array_field": "items", "source_node": "data_source" }),
            credentials: HashMap::new(),
            input_schema: serde_json::json!({}), output_schema: serde_json::json!({}),
            retry: Default::default(), fallback_node: None, disabled: false,
            position: Default::default(),
        };
        let body = WorkflowNode {
            id: "body".to_string(), node_type_id: "failing_body_on_error_test".to_string(),
            node_type: NodeType::Utility, name: "Body".to_string(),
            config: serde_json::json!({}), credentials: HashMap::new(),
            input_schema: serde_json::json!({}), output_schema: serde_json::json!({}),
            retry: Default::default(), fallback_node: None, disabled: false,
            position: Default::default(),
        };
        let recovery = WorkflowNode {
            id: "recovery".to_string(), node_type_id: "recovery_body_on_error_test".to_string(),
            node_type: NodeType::Utility, name: "Recovery".to_string(),
            config: serde_json::json!({}), credentials: HashMap::new(),
            input_schema: serde_json::json!({}), output_schema: serde_json::json!({}),
            retry: Default::default(), fallback_node: None, disabled: false,
            position: Default::default(),
        };

        let edges = vec![
            WorkflowEdge {
                id: "e_src_loop".to_string(), from_node: "data_source".to_string(), from_port: "output".to_string(),
                to_node: "loop_node".to_string(), to_port: "input".to_string(),
                condition: None, on_success: None, on_failure: None,
            },
            WorkflowEdge {
                id: "e_loop_body".to_string(), from_node: "loop_node".to_string(), from_port: "loop_body".to_string(),
                to_node: "body".to_string(), to_port: "input".to_string(),
                condition: None, on_success: None, on_failure: None,
            },
            WorkflowEdge {
                id: "e_body_on_error".to_string(), from_node: "body".to_string(), from_port: "on_error".to_string(),
                to_node: "recovery".to_string(), to_port: "input".to_string(),
                condition: None, on_success: None, on_failure: None,
            },
        ];

        let workflow = Workflow {
            schema_version: CURRENT_VERSION.to_string(), id: "wf_loop_on_error_test".to_string(),
            name: "Loop On-Error Routing Test".to_string(), description: String::new(),
            nodes: vec![data_source, loop_node, body, recovery],
            edges, metadata: Default::default(), max_duration_secs: None, unlimited_duration: false,
            parallel_execution: false, max_concurrent_nodes: None, settings: Default::default(),
        };

        let result = WorkflowExecutor::new(Arc::new(registry), Arc::new(NoopCreds))
            .run(Arc::new(workflow), HashMap::new())
            .await
            .unwrap();

        assert!(result.success, "workflow must complete via on_error routing, not abort: {:?}", result.error);
        assert_eq!(
            fail_counter.load(Ordering::SeqCst), 3,
            "the failing body must run once per item (3 items) — a lower count means the \
             loop aborted early instead of being routed past the failure"
        );
        assert_eq!(
            recovery_counter.load(Ordering::SeqCst), 3,
            "on_error target must run once per failed item — broken on_error routing would \
             leave this at 0, since the loop would abort on the very first failure before this node could ever run"
        );

        let loop_out = result.node_outputs.get("loop_node").expect("loop_node must have output");
        let all_results = loop_out["all_results"].as_array().expect("all_results must be an array");
        assert_eq!(all_results.len(), 3, "one all_results entry per completed iteration");
    }

    // Cancel fired mid-execute on a loop body node that has an on_error edge
    // wired: the whole run must stop (Err(ExecutionCancelled)), not be routed
    // to the recovery node and continue to the next iteration. No prior test
    // in this module exercised cancellation of a loop body node at all.
    #[tokio::test]
    async fn loop_body_cancel_mid_execute_bypasses_on_error_route() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct SlowBodyNode;
        #[async_trait::async_trait]
        impl Node for SlowBodyNode {
            fn type_id(&self) -> &'static str { "slow_body_cancel_test" }
            fn display_name(&self) -> &'static str { "Slow Body" }
            fn node_type(&self) -> NodeType { NodeType::Utility }
            fn version(&self) -> &'static str { "1.0" }
            fn input_schema(&self) -> serde_json::Value { serde_json::json!({}) }
            fn output_schema(&self) -> serde_json::Value { serde_json::json!({}) }
            async fn execute(&self, _: NodeInput) -> NodeOutput {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                NodeOutput::success(serde_json::json!({}))
            }
        }

        struct RecoveryNode { counter: Arc<AtomicUsize> }
        #[async_trait::async_trait]
        impl Node for RecoveryNode {
            fn type_id(&self) -> &'static str { "recovery_cancel_test" }
            fn display_name(&self) -> &'static str { "Recovery" }
            fn node_type(&self) -> NodeType { NodeType::Utility }
            fn version(&self) -> &'static str { "1.0" }
            fn input_schema(&self) -> serde_json::Value { serde_json::json!({}) }
            fn output_schema(&self) -> serde_json::Value { serde_json::json!({}) }
            async fn execute(&self, _: NodeInput) -> NodeOutput {
                self.counter.fetch_add(1, Ordering::SeqCst);
                NodeOutput::success(serde_json::json!({ "recovered": true }))
            }
        }

        let recovery_counter = Arc::new(AtomicUsize::new(0));

        let mut registry = NodeRegistry::new();
        registry.register(Arc::new(DataSourceNode {
            output: serde_json::json!({ "items": [1, 2, 3] }),
        }));
        registry.register(Arc::new(LoopNode));
        registry.register(Arc::new(SlowBodyNode));
        registry.register(Arc::new(RecoveryNode { counter: recovery_counter.clone() }));

        let data_source = WorkflowNode {
            id: "data_source".to_string(), node_type_id: "data_source_loop_test".to_string(),
            node_type: NodeType::Utility, name: "Data Source".to_string(),
            config: serde_json::json!({}), credentials: HashMap::new(),
            input_schema: serde_json::json!({}), output_schema: serde_json::json!({}),
            retry: Default::default(), fallback_node: None, disabled: false,
            position: Default::default(),
        };
        let loop_node = WorkflowNode {
            id: "loop_node".to_string(), node_type_id: "loop".to_string(),
            node_type: NodeType::Logic, name: "Loop".to_string(),
            config: serde_json::json!({ "array_field": "items", "source_node": "data_source" }),
            credentials: HashMap::new(),
            input_schema: serde_json::json!({}), output_schema: serde_json::json!({}),
            retry: Default::default(), fallback_node: None, disabled: false,
            position: Default::default(),
        };
        let body = WorkflowNode {
            id: "body".to_string(), node_type_id: "slow_body_cancel_test".to_string(),
            node_type: NodeType::Utility, name: "Body".to_string(),
            config: serde_json::json!({}), credentials: HashMap::new(),
            input_schema: serde_json::json!({}), output_schema: serde_json::json!({}),
            retry: Default::default(), fallback_node: None, disabled: false,
            position: Default::default(),
        };
        let recovery = WorkflowNode {
            id: "recovery".to_string(), node_type_id: "recovery_cancel_test".to_string(),
            node_type: NodeType::Utility, name: "Recovery".to_string(),
            config: serde_json::json!({}), credentials: HashMap::new(),
            input_schema: serde_json::json!({}), output_schema: serde_json::json!({}),
            retry: Default::default(), fallback_node: None, disabled: false,
            position: Default::default(),
        };

        let edges = vec![
            WorkflowEdge {
                id: "e_src_loop".to_string(), from_node: "data_source".to_string(), from_port: "output".to_string(),
                to_node: "loop_node".to_string(), to_port: "input".to_string(),
                condition: None, on_success: None, on_failure: None,
            },
            WorkflowEdge {
                id: "e_loop_body".to_string(), from_node: "loop_node".to_string(), from_port: "loop_body".to_string(),
                to_node: "body".to_string(), to_port: "input".to_string(),
                condition: None, on_success: None, on_failure: None,
            },
            WorkflowEdge {
                id: "e_body_on_error".to_string(), from_node: "body".to_string(), from_port: "on_error".to_string(),
                to_node: "recovery".to_string(), to_port: "input".to_string(),
                condition: None, on_success: None, on_failure: None,
            },
        ];

        let workflow = Workflow {
            schema_version: CURRENT_VERSION.to_string(), id: "wf_loop_cancel_test".to_string(),
            name: "Loop Body Cancel Test".to_string(), description: String::new(),
            nodes: vec![data_source, loop_node, body, recovery],
            edges, metadata: Default::default(), max_duration_secs: None, unlimited_duration: false,
            parallel_execution: false, max_concurrent_nodes: None, settings: Default::default(),
        };

        let token = CancellationToken::new();
        let cancel_token = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            cancel_token.cancel();
        });

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            WorkflowExecutor::new(Arc::new(registry), Arc::new(NoopCreds))
                .with_cancel_token(token)
                .run(Arc::new(workflow), HashMap::new()),
        ).await;

        let result = result.expect(
            "run() did not return within 5s — cancellation did not interrupt the in-flight body node"
        );
        assert!(
            matches!(result, Err(EngineError::ExecutionCancelled)),
            "expected ExecutionCancelled (on_error route must not mask a cancel); got {:?}", result
        );
        assert_eq!(
            recovery_counter.load(Ordering::SeqCst), 0,
            "recovery must never run — a cancelled body node must not be routed like an ordinary failure"
        );
    }

    // Cancel fired mid-execute on the loop node's own execute() call (the
    // condition/iteration-control call, not a body node) must interrupt
    // promptly instead of waiting for that call to return. No prior test in
    // this module exercised cancellation at this call site — only the
    // between-iteration check (line 142) and body-node cancellation
    // (previous test) had coverage.
    #[tokio::test]
    async fn loop_node_execute_cancel_mid_call_returns_cancelled() {
        struct SlowLoopNode;
        #[async_trait::async_trait]
        impl Node for SlowLoopNode {
            fn type_id(&self) -> &'static str { "loop" }
            fn display_name(&self) -> &'static str { "Slow Loop" }
            fn node_type(&self) -> NodeType { NodeType::Logic }
            fn version(&self) -> &'static str { "1.0" }
            fn input_schema(&self) -> serde_json::Value { serde_json::json!({}) }
            fn output_schema(&self) -> serde_json::Value { serde_json::json!({}) }
            async fn execute(&self, _: NodeInput) -> NodeOutput {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                NodeOutput::success(serde_json::json!({ "done": true, "all_results": [] }))
            }
        }

        let mut registry = NodeRegistry::new();
        registry.register(Arc::new(DataSourceNode {
            output: serde_json::json!({ "items": [1, 2, 3] }),
        }));
        registry.register(Arc::new(SlowLoopNode));

        let data_source = WorkflowNode {
            id: "data_source".to_string(), node_type_id: "data_source_loop_test".to_string(),
            node_type: NodeType::Utility, name: "Data Source".to_string(),
            config: serde_json::json!({}), credentials: HashMap::new(),
            input_schema: serde_json::json!({}), output_schema: serde_json::json!({}),
            retry: Default::default(), fallback_node: None, disabled: false,
            position: Default::default(),
        };
        let loop_node = WorkflowNode {
            id: "loop_node".to_string(), node_type_id: "loop".to_string(),
            node_type: NodeType::Logic, name: "Loop".to_string(),
            config: serde_json::json!({ "array_field": "items", "source_node": "data_source" }),
            credentials: HashMap::new(),
            input_schema: serde_json::json!({}), output_schema: serde_json::json!({}),
            retry: Default::default(), fallback_node: None, disabled: false,
            position: Default::default(),
        };

        let edges = vec![
            WorkflowEdge {
                id: "e_src_loop".to_string(), from_node: "data_source".to_string(), from_port: "output".to_string(),
                to_node: "loop_node".to_string(), to_port: "input".to_string(),
                condition: None, on_success: None, on_failure: None,
            },
        ];

        let workflow = Workflow {
            schema_version: CURRENT_VERSION.to_string(), id: "wf_loop_node_cancel_test".to_string(),
            name: "Loop Node Execute Cancel Test".to_string(), description: String::new(),
            nodes: vec![data_source, loop_node],
            edges, metadata: Default::default(), max_duration_secs: None, unlimited_duration: false,
            parallel_execution: false, max_concurrent_nodes: None, settings: Default::default(),
        };

        let token = CancellationToken::new();
        let cancel_token = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            cancel_token.cancel();
        });

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            WorkflowExecutor::new(Arc::new(registry), Arc::new(NoopCreds))
                .with_cancel_token(token)
                .run(Arc::new(workflow), HashMap::new()),
        ).await;

        let result = result.expect(
            "run() did not return within 5s — cancellation did not interrupt the loop node's own execute() call"
        );
        assert!(
            matches!(result, Err(EngineError::ExecutionCancelled)),
            "expected ExecutionCancelled; got {:?}", result
        );
    }

    // ── regression: WorkflowEdge.on_failure field routing ─────────
    //
    // Same scenario as above, but via the other mechanism top-level nodes get
    // (executor/mod.rs::find_failure_route) — an `on_failure` pointer on an
    // ordinary edge, rather than a dedicated `on_error`-port edge. Mirrors the
    // edge shape executor/mod.rs's own `find_failure_route` tests use: the
    // field lives on body's normal "output" edge and is never taken via that
    // edge's own port (body always fails, so "output" is never the taken port).
    #[tokio::test]
    async fn loop_body_failure_routes_via_on_failure_field_and_loop_completes() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct FailingNode { counter: Arc<AtomicUsize> }
        #[async_trait::async_trait]
        impl Node for FailingNode {
            fn type_id(&self) -> &'static str { "failing_body_on_failure_test" }
            fn display_name(&self) -> &'static str { "Failing Body" }
            fn node_type(&self) -> NodeType { NodeType::Utility }
            fn version(&self) -> &'static str { "1.0" }
            fn input_schema(&self) -> serde_json::Value { serde_json::json!({}) }
            fn output_schema(&self) -> serde_json::Value { serde_json::json!({}) }
            async fn execute(&self, _: NodeInput) -> NodeOutput {
                self.counter.fetch_add(1, Ordering::SeqCst);
                NodeOutput::failure(crate::error::NodeError::unrecoverable(
                    "SIMULATED_FAILURE", "intentional test failure",
                ))
            }
        }

        struct RecoveryNode { counter: Arc<AtomicUsize> }
        #[async_trait::async_trait]
        impl Node for RecoveryNode {
            fn type_id(&self) -> &'static str { "recovery_body_on_failure_test" }
            fn display_name(&self) -> &'static str { "Recovery" }
            fn node_type(&self) -> NodeType { NodeType::Utility }
            fn version(&self) -> &'static str { "1.0" }
            fn input_schema(&self) -> serde_json::Value { serde_json::json!({}) }
            fn output_schema(&self) -> serde_json::Value { serde_json::json!({}) }
            async fn execute(&self, _: NodeInput) -> NodeOutput {
                self.counter.fetch_add(1, Ordering::SeqCst);
                NodeOutput::success(serde_json::json!({ "recovered": true }))
            }
        }

        let fail_counter = Arc::new(AtomicUsize::new(0));
        let recovery_counter = Arc::new(AtomicUsize::new(0));

        let mut registry = NodeRegistry::new();
        registry.register(Arc::new(DataSourceNode {
            output: serde_json::json!({ "items": ["a", "b"] }),
        }));
        registry.register(Arc::new(LoopNode));
        registry.register(Arc::new(FailingNode { counter: fail_counter.clone() }));
        registry.register(Arc::new(RecoveryNode { counter: recovery_counter.clone() }));

        let data_source = WorkflowNode {
            id: "data_source".to_string(), node_type_id: "data_source_loop_test".to_string(),
            node_type: NodeType::Utility, name: "Data Source".to_string(),
            config: serde_json::json!({}), credentials: HashMap::new(),
            input_schema: serde_json::json!({}), output_schema: serde_json::json!({}),
            retry: Default::default(), fallback_node: None, disabled: false,
            position: Default::default(),
        };
        let loop_node = WorkflowNode {
            id: "loop_node".to_string(), node_type_id: "loop".to_string(),
            node_type: NodeType::Logic, name: "Loop".to_string(),
            config: serde_json::json!({ "array_field": "items", "source_node": "data_source" }),
            credentials: HashMap::new(),
            input_schema: serde_json::json!({}), output_schema: serde_json::json!({}),
            retry: Default::default(), fallback_node: None, disabled: false,
            position: Default::default(),
        };
        let body = WorkflowNode {
            id: "body".to_string(), node_type_id: "failing_body_on_failure_test".to_string(),
            node_type: NodeType::Utility, name: "Body".to_string(),
            config: serde_json::json!({}), credentials: HashMap::new(),
            input_schema: serde_json::json!({}), output_schema: serde_json::json!({}),
            retry: Default::default(), fallback_node: None, disabled: false,
            position: Default::default(),
        };
        let recovery = WorkflowNode {
            id: "recovery".to_string(), node_type_id: "recovery_body_on_failure_test".to_string(),
            node_type: NodeType::Utility, name: "Recovery".to_string(),
            config: serde_json::json!({}), credentials: HashMap::new(),
            input_schema: serde_json::json!({}), output_schema: serde_json::json!({}),
            retry: Default::default(), fallback_node: None, disabled: false,
            position: Default::default(),
        };

        let edges = vec![
            WorkflowEdge {
                id: "e_src_loop".to_string(), from_node: "data_source".to_string(), from_port: "output".to_string(),
                to_node: "loop_node".to_string(), to_port: "input".to_string(),
                condition: None, on_success: None, on_failure: None,
            },
            WorkflowEdge {
                id: "e_loop_body".to_string(), from_node: "loop_node".to_string(), from_port: "loop_body".to_string(),
                to_node: "body".to_string(), to_port: "input".to_string(),
                condition: None, on_success: None, on_failure: None,
            },
            WorkflowEdge {
                id: "e_body_output".to_string(), from_node: "body".to_string(), from_port: "output".to_string(),
                to_node: "recovery".to_string(), to_port: "input".to_string(),
                condition: None, on_success: None, on_failure: Some("recovery".to_string()),
            },
        ];

        let workflow = Workflow {
            schema_version: CURRENT_VERSION.to_string(), id: "wf_loop_on_failure_test".to_string(),
            name: "Loop On-Failure Field Routing Test".to_string(), description: String::new(),
            nodes: vec![data_source, loop_node, body, recovery],
            edges, metadata: Default::default(), max_duration_secs: None, unlimited_duration: false,
            parallel_execution: false, max_concurrent_nodes: None, settings: Default::default(),
        };

        let result = WorkflowExecutor::new(Arc::new(registry), Arc::new(NoopCreds))
            .run(Arc::new(workflow), HashMap::new())
            .await
            .unwrap();

        assert!(result.success, "workflow must complete via on_failure routing, not abort: {:?}", result.error);
        assert_eq!(
            fail_counter.load(Ordering::SeqCst), 2,
            "the failing body must run once per item (2 items)"
        );
        assert_eq!(
            recovery_counter.load(Ordering::SeqCst), 2,
            "on_failure target must run once per failed item — broken on_failure routing would \
             leave this at 0, since the loop would abort on the very first failure before this node could ever run"
        );
    }

    // ── regression guard: unrouted failure still aborts the loop ──
    //
    // A body node with NEITHER an on_error edge NOR an on_failure field must
    // still abort the whole loop on its first failure.
    #[tokio::test]
    async fn loop_body_failure_with_no_routing_still_aborts_whole_loop() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct FailingNode { counter: Arc<AtomicUsize> }
        #[async_trait::async_trait]
        impl Node for FailingNode {
            fn type_id(&self) -> &'static str { "failing_body_unrouted_test" }
            fn display_name(&self) -> &'static str { "Failing Body" }
            fn node_type(&self) -> NodeType { NodeType::Utility }
            fn version(&self) -> &'static str { "1.0" }
            fn input_schema(&self) -> serde_json::Value { serde_json::json!({}) }
            fn output_schema(&self) -> serde_json::Value { serde_json::json!({}) }
            async fn execute(&self, _: NodeInput) -> NodeOutput {
                self.counter.fetch_add(1, Ordering::SeqCst);
                NodeOutput::failure(crate::error::NodeError::unrecoverable(
                    "SIMULATED_FAILURE", "intentional test failure",
                ))
            }
        }

        let fail_counter = Arc::new(AtomicUsize::new(0));

        let mut registry = NodeRegistry::new();
        registry.register(Arc::new(DataSourceNode {
            output: serde_json::json!({ "items": [1, 2, 3] }),
        }));
        registry.register(Arc::new(LoopNode));
        registry.register(Arc::new(FailingNode { counter: fail_counter.clone() }));

        let workflow = loop_workflow_with_bodies(vec![
            ("failing_body_unrouted_test", false),
        ]);

        let result = WorkflowExecutor::new(Arc::new(registry), Arc::new(NoopCreds))
            .run(Arc::new(workflow), HashMap::new())
            .await
            .unwrap();

        assert!(
            !result.success,
            "an unrouted body-node failure must still abort the whole loop"
        );
        assert_eq!(
            fail_counter.load(Ordering::SeqCst), 1,
            "the loop must abort on the FIRST failing item, not continue to remaining items — \
             a count > 1 here would mean the fix started silently swallowing unrouted \
             failures instead of preserving the original abort-on-first-failure contract"
        );
    }

    // ── mixed in-body / out-of-body on_error targets ──────────
    //
    // A body node's on_error port can carry more than one edge. If one target
    // is inside the loop body (routable) and another resolves to a node
    // outside it (e.g. also reachable via the loop's own "done" port, so
    // collect_loop_body_nodes correctly excludes it from the body), the
    // in-body target must still run every iteration, and the out-of-body
    // target must be skipped rather than silently or incorrectly re-run
    // per-iteration.
    #[tokio::test]
    async fn loop_body_mixed_on_error_targets_routes_in_body_and_skips_out_of_body() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct FailingNode { counter: Arc<AtomicUsize> }
        #[async_trait::async_trait]
        impl Node for FailingNode {
            fn type_id(&self) -> &'static str { "failing_body_mixed_test" }
            fn display_name(&self) -> &'static str { "Failing Body" }
            fn node_type(&self) -> NodeType { NodeType::Utility }
            fn version(&self) -> &'static str { "1.0" }
            fn input_schema(&self) -> serde_json::Value { serde_json::json!({}) }
            fn output_schema(&self) -> serde_json::Value { serde_json::json!({}) }
            async fn execute(&self, _: NodeInput) -> NodeOutput {
                self.counter.fetch_add(1, Ordering::SeqCst);
                NodeOutput::failure(crate::error::NodeError::unrecoverable(
                    "SIMULATED_FAILURE", "intentional test failure",
                ))
            }
        }

        struct CountingNode { tid: &'static str, counter: Arc<AtomicUsize> }
        #[async_trait::async_trait]
        impl Node for CountingNode {
            fn type_id(&self) -> &'static str { self.tid }
            fn display_name(&self) -> &'static str { "Counting" }
            fn node_type(&self) -> NodeType { NodeType::Utility }
            fn version(&self) -> &'static str { "1.0" }
            fn input_schema(&self) -> serde_json::Value { serde_json::json!({}) }
            fn output_schema(&self) -> serde_json::Value { serde_json::json!({}) }
            async fn execute(&self, _: NodeInput) -> NodeOutput {
                self.counter.fetch_add(1, Ordering::SeqCst);
                NodeOutput::success(serde_json::json!({}))
            }
        }

        let fail_counter = Arc::new(AtomicUsize::new(0));
        let recovery_counter = Arc::new(AtomicUsize::new(0));
        let after_loop_counter = Arc::new(AtomicUsize::new(0));

        let mut registry = NodeRegistry::new();
        registry.register(Arc::new(DataSourceNode {
            output: serde_json::json!({ "items": [1, 2] }),
        }));
        registry.register(Arc::new(LoopNode));
        registry.register(Arc::new(FailingNode { counter: fail_counter.clone() }));
        registry.register(Arc::new(CountingNode {
            tid: "recovery_body_mixed_test", counter: recovery_counter.clone(),
        }));
        registry.register(Arc::new(CountingNode {
            tid: "after_loop_mixed_test", counter: after_loop_counter.clone(),
        }));

        let data_source = WorkflowNode {
            id: "data_source".to_string(), node_type_id: "data_source_loop_test".to_string(),
            node_type: NodeType::Utility, name: "Data Source".to_string(),
            config: serde_json::json!({}), credentials: HashMap::new(),
            input_schema: serde_json::json!({}), output_schema: serde_json::json!({}),
            retry: Default::default(), fallback_node: None, disabled: false,
            position: Default::default(),
        };
        let loop_node = WorkflowNode {
            id: "loop_node".to_string(), node_type_id: "loop".to_string(),
            node_type: NodeType::Logic, name: "Loop".to_string(),
            config: serde_json::json!({ "array_field": "items", "source_node": "data_source" }),
            credentials: HashMap::new(),
            input_schema: serde_json::json!({}), output_schema: serde_json::json!({}),
            retry: Default::default(), fallback_node: None, disabled: false,
            position: Default::default(),
        };
        let body = WorkflowNode {
            id: "body".to_string(), node_type_id: "failing_body_mixed_test".to_string(),
            node_type: NodeType::Utility, name: "Body".to_string(),
            config: serde_json::json!({}), credentials: HashMap::new(),
            input_schema: serde_json::json!({}), output_schema: serde_json::json!({}),
            retry: Default::default(), fallback_node: None, disabled: false,
            position: Default::default(),
        };
        let recovery = WorkflowNode {
            id: "recovery".to_string(), node_type_id: "recovery_body_mixed_test".to_string(),
            node_type: NodeType::Utility, name: "Recovery".to_string(),
            config: serde_json::json!({}), credentials: HashMap::new(),
            input_schema: serde_json::json!({}), output_schema: serde_json::json!({}),
            retry: Default::default(), fallback_node: None, disabled: false,
            position: Default::default(),
        };
        let after_loop = WorkflowNode {
            id: "after_loop".to_string(), node_type_id: "after_loop_mixed_test".to_string(),
            node_type: NodeType::Utility, name: "After Loop".to_string(),
            config: serde_json::json!({}), credentials: HashMap::new(),
            input_schema: serde_json::json!({}), output_schema: serde_json::json!({}),
            retry: Default::default(), fallback_node: None, disabled: false,
            position: Default::default(),
        };

        let edges = vec![
            WorkflowEdge {
                id: "e_src_loop".to_string(), from_node: "data_source".to_string(), from_port: "output".to_string(),
                to_node: "loop_node".to_string(), to_port: "input".to_string(),
                condition: None, on_success: None, on_failure: None,
            },
            WorkflowEdge {
                id: "e_loop_body".to_string(), from_node: "loop_node".to_string(), from_port: "loop_body".to_string(),
                to_node: "body".to_string(), to_port: "input".to_string(),
                condition: None, on_success: None, on_failure: None,
            },
            // Establishes after_loop as a "done"-port node — collect_loop_body_nodes
            // excludes anything reachable via loop_node's "done" port from the body
            // set, regardless of any other edge (body's second on_error edge, below)
            // also pointing at it.
            WorkflowEdge {
                id: "e_loop_done".to_string(), from_node: "loop_node".to_string(), from_port: "done".to_string(),
                to_node: "after_loop".to_string(), to_port: "input".to_string(),
                condition: None, on_success: None, on_failure: None,
            },
            // Two on_error targets from the same body node: one in-body
            // (recovery), one that resolves to the done-excluded after_loop node.
            WorkflowEdge {
                id: "e_body_on_error_1".to_string(), from_node: "body".to_string(), from_port: "on_error".to_string(),
                to_node: "recovery".to_string(), to_port: "input".to_string(),
                condition: None, on_success: None, on_failure: None,
            },
            WorkflowEdge {
                id: "e_body_on_error_2".to_string(), from_node: "body".to_string(), from_port: "on_error".to_string(),
                to_node: "after_loop".to_string(), to_port: "input".to_string(),
                condition: None, on_success: None, on_failure: None,
            },
        ];

        let workflow = Workflow {
            schema_version: CURRENT_VERSION.to_string(), id: "wf_loop_mixed_on_error_test".to_string(),
            name: "Loop Mixed On-Error Targets Test".to_string(), description: String::new(),
            nodes: vec![data_source, loop_node, body, recovery, after_loop],
            edges, metadata: Default::default(), max_duration_secs: None, unlimited_duration: false,
            parallel_execution: false, max_concurrent_nodes: None, settings: Default::default(),
        };

        let result = WorkflowExecutor::new(Arc::new(registry), Arc::new(NoopCreds))
            .run(Arc::new(workflow), HashMap::new())
            .await
            .unwrap();

        assert!(
            result.success,
            "workflow must complete via the in-body on_error target, not abort: {:?}", result.error
        );
        assert_eq!(
            fail_counter.load(Ordering::SeqCst), 2,
            "the failing body must run once per item (2 items)"
        );
        assert_eq!(
            recovery_counter.load(Ordering::SeqCst), 2,
            "the in-body on_error target must still run every time despite a second, \
             out-of-body on_error target existing on the same port"
        );
        assert_eq!(
            after_loop_counter.load(Ordering::SeqCst), 1,
            "after_loop must run exactly once — via the loop's normal top-level \"done\" \
             routing after all iterations finish, NOT once per iteration via the body's \
             on_error edge (which must be skipped: after_loop is outside the loop body)"
        );
    }
}
