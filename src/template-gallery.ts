import type { TemplateInfo } from "./templates";

export type TemplateLoader = (workflow: Record<string, unknown>) => Promise<boolean>;

export function needsLabel(needs: string[]): string {
  return needs.length ? `Needs: ${needs.join(", ")}` : "Needs: nothing";
}

export function renderTemplateCards(
  host: HTMLElement,
  templates: TemplateInfo[],
  onPick: (t: TemplateInfo) => void,
  compact = false,
): void {
  host.textContent = "";
  for (const t of templates) {
    const card = document.createElement("button");
    card.type = "button";
    card.className = compact ? "template-card template-card--compact" : "template-card";
    card.dataset.templateId = t.id;

    const title = document.createElement("span");
    title.className = "template-card-title";
    title.textContent = t.title;
    card.appendChild(title);

    if (!compact && t.summary) {
      const summary = document.createElement("span");
      summary.className = "template-card-summary";
      summary.textContent = t.summary;
      card.appendChild(summary);
    }

    const meta = document.createElement("span");
    meta.className = "template-card-meta";
    const level = document.createElement("span");
    level.className = `template-badge template-badge--${t.difficulty.toLowerCase()}`;
    level.textContent = t.difficulty;
    const needs = document.createElement("span");
    needs.className = "template-needs";
    needs.textContent = needsLabel(t.needs);
    meta.append(level, needs);
    card.appendChild(meta);

    card.addEventListener("click", () => onPick(t));
    host.appendChild(card);
  }
}
