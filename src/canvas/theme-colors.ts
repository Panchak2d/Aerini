// Paper-theme canvas color fix.
//
// The Canvas 2D renderer (Node.ts, Canvas.ts, Connector.ts, Minimap.ts) can't
// consume CSS var() — fillStyle/strokeStyle need literal computed color
// strings. This module is the one place that resolves variables.css's custom
// properties into those literals, so every canvas file reads real theme
// colors instead of hardcoding Midnight's hex directly.
//
// Pattern generalized from Minimap.ts's original resolveMinimapColors() (pure
// function, unit-testable without a real canvas 2D context — jsdom has none,
// see canvas-safety.test.ts's own note) and Node.ts's original catAccents()
// (live cache + <html data-theme> MutationObserver, so a switch mid-session
// invalidates it). Both existed independently before this batch; this merges
// them into one cache/observer so Canvas.ts and Connector.ts — which had
// neither — get the same live theme-reactivity for free, and Minimap.ts
// stops needing its own one-time, non-reactive snapshot.
//
// Fallbacks below are byte-identical to the hardcoded literals this fix
// replaces across those files — a lookup miss (no stylesheet linked, e.g.
// most unit tests) reproduces exactly today's Midnight appearance.
//
// Adding theme #3+ — including a future user-defined custom theme — touches
// only variables.css (one new `html[data-theme="x"]` block). This module and
// everything that reads it needs zero changes: it reads tokens by name, not
// by theme, exactly like theme.ts's own THEMES registry.

export interface CanvasThemeColors {
  surface1: string; surface2: string; surface3: string;
  border: string;
  textPrimary: string; textSecondary: string;
  actionNav: string; actionRun: string; warning: string; error: string; ai: string;
  catAction: string; catAI: string; catLogic: string; catUtility: string; catTrigger: string;
  wire: string; wireHover: string;
}

const FALLBACK: CanvasThemeColors = {
  surface1: "#161b22", surface2: "#1c2128", surface3: "#21262d",
  border: "#30363d",
  textPrimary: "#e6edf3", textSecondary: "#8b949e",
  actionNav: "#4d9eff", actionRun: "#34d399", warning: "#f59e0b", error: "#f87171", ai: "#a78bfa",
  catAction: "#4d9eff", catAI: "#a78bfa", catLogic: "#34d399", catUtility: "#f59e0b", catTrigger: "#8aa9c9",
  wire: "#686f78", wireHover: "#858d97",
};

/** Pure token → literal mapping. `getVar` is whatever resolves a custom
 * property to its current string value — real callers pass
 * `getComputedStyle(document.documentElement).getPropertyValue`; tests pass
 * a plain fake (same split Minimap.ts's own resolver used). */
export function resolveCanvasColors(getVar: (name: string) => string): CanvasThemeColors {
  const v = (name: string, fallback: string) => (getVar(name) || "").trim() || fallback;
  return {
    surface1:      v("--color-surface-1", FALLBACK.surface1),
    surface2:      v("--color-surface-2", FALLBACK.surface2),
    surface3:      v("--color-surface-3", FALLBACK.surface3),
    border:        v("--color-border", FALLBACK.border),
    textPrimary:   v("--color-text-primary", FALLBACK.textPrimary),
    textSecondary: v("--color-text-secondary", FALLBACK.textSecondary),
    actionNav:     v("--color-action-nav", FALLBACK.actionNav),
    actionRun:     v("--color-action-run", FALLBACK.actionRun),
    warning:       v("--color-warning", FALLBACK.warning),
    error:         v("--color-error", FALLBACK.error),
    ai:            v("--color-ai", FALLBACK.ai),
    catAction:     v("--cat-action", FALLBACK.catAction),
    catAI:         v("--cat-ai", FALLBACK.catAI),
    catLogic:      v("--cat-logic", FALLBACK.catLogic),
    catUtility:    v("--cat-utility", FALLBACK.catUtility),
    catTrigger:    v("--cat-trigger", FALLBACK.catTrigger),
    wire:          v("--color-wire", FALLBACK.wire),
    wireHover:     v("--color-wire-hover", FALLBACK.wireHover),
  };
}

// ── Live cache, shared by every canvas file ────────────────────────────────
// draw() runs on every rAF tick (Canvas.ts's loop()) across Node/Canvas/
// Connector/Minimap — a per-frame getComputedStyle call would cost real
// main-thread time for a value that cannot change without a theme switch.
// One MutationObserver for the whole canvas layer, keyed off <html
// data-theme> — the same attribute theme.ts's applyTheme() sets — not a
// theme.ts callback, so this stays correct regardless of what else ever
// sets that attribute.
//
// Guarded because canvas/*.ts is imported by vitest files running under the
// default `environment: "node"` (no `document` at all) — danger-badge.test.ts,
// node-kind-hierarchy.test.ts, canvas-safety.test.ts(jsdom, but variables.css
// is never linked into its DOM) among others. Both paths fall back to
// FALLBACK above, which is byte-for-byte the same hex every one of these
// files hardcoded before this fix — zero-visual-diff wiring for Midnight,
// only Paper (and any future theme) actually changes.
let _cache: CanvasThemeColors | null = null;
let _observerAttached = false;

function ensureObserver(): void {
  if (_observerAttached) return;
  _observerAttached = true;
  if (typeof MutationObserver === "undefined") return; // defensive; not expected missing wherever `document` exists
  new MutationObserver(() => { _cache = null; })
    .observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
}

/** Cached, theme-reactive accessor. Safe to call every frame from any canvas
 * file — real work (getComputedStyle + resolve) happens once per theme
 * switch, not once per draw() call. */
export function getCanvasColors(): CanvasThemeColors {
  if (_cache) return _cache;
  if (typeof document === "undefined") {
    _cache = { ...FALLBACK };
    return _cache;
  }
  ensureObserver();
  const cs = getComputedStyle(document.documentElement);
  _cache = resolveCanvasColors((name) => cs.getPropertyValue(name));
  return _cache;
}

/** Forces the next getCanvasColors() call to re-resolve instead of waiting
 * for the MutationObserver's own microtask. Two callers: tests (deterministic,
 * no `await new Promise(setTimeout...)` needed), and theme.ts's applyTheme()
 * — which needs the new colors available synchronously-after (for
 * icon-cache.ts's preloadAllIcons() re-warm, immediately following in the
 * same function) rather than racing the observer's own not-yet-run
 * microtask. Not called from any canvas draw path — those rely on the
 * observer, which is sufficient there (next frame, not next line). */
export function _resetCanvasColorCache(): void {
  _cache = null;
}

// ── Derived colors ──────────────────────────────────────────────────────────
// Composed from the base tokens above via the same alpha-suffix concatenation
// already used throughout this codebase (e.g. `accent + "88"`) — no new
// formula, no new CSS tokens. Centralized here so extending to a 6th note
// color or a 5th node kind is one line here, not a hunt across 3 files.

function hexToRgb(hex: string): [number, number, number] {
  const h = hex.replace("#", "");
  const full = h.length === 3 ? h.split("").map(c => c + c).join("") : h;
  const n = parseInt(full.slice(0, 6), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

function rgbToHex(r: number, g: number, b: number): string {
  const c = (x: number) => Math.round(Math.max(0, Math.min(255, x))).toString(16).padStart(2, "0");
  return `#${c(r)}${c(g)}${c(b)}`;
}

/** Flattens `fg` over `bg` at strength `t` (0–1) into a solid opaque hex.
 * Used where a canvas element must stay fully opaque — a note body, like
 * every other node body — but still carry a per-theme, per-hue tint, with
 * no new static color tokens. (An alpha-suffix fillStyle would show the
 * canvas's own dot-grid through the note, unlike every other node.) */
export function tintOver(fg: string, bg: string, t: number): string {
  const [fr, fgc, fb] = hexToRgb(fg);
  const [br, bgc, bb] = hexToRgb(bg);
  return rgbToHex(fr * t + br * (1 - t), fgc * t + bgc * (1 - t), fb * t + bb * (1 - t));
}

/** Per-kind node-header tint. Previously 4 hardcoded near-black hexes
 * (Node.ts's TYPE_META_BASE.dim), fixed regardless of theme — the header
 * stayed near-black even on Paper. Replaced with the kind's own accent at
 * low alpha, composited over the node body (drawn first, same draw() call,
 * always opaque) — automatically theme-correct for Midnight, Paper, and any
 * future theme. DISCLOSED: this also changes Midnight's exact header hue —
 * was a fixed near-black per kind, now a translucent tint of that kind's own
 * accent — a deliberate, requested part of this fix (accepted design call),
 * not a side effect. The four kinds stay visually distinct by hue. */
export function headerTint(accentHex: string): string {
  return accentHex + "26"; // ~15% alpha, composited over the opaque body fill
}

export interface NoteColorSet { bg: string; border: string; text: string; }

/** Note-node color palette, keyed by the 5 names NoteEditor.ts writes to
 * config.color (confirmed against panels/NoteEditor.ts's own NOTE_COLORS
 * array — Rule 23). Previously 5 hardcoded bg/border/text hex triads,
 * correct on Midnight only. Now derived from the same base semantic tokens
 * every other themed element uses: "default" reads the neutral surface/
 * border/text tokens; the 4 hue names reuse the matching status token
 * (yellow→warning, blue→actionNav, green→actionRun, red→error) — bg via
 * tintOver (must stay opaque, see above), border/text via the existing
 * alpha-suffix pattern. Zero new tokens; a future custom theme that defines
 * the 5 base status colors gets correct note colors for free. */
export function resolveNoteColors(colors: CanvasThemeColors): Record<string, NoteColorSet> {
  const t = 0.16; // tint strength — subtle, keeps text-on-bg legible without drowning the hue
  return {
    default: { bg: colors.surface2, border: colors.border, text: colors.textSecondary },
    yellow:  { bg: tintOver(colors.warning,   colors.surface2, t), border: colors.warning   + "44", text: colors.warning },
    blue:    { bg: tintOver(colors.actionNav, colors.surface2, t), border: colors.actionNav + "44", text: colors.actionNav },
    green:   { bg: tintOver(colors.actionRun, colors.surface2, t), border: colors.actionRun + "44", text: colors.actionRun },
    red:     { bg: tintOver(colors.error,     colors.surface2, t), border: colors.error     + "44", text: colors.error },
  };
}
