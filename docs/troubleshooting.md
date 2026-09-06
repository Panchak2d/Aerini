# Troubleshooting

This page is organized by symptom: pick the thing that's actually going wrong and jump to it. It doesn't assume you've read the Nodes, Expressions, Credentials, or Background Runs guides first; where the real detail lives on one of those pages, this page links to it instead of repeating it.

If you haven't built a workflow yet, start with [Getting Started](getting-started/getting-started.md) instead. This page is for when something that used to work, or something you expected to work, isn't.

## Aerini won't open at all

On macOS and Windows, the very first launch usually triggers a warning from the OS itself (Gatekeeper on macOS, SmartScreen on Windows) rather than anything wrong with the app. [Installation](getting-started/installation.md) covers the exact steps for each platform; you only need to do it once per machine.

## A workflow won't run

Before anything executes, Aerini checks the workflow and refuses to run it if something's missing. You'll see a toast listing up to three problems at once, each one naming the node and the field: a URL missing on an HTTP Request, a command missing on Shell Command, a script missing on Code, a prompt missing on AI Prompt, and so on. An empty canvas or a workflow with no trigger node gets caught the same way, before it ever tries to run anything.

If you clicked Run and it doesn't look like anything happened, check these in order:

- **You dismissed the "this workflow runs code on your computer" dialog.** This is the only genuinely silent case: clicking anywhere other than confirm cancels the run with no toast and no error at all. That's expected, not a bug; declining the dialog is supposed to do nothing. See the dangerous-node section below for when this dialog shows up.
- **Something's already running.** Either this exact workflow from another action, or, if you're testing a single node while a full run is still going, the shared "one run at a time" guard blocks the new one. A brief info toast says so, easy to miss if you weren't watching for it.
- **Your Test Input JSON box has something invalid in it.** Aerini doesn't block the run for this; it runs with no test input instead and shows a brief toast saying the JSON was ignored. If the output doesn't match what you expected from your test data, check that box for a stray comma or an unclosed brace before anything else.

If your trigger is a **Webhook**, pressing Run behaves differently than every other trigger: instead of running the rest of the workflow immediately, it waits for a real HTTP request to arrive before doing anything. If you're just trying to test the nodes after the trigger, that's expected; see [Background Runs](guide/background-runs.md#two-ways-to-run-a-workflow) for the two ways to run a webhook workflow and what each one waits for.

If the run looks stuck on "Running..." for a long time, it isn't stuck forever: a full run times out after 2 minutes and a single-node test run times out after 1 minute, and either one ends with a clear timeout message rather than hanging silently. Most individual nodes are bounded well under that on their own (HTTP Request and Shell Command both give up after 30 seconds by default), so hitting the full 2-minute cap on an ordinary workflow is unusual. A Loop node working through a long list, or a workflow with a lot of sequential steps, is the most common way the total adds up even though no single step is actually hanging.

## A scheduled or webhook workflow stops firing, or keeps failing every time

[Background Runs](guide/background-runs.md) already covers the core behavior here in depth: why an overlapping run gets skipped instead of queued, how a duplicate webhook delivery inside the dedup window gets skipped the same way, what happens when two workflows fight over the same port, and why a Run on launch workflow can start again on its own after you thought you'd stopped it. Start there rather than re-deriving any of it here.

One thing worth knowing that isn't covered on that page: **Schedule Run doesn't run the same pre-flight check the Run button does.** A workflow missing a required field, one that would get caught immediately if you pressed Run by hand, can still be scheduled successfully. It will then fail on every single fire instead of being caught up front. If a background job keeps failing and you're not sure why, open [its run history](guide/background-runs.md#run-history) and look at the actual error on a failed entry rather than assuming the schedule itself is broken.

## A field comes out empty, or you see a warning in the run's log you don't recognize

[Expressions](guide/expressions.md) explains the general rule: an expression Aerini can't resolve never crashes the workflow, it just resolves to an empty string and logs a warning explaining why. The run's log is the first place to check when a field came out blank; the warning usually says exactly what went wrong.

The most common causes:

- The node name in your expression doesn't match any node on the canvas, usually because the node got renamed after the expression was written.
- The node you're referencing is later in the chain, or on a branch that hasn't run yet at the point this expression is evaluated.
- The field path after `.output` doesn't exist on that node's actual output shape.
- You're reading a `{{$vars.key}}` that was never set by a Set Variable node earlier in this same run.
- You're reading `{{$env.VAR_NAME}}` and no allowlist is in effect. This only ever works when running `aerini-server api` with `--allow-env-vars` set; the desktop app and `aerini-server serve` never have an allowlist to pass, so any `$env` expression there resolves empty regardless of whether the variable actually exists on the machine.

Two nodes sharing the same name is a quieter version of the same problem: the expression still resolves, silently, against whichever one comes first, and a warning about the ambiguity lands in the log. Renaming nodes to be unique fixes it for good.

There's one case worth calling out on its own, because it doesn't just come out empty, it comes out **wrong**: a path like `{{Webhook.body.ref}}` is missing the required `.output` segment (it should read `{{Webhook.output.body.ref}}`). Aerini warns about the malformed shape but still tries to continue, and ends up reading a completely different, usually nonexistent, top-level field instead of the one you meant. If a field has some value in it, just not the value you expected, check every expression referencing that node for a missing `.output` segment before looking anywhere else.

A credential that fails to resolve produces the same kind of warning through an unrelated mechanism: not an unresolved expression, but a credential ID the workflow references that no longer resolves in the store it's running against. See [A credential isn't showing up in a node's dropdown](#a-credential-isnt-showing-up-in-a-nodes-dropdown) below for that case specifically.

## A credential isn't showing up in a node's dropdown

For most nodes, a field described as coming from your saved credentials shows a picker. A few don't, even though their description says they should: [Credentials](guide/credentials.md#using-a-credential-in-a-node) has the full list, currently Discord's webhook URL, S3's access keys, and the OAuth `client_id`/`client_secret` pair on both Google Sheets and Social Upload. For those, typing the value straight into the node field is the correct, working flow, not a workaround.

A related symptom: the dropdown is there and had something selected, but the node behaves as if the field is empty when it runs (an auth error, a rejected request). Check the run's log first. It's the same kind of warning as [any other blank-field warning](#a-field-comes-out-empty-or-you-see-a-warning-in-the-runs-log-you-dont-recognize), naming the credential and the reason it didn't resolve: not found, unreadable, or a store-level error. "Not found" is the most common of the three, and it happens when the credential ID the workflow points at no longer exists in the store it's actually running against, most often after copying a workflow file to a different Aerini install (a fresh profile, or the machine running `aerini-server`) without adding a credential with the same ID there first. Reopen the node's Connection section and reselect the credential to fix it.

## The "this workflow runs code on your computer" dialog shows up unexpectedly, or doesn't show up when you'd expect it

[Background Runs](guide/background-runs.md#unattended-execution-and-dangerous-nodes) already covers the core rule: a manual Run shows this confirmation once per session, a scheduled or webhook-triggered background fire skips it and only logs a warning instead. Two things worth adding:

- It isn't only Shell Command, Code, and Database that trigger it. Any node loaded from a `.wasm` plugin counts as well, since Aerini has no way to know what a plugin actually does on your machine.
- Approving it doesn't stick around forever. The approval is remembered only for the current app session, and only for that exact set of dangerous nodes on that exact workflow. Restart Aerini, or add or remove a dangerous node, and you'll see the dialog again the next time you run it.

## A webhook never receives a request

[Background Runs](guide/background-runs.md#how-webhook-behaves-in-the-background) already covers the two most common causes: the webhook only ever binds to `127.0.0.1`, so reaching it from outside your own machine (including from `aerini-server` running elsewhere) needs a reverse proxy in front of it, and two workflows can't claim the same port at once.

If you're testing with a plain Run press rather than Schedule Run, a few more specific failures are worth knowing about, since they show up as the run's own error rather than a toast:

- A port below 1024 is rejected outright; those need root/administrator privileges Aerini doesn't ask for.
- If another Webhook node is already listening on that port in this same running copy of Aerini, you'll get a clear message saying so rather than a confusing connection error.
- A wrong path on an otherwise-correct request comes back as **401 Unauthorized**, not 404. That's deliberate, so a guessed path can't be used to confirm a port is even listening, but it also means a 401 you get while testing might mean "wrong path," not "wrong secret." Check the path before assuming your secret header is the problem.
- If nothing valid arrives before the node's configured Timeout runs out (60 seconds by default), the run ends with a plain "no request received" failure.

## See also

- [Concepts](getting-started/concepts.md), for what a trigger, a run, and a workflow are
- [Nodes Reference](guide/nodes.md), for every node's exact fields and ports
- [Expressions](guide/expressions.md), for the full `{{...}}` syntax
- [Credentials](guide/credentials.md), for storing and using API keys
- [Background Runs](guide/background-runs.md), for schedules, webhooks, and run history in depth
- [FAQ](faq.md), for short answers to questions that aren't really a "something's broken" problem
