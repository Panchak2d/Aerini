import { planNodeTest, type NodeTestGraph } from "../node-test";
import { describeTestPlan, setUseCachedResults, useCachedResults, type TestPlan } from "../preset-cache";

function hasRecord(plan: TestPlan): boolean {
  return plan.cachedSteps.length > 0 || plan.stale.length > 0;
}

/**
 * Adds a short note above the node's settings saying how Test will treat the
 * steps before it, with a labelled choice between reusing their last results
 * and running them again. Nothing is added when no step feeds the node.
 */
export function mountTestScope(host: HTMLElement, graph: NodeTestGraph | undefined, nodeId: string): void {
  if (!graph) return;

  const render = async (): Promise<void> => {
    const { plan } = await planNodeTest(graph, nodeId, useCachedResults());
    host.querySelector(".popover-test-scope")?.remove();
    if (plan.ancestors.length === 0) return;

    const box = document.createElement("div");
    box.className = "popover-test-scope";

    if (hasRecord(plan)) {
      const label = document.createElement("label");
      label.className = "popover-test-scope-label";
      label.textContent = "Earlier steps";
      const select = document.createElement("select");
      select.className = "popover-test-scope-select";
      for (const [value, text] of [["cached", "Use last results"], ["all", "Run everything"]] as const) {
        const opt = document.createElement("option");
        opt.value = value;
        opt.textContent = text;
        select.appendChild(opt);
      }
      select.value = useCachedResults() ? "cached" : "all";
      select.addEventListener("change", () => {
        setUseCachedResults(select.value === "cached");
        void render();
      });
      label.appendChild(select);
      box.appendChild(label);
    }

    const note = document.createElement("div");
    note.className = "popover-test-scope-note";
    note.textContent = describeTestPlan(plan);
    box.appendChild(note);
    host.prepend(box);
  };

  render().catch(e => console.warn("Could not describe how Test will treat earlier steps:", e));
}
