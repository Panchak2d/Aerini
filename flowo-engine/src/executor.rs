//! Workflow executor.
//!
//! [`WorkflowExecutor`] runs a single workflow to completion:
//!
//! 1. Builds an [`crate::graph::ExecutionGraph`] (topological sort, cycle check).
//! 2. Walks nodes in topo order, resolving `{{...}}` expressions before each execution.
//! 3. Handles retries (per-node `RetryPolicy`), disabled nodes, loop nodes, and
//!    fallback routing (`on_success` / `on_failure` edges).
//! 4. Emits node-status events to the [`crate::EventSink`] after each node completes.
//!
//! # Credential resolution
//!
//! The executor holds an `Arc<dyn CredentialResolver>`. In the desktop app this is
//! `StoreCredentialResolver` (reads from the encrypted SQLite store). In the server
//! binary it is `EnvCredentialResolver` (reads from environment variables). Nodes
//! receive resolved values — they never see credential IDs or the store directly.

use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use tokio_util::sync::CancellationToken;

use crate::context::{new_shared_state, LogLevel, SharedExecutionState};
use crate::error::{EngineError, NodeError};
use crate::graph::ExecutionGraph;
use crate::model::{NodeInput, NodeOutput, Workflow};
use crate::node::{Node, NodeRegistry};
use crate::EventSink;

// ── Execution result ──────────────────────────────────────────────────────────

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct WorkflowResult {
    pub execution_id: String,
    pub workflow_id: String,
    pub success: bool,
    pub node_outputs: HashMap<String, Value>,
    pub logs: Vec<crate::context::ExecutionLogEntry>,
    pub error: Option<String>,
    /// Schema validation warnings (or errors in strict mode) encountered during execution.
    /// Empty when strict_schema_validation is false and all schemas are clean.
    #[serde(default)]
    pub validation_errors: Vec<String>,
}

// ── Credential resolver ───────────────────────────────────────────────────────

#[async_trait::async_trait]
pub trait CredentialResolver: Send + Sync + 'static {
    async fn resolve(&self, credential_id: &str) -> Option<String>;
}

// ── Executor ──────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct WorkflowExecutor {
    registry:            Arc<NodeRegistry>,
    credential_resolver: Arc<dyn CredentialResolver>,
    // Optional: when None, node-status events are dropped silently.
    event_sink:          Option<Arc<dyn EventSink>>,
    // Optional: when None, {{$env.VAR}} expressions resolve to "" with a warning.
    env_allowlist:       Option<Arc<std::collections::HashSet<String>>>,
    // Optional: when set, FileNode restricts all paths to this directory tree.
    file_sandbox_dir:    Option<Arc<std::path::PathBuf>>,
    // When true, ShellExecNode returns SHELL_DISABLED immediately without executing.
    shell_exec_disabled: bool,
    // When true, CodeNode returns CODE_DISABLED immediately without executing.
    code_exec_disabled:  bool,
    // When true, input schema violations fail the node. When false (default), they log a warning.
    strict_schema_validation: bool,
    // When true, independent branches execute concurrently via tokio tasks. Default: false.
    parallel_execution: bool,
    // Maximum simultaneous node tasks when parallel_execution is true. Default: 8.
    max_concurrent_nodes: usize,
    // When set, the executor checks this token between nodes and short-circuits retry backoff sleeps.
    cancel_token: Option<CancellationToken>,
    // Server-level ceiling applied to every workflow regardless of per-workflow max_duration_secs.
    server_max_duration_secs: Option<u64>,
}

impl WorkflowExecutor {
    pub fn new(
        registry:            Arc<NodeRegistry>,
        credential_resolver: Arc<dyn CredentialResolver>,
    ) -> Self {
        Self { registry, credential_resolver, event_sink: None, env_allowlist: None, file_sandbox_dir: None, shell_exec_disabled: false, code_exec_disabled: false, strict_schema_validation: false, parallel_execution: false, max_concurrent_nodes: 8, cancel_token: None, server_max_duration_secs: None }
    }

    /// Restrict `{{$env.VAR}}` expressions to the listed variable names.
    /// Call with an empty vec to allow no env vars. Omit entirely to disable $env.
    pub fn with_env_allowlist(mut self, vars: Vec<String>) -> Self {
        self.env_allowlist = Some(Arc::new(vars.into_iter().collect()));
        self
    }

    /// Attach an event sink. Fluent builder — allows optional chaining.
    pub fn with_event_sink(mut self, sink: Arc<dyn EventSink>) -> Self {
        self.event_sink = Some(sink);
        self
    }

    /// Restrict FileNode to paths within `dir`. Required in server/API mode to
    /// prevent authenticated users from reading arbitrary filesystem paths.
    pub fn with_file_sandbox_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.file_sandbox_dir = Some(Arc::new(dir));
        self
    }

    /// Disable the Shell Command node. When true, any ShellExecNode returns a
    /// SHELL_DISABLED error immediately without spawning a subprocess.
    /// Recommended for API mode deployments where arbitrary shell access is undesirable.
    pub fn with_shell_disabled(mut self, disabled: bool) -> Self {
        self.shell_exec_disabled = disabled;
        self
    }

    /// Disable the Code (JS) node. When true, any CodeNode returns a
    /// CODE_DISABLED error immediately without spawning a Node.js process.
    /// Recommended for API mode deployments where arbitrary JS execution is undesirable.
    pub fn with_code_disabled(mut self, disabled: bool) -> Self {
        self.code_exec_disabled = disabled;
        self
    }

    /// When true, any node whose resolved input fails its declared JSON Schema causes the
    /// node to fail immediately (SCHEMA_VIOLATION). When false (default), violations are
    /// logged as warnings and execution continues — safe upgrade path for existing workflows.
    pub fn with_strict_schema_validation(mut self, strict: bool) -> Self {
        self.strict_schema_validation = strict;
        self
    }

    /// Enable parallel execution of independent branches.
    /// When true, nodes whose upstream dependencies have all completed are run
    /// concurrently via tokio tasks instead of sequentially. Default: false.
    /// The sequential path is unchanged when this is false.
    pub fn with_parallel_execution(mut self, enabled: bool) -> Self {
        self.parallel_execution = enabled;
        self
    }

    /// Maximum number of node tasks running simultaneously in parallel mode.
    /// Values below 1 are clamped to 1. Default: 8.
    pub fn with_max_concurrent_nodes(mut self, limit: usize) -> Self {
        self.max_concurrent_nodes = limit.max(1);
        self
    }

    /// Attach a cancellation token. When cancelled, the executor stops between
    /// nodes and short-circuits retry backoff sleeps. No-op if never cancelled.
    pub fn with_cancel_token(mut self, t: CancellationToken) -> Self {
        self.cancel_token = Some(t);
        self
    }

    /// Set a server-level ceiling on workflow execution time (seconds).
    /// Takes effect as `min(workflow.max_duration_secs, ceiling)` when both are set.
    /// When only this ceiling is set it applies to every workflow run by this executor.
    /// Values below 10 are clamped to 10; values above 86400 are clamped to 86400.
    pub fn with_server_max_duration_secs(mut self, secs: Option<u64>) -> Self {
        self.server_max_duration_secs = secs.map(|s| s.clamp(10, 86400));
        self
    }

    fn emit_node_status(&self, workflow_id: &str, node_id: &str, status: &str) {
        if let Some(ref sink) = self.event_sink {
            sink.emit("node-status", serde_json::json!({
                "workflow_id": workflow_id,
                "node_id":     node_id,
                "status":      status,
            }));
        }
    }

    // ── Schema validation ─────────────────────────────────────────────────────

    /// Validate `input` against `schema`. Returns an empty Vec on success.
    /// Skips validation when schema is null, not an object, or an empty object `{}`.
    /// When the schema itself is invalid (compile error), returns a single warning
    /// string rather than panicking — execution continues.
    fn validate_node_input(schema: &Value, input: &Value) -> Vec<String> {
        // Skip empty / null schemas — no constraints declared.
        match schema {
            Value::Null => return vec![],
            Value::Object(m) if m.is_empty() => return vec![],
            Value::Object(_) => {}
            _ => return vec![],
        }

        let validator = match jsonschema::options()
            .with_draft(jsonschema::Draft::Draft7)
            .build(schema)
        {
            Ok(v) => v,
            Err(e) => {
                return vec![format!(
                    "Schema compile error (validation skipped): {}",
                    e
                )];
            }
        };

        validator
            .iter_errors(input)
            .map(|e| format!("{} (at path: {})", e, e.instance_path()))
            .collect()
    }

    /// Run schema validation for `node_id`'s input.
    /// - Non-strict (default): log warnings, return None (execution continues).
    /// - Strict: return Some(failure NodeOutput) — caller treats it as a node failure.
    async fn check_schema(
        &self,
        schema:   &Value,
        input:    &Value,
        node_id:  &str,
        state:    &SharedExecutionState,
        warnings: &mut Vec<String>,
    ) -> Option<crate::model::NodeOutput> {
        let errors = Self::validate_node_input(schema, input);
        if errors.is_empty() {
            return None;
        }

        if self.strict_schema_validation {
            let reason = errors.join("; ");
            Some(crate::model::NodeOutput::failure(
                crate::error::NodeError::unrecoverable(
                    "SCHEMA_VIOLATION",
                    format!(
                        "Input schema validation failed for node '{}': {}",
                        node_id, reason
                    ),
                ),
            ))
        } else {
            let mut s = state.write().await;
            for msg in &errors {
                let w = format!("[Schema] node '{}': {}", node_id, msg);
                s.log(Some(node_id), LogLevel::Warn, w.clone());
                warnings.push(w);
            }
            None
        }
    }

    pub async fn run(
        &self,
        workflow: &Workflow,
        initial_variables: HashMap<String, Value>,
    ) -> Result<WorkflowResult, EngineError> {
        let limit_secs = match (workflow.max_duration_secs, self.server_max_duration_secs) {
            (Some(wf), Some(srv)) => Some(wf.min(srv).clamp(10, 86400)),
            (Some(wf), None)      => Some(wf.clamp(10, 86400)),
            (None,     Some(srv)) => Some(srv),  // already clamped in builder
            (None,     None)      => None,
        };

        if let Some(secs) = limit_secs {
            match tokio::time::timeout(
                std::time::Duration::from_secs(secs),
                self.run_inner(workflow, initial_variables),
            ).await {
                Ok(result) => result,
                Err(_elapsed) => Err(EngineError::WorkflowTimeout {
                    workflow_id: workflow.id.clone(),
                    limit_secs: secs,
                }),
            }
        } else {
            self.run_inner(workflow, initial_variables).await
        }
    }

    async fn run_inner(
        &self,
        workflow: &Workflow,
        initial_variables: HashMap<String, Value>,
    ) -> Result<WorkflowResult, EngineError> {
        if self.parallel_execution {
            let executor_arc = Arc::new(self.clone());
            let workflow_arc = Arc::new(workflow.clone());
            return run_inner_parallel(executor_arc, workflow_arc, initial_variables).await;
        }

        let graph = ExecutionGraph::build(workflow)?;
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
                for id in &active_nodes {
                    state.write().await.mark_skipped(id);
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
                        node_outputs: HashMap::new(),
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
                state.write().await.log(
                    Some(node_id),
                    LogLevel::Info,
                    format!("Node '{}' is disabled — skipped", node_def.name),
                );
                state.write().await.mark_skipped(node_id);
                self.emit_node_status(&workflow.id, node_id, "skipped");
                // Activate output successors so the chain continues through the disabled node.
                let (_, drop_warn) = self.activate_successors(node_id, "output", workflow, &mut active_nodes);
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
                    .execute_loop_node(node_id, node_def, workflow, &graph.topo_order, &state)
                    .await;
                match loop_result {
                    Ok(done_output) => {
                        state.write().await.mark_succeeded(node_id, done_output);
                        self.emit_node_status(&workflow.id, node_id, "success");
                        // Activate only the "done" port successors; body nodes were
                        // already executed inside execute_loop_node.
                        let (_, drop_warn) = self.activate_successors(node_id, "done", workflow, &mut active_nodes);
                        if let Some(msg) = drop_warn {
                            if node_def.node_type_id != "output" {
                                let user_msg = msg.replacen(node_id, &node_def.name, 1);
                                state.write().await.log(Some(node_id), LogLevel::Error, user_msg);
                            }
                        }
                        // Mark all body nodes as handled so the outer loop skips them.
                        let body_node_ids =
                            self.collect_loop_body_nodes(node_id, workflow, &graph.topo_order);
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
                        let (_, _) = self.activate_successors(node_id, "on_error", workflow, &mut active_nodes);
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
                                    node_outputs: HashMap::new(),
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

            let resolved_input = self.build_input(workflow, node_def, &state).await;

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
                let (_, _) = self.activate_successors(node_id, "on_error", workflow, &mut active_nodes);
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
                            node_outputs: HashMap::new(),
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

                if let Some(ref val) = output.output {
                    let key = val["_variable_key"].as_str().unwrap_or("").trim();
                    if !key.is_empty() {
                        if let Some(v) = val.get("_variable_value") {
                            state.write().await.set_variable(key.to_string(), v.clone());
                        }
                    }
                }

                state.write().await.mark_succeeded(node_id, output);
                self.emit_node_status(&workflow.id, node_id, "success");

                let (fallback_fired, drop_warn) = self.activate_successors(
                    node_id, &taken_port, workflow, &mut active_nodes,
                );
                if let Some(msg) = drop_warn {
                    if node_def.node_type_id != "output" {
                        let user_msg = msg.replacen(node_id, &node_def.name, 1);
                        state.write().await.log(Some(node_id), LogLevel::Error, user_msg);
                    }
                }
                if fallback_fired && taken_port != "output" {
                    state.write().await.log(
                        Some(node_id),
                        LogLevel::Warn,
                        format!(
                            "Node '{}': no edge found on port '{}' — fell back to 'output' \
                             port routing. Connect the '{}' port explicitly to suppress this.",
                            node_def.name, taken_port, taken_port
                        ),
                    );
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
                let (_, _) = self.activate_successors(node_id, "on_error", workflow, &mut active_nodes);
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
                            node_outputs: HashMap::new(),
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
            node_outputs: s.snapshot().node_outputs,
            logs:         s.logs.clone(),
            error:        None,
            validation_errors: validation_warnings,
        })
    }

    fn resolve_taken_port(output: &NodeOutput) -> String {
        if let Some(ref val) = output.output {
            if let Some(branch) = val.get("branch").and_then(|b| b.as_str()) {
                return branch.to_string();
            }
            if let Some(done) = val.get("done").and_then(|d| d.as_bool()) {
                return if done { "done".to_string() } else { "loop_body".to_string() };
            }
        }
        "output".to_string()
    }

    fn activate_successors(
        &self,
        node_id:      &str,
        taken_port:   &str,
        workflow:     &Workflow,
        active_nodes: &mut HashSet<String>,
    ) -> (bool, Option<String>) {
        let mut activated = false;
        for edge in &workflow.edges {
            if edge.from_node == node_id && edge.from_port == taken_port {
                active_nodes.insert(edge.to_node.clone());
                activated = true;
            }
        }

        if !activated && taken_port != "on_error" {
            let mut fallback_activated = false;
            for edge in &workflow.edges {
                if edge.from_node == node_id && edge.from_port == "output" {
                    active_nodes.insert(edge.to_node.clone());
                    fallback_activated = true;
                }
            }
            if !fallback_activated {
                return (false, Some(format!(
                    "Node '{}' produced port '{}' but no edge is connected — branch silently dropped",
                    node_id, taken_port
                )));
            }
            return (fallback_activated, None);
        }

        (false, None)
    }

    async fn build_input(
        &self,
        workflow: &Workflow,
        node_def: &crate::model::WorkflowNode,
        state:    &SharedExecutionState,
    ) -> NodeInput {
        let (raw_input, ctx, exec_id) = {
            let s = state.read().await;
            (node_def.config.clone(), s.snapshot(), s.execution_id.clone())
        };

        let mut input = raw_input;
        if !node_def.credentials.is_empty() {
            if let Some(obj) = input.as_object_mut() {
                for (key, credential_id) in &node_def.credentials {
                    if let Some(secret) = self.credential_resolver.resolve(credential_id).await {
                        obj.insert(key.clone(), Value::String(secret));
                    }
                }
            }
        }

        let (resolved_input, expr_warnings) =
            crate::expression::resolve_all_strings(&input, workflow, &ctx, self.env_allowlist.as_deref());

        if !expr_warnings.is_empty() {
            let mut s = state.write().await;
            for msg in expr_warnings {
                s.log(Some(&node_def.id), LogLevel::Info, msg);
            }
        }

        NodeInput {
            node_id:      node_def.id.clone(),
            workflow_id:  workflow.id.clone(),
            execution_id: exec_id,
            input:        resolved_input,
            context:      {
                let mut ctx = ctx;
                if let Some(ref sandbox) = self.file_sandbox_dir {
                    ctx.metadata.insert(
                        "__file_sandbox_dir".to_string(),
                        Value::String(sandbox.to_string_lossy().to_string()),
                    );
                }
                if self.shell_exec_disabled {
                    ctx.metadata.insert("__shell_disabled".to_string(), Value::Bool(true));
                }
                if self.code_exec_disabled {
                    ctx.metadata.insert("__code_disabled".to_string(), Value::Bool(true));
                }
                ctx
            },
        }
    }

    async fn execute_with_retry(
        &self,
        node:     Arc<dyn Node>,
        input:    NodeInput,
        node_def: &crate::model::WorkflowNode,
        state:    &SharedExecutionState,
    ) -> (NodeOutput, u32) {
        let max_attempts = node_def.retry.max_attempts.clamp(1, 10);
        let backoff_ms   = node_def.retry.backoff_ms.min(60_000);
        let node_id      = &node_def.id;

        let mut last_output = NodeOutput::failure(NodeError::unrecoverable(
            "NEVER_EXECUTED", "Bug: node never executed",
        ));
        let mut actual_attempts: u32 = 0;

        for attempt in 1..=max_attempts {
            actual_attempts = attempt;

            if attempt > 1 {
                state.write().await.log(
                    Some(node_id), LogLevel::Warn,
                    format!("Node '{}' retry {}/{}", node_id, attempt, max_attempts),
                );
                if let Some(ref token) = self.cancel_token {
                    tokio::select! {
                        _ = sleep(Duration::from_millis(backoff_ms)) => {}
                        _ = token.cancelled() => {
                            return (NodeOutput::failure(NodeError::unrecoverable(
                                "CANCELLED", "Run cancelled by user",
                            )), attempt);
                        }
                    }
                } else {
                    sleep(Duration::from_millis(backoff_ms)).await;
                }
            }

            let output = node.execute(input.clone()).await;
            if output.success {
                return (output, actual_attempts);
            }

            let recoverable = output.error.as_ref().map(|e| e.recoverable).unwrap_or(false);
            last_output = output;
            if !recoverable {
                break;
            }
        }

        (last_output, actual_attempts)
    }

    fn find_failure_route(&self, node_id: &str, graph: &ExecutionGraph) -> Option<String> {
        let idx = graph.index_map.get(node_id)?;
        for neighbor_idx in graph.graph.neighbors_directed(*idx, petgraph::Direction::Outgoing) {
            let neighbor_id = &graph.graph[neighbor_idx];
            if let Some(meta) = graph.edge_meta(node_id, neighbor_id) {
                if meta.on_failure.is_some() {
                    return meta.on_failure.clone();
                }
            }
        }
        None
    }

    // Collect all node IDs in the loop body — nodes reachable from the
    // loop node via loop_body edges, transitively — returned in topo order.
    // Nodes reachable via the loop node's "done" port are explicitly excluded so
    // post-loop nodes are never accidentally pulled into the body set.
    fn collect_loop_body_nodes(
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
    async fn execute_loop_node(
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
            let loop_input = self.build_input(workflow, loop_node_def, state).await;
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
                    state.write().await.log(
                        Some(body_id),
                        LogLevel::Info,
                        format!("Node '{}' is disabled — skipped", body_def.name),
                    );
                    state.write().await.mark_skipped(body_id);
                    self.emit_node_status(&workflow.id, body_id, "skipped");
                    continue;
                }

                let body_impl = self.registry.get(&body_def.node_type_id).ok_or_else(|| {
                    format!("Body node type '{}' not registered", body_def.node_type_id)
                })?;

                let body_input = self.build_input(workflow, body_def, state).await;

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
                if let Some(ref val) = body_output.output {
                    let result_key = format!("__loop_{}_result_{}", loop_node_id, iteration);
                    state.write().await.set_loop_var(result_key, val.clone());
                }

                state.write().await.mark_succeeded(body_id, body_output);
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

// ── Parallel execution types ──────────────────────────────────────────────────

enum NodeOutcome {
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

struct NodeTaskResult {
    node_id:         String,
    outcome:         NodeOutcome,
    /// Body node IDs managed by a loop task. Empty for non-loop tasks.
    loop_body_nodes: Vec<String>,
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
    state:        &SharedExecutionState,
) -> (bool, Option<WorkflowResult>) {
    let before = active_nodes.len();
    executor.activate_successors(node_id, "on_error", workflow, active_nodes);
    let on_error_wired = active_nodes.len() > before;

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
async fn run_inner_parallel(
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

                    let resolved_input = exec.build_input(&wf, &ndef, &st).await;

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
                            .and_then(|_| output.output.as_ref())
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
        node_outputs:      s.snapshot().node_outputs,
        logs:              s.logs.clone(),
        error:             None,
        validation_errors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::EngineError;
    use crate::model::{NodeType, Workflow, WorkflowNode};
    use crate::node::{Node, NodeRegistry};
    use std::collections::HashMap;
    use std::sync::Arc;

    struct NoopCredentials;
    #[async_trait::async_trait]
    impl CredentialResolver for NoopCredentials {
        async fn resolve(&self, _id: &str) -> Option<String> {
            None
        }
    }

    // Node that completes instantly with empty output.
    struct InstantNode;
    #[async_trait::async_trait]
    impl Node for InstantNode {
        fn type_id(&self) -> &'static str { "instant_test" }
        fn display_name(&self) -> &'static str { "Instant Test" }
        fn node_type(&self) -> NodeType { NodeType::Utility }
        fn version(&self) -> &'static str { "1.0" }
        fn input_schema(&self) -> serde_json::Value { serde_json::json!({}) }
        fn output_schema(&self) -> serde_json::Value { serde_json::json!({}) }
        async fn execute(&self, _input: crate::model::NodeInput) -> crate::model::NodeOutput {
            crate::model::NodeOutput::success(serde_json::json!({}))
        }
    }

    // Node that sleeps for 60 s — only completes if the timeout does NOT fire.
    struct SlowNode;
    #[async_trait::async_trait]
    impl Node for SlowNode {
        fn type_id(&self) -> &'static str { "slow_test" }
        fn display_name(&self) -> &'static str { "Slow Test" }
        fn node_type(&self) -> NodeType { NodeType::Utility }
        fn version(&self) -> &'static str { "1.0" }
        fn input_schema(&self) -> serde_json::Value { serde_json::json!({}) }
        fn output_schema(&self) -> serde_json::Value { serde_json::json!({}) }
        async fn execute(&self, _input: crate::model::NodeInput) -> crate::model::NodeOutput {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            crate::model::NodeOutput::success(serde_json::json!({}))
        }
    }

    fn single_node_workflow(node_type_id: &str, max_duration_secs: Option<u64>) -> Workflow {
        let node = WorkflowNode {
            id: "n1".to_string(),
            node_type_id: node_type_id.to_string(),
            node_type: NodeType::Utility,
            name: "Test Node".to_string(),
            config: serde_json::json!({}),
            credentials: HashMap::new(),
            input_schema: serde_json::json!({}),
            output_schema: serde_json::json!({}),
            retry: Default::default(),
            fallback_node: None,
            disabled: false,
            position: Default::default(),
        };
        Workflow {
            schema_version: "1.0".to_string(),
            id: "wf1".to_string(),
            name: "Test Workflow".to_string(),
            description: String::new(),
            nodes: vec![node],
            edges: vec![],
            metadata: Default::default(),
            max_duration_secs,
        }
    }

    // ── Serde tests ───────────────────────────────────────────────────────────

    #[test]
    fn max_duration_secs_absent_is_none() {
        let json = r#"{"id":"w1","name":"T","nodes":[],"edges":[]}"#;
        let w: Workflow = serde_json::from_str(json).unwrap();
        assert_eq!(w.max_duration_secs, None);
    }

    #[test]
    fn max_duration_secs_null_is_none() {
        let json = r#"{"id":"w1","name":"T","nodes":[],"edges":[],"max_duration_secs":null}"#;
        let w: Workflow = serde_json::from_str(json).unwrap();
        assert_eq!(w.max_duration_secs, None);
    }

    #[test]
    fn max_duration_secs_value_deserializes() {
        let json = r#"{"id":"w1","name":"T","nodes":[],"edges":[],"max_duration_secs":300}"#;
        let w: Workflow = serde_json::from_str(json).unwrap();
        assert_eq!(w.max_duration_secs, Some(300));
    }

    // ── Error display ─────────────────────────────────────────────────────────

    #[test]
    fn workflow_timeout_error_display() {
        let e = EngineError::WorkflowTimeout {
            workflow_id: "wf_abc".to_string(),
            limit_secs: 30,
        };
        assert_eq!(
            e.to_string(),
            "Workflow 'wf_abc' exceeded maximum execution time of 30s"
        );
    }

    // ── Executor timeout behaviour (requires tokio test-util) ─────────────────

    #[tokio::test]
    async fn no_timeout_workflow_completes() {
        let mut registry = NodeRegistry::new();
        registry.register(Arc::new(InstantNode));
        let executor = WorkflowExecutor::new(
            Arc::new(registry),
            Arc::new(NoopCredentials),
        );
        let workflow = single_node_workflow("instant_test", None);
        let result = executor.run(&workflow, HashMap::new()).await;
        assert!(result.is_ok());
        assert!(result.unwrap().success);
    }

    #[tokio::test]
    async fn workflow_timeout_fires_before_slow_node() {
        tokio::time::pause();

        let mut registry = NodeRegistry::new();
        registry.register(Arc::new(SlowNode));
        let executor = Arc::new(WorkflowExecutor::new(
            Arc::new(registry),
            Arc::new(NoopCredentials),
        ));

        // max_duration_secs = 10 (minimum clamp), SlowNode sleeps 60s.
        let workflow = single_node_workflow("slow_test", Some(10));

        let exec = Arc::clone(&executor);
        let wf = workflow.clone();
        let handle = tokio::spawn(async move {
            exec.run(&wf, HashMap::new()).await
        });

        // Advance past the 10s timeout.
        tokio::time::advance(std::time::Duration::from_secs(11)).await;
        tokio::task::yield_now().await;

        let result = handle.await.unwrap();
        assert!(matches!(
            result,
            Err(EngineError::WorkflowTimeout { limit_secs: 10, .. })
        ));
    }

    #[tokio::test]
    async fn value_below_minimum_clamped_to_10s() {
        tokio::time::pause();

        let mut registry = NodeRegistry::new();
        registry.register(Arc::new(SlowNode));
        let executor = Arc::new(WorkflowExecutor::new(
            Arc::new(registry),
            Arc::new(NoopCredentials),
        ));

        // Setting 5 should clamp to 10 — timeout error should report limit_secs = 10.
        let workflow = single_node_workflow("slow_test", Some(5));

        let exec = Arc::clone(&executor);
        let wf = workflow.clone();
        let handle = tokio::spawn(async move {
            exec.run(&wf, HashMap::new()).await
        });

        tokio::time::advance(std::time::Duration::from_secs(11)).await;
        tokio::task::yield_now().await;

        let result = handle.await.unwrap();
        assert!(matches!(
            result,
            Err(EngineError::WorkflowTimeout { limit_secs: 10, .. })
        ));
    }
    #[tokio::test]
    async fn server_ceiling_applies_when_workflow_has_no_timeout() {
        tokio::time::pause();

        let mut registry = NodeRegistry::new();
        registry.register(Arc::new(SlowNode));
        let executor = Arc::new(
            WorkflowExecutor::new(Arc::new(registry), Arc::new(NoopCredentials))
                .with_server_max_duration_secs(Some(10)),
        );

        // Workflow has no max_duration_secs — server ceiling must fire.
        let workflow = single_node_workflow("slow_test", None);

        let exec = Arc::clone(&executor);
        let wf   = workflow.clone();
        let handle = tokio::spawn(async move { exec.run(&wf, HashMap::new()).await });

        tokio::time::advance(std::time::Duration::from_secs(11)).await;
        tokio::task::yield_now().await;

        let result = handle.await.unwrap();
        assert!(matches!(
            result,
            Err(EngineError::WorkflowTimeout { limit_secs: 10, .. })
        ));
    }

    #[tokio::test]
    async fn server_ceiling_clamps_longer_workflow_timeout() {
        tokio::time::pause();

        let mut registry = NodeRegistry::new();
        registry.register(Arc::new(SlowNode));
        let executor = Arc::new(
            WorkflowExecutor::new(Arc::new(registry), Arc::new(NoopCredentials))
                .with_server_max_duration_secs(Some(10)),
        );

        // Workflow sets 3600s, server ceiling is 10s — ceiling wins.
        let workflow = single_node_workflow("slow_test", Some(3600));

        let exec = Arc::clone(&executor);
        let wf   = workflow.clone();
        let handle = tokio::spawn(async move { exec.run(&wf, HashMap::new()).await });

        tokio::time::advance(std::time::Duration::from_secs(11)).await;
        tokio::task::yield_now().await;

        let result = handle.await.unwrap();
        assert!(matches!(
            result,
            Err(EngineError::WorkflowTimeout { limit_secs: 10, .. })
        ));
    }

    #[tokio::test]
    async fn workflow_timeout_wins_when_shorter_than_server_ceiling() {
        tokio::time::pause();

        let mut registry = NodeRegistry::new();
        registry.register(Arc::new(SlowNode));
        let executor = Arc::new(
            WorkflowExecutor::new(Arc::new(registry), Arc::new(NoopCredentials))
                .with_server_max_duration_secs(Some(3600)),
        );

        // Workflow sets 10s, server ceiling is 3600s — workflow limit wins.
        let workflow = single_node_workflow("slow_test", Some(10));

        let exec = Arc::clone(&executor);
        let wf   = workflow.clone();
        let handle = tokio::spawn(async move { exec.run(&wf, HashMap::new()).await });

        tokio::time::advance(std::time::Duration::from_secs(11)).await;
        tokio::task::yield_now().await;

        let result = handle.await.unwrap();
        assert!(matches!(
            result,
            Err(EngineError::WorkflowTimeout { limit_secs: 10, .. })
        ));
    }

}
