import { runWorkflow, cancelRun, startScheduledWorkflow, parseSchedulerError, REPLAY_NODE_OUTPUT_KEY, type WorkflowResult } from "../ipc/workflow";
import { isTriggerNodeType } from "../canvas/node-registry";
import { serialize } from "../canvas/CanvasSerializer";
import type { Canvas } from "../canvas/Canvas";
import { checkDangerousNodes } from "../validation";
import { showConfirm } from "../confirm";
import {
  renderSummaryTab, renderResultsTab, renderErrorsTab,
  renderLogsView, renderDebugView,
  syntaxHighlight, extractPreview,
  wireCopyButtons, ICON_CIRCLE_ALERT,
} from "../output-renderer";
import { saveRunToHistory, saveRunStarted, renderHistoryPanel, type HistoryPanel, type RunRecord } from "../run-history";
import { setWorkflowRunning } from "../workflow-manager";
import { isTauri, escapeHtml } from "../utils";
import { resolveNextRunAt, triggerTypeFromKindJson } from "../monitor-helpers";
import { RunStateMachine } from "./state-machine";

// Hard timeout for the entire workflow run. Prevents the UI from being
// stuck forever if the backend hangs (e.g. Schedule node, hung child process).
const RUN_TIMEOUT_MS = 120_000; // 2 minutes

// Single-node test runs are a much smaller subgraph than a full run — half
// the timeout is enough headroom without making a hung node-test tie up the
// UI (and the shared isRunning guard) for the full 2 minutes.
const SINGLE_NODE_TIMEOUT_MS = 60_000; // 1 minute

export class RunManager {
  private canvas: Canvas;
  private onStatus: (msg: string) => void;
  private onToast:  (msg: string, type: "success" | "error" | "info") => void;
  private state = new RunStateMachine();
  private _activeHistoryPanel: HistoryPanel | null = null;
  // Approval memory for the paths RunManager gates itself: "Run this node"
  // and Replay. Separate from toolbar.ts's Set for the main Run button.
  // Keys are derived from the dangerous nodes' content, so an approval can
  // only match the same nodes with the same settings.
  private approvedForExecution = new Set<string>();

  onRunStateChange: ((running: boolean) => void) | null = null;
  onRunResult: ((success: boolean) => void) | null = null;
  // Direct assignment to onRunStateChange only supports one registrant — a
  // second `runManager.onRunStateChange = ...` silently replaces the first
  // (this bit app.ts's status-hint updater, clobbered by toolbar.ts's
  // Run/Stop button toggle since bindToolbar() runs later in app.ts's init
  // order). Additional consumers (e.g. ChatPanel) must use this instead.
  private runStateListeners: Array<(running: boolean) => void> = [];

  addRunStateListener(fn: (running: boolean) => void): void {
    this.runStateListeners.push(fn);
  }

  private emitRunState(running: boolean): void {
    this.onRunStateChange?.(running);
    for (const fn of this.runStateListeners) fn(running);
  }

  get isRunning(): boolean { return this.state.isRunning; }

  // Called from app.ts whenever a workflow is loaded onto the canvas —
  // either via handleLoad, handleNew, or any other navigation event.
  // Keeps RunManager's currentWorkflowId in sync so onNodeStatusEvent
  // routes correctly and showResultFromScheduler saves history under
  // the right workflow ID.
  setCurrentWorkflow(id: string, name: string, parallelExecution = false, maxConcurrentNodes = 8): void {
    const switched = id !== this.state.currentWorkflowId;
    this.state.setCurrentWorkflow(id, name, parallelExecution, maxConcurrentNodes);
    if (switched && !this.state.isRunning) this.resetOutputPanel();
  }

  // Clears the output drawer's stale content on workflow switch (see
  // setCurrentWorkflow above). Idle markup mirrors #output-content's own
  // static default in index.html.
  private resetOutputPanel(): void {
    this._activeHistoryPanel = null;
    this.state.setLastResult(null);
    const tabsEl = document.getElementById("drawer-tabs");
    const content = document.getElementById("output-content");
    if (tabsEl) tabsEl.innerHTML = "";
    if (content) {
      content.innerHTML =
        `<div id="output-idle" class="output-idle"><div class="output-idle-title">Nothing run yet.</div><div class="output-idle-sub">Click <b>Run</b> above to execute this workflow.</div></div>`;
    }
  }

  // Called from app.ts's global listenNodeStatus subscription.
  // Routes the event to the canvas only when the node belongs to the workflow
  // currently loaded on the canvas. This makes scheduler-triggered runs animate
  // exactly like manual runs — the subscription is permanent, not per-run.
  onNodeStatusEvent(workflowId: string, nodeId: string, status: string): void {
    if (workflowId !== this.state.currentWorkflowId) return;
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
    // Performance lives outside #drawer-tabs (see index.html comment) so it's
    // never rebuilt by makeTab()'s per-run tab lists — wire its click once,
    // here, rather than re-binding it on every buildDrawerTabs* call.
    const perfTab = document.getElementById("drawer-tab-performance");
    perfTab?.addEventListener("click", () => this.setActiveTab(perfTab));
  }

  // Reveal the drawer and scroll to a specific node's output tab
  revealNodeOutput(nodeId: string, nodeName: string): void {
    const lastResult = this.state.lastResult;
    if (!lastResult) return;
    const out = lastResult.node_outputs[nodeId];
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
    const lastResult = this.state.lastResult;
    if (!lastResult) return;
    const errorLogs = lastResult.logs.filter(
      l => l.node_id === nodeId && l.level === "error"
    );
    if (!errorLogs.length) return;

    const drawer  = document.getElementById("output-drawer")!;
    const content = document.getElementById("output-content")!;
    drawer.classList.remove("hidden");
    this.showEphemeralPane();

    const nodeOutput = lastResult.node_outputs[nodeId];
    const inputContext = lastResult.node_outputs; // best proxy for what node received

    content.innerHTML = `
      <div class="error-detail-card">
        <div class="error-detail-header">
          ${ICON_CIRCLE_ALERT}
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
    const runId = `run_${Date.now()}_${Math.random().toString(36).slice(2, 7)}`;
    saveRunToHistory(runId, this.state.currentWorkflowId, workflowName, result);

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
    // user away from Errors/Logs/Results they were reading. Unscoped query —
    // Performance lives outside #drawer-tabs (never wiped, see index.html),
    // so a scoped query would silently fail to find and restore it.
    if (activeTabLabel && activeTabLabel !== "Summary") {
      const tabs = document.querySelectorAll<HTMLElement>(".drawer-tab");
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
  openHistoryDrawer(): void {
    const drawer  = document.getElementById("output-drawer");
    const tabsEl  = document.getElementById("drawer-tabs");
    const content = document.getElementById("output-content");
    if (!drawer || !tabsEl || !content) return;

    drawer.classList.remove("hidden");
    tabsEl.innerHTML = "";
    content.innerHTML = "";

    const historyTab = this.makeTab("History", "tab-neutral");
    // #output-content keeps the panel appended below (the Performance toggle
    // never clears it), so re-showing this pane is all a click needs to do.
    historyTab.addEventListener("click", () => this.setActiveTab(historyTab));
    this.setActiveTab(historyTab);
    tabsEl.appendChild(historyTab);

    const panel = renderHistoryPanel(
      this.state.currentWorkflowId,
      (result: WorkflowResult) => {
        this._activeHistoryPanel = null;
        this.buildDrawerTabsFromResult(result);
      },
      (record: RunRecord) => this.replayRun(record),
    );
    this._activeHistoryPanel = panel;
    content.appendChild(panel);
  }

  // Force-reset the run button. Called externally (e.g. on handleNew, or Stop click).
  // Also stops a single-node test run — handleRunSingleNode shares this same
  // isRunning/activeRunId pair, so forceReset's guard sees it as an active run.
  forceReset(): void {
    if (!this.state.isRunning) return;
    const runId = this.state.activeRunId;
    this.state.requestCancel();
    this.canvas.resetAllStatus();
    if (runId) cancelRun(runId).catch(() => {}); // fire-and-forget; no-op if idle
    const panelBtn = document.getElementById("btn-run-panel") as HTMLButtonElement | null;
    if (panelBtn) { panelBtn.disabled = false; panelBtn.textContent = "Run Workflow"; }
    this.state.stop();
    this.emitRunState(false);
    this.onStatus("Run cancelled");
  }

  // overrideVars: when supplied, used as initialVariables instead of reading
  // the Test Input JSON textarea — this is Run Replay's only hook into the
  // normal run pipeline (timeout race, cancellation, history save, drawer
  // build all stay shared with a manual Run rather than duplicated).
  async handleRun(currentId: string, currentName: string, overrideVars?: Record<string, unknown>): Promise<void> {
    this.state.setWorkflowIdentity(currentId, currentName);
    if (this.state.isRunning) {
      this.onToast("A workflow is already running. Wait for it to finish.", "info");
      return;
    }
    if (this.canvas.nodes.size === 0) {
      this.onToast("Add at least one node before running", "info");
      return;
    }

    // runId doubles as this run's identity for cancellation: the same id is
    // passed to runWorkflow() below and to cancelRun() from forceReset() or
    // the timeout path, so Stop/timeout always targets this specific run
    // rather than a shared global slot.
    const runId = `run_${Date.now()}_${Math.random().toString(36).slice(2, 7)}`;

    const panelBtn = document.getElementById("btn-run-panel") as HTMLButtonElement | null;
    this.state.start(runId);
    this.emitRunState(true);
    setWorkflowRunning(currentId, true);   // mark in sidebar
    if (panelBtn) { panelBtn.disabled = true; panelBtn.textContent = "Running…"; }

    this.canvas.resetAllStatus();
    const drawer  = document.getElementById("output-drawer")!;
    const content = document.getElementById("output-content")!;
    drawer.classList.remove("hidden");
    this.showEphemeralPane();
    content.innerHTML = '<div class="run-placeholder">Running workflow…</div>';
    this.onStatus("Running…");
    document.getElementById("drawer-tabs")!.innerHTML = "";

    const rs = document.getElementById("run-status");
    if (rs) { rs.textContent = ""; rs.style.color = ""; }

    let vars: Record<string, unknown> = {};
    if (overrideVars) {
      vars = overrideVars;
    } else {
      try {
        const raw = (document.getElementById("run-input") as HTMLTextAreaElement)?.value?.trim();
        if (raw) vars = JSON.parse(raw);
      } catch {
        this.onToast("Test Input JSON is invalid — ignored", "info");
      }
    }

    const json = serialize(currentId, currentName, this.canvas.nodes, this.canvas.connectors,
      this.state.currentParallelExecution, this.state.currentMaxConcurrentNodes);

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
    // `timedOut` records which side of the Promise.race below actually lost,
    // so the catch block can tell the backend to stop the still-running
    // workflow instead of merely giving up on watching it. Declared
    // here, before try, so both try and catch can see it.
    let timedOut = false;
    try {
      // Node-status events are routed via the global listenNodeStatus
      // subscription wired in app.ts → runManager.onNodeStatusEvent().
      // No per-run subscription is needed here.

      // "running" record written before execution starts (crash mid-run still
      // leaves a trace) — reuses the same runId generated above.
      saveRunStarted(runId, this.state.currentWorkflowId, this.state.currentWorkflowName);

      // Race the workflow against a hard timeout so the UI never gets permanently
      // stuck.
      const timeoutPromise = new Promise<never>((_, reject) =>
        setTimeout(() => {
          timedOut = true;
          reject(new Error(`Workflow timed out after ${RUN_TIMEOUT_MS / 1000}s. If you have a Schedule or Webhook node, it blocks until triggered; a trigger plugin runs once as an ordinary step.`));
        }, RUN_TIMEOUT_MS)
      );

      const result = await Promise.race([
        runWorkflow(json, vars, runId),
        timeoutPromise,
      ]);

      // If the user cancelled while we were awaiting, discard the result silently.
      // forceReset() already reset the UI — no toast, no history entry, no drawer rebuild.
      if (this.state.consumeCancelRequest()) {
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
      saveRunToHistory(runId, this.state.currentWorkflowId, this.state.currentWorkflowName, result);
      this.state.setLastResult(result);
      this.onRunResult?.(result.success);

      this.buildDrawerTabs(result);

      if (rs) {
        rs.textContent = result.success ? "Complete" : "Failed";
        rs.style.color = result.success ? "var(--green)" : "var(--red)";
      }
      this.onStatus(result.success ? "Run complete" : "Run failed");
      if (!result.success) this.onToast(`Workflow failed: ${result.error ?? "Unknown error"}`, "error");

    } catch (e) {
      // The timeout promise winning the race only stops the UI from watching
      // this run — it does not stop the run itself. Tell the backend to
      // actually cancel it; a genuine cancellation via Stop already
      // does this in forceReset(), this only covers the timeout path.
      if (timedOut) cancelRun(runId).catch(() => {});
      this.onRunResult?.(false);
      // run_workflow rejects with the same already_running JSON shape the
      // scheduler uses (per-workflow exec-lock) if this exact workflow is
      // somehow already running from another invocation — show the friendly
      // message instead of the raw {"error_kind":...} string.
      const parsed = parseSchedulerError(String(e));
      const errMsg = parsed.error_kind === "already_running"
        ? `"${currentName}" is already running from another action. Wait for it to finish, then try again.`
        : String(e);
      content.innerHTML = `<div class="run-error-card">
        <div class="run-error-label">${ICON_CIRCLE_ALERT}Execution error</div>
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

  // Replay a past run using its exact recorded trigger output — reuses the
  // entire normal run pipeline via handleRun's overrideVars hook (timeout,
  // cancellation, history save, drawer build). Only the entry trigger node's
  // output is replayed; every other node re-executes live against today's
  // workflow definition, same as a normal Run — so behavior can differ if
  // the workflow was edited since that run.
  async replayRun(record: RunRecord): Promise<void> {
    if (this.state.isRunning) {
      this.onToast("A workflow is already running. Wait for it to finish.", "info");
      return;
    }
    const triggerNode = [...this.canvas.nodes.values()].find(n => isTriggerNodeType(n.data.node_type_id));
    if (!triggerNode) {
      this.onToast("This workflow has no trigger node to replay — nothing to reuse as input.", "error");
      return;
    }
    let result: WorkflowResult;
    try {
      result = JSON.parse(record.result_json) as WorkflowResult;
    } catch {
      this.onToast("Could not read this run's recorded data.", "error");
      return;
    }
    const recordedOutput = result.node_outputs[triggerNode.data.id];
    if (recordedOutput === undefined) {
      this.onToast("This run has no recorded trigger output to replay — the workflow may have changed since.", "error");
      return;
    }
    if (!await checkDangerousNodes(this.state.currentWorkflowId, this.canvas.nodes.values(), this.approvedForExecution, showConfirm)) {
      this.onStatus("Run cancelled");
      return;
    }
    this.onToast(`Replaying with "${record.workflow_name}"'s recorded trigger input…`, "info");
    await this.handleRun(this.state.currentWorkflowId, this.state.currentWorkflowName, {
      [REPLAY_NODE_OUTPUT_KEY]: { node_id: triggerNode.data.id, output: recordedOutput },
    });
  }

  // handleRunSingleNode — builds a subgraph of all ancestors + the target node
  async handleRunSingleNode(targetNodeId: string, currentId: string, currentName: string): Promise<void> {
    if (!isTauri()) { this.onStatus("Run requires the desktop app"); return; }

    // shares handleRun's own isRunning guard — checked synchronously,
    // before any await, so this and handleRun() (or two rapid single-node
    // runs) can't both pass the check before either sets it. Setting
    // isRunning here is also what makes Stop (forceReset) work for a
    // single-node run — forceReset only acts when isRunning is true.
    if (this.state.isRunning) {
      this.onToast("A workflow is already running. Wait for it to finish.", "info");
      return;
    }

    const runId = `run_${Date.now()}_${Math.random().toString(36).slice(2, 7)}`;
    this.state.setWorkflowIdentity(currentId, currentName);
    this.state.start(runId);
    this.emitRunState(true);
    setWorkflowRunning(currentId, true);

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
      this.onStatus("Node not found");
      this.resetBtns(null);
      return;
    }

    // Gate on the actual subgraph about to run, not the whole canvas — a
    // Shell/Code/Database node anywhere in the target's ancestor chain must
    // not execute silently just because this path skips the main Run
    // button's validateWorkflow/checkDangerousNodes gate. Deliberately
    // NOT running validateWorkflow's whole-workflow checks (e.g. "has a
    // trigger") here — testing one node in isolation, before a trigger is
    // even wired, is this feature's normal, legitimate use.
    if (!await checkDangerousNodes(currentId, subNodes.values(), this.approvedForExecution, showConfirm)) {
      this.onStatus("Run cancelled");
      this.resetBtns(null);
      return;
    }

    const json = serialize(`${currentId}_sub`, `${currentName} (node test)`, subNodes, subConns,
      this.state.currentParallelExecution, this.state.currentMaxConcurrentNodes);
    this.onStatus(`Running "${allNodes.get(targetNodeId)?.data.name ?? targetNodeId}"…`);

    // Open drawer
    const drawer  = document.getElementById("output-drawer");
    const tabsEl  = document.getElementById("drawer-tabs");
    const content = document.getElementById("output-content");
    if (drawer) drawer.classList.remove("hidden");
    this.showEphemeralPane();
    if (tabsEl)  tabsEl.innerHTML  = `<span class="drawer-tab tab-neutral active">Running…</span>`;
    if (content) content.innerHTML = `<div class="run-spinner-wrap"><div class="run-spinner"></div><div class="run-spinner-label">Running node…</div></div>`;

    // Same race-against-timeout shape as handleRun(), scaled down for a
    // single-node test (SINGLE_NODE_TIMEOUT_MS).
    let timedOut = false;
    const timeoutPromise = new Promise<never>((_, reject) =>
      setTimeout(() => {
        timedOut = true;
        reject(new Error(`Node run timed out after ${SINGLE_NODE_TIMEOUT_MS / 1000}s.`));
      }, SINGLE_NODE_TIMEOUT_MS)
    );

    try {
      const result = await Promise.race([runWorkflow(json, {}, runId), timeoutPromise]);

      // User clicked Stop while we were awaiting — forceReset() already
      // reset the UI; discard the result silently, matching handleRun().
      if (this.state.consumeCancelRequest()) return;

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
      this.state.setLastResult(result);
      this.buildDrawerTabs(result);
      this.onStatus(result.success ? "Node run complete" : "Node run failed");
    } catch (e) {
      // Timeout only stops the UI from watching — tell the backend to
      // actually cancel the still-running node execution.
      if (timedOut) cancelRun(runId).catch(() => {});
      const parsed = parseSchedulerError(String(e));
      const errMsg = parsed.error_kind === "already_running"
        ? `"${currentName}" is already running from another action. Wait for it to finish, then try again.`
        : String(e);
      this.onStatus(`Run failed: ${escapeHtml(errMsg)}`);
      if (content) content.innerHTML = `<div class="run-error-msg">${escapeHtml(errMsg)}</div>`;
    } finally {
      // Always runs — even if runWorkflow hangs and times out.
      this.resetBtns(null);
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

    const logsTab = this.makeTab("Execution Transcript", "tab-neutral");
    logsTab.addEventListener("click", () => {
      this._activeHistoryPanel = null;
      this.setActiveTab(logsTab);
      content.innerHTML = renderLogsView(result);
      this.wireLogNodeLinks(content);
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
      const panel = renderHistoryPanel(
        this.state.currentWorkflowId,
        (result: WorkflowResult) => {
          this._activeHistoryPanel = null;
          this.buildDrawerTabsFromResult(result);
        },
        (record: RunRecord) => this.replayRun(record),
      );
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
    backBtn.innerHTML = `<svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="15 18 9 12 15 6"/></svg> History`;
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

    const logsTab = this.makeTab("Execution Transcript", "tab-neutral");
    logsTab.addEventListener("click", () => {
      this.setActiveTab(logsTab);
      content.innerHTML = "";
      content.insertAdjacentHTML("beforeend", renderLogsView(result));
      this.wireLogNodeLinks(content);
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
    const showingPerf = active.id === "drawer-tab-performance";
    document.getElementById("output-content")?.classList.toggle("hidden", showingPerf);
    document.getElementById("output-content-performance")?.classList.toggle("hidden", !showingPerf);
  }

  // Ensures #output-content is the visible pane before writing straight into
  // it (run placeholder, node-test spinner, error detail) — these paths
  // don't go through a .drawer-tab click, so setActiveTab's pane-toggle
  // never runs for them. Without this, content written while the
  // Performance tab is active would land in a hidden element.
  private showEphemeralPane(): void {
    document.getElementById("output-content")?.classList.remove("hidden");
    document.getElementById("output-content-performance")?.classList.add("hidden");
  }

  // Wires click (and Enter/Space, for keyboard users) on every rendered
  // log line that carries a data-node-id — same select-on-canvas affordance
  // and same click+keydown pairing as run-history.ts's buildItem().
  private wireLogNodeLinks(container: Element): void {
    container.querySelectorAll<HTMLElement>(".log-line[data-node-id]").forEach(line => {
      const id = line.dataset.nodeId!;
      const select = (): void => {
        const n = this.canvas.nodes.get(id);
        if (n) { this.canvas.clearSelection(); this.canvas.selectNode(n); }
      };
      line.addEventListener("click", select);
      line.addEventListener("keydown", (e) => {
        if (e.key !== "Enter" && e.key !== " ") return;
        e.preventDefault();
        select();
      });
    });
  }

  private resetBtns(p: HTMLButtonElement | null): void {
    this.state.stop();
    this.emitRunState(false);
    setWorkflowRunning(this.state.currentWorkflowId, false);
    if (p) { p.disabled = false; p.textContent = "Run Workflow"; }
  }

  /** Briefly flash the active drawer tab green to signal new output arrived. */
  static flashActiveTab(): void {
    const active = document.querySelector<HTMLElement>(".drawer-tab.active");
    if (!active) return;
    active.classList.add("tab-flash");
    setTimeout(() => active.classList.remove("tab-flash"), 1500);
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
    // (first node whose type is a built-in trigger or a trigger plugin)
    let triggerType: string | null = null;
    try {
      const doc = JSON.parse(json) as { nodes?: Array<{ node_type_id: string }> };
      const nodes = doc.nodes ?? [];
      // entry node = no incoming edges; simplest check: node_type_id is a known trigger
      const triggerNode = nodes.find(n => isTriggerNodeType(n.node_type_id));
      triggerType = triggerNode?.node_type_id ?? null;
    } catch {
      this.onToast(`Could not parse workflow — invalid format`, "error");
      return false;
    }

    // Manual trigger or no recognised trigger — reject with clear message
    if (!triggerType) {
      this.onToast(
        `"${name}" has no Schedule, Webhook, or trigger-plugin trigger. Background run is only for recurring workflows. Use the Run button for one-shot execution.`,
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
          // Re-thrown for the caller to handle — the response differs by
          // caller (toolbar.ts's "Run in background" handler shows a toast).
          throw rawError;
        case "already_running":
          this.onToast(`"${name}" is already running in the background.`, "info");
          return false;
        case "not_schedulable":
          this.onToast(
            `"${name}" cannot be scheduled — add a Schedule, Webhook, or trigger-plugin node.`,
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
  /** "interval" | "cron" | "once" | "webhook" | "manual" | "plugin"; absent when unknown. */
  triggerType?: string | null;
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
  next_run_at: string | null; trigger_kind?: string;
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
      nextRunAt: isActive ? (row.next_run_at ?? undefined) : undefined,
      runCount:  row.run_count,
      triggerType: triggerTypeFromKindJson(row.trigger_kind) ?? existing?.triggerType,
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
  trigger_type?: string | null;
}): void {
  const existing = _bgJobs.get(evt.workflow_id);
  const startedAt = existing?.startedAt ?? Date.now();

  let status: BgJob["status"];
  switch (evt.status) {
    case "running":  status = "running"; break;
    case "done":     status = "done";    break;
    case "error":    status = "failed";  break;
    // "stopped" is a user-initiated stop — not a failure. Map to its own
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
    // A null next_run_at keeps the previous value only while the job stays
    // running: the scheduler emits a completion event with null just before
    // the real next time, and overwriting it flickers the countdown. A stopped,
    // finished, or restarted job never keeps a stale countdown.
    nextRunAt:   resolveNextRunAt(evt.next_run_at, existing, status),
    runCount:    evt.run_count,
    triggerType: evt.trigger_type ?? existing?.triggerType,
  });
}
