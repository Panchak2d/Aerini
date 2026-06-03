import { runWorkflow, cancelRun, startScheduledWorkflow, parseSchedulerError, type WorkflowResult } from "./ipc/workflow";
import { serialize } from "./canvas/CanvasSerializer";
import type { Canvas } from "./canvas/Canvas";
import {
  renderSummaryTab, renderResultsTab, renderErrorsTab,
  renderLogsView, renderDebugView,
  syntaxHighlight, extractPreview,
  wireCopyButtons,
} from "./output-renderer";
import { saveRunToHistory, renderHistoryPanel, type HistoryPanel } from "./run-history";
import { setWorkflowRunning } from "./workflow-manager";
import { isTauri, escapeHtml } from "./utils";

// Hard timeout for the entire workflow run. Prevents the UI from being
// stuck forever if the backend hangs (e.g. Schedule node, hung child process).
const RUN_TIMEOUT_MS = 120_000; // 2 minutes

export class RunManager {
  private canvas: Canvas;
  private onStatus: (msg: string) => void;
  private onToast:  (msg: string, type: "success" | "error" | "info") => void;
  private lastResult: WorkflowResult | null = null;
  private _isRunning = false;
  private _cancelRequested = false;
  private currentWorkflowName        = "Untitled";
  private currentWorkflowId          = "";
  private currentParallelExecution   = false;
  private currentMaxConcurrentNodes  = 8;
  private _activeHistoryPanel: HistoryPanel | null = null;

  onRunStateChange: ((running: boolean) => void) | null = null;

  get isRunning(): boolean { return this._isRunning; }

  // Called from app.ts whenever a workflow is loaded onto the canvas —
  // either via handleLoad, handleNew, or any other navigation event.
  // Keeps RunManager's currentWorkflowId in sync so onNodeStatusEvent
  // routes correctly and showResultFromScheduler saves history under
  // the right workflow ID.
  setCurrentWorkflow(id: string, name: string, parallelExecution = false, maxConcurrentNodes = 8): void {
    this.currentWorkflowId         = id;
    this.currentWorkflowName       = name;
    this.currentParallelExecution  = parallelExecution;
    this.currentMaxConcurrentNodes = maxConcurrentNodes;
  }

  // Called from app.ts's global listenNodeStatus subscription.
  // Routes the event to the canvas only when the node belongs to the workflow
  // currently loaded on the canvas. This makes scheduler-triggered runs animate
  // exactly like manual runs — the subscription is permanent, not per-run.
  onNodeStatusEvent(workflowId: string, nodeId: string, status: string): void {
    if (workflowId !== this.currentWorkflowId) return;
    this.canvas.setNodeStatus(nodeId, status === "skipped" ? "idle" : status as "idle" | "running" | "success" | "error");
  }

  constructor(
    canvas: Canvas,
    onStatus: (m: string) => void,
    onToast:  (m: string, t: "success" | "error" | "info") => void
  ) {
    this.canvas = canvas;
    this.onStatus = onStatus;
    this.onToast  = onToast;
  }

  // Reveal the drawer and scroll to a specific node's output tab
  revealNodeOutput(nodeId: string, nodeName: string): void {
    if (!this.lastResult) return;
    const out = this.lastResult.node_outputs[nodeId];
    if (out === undefined) return;
    const drawer = document.getElementById("output-drawer")!;
    drawer.classList.remove("hidden");
    // Find the tab for this node and click it
    const tabs = document.querySelectorAll<HTMLElement>(".drawer-tab");
    const label = nodeName.slice(0, 18);
    for (const tab of tabs) {
      if (tab.textContent?.includes(label) || tab.textContent?.includes(nodeId.slice(0, 8))) {
        tab.click();
        return;
      }
    }
  }

  // Show error details in the drawer when user clicks a failed node
  showNodeErrorDetail(nodeId: string, nodeName: string): void {
    if (!this.lastResult) return;
    const errorLogs = this.lastResult.logs.filter(
      l => l.node_id === nodeId && l.level === "error"
    );
    if (!errorLogs.length) return;

    const drawer  = document.getElementById("output-drawer")!;
    const content = document.getElementById("output-content")!;
    drawer.classList.remove("hidden");

    const nodeOutput = this.lastResult.node_outputs[nodeId];
    const inputContext = this.lastResult.node_outputs; // best proxy for what node received

    content.innerHTML = `
      <div class="error-detail-card">
        <div class="error-detail-header">
          <span class="error-detail-badge">FAILED</span>
          <span class="error-detail-name">${escapeHtml(nodeName)}</span>
        </div>
        <div class="error-detail-section">
          <div class="error-detail-label">What went wrong</div>
          ${errorLogs.map(l =>
            `<div class="error-detail-msg">${escapeHtml(l.message)}</div>`
          ).join("")}
        </div>
        ${nodeOutput ? `
        <div class="error-detail-section">
          <div class="error-detail-label">Node output (partial)</div>
          <pre class="node-card-json">${syntaxHighlight(JSON.stringify(nodeOutput, null, 2))}</pre>
        </div>` : ""}
        <div class="error-detail-section">
          <details>
            <summary class="error-detail-label error-detail-label--interactive">Execution context (what other nodes produced)</summary>
            <pre class="node-card-json node-card-json--scrollable">${syntaxHighlight(JSON.stringify(inputContext, null, 2))}</pre>
          </details>
        </div>
        <button class="run-error-copy" id="run-error-copy-btn">Copy error</button>
      </div>`;
    const copyBtn = content.querySelector<HTMLButtonElement>("#run-error-copy-btn");
    if (copyBtn) {
      const errorText = errorLogs.map(l => l.message).join("\n");
      copyBtn.addEventListener("click", () => {
        navigator.clipboard.writeText(errorText).then(() => { copyBtn.textContent = "Copied"; });
      });
    }
  }

  // Called when a scheduler-status event arrives with a completed result.
  // Saves to history and opens the full output panel (Summary + node tabs + Logs + History).
  showResultFromScheduler(workflowName: string, result: WorkflowResult): void {
    saveRunToHistory(this.currentWorkflowId, workflowName, result);

    // Always paint the final node states onto the canvas — this is what makes
    // nodes turn green/red after a scheduled run, matching the manual run UX.
    for (const [id, out] of Object.entries(result.node_outputs)) {
      const preview = extractPreview(out);
      this.canvas.setNodeOutput(id, preview);
      const node = this.canvas.nodes.get(id);
      if (node && node.status === "success") {
        node.showOutput = true;
        node.updatePortPositions();
      }
    }
    this.canvas.clearConnectorActive();
    // Mark errored nodes explicitly (node_outputs only contains successful nodes)
    if (!result.success) {
      const f = result.logs.find((l) => l.level === "error" && l.node_id);
      if (f?.node_id) this.canvas.setNodeStatus(f.node_id, "error");
    }

    const drawer = document.getElementById("output-drawer");
    if (!drawer || drawer.classList.contains("hidden")) {
      // Drawer closed — canvas updated silently, don't interrupt the user
      return;
    }

    // Drawer is open — only refresh tabs if user is not browsing history
    if (this._activeHistoryPanel) {
      this._activeHistoryPanel._refresh();
      return;
    }

    const tabsEl  = document.getElementById("drawer-tabs");
    const content = document.getElementById("output-content");
    if (!tabsEl || !content) return;

    // Capture which tab the user is on before wiping the drawer.
    const activeTabLabel = document.querySelector<HTMLElement>(".drawer-tab.active")?.textContent?.trim();

    tabsEl.innerHTML = "";
    content.innerHTML = "";
    this.buildDrawerTabs(result);

    // Restore the previously active tab so a scheduled run doesn't yank the
    // user away from Errors/Logs/Results they were reading.
    if (activeTabLabel && activeTabLabel !== "Summary") {
      const tabs = tabsEl.querySelectorAll<HTMLElement>(".drawer-tab");
      for (const tab of tabs) {
        if (tab.textContent?.trim() === activeTabLabel) {
          tab.click();
          break;
        }
      }
    }
  }

  // Open the output drawer directly to the History tab.
  // Called when the user clicks a bg job in the sidebar — shows past runs
  // for that workflow without needing a live lastResult.
  openHistoryDrawer(workflowName?: string): void {
    const drawer  = document.getElementById("output-drawer");
    const tabsEl  = document.getElementById("drawer-tabs");
    const content = document.getElementById("output-content");
    if (!drawer || !tabsEl || !content) return;

    drawer.classList.remove("hidden");
    tabsEl.innerHTML = "";
    content.innerHTML = "";

    const historyTab = this.makeTab("History", "tab-neutral");
    this.setActiveTab(historyTab);
    tabsEl.appendChild(historyTab);

    const panel = renderHistoryPanel(this.currentWorkflowId, (result: WorkflowResult) => {
      this._activeHistoryPanel = null;
      this.buildDrawerTabsFromResult(result);
    });
    this._activeHistoryPanel = panel;
    content.appendChild(panel);
  }

  // Force-reset the run button. Called externally (e.g. on handleNew, or Stop click).
  forceReset(): void {
    if (!this._isRunning) return;
    this._cancelRequested = true;
    this.canvas.resetAllStatus();
    cancelRun().catch(() => {}); // fire-and-forget; no-op if idle
    const panelBtn = document.getElementById("btn-run-panel") as HTMLButtonElement | null;
    if (panelBtn) { panelBtn.disabled = false; panelBtn.textContent = "Run Workflow"; }
    this._isRunning = false;
    this.onRunStateChange?.(false);
    this.onStatus("Run cancelled");
  }

  async handleRun(currentId: string, currentName: string): Promise<void> {
    this.currentWorkflowName = currentName;
    this.currentWorkflowId   = currentId;
    if (this._isRunning) {
      this.onToast("A workflow is already running. Wait for it to finish.", "info");
      return;
    }
    if (this.canvas.nodes.size === 0) {
      this.onToast("Add at least one node before running", "info");
      return;
    }

    const panelBtn = document.getElementById("btn-run-panel") as HTMLButtonElement | null;
    this._isRunning = true;
    this.onRunStateChange?.(true);
    setWorkflowRunning(currentId, true);   // mark in sidebar
    if (panelBtn) { panelBtn.disabled = true; panelBtn.textContent = "Running…"; }

    this.canvas.resetAllStatus();
    const drawer  = document.getElementById("output-drawer")!;
    const content = document.getElementById("output-content")!;
    drawer.classList.remove("hidden");
    // Update canvas padding so it shrinks above the drawer
    const drawerH = drawer.offsetHeight || 260;
    document.documentElement.style.setProperty("--drawer-offset", `${drawerH}px`);
    this.canvas.resize();
    content.innerHTML = '<div class="run-placeholder">Running workflow…</div>';
    this.clearLogs();
    this.onStatus("Running…");
    document.getElementById("drawer-tabs")!.innerHTML = "";

    const rs = document.getElementById("run-status");
    if (rs) { rs.textContent = ""; rs.style.color = ""; }

    let vars: Record<string, unknown> = {};
    try {
      const raw = (document.getElementById("run-input") as HTMLTextAreaElement)?.value?.trim();
      if (raw) vars = JSON.parse(raw);
    } catch {
      this.onToast("Test Input JSON is invalid — ignored", "info");
    }

    const json = serialize(currentId, currentName, this.canvas.nodes, this.canvas.connectors,
      this.currentParallelExecution, this.currentMaxConcurrentNodes);

    if (!isTauri()) {
      content.innerHTML = `<div class="run-notice">
        <strong>Browser mode</strong>
        Workflow execution requires the Tauri desktop app.
        Run <code>npm run dev</code> to enable execution.
      </div>`;
      this.resetBtns(panelBtn);
      this.onStatus("Run requires the desktop app");
      return;
    }

    // Everything from here is in one try/finally so the button ALWAYS resets.
    try {
      // Node-status events are routed via the global listenNodeStatus
      // subscription wired in app.ts → runManager.onNodeStatusEvent().
      // No per-run subscription is needed here.

      // Race the workflow against a hard timeout so the UI never gets permanently stuck
      const timeoutPromise = new Promise<never>((_, reject) =>
        setTimeout(() => reject(new Error(`Workflow timed out after ${RUN_TIMEOUT_MS / 1000}s. If you have a Schedule or Webhook node, it blocks until triggered.`)), RUN_TIMEOUT_MS)
      );

      const result = await Promise.race([
        runWorkflow(json, vars),
        timeoutPromise,
      ]);

      // If the user cancelled while we were awaiting, discard the result silently.
      // forceReset() already reset the UI — no toast, no history entry, no drawer rebuild.
      if (this._cancelRequested) {
        this._cancelRequested = false;
        return;
      }

      // Update canvas — show inline output previews on each node
      for (const [id, out] of Object.entries(result.node_outputs)) {
        const preview = extractPreview(out);
        this.canvas.setNodeOutput(id, preview);
        // Auto-show the inline preview strip on the node
        const node = this.canvas.nodes.get(id);
        if (node && node.status === "success") {
          node.showOutput = true;
          node.updatePortPositions();
        }
      }
      this.canvas.clearConnectorActive();
      if (!result.success) {
        const f = result.logs.find((l) => l.level === "error" && l.node_id);
        if (f?.node_id) this.canvas.setNodeStatus(f.node_id, "error");
      }

      // Save to run history
      saveRunToHistory(this.currentWorkflowId, this.currentWorkflowName, result);
      this.lastResult = result;

      this.buildDrawerTabs(result);
      this.populateLogs(result);

      if (rs) {
        rs.textContent = result.success ? "Complete" : "Failed";
        rs.style.color = result.success ? "var(--green)" : "var(--red)";
      }
      this.onStatus(result.success ? "Run complete" : "Run failed");
      if (!result.success) this.onToast(`Workflow failed: ${result.error ?? "Unknown error"}`, "error");

    } catch (e) {
      const errMsg = String(e);
      content.innerHTML = `<div class="run-error-card">
        <div class="run-error-label">Execution error</div>
        <div class="run-error-msg">${escapeHtml(errMsg)}</div>
        <button class="run-error-copy" id="run-catch-copy-btn">Copy error</button>
      </div>`;
      const catchCopyBtn = content.querySelector<HTMLButtonElement>("#run-catch-copy-btn");
      if (catchCopyBtn) {
        catchCopyBtn.addEventListener("click", () => {
          navigator.clipboard.writeText(errMsg).then(() => { catchCopyBtn.textContent = "Copied"; });
        });
      }
      this.onStatus(`Error: ${escapeHtml(errMsg)}`);
      this.onToast("Workflow run failed — see output for details", "error");
    } finally {
      // Always runs — even if runWorkflow hangs and times out
      this.resetBtns(panelBtn);
    }
  }

  // handleRunSingleNode — builds a subgraph of all ancestors + the target node
  async handleRunSingleNode(targetNodeId: string, currentId: string, currentName: string): Promise<void> {
    if (!isTauri()) { this.onStatus("Run requires the desktop app"); return; }

    this.currentWorkflowId   = currentId;
    this.currentWorkflowName = currentName;

    // Walk connectors backwards to collect all ancestor nodes
    const allNodes       = this.canvas.nodes;
    const allConnectors  = this.canvas.connectors;
    const includedIds    = new Set<string>();

    const collect = (nodeId: string) => {
      if (includedIds.has(nodeId)) return;
      includedIds.add(nodeId);
      for (const conn of allConnectors.values()) {
        if (conn.data.to_node === nodeId) collect(conn.data.from_node);
      }
    };
    collect(targetNodeId);

    // Filter to subgraph
    const subNodes = new Map([...allNodes].filter(([id]) => includedIds.has(id)));
    const subConns = new Map([...allConnectors].filter(([, c]) =>
      includedIds.has(c.data.from_node) && includedIds.has(c.data.to_node)
    ));

    if (subNodes.size === 0) {
      this.onStatus("Node not found"); return;
    }

    const json = serialize(`${currentId}_sub`, `${currentName} (node test)`, subNodes, subConns,
      this.currentParallelExecution, this.currentMaxConcurrentNodes);
    this.onStatus(`Running "${allNodes.get(targetNodeId)?.data.name ?? targetNodeId}"…`);

    // Open drawer
    const drawer  = document.getElementById("output-drawer");
    const tabsEl  = document.getElementById("drawer-tabs");
    const content = document.getElementById("output-content");
    if (drawer) drawer.classList.remove("hidden");
    if (tabsEl)  tabsEl.innerHTML  = `<span class="drawer-tab tab-neutral active">Running…</span>`;
    if (content) content.innerHTML = `<div class="run-spinner-wrap"><div class="run-spinner"></div><div class="run-spinner-label">Running node…</div></div>`;

    try {
      const result = await runWorkflow(json, {});
      for (const [id, out] of Object.entries(result.node_outputs)) {
        const preview = extractPreview(out);
        this.canvas.setNodeOutput(id, preview);
        const node = this.canvas.nodes.get(id);
        if (node && node.status === "success") { node.showOutput = true; node.updatePortPositions(); }
      }
      if (!result.success) {
        const f = result.logs.find((l) => l.level === "error" && l.node_id);
        if (f?.node_id) this.canvas.setNodeStatus(f.node_id, "error");
      }
      this.lastResult = result;
      this.buildDrawerTabs(result);
      this.onStatus(result.success ? "Node run complete" : "Node run failed");
    } catch (e) {
      this.onStatus(`Run failed: ${escapeHtml(String(e))}`);
      if (content) content.innerHTML = `<div class="run-error-msg">${escapeHtml(String(e))}</div>`;
    }
  }

  private buildDrawerTabs(result: WorkflowResult): void {
    const tabsEl  = document.getElementById("drawer-tabs")!;
    const content = document.getElementById("output-content")!;
    const nodes   = this.canvas.nodes;
    tabsEl.innerHTML = "";

    const hasErrors  = result.logs.some(l => l.level === "error");
    const hasOutputs = Object.keys(result.node_outputs).length > 0;

    const summaryTab = this.makeTab("Summary", result.success ? "tab-success" : "tab-error");
    summaryTab.addEventListener("click", () => {
      this._activeHistoryPanel = null;
      this.setActiveTab(summaryTab);
      content.innerHTML = renderSummaryTab(result, nodes);
      wireCopyButtons(content);
    });
    tabsEl.appendChild(summaryTab);

    let resultsTab: HTMLElement | null = null;
    if (hasOutputs) {
      resultsTab = this.makeTab("Results", "tab-neutral");
      resultsTab.addEventListener("click", () => {
        this._activeHistoryPanel = null;
        this.setActiveTab(resultsTab!);
        content.innerHTML = "";
        content.appendChild(renderResultsTab(result, nodes));
      });
      tabsEl.appendChild(resultsTab);
    }

    if (hasErrors) {
      const errTab = this.makeTab("Errors", "tab-error");
      errTab.addEventListener("click", () => {
        this._activeHistoryPanel = null;
        this.setActiveTab(errTab);
        content.innerHTML = renderErrorsTab(result, nodes);
        wireCopyButtons(content);
      });
      tabsEl.appendChild(errTab);
    }

    const logsTab = this.makeTab("Logs", "tab-neutral");
    logsTab.addEventListener("click", () => {
      this._activeHistoryPanel = null;
      this.setActiveTab(logsTab);
      content.innerHTML = renderLogsView(result);
    });
    tabsEl.appendChild(logsTab);

    const debugTab = this.makeTab("Debug", "tab-neutral");
    debugTab.addEventListener("click", () => {
      this._activeHistoryPanel = null;
      this.setActiveTab(debugTab);
      content.innerHTML = `<pre class="debug-log">${renderDebugView(result)}</pre>`;
    });
    tabsEl.appendChild(debugTab);

    const historyTab = this.makeTab("History", "tab-neutral");
    historyTab.addEventListener("click", () => {
      this.setActiveTab(historyTab);
      content.innerHTML = "";
      const panel = renderHistoryPanel(this.currentWorkflowId, (result: WorkflowResult) => {
        this._activeHistoryPanel = null;
        this.buildDrawerTabsFromResult(result);
      });
      this._activeHistoryPanel = panel;
      content.appendChild(panel);
    });
    tabsEl.appendChild(historyTab);

    // Default: always open Summary. User can navigate to Results/Errors themselves.
    this.setActiveTab(summaryTab);
    content.innerHTML = renderSummaryTab(result, nodes);
    wireCopyButtons(content);
  }

  // Restore a historical run — uses same 5-tab structure as live runs.
  // Canvas nodes may differ so we pass an empty map for history entries.
  private buildDrawerTabsFromResult(result: WorkflowResult): void {
    const tabsEl  = document.getElementById("drawer-tabs")!;
    const content = document.getElementById("output-content")!;
    tabsEl.innerHTML  = "";
    content.innerHTML = "";

    const nodes      = this.canvas.nodes;
    const hasErrors  = result.logs.some(l => l.level === "error");
    const hasOutputs = Object.keys(result.node_outputs).length > 0;

    // Back button — returns to the history list
    const backBtn = document.createElement("button");
    backBtn.className = "drawer-tab drawer-tab-back";
    backBtn.title = "Back to history";
    backBtn.innerHTML = `<svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="15 18 9 12 15 6"/></svg> History`;
    backBtn.addEventListener("click", () => this.openHistoryDrawer());
    tabsEl.appendChild(backBtn);

    // Divider between back button and run-specific tabs
    const sep = document.createElement("span");
    sep.className = "drawer-tab-sep";
    tabsEl.appendChild(sep);

    const summaryTab = this.makeTab("Summary", result.success ? "tab-success" : "tab-error");
    summaryTab.addEventListener("click", () => {
      this.setActiveTab(summaryTab);
      content.innerHTML = "";
      content.insertAdjacentHTML("beforeend", renderSummaryTab(result, nodes));
      wireCopyButtons(content);
    });
    tabsEl.appendChild(summaryTab);

    if (hasOutputs) {
      const resultsTab = this.makeTab("Results", "tab-neutral");
      resultsTab.addEventListener("click", () => {
        this.setActiveTab(resultsTab);
        content.innerHTML = "";
        content.appendChild(renderResultsTab(result, nodes));
      });
      tabsEl.appendChild(resultsTab);
    }

    if (hasErrors) {
      const errTab = this.makeTab("Errors", "tab-error");
      errTab.addEventListener("click", () => {
        this.setActiveTab(errTab);
        content.innerHTML = "";
        content.insertAdjacentHTML("beforeend", renderErrorsTab(result, nodes));
        wireCopyButtons(content);
      });
      tabsEl.appendChild(errTab);
    }

    const logsTab = this.makeTab("Logs", "tab-neutral");
    logsTab.addEventListener("click", () => {
      this.setActiveTab(logsTab);
      content.innerHTML = "";
      content.insertAdjacentHTML("beforeend", renderLogsView(result));
    });
    tabsEl.appendChild(logsTab);

    const debugTab = this.makeTab("Debug", "tab-neutral");
    debugTab.addEventListener("click", () => {
      this.setActiveTab(debugTab);
      content.innerHTML = "";
      content.insertAdjacentHTML("beforeend", `<pre class="debug-log">${renderDebugView(result)}</pre>`);
    });
    tabsEl.appendChild(debugTab);

    // Auto-show Summary immediately — no blank state
    this.setActiveTab(summaryTab);
    content.insertAdjacentHTML("beforeend", renderSummaryTab(result, nodes));
    wireCopyButtons(content);
    RunManager.flashActiveTab();
  }

  private makeTab(label: string, cls: string): HTMLElement {
    const t = document.createElement("button");
    t.className = `drawer-tab ${cls}`;
    t.textContent = label;
    return t;
  }

  private setActiveTab(active: HTMLElement): void {
    document.querySelectorAll(".drawer-tab").forEach(t => t.classList.remove("active"));
    active.classList.add("active");
  }

  clearLogs(): void {
    const el = document.getElementById("logs-list");
    if (el) el.innerHTML = "";
  }

  private populateLogs(result: WorkflowResult): void {
    const el = document.getElementById("logs-list");
    if (!el) return;
    el.innerHTML = "";
    for (const log of result.logs) {
      const entry = document.createElement("div");
      entry.className = `log-entry ${log.level}`;
      const t = new Date(log.timestamp).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
      entry.innerHTML = `<span class="log-time">${t}</span>${log.node_id ? `<span class="log-node">[${log.node_id.slice(0, 12)}]</span>` : ""}<span class="log-msg">${escapeHtml(log.message)}</span>`;
      if (log.node_id) {
        entry.style.cursor = "pointer";
        entry.addEventListener("click", () => {
          const n = this.canvas.nodes.get(log.node_id!);
          if (n) { this.canvas.clearSelection(); this.canvas.selectNode(n); }
        });
      }
      el.appendChild(entry);
    }
  }

  private resetBtns(p: HTMLButtonElement | null): void {
    this._isRunning = false;
    this.onRunStateChange?.(false);
    setWorkflowRunning(this.currentWorkflowId, false);
    if (p) { p.disabled = false; p.textContent = "Run Workflow"; }
  }

  // ── Drawer visibility helpers ─────────────────────────────────────────────
  // The output drawer is position:fixed and overlaps the canvas.
  // We set --drawer-offset on :root so #canvas-area shrinks above it,
  // then call canvas.resize() so the canvas element updates its pixel size.

  static showDrawer(canvas: import("./canvas/Canvas").Canvas): void {
    const drawer = document.getElementById("output-drawer")!;
    if (!drawer.classList.contains("hidden")) return; // already visible
    drawer.classList.remove("hidden");
    RunManager.updateDrawerOffset(canvas);
  }

  /** Briefly flash the active drawer tab green to signal new output arrived. */
  static flashActiveTab(): void {
    const active = document.querySelector<HTMLElement>(".drawer-tab.active");
    if (!active) return;
    active.classList.add("tab-flash");
    setTimeout(() => active.classList.remove("tab-flash"), 1500);
  }

  static hideDrawer(canvas: import("./canvas/Canvas").Canvas): void {
    const drawer = document.getElementById("output-drawer")!;
    drawer.classList.add("hidden");
    document.documentElement.style.removeProperty("--drawer-offset");
    canvas.resize();
  }

  static updateDrawerOffset(canvas: import("./canvas/Canvas").Canvas): void {
    const drawer = document.getElementById("output-drawer");
    if (!drawer || drawer.classList.contains("hidden")) {
      document.documentElement.style.removeProperty("--drawer-offset");
    } else {
      const h = drawer.offsetHeight || 260;
      document.documentElement.style.setProperty("--drawer-offset", `${h}px`);
    }
    canvas.resize();
  }


  // ── Background run ────────────────────────────────────────────────────────
  // executeBgJob receives a pre-captured snapshot {id, name, json}.
  // For schedulable triggers (schedule, webhook): hands off to the Rust
  // scheduler daemon which owns the loop forever.
  // For manual/non-schedulable triggers: rejects with a clear user-facing error.

  async executeBgJob(snapshot: { id: string; name: string; json: string }): Promise<boolean> {
    const { id, name, json } = snapshot;

    if (!isTauri()) {
      this.onToast("Background run requires the Tauri desktop app", "info");
      return false;
    }

    // Detect trigger type from the snapshot — find the entry node
    // (first node with node_type_id = schedule or webhook)
    let triggerType: string | null = null;
    try {
      const doc = JSON.parse(json) as { nodes?: Array<{ node_type_id: string }> };
      const nodes = doc.nodes ?? [];
      // entry node = no incoming edges; simplest check: node_type_id is a known trigger
      const triggerNode = nodes.find(n =>
        n.node_type_id === "schedule" || n.node_type_id === "webhook"
      );
      triggerType = triggerNode?.node_type_id ?? null;
    } catch {
      this.onToast(`Could not parse workflow — invalid format`, "error");
      return false;
    }

    // Manual trigger or no recognised trigger — reject with clear message
    if (!triggerType) {
      this.onToast(
        `"${name}" has no Schedule or Webhook trigger. Background run is only for recurring workflows. Use the Run button for one-shot execution.`,
        "error"
      );
      return false;
    }

    // Hand off to the Rust scheduler daemon — it owns the loop from here
    try {
      await startScheduledWorkflow(id);
      this.onStatus(`"${name}" scheduled — loop running in background`);
      // BgJobStore and sidebar dot are updated by scheduler-status events
      // emitted from the Rust side (wired in app.ts via listenSchedulerStatus)
      return true;
    } catch (rawError) {
      const err = parseSchedulerError(String(rawError));
      switch (err.error_kind) {
        case "port_conflict":
          // Port conflict modal is handled by app.ts which listens for this error type
          // Re-throw so app.ts can catch and show the modal
          throw rawError;
        case "already_running":
          this.onToast(`"${name}" is already running in the background.`, "info");
          return false;
        case "not_schedulable":
          this.onToast(
            `"${name}" cannot be scheduled — add a Schedule or Webhook trigger node.`,
            "error"
          );
          return false;
        case "workflow_not_found":
          this.onToast(`"${name}" was not found in the database. Save the workflow first.`, "error");
          return false;
        default:
          this.onToast(`Failed to start background run: ${(err as { message?: string }).message ?? rawError}`, "error");
          return false;
      }
    }
  }
}

// ── BgJobStore ────────────────────────────────────────────────────────────────
// Module-level, survives canvas switches. UI reads this to render the sidebar.

export interface BgJob {
  id: string;
  name: string;
  status: "running" | "done" | "failed" | "stopped";
  startedAt: number;
  finishedAt?: number;
  result?: WorkflowResult;
  error?: string;
  nextRunAt?: string | null;
  runCount?: number;
  alwaysOn?: boolean;
}

const _bgJobs = new Map<string, BgJob>();
let _bgJobListener: (() => void) | null = null;

export function onBgJobsChanged(fn: () => void): void {
  _bgJobListener = fn;
}

export function updateBgJobStore(id: string, job: BgJob): void {
  _bgJobs.set(id, job);
  _bgJobListener?.();
}

export function getBgJobs(): BgJob[] {
  return [..._bgJobs.values()].sort((a, b) => b.startedAt - a.startedAt);
}

export function removeBgJob(id: string): void {
  _bgJobs.delete(id);
  _bgJobListener?.();
}

export function hydrateBgJobsFromScheduler(rows: Array<{
  workflow_id: string; workflow_name: string; status: string;
  run_count: number; last_run_at: string | null; last_error: string | null;
  next_run_at: string | null;
}>): void {
  for (const row of rows) {
    const existing = _bgJobs.get(row.workflow_id);
    // Only skip if an in-memory live run is active — those have authoritative
    // real-time state from scheduler events that must not be overwritten by a
    // stale DB snapshot. Stopped/done/failed jobs must always accept DB refresh
    // so that next_run_at and run_count are current on app open.
    if (existing && existing.status === "running") continue;

    const isActive = row.status === "active";
    _bgJobs.set(row.workflow_id, {
      id:        row.workflow_id,
      name:      row.workflow_name,
      status:    isActive ? "running" : (row.status === "done" ? "done" : row.status === "stopped" ? "stopped" : "failed"),
      startedAt: existing?.startedAt ?? Date.now(),
      error:     row.last_error ?? undefined,
      // Populate nextRunAt and runCount from DB row so the countdown renders
      // immediately on startup without waiting for the first scheduler event.
      nextRunAt: row.next_run_at ?? undefined,
      runCount:  row.run_count,
    });
  }
  _bgJobListener?.();
}

// ── Bridge: Rust scheduler-status events → BgJobStore ─────────────────────────
// Called from app.ts whenever a scheduler-status event arrives from the daemon.
// Maps the Rust event shape to the frontend BgJob shape so renderBgJobs works
// for both in-process bg runs and daemon-managed scheduled jobs.

export function updateBgJobStoreFromEvent(evt: {
  workflow_id:   string;
  workflow_name: string;
  status:        string;
  run_count:     number;
  last_run_at:   string | null;
  next_run_at:   string | null;
  last_error:    string | null;
}): void {
  const existing = _bgJobs.get(evt.workflow_id);
  const startedAt = existing?.startedAt ?? Date.now();

  let status: BgJob["status"];
  switch (evt.status) {
    case "running":  status = "running"; break;
    case "done":     status = "done";    break;
    case "error":    status = "failed";  break;
    // Fix #8: "stopped" is a user-initiated stop — not a failure. Map to its own
    // status so it renders with a neutral dot instead of a red failure dot.
    case "stopped":  status = "stopped"; break;
    // "waiting" = between runs — show as running so the dot stays green/pulsing
    case "waiting":  status = "running"; break;
    default:         status = "running"; break;
  }

  updateBgJobStore(evt.workflow_id, {
    id:          evt.workflow_id,
    name:        evt.workflow_name,
    status,
    startedAt,
    finishedAt:  evt.status === "done" || evt.status === "error" || evt.status === "stopped"
                   ? Date.now()
                   : undefined,
    error:       evt.last_error ?? undefined,
    // Preserve the existing nextRunAt if the event carries null — the scheduler
    // emits a completion event with next_run_at=null immediately before emitting
    // the real next timestamp. Overwriting with null causes a visible countdown
    // flicker. The real value arrives in the very next event and will update it.
    nextRunAt:   evt.next_run_at ?? existing?.nextRunAt,
    runCount:    evt.run_count,
  });
}
