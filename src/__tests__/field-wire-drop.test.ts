// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  beginFieldWireDrag, cancelFieldWireDrag, endFieldWireDrag, fieldDropOwnsPress, isFieldWireDragActive,
  registerFieldDropTarget, type FieldDropTarget,
} from "../field-wire-drop";

const unregs: Array<() => void> = [];
function mount(accepts: (id: string) => boolean = (id) => id === "src"): { t: FieldDropTarget; host: HTMLElement; drop: ReturnType<typeof vi.fn> } {
  const host = document.createElement("div");
  const field = document.createElement("input");
  host.appendChild(field);
  document.body.appendChild(host);
  const drop = vi.fn();
  const t: FieldDropTarget = { field, host, sourceAt: () => "src", accepts, drop };
  unregs.push(registerFieldDropTarget(t));
  return { t, host, drop };
}
function at(el: Element | null): void {
  (document as unknown as { elementFromPoint: () => Element | null }).elementFromPoint = () => el;
}
const up = (shiftKey = false) => new MouseEvent("mouseup", { clientX: 5, clientY: 5, shiftKey });

afterEach(() => {
  cancelFieldWireDrag();
  while (unregs.length) unregs.pop()!();
  document.body.replaceChildren();
});

describe("dropping a wire on a field", () => {
  it("uses the node's data when released over an accepting field, and highlights while dragging", () => {
    const { host, drop, t } = mount();
    beginFieldWireDrag("src", "output");
    expect(host.classList.contains("field-wire-target")).toBe(true);
    at(t.field);
    window.dispatchEvent(new MouseEvent("mousemove", { clientX: 5, clientY: 5 }));
    expect(host.classList.contains("field-wire-target--hover")).toBe(true);
    expect(endFieldWireDrag(up())).toBe(true);
    expect(drop).toHaveBeenCalledWith("src", false);
    expect(host.classList.contains("field-wire-target")).toBe(false);
    expect(isFieldWireDragActive()).toBe(false);
  });

  it("passes Shift through, and leaves the wire alone when dropped elsewhere or cancelled", () => {
    const { t, drop } = mount();
    beginFieldWireDrag("src", "output");
    at(t.field);
    expect(endFieldWireDrag(up(true))).toBe(true);
    expect(drop).toHaveBeenLastCalledWith("src", true);

    beginFieldWireDrag("src", "output");
    at(document.body);
    expect(endFieldWireDrag(up())).toBe(false);

    beginFieldWireDrag("src", "output");
    expect(cancelFieldWireDrag()).toBe(true);
    at(t.field);
    expect(endFieldWireDrag(up())).toBe(false);
    expect(drop).toHaveBeenCalledTimes(1);
  });

  it("ignores non-result ports and nodes that cannot feed the field; presses on a feeding dot keep the popover open", () => {
    const { host } = mount((id) => id === "src");
    beginFieldWireDrag("src", "on_error");
    expect(isFieldWireDragActive()).toBe(false);
    beginFieldWireDrag("other", "output");
    expect(isFieldWireDragActive()).toBe(false);
    expect(host.classList.contains("field-wire-target")).toBe(false);
    expect(fieldDropOwnsPress(new MouseEvent("mousedown"))).toBe(true);
  });
});
