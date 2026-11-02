const FORBIDDEN_CHARS = /[.{},"']/;
const FUNCTION_LIKE_PREFIX = /^[A-Za-z_][A-Za-z0-9_]*\(/;
const TRAILING_NUMBER = /^(.*\S) (\d+)$/;

/** Plain-language reason a name can't be used as an expression reference, or null when it can. */
export function validateNodeName(name: string): string | null {
  const v = name.trim();
  if (!v) return "A node needs a name.";

  const bad = FORBIDDEN_CHARS.exec(v);
  if (bad) {
    const ch = bad[0];
    const quote = ch === "'" ? '"' : "'";
    return `Names can't contain ${quote}${ch}${quote}. Try "${v.split(ch).join("-")}".`;
  }
  if (v.startsWith("$")) return "Names can't start with '$'.";
  if (FUNCTION_LIKE_PREFIX.test(v)) {
    return `"${v}" would be read as a function call. Put a space before the '(' or remove the parentheses.`;
  }

  let depth = 0;
  for (const c of v) {
    if (c === "(") depth++;
    else if (c === ")" && --depth < 0) break;
  }
  if (depth !== 0) return "Parentheses in a name must come in pairs.";
  return null;
}

/** `base` when free, otherwise `base 2`, `base 3`, ... (a trailing number is replaced when the unnumbered name is also taken). */
export function uniqueNodeName(base: string, taken: Iterable<string>): string {
  const used = new Set(taken);
  if (!used.has(base)) return base;
  const m = TRAILING_NUMBER.exec(base);
  const stem = m && used.has(m[1]) ? m[1] : base;
  for (let n = 2; ; n++) {
    const candidate = `${stem} ${n}`;
    if (!used.has(candidate)) return candidate;
  }
}

function rewriteExpression(expr: string, oldName: string, newName: string): string {
  const needle = `${oldName}.`;
  let out = "";
  let quote = "";
  let escaped = false;
  let prev = "";
  let i = 0;
  while (i < expr.length) {
    const c = expr[i];
    if (quote) {
      out += c;
      if (escaped) escaped = false;
      else if (c === "\\") escaped = true;
      else if (c === quote) quote = "";
      i++;
      continue;
    }
    if (c === '"' || c === "'") {
      quote = c;
      out += c;
      prev = c;
      i++;
      continue;
    }
    if (/\s/.test(c)) {
      out += c;
      i++;
      continue;
    }
    if ((prev === "" || prev === "(" || prev === ",") && expr.startsWith(needle, i)) {
      out += `${newName}.`;
      i += needle.length;
      prev = ".";
      continue;
    }
    out += c;
    prev = c;
    i++;
  }
  return out;
}

function rewriteText(text: string, oldName: string, newName: string): string {
  if (!text.includes("{{") || !text.includes(oldName)) return text;
  let out = "";
  let i = 0;
  while (i < text.length) {
    const open = text.indexOf("{{", i);
    if (open < 0) break;
    const close = text.indexOf("}}", open + 2);
    if (close < 0) break;
    out += text.slice(i, open + 2) + rewriteExpression(text.slice(open + 2, close), oldName, newName) + "}}";
    i = close + 2;
  }
  return out + text.slice(i);
}

/**
 * Rewrites `{{OldName.…}}` references (including ones used as function arguments)
 * inside every string of `value`, recursing through arrays and object values.
 * Returns `value` itself, not a copy, when nothing changed.
 */
export function renameReferences(value: unknown, oldName: string, newName: string): unknown {
  if (typeof value === "string") return rewriteText(value, oldName, newName);
  if (Array.isArray(value)) {
    let changed = false;
    const next = value.map(v => {
      const r = renameReferences(v, oldName, newName);
      if (r !== v) changed = true;
      return r;
    });
    return changed ? next : value;
  }
  if (value && typeof value === "object") {
    let changed = false;
    const next: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(value as Record<string, unknown>)) {
      const r = renameReferences(v, oldName, newName);
      if (r !== v) changed = true;
      next[k] = r;
    }
    return changed ? next : value;
  }
  return value;
}
