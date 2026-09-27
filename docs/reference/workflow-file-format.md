# Workflow File Format

This page documents the JSON shape of a workflow: every top-level field, what a node and a connection look like on the wire, and which fields are required. It assumes you already know what a workflow, node, and connection are; see [Concepts](../getting-started/concepts.md) first if not.

## The file itself

A workflow is plain JSON, saved with a `.aerini` extension by convention. A `.json` file with the same shape works the same way, see [Workflow file](../glossary.md#workflow-file). This is the shape Aerini's own database stores, the shape exchanged between the desktop frontend and the engine, and the shape the scheduler loads for a background run.

## Top-level fields

| Field | Type | Required | Default / notes |
|---|---|---|---|
| `schema_version` | string | No | See [Schema version](#schema-version) below. |
| `id` | string | Yes | Unique ID for this workflow. |
| `name` | string | Yes | Display name shown in the Workflows sidebar and window title. |
| `description` | string | No | Defaults to an empty string. Not currently editable from the desktop UI; present in the schema for hand-authored or generated files. |
| `nodes` | array of [node objects](#nodes) | Yes | Every box on the canvas, including trigger nodes. |
| `edges` | array of [connection objects](#connections) | Yes | Called a "connection" everywhere else in these docs; the JSON key is `edges`. |
| `metadata` | object | No | Defaults per field if omitted, see [Metadata](#metadata) below. |
| `max_duration_secs` | number | No | Wall-clock limit for the whole run, in seconds. The executor clamps any value to the 10 to 86400 range. Omit for no limit. |
| `unlimited_duration` | boolean | No | Defaults to `false`. Only takes effect on a manual run started from the desktop app; scheduled and background runs never read this field. When `true`, the run ignores both the server's duration ceiling and this workflow's own `max_duration_secs`. |
| `parallel_execution` | boolean | No | Defaults to `false`. When `true`, independent branches run concurrently instead of one after another. |
| `max_concurrent_nodes` | number | No | Caps simultaneous node tasks when `parallel_execution` is `true`. Falls back to 8 if unset. |
| `settings` | object | No | Currently holds only `chat`, see [Chat settings](#chat-settings) below. |

### Metadata

The `metadata` object as a whole is optional: leave it out entirely and every field below falls back to its default. Include it, though, and `author`, `created_at`, and `updated_at` are required inside it; only `tags` and `collection_id` keep their own per-field defaults regardless.

| Field | Type | Default when `metadata` is absent |
|---|---|---|
| `author` | string | `"user"` |
| `created_at` | string, ISO-8601 | current time |
| `updated_at` | string, ISO-8601 | current time |
| `version` | string | `"1.0.0"`. A free-form label, not the same field as the top-level `schema_version`. |
| `tags` | array of strings | `[]` |
| `collection_id` | string or `null` | `null` |

`collection_id` references a named collection in the Workflows sidebar by ID. The collection's own name and color live outside the workflow file; a missing or `null` `collection_id` leaves the workflow Uncategorized.

### Chat settings

`settings.chat` mirrors the Chat Panel toggles set from the Workflow Settings modal. Each field defaults independently, so a file that sets only some of them still loads correctly. See [Chat Panel](../guide/chat-panel.md) for what each toggle actually does in the app.

| Field | Type | Default |
|---|---|---|
| `allow_attachments` | boolean | `false` |
| `allow_image_responses` | boolean | `true` |
| `max_message_length` | number | `2000` |
| `session_persistence` | boolean | `true` |
| `show_branding` | boolean | `true` |

## Nodes

Aerini has no separate top-level list for triggers. Manual Trigger, Schedule, and Webhook are ordinary entries in `nodes`, identified by the `node_type_id` values `manual_trigger`, `schedule`, and `webhook`.

| Field | Type | Required | Default / notes |
|---|---|---|---|
| `id` | string | Yes | Unique within this workflow, e.g. `"node_abc123"`. |
| `node_type_id` | string | Yes | Which node implementation to run, e.g. `"http_request"`. See [Nodes Reference](../guide/nodes.md) for every valid value. |
| `node_type` | string | Yes | One of `"action"`, `"ai"`, `"logic"`, `"utility"`. Controls which palette section the node appears in; has no effect on execution. |
| `name` | string | Yes | Label shown on the canvas block. |
| `config` | object | No | Defaults to `{}`. Node-specific settings from the config panel; shape depends entirely on `node_type_id`, see [Nodes Reference](../guide/nodes.md). |
| `credentials` | object | No | Defaults to `{}`. Maps a config field name to a saved credential's ID, e.g. `{"api_key": "openai-prod"}`. The secret itself never appears in this file, see [Credentials](../guide/credentials.md). |
| `input_schema` | object | Yes | JSON Schema the node's input must match. The app always writes `{}` for a node with no schema of its own; the key still has to be present. |
| `output_schema` | object | Yes | Same rule as `input_schema`, for the node's output. |
| `retry` | object | No | Defaults to one attempt, no automatic retry, see [Retry](#retry) below. |
| `fallback_node` | string or `null` | No | Reserved for future use. Not read by the executor in the current version. |
| `disabled` | boolean | No | Defaults to `false`. When `true`, the executor skips the node and treats it as if it succeeded with empty output. |
| `position` | object | No | Defaults to `{"x": 0, "y": 0}`. Canvas coordinates, `{"x": number, "y": number}`, ignored by the executor. |

The desktop app also writes `ports` and `dynamic_ports` into every saved node object, canvas metadata it uses to draw connection points. Neither is read by the engine; a file that leaves them out loads fine.

### Retry

| Field | Type | Default |
|---|---|---|
| `max_attempts` | number | `1` |
| `backoff_ms` | number | `500` |

`max_attempts: 1` means no automatic retry. A node with side effects, an email send or an HTTP POST, only retries on its own if a workflow author raises this above 1 from the Retry UI.

## Connections

A connection is the line drawn from one node's output to another's input; the JSON key for the array holding them is `edges`, and each entry below is one connection.

| Field | Type | Required | Default / notes |
|---|---|---|---|
| `id` | string | Yes | Unique ID for this connection. |
| `from_node` | string | Yes | Source node's `id`. |
| `from_port` | string | Yes | Output port name on the source node, e.g. `"output"`, or a branch name such as `"on_true"` for a node with more than one output. |
| `to_node` | string | Yes | Target node's `id`. |
| `to_port` | string | Yes | Input port name on the target node, e.g. `"input"`. |
| `condition` | string or `null` | No | Expression the canvas uses to decide whether to draw the line. Not evaluated by the executor: routing follows `from_port` alone. |
| `on_success` | string or `null` | No | Alternate target node ID activated when the source node succeeds, used to register an extra reachability edge. |
| `on_failure` | string or `null` | No | Target node ID the executor routes to when the source node fails after every retry attempt, instead of aborting the run. |

## Schema version

Every workflow file carries a `schema_version` field so whatever loads it knows which shape to expect. As of Aerini 0.4.0 the format hasn't changed since it was introduced, so this is `"1.0"` everywhere and there is nothing to migrate yet. A file with no `schema_version` field at all, saved before the field existed, is treated as the oldest version this build knows and loads normally.

What happens for a version that's older, newer, or otherwise mismatched, and what runs when a migration path actually exists, is covered in [Updating §Opening older or newer workflow files](../operations/updating.md#opening-older-or-newer-workflow-files) rather than repeated here. A version-by-version history of format changes will live in `schema-migrations.md` once there is more than one version to show.

## aerini-server's exported config

Choosing **Export → Export for Server** writes a config file, `aerini-server.json`, whose `workflow_json` field holds this exact schema, unmodified, as a JSON string, the same shape the desktop app already has saved to its own database. `aerini-server serve` reads that file and loads the workflow the same way the desktop app does. See [Server Deployment](../operations/server-deploy.md) and [Server CLI Reference](../operations/server-cli-reference.md#serve) for the rest of that config file and the flags around it.

## What Export writes

**Export → Export workflow** and **Export → Export All** write this same schema, field for field, with one deliberate exception: `metadata.collection_id` is always written as `null`. Collection membership is this sidebar's local folder organization, not a portable property of the workflow itself, so it never leaves the app through an exported file.

Everything else, including `parallel_execution`, `unlimited_duration`, `max_concurrent_nodes`, `settings`, and the rest of `metadata`, is written exactly as it stands in the workflow being exported, omitted only where [Top-level fields](#top-level-fields) above already says the native format omits an at-default field. Re-importing an exported file restores all of it; the workflow lands in Uncategorized, since collection membership is the one thing an export never carries.

A `.json` file works the same as a `.aerini` file here too, see [Workflow file](../glossary.md#workflow-file).

## What's next

- [Concepts](../getting-started/concepts.md), for what a workflow, node, and connection are before reading their JSON shape
- [Nodes Reference](../guide/nodes.md), for what belongs inside a given node type's `config`
- [Credentials](../guide/credentials.md), for how the `credentials` map resolves to a real secret at run time
- `schema-migrations.md`, for a version-by-version history once the format has more than one version to show
