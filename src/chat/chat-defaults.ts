import { NODE_IDS } from "../node-ids";

export interface AiChatFields {
  /** Config key holding the text sent to the model. */
  input: string;
  /** Output field holding the model's reply text. */
  reply: string;
}

const AI_PROMPT_FIELDS: AiChatFields = { input: "prompt", reply: "content" };

/** Chat-relevant fields for each AI node type; a type missing here isn't an AI node Chat knows how to feed. */
export const AI_CHAT_FIELDS: Readonly<Partial<Record<string, AiChatFields>>> = {
  [NODE_IDS.AI_PROMPT]: AI_PROMPT_FIELDS,
  [NODE_IDS.AI_AGENT]:  { input: "goal", reply: "result" },
};

export const AI_PROMPT_REPLY_FIELD = AI_PROMPT_FIELDS.reply;

/** The expression for the text a user typed in Chat, as the Webhook node delivers it. */
export function chatMessageExpr(webhookName: string): string {
  return `{{${webhookName}.output.body.message}}`;
}

export function isBlank(value: unknown): boolean {
  return typeof value !== "string" || value.trim() === "";
}
