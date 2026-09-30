# Security

This page is Aerini's product security model: how the desktop app isolates itself, how credentials are encrypted, how outbound requests are checked against SSRF, what a `.wasm` plugin can and can't reach, and how to harden a server deployment. If you're deciding whether it's safe to expose a Webhook node to the internet or run `aerini-server` on a box other people can reach, this is the page for that decision. For the repository's vulnerability-reporting process instead, see the repo's root `SECURITY.md`, a different document with a similar name; [the docs home](../index.md#two-documents-both-called-security) explains the distinction.

## Desktop security model

The desktop app is built on Tauri: a native shell with an embedded webview for the UI, backed by Rust for anything that touches your filesystem, network, or credential store.

### The webview's own boundary

The UI you interact with runs inside that embedded webview, and it's locked down by a Content Security Policy that only allows scripts from the app itself, no inline or `eval`'d code. Its `connect-src` directive, the piece that governs any outbound fetch/XHR/WebSocket call the webview itself could make, permits the local IPC channel Aerini's own frontend and backend talk over, plus a set of AI-provider and OAuth domains (OpenAI, Anthropic, Google, Instagram, TikTok). Google Fonts is allowed separately, under `style-src`/`font-src`, for the one automatic connection described below; that's a different directive governing stylesheet and font loading, not `connect-src`'s outbound-request allowlist. In the current build, none of Aerini's actual outbound calls to the AI/OAuth providers happen through the webview's own network stack either: AI Prompt, AI Agent, and the OAuth flows all run from the Rust engine, which `connect-src` doesn't govern and doesn't need to, since it isn't a webview page. The webview's native capabilities are cut down from Tauri's raw defaults, not left wide open: window and webview introspection and control (position, size, focus, visibility, and similar; no ability to open new windows), the event bus, app metadata, and the four file/message dialogs. None of that reaches the filesystem, a shell, or the network directly, which is the boundary that actually matters here. Everything else Aerini does, running a workflow, reading a file, decrypting a credential, loading a plugin, happens behind a fixed set of named commands the frontend calls into, not through an open API surface the webview holds itself.

### The update check exception

Aerini makes exactly one automatic outbound connection with no workflow involved: loading the Inter font from Google Fonts over HTTPS, the same fact stated on [the docs home](../index.md) and in the [FAQ](../faq.md#does-aerini-collect-any-telemetry-or-send-data-anywhere-on-its-own). Checking for a new release is a second, separate network action, but not an automatic one. It only runs when you click Check for Updates in Settings, it asks GitHub's own Releases API for the latest tagged version, and it validates that GitHub's response actually points back to `github.com` before handing you a link. It never downloads or installs anything itself; getting the new version means installing it the same way you installed Aerini the first time, covered in [Installation](../getting-started/installation.md). There's no background poller, no scheduled check, and no auto-updater component built into the app at all.

### Code (JS) on the desktop app

By default, on the desktop app, a Code (JS) node runs with a full, unrestricted Node.js runtime: it can import `fs`, `child_process`, `net`, or anything else in the standard library, and nothing at the engine level stops it. The only guard is the one-time confirmation dialog and canvas badge shared by every dangerous node, covered in the [Nodes Reference](nodes.md#code-js) and not repeated here. That's a deliberate tradeoff for a single-user local app: you already control every workflow you build or import, so an engine-level sandbox would only add friction. That tradeoff changes once code you didn't write starts running unattended on a machine other people can reach, which is exactly the case the sandboxing described below exists for.

## Credential encryption

Covered in full in [Credentials](credentials.md); this is the one-paragraph summary. Every saved credential is encrypted with AES-256-GCM before it touches disk, and the encryption key itself lives in your OS keyring (Keychain, Credential Manager, or Secret Service), not in a plain file next to your workflows. Aerini has no account system and no server to send that key or your secrets to. The one gap worth knowing up front: only fields literally named `api_key` or `password` get the automatic "Use Saved Credential" picker, so a handful of fields on a few nodes are typed in directly and saved into the workflow file unencrypted. [Credentials §Using a credential in a node](credentials.md#using-a-credential-in-a-node) has the exact node-by-node breakdown, and [Credentials §Backing up your encryption key](credentials.md#backing-up-your-encryption-key) covers what happens if you lose the keyring entry.

Running unattended under `aerini-server` uses a different mechanism entirely, environment variables instead of the encrypted store, covered in [Credentials §Running headless](credentials.md#running-headless-aerini-server-and-environment-variables).

## SSRF protection

Server-side request forgery is what happens when something that makes requests on your behalf, an HTTP node, a database connection, an AI provider's base URL, gets pointed at an address it shouldn't reach: your own machine's internal services, another box on your LAN, or a cloud provider's metadata endpoint, which often hands out credentials to whoever asks it directly.

Every node whose target is a URL or connection string you supply, HTTP Request, Send Email's SMTP host, S3 Storage, AI Prompt/AI Agent's base URL, Database, and Image Generation's local-server option, checks that target before connecting. Two policies exist:

- **Strict (the default):** blocks loopback (`127.0.0.1`, `::1`), private ranges (`10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`), and `localhost` itself.
- **AllowLocal:** permits those same ranges, for the handful of nodes whose entire purpose is reaching a server on your own machine or LAN, where the address you type in is itself your explicit trust signal.

Under both policies, a fixed set of ranges is always blocked regardless of which one applies: link-local addresses (including the `169.254.169.254` cloud metadata address most providers use), the Azure IMDS endpoint specifically, RFC 6598 shared address space, and broadcast, multicast, unspecified, and documentation ranges. None of those are legitimate targets for a self-hosted server, so opting into AllowLocal doesn't reopen them.

| Node | Policy |
|---|---|
| HTTP Request | Strict |
| Send Email (SMTP host) | Strict |
| S3 Storage (AWS, R2) | Strict |
| S3 Storage (MinIO) | AllowLocal |
| Database (Postgres, MySQL, Redis) | Strict, unless its own `Allow Local` field is turned on |
| AI Prompt, AI Agent (`base_url`) | AllowLocal |
| Image Generation (Automatic1111, ComfyUI) | AllowLocal |
| Image Generation (Flux and other cloud providers) | Strict |
| `.wasm` plugin outbound HTTP | Strict, always, not configurable per plugin |

AllowLocal exists because a real, common use of Aerini is pointing AI Prompt at a locally running model server like Ollama, which listens on `localhost` by design. That's a legitimate target the strict policy would otherwise reject outright.

**The one limit this can't close:** the check above resolves a domain name and validates the result before connecting, but a small window exists between that check and the actual connection. A DNS server that returns a public address during the check and a private one moments later, DNS rebinding, can slip through an application-level check like this one; that limitation is unavoidable at this layer, not a bug specific to Aerini. If you're exposing anything that accepts outside input into a Database or HTTP Request node, the fix lives one layer down: a host-level egress firewall that blocks outbound connections to the same ranges regardless of what any application-layer check decides. A minimal `nftables` example:

```
table inet aerini_egress {
  chain output {
    type filter hook output priority 0; policy accept;
    ip daddr { 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, 127.0.0.0/8, 169.254.0.0/16 } drop
  }
}
```

That example covers IPv4 only; if your deployment also has IPv6 reachable, add an equivalent `ip6 daddr` rule for `::1/128` and `fc00::/7`, since Aerini's own check blocks both address families. Adjust to your own setup (a rule scoped to the `aerini-server` process or its container, rather than the whole host, is usually preferable), and keep `169.254.169.254` explicitly covered if your cloud provider uses a metadata endpoint outside the `169.254.0.0/16` block shown above. `aerini-server` warns on startup if it's bound to a non-loopback address without acknowledging this gap; see [Hardening a server deployment](#hardening-a-server-deployment) below for the flag that silences that warning once a firewall is actually in place.

## Dangerous nodes

Shell Command, Code (JS), Database, and every `.wasm` plugin node are treated as "dangerous": on the desktop app, that means a one-time confirmation dialog and a canvas warning badge, detailed in the [Nodes Reference](nodes.md). Running one of those nodes unattended (scheduled or webhook-triggered) skips that dialog entirely and only logs a warning, covered in [Background Runs §Unattended execution and dangerous nodes](background-runs.md#unattended-execution-and-dangerous-nodes). Neither of those is this page's job to restate; what belongs here is what changes once the same node types run under `aerini-server` instead of the desktop app.

On `aerini-server`, all three of Shell Command, Code (JS), and Database execution are **disabled by default**, in both `serve` and `api` mode. Each has its own opt-in flag, `--allow-shell`, `--allow-code`, `--allow-database`, and a workflow containing one of those node types simply fails that node at run time until the matching flag is passed. This is the inverse of the desktop app's posture: desktop trusts you by default since you're the only one who can reach it, while a server assumes it might be reachable by more than just you and asks you to opt back in per capability, one flag at a time, after you've actually reviewed what the workflow does. Database execution carries an extra reason to stay off by default beyond SSRF surface: `sqlx-mysql`'s RSA timing side-channel (RUSTSEC-2023-0071) is a real, tracked advisory, not a hypothetical.

`--allow-code` alone still leaves Code (JS) running the full, unrestricted Node.js runtime described above, matching desktop's default. Add `--code-sandbox` to change that: with it set, the ESM module loader blocks importing `fs`, `net`, `child_process`, `module`, `v8`, `inspector`, and similar built-ins, `eval` and `new Function` are disabled outright, and the global `fetch`/`WebSocket`/`XMLHttpRequest` objects are removed before your code runs (they're built-in globals in modern Node, so blocking the import path alone wouldn't catch them). `v8` and `inspector` are blocked for the same reason as the rest of the list even though neither is a filesystem or network module by name: `v8.writeHeapSnapshot()` writes a script-influenced file to an arbitrary path, and `inspector.open()` starts a debugger session that executes code without going through `eval`. `process.getBuiltinModule()` gets the same treatment as `fetch`: it's a `process` global that returns a builtin module — including `module`, whose `createRequire()` hands back a full CommonJS `require()` — without ever calling the ESM loader's resolve hook, so it's overridden with the same blocklist rather than left as an unguarded path to everything above. `process.binding()`, `process._linkedBinding()`, and `process.dlopen()` are removed outright rather than name-filtered: they hand back the same raw filesystem and process-spawning capability the blocklist above targets, but reach it through Node's internal binding layer instead of the module system, bypassing the loader hook entirely regardless of which module names it blocks. None of that is a byte-for-byte replacement for OS-level isolation on its own; it's paired with resource limits, but those limits are platform-dependent:

- **Linux:** the sandbox is fully enforced, including hard CPU and memory caps (`setrlimit`) on the subprocess, default 512 MB, configurable via `--max-code-memory-mb`.
- **macOS:** the module-import restrictions apply, but the CPU and memory caps don't; a runaway script can still exhaust host resources, bounded only by the node's own timeout (max 60 seconds). `aerini-server` refuses to start with `--allow-code --code-sandbox` on macOS unless you also pass `--i-acknowledge-partial-sandbox`, specifically so this gap can't be enabled silently.
- **Windows:** neither the module-import restrictions nor the resource caps are implemented. The server itself still starts; the refusal happens per node instead, the first time a workflow actually runs a Code (JS) node with `--code-sandbox` active. That node call fails outright rather than silently running unsandboxed, so a bad config surfaces at first use, not at server startup the way it does on macOS.

For a genuinely multi-user API deployment where `--allow-code` has to be on, Linux is the only platform where `--code-sandbox` provides a real boundary.

## The plugin (`.wasm`) sandbox boundary

Plugins are WebAssembly components, compiled against WASI Preview 2, loaded from a directory you point Aerini at (`plugin_dir` on the server, an install flow on desktop). Each execution gets a fresh sandbox instance, never reused between runs, so one failed or misbehaving execution can't leave state behind for the next. Inside that sandbox:

- **Memory** is capped at 64 MiB per execution.
- **CPU time** is capped at roughly 30 seconds of wall-clock execution, enforced independently of the node's own logic (a busy-loop gets killed, not just a slow network call).
- **Filesystem access is fully denied.** A plugin gets no preopened directories at all; any file operation it attempts fails.
- **Outbound HTTP is the only network access a plugin has**, and every request it makes is checked against the same Strict SSRF policy the Database and HTTP Request nodes enforce, described above. There's no per-plugin opt-out and no AllowLocal option for plugins.

What sandboxing here does **not** do: protect a credential from a plugin you've deliberately wired one into. If a workflow connects a saved credential to a plugin node's input, the plugin receives that credential's actual resolved value, the same as any built-in node would, not just an ID. The sandbox limits where a plugin's own code can reach on its own; it says nothing about what a plugin does with data your workflow hands it directly. Treat a `.wasm` plugin credential connection with the same judgment you'd apply to any other place a secret changes hands.

Publishers can optionally sign a plugin file with an Ed25519 signature (`<name>.wasm.sig`, checked against a BLAKE3 hash of the file), and the desktop app's plugin panel shows whether a signature verified. Today, this is purely informational: an unsigned plugin loads exactly the same as a signed one, and a failed or missing signature doesn't block loading. Don't treat "no warning shown" as equivalent to "this plugin was vetted"; it currently just means the sandbox limits above are all that stand between the plugin and your machine.

A `.wasm` plugin node is flagged "dangerous" in the UI alongside Shell Command, Code, and Database, the same confirmation dialog and badge described above.

**Trigger plugins**, a plugin that supplies its own long-running trigger instead of running once per execution, are the one exception to the CPU cap: since they're meant to stay alive listening for events indefinitely, no wall-clock deadline is applied to them, only to the one-shot `execute()` path above. The 64 MiB memory cap still applies. They get the same SSRF-filtered outbound HTTP as action plugins and no additional network grant beyond that.

## Hardening a server deployment

`aerini-server` has two modes, covered for setup in [Background Runs §Beyond the desktop app](background-runs.md#beyond-the-desktop-app) and in full in [Server Deployment](../operations/server-deploy.md): `serve` runs one workflow exported from the desktop app, `api` runs a full multi-workflow REST server. Both default to binding `127.0.0.1` only, both default every dangerous node type to disabled, and both print an explicit warning to the console if you change either of those defaults without acknowledging the tradeoff.

**`serve` mode** authenticates through an optional `run_secret`, set when you export the workflow from the desktop app and stored as an argon2id hash in `aerini-server.json`. Without one set, the status page (workflow name, run counts, timestamps) is still visible to anyone who can reach the port, though the Run Now button and authenticated log endpoints stay disabled either way. If that config file was exported before Aerini 0.3, its secret may still be a legacy BLAKE3 hash, which the server refuses to start with by default (BLAKE3 is fast, not brute-force resistant) unless you pass `--allow-legacy-run-secret`; re-exporting from the desktop app upgrades it to argon2id instead. On Unix systems, the server also warns if that config file is readable by anyone but its owner, since it holds that hash; `chmod 600` it.

**`api` mode** authenticates every request with a bearer token, randomly generated on first run if you don't set one, and supports additional scoped tokens limited to `read`, `write`, or `admin`, so an integration you hand a token to can't do more than it needs. A token can also be confined to specific workflows, in which case it cannot read, run, change or delete any other workflow, or touch the server-wide credentials; see [Per-token workflow ACL](../operations/server-api-reference.md#per-token-workflow-acl). Local origins are always allowed to call the API; anything else needs an explicit `--allow-origin`.

A short list of flags matters most for an actual exposure decision, beyond the dangerous-node and code-sandbox flags already covered above:

- **`--bind`**: stays `127.0.0.1` until you explicitly set it to something reachable from outside the machine. Don't change this before the rest of this list is in place.
- **`--trusted-proxy-count`**: if you're behind a reverse proxy (nginx, Caddy, a load balancer), this has to match the number of proxy hops exactly, or per-IP rate limiting reads the proxy's address for every client instead of the real one.
- **`--file-sandbox-dir`**: without it, a File node can read or write any path the server process itself can reach. Setting it restricts every File node in every workflow to that one directory tree. Recommended for anything other than a fully trusted single-user box.
- **`--ssrf-firewall-acknowledged`**: suppresses the startup warning about the DNS-rebinding gap described above. Pass it only after the egress firewall rule is actually in place, not to make the warning go away.

The full flag reference, every option for both subcommands, lives in [Server CLI Reference](../operations/server-cli-reference.md); this section covers the ones that change your actual exposure, not the complete list.

## See also

- [Credentials](credentials.md), for the full encryption, key-backup, and headless-credential model
- [Nodes Reference](nodes.md), for the dangerous-node confirmation dialog and every node's own fields
- [Background Runs](background-runs.md), for how unattended runs interact with dangerous nodes and for exporting a workflow to `aerini-server`
- [Troubleshooting](../troubleshooting.md), for when a node fails against a blocked SSRF target rather than a question about the policy itself
- [FAQ](../faq.md), for the short version of the no-telemetry and update-check answers
