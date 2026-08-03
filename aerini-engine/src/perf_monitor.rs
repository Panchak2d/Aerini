//! Persistent-across-completion performance aggregation — one layer above
//! `mem_tracking`'s ephemeral per-run allocator groups.
//!
//! # Why this module exists, not just a bigger `mem_tracking`
//!
//! `mem_tracking::GROUPS` holds a run's live-byte entry only while its
//! `TrackedGroup` guard is alive — by design, it drops the instant the run's
//! future resolves. So a plain read of `mem_tracking::snapshot()` after a run
//! ends returns `[]`: nothing left to look at, even though the run's memory
//! profile is exactly what a caller would want to inspect right after
//! completion.
//!
//! Extending `mem_tracking` itself to *not* drop on completion would fight
//! its own stated design (idle = zero tracking overhead, ref-counted gate)
//! and conflates two different questions: "what is the allocator doing
//! right now" (that module's job) vs. "what did this run's memory profile
//! look like end to end" (this module's job). Kept as a thin separate
//! layer that *samples* the first and aggregates into the second.
//!
//! # Key — `workflow_id`, not a fresh run id
//!
//! Keyed by `workflow_id`, matching `mem_tracking`'s own key and the exact
//! safety argument `executor::run()` already documents at its `mem_meta`
//! construction site: desktop's `ActiveRunToken`, the server's
//! `ApiState::exec_locks`, and `SchedulerDaemon`'s own exec-lock already
//! guarantee at most one concurrent execution of a given `workflow_id`, so
//! no separate per-call id is needed here either. A caller needing a
//! *per-execution* persistent identity (e.g. the SQLite
//! `performance_reports` table) supplies its own row id at persistence
//! time — the same way `RunRecord.id` already works for `run_history`
//! today (assembled by the caller, not by the engine).
//!
//! # Metric — allocator-attributed bytes, not process RSS
//!
//! Reuses `mem_tracking::snapshot()`'s existing allocator-group byte counts
//! as the sole live-metric source. Whole-process RSS is an explicitly
//! *deferred* future metric — no `sysinfo` dependency added here.
//! [`PerformanceReport`] is intentionally flat rather than pre-built around
//! a metrics map for a metric not yet collected.
//!
//! # Status vocabulary
//!
//! Reuses `run_history`'s existing three-value vocabulary — see
//! [`PerfStatus`] — rather than inventing a fourth ("cancelled") value
//! nothing else in this app can currently produce.

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use dashmap::DashMap;
use once_cell::sync::Lazy;
use tokio::time::Duration;

/// Sampling cadence — mid of this feature's 100–250ms spec range, and the
/// same cadence `statusbar-fields.ts`'s own UI poll already uses elsewhere
/// in this product (HIGH confidence: a deliberate consistency pick, not an
/// independently load-tested value).
const SAMPLE_INTERVAL_MS: u64 = 150;

/// Once a run's `history` exceeds this many samples, it is halved (every
/// second sample dropped) rather than grown further — bounded memory
/// regardless of run length, per this feature's "the monitor must not
/// itself become a memory problem" requirement. 512 samples @ 150ms is
/// ~77s of full-resolution history before the first halving; halving is
/// O(n) but runs once per doubling of run duration, not once per tick.
const MAX_HISTORY_SAMPLES: usize = 512;

fn now_ms() -> u64 {
    // Same fallback convention as `mem_tracking::now_ms` (cited) — a
    // pre-1970 system clock isn't worth failing a diagnostics feature over.
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Reuses `run_history`'s existing status vocabulary (see module doc
/// comment, "Status vocabulary").
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerfStatus {
    Running,
    Success,
    Failed,
}

impl PerfStatus {
    /// `run_history`'s `status` column strings, exactly (VERIFIED against
    /// `db::run_history::save_run_started`/`save_run` — neither ever writes
    /// anything besides these plus `"interrupted"`, which is swept in by
    /// `WorkflowDb::open` on next startup, never written by a live run —
    /// not reachable from this enum by construction, so not represented
    /// here).
    pub fn as_db_str(self) -> &'static str {
        match self {
            PerfStatus::Running => "running",
            PerfStatus::Success => "success",
            PerfStatus::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct HistorySample {
    pub at_ms: u64,
    pub bytes: u64,
}

/// A performance profile for one workflow execution.
///
/// Immutable once built (by `finalize()` or `get_live_snapshot()`) — safe
/// to hand to any caller (a SQLite writer, an IPC/REST layer, a UI
/// panel/popover) without further synchronization.
///
/// Deliberately flat (no nested "metrics" map) — only one metric is
/// collected today. Adding a second (RSS, CPU, thread count, ...) later
/// means adding fields here, not retrofitting a map nothing uses yet.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PerformanceReport {
    pub workflow_id: String,
    pub status: PerfStatus,
    pub started_at_ms: u64,
    /// The run's actual completion time when `status != Running`. When
    /// `status == Running` (only possible via `get_live_snapshot`, never
    /// via `finalize`), this is instead "as of" — the timestamp this
    /// particular in-progress reading was taken, not a real completion.
    pub finished_at_ms: u64,
    pub duration_ms: u64,
    pub sampling_interval_ms: u64,
    pub baseline_bytes: u64,
    pub final_bytes: u64,
    pub peak_bytes: u64,
    pub peak_at_ms: u64,
    pub minimum_bytes: u64,
    pub average_bytes: u64,
    /// Signed — a run can legitimately end below its own baseline (a node
    /// frees a large buffer it allocated earlier in the same run).
    pub delta_bytes: i64,
    pub sample_count: u64,
    pub history: Vec<HistorySample>,
}

/// Mutable, in-progress counterpart of [`PerformanceReport`]. Lives in
/// [`LIVE`] for exactly one run's duration.
struct LiveState {
    started_at_ms: u64,
    baseline_bytes: u64,
    current_bytes: AtomicU64,
    peak_bytes: AtomicU64,
    peak_at_ms: AtomicU64,
    minimum_bytes: AtomicU64,
    sum_bytes: AtomicU64,
    sample_count: AtomicU64,
    // A lock, not an atomic Vec (there's no such primitive) — pushes are
    // one per ~150ms tick, and the occasional halving pass touches the
    // whole Vec; neither is a per-allocation hot path, so contention here
    // is not a concern (same reasoning `mem_tracking::TRACKING_GATE`'s own
    // coarser-grained Mutex already relies on).
    history: Mutex<Vec<HistorySample>>,
}

impl LiveState {
    fn new(baseline_bytes: u64) -> Self {
        let started_at_ms = now_ms();
        Self {
            started_at_ms,
            baseline_bytes,
            current_bytes: AtomicU64::new(baseline_bytes),
            peak_bytes: AtomicU64::new(baseline_bytes),
            peak_at_ms: AtomicU64::new(started_at_ms),
            minimum_bytes: AtomicU64::new(baseline_bytes),
            sum_bytes: AtomicU64::new(baseline_bytes),
            // Seeded to 1, not 0: the baseline reading above already counts
            // as sample #1, so a run that finishes inside one sampling
            // period still has a non-garbage average/history instead of a
            // divide-by-zero or an empty series.
            sample_count: AtomicU64::new(1),
            history: Mutex::new(vec![HistorySample { at_ms: started_at_ms, bytes: baseline_bytes }]),
        }
    }

    fn record(&self, bytes: u64, at_ms: u64) {
        self.current_bytes.store(bytes, Ordering::Relaxed);
        self.sum_bytes.fetch_add(bytes, Ordering::Relaxed);
        self.sample_count.fetch_add(1, Ordering::Relaxed);

        // `AtomicU64` has no stable `fetch_max`/`fetch_min` used elsewhere
        // in this crate to follow (`mem_tracking.rs` only uses
        // `fetch_add`/`fetch_sub`) — a compare-exchange loop is the
        // standard portable equivalent.
        let mut prev_peak = self.peak_bytes.load(Ordering::Relaxed);
        while bytes > prev_peak {
            match self.peak_bytes.compare_exchange_weak(
                prev_peak, bytes, Ordering::Relaxed, Ordering::Relaxed,
            ) {
                Ok(_) => { self.peak_at_ms.store(at_ms, Ordering::Relaxed); break; }
                Err(actual) => prev_peak = actual,
            }
        }
        let mut prev_min = self.minimum_bytes.load(Ordering::Relaxed);
        while bytes < prev_min {
            match self.minimum_bytes.compare_exchange_weak(
                prev_min, bytes, Ordering::Relaxed, Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => prev_min = actual,
            }
        }

        let mut hist = self.history.lock().unwrap_or_else(|e| e.into_inner());
        hist.push(HistorySample { at_ms, bytes });
        if hist.len() > MAX_HISTORY_SAMPLES {
            // Halve: keep every 2nd sample. Cheap, deterministic, and
            // preserves overall shape (a spike may lose a little
            // precision, never disappears entirely) — the "efficient
            // downsampling strategy" this feature's spec calls for.
            let halved: Vec<HistorySample> = hist.iter().step_by(2).copied().collect();
            *hist = halved;
        }
    }
}

static LIVE: Lazy<DashMap<String, Arc<LiveState>>> = Lazy::new(DashMap::new);
static RECENT: Lazy<DashMap<String, PerformanceReport>> = Lazy::new(DashMap::new);

/// Sum of every currently-live run/node allocator byte count for one
/// `workflow_id`, straight from `mem_tracking::snapshot()` (VERIFIED —
/// same fields `mem-summary.ts::summarizeMemory` already sums on the
/// frontend; ported here so the aggregation logic lives in one place
/// shared by desktop and server, not duplicated per frontend). `0` both
/// when the workflow isn't running and when it is running but has
/// (legitimately) allocated nothing yet through tracked groups — this
/// function alone can't distinguish those; every caller in this module
/// only calls it while a run is already known to be in flight, so that
/// ambiguity never matters here.
fn total_live_bytes_for(workflow_id: &str) -> u64 {
    crate::mem_tracking::snapshot()
        .into_iter()
        .find(|r| r.workflow_id == workflow_id)
        .map(|r| {
            let nodes_total: i64 = r.nodes.iter().map(|n| n.live_bytes).sum();
            (r.live_bytes + nodes_total).max(0) as u64
        })
        .unwrap_or(0)
}

/// Runs `inner` to completion, sampling `mem_tracking`'s live byte count
/// for `workflow_id` roughly every [`SAMPLE_INTERVAL_MS`] while it does,
/// and depositing a frozen [`PerformanceReport`] into the recent-report
/// store the moment it finishes — see the module doc comment for why this
/// is a separate layer from `mem_tracking`'s own ephemeral registry.
///
/// `status_of` maps `inner`'s output to a [`PerfStatus`] for the finished
/// report — passed in rather than hardcoded, because this function has no
/// way to inspect a generic `T` for success/failure itself. The one real
/// caller (`executor::run()`) already knows exactly how to answer that for
/// its own `Result<WorkflowResult, EngineError>`.
///
/// Does not require `F: 'static` (unlike a `tokio::spawn`-based sampler
/// would) — the sampling loop runs via `tokio::select!` in this same
/// `async fn`'s own body, borrowing nothing beyond this call's stack, the
/// same technique `mem_tracking::TrackedFuture` already uses for its own
/// per-poll group re-entry (cited in that module).
pub async fn monitor_run<F, T>(
    workflow_id: String,
    inner: F,
    status_of: impl FnOnce(&T) -> PerfStatus,
) -> T
where
    F: Future<Output = T> + Send,
{
    let baseline = total_live_bytes_for(&workflow_id);
    let live = Arc::new(LiveState::new(baseline));
    LIVE.insert(workflow_id.clone(), Arc::clone(&live));

    tokio::pin!(inner);
    let mut ticker = tokio::time::interval(Duration::from_millis(SAMPLE_INTERVAL_MS));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // `Interval::tick()`'s first call resolves immediately (documented
    // tokio behavior) — consumed here so it isn't double-counted against
    // the sample `LiveState::new` already seeded above; the loop below is
    // then genuinely spaced ~SAMPLE_INTERVAL_MS apart from this point on.
    ticker.tick().await;

    let output = loop {
        tokio::select! {
            biased;
            out = &mut inner => break out,
            _ = ticker.tick() => {
                let bytes = total_live_bytes_for(&workflow_id);
                live.record(bytes, now_ms());
            }
        }
    };

    finalize(&workflow_id, &live, status_of(&output));

    // Defensive: only remove the LIVE entry if it's still *this* run's
    // LiveState (guards blast radius if the "≤1 concurrent run per
    // workflow_id" assumption documented above is ever violated elsewhere).
    // Cheap to add; doesn't attempt to fully own a cross-module concurrency
    // guarantee this module doesn't own.
    let should_remove = LIVE.get(&workflow_id).map(|e| Arc::ptr_eq(&e, &live)).unwrap_or(false);
    if should_remove {
        LIVE.remove(&workflow_id);
    }

    output
}

fn finalize(workflow_id: &str, live: &LiveState, status: PerfStatus) {
    let finished_at_ms = now_ms();
    let sample_count = live.sample_count.load(Ordering::Relaxed);
    let sum = live.sum_bytes.load(Ordering::Relaxed);
    let final_bytes = live.current_bytes.load(Ordering::Relaxed);
    let baseline_bytes = live.baseline_bytes;

    let report = PerformanceReport {
        workflow_id: workflow_id.to_string(),
        status,
        started_at_ms: live.started_at_ms,
        finished_at_ms,
        duration_ms: finished_at_ms.saturating_sub(live.started_at_ms),
        sampling_interval_ms: SAMPLE_INTERVAL_MS,
        baseline_bytes,
        final_bytes,
        peak_bytes: live.peak_bytes.load(Ordering::Relaxed),
        peak_at_ms: live.peak_at_ms.load(Ordering::Relaxed),
        minimum_bytes: live.minimum_bytes.load(Ordering::Relaxed),
        // `sample_count` is always >= 1 (`LiveState::new` seeds it), so
        // this division is safe without a zero-guard; `.max(1)` here is
        // belt-and-suspenders against a future refactor removing that seed.
        average_bytes: sum / sample_count.max(1),
        delta_bytes: final_bytes as i64 - baseline_bytes as i64,
        sample_count,
        history: live.history.lock().unwrap_or_else(|e| e.into_inner()).clone(),
    };

    RECENT.insert(workflow_id.to_string(), report);
}

/// Non-destructive read of the given workflow's most recently *finished*
/// report — `None` until at least one run of it has completed since
/// process start (or since [`clear_recent`] was called). Repeated calls
/// return the same report until the *next* run overwrites it — this is
/// what lets a UI keep showing a run's numbers after it ends instead of
/// reverting to blank the instant `mem_tracking` drops the live group.
pub fn get_recent_report(workflow_id: &str) -> Option<PerformanceReport> {
    RECENT.get(workflow_id).map(|r| r.clone())
}

/// Drops the stored recent report for `workflow_id`, if any. Idempotent:
/// calling it with nothing stored is a no-op, not an error.
pub fn clear_recent(workflow_id: &str) {
    RECENT.remove(workflow_id);
}

/// Builds an immutable, point-in-time [`PerformanceReport`] snapshot from a
/// still-running workflow's live counters. Shared by [`get_live_snapshot`]
/// (single workflow) and [`all_live_snapshots`] (all workflows) so the two
/// can never drift out of sync with each other's field-by-field construction.
fn build_live_report(workflow_id: &str, live: &LiveState) -> PerformanceReport {
    let now = now_ms();
    let sample_count = live.sample_count.load(Ordering::Relaxed);
    let sum = live.sum_bytes.load(Ordering::Relaxed);
    let final_bytes = live.current_bytes.load(Ordering::Relaxed);
    PerformanceReport {
        workflow_id: workflow_id.to_string(),
        status: PerfStatus::Running,
        started_at_ms: live.started_at_ms,
        finished_at_ms: now,
        duration_ms: now.saturating_sub(live.started_at_ms),
        sampling_interval_ms: SAMPLE_INTERVAL_MS,
        baseline_bytes: live.baseline_bytes,
        final_bytes,
        peak_bytes: live.peak_bytes.load(Ordering::Relaxed),
        peak_at_ms: live.peak_at_ms.load(Ordering::Relaxed),
        minimum_bytes: live.minimum_bytes.load(Ordering::Relaxed),
        average_bytes: sum / sample_count.max(1),
        delta_bytes: final_bytes as i64 - live.baseline_bytes as i64,
        sample_count,
        history: live.history.lock().unwrap_or_else(|e| e.into_inner()).clone(),
    }
}

/// A point-in-time read of a run currently in progress — `None` if
/// `workflow_id` isn't running right now. Exposed for a future live-panel
/// "watch it happen" view; not currently called anywhere in this crate.
pub fn get_live_snapshot(workflow_id: &str) -> Option<PerformanceReport> {
    let live = LIVE.get(workflow_id)?;
    Some(build_live_report(workflow_id, live.value()))
}

/// A point-in-time read of *every* workflow currently in progress at once —
/// the "all live" counterpart `get_live_snapshot` alone can't provide, for a
/// caller that needs to broadcast every concurrently-running workflow at
/// once rather than a single "currently open" `workflow_id`. Mirrors
/// `mem_tracking::snapshot()`'s own `DashMap::iter()` + `entry.value()`
/// pattern (same construct that module already uses).
pub fn all_live_snapshots() -> Vec<PerformanceReport> {
    LIVE.iter()
        .map(|entry| build_live_report(entry.key(), entry.value()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // NOTE ON TEST ISOLATION: unlike `mem_tracking::tests`'s `TEST_SERIAL`
    // (needed there because `TRACKING_GATE` is one shared, unkeyed
    // counter every test bumps), every test below uses its own globally
    // unique `workflow_id` string as the `LIVE`/`RECENT` DashMap key, so
    // concurrently-running `#[tokio::test]`s in this binary cannot
    // observe each other's entries — no shared-state race to serialize
    // against. `total_live_bytes_for` does read the real, process-global
    // `mem_tracking::snapshot()`, but no test here registers a
    // `TrackedGroup`, so it deterministically reads 0 regardless of what
    // else is running (same "no real #[global_allocator] hook fires in a
    // plain #[test] binary" limitation `mem_tracking::tests`'s own note
    // already documents — inherited, not newly introduced here).

    #[test]
    fn live_state_record_tracks_peak_minimum_average_and_bounds_history() {
        let state = LiveState::new(100);
        state.record(150, 1000); // peak
        state.record(50, 2000);  // minimum
        state.record(120, 3000);

        assert_eq!(state.peak_bytes.load(Ordering::Relaxed), 150);
        assert_eq!(state.peak_at_ms.load(Ordering::Relaxed), 1000);
        assert_eq!(state.minimum_bytes.load(Ordering::Relaxed), 50);
        // 4 samples total: baseline(100) + 150 + 50 + 120 = 420 / 4 = 105.
        assert_eq!(state.sample_count.load(Ordering::Relaxed), 4);
        assert_eq!(state.sum_bytes.load(Ordering::Relaxed), 420);

        // Edge case: push well past MAX_HISTORY_SAMPLES and confirm the
        // buffer is bounded (halved, not grown without limit), while the
        // aggregate counters above (peak/min/sum) — which don't live in
        // the history Vec — stay correct regardless of how many times
        // history itself gets halved.
        for i in 0..(MAX_HISTORY_SAMPLES * 2) {
            state.record(100, 4000 + i as u64);
        }
        let hist_len = state.history.lock().unwrap().len();
        assert!(
            hist_len <= MAX_HISTORY_SAMPLES,
            "history must stay bounded, got {hist_len}"
        );
        assert!(hist_len > 0, "downsampling must never empty the series");
        assert_eq!(state.peak_bytes.load(Ordering::Relaxed), 150, "peak survives history downsampling");
    }

    #[tokio::test]
    async fn monitor_run_freezes_a_report_that_survives_after_completion() {
        let wf = "wf_perfmon_freeze_test".to_string();

        assert!(get_recent_report(&wf).is_none(), "nothing has run yet");

        let output = monitor_run(wf.clone(), async { 42 }, |v: &i32| {
            if *v == 42 { PerfStatus::Success } else { PerfStatus::Failed }
        }).await;
        assert_eq!(output, 42);

        let report = get_recent_report(&wf).expect("report must be stored once the run finishes");
        assert_eq!(report.status, PerfStatus::Success);
        assert!(report.finished_at_ms >= report.started_at_ms);
        assert!(report.sample_count >= 1, "baseline reading alone counts as sample #1");

        let report_again = get_recent_report(&wf).expect("must not vanish on a second read");
        assert_eq!(report_again.started_at_ms, report.started_at_ms);

        // And the live-in-progress view must be gone now that it's done.
        assert!(get_live_snapshot(&wf).is_none(), "run is finished, not in LIVE anymore");
    }

    #[tokio::test]
    async fn monitor_run_maps_failure_via_status_of_and_unknown_workflow_reports_none() {
        let wf = "wf_perfmon_failure_test".to_string();
        let unrelated = "wf_perfmon_never_run".to_string();

        let _ = monitor_run(wf.clone(), async { Err::<(), &str>("boom") }, |r: &Result<(), &str>| {
            match r { Ok(_) => PerfStatus::Success, Err(_) => PerfStatus::Failed }
        }).await;

        let report = get_recent_report(&wf).expect("report must be stored regardless of outcome");
        assert_eq!(report.status, PerfStatus::Failed);

        assert!(get_recent_report(&unrelated).is_none(), "a workflow that never ran has no report");
        assert!(get_live_snapshot(&unrelated).is_none(), "a workflow that never ran isn't live either");
    }

    #[tokio::test]
    async fn monitor_run_samples_periodically_while_the_run_is_still_in_progress() {
        let wf = "wf_perfmon_periodic_test".to_string();
        // Long enough to guarantee at least one real 150ms tick fires
        // beyond the baseline seed, short enough to keep the test fast.
        monitor_run(wf.clone(), async {
            tokio::time::sleep(Duration::from_millis(340)).await;
        }, |_: &()| PerfStatus::Success).await;

        let report = get_recent_report(&wf).expect("report must be stored");
        assert!(
            report.sample_count >= 2,
            "a run spanning >1 tick must sample more than just the baseline, got {}",
            report.sample_count
        );
    }

    // all_live_snapshots must report every concurrently-running workflow at once, 
    // and must not include one that has already finished.
    #[tokio::test]
    async fn all_live_snapshots_reports_every_running_workflow_and_excludes_finished_ones() {
        let wf_a = "wf_perfmon_all_live_a".to_string();
        let wf_b = "wf_perfmon_all_live_b".to_string();
        let wf_done = "wf_perfmon_all_live_done".to_string();

        // wf_done finishes before either A/B starts — must not appear below.
        monitor_run(wf_done.clone(), async {}, |_: &()| PerfStatus::Success).await;

        let (_, _) = tokio::join!(
            monitor_run(wf_a.clone(), async {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }, |_: &()| PerfStatus::Success),
            monitor_run(wf_b.clone(), async {
                // Bounded, not an unbounded loop: if all_live_snapshots()
                // were ever broken (the thing this test exists to catch),
                // an unbounded retry would hang the suite instead of
                // failing. 50 * 10ms = 500ms ceiling, well past the point
                // both entries are expected (both insert into LIVE before
                // either inner future is polled at all).
                let mut snapshots = Vec::new();
                let mut found = false;
                for _ in 0..50 {
                    snapshots = all_live_snapshots();
                    if snapshots.iter().any(|r| r.workflow_id == wf_a)
                        && snapshots.iter().any(|r| r.workflow_id == wf_b) {
                        found = true;
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                assert!(found, "both wf_a and wf_b must appear in an all-live read within 500ms");
                assert!(
                    snapshots.iter().all(|r| r.workflow_id != wf_done),
                    "a finished workflow must not appear in an all-live read"
                );
                assert!(
                    snapshots.iter().all(|r| r.status == PerfStatus::Running),
                    "every entry from an all-live read must report Running"
                );
            }, |_: &()| PerfStatus::Success),
        );

        // Edge case: once both finish, neither remains in a fresh read.
        let after = all_live_snapshots();
        assert!(after.iter().all(|r| r.workflow_id != wf_a && r.workflow_id != wf_b));
    }

    #[tokio::test]
    async fn clear_recent_removes_a_stored_report() {
        let wf = "wf_perfmon_clear_test".to_string();
        monitor_run(wf.clone(), async {}, |_: &()| PerfStatus::Success).await;
        assert!(get_recent_report(&wf).is_some());

        clear_recent(&wf);
        assert!(get_recent_report(&wf).is_none());

        // Idempotent — clearing an already-empty entry is not an error.
        clear_recent(&wf);
    }
}
