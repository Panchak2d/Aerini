//! `aerini-engine` — the core library shared by the desktop app and the server binary.
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
//! | [`mem_tracking`] | Per-workflow/per-node live memory attribution (`#[global_allocator]`) |
//! | [`expression`] | `{{...}}` template resolver with 40 inline functions |
//! | [`scheduler`] | `SchedulerDaemon` — background job loop for all trigger kinds |
//! | [`db`] | SQLite persistence: workflows, run records, versions, settings |
//! | [`migration`] | Workflow JSON schema migration engine; `MigrationEngine::apply()` |
//! | [`store`] | AES-256-GCM encrypted credential store |
//! | [`cron`] | 5-field cron parser; `@hourly`/`@daily` macros; named weekdays/months |
//! | [`error`] | `EngineError` (thiserror); `NodeError` |
//! | [`provider`] | `ProviderRegistry` — AI provider metadata, auth headers, URL detection; `shared_ai_client()` |
//!
//! # Key design constraint — no Tauri dependency
//!
//! This crate must never import `tauri`. The decoupling point is [`EventSink`]:
//! the Tauri app wraps `tauri::AppHandle`; `aerini-server` uses an SSE broadcast channel.
//! The executor and scheduler hold only `Arc<dyn EventSink>` and are runtime-agnostic.

// global allocator for per-workflow/per-node live
// memory attribution. Must be declared before first use, at crate root.
// Safe to declare here rather than in either binary crate: a
// `#[global_allocator]` applies to the whole final binary regardless of
// which crate in its dependency graph declares it, and — confirmed via a
// repo-wide grep this fix — no other crate in this workspace declares one,
// so `aerini` (desktop), `aerini-server`, and this crate's own `cargo test`
// harness each get exactly one, automatically, with no per-binary wiring
// needed beyond calling `mem_tracking::install()` once at startup (see
// `src-tauri/src/lib.rs`'s `.setup()` and `aerini-server/src/main.rs`).
#[global_allocator]
static GLOBAL_ALLOCATOR: tracking_allocator::Allocator<std::alloc::System> =
    tracking_allocator::Allocator::system();

pub mod context;
pub mod cron;
pub mod db;
pub mod error;
pub mod executor;
pub mod expression;
pub mod graph;
pub mod mem_tracking;
pub mod migration;
pub mod model;
pub mod node;
pub mod nodes;
pub mod perf_monitor;
mod plugin_http;
pub mod plugin_loader;
pub mod provider;
pub mod scheduler;
pub mod store;

// Top-level re-exports used by both tauri app and server binary.
pub use model::Workflow;
pub use node::NodeRegistry;

/// The current engine version. Included in run records and API responses.
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

// ── EventSink ─────────────────────────────────────────────────────────────────

/// Receives structured events emitted by the executor as a workflow runs.
///
/// In the desktop app this routes events to Tauri's IPC layer; in the server
/// binary it broadcasts over an SSE channel. Embedders implement this trait to
/// forward events to whatever consumer fits their context (log sink, WebSocket,
/// in-memory queue, etc.).
///
/// # Non-blocking contract
///
/// `emit()` must return quickly — do not `await` inside it. Hand off to an
/// async consumer via a channel (`tokio::sync::mpsc::unbounded_channel` works
/// well) rather than blocking the calling thread.
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
