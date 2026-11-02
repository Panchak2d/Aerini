/**
 * Path-aware view of a node's output. Every plain value is a button that
 * inserts the exact reference to it into the field being edited. Built beside
 * `syntaxHighlight` (output-renderer.ts), which stays as it is.
 */

import { formatSample, isAddressableKey } from "./data-sources";
import { insertFromTree } from "./point-mode";

const MAX_DEPTH = 6;
const MAX_CHILDREN = 200;

export interface JsonTreeOptions {
  nodeId: string;
  nodeName: string;
  /** Replaceable in tests; returns true when a field took the reference. */
  onPick?: (expr: string) => boolean;
}

function isContainer(v: unknown): v is Record<string, unknown> | unknown[] {
  return typeof v === "object" && v !== null;
}

export function buildJsonTree(value: unknown, opts: JsonTreeOptions): HTMLElement {
  const root = document.createElement("div");
  root.className = "json-tree";
  const pick = opts.onPick ?? ((expr: string) => insertFromTree(opts.nodeId, expr));

  const leaf = (label: string, v: unknown, path: string | null): HTMLElement => {
    const row = document.createElement("div");
    row.className = "json-tree-row";
    const key = document.createElement("span");
    key.className = "json-tree-key";
    key.textContent = label;
    row.appendChild(key);
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "json-tree-leaf";
    btn.setAttribute("data-json-node", opts.nodeId);
    btn.textContent = formatSample(v, 80) || "(empty)";
    if (path === null) {
      btn.disabled = true;
      btn.title = "This name can't be used in a reference.";
    } else {
      const expr = `{{${opts.nodeName}.output${path}}}`;
      btn.title = `Click to use ${expr} in the field you are editing`;
      // Keeps the caret in the field: a click here must not move focus.
      btn.addEventListener("mousedown", (e) => e.preventDefault());
      btn.addEventListener("click", () => { pick(expr); });
    }
    row.appendChild(btn);
    return row;
  };

  const walk = (v: unknown, path: string | null, depth: number, into: HTMLElement): void => {
    const entries: Array<[string, unknown, string | null]> = Array.isArray(v)
      ? v.slice(0, MAX_CHILDREN).map((c, i): [string, unknown, string | null] => [`[${i}]`, c, path === null ? null : `${path}[${i}]`])
      : Object.entries(v as Record<string, unknown>).slice(0, MAX_CHILDREN).map(([k, c]): [string, unknown, string | null] =>
          [k, c, path !== null && isAddressableKey(k) ? `${path}.${k}` : null]);
    for (const [label, child, childPath] of entries) {
      if (isContainer(child) && depth < MAX_DEPTH) {
        const d = document.createElement("details");
        d.className = "json-tree-node";
        d.open = depth < 1;
        const sum = document.createElement("summary");
        sum.textContent = label;
        d.appendChild(sum);
        const inner = document.createElement("div");
        inner.className = "json-tree-children";
        walk(child, childPath, depth + 1, inner);
        d.appendChild(inner);
        into.appendChild(d);
      } else {
        into.appendChild(leaf(label, child, childPath));
      }
    }
  };

  if (isContainer(value)) walk(value, "", 0, root);
  else root.appendChild(leaf("value", value, ""));
  return root;
}
