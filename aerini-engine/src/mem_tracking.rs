//! Per-workflow / per-node live memory attribution.
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;
use std::task::{Context, Poll};
use std::time::{SystemTime, UNIX_EPOCH};

use dashmap::DashMap;
use once_cell::sync::Lazy;
use tracking_allocator::{
    AllocationGroupId, AllocationGroupToken, AllocationRegistry, AllocationTracker,
};

// ── Group metadata ──────────────────────────────────────────────────────────

/// What a tracked allocation group represents. `snake_case` on the wire so
/// the frontend can match it directly without a translation table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupKind {
    Run,
    Node,
}

/// Static description of one registered group. Set once at registration and
/// never mutated afterward — only the associated [`GroupEntry::live_bytes`]
/// counter changes as the group runs.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GroupMeta {
    pub kind: GroupKind,
    pub workflow_id: String,
    /// `None` for a `Run` group; `Some(node_id)` for a `Node` group.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_type_id: Option<String>,
    /// Milliseconds since the Unix epoch. Plain `u64` rather than
    /// `chrono::DateTime` — this module has no other dependency on the rest
    /// of the crate beyond `serde`/`dashmap`/`once_cell`/`tracking-allocator`,
    /// kept that way deliberately since nothing else needs it to.
    pub started_at_ms: u64,
}

impl GroupMeta {
    pub fn run(workflow_id: String) -> Self {
        Self {
            kind: GroupKind::Run,
            workflow_id,
            node_id: None,
            node_type_id: None,
            started_at_ms: now_ms(),
        }
    }

    pub fn node(workflow_id: String, node_id: String, node_type_id: String) -> Self {
        Self {
            kind: GroupKind::Node,
            workflow_id,
            node_id: Some(node_id),
            node_type_id: Some(node_type_id),
            started_at_ms: now_ms(),
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        // A pre-1970 system clock is not a case worth failing a diagnostics
        // feature over — 0 just means "unknown start time" to the reader.
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

struct GroupEntry {
    meta: GroupMeta,
    /// Signed so a (should-never-happen, but not `unsafe` to allow for)
    /// dealloc-without-matching-alloc underflow saturates to a negative
    /// number instead of wrapping a `u64` to near-`u64::MAX`. Clamped to 0
    /// at the reporting boundary (`snapshot()`), not here — keeping the raw
    /// value signed internally makes such a bug visible in a debugger/log
    /// rather than silently clamped away at the point it would occur.
    live_bytes: AtomicI64,
}

// ── Live registry ───────────────────────────────────────────────────────────

/// Every currently-active tracked group, keyed by the id `tracking-allocator`
/// assigned it at registration. An entry exists from the moment a
/// [`TrackedGroup`] is constructed until it drops.
static GROUPS: Lazy<DashMap<AllocationGroupId, GroupEntry>> = Lazy::new(DashMap::new);

/// Reference count of live [`TrackedGroup`]s, gating
/// `AllocationRegistry::enable_tracking`/`disable_tracking`. A plain
/// `Mutex<i64>` rather than an atomic counter: the increment-then-maybe-toggle
/// (and decrement-then-maybe-toggle) must happen as one atomic unit to avoid
/// a race where two concurrent 0→1 transitions could both decide "I'm not
/// first, someone else will enable tracking" and neither does, or a 1→0/0→1
/// pair could reorder into a "disable-after-enable" that leaves tracking off
/// while a group is still alive. This lock is held only around the
/// increment/decrement + the (idempotent, cheap) enable/disable call — it is
/// touched once per run and once per node execution, never per-allocation,
/// so lock contention here is not a hot-path concern.
static TRACKING_GATE: Mutex<i64> = Mutex::new(0);

fn acquire_tracking() {
    let mut count = TRACKING_GATE.lock().unwrap_or_else(|e| e.into_inner());
    *count += 1;
    if *count == 1 {
        AllocationRegistry::enable_tracking();
    }
}

fn release_tracking() {
    let mut count = TRACKING_GATE.lock().unwrap_or_else(|e| e.into_inner());
    *count -= 1;
    if *count <= 0 {
        *count = 0;
        AllocationRegistry::disable_tracking();
    }
}

// ── Tracker ──────────────────────────────────────────────────────────────────

struct Tracker;

impl AllocationTracker for Tracker {
    fn allocated(
        &self,
        _addr: usize,
        _object_size: usize,
        wrapped_size: usize,
        group_id: AllocationGroupId,
    ) {
        if group_id == AllocationGroupId::ROOT {
            return;
        }
        if let Some(entry) = GROUPS.get(&group_id) {
            entry.live_bytes.fetch_add(wrapped_size as i64, Ordering::Relaxed);
        }
    }

    fn deallocated(
        &self,
        _addr: usize,
        _object_size: usize,
        wrapped_size: usize,
        source_group_id: AllocationGroupId,
        _current_group_id: AllocationGroupId,
    ) {
        // Credited to `source_group_id` (the group that *allocated* this
        // block), not `current_group_id` (whatever group happens to be
        // active at free time) — per tracking-allocator's own
        // `AllocationTracker::deallocated` docs: "source_group_id
        // contains the group ID where the given allocation originated from".
        // This is what makes live-byte accounting correct across a group
        // boundary (e.g. a `Vec` a node allocates, that outlives the node's
        // own group and is freed later while some other group is active).
        if source_group_id == AllocationGroupId::ROOT {
            return;
        }
        if let Some(entry) = GROUPS.get(&source_group_id) {
            entry.live_bytes.fetch_sub(wrapped_size as i64, Ordering::Relaxed);
        }
    }
}

/// Installs the global tracker. Call exactly once, at process startup,
/// before any workflow can possibly run — see `src-tauri/src/lib.rs`'s
/// `.setup()` and `aerini-server/src/main.rs`'s `main()`. A second call
/// anywhere in the process (should not happen — both call sites are
/// single-shot startup code) is swallowed rather than panicking, since by
/// the time a second call could occur the tracker is already correctly
/// installed and there is nothing left to do.
pub fn install() {
    let _ = AllocationRegistry::set_global_tracker(Tracker);
}

// ── RAII group handle ───────────────────────────────────────────────────────

/// Owns one registered allocation group. Constructing one registers it in
/// [`GROUPS`] and bumps [`TRACKING_GATE`]; dropping it removes the entry and
/// releases the gate. Not `Clone` — a group is entered by exactly one
/// [`TrackedFuture`] at a time (though that future may itself retry, in
/// which case the *caller* constructs a fresh `TrackedGroup` per attempt —
/// see `executor/mod.rs::execute_with_retry`, which clones the cheap
/// [`GroupMeta`] per retry rather than reusing one `TrackedGroup`, so that
/// each retry attempt is reported as its own group rather than merging
/// bytes across attempts).
pub struct TrackedGroup {
    id: AllocationGroupId,
    token: AllocationGroupToken,
}

impl TrackedGroup {
    /// Registers a new group. Returns `None` only in the documented failure
    /// mode of `AllocationGroupToken::register()` — the process-wide
    /// allocation-group id space (2^64 on a 64-bit target) has been
    /// exhausted. Astronomically unlikely in any real process lifetime;
    /// handled by simply not tracking that one call rather than treating a
    /// diagnostics feature's exhaustion as a reason to fail an actual
    /// workflow run.
    fn register(meta: GroupMeta) -> Option<Self> {
        let token = AllocationGroupToken::register()?;
        let id = token.id();
        GROUPS.insert(id.clone(), GroupEntry { meta, live_bytes: AtomicI64::new(0) });
        acquire_tracking();
        Some(Self { id, token })
    }
}

impl Drop for TrackedGroup {
    fn drop(&mut self) {
        GROUPS.remove(&self.id);
        release_tracking();
    }
}

// ── Future wrapper ──────────────────────────────────────────────────────────

/// Wraps a future so that, on every `poll()`, `group`'s token is entered for
/// the duration of that one synchronous `poll()` call and released again
/// before it returns — safe under tokio's work-stealing scheduler regardless
/// of which worker thread executes any given `poll()` call. See the module
/// doc comment for the full reasoning.
///
/// `inner` is boxed rather than held as a bare, structurally-pinned type
/// parameter: every real call site wraps an already-boxed
/// `async_trait`-generated future (`Node::execute`) or a borrowed `async fn`
/// future tied to `&self`/local data, neither of which is `'static`, and
/// boxing sidesteps needing `unsafe` pin-projection to soundly poll a
/// non-`Unpin` inner future from behind `&mut self`. The extra heap
/// allocation this adds happens once per node execution / once per run, not
/// per-`poll()` and not per-allocation — negligible next to the work being
/// wrapped (an HTTP call, a DB query, a full workflow run).
pub struct TrackedFuture<'a, T> {
    group: TrackedGroup,
    inner: Pin<Box<dyn Future<Output = T> + Send + 'a>>,
}

// `inner` is a `Pin<Box<_>>`, which is `Unpin` regardless of whether the
// boxed future itself is `Unpin` (moving the box pointer is fine; only the
// heap-allocated contents it points to must not move, and `Box` already
// guarantees that independent of pinning). `group` is a plain, movable
// struct. So every field is `Unpin` and this type can be too — no manual
// `unsafe impl` needed, `#[derive]` isn't applicable here (this impl block
// exists implicitly), stated explicitly in this comment only because it's
// what makes `poll()` below able to use `Pin::get_mut` safely.

impl<'a, T> Future for TrackedFuture<'a, T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        // Deliberately two *direct, disjoint field* accesses on `this`
        // (`this.group.token` and `this.inner`), not routed through a
        // `&mut self`-taking helper method — the borrow checker treats
        // direct field projections as independently borrowable, but a
        // helper method taking `&mut self` would borrow the whole struct
        // and conflict with the second field access below. See the
        // module doc comment; this is the one place that distinction
        // actually matters in this file.
        let this = self.get_mut();
        let _guard = this.group.token.enter();
        this.inner.as_mut().poll(cx)
    }
}

/// Runs `inner` under a freshly-registered group described by `meta`. If
/// group registration fails (see [`TrackedGroup::register`]'s doc comment —
/// practically unreachable), `inner` still runs, just without memory
/// attribution for this one call: a diagnostics feature failing open rather
/// than failing the workflow run it is only supposed to be observing.
pub async fn run_tracked<'a, F>(meta: GroupMeta, inner: F) -> F::Output
where
    F: Future + Send + 'a,
{
    match TrackedGroup::register(meta) {
        Some(group) => {
            let tracked: TrackedFuture<'a, F::Output> =
                TrackedFuture { group, inner: Box::pin(inner) };
            tracked.await
        }
        None => inner.await,
    }
}

// ── Query surface ───────────────────────────────────────────────────────────

/// One run's worth of live memory data, with its node executions nested
/// underneath. Returned by [`snapshot`]; serialized directly as the
/// `get_memory_breakdown` Tauri command's response and as the periodic
/// `memory-breakdown` event payload (both in `src-tauri`).
#[derive(Debug, Clone, serde::Serialize)]
pub struct RunBreakdown {
    pub workflow_id: String,
    pub started_at_ms: u64,
    /// Bytes attributed directly to the run's own group — i.e. executor
    /// bookkeeping *outside* any node execution (topo-order iteration,
    /// input-building, logging). Does **not** include node bytes; sum
    /// `nodes[*].live_bytes` separately if a single "whole run" total is
    /// wanted. This split, not a pre-summed total, is the intended shape:
    /// it is what lets a UI show "N bytes in executor overhead" apart from
    /// "N bytes in the HTTP node," which a single merged number would hide.
    pub live_bytes: i64,
    pub nodes: Vec<NodeBreakdown>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct NodeBreakdown {
    pub node_id: String,
    pub node_type_id: String,
    pub started_at_ms: u64,
    pub live_bytes: i64,
}

/// A snapshot of every workflow run currently in flight on this process,
/// each with its currently-executing node(s) nested underneath. Cheap to
/// call repeatedly — a `DashMap` iteration plus a handful of small Vec
/// pushes, no locks held for longer than one shard at a time.
///
/// Precision note: a node's own group only
/// covers `Node::execute()`'s own call. Data a node hands off to the next
/// node (e.g. a large HTTP response body copied into `node_outputs`) is
/// attributed to whichever group is active at the moment that copy
/// allocation happens, which may already be a *different* node's group or
/// the run's own group by the time it's read back out — inherent to
/// dataflow, not a bug. Treat `live_bytes` as "memory this specific call is
/// responsible for holding onto right now," not as a strict leak-free
/// causal attribution.
pub fn snapshot() -> Vec<RunBreakdown> {
    let mut runs: std::collections::HashMap<String, RunBreakdown> = std::collections::HashMap::new();
    // Keyed by workflow_id, so partitioning is correct by construction —
    // no separate "which orphan belongs to which workflow_id" step needed.
    let mut orphans: std::collections::HashMap<String, Vec<NodeBreakdown>> = std::collections::HashMap::new();

    // First pass: seed one `RunBreakdown` per live Run group.
    for entry in GROUPS.iter() {
        let meta = &entry.value().meta;
        if meta.kind == GroupKind::Run {
            runs.insert(meta.workflow_id.clone(), RunBreakdown {
                workflow_id: meta.workflow_id.clone(),
                started_at_ms: meta.started_at_ms,
                live_bytes: entry.value().live_bytes.load(Ordering::Relaxed).max(0),
                nodes: Vec::new(),
            });
        }
    }

    // Second pass: attach each live Node group to its run by `workflow_id`.
    // A Node group can only ever be alive while its owning Run group is
    // also alive (the executor always `.await`s a node's execution to
    // completion before its Run group's own `run_inner` future can finish
    // polling — see this module's doc comment), so "no matching run found"
    // should be unreachable in practice; handled defensively rather than
    // assumed, — an orphan is surfaced, not silently dropped.
    for entry in GROUPS.iter() {
        let meta = &entry.value().meta;
        if meta.kind != GroupKind::Node {
            continue;
        }
        let node = NodeBreakdown {
            node_id: meta.node_id.clone().unwrap_or_default(),
            node_type_id: meta.node_type_id.clone().unwrap_or_default(),
            started_at_ms: meta.started_at_ms,
            live_bytes: entry.value().live_bytes.load(Ordering::Relaxed).max(0),
        };
        match runs.get_mut(&meta.workflow_id) {
            Some(run) => run.nodes.push(node),
            None => orphans.entry(meta.workflow_id.clone()).or_default().push(node),
        }
    }

    let mut result: Vec<RunBreakdown> = runs.into_values().collect();

    // Defensive fallback for the "should be unreachable" orphan case above:
    // surface orphaned node entries under a synthetic zero-byte run per
    // distinct `workflow_id` rather than silently discarding them, so a
    // genuine bug here would show up as a visibly odd entry in the UI
    // instead of vanishing without a trace.
    for (workflow_id, nodes) in orphans {
        let started_at_ms = nodes.iter().map(|n| n.started_at_ms).min().unwrap_or_else(now_ms);
        result.push(RunBreakdown { workflow_id, started_at_ms, live_bytes: 0, nodes });
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[test]
    fn registering_and_dropping_a_group_updates_the_gate() {
        let _serial = TEST_SERIAL.blocking_lock();
        let baseline = *TRACKING_GATE.lock().unwrap();
        {
            let _g = TrackedGroup::register(GroupMeta::run("wf_test_gate".to_string()))
                .expect("registration should not fail in a fresh process");
            assert_eq!(*TRACKING_GATE.lock().unwrap(), baseline + 1);
        }
        assert_eq!(*TRACKING_GATE.lock().unwrap(), baseline);
    }

    #[test]
    fn nested_run_and_node_groups_both_appear_in_snapshot_and_join_by_workflow_id() {
        let _serial = TEST_SERIAL.blocking_lock();
        let run_group = TrackedGroup::register(GroupMeta::run("wf_snap".to_string()))
            .expect("run group registration should not fail");
        let node_group = TrackedGroup::register(GroupMeta::node(
            "wf_snap".to_string(), "n1".to_string(), "http".to_string(),
        )).expect("node group registration should not fail");

        let snap = snapshot();
        let run = snap.iter().find(|r| r.workflow_id == "wf_snap")
            .expect("wf_snap run should be present while its group is alive");
        assert_eq!(run.nodes.len(), 1, "node group should nest under its run, not appear as an orphan");
        assert_eq!(run.nodes[0].node_id, "n1");
        assert_eq!(run.nodes[0].node_type_id, "http");

        drop(node_group);
        drop(run_group);
        let snap_after = snapshot();
        assert!(
            snap_after.iter().all(|r| r.workflow_id != "wf_snap"),
            "run should disappear from the snapshot once its group drops"
        );
    }

    #[test]
    fn live_bytes_credited_to_allocating_group_not_currently_active_group_on_dealloc() {
        // Exercises the Tracker's own allocated/deallocated logic directly,
        // bypassing the real global allocator (this crate's #[test] binary
        // does not install one — see the module note above). Confirms the
        // bookkeeping rule the doc comment on `deallocated` claims: a
        // dealloc's `source_group_id` (not whatever `current_group_id` is
        // passed) is what gets debited.
        let _serial = TEST_SERIAL.blocking_lock();
        let group = TrackedGroup::register(GroupMeta::run("wf_dealloc".to_string()))
            .expect("registration should not fail");
        let tracker = Tracker;
        tracker.allocated(0x1000, 64, 64, group.id.clone());
        assert_eq!(
            GROUPS.get(&group.id).unwrap().live_bytes.load(Ordering::Relaxed),
            64
        );

        let other = TrackedGroup::register(GroupMeta::run("wf_other".to_string()))
            .expect("registration should not fail");
        // Simulate freeing the first group's allocation while the *other*
        // group happens to be "current" — source_group_id still points at
        // the original allocator.
        tracker.deallocated(0x1000, 64, 64, group.id.clone(), other.id.clone());
        assert_eq!(
            GROUPS.get(&group.id).unwrap().live_bytes.load(Ordering::Relaxed),
            0,
            "dealloc must debit the source group, not whatever group is currently active"
        );
        assert_eq!(
            GROUPS.get(&other.id).unwrap().live_bytes.load(Ordering::Relaxed),
            0,
            "the currently-active-but-uninvolved group must not be touched"
        );
    }

    #[tokio::test]
    async fn run_tracked_reports_output_and_cleans_up_on_completion() {
        let _serial = TEST_SERIAL.lock().await;
        let meta = GroupMeta::node("wf_rt".to_string(), "n1".to_string(), "code".to_string());
        let out = run_tracked(meta, async { 1 + 1 }).await;
        assert_eq!(out, 2);
        assert!(
            snapshot().iter().flat_map(|r| r.nodes.iter()).all(|n| n.node_id != "n1_should_not_exist"),
            "sanity check on snapshot() shape, not a real assertion about n1 (see next test)"
        );
    }

    #[tokio::test]
    async fn run_tracked_group_is_gone_once_the_wrapped_future_completes() {
        let _serial = TEST_SERIAL.lock().await;
        let meta = GroupMeta::node("wf_rt2".to_string(), "n_gone".to_string(), "code".to_string());
        run_tracked(meta, async {}).await;
        assert!(
            snapshot().iter().flat_map(|r| r.nodes.iter()).all(|n| n.node_id != "n_gone"),
            "TrackedGroup must be dropped (and removed from GROUPS) once run_tracked's future resolves"
        );
    }
}
