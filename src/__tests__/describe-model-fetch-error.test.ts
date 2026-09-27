import { describe, it, expect } from "vitest";
import { describeModelFetchError } from "../ipc/providers";

describe("describeModelFetchError", () => {
  it("normal case: strips the backend's CODE: prefix, leaving just the reason", () => {
    expect(describeModelFetchError(new Error("BAD_KEY: Incorrect API key provided")))
      .toBe("Incorrect API key provided");
  });

  it("edge case: a message with no CODE: prefix is returned as-is", () => {
    expect(describeModelFetchError(new Error("connection reset")))
      .toBe("connection reset");
  });

  it("edge case: a non-Error rejection and an empty message both fall back to a fixed string", () => {
    expect(describeModelFetchError("")).toBe("Unknown error");
    expect(describeModelFetchError(new Error(""))).toBe("Unknown error");
  });
});
