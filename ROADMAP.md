# Roadmap

This is a living document. Items move as priorities and capacity change.

---

## v0.2 — Parallel execution and hardening (shipped — v0.2.0)

- [x] Parallel execution (per-workflow opt-in)
- [x] Global concurrent run limit (`--max-concurrent-runs`)
- [x] Per-workflow SSE ACL
- [x] Code node sandbox (filesystem restriction + subprocess restriction)

---

## v0.3 — Extensibility (shipped — v0.3.0)

- [x] Plugin / custom node API
- [x] Pre-built binary releases for macOS, Windows, Linux

---

## v0.4 — Versioning and backups (shipped — v0.4.0)

- [x] Workflow versioning and history
- [x] Improved run history UI
- [x] Bulk workflow export
- [x] Multi-node plugin packages (`.aerinipkg`)

---

## v0.4.2 — Workflow organization and in-app updates (shipped — v0.4.2)

- [x] In-app updates (check, verify signature, install and restart)
- [x] Workflows sidebar: tags, tag filter and per-row actions menu
- [x] Canvas undo / redo and one-click wire disconnect
- [x] Chat panel file attachments saved with the session
- [x] Trigger plugins can report metadata and their next event

---

## v0.4.3 — Agent access (shipped — v0.4.3)

- [x] MCP server (`aerini-mcp`): an AI agent can read the node catalog, check and write workflows, and run them
- [x] `aerini-server api`: `GET /api/nodes` and `POST /api/validate-workflow`

---

## v0.4.4 — Canvas polish

- [ ] Improved minimap

---

## v0.4.5 — AI node hardening

- [ ] Share one request and response implementation between AI Prompt and AI Agent, and give AI Prompt the same structured output (response schema) and effort settings the AI Agent has; existing AI Prompt outputs and saved workflows stay unchanged
- [ ] Model picker hint for Ollama: show whether the selected local model supports tools

---

## v0.5 — Sub-workflows

- [ ] Sub-workflows (in progress)
- [ ] AI Agent runs its own tool loop on top of sub-workflows, with caps, per-tool credentials and human approval (design document first)
- [ ] AI Agent can use tools from external MCP servers (after the tool loop)
- [ ] Review OpenAI's Responses API and hosted tools for the AI nodes
- [ ] More trigger types (database poll)
- [ ] Workflow bundle import

---

## v1.0 — Stable release

- Stable IPC and serde wire contracts (semver enforced from here)
- Signed binaries on all three platforms
- Plugin API finalised and documented
- Engine execution paths fully tested

---

## Long-term

- [ ] Distributed execution across multiple machines
- [ ] Workflow marketplace
- [ ] SDK for embedding the Aerini engine in other applications
- [ ] Enterprise features (SSO, audit log, team workspaces)

---

## Experimental

- [ ] Workflow synthesis from natural language description
- [ ] Automatic retry strategy suggestions based on run history

---

## Won't do

- Built-in tunnel node (use cloudflared or ngrok — see docs/webhooks-public.md)
- Hosted cloud version (local-first is the design)
- Metric-driven contribution rewards
