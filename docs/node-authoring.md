# Custom Node Authoring

How to add a new node type to Flowo. The surface is small: one Rust file, one registry call, one icon entry.

---

## Overview

Every node in the canvas is a Rust struct that implements the `Node` trait from `flowo-engine/src/node.rs`. The executor dispatches through `Arc<dyn Node>` — the registry maps a string type ID to an implementation.

Adding a node requires three things:

1. Implement `Node` on a struct in `flowo-engine/src/nodes/`.
2. Register it in `register_builtins()` in `flowo-engine/src/nodes/mod.rs`.
3. Add an icon entry in `src/utils.ts` → `NODE_ICONS`.

No other files require changes.

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

A stable snake_case string identifier — this is what gets stored in `.flowo` files as `node_type_id`. **Never change it after shipping** — existing workflows reference it by this string. Examples: `"http_request"`, `"shell_exec"`, `"ai_prompt"`.

### `display_name`

The human-readable label shown in the node search palette and on the canvas block. Can be changed at any time.

### `node_type`

Controls where the node appears in the palette and how the canvas colours it:

```rust
pub enum NodeType {
    Trigger,   // entry nodes — Schedule, Webhook, ManualTrigger
    Action,    // most nodes — HTTP, Shell, Email, etc.
    Logic,     // branching — IfCondition, Switch, Loop
    Transform, // data manipulation — JSON, Transform, TextSplitter
    Output,    // terminal nodes — Output, Stop
}
```

### `version`

A semver string (`"1.0.0"`). Currently informational only — not used for compatibility checks.

### `input_schema` / `output_schema`

JSON Schema (draft-07) objects describing the node's config fields. These serve two purposes:

- The frontend uses them to auto-generate the config panel UI.
- The executor uses them for runtime input validation (when enabled).

A minimal schema with no required fields:

```rust
fn input_schema(&self) -> Value {
    json!({ "type": "object", "properties": {} })
}
```

A schema with required fields and types:

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

Controls the connectors drawn on the node block. The default (one input on the left, one output on the right) covers most nodes:

```rust
fn ports(&self) -> NodePorts {
    NodePorts::default()
}
```

For nodes with conditional routing (like `if_condition` or `switch`), override this to add named output ports:

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

Port `id` values are stored in `WorkflowEdge.from_port` — **do not rename them after shipping**.

---

## `NodeInput`

`execute()` receives a `NodeInput`:

```rust
pub struct NodeInput {
    pub node_id:      String,
    pub workflow_id:  String,
    pub execution_id: String,
    pub input:        Value,      // merged config + resolved credentials
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

`input.context.node_outputs` lets you access upstream node outputs by node ID, in case you need data beyond what was wired through the config. In most nodes you don't need this — expression resolution handles wiring automatically.

---

## `NodeOutput`

Return one of these from `execute()`:

```rust
// Success with output data
NodeOutput::success(json!({ "status": 200, "body": response_body }))

// Success with output data and log lines
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
// recoverable: true — executor will retry (if retries configured)
NodeError::recoverable("RATE_LIMITED", "429 from upstream API")

// recoverable: false — no retry, branch fails immediately
NodeError::unrecoverable("MISSING_FIELD", "url is required")
```

The `code` field is an UPPER_SNAKE_CASE string shown in the Errors tab. Make it specific enough to be actionable: `"AUTH_FAILED"` not `"ERROR"`.

---

## Registering the node

In `flowo-engine/src/nodes/mod.rs`:

1. Add a `pub mod your_node;` declaration at the top with the other module declarations.
2. Add a `registry.register(Arc::new(your_node::YourNode));` call inside `register_builtins()`.

```rust
// mod declarations (alphabetical by convention)
pub mod your_node;

// inside register_builtins():
registry.register(Arc::new(your_node::YourNode));
```

If your node needs the data directory (e.g. for a local database file), it receives `data_dir: &std::path::Path` — see `ai_memory::AiMemoryNode::new(data_dir.join("ai_memory.db"))` for the pattern.

---

## Adding the icon

In `src/utils.ts`, add an entry to `NODE_ICONS`:

```typescript
export const NODE_ICONS: Record<string, string> = {
  // ... existing entries ...
  your_node_type_id: "⚡",  // use any single emoji or short symbol
};
```

The key must exactly match the string returned by your node's `type_id()`. If the key is absent, the canvas renders a generic placeholder.

---

## Complete example

A minimal node that reverses a string:

```rust
// flowo-engine/src/nodes/reverse.rs

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
    fn node_type(&self)      -> NodeType     { NodeType::Transform }
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

Register in `nodes/mod.rs`:

```rust
pub mod reverse;

// inside register_builtins():
registry.register(Arc::new(reverse::ReverseNode));
```

Icon in `src/utils.ts`:

```typescript
reverse: "⇄",
```

---

## Checklist before shipping a node

- [ ] `type_id()` is unique across all registered nodes
- [ ] `type_id()` and all port `id` values will never be renamed
- [ ] `input_schema` lists all required fields
- [ ] All `NodeError` codes are UPPER_SNAKE_CASE and specific
- [ ] `execute()` never panics — all `unwrap()` calls are on values that cannot be `None`
- [ ] Credentials are read from `input.input` (already decrypted by executor), not fetched directly
- [ ] Long-running operations respect cancellation (check `tokio::select!` if needed)
- [ ] Icon added to `NODE_ICONS`
