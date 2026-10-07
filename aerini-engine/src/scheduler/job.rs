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
use thiserror::Error;

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
        /// Suppresses re-firing the workflow for a request whose body was
        /// already seen within this many seconds (0 = disabled). Added after
        /// the original four fields — `#[serde(default)]` keeps older
        /// persisted rows (missing this field) deserializing as `0`,
        /// unchanged behavior.
        #[serde(default)]
        dedup_window_secs: u64,
    },
    /// No automatic trigger — only fires via manual Run press.
    /// Background scheduling is not supported for this trigger type.
    Manual,
    /// Sourced from a trigger-capable WASM plugin (`aerini-node-with-trigger`'s
    /// `trigger` export). `type_id` identifies which installed plugin acts as
    /// the source, resolved against the configured plugin directory at arm
    /// time — not a path, so this stays valid across a plugin reinstall at
    /// the same type_id. `config` is the trigger node's resolved
    /// configuration, JSON-encoded as a single object string, passed
    /// through unchanged to the plugin's `events(config)` call.
    Plugin { type_id: String, config: String },
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
            TriggerKind::Plugin { .. }   => "Plugin",
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
            TriggerKind::Plugin { type_id, .. }     => format!("Plugin trigger: {}", type_id),
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

/// Placeholder written into a redacted row's `trigger_kind` in place of a
/// real webhook secret. Deliberately non-empty so it can't be mistaken for
/// an unset/empty secret.
const REDACTED_WEBHOOK_SECRET: &str = "<redacted>";

impl ScheduledJobRow {
    /// Discriminant of the stored trigger, matching `TriggerKind`'s serde tag.
    /// `None` if `trigger_kind` isn't valid `TriggerKind` JSON.
    pub fn trigger_type(&self) -> Option<String> {
        let kind = serde_json::from_str::<TriggerKind>(&self.trigger_kind).ok()?;
        Some(match kind {
            TriggerKind::Interval { .. } => "interval",
            TriggerKind::Cron { .. }     => "cron",
            TriggerKind::Once { .. }     => "once",
            TriggerKind::Webhook { .. }  => "webhook",
            TriggerKind::Manual          => "manual",
            TriggerKind::Plugin { .. }   => "plugin",
        }.to_string())
    }

    /// Returns a copy with any `Webhook` trigger's `secret` replaced by
    /// [`REDACTED_WEBHOOK_SECRET`]. Every other field, and every other
    /// trigger kind, is unchanged.
    ///
    /// Internal engine consumers (`scheduler/mod.rs`, `scheduler/runner.rs`)
    /// need the real secret to arm listeners and validate incoming webhook
    /// requests, so they must keep reading rows straight from `SchedulerDb`.
    /// This method exists only for call sites that hand a row to something
    /// outside the engine's trust boundary — an HTTP response or an IPC
    /// reply — where the raw secret would otherwise leak.
    pub fn redacted(&self) -> Self {
        let mut row = self.clone();
        if let Ok(TriggerKind::Webhook { port, path, method, dedup_window_secs, .. }) =
            serde_json::from_str::<TriggerKind>(&row.trigger_kind)
        {
            let redacted_trigger = TriggerKind::Webhook {
                port,
                path,
                method,
                secret: REDACTED_WEBHOOK_SECRET.to_string(),
                dedup_window_secs,
            };
            if let Ok(json) = serde_json::to_string(&redacted_trigger) {
                row.trigger_kind = json;
            }
        }
        row
    }
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
    /// `"interval" | "cron" | "once" | "webhook" | "manual" | "plugin"`.
    /// `None` when the job row is missing or its stored trigger can't be parsed.
    pub trigger_type:  Option<String>,
    /// Full `WorkflowResult`. Populated **only** on the post-run emission inside
    /// `fire_once_with_vars` (status `"waiting"` on workflow success, `"error"`
    /// on workflow failure). `None` everywhere else this event is constructed —
    /// `runner.rs`'s `emit_waiting`/`emit_error`/`emit_done_async` (and their
    /// sync/async siblings) and `SchedulerDaemon::emit_status` in
    /// `scheduler/mod.rs` all set it to `None` explicitly, including the
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

#[derive(Debug, Clone, Serialize, Deserialize, Error)]
#[serde(tag = "error_kind", rename_all = "snake_case")]
pub enum SchedulerError {
    #[error("port {} is already in use by \"{}\"", .0.port, .0.held_by_workflow_name)]
    PortConflict(PortConflict),
    #[error("workflow not found")]
    WorkflowNotFound,
    #[error("workflow is not schedulable (no Schedule, Webhook, or trigger-plugin trigger)")]
    NotSchedulable,
    #[error("workflow is already running")]
    AlreadyRunning,
    #[error("{message}")]
    Other { message: String },
}

impl From<SchedulerError> for String {
    fn from(e: SchedulerError) -> String {
        serde_json::to_string(&e).unwrap_or_else(|_| "scheduler_error".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row_with_trigger(trigger: &TriggerKind) -> ScheduledJobRow {
        ScheduledJobRow {
            workflow_id:   "wf-1".to_string(),
            workflow_name: "Test Workflow".to_string(),
            trigger_kind:  serde_json::to_string(trigger).unwrap(),
            status:        "active".to_string(),
            always_on:     false,
            run_count:     0,
            last_run_at:   None,
            next_run_at:   None,
            last_error:    None,
            created_at:    "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn trigger_type_names_each_kind_and_is_none_for_unparseable_json() {
        assert_eq!(row_with_trigger(&TriggerKind::Interval { secs: 5 }).trigger_type().as_deref(), Some("interval"));
        assert_eq!(row_with_trigger(&TriggerKind::Manual).trigger_type().as_deref(), Some("manual"));
        let plugin = TriggerKind::Plugin { type_id: "p".to_string(), config: "{}".to_string() };
        assert_eq!(row_with_trigger(&plugin).trigger_type().as_deref(), Some("plugin"));
        let mut bad = row_with_trigger(&TriggerKind::Manual);
        bad.trigger_kind = "not json".to_string();
        assert_eq!(bad.trigger_type(), None);
    }

    #[test]
    fn redacted_strips_webhook_secret_but_keeps_port_path_method() {
        let trigger = TriggerKind::Webhook {
            port:   3456,
            path:   "/hook".to_string(),
            method: "POST".to_string(),
            secret: "super-secret-value".to_string(),
            dedup_window_secs: 0,
        };
        let row      = row_with_trigger(&trigger);
        let redacted = row.redacted();

        let parsed: TriggerKind = serde_json::from_str(&redacted.trigger_kind).unwrap();
        match parsed {
            TriggerKind::Webhook { port, path, method, secret, .. } => {
                assert_eq!(port, 3456);
                assert_eq!(path, "/hook");
                assert_eq!(method, "POST");
                assert_eq!(secret, REDACTED_WEBHOOK_SECRET);
                assert_ne!(secret, "super-secret-value");
            }
            other => panic!("expected Webhook trigger, got {:?}", other),
        }
    }

    #[test]
    fn redacted_leaves_non_webhook_triggers_untouched() {
        let trigger = TriggerKind::Cron { expr: "0 * * * *".to_string() };
        let row      = row_with_trigger(&trigger);
        let redacted = row.redacted();
        assert_eq!(redacted.trigger_kind, row.trigger_kind);
    }

    #[test]
    fn webhook_json_predating_dedup_window_secs_deserializes_with_default_zero() {
        let legacy_json = r#"{"kind":"webhook","port":3456,"path":"/hook","method":"POST","secret":"s"}"#;
        let trigger: TriggerKind = serde_json::from_str(legacy_json)
            .expect("pre-dedup persisted rows must still deserialize");
        match trigger {
            TriggerKind::Webhook { dedup_window_secs, .. } => assert_eq!(dedup_window_secs, 0),
            other => panic!("expected Webhook trigger, got {:?}", other),
        }
    }

    #[test]
    fn redacted_leaves_every_other_field_untouched() {
        let trigger  = TriggerKind::Interval { secs: 60 };
        let mut row  = row_with_trigger(&trigger);
        row.run_count   = 7;
        row.last_error  = Some("boom".to_string());
        row.always_on   = true;
        let redacted = row.redacted();
        assert_eq!(redacted.workflow_id, row.workflow_id);
        assert_eq!(redacted.workflow_name, row.workflow_name);
        assert_eq!(redacted.status, row.status);
        assert_eq!(redacted.always_on, row.always_on);
        assert_eq!(redacted.run_count, row.run_count);
        assert_eq!(redacted.last_error, row.last_error);
        assert_eq!(redacted.created_at, row.created_at);
    }
}
