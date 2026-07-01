import type { WorkflowResult } from "../ipc/workflow";

// Pure run-state container — extracted from RunManager (Patch 19) so the
// "what is the run doing / what did it last produce" state is isolated from
// DOM, Canvas, and Tauri orchestration, which lives in stream-handler.ts.
// No DOM access. No Tauri calls. No Canvas reference. Fully testable in isolation.
//
// Purity verified (Patch 26): no document., no window., no Tauri imports.
// Only external dependency is the WorkflowResult type from ../ipc/workflow —
// a type-only import that produces zero runtime code.

// Discriminated union of every event that can mutate run state.
// transition() is the single declared entry point; existing methods remain
// callable for backward compatibility with stream-handler.ts call sites.
export type RunEvent =
  | { type: "start" }
  | { type: "stop" }
  | { type: "request_cancel" }
  | { type: "set_result"; result: WorkflowResult | null }
  | { type: "set_workflow"; id: string; name: string; parallelExecution?: boolean; maxConcurrentNodes?: number }
  | { type: "set_workflow_identity"; id: string; name: string };

export class RunStateMachine {
  private _isRunning = false;
  private _cancelRequested = false;
  private _lastResult: WorkflowResult | null = null;
  private _currentWorkflowName       = "Untitled";
  private _currentWorkflowId         = "";
  private _currentParallelExecution  = false;
  private _currentMaxConcurrentNodes = 8;

  get isRunning(): boolean { return this._isRunning; }
  get lastResult(): WorkflowResult | null { return this._lastResult; }
  get currentWorkflowId(): string { return this._currentWorkflowId; }
  get currentWorkflowName(): string { return this._currentWorkflowName; }
  get currentParallelExecution(): boolean { return this._currentParallelExecution; }
  get currentMaxConcurrentNodes(): number { return this._currentMaxConcurrentNodes; }

  // Called from app.ts whenever a workflow is loaded onto the canvas —
  // either via handleLoad, handleNew, or any other navigation event.
  setCurrentWorkflow(id: string, name: string, parallelExecution = false, maxConcurrentNodes = 8): void {
    this._currentWorkflowId         = id;
    this._currentWorkflowName       = name;
    this._currentParallelExecution  = parallelExecution;
    this._currentMaxConcurrentNodes = maxConcurrentNodes;
  }

  // Updates only id/name, leaving parallelExecution/maxConcurrentNodes as
  // previously set. Mirrors the direct two-field assignment that handleRun()
  // and handleRunSingleNode() used to perform — calling setCurrentWorkflow()
  // there would silently reset parallelExecution/maxConcurrentNodes to their
  // default parameter values (false / 8), which is a behavior change.
  setWorkflowIdentity(id: string, name: string): void {
    this._currentWorkflowId   = id;
    this._currentWorkflowName = name;
  }

  start(): void { this._isRunning = true; }
  stop(): void  { this._isRunning = false; }

  requestCancel(): void { this._cancelRequested = true; }

  // Returns true (and clears the flag) if a cancel was requested — matches
  // the original check-then-reset pattern previously inlined in handleRun().
  consumeCancelRequest(): boolean {
    const wasRequested = this._cancelRequested;
    this._cancelRequested = false;
    return wasRequested;
  }

  setLastResult(result: WorkflowResult | null): void { this._lastResult = result; }

  // Single declared entry point for state transitions (Patch 26).
  // Delegates to the existing named methods so stream-handler.ts call sites
  // are unaffected — adding transition() is purely additive.
  transition(event: RunEvent): void {
    switch (event.type) {
      case "start":                 this.start(); break;
      case "stop":                  this.stop(); break;
      case "request_cancel":        this.requestCancel(); break;
      case "set_result":            this.setLastResult(event.result); break;
      case "set_workflow":          this.setCurrentWorkflow(event.id, event.name, event.parallelExecution, event.maxConcurrentNodes); break;
      case "set_workflow_identity": this.setWorkflowIdentity(event.id, event.name); break;
    }
  }
}
