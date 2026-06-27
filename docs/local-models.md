# Local models (Ollama and other OpenAI-compatible servers)

The **AI Prompt** node talks to any OpenAI-compatible chat completions
endpoint by leaving `provider` on `auto` and pointing `base_url` at it. That
covers locally-run model servers like Ollama and LM Studio, not just hosted
providers.

## Known limitation — read before configuring Ollama

**Verified against the current codebase (`aerini-engine/src/nodes/ai_prompt.rs`,
`aerini-engine/src/nodes/util.rs`):** the AI Prompt node's SSRF check
(`check_host_ssrf_from_url`) blocks `localhost` and any private/loopback
address **unconditionally** — not just in server mode, despite what the
node's own `base_url` field description currently says. A `base_url` of
`http://localhost:11434/v1` (or `http://127.0.0.1:11434/v1`, or any
`192.168.x.x` / `10.x.x.x` address) is rejected immediately with:

```
SSRF_BLOCKED: Requests to private/internal IP addresses are not permitted (...)
```

This means **Ollama does not currently work with the AI Prompt node**,
in either desktop or server mode, for the standard local setup — even
though the node's own description text advertises "a local Ollama model"
as a supported case. The engine already has a precedent for this exact
problem: `image_gen.rs`'s A1111 and ComfyUI backends use
`check_host_ssrf_from_url_allow_local` instead of the strict check,
specifically because those are local-only tools. `ai_prompt.rs` (and
`ai_agent.rs`, which has the identical check) do not use that variant.

This is a code-level bug, not a docs gap, and fixing it is out of scope for
this documentation patch (Rule 6 — flagged, not silently fixed; see this
patch's `MANIFEST.txt`). **Recommended fix for a follow-up patch:** switch
`ai_prompt.rs` and `ai_agent.rs` to `check_host_ssrf_from_url_allow_local`,
mirroring the `image_gen.rs` precedent.

The setup steps below describe the **intended** configuration — they will
work as soon as that fix ships. Until then, the only way to reach a local
Ollama instance from the AI Prompt node is to put it behind a
public(-ish) hostname that doesn't resolve to a private/loopback address,
which defeats the point of running it locally for most setups.

---

## Intended setup (once the SSRF fix above ships)

1. Install and run [Ollama](https://ollama.com), then pull a model:
   ```bash
   ollama pull llama3
   ```
   Ollama serves an OpenAI-compatible API on port `11434` by default.

2. In the AI Prompt node, configure:

   | Field | Value |
   |---|---|
   | `provider` | `auto` (or `openai` explicitly) |
   | `base_url` | `http://localhost:11434/v1` |
   | `model` | the model name you pulled, e.g. `llama3` |
   | `api_key` | leave blank — Ollama doesn't require one |

3. `provider: auto` detects an OpenAI-compatible target whenever `base_url`
   isn't an Anthropic or Gemini endpoint, so Ollama, LM Studio, and similar
   local servers all route through the same OpenAI-compatible request path.

Other OpenAI-compatible local servers (LM Studio, vLLM's OpenAI server,
text-generation-webui's OpenAI extension, etc.) follow the same pattern —
point `base_url` at whatever port that server exposes, with `/v1` appended
if its API lives under that path.

## What doesn't change

Local models go through the exact same node — no separate "Ollama node"
exists or is planned. `temperature`, `max_tokens`, `system`, and
`rate_limit_rpm` all behave the same as they do for hosted providers, except
that token/cost ceilings are whatever your local server enforces, not a
billing limit.
