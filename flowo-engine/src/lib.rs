//! `flowo-engine` — the core library shared by the desktop app and the server binary.
//!
//! # Crate structure
//!
//! | Module | Responsibility |
//! |--------|---------------|
//! | [`model`] | All serde types: `Workflow`, `WorkflowNode`, `WorkflowEdge`, `RetryPolicy` |
//! | [`node`] | `Node` trait, `NodeRegistry`, port definitions |
//! | [`nodes`] | 30 built-in node implementations; `register_builtins()` |
//! | [`executor`] | `WorkflowExecutor` — runs a workflow, resolves expressions, handles retries |
//! | [`graph`] | petgraph wrapper: topological sort, cycle detection, BFS reachability |
//! | [`context`] | `ExecutionState` / `SharedExecutionState` — per-run mutable state |
//! | [`expression`] | `{{...}}` template resolver with 40 inline functions |
//! | [`scheduler`] | `SchedulerDaemon` — background job loop for all trigger kinds |
//! | [`db`] | SQLite persistence: workflows, run records, versions, settings |
//! | [`migration`] | Workflow JSON schema migration engine; `MigrationEngine::apply()` |
//! | [`store`] | AES-256-GCM encrypted credential store |
//! | [`cron`] | 5-field cron parser; `@hourly`/`@daily` macros; named weekdays/months |
//! | [`error`] | `EngineError` (thiserror); `NodeError` |
//!
//! # Key design constraint — no Tauri dependency
//!
//! This crate must never import `tauri`. The decoupling point is [`EventSink`]:
//! the Tauri app wraps `tauri::AppHandle`; `flowo-server` uses an SSE broadcast channel.
//! The executor and scheduler hold only `Arc<dyn EventSink>` and are runtime-agnostic.

pub mod context;
pub mod cron;
pub mod db;
pub mod error;
pub mod executor;
pub mod expression;
pub mod graph;
pub mod migration;
pub mod model;
pub mod node;
pub mod nodes;
pub mod scheduler;
pub mod store;

// Top-level re-exports used by both tauri app and server binary.
pub use model::Workflow;
pub use node::NodeRegistry;

pub const ENGINE_VERSION: &str = "0.2.0";

// ── EventSink ─────────────────────────────────────────────────────────────────
// Abstracts over how execution events are delivered to the consumer.
// The Tauri app wraps tauri::AppHandle; the server uses an SSE broadcast channel.
// Using a trait here is the key step that decouples executor.rs and scheduler/mod.rs
// from any Tauri dependency, making the engine usable in a headless binary.

pub trait EventSink: Send + Sync + 'static {
    /// Emit a named event with a JSON payload to the consumer.
    /// Implementations must be non-blocking — do not await inside emit().
    fn emit(&self, event: &str, payload: serde_json::Value);
}

/// No-op sink — drops all events silently. Used in unit tests and environments
/// where event streaming is not needed.
pub struct NoopEventSink;

impl EventSink for NoopEventSink {
    fn emit(&self, _event: &str, _payload: serde_json::Value) {}
}
