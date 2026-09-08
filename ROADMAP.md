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

## v0.5 — Sub-workflows

- [ ] Sub-workflows (in progress)
- [ ] More trigger types (file watch, database poll)
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
