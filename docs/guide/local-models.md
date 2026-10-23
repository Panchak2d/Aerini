# Local Models

AI Prompt and AI Agent don't require a hosted API. Point either node's Base URL at a server running on your own machine or network instead, such as Ollama, and the request never leaves your hardware. This is the reference page: what to type in each field, how Aerini decides a server is local, and what goes wrong. To get a first reply working, start with the [Local Models Quickstart](local-models-quickstart.md). To chat with a local model from inside Aerini, see [Chat with a Local Model](chat-with-a-local-model.md).

> [!NOTE]
> **Aerini doesn't download models.** Ollama does that. Aerini's **Fetch Models** button reads the list of models your server already has installed, so a model shows up there only after you've pulled it. See [How models get onto your machine](#how-models-get-onto-your-machine).

If you haven't read [Nodes Reference](nodes.md), [Credentials](credentials.md), and [Security](security.md) yet, read those first. This page assumes you already know what AI Prompt's fields do, how the credential store and its Advanced metadata work, and why loopback and private-network addresses are allowed here at all. None of that gets repeated below.

**Where to start**

| You want to... | Read |
|---|---|
| Get a first reply from a local model | [Local Models Quickstart](local-models-quickstart.md) |
| Know what to type in each field | [Pointing AI Prompt at it](#pointing-ai-prompt-at-it) |
| See which models you have, or understand Fetch Models | [Finding available models](#finding-available-models) |
| Stop retyping the same URL and model | [Reusing this setup with a saved credential](#reusing-this-setup-with-a-saved-credential) |
| Use tools with a local model | [AI Agent](#ai-agent) |
| Reach Ollama on another computer | [Reaching a model server on your LAN](#reaching-a-model-server-on-your-lan-not-just-this-machine) |
| Fix an error | [If it's not working](#if-it-s-not-working) |

## Before you start

Install [Ollama](https://ollama.com/download), or any other server that speaks the OpenAI chat completions format, and have it running. Pull at least one model:

```
ollama pull llama3.2
```

Ollama listens on `http://localhost:11434` by default and doesn't check for an API key. Everything below assumes that default port; adjust it if you've changed it.

## How models get onto your machine

Ollama owns the models: it downloads them, stores them, loads them into memory and runs them. Aerini only talks to it.

- **Get a model.** Click **Pull model** on an AI node (see [Pulling and deleting models](#pulling-and-deleting-models)), or run `ollama pull <name>` in a terminal, for example `ollama pull llama3.2`. Ollama's own documentation says to pull a model before using it.
- **See what you have.** Run `ollama list`, or click **Fetch Models** on an AI node. Both show what's installed. If you pull a model in a terminal while the node's settings are open, click **Fetch Models** again to refresh the list. Pulling or deleting with the buttons in Aerini refreshes it for you.
- **Where they live.** By default Ollama stores models in `~/.ollama/models` on macOS, `/usr/share/ollama/.ollama/models` on Linux, and `C:\Users\<username>\.ollama\models` on Windows. Models are large, so check your free disk space before pulling several.
- **How long they stay loaded.** Ollama keeps a model in memory for about five minutes after its last request by default. The first request after that is slower because the model has to load again. `ollama ps` shows what's loaded right now.

Aerini only downloads a model when you click **Pull model**. A model that isn't installed can't be selected from Fetch Models and will fail when the node runs, so pull it first.

## Pointing AI Prompt at it

Add an AI Prompt node and set:

- **Provider**: leave it at `auto`, or set it to `local` directly. Aerini has a `local` provider entry — a wire-identical twin of `openai`'s, meant for Ollama, LM Studio, vLLM, llama.cpp, or anything else speaking the OpenAI chat-completions format. `auto` recognizes the literal hostname `localhost` and any loopback or private-network IP (e.g. `127.0.0.1`, `192.168.1.50`) and resolves to `local` automatically, no extra click needed. The one gap: this check runs on the URL text itself, not DNS, so a hostname that needs a lookup to resolve to a local address (e.g. `myollama.local`) isn't recognized and `auto` falls back to `openai` instead — set Provider to `local` explicitly in that case. Setting Provider to `openai` by hand still works too (same wire behavior), but skips the blank-Model safeguard covered below, so prefer `local` or `auto`.
- **Base URL**: `http://localhost:11434/v1`. Both the scheme and the `/v1` matter. Ollama's OpenAI-compatible endpoint lives under `/v1/chat/completions`, and Aerini sends the request to exactly whatever you type here plus `/chat/completions`, nothing smarter. Once Provider is set to `local`, a **Use Ollama defaults** button next to the field fills this in for you; any other local server still just needs its own URL typed in.
- **Model**: the exact tag you pulled, e.g. `llama3.2`. Type it directly, or use **Fetch Models** next to the field to pull a live list from your server (see [Finding available models](#finding-available-models) below). Leaving this blank fails immediately with a `MISSING_MODEL` error whenever Provider is `local` or `auto` resolves to it, instead of silently sending a model your server has never heard of. The one case that isn't caught: `auto` pointed at a hostname it can't recognize as local (see the Provider note above) still falls back to `openai`'s default, `gpt-5.6`, on a blank Model.
- **API Key**: leave it empty. Aerini only adds an `Authorization` header when this field is non-empty, and Ollama doesn't check for one by default.

Run the node. A working setup returns a reply in `content`, the same shape you'd get back from OpenAI or Claude.

## Finding available models

Instead of typing a tag by hand, click **Fetch Models** next to the Model field. It queries your server's `/models` endpoint using whatever Provider, Base URL, and API Key are currently set on the node — `local` and `auto` (once it's resolved to `local`) both work, using the same detection the node itself uses at run time. A successful fetch offers a dropdown of everything your server reports having pulled; pick one, or keep typing directly, since the plain text field is always still there underneath and typing never stops working.

**Fetch Models only lists; it never downloads anything.** A model that isn't installed won't be in the list; pull it with **Pull model** or `ollama pull` and click the button again.

If the fetch fails — nothing listening at that Base URL, or the server doesn't expose `/models` — the field is left exactly as it was. The button's own label gives brief feedback, and the specific reason (no key, a bad key, nothing listening at that address, etc.) is shown underneath it.

If a saved credential is selected on the node, its Secret Value is used as the API Key for the fetch; otherwise a one-off key typed directly into the node is used, the same precedence the node applies at run time. Ollama and other local servers ignore this value either way.

## Pulling and deleting models

In the desktop app, **Pull model** and **Delete model** buttons appear under **Fetch Models** on AI Prompt and AI Agent, and in the Advanced section of the Credentials panel. Aerini shows them only after the server at your Base URL answers like Ollama, so they stay hidden for LM Studio, vLLM, llama.cpp and cloud providers.

- **Pull.** Type the model name in **Model**, for example `llama3.2` or `llama3.2:3b`, then click **Pull model**. A bar shows progress, speed and time left, and **Cancel** stops the download. When it finishes, the model list refreshes. Only plain Ollama library names are accepted (`name`, `name:tag` or `namespace/name`); names that point at another registry, such as `hf.co/...`, aren't supported, so use `ollama pull` for those.
- **Closing the panel stops the download.** Closing the node's settings or the Credentials panel cancels a pull in progress, and so does anything that reopens them, such as changing Provider on the node or saving or editing a credential. Pull again to continue where it left off. Ollama normally resumes from the partial download; if the server was restarted in between, it may start over.
- **Delete.** Type the model name, click **Delete model** and confirm. Aerini removes it from the server and frees its disk space. You can pull it again later.
- **A server on another machine.** If the Base URL isn't this computer, Aerini names the server and the model and asks you to confirm before it downloads anything, because the download uses that machine's disk and network.
- **Only when you click.** Aerini never pulls or deletes a model while a workflow runs. A model that isn't installed still fails the node with a clear error.

A saved credential or a key typed into the node is sent with these requests the same way it is for Fetch Models. Models are large, so check your free disk space first: if the disk fills up, the pull fails partway and the error is shown under the buttons.

## Reusing this setup with a saved credential

Typing the same base URL and model into every node gets tedious fast. [Credentials §Adding a credential](credentials.md#adding-a-credential) and [§AI nodes: auto-fill and one-off keys](credentials.md#ai-nodes-auto-fill-and-one-off-keys) cover the general mechanism; here's what to put in each field for Ollama specifically.

Open the Credentials panel and add one:

- **Type**: API Key. It barely matters here, since nothing about the value gets checked; it just decides how the credential is labeled in the list.
- **Name**: something you'll recognize, like "Ollama Local".
- **Secret Value**: optional, as long as Advanced Provider (below) is set to `local` — leave it blank and Aerini simply won't send an `Authorization` header, which is exactly what a default Ollama install expects. With any other Advanced Provider value, including `openai`, the form still requires something here even though Ollama itself never reads it; type anything, like `not-needed`, and it's sent as a Bearer token that gets ignored the same way any other value would.
- **Advanced**: set **Provider** to `local` (not `ollama`, since no such provider id is registered, and not `openai`, which would require a Secret Value above and skip the blank-Model safeguard on the node itself), **Model** to your pulled tag, and **Base URL** to `http://localhost:11434/v1`.

Save it. Now, on any AI Prompt or AI Agent node, pick this credential from **Use Saved Credential** and Aerini fills provider, model, and base URL in for you, but only into whichever of those three fields are still blank on that node. Anything you've already typed into one of them stays untouched.

## AI Agent

AI Agent's `base_url`, `api_key`, and provider detection work identically to AI Prompt's: same `AllowLocal` policy, same base-URL fallback, same `MISSING_MODEL` hard error if Model is left blank while Provider is `local` or `auto` resolves to it. Everything above, including [Finding available models](#finding-available-models), applies without changes.

One difference worth knowing: AI Agent's **Provider** field is required rather than optional, so you have to actually pick a value from the dropdown instead of leaving it unset. Picking `auto` still resolves exactly the same way it does on AI Prompt, it just has to be picked rather than assumed.

Also worth checking before wiring the **Tools** field up against a local model: tool-calling depends on the specific model you've pulled, not just the provider. Some model families implement it and some don't, regardless of size, so a model with no tool-calling template will either refuse the request or ignore the tools array outright. Check whether the model you're running documents tool-calling support before building a workflow around it.

## Reaching a model server on your LAN, not just this machine

`AllowLocal` covers more than loopback. It also permits private-network ranges (`10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`), so a base URL like `http://192.168.1.50:11434/v1` aimed at another machine on your network passes the same check `http://localhost:11434/v1` does.

Getting there also depends on the server itself. Ollama listens only on `127.0.0.1` by default and refuses connections from anywhere but the machine it's running on; reaching it from another device means setting `OLLAMA_HOST` to `0.0.0.0` (or a specific interface) on that machine first, and opening the port in its firewall. That's Ollama's own configuration, not Aerini's, and it's worth doing deliberately rather than by accident. Anything that can reach that port can run inference and pull or delete models, with no login involved.

## If it's not working

| What you see | What it means and what to do |
|---|---|
| The node fails immediately with a network error | Nothing is listening at that address and port. Confirm the server is actually running and that the port in Base URL matches what it's bound to. |
| "URL scheme 'localhost' is not permitted", or similar | The `http://` is missing from the front of Base URL. Something like `localhost:11434` on its own parses as a URL whose scheme is `localhost`, not as a host and port, so Aerini rejects it before it ever tries to connect. Add `http://` to the front. |
| 'model is required when provider is "local"' | Model was left blank with Provider set to (or `auto`-detected as) `local`. This is a deliberate hard error, not a bug: fill in the tag you pulled, or use **Fetch Models**. |
| Fetch Models returns an empty list | The server is reachable but has no models installed. Click **Pull model** (or run `ollama pull <name>`), then click **Fetch Models** again. |
| Pull model and Delete model don't appear | The server at Base URL didn't answer like Ollama, or Provider isn't `local` or `auto`, or Base URL is blank. Check that Ollama is running and that Base URL ends in `/v1`. |
| The reply comes back empty, or complains about a model it can't find | Model doesn't match a tag you've actually pulled. Check the exact tag against `ollama list`, or use **Fetch Models** to see the live list. A blank Model fails immediately with the error above rather than silently misfiring, except when `auto` is pointed at a hostname it can't recognize as local (see [Pointing AI Prompt at it](#pointing-ai-prompt-at-it)); that case still falls back to `gpt-5.6` without warning. |
| The request seems to reach real OpenAI, or asks for billing | Base URL was left blank while Provider was `auto` (the default) or some other unrecognized value. Both fall back to OpenAI's own default endpoint, same as leaving Provider unset entirely on a plain OpenAI setup. This doesn't happen with Provider explicitly set to `local`: a blank Base URL there fails immediately instead, since `local` has no cloud default to fall back to. |
| Long conversations start forgetting earlier turns | The model's context window is full. Current Ollama documentation gives a default of 4096 tokens, and older versions used 2048. Raise it with the `OLLAMA_CONTEXT_LENGTH` environment variable, for example `OLLAMA_CONTEXT_LENGTH=8192 ollama serve`, or send fewer earlier messages. |

## Why this is allowed

Every other node that takes a URL blocks loopback and private-network addresses by default. AI Prompt and AI Agent's `base_url` field is one of the few exceptions: it runs under the `AllowLocal` SSRF policy instead of the default `Strict` one, specifically so a `base_url` like `http://localhost:11434` doesn't get rejected before the request is even sent. [Security §SSRF protection](security.md#ssrf-protection) covers the full policy and the ranges that stay blocked even under `AllowLocal`. This page just uses that door, it doesn't reopen it.

## See also

- [Local Models Quickstart](local-models-quickstart.md) and [Chat with a Local Model](chat-with-a-local-model.md), the step-by-step versions of this page
- [Nodes Reference](nodes.md#ai-prompt), for every other field on AI Prompt and AI Agent
- [Credentials](credentials.md#ai-nodes-auto-fill-and-one-off-keys), for the full saved-credential auto-fill mechanism
- [Security](security.md#ssrf-protection), for the complete SSRF policy and what stays blocked even under `AllowLocal`
