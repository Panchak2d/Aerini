import { invoke } from "@tauri-apps/api/core";

/**
 * Returns the OAuth callback port that would be used right now if a flow were
 * started this instant — 42069 if free, otherwise whatever OS-assigned port it
 * would actually fall back to (T2-15/S10-2).
 */
export async function getOAuthRedirectPort(): Promise<number> {
  return invoke("get_oauth_redirect_port");
}
