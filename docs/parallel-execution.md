# Parallel Execution

By default, Flowo runs a workflow sequentially — one node finishes, then the next one starts. When you have independent branches that don't depend on each other, this means waiting for one branch to finish before the other can begin, even if they have no relationship.

Parallel execution changes that. Independent branches run at the same time, and the workflow waits for all of them to finish before continuing to any node that depends on their results.

---

## When to use it

**Good candidates:**

- Workflows that make multiple independent API calls and then combine the results. For example: fetch weather data, fetch stock prices, and fetch news headlines simultaneously, then pass all three into a single summary prompt.
- Any workflow where the order of independent branches genuinely doesn't matter.

**When to leave it off:**

- Workflows with side-effect nodes — Slack, Send Email, Stripe — where you need to guarantee a specific execution order.
- Workflows where a downstream node depends not just on what upstream branches *produced*, but on the *order they completed in*.

If you're unsure, leave it off. Sequential execution is easier to reason about and debug.

---

## How to enable

Open the workflow settings panel and turn on **"Run independent branches concurrently"**.

The setting applies per-workflow. Enabling it for one workflow has no effect on others.

---

## Max concurrent nodes

Default: 8 nodes running simultaneously. Lower this number if your workflow is hitting external rate limits — calling the same API from 8 simultaneous nodes may trigger throttling that wouldn't happen with 2 or 3.

---

## What "independent" means

Two nodes are independent if neither is an upstream dependency of the other. Flowo determines this automatically from the edge graph — you don't need to mark anything manually. If node A produces data that node B needs, they're dependent and B will always wait for A, regardless of this setting.
