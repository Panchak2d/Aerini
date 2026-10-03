// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  initCommandPalette,
  openPalette,
  closePalette,
  filterByCategory,
  buildSidebarPalette,
} from "../palette-manager";
import { registerNodeDescriptors } from "../canvas/CanvasSerializer";

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

const PLUGIN_NODE = {
  type_id: "custom_plugin_node",
  display_name: "Custom Plugin",
  node_type: "action" as const,
  version: "1.0",
  input_schema: {},
  output_schema: {},
  is_plugin: true,
  icon: '<circle cx="12" cy="12" r="9" />',
  ports: {
    inputs:  [makePort("input",  "left")],
    outputs: [makePort("output", "right")],
  },
};

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
    registerNodeDescriptors(ALL_NODES);
    initCommandPalette(mockCanvas as never, vi.fn());
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
// Command palette — wire-drop mismatch feedback (inline .style.* writes
// replaced with CSS class hooks; behavior/timing must stay identical)
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
    registerNodeDescriptors([NO_INPUT_NODE]);
    initCommandPalette(canvas as never, vi.fn());
    openPalette();

    const row = document.querySelector<HTMLElement>(".palette-result")!;
    expect(row.classList.contains("palette-result--disabled")).toBe(true);
    expect(row.style.opacity).toBe(""); // no inline style written
  });

  it("flashes .palette-result--flash-error on a mismatched click, then clears it after 600ms, without touching inline style", () => {
    vi.useFakeTimers();
    const canvas = { ...mockCanvas, _pendingWireDrop: { fromNode: "n1" } };
    registerNodeDescriptors([NO_INPUT_NODE]);
    initCommandPalette(canvas as never, vi.fn());
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
// Sidebar palette — collapsible category sections
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

  // Locates the header immediately preceding a specific item, found via its
  // unique data-search substring — not by data-cat. The "action" node_type
  // now renders as several independent subheaders (Core Actions/Files &
  // Storage/Integrations/Triggers), so two items can share data-cat="action"
  // while living under entirely different headers.
  function headerForItem(search: string): HTMLButtonElement {
    let el: Element | null = document.querySelector(`.palette-item[data-search*="${search}"]`);
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

  it("clicking a header collapses only its own subheader's items, leaving sibling Action subheaders and other categories untouched", () => {
    // http_request -> "Core Actions", slack -> "Integrations" — both
    // data-cat="action", but different action subheaders.
    const coreActionsHeader  = headerForItem("http request");
    const integrationsHeader = headerForItem("slack");
    expect(coreActionsHeader).not.toBe(integrationsHeader);

    coreActionsHeader.click();
    expect(coreActionsHeader.classList.contains("collapsed")).toBe(true);
    expect(coreActionsHeader.getAttribute("aria-expanded")).toBe("false");
    expect(integrationsHeader.classList.contains("collapsed")).toBe(false);

    const httpItem  = document.querySelector<HTMLElement>('.palette-item[data-search*="http request"]')!;
    const slackItem = document.querySelector<HTMLElement>('.palette-item[data-search*="slack"]')!;
    const aiItem    = document.querySelector<HTMLElement>('.palette-item[data-search*="ai prompt"]')!;
    expect(httpItem.classList.contains("collapsed")).toBe(true);
    expect(slackItem.classList.contains("collapsed")).toBe(false);
    expect(aiItem.classList.contains("collapsed")).toBe(false);
  });

  it("clicking a collapsed header a second time re-expands it", () => {
    const header = headerForItem("http request");
    header.click();
    header.click();

    expect(header.classList.contains("collapsed")).toBe(false);
    expect(header.getAttribute("aria-expanded")).toBe("true");
    const httpItem = document.querySelector<HTMLElement>('.palette-item[data-search*="http request"]')!;
    expect(httpItem.classList.contains("collapsed")).toBe(false);
  });

  it("collapse state (.collapsed) is independent of filter state (.hidden) — a collapsed-but-matching category stays reachable", () => {
    const header = headerForItem("http request");
    header.click();
    filterByCategory("action"); // matches http_request and slack alike (both data-cat="action")

    expect(header.classList.contains("hidden")).toBe(false); // header itself never hidden by filtering
    const httpItem = document.querySelector<HTMLElement>('.palette-item[data-search*="http request"]')!;
    expect(httpItem.classList.contains("collapsed")).toBe(true); // still tucked away
    expect(httpItem.classList.contains("hidden")).toBe(false);   // but not filtered out
  });
});

// ---------------------------------------------------------------------------
// Plugin triggers — grouped with the built-in triggers
// ---------------------------------------------------------------------------

describe("buildSidebarPalette — plugin trigger grouping", () => {
  const PLUGIN_TRIGGER = {
    ...PLUGIN_NODE,
    type_id: "com.example.heartbeat",
    display_name: "Heartbeat Trigger",
    node_type: "utility" as const,
    trigger_capable: true,
  };
  const PLAIN_PLUGIN_UTILITY = {
    ...PLUGIN_NODE,
    type_id: "com.example.plain",
    display_name: "Plain Utility Plugin",
    node_type: "utility" as const,
  };

  function headerFor(search: string): HTMLElement {
    let el: Element | null = document.querySelector(`.palette-item[data-search*="${search}"]`);
    while (el && !el.classList.contains("palette-category")) el = el.previousElementSibling;
    return el as HTMLElement;
  }

  beforeEach(() => {
    document.body.innerHTML = `
      <div id="node-palette"></div>
      <div id="canvas"></div>
    `;
    buildSidebarPalette(
      [NO_INPUT_NODE, PLUGIN_TRIGGER, PLAIN_PLUGIN_UTILITY, HTTP_NODE] as never,
      mockCanvas as never,
      vi.fn(),
    );
  });

  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("lists a trigger-capable plugin under the same Triggers header as a built-in trigger, not under Utility", () => {
    const builtInHeader = headerFor("manual trigger");
    const pluginHeader = headerFor("heartbeat trigger");
    expect(pluginHeader).toBe(builtInHeader);
    expect(pluginHeader.textContent).toContain("Triggers");
    expect(headerFor("plain utility plugin").textContent).toContain("Utility");
  });

  it("tags a trigger-capable plugin for the Trigger chip filter, and keeps its Plugin badge", () => {
    const item = document.querySelector<HTMLElement>('.palette-item[data-search*="heartbeat trigger"]')!;
    expect(item.dataset.cat).toBe("trigger");
    expect(item.querySelector(".palette-plugin-tag")).not.toBeNull();
  });
});

// ---------------------------------------------------------------------------
// Plugin icon — sidebar list and command palette result row
//
// A plugin's own icon (wit/node.wit's metadata.icon) renders as the *main*
// icon here, not a separate small badge next to the name -- plugin type_ids
// are never in icon-cache.ts's static NODE_SVG_INNER map (a plugin can't add
// itself to a host-shipped file), so without this, every plugin node fell
// back to the flat ".palette-dot"/".palette-result-icon" "·" placeholder
// regardless of whether it supplied a real icon.
// ---------------------------------------------------------------------------

describe("plugin icon", () => {
  afterEach(() => {
    if (document.getElementById("command-palette-overlay")) closePalette();
    document.body.innerHTML = "";
  });

  it("sidebar list renders the plugin's sanitized icon as its main icon, not the flat dot fallback", () => {
    document.body.innerHTML = `
      <div id="node-palette"></div>
      <div id="canvas"></div>
    `;
    buildSidebarPalette([...ALL_NODES, PLUGIN_NODE], mockCanvas as never, vi.fn());

    const item = document.querySelector<HTMLElement>('.palette-item[data-search*="custom_plugin_node"]')!;
    expect(item.querySelector(".palette-dot")).toBeNull();
    const icon = item.querySelector(".palette-icon")!;
    expect(icon.querySelector("svg.icon-svg")).not.toBeNull();
    expect(icon.querySelector("circle")).not.toBeNull();
    expect(icon.getAttribute("title")).toBe("Plugin node");
    // No separate icon badge -- the icon above is the only place this
    // plugin's icon renders in this row. It does get a persistent visible
    // text tag though, since the icon/tooltip alone isn't discoverable.
    expect(item.querySelector(".palette-plugin-badge")).toBeNull();
    expect(item.querySelector(".palette-plugin-tag")?.textContent).toBe("Plugin");
  });

  it("command palette result row renders the plugin's sanitized icon as its main icon, not the flat dot fallback", () => {
    document.body.innerHTML = `
      <div id="command-palette-overlay" class="hidden">
        <input id="palette-search" type="text" />
        <div id="palette-results"></div>
      </div>
      <div id="canvas"></div>
    `;
    registerNodeDescriptors([...ALL_NODES, PLUGIN_NODE]);
    initCommandPalette(mockCanvas as never, vi.fn());
    openPalette();

    const row = Array.from(document.querySelectorAll<HTMLElement>(".palette-result"))
      .find(r => r.textContent?.includes("Custom Plugin"))!;
    const icon = row.querySelector(".palette-result-icon")!;
    expect(icon.textContent).not.toBe("·");
    expect(icon.querySelector("svg.icon-svg")).not.toBeNull();
    expect(icon.querySelector("circle")).not.toBeNull();
    expect(icon.getAttribute("title")).toBe("Plugin node");
    expect(row.querySelector(".palette-plugin-badge")).toBeNull();
    expect(row.querySelector(".palette-plugin-tag")?.textContent).toBe("Plugin");
  });

  it("non-plugin nodes keep using the built-in category icon, with no \"Plugin node\" tooltip", () => {
    document.body.innerHTML = `
      <div id="node-palette"></div>
      <div id="command-palette-overlay" class="hidden">
        <input id="palette-search" type="text" />
        <div id="palette-results"></div>
      </div>
      <div id="canvas"></div>
    `;
    buildSidebarPalette(ALL_NODES, mockCanvas as never, vi.fn());
    registerNodeDescriptors(ALL_NODES);
    initCommandPalette(mockCanvas as never, vi.fn());
    openPalette();

    expect(document.querySelectorAll(".palette-plugin-badge").length).toBe(0);
    expect(document.querySelectorAll('[data-tooltip="Plugin node"]').length).toBe(0);
    expect(document.querySelectorAll(".palette-plugin-tag").length).toBe(0);
  });
});

// ---------------------------------------------------------------------------
// Node info tooltip — plugin tag. Same badge the sidebar list and command
// palette row already show (above); this is the third hover surface that
// was missing it.
// ---------------------------------------------------------------------------

describe("node info tooltip — plugin tag", () => {
  // Local fixtures carry a description so showNodeInfoTooltip doesn't
  // early-return (HTTP_NODE/PLUGIN_NODE have none, and neither type_id is in
  // NODE_DESCRIPTION_FALLBACK) — kept local rather than mutating the shared
  // fixtures other describe blocks rely on.
  const HTTP_NODE_DESC   = { ...HTTP_NODE,   description: "Make an HTTP request." };
  const PLUGIN_NODE_DESC = { ...PLUGIN_NODE, description: "A custom plugin node." };

  beforeEach(() => {
    document.body.innerHTML = `
      <div id="node-palette"></div>
      <div id="canvas"></div>
      <div id="node-info-tooltip" class="node-info-tooltip hidden">
        <div class="nit-header">
          <span class="nit-dot"></span>
          <span class="nit-name"></span>
          <span class="nit-plugin-tag palette-plugin-tag hidden">Plugin</span>
        </div>
        <p class="nit-desc"></p>
        <div class="nit-ports"></div>
      </div>
    `;
    buildSidebarPalette([HTTP_NODE_DESC, PLUGIN_NODE_DESC], mockCanvas as never, vi.fn());
  });

  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("shows the plugin tag on hover for a plugin node", () => {
    vi.useFakeTimers();
    const item = document.querySelector<HTMLElement>('.palette-item[data-search*="custom_plugin_node"]')!;
    item.dispatchEvent(new MouseEvent("mouseenter"));
    vi.advanceTimersByTime(600);

    expect(document.getElementById("node-info-tooltip")!.classList.contains("hidden")).toBe(false);
    expect(document.querySelector(".nit-plugin-tag")!.classList.contains("hidden")).toBe(false);
    vi.useRealTimers();
  });

  it("keeps the plugin tag hidden on hover for a non-plugin node", () => {
    vi.useFakeTimers();
    const item = document.querySelector<HTMLElement>('.palette-item[data-search*="http_request"]')!;
    item.dispatchEvent(new MouseEvent("mouseenter"));
    vi.advanceTimersByTime(600);

    expect(document.getElementById("node-info-tooltip")!.classList.contains("hidden")).toBe(false);
    expect(document.querySelector(".nit-plugin-tag")!.classList.contains("hidden")).toBe(true);
    vi.useRealTimers();
  });

  it("hovering a plugin node then a non-plugin node re-hides the tag, not leaking the previous state", () => {
    vi.useFakeTimers();
    const pluginItem = document.querySelector<HTMLElement>('.palette-item[data-search*="custom_plugin_node"]')!;
    pluginItem.dispatchEvent(new MouseEvent("mouseenter"));
    vi.advanceTimersByTime(600);
    expect(document.querySelector(".nit-plugin-tag")!.classList.contains("hidden")).toBe(false);

    const httpItem = document.querySelector<HTMLElement>('.palette-item[data-search*="http_request"]')!;
    httpItem.dispatchEvent(new MouseEvent("mouseenter"));
    vi.advanceTimersByTime(600);
    expect(document.querySelector(".nit-plugin-tag")!.classList.contains("hidden")).toBe(true);
    vi.useRealTimers();
  });
});

// ---------------------------------------------------------------------------
// Preset preview tooltip — show-delay. Mirrors showNodeInfoTooltip's
// 500ms hover delay so a quick mouse pass over the preset list doesn't
// flash a tooltip for every row.
// ---------------------------------------------------------------------------

describe("preset preview tooltip — show delay", () => {
  beforeEach(() => {
    document.body.innerHTML = `
      <div id="node-palette"></div>
      <div id="canvas"></div>
      <div id="template-preview-tooltip" class="hidden">
        <span class="tpt-dot"></span>
        <span class="tpt-name"></span>
        <div class="tpt-config"></div>
      </div>
    `;
    buildSidebarPalette([AI_NODE], mockCanvas as never, vi.fn());
  });

  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("does not show immediately on mouseenter", () => {
    vi.useFakeTimers();
    const item = document.querySelector<HTMLElement>('.palette-preset[data-search*="claude"]')!;
    item.dispatchEvent(new MouseEvent("mouseenter"));

    expect(document.getElementById("template-preview-tooltip")!.classList.contains("hidden")).toBe(true);
    vi.useRealTimers();
  });

  it("shows after the 500ms delay elapses", () => {
    vi.useFakeTimers();
    const item = document.querySelector<HTMLElement>('.palette-preset[data-search*="claude"]')!;
    item.dispatchEvent(new MouseEvent("mouseenter"));
    vi.advanceTimersByTime(500);

    const tip = document.getElementById("template-preview-tooltip")!;
    expect(tip.classList.contains("hidden")).toBe(false);
    expect(tip.querySelector(".tpt-name")!.textContent).toBe("Claude (Anthropic)");
    vi.useRealTimers();
  });

  it("cancels the pending show if the mouse leaves before the delay elapses", () => {
    vi.useFakeTimers();
    const item = document.querySelector<HTMLElement>('.palette-preset[data-search*="claude"]')!;
    item.dispatchEvent(new MouseEvent("mouseenter"));
    item.dispatchEvent(new MouseEvent("mouseleave"));
    vi.advanceTimersByTime(500);

    expect(document.getElementById("template-preview-tooltip")!.classList.contains("hidden")).toBe(true);
    vi.useRealTimers();
  });
});
