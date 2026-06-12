import type { NodeDescriptor } from "./ipc/workflow";
import type { Canvas } from "./canvas/Canvas";
import { NODE_IDS } from "./node-ids";
import { NODE_ICONS, escapeHtml } from "./utils";

interface Category { label: string; nodes: NodeDescriptor[] }

const CATEGORY_ORDER = ["trigger", "action", "ai", "logic", "utility", "other"];

function buildCategories(nodes: NodeDescriptor[]): Category[] {
  const groups = new Map<string, NodeDescriptor[]>();
  for (const node of nodes) {
    const cat = node.node_type ?? "other";
    if (!groups.has(cat)) groups.set(cat, []);
    groups.get(cat)!.push(node);
  }
  return CATEGORY_ORDER
    .filter(k => groups.has(k))
    .map(k => ({ label: k.charAt(0).toUpperCase() + k.slice(1), nodes: groups.get(k)! }));
}


const CAT_NAMES: Record<string, string> = {
  action: "Actions", ai: "AI", logic: "Logic", utility: "Utility",
};

export function buildSidebarPalette(
  allNodes: NodeDescriptor[],
  canvas: Canvas,
  onStatus: (msg: string) => void
): void {
  const pal = document.getElementById("node-palette")!;
  pal.innerHTML = "";

  const PRESETS: Array<{ label: string; baseTypeId: string; config: Record<string, unknown> }> = [
    {
      label: "Claude (Anthropic)",
      baseTypeId: NODE_IDS.AI_PROMPT,
      config: { provider: "anthropic", model: "claude-sonnet-4-20250514", temperature: 0.7, max_tokens: 2048 },
    },
    {
      label: "GPT-4o (OpenAI)",
      baseTypeId: NODE_IDS.AI_PROMPT,
      config: { provider: "openai", model: "gpt-4o", temperature: 0.7, max_tokens: 2048 },
    },
    {
      label: "Gemini (Google)",
      baseTypeId: NODE_IDS.AI_PROMPT,
      config: { provider: "gemini", model: "gemini-2.5-flash", temperature: 0.7, max_tokens: 2048 },
    },
    {
      label: "Ollama (Local)",
      baseTypeId: NODE_IDS.AI_PROMPT,
      config: { provider: "auto", model: "llama3", base_url: "http://localhost:11434/v1", temperature: 0.7, max_tokens: 2048 },
    },
    {
      label: "DALL-E 3 (OpenAI)",
      baseTypeId: NODE_IDS.IMAGE_GEN,
      config: { provider: "dalle3", n: 1, size: "1024x1024", quality: "standard" },
    },
    {
      label: "Imagen 4 (Google)",
      baseTypeId: NODE_IDS.IMAGE_GEN,
      config: { provider: "imagen4", n: 1, aspect_ratio: "1:1" },
    },
    {
      label: "Save to Folder",
      baseTypeId: NODE_IDS.SAVE_TO_FOLDER,
      config: { overwrite: true, filename_prefix: "", subfolders: [] },
    },
    {
      label: "Collect Files",
      baseTypeId: NODE_IDS.COLLECT_FILES,
      config: { sources: [{ id: "src_1", name: "Source 1", source_expr: "" }] },
    },
    {
      label: "Upload to YouTube",
      baseTypeId: NODE_IDS.SOCIAL_UPLOAD,
      config: { platform: "youtube", privacy: "private" },
    },
    {
      label: "Upload to Instagram",
      baseTypeId: NODE_IDS.SOCIAL_UPLOAD,
      config: { platform: "instagram" },
    },
    {
      label: "Upload to TikTok",
      baseTypeId: NODE_IDS.SOCIAL_UPLOAD,
      config: { platform: "tiktok", privacy: "self_only" },
    },
  ];

  const nodeDescMap = new Map(allNodes.map(n => [n.type_id, n]));
  const hasAnyPreset = PRESETS.some(p => nodeDescMap.has(p.baseTypeId));
  if (hasAnyPreset) {
    const ph = document.createElement("div");
    ph.className = "palette-category";
    ph.textContent = "Templates";
    pal.appendChild(ph);

    for (const preset of PRESETS) {
      const base = nodeDescMap.get(preset.baseTypeId);
      if (!base) continue;
      const desc: NodeDescriptor = {
        ...base,
        display_name: preset.label,
        type_id: base.type_id,
      };
      const dotClass = `dot-${base.node_type}`;
      const item = document.createElement("div");
      item.className = "palette-item palette-preset";
      item.dataset.search = preset.label.toLowerCase();
      item.innerHTML = `<span class="palette-dot ${dotClass}"></span><span class="palette-name">${preset.label}</span><span class="palette-preset-tag">preset</span>`;

      item.addEventListener("click", () => {
        blurSearch();
        const r = document.getElementById("canvas")!.getBoundingClientRect();
        const node = canvas.placeNode(desc, r.width / 2, r.height / 2);
        Object.assign(node.data.config, preset.config);
        node.data.name = preset.label;
        onStatus(`Placed ${preset.label} preset — double-click to configure`);
      });
      armDragOnThreshold(item, canvas, () => {
        const presetDesc = { ...desc };
        canvas._pendingPresetConfig = preset.config;
        canvas._pendingPresetName = preset.label;
        return presetDesc;
      });
      pal.appendChild(item);
    }
  }

  for (const cat of buildCategories(allNodes)) {
    if (!cat.nodes.length) continue;

    const h = document.createElement("div");
    h.className = "palette-category";
    h.textContent = cat.label;
    pal.appendChild(h);

    for (const desc of cat.nodes) {
      const item = document.createElement("div");
      item.className = "palette-item";
      item.dataset.search = `${desc.display_name} ${desc.node_type} ${desc.type_id}`.toLowerCase();
      item.innerHTML = `<span class="palette-dot dot-${desc.node_type}"></span><span class="palette-name">${escapeHtml(desc.display_name)}</span>${desc.is_plugin ? '<span class="palette-plugin-badge" title="Plugin node">P</span>' : ""}`;

      item.addEventListener("click", () => {
        blurSearch();
        const r = document.getElementById("canvas")!.getBoundingClientRect();
        canvas.placeNode(desc, r.width / 2, r.height / 2);
        onStatus(`Placed ${desc.display_name} — double-click to configure`);
      });
      armDragOnThreshold(item, canvas, () => desc);
      pal.appendChild(item);
    }
  }
}

export function bindSidebarSearch(): void {
  document.getElementById("node-search")!.addEventListener("input", e => {
    const q = (e.target as HTMLInputElement).value.toLowerCase().trim();
    document.querySelectorAll<HTMLElement>(".palette-item").forEach(el =>
      el.classList.toggle("hidden", !!q && !(el.dataset.search ?? "").includes(q))
    );
    document.querySelectorAll<HTMLElement>(".palette-category").forEach(cat => {
      let sib = cat.nextElementSibling;
      let any = false;
      while (sib && !sib.classList.contains("palette-category")) {
        if (!(sib as HTMLElement).classList.contains("hidden")) { any = true; break; }
        sib = sib.nextElementSibling;
      }
      cat.classList.toggle("hidden", !any);
    });
  });
}

export function blurSearch(): void {
  (document.getElementById("node-search") as HTMLInputElement)?.blur();
}

let paletteActive   = 0;
let paletteFiltered: NodeDescriptor[] = [];
let _allNodes: NodeDescriptor[] = [];
let _canvas:   Canvas | null = null;
let _onStatus: ((msg: string) => void) | null = null;

export function initCommandPalette(
  allNodes: NodeDescriptor[],
  canvas: Canvas,
  onStatus: (msg: string) => void
): void {
  _allNodes = allNodes;
  _canvas   = canvas;
  _onStatus = onStatus;

  const overlay = document.getElementById("command-palette-overlay")!;
  const inp = document.getElementById("palette-search") as HTMLInputElement;

  inp.addEventListener("input", () => { paletteActive = 0; renderPaletteResults(inp.value); });
  inp.addEventListener("keydown", e => {
    const total = paletteFiltered.length;
    if (e.key === "ArrowDown")  { e.preventDefault(); paletteActive = Math.min(paletteActive + 1, total - 1); highlightPalette(); document.querySelector(".palette-result.active")?.scrollIntoView({ block: "nearest" }); }
    else if (e.key === "ArrowUp")   { e.preventDefault(); paletteActive = Math.max(paletteActive - 1, 0); highlightPalette(); document.querySelector(".palette-result.active")?.scrollIntoView({ block: "nearest" }); }
    else if (e.key === "Enter")     { const d = paletteFiltered[paletteActive]; if (d) insertFromPalette(d); }
    else if (e.key === "Escape")    { closePalette(); }
  });
  overlay.addEventListener("click", e => { if (e.target === overlay) closePalette(); });
}

export function openPalette(): void {
  const overlay = document.getElementById("command-palette-overlay")!;
  const inp = document.getElementById("palette-search") as HTMLInputElement;
  overlay.classList.remove("hidden");
  inp.value = ""; paletteActive = 0;
  renderPaletteResults("");
  requestAnimationFrame(() => inp.focus());
}

export function closePalette(): void {
  const overlay = document.getElementById("command-palette-overlay")!;
  overlay.classList.add("hidden");
  overlay.dispatchEvent(new CustomEvent("palette-closed"));
}

function renderPaletteResults(q: string): void {
  const resultsEl = document.getElementById("palette-results")!;
  resultsEl.innerHTML = "";
  const lower = q.toLowerCase().trim();
  paletteFiltered = lower
    ? _allNodes.filter(t => t.display_name.toLowerCase().includes(lower) || t.type_id.includes(lower))
    : _allNodes;

  if (!paletteFiltered.length) {
    resultsEl.innerHTML = `<div class="palette-empty">No nodes match "${q}"</div>`;
    return;
  }

  const grouped: Record<string, NodeDescriptor[]> = {};
  for (const d of paletteFiltered) (grouped[d.node_type] ??= []).push(d);

  let idx = 0;
  for (const [cat, items] of Object.entries(grouped)) {
    const gl = document.createElement("div");
    gl.className = "palette-group-label";
    gl.textContent = CAT_NAMES[cat] ?? cat;
    resultsEl.appendChild(gl);

    for (const desc of items) {
      const i = idx++;
      const row = document.createElement("div");
      row.className = "palette-result";
      if (i === paletteActive) row.classList.add("active");
      row.dataset.idx = String(i);
      const icon     = escapeHtml(NODE_ICONS[desc.type_id] ?? "·");
      const catClass = ["action", "ai", "logic", "utility"].includes(desc.node_type)
        ? desc.node_type : "utility";
      row.innerHTML = `
        <div class="palette-result-icon palette-result-icon--${catClass}">${icon}</div>
        <div>
          <div class="palette-result-name">${escapeHtml(desc.display_name)}${desc.is_plugin ? '<span class="palette-plugin-badge" title="Plugin node">P</span>' : ""}</div>
          <div class="palette-result-cat">${escapeHtml(CAT_NAMES[desc.node_type] ?? desc.node_type)}</div>
        </div>
        <kbd class="palette-result-kbd">Enter</kbd>`;

      // In wire-drop mode, dim nodes that cannot receive a connection
      const noInputs = !desc.ports.inputs.length;
      const inWireDrop = _canvas?._pendingWireDrop != null;
      if (inWireDrop && noInputs) {
        row.style.opacity = "0.35";
        row.title = "This node has no input ports — cannot connect";
      }
      row.addEventListener("click", () => insertFromPalette(desc));
      row.addEventListener("mouseover", () => { paletteActive = i; highlightPalette(); });
      armDragOnThreshold(row, _canvas!, () => desc);
      resultsEl.appendChild(row);
    }
  }
}

function highlightPalette(): void {
  document.querySelectorAll<HTMLElement>(".palette-result").forEach(el =>
    el.classList.toggle("active", parseInt(el.dataset.idx ?? "-1") === paletteActive)
  );
}

function insertFromPalette(desc: NodeDescriptor): void {
  if (!_canvas) return;
  const overlay = document.getElementById("command-palette-overlay")!;
  // In wire-drop mode: only allow nodes that have at least one input port
  if (_canvas._pendingWireDrop) {
    if (!desc.ports.inputs.length) {
      // Flash the result red briefly to signal it can't be connected
      const activeEl = document.querySelector<HTMLElement>(".palette-result.active");
      if (activeEl) {
        activeEl.style.outline = "1px solid var(--red)";
        setTimeout(() => { activeEl.style.outline = ""; }, 600);
      }
      _canvas._pendingWireDrop = null;
      return; // Don't place or close
    }
    overlay.dispatchEvent(new CustomEvent("wire-drop-pick", { detail: desc }));
    closePalette();
    return;
  }
  const r = document.getElementById("canvas")!.getBoundingClientRect();
  _canvas.placeNode(desc, r.width / 2, r.height / 2);
  _onStatus?.(`Placed ${desc.display_name} — double-click to configure`);
  closePalette();
}

// Arms pendingInsert only after the mouse has moved ≥6px from the original
// mousedown position. This prevents single clicks from triggering a drag.

const DRAG_THRESHOLD = 6; // px

function armDragOnThreshold(
  el: HTMLElement,
  canvas: Canvas,
  getDesc: () => NodeDescriptor,
): void {
  el.addEventListener("mousedown", e => {
    if (e.button !== 0) return;
    const startX = e.clientX;
    const startY = e.clientY;
    let armed = false;

    const onMove = (me: MouseEvent) => {
      if (armed) return;
      const dx = me.clientX - startX;
      const dy = me.clientY - startY;
      if (Math.hypot(dx, dy) >= DRAG_THRESHOLD) {
        armed = true;
        canvas.pendingInsert = getDesc();
        canvas.insertGhost = null;
        cleanup();
      }
    };

    const cleanup = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };

    const onUp = () => {
      // Mouseup without reaching threshold = click, not drag. Cancel.
      if (!armed) {
        canvas.pendingInsert = null;
        canvas.insertGhost = null;
      }
      cleanup();
    };

    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  });
}
