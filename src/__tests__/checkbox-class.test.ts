import { describe, it, expect } from "vitest";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, resolve } from "node:path";

const SRC = resolve(__dirname, "..");
const SITE = /\.type\s*=\s*["']checkbox["']|type=\\?["']checkbox\\?["']|setAttribute\(\s*["']type["']\s*,\s*["']checkbox["']\s*\)/;
const LINES_BEFORE = 3;
const LINES_AFTER = 4;

function sourceFiles(dir: string): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    if (name === "__tests__") continue;
    const full = join(dir, name);
    if (statSync(full).isDirectory()) out.push(...sourceFiles(full));
    else if (name.endsWith(".ts")) out.push(full);
  }
  return out;
}

describe("checkbox styling", () => {
  it("every checkbox created in TypeScript carries the shared ui-check class", () => {
    let sites = 0;
    const missing: string[] = [];
    for (const file of sourceFiles(SRC)) {
      const lines = readFileSync(file, "utf-8").split("\n");
      lines.forEach((line, i) => {
        if (!SITE.test(line)) return;
        sites++;
        const near = lines.slice(Math.max(0, i - LINES_BEFORE), i + LINES_AFTER + 1).join("\n");
        if (!/\bui-check\b/.test(near)) missing.push(`${file.slice(SRC.length + 1)}:${i + 1}`);
      });
    }
    expect(sites).toBeGreaterThan(0);
    expect(missing).toEqual([]);
  });

  it("static checkboxes in index.html are either ui-check boxes or settings switches", () => {
    const html = readFileSync(join(SRC, "index.html"), "utf-8");
    const inputs = html.match(/<input\b[^>]*type="checkbox"[^>]*>/g) ?? [];
    expect(inputs.length).toBeGreaterThan(0);
    const unstyled = inputs.filter((tag) => !/class="[^"]*\b(ui-check|settings-toggle)\b/.test(tag));
    expect(unstyled).toEqual([]);
  });

  it("the shared class styles checked, indeterminate, disabled, keyboard focus and forced colors without image masks", () => {
    const css = readFileSync(join(SRC, "styles/panels.css"), "utf-8");
    for (const state of [":checked", ":indeterminate", ":disabled", ":focus-visible"]) {
      expect(css).toContain(`.ui-check${state}`);
    }
    expect(css).toMatch(/\.ui-check\s*\{[^}]*appearance:\s*none/);
    expect(css).toMatch(/@media \(forced-colors: active\)\s*\{[^@]*\.ui-check/);
    const block = css.slice(css.indexOf(".ui-check {"), css.indexOf(".ui-check-row"));
    expect(block).not.toMatch(/mask-image|url\(/);
  });
});
