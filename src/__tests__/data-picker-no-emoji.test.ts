import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";

const FILES = [
  "src/expression-picker.ts",
  "src/expression-functions.ts",
  "src/field-preview.ts",
  "src/popover/expression-field.ts",
  "src/point-mode.ts",
  "src/field-wire-drop.ts",
  "src/data-switch.ts",
  "src/problems.ts",
  "src/problems-panel.ts",
  "src/mention.ts",
  "src/smart-paste.ts",
  "src/error-map.ts",
  "src/preset-cache.ts",
  "src/side-effects.ts",
  "src/node-test.ts",
  "src/popover/test-scope.ts",
  "src/templates.ts",
  "src/template-gallery.ts",
  "src/checklist.ts",
  "src/port-labels.ts",
  "src/onboarding.ts",
  "src/connect-insert.ts",
  "src/describe-field.ts",
  "src/popover/describe-panel.ts",
];
const PICTOGRAPHIC = /[\u{1F000}-\u{1FAFF}\u{2190}-\u{21FF}\u{2300}-\u{23FF}\u{2460}-\u{27BF}\u{2900}-\u{297F}\u{2B00}-\u{2BFF}\uFE0F]/u;

describe("data picker source", () => {
  it.each(FILES)("%s_has_no_emoji_or_pictographic_characters", (file) => {
    const bad = readFileSync(file, "utf8").split("\n").filter((l) => PICTOGRAPHIC.test(l));
    expect(bad).toEqual([]);
  });
});
