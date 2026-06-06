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
  SHELL_EXEC:     "shell_exec",
  CODE:           "code",
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
export const DANGEROUS_NODE_IDS: Set<string> = new Set([NODE_IDS.SHELL_EXEC, NODE_IDS.CODE, NODE_IDS.FILE]);
