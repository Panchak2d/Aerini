# Roadmap

This is a living document. Items move as priorities and capacity change.

---

## v0.2 — Parallel execution and hardening (active)

- [ ] Parallel execution (per-workflow opt-in)
- [ ] Global concurrent run limit (`--max-concurrent-runs`)
- [ ] Per-workflow SSE ACL
- [ ] Code node sandbox (filesystem restriction + subprocess restriction)
- [ ] Pre-built binary releases for macOS, Windows, Linux

---

## v0.3 — Extensibility

- [ ] Plugin / custom node API
- [ ] Workflow versioning and history
- [ ] Improved run history UI
- [ ] More trigger types (file watch, database poll)
- [ ] Export / import workflow bundles

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
