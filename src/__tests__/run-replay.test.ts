/* @vitest-environment jsdom */
import { describe, it, expect, vi } from "vitest";
import type { Canvas } from "../canvas/Canvas";
import type { RunRecord } from "../run-history";
import { RunManager } from "../run-manager";
import { REPLAY_NODE_OUTPUT_KEY } from "../ipc/workflow";

function triggerCanvas(): Map<string, unknown> {
  return new Map([["n1", { data: { id: "n1", node_type_id: "manual_trigger" } }]]);
}

function makeManager(nodes?: Map<string, unknown>): RunManager {
  const canvas = { nodes: nodes ?? new Map() } as unknown as Canvas;
  return new RunManager(canvas, vi.fn(), vi.fn());
}

function recordWith(nodeOutputs: Record<string, unknown>): RunRecord {
  return {
    id: "run-1", workflow_id: "wf-1", workflow_name: "My Flow", ran_at: new Date().toISOString(),
    success: true, duration_ms: 42, status: "completed",
    result_json: JSON.stringify({
      execution_id: "run-1", workflow_id: "wf-1", success: true, node_outputs: nodeOutputs, logs: [],
    }),
  };
}

describe("RunManager.replayRun", () => {
  it("normal case: replays the trigger node's exact recorded output through handleRun's overrideVars hook", async () => {
    const rm = makeManager(triggerCanvas());
    const handleRun = vi.spyOn(rm, "handleRun").mockResolvedValue(undefined);

    await rm.replayRun(recordWith({ n1: { body: { hello: "world" } } }));

    expect(handleRun).toHaveBeenCalledWith("", "Untitled", {
      [REPLAY_NODE_OUTPUT_KEY]: { node_id: "n1", output: { body: { hello: "world" } } },
    });
  });

  it("edge case: no trigger node on canvas — toasts an error and never calls handleRun", async () => {
    const rm = makeManager(new Map([["n1", { data: { id: "n1", node_type_id: "http_request" } }]]));
    const handleRun = vi.spyOn(rm, "handleRun").mockResolvedValue(undefined);
    const onToast = (rm as unknown as { onToast: ReturnType<typeof vi.fn> }).onToast;

    await rm.replayRun(recordWith({ n1: { foo: 1 } }));

    expect(handleRun).not.toHaveBeenCalled();
    expect(onToast).toHaveBeenCalledWith(expect.stringContaining("no trigger node"), "error");
  });

  it("edge case: run has no recorded output for the current trigger node — toasts an error and never calls handleRun", async () => {
    const rm = makeManager(triggerCanvas());
    const handleRun = vi.spyOn(rm, "handleRun").mockResolvedValue(undefined);
    const onToast = (rm as unknown as { onToast: ReturnType<typeof vi.fn> }).onToast;

    // result_json recorded some other node's output only — trigger "n1" is absent,
    // e.g. the workflow was edited and the trigger node's id changed since this run.
    await rm.replayRun(recordWith({ some_other_node: { foo: 1 } }));

    expect(handleRun).not.toHaveBeenCalled();
    expect(onToast).toHaveBeenCalledWith(expect.stringContaining("no recorded trigger output"), "error");
  });
});
