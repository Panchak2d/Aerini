# Glossary

Plain-language definitions for terms used across these docs. If you hit a word you don't recognize on any page, it should be here.

Terms are grouped by which layer of Aerini they belong to. Skip to the section you need, or search this page (Ctrl/Cmd+F) for the word.

## Workflow basics

### Workflow
The whole automation you build: a set of [nodes](#node) connected in an order, saved together as one file. A workflow is what you run, schedule, or trigger.

### Workflow file
The file a workflow is saved as: plain JSON under the hood, with a `.aerini` extension. You can drag one onto Aerini to import it, or export one to share or back up. A `.json` file with the same shape also works.

### Schema version
The version number in every workflow file's `schema_version` field, tracking which shape of the JSON format that file uses. Whichever app or server loads the file checks this field first, so older and newer files can still be opened safely. See [Updating §Opening older or newer workflow files](operations/updating.md#opening-older-or-newer-workflow-files) for what happens on each version mismatch.

### Migration
The automatic upgrade of a workflow file's JSON from an older [schema version](#schema-version) to the one the loading app or server understands, run the moment the file is opened, with no action needed from you, as long as a migration path exists between the two versions.

### Node
A single step in a workflow, one box on the [canvas](#canvas). Each node does one thing: make an HTTP request, send an email, check a condition, run on a schedule. A workflow is nodes wired together.

### Canvas
The visual surface where you drag nodes and draw connections between them. It's the main screen you work in.

### Connection
The line you draw from one node's output to another node's input. It tells Aerini "when this node finishes, send its result to that one next." Some nodes (like If or Switch) have more than one output, so their connections can branch.

### Port
The small connection point on the edge of a node that a connection attaches to. Outputs are on one side, inputs on the other.

### Trigger
The node that starts a workflow run. Every workflow needs one: [Manual Trigger](#manual-trigger), [Schedule](#schedule), or [Webhook](#webhook).

### Manual Trigger
A trigger you fire yourself, by pressing Run in the app or calling the server API. Good for testing, or for workflows you only ever want to run on demand.

### Run
One execution of a workflow, start to finish. It runs from the moment its trigger fires to its last node completing (or failing). Each run is tracked separately, even for the same workflow.

### Run history
The stored record of a workflow's past runs: when each one happened, whether it succeeded, and what it produced. Lets you check on a schedule you're not watching live.

### Retry policy
Per-node setting for what happens after a recoverable failure: how many extra attempts to make, and how long to wait between them. The default is no automatic retry, so a node with a real-world side effect (sending an email, charging a card) never fires twice by accident unless you turn retries on.

### Parallel execution
An opt-in workflow setting. Off by default, which runs nodes one at a time in a predictable order. Turned on, independent branches of the workflow run at the same time instead of waiting on each other. That's faster, but only safe when those branches don't depend on one another's side effects happening in a particular order.

## Scheduling and triggers

### Schedule
A trigger that fires a workflow automatically: at a fixed interval, on a [cron expression](#cron-expression), or once at a specific time.

### Cron expression
A five-part pattern describing a recurring time (`minute hour day month weekday`), e.g. `0 9 * * 1-5` for 9 AM on weekdays. Used by the Schedule node when you want a recurring, non-fixed-interval schedule. When both day and weekday are restricted, either one matching is enough (`0 9 1 * MON` is the 1st or any Monday).

### Webhook
A trigger that starts a workflow when an HTTP request hits a specific local address, for example when GitHub or Stripe sends a notification to a URL you gave them.

## Background execution

### Run on launch
A toggle that marks a Schedule- or Webhook-triggered workflow to start automatically every time you open Aerini, without clicking Schedule Run again. See [Background Runs §Run on launch](guide/background-runs.md#run-on-launch).

### Background run
A [run](#run) handed to Aerini's scheduler via **Schedule Run**, which keeps it going whether or not you're looking at it, until you stop it or quit Aerini. See [Background Runs](guide/background-runs.md).

### Dedup window
A Webhook setting: any request with a body identical to one already seen within this many seconds is skipped rather than re-run, so a provider that resends the same event doesn't trigger duplicate runs. Only applies once a workflow is running in the background. See [Nodes Reference §Webhook](guide/nodes.md#webhook).

### Interrupted
The run-history status shown when a run never finished because Aerini was force-quit or crashed mid-execution. Unlike a normal success or failure, an interrupted run has no recorded output and can't be replayed.

### System tray
The OS notification area icon Aerini keeps running from after its window is closed. Any background schedule or webhook listener keeps going as long as the tray icon is present; quitting from its menu is what actually stops everything.

## Data and configuration

### Expression
The `{{...}}` syntax used inside a node's settings to pull in a value from somewhere else, such as another node's output, the current run's ID, or an environment variable, instead of typing a fixed value.

### Credential
A stored secret, such as an API key, password, or OAuth token, that a node needs to talk to an outside service. Stored encrypted, referenced by name from the nodes that use it rather than pasted into them directly.

### Node registry
The engine's internal list of every node type it knows how to run: every built-in node, plus any [plugins](#plugin) you've installed.

### Node type
The category a node carries for organizing the node picker (for example, Action or Logic). Purely cosmetic; it has no effect on how a node runs. See [Nodes Reference](guide/nodes.md) for the full breakdown.

## Plugins and sandboxing

### Plugin
A node type added to Aerini without modifying or recompiling it: a compiled `.wasm` file you install through Settings. Plugin nodes show up in the node picker alongside built-ins, marked with a "Plugin" tag (also in the node info tooltip and the node's config panel), and carry a small "P" chip on the canvas.

### WASM
Short for WebAssembly, a portable, sandboxed binary format. Aerini plugins are compiled to WASM, which is what lets them run safely alongside the built-in engine without being written in a specific language or trusted with full access to your machine.

### WASI
The WebAssembly System Interface: the standard that defines what a WASM program is allowed to do on the host machine (read a file, make a network request, and so on). Aerini plugins target WASI Preview 2 specifically.

### WIT
Short for WebAssembly Interface Type, the interface language a plugin author uses to declare which functions their plugin implements, so Aerini's engine knows how to call it. You'll see `.wit` files if you write a plugin.

### Sandbox
An isolated environment a piece of code runs inside, with deliberately limited access to the rest of the system. Aerini runs each plugin execution in a fresh sandbox with a capped memory limit, no filesystem access, and only outbound HTTP allowed.

## Security

### OAuth
A way to authorize an app to act on your behalf with a third-party service, without ever handing that service your password. Aerini uses it for YouTube, Instagram, TikTok, and Google Sheets: you approve access in the provider's own login page, and Aerini receives a token it can use (and automatically refresh) instead of a fixed API key. See [Credentials §OAuth](guide/credentials.md#oauth) for the full walkthrough.

### AES-256-GCM
The encryption algorithm Aerini uses to encrypt every saved [credential](#credential) before writing it to disk. The key that does the encrypting is itself kept in your OS [keychain](#keychain), not in a plain file next to your workflows.

### Keychain
Common name for your operating system's built-in secure storage for secrets, used interchangeably with [keyring](#keyring) throughout these docs: Keychain on macOS, Credential Manager on Windows, Secret Service on Linux.

### SSRF
Short for Server-Side Request Forgery: an attack where something that makes requests on your behalf (like an HTTP node) gets tricked into hitting an address it shouldn't, such as internal infrastructure it wasn't meant to reach. Aerini validates outbound request targets by default to block this.

### Keyring
Your operating system's built-in secure storage for secrets: Keychain on macOS, Credential Manager on Windows, Secret Service on Linux. Also called the OS keychain. Aerini stores its credential-encryption key here rather than in a plain file.

### Scoped token
An API token used to authenticate against `aerini-server`'s REST API, limited to one of three scopes (`read`, `write`, or `admin`) so a token handed to one integration can't do more than it needs to.

## Under the hood

### aerini-engine
The Rust crate that actually runs workflows: the node registry, the executor, the scheduler, expression resolution. It doesn't know anything about a UI; the desktop app and `aerini-server` both sit on top of it.

### aerini-server
The headless binary that runs Aerini workflows without the desktop app, either as a single-workflow daemon or as a multi-workflow REST API for running things on a server or in Docker.

### Tauri
The framework Aerini's desktop app is built on: a lightweight shell that renders the web-based interface in a native window and lets it talk to the Rust engine underneath.

### IPC
Short for Inter-Process Communication, the mechanism the desktop app's interface uses to call into the Rust engine underneath it, for example "run this workflow" or "save this credential." Not something you interact with directly, but it's the term used if you're extending the desktop app itself.
