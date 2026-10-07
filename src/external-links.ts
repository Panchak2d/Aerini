import { openUrl } from "@tauri-apps/plugin-opener";
import { isTauri } from "./utils";

export const EXTERNAL_URL_SCOPE: readonly string[] = [
  "https://github.com/Panchak2d/**",
  "https://www.patreon.com/**",
  "https://developers.cloudflare.com/cloudflare-one/connections/connect-apps/install-and-setup/",
  "https://ngrok.com/",
  "https://console.cloud.google.com/",
  "https://developers.facebook.com/",
  "https://developers.tiktok.com/",
  "https://www.youtube.com/watch*",
];

const escapeRegExp = (s: string): string => s.replace(/[.+^${}()|[\]\\?]/g, "\\$&");

const SCOPE_PATTERNS: readonly RegExp[] = EXTERNAL_URL_SCOPE.map(
  (glob) => new RegExp(`^${glob.split(/\*+/).map(escapeRegExp).join(".*")}$`),
);

export function isAllowedExternalUrl(href: string): boolean {
  let url: URL;
  try {
    url = new URL(href);
  } catch {
    return false;
  }
  return url.protocol === "https:" && SCOPE_PATTERNS.some((re) => re.test(url.href));
}

type Toast = (msg: string, type?: "success" | "error" | "info" | "warning") => void;

export function bindExternalLinks(toast: Toast): void {
  if (!isTauri()) return;

  const onActivate = (e: MouseEvent): void => {
    if (e.defaultPrevented || e.button > 1) return;
    const anchor = e.target instanceof Element ? e.target.closest("a[href]") : null;
    if (!anchor || anchor.hasAttribute("download")) return;

    let url: URL;
    try {
      url = new URL((anchor as HTMLAnchorElement).href);
    } catch {
      return;
    }
    if (url.protocol !== "https:" && url.protocol !== "http:") return;
    if (url.origin === window.location.origin) return;

    e.preventDefault();
    if (!isAllowedExternalUrl(url.href)) {
      toast("Aerini only opens links to its own project, documentation and setup pages.", "info");
      return;
    }
    openUrl(url.href).catch(() => toast("Couldn't open the link in your browser.", "error"));
  };

  document.addEventListener("click", onActivate);
  document.addEventListener("auxclick", onActivate);
}
