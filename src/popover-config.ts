import { invoke } from "@tauri-apps/api/core";
import { NODE_IDS } from "./node-ids";
import type { CanvasNode } from "./canvas/Node";
import { listCredentials } from "./ipc/credentials";
import { runWorkflow } from "./ipc/workflow";
import type { NodeDescriptor, WorkflowLogEntry } from "./ipc/workflow";
import { escapeHtml } from "./utils";
import { showExpressionPicker, closeExpressionPicker } from "./expression-picker";
import { showSocialSetupGuide } from "./panels/SocialSetupGuide";

// Registry of node descriptors — populated by app.ts via setDescriptorRegistry()
// Used to recover field schemas when a saved node has empty input_schema.properties
let _descriptorRegistry: Map<string, NodeDescriptor> = new Map();
export function setDescriptorRegistry(nodes: NodeDescriptor[]): void {
  _descriptorRegistry = new Map(nodes.map(d => [d.type_id, d]));
}

const CREDENTIAL_KEYS = new Set(["api_key", "password"]);

const CRON_PRESETS = [
  { label: "Every minute",       value: "* * * * *" },
  { label: "Every hour",         value: "0 * * * *" },
  { label: "Daily at 9 am",      value: "0 9 * * *" },
  { label: "Weekdays at 9 am",   value: "0 9 * * 1-5" },
  { label: "Weekly (Mon 9 am)",  value: "0 9 * * 1" },
  { label: "Monthly (1st 9 am)", value: "0 9 1 * *" },
];

let _activePopover: HTMLElement | null = null;
let _activeNode: CanvasNode | null = null;
// Unique ID per popover instance — prevents old onOutside handlers from
// closing a newly-opened popover when rapidly switching between nodes.
let _activePopoverId = 0;

export function closePopover(): void {
  if (_activePopover) {
    _activePopoverId++;           // invalidate any pending onOutside timers
    _activePopover.remove();
    _activePopover = null;
    _activeNode = null;
    closeExpressionPicker();
  }
}

export async function showPopover(
  node: CanvasNode,
  canvasEl: HTMLCanvasElement,
  onChangeFn: () => void,
): Promise<void> {
  // Close any existing popover immediately (no animation — prevents race conditions)
  closePopover();
  _activeNode = node;

  const myId = ++_activePopoverId; // snapshot this popover's ID

  // Wrap the caller's onChange so dynamic-port nodes re-derive their port list
  // whenever any config field changes, keeping the canvas port layout in sync.
  const onChange = () => {
    if (node.data.dynamic_ports) {
      node.derivePorts(node.data.config as Record<string, unknown>);
      node.rebuildPorts();
    }
    onChangeFn();
  };

  const creds = await listCredentials().catch(() => []);

  // If another popover opened while we were awaiting credentials, abort
  if (myId !== _activePopoverId) return;

  // Parse schema — fall back to the ALL_NODES descriptor registry if the saved
  // node has an empty input_schema (happens when loaded from a .flowo file that
  // was saved before the serializer included the full schema).
  const schema  = node.data.input_schema as Record<string, unknown>;
  let rawProps = (schema?.properties ?? {}) as Record<string, unknown>;
  if (!Object.keys(rawProps).length) {
    // Fall back to the descriptor registry schema for this node type
    const desc = _descriptorRegistry.get(node.data.node_type_id);
    if (desc) {
      const ds = desc.input_schema as Record<string, unknown>;
      rawProps = (ds?.properties ?? {}) as Record<string, unknown>;
    }
  }
  const props   = rawProps as Record<string, { type?: string; description?: string; enum?: string[]; minimum?: number; maximum?: number; }>;
  // Keys managed by custom UI blocks — excluded from generic field rendering.
  const CUSTOM_UI_KEYS = new Set(["subfolders", "sources", "files", "folder_path", "overwrite"]);
  const cfgKeys = Object.entries(props).filter(([k]) => !CREDENTIAL_KEYS.has(k) && !CUSTOM_UI_KEYS.has(k));

  const pop = document.createElement("div");
  pop.className = "node-popover"; pop.id = "node-popover";

  // Header — name + node type subtitle
  const header = document.createElement("div");
  header.className = "popover-header";
  const headerText = document.createElement("div");
  headerText.className = "popover-header-text";
  const titleEl = document.createElement("div");
  titleEl.className = "popover-title"; titleEl.textContent = node.data.name;
  const subtitleEl = document.createElement("div");
  subtitleEl.className = "popover-subtitle"; subtitleEl.textContent = node.data.node_type_id.replace(/_/g, " ").replace(/\b\w/g, c => c.toUpperCase());
  headerText.appendChild(titleEl); headerText.appendChild(subtitleEl);
  const closeBtn = document.createElement("button");
  closeBtn.className = "popover-close";
  closeBtn.innerHTML = `<svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>`;
  closeBtn.addEventListener("click", closePopover);

  const testBtn = document.createElement("button");
  testBtn.className = "popover-test-btn";
  testBtn.title = "Test this node in isolation";
  testBtn.innerHTML = `<svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polygon points="5 3 19 12 5 21 5 3"/></svg> Test`;
  testBtn.addEventListener("click", async () => {
    testBtn.disabled = true;
    testBtn.textContent = "Running…";
    try {
      await testSingleNode(node, onChange);
    } finally {
      testBtn.disabled = false;
      testBtn.innerHTML = `<svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polygon points="5 3 19 12 5 21 5 3"/></svg> Test`;
    }
  });

  header.appendChild(headerText);
  header.appendChild(testBtn);
  header.appendChild(closeBtn);
  pop.appendChild(header);

  // Body
  const body = document.createElement("div");
  body.className = "popover-body";

  // Search — only when ≥4 config fields
  if (cfgKeys.length >= 4) {
    const wrap = document.createElement("div");
    wrap.className = "popover-search-wrap";
    const si = document.createElement("input") as HTMLInputElement;
    si.type = "text"; si.placeholder = "Search fields…"; si.className = "popover-search";
    si.addEventListener("input", () => {
      const q = si.value.toLowerCase();
      body.querySelectorAll<HTMLElement>(".field-group").forEach(fg => {
        const lbl = fg.querySelector(".field-label")?.textContent?.toLowerCase() ?? "";
        fg.style.display = q && !lbl.includes(q) ? "none" : "";
      });
    });
    wrap.appendChild(si); body.appendChild(wrap);
  }

  // Node name
  body.appendChild(mkSection("Node"));
  body.appendChild(mkField("Name", () => {
    const inp = mk<HTMLInputElement>("input");
    inp.type = "text"; inp.value = node.data.name; inp.autocomplete = "off"; inp.spellcheck = false;
    inp.addEventListener("input", () => { node.data.name = inp.value; titleEl.textContent = inp.value; onChange(); });
    return inp;
  }));

  // Config fields
  if (cfgKeys.length > 0) {
    body.appendChild(mkSection("Configuration"));

    // AI cost warning for scheduled AI nodes
    const isScheduled = node.data.node_type_id === NODE_IDS.SCHEDULE;
    const isAiNode    = node.data.node_type_id === NODE_IDS.AI_PROMPT || node.data.node_type_id === NODE_IDS.AI_AGENT;
    if (isAiNode) {
      const warn = document.createElement("div");
      warn.className = "config-hint config-hint-warn";
      warn.textContent = "Note: Each execution of this node will consume API credits. Set a sensible schedule interval to avoid unexpected costs.";
      body.appendChild(warn);
    }

      // Render a custom mode selector + conditionally show only the relevant
      // config fields. Without this, the backend receives an empty "mode" and
      // fails with "mode is required".
    if (isScheduled) {
      // Ensure a default mode is always set so the node is runnable immediately
      if (!node.data.config["mode"]) {
        node.data.config["mode"] = "interval";
        onChange();
      }

      const schedModes = [
        { value: "interval", label: "Interval (seconds)" },
        { value: "cron",     label: "Cron Expression" },
        { value: "once",     label: "Run Once" },
      ];

      // Mode selector
      body.appendChild(mkField("Mode", () => {
        return mkCustomSelect(
          schedModes.map(m => m.label),
          schedModes.find(m => m.value === node.data.config["mode"])?.label ?? "Interval (seconds)",
          (label) => {
            const opt = schedModes.find(m => m.label === label);
            if (opt) {
              node.data.config["mode"] = opt.value;
              onChange();
              // Re-render popover so conditional fields update
              showPopover(node, canvasEl, onChange);
            }
          }
        );
      }, "When to trigger this workflow"));

      const currentMode = node.data.config["mode"] as string;

      // Interval field — only when mode = interval
      if (currentMode === "interval") {
        body.appendChild(mkField("Interval (seconds)", () => {
          const inp = mk<HTMLInputElement>("input");
          inp.type = "number"; inp.min = "10";
          inp.autocomplete = "off";
          inp.value = String(node.data.config["interval_secs"] ?? 60);
          inp.placeholder = "Seconds between runs";

          const warning = document.createElement("div");
          warning.className = "field-hint field-hint--warn";
          warning.style.display = "none";
          warning.textContent = "Minimum is 10 seconds — value reset to 10.";

          inp.addEventListener("input", () => {
            node.data.config["interval_secs"] = Number(inp.value);
            onChange();
          });
          inp.addEventListener("blur", () => {
            const v = Number(inp.value);
            if (!Number.isNaN(v) && v < 10) {
              inp.value = "10";
              node.data.config["interval_secs"] = 10;
              onChange();
              warning.style.display = "block";
              setTimeout(() => { warning.style.display = "none"; }, 3000);
            }
          });

          const wrap = document.createElement("div");
          wrap.appendChild(inp);
          wrap.appendChild(warning);
          return wrap;
        }, "Seconds between runs (minimum 10)"));
      }

      // Cron fields — only when mode = cron
      if (currentMode === "cron") {
        body.appendChild(mkField("Cron Expression", () => {
          const wrap = document.createElement("div");
          wrap.className = "cron-wrap";

          const presetsRow = document.createElement("div");
          presetsRow.className = "cron-presets";
          CRON_PRESETS.forEach(p => {
            const btn = document.createElement("button");
            btn.type = "button"; btn.className = "cron-preset-btn";
            btn.textContent = p.label; btn.title = p.value;
            btn.addEventListener("click", () => {
              inp.value = p.value;
              node.data.config["cron_expr"] = p.value;
              onChange();
            });
            presetsRow.appendChild(btn);
          });

          const inp = mk<HTMLInputElement>("input");
          inp.value = String(node.data.config["cron_expr"] ?? "");
          inp.placeholder = "e.g. 0 9 * * 1-5";
          inp.autocomplete = "off";
          inp.spellcheck = false;
          inp.addEventListener("input", () => { node.data.config["cron_expr"] = inp.value; onChange(); });

          const hint = document.createElement("div");
          hint.className = "cron-hint";
          hint.textContent = "min hour day month weekday (0=Sun)";

          wrap.appendChild(presetsRow);
          wrap.appendChild(inp);
          wrap.appendChild(hint);
          return wrap;
        }, "Cron expression e.g. 0 9 * * 1-5 (weekdays at 9am)"));
      }

      // Run-at field — only when mode = once
      if (currentMode === "once") {
        body.appendChild(mkField("Run At", () => {
          const inp = mk<HTMLInputElement>("input");
          inp.type = "text";
          inp.autocomplete = "off";
          inp.spellcheck = false;
          inp.placeholder = "ISO timestamp e.g. 2025-12-31T09:00:00Z";
          inp.addEventListener("input", () => { node.data.config["run_at"] = inp.value; onChange(); });
          return inp;
        }, "ISO 8601 timestamp — when to fire once"));
      }

      // Skip generic cfgKeys rendering for schedule — we handled it above
      // Fall through to credentials/connection section
    } else {

    for (const [key, prop] of cfgKeys) {
      const cur = String(node.data.config[key] ?? "");
      body.appendChild(mkField(formatLabel(key), () => {
        // Cron expression — show preset picker above the input
        if (key === "cron_expr") {
          const wrap = document.createElement("div");
          wrap.className = "cron-wrap";

          const presetsRow = document.createElement("div");
          presetsRow.className = "cron-presets";
          CRON_PRESETS.forEach(p => {
            const btn = document.createElement("button");
            btn.type = "button";
            btn.className = "cron-preset-btn";
            btn.textContent = p.label;
            btn.title = p.value;
            btn.addEventListener("click", () => {
              inp.value = p.value;
              node.data.config[key] = p.value;
              onChange();
            });
            presetsRow.appendChild(btn);
          });

          const inp = mk<HTMLInputElement>("input");
          inp.value = cur;
          inp.placeholder = "e.g. 0 9 * * 1-5";
          inp.addEventListener("input", () => { node.data.config[key] = inp.value; onChange(); });

          const hint = document.createElement("div");
          hint.className = "cron-hint";
          hint.textContent = "min hour day month weekday (0=Sun)";

          wrap.appendChild(presetsRow);
          wrap.appendChild(inp);
          wrap.appendChild(hint);
          return wrap;
        }

        if (prop.enum) {
          return mkCustomSelect(prop.enum, cur, (v) => { node.data.config[key] = v; onChange(); });
        }
        const isMultiline = ["body","command","prompt","system","condition",
          "mock_payload","cases","mappings","content","code","goal","tools","context"].includes(key);
        if (isMultiline) {
          const wrap = document.createElement("div");
          wrap.className = "field-multiline-wrap";

          const ta = mk<HTMLTextAreaElement>("textarea");
          ta.value = cur;
          ta.rows = key === "code" ? 12 : 3;
          ta.placeholder = prop.description ?? "";
          if (key === "code") ta.className = "code-editor";
          ta.addEventListener("input", () => { node.data.config[key] = ta.value; onChange(); });
          ta.addEventListener("keydown", (e) => {
            if (e.key === "Tab") {
              e.preventDefault();
              const s = ta.selectionStart, en = ta.selectionEnd;
              ta.value = ta.value.slice(0, s) + "  " + ta.value.slice(en);
              ta.selectionStart = ta.selectionEnd = s + 2;
              node.data.config[key] = ta.value; onChange();
            }
          });
          wrap.appendChild(ta);

          // Skip expression button on code fields — code uses raw JS, not {{}} syntax
          if (key !== "code") {
            const exprBtn = document.createElement("button");
            exprBtn.type = "button";
            exprBtn.className = "field-interp-hint";
            exprBtn.title = "Insert expression from a previous node\nExample: {{HTTP Request.output.body.name}}";
            exprBtn.textContent = "{{ }}";
            exprBtn.addEventListener("click", (e) => {
              e.stopPropagation();
              showExpressionPicker(exprBtn, ta, node.data.id, canvasEl);
            });
            wrap.appendChild(exprBtn);
          } else {
            const hint = document.createElement("div");
            hint.className = "field-interp-hint";
            hint.title = "Type {{ to insert data from another node\nExample: {{node_id.output}}";
            hint.textContent = "{{ }}";
            wrap.appendChild(hint);
          }

          if (key === "code") {
            const loadBtn = document.createElement("button");
            loadBtn.type = "button";
            loadBtn.className = "code-load-btn";
            loadBtn.textContent = "Load .js file";
            loadBtn.addEventListener("click", () => {
              const inp = document.createElement("input");
              inp.type = "file"; inp.accept = ".js,.ts,.txt";
              inp.addEventListener("change", () => {
                const file = inp.files?.[0]; if (!file) return;
                const reader = new FileReader();
                reader.onload = () => {
                  ta.value = String(reader.result ?? "");
                  node.data.config[key] = ta.value; onChange();
                };
                reader.readAsText(file);
              });
              inp.click();
            });
            wrap.appendChild(loadBtn);
          }

          return wrap;
        }
        // Render number fields with type=number so the browser provides
        // a numeric stepper and the stored value is a JS number, not a string.
        if (prop.type === "number") {
          const inp = mk<HTMLInputElement>("input");
          inp.type = "number"; inp.autocomplete = "off";
          inp.value = String(node.data.config[key] ?? "");
          inp.placeholder = prop.description ?? "";
          if (prop.minimum !== undefined) inp.min = String(prop.minimum);
          if (prop.maximum !== undefined) inp.max = String(prop.maximum);
          inp.addEventListener("input", () => {
            const v = parseFloat(inp.value);
            if (Number.isNaN(v)) {
              // Field cleared — remove key rather than storing "" which would
              // be passed as a string to Rust and fail deserialization.
              delete node.data.config[key];
            } else {
              node.data.config[key] = v;
            }
            onChange();
          });
          return inp;
        }
        const inp = mk<HTMLInputElement>("input");
        inp.type = "text"; inp.value = cur; inp.placeholder = prop.description ?? "";
        inp.autocomplete = "off"; inp.spellcheck = false;
        inp.addEventListener("input", () => { node.data.config[key] = inp.value; onChange(); });

        // Only add the expression button for string-typed fields.
        // Number and enum fields don't accept {{}} expressions.
        if (!prop.enum && prop.type !== "number") {
          const wrap = document.createElement("div");
          wrap.className = "field-string-wrap";
          const exprBtn = document.createElement("button");
          exprBtn.type = "button";
          exprBtn.className = "field-interp-hint field-interp-hint--inline";
          exprBtn.title = "Insert expression from a previous node\nExample: {{HTTP Request.output.body.name}}";
          exprBtn.textContent = "{{ }}";
          exprBtn.addEventListener("click", (e) => {
            e.stopPropagation();
            showExpressionPicker(exprBtn, inp, node.data.id, canvasEl);
          });
          wrap.appendChild(inp);
          wrap.appendChild(exprBtn);
          return wrap;
        }
        return inp;
      }, prop.description));
    }
    } // end else (non-schedule nodes)
  } // end if (cfgKeys.length > 0)

  // ── Custom UI: save_to_folder ─────────────────────────────────────────────
  if (node.data.node_type_id === "save_to_folder") {
    body.appendChild(mkSection("Folder"));

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
      type SfSlot = { id: string; name: string; source_expr?: string };
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
      type SfSlot = { id: string; name: string; source_expr: string };
      (node.data.config["subfolders"] as SfSlot[]).push({
        id: `sf_${Date.now()}`,
        name: "New Subfolder",
        source_expr: "",
      });
      renderSfList();
      onChange();
    });
    body.appendChild(addSfBtn);
  }

  // ── Custom UI: collect_files ──────────────────────────────────────────────
  if (node.data.node_type_id === "collect_files") {
    body.appendChild(mkSection("Sources"));

    const srcHint = document.createElement("div");
    srcHint.className = "config-hint";
    srcHint.textContent = "Each source becomes an input port. Connect an upstream node to each port.";
    body.appendChild(srcHint);

    const srcListEl = document.createElement("div");
    srcListEl.className = "subfolder-list";

    const renderSrcList = () => {
      srcListEl.innerHTML = "";
      type SrcSlot = { id: string; name: string; source_expr?: string };
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
      type SrcSlot = { id: string; name: string; source_expr: string };
      const srcs = node.data.config["sources"] as SrcSlot[];
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

  // ── Custom UI: social_upload ──────────────────────────────────────────────
  if (node.data.node_type_id === "social_upload") {
    body.appendChild(mkSection("Platform Setup"));

    const guideHint = document.createElement("div");
    guideHint.className = "config-hint";
    guideHint.textContent = "Need OAuth credentials? The setup guide walks through each platform step by step.";
    body.appendChild(guideHint);

    const guideBtn = document.createElement("button");
    guideBtn.type = "button";
    guideBtn.className = "subfolder-add-btn";
    guideBtn.textContent = "Open Setup Guide";
    guideBtn.addEventListener("click", () => {
      const platform = (node.data.config["platform"] as string | undefined) ?? "youtube";
      const validPlatforms = ["youtube", "instagram", "tiktok"] as const;
      const p = validPlatforms.includes(platform as typeof validPlatforms[number])
        ? (platform as typeof validPlatforms[number])
        : "youtube";
      showSocialSetupGuide(p);
    });
    body.appendChild(guideBtn);
  }

  // Credentials
  const hasCredField = Object.keys(props).some(k => CREDENTIAL_KEYS.has(k))
    || Object.keys(node.data.credentials).length > 0
    || node.data.node_type_id === NODE_IDS.HTTP_REQUEST;
  if (hasCredField) {
    const credKey = props["api_key"] !== undefined ? "api_key"
      : props["password"] !== undefined ? "password"
      : "api_key"; // default for http_request
    body.appendChild(mkSection("Connection"));

    // Auth mode selector for HTTP nodes
    if (node.data.node_type_id === NODE_IDS.HTTP_REQUEST) {
      body.appendChild(mkField("Auth Mode", () => {
        const modes = [
          { value: "none",           label: "None" },
          { value: "bearer",         label: "Bearer Token" },
          { value: "api_key_header", label: "API Key Header" },
          { value: "basic",          label: "Basic Auth" },
        ];
        const cur = (node.data.config["auth_mode"] as string) ?? "bearer";
        return mkCustomSelect(
          modes.map(m => m.label),
          modes.find(m => m.value === cur)?.label ?? "Bearer Token",
          (label) => {
            const opt = modes.find(m => m.label === label);
            if (opt) { node.data.config["auth_mode"] = opt.value; onChange(); }
          }
        );
      }, "How the credential is attached to the request"));
    }

    const hint = document.createElement("div");
    hint.className = "config-hint";
    hint.textContent = "Select a saved Connection to attach credentials to this node.";
    body.appendChild(hint);
    body.appendChild(mkField("Use Connection", () => {
      const options = [{ value: "", label: "— none —" }, ...creds.map(c => ({ value: c.id, label: c.name }))];
      const cur = node.data.credentials[credKey] ?? "";
      return mkCustomSelect(options.map(o => o.label), options.find(o => o.value === cur)?.label ?? "— none —", (label) => {
        const opt = options.find(o => o.label === label);
        if (opt?.value) node.data.credentials[credKey] = opt.value;
        else delete node.data.credentials[credKey];
        onChange();
      });
    }));
    if (!creds.length) {
      const warn = document.createElement("div");
      warn.className = "config-hint config-hint-warn";
      warn.textContent = "No connections saved. Click Connections in the toolbar.";
      body.appendChild(warn);
    }
  }

  // Reliability
  body.appendChild(mkSection("Reliability"));
  body.appendChild(mkField("Retry attempts", () => {
    const inp = mk<HTMLInputElement>("input");
    inp.type = "number"; inp.min = "1"; inp.max = "10"; inp.autocomplete = "off";
    inp.value = String(node.data.retry.max_attempts);
    inp.addEventListener("input", () => {
      const v = parseInt(inp.value);
      if (!isNaN(v) && v >= 1) { node.data.retry.max_attempts = v; onChange(); }
    });
    return inp;
  }, "1 = no retry"));

  // Webhook — inform user of one-shot semantics and replay-attack limitation
  if (node.data.node_type_id === NODE_IDS.WEBHOOK) {
    const webhookNote = document.createElement("div");
    webhookNote.className = "popover-info-banner";
    webhookNote.innerHTML = `<strong>Run Now</strong> waits for one incoming request, then stops. For a persistent listener, use <strong>Schedule Run</strong>.`;
    body.appendChild(webhookNote);

    const replayNote = document.createElement("div");
    replayNote.className = "popover-info-banner popover-info-banner--warn";
    replayNote.innerHTML =
      `<strong>Secret replay risk:</strong> The secret field authenticates the caller but does not sign the request body. ` +
      `A captured valid request can be replayed verbatim. For integrations that send HMAC body signatures ` +
      `(Stripe, GitHub, etc.), verify the platform signature header in a downstream <strong>Code</strong> node instead of relying on this field alone.`;
    body.appendChild(replayNote);

    const tunnelNote = document.createElement("div");
    tunnelNote.className = "popover-info-banner";
    tunnelNote.innerHTML =
      `<strong>ℹ Localhost only:</strong> The webhook binds to localhost. To receive requests from Stripe, GitHub, or other external services, ` +
      `you need a public URL — use <a href="https://developers.cloudflare.com/cloudflare-one/connections/connect-apps/install-and-setup/" target="_blank">cloudflared</a> ` +
      `or <a href="https://ngrok.com" target="_blank">ngrok</a>. ` +
      `See <strong>docs/webhooks-public.md</strong> for setup instructions.`;
    body.appendChild(tunnelNote);
  }

  pop.appendChild(body);
  document.body.appendChild(pop);
  _activePopover = pop;

  positionPopover(pop, node, canvasEl);

  // Close on outside click — only if this popover is still the active one
  const onOutside = (e: MouseEvent) => {
    if (myId !== _activePopoverId) {
      document.removeEventListener("mousedown", onOutside, true);
      return;
    }
    if (!pop.contains(e.target as Node)) {
      document.removeEventListener("mousedown", onOutside, true);
      closePopover();
    }
  };

  // Close on Esc
  const onEsc = (e: KeyboardEvent) => {
    if (e.key === "Escape" && myId === _activePopoverId) {
      document.removeEventListener("keydown", onEsc, true);
      closePopover();
    }
  };
  document.addEventListener("keydown", onEsc, true);

  // Delay slightly so the triggering dblclick doesn't immediately close the popover
  setTimeout(() => {
    if (myId === _activePopoverId) {
      document.addEventListener("mousedown", onOutside, true);
    }
  }, 120);
}

// ── Custom dropdown (replaces <select>) ───────────────────────────────────────
// WebKitGTK on Linux renders <select> inside position:fixed elements inline
// (shows all options as a list) rather than as a native popup. This custom
// implementation avoids that entirely.

function mkCustomSelect(
  options: string[],
  current: string,
  onChange: (value: string) => void,
): HTMLElement {
  const wrap = document.createElement("div");
  wrap.className = "csel-wrap";

  const trigger = document.createElement("button");
  trigger.type = "button";
  trigger.className = "csel-trigger";

  const label = document.createElement("span");
  label.className = "csel-label";
  label.textContent = current || options[0] || "";

  const arrow = document.createElement("span");
  arrow.className = "csel-arrow";
  arrow.innerHTML = `<svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><polyline points="6 9 12 15 18 9"/></svg>`;

  trigger.appendChild(label);
  trigger.appendChild(arrow);
  wrap.appendChild(trigger);

  const dropdown = document.createElement("div");
  dropdown.className = "csel-dropdown hidden";

  for (const opt of options) {
    const item = document.createElement("div");
    item.className = "csel-option";
    if (opt === current) item.classList.add("selected");
    item.textContent = opt;
    item.addEventListener("mousedown", (e) => {
      e.preventDefault();
      e.stopPropagation();
      label.textContent = opt;
      dropdown.querySelectorAll(".csel-option").forEach(el => el.classList.remove("selected"));
      item.classList.add("selected");
      onChange(opt);
      closeDropdown();
    });
    dropdown.appendChild(item);
  }

  wrap.appendChild(dropdown);

  let isOpen = false;

  const openDropdown = () => {
    if (isOpen) return;
    isOpen = true;
    dropdown.classList.remove("hidden");
    trigger.classList.add("open");
    // Position dropdown below (or above) the trigger
    const tr = trigger.getBoundingClientRect();
    const dropH = Math.min(options.length * 33 + 8, 200);
    const spaceBelow = window.innerHeight - tr.bottom - 6;
    if (spaceBelow >= dropH || spaceBelow > tr.top) {
      dropdown.style.top  = `${tr.bottom + 4}px`;
    } else {
      dropdown.style.top  = `${tr.top - dropH - 4}px`;
    }
    dropdown.style.left  = `${tr.left}px`;
    dropdown.style.width = `${tr.width}px`;
    // Close on outside click
    const onOutside = (e: MouseEvent) => {
      if (!wrap.contains(e.target as Node)) {
        closeDropdown();
        document.removeEventListener("mousedown", onOutside, true);
      }
    };
    setTimeout(() => document.addEventListener("mousedown", onOutside, true), 0);
  };

  const closeDropdown = () => {
    if (!isOpen) return;
    isOpen = false;
    dropdown.classList.add("hidden");
    trigger.classList.remove("open");
  };

  trigger.addEventListener("click", (e) => {
    e.stopPropagation();
    isOpen ? closeDropdown() : openDropdown();
  });

  // Keyboard navigation
  trigger.addEventListener("keydown", (e) => {
    if (e.key === "Enter" || e.key === " ") { e.preventDefault(); isOpen ? closeDropdown() : openDropdown(); }
    if (e.key === "Escape") closeDropdown();
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      if (!isOpen) openDropdown();
      const items = [...dropdown.querySelectorAll<HTMLElement>(".csel-option")];
      const cur   = dropdown.querySelector<HTMLElement>(".csel-option.selected");
      const idx   = cur ? items.indexOf(cur) : -1;
      const next  = e.key === "ArrowDown" ? items[idx + 1] : items[idx - 1];
      if (next) { next.classList.add("selected"); cur?.classList.remove("selected"); next.scrollIntoView({ block: "nearest" }); }
    }
  });

  return wrap;
}

function positionPopover(pop: HTMLElement, node: CanvasNode, canvasEl: HTMLCanvasElement): void {
  const canvas = (canvasEl as unknown as { __canvas?: import("./canvas/Canvas").Canvas }).__canvas;
  if (!canvas) { pop.style.left = "50%"; pop.style.top = "50%"; pop.style.transform = "translate(-50%,-50%)"; return; }
  const r = canvasEl.getBoundingClientRect();
  const nodeRightSx = (node.data.position.x + 220) * canvas.zoom + canvas.panX + r.left;
  const nodeMidSy   = (node.data.position.y + node.height / 2) * canvas.zoom + canvas.panY + r.top;
  const POP_W = 360, POP_MAX_H = 680, MARGIN = 12;
  let left = nodeRightSx + MARGIN;
  let top  = nodeMidSy - 140;
  if (left + POP_W > window.innerWidth - MARGIN) left = nodeRightSx - POP_W - MARGIN * 2;
  if (left < MARGIN) left = MARGIN;
  if (top + POP_MAX_H > window.innerHeight - MARGIN) top = window.innerHeight - POP_MAX_H - MARGIN;
  if (top < MARGIN) top = MARGIN;
  pop.style.left = `${left}px`; pop.style.top = `${top}px`; pop.style.transform = "none";
}

function mkSection(title: string): HTMLElement {
  const d = document.createElement("div"); d.className = "popover-section-title"; d.textContent = title; return d;
}
function mkField(label: string, factory: () => HTMLElement, hint?: string): HTMLElement {
  const g = document.createElement("div"); g.className = "field-group";
  const l = document.createElement("label"); l.className = "field-label"; l.textContent = label;
  g.appendChild(l); g.appendChild(factory());
  if (hint) { const h = document.createElement("div"); h.className = "field-hint"; h.textContent = hint; g.appendChild(h); }
  return g;
}
function mk<T extends HTMLElement>(tag: string): T { return document.createElement(tag) as T; }
function formatLabel(key: string): string {
  const OVERRIDES: Record<string, string> = {
    api_key: "API Key", base_url: "Base URL", cron_expr: "Cron Expression",
    rate_limit_rpm: "Rate Limit (RPM)", smtp_host: "SMTP Host", smtp_port: "SMTP Port",
    db_path: "Database Path", max_tokens: "Max Tokens", mock_payload: "Mock Payload",
    timeout_secs: "Timeout (seconds)", duration_secs: "Duration (seconds)",
    poll_interval_secs: "Poll Interval (seconds)", backoff_ms: "Backoff (ms)",
    max_attempts: "Max Attempts", source_node: "Source Node", array_field: "Array Field",
    item_var: "Item Variable", index_var: "Index Variable", chunk_size: "Chunk Size",
    input_text: "Input Text", run_at: "Run At (ISO timestamp)", interval_secs: "Interval (seconds)",
    max_messages: "Max Messages", max_iterations: "Max Iterations", session_id: "Session ID",
  };
  if (OVERRIDES[key]) return OVERRIDES[key];
  return key.replace(/_/g, " ").replace(/\b\w/g, c => c.toUpperCase())
    .replace(/\bUrl\b/g, "URL").replace(/\bId\b/g, "ID").replace(/\bJson\b/g, "JSON")
    .replace(/\bSql\b/g, "SQL").replace(/\bHtml\b/g, "HTML").replace(/\bRpm\b/g, "RPM")
    .replace(/\bSmtp\b/g, "SMTP").replace(/\bApi\b/g, "API");
}

// ── Single-node test ──────────────────────────────────────────────────────────

async function testSingleNode(node: CanvasNode, onChange: () => void): Promise<void> {
  // Wrap the node in a minimal workflow: trigger → node
  const triggerId = "test_trigger";
  const minimalWorkflow = {
    id: `test_${node.data.id}`,
    name: `Test: ${node.data.name}`,
    nodes: [
      {
        id: triggerId,
        node_type_id: NODE_IDS.MANUAL_TRIGGER,
        node_type: "action",
        name: "Test Trigger",
        config: {},
        credentials: {},
        position: { x: 0, y: 0 },
        ports: { inputs: [], outputs: [{ id: "output", label: "Start", position: "right" }] },
        input_schema: {}, output_schema: {}, retry: { max_attempts: 1, backoff_ms: 0 }, fallback_node: null,
      },
      {
        id: node.data.id,
        node_type_id: node.data.node_type_id,
        node_type: node.data.node_type ?? "action",
        name: node.data.name,
        config: node.data.config,
        credentials: node.data.credentials,
        position: { x: 300, y: 0 },
        ports: node.data.ports,
        input_schema: node.data.input_schema,
        output_schema: node.data.output_schema,
        retry: node.data.retry ?? { max_attempts: 1, backoff_ms: 0 },
        fallback_node: null,
      },
    ],
    edges: [
      { id: "e_test", from_node: triggerId, from_port: "output", to_node: node.data.id, to_port: "input" },
    ],
  };

  const workflowJson = JSON.stringify(minimalWorkflow);
  const pop = document.getElementById("node-popover");
  if (!pop) return;
  const body = pop.querySelector(".popover-body") as HTMLElement | null;
  const container = body ?? pop;

  // Remove any existing test result panel
  container.querySelector(".popover-test-result")?.remove();

  try {
    const result = await runWorkflow(workflowJson, {});
    const nodeOutput = result.node_outputs?.[node.data.id];
    const success   = result.success && nodeOutput !== undefined;
    const errors    = (result.logs ?? []).filter((l: WorkflowLogEntry) => l.level === "error");

    const panel = document.createElement("div");
    panel.className = `popover-test-result ${success ? "test-ok" : "test-fail"}`;

    if (success) {
      panel.innerHTML = `
        <div class="test-result-header">Node ran successfully</div>
        <pre class="test-result-json">${escapeHtml(JSON.stringify(nodeOutput, null, 2))}</pre>`;
    } else {
      const errMsg = errors.map((l: WorkflowLogEntry) => l.message).join("\n") || result.error || "Unknown error";
      panel.innerHTML = `
        <div class="test-result-header">Node failed</div>
        <pre class="test-result-json test-result-err">${escapeHtml(errMsg)}</pre>`;
    }

    container.appendChild(panel);
    // Scroll the popover body (not the viewport) to show the result.
    container.scrollTop = container.scrollHeight;
  } catch (e) {
    const panel = document.createElement("div");
    panel.className = "popover-test-result test-fail";
    panel.innerHTML = `<div class="test-result-header">Error</div><pre class="test-result-json test-result-err">${escapeHtml(String(e))}</pre>`;
    container.appendChild(panel);
    container.scrollTop = container.scrollHeight;
  }

  onChange();
}
