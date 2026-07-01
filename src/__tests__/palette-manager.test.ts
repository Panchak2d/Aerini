// @vitest-environment jsdom
/**
 * DEVIATION NOTE: palette-manager.ts has no exported pure filter function.
 * Filtering is split across two private functions:
 *   - `renderPaletteResults(q)` — filters _allNodes and renders to #palette-results
 *     (the command palette / ⌘K panel)
 *   - `applyFilters()` — reads .palette-item elements and toggles the "hidden"
 *     class (sidebar palette category chips)
 *
 * Tests 1–4 exercise the command palette via `initCommandPalette` + `openPalette`
 * and inspect the rendered DOM.
 *
 * Tests 5–9 exercise `filterByCategory` / `applyFilters` by manually seeding
 * `.palette-item` elements in the DOM then calling the exported
 * `filterByCategory` function — exactly the path the UI uses.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  initCommandPalette,
  openPalette,
  closePalette,
  filterByCategory,
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
