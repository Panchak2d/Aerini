mod commands;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use flowo_engine::{
    db::WorkflowDb,
    node::NodeRegistry,
    nodes::register_builtins,
    scheduler::SchedulerDaemon,
    store::{CredentialStore, KeySource, StoreCredentialResolver},
    EventSink,
};
use tauri::Manager;
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_dialog::DialogExt;
use tokio_util::sync::CancellationToken;

/// Holds the active manual-run cancellation token.
/// Replaced at the start of each run; cleared on completion.
pub struct ActiveRunToken(pub Mutex<Option<CancellationToken>>);

struct TauriEventSink {
    handle: tauri::AppHandle,
}

impl EventSink for TauriEventSink {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        let _ = tauri::Emitter::emit(&self.handle, event, payload);
    }
}

/// Read a .flowo workflow file from an absolute path.
/// Restricted to .flowo extension only.
#[tauri::command]
fn read_text_file(path: String) -> Result<String, String> {
    let canonical = std::fs::canonicalize(&path)
        .map_err(|e| format!("Could not resolve path '{}': {}", path, e))?;
    let canonical_str = canonical.to_string_lossy();
    if !canonical_str.ends_with(".flowo") {
        return Err("Only .flowo files can be opened this way.".to_string());
    }
    std::fs::read_to_string(&canonical)
        .map_err(|e| format!("Could not read file '{}': {}", canonical_str, e))
}

#[tauri::command]
async fn save_file_dialog(
    app:      tauri::AppHandle,
    content:  String,
    filename: String,
) -> Result<String, String> {
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<tauri_plugin_dialog::FilePath>>();
    app.dialog()
        .file()
        .set_file_name(&filename)
        .add_filter("Flowo Workflow", &["flowo"])
        .save_file(move |path| { let _ = tx.send(path); });

    match rx.await {
        Ok(Some(path)) => {
            let path_buf = path.as_path()
                .ok_or_else(|| "Invalid path".to_string())?
                .to_path_buf();
            std::fs::write(&path_buf, content.as_bytes())
                .map_err(|e| format!("Write failed: {}", e))?;
            Ok(path_buf.display().to_string())
        }
        Ok(None) => Err("cancelled".to_string()),
        Err(_)   => Err("Dialog error".to_string()),
    }
}

/// Save a server export zip — opens a save dialog filtered for .zip,
/// copies the temp zip to the user-chosen path, then deletes the temp file.
#[tauri::command]
async fn save_export_zip(
    app:      tauri::AppHandle,
    zip_path: String,
    filename: String,
) -> Result<String, String> {
    let src = std::path::PathBuf::from(&zip_path);
    let temp = std::env::temp_dir();
    // Canonicalize to resolve '..' and symlinks before containment check.
    // Also confirms the file exists — canonicalize fails if the path doesn't.
    let canonical_src = src.canonicalize()
        .map_err(|_| format!("Export zip not found at: {}", zip_path))?;
    if !canonical_src.starts_with(&temp) {
        return Err("zip_path must be within the system temp directory".to_string());
    }

    let (tx, rx) = tokio::sync::oneshot::channel::<Option<tauri_plugin_dialog::FilePath>>();
    app.dialog()
        .file()
        .set_file_name(&filename)
        .add_filter("Zip Archive", &["zip"])
        .save_file(move |path| { let _ = tx.send(path); });

    match rx.await {
        Ok(Some(path)) => {
            let dest = path.as_path()
                .ok_or_else(|| "Invalid path".to_string())?
                .to_path_buf();
            std::fs::copy(&canonical_src, &dest)
                .map_err(|e| format!("Copy failed: {}", e))?;
            let _ = std::fs::remove_file(&canonical_src);
            Ok(dest.display().to_string())
        }
        Ok(None) => {
            let _ = std::fs::remove_file(&canonical_src);
            Err("cancelled".to_string())
        }
        Err(_) => Err("Dialog error".to_string()),
    }
}

#[tauri::command]
fn close_window(app: tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
}

#[tauri::command]
fn force_quit(
    app:    tauri::AppHandle,
    daemon: tauri::State<'_, Arc<SchedulerDaemon>>,
) {
    daemon.stop_all();
    app.exit(0);
}

#[tauri::command]
async fn check_nodejs_available() -> bool {
    std::process::Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Cancel the currently running manual workflow, if any. No-op when idle.
#[tauri::command]
fn cancel_run(active_run: tauri::State<'_, Arc<ActiveRunToken>>) {
    if let Some(ref token) = *active_run.0.lock().unwrap() {
        token.cancel();
    }
}

#[tauri::command]
fn get_autostart(app: tauri::AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
fn set_autostart(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    let al = app.autolaunch();
    if enabled { al.enable().map_err(|e| e.to_string()) }
    else       { al.disable().map_err(|e| e.to_string()) }
}

/// Open a native folder picker dialog and return the chosen path.
/// Returns null (None → JS null) when the user cancels.
#[tauri::command]
async fn pick_folder_dialog(app: tauri::AppHandle) -> Option<String> {
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<tauri_plugin_dialog::FilePath>>();
    app.dialog()
        .file()
        .pick_folder(move |path| { let _ = tx.send(path); });
    match rx.await {
        Ok(Some(path)) => path.as_path().map(|p| p.display().to_string()),
        _ => None,
    }
}

/// Write base64-encoded media bytes to a temp file and return the absolute path.
///
/// G9: strips `/`, `\`, and `..` from the filename before constructing the path.
/// The file is written to `{temp_dir}/flowo_media/{safe_filename}`.
#[tauri::command]
async fn write_temp_file(filename: String, data: String) -> Result<String, String> {
    // G9: path traversal protection — strip separators and collapse ".."
    let safe_name: String = filename
        .chars()
        .filter(|c| *c != '/' && *c != '\\')
        .collect::<String>()
        .split("..")
        .collect::<Vec<_>>()
        .join("");
    let safe_name = safe_name.trim().to_string();
    if safe_name.is_empty() {
        return Err("Invalid filename: empty after sanitisation".to_string());
    }

    let dir = std::env::temp_dir().join("flowo_media");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("Failed to create temp dir: {}", e))?;

    const MAX_TEMP_FILE_BYTES: usize = 50 * 1024 * 1024; // 50 MB
    let bytes = BASE64.decode(&data)
        .map_err(|e| format!("Base64 decode error: {}", e))?;
    if bytes.len() > MAX_TEMP_FILE_BYTES {
        return Err(format!(
            "File too large: {} bytes (limit {} bytes)",
            bytes.len(),
            MAX_TEMP_FILE_BYTES,
        ));
    }

    let path = dir.join(&safe_name);
    tokio::fs::write(&path, &bytes)
        .await
        .map_err(|e| format!("Write failed: {}", e))?;

    Ok(path.display().to_string())
}

/// G5: Delete files in `{temp_dir}/flowo_media/` that are older than 24 hours.
/// Must be called via spawn_blocking — uses std::fs which is synchronous.
fn cleanup_old_temp_files(dir: &std::path::Path) {
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(86_400))
        .unwrap_or(std::time::UNIX_EPOCH);

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if let Ok(modified) = meta.modified() {
                    if modified < cutoff {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .setup(|app| {
            let data_dir: PathBuf = app.path().app_data_dir()
                .expect("Failed to resolve app data directory");
            std::fs::create_dir_all(&data_dir)?;

            let db = Arc::new(
                WorkflowDb::open(&data_dir.join("workflows.db"), 8)
                    .expect("Failed to open workflow database")
            );
            let cred_store = Arc::new(
                CredentialStore::open(
                    &data_dir.join("credentials.db"),
                    KeySource::OsKeychain { fallback: data_dir.join(".cred.key") },
                ).expect("Failed to open credential store")
            );
            let mut registry = NodeRegistry::new();
            register_builtins(&mut registry, &data_dir, Some(Arc::clone(&db)));
            let registry = Arc::new(registry);

            let resolver = Arc::new(StoreCredentialResolver { store: Arc::clone(&cred_store) });

            let event_sink: Arc<dyn EventSink> = Arc::new(TauriEventSink {
                handle: app.handle().clone(),
            });

            let parallel_execution = db.get_setting("parallel_execution")
                .unwrap_or(None)
                .map(|v| v == "true")
                .unwrap_or(false);

            let daemon = Arc::new(SchedulerDaemon::new(
                Arc::clone(&db) as Arc<dyn flowo_engine::scheduler::SchedulerDb>,
                Arc::clone(&registry),
                Arc::clone(&resolver) as Arc<dyn flowo_engine::executor::CredentialResolver>,
                Arc::clone(&event_sink),
            ).with_parallel_execution(parallel_execution));
            daemon.start(tauri::async_runtime::handle().inner());

            app.manage(Arc::clone(&db));
            app.manage(Arc::clone(&cred_store));
            app.manage(Arc::clone(&registry));
            app.manage(
                Arc::clone(&resolver)
                    as Arc<dyn flowo_engine::executor::CredentialResolver>
            );
            app.manage(Arc::clone(&daemon));
            app.manage(Arc::clone(&event_sink));
            app.manage(Arc::new(ActiveRunToken(Mutex::new(None))));

            // G5: async startup cleanup of temp media files older than 24h
            // spawn_blocking so the std::fs directory scan doesn't occupy a tokio thread
            let temp_media_dir = std::env::temp_dir().join("flowo_media");
            tauri::async_runtime::spawn(async move {
                let _ = tokio::task::spawn_blocking(move || {
                    cleanup_old_temp_files(&temp_media_dir);
                }).await;
            });

            // OS close button hides to tray rather than quitting
            let main_window = app.get_webview_window("main")
                .expect("main webview window must exist at setup time");
            let main_window_clone = main_window.clone();
            main_window.on_window_event(move |event| {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = tauri::Emitter::emit(
                        &main_window_clone,
                        "tauri://close-requested",
                        (),
                    );
                }
            });

            let tray_menu = tauri::menu::MenuBuilder::new(app)
                .item(&tauri::menu::MenuItemBuilder::with_id("show", "Open Flowo").build(app)?)
                .separator()
                .item(&tauri::menu::MenuItemBuilder::with_id("quit", "Quit Flowo").build(app)?)
                .build()?;

            let _tray = tauri::tray::TrayIconBuilder::new()
                .icon(app.default_window_icon().expect("app must have a window icon configured").clone())
                .menu(&tray_menu)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => {
                        if let Some(daemon) = app.try_state::<Arc<SchedulerDaemon>>() {
                            daemon.stop_all();
                        }
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let tauri::tray::TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Left, ..
                    } = event {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            if window.is_visible().unwrap_or(false) {
                                let _ = window.hide();
                            } else {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                    }
                })
                .build(app)?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            read_text_file,
            save_file_dialog,
            save_export_zip,
            close_window,
            force_quit,
            check_nodejs_available,
            cancel_run,
            get_autostart,
            set_autostart,
            commands::workflow::save_workflow,
            commands::workflow::load_workflow,
            commands::workflow::list_workflows,
            commands::workflow::delete_workflow,
            commands::workflow::run_workflow,
            commands::workflow::get_node_types,
            commands::workflow::get_setting,
            commands::workflow::set_setting,
            commands::workflow::save_run_record,
            commands::workflow::list_run_records,
            commands::workflow::delete_run_record,
            commands::workflow::clear_run_records,
            commands::workflow::save_version,
            commands::workflow::list_versions,
            commands::workflow::get_version,
            commands::workflow::delete_version,
            commands::credentials::list_credentials,
            commands::credentials::save_credential,
            commands::credentials::delete_credential,
            // Keep original command names for IPC compatibility with frontend
            commands::scheduler::start_scheduled_workflow,
            commands::scheduler::stop_scheduled_workflow,
            commands::scheduler::get_scheduled_jobs,
            commands::scheduler::set_always_on,
            commands::scheduler::stop_all_jobs,
            commands::scheduler::request_scheduler_state,
            commands::export::validate_workflow_for_export,
            commands::export::generate_server_package,
            commands::export::generate_docker_package,
            pick_folder_dialog,
            write_temp_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running flowo")
}
