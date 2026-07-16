import { escapeHtml } from "../utils";
import { getOAuthRedirectPort } from "../ipc/oauth";

// ---------------------------------------------------------------------------
// SocialSetupGuide — per-platform OAuth setup instructions
// Call showSocialSetupGuide(platform?) to open. Defaults to YouTube tab.
// ---------------------------------------------------------------------------

type Platform = "youtube" | "instagram" | "tiktok";

const DEFAULT_OAUTH_PORT = 42069;
const DEFAULT_REDIRECT_URI = `http://127.0.0.1:${DEFAULT_OAUTH_PORT}/callback`;

// T2-15/S10-2: PLATFORMS below is built once, at module load, so the real
// redirect URI can't be baked into its `steps` strings directly — it's only
// known live, via an async port probe that may resolve after first render.
// Each step that shows the redirect URI uses this placeholder instead; renderGuide()
// substitutes it for the current best-known value every time it renders.
const REDIRECT_URI_TOKEN = "%%REDIRECT_URI%%";

// Best-known live redirect URI. Starts at the documented default and is updated
// (and the guide re-rendered, if still open) once getOAuthRedirectPort() resolves.
let _liveRedirectUri = DEFAULT_REDIRECT_URI;
// True once a probe has confirmed the live port isn't the documented default —
// drives the warning banner below.
let _portMismatch = false;

interface PlatformDef {
  id:    Platform;
  label: string;
  icon:  string;
  steps: string[];
  notes: string[];
  warning?: string;
}

const PLATFORMS: PlatformDef[] = [
  {
    id:    "youtube",
    label: "YouTube",
    icon:  "▶",
    steps: [
      `Go to <a href="https://console.cloud.google.com" target="_blank" rel="noopener noreferrer">console.cloud.google.com</a> and create a new project (or select an existing one).`,
      `In the left menu, go to <strong>APIs &amp; Services → Library</strong>. Search for <strong>YouTube Data API v3</strong> and click <strong>Enable</strong>.`,
      `Go to <strong>APIs &amp; Services → OAuth consent screen</strong>. Choose <strong>External</strong>, fill in the required fields (App name, support email), and save.`,
      `Go to <strong>APIs &amp; Services → Credentials</strong>. Click <strong>+ Create Credentials → OAuth client ID</strong>.`,
      `Set Application type to <strong>Desktop app</strong>. Give it a name and click <strong>Create</strong>.`,
      `Copy your <strong>Client ID</strong> and <strong>Client Secret</strong> from the dialog.`,
      `Under <strong>Authorized redirect URIs</strong>, add exactly: <code class="ssg-copyable" data-value="${REDIRECT_URI_TOKEN}">${REDIRECT_URI_TOKEN}</code>`,
      `In Aerini, open <strong>Connections</strong> and add a new credential of type <strong>YouTube OAuth</strong>. Paste your Client ID and Client Secret.`,
    ],
    notes: [
      "Your app starts in 'Testing' mode. Add your Google account as a test user under OAuth consent screen → Test users.",
      "To upload publicly without re-authorising every 7 days, submit your app for Google verification (OAuth consent screen → Publish App).",
    ],
  },
  {
    id:    "instagram",
    label: "Instagram",
    icon:  "◈",
    warning:
      "Instagram requires media to be hosted at a public URL. The Social Upload node cannot send local files directly to Instagram. Upload your media to a CDN or web server first, then pass the public URL as input.",
    steps: [
      `Go to <a href="https://developers.facebook.com" target="_blank" rel="noopener noreferrer">developers.facebook.com</a> and log in with your Meta account.`,
      `Click <strong>My Apps → Create App</strong>. Choose <strong>Other</strong> as the use case, then <strong>Consumer</strong> as the app type.`,
      `Inside your app dashboard, click <strong>Add Product</strong> and add <strong>Instagram</strong>.`,
      `Go to <strong>Instagram → API setup with Instagram login</strong>. Click <strong>Generate token</strong> to confirm your Instagram account is linked.`,
      `Go to <strong>App settings → Basic</strong>. Note your <strong>App ID</strong> (Client ID) and <strong>App Secret</strong> (Client Secret).`,
      `Under <strong>Instagram → Settings → Valid OAuth Redirect URIs</strong>, add exactly: <code class="ssg-copyable" data-value="${REDIRECT_URI_TOKEN}">${REDIRECT_URI_TOKEN}</code>`,
      `Request the <strong>instagram_content_publish</strong> permission under <strong>App Review → Permissions and Features</strong>. For testing, add your Instagram account under <strong>Roles → Instagram Testers</strong>.`,
      `In Aerini, open <strong>Connections</strong> and add a new credential of type <strong>Instagram OAuth</strong>. Paste your App ID and App Secret.`,
    ],
    notes: [
      "Business or Creator accounts are required for content publishing. Personal accounts are not supported by the Instagram API.",
      "The instagram_content_publish permission requires App Review approval before use with accounts other than your own test accounts.",
    ],
  },
  {
    id:    "tiktok",
    label: "TikTok",
    icon:  "♪",
    steps: [
      `Go to <a href="https://developers.tiktok.com" target="_blank" rel="noopener noreferrer">developers.tiktok.com</a> and sign in with your TikTok account.`,
      `Click <strong>Manage Apps → Create app</strong>. Fill in the app name and description. Set platform to <strong>Web</strong>.`,
      `Under <strong>Products</strong>, add <strong>Login Kit</strong> and <strong>Content Posting API</strong>.`,
      `In the <strong>Login Kit</strong> settings, add the redirect URI exactly: <code class="ssg-copyable" data-value="${REDIRECT_URI_TOKEN}">${REDIRECT_URI_TOKEN}</code>`,
      `Request the <strong>video.publish</strong> scope under <strong>Content Posting API → Scopes</strong>. For sandbox testing, use the sandbox environment.`,
      `Copy your <strong>Client Key</strong> (Client ID) and <strong>Client Secret</strong> from the app's <strong>App info</strong> page.`,
      `In Aerini, open <strong>Connections</strong> and add a new credential of type <strong>TikTok OAuth</strong>. Paste your Client Key and Client Secret.`,
    ],
    notes: [
      "TikTok apps default to sandbox mode. In sandbox, posts are private and visible only to your account. Submit for review to enable public publishing.",
      "The video.publish scope requires TikTok app review approval. Processing typically takes 1–3 business days.",
    ],
  },
];

// ---------------------------------------------------------------------------

let _el: HTMLElement | null = null;
let _activePlatform: Platform = "youtube";

function getOrCreateEl(): HTMLElement {
  if (_el) return _el;
  _el = document.createElement("div");
  _el.id = "social-setup-guide";
  _el.className = "ssg-overlay hidden";
  document.body.appendChild(_el);
  return _el;
}

export function showSocialSetupGuide(platform: Platform = "youtube"): void {
  _activePlatform = platform;
  const el = getOrCreateEl();
  el.classList.remove("hidden");
  renderGuide(el); // render immediately with the current best-known redirect URI

  // Best-effort live port probe (T2-15/S10-2). Re-renders only if the guide is
  // still open and the result actually changed anything — avoids a pointless
  // rebuild when the port was already the documented default, as it usually is.
  // Failure (older backend, IPC error) is silent: the guide keeps showing the
  // documented default, exactly as it did before this fix existed.
  getOAuthRedirectPort()
    .then(port => {
      const uri = `http://127.0.0.1:${port}/callback`;
      const mismatch = port !== DEFAULT_OAUTH_PORT;
      if (uri === _liveRedirectUri && mismatch === _portMismatch) return;
      _liveRedirectUri = uri;
      _portMismatch = mismatch;
      if (!el.classList.contains("hidden")) renderGuide(el);
    })
    .catch(() => { /* keep showing the default — same as before this fix */ });
}

export function hideSocialSetupGuide(): void {
  _el?.classList.add("hidden");
}

function renderGuide(el: HTMLElement): void {
  // Placeholder substitution (T2-15/S10-2): PLATFORMS' step strings were built
  // once at module load with a fixed token in place of the redirect URI, since
  // the real value is only known live and can change between opens. Substitute
  // here, at render time, into a plain string, before assigning to innerHTML —
  // the URI's charset (letters/digits/./:/`/`) needs no HTML escaping, but it's
  // routed through escapeHtml anyway for consistency with every other dynamic
  // value in this file.
  const liveUriHtml = escapeHtml(_liveRedirectUri);
  const stepsHtml = (steps: string[]) =>
    steps.map(s => s.split(REDIRECT_URI_TOKEN).join(liveUriHtml));

  el.innerHTML = `
    <div class="ssg-backdrop"></div>
    <div class="ssg-box" role="dialog" aria-modal="true" aria-label="Social platform setup guide">
      <div class="ssg-header">
        <div>
          <div class="ssg-title">Social Platform Setup</div>
          <div class="ssg-subtitle">Follow the steps for your platform to create OAuth credentials.</div>
        </div>
        <button class="ssg-close" id="ssg-close" aria-label="Close">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round">
            <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
          </svg>
        </button>
      </div>

      <div class="ssg-tab-bar" role="tablist">
        ${PLATFORMS.map(p => `
          <button class="ssg-tab${p.id === _activePlatform ? " ssg-tab--active" : ""}"
            role="tab"
            aria-selected="${p.id === _activePlatform}"
            data-platform="${p.id}">
            <span class="ssg-tab-icon">${p.icon}</span>${escapeHtml(p.label)}
          </button>
        `).join("")}
      </div>

      <div class="ssg-pane-wrap">
        ${PLATFORMS.map(p => `
          <div class="ssg-pane${p.id === _activePlatform ? "" : " hidden"}" data-pane="${p.id}">
            ${p.warning ? `
              <div class="ssg-warning" role="alert">
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"/><line x1="12" y1="9" x2="12" y2="13"/><line x1="12" y1="17" x2="12.01" y2="17"/></svg>
                <span>${p.warning}</span>
              </div>
            ` : ""}
            ${_portMismatch ? `
              <div class="ssg-warning" role="alert">
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"/><line x1="12" y1="9" x2="12" y2="13"/><line x1="12" y1="17" x2="12.01" y2="17"/></svg>
                <span>Port ${DEFAULT_OAUTH_PORT} is currently unavailable, so OAuth will use a different port for this session. The redirect URI shown in the steps below and at the bottom of this guide already reflects the port that will actually be used — register that value with this platform, not ${DEFAULT_OAUTH_PORT}, or close whatever else is using port ${DEFAULT_OAUTH_PORT} and reopen this guide.</span>
              </div>
            ` : ""}
            <ol class="ssg-steps">
              ${stepsHtml(p.steps).map((step, i) => `
                <li class="ssg-step">
                  <span class="ssg-step-num">${i + 1}</span>
                  <span class="ssg-step-text">${step}</span>
                </li>
              `).join("")}
            </ol>
            ${p.notes.length ? `
              <div class="ssg-notes">
                <div class="ssg-notes-label">Notes</div>
                <ul class="ssg-notes-list">
                  ${p.notes.map(n => `<li>${escapeHtml(n)}</li>`).join("")}
                </ul>
              </div>
            ` : ""}
          </div>
        `).join("")}
      </div>

      <div class="ssg-footer">
        <div class="ssg-redirect-row">
          <span class="ssg-redirect-label">Redirect URI (use this exact value in all platforms):</span>
          <span class="ssg-redirect-uri">${liveUriHtml}</span>
          <button class="ssg-copy-btn" id="ssg-copy-uri">Copy</button>
        </div>
      </div>
    </div>`;

  el.querySelector(".ssg-backdrop")!.addEventListener("click", hideSocialSetupGuide);
  el.querySelector("#ssg-close")!.addEventListener("click", hideSocialSetupGuide);

  el.querySelectorAll<HTMLButtonElement>(".ssg-tab").forEach(tab => {
    tab.addEventListener("click", () => {
      _activePlatform = tab.dataset.platform as Platform;
      el.querySelectorAll(".ssg-tab").forEach(t => {
        t.classList.toggle("ssg-tab--active", t === tab);
        t.setAttribute("aria-selected", String(t === tab));
      });
      el.querySelectorAll<HTMLElement>(".ssg-pane").forEach(pane => {
        pane.classList.toggle("hidden", pane.dataset.pane !== _activePlatform);
      });
    });
  });

  // Copyable redirect URI values (inline code blocks in steps)
  el.querySelectorAll<HTMLElement>(".ssg-copyable").forEach(code => {
    code.title = "Click to copy";
    code.style.cursor = "pointer";
    code.addEventListener("click", () => {
      navigator.clipboard.writeText(code.dataset.value ?? code.textContent ?? "").catch(() => {});
      const orig = code.textContent;
      code.textContent = "Copied!";
      setTimeout(() => { code.textContent = orig; }, 1500);
    });
  });

  // Copy redirect URI button
  el.querySelector<HTMLButtonElement>("#ssg-copy-uri")!.addEventListener("click", e => {
    const btn = e.currentTarget as HTMLButtonElement;
    navigator.clipboard.writeText(_liveRedirectUri).catch(() => {});
    btn.textContent = "Copied!";
    setTimeout(() => { btn.textContent = "Copy"; }, 1500);
  });
}
