import { describe, it, expect } from "vitest";
import { registerNodeDescriptors, getAllNodeDescriptors, isTriggerNodeType, findTriggerNodeTypeId } from "../canvas/node-registry";
import type { NodeDescriptor } from "../ipc/workflow";

function desc(type_id: string): NodeDescriptor {
  return {
    type_id, display_name: type_id, node_type: "action", version: "1",
    input_schema: {}, output_schema: {},
    ports: { inputs: [], outputs: [] },
  };
}

describe("getAllNodeDescriptors", () => {
  it("normal case: returns every registered descriptor, in registration order", () => {
    registerNodeDescriptors([desc("a"), desc("b"), desc("c")]);
    expect(getAllNodeDescriptors().map(d => d.type_id)).toEqual(["a", "b", "c"]);
  });

  it("edge case: reflects only the most recent registerNodeDescriptors() call, not an accumulation across calls", () => {
    registerNodeDescriptors([desc("a"), desc("b")]);
    registerNodeDescriptors([desc("x")]);
    expect(getAllNodeDescriptors().map(d => d.type_id)).toEqual(["x"]);
  });
});

describe("isTriggerNodeType", () => {
  it("is true for built-in trigger ids without any registration", () => {
    registerNodeDescriptors([]);
    expect(isTriggerNodeType("schedule")).toBe(true);
    expect(isTriggerNodeType("webhook")).toBe(true);
    expect(isTriggerNodeType("manual_trigger")).toBe(true);
    expect(isTriggerNodeType("http_request")).toBe(false);
  });

  it("is true for a registered trigger_capable plugin and false for a plain plugin", () => {
    registerNodeDescriptors([
      { ...desc("hb"), is_plugin: true, trigger_capable: true },
      { ...desc("plain"), is_plugin: true },
    ]);
    expect(isTriggerNodeType("hb")).toBe(true);
    expect(isTriggerNodeType("plain")).toBe(false);
    expect(isTriggerNodeType("unknown")).toBe(false);
  });

  it("findTriggerNodeTypeId returns a plugin trigger as the entry type", () => {
    registerNodeDescriptors([{ ...desc("hb"), is_plugin: true, trigger_capable: true }]);
    const nodes = [{ data: { node_type_id: "http_request" } }, { data: { node_type_id: "hb" } }];
    expect(findTriggerNodeTypeId(nodes)).toBe("hb");
  });
});
