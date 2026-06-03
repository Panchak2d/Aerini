# Exposing Webhook Nodes to the Internet

The Flowo Webhook node binds to `127.0.0.1` (localhost only). External services like Stripe, GitHub, and Twilio need a reachable public URL. This page covers two options for desktop/local setups.

---

## Option 1 — cloudflared (Cloudflare Tunnel)

Cloudflare Tunnel creates a stable public URL backed by Cloudflare's edge network. No account required for temporary tunnels; free Cloudflare account required for persistent named tunnels.

### Quick start (temporary tunnel)

```bash
# macOS
brew install cloudflare/cloudflare/cloudflared

# Windows (winget)
winget install --id Cloudflare.cloudflared

# Linux
curl -L https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64 \
  -o /usr/local/bin/cloudflared && chmod +x /usr/local/bin/cloudflared
```

Start a tunnel pointing at your Webhook node's port:

```bash
cloudflared tunnel --url http://localhost:3456
```

Cloudflared prints a URL like `https://random-name.trycloudflare.com`. Use that as the webhook URL in Stripe, GitHub, etc.

The URL changes every time you run the command. For a fixed URL, use a named tunnel with a free Cloudflare account — see [Cloudflare Tunnel docs](https://developers.cloudflare.com/cloudflare-one/connections/connect-apps/install-and-setup/).

---

## Option 2 — ngrok

ngrok provides an HTTPS tunnel with a web dashboard for inspecting requests. Free tier gives a random URL that changes on each restart; paid plans give fixed URLs.

```bash
# Install (see https://ngrok.com/download for your platform)
# Then authenticate once:
ngrok config add-authtoken <YOUR_TOKEN>

# Start a tunnel
ngrok http 3456
```

ngrok prints a URL like `https://a1b2c3d4.ngrok-free.app`. Use that as your webhook URL.

The ngrok dashboard at `http://127.0.0.1:4040` lets you inspect and replay incoming webhook requests — useful for debugging.

---

## Which port do I use?

The port is set in the Webhook node's **Port** field (default: `3456`). If you change it, use the same port in the tunnel command.

If you run multiple Webhook nodes concurrently, each needs a different port. Start a separate tunnel for each port.

---

## Server deployments

If you're running `flowo-server` on a VPS or cloud instance, use a reverse proxy (Caddy or Nginx) instead of a tunnel. See [server-deploy.md](server-deploy.md) for instructions.
