# Server Deployment

`flowo-server` runs your workflows on a Linux server 24/7 — no desktop app needed on the server side. There are two modes: **serve mode** for deploying a single workflow, and **API mode** for managing many workflows remotely.

---

## Which mode to use

| Situation | Mode |
|---|---|
| Deploy one workflow to a server | **Serve mode** — use the Export for Server button |
| Manage many workflows on one server | **API mode** |
| Local testing before deploying | Either — both work on your own machine |

---

## Prerequisites

Before you start, you need:

- **A Linux server.** Any VPS (virtual private server) from a provider like DigitalOcean, Hetzner, Linode, Vultr, or AWS EC2 will work. Ubuntu 22.04 LTS is recommended. 1 vCPU and 1 GB RAM is sufficient for most workflows.
- **SSH access to that server.** Your provider gives you an IP address and either a root password or an SSH key when you create the server.
- **`flowo-server` binary.** See [Getting the binary](#getting-the-binary) below.
- **Node.js on the server (optional).** Only required if any of your workflows use the **Code (JS)** node. See [Installing Node.js on the server](#installing-nodejs-on-the-server).

### Connecting to your server via SSH

SSH is how you run commands on a remote server from your own computer's terminal (Terminal on macOS/Linux, PowerShell or Windows Terminal on Windows).

```bash
# Replace 203.0.113.10 with your server's actual IP address
# Replace ubuntu with your server's username (often root, ubuntu, or debian)
ssh ubuntu@203.0.113.10
```

Your provider's control panel shows the IP address and default username. The first time you connect, your terminal asks you to confirm the server's fingerprint — type `yes`.

If your provider gave you an SSH key file instead of a password:

```bash
ssh -i /path/to/your-key.pem ubuntu@203.0.113.10
```

---

## Getting the binary

The `flowo-server` binary is what actually runs on your server. There are two ways to get it there.

### Option A — Export it from the desktop app (Serve mode only)

When you use the **Export → Export for Server** button, the generated zip already includes the `flowo-server` binary compiled for Linux. You don't need to do anything separately — the binary is inside the zip.

### Option B — Build from source on the server

If you need API mode, or want the latest build, compile it directly on the server:

```bash
# 1. Install Rust on the server
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"

# 2. Clone the repository
git clone https://github.com/Panchak2d/flowo
cd flowo

# 3. Build the server binary (takes 3–5 minutes the first time)
cargo build --release -p flowo-server

# 4. Copy the binary to a permanent location
sudo cp target/release/flowo-server /usr/local/bin/flowo-server
sudo chmod +x /usr/local/bin/flowo-server

# 5. Verify it works
flowo-server --help
```

Once the binary is at `/usr/local/bin/flowo-server`, you can run `flowo-server` from anywhere on the server.

### Installing Node.js on the server

Only needed if your workflows use the **Code (JS)** node.

```bash
# Install Node.js 20 LTS
curl -fsSL https://deb.nodesource.com/setup_20.x | sudo -E bash -
sudo apt-get install -y nodejs

# Verify
node --version   # should print v20.x.x
```

---

## Opening the port in your firewall

Your server may have a firewall blocking incoming traffic. You need to open port `7700` so the status page and API are reachable.

**Cloud provider firewall (Security Groups / Firewall rules):**

Most providers have a firewall in their control panel that you configure before traffic even reaches your server. Look for "Firewall", "Security Group", or "Networking" in your provider's dashboard. Add an inbound rule allowing TCP on port 443 (HTTPS) and, during initial testing, port 7700.

> Once you set up HTTPS with a reverse proxy (see [HTTPS](#https-required-for-any-non-localhost-exposure)), remove port 7700 from public access and only keep 443 open.

**Server-side firewall (ufw):**

Ubuntu ships with `ufw`. If it is enabled on your server:

```bash
# Check whether ufw is active
sudo ufw status

# Allow HTTPS (needed if using Caddy or Nginx later)
sudo ufw allow 443

# Allow port 7700 for initial testing — lock it down once HTTPS is set up
sudo ufw allow 7700

sudo ufw reload
```

---

## Serve mode

This is what the **Export → Export for Server** button produces. You get a self-contained zip, upload it to your server, and run `./install.sh`. The workflow runs as a background service that survives server reboots.

### Step 1 — Export the package from the desktop app

1. Open the workflow you want to deploy.
2. The workflow must have a **Schedule** or **Webhook** trigger node as its first node. Workflows that start with a Manual Trigger cannot be exported.
3. Go to the **Export → Export for Server** button.
4. Set the status page port (default: `7700`).
5. Click **Generate Package**.

The export panel lists which credentials the workflow uses and the environment variable name each one maps to. Note these — you will fill them in on the server.

### Step 2 — Upload the zip to your server

On your **own computer** (not the server), open a terminal and run:

```bash
# Replace the filename, username, and IP with your own values
scp flowo-server-MyWorkflow.zip ubuntu@203.0.113.10:~/
```

`scp` works like `ssh` but copies a file. The `~/` at the end puts the file in the home directory on the server.

### Step 3 — Extract and configure on the server

SSH into your server, then run:

```bash
# Extract the zip
unzip flowo-server-MyWorkflow.zip

# Go into the extracted folder
cd flowo-server-MyWorkflow

# See which credentials the workflow needs
cat .env.example
```

Copy the example file and fill in your real values:

```bash
cp .env.example .env
nano .env
```

Replace each placeholder with your actual credentials. For example:

```
FLOWO_CRED_OPENAI_PROD=sk-abc123...
FLOWO_CRED_SLACK_BOT=xoxb-...
```

Press `Ctrl+X`, then `Y`, then `Enter` to save and exit nano.

### Step 4 — Run the installer

```bash
chmod +x install.sh
./install.sh
```

The installer:
- Copies files to `~/.flowo-server/MyWorkflow/`
- Creates a systemd user service
- Starts the service immediately
- Enables it to start automatically on server reboots
- Enables "linger" so the service keeps running after you log out of SSH

### Step 5 — Verify it is running

```bash
# Check the service status
systemctl --user status flowo-MyWorkflow

# Follow live logs (press Ctrl+C to stop)
journalctl --user -u flowo-MyWorkflow -f
```

Open a browser and go to `http://203.0.113.10:7700`. You should see a status page showing whether the workflow is running or waiting, when it last ran, and the last 50 log lines.

> If the status page is not reachable, check your cloud provider's firewall/security group rules and make sure port 7700 is open. See [Opening the port](#opening-the-port-in-your-firewall).

### Managing the service

```bash
systemctl --user stop    flowo-MyWorkflow     # stop the workflow
systemctl --user start   flowo-MyWorkflow     # start it again
systemctl --user restart flowo-MyWorkflow     # required after editing .env
systemctl --user disable flowo-MyWorkflow     # stop auto-starting on boot
```

### Updating the workflow

1. Edit the workflow in Flowo Desktop.
2. Go to the **Export → Export for Server** button → **Generate Package**.
3. Upload the new zip to the server with `scp` and run `./install.sh` again. It updates in place and restarts the service automatically.

### Running without systemd

If your server does not use systemd, you can run the server directly:

```bash
cd ~/.flowo-server/MyWorkflow
source .env
./flowo-server serve --config flowo-server.json
```

Press `Ctrl+C` to stop. The workflow stops when this process stops, so this is only suitable for testing.

> `flowo-server.json` contains a `run_secret` in plaintext. Restrict access: `chmod 600 flowo-server.json`.

---

## API mode

API mode runs one `flowo-server` process that manages many workflows. You upload workflows, start and stop them, and manage credentials — all via HTTP requests or the built-in CLI commands.

The `flowo-server` binary is self-contained: it includes the scheduler, executor, workflow database, credential store, and a CLI client. There are no external runtime dependencies except Node.js (only if workflows use Code nodes).

### Step 1 — Get the binary onto your server

Follow [Option B — Build from source on the server](#option-b--build-from-source-on-the-server) to compile and install the binary. After that step, `flowo-server` is available as a command everywhere on the server.

Verify:

```bash
which flowo-server          # should print /usr/local/bin/flowo-server
flowo-server --help
```

### Step 2 — First run

SSH into your server and run:

```bash
flowo-server api
```

On first run, the server generates a random API token and prints it:

```
┌──────────────────────────────────────────────────────┐
│  Flowo Server — API Token (save this somewhere safe)  │
│                                                        │
│  a3f9c2d1e8b74f6a9c2d1e8b74f6a9c2d1e8b74f6a9c2d1  │
│                                                        │
│  Set FLOWO_TOKEN env var to skip this on restart.     │
└──────────────────────────────────────────────────────┘
```

**Copy this token and save it somewhere safe** — a password manager, a notes app, anywhere offline. Every API request and CLI command requires it. If you lose it, you will need to reset the server's data directory.

Press `Ctrl+C` to stop the server. You will run it as a persistent service in the next step.

### Step 3 — Run as a persistent systemd service

Running `flowo-server api` directly means it stops when you close your SSH session. To keep it running permanently, set it up as a systemd service.

Still on the server, run these commands. Replace `your-token-here` with the token you saved in Step 2:

```bash
# Create the systemd user service directory if it doesn't exist
mkdir -p ~/.config/systemd/user

# Write the service file
cat > ~/.config/systemd/user/flowo-api.service << 'EOF'
[Unit]
Description=Flowo API Server
After=network.target

[Service]
Type=simple
WorkingDirectory=%h/.flowo-server
Environment=FLOWO_TOKEN=your-token-here
Environment=FLOWO_PORT=7700
Environment=FLOWO_DATA_DIR=%h/.flowo-server
ExecStart=/usr/local/bin/flowo-server api
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
EOF

# Reload systemd so it picks up the new file
systemctl --user daemon-reload

# Enable the service (starts automatically on boot)
systemctl --user enable flowo-api

# Start it now
systemctl --user start flowo-api

# Allow the service to keep running after you log out of SSH
loginctl enable-linger $USER
```

The `loginctl enable-linger` step is important — without it, systemd stops all user services the moment you log out.

Check it is running:

```bash
systemctl --user status flowo-api
journalctl --user -u flowo-api -f
```

Look for a log line like `Flowo Server started in API mode` and the URL it is listening on.

### Step 4 — Upload a workflow from your computer

On your **own computer** (not the server), export a workflow from the Flowo desktop app using the **Export → Export as .flowo** button. This produces a `.flowo` file.

Then upload it to the server from your computer's terminal:

```bash
# Replace 203.0.113.10 with your server IP and YOUR_TOKEN with your token
curl -X POST \
  -H "Authorization: Bearer YOUR_TOKEN" \
  -H "Content-Type: application/json" \
  -d "{\"workflow_json\": $(cat MyWorkflow.flowo | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()))')}" \
  http://203.0.113.10:7700/api/workflows
```

The server responds with a JSON object that includes the workflow's `id`. Copy that ID — you need it to start and manage the workflow.

### Step 5 — Start the workflow on its schedule

```bash
# Replace WORKFLOW_ID with the id from Step 4
# always_on: true means it restarts automatically if the server restarts
curl -X POST \
  -H "Authorization: Bearer YOUR_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"always_on": true}' \
  http://203.0.113.10:7700/api/scheduler/WORKFLOW_ID/start
```

### Step 6 — Add credentials the workflow needs

If the workflow uses API keys (Slack, OpenAI, etc.), save them to the server's credential store:

```bash
curl -X POST \
  -H "Authorization: Bearer YOUR_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"id":"openai-prod","name":"OpenAI API Key","value":"sk-..."}' \
  http://203.0.113.10:7700/api/credentials
```

Restart the workflow after adding credentials:

```bash
flowo-server restart "MyWorkflow" \
  --server http://203.0.113.10:7700 \
  --token YOUR_TOKEN
```

### CLI commands

The CLI commands are HTTP clients that work from the server itself or from your own computer:

```bash
# Set these once to avoid repeating them in every command
export FLOWO_TOKEN=your-token-here
export FLOWO_SERVER=http://203.0.113.10:7700

# List all workflows and their status
flowo-server list

# Stop, start, restart a workflow by name (partial name match works)
flowo-server stop    "MyWorkflow"
flowo-server start   "MyWorkflow"
flowo-server restart "MyWorkflow"
```

Without the environment variables, pass them as flags:

```bash
flowo-server list --server http://203.0.113.10:7700 --token YOUR_TOKEN
```

### Run a workflow immediately

```bash
curl -X POST \
  -H "Authorization: Bearer YOUR_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"initial_variables":{}}' \
  http://203.0.113.10:7700/api/workflows/WORKFLOW_ID/run
```

### Full API reference

All endpoints except `/api/health` require `Authorization: Bearer YOUR_TOKEN`.

| Method | Path | What it does |
|---|---|---|
| GET | `/api/health` | Check server is alive (no auth required) |
| GET | `/api/workflows` | List all workflows |
| POST | `/api/workflows` | Upload a workflow (`{"workflow_json":"..."}`) |
| GET | `/api/workflows/:id` | Get a workflow's JSON |
| DELETE | `/api/workflows/:id` | Delete a workflow |
| POST | `/api/workflows/:id/run` | Run a workflow immediately |
| GET | `/api/scheduler` | List all scheduled/running jobs |
| POST | `/api/scheduler/:id/start` | Start a workflow on schedule |
| POST | `/api/scheduler/:id/stop` | Stop a running workflow |
| GET | `/api/credentials` | List credentials (names only, not values) |
| POST | `/api/credentials` | Save a credential |
| DELETE | `/api/credentials/:id` | Delete a credential |
| GET | `/api/events` | SSE stream of live node and scheduler events |

### Live event stream

```bash
curl -N \
  -H "Authorization: Bearer YOUR_TOKEN" \
  http://203.0.113.10:7700/api/events
```

Events look like:

```
data: {"event":"node-status","payload":{"node_id":"abc","status":"running"}}
data: {"event":"scheduler-status","payload":{"workflow_id":"xyz","status":"waiting","run_count":5}}
```

---

## Run API mode as a systemd service

Already covered in [Step 3 of API mode](#step-3--run-as-a-persistent-systemd-service) above. This is the recommended way to run `flowo-server api` in production.

---

## Docker

Docker lets you run `flowo-server` in an isolated container without installing Rust or managing the binary manually.

### Installing Docker on the server

If Docker is not already installed:

```bash
# Install Docker on Ubuntu
curl -fsSL https://get.docker.com | sh

# Add your user to the docker group so you don't need sudo for every command
sudo usermod -aG docker $USER

# Log out and SSH back in for the group change to take effect
exit
```

SSH back in, then verify:

```bash
docker --version
docker run hello-world
```

### Build the image

Clone the repository on the server and build:

```bash
git clone https://github.com/Panchak2d/flowo
cd flowo
docker build -t flowo-server .
```

The build takes a few minutes.

### Run the container

```bash
docker run -d \
  -p 127.0.0.1:7700:7700 \
  -v "$HOME/.flowo-server:/data" \
  -e FLOWO_TOKEN=your-token-here \
  --name flowo \
  --restart unless-stopped \
  flowo-server
```

Key options explained:
- `-p 127.0.0.1:7700:7700` — binds port 7700 to localhost only, so it is **not** directly reachable from the internet. Traffic must go through your reverse proxy (see [HTTPS](#https-required-for-any-non-localhost-exposure)).
- `-v "$HOME/.flowo-server:/data"` — stores all data on the host so it survives container restarts and rebuilds.
- `--restart unless-stopped` — Docker restarts the container automatically after a crash or server reboot.

If you do not provide `FLOWO_TOKEN`, the server generates one and prints it to logs:

```bash
docker logs flowo 2>&1 | grep -A 5 "API Token"
```

### docker-compose

A `docker-compose.yml` is included at the project root:

```bash
# Create a .env file with your token
echo "FLOWO_TOKEN=your-token-here" > .env

# Start in the background
docker compose up -d

# Follow logs
docker compose logs -f

# Stop
docker compose down
```

### Persistent data

The container stores all data under `/data` inside the container, which maps to `~/.flowo-server` on the host:

- `flowo.db` — workflows and run history
- `credentials.db` — encrypted credentials
- `flowo.key` — the encryption key for credentials

**Back up `~/.flowo-server` regularly.** If `flowo.key` is deleted, stored credentials cannot be recovered.

---

## Token management (API mode)

API mode now supports multiple tokens. Each token has a label, one or more scopes, and can be revoked without restarting the server.

### Scopes

| Scope | What it allows |
|---|---|
| `read` | GET endpoints (list workflows, list credentials, etc.) |
| `write` | POST and DELETE on workflows, credentials, and scheduler |
| `admin` | Everything above + token management (`/api/tokens`) |

The default token created on first run has `read`, `write`, and `admin` scopes.

### Backward compatibility

Existing deployments using `FLOWO_TOKEN` or `--token` are unaffected. On first startup after upgrading, the existing token is automatically imported into the token store as an admin token with label `"default"`. The token value stays the same — no client changes needed.

### Create a token

```bash
# Create a read-only token (e.g. for a dashboard)
flowo-server tokens create \
  --label "dashboard" \
  --scopes read \
  --token YOUR_ADMIN_TOKEN

# Create a read-write token (e.g. for CI)
flowo-server tokens create \
  --label "ci" \
  --scopes read,write \
  --token YOUR_ADMIN_TOKEN
```

The raw token is printed once. Save it immediately — it cannot be retrieved later.

Via API:

```bash
curl -X POST \
  -H "Authorization: Bearer YOUR_ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"label":"ci","scopes":["read","write"]}' \
  http://localhost:7700/api/tokens
```

Response:

```json
{
  "token":  "a3f9c2d1e8b74f6a...",
  "label":  "ci",
  "scopes": ["read", "write"],
  "note":   "Save this token — it will not be shown again."
}
```

### List tokens

```bash
flowo-server tokens list --token YOUR_ADMIN_TOKEN
```

Or via API:

```bash
curl -H "Authorization: Bearer YOUR_ADMIN_TOKEN" \
  http://localhost:7700/api/tokens
```

Returns all tokens with their `token_id`, label, scopes, creation time, and revocation time (if revoked).

### Revoke a token

Get the `token_id` from `tokens list`, then:

```bash
flowo-server tokens revoke TOKEN_ID --token YOUR_ADMIN_TOKEN
```

Or via API:

```bash
curl -X DELETE \
  -H "Authorization: Bearer YOUR_ADMIN_TOKEN" \
  http://localhost:7700/api/tokens/TOKEN_ID
```

Revocation is immediate. The revoked token returns `401` on any subsequent request. Revocation is permanent — revoked tokens cannot be re-activated.

> **Warning:** If you revoke all admin tokens, token management is locked out. You can recover by stopping the server, deleting `tokens.db` from the data directory, and restarting — the server will re-import the `FLOWO_TOKEN` env var as a new admin token (or generate a fresh one if `FLOWO_TOKEN` is unset).

### Token storage

Tokens are stored in `tokens.db` in the data directory (default: `~/.flowo-server/`). Token hashes (BLAKE3) are stored, not raw values. Back up this file alongside `flowo.db` and `credentials.db`.

### Token management API reference

All token endpoints require `Authorization: Bearer <admin-token>` with `admin` scope.

| Method | Path | What it does |
|---|---|---|
| GET | `/api/tokens` | List all tokens |
| POST | `/api/tokens` | Create a token (`{"label":"...","scopes":["read","write"]}`) |
| DELETE | `/api/tokens/:id` | Revoke a token by `token_id` |

---

`flowo-server` does not handle TLS (HTTPS) itself. The `Authorization: Bearer` token travels in plaintext over HTTP. **If your server is reachable from the internet, you must put a TLS-terminating reverse proxy in front of it. Never expose port 7700 directly to the internet.**

A reverse proxy accepts HTTPS connections on port 443 and forwards them to `flowo-server` on port 7700, which stays bound to `localhost` and is not publicly reachable.

### Step 1 — Point a domain at your server

You need a domain name (e.g. `api.yourdomain.com`) pointing to your server's IP address. In your domain registrar's DNS settings, add an A record:

```
Type:  A
Name:  api            (or @ for the root domain)
Value: 203.0.113.10   (your server's IP address)
TTL:   300
```

DNS changes can take a few minutes to an hour to propagate. Test from your own computer:

```bash
ping api.yourdomain.com
# The IP address shown should match your server's IP
```

Wait until the IP resolves correctly before continuing.

### Option A — Caddy (recommended, easiest)

Caddy automatically gets and renews Let's Encrypt TLS certificates. No manual certificate management needed.

**Install Caddy on the server:**

```bash
sudo apt install -y debian-keyring debian-archive-keyring apt-transport-https curl
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | sudo gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' | sudo tee /etc/apt/sources.list.d/caddy-stable.list
sudo apt update
sudo apt install caddy
```

**Create the Caddyfile:**

```bash
sudo nano /etc/caddy/Caddyfile
```

Replace everything in the file with:

```
api.yourdomain.com {
    reverse_proxy localhost:7700
}
```

Save and exit (`Ctrl+X`, then `Y`, then `Enter`).

**Reload Caddy:**

```bash
sudo systemctl reload caddy
```

Caddy fetches a certificate automatically within seconds. Your server is now reachable at `https://api.yourdomain.com`.

Update your CLI and curl commands to use the HTTPS URL:

```bash
export FLOWO_SERVER=https://api.yourdomain.com
flowo-server list --token YOUR_TOKEN
```

### Option B — Nginx + Certbot

**Install Nginx and Certbot:**

```bash
sudo apt update
sudo apt install -y nginx certbot python3-certbot-nginx
```

**Get a TLS certificate:**

```bash
sudo certbot --nginx -d api.yourdomain.com
```

Certbot prompts for your email address and automatically edits your Nginx config to enable TLS.

**Verify or manually set the Nginx config:**

```bash
sudo nano /etc/nginx/sites-available/flowo
```

The file should contain:

```nginx
server {
    listen 443 ssl;
    server_name api.yourdomain.com;
    ssl_certificate     /etc/letsencrypt/live/api.yourdomain.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/api.yourdomain.com/privkey.pem;

    location / {
        proxy_pass http://127.0.0.1:7700;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header Host $host;
    }
}

server {
    listen 80;
    server_name api.yourdomain.com;
    return 301 https://$host$request_uri;
}
```

Enable the site and reload Nginx:

```bash
sudo ln -s /etc/nginx/sites-available/flowo /etc/nginx/sites-enabled/
sudo nginx -t          # test for config errors before reloading
sudo systemctl reload nginx
```

Test automatic certificate renewal:

```bash
sudo certbot renew --dry-run
```

### Block direct access to port 7700

After setting up a reverse proxy, block direct access so all traffic goes through HTTPS:

```bash
# ufw
sudo ufw deny 7700
sudo ufw allow 443
sudo ufw allow 80      # needed for HTTP → HTTPS redirect and certificate renewal
sudo ufw reload
```

Also remove port 7700 from your cloud provider's firewall/security group rules.

---

## Environment variable expressions

Workflows support `{{$env.VAR_NAME}}` to inject environment variables into node inputs at runtime. This is disabled by default in server mode to prevent workflows from reading sensitive variables like `FLOWO_TOKEN` or `AWS_SECRET_ACCESS_KEY`.

To allow specific variables:

```bash
flowo-server api --allow-env-vars APP_ENV,REGION,LOG_LEVEL
```

Only the listed names resolve. Any `{{$env.OTHER_VAR}}` not in the list resolves to an empty string with a warning logged.

In the systemd unit, add to the `ExecStart` line:

```ini
ExecStart=/usr/local/bin/flowo-server api \
  --allow-env-vars APP_ENV,REGION
```

**Never add `FLOWO_TOKEN` or any `FLOWO_CRED_*` variable to this list.** Use the credential store (`/api/credentials`) instead.

---

## Security

### Shell and Code nodes (disabled by default)

The **Shell Command** and **Code (JS)** nodes execute arbitrary commands and JavaScript on the server with the same privileges as the `flowo-server` process. In API mode, any holder of a `write`-scope token can run arbitrary shell commands or JS code by uploading a workflow that contains those nodes.

**Both node types are disabled by default.** Any workflow that contains them returns a `SHELL_DISABLED` or `CODE_DISABLED` error immediately — no subprocess is spawned.

To opt in to shell or code execution, pass the flag explicitly:

```bash
flowo-server api --disable-shell=false           # allow shell, code still disabled
flowo-server api --disable-code=false            # allow JS, shell still disabled
flowo-server api --disable-shell=false --disable-code=false   # allow both
```

When a node is disabled:
- The workflow continues normally; only that node fails.
- The node outputs a `SHELL_DISABLED` or `CODE_DISABLED` error that appears in the run logs.
- No subprocess is spawned.

**Upgrade note:** If you were relying on Shell or Code nodes in a previous deployment, add `--disable-shell=false --disable-code=false` to your startup command or systemd unit.

In the systemd unit:

```ini
ExecStart=/usr/local/bin/flowo-server api \
  --disable-shell=false \
  --disable-code=false
```

---

## Logging

```bash
RUST_LOG=info  flowo-server api   # default
RUST_LOG=debug flowo-server api   # verbose
RUST_LOG=warn  flowo-server api   # warnings and errors only
```

In the systemd unit, add to the `[Service]` block:

```ini
Environment=RUST_LOG=info
```

### Structured JSON logs

For log aggregators (Loki, Datadog, CloudWatch):

```bash
FLOWO_LOG_FORMAT=json flowo-server api
```

Each line is a JSON object:

```json
{"timestamp":"2025-01-15T08:00:01.234Z","level":"INFO","fields":{"message":"Flowo Server started in API mode","api_url":"http://0.0.0.0:7700"},"target":"flowo_server::api_server"}
```

Parse with `jq`:

```bash
journalctl --user -u flowo-api -o cat | jq .
```

---

## Troubleshooting

**"Cannot bind port 7700"** — Something else is using that port. Change it: `flowo-server api --port 7800`. Update your systemd unit and reverse proxy config to match.

**"Missing required environment variables" (serve mode)** — Edit `.env`, fill in the listed variables, then `systemctl --user restart flowo-MyWorkflow`.

**"Invalid workflow JSON"** — Re-export from the desktop app. The JSON format may have changed between versions.

**Status page / API not reachable from browser** — Check two things: (1) your cloud provider's security group/firewall rules allow inbound TCP on port 7700 (or 443 if using HTTPS); (2) `ufw` on the server allows the port (`sudo ufw status`).

**Workflow runs but errors immediately** — Check `journalctl --user -u flowo-MyWorkflow -f`. Common causes: missing credential, network unreachable from server, Shell Command node uses a tool not installed on the server.

**Shell Command node works on desktop, fails on server** — The server runs Linux. Commands like `pbcopy` (macOS) or PowerShell don't exist. Check the tool is installed: `which yourcommand`. Install it if needed: `sudo apt install yourpackage`.

**Desktop Notification node errors on server** — Expected. The error is logged and the workflow continues.

**Service stops after logout** — Run `loginctl enable-linger $USER`. This lets systemd user services keep running after you log out.

**Webhook workflow never triggers** — Confirm the sending service can reach `127.0.0.1:PORT` on the server (use a reverse proxy to expose it externally). Check that `path`, `method`, and `secret` in the node config match what the sender is sending. The node waits for `timeout_secs` (default 60) for a valid request to arrive.

**Docker: port not reachable after starting container** — Verify the container is running (`docker ps`), check your cloud provider's firewall allows the port, and confirm your reverse proxy is pointing to the right port.
