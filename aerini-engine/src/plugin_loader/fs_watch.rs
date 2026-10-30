use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use notify::{Event as NotifyEvent, EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};
use notify::event::{ModifyKind, RenameMode};
use serde_json::Value;

use crate::plugin_loader::bindings::wit_fs_watch;

/// An [`FsWatchRegistry`] handle plus the plugin type-id this particular
/// `Store` polls/unwatches under -- same shape and same scoping rationale
/// as [`PluginStorageHandle`] immediately above.
#[derive(Clone)]
pub(super) struct PluginFsWatchHandle {
    pub(super) registry: Arc<FsWatchRegistry>,
    pub(super) plugin_type_id: String,
    pub(super) scope: FsWatchScope,
}

/// Which paths a plugin may watch: anywhere, only inside the server's file
/// sandbox directory, or nowhere when that directory is configured but unusable.
#[derive(Clone)]
pub(super) enum FsWatchScope {
    Unrestricted,
    Within(PathBuf),
    Denied,
}

impl FsWatchScope {
    pub(super) fn from_metadata(metadata: &HashMap<String, Value>) -> Self {
        match crate::nodes::fs_sandbox::root_from_metadata(metadata) {
            Ok(None) => Self::Unrestricted,
            Ok(Some(root)) => Self::Within(root),
            Err(_) => Self::Denied,
        }
    }

    pub(super) fn from_root(root: Option<&Path>) -> Self {
        match root {
            None => Self::Unrestricted,
            Some(root) => std::fs::canonicalize(root).map_or(Self::Denied, Self::Within),
        }
    }

    fn permits(&self, path: &str) -> bool {
        match self {
            Self::Unrestricted => true,
            Self::Denied => false,
            Self::Within(root) => std::fs::canonicalize(path).is_ok_and(|resolved| resolved.starts_with(root)),
        }
    }
}

impl PluginFsWatchHandle {
    pub(super) fn poll(&self, target: &wit_fs_watch::WatchTarget) -> Result<wit_fs_watch::PollResult, wit_fs_watch::WatchError> {
        if !self.scope.permits(&target.path) {
            return Err(wit_fs_watch::WatchError::PermissionDenied);
        }
        self.registry.poll(&self.plugin_type_id, target)
    }
}

/// Max distinct watch targets one plugin type-id may have active at once.
/// Bounds OS watch-descriptor usage the same way `STORAGE_MAX_KEYS_PER_SCOPE`
/// bounds storage row growth — generous for what this interface is for (a
/// handful of configured watchers per plugin), not a namespace for
/// arbitrarily many.
const FS_WATCH_MAX_TARGETS_PER_PLUGIN: usize = 25;

/// Max buffered events per target between `poll` calls. Once full, the
/// oldest event is dropped to make room for the newest and `overflowed` is
/// reported on the next `poll` — see `interface fs-watch`'s own doc comment
/// on `poll-result.overflowed` for why this is surfaced rather than silent.
const FS_WATCH_BUFFER_CAP: usize = 500;

/// A target not polled for this long has its underlying `notify` watcher
/// stopped and its buffer discarded, freeing the OS watch descriptor — swept
/// lazily at the start of every `poll` call (see [`FsWatchRegistry::poll`]),
/// not by a dedicated background thread.
const FS_WATCH_IDLE_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// How long an unmatched `RenameMode::From` notification is held, waiting
/// for a matching `To`, before being reported as a plain `deleted` (and a
/// later, unmatched `To` as a plain `created`) instead of a `renamed` pair.
/// A real rename is a single atomic OS operation — the two notification
/// halves arrive within microseconds of each other in practice — so this
/// window is generous, not a tight race. Exact rename-event pairing
/// behavior (`Both` vs. split `From`/`To`) varies by platform and backend;
/// confirmed only from `notify`'s own issue tracker and example code, not
/// from running a watcher on a live filesystem on all three target
/// platforms.
const FS_WATCH_RENAME_CORRELATION_WINDOW: Duration = Duration::from_millis(500);

/// How close together two notifications for the same `(path, event-type)`
/// must be to be treated as one logical change and coalesced into a single
/// buffered event, refreshed in place, rather than appended as a second one
/// — the File Watcher Plugin spec's own reliability philosophy: "one
/// meaningful filesystem change should result in one meaningful automation
/// event." Many editors and copy tools fire several raw writes for what a
/// user experiences as one save.
const FS_WATCH_COALESCE_WINDOW: Duration = Duration::from_millis(300);

/// Host-side backing for the WIT `fs-watch` import. One real `notify`
/// watcher runs per distinct [`WatchKey`], for the process's lifetime,
/// independent of any single `execute()` call's ephemeral, sandboxed
/// `Store` — the same "outlives the Store that calls into it" shape
/// [`PluginStorage`] already has for `storage`, except purely in-memory:
/// losing buffered-but-unpolled events across a process restart is expected
/// and acceptable (a poll afterward picks up watching from that point
/// forward), the same as any at-least-once, best-effort filesystem
/// notification system.
pub(super) struct FsWatchRegistry {
    targets: Mutex<HashMap<WatchKey, WatchState>>,
}

/// Identifies one active watch. Two `poll` calls (from any execution of the
/// same plugin type-id) with an equivalent `watch-target` share one
/// `WatchState` and its buffer — see `interface fs-watch`'s own doc comment
/// on `poll`.
#[derive(Clone, PartialEq, Eq, Hash)]
struct WatchKey {
    plugin_type_id: String,
    path: PathBuf,
    recursive: bool,
    /// Sorted and deduplicated so `["created","modified"]` and
    /// `["modified","created"]` hash and compare equal — "the same target"
    /// as a human reading two configs would expect.
    events: Vec<String>,
}

/// Live state for one [`WatchKey`]: the real `notify` watcher, kept alive
/// only for its `Drop` impl (stops the OS-level watch when this entry is
/// removed) — its callback closure holds the actual event-processing logic
/// and its own clone of `buffer` — plus the buffer that closure feeds and
/// `poll` drains.
struct WatchState {
    _watcher: RecommendedWatcher,
    buffer: Arc<Mutex<TargetBuffer>>,
    last_polled: Instant,
}

/// The mutable, shared-with-the-watcher-callback part of one target's
/// state — split out from [`WatchState`] so the `notify` callback closure
/// (running on `notify`'s own OS thread) only ever locks this one target's
/// small buffer, never [`FsWatchRegistry::targets`]'s registry-wide map
/// lock, and never needs to re-derive its own `WatchKey` from a raw event.
struct TargetBuffer {
    events: VecDeque<wit_fs_watch::FsEvent>,
    overflowed: bool,
    pending_rename_from: Option<(PathBuf, Instant)>,
    /// `(path, event-type, when)` last pushed — coalescing input, see
    /// `FS_WATCH_COALESCE_WINDOW`.
    last_pushed: Option<(String, String, Instant)>,
    /// This target's own `watch-target.events` filter, copied in at watch
    /// start so the callback can drop unwanted kinds without touching
    /// `WatchKey` (which lives in the outer map, not reachable from here).
    events_wanted: std::collections::HashSet<String>,
}

impl FsWatchRegistry {
    pub(super) fn new() -> Self {
        Self { targets: Mutex::new(HashMap::new()) }
    }

    /// See `interface fs-watch`'s own doc comment on `poll` for the
    /// contract. Validates `target`, canonicalizes its path, starts a real
    /// `notify` watcher on first use for this exact target (subject to
    /// `FS_WATCH_MAX_TARGETS_PER_PLUGIN`), and returns/drains whatever this
    /// target's buffer has accumulated since the last call.
    pub(super) fn poll(
        &self,
        plugin_type_id: &str,
        target: &wit_fs_watch::WatchTarget,
    ) -> Result<wit_fs_watch::PollResult, wit_fs_watch::WatchError> {
        let mut events = target.events.clone();
        events.sort();
        events.dedup();
        if events.is_empty()
            || events.iter().any(|e| !matches!(e.as_str(), "created" | "modified" | "deleted" | "renamed"))
        {
            return Err(wit_fs_watch::WatchError::InvalidConfig);
        }

        let raw_path = Path::new(&target.path);
        if !raw_path.is_absolute() {
            return Err(wit_fs_watch::WatchError::InvalidConfig);
        }
        let canonical = std::fs::canonicalize(raw_path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => wit_fs_watch::WatchError::PathNotFound,
            std::io::ErrorKind::PermissionDenied => wit_fs_watch::WatchError::PermissionDenied,
            _ => wit_fs_watch::WatchError::Unavailable,
        })?;

        let key = WatchKey {
            plugin_type_id: plugin_type_id.to_string(),
            path: canonical.clone(),
            recursive: target.recursive,
            events: events.clone(),
        };

        // Sweep idle targets (any plugin, not just this call's) before
        // deciding whether this poll needs to start a new watch — keeps a
        // long-idle target from counting against this plugin's own quota.
        self.reap_idle();

        let mut targets = self.targets.lock().unwrap_or_else(std::sync::PoisonError::into_inner);

        if !targets.contains_key(&key) {
            let active_for_plugin =
                targets.keys().filter(|k| k.plugin_type_id == plugin_type_id).count();
            if active_for_plugin >= FS_WATCH_MAX_TARGETS_PER_PLUGIN {
                return Err(wit_fs_watch::WatchError::QuotaExceeded);
            }

            let buffer = Arc::new(Mutex::new(TargetBuffer {
                events: VecDeque::new(),
                overflowed: false,
                pending_rename_from: None,
                last_pushed: None,
                events_wanted: events.iter().cloned().collect(),
            }));
            let callback_buffer = buffer.clone();

            let mut watcher = notify::recommended_watcher(move |res: notify::Result<NotifyEvent>| {
                if let Ok(event) = res {
                    handle_raw_event(&callback_buffer, event);
                }
                // A raw `Err` here is a watcher-level failure (backend I/O
                // error, not an OS event-queue overflow -- `notify` reports
                // overflow as `Ok(Event)` with `need_rescan()` true, handled
                // in `handle_raw_event` instead) rather than an event to
                // report through `fs-event`'s own vocabulary — nothing in
                // `interface fs-watch` represents it, and the watcher keeps
                // running afterward regardless, so it is silently dropped
                // rather than surfaced as a phantom event.
            })
            .map_err(|_| wit_fs_watch::WatchError::Unavailable)?;

            let mode = if target.recursive { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive };
            watcher.watch(&canonical, mode).map_err(|_| wit_fs_watch::WatchError::Unavailable)?;

            targets.insert(key.clone(), WatchState { _watcher: watcher, buffer, last_polled: Instant::now() });
        }

        let state = targets.get_mut(&key).expect("just inserted above, or already present");
        state.last_polled = Instant::now();
        let buffer = state.buffer.clone();
        // Dropped before locking `buffer` below: nothing else needs
        // `targets` held while draining one target's own, separately
        // locked buffer.
        drop(targets);

        let mut buf = buffer.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let drained: Vec<wit_fs_watch::FsEvent> = buf.events.drain(..).collect();
        let overflowed = buf.overflowed;
        buf.overflowed = false;
        Ok(wit_fs_watch::PollResult { events: drained, overflowed })
    }

    /// See `interface fs-watch`'s own doc comment on `unwatch`. A path that
    /// no longer exists can't be canonicalized to reconstruct its original
    /// `WatchKey`, so this is a best-effort match, not a guarantee — a
    /// target whose path was deleted out from under it is cleaned up by
    /// `FS_WATCH_IDLE_TIMEOUT` instead, once nothing polls it anymore.
    pub(super) fn unwatch(&self, plugin_type_id: &str, target: &wit_fs_watch::WatchTarget) {
        let mut events = target.events.clone();
        events.sort();
        events.dedup();
        let Ok(canonical) = std::fs::canonicalize(&target.path) else { return };
        let key = WatchKey {
            plugin_type_id: plugin_type_id.to_string(),
            path: canonical,
            recursive: target.recursive,
            events,
        };
        self.targets.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&key);
    }

    fn reap_idle(&self) {
        self.targets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|_, state| state.last_polled.elapsed() < FS_WATCH_IDLE_TIMEOUT);
    }
}

/// Converts one raw `notify` event into zero or more buffered
/// [`wit_fs_watch::FsEvent`]s, applying rename correlation before anything
/// is pushed (`push_event` applies coalescing). Runs on `notify`'s own
/// callback thread, one call per raw OS notification — kept fast and
/// scoped to this one target's [`TargetBuffer`], never the whole registry.
fn handle_raw_event(buffer: &Arc<Mutex<TargetBuffer>>, event: NotifyEvent) {
    let mut buf = buffer.lock().unwrap_or_else(std::sync::PoisonError::into_inner);

    // `need_rescan` is `notify`'s own cross-backend signal that events may
    // have been missed (e.g. Linux inotify's queue overflow arrives as this
    // flag on an `Other`-kind event, not as an `Err`) -- feeds the same
    // honest `overflowed` flag `push_event`'s buffer-cap path sets, since
    // both mean the same thing to a plugin: some events since the last poll
    // are not fully represented.
    if event.need_rescan() {
        buf.overflowed = true;
    }

    // A stale pending `From` (its matching `To` never arrived within the
    // correlation window) resolves to a plain `deleted` the next time this
    // callback runs at all, not on a timer — a real rename's `To` follows
    // within microseconds, so anything reaching this check is already well
    // past that.
    if let Some((from, at)) = &buf.pending_rename_from {
        if at.elapsed() > FS_WATCH_RENAME_CORRELATION_WINDOW {
            let from = from.clone();
            buf.pending_rename_from = None;
            push_event(&mut buf, "deleted", &from, None);
        }
    }

    match event.kind {
        EventKind::Create(_) => {
            if let Some(path) = event.paths.first() {
                push_event(&mut buf, "created", path, None);
            }
        }
        EventKind::Remove(_) => {
            if let Some(path) = event.paths.first() {
                push_event(&mut buf, "deleted", path, None);
            }
        }
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {
            if let (Some(from), Some(to)) = (event.paths.first(), event.paths.get(1)) {
                push_event(&mut buf, "renamed", to, Some(from.clone()));
            }
        }
        EventKind::Modify(ModifyKind::Name(RenameMode::From)) => {
            if let Some(path) = event.paths.first() {
                buf.pending_rename_from = Some((path.clone(), Instant::now()));
            }
        }
        EventKind::Modify(ModifyKind::Name(RenameMode::To)) => {
            if let Some(to) = event.paths.first() {
                if let Some((from, _)) = buf.pending_rename_from.take() {
                    push_event(&mut buf, "renamed", to, Some(from));
                } else {
                    push_event(&mut buf, "created", to, None);
                }
            }
        }
        // `Any`/`Other` carry one path with no indication of which side of
        // a rename it is (the catch-all a backend uses when it can't -- or,
        // on some `notify` versions' macOS fsevent path, doesn't -- stitch
        // From/To itself). Existence on disk is the only signal available:
        // gone means this was the origin (same as `From`), present means
        // it's the destination (same as `To`), so the existing correlation
        // logic above applies unchanged either way.
        EventKind::Modify(ModifyKind::Name(RenameMode::Any | RenameMode::Other)) => {
            if let Some(path) = event.paths.first() {
                if path.exists() {
                    if let Some((from, _)) = buf.pending_rename_from.take() {
                        push_event(&mut buf, "renamed", path, Some(from));
                    } else {
                        push_event(&mut buf, "created", path, None);
                    }
                } else {
                    buf.pending_rename_from = Some((path.clone(), Instant::now()));
                }
            }
        }
        // Some backends (notably Windows') report plain content writes as
        // `ModifyKind::Any` rather than `Data` — high-confidence from
        // `notify`'s own documented event taxonomy, not independently
        // verified against a live Windows watcher (no toolchain able to run
        // one here; confirm with a real create/modify/delete/rename smoke
        // test on Linux, macOS, and Windows before shipping).
        EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Any | ModifyKind::Other) => {
            if let Some(path) = event.paths.first() {
                push_event(&mut buf, "modified", path, None);
            }
        }
        EventKind::Modify(ModifyKind::Metadata(_))
        | EventKind::Access(_)
        | EventKind::Other
        | EventKind::Any => {
            // Not a value `fs-event.event-type`'s vocabulary covers.
        }
    }
}

/// Pushes one logical event into `buf` if `event_type` is one this target's
/// `events_wanted` asked for, coalescing it into the existing back-of-buffer
/// entry when the same `(path, event_type)` was just pushed within
/// `FS_WATCH_COALESCE_WINDOW`, and enforcing `FS_WATCH_BUFFER_CAP` (oldest
/// dropped, `overflowed` set) otherwise.
fn push_event(buf: &mut TargetBuffer, event_type: &str, path: &Path, previous_path: Option<PathBuf>) {
    if !buf.events_wanted.contains(event_type) {
        return;
    }

    let path_str = path.to_string_lossy().to_string();
    let now = Instant::now();

    if let Some((last_path, last_type, at)) = &buf.last_pushed {
        if last_type == event_type && last_path == &path_str && now.duration_since(*at) < FS_WATCH_COALESCE_WINDOW {
            if let Some(back) = buf.events.back_mut() {
                if back.event_type == event_type && back.path == path_str {
                    let (size, is_directory) = stat_path(path, event_type);
                    back.size = size;
                    back.is_directory = is_directory;
                    back.observed_at = Utc::now().to_rfc3339();
                    buf.last_pushed = Some((path_str, event_type.to_string(), now));
                    return;
                }
            }
        }
    }

    let (size, is_directory) = stat_path(path, event_type);
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let extension = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();

    let fs_event = wit_fs_watch::FsEvent {
        event_type: event_type.to_string(),
        path: path_str.clone(),
        previous_path: previous_path.map(|p| p.to_string_lossy().to_string()).unwrap_or_default(),
        name,
        extension,
        size,
        is_directory,
        observed_at: Utc::now().to_rfc3339(),
    };

    if buf.events.len() >= FS_WATCH_BUFFER_CAP {
        buf.events.pop_front();
        buf.overflowed = true;
    }
    buf.events.push_back(fs_event);
    buf.last_pushed = Some((path_str, event_type.to_string(), now));
}

/// Best-effort `(size, is_directory)` for `path` at the moment of an event —
/// `(0, false)` for "deleted" (nothing left to stat) and for any path that
/// no longer exists by the time this runs, which is expected under fast
/// concurrent modification, not a bug: this is advisory metadata, not a
/// guarantee the path still looks like this by the time the plugin reads
/// the event.
fn stat_path(path: &Path, event_type: &str) -> (u64, bool) {
    if event_type == "deleted" {
        return (0, false);
    }
    match std::fs::metadata(path) {
        Ok(meta) => (if meta.is_dir() { 0 } else { meta.len() }, meta.is_dir()),
        Err(_) => (0, false),
    }
}

#[cfg(test)]
mod fs_watch_registry_tests {
    use super::*;

    fn target(path: &std::path::Path, events: &[&str]) -> wit_fs_watch::WatchTarget {
        wit_fs_watch::WatchTarget {
            path: path.to_string_lossy().to_string(),
            recursive: false,
            events: events.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Normal and edge case: a sandbox scope permits paths inside its root and
    /// refuses siblings, `..` escapes and (when the root is unusable) everything.
    #[test]
    fn watch_scope_confines_paths_to_the_sandbox_root() {
        let root = tempfile::tempdir().expect("tempdir create failed");
        let outside = tempfile::tempdir().expect("tempdir create failed");
        let inside_path = root.path().join("watched");
        std::fs::create_dir(&inside_path).expect("mkdir failed");

        let scope = FsWatchScope::from_root(Some(root.path()));
        assert!(scope.permits(&inside_path.to_string_lossy()));
        assert!(!scope.permits(&outside.path().to_string_lossy()));
        let escape = inside_path.join("..").join("..");
        assert!(!scope.permits(&escape.to_string_lossy()));

        assert!(FsWatchScope::from_root(None).permits(&outside.path().to_string_lossy()));
        assert!(!FsWatchScope::from_root(Some(Path::new("/nonexistent/sandbox"))).permits(&inside_path.to_string_lossy()));
    }

    /// Normal case: creating a file inside a watched directory produces one
    /// "created" `fs-event` with the expected path/name/extension on the
    /// next `poll`.
    #[test]
    fn poll_reports_a_created_file() {
        let dir = tempfile::tempdir().expect("tempdir create failed");
        let registry = FsWatchRegistry::new();
        let t = target(dir.path(), &["created"]);

        registry.poll("test.plugin", &t).expect("first poll (watch start) failed");
        std::fs::write(dir.path().join("note.txt"), b"hi").expect("write failed");
        // notify's OS callback is asynchronous; give it a moment to fire
        // before polling again. Generous relative to FS_WATCH_COALESCE_WINDOW
        // so this isn't racing the coalescing logic under test elsewhere.
        std::thread::sleep(Duration::from_millis(800));

        let result = registry.poll("test.plugin", &t).expect("second poll failed");
        assert!(!result.overflowed);
        assert_eq!(result.events.len(), 1, "expected exactly one coalesced created event");
        assert_eq!(result.events[0].event_type, "created");
        assert_eq!(result.events[0].name, "note.txt");
        assert_eq!(result.events[0].extension, "txt");
    }

    /// Edge case: a `watch-target` whose `events` list is empty is rejected
    /// with `invalid-config` rather than defaulting to "watch everything".
    #[test]
    fn poll_rejects_empty_events_list() {
        let dir = tempfile::tempdir().expect("tempdir create failed");
        let registry = FsWatchRegistry::new();
        let t = target(dir.path(), &[]);

        assert!(matches!(registry.poll("test.plugin", &t), Err(wit_fs_watch::WatchError::InvalidConfig)));
    }

    /// Edge case: a relative path is rejected outright — the host has no
    /// "plugin's working directory" to resolve one against.
    #[test]
    fn poll_rejects_relative_path() {
        let registry = FsWatchRegistry::new();
        let t = wit_fs_watch::WatchTarget {
            path: "relative/path".to_string(),
            recursive: false,
            events: vec!["created".to_string()],
        };

        assert!(matches!(registry.poll("test.plugin", &t), Err(wit_fs_watch::WatchError::InvalidConfig)));
    }

    /// Edge case: a path that doesn't exist yields `path-not-found`, not a
    /// generic failure.
    #[test]
    fn poll_reports_path_not_found() {
        let registry = FsWatchRegistry::new();
        let missing = std::env::temp_dir().join("aerini-fs-watch-test-does-not-exist");
        let t = target(&missing, &["created"]);

        assert!(matches!(registry.poll("test.plugin", &t), Err(wit_fs_watch::WatchError::PathNotFound)));
    }

    /// Edge case: a plugin already at its target quota gets
    /// `quota-exceeded` on a distinct new target, but polling one it
    /// already holds keeps working.
    #[test]
    fn poll_enforces_per_plugin_quota() {
        let registry = FsWatchRegistry::new();
        let mut dirs = Vec::new();
        for _ in 0..FS_WATCH_MAX_TARGETS_PER_PLUGIN {
            let dir = tempfile::tempdir().expect("tempdir create failed");
            let t = target(dir.path(), &["created"]);
            registry.poll("quota.plugin", &t).expect("poll under quota failed");
            dirs.push(dir);
        }

        let overflow_dir = tempfile::tempdir().expect("tempdir create failed");
        let overflow_target = target(overflow_dir.path(), &["created"]);
        assert!(matches!(
            registry.poll("quota.plugin", &overflow_target),
            Err(wit_fs_watch::WatchError::QuotaExceeded)
        ));

        // Re-polling an existing target is not a "new" one — must still
        // succeed at exactly the quota.
        let existing = target(dirs[0].path(), &["created"]);
        assert!(registry.poll("quota.plugin", &existing).is_ok());
    }

    /// `unwatch` on a target nobody is watching is a documented no-op, not
    /// an error.
    #[test]
    fn unwatch_nonexistent_target_is_a_no_op() {
        let dir = tempfile::tempdir().expect("tempdir create failed");
        let registry = FsWatchRegistry::new();
        let t = target(dir.path(), &["created"]);
        registry.unwatch("test.plugin", &t); // must not panic
    }

    fn empty_buffer(wanted: &[&str]) -> Arc<Mutex<TargetBuffer>> {
        Arc::new(Mutex::new(TargetBuffer {
            events: VecDeque::new(),
            overflowed: false,
            pending_rename_from: None,
            last_pushed: None,
            events_wanted: wanted.iter().map(|s| s.to_string()).collect(),
        }))
    }

    /// Normal case: a `RenameMode::Any` event (the catch-all `EventKind`
    /// requires a match arm for) is treated as the destination side of a
    /// rename when nothing is pending and the path exists.
    #[test]
    fn handle_raw_event_treats_existing_any_rename_as_created() {
        let dir = tempfile::tempdir().expect("tempdir create failed");
        let file = dir.path().join("new-name.txt");
        std::fs::write(&file, b"hi").expect("write failed");
        let buffer = empty_buffer(&["created", "renamed"]);

        let event = NotifyEvent::new(EventKind::Modify(ModifyKind::Name(RenameMode::Any))).add_path(file);
        handle_raw_event(&buffer, event);

        let buf = buffer.lock().expect("mutex poisoned");
        assert_eq!(buf.events.len(), 1);
        assert_eq!(buf.events[0].event_type, "created");
        assert!(buf.pending_rename_from.is_none());
    }

    /// Edge case: `RenameMode::Other` for a path that no longer exists sets
    /// up the same pending-rename state as `From`; a later `Any` for the
    /// surviving path correlates into one `renamed` event, not a spurious
    /// delete-then-create pair.
    #[test]
    fn handle_raw_event_correlates_other_and_any_into_one_rename() {
        let dir = tempfile::tempdir().expect("tempdir create failed");
        let old_path = dir.path().join("does-not-exist.txt");
        let new_path = dir.path().join("renamed.txt");
        std::fs::write(&new_path, b"hi").expect("write failed");
        let buffer = empty_buffer(&["created", "renamed", "deleted"]);

        handle_raw_event(
            &buffer,
            NotifyEvent::new(EventKind::Modify(ModifyKind::Name(RenameMode::Other))).add_path(old_path.clone()),
        );
        handle_raw_event(
            &buffer,
            NotifyEvent::new(EventKind::Modify(ModifyKind::Name(RenameMode::Any))).add_path(new_path),
        );

        let buf = buffer.lock().expect("mutex poisoned");
        assert_eq!(buf.events.len(), 1, "expected the pair to correlate into a single event");
        assert_eq!(buf.events[0].event_type, "renamed");
        assert_eq!(buf.events[0].previous_path, old_path.to_string_lossy());
        assert!(buf.pending_rename_from.is_none());
    }

    /// Normal case: `notify`'s cross-backend rescan/overflow signal sets
    /// the honest `overflowed` flag even on an event with no actionable
    /// fs-event of its own.
    #[test]
    fn handle_raw_event_sets_overflowed_on_rescan_flag() {
        let buffer = empty_buffer(&["created"]);

        let event = NotifyEvent::new(EventKind::Other).set_flag(notify::event::Flag::Rescan);
        handle_raw_event(&buffer, event);

        let buf = buffer.lock().expect("mutex poisoned");
        assert!(buf.overflowed);
        assert!(buf.events.is_empty(), "a bare rescan signal is not itself a created/modified/deleted/renamed event");
    }

    /// Edge case: a rescan flag riding on an otherwise-actionable event
    /// still produces that event -- the overflow signal augments, it never
    /// suppresses, normal processing of the same event.
    #[test]
    fn handle_raw_event_rescan_flag_does_not_suppress_the_event() {
        let dir = tempfile::tempdir().expect("tempdir create failed");
        let file = dir.path().join("note.txt");
        std::fs::write(&file, b"hi").expect("write failed");
        let buffer = empty_buffer(&["created"]);

        let event = NotifyEvent::new(EventKind::Create(notify::event::CreateKind::Any))
            .add_path(file)
            .set_flag(notify::event::Flag::Rescan);
        handle_raw_event(&buffer, event);

        let buf = buffer.lock().expect("mutex poisoned");
        assert!(buf.overflowed);
        assert_eq!(buf.events.len(), 1);
        assert_eq!(buf.events[0].event_type, "created");
    }
}
