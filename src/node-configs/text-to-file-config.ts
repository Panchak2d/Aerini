import { mk, mkCustomSelect, mkField, mkSection, type ExtensionContext } from "./popover-utils";

// ── Format presets ────────────────────────────────────────────────────────────

const FORMATS: Array<{ label: string; ext: string; mime: string }> = [
  { label: "Plain Text (.txt)",  ext: "txt",  mime: "text/plain" },
  { label: "Markdown (.md)",     ext: "md",   mime: "text/markdown" },
  { label: "HTML (.html)",       ext: "html", mime: "text/html" },
  { label: "CSV (.csv)",         ext: "csv",  mime: "text/csv" },
  { label: "JSON (.json)",       ext: "json", mime: "application/json" },
  { label: "PDF (.pdf)",         ext: "pdf",  mime: "application/pdf" },
  { label: "Word (.docx)",       ext: "docx", mime: "application/vnd.openxmlformats-officedocument.wordprocessingml.document" },
];

function extOf(filename: string): string {
  return filename.split(".").pop()?.toLowerCase() ?? "";
}

function baseOf(filename: string): string {
  return filename.includes(".") ? filename.replace(/\.[^.]+$/, "") : filename;
}

// ── Renderer ──────────────────────────────────────────────────────────────────

export function renderTextToFileFields(ctx: ExtensionContext): void {
  const { node, body, onChange } = ctx;
  const cfg = node.data.config as Record<string, unknown>;

  // Hide raw generic fields — replaced by the richer UI below.
  body.querySelectorAll<HTMLElement>(".field-group").forEach(fg => {
    const label = fg.querySelector(".field-label")?.textContent?.toLowerCase() ?? "";
    if (label === "filename" || label === "mime type" || label === "format") fg.style.display = "none";
  });

  body.appendChild(mkSection("Output Format"));

  const currentFilename = (cfg["filename"] as string | undefined) ?? "output.txt";
  const currentExt      = extOf(currentFilename);
  const presetMatch     = FORMATS.find(f => f.ext === currentExt);
  const currentLabel    = presetMatch?.label ?? FORMATS[0].label;

  // filenameInp is assigned synchronously by mkField before any event handler fires.
  let filenameInp!: HTMLInputElement;

  // ── Filename input (always visible) ───────────────────────────────────────
  const filenameRow = mkField("File Name", () => {
    const inp = mk<HTMLInputElement>("input");
    inp.type         = "text";
    inp.value        = currentFilename;
    inp.placeholder  = "output.pdf";
    inp.autocomplete = "off";
    inp.addEventListener("input", () => {
      cfg["filename"] = inp.value;
      // Sync format and mime_type when the user types a known extension.
      const typedExt    = extOf(inp.value);
      const matchedFmt  = FORMATS.find(f => f.ext === typedExt);
      if (matchedFmt) {
        cfg["format"]    = matchedFmt.ext;
        cfg["mime_type"] = matchedFmt.mime;
      } else {
        delete cfg["format"];
      }
      onChange();
    });
    filenameInp = inp;
    return inp;
  }, "Name of the saved file — e.g. report.pdf. The Format picker updates the extension automatically.");
  body.appendChild(filenameRow);

  // ── Format picker (custom dropdown — avoids WebKitGTK inline-select bug) ──
  const formatField = mkField("Format", () => {
    return mkCustomSelect(
      FORMATS.map(f => f.label),
      currentLabel,
      (label) => {
        const chosen = FORMATS.find(f => f.label === label);
        if (!chosen) return;
        const base        = baseOf(filenameInp.value || "output") || "output";
        cfg["filename"]   = `${base}.${chosen.ext}`;
        cfg["mime_type"]  = chosen.mime;
        cfg["format"]     = chosen.ext;
        filenameInp.value = cfg["filename"] as string;
        onChange();
      },
    );
  }, "Sets the file extension and MIME type. Edit File Name above to change the base name.");
  body.appendChild(formatField);

  // ── MIME override ─────────────────────────────────────────────────────────
  body.appendChild(mkField("MIME Type", () => {
    const inp = mk<HTMLInputElement>("input");
    inp.type         = "text";
    inp.value        = (cfg["mime_type"] as string | undefined) ?? "text/plain";
    inp.placeholder  = "text/plain";
    inp.autocomplete = "off";
    inp.addEventListener("input", () => { cfg["mime_type"] = inp.value; onChange(); });
    return inp;
  }, "Auto-set by Format picker. Override only if needed."));
}
