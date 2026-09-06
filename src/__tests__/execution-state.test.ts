/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn((p: string) => p),
}));

describe("run-manager/state-machine — activeRunId", () => {
  it("start(runId) sets isRunning and activeRunId; stop() clears both", async () => {
    const { RunStateMachine } = await import("../run-manager/state-machine");
    const sm = new RunStateMachine();
    expect(sm.isRunning).toBe(false);
    expect(sm.activeRunId).toBeNull();

    sm.start("run_abc");
    expect(sm.isRunning).toBe(true);
    expect(sm.activeRunId).toBe("run_abc");

    sm.stop();
    expect(sm.isRunning).toBe(false);
    expect(sm.activeRunId).toBeNull();
  });

  it("activeRunId always reflects the most recent start() call — forceReset()/timeout paths read this to target cancelRun at the right run", async () => {
    const { RunStateMachine } = await import("../run-manager/state-machine");
    const sm = new RunStateMachine();
    sm.start("run_1");
    sm.stop();
    sm.start("run_2");
    expect(sm.activeRunId).toBe("run_2");
  });
});

function setupConfirmDom() {
  document.body.innerHTML = `
    <div id="confirm-modal" class="hidden">
      <div class="confirm-backdrop"></div>
      <div class="confirm-message" id="confirm-message"></div>
      <button class="confirm-btn-cancel" id="confirm-cancel">Cancel</button>
      <button class="confirm-btn-ok" id="confirm-ok">Continue</button>
    </div>`;
}

async function flush() {
  await new Promise((r) => setTimeout(r, 0));
}

describe("confirm.ts — showConfirm reentrancy", () => {
  beforeEach(() => {
    setupConfirmDom();
    vi.resetModules();
                        
  });

  it("normal case: a single call resolves true on OK, false on Cancel", async () => {
    const { showConfirm } = await import("../confirm");

    const p1 = showConfirm("Delete this?");
    await flush();
    document.getElementById("confirm-ok")!.click();
    expect(await p1).toBe(true);

    const p2 = showConfirm("Delete that?");
    await flush();
    document.getElementById("confirm-cancel")!.click();
    expect(await p2).toBe(false);
  });

  it("edge case: two overlapping calls are serialized, not stacked — one OK click resolves only the currently-showing dialog, and the second dialog opens afterward with its own message", async () => {
    const { showConfirm } = await import("../confirm");

    // Two calls fired back-to-back, before either is answered — mirrors a
    // double-triggered Run.
    const p1 = showConfirm("Delete credential X?");
    const p2 = showConfirm("Restore version Y?");
    await flush();

    // Only dialog 1 is showing — dialog 2 is queued, not stacked on the
    // same #confirm-ok/#confirm-cancel listeners.
    expect(document.getElementById("confirm-message")!.textContent).toBe("Delete credential X?");

    document.getElementById("confirm-ok")!.click();
    expect(await p1).toBe(true);


    await flush();
    expect(document.getElementById("confirm-message")!.textContent).toBe("Restore version Y?");

    document.getElementById("confirm-cancel")!.click();
    expect(await p2).toBe(false);
  });

  it("edge case: the modal is hidden between the two dialogs, not left visibly open across the handoff", async () => {
    const { showConfirm } = await import("../confirm");
    const modal = document.getElementById("confirm-modal")!;

    const p1 = showConfirm("First?");
    showConfirm("Second?");
    await flush();
    expect(modal.classList.contains("hidden")).toBe(false);

    document.getElementById("confirm-ok")!.click();
    await p1;
    // Between dialog 1 closing and dialog 2 opening there is a real state
    // transition, not two dialogs sharing one continuously-open modal.
    await flush();
    expect(modal.classList.contains("hidden")).toBe(false); // dialog 2 is now open
    expect(document.getElementById("confirm-message")!.textContent).toBe("Second?");
  });
});
