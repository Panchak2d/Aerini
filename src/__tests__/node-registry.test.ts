import { describe, it, expect } from "vitest";
import { registerNodeDescriptors, getAllNodeDescriptors } from "../canvas/node-registry";
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
