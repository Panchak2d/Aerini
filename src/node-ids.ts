export const NODE_IDS = {
  SCHEDULE:       "schedule",
  WEBHOOK:        "webhook",
  MANUAL_TRIGGER: "manual_trigger",
  AI_PROMPT:      "ai_prompt",
  AI_AGENT:       "ai_agent",
  HTTP_REQUEST:   "http_request",
  SHELL_EXEC:     "shell_exec",
  CODE:           "code",
  FILE:           "file",
} as const;

export type NodeTypeId = typeof NODE_IDS[keyof typeof NODE_IDS];

export const TRIGGER_NODE_IDS:   Set<string> = new Set([NODE_IDS.SCHEDULE, NODE_IDS.WEBHOOK, NODE_IDS.MANUAL_TRIGGER]);
export const DANGEROUS_NODE_IDS: Set<string> = new Set([NODE_IDS.SHELL_EXEC, NODE_IDS.CODE, NODE_IDS.FILE]);
