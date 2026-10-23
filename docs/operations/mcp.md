# MCP Server for AI Agents

`aerini-mcp` is a [Model Context Protocol](https://modelcontextprotocol.io/) (MCP) server that lets an AI agent (Claude Code, Claude Desktop, or any other MCP client that can start a local program) look up what Aerini's nodes can do, check a workflow, save it, and run it. It talks to the client over standard input and output, so there is nothing to host: the client starts it as a child process.

It is a separate small program, not part of the desktop app or of `aerini-server`. This is the reference page: modes, flags, tools, limits, security and error messages. If you want to get something working first, follow the [MCP Quickstart](mcp-quickstart.md). For how an agent should use the tools, see [Building Workflows with an AI Agent](../guide/agents.md).

**Where to start**

| You are... | Read |
|---|---|
| New to MCP or to this program | [MCP Quickstart](mcp-quickstart.md), then [What this is](#what-this-is) |
| Not sure which file to download | [Install the MCP Program](mcp-install.md), step 1 and step 2 |
| Setting up a client | [Client configuration](#client-configuration) |
| Something does not work | [Troubleshooting](#troubleshooting) |
| Reviewing the security model | [Write access](#write-access-and-allow-write), [Limits](#limits), [Security](#security) |
| Looking up a flag, tool or limit | [Configuration](#configuration), [Tools](#tools), [Limits](#limits) |

## What this is

An AI assistant such as Claude cannot see or change anything on your computer by itself. MCP is a standard way to give it a small, defined set of tools. Three parts are involved:

```text
 You  ->  AI client (Claude Code,   ->  aerini-mcp        ->  Aerini server (online)
          Claude Desktop, ...)          (this program)        or a .aerini file (offline)
                  starts it and talks to it
                  through stdin/stdout
```

- The **client** is the app you chat with. It decides which tools the model may call, and asks you before it uses them.
- **`aerini-mcp`** is the small program this page is about. The client starts it, and stops it when you close the client. You never run it yourself except to test it.
- What it connects to depends on the mode (see [Choosing a mode](#choosing-a-mode)): either a running `aerini-server api`, or nothing at all, in which case it saves workflows as files for the desktop app.

The agent gets tools to list nodes, read their schemas, check a workflow, save it, and, in online mode, run it. It gets no tools for credentials, tokens or plugins.

## Quick start

The fastest working setup is **offline mode**: no server, no token, nothing to host. The agent designs a workflow and hands you a `.aerini` file to open in the desktop app. It never runs anything. The step-by-step version, with a first prompt to try, is the [MCP Quickstart](mcp-quickstart.md).

The short form, for readers who have done this before: download the `aerini-mcp-offline` archive and its `.sha256` from the [Releases page](https://github.com/Panchak2d/aerini/releases) ([which file?](mcp-install.md)), verify and unpack it, then register it with your client by absolute path. For Claude Code:

```bash
claude mcp add --transport stdio aerini -- /full/path/to/aerini-mcp-offline
```

If you want the agent to run workflows, you need a server and a token: see [Choosing a mode](#choosing-a-mode).

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

Which one do you need?

- *I only use the desktop app and want an agent to draft workflows for it:* offline.
- *I run `aerini-server api` and want an agent to read, edit or run its workflows:* online.
- *I want to try it with no risk:* offline. It cannot reach a server, run a node or change anything but the one folder it writes to.

The mode is picked by the program's environment, not by a flag. `aerini-mcp` starts in the mode its environment selects and reports a start-up error instead of guessing: a URL without a token, or no URL in a build that does not include offline mode, stops it before it answers anything.

> [!NOTE]
> The two programs are different builds, not one program with a switch. The plain `aerini-mcp` download has no offline mode. If you run it without `AERINI_URL` it stops with an error (see [Troubleshooting](#troubleshooting)). If you want offline mode, download the file whose name contains `-offline`.

## Getting the program

Every release publishes both programs for Linux x86_64 and arm64, macOS Apple Silicon and Windows x64, each with a `.sha256` checksum. Picking the right file, verifying it, unpacking it and clearing the macOS and Windows warnings take seven short steps, written up in [Install the MCP Program](mcp-install.md). A source build for other platforms is covered there too.

Keep `aerini-mcp` and `aerini-server` on the same release: an older `aerini-mcp` works against a newer server, but a newer `aerini-mcp` against an older server fails on the routes the server does not have. See [Troubleshooting](#troubleshooting).

## Setting up the server side

This section applies to online mode only. Offline mode needs none of it.

In online mode `aerini-mcp` acts for a *token*: a secret string the server checks on every request. The token's *scope* (`read`, `write`, `admin`) and its optional workflow list decide what the agent can do, whatever the client or the program says.

1. Run `aerini-server api` as described in [Server Deployment](server-deploy.md#api-mode). Note its address; the default port is `7700`.
2. Create a token for the agent. Do not use the admin token the server printed on its first start.

   ```bash
   ./aerini-server tokens create --label claude-agent --scopes read,write --expires-in 604800 --token $AERINI_TOKEN
   ```

   Here `$AERINI_TOKEN` is your admin token. The command prints the new token once, so copy it now. `--scopes read` is enough to explore and validate; add `write` only if the agent should run workflows or change anything. Give it an expiry you are willing to renew (`604800` seconds is 7 days).
3. Restrict the token to the workflows the agent needs. `tokens create` has no flag for this, so use the REST API, either at creation with `workflow_ids` or afterwards per workflow:

   ```bash
   curl -s -X POST http://127.0.0.1:7700/api/tokens \
     -H "Authorization: Bearer $AERINI_TOKEN" -H "Content-Type: application/json" \
     -d '{"label":"claude-agent","scopes":["read","write"],"expires_in_secs":604800,"workflow_ids":["daily-report"]}'
   ```

   A restricted token can only read, run, save, delete and schedule the workflows it was granted. It cannot create a workflow under an id it was not granted, and it cannot use the server-wide credential routes. Details are in [Per-token workflow ACL](server-api-reference.md#per-token-workflow-acl).
4. Test the token before you put it in a client. A read-scoped token may list the node catalog, so this call must succeed:

   ```bash
   curl -s -H "Authorization: Bearer <agent token>" http://127.0.0.1:7700/api/nodes
   ```

   A JSON response means the address and token are right. `401` means the token is wrong, expired or revoked; no connection means the server is not running or the address is wrong. On Windows PowerShell, write `curl.exe` so you do not get PowerShell's own `curl` alias.

Never give an agent an `admin` token. `aerini-mcp` exposes no credential, token or plugin tools, but a token's scope is what the server enforces, and an admin token would let anything that can read the client's configuration do all of it.

## Configuration

| Flag | Environment variable | Default | What it does |
|---|---|---|---|
| `--url <url>` | `AERINI_URL` | none | Base address of an `aerini-server api` instance, for example `http://127.0.0.1:7700`. Set it for online mode, leave it unset for offline mode. A URL with a username or password, or a scheme other than `http` or `https`, is refused. Redirects are not followed. |
| none | `AERINI_TOKEN` | none | The agent's bearer token. Required with `--url`. It is read only from the environment, never from a flag, so it does not show in a process list. |
| `--allow-write` | none | off | Allows `save_workflow` (online), `delete_workflow`, `start_schedule` and `stop_schedule`. See [Write access](#write-access-and-allow-write). |
| `--timeout-secs <n>` | none | `30` | Time allowed for each server call other than `run_workflow`, 1 to 3600. |
| `--max-output-bytes <n>` | none | `32768` | Largest run result returned to the model, 1024 to 4194304. The rest is cut and marked. |
| `--out-dir <folder>` | `AERINI_OUT_DIR` | `~/aerini-workflows` | Offline mode only: where `save_workflow` writes `.aerini` files. |

`AERINI_TOKEN` is also the name `aerini-server` reads for its own token. Do not export your admin token under that name in the shell that starts the agent. In a client's configuration, set the variable explicitly, as in the snippets below.

Flags go in the client's `args`, and environment variables in its `env`. A client such as Claude Desktop starts the program from a working directory you do not control, so give `--out-dir` as an absolute path.

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

Offline, `save_workflow` writes the file named after the workflow's id, with unsafe characters replaced by `_` and `.aerini` added. A name Windows reserves for devices (such as `nul` or `com1`) gets a leading `_`. It never overwrites an existing file unless the call passes `overwrite=true`.

The tools carry honest hints for the client (`run_workflow` is marked destructive and open-world, the read tools read-only), but clients treat hints as advice. Keep your client's per-tool approval on for the tools that change things or run workflows.

## Write access and `--allow-write`

Three separate controls decide what an agent can change, and only two of them are enforced by the server.

- **Token scope** (server): a `read` token cannot save, delete, schedule or run anything, whatever the client does.
- **Workflow access list** (server): limits which workflow ids a token can touch. It does not limit what nodes a granted workflow contains.
- **`--allow-write`** (this program only): with the flag off, `aerini-mcp` refuses to call the four write tools. It stops a well-behaved client from editing the server by accident. It does not stop anything that can read the token and call the REST API directly, such as an agent that can run shell commands and read the client's configuration file.

`run_workflow` is not behind `--allow-write` because it does not change what the server stores, but it executes the workflow with all its side effects: HTTP calls, messages, writes to databases and files. The server therefore requires a `write`-scoped token for it, and a `read`-only token cannot run anything. A write token with `--allow-write` off gives an agent that can run workflows you built but cannot edit or schedule them through this program.

A safe way to start, from least to most power:

| Setup | The agent can |
|---|---|
| Offline program | Design workflows and write files. Nothing else. |
| Online, `read` token, no flag | Look around and validate. |
| Online, `write` token, no flag | Also run workflows you built, if the token may touch them. |
| Online, `write` token, `--allow-write` | Also save, delete and schedule. |

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

What a workflow returns can come from web pages, API responses, files, webhooks and AI replies, all of which an attacker may influence. `aerini-mcp` returns it under a field marked as data, strips control, bidirectional, zero-width and Unicode tag characters from every string, and cuts it at the output limit. That reduces the risk but cannot remove it. A model that reads the result can still be talked into acting on it, so keep approval on for `run_workflow` and for the write tools, and do not let an agent save and then run a workflow it just wrote without a person looking at it.

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

Claude Code's options, including `--env` and `--scope`, go before the server name, and `--` separates them from the program and its flags. Place `--transport stdio` between the last `--env` and the name, as above, or the CLI reads the name as another variable. Check the result with `claude mcp list`, or type `/mcp` inside a Claude Code session.

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

1. Open the configuration file from **Settings → Developer → Edit Config**, or edit it directly: `~/Library/Application Support/Claude/claude_desktop_config.json` on macOS, `%APPDATA%\Claude\claude_desktop_config.json` on Windows.
2. Add the server under `mcpServers`. If the file already has servers, add yours beside them; do not paste a second `mcpServers` block.
3. Use absolute paths (a relative path does not work), and on Windows double every backslash.
4. Save, then **quit Claude Desktop completely** and open it again. On macOS, closing the window leaves the app running with the old configuration.

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

On Windows, write the program path as `C:\\Users\\you\\tools\\aerini-mcp.exe`. The file must be valid JSON: a missing comma or a trailing comma can stop Claude Desktop from loading any server in it.

### Other clients

Any MCP client that starts a stdio server works: give it the program as the command, the flags as arguments, and `AERINI_URL` and `AERINI_TOKEN` as environment variables. `aerini-mcp` speaks MCP over standard input and output. It writes nothing but protocol messages to standard output; start-up errors and warnings go to standard error, and a start-up error ends the program with exit code `2`.

## Try it

Once the client shows the Aerini tools, give it a task and watch which tools it calls. Your client should ask you to approve each tool call the first time.

**Offline.** Ask something concrete and small:

> List the Aerini nodes that can send an HTTP request. Then build a workflow with a manual trigger and one HTTP Request node that fetches `https://example.com`, validate it, and save it.

The agent should call `list_nodes`, `get_node_schema`, `validate_workflow` and `save_workflow`, then tell you the path of the file it wrote (by default under `~/aerini-workflows`). Open the desktop app and drag that file onto the canvas.

**Online.** Start with questions that change nothing:

> Which workflows are on the server, and what is scheduled?

That calls `list_workflows` and `get_scheduler_status`. Move on to authoring or running only after you have checked what the token and `--allow-write` allow. For the order an agent should work in, see [Building Workflows with an AI Agent](../guide/agents.md).

## Troubleshooting

### The client shows no Aerini tools

Start the program by hand to see its start-up error. For online mode: `AERINI_URL=http://127.0.0.1:7700 AERINI_TOKEN=<token> /path/to/aerini-mcp`. For offline mode: `/path/to/aerini-mcp-offline`. A healthy program prints nothing and waits for a client on standard input; press Ctrl+C to stop it. On Windows PowerShell, set the variables with `$env:AERINI_URL = "..."` first.

| What you see | Cause and fix |
|---|---|
| `AERINI_URL is not set and this build of aerini-mcp has no offline mode` | You started the online program with no URL. Set `AERINI_URL` and `AERINI_TOKEN`, or download the `aerini-mcp-offline` archive instead. |
| `AERINI_URL is set but AERINI_TOKEN is not` | A URL without a token. Add `AERINI_TOKEN` to the client's `env`. A blank value counts as unset. |
| `AERINI_URL is not a valid URL`, `must start with http:// or https://`, `must not contain a username or password`, or `has no host` | Fix the address. Put the token in `AERINI_TOKEN`, not in the URL. |
| "Permission denied" (Linux, macOS) | The file is not executable: `chmod +x /path/to/aerini-mcp`. |
| macOS says the program cannot be opened, or a client reports it was blocked | The quarantine flag is set. See [step 6](mcp-install.md#step-6-clear-the-macos-and-windows-warnings). |
| Windows shows "Windows protected your PC" | SmartScreen. See [step 6](mcp-install.md#step-6-clear-the-macos-and-windows-warnings). |
| The program runs by hand but the client lists no tools | The path in the client is wrong or relative, a Windows backslash is not doubled, or the JSON is invalid. In Claude Desktop, also check that you quit it completely and reopened it. |
| Tools appear, then vanish after you edit the configuration | Most clients read the configuration only at start-up. Restart the client or start a new session. |

Claude Desktop keeps logs of what it did with each server. On macOS they are in `~/Library/Logs/Claude`. The start-up error from the steps above usually appears there.

### A tool call fails

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
| `Not written: ... already exists; pass overwrite=true to replace it.` | Offline: a file with that name is already in the output folder. Ask the agent to save under a new workflow id, or to pass `overwrite=true` if you want the old file replaced. |

## Glossary

| Term | Meaning |
|---|---|
| MCP (Model Context Protocol) | An open standard that lets an AI client call tools provided by another program. |
| Client | The app you talk to that starts and uses MCP servers: Claude Code, Claude Desktop and others. |
| stdio | Standard input and output. The client talks to `aerini-mcp` through them, so there is no network port to open. |
| Token | A secret string that proves who is calling the Aerini server. |
| Scope | What a token may do: `read`, `write` or `admin`. |
| Workflow access list | A per-token list of the workflow ids it may touch. |
| Target / platform | The CPU and operating system a program was built for, such as `aarch64-apple-darwin`. |
| Checksum (`.sha256`) | A fingerprint of a file, used to detect a damaged download. |
| Quarantine flag | A macOS marker on files downloaded through a browser. Gatekeeper uses it to block programs it has not verified. |

## Updating

Replace the program with the one from the new release. Nothing is stored between runs, so there is nothing to migrate. Update the server first, then the program. Restart the client afterwards so it starts the new program.

## What's next

- [MCP Quickstart](mcp-quickstart.md), the shortest path to a working setup
- [Install the MCP Program](mcp-install.md), picking, verifying and unpacking the download
- [Building Workflows with an AI Agent](../guide/agents.md), how an agent should use the tools
- [Server API Reference](server-api-reference.md), the REST routes behind the online tools
- [Server Deployment](server-deploy.md), running the server
- [Security](../guide/security.md), scopes, workflow access lists and server hardening
