# Desktop IPC Reference

This document is for people working on the Aerini desktop app itself — the Tauri shell and the TypeScript frontend. If you're building a workflow, writing a plugin, or deploying `aerini-server`, you don't need this; see the [README](../README.md) for the right guide.

Everything here is **internal**. Unlike the [REST API](api-reference.md), which is a stable contract for `aerini-server`, these are direct function calls between the TypeScript frontend (`src/`) and the Rust shell (`src-tauri/`). Names, parameters, and return shapes can change between releases without a deprecation period — if you're scripting against Aerini, use the REST API or [embed `aerini-engine`](embedding.md) directly instead of calling these.

---

## How this fits together

The frontend calls `invoke("command_name", { ...args })` from `@tauri-apps/api/core`. Rust handles it with a function annotated `#[tauri::command]` in `src-tauri/src/commands/` (or directly in `src-tauri/src/lib.rs` for a handful of app-level utilities), and returns either a value or an `Err(String)` — which `invoke()` turns into a rejected promise on the JS side.

State that commands depend on (the database, the scheduler daemon, the node registry, credential store, etc.) is registered once at startup and injected automatically — the frontend never passes these, only the arguments listed below.

---

## Workflow storage

| Command | Args | Returns | Notes |
|---|---|---|---|
| `save_workflow` | `workflow_json: string` | `()` | Upserts by the `id` embedded in the JSON. |
| `load_workflow` | `id: string` | `string \| null` | Raw workflow JSON, or `null` if no such workflow. |
| `list_workflows` | — | `WorkflowSummary[]` | For the workflow picker / sidebar list. |
| `delete_workflow` | `id: string` | `()` | Also stops any scheduled job for this workflow first — deleting a workflow that's currently running as a background job stops it, it doesn't orphan it. |

## Workflow versions

| Command | Args | Returns | Notes |
|---|---|---|---|
| `save_version` | `workflow_id: string`, `snapshot_json: string`, `message?: string` | `()` | Stores a point-in-time snapshot. `message` is an optional human note (e.g. "before refactor"). |
| `list_versions` | `workflow_id: string` | `VersionRow[]` | Metadata only (id, timestamp, message) — not the full snapshot. |
| `get_version` | `id: string` | `string \| null` | Full snapshot JSON for one version. |
| `delete_version` | `id: string` | `()` | Removes one saved version. Does not affect the live workflow. |

## Running workflows

| Command | Args | Returns | Notes |
|---|---|---|---|
| `run_workflow` | `workflow_json: string`, `initial_variables: object` | `WorkflowResult` | Runs the workflow immediately, foreground. `initial_variables` becomes `{{$vars.*}}` inside the run. This is the call behind the canvas's Run button. |
| `cancel_run` | — | `()` | Cancels the currently in-progress foreground run, if any. No-op if nothing is running — safe to call speculatively. |
| `get_node_types` | — | `NodeDescriptor[]` | Every node type the registry knows about — all 39 built-ins plus any loaded `.wasm` plugins — sorted by display name. This is what populates the palette. Each descriptor includes `is_plugin: bool`, which is what drives the "P" badge (see [Plugin Authoring](plugin-authoring.md)). |

## Run history

These back the History tab in the output drawer and the [run status](background-runs.md#run-history-and-the-interrupted-status) lifecycle.

| Command | Args | Returns | Notes |
|---|---|---|---|
| `save_run_started` | `id: string`, `workflow_id: string`, `workflow_name: string`, `ran_at: string` | `()` | Called before the workflow starts executing. Writes a record with `status = "running"`. If the app is killed before the matching `save_run_record` call, this is the row that later gets relabeled `interrupted`. |
| `save_run_record` | `record: RunRecord` | `()` | Called after the run finishes. Upserts (by `id`) over the `save_run_started` row, replacing `status` with the final `success`/`failed` and filling in duration, outputs, and logs. |
| `list_run_records` | `workflow_id: string`, `offset: number`, `limit: number`, `filter: string` | `RunRecord[]` | Paginated history for one workflow. `filter` restricts by status (e.g. `"failed"`) — pass an empty string for all statuses. |
| `delete_run_record` | `id: string` | `()` | Removes one history entry. |
| `clear_run_records` | `workflow_id: string` | `()` | Removes all history entries for a workflow. Used by the "Clear history" action — irreversible. |

## Settings (key/value)

Generic per-user settings storage — used for things like the plugin folder path, UI preferences, and onboarding flags.

| Command | Args | Returns | Notes |
|---|---|---|---|
| `get_setting` | `key: string` | `string \| null` | `null` if the key has never been set. |
| `set_setting` | `key: string`, `value: string` | `()` | Values are stored as plain strings — callers that need structured data `JSON.stringify`/`parse` themselves. |

## Credentials

Full behavior (encryption, key storage, etc.) is documented in [Credentials](credentials.md). This is just the IPC surface.

| Command | Args | Returns | Notes |
|---|---|---|---|
| `list_credentials` | — | `CredentialEntry[]` | Metadata only — IDs and names, never decrypted values. |
| `save_credential` | `req: CreateCredentialRequest` | `()` | Encrypts and stores (or updates) a credential. |
| `delete_credential` | `id: string` | `()` | Before deleting, scans all saved workflows for nodes that reference this credential ID. The deletion still proceeds — Aerini doesn't block you — but the frontend uses this to warn you which workflows will break. |

## Scheduler

Backs the Background Runs sidebar — see [Background Runs](background-runs.md).

| Command | Args | Returns | Notes |
|---|---|---|---|
| `start_scheduled_workflow` | `workflow_id: string`, `port_override?: number`, `always_on?: boolean` | `()` | Starts the workflow's trigger loop (Schedule or Webhook). `port_override` lets the UI resolve a port conflict for Webhook triggers without editing the saved workflow. |
| `stop_scheduled_workflow` | `workflow_id: string` | `()` | Stops one job. This is an immediate hard stop — not the graceful drain `aerini-server` does on `SIGTERM` (see [Execution Flow — graceful shutdown](execution-flow.md#10-graceful-shutdown), which is server-only). |
| `get_scheduled_jobs` | — | `ScheduledJobRow[]` | Current state of every job, for rendering the sidebar. |
| `set_always_on` | `workflow_id: string`, `always_on: boolean` | `()` | Persists the Always On flag; the daemon reads it on startup to auto-restart jobs. |
| `stop_all_jobs` | — | `()` | Stops every running job. Called on app quit. |
| `request_scheduler_state` | — | `()` | Asks the daemon to re-emit `scheduler-status` for every active job. The frontend calls this once after it's registered its event listener, so it gets the current state instead of waiting for the next natural status change. Replaced an earlier fixed 800ms startup delay that did the same job less reliably. |

## Export for Server

Backs the "Export for Server" panel — see [Server Deployment](server-deploy.md).

| Command | Args | Returns | Notes |
|---|---|---|---|
| `validate_workflow_for_export` | `workflow_id: string` | `ValidateResult` | Checks the workflow is exportable (has a Schedule or Webhook trigger) and scans it for credential references and `{{$vars.*}}` usages. |
| `generate_server_package` | `request: ExportRequest` | path to generated zip | Builds the systemd-based export bundle (binary, `aerini-server.json`, `.env.example`, `install.sh`). |
| `generate_docker_package` | `request: ExportRequest` | path to generated zip | Builds the Docker-based export bundle (`Dockerfile`, `docker-compose.yml`, `aerini-server.json`, `README.md`). |

**`ValidateResult` shape:**

```ts
{
  trigger_desc: string,        // human-readable description of the trigger, e.g. "Webhook on :3456/hook"
  credentials: CredentialExport[],  // one entry per credential the workflow references → AERINI_CRED_*
  variables: string[],         // every distinct {{$vars.X}} name found anywhere in the workflow → AERINI_VAR_*
}
```

`variables` is found by serializing the whole workflow to JSON and scanning for `$vars.IDENTIFIER` references, so it catches usages at any nesting depth — including inside trigger configuration, not just node parameters. If a workflow doesn't use `$vars` anywhere, this is an empty array and the "Required variables" section doesn't appear in the export panel.

## Plugins

Backs **Settings → Plugins** — see [Plugin Authoring — Installing and managing plugins](plugin-authoring.md#installing-and-managing-plugins-desktop-app) for the user-facing behavior. All three take a `plugin_dir` so the frontend is the source of truth for where that is (read via `get_setting`/`set_setting`); Rust does no path resolution beyond what's passed in.

| Command | Args | Returns | Notes |
|---|---|---|---|
| `list_installed_plugins` | `plugin_dir: string` | `PluginInfo[]` | Lists every `.wasm` file in `plugin_dir`. A directory that doesn't exist returns an empty list, not an error — so this is safe to call before the user has set a plugin folder. Files that fail to load are still included, with `load_error` set and `display_name` falling back to the filename. |
| `install_plugin_from_path` | `src_path: string`, `plugin_dir: string` | `string` (installed filename) | Validates the source has a `.wasm` extension and exists. Only the filename component of `src_path` is used for the destination — protects against a crafted path trying to write outside `plugin_dir`. Creates `plugin_dir` if it doesn't exist. **Fails if a file with the same name is already installed** — it does not overwrite; remove the old one first. |
| `remove_plugin` | `filename: string`, `plugin_dir: string` | `()` | `filename` must be a bare name (no `/`, `\`, or `..`) ending in `.wasm`. After joining with `plugin_dir`, both paths are canonicalized and the result must still be inside `plugin_dir` — defense in depth on top of the filename check. |

**`PluginInfo` shape:**

```ts
{
  filename: string,
  display_name: string,   // == filename when load_error is set
  type_id: string,         // empty string when load_error is set
  category: string,        // empty string when load_error is set
  load_error: string | null,
}
```

Two related dialog helpers exist purely to feed `install_plugin_from_path`:

| Command | Args | Returns | Notes |
|---|---|---|---|
| `pick_folder_dialog` | — | `string \| null` | Native folder picker, used for setting the plugin directory. `null` if the user cancels. |
| `pick_wasm_file_dialog` | — | `string \| null` | Native file picker pre-filtered to `.wasm`. `null` if the user cancels. |

---

## App-level utilities

Small, mostly one-off commands that don't fit a category above.

| Command | Args | Returns | Notes |
|---|---|---|---|
| `read_text_file` | `path: string` | `string` | Restricted to `.aerini` files — used for importing a workflow file. Rejects anything else after canonicalizing the path. |
| `save_file_dialog` | `content: string`, `filename: string` | path or `null` | Native "save as" dialog, writes `content` to the chosen path. |
| `save_export_zip` | `zip_path: string`, `filename: string` | path or `null` | Native "save as" for a previously generated export zip — copies the temp file to the chosen path and deletes the temp file. |
| `write_temp_file` | `filename: string`, `data: string` | `string` (temp path) | Writes to a temp directory for later use by `save_export_zip` or similar. Strips path separators and collapses `..` from `filename`. |
| `check_bundled_node` | — | `string` (version) or rejects | Spawns the bundled Node.js runtime with `--version`; resolves with its version string, or rejects with a message describing why it didn't spawn (missing/corrupt bundle) — used to show a warning banner if the Code (JS) node won't work. |
| `get_autostart` / `set_autostart` | — / `enabled: boolean` | `boolean` / `()` | "Launch on system startup" toggle, via the OS autostart mechanism. |
| `close_window` | — | `()` | Hides the main window (minimize-to-tray), doesn't quit. |
| `force_quit` | — | `()` | Stops all scheduled jobs and exits the process — the real "Quit" action. Note this is a hard stop, not a drain; any run in progress at this moment ends up `interrupted` in the history (see [Background Runs](background-runs.md#run-history-and-the-interrupted-status)). |

---

## A note on error handling

Every fallible command returns `Result<T, String>`. The `String` is a human-readable message intended to be shown more or less as-is in a toast or inline error — these aren't error codes meant for programmatic branching. If you need to distinguish error *types* in the frontend, you're working against the grain of this API; consider whether the check belongs on the Rust side instead.
