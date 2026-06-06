import type { Canvas } from "../canvas/Canvas";
import type { CanvasNode } from "../canvas/Node";

// Shows a floating textarea directly over the note node on the canvas.
export function showNoteEditor(
  node: CanvasNode,
  canvasEl: HTMLCanvasElement,
  onChange: () => void
): void {
  document.getElementById("note-inline-editor")?.remove();

  const canvas = (canvasEl as unknown as Record<string, unknown>).__canvas as Canvas;
  if (!canvas) return;

  const r      = canvasEl.getBoundingClientRect();
  const NODE_W = 220;
  const sx     = node.data.position.x * canvas.zoom + canvas.panX + r.left;
  const sy     = node.data.position.y * canvas.zoom + canvas.panY + r.top;
  const sw     = NODE_W * canvas.zoom;
  const sh     = Math.max(60, node.height) * canvas.zoom;

  const wrap = document.createElement("div");
  wrap.id = "note-inline-editor";
  wrap.style.cssText = `position:fixed;left:${sx}px;top:${sy}px;width:${sw}px;min-height:${sh}px;z-index:450;`;

  const ta = document.createElement("textarea");
  ta.className = "note-inline-textarea";
  ta.value = String(node.data.config["text"] ?? "");
  ta.placeholder = "Write a note…";
  ta.style.cssText = `width:100%;min-height:${sh}px;resize:both;`;

  ta.addEventListener("input", () => {
    node.data.config["text"] = ta.value;
    onChange();
  });

  const NOTE_COLORS = ["default", "yellow", "blue", "green", "red"] as const;
  const COLOR_HEX: Record<string, string> = {
    default: "#30363d", yellow: "#f59e0b", blue: "#4d9eff", green: "#34d399", red: "#f87171",
  };
  const colorRow = document.createElement("div");
  colorRow.className = "note-color-row";

  for (const c of NOTE_COLORS) {
    const btn = document.createElement("button");
    btn.className = "note-color-btn";
    btn.style.background = COLOR_HEX[c];
    if (node.data.config["color"] === c || (!node.data.config["color"] && c === "default")) {
      btn.classList.add("active");
    }
    btn.addEventListener("click", () => {
      node.data.config["color"] = c;
      colorRow.querySelectorAll(".note-color-btn").forEach(b => b.classList.remove("active"));
      btn.classList.add("active");
      onChange();
    });
    colorRow.appendChild(btn);
  }

  wrap.appendChild(ta);
  wrap.appendChild(colorRow);
  document.body.appendChild(wrap);

  ta.focus();
  ta.setSelectionRange(ta.value.length, ta.value.length);

  const dismiss = (e: MouseEvent) => {
    if (!wrap.contains(e.target as Node)) {
      wrap.remove();
      document.removeEventListener("mousedown", dismiss, true);
    }
  };
  setTimeout(() => document.addEventListener("mousedown", dismiss, true), 80);

  ta.addEventListener("keydown", e => {
    if (e.key === "Escape") {
      wrap.remove();
      document.removeEventListener("mousedown", dismiss, true);
    }
  });
}
