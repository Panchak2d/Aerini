import { invoke } from "@tauri-apps/api/core";
import { mk, mkField, mkSection, type ExtensionContext } from "./popover-utils";

type SfSlot = { id: string; name: string; source_expr?: string };

export function renderSaveToFolderFields(ctx: ExtensionContext): void {
  const { node, body, onChange } = ctx;

  body.appendChild(mkSection("Folder"));

  // Folder picker — custom two-column layout; cannot use mkField.
  const folderWrap = document.createElement("div");
  folderWrap.className = "field-group";
  const folderLabel = document.createElement("label");
  folderLabel.className = "field-label";
  folderLabel.textContent = "Destination Folder";
  folderWrap.appendChild(folderLabel);

  const folderRow = document.createElement("div");
  folderRow.className = "folder-picker-row";
  folderRow.setAttribute("role", "group");
  folderRow.setAttribute("aria-label", "Destination folder");

  const folderDisplay = document.createElement("span");
  folderDisplay.className = "folder-picker-path";
  folderDisplay.textContent = (node.data.config["folder_path"] as string) || "Not set";
  folderDisplay.setAttribute("aria-live", "polite");

  const pickBtn = document.createElement("button");
  pickBtn.type = "button";
  pickBtn.className = "folder-pick-btn";
  pickBtn.textContent = "Choose Folder";
  pickBtn.setAttribute("aria-label", "Choose destination folder");
  pickBtn.addEventListener("click", async () => {
    pickBtn.disabled = true;
    const path = await invoke<string | null>("pick_folder_dialog").catch(() => null);
    pickBtn.disabled = false;
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
  }, "When disabled, files with the same name are kept and the new file is skipped."));

  body.appendChild(mkSection("Subfolders"));

  const sfHint = document.createElement("div");
  sfHint.className = "config-hint";
  sfHint.textContent = "Each subfolder becomes an input port. Connect a node that produces a files array (AI Image, Collect Files, S3 Storage).";
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
      nameInp.setAttribute("aria-label", `Subfolder ${idx + 1} name`);
      nameInp.addEventListener("input", () => {
        (node.data.config["subfolders"] as SfSlot[])[idx].name = nameInp.value;
        onChange();
      });

      const delBtn = document.createElement("button");
      delBtn.type = "button";
      delBtn.className = "subfolder-del-btn";
      delBtn.setAttribute("aria-label", `Remove subfolder ${idx + 1}`);
      delBtn.innerHTML = `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>`;
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
