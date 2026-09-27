import {
  getSetting, setSetting, pickFolderDialog, pickPluginFileDialog,
  listInstalledPlugins, installPluginFromPath, removePlugin, reloadPlugins,
  installPluginPackFromPath, removePluginPack,
} from "./ipc/workflow";
import type { PluginInfo } from "./ipc/workflow";
import { escapeHtml } from "./utils";
import { showConfirm } from "./confirm";

type ToastFn = (msg: string, type: "success" | "error" | "info") => void;

let _pluginDir: string | null = null;
let _plugins: PluginInfo[] = [];
let _loaded = false;

/**
 * Maps a `signature_status` string to a display class. "Verified — trusted
 * publisher" is the one positive case; "Unsigned" is neutral (the backend's
 * own comment on `signature_status_message` calls plain-unsigned expected
 * and common, not suspicious); every other status names an actual problem
 * (unrecognized, malformed, integrity mismatch, invalid, untrusted key) and
 * is styled as a warning.
 */
export function signatureStatusClass(status: string): "positive" | "neutral" | "warning" {
  if (status === "Verified — trusted publisher") return "positive";
  if (status.startsWith("Unsigned")) return "neutral";
  return "warning";
}

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

  const reloadBtn = document.getElementById("btn-reload-plugins") as HTMLButtonElement | null;
  if (reloadBtn) reloadBtn.disabled = !_pluginDir;
}

function renderPluginItem(p: PluginInfo): string {
  return `
    <div class="plugin-list-item" data-filename="${escapeHtml(p.filename)}">
      <div class="plugin-list-item-info">
        <div class="plugin-list-item-name" title="${escapeHtml(p.display_name)}">${escapeHtml(p.display_name)}</div>
        ${p.load_error
          ? `<div class="plugin-list-item-error">${escapeHtml(p.load_error)}</div>`
          : `<div class="plugin-list-item-meta">${escapeHtml(p.type_id)} · ${escapeHtml(p.filename)}</div>`}
        ${p.registry_warning ? `<div class="plugin-list-item-warning">${escapeHtml(p.registry_warning)}</div>` : ""}
        ${p.signature_status
          ? `<div class="plugin-list-item-signature plugin-list-item-signature--${signatureStatusClass(p.signature_status)}">${escapeHtml(p.signature_status)}</div>`
          : ""}
      </div>
      <button type="button" class="plugin-item-remove" data-filename="${escapeHtml(p.filename)}"
        data-tooltip="Remove plugin &quot;${escapeHtml(p.display_name)}&quot;"
        aria-label="Remove plugin ${escapeHtml(p.display_name)}">
        <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
      </button>
    </div>
  `;
}

/**
 * Renders `_plugins` grouped by `pack_id`: the first entry belonging to a
 * given pack emits a pack group (header + every member of that pack, in
 * their original list order) with a single "Remove pack" action; every
 * later entry belonging to the same pack is skipped since it was already
 * rendered as part of that group. A `null` pack_id renders exactly as a
 * standalone item always has.
 */
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

  const renderedPacks = new Set<string>();
  el.innerHTML = _plugins.map(p => {
    if (!p.pack_id) return renderPluginItem(p);
    if (renderedPacks.has(p.pack_id)) return "";
    renderedPacks.add(p.pack_id);
    const members = _plugins.filter(m => m.pack_id === p.pack_id);
    const packName = p.pack_display_name ?? p.pack_id;
    return `
      <div class="plugin-pack-group" data-pack-id="${escapeHtml(p.pack_id)}">
        <div class="plugin-pack-header">
          <span class="plugin-pack-name">${escapeHtml(packName)}</span>
          <button class="btn-sm btn-plugin-pack-remove" data-pack-id="${escapeHtml(p.pack_id)}">Remove pack</button>
        </div>
        <div class="plugin-pack-members">
          ${members.map(renderPluginItem).join("")}
        </div>
      </div>
    `;
  }).join("");

  el.querySelectorAll<HTMLButtonElement>(".plugin-item-remove").forEach(btn => {
    btn.addEventListener("click", () => {
      const p = _plugins.find(pl => pl.filename === btn.dataset.filename);
      onRemovePlugin(btn.dataset.filename!, p?.display_name ?? btn.dataset.filename!, _toast);
    });
  });
  el.querySelectorAll<HTMLButtonElement>(".btn-plugin-pack-remove").forEach(btn => {
    btn.addEventListener("click", () => {
      const packId = btn.dataset.packId!;
      const packName = _plugins.find(pl => pl.pack_id === packId)?.pack_display_name ?? packId;
      onRemovePluginPack(packId, packName, _toast);
    });
  });
}

let _toast: ToastFn = () => {};

/** Set by `bindPluginSettings`. Re-fetches node descriptors and pushes them
 *  into every panel/registry that caches a snapshot, so a successful
 *  install/remove/reload shows up without an app restart. Not called when
 *  `reloadPlugins` itself fails — the backend registry didn't change, so
 *  there's nothing new to show. */
let _onNodesReloaded: (() => void | Promise<void>) | null = null;

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
  const btn = document.getElementById("btn-plugin-dir-browse") as HTMLButtonElement | null;
  if (btn) btn.disabled = true;
  try {
    await setSetting("plugin_dir", path);
    _pluginDir = path;
    renderDirPath();
    try {
      const report = await reloadPlugins(path);
      await _onNodesReloaded?.();
      const count = report.loaded.length;
      _toast(
        count > 0
          ? `Plugin folder set. ${count} plugin${count === 1 ? "" : "s"} loaded.`
          : "Plugin folder set. No plugins found in it yet.",
        "success",
      );
    } catch (reloadErr) {
      _toast(`Plugin folder set, but reload failed (${reloadErr}). Restart to apply.`, "info");
      showRestartBanner();
    }
    await refreshPlugins();
  } finally {
    if (btn) btn.disabled = false;
  }
}

// Both buttons show a disabled + relabeled state for the duration of the
// async call (install/reload can take a moment and give zero feedback
// otherwise) using .btn-sm's existing :disabled treatment -- no new CSS.
// The label is captured/restored rather than hardcoded so it stays correct
// if the button's own text ever changes.
async function onInstall(toast: ToastFn): Promise<void> {
  if (!_pluginDir) return;
  const srcPath = await pickPluginFileDialog().catch(() => null);
  if (!srcPath) return;
  const btn = document.getElementById("btn-install-plugin") as HTMLButtonElement | null;
  const originalLabel = btn?.textContent ?? "";
  if (btn) { btn.disabled = true; btn.textContent = "Installing…"; }
  try {
    await installPluginOrPackWithUpdatePrompt(srcPath, _pluginDir, toast);
  } finally {
    if (btn) { btn.disabled = !_pluginDir; btn.textContent = originalLabel; }
  }
}

/**
 * Single entry point for installing a picked or dropped file, whichever of
 * `.wasm` (a standalone plugin) or `.aerinipkg` (a multi-node pack) it is —
 * dispatches on `srcPath`'s extension. Both the Plugins tab (`onInstall`)
 * and native drag-drop install (`drag-drop.ts`) go through this instead of
 * each reimplementing the extension check.
 */
export async function installPluginOrPackWithUpdatePrompt(
  srcPath: string,
  pluginDir: string,
  toast: ToastFn,
): Promise<void> {
  if (srcPath.toLowerCase().endsWith(".aerinipkg")) {
    await installPackWithUpdatePrompt(srcPath, pluginDir, toast);
  } else {
    await installWithUpdatePrompt(srcPath, pluginDir, toast);
  }
}

/**
 * Installs `srcPath` into `pluginDir`. On a `plugin_already_installed:`-
 * prefixed rejection (the exact prefix `install_plugin_from_path` returns for
 * an identity match, matched here rather than the full message), offers an
 * update confirmation and retries with `overwrite: true` if accepted.
 *
 * A successful install/update reloads the plugin registry immediately, so
 * the plugin is live with no restart. If the reload call itself fails (the
 * file is on disk, but the running registry couldn't be rebuilt), that's
 * surfaced as a distinct, non-alarming message and the restart banner is
 * shown as a fallback path — installation itself already succeeded.
 */
export async function installWithUpdatePrompt(
  srcPath: string,
  pluginDir: string,
  toast: ToastFn,
  overwrite = false,
): Promise<void> {
  try {
    await installPluginFromPath(srcPath, pluginDir, overwrite);
    try {
      await reloadPlugins(pluginDir);
      await _onNodesReloaded?.();
      toast(overwrite ? "Plugin updated and reloaded." : "Plugin installed and activated.", "success");
    } catch (reloadErr) {
      toast(
        `${overwrite ? "Plugin updated" : "Plugin installed"}, but reload failed (${reloadErr}). Restart to apply.`,
        "info",
      );
      showRestartBanner();
    }
    if (pluginDir === _pluginDir) await refreshPlugins();
  } catch (e) {
    const prefix = "plugin_already_installed:";
    const message = String(e);
    if (!overwrite && message.startsWith(prefix)) {
      const detail = message.slice(prefix.length).trim();
      const ok = await showConfirm(`${detail}. Update it?`, false, "Update");
      if (ok) await installWithUpdatePrompt(srcPath, pluginDir, toast, true);
      return;
    }
    toast(`Plugin install failed: ${message}`, "error");
  }
}

/** Pack equivalent of `installWithUpdatePrompt` — same shape, matched
 *  against `pack_already_installed:` and calling the pack install command. */
export async function installPackWithUpdatePrompt(
  srcPath: string,
  pluginDir: string,
  toast: ToastFn,
  overwrite = false,
): Promise<void> {
  try {
    await installPluginPackFromPath(srcPath, pluginDir, overwrite);
    try {
      await reloadPlugins(pluginDir);
      await _onNodesReloaded?.();
      toast(overwrite ? "Plugin pack updated and reloaded." : "Plugin pack installed and activated.", "success");
    } catch (reloadErr) {
      toast(
        `${overwrite ? "Plugin pack updated" : "Plugin pack installed"}, but reload failed (${reloadErr}). Restart to apply.`,
        "info",
      );
      showRestartBanner();
    }
    if (pluginDir === _pluginDir) await refreshPlugins();
  } catch (e) {
    const prefix = "pack_already_installed:";
    const message = String(e);
    if (!overwrite && message.startsWith(prefix)) {
      const detail = message.slice(prefix.length).trim();
      const ok = await showConfirm(`${detail}. Update it?`, false, "Update");
      if (ok) await installPackWithUpdatePrompt(srcPath, pluginDir, toast, true);
      return;
    }
    toast(`Plugin pack install failed: ${message}`, "error");
  }
}

async function onRemovePlugin(filename: string, displayName: string, toast: ToastFn): Promise<void> {
  if (!_pluginDir) return;
  const packId = _plugins.find(p => p.filename === filename)?.pack_id ?? null;
  const message = packId
    ? `Remove "${displayName}"? It's part of a plugin pack — removing it individually leaves that pack's other members installed but the pack record inconsistent. Use "Remove pack" instead to remove the whole thing cleanly. Continue anyway?`
    : `Remove plugin "${displayName}"? You'll need to reinstall the .wasm file to use it again.`;
  const ok = await showConfirm(message, true, "Remove");
  if (!ok) return;
  try {
    await removePlugin(filename, _pluginDir);
    try {
      await reloadPlugins(_pluginDir);
      await _onNodesReloaded?.();
      toast("Plugin removed.", "success");
    } catch (reloadErr) {
      toast(`Plugin removed, but reload failed (${reloadErr}). Restart to apply.`, "info");
      showRestartBanner();
    }
    await refreshPlugins();
  } catch (e) {
    toast(`Failed to remove plugin: ${e}`, "error");
  }
}

async function onRemovePluginPack(packId: string, displayName: string, toast: ToastFn): Promise<void> {
  if (!_pluginDir) return;
  const ok = await showConfirm(
    `Remove plugin pack "${displayName}"? All of its plugins will be removed. You'll need to reinstall the .aerinipkg file to use them again.`,
    true,
    "Remove",
  );
  if (!ok) return;
  try {
    await removePluginPack(packId, _pluginDir);
    try {
      await reloadPlugins(_pluginDir);
      await _onNodesReloaded?.();
      toast("Plugin pack removed.", "success");
    } catch (reloadErr) {
      toast(`Plugin pack removed, but reload failed (${reloadErr}). Restart to apply.`, "info");
      showRestartBanner();
    }
    await refreshPlugins();
  } catch (e) {
    toast(`Failed to remove plugin pack: ${e}`, "error");
  }
}

async function onManualReload(toast: ToastFn): Promise<void> {
  if (!_pluginDir) return;
  const btn = document.getElementById("btn-reload-plugins") as HTMLButtonElement | null;
  const originalLabel = btn?.textContent ?? "";
  if (btn) { btn.disabled = true; btn.textContent = "Reloading…"; }
  try {
    await reloadPlugins(_pluginDir);
    await _onNodesReloaded?.();
    toast("Plugins reloaded.", "success");
    await refreshPlugins();
  } catch (e) {
    toast(`Plugin reload failed: ${e}`, "error");
  } finally {
    if (btn) { btn.disabled = !_pluginDir; btn.textContent = originalLabel; }
  }
}

/**
 * Wires up the Plugins zone. Loads `plugin_dir` and the installed plugin
 * list the first time the Plugins rail zone is opened (and after any
 * install/remove); subsequent opens reuse the cached list.
 *
 * `onNodesReloaded` runs after every install/remove/reload that succeeds
 * in swapping the backend registry, so callers can refresh whatever else
 * caches a node-descriptor snapshot (sidebar palette, command palette,
 * canvas/popover registries). It does not run when `reloadPlugins` itself
 * throws — the registry didn't change in that case.
 */
export function bindPluginSettings(toast: ToastFn, onNodesReloaded?: () => void | Promise<void>): void {
  _toast = toast;
  _onNodesReloaded = onNodesReloaded ?? null;

  const browseBtn = document.getElementById("btn-plugin-dir-browse");
  const installBtn = document.getElementById("btn-install-plugin");
  const reloadBtn = document.getElementById("btn-reload-plugins");

  browseBtn?.addEventListener("click", () => { onBrowse(); });
  installBtn?.addEventListener("click", () => { onInstall(toast); });
  reloadBtn?.addEventListener("click", () => { onManualReload(toast); });

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
