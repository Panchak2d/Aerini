import { DANGEROUS_NODE_IDS, NODE_IDS } from "./node-ids";

/**
 * Static description fallback for nodes that do not yet carry a description
 * from the Rust engine (e.g. frontend-only canvas nodes, or plugin nodes
 * authored before the description() trait method existed).
 *
 * Keys are type_id values. The Rust-sourced description (NodeDescriptor.description)
 * takes priority; this map is only consulted when that field is absent or empty.
 */
export const NODE_DESCRIPTION_FALLBACK: Readonly<Record<string, string>> = {
  note:           "A text annotation on the canvas. No inputs or outputs — does not affect workflow execution.",
};

const SUMMARY_MAX = 200;

export function firstSentence(text: string): string {
  const flat = text.replace(/\s+/g, " ").trim();
  const end = flat.search(/[.!?](\s|$)/);
  const sentence = end === -1 ? flat : flat.slice(0, end + 1);
  return sentence.length > SUMMARY_MAX ? sentence.slice(0, SUMMARY_MAX).trimEnd() : sentence;
}

const BEGINNER_NODE_IDS: ReadonlySet<string> = new Set([
  NODE_IDS.MANUAL_TRIGGER, NODE_IDS.SCHEDULE, NODE_IDS.HTTP_REQUEST, NODE_IDS.AI_PROMPT,
  NODE_IDS.IF_CONDITION, NODE_IDS.SET_VARIABLE, NODE_IDS.GET_VARIABLE, NODE_IDS.OUTPUT,
  NODE_IDS.TEXT_TO_FILE, NODE_IDS.DELAY, NODE_IDS.FEED_PARSER, "notification",
]);

export function isBeginnerNode(typeId: string): boolean {
  return BEGINNER_NODE_IDS.has(typeId);
}

export function isAdvancedNode(typeId: string): boolean {
  return DANGEROUS_NODE_IDS.has(typeId);
}
