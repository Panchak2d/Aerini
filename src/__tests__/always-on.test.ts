/**
 * @vitest-environment jsdom
 *
 * `updateAlwaysOnBtn`'s has-trigger branch removes `always-on-wrap`'s
 * `data-tooltip` attribute; the no-trigger branch never restored it, so the
 * explanatory tooltip was permanently gone for the rest of the app session
 * after the first trigger-bearing workflow was ever opened. Fix restores it
 * on every no-trigger call, not just before the first has-trigger call.
 */
import { describe, it, expect, beforeEach } from "vitest";
import { updateAlwaysOnBtn } from "../always-on";
import type { Canvas } from "../canvas/Canvas";
import type { WorkflowManager } from "../workflow-manager";

const TOOLTIP = "Add a Schedule, Webhook, or trigger-plugin trigger to enable Run on launch";

function stubCanvas(nodeTypeIds: string[]): Canvas {
  const nodes = new Map(
    nodeTypeIds.map((t, i) => [String(i), { data: { node_type_id: t } }])
  );
  return { nodes } as unknown as Canvas;
}

const wfManager = { currentId: "wf1" } as unknown as WorkflowManager;

beforeEach(() => {
  document.body.innerHTML = `
    <button id="btn-always-on" class="hidden"></button>
    <span id="always-on-wrap" data-tooltip="${TOOLTIP}"></span>
  `;
});

describe("updateAlwaysOnBtn — tooltip restoration", () => {
  it("removes data-tooltip when the workflow has a schedulable trigger", async () => {
    await updateAlwaysOnBtn(stubCanvas(["schedule"]), wfManager);
    const wrap = document.getElementById("always-on-wrap")!;
    expect(wrap.hasAttribute("data-tooltip")).toBe(false);
  });

  it("restores data-tooltip when later switching to a workflow with no trigger (the bug)", async () => {
    // First: trigger-bearing workflow removes the tooltip.
    await updateAlwaysOnBtn(stubCanvas(["schedule"]), wfManager);
    expect(document.getElementById("always-on-wrap")!.hasAttribute("data-tooltip")).toBe(false);

    // Then: switch to a workflow with no schedulable trigger.
    await updateAlwaysOnBtn(stubCanvas(["http_request"]), wfManager);
    const wrap = document.getElementById("always-on-wrap")!;
    expect(wrap.getAttribute("data-tooltip")).toBe(TOOLTIP);
  });

  it("no-trigger branch hides the button", async () => {
    await updateAlwaysOnBtn(stubCanvas(["http_request"]), wfManager);
    expect(document.getElementById("btn-always-on")!.classList.contains("hidden")).toBe(true);
  });
});
