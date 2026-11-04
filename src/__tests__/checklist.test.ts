// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from "vitest";
import {
  CHECKLIST_DISMISSED_KEY, computeChecklist, initChecklist, snapshotFromCanvas, type ChecklistSource,
} from "../checklist";
import { ONBOARDED_KEY } from "../onboarding";

const node = (node_type_id: string, config: Record<string, unknown> = {}) =>
  ({ data: { node_type_id, config, credentials: {} } }) as never;
const source = (over: Partial<ChecklistSource> = {}): ChecklistSource => ({
  nodes: new Map(), connectors: new Map(), runFinished: false, resultVisible: false, ...over,
});

describe("first-run checklist progress", () => {
  it("counts_a_trigger_and_a_worker_node_separately_and_ignores_notes", () => {
    const s = snapshotFromCanvas(source({ nodes: new Map([["a", node("manual_trigger")], ["n", node("note")]]) }));
    expect(s).toMatchObject({ hasTrigger: true, hasNode: false });
    const s2 = snapshotFromCanvas(source({ nodes: new Map([["a", node("manual_trigger")], ["b", node("http_request")]]), connectors: new Map([["w", {}]]) }));
    expect(s2).toMatchObject({ hasTrigger: true, hasNode: true, hasWire: true });
  });

  it("marks_fill_a_field_done_only_when_every_required_field_has_a_value", () => {
    const blank = snapshotFromCanvas(source({ nodes: new Map([["b", node("http_request")]]) }));
    const filled = snapshotFromCanvas(source({ nodes: new Map([["b", node("http_request", { url: "https://example.com", method: "GET" })]]) }));
    expect(blank.hasFilledField).toBe(false);
    expect(filled.hasFilledField).toBe(true);
  });

  it("needs_a_finished_run_before_the_result_step_can_be_done", () => {
    expect(snapshotFromCanvas(source({ resultVisible: true })).hasResult).toBe(false);
    const done = snapshotFromCanvas(source({ runFinished: true, resultVisible: true }));
    expect(done).toMatchObject({ hasRun: true, hasResult: true });
    expect(computeChecklist(done).map(s => s.id)).toEqual(["hasTrigger", "hasNode", "hasWire", "hasFilledField", "hasRun", "hasResult"]);
  });
});

describe("first-run checklist panel", () => {
  const mount = () => {
    document.body.innerHTML = '<div id="first-run-checklist" class="hidden"></div><div id="output-drawer" class="hidden"></div>';
    return document.getElementById("first-run-checklist") as HTMLElement;
  };
  const canvas = () => ({ nodes: new Map(), connectors: new Map() });
  beforeEach(() => localStorage.clear());

  it("shows_six_steps_for_a_new_user_and_stays_dismissed_after_the_close_button", () => {
    const host = mount();
    initChecklist(canvas(), () => false);
    expect(host.classList.contains("hidden")).toBe(false);
    expect(host.querySelectorAll(".checklist-step")).toHaveLength(6);
    (host.querySelector(".checklist-dismiss") as HTMLElement).click();
    expect(host.classList.contains("hidden")).toBe(true);
    expect(localStorage.getItem(CHECKLIST_DISMISSED_KEY)).toBe("1");
  });

  it("stays_hidden_for_a_user_who_finished_onboarding_before_the_checklist_existed", () => {
    const host = mount();
    localStorage.setItem(ONBOARDED_KEY, "1");
    initChecklist(canvas(), () => false);
    expect(host.classList.contains("hidden")).toBe(true);
  });
});
