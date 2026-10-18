//! Dry-run workflow validation route.

use aerini_engine::validation::validate_workflow_json;
use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde_json::json;

use crate::token_store::TokenRecord;
use super::state::{internal_error, require_read, ApiState};
use super::workflows::SaveWorkflowBody;

/// POST /api/validate-workflow — body `{"workflow_json": "<string>"}`, the
/// same envelope `POST /api/workflows` takes. Runs the engine's best-effort
/// static checks against this server's live node registry and returns the
/// report with `200`, including when the workflow is invalid: the request
/// succeeded, the answer is in `valid`. Nothing is saved or executed, so read
/// scope suffices, and no workflow ACL applies — a token restricted to
/// specific workflows can still check a draft before it is granted an id.
pub async fn validate_workflow(
    State(s):          State<ApiState>,
    Extension(caller): Extension<TokenRecord>,
    Json(b):           Json<SaveWorkflowBody>,
) -> impl IntoResponse {
    if let Err(e) = require_read(&caller) { return e.into_response(); }

    let registry = s.registry.current();
    match tokio::task::spawn_blocking(move || validate_workflow_json(&b.workflow_json, &registry)).await {
        Ok(report) => (StatusCode::OK, Json(json!(report))).into_response(),
        Err(e)     => internal_error("validate_workflow: task failed", e).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api_server::routes::test_support::{make_state, token, with_auth};
    use axum::{body::Body, http::Request, routing::post, Router};
    use serde_json::Value;
    use tower::ServiceExt;

    fn app(tag: &str) -> (Router, ApiState) {
        let state = make_state(tag);
        let router = with_auth(
            Router::new().route("/api/validate-workflow", post(validate_workflow)),
            state.clone(),
        );
        (router, state)
    }

    fn workflow_json(nodes: Value, edges: Value) -> String {
        json!({ "id": "wf", "name": "wf", "nodes": nodes, "edges": edges }).to_string()
    }

    fn node(id: &str, type_id: &str) -> Value {
        json!({
            "id": id, "node_type_id": type_id, "node_type": "utility", "name": id,
            "config": {}, "input_schema": {}, "output_schema": {}
        })
    }

    async fn post_validate(app: Router, bearer: Option<&str>, body: Value) -> axum::response::Response {
        let mut req = Request::builder()
            .method("POST")
            .uri("/api/validate-workflow")
            .header("content-type", "application/json");
        if let Some(t) = bearer {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        app.oneshot(req.body(Body::from(body.to_string())).unwrap()).await.unwrap()
    }

    async fn json_body(res: axum::response::Response) -> Value {
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn requires_auth_and_read_scope() {
        let (app, state) = app("validate_auth");
        let body = json!({ "workflow_json": "{}" });

        let res = post_validate(app.clone(), None, body.clone()).await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        let write_only = token(&state, &["write"]);
        let res = post_validate(app, Some(&write_only), body).await;
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn valid_workflow_reports_valid_true_and_saves_nothing() {
        let (app, state) = app("validate_ok");
        let reader = token(&state, &["read"]);
        let wf = workflow_json(
            json!([node("t", "manual_trigger"), node("o", "output")]),
            json!([{ "id": "e1", "from_node": "t", "from_port": "output", "to_node": "o", "to_port": "input" }]),
        );
        let res = post_validate(app, Some(&reader), json!({ "workflow_json": wf })).await;
        assert_eq!(res.status(), StatusCode::OK);
        let v = json_body(res).await;
        assert_eq!(v["valid"], json!(true), "{v}");
        assert_eq!(v["best_effort"], json!(true));
        assert!(state.db.list().unwrap().is_empty(), "validation must not persist the workflow");
    }

    #[tokio::test]
    async fn invalid_workflow_is_a_200_with_the_errors_listed() {
        let (app, state) = app("validate_bad");
        let reader = token(&state, &["read"]);

        let wf = workflow_json(json!([node("t", "manual_trigger"), node("x", "no_such_type")]), json!([]));
        let res = post_validate(app.clone(), Some(&reader), json!({ "workflow_json": wf })).await;
        assert_eq!(res.status(), StatusCode::OK);
        let v = json_body(res).await;
        assert_eq!(v["valid"], json!(false));
        assert!(v["errors"].as_array().unwrap().iter().any(|e| e["check"] == "node_types" && e["node_id"] == "x"), "{v}");

        let res = post_validate(app, Some(&reader), json!({ "workflow_json": "{not json" })).await;
        assert_eq!(res.status(), StatusCode::OK);
        let v = json_body(res).await;
        assert_eq!(v["checks_run"], json!(["parse"]));
    }

    #[tokio::test]
    async fn body_without_the_workflow_json_envelope_is_a_client_error() {
        let (app, state) = app("validate_envelope");
        let reader = token(&state, &["read"]);
        let res = post_validate(app, Some(&reader), json!({ "workflow": "x" })).await;
        assert!(res.status().is_client_error(), "{}", res.status());
    }
}
