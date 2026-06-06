import { invoke } from "@tauri-apps/api/core";
import { mk, mkField, mkSection, type ExtensionContext } from "./popover-utils";

type SfSlot = { id: string; name: string; source_expr?: string };

export function renderSaveToFolderFields(ctx: ExtensionContext): void {
  const { node, body, onChange } = ctx;

  body.appendChild(mkSection("Folder"));

  // Folder picker row — cannot use mkField because it needs a custom two-column layout.
  const folderWrap = document.createElement("div");
  folderWrap.className = "field-group";
  const folderLabel = document.createElement("label");
  folderLabel.className = "field-label";
  folderLabel.textContent = "Destination Folder";
  folderWrap.appendChild(folderLabel);

  const folderRow = document.createElement("div");
  folderRow.className = "folder-picker-row";

  const folderDisplay = document.createElement("span");
  folderDisplay.className = "folder-picker-path";
  folderDisplay.textContent = (node.data.config["folder_path"] as string) || "Not set";

  const pickBtn = document.createElement("button");
  pickBtn.type = "button";
  pickBtn.className = "folder-pick-btn";
  pickBtn.textContent = "Choose Folder";
  pickBtn.addEventListener("click", async () => {
    const path = await invoke<string | null>("pick_folder_dialog").catch(() => null);
    if (path) {
      node.data.config["folder_path"] = path;
      folderDisplay.textContent = path;
      onChange();
    }
  });

  folderRow.appendChild(folderDisplay);
  folderRow.appendChild(pickBtn);
  folderWrap.appendChild(folderRow);
  body.appendChild(folderWrap);

  body.appendChild(mkField("Overwrite existing files", () => {
    const chk = mk<HTMLInputElement>("input");
    chk.type = "checkbox";
    chk.checked = (node.data.config["overwrite"] as boolean) !== false;
    chk.addEventListener("change", () => { node.data.config["overwrite"] = chk.checked; onChange(); });
    return chk;
  }));

  body.appendChild(mkSection("Subfolders"));

  const sfHint = document.createElement("div");
  sfHint.className = "config-hint";
  sfHint.textContent = "Each subfolder becomes an input port. Connect an upstream node to each port.";
  body.appendChild(sfHint);

  const sfListEl = document.createElement("div");
  sfListEl.className = "subfolder-list";

  const renderSfList = () => {
    sfListEl.innerHTML = "";
    const sfs = (node.data.config["subfolders"] as SfSlot[] | undefined) ?? [];
    sfs.forEach((sf, idx) => {
      const row = document.createElement("div");
      row.className = "subfolder-row";

      const nameInp = mk<HTMLInputElement>("input");
      nameInp.type = "text";
      nameInp.value = sf.name;
      nameInp.placeholder = "Subfolder name";
      nameInp.autocomplete = "off";
      nameInp.spellcheck = false;
      nameInp.addEventListener("input", () => {
        (node.data.config["subfolders"] as SfSlot[])[idx].name = nameInp.value;
        onChange();
      });

      const delBtn = document.createElement("button");
      delBtn.type = "button";
      delBtn.className = "subfolder-del-btn";
      delBtn.textContent = "\u00d7";
      delBtn.title = "Remove subfolder";
      delBtn.addEventListener("click", () => {
        (node.data.config["subfolders"] as SfSlot[]).splice(idx, 1);
        renderSfList();
        onChange();
      });

      row.appendChild(nameInp);
      row.appendChild(delBtn);
      sfListEl.appendChild(row);
    });
  };

  renderSfList();
  body.appendChild(sfListEl);

  const addSfBtn = document.createElement("button");
  addSfBtn.type = "button";
  addSfBtn.className = "subfolder-add-btn";
  addSfBtn.textContent = "+ Add Subfolder";
  addSfBtn.addEventListener("click", () => {
    if (!Array.isArray(node.data.config["subfolders"])) {
      node.data.config["subfolders"] = [];
    }
    (node.data.config["subfolders"] as Array<{ id: string; name: string; source_expr: string }>).push({
      id: `sf_${Date.now()}`,
      name: "New Subfolder",
      source_expr: "",
    });
    renderSfList();
    onChange();
  });
  body.appendChild(addSfBtn);
}
