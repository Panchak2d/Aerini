import { invoke } from "@tauri-apps/api/core";

/** Opens an http(s) address in the default browser. The Rust side validates it again. */
export const openNoteLinkExternal = (url: string): Promise<void> =>
  invoke<void>("open_note_link", { url });
