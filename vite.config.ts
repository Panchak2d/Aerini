import { defineConfig } from "vite";

export default defineConfig({
  root: "src",
  base: "./",
  build: {
    outDir: "../dist",
    emptyOutDir: true,
    // P31 hotfix: Vite code-splits CSS per dynamically-imported chunk (the
    // lazy ChatPanel/BgJobsPanel modules from P29/P30) and injects it via a
    // runtime <style> tag. tauri.conf.json's CSP (style-src 'self', no
    // 'unsafe-inline') blocks those injections, so the chunk's CSS silently
    // fails to apply — e.g. .zone-header/.zone-title loses its flex layout
    // and collapses to the top-left corner. Bundling all CSS into one file
    // loaded via a static <link> at build time avoids runtime style
    // injection entirely, with zero CSP loosening.
    cssCodeSplit: false,
  },
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
});
