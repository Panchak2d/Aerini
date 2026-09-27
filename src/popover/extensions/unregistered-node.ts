export function renderUnregisteredNodeNotice(body: HTMLElement, typeId: string): void {
  const note = document.createElement("div");
  note.className = "popover-info-banner popover-info-banner--warn";
  note.setAttribute("role", "note");
  const label = document.createElement("strong");
  label.textContent = "Node type not available: ";
  const id = document.createElement("code");
  id.textContent = typeId;
  note.append(
    label, id,
    " isn't registered in this app, so this node can't run here. It may come from a plugin " +
    "that isn't installed (the Plugins tab) or from a different version of Aerini. " +
    "Its saved settings are shown as-is and stay editable.",
  );
  body.appendChild(note);
}
