import { isTauri } from "./utils";
import { showConfirm } from "./confirm";
import { showToast } from "./toast";
import { safeHref } from "./canvas/note-format";
import { openNoteLinkExternal } from "./ipc/note-link";

const SHOWN_URL_MAX = 160;

function shown(url: string): string {
  return url.length > SHOWN_URL_MAX ? `${url.slice(0, SHOWN_URL_MAX)}…` : url;
}

/**
 * Follows a link clicked in a note. A workflow file can come from anyone, so
 * the real site and the full address are always shown, because the label can
 * say anything, and nothing opens without a yes. The site is shown as the URL
 * parser reports it, so an international name appears in its xn-- form.
 */
export async function openNoteLink(raw: string): Promise<void> {
  const href = safeHref(raw);
  if (!href) return;

  const host = new URL(href).host;
  const message = `Open this link${isTauri() ? " in your browser" : " in a new tab"}?\n\nSite: ${host}\nAddress: ${shown(href)}`;
  if (!(await showConfirm(message, false, "Open", "neutral"))) return;

  if (!isTauri()) {
    window.open(href, "_blank", "noopener,noreferrer");
    return;
  }
  openNoteLinkExternal(href).catch(() => showToast("Couldn't open the link in your browser.", "error"));
}
