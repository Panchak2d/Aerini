import { describe, it, expect } from "vitest";
import { canonicalJson } from "../canonical-json";

describe("canonicalJson", () => {
  it("ignores object key order and drops values JSON.stringify drops, as a save/load round trip does", () => {
    expect(canonicalJson({ a: 1, b: 2 })).toBe(canonicalJson({ b: 2, a: 1 }));
    expect(canonicalJson({ b: 1, a: { d: [1, undefined], c: undefined } })).toBe('{"a":{"d":[1,null]},"b":1}');
    expect(canonicalJson(undefined)).toBe("null");
  });
});
