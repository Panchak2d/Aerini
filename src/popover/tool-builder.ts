import { mkCustomSelect } from "../node-configs/popover-utils";
import type { CanvasNode } from "../canvas/Node";

export const TOOL_PARAM_TYPES = ["string", "number", "integer", "boolean"] as const;
export type ToolParamType = typeof TOOL_PARAM_TYPES[number];

export interface ToolParam {
  name: string;
  type: ToolParamType;
  description: string;
  required: boolean;
  /** Allowed values; only meaningful, and only saved, for `type: "string"`. */
  enumValues: string[];
}

export interface ToolDef {
  name: string;
  description: string;
  params: ToolParam[];
}

const TOOL_NAME_PATTERN = /^[A-Za-z0-9_-]{1,64}$/;
const TOOL_KEYS = new Set(["name", "description", "parameters"]);
const PARAMETERS_KEYS = new Set(["type", "properties", "required", "additionalProperties"]);
const PROPERTY_KEYS = new Set(["type", "description", "enum"]);

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function onlyKeys(o: Record<string, unknown>, allowed: Set<string>): boolean {
  return Object.keys(o).every(k => allowed.has(k));
}

function isParamType(v: unknown): v is ToolParamType {
  return typeof v === "string" && (TOOL_PARAM_TYPES as readonly string[]).includes(v);
}

function parseEnum(v: unknown): string[] | null {
  if (!Array.isArray(v) || v.length === 0) return null;
  const seen = new Set<string>();
  for (const item of v) {
    if (typeof item !== "string" || item === "" || item !== item.trim() || item.includes("\n") || seen.has(item)) return null;
    seen.add(item);
  }
  return v as string[];
}

function parseProperty(name: string, v: unknown, required: boolean): ToolParam | null {
  if (name.trim() === "" || !isRecord(v) || !onlyKeys(v, PROPERTY_KEYS)) return null;
  const type = v["type"];
  if (!isParamType(type)) return null;
  const description = v["description"];
  if (description !== undefined && typeof description !== "string") return null;
  let enumValues: string[] = [];
  if (v["enum"] !== undefined) {
    const parsed = type === "string" ? parseEnum(v["enum"]) : null;
    if (!parsed) return null;
    enumValues = parsed;
  }
  return { name, type, description: description ?? "", required, enumValues };
}

function parseParameters(v: unknown): ToolParam[] | null {
  if (v === undefined) return [];
  if (!isRecord(v) || !onlyKeys(v, PARAMETERS_KEYS)) return null;
  if (v["type"] !== undefined && v["type"] !== "object") return null;
  if (v["additionalProperties"] !== undefined && v["additionalProperties"] !== false) return null;
  const properties = v["properties"] === undefined ? {} : v["properties"];
  if (!isRecord(properties)) return null;

  const requiredRaw = v["required"] === undefined ? [] : v["required"];
  if (!Array.isArray(requiredRaw) || requiredRaw.some(r => typeof r !== "string")) return null;
  const required = new Set<string>(requiredRaw as string[]);
  if (required.size !== requiredRaw.length) return null;
  const propertyNames = new Set(Object.keys(properties));
  for (const r of required) if (!propertyNames.has(r)) return null;

  const params: ToolParam[] = [];
  for (const [name, schema] of Object.entries(properties)) {
    const param = parseProperty(name, schema, required.has(name));
    if (!param) return null;
    params.push(param);
  }
  return params;
}

function parseTool(v: unknown): ToolDef | null {
  if (!isRecord(v) || !onlyKeys(v, TOOL_KEYS) || typeof v["name"] !== "string") return null;
  const description = v["description"];
  if (description !== undefined && typeof description !== "string") return null;
  const params = parseParameters(v["parameters"]);
  if (!params) return null;
  return { name: v["name"], description: description ?? "", params };
}

/** The builder's model of a stored Tools value, or `null` when the value uses
 *  anything the builder cannot show and write back unchanged: text that is not
 *  a JSON array (an expression, a typo), the OpenAI `{type, function}` form,
 *  `input_schema`, nested or `anyOf` parameter schemas, unknown keys, or a
 *  `required` entry naming no property. Those values must stay in the raw JSON
 *  editor; converting them would lose data. Accepts the stored string or an
 *  already-parsed array. */
export function parseTools(raw: unknown): ToolDef[] | null {
  let list: unknown = raw;
  if (raw === undefined || raw === null) return [];
  if (typeof raw === "string") {
    if (raw.trim() === "") return [];
    try { list = JSON.parse(raw); } catch { return null; }
  }
  if (!Array.isArray(list)) return null;
  const tools: ToolDef[] = [];
  for (const item of list) {
    const tool = parseTool(item);
    if (!tool) return null;
    tools.push(tool);
  }
  return tools;
}

function toolToJson(tool: ToolDef): Record<string, unknown> {
  const seen = new Set<string>();
  const entries: Array<[string, unknown]> = [];
  const required: string[] = [];
  for (const p of tool.params) {
    const name = p.name.trim();
    if (name === "" || seen.has(name)) continue;
    seen.add(name);
    const schema: Record<string, unknown> = { type: p.type };
    if (p.description.trim() !== "") schema["description"] = p.description;
    if (p.type === "string" && p.enumValues.length > 0) schema["enum"] = p.enumValues;
    entries.push([name, schema]);
    if (p.required) required.push(name);
  }
  const parameters: Record<string, unknown> = { type: "object", properties: Object.fromEntries(entries) };
  if (required.length > 0) parameters["required"] = required;
  parameters["additionalProperties"] = false;

  const out: Record<string, unknown> = { name: tool.name.trim() };
  if (tool.description.trim() !== "") out["description"] = tool.description;
  out["parameters"] = parameters;
  return out;
}

/** The Tools value for a model: pretty JSON, or "" for no tools. A parameter
 *  with no name, or a name already used by an earlier one, is left out until it
 *  is fixed, because a JSON object cannot hold it. Tools are always kept so an
 *  unfinished one is flagged here and by the node's own check before it runs. */
export function serializeTools(tools: ToolDef[]): string {
  return tools.length === 0 ? "" : JSON.stringify(tools.map(toolToJson), null, 2);
}

export function toolNameError(tools: ToolDef[], index: number): string | null {
  const name = (tools[index]?.name ?? "").trim();
  if (name === "") return "A name is required.";
  if (!TOOL_NAME_PATTERN.test(name)) return "Use only letters, digits, underscores and hyphens, up to 64 characters.";
  if (tools.some((t, i) => i !== index && t.name.trim() === name)) return "Another tool already uses this name.";
  return null;
}

export function paramNameError(tool: ToolDef, index: number): string | null {
  const name = (tool.params[index]?.name ?? "").trim();
  if (name === "") return "Not saved until it has a name.";
  if (tool.params.some((p, i) => i < index && p.name.trim() === name)) return "Not saved: an earlier parameter uses this name.";
  return null;
}

/** Tool names a Switch can match: those that pass `toolNameError`. */
export function routableToolNames(tools: ToolDef[]): string[] {
  return tools.flatMap((t, i) => toolNameError(tools, i) === null ? [t.name.trim()] : []);
}

function newTool(): ToolDef {
  return { name: "", description: "", params: [] };
}

function newParam(): ToolParam {
  return { name: "", type: "string", description: "", required: false, enumValues: [] };
}

let idCounter = 0;

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className?: string, text?: string): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (className) e.className = className;
  if (text !== undefined) e.textContent = text;
  return e;
}

function button(label: string, className: string, onClick: () => void, ariaLabel?: string): HTMLButtonElement {
  const b = el("button", className, label);
  b.type = "button";
  if (ariaLabel) b.setAttribute("aria-label", ariaLabel);
  b.addEventListener("click", onClick);
  return b;
}

function storedText(value: unknown): string {
  if (value === undefined || value === null) return "";
  return typeof value === "string" ? value : JSON.stringify(value, null, 2);
}

/** Tools field: a form per tool over the same JSON text the raw editor shows.
 *  `renderRaw` builds the raw JSON editor for a given text; it is passed in so
 *  this module does not import the field renderer that imports it. */
export function renderToolBuilderField(
  key: string,
  node: CanvasNode,
  onChange: () => void,
  syncRequired: () => void,
  renderRaw: (text: string) => HTMLElement,
): HTMLElement {
  const root = el("div", "tool-builder");
  const bar = el("div", "tool-builder-bar");
  const body = el("div", "tool-builder-body");
  root.append(bar, body);

  let mode: "builder" | "json" = parseTools(node.data.config[key]) ? "builder" : "json";

  const show = () => {
    bar.replaceChildren();
    body.replaceChildren();
    if (mode === "builder") {
      bar.appendChild(button("Edit as JSON", "folder-pick-btn", () => { mode = "json"; show(); }));
      body.appendChild(buildForm());
    } else {
      showJson();
    }
  };

  const showJson = () => {
    const toggle = button("Use builder", "folder-pick-btn", () => { mode = "builder"; show(); });
    const note = el("div", "field-hint");
    note.setAttribute("role", "status");
    const rawEl = renderRaw(storedText(node.data.config[key]));
    const sync = () => {
      const fits = parseTools(node.data.config[key]) !== null;
      toggle.disabled = !fits;
      note.textContent = fits ? "" : "The builder cannot show this list as written, so it stays exactly as you typed it. Reasons include text that is not a JSON array, nested parameters, input_schema, or the OpenAI function form.";
    };
    sync();
    rawEl.querySelector("textarea")?.addEventListener("input", sync);
    bar.appendChild(toggle);
    body.append(note, rawEl);
  };

  const buildForm = (): HTMLElement => {
    const tools = parseTools(node.data.config[key]) ?? [];
    const form = el("div", "tool-builder-form");
    const list = el("div", "tool-builder-list");
    const routeHint = el("div", "field-hint");
    const refreshers: Array<() => void> = [];

    const commit = () => {
      node.data.config[key] = serializeTools(tools);
      onChange();
      syncRequired();
      refreshers.forEach(r => r());
    };

    const errorSlot = (anchor: HTMLInputElement, message: () => string | null, sink: Array<() => void>): HTMLElement => {
      const slot = el("div", "tool-builder-error");
      slot.id = `tb-err-${++idCounter}`;
      const refresh = () => {
        const msg = message();
        slot.textContent = msg ?? "";
        if (msg) {
          anchor.setAttribute("aria-invalid", "true");
          anchor.setAttribute("aria-describedby", slot.id);
        } else {
          anchor.removeAttribute("aria-invalid");
          anchor.removeAttribute("aria-describedby");
        }
      };
      sink.push(refresh);
      refresh();
      return slot;
    };

    const refreshRoute = () => {
      const names = routableToolNames(tools);
      routeHint.hidden = names.length === 0;
      routeHint.textContent = `To act on a request, add a Switch: Source Node "${node.data.name}", Field tool_calls.0.tool, one case per tool (${names.join(", ")}).`;
    };

    const renderList = (focusSelector?: string) => {
      refreshers.length = 0;
      refreshers.push(refreshRoute);
      list.replaceChildren(...tools.map((tool, i) => buildToolCard(tool, i)));
      refreshers.forEach(r => r());
      if (focusSelector) form.querySelector<HTMLElement>(focusSelector)?.focus();
    };

    const buildToolCard = (tool: ToolDef, index: number): HTMLElement => {
      const card = el("div", "tool-card");
      card.setAttribute("role", "group");
      card.setAttribute("aria-label", `Tool ${index + 1}`);

      const header = el("div", "tool-card-header");
      const title = el("span", "tool-card-title");
      const removeTool = button("Remove tool", "folder-pick-btn tool-builder-remove", () => {
        tools.splice(index, 1);
        renderList(".tool-builder-add-tool");
        commit();
      });
      refreshers.push(() => {
        const label = tool.name.trim() || `Tool ${index + 1}`;
        title.textContent = label;
        removeTool.setAttribute("aria-label", `Remove ${label}`);
      });
      header.append(title, removeTool);

      const nameId = `tb-name-${++idCounter}`;
      const nameLabel = el("label", "tool-card-label", "Name");
      nameLabel.htmlFor = nameId;
      const nameInput = el("input");
      nameInput.type = "text";
      nameInput.id = nameId;
      nameInput.autocomplete = "off";
      nameInput.spellcheck = false;
      nameInput.placeholder = "get_weather";
      nameInput.value = tool.name;
      nameInput.addEventListener("input", () => { tool.name = nameInput.value; commit(); });

      const descId = `tb-desc-${++idCounter}`;
      const descLabel = el("label", "tool-card-label", "Description");
      descLabel.htmlFor = descId;
      const descInput = el("textarea");
      descInput.id = descId;
      descInput.rows = 2;
      descInput.value = tool.description;
      descInput.placeholder = "What the tool does and when to use it";
      descInput.addEventListener("input", () => { tool.description = descInput.value; commit(); });
      const descHint = el("div", "field-hint", "The model chooses a tool by its description.");

      const paramsTitle = el("div", "tool-card-label", "Parameters");
      const paramList = el("div", "tool-param-list");
      let paramRefreshers: Array<() => void> = [];
      refreshers.push(() => paramRefreshers.forEach(r => r()));
      const renderParams = (focusSelector?: string) => {
        paramRefreshers = [];
        paramList.replaceChildren(...tool.params.map((_, j) => buildParamRow(tool, index, j, paramRefreshers, renderParams)));
        if (focusSelector) card.querySelector<HTMLElement>(focusSelector)?.focus();
      };
      const addParam = button("Add parameter", "folder-pick-btn tool-builder-add-param", () => {
        tool.params.push(newParam());
        renderParams(".tool-param:last-child .tool-param-name");
        commit();
      });

      card.append(
        header,
        nameLabel, nameInput, errorSlot(nameInput, () => toolNameError(tools, index), refreshers),
        descLabel, descInput, descHint,
        paramsTitle, paramList, addParam,
      );
      renderParams();
      return card;
    };

    const buildParamRow = (
      tool: ToolDef, toolIndex: number, index: number,
      sink: Array<() => void>,
      rerenderParams: (focusSelector?: string) => void,
    ): HTMLElement => {
      const param = tool.params[index]!;
      const row = el("div", "tool-param");
      row.setAttribute("role", "group");
      row.setAttribute("aria-label", `Parameter ${index + 1} of tool ${toolIndex + 1}`);

      const top = el("div", "tool-param-top");
      const nameInput = el("input", "tool-param-name");
      nameInput.type = "text";
      nameInput.autocomplete = "off";
      nameInput.spellcheck = false;
      nameInput.placeholder = "city";
      nameInput.value = param.name;
      nameInput.setAttribute("aria-label", "Parameter name");
      nameInput.addEventListener("input", () => { param.name = nameInput.value; commit(); });

      const typeSelect = mkCustomSelect([...TOOL_PARAM_TYPES], param.type, (v) => {
        if (!isParamType(v)) return;
        param.type = v;
        if (v !== "string") param.enumValues = [];
        rerenderParams(`.tool-param:nth-child(${index + 1}) .csel-trigger`);
        commit();
      });
      typeSelect.querySelector("button")?.setAttribute("aria-label", "Parameter type");

      const reqLabel = el("label", "tool-param-required");
      const reqBox = el("input");
      reqBox.type = "checkbox";
      reqBox.checked = param.required;
      reqBox.addEventListener("change", () => { param.required = reqBox.checked; commit(); });
      reqLabel.append(reqBox, document.createTextNode(" Required"));

      const remove = button("×", "field-chip-remove", () => {
        tool.params.splice(index, 1);
        rerenderParams(".tool-builder-add-param");
        commit();
      }, "Remove parameter");

      top.append(nameInput, typeSelect, reqLabel, remove);

      const descInput = el("input");
      descInput.type = "text";
      descInput.autocomplete = "off";
      descInput.placeholder = "What this value means";
      descInput.value = param.description;
      descInput.setAttribute("aria-label", "Parameter description");
      descInput.addEventListener("input", () => { param.description = descInput.value; commit(); });

      row.append(top, errorSlot(nameInput, () => paramNameError(tool, index), sink), descInput);

      if (param.type === "string") {
        const enumInput = el("textarea");
        enumInput.rows = 2;
        enumInput.placeholder = "Allowed values, one per line (optional)";
        enumInput.value = param.enumValues.join("\n");
        enumInput.setAttribute("aria-label", "Allowed values, one per line");
        enumInput.addEventListener("input", () => {
          const seen = new Set<string>();
          param.enumValues = enumInput.value.split("\n").map(s => s.trim()).filter(s => s !== "" && !seen.has(s) && seen.add(s));
          commit();
        });
        row.appendChild(enumInput);
      }
      return row;
    };

    const addTool = button("Add tool", "folder-pick-btn tool-builder-add-tool", () => {
      tools.push(newTool());
      renderList(".tool-card:last-child input");
      commit();
    });

    const saveNote = el("div", "field-hint", "Parameters are saved as a closed object (additionalProperties false), which Strict Tools needs.");

    form.append(list, addTool, routeHint, saveNote);
    renderList();
    return form;
  };

  show();
  return root;
}
