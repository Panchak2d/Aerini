# Local models (Ollama and other OpenAI-compatible servers)

The **AI Prompt** and **AI Agent** nodes talk to any OpenAI-compatible chat
completions endpoint by leaving `provider` on `auto` and pointing `base_url`
at it. That covers locally-run model servers like Ollama and LM Studio, not
just hosted providers.

**Verified against the current codebase** (`aerini-engine/src/nodes/ai_prompt/mod.rs`,
`aerini-engine/src/nodes/ai_agent.rs`): both nodes' SSRF check runs under
`SsrfPolicy::AllowLocal`, so `localhost`, `127.0.0.1`, and private-range
addresses (`192.168.x.x`, `10.x.x.x`) are permitted — same precedent as the
A1111/ComfyUI backends in `image_gen.rs`. A `base_url` of
`http://localhost:11434/v1` works in both desktop and server mode.

## Setup

1. Install and run [Ollama](https://ollama.com), then pull a model:
   ```bash
   ollama pull llama3
   ```
   Ollama serves an OpenAI-compatible API on port `11434` by default.

2. In the AI Prompt (or AI Agent) node, configure:

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
