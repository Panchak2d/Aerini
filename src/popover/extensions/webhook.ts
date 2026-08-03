import type { ExtensionContext } from "../../node-configs/popover-utils";

export function renderWebhookBanners(ctx: ExtensionContext): void {
  const runNote = document.createElement("div");
  runNote.className = "popover-info-banner";
  runNote.innerHTML = `<strong>Run Now</strong> waits for one incoming request, then stops. For a persistent listener, use <strong>Schedule Run</strong>.`;
  ctx.body.appendChild(runNote);

  const replayNote = document.createElement("div");
  replayNote.className = "popover-info-banner popover-info-banner--warn";
  replayNote.innerHTML =
    `<strong>Secret replay risk:</strong> The secret field authenticates the caller but does not sign the request body. ` +
    `A captured valid request can be replayed verbatim. For integrations that send HMAC body signatures ` +
    `(Stripe, GitHub, etc.), verify the platform signature header in a downstream <strong>Code</strong> node instead of relying on this field alone.`;
  ctx.body.appendChild(replayNote);

  const tunnelNote = document.createElement("div");
  tunnelNote.className = "popover-info-banner";
  tunnelNote.innerHTML =
    `<strong>ℹ Localhost only:</strong> The webhook binds to localhost. To receive requests from Stripe, GitHub, or other external services, ` +
    `you need a public URL — use <a href="https://developers.cloudflare.com/cloudflare-one/connections/connect-apps/install-and-setup/" target="_blank" rel="noopener noreferrer">cloudflared</a> ` +
    `or <a href="https://ngrok.com" target="_blank" rel="noopener noreferrer">ngrok</a>. ` +
    `See <strong>docs/webhooks-public.md</strong> for setup instructions.`;
  ctx.body.appendChild(tunnelNote);
}
