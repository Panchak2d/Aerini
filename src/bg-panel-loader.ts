// Shared lazy-loader for the deferred BgJobsPanel module (P30).
// Single module-level cache so app.ts, toolbar.ts, and scheduler-events.ts
// all share the same loaded-module reference instead of each keeping a
// redundant local copy of the same import()-caching pattern.

type BgPanelModule = typeof import("./panels/BgJobsPanel");

let _bgPanel: BgPanelModule | null = null;

export function loadBgPanel(): Promise<BgPanelModule> {
  return _bgPanel
    ? Promise.resolve(_bgPanel)
    : import("./panels/BgJobsPanel").then(m => { _bgPanel = m; return m; });
}

// Synchronous accessor for call sites that only need the module if it has
// already loaded (e.g. an optional-chained update after a prior await).
// Returns null if BgJobsPanel has not been loaded yet.
export function getBgPanelIfLoaded(): BgPanelModule | null {
  return _bgPanel;
}
