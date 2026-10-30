use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use chrono::{DateTime, Utc};
use wasmtime::Engine;
use wasmtime::component::ResourceTable;
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpCtxView, WasiHttpView};

use crate::plugin_loader::bindings::{wit_fs_watch, wit_storage, wit_trigger_next_fire};
use crate::plugin_loader::fs_watch::PluginFsWatchHandle;
use crate::plugin_loader::http_hooks::PluginHttpHooks;
use crate::plugin_loader::limits::{PLUGIN_TRIGGER_YIELD_TICKS, PluginLimits};
use crate::plugin_loader::storage::PluginStorageHandle;

/// Per-instance store state for a trigger-plugin component, run on
/// [`PluginLoader::trigger_engine`].
///
/// HTTP is linked via `wasi:http` p3 ([`PluginHttpHooks`]), SSRF-filtered and
/// address-pinned identically to [`PluginState`]'s p2 HTTP -- see "Outbound HTTP" in this
/// module's own doc comment. Raw `wasi:sockets` capability is left at
/// `WasiCtxBuilder`'s own default (no `allow_tcp`/`socket_addr_check`
/// override), which denies every address absent an explicit grant -- the
/// same effective position [`make_plugin_state`] leaves action plugins in
/// for raw sockets. The `storage` import is linked identically to
/// [`PluginState`]'s -- see `storage` field below.
///
/// One `TriggerPluginState` (and one `Store`) per running trigger instance,
/// held for that instance's whole lifetime -- unlike [`PluginState`], which
/// is recreated per `execute()` call, a trigger instance's `events()` stream
/// must stay backed by the same store for as long as it's being drained.
pub(in crate::plugin_loader) struct TriggerPluginState {
    wasi_ctx: WasiCtx,
    http_ctx: WasiHttpCtx,
    table: ResourceTable,
    limits: PluginLimits,
    http_hooks: PluginHttpHooks,
    /// This instance's `storage` import backing, if any -- same shape and
    /// same "scoped by the resolved `type_id`, `None` if the directory's
    /// storage database couldn't be opened" contract as [`PluginState::storage`].
    storage: Option<PluginStorageHandle>,
    /// This instance's `fs-watch` import backing -- same shape and scoping
    /// as [`PluginState::fs_watch`]. Always `Some` in practice (unlike
    /// `storage`, opening [`FsWatchRegistry`] cannot fail), kept `Option`
    /// only for symmetry with `storage`'s degrade-gracefully contract.
    fs_watch: Option<PluginFsWatchHandle>,
    /// Count of guest-initiated host calls (WASI, `wasi:http`, `storage`,
    /// `fs-watch`) plus delivered trigger events, read by the stall watchdog.
    pub(in crate::plugin_loader) activity: Arc<AtomicU64>,
    /// Where `trigger-schedule.report-next-fire` publishes the validated time.
    /// `None` until [`PluginLoader::start_trigger`] attaches one, so a call made
    /// during `describe` or `execute` has nowhere to report to.
    pub(in crate::plugin_loader) next_fire: Option<tokio::sync::watch::Sender<Option<DateTime<Utc>>>>,
}

impl TriggerPluginState {
    fn touch(&self) {
        self.activity.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

impl WasiView for TriggerPluginState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        self.touch();
        WasiCtxView { ctx: &mut self.wasi_ctx, table: &mut self.table }
    }
}

impl WasiHttpView for TriggerPluginState {
    fn http(&mut self) -> WasiHttpCtxView<'_> {
        self.touch();
        WasiHttpCtxView {
            ctx: &mut self.http_ctx,
            table: &mut self.table,
            hooks: &mut self.http_hooks,
        }
    }
}

impl wit_storage::StorageHost for TriggerPluginState {
    fn get(&mut self, key: String) -> Option<String> {
        self.touch();
        let handle = self.storage.as_ref()?;
        handle.db.get(&handle.scope, &key)
    }

    fn set(&mut self, key: String, value: String) -> Result<(), wit_storage::StorageError> {
        self.touch();
        let handle = self.storage.as_ref().ok_or(wit_storage::StorageError::Unavailable)?;
        handle.db.set(&handle.scope, &key, &value)
    }

    fn delete(&mut self, key: String) {
        self.touch();
        if let Some(handle) = self.storage.as_ref() {
            handle.db.delete(&handle.scope, &key);
        }
    }

    fn list_keys(&mut self, prefix: String) -> Result<Vec<String>, wit_storage::StorageError> {
        self.touch();
        let handle = self.storage.as_ref().ok_or(wit_storage::StorageError::Unavailable)?;
        handle.db.list_keys(&handle.scope, &prefix)
    }
}

impl wit_fs_watch::FsWatchHost for TriggerPluginState {
    fn poll(&mut self, target: wit_fs_watch::WatchTarget) -> Result<wit_fs_watch::PollResult, wit_fs_watch::WatchError> {
        self.touch();
        let handle = self.fs_watch.as_ref().ok_or(wit_fs_watch::WatchError::Unavailable)?;
        handle.poll(&target)
    }

    fn unwatch(&mut self, target: wit_fs_watch::WatchTarget) {
        self.touch();
        if let Some(handle) = self.fs_watch.as_ref() {
            handle.registry.unwatch(&handle.plugin_type_id, &target);
        }
    }
}

/// A reported next-fire time may be this far in the past and still be accepted
/// (it is then treated as "now"), to absorb clock reads and scheduling delay
/// between the guest computing the time and the host receiving it.
const NEXT_FIRE_PAST_SLACK_MS: u64 = 5_000;

/// Furthest ahead a reported next-fire time is shown; later times are shortened
/// to this horizon rather than rejected.
const NEXT_FIRE_MAX_HORIZON_MS: u64 = 30 * 24 * 60 * 60 * 1000;

/// Validates a guest-reported next-fire time against the host clock.
fn validate_next_fire(
    reported_ms: u64,
    now_ms: u64,
) -> Result<DateTime<Utc>, wit_trigger_next_fire::ScheduleError> {
    if reported_ms.saturating_add(NEXT_FIRE_PAST_SLACK_MS) < now_ms {
        return Err(wit_trigger_next_fire::ScheduleError::InPast);
    }
    let shown_ms = reported_ms.clamp(now_ms, now_ms.saturating_add(NEXT_FIRE_MAX_HORIZON_MS));
    i64::try_from(shown_ms)
        .ok()
        .and_then(DateTime::from_timestamp_millis)
        .ok_or(wit_trigger_next_fire::ScheduleError::Unavailable)
}

impl wit_trigger_next_fire::TriggerScheduleHost for TriggerPluginState {
    fn report_next_fire(&mut self, unix_millis: u64) -> Result<(), wit_trigger_next_fire::ScheduleError> {
        self.touch();
        let sink = self.next_fire.as_ref().ok_or(wit_trigger_next_fire::ScheduleError::Unavailable)?;
        let now_ms = u64::try_from(Utc::now().timestamp_millis()).unwrap_or(0);
        let at = validate_next_fire(unix_millis, now_ms)?;
        sink.send_if_modified(|current| {
            if *current == Some(at) {
                false
            } else {
                *current = Some(at);
                true
            }
        });
        Ok(())
    }
}

pub(in crate::plugin_loader) fn make_trigger_plugin_state(
    storage: Option<PluginStorageHandle>,
    fs_watch: Option<PluginFsWatchHandle>,
) -> TriggerPluginState {
    TriggerPluginState {
        wasi_ctx: WasiCtx::builder().build(),
        http_ctx: WasiHttpCtx::new(),
        table: ResourceTable::new(),
        limits: PluginLimits::default(),
        http_hooks: PluginHttpHooks,
        storage,
        fs_watch,
        activity: Arc::new(AtomicU64::new(0)),
        next_fire: None,
    }
}

/// Builds a trigger-engine store. Guest code yields to the async runtime every
/// [`PLUGIN_TRIGGER_YIELD_TICKS`] instead of trapping, so a guest that spins
/// without returning cannot hold a runtime thread; callers bound the wall-clock
/// time with [`PLUGIN_TRIGGER_CALL_TIMEOUT`], which cancels the call at one of
/// those yield points. (`trigger_engine` has epoch interruption enabled, so a
/// store without a deadline would trap at once.) A trigger instance that stays
/// alive and idle between events indefinitely keeps this yield cadence once
/// resolved; [`PluginLoader::start_trigger`] swaps the plain yield for the
/// stall watchdog ([`install_stall_watch`]), and stopping the stream is the
/// owning job task's abort (`scheduler/runner.rs`).
pub(in crate::plugin_loader) fn make_trigger_store(
    engine: &Engine,
    state: TriggerPluginState,
) -> wasmtime::Store<TriggerPluginState> {
    let mut store = wasmtime::Store::new(engine, state);
    store.limiter(|s| &mut s.limits);
    store.set_epoch_deadline(PLUGIN_TRIGGER_YIELD_TICKS);
    store.epoch_deadline_async_yield_and_update(PLUGIN_TRIGGER_YIELD_TICKS);
    store
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_loader::storage::PluginStorage;

    /// Normal case: `TriggerPluginState`'s `StorageHost` delegates to the
    /// backing `PluginStorage` exactly like `PluginState`'s does — a value
    /// set through the handle is visible through a `get` on the same
    /// instance.
    #[test]
    fn trigger_plugin_state_storage_round_trips_through_handle() {
        use wit_storage::StorageHost as _;
        let dir = tempfile::tempdir().expect("tempdir create failed");
        let db = Arc::new(PluginStorage::open(&dir.keep()).expect("PluginStorage::open failed"));
        let mut state = make_trigger_plugin_state(
            Some(PluginStorageHandle { db, scope: "trigger-plugin-a".to_string() }),
            None,
        );

        assert_eq!(state.get("k1".to_string()), None);
        state.set("k1".to_string(), "v1".to_string()).expect("set failed");
        assert_eq!(state.get("k1".to_string()), Some("v1".to_string()));
    }

    /// Edge case: a `None` storage handle (directory's storage DB
    /// unavailable) must degrade exactly per `wit/node.wit`'s documented
    /// contract — `get` empty, `set`/`list_keys` return `Unavailable`,
    /// `delete` a silent no-op — never panic.
    #[test]
    fn trigger_plugin_state_storage_degrades_when_handle_is_none() {
        use wit_storage::StorageHost as _;
        let mut state = make_trigger_plugin_state(None, None);

        assert_eq!(state.get("k1".to_string()), None);
        assert!(matches!(
            state.set("k1".to_string(), "v1".to_string()),
            Err(wit_storage::StorageError::Unavailable)
        ));
        assert!(matches!(
            state.list_keys("".to_string()),
            Err(wit_storage::StorageError::Unavailable)
        ));
        state.delete("k1".to_string()); // must not panic
    }

    const NOW_MS: u64 = 1_800_000_000_000;

    #[test]
    fn next_fire_future_time_within_horizon_is_kept() {
        let at = validate_next_fire(NOW_MS + 60_000, NOW_MS).expect("accepted");
        assert_eq!(at.timestamp_millis(), (NOW_MS + 60_000) as i64);
    }

    #[test]
    fn next_fire_slightly_past_is_treated_as_now() {
        let at = validate_next_fire(NOW_MS - NEXT_FIRE_PAST_SLACK_MS, NOW_MS).expect("accepted");
        assert_eq!(at.timestamp_millis(), NOW_MS as i64);
    }

    #[test]
    fn next_fire_past_beyond_slack_is_rejected() {
        let err = validate_next_fire(NOW_MS - NEXT_FIRE_PAST_SLACK_MS - 1, NOW_MS).unwrap_err();
        assert!(matches!(err, wit_trigger_next_fire::ScheduleError::InPast));
    }

    #[test]
    fn next_fire_beyond_horizon_is_capped_not_rejected() {
        let at = validate_next_fire(u64::MAX, NOW_MS).expect("accepted");
        assert_eq!(at.timestamp_millis(), (NOW_MS + NEXT_FIRE_MAX_HORIZON_MS) as i64);
    }

    #[test]
    fn report_next_fire_without_a_sink_is_unavailable() {
        use wit_trigger_next_fire::TriggerScheduleHost as _;
        let mut state = make_trigger_plugin_state(None, None);
        let err = state.report_next_fire(u64::MAX).unwrap_err();
        assert!(matches!(err, wit_trigger_next_fire::ScheduleError::Unavailable));
    }

    #[test]
    fn report_next_fire_publishes_latest_and_rejects_past_without_overwriting() {
        use wit_trigger_next_fire::TriggerScheduleHost as _;
        let (tx, rx) = tokio::sync::watch::channel(None);
        let mut state = make_trigger_plugin_state(None, None);
        state.next_fire = Some(tx);

        let soon = (Utc::now().timestamp_millis() + 60_000) as u64;
        state.report_next_fire(soon).expect("accepted");
        let first = *rx.borrow();
        assert_eq!(first.map(|t| t.timestamp_millis()), Some(soon as i64));

        let err = state.report_next_fire(1).unwrap_err();
        assert!(matches!(err, wit_trigger_next_fire::ScheduleError::InPast));
        assert_eq!(*rx.borrow(), first);
    }
}
