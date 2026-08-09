// @vitest-environment jsdom

import { describe, it, expect, vi, beforeEach } from "vitest";
import type { WorkflowDiffResult } from "../canvas/WorkflowDiff";

const diffMock = vi.hoisted(() => ({ diffWorkflowJson: vi.fn() }));
vi.mock("../canvas/WorkflowDiff", () => ({ diffWorkflowJson: diffMock.diffWorkflowJson }));

import { showDiffPanel } from "../panels/DiffPanel";

function emptyResult(): WorkflowDiffResult {
  return { metadata: [], nodes: [], edges: [], isEmpty: true };
}

beforeEach(() => {
  document.body.innerHTML = "";
  diffMock.diffWorkflowJson.mockReset();
});

describe("showDiffPanel — empty diff", () => {
  it("renders the 'no differences' message and no section", () => {
    diffMock.diffWorkflowJson.mockReturnValue(emptyResult());
    showDiffPanel("A", "{}", "B", "{}");
    const body = document.querySelector(".diff-panel-body")!;
    expect(body.querySelector(".diff-empty")?.textContent).toContain("No differences");
    expect(body.querySelector(".diff-section")).toBeNull();
  });
});

describe("showDiffPanel — header", () => {
  it("shows both labels joined by an arrow, HTML-escaped", () => {
    diffMock.diffWorkflowJson.mockReturnValue(emptyResult());
    showDiffPanel("<b>Old</b>", "{}", "New", "{}");
    const title = document.querySelector(".diff-panel-title")!;
    expect(title.innerHTML).toContain("&lt;b&gt;Old&lt;/b&gt;");
    expect(title.textContent).toBe("<b>Old</b> → New");
  });
});

describe("showDiffPanel — sections and badges", () => {
  it("renders a Workflow settings section only when metadata changes exist", () => {
    diffMock.diffWorkflowJson.mockReturnValue({
      metadata: [{ field: "Name", from: "Old", to: "New" }],
      nodes: [], edges: [], isEmpty: false,
    } satisfies WorkflowDiffResult);
    showDiffPanel("A", "{}", "B", "{}");
    const sections = Array.from(document.querySelectorAll(".diff-section-title")).map(e => e.textContent);
    expect(sections).toEqual(["Workflow settings"]);
  });

  it("renders Nodes(N) and Connections(N) section titles with counts", () => {
    diffMock.diffWorkflowJson.mockReturnValue({
      metadata: [],
      nodes: [{ id: "n1", name: "Node A", status: "added", changes: [] }],
      edges: [{ id: "e1", from_node: "n1", to_node: "n2", status: "removed", changes: [] }],
      isEmpty: false,
    } satisfies WorkflowDiffResult);
    showDiffPanel("A", "{}", "B", "{}");
    const sections = Array.from(document.querySelectorAll(".diff-section-title")).map(e => e.textContent);
    expect(sections).toContain("Nodes (1)");
    expect(sections).toContain("Connections (1)");
  });

  it("renders the correct status badge label and CSS class for added/removed/changed", () => {
    diffMock.diffWorkflowJson.mockReturnValue({
      metadata: [],
      nodes: [
        { id: "n1", name: "A", status: "added", changes: [] },
        { id: "n2", name: "B", status: "removed", changes: [] },
        { id: "n3", name: "C", status: "changed", changes: [{ field: "name", from: "x", to: "y" }] },
      ],
      edges: [], isEmpty: false,
    } satisfies WorkflowDiffResult);
    showDiffPanel("A", "{}", "B", "{}");
    const entries = Array.from(document.querySelectorAll(".diff-entry"));
    expect(entries).toHaveLength(3);
    expect(entries[0].className).toContain("diff-status-added");
    expect(entries[0].querySelector(".diff-status-badge")?.textContent).toBe("Added");
    expect(entries[1].querySelector(".diff-status-badge")?.textContent).toBe("Removed");
    expect(entries[2].querySelector(".diff-status-badge")?.textContent).toBe("Changed");
  });

  it("renders a field row per change with label, old value, arrow, and new value", () => {
    diffMock.diffWorkflowJson.mockReturnValue({
      metadata: [],
      nodes: [{ id: "n1", name: "A", status: "changed", changes: [{ field: "config", from: "old", to: "new" }] }],
      edges: [], isEmpty: false,
    } satisfies WorkflowDiffResult);
    showDiffPanel("A", "{}", "B", "{}");
    const row = document.querySelector(".diff-field-row")!;
    expect(row.querySelector(".diff-field-label")?.textContent).toBe("config");
    expect(row.querySelector(".diff-field-old")?.textContent).toBe("old");
    expect(row.querySelector(".diff-field-arrow")?.textContent).toBe("→");
    expect(row.querySelector(".diff-field-new")?.textContent).toBe("new");
  });
});

describe("showDiffPanel — fmtValue formatting", () => {
  function changeWith(from: unknown, to: unknown) {
    diffMock.diffWorkflowJson.mockReturnValue({
      metadata: [{ field: "F", from, to }], nodes: [], edges: [], isEmpty: false,
    } satisfies WorkflowDiffResult);
    showDiffPanel("A", "{}", "B", "{}");
    return document.querySelector(".diff-field-row")!;
  }

  it("renders undefined as '(none)'", () => {
    expect(changeWith(undefined, "x").querySelector(".diff-field-old")?.textContent).toBe("(none)");
  });
  it("renders null as 'null'", () => {
    expect(changeWith(null, "x").querySelector(".diff-field-old")?.textContent).toBe("null");
  });
  it("renders an empty string as '(empty)'", () => {
    expect(changeWith("", "x").querySelector(".diff-field-old")?.textContent).toBe("(empty)");
  });
  it("renders an object as JSON.stringify output", () => {
    const row = changeWith({ a: 1 }, "x");
    expect(row.querySelector(".diff-field-old")?.textContent).toBe(JSON.stringify({ a: 1 }));
  });
  it("HTML-escapes a string value containing markup", () => {
    const row = changeWith("<img src=x>", "x");
    expect(row.querySelector(".diff-field-old")?.innerHTML).toContain("&lt;img");
  });
});

describe("showDiffPanel — error path", () => {
  it("shows a 'could not compare' message when the diff engine throws", () => {
    diffMock.diffWorkflowJson.mockImplementation(() => { throw new Error("bad json"); });
    showDiffPanel("A", "not json", "B", "{}");
    const body = document.querySelector(".diff-panel-body")!;
    expect(body.querySelector(".diff-empty")?.textContent).toContain("Could not compare versions");
    expect(body.querySelector(".diff-empty")?.textContent).toContain("bad json");
  });
});

describe("showDiffPanel — overlay lifecycle", () => {
  it("removes a prior diff overlay before rendering a new one (singleton)", () => {
    diffMock.diffWorkflowJson.mockReturnValue(emptyResult());
    showDiffPanel("A", "{}", "B", "{}");
    showDiffPanel("C", "{}", "D", "{}");
    expect(document.querySelectorAll("#diff-panel-overlay")).toHaveLength(1);
    expect(document.querySelector(".diff-panel-title")?.textContent).toBe("C → D");
  });

  it("removes the overlay when the close button is clicked", () => {
    diffMock.diffWorkflowJson.mockReturnValue(emptyResult());
    showDiffPanel("A", "{}", "B", "{}");
    (document.querySelector(".popover-close") as HTMLButtonElement).click();
    expect(document.getElementById("diff-panel-overlay")).toBeNull();
  });

  it("removes the overlay when clicking the backdrop but not when clicking inside the panel", () => {
    diffMock.diffWorkflowJson.mockReturnValue(emptyResult());
    showDiffPanel("A", "{}", "B", "{}");
    document.querySelector(".diff-panel")!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(document.getElementById("diff-panel-overlay")).not.toBeNull();
    document.getElementById("diff-panel-overlay")!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(document.getElementById("diff-panel-overlay")).toBeNull();
  });
});
