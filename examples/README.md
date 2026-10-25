# Examples

This directory contains example workflows importable into Aerini.

## How to import

Aerini imports a `.aerini` or `.json` workflow file two ways:
1. Drag the file onto the canvas.
2. Or, on an empty canvas, click the import button and pick the file from the browser.

There is no separate **Import** toolbar button — the two paths above are the only import mechanism.

An imported file is always added as a new workflow, so importing never overwrites one you already have. If its name is already taken, Aerini adds a suffix: `My Workflow (2)`, then `(3)`, and so on.

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

## AI_Agent_Router.aerini

An AI Agent tool-routing workflow, importable as described in "How to import" above. Its walkthrough is `AI_Agent_Router.md`. It demonstrates:
- **AI Agent** — reads a question and either answers or asks for one of two declared tools
- **If / Condition** — rejects a request for an unknown tool or with bad arguments before anything runs
- **Switch** — routes the request on `tool_calls.0.tool`; the default branch means no tool was needed
- **HTTP Request** (x2), **Set Variable**, **Merge** — the tool branches, rejoined into one path
- **AI Agent** (second) — continues the conversation with the tool result and writes the answer

Needs an API key for your AI provider on both AI Agent nodes. Includes the safety rules for acting on a model's tool requests.

## AI_Agent_Loop.aerini

An AI Agent that keeps going until it is done, importable as described in "How to import" above. Its walkthrough is `AI_Agent_Loop.md`. It demonstrates:
- **Loop** — runs one turn per item of a list, so the list length is the turn budget; **Stop When** ends it early
- **AI Agent** — reads its own earlier `messages` and the previous turn's tool result, then answers or asks for one tool
- **If / Condition**, **Switch** — reject a bad request and route a good one to the tool's branch
- **HTTP Request** (x2), **Set Variable**, **Merge** — the tool branches and the result handed back for the next turn

Needs an API key for your AI provider on the AI Agent. The pattern was tested on Gemini. States what bounds the loop (the list, Stop When, Max Iterations) and what does not (no token budget or time cap), and includes the safety rules for acting on a model's tool requests.

## Chatbot___AI_Memory___Widget.md

A chatbot workflow template demonstrating:
- **Webhook** — receives POST requests relayed from the embedded chat widget
- **AI Memory (read)** — retrieves stored conversation history for the session
- **AI Prompt** — generates a response with the retrieved history folded into the prompt
- **AI Memory (append user)** / **AI Memory (append assistant)** — persists both sides of the new exchange, one `append` call per turn
- **Output** — returns the reply text to the widget

Includes widget embed instructions (the real `data-server`/`data-workflow-id`/`data-secret`/`data-token` attributes) and security notes distinguishing the send-credential from the reply-read token.

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
- **HTTP Request** (x2, distinctly named) — fetches the current top Hacker News story
- **AI Prompt** — summarizes it in 3 bullet points
- **Notion** — writes the summary into an existing database property (title + a rich-text column)

Includes the exact cron expression, chained-request wiring with disambiguated node names, and required credentials.

## Scheduled_Report___HTTP___Transform___Email.md

A recurring report workflow demonstrating:
- **Schedule** — triggers weekday mornings
- **HTTP Request** — fetches data from an API
- **Transform Data** — extracts one field via a Source Node + JSON-Pointer mapping
- **Send Email** — emails the result

Includes the cron expression, the Transform Data mapping, and SMTP credential setup.
