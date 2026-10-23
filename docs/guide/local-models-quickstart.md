# Local Models Quickstart

By the end of this page, an **AI Prompt** node in Aerini will answer a question using a model running on your own computer. The prompt and the reply never leave your machine, and there's no API key and no bill. It takes about 15 minutes, most of it waiting for a model to download.

**You'll need:** the Aerini desktop app (see [Installation](../getting-started/installation.md)) and enough free disk space for a model. Models are multi-gigabyte downloads.

## What this is, in one minute

A **local model** is an AI model that runs on your hardware instead of in a company's cloud. **Ollama** is a free program that downloads these models and runs them. Aerini talks to Ollama over your own machine, the same way it would talk to OpenAI or Claude over the internet, so any workflow you can build with a hosted model works with a local one.

Two programs, two jobs:

| Program | Job |
|---|---|
| **Ollama** | Downloads models, keeps them on disk, runs them. |
| **Aerini** | Sends the prompt to Ollama and uses the reply in your workflow. |

Ollama owns the models. You can download one with Ollama in a terminal (Step 2), or with the **Pull model** button on the node in the desktop app (Step 4). Aerini's **Fetch Models** button shows which ones you have installed.

## Step 1: Install Ollama

Download the installer for your system from [ollama.com/download](https://ollama.com/download) and run it. On Linux, Ollama's own install script also works:

```bash
curl -fsSL https://ollama.com/install.sh | sh
```

## Step 2: Download a model

Open a terminal (PowerShell on Windows) and run:

```bash
ollama pull llama3.2
```

This downloads the model once and stores it on disk. It can take a while on a slow connection. Ollama's own documentation says to pull a model before you use it.

Check that it's there:

```bash
ollama list
```

You should see `llama3.2` in the list. Ollama is now running in the background on `http://localhost:11434`.

Prefer not to use a terminal? Skip the pull for now: once Step 4 points the node at Ollama, type the name in **Model** and click **Pull model** instead.

> [!TIP]
> `llama3.2` is just a small, popular example. Any model Ollama can run works the same way in Aerini. Whatever you pull, you'll use its exact name in Step 4.

## Step 3: Build a three-node workflow

Open Aerini and start a blank canvas, as in [Getting Started](../getting-started/getting-started.md). Add these three nodes and connect them in order:

1. **Manual Trigger** (under Triggers)
2. **AI Prompt** (under AI)
3. **Output** (under Utility)

## Step 4: Point AI Prompt at Ollama

Double-click **AI Prompt** to open its settings, and set:

| Field | Value | Why |
|---|---|---|
| **Provider** | `local` | `auto` also works, but picking `local` makes the **Use Ollama defaults** button appear. |
| **Base URL** | Click **Use Ollama defaults** | It fills in `http://localhost:11434/v1`. The `http://` and the `/v1` both matter. |
| **Model** | Click **Fetch Models**, then pick `llama3.2` | Fetch Models lists the models Ollama already has on disk. You can also type the name. |
| **Prompt** | `Say hello in five words.` | Anything will do for a first test. |
| **API Key** | leave empty | Ollama doesn't check for one. |

**Fetch Models** asks your Ollama server what it has installed and shows the answer as a dropdown. It only lists; it never downloads anything. If the list comes back empty, you haven't pulled a model yet: go back to Step 2, or type `llama3.2` in **Model** and click **Pull model**, which appears under **Fetch Models** once Aerini sees Ollama. Closing the node's settings stops a download in progress.

## Step 5: Run it

Press **Run**. The first reply is slow, because Ollama has to load the model into memory. Replies after that are quicker. Ollama keeps a model loaded for about five minutes after its last use by default, so a pause longer than that means the next request pays the loading cost again.

Open **Output**'s result and you should see what AI Prompt returned: the reply in `content`, plus the `model` and `provider` it used.

You've now run a model on your own machine from a visual workflow.

## If it didn't work

| What you see | What to do |
|---|---|
| A network error straight away | Ollama isn't running, or the port in Base URL is wrong. Start Ollama and check Base URL. |
| `model is required when provider is "local"` | Model is blank. Fetch Models and pick one, or type the name. |
| The reply is empty, or the node says it can't find the model | The Model name doesn't match an installed one. Run `ollama list` and copy the name exactly. |
| `URL scheme 'localhost' is not permitted` | `http://` is missing from the front of Base URL. |

More causes and fixes are in [Local Models, If it's not working](local-models.md#if-it-s-not-working).

## Where next

| You want to... | Go to |
|---|---|
| Chat with this model from a window inside Aerini | [Chat with a Local Model](chat-with-a-local-model.md) |
| Save the Base URL and Model so you don't retype them | [Local Models, Reusing this setup](local-models.md#reusing-this-setup-with-a-saved-credential) |
| Reach Ollama on another machine on your network | [Local Models, Reaching a model server on your LAN](local-models.md#reaching-a-model-server-on-your-lan-not-just-this-machine) |
| Use a local model with the AI Agent node and tools | [Local Models, AI Agent](local-models.md#ai-agent) |
| Look up every field | [Nodes Reference, AI Prompt](nodes.md#ai-prompt) |

::: details For experienced users
- The `local` provider is wire-identical to `openai`: Aerini posts to your Base URL plus `/chat/completions`. Ollama serves that under `/v1`.
- `auto` resolves to `local` for the literal hostname `localhost` and for loopback and private-network IPs, by inspecting the URL text, not DNS. A hostname that needs a lookup, such as `myollama.local`, falls back to `openai`, so set Provider to `local` there.
- **Fetch Models** calls your server's `/models` endpoint with the node's current Provider, Base URL and key. Ollama exposes `/v1/models` as part of its OpenAI compatibility.
- The request goes through the `AllowLocal` SSRF policy, which permits loopback and private ranges for this field only. See [Security](security.md#ssrf-protection).
- **Context window.** Current Ollama documentation says the default context window is 4096 tokens, and that older versions used 2048. A long chat history or a big attached file can silently exceed it. Raise it with `OLLAMA_CONTEXT_LENGTH`, for example `OLLAMA_CONTEXT_LENGTH=8192 ollama serve`.
- **Timeout.** The node waits up to 3600 seconds by default (range 10 to 86400). Raise **Timeout (seconds)** for slow hardware or big models.
:::
