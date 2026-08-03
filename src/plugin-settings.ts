import {
  getSetting, setSetting, pickFolderDialog, pickWasmFileDialog,
  listInstalledPlugins, installPluginFromPath, removePlugin,
} from "./ipc/workflow";
import type { PluginInfo } from "./ipc/workflow";
import { escapeHtml } from "./utils";
import { showConfirm } from "./confirm";

type ToastFn = (msg: string, type: "success" | "error" | "info") => void;

let _pluginDir: string | null = null;
let _plugins: PluginInfo[] = [];
let _loaded = false;

export function showRestartBanner(): void {
  const banner = document.getElementById("plugin-restart-banner");
  if (banner) banner.hidden = false;
}

function renderDirPath(): void {
  const el = document.getElementById("plugin-dir-path");
  if (!el) return;
  el.textContent = _pluginDir && _pluginDir.length > 0 ? _pluginDir : "Not set";

  const installBtn = document.getElementById("btn-install-plugin") as HTMLButtonElement | null;
  if (installBtn) installBtn.disabled = !_pluginDir;
}

function renderPluginList(): void {
  const el = document.getElementById("plugin-list");
  if (!el) return;

  if (!_pluginDir) {
    el.innerHTML = `<div class="plugin-list-empty">No plugin directory set.</div>`;
    return;
  }
  if (_plugins.length === 0) {
    el.innerHTML = `<div class="plugin-list-empty">No plugins installed.</div>`;
    return;
  }

  el.innerHTML = _plugins.map(p => `
    <div class="plugin-list-item" data-filename="${escapeHtml(p.filename)}">
      <div class="plugin-list-item-info">
        <div class="plugin-list-item-name">${escapeHtml(p.display_name)}</div>
        ${p.load_error
          ? `<div class="plugin-list-item-error">${escapeHtml(p.load_error)}</div>`
          : `<div class="plugin-list-item-meta">${escapeHtml(p.type_id)} · ${escapeHtml(p.filename)}</div>`}
      </div>
      <button class="btn-sm btn-plugin-remove" data-filename="${escapeHtml(p.filename)}">Remove</button>
    </div>
  `).join("");

  el.querySelectorAll<HTMLButtonElement>(".btn-plugin-remove").forEach(btn => {
    btn.addEventListener("click", () => {
      const p = _plugins.find(pl => pl.filename === btn.dataset.filename);
      onRemovePlugin(btn.dataset.filename!, p?.display_name ?? btn.dataset.filename!, _toast);
    });
  });
}

let _toast: ToastFn = () => {};

async function refreshPlugins(): Promise<void> {
  if (!_pluginDir) {
    _plugins = [];
    renderPluginList();
    return;
  }
  try {
    _plugins = await listInstalledPlugins(_pluginDir);
  } catch {
    _plugins = [];
  }
  renderPluginList();
}

async function onBrowse(): Promise<void> {
  const path = await pickFolderDialog().catch(() => null);
  if (!path) return;
  await setSetting("plugin_dir", path);
  _pluginDir = path;
  renderDirPath();
  await refreshPlugins();
}

async function onInstall(toast: ToastFn): Promise<void> {
  if (!_pluginDir) return;
  const srcPath = await pickWasmFileDialog().catch(() => null);
  if (!srcPath) return;

  try {
    await installPluginFromPath(srcPath, _pluginDir);
    toast("Plugin installed. Restart to activate.", "success");
    showRestartBanner();
    await refreshPlugins();
  } catch (e) {
    toast(`Plugin install failed: ${e}`, "error");
  }
}

async function onRemovePlugin(filename: string, displayName: string, toast: ToastFn): Promise<void> {
  if (!_pluginDir) return;
  const ok = await showConfirm(`Remove plugin "${displayName}"? You'll need to reinstall the .wasm file to use it again.`, true, "Remove");
  if (!ok) return;
  try {
    await removePlugin(filename, _pluginDir);
    toast("Plugin removed. Restart to apply.", "success");
    showRestartBanner();
    await refreshPlugins();
  } catch (e) {
    toast(`Failed to remove plugin: ${e}`, "error");
  }
}

/**
 * Wires up the Plugins zone. Loads `plugin_dir` and the installed plugin
 * list the first time the Plugins rail zone is opened (and after any
 * install/remove); subsequent opens reuse the cached list.
 *
 * relocated from the Settings modal (#btn-settings) to its own
 * rail zone (#tab-plugins) — trigger element changed, everything else
 * (the _loaded guard, IPC calls, DOM target ids) is unchanged.
 */
export function bindPluginSettings(toast: ToastFn): void {
  _toast = toast;

  const browseBtn = document.getElementById("btn-plugin-dir-browse");
  const installBtn = document.getElementById("btn-install-plugin");

  browseBtn?.addEventListener("click", () => { onBrowse(); });
  installBtn?.addEventListener("click", () => { onInstall(toast); });

  document.getElementById("tab-plugins")?.addEventListener("click", async () => {
    if (_loaded) return;
    _loaded = true;
    try {
      _pluginDir = await getSetting("plugin_dir");
    } catch {
      _pluginDir = null;
    }
    renderDirPath();
    await refreshPlugins();
  });
}
