import type { Canvas } from "./canvas/Canvas";
import type { NodeDescriptor } from "./ipc/workflow";

// Shows the command palette in wire-drop mode: user picks a node, which is
// placed at the drop point and auto-connected to the source wire.
export function openWireDropPicker(
  _allNodes: NodeDescriptor[],
  canvas: Canvas,
  _canvasEl: HTMLCanvasElement,
  onStatus: (m: string) => void,
): void {
  const overlay = document.getElementById("command-palette-overlay")!;
  const inp     = document.getElementById("palette-search") as HTMLInputElement;
  const hint    = document.getElementById("palette-hint");

  if (hint) hint.textContent = "Pick a node to connect · Esc cancel";

  overlay.classList.remove("hidden");
  inp.value = "";
  inp.dispatchEvent(new Event("input"));
  requestAnimationFrame(() => inp.focus());

  const onPick = (e: Event) => {
    if (!(e instanceof CustomEvent)) return;
    const detail = e.detail as NodeDescriptor | null;
    if (!detail) return;
    overlay.removeEventListener("wire-drop-pick", onPick);
    canvas.completeWireDrop(detail);
    onStatus(`Connected → ${detail.display_name}`);
    if (hint) hint.textContent = "↑↓ navigate · Enter place · Esc close";
  };
  overlay.addEventListener("wire-drop-pick", onPick);

  const onClose = () => {
    overlay.removeEventListener("wire-drop-pick", onPick);
    canvas._pendingWireDrop = null;
    if (hint) hint.textContent = "↑↓ navigate · Enter place · Esc close";
  };
  overlay.addEventListener("palette-closed", onClose, { once: true });
}
