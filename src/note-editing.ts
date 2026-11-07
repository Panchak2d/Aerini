export interface TextEdit {
  value: string;
  start: number;
  end: number;
}

const CHECK_PREFIX = /^- \[[ xX]\] /;
const LINK_URL = "https://";

export function toggleBold(value: string, start: number, end: number): TextEdit {
  const sel = value.slice(start, end);
  if (sel.length >= 4 && sel.startsWith("**") && sel.endsWith("**")) {
    return { value: value.slice(0, start) + sel.slice(2, -2) + value.slice(end), start, end: end - 4 };
  }
  if (start >= 2 && value.slice(start - 2, start) === "**" && value.slice(end, end + 2) === "**" && start !== end) {
    return { value: value.slice(0, start - 2) + sel + value.slice(end + 2), start: start - 2, end: end - 2 };
  }
  return { value: `${value.slice(0, start)}**${sel}**${value.slice(end)}`, start: start + 2, end: end + 2 };
}

export function insertLink(value: string, start: number, end: number): TextEdit {
  const label = value.slice(start, end) || "link";
  const before = `${value.slice(0, start)}[${label}](`;
  return { value: `${before}${LINK_URL})${value.slice(end)}`, start: before.length, end: before.length + LINK_URL.length };
}

function lineStart(value: string, pos: number): number {
  return pos === 0 ? 0 : value.lastIndexOf("\n", pos - 1) + 1;
}

function lineBounds(value: string, start: number, end: number): { from: number; to: number } {
  const from = lineStart(value, start);
  const nl = value.indexOf("\n", end > start ? end : start);
  return { from, to: nl === -1 ? value.length : nl };
}

export function toggleChecklist(value: string, start: number, end: number): TextEdit {
  const { from, to } = lineBounds(value, start, end);
  const lines = value.slice(from, to).split("\n");
  const allChecked = lines.every(l => CHECK_PREFIX.test(l));
  const next = lines.map(l => (allChecked ? l.replace(CHECK_PREFIX, "") : CHECK_PREFIX.test(l) ? l : `- [ ] ${l}`)).join("\n");
  const delta = next.length - (to - from);
  return { value: value.slice(0, from) + next + value.slice(to), start: from, end: to + delta };
}

export function continueChecklist(value: string, pos: number): TextEdit | null {
  const from = lineStart(value, pos);
  const nl = value.indexOf("\n", pos);
  const to = nl === -1 ? value.length : nl;
  const line = value.slice(from, to);
  const m = CHECK_PREFIX.exec(line);
  if (!m || pos < from + m[0].length) return null;
  if (line.length === m[0].length) {
    return { value: value.slice(0, from) + value.slice(to), start: from, end: from };
  }
  const insert = "\n- [ ] ";
  return { value: value.slice(0, pos) + insert + value.slice(pos), start: pos + insert.length, end: pos + insert.length };
}
