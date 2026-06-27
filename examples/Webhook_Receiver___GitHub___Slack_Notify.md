# Example — Webhook Receiver (Webhook → If → Slack Notify)

Accepts a GitHub webhook, checks the event type, and posts to Slack on push events only.

## Nodes used

| Node | Purpose |
|------|---------|
| Webhook | Receive incoming POST from GitHub |
| If / Condition | Check `{{Webhook.body.ref}}` starts with `refs/heads/main` |
| Slack | Post a message to a channel |
| Stop | Silently end the run for non-matching events |

## Expressions

- Condition: `{{Webhook.body.ref}} == "refs/heads/main"`
- Slack message: `"Push to main by {{Webhook.body.pusher.name}}: {{Webhook.body.head_commit.message}}"`

## How to build it

1. Add a **Webhook** node. Copy the generated URL — paste it into GitHub repo Settings → Webhooks.
   Set content type `application/json`. Select event: **Push**.
2. Add an **If / Condition** node. Condition: `{{Webhook.body.ref}} == "refs/heads/main"`.
3. Connect the **true** branch to a **Slack** node. Set channel and message using expressions above.
4. Connect the **false** branch to a **Stop** node.
5. Connect: Webhook → If → (true) Slack / (false) Stop.

## Credentials needed

- Slack Bot Token (Settings → Credentials → Add → Slack)

## Notes

- The Webhook node's **Secret** field checks the value you set here against a shared-secret header Aerini expects on incoming requests — it does **not** implement GitHub's own HMAC-SHA256 signature scheme (`X-Hub-Signature-256`). GitHub's "Secret" setting on the webhook itself only produces that HMAC signature; it does not send the secret as a plain header, so the two can't be wired together directly. Configuring the same value in both places does not make Aerini verify GitHub's signature.
  For real protection on GitHub (or any HMAC-signing) webhooks, verify `X-Hub-Signature-256` in a downstream **Code** node — see `docs/security.md` §6 for the pattern and exact header details.
- To receive webhooks on a local machine, use cloudflared or ngrok to expose the port.
  See `docs/webhooks-public.md`.
