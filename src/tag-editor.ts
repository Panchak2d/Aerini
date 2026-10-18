import {
  MAX_TAGS_PER_WORKFLOW, normalizeTag, suggestTags, tagKey,
  type TagEdit, type TagIndex,
} from "./workflow-tags";
import { closeOpenMenu } from "./menu";

export interface TagEditorChip {
  label: string;
  /** How many of the `total` edited workflows carry this tag. */
  have: number;
}

export interface TagEditorOptions {
  root: HTMLElement;
  initial: TagEditorChip[];
  total: number;
  getLibrary: () => TagIndex;
  onChange?: (displayed: string[], edit: TagEdit) => void;
}

export interface TagEditor {
  getEdit(): TagEdit;
  getDisplayed(): string[];
  focus(): void;
}

const SUGGESTION_LIMIT = 8;
let editorSeq = 0;

export function createTagEditor(opts: TagEditorOptions): TagEditor {
  const uid = `tag-editor-${++editorSeq}`;
  const base = opts.initial.map(c => ({ key: tagKey(c.label), label: c.label, have: c.have }));
  const baseByKey = new Map(base.map(c => [c.key, c]));
  const removed = new Set<string>();
  const added = new Map<string, string>();
  let active = -1;
  let suggestions: string[] = [];

  opts.root.textContent = "";
  const chipsEl = document.createElement("div");
  chipsEl.className = "wf-tags-chips";
  const input = document.createElement("input");
  input.type = "text";
  input.className = "settings-text-input";
  input.placeholder = "Add a tag, press Enter";
  input.autocomplete = "off";
  input.spellcheck = false;
  input.setAttribute("aria-label", "Add a tag");
  input.setAttribute("role", "combobox");
  input.setAttribute("aria-autocomplete", "list");
  input.setAttribute("aria-expanded", "false");
  input.setAttribute("aria-controls", `${uid}-list`);
  const listEl = document.createElement("ul");
  listEl.id = `${uid}-list`;
  listEl.className = "wf-tag-suggest";
  listEl.setAttribute("role", "listbox");
  listEl.setAttribute("aria-label", "Existing tags");
  listEl.hidden = true;
  const msgEl = document.createElement("div");
  msgEl.className = "wf-tag-editor-msg";
  msgEl.setAttribute("role", "status");
  opts.root.append(chipsEl, input, listEl, msgEl);

  const displayed = (): Array<{ key: string; label: string; have: number }> => {
    const out = base
      .filter(c => !removed.has(c.key))
      .map(c => (added.has(c.key) ? { ...c, have: opts.total } : c));
    for (const [key, label] of added) if (!baseByKey.has(key)) out.push({ key, label, have: opts.total });
    return out;
  };

  const getEdit = (): TagEdit => ({
    add: [...added.values()],
    remove: base.filter(c => removed.has(c.key)).map(c => c.label),
  });

  const changed = (): void => {
    render();
    opts.onChange?.(displayed().map(c => c.label), getEdit());
  };

  const removeKey = (key: string): void => {
    msgEl.textContent = "";
    added.delete(key);
    if (baseByKey.has(key)) removed.add(key);
    changed();
  };

  const render = (): void => {
    chipsEl.textContent = "";
    for (const c of displayed()) {
      const chip = document.createElement("span");
      chip.className = "wf-tag-chip";
      const label = document.createElement("span");
      label.textContent = c.label;
      chip.appendChild(label);
      if (c.have < opts.total) {
        const n = document.createElement("span");
        n.className = "wf-tag-chip-count";
        n.textContent = `${c.have}/${opts.total}`;
        n.title = `On ${c.have} of ${opts.total} workflows`;
        chip.appendChild(n);
      }
      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "wf-tag-chip-remove";
      remove.textContent = "✕";
      remove.setAttribute("aria-label", `Remove "${c.label}"`);
      remove.title = `Remove "${c.label}"`;
      remove.setAttribute("data-tooltip", `Remove "${c.label}"`);
      remove.addEventListener("click", () => { removeKey(c.key); input.focus(); });
      chip.appendChild(remove);
      chipsEl.appendChild(chip);
    }
  };

  const addTag = (raw: string): void => {
    const tag = normalizeTag(raw);
    if (!tag) return;
    const key = tagKey(tag);
    const current = displayed().find(c => c.key === key);
    if (current && current.have >= opts.total) return;
    if (!current && opts.total === 1 && displayed().length >= MAX_TAGS_PER_WORKFLOW) {
      msgEl.textContent = `A workflow can have up to ${MAX_TAGS_PER_WORKFLOW} tags`;
      return;
    }
    msgEl.textContent = "";
    const label = opts.getLibrary().get(key)?.label ?? baseByKey.get(key)?.label ?? tag;
    removed.delete(key);
    const b = baseByKey.get(key);
    if (!b || b.have < opts.total) added.set(key, label);
    changed();
  };

  const commitText = (text: string): void => {
    for (const part of text.split(",")) addTag(part);
  };

  const closeList = (): void => {
    listEl.hidden = true;
    active = -1;
    suggestions = [];
    input.setAttribute("aria-expanded", "false");
    input.removeAttribute("aria-activedescendant");
  };

  const setActive = (i: number): void => {
    active = i;
    Array.from(listEl.children).forEach((li, idx) => {
      li.setAttribute("aria-selected", String(idx === i));
      li.classList.toggle("active", idx === i);
    });
    if (i >= 0) input.setAttribute("aria-activedescendant", `${uid}-opt-${i}`);
    else input.removeAttribute("aria-activedescendant");
  };

  const openList = (): void => {
    const lib = opts.getLibrary();
    const have = new Set(displayed().filter(c => c.have >= opts.total).map(c => c.key));
    const rows = suggestTags(lib, have, input.value, SUGGESTION_LIMIT);
    listEl.textContent = "";
    suggestions = rows.map(r => r.label);
    if (!rows.length) { closeList(); return; }
    rows.forEach((r, i) => {
      const li = document.createElement("li");
      li.id = `${uid}-opt-${i}`;
      li.className = "wf-tag-suggest-item";
      li.setAttribute("role", "option");
      li.setAttribute("aria-selected", "false");
      const name = document.createElement("span");
      name.textContent = r.label;
      const n = document.createElement("span");
      n.className = "wf-tag-suggest-count";
      n.textContent = String(r.count);
      li.append(name, n);
      li.addEventListener("mousedown", e => e.preventDefault());
      li.addEventListener("click", () => { addTag(r.label); input.value = ""; openList(); input.focus(); });
      listEl.appendChild(li);
    });
    listEl.hidden = false;
    input.setAttribute("aria-expanded", "true");
    setActive(-1);
  };

  input.addEventListener("input", () => {
    if (input.value.includes(",")) {
      const parts = input.value.split(",");
      const tail = parts.pop() ?? "";
      for (const p of parts) addTag(p);
      input.value = tail;
    }
    openList();
  });
  input.addEventListener("focus", openList);
  input.addEventListener("blur", closeList);
  input.addEventListener("keydown", e => {
    if (e.key === "Enter") {
      e.preventDefault();
      if (active >= 0 && suggestions[active]) addTag(suggestions[active]);
      else commitText(input.value);
      input.value = "";
      openList();
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      if (listEl.hidden) { openList(); if (!suggestions.length) return; }
      const n = suggestions.length;
      if (!n) return;
      setActive(e.key === "ArrowDown" ? (active + 1) % n : (active - 1 + n) % n);
    } else if (e.key === "Escape" && !listEl.hidden) {
      e.stopPropagation();
      closeList();
    } else if (e.key === "Backspace" && input.value === "") {
      const all = displayed();
      if (all.length) removeKey(all[all.length - 1].key);
    }
  });

  render();

  return {
    getEdit,
    getDisplayed: () => displayed().map(c => c.label),
    focus: () => input.focus(),
  };
}

export interface TagPopoverOptions {
  anchor: HTMLElement;
  invoker: HTMLElement;
  label: string;
  initial: TagEditorChip[];
  total: number;
  getLibrary: () => TagIndex;
  onApply: (edit: TagEdit) => void;
}

let closePopover: (() => void) | null = null;

export function closeTagPopover(): void {
  closePopover?.();
}

/** Closing by outside click, focus leaving, or Done applies the edit as one
 *  change; Escape discards it. An unchanged edit never calls `onApply`. */
export function openTagPopover(opts: TagPopoverOptions): void {
  closeOpenMenu();
  closeTagPopover();

  const pop = document.createElement("div");
  pop.className = "wf-tag-popover";
  pop.setAttribute("role", "dialog");
  pop.setAttribute("aria-label", opts.label);
  const title = document.createElement("div");
  title.className = "wf-tag-popover-title";
  title.textContent = opts.label;
  const body = document.createElement("div");
  body.className = "wf-tags-editor";
  const done = document.createElement("button");
  done.type = "button";
  done.className = "btn-sm wf-tag-popover-done";
  done.textContent = "Done";
  pop.append(title, body, done);

  const editor = createTagEditor({ root: body, initial: opts.initial, total: opts.total, getLibrary: opts.getLibrary });

  let closed = false;
  const finish = (apply: boolean, restoreFocus: boolean): void => {
    if (closed) return;
    closed = true;
    if (closePopover === closeApplying) closePopover = null;
    document.removeEventListener("mousedown", onOutside, true);
    pop.remove();
    if (restoreFocus && opts.invoker.isConnected) opts.invoker.focus({ preventScroll: true });
    if (!apply) return;
    const edit = editor.getEdit();
    if (edit.add.length || edit.remove.length) opts.onApply(edit);
  };
  const closeApplying = (): void => finish(true, false);
  const onOutside = (ev: MouseEvent): void => {
    if (!pop.contains(ev.target as Node)) finish(true, false);
  };

  done.addEventListener("click", () => finish(true, true));
  pop.addEventListener("keydown", e => {
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); finish(false, true); }
  });
  pop.addEventListener("focusout", e => {
    const to = (e as FocusEvent).relatedTarget as Node | null;
    if (to && !pop.contains(to)) finish(true, false);
  });

  document.body.appendChild(pop);
  const r = opts.anchor.getBoundingClientRect();
  const w = pop.offsetWidth;
  const h = pop.offsetHeight;
  const pad = 8;
  let top = r.bottom + 4;
  if (top + h > window.innerHeight - pad && r.top - 4 - h >= pad) top = r.top - 4 - h;
  pop.style.left = `${Math.max(pad, Math.min(r.left, window.innerWidth - w - pad))}px`;
  pop.style.top = `${Math.max(pad, Math.min(top, window.innerHeight - h - pad))}px`;

  closePopover = closeApplying;
  setTimeout(() => { if (!closed) document.addEventListener("mousedown", onOutside, true); }, 0);
  editor.focus();
}
