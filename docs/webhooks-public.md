# Exposing Webhooks to the Internet

Flowo's Webhook node listens on `127.0.0.1` — your own machine only. External services like Stripe, GitHub, and Twilio need a public URL they can reach. This page covers two tools that create a secure tunnel from the internet to your local machine.

If you're running `flowo-server` on a VPS or cloud server, you don't need a tunnel — use a reverse proxy instead. See [Server Deployment — receiving webhooks](server-deploy.md#receiving-webhooks-on-a-server).

---

## Option 1 — cloudflared (Cloudflare Tunnel)

Cloudflare Tunnel creates a public HTTPS URL backed by Cloudflare's network. No account is required for temporary tunnels; a free Cloudflare account gives you a persistent named tunnel with a stable URL.

### Install cloudflared

```bash
# macOS (Homebrew)
brew install cloudflare/cloudflare/cloudflared

# Windows (winget)
winget install --id Cloudflare.cloudflared

# Linux
curl -L https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64 \
  -o /usr/local/bin/cloudflared && chmod +x /usr/local/bin/cloudflared
```

### Start a temporary tunnel

```bash
cloudflared tunnel --url http://localhost:3456
```

Replace `3456` with the port configured in your Webhook node. Cloudflared prints a URL like:

```
https://random-name.trycloudflare.com
```

Use that as the webhook URL in Stripe, GitHub, or whatever service you're integrating. The URL changes every time you restart the tunnel. For a stable URL that doesn't change, create a named tunnel with a free Cloudflare account — see the [Cloudflare Tunnel documentation](https://developers.cloudflare.com/cloudflare-one/connections/connect-apps/install-and-setup/).

---

## Option 2 — ngrok

ngrok provides an HTTPS tunnel with a web dashboard for inspecting and replaying incoming requests. The free tier gives a random URL that changes on each restart; paid plans give fixed URLs.

### Install and authenticate

Download from [ngrok.com/download](https://ngrok.com/download) for your platform, then authenticate once:

```bash
ngrok config add-authtoken YOUR_TOKEN
```

Your token is on the [ngrok dashboard](https://dashboard.ngrok.com) after signing up.

### Start a tunnel

```bash
ngrok http 3456
```

ngrok prints a URL like `https://a1b2c3d4.ngrok-free.app`. Use that as your webhook URL.

The ngrok dashboard at `http://127.0.0.1:4040` shows every incoming request in real time — you can inspect headers, bodies, and replay requests without waiting for the real service to send them again. This is especially useful when debugging Stripe or GitHub webhooks that are hard to trigger repeatedly.

---

## Which port do I use?

The port is whatever you set in the Webhook node's **Port** field (default: `3456`). Use that same number in the tunnel command.

If you run multiple Webhook workflows at once, each needs a different port. Start a separate tunnel for each port.

---

## Security: what the built-in secret does and doesn't do

The Webhook node's **Secret** field validates that incoming requests include the right `X-Flowo-Secret` header. The comparison is timing-safe. But it proves caller identity, not message integrity — a captured request with the same header could theoretically be replayed.

The optional **Validate Timestamp** setting narrows the replay window to 5 minutes, though the timestamp isn't cryptographically bound to the request body.

**For payment processors and other sensitive integrations**, use the platform's own signature verification in a downstream Code node:

- **Stripe** sends a `Stripe-Signature` header. See [Stripe's documentation](https://stripe.com/docs/webhooks/signatures) for verification code.
- **GitHub** sends an `X-Hub-Signature-256` header. See [GitHub's documentation](https://docs.github.com/en/webhooks/using-webhooks/validating-webhook-deliveries).

The built-in secret is fine as a first-line check; the platform's HMAC signature is what provides cryptographic proof that the body hasn't been tampered with.

Full webhook security details: [Security — Webhook security](security.md#6-webhook-security)
