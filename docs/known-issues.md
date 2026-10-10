# Known Issues

Limits that exist in the current release and have no fix yet. Each entry says what happens, when it can affect you, and what to do about it. For problems that have a fix or a workaround you can apply today, see [Troubleshooting](troubleshooting.md).

## Stopping a Shell Command or Code (JS) node on Windows kills only the command itself

On macOS and Linux, a timeout or a cancelled run kills the command and everything it started. On Windows only the command's own process is killed. A background process it launched (for example with `start`) keeps running after the run ends. If a Shell Command starts long-lived helpers on Windows, stop them yourself, or have the command clean up after itself.

## Aerini won't start if macOS Keychain access is denied

On macOS, Aerini asks for Keychain access to read the key that decrypts your saved credentials. If you click **Deny**, or the prompt fails, Aerini shows an error and closes. Your credentials are left untouched. Opening Aerini again normally shows the prompt again; choose **Always Allow**. Versions before this one could instead start with a new, empty key after a denial, leaving saved credentials unreadable; if that already happened to you, restore the key from your backup as described in [Credentials](guide/credentials.md#if-aerini-cant-read-the-key).

## macOS asks for Keychain access again after an update

Aerini's macOS build isn't signed with an Apple Developer certificate. macOS can treat each new build of such an app as a different program, so after an update (in-app or by hand) it may ask again for permission to read the key that decrypts your saved credentials. Choose **Always Allow**. If you click **Deny**, Aerini shows an error and closes without touching your credentials, as described in the entry above. Removing the repeat prompts needs an Apple Developer ID signature, which the project doesn't have yet.

## A database cancel can, rarely, hit the next statement on the same connection

When a run is cancelled or times out, a Postgres or MySQL `Execute` statement is cancelled by a second request to the server. If the statement finishes in the instant before that request lands, the cancel can interrupt the next statement that happens to be running on the same pooled connection. That statement fails with a cancellation error and can be retried. It needs a cancel and a statement completion to coincide, so it is rare.

## Postgres and MySQL cancel covers Execute, not Query

Cancelling a run or hitting a timeout stops a running `Execute` statement on the server. A long-running `Query` (read) statement is abandoned by Aerini but keeps running on the database until it finishes. SQLite is not affected: both operations are interrupted. Use a statement timeout on the database side for reads that can run long.

## A performance report for someone else's workflow answers `404`, other routes answer `403`

On `aerini-server` in `api` mode, a token restricted to specific workflows gets `404` from `GET|DELETE /api/performance/reports/:run_id` for a run outside its grants, the same as for a run that doesn't exist. Every other restricted route answers `403`. This is deliberate, since the report is looked up by run id and a `403` would confirm the run exists, but scripts that branch on the status code need to handle both. See [Per-token workflow ACL](operations/server-api-reference.md#per-token-workflow-acl).

## Postgres and MySQL can connect to a different address than the one Aerini checked

Aerini checks that a database host is allowed before connecting, but the database driver then looks the name up again to connect. A hostname that resolves to an allowed address for the check and a blocked one for the connection (DNS rebinding) is not stopped. Setting the TLS mode to `verify-full` (Postgres) or `VERIFY_IDENTITY` (MySQL) closes this for that connection, because the server's certificate must match the host name. With any other mode, use a firewall on the machine running Aerini to block the addresses you don't want it to reach. See [Security](guide/security.md).

## Linux on arm64 has not been tested

The Linux builds are verified on x86-64 only. Aerini may run on arm64 Linux, but no one has run it there, so a build or runtime problem specific to that platform is possible.

## Windows: the sign-in callback check and desktop notifications have not been run on Windows

The sign-in (OAuth) callback listener's handling of the browser hand-off on Windows (`rundll32`) and the Desktop Notification node on Windows have only been checked by reading and by unit tests, never run on a Windows machine. A Desktop Notification may report `sent: true` and still show nothing, because the notification's application id (`Aerini`) is not registered with Windows. If a notification does not appear, check the run output rather than relying on the node's result alone.

## A connect timeout may be treated as a plain timeout, so a send is not retried

A failed connection is retried for every node with a retry policy, and a timeout only for nodes that are safe to repeat (not for Slack, Telegram, Discord, SendGrid, GitHub, Notion, Sheets Append Row, image generation, or HTTP `POST`/`PATCH`). A connection that times out while being made is expected to count as a failed connection and be retried, but this has not been confirmed against a real network. If it is reported as a timeout, a send node does not retry it. That is the safe direction: nothing is sent twice.

## Parts that have not been tested against the real service

These paths are covered only by tests against local stand-ins, not by a real service, so a difference in a provider's real behavior can show up:

- S3 Storage against real AWS S3, Cloudflare R2 and MinIO, including presigned `PUT` headers, `delimiter` listings and the large-listing limit.
- Send Email against a real server: the `STARTTLS` (port 587) and implicit TLS (port 465) paths. The tests use a local server without TLS.
- Slack, Discord and SendGrid calls against the real services.
- Image Generation with the real providers (ComfyUI cancel, Flux, GPT Image, Nano Banana, Automatic1111).
- Nano Banana with `gemini-nano-banana-2.1` or `gemini-3.1-flash-lite-image`: Google's documentation does not confirm those models work with the request this node sends, so the default model is unchanged. If one fails, switch back to `gemini-2.5-flash-image` until Google retires it.
- Notion `create_page` with a **Data Source ID**, which is sent with Notion API version `2025-09-03`.
