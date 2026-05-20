# Credentials

Credentials are how Flowo stores your API keys and secrets securely. Instead of pasting a key directly into a node's config (which would be visible in the workflow JSON), you store it once in the credential store and reference it by name. The actual value is encrypted on disk and only decrypted in memory at the moment a node runs.

---

## Add a credential

1. Click the **Connections** button in the sidebar (lock icon).
2. Click **Add Credential**.
3. Set an **ID** — a short slug used to reference this credential in nodes (e.g. `openai-prod`, `slack-bot`). IDs are case-sensitive. Use lowercase letters, numbers, and hyphens — no spaces.
4. Set a **Name** — the human-readable label shown in dropdowns (e.g. `OpenAI API Key`).
5. Paste the **Value** — the actual secret.
6. Save.

The value is encrypted immediately on save. It's never stored in plaintext anywhere on disk.

---

## Use a credential in a node

In any node that accepts an API key, the config panel shows a credential dropdown next to the key field. Select the credential by its name. The workflow JSON stores only the credential ID — the actual value is looked up at runtime and never written to disk in plaintext or sent to the frontend.

---

## Getting API keys for common services

Here's where to find the API key for each supported service.

### OpenAI

1. Go to [platform.openai.com/api-keys](https://platform.openai.com/api-keys).
2. Click **Create new secret key**.
3. Give it a name (e.g. `Flowo`), copy the key — it starts with `sk-`.
4. Save it immediately. OpenAI only shows it once.

Add as a credential with ID `openai` and paste the key as the value.

### Anthropic (Claude)

1. Go to [console.anthropic.com/settings/keys](https://console.anthropic.com/settings/keys).
2. Click **Create Key**.
3. Copy the key — it starts with `sk-ant-`.

### Slack

1. Go to [api.slack.com/apps](https://api.slack.com/apps).
2. Create a new app or open an existing one.
3. Go to **OAuth & Permissions → Bot Token Scopes** and add `chat:write`.
4. Click **Install to Workspace**.
5. Copy the **Bot User OAuth Token** — it starts with `xoxb-`.

Before the bot can post to a channel, you must invite it: in Slack, open the channel and type `/invite @YourBotName`.

### SendGrid

1. Go to [app.sendgrid.com/settings/api_keys](https://app.sendgrid.com/settings/api_keys).
2. Click **Create API Key**.
3. Choose **Restricted Access**, enable **Mail Send → Full Access**.
4. Copy the key — it starts with `SG.`.

You also need a verified sender email address or verified domain in SendGrid before you can send. Go to **Settings → Sender Authentication** if you haven't done this.

### GitHub

1. Go to [github.com/settings/tokens](https://github.com/settings/tokens).
2. Click **Generate new token (classic)**.
3. Give it a name, set an expiration, and select the scopes you need:
   - `public_repo` — for creating issues or comments on public repos
   - `repo` — for private repos
4. Copy the token — it starts with `ghp_`. GitHub only shows it once.

### Google Sheets

Google Sheets uses OAuth 2.0, which is more involved than a simple API key. You need a Google Cloud service account.

1. Go to [console.cloud.google.com](https://console.cloud.google.com) and create a project (or use an existing one).
2. Go to **APIs & Services → Enable APIs** and enable the **Google Sheets API**.
3. Go to **APIs & Services → Credentials → Create Credentials → Service Account**.
4. Give the service account a name, click through to finish creating it.
5. Click the service account you just created, go to **Keys → Add Key → Create new key → JSON**.
6. A JSON file downloads. Open it and copy the value of the `"private_key"` field (the long block starting with `-----BEGIN RSA PRIVATE KEY-----`). Alternatively, use the service account's email address with a personal access token approach — see Google's documentation.
7. Back in the spreadsheet, click **Share** and share the sheet with the service account's email address (visible in the service account details, ends with `@your-project.iam.gserviceaccount.com`). Give it **Editor** access.

> For a simpler approach: use a tool like [oauth2l](https://github.com/google/oauth2l) or a short Python script to generate a short-lived access token, and paste that as the credential value. Access tokens expire after 1 hour, so this only works for testing.

### Notion

1. Go to [notion.so/my-integrations](https://www.notion.so/my-integrations).
2. Click **New integration**.
3. Give it a name, select the workspace, set **Capabilities** to read/update/insert content.
4. Copy the **Internal Integration Token** — it starts with `secret_`.
5. In each Notion database you want to access, click **...** (top right) → **Connections** → add your integration by name.

### Telegram

1. Open Telegram and search for **@BotFather**.
2. Send `/newbot`.
3. Follow the prompts to name your bot and get a username.
4. BotFather sends you a token — it looks like `123456789:ABCdef...`.

To find your chat ID (needed for the Telegram node's `chat_id` field): send a message to your bot, then go to `https://api.telegram.org/bot<YOUR_TOKEN>/getUpdates` in a browser. Look for `"chat":{"id":...}` in the response.

### Discord

Discord uses webhook URLs, not a bot token. No developer account needed.

1. Open your Discord server. Go to **Server Settings → Integrations → Webhooks**.
2. Click **New Webhook**.
3. Choose the channel, give it a name.
4. Click **Copy Webhook URL**.

Paste the whole URL as the credential value. It looks like `https://discord.com/api/webhooks/123456/abcdef...`.

### Stripe

1. Go to [dashboard.stripe.com/apikeys](https://dashboard.stripe.com/apikeys).
2. Use `sk_test_...` for testing and `sk_live_...` for production.
3. Never put a live key in a workflow you're still developing.

### SMTP (Send Email node)

The Send Email node uses SMTP directly — no API key, just a username and password. Common settings:

| Provider | SMTP Host | Port | Notes |
|---|---|---|---|
| Gmail | `smtp.gmail.com` | `587` | Use an App Password, not your account password. [Create one here](https://myaccount.google.com/apppasswords). Two-factor must be enabled on your Google account. |
| Outlook / Hotmail | `smtp.office365.com` | `587` | Use your full email address as the username. |
| Fastmail | `smtp.fastmail.com` | `587` | Use your full email address and a [Fastmail app password](https://www.fastmail.com/help/clients/apppassword.html). |
| Mailgun | `smtp.mailgun.org` | `587` | Find credentials in Mailgun dashboard → Sending → Domain settings → SMTP credentials. |

---

## How encryption works

Credentials are encrypted with AES-256-GCM. The encryption key is stored in the OS-native credential store on the desktop app, and in a local file in server mode.

### Desktop app (macOS, Windows, Linux)

The desktop app stores the encryption key in the OS keychain:

| Platform | Store |
|---|---|
| macOS | macOS Keychain (login keychain, service `flowo`, account `encryption_key`) |
| Windows | Windows Credential Manager (service `flowo`, account `encryption_key`) |
| Linux | SecretService via D-Bus (e.g. GNOME Keyring, KWallet) |

If the keychain is unavailable (e.g. Linux without a running SecretService daemon), the app logs a warning and falls back to a plain file — see the table in the next section for the fallback path.

**Migration from older versions:** If you upgrade from a version that used a key file (`.cred.key`), the desktop app automatically migrates the key into the OS keychain on first launch and deletes the file. No manual action is required.

### Server mode (flowo-server)

The server uses a plain file by default (servers rarely have an OS keychain):

| Platform | Key file location |
|---|---|
| macOS | `<data_dir>/flowo.key` |
| Windows | `<data_dir>\flowo.key` |
| Linux | `<data_dir>/flowo.key` |

`<data_dir>` defaults to `~/.flowo-server` and can be changed with `--data-dir`.

On Unix (macOS and Linux), the key file is created with `chmod 600` — owner-readable only.

To opt in to OS keychain storage on the server (useful when running on a desktop-adjacent machine with a running keychain daemon), pass the `--keychain` flag:

```
flowo-server api --keychain --token mytoken
```

If the keychain is unavailable, the server falls back to the file automatically with a warning in the log.

**Back up your key.** Regardless of where the key is stored — keychain or file — losing it means stored credentials cannot be recovered. The `credentials.db` file is useless without its key.

---

## Delete a credential

Open the Connections panel, click the trash icon next to the credential. Flowo checks whether any saved workflow references that credential ID before deleting. If it does, the delete is blocked and shows which workflows would break.

---

## Rotating a credential

There's no in-place rotation UI. The workflow is:

1. Add the new credential with a new ID (e.g. `openai-prod-2`).
2. Open each workflow that uses the old credential, update the affected nodes to use the new ID.
3. Save each workflow.
4. Delete the old credential.

---

## Credentials in server mode

When you export a workflow for server deployment, credentials are not bundled into the zip. Instead, `flowo-server` reads them from environment variables at runtime.

The export panel shows you exactly which environment variables to set. The naming convention is automatic:

```
credential ID "openai-prod"   → FLOWO_CRED_OPENAI_PROD
credential ID "slack-bot"     → FLOWO_CRED_SLACK_BOT
credential ID "my.key"        → FLOWO_CRED_MY_KEY
```

Set these in the `.env` file before running `install.sh`, or in your systemd unit file. See [Server Deployment](server-deploy.md) for the full workflow.

In API mode, save credentials directly to the server using the `/api/credentials` endpoint — they're encrypted the same way as the desktop app.

---

## Security notes

For the complete security model — including the key file threat model, SSRF protection, dangerous node confirmations, server hardening, and prompt injection — see [Security](security.md).

- Credential values are never sent to the frontend. The TypeScript layer only ever sees credential IDs.
- The Connections panel lists credential names and IDs only — there's no way to read back a stored value through the UI.
- In the desktop app, you're prompted to confirm before running any workflow that contains Shell Command or Code (JS) nodes. These nodes can access credentials indirectly through `{{$env.VAR_NAME}}` — but `$env` is disabled in the desktop app, so this path is closed.
- In server mode, `$env` is opt-in per variable. Never add `FLOWO_TOKEN` or `FLOWO_CRED_*` variables to the `--allow-env-vars` list — doing so lets any workflow read and exfiltrate them.
