// @vitest-environment jsdom

import { describe, it, expect, beforeEach } from "vitest";
import { bindWorkflowSectionControls } from "../sidebar-sections";

function itemHtml(id: string, name: string): string {
  return `<div class="workflow-item" data-wf-id="${id}"><span class="workflow-item-name">${name}</span></div>`;
}

describe("bindWorkflowSectionControls — search 'no match' message", () => {
  beforeEach(() => {
    document.body.innerHTML = `
      <input id="wf-search" />
      <button id="btn-wf-sort" data-sort="updated_desc"></button>
      <div id="workflow-list">
        <div class="workflow-collection-group">
          ${itemHtml("a", "Alpha")}
          ${itemHtml("b", "Beta")}
        </div>
      </div>
    `;
    bindWorkflowSectionControls(() => {});
  });

  it("shows a message naming the query when nothing matches", () => {
    const search = document.getElementById("wf-search") as HTMLInputElement;
    search.value = "zzz-nope";
    search.dispatchEvent(new Event("input"));

    const msg = document.getElementById("wf-search-no-match");
    expect(msg).toBeTruthy();
    expect(msg!.textContent).toContain("zzz-nope");
  });

  it("removes the message once a match reappears or the query is cleared", () => {
    const search = document.getElementById("wf-search") as HTMLInputElement;
    search.value = "zzz-nope";
    search.dispatchEvent(new Event("input"));
    expect(document.getElementById("wf-search-no-match")).toBeTruthy();

    search.value = "Alpha";
    search.dispatchEvent(new Event("input"));
    expect(document.getElementById("wf-search-no-match")).toBeFalsy();
  });

  it("never shows the message when there are no collection groups at all (the plain empty-library state already covers it)", () => {
    document.getElementById("workflow-list")!.innerHTML = "";
    const search = document.getElementById("wf-search") as HTMLInputElement;
    search.value = "anything";
    search.dispatchEvent(new Event("input"));
    expect(document.getElementById("wf-search-no-match")).toBeFalsy();
  });
});
