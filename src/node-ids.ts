export const NODE_IDS = {
  // Triggers
  SCHEDULE:       "schedule",
  WEBHOOK:        "webhook",
  MANUAL_TRIGGER: "manual_trigger",
  // AI
  AI_PROMPT:      "ai_prompt",
  AI_AGENT:       "ai_agent",
  AI_MEMORY:      "ai_memory",
  TEXT_SPLITTER:  "text_splitter",
  IMAGE_GEN:      "image_gen",
  // Integrations
  HTTP_REQUEST:   "http_request",
  EMAIL_SEND:     "email_send",
  // Files
  TEXT_TO_FILE:   "text_to_file",
  SHELL_EXEC:     "shell_exec",
  CODE:           "code",
  DATABASE:       "database",
  FILE:           "file",
  SAVE_TO_FOLDER: "save_to_folder",
  COLLECT_FILES:  "collect_files",
  SOCIAL_UPLOAD:  "social_upload",
  // Logic
  IF_CONDITION:   "if_condition",
  SWITCH:         "switch",
  LOOP:           "loop",
  STOP:           "stop",
  MERGE:          "merge",
  // Utility
  DELAY:          "delay",
  WAIT:           "wait",
  TRANSFORM:      "transform",
  TRANSFORM_DATA: "transform_data",
  JSON_NODE:      "json",
  SET_VARIABLE:   "set_variable",
  GET_VARIABLE:   "get_variable",
  OUTPUT:         "output",
  NOTE:           "note",
} as const;

export type NodeTypeId = typeof NODE_IDS[keyof typeof NODE_IDS];

export const TRIGGER_NODE_IDS:   Set<string> = new Set([NODE_IDS.SCHEDULE, NODE_IDS.WEBHOOK, NODE_IDS.MANUAL_TRIGGER]);

// Mirrors aerini_engine::nodes::DANGEROUS_NODE_TYPE_IDS (aerini-engine/src/nodes/mod.rs,
// Batch L) exactly, kept in sync by hand per that constant's own doc comment — the two
// crates share no build step. `file` deliberately dropped, not carried forward as a
// UI-only extra: the Rust canonical list never gated or warned on File nodes at any of
// its execution entry points, so this list previously warned on the one node the backend
// doesn't and stayed silent on the one it does (AUDIT_REPORT.md S8-1/S9-4). Whether File's
// own unrestricted-path behavior (S3's "full host access by design" framing) warrants a
// warning of its own is a separate, unopened question — S8-1's own fix note offered two
// directions (gate File server-side too, or document why it's treated differently) and
// neither has been decided; not resolved here, not silently assumed either way.
// Consumers are cosmetic/advisory only, never an execution gate: validation.ts's
// checkDangerousNodes (run confirmation) and Node.ts's canvas badge (Batch M).
export const DANGEROUS_NODE_IDS: Set<string> = new Set([NODE_IDS.SHELL_EXEC, NODE_IDS.CODE, NODE_IDS.DATABASE]);
