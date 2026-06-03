import { Canvas } from "./canvas/Canvas";
import { switchTab, openPanel, closePanel } from "./panels/NodeConfigPanel";
import { CredentialPanel } from "./panels/CredentialPanel";
import { deserialize, registerNodeDescriptors } from "./canvas/CanvasSerializer";
import type { NodeDescriptor } from "./ipc/workflow";
import { stopScheduledWorkflow, startScheduledWorkflow, getScheduledJobs, setAlwaysOn, getNodeTypes, parseSchedulerError, checkNodejsAvailable, type WorkflowResult, type ScheduledJobRow } from "./ipc/workflow";
import { WorkflowManager, setWorkflowRunning } from "./workflow-manager";
import { RunManager, getBgJobs, onBgJobsChanged, updateBgJobStoreFromEvent, removeBgJob, hydrateBgJobsFromScheduler } from "./run-manager";
import { saveRunToHistory } from "./run-history";
import {
  buildSidebarPalette, bindSidebarSearch,
  initCommandPalette, openPalette,
} from "./palette-manager";
import { initModals } from "./modal-manager";
import { bindDropImport, bindFileInput } from "./drag-drop";
import { showPopover, closePopover, setDescriptorRegistry } from "./popover-config";
import { listenCloseRequested, listenSchedulerStatus, listenSchedulerSkip, listenNodeStatus, requestSchedulerState, type SchedulerStatusEvent } from "./ipc/events";
import { initSidebarSections, bindSectionSearchToggles, bindWorkflowSectionControls, bindBgRunsFilter, updateActivityBadge, updateBgRunningState, activateZone, getCurrentZone } from "./sidebar-sections";
import type { CanvasNode } from "./canvas/Node";
import { getAutostart, setAutostart } from "./ipc/autostart";
import { isTauri, escapeHtml as escHtml } from "./utils";

import { showExportServerPanel } from "./export-server-panel";

// ── Confirm dialog (replaces window.confirm, which Tauri webview suppresses) ──
// Returns a Promise<boolean>. Resolves true on OK, false on Cancel.
export function showConfirm(message: string, isDanger = false): Promise<boolean> {
  return new Promise(resolve => {
    const modal   = document.getElementById("confirm-modal")!;
    const msgEl   = document.getElementById("confirm-message")!;
    const okBtn   = document.getElementById("confirm-ok")! as HTMLButtonElement;
    const cancelBtn = document.getElementById("confirm-cancel")!;
    const backdrop  = modal.querySelector(".confirm-backdrop")!;

    msgEl.textContent = message;
    // UX-08: destructive actions get red OK button, neutral ones get blue
    okBtn.classList.toggle("confirm-danger", isDanger);
    modal.classList.remove("hidden");

    const cleanup = (result: boolean) => {
      modal.classList.add("hidden");
      okBtn.removeEventListener("click", onOk);
      cancelBtn.removeEventListener("click", onCancel);
      backdrop.removeEventListener("click", onCancel);
      resolve(result);
    };

    const onOk     = () => cleanup(true);
    const onCancel = () => cleanup(false);

    okBtn.addEventListener("click", onOk);
    cancelBtn.addEventListener("click", onCancel);
    backdrop.addEventListener("click", onCancel);
  });
}

// ── App bootstrap ─────────────────────────────────────────────────────────────

async function init() {
  const allNodes = await getNodeTypes().catch((): NodeDescriptor[] => []);
  registerNodeDescriptors(allNodes);
  setDescriptorRegistry(allNodes);

  const canvasEl   = document.getElementById("canvas") as HTMLCanvasElement;
  const canvas     = new Canvas(canvasEl);
  // Expose canvas on the DOM element so popover positioning can access zoom/pan
  (canvasEl as unknown as Record<string, unknown>).__canvas = canvas;
  const credPanel   = new CredentialPanel();

  function toast(msg: string, type: "success" | "error" | "info" = "info") {
    const colors = { success: "var(--green)", error: "var(--red)", info: "var(--blue)" };
    const t = document.createElement("div");
    t.className = "toast"; t.style.background = colors[type]; t.textContent = msg;
    document.body.appendChild(t);
    setTimeout(() => { t.style.opacity = "0"; setTimeout(() => t.remove(), 300); }, 2800);
  }

  function setStatus(m: string) {
    const el = document.getElementById("status-text"); if (el) el.textContent = m;
  }
  function setTitle(n: string) {
    const el = document.getElementById("workflow-name-label");
    if (el) { el.textContent = n; el.title = n; }
  }
  function markUnsaved(on: boolean) {
    document.getElementById("unsaved-dot")?.classList.toggle("visible", on);
  }
  function updateStatusHint() {
    const el = document.getElementById("status-hint"); if (!el) return;
    if (canvas.nodes.size === 0) el.textContent = "Click a node to place it · Space to search · Ctrl+S to save";
    else if (canvas.selectedNode) el.textContent = "Del to delete · Ctrl+D to duplicate · Double-click to configure";
    else el.textContent = "Double-click node to configure · Ctrl+S to save · Ctrl+Enter to run";
  }

  if (!isTauri()) document.getElementById("browser-run-notice")?.classList.remove("hidden");

  // WorkflowManager — pass showConfirm so it uses the modal, not window.confirm
  const wfManager = new WorkflowManager(canvas, {
    onUnsaved: markUnsaved,
    onTitle:   setTitle,
    onStatus:  setStatus,
    onToast:   toast,
    confirm:     showConfirm,
    onPanelClose: () => { closePanel(); closePopover(); },
  });

  // After loading or creating a workflow, switch to Nodes zone so the
  // user can immediately start placing nodes without an extra click.
  // Guard: only switch when the user is already in the Workflows zone —
  // navigating from Background Runs must not clobber the user's zone choice.
  wfManager.onNavigate = () => {
    if (getCurrentZone() === "workflows") activateZone("nodes");
    updateAlwaysOnBtn();
    updateBgRunButton(wfManager.currentId);
    refreshRunBtn();
    // Keep RunManager's currentWorkflowId in sync — it uses this to route
    // node-status events to the canvas and to save history under the right ID.
    runManager.setCurrentWorkflow(
      wfManager.currentId,
      wfManager.currentName,
      wfManager.parallelExecution,
      wfManager.maxConcurrentNodes,
    );
  };

  // Shown only when current workflow has a Schedule or Webhook trigger node.

  async function updateAlwaysOnBtn(): Promise<void> {
    const btn = document.getElementById("btn-always-on") as HTMLButtonElement | null;
    if (!btn) return;

    const hasSchedulableTrigger = [...canvas.nodes.values()].some(n =>
      n.data.node_type_id === "schedule" || n.data.node_type_id === "webhook"
    );

    const wrap = document.getElementById("always-on-wrap");
    if (!hasSchedulableTrigger) {
      btn.classList.add("hidden");
      // Keep the wrapper visible so its tooltip explains how to enable Auto-start
      if (wrap) wrap.style.display = "";
      return;
    }

    btn.classList.remove("hidden");
    // Hide wrapper tooltip hint — button is now visible and self-explanatory
    if (wrap) wrap.removeAttribute("data-tooltip");

    // Check if this workflow is registered and always_on
    let alwaysOn = false;
    if (isTauri()) {
      try {
        const jobs = await getScheduledJobs();
        const job  = jobs.find((j: ScheduledJobRow) => j.workflow_id === wfManager.currentId);
        alwaysOn   = job?.always_on ?? false;
      } catch { /* non-fatal */ }
    }

    btn.classList.toggle("btn-always-on--active", alwaysOn);
    btn.title = alwaysOn
      ? "Auto-start is ON — this workflow starts automatically when Flowo opens. Click to disable."
      : "Auto-start is OFF — click to make this workflow start automatically when Flowo opens.";
  }

  canvas.onCanvasChanged = () => {
    wfManager.markUnsaved(true);
    wfManager.scheduleAutoSave();
    updateStatusHint();
    // Only sync check — no IPC call on every canvas change
    const btn = document.getElementById("btn-always-on");
    if (btn) {
      const hasSchedulableTrigger = [...canvas.nodes.values()].some(n =>
        n.data.node_type_id === "schedule" || n.data.node_type_id === "webhook"
      );
      btn.classList.toggle("hidden", !hasSchedulableTrigger);
    }
  };

  const runManager = new RunManager(canvas, setStatus, toast);

  // Init variable interpolation autocomplete (fires on {{ in any config field)
  initInterpolationAutocomplete(canvas);

  // Canvas callbacks
  // Single click → reveal output/error in drawer (non-intrusive)
  canvas.onNodeClicked = n => {
    if (n.status === "error")   runManager.showNodeErrorDetail(n.data.id, n.data.name);
    if (n.status === "success") runManager.revealNodeOutput(n.data.id, n.data.name);
  };

  // Double-click → open config popover (intentional, not accidental)
  canvas.onNodeSelected = n => {
    if (n) {
      // Note nodes get inline canvas editing instead of the popover
      if (n.data.node_type_id === "note") {
        showNoteEditor(n, canvasEl, () => {
          wfManager.markUnsaved(true);
          wfManager.scheduleAutoSave();
        });
        return;
      }
      showPopover(n, canvasEl, () => {
        wfManager.markUnsaved(true);
        wfManager.scheduleAutoSave();
      });
    } else {
      closePopover();
    }
  };
  // canvas.onCanvasChanged is set above in the Always On block
  canvas.onPaletteRequest = openPalette;
  canvas.onPanelClose     = () => { closePanel(); closePopover(); };
  canvas.onRunNode        = (nodeId) => runManager.handleRunSingleNode(nodeId, wfManager.currentId, wfManager.currentName);

  // Wire-drop: connector released on empty space → open node picker at drop point
  canvas.onWireDropRequest = (_fromNode, _fromPort, _wx, _wy) => {
    openWireDropPicker(allNodes, canvas, canvasEl, setStatus);
  };

  document.getElementById("canvas")!.addEventListener("mousedown", () => {
    (document.getElementById("node-search") as HTMLInputElement)?.blur();
  }, { capture: true });

  buildSidebarPalette(allNodes, canvas, setStatus);
  bindSidebarSearch();
  initCommandPalette(allNodes, canvas, setStatus);
  initModals(allNodes, (obj) => {
    try {
      const { id, name, nodes, connectors, parallelExecution, maxConcurrentNodes } = deserialize(JSON.stringify(obj));
      canvas.nodes = nodes; canvas.connectors = connectors;
      canvas.clearSelection(); canvas.fitToScreen();
      wfManager.parallelExecution  = parallelExecution;
      wfManager.maxConcurrentNodes = maxConcurrentNodes;
      wfManager.currentId = id; wfManager.currentName = name;
      wfManager.markUnsaved(false); setTitle(name);
      document.getElementById("output-drawer")!.classList.add("hidden");
      runManager.clearLogs();
      setStatus(`Imported "${name}"`);
      wfManager.refreshWorkflowList();
      toast(`Imported "${name}"`, "success");
    } catch (e) { toast(`Import failed: ${e}`, "error"); }
  });
  bindDropImport(toast);
  bindFileInput(toast);

  // Toolbar
  const $ = (id: string) => document.getElementById(id)!;
  $("btn-save").addEventListener("click",      () => wfManager.handleSave());

  $("btn-always-on").addEventListener("click", async () => {
    const btn = document.getElementById("btn-always-on") as HTMLButtonElement;
    const isActive = btn.classList.contains("btn-always-on--active");
    btn.disabled = true;
    try {
      const enabling = !isActive;
      if (enabling) {
        const snapshot = await wfManager.prepareForBgRun();
        if (!snapshot) { btn.disabled = false; return; }
        const jobs = await getScheduledJobs();
        const existingJob = jobs.find((j: ScheduledJobRow) => j.workflow_id === wfManager.currentId);
        // Only skip start_job if the job is already actively running (status="active").
        // A registered-but-stopped job must be restarted now, not just flagged.
        const isRunning = existingJob?.status === "active";
        if (!isRunning) {
          // Start now with alwaysOn=true — atomically registers + arms + flags.
          await startScheduledWorkflow(wfManager.currentId, undefined, true);
        } else {
          // Already running — flip the always_on flag without touching the run loop.
          await setAlwaysOn(wfManager.currentId, true);
        }
      } else {
        await setAlwaysOn(wfManager.currentId, false);
      }
      btn.classList.toggle("btn-always-on--active", enabling);
      btn.title = enabling
        ? "Auto-start is ON — this workflow starts automatically when Flowo opens. Click to disable."
        : "Auto-start is OFF — click to make this workflow start automatically when Flowo opens.";
      toast(enabling ? "Auto-start enabled — workflow will start on every app launch" : "Auto-start disabled", enabling ? "success" : "info");
    } catch (e) {
      toast(`Could not update auto-start: ${e}`, "error");
    } finally {
      btn.disabled = false;
    }
  });
  $("btn-versions").addEventListener("click",  () => showVersionPanel(wfManager, toast));

  // Workflow settings modal
  $("btn-wf-settings").addEventListener("click", () => {
    const modal        = $("wf-settings-modal");
    const parallelChk  = document.getElementById("wf-setting-parallel") as HTMLInputElement;
    const concurrencyRow = document.getElementById("wf-setting-concurrency-row") as HTMLElement;
    const maxConcInput = document.getElementById("wf-setting-max-concurrent") as HTMLInputElement;
    parallelChk.checked  = wfManager.parallelExecution;
    maxConcInput.value   = String(wfManager.maxConcurrentNodes);
    concurrencyRow.hidden = !wfManager.parallelExecution;
    modal.classList.remove("hidden");
  });
  $("btn-close-wf-settings").addEventListener("click", () => {
    $("wf-settings-modal").classList.add("hidden");
  });
  document.getElementById("wf-setting-parallel")!.addEventListener("change", (e) => {
    const checked = (e.target as HTMLInputElement).checked;
    wfManager.parallelExecution = checked;
    wfManager.markUnsaved(true);
    const row = document.getElementById("wf-setting-concurrency-row")!;
    row.hidden = !checked;
  });
  document.getElementById("wf-setting-max-concurrent")!.addEventListener("change", (e) => {
    const val = parseInt((e.target as HTMLInputElement).value, 10);
    if (!isNaN(val) && val >= 1 && val <= 64) {
      wfManager.maxConcurrentNodes = val;
      wfManager.markUnsaved(true);
    }
  });
  $("wf-settings-modal").addEventListener("click", (e) => {
    if (e.target === $("wf-settings-modal")) $("wf-settings-modal").classList.add("hidden");
  });

  const approvedForExecution = new Set<string>();
  const DANGEROUS_NODE_TYPES = new Set(["shell_exec", "code", "file"]);

  const checkDangerousNodes = async (workflowId: string): Promise<boolean> => {
    const dangerousNodes = [...canvas.nodes.values()]
      .filter(n => DANGEROUS_NODE_TYPES.has(n.data.node_type_id));
    if (!dangerousNodes.length) return true;
    const dangerousIds = dangerousNodes.map(n => n.data.id).sort().join(",");
    const approvalKey = `${workflowId}::${dangerousIds}`;
    if (approvedForExecution.has(approvalKey)) return true;
    const nodeTypes = dangerousNodes
      .map(n => n.data.name || n.data.node_type_id)
      .join(", ");
    const confirmed = await showConfirm(
      `This workflow contains nodes that execute code on your computer:\n\n${nodeTypes}\n\nOnly run workflows from sources you trust. Continue?`,
      true
    );
    if (confirmed) { approvedForExecution.add(approvalKey); return true; }
    return false;
  };

  // btn-bg-run is now wired inside the run dropdown block above
  const runWithValidation = async () => {
    // Guard: prevent double-fire from rapid clicks or concurrent triggers
    if (runManager.isRunning) return;
    const errors = validateWorkflow(canvas);
    if (errors.length) {
      toast(`Fix before running:\n${errors.slice(0,3).join("\n")}`, "error");
      return;
    }
    if (!await checkDangerousNodes(wfManager.currentId)) return;
    runManager.handleRun(wfManager.currentId, wfManager.currentName);
  };

  // Run dropdown wiring
  // btn-run-main toggles between Run and Stop.
  // While running: clicking it calls forceReset() to unblock the UI immediately.
  // The backend task continues until it finishes or times out, but the UI is freed.
  const runMainBtn = $("btn-run-main") as HTMLButtonElement;

  // refreshRunBtn — single source of truth for the Run/Stop button label and behaviour.
  // Called after every state change: in-process run start/stop, scheduler events,
  // and workflow loads. Reads both the in-process runner state AND the scheduler
  // daemon state so the button always reflects what is actually happening.
  const refreshRunBtn = () => {
    const schedulerRunning = getBgJobs().some(
      j => j.id === wfManager.currentId && j.status === "running"
    );
    const isRunning = runManager.isRunning || schedulerRunning;
    runMainBtn.disabled = false;
    runMainBtn.textContent = isRunning ? "Stop" : "Run";
    runMainBtn.classList.toggle("btn-run-stop", isRunning);
    runMainBtn.title = schedulerRunning && !runManager.isRunning
      ? "Stop this scheduled background run"
      : "";
  };

  runManager.onRunStateChange = (_running: boolean) => { refreshRunBtn(); };

  runMainBtn.addEventListener("click", () => {
    const schedulerRunning = getBgJobs().some(
      j => j.id === wfManager.currentId && j.status === "running"
    );
    if (runManager.isRunning) {
      // In-process run — cancel it (backend continues until timeout)
      runManager.forceReset();
    } else if (schedulerRunning) {
      // Scheduled daemon run — stop it via IPC
      stopScheduledWorkflow(wfManager.currentId)
        .then(() => { refreshRunBtn(); updateBgRunButton(wfManager.currentId); })
        .catch(e => toast(`Could not stop: ${e}`, "error"));
    } else {
      runWithValidation();
    }
  });
  $("btn-run-now")?.addEventListener("click", () => {
    closeAllDropdowns();
    runWithValidation();
  });
  $("btn-run-chevron").addEventListener("click", (e) => {
    e.stopPropagation();
    const menu = $("run-dropdown-menu");
    const isOpen = menu.classList.contains("open");
    closeAllDropdowns();
    if (!isOpen) menu.classList.add("open");
  });
  $("btn-bg-run")?.addEventListener("click", async () => {
    closeAllDropdowns();
    const snapshot = await wfManager.prepareForBgRun();
    if (!snapshot) return;
    try {
      const started = await runManager.executeBgJob(snapshot);
      if (started) {
        // Keep the user on the current canvas so they can observe the running
        // state. Switch the sidebar to Background Runs so the job is visible.
        activateZone("bgruns");
        toast("Workflow scheduled — running in background", "success");
      }
    } catch {
      // port_conflict re-throws — modal handled elsewhere, stay on canvas.
    }
  });

  // Export dropdown wiring
  $("btn-export").addEventListener("click", (e) => {
    e.stopPropagation();
    const menu = $("export-dropdown-menu");
    const isOpen = menu.classList.contains("open");
    closeAllDropdowns();
    if (!isOpen) menu.classList.add("open");
  });
  $("btn-export-flowo")?.addEventListener("click", () => {
    closeAllDropdowns();
    wfManager.handleExport();
  });
  $("btn-export-server")?.addEventListener("click", () => {
    closeAllDropdowns();
    showExportServerPanel(wfManager.currentId, toast);
  });

  // Close dropdowns on outside click
  document.addEventListener("click", () => closeAllDropdowns());

  $("btn-new-workflow").addEventListener("click", () => wfManager.handleNew());
  $("btn-credentials").addEventListener("click",  () => credPanel.show());
  $("btn-close-drawer").addEventListener("click", () => {
    $("output-drawer").classList.add("hidden");
    document.documentElement.style.removeProperty("--drawer-offset");
    canvas.resize();
  });
  $("btn-focus-mode").addEventListener("click",   () => canvas.toggleFocusMode());
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
        toast(enabled ? "Flowo will now launch at login" : "Launch at login disabled", "success");
      } catch (err) {
        toast(`Could not update launch at login: ${err}`, "error");
        (e.target as HTMLInputElement).checked = !enabled;
      }
    });

  // Grid snap — wire checkbox to localStorage; Canvas.ts reads flowo_grid_snap on drag.
  const gridSnapEl = document.getElementById("setting-grid-snap") as HTMLInputElement | null;
  if (gridSnapEl) {
    gridSnapEl.checked = localStorage.getItem("flowo_grid_snap") === "true";
    gridSnapEl.addEventListener("change", () => {
      localStorage.setItem("flowo_grid_snap", String(gridSnapEl.checked));
    });
  }

  // Auto-fit on load — wire checkbox to localStorage; workflow-manager.ts reads flowo_autofit.
  const autofitEl = document.getElementById("setting-autofit") as HTMLInputElement | null;
  if (autofitEl) {
    autofitEl.checked = localStorage.getItem("flowo_autofit") !== "false";
    autofitEl.addEventListener("change", () => {
      localStorage.setItem("flowo_autofit", String(autofitEl.checked));
    });
  }
  $("btn-shortcuts").addEventListener("click",    () => $("shortcuts-modal").classList.remove("hidden"));
  $("btn-run-panel")?.addEventListener("click",   () => runWithValidation());
  $("btn-copy-output").addEventListener("click",  () => {
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
    const isMax = drawer.style.height === "70vh";
    drawer.style.height = isMax ? "var(--drawer-h)" : "70vh";
    setTimeout(() => RunManager.updateDrawerOffset(canvas), 20);
  });

  // Panel tabs — Run and Logs only (Config moved to popover)
  document.querySelectorAll(".panel-tab").forEach(t =>
    t.addEventListener("click", () => {
      const tab = (t as HTMLElement).dataset.tab ?? "run";
      switchTab(tab);
      openPanel();
    })
  );
  $("btn-close-panel").addEventListener("click", () => { closePanel(); });

  // Rename on double-click
  const titleEl = $("workflow-name-label");
  titleEl.addEventListener("dblclick", () => wfManager.startRename(titleEl));

  // Keyboard shortcuts
  window.addEventListener("keydown", e => {
    const inInput = e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement;
    if ((e.metaKey || e.ctrlKey) && e.key === "s")     { e.preventDefault(); wfManager.handleSave(); return; }
    if ((e.metaKey || e.ctrlKey) && e.key === "n")     { e.preventDefault(); wfManager.handleNew(); return; }
    if ((e.metaKey || e.ctrlKey) && e.key === "Enter") { e.preventDefault(); runWithValidation(); return; }
    if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key === "F") { e.preventDefault(); canvas.fitToScreen(); return; }
    if (inInput) return;
    if (e.key === "?" || e.key === "/") { $("shortcuts-modal").classList.remove("hidden"); return; }
    if (e.key === "Escape") {
      $("shortcuts-modal").classList.add("hidden");
      // Also close the node config panel if open and no modal is blocking.
      // Matches macOS HIG and power-user muscle memory (n8n, Figma).
      const panel = document.getElementById("right-panel");
      const otherModalIds = ["confirm-modal", "import-preview-modal", "settings-modal", "onboarding-modal"];
      const anyModalVisible = otherModalIds.some(id => !document.getElementById(id)?.classList.contains("hidden"));
      if (!anyModalVisible && panel && panel.offsetParent !== null) {
        closePanel();
      }
    }
    if (e.key === "m" || e.key === "M") {
      const w = document.getElementById("minimap-wrap");
      const hidden = w?.classList.toggle("minimap-hidden");
      localStorage.setItem("flowo_minimap_hidden", String(!!hidden));
    }
  });

  bindDrawerResize();
  bindPanelResize();

  // Minimap toggle
  const minimapWrap = document.getElementById("minimap-wrap");
  const minimapBtn  = document.getElementById("btn-minimap-toggle");
  const minimapHidden = localStorage.getItem("flowo_minimap_hidden") === "true";
  if (minimapHidden) minimapWrap?.classList.add("minimap-hidden");
  minimapBtn?.addEventListener("click", () => {
    const hidden = minimapWrap?.classList.toggle("minimap-hidden");
    localStorage.setItem("flowo_minimap_hidden", String(!!hidden));
  });
  await wfManager.refreshWorkflowList();
  updateStatusHint();

  // Sidebar sections — collapse, reorder, resize, search
  initSidebarSections();
  bindSectionSearchToggles();
  bindWorkflowSectionControls((sortMode) => wfManager.setSortMode(sortMode));
  bindBgRunsFilter((status, query) => renderBgJobs(wfManager, runManager, status, query, toast));

  // Wire background-jobs sidebar section
  // In-memory store updates (for non-scheduler bg runs)
  onBgJobsChanged(() => {
    renderBgJobsDebounced(wfManager, runManager, "all", "", toast);
    updateBgRunButton(wfManager.currentId);
    refreshRunBtn();
  });
  renderBgJobs(wfManager, runManager, "all", "", toast);

  // Global node-status subscription — permanent, not per-run.
  // Routes every node-status event from the executor to the canvas.
  // Works for both manual runs (run_workflow IPC) and scheduler daemon runs.
  // onNodeStatusEvent filters by workflow_id so events for background
  // workflows that aren't currently loaded on the canvas are silently dropped.
  // Awaited so the webview listener is fully registered before init() continues —
  // events emitted before registration is complete are silently dropped by Tauri.
  await listenNodeStatus((evt) => {
    runManager.onNodeStatusEvent(evt.workflow_id, evt.node_id, evt.status);
  });

  // Session-scoped set of workflow IDs for which we have already switched the
  // sidebar to the Background Runs zone. Prevents the zone from switching
  // unexpectedly on every restart (run_count resets to 0 on each start_job call).
  const _bgZoneActivated = new Set<string>();

  // Rust scheduler daemon events — update sidebar on every state transition.
  // Awaited for the same reason as listenNodeStatus above.
  await listenSchedulerStatus((evt: SchedulerStatusEvent) => {
    updateBgJobStoreFromEvent(evt);
    renderBgJobsDebounced(wfManager, runManager, "all", "", toast);
    updateBgRunButton(wfManager.currentId);
    refreshRunBtn();
    // Reset canvas node states at the START of each new scheduler cycle so
    // the previous run's green/red colours don't bleed into the new run.
    // Only reset when the running workflow is the one currently on canvas.
    if (evt.status === "running" && evt.workflow_id === wfManager.currentId) {
      canvas.resetAllStatus();
      // If the output drawer is open and not showing history, replace its
      // content with a spinner so the user sees the run is starting.
      const drawer = document.getElementById("output-drawer");
      const drawerTabs = document.getElementById("drawer-tabs");
      const drawerContent = document.getElementById("output-content");
      if (drawer && !drawer.classList.contains("hidden")
          && drawerTabs && drawerContent
          && !drawer.querySelector(".history-panel")) {
        drawerTabs.innerHTML = `<span class="drawer-tab tab-neutral active">Running…</span>`;
        drawerContent.innerHTML = `<div class="run-spinner-wrap"><div class="run-spinner"></div><div class="run-spinner-label">Scheduled run in progress…</div></div>`;
      }
    }
    if ((evt.status === "stopped" || evt.status === "error") && evt.workflow_id === wfManager.currentId) {
      canvas.resetAllStatus();
    }
    // Update the workflow list running dot
    setWorkflowRunning(evt.workflow_id,
      evt.status === "running" || evt.status === "waiting"
    );
    // Switch to bg zone the first time a new scheduled job starts this session.
    if (evt.status === "running" && !_bgZoneActivated.has(evt.workflow_id)) {
      _bgZoneActivated.add(evt.workflow_id);
      activateZone("bgruns");
    }
    // When a run completes: always save to history.
    // Only open the output panel if this is the workflow currently on canvas —
    // opening it for a background workflow the user isn't looking at is disruptive.
    if ((evt.status === "waiting" || evt.status === "error") && evt.last_result) {
      try {
        const result = evt.last_result as WorkflowResult;
        if (evt.workflow_id === wfManager.currentId) {
          // User is looking at this workflow — show full output panel
          runManager.showResultFromScheduler(evt.workflow_name, result);
        } else {
          // Different workflow — save history silently.
          // Only toast on failure — successful runs are noise at scale.
          saveRunToHistory(evt.workflow_id, evt.workflow_name, result);
          if (!result.success) {
            toast(`"${evt.workflow_name}" failed — check Background Runs for details`, "error");
          }
        }
      } catch { /* non-fatal */ }
    }
  });

  // Rust scheduler daemon skip events — fired when a scheduled run is skipped
  // because the previous run is still in progress. Show an info toast only when
  // the user is looking at the affected workflow or has the Background Runs panel
  // open — avoids noise for background workflows the user is not watching.
  await listenSchedulerSkip((evt) => {
    const show = evt.workflow_id === wfManager.currentId || getCurrentZone() === "bgruns";
    if (!show) return;
    const job = getBgJobs().find(j => j.id === evt.workflow_id);
    const name = job?.name ?? evt.workflow_id;
    toast(`"${name}" run skipped — previous run still in progress`, "info");
  });

  // Populate bg-jobs section from persistent scheduler state on startup
  if (isTauri()) {
    // Listener is registered above — now request state replay from the daemon.
    // This replaces the old 800 ms startup delay: we signal readiness explicitly
    // instead of hoping the listener registered in time.
    requestSchedulerState().catch(() => {});

    getScheduledJobs().then(rows => {
      hydrateBgJobsFromScheduler(rows);
      renderBgJobs(wfManager, runManager, "all", "", toast);
      // Inform the user if any scheduled workflows are already running
      const activeCount = rows.filter(r => r.status === "active").length;
      if (activeCount > 0) {
        const label = activeCount === 1 ? "1 workflow running in background" : `${activeCount} workflows running in background`;
        setStatus(label);
        toast(label, "info");
      }
    }).catch(() => {});
  }

  // Wire OS close button → unsaved-changes check (Tauri only)
  if (isTauri()) {
    listenCloseRequested(
      () => wfManager.hasUnsaved,
      showConfirm
    ).catch(console.error);
  }

  // Show onboarding on first launch
  initOnboarding(canvas, wfManager);

  // Item 3: Detect Node.js availability (Tauri only) and show palette banner if missing.
  if (isTauri()) {
    checkNodejsAvailable().then(available => {
      if (!available) {
        injectNodejsBanner(canvas);
      }
    }).catch(() => { /* non-fatal */ });
  }
}

// ── Node.js detection banner ──────────────────────────────────────────────────

/**
 * Inject a dismissable banner into the command palette warning that Node.js is
 * not found. The banner appears above the search field whenever the palette is
 * open. Dismissing it removes it for the lifetime of the session.
 *
 * Also surfaces a one-time toast when Code nodes are present in the workflow,
 * so the user sees the warning even before opening the palette.
 */
function injectNodejsBanner(canvas: import("./canvas/Canvas").Canvas): void {
  const palette = document.getElementById("command-palette");
  const inputWrap = document.getElementById("palette-input-wrap");
  if (!palette || !inputWrap) return;

  const BANNER_ID = "nodejs-missing-banner";
  if (document.getElementById(BANNER_ID)) return; // already injected

  const banner = document.createElement("div");
  banner.id = BANNER_ID;
  banner.className = "nodejs-banner";
  banner.setAttribute("role", "alert");

  const msg = document.createElement("span");
  msg.textContent = "Code node requires Node.js — ";

  const link = document.createElement("a");
  link.textContent = "Download";
  link.href = "https://nodejs.org";
  link.target = "_blank";
  link.rel = "noopener noreferrer";

  const dismiss = document.createElement("button");
  dismiss.textContent = "×";
  dismiss.setAttribute("aria-label", "Dismiss");
  dismiss.className = "nodejs-banner-dismiss";
  dismiss.addEventListener("click", () => banner.remove());

  banner.appendChild(msg);
  banner.appendChild(link);
  banner.appendChild(dismiss);

  palette.insertBefore(banner, inputWrap);

  // If the current workflow already has a Code node, show a one-time toast
  // so the warning is visible without opening the palette.
  const hasCodeNode = Array.from(canvas.nodes.values()).some(
    n => n.data.node_type_id === "code"
  );
  if (hasCodeNode) {
    // Defer to ensure toast infrastructure is ready.
    setTimeout(() => {
      const toastEl = document.createElement("div");
      toastEl.className = "toast toast--warning";
      toastEl.textContent = "This workflow uses Code nodes but Node.js was not found. Download it at nodejs.org.";
      document.body.appendChild(toastEl);
      setTimeout(() => toastEl.remove(), 8000);
    }, 1500);
  }
}

// ── Workflow validation ────────────────────────────────────────────────────────

function validateWorkflow(canvas: Canvas): string[] {
  const errors: string[] = [];
  const nodes = canvas.nodes;

  if (nodes.size === 0) {
    errors.push("Canvas is empty — add some nodes first.");
    return errors;
  }

  // Must have at least one trigger node
  const hasTrigger = [...nodes.values()].some(n =>
    ["manual_trigger", "webhook", "schedule"].includes(n.data.node_type_id)
  );
  if (!hasTrigger) {
    errors.push("No trigger node found. Add a Manual Trigger, Webhook, or Schedule.");
  }

  // Required field checks per node type
  const REQUIRED: Record<string, string[]> = {
    http_request: ["url", "method"],
    email_send:   ["to", "subject"],
    shell_exec:   ["command"],
    code:         ["code"],
    ai_prompt:    ["prompt"],
    ai_agent:     ["goal"],
    schedule:     ["mode"],
    database:     ["db_path", "query"],
  };

  for (const node of nodes.values()) {
    const required = REQUIRED[node.data.node_type_id];
    if (!required) continue;
    for (const field of required) {
      const val = node.data.config[field];
      if (!val || String(val).trim() === "") {
        errors.push(`"${node.data.name}" — ${field.replace(/_/g," ")} is required.`);
      }
    }
  }

  return errors;
}

// ── Background jobs sidebar renderer ────────────────────────────────────────────

const _countdownIntervals = new Map<string, ReturnType<typeof setInterval>>();

// Debounce renderBgJobs calls so rapid scheduler events (e.g. running→waiting
// emitted within milliseconds of each other) do not cause the countdown
// intervals to be torn down and recreated in a visible gap.
let _renderBgJobsTimer: ReturnType<typeof setTimeout> | null = null;
function renderBgJobsDebounced(
  wfManager: WorkflowManager,
  runManager: RunManager,
  filterStatus = "all",
  filterQuery  = "",
  toastFn?: (msg: string, type: "success" | "error" | "info") => void
): void {
  if (_renderBgJobsTimer !== null) clearTimeout(_renderBgJobsTimer);
  _renderBgJobsTimer = setTimeout(() => {
    _renderBgJobsTimer = null;
    renderBgJobs(wfManager, runManager, filterStatus, filterQuery, toastFn);
  }, 60);
}

function renderBgJobs(
  wfManager:    WorkflowManager,
  runManager:   RunManager,
  filterStatus  = "all",
  filterQuery   = "",
  toastFn?: (msg: string, type: "success" | "error" | "info") => void
): void {
  const list = document.getElementById("bg-jobs-list");
  if (!list) return;

  let jobs = getBgJobs();

  // Update activity bar badge + running state
  updateActivityBadge("bgruns", jobs.length);
  updateBgRunningState(jobs.some(j => j.status === "running"));

  // Apply filter
  if (filterStatus !== "all") {
    jobs = jobs.filter(j => j.status === filterStatus);
  }
  if (filterQuery) {
    jobs = jobs.filter(j => j.name.toLowerCase().includes(filterQuery));
  }

  // Clear old countdown intervals
  _countdownIntervals.forEach(id => clearInterval(id));
  _countdownIntervals.clear();

  list.innerHTML = "";

  if (!jobs.length) {
    const empty = document.createElement("div");
    empty.className = "bg-jobs-empty";
    empty.innerHTML = filterStatus !== "all" || filterQuery
      ? "<span>No runs match the filter</span>"
      : "<svg width='24' height='24' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-width='1.5' stroke-linecap='round' opacity='0.35'><circle cx='12' cy='12' r='9'/><polygon points='10 8 16 12 10 16 10 8' fill='currentColor' stroke='none'/></svg><span>No scheduled workflows running</span><small>Use <strong>Schedule Run</strong> in the toolbar to run a workflow with a Schedule or Webhook trigger in the background.</small>";
    list.appendChild(empty);
    return;
  }

  for (const job of jobs) {
    const item = document.createElement("div");
    item.className = "bg-job-item";

    const dot = document.createElement("span");
    dot.className = `bg-job-dot bg-job-dot--${job.status}`;

    const nameEl = document.createElement("span");
    nameEl.className = "bg-job-name";
    nameEl.textContent = job.name;
    nameEl.title = job.name;

    // Status / countdown
    const statusEl = document.createElement("span");
    statusEl.className = `bg-job-status bg-job-status--${job.status}`;

    if (job.status === "running") {
      // Show countdown to next run if we have next_run_at.
      // Fix #7: store nextRunAt as a data attribute so the interval callback reads
      // the current value from the DOM rather than a stale closure-captured copy.
      // renderBgJobs replaces the DOM on every scheduler event, so the interval
      // self-terminates when its item is removed; new items have fresh timestamps.
      if (job.nextRunAt) {
        statusEl.dataset.nextRunAt = job.nextRunAt;
        const updateCountdown = () => {
          const target = statusEl.dataset.nextRunAt;
          if (!target) { statusEl.textContent = "running"; return; }
          const secsLeft = Math.max(0, Math.round(
            (new Date(target).getTime() - Date.now()) / 1000
          ));
          if (secsLeft === 0) {
            statusEl.textContent = "running…";
          } else if (secsLeft < 60) {
            statusEl.textContent = `next in ${secsLeft}s`;
          } else {
            const m = Math.floor(secsLeft / 60);
            const s = secsLeft % 60;
            statusEl.textContent = `next in ${m}m ${s}s`;
          }
        };
        updateCountdown();
        const timerId = setInterval(() => {
          if (!document.getElementById("bg-jobs-list")?.contains(item)) {
            clearInterval(timerId);
            return;
          }
          updateCountdown();
        }, 1000);
        _countdownIntervals.set(job.id, timerId);
      } else {
        statusEl.textContent = "running";
      }
    } else if (job.status === "done") {
      const secs = job.finishedAt
        ? Math.round((job.finishedAt - job.startedAt) / 1000) : 0;
      statusEl.textContent = secs < 60 ? `${secs}s` : `${Math.round(secs / 60)}m`;
    } else if (job.status === "stopped") {
      // Fix #8: user-initiated stop — show neutral "stopped", not red "failed"
      statusEl.textContent = "stopped";
    } else {
      statusEl.textContent = "failed";
    }

    // Hover actions
    const actions = document.createElement("span");
    actions.className = "bg-job-hover-actions";

    if (job.status === "running") {
      const stopBtn = document.createElement("button");
      stopBtn.className = "bg-job-action-btn bg-job-action-stop";
      stopBtn.title = "Stop workflow";
      stopBtn.innerHTML = `<svg width="9" height="9" viewBox="0 0 24 24" fill="currentColor"><rect x="3" y="3" width="18" height="18" rx="2"/></svg>`;
      stopBtn.addEventListener("click", async (e) => {
        e.stopPropagation();
        stopBtn.disabled = true;
        try { await stopScheduledWorkflow(job.id); } catch { /* event updates */ }
      });
      actions.appendChild(stopBtn);
    } else {
      const restartBtn = document.createElement("button");
      restartBtn.className = "bg-job-action-btn bg-job-action-restart";
      restartBtn.title = "Restart workflow";
      restartBtn.innerHTML = `<svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><polyline points="23 4 23 10 17 10"/><path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10"/></svg>`;
      restartBtn.addEventListener("click", async (e) => {
        e.stopPropagation();
        restartBtn.disabled = true;
        try {
          await startScheduledWorkflow(job.id);
        } catch (err) {
          restartBtn.disabled = false;
          const parsed = parseSchedulerError(String(err));
          if (parsed.error_kind === "workflow_not_found") {
            toastFn?.(`Cannot restart "${job.name}" — workflow was deleted`, "error");
          } else {
            toastFn?.(`Restart failed: ${(parsed as { message?: string }).message ?? String(err)}`, "error");
          }
        }
      });
      actions.appendChild(restartBtn);

      const dismissBtn = document.createElement("button");
      dismissBtn.className = "bg-job-action-btn bg-job-action-dismiss";
      dismissBtn.title = "Remove from list";
      dismissBtn.innerHTML = `<svg width="9" height="9" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>`;
      dismissBtn.addEventListener("click", (e) => {
        e.stopPropagation();
        removeBgJob(job.id);
        wfManager.refreshWorkflowList();
      });
      actions.appendChild(dismissBtn);
    }

    item.appendChild(dot);
    item.appendChild(nameEl);
    item.appendChild(statusEl);
    item.appendChild(actions);

    // Click row — load workflow onto canvas, show output panel.
    // onNavigate fires synchronously inside handleLoad and calls
    // runManager.setCurrentWorkflow before the await resolves, so
    // openHistoryDrawer can be called directly without a setTimeout.
    item.addEventListener("click", async () => {
      item.style.opacity = "0.5";
      item.style.pointerEvents = "none";
      try {
        await wfManager.handleLoad(job.id);
      } catch {
        toastFn?.("Workflow no longer exists", "error");
        item.classList.add("bg-job-item--stale");
        return;
      } finally {
        item.style.opacity = "";
        item.style.pointerEvents = "";
      }
      runManager.openHistoryDrawer(job.name);
    });

    list.appendChild(item);
  }
}

// ── Note node inline editor ───────────────────────────────────────────────────
// Shows a floating textarea directly over the note on the canvas.

function showNoteEditor(
  node: CanvasNode,
  canvasEl: HTMLCanvasElement,
  onChange: () => void
): void {
  // Remove any existing editor
  document.getElementById("note-inline-editor")?.remove();

  const canvas = (canvasEl as unknown as Record<string, unknown>).__canvas as import("./canvas/Canvas").Canvas;
  if (!canvas) return;

  const r    = canvasEl.getBoundingClientRect();
  const NODE_W = 220;
  const sx   = node.data.position.x * canvas.zoom + canvas.panX + r.left;
  const sy   = node.data.position.y * canvas.zoom + canvas.panY + r.top;
  const sw   = NODE_W * canvas.zoom;
  const sh   = Math.max(60, node.height) * canvas.zoom;

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

  // Color selector
  const NOTE_COLORS = ["default", "yellow", "blue", "green", "red"] as const;
  const colorRow = document.createElement("div");
  colorRow.className = "note-color-row";
  const COLOR_HEX: Record<string, string> = {
    default: "#30363d", yellow: "#f59e0b", blue: "#4d9eff", green: "#34d399", red: "#f87171",
  };
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

  // Esc also closes
  ta.addEventListener("keydown", e => {
    if (e.key === "Escape") { wrap.remove(); document.removeEventListener("mousedown", dismiss, true); }
  });
}

// ── Variable interpolation autocomplete ───────────────────────────────────────
// Fires when the user types {{ in any input/textarea inside the popover.
// Shows a dropdown of available node output paths from the current canvas.

let _interpCanvas: import("./canvas/Canvas").Canvas | null = null;

export function initInterpolationAutocomplete(canvas: import("./canvas/Canvas").Canvas): void {
  _interpCanvas = canvas;

  document.addEventListener("input", (e) => {
    const el = e.target as HTMLElement;
    if (!(el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement)) return;
    // Only fire inside the popover or right panel
    if (!el.closest(".node-popover, #right-panel")) return;
    handleInterpolationInput(el);
  });

  document.addEventListener("keydown", (e) => {
    const dropdown = document.getElementById("interp-dropdown");
    if (!dropdown || dropdown.classList.contains("hidden")) return;
    if (e.key === "Escape") { dropdown.classList.add("hidden"); return; }
    const items = dropdown.querySelectorAll<HTMLElement>(".interp-item");
    const active = dropdown.querySelector<HTMLElement>(".interp-item.active");
    const idx = active ? [...items].indexOf(active) : -1;
    if (e.key === "ArrowDown") {
      e.preventDefault();
      items[Math.min(idx + 1, items.length - 1)]?.classList.add("active");
      active?.classList.remove("active");
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      items[Math.max(idx - 1, 0)]?.classList.add("active");
      active?.classList.remove("active");
    } else if (e.key === "Enter" || e.key === "Tab") {
      e.preventDefault();
      (active ?? items[0])?.click();
    }
  });

  // Close dropdown on outside click
  document.addEventListener("mousedown", (e) => {
    const dd = document.getElementById("interp-dropdown");
    if (dd && !dd.contains(e.target as Node)) dd.classList.add("hidden");
  }, true);
}

function handleInterpolationInput(el: HTMLInputElement | HTMLTextAreaElement): void {
  const val = el.value;
  const cursor = el.selectionStart ?? val.length;
  const before = val.slice(0, cursor);
  const triggerIdx = before.lastIndexOf("{{");
  if (triggerIdx === -1) { document.getElementById("interp-dropdown")?.classList.add("hidden"); return; }

  const query = before.slice(triggerIdx + 2).toLowerCase();
  const paths = buildInterpolationPaths();
  const matches = paths.filter(p => p.toLowerCase().includes(query));

  showInterpolationDropdown(el, matches, (chosen) => {
    const after = val.slice(cursor);
    const newVal = val.slice(0, triggerIdx) + "{{" + chosen + "}}" + after;
    el.value = newVal;
    el.dispatchEvent(new Event("input", { bubbles: true }));
    const newCursor = triggerIdx + 2 + chosen.length + 2;
    el.setSelectionRange(newCursor, newCursor);
    document.getElementById("interp-dropdown")?.classList.add("hidden");
  });
}

function buildInterpolationPaths(): string[] {
  if (!_interpCanvas) return [];
  const paths: string[] = [];
  for (const node of _interpCanvas.nodes.values()) {
    const name = node.data.name;
    paths.push(`${name}`);
    paths.push(`${name}.output`);
    paths.push(`${name}.result`);
    paths.push(`${name}.content`);
    paths.push(`${name}.value`);
  }
  return paths;
}

function showInterpolationDropdown(
  anchor: HTMLElement,
  paths: string[],
  onSelect: (path: string) => void
): void {
  let dd = document.getElementById("interp-dropdown");
  if (!dd) {
    dd = document.createElement("div");
    dd.id = "interp-dropdown";
    document.body.appendChild(dd);
  }

  if (!paths.length) { dd.classList.add("hidden"); return; }

  const rect = anchor.getBoundingClientRect();
  dd.style.cssText = `position:fixed;left:${rect.left}px;top:${rect.bottom + 2}px;z-index:600;max-height:180px;overflow-y:auto;`;
  dd.className = "interp-dropdown";
  dd.innerHTML = "";

  // Limit to first 12 matches
  for (const p of paths.slice(0, 12)) {
    const item = document.createElement("div");
    item.className = "interp-item";
    // Strip the alias comment for insertion; show it as hint
    const isAlias = p.includes(" (");
    const insertValue = isAlias ? p.slice(0, p.indexOf(" (")) : p;
    const hint = isAlias ? p.slice(p.indexOf(" (")) : "";
    item.innerHTML = `<span class="interp-path">${escHtml(insertValue)}</span>${hint ? `<span class="interp-hint">${escHtml(hint)}</span>` : ""}`;
    item.addEventListener("mousedown", (e) => { e.preventDefault(); onSelect(insertValue); });
    dd.appendChild(item);
  }
  dd.classList.remove("hidden");
}

// ── Wire-drop node picker ─────────────────────────────────────────────────────
// Shows a compact inline picker when a wire is dropped on empty canvas space.
// The picked node is placed at the drop point and auto-connected.

function openWireDropPicker(
  _allNodes: NodeDescriptor[],
  canvas: Canvas,
  _canvasEl: HTMLCanvasElement,
  onStatus: (m: string) => void,
): void {
  // Reuse the command palette but intercept the selection to call completeWireDrop
  const overlay = document.getElementById("command-palette-overlay")!;
  const inp = document.getElementById("palette-search") as HTMLInputElement;

  // Patch the hint to explain wire-drop context
  const hint = document.getElementById("palette-hint");
  if (hint) hint.textContent = "Pick a node to connect · Esc cancel";

  overlay.classList.remove("hidden");
  inp.value = ""; 
  // Trigger a render of results
  inp.dispatchEvent(new Event("input"));
  requestAnimationFrame(() => inp.focus());

  // Listen for a single selection — after pick, complete the wire
  const onPick = (e: Event) => {
    if (!(e instanceof CustomEvent)) return;
    const detail = e.detail as NodeDescriptor | null;
    if (!detail) return;
    overlay.removeEventListener("wire-drop-pick", onPick);
    canvas.completeWireDrop(detail);
    onStatus(`Connected → ${detail.display_name}`);
    if (hint) hint.textContent = "↑↓ navigate · Enter place · Esc close";
  };
  overlay.addEventListener("wire-drop-pick", onPick);

  // If user closes palette without picking, cancel the wire drop
  const onClose = () => {
    overlay.removeEventListener("wire-drop-pick", onPick);
    canvas._pendingWireDrop = null;
    if (hint) hint.textContent = "↑↓ navigate · Enter place · Esc close";
  };
  overlay.addEventListener("palette-closed", onClose, { once: true });
}

// ── Version history panel ─────────────────────────────────────────────────────

async function showVersionPanel(
  wfManager: WorkflowManager,
  toast: (m: string, t: "success" | "error" | "info") => void,
): Promise<void> {
  // Remove any existing panel
  document.getElementById("version-panel-overlay")?.remove();

  const overlay = document.createElement("div");
  overlay.id = "version-panel-overlay";
  overlay.className = "version-overlay";

  const panel = document.createElement("div");
  panel.className = "version-panel";

  const hdr = document.createElement("div");
  hdr.className = "version-panel-header";
  hdr.innerHTML = `<span class="version-panel-title">Version History</span>`;
  const closeBtn = document.createElement("button");
  closeBtn.className = "popover-close";
  closeBtn.innerHTML = `<svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>`;
  closeBtn.addEventListener("click", () => overlay.remove());
  hdr.appendChild(closeBtn);
  panel.appendChild(hdr);

  const body = document.createElement("div");
  body.className = "version-panel-body";

  // Show loading state while fetching
  body.innerHTML = `<div class="version-empty">Loading…</div>`;
  panel.appendChild(body);
  overlay.appendChild(panel);
  document.body.appendChild(overlay);
  overlay.addEventListener("click", e => { if (e.target === overlay) overlay.remove(); });

  const versions = await wfManager.getVersions();

  body.innerHTML = "";
  if (!versions.length) {
    body.innerHTML = `<div class="version-empty">No saved versions yet.<br>Each time you save, a snapshot is created here.</div>`;
  } else {
    for (const v of versions) {
      const item = document.createElement("div");
      item.className = "version-item";
      const dt = new Date(v.created_at);
      const label = dt.toLocaleDateString([], { month: "short", day: "numeric" })
        + " " + dt.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
      const displayMsg = v.message ?? "Saved";
      item.innerHTML = `
        <div class="version-item-meta">
          <span class="version-item-name">${escHtml(displayMsg)}</span>
          <span class="version-item-date">${label}</span>
        </div>`;

      const actions = document.createElement("div");
      actions.className = "version-item-actions";

      const restoreBtn = document.createElement("button");
      restoreBtn.className = "version-restore-btn";
      restoreBtn.textContent = "Restore";
      restoreBtn.addEventListener("click", async () => {
        overlay.remove();
        const ok = await showConfirm(`Restore to version from ${label}? Current unsaved changes will be lost.`);
        if (!ok) return;
        try {
          const result = await wfManager.restoreVersion(v.id);
          if (result) {
            toast(`Restored to ${label}`, "success");
          } else {
            toast("Restore failed — version not found", "error");
          }
        } catch (e) {
          toast(`Restore failed: ${e}`, "error");
        }
      });

      const deleteBtn = document.createElement("button");
      deleteBtn.className = "version-delete-btn";
      deleteBtn.title = "Delete this version";
      deleteBtn.innerHTML = `<svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>`;
      deleteBtn.addEventListener("click", async () => {
        try {
          await wfManager.deleteVersion(v.id);
          item.remove();
          if (!body.querySelector(".version-item")) {
            body.innerHTML = `<div class="version-empty">No saved versions yet.<br>Each time you save, a snapshot is created here.</div>`;
          }
        } catch (e) {
          toast(`Delete failed: ${e}`, "error");
        }
      });

      actions.appendChild(restoreBtn);
      actions.appendChild(deleteBtn);
      item.appendChild(actions);
      body.appendChild(item);
    }
  }
}

function bindDrawerResize() {
  const handle = document.getElementById("drawer-resize-handle")!;
  const drawer = document.getElementById("output-drawer")!;
  let startY = 0, startH = 0, dragging = false;
  handle.addEventListener("mousedown", e => {
    dragging = true; startY = e.clientY; startH = drawer.offsetHeight;
    handle.classList.add("dragging"); e.preventDefault();
  });
  window.addEventListener("mousemove", e => {
    if (!dragging) return;
    const newH = Math.max(120, Math.min(window.innerHeight * 0.7, startH + (startY - e.clientY)));
    drawer.style.height = `${newH}px`;
    document.documentElement.style.setProperty("--drawer-h", `${newH}px`);
  });
  window.addEventListener("mouseup", () => { if (dragging) { dragging = false; handle.classList.remove("dragging"); } });
}

function bindPanelResize() {
  const handle = document.getElementById("panel-resize-handle")!;
  let startX = 0, startW = 0, dragging = false;
  handle.addEventListener("mousedown", e => {
    dragging = true; startX = e.clientX;
    startW = document.getElementById("right-panel")!.offsetWidth;
    handle.classList.add("dragging"); e.preventDefault();
  });
  window.addEventListener("mousemove", e => {
    if (!dragging) return;
    const newW = Math.max(240, Math.min(600, startW + (startX - e.clientX)));
    document.documentElement.style.setProperty("--panel-w", `${newW}px`);
  });
  window.addEventListener("mouseup", () => { if (dragging) { dragging = false; handle.classList.remove("dragging"); } });
}

init().catch(console.error);

// ── Onboarding ────────────────────────────────────────────────────────────────

function initOnboarding(canvas: Canvas, wfManager: WorkflowManager): void {
  const modal   = document.getElementById("onboarding-modal")!;
  const dismiss = document.getElementById("onboarding-dismiss")!;
  const example = document.getElementById("onboarding-load-example")!;

  if (localStorage.getItem("flowo_onboarded")) return;

  // Show after a short delay so the app fully renders first
  setTimeout(() => modal.classList.remove("hidden"), 400);

  const close = () => {
    modal.classList.add("hidden");
    localStorage.setItem("flowo_onboarded", "1");
  };

  dismiss.addEventListener("click", close);

  example.addEventListener("click", async () => {
    close();
    // Load a simple "Fetch + Show" example workflow.
    // schema_version is required by the Rust Workflow deserializer.
    // All node fields must be present so CanvasSerializer.deserialize()
    // constructs valid CanvasNodeData objects without falling back to
    // undefined-typed defaults.
    const now = new Date().toISOString();
    const exampleWorkflow = {
      schema_version: "1.0",
      id: "example_getting_started",
      name: "Getting Started — Fetch & Show",
      description: "",
      nodes: [
        {
          id: "n1", node_type_id: "manual_trigger", node_type: "action",
          name: "Start", position: { x: 120, y: 200 }, config: {},
          credentials: {}, retry: { max_attempts: 1, backoff_ms: 500 },
          fallback_node: null,
          input_schema:  { type: "object", properties: { mock_payload: { type: "string", description: "Optional JSON payload to inject when running manually" } } },
          output_schema: { type: "object" },
          ports: { inputs: [], outputs: [{ id: "output", label: "Start", position: "right" }] },
        },
        {
          id: "n2", node_type_id: "http_request", node_type: "action",
          name: "Fetch Data", position: { x: 380, y: 200 },
          config: { url: "https://httpbin.org/get", method: "GET" },
          credentials: {}, retry: { max_attempts: 3, backoff_ms: 500 },
          fallback_node: null,
          input_schema:  { type: "object", properties: { url: { type: "string" }, method: { type: "string", enum: ["GET","POST","PUT","PATCH","DELETE"] } } },
          output_schema: { type: "object" },
          ports: { inputs: [{ id: "input", label: "In", position: "left" }], outputs: [{ id: "output", label: "Success", position: "right" }, { id: "on_error", label: "Error", position: "right" }] },
        },
        {
          id: "n3", node_type_id: "output", node_type: "utility",
          name: "Show Result", position: { x: 640, y: 200 },
          config: { label: "HTTP Result" },
          credentials: {}, retry: { max_attempts: 1, backoff_ms: 500 },
          fallback_node: null,
          input_schema:  { type: "object", properties: { label: { type: "string" }, source_node: { type: "string" }, field: { type: "string" } } },
          output_schema: { type: "object" },
          ports: { inputs: [{ id: "input", label: "In", position: "left" }], outputs: [{ id: "output", label: "Out", position: "right" }] },
        },
      ],
      edges: [
        { id: "e1", from_node: "n1", from_port: "output", to_node: "n2", to_port: "input", condition: null, on_success: null, on_failure: null },
        { id: "e2", from_node: "n2", from_port: "output", to_node: "n3", to_port: "input", condition: null, on_success: null, on_failure: null },
      ],
      metadata: { author: "user", created_at: now, updated_at: now, version: "1.0.0", tags: [] },
    };
    await wfManager.loadFromObject(exampleWorkflow);
  });
}

// Call after init — wfManager and canvas are set in module scope via init()
// We expose them for onboarding via a deferred call from init().

// ── BUG-02: Live bg-run button state ─────────────────────────────────────────
// Called whenever the bg job store changes or scheduler events arrive.
// Disables / relabels the Schedule Run button when the CURRENT workflow is
// already running in the background — prevents duplicate runs and gives the
// user clear feedback.

function updateBgRunButton(currentWorkflowId: string): void {
  const btn = document.getElementById("btn-bg-run") as HTMLButtonElement | null;
  if (!btn) return;
  const jobs = getBgJobs();
  const isRunning = jobs.some(j => j.id === currentWorkflowId && j.status === "running");
  btn.disabled = isRunning;
  btn.textContent = isRunning ? "Scheduled…" : "Schedule Run";
  btn.title = isRunning
    ? "This workflow is already running in the background. Stop it from the Background Runs panel to restart."
    : "Run silently in the background — result stored in history";
}

// ── Dropdown utilities ────────────────────────────────────────────────────────

function closeAllDropdowns(): void {
  document.querySelectorAll('.toolbar-dropdown-menu').forEach(m => m.classList.remove('open'));
}
