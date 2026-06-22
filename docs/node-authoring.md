# Custom Node Authoring

How to add a new built-in node type to Aerini's engine. This is for contributors to the Aerini codebase. If you want to ship a new node type without modifying Aerini itself, see [Plugin Authoring](plugin-authoring.md) instead.

The surface area is small: one Rust file, one registry call, one icon entry. No other files need to change.

---

## Overview

Every node is a Rust struct that implements the `Node` trait from `aerini-engine/src/node.rs`. The executor dispatches to nodes through `Arc<dyn Node>`, using a registry that maps string type IDs to implementations.

Three things are required to add a node:

1. Implement `Node` on a struct in `aerini-engine/src/nodes/`
2. Register it in `register_builtins()` in `aerini-engine/src/nodes/mod.rs`
3. Add an icon entry in `src/utils.ts` → `NODE_ICONS`

---

## The `Node` trait

```rust
#[async_trait]
pub trait Node: Send + Sync {
    fn type_id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn node_type(&self) -> NodeType;
    fn version(&self) -> &'static str;
    fn input_schema(&self) -> Value;
    fn output_schema(&self) -> Value;
    fn ports(&self) -> NodePorts { NodePorts::default() }
    async fn execute(&self, input: NodeInput) -> NodeOutput;
}
```

### `type_id`

A stable snake_case identifier stored in `.aerini` files as `node_type_id`. **Never change this after shipping** — saved workflows reference it by this exact string. Examples: `"http_request"`, `"ai_prompt"`, `"shell_exec"`.

### `display_name`

The human-readable label in the palette and on the canvas. Safe to change at any time.

### `node_type`

Controls palette grouping and canvas color:

```rust
pub enum NodeType {
    Action,   // HTTP, Shell, Email, triggers, Stop
    Ai,       // AI Prompt, AI Agent, AI Memory, Image Generation
    Logic,    // If/Condition, Switch, Loop, Merge, Collect Files
    Utility,  // JSON, Transform, Set/Get Variable, Text Splitter
}
```

There are no separate `Trigger` or `Output` variants. Schedule, Webhook, and Manual Trigger use `Action`. The Output node uses `Utility`. Stop uses `Logic`.

### `version`

A semver string (`"1.0.0"`). Informational only — not used for compatibility checks.

### `input_schema` / `output_schema`

JSON Schema (draft-07) objects. `input_schema` drives the config panel UI and runtime input validation. `output_schema` populates the expression picker for downstream nodes.

Minimal schema (no required fields):

```rust
fn input_schema(&self) -> Value {
    json!({ "type": "object", "properties": {} })
}
```

Schema with required fields:

```rust
fn input_schema(&self) -> Value {
    json!({
        "type": "object",
        "required": ["url"],
        "properties": {
            "url":     { "type": "string",  "description": "Target URL" },
            "timeout": { "type": "integer", "description": "Timeout in seconds", "default": 30 }
        }
    })
}
```

Credential fields use the `"x-credential": true` extension — the frontend renders these as a dropdown populated from the credential store:

```rust
"api_key": { "type": "string", "x-credential": true, "description": "API key credential ID" }
```

### `ports`

Controls the connectors drawn on the node. The default (one input on the left, one output on the right) covers the majority of nodes:

```rust
fn ports(&self) -> NodePorts {
    NodePorts::default()
}
```

For nodes with conditional routing, override this to add named outputs:

```rust
fn ports(&self) -> NodePorts {
    NodePorts {
        inputs: vec![PortDefinition {
            id: "input".to_string(),
            label: "In".to_string(),
            position: PortPosition::Left,
        }],
        outputs: vec![
            PortDefinition { id: "true".to_string(),  label: "True".to_string(),  position: PortPosition::Right },
            PortDefinition { id: "false".to_string(), label: "False".to_string(), position: PortPosition::Right },
        ],
    }
}
```

Port `id` values are stored in `WorkflowEdge.from_port` — treat them as permanent once shipped.

---

## `NodeInput`

`execute()` receives a `NodeInput`:

```rust
pub struct NodeInput {
    pub node_id:      String,
    pub workflow_id:  String,
    pub execution_id: String,
    pub input:        Value,           // merged config + resolved credentials
    pub context:      ExecutionContext,
}

pub struct ExecutionContext {
    pub variables:    HashMap<String, Value>,  // user-set variables
    pub node_outputs: HashMap<String, Value>,  // outputs of upstream nodes
    pub metadata:     HashMap<String, Value>,  // executor-injected flags
}
```

`input.input` contains the node's config fields with all `{{...}}` expressions already resolved and all credential IDs already replaced with their decrypted values. Read from it directly:

```rust
let url = input.input["url"].as_str().unwrap_or("");
let timeout = input.input["timeout"].as_u64().unwrap_or(30);
```

`input.context.node_outputs` lets you access upstream node outputs by node ID directly — useful for data that wasn't wired through the config panel. In most nodes you won't need this; expression resolution handles standard wiring.

---

## `NodeOutput`

Return one of these constructors from `execute()`:

```rust
// Success
NodeOutput::success(json!({ "status": 200, "body": response_body }))

// Success with log lines visible in the Logs tab
NodeOutput::success_with_logs(
    json!({ "sent": true }),
    vec!["Message delivered to #general".to_string()],
)

// Failure — stops this branch, marks the node red
NodeOutput::failure(NodeError::unrecoverable("HTTP_ERROR", "Connection refused"))

// Failure with logs
NodeOutput::failure_with_logs(error, vec!["Attempted 3 retries".to_string()])
```

---

## `NodeError`

```rust
// recoverable: true — executor retries per the node's RetryPolicy
NodeError::recoverable("RATE_LIMITED", "429 from upstream API")

// recoverable: false — no retry, branch fails immediately
NodeError::unrecoverable("MISSING_FIELD", "url is required")
```

The `code` field is shown in the Errors tab. Use UPPER_SNAKE_CASE and be specific: `"AUTH_FAILED"` rather than `"ERROR"`.

---

## Registering the node

In `aerini-engine/src/nodes/mod.rs`:

1. Add a `pub mod your_node;` declaration with the other module declarations (alphabetical by convention).
2. Call `registry.register(Arc::new(your_node::YourNode));` inside `register_builtins()`.

```rust
// Module declarations
pub mod your_node;

// Inside register_builtins():
registry.register(Arc::new(your_node::YourNode));
```

If your node needs the data directory (for a local database file, for example), it receives `data_dir: &std::path::Path`. See `ai_memory::AiMemoryNode::new(data_dir.join("ai_memory.db"))` for the pattern.

---

## Adding the icon

In `src/utils.ts`, add an entry to `NODE_ICONS`:

```typescript
export const NODE_ICONS: Record<string, string> = {
  // ... existing entries ...
  your_node_type_id: "⚡",  // any single emoji or short symbol
};
```

The key must exactly match the string returned by `type_id()`. Missing entries fall back to a generic placeholder.

---

## Complete example

A minimal node that reverses a string:

```rust
// aerini-engine/src/nodes/reverse.rs

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortDefinition, PortPosition};

pub struct ReverseNode;

#[async_trait]
impl Node for ReverseNode {
    fn type_id(&self)        -> &'static str { "reverse" }
    fn display_name(&self)   -> &'static str { "Reverse String" }
    fn node_type(&self)      -> NodeType     { NodeType::Utility }
    fn version(&self)        -> &'static str { "1.0.0" }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["text"],
            "properties": {
                "text": { "type": "string", "description": "Text to reverse" }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "result": { "type": "string" }
            }
        })
    }

    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs:  vec![PortDefinition { id: "input".to_string(),  label: "In".to_string(),  position: PortPosition::Left }],
            outputs: vec![PortDefinition { id: "output".to_string(), label: "Out".to_string(), position: PortPosition::Right }],
        }
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let text = match input.input["text"].as_str() {
            Some(t) => t.to_string(),
            None    => return NodeOutput::failure(NodeError::unrecoverable("MISSING_TEXT", "text is required")),
        };

        let result: String = text.chars().rev().collect();
        NodeOutput::success(json!({ "result": result }))
    }
}
```

Register:

```rust
// nodes/mod.rs
pub mod reverse;

// inside register_builtins():
registry.register(Arc::new(reverse::ReverseNode));
```

Icon:

```typescript
reverse: "⇄",
```

---

## Checklist before shipping

- [ ] `type_id()` is unique among all registered nodes
- [ ] `type_id()` and all port `id` values will never change after shipping
- [ ] `input_schema` lists all required fields
- [ ] All `NodeError` codes are UPPER_SNAKE_CASE and descriptive enough to act on
- [ ] `execute()` never panics — all `unwrap()` calls target values that cannot be `None`
- [ ] Credentials are read from `input.input` (already decrypted), not fetched separately
- [ ] Long-running operations use `tokio::select!` to respect cancellation
- [ ] Icon added to `NODE_ICONS`
