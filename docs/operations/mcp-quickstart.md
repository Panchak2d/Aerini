# MCP Quickstart

By the end of this page, an AI assistant such as Claude will have built a small workflow for you and saved it as a file, and you'll have opened that file in the Aerini desktop app. It takes about 15 minutes, and you don't need a server, an API key or a token.

**You'll need:** the Aerini desktop app, and either Claude Code or Claude Desktop. If you use another MCP client that starts local programs, the same steps work; see [Other clients](mcp.md#other-clients).

## What this is, in one minute

An AI assistant can't see or change anything on your computer by itself. **MCP** (Model Context Protocol) is a standard way to hand it a small, fixed set of tools. `aerini-mcp` is the small program that provides Aerini's tools:

```text
 You  ->  AI client (Claude Code,   ->  aerini-mcp-offline  ->  a .aerini file
          Claude Desktop, ...)          (starts and stops with    in ~/aerini-workflows
                                         the client)
```

In this tutorial you use the **offline** build. It can look up which nodes exist, check a workflow for mistakes, and write the result to a file. It cannot run anything, reach a network or change anything except one folder. That makes it the safe way to try MCP.

## Step 1: Download the program

1. Open the [Releases page](https://github.com/Panchak2d/aerini/releases).
2. Find the **MCP server** table in the release notes, and download the **Offline** archive for your computer, plus the `.sha256` file with the same name.

The file name must start with `aerini-mcp-offline_`. A name that starts with `Aerini_` is the desktop app, which you don't need here. If you're unsure which platform is yours, [Install the MCP Program](mcp-install.md#step-2-match-the-platform-to-your-computer) shows how to check.

## Step 2: Verify and unpack it

Put both files in one folder, then check that the download is intact. On Linux:

```bash
sha256sum -c aerini-mcp-offline_0.4.3_x86_64-unknown-linux-gnu.tar.gz.sha256
```

You want to see `OK` after the file name. The macOS and Windows commands are in [Step 4 of the install page](mcp-install.md#step-4-verify-the-download). The examples use version `0.4.3`; use the one you downloaded.

Then unpack it into a folder of its own (Linux and macOS):

```bash
mkdir -p ~/aerini-mcp
tar -xzf aerini-mcp-offline_0.4.3_x86_64-unknown-linux-gnu.tar.gz -C ~/aerini-mcp
```

On Windows, use `Expand-Archive`; see [Step 5](mcp-install.md#step-5-unpack-it-into-its-own-folder).

## Step 3: Clear the macOS or Windows warning

The programs aren't code-signed, so your system may refuse to start them the first time.

- **macOS:** run `xattr -d com.apple.quarantine ~/aerini-mcp/aerini-mcp-offline` once. If it says the attribute doesn't exist, carry on.
- **Windows:** if SmartScreen appears, click **More info**, then **Run anyway**.

## Step 4: Check that it starts

```bash
~/aerini-mcp/aerini-mcp-offline --version
```

It prints the program's name and version. Then write down the **full path** of the program, because clients don't search your `PATH`. On Linux and macOS, `realpath ~/aerini-mcp/aerini-mcp-offline` prints it. On Windows, `(Resolve-Path $HOME\aerini-mcp\aerini-mcp-offline.exe).Path` does.

## Step 5: Register it with your client

Pick the client you use.

### Claude Code

```bash
claude mcp add --transport stdio aerini -- /full/path/to/aerini-mcp-offline
```

Check it with `claude mcp list`, or type `/mcp` inside a Claude Code session. Aerini should be listed.

### Claude Desktop

1. Open **Settings → Developer → Edit Config**.
2. Add the server under `mcpServers`. If the file already has servers, add yours beside them; don't paste a second `mcpServers` block.

   ```json
   {
     "mcpServers": {
       "aerini": {
         "command": "/full/path/to/aerini-mcp-offline",
         "args": ["--out-dir", "/full/path/to/a/folder/for/saved/workflows"]
       }
     }
   }
   ```

3. Use absolute paths. On Windows, double every backslash: `C:\\Users\\you\\aerini-mcp\\aerini-mcp-offline.exe`.
4. **Quit Claude Desktop completely** and open it again. On macOS, closing the window leaves the app running with the old configuration.

Claude Desktop starts the program from a folder you don't control, which is why the example sets `--out-dir` to an absolute path.

## Step 6: Ask for a workflow

Start a new chat (or a new Claude Code session) and ask something small and concrete:

> List the Aerini nodes that can send an HTTP request. Then build a workflow with a manual trigger and one HTTP Request node that fetches `https://example.com`, validate it, and save it.

Your client asks you to approve each tool call the first time. Approve them. You should see the agent call `list_nodes`, `get_node_schema`, `validate_workflow` and `save_workflow`, and then tell you the path of the file it wrote. By default that's a `.aerini` file under `~/aerini-workflows`.

## Step 7: Open it in Aerini

Open the desktop app and drag the saved `.aerini` file onto the canvas. On an empty canvas you can use the import button instead. Press **Run** to see it work. Importing always adds a new workflow, so it never overwrites one you already have.

You've now done the whole loop: an agent designed a workflow, checked it, and handed it to you as a file.

## If it didn't work

| What you see | Most likely cause |
|---|---|
| The client lists no Aerini tools | The path is wrong or relative, a Windows backslash isn't doubled, the JSON is invalid, or Claude Desktop wasn't fully quit. |
| macOS says the program can't be opened | The quarantine flag is set. Repeat Step 3. |
| `Not written: ... already exists` | A file with that name is already in the output folder. Ask the agent to save under a new workflow id. |

Run the program by hand to see its start-up error: `~/aerini-mcp/aerini-mcp-offline`. A healthy offline program prints nothing and waits for a client; press Ctrl+C to stop it. The full list of messages is in [Troubleshooting](mcp.md#troubleshooting).

## Where next

| You want to... | Go to |
|---|---|
| Let the agent run workflows, not just draft them | [Choosing a mode](mcp.md#choosing-a-mode), then [Setting up the server side](mcp.md#setting-up-the-server-side) |
| Understand what each switch and limit does | [MCP Server for AI Agents](mcp.md) |
| Teach your agent the right working order | [Building Workflows with an AI Agent](../guide/agents.md) |
| Know what to lock down before giving an agent write access | [Write access and `--allow-write`](mcp.md#write-access-and-allow-write) and [Security](mcp.md#security) |

::: details For experienced users
- The mode is chosen by the environment, not a flag: no `AERINI_URL` means offline, and offline exists only in the `aerini-mcp-offline` build.
- Offline serves four tools: `list_nodes`, `get_node_schema`, `validate_workflow` and `save_workflow`. The other seven are listed and answer with an error that says how to enable them.
- `save_workflow` never overwrites unless the call passes `overwrite=true`, and unsafe characters in the workflow id become `_`.
- Offline sees built-in nodes only, not plugin nodes. For the live catalog of a server, use online mode.
- `validate_workflow` is best effort: `valid: true` means no known structural problem, not that the run will succeed.
:::
