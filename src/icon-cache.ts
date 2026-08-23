// Inner SVG elements for each node type.
// All icons sourced from Lucide (ISC license), verified 2025-06.
// Stroke color is injected at render time; no fill.
const NODE_SVG_INNER: Record<string, string> = {
  manual_trigger: `<path d="M5 5a2 2 0 0 1 3.008-1.728l11.997 6.998a2 2 0 0 1 .003 3.458l-12 7A2 2 0 0 1 5 19z" />`,
  webhook:        `<path d="M12 6v16" /><path d="m19 13 2-1a9 9 0 0 1-18 0l2 1" /><path d="M9 11h6" /><circle cx="12" cy="4" r="2" />`,
  schedule:       `<circle cx="12" cy="12" r="10" /><path d="M12 6v6l4 2" />`,
  http_request:   `<path d="M7 7h10v10" /><path d="M7 17 17 7" />`,
  shell_exec:     `<path d="M12 19h8" /><path d="m4 17 6-6-6-6" />`,
  email_send:     `<path d="m22 7-8.991 5.727a2 2 0 0 1-2.009 0L2 7" /><rect x="2" y="4" width="20" height="16" rx="2" />`,
  file:           `<path d="M6 22a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h8a2.4 2.4 0 0 1 1.704.706l3.588 3.588A2.4 2.4 0 0 1 20 8v12a2 2 0 0 1-2 2z" /><path d="M14 2v5a1 1 0 0 0 1 1h5" />`,
  if_condition:   `<path d="M15 6a9 9 0 0 0-9 9V3" /><circle cx="18" cy="6" r="3" /><circle cx="6" cy="18" r="3" />`,
  switch:         `<circle cx="9" cy="12" r="3" /><rect width="20" height="14" x="2" y="5" rx="7" />`,
  loop:           `<path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8" /><path d="M21 3v5h-5" /><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16" /><path d="M8 16H3v5" />`,
  stop:           `<rect width="18" height="18" x="3" y="3" rx="2" />`,
  merge:          `<path d="m8 6 4-4 4 4" /><path d="M12 2v10.3a4 4 0 0 1-1.172 2.872L4 22" /><path d="m20 22-5-5" />`,
  delay:          `<line x1="10" x2="14" y1="2" y2="2" /><line x1="12" x2="15" y1="14" y2="11" /><circle cx="12" cy="14" r="8" />`,
  wait:           `<path d="M5 22h14" /><path d="M5 2h14" /><path d="M17 22v-4.172a2 2 0 0 0-.586-1.414L12 12l-4.414 4.414A2 2 0 0 0 7 17.828V22" /><path d="M7 2v4.172a2 2 0 0 0 .586 1.414L12 12l4.414-4.414A2 2 0 0 0 17 6.172V2" />`,
  transform:      `<path d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" /><path d="M3 3v5h5" /><path d="M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16" /><path d="M16 16h5v5" />`,
  transform_data: `<path d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" /><path d="M3 3v5h5" /><path d="M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16" /><path d="M16 16h5v5" />`,
  json:           `<path d="M8 3H7a2 2 0 0 0-2 2v5a2 2 0 0 1-2 2 2 2 0 0 1 2 2v5c0 1.1.9 2 2 2h1" /><path d="M16 21h1a2 2 0 0 0 2-2v-5c0-1.1.9-2 2-2a2 2 0 0 1-2-2V5a2 2 0 0 0-2-2h-1" />`,
  set_variable:   `<line x1="5" x2="19" y1="9" y2="9" /><line x1="5" x2="19" y1="15" y2="15" />`,
  get_variable:   `<path d="M8 21s-4-3-4-9 4-9 4-9" /><path d="M16 3s4 3 4 9-4 9-4 9" /><line x1="15" x2="9" y1="9" y2="15" /><line x1="9" x2="15" y1="9" y2="15" />`,
  ai_prompt:      `<path d="M11.017 2.814a1 1 0 0 1 1.966 0l1.051 5.558a2 2 0 0 0 1.594 1.594l5.558 1.051a1 1 0 0 1 0 1.966l-5.558 1.051a2 2 0 0 0-1.594 1.594l-1.051 5.558a1 1 0 0 1-1.966 0l-1.051-5.558a2 2 0 0 0-1.594-1.594l-5.558-1.051a1 1 0 0 1 0-1.966l5.558-1.051a2 2 0 0 0 1.594-1.594z" /><path d="M20 2v4" /><path d="M22 4h-4" /><circle cx="4" cy="20" r="2" />`,
  ai_agent:       `<path d="M12 8V4H8" /><rect width="16" height="12" x="4" y="8" rx="2" /><path d="M2 14h2" /><path d="M20 14h2" /><path d="M15 13v2" /><path d="M9 13v2" />`,
  ai_memory:      `<path d="M12 18V5" /><path d="M15 13a4.17 4.17 0 0 1-3-4 4.17 4.17 0 0 1-3 4" /><path d="M17.598 6.5A3 3 0 1 0 12 5a3 3 0 1 0-5.598 1.5" /><path d="M17.997 5.125a4 4 0 0 1 2.526 5.77" /><path d="M18 18a4 4 0 0 0 2-7.464" /><path d="M19.967 17.483A4 4 0 1 1 12 18a4 4 0 1 1-7.967-.517" /><path d="M6 18a4 4 0 0 1-2-7.464" /><path d="M6.003 5.125a4 4 0 0 0-2.526 5.77" />`,
  text_splitter:  `<circle cx="6" cy="6" r="3" /><path d="M8.12 8.12 12 12" /><path d="M20 4 8.12 15.88" /><circle cx="6" cy="18" r="3" /><path d="M14.8 14.8 20 20" />`,
  database:       `<ellipse cx="12" cy="5" rx="9" ry="3" /><path d="M3 5V19A9 3 0 0 0 21 19V5" /><path d="M3 12A9 3 0 0 0 21 12" />`,
  notification:   `<path d="M10.268 21a2 2 0 0 0 3.464 0" /><path d="M3.262 15.326A1 1 0 0 0 4 17h16a1 1 0 0 0 .74-1.673C19.41 13.956 18 12.499 18 8A6 6 0 0 0 6 8c0 4.499-1.411 5.956-2.738 7.326" />`,
  code:           `<path d="m18 16 4-4-4-4" /><path d="m6 8-4 4 4 4" /><path d="m14.5 4-5 16" />`,
  output:         `<circle cx="12" cy="12" r="10" /><circle cx="12" cy="12" r="1" />`,
  note:           `<path d="M21 9a2.4 2.4 0 0 0-.706-1.706l-3.588-3.588A2.4 2.4 0 0 0 15 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2z" /><path d="M15 3v5a1 1 0 0 0 1 1h5" />`,
  s3_storage:     `<path d="M17.5 19H9a7 7 0 1 1 6.71-9h1.79a4.5 4.5 0 1 1 0 9Z" />`,
  image_gen:      `<rect width="18" height="18" x="3" y="3" rx="2" ry="2" /><circle cx="9" cy="9" r="2" /><path d="m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21" />`,
  save_to_folder: `<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z" /><path d="M12 10v6" /><path d="m15 13-3 3-3-3" />`,
  collect_files:  `<path d="M12 10v6" /><path d="M9 13h6" /><path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z" />`,
  social_upload:  `<path d="M12 3v12" /><path d="m17 8-5-5-5 5" /><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />`,
  text_to_file:   `<path d="M6 22a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h8a2.4 2.4 0 0 1 1.704.706l3.588 3.588A2.4 2.4 0 0 1 20 8v12a2 2 0 0 1-2 2z" /><path d="M14 2v5a1 1 0 0 0 1 1h5" /><path d="M8 13h8" /><path d="M8 17h5" />`,
};

function buildSvg(inner: string, color: string, size: number): string {
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${size}" height="${size}" viewBox="0 0 24 24" fill="none" stroke="${color}" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${inner}</svg>`;
}

const _bitmapCache = new Map<string, ImageBitmap>();

export function getIconBitmap(typeId: string, color: string): ImageBitmap | null {
  return _bitmapCache.get(`${typeId}:${color}`) ?? null;
}

async function loadBitmap(typeId: string, inner: string, color: string, size: number): Promise<void> {
  const key = `${typeId}:${color}`;
  if (_bitmapCache.has(key)) return;
  try {
    const blob = new Blob([buildSvg(inner, color, size)], { type: "image/svg+xml" });
    _bitmapCache.set(key, await createImageBitmap(blob));
  } catch {
    // Silently skip — canvas falls back to accent letter
  }
}

import { getCanvasColors } from "./canvas/theme-colors";

// The exact, complete set of colors Node.ts's draw() can ever pass as
// `accent` to getIconBitmap (its one call site) — verified against
// statusAccent()/typeMeta() in canvas/Node.ts, not assumed. Only 5 distinct
// values despite 4 category + 3 status states existing: --cat-logic and
// --cat-utility are literal `var(--color-action-run)` / `var(--color-warning)`
// aliases in variables.css (:root, not re-declared per theme), so "success"
// and "running" status colors always equal "logic" and "utility" category
// colors — true in every theme, not a Midnight-only coincidence, confirmed
// against variables.css directly.
//
// Reads the LIVE theme tokens (getCanvasColors(), same shared cache Node.ts/
// Canvas.ts/Connector.ts/Minimap.ts use) rather than a hardcoded list:
// getIconBitmap does an exact string-key lookup (`${typeId}:${color}`) with
// no cross-theme fallback, so preloading colors that don't match the active
// theme would silently degrade every node icon to its plain-letter fallback.
export function accentColors(): string[] {
  const c = getCanvasColors();
  return [c.catAction, c.catAI, c.catLogic, c.catUtility, c.error];
}

// Preload at physical pixel size (dpr * 16) so bitmaps are sharp on retina displays.
// The canvas transform (dpr * zoom) maps 16 world-coord units to exactly these pixels at zoom=1.
export async function preloadAllIcons(): Promise<void> {
  const size  = Math.round(window.devicePixelRatio * 16);
  const tasks: Promise<void>[] = [];
  for (const [typeId, inner] of Object.entries(NODE_SVG_INNER)) {
    for (const color of accentColors()) {
      tasks.push(loadBitmap(typeId, inner, color, size));
    }
  }
  await Promise.allSettled(tasks);
}

export function wrapIconSvg(inner: string): string {
  return `<svg class="icon-svg" xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${inner}</svg>`;
}

export function getIconSvg(typeId: string): string {
  const inner = NODE_SVG_INNER[typeId];
  if (!inner) return "";
  return wrapIconSvg(inner);
}
