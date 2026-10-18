//! Generated node reference.
//!
//! `docs/reference/node-schemas.json` is every built-in node descriptor, the same
//! data the app's node picker gets from `NodeRegistry::all_descriptors()`.
//! `docs/reference/node-catalog.md` is the same data as a readable page. Both are
//! written by this test when `AERINI_REGEN_DOCS` is set and compared with the
//! committed copies otherwise, so they cannot drift from the code.
//!
//! Regenerate: `AERINI_REGEN_DOCS=1 cargo test -p aerini-engine --features schema --test reference_docs --test workflow_schema`

mod common;

use aerini_engine::migration::CURRENT_VERSION;
use aerini_engine::node::{NodeDescriptor, NodePorts, NodeRegistry};
use aerini_engine::nodes::register_builtins;
use serde_json::{json, Value};

fn builtin_descriptors() -> Vec<NodeDescriptor> {
    let mut registry = NodeRegistry::new();
    register_builtins(&mut registry, &std::env::temp_dir(), None);
    registry.seal_builtins();
    let mut all = registry.all_descriptors();
    all.sort_by(|a, b| a.type_id.cmp(&b.type_id));
    all
}

fn node_schemas_json(nodes: &[NodeDescriptor]) -> String {
    let doc = json!({
        "schema_version": CURRENT_VERSION,
        "engine_version": env!("CARGO_PKG_VERSION"),
        "nodes": nodes,
    });
    let mut out = serde_json::to_string_pretty(&doc).expect("descriptors serialize");
    out.push('\n');
    out
}

/// Text for a Markdown table cell or paragraph. VitePress compiles every page as
/// a Vue template, so `{{` and raw angle brackets must not reach it unescaped.
fn esc(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('|', "\\|")
        .replace('`', "'")
        .replace("{{", "&#123;&#123;")
        .replace("}}", "&#125;&#125;")
        .replace('\n', " ")
}

/// Inline code. Code spans are not compiled by Vue, so the text stays literal.
fn code(text: &str) -> String {
    format!("`{}`", text.replace('|', "\\|").replace('`', "'").replace('\n', " "))
}

fn type_label(prop: &Value) -> String {
    match prop.get("type") {
        Some(Value::String(t)) => t.clone(),
        Some(Value::Array(types)) => types
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" or "),
        _ if prop.get("enum").is_some() => "enum".to_string(),
        _ => "any".to_string(),
    }
}

fn value_label(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn field_notes(prop: &Value) -> String {
    let mut notes = Vec::new();
    if let Some(text) = prop.get("description").and_then(Value::as_str) {
        if !text.is_empty() {
            notes.push(esc(text));
        }
    }
    if let Some(options) = prop.get("enum").and_then(Value::as_array) {
        let list: Vec<String> = options.iter().map(|o| code(&value_label(o))).collect();
        notes.push(format!("One of: {}.", list.join(", ")));
    }
    if let Some(default) = prop.get("default") {
        notes.push(format!("Default: {}.", code(&value_label(default))));
    }
    if prop.get("x-aerini-credential").is_some() {
        notes.push("Takes a saved credential.".to_string());
    }
    notes.join(" ")
}

fn ports_line(label: &str, ports: &[aerini_engine::node::PortDefinition]) -> String {
    if ports.is_empty() {
        return format!("{label}: none");
    }
    let list: Vec<String> = ports
        .iter()
        .map(|p| format!("{} ({})", code(&p.id), esc(&p.label)))
        .collect();
    format!("{label}: {}", list.join(", "))
}

fn describe_ports(ports: &NodePorts, dynamic: bool) -> String {
    let mut line = format!(
        "{}; {}",
        ports_line("In", &ports.inputs),
        ports_line("Out", &ports.outputs)
    );
    if dynamic {
        line.push_str(". The ports depend on the config, so these are the defaults");
    }
    line
}

fn category_key(node: &NodeDescriptor) -> String {
    serde_json::to_value(&node.node_type)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn category_title(node_type: &str) -> &str {
    match node_type {
        "action" => "Action nodes",
        "ai" => "AI nodes",
        "logic" => "Logic nodes",
        "utility" => "Utility nodes",
        other => other,
    }
}

fn node_catalog_md(nodes: &[NodeDescriptor]) -> String {
    let mut out = String::new();
    out.push_str("# Node Catalog\n\n");
    out.push_str(&format!(
        "Every built-in node, generated from the engine ({} nodes, engine {}, workflow schema version {}). The same data is in `docs/reference/node-schemas.json` as machine-readable JSON Schema. For what each node does in practice, gotchas and examples, see the [Nodes Reference](../guide/nodes.md).\n\n",
        nodes.len(),
        env!("CARGO_PKG_VERSION"),
        CURRENT_VERSION
    ));
    out.push_str("This page is generated. Do not edit it by hand; see [Regenerating the reference files](../development/testing.md#regenerating-the-reference-files).\n\n");
    out.push_str("**Required** means the node's schema requires the field. Some nodes check more when they run, for example a field that is only needed for one setting of another field, so a config that fits the schema can still fail.\n\n");

    for node_type in ["action", "ai", "logic", "utility"] {
        let group: Vec<&NodeDescriptor> =
            nodes.iter().filter(|n| category_key(n) == node_type).collect();
        if group.is_empty() {
            continue;
        }
        out.push_str(&format!("## {}\n\n", category_title(node_type)));
        for node in group {
            out.push_str(&format!("### {}\n\n", esc(&node.display_name)));
            out.push_str(&format!("{} version {}\n\n", code(&node.type_id), esc(&node.version)));
            if !node.description.is_empty() {
                out.push_str(&format!("{}\n\n", esc(&node.description)));
            }
            out.push_str(&format!("Ports. {}.\n\n", describe_ports(&node.ports, node.dynamic_ports)));

            let required: Vec<&str> = node
                .input_schema
                .get("required")
                .and_then(Value::as_array)
                .map(|r| r.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let mut fields: Vec<(&String, &Value)> = node
                .input_schema
                .get("properties")
                .and_then(Value::as_object)
                .map(|p| p.iter().collect())
                .unwrap_or_default();
            fields.sort_by_key(|(name, _)| (!required.contains(&name.as_str()), name.to_string()));

            if fields.is_empty() {
                out.push_str("No config fields.\n\n");
            } else {
                out.push_str("| Field | Type | Required | Notes |\n|---|---|---|---|\n");
                for (name, prop) in fields {
                    out.push_str(&format!(
                        "| {} | {} | {} | {} |\n",
                        code(name),
                        esc(&type_label(prop)),
                        if required.contains(&name.as_str()) { "Yes" } else { "No" },
                        field_notes(prop)
                    ));
                }
                out.push('\n');
            }

            if let Some(outputs) = node.output_schema.get("properties").and_then(Value::as_object) {
                if !outputs.is_empty() {
                    let names: Vec<String> = outputs.keys().map(|k| code(k)).collect();
                    out.push_str(&format!("Output fields: {}.\n\n", names.join(", ")));
                }
            }
        }
    }
    out
}

#[test]
fn node_schemas_json_matches_committed() {
    common::assert_committed("node-schemas.json", &node_schemas_json(&builtin_descriptors()));
}

#[test]
fn node_catalog_md_matches_committed() {
    common::assert_committed("node-catalog.md", &node_catalog_md(&builtin_descriptors()));
}

#[test]
fn node_catalog_text_is_safe_for_vitepress() {
    let catalog = node_catalog_md(&builtin_descriptors());
    // Odd-numbered pieces between backticks are code spans, which Vue does not compile.
    for (i, piece) in catalog.split('`').enumerate() {
        if i % 2 == 1 {
            continue;
        }
        assert!(!piece.contains("{{"), "unescaped {{{{ outside a code span: {piece}");
        assert!(!piece.contains('<'), "unescaped < outside a code span: {piece}");
    }
}
