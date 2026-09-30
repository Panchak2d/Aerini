# Known Issues

Limits that exist in the current release and have no fix yet. Each entry says what happens, when it can affect you, and what to do about it. For problems that have a fix or a workaround you can apply today, see [Troubleshooting](troubleshooting.md).

## Stopping a Shell Command or Code (JS) node on Windows kills only the command itself

On macOS and Linux, a timeout or a cancelled run kills the command and everything it started. On Windows only the command's own process is killed. A background process it launched (for example with `start`) keeps running after the run ends. If a Shell Command starts long-lived helpers on Windows, stop them yourself, or have the command clean up after itself.

## A database cancel can, rarely, hit the next statement on the same connection

When a run is cancelled or times out, a Postgres or MySQL `Execute` statement is cancelled by a second request to the server. If the statement finishes in the instant before that request lands, the cancel can interrupt the next statement that happens to be running on the same pooled connection. That statement fails with a cancellation error and can be retried. It needs a cancel and a statement completion to coincide, so it is rare.

## Postgres and MySQL cancel covers Execute, not Query

Cancelling a run or hitting a timeout stops a running `Execute` statement on the server. A long-running `Query` (read) statement is abandoned by Aerini but keeps running on the database until it finishes. SQLite is not affected: both operations are interrupted. Use a statement timeout on the database side for reads that can run long.

## A performance report for someone else's workflow answers `404`, other routes answer `403`

On `aerini-server` in `api` mode, a token restricted to specific workflows gets `404` from `GET|DELETE /api/performance/reports/:run_id` for a run outside its grants, the same as for a run that doesn't exist. Every other restricted route answers `403`. This is deliberate, since the report is looked up by run id and a `403` would confirm the run exists, but scripts that branch on the status code need to handle both. See [Per-token workflow ACL](operations/server-api-reference.md#per-token-workflow-acl).
