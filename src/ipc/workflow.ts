import { invoke } from "@tauri-apps/api/core";

export interface PortDefinition {
  id: string;
  label: string;
  position: "left" | "right" | "top" | "bottom";
}

export interface NodeDescriptor {
  type_id: string;
  display_name: string;
  node_type: "action" | "ai" | "logic" | "utility";
  version: string;
  input_schema: unknown;
  output_schema: unknown;
  ports: { inputs: PortDefinition[]; outputs: PortDefinition[] };
  /** True when this node's ports are derived from config, not static. */
  dynamic_ports?: boolean;
  /** True when this node was loaded from a WASM plugin rather than built in. */
  is_plugin?: boolean;
}

export interface WorkflowLogEntry {
  timestamp: string;
  node_id?: string;
  level: "info" | "warn" | "error";
  message: string;
}

export interface WorkflowResult {
  execution_id: string;
  workflow_id: string;
  success: boolean;
  node_outputs: Record<string, unknown>;
  logs: WorkflowLogEntry[];
  error?: string;
}

export interface WorkflowSummary {
  id: string;
  name: string;
  updated_at: string;
}

export const listWorkflows  = () => invoke<WorkflowSummary[]>("list_workflows");
export const saveWorkflow   = (json: string) => invoke<void>("save_workflow", { workflowJson: json });
export const loadWorkflow   = (id: string) => invoke<string | null>("load_workflow", { id });
export const deleteWorkflow = (id: string) => invoke<void>("delete_workflow", { id });
export const getNodeTypes   = () => invoke<NodeDescriptor[]>("get_node_types");

export const runWorkflow = (
  workflowJson: string,
  initialVariables: Record<string, unknown> = {}
) => invoke<WorkflowResult>("run_workflow", { workflowJson, initialVariables });

export const cancelRun = () => invoke<void>("cancel_run");

export interface ScheduledJobRow {
  workflow_id:   string;
  workflow_name: string;
  trigger_kind:  string;     // JSON-serialised TriggerKind
  status:        "active" | "paused" | "done" | "error" | "stopped";
  always_on:     boolean;
  run_count:     number;
  last_run_at:   string | null;
  next_run_at:   string | null;
  last_error:    string | null;
  created_at:    string;
}

// Typed scheduler error — mirrors Rust SchedulerError enum.
export type SchedulerError =
  | { error_kind: "port_conflict"; port: number; held_by_workflow_id: string; held_by_workflow_name: string }
  | { error_kind: "workflow_not_found" }
  | { error_kind: "not_schedulable" }
  | { error_kind: "already_running" }
  | { error_kind: "other"; message: string };

export function parseSchedulerError(raw: string): SchedulerError {
  try { return JSON.parse(raw) as SchedulerError; }
  catch { return { error_kind: "other", message: raw }; }
}

export const startScheduledWorkflow = (workflowId: string, portOverride?: number, alwaysOn?: boolean) =>
  invoke<void>("start_scheduled_workflow", {
    workflowId,
    portOverride: portOverride ?? null,
    alwaysOn: alwaysOn ?? null,
  });

export const stopScheduledWorkflow = (workflowId: string) =>
  invoke<void>("stop_scheduled_workflow", { workflowId });

export const getScheduledJobs = () =>
  invoke<ScheduledJobRow[]>("get_scheduled_jobs");

export const setAlwaysOn = (workflowId: string, alwaysOn: boolean) =>
  invoke<void>("set_always_on", { workflowId, alwaysOn });

export const forceQuit = () => invoke<void>("force_quit");

export const getSetting = (key: string) =>
  invoke<string | null>("get_setting", { key });

export const setSetting = (key: string, value: string) =>
  invoke<void>("set_setting", { key, value });

export interface VersionRow {
  id:          string;
  workflow_id: string;
  message:     string | null;
  created_at:  string;
}

export const saveVersion = (workflowId: string, snapshotJson: string, message?: string) =>
  invoke<void>("save_version", {
    workflowId,
    snapshotJson,
    message: message ?? null,
  });

export const listVersions = (workflowId: string) =>
  invoke<VersionRow[]>("list_versions", { workflowId });

export const getVersion = (id: string) =>
  invoke<string | null>("get_version", { id });

export const deleteVersion = (id: string) =>
  invoke<void>("delete_version", { id });

export const checkNodejsAvailable = () =>
  invoke<boolean>("check_nodejs_available");

export interface PluginInfo {
  filename: string;
  display_name: string;
  type_id: string;
  category: string;
  load_error: string | null;
}

export const pickFolderDialog = () =>
  invoke<string | null>("pick_folder_dialog");

export const pickWasmFileDialog = () =>
  invoke<string | null>("pick_wasm_file_dialog");

export const listInstalledPlugins = (pluginDir: string) =>
  invoke<PluginInfo[]>("list_installed_plugins", { pluginDir });

export const installPluginFromPath = (srcPath: string, pluginDir: string) =>
  invoke<string>("install_plugin_from_path", { srcPath, pluginDir });

export const removePlugin = (filename: string, pluginDir: string) =>
  invoke<void>("remove_plugin", { filename, pluginDir });
