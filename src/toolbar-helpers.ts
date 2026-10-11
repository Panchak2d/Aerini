export type Toast = (msg: string, type?: "success" | "error" | "info") => void;

export function buildIconBtn(id: string, title: string, svgInner: string): HTMLButtonElement {
  const btn = document.createElement("button");
  btn.id = id;
  btn.type = "button";
  btn.className = "btn-toolbar btn-icon-only";
  btn.title = title;
  btn.setAttribute("data-tooltip", title);
  btn.setAttribute("aria-label", title);
  btn.innerHTML = svgInner;
  return btn;
}
