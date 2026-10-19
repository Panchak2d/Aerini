//! Node catalog route: what node types this server can run, with their
//! config schemas and ports, so a client can author workflows without
//! guessing.

use aerini_engine::node::{NodeDescriptor, NodeRegistry};
use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde_json::{json, Value};

use crate::token_store::TokenRecord;
use super::state::{internal_error, require_read, ApiState};

/// Which of the server's execution switches are off. A node whose type is
/// governed by one of them is listed with `"enabled": false`: it can be saved
/// in a workflow but fails when a run reaches it.
#[derive(Clone, Copy)]
pub(super) struct DisabledNodeKinds {
    pub shell:    bool,
    pub code:     bool,
    pub database: bool,
}

impl DisabledNodeKinds {
    fn disables(self, type_id: &str) -> bool {
        match type_id {
            "shell_exec" => self.shell,
            "code"       => self.code,
            "database"   => self.database,
            _            => false,
        }
    }
}

/// One catalog entry per registered node, sorted by `type_id` so the output
/// is stable between calls. The plugin-declared `icon` markup is dropped: it
/// is untrusted, large, and of no use to a client that is not drawing a
/// palette.
pub(super) fn catalog(
    registry: &NodeRegistry,
    disabled: DisabledNodeKinds,
) -> Result<Vec<Value>, serde_json::Error> {
    let mut descriptors: Vec<NodeDescriptor> = registry.all_descriptors();
    descriptors.sort_by(|a, b| a.type_id.cmp(&b.type_id));

    descriptors
        .iter()
        .map(|d| -> Result<Value, serde_json::Error> {
            let mut entry = serde_json::to_value(d)?;
            if let Some(fields) = entry.as_object_mut() {
                fields.remove("icon");
                fields.insert("enabled".to_string(), json!(!disabled.disables(&d.type_id)));
            }
            Ok(entry)
        })
        .collect()
}

/// GET /api/nodes — the live registry (built-ins plus loaded plugins) as a
/// catalog. Read scope; not tied to any workflow, so the per-workflow ACL
/// does not apply.
pub async fn list_nodes(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
) -> impl IntoResponse {
    if let Err(e) = require_read(&caller) { return e.into_response(); }

    let disabled = DisabledNodeKinds {
        shell:    s.shell_exec_disabled,
        code:     s.code_exec_disabled,
        database: s.database_exec_disabled,
    };
    match catalog(&s.registry.current(), disabled) {
        Ok(nodes) => (
            StatusCode::OK,
            Json(json!({ "engine_version": aerini_engine::ENGINE_VERSION, "nodes": nodes })),
        ).into_response(),
        Err(e) => internal_error("list_nodes: serialize", e).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api_server::routes::test_support::{make_state, token, with_auth};
    use axum::{body::Body, http::Request, routing::get, Router};
    use std::sync::Arc;
    use tower::ServiceExt;

    const ALL_ENABLED: DisabledNodeKinds = DisabledNodeKinds { shell: false, code: false, database: false };

    fn builtin_registry(tag: &str) -> Arc<NodeRegistry> {
        make_state(tag).registry.current()
    }

    fn entry<'a>(nodes: &'a [Value], type_id: &str) -> &'a Value {
        nodes.iter().find(|n| n["type_id"] == type_id).unwrap_or_else(|| panic!("{type_id} missing"))
    }

    #[test]
    fn catalog_is_sorted_by_type_id_and_has_no_icon_field() {
        let nodes = catalog(&builtin_registry("nodes_sorted"), ALL_ENABLED).unwrap();
        let ids: Vec<&str> = nodes.iter().map(|n| n["type_id"].as_str().unwrap()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted);
        assert!(!ids.is_empty());
        assert!(nodes.iter().all(|n| n.get("icon").is_none()));
        assert!(nodes.iter().all(|n| n["input_schema"].is_object() && n["ports"].is_object()));
    }

    #[test]
    fn disabled_kinds_mark_only_their_own_node_types() {
        let registry = builtin_registry("nodes_disabled");
        let all_off = DisabledNodeKinds { shell: true, code: true, database: true };
        let nodes = catalog(&registry, all_off).unwrap();
        for id in ["shell_exec", "code", "database"] {
            assert_eq!(entry(&nodes, id)["enabled"], json!(false), "{id}");
        }
        assert_eq!(entry(&nodes, "http_request")["enabled"], json!(true));

        let only_shell = DisabledNodeKinds { shell: true, code: false, database: false };
        let nodes = catalog(&registry, only_shell).unwrap();
        assert_eq!(entry(&nodes, "shell_exec")["enabled"], json!(false));
        assert_eq!(entry(&nodes, "code")["enabled"], json!(true));
        assert_eq!(entry(&nodes, "database")["enabled"], json!(true));
    }

    async fn get_nodes(app: Router, bearer: Option<&str>) -> axum::response::Response {
        let mut req = Request::builder().uri("/api/nodes");
        if let Some(t) = bearer {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        app.oneshot(req.body(Body::empty()).unwrap()).await.unwrap()
    }

    #[tokio::test]
    async fn route_requires_auth_and_read_scope_and_returns_the_catalog() {
        let state = make_state("nodes_route");
        let app = with_auth(Router::new().route("/api/nodes", get(list_nodes)), state.clone());

        assert_eq!(get_nodes(app.clone(), None).await.status(), StatusCode::UNAUTHORIZED);

        let write_only = token(&state, &["write"]);
        assert_eq!(get_nodes(app.clone(), Some(&write_only)).await.status(), StatusCode::FORBIDDEN);

        let reader = token(&state, &["read"]);
        let res = get_nodes(app, Some(&reader)).await;
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["engine_version"], json!(aerini_engine::ENGINE_VERSION));
        let nodes = v["nodes"].as_array().expect("nodes array");
        assert_eq!(entry(nodes, "shell_exec")["enabled"], json!(false), "make_state disables shell");
    }
}
