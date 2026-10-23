/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import type { OllamaProbe, PullProgress } from "../ipc/providers";

const probeMock = vi.fn<(t: unknown) => Promise<OllamaProbe>>();
const pullMock = vi.fn<(id: string, t: unknown, m: string, cb: (p: PullProgress) => void) => Promise<"done" | "cancelled">>();
const cancelMock = vi.fn<(id: string) => Promise<void>>();

vi.mock("../ipc/providers", async () => {
  const actual = await vi.importActual<typeof import("../ipc/providers")>("../ipc/providers");
  return {
    ...actual,
    probeOllama: (t: unknown) => probeMock(t),
    pullOllamaModel: (...a: Parameters<typeof pullMock>) => pullMock(...a),
    cancelOllamaPull: (id: string) => cancelMock(id),
    deleteOllamaModel: vi.fn(() => Promise.resolve()),
  };
});
vi.mock("../confirm", () => ({ showConfirm: vi.fn(() => Promise.resolve(true)) }));

import { formatBytes, formatDuration, PullMeter, mountOllamaModelControl } from "../ollama-model-control";

const TARGET = { provider: "local", base_url: "http://localhost:11434/v1" };
const OLLAMA: OllamaProbe = { ollama: true, version: "0.30.0", loopback: true, reason: null };

describe("pull progress formatting", () => {
  it("formats sizes in decimal units and durations in plain words", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(1500)).toBe("2 KB");
    expect(formatBytes(4_700_000_000)).toBe("4.7 GB");
    expect(formatBytes(-1)).toBe("0 B");
    expect(formatDuration(30)).toBe("less than a minute");
    expect(formatDuration(150)).toBe("3 min");
    expect(formatDuration(3 * 3600 + 60)).toBe("3 h 1 min");
  });

  it("keeps the bar fraction monotonic and stays indeterminate until a total is known", () => {
    const m = new PullMeter();
    m.push({ phase: "manifest", completed: 0, total: 0 }, 0);
    expect(m.fraction()).toBeNull();
    m.push({ phase: "downloading", completed: 50, total: 100 }, 1000);
    m.push({ phase: "downloading", completed: 60, total: 400 }, 2000);
    expect(m.fraction()).toBe(0.5);
  });

  it("reports speed and time left, and never says 'about less than a minute'", () => {
    const m = new PullMeter();
    m.push({ phase: "downloading", completed: 0, total: 10_000 }, 0);
    m.push({ phase: "downloading", completed: 5_000, total: 10_000 }, 2000);
    expect(m.speedBytesPerSecond()).toBe(2500);
    expect(m.detail()).toBe("5 KB of 10 KB · 3 KB/s · less than a minute left");
  });
});

describe("mountOllamaModelControl", () => {
  const flush = () => vi.advanceTimersByTimeAsync(300);
  let changed: ReturnType<typeof vi.fn>;
  let model: string;

  beforeEach(() => {
    vi.useFakeTimers();
    probeMock.mockReset().mockResolvedValue(OLLAMA);
    pullMock.mockReset();
    cancelMock.mockReset().mockResolvedValue();
    changed = vi.fn();
    model = "llama3.2";
  });
  afterEach(() => vi.useRealTimers());

  const mount = () =>
    mountOllamaModelControl({ getTarget: () => TARGET, getModel: () => model, onModelsChanged: changed as () => void });

  it("stays hidden unless the probe says Ollama", async () => {
    probeMock.mockResolvedValue({ ollama: false, version: null, loopback: true, reason: "not_ollama" });
    const c = mount();
    await flush();
    expect(c.el.hidden).toBe(true);
    probeMock.mockResolvedValue(OLLAMA);
    c.refresh();
    await flush();
    expect(c.el.hidden).toBe(false);
    c.dispose();
  });

  it("pulls on click and asks the host to refresh its model list when done", async () => {
    pullMock.mockResolvedValue("done");
    const c = mount();
    await flush();
    (c.el.querySelector("button") as HTMLButtonElement).click();
    await vi.advanceTimersByTimeAsync(0);
    expect(pullMock).toHaveBeenCalledTimes(1);
    expect(pullMock.mock.calls[0][2]).toBe("llama3.2");
    expect(changed).toHaveBeenCalledTimes(1);
    expect(c.el.querySelector("[role=status]")?.textContent).toBe("Downloaded llama3.2.");
    c.dispose();
  });

  it("cancels a running download when disposed", async () => {
    pullMock.mockReturnValue(new Promise(() => {}));
    const c = mount();
    await flush();
    (c.el.querySelector("button") as HTMLButtonElement).click();
    await vi.advanceTimersByTimeAsync(0);
    const pullId = pullMock.mock.calls[0][0];
    c.dispose();
    c.dispose();
    expect(cancelMock).toHaveBeenCalledTimes(1);
    expect(cancelMock).toHaveBeenCalledWith(pullId);
  });
});

describe("mountOllamaModelControl: settings edited mid-download", () => {
  it("probes again once the running pull ends", async () => {
    vi.useFakeTimers();
    probeMock.mockReset().mockResolvedValue(OLLAMA);
    pullMock.mockReset();
    let finish: (v: "done") => void = () => {};
    pullMock.mockReturnValue(new Promise<"done">((r) => { finish = r; }));
    const c = mountOllamaModelControl({ getTarget: () => TARGET, getModel: () => "llama3.2", onModelsChanged: () => {} });
    await vi.advanceTimersByTimeAsync(300);
    (c.el.querySelector("button") as HTMLButtonElement).click();
    await vi.advanceTimersByTimeAsync(0);
    probeMock.mockClear();

    c.refresh();
    await vi.advanceTimersByTimeAsync(300);
    expect(probeMock).not.toHaveBeenCalled();

    finish("done");
    await vi.advanceTimersByTimeAsync(300);
    expect(probeMock).toHaveBeenCalledTimes(1);
    c.dispose();
    vi.useRealTimers();
  });
});
