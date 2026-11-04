/**
 * Which node types do something outside the app when they run: send a message,
 * change data elsewhere, or spend money. Used to ask before a Test runs such
 * a node as an earlier step. Code, Shell and Database are not listed here: they
 * have their own confirmation (`checkDangerousNodes`). A plugin node, or a
 * built-in type missing from this list, is not flagged because its effect is
 * unknown.
 */

export interface SideEffect {
  label: string;
}

const ALWAYS: Readonly<Record<string, string>> = {
  email_send:    "sends an email",
  sendgrid:      "sends an email",
  slack:         "posts to Slack",
  discord:       "posts to Discord",
  telegram:      "sends a Telegram message",
  notification:  "shows a notification",
  github:        "creates or comments on a GitHub item",
  notion:        "creates or changes a Notion page",
  stripe:        "creates a Stripe payment",
  social_upload: "uploads to a social platform",
  s3_storage:    "changes cloud storage",
  text_to_file:  "writes a file",
  save_to_folder: "saves files to a folder",
};

const AI_TYPES: ReadonlySet<string> = new Set(["ai_prompt", "ai_agent", "image_gen"]);
const READ_ONLY_HTTP: ReadonlySet<string> = new Set(["GET", "HEAD", "OPTIONS"]);

function text(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

export function sideEffectOf(typeId: string, config: Record<string, unknown> | undefined): SideEffect | null {
  const cfg = config ?? {};
  const always = ALWAYS[typeId];
  if (always) return { label: always };

  switch (typeId) {
    case "http_request": {
      const raw = text(cfg.method);
      if (!raw) return null;
      if (raw.includes("{{")) return { label: "sends an HTTP request whose method is set by data" };
      const method = raw.toUpperCase();
      return READ_ONLY_HTTP.has(method) ? null : { label: `sends an HTTP ${method} request` };
    }
    case "file": {
      const op = text(cfg.operation).toLowerCase();
      if (op === "write" || op === "append") return { label: "writes to a file" };
      if (op === "delete") return { label: "deletes a file" };
      return null;
    }
    case "google_sheets":
      return text(cfg.action) === "append_row" ? { label: "adds a row to Google Sheets" } : null;
    default:
      break;
  }

  if (AI_TYPES.has(typeId)) {
    const provider = text(cfg.provider).toLowerCase();
    const baseUrl  = text(cfg.base_url).toLowerCase();
    if (provider === "local" || /\/\/(localhost|127\.0\.0\.1|\[::1\])[:/]/.test(`${baseUrl}/`)) return null;
    return { label: "uses your AI account and may cost money" };
  }
  return null;
}
