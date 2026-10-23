# Chat with a Local Model

This page builds a small workflow and opens the [Chat Panel](chat-panel.md) on it, so you can talk to a model running on your own machine from inside Aerini. Nothing you type leaves your computer.

**Before you start:** Ollama is installed, you've pulled a model, and AI Prompt already answers from it. If not, do the [Local Models Quickstart](local-models-quickstart.md) first. You also need the desktop app; the Chat Panel doesn't exist in `aerini-server`.

## 1. Build the workflow

Chat needs a **Webhook** node to receive your message and an **Output** node to send the reply back. Put an **AI Prompt** between them and connect the three in order: Webhook, AI Prompt, Output.

**Webhook.** Leave the defaults (port `3456`, path `/webhook`). The Chat Panel sends your message to this node on your own machine, so there's no secret or token to set.

**AI Prompt:**

| Field | Value |
|---|---|
| **Provider** | `local` |
| **Base URL** | `http://localhost:11434/v1` (the **Use Ollama defaults** button fills it in) |
| **Model** | The exact tag you pulled, for example `llama3.2`. **Fetch Models** lists what's installed. |
| **Prompt** | `{{Webhook.output.body.message}}` |
| **System** | Optional. For example: `You are a concise, friendly assistant.` |

**Output.** Set **Field** to `content`. Output shows whatever reaches it, and AI Prompt returns an object, so without this the Chat Panel would pretty-print the whole object as JSON instead of showing just the reply text.

Save with `Ctrl+S` (`Cmd+S` on macOS).

## 2. Open Chat and start the workflow

The **Chat** button in the toolbar appears once the canvas has a Webhook and an Output node. Click it, or press `Ctrl+Shift+C` (`Cmd+Shift+C` on macOS).

The workflow has to be running in the background before it can reply. A notice in the panel has a **Start** button. Click it. It does the same as the toolbar's **Schedule Run**: it saves the workflow if needed and hands it to the scheduler. If port `3456` is taken, Chat tries the next few ports and tells you which one it used.

> [!NOTE]
> Don't use the plain **Run** button with a Webhook trigger. It waits for one request that Chat can never send. If you press it while Chat is open, Aerini offers **Start Instead**.

## 3. Send a message

Type a message and press **Enter** (`Shift+Enter` for a new line). You should see a typing indicator, and then the reply appears all at once. Replies don't stream in word by word.

Local models can be slow, especially on the first message while Ollama loads the model. After 30 seconds the panel adds a note that the model is still working. That's not a failure: Chat keeps waiting and keeps the input locked until the reply arrives or the run fails. To give up on a reply, click **Stop**; the workflow keeps running and your next message works without a restart.

The AI Prompt node itself waits up to an hour by default. If you have slow hardware and want a different limit, change **Timeout (seconds)** on the node.

## 4. Make it remember the conversation

Right now every message starts fresh, because AI Prompt only sees the current message. To give the model memory, add an **AI Memory** node:

1. Add an AI Memory node with **Operation** `read`, **Session ID** `{{Webhook.output.body.session_id}}` and **Max Messages** `20`, between Webhook and AI Prompt.
2. Change the AI Prompt **Prompt** to include the history, for example: `Conversation so far (newest first): {{AI Memory (read).output.messages}}` followed by a new line and `User: {{Webhook.output.body.message}}`.
3. After AI Prompt, add two AI Memory nodes with **Operation** `append` and the same Session ID: one with **Role** `user` and **Content** `{{Webhook.output.body.message}}`, then one with **Role** `assistant` and **Content** `{{AI Prompt.output.content}}`, chained one after the other.
4. Wire **Output** directly from AI Prompt, in parallel with the append branch, and set Output's **Source Node** to `AI Prompt` and **Field** to `content`. With a blank Source Node, Output falls back to whichever node finished most recently, which can be an append node, and the panel would then show its output instead of the reply.

The complete wiring, including why it takes two appends, is in the [Chatbot with AI Memory and Widget example](https://github.com/Panchak2d/Aerini/blob/main/examples/Chatbot___AI_Memory___Widget.md). The Chat Panel's **Clear** button and session delete both clear the matching AI Memory session, because they use the same session ID.

> [!WARNING]
> History grows with every turn, and Ollama's context window is limited. Current Ollama documentation gives a default of 4096 tokens, and older versions used 2048. When the history no longer fits, the model can lose the start of the conversation without any error. Keep **Max Messages** modest, or raise Ollama's limit with `OLLAMA_CONTEXT_LENGTH`.

## If it didn't work

| What you see | What to do |
|---|---|
| No **Chat** button | The canvas needs both a Webhook and an Output node. |
| The reply is a block of JSON | Set the Output node's **Field** to `content`. |
| A red error with **Retry** | The run failed. Open the run in [Background Runs](background-runs.md) to see which node failed. A network error on AI Prompt usually means Ollama isn't running. |
| **Restart & Retry** appears | The background job stopped. This button restarts it and resends. |
| Messages are ignored while a reply is pending | The workflow handles one message at a time, which is why the input stays locked. |
| `model is required when provider is "local"` | Model is blank on AI Prompt. |

::: details For experienced users
- Chat posts `{"message": "...", "session_id": "..."}` to the workflow's own Webhook listener on `127.0.0.1`, plus `attachments` when files are attached. The reply is read from the **Output** node's result in run history, not from the HTTP response.
- Attachments: files reach the model only if a node wires them in, for example `{{Webhook.output.files}}` into AI Prompt's **Files** port. The panel offers to make that connection for you. A local model can only use an image if it supports images, and PDFs are sent as `file` blocks that a local or third-party endpoint may reject. Text and markdown files are decoded into the system prompt for every provider.
- Several Webhook or Output nodes: Chat uses whichever it finds first while scanning the workflow.
- Tool calling with the **AI Agent** node depends on the specific model, not just the provider. See [Local Models, AI Agent](local-models.md#ai-agent).
- Per-workflow chat settings (attachments, image replies, message length) live under **Workflow Settings**; see [Chat Panel, Configuring Chat](chat-panel.md#configuring-chat).
:::
