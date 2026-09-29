# Desktop IPC Reference

The desktop app's frontend never touches the filesystem, a database, or the Rust engine directly. Every one of those operations crosses through Tauri's IPC bridge as a named command, registered once in `src-tauri/src/lib.rs`'s `invoke_handler!` list. This page is the exhaustive list: every registered command, its parameters, what it returns, and where in the frontend it's actually called from. It assumes you've read [Architecture](architecture.md), specifically the [Tauri shell](architecture.md#the-tauri-shell-src-tauri) section this page is the detail behind.

74 commands are registered today, spread across eleven modules under `src-tauri/src/commands/` (`workflow`, `chat`, `credentials`, `oauth`, `scheduler`, `export`, `plugins`, `providers`, `memory`, `performance`, `update`) plus 14 commands declared directly in `lib.rs` that don't belong to any one domain.

## Conventions

**Calling a command.** The frontend calls `invoke("command_name", { param1, param2 })` from `@tauri-apps/api/core`. Tauri converts each Rust parameter's `snake_case` name to `camelCase` on the JS side automatically; a Rust parameter named `workflow_id` is passed as `workflowId` in the `invoke()` call. This page names parameters in their Rust form and notes the JS call shape only where it's easy to get wrong.

**Where a command is actually called from.** `architecture.md` describes each command module as mapping closely to one file under `src/ipc/`. That holds for `chat.rs`, `credentials.rs`, `oauth.rs`, `update.rs`, and most of `workflow.rs`, but not for the rest: `scheduler.rs` and `plugins.rs` commands are wrapped in `src/ipc/workflow.ts` rather than files of their own; `export.rs`'s three package-generating commands are called with a bare `invoke()` directly from `src/export-server-panel.ts`; `workflow.rs`'s six run-history commands are called directly from `src/run-history.ts`; and the 14 top-level `lib.rs` commands are scattered across `src/drag-drop.ts`, `src/workflow-manager.ts`, `src/toolbar.ts`, `src/app.ts`, `src/output-renderer.ts`, `src/ipc/autostart.ts`, and `src/ipc/workflow.ts`. There is no single file that owns "the IPC layer"; this page's own "Frontend caller" column is the authoritative map, not the module name.

**Error shape.** The overwhelming majority of commands return `Result<T, String>`, and on failure the frontend's `invoke()` promise rejects with that plain string as the reason (`catch (err) { /* err is a string */ }`). Three deliberate exceptions:

- `scheduler::start_scheduled_workflow` serializes its underlying `SchedulerError` enum to a JSON string (`serde_json::to_string`, falling back to `format!("{:?}", e)` if that itself fails) instead of a human message. `src/ipc/workflow.ts`'s `parseSchedulerError()` parses it back into a typed union (`port_conflict`, `workflow_not_found`, `not_schedulable`, `already_running`, or `other`), falling back to `{ error_kind: "other", message: raw }` if parsing fails. Every other scheduler command (`stop_scheduled_workflow`, `set_always_on`, `stop_all_jobs`, `request_scheduler_state`) returns a plain string like everything else; only this one command is JSON-structured.
- `plugins::install_plugin_from_path` and `install_plugin_pack_from_path` embed a machine-readable, colon-terminated prefix in an otherwise plain error string, for example `plugin_already_installed: 'x' is already installed as 'x.wasm'`. Seven distinct prefixes exist (`plugin_already_installed`, `plugin_integrity_failed`, `plugin_signature_invalid`, `plugin_key_mismatch`, `plugin_signature_downgrade`, `pack_already_installed`, `pack_member_conflict`), but `src/plugin-settings.ts` only pattern-matches two of them, `plugin_already_installed:` and `pack_already_installed:`, to offer an update-and-retry-with-`overwrite`-confirmation. The other five prefixes still get returned this way but are shown to the user as plain error text with no special handling.
- A handful of read-only commands with nothing that can fail return their value directly with no `Result` at all: `memory::get_memory_breakdown` (`Vec<RunBreakdown>`), `memory::get_process_memory` and `performance::get_live_performance`/`get_recent_performance` (`Option<T>`, `None` meaning "nothing to report" rather than an error).

**Async commands and blocking work.** Most commands that touch the workflow database are `async fn` that immediately hand the actual work to `tokio::task::spawn_blocking`, since `WorkflowDb`'s own methods are synchronous SQLite calls. The `.await` only resolves once that blocking work finishes; a failure to join the spawned task at all (for example, a panic inside it) is converted to a plain string and returned the same way as any other error, indistinguishable to the frontend from a normal `Result::Err`.

## Workflow

`src-tauri/src/commands/workflow.rs` (378 lines) is the largest module by command count: 19 commands covering workflow CRUD and execution, run history, versions, and two commands that don't fit either category. All but the run-history group are wrapped in `src/ipc/workflow.ts`.

### CRUD and execution

| Command | Parameters | Returns | Frontend caller |
|---|---|---|---|
| `save_workflow` | `workflow_json: String` | `Result<(), String>` | `saveWorkflow` (`ipc/workflow.ts`) |
| `load_workflow` | `id: String` | `Result<Option<String>, String>` | `loadWorkflow` (`ipc/workflow.ts`) |
| `list_workflows` | none | `Result<Vec<WorkflowSummary>, String>` | `listWorkflows` (`ipc/workflow.ts`) |
| `delete_workflow` | `id: String` | `Result<(), String>` | `deleteWorkflow` (`ipc/workflow.ts`) |
| `run_workflow` | `workflow_json: String`, `initial_variables: HashMap<String, Value>`, `run_id: Option<String>` | `Result<WorkflowResult, String>` | `runWorkflow` (`ipc/workflow.ts`) |
| `get_node_types` | none | `Result<Vec<NodeDescriptor>, String>` | `getNodeTypes` (`ipc/workflow.ts`) |

`save_workflow` parses `workflow_json` into a `Workflow` before saving; a malformed payload fails with the parse error, not a database error. `load_workflow` returns `Ok(None)`, not an error, for an id that doesn't exist; `Ok(Some(json))` re-serializes the loaded row with `to_json_pretty`, so what comes back is not byte-identical to what was last saved.

`run_workflow` does the most work of any command in this file:

- Generates its own `run_id` (a UUID) when the caller omits one. Omit it for a fire-and-forget run (a popover node preview) that will never need to be cancelled by id; the frontend's own `cancelRun` convention requires a known `run_id` to target, so any run that should be independently stoppable must pass one.
- Rejects a second concurrent run of the same `workflow_id` with a 5-second wait on an internal exec lock before failing, matching the equivalent reject-if-busy behavior in `aerini-server`'s own run route. A single-node test run uses a distinct `${workflow_id}_sub` id internally, so it never contends with a full run of the same open canvas.
- Always sets `caller_is_admin(true)` on the executor, since the desktop app is single-tenant: whoever is running a workflow on their own machine already has full access to it. This unlocks node-level admin gates (for example, a Database node's `allow_raw_sql`) unconditionally on desktop, unlike `aerini-server`, where the same gate depends on the calling token's scope.
- A cancelled run (via `cancel_run`, below) does not surface as an `Err`. It resolves `Ok` with a `WorkflowResult` whose `success` is `false` and `error` is `"Run cancelled by user"`, the same shape a normal failed run would have.

### Run history

| Command | Parameters | Returns | Frontend caller |
|---|---|---|---|
| `save_run_started` | `id: String`, `workflow_id: String`, `workflow_name: String`, `ran_at: String` | `Result<(), String>` | `saveRunStarted` (`src/run-history.ts`) |
| `save_run_record` | `record: RunRecord` | `Result<(), String>` | `saveRunToHistory`, and the local-storage migration path (`src/run-history.ts`) |
| `save_performance_report` | `run_id: String`, `workflow_id: String` | `Result<(), String>` | `saveRunToHistory` (`src/run-history.ts`), called immediately after `save_run_record` |
| `list_run_records` | `workflow_id: String`, `offset: i64`, `limit: i64`, `filter: String` | `Result<Vec<RunRecord>, String>` | `loadPage` (`src/run-history.ts`) |
| `delete_run_record` | `id: String` | `Result<(), String>` | `deleteRunRecord` (`src/run-history.ts`) |
| `clear_run_records` | `workflow_id: String` | `Result<(), String>` | `clearRunRecords` (`src/run-history.ts`) |

None of these six are wrapped in `src/ipc/workflow.ts`; `src/run-history.ts` calls all of them with a bare `invoke()`. `save_run_record` and `save_performance_report` are both called, in that order, every time a run finishes; a failure from either is caught and logged to the console, not surfaced to the user, so a failed history write never blocks or interrupts the run itself.

`save_performance_report` deliberately does not accept a caller-supplied report. It fetches the frozen report the engine already produced for that `run_id` via `perf_monitor::get_recent_report` and persists that. If no matching report exists yet, which can only happen on a stale or duplicate call given `run_workflow`'s own locking, it is treated as a no-op and returns `Ok(())`, not an error.

### Versions

| Command | Parameters | Returns | Frontend caller |
|---|---|---|---|
| `save_version` | `workflow_id: String`, `snapshot_json: String`, `message: Option<String>` | `Result<(), String>` | `saveVersion` (`ipc/workflow.ts`) |
| `list_versions` | `workflow_id: String` | `Result<Vec<VersionRow>, String>` | `listVersions` (`ipc/workflow.ts`) |
| `get_version` | `id: String` | `Result<Option<String>, String>` | `getVersion` (`ipc/workflow.ts`) |
| `delete_version` | `id: String` | `Result<(), String>` | `deleteVersion` (`ipc/workflow.ts`) |

### Settings and chat session

| Command | Parameters | Returns | Frontend caller |
|---|---|---|---|
| `get_setting` | `key: String` | `Result<Option<String>, String>` | `getSetting` (`ipc/workflow.ts`) |
| `set_setting` | `key: String`, `value: String` | `Result<(), String>` | `setSetting` (`ipc/workflow.ts`) |
| `clear_chat_session` | `session_id: String` | `Result<(), String>` | `clearChatSession` (`ipc/workflow.ts`) |

`clear_chat_session` clears AI Memory rows for a chat session by looking up the registered `ai_memory` node type and invoking its `execute` directly with a synthetic `NodeInput`, rather than opening a second database connection to reimplement the delete. This is the one command in the whole surface that runs a node outside an actual workflow run.

## Chat

`chat.rs` (71 lines), all six commands wrapped in `src/ipc/chat.ts`.

| Command | Parameters | Returns | Frontend caller |
|---|---|---|---|
| `list_chat_session_meta` | `workflow_id: String` | `Result<Vec<ChatSessionMeta>, String>` | `listChatSessionMeta` |
| `load_chat_messages` | `session_id: String` | `Result<Vec<ChatMessageRecord>, String>` | `loadChatMessages` |
| `append_chat_message` | `session: ChatSessionMeta`, `message: ChatMessageRecord` | `Result<(), String>` | `appendChatMessage` |
| `save_chat_session_meta` | `session: ChatSessionMeta` | `Result<(), String>` | `saveChatSessionMeta` |
| `save_chat_session` | `session: ChatSessionRecord` | `Result<(), String>` | `saveChatSession` |
| `delete_chat_session` | `id: String` | `Result<(), String>` | `deleteChatSession` |

`list_chat_session_meta` returns each session's id, name, and creation time without its messages; `load_chat_messages` fetches one session's messages on demand. `append_chat_message` upserts the session row and inserts one message in a single transaction. Repeating a call with the same message id does not add a second row, so a failed send can be retried safely; an id already owned by another session is rejected and the whole call rolls back. `save_chat_session_meta` saves a session with no messages yet. `save_chat_session` upserts a session's metadata and syncs its message list to the list sent, in one transaction: new message ids are inserted, rows whose content changed are rewritten, unchanged rows are left alone, and ids no longer present are deleted. The chat panel now calls it only for the one-time migration of history from older browser storage. `delete_chat_session` relies on a foreign-key cascade to remove the session's messages.

## Credentials

`credentials.rs` (103 lines), all seven commands wrapped in `src/ipc/credentials.ts`.

| Command | Parameters | Returns | Frontend caller |
|---|---|---|---|
| `list_credentials` | none | `Result<Vec<CredentialEntry>, String>` | `listCredentials` |
| `list_credential_usage` | none | `Result<HashMap<String, Vec<String>>, String>` | `listCredentialUsage` |
| `get_credential_metadata` | `id: String` | `Result<Option<CredentialMetadata>, String>` | `getCredentialMetadata` |
| `get_credential_secret` | `id: String` | `Result<Option<String>, String>` | `getCredentialSecret` |
| `save_credential` | `req: CreateCredentialRequest` | `Result<(), String>` | `saveCredential` |
| `delete_credential` | `id: String` | `Result<(), String>` | `deleteCredential` |
| `export_encryption_key` | none | `Result<String, String>` | `exportEncryptionKey` |

`list_credential_usage` scans every saved workflow once and maps each credential id to the distinct workflow names referencing it; it shares that same scan function with `delete_credential`'s own guard, so both read the same signal instead of the two drifting apart independently. `delete_credential` refuses to delete (returning `Err` with the count and names) a credential still referenced by at least one workflow. `export_encryption_key` returns the raw AES-256 credential-encryption key, base64-encoded; it's the only recovery path if the OS keychain entry holding it is ever lost, and it is a thin pass-through with no logic of its own beyond that encoding.

## OAuth

`oauth.rs` (17 lines), one command, wrapped in `src/ipc/oauth.ts`.

| Command | Parameters | Returns | Frontend caller |
|---|---|---|---|
| `get_oauth_redirect_port` | none | `Result<u16, String>` | `getOAuthRedirectPort` |

Reports the port an OAuth callback listener would actually bind if a flow started right now: `42069` if free, otherwise whatever port the OS assigns instead. `SocialSetupGuide.ts` shows this as the redirect URI a social-platform or Google Sheets OAuth app should be configured with, since a mismatch between the port shown and the port actually used would make the provider reject the callback.

## Scheduler

`scheduler.rs` (70 lines), seven commands. All seven are wrapped in `src/ipc/workflow.ts`, not a `scheduler.ts` file of their own, despite being a distinct module in Rust.

| Command | Parameters | Returns | Frontend caller |
|---|---|---|---|
| `start_scheduled_workflow` | `workflow_id: String`, `port_override: Option<u16>`, `always_on: Option<bool>` | `Result<(), String>` (JSON-structured, see [Conventions](#conventions)) | `startScheduledWorkflow` |
| `stop_scheduled_workflow` | `workflow_id: String` | `Result<(), String>` | `stopScheduledWorkflow` |
| `get_scheduled_jobs` | none | `Result<Vec<ScheduledJobRow>, String>` | `getScheduledJobs` (5-second client-side timeout) |
| `get_scheduled_job` | `workflow_id: String` | `Result<Option<ScheduledJobRow>, String>` | `getScheduledJob` (same timeout) |
| `set_always_on` | `workflow_id: String`, `always_on: bool` | `Result<(), String>` | `setAlwaysOn` |
| `stop_all_jobs` | none | `Result<(), String>` | none: registered, no frontend caller |
| `request_scheduler_state` | none | `Result<(), String>` | `requestSchedulerState` (`ipc/events.ts`) |

`get_scheduled_jobs` and `get_scheduled_job` both redact a job's webhook secret before returning its row; the scheduler's own in-process listener keeps the real value for its HMAC comparison, but it never crosses IPC. `stop_all_jobs` stops every active scheduled job; it's registered and callable but has no current frontend caller, since `force_quit` (below) and workflow deletion both call the underlying `SchedulerDaemon::stop_all()` method directly in-process rather than through this command. `request_scheduler_state` is called once, after the frontend's `scheduler-status` event listener is registered, to have the backend replay current state for every active job and emit `scheduler-ready`; it replaced an earlier fixed startup delay.

## Export

`export.rs` (499 lines plus a 668-line internal `templates` submodule with no commands of its own), four commands. None are wrapped in `src/ipc/`; all four are called with a bare `invoke()` from `src/export-server-panel.ts` and `src/toolbar.ts`.

| Command | Parameters | Returns | Frontend caller |
|---|---|---|---|
| `validate_workflow_for_export` | `workflow_id: String` | `Result<ValidateResult, String>` | `export-server-panel.ts` |
| `generate_server_package` | `request: ExportRequest { workflow_id: String, status_port: u16 }` | `Result<ExportResult, String>` | `export-server-panel.ts` |
| `generate_docker_package` | `request: ExportRequest` | `Result<ExportResult, String>` | `export-server-panel.ts` |
| `export_all_workflows` | none | `Result<ExportAllResult, String>` | `exportAllWorkflows` (`ipc/workflow.ts`), called from `toolbar.ts` |

`validate_workflow_for_export` and both `generate_*_package` commands reject a workflow whose only trigger is a Manual Trigger, since nothing would ever start it once exported; a Schedule or Webhook trigger is required. Both package commands build their zip in the system temp directory and return its path; the frontend then calls the separate top-level `save_export_zip` command (below) to move it to a user-chosen location via a save dialog. `ExportResult.run_secret_plaintext` is the one and only time the generated run secret is available in plaintext; the exported config file stores only its argon2id hash, and the UI has no other way to retrieve it later. `export_all_workflows` bundles every saved workflow into one zip of individual `.aerini` files, the same per-workflow JSON shape a single-workflow export produces, with each workflow's `metadata.collection_id` nulled out the same way a single export already does.

## Plugins

`plugins.rs` (1,676 lines, by far the largest command file), six commands, all wrapped in `src/ipc/workflow.ts` rather than a `plugins.ts` file of their own.

| Command | Parameters | Returns | Frontend caller |
|---|---|---|---|
| `list_installed_plugins` | `plugin_dir: String` | `Result<Vec<PluginInfo>, String>` | `listInstalledPlugins` |
| `install_plugin_from_path` | `src_path: String`, `plugin_dir: String`, `overwrite: bool` | `Result<String, String>` | `installPluginFromPath`, via `installWithUpdatePrompt` (`src/plugin-settings.ts`) |
| `install_plugin_pack_from_path` | `src_path: String`, `plugin_dir: String`, `overwrite: bool` | `Result<String, String>` | `installPluginPackFromPath`, via `installPackWithUpdatePrompt` (`src/plugin-settings.ts`) |
| `remove_plugin` | `filename: String`, `plugin_dir: String` | `Result<(), String>` | `removePlugin` (`src/plugin-settings.ts`) |
| `remove_plugin_pack` | `pack_id: String`, `plugin_dir: String` | `Result<(), String>` | `removePluginPack` (`src/plugin-settings.ts`) |
| `reload_plugins` | `plugin_dir: String` | `Result<PluginLoadReport, String>` | `reloadPlugins`, called after every successful install or removal (`src/plugin-settings.ts`) |

`list_installed_plugins` returns an empty list, not an error, for a `plugin_dir` that doesn't exist yet. Each `PluginInfo` carries a `load_error` (set when the file failed to describe at all), a `registry_warning` (set when the plugin's `type_id` lost a collision against a built-in or another plugin at last reload, so the file is installed but inactive), and a `signature_status` string for display, independently of each other.

`install_plugin_from_path` and `install_plugin_pack_from_path` share the same ordering: every signature and trust check runs before any file on disk is touched. See [Conventions](#conventions) for the prefixed error strings both can return, and [Writing a Plugin Node](plugin-authoring.md) for the trust-on-first-use model those checks enforce. `overwrite` must be explicitly passed `true` to replace an existing install; without it, a matching existing plugin or pack fails with a `plugin_already_installed:`/`pack_already_installed:` prefixed error rather than silently updating.

`reload_plugins` rebuilds the node registry from scratch (built-ins plus every `.wasm` in `plugin_dir`) and swaps it in atomically, no restart needed; a workflow run already in progress keeps executing against the registry it started with. An internal lock serializes overlapping reload calls so a slower call started first can't finish after, and overwrite, a faster one started later.

Two more commands the Plugins settings panel depends on, `pick_folder_dialog` and `pick_plugin_file_dialog`, are not in this file. They're declared at the top level in `lib.rs`; see [Top-level commands](#top-level-commands) below.

## Memory

`memory.rs` (45 lines), two commands, both wrapped in `src/ipc/memory.ts`. Neither returns a `Result`; see [Conventions](#conventions).

| Command | Parameters | Returns | Frontend caller |
|---|---|---|---|
| `get_memory_breakdown` | none | `Vec<RunBreakdown>` | `getMemoryBreakdown` |
| `get_process_memory` | none | `Option<ProcessMemory>` | `getProcessMemory` |

`get_memory_breakdown` is a synchronous, stateless read over a global allocator-tracking registry that the same periodic `memory-breakdown` event (emitted roughly every 2 seconds while at least one run is in flight, from `lib.rs`'s own setup code) also reads from; this command exists for the frontend's first paint before that event's first tick lands, and for an on-demand refresh. An empty `Vec` is the real "nothing running" state, not a stale read, since tracking is disabled entirely at idle. `get_process_memory` reports this process's own resident set size independent of the per-run breakdown above; it returns `None` only if the current process id can't be resolved on the current platform.

## Performance

`performance.rs` (80 lines), six commands. Two return their value directly with no `Result` (see [Conventions](#conventions)); the other four wrap the `performance_reports` SQLite table.

| Command | Parameters | Returns | Frontend caller |
|---|---|---|---|
| `get_live_performance` | `workflow_id: String` | `Option<PerformanceReport>` | `getLivePerformance` (`ipc/performance.ts`) |
| `get_recent_performance` | `workflow_id: String` | `Option<PerformanceReport>` | `getRecentPerformance` (`ipc/performance.ts`) |
| `get_performance_report` | `run_id: String` | `Result<Option<PerformanceReportRecord>, String>` | none: registered, no frontend caller |
| `list_performance_reports` | `workflow_id: String`, `offset: i64`, `limit: i64` | `Result<Vec<PerformanceReportRecord>, String>` | none: registered, no frontend caller |
| `delete_performance_report` | `run_id: String` | `Result<(), String>` | none: registered, no frontend caller |
| `clear_performance_reports` | `workflow_id: String` | `Result<(), String>` | none: registered, no frontend caller |

`get_live_performance` and `get_recent_performance` are, despite the module's own doc comment claiming otherwise, in active use today: `src/panels/PerformancePanel.ts`, `src/panels/MonitorPanel.ts`, and `src/statusbar-fields.ts` all call `getRecentPerformance`, and `PerformancePanel.ts` also calls `getLivePerformance` and listens for the `performance-live` event `ipc/performance.ts` exposes alongside them. The doc comment is stale for these two. The remaining four commands, which read and write the persisted `performance_reports` table rather than the in-memory live/recent snapshots, genuinely have no frontend caller yet, confirmed by a repository-wide search rather than assumed from the comment.

## Update

`update.rs` (79 lines), one command, wrapped in `src/ipc/update.ts`.

| Command | Parameters | Returns | Frontend caller |
|---|---|---|---|
| `check_for_update` | none | `Result<UpdateCheckResult, String>` | `checkForUpdate`, called from `src/toolbar.ts` |

User-triggered only; nothing polls or schedules this automatically. Calls the GitHub Releases API directly, validates that the release URL it gets back actually points at `github.com` over `https` before returning it, and compares semantic versions to compute `is_newer`.

## Providers

`providers.rs` (102 lines), one command.

| Command | Parameters | Returns | Frontend caller |
|---|---|---|---|
| `list_provider_models` | `provider: String`, `base_url: String`, `api_key: String` | `Result<Vec<String>, String>` | `listProviderModels` (`src/ipc/providers.ts`), called from the "Fetch Models" button on `model` fields flagged `x-aerini-model-picker` (`field-renderer.ts`/`lifecycle.ts`), and from the Credential panel's own Advanced "Fetch Models" button (`CredentialPanel.ts`), which reads provider/base URL/secret value off its own form fields instead of a node's config |

Discovers a provider's available models by calling its `/models`-style endpoint. Thin wrapper: `aerini_engine::nodes::ai_prompt::list_models` does the actual work — resolving `base_url` the same way (and under the same `SsrfPolicy::AllowLocal` SSRF check) the AI Prompt node's own `execute()` does, then dispatching to a per-provider parser. `openai` and `local` share one parser (`{"data":[{"id":...}]}`); `anthropic` uses the same shape at `/v1/models`; `gemini` has a different envelope (`{"models":[{"name":"models/xxx"}]}`) and strips the `"models/"` prefix so all three return bare model ids. `provider` also accepts `"auto"` (and anything else not already one of the four ids): the command resolves it via `ProviderRegistry::detect_from_url(base_url)` first, the same classifier `execute()` uses, so the AI nodes' own default `provider` setting can discover models too.

## Top-level commands

14 commands declared directly in `lib.rs` rather than under `commands/`, because none belongs to one of the eleven domains above. Callers are scattered across several frontend files; there is no single wrapper file for this group.

| Command | Parameters | Returns | Frontend caller |
|---|---|---|---|
| `show_main_window` | none (takes a `tauri::Window`) | `Result<(), String>` | `app.ts`, called once the frontend has painted its first real frame |
| `read_text_file` | `path: String` | `Result<String, String>` | `drag-drop.ts` |
| `save_file_dialog` | `content: String`, `filename: String` | `Result<String, String>` | `workflow-manager.ts` |
| `save_export_zip` | `zip_path: String`, `filename: String` | `Result<String, String>` | `export-server-panel.ts`, `toolbar.ts` |
| `close_window` | none | none (no return value) | `ipc/events.ts`, on the app's close-requested event |
| `force_quit` | none | none (no return value) | none: registered, no frontend caller |
| `check_bundled_node` | none | `Result<String, String>` | `checkBundledNode` (`ipc/workflow.ts`), called from `app.ts` |
| `cancel_run` | `run_id: String` | none (no return value) | `cancelRun` (`ipc/workflow.ts`), called from `src/run-manager/` |
| `get_autostart` | none | `bool` | `getAutostart` (`ipc/autostart.ts`), called from `toolbar.ts` |
| `set_autostart` | `enabled: bool` | `Result<(), String>` | `setAutostart` (`ipc/autostart.ts`), called from `toolbar.ts` |
| `pick_folder_dialog` | none | `Option<String>` | `pickFolderDialog` (`ipc/workflow.ts`), called from `plugin-settings.ts` |
| `pick_plugin_file_dialog` | none | `Option<String>` | `pickPluginFileDialog` (`ipc/workflow.ts`), called from `plugin-settings.ts` |
| `pick_file_dialog` | none | `Option<String>` | `src/popover/field-renderer.ts`, a bare `invoke()` behind the "Choose File…" button |
| `write_temp_file` | `filename: String`, `data: String` (base64) | `Result<String, String>` | `output-renderer.ts`, for rendering video/audio output |

`read_text_file` only opens a path ending in `.aerini` or `.json`, rejecting anything else outright, and only after canonicalizing the path first. `save_export_zip` requires its `zip_path` to canonicalize to somewhere inside the system temp directory, refusing anything else; it always deletes the source temp file afterward, whether the save succeeded or the user cancelled the dialog. `write_temp_file` strips `/`, `\`, and any `..` sequence from the filename before writing, caps the decoded payload at 50 MiB, and writes into a fixed `aerini_media` subdirectory of the system temp directory, never a caller-chosen location. `pick_folder_dialog` and `pick_plugin_file_dialog` both return `None`, not an error, when the user cancels the native dialog; `pick_plugin_file_dialog` filters the picker to `.wasm` and `.aerinipkg` files.

## What this page doesn't cover

[Architecture](architecture.md#the-tauri-shell-src-tauri) covers what `run()` does before any command can be called at all: opening the database, building the initial node registry, starting the scheduler daemon. That startup sequence isn't repeated here. [Adding a Built-in Node](node-authoring.md) and [Writing a Plugin Node](plugin-authoring.md) each mention the specific commands their own subject matter calls into (a node's config panel opening a file picker or starting an OAuth flow; the plugin install commands) in their own context; this page is the place for the exact parameter and return shape, not a restatement of when or why a node or plugin would call one.

`src/ipc/events.ts` and the event listeners embedded directly in `ipc/memory.ts` and `ipc/performance.ts` are deliberately not catalogued as a command surface here. Every table above documents a Tauri command: a named, invoked, request/response call the frontend initiates. Events run the other direction, they're pushed by the engine through `TauriEventSink`'s `tauri::Emitter::emit` calls (`node-status`, `scheduler-status`, `scheduler-skip`, `memory-breakdown`, `performance-live`) and picked up by a `win.listen()` call wherever the frontend needs that data, with no parameters, no return value, and no caller-initiated request to document. That channel already has its own explanation in [Architecture](architecture.md#the-tauri-shell-src-tauri); repeating it here would misfile a one-way notification stream under a page about two-way command calls.

## What's next

- [Architecture](architecture.md), if you haven't already read what this page assumes
- [Adding a Built-in Node](node-authoring.md), for a node's config panel calling into any of the commands above
- [Writing a Plugin Node](plugin-authoring.md), for the install/reload commands' role in the Plugins panel
- [Testing](testing.md), for where this codebase's tests live and how to run them
