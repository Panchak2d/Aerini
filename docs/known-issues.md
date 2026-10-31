# Known Issues

Limits that exist in the current release and have no fix yet. Each entry says what happens, when it can affect you, and what to do about it. For problems that have a fix or a workaround you can apply today, see [Troubleshooting](troubleshooting.md).

## Aerini won't start if macOS Keychain access is denied

On macOS, Aerini asks for Keychain access to read the key that decrypts your saved credentials. If you click **Deny**, or the prompt fails, Aerini shows an error and closes. Your credentials are left untouched. Opening Aerini again normally shows the prompt again; choose **Always Allow**. Versions before this one could instead start with a new, empty key after a denial, leaving saved credentials unreadable; if that already happened to you, restore the key from your backup as described in [Credentials](guide/credentials.md#if-aerini-cant-read-the-key).

## macOS asks for Keychain access again after an update

Aerini's macOS build isn't signed with an Apple Developer certificate. macOS can treat each new build of such an app as a different program, so after an update (in-app or by hand) it may ask again for permission to read the key that decrypts your saved credentials. Choose **Always Allow**. If you click **Deny**, Aerini shows an error and closes without touching your credentials, as described in the entry above. Removing the repeat prompts needs an Apple Developer ID signature, which the project doesn't have yet.


## Launch at login starts hidden, and Aerini can't see an old-style Linux tray

**Launch at login** starts Aerini in the system tray with its window hidden, so a login doesn't put a window in front of you. Workflows with Schedule, Webhook or trigger-plugin triggers still run. Open the window from the tray icon's **Open Aerini**. The exception is the first start after an in-app update, which opens the window so you can see the result.

On Linux, Aerini asks the desktop whether it has a tray (a StatusNotifier host, as KDE Plasma and GNOME with the AppIndicator extension provide). If none turns up within 20 seconds of the start, the window opens instead of staying hidden, and closing the window quits Aerini unless a run or a background job is active, in which case the window minimizes. A desktop whose only tray is the old XEmbed kind (for example some i3 or Openbox setups using `trayer`) is not detected, so there Aerini opens its window at login and the close button quits or minimizes even if a tray icon appears.

## Secrets stay in memory after a node has used them

Aerini overwrites the encryption key and each decrypted credential when it drops them, but a node needs the value in an ordinary string to use it. While a run is in progress, copies exist in the node's input, in HTTP client buffers and in the process environment of `aerini-server` (which reads credentials from environment variables), and anything shown in the window, such as a secret you type or the backup key, lives in the webview's memory until it is collected. A process that can read Aerini's memory can find them. Run results mask saved credential values, but the memory itself is not scrubbed.

## Run results do not mask every secret

Masking is an exact-text match on saved credential values and on values typed into `api_key`, `password`, an S3 Secret Access Key, a Google Sheets or Social Upload Client Secret, or a plugin field marked `x-aerini-credential`. A secret a node has split or hashed is not recognised, and neither is a secret that sits in a field outside that list: a Discord webhook URL, a header you set on an HTTP Request node, or a Webhook trigger's shared secret. A node that echoes one of those into its output writes it into run history. Keep such values in a saved credential where the node has a field for one, and read a node's output before sharing a run.

## The Gemini model list sends the API key in the URL

**Fetch Models** for Gemini requests `/models?key=<api key>` and also sends the key in the `x-goog-api-key` header. Errors never include the URL, so the key does not reach Aerini's logs or run history, but the request line is visible to Google's servers and to a proxy that terminates TLS. Dropping the query parameter depends on Google accepting the header alone for this endpoint, which hasn't been confirmed. Use a key restricted to the Generative Language API, and rotate it if you route the app through an inspecting proxy.

## A database cancel is best-effort

When a run is cancelled or times out, Aerini closes the connection that was running a Postgres or MySQL `Execute` or `Query` statement and asks the server, over a second connection, to stop that statement. If the second connection can't be made within 5 seconds (for example the database is at its connection limit), or the session id needed to target the statement couldn't be read at the start, the statement keeps running on the database until it finishes, although Aerini no longer waits for it. Use a statement timeout on the database side for statements that can run long. SQLite is not affected: both operations are interrupted.

## A performance report for someone else's workflow answers `404`, other routes answer `403`

On `aerini-server` in `api` mode, a token restricted to specific workflows gets `404` from `GET|DELETE /api/performance/reports/:run_id` for a run outside its grants, the same as for a run that doesn't exist. Every other restricted route answers `403`. This is deliberate, since the report is looked up by run id and a `403` would confirm the run exists, but scripts that branch on the status code need to handle both. See [Per-token workflow ACL](operations/server-api-reference.md#per-token-workflow-acl).

## Postgres and MySQL can connect to a different address than the one Aerini checked

Aerini checks that a database host is allowed before connecting, but the database driver then looks the name up again to connect. A hostname that resolves to an allowed address for the check and a blocked one for the connection (DNS rebinding) is not stopped. Setting the TLS mode to `verify-full` (Postgres) or `VERIFY_IDENTITY` (MySQL) closes this for that connection, because the server's certificate must match the host name. Connecting to the checked address instead is not possible with the database driver (sqlx 0.8): it has no setting for the TLS server name apart from the address, so it would break certificate checks and providers that route by server name. With any other mode, use a firewall on the machine running Aerini to block the addresses you don't want it to reach. See [Security](guide/security.md).

## A saved credential is not tied to where it is sent

A workflow chooses which saved credential a node uses and which address the node calls. Importing a workflow that references a credential ID you also have, for example `openai_prod`, uses your key with whatever URL that workflow sets. On `aerini-server` in `api` mode, a token with write access can create a workflow that uses any credential ID the server knows. A credential can fill only a node's credential fields, so it can no longer be written into a URL or headers field, but nothing yet limits the hosts a credential may be sent to. Read the URL fields of a workflow you did not write before running it, and give server tokens only the access they need.

## Linux on arm64 has not been tested

The Linux builds are verified on x86-64 only. Aerini may run on arm64 Linux, but no one has run it there, so a build or runtime problem specific to that platform is possible.

## Windows: the sign-in callback check, desktop notifications and stopping a command's child processes have not been run on Windows

The sign-in (OAuth) callback listener's handling of the browser hand-off on Windows (`rundll32`), the Desktop Notification node on Windows, and the stopping of a Shell Command or Code (JS) node's child processes on Windows have only been checked by reading and by unit tests, never run on a Windows machine. When a Shell Command or Code (JS) node times out or its run is cancelled, Aerini ends the command and everything it started, as on macOS and Linux. A process the command launched in the first instants before Aerini grouped them could in theory be missed, and if Windows refuses the grouping only the command itself is ended; check for leftover processes after a stopped run. Aerini registers the notification's application id (`Aerini`) in the current user's registry before sending, since Windows shows nothing for an unknown id, but whether the notification then appears has not been seen on a real desktop. A Desktop Notification may still report `sent: true` and show nothing, for example while Focus assist is on. If a notification does not appear, check the run output rather than relying on the node's result alone.

## Linux desktop notifications have not been tried against a real daemon

The Desktop Notification node asks the notification daemon whether it reads markup in the body (the `body-markup` capability) and escapes `&`, `<` and `>` only if it does, or if the daemon cannot be asked. That question, and the 15 second limit on the notifier, have not been tried against a real or fake notification daemon; the limit is checked only with stand-in commands. dunst and mako report the capability from their global markup setting, so a daemon that decides per notification (a dunst rule for `notify-send`, say) can still show the escapes.

## Parts that have not been tested against the real service

These paths are covered only by tests against local stand-ins, not by a real service, so a difference in a provider's real behavior can show up:

- S3 Storage against real AWS S3, Cloudflare R2 and MinIO, including presigned `PUT` headers, `delimiter` listings and the large-listing limit.
- Send Email against a real server: the `STARTTLS` (port 587) and implicit TLS (port 465) paths. The tests use a local server without TLS.
- Slack, Discord and SendGrid calls against the real services.
- Image Generation with the real providers (ComfyUI cancel, Flux, GPT Image, Nano Banana, Automatic1111).
- Nano Banana with `gemini-nano-banana-2.1` or `gemini-3.1-flash-lite-image`: Google's documentation does not confirm those models work with the request this node sends, so the default model is unchanged. If one fails, switch back to `gemini-2.5-flash-image` until Google retires it.
- Notion `create_page` with a **Data Source ID**, which is sent with Notion API version `2025-09-03`.
- Instagram upload: the reply to `GET /me?fields=user_id,username` is read as a flat `user_id` (then `id`) or as `data[0].user_id`, because Meta's own sample and other code disagree on the shape. Confirm on a real professional account.

## Planned work and open questions

Gaps in the AI nodes and in server packaging that this release does not close. Each has an ID so a release note or an issue can point at it, and a target version where one is planned; a target is a plan, not a promise.

| ID | What is missing | Target |
|---|---|---|
| I10, I11 | The AI Agent does not run tools itself: a tool loop inside the node waits on sub-workflows, per-tool credentials and a human-approval step, so for now you wire each tool as nodes ([AI Agent Router](https://github.com/Panchak2d/Aerini/blob/main/examples/AI_Agent_Router.md), [AI Agent Loop](https://github.com/Panchak2d/Aerini/blob/main/examples/AI_Agent_Loop.md)), and the Loop example is bounded only by its turn count, with no token budget or time limit. | v0.5.0 |
| I15 | The AI Agent cannot use tools from an external MCP server; that comes after the tool loop. | v0.5.0 |
| B5 | The OpenAI nodes use Chat Completions only; OpenAI's Responses API and hosted tools have not been reviewed yet. | v0.5.0 |
| I17 | AI Prompt and AI Agent each carry their own request code, and AI Prompt has no Response Schema or Effort setting yet. | v0.4.5 |
| B3 | The model picker does not show whether a local Ollama model supports tools; a model without tool support fails the AI Agent with `TOOLS_NOT_SUPPORTED`. | v0.4.5 |
| BL1 | The Linux Server package from Export for Server does not contain `aerini-server`. `install.sh` downloads the matching release, which is published for x86-64 Linux only; on another architecture, or for a desktop build with no release, build it from source or run the Docker image instead ([Server Deployment](operations/server-deploy.md#getting-the-aerini-server-binary)). | not scheduled |
| BL3 | In the AI Agent Loop example, Stop When on `finished` does not end the Loop when a reply is cut off at Max Tokens, and whether `awaiting_tools` is a better condition has not been tested. | open question |
| BL4 | The AI Agent in a Loop has been run on Gemini only; OpenAI, Anthropic and local models are untested there. | untested |
