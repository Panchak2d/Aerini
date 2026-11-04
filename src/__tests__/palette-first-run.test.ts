// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { buildSidebarPalette, filterByCategory } from "../palette-manager";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), convertFileSrc: vi.fn((p: string) => p) }));

const port = (id: string, position: "left" | "right") => ({ id, label: id, position });
const desc = (type_id: string, description?: string) => ({
  type_id, display_name: type_id, node_type: "action" as const, version: "1", description,
  input_schema: {}, output_schema: {}, ports: { inputs: [port("input", "left")], outputs: [port("output", "right")] },
});

describe("sidebar palette first-run aids", () => {
  beforeEach(() => {
    document.body.innerHTML = '<div id="category-chips"><button class="cat-chip active" data-cat="all"></button><button class="cat-chip" data-cat="beginner"></button></div><div id="node-palette"></div>';
    buildSidebarPalette(
      [desc("http_request", "Calls a web address. Sends headers."), desc("shell_exec", "Runs a command."), desc("ai_prompt", "Asks an AI.")] as never,
      {} as never, () => {},
    );
  });
  const item = (type: string) => [...document.querySelectorAll<HTMLElement>(".palette-item:not(.palette-preset)")].find(e => e.dataset.search?.includes(type))!;

  it("shows_the_first_sentence_of_each_description_under_the_name", () => {
    expect(item("http_request").querySelector(".palette-desc")?.textContent).toBe("Calls a web address.");
  });

  it("tags_dangerous_nodes_advanced_and_hides_them_under_the_beginner_filter", () => {
    expect(item("shell_exec").querySelector(".palette-advanced-tag")?.textContent).toBe("Advanced");
    expect(item("http_request").querySelector(".palette-advanced-tag")).toBeNull();
    filterByCategory("beginner");
    expect(item("http_request").classList.contains("hidden")).toBe(false);
    expect(item("shell_exec").classList.contains("hidden")).toBe(true);
    document.querySelectorAll<HTMLElement>(".palette-preset").forEach(p => expect(p.classList.contains("hidden")).toBe(true));
  });
});
