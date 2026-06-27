/**
 * Expression picker — floating panel that shows predecessor nodes and their
 * output schema fields. Clicking a field inserts {{node_name.output.field}}
 * at the cursor position of the target input or textarea.
 *
 * Usage:
 *   showExpressionPicker(anchorEl, targetInput, currentNodeId, canvasEl)
 *   closeExpressionPicker()
 */

let _pickerEl: HTMLElement | null = null;
let _pickerId = 0;

interface CanvasInternal {
  connectors: Map<string, { data: { to_node: string; from_node: string } }>;
  nodes: Map<string, { data: { id: string; name: string; output_schema?: Record<string, unknown> } }>;
}

export function closeExpressionPicker(): void {
  if (_pickerEl) {
    _pickerEl.remove();
    _pickerEl = null;
  }
}

export function showExpressionPicker(
  anchorEl: HTMLElement,
  targetInput: HTMLInputElement | HTMLTextAreaElement,
  currentNodeId: string,
  canvasEl: HTMLCanvasElement,
): void {
  closeExpressionPicker();
  const myId = ++_pickerId;

  const canvas = (canvasEl as unknown as { __canvas: CanvasInternal }).__canvas;

  if (!canvas) return;

  // BFS walk backwards through connector graph to collect all ancestors
  const predecessorIds = new Set<string>();
  const queue: string[] = [currentNodeId];
  while (queue.length) {
    const current = queue.shift()!;
    for (const conn of canvas.connectors.values()) {
      if (conn.data.to_node === current && !predecessorIds.has(conn.data.from_node)) {
        predecessorIds.add(conn.data.from_node);
        queue.push(conn.data.from_node);
      }
    }
  }

  const predecessors: Array<{ id: string; name: string; schema: Record<string, unknown> }> = [];
  for (const id of predecessorIds) {
    const node = canvas.nodes.get(id);
    if (node) {
      predecessors.push({
        id: node.data.id,
        name: node.data.name,
        schema: node.data.output_schema ?? {},
      });
    }
  }

  const picker = document.createElement("div");
  picker.className = "expr-picker";
  picker.setAttribute("role", "dialog");
  picker.setAttribute("aria-label", "Expression picker");

  const header = document.createElement("div");
  header.className = "expr-picker-header";
  header.textContent = "Insert expression";
  picker.appendChild(header);

  const body = document.createElement("div");
  body.className = "expr-picker-body";

  const runSection = mkSection("Run variables");
  body.appendChild(runSection);

  const runVars = [
    { expr: "$run.id",            desc: "Execution ID" },
    { expr: "$run.timestamp",     desc: "ISO timestamp of this run" },
    { expr: "$run.workflow_name", desc: "Workflow name" },
  ];
  for (const v of runVars) {
    body.appendChild(mkItem(`{{${v.expr}}}`, v.desc, () => {
      insertAtCursor(targetInput, `{{${v.expr}}}`);
      closeExpressionPicker();
    }));
  }

  body.appendChild(mkSection("Functions"));

  const FUNCTIONS: Array<{ sig: string; desc: string; template: string }> = [
    // String
    { sig: "upper(string)",               desc: "Uppercase a string",                         template: "{{upper(|)}}" },
    { sig: "lower(string)",               desc: "Lowercase a string",                         template: "{{lower(|)}}" },
    { sig: "trim(string)",                desc: "Strip leading and trailing whitespace",        template: "{{trim(|)}}" },
    { sig: "trim_start(string)",          desc: "Strip leading whitespace",                    template: "{{trim_start(|)}}" },
    { sig: "trim_end(string)",            desc: "Strip trailing whitespace",                   template: "{{trim_end(|)}}" },
    { sig: "len(string|array)",           desc: "Character or element count",                  template: "{{len(|)}}" },
    { sig: "slice(string, start, end)",   desc: "Substring by character index (0-based)",      template: "{{slice(|, 0, 10)}}" },
    { sig: "replace(string, from, to)",   desc: "Replace first occurrence of a substring",     template: "{{replace(|, \"old\", \"new\")}}" },
    { sig: "replace_all(string, from, to)", desc: "Replace all occurrences of a substring",   template: "{{replace_all(|, \"old\", \"new\")}}" },
    { sig: "contains(string, substr)",    desc: "True if string contains substr",              template: "{{contains(|, \"text\")}}" },
    { sig: "starts_with(string, prefix)", desc: "True if string starts with prefix",           template: "{{starts_with(|, \"prefix\")}}" },
    { sig: "ends_with(string, suffix)",   desc: "True if string ends with suffix",             template: "{{ends_with(|, \"suffix\")}}" },
    { sig: "split(string, delimiter)",    desc: "Split into an array",                         template: "{{split(|, \",\")}}" },
    { sig: "join(array, delimiter)",      desc: "Join array elements into a string",           template: "{{join(|, \",\")}}" },
    { sig: "pad_start(string, len, char)", desc: "Left-pad string to length",                 template: "{{pad_start(|, 10, \"0\")}}" },
    { sig: "pad_end(string, len, char)",  desc: "Right-pad string to length",                  template: "{{pad_end(|, 10, \" \")}}" },
    // Number
    { sig: "round(number, decimals)",     desc: "Round to N decimal places",                  template: "{{round(|, 2)}}" },
    { sig: "floor(number)",               desc: "Round down to nearest integer",               template: "{{floor(|)}}" },
    { sig: "ceil(number)",                desc: "Round up to nearest integer",                 template: "{{ceil(|)}}" },
    { sig: "abs(number)",                 desc: "Absolute value",                              template: "{{abs(|)}}" },
    { sig: "min(a, b)",                   desc: "Smaller of two numbers",                      template: "{{min(|, 0)}}" },
    { sig: "max(a, b)",                   desc: "Larger of two numbers",                       template: "{{max(|, 0)}}" },
    { sig: "to_int(string)",              desc: "Parse string to integer",                     template: "{{to_int(|)}}" },
    { sig: "to_float(string)",            desc: "Parse string to decimal number",              template: "{{to_float(|)}}" },
    // Date
    { sig: "now()",                       desc: "Current UTC time as ISO 8601 string",         template: "{{now()}}" },
    { sig: "format_date(timestamp, fmt)", desc: "Format ISO timestamp — tokens: YYYY MM DD HH mm ss", template: "{{format_date(|, \"YYYY-MM-DD\")}}" },
    { sig: "parse_date(string)",          desc: "Parse a date string to ISO 8601",             template: "{{parse_date(|)}}" },
    { sig: "add_days(timestamp, n)",      desc: "Add N days to a timestamp",                   template: "{{add_days(|, 1)}}" },
    { sig: "add_hours(timestamp, n)",     desc: "Add N hours to a timestamp",                  template: "{{add_hours(|, 1)}}" },
    { sig: "add_minutes(timestamp, n)",   desc: "Add N minutes to a timestamp",                template: "{{add_minutes(|, 30)}}" },
    { sig: "date_diff(ts1, ts2, unit)",   desc: "Difference between timestamps — unit: days hours minutes seconds", template: "{{date_diff(|, \"\", \"days\")}}" },
    // Array / Object
    { sig: "first(array)",                desc: "First element of an array",                   template: "{{first(|)}}" },
    { sig: "last(array)",                 desc: "Last element of an array",                    template: "{{last(|)}}" },
    { sig: "nth(array, index)",           desc: "Element at 0-based index",                    template: "{{nth(|, 0)}}" },
    { sig: "keys(object)",                desc: "Array of object keys",                        template: "{{keys(|)}}" },
    { sig: "values(object)",              desc: "Array of object values",                      template: "{{values(|)}}" },
    // Conditional
    { sig: "if(condition, then, else)",   desc: "Return then if condition is truthy, else otherwise", template: "{{if(|, \"\", \"\")}}" },
  ];

  for (const func of FUNCTIONS) {
    body.appendChild(mkFunctionItem(func.sig, func.desc, func.template, targetInput));
  }

  if (predecessors.length === 0 && predecessorIds.size === 0) {
    const hint = document.createElement("div");
    hint.className = "expr-picker-hint";
    hint.textContent = "No connected predecessor nodes. Connect a node to this one to reference its output fields here.";
    body.appendChild(hint);
  }

  for (const pred of predecessors) {
    body.appendChild(mkSection(pred.name));

    // Always offer the full output reference
    body.appendChild(mkItem(
      `{{${pred.name}.output}}`,
      "Entire output (JSON)",
      () => { insertAtCursor(targetInput, `{{${pred.name}.output}}`); closeExpressionPicker(); }
    ));

    // Enumerate top-level output schema properties
    const props = getSchemaProperties(pred.schema);
    if (props.length === 0) {
      const hint = document.createElement("div");
      hint.className = "expr-picker-hint";
      hint.textContent = "No output schema fields defined for this node.";
      body.appendChild(hint);
    }
    for (const { key, type: propType, description } of props) {
      const expr = `{{${pred.name}.output.${key}}}`;
      const label = description ? `${key} — ${description}` : `${key} (${propType})`;
      body.appendChild(mkItem(expr, label, () => {
        insertAtCursor(targetInput, expr);
        closeExpressionPicker();
      }));
    }
  }

  picker.appendChild(body);
  document.body.appendChild(picker);
  _pickerEl = picker;

  // Position below the anchor element
  positionPicker(picker, anchorEl);

  // Close on outside click
  const onOutside = (e: MouseEvent) => {
    if (myId !== _pickerId) { document.removeEventListener("mousedown", onOutside, true); return; }
    if (!picker.contains(e.target as Node) && e.target !== anchorEl) {
      document.removeEventListener("mousedown", onOutside, true);
      closeExpressionPicker();
    }
  };
  setTimeout(() => {
    if (myId === _pickerId) document.addEventListener("mousedown", onOutside, true);
  }, 80);

  // Close on Escape
  const onEsc = (e: KeyboardEvent) => {
    if (e.key === "Escape" && myId === _pickerId) {
      document.removeEventListener("keydown", onEsc, true);
      closeExpressionPicker();
      targetInput.focus();
    }
  };
  document.addEventListener("keydown", onEsc, true);
}

function insertAtCursor(
  el: HTMLInputElement | HTMLTextAreaElement,
  text: string,
): void {
  const start = el.selectionStart ?? el.value.length;
  const end   = el.selectionEnd   ?? el.value.length;
  el.value = el.value.slice(0, start) + text + el.value.slice(end);
  const newCursor = start + text.length;
  el.selectionStart = newCursor;
  el.selectionEnd   = newCursor;
  // Fire input event so popover/field-renderer.ts onChange handler picks up the new value
  el.dispatchEvent(new Event("input", { bubbles: true }));
  el.focus();
}

function positionPicker(picker: HTMLElement, anchor: HTMLElement): void {
  const r = anchor.getBoundingClientRect();
  const PICKER_W = 300;
  const PICKER_MAX_H = 340;
  const MARGIN = 8;

  let left = r.left;
  let top  = r.bottom + 4;

  if (left + PICKER_W > window.innerWidth - MARGIN) {
    left = window.innerWidth - PICKER_W - MARGIN;
  }
  if (left < MARGIN) left = MARGIN;
  if (top + PICKER_MAX_H > window.innerHeight - MARGIN) {
    top = r.top - PICKER_MAX_H - 4;
  }
  if (top < MARGIN) top = MARGIN;

  picker.style.left = `${left}px`;
  picker.style.top  = `${top}px`;
}

function mkSection(title: string): HTMLElement {
  const d = document.createElement("div");
  d.className = "expr-picker-section";
  d.textContent = title;
  return d;
}

function mkItem(expr: string, label: string, onClick: () => void): HTMLElement {
  const d = document.createElement("div");
  d.className = "expr-picker-item";
  d.setAttribute("role", "button");
  d.setAttribute("tabindex", "0");

  const exprEl = document.createElement("span");
  exprEl.className = "expr-picker-expr";
  exprEl.textContent = expr;

  const labelEl = document.createElement("span");
  labelEl.className = "expr-picker-label";
  labelEl.textContent = label;

  d.appendChild(exprEl);
  d.appendChild(labelEl);

  d.addEventListener("mousedown", (e) => { e.preventDefault(); onClick(); });
  d.addEventListener("keydown", (e) => {
    if (e.key === "Enter" || e.key === " ") { e.preventDefault(); onClick(); }
  });

  return d;
}

interface SchemaProp { key: string; type: string; description: string }

/** Insert `text` at cursor and position the caret at the `|` placeholder position. */
function insertFunction(
  el: HTMLInputElement | HTMLTextAreaElement,
  template: string,
): void {
  const markerIdx = template.indexOf("|");
  const text = markerIdx >= 0 ? template.slice(0, markerIdx) + template.slice(markerIdx + 1) : template;
  const start = el.selectionStart ?? el.value.length;
  const end   = el.selectionEnd   ?? el.value.length;
  el.value = el.value.slice(0, start) + text + el.value.slice(end);
  const cursorPos = start + (markerIdx >= 0 ? markerIdx : text.length);
  el.selectionStart = cursorPos;
  el.selectionEnd   = cursorPos;
  el.dispatchEvent(new Event("input", { bubbles: true }));
  el.focus();
}

function mkFunctionItem(
  sig: string,
  desc: string,
  template: string,
  targetInput: HTMLInputElement | HTMLTextAreaElement,
): HTMLElement {
  const d = document.createElement("div");
  d.className = "expr-picker-item expr-picker-item--fn";
  d.setAttribute("role", "button");
  d.setAttribute("tabindex", "0");

  const sigEl = document.createElement("span");
  sigEl.className = "expr-picker-expr";
  sigEl.textContent = sig;

  const descEl = document.createElement("span");
  descEl.className = "expr-picker-label";
  descEl.textContent = desc;

  d.appendChild(sigEl);
  d.appendChild(descEl);

  const onClick = () => {
    insertFunction(targetInput, template);
    closeExpressionPicker();
  };
  d.addEventListener("mousedown", (e) => { e.preventDefault(); onClick(); });
  d.addEventListener("keydown", (e) => {
    if (e.key === "Enter" || e.key === " ") { e.preventDefault(); onClick(); }
  });

  return d;
}

function getSchemaProperties(schema: Record<string, unknown>): SchemaProp[] {
  const props = (schema as Record<string, unknown>)?.properties as Record<string, unknown> | undefined;
  if (!props || typeof props !== "object") return [];

  return Object.entries(props).map(([key, val]) => {
    const v = val as Record<string, unknown>;
    return {
      key,
      type:        String(v?.type ?? "any"),
      description: String(v?.description ?? ""),
    };
  });
}
