use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use crate::plugin_loader::limits::PLUGIN_TRIGGER_YIELD_TICKS;
use crate::plugin_loader::trigger::state::TriggerPluginState;

/// How long a running trigger may execute with no activity before it is
/// stopped. "Activity" is any guest call into a host import (WASI, `wasi:http`,
/// `storage`, `fs-watch`) or an event delivered on the stream. A trigger parked
/// in a host call, or waiting on its stream, executes no guest code and so
/// accrues nothing against this limit.
const PLUGIN_TRIGGER_STALL_LIMIT: Duration = Duration::from_secs(30);

/// Longest interval between two epoch checks still counted as unbroken guest
/// execution. A spinning guest is checked about once a second; a longer gap
/// means the guest was parked in between, which is not time spent running.
const PLUGIN_TRIGGER_STALL_MAX_GAP: Duration = Duration::from_secs(5);

/// Accumulates guest execution time that passed with no activity.
struct StallTracker {
    seen: u64,
    last_check: Instant,
    quiet: Duration,
}

impl StallTracker {
    fn new(activity: u64, now: Instant) -> Self {
        Self { seen: activity, last_check: now, quiet: Duration::ZERO }
    }

    /// Restarts the count from `now`.
    fn rearm(&mut self, activity: u64, now: Instant) {
        *self = Self::new(activity, now);
    }

    /// Records an epoch check at `now` with the current activity count and
    /// returns `true` once [`PLUGIN_TRIGGER_STALL_LIMIT`] of unbroken, quiet
    /// execution has accumulated.
    fn observe(&mut self, activity: u64, now: Instant) -> bool {
        let gap = now.saturating_duration_since(self.last_check);
        self.last_check = now;
        if activity != self.seen {
            self.seen = activity;
            self.quiet = Duration::ZERO;
        } else if gap <= PLUGIN_TRIGGER_STALL_MAX_GAP {
            self.quiet += gap;
        } else {
            self.quiet = Duration::ZERO;
        }
        self.quiet >= PLUGIN_TRIGGER_STALL_LIMIT
    }
}

pub(in crate::plugin_loader) fn stall_message() -> String {
    format!(
        "plugin made no host call and delivered no event for {}s of execution and was stopped",
        PLUGIN_TRIGGER_STALL_LIMIT.as_secs()
    )
}

/// Handles shared between a resolved trigger's stall callback and its pump.
pub(in crate::plugin_loader) struct StallWatch {
    /// Set by the pump once `events()` has returned; until then the callback
    /// only yields, so the `events()` call keeps its own timeout.
    pub(in crate::plugin_loader) armed: Arc<AtomicBool>,
    pub(in crate::plugin_loader) stalled: Arc<AtomicBool>,
    pub(in crate::plugin_loader) signal: Arc<tokio::sync::Notify>,
}

/// Replaces the store's plain yield with a callback that still yields every
/// [`PLUGIN_TRIGGER_YIELD_TICKS`] but also feeds [`StallTracker`], and traps
/// the guest when it reports a stall.
pub(in crate::plugin_loader) fn install_stall_watch(store: &mut wasmtime::Store<TriggerPluginState>) -> StallWatch {
    let activity = Arc::clone(&store.data().activity);
    let armed = Arc::new(AtomicBool::new(false));
    let stalled = Arc::new(AtomicBool::new(false));
    let signal = Arc::new(tokio::sync::Notify::new());

    let mut tracker = StallTracker::new(
        activity.load(std::sync::atomic::Ordering::Relaxed),
        Instant::now(),
    );
    let (cb_armed, cb_stalled, cb_signal) = (Arc::clone(&armed), Arc::clone(&stalled), Arc::clone(&signal));

    store.set_epoch_deadline(PLUGIN_TRIGGER_YIELD_TICKS);
    store.epoch_deadline_callback(move |_cx| {
        let now = Instant::now();
        let count = activity.load(std::sync::atomic::Ordering::Relaxed);
        if !cb_armed.load(std::sync::atomic::Ordering::Acquire) {
            tracker.rearm(count, now);
        } else if tracker.observe(count, now) {
            cb_stalled.store(true, std::sync::atomic::Ordering::Release);
            cb_signal.notify_one();
            return Err(wasmtime::format_err!("{}", stall_message()));
        }
        Ok(wasmtime::UpdateDeadline::Yield(PLUGIN_TRIGGER_YIELD_TICKS))
    });

    StallWatch { armed, stalled, signal }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn stall_tracker_trips_after_limit_of_quiet_execution() {
        let t0 = Instant::now();
        let mut t = StallTracker::new(0, t0);
        for i in 1..=29 {
            assert!(!t.observe(0, t0 + secs(i)));
        }
        assert!(t.observe(0, t0 + secs(30)));
    }

    #[test]
    fn stall_tracker_activity_restarts_the_count() {
        let t0 = Instant::now();
        let mut t = StallTracker::new(0, t0);
        for i in 1..=20 {
            assert!(!t.observe(0, t0 + secs(i)));
        }
        assert!(!t.observe(1, t0 + secs(21)));
        for i in 22..=50 {
            assert!(!t.observe(1, t0 + secs(i)));
        }
        assert!(t.observe(1, t0 + secs(51)));
    }

    #[test]
    fn stall_tracker_ignores_time_spent_parked() {
        let t0 = Instant::now();
        let mut t = StallTracker::new(0, t0);
        assert!(!t.observe(0, t0 + secs(3600)));
        assert!(!t.observe(0, t0 + secs(3601)));
    }

    #[test]
    fn stall_tracker_rearm_discards_accumulated_quiet() {
        let t0 = Instant::now();
        let mut t = StallTracker::new(0, t0);
        for i in 1..=20 {
            t.observe(0, t0 + secs(i));
        }
        t.rearm(0, t0 + secs(21));
        for i in 22..=50 {
            assert!(!t.observe(0, t0 + secs(i)));
        }
    }
}
