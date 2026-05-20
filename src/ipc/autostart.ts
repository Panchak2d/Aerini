import { invoke } from "@tauri-apps/api/core";

/** Returns whether Flowo is currently set to launch at login. */
export const getAutostart = (): Promise<boolean> =>
  invoke<boolean>("get_autostart");

/** Enables or disables launch at login. */
export const setAutostart = (enabled: boolean): Promise<void> =>
  invoke<void>("set_autostart", { enabled });
