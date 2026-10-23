import type { ExtensionContext } from "../../node-configs/popover-utils";
import { AI_PROMPT_REPLY_FIELD, isBlank } from "../../chat/chat-defaults";

export function renderOutputHint(ctx: ExtensionContext): void {
  if (!isBlank((ctx.node.data.config as Record<string, unknown>)["field"])) return;
  const tip = document.createElement("div");
  tip.className = "popover-info-banner";
  tip.innerHTML =
    `<strong>Showing everything.</strong> After an AI Prompt, set Field to <strong>${AI_PROMPT_REPLY_FIELD}</strong> ` +
    `to show only the reply text instead of the whole result.`;
  ctx.body.appendChild(tip);
}
