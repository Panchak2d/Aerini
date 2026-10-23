import { showConfirm } from "./confirm";
import {
  probeOllama,
  pullOllamaModel,
  cancelOllamaPull,
  deleteOllamaModel,
  describeModelFetchError,
  type OllamaTarget,
  type OllamaProbe,
  type PullPhase,
  type PullProgress,
} from "./ipc/providers";

/** Decimal units, matching what Ollama itself reports. */
export function formatBytes(n: number): string {
  if (!Number.isFinite(n) || n < 0) return "0 B";
  if (n < 1000) return `${Math.round(n)} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = n;
  let i = -1;
  do {
    value /= 1000;
    i++;
  } while (value >= 1000 && i < units.length - 1);
  return `${value.toFixed(i >= 2 ? 1 : 0)} ${units[i]}`;
}

export function formatDuration(seconds: number): string {
  if (seconds < 60) return "less than a minute";
  if (seconds < 3600) return `${Math.ceil(seconds / 60)} min`;
  const h = Math.floor(seconds / 3600);
  const m = Math.ceil((seconds % 3600) / 60);
  return m === 60 ? `${h + 1} h` : `${h} h ${m} min`;
}

export function describePhase(phase: PullPhase): string {
  switch (phase) {
    case "manifest":    return "Contacting the model registry…";
    case "downloading": return "Downloading";
    case "verifying":   return "Verifying the download…";
    case "finalizing":  return "Finishing up…";
    default:            return "Working…";
  }
}

const SPEED_WINDOW_MS = 5000;
const MIN_SPEED_SPAN_MS = 1000;
const MAX_ETA_SECONDS = 24 * 3600;

/** Turns raw progress steps into a bar fraction, speed and time left. The
 *  fraction never moves backwards: the total can grow while a download is
 *  still announcing its layers. */
export class PullMeter {
  private samples: Array<{ at: number; completed: number }> = [];
  private best = 0;
  private sawTotal = false;
  private latest: PullProgress = { phase: "manifest", completed: 0, total: 0 };

  push(p: PullProgress, now: number): void {
    this.latest = p;
    if (p.total > 0) {
      this.sawTotal = true;
      this.best = Math.max(this.best, Math.min(1, p.completed / p.total));
    }
    this.samples.push({ at: now, completed: p.completed });
    const cutoff = now - SPEED_WINDOW_MS;
    while (this.samples.length > 2 && this.samples[1].at < cutoff) this.samples.shift();
  }

  /** `null` until a total is known (an indeterminate bar). */
  fraction(): number | null {
    return this.sawTotal ? this.best : null;
  }

  speedBytesPerSecond(): number | null {
    if (this.samples.length < 2) return null;
    const first = this.samples[0];
    const last = this.samples[this.samples.length - 1];
    const span = last.at - first.at;
    if (span < MIN_SPEED_SPAN_MS) return null;
    const rate = ((last.completed - first.completed) * 1000) / span;
    return rate > 0 ? rate : null;
  }

  etaSeconds(): number | null {
    const speed = this.speedBytesPerSecond();
    if (speed === null || this.latest.total <= 0) return null;
    const eta = Math.max(0, this.latest.total - this.latest.completed) / speed;
    return eta > MAX_ETA_SECONDS ? null : eta;
  }

  detail(): string {
    const { completed, total, phase } = this.latest;
    if (phase !== "downloading" || total <= 0) return "";
    const parts = [`${formatBytes(completed)} of ${formatBytes(total)}`];
    const speed = this.speedBytesPerSecond();
    if (speed !== null) parts.push(`${formatBytes(speed)}/s`);
    const eta = this.etaSeconds();
    if (eta !== null) parts.push(eta < 60 ? "less than a minute left" : `about ${formatDuration(eta)} left`);
    return parts.join(" · ");
  }
}

export interface OllamaControlHost {
  /** The server to manage, or `null` when the node/panel isn't pointed at a
   *  local server yet (no probe is sent then). */
  getTarget(): OllamaTarget | null;
  /** The Model field's current text. */
  getModel(): string;
  /** Called after a pull or delete changed what the server has installed. */
  onModelsChanged(): void;
}

export interface OllamaControl {
  el: HTMLElement;
  /** Looks for an Ollama server again, e.g. after Provider or Base URL changed. */
  refresh(): void;
  /** Stops any running download and drops pending work. Safe to call twice. */
  dispose(): void;
}

type Busy = "pulling" | "cancelling" | "deleting" | null;

const PROBE_DEBOUNCE_MS = 250;

function hostLabel(target: OllamaTarget): string {
  try {
    return new URL(target.base_url).host || target.base_url;
  } catch {
    return target.base_url;
  }
}

function newPullId(): string {
  return typeof crypto !== "undefined" && "randomUUID" in crypto
    ? crypto.randomUUID()
    : `pull-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

/** Pull and delete controls for an Ollama server, shown only after the
 *  configured server answered as Ollama. Nothing here runs at workflow run
 *  time: every download or removal is a deliberate click. */
export function mountOllamaModelControl(host: OllamaControlHost): OllamaControl {
  const root = document.createElement("div");
  root.className = "ollama-ctl";
  root.hidden = true;

  const row = document.createElement("div");
  row.className = "ollama-ctl-row";
  const pullBtn = document.createElement("button");
  pullBtn.type = "button";
  pullBtn.className = "code-load-btn";
  pullBtn.textContent = "Pull model";
  const deleteBtn = document.createElement("button");
  deleteBtn.type = "button";
  deleteBtn.className = "code-load-btn is-danger";
  deleteBtn.textContent = "Delete model";
  row.append(pullBtn, deleteBtn);

  const progress = document.createElement("div");
  progress.className = "ollama-ctl-progress";
  progress.hidden = true;
  const phaseEl = document.createElement("div");
  phaseEl.className = "config-hint";
  const bar = document.createElement("div");
  bar.className = "ollama-bar is-indeterminate";
  bar.setAttribute("role", "progressbar");
  bar.setAttribute("aria-label", "Download progress");
  bar.setAttribute("aria-valuemin", "0");
  bar.setAttribute("aria-valuemax", "100");
  const fill = document.createElement("div");
  fill.className = "ollama-bar-fill";
  bar.appendChild(fill);
  const detailEl = document.createElement("div");
  detailEl.className = "config-hint";
  const cancelBtn = document.createElement("button");
  cancelBtn.type = "button";
  cancelBtn.className = "code-load-btn";
  cancelBtn.textContent = "Cancel";
  const stopNote = document.createElement("div");
  stopNote.className = "config-hint";
  stopNote.textContent = "Closing this panel stops the download. Pull again later to continue where it left off.";
  progress.append(phaseEl, bar, detailEl, cancelBtn, stopNote);

  const status = document.createElement("div");
  status.className = "config-hint";
  status.setAttribute("role", "status");
  status.setAttribute("aria-live", "polite");

  root.append(row, progress, status);

  let disposed = false;
  let busy: Busy = null;
  let currentPullId = "";
  let meter = new PullMeter();
  let probeSeq = 0;
  let probeTimer: ReturnType<typeof setTimeout> | undefined;
  let retryLabel = false;
  let probeWhileBusy = false;

  function settle(): void {
    if (probeWhileBusy && !disposed) {
      probeWhileBusy = false;
      refresh();
    }
  }

  function setStatus(text: string, warn = false): void {
    status.textContent = text;
    status.className = warn ? "config-hint config-hint-warn" : "config-hint";
  }

  function render(): void {
    const working = busy !== null;
    pullBtn.hidden = busy === "pulling" || busy === "cancelling";
    pullBtn.textContent = retryLabel ? "Pull again" : "Pull model";
    pullBtn.disabled = working;
    deleteBtn.disabled = working;
    deleteBtn.hidden = busy === "pulling" || busy === "cancelling";
    progress.hidden = !(busy === "pulling" || busy === "cancelling");
    cancelBtn.disabled = busy === "cancelling";
    cancelBtn.textContent = busy === "cancelling" ? "Cancelling…" : "Cancel";
  }

  function renderProgress(p: PullProgress): void {
    meter.push(p, Date.now());
    phaseEl.textContent = describePhase(p.phase);
    detailEl.textContent = meter.detail();
    const fraction = meter.fraction();
    if (fraction === null) {
      bar.classList.add("is-indeterminate");
      bar.removeAttribute("aria-valuenow");
      fill.style.width = "";
    } else {
      const pct = Math.round(fraction * 100);
      bar.classList.remove("is-indeterminate");
      bar.setAttribute("aria-valuenow", String(pct));
      fill.style.width = `${pct}%`;
    }
  }

  function focusIsInside(): boolean {
    return root.contains(document.activeElement);
  }

  /** Hiding or disabling the button that was just used would drop keyboard
   *  focus onto the page. Hand it to `el` unless the person moved on. */
  function restoreFocus(el: HTMLElement, hadFocus: boolean): void {
    if (!hadFocus || disposed) return;
    const active = document.activeElement;
    if (active === null || active === document.body || root.contains(active)) el.focus();
  }

  function hideAll(): void {
    root.hidden = true;
    busy = null;
  }

  function showReady(): void {
    root.hidden = false;
    row.hidden = false;
    render();
  }

  async function runProbe(): Promise<void> {
    if (disposed) return;
    if (busy) {
      probeWhileBusy = true;
      return;
    }
    const target = host.getTarget();
    if (!target) {
      hideAll();
      return;
    }
    const seq = ++probeSeq;
    let result: OllamaProbe | null = null;
    try {
      result = await probeOllama(target);
    } catch {
      result = null;
    }
    if (disposed || seq !== probeSeq || busy) return;
    if (result?.ollama) {
      setStatus("");
      showReady();
    } else {
      hideAll();
    }
  }

  function refresh(): void {
    if (disposed) return;
    clearTimeout(probeTimer);
    probeTimer = setTimeout(() => { void runProbe(); }, PROBE_DEBOUNCE_MS);
  }

  /** A fresh probe at click time, so a Base URL edited since the control
   *  appeared can't send a download to a server that isn't Ollama. */
  async function confirmServer(target: OllamaTarget): Promise<OllamaProbe | null> {
    try {
      const result = await probeOllama(target);
      if (result.ollama) return result;
    } catch {
      /* falls through to the notice below */
    }
    setStatus("This server doesn't look like Ollama anymore, so model management is unavailable.", true);
    hideAll();
    root.hidden = false;
    row.hidden = true;
    return null;
  }

  async function onPull(): Promise<void> {
    if (busy || disposed) return;
    const model = host.getModel().trim();
    const target = host.getTarget();
    if (!target) {
      hideAll();
      return;
    }
    if (!model) {
      setStatus("Type a model name above first, for example llama3.2.", true);
      return;
    }
    row.hidden = false;
    setStatus("");
    const server = await confirmServer(target);
    if (!server || disposed) return;
    if (!server.loopback) {
      const ok = await showConfirm(
        `Download "${model}" onto ${hostLabel(target)}? That server isn't on this computer, so the download uses its disk space and network.`,
        false,
        "Download",
        "neutral",
      );
      if (!ok || disposed) return;
    }

    const hadFocus = focusIsInside();
    const pullId = newPullId();
    currentPullId = pullId;
    meter = new PullMeter();
    busy = "pulling";
    renderProgress({ phase: "manifest", completed: 0, total: 0 });
    render();
    restoreFocus(cancelBtn, hadFocus);
    try {
      const result = await pullOllamaModel(pullId, target, model, (p) => {
        if (disposed || currentPullId !== pullId) return;
        renderProgress(p);
      });
      if (disposed) return;
      busy = null;
      if (result === "cancelled") {
        retryLabel = true;
        setStatus("Download stopped. Pull again to continue where it left off.");
      } else {
        retryLabel = false;
        setStatus(`Downloaded ${model}.`);
        host.onModelsChanged();
      }
    } catch (err) {
      if (disposed) return;
      busy = null;
      retryLabel = true;
      setStatus(describeModelFetchError(err), true);
    } finally {
      if (!disposed) {
        render();
        restoreFocus(pullBtn, hadFocus);
        settle();
      }
    }
  }

  async function onCancel(): Promise<void> {
    if (busy !== "pulling") return;
    busy = "cancelling";
    render();
    try {
      await cancelOllamaPull(currentPullId);
    } catch {
      /* the pull may have just ended; its own result settles the state */
    }
  }

  async function onDelete(): Promise<void> {
    if (busy || disposed) return;
    const model = host.getModel().trim();
    const target = host.getTarget();
    if (!target) {
      hideAll();
      return;
    }
    if (!model) {
      setStatus("Type the name of the model to delete above first.", true);
      return;
    }
    row.hidden = false;
    setStatus("");
    const server = await confirmServer(target);
    if (!server || disposed) return;
    const ok = await showConfirm(
      `Delete "${model}" from ${hostLabel(target)}? This frees its disk space. You can download it again later.`,
      true,
      "Delete model",
      "danger",
    );
    if (!ok || disposed) return;

    const hadFocus = focusIsInside();
    busy = "deleting";
    render();
    setStatus(`Deleting ${model}…`);
    try {
      await deleteOllamaModel(target, model);
      if (disposed) return;
      busy = null;
      setStatus(`Deleted ${model}.`);
      host.onModelsChanged();
    } catch (err) {
      if (disposed) return;
      busy = null;
      setStatus(describeModelFetchError(err), true);
    } finally {
      if (!disposed) {
        render();
        restoreFocus(deleteBtn, hadFocus);
        settle();
      }
    }
  }

  pullBtn.addEventListener("click", () => { void onPull(); });
  deleteBtn.addEventListener("click", () => { void onDelete(); });
  cancelBtn.addEventListener("click", () => { void onCancel(); });
  render();
  refresh();

  return {
    el: root,
    refresh,
    dispose(): void {
      if (disposed) return;
      disposed = true;
      clearTimeout(probeTimer);
      if (busy === "pulling" || busy === "cancelling") {
        void cancelOllamaPull(currentPullId).catch(() => {});
      }
    },
  };
}
