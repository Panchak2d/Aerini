/* @vitest-environment jsdom */
import { describe, it, expect, beforeAll } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

let baseCss: string;
let workspaceCss: string;
let monitorCss: string;

beforeAll(() => {
  baseCss      = readFileSync(resolve(__dirname, "../styles/base.css"), "utf-8");
  workspaceCss = readFileSync(resolve(__dirname, "../styles/workspace.css"), "utf-8");
  monitorCss   = readFileSync(resolve(__dirname, "../styles/monitor.css"), "utf-8");
});

function buildDom(collapsed: boolean) {
  document.head.innerHTML = `<style>${baseCss}\n${workspaceCss}</style>`;
  document.body.innerHTML = `
    <div id="output-drawer" class="${collapsed ? "hidden" : ""}">
      <div class="drawer-header" id="drawer-header">
        <span>Output</span>
        <span class="drawer-hint">Run the workflow to see results</span>
      </div>
    </div>
  `;
}

describe("output-drawer collapsed-state CSS cascade (R2C regression)", () => {
  it("edge case (the bug): collapsed drawer header is NOT display:none, despite base.css's global .hidden rule", () => {
    buildDom(true);
    const drawer = document.getElementById("output-drawer")!;
    const header = document.getElementById("drawer-header")!;
    expect(getComputedStyle(drawer).display).not.toBe("none");
    expect(getComputedStyle(header).display).not.toBe("none");
  });

  it("normal case: open (non-collapsed) drawer keeps its normal flex layout", () => {
    buildDom(false);
    const drawer = document.getElementById("output-drawer")!;
    expect(getComputedStyle(drawer).display).toBe("flex");
  });
});

describe("output-drawer full suppression during Monitor mode", () => {
  function buildMonitorDom(drawerCollapsed: boolean) {
    document.head.innerHTML = `<style>${baseCss}\n${workspaceCss}\n${monitorCss}</style>`;
    document.body.className = "monitor-mode";
    document.body.innerHTML = `
      <div id="output-drawer" class="${drawerCollapsed ? "hidden" : ""}">
        <div class="drawer-header" id="drawer-header"><span>Output</span></div>
      </div>
    `;
  }

  it("normal case: a collapsed drawer is fully display:none, not just its collapsed peek bar", () => {
    buildMonitorDom(true);
    expect(getComputedStyle(document.getElementById("output-drawer")!).display).toBe("none");
  });

  it("edge case: even a drawer some other code path force-opened (no .hidden class) stays display:none while body.monitor-mode is set", () => {
    buildMonitorDom(false);
    expect(getComputedStyle(document.getElementById("output-drawer")!).display).toBe("none");
  });
});
