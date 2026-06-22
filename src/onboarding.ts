import type { Canvas } from "./canvas/Canvas";
import type { WorkflowManager } from "./workflow-manager";
import { NODE_IDS } from "./node-ids";

export function initOnboarding(canvas: Canvas, wfManager: WorkflowManager): void {
  const modal   = document.getElementById("onboarding-modal")!;
  const dismiss = document.getElementById("onboarding-dismiss")!;
  const example = document.getElementById("onboarding-load-example")!;

  if (localStorage.getItem("aerini_onboarded")) return;

  setTimeout(() => modal.classList.remove("hidden"), 400);

  const close = () => {
    modal.classList.add("hidden");
    localStorage.setItem("aerini_onboarded", "1");
  };

  dismiss.addEventListener("click", close);

  example.addEventListener("click", async () => {
    close();
    const now = new Date().toISOString();
    const exampleWorkflow = {
      schema_version: "1.0",
      id: "example_getting_started",
      name: "Getting Started — Fetch & Show",
      description: "",
      nodes: [
        {
          id: "n1", node_type_id: NODE_IDS.MANUAL_TRIGGER, node_type: "action",
          name: "Start", position: { x: 120, y: 200 }, config: {},
          credentials: {}, retry: { max_attempts: 1, backoff_ms: 500 },
          fallback_node: null,
          input_schema:  { type: "object", properties: { mock_payload: { type: "string", description: "Optional JSON payload to inject when running manually" } } },
          output_schema: { type: "object" },
          ports: { inputs: [], outputs: [{ id: "output", label: "Start", position: "right" }] },
        },
        {
          id: "n2", node_type_id: NODE_IDS.HTTP_REQUEST, node_type: "action",
          name: "Fetch Data", position: { x: 380, y: 200 },
          config: { url: "https://httpbin.org/get", method: "GET" },
          credentials: {}, retry: { max_attempts: 3, backoff_ms: 500 },
          fallback_node: null,
          input_schema:  { type: "object", properties: { url: { type: "string" }, method: { type: "string", enum: ["GET","POST","PUT","PATCH","DELETE"] } } },
          output_schema: { type: "object" },
          ports: { inputs: [{ id: "input", label: "In", position: "left" }], outputs: [{ id: "output", label: "Success", position: "right" }, { id: "on_error", label: "Error", position: "right" }] },
        },
        {
          id: "n3", node_type_id: NODE_IDS.OUTPUT, node_type: "utility",
          name: "Show Result", position: { x: 640, y: 200 },
          config: { label: "HTTP Result" },
          credentials: {}, retry: { max_attempts: 1, backoff_ms: 500 },
          fallback_node: null,
          input_schema:  { type: "object", properties: { label: { type: "string" }, source_node: { type: "string" }, field: { type: "string" } } },
          output_schema: { type: "object" },
          ports: { inputs: [{ id: "input", label: "In", position: "left" }], outputs: [{ id: "output", label: "Out", position: "right" }] },
        },
      ],
      edges: [
        { id: "e1", from_node: "n1", from_port: "output", to_node: "n2", to_port: "input", condition: null, on_success: null, on_failure: null },
        { id: "e2", from_node: "n2", from_port: "output", to_node: "n3", to_port: "input", condition: null, on_success: null, on_failure: null },
      ],
      metadata: { author: "user", created_at: now, updated_at: now, version: "1.0.0", tags: [] },
    };
    await wfManager.loadFromObject(exampleWorkflow);
  });

  // canvas param reserved for future use (e.g. animated demo path)
  void canvas;
}
