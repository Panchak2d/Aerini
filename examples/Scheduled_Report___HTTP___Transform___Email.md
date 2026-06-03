# Example — Scheduled Report (HTTP → Transform → Email)

Fetches data from an API every morning, transforms it, and emails the result.

## Nodes used

| Node | Purpose |
|------|---------|
| Schedule | Trigger at 08:00 every weekday |
| HTTP Request | `GET https://api.example.com/stats` |
| Transform Data | Extract `data.summary` from the response |
| Send Email | Send the summary to a recipient |

## Expressions

Wire `{{HTTP Request.body.data.summary}}` into the Transform node's input.
Wire `{{Transform Data.result}}` into the email body.

## How to build it

1. Add a **Schedule** node. Set cron to `0 8 * * 1-5`.
2. Add an **HTTP Request** node. Set method `GET` and your API URL. Add any auth headers under Headers.
3. Add a **Transform Data** node. Set expression to `input.data.summary`.
4. Add a **Send Email** node. Fill in SMTP credentials under Credentials. Set body to `{{Transform Data.result}}`.
5. Connect: Schedule → HTTP Request → Transform Data → Send Email.
6. Run manually once to verify output, then enable the schedule.

## Credentials needed

- SMTP credentials for Send Email (Settings → Credentials → Add → SMTP)
