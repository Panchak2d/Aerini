mod common;

use common::{reply, serve, TOKEN};
use rmcp::{
    model::{CallToolRequestParams, ProtocolVersion},
    service::RunningService,
    transport::{ConfigureCommandExt, TokioChildProcess},
    ClientHandler, ClientLifecycleMode, ClientServiceExt, RoleClient,
};
use serde_json::{json, Value};
use tokio::process::Command;

#[derive(Clone, Default)]
struct Probe;

impl ClientHandler for Probe {}

const BIN: &str = env!("CARGO_BIN_EXE_aerini-mcp");

async fn connect(base: &str, allow_write: bool) -> RunningService<RoleClient, Probe> {
    let transport = TokioChildProcess::new(Command::new(BIN).configure(|cmd| {
        cmd.env("AERINI_URL", base).env("AERINI_TOKEN", TOKEN).env_remove("AERINI_OUT_DIR");
        if allow_write {
            cmd.arg("--allow-write");
        }
    }))
    .expect("spawn aerini-mcp");
    Probe
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Discover { preferred_versions: vec![ProtocolVersion::V_2026_07_28] },
        )
        .await
        .expect("server/discover handshake")
}

async fn call(client: &RunningService<RoleClient, Probe>, name: &'static str, args: Value) -> Value {
    let mut params = CallToolRequestParams::new(name);
    if let Some(map) = args.as_object() {
        params = params.with_arguments(map.clone());
    }
    let result = client.call_tool(params).await.expect("tools/call answered");
    serde_json::to_value(result).unwrap()
}

#[tokio::test]
async fn discover_reports_the_new_protocol_version_identity_and_instructions() {
    let mock = serve(vec![]).await;
    let client = connect(&mock.base, false).await;

    let info = client.peer_info().expect("discover result kept as peer info");
    assert_eq!(info.protocol_version, ProtocolVersion::V_2026_07_28);
    assert!(info.capabilities.tools.is_some());
    assert_eq!(info.server_info.as_ref().map(|i| i.name.to_string()), Some("aerini-mcp".to_string()));
    assert!(info.instructions.as_deref().unwrap_or_default().contains("validate_workflow"));

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn tools_list_is_sorted_stable_and_carries_honest_annotations() {
    let mock = serve(vec![]).await;
    let client = connect(&mock.base, false).await;

    let first = client.list_all_tools().await.unwrap();
    let second = client.list_all_tools().await.unwrap();
    let names: Vec<String> = first.iter().map(|t| t.name.to_string()).collect();
    let again: Vec<String> = second.iter().map(|t| t.name.to_string()).collect();
    assert_eq!(names, again);
    assert_eq!(
        names,
        [
            "delete_workflow", "get_node_schema", "get_scheduler_status", "get_workflow", "list_nodes",
            "list_workflows", "run_workflow", "save_workflow", "start_schedule", "stop_schedule", "validate_workflow",
        ]
    );

    let hints = |name: &str| {
        let tool = first.iter().find(|t| t.name == name).expect("listed");
        let a = tool.annotations.as_ref().expect("annotated");
        (a.read_only_hint, a.destructive_hint)
    };
    for read in ["get_node_schema", "get_scheduler_status", "get_workflow", "list_nodes", "list_workflows", "validate_workflow"] {
        assert_eq!(hints(read).0, Some(true), "{read}");
    }
    for destructive in ["delete_workflow", "run_workflow", "save_workflow"] {
        assert_eq!(hints(destructive), (Some(false), Some(true)), "{destructive}");
    }
    for additive in ["start_schedule", "stop_schedule"] {
        assert_eq!(hints(additive), (Some(false), Some(false)), "{additive}");
    }

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn tools_call_returns_the_server_data_over_stdio() {
    let mock = serve(vec![reply("GET", "/api/workflows", 200, r#"{"items":[{"id":"a"}],"total":1,"limit":5,"offset":0}"#)]).await;
    let client = connect(&mock.base, false).await;

    let result = call(&client, "list_workflows", json!({"limit": 5})).await;
    assert_ne!(result["isError"], json!(true));
    let body: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["items"][0]["id"], "a");
    assert_eq!(mock.seen()[0].query, "limit=5");

    client.cancel().await.unwrap();
}

#[tokio::test]
async fn tool_failures_and_bad_arguments_are_is_error_results_and_unknown_tools_are_protocol_errors() {
    let mock = serve(vec![reply("GET", "/api/workflows/locked", 403, r#"{"error":"token ACL does not permit access to that workflow"}"#)]).await;
    let client = connect(&mock.base, false).await;

    let denied = call(&client, "get_workflow", json!({"id": "locked"})).await;
    assert_eq!(denied["isError"], json!(true));
    assert!(denied["content"][0]["text"].as_str().unwrap().contains("token ACL does not permit"));

    let gated = call(&client, "delete_workflow", json!({"id": "x"})).await;
    assert_eq!(gated["isError"], json!(true));
    assert!(gated["content"][0]["text"].as_str().unwrap().contains("--allow-write"));

    assert!(client.call_tool(CallToolRequestParams::new("no_such_tool")).await.is_err());
    let missing = client.call_tool(CallToolRequestParams::new("get_workflow")).await.expect("bad arguments come back as a tool result");
    let missing = serde_json::to_value(&missing).unwrap();
    assert_eq!(missing["isError"], json!(true));
    assert!(missing["content"][0]["text"].as_str().unwrap().contains("missing field `id`"));

    client.cancel().await.unwrap();
}

#[cfg(not(feature = "offline"))]
#[test]
fn a_build_without_offline_mode_refuses_to_start_without_a_server_url() {
    let out = std::process::Command::new(BIN)
        .env_remove("AERINI_URL")
        .env_remove("AERINI_TOKEN")
        .output()
        .expect("run aerini-mcp");
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no offline mode"));
    assert!(out.stdout.is_empty(), "stdout belongs to the MCP transport");
}
