import { invoke } from "@tauri-apps/api/core";

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
