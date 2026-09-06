import type { Canvas } from "./canvas/Canvas";
import type { WorkflowManager } from "./workflow-manager";
import { RunManager, getBgJobs } from "./run-manager";
import { stopScheduledWorkflow, parseSchedulerError, exportAllWorkflows } from "./ipc/workflow";
import { isTauri } from "./utils";
import { getAutostart, setAutostart } from "./ipc/autostart";
import { checkForUpdate } from "./ipc/update";
import { invoke } from "@tauri-apps/api/core";
import { validateWorkflow, checkDangerousNodes } from "./validation";
import { showConfirm } from "./confirm";
import { bindWfSettings } from "./wf-settings";
import { bindAlwaysOnToggle, updateAlwaysOnBtn } from "./always-on";
import { activateZone } from "./sidebar-sections";
import { isMonitorModeActive } from "./monitor-mode";
import { bindDrawerResize } from "./resize";
import { loadBgPanel } from "./bg-panel-loader";
import { NODE_IDS, findTriggerNodeTypeId } from "./node-ids";

type Toast = (msg: string, type?: "success" | "error" | "info") => void;

export interface IChatPanel {
  toggle(): void;
  refreshButtonVisibility(): void;
  onWorkflowSwitched(): void;
  isOpen(): boolean;
  startForChat(): void;
}

export interface ToolbarResult {
  refreshRunBtn:     () => void;
  runWithValidation: () => Promise<void>;
}

async function handleExportAll(toast: Toast): Promise<void> {
  try {
    const result = await exportAllWorkflows();
    if (result.workflow_count === 0) {
      toast("No workflows to export", "info");
      return;
    }
    const filename = `aerini-backup-${new Date().toISOString().slice(0, 10)}.zip`;
    await invoke<string>("save_export_zip", { zipPath: result.zip_path, filename });
    toast(`✓ Exported ${result.workflow_count} workflow${result.workflow_count === 1 ? "" : "s"}`, "success");
  } catch (err) {
    if (err !== "cancelled") toast(`Export failed: ${err}`, "error");
  }
}

function buildIconBtn(id: string, title: string, svgInner: string): HTMLButtonElement {
  const btn = document.createElement("button");
  btn.id = id;
  btn.type = "button";
  btn.className = "btn-toolbar btn-icon-only";
  btn.title = title;
  btn.setAttribute("aria-label", title);
  btn.innerHTML = svgInner;
  return btn;
}


export function bindZoomControls(canvas: Canvas): void {
  const divider = document.getElementById("toolbar-divider-new-workflow")
    ?? document.querySelector<HTMLElement>("#toolbar .toolbar-divider");
  if (!divider?.parentElement) return; // defensive: nothing to anchor on

  const group = document.createElement("div");
  group.className = "toolbar-zoom-group";
  group.setAttribute("role", "group");
  group.setAttribute("aria-label", "Canvas zoom");

  const zoomOutBtn = buildIconBtn("btn-zoom-out", "Zoom out",
    `<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><line x1="5" y1="12" x2="19" y2="12"/></svg>`);
  const readout = document.createElement("span");
  readout.className = "toolbar-zoom-readout";
  readout.textContent = `${Math.round(canvas.zoom * 100)}%`;
  const zoomInBtn = buildIconBtn("btn-zoom-in", "Zoom in",
    `<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/></svg>`);
  // Corner-bracket "fit" glyph — path lineage: aerini-ui-hybrid-v2.html's own
  // verified <symbol id="i-fit"> (viewBox 16x16), coordinates scaled ×1.5 to
  // this codebase's 24x24 icon convention.
  const fitBtn = buildIconBtn("btn-fit-screen", "Fit to screen (Ctrl+Shift+F)",
    `<svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 9V3h6M21 9V3h-6M3 15v6h6M21 15v6h-6"/></svg>`);

  zoomOutBtn.addEventListener("click", () => canvas.zoomOut());
  zoomInBtn.addEventListener("click",  () => canvas.zoomIn());
  fitBtn.addEventListener("click",     () => canvas.fitToScreen());

  group.append(zoomOutBtn, readout, zoomInBtn, fitBtn);
  divider.insertAdjacentElement("afterend", group);
  group.insertAdjacentElement("afterend", divider.cloneNode(true) as HTMLElement);

  // Polling, not canvas.onZoomChange: that single-callback slot is already
  // assigned by app.ts (the zoom-percentage hint) — a second assignment
  // here would run later (bindToolbar is called after app.ts's own
  // assignment) and silently overwrite it. Polling is self-contained and,
  // as a side benefit, uniformly covers every way zoom can change (wheel,
  // pinch, zoomIn/Out, fitToScreen) without adding wiring into InputHandler.ts.
  let lastPct = Math.round(canvas.zoom * 100);
  setInterval(() => {
    const pct = Math.round(canvas.zoom * 100);
    if (pct !== lastPct) { readout.textContent = `${pct}%`; lastPct = pct; }
    zoomOutBtn.disabled = canvas.zoom <= canvas.MIN_ZOOM;
    zoomInBtn.disabled  = canvas.zoom >= canvas.MAX_ZOOM;
  }, 300);
}

export function bindDrawerToggle(canvas: Canvas): void {
  const drawer = document.getElementById("output-drawer");
  const header = document.getElementById("drawer-header");
  const closeBtn = document.getElementById("btn-close-drawer");
  if (!drawer || !header || !closeBtn) return; // defensive — mirrors bindZoomControls' no-throw contract

  function setDrawerCollapsed(collapsed: boolean): void {
    drawer!.classList.toggle("hidden", collapsed);
    header!.setAttribute("aria-expanded", String(!collapsed));
    canvas.resize();
    // Whichever control triggered this toggle must release focus, or it
    // keeps intercepting Space app-wide (InputHandler.ts's focusOwnsSpace
    // guard defers Space to whatever button/role=button is focused) and
    // keeps showing a stale focus ring.
    header!.blur();
    closeBtn!.blur();
  }
  closeBtn.addEventListener("click", (e) => {
    e.stopPropagation(); // header's own click handler below would otherwise re-toggle it back open
    setDrawerCollapsed(true);
  });
  header.addEventListener("click", (e) => {
    if (isMonitorModeActive()) return;
    if ((e.target as HTMLElement).closest(".drawer-actions, .drawer-tabs, .drawer-tab-static")) return;
    setDrawerCollapsed(!drawer!.classList.contains("hidden"));
  });
  header.addEventListener("keydown", (e) => {
    if (isMonitorModeActive()) return;
    if (e.key !== "Enter" && e.key !== " ") return;
    if ((e.target as HTMLElement).closest(".drawer-actions, .drawer-tabs, .drawer-tab-static")) return;
    e.preventDefault();
    setDrawerCollapsed(!drawer!.classList.contains("hidden"));
  });
}

export function bindToolbar(
  canvas:           Canvas,
  wfManager:        WorkflowManager,
  runManager:       RunManager,
  chatPanel:        IChatPanel,
  toast:            Toast,
  updateStatusHint: () => void,
): ToolbarResult {
  const $ = (id: string) => document.getElementById(id)!;

  const approvedForExecution = new Set<string>();

  const runWithValidation = async () => {
    if (runManager.isRunning) return;
    const errors = validateWorkflow(canvas);
    if (errors.length) {
      toast(`Fix before running:\n${errors.slice(0, 3).join("\n")}`, "error");
      return;
    }
    const triggerIsWebhook = findTriggerNodeTypeId(canvas.nodes.values()) === NODE_IDS.WEBHOOK;
    if (triggerIsWebhook && chatPanel.isOpen()) {
      const startInstead = await showConfirm(
        "This runs once and waits up to 60s for a single webhook call \u2014 Chat won't get a reply this way. Start the workflow instead for an interactive session?",
        false,
        "Start Instead",
        "neutral",
      );
      if (startInstead) { chatPanel.startForChat(); return; }
    }
    if (!await checkDangerousNodes(wfManager.currentId, canvas.nodes.values(), approvedForExecution, showConfirm)) return;
    runManager.handleRun(wfManager.currentId, wfManager.currentName);
  };

  const runMainBtn = $("btn-run-main") as HTMLButtonElement;

  const refreshRunBtn = () => {
    const schedulerRunning = getBgJobs().some(
      j => j.id === wfManager.currentId && j.status === "running"
    );
    const isRunning = runManager.isRunning || schedulerRunning;
    runMainBtn.disabled = false;
    runMainBtn.textContent = isRunning ? "Stop" : "Run";
    runMainBtn.classList.toggle("btn-run-stop", isRunning);
    const triggerIsWebhook = findTriggerNodeTypeId(canvas.nodes.values()) === NODE_IDS.WEBHOOK;
    runMainBtn.title = schedulerRunning && !runManager.isRunning
      ? "Stop this scheduled background run"
      : !isRunning && triggerIsWebhook
      ? "Runs once — waits up to 60s for a single webhook request, then stops. For Chat or a persistent listener, use Start in the Chat panel instead."
      : "";
  };

  const runWrap = document.getElementById("run-dropdown-wrap");
  let _flashTimer: ReturnType<typeof setTimeout> | null = null;

  runManager.addRunStateListener((running: boolean) => {
    refreshRunBtn();
    if (running) {
      if (_flashTimer) { clearTimeout(_flashTimer); _flashTimer = null; }
      runWrap?.setAttribute("data-run-state", "running");
    } else if (!_flashTimer) {
      runWrap?.setAttribute("data-run-state", "idle");
    }
  });

  runManager.onRunResult = (success: boolean) => {
    runWrap?.setAttribute("data-run-state", success ? "success" : "error");
    _flashTimer = setTimeout(() => {
      runWrap?.setAttribute("data-run-state", "idle");
      _flashTimer = null;
    }, 300);
  };

  runMainBtn.addEventListener("click", () => {
    const schedulerRunning = getBgJobs().some(
      j => j.id === wfManager.currentId && j.status === "running"
    );
    if (runManager.isRunning) {
      runManager.forceReset();
    } else if (schedulerRunning) {
      stopScheduledWorkflow(wfManager.currentId)
        .then(() => {
          refreshRunBtn();
          loadBgPanel().then(m => m.updateBgRunButton(wfManager.currentId));
          toast("Workflow stopped", "success");
        })
        .catch(e => toast(`Could not stop: ${e}`, "error"));
    } else {
      runWithValidation();
    }
  });

  $("btn-save").addEventListener("click",    () => wfManager.handleSave());
  bindAlwaysOnToggle(canvas, wfManager, toast);
  $("btn-versions").addEventListener("click", () =>
    import("./panels/VersionPanel").then(m => m.showVersionPanel(wfManager, toast))
  );
  bindWfSettings(wfManager);

  $("btn-run-now")?.addEventListener("click", () => { closeAllDropdowns(); runWithValidation(); });
  $("btn-run-chevron").addEventListener("click", (e) => {
    e.stopPropagation();
    const menu   = $("run-dropdown-menu");
    const isOpen = menu.classList.contains("open");
    closeAllDropdowns();
    if (!isOpen) {
      menu.classList.add("open");
      $("btn-run-chevron").setAttribute("aria-expanded", "true");
    }
  });
  $("btn-bg-run")?.addEventListener("click", async () => {
    closeAllDropdowns();
    const snapshot = await wfManager.prepareForBgRun();
    if (!snapshot) return;
    try {
      const started = await runManager.executeBgJob(snapshot);
      if (started) {
        activateZone("bgruns");
        toast("Workflow scheduled — running in background", "success");
      }
    } catch (rawError) {
      const err = parseSchedulerError(String(rawError));
      if (err.error_kind === "port_conflict") {
        toast(`Port ${err.port} is already in use by "${err.held_by_workflow_name}".`, "error");
      } else {
        toast(`Could not start background run: ${(err as { message?: string }).message ?? rawError}`, "error");
      }
    }
  });

  $("btn-export").addEventListener("click", (e) => {
    e.stopPropagation();
    const menu   = $("export-dropdown-menu");
    const isOpen = menu.classList.contains("open");
    closeAllDropdowns();
    if (!isOpen) {
      menu.classList.add("open");
      $("btn-export").setAttribute("aria-expanded", "true");
    }
  });
  $("btn-export-aerini")?.addEventListener("click", () => { closeAllDropdowns(); wfManager.handleExport(); });
  $("btn-export-server")?.addEventListener("click", () => { closeAllDropdowns(); import("./export-server-panel").then(m => m.showExportServerPanel(wfManager.currentId, toast)); });
  $("btn-export-all")?.addEventListener("click", () => { closeAllDropdowns(); handleExportAll(toast); });

  document.addEventListener("click", () => closeAllDropdowns());

  $("btn-new-workflow").addEventListener("click", () => wfManager.handleNew());
  $("btn-new-workflow-toolbar").addEventListener("click", () => wfManager.handleNew());
  $("btn-wf-add-collection").addEventListener("click", () => wfManager.createCollection());
  $("btn-wf-select-mode").addEventListener("click", () => {
    wfManager.toggleSelectMode();
    const btn = $("btn-wf-select-mode");
    btn.classList.toggle("active", wfManager.selectMode);
    btn.setAttribute("aria-pressed", String(wfManager.selectMode));
  });
  let _credPanel: { show(): void } | null = null;
  let _credLoading = false;
  $("btn-credentials").addEventListener("click", () => {
    if (_credPanel)   { _credPanel.show(); return; }
    if (_credLoading) return;
    _credLoading = true;
    import("./panels/CredentialPanel").then(({ CredentialPanel }) => {
      _credPanel = new CredentialPanel();
      _credPanel.show();
    });
  });
  $("btn-chat")?.addEventListener("click", () => chatPanel.toggle());
  // Collapse/expand the output drawer. The drawer keeps using the existing
  bindDrawerToggle(canvas);
  $("btn-focus-mode").addEventListener("click", () => canvas.toggleFocusMode());

  $("btn-settings").addEventListener("click", async () => {
    $("settings-modal").classList.remove("hidden");
    if (isTauri()) {
      const el = document.getElementById("setting-autostart") as HTMLInputElement | null;
      if (el) { try { el.checked = await getAutostart(); } catch { /* ignore */ } }
    }
  });
  (document.getElementById("setting-autostart") as HTMLInputElement | null)
    ?.addEventListener("change", async (e) => {
      if (!isTauri()) return;
      const enabled = (e.target as HTMLInputElement).checked;
      try {
        await setAutostart(enabled);
        toast(enabled ? "Aerini will now launch at login" : "Launch at login disabled", "success");
      } catch (err) {
        toast(`Could not update launch at login: ${err}`, "error");
        (e.target as HTMLInputElement).checked = !enabled;
      }
    });

  $("btn-check-updates")?.addEventListener("click", async () => {
    if (!isTauri()) return;
    const btn = $("btn-check-updates") as HTMLButtonElement;
    const statusEl = document.getElementById("update-check-status");
    btn.disabled = true;
    if (statusEl) statusEl.textContent = "Checking…";
    try {
      const result = await checkForUpdate();
      if (!statusEl) return;
      statusEl.textContent = "";
      if (result.is_newer) {
        const msg = document.createElement("span");
        msg.textContent = `Update available: v${result.latest_version} — `;
        const link = document.createElement("a");
        link.textContent = "View release";
        link.href = result.release_url;
        link.target = "_blank";
        link.rel = "noopener noreferrer";
        link.className = "settings-support-link";
        statusEl.appendChild(msg);
        statusEl.appendChild(link);
      } else {
        statusEl.textContent = `You're up to date (v${result.current_version}).`;
      }
    } catch (err) {
      if (statusEl) statusEl.textContent = "Could not check for updates.";
      toast(`Update check failed: ${err}`, "error");
    } finally {
      btn.disabled = false;
    }
  });

  const gridSnapEl = document.getElementById("setting-grid-snap") as HTMLInputElement | null;
  if (gridSnapEl) {
    gridSnapEl.checked = localStorage.getItem("aerini_grid_snap") === "true";
    gridSnapEl.addEventListener("change", () => {
      localStorage.setItem("aerini_grid_snap", String(gridSnapEl.checked));
    });
  }

  const autofitEl = document.getElementById("setting-autofit") as HTMLInputElement | null;
  if (autofitEl) {
    autofitEl.checked = localStorage.getItem("aerini_autofit") !== "false";
    autofitEl.addEventListener("change", () => {
      localStorage.setItem("aerini_autofit", String(autofitEl.checked));
    });
  }

  $("btn-run-panel")?.addEventListener("click",  () => runWithValidation());
  $("btn-copy-output").addEventListener("click", () => {
    const text = document.getElementById("output-content")?.innerText ?? "";
    navigator.clipboard.writeText(text).then(() => {
      const btn = $("btn-copy-output");
      btn.classList.add("copied");
      toast("Copied to clipboard", "success");
      setTimeout(() => btn.classList.remove("copied"), 1500);
    });
  });
  $("btn-expand-output").addEventListener("click", () => {
    const drawer = $("output-drawer");
    const isMax  = drawer.style.height === "70vh";
    drawer.style.height = isMax ? "var(--drawer-h)" : "70vh";
  });

  const titleEl = $("workflow-name-label");
  titleEl.addEventListener("dblclick", () => wfManager.startRename(titleEl));

  window.addEventListener("keydown", e => {
    if (isMonitorModeActive()) return;
    const inInput = e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement;
    if ((e.metaKey || e.ctrlKey) && e.key === "s")     { e.preventDefault(); wfManager.handleSave(); return; }
    if ((e.metaKey || e.ctrlKey) && e.key === "n")     { e.preventDefault(); wfManager.handleNew(); return; }
    if ((e.metaKey || e.ctrlKey) && e.key === "Enter") { e.preventDefault(); runWithValidation(); return; }
    if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key === "F") { e.preventDefault(); canvas.fitToScreen(); return; }
    if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key === "C") { e.preventDefault(); chatPanel.toggle(); return; }
    if (inInput) return;
    if (e.key === "?" || e.key === "/") { $("shortcuts-modal").classList.remove("hidden"); return; }
    if (e.key === "Escape") {
      $("shortcuts-modal").classList.add("hidden");
    }
    if (e.key === "m" || e.key === "M") {
      const w      = document.getElementById("minimap-wrap");
      const hidden = w?.classList.toggle("minimap-hidden");
      localStorage.setItem("aerini_minimap_hidden", String(!!hidden));
    }
  });

  // Minimap toggle
  const minimapWrap   = document.getElementById("minimap-wrap");
  const minimapBtn    = document.getElementById("btn-minimap-toggle");
  const minimapHidden = localStorage.getItem("aerini_minimap_hidden") === "true";
  if (minimapHidden) minimapWrap?.classList.add("minimap-hidden");
  minimapBtn?.addEventListener("click", () => {
    const hidden = minimapWrap?.classList.toggle("minimap-hidden");
    localStorage.setItem("aerini_minimap_hidden", String(!!hidden));
  });

  bindZoomControls(canvas);

  bindDrawerResize();

  // Defer BgJobsPanel init — runs after first paint, invisible to the user
  const _deferBgPanel = (fn: () => void) =>
    typeof requestIdleCallback === "function"
      ? requestIdleCallback(fn, { timeout: 200 })
      : setTimeout(fn, 0);

  _deferBgPanel(() => {
    loadBgPanel().then(m =>
      m.renderBgJobs(wfManager, runManager, "all", "", toast)
    );
  });

  // Trigger initial always-on button state
  updateAlwaysOnBtn(canvas, wfManager);
  chatPanel.refreshButtonVisibility();
  updateStatusHint();

  return { refreshRunBtn, runWithValidation };
}

function closeAllDropdowns(): void {
  document.querySelectorAll(".toolbar-dropdown-menu").forEach(m => m.classList.remove("open"));
  document.getElementById("btn-run-chevron")?.setAttribute("aria-expanded", "false");
  document.getElementById("btn-export")?.setAttribute("aria-expanded", "false");
}
