# Server Deployment

`flowo-server` is a headless binary that runs workflows on a Linux server without the desktop app. There are two modes:

- **Serve mode:** runs one specific workflow, exported from the desktop app. The simplest path for getting a single workflow running 24/7.
- **API mode:** manages many workflows on one server via a REST API.

---

## License notice for service operators

`flowo-server` is licensed under AGPL-3.0. If you run a **modified** version as a network-accessible service, AGPL-3.0 requires you to make your modifications available under the same license.

Running the **unmodified** binary, or modifying it solely for your own use with no other users, does not trigger this obligation.

If you need to keep your modifications proprietary, a commercial license is available. See the [Commercial licensing](../README.md#commercial-licensing) section of the README.

---

## Serve mode (single workflow)

### Step 1: Export from the desktop app

Open any workflow that has a Schedule or Webhook trigger. Click **File → Export for Server**.

Flowo generates a zip containing:

- `flowo-server`: the server binary (Linux x86-64, statically linked)
- `flowo-server.json`: the workflow config
- `.env.example`: a list of environment variables you need to set (one per credential used by the workflow)
- `flowo-server.service`: a systemd unit file
- `install.sh`: an install script that handles everything

### Step 2: Upload and install

Upload the zip to your Linux server and run the install script:

```bash
scp flowo-export.zip user@yourserver.com:/tmp/
ssh user@yourserver.com
cd /tmp && unzip flowo-export.zip && cd flowo-export
./install.sh
```

The install script:
1. Copies `flowo-server` to `/usr/local/bin/`
2. Creates `/etc/flowo/` and writes `flowo-server.json` there
3. Creates `/etc/flowo/.env` from `.env.example` (you fill in the values)
4. Installs and enables the systemd service

### Step 3: Set credentials

Open `/etc/flowo/.env` and fill in the credential values:

```bash
sudo nano /etc/flowo/.env
```

The variable names are derived from your credential IDs automatically:

| Credential ID | Environment variable |
|---|---|
| `openai-prod` | `FLOWO_CRED_OPENAI_PROD` |
| `slack-bot` | `FLOWO_CRED_SLACK_BOT` |
| `my.key` | `FLOWO_CRED_MY_KEY` |

Hyphens and dots become underscores; the name is uppercased; `FLOWO_CRED_` is prepended.

### Step 4: Start the service

```bash
sudo systemctl start flowo-server
sudo systemctl status flowo-server
```

The service starts on boot automatically. To check logs:

```bash
sudo journalctl -u flowo-server -f
```

### Status page

Serve mode exposes a status page at `http://127.0.0.1:<port>/` where `<port>` is the value in `flowo-server.json` (default `7700`). It shows:

- Workflow name and trigger type
- Last run time, status, and duration
- A Run button for manual trigger (requires the raw `run_secret` shown at export time)
- Run history (requires `run_secret`)
- Live logs via `GET /api/logs`

The status page binds to `127.0.0.1` by default. To expose it on a network interface, add `--bind 0.0.0.0` to the `ExecStart` line in the systemd unit file (but put it behind a reverse proxy with authentication first).

> **Warning:** Without a `run_secret` in `flowo-server.json`, the status page is **unauthenticated** and publicly accessible. It exposes the workflow name, trigger type, run counts, last-run status, and timestamps. If this information is sensitive, always export workflows with a `run_secret` set (the desktop app generates one automatically at export time).

### Serve mode CLI reference

```bash
flowo-server serve [OPTIONS]

Options:
  --config <path>                  Path to flowo-server.json. Default: flowo-server.json
  --port <port>                    Override the status page port from the config file
  --bind <addr>                    Interface to bind to. Default: 127.0.0.1
  --trusted-proxy-count <n>        Number of reverse-proxy hops to trust for X-Forwarded-For. Default: 0
  --allow-shell                    Enable Shell Command nodes (disabled by default)
  --allow-code                     Enable Code (JS) nodes (disabled by default)
  --reject-legacy-run-secret       Refuse to start if run_secret uses a legacy BLAKE3 hash (not argon2id)
  --ssrf-firewall-acknowledged     Suppress the SSRF egress firewall warning (set after firewall is configured)
```

Shell Command and Code (JS) nodes are disabled in serve mode by default. Pass `--allow-shell` or `--allow-code` only after auditing every node in the exported workflow.

---

## API mode (multiple workflows)

API mode manages many workflows on one server. It exposes a REST API secured with a bearer token.

### Start the server

```bash
flowo-server api --token mysecrettoken --port 7700
```

Or use environment variables:

```bash
export FLOWO_TOKEN=mysecrettoken
export FLOWO_PORT=7700
flowo-server api
```

If `--token` is not set, a random token is generated on first run and printed to stdout. Copy it. It is not shown again.

### API mode CLI reference

```bash
flowo-server api [OPTIONS]

Options:
  --token <token>                  Bearer token for authentication. Env: FLOWO_TOKEN
  --port <port>                    Port to listen on. Default: 7700. Env: FLOWO_PORT
  --data-dir <path>                Data directory for SQLite and the key file. Default: ~/.flowo-server. Env: FLOWO_DATA_DIR
  --bind <addr>                    Interface to bind to. Default: 127.0.0.1
  --allow-origin <origins>         Comma-separated additional CORS origins allowed
  --allow-env-vars <vars>          Comma-separated env vars workflows may read via {{$env.VAR}}. Disabled by default.
  --file-sandbox-dir <path>        Restrict File nodes to this directory tree
  --trusted-proxy-count <n>        Proxies to trust for X-Forwarded-For. Default: 0
  --allow-shell                    Enable Shell Command nodes
  --allow-code                     Enable Code (JS) nodes
  --keychain                       Store the encryption key in the OS keychain instead of a file
  --parallel-execution             Run independent workflow branches concurrently
  --max-concurrent-nodes <n>       Max nodes executing simultaneously (parallel mode). Default: 8
  --max-workflow-duration-secs <n> Max wall-clock time for any single execution
  --db-pool-size <n>               SQLite connection pool size. Env: FLOWO_DB_POOL_SIZE
  --ssrf-firewall-acknowledged     Suppress the SSRF egress firewall warning (set after firewall is configured)
```

### REST API

All endpoints require `Authorization: Bearer <token>`.

#### Workflows

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/workflows` | List all workflows. Accepts `?limit=100&offset=0`. |
| `GET` | `/api/workflows/:id` | Get a workflow by ID. |
| `POST` | `/api/workflows` | Save a workflow. Body: `{ "workflow_json": "..." }`. |
| `DELETE` | `/api/workflows/:id` | Delete a workflow and stop its scheduler. |
| `POST` | `/api/workflows/:id/run` | Trigger a manual run. |
| `GET` | `/api/workflows/:id/runs` | Run history. Accepts `?limit=50&offset=0&filter=success|failed`. |
| `GET` | `/api/workflows/:id/runs/:exec_id` | Single run detail. |
| `GET` | `/api/workflows/stream` | Server-sent events stream for live run updates. |

#### Scheduler

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/scheduler` | List all scheduled jobs and their status. |
| `POST` | `/api/scheduler/:id/start` | Start scheduling a workflow. |
| `POST` | `/api/scheduler/:id/stop` | Stop a scheduled workflow. |

#### Credentials

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/credentials` | List all credential IDs and names (values never returned). |
| `POST` | `/api/credentials` | Save a credential. Body: `{ "id": "...", "name": "...", "value": "..." }`. |
| `DELETE` | `/api/credentials/:id` | Delete a credential. |

#### Tokens

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/tokens` | List all API tokens. |
| `POST` | `/api/tokens` | Create a new token. Body: `{ "name": "...", "role": "read" or "write" }`. |
| `DELETE` | `/api/tokens/:id` | Delete a token. |

#### Health

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/health` | Returns `{ "status": "ok", "version": "..." }`. No auth required. |

### CLI control commands

These commands talk to a running API mode server:

```bash
# List all workflows
flowo-server list --token mytoken

# Check status
flowo-server status --token mytoken

# Stop a workflow (partial name match accepted)
flowo-server stop "my workflow" --token mytoken

# Start a stopped workflow
flowo-server start "my workflow" --token mytoken

# Restart a workflow
flowo-server restart "my workflow" --token mytoken
```

Use `--server http://yourserver.com:7700` to point at a remote server. Default is `http://localhost:7700`.

---

## Docker

A `Dockerfile` and `docker-compose.yml` are included in the repository. The Docker image builds a fully static musl-linked binary and runs it in a minimal Debian image.

### Quick start

```bash
# Clone the repo and build the image
git clone https://github.com/Panchak2d/flowo
cd flowo
docker compose up -d
```

Set `FLOWO_TOKEN` before starting:

```bash
FLOWO_TOKEN=mysecrettoken docker compose up -d
```

Or create a `.env` file:

```
FLOWO_TOKEN=mysecrettoken
```

The compose file binds to `127.0.0.1:7700` and creates a named volume `flowo_data` for persistent storage.

### Build the image manually

```bash
docker build -t flowo-server .
docker run -d \
  -p 127.0.0.1:7700:7700 \
  -v flowo_data:/data \
  -e FLOWO_TOKEN=mysecrettoken \
  -e FLOWO_PORT=7700 \
  flowo-server api
```

---

## HTTPS with a reverse proxy

The server binds to `127.0.0.1` by default. To expose it securely over the internet, put it behind Caddy or Nginx.

### Caddy (recommended: handles TLS automatically)

```
your-domain.com {
    reverse_proxy 127.0.0.1:7700
}
```

Caddy fetches and renews a Let's Encrypt certificate automatically. When you add Caddy as a proxy, set `--trusted-proxy-count 1` so Flowo reads the real client IP from `X-Forwarded-For` rather than Caddy's loopback address.

### Nginx

```nginx
server {
    listen 443 ssl;
    server_name your-domain.com;

    ssl_certificate     /etc/letsencrypt/live/your-domain.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/your-domain.com/privkey.pem;

    location / {
        proxy_pass http://127.0.0.1:7700;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $remote_addr;
    }
}
```

Set `--trusted-proxy-count 1` when starting flowo-server.

---

## Connecting the desktop app to API mode

Once `flowo-server api` is running, you can point the Flowo desktop app at it for remote workflow management. Go to **Settings → Server** in the desktop app and set the server URL and token.

Workflows saved through the desktop app are pushed to the server and scheduled there. Results are streamed back to the desktop in real time via server-sent events.

---

## Receiving webhooks on a server

The Webhook node in serve mode binds to `127.0.0.1` on whatever port you configure. External traffic can't reach `127.0.0.1` directly.

To receive webhooks from external services (GitHub, Stripe, etc.), the reverse proxy must forward traffic to the Webhook node's port. Example with Caddy:

```
your-domain.com/webhook {
    reverse_proxy 127.0.0.1:3456
}
```

Set the webhook URL in the external service to `https://your-domain.com/webhook`. Requests arriving there are forwarded to the Flowo Webhook node.

---

## Security notes for server deployments

- `--bind 0.0.0.0` exposes the server directly on all network interfaces. Always put a reverse proxy with TLS in front of it before doing this.
- Shell Command and Code (JS) nodes are disabled by default. Enable them only when you control and have audited all workflows running on the server.
- `--allow-env-vars` lets workflows read environment variables. Never include `FLOWO_TOKEN`, `FLOWO_CRED_*`, or any other sensitive variables in this list.
- `--file-sandbox-dir` restricts File nodes to a specific directory. Set this in production to prevent workflows from reading or writing arbitrary paths on the server.
- Set `--trusted-proxy-count` to exactly match the number of proxies between the internet and Flowo. Too high allows clients to spoof their IP via `X-Forwarded-For`.

### SSRF: egress firewall required for public deployments

The HTTP Request, Database, and AI nodes perform DNS pre-validation to block Server-Side Request Forgery (SSRF). However, a **TOCTOU (time-of-check/time-of-use) gap** exists between DNS resolution and the actual TCP connection. A malicious DNS server can return a public IP during the check and switch to a private IP (e.g. `169.254.169.254`) on the actual connect, bypassing the application-layer check entirely.

**This cannot be fixed in application code.** When running `flowo-server` on a network-accessible address, you **must** configure a host-level egress firewall:

```bash
# Block RFC-1918, loopback, link-local, and cloud metadata (iptables example)
iptables -A OUTPUT -d 10.0.0.0/8 -j DROP
iptables -A OUTPUT -d 172.16.0.0/12 -j DROP
iptables -A OUTPUT -d 192.168.0.0/16 -j DROP
iptables -A OUTPUT -d 127.0.0.0/8 -j DROP
iptables -A OUTPUT -d 169.254.0.0/16 -j DROP
iptables -A OUTPUT -d 168.63.129.16/32 -j DROP
```

See `docs/security.md` for the full recommended ruleset. Once your firewall is in place, pass `--ssrf-firewall-acknowledged` to suppress the startup warning.

### Legacy run_secret (serve mode)

> **Warning:** Always re-export workflows from the Flowo desktop app **0.3 or later** before deploying to a public server. Workflows exported with Flowo 0.2 or earlier store `run_secret` as a BLAKE3 hash, which is not brute-force resistant. If an attacker reads your `flowo-server.json` (e.g. from a misconfigured backup), a short `run_secret` can be cracked in seconds with a GPU.
>
> Pass `--reject-legacy-run-secret` to make the server refuse to start with a legacy hash.

### Database nodes and RUSTSEC-2023-0071

> **Warning:** Do **not** enable `--allow-database` in multi-tenant API mode where untrusted token holders can configure Database node connection strings. The `sqlx-mysql` dependency contains a timing side-channel in its RSA key exchange (RUSTSEC-2023-0071). In multi-tenant mode, a token holder who controls a MySQL connection string can measure handshake timing across many connections to recover session key material.
>
> This flag is safe in single-user or fully trusted deployments where you control all token holders. The upstream fix is tracked at [launchbadge/sqlx#3538](https://github.com/launchbadge/sqlx/issues/3538).

### Code node sandbox on macOS

When running `flowo-server api --allow-code --code-sandbox` on macOS, the ESM module import restrictions apply (blocking `fs`, `net`, `child_process`, etc.), but **CPU and memory resource limits (`setrlimit`) are Linux-only**. On macOS, a runaway script can exhaust system CPU and memory ; only the `timeout_secs` ceiling (max 60 s) applies. Deploy on Linux for full sandboxing enforcement.

See [Security](security.md) for the full security model.

---

## Receiving webhooks from the internet

The Webhook node binds to `127.0.0.1` (localhost only). External services like Stripe, GitHub, and Twilio cannot reach it directly without a reverse proxy or tunnel.

For desktop/local deployments: see [webhooks-public.md](webhooks-public.md) for tunnel options (cloudflared, ngrok).

For server deployments: configure Caddy or Nginx to forward requests to the Webhook node's port as shown in the reverse proxy section above.

---

## SSE event scope

> **Note:** `GET /api/events` delivers execution events for **all** workflows to any token with `read` scope. In multi-user deployments where token holders should not see each other's workflow activity, avoid issuing read tokens to untrusted parties until per-workflow ACL is implemented.
