import type { WorkflowResult } from "../ipc/workflow";

// Pure run-state container — isolates "what is the run doing / what did it
// last produce" state from DOM, Canvas, and Tauri orchestration, which lives
// in stream-handler.ts. No DOM access. No Tauri calls. No Canvas reference.
// Fully testable in isolation.
//
// Purity verified: no document., no window., no Tauri imports.
// Only external dependency is the WorkflowResult type from ../ipc/workflow —
// a type-only import that produces zero runtime code.

export class RunStateMachine {
  private _isRunning = false;
  private _activeRunId: string | null = null;
  private _cancelRequested = false;
  private _lastResult: WorkflowResult | null = null;
  private _currentWorkflowName       = "Untitled";
  private _currentWorkflowId         = "";
  private _currentParallelExecution  = false;
  private _currentMaxConcurrentNodes = 8;

  get isRunning(): boolean { return this._isRunning; }
  // The run_id of the run currently in flight (main run or single-node run —
  // both go through start()/stop(), so this is always the right target
  // for cancel_run regardless of which entry point started it). null when idle.
  get activeRunId(): string | null { return this._activeRunId; }
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
  // previously set. Deliberately distinct from setCurrentWorkflow(): calling
  // that here instead would silently reset parallelExecution/
  // maxConcurrentNodes to their default parameter values (false / 8) on
  // every handleRun()/handleRunSingleNode() call.
  setWorkflowIdentity(id: string, name: string): void {
    this._currentWorkflowId   = id;
    this._currentWorkflowName = name;
  }

  // runId must be the same id passed to runWorkflow(...) for this run, so
  // cancelRun(activeRunId) targets the run actually in flight.
  start(runId: string): void { this._isRunning = true; this._activeRunId = runId; }
  stop(): void { this._isRunning = false; this._activeRunId = null; }

  requestCancel(): void { this._cancelRequested = true; }

  // Returns true (and clears the flag) if a cancel was requested — a
  // check-then-reset pattern; callers use this to discard a run's result
  // silently once Stop has already reset the UI.
  consumeCancelRequest(): boolean {
    const wasRequested = this._cancelRequested;
    this._cancelRequested = false;
    return wasRequested;
  }

  setLastResult(result: WorkflowResult | null): void { this._lastResult = result; }
}
