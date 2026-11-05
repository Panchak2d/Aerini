/**
 * Status-bar "Problems" chip and its list: blank required fields and broken
 * references found before a run, each with a link to its node and, where one
 * exists, a one-click fix. Also marks nodes that have a reference problem so
 * the canvas badge shows them.
 */

import type { Canvas } from "./canvas/Canvas";
import { applyFixToValue, onProblemsInvalidated } from "./problems";
import type { Problem } from "./problems";
import { collectProblems } from "./validation";

const REFRESH_DELAY_MS = 250;
const SVG_NS = "http://www.w3.org/2000/svg";

export interface ProblemsSummary {
  errors: number;
  warnings: number;
  label: string;
}

export function summarizeProblems(problems: Problem[]): ProblemsSummary {
  const errors = problems.filter(p => p.severity === "error").length;
  const warnings = problems.length - errors;
  const n = problems.length;
  return { errors, warnings, label: n === 0 ? "" : `${n} ${n === 1 ? "problem" : "problems"}` };
}

/** Rewrites the broken reference in place as one undo step. Returns false when the text is no longer there. */
/** Index to focus after an arrow, Home or End key in a list of `count` rows, or null when the key is not for navigation. */
export function nextProblemFocus(count: number, index: number, key: string): number | null {
  if (count <= 0) return null;
  if (key === "ArrowDown") return Math.min(count - 1, index + 1);
  if (key === "ArrowUp") return Math.max(0, index - 1);
  if (key === "Home") return 0;
  if (key === "End") return count - 1;
  return null;
}

export function applyProblemFix(canvas: Canvas, problem: Problem): boolean {
  const node = canvas.nodes.get(problem.nodeId);
  if (!node || !problem.fix || !problem.field) return false;
  const next = applyFixToValue(node.data.config[problem.field], problem.fix);
  if (next === null) return false;
  const field = problem.field;
  canvas.editNode(node, () => { node.data.config[field] = next; });
  return true;
}

/** Flags every node that has a reference or name problem so its canvas badge shows. */
export function markProblemNodes(canvas: Canvas, problems: Problem[]): void {
  const flagged = new Set(problems.filter(p => p.kind !== "required").map(p => p.nodeId));
  for (const n of canvas.nodes.values()) n.hasProblem = flagged.has(n.data.id);
}

function icon(severity: Problem["severity"]): SVGElement {
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("viewBox", "0 0 16 16");
  svg.setAttribute("width", "14");
  svg.setAttribute("height", "14");
  svg.setAttribute("aria-hidden", "true");
  svg.setAttribute("class", `problem-icon problem-icon-${severity}`);
  const shape = document.createElementNS(SVG_NS, "path");
  shape.setAttribute("fill", "none");
  shape.setAttribute("stroke", "currentColor");
  shape.setAttribute("stroke-width", "1.5");
  shape.setAttribute("stroke-linecap", "round");
  shape.setAttribute("stroke-linejoin", "round");
  shape.setAttribute(
    "d",
    severity === "error"
      ? "M8 1.75a6.25 6.25 0 1 0 0 12.5 6.25 6.25 0 0 0 0-12.5ZM8 4.75v4M8 11v.25"
      : "M8 2 14.25 13H1.75L8 2ZM8 6.5v3M8 11.25v.25",
  );
  svg.appendChild(shape);
  return svg;
}

export function initProblemsPanel(canvas: Canvas): { refresh: () => void } {
  const chip = document.getElementById("problems-chip") as HTMLButtonElement | null;
  if (!chip) return { refresh: () => {} };

  const panel = document.createElement("div");
  panel.id = "problems-popover";
  panel.className = "problems-popover";
  panel.setAttribute("role", "dialog");
  panel.setAttribute("aria-label", "Problems");
  panel.hidden = true;
  document.body.appendChild(panel);

  let problems: Problem[] = [];
  let timer: ReturnType<typeof setTimeout> | null = null;

  const isOpen = () => !panel.hidden;

  function setOpen(open: boolean): void {
    panel.hidden = !open;
    chip!.setAttribute("aria-expanded", open ? "true" : "false");
    if (open) {
      render();
      const r = chip!.getBoundingClientRect();
      panel.style.left = `${Math.max(8, Math.min(r.left, window.innerWidth - panel.offsetWidth - 8))}px`;
      panel.style.bottom = `${Math.max(8, window.innerHeight - r.top + 6)}px`;
    }
  }

  function render(): void {
    panel.replaceChildren();
    const title = document.createElement("div");
    title.className = "problems-title";
    title.textContent = "Problems";
    panel.appendChild(title);

    const list = document.createElement("ul");
    list.className = "problems-list";
    for (const p of problems) {
      const li = document.createElement("li");
      li.className = `problem-row problem-row-${p.severity}`;

      const open = document.createElement("button");
      open.type = "button";
      open.className = "problem-open";
      open.appendChild(icon(p.severity));
      const text = document.createElement("span");
      text.className = "problem-text";
      const name = document.createElement("span");
      name.className = "problem-node";
      name.textContent = p.nodeName;
      const msg = document.createElement("span");
      msg.className = "problem-message";
      msg.textContent = p.message;
      text.append(name, msg);
      open.appendChild(text);
      open.addEventListener("click", () => {
        const node = canvas.nodes.get(p.nodeId);
        if (!node) return;
        setOpen(false);
        canvas.revealNode(node);
        canvas.openNodeConfig(node);
      });
      li.appendChild(open);

      if (p.fix) {
        const fix = document.createElement("button");
        fix.type = "button";
        fix.className = "problem-fix";
        fix.textContent = p.fix.label;
        fix.addEventListener("click", () => {
          if (applyProblemFix(canvas, p)) canvas.onWarn?.(`Updated ${p.nodeName}: ${p.fix!.before} is now ${p.fix!.after}`);
          refresh();
        });
        li.appendChild(fix);
      }
      list.appendChild(li);
    }
    panel.appendChild(list);
  }

  panel.addEventListener("keydown", (e) => {
    const rows = Array.from(panel.querySelectorAll<HTMLButtonElement>(".problem-open"));
    const at = rows.indexOf(document.activeElement as HTMLButtonElement);
    if (at < 0) return;
    const next = nextProblemFocus(rows.length, at, e.key);
    if (next === null) return;
    e.preventDefault();
    rows[next].focus();
  });

  function refresh(): void {
    if (timer) { clearTimeout(timer); timer = null; }
    problems = collectProblems(canvas);
    markProblemNodes(canvas, problems);
    const { errors, label } = summarizeProblems(problems);
    chip!.classList.toggle("hidden", problems.length === 0);
    chip!.dataset.level = errors > 0 ? "error" : "warning";
    const labelEl = chip!.querySelector<HTMLElement>(".problems-chip-label");
    if (labelEl) labelEl.textContent = label;
    if (problems.length === 0) setOpen(false);
    else if (isOpen()) render();
  }

  function schedule(): void {
    if (timer) clearTimeout(timer);
    timer = setTimeout(refresh, REFRESH_DELAY_MS);
  }

  chip.addEventListener("click", () => setOpen(!isOpen()));
  document.addEventListener("mousedown", (e) => {
    if (!isOpen()) return;
    const t = e.target as Node | null;
    if (t && (panel.contains(t) || chip.contains(t))) return;
    setOpen(false);
  });
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && isOpen()) { setOpen(false); chip.focus(); }
  });
  onProblemsInvalidated(schedule);

  refresh();
  return { refresh: schedule };
}
