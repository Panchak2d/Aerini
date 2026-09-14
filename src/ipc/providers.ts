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
