# Chat Panel

The Chat Panel is a built-in chat window for talking to one of your workflows directly inside the desktop app, without building a separate front end or embedding anything on a web page. This page covers what makes it appear, starting a workflow so it can reply, sending and receiving messages, managing sessions, the settings that control it, and how it relates to the embeddable [Widget](widget-embedding.md) and the [AI Memory](nodes.md#ai-memory) node.

If you haven't read [Concepts](../getting-started/concepts.md) yet, do that first for what a node and a run are. This page also assumes you've seen the Webhook and Output entries in [Nodes Reference](nodes.md#webhook); it won't repeat their field-by-field settings.

## Opening Chat

There's no toggle for turning Chat on. The **Chat** button in the toolbar, and its shortcut `Ctrl+Shift+C` (`Cmd+Shift+C` on macOS), only appear once the current workflow has a **Webhook** node and an **Output** node somewhere on the canvas — add both and the button un-hides itself. Try the shortcut before that and Aerini tells you exactly what's missing: "This workflow needs a Webhook trigger and an Output node to use Chat."

With more than one Webhook or Output node on the same canvas, Chat uses whichever one it finds first while scanning the workflow; there's currently no way to pick a specific pair if you have several.

## Starting the workflow

Chat needs the workflow running as a background job to reply — the same "handed off to the scheduler" state [Background Runs](background-runs.md) describes, not a foreground **Run**. While the workflow isn't running, the panel shows one notice with a **Start** button: centered in the panel when the session has no messages yet, and as a banner above the conversation once there are some. Once the workflow is running, the centered notice loses its button and just tells you to send a message. **Start** does exactly what the toolbar's **Schedule Run** does: saves the workflow if needed and hands it to the scheduler. If the workflow's configured port is already taken, Chat automatically retries on the next few ports up and tells you which one it landed on, the same fallback [Monitor](monitor.md#start-all-and-stop-all)'s Start All uses.

Pressing the main **Run** button while Chat is open and the trigger is a Webhook prompts a **Start Instead** confirmation rather than running once — a plain Run waits up to the Webhook node's own timeout for a single request and Chat can never get a reply that way, so Aerini offers to start it properly instead.

## Sending and receiving messages

Type a message and press **Enter** (`Shift+Enter` for a newline) or click send. With attachments turned on, you can also send files with no text; the message is then posted as an empty string, so a workflow that assumes `{{Webhook.output.body.message}}` is never empty needs to handle that case. Aerini posts `{"message": "...", "session_id": "..."}` — plus `attachments` if you've attached any files — straight to the workflow's own Webhook listener on `127.0.0.1`. Reference the message in your workflow as `{{Webhook.output.body.message}}`, same as any other Webhook-triggered run.

The HTTP call itself resolves immediately; it isn't what carries the reply back. Chat waits for the background run to finish and reads whatever the **Output** node produced — the same per-node output [Run history](background-runs.md#run-history) records for every background run. If nothing comes back within 30 seconds, the message shows a timeout error with a **Retry** button; if the webhook can't be reached at all (most often because the job stopped in between), you get **Restart & Retry** instead, which restarts the background job before resending. A reply that still arrives after a client-side timeout is shown when it lands rather than dropped.

A reply's Output value is rendered as Markdown (sanitized before display, so a workflow returning stray HTML can't inject anything into the app). If the Output node's value is a media batch containing images, Chat renders them as image bubbles instead of text — click one to view it full-size, or use the copy/download buttons on it — unless **Allow image responses** is off, in which case Chat shows a placeholder note that image responses are disabled, instead of the images. Any other non-string reply is pretty-printed as JSON. There's no partial or streaming reply: the whole thing appears at once when the run completes.

### Attachments

With **Allow file attachments** on, the paperclip button accepts `.png`, `.jpg`, `.jpeg`, `.webp`, `.gif`, `.pdf`, `.txt`, and `.md` files — the same types the AI Prompt node's static attachments accept. There's a running byte budget shown under the attachment chips as the request fills up: the whole request body, message text plus attachments plus JSON overhead, has to fit in the Webhook listener's own 1 MB hard cap, the same limit the [Widget](widget-embedding.md#what-each-error-means)'s "message too large" error refers to and shared with any other traffic hitting that same listener. Chat blocks an attachment that would exceed it before sending, rather than letting the request fail. Attached files reach the workflow on the Webhook node's `files` output (`{{Webhook.output.files}}`) as well as in `body.attachments`, so you can wire the Webhook node's output to a Files input such as an AI Prompt node's.

Once a message with attachments is sent, each file appears as a chip showing its filename inside your message. Click an image chip to view it full-size; any other chip downloads the file. The file itself is saved with the session in the app's local database, so the chip is still there after you switch sessions or restart the app. Clearing or deleting the session removes the stored files with it.

## Sessions

Chat keeps any number of named conversations per workflow, stored locally so they survive an app restart. The session menu (the label at the top of the panel) lists them, lets you switch, and creates a new one via **+ New Session**. Switching or creating is blocked while a reply is still pending, so you can't abandon a message mid-flight. Only the open conversation is loaded from disk; switching to another one may briefly show "Loading conversation…" while its messages are read, and the input is locked until it finishes. If the load fails, an error appears and you stay in the session you were in.

**Clear** empties the current session's messages but keeps its name; deleting a session from the menu removes it entirely (you can't delete the only remaining session). Both actions also clear that session's stored history in the **AI Memory** node, not just the Chat Panel's own message list — see [Chat Panel, Widget Embedding, and AI Memory](#chat-panel-widget-embedding-and-ai-memory) below for why.

## Configuring Chat

Open **Workflow Settings** in the toolbar and scroll to the **Chat** group. Each setting is per-workflow and saved in the workflow file.

| Setting | What it does |
|---|---|
| Allow file attachments | Shows the paperclip button. Off by default. |
| Allow image responses | Renders image replies as images; off shows a placeholder note instead. On by default. |
| Max message length | Character limit per message, enforced before sending. Default 2000. |
| Session persistence | Intended to control whether sessions are kept across restarts — currently read but not enforced: history is always saved locally regardless of this setting. |
| Show "Built with Aerini" | Toggles the small footer credit at the bottom of the panel. On by default. |

These map directly to `settings.chat` in the workflow file; see [Workflow File Format §Chat settings](../reference/workflow-file-format.md#chat-settings) for the raw field names, types, and JSON defaults.

## Chat Panel, Widget Embedding, and AI Memory

Chat Panel and the [embeddable Widget](widget-embedding.md) look similar — both are a chat window in front of a Webhook-and-Output workflow — but they're separate features with no shared code path:

- **Chat Panel** is desktop-only — there's no equivalent in `aerini-server`. It calls the workflow's webhook directly over the loopback address, the same machine and app, so there's no secret or token to configure — the trust boundary is "you're running the app," the same model [Security §Desktop security model](security.md#desktop-security-model) describes for the rest of the desktop app. Replies arrive over an internal Tauri event, not a network response.
- **Widget Embedding** is for a workflow reachable from *outside* the machine, on a web page you host somewhere else — including a headless deployment with no desktop app at all. It only works through `aerini-server api` mode, goes through an authenticated relay endpoint with a secret, a scoped token, CORS, and rate limits, and gets its replies over Server-Sent Events. None of that applies to Chat Panel.

Both happen to pair well with an **AI Memory** node for conversation continuity, and the connection between Chat Panel and AI Memory is closer than "pairs well": Chat Panel's **Clear** button and session delete both call directly into the AI Memory node's own `clear` operation, using the Chat Panel's session ID as the AI Memory session ID. If your workflow reads and appends to AI Memory using `{{Webhook.output.body.session_id}}` — exactly the pattern in [Chatbot with AI Memory and Widget](https://github.com/Panchak2d/Aerini/blob/main/examples/Chatbot___AI_Memory___Widget.md) — clearing a Chat Panel session also erases that conversation's memory, keeping the two in sync automatically. A workflow that doesn't use AI Memory at all is unaffected; the call still runs but has nothing to delete.

## See also

- [Nodes Reference](nodes.md#webhook), for the Webhook and Output nodes' full field tables, and [§AI Memory](nodes.md#ai-memory) for the node Chat Panel's Clear button reaches into
- [Widget Embedding](widget-embedding.md), for the public, server-hosted equivalent
- [Background Runs](background-runs.md), for what "running in the background" means and how Schedule Run works
- [Workflow File Format §Chat settings](../reference/workflow-file-format.md#chat-settings), for the raw `settings.chat` JSON shape
- [Keyboard Shortcuts](keyboard-shortcuts.md), for `Ctrl+Shift+C` alongside every other shortcut
