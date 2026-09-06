# Aerini Documentation

Aerini is a visual workflow automation app. You drag boxes called **nodes** onto a canvas, connect them in order, and Aerini runs them: by hand, on a schedule, or when a web request comes in. There's no cloud account and no subscription, and nothing about your workflows leaves your machine unless a node you add is the one doing it (an email node sending an email, an HTTP node calling an API you chose).

If you've used Zapier, Make, or n8n, the shape will feel familiar. The difference is where it runs: on your own computer or your own server, not a company's cloud.

## The problem it solves

Most small, repetitive computer tasks ("check this every morning," "when X happens, do Y," "pull data from here and put it there") either get done by hand forever or need a developer to write and maintain a script for them. Aerini sits in between. You build the automation visually, without writing code, but it's still a real running program on infrastructure you control, not a black box in someone else's cloud.

## Who it's for

- **Non-technical automation builder.** You've never opened a terminal and don't plan to. You install Aerini, drag some nodes onto the canvas, and get a working automation without touching code. This is the primary audience, and everything in [Getting Started](getting-started/installation.md) is written for you first.
- **Self-hoster / ops person.** You're comfortable with a terminal and want a workflow running 24/7 on a Linux box or in Docker, without the desktop app open. You care about flags, tokens, and hardening. See [Server Deployment](operations/server-deploy.md).
- **Plugin / integration developer.** You want to add a node type Aerini doesn't ship with, either as a `.wasm` plugin or by embedding Aerini's engine directly inside your own Rust program. See [Plugin Authoring](development/plugin-authoring.md) or [Embedding aerini-engine](development/embedding.md).
- **Aerini contributor.** You want to build Aerini itself from source, add a new built-in node, or touch the codebase directly. See [Architecture](development/architecture.md) and the repository's `CONTRIBUTING.md`.

You don't need to know which of these you are before you start. Everyone begins in the same place: [Installation](getting-started/installation.md).

## What you can build

A workflow is nodes connected in an order, so anything you can break into "when this happens, do these steps" fits. In practice, people use Aerini for things like:

- A morning report: fetch numbers from an API, reshape them, and email or message the result.
- An alert: check something on a timer, and only notify someone when it looks wrong.
- A webhook receiver: accept a notification from GitHub or Stripe, and act on it.
- Small AI pipelines: prompt a model, transform its answer, and hand the result to another node.
- File and data shuffling: read files, change their contents, write the results somewhere else.

[Nodes Reference](guide/nodes.md) lists every building block available. Skim it once you have a sense of what a node is (see [Concepts](getting-started/concepts.md)), rather than reading it end to end now.

## What Aerini can't do

The edges matter as much as the feature list.

- **No hosted service.** There's no Aerini cloud to sign up for. Something, your desktop or a server you run, has to be on for a scheduled or webhook-triggered workflow to fire.
- **Single machine by design.** A workflow runs wherever you started it. There's no built-in way to sync the same workflow across two machines or hand a run off between them.
- **Doesn't expose your machine to the internet on its own.** A Webhook node listens on a local port. If you want the outside world to reach it, you're responsible for exposing that port yourself, with a reverse proxy or a tunneling tool like `cloudflared` or `ngrok`.
- **Pre-1.0.** Aerini is currently version 0.4.0. The workflow file format and internal APIs are expected to keep changing until 1.0, when they'll be locked down under semantic versioning.
- **Not a general data platform.** No data warehouse, no built-in analytics, no massive-scale batch processing. It's an automation tool for individuals and small setups, not enterprise ETL infrastructure.

> [!NOTE]
> Aerini collects no telemetry, analytics, or crash reports of any kind. The one exception: the app's own interface loads a font (Inter) from Google Fonts over HTTPS. Everything else is under your control. See [Security](guide/security.md) for the full picture.

## Two documents both called "Security"

There are two, and they cover different things.

- **This site's [Security](guide/security.md)** explains Aerini's security *model*: how credentials are encrypted, what sandboxing plugins get, how server-side request forgery (SSRF) protection works, and how to harden a server deployment.
- **The repository's root-level `SECURITY.md`** is a *vulnerability reporting process*: where to report a security issue you've found, not a description of how the product works.

If you're deciding whether it's safe to expose a Webhook node to the internet, you want the first one.

## Where to go next

| If you want to... | Go to |
|---|---|
| Install Aerini | [Installation](getting-started/installation.md) |
| Build and run your first workflow | [Getting Started](getting-started/getting-started.md) |
| Understand the mental model before diving in | [Concepts](getting-started/concepts.md) |
| Look up a term you don't recognize | [Glossary](glossary.md) |
| See every built-in node | [Nodes Reference](guide/nodes.md) |
| Wire node outputs into other nodes | [Expressions](guide/expressions.md) |
| Store an API key or set up OAuth | [Credentials](guide/credentials.md) |
| Run workflows on a schedule or via webhook | [Background Runs](guide/background-runs.md) |
| See a worked example end to end | [Examples](guide/examples.md) |
| Fix a stuck or failed run | [Troubleshooting](troubleshooting.md) |
| Run Aerini unattended on a server | [Server Deployment](operations/server-deploy.md) |
| Update the desktop app, a server deployment, or an old workflow file | [Updating](operations/updating.md) |
| Understand how the pieces fit together internally | [Architecture](development/architecture.md) |

Not sure where to start? [Installation](getting-started/installation.md) is the first page in the reading order everything else assumes.

## Project status

Aerini is open source (AGPL-3.0), currently at version 0.4.0, pre-1.0, and actively developed. See the repository's `README.md` for license and contribution details.
