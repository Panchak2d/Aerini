import { invoke } from "@tauri-apps/api/core";

export interface UpdateCheckResult {
  current_version: string;
  latest_version: string;
  is_newer: boolean;
  release_url: string;
}

export const checkForUpdate = (): Promise<UpdateCheckResult> =>
  invoke<UpdateCheckResult>("check_for_update");
