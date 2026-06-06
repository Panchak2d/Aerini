import type { Canvas } from "./canvas/Canvas";
import type { WorkflowManager } from "./workflow-manager";
import type { CredentialPanel } from "./panels/CredentialPanel";
import { RunManager, getBgJobs } from "./run-manager";
import { stopScheduledWorkflow } from "./ipc/workflow";
import { isTauri } from "./utils";
import { getAutostart, setAutostart } from "./ipc/autostart";
import { showVersionPanel } from "./panels/VersionPanel";
import { showExportServerPanel } from "./export-server-panel";
import { switchTab, openPanel, closePanel } from "./panels/NodeConfigPanel";
import { validateWorkflow, checkDangerousNodes } from "./validation";
import { showConfirm } from "./confirm";
import { updateBgRunButton, renderBgJobs } from "./panels/BgJobsPanel";
import { bindWfSettings } from "./wf-settings";
import { bindAlwaysOnToggle, updateAlwaysOnBtn } from "./always-on";
import { activateZone } from "./sidebar-sections";
import { bindDrawerResize, bindPanelResize } from "./resize";

type Toast = (msg: string, type?: "success" | "error" | "info") => void;

export interface ToolbarResult {
  refreshRunBtn:     () => void;
  runWithValidation: () => Promise<void>;
}

export function bindToolbar(
  canvas:           Canvas,
  wfManager:        WorkflowManager,
  runManager:       RunManager,
  credPanel:        CredentialPanel,
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
    if (!await checkDangerousNodes(wfManager.currentId, canvas, approvedForExecution, showConfirm)) return;
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
    runMainBtn.title = schedulerRunning && !runManager.isRunning
      ? "Stop this scheduled background run"
      : "";
  };

  runManager.onRunStateChange = () => refreshRunBtn();

  runMainBtn.addEventListener("click", () => {
    const schedulerRunning = getBgJobs().some(
      j => j.id === wfManager.currentId && j.status === "running"
    );
    if (runManager.isRunning) {
      runManager.forceReset();
    } else if (schedulerRunning) {
      stopScheduledWorkflow(wfManager.currentId)
        .then(() => { refreshRunBtn(); updateBgRunButton(wfManager.currentId); })
        .catch(e => toast(`Could not stop: ${e}`, "error"));
    } else {
      runWithValidation();
    }
  });

  $("btn-save").addEventListener("click",    () => wfManager.handleSave());
  bindAlwaysOnToggle(wfManager, toast);
  $("btn-versions").addEventListener("click", () => showVersionPanel(wfManager, toast));
  bindWfSettings(wfManager);

  $("btn-run-now")?.addEventListener("click", () => { closeAllDropdowns(); runWithValidation(); });
  $("btn-run-chevron").addEventListener("click", (e) => {
    e.stopPropagation();
    const menu   = $("run-dropdown-menu");
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
        activateZone("bgruns");
        toast("Workflow scheduled — running in background", "success");
      }
    } catch { /* port_conflict re-throws — modal handled elsewhere */ }
  });

  $("btn-export").addEventListener("click", (e) => {
    e.stopPropagation();
    const menu   = $("export-dropdown-menu");
    const isOpen = menu.classList.contains("open");
    closeAllDropdowns();
    if (!isOpen) menu.classList.add("open");
  });
  $("btn-export-flowo")?.addEventListener("click", () => { closeAllDropdowns(); wfManager.handleExport(); });
  $("btn-export-server")?.addEventListener("click", () => { closeAllDropdowns(); showExportServerPanel(wfManager.currentId, toast); });

  document.addEventListener("click", () => closeAllDropdowns());

  $("btn-new-workflow").addEventListener("click", () => wfManager.handleNew());
  $("btn-credentials").addEventListener("click",  () => credPanel.show());
  $("btn-close-drawer").addEventListener("click", () => {
    $("output-drawer").classList.add("hidden");
    document.documentElement.style.removeProperty("--drawer-offset");
    canvas.resize();
  });
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
        toast(enabled ? "Flowo will now launch at login" : "Launch at login disabled", "success");
      } catch (err) {
        toast(`Could not update launch at login: ${err}`, "error");
        (e.target as HTMLInputElement).checked = !enabled;
      }
    });

  const gridSnapEl = document.getElementById("setting-grid-snap") as HTMLInputElement | null;
  if (gridSnapEl) {
    gridSnapEl.checked = localStorage.getItem("flowo_grid_snap") === "true";
    gridSnapEl.addEventListener("change", () => {
      localStorage.setItem("flowo_grid_snap", String(gridSnapEl.checked));
    });
  }

  const autofitEl = document.getElementById("setting-autofit") as HTMLInputElement | null;
  if (autofitEl) {
    autofitEl.checked = localStorage.getItem("flowo_autofit") !== "false";
    autofitEl.addEventListener("change", () => {
      localStorage.setItem("flowo_autofit", String(autofitEl.checked));
    });
  }

  $("btn-shortcuts").addEventListener("click",   () => $("shortcuts-modal").classList.remove("hidden"));
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
    setTimeout(() => RunManager.updateDrawerOffset(canvas), 20);
  });

  document.querySelectorAll(".panel-tab").forEach(t =>
    t.addEventListener("click", () => {
      const tab = (t as HTMLElement).dataset.tab ?? "run";
      switchTab(tab);
      openPanel();
    })
  );
  $("btn-close-panel").addEventListener("click", () => closePanel());

  const titleEl = $("workflow-name-label");
  titleEl.addEventListener("dblclick", () => wfManager.startRename(titleEl));

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
      const panel = document.getElementById("right-panel");
      const otherModalIds = ["confirm-modal", "import-preview-modal", "settings-modal", "onboarding-modal"];
      const anyModalVisible = otherModalIds.some(id => !document.getElementById(id)?.classList.contains("hidden"));
      if (!anyModalVisible && panel && panel.offsetParent !== null) closePanel();
    }
    if (e.key === "m" || e.key === "M") {
      const w      = document.getElementById("minimap-wrap");
      const hidden = w?.classList.toggle("minimap-hidden");
      localStorage.setItem("flowo_minimap_hidden", String(!!hidden));
    }
  });

  // Minimap toggle
  const minimapWrap   = document.getElementById("minimap-wrap");
  const minimapBtn    = document.getElementById("btn-minimap-toggle");
  const minimapHidden = localStorage.getItem("flowo_minimap_hidden") === "true";
  if (minimapHidden) minimapWrap?.classList.add("minimap-hidden");
  minimapBtn?.addEventListener("click", () => {
    const hidden = minimapWrap?.classList.toggle("minimap-hidden");
    localStorage.setItem("flowo_minimap_hidden", String(!!hidden));
  });

  bindDrawerResize();
  bindPanelResize();

  // Sidebar: navigate to bg-runs on bg-run click in workflow list
  renderBgJobs(wfManager, runManager, "all", "", toast);

  // Trigger initial always-on button state
  updateAlwaysOnBtn(canvas, wfManager);
  updateStatusHint();

  return { refreshRunBtn, runWithValidation };
}

function closeAllDropdowns(): void {
  document.querySelectorAll(".toolbar-dropdown-menu").forEach(m => m.classList.remove("open"));
}
