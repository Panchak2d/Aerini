//! Plugin reload and load-report routes (admin scope required).

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde_json::json;

use aerini_engine::node::NodeRegistry;
use aerini_engine::nodes::register_builtins;
use aerini_engine::plugin_loader::{load_plugins, PluginLoadReport};

use crate::token_store::TokenRecord;
use super::state::{ApiState, internal_error, require_admin};

/// POST /api/plugins/reload — rebuilds the node registry from scratch
/// (built-ins + every `.wasm` in the configured `--plugin-dir`) and swaps
/// it in atomically. No restart required.
///
/// A workflow run already in progress keeps executing against the registry
/// snapshot it started with; only runs, and scheduled/webhook fires,
/// starting after this returns see the reloaded set. See
/// `aerini_engine::node::Reloadable`'s doc comment for the underlying
/// guarantee.
///
/// Admin-scoped: unlike installing your own credential or running your own
/// workflow, a reload changes what code executes for every caller on this
/// server, not just the caller's own resources.
pub async fn reload_plugins(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
) -> impl IntoResponse {
    if let Err(e) = require_admin(&caller) { return e.into_response(); }

    let Some(ref plugin_dir) = s.plugin_dir else {
        return (StatusCode::BAD_REQUEST, Json(json!({
            "error": "no --plugin-dir configured for this server — nothing to reload"
        }))).into_response();
    };

    // Serializes overlapping reloads so a slower-but-earlier one can't
    // finish after, and clobber, a faster-but-later one with stale data —
    // the registry swap itself is atomic, but that alone doesn't order two
    // independent scan+compile+swap sequences against each other.
    let _guard = s.reload_lock.lock().await;

    // Compiling plugins is CPU-bound and blocking, so it runs on the blocking
    // pool; run inline it would stall every other request on this worker.
    let data_dir   = std::sync::Arc::clone(&s.data_dir);
    let db         = std::sync::Arc::clone(&s.db);
    let plugin_dir = std::sync::Arc::clone(plugin_dir);
    let built = tokio::task::spawn_blocking(move || {
        let mut new_registry = NodeRegistry::new();
        register_builtins(&mut new_registry, &data_dir, Some(db));

        // A plugin_dir that doesn't exist on disk is a warning, not an error:
        // the server comes back with just the built-ins, as it does at startup.
        let report = if !plugin_dir.exists() {
            tracing::warn!("plugin_dir {:?} does not exist — no plugins loaded", plugin_dir);
            PluginLoadReport::default()
        } else {
            load_plugins(&mut new_registry, &plugin_dir)
        };
        (new_registry, report)
    }).await;
    let (new_registry, report) = match built {
        Ok(pair) => pair,
        Err(e)   => return internal_error("reload_plugins: task failed", e).into_response(),
    };

    s.registry.reload(new_registry);
    *s.last_load_report.write().await = report.clone();
    (StatusCode::OK, Json(json!(report))).into_response()
}

/// GET /api/plugins/load-report — returns the most recent plugin load
/// outcome (startup, or the last `/api/plugins/reload`), without
/// triggering a new scan+compile+swap.
///
/// Same admin gate as `reload_plugins`: read-only, but the report reveals
/// `plugin_dir` contents and collisions — the same trust level as reload.
pub async fn load_report(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
) -> impl IntoResponse {
    if let Err(e) = require_admin(&caller) { return e.into_response(); }

    let report = s.last_load_report.read().await.clone();
    (StatusCode::OK, Json(json!(report))).into_response()
}
