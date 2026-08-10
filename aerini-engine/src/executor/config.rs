//! Builder configuration for [`super::WorkflowExecutor`].
//!
//! Holds every field set through a `with_*` call on [`super::WorkflowExecutor`].
//! `WorkflowExecutor` keeps its own `with_*` methods (public API, unchanged
//! signatures) and delegates each one to the matching method here.

use std::collections::HashSet;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::EventSink;

#[derive(Clone)]
pub(crate) struct WorkflowExecutorConfig {
    // Optional: when None, node-status events are dropped silently.
    pub(super) event_sink:          Option<Arc<dyn EventSink>>,
    // Optional: when None, {{$env.VAR}} expressions resolve to "" with a warning.
    pub(super) env_allowlist:       Option<Arc<HashSet<String>>>,
    // Optional: when set, FileNode restricts all paths to this directory tree.
    pub(super) file_sandbox_dir:    Option<Arc<std::path::PathBuf>>,
    // When true, ShellExecNode returns SHELL_DISABLED immediately without executing.
    pub(super) shell_exec_disabled: bool,
    // When true, CodeNode returns CODE_DISABLED immediately without executing.
    pub(super) code_exec_disabled:  bool,
    // When true, DatabaseNode returns DATABASE_DISABLED immediately without connecting.
    pub(super) database_exec_disabled: bool,
    // When true, input schema violations fail the node. When false (default), they log a warning.
    pub(super) strict_schema_validation: bool,
    // When true, independent branches execute concurrently via tokio tasks. Default: false.
    pub(super) parallel_execution: bool,
    // Maximum simultaneous node tasks when parallel_execution is true. Default: 8.
    pub(super) max_concurrent_nodes: usize,
    // When set, the executor checks this token between nodes, short-circuits retry
    // backoff sleeps, and races it against every execute() attempt so an in-flight
    // node is interrupted directly rather than only checked between nodes.
    pub(super) cancel_token: Option<CancellationToken>,
    // Server-level ceiling applied to every workflow regardless of per-workflow max_duration_secs.
    pub(super) server_max_duration_secs: Option<u64>,
    // When true, run() applies no timeout at all — wins over both server_max_duration_secs
    // and workflow.max_duration_secs. Default: false.
    pub(super) unlimited_duration: bool,
    // When true, nodes gated on admin scope (e.g. allow_raw_sql) are unlocked.
    pub(super) caller_is_admin: bool,
    // When true, Code (JS) nodes run with module import restrictions and OS resource limits.
    // Only meaningful when code_exec_disabled is false.
    pub(super) code_sandbox_enabled: bool,
    // When set, overrides the default 512 MB RLIMIT_AS cap for Code (JS) node subprocesses.
    // Linux only; ignored on macOS and Windows.
    pub(super) code_max_memory_mb: Option<u64>,
}

impl Default for WorkflowExecutorConfig {
    fn default() -> Self {
        Self {
            event_sink:             None,
            env_allowlist:          None,
            file_sandbox_dir:       None,
            shell_exec_disabled:    false,
            code_exec_disabled:     false,
            database_exec_disabled: false,
            strict_schema_validation: false,
            parallel_execution:     false,
            max_concurrent_nodes:   8,
            cancel_token:           None,
            server_max_duration_secs: None,
            unlimited_duration:     false,
            caller_is_admin:        false,
            code_sandbox_enabled:   false,
            code_max_memory_mb:     None,
        }
    }
}

impl WorkflowExecutorConfig {
    pub(super) fn with_env_allowlist(mut self, vars: Vec<String>) -> Self {
        self.env_allowlist = Some(Arc::new(vars.into_iter().collect()));
        self
    }

    pub(super) fn with_event_sink(mut self, sink: Arc<dyn EventSink>) -> Self {
        self.event_sink = Some(sink);
        self
    }

    pub(super) fn with_file_sandbox_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.file_sandbox_dir = Some(Arc::new(dir));
        self
    }

    pub(super) fn with_shell_disabled(mut self, disabled: bool) -> Self {
        self.shell_exec_disabled = disabled;
        self.validate();
        self
    }

    pub(super) fn with_code_disabled(mut self, disabled: bool) -> Self {
        self.code_exec_disabled = disabled;
        self.validate();
        self
    }

    pub(super) fn with_database_disabled(mut self, disabled: bool) -> Self {
        self.database_exec_disabled = disabled;
        self
    }

    pub(super) fn with_strict_schema_validation(mut self, strict: bool) -> Self {
        self.strict_schema_validation = strict;
        self
    }

    pub(super) fn with_parallel_execution(mut self, enabled: bool) -> Self {
        self.parallel_execution = enabled;
        self
    }

    pub(super) fn with_max_concurrent_nodes(mut self, limit: usize) -> Self {
        self.max_concurrent_nodes = limit;
        self.validate();
        self
    }

    pub(super) fn with_cancel_token(mut self, t: CancellationToken) -> Self {
        self.cancel_token = Some(t);
        self
    }

    pub(super) fn with_server_max_duration_secs(mut self, secs: Option<u64>) -> Self {
        self.server_max_duration_secs = secs.map(|s| s.clamp(10, 86400));
        self
    }

    pub(super) fn with_unlimited_duration(mut self, enabled: bool) -> Self {
        self.unlimited_duration = enabled;
        self
    }

    pub(super) fn with_caller_is_admin(mut self, is_admin: bool) -> Self {
        self.caller_is_admin = is_admin;
        self
    }

    pub(super) fn with_code_sandbox(mut self, enabled: bool) -> Self {
        self.code_sandbox_enabled = enabled;
        self
    }

    pub(super) fn with_code_max_memory_mb(mut self, mb: Option<u64>) -> Self {
        self.code_max_memory_mb = mb;
        self
    }

    /// Re-checks invariants that a single `with_*` call can't fully enforce on
    /// its own. Called automatically by `with_max_concurrent_nodes` (clamp) and
    /// by `with_shell_disabled` / `with_code_disabled` (informational warning).
    /// Not part of the public API — `WorkflowExecutor`'s builder methods are
    /// the only public surface, and their signatures are unchanged.
    ///
    /// - `max_concurrent_nodes` below 1 is silently clamped to 1 (preserves the
    ///   exact clamping behavior `with_max_concurrent_nodes` always had).
    /// - Logs (does not error) if both shell and code execution are disabled
    ///   at once — informational only, two independent features being off
    ///   simultaneously is a valid configuration, not a contradiction worth
    ///   failing on.
    fn validate(&mut self) {
        if self.max_concurrent_nodes < 1 {
            self.max_concurrent_nodes = 1;
        }
        if self.shell_exec_disabled && self.code_exec_disabled {
            tracing::warn!(
                "WorkflowExecutorConfig: shell and code execution are both disabled \
                 simultaneously — informational only, no action needed if intentional"
            );
        }
    }
}
