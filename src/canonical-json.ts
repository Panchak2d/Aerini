// Returns undefined for values JSON.stringify drops (undefined, functions, symbols).
function encode(value: unknown): string | undefined {
  if (value === null || typeof value !== "object") return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(v => encode(v) ?? "null").join(",")}]`;
  const obj = value as Record<string, unknown>;
  const parts: string[] = [];
  for (const key of Object.keys(obj).sort()) {
    const encoded = encode(obj[key]);
    if (encoded !== undefined) parts.push(`${JSON.stringify(key)}:${encoded}`);
  }
  return `{${parts.join(",")}}`;
}

/**
 * JSON text for plain data that is identical for equal data regardless of
 * object key order. Values JSON.stringify would omit are omitted the same
 * way, so the text matches what a save/load round trip produces.
 */
export function canonicalJson(value: unknown): string {
  return encode(value) ?? "null";
}
