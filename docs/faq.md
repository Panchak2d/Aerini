# FAQ

Quick answers to things people ask before they've read anything else, or in the middle of using Aerini. Each answer is short; follow the link if you want the full detail behind it.

If something in Aerini isn't working the way you expect, this isn't the right page: see [Troubleshooting](troubleshooting.md) instead.

## Does Aerini collect any telemetry or send data anywhere on its own?

No telemetry, analytics, or crash reports of any kind, and nothing runs in the background to phone home. The only outbound connection Aerini's own interface makes without you asking for it is loading one font, Inter, from Google Fonts over HTTPS. Checking for app updates is a separate, one-click action you take yourself (see below), not something that happens automatically. Anything else that leaves your machine (an HTTP Request node calling an API, an email node sending mail) only happens because a node you configured is the one doing it. See [Security](guide/security.md) for the full model.

## Does Aerini work without an internet connection?

Yes, for anything that doesn't call an outside service. The app itself needs no connection to run. A Schedule trigger fires on your machine's own clock, and a Webhook trigger just waits for an incoming request; neither needs internet on Aerini's end to do that. The only time a workflow needs internet is when one of its nodes talks to something external (an HTTP Request node, a Slack node, an AI Prompt node pointed at a hosted model). See the [Nodes Reference](guide/nodes.md) for which nodes reach outside your machine and which stay local.

## Where do my workflows, credentials, and run history actually live?

Everything stays on the machine Aerini runs on, inside one per-OS data folder. Workflows and their run history sit in a local database there; saved credentials sit in a separate, encrypted one, with the actual encryption key kept in your OS keyring rather than in that folder. See [Installation §Uninstalling](getting-started/installation.md#uninstalling) for the exact folder paths on each OS, and [Credentials](guide/credentials.md) for how the encryption and key backup work.

## Do I need to know how to code to use Aerini?

No. Nodes are configured through forms in the canvas UI, and you connect them by dragging, not by writing anything. One optional node, [Code (JS)](guide/nodes.md#code-js), lets you drop in a JavaScript snippet if you want to, but nothing in Aerini requires it. See [Concepts](getting-started/concepts.md) for how nodes and connections fit together.

## Is there a mobile app?

No. Aerini is a desktop app for macOS, Windows, and Linux, plus a separate `aerini-server` binary for headless or server use. There's no phone or tablet app. See [Installation](getting-started/installation.md) for the platforms it supports.

## What's the difference between clicking Run and setting up a background run?

Run (or Run Now) executes the workflow once, immediately, and shows you the result right there. A background run hands the workflow to Aerini's scheduler, which owns it from then on and fires it on its own, on a timer or when a webhook request arrives, whether or not you're looking at the app. See [Background Runs §Two ways to run a workflow](guide/background-runs.md#two-ways-to-run-a-workflow) for the full picture.

## What happens to a running workflow if I close the app?

Nothing stops. Closing Aerini's window just hides it to your system tray; the app, and anything it's running, keeps going. Fully quitting Aerini (Quit Aerini from the tray menu) is what actually stops everything, and if that happens mid-run, the run shows up in [run history](guide/background-runs.md#run-history) as Interrupted rather than a pass or fail. See [Background Runs §Stopping background runs for good](guide/background-runs.md#stopping-background-runs-for-good) for the full detail.

## Can I run Aerini without the desktop app open?

Yes. Export a workflow with Export for Server from the toolbar's Export menu, then run it with `aerini-server`, a separate binary built for exactly this: no desktop UI, its own scheduler, its own run history. It has two modes: one that runs a single exported workflow, and one that runs a full multi-workflow REST API. See [Background Runs §Beyond the desktop app](guide/background-runs.md#beyond-the-desktop-app) for setup.

## Can I import or export workflows?

Yes. The toolbar's Export menu can save the current workflow as a `.aerini` file, or export every workflow at once. To bring one back in, drag a `.aerini` or `.json` file onto the canvas, or use the import button on an empty canvas; Aerini shows you a preview of what it contains before anything is actually imported. This is separate from Export for Server, which packages a workflow to run under `aerini-server` rather than for sharing or backup.

## How do I update Aerini?

Open Settings and click Check for Updates. Aerini checks for a newer release and, if one exists, gives you a link to it; it never downloads or installs anything on its own, and that check only happens when you click the button. Getting the new version means downloading and installing it the same way you installed Aerini the first time. See [Installation](getting-started/installation.md) for the steps for your OS.

## Is Aerini free to use?

Yes. Aerini is open source under AGPL-3.0. You can use, modify, and self-host it freely, and running the unmodified `aerini-server` binary internally for your own team never triggers any extra obligation. If you modify Aerini and run it as a network-accessible service, though, AGPL-3.0 requires releasing those modifications under the same license. A separate commercial license removes that requirement for one named product or service (with any number of end customers), for cases like building a product or SaaS on top of Aerini, or deploying a modified `aerini-server` as a service for others. See the repository's `README.md` for the full terms.

## See also

- [Troubleshooting](troubleshooting.md), for when something isn't working rather than a question about how it works
- [Getting Started](getting-started/getting-started.md), for a first walkthrough
- [Glossary](glossary.md), for terms used across these docs
