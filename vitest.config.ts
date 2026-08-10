import { defineConfig } from "vitest/config";

// --no-experimental-webstorage doesn't exist before Node 25 (nodejs/node#57666)
// — passing it on an older Node crashes the process with "bad option" instead
// of being ignored, so it must be gated by version rather than passed unconditionally.
const nodeMajor = Number(process.versions.node.split(".")[0]);

export default defineConfig({
  test: {
    environment: "node",
    include: ["src/__tests__/**/*.test.ts"],
    // Node 25+ ships a built-in `localStorage` global on by default
    // (nodejs/node#57666) that, without --localstorage-file, resolves to
    // undefined and shadows jsdom's own localStorage in files using the
    // `@vitest-environment jsdom` docblock — breaking `localStorage.clear()`
    // etc. with "Cannot read properties of undefined". Disabling Node's
    // copy lets jsdom's localStorage own the global again. VERIFIED via
    // nodejs.org globals docs + matching community reports (2026).
    execArgv: nodeMajor >= 25 ? ["--no-experimental-webstorage"] : [],
  },
});
