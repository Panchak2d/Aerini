//! `Node` trait and registry.
//!
//! Every node type in the canvas implements [`Node`]. The trait is object-safe via
//! `async_trait` — the executor holds `Arc<dyn Node>` and dispatches through it.
//!
//! # Implementing a node
//!
//! 1. Implement [`Node`] on a unit struct (or a struct holding shared state).
//! 2. Call `registry.register(Arc::new(YourNode))` in [`crate::nodes::register_builtins`].
//! 3. Add the icon entry to `src/utils.ts` → `NODE_ICONS`.
//!
//! That is the complete surface — no other files require changes.
//!
//! # Port layout
//!
//! [`NodePorts`] describes which connectors a node exposes. The default is one input
//! (`"input"`, left) and one output (`"output"`, right). Nodes with conditional routing
//! (e.g. `if_condition`, `switch`) override `ports()` to add named output ports.
//!
//! # Dynamic ports
//!
//! Nodes whose port count changes based on user config (e.g. `save_to_folder`,
//! `collect_files`) override `is_dynamic_ports()` to return `true` and implement
//! `ports_from_config()`. The frontend uses these to rebuild ports live as config
//! changes. All existing nodes use the default static `ports()` path.

use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

use crate::model::{NodeInput, NodeOutput, NodeType};

// ── Node trait ────────────────────────────────────────────────────────────────

/// A node type that can be registered and executed by the workflow engine.
///
/// Implement this trait on a unit struct (or a struct holding shared read-only state).
/// The executor holds all registered nodes behind `Arc<dyn Node>` and dispatches through
/// them at run time — implementations must be `Send + Sync`.
///
/// The trait is object-safe; do not add non-object-safe methods.
#[async_trait]
pub trait Node: Send + Sync {
    /// Matches `WorkflowNode.node_type_id` (e.g. `"http_request"`, `"shell_exec"`).
    /// Must be globally unique across all registered nodes.
    fn type_id(&self) -> &'static str;

    /// Human-readable label shown in the node palette and as the canvas block title.
    fn display_name(&self) -> &'static str;

    /// Palette category — determines which section of the node picker this node appears in.
    fn node_type(&self) -> NodeType;

    /// Semver version string for this implementation (e.g. `"1.0.0"`).
    /// Stored in [`NodeDescriptor`] and included in run history for future migration tooling.
    fn version(&self) -> &'static str;

    /// JSON Schema (Draft 7) describing the config fields shown in the node's config panel.
    /// Return `serde_json::Value::Null` or `json!({})` to declare no schema constraints.
    fn input_schema(&self) -> Value;

    /// JSON Schema (Draft 7) describing the shape of the value written to
    /// [`crate::model::NodeOutput::output`] on success.
    /// Return `serde_json::Value::Null` or `json!({})` to declare no schema constraints.
    fn output_schema(&self) -> Value;

    /// Port definitions — what connectors this node exposes.
    /// Defaults to one input port "input" and one output port "output".
    fn ports(&self) -> NodePorts {
        NodePorts::default()
    }

    /// Returns true when this node's ports are derived from its config at
    /// canvas load time rather than being static. Override to return true
    /// and implement `ports_from_config` for config-driven port counts.
    /// All existing nodes use the default (false).
    fn is_dynamic_ports(&self) -> bool {
        false
    }

    /// Returns ports derived from the given node config, or `None` for
    /// static-port nodes (default). Only called when `is_dynamic_ports()`
    /// returns true.
    fn ports_from_config(&self, _config: &Value) -> Option<NodePorts> {
        None
    }

    /// Execute this node with the resolved inputs in `input`.
    ///
    /// **Contract:**
    /// - Never panic — return [`crate::model::NodeOutput::failure`] for all error conditions.
    /// - Do not block the async executor — use `tokio::task::spawn_blocking` for CPU-intensive work.
    /// - The executor retries on *recoverable* failures up to `RetryPolicy::max_attempts`.
    ///   Mark an error unrecoverable when retrying would have no effect
    ///   (invalid config, auth rejection, etc.).
    async fn execute(&self, input: NodeInput) -> NodeOutput;
}

// ── Port definitions ──────────────────────────────────────────────────────────

/// Describes the connectable ports on a node block in the canvas.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PortDefinition {
    /// Port identifier — used in WorkflowEdge.from_port / to_port.
    pub id: String,
    /// Label shown on the block in the UI.
    pub label: String,
    /// Position hint for the canvas renderer.
    pub position: PortPosition,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PortPosition {
    Top,
    Bottom,
    Left,
    Right,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NodePorts {
    pub inputs: Vec<PortDefinition>,
    pub outputs: Vec<PortDefinition>,
}

impl Default for NodePorts {
    fn default() -> Self {
        Self {
            inputs: vec![PortDefinition {
                id: "input".to_string(),
                label: "In".to_string(),
                position: PortPosition::Left,
            }],
            outputs: vec![PortDefinition {
                id: "output".to_string(),
                label: "Out".to_string(),
                position: PortPosition::Right,
            }],
        }
    }
}

// ── Node registry ─────────────────────────────────────────────────────────────

/// Map from `type_id` to registered [`Node`] implementation.
///
/// Built once at startup via [`crate::nodes::register_builtins`], then wrapped in `Arc`
/// and shared across executor instances for the lifetime of the process.
pub struct NodeRegistry {
    nodes: HashMap<String, Arc<dyn Node>>,
}

impl NodeRegistry {
    pub fn new() -> Self {
        Self { nodes: HashMap::new() }
    }

    /// Register a node implementation, keyed by its [`Node::type_id`].
    ///
    /// If a node with the same `type_id` is already registered it is silently
    /// replaced. Call order determines which implementation wins for a given type.
    pub fn register(&mut self, node: Arc<dyn Node>) {
        self.nodes.insert(node.type_id().to_string(), node);
    }

    /// Look up by node_type_id (e.g. "http_request").
    pub fn get(&self, type_id: &str) -> Option<Arc<dyn Node>> {
        self.nodes.get(type_id).cloned()
    }

    #[allow(dead_code)]
    pub fn contains(&self, type_id: &str) -> bool {
        self.nodes.contains_key(type_id)
    }

    /// All descriptors — sent to the frontend so it knows what blocks are available.
    pub fn all_descriptors(&self) -> Vec<NodeDescriptor> {
        self.nodes.values().map(|n| NodeDescriptor::from_node(n.as_ref())).collect()
    }
}

impl Default for NodeRegistry {
    fn default() -> Self { Self::new() }
}

// ── Node descriptor (serializable, sent to UI) ───────────────────────────────

/// Serializable snapshot of a node's static metadata.
///
/// Sent to the frontend so it can populate the node palette and configure
/// port layout without holding a reference to the [`Node`] trait object.
/// Built by [`NodeDescriptor::from_node`] and returned by
/// [`NodeRegistry::all_descriptors`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NodeDescriptor {
    pub type_id: String,
    pub display_name: String,
    pub node_type: NodeType,
    pub version: String,
    pub input_schema: Value,
    pub output_schema: Value,
    pub ports: NodePorts,
    /// True when this node's ports are derived from config rather than static.
    /// The frontend uses this to call derivePorts() instead of reading the
    /// static port list from the descriptor.
    pub dynamic_ports: bool,
}

impl NodeDescriptor {
    pub fn from_node(node: &dyn Node) -> Self {
        Self {
            type_id: node.type_id().to_string(),
            display_name: node.display_name().to_string(),
            node_type: node.node_type(),
            version: node.version().to_string(),
            input_schema: node.input_schema(),
            output_schema: node.output_schema(),
            ports: node.ports(),
            dynamic_ports: node.is_dynamic_ports(),
        }
    }
}
