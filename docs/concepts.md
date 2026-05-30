# Concepts

This page explains what everything in Flowo is and how it fits together. If you've never used an automation tool before, start here before anything else.

---

## Workflow

A workflow is a set of steps that run in order. Each step does one thing — fetch some data, send a message, make a decision — and passes its result to the next step.

Think of it like a recipe. A recipe says "first do this, then do that, and if this condition is true, do this other thing." A workflow is exactly that, except instead of cooking instructions, the steps are things like "call an API" or "send a Slack message."

In Flowo, a workflow is what you see on the canvas: a set of connected boxes.

---

## Node

A node is a single step in a workflow. Every box on the canvas is a node.

Each node does exactly one thing:
- A **Schedule** node provides the starting signal on a timer
- An **HTTP Request** node fetches data from a URL
- A **Slack** node posts a message to a channel
- An **If / Condition** node checks a value and sends execution down one of two paths
- A **Code (JS)** node runs a small JavaScript snippet you write

Nodes have inputs (data coming in) and outputs (data going out). You connect them by drawing lines between them.

---

## Canvas

The canvas is the visual area where you build workflows. You place nodes on it and draw lines between them to create the flow of data.

**Ports** are the small circles on the edges of each node:
- Left side = input port (data flows in)
- Right side = output port (data flows out)

To connect two nodes: click and hold the output port of one, drag to the input port of another, release. A line (connector) appears. To remove it, drag it off the port onto empty space.

Some nodes have multiple output ports — for example, If / Condition has a "True" output and a "False" output. Drag connectors from whichever outputs apply to your workflow.

---

## Trigger

Every workflow needs to start somewhere. The first node in any workflow is always a trigger node — it's what starts the whole thing.

There are three types of triggers:

- **Manual Trigger** — runs when you press the Run button. Good for testing.
- **Schedule** — runs on a timer (e.g. every hour, or at 9 AM on weekdays).
- **Webhook** — runs when an HTTP request arrives at a specific URL and port on your machine.

A workflow without a trigger can't run automatically. Only Manual Trigger workflows need to be run by hand; Schedule and Webhook workflows can run in the background.

---

## Run

A run is one execution of a workflow from start to finish. Every time the trigger fires and the workflow completes (or fails), that's one run.

After a run, Flowo shows you what happened:
- **Summary** — which nodes ran, how long they took, pass/fail
- **Results** — the full output of every node (what data each one produced)
- **Errors** — what went wrong and where, with the full error message
- **Logs** — a chronological log of every step
- **History** — a list of all past runs you can browse

---

## Expression

An expression is how you pass data from one node to the next inside your configuration.

For example: an HTTP Request node fetches some JSON data. The next node — a Slack node — needs to use part of that data in its message. You write `{{HTTP Request.output.body.user.name}}` in the Slack message field, and Flowo replaces that with the actual value when the workflow runs.

The `{{...}}` syntax is an expression. It tells Flowo "look up this value from a previous node's output and put it here."

You don't need to memorize this syntax. Pressing `{{` in any text field opens a dropdown that shows you all available values and lets you click to insert them.

See [Expressions](expressions.md) for the full reference.

---

## Credential

A credential is a stored API key or secret.

Most services (OpenAI, Slack, Stripe, etc.) require an API key to prove who you are when making requests. Instead of pasting the key directly into a node's config — where it would be visible in your workflow and saved in plaintext — you store it once in Flowo's credential store. Flowo encrypts it on disk.

When you configure a node that needs an API key, you pick the credential by name from a dropdown. The actual key value is never shown in the UI again, and it's never written to your workflow file.

See [Credentials](credentials.md) for how to add credentials and where to find API keys for each service.

---

## Background Run

A background run is a workflow that keeps executing on its own while you use Flowo for other things, or while it's minimized to the system tray.

Background runs require a Schedule or Webhook trigger. Once you start a background run, the scheduler daemon takes over — it waits the configured interval, runs the workflow, records the result, and waits again.

Background runs stop when you close Flowo. For workflows that need to run 24/7 without the app open, deploy to a server — see [Server Deployment](server-deploy.md).

---

## flowo-server

`flowo-server` is a command-line program included with Flowo that runs workflows on a Linux server without the desktop app.

There are two modes:
- **Serve mode** — deploys one specific workflow. The desktop app's "Export for Server" button produces a zip with everything needed.
- **API mode** — manages many workflows via a REST API.

You don't need `flowo-server` to use Flowo on your own computer. It's only needed when you want to run workflows on a remote server 24/7.

---

## Output Drawer

The output drawer is the panel that slides up from the bottom of the Flowo window after a run. It shows everything that happened during the run — what each node produced, any errors, and all log lines.

You can also open the drawer manually to browse past runs from the History tab.

---

## Version History

Every time you save a workflow, Flowo takes a snapshot of it. You can browse and restore any previous version by right-clicking the workflow in the sidebar and choosing **Versions**.

This is different from run history. Version history is about what the workflow *looked like*; run history is about what it *produced* when it ran.
