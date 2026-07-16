import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { invoke } from "@tauri-apps/api/core";
import { showImportPreview } from "./modal-manager";
import { isTauri } from "./utils";
import { getSetting, installPluginFromPath } from "./ipc/workflow";
import { showRestartBanner } from "./plugin-settings";
import { showConfirm } from "./confirm";

type ToastFn = (msg: string, type: "success" | "error" | "info") => void;

export function bindDropImport(toast: ToastFn): void {
  const overlay = document.getElementById("drop-overlay")!;
  const area    = document.getElementById("canvas-area")!;

  // DOM drag-drop (browser mode / in-app drag)
  let dragCounter = 0;
  area.addEventListener("dragenter", e => {
    e.preventDefault(); dragCounter++; overlay.classList.add("active");
  });
  area.addEventListener("dragleave", () => {
    dragCounter--; if (dragCounter <= 0) { dragCounter = 0; overlay.classList.remove("active"); }
  });
  area.addEventListener("dragover", e => e.preventDefault());
  area.addEventListener("drop", e => {
    e.preventDefault(); dragCounter = 0; overlay.classList.remove("active");
    const file = e.dataTransfer?.files[0];
    if (!file) return;
    if (file.name.toLowerCase().endsWith(".wasm")) {
      // Reading arbitrary file paths from a DOM File object is browser-sandboxed,
      // so .wasm installs from DOM drag are not supported here — the Tauri
      // native drop path (below) handles the actual install.
      toast("To install a plugin, use the Install .wasm button in Settings → Plugins.", "info");
      return;
    }
    readAndPreviewFile(file, toast);
  });

  // Tauri v2 native OS file-drop (drag from file manager)
  if (isTauri()) {
    getCurrentWebviewWindow().onDragDropEvent(event => {
      const { type } = event.payload;

      if (type === "enter" || type === "over") {
        overlay.classList.add("active");
        return;
      }
      if (type === "leave") {
        overlay.classList.remove("active");
        return;
      }
      if (type === "drop") {
        overlay.classList.remove("active");
        const paths = (event.payload as { type: string; paths: string[] }).paths;
        if (!paths?.length) return;

        const wasmPath = paths.find(p => p.toLowerCase().endsWith(".wasm"));
        if (wasmPath) {
          handleWasmDrop(wasmPath, toast);
          return;
        }

        const aeriniPath = paths.find(p =>
          p.endsWith(".aerini") || p.endsWith(".json")
        );
        if (!aeriniPath) {
          toast("Drop a .aerini file to import a workflow", "info");
          return;
        }

        // Use read_text_file command — capability grants all file reads in tauri.conf.json
        invoke<string>("read_text_file", { path: aeriniPath })
          .then(content => {
            try {
              const obj = JSON.parse(content);
              showImportPreview(obj);
            } catch {
              toast("Could not parse the dropped file — is it a valid .aerini file?", "error");
            }
          })
          .catch(e => toast(`Could not read file: ${e}`, "error"));
      }
    }).catch(e => console.error("onDragDropEvent registration failed:", e));
  }

  document.getElementById("empty-state-import-btn")?.addEventListener("click", () => {
    document.getElementById("file-input")!.click();
  });
}

export function bindFileInput(toast: ToastFn): void {
  document.getElementById("file-input")!.addEventListener("change", e => {
    const file = (e.target as HTMLInputElement).files?.[0];
    if (file) readAndPreviewFile(file, toast);
    (e.target as HTMLInputElement).value = "";
  });
}

export async function handleWasmDrop(srcPath: string, toast: ToastFn): Promise<void> {
  let pluginDir: string | null = null;
  try {
    pluginDir = await getSetting("plugin_dir");
  } catch {
    pluginDir = null;
  }
  if (!pluginDir) {
    toast("Set a plugin directory in Settings → Plugins first.", "info");
    return;
  }
  // T2-14/S9-5: the .aerini/.json drop path already gates on
  // showImportPreview before anything happens; a dropped .wasm plugin
  // previously installed with zero confirmation despite running with the
  // same trust-sensitive capabilities (S5/S8's SSRF-bypass trust
  // boundary). Match the friction level here.
  const fileName = srcPath.split(/[\\/]/).pop() ?? srcPath;
  const ok = await showConfirm(
    `Install plugin "${fileName}"? Only install plugins from sources you trust.`,
    true,
    "Install",
  );
  if (!ok) return;
  try {
    await installPluginFromPath(srcPath, pluginDir);
    toast("Plugin installed. Restart to activate.", "success");
    showRestartBanner();
  } catch (e) {
    toast(`Plugin install failed: ${e}`, "error");
  }
}

function readAndPreviewFile(file: File, toast: ToastFn): void {
  const reader = new FileReader();
  reader.onload = e => {
    try {
      const obj = JSON.parse(e.target?.result as string);
      showImportPreview(obj);
    } catch {
      toast("Invalid .aerini file — could not parse JSON", "error");
    }
  };
  reader.readAsText(file);
}
