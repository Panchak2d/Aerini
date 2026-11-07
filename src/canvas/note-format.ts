export const MAX_NOTE_TEXT = 10000;
export const MAX_FRAME_TITLE = 120;
const MAX_HREF = 2048;
const CHECK_INDENT = 20;

export interface Span {
  text: string;
  bold: boolean;
  href: string | null;
}

export interface RichLine {
  src: number;
  heading: boolean;
  check: boolean;
  checked: boolean;
  spans: Span[];
}

export interface Run {
  text: string;
  bold: boolean;
  href: string | null;
  x: number;
  w: number;
}

export interface LaidLine {
  src: number;
  heading: boolean;
  /** True on the first visual line of a checkbox item; wrapped continuations are false. */
  box: boolean;
  checked: boolean;
  indent: number;
  runs: Run[];
}

export type Measure = (text: string, bold: boolean, heading: boolean) => number;

/** Only plain http(s) links without embedded credentials; anything else renders as text. */
export function safeHref(raw: string): string | null {
  if (raw.length > MAX_HREF) return null;
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    return null;
  }
  if (url.protocol !== "https:" && url.protocol !== "http:") return null;
  if (url.username || url.password || !url.hostname) return null;
  return url.href;
}

const CHECK_RE = /^- \[([ xX])\] ?/;
const HEADING_RE = /^# +/;
const LINK_RE = /^\[([^\][\n]{1,200})\]\(([^()\s]{1,2048})\)/;

function parseInline(line: string): Span[] {
  const spans: Span[] = [];
  let buf = "";
  let bold = false;
  const flush = () => {
    if (buf) spans.push({ text: buf, bold, href: null });
    buf = "";
  };
  let i = 0;
  while (i < line.length) {
    if (line.startsWith("**", i)) {
      const closes = bold || line.indexOf("**", i + 2) > i + 2;
      if (closes) {
        flush();
        bold = !bold;
        i += 2;
        continue;
      }
    }
    if (line[i] === "[") {
      const m = LINK_RE.exec(line.slice(i));
      const href = m ? safeHref(m[2]) : null;
      if (m && href) {
        flush();
        spans.push({ text: m[1], bold, href });
        i += m[0].length;
        continue;
      }
    }
    buf += line[i];
    i++;
  }
  flush();
  return spans;
}

export function parseNote(text: string): RichLine[] {
  const capped = text.length > MAX_NOTE_TEXT ? text.slice(0, MAX_NOTE_TEXT) : text;
  return capped.split(/\r?\n/).map((raw, src) => {
    const m = CHECK_RE.exec(raw);
    if (m) {
      return { src, heading: false, check: true, checked: m[1] !== " ", spans: parseInline(raw.slice(m[0].length)) };
    }
    const h = HEADING_RE.exec(raw);
    if (h) return { src, heading: true, check: false, checked: false, spans: parseInline(raw.slice(h[0].length)) };
    return { src, heading: false, check: false, checked: false, spans: parseInline(raw) };
  });
}

export function toggleCheckbox(text: string, src: number): string {
  const lines = text.split("\n");
  const raw = lines[src];
  const m = raw === undefined ? null : CHECK_RE.exec(raw);
  if (!m) return text;
  lines[src] = raw.slice(0, 3) + (m[1] === " " ? "x" : " ") + raw.slice(4);
  return lines.join("\n");
}

interface Token { text: string; bold: boolean; href: string | null; space: boolean }

function tokenize(spans: Span[]): Token[] {
  const out: Token[] = [];
  for (const s of spans) {
    for (const part of s.text.split(/( +)/)) {
      if (part === "") continue;
      out.push({ text: part, bold: s.bold, href: s.href, space: part[0] === " " });
    }
  }
  return out;
}

function pushRun(runs: Run[], text: string, bold: boolean, href: string | null, x: number, w: number): void {
  const last = runs[runs.length - 1];
  if (last && last.bold === bold && last.href === href && Math.abs(last.x + last.w - x) < 0.01) {
    last.text += text;
    last.w += w;
  } else {
    runs.push({ text, bold, href, x, w });
  }
}

function breakWord(text: string, bold: boolean, heading: boolean, measure: Measure, maxW: number): string[] {
  const pieces: string[] = [];
  let cur = "";
  for (const ch of text) {
    if (cur && measure(cur + ch, bold, heading) > maxW) {
      pieces.push(cur);
      cur = ch;
    } else {
      cur += ch;
    }
  }
  if (cur) pieces.push(cur);
  return pieces;
}

export function layoutNote(lines: RichLine[], measure: Measure, maxW: number, checkIndent = CHECK_INDENT): LaidLine[] {
  const out: LaidLine[] = [];
  for (const line of lines) {
    const indent = line.check ? checkIndent : 0;
    const avail = Math.max(24, maxW - indent);
    let cur: LaidLine = { src: line.src, heading: line.heading, box: line.check, checked: line.checked, indent, runs: [] };
    let x = 0;
    const newLine = () => {
      const last = cur.runs[cur.runs.length - 1];
      if (last && last.text.endsWith(" ")) {
        last.text = last.text.replace(/ +$/, "");
        last.w = measure(last.text, last.bold, line.heading);
        if (!last.text) cur.runs.pop();
      }
      out.push(cur);
      cur = { src: line.src, heading: line.heading, box: false, checked: line.checked, indent, runs: [] };
      x = 0;
    };
    for (const tok of tokenize(line.spans)) {
      const w = measure(tok.text, tok.bold, line.heading);
      if (tok.space) {
        if (x > 0 && x + w <= avail) { pushRun(cur.runs, tok.text, tok.bold, tok.href, x, w); x += w; }
        continue;
      }
      if (x + w <= avail) {
        pushRun(cur.runs, tok.text, tok.bold, tok.href, x, w);
        x += w;
        continue;
      }
      if (x > 0) newLine();
      if (w <= avail) {
        pushRun(cur.runs, tok.text, tok.bold, tok.href, 0, w);
        x = w;
        continue;
      }
      const pieces = breakWord(tok.text, tok.bold, line.heading, measure, avail);
      pieces.forEach((piece, i) => {
        const pw = measure(piece, tok.bold, line.heading);
        pushRun(cur.runs, piece, tok.bold, tok.href, 0, pw);
        x = pw;
        if (i < pieces.length - 1) newLine();
      });
    }
    out.push(cur);
  }
  return out;
}
