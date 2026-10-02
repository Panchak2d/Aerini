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
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

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

    /// True when this node was loaded from a WASM plugin rather than built in.
    /// The frontend uses this to render a plugin badge in the palette.
    /// All built-in nodes use the default (false).
    fn is_plugin(&self) -> bool {
        false
    }

    /// True when this plugin exports the `trigger` WIT interface, checked
    /// once at load time. The executor uses this to recognize a
    /// trigger-capable node as a legitimate entry point instead of warning
    /// on it as a disconnected node. All built-in nodes and plugins that
    /// don't export `trigger` use the default (false).
    fn is_trigger_capable(&self) -> bool {
        false
    }

    /// One or two sentence plain-text description shown in the node palette tooltip.
    /// Plugins that do not override this return an empty string; the frontend falls
    /// back to a static description map for built-in nodes that have not yet migrated.
    fn description(&self) -> &'static str {
        ""
    }

    /// Raw icon markup (SVG shape fragment), untrusted until sanitized at render
    /// time. Empty string -> generic glyph. All built-in nodes and plugins that
    /// do not export the optional `metadata` WIT interface use this default.
    fn icon(&self) -> &'static str {
        ""
    }

    /// Author or organization name. Empty string when unset. All built-in nodes
    /// and plugins that do not export the optional `metadata` WIT interface use
    /// this default.
    fn author(&self) -> &'static str {
        ""
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
    /// - `input.cancel_token`, when `Some`, resolves when the run is cancelled. A node
    ///   with a long-running wait (listening for a callback, polling a remote job)
    ///   should race it with `tokio::select!` so cancellation interrupts the wait
    ///   instead of running until the node's own timeout elapses.
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
    /// Optional semantic type tag for this port (e.g. `"files"`).
    ///
    /// Canvas uses this to validate connections at wire-draw time and to
    /// choose the correct auto-injected expression. When absent (`None`),
    /// no type constraint is enforced and the generic `.output` expression
    /// is used as the fallback. Serialised as `null` when absent — existing
    /// saved workflows without this field deserialise cleanly via `default`.
    #[serde(default)]
    pub port_type: Option<String>,
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
                id:        "input".to_string(),
                label:     "In".to_string(),
                position:  PortPosition::Left,
                port_type: None,
            }],
            outputs: vec![PortDefinition {
                id:        "output".to_string(),
                label:     "Out".to_string(),
                position:  PortPosition::Right,
                port_type: None,
            }],
        }
    }
}

// ── Node registry ─────────────────────────────────────────────────────────────

/// Map from `type_id` to registered [`Node`] implementation.
///
/// Built once at startup via [`crate::nodes::register_builtins`], then wrapped in `Arc`
/// and shared across executor instances for the lifetime of the process.
///
/// # Built-in protection
///
/// After all built-in nodes are registered, call [`seal_builtins`] to lock the
/// current set of type IDs as reserved. Any subsequent [`register_plugin`] call
/// whose `type_id` collides with a built-in is rejected with an error and a
/// `WARN`-level log, preventing malicious or misconfigured WASM plugins from
/// silently replacing built-in node implementations.
pub struct NodeRegistry {
    nodes:    HashMap<String, Arc<dyn Node>>,
    builtins: HashSet<String>,
}

impl NodeRegistry {
    pub fn new() -> Self {
        Self { nodes: HashMap::new(), builtins: HashSet::new() }
    }

    /// Register a built-in node implementation, keyed by its [`Node::type_id`].
    ///
    /// If a node with the same `type_id` is already registered it is silently
    /// replaced — duplicates among builtins indicate a programming error but are
    /// not fatal. Call [`seal_builtins`] after all built-ins are registered.
    pub fn register(&mut self, node: Arc<dyn Node>) {
        self.nodes.insert(node.type_id().to_string(), node);
    }

    /// Lock the current set of registered type IDs as the built-in namespace.
    ///
    /// Must be called once, after [`crate::nodes::register_builtins`] and before
    /// any [`register_plugin`] call. Subsequent plugin registrations that collide
    /// with a sealed type ID are rejected.
    pub fn seal_builtins(&mut self) {
        self.builtins = self.nodes.keys().cloned().collect();
    }

    /// Register a WASM plugin node.
    ///
    /// Returns `Err` if the plugin's `type_id` conflicts with a sealed built-in.
    /// Returns `Ok(false)` (rather than `Ok(true)`) when the `type_id` was already
    /// claimed by a different plugin — the new node still wins (last-registered),
    /// but callers can use this signal to surface the collision instead of only
    /// logging it. A `WARN` log is emitted in both the error case and this case.
    pub fn register_plugin(&mut self, node: Arc<dyn Node>) -> Result<bool, String> {
        let id = node.type_id();
        if self.builtins.contains(id) {
            let msg = format!("plugin type_id '{id}' conflicts with a built-in node — rejected");
            tracing::warn!("{}", msg);
            return Err(msg);
        }
        let collided = self.nodes.contains_key(id);
        if collided {
            tracing::warn!(
                "plugin type_id '{}' already registered by another plugin — overwriting",
                id
            );
        }
        self.nodes.insert(id.to_string(), node);
        Ok(!collided)
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

// ── Reloadable ────────────────────────────────────────────────────────────────

/// A value that can be atomically replaced at runtime, for callers that need
/// to observe updates (e.g. a rebuilt [`NodeRegistry`] after installing a
/// plugin) without restarting the process.
///
/// `current()` clones the `Arc` under a read lock and returns immediately —
/// the lock is never held across an `.await`. A workflow run captures its
/// own `current()` snapshot once, at the start of the run, and holds that
/// `Arc` for the run's entire duration: a `reload()` call therefore only
/// affects runs started after it returns. A run already in progress keeps
/// executing against the version it started with, even if `reload()` is
/// called mid-run — this is what makes `reload()` safe to call at any time,
/// with no run-tracking or cancellation logic needed.
pub struct Reloadable<T>(RwLock<Arc<T>>);

impl<T> Reloadable<T> {
    pub fn new(value: T) -> Self {
        Self(RwLock::new(Arc::new(value)))
    }

    /// Snapshot for one command call or one workflow run. Cheap: the read
    /// lock is held only long enough to clone the `Arc` pointer.
    ///
    /// Recovers from a poisoned lock instead of propagating the panic: the
    /// critical section here is a single pointer clone/assignment with no
    /// partially-mutated state to distrust, so treating a poison as fatal
    /// would only mean one unrelated panic elsewhere permanently breaks
    /// every later reload and every later run for the rest of the process's
    /// life — worse than reading the value the poisoning writer left behind.
    pub fn current(&self) -> Arc<T> {
        let guard = self.0.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        Arc::clone(&guard)
    }

    /// Atomically replace the value. Snapshots already returned by an
    /// earlier `current()` call are unaffected.
    pub fn reload(&self, value: T) {
        let mut guard = self.0.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = Arc::new(value);
    }
}

#[cfg(test)]
mod reloadable_tests {
    use super::Reloadable;

    #[test]
    fn current_reflects_the_latest_reload() {
        let r = Reloadable::new(1);
        assert_eq!(*r.current(), 1);
        r.reload(2);
        assert_eq!(*r.current(), 2);
    }

    /// A snapshot captured before `reload()` must keep reading its own
    /// (old) value afterward, not switch underneath the holder.
    #[test]
    fn a_snapshot_captured_before_reload_is_unaffected_by_it() {
        let r = Reloadable::new("v1");
        let in_flight_snapshot = r.current();
        r.reload("v2");
        assert_eq!(*in_flight_snapshot, "v1");
        assert_eq!(*r.current(), "v2");
    }
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
    /// True when this node was loaded from a WASM plugin rather than built in.
    pub is_plugin: bool,
    /// True when this node can start a workflow run (see
    /// [`Node::is_trigger_capable`]). The frontend uses this to list a plugin
    /// trigger under the palette's Triggers section; built-in triggers are
    /// recognized by type id there.
    #[serde(default)]
    pub trigger_capable: bool,
    /// One or two sentence plain-text description of what this node does.
    /// Empty string when the node does not override `Node::description()`.
    #[serde(default)]
    pub description: String,
    /// Raw icon markup declared by the plugin. Untrusted — sanitized only at
    /// render time. Empty string when unset.
    #[serde(default)]
    pub icon: String,
    /// Author or organization name declared by the plugin. Empty string when unset.
    #[serde(default)]
    pub author: String,
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
            is_plugin: node.is_plugin(),
            trigger_capable: node.is_trigger_capable(),
            description: node.description().to_string(),
            icon: node.icon().to_string(),
            author: node.author().to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StubNode(&'static str);

    #[async_trait]
    impl Node for StubNode {
        fn type_id(&self) -> &'static str { self.0 }
        fn display_name(&self) -> &'static str { self.0 }
        fn node_type(&self) -> NodeType { NodeType::Utility }
        fn version(&self) -> &'static str { "0.0.0" }
        fn input_schema(&self) -> Value { Value::Null }
        fn output_schema(&self) -> Value { Value::Null }
        async fn execute(&self, _input: NodeInput) -> NodeOutput {
            unimplemented!("not exercised by registry tests")
        }
    }

    fn sealed_registry_with_builtin(id: &'static str) -> NodeRegistry {
        let mut reg = NodeRegistry::new();
        reg.register(Arc::new(StubNode(id)));
        reg.seal_builtins();
        reg
    }

    #[test]
    fn register_plugin_first_time_returns_ok_true() {
        let mut reg = sealed_registry_with_builtin("http_request");
        let result = reg.register_plugin(Arc::new(StubNode("my_plugin_node")));
        assert_eq!(result, Ok(true));
        assert!(reg.contains("my_plugin_node"));
    }

    #[test]
    fn register_plugin_builtin_collision_returns_err_and_keeps_builtin() {
        let mut reg = sealed_registry_with_builtin("http_request");
        let result = reg.register_plugin(Arc::new(StubNode("http_request")));
        assert!(result.is_err());
        // built-in must still be the one registered under that id
        assert_eq!(reg.get("http_request").unwrap().display_name(), "http_request");
    }

    #[test]
    fn register_plugin_plugin_collision_returns_ok_false_and_last_wins() {
        let mut reg = sealed_registry_with_builtin("http_request");
        reg.register_plugin(Arc::new(StubNode("shared_id"))).unwrap();
        let result = reg.register_plugin(Arc::new(StubNode("shared_id")));
        assert_eq!(result, Ok(false));
        assert!(reg.contains("shared_id"));
    }

    #[test]
    fn descriptor_icon_and_author_default_to_empty_string() {
        let descriptor = NodeDescriptor::from_node(&StubNode("plain_node"));
        assert_eq!(descriptor.icon, "");
        assert_eq!(descriptor.author, "");
    }

    struct BrandedStubNode;

    #[async_trait]
    impl Node for BrandedStubNode {
        fn type_id(&self) -> &'static str { "branded_node" }
        fn display_name(&self) -> &'static str { "Branded Node" }
        fn node_type(&self) -> NodeType { NodeType::Utility }
        fn version(&self) -> &'static str { "2.0.0" }
        fn input_schema(&self) -> Value { Value::Null }
        fn output_schema(&self) -> Value { Value::Null }
        fn icon(&self) -> &'static str { "<circle r=\"1\"/>" }
        fn author(&self) -> &'static str { "Acme Co" }
        async fn execute(&self, _input: NodeInput) -> NodeOutput {
            unimplemented!("not exercised by descriptor tests")
        }
    }

    struct TriggerStubNode;

    #[async_trait]
    impl Node for TriggerStubNode {
        fn type_id(&self) -> &'static str { "trigger_stub" }
        fn display_name(&self) -> &'static str { "Trigger Stub" }
        fn node_type(&self) -> NodeType { NodeType::Utility }
        fn version(&self) -> &'static str { "1.0.0" }
        fn input_schema(&self) -> Value { Value::Null }
        fn output_schema(&self) -> Value { Value::Null }
        fn is_trigger_capable(&self) -> bool { true }
        async fn execute(&self, _input: NodeInput) -> NodeOutput {
            unimplemented!("not exercised by descriptor tests")
        }
    }

    #[test]
    fn descriptor_reports_trigger_capability_only_when_node_declares_it() {
        assert!(NodeDescriptor::from_node(&TriggerStubNode).trigger_capable);
        assert!(!NodeDescriptor::from_node(&StubNode("plain_node")).trigger_capable);
    }

    #[test]
    fn descriptor_carries_icon_and_author_when_node_overrides_them() {
        let descriptor = NodeDescriptor::from_node(&BrandedStubNode);
        assert_eq!(descriptor.icon, "<circle r=\"1\"/>");
        assert_eq!(descriptor.author, "Acme Co");
    }
}
