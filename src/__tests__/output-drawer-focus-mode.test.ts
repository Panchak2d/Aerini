/* @vitest-environment jsdom */
import { describe, it, expect, beforeAll } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

let baseCss: string;
let workspaceCss: string;

beforeAll(() => {
  baseCss      = readFileSync(resolve(__dirname, "../styles/base.css"), "utf-8");
  workspaceCss = readFileSync(resolve(__dirname, "../styles/workspace.css"), "utf-8");
});

function buildDom(bodyClass: string): void {
  document.head.innerHTML = `<style>${baseCss}\n${workspaceCss}</style>`;
  document.body.className = bodyClass;
  document.body.innerHTML = `<div id="output-drawer"></div>`;
}

describe("#output-drawer left/right offset under focus mode", () => {
  it("baseline (not focus mode): left is keyed to --sidebar-w, right is 0 as before", () => {
    buildDom("");
    const drawer = document.getElementById("output-drawer")!;
    expect(getComputedStyle(drawer).left).toBe("var(--sidebar-w)");
    expect(getComputedStyle(drawer).right).toBe("0px");
  });

  it("edge case: focus mode alone must reset the offset to 0, not keep the stale --sidebar-w", () => {
    buildDom("focus-mode");
    const drawer = document.getElementById("output-drawer")!;
    expect(getComputedStyle(drawer).left).toBe("0px");
    expect(getComputedStyle(drawer).right).toBe("0px");
  });

  it("panel-open is inert (ERROR-RECOVERY): with or without focus mode, it no longer affects the drawer's offset", () => {
    buildDom("panel-open");
    const idle = document.getElementById("output-drawer")!;
    expect(getComputedStyle(idle).left).toBe("var(--sidebar-w)");
    expect(getComputedStyle(idle).right).toBe("0px");

    buildDom("focus-mode panel-open");
    const withFocus = document.getElementById("output-drawer")!;
    expect(getComputedStyle(withFocus).left).toBe("0px");
    expect(getComputedStyle(withFocus).right).toBe("0px");
  });
});
