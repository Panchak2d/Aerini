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

use crate::context::{LogLevel, SharedExecutionState};
use crate::error::{EngineError, NodeError};
use crate::graph::ExecutionGraph;
use petgraph::Direction;
use crate::model::{NodeInput, NodeOutput, Workflow};
use crate::node::{Node, NodeRegistry};
use crate::EventSink;

mod sequential;
mod loop_executor;
mod parallel;

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
    pub(super) registry:            Arc<NodeRegistry>,
    pub(super) credential_resolver: Arc<dyn CredentialResolver>,
    // Optional: when None, node-status events are dropped silently.
    pub(super) event_sink:          Option<Arc<dyn EventSink>>,
    // Optional: when None, {{$env.VAR}} expressions resolve to "" with a warning.
    pub(super) env_allowlist:       Option<Arc<std::collections::HashSet<String>>>,
    // Optional: when set, FileNode restricts all paths to this directory tree.
    pub(super) file_sandbox_dir:    Option<Arc<std::path::PathBuf>>,
    // When true, ShellExecNode returns SHELL_DISABLED immediately without executing.
    pub(super) shell_exec_disabled: bool,
    // When true, CodeNode returns CODE_DISABLED immediately without executing.
    pub(super) code_exec_disabled:  bool,
    // When true, input schema violations fail the node. When false (default), they log a warning.
    pub(super) strict_schema_validation: bool,
    // When true, independent branches execute concurrently via tokio tasks. Default: false.
    pub(super) parallel_execution: bool,
    // Maximum simultaneous node tasks when parallel_execution is true. Default: 8.
    pub(super) max_concurrent_nodes: usize,
    // When set, the executor checks this token between nodes and short-circuits retry backoff sleeps.
    pub(super) cancel_token: Option<CancellationToken>,
    // Server-level ceiling applied to every workflow regardless of per-workflow max_duration_secs.
    pub(super) server_max_duration_secs: Option<u64>,
    // When true, nodes gated on admin scope (e.g. allow_raw_sql) are unlocked.
    pub(super) caller_is_admin: bool,
    // When true, Code (JS) nodes run with module import restrictions and OS resource limits.
    // Only meaningful when code_exec_disabled is false.
    pub(super) code_sandbox_enabled: bool,
}

impl WorkflowExecutor {
    pub fn new(
        registry:            Arc<NodeRegistry>,
        credential_resolver: Arc<dyn CredentialResolver>,
    ) -> Self {
        Self { registry, credential_resolver, event_sink: None, env_allowlist: None, file_sandbox_dir: None, shell_exec_disabled: false, code_exec_disabled: false, strict_schema_validation: false, parallel_execution: false, max_concurrent_nodes: 8, cancel_token: None, server_max_duration_secs: None, caller_is_admin: false, code_sandbox_enabled: false }
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

    /// Grant admin-level node permissions (e.g. `allow_raw_sql`) to this executor.
    /// Should be set to `true` only when the caller holds the `admin` API scope.
    pub fn with_caller_is_admin(mut self, is_admin: bool) -> Self {
        self.caller_is_admin = is_admin;
        self
    }

    /// Enable Code (JS) node sandboxing.
    /// When true, the Code node subprocess is launched with:
    ///   - `--disallow-code-generation-from-strings`
    ///   - A loader that blocks dangerous built-in module imports
    ///     (child_process, fs, fs/promises, net, http, https, dgram, dns, os)
    ///   - CPU and memory resource limits (Linux only, via setrlimit)
    /// Desktop mode (sandbox = false): full Node.js stdlib available as documented.
    /// Server mode with --allow-code: sandbox defaults to true; admin may disable.
    pub fn with_code_sandbox(mut self, enabled: bool) -> Self {
        self.code_sandbox_enabled = enabled;
        self
    }

    pub(super) fn emit_node_status(&self, workflow_id: &str, node_id: &str, status: &str) {
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
    pub(super) fn validate_node_input(schema: &Value, input: &Value) -> Vec<String> {
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
    pub(super) async fn check_schema(
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
        workflow: Arc<Workflow>,
        initial_variables: HashMap<String, Value>,
    ) -> Result<WorkflowResult, EngineError> {
        let limit_secs = match (workflow.max_duration_secs, self.server_max_duration_secs) {
            (Some(wf), Some(srv)) => Some(wf.min(srv).clamp(10, 86400)),
            (Some(wf), None)      => Some(wf.clamp(10, 86400)),
            (None,     Some(srv)) => Some(srv),  // already clamped in builder
            (None,     None)      => None,
        };

        if let Some(secs) = limit_secs {
            let wf_id = workflow.id.clone();
            match tokio::time::timeout(
                std::time::Duration::from_secs(secs),
                self.run_inner(workflow, initial_variables),
            ).await {
                Ok(result) => result,
                Err(_elapsed) => Err(EngineError::WorkflowTimeout {
                    workflow_id: wf_id,
                    limit_secs: secs,
                }),
            }
        } else {
            self.run_inner(workflow, initial_variables).await
        }
    }

    pub(super) fn resolve_taken_port(output: &NodeOutput) -> String {
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

    pub(super) fn activate_successors(
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

    pub(super) async fn build_input(
        &self,
        workflow: &Workflow,
        node_def: &crate::model::WorkflowNode,
        state:    &SharedExecutionState,
    ) -> Result<NodeInput, NodeOutput> {
        let (raw_input, ctx, exec_id) = {
            let s = state.read().await;
            (node_def.config.clone(), s.snapshot(), s.execution_id.clone())
        };

        // Resolve expressions first so credential values are never passed through
        // the expression engine. A credential containing `{{...}}` must not expand
        // against the current execution context (prevents credential-value injection
        // in shared-server deployments).

        // SECURITY: Expression interpolation in SQL query strings is blocked unconditionally.
        // Post-resolution heuristics (single-quote check) cannot catch numeric injection —
        // e.g. {{val}} resolving to `1 OR 1=1` produces no quotes. The only safe fix is
        // to reject {{...}} in SQL queries before resolution. All dynamic values must use
        // `?` placeholders and the `params` array.
        if node_def.node_type_id == "database" {
            if let Some(query_raw) = raw_input["query"].as_str() {
                if query_raw.contains("{{") {
                    return Err(NodeOutput::failure(NodeError::unrecoverable(
                        "SQL_EXPRESSION_BLOCKED",
                        format!(
                            "Node '{}': the `query` field contains `{{{{...}}}}` expression \
                             interpolation. Values are inserted directly into the SQL string \
                             and cannot be safely sanitized — numeric injection (e.g. `1 OR 1=1`) \
                             produces no quotes and bypasses all heuristics. Use `?` placeholders \
                             and the `params` array for all dynamic values. Example: \
                             query: \"SELECT * FROM t WHERE id = ?\" with \
                             params: [\"{{{{node.output.id}}}}\"].",
                            node_def.name
                        ),
                    )));
                }
            }
        }

        let (mut resolved_input, expr_warnings) =
            crate::expression::resolve_all_strings(&raw_input, workflow, &ctx, self.env_allowlist.as_deref());

        if !node_def.credentials.is_empty() {
            if let Some(obj) = resolved_input.as_object_mut() {
                for (key, credential_id) in &node_def.credentials {
                    if let Some(secret) = self.credential_resolver.resolve(credential_id).await {
                        obj.insert(key.clone(), Value::String(secret));
                    }
                }
            }
        }

        if !expr_warnings.is_empty() {
            let mut s = state.write().await;
            for msg in expr_warnings {
                s.log(Some(&node_def.id), LogLevel::Warn, msg);
            }
        }

        Ok(NodeInput {
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
                if self.code_sandbox_enabled {
                    ctx.metadata.insert("__code_sandbox".to_string(), Value::Bool(true));
                }
                ctx.metadata.insert("__caller_is_admin".to_string(), Value::Bool(self.caller_is_admin));
                ctx
            },
        })
    }

    pub(super) async fn execute_with_retry(
        &self,
        node:     Arc<dyn Node>,
        input:    NodeInput,
        node_def: &crate::model::WorkflowNode,
        state:    &SharedExecutionState,
    ) -> (NodeOutput, u32) {
        let max_attempts = node_def.retry.max_attempts.clamp(1, 10);
        let backoff_ms   = node_def.retry.backoff_ms.min(60_000);
        let node_id      = &node_def.id;

        // Fast path: single attempt — consume input without cloning.
        if max_attempts == 1 {
            let output = node.execute(input).await;
            return (output, 1);
        }

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

    pub(super) fn find_failure_route(&self, node_id: &str, graph: &ExecutionGraph) -> Option<String> {
        let idx = graph.index_map.get(node_id)?;
        for neighbor_idx in graph.graph.neighbors_directed(*idx, Direction::Outgoing) {
            let neighbor_id = &graph.graph[neighbor_idx];
            if let Some(meta) = graph.edge_meta(node_id, neighbor_id) {
                if meta.on_failure.is_some() {
                    return meta.on_failure.clone();
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::EngineError;
    use crate::migration::CURRENT_VERSION;
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
            schema_version: CURRENT_VERSION.to_string(),
            id: "wf1".to_string(),
            name: "Test Workflow".to_string(),
            description: String::new(),
            nodes: vec![node],
            edges: vec![],
            metadata: Default::default(),
            max_duration_secs,
            parallel_execution: false,
            max_concurrent_nodes: None,
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
        let result = executor.run(Arc::new(workflow), HashMap::new()).await;
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
            exec.run(Arc::new(wf), HashMap::new()).await
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
            exec.run(Arc::new(wf), HashMap::new()).await
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
        let handle = tokio::spawn(async move { exec.run(Arc::new(wf), HashMap::new()).await });

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
        let handle = tokio::spawn(async move { exec.run(Arc::new(wf), HashMap::new()).await });

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
        let handle = tokio::spawn(async move { exec.run(Arc::new(wf), HashMap::new()).await });

        tokio::time::advance(std::time::Duration::from_secs(11)).await;
        tokio::task::yield_now().await;

        let result = handle.await.unwrap();
        assert!(matches!(
            result,
            Err(EngineError::WorkflowTimeout { limit_secs: 10, .. })
        ));
    }
}
