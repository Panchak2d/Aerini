import { _resetCanvasColorCache } from "./canvas/theme-colors";
import { preloadAllIcons } from "./icon-cache";


export interface ThemeDef {
  /** Matches the `html[data-theme="..."]` selector. "midnight" is the one
   * exception — it has no attribute value (see DEFAULT_THEME below); kept
   * in this array anyway so the <select> can list it like any other
   * option instead of needing a "no selection" special case in the UI. */
  id: string;
  label: string;
}

export const THEMES: ThemeDef[] = [
  { id: "midnight", label: "Midnight (default)" },
  { id: "paper", label: "Paper" },
];

const DEFAULT_THEME = "midnight";
const STORAGE_KEY = "aerini_theme";

function isKnownTheme(id: string): boolean {
  return THEMES.some(t => t.id === id);
}

/** Reads the persisted theme choice, falling back to the default for
 * anything missing or unrecognized (e.g. a value left over from a
 * since-removed theme, or hand-edited storage) — never returns an id with
 * no matching CSS block. */
export function getStoredTheme(): string {
  const stored = localStorage.getItem(STORAGE_KEY);
  return stored && isKnownTheme(stored) ? stored : DEFAULT_THEME;
}

/** Applies a theme by id: sets/removes <html data-theme> and persists the
 * choice. Falls back to the default for an unrecognized id (defensive —
 * e.g. a stale value from a since-removed theme) rather than writing an
 * attribute value no CSS block matches. */
export function applyTheme(id: string): void {
  const theme = isKnownTheme(id) ? id : DEFAULT_THEME;
  if (theme === DEFAULT_THEME) {
    // Midnight is the bare :root block — no attribute means "use it",
    // matching the app's default state.
    document.documentElement.removeAttribute("data-theme");
  } else {
    document.documentElement.setAttribute("data-theme", theme);
  }
  localStorage.setItem(STORAGE_KEY, theme);

  // Re-warm icon-cache.ts's bitmap cache for the new theme's accent colors.
  // getIconBitmap does an exact string-key lookup with no cross-theme
  // fallback (canvas falls back to a plain letter glyph on a miss) — without
  // this, node icons would silently degrade to letters on every switch until
  // the app was reloaded. _resetCanvasColorCache() first: the canvas layer's
  // own MutationObserver (theme-colors.ts) would eventually pick up this
  // attribute change too, but only on its next microtask — calling it
  // explicitly here makes preloadAllIcons() read the *new* colors
  // synchronously-after, not race that not-yet-run microtask.
  _resetCanvasColorCache();
  void preloadAllIcons();
}

/**
 * Call once, as early as possible in app startup (top of app.ts). Re-applies
 * the persisted choice so a saved non-default theme survives a reload.
 *
 * DISCLOSED TRADEOFF: index.html has one script entry point, a deferred
 * `type="module"` (app.ts) — no inline script precedes it. tauri.conf.json's
 * CSP is `script-src 'self'` with no `unsafe-inline` (Aerini_AI_UI_Refactor_Rules.md
 * §3 forbids relaxing CSP), which
 * rules out the usual inline-head-script FOUC fix. In this Tauri webview
 * `index.html`/`main.css` are local files with effectively-zero fetch
 * latency, so the residual risk (default theme paints for one frame before
 * this runs, for a user who saved Paper) is real but minor — disclosed, not
 * hidden, and not worth a CSP exception or a second script-loading
 * mechanism to close.
 */
export function initTheme(): void {
  applyTheme(getStoredTheme());
}
