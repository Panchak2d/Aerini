# Parallel Execution

## What it does

When enabled, independent branches in a workflow run concurrently instead of
sequentially. Nodes with no data dependency on each other execute at the same time.

## When to use it

- Workflows with multiple independent API calls (fetch weather + fetch stock price
  + fetch news — then merge results)
- Any workflow where branches share no state and execution order does not matter

## When NOT to use it

- Workflows with side-effect nodes (Slack, Email, Stripe) where you need to
  guarantee execution order
- Workflows where a downstream node depends on the *completion order* of upstream
  branches, not just their *outputs*

## How to enable

Workflow settings panel → **"Run independent branches concurrently"**

Default: off. The setting is stored per-workflow, so enabling it for one workflow
does not affect others.

## Max concurrent nodes

Default: 8. Reduce this if your workflow is hitting external rate limits from
simultaneous requests to the same service.

## What "independent" means

Two nodes are independent if neither is a transitive upstream dependency of the
other. Flowo determines this from the workflow's edge graph automatically.
