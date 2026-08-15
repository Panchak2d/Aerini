/* @vitest-environment jsdom */
import { describe, it, expect, beforeEach } from "vitest";
import { enterMonitorMode, exitMonitorMode, isMonitorModeActive } from "../monitor-mode";

function setDom(drawerOpen: boolean): void {
  document.body.className = "";
  document.body.innerHTML = `<div id="output-drawer" class="${drawerOpen ? "" : "hidden"}"></div>`;
}

describe("monitor-mode", () => {
  beforeEach(() => setDom(true));

  it("normal case: entering sets body.monitor-mode, isMonitorModeActive() true, and force-hides an open drawer", () => {
    enterMonitorMode();
    expect(document.body.classList.contains("monitor-mode")).toBe(true);
    expect(isMonitorModeActive()).toBe(true);
    expect(document.getElementById("output-drawer")!.classList.contains("hidden")).toBe(true);
    exitMonitorMode();
  });

  it("edge case: exiting restores a drawer that was open before entry, and clears the body class", () => {
    enterMonitorMode();
    exitMonitorMode();
    expect(document.body.classList.contains("monitor-mode")).toBe(false);
    expect(isMonitorModeActive()).toBe(false);
    expect(document.getElementById("output-drawer")!.classList.contains("hidden")).toBe(false);
  });

  it("edge case: a drawer that was already closed before entry stays closed after exit", () => {
    setDom(false);
    enterMonitorMode();
    exitMonitorMode();
    expect(document.getElementById("output-drawer")!.classList.contains("hidden")).toBe(true);
  });

  it("edge case: a drawer closed before entry stays closed after exit even if another code path removed .hidden mid-session (e.g. starting a run from Monitor)", () => {
    setDom(false);
    enterMonitorMode();
    document.getElementById("output-drawer")!.classList.remove("hidden");
    exitMonitorMode();
    expect(document.getElementById("output-drawer")!.classList.contains("hidden")).toBe(true);
  });
});
