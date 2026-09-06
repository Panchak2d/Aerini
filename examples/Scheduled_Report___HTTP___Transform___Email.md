# Example — Scheduled Report (HTTP → Transform → Email)

Fetches data from an API every morning, extracts one field from the response, and emails it.

## Nodes used

| Node | Purpose |
|------|---------|
| Schedule | Trigger at 08:00 every weekday |
| HTTP Request | `GET https://api.example.com/stats` |
| Transform Data | Extract `body.data.summary` from the HTTP Request's output |
| Send Email | Send the summary to a recipient |

## Expressions

Transform Data doesn't take a wired expression as its input — it pulls directly from a specific node's output via its own **Source Node** setting, using a JSON Pointer rather than the `{{...}}` syntax. There's no `expression` field on this node.

Wire `{{Transform Data.output.summary}}` into the Send Email node's Body field.

## How to build it

1. Add a **Schedule** node. Set cron to `0 8 * * 1-5`.
2. Add an **HTTP Request** node. Set Method to `GET` and URL to your API endpoint. Add any auth headers under Headers.
3. Add a **Transform Data** node.
   - **Source Node**: select the HTTP Request node from the dropdown. This is required — without it, Transform Data has no single node's shape for its mappings to resolve against.
   - **Mappings**: `[{ "from": "/body/data/summary", "to": "summary" }]`. `from` is a JSON Pointer into the HTTP Request's output (which is `{status, body, headers}`, so the API response itself sits under `/body`); `to` is the key the extracted value lands on in Transform Data's own output.
4. Add a **Send Email** node.
   - **SMTP Host / SMTP Port / From / To / Subject / Username**: typed directly into the node.
   - **Password**: resolved from a saved credential (see below).
   - **Body**: `{{Transform Data.output.summary}}`
5. Connect: Schedule → HTTP Request → Transform Data → Send Email. The wire controls execution order; the actual data pull for Transform Data is its Source Node setting from step 3, not the wire itself.
6. Run manually once to verify output, then enable the schedule.

## Credentials needed

Add a credential under **Settings → Credentials → Add a credential** (any Type — the type is just a label for your own reference) with your SMTP account's password as the Secret Value, then select it from the Send Email node's **Password** field via its Saved Credential dropdown. `smtp_host`, `from`, `to`, `subject`, and `username` are plain fields typed directly into the node — only `password` gets the credential picker.
