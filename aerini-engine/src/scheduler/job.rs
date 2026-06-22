//! Scheduler data types — wire-safe, shared between engine and server binary.
//!
//! # Key types
//!
//! - [`TriggerKind`] — serde-tagged enum; field names are part of the JSON wire contract.
//!   Tag field is `"kind"`, variants are `snake_case`.
//! - [`ScheduledJobRow`] — mirrors the `scheduled_jobs` SQLite table; returned by
//!   `get_scheduled_jobs` IPC command.
//! - [`SchedulerStatusEvent`] — emitted to the frontend on every job state transition.
//! - [`SchedulerError`] — structured error returned as JSON so the frontend can
//!   pattern-match on `error_kind` and show the appropriate modal (e.g. port-conflict dialog).
//!
//! # Frozen
//!
//! All serde field names and tag values in this module are frozen wire contracts.
//! Changing them breaks the TypeScript frontend and any server API clients.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TriggerKind {
    /// Run every N seconds, forever.
    Interval { secs: u64 },
    /// Run on a cron schedule, forever.
    Cron { expr: String },
    /// Run once at a specific UTC time, then mark done.
    Once { run_at: DateTime<Utc> },
    /// Listen on a TCP port for HTTP requests; re-arm after each request.
    Webhook {
        port: u16,
        path: String,
        method: String,
        secret: String,
    },
    /// No automatic trigger — only fires via manual Run press.
    /// Background scheduling is not supported for this trigger type.
    Manual,
}

impl TriggerKind {
    /// Human-readable label shown in the UI.
    #[allow(dead_code)]
    pub fn label(&self) -> &'static str {
        match self {
            TriggerKind::Interval { .. } => "Interval",
            TriggerKind::Cron { .. }     => "Cron",
            TriggerKind::Once { .. }     => "Once",
            TriggerKind::Webhook { .. }  => "Webhook",
            TriggerKind::Manual          => "Manual",
        }
    }

    /// Returns true if this trigger can sustain an always-on background loop.
    pub fn is_schedulable(&self) -> bool {
        !matches!(self, TriggerKind::Manual)
    }

    /// Human-readable description for log output and server status pages.
    pub fn describe(&self) -> String {
        match self {
            TriggerKind::Interval { secs }          => format!("Every {} seconds", secs),
            TriggerKind::Cron { expr }              => format!("Cron: {}", expr),
            TriggerKind::Once { run_at }            => format!("Once at {}", run_at),
            TriggerKind::Webhook { port, path, .. } => format!("Webhook on :{}{}", port, path),
            TriggerKind::Manual                     => "Manual".to_string(),
        }
    }
}

/// Mirrors the `scheduled_jobs` SQLite table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledJobRow {
    pub workflow_id:   String,
    pub workflow_name: String,
    /// JSON-serialised TriggerKind.
    pub trigger_kind:  String,
    /// "active" | "paused" | "done" | "error" | "stopped"
    pub status:        String,
    /// If true: auto-arms on every app launch without user interaction.
    pub always_on:     bool,
    pub run_count:     i64,
    pub last_run_at:   Option<String>,
    pub next_run_at:   Option<String>,
    pub last_error:    Option<String>,
    pub created_at:    String,
}

/// Emitted to the frontend on every state transition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerStatusEvent {
    pub workflow_id:   String,
    pub workflow_name: String,
    /// "waiting" | "running" | "done" | "error" | "stopped"
    pub status:        String,
    pub run_count:     i64,
    pub last_run_at:   Option<String>,
    pub next_run_at:   Option<String>,
    pub last_error:    Option<String>,
    /// Full `WorkflowResult`. Populated **only** on the post-run emission inside
    /// `fire_once_with_vars` (status `"waiting"` on workflow success, `"error"`
    /// on workflow failure). `None` everywhere else this event is constructed —
    /// `emit_waiting`, `emit_error`, `emit_done`, and `SchedulerDaemon::emit_status`
    /// in `scheduler/mod.rs` all set it to `None` explicitly, including the
    /// re-arm `"waiting"` event and the genuine one-shot `"done"` status.
    /// (Verified by reading every `SchedulerStatusEvent { .. }` construction
    /// site in `runner.rs` and `mod.rs`.)
    pub last_result:   Option<serde_json::Value>,
}

/// Returned as structured JSON so the frontend can pattern-match and show
/// the three-option modal rather than a generic error toast.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortConflict {
    pub port:                    u16,
    pub held_by_workflow_id:   String,
    pub held_by_workflow_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "error_kind", rename_all = "snake_case")]
pub enum SchedulerError {
    PortConflict(PortConflict),
    WorkflowNotFound,
    NotSchedulable,
    AlreadyRunning,
    Other { message: String },
}

impl From<SchedulerError> for String {
    fn from(e: SchedulerError) -> String {
        serde_json::to_string(&e).unwrap_or_else(|_| "scheduler_error".to_string())
    }
}
