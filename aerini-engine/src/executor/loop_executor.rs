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
            let loop_input = match self.build_input(workflow, loop_node_def, state).await {
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

            // Tracks this iteration's result for all_results. Original behavior (before
            // this fix) stored each body node's output under the SAME key
            // `__loop_{id}_result_{iteration}` — so only the LAST body node's output
            // survived per iteration (one entry per iteration, k-body-node loops still
            // contributed exactly one entry). Preserved here via overwrite-then-push-once,
            // instead of pushing on every body node (which would wrongly produce k×n entries).
            let mut iteration_result: Option<serde_json::Value> = None;

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

                let body_input = match self.build_input(workflow, body_def, state).await {
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

                // Track this iteration's result (overwrite — last body node wins,
                // matching the original per-iteration key-collision semantics).
                if let Some(ref val) = body_output.output {
                    iteration_result = Some(val.clone());
                }
                state.write().await.mark_succeeded(body_id, body_output);
                self.emit_node_status(&workflow.id, body_id, "success");
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
    use super::super::{CredentialResolver, WorkflowExecutor};
    use crate::migration::CURRENT_VERSION;
    use crate::model::{NodeInput, NodeOutput, NodeType, Workflow, WorkflowEdge, WorkflowNode};
    use crate::node::{Node, NodeRegistry};
    use crate::nodes::loop_node::LoopNode;
    use std::collections::HashMap;
    use std::sync::Arc;

    // ── Credentials stub ──────────────────────────────────────────────────────

    struct NoopCreds;
    #[async_trait::async_trait]
    impl CredentialResolver for NoopCreds {
        async fn resolve(&self, _: &str) -> Option<String> {
            None
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
    // that replaced the pre-fix unconditional push is behaving correctly.
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
        // Pre-fix behaviour (unconditional push) would yield [null, null].
        assert!(
            all_results.is_empty(),
            "expected empty all_results when every body returns output:None; \
             got {} entries — null values are being pushed incorrectly: {:?}",
            all_results.len(), all_results
        );
    }

    // ── P23: loop_executor.rs tests ───────────────────────────────────────────

    // Max iterations enforced: LoopNode hard-caps at 10,000 items (ARRAY_TOO_LARGE).
    // NOTE: LoopNode has no configurable max_iterations field — the limit is hardcoded
    // at 10,000 items in LoopNode.execute() and 10,001 iterations in execute_loop_node.
    // This test exercises the accessible cap: 10,001-item array → ARRAY_TOO_LARGE.
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
}
