import { mk, mkSection, type ExtensionContext } from "./popover-utils";

type SrcSlot = { id: string; name: string; source_expr?: string };

export function renderCollectFilesFields(ctx: ExtensionContext): void {
  const { node, body, onChange } = ctx;

  body.appendChild(mkSection("Sources"));

  const srcHint = document.createElement("div");
  srcHint.className = "config-hint";
  srcHint.textContent = "Each source becomes an input port. Connect an upstream node to each port.";
  body.appendChild(srcHint);

  const srcListEl = document.createElement("div");
  srcListEl.className = "subfolder-list";

  const renderSrcList = () => {
    srcListEl.innerHTML = "";
    const srcs = (node.data.config["sources"] as SrcSlot[] | undefined) ?? [];
    srcs.forEach((src, idx) => {
      const row = document.createElement("div");
      row.className = "subfolder-row";

      const nameInp = mk<HTMLInputElement>("input");
      nameInp.type = "text";
      nameInp.value = src.name;
      nameInp.placeholder = "Source label";
      nameInp.autocomplete = "off";
      nameInp.spellcheck = false;
      nameInp.addEventListener("input", () => {
        (node.data.config["sources"] as SrcSlot[])[idx].name = nameInp.value;
        onChange();
      });

      const delBtn = document.createElement("button");
      delBtn.type = "button";
      delBtn.className = "subfolder-del-btn";
      delBtn.textContent = "\u00d7";
      delBtn.title = "Remove source";
      delBtn.addEventListener("click", () => {
        (node.data.config["sources"] as SrcSlot[]).splice(idx, 1);
        renderSrcList();
        onChange();
      });

      row.appendChild(nameInp);
      row.appendChild(delBtn);
      srcListEl.appendChild(row);
    });
  };

  renderSrcList();
  body.appendChild(srcListEl);

  const addSrcBtn = document.createElement("button");
  addSrcBtn.type = "button";
  addSrcBtn.className = "subfolder-add-btn";
  addSrcBtn.textContent = "+ Add Source";
  addSrcBtn.addEventListener("click", () => {
    if (!Array.isArray(node.data.config["sources"])) {
      node.data.config["sources"] = [];
    }
    const srcs = node.data.config["sources"] as Array<{ id: string; name: string; source_expr: string }>;
    srcs.push({
      id: `src_${Date.now()}`,
      name: `Source ${srcs.length + 1}`,
      source_expr: "",
    });
    renderSrcList();
    onChange();
  });
  body.appendChild(addSrcBtn);
}
