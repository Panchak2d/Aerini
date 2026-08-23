import { mk, mkSection, type ExtensionContext } from "../../node-configs/popover-utils";

/**
 * Generic raw-JSON editor for a plugin node's full config object. Plugin
 * input_schema properties can be object- or array-typed (unlike every
 * built-in node's flat schema), and field-renderer.ts's generic loop has no
 * renderer for either — this is the fallback that lets a plugin author's
 * nested config still be edited, and the only config surface at all for a
 * trigger-only plugin with zero declared schema properties.
 *
 * Edits commit on blur, not on every keystroke, so in-progress/invalid
 * typing is never destroyed mid-edit.
 */
export function renderPluginRawConfigEditor(ctx: ExtensionContext): void {
  const { node, body, onChange, rerender, hasConfigSection } = ctx;

  if (!hasConfigSection) {
    body.appendChild(mkSection("Configuration"));
  } else {
    body.appendChild(mkSection("Advanced: full config (JSON)"));
    const hint = document.createElement("div");
    hint.className = "config-hint";
    hint.textContent = "Edits the full config object directly, including fields not shown above.";
    body.appendChild(hint);
  }

  const wrap = document.createElement("div");
  wrap.className = "field-multiline-wrap";

  const ta = mk<HTMLTextAreaElement>("textarea");
  ta.className = "code-editor";
  ta.rows = 10;
  ta.spellcheck = false;
  ta.autocomplete = "off";
  ta.value = JSON.stringify(node.data.config, null, 2);

  const error = document.createElement("div");
  error.className = "field-hint field-hint--warn";
  error.style.display = "none";
  error.setAttribute("role", "alert");

  ta.addEventListener("keydown", (e) => {
    if (e.key === "Tab") {
      e.preventDefault();
      const s = ta.selectionStart, en = ta.selectionEnd;
      ta.value = ta.value.slice(0, s) + "  " + ta.value.slice(en);
      ta.selectionStart = ta.selectionEnd = s + 2;
    }
  });

  ta.addEventListener("blur", () => {
    let parsed: unknown;
    try {
      parsed = JSON.parse(ta.value);
    } catch {
      error.textContent = "Invalid JSON — edits not applied.";
      error.style.display = "block";
      return;
    }
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
      error.textContent = "Must be a JSON object — edits not applied.";
      error.style.display = "block";
      return;
    }
    error.style.display = "none";

    // Replace contents in place (not `node.data.config = parsed`) so any
    // existing reference to the same object (e.g. a dynamic-ports node's
    // derivePorts closure) sees the update without needing its own resync.
    const config = node.data.config as Record<string, unknown>;
    for (const k of Object.keys(config)) delete config[k];
    Object.assign(config, parsed as Record<string, unknown>);

    onChange();
    rerender();
  });

  wrap.appendChild(ta);
  body.appendChild(wrap);
  body.appendChild(error);
}
