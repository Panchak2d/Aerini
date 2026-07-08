//! P28 — E2E workflow integration tests.
//!
//! Each test builds a minimal Workflow from scratch, runs it through
//! WorkflowExecutor, and asserts on the WorkflowResult.  No network, DB, or
//! filesystem access is required — all nodes used here are pure in-process.
//!
//! Run with: `cargo test -p aerini-engine --test workflow_integration`

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use aerini_engine::executor::{CredentialResolver, WorkflowExecutor};
use aerini_engine::model::{
    CanvasPosition, NodeType, RetryPolicy, Workflow, WorkflowEdge, WorkflowNode,
};
use aerini_engine::node::NodeRegistry;
use aerini_engine::nodes::register_builtins;

// ── Helpers ───────────────────────────────────────────────────────────────────

struct NoopResolver;

#[async_trait]
impl CredentialResolver for NoopResolver {
    async fn resolve(&self, _id: &str) -> Option<String> {
        None
    }
}

fn make_registry() -> Arc<NodeRegistry> {
    let mut registry = NodeRegistry::new();
    let data_dir = std::env::temp_dir();
    register_builtins(&mut registry, &data_dir, None);
    registry.seal_builtins();
    Arc::new(registry)
}

fn make_executor() -> WorkflowExecutor {
    WorkflowExecutor::new(make_registry(), Arc::new(NoopResolver))
}

fn node(
    id: &str,
    type_id: &str,
    node_type: NodeType,
    config: Value,
) -> WorkflowNode {
    WorkflowNode {
        id: id.to_string(),
        node_type_id: type_id.to_string(),
        node_type,
        name: id.to_string(),
        config,
        credentials: HashMap::new(),
        input_schema: json!({}),
        output_schema: json!({}),
        retry: RetryPolicy::default(),
        fallback_node: None,
        disabled: false,
        position: CanvasPosition::default(),
    }
}

fn edge(id: &str, from: &str, from_port: &str, to: &str) -> WorkflowEdge {
    WorkflowEdge {
        id: id.to_string(),
        from_node: from.to_string(),
        from_port: from_port.to_string(),
        to_node: to.to_string(),
        to_port: "input".to_string(),
        condition: None,
        on_success: None,
        on_failure: None,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// ManualTrigger → Transform → Output
///
/// The trigger fires with `{"value": "hello"}`.  The transform maps `/value`
/// to `result`.  The output node wraps it.  Assert the chain runs to
/// completion and the output node's result contains the mapped field.
#[tokio::test]
async fn full_chain_trigger_transform_output() {
    let mut wf = Workflow::new("wf-chain", "Chain Test");

    wf.nodes = vec![
        node(
            "n_trigger",
            "manual_trigger",
            NodeType::Action,
            json!({ "mock_payload": "{\"value\": \"hello\"}" }),
        ),
        node(
            "n_transform",
            "transform",
            NodeType::Utility,
            json!({
                "source_node": "n_trigger",
                "mappings": [{ "from": "/value", "to": "result" }]
            }),
        ),
        node(
            "n_output",
            "output",
            NodeType::Utility,
            json!({ "source_node": "n_transform" }),
        ),
    ];

    wf.edges = vec![
        edge("e1", "n_trigger", "output", "n_transform"),
        edge("e2", "n_transform", "output", "n_output"),
    ];

    let result = make_executor()
        .run(Arc::new(wf), HashMap::new())
        .await
        .expect("run must not return Err");

    assert!(result.success, "workflow must succeed: {:?}", result.error);

    let transform_out = result
        .node_outputs
        .get("n_transform")
        .expect("n_transform must have output");
    assert_eq!(
        transform_out["result"],
        json!("hello"),
        "mapped field must carry through"
    );

    assert!(
        result.node_outputs.contains_key("n_output"),
        "output node must have executed"
    );
}

/// ManualTrigger → IfCondition (true branch) → Output
///
/// Condition "5 > 3" evaluates true.  Only the on_true successor must run;
/// the on_false branch node must not appear in node_outputs.
#[tokio::test]
async fn if_condition_true_branch() {
    let mut wf = Workflow::new("wf-if-true", "If True");

    wf.nodes = vec![
        node("n_trigger", "manual_trigger", NodeType::Action, json!({})),
        node(
            "n_if",
            "if_condition",
            NodeType::Logic,
            json!({ "condition": "5 > 3" }),
        ),
        node("n_true",  "output", NodeType::Utility, json!({})),
        node("n_false", "output", NodeType::Utility, json!({})),
    ];

    wf.edges = vec![
        edge("e1", "n_trigger", "output", "n_if"),
        WorkflowEdge {
            id:        "e2".to_string(),
            from_node: "n_if".to_string(),
            from_port: "on_true".to_string(),
            to_node:   "n_true".to_string(),
            to_port:   "input".to_string(),
            condition: None,
            on_success: None,
            on_failure: None,
        },
        WorkflowEdge {
            id:        "e3".to_string(),
            from_node: "n_if".to_string(),
            from_port: "on_false".to_string(),
            to_node:   "n_false".to_string(),
            to_port:   "input".to_string(),
            condition: None,
            on_success: None,
            on_failure: None,
        },
    ];

    let result = make_executor()
        .run(Arc::new(wf), HashMap::new())
        .await
        .expect("run must not return Err");

    assert!(result.success, "{:?}", result.error);
    assert!(
        result.node_outputs.contains_key("n_true"),
        "true branch must have executed"
    );
    assert!(
        !result.node_outputs.contains_key("n_false"),
        "false branch must not have executed"
    );
}

/// ManualTrigger → IfCondition (false branch) → Output
///
/// Condition "1 > 3" evaluates false.  Only the on_false successor must run.
#[tokio::test]
async fn if_condition_false_branch() {
    let mut wf = Workflow::new("wf-if-false", "If False");

    wf.nodes = vec![
        node("n_trigger", "manual_trigger", NodeType::Action, json!({})),
        node(
            "n_if",
            "if_condition",
            NodeType::Logic,
            json!({ "condition": "1 > 3" }),
        ),
        node("n_true",  "output", NodeType::Utility, json!({})),
        node("n_false", "output", NodeType::Utility, json!({})),
    ];

    wf.edges = vec![
        edge("e1", "n_trigger", "output", "n_if"),
        WorkflowEdge {
            id:        "e2".to_string(),
            from_node: "n_if".to_string(),
            from_port: "on_true".to_string(),
            to_node:   "n_true".to_string(),
            to_port:   "input".to_string(),
            condition: None,
            on_success: None,
            on_failure: None,
        },
        WorkflowEdge {
            id:        "e3".to_string(),
            from_node: "n_if".to_string(),
            from_port: "on_false".to_string(),
            to_node:   "n_false".to_string(),
            to_port:   "input".to_string(),
            condition: None,
            on_success: None,
            on_failure: None,
        },
    ];

    let result = make_executor()
        .run(Arc::new(wf), HashMap::new())
        .await
        .expect("run must not return Err");

    assert!(result.success, "{:?}", result.error);
    assert!(
        result.node_outputs.contains_key("n_false"),
        "false branch must have executed"
    );
    assert!(
        !result.node_outputs.contains_key("n_true"),
        "true branch must not have executed"
    );
}

/// ManualTrigger → Loop (3-item array) → [body: nothing] → [done: Output]
///
/// The trigger emits `{"items": [1, 2, 3]}`.  The loop iterates all three
/// items.  The done-port output must report `total == 3` and
/// `all_results.len() == 0` (no body nodes collect results).
#[tokio::test]
async fn loop_correct_iterations() {
    let mut wf = Workflow::new("wf-loop", "Loop Test");

    // Trigger emits the array the loop will iterate.
    wf.nodes = vec![
        node(
            "n_trigger",
            "manual_trigger",
            NodeType::Action,
            json!({ "mock_payload": "{\"items\": [1, 2, 3]}" }),
        ),
        node(
            "n_loop",
            "loop",
            NodeType::Logic,
            json!({
                "source_node": "n_trigger",
                "array_field": "items"
            }),
        ),
        node("n_done", "output", NodeType::Utility, json!({})),
    ];

    wf.edges = vec![
        edge("e1", "n_trigger", "output", "n_loop"),
        WorkflowEdge {
            id:        "e2".to_string(),
            from_node: "n_loop".to_string(),
            from_port: "done".to_string(),
            to_node:   "n_done".to_string(),
            to_port:   "input".to_string(),
            condition: None,
            on_success: None,
            on_failure: None,
        },
    ];

    let result = make_executor()
        .run(Arc::new(wf), HashMap::new())
        .await
        .expect("run must not return Err");

    assert!(result.success, "{:?}", result.error);

    let loop_out = result
        .node_outputs
        .get("n_loop")
        .expect("loop node must have output");

    assert_eq!(
        loop_out["total"],
        json!(3),
        "loop must report total == 3"
    );
    assert_eq!(
        loop_out["done"],
        json!(true),
        "loop must be done after 3 items"
    );

    let all_results = loop_out["all_results"]
        .as_array()
        .expect("all_results must be an array");
    // No body nodes → no results collected, but the array must exist.
    assert_eq!(
        all_results.len(),
        0,
        "no body nodes → all_results must be empty"
    );
}

/// Workflow with a cycle (A → B → A) must return Err(CycleDetected) before
/// any node executes.  The executor must not spin forever.
#[tokio::test]
async fn cycle_detection() {
    use aerini_engine::error::EngineError;

    let mut wf = Workflow::new("wf-cycle", "Cycle Test");

    wf.nodes = vec![
        node("n_a", "manual_trigger", NodeType::Action, json!({})),
        node("n_b", "output",         NodeType::Utility, json!({})),
    ];

    // A → B and B → A form the cycle.
    wf.edges = vec![
        edge("e1", "n_a", "output", "n_b"),
        edge("e2", "n_b", "output", "n_a"),
    ];

    let err = make_executor()
        .run(Arc::new(wf), HashMap::new())
        .await
        .expect_err("a cyclic workflow must return Err");

    assert!(
        matches!(err, EngineError::CycleDetected(_)),
        "expected CycleDetected, got: {:?}",
        err
    );
}

/// A workflow containing a node with no incoming edges other than the trigger
/// (i.e. a second, isolated trigger node) must complete without panicking.
/// The isolated node executes as an independent entry point; it does not block
/// the main chain.
#[tokio::test]
async fn disconnected_node() {
    let mut wf = Workflow::new("wf-disconnected", "Disconnected Node Test");

    wf.nodes = vec![
        // Main chain.
        node("n_trigger", "manual_trigger", NodeType::Action, json!({})),
        node("n_output",  "output",         NodeType::Utility, json!({})),
        // Isolated trigger — no edges to or from it.
        node("n_orphan", "manual_trigger", NodeType::Action, json!({})),
    ];

    wf.edges = vec![
        edge("e1", "n_trigger", "output", "n_output"),
        // n_orphan has no edges.
    ];

    // Must not panic, must not return Err.
    let result = make_executor()
        .run(Arc::new(wf), HashMap::new())
        .await
        .expect("run must not return Err for a disconnected (orphan) node");

    assert!(result.success, "{:?}", result.error);

    // Main chain must have executed.
    assert!(
        result.node_outputs.contains_key("n_output"),
        "main chain output must have executed"
    );
    // Orphan must also have executed (it is an entry point).
    assert!(
        result.node_outputs.contains_key("n_orphan"),
        "orphan node must have executed as an isolated entry point"
    );
}
