# Credentials

Most automation workflows call external services — OpenAI, Slack, Stripe, Google Sheets. Those services require a key to prove you're authorized to use them. Aerini's credential store is where you keep those keys.

The reason this exists as a separate system (rather than just pasting keys directly into node config fields) is security. Credentials are encrypted on disk. They're never written to your workflow files. If you share a workflow export with a colleague, your API keys don't come along for the ride.

---

## Add a credential

1. Click the **Connections** button in the sidebar (the lock icon).
2. Click **Add Credential**.
3. **ID** — a short slug used to reference this credential in nodes. Use lowercase letters, numbers, and hyphens. No spaces. Example: `openai-prod`, `slack-bot`, `stripe-live`.
4. **Name** — the label shown in dropdowns when you configure nodes. Example: `OpenAI Production Key`, `Slack Bot Token`.
5. **Value** — paste the actual key here.
6. Click Save.

The value is encrypted the moment you save. After that point, Aerini never shows it in the UI again — you can only update or delete it.

---

## Use a credential in a node

In any node that needs an API key, the config panel shows a credential dropdown. Select the credential by its Name. The workflow file stores only the credential's ID — the actual key is looked up at runtime and never leaves the encrypted store.

---

## Getting API keys for supported services

### OpenAI

1. Go to [platform.openai.com/api-keys](https://platform.openai.com/api-keys).
2. Click **Create new secret key**. Give it a name, click Create.
3. Copy the key immediately — it starts with `sk-`. OpenAI shows it only once.

Suggested credential ID: `openai`

### Anthropic (Claude)

1. Go to [console.anthropic.com/settings/keys](https://console.anthropic.com/settings/keys).
2. Click **Create Key**. Copy the key — it starts with `sk-ant-`.

Suggested credential ID: `anthropic`

### Slack

1. Go to [api.slack.com/apps](https://api.slack.com/apps) and create a new app (or open an existing one).
2. Navigate to **OAuth & Permissions → Bot Token Scopes** and add `chat:write`.
3. Click **Install to Workspace** at the top of that same page.
4. Copy the **Bot User OAuth Token** — it starts with `xoxb-`.

Before the bot can post to a channel, you need to invite it: open the channel in Slack, type `/invite @YourBotName`, and send.

Suggested credential ID: `slack-bot`

### SendGrid

1. Go to [app.sendgrid.com/settings/api_keys](https://app.sendgrid.com/settings/api_keys).
2. Click **Create API Key**. Choose **Restricted Access** and enable **Mail Send → Full Access**.
3. Copy the key — it starts with `SG.`.

You also need a verified sender address or domain before you can send mail. Check **Settings → Sender Authentication** in SendGrid if you haven't done this.

Suggested credential ID: `sendgrid`

### GitHub

1. Go to [github.com/settings/tokens](https://github.com/settings/tokens).
2. Click **Generate new token (classic)**.
3. Name it, set an expiry, and select scopes:
   - `public_repo` — for issues and comments on public repos
   - `repo` — for private repos
4. Copy the token — it starts with `ghp_`. GitHub shows it only once.

Suggested credential ID: `github`

### Google Sheets

Google Sheets uses a service account rather than a simple API key.

1. Go to [console.cloud.google.com](https://console.cloud.google.com) and create or select a project.
2. Go to **APIs & Services → Enable APIs** and enable the **Google Sheets API**.
3. Go to **APIs & Services → Credentials → Create Credentials → Service Account**. Give it a name and finish creating it.
4. Open the service account, go to **Keys → Add Key → Create new key → JSON**. A JSON file downloads.
5. Open that JSON file in a text editor and copy the entire value of the `"private_key"` field — the long block starting with `-----BEGIN RSA PRIVATE KEY-----`.
6. Share each spreadsheet you want to access with the service account's email address (ends with `@your-project.iam.gserviceaccount.com`). Give it Editor access.

Paste the private key as the credential value.

Suggested credential ID: `google-sheets`

### Notion

1. Go to [notion.so/my-integrations](https://www.notion.so/my-integrations).
2. Click **New integration**, name it, and set **Capabilities** to allow reading, inserting, and updating content.
3. Copy the **Internal Integration Token** — it starts with `secret_`.
4. For each Notion database you want to access: open the database, click **...** (top right) → **Connections**, and add your integration by name.

Suggested credential ID: `notion`

### Telegram

1. Open Telegram and search for **@BotFather**.
2. Send `/newbot` and follow the prompts to name your bot.
3. BotFather sends you a token — it looks like `123456789:ABCdef...`.

To find your chat ID (needed for the `chat_id` field in the Telegram node): send a message to your bot, then visit `https://api.telegram.org/bot<YOUR_TOKEN>/getUpdates` in a browser. Look for `"chat":{"id":...}` in the response.

Suggested credential ID: `telegram`

### Discord

Discord uses webhook URLs rather than a bot token. No developer account required.

1. Open your server. Go to **Server Settings → Integrations → Webhooks**.
2. Click **New Webhook**, pick a channel, give it a name.
3. Click **Copy Webhook URL** — it starts with `https://discord.com/api/webhooks/...`.

Paste the entire URL as the credential value.

Suggested credential ID: `discord`

### Stripe

1. Go to [dashboard.stripe.com/apikeys](https://dashboard.stripe.com/apikeys).
2. Use `sk_test_...` for testing, `sk_live_...` for production.

Use a test key while you're developing a workflow. Switch to the live key only after you've confirmed everything works correctly.

Suggested credential ID: `stripe-test` and `stripe-live` (kept separate to avoid mistakes)

### SMTP (Send Email node)

The Send Email node connects via SMTP rather than using an API key. You provide a hostname, port, username, and password. Common setups:

| Provider | SMTP Host | Port | Notes |
|---|---|---|---|
| Gmail | `smtp.gmail.com` | `587` | Use an [App Password](https://myaccount.google.com/apppasswords), not your normal password. Requires two-factor auth enabled on your Google account. |
| Outlook / Hotmail | `smtp.office365.com` | `587` | Use your full email address as the username. |
| Fastmail | `smtp.fastmail.com` | `587` | Use a [Fastmail app password](https://www.fastmail.com/help/clients/apppassword.html). |
| Mailgun | `smtp.mailgun.org` | `587` | Find SMTP credentials under Sending → Domain settings → SMTP credentials in the Mailgun dashboard. |

---

## How encryption works

Credentials are encrypted with **AES-256-GCM** — the same standard used by banks and messaging apps to protect data at rest.

When you save a credential, Aerini generates a random nonce and encrypts the value with your 256-bit key. The encrypted bytes and the nonce are stored together in `credentials.db`. The key itself is never stored in that file.

When a node runs and needs a credential, Aerini decrypts it in memory for that single operation. The plaintext key is never written to disk, never logged, and never sent to the UI — the frontend only ever sees the credential's ID (like `slack-bot`), not the value.

**Where the key lives:**

| Platform | Key store |
|---|---|
| macOS | macOS Keychain |
| Windows | Windows Credential Manager |
| Linux | SecretService via D-Bus (GNOME Keyring or KWallet) |

On Linux, if no SecretService daemon is running, Aerini falls back to a plain file at `.cred.key` in the app data directory (created with `600` permissions, readable only by your user). If you see a warning about this at startup, install a keychain daemon:

```bash
# GNOME
sudo apt install gnome-keyring

# KDE
sudo apt install kwallet-pam
```

Once a keychain becomes available, Aerini migrates the key into it automatically and deletes the file.

**In server mode:** the key is stored in a file by default (`~/.aerini-server/aerini.key`). See [Security](security.md#3-the-encryption-key--your-most-important-file) for the full picture, including how to use the OS keychain on a server and what you need to know about backup safety.

---

## Delete a credential

Open the Connections panel and click the trash icon next to the credential. Aerini checks whether any workflow currently references that credential ID before deleting. If one does, the delete is blocked and shows you which workflows would break. Update those workflows first, then delete.

---

## Rotate a credential (change to a new key)

There's no in-place rotation UI. The process:

1. Create a new credential with a new ID (e.g. `openai-v2`).
2. Open each workflow using the old credential and update the affected nodes to use the new ID.
3. Save each workflow.
4. Delete the old credential.

---

## In server deployments

When you export a workflow for server deployment, credentials are **not included** in the export zip. Instead, `aerini-server` reads credentials from environment variables at runtime.

The export panel shows exactly which environment variables to set. The naming is automatic:

| Credential ID | Environment variable |
|---|---|
| `openai-prod` | `AERINI_CRED_OPENAI_PROD` |
| `slack-bot` | `AERINI_CRED_SLACK_BOT` |
| `my.key` | `AERINI_CRED_MY_KEY` |

Hyphens and dots become underscores, the name is uppercased, and `AERINI_CRED_` is prepended. Set these variables in `/etc/aerini/.env` (or your deployment's equivalent) before starting the server.

Full server setup walkthrough: [Server Deployment](server-deploy.md)

---

## Where the encryption key is stored

### Desktop app

| Platform | Key store |
|---|---|
| macOS | macOS Keychain (login keychain, service `aerini`, account `encryption_key`) |
| Windows | Windows Credential Manager |
| Linux | SecretService via D-Bus (GNOME Keyring, KWallet, or equivalent) |

If the keychain is unavailable (common on Linux without a running SecretService daemon), the app falls back to a plain file at `.cred.key` in the app data directory with `chmod 600`. Once a keychain becomes available, Aerini migrates the key into it automatically on next launch and deletes the file.

### Server mode

The server stores the key in a plain file by default — `<data_dir>/aerini.key` (default: `~/.aerini-server/aerini.key`), created with `chmod 600`.

`chmod 600` protects against other OS users. It does not protect against root, against a process running as the same user, or against a backup that contains both `aerini.key` and `credentials.db` together. To use the OS keychain instead:

```bash
aerini-server api --keychain --token mytoken
```

See [Security — encryption key](security.md#3-the-encryption-key--your-most-important-file) for the full picture.

---

## Back up the encryption key

The key described above is the only thing that can decrypt anything in `credentials.db`. If it's lost — a keychain reset, a fresh OS install, migrating to a new machine — every credential in every workflow becomes permanently unreadable. There is no password reset for this; back it up before you need it, not after.

**Desktop app:**

1. Open the **Connections** panel (the lock icon in the sidebar).
2. Click **Backup Encryption Key** and confirm the warning.
3. Copy the value shown and store it somewhere as secure as the credentials themselves — a password manager entry, not a plain text file next to your workflows.

**To restore** (only needed if the OS keychain entry is lost and no fallback file survived):

1. Quit Aerini.
2. Write the backed-up value, exactly as copied, to `.cred.key` in the app data directory for your platform:

   | Platform | Path |
   |---|---|
   | macOS | `~/Library/Application Support/com.aerini.app/.cred.key` |
   | Windows | `%APPDATA%\com.aerini.app\.cred.key` |
   | Linux | `~/.local/share/com.aerini.app/.cred.key` |

3. Relaunch Aerini. The existing keychain-migration behavior (see above) picks up the file automatically and moves it back into the OS keychain — no separate "restore" button or import step.

**In server mode**, the key already lives in a plain file you control (`<data_dir>/aerini.key`) — back that file up directly (see [Security](security.md#3-the-encryption-key--your-most-important-file)); there's no separate export step for `aerini-server`.
