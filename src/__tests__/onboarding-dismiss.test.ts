// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initOnboarding, ONBOARDED_KEY } from "../onboarding";

const html = `
  <div id="onboarding-modal" class="hidden">
    <div class="onboarding-box">
      <button id="onboarding-close"></button>
      <div id="onboarding-templates"></div>
      <button id="onboarding-dismiss"></button>
    </div>
  </div>`;

const modal = () => document.getElementById("onboarding-modal")!;
const shown = () => !modal().classList.contains("hidden");

describe("onboarding dialog dismissal", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    localStorage.removeItem(ONBOARDED_KEY);
    document.body.innerHTML = html;
    initOnboarding(async () => true);
    vi.advanceTimersByTime(400);
  });
  afterEach(() => vi.useRealTimers());

  it("close_button_hides_the_dialog_and_remembers_it", () => {
    expect(shown()).toBe(true);
    document.getElementById("onboarding-close")!.click();
    expect(shown()).toBe(false);
    expect(localStorage.getItem(ONBOARDED_KEY)).toBe("1");
  });

  it("escape_closes_the_dialog_and_stops_reaching_other_key_handlers", () => {
    const other = vi.fn();
    document.body.addEventListener("keydown", other);
    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    expect(shown()).toBe(false);
    expect(other).not.toHaveBeenCalled();
  });

  it("escape_after_closing_is_not_swallowed", () => {
    document.getElementById("onboarding-dismiss")!.click();
    const other = vi.fn();
    document.body.addEventListener("keydown", other);
    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    expect(other).toHaveBeenCalledTimes(1);
  });

  it("backdrop_click_closes_but_a_click_inside_the_box_does_not", () => {
    modal().querySelector(".onboarding-box")!.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    expect(shown()).toBe(true);
    modal().dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    expect(shown()).toBe(false);
  });
});
