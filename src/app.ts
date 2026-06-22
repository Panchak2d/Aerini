import { Canvas } from "./canvas/Canvas";
import { TRIGGER_NODE_IDS, NODE_IDS } from "./node-ids";
import { closePanel } from "./panels/NodeConfigPanel";
import { CredentialPanel } from "./panels/CredentialPanel";
import { deserialize, registerNodeDescriptors } from "./canvas/CanvasSerializer";
import type { NodeDescriptor } from "./ipc/workflow";
import { getNodeTypes, checkNodejsAvailable } from "./ipc/workflow";
import { WorkflowManager } from "./workflow-manager";
import { RunManager, onBgJobsChanged } from "./run-manager";
import {
  buildSidebarPalette, bindSidebarSearch, filterByCategory,
  initCommandPalette, openPalette,
} from "./palette-manager";
import { initModals } from "./modal-manager";
import { bindDropImport, bindFileInput } from "./drag-drop";
import { bindPluginSettings } from "./plugin-settings";
import { showPopover, closePopover, setDescriptorRegistry } from "./popover-config";
import { listenCloseRequested } from "./ipc/events";
import { getVersion } from "@tauri-apps/api/app";
import { initSidebarSections, bindSectionSearchToggles, bindWorkflowSectionControls, bindBgRunsFilter, activateZone, getCurrentZone } from "./sidebar-sections";
import { isTauri } from "./utils";
import { preloadAllIcons } from "./icon-cache";

import { showConfirm } from "./confirm";
import { injectNodejsBanner } from "./banners";
import { showNoteEditor } from "./panels/NoteEditor";
import { initInterpolationAutocomplete } from "./interpolation";
import { openWireDropPicker, openInputWireDropPicker } from "./wire-drop";
import { initOnboarding } from "./onboarding";
import { renderBgJobs, renderBgJobsDebounced, updateBgRunButton } from "./panels/BgJobsPanel";
import { updateAlwaysOnBtn } from "./always-on";
import { bindSchedulerEvents } from "./scheduler-events";
import { bindToolbar } from "./toolbar";
import { ChatPanel } from "./panels/ChatPanel";

// ── App bootstrap ─────────────────────────────────────────────────────────────

async function init() {
  const [allNodes] = await Promise.all([
    getNodeTypes().catch((): NodeDescriptor[] => []),
    preloadAllIcons(),
  ]);
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

  // N-1: show zoom % briefly on scroll, then revert to normal hint
  let _zoomHintTimer: ReturnType<typeof setTimeout> | null = null;
  canvas.onZoomChange = (zoom) => {
    const el = document.getElementById("status-hint"); if (!el) return;
    el.textContent = `${Math.round(zoom * 100)}%`;
    if (_zoomHintTimer) clearTimeout(_zoomHintTimer);
    _zoomHintTimer = setTimeout(() => updateStatusHint(), 1500);
  };

  // N-10: persist viewport per workflow so zoom/pan survive workflow switches
  canvas.onViewportChange = () => {
    if (!wfManager.currentId) return;
    localStorage.setItem(
      `aerini_viewport_${wfManager.currentId}`,
      JSON.stringify({ panX: canvas.panX, panY: canvas.panY, zoom: canvas.zoom })
    );
  };

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
    updateAlwaysOnBtn(canvas, wfManager);
    updateBgRunButton(wfManager.currentId);
    refreshRunBtn();
    chatPanel.onWorkflowSwitched();
    runManager.setCurrentWorkflow(
      wfManager.currentId,
      wfManager.currentName,
      wfManager.parallelExecution,
      wfManager.maxConcurrentNodes,
    );
  };

  canvas.onCanvasChanged = () => {
    wfManager.markUnsaved(true);
    wfManager.scheduleAutoSave();
    updateStatusHint();
    chatPanel.refreshButtonVisibility();
    // Only sync check — no IPC call on every canvas change
    const btn = document.getElementById("btn-always-on");
    if (btn) {
      const hasSchedulableTrigger = [...canvas.nodes.values()].some(n =>
        TRIGGER_NODE_IDS.has(n.data.node_type_id as string)
      );
      btn.classList.toggle("hidden", !hasSchedulableTrigger);
    }
  };

  const runManager = new RunManager(canvas, setStatus, toast);
  const chatPanel  = new ChatPanel(canvas, wfManager, toast);

  // N-8: update status hint when run starts/ends
  runManager.onRunStateChange = (running) => {
    const el = document.getElementById("status-hint"); if (!el) return;
    if (running) el.textContent = "Workflow running\u2026 Ctrl+. to stop";
    else updateStatusHint();
  };

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
      if (n.data.node_type_id === NODE_IDS.NOTE) {
        showNoteEditor(n, canvasEl, () => {
          wfManager.markUnsaved(true);
          wfManager.scheduleAutoSave();
        });
        return;
      }
      showPopover(n, canvasEl, () => {
        wfManager.markUnsaved(true);
        wfManager.scheduleAutoSave();
      }, canvas);
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

  canvas.onInputWireDropRequest = (_toNode, _toPort, _wx, _wy) => {
    openInputWireDropPicker(allNodes, canvas, canvasEl, setStatus);
  };

  document.getElementById("canvas")!.addEventListener("mousedown", () => {
    (document.getElementById("node-search") as HTMLInputElement)?.blur();
  }, { capture: true });

  buildSidebarPalette(allNodes, canvas, setStatus);
  bindSidebarSearch();
  document.querySelectorAll<HTMLElement>(".cat-chip").forEach(chip => {
    chip.addEventListener("click", () => filterByCategory(chip.dataset.cat ?? "all"));
  });
  initCommandPalette(allNodes, canvas, setStatus);
  initModals(allNodes, (obj) => {
    try {
      const { id, name, nodes, connectors, parallelExecution, maxConcurrentNodes, chatSettings } = deserialize(JSON.stringify(obj));
      canvas.nodes = nodes; canvas.connectors = connectors;
      canvas.clearSelection(); canvas.fitToScreen();
      wfManager.parallelExecution  = parallelExecution;
      wfManager.maxConcurrentNodes = maxConcurrentNodes;
      wfManager.chatSettings       = chatSettings;
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
  bindPluginSettings(toast);

  const { refreshRunBtn } = bindToolbar(
    canvas, wfManager, runManager, credPanel, chatPanel,
    toast, updateStatusHint,
  );

  await wfManager.refreshWorkflowList();

  initSidebarSections();
  bindSectionSearchToggles();
  bindWorkflowSectionControls((sortMode) => wfManager.setSortMode(sortMode));
  bindBgRunsFilter((status, query) => renderBgJobs(wfManager, runManager, status, query, toast));

  onBgJobsChanged(() => {
    renderBgJobsDebounced(wfManager, runManager, "all", "", toast);
    updateBgRunButton(wfManager.currentId);
    refreshRunBtn();
  });

  await bindSchedulerEvents(canvas, wfManager, runManager, toast, setStatus, refreshRunBtn);

  if (isTauri()) {
    listenCloseRequested(() => wfManager.hasUnsaved, showConfirm).catch(console.error);
  }

  initOnboarding(canvas, wfManager);

  if (isTauri()) {
    checkNodejsAvailable().then(available => {
      if (!available) injectNodejsBanner(canvas);
    }).catch(() => {});
  }

  if (isTauri()) {
    getVersion().then(v => {
      const verEl = document.getElementById("app-version");
      if (verEl) verEl.textContent = v;
      const onboardEl = document.querySelector<HTMLElement>(".onboarding-version");
      if (onboardEl) onboardEl.textContent = `v${v}`;
    }).catch(() => {});
  }
}

init().catch(console.error);
