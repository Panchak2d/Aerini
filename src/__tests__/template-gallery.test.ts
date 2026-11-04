// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { needsLabel, renderTemplateCards } from "../template-gallery";
import type { TemplateInfo } from "../templates";

const tpl = (over: Partial<TemplateInfo> = {}): TemplateInfo => ({
  id: "tpl_a", title: "Fetch and show", summary: "Gets a page.", difficulty: "Beginner", needs: [], workflow: { name: "x" }, ...over,
});

describe("template cards", () => {
  it("shows_title_difficulty_and_what_the_template_needs", () => {
    const host = document.createElement("div");
    renderTemplateCards(host, [tpl(), tpl({ id: "tpl_b", needs: ["Ollama"], difficulty: "Advanced" })], () => {});
    const cards = host.querySelectorAll(".template-card");
    expect(cards).toHaveLength(2);
    expect(cards[0].textContent).toContain("Needs: nothing");
    expect(cards[1].textContent).toContain("Needs: Ollama");
    expect(cards[1].querySelector(".template-badge")?.textContent).toBe("Advanced");
    expect(needsLabel(["API key", "Ollama"])).toBe("Needs: API key, Ollama");
  });

  it("passes_the_clicked_template_to_the_loader_and_hides_the_summary_when_compact", () => {
    const host = document.createElement("div");
    const onPick = vi.fn();
    renderTemplateCards(host, [tpl()], onPick, true);
    expect(host.querySelector(".template-card-summary")).toBeNull();
    (host.querySelector(".template-card") as HTMLElement).click();
    expect(onPick).toHaveBeenCalledWith(expect.objectContaining({ id: "tpl_a" }));
  });
});
