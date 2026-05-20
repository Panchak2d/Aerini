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

#[async_trait]
pub trait Node: Send + Sync {
    /// Matches WorkflowNode.node_type_id (e.g. "http_request", "shell_exec").
    fn type_id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn node_type(&self) -> NodeType;
    fn version(&self) -> &'static str;
    fn input_schema(&self) -> Value;
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

pub struct NodeRegistry {
    nodes: HashMap<String, Arc<dyn Node>>,
}

impl NodeRegistry {
    pub fn new() -> Self {
        Self { nodes: HashMap::new() }
    }

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
