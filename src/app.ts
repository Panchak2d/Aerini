import { Canvas } from "./canvas/Canvas";
import { TRIGGER_NODE_IDS, NODE_IDS } from "./node-ids";
import { deserialize, registerNodeDescriptors } from "./canvas/CanvasSerializer";
import type { ChatSettings } from "./canvas/CanvasSerializer";
import type { NodeDescriptor } from "./ipc/workflow";
import { getNodeTypes, checkBundledNode } from "./ipc/workflow";
import { WorkflowManager } from "./workflow-manager";
import { RunManager, onBgJobsChanged } from "./run-manager";
import { initStatusBarFields } from "./statusbar-fields";
import { initPerformancePanel } from "./panels/PerformancePanel";
import {
  buildSidebarPalette, bindSidebarSearch, filterByCategory,
  initCommandPalette, openPalette,
} from "./palette-manager";
import { initModals } from "./modal-manager";
import { bindDropImport, bindFileInput } from "./drag-drop";
import { bindPluginSettings } from "./plugin-settings";
import { showPopover, closePopover } from "./popover";
import { listenCloseRequested } from "./ipc/events";
import { getVersion } from "@tauri-apps/api/app";
import { initSidebarSections, bindSectionSearchToggles, bindWorkflowSectionControls, bindBgRunsFilter, activateZone, getCurrentZone } from "./sidebar-sections";
import { setMonitorWfManager } from "./panels/MonitorPanel";
import { isTauri } from "./utils";
import { invoke } from "@tauri-apps/api/core";
import { preloadAllIcons } from "./icon-cache";

import { showConfirm } from "./confirm";
import { initInterpolationAutocomplete } from "./interpolation";
import { openWireDropPicker, openInputWireDropPicker } from "./wire-drop";
import { initOnboarding } from "./onboarding";
import { updateAlwaysOnBtn } from "./always-on";
import { bindSchedulerEvents } from "./scheduler-events";
import { bindToolbar } from "./toolbar";
import { initTooltips } from "./tooltip-manager";
import { loadBgPanel, getBgPanelIfLoaded } from "./bg-panel-loader";
import type { ChatPanel as ChatPanelType } from "./panels/ChatPanel";
import { initTheme } from "./theme";

// applied as the very first thing this module does, ahead of every
// function declaration and ahead of init()'s own call at the bottom of this
// file — see theme.ts's own doc comment for why this (not an inline head
// script) is the earliest point available under this app's CSP.
initTheme();

// ── App bootstrap ─────────────────────────────────────────────────────────────

async function init() {
  preloadAllIcons(); // fire-and-forget: canvas falls back to accent letter until bitmaps ready
  initTooltips();

  // Restore the active sidebar zone synchronously, before any await below.
  // index.html ships #zone-nodes as the hardcoded default `active` zone; this
  // call corrects it to the last-used zone (e.g. "bgruns" if a background job
  // was running last session). It must run before the getNodeTypes() IPC
  // round-trip — otherwise the static default stays visibly active until
  // that call resolves, producing a flash of the wrong zone on startup. That
  // window is negligible under `vite dev`'s HMR server but long enough to be
  // visible in a production/Tauri cold start.
  initSidebarSections();

  // Lazy BgJobsPanel — deferred after first paint
  function _deferBgPanel(fn: () => void): void {
    if (typeof requestIdleCallback === "function") requestIdleCallback(fn, { timeout: 200 });
    else setTimeout(fn, 0);
  }
  const allNodes = await getNodeTypes().catch((): NodeDescriptor[] => []);
  registerNodeDescriptors(allNodes);

  const canvasEl   = document.getElementById("canvas") as HTMLCanvasElement;
  const canvas     = new Canvas(canvasEl);
  // Expose canvas on the DOM element so popover positioning can access zoom/pan
  (canvasEl as unknown as Record<string, unknown>).__canvas = canvas;

  // Window is created hidden (tauri.conf.json) to avoid a flash of the desktop
  // showing through before content paints. Reveal it only once the browser has
  // actually drawn the first real frame (two rAFs = layout + paint committed).
  if (isTauri()) {
    requestAnimationFrame(() => requestAnimationFrame(() => {
      invoke("show_main_window").catch(console.error);
    }));
  }

  function toast(msg: string, type: "success" | "error" | "info" | "warning" = "info") {
    const colors = { success: "var(--green)", error: "var(--red)", info: "var(--blue)", warning: "var(--amber)" };
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
    if (el) { el.textContent = n; el.title = n; el.setAttribute("data-tooltip", n); }
  }
  function markUnsaved(on: boolean) {
    document.getElementById("unsaved-dot")?.classList.toggle("visible", on);
  }
  function updateStatusHint() {
    const el = document.getElementById("status-hint"); if (!el) return;
    const hint = canvas.nodes.size === 0
      ? "Click a node to place it · Space to search · Ctrl+S to save"
      : canvas.selectedNode
      ? "Del to delete · Ctrl+D to duplicate · Double-click to configure"
      : "Double-click node to configure · Ctrl+S to save · Ctrl+Enter to run";
    el.textContent = hint; el.title = hint;
  }

  // Persist viewport per workflow so zoom/pan survive workflow switches
  canvas.onViewportChange = () => {
    if (!wfManager.currentId) return;
    localStorage.setItem(
      `aerini_viewport_${wfManager.currentId}`,
      JSON.stringify({ panX: canvas.panX, panY: canvas.panY, zoom: canvas.zoom })
    );
  };

  if (!isTauri()) {
    setTimeout(() => {
      const ann = document.getElementById("a11y-announcer");
      if (ann) ann.textContent = "Browser mode. Execution requires the Tauri desktop app. Workflow builder works fully here.";
    }, 300);
  }

  // WorkflowManager — pass showConfirm so it uses the modal, not window.confirm
  const wfManager = new WorkflowManager(canvas, {
    onUnsaved: markUnsaved,
    onTitle:   setTitle,
    onStatus:  setStatus,
    onToast:   toast,
    confirm:     showConfirm,
    onPanelClose: () => { closePopover(); },
  });
  setMonitorWfManager(wfManager);

  // After loading or creating a workflow, switch to Nodes zone so the
  // user can immediately start placing nodes without an extra click.
  // Guard: only switch when the user is already in the Workflows zone —
  // navigating from Background Runs must not clobber the user's zone choice.
  wfManager.onNavigate = () => {
    if (getCurrentZone() === "workflows") activateZone("nodes");
    updateAlwaysOnBtn(canvas, wfManager);
    getBgPanelIfLoaded()?.updateBgRunButton(wfManager.currentId);
    refreshRunBtn();
    chatPanel.onWorkflowSwitched();
    statusBarFields.refreshMem();
    perfPanel.refresh();
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

  // Lazy ChatPanel proxy — defers loading marked + DOMPurify until first use.
  // btn-chat is `hidden` by default in HTML; refreshButtonVisibility() and
  // onWorkflowSwitched() below run the same Webhook+Output check the real
  // ChatPanel would, so the button still un-hides pre-load — only opening
  // the panel itself (toggle()) pulls in the heavy module.
  let _chatPanelPromise: Promise<ChatPanelType> | null = null;
  let _chatPanelInst:    ChatPanelType | null = null;

  function _loadChat() {
    if (!_chatPanelPromise) {
      _chatPanelPromise = import("./panels/ChatPanel").then(({ ChatPanel: CP }) => {
        _chatPanelInst = new CP(canvas, wfManager, toast, runManager);
        _chatPanelInst.refreshButtonVisibility();
        return _chatPanelInst;
      });
    }
    return _chatPanelPromise;
  }

  function _hasWebhookAndOutput(): boolean {
    let hasWebhook = false, hasOutput = false;
    for (const n of canvas.nodes.values()) {
      if (n.data.node_type_id === NODE_IDS.WEBHOOK) hasWebhook = true;
      if (n.data.node_type_id === NODE_IDS.OUTPUT)   hasOutput  = true;
      if (hasWebhook && hasOutput) return true;
    }
    return false;
  }

  const chatPanel = {
    toggle() { _loadChat().then(p => p.toggle()); },
    refreshButtonVisibility() {
      if (_chatPanelInst) { _chatPanelInst.refreshButtonVisibility(); return; }
      document.getElementById("btn-chat")?.classList.toggle("hidden", !_hasWebhookAndOutput());
    },
    onWorkflowSwitched() {
      if (_chatPanelInst) { _chatPanelInst.onWorkflowSwitched(); return; }
      document.getElementById("btn-chat")?.classList.toggle("hidden", !_hasWebhookAndOutput());
    },
    isOpen() {
      return _chatPanelInst ? _chatPanelInst.isOpen() : false;
    },
    startForChat() { _loadChat().then(p => p.startForChat()); },
    applyToggles(settings: ChatSettings) {
      if (_chatPanelInst) _chatPanelInst.applyToggles(settings);
    },
  };

  // Update status hint when run starts/ends
  runManager.addRunStateListener((running) => {
    const el = document.getElementById("status-hint"); if (!el) return;
    if (running) { const hint = "Workflow running\u2026 Ctrl+. to stop"; el.textContent = hint; el.title = hint; }
    else updateStatusHint();
  });

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
        import("./panels/NoteEditor").then(m => m.showNoteEditor(n, canvasEl, () => {
          wfManager.markUnsaved(true);
          wfManager.scheduleAutoSave();
        }));
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
  canvas.onRunNode        = (nodeId) => runManager.handleRunSingleNode(nodeId, wfManager.currentId, wfManager.currentName);

  const perfPanel = initPerformancePanel(wfManager);
  const statusBarFields = initStatusBarFields(canvas, wfManager, perfPanel.refresh);

  // Wire-drop: connector released on empty space → open node picker at drop point
  canvas.onWireDropRequest = (_fromNode, _fromPort, _wx, _wy) => {
    openWireDropPicker(canvas, setStatus);
  };

  canvas.onInputWireDropRequest = (_toNode, _toPort, _wx, _wy) => {
    openInputWireDropPicker(canvas, setStatus);
  };

  canvas.onWarn = (msg) => toast(msg, "info");

  document.getElementById("canvas")!.addEventListener("mousedown", () => {
    (document.getElementById("node-search") as HTMLInputElement)?.blur();
  }, { capture: true });

  buildSidebarPalette(allNodes, canvas, setStatus);
  bindSidebarSearch();
  document.querySelectorAll<HTMLElement>(".cat-chip").forEach(chip => {
    chip.addEventListener("click", () => filterByCategory(chip.dataset.cat ?? "all"));
  });
  initCommandPalette(canvas, setStatus);
  initModals((obj) => {
    try {
      const { id, name, nodes, connectors, parallelExecution, maxConcurrentNodes, unlimitedDuration, chatSettings, tags, collectionId, maxDurationSecs, importWarnings } = deserialize(JSON.stringify(obj));
      canvas.nodes = nodes; canvas.connectors = connectors;
      canvas.clearSelection(); canvas.fitToScreen();
      wfManager.parallelExecution  = parallelExecution;
      wfManager.maxConcurrentNodes = maxConcurrentNodes;
      wfManager.unlimitedDuration  = unlimitedDuration;
      wfManager.maxDurationSecs    = maxDurationSecs;
      wfManager.chatSettings       = chatSettings;
      wfManager.currentTags        = tags;
      wfManager.currentCollectionId = collectionId;
      wfManager.currentId = id; wfManager.currentName = name;
      wfManager.markUnsaved(false); setTitle(name);
      document.getElementById("output-drawer")!.classList.add("hidden");
      setStatus(`Imported "${name}"`);
      wfManager.refreshWorkflowList();
      // One toast either way — this app has no toast queue/stacking, so a
      // second call right after would just render on top of the first at
      // the same fixed position (see .toast in base.css).
      if (importWarnings.length) toast(`Imported "${name}" — ${importWarnings.join(" ")}`, "warning");
      else toast(`Imported "${name}"`, "success");
    } catch (e) { toast(`Import failed: ${e}`, "error"); }
  });
  bindDropImport(toast);
  bindFileInput(toast);

  // A plugin install/remove/reload swaps the backend node registry. Re-fetch
  // the descriptor list and register it — the command palette and modals
  // read the registry live on each use, so only the sidebar palette (which
  // takes its list as an explicit argument, not a subscription) needs its
  // own rebuild call here.
  async function refreshNodeDescriptors(): Promise<void> {
    let fresh: NodeDescriptor[];
    try {
      fresh = await getNodeTypes();
    } catch {
      // Registry swap already succeeded server-side; a transient failure to
      // re-fetch it shouldn't blank every panel back to zero nodes. Leave
      // the previous (still-valid, just not-quite-current) list in place.
      return;
    }
    registerNodeDescriptors(fresh);
    buildSidebarPalette(fresh, canvas, setStatus);
  }

  bindPluginSettings(toast, refreshNodeDescriptors);

  const { refreshRunBtn } = bindToolbar(
    canvas, wfManager, runManager, chatPanel,
    toast, updateStatusHint,
  );

  await wfManager.refreshWorkflowList();

  bindSectionSearchToggles();
  bindWorkflowSectionControls((sortMode) => wfManager.setSortMode(sortMode));
  bindBgRunsFilter((status, query) =>
    loadBgPanel().then(m => m.renderBgJobs(wfManager, runManager, status, query, toast))
  );

  // Defer BgJobsPanel listener registration — after first paint
  _deferBgPanel(() => {
    loadBgPanel().then(m => {
      m.renderBgJobs(wfManager, runManager, "all", "", toast);
      onBgJobsChanged(() => {
        m.renderBgJobsDebounced(wfManager, runManager, "all", "", toast);
        getBgPanelIfLoaded()?.updateBgRunButton(wfManager.currentId);
        refreshRunBtn();
      });
    });
  });

  await bindSchedulerEvents(canvas, wfManager, runManager, toast, setStatus, refreshRunBtn, statusBarFields.refreshMem, perfPanel.refresh);

  if (isTauri()) {
    listenCloseRequested(() => wfManager.hasUnsaved, showConfirm).catch(console.error);
  }

  initOnboarding(canvas, wfManager);

  if (isTauri()) {
    checkBundledNode().catch((msg: string) => toast(msg, "warning"));
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

init().catch((e) => {
  console.error(e);
  if (isTauri()) invoke("show_main_window").catch(console.error);
});
