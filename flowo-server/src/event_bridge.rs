use chrono::Utc;
use flowo_engine::db::{RunRecord, WorkflowDb};
use flowo_engine::executor::WorkflowResult;
use flowo_engine::EventSink;
use serde_json::Value;
use std::sync::{Arc, RwLock};

use crate::log_buffer::LogBuffer;

/// Shared run state — updated by the event bridge, read by the status page.
#[derive(Debug, Clone)]
pub struct RunState {
    pub status:        String,
    pub last_run_at:   Option<String>,
    pub next_run_at:   Option<String>,
    pub run_count:     u64,
    pub last_error:    Option<String>,
    pub last_duration_ms: Option<u64>,
}

impl Default for RunState {
    fn default() -> Self {
        Self {
            status:           "starting".to_string(),
            last_run_at:      None,
            next_run_at:      None,
            run_count:        0,
            last_error:       None,
            last_duration_ms: None,
        }
    }
}

pub type SharedRunState = Arc<RwLock<RunState>>;

/// Implements EventSink by writing structured events to the log buffer and
/// updating the shared RunState for the status page.
pub struct EventBridge {
    pub log:          LogBuffer,
    pub state:        SharedRunState,
    run_start:        RwLock<Option<std::time::Instant>>,
    /// When set, completed runs are persisted to SQLite for history across restarts.
    run_history:      Option<Arc<WorkflowDb>>,
    workflow_name:    String,
}

impl EventBridge {
    pub fn new(log: LogBuffer, state: SharedRunState) -> Self {
        Self {
            log,
            state,
            run_start:     RwLock::new(None),
            run_history:   None,
            workflow_name: String::new(),
        }
    }

    /// Attach a persistent run history store. Call before the first run.
    pub fn with_run_history(mut self, db: Arc<WorkflowDb>, workflow_name: String) -> Self {
        self.run_history   = Some(db);
        self.workflow_name = workflow_name;
        self
    }
}

impl EventSink for EventBridge {
    fn emit(&self, event: &str, payload: Value) {
        match event {
            "node-status" => {
                let node_id = payload["node_id"].as_str().unwrap_or("?");
                let status  = payload["status"].as_str().unwrap_or("?");
                let level   = match status {
                    "error"  => "ERROR",
                    "running" => "INFO",
                    _        => "INFO",
                };
                self.log.push(level, Some(node_id),
                    format!("Node '{}' → {}", node_id, status));

                if status == "running" {
                    *self.run_start.write().expect("event_bridge run_start RwLock poisoned") = Some(std::time::Instant::now());
                }
            }
            "scheduler-status" => {
                let status     = payload["status"].as_str().unwrap_or("?");
                let next_run   = payload["next_run_at"].as_str().map(|s| s.to_string());
                let last_error = payload["last_error"].as_str()
                    .map(flowo_engine::nodes::util::scrub_url_in_error);
                let run_count  = payload["run_count"].as_u64().unwrap_or(0);

                // Extract last_result as owned Value before any borrows extend further.
                // Only "waiting" and "error" statuses carry a completed run result.
                let last_result_val: Option<Value> =
                    if status == "waiting" || status == "error" {
                        payload.get("last_result").cloned().filter(|v| !v.is_null())
                    } else {
                        None
                    };

                let duration_ms = if status == "waiting" || status == "error" {
                    self.run_start.read().expect("event_bridge run_start RwLock poisoned")
                        .map(|t| t.elapsed().as_millis() as u64)
                } else {
                    None
                };

                let last_run_at = if status == "waiting" || status == "error" {
                    Some(Utc::now().to_rfc3339())
                } else {
                    None
                };

                let msg = match status {
                    "running"  => "Workflow started".to_string(),
                    "waiting"  => format!("Run complete. Next: {}",
                        next_run.as_deref().unwrap_or("—")),
                    "error"    => format!("Run failed: {}",
                        last_error.as_deref().unwrap_or("unknown error")),
                    "done"     => "One-shot run complete. Exiting.".to_string(),
                    other      => format!("Status: {}", other),
                };

                let level = if status == "error" { "ERROR" } else { "INFO" };
                self.log.push(level, None, &msg);

                let mut s = self.state.write().expect("event_bridge state RwLock poisoned");
                s.status     = status.to_string();
                s.run_count  = run_count;
                s.last_error = last_error;
                if let Some(lr) = last_run_at { s.last_run_at = Some(lr); }
                if let Some(nr) = next_run    { s.next_run_at = Some(nr); }
                if let Some(d)  = duration_ms { s.last_duration_ms = Some(d); }
                drop(s);

                // Persist completed run to SQLite history.
                if let Some(rv) = last_result_val {
                    if let Some(ref history_db) = self.run_history {
                        match serde_json::from_value::<WorkflowResult>(rv) {
                            Ok(wf_result) => {
                                let result_json = serde_json::to_string(&wf_result)
                                    .unwrap_or_default();
                                let record = RunRecord {
                                    id:            wf_result.execution_id.clone(),
                                    workflow_id:   wf_result.workflow_id.clone(),
                                    workflow_name: self.workflow_name.clone(),
                                    ran_at:        Utc::now().to_rfc3339(),
                                    success:       wf_result.success,
                                    duration_ms:   duration_ms.map(|d| d as i64).unwrap_or(0),
                                    result_json,
                                    status: if wf_result.success { "success" } else { "failed" }.to_string(),
                                };
                                if let Err(e) = history_db.save_run(&record) {
                                    self.log.push("WARN", None,
                                        format!("Failed to persist run to history: {}", e));
                                }
                            }
                            Err(e) => {
                                self.log.push("WARN", None,
                                    format!("Failed to deserialize WorkflowResult for history: {}", e));
                            }
                        }
                    }
                }
            }
            other => {
                // Log anything else at debug level for diagnostics
                self.log.push("DEBUG", None,
                    format!("[{}] {}", other, payload));
            }
        }
    }
}

#[derive(Clone)]
pub struct BroadcastEventSink {
    pub tx: tokio::sync::broadcast::Sender<String>,
}

impl flowo_engine::EventSink for BroadcastEventSink {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        let mut msg = String::with_capacity(32 + event.len());
        msg.push_str("{\"event\":\"");
        msg.push_str(event);
        msg.push_str("\",\"payload\":");
        if let Ok(p) = serde_json::to_string(&payload) {
            msg.push_str(&p);
            msg.push('}');
            let _ = self.tx.send(msg);
        }
    }
}
