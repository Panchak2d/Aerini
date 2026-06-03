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

- The Webhook node generates a secret token. Configure it in GitHub's webhook settings under **Secret**
  so Flowo can verify the signature. See `docs/security.md` for details.
- To receive webhooks on a local machine, use cloudflared or ngrok to expose the port.
  See `docs/webhooks-public.md`.
