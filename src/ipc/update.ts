import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

export type ManualReason =
  | "unknown_install"
  | "appimage_missing"
  | "appimage_not_writable"
  | "macos_translocated"
  | "macos_disk_image"
  | "no_release_for_platform";

export type InstallSupport =
  | { kind: "supported"; admin_prompt: boolean }
  | { kind: "manual"; reason: ManualReason; message: string };

export interface UpdateCheckResult {
  current_version: string;
  available: boolean;
  latest_version: string | null;
  install_support: InstallSupport;
  release_url: string;
}

export type InstallOutcome =
  | { status: "active_runs"; scheduled: number; manual: number }
  | { status: "cancelled" };

export interface UpdateNotice {
  kind: "installed" | "incomplete";
  version: string;
}

export interface UpdateProgress {
  stage: "downloading" | "installing";
  downloaded: number;
  total: number | null;
}

export const checkForUpdate = (): Promise<UpdateCheckResult> =>
  invoke<UpdateCheckResult>("check_for_update");

/**
 * Downloads, verifies and installs the update found by the last check, then
 * restarts. On success the app exits and this promise never settles.
 */
export const installUpdate = (force = false): Promise<InstallOutcome> =>
  invoke<InstallOutcome>("install_update", { force });

export const cancelUpdateDownload = (): Promise<void> =>
  invoke<void>("cancel_update_download");

/** Result of the previous install attempt; returned once, then null. */
export const takeUpdateNotice = (): Promise<UpdateNotice | null> =>
  invoke<UpdateNotice | null>("take_update_notice");

export const listenUpdateProgress = (
  cb: (progress: UpdateProgress) => void,
): Promise<() => void> =>
  getCurrentWebviewWindow().listen<UpdateProgress>("update-progress", (e) => cb(e.payload));
