import { mkSection, type ExtensionContext } from "../../node-configs/popover-utils";
import { showSocialSetupGuide } from "../../panels/SocialSetupGuide";

export function renderSocialUploadFields(ctx: ExtensionContext): void {
  ctx.body.appendChild(mkSection("Platform Setup"));

  const hint = document.createElement("div");
  hint.className = "config-hint";
  hint.textContent = "Need OAuth credentials? The setup guide walks through each platform step by step.";
  ctx.body.appendChild(hint);

  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = "subfolder-add-btn";
  btn.textContent = "Open Setup Guide";
  btn.addEventListener("click", () => {
    const platform = (ctx.node.data.config["platform"] as string | undefined) ?? "youtube";
    const validPlatforms = ["youtube", "instagram", "tiktok"] as const;
    const p = validPlatforms.includes(platform as typeof validPlatforms[number])
      ? (platform as typeof validPlatforms[number])
      : "youtube";
    showSocialSetupGuide(p);
  });
  ctx.body.appendChild(btn);
}
