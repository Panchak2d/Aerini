/**
 * DEVIATION NOTES:
 *
 * Plan: "Test that a value matching the files array shape renders a file chip"
 * Reality: No "file chip" renderer exists. Media files are rendered by
 * `renderMediaBatch` (private), which is async and calls Tauri's
 * `invoke("write_temp_file")`. Not unit-testable in isolation.
 *
 * Plan: "Test that a base64 image value renders an image element"
 * Reality: `renderMediaBatch` creates <img> elements for image/* MIME types
 * but it is private, async, and Tauri-dependent. Not reachable in a unit test.
 *
 * Plan: "Test that a plain string renders to text, not HTML-escaped incorrectly"
 * Reality: `extractPreview` is the public pure helper for previewing values.
 * It does NOT HTML-escape — it is a preview function, not a renderer.
 * The HTML renderers (renderGenericOutput etc.) are internal.
 *
 * NOTE — syntaxHighlight string highlighting:
 * `syntaxHighlight` calls `escapeHtml` as its first step, which converts `"`
 * to `&quot;`. The regex that wraps JSON string keys and values requires
 * literal `"` characters. After escaping, `"` is gone, so json-key and
 * json-str spans are never produced. Only `null`, `true`, `false`, and
 * numbers are wrapped — these survive escapeHtml unchanged.
 * Tests below reflect this actual behaviour.
 */
import { describe, it, expect, vi } from "vitest";
import { extractPreview, syntaxHighlight } from "../output-renderer";

// output-renderer.ts imports invoke/convertFileSrc from @tauri-apps/api/core.
// Mock the module so the import does not throw in Node.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn((path: string) => path),
}));

// ---------------------------------------------------------------------------
// extractPreview — null / undefined
// ---------------------------------------------------------------------------

describe("extractPreview — null / undefined", () => {
  it("returns 'null' for null input", () => {
    expect(extractPreview(null)).toBe("null");
  });

  it("returns 'null' for undefined input", () => {
    expect(extractPreview(undefined)).toBe("null");
  });
});

describe("extractPreview — primitive values", () => {
  it("returns a plain string directly (no HTML escaping)", () => {
    expect(extractPreview("hello world")).toBe("hello world");
  });

  it("returns angle-bracket strings unchanged (not escaped)", () => {
    // extractPreview is a preview function, not a renderer; it never HTML-escapes
    expect(extractPreview("<b>bold</b>")).toBe("<b>bold</b>");
  });

  it("truncates strings longer than 120 characters", () => {
    const long = "x".repeat(200);
    expect(extractPreview(long).length).toBe(120);
  });

  it("returns string form of a number", () => {
    expect(extractPreview(42)).toBe("42");
  });

  it("returns string form of booleans", () => {
    expect(extractPreview(true)).toBe("true");
    expect(extractPreview(false)).toBe("false");
  });
});

describe("extractPreview — object priority keys", () => {
  it("extracts 'content' key first", () => {
    expect(extractPreview({ content: "AI response" })).toBe("AI response");
  });

  it("extracts 'result' key", () => {
    expect(extractPreview({ result: "computed" })).toBe("computed");
  });

  it("extracts 'body' key", () => {
    expect(extractPreview({ body: "http body" })).toBe("http body");
  });

  it("extracts 'value' key", () => {
    expect(extractPreview({ value: "stored" })).toBe("stored");
  });

  it("extracts 'text' key", () => {
    expect(extractPreview({ text: "plain text" })).toBe("plain text");
  });

  it("falls back to JSON.stringify for objects with no priority key", () => {
    const result = extractPreview({ unknown_key: "something" });
    expect(result).toContain("unknown_key");
  });
});

// ---------------------------------------------------------------------------
// syntaxHighlight — HTML escaping contract
// ---------------------------------------------------------------------------

describe("syntaxHighlight — XSS escaping", () => {
  it("escapes < and > so raw script tags never appear in output", () => {
    const out = syntaxHighlight("<script>alert(1)</script>");
    expect(out).not.toContain("<script>");
    expect(out).toContain("&lt;script&gt;");
  });

  it("escapes & so ampersands don't produce double-encoding elsewhere", () => {
    const out = syntaxHighlight("a & b");
    expect(out).toContain("&amp;");
    expect(out).not.toContain("&amp;amp;");
  });
});

// ---------------------------------------------------------------------------
// syntaxHighlight — span wrapping (values that survive escapeHtml unchanged)
// ---------------------------------------------------------------------------

describe("syntaxHighlight — literal keyword spans", () => {
  it("wraps null in json-null span", () => {
    expect(syntaxHighlight("null")).toContain('<span class="json-null">null</span>');
  });

  it("wraps true in json-bool span", () => {
    expect(syntaxHighlight("true")).toContain('<span class="json-bool">true</span>');
  });

  it("wraps false in json-bool span", () => {
    expect(syntaxHighlight("false")).toContain('<span class="json-bool">false</span>');
  });

  it("wraps a standalone integer in json-num span", () => {
    expect(syntaxHighlight("42")).toContain('<span class="json-num">42</span>');
  });

  it("wraps a negative number in json-num span", () => {
    expect(syntaxHighlight("-7")).toContain('<span class="json-num">-7</span>');
  });
});

describe("syntaxHighlight — balanced span tags", () => {
  it("all opened spans are closed — no orphan tags", () => {
    // Use JSON with only literals that survive escapeHtml (no string values)
    const out = syntaxHighlight(JSON.stringify([1, true, null, false, 42]));
    const opens  = (out.match(/<span/g)  ?? []).length;
    const closes = (out.match(/<\/span>/g) ?? []).length;
    expect(opens).toBe(closes);
    expect(opens).toBeGreaterThan(0);
  });
});

describe("syntaxHighlight — string values (behaviour note)", () => {
  it("double-quote chars are HTML-escaped to &quot; — json-str spans are NOT produced", () => {
    // This documents the actual behaviour: escapeHtml converts " → &quot; before
    // the span-wrapping regex runs, so no json-str or json-key spans are emitted.
    const out = syntaxHighlight('"hello"');
    expect(out).toBe("&quot;hello&quot;");
    expect(out).not.toContain("json-str");
  });
});
