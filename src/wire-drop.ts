import type { Connector } from "./canvas/Connector";
import type { Canvas } from "./canvas/Canvas";
import type { NodeDescriptor } from "./ipc/workflow";

const DEFAULT_HINT = "↑↓ navigate · Enter place · Esc close";

interface PickerMode {
  hint: string;
  /** The overlay event palette-manager dispatches when a node is picked in this mode. */
  pickEvent: string;
  onPick: (desc: NodeDescriptor) => void;
  /** Runs when the palette closes without a pick, and clears the mode's pending state. */
  onCancel: () => void;
}

// Shows the command palette in a pick mode and tears the mode down again
// whether the user picks a node or closes the palette.
function openPicker(mode: PickerMode): void {
  const overlay = document.getElementById("command-palette-overlay")!;
  const inp     = document.getElementById("palette-search") as HTMLInputElement;
  const hint    = document.getElementById("palette-hint");

  if (hint) hint.textContent = mode.hint;

  overlay.classList.remove("hidden");
  inp.value = "";
  inp.dispatchEvent(new Event("input"));
  requestAnimationFrame(() => inp.focus());

  const onPick = (e: Event) => {
    if (!(e instanceof CustomEvent)) return;
    const detail = e.detail as NodeDescriptor | null;
    if (!detail) return;
    overlay.removeEventListener(mode.pickEvent, onPick);
    mode.onPick(detail);
    if (hint) hint.textContent = DEFAULT_HINT;
  };
  overlay.addEventListener(mode.pickEvent, onPick);

  const onClose = () => {
    overlay.removeEventListener(mode.pickEvent, onPick);
    mode.onCancel();
    if (hint) hint.textContent = DEFAULT_HINT;
  };
  overlay.addEventListener("palette-closed", onClose, { once: true });
}

// Wire-drop mode: user picks a node, which is placed at the drop point and
// auto-connected to the source wire.
export function openWireDropPicker(
  canvas: Canvas,
  onStatus: (m: string) => void,
): void {
  openPicker({
    hint: "Pick a node to connect · Esc cancel",
    pickEvent: "wire-drop-pick",
    onPick: (desc) => {
      canvas.completeWireDrop(desc);
      onStatus(`Connected → ${desc.display_name}`);
    },
    onCancel: () => { canvas._pendingWireDrop = null; },
  });
}

// Input-wire-drop mode: user picks a node, which is placed to the left of the
// dragged input port and auto-connected into it.
export function openInputWireDropPicker(
  canvas: Canvas,
  onStatus: (m: string) => void,
): void {
  openPicker({
    hint: "Pick a node to wire in · Esc cancel",
    pickEvent: "input-wire-drop-pick",
    onPick: (desc) => {
      canvas.completeInputWireDrop(desc);
      onStatus(`Connected ← ${desc.display_name}`);
    },
    onCancel: () => { canvas._pendingInputWireDrop = null; },
  });
}

// Place-at-point mode: the picked node lands at `at` (canvas-relative pixels).
// With `onto`, it is also spliced into that wire.
export function openPlacePicker(
  canvas: Canvas,
  at: { sx: number; sy: number },
  onStatus: (m: string) => void,
  onto: Connector | null = null,
): void {
  canvas._pendingPlacement = { sx: at.sx, sy: at.sy, onto };
  openPicker({
    hint: onto ? "Pick a node to insert · Esc cancel" : "Pick a node to add here · Esc cancel",
    pickEvent: "place-pick",
    onPick: (desc) => {
      canvas.completePlacement(desc);
      onStatus(`Placed ${desc.display_name} — double-click to configure`);
    },
    onCancel: () => { canvas._pendingPlacement = null; },
  });
}
