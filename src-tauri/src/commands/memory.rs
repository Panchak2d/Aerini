//! Batch AH (`PLAN_CORE.md` §M) — live per-workflow/per-node memory
//! breakdown, backing the frontend's memory indicator (Batch 13c).

/// Returns a snapshot of every workflow run currently in flight on this
/// process, each with its currently-executing node(s) nested underneath.
///
/// No Tauri state needed — `aerini_engine::mem_tracking` is a self-contained
/// global registry, the same one the periodic `memory-breakdown` event
/// (spawned in `lib.rs`'s `.setup()`) reads from. This command exists
/// alongside that event for the frontend's initial paint (before the first
/// 2s tick lands) and for an on-demand refresh, not as the primary delivery
/// mechanism — see `PLAN_CORE.md` §M.2: "event emission (not polling)".
#[tauri::command]
pub fn get_memory_breakdown() -> Vec<aerini_engine::mem_tracking::RunBreakdown> {
    aerini_engine::mem_tracking::snapshot()
}
