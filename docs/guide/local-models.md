# Local Models

AI Prompt and AI Agent don't require a hosted API. Point either node's Base URL at a server running on your own machine or network instead, such as Ollama, and the request never leaves your hardware. This page covers what to actually type into those fields to make that connection work, and the handful of ways it fails when something's off.

If you haven't read [Nodes Reference](nodes.md), [Credentials](credentials.md), and [Security](security.md) yet, read those first. This page assumes you already know what AI Prompt's fields do, how the credential store and its Advanced metadata work, and why loopback and private-network addresses are allowed here at all. None of that gets repeated below.

## Why this is allowed

Every other node that takes a URL blocks loopback and private-network addresses by default. AI Prompt and AI Agent's `base_url` field is one of the few exceptions: it runs under the `AllowLocal` SSRF policy instead of the default `Strict` one, specifically so a `base_url` like `http://localhost:11434` doesn't get rejected before the request is even sent. [Security §SSRF protection](security.md#ssrf-protection) covers the full policy and the ranges that stay blocked even under `AllowLocal`. This page just uses that door, it doesn't reopen it.

## Before you start

Install Ollama, or any other server that speaks the OpenAI chat completions format, and have it running. Pull at least one model:

```
ollama pull llama3.2
```

Ollama listens on `http://localhost:11434` by default and doesn't check for an API key. Everything below assumes that default port; adjust it if you've changed it.

## Pointing AI Prompt at it

Add an AI Prompt node and set:

- **Provider**: leave it at `auto`, or set it to `openai` directly. Aerini has no separate "Ollama" provider entry; an OpenAI-compatible base URL is all `auto` needs, since detection only special-cases Anthropic's and Google's own domains and falls back to `openai` for everything else, `localhost` included.
- **Base URL**: `http://localhost:11434/v1`. Both the scheme and the `/v1` matter. Ollama's OpenAI-compatible endpoint lives under `/v1/chat/completions`, and Aerini sends the request to exactly whatever you type here plus `/chat/completions`, nothing smarter.
- **Model**: the exact tag you pulled, e.g. `llama3.2`. Leave this blank and the node quietly falls back to `gpt-5.6`, a model your local Ollama has never heard of.
- **API Key**: leave it empty. Aerini only adds an `Authorization` header when this field is non-empty, and Ollama doesn't check for one by default.

Run the node. A working setup returns a reply in `content`, the same shape you'd get back from OpenAI or Claude.

## Reusing this setup with a saved credential

Typing the same base URL and model into every node gets tedious fast. [Credentials §Adding a credential](credentials.md#adding-a-credential) and [§AI nodes: auto-fill and one-off keys](credentials.md#ai-nodes-auto-fill-and-one-off-keys) cover the general mechanism; here's what to put in each field for Ollama specifically.

Open the Credentials panel and add one:

- **Type**: API Key. It barely matters here, since nothing about the value gets checked; it just decides how the credential is labeled in the list.
- **Name**: something you'll recognize, like "Ollama Local".
- **Secret Value**: the form won't save a credential without one, even though Ollama itself never reads it. Type anything, like `not-needed`. It gets sent as a Bearer token the same way a real key would be, and Ollama ignores it the same way it ignores any other value here.
- **Advanced**: set **Provider** to `openai` (not `ollama`, since no such provider id is registered), **Model** to your pulled tag, and **Base URL** to `http://localhost:11434/v1`.

Save it. Now, on any AI Prompt or AI Agent node, pick this credential from **Use Saved Credential** and Aerini fills provider, model, and base URL in for you, but only into whichever of those three fields are still blank on that node. Anything you've already typed into one of them stays untouched.

## AI Agent

AI Agent's `base_url`, `api_key`, and provider detection work identically to AI Prompt's: same `AllowLocal` policy, same base-URL fallback, same silent `gpt-5.6` default if Model is left blank. Everything above applies without changes.

One difference worth knowing: AI Agent's **Provider** field is required rather than optional, so you have to actually pick a value from the dropdown instead of leaving it unset. Picking `auto` still resolves exactly the same way it does on AI Prompt, it just has to be picked rather than assumed.

Also worth checking before wiring the **Tools** field up against a local model: tool-calling depends on the specific model you've pulled, not just the provider. Some model families implement it and some don't, regardless of size, so a model with no tool-calling template will either refuse the request or ignore the tools array outright. Check whether the model you're running documents tool-calling support before building a workflow around it.

## Reaching a model server on your LAN, not just this machine

`AllowLocal` covers more than loopback. It also permits private-network ranges (`10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`), so a base URL like `http://192.168.1.50:11434/v1` aimed at another machine on your network passes the same check `http://localhost:11434/v1` does.

Getting there also depends on the server itself. Ollama listens only on `127.0.0.1` by default and refuses connections from anywhere but the machine it's running on; reaching it from another device means setting `OLLAMA_HOST` to `0.0.0.0` (or a specific interface) on that machine first, and opening the port in its firewall. That's Ollama's own configuration, not Aerini's, and it's worth doing deliberately rather than by accident. Anything that can reach that port can run inference and pull or delete models, with no login involved.

## If it's not working

- **The node fails immediately with a network error**: nothing is listening at that address and port. Confirm the server is actually running and that the port in Base URL matches what it's bound to.
- **"URL scheme 'localhost' is not permitted"**, or similar: the `http://` is missing from the front of Base URL. Something like `localhost:11434` on its own parses as a URL whose scheme is `localhost`, not as a host and port, so Aerini rejects it before it ever tries to connect. Add `http://` to the front.
- **The reply comes back empty, or complains about a model it can't find**: Model doesn't match a tag you've actually pulled. Leaving Model blank is the most common cause, since it silently sends `gpt-5.6` instead. Check the exact tag against what your server reports it has installed.
- **The request seems to reach real OpenAI, or asks for billing**: Base URL was left blank somewhere along the way. An empty Base URL resolves to OpenAI's own API, not to whatever's running locally, regardless of what Provider is set to.

## See also

- [Nodes Reference](nodes.md#ai-prompt), for every other field on AI Prompt and AI Agent
- [Credentials](credentials.md#ai-nodes-auto-fill-and-one-off-keys), for the full saved-credential auto-fill mechanism
- [Security](security.md#ssrf-protection), for the complete SSRF policy and what stays blocked even under `AllowLocal`
