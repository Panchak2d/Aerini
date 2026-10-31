# New Server, Start to Finish

This page takes you from a workflow that runs on your desktop to the same workflow running on a fresh Linux server, with HTTPS if it starts on a Webhook. It follows the **Export for Server → Docker** package. [Server Deployment](server-deploy.md) is the reference for everything this page touches; follow its links when you want the reasons or the other shapes (bare metal, `api` mode).

## Before you start

- A workflow you have already run on the desktop, with every credential it needs saved there.
- A server you can SSH into: x86-64, Ubuntu or Debian. The official image is published for x86-64 only; on ARM, use the source-build files in the package (see [Troubleshooting](#troubleshooting)).
- If the workflow starts on a **Webhook**: a domain name you can add a DNS record to, and a secret set in the Webhook node. Without a secret, anyone who finds the URL can run the workflow.

A workflow that starts on a Schedule or by hand needs no domain and no open web ports. Skip step 3 for it, and in step 5 allow only SSH.

## 1. Export the package

1. Open the workflow and choose **Export → Export for Server**.
2. Check the **Trigger** line and the **Credentials** list. Each row names the environment variable the server will read.
3. Pick the **Docker** tab, leave the **Status page port** at `7700` unless it clashes with something on the server, and click **Generate Docker Package**.
4. Save the zip. It holds the hash of the workflow's run secret, so treat it like a credentials file: do not commit it or send it over an unencrypted channel.

## 2. Create the server and upload the package

Create the server with any provider and sign in over SSH. Then, from your own computer:

```bash
scp aerini-package.zip you@your-server:~
```

Use the zip's real file name. On the server, install `unzip` if it is missing, then unpack:

```bash
sudo apt install unzip
unzip aerini-package.zip -d aerini && cd aerini
```

## 3. Point a domain at the server (Webhook only)

Add an `A` record (or `AAAA` for IPv6) for the name you want, such as `hooks.example.com`, pointing at the server's public address. Caddy asks for the HTTPS certificate when it starts, so the record has to resolve first.

## 4. Install Docker

Follow Docker's own instructions for your distribution: [Install Docker Engine on Ubuntu](https://docs.docker.com/engine/install/ubuntu/), or the matching page for Debian. Use Docker's repository, not a distribution package, so that you get the Compose plugin. The package needs Docker Engine 20.10 or later with Compose V2. Check:

```bash
docker --version
docker compose version
```

On Debian and Ubuntu the Docker service starts at boot by default; see [Server Deployment §Docker](server-deploy.md#docker-recommended) for other systems.

## 5. Open the firewall (Webhook only)

Close everything except SSH, HTTP and HTTPS. Allow SSH first, or you lock yourself out:

```bash
sudo ufw allow ssh
sudo ufw allow 80/tcp
sudo ufw allow 443/tcp
sudo ufw allow 443/udp
sudo ufw enable
```

Also open ports 80 and 443 in your provider's own firewall or security group if it has one. Caddy needs them to get and renew the certificate. Docker publishes the ports in the package's Compose file through its own rules, ahead of ufw's, so what the Compose file publishes is what is reachable. The package binds the status page to `127.0.0.1` for that reason; do not change that mapping.

## 6. Fill in credentials and start

```bash
cp .env.example .env
nano .env
docker compose up -d
```

In `.env`, give each credential variable its value. For a Webhook workflow, also set `AERINI_DOMAIN` to the name from step 3. The `docker-compose.yml` refuses to start without it. `.env` holds secrets; keep it on the server only.

The first start pulls the official image, pinned to the release that exported the package.

## 7. Check that it works

On the server:

```bash
docker compose ps
curl http://127.0.0.1:7700/api/health
```

Use your own status port if you changed it. The health route needs no token. To open the status page from your own computer, tunnel to it:

```bash
ssh -L 7700:127.0.0.1:7700 you@your-server
```

Then browse to `http://localhost:7700`.

For a Webhook workflow, call it from anywhere (use the package's own `README.md` for your exact path and method):

```bash
curl -X POST https://hooks.example.com/webhook \
  -H "x-webhook-secret: <your-webhook-secret>" \
  -H "content-type: application/json" -d '{"hello": "world"}'
```

Only the webhook's path and method are served; everything else answers 404, and the status page is not reachable through Caddy.

## Keeping it running

- **Change the workflow:** edit it on the desktop, export again, replace `aerini-server.json` on the server with the one from the new zip, then run `docker compose restart`.
- **Update Aerini itself:** export again from a newer app. See [Updating](updating.md).
- **Back up:** see [Backup and restore](server-deploy.md#backup-and-restore). Keep the backup off the server.
- **Logs:** `docker compose logs -f`.

## Troubleshooting

| What you see | Likely cause |
|---|---|
| `Set AERINI_DOMAIN in .env` | A Webhook export needs `AERINI_DOMAIN` set before `docker compose up -d`. |
| Caddy cannot get a certificate | The DNS record has not reached the server yet, or port 80 or 443 is closed in the provider's firewall. |
| Docker reports the image was not found, or the server is ARM | The image is published for x86-64 and for released versions only. Build from source: `docker compose -f docker-compose.yml -f docker-compose.build.yml up --build -d`. |
| A node fails although the workflow runs | Shell, Code and Database nodes are off by default. The package's README lists the flags to enable them, after you have audited the workflow. |
| A credential field is empty in a node | The variable in `.env` is missing or misnamed. The names in `.env.example` are the ones the server reads. |

For anything else, see [Server Deployment](server-deploy.md) and [Troubleshooting](../troubleshooting.md).
