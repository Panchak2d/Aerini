# Build a Chat Bot

A chat bot in Aerini is three nodes: a **Webhook** that receives each message, an **AI Prompt** that answers it, and an **Output** that hands the answer back to the [Chat Panel](chat-panel.md). This page builds one, either from the starter template or by hand, and lists what to check when the reply isn't what you expect.

If you haven't read [Concepts](../getting-started/concepts.md), do that first for what a node and a run are. Model setup for a local server such as Ollama is in [Local Models](local-models.md), and a hosted model needs a key from [Credentials](credentials.md); neither is repeated here.

## The quick way: the template

On an empty canvas, click **Chat bot template**. Aerini adds a new workflow named "Chat bot" (with a number added if that name is taken) with the three nodes already wired and filled in. Then:

1. Double-click **AI Prompt** and choose a provider and model.
2. Click **Chat** in the toolbar, then **Start**.
3. Type a message.

## By hand

1. **Add a Webhook node.** Leave its defaults, or set Path to `/chat`. Chat sends every message to this node.
2. **Add an AI Prompt node and wire the Webhook's Triggered port to its In port.** If the Prompt field is empty, Aerini fills it with `{{Webhook.output.body.message}}` and says so. That expression is the text you typed in Chat, using whatever name your Webhook node has. Change it if your workflow needs something else, such as a fixed instruction followed by the message.
3. **Pick a provider and model on the AI Prompt node.** A local server also needs its Base URL; see [Local Models](local-models.md).
4. **Add an Output node and wire the AI Prompt's Success port to its In port.** If the Output's Field is empty, Aerini sets it to `content`, the part of the AI Prompt's result that holds the reply text. Chat needs an Output node to show anything.
5. **Click Chat, then Start, and send a message.**

Wire only the ports named above. A Webhook's **Error** port fires only when the Webhook itself fails, and AI Prompt's **Files** input expects a list of files, so wiring one to the other does nothing useful. To give the model the files you attach in Chat, see [Attachments](chat-panel.md#attachments).

## What Chat checks for you

The banner above the messages in the Chat Panel flags these problems and fixes each with one click. **Ctrl+Z** on the canvas undoes a fix.

| Banner says | Why | Fix button |
|---|---|---|
| `"AI Prompt" has no prompt, so it never sees what you type here.` | An empty Prompt (or Goal, on an AI Agent) means the model receives nothing from Chat. | **Use chat message** |
| `Replies will show as raw JSON, because "Output" displays everything "AI Prompt" returns.` | The Output's Field is empty, so Chat shows everything the AI node returned (`content`, token counts, model name) instead of just the reply. | **Show reply text only** |

The banner also reports wiring problems and unread attachments; see [Chat Panel](chat-panel.md).

## When it doesn't work

- **Nothing happens when you send a message.** The workflow has to be running as a background job: press **Start** in the Chat Panel. The main **Run** button runs it once and can't deliver a reply.
- **The reply is a block of JSON.** Set the Output's Field to `content` (`result` for an AI Agent).
- **The reply is empty or the run fails right away.** Open the run in the **History** tab of the Output drawer and read its error. A missing model, a wrong Base URL and a missing API key are the usual causes.
- **The model doesn't know what you said earlier.** Each message is a separate run. Add an **AI Memory** node, as in [Chatbot with AI Memory and Widget](https://github.com/Panchak2d/Aerini/blob/main/examples/Chatbot___AI_Memory___Widget.md).

## See also

- [Chat Panel](chat-panel.md), for sessions, attachments and settings
- [Expressions](expressions.md), for the `{{ }}` syntax
- [Examples](examples.md), for a chat bot with memory
