//! Read-side Tauri commands backing the Monitor panel's memory stats:
//! the per-run allocator breakdown below, and process-wide RSS further down.

use sysinfo::{ProcessesToUpdate, System};

/// Returns a snapshot of every workflow run currently in flight on this
/// process, each with its currently-executing node(s) nested underneath.
///
/// No Tauri state needed — `aerini_engine::mem_tracking` is a self-contained
/// global registry, the same one the periodic `memory-breakdown` event
/// (spawned in `lib.rs`'s `.setup()`) reads from. This command exists
/// alongside that event for the frontend's initial paint (before the first
/// 2s tick lands) and for an on-demand refresh — the event is still the
/// primary delivery mechanism, not this.
#[tauri::command]
pub fn get_memory_breakdown() -> Vec<aerini_engine::mem_tracking::RunBreakdown> {
    aerini_engine::mem_tracking::snapshot()
}

/// This process's own resident set size, independent of the per-run
/// allocator attribution above — see that command's doc for how the two
/// differ. `current_bytes` is `Process::memory()`, which is documented
/// upstream (sysinfo 0.39.6) as bytes of RSS, not kB.
#[derive(serde::Serialize)]
pub struct ProcessMemory {
    pub current_bytes: u64,
}

/// `None` only if the current process id can't be resolved (sysinfo
/// reports this as possible on unsupported platforms) or has no matching
/// entry right after a scoped refresh — both are pre-existing sysinfo
/// fallibility, not this command's own logic, so no error is fabricated
/// past that point.
///
/// Refreshes a single pid (this process's own) rather than the full
/// process table on every poll — the same "don't scan everything, scan
/// what changed" discipline `perf_monitor`'s own sampler already follows.
#[tauri::command]
pub fn get_process_memory() -> Option<ProcessMemory> {
    let pid = sysinfo::get_current_pid().ok()?;
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    sys.process(pid)
        .map(|p| ProcessMemory { current_bytes: p.memory() })
}
