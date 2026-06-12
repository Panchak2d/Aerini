# Server Deployment

`flowo-server` is a command-line program that runs your workflows on a Linux server without the Flowo desktop app. Use it when you need workflows running around the clock — on a VPS, a home server, or any Linux machine.

**Do you need this?** If you're happy running workflows while Flowo is open on your computer, you don't need this at all. Server deployment is for when you want uninterrupted 24/7 execution, or when you want to run workflows triggered by public webhooks from services like Stripe or GitHub.

Two modes are available:

- **Serve mode** — runs one exported workflow. The simplest path for a single automation.
- **API mode** — manages many workflows on one server via a REST API.

---

## License notice

`flowo-server` is licensed under AGPL-3.0. Running the unmodified binary for your own use does not impose any obligations. If you run a **modified** version as a network service for others, AGPL-3.0 requires you to publish your modifications under the same license. A commercial license is available if you need to keep modifications proprietary — see [Commercial licensing](../README.md#commercial-licensing).

---

## Serve mode — run a single workflow

This is the fastest path. You export a workflow from the desktop app and run it on a server in minutes.

### Step 1 — Export from the desktop app

Open any workflow with a Schedule or Webhook trigger. Click **File → Export for Server**.

Flowo generates a zip containing:

| File | What it is |
|---|---|
| `flowo-server` | The server binary (Linux x86-64, statically linked — no dependencies to install) |
| `flowo-server.json` | Your workflow configuration |
| `.env.example` | A list of environment variables you need to set (one per credential the workflow uses) |
| `flowo-server.service` | A systemd unit file so the workflow starts on boot |
| `install.sh` | An install script that handles placement, systemd setup, and initial configuration |

### Step 2 — Upload and install

Upload the zip to your server and run the install script. Replace `user@yourserver.com` with your actual server address:

```bash
scp flowo-export.zip user@yourserver.com:/tmp/
ssh user@yourserver.com
cd /tmp && unzip flowo-export.zip && cd flowo-export
./install.sh
```

The script does four things automatically:
1. Copies `flowo-server` to `/usr/local/bin/`
2. Creates `/etc/flowo/` and places `flowo-server.json` there
3. Creates `/etc/flowo/.env` from `.env.example` for you to fill in
4. Installs and enables the systemd service so it starts on boot

### Step 3 — Set your credentials

Open the `.env` file and fill in your API keys:

```bash
sudo nano /etc/flowo/.env
```

The variable names are automatically derived from the credential IDs you set in the desktop app:

| Credential ID you set in Flowo | Environment variable name |
|---|---|
| `openai-prod` | `FLOWO_CRED_OPENAI_PROD` |
| `slack-bot` | `FLOWO_CRED_SLACK_BOT` |
| `my.key` | `FLOWO_CRED_MY_KEY` |

Hyphens and dots become underscores, the name is uppercased, and `FLOWO_CRED_` is prepended. The export panel shows the complete list of variables your specific workflow needs.

### Required variables (`$vars.X`)

If your workflow reads `{{$vars.something}}` anywhere — in any node, at any nesting depth, including inside trigger configs — the export panel shows a separate **Required variables** section listing each one, alongside its suggested environment variable name: `something` becomes `FLOWO_VAR_SOMETHING`.

This is distinct from the credential variables above. Credentials (`FLOWO_CRED_...`) come from what you've stored in the Connections panel; `$vars` variables (`FLOWO_VAR_...`) are plain values you pass in yourself — feature flags, environment names, recipient addresses, anything that isn't a secret but still shouldn't be hardcoded into the workflow JSON. Set them in `/etc/flowo/.env` the same way as credential variables. If your workflow doesn't use `$vars` at all, this section doesn't appear — there's nothing to fill in.

### Step 4 — Start the service

```bash
sudo systemctl start flowo-server
sudo systemctl status flowo-server
```

The service starts automatically on every boot. To follow the live logs:

```bash
sudo journalctl -u flowo-server -f
```

### Status page

Serve mode exposes a status page at `http://127.0.0.1:7700/` (or whichever port is in `flowo-server.json`). It shows:

- Workflow name and trigger type
- Last run time, status, and duration
- A manual Run button (requires the `run_secret` shown at export time)
- Run history
- Live logs via `GET /api/logs`

The status page binds to `127.0.0.1` by default. To view it remotely, either SSH tunnel to it or add `--bind 0.0.0.0` to the `ExecStart` line in the systemd unit file — but put it behind a reverse proxy with authentication before doing that.

> **Security note:** without a `run_secret` in `flowo-server.json`, the status page is unauthenticated and shows your workflow name, trigger type, and run history. The desktop app generates a `run_secret` automatically at export time. If yours doesn't have one, add `"run_secret": "a-long-random-string"` to `flowo-server.json`.

### Serve mode command reference

```bash
flowo-server serve [OPTIONS]

Options:
  --config <path>                Path to flowo-server.json. Default: flowo-server.json
  --port <port>                  Override the status page port
  --bind <addr>                  Interface to bind to. Default: 127.0.0.1
  --trusted-proxy-count <n>      Reverse-proxy hops to trust for X-Forwarded-For. Default: 0
  --allow-shell                  Enable Shell Command nodes (disabled by default)
  --allow-code                   Enable Code (JS) nodes (disabled by default)
  --reject-legacy-run-secret     Refuse to start if run_secret uses the legacy BLAKE3 hash format
  --ssrf-firewall-acknowledged   Suppress the SSRF egress warning (set after firewall is configured)
```

Shell Command and Code (JS) nodes are disabled in serve mode by default. Only pass `--allow-shell` or `--allow-code` after reviewing every node in the workflow you're deploying.

---

## API mode — manage multiple workflows

API mode runs many workflows on one server and exposes a REST API for programmatic management.

### Start the server

```bash
flowo-server api --token mysecrettoken --port 7700
```

Or use environment variables instead of flags:

```bash
export FLOWO_TOKEN=mysecrettoken
export FLOWO_PORT=7700
flowo-server api
```

If you don't set `--token`, a random token is generated on first run and printed to stdout. Copy it — it won't be shown again.

### API mode command reference

```bash
flowo-server api [OPTIONS]

Options:
  --token <token>               Bearer token for API authentication. Env: FLOWO_TOKEN
  --port <port>                 Port to listen on. Default: 7700. Env: FLOWO_PORT
  --data-dir <path>             Directory for the SQLite database and key file. Default: ~/.flowo-server. Env: FLOWO_DATA_DIR
  --bind <addr>                 Interface to bind to. Default: 127.0.0.1
  --allow-origin <origins>      Additional CORS origins (comma-separated)
  --allow-env-vars <vars>       Environment variables workflows may read via {{$env.VAR}}. Disabled by default.
  --file-sandbox-dir <path>     Restrict File nodes to this directory tree
  --trusted-proxy-count <n>     Reverse-proxy hops to trust for X-Forwarded-For. Default: 0
  --allow-shell                 Enable Shell Command nodes
  --allow-code                  Enable Code (JS) nodes
  --keychain                    Use the OS keychain instead of a key file for the encryption key
  --max-concurrent-runs <n>     Maximum workflows running simultaneously. Default: 16
  --ssrf-firewall-acknowledged  Suppress the SSRF egress warning
```

Full REST API documentation: [API Reference](api-reference.md)

---

## Receiving webhooks on a server

When `flowo-server` runs on a machine with a public IP address, webhook workflows can receive requests from the internet — no tunnel required.

The workflow's Webhook node listens on the port you configured. The server accepts connections on that port (assuming it's open in your firewall). Your external service (GitHub, Stripe, etc.) sends its webhook to `https://yourserver.com:PORT/PATH`.

**Recommended setup:** put `flowo-server` behind a reverse proxy (Caddy or Nginx) so you get HTTPS and a clean domain name without exposing a numbered port directly.

### Caddy example

Install Caddy on your server, then edit `/etc/caddy/Caddyfile`:

```
webhooks.yourserver.com {
    reverse_proxy 127.0.0.1:3456
}
```

Restart Caddy (`sudo systemctl restart caddy`). Caddy automatically obtains a TLS certificate from Let's Encrypt. Your webhook URL becomes `https://webhooks.yourserver.com/webhook`.

### Nginx example

```nginx
server {
    listen 443 ssl;
    server_name webhooks.yourserver.com;

    ssl_certificate     /etc/letsencrypt/live/webhooks.yourserver.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/webhooks.yourserver.com/privkey.pem;

    location / {
        proxy_pass http://127.0.0.1:3456;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

If you use a proxy, add `--trusted-proxy-count 1` to the `flowo-server` flags so it reads the real client IP from the `X-Forwarded-For` header instead of seeing the proxy's address.

---

## Running with Docker

A `Dockerfile` is included if you prefer containers.

```bash
# Build the image
docker build -t flowo-server .

# Run it
docker run -d \
  -p 7700:7700 \
  -e FLOWO_TOKEN=mysecrettoken \
  -v flowo-data:/data \
  -e FLOWO_DATA_DIR=/data \
  flowo-server
```

A `docker-compose.yml` is also included in the repo root for a one-command setup:

```bash
docker-compose up -d
```

---

## Security hardening checklist

Before exposing `flowo-server` to the internet, work through these:

**Authentication**
- [ ] `--token` is set to a long, random string (not a dictionary word)
- [ ] The token is stored securely — not hardcoded in a shell script or committed to version control
- [ ] The API is not publicly reachable without the token header

**Networking**
- [ ] Server is behind a reverse proxy with HTTPS
- [ ] The API port (7700 by default) is firewalled — only the reverse proxy can reach it directly
- [ ] Webhook ports are firewalled to only accept connections via the reverse proxy
- [ ] An egress firewall blocks outbound connections to private IP ranges (required to fully close the DNS rebinding gap — see [Security](security.md#5-http-node--ssrf-protection))

**Dangerous nodes**
- [ ] `--allow-shell` is NOT set unless your workflow requires Shell Command nodes and you've reviewed every node in it
- [ ] `--allow-code` is NOT set unless your workflow requires Code (JS) nodes and you've reviewed every node in it

**Data**
- [ ] `--file-sandbox-dir` is set if your workflow uses File nodes
- [ ] `--allow-env-vars` lists only the specific variables your workflow needs, never `FLOWO_TOKEN` or `FLOWO_CRED_*`
- [ ] The data directory is backed up regularly
- [ ] `flowo.key` and `credentials.db` are never in the same unencrypted backup

**Process**
- [ ] Server runs as a dedicated non-root user, not as `root`
- [ ] The systemd unit uses `NoNewPrivileges=yes` and `PrivateTmp=yes`

---

## CLI control commands

These commands manage a running API mode server without going through the REST API:

```bash
# List all workflows on the server
flowo-server list --token mytoken

# Check server and scheduler status
flowo-server status --token mytoken

# Stop a running workflow (partial name match accepted)
flowo-server stop "my workflow" --token mytoken

# Start a stopped workflow
flowo-server start "my workflow" --token mytoken

# Restart a workflow
flowo-server restart "my workflow" --token mytoken
```

Use `--server http://yourserver.com:7700` to target a remote server. Default is `http://localhost:7700`.

---

## Connecting the desktop app to API mode

Once `flowo-server api` is running, you can point the Flowo desktop app at it for remote workflow management. Go to **Settings → Server** in the desktop app and set the server URL and token.

Workflows saved through the desktop app are pushed to the server and scheduled there. Execution results stream back to the desktop in real time via server-sent events.

---

## SSE event scope

`GET /api/events` delivers execution events for **all** workflows to any token with `read` scope. In multi-user deployments where token holders should not see each other's workflow activity, avoid issuing `read` tokens to untrusted parties until per-workflow ACL is implemented in a future release.

---

## Critical security warnings

### Egress firewall — required for public deployments

The HTTP Request, Database, and AI nodes validate DNS before making requests to prevent SSRF. However, a TOCTOU gap exists between DNS resolution and the actual TCP connection. A malicious DNS server can return a valid public IP during the check, then switch to a private IP for the actual connection.

**This cannot be fixed in application code.** When running `flowo-server` on a network-accessible address, configure a host-level egress firewall:

```bash
# Block RFC-1918, loopback, link-local, and cloud metadata endpoints
iptables -A OUTPUT -d 10.0.0.0/8 -j DROP
iptables -A OUTPUT -d 172.16.0.0/12 -j DROP
iptables -A OUTPUT -d 192.168.0.0/16 -j DROP
iptables -A OUTPUT -d 127.0.0.0/8 -j DROP
iptables -A OUTPUT -d 169.254.0.0/16 -j DROP
iptables -A OUTPUT -d 168.63.129.16/32 -j DROP
```

Once your firewall is in place, pass `--ssrf-firewall-acknowledged` to suppress the startup warning.

### Legacy run_secret (serve mode)

> **Warning:** Always re-export workflows using Flowo 0.3 or later before deploying to a public server. Workflows exported with Flowo 0.2 or earlier store `run_secret` as a BLAKE3 hash, which is not brute-force resistant. If an attacker reads your `flowo-server.json` (for example from a misconfigured backup), a short `run_secret` can be cracked in seconds with a GPU.
>
> Pass `--reject-legacy-run-secret` to make the server refuse to start with a legacy hash.

### Database nodes and RUSTSEC-2023-0071

> **Warning:** Do **not** enable `--allow-database` in API mode where untrusted token holders can configure Database node connection strings. The `sqlx-mysql` dependency contains a timing side-channel in its RSA key exchange (RUSTSEC-2023-0071). In multi-tenant mode, a token holder who controls a MySQL connection string can measure handshake timing across many connections to recover session key material.
>
> This flag is safe in single-user or fully trusted deployments where you control all token holders. Track the upstream fix at [launchbadge/sqlx#3538](https://github.com/launchbadge/sqlx/issues/3538).

### Code node sandbox on macOS

When running `flowo-server api --allow-code --code-sandbox` on macOS, ESM module import restrictions apply (blocking `fs`, `net`, `child_process`, etc.), but CPU and memory resource limits (`setrlimit`) are **Linux-only**. On macOS, a runaway script can exhaust system CPU and memory — only the `timeout_secs` ceiling (max 60 seconds) applies. Deploy on Linux for full sandbox enforcement.

---

## Updating the server binary

Stop the service before replacing the binary. `flowo-server` applies any pending database migrations automatically on startup — you don't need to run them manually.

```bash
sudo systemctl stop flowo-server
sudo cp new-flowo-server /usr/local/bin/flowo-server
sudo systemctl start flowo-server
```

Downgrading to an older binary after a schema migration has run is not supported. If you need to roll back, restore from a backup taken before the upgrade.

---

## Troubleshooting

**"flowo-server: command not found" after install.**
The install script copies the binary to `/usr/local/bin/`. Confirm it's there: `ls -la /usr/local/bin/flowo-server`. If it's missing, rerun the install script or copy it manually.

**Service fails to start. How do I see why?**
```bash
sudo journalctl -u flowo-server --no-pager -n 50
```
The most recent 50 log lines usually show the error. Common causes: missing or malformed `.env` file, port already in use by another process, missing `FLOWO_TOKEN`.

**Workflow credentials aren't working.**
Check that the environment variable names in `.env` exactly match the convention (`FLOWO_CRED_` + uppercased credential ID with hyphens replaced by underscores). Restart the service after editing `.env`.

**Webhook requests time out or never arrive.**
Verify the port is open in your firewall (`sudo ufw status`). If you're using a reverse proxy, confirm it's running and pointing at the right port. Test from the server itself first: `curl -X POST http://127.0.0.1:3456/webhook -d '{}'`.

**"Previous run still in progress" keeps appearing.**
Only one instance of each workflow runs at a time. If your workflow takes longer than your schedule interval, the next run is skipped. Investigate what's causing slow execution, or adjust the interval.
