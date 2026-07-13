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

mod config;
mod sequential;
mod loop_executor;
mod parallel;

use config::WorkflowExecutorConfig;

// ── Execution result ──────────────────────────────────────────────────────────

/// The outcome of a complete workflow execution.
///
/// Returned by [`WorkflowExecutor::run`]. `success: false` with a populated `error`
/// indicates the workflow was aborted. Individual node failures within an
/// otherwise-successful run appear only in `logs`.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct WorkflowResult {
    pub execution_id: String,
    pub workflow_id: String,
    pub success: bool,
    /// Output values from each completed node, keyed by node ID.
    /// Each value is the `output` field from that node's [`crate::model::NodeOutput`].
    pub node_outputs: HashMap<String, Value>,
    pub logs: Vec<crate::context::ExecutionLogEntry>,
    pub error: Option<String>,
    /// Schema validation warnings (or errors in strict mode) encountered during execution.
    /// Empty when strict_schema_validation is false and all schemas are clean.
    #[serde(default)]
    pub validation_errors: Vec<String>,
}

// ── Credential resolver ───────────────────────────────────────────────────────

/// Provides resolved credential values to the executor at run time.
///
/// The executor calls `resolve(credential_id)` for each credential reference in
/// the workflow before passing input to a node. Nodes receive the resolved plaintext
/// value directly — they never see credential IDs or the backing store.
///
/// **Desktop app:** reads from the AES-256-GCM encrypted SQLite credential store.
/// **Server binary:** reads from environment variables.
/// **Embedders:** implement this trait for whatever secret store fits your context.
#[async_trait::async_trait]
pub trait CredentialResolver: Send + Sync + 'static {
    /// Return the plaintext value for `credential_id`, or `None` if not found.
    ///
    /// `None` causes the corresponding credential field to resolve to an empty string
    /// in the node's input. The executor does not treat a missing credential as a hard failure.
    async fn resolve(&self, credential_id: &str) -> Option<String>;
}

// ── Executor ──────────────────────────────────────────────────────────────────

/// Runs a single workflow to completion.
///
/// Constructed with [`WorkflowExecutor::new`] and configured through fluent builder
/// methods. The executor is `Clone` — you can share a configured instance across
/// multiple concurrent `run()` calls without additional locking.
///
/// Each `run()` call creates its own internal execution state and does not mutate
/// the executor, so clones are safe to use concurrently.
#[derive(Clone)]
pub struct WorkflowExecutor {
    pub(super) registry:            Arc<NodeRegistry>,
    pub(super) credential_resolver: Arc<dyn CredentialResolver>,
    pub(super) config:              WorkflowExecutorConfig,
}

impl WorkflowExecutor {
    /// Create an executor with the minimum required components.
    ///
    /// All optional configuration (event sink, sandboxing, timeouts, parallel execution)
    /// defaults to off. Use the builder methods to enable features before calling `run()`.
    pub fn new(
        registry:            Arc<NodeRegistry>,
        credential_resolver: Arc<dyn CredentialResolver>,
    ) -> Self {
        Self { registry, credential_resolver, config: WorkflowExecutorConfig::default() }
    }

    /// Restrict `{{$env.VAR}}` expressions to the listed variable names.
    /// Call with an empty vec to allow no env vars. Omit entirely to disable $env.
    pub fn with_env_allowlist(mut self, vars: Vec<String>) -> Self {
        self.config = self.config.with_env_allowlist(vars);
        self
    }

    /// Attach an event sink. Fluent builder — allows optional chaining.
    pub fn with_event_sink(mut self, sink: Arc<dyn EventSink>) -> Self {
        self.config = self.config.with_event_sink(sink);
        self
    }

    /// Restrict FileNode to paths within `dir`. Required in server/API mode to
    /// prevent authenticated users from reading arbitrary filesystem paths.
    pub fn with_file_sandbox_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.config = self.config.with_file_sandbox_dir(dir);
        self
    }

    /// Disable the Shell Command node. When true, any ShellExecNode returns a
    /// SHELL_DISABLED error immediately without spawning a subprocess.
    /// Recommended for API mode deployments where arbitrary shell access is undesirable.
    pub fn with_shell_disabled(mut self, disabled: bool) -> Self {
        self.config = self.config.with_shell_disabled(disabled);
        self
    }

    /// Disable the Code (JS) node. When true, any CodeNode returns a
    /// CODE_DISABLED error immediately without spawning a Node.js process.
    /// Recommended for API mode deployments where arbitrary JS execution is undesirable.
    pub fn with_code_disabled(mut self, disabled: bool) -> Self {
        self.config = self.config.with_code_disabled(disabled);
        self
    }

    /// Disable the Database node. When true, any DatabaseNode returns a
    /// DATABASE_DISABLED error immediately without opening any connection.
    /// Recommended for multi-tenant API deployments due to RUSTSEC-2023-0071
    /// (RSA Marvin timing side-channel in sqlx-mysql) and SSRF surface reduction.
    pub fn with_database_disabled(mut self, disabled: bool) -> Self {
        self.config = self.config.with_database_disabled(disabled);
        self
    }

    /// When true, any node whose resolved input fails its declared JSON Schema causes the
    /// node to fail immediately (SCHEMA_VIOLATION). When false (default), violations are
    /// logged as warnings and execution continues — safe upgrade path for existing workflows.
    pub fn with_strict_schema_validation(mut self, strict: bool) -> Self {
        self.config = self.config.with_strict_schema_validation(strict);
        self
    }

    /// Enable parallel execution of independent branches.
    /// When true, nodes whose upstream dependencies have all completed are run
    /// concurrently via tokio tasks instead of sequentially. Default: false.
    /// The sequential path is unchanged when this is false.
    pub fn with_parallel_execution(mut self, enabled: bool) -> Self {
        self.config = self.config.with_parallel_execution(enabled);
        self
    }

    /// Maximum number of node tasks running simultaneously in parallel mode.
    /// Values below 1 are clamped to 1. Default: 8.
    pub fn with_max_concurrent_nodes(mut self, limit: usize) -> Self {
        self.config = self.config.with_max_concurrent_nodes(limit);
        self
    }

    /// Attach a cancellation token. When cancelled, the executor stops between
    /// nodes and short-circuits retry backoff sleeps. No-op if never cancelled.
    pub fn with_cancel_token(mut self, t: CancellationToken) -> Self {
        self.config = self.config.with_cancel_token(t);
        self
    }

    /// Set a server-level ceiling on workflow execution time (seconds).
    /// Takes effect as `min(workflow.max_duration_secs, ceiling)` when both are set.
    /// When only this ceiling is set it applies to every workflow run by this executor.
    /// Values below 10 are clamped to 10; values above 86400 are clamped to 86400.
    pub fn with_server_max_duration_secs(mut self, secs: Option<u64>) -> Self {
        self.config = self.config.with_server_max_duration_secs(secs);
        self
    }

    /// Grant admin-level node permissions (e.g. `allow_raw_sql`) to this executor.
    /// Should be set to `true` only when the caller holds the `admin` API scope.
    pub fn with_caller_is_admin(mut self, is_admin: bool) -> Self {
        self.config = self.config.with_caller_is_admin(is_admin);
        self
    }

    /// Enable Code (JS) node sandboxing.
    /// When true, the Code node subprocess is launched with:
    ///   - `--disallow-code-generation-from-strings`
    ///   - A loader that blocks dangerous built-in module imports
    ///     (child_process, fs, fs/promises, net, http, https, dgram, dns, os)
    ///   - CPU and memory resource limits (Linux only, via setrlimit)
    ///
    /// Desktop mode (sandbox = false): full Node.js stdlib available as documented.
    /// Server mode with --allow-code: sandbox defaults to true; admin may disable.
    pub fn with_code_sandbox(mut self, enabled: bool) -> Self {
        self.config = self.config.with_code_sandbox(enabled);
        self
    }

    /// Override the default 512 MB memory cap for Code (JS) node subprocesses.
    /// Only effective on Linux with --code-sandbox active; no-op on macOS/Windows.
    pub fn with_code_max_memory_mb(mut self, mb: Option<u64>) -> Self {
        self.config = self.config.with_code_max_memory_mb(mb);
        self
    }

    pub(super) fn emit_node_status(&self, workflow_id: &str, node_id: &str, status: &str) {
        if let Some(ref sink) = self.config.event_sink {
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

        if self.config.strict_schema_validation {
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

    /// Execute `workflow`, returning a [`WorkflowResult`] on success.
    ///
    /// `initial_variables` are merged into the execution context before the first node
    /// runs and are accessible in any node config field via `{{$vars.key}}` expressions.
    ///
    /// Returns `Err(EngineError::WorkflowTimeout)` if the workflow exceeds its configured
    /// maximum duration. Graph errors (cycles, unknown node references) and unregistered
    /// node types also return `Err`. Individual node failures are recorded in the result
    /// and do not cause an `Err` return unless the workflow has no recovery path.
    pub async fn run(
        &self,
        workflow: Arc<Workflow>,
        initial_variables: HashMap<String, Value>,
    ) -> Result<WorkflowResult, EngineError> {
        let limit_secs = match (workflow.max_duration_secs, self.config.server_max_duration_secs) {
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
            if let Some(port) = val.get("port").and_then(|p| p.as_str()) {
                return port.to_string();
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
            crate::expression::resolve_all_strings(&raw_input, workflow, &ctx, self.config.env_allowlist.as_deref());

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
                if let Some(ref sandbox) = self.config.file_sandbox_dir {
                    ctx.metadata.insert(
                        "__file_sandbox_dir".to_string(),
                        Value::String(sandbox.to_string_lossy().to_string()),
                    );
                }
                if self.config.shell_exec_disabled {
                    ctx.metadata.insert("__shell_disabled".to_string(), Value::Bool(true));
                }
                if self.config.code_exec_disabled {
                    ctx.metadata.insert("__code_disabled".to_string(), Value::Bool(true));
                }
                if self.config.database_exec_disabled {
                    ctx.metadata.insert("__database_disabled".to_string(), Value::Bool(true));
                }
                if self.config.code_sandbox_enabled {
                    ctx.metadata.insert("__code_sandbox".to_string(), Value::Bool(true));
                    // Expose the --allow-env-vars allowlist so the Code node can
                    // re-inject approved vars after env_clear() in sandbox mode.
                    if let Some(ref allowlist) = self.config.env_allowlist {
                        let vars: Vec<Value> = allowlist.iter()
                            .map(|s| Value::String(s.clone()))
                            .collect();
                        ctx.metadata.insert("__allowed_env_vars".to_string(), Value::Array(vars));
                    }
                }
                if let Some(mb) = self.config.code_max_memory_mb {
                    ctx.metadata.insert("__code_max_memory_mb".to_string(), Value::Number(mb.into()));
                }
                ctx.metadata.insert("__caller_is_admin".to_string(), Value::Bool(self.config.caller_is_admin));

                // Build name-keyed output map so Code node JS can use context["Node Name"].field
                // instead of context["node_1234_5"] (internal IDs are not user-visible).
                let name_outputs: serde_json::Map<String, Value> = workflow.nodes.iter()
                    .filter_map(|n| ctx.node_outputs.get(&n.id).map(|v| (n.name.clone(), v.clone())))
                    .collect();
                ctx.metadata.insert(
                    "__node_name_outputs".to_string(),
                    Value::Object(name_outputs),
                );

                // Inject direct upstream node output so Code node JS `input` = wired-in node's data.
                // Use find_map over ALL incoming edges so we skip edges whose upstream node was
                // skipped or has not yet produced output.  The previous find().and_then() pattern
                // stopped at the first matching edge regardless of whether that node ran — if that
                // edge's from_node wasn't in node_outputs (e.g. it was skipped, or edge ordering
                // happened to put an inactive edge first), __direct_input was silently never set
                // and the Code node received `input = {}` with no fields.
                if let Some(upstream_output) = workflow.edges.iter()
                    .filter(|e| e.to_node == node_def.id)
                    .find_map(|e| ctx.node_outputs.get(&e.from_node).cloned())
                {
                    ctx.metadata.insert("__direct_input".to_string(), upstream_output);
                }

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
                if let Some(ref token) = self.config.cancel_token {
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
    use tokio_util::sync::CancellationToken;

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
            settings: Default::default(),
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

    // ── __direct_input propagation ────────────────────────────────────────────
    //
    // Regression test for the bug where find().and_then() picked the first
    // matching edge but returned None if that edge's upstream hadn't produced
    // output yet, silently leaving __direct_input unset and the Code node JS
    // `input` variable as an empty object.
    //
    // This test builds a two-node workflow: EchoNode (upstream) → InspectorNode
    // (downstream). EchoNode emits { "ping": "pong" }. InspectorNode reads
    // __direct_input from its NodeInput and asserts the value is correct.
    // The test fails before the fix (InspectorNode sees {} instead of the
    // echo output) and passes after.

    use std::sync::Mutex;

    struct EchoNode;
    #[async_trait::async_trait]
    impl Node for EchoNode {
        fn type_id(&self)        -> &'static str { "echo_test" }
        fn display_name(&self)   -> &'static str { "Echo Test" }
        fn node_type(&self)      -> NodeType     { NodeType::Utility }
        fn version(&self)        -> &'static str { "1.0" }
        fn input_schema(&self)   -> serde_json::Value { serde_json::json!({}) }
        fn output_schema(&self)  -> serde_json::Value { serde_json::json!({}) }
        async fn execute(&self, _: crate::model::NodeInput) -> crate::model::NodeOutput {
            crate::model::NodeOutput::success(serde_json::json!({ "ping": "pong" }))
        }
    }

    // Captures the __direct_input value it receives so the test can assert on it.
    struct InspectorNode {
        captured: Arc<Mutex<Option<serde_json::Value>>>,
    }
    #[async_trait::async_trait]
    impl Node for InspectorNode {
        fn type_id(&self)        -> &'static str { "inspector_test" }
        fn display_name(&self)   -> &'static str { "Inspector Test" }
        fn node_type(&self)      -> NodeType     { NodeType::Utility }
        fn version(&self)        -> &'static str { "1.0" }
        fn input_schema(&self)   -> serde_json::Value { serde_json::json!({}) }
        fn output_schema(&self)  -> serde_json::Value { serde_json::json!({}) }
        async fn execute(&self, input: crate::model::NodeInput) -> crate::model::NodeOutput {
            let empty = serde_json::Value::Object(Default::default());
            let direct = input.context.metadata
                .get("__direct_input")
                .cloned()
                .unwrap_or(empty);
            *self.captured.lock().unwrap() = Some(direct);
            crate::model::NodeOutput::success(serde_json::json!({}))
        }
    }

    fn two_node_workflow() -> Workflow {
        // WorkflowNode is already imported at module level; only WorkflowEdge is new here.
        use crate::model::WorkflowEdge;
        let upstream = WorkflowNode {
            id: "n_upstream".to_string(),
            node_type_id: "echo_test".to_string(),
            node_type: NodeType::Utility,
            name: "Echo".to_string(),
            config: serde_json::json!({}),
            credentials: HashMap::new(),
            input_schema: serde_json::json!({}),
            output_schema: serde_json::json!({}),
            retry: Default::default(),
            fallback_node: None,
            disabled: false,
            position: Default::default(),
        };
        let downstream = WorkflowNode {
            id: "n_downstream".to_string(),
            node_type_id: "inspector_test".to_string(),
            node_type: NodeType::Utility,
            name: "Inspector".to_string(),
            config: serde_json::json!({}),
            credentials: HashMap::new(),
            input_schema: serde_json::json!({}),
            output_schema: serde_json::json!({}),
            retry: Default::default(),
            fallback_node: None,
            disabled: false,
            position: Default::default(),
        };
        let edge = WorkflowEdge {
            id: "e1".to_string(),
            from_node: "n_upstream".to_string(),
            from_port: "output".to_string(),
            to_node: "n_downstream".to_string(),
            to_port: "input".to_string(),
            condition: None,
            on_success: None,
            on_failure: None,
        };
        Workflow {
            schema_version: CURRENT_VERSION.to_string(),
            id: "wf_direct_input_test".to_string(),
            name: "Direct Input Test".to_string(),
            description: String::new(),
            nodes: vec![upstream, downstream],
            edges: vec![edge],
            metadata: Default::default(),
            max_duration_secs: None,
            parallel_execution: false,
            max_concurrent_nodes: None,
            settings: Default::default(),
        }
    }

    #[tokio::test]
    async fn direct_input_propagated_to_downstream_node() {
        let captured: Arc<Mutex<Option<serde_json::Value>>> = Arc::new(Mutex::new(None));

        let mut registry = NodeRegistry::new();
        registry.register(Arc::new(EchoNode));
        registry.register(Arc::new(InspectorNode { captured: Arc::clone(&captured) }));

        let executor = WorkflowExecutor::new(Arc::new(registry), Arc::new(NoopCredentials));
        let workflow = two_node_workflow();
        let result = executor.run(Arc::new(workflow), HashMap::new()).await;

        assert!(result.is_ok(), "workflow failed: {:?}", result.err());
        assert!(result.unwrap().success, "workflow did not succeed");

        let captured_val = captured.lock().unwrap().clone();
        assert!(
            captured_val.is_some(),
            "InspectorNode never ran — upstream did not activate it"
        );

        let val = captured_val.unwrap();
        assert_eq!(
            val.get("ping").and_then(|v| v.as_str()),
            Some("pong"),
            "__direct_input was {:?} — expected {{\"ping\":\"pong\"}}. \
             This means the executor's find_map fix is not working.",
            val
        );
    }

    // ── P23 node stubs ────────────────────────────────────────────────────────

    // Fires the supplied CancellationToken on execute(), then returns success.
    // Used to simulate mid-run cancellation: place upstream of another node so
    // the sequential cancel check fires before the downstream node starts.
    struct CancellingNode { token: CancellationToken }
    #[async_trait::async_trait]
    impl Node for CancellingNode {
        fn type_id(&self)        -> &'static str { "cancelling_test" }
        fn display_name(&self)   -> &'static str { "Cancelling Test" }
        fn node_type(&self)      -> NodeType     { NodeType::Utility }
        fn version(&self)        -> &'static str { "1.0" }
        fn input_schema(&self)   -> serde_json::Value { serde_json::json!({}) }
        fn output_schema(&self)  -> serde_json::Value { serde_json::json!({}) }
        async fn execute(&self, _: crate::model::NodeInput) -> crate::model::NodeOutput {
            self.token.cancel();
            crate::model::NodeOutput::success(serde_json::json!({}))
        }
    }

    // Always returns a failure. Used to test cascading failure routing.
    struct FailingNode;
    #[async_trait::async_trait]
    impl Node for FailingNode {
        fn type_id(&self)        -> &'static str { "failing_test" }
        fn display_name(&self)   -> &'static str { "Failing Test" }
        fn node_type(&self)      -> NodeType     { NodeType::Utility }
        fn version(&self)        -> &'static str { "1.0" }
        fn input_schema(&self)   -> serde_json::Value { serde_json::json!({}) }
        fn output_schema(&self)  -> serde_json::Value { serde_json::json!({}) }
        async fn execute(&self, _: crate::model::NodeInput) -> crate::model::NodeOutput {
            crate::model::NodeOutput::failure(
                crate::error::NodeError::unrecoverable("TEST_FAIL", "intentional test failure"),
            )
        }
    }

    // Returns NodeOutput::success(Value::Null). Used to verify that null output
    // does not panic downstream nodes or the executor.
    struct NullValueNode;
    #[async_trait::async_trait]
    impl Node for NullValueNode {
        fn type_id(&self)        -> &'static str { "null_value_test" }
        fn display_name(&self)   -> &'static str { "Null Value Test" }
        fn node_type(&self)      -> NodeType     { NodeType::Utility }
        fn version(&self)        -> &'static str { "1.0" }
        fn input_schema(&self)   -> serde_json::Value { serde_json::json!({}) }
        fn output_schema(&self)  -> serde_json::Value { serde_json::json!({}) }
        async fn execute(&self, _: crate::model::NodeInput) -> crate::model::NodeOutput {
            crate::model::NodeOutput::success(serde_json::Value::Null)
        }
    }

    // Builds a two-node workflow: up_type → down_type, with fixed IDs "n_up"/"n_down".
    fn two_node_typed_workflow(up_type: &str, down_type: &str) -> Workflow {
        use crate::model::WorkflowEdge;
        let up = WorkflowNode {
            id: "n_up".to_string(),
            node_type_id: up_type.to_string(),
            node_type: NodeType::Utility,
            name: "Upstream".to_string(),
            config: serde_json::json!({}),
            credentials: HashMap::new(),
            input_schema: serde_json::json!({}),
            output_schema: serde_json::json!({}),
            retry: Default::default(),
            fallback_node: None,
            disabled: false,
            position: Default::default(),
        };
        let down = WorkflowNode {
            id: "n_down".to_string(),
            node_type_id: down_type.to_string(),
            node_type: NodeType::Utility,
            name: "Downstream".to_string(),
            config: serde_json::json!({}),
            credentials: HashMap::new(),
            input_schema: serde_json::json!({}),
            output_schema: serde_json::json!({}),
            retry: Default::default(),
            fallback_node: None,
            disabled: false,
            position: Default::default(),
        };
        let edge = WorkflowEdge {
            id: "e1".to_string(),
            from_node: "n_up".to_string(),
            from_port: "output".to_string(),
            to_node: "n_down".to_string(),
            to_port: "input".to_string(),
            condition: None,
            on_success: None,
            on_failure: None,
        };
        Workflow {
            schema_version: CURRENT_VERSION.to_string(),
            id: "wf_two_typed".to_string(),
            name: "Two Node Typed".to_string(),
            description: String::new(),
            nodes: vec![up, down],
            edges: vec![edge],
            metadata: Default::default(),
            max_duration_secs: None,
            parallel_execution: false,
            max_concurrent_nodes: None,
            settings: Default::default(),
        }
    }

    // ── P23: executor/mod.rs tests ────────────────────────────────────────────

    // Cancel token fired mid-run: executor stops between nodes and returns
    // Err(ExecutionCancelled). CancellingNode fires the token on execute();
    // the sequential cancel check then trips before InstantNode starts.
    #[tokio::test]
    async fn cancel_token_mid_run_stops_execution() {
        let token = CancellationToken::new();

        let mut registry = NodeRegistry::new();
        registry.register(Arc::new(CancellingNode { token: token.clone() }));
        registry.register(Arc::new(InstantNode));

        let executor = WorkflowExecutor::new(Arc::new(registry), Arc::new(NoopCredentials))
            .with_cancel_token(token);

        let workflow = two_node_typed_workflow("cancelling_test", "instant_test");
        let result = executor.run(Arc::new(workflow), HashMap::new()).await;

        assert!(
            matches!(result, Err(EngineError::ExecutionCancelled)),
            "expected ExecutionCancelled; got {:?}", result
        );
    }

    // Cascading failure: failing node A has no on_error route → workflow returns
    // success=false with the error surfaced. Downstream node B is never activated
    // and must not appear in node_outputs.
    #[tokio::test]
    async fn cascading_failure_downstream_skipped_error_surfaced() {
        let mut registry = NodeRegistry::new();
        registry.register(Arc::new(FailingNode));
        registry.register(Arc::new(InstantNode));

        let executor = WorkflowExecutor::new(Arc::new(registry), Arc::new(NoopCredentials));
        // n_up = FailingNode, n_down = InstantNode (never reached)
        let workflow = two_node_typed_workflow("failing_test", "instant_test");

        let result = executor.run(Arc::new(workflow), HashMap::new()).await;

        // run() returns Ok even on workflow failure — Err only for engine errors.
        assert!(result.is_ok(), "run() must not return Err for a node failure");
        let r = result.unwrap();
        assert!(!r.success, "workflow must not be marked successful");
        assert!(r.error.is_some(), "error field must be populated");
        assert!(
            !r.node_outputs.contains_key("n_down"),
            "downstream node must not appear in node_outputs when upstream failed with no on_error"
        );
    }

    // max_concurrent_nodes(0): config.validate() clamps to 1. Workflow runs
    // without panic, confirming clamping happens before execution.
    #[tokio::test]
    async fn max_concurrent_nodes_zero_clamped_to_one_no_panic() {
        let mut registry = NodeRegistry::new();
        registry.register(Arc::new(InstantNode));

        let executor = WorkflowExecutor::new(Arc::new(registry), Arc::new(NoopCredentials))
            .with_max_concurrent_nodes(0);

        assert_eq!(
            executor.config.max_concurrent_nodes, 1,
            "with_max_concurrent_nodes(0) must clamp to 1 via validate()"
        );

        let result = executor
            .run(Arc::new(single_node_workflow("instant_test", None)), HashMap::new())
            .await;
        assert!(result.is_ok());
        assert!(result.unwrap().success);
    }

    // Node returns NodeOutput::success(Value::Null): downstream node receives null
    // in context, executor does not panic, workflow succeeds end-to-end.
    // node_outputs entry for the null-output node is Value::Null.
    #[tokio::test]
    async fn null_output_value_does_not_panic_downstream_receives_null() {
        let mut registry = NodeRegistry::new();
        registry.register(Arc::new(NullValueNode));
        registry.register(Arc::new(InstantNode));

        let executor = WorkflowExecutor::new(Arc::new(registry), Arc::new(NoopCredentials));
        // NullValueNode → InstantNode; downstream must handle null context without panic.
        let workflow = two_node_typed_workflow("null_value_test", "instant_test");

        let result = executor.run(Arc::new(workflow), HashMap::new()).await;
        assert!(result.is_ok());
        let r = result.unwrap();
        assert!(r.success, "workflow must succeed; got error: {:?}", r.error);
        assert_eq!(
            r.node_outputs.get("n_up"),
            Some(&serde_json::Value::Null),
            "null-output node must store Value::Null in node_outputs"
        );
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

    // ── T1-1: resolve_taken_port recognizes Switch's "port" field ─────────────
    //
    // resolve_taken_port had no case for the "port" field switch.rs's output
    // carries (case_1..case_8/default) — every Switch execution fell through
    // to the hardcoded "output" fallback, which matches no edge a Switch node
    // can actually have, so activate_successors dropped every branch.

    #[test]
    fn resolve_taken_port_reads_switch_port_field() {
        let output = NodeOutput::success(serde_json::json!({
            "matched_case": "ok",
            "port": "case_3",
            "value": "ok",
            "data": {}
        }));
        assert_eq!(WorkflowExecutor::resolve_taken_port(&output), "case_3");
    }

    #[test]
    fn resolve_taken_port_falls_back_to_output_when_no_recognized_field() {
        let output = NodeOutput::success(serde_json::json!({ "some_field": "value" }));
        assert_eq!(WorkflowExecutor::resolve_taken_port(&output), "output");
    }

    // Outputs { "status": "ok" } for the Switch node under test to route on.
    struct SwitchSourceNode;
    #[async_trait::async_trait]
    impl Node for SwitchSourceNode {
        fn type_id(&self)        -> &'static str { "switch_source_test" }
        fn display_name(&self)   -> &'static str { "Switch Source Test" }
        fn node_type(&self)      -> NodeType     { NodeType::Utility }
        fn version(&self)        -> &'static str { "1.0" }
        fn input_schema(&self)   -> serde_json::Value { serde_json::json!({}) }
        fn output_schema(&self)  -> serde_json::Value { serde_json::json!({}) }
        async fn execute(&self, _: crate::model::NodeInput) -> crate::model::NodeOutput {
            crate::model::NodeOutput::success(serde_json::json!({ "status": "ok" }))
        }
    }

    // Records that it ran. Wired to the Switch node's "case_1" port — the
    // branch that must activate for a "status" == "ok" match.
    struct CaseOneMarkerNode { ran: Arc<Mutex<bool>> }
    #[async_trait::async_trait]
    impl Node for CaseOneMarkerNode {
        fn type_id(&self)        -> &'static str { "case_one_marker_test" }
        fn display_name(&self)   -> &'static str { "Case One Marker Test" }
        fn node_type(&self)      -> NodeType     { NodeType::Utility }
        fn version(&self)        -> &'static str { "1.0" }
        fn input_schema(&self)   -> serde_json::Value { serde_json::json!({}) }
        fn output_schema(&self)  -> serde_json::Value { serde_json::json!({}) }
        async fn execute(&self, _: crate::model::NodeInput) -> crate::model::NodeOutput {
            *self.ran.lock().unwrap() = true;
            crate::model::NodeOutput::success(serde_json::json!({}))
        }
    }

    // Records that it ran. Wired to the Switch node's "case_2" port — the
    // branch that must NOT activate for a "status" == "ok" match.
    struct CaseTwoMarkerNode { ran: Arc<Mutex<bool>> }
    #[async_trait::async_trait]
    impl Node for CaseTwoMarkerNode {
        fn type_id(&self)        -> &'static str { "case_two_marker_test" }
        fn display_name(&self)   -> &'static str { "Case Two Marker Test" }
        fn node_type(&self)      -> NodeType     { NodeType::Utility }
        fn version(&self)        -> &'static str { "1.0" }
        fn input_schema(&self)   -> serde_json::Value { serde_json::json!({}) }
        fn output_schema(&self)  -> serde_json::Value { serde_json::json!({}) }
        async fn execute(&self, _: crate::model::NodeInput) -> crate::model::NodeOutput {
            *self.ran.lock().unwrap() = true;
            crate::model::NodeOutput::success(serde_json::json!({}))
        }
    }

    // n_status → n_switch, whose "case_1"/"case_2" ports fan out to n_case1/
    // n_case2. Switch config matches "status" == "ok" to case_1, so only
    // n_case1 should activate.
    fn switch_two_branch_workflow() -> Workflow {
        use crate::model::WorkflowEdge;
        let status = WorkflowNode {
            id: "n_status".to_string(),
            node_type_id: "switch_source_test".to_string(),
            node_type: NodeType::Utility,
            name: "Status Source".to_string(),
            config: serde_json::json!({}),
            credentials: HashMap::new(),
            input_schema: serde_json::json!({}),
            output_schema: serde_json::json!({}),
            retry: Default::default(),
            fallback_node: None,
            disabled: false,
            position: Default::default(),
        };
        let switch = WorkflowNode {
            id: "n_switch".to_string(),
            node_type_id: "switch".to_string(),
            node_type: NodeType::Logic,
            name: "Switch".to_string(),
            config: serde_json::json!({
                "field": "status",
                "source_node": "n_status",
                "cases": r#"[{"match":"ok","port":"case_1"},{"match":"error","port":"case_2"}]"#
            }),
            credentials: HashMap::new(),
            input_schema: serde_json::json!({}),
            output_schema: serde_json::json!({}),
            retry: Default::default(),
            fallback_node: None,
            disabled: false,
            position: Default::default(),
        };
        let case1 = WorkflowNode {
            id: "n_case1".to_string(),
            node_type_id: "case_one_marker_test".to_string(),
            node_type: NodeType::Utility,
            name: "Case 1 Marker".to_string(),
            config: serde_json::json!({}),
            credentials: HashMap::new(),
            input_schema: serde_json::json!({}),
            output_schema: serde_json::json!({}),
            retry: Default::default(),
            fallback_node: None,
            disabled: false,
            position: Default::default(),
        };
        let case2 = WorkflowNode {
            id: "n_case2".to_string(),
            node_type_id: "case_two_marker_test".to_string(),
            node_type: NodeType::Utility,
            name: "Case 2 Marker".to_string(),
            config: serde_json::json!({}),
            credentials: HashMap::new(),
            input_schema: serde_json::json!({}),
            output_schema: serde_json::json!({}),
            retry: Default::default(),
            fallback_node: None,
            disabled: false,
            position: Default::default(),
        };
        let edges = vec![
            WorkflowEdge {
                id: "e_status_switch".to_string(),
                from_node: "n_status".to_string(),
                from_port: "output".to_string(),
                to_node: "n_switch".to_string(),
                to_port: "input".to_string(),
                condition: None,
                on_success: None,
                on_failure: None,
            },
            WorkflowEdge {
                id: "e_case1".to_string(),
                from_node: "n_switch".to_string(),
                from_port: "case_1".to_string(),
                to_node: "n_case1".to_string(),
                to_port: "input".to_string(),
                condition: None,
                on_success: None,
                on_failure: None,
            },
            WorkflowEdge {
                id: "e_case2".to_string(),
                from_node: "n_switch".to_string(),
                from_port: "case_2".to_string(),
                to_node: "n_case2".to_string(),
                to_port: "input".to_string(),
                condition: None,
                on_success: None,
                on_failure: None,
            },
        ];
        Workflow {
            schema_version: CURRENT_VERSION.to_string(),
            id: "wf_switch_two_branch".to_string(),
            name: "Switch Two Branch".to_string(),
            description: String::new(),
            nodes: vec![status, switch, case1, case2],
            edges,
            metadata: Default::default(),
            max_duration_secs: None,
            parallel_execution: false,
            max_concurrent_nodes: None,
            settings: Default::default(),
        }
    }

    #[tokio::test]
    async fn switch_node_activates_only_matched_case_branch() {
        let case1_ran: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));
        let case2_ran: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));

        let mut registry = NodeRegistry::new();
        registry.register(Arc::new(SwitchSourceNode));
        registry.register(Arc::new(crate::nodes::switch::SwitchNode));
        registry.register(Arc::new(CaseOneMarkerNode { ran: Arc::clone(&case1_ran) }));
        registry.register(Arc::new(CaseTwoMarkerNode { ran: Arc::clone(&case2_ran) }));

        let executor = WorkflowExecutor::new(Arc::new(registry), Arc::new(NoopCredentials));
        let result = executor.run(Arc::new(switch_two_branch_workflow()), HashMap::new()).await;

        assert!(result.is_ok(), "workflow failed: {:?}", result.err());
        assert!(result.unwrap().success, "workflow did not succeed");
        assert!(
            *case1_ran.lock().unwrap(),
            "matched case_1 branch never activated — resolve_taken_port regressed"
        );
        assert!(
            !*case2_ran.lock().unwrap(),
            "unmatched case_2 branch activated — routing is not selective"
        );
    }
}
