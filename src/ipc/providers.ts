import { invoke, Channel } from "@tauri-apps/api/core";

/** Discovers a provider's available model ids via its `/models`-style
 *  endpoint. `provider` accepts `"auto"` — the backend resolves it the same
 *  way AI Prompt/Agent's own execution path does. Rejects on any failure
 *  (bad key, network error, unknown provider); callers must treat that as
 *  "discovery unavailable", never as a reason to block manual model entry. */
export async function listProviderModels(
  provider: string,
  baseUrl: string,
  apiKey: string,
): Promise<string[]> {
  return invoke("list_provider_models", { provider, baseUrl, apiKey });
}

/** Turns a rejected `listProviderModels()` error into a short, readable
 *  reason to show next to the Fetch Models button. The backend always
 *  rejects with `"{CODE}: {message}"` (see `list_provider_models` in
 *  src-tauri/src/commands/providers.rs — MISSING_API_KEY, BAD_KEY,
 *  UNKNOWN_PROVIDER, SSRF_BLOCKED, NETWORK_ERROR, API_ERROR); this strips
 *  the code prefix so the reader sees the actual reason (e.g. "Anthropic
 *  requires an API key") instead of a generic dead end. Falls back to the
 *  raw string when it doesn't match that shape, and to a fixed message when
 *  there's nothing usable at all. */
export function describeModelFetchError(err: unknown): string {
  const raw = err instanceof Error ? err.message : String(err ?? "");
  const m = raw.match(/^[A-Z_]+:\s*(.+)$/s);
  return m?.[1]?.trim() || raw.trim() || "Unknown error";
}

/** The local server a pull, probe or delete is aimed at. `credential_id`
 *  makes the backend read the saved secret itself, so it never reaches the
 *  webview; otherwise `api_key` is sent as typed. */
export interface OllamaTarget {
  provider: string;
  base_url: string;
  api_key?: string;
  credential_id?: string;
}

/** What `ollama_probe` learned. `reason` explains `ollama: false`:
 *  `not_local`, `no_base_url`, `invalid_url`, `blocked`, `unreachable` or
 *  `not_ollama`. */
export interface OllamaProbe {
  ollama: boolean;
  version: string | null;
  loopback: boolean;
  reason: string | null;
}

export type PullPhase = "manifest" | "downloading" | "verifying" | "finalizing" | "working";

/** One progress step, summed over every layer announced so far. */
export interface PullProgress {
  phase: PullPhase;
  completed: number;
  total: number;
}

/** Asks the backend whether the target speaks the Ollama API. Never rejects
 *  for an ordinary failure; a negative answer carries a `reason`. */
export async function probeOllama(target: OllamaTarget): Promise<OllamaProbe> {
  return invoke("ollama_probe", { target });
}

/** Downloads a model, reporting progress until it finishes. Resolves
 *  `"cancelled"` when `cancelOllamaPull` stopped it; rejects with
 *  `"{CODE}: {message}"` on any failure. */
export async function pullOllamaModel(
  pullId: string,
  target: OllamaTarget,
  model: string,
  onProgress: (p: PullProgress) => void,
): Promise<"done" | "cancelled"> {
  const channel = new Channel<PullProgress>();
  channel.onmessage = onProgress;
  return invoke("ollama_pull_model", { pullId, target, model, onProgress: channel });
}

export async function cancelOllamaPull(pullId: string): Promise<void> {
  return invoke("ollama_cancel_pull", { pullId });
}

export async function deleteOllamaModel(target: OllamaTarget, model: string): Promise<void> {
  return invoke("ollama_delete_model", { target, model });
}
