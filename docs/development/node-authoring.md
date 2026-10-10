# Adding a Built-in Node

A built-in node is a Rust type compiled directly into `aerini-engine`, one of the 40 that ship with every install. This page walks through adding one: the trait it implements, where it gets registered, and what else (icon, palette placement, danger list) actually needs a matching change versus what doesn't. It assumes you've read [Architecture](architecture.md); this page only covers the compiled-in path. A `.wasm` plugin implements the same `Node` trait indirectly, through `WasmPluginNode`, and is a different, separately compiled artifact; that path belongs to [Writing a Plugin Node](plugin-authoring.md), not here.

`CONTRIBUTING.md` has its own shorter "Adding a new node" section. Several details in it are stale against the current source, covered below as they come up. Treat this page as the current reference where the two disagree.

## The Node trait

Every built-in node implements `Node` (`aerini-engine/src/node.rs`). Seven methods have no default and must be implemented:

| Method | Returns |
|---|---|
| `type_id` | The id used everywhere else: `WorkflowNode.node_type_id`, the registry key, the danger list, the icon map. Must be globally unique. |
| `display_name` | Label shown in the palette and on the canvas block. |
| `node_type` | Palette category, one of the four `NodeType` variants (see below). |
| `version` | A semver string. Every currently registered node uses full `major.minor.patch` form (`"1.0.0"`, `"2.0.0"`); nothing enforces this, but it's the consistent existing convention. |
| `input_schema` | JSON Schema (Draft 7) for the config panel. `Value::Null` or `json!({})` if there's nothing to configure. |
| `output_schema` | JSON Schema for the shape written to the node's output on success. |
| `execute` | Runs the node against a resolved `NodeInput` and returns a `NodeOutput`. Async. |

Everything else on the trait has a default and only needs overriding when it applies: `ports` (defaults to one input, one output), `is_dynamic_ports` (config-driven port count), `is_plugin` (always `false` for a built-in), `is_trigger_capable`, `description`, `icon`, `author`, and `ports_from_config`.

## A minimal node

```rust
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::Node;

pub struct UppercaseNode;

#[async_trait]
impl Node for UppercaseNode {
    fn type_id(&self) -> &'static str { "uppercase" }
    fn display_name(&self) -> &'static str { "Uppercase" }
    fn node_type(&self) -> NodeType { NodeType::Utility }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Convert the input text to uppercase." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "text": { "type": "string", "description": "Text to convert" }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({ "type": "object", "properties": { "result": { "type": "string" } } })
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let text = input.input["text"].as_str().unwrap_or("");
        NodeOutput::success(json!({ "result": text.to_uppercase() }))
    }
}
```

Two things about `NodeInput` worth being explicit about, since `CONTRIBUTING.md`'s current example gets both wrong: the config value lives in a field called `input` (a `serde_json::Value`), not `params`, and `NodeType` comes from `crate::model`, not `crate::node`. Every existing node file imports it that way; `crate::node` never re-exports it.

There's a third thing `CONTRIBUTING.md` gets wrong, worth its own paragraph because it's about behavior, not just imports: it tells authors to "resolve" a credential via `input.resolve_credential()`. That method doesn't exist anywhere in the crate. By the time `execute` runs, the executor has already substituted any credential-backed config field with its plaintext value directly into `input.input`. A node never sees a credential id and never calls anything to resolve one; it just reads the field like any other config value. The one rule that does still apply is not to log or echo that value back into `logs` or an error message, since the node has no way to know which fields came from a saved credential and which didn't.

If your node makes outbound HTTP requests to a host the user supplies, build the client with `crate::nodes::util::guarded_client_builder(SsrfPolicy::Strict)` (or `AllowLocal` for a node whose purpose is a local server), keep it in a `OnceLock` like the other integration nodes, and call `check_host_ssrf_from_url` before the request for a clear error. The builder's resolver refuses blocked addresses when the connection is made, so a hostname cannot pass the early check and then resolve to an internal address; IP-literal URLs skip the resolver, which is why the early check stays. Report a failed `send()` with `util::http_err_output(replay, &e)` or `util::is_retryable_network_error(replay, &e)`, which treat a refused target as `SSRF_BLOCKED` and do not retry it. Pass `Replay::Never` for a call that sends, creates or charges, so a timeout (the provider may already have acted) is not retried, and `Replay::Safe` for a read or a call that carries a stable idempotency key; a failed connection is retried either way. The `check_ssrf()` in `http.rs` is private to that module; `CONTRIBUTING.md` points at it, but the helpers in `nodes/util.rs` are the ones to call.

## Registering it

Two edits in `aerini-engine/src/nodes/mod.rs`.

First, a `pub mod your_node;` line alongside the other `pub mod` declarations near the top of the file. There's no enforced ordering here (the existing list isn't strictly alphabetical: `notion` sits before `notification` and `s3` sits after `slack`). Put it wherever's convenient; most contributors add it near modules for similar nodes.

Second, a `registry.register(Arc::new(your_node::YourNode));` call inside `register_builtins()`. This part does have real structure: the function is broken into comment-delimited sections (`// ── Triggers`, `// ── Logic`, `// ── AI`, `// ── Files`, a mixed Utility/Files/Integrations block, `// ── Integrations`, `// ── Utility`) that roughly track the palette categories below. Put your `register` call under the section matching what the node actually is, and if it doesn't match that section's default category, add the same short inline comment the existing mixed block already uses (`// Files`, `// Integrations`) so the grouping stays legible without re-reading every call.

## Category and the palette

`NodeType` has four variants: `Action`, `Ai`, `Logic`, `Utility`. The [Nodes Reference](../guide/nodes.md) groups the palette into seven sections (Triggers, Core Actions, Files & Storage, Integrations, AI, Logic, Utility), and those aren't the same list described twice. `Ai`, `Logic`, and `Utility` map straight across. `Action` is the one category the frontend splits further, in `src/palette-manager.ts`: a node whose `type_id` is in `TRIGGER_NODE_IDS` (`src/node-ids.ts`) becomes a Trigger, one in `INTEGRATION_NODE_IDS` or `FILES_STORAGE_NODE_IDS` (both declared in `palette-manager.ts` itself) becomes Integrations or Files & Storage, and anything else returning `NodeType::Action` lands in Core Actions by default.

That default matters: a new `Action` node with no entry in any of those three sets still gets a palette section, it just lands in Core Actions rather than wherever you actually intended. If your node belongs in Triggers, Integrations, or Files & Storage, add its `type_id` to the matching set in `palette-manager.ts` as a second, frontend-side edit. This is a real exception to `node.rs`'s own doc comment, which says the icon entry is the only other file that needs touching; that claim holds for a plain Core Actions/AI/Logic/Utility node, not for one that needs a non-default palette section.

## The icon

Add an entry to `NODE_ICONS` in `src/utils.ts`, keyed by the same `type_id`:

```typescript
your_node: "🔧",
```

A single emoji or an inline SVG string both work; every existing entry uses one or the other.

## description() and the legacy fallback map

`node.rs`'s own doc comment on `description()` mentions a frontend fallback map for nodes that haven't migrated to the trait method yet. That map, `NODE_DESCRIPTION_FALLBACK` in `src/node-descriptions.ts`, has exactly one entry today: `note`, a frontend-only canvas annotation with no backing `Node` implementation at all. Every one of the 40 currently registered nodes overrides `description()` directly in Rust. There is no live built-in node relying on that fallback map, and a new node should implement `description()` the same way, not add anything to the frontend map.

## Dangerous nodes

If your node runs arbitrary shell commands, arbitrary code, or has direct database access, add its `type_id` to `DANGEROUS_NODE_TYPE_IDS` in `aerini-engine/src/nodes/mod.rs`, right above `register_builtins`. Desktop's `run_workflow` command, the `SchedulerDaemon`'s job start, `aerini-server`'s `serve` mode startup warning, and export packaging all check a workflow's actual node list against this constant. `aerini-server`'s `api` mode is the exception: it doesn't consult this list at all, it disables shell/code/database execution at the executor level based purely on whether its own `--allow-shell`/`--allow-code`/`--allow-database` flags were passed, independent of what any given uploaded workflow contains. `nodes/mod.rs`'s own doc comment on this constant currently claims `api` mode is one of its consumers; it isn't, confirmed directly against `api_mode` and `api_server/mod.rs` rather than trusting that comment. Worth flagging for a source fix, not something this page can correct on its own.

The frontend keeps a second copy, `DANGEROUS_NODE_IDS` in `src/node-ids.ts`. As of this page, the two lists agree (`shell_exec`, `code`, `database` on both sides, confirmed by `danger-badge.test.ts`), but nothing ties them together mechanically. Forgetting the frontend update doesn't fail a build or a test on its own: it means the canvas never shows the danger badge on your node and the pre-run confirmation dialog (`checkDangerousNodes` in `src/validation.ts`, called from the toolbar's Run button) never warns about it, even though the backend-side checks on scheduled runs, always-on runs, and `serve` mode's own startup warning still work correctly off the Rust list. A node that's dangerous but missing from the frontend list is silently under-warned in the desktop UI, not silently unprotected everywhere; keep both lists in your PR whenever you add one.

## The config panel: when you need one

Most nodes need nothing beyond the `input_schema` above. The popover's generic field renderer (`src/popover/field-renderer.ts`) turns a JSON Schema's string, number, boolean, and enum properties into working form fields automatically, including the `{{...}}` expression picker, a multi-line box for a string property annotated `"x-aerini-multiline": true` (use it for JSON or long text; the flag applies to that property only, whatever its name) and, for any field named `api_key`/`password` or annotated with `x-aerini-credential`, the saved-credential picker. A property declared `type: "object"`, an array whose `items` aren't plain strings, or one left untyped that ends up holding an object/array value (HTTP Request's `body` is an example) has no form control here — it's edited as JSON in an **Advanced: full config (JSON)** section instead, the same routing [Writing a Plugin Node](plugin-authoring.md) documents in full.

A handful of nodes need more than that and register an entry in `NODE_CONFIG_EXTENSIONS` (`src/popover/lifecycle.ts`): a static banner or warning that isn't tied to any one schema field (AI Prompt/Agent's API-cost notice, HTTP's SSRF notice, Webhook's reliability banners), a set of fields whose visibility depends on another field's value rather than being a flat list (Schedule's three trigger modes replace the generic field loop entirely), or a non-standard credential section (HTTP forces its Connection section open and swaps in a custom auth-mode picker). Two of these nodes, Save to Folder and Collect Files, also override `is_dynamic_ports`, but dynamic ports aren't the deciding factor on their own: most of the extension list doesn't use them. The real dividing line is simpler: if the generic schema-driven renderer can produce the UI you want, you don't need an extension; if it can't (conditional fields, a banner, bespoke credential handling), it goes in `NODE_CONFIG_EXTENSIONS`, in `src/node-configs/` or `src/popover/extensions/` alongside the existing ones.

## Tests

`CONTRIBUTING.md` asks for at least one `#[cfg(test)]` block per new node covering the normal path, missing/null input, and any credential resolution. In practice, 26 of the 39 files implementing built-in nodes carry their own test module (one of those 26, `variables.rs`, covers both Set and Get Variable in one file). The other 13, mostly the simpler integration nodes (Slack, Discord, GitHub, Notion, Telegram, SendGrid, Email, Notification) and a few others (Schedule, Manual Trigger, Delay, Stop, Image Generation), rely on the broader coverage in `aerini-engine/tests/workflow_integration.rs` instead. `CONTRIBUTING.md`'s guidance is still the current expectation for anything new: add the node's own `#[cfg(test)]` block rather than leaning on the integration suite to happen to cover it.

## What this page doesn't cover

[Writing a Plugin Node](plugin-authoring.md) covers the `.wasm` path: the same `Node` trait, implemented by `WasmPluginNode` on top of a compiled component instead of directly in this crate, with its own sandbox and registration story. [Desktop IPC Reference](desktop-ipc-reference.md) covers the Tauri commands a node's config panel might call into, if it needs to (file pickers, OAuth flows); nothing here does that on its own account. [Nodes Reference](../guide/nodes.md) documents the 40 nodes that already exist for someone building a workflow; this page is for adding number 41, not for using any of the current ones.
