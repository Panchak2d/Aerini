# Server CLI Reference

Every flag `aerini-server` accepts, for both the `serve` and `api` subcommands, plus the control subcommands (`list`, `stop`, `start`, `restart`, `status`, `tokens`). For what each mode is for and how to get one running, see [Server Deployment](server-deploy.md). For the reasoning behind the security-relevant defaults, see [Security](../guide/security.md).

The whole binary accepts `--version`; every subcommand accepts `--help` for this same information printed from the binary itself.

## `serve`

Runs one workflow exported from the desktop app (**Export → Export for Server**).

| Flag | Env var | Default | Notes |
|---|---|---|---|
| `--config <path>` | | `aerini-server.json` | Path to the config file produced by the export. |
| `--port <port>` | | none | Overrides the status page port set in `aerini-server.json`. Leave unset to use that file's own port. |
| `--bind <address>` | `AERINI_BIND` | `127.0.0.1` | Interface the status page binds to. Set to `0.0.0.0` to expose it beyond localhost. |
| `--trusted-proxy-count <n>` | `AERINI_TRUSTED_PROXY_COUNT` | `0` | Reverse-proxy hops to trust when reading `X-Forwarded-For`. Must match your proxy topology exactly, too high a value lets a client spoof its IP and dodge per-IP rate limiting. |
| `--allow-shell` | | `false` | Enables Shell Command nodes. If the workflow has one and this is left unset, the server prints a startup warning instead of refusing to start. |
| `--allow-code` | | `false` | Enables Code (JS) nodes. Same startup-warning behavior as `--allow-shell` if the workflow has one and this is unset. |
| `--allow-database` | | `false` | Enables Database nodes (PostgreSQL, MySQL, SQLite, Redis). Same startup-warning behavior if the workflow has one and this is unset. |
| `--allow-legacy-run-secret` | | `false` | Lets the server start when the config's `run_secret` is a legacy BLAKE3 hash instead of argon2id. Without it, a legacy hash makes the server refuse to start. `serve`-only, since only `serve` has a `run_secret` at all. |
| `--file-sandbox-dir <dir>` | | none | Restricts File node reads and writes to this directory tree. If the workflow has a File node and this is left unset, the server prints a startup warning and File nodes can reach any path the process can. `api` mode handles the unset case differently, see below. |
| `--i-acknowledge-partial-sandbox` | | `false` | `serve`-only. Required to start with `--allow-code --code-sandbox` on macOS, see [Platform behavior of `--code-sandbox`](#platform-behavior-of---code-sandbox) below. |
| `--ssrf-firewall-acknowledged` | | `false` | Suppresses the startup warning about the [SSRF](../glossary.md#ssrf) DNS-rebinding gap when bound to a non-loopback address. Pass it only once a host-level egress firewall is actually in place. |
| `--parallel-execution` | | `false` | Runs independent workflow branches concurrently instead of sequentially. |
| `--max-concurrent-nodes <n>` | | `8` | Caps simultaneous nodes when `--parallel-execution` is set; ignored otherwise. A value below `1` is clamped to `1`, not rejected. |
| `--max-workflow-duration-secs <secs>` | | none (no limit) | Server-level ceiling on one workflow execution, applied as `min(the workflow's own duration setting, this value)` when the workflow has one. A value outside 10 to 86400 (24 hours) is clamped to that range rather than rejected. |
| `--code-sandbox` | | `false` | Applies ESM import restrictions to Code (JS) nodes, plus CPU/memory limits on Linux. See [Platform behavior of `--code-sandbox`](#platform-behavior-of---code-sandbox) below. |
| `--max-code-memory-mb <mb>` | | `512` (Linux only) | Memory ceiling for a Code node's Node.js subprocess. Only takes effect with `--allow-code --code-sandbox` on Linux; no effect on macOS or Windows. |
| `--plugin-dir <dir>` | | none | Directory of `.wasm` plugin nodes to load. A file that fails to load is skipped with a warning. `serve` reloads this directory on `SIGHUP` without restarting the process. |

## `api`

Runs the multi-workflow REST API, with bearer-token auth and no export package or config file, everything here is a flag or an environment variable.

| Flag | Env var | Default | Notes |
|---|---|---|---|
| `--token <token>` | `AERINI_TOKEN` | auto-generated | Bearer token for API auth. If you set one, it must be at least 32 characters, the server exits with an error otherwise. Left unset, the server generates a 43-character token, prints it once to stderr, and grants it all three [scopes](../glossary.md#scoped-token) (`read`, `write`, `admin`). |
| `--port <port>` | `AERINI_PORT` | `7700` | Port to listen on. Unlike `serve`'s `--port`, this one is env-backed and always has a default. |
| `--data-dir <dir>` | `AERINI_DATA_DIR` | `~/.aerini-server` | Directory for the SQLite database and the token/credential key file. `~` or a leading `~/` expands to your home directory (`$HOME`, or `USERPROFILE` on Windows when `HOME` is unset); `~name/...` is used as written. The server exits with an error if it needs a home directory and can't find one. |
| `--allow-origin <list>` | | none | Comma-separated additional origins allowed to call the API. Localhost origins (`http://localhost[:port]`, `http://127.0.0.1[:port]`, `http://[::1][:port]`) are always allowed regardless of this flag. |
| `--allow-env-vars <list>` | | none | Comma-separated environment variable names a workflow may read via `{{$env.VAR}}`. Anything not listed resolves to an empty string. |
| `--bind <address>` | `AERINI_BIND` | `127.0.0.1` | Same as `serve`'s `--bind`. |
| `--file-sandbox-dir <dir>` | | `<data-dir>/files` | Restricts File node I/O to this directory. Unlike `serve`, leaving this unset doesn't just warn: the server creates `<data-dir>/files` and uses it as the sandbox automatically. |
| `--trusted-proxy-count <n>` | `AERINI_TRUSTED_PROXY_COUNT` | `0` | Same as `serve`'s. |
| `--allow-shell` | | `false` | Enables Shell Command nodes. Unlike `serve`, this prints its startup warning banner whenever the flag itself is passed, regardless of whether any current workflow has a Shell Command node yet, since any token with `write` scope can create one later. |
| `--allow-code` | | `false` | Enables Code (JS) nodes. Same always-on-when-passed warning pattern as `--allow-shell`. |
| `--allow-database` | | `false` | Enables Database nodes. Same always-on-when-passed warning pattern. |
| `--code-sandbox` | | `false` | Same as `serve`'s. See [Platform behavior of `--code-sandbox`](#platform-behavior-of---code-sandbox) below. |
| `--ssrf-firewall-acknowledged` | | `false` | Same as `serve`'s. |
| `--keychain` | | `false` | Backs the credential/token encryption key with the OS keychain (macOS Keychain, Windows Credential Manager, Linux Secret Service) instead of a plain file. Falls back to the file automatically, with a warning, if the keychain isn't available. |
| `--parallel-execution` | | `false` | Same as `serve`'s. |
| `--max-concurrent-nodes <n>` | | `8` | Same as `serve`'s, same clamp-to-1 floor. |
| `--max-workflow-duration-secs <secs>` | | none (no limit) | Same as `serve`'s: a ceiling combined with the workflow's own setting via `min()`, same 10 to 86400 clamp. |
| `--db-pool-size <n>` | `AERINI_DB_POOL_SIZE` | `max(available parallelism, 8)` | SQLite connection pool size. Should be at least `--max-concurrent-nodes` to avoid connection starvation. |
| `--max-concurrent-runs <n>` | | `10` | Maximum workflow executions running at once, across every caller. |
| `--max-queue-wait-secs <secs>` | | `30` | How long a run request waits for a free execution slot before the server gives up and returns `503`. `0` disables the queue entirely, reverting to an immediate `503` at capacity. |
| `--max-code-memory-mb <mb>` | | `512` (Linux only) | Same as `serve`'s. |
| `--plugin-dir <dir>` | | none | Same purpose as `serve`'s. `api` mode reloads plugins through `POST /api/plugins/reload` (see [Server API Reference](server-api-reference.md)) instead of `SIGHUP`. |

Most flags above are identical between the two subcommands. `--file-sandbox-dir` is the one that isn't: `serve` warns and leaves File nodes unrestricted when it's absent, `api` sandboxes to a directory under `--data-dir` automatically. `--port`, `--token`, `--data-dir`, `--allow-origin`, `--allow-env-vars`, `--keychain`, `--db-pool-size`, `--max-concurrent-runs`, and `--max-queue-wait-secs` exist only on `api`. `--config`, `--allow-legacy-run-secret`, and `--i-acknowledge-partial-sandbox` exist only on `serve`.

## Platform behavior of `--code-sandbox`

`--code-sandbox` behaves differently on each platform, for both subcommands:

- **Linux**: full enforcement. An ESM loader blocks `fs`/`net`/`child_process`-style imports, `eval`/`new Function` are separately blocked, and `setrlimit` caps both CPU time and memory (`--max-code-memory-mb`, default 512 MB) on the Node.js subprocess.
- **macOS**: partial. The ESM import restrictions and the `eval`/`new Function` block still apply, but `setrlimit` isn't available, so CPU and memory are uncapped and the per-node timeout is the only real ceiling. `serve` refuses to start with `--allow-code --code-sandbox` on macOS unless `--i-acknowledge-partial-sandbox` is also passed; `api` has no equivalent flag and only prints a warning before continuing.
- **Windows**: neither mechanism exists. Nothing at startup stops you from combining `--allow-code --code-sandbox` on either subcommand, but the first Code node that actually tries to run fails with a `SANDBOX_NOT_SUPPORTED` error instead of running unsandboxed. Run without `--code-sandbox` on Windows, or deploy on Linux for real sandboxing.

## Control commands

`list`, `stop`, `start`, `restart`, and `status` talk to a running `api`-mode server over HTTP using the same bearer token a client would. They accept no config file and start no server themselves.

| Command | Positional | Scope required | Notes |
|---|---|---|---|
| `list` | | `read` | Lists every scheduled workflow. |
| `stop <workflow>` | workflow name or ID | `read` + `write` | `read` to resolve the name/ID, `write` to actually stop it. Partial matches are accepted; an ambiguous match prints every candidate instead of guessing. |
| `start <workflow>` | workflow name or ID | `read` + `write` | Same matching rule as `stop`. |
| `restart <workflow>` | workflow name or ID | `read` + `write` | Calls stop then start against the same REST endpoints a scoped token would use. |
| `status` | | `read` | Identical output to `list`. |

Each of the five accepts:

| Flag | Env var | Default |
|---|---|---|
| `--server <url>` | | `http://localhost:7700` |
| `--token <token>` | `AERINI_TOKEN` | required |

### `tokens`

Manages scoped API tokens against a running `api`-mode server. Every `tokens` action requires `admin` scope.

| Subcommand | Args | Notes |
|---|---|---|
| `tokens list` | `--server`, `--token` | Lists active and revoked tokens. |
| `tokens create` | `--label <label>` (required), `--scopes <list>` (default `read,write`), `--expires-in <secs>` (optional, omit for a non-expiring token), `--server`, `--token` | `label` must be 1 to 256 characters; `scopes` must be a subset of `read`, `write`, `admin`. Either violation returns an error instead of creating a token. The raw token is only ever printed once, at creation. |
| `tokens revoke <token_id>` | `token_id` (positional, from `tokens list`), `--server`, `--token` | Fails with an error if `token_id` matches the token currently authenticating the call, you can't revoke your own in-use token. |

`--server` and `--token` behave the same way here as for `list`/`stop`/`start`/`restart`/`status` above.

## What's next

- [Server Deployment](server-deploy.md), for getting either mode actually running
- [Server API Reference](server-api-reference.md), for the REST routes these flags and control commands sit on top of
- [Security](../guide/security.md), for the reasoning behind the sandboxing, SSRF, and token defaults
- [Credentials](../guide/credentials.md), for how `serve`'s environment-variable credentials and `api`'s own encrypted store differ
