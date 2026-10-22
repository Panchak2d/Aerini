import { describe, it, expect } from "vitest";
import { isWorkflowNameTaken, uniqueWorkflowName } from "../workflow-names";

describe("uniqueWorkflowName", () => {
  it("returns the name unchanged when free", () => {
    expect(uniqueWorkflowName(["Other"], "AI chat panel")).toBe("AI chat panel");
  });

  it("skips every taken suffix, ignoring case and padding", () => {
    expect(uniqueWorkflowName(["AI chat panel", " ai chat panel (2) "], "AI chat panel")).toBe("AI chat panel (3)");
  });
});

describe("isWorkflowNameTaken", () => {
  const list = [{ id: "a", name: "Report" }, { id: "b", name: "Other" }];

  it("detects a clash with a different workflow, case-insensitively", () => {
    expect(isWorkflowNameTaken(list, " report ", "b")).toBe(true);
  });

  it("ignores the workflow being renamed and a missing list", () => {
    expect(isWorkflowNameTaken(list, "Report", "a")).toBe(false);
    expect(isWorkflowNameTaken(null, "Report")).toBe(false);
  });
});
