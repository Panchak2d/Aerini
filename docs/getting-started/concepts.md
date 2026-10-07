# Concepts

This page explains the handful of ideas everything else in Aerini is built from: workflow, node, canvas, connection, trigger, and run. Once these click, every other page in these docs will make more sense. They all assume you know these six words.

> [!TIP]
> This page reads best *after* you've built something. If you haven't gone through [Installation](installation.md) and [Getting Started](getting-started.md) yet, do that first. Coming back here with a real workflow already open makes these ideas concrete instead of abstract.

## The shape of a workflow

A **[workflow](../glossary.md#workflow)** is a small assembly line. Something happens (a [trigger](../glossary.md#trigger)), and then a sequence of steps runs one after another, each one handing its result to the next.

Take a real example that ships with Aerini: a scheduled report.

```
Schedule  →  HTTP Request  →  Transform Data  →  Send Email
(trigger)     (fetch data)     (reshape it)       (deliver it)
```

Every morning at 8 AM, the Schedule node fires. It hands off to the HTTP Request node, which fetches some data from an API. That data flows into Transform Data, which pulls out the one field that matters. That result flows into Send Email, which sends it to you. Four boxes, three arrows, one automation.

That's a workflow: a trigger, some nodes, and the connections between them, saved together as one file (a **workflow file**, see the [glossary](../glossary.md#workflow-file)).

## Node

A **[node](../glossary.md#node)** is one step, one box in the diagram above. Each node does exactly one job: make a request, send an email, check a condition, wait for a timer. You build a workflow by placing nodes and deciding what order they run in.

Nodes fall into seven categories: Triggers, Core Actions, Files & Storage, Integrations, AI, Logic, and Utility. You don't need to memorize them to use them. When you're ready to see everything available, [Nodes Reference](../guide/nodes.md) lists all of them, grouped the same way, with what each one does and what settings it takes.

Some nodes only have one way out (HTTP Request always produces one result). Others branch: an If / Condition node has two outputs, "True" and "False," and a Switch node can have several. That branching is what lets a workflow make decisions instead of just running straight through.

## Canvas

The **[canvas](../glossary.md#canvas)** is the screen where you do all of this, the surface you drag nodes onto and draw connections across. It's just the visual editor; the workflow itself is the nodes-and-connections structure underneath it, which is why you can save it, share it, or edit it as a file independently of ever opening the canvas again.

## Connection

A **[connection](../glossary.md#connection)** is the line between two nodes, the arrows in the diagram above. Drawing one from Node A to Node B means "when A finishes, send its output to B, and start B running."

Each node has **[ports](../glossary.md#port)** on its edges: outputs on one side, inputs on the other. A connection always runs from one port to another. A branching node like If has two output ports (one per branch); connect something to only the branch you care about, and the other branch simply doesn't run when it's not the one taken. An output can feed any number of inputs, but an input takes one connection: dropping a second onto it replaces the first. Merge is the exception, and takes many.

## Trigger

Every workflow needs exactly one way to start, and that's the job of a **[trigger](../glossary.md#trigger)** node. Aerini has three:

- **[Manual Trigger](../glossary.md#manual-trigger)**: you press Run yourself, or call the server API. Best for testing, or for anything you only want to happen on demand.
- **[Schedule](../glossary.md#schedule)**: fires automatically on a timer, using a fixed interval, a recurring [cron expression](../glossary.md#cron-expression), or a specific one-time date.
- **[Webhook](../glossary.md#webhook)**: fires when an HTTP request arrives, so something else (GitHub, Stripe, your own code) can start the workflow.

The trigger is always the first node in the chain; nothing runs before it.

## Run

A **[run](../glossary.md#run)** is one execution of a workflow, start to finish: a single trip down the assembly line. If your scheduled report fires every weekday morning, that's five separate runs a week, each tracked on its own: when it happened, whether every node succeeded, and what came out the other end. That record is the **[run history](../glossary.md#run-history)**, and it's how you check on a schedule without having to watch it fire live.

## Putting it together

Back to the example:

1. **Trigger**: Schedule fires at 8 AM.
2. **Node**: HTTP Request runs, fetching data.
3. **Connection**: its output flows into Transform Data.
4. **Node**: Transform Data reshapes the result.
5. **Connection**: its output flows into Send Email.
6. **Node**: Send Email sends it.
7. **Run**: the whole trip just described is one run, now sitting in run history.

That's the entire mental model. Everything else, like the specific nodes available, how to reference one node's output from another (see [Expressions](../guide/expressions.md)), or how to store the API keys and SMTP details a workflow like this needs (see [Credentials](../guide/credentials.md)), builds on exactly these six ideas.

## What's next

- [Nodes Reference](../guide/nodes.md): every node Aerini ships with.
- [Expressions](../guide/expressions.md): the `{{...}}` syntax for wiring one node's output into another's settings.
- [Glossary](../glossary.md): quick lookup for any term on this page.
