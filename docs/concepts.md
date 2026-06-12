# Concepts

This page explains the core ideas behind Flowo in plain language. If you're new to automation tools, read this before anything else — it'll make the rest of the documentation much easier to follow.

---

## Workflow

A workflow is a set of steps that run in a specific order. Each step does one thing, then passes its result to the next step.

A recipe is a useful mental model: "fetch data from this API, transform it, then send the result to Slack." A workflow is that exact sequence, laid out visually as connected boxes on a screen.

In Flowo, you build workflows on a canvas. The boxes are called **nodes**; the lines connecting them are **edges**.

---

## Node

A node is a single step. Every box you see on the canvas is a node, and each one does exactly one thing:

- **Schedule** — fires a timing signal (e.g. "every morning at 9 AM")
- **HTTP Request** — fetches data from a URL
- **If / Condition** — checks a value and routes execution one of two ways
- **Slack** — posts a message to a Slack channel
- **Code (JS)** — runs a short JavaScript snippet you write
- **AI Prompt** — sends a prompt to an AI model and returns the response

Each node has an **input** (data coming in from the left) and an **output** (data going out to the right). The output of one node becomes the input of the next.

---

## Canvas

The canvas is the main workspace — the large area where you place and connect nodes. Think of it like a whiteboard where you draw your automation.

**Ports** are the small circles on each node's edges:
- Left side = input port (receives data)
- Right side = output port (sends data)

To connect two nodes, drag from one node's output port to another node's input port. A line appears. To remove it, drag the line off the port onto empty canvas space.

Some nodes have more than one output port. The **If / Condition** node, for example, has a "True" port and a "False" port — execution follows one path or the other based on whether the condition passes. Every node also has an `on_error` port that you can wire to a notification or Stop node if you want the workflow to handle failures gracefully rather than just stopping.

---

## Trigger

Every workflow needs a starting point. The first node is always a **trigger** — it determines when and why the workflow runs.

There are three types:

- **Manual Trigger** — runs when you press the Run button (or click Run from the API). Good for testing and one-off jobs.
- **Schedule** — runs on a timer. You can set an interval (every hour), a cron expression (weekdays at 9 AM), or a one-time date.
- **Webhook** — runs when an HTTP request arrives at a specific URL. Useful for responding to events from other services — a new Stripe payment, a GitHub push, and so on.

A workflow with only a Manual Trigger can't run automatically in the background. You need a Schedule or Webhook trigger for that.

---

## Run

A run is one complete execution of a workflow, from the trigger firing to the last node finishing.

Every run is recorded. After it finishes, you can see:
- Which nodes ran and how long each took
- The exact data each node produced
- Any errors, with full error messages
- A chronological log of every step

If a run fails partway through, the workflow stops at the failed node (unless you've wired the `on_error` port to handle it). The failed node turns red on the canvas.

---

## Expression

An expression is how you pass data from one node to another.

Say an HTTP Request node fetches some user data from an API. The JSON response includes `{ "name": "Alice", "email": "alice@example.com" }`. You want the next node — a Slack message — to say "New user: Alice." You write `New user: {{http.output.body.name}}` in the Slack message field, and Flowo substitutes the real value when the workflow runs.

The `{{...}}` syntax is an expression. You're telling Flowo: "go look up this value from a previous node's output and put it here."

You don't need to type paths from memory. Press `{{` inside any text field to open the expression picker, which shows every value available from upstream nodes. Click to insert.

Full syntax reference: [Expressions](expressions.md)

---

## Credential

A credential is a stored API key or password.

Most external services require a key to identify you: OpenAI, Stripe, Slack, GitHub, etc. Instead of pasting that key directly into a node's configuration — where anyone who opens the workflow file could see it — you store it once in Flowo's credential store. Flowo encrypts it on disk.

When you configure a node that needs a key, you pick the credential by name from a dropdown. The actual value never appears in the UI again and is never written to the workflow file.

More on adding credentials and finding API keys: [Credentials](credentials.md)

---

## Background Run

A background run is a workflow that keeps executing on its own while you use Flowo for something else, or while it's minimized to the system tray.

Background runs require a Schedule or Webhook trigger. Once started, a scheduler daemon takes over: it sleeps until the next scheduled time, runs the workflow, records the result, and goes back to sleep.

Background runs stop when you close Flowo. If you need a workflow running 24 hours a day, seven days a week, without the desktop app open, see [Server Deployment](server-deploy.md).

---

## flowo-server

`flowo-server` is a command-line tool (included with Flowo) that runs workflows on a Linux server — no desktop app required on the server.

Two modes:
- **Serve mode** — runs one exported workflow. The desktop app's "Export for Server" button produces everything you need.
- **API mode** — manages many workflows on one server via a REST API.

You don't need `flowo-server` for everyday use on your own machine. It only comes up when you want workflows running on a remote server.

---

## Output Drawer

The output drawer is the panel that slides up from the bottom after a run. It shows everything that happened: what each node produced, any errors, and a log of every step in order.

You can open it at any time and use the History tab to browse past runs.

---

## Version History

Every time you save a workflow, Flowo creates a snapshot. Right-click the workflow in the sidebar → **Versions** to browse and restore any previous version.

Version history tracks what the workflow *looked like*. Run history tracks what the workflow *did* when it executed. These are separate.

---

## Common questions

**What's the difference between a Schedule and a Webhook trigger?**

Schedule runs on a timer you control — every hour, daily at 9 AM, etc. Webhook runs in response to an event from another service — for example, when someone submits a payment through Stripe. Use Schedule for time-based tasks; use Webhook for event-driven reactions.

**Can I have multiple triggers in one workflow?**

No. Each workflow has exactly one trigger node. If you need the same logic to run on two different triggers, create two workflows and put the shared logic in each one.

**What happens when a node fails?**

If a node fails and you haven't wired its `on_error` port, the workflow stops and records the failure. If you have wired `on_error`, execution follows that branch — you can route it to a Slack message, a Stop node with a custom reason, or anything else.

**Does Flowo send my data anywhere?**

No. Everything stays on your machine: workflow definitions, credentials, run history, node outputs. The only outbound network traffic is what your workflow nodes explicitly make — an HTTP request you configured, a Slack message you set up, etc. Flowo itself makes no analytics calls, no update checks, nothing.
