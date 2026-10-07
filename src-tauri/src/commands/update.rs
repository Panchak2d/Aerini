//! In-app updater commands. The webview never supplies a URL or version: it can
//! only ask to check, install the update found by the last check, or cancel.
//! Updates are fetched and signature-verified by `tauri-plugin-updater`, which
//! is registered Rust-side only and has no webview permission.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aerini_engine::scheduler::SchedulerDaemon;
use serde::{Deserialize, Serialize};
use tauri::utils::config::BundleType;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Error as UpdaterError, Update, UpdaterExt};
use tokio_util::sync::CancellationToken;

const REPO_OWNER: &str = "Panchak2d";
const REPO_NAME: &str = "aerini";

const CHECK_TIMEOUT: Duration = Duration::from_secs(15);
/// Absolute ceiling for one download. Stalls are caught much earlier by
/// `DOWNLOAD_STALL_LIMIT`, so this only bounds a connection that keeps trickling.
const DOWNLOAD_HARD_LIMIT: Duration = Duration::from_secs(30 * 60);
const DOWNLOAD_STALL_LIMIT: Duration = Duration::from_secs(45);
const STALL_POLL: Duration = Duration::from_secs(5);
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
const RUN_DRAIN_GRACE: Duration = Duration::from_secs(3);
const RUN_DRAIN_POLL: Duration = Duration::from_millis(50);

const PROGRESS_EVENT: &str = "update-progress";
const MARKER_FILE: &str = ".update-pending";
const MARKER_MAX_AGE_SECS: u64 = 24 * 60 * 60;

/// Release page for one version. Built from constants plus a character-checked
/// version, never from manifest-supplied URLs.
pub fn release_url(version: &str) -> String {
    let v = version.strip_prefix('v').unwrap_or(version);
    let well_formed = !v.is_empty()
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'));
    if well_formed {
        format!("https://github.com/{REPO_OWNER}/{REPO_NAME}/releases/tag/v{v}")
    } else {
        releases_page_url()
    }
}

fn releases_page_url() -> String {
    format!("https://github.com/{REPO_OWNER}/{REPO_NAME}/releases/latest")
}

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ManualReason {
    UnknownInstall,
    AppImageMissing,
    AppImageNotWritable,
    MacosTranslocated,
    MacosDiskImage,
    NoReleaseForPlatform,
}

impl ManualReason {
    fn message(self) -> &'static str {
        match self {
            Self::UnknownInstall => {
                "This copy of Aerini was not installed from an official installer, so it can't update itself."
            }
            Self::AppImageMissing => {
                "This AppImage was launched in a way that hides its file location, so it can't update itself."
            }
            Self::AppImageNotWritable => {
                "The folder containing this AppImage is read-only, so it can't update itself."
            }
            Self::MacosTranslocated => {
                "Aerini is running from a temporary location. Move it to Applications and reopen it first."
            }
            Self::MacosDiskImage => {
                "Aerini is running from a disk image. Move it to Applications and reopen it first."
            }
            Self::NoReleaseForPlatform => {
                "In-app updates aren't available for this platform yet."
            }
        }
    }
}

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InstallSupport {
    Supported { admin_prompt: bool },
    Manual { reason: ManualReason, message: &'static str },
}

impl InstallSupport {
    fn manual(reason: ManualReason) -> Self {
        Self::Manual { reason, message: reason.message() }
    }
}

fn classify_support(
    os: &str,
    bundle: Option<BundleType>,
    exe_path: &Path,
    appimage: Option<&Path>,
    appimage_dir_writable: bool,
) -> InstallSupport {
    use InstallSupport::Supported;
    match (os, bundle) {
        (_, None) => InstallSupport::manual(ManualReason::UnknownInstall),
        ("macos", Some(_)) => {
            if exe_path.components().any(|c| c.as_os_str() == "AppTranslocation") {
                InstallSupport::manual(ManualReason::MacosTranslocated)
            } else if exe_path.starts_with("/Volumes") {
                InstallSupport::manual(ManualReason::MacosDiskImage)
            } else {
                Supported { admin_prompt: false }
            }
        }
        ("linux", Some(BundleType::AppImage)) => match appimage {
            None => InstallSupport::manual(ManualReason::AppImageMissing),
            Some(_) if !appimage_dir_writable => {
                InstallSupport::manual(ManualReason::AppImageNotWritable)
            }
            Some(_) => Supported { admin_prompt: false },
        },
        ("linux", Some(BundleType::Deb | BundleType::Rpm)) => Supported { admin_prompt: true },
        ("windows", Some(BundleType::Nsis)) => Supported { admin_prompt: false },
        ("windows", Some(BundleType::Msi)) => Supported { admin_prompt: true },
        _ => InstallSupport::manual(ManualReason::UnknownInstall),
    }
}

#[cfg(target_os = "linux")]
fn appimage_path(app: &AppHandle) -> Option<PathBuf> {
    app.env().appimage.map(PathBuf::from)
}

#[cfg(not(target_os = "linux"))]
fn appimage_path(_app: &AppHandle) -> Option<PathBuf> {
    None
}

/// Creating and dropping a scratch file is the only check that also honors
/// read-only mounts and ACLs; permission bits alone do not.
fn dir_is_writable(dir: &Path) -> bool {
    tempfile::Builder::new()
        .prefix(".aerini-write-test")
        .tempfile_in(dir)
        .is_ok()
}

fn current_install_support(app: &AppHandle) -> InstallSupport {
    let exe = tauri::utils::platform::current_exe().unwrap_or_default();
    let appimage = appimage_path(app);
    let writable = appimage
        .as_deref()
        .and_then(Path::parent)
        .is_some_and(dir_is_writable);
    classify_support(
        std::env::consts::OS,
        tauri::utils::platform::bundle_type(),
        &exe,
        appimage.as_deref(),
        writable,
    )
}

#[derive(Serialize)]
pub struct UpdateCheckResult {
    pub current_version: String,
    pub available: bool,
    pub latest_version: Option<String>,
    pub install_support: InstallSupport,
    pub release_url: String,
}

fn check_result(
    current: &str,
    latest: Option<String>,
    available: bool,
    install_support: InstallSupport,
) -> UpdateCheckResult {
    let release_url = match (&latest, available) {
        (Some(v), true) => release_url(v),
        _ => releases_page_url(),
    };
    UpdateCheckResult {
        current_version: current.to_string(),
        available,
        latest_version: latest,
        install_support,
        release_url,
    }
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum InstallOutcome {
    /// Nothing was installed. Re-invoke with `force: true` after the user confirms.
    ActiveRuns { scheduled: usize, manual: usize },
    Cancelled,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NoticeKind {
    Installed,
    Incomplete,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct UpdateNotice {
    pub kind: NoticeKind,
    pub version: String,
}

#[derive(Serialize, Deserialize)]
struct Marker {
    to: String,
    created_unix: u64,
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn notice_from_marker(raw: &str, current: &str, now: u64) -> Option<UpdateNotice> {
    let marker: Marker = serde_json::from_str(raw).ok()?;
    if now.saturating_sub(marker.created_unix) > MARKER_MAX_AGE_SECS {
        return None;
    }
    let same = marker.to.trim_start_matches('v') == current.trim_start_matches('v');
    Some(UpdateNotice {
        kind: if same { NoticeKind::Installed } else { NoticeKind::Incomplete },
        version: marker.to,
    })
}

pub fn marker_path(data_dir: &Path) -> PathBuf {
    data_dir.join(MARKER_FILE)
}

fn write_marker(path: &Path, version: &str) -> std::io::Result<()> {
    let marker = Marker { to: version.to_string(), created_unix: now_unix() };
    std::fs::write(path, serde_json::to_vec(&marker)?)
}

fn remove_marker(path: &Path) {
    if let Err(e) = std::fs::remove_file(path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(error = %e, "could not remove update marker");
        }
    }
}

/// Reads and deletes the marker left by an install attempt, so the notice is
/// reported once. A missing, corrupt or stale marker yields `None`.
pub fn consume_startup_marker(data_dir: &Path, current: &str) -> Option<UpdateNotice> {
    let path = marker_path(data_dir);
    let raw = std::fs::read_to_string(&path).ok();
    if raw.is_some() {
        remove_marker(&path);
    }
    notice_from_marker(&raw?, current, now_unix())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Checking,
    Downloading,
    Installing,
}

struct Inner {
    phase: Phase,
    pending: Option<Update>,
    /// Verified bytes of `pending`, kept so confirming an active-run prompt or
    /// retrying a failed install does not download again.
    staged: Option<Vec<u8>>,
    cancel: Option<CancellationToken>,
    notice: Option<UpdateNotice>,
}

fn lock(inner: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    inner.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Resets the phase on every exit path, including early returns and panics.
struct PhaseGuard {
    inner: Arc<Mutex<Inner>>,
}

impl Drop for PhaseGuard {
    fn drop(&mut self) {
        let mut g = lock(&self.inner);
        g.phase = Phase::Idle;
        g.cancel = None;
    }
}

pub struct UpdateState {
    marker_path: PathBuf,
    /// Set by the Windows pre-exit hook once background jobs were stopped, so a
    /// failed launch of the installer can re-arm them.
    jobs_stopped: Arc<AtomicBool>,
    inner: Arc<Mutex<Inner>>,
}

impl UpdateState {
    pub fn new(marker_path: PathBuf, notice: Option<UpdateNotice>) -> Self {
        Self {
            marker_path,
            jobs_stopped: Arc::new(AtomicBool::new(false)),
            inner: Arc::new(Mutex::new(Inner {
                phase: Phase::Idle,
                pending: None,
                staged: None,
                cancel: None,
                notice,
            })),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        lock(&self.inner)
    }

    fn begin(&self, phase: Phase) -> Result<PhaseGuard, String> {
        let mut g = self.lock();
        match g.phase {
            Phase::Idle => {
                g.phase = phase;
                Ok(PhaseGuard { inner: Arc::clone(&self.inner) })
            }
            Phase::Checking => Err("An update check is already in progress.".to_string()),
            Phase::Downloading => Err("An update is already downloading.".to_string()),
            Phase::Installing => Err("An update is already installing.".to_string()),
        }
    }

    fn clear_pending(&self) {
        let mut g = self.lock();
        g.pending = None;
        g.staged = None;
    }
}

#[derive(Serialize, Clone)]
struct ProgressPayload {
    stage: &'static str,
    downloaded: u64,
    total: Option<u64>,
}

fn emit_progress(app: &AppHandle, stage: &'static str, downloaded: u64, total: Option<u64>) {
    let _ = app.emit(PROGRESS_EVENT, ProgressPayload { stage, downloaded, total });
}

fn user_message(e: &UpdaterError) -> String {
    use UpdaterError as E;
    tracing::warn!(error = %e, "updater error");
    match e {
        E::Reqwest(r) if r.is_timeout() => {
            "The update server did not respond in time. Check your connection and try again.".into()
        }
        E::Reqwest(r) if r.is_connect() => {
            "Could not reach the update server. Check your connection and try again.".into()
        }
        E::Reqwest(_) => "A network error interrupted the update. Try again.".into(),
        E::Network(detail) => format!("The update download failed: {detail}"),
        E::ReleaseNotFound => "No update information is published yet. Try again later.".into(),
        E::Minisign(_)
        | E::Base64(_)
        | E::SignatureUtf8(_)
        | E::SignedVersionMismatch { .. }
        | E::MissingSignedVersion => {
            "The downloaded update failed signature verification and was not installed.".into()
        }
        E::AuthenticationFailed => {
            "Administrator authentication was cancelled or failed. The update was not installed."
                .into()
        }
        E::DebInstallFailed | E::PackageInstallFailed => {
            "The system package manager could not install the update.".into()
        }
        E::TempDirNotOnSameMountPoint | E::TempDirNotFound | E::Io(_) => {
            "Aerini could not write the update to disk. Check free space and folder permissions."
                .into()
        }
        _ => format!("Update failed: {e}"),
    }
}

fn active_run_counts(app: &AppHandle) -> (usize, usize) {
    let scheduled = app
        .try_state::<Arc<SchedulerDaemon>>()
        .map_or(0, |d| d.active_runs());
    let manual = app
        .try_state::<Arc<crate::ActiveRunToken>>()
        .map_or(0, |r| r.active_count());
    (scheduled, manual)
}

/// Same effect as the tray's Quit on background jobs, plus cancelling manual
/// runs so their child processes are dropped before files are replaced.
fn quiesce(app: &AppHandle) {
    if let Some(runs) = app.try_state::<Arc<crate::ActiveRunToken>>() {
        runs.cancel_all();
    }
    if let Some(daemon) = app.try_state::<Arc<SchedulerDaemon>>() {
        daemon.stop_all();
    }
}

/// Blocks, so it must only run on a blocking thread (the plugin's pre-exit hook
/// and `spawn_blocking` both qualify).
fn quiesce_and_drain(app: &AppHandle) {
    quiesce(app);
    let deadline = Instant::now() + RUN_DRAIN_GRACE;
    while Instant::now() < deadline {
        let (scheduled, manual) = active_run_counts(app);
        if scheduled + manual == 0 {
            return;
        }
        std::thread::sleep(RUN_DRAIN_POLL);
    }
}

fn restore_jobs_if_stopped(app: &AppHandle, state: &UpdateState) {
    if state.jobs_stopped.swap(false, Ordering::SeqCst) {
        if let Some(daemon) = app.try_state::<Arc<SchedulerDaemon>>() {
            daemon.start(tauri::async_runtime::handle().inner());
        }
    }
}

async fn stall_watch(last_progress_ms: &AtomicU64, started: Instant) {
    let limit_ms = DOWNLOAD_STALL_LIMIT.as_millis() as u64;
    loop {
        tokio::time::sleep(STALL_POLL).await;
        let now_ms = started.elapsed().as_millis() as u64;
        if now_ms.saturating_sub(last_progress_ms.load(Ordering::Relaxed)) >= limit_ms {
            return;
        }
    }
}

/// `Ok(None)` means the user cancelled.
async fn download(
    app: &AppHandle,
    state: &UpdateState,
    update: &Update,
) -> Result<Option<Vec<u8>>, String> {
    let token = CancellationToken::new();
    state.lock().cancel = Some(token.clone());

    let mut update = update.clone();
    update.timeout = Some(DOWNLOAD_HARD_LIMIT);

    let started = Instant::now();
    let last_progress_ms = Arc::new(AtomicU64::new(0));
    let progress_clock = Arc::clone(&last_progress_ms);
    let progress_app = app.clone();
    let mut received: u64 = 0;
    let mut last_emit: Option<Instant> = None;

    emit_progress(app, "downloading", 0, None);
    let fetch = update.download(
        move |chunk, total| {
            received += chunk as u64;
            progress_clock.store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
            let finished = total.is_some_and(|t| received >= t);
            if finished || last_emit.is_none_or(|t| t.elapsed() >= PROGRESS_INTERVAL) {
                last_emit = Some(Instant::now());
                emit_progress(&progress_app, "downloading", received, total);
            }
        },
        || {},
    );

    tokio::select! {
        res = fetch => match res {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) => Err(user_message(&e)),
        },
        _ = token.cancelled() => Ok(None),
        _ = stall_watch(&last_progress_ms, started) => {
            Err("The download stalled. Check your connection and try again.".to_string())
        }
    }
}

#[tauri::command]
pub async fn check_for_update(
    app: AppHandle,
    state: tauri::State<'_, UpdateState>,
) -> Result<UpdateCheckResult, String> {
    let _guard = state.begin(Phase::Checking)?;
    let current = app.package_info().version.to_string();

    // The plugin resolves the platform entry before it reports whether the
    // version is newer, so a manifest without this platform's key fails the
    // whole check. The comparator runs first and records what the manifest
    // announced, which lets that case still say "version X exists, install it
    // manually". It also replaces the allowDowngrades behavior with plain
    // "strictly newer"; do not swap it for the default comparator.
    let announced: Arc<Mutex<Option<(String, bool)>>> = Arc::new(Mutex::new(None));
    let record = Arc::clone(&announced);
    let hook_app = app.clone();
    let jobs_stopped = Arc::clone(&state.jobs_stopped);

    let updater = app
        .updater_builder()
        .timeout(CHECK_TIMEOUT)
        .version_comparator(move |installed, release| {
            let newer = release.version > installed;
            *record.lock().unwrap_or_else(PoisonError::into_inner) =
                Some((release.version.to_string(), newer));
            newer
        })
        .on_before_exit(move || {
            jobs_stopped.store(true, Ordering::SeqCst);
            quiesce_and_drain(&hook_app);
        })
        .build()
        .map_err(|e| user_message(&e))?;

    let support = current_install_support(&app);
    match updater.check().await {
        Ok(Some(update)) => {
            let latest = update.version.clone();
            {
                let mut g = state.lock();
                g.pending = Some(update);
                g.staged = None;
            }
            Ok(check_result(&current, Some(latest), true, support))
        }
        Ok(None) => {
            state.clear_pending();
            Ok(check_result(&current, None, false, support))
        }
        Err(UpdaterError::TargetNotFound(_) | UpdaterError::TargetsNotFound(_)) => {
            state.clear_pending();
            let seen = announced.lock().unwrap_or_else(PoisonError::into_inner).clone();
            match seen {
                Some((version, true)) => Ok(check_result(
                    &current,
                    Some(version),
                    true,
                    InstallSupport::manual(ManualReason::NoReleaseForPlatform),
                )),
                Some((_, false)) => Ok(check_result(&current, None, false, support)),
                None => Err("The update information could not be read.".to_string()),
            }
        }
        Err(e) => Err(user_message(&e)),
    }
}

#[tauri::command]
pub async fn install_update(
    force: Option<bool>,
    app: AppHandle,
    state: tauri::State<'_, UpdateState>,
) -> Result<InstallOutcome, String> {
    let force = force.unwrap_or(false);
    let _guard = state.begin(Phase::Downloading)?;

    if let InstallSupport::Manual { message, .. } = current_install_support(&app) {
        return Err(message.to_string());
    }

    let (update, staged) = {
        let mut g = state.lock();
        match g.pending.clone() {
            Some(update) => (update, g.staged.take()),
            None => return Err("No update is ready. Check for updates first.".to_string()),
        }
    };

    let bytes = match staged {
        Some(bytes) => bytes,
        None => match download(&app, &state, &update).await? {
            Some(bytes) => bytes,
            None => return Ok(InstallOutcome::Cancelled),
        },
    };

    let (scheduled, manual) = active_run_counts(&app);
    if scheduled + manual > 0 && !force {
        state.lock().staged = Some(bytes);
        return Ok(InstallOutcome::ActiveRuns { scheduled, manual });
    }

    state.lock().phase = Phase::Installing;
    state.jobs_stopped.store(false, Ordering::SeqCst);

    if let Err(e) = write_marker(&state.marker_path, &update.version) {
        tracing::warn!(error = %e, "could not write update marker");
    }
    let total = bytes.len() as u64;
    emit_progress(&app, "installing", total, Some(total));

    let installed = tokio::task::spawn_blocking(move || {
        let result = update.install(&bytes);
        (result, bytes)
    })
    .await;

    match installed {
        Ok((Ok(()), _)) => {
            let drain_app = app.clone();
            let _ = tokio::task::spawn_blocking(move || quiesce_and_drain(&drain_app)).await;
            app.restart()
        }
        Ok((Err(e), bytes)) => {
            remove_marker(&state.marker_path);
            restore_jobs_if_stopped(&app, &state);
            state.lock().staged = Some(bytes);
            Err(user_message(&e))
        }
        Err(join_error) => {
            remove_marker(&state.marker_path);
            restore_jobs_if_stopped(&app, &state);
            tracing::error!(error = %join_error, "update installer task failed");
            Err("The update installer stopped unexpectedly.".to_string())
        }
    }
}

#[tauri::command]
pub fn cancel_update_download(state: tauri::State<'_, UpdateState>) -> Result<(), String> {
    let mut g = state.lock();
    match g.phase {
        Phase::Downloading => {
            if let Some(token) = &g.cancel {
                token.cancel();
            }
            Ok(())
        }
        Phase::Installing => {
            Err("The update is already installing and can't be cancelled.".to_string())
        }
        Phase::Idle | Phase::Checking => {
            g.staged = None;
            Ok(())
        }
    }
}

/// Returns the result of the previous install attempt once, then `None`.
/// Pulled by the frontend because an event emitted during startup can fire
/// before the page has registered a listener.
#[tauri::command]
pub fn take_update_notice(state: tauri::State<'_, UpdateState>) -> Option<UpdateNotice> {
    state.lock().notice.take()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUPPORTED_PLAIN: InstallSupport = InstallSupport::Supported { admin_prompt: false };
    const SUPPORTED_ADMIN: InstallSupport = InstallSupport::Supported { admin_prompt: true };

    fn reason(support: InstallSupport) -> Option<ManualReason> {
        match support {
            InstallSupport::Manual { reason, .. } => Some(reason),
            InstallSupport::Supported { .. } => None,
        }
    }

    #[test]
    fn windows_and_deb_rpm_installs_are_supported_with_admin_flag_where_a_prompt_appears() {
        let exe = Path::new("/irrelevant");
        let c = |os, b| classify_support(os, Some(b), exe, None, false);
        assert_eq!(c("windows", BundleType::Nsis), SUPPORTED_PLAIN);
        assert_eq!(c("windows", BundleType::Msi), SUPPORTED_ADMIN);
        assert_eq!(c("linux", BundleType::Deb), SUPPORTED_ADMIN);
        assert_eq!(c("linux", BundleType::Rpm), SUPPORTED_ADMIN);
    }

    #[test]
    fn missing_bundle_type_and_mismatched_os_bundle_pairs_fall_back_to_manual() {
        let exe = Path::new("/irrelevant");
        assert_eq!(
            reason(classify_support("linux", None, exe, None, true)),
            Some(ManualReason::UnknownInstall)
        );
        assert_eq!(
            reason(classify_support("windows", Some(BundleType::Deb), exe, None, true)),
            Some(ManualReason::UnknownInstall)
        );
    }

    #[test]
    fn appimage_needs_a_known_writable_location() {
        let exe = Path::new("/tmp/.mount_x/usr/bin/aerini");
        let image = Path::new("/home/u/Aerini.AppImage");
        let c = |img, writable| {
            classify_support("linux", Some(BundleType::AppImage), exe, img, writable)
        };
        assert_eq!(c(Some(image), true), SUPPORTED_PLAIN);
        assert_eq!(reason(c(Some(image), false)), Some(ManualReason::AppImageNotWritable));
        assert_eq!(reason(c(None, true)), Some(ManualReason::AppImageMissing));
    }

    #[test]
    fn macos_translocated_or_disk_image_paths_are_manual_and_normal_installs_are_not() {
        let c = |p: &str| classify_support("macos", Some(BundleType::App), Path::new(p), None, false);
        assert_eq!(
            reason(c("/private/var/folders/ab/T/AppTranslocation/1234/d/Aerini.app/Contents/MacOS/aerini")),
            Some(ManualReason::MacosTranslocated)
        );
        assert_eq!(
            reason(c("/Volumes/Aerini/Aerini.app/Contents/MacOS/aerini")),
            Some(ManualReason::MacosDiskImage)
        );
        assert_eq!(c("/Applications/Aerini.app/Contents/MacOS/aerini"), SUPPORTED_PLAIN);
    }

    #[test]
    fn release_url_is_built_from_constants_and_rejects_unsafe_versions() {
        assert_eq!(
            release_url("0.5.0"),
            "https://github.com/Panchak2d/aerini/releases/tag/v0.5.0"
        );
        assert_eq!(release_url("v1.2.3-rc.1"), release_url("1.2.3-rc.1"));
        let fallback = "https://github.com/Panchak2d/aerini/releases/latest";
        assert_eq!(release_url(""), fallback);
        assert_eq!(release_url("1.0/../../evil"), fallback);
        assert_eq!(release_url("1.0 0"), fallback);
    }

    #[test]
    fn marker_reports_installed_when_versions_match_and_incomplete_when_they_differ() {
        let raw = r#"{"to":"0.5.0","created_unix":1000}"#;
        assert_eq!(
            notice_from_marker(raw, "0.5.0", 1100),
            Some(UpdateNotice { kind: NoticeKind::Installed, version: "0.5.0".into() })
        );
        assert_eq!(
            notice_from_marker(raw, "0.4.1", 1100),
            Some(UpdateNotice { kind: NoticeKind::Incomplete, version: "0.5.0".into() })
        );
    }

    #[test]
    fn marker_is_ignored_when_stale_or_unparseable() {
        let raw = r#"{"to":"0.5.0","created_unix":1000}"#;
        assert_eq!(notice_from_marker(raw, "0.5.0", 1000 + MARKER_MAX_AGE_SECS + 1), None);
        assert!(notice_from_marker(raw, "0.5.0", 1000 + MARKER_MAX_AGE_SECS).is_some());
        assert_eq!(notice_from_marker("not json", "0.5.0", 1100), None);
    }
}
