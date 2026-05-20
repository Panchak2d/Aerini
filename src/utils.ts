export const isTauri = (): boolean => "__TAURI_INTERNALS__" in window;

export function escapeHtml(s: string): string {
  return String(s)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

// Symbols chosen for clarity at small size — no emoji, no colour-coded text.
// Used by both the canvas renderer and the palette sidebar.
export const NODE_ICONS: Record<string, string> = {
  manual_trigger: "▶",
  webhook:        "⚓",
  schedule:       "◷",
  http_request:   "↗",
  shell_exec:     "$",
  email_send:     "✉",
  file:           "⬝",
  if_condition:   "?",
  switch:         "⟨⟩",
  loop:           "↻",
  stop:           "■",
  merge:          "⇒",
  delay:          "⏱",
  wait:           "◔",
  transform:      "⟳",
  json:           "{ }",
  set_variable:   "=",
  get_variable:   "~",
  ai_prompt:      "✦",
  ai_agent:       "⬡",
  ai_memory:      "◎",
  text_splitter:  "⋮",
  database:       "⊞",
  notification:   "◈",
  code:           "</>",
  output:         "◉",
  note:           "≡",
  s3_storage:     "☁",
  image_gen:      "⬙",
  save_to_folder: "⬎",
  collect_files:  "⊕",
  social_upload:  "⬆",
};
