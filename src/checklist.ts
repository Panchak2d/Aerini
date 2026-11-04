import type { CanvasNode } from "./canvas/Node";
import { isTriggerNodeType } from "./canvas/node-registry";
import { NODE_IDS } from "./node-ids";
import { ONBOARDED_KEY } from "./onboarding";
import { onProblemsInvalidated } from "./problems";
import { storageGet, storageSet } from "./safe-storage";
import { blankRequiredFields, requiredFieldsFor } from "./validation";

export const CHECKLIST_DISMISSED_KEY = "aerini_checklist_dismissed";

export interface ChecklistSnapshot {
  hasTrigger: boolean;
  hasNode: boolean;
  hasWire: boolean;
  hasFilledField: boolean;
  hasRun: boolean;
  hasResult: boolean;
}

export interface ChecklistStep { id: keyof ChecklistSnapshot; label: string; done: boolean }

export interface ChecklistSource {
  nodes: Map<string, CanvasNode>;
  connectors: Map<string, unknown>;
  runFinished: boolean;
  resultVisible: boolean;
}

const LABELS: Array<[keyof ChecklistSnapshot, string]> = [
  ["hasTrigger", "Add a trigger"],
  ["hasNode", "Add a node"],
  ["hasWire", "Connect them with a wire"],
  ["hasFilledField", "Fill in a field"],
  ["hasRun", "Run the workflow"],
  ["hasResult", "See the result"],
];

export function snapshotFromCanvas(src: ChecklistSource): ChecklistSnapshot {
  const worker = [...src.nodes.values()].filter(n => {
    const type = n.data.node_type_id;
    return !isTriggerNodeType(type) && type !== NODE_IDS.NOTE;
  });
  return {
    hasTrigger: [...src.nodes.values()].some(n => isTriggerNodeType(n.data.node_type_id)),
    hasNode: worker.length > 0,
    hasWire: src.connectors.size > 0,
    hasFilledField: worker.some(n => requiredFieldsFor(n.data).length > 0 && blankRequiredFields(n.data).length === 0),
    hasRun: src.runFinished,
    hasResult: src.runFinished && src.resultVisible,
  };
}

export function computeChecklist(snap: ChecklistSnapshot): ChecklistStep[] {
  return LABELS.map(([id, label]) => ({ id, label, done: snap[id] }));
}

export function isChecklistDismissed(): boolean {
  return storageGet(CHECKLIST_DISMISSED_KEY) === "1";
}

const CHECK_SVG = '<svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true"><path d="M2 5.2 4.2 7.4 8 3" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>';
const CLOSE_SVG = '<svg width="12" height="12" viewBox="0 0 12 12" fill="none" aria-hidden="true"><path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/></svg>';

export function renderChecklist(host: HTMLElement, steps: ChecklistStep[], onDismiss: () => void): void {
  host.textContent = "";
  const done = steps.filter(s => s.done).length;

  const head = document.createElement("div");
  head.className = "checklist-head";
  const title = document.createElement("span");
  title.className = "checklist-title";
  title.textContent = done === steps.length ? "All set" : `Getting started (${done} of ${steps.length})`;
  const close = document.createElement("button");
  close.type = "button";
  close.className = "checklist-dismiss";
  close.setAttribute("aria-label", "Dismiss the getting started checklist");
  close.innerHTML = CLOSE_SVG;
  close.addEventListener("click", onDismiss);
  head.append(title, close);

  const list = document.createElement("ul");
  list.className = "checklist-steps";
  for (const step of steps) {
    const li = document.createElement("li");
    li.className = step.done ? "checklist-step checklist-step--done" : "checklist-step";
    const mark = document.createElement("span");
    mark.className = "checklist-mark";
    mark.innerHTML = CHECK_SVG;
    const text = document.createElement("span");
    text.textContent = step.label;
    li.append(mark, text);
    list.appendChild(li);
  }
  host.append(head, list);
}

export interface ChecklistCanvas {
  nodes: Map<string, CanvasNode>;
  connectors: Map<string, unknown>;
}

export function initChecklist(canvas: ChecklistCanvas, hasRunResult: () => boolean): { refresh: () => void } {
  const host = document.getElementById("first-run-checklist");
  if (storageGet(ONBOARDED_KEY) && storageGet(CHECKLIST_DISMISSED_KEY) === null) storageSet(CHECKLIST_DISMISSED_KEY, "1");
  if (!host || isChecklistDismissed()) return { refresh: () => {} };

  const drawer = document.getElementById("output-drawer");
  const dismiss = () => {
    storageSet(CHECKLIST_DISMISSED_KEY, "1");
    host.classList.add("hidden");
  };
  const refresh = () => {
    if (isChecklistDismissed()) return;
    const snap = snapshotFromCanvas({
      nodes: canvas.nodes,
      connectors: canvas.connectors,
      runFinished: hasRunResult(),
      resultVisible: !!drawer && !drawer.classList.contains("hidden"),
    });
    renderChecklist(host, computeChecklist(snap), dismiss);
    host.classList.remove("hidden");
  };

  onProblemsInvalidated(refresh);
  if (drawer) new MutationObserver(refresh).observe(drawer, { attributes: true, attributeFilter: ["class"] });
  refresh();
  return { refresh };
}
