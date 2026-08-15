/* @vitest-environment jsdom */
import { describe, it, expect, beforeEach, vi } from "vitest";
import { initSidebarSections } from "../sidebar-sections";

const mountMonitorPanel = vi.fn();
const unmountMonitorPanel = vi.fn();
vi.mock("../panels/MonitorPanel", () => ({
  mountMonitorPanel: () => mountMonitorPanel(),
  unmountMonitorPanel: () => unmountMonitorPanel(),
}));

const LS_ZONE = "aerini_active_zone_v2";

function setDom(): void {
  document.body.className = "";
  document.body.innerHTML = `
    <div id="activity-bar">
      <button class="activity-btn active" data-zone="nodes"></button>
      <button class="activity-btn" data-zone="workflows"></button>
      <button class="activity-btn" data-zone="monitor"></button>
    </div>
    <div id="sidebar-panel">
      <div class="sidebar-zone active" id="zone-nodes"></div>
      <div class="sidebar-zone" id="zone-workflows"></div>
      <div class="sidebar-zone" id="zone-monitor"></div>
    </div>
    <div id="output-drawer"></div>
  `;
}

describe("sidebar activity-bar \u2014 Monitor mode call-out", () => {
  beforeEach(() => {
    localStorage.removeItem(LS_ZONE);
    setDom();
    mountMonitorPanel.mockClear();
    unmountMonitorPanel.mockClear();
  });

  it("normal case: clicking Monitor enters monitor mode and mounts the panel; clicking another tab exits and unmounts it", () => {
    initSidebarSections();
    document.querySelector<HTMLElement>('[data-zone="monitor"]')!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(document.body.classList.contains("monitor-mode")).toBe(true);
    expect(mountMonitorPanel).toHaveBeenCalledTimes(1);

    document.querySelector<HTMLElement>('[data-zone="workflows"]')!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(document.body.classList.contains("monitor-mode")).toBe(false);
    expect(unmountMonitorPanel).toHaveBeenCalledTimes(1);
  });

  it("edge case: a session restored with 'monitor' as the saved zone re-enters monitor mode and mounts the panel on init, not just the visual tab", () => {
    localStorage.setItem(LS_ZONE, JSON.stringify("monitor"));
    initSidebarSections();
    expect(document.body.classList.contains("monitor-mode")).toBe(true);
    expect(mountMonitorPanel).toHaveBeenCalledTimes(1);
  });
});
