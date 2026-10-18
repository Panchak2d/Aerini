//! Generated workflow file schema, `docs/reference/workflow.schema.json`.
//!
//! The schema comes from the `Workflow` types in `model.rs` (the `schema` feature),
//! so it follows their serde attributes. These tests compare it with the committed
//! copy, check that every shipped example workflow passes it, and check that it
//! accepts and rejects the same documents as `Workflow::from_json` on the cases
//! where the two can be compared.
//!
//! Regenerate: `AERINI_REGEN_DOCS=1 cargo test -p aerini-engine --features schema --test reference_docs --test workflow_schema`

#![cfg(feature = "schema")]

mod common;

use aerini_engine::model::Workflow;
use serde_json::{json, Value};
use std::io::Read;
use std::path::PathBuf;

/// `WorkflowMetadata::default()` stamps the current time, so the default schemars
/// records for the `metadata` field would change on every run. Drop any default
/// object that carries a timestamp.
fn strip_clock_defaults(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if map
                .get("default")
                .and_then(Value::as_object)
                .is_some_and(|d| d.contains_key("created_at"))
            {
                map.remove("default");
            }
            map.values_mut().for_each(strip_clock_defaults);
        }
        Value::Array(items) => items.iter_mut().for_each(strip_clock_defaults),
        _ => {}
    }
}

fn workflow_schema() -> Value {
    let mut schema =
        serde_json::to_value(schemars::schema_for!(Workflow)).expect("schema serializes");
    strip_clock_defaults(&mut schema);
    schema["x-aerini-engine-version"] = json!(env!("CARGO_PKG_VERSION"));
    schema
}

fn workflow_schema_json() -> String {
    let mut out = serde_json::to_string_pretty(&workflow_schema()).expect("schema serializes");
    out.push('\n');
    out
}

fn validator() -> jsonschema::Validator {
    jsonschema::options()
        .build(&workflow_schema())
        .expect("generated schema compiles")
}

fn schema_accepts(validator: &jsonschema::Validator, instance: &Value) -> bool {
    validator.iter_errors(instance).next().is_none()
}

/// Every workflow file shipped under `examples/`: loose `.aerini` / `.json` files,
/// and the `workflow_json` inside the `aerini-server.json` of each `.zip` package.
fn shipped_example_workflows() -> Vec<(String, String)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("examples");
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("examples directory exists") {
        let path = entry.expect("readable directory entry").path();
        let label = path.file_name().expect("file name").to_string_lossy().into_owned();
        match path.extension().and_then(|e| e.to_str()) {
            Some("aerini") | Some("json") => {
                found.push((label, std::fs::read_to_string(&path).expect("readable example")));
            }
            Some("zip") => {
                let file = std::fs::File::open(&path).expect("readable example package");
                let mut archive = zip::ZipArchive::new(file).expect("valid zip");
                let Ok(mut export) = archive.by_name("aerini-server.json") else {
                    continue;
                };
                let mut text = String::new();
                export.read_to_string(&mut text).expect("utf-8 export");
                let export: Value = serde_json::from_str(&text).expect("export is JSON");
                let workflow = export["workflow_json"]
                    .as_str()
                    .expect("export has a workflow_json string");
                found.push((format!("{label}!aerini-server.json"), workflow.to_string()));
            }
            _ => {}
        }
    }
    found
}

#[test]
fn workflow_schema_matches_committed() {
    common::assert_committed("workflow.schema.json", &workflow_schema_json());
}

#[test]
fn shipped_example_workflows_satisfy_the_schema_and_parse() {
    let validator = validator();
    let examples = shipped_example_workflows();
    assert!(!examples.is_empty(), "no example workflow found under examples/");

    for (label, text) in examples {
        let instance: Value = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("{label} is not JSON: {e}"));
        let errors: Vec<String> = validator
            .iter_errors(&instance)
            .map(|e| format!("{} (at {})", e, e.instance_path()))
            .collect();
        assert!(errors.is_empty(), "{label} violates the schema: {errors:?}");
        Workflow::from_json(&text).unwrap_or_else(|e| panic!("{label} does not parse: {e}"));
    }
}

#[test]
fn schema_and_from_json_agree() {
    let validator = validator();

    let node = json!({
        "id": "n1", "node_type_id": "http_request", "node_type": "action", "name": "Fetch",
        "input_schema": {}, "output_schema": {}
    });
    let edge = json!({
        "id": "e1", "from_node": "n1", "from_port": "output", "to_node": "n2", "to_port": "input"
    });
    let base = json!({ "id": "wf", "name": "Test", "nodes": [], "edges": [] });

    let with = |key: &str, value: Value| {
        let mut doc = base.clone();
        doc[key] = value;
        doc
    };
    let without = |key: &str| {
        let mut doc = base.clone();
        doc.as_object_mut().expect("object").remove(key);
        doc
    };
    let mut node_missing_input_schema = node.clone();
    node_missing_input_schema.as_object_mut().expect("object").remove("input_schema");
    let mut node_bad_category = node.clone();
    node_bad_category["node_type"] = json!("bogus");
    let mut edge_missing_port = edge.clone();
    edge_missing_port.as_object_mut().expect("object").remove("to_port");

    let cases: Vec<(&str, Value, bool)> = vec![
        ("minimal workflow", base.clone(), true),
        ("workflow with a node and an edge", {
            let mut doc = with("nodes", json!([node.clone()]));
            doc["edges"] = json!([edge.clone()]);
            doc
        }, true),
        ("unknown top-level field", with("future_field", json!(1)), true),
        ("missing name", without("name"), false),
        ("missing nodes", without("nodes"), false),
        ("missing edges", without("edges"), false),
        ("node without input_schema", with("nodes", json!([node_missing_input_schema])), false),
        ("node with an unknown category", with("nodes", json!([node_bad_category])), false),
        ("edge without to_port", with("edges", json!([edge_missing_port])), false),
        ("metadata without author", with("metadata", json!({
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z", "version": "1.0.0"
        })), false),
        ("negative max_duration_secs", with("max_duration_secs", json!(-5)), false),
        ("fractional max_duration_secs", with("max_duration_secs", json!(1.5)), false),
        ("null max_duration_secs", with("max_duration_secs", Value::Null), true),
        ("text for a boolean field", with("parallel_execution", json!("yes")), false),
    ];

    for (name, doc, expected) in cases {
        let by_schema = schema_accepts(&validator, &doc);
        let by_parser = Workflow::from_json(&doc.to_string()).is_ok();
        assert_eq!(by_parser, expected, "{name}: Workflow::from_json disagrees with the expected result");
        assert_eq!(by_schema, expected, "{name}: the schema disagrees with Workflow::from_json");
    }
}
