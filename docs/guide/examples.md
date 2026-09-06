# Examples

Five worked examples, each a plain-language walkthrough of a complete workflow: what it does, which nodes it uses, and how to wire them together. None of these are one-click imports. For four of the five, building one means opening a new workflow in Aerini and following its steps by hand on the canvas; the fifth ships differently, noted below. [The examples folder's own README](https://github.com/Panchak2d/Aerini/blob/main/examples/README.md) covers the general mechanics of importing a workflow file, for the cases where you do have one.

This page assumes you've read [Nodes Reference](nodes.md) and [Expressions](expressions.md), and it doesn't repeat their field-by-field detail. If an example uses a scheduled or webhook trigger, [Background Runs](background-runs.md) covers what happens once it's running unattended. If it needs an API key, [Credentials](credentials.md) covers where that goes.

## Getting Started: Fetch and Show

The simplest workflow of the five: a **Schedule** trigger fires every 10 seconds, an **HTTP Request** node calls a public API, and two **Output** nodes display the result. A good first build, just to confirm a fresh install works before trying anything real.

This one ships differently from the rest, as a self-hosted Docker deployment package rather than a Markdown walkthrough. [See the Examples README](https://github.com/Panchak2d/Aerini/blob/main/examples/README.md) for how to unzip and run it, and to recreate the workflow itself on the canvas.

## Scheduled Report

Fetches data from an API every weekday morning, extracts one field from the response, and emails the result. Uses a **Schedule** trigger (cron), **HTTP Request**, **Transform Data**, and **Send Email**.

[Read the full walkthrough](https://github.com/Panchak2d/Aerini/blob/main/examples/Scheduled_Report___HTTP___Transform___Email.md).

## Webhook Receiver

Accepts an incoming GitHub webhook, checks whether it's a push to `main`, and posts to Slack only when it matches. Uses a **Webhook** trigger, **If / Condition**, **Slack**, and **Stop** for everything else.

Includes a note on why the Webhook node's Secret field alone doesn't verify GitHub's own signature, and what to add if that matters for your setup.

[Read the full walkthrough](https://github.com/Panchak2d/Aerini/blob/main/examples/Webhook_Receiver___GitHub___Slack_Notify.md).

## AI Pipeline

Runs once a day: fetches the current top Hacker News story, summarizes it with an AI model, and creates a page in Notion with the result. Uses a **Schedule** trigger (cron), two chained **HTTP Request** nodes, **AI Prompt**, and **Notion**.

[Read the full walkthrough](https://github.com/Panchak2d/Aerini/blob/main/examples/AI_Pipeline___Schedule___Prompt___Notion.md).

## Chatbot with AI Memory and Widget

A persistent chatbot: each message from an embedded chat widget gets a reply from an AI model, and the exchange is saved so the next message continues the same conversation instead of starting over. Uses **AI Memory** to read history before the model runs and write it back after, with an **AI Prompt** node in between.

The longest of the five. Covers embedding the widget script on a web page, plus security notes on token exposure and prompt injection worth reading before you expose one publicly.

[Read the full walkthrough](https://github.com/Panchak2d/Aerini/blob/main/examples/Chatbot___AI_Memory___Widget.md).

## See also

- [Nodes Reference](nodes.md), for every field these examples reference
- [Expressions](expressions.md), for the `{{...}}` syntax wiring node outputs together
- [Credentials](credentials.md), for the API keys most of these need
- [Background Runs](background-runs.md), for what happens once a Schedule or Webhook example is running unattended
