import { escapeHtml } from "../../utils";
import { mkSection, type ExtensionContext } from "../../node-configs/popover-utils";

export function renderAiCostWarning(ctx: ExtensionContext): void {
  const warn = document.createElement("div");
  warn.className = "config-hint config-hint-warn";
  warn.textContent = "Note: Each execution of this node will consume API credits. Set a sensible schedule interval to avoid unexpected costs.";
  ctx.body.appendChild(warn);
}

export function renderAiAttachments(ctx: ExtensionContext): void {
  const { node, body, onChange } = ctx;
  const config = node.data.config as Record<string, unknown>;

  // Ensure the array exists in memory without triggering onChange (no user action yet).
  if (!Array.isArray(config["attachments"])) {
    config["attachments"] = [];
  }

  // MIME fallback for browsers that return "" for non-standard types (e.g. .md on Windows).
  const MIME_MAP: Record<string, string> = {
    ".md":   "text/markdown",
    ".txt":  "text/plain",
    ".pdf":  "application/pdf",
    ".png":  "image/png",
    ".jpg":  "image/jpeg",
    ".jpeg": "image/jpeg",
    ".webp": "image/webp",
    ".gif":  "image/gif",
  };

  // SVG icons — consistent with codebase; no emoji (platform-inconsistent rendering).
  const FILE_ICON = `<svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/></svg>`;
  const X_ICON    = `<svg width="9" height="9" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>`;

  body.appendChild(mkSection("Attachments"));

  const typeLabel = document.createElement("div");
  typeLabel.className = "field-hint";
  typeLabel.textContent = "PNG \u00b7 JPEG \u00b7 WebP \u00b7 GIF \u00b7 PDF \u00b7 TXT \u00b7 MD";
  body.appendChild(typeLabel);

  const chipsWrap = document.createElement("div");
  chipsWrap.className = "attachment-chips";
  body.appendChild(chipsWrap);

  const sizeWarn = document.createElement("div");
  sizeWarn.className = "config-hint config-hint-warn";
  sizeWarn.textContent = "Large attachments increase workflow file size.";
  sizeWarn.style.display = "none";
  body.appendChild(sizeWarn);

  const MB          = 1024 * 1024;
  const WARN_SINGLE = 2 * MB;
  const WARN_TOTAL  = 5 * MB;

  function refreshChips(): void {
    const atts = config["attachments"] as Array<{ filename: string; data: string; mime_type: string }>;
    chipsWrap.innerHTML = "";

    // Byte estimate from base64 length: actual bytes \u2248 b64chars \u00d7 0.75
    let anyLarge   = false;
    let totalBytes = 0;
    for (const att of atts) {
      const bytes = att.data.length * 0.75;
      if (bytes > WARN_SINGLE) anyLarge = true;
      totalBytes += bytes;
    }
    sizeWarn.style.display = (anyLarge || totalBytes > WARN_TOTAL) ? "" : "none";

    for (let i = 0; i < atts.length; i++) {
      const att = atts[i];

      const chip = document.createElement("div");
      chip.className = "attachment-chip";

      const lbl = document.createElement("span");
      lbl.className = "attachment-chip-label";
      lbl.innerHTML = FILE_ICON + " " + escapeHtml(att.filename);
      lbl.title = att.filename;
      chip.appendChild(lbl);

      const removeBtn = document.createElement("button");
      removeBtn.type      = "button";
      removeBtn.className = "attachment-chip-remove";
      removeBtn.setAttribute("aria-label", "Remove " + att.filename);
      removeBtn.innerHTML = X_ICON;
      const idx = i;
      removeBtn.addEventListener("click", () => {
        (config["attachments"] as Array<unknown>).splice(idx, 1);
        onChange();
        refreshChips();
      });
      chip.appendChild(removeBtn);
      chipsWrap.appendChild(chip);
    }
  }

  refreshChips();

  const attachBtn = document.createElement("button");
  attachBtn.type      = "button";
  attachBtn.className = "subfolder-add-btn";
  attachBtn.textContent = "+ Attach File";
  attachBtn.addEventListener("click", () => {
    const fileInput = document.createElement("input");
    fileInput.type   = "file";
    fileInput.accept = ".png,.jpg,.jpeg,.webp,.gif,.pdf,.txt,.md";
    fileInput.addEventListener("change", () => {
      const file = fileInput.files?.[0];
      if (!file) { attachBtn.disabled = false; return; }

      attachBtn.disabled = true;

      const reader = new FileReader();

      reader.onerror = () => { attachBtn.disabled = false; };

      reader.onload = () => {
        // Guard: result is typed string | ArrayBuffer | null; must be string here.
        const raw = reader.result;
        if (typeof raw !== "string") { attachBtn.disabled = false; return; }

        // Slice after first comma — correct way to strip "data:<mime>;base64," prefix.
        const b64 = raw.slice(raw.indexOf(",") + 1);

        // file.type is unreliable for non-standard extensions (e.g. .md on Windows).
        const ext      = file.name.slice(file.name.lastIndexOf(".")).toLowerCase();
        const mimeType = file.type || MIME_MAP[ext] || "application/octet-stream";

        (config["attachments"] as Array<{ filename: string; data: string; mime_type: string }>)
          .push({ filename: file.name, data: b64, mime_type: mimeType });

        onChange();
        refreshChips();
        attachBtn.disabled = false;
      };

      reader.readAsDataURL(file);
    });
    fileInput.click();
  });
  body.appendChild(attachBtn);
}
