# MCP Server for AI Agents

`aerini-mcp` is a [Model Context Protocol](https://modelcontextprotocol.io/) server that lets an AI agent (Claude Code, Claude Desktop, or any other MCP client that can start a local program) look up what Aerini's nodes can do, check a workflow, save it, and run it. It talks to the client over standard input and output, so there is nothing to host: the client starts it as a child process.

It is a separate small program, not part of the desktop app or of `aerini-server`. For how an agent should use the tools, see [Building Workflows with an AI Agent](../guide/agents.md). This page covers installing it, configuring a client, what each switch controls, and what to lock down first.

## Choosing a mode

| | Online | Offline |
|---|---|---|
| Needs | A running [`aerini-server api`](server-deploy.md#api-mode) and a token for it | Nothing running |
| Program | `aerini-mcp` | `aerini-mcp-offline` |
| Selected by | `AERINI_URL` is set | `AERINI_URL` is not set |
| Tools | All 11 | `list_nodes`, `get_node_schema`, `validate_workflow`, `save_workflow` |
| Runs workflows | Yes, on the server | Never |
| `save_workflow` | Replaces the workflow with that id on the server | Writes a `.aerini` file you import into the desktop app |
| Sees plugin nodes | Yes, the server's live catalog | No, built-in nodes only |

Use offline mode if you run the desktop app and have no server. The desktop app has no REST API and keeps its workflows in its own store, separate from any server's, so an agent in online mode sees the server's workflows and never the desktop's. Offline mode closes that gap by writing a file: drag the saved `.aerini` file onto the canvas, or on an empty canvas use the import button.

`aerini-mcp` starts in the mode its environment selects and reports a start-up error instead of guessing: a URL without a token, or no URL in a build that does not include offline mode, stops it before it answers anything.

## Getting the program

Every release publishes both programs for each platform on the [Releases page](https://github.com/Panchak2d/aerini/releases), as an archive that holds the program and the license text, plus a `.sha256` file beside it.

| Platform | Online | Offline |
|---|---|---|
| Linux x86_64 | `aerini-mcp_<version>_x86_64-unknown-linux-gnu.tar.gz` | `aerini-mcp-offline_<version>_x86_64-unknown-linux-gnu.tar.gz` |
| Linux arm64 | `aerini-mcp_<version>_aarch64-unknown-linux-gnu.tar.gz` | `aerini-mcp-offline_<version>_aarch64-unknown-linux-gnu.tar.gz` |
| macOS Apple Silicon | `aerini-mcp_<version>_aarch64-apple-darwin.tar.gz` | `aerini-mcp-offline_<version>_aarch64-apple-darwin.tar.gz` |
| Windows x64 | `aerini-mcp_<version>_x86_64-pc-windows-msvc.zip` | `aerini-mcp-offline_<version>_x86_64-pc-windows-msvc.zip` |

The online program is about 4 MB. The offline program contains the whole workflow engine and is about 47 MB (sizes measured on a Linux build; other platforms differ), which is why it is a separate download: an agent that only talks to a server does not need it. The offline archive holds a program named `aerini-mcp-offline`, so both can sit in one folder.

Check a download against its checksum before you run it:

```bash
sha256sum -c aerini-mcp_<version>_x86_64-unknown-linux-gnu.tar.gz.sha256
```

On macOS use `shasum -a 256 -c` instead of `sha256sum -c`.

Keep the program on the same release as the server. The server's REST surface only grows, so an older `aerini-mcp` works against a newer server, but a newer `aerini-mcp` against an older server fails on the routes the server does not have; see [Troubleshooting](#troubleshooting).

**macOS and Windows warnings.** The builds are not code-signed, like the desktop installers. On macOS a program downloaded through a browser carries a quarantine flag and Gatekeeper refuses to start it from an MCP client; clear the flag once with `xattr -d com.apple.quarantine <path to the program>`. On Windows, SmartScreen may warn the first time.

To build it yourself, from the repository root:

```bash
cargo build --release -p aerini-mcp
cargo build --release -p aerini-mcp --features offline
```

Each command leaves `target/release/aerini-mcp`; the second one overwrites the first, so copy the first result somewhere before you run the second. The offline build compiles the engine and its dependencies and takes much longer.

## Setting up the server side

This section applies to online mode only.

1. Run `aerini-server api` as described in [Server Deployment](server-deploy.md#api-mode). Note its address; the default port is `7700`.
2. Create a token for the agent. Do not use the admin token the server printed on its first start.

   ```bash
   ./aerini-server tokens create --label claude-agent --scopes read,write --expires-in 604800 --token $AERINI_TOKEN
   ```

   Here `$AERINI_TOKEN` is your admin token. The command prints the new token once. `--scopes read` is enough to explore and validate; add `write` only if the agent should run workflows or change anything. Give it an expiry you are willing to renew.
3. Restrict the token to the workflows the agent needs. `tokens create` has no flag for this, so use the REST API, either at creation with `workflow_ids` or afterwards per workflow:

   ```bash
   curl -s -X POST http://127.0.0.1:7700/api/tokens \
     -H "Authorization: Bearer $AERINI_TOKEN" -H "Content-Type: application/json" \
     -d '{"label":"claude-agent","scopes":["read","write"],"expires_in_secs":604800,"workflow_ids":["daily-report"]}'
   ```

   A restricted token can only read, run, save, delete and schedule the workflows it was granted. It cannot create a workflow under an id it was not granted, and it cannot use the server-wide credential routes. Details are in [Per-token workflow ACL](server-api-reference.md#per-token-workflow-acl).

Never give an agent an `admin` token. `aerini-mcp` exposes no credential, token or plugin tools, but a token's scope is what the server enforces, and an admin token would let anything that can read the client's configuration do all of it.

## Configuration

| Flag | Environment variable | Default | What it does |
|---|---|---|---|
| `--url <url>` | `AERINI_URL` | none | Base address of an `aerini-server api` instance, for example `http://127.0.0.1:7700`. Set it for online mode, leave it unset for offline mode. A URL with a username or password, or a scheme other than `http` or `https`, is refused. Redirects are not followed. |
| none | `AERINI_TOKEN` | none | The agent's bearer token. Required with `--url`. It is read only from the environment, never from a flag, so it does not show in a process list. |
| `--allow-write` | none | off | Allows `save_workflow` (online), `delete_workflow`, `start_schedule` and `stop_schedule`. See [Write access](#write-access-and---allow-write). |
| `--timeout-secs <n>` | none | `30` | Time allowed for each server call other than `run_workflow`, 1 to 3600. |
| `--max-output-bytes <n>` | none | `32768` | Largest run result returned to the model, 1024 to 4194304. The rest is cut and marked. |
| `--out-dir <folder>` | `AERINI_OUT_DIR` | `~/aerini-workflows` | Offline mode only: where `save_workflow` writes `.aerini` files. |

`AERINI_TOKEN` is also the name `aerini-server` reads for its own token. Do not export your admin token under that name in the shell that starts the agent. In a client's configuration, set the variable explicitly, as in the snippets below.

## Tools

All 11 tools are always listed, in the same order, whatever the flags. A tool that is switched off answers with an error that says how to enable it instead of disappearing.

| Tool | Modes | Token scope | Needs `--allow-write` | What it does |
|---|---|---|---|---|
| `list_nodes` | both | read | no | Compact list of node types: id, name, category, one-line description, and `enabled`. |
| `get_node_schema` | both | read | no | One node type's config schema, output schema and ports. |
| `validate_workflow` | both | read | no | Checks a workflow without saving or running it. Best effort, see below. |
| `list_workflows` | online | read | no | Saved workflows, with paging. |
| `get_workflow` | online | read | no | One workflow as a JSON object plus its `row_version`. |
| `get_scheduler_status` | online | read | no | Scheduled and background jobs and their state. |
| `save_workflow` | both | write | online only | Online: replaces the workflow with that id. Offline: writes a `.aerini` file. |
| `delete_workflow` | online | write | yes | Deletes a workflow, its run history and its schedule. Cannot be undone. |
| `run_workflow` | online | write | no | Runs a workflow once and waits for the result. |
| `start_schedule` | online | write | yes | Starts a workflow's trigger. It then runs without further calls. |
| `stop_schedule` | online | write | yes | Stops a running trigger. |

`validate_workflow` reports unknown node types, missing or cyclic connections, too many wires into one port, connections that name ports a node does not have, and empty required fields. It is a best-effort check: it cannot see credentials, network access or sandbox limits, and it does not check every per-node rule. `valid: true` means no known structural problem, not that the run will succeed. See [Server API Reference](server-api-reference.md#node-catalog-and-validation).

The tools carry honest hints for the client (`run_workflow` is marked destructive and open-world, the read tools read-only), but clients treat hints as advice. Keep your client's per-tool approval on for the tools that change things or run workflows.

## Write access and `--allow-write`

Three separate controls decide what an agent can change, and only two of them are enforced by the server.

- **Token scope** (server): a `read` token cannot save, delete, schedule or run anything, whatever the client does.
- **Workflow access list** (server): limits which workflow ids a token can touch. It does not limit what nodes a granted workflow contains.
- **`--allow-write`** (this program only): with the flag off, `aerini-mcp` refuses to call the four write tools. It stops a well-behaved client from editing the server by accident. It does not stop anything that can read the token and call the REST API directly, such as an agent that can run shell commands and read the client's configuration file.

`run_workflow` is not behind `--allow-write` because it does not change what the server stores, but it executes the workflow with all its side effects: HTTP calls, messages, writes to databases and files. The server therefore requires a `write`-scoped token for it, and a `read`-only token cannot run anything. A write token with `--allow-write` off gives an agent that can run workflows you built but cannot edit or schedule them through this program.

## Limits

| Limit | Value | Behavior |
|---|---|---|
| Request rate | 300 requests per minute per client IP, set by the server | `429` with `Retry-After: 60`. `aerini-mcp` never retries; it hands the wait to the model in the error. |
| Run slots | `--max-concurrent-runs` on the server, default `10`, shared by every caller | A run that cannot get a slot within `--max-queue-wait-secs` (default `30`) gets `503` with `Retry-After`. |
| Same workflow twice | One run of a workflow at a time | A second request waits up to 5 seconds, then gets `429` `workflow already running`. |
| `wait_secs` | Default `30`, maximum `900` | The time `run_workflow` waits for the result. When it runs out, `aerini-mcp` closes the request and returns an error. The server cancels a run when the client disconnects (see [Server API Reference](server-api-reference.md#workflows)), so the run is stopped and its result is lost. There is no tool to look a run up afterwards. |
| Run output | `--max-output-bytes`, default 32 KiB | Output is cut at that size. The result then carries `truncated`, `original_bytes` and `returned_bytes`, and the cut text is the start of the JSON, not valid JSON. |
| `list_workflows` page | Default 100, at most 500 | Use `offset` to page. |

A workflow may be allowed to run for up to 24 hours on the server, but one tool call waits at most 15 minutes. For longer work, start the workflow with a trigger (`start_schedule`) and check on it with `get_scheduler_status`.

## Security

### Plain `http` sends the token in clear text

If `AERINI_URL` starts with `http://` and the host is not `localhost`, `127.0.0.1` or `::1`, `aerini-mcp` prints a warning to its error output at start-up: the token travels unencrypted and anyone on the path can read it. Put the server behind HTTPS (see [Server Deployment](server-deploy.md)) or reach it through an SSH tunnel to a local port.

### Shell, Code and Database nodes plus a write token re-open remote code execution

`aerini-server api` switches Shell Command, Code (JS) and Database nodes off unless you start it with `--allow-shell`, `--allow-code` or `--allow-database`. That default is what keeps an agent with a write token from running commands on the server. If you enable any of them while you also issue an agent a write-scoped token, the agent can save a workflow containing that node and run it. The workflow access list does not help: it limits which workflow ids the token may write, not what nodes they hold, and a token that may overwrite a granted workflow may put a Shell Command node in it.

Do one of these instead: leave the three flags off on any server an agent has a write token for; give the agent a `read` token only; or run the agent against a separate server that has none of them enabled. `list_nodes` shows which node types the server has switched off (`enabled: false`).

### Run output is untrusted

What a workflow returns can come from web pages, API responses, files, webhooks and AI replies, all of which an attacker may influence. `aerini-mcp` returns it under a field marked as data, strips control, bidirectional and zero-width characters from every string, and cuts it at the output limit. That reduces the risk but cannot remove it. A model that reads the result can still be talked into acting on it, so keep approval on for `run_workflow` and for the write tools, and do not let an agent save and then run a workflow it just wrote without a person looking at it.

### Where the token lives

A client keeps the token in its own configuration. Claude Code's `claude mcp add` stores it in plain text in `~/.claude.json`; Claude Desktop stores it in `claude_desktop_config.json`. Use a short expiry, a narrow scope and a workflow access list, and revoke it with `./aerini-server tokens revoke <token_id> --token $AERINI_TOKEN` when you stop using it. For a configuration you share through version control, reference a variable from the environment instead of writing the token, as in the `.mcp.json` snippet below.

## Client configuration

Replace the program path with where you put it. Use the full path.

### Claude Code

Online, read and run only (running needs a write-scoped token; `--allow-write` stays off):

```bash
claude mcp add --env AERINI_URL=http://127.0.0.1:7700 --env AERINI_TOKEN=<agent token> --transport stdio aerini -- /path/to/aerini-mcp
```

Online, with authoring (`--allow-write` and a write token):

```bash
claude mcp add --env AERINI_URL=http://127.0.0.1:7700 --env AERINI_TOKEN=<agent token> --transport stdio aerini -- /path/to/aerini-mcp --allow-write
```

Offline, using the offline program:

```bash
claude mcp add --transport stdio aerini -- /path/to/aerini-mcp-offline
```

Claude Code's options, including `--env` and `--scope`, go before the server name, and `--` separates them from the program and its flags. Place `--transport stdio` between the last `--env` and the name, as above, or the CLI reads the name as another variable. Check the result with `claude mcp list`.

To share a setup with a team, add the server to a `.mcp.json` file in the project and keep the token out of it. Claude Code expands `${VAR}` in `command`, `args` and `env`, so each person supplies their own token from their environment:

```json
{
  "mcpServers": {
    "aerini": {
      "type": "stdio",
      "command": "/path/to/aerini-mcp",
      "args": [],
      "env": {
        "AERINI_URL": "http://127.0.0.1:7700",
        "AERINI_TOKEN": "${AERINI_AGENT_TOKEN}"
      }
    }
  }
}
```

Claude Code asks each person to approve a project-scoped server before it starts.

### Claude Desktop

Open the configuration file from **Settings → Developer → Edit Config**, or edit it directly: `~/Library/Application Support/Claude/claude_desktop_config.json` on macOS, `%APPDATA%\Claude\claude_desktop_config.json` on Windows. Add the server under `mcpServers`, use absolute paths (a relative path does not work), and on Windows double every backslash. Restart Claude Desktop afterwards.

Online, read and run only:

```json
{
  "mcpServers": {
    "aerini": {
      "command": "/path/to/aerini-mcp",
      "args": [],
      "env": {
        "AERINI_URL": "http://127.0.0.1:7700",
        "AERINI_TOKEN": "<agent token>"
      }
    }
  }
}
```

Online, with authoring: the same entry with `"args": ["--allow-write"]`.

Offline:

```json
{
  "mcpServers": {
    "aerini": {
      "command": "/path/to/aerini-mcp-offline",
      "args": ["--out-dir", "/path/to/folder/for/saved/workflows"]
    }
  }
}
```

On Windows, write the program path as `C:\\Users\\you\\tools\\aerini-mcp.exe`.

### Other clients

Any MCP client that starts a stdio server works: give it the program as the command, the flags as arguments, and `AERINI_URL` and `AERINI_TOKEN` as environment variables. `aerini-mcp` speaks MCP over standard input and output and sends no log lines on it.

## Troubleshooting

| Message starts with | Meaning |
|---|---|
| `401 Unauthorized` | The token is wrong, expired or revoked. |
| `403 Forbidden` | The token's scope or workflow access list does not allow this. The server's own reason is included. Ask whoever runs the server; do not retry. |
| `404: this Aerini server has no such route` | The server is older than `aerini-mcp`. Update `aerini-server`. |
| `404 Not found` | The workflow id does not exist. |
| `409 Conflict` | Someone changed the workflow since `get_workflow`. The message shows `current_row_version`. Read it again, merge, and save with the new `if_match_row_version`. |
| `429 Too Many Requests` | Rate limit, or the workflow is already running. Wait the `Retry-After` time. |
| `503 Service Unavailable` | The server is at its run limit. Wait the `Retry-After` time. |
| `Timed out after` | The server did not answer in time. For a run, raise `wait_secs` (maximum 900). |
| `Cannot reach the Aerini server` | Wrong address or port, or the server is not running. The message includes the cause. |
| `This tool needs online mode` | You are in offline mode and called a server tool. Set `AERINI_URL` and `AERINI_TOKEN`. |
| `This tool changes the Aerini server and is switched off` | Restart `aerini-mcp` with `--allow-write`; the token also needs write scope. |

If a client shows no Aerini tools at all, start the program by hand to see its start-up error: `AERINI_URL=http://127.0.0.1:7700 AERINI_TOKEN=<token> /path/to/aerini-mcp`. It waits for a client on standard input; press Ctrl+C to stop it.

## Updating

Replace the program with the one from the new release. Nothing is stored between runs, so there is nothing to migrate. Update the server first, then the program.

## What's next

- [Building Workflows with an AI Agent](../guide/agents.md), how an agent should use the tools
- [Server API Reference](server-api-reference.md), the REST routes behind the online tools
- [Server Deployment](server-deploy.md), running the server
- [Security](../guide/security.md), scopes, workflow access lists and server hardening
