//! Best-effort static validation of a workflow, without running it.
//!
//! [`validate_workflow`] reports what the engine would reject at the start of
//! a run (unknown node types, dangling or cyclic edges, port-arity violations)
//! plus a few authoring mistakes that only show up later as silent
//! misbehaviour (an edge naming a port the node does not have, a required
//! config field that is empty). It never executes anything and never reads
//! credentials or config values into its output — only field names.
//!
//! "Valid" here means *no known structural problem*, not *will run
//! successfully*: node config is not type-checked against the node's schema
//! (values are often `{{expressions}}` or supplied by an upstream node), and
//! per-node runtime rules such as the database node's `db_type`-dependent
//! fields are not repeated here.

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use serde_json::Value;

use crate::graph::{validate_port_arity, ExecutionGraph};
use crate::model::{Workflow, WorkflowNode};
use crate::node::{Node, NodePorts, NodeRegistry};

/// Built-in node types that start a workflow. A trigger-capable plugin also
/// counts; see [`Node::is_trigger_capable`].
pub const TRIGGER_NODE_TYPE_IDS: &[&str] = &["manual_trigger", "schedule", "webhook"];

pub const CHECK_PARSE: &str = "parse";
pub const CHECK_DUPLICATE_IDS: &str = "duplicate_ids";
pub const CHECK_NODE_TYPES: &str = "node_types";
pub const CHECK_GRAPH: &str = "graph";
pub const CHECK_PORT_ARITY: &str = "port_arity";
pub const CHECK_PORTS: &str = "ports";
pub const CHECK_TRIGGERS: &str = "triggers";
pub const CHECK_REQUIRED_CONFIG: &str = "required_config";

/// Every check run on a successfully parsed workflow, in report order.
const STRUCTURAL_CHECKS: &[&str] = &[
    CHECK_DUPLICATE_IDS,
    CHECK_NODE_TYPES,
    CHECK_GRAPH,
    CHECK_PORT_ARITY,
    CHECK_PORTS,
    CHECK_TRIGGERS,
    CHECK_REQUIRED_CONFIG,
];

/// One finding. `check` names the check class that produced it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ValidationIssue {
    pub check: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edge_id: Option<String>,
}

impl ValidationIssue {
    fn new(check: &'static str, message: impl Into<String>) -> Self {
        Self { check, message: message.into(), node_id: None, edge_id: None }
    }

    fn at_node(mut self, node_id: &str) -> Self {
        self.node_id = Some(node_id.to_string());
        self
    }

    fn at_edge(mut self, edge_id: &str) -> Self {
        self.edge_id = Some(edge_id.to_string());
        self
    }
}

/// Outcome of a validation pass. `errors` are problems the engine rejects or
/// that make the workflow malformed; `warnings` are likely mistakes. `valid`
/// is true exactly when `errors` is empty. `best_effort` is always true: see
/// the module docs for what a passing report does not promise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ValidationReport {
    pub valid: bool,
    pub best_effort: bool,
    pub errors: Vec<ValidationIssue>,
    pub warnings: Vec<ValidationIssue>,
    pub checks_run: Vec<&'static str>,
}

impl ValidationReport {
    fn new(checks_run: Vec<&'static str>, errors: Vec<ValidationIssue>, warnings: Vec<ValidationIssue>) -> Self {
        Self { valid: errors.is_empty(), best_effort: true, errors, warnings, checks_run }
    }
}

/// Parses `json` (applying schema migrations, as saving does) and validates
/// the result. A document that does not parse yields a report with a single
/// `parse` error and only that check run.
pub fn validate_workflow_json(json: &str, registry: &NodeRegistry) -> ValidationReport {
    match Workflow::from_json(json) {
        Ok(workflow) => validate_workflow(&workflow, registry),
        Err(message) => ValidationReport::new(
            vec![CHECK_PARSE],
            vec![ValidationIssue::new(CHECK_PARSE, message)],
            Vec::new(),
        ),
    }
}

/// Validates an already-parsed workflow against `registry`. The checks are
/// independent: one failing does not stop the others, so a single call lists
/// every class of problem. The `graph` and `port_arity` checks stop at the
/// first violation of their kind; `node_types`, `ports`, `triggers` and
/// `required_config` report every occurrence.
pub fn validate_workflow(workflow: &Workflow, registry: &NodeRegistry) -> ValidationReport {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    check_duplicate_ids(workflow, &mut errors, &mut warnings);
    check_node_types(workflow, registry, &mut errors, &mut warnings);

    if let Err(e) = ExecutionGraph::build(workflow) {
        errors.push(ValidationIssue::new(CHECK_GRAPH, e.to_string()));
    }
    if let Err(e) = validate_port_arity(workflow, registry) {
        errors.push(ValidationIssue::new(CHECK_PORT_ARITY, e.to_string()));
    }

    check_edge_ports(workflow, registry, &mut warnings);
    check_triggers(workflow, registry, &mut warnings);
    check_required_config(workflow, registry, &mut warnings);

    ValidationReport::new(STRUCTURAL_CHECKS.to_vec(), errors, warnings)
}

fn check_duplicate_ids(
    workflow: &Workflow,
    errors: &mut Vec<ValidationIssue>,
    warnings: &mut Vec<ValidationIssue>,
) {
    let mut seen_nodes: HashSet<&str> = HashSet::new();
    let mut reported_nodes: HashSet<&str> = HashSet::new();
    for node in &workflow.nodes {
        if !seen_nodes.insert(node.id.as_str()) && reported_nodes.insert(node.id.as_str()) {
            errors.push(
                ValidationIssue::new(
                    CHECK_DUPLICATE_IDS,
                    format!("Node id '{}' is used by more than one node; ids must be unique", node.id),
                )
                .at_node(&node.id),
            );
        }
    }

    let mut seen_edges: HashSet<&str> = HashSet::new();
    let mut reported_edges: HashSet<&str> = HashSet::new();
    for edge in &workflow.edges {
        if !seen_edges.insert(edge.id.as_str()) && reported_edges.insert(edge.id.as_str()) {
            warnings.push(
                ValidationIssue::new(
                    CHECK_DUPLICATE_IDS,
                    format!("Edge id '{}' is used by more than one edge; ids should be unique", edge.id),
                )
                .at_edge(&edge.id),
            );
        }
    }
}

/// An unregistered type fails the run when the node is reached; on a disabled
/// node it is only a warning, since the node is skipped.
fn check_node_types(
    workflow: &Workflow,
    registry: &NodeRegistry,
    errors: &mut Vec<ValidationIssue>,
    warnings: &mut Vec<ValidationIssue>,
) {
    for node in &workflow.nodes {
        if registry.get(&node.node_type_id).is_some() {
            continue;
        }
        let issue = ValidationIssue::new(
            CHECK_NODE_TYPES,
            format!(
                "Node '{}' uses node_type_id '{}', which is not registered on this server",
                node.id, node.node_type_id
            ),
        )
        .at_node(&node.id);
        if node.disabled {
            warnings.push(issue);
        } else {
            errors.push(issue);
        }
    }
}

/// Ports a node exposes for this particular config: derived from the config
/// for dynamic-port nodes, static otherwise.
fn effective_ports(node: &dyn Node, config: &Value) -> NodePorts {
    if node.is_dynamic_ports() {
        node.ports_from_config(config).unwrap_or_else(|| node.ports())
    } else {
        node.ports()
    }
}

/// Ports of `def`, worked out once per node however many edges touch it.
/// `None` when the node's type is not registered.
fn cached_ports<'c>(
    cache:    &'c mut HashMap<String, Option<NodePorts>>,
    registry: &NodeRegistry,
    def:      &WorkflowNode,
) -> Option<&'c NodePorts> {
    cache
        .entry(def.id.clone())
        .or_insert_with(|| {
            registry
                .get(&def.node_type_id)
                .map(|node| effective_ports(&*node, &def.config))
        })
        .as_ref()
}

/// The executor routes by `from_port` alone, so an edge naming a port the
/// node does not declare silently carries nothing. Edges whose endpoints or
/// node types are unknown are skipped here; other checks report those.
fn check_edge_ports(workflow: &Workflow, registry: &NodeRegistry, warnings: &mut Vec<ValidationIssue>) {
    let nodes: HashMap<&str, &WorkflowNode> =
        workflow.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut cache: HashMap<String, Option<NodePorts>> = HashMap::new();

    for edge in &workflow.edges {
        if let Some(def) = nodes.get(edge.from_node.as_str()) {
            if let Some(ports) = cached_ports(&mut cache, registry, def) {
                if !ports.outputs.iter().any(|p| p.id == edge.from_port) {
                    warnings.push(
                        ValidationIssue::new(
                            CHECK_PORTS,
                            format!(
                                "Edge '{}' leaves node '{}' from port '{}', but a '{}' node has output ports: {}",
                                edge.id,
                                edge.from_node,
                                edge.from_port,
                                def.node_type_id,
                                port_id_list(&ports.outputs),
                            ),
                        )
                        .at_edge(&edge.id)
                        .at_node(&edge.from_node),
                    );
                }
            }
        }
        if let Some(def) = nodes.get(edge.to_node.as_str()) {
            if let Some(ports) = cached_ports(&mut cache, registry, def) {
                if !ports.inputs.iter().any(|p| p.id == edge.to_port) {
                    warnings.push(
                        ValidationIssue::new(
                            CHECK_PORTS,
                            format!(
                                "Edge '{}' enters node '{}' at port '{}', but a '{}' node has input ports: {}",
                                edge.id,
                                edge.to_node,
                                edge.to_port,
                                def.node_type_id,
                                port_id_list(&ports.inputs),
                            ),
                        )
                        .at_edge(&edge.id)
                        .at_node(&edge.to_node),
                    );
                }
            }
        }
    }
}

fn port_id_list(ports: &[crate::node::PortDefinition]) -> String {
    if ports.is_empty() {
        return "none".to_string();
    }
    ports.iter().map(|p| format!("'{}'", p.id)).collect::<Vec<_>>().join(", ")
}

/// A workflow with no enabled trigger can still be run on demand, but it
/// cannot start itself from a schedule or an incoming webhook.
fn check_triggers(workflow: &Workflow, registry: &NodeRegistry, warnings: &mut Vec<ValidationIssue>) {
    if workflow.nodes.is_empty() {
        return;
    }
    let has_trigger = workflow.nodes.iter().filter(|n| !n.disabled).any(|n| {
        TRIGGER_NODE_TYPE_IDS.contains(&n.node_type_id.as_str())
            || registry.get(&n.node_type_id).is_some_and(|node| node.is_trigger_capable())
    });
    if !has_trigger {
        warnings.push(ValidationIssue::new(
            CHECK_TRIGGERS,
            "No enabled trigger node (manual_trigger, schedule, webhook, or a trigger plugin); \
             the workflow can only be run on demand",
        ));
    }
}

fn is_blank(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => s.trim().is_empty(),
        Some(_) => false,
    }
}

/// Top-level fields the node type's schema lists under `required` that are
/// blank in the node's config and not filled from a credential. Only a
/// warning: an upstream node may supply the value when the workflow runs.
/// Field names only — values are never read into the message.
fn check_required_config(workflow: &Workflow, registry: &NodeRegistry, warnings: &mut Vec<ValidationIssue>) {
    let mut required_by_type: HashMap<&str, Option<Vec<String>>> = HashMap::new();

    for def in workflow.nodes.iter().filter(|n| !n.disabled) {
        let required = required_by_type
            .entry(def.node_type_id.as_str())
            .or_insert_with(|| {
                let node = registry.get(&def.node_type_id)?;
                let schema = node.input_schema();
                let fields = schema.get("required")?.as_array()?;
                let names: Vec<String> = fields.iter().filter_map(Value::as_str).map(str::to_string).collect();
                Some(names)
            });
        let Some(required) = required else { continue };

        for field in required.iter() {
            if def.credentials.contains_key(field) {
                continue;
            }
            if is_blank(def.config.get(field.as_str())) {
                warnings.push(
                    ValidationIssue::new(
                        CHECK_REQUIRED_CONFIG,
                        format!(
                            "Node '{}' ({}): required field '{}' is not set in its config \
                             (fine if an upstream node supplies it at run time)",
                            def.id, def.node_type_id, field
                        ),
                    )
                    .at_node(&def.id),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NodeInput, NodeOutput, NodeType};
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Arc;

    struct FakeNode {
        id: &'static str,
        schema: Value,
        trigger: bool,
    }

    #[async_trait]
    impl Node for FakeNode {
        fn type_id(&self) -> &'static str { self.id }
        fn display_name(&self) -> &'static str { self.id }
        fn node_type(&self) -> NodeType { NodeType::Utility }
        fn version(&self) -> &'static str { "0.0.0" }
        fn input_schema(&self) -> Value { self.schema.clone() }
        fn output_schema(&self) -> Value { Value::Null }
        fn is_trigger_capable(&self) -> bool { self.trigger }
        async fn execute(&self, _input: NodeInput) -> NodeOutput {
            unimplemented!("validation never executes nodes")
        }
    }

    fn registry() -> NodeRegistry {
        let mut reg = NodeRegistry::new();
        reg.register(Arc::new(FakeNode { id: "manual_trigger", schema: Value::Null, trigger: false }));
        reg.register(Arc::new(FakeNode {
            id: "needs_url",
            schema: json!({ "properties": { "url": {} }, "required": ["url"] }),
            trigger: false,
        }));
        reg.register(Arc::new(FakeNode { id: "plain", schema: Value::Null, trigger: false }));
        reg
    }

    fn node(id: &str, type_id: &str) -> Value {
        json!({
            "id": id, "node_type_id": type_id, "node_type": "utility", "name": id,
            "config": {}, "input_schema": {}, "output_schema": {}
        })
    }

    fn edge(id: &str, from: &str, to: &str) -> Value {
        json!({ "id": id, "from_node": from, "from_port": "output", "to_node": to, "to_port": "input" })
    }

    fn workflow(nodes: Vec<Value>, edges: Vec<Value>) -> String {
        json!({ "id": "wf", "name": "wf", "nodes": nodes, "edges": edges }).to_string()
    }

    fn checks(issues: &[ValidationIssue]) -> Vec<&'static str> {
        issues.iter().map(|i| i.check).collect()
    }

    #[test]
    fn clean_workflow_is_valid_with_no_findings() {
        let json = workflow(
            vec![node("t", "manual_trigger"), node("p", "plain")],
            vec![edge("e1", "t", "p")],
        );
        let report = validate_workflow_json(&json, &registry());
        assert!(report.valid, "{report:?}");
        assert!(report.errors.is_empty() && report.warnings.is_empty(), "{report:?}");
        assert_eq!(report.checks_run, STRUCTURAL_CHECKS);
        assert!(report.best_effort);
    }

    #[test]
    fn unparseable_json_reports_only_the_parse_check() {
        let report = validate_workflow_json("{not json", &registry());
        assert!(!report.valid);
        assert_eq!(report.checks_run, vec![CHECK_PARSE]);
        assert_eq!(checks(&report.errors), vec![CHECK_PARSE]);
    }

    #[test]
    fn unknown_node_type_is_an_error_naming_the_node() {
        let json = workflow(vec![node("t", "manual_trigger"), node("x", "no_such_type")], vec![edge("e1", "t", "x")]);
        let report = validate_workflow_json(&json, &registry());
        assert!(!report.valid);
        let issue = report.errors.iter().find(|i| i.check == CHECK_NODE_TYPES).expect("node_types error");
        assert_eq!(issue.node_id.as_deref(), Some("x"));
    }

    #[test]
    fn unknown_node_type_on_a_disabled_node_is_only_a_warning() {
        let mut ghost = node("x", "no_such_type");
        ghost["disabled"] = json!(true);
        let json = workflow(vec![node("t", "manual_trigger"), ghost], vec![edge("e1", "t", "x")]);
        let report = validate_workflow_json(&json, &registry());
        assert!(report.valid, "{report:?}");
        assert_eq!(checks(&report.warnings), vec![CHECK_NODE_TYPES]);
    }

    #[test]
    fn dangling_edge_and_cycle_are_graph_errors() {
        let dangling = workflow(vec![node("t", "manual_trigger")], vec![edge("e1", "t", "missing")]);
        let report = validate_workflow_json(&dangling, &registry());
        assert!(checks(&report.errors).contains(&CHECK_GRAPH), "{report:?}");

        let cyclic = workflow(
            vec![node("t", "manual_trigger"), node("a", "plain"), node("b", "plain")],
            vec![edge("e1", "t", "a"), edge("e2", "a", "b"), edge("e3", "b", "a")],
        );
        let report = validate_workflow_json(&cyclic, &registry());
        assert!(checks(&report.errors).contains(&CHECK_GRAPH), "{report:?}");
    }

    #[test]
    fn two_wires_into_a_single_arity_port_is_a_port_arity_error() {
        let json = workflow(
            vec![node("a", "manual_trigger"), node("b", "manual_trigger"), node("c", "plain")],
            vec![edge("e1", "a", "c"), edge("e2", "b", "c")],
        );
        let report = validate_workflow_json(&json, &registry());
        assert!(checks(&report.errors).contains(&CHECK_PORT_ARITY), "{report:?}");
    }

    #[test]
    fn duplicate_node_id_is_an_error_reported_once() {
        let json = workflow(
            vec![node("t", "manual_trigger"), node("t", "plain"), node("t", "plain")],
            vec![],
        );
        let report = validate_workflow_json(&json, &registry());
        let dupes: Vec<_> = report.errors.iter().filter(|i| i.check == CHECK_DUPLICATE_IDS).collect();
        assert_eq!(dupes.len(), 1, "{report:?}");
    }

    #[test]
    fn edge_naming_a_missing_port_warns_and_lists_the_valid_ones() {
        let mut bad = edge("e1", "t", "p");
        bad["from_port"] = json!("nope");
        let json = workflow(vec![node("t", "manual_trigger"), node("p", "plain")], vec![bad]);
        let report = validate_workflow_json(&json, &registry());
        assert!(report.valid, "a bad port is a warning, not an error: {report:?}");
        let issue = report.warnings.iter().find(|i| i.check == CHECK_PORTS).expect("ports warning");
        assert!(issue.message.contains("'output'"), "{}", issue.message);
        assert_eq!(issue.edge_id.as_deref(), Some("e1"));
    }

    #[test]
    fn workflow_without_an_enabled_trigger_warns() {
        let json = workflow(vec![node("p", "plain")], vec![]);
        let report = validate_workflow_json(&json, &registry());
        assert!(report.valid, "{report:?}");
        assert_eq!(checks(&report.warnings), vec![CHECK_TRIGGERS]);

        let mut off = node("t", "manual_trigger");
        off["disabled"] = json!(true);
        let json = workflow(vec![off, node("p", "plain")], vec![edge("e1", "t", "p")]);
        assert!(checks(&validate_workflow_json(&json, &registry()).warnings).contains(&CHECK_TRIGGERS));
    }

    #[test]
    fn trigger_capable_plugin_satisfies_the_trigger_check() {
        let mut reg = registry();
        reg.register(Arc::new(FakeNode { id: "my_trigger", schema: Value::Null, trigger: true }));
        let json = workflow(vec![node("t", "my_trigger")], vec![]);
        assert!(validate_workflow_json(&json, &reg).warnings.is_empty());
    }

    #[test]
    fn blank_required_field_warns_by_name_without_echoing_values() {
        let mut n = node("n", "needs_url");
        n["config"] = json!({ "url": "   ", "other": "secret-value" });
        let json = workflow(vec![node("t", "manual_trigger"), n], vec![edge("e1", "t", "n")]);
        let report = validate_workflow_json(&json, &registry());
        assert!(report.valid, "{report:?}");
        let issue = report.warnings.iter().find(|i| i.check == CHECK_REQUIRED_CONFIG).expect("required warning");
        assert!(issue.message.contains("'url'"), "{}", issue.message);
        assert!(!issue.message.contains("secret-value"));
    }

    #[test]
    fn required_field_set_or_filled_by_credential_does_not_warn() {
        let mut set = node("n", "needs_url");
        set["config"] = json!({ "url": "https://example.com" });
        let json = workflow(vec![node("t", "manual_trigger"), set], vec![edge("e1", "t", "n")]);
        assert!(validate_workflow_json(&json, &registry()).warnings.is_empty());

        let mut cred = node("n", "needs_url");
        cred["credentials"] = json!({ "url": "my_cred" });
        let json = workflow(vec![node("t", "manual_trigger"), cred], vec![edge("e1", "t", "n")]);
        assert!(validate_workflow_json(&json, &registry()).warnings.is_empty());
    }

    #[test]
    fn report_serializes_without_empty_location_fields() {
        let report = validate_workflow_json("{", &registry());
        let v = serde_json::to_value(&report).unwrap();
        assert_eq!(v["valid"], json!(false));
        assert_eq!(v["errors"][0]["check"], json!("parse"));
        assert!(v["errors"][0].get("node_id").is_none());
        assert!(v["errors"][0].get("edge_id").is_none());
    }
}
