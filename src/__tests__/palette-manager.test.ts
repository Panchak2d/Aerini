// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  initCommandPalette,
  openPalette,
  closePalette,
  filterByCategory,
  buildSidebarPalette,
} from "../palette-manager";

// palette-manager → icon-cache → @tauri-apps/api/core
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn((p: string) => p),
}));

// ---------------------------------------------------------------------------
// Test fixtures
// ---------------------------------------------------------------------------

const makePort = (id: string, pos: "left" | "right") => ({ id, label: id, position: pos });

const HTTP_NODE = {
  type_id: "http_request",
  display_name: "HTTP Request",
  node_type: "action" as const,
  version: "1.0",
  input_schema: {},
  output_schema: {},
  ports: {
    inputs:  [makePort("input",  "left")],
    outputs: [makePort("output", "right")],
  },
};

const AI_NODE = {
  type_id: "ai_prompt",
  display_name: "AI Prompt",
  node_type: "ai" as const,
  version: "1.0",
  input_schema: {},
  output_schema: {},
  ports: {
    inputs:  [makePort("input",  "left")],
    outputs: [makePort("output", "right")],
  },
};

const SLACK_NODE = {
  type_id: "slack",
  display_name: "Slack",
  node_type: "action" as const,
  version: "1.0",
  input_schema: {},
  output_schema: {},
  ports: {
    inputs:  [makePort("input",  "left")],
    outputs: [makePort("output", "right")],
  },
};

const ALL_NODES = [HTTP_NODE, AI_NODE, SLACK_NODE];

const NO_INPUT_NODE = {
  type_id: "manual_trigger",
  display_name: "Manual Trigger",
  node_type: "action" as const,
  version: "1.0",
  input_schema: {},
  output_schema: {},
  ports: {
    inputs:  [],
    outputs: [makePort("output", "right")],
  },
};

// Minimal Canvas stub — only properties read inside renderPaletteResults
const mockCanvas = {
  _pendingWireDrop:      null,
  _pendingInputWireDrop: null,
  _pendingPresetConfig:  null,
  _pendingPresetName:    null,
  placeNode:             vi.fn(),
  pendingInsert:         null,
  insertGhost:           null,
};

// ---------------------------------------------------------------------------
// Command palette — name filter
// ---------------------------------------------------------------------------

describe("command palette — name filter", () => {
  beforeEach(() => {
    document.body.innerHTML = `
      <div id="command-palette-overlay" class="hidden">
        <input id="palette-search" type="text" />
        <div id="palette-results"></div>
      </div>
      <div id="canvas"></div>
    `;
    initCommandPalette(ALL_NODES, mockCanvas as never, vi.fn());
    openPalette();
  });

  afterEach(() => {
    // closePalette needs the overlay to exist — call before clearing DOM
    closePalette();
    document.body.innerHTML = "";
  });

  it("empty query renders all nodes", () => {
    const rows = document.querySelectorAll(".palette-result");
    expect(rows.length).toBe(3);
  });

  it("filter by name substring returns only matching nodes", () => {
    const inp = document.getElementById("palette-search") as HTMLInputElement;
    inp.value = "http";
    inp.dispatchEvent(new Event("input"));

    const rows = document.querySelectorAll(".palette-result");
    expect(rows.length).toBe(1);
    expect(rows[0].textContent).toContain("HTTP Request");
  });

  it("name filter is case-insensitive — 'SLACK' matches Slack node", () => {
    const inp = document.getElementById("palette-search") as HTMLInputElement;
    inp.value = "SLACK";
    inp.dispatchEvent(new Event("input"));

    const rows = document.querySelectorAll(".palette-result");
    expect(rows.length).toBe(1);
    expect(rows[0].textContent).toContain("Slack");
  });

  it("query with no matches renders the empty state", () => {
    const inp = document.getElementById("palette-search") as HTMLInputElement;
    inp.value = "zzznomatch";
    inp.dispatchEvent(new Event("input"));

    expect(document.querySelector(".palette-empty")).not.toBeNull();
    expect(document.querySelectorAll(".palette-result").length).toBe(0);
  });
});

// ---------------------------------------------------------------------------
// Command palette — wire-drop mismatch feedback (Batch 10: inline .style.*
// writes replaced with CSS class hooks; behavior/timing must stay identical)
// ---------------------------------------------------------------------------

describe("command palette — wire-drop mismatch feedback", () => {
  beforeEach(() => {
    document.body.innerHTML = `
      <div id="command-palette-overlay" class="hidden">
        <input id="palette-search" type="text" />
        <div id="palette-results"></div>
      </div>
      <div id="canvas"></div>
    `;
  });

  afterEach(() => {
    closePalette();
    document.body.innerHTML = "";
  });

  it("dims a node with no input ports via a class, not an inline style, while dropping from an output wire", () => {
    const canvas = { ...mockCanvas, _pendingWireDrop: { fromNode: "n1" } };
    initCommandPalette([NO_INPUT_NODE], canvas as never, vi.fn());
    openPalette();

    const row = document.querySelector<HTMLElement>(".palette-result")!;
    expect(row.classList.contains("palette-result--disabled")).toBe(true);
    expect(row.style.opacity).toBe(""); // no inline style written
  });

  it("flashes .palette-result--flash-error on a mismatched click, then clears it after 600ms, without touching inline style", () => {
    vi.useFakeTimers();
    const canvas = { ...mockCanvas, _pendingWireDrop: { fromNode: "n1" } };
    initCommandPalette([NO_INPUT_NODE], canvas as never, vi.fn());
    openPalette();

    const row = document.querySelector<HTMLElement>(".palette-result")!;
    row.click();

    expect(row.classList.contains("palette-result--flash-error")).toBe(true);
    expect(row.style.outline).toBe("");
    expect(canvas._pendingWireDrop).toBeNull(); // mismatch still cancels the wire-drop, unchanged

    vi.advanceTimersByTime(600);
    expect(row.classList.contains("palette-result--flash-error")).toBe(false);

    vi.useRealTimers();
  });
});

// ---------------------------------------------------------------------------
// Sidebar palette — category filter (via filterByCategory / applyFilters)
// ---------------------------------------------------------------------------

describe("filterByCategory — sidebar palette", () => {
  beforeEach(() => {
    // Structure matches real app: category header followed by its items
    document.body.innerHTML = `
      <input id="node-search" type="text" value="" />
      <div class="cat-chip" data-cat="all"></div>
      <div class="cat-chip" data-cat="action"></div>
      <div class="cat-chip" data-cat="ai"></div>
      <div class="palette-category">Actions</div>
      <div class="palette-item" data-cat="action" data-search="http request action http_request">HTTP Request</div>
      <div class="palette-item" data-cat="action" data-search="slack action slack">Slack</div>
      <div class="palette-category">AI</div>
      <div class="palette-item" data-cat="ai" data-search="ai prompt ai ai_prompt">AI Prompt</div>
    `;
  });

  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("filter by 'action' hides the AI node", () => {
    filterByCategory("action");
    const aiItem = document.querySelector<HTMLElement>('[data-cat="ai"].palette-item')!;
    expect(aiItem.classList.contains("hidden")).toBe(true);
  });

  it("filter by 'action' keeps action nodes visible", () => {
    filterByCategory("action");
    const actionItems = document.querySelectorAll<HTMLElement>('[data-cat="action"].palette-item');
    actionItems.forEach(el => expect(el.classList.contains("hidden")).toBe(false));
  });

  it("filter by 'ai' hides action nodes", () => {
    filterByCategory("ai");
    const actionItems = document.querySelectorAll<HTMLElement>('[data-cat="action"].palette-item');
    actionItems.forEach(el => expect(el.classList.contains("hidden")).toBe(true));
  });

  it("filter by 'all' makes every palette-item visible", () => {
    filterByCategory("action"); // narrow first
    filterByCategory("all");    // then reset

    const allItems = document.querySelectorAll<HTMLElement>(".palette-item");
    allItems.forEach(el => expect(el.classList.contains("hidden")).toBe(false));
  });

  it("active cat-chip reflects the selected category", () => {
    filterByCategory("action");

    const actionChip = document.querySelector<HTMLElement>('[data-cat="action"].cat-chip')!;
    const aiChip     = document.querySelector<HTMLElement>('[data-cat="ai"].cat-chip')!;
    const allChip    = document.querySelector<HTMLElement>('[data-cat="all"].cat-chip')!;

    expect(actionChip.classList.contains("active")).toBe(true);
    expect(aiChip.classList.contains("active")).toBe(false);
    expect(allChip.classList.contains("active")).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// Sidebar palette — collapsible category sections (Batch 5)
// ---------------------------------------------------------------------------

describe("buildSidebarPalette — collapsible categories", () => {
  beforeEach(() => {
    document.body.innerHTML = `
      <div id="node-palette"></div>
      <div id="canvas"></div>
    `;
    buildSidebarPalette(ALL_NODES, mockCanvas as never, vi.fn());
  });

  afterEach(() => {
    document.body.innerHTML = "";
  });

  function headers(): HTMLButtonElement[] {
    return Array.from(document.querySelectorAll<HTMLButtonElement>(".palette-category"));
  }

  // Locates a category's header by walking back from its first item, rather
  // than matching on label text — buildCategories()'s label capitalization
  // ("Action", "Ai") is a pre-existing, unrelated quirk, not something this
  // test should assume or depend on.
  function headerFor(cat: string): HTMLButtonElement {
    let el: Element | null = document.querySelector(`.palette-item[data-cat="${cat}"]`);
    while (el && !el.classList.contains("palette-category")) el = el.previousElementSibling;
    return el as HTMLButtonElement;
  }

  it("renders a real, focusable button per category, each expanded by default", () => {
    const hs = headers();
    expect(hs.length).toBeGreaterThan(0);
    hs.forEach(h => {
      expect(h.tagName).toBe("BUTTON");
      expect(h.getAttribute("aria-expanded")).toBe("true");
      expect(h.classList.contains("collapsed")).toBe(false);
    });
  });

  it("clicking a header collapses only its own items, leaving other categories untouched", () => {
    const actionsHeader = headerFor("action");
    const aiHeader       = headerFor("ai");

    actionsHeader.click();
    expect(actionsHeader.classList.contains("collapsed")).toBe(true);
    expect(actionsHeader.getAttribute("aria-expanded")).toBe("false");
    expect(aiHeader.classList.contains("collapsed")).toBe(false);

    const actionItems = document.querySelectorAll<HTMLElement>('.palette-item[data-cat="action"]');
    expect(actionItems.length).toBeGreaterThan(0);
    actionItems.forEach(el => expect(el.classList.contains("collapsed")).toBe(true));

    const aiItems = document.querySelectorAll<HTMLElement>('.palette-item[data-cat="ai"]');
    aiItems.forEach(el => expect(el.classList.contains("collapsed")).toBe(false));
  });

  it("clicking a collapsed header a second time re-expands it", () => {
    const actionsHeader = headerFor("action");
    actionsHeader.click();
    actionsHeader.click();

    expect(actionsHeader.classList.contains("collapsed")).toBe(false);
    expect(actionsHeader.getAttribute("aria-expanded")).toBe("true");
    document.querySelectorAll<HTMLElement>('.palette-item[data-cat="action"]').forEach(el =>
      expect(el.classList.contains("collapsed")).toBe(false)
    );
  });

  it("collapse state (.collapsed) is independent of filter state (.hidden) — a collapsed-but-matching category stays reachable", () => {
    const actionsHeader = headerFor("action");
    actionsHeader.click();
    filterByCategory("action"); // matches the very items we just collapsed

    expect(actionsHeader.classList.contains("hidden")).toBe(false); // header itself never hidden by filtering
    document.querySelectorAll<HTMLElement>('.palette-item[data-cat="action"]').forEach(el => {
      expect(el.classList.contains("collapsed")).toBe(true); // still tucked away
      expect(el.classList.contains("hidden")).toBe(false);    // but not filtered out
    });
  });
});
