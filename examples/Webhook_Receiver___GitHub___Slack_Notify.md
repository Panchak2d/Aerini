# Example — Webhook Receiver (Webhook → If → Slack Notify)

Accepts a GitHub webhook, checks the event type, and posts to Slack on push events only.

## Nodes used

| Node | Purpose |
|------|---------|
| Webhook | Receive incoming POST from GitHub |
| If / Condition | Check whether `{{Webhook.output.body.ref}}` equals `refs/heads/main` |
| Slack | Post a message to a channel |
| Stop | Silently end the run for non-matching events |

## Expressions

- Condition: `{{Webhook.output.body.ref}} == refs/heads/main`
- Slack text: `"Push to main by {{Webhook.output.body.pusher.name}}: {{Webhook.output.body.head_commit.message}}"`

## How to build it

1. Add a **Webhook** node. Set Path (default `/webhook`) and set Method to `POST` or `ANY`. The node doesn't generate a URL for you — build one from the port and path you set (default `http://<your-host>:3456/webhook`) and paste that into GitHub's repo Settings → Webhooks → Payload URL. Set Content type to `application/json` and select event **Push** on GitHub's own webhook settings page (these are GitHub-side settings, not fields on the Aerini node).
2. Add an **If / Condition** node. Condition: `{{Webhook.output.body.ref}} == refs/heads/main`. This is an exact match, not a "starts with" — GitHub's push payload's `ref` field is exactly `refs/heads/main` for a push to that branch, so equality is sufficient here.
3. Connect the **True** branch to a **Slack** node. Set Channel and Text using the expressions above.
4. Connect the **False** branch to a **Stop** node.
5. Connect: Webhook → If / Condition → (True) Slack / (False) Stop.

## Credentials needed

Add a credential under **Settings → Credentials → Add a credential** (any Type — it's just a label) with your Slack Bot Token as the Secret Value, then select it from the Slack node's `api_key` field via its Saved Credential dropdown.

## Notes

- The Webhook node's **Secret** field checks the value you set here against a shared-secret header Aerini expects on incoming requests — it does **not** implement GitHub's own HMAC-SHA256 signature scheme (`X-Hub-Signature-256`). GitHub's "Secret" setting on the webhook itself only produces that HMAC signature; it does not send the secret as a plain header, so the two can't be wired together directly. Configuring the same value in both places does not make Aerini verify GitHub's signature.
  For real protection on GitHub (or any HMAC-signing) webhooks, verify `X-Hub-Signature-256` in a downstream **Code** node — see the Webhook node's own field notes in [Nodes Reference](../docs/guide/nodes.md#webhook) for the pattern and exact header details.
- To receive webhooks on a local machine, the Webhook node only ever binds to `127.0.0.1` — reaching it from GitHub needs a reverse proxy or a tunneling tool like `cloudflared` or `ngrok` in front of it. See [Troubleshooting §A webhook never receives a request](../docs/troubleshooting.md#a-webhook-never-receives-a-request).
