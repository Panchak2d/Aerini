import { showConfirm } from "./confirm";
import { isTauri } from "./utils";
import {
  checkForUpdate, installUpdate, cancelUpdateDownload, takeUpdateNotice, listenUpdateProgress,
} from "./ipc/update";
import type { InstallOutcome, UpdateCheckResult } from "./ipc/update";

const RELEASES_URL = "https://github.com/Panchak2d/aerini/releases/latest";
const TRUSTED_URL_PREFIX = "https://github.com/Panchak2d/aerini/";

export type UpdateState =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "up_to_date"; version: string }
  | { kind: "available"; current: string; latest: string; releaseUrl: string; adminPrompt: boolean }
  | { kind: "manual"; latest: string; releaseUrl: string; message: string }
  | { kind: "confirming" }
  | { kind: "downloading"; latest: string; downloaded: number; total: number | null; cancelling: boolean }
  | { kind: "installing"; latest: string }
  | { kind: "error"; message: string; releaseUrl: string };

export interface UpdateView {
  status: string;
  link: { label: string; href: string } | null;
  /** `percent: null` is an indeterminate bar. */
  progress: { percent: number | null } | null;
  showInstall: boolean;
  showCancel: boolean;
  checkDisabled: boolean;
  cancelDisabled: boolean;
}

export interface UpdateEls {
  status: HTMLElement;
  progressWrap: HTMLElement;
  progress: HTMLElement;
  percent: HTMLElement;
  checkBtn: HTMLButtonElement;
  installBtn: HTMLButtonElement;
  cancelBtn: HTMLButtonElement;
}

const fmtVersion = (v: string): string => `v${v.replace(/^v/i, "")}`;

const safeHref = (url: string): string =>
  url.startsWith(TRUSTED_URL_PREFIX) ? url : RELEASES_URL;

const errorText = (err: unknown): string =>
  typeof err === "string" ? err : err instanceof Error ? err.message : String(err);

export function stateFromCheck(r: UpdateCheckResult): UpdateState {
  if (!r.available || !r.latest_version) return { kind: "up_to_date", version: r.current_version };
  if (r.install_support.kind === "manual") {
    return {
      kind: "manual",
      latest: r.latest_version,
      releaseUrl: r.release_url,
      message: r.install_support.message,
    };
  }
  return {
    kind: "available",
    current: r.current_version,
    latest: r.latest_version,
    releaseUrl: r.release_url,
    adminPrompt: r.install_support.admin_prompt,
  };
}

const IDLE_TEXT =
  "Checks GitHub for a newer release only when you click. Nothing is checked in the background.";

export function viewFor(state: UpdateState): UpdateView {
  const base: UpdateView = {
    status: "",
    link: null,
    progress: null,
    showInstall: false,
    showCancel: false,
    checkDisabled: false,
    cancelDisabled: false,
  };
  switch (state.kind) {
    case "idle":
      return { ...base, status: IDLE_TEXT };
    case "checking":
      return { ...base, status: "Checking for updates\u2026", checkDisabled: true };
    case "up_to_date":
      return { ...base, status: `You're up to date (${fmtVersion(state.version)}).` };
    case "available": {
      const admin = state.adminPrompt
        ? " Your system will ask for an administrator password to install it."
        : "";
      return {
        ...base,
        status:
          `${fmtVersion(state.latest)} is available (you have ${fmtVersion(state.current)}). ` +
          `Your workflows, credentials and settings are kept.${admin}`,
        link: { label: "View release notes", href: safeHref(state.releaseUrl) },
        showInstall: true,
      };
    }
    case "manual":
      return {
        ...base,
        status: `${fmtVersion(state.latest)} is available. ${state.message}`,
        link: { label: "Download from GitHub", href: safeHref(state.releaseUrl) },
      };
    case "confirming":
      return { ...base, status: "Waiting for your confirmation\u2026", checkDisabled: true };
    case "downloading": {
      const { downloaded, total } = state;
      const percent =
        total !== null && total > 0 ? Math.min(100, Math.round((downloaded / total) * 100)) : null;
      return {
        ...base,
        status: state.cancelling
          ? "Cancelling\u2026"
          : `Downloading ${fmtVersion(state.latest)}\u2026`,
        progress: { percent },
        showCancel: true,
        cancelDisabled: state.cancelling,
        checkDisabled: true,
      };
    }
    case "installing":
      return {
        ...base,
        status: `Installing ${fmtVersion(state.latest)}. Aerini will restart in a moment.`,
        progress: { percent: 100 },
        checkDisabled: true,
      };
    case "error":
      return {
        ...base,
        status: state.message,
        link: { label: "Download from GitHub", href: safeHref(state.releaseUrl) },
      };
  }
}

/** Version, message and URL strings reach the DOM only through textContent and a vetted href. */
export function applyView(els: UpdateEls, view: UpdateView): void {
  const sig = `${view.status}\n${view.link?.label ?? ""}\n${view.link?.href ?? ""}`;
  if (els.status.dataset.sig !== sig) {
    els.status.dataset.sig = sig;
    els.status.textContent = view.status;
    if (view.link) {
      const a = document.createElement("a");
      a.textContent = view.link.label;
      a.href = view.link.href;
      a.target = "_blank";
      a.rel = "noopener noreferrer";
      a.className = "settings-support-link";
      els.status.append(" ", a);
    }
  }

  els.progressWrap.hidden = view.progress === null;
  if (view.progress) {
    const { percent } = view.progress;
    if (percent === null) els.progress.removeAttribute("value");
    else els.progress.setAttribute("value", String(percent));
    els.percent.textContent = percent === null ? "" : `${percent}%`;
  }

  els.installBtn.hidden = !view.showInstall;
  els.cancelBtn.hidden = !view.showCancel;
  els.cancelBtn.disabled = view.cancelDisabled;
  els.checkBtn.disabled = view.checkDisabled;
}

function runningMessage(scheduled: number, manual: number): string {
  const parts: string[] = [];
  if (scheduled > 0) parts.push(`${scheduled} background ${scheduled === 1 ? "run" : "runs"}`);
  if (manual > 0) parts.push(`${manual} workflow ${manual === 1 ? "run" : "runs"}`);
  const total = scheduled + manual;
  return (
    `${parts.join(" and ")} ${total === 1 ? "is" : "are"} still running. ` +
    `Installing now stops ${total === 1 ? "it" : "them"} and restarts Aerini. Install anyway?`
  );
}

export interface UpdateUiDeps {
  toast: (msg: string, type?: "success" | "error" | "info") => void;
  hasUnsaved: () => boolean;
}

function lookupEls(): UpdateEls | null {
  const get = <T extends HTMLElement>(id: string) => document.getElementById(id) as T | null;
  const els = {
    status: get("update-check-status"),
    progressWrap: get("update-progress-wrap"),
    progress: get("update-progress"),
    percent: get("update-percent"),
    checkBtn: get<HTMLButtonElement>("btn-check-updates"),
    installBtn: get<HTMLButtonElement>("btn-install-update"),
    cancelBtn: get<HTMLButtonElement>("btn-cancel-update"),
  };
  return Object.values(els).every(Boolean) ? (els as UpdateEls) : null;
}

export function bindUpdateUi({ toast, hasUnsaved }: UpdateUiDeps): void {
  const row = document.getElementById("update-row");
  const els = lookupEls();
  if (!row || !els) return;
  if (!isTauri()) {
    row.hidden = true;
    return;
  }

  let state: UpdateState = { kind: "idle" };
  const current = (): UpdateState => state;
  const set = (next: UpdateState): void => {
    state = next;
    applyView(els, viewFor(next));
  };
  set(state);

  async function runCheck(): Promise<void> {
    const s = current().kind;
    if (s === "checking" || s === "confirming" || s === "downloading" || s === "installing") return;
    set({ kind: "checking" });
    try {
      set(stateFromCheck(await checkForUpdate()));
    } catch (err) {
      set({ kind: "error", message: errorText(err), releaseUrl: RELEASES_URL });
    }
  }

  async function attempt(
    force: boolean,
    latest: string,
    releaseUrl: string,
  ): Promise<InstallOutcome | null> {
    set({ kind: "downloading", latest, downloaded: 0, total: null, cancelling: false });
    try {
      return await installUpdate(force);
    } catch (err) {
      set({ kind: "error", message: errorText(err), releaseUrl });
      return null;
    }
  }

  async function runInstall(): Promise<void> {
    const start = current();
    if (start.kind !== "available") return;
    const { latest, releaseUrl } = start;

    set({ kind: "confirming" });
    if (hasUnsaved()) {
      const proceed = await showConfirm(
        "You have changes that may not be saved yet. Aerini restarts to finish the update. Install anyway?",
        true,
        "Install anyway",
        "danger",
      );
      if (!proceed) {
        set(start);
        return;
      }
    }

    let outcome = await attempt(false, latest, releaseUrl);
    if (outcome?.status === "active_runs") {
      set({ kind: "confirming" });
      const proceed = await showConfirm(
        runningMessage(outcome.scheduled, outcome.manual),
        true,
        "Stop and install",
        "danger",
      );
      if (!proceed) {
        set(start);
        return;
      }
      outcome = await attempt(true, latest, releaseUrl);
    }
    if (outcome) set(start);
  }

  async function runCancel(): Promise<void> {
    const s = current();
    if (s.kind !== "downloading" || s.cancelling) return;
    set({ ...s, cancelling: true });
    try {
      await cancelUpdateDownload();
    } catch {
      const now = current();
      if (now.kind === "downloading") set({ ...now, cancelling: false });
    }
  }

  els.checkBtn.addEventListener("click", () => { void runCheck(); });
  els.installBtn.addEventListener("click", () => { void runInstall(); });
  els.cancelBtn.addEventListener("click", () => { void runCancel(); });

  listenUpdateProgress((p) => {
    const s = current();
    if (s.kind !== "downloading") return;
    if (p.stage === "installing") set({ kind: "installing", latest: s.latest });
    else set({ ...s, downloaded: p.downloaded, total: p.total });
  }).catch(console.error);

  takeUpdateNotice()
    .then((notice) => {
      if (!notice) return;
      const v = fmtVersion(notice.version);
      if (notice.kind === "installed") {
        toast(`Updated to ${v}`, "success");
        return;
      }
      const message = `The update to ${v} did not finish installing. Click Check for Updates to try again.`;
      toast(message, "error");
      set({ kind: "error", message, releaseUrl: RELEASES_URL });
    })
    .catch(() => {});
}
