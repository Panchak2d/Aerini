# Examples

This directory contains example workflows importable into Aerini.

## How to import

Aerini imports a `.aerini` or `.json` workflow file two ways:
1. Drag the file onto the canvas.
2. Or, on an empty canvas, click the import button and pick the file from the browser.

There is no separate **Import** toolbar button — the two paths above are the only import mechanism.

## Getting_Started___Fetch___Show.zip

A self-hosted Docker deployment package for the beginner workflow below — not an in-app-importable
file. Unzip it and follow its own `README.md` (`cp .env.example .env && docker compose up --build -d`).
To edit the workflow itself in Aerini, recreate it from the description below; the app has no
project-level import for a package like this yet (only single `.aerini`/`.json` workflow files, per
"How to import" above).

The workflow:
- **Schedule** — triggers every 10 seconds
- **Fetch Data** — HTTP node calling a public API
- **Show Result** — Output node displaying the result
- **Output** — a second output node re-displaying the same result

## Chatbot___AI_Memory___Widget.md

A chatbot workflow template demonstrating:
- **Manual Trigger** — receives POST requests from the embedded chat widget
- **AI Memory (read)** — retrieves stored conversation history for the session
- **AI Prompt** — generates a response with full conversation context
- **AI Memory (write)** — persists the new exchange to history
- **Output** — returns the response to the widget

Includes widget embed instructions and security notes on token exposure and prompt injection.

## Webhook_Receiver___GitHub___Slack_Notify.md

A webhook-triggered workflow demonstrating:
- **Webhook** — receives an incoming GitHub webhook POST
- **If / Condition** — checks the event is a push to `main`
- **Slack** — posts a notification on match
- **Stop** — silently ends the run on non-matching events

Includes setup steps for the GitHub webhook, the exact expressions used, and a note on why the
Webhook node's shared-secret field does not by itself verify GitHub's HMAC signature.

## AI_Pipeline___Schedule___Prompt___Notion.md

A daily AI content pipeline demonstrating:
- **Schedule** — triggers once a day
- **HTTP Request** (x2) — fetches the current top Hacker News story
- **AI Prompt** — summarizes it in 3 bullet points
- **Notion** — creates a page with the summary

Includes the exact cron expression, chained-request wiring, and required credentials.

## Scheduled_Report___HTTP___Transform___Email.md

A recurring report workflow demonstrating:
- **Schedule** — triggers weekday mornings
- **HTTP Request** — fetches data from an API
- **Transform Data** — extracts the relevant field
- **Send Email** — emails the result

Includes the cron expression, transform expression, and SMTP credential setup.
