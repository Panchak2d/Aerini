import { afterEach, describe, expect, it } from "vitest";
import { registerNodeDescriptors } from "../canvas/node-registry";
import { displayPortLabel } from "../port-labels";

const desc = (type_id: string, is_plugin = false) => ({
  type_id, display_name: type_id, node_type: "action", version: "1",
  input_schema: {}, output_schema: {}, ports: { inputs: [], outputs: [] }, is_plugin,
});

describe("port label display", () => {
  afterEach(() => registerNodeDescriptors([]));

  it("shows_Result_for_a_built_in_nodes_Out_port", () => {
    registerNodeDescriptors([desc("code")] as never);
    expect(displayPortLabel("code", "output", "Out")).toBe("Result");
  });

  it("leaves_input_error_plugin_and_custom_labels_alone", () => {
    registerNodeDescriptors([desc("code"), desc("plug", true)] as never);
    expect(displayPortLabel("code", "input", "In")).toBe("In");
    expect(displayPortLabel("code", "on_error", "Error")).toBe("Error");
    expect(displayPortLabel("code", "output", "Chunks")).toBe("Chunks");
    expect(displayPortLabel("plug", "output", "Out")).toBe("Out");
    expect(displayPortLabel("unknown_type", "output", "Out")).toBe("Out");
  });
});
