use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortArity, PortDefinition, PortPosition};

mod pool;
mod sqlite;
mod postgres;
mod mysql;
mod redis;

pub use pool::start_pool_eviction_task;

// ── Node ──────────────────────────────────────────────────────────────────────

pub struct DatabaseNode;

#[async_trait]
impl Node for DatabaseNode {
    fn type_id(&self)      -> &'static str { "database" }
    fn display_name(&self) -> &'static str { "Database" }
    fn node_type(&self)    -> NodeType     { NodeType::Action }
    fn version(&self)      -> &'static str { "2.0.0" }
    fn description(&self)  -> &'static str { "Run SQL queries against a SQLite, PostgreSQL, or MySQL database, or key-value operations against Redis, and return the results." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "db_type": {
                    "type": "string",
                    "enum": ["sqlite", "postgres", "mysql", "redis"],
                    "description": "Database backend. Default: sqlite."
                },
                "db_path": {
                    "type": "string",
                    "description": "SQLite only. Absolute path ending in .db, .sqlite, or .sqlite3."
                },
                "connection_url": {
                    "type": "string",
                    "description": "Postgres / MySQL / Redis. Full connection URL including credentials. Examples: postgres://user:pass@host/db  mysql://user:pass@host/db  redis://host:6379"
                },
                "query": {
                    "type": "string",
                    "description": "SQL query. Use ? for parameters. Not used for Redis — use operation + key/value/field."
                },
                "params": {
                    "type": "string",
                    "description": "JSON array of positional parameters for ? placeholders. Example: [42, \"Alice\"]"
                },
                "operation": {
                    "type": "string",
                    "description": "SQL: query (SELECT) or execute (INSERT/UPDATE/DELETE). Redis: get, set, del, lpush, rpush, lpop, rpop, hget, hset."
                },
                "key": {
                    "type": "string",
                    "description": "Redis: key to operate on."
                },
                "value": {
                    "type": "string",
                    "description": "Redis: value for set, lpush, rpush, hset."
                },
                "field": {
                    "type": "string",
                    "description": "Redis: hash field name for hget and hset."
                },
                "expire": {
                    "type": "number",
                    "description": "Redis set: optional TTL in seconds. Omit for no expiry."
                },
                "allow_raw_sql": {
                    "type": "boolean",
                    "description": "SQL only. Default: false. When false, a query containing single-quoted string literals (or double-quoted ones on MySQL) is refused so values go through `?` and `params`. Only an admin caller (a server run with admin rights) can turn this on; the desktop app ignores it."
                },
                "allow_local": {
                    "type": "boolean",
                    "description": "Postgres / MySQL / Redis only. Default: false — connections to localhost, 127.0.0.1, and private-network addresses (10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, etc.) are blocked (SSRF protection). Set true only when connection_url intentionally targets a database you run yourself on this machine or LAN."
                }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "rows":           { "type": "array",            "description": "SQL SELECT rows." },
                "rows_affected":  { "type": "number",           "description": "SQL execute: rows changed." },
                "last_insert_id": { "type": "number",           "description": "Last inserted row ID. SQLite and MySQL populate this. Postgres returns 0 — use RETURNING instead." },
                "columns":        { "type": "array",            "description": "SQL SELECT: column names in result order." },
                "value":          { "type": ["string", "null"], "description": "Redis get/hget/lpop/rpop: retrieved value or null." },
                "ok":             { "type": "boolean",          "description": "Redis set/hset: true on success." },
                "deleted":        { "type": "number",           "description": "Redis del: number of keys deleted." },
                "list_length":    { "type": "number",           "description": "Redis lpush/rpush: list length after push." }
            }
        })
    }

    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs:  vec![PortDefinition { id: "input".to_string(),    label: "In".to_string(),    position: PortPosition::Left , port_type: None, arity: PortArity::Single }],
            outputs: vec![
                PortDefinition { id: "output".to_string(),   label: "Out".to_string(),   position: PortPosition::Right , port_type: None, arity: PortArity::Single },
                PortDefinition { id: "on_error".to_string(), label: "Error".to_string(), position: PortPosition::Right, port_type: None, arity: PortArity::Single },
            ],
        }
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        if input.context.metadata.get("__database_disabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            return NodeOutput::failure(NodeError::unrecoverable(
                "DATABASE_DISABLED",
                "Database node is disabled in this deployment. \
                 Pass --allow-database to the server to enable it.",
            ));
        }
        let db_type = input.input["db_type"].as_str().unwrap_or("sqlite");
        match db_type {
            "postgres" | "mysql" => postgres::execute_sqlx(input).await,
            "redis"              => redis::execute_redis(input).await,
            _                   => sqlite::execute_sqlite(input).await,
        }
    }
}

// ── SQLite execution ──────────────────────────────────────────────────────────

/// Checks whether a resolved SQL query string contains single-quoted literals that
/// may indicate direct expression interpolation instead of parameterized binding.
/// Returns a warning string when suspicious patterns are found.
///
/// Users must always use `?` placeholders and the `params` array for any value
/// that comes from workflow data or external input. Inline expression substitution
/// bypasses parameterized query protection.
pub(super) fn check_query_for_inline_values(query: &str) -> Option<String> {
    // Note: Aerini expression syntax ({{...}}) is resolved by the executor BEFORE
    // this function is called — checking for "{{" here would be dead code. The
    // correct enforcement point is at the workflow/executor level (pre-resolution).
    //
    // What we CAN check here (post-resolution) is literal SQL injection patterns:
    //   - PostgreSQL dollar-quoting ($$...$$): produces string literals with no
    //     single quotes, bypassing the single-quote heuristic entirely.
    //   - Single-quoted literals: may indicate inline value substitution.
    if query.trim().contains("$$") {
        return Some(
            "SQL INJECTION WARNING: This query contains PostgreSQL dollar-quoting ($$). \
             Dollar-quoted strings bypass the single-quote injection heuristic. \
             Use `?` placeholders and the `params` array for all dynamic values."
            .to_string()
        );
    }
    if query.trim().contains('\'') {
        return Some(
            "SQL INJECTION WARNING: This query contains single-quoted string literals. \
             If any quoted value originates from a workflow expression or external input, \
             use `?` placeholders and the `params` array instead of inline expressions. \
             Inline expression substitution bypasses parameterized query protection."
            .to_string()
        );
    }
    None
}

// ── Read-only query enforcement ────────────────────────────
//
// Shared by all three SQL backends (sqlite, postgres, mysql) so the "query"
// operation's read-only guarantee can't drift out of sync between them: a
// first-keyword-only check is bypassable via `WITH x AS (SELECT 1) DELETE
// FROM users` (legal SQL: a WITH clause may prefix SELECT, INSERT, UPDATE, or
// DELETE, per SQLite/ANSI SQL's own CTE grammar).
//
// Design: an allowlist, not a denylist. Only a bare SELECT, or a WITH clause
// whose every CTE body AND final statement are (recursively) SELECT, passes.
// Everything else is rejected — an allowlist of exactly one accepted shape
// can't leave a gap the way enumerating every dangerous keyword
// (INSERT/UPDATE/DELETE/DROP/TRUNCATE/PRAGMA/ATTACH/...) could.
//
// The recursion into each CTE body (not just the trailing keyword) exists
// because PostgreSQL supports data-modifying CTEs — `WITH x AS (DELETE FROM t
// RETURNING *) SELECT * FROM x` is valid, real Postgres syntax that performs a
// DELETE even though the clause's own trailing keyword is SELECT (verified
// against PostgreSQL's own docs: "You can use data-modifying statements
// (INSERT, UPDATE, DELETE, or MERGE) in WITH"). Checking only the keyword
// after the WITH clause is not sufficient once postgres is in scope.
// SQLite/MySQL don't support data-modifying CTE bodies, so this recursion is
// a no-op for those two backends — any such body would fail their own SQL
// parser as invalid syntax regardless of this check.
//
// Recursion depth is capped (`MAX_CTE_NESTING_DEPTH`): a `query` string is
// workflow data, which can originate from an untrusted imported file, so an
// unbounded parser here would be a stack-overflow DoS. Real CTE nesting is at
// most a handful of levels; 32 is generous headroom.
//
// Any parse failure while walking a WITH clause (malformed CTE list, unclosed
// string/paren, depth exceeded) is also rejected — fail closed, since this
// function's entire purpose is to gate write access and an ambiguous input must
// never be assumed safe.

const MAX_CTE_NESTING_DEPTH: u32 = 32;

/// Skips ASCII/Unicode whitespace, `-- line comments`, and `/* block comments */`
/// from the front of `s`. An unterminated block comment consumes to end of
/// string rather than erroring — safe for a read-only classifier, since that can
/// only make the function see *less* trailing statement, never invent tokens
/// that were not there.
fn skip_ws_and_comments(s: &str) -> &str {
    let mut s = s;
    loop {
        let trimmed = s.trim_start();
        if let Some(after) = trimmed.strip_prefix("--") {
            let end = after.find('\n').map(|i| i + 1).unwrap_or(after.len());
            s = &after[end..];
            continue;
        }
        if let Some(after) = trimmed.strip_prefix("/*") {
            match after.find("*/") {
                Some(i) => { s = &after[i + 2..]; continue; }
                None    => return "",
            }
        }
        return trimmed;
    }
}

/// Returns the next bare alphanumeric/underscore token at the front of `s`
/// (after skipping whitespace/comments), in its original case, plus the
/// remainder starting immediately after it. Returns ("", <ws-skipped s>) if
/// the next character isn't identifier-shaped.
///
/// Callers compare the returned word with `eq_ignore_ascii_case` rather than
/// uppercasing it here — every call site only compares or checks emptiness,
/// so an owned uppercased copy would be an allocation with no reader. ASCII
/// case-insensitive comparison (not `to_uppercase`/`to_lowercase`) is also
/// required for correctness: Unicode case-folding can change a string's byte
/// length (e.g. 'İ' -> "i̇"), which would desync a byte-offset built from a
/// folded copy against the original; ASCII-only comparison never does that.
fn take_word(s: &str) -> (&str, &str) {
    let s = skip_ws_and_comments(s);
    let end = s.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(s.len());
    (&s[..end], &s[end..])
}

/// Skips one `quote`-delimited literal (string literal `'...'`, or a quoted
/// identifier `"..."` / `` `...` ``) at the front of `s`, honoring the standard
/// doubled-quote escape for an embedded quote character. `s` must start
/// immediately after the opening quote. Returns the remainder after the closing
/// quote, or Err if the literal is never closed.
fn skip_quoted(s: &str, quote: char) -> Result<&str, ()> {
    let mut s = s;
    loop {
        match s.find(quote) {
            None => return Err(()),
            Some(i) => {
                let after = &s[i + quote.len_utf8()..];
                if after.starts_with(quote) {
                    s = &after[quote.len_utf8()..]; // doubled quote — escaped, keep going
                } else {
                    return Ok(after);
                }
            }
        }
    }
}

/// Skips one SQL identifier at the front of `s`: quoted (`"..."` or `` `...` ``)
/// or a bare alphanumeric/underscore run. Returns the remainder after it, or Err
/// if `s` doesn't start with anything identifier-shaped.
fn skip_identifier(s: &str) -> Result<&str, ()> {
    let s = skip_ws_and_comments(s);
    if let Some(rest) = s.strip_prefix('"') {
        return skip_quoted(rest, '"');
    }
    if let Some(rest) = s.strip_prefix('`') {
        return skip_quoted(rest, '`');
    }
    let (word, rest) = take_word(s);
    if word.is_empty() { Err(()) } else { Ok(rest) }
}

/// Skips one parenthesized group `( ... )` at the front of `s`, tracking nested
/// parens and skipping over string/quoted-identifier literals so that a paren
/// or quote character inside one doesn't miscount depth. Returns the remainder
/// after the matching close-paren, or Err if `s` doesn't start with `(` or the
/// group is never closed.
fn skip_paren_group(s: &str) -> Result<&str, ()> {
    let s = skip_ws_and_comments(s);
    let mut rest = s.strip_prefix('(').ok_or(())?;
    let mut depth: u32 = 1;
    loop {
        rest = skip_ws_and_comments(rest);
        let mut chars = rest.chars();
        match chars.next() {
            None => return Err(()),
            Some('(') => { depth += 1; rest = chars.as_str(); }
            Some(')') => {
                depth -= 1;
                rest = chars.as_str();
                if depth == 0 { return Ok(rest); }
            }
            Some(q @ ('\'' | '"' | '`')) => {
                rest = skip_quoted(chars.as_str(), q)?;
            }
            Some(_) => { rest = chars.as_str(); }
        }
    }
}

/// Returns `Ok(())` iff the statement starting at `s` is provably read-only: a
/// bare SELECT, or a WITH clause (optionally RECURSIVE) whose CTE bodies and
/// final statement are all, recursively, read-only per this same rule.
fn is_read_only_statement(s: &str, depth: u32) -> Result<(), ()> {
    if depth > MAX_CTE_NESTING_DEPTH {
        return Err(());
    }
    let (kw, after) = take_word(s);
    if kw.eq_ignore_ascii_case("SELECT") {
        return Ok(());
    }
    if !kw.eq_ignore_ascii_case("WITH") {
        return Err(());
    }
    let mut rest = skip_ws_and_comments(after);
    let (maybe_recursive, after_recursive) = take_word(rest);
    if maybe_recursive.eq_ignore_ascii_case("RECURSIVE") {
        rest = skip_ws_and_comments(after_recursive);
    }
    loop {
        rest = skip_identifier(rest)?;
        rest = skip_ws_and_comments(rest);
        if rest.starts_with('(') {
            rest = skip_paren_group(rest)?; // optional column-name list
            rest = skip_ws_and_comments(rest);
        }
        let (as_kw, after_as) = take_word(rest);
        if !as_kw.eq_ignore_ascii_case("AS") {
            return Err(());
        }
        rest = skip_ws_and_comments(after_as);
        if !rest.starts_with('(') {
            return Err(());
        }
        // Validate the CTE body itself before opaquely skipping past it — a
        // data-modifying CTE body (Postgres) must be rejected even though the
        // clause's own trailing keyword, checked below, is SELECT.
        is_read_only_statement(skip_ws_and_comments(&rest[1..]), depth + 1)?;
        rest = skip_paren_group(rest)?;
        rest = skip_ws_and_comments(rest);
        if let Some(after_comma) = rest.strip_prefix(',') {
            rest = after_comma;
            continue;
        }
        break;
    }
    let (final_kw, _) = take_word(rest);
    if final_kw.eq_ignore_ascii_case("SELECT") { Ok(()) } else { Err(()) }
}

/// Entry point: rejects any query that is not provably read-only. Used by the
/// "query" operation on all three SQL backends — "execute" has no such
/// restriction and remains the correct, unrestricted path for writes.
pub(super) fn enforce_read_only_query(query: &str) -> Result<(), String> {
    const REJECT_MSG: &str = "The Query operation only accepts SELECT statements \
        (a bare SELECT, or a WITH clause whose CTE definitions and final statement \
        are all SELECT). Use the Execute operation for INSERT, UPDATE, DELETE, or \
        other write operations.";
    scan_for_ambiguous_syntax(query).map_err(|_| REJECT_MSG.to_string())?;
    is_read_only_statement(query, 0).map_err(|_| REJECT_MSG.to_string())
}

/// Lexical pre-pass over the whole statement. The classifier above walks one
/// dialect-neutral reading of the text, but PostgreSQL, MySQL and SQLite lex
/// some constructs differently, and a construct that one reads as a comment
/// or string and another as code can hide a write from the classifier. This
/// pass rejects every such construct outright (fail closed) and also rejects
/// `INTO` (PostgreSQL `SELECT INTO` creates a table; MySQL `INTO OUTFILE`
/// writes a file) and a second statement after a `;`.
///
/// Rejected: `$tag$` dollar quotes, `#` comments, `--` not followed by
/// whitespace, MySQL `/*! */` executable comments, nested `/* /* */ */`
/// comments, a backslash inside a `'` or `"` literal (an escape in
/// PostgreSQL `E''` strings and MySQL, a plain character elsewhere), the word
/// `INTO`, and any text after a `;`.
fn scan_for_ambiguous_syntax(query: &str) -> Result<(), ()> {
    let b = query.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match c {
            b'\'' | b'"' | b'`' => {
                let rest = &query[i + 1..];
                let after = skip_quoted(rest, c as char)?;
                let consumed = &rest[..rest.len() - after.len()];
                if c != b'`' && consumed.contains('\\') {
                    return Err(());
                }
                i = query.len() - after.len();
            }
            b'#' => return Err(()),
            b';' => {
                if !skip_ws_and_comments(&query[i + 1..]).is_empty() {
                    return Err(());
                }
                i += 1;
            }
            b'$' => {
                let after_identifier = i > 0
                    && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_' || b[i - 1] == b'$');
                let numbered_parameter = b.get(i + 1).is_some_and(|n| n.is_ascii_digit());
                if !after_identifier && !numbered_parameter {
                    return Err(());
                }
                i += 1;
            }
            b'-' if b.get(i + 1) == Some(&b'-') => match b.get(i + 2) {
                None => return Ok(()),
                Some(n) if n.is_ascii_whitespace() => {
                    match query[i + 2..].find('\n') {
                        Some(nl) => i += 2 + nl + 1,
                        None => return Ok(()),
                    }
                }
                Some(_) => return Err(()),
            },
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let body = &query[i + 2..];
                if body.starts_with('!') || body.starts_with("M!") {
                    return Err(());
                }
                match body.find("*/") {
                    None => return Ok(()),
                    Some(end) => {
                        if body[..end].contains("/*") {
                            return Err(());
                        }
                        i += 2 + end + 2;
                    }
                }
            }
            _ if c.is_ascii_alphabetic() || c == b'_' => {
                let mut j = i;
                while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                    j += 1;
                }
                if query[i..j].eq_ignore_ascii_case("INTO") {
                    return Err(());
                }
                i = j;
            }
            _ => i += 1,
        }
    }
    Ok(())
}

/// Statements that would let a SQLite `execute` reach a file the node's path
/// checks never saw: `ATTACH` and `DETACH`, and `VACUUM ... INTO`, which
/// writes a copy of the database to any path (the filename may be a bound
/// parameter, so the inline-value check cannot see it).
pub(super) fn reject_path_escaping_statement(query: &str) -> Result<(), String> {
    let (kw, rest) = take_word(query);
    let blocked = kw.eq_ignore_ascii_case("ATTACH")
        || kw.eq_ignore_ascii_case("DETACH")
        || (kw.eq_ignore_ascii_case("VACUUM") && scan_for_ambiguous_syntax(rest).is_err());
    if blocked {
        return Err(
            "ATTACH, DETACH and VACUUM INTO are not allowed: they open or write database \
             files outside the db_path this node validated."
                .to_string(),
        );
    }
    Ok(())
}

/// Upper bound on rows one Query operation returns.
pub(super) const MAX_QUERY_ROWS: usize = 100_000;

pub(super) fn row_limit_message() -> String {
    format!(
        "Query returned more than {} rows. Add a LIMIT, or select fewer rows per run.",
        MAX_QUERY_ROWS
    )
}

pub(super) fn undecodable_placeholder(type_name: &str) -> String {
    format!("<{} not supported: cast it to text in the query>", type_name)
}

pub(super) fn unsupported_type_log(column: &str, type_name: &str) -> String {
    format!(
        "[UNSUPPORTED_TYPE] column '{}' is {}, which is not decoded; its values are returned \
         as a placeholder. Cast it to text in the query.",
        column, type_name
    )
}

pub(super) fn f32_json(f: f32) -> Value {
    format!("{}", f)
        .parse::<f64>()
        .ok()
        .and_then(serde_json::Number::from_f64)
        .map_or(Value::Null, Value::Number)
}

/// MySQL reads `"text"` as a string literal by default, so an inlined value in
/// double quotes escapes the single-quote check above.
pub(super) fn check_double_quoted_literal(query: &str) -> Option<String> {
    if query.contains('"') {
        return Some(
            "SQL INJECTION WARNING: This MySQL query contains double-quoted string literals. \
             MySQL treats double quotes as strings, so inlined values can break out of them. \
             Use `?` placeholders and the `params` array instead; quote identifiers with \
             backticks."
                .to_string(),
        );
    }
    None
}

// ── Shared helpers ────────────────────────────────────────────────────────────

/// Spawns `fut` on the current Tokio runtime if there is one; otherwise drops it.
/// Used from `Drop` paths, which may run outside a runtime during shutdown.
pub(super) fn spawn_detached<F>(fut: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(fut);
    }
}

pub(super) fn parse_params(v: &Value) -> Vec<Value> {
    v.as_str()
        .and_then(|s| serde_json::from_str::<Vec<Value>>(s).ok())
        .or_else(|| v.as_array().cloned())
        .unwrap_or_default()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::pool::validate_db_path;

    #[test]
    fn relative_path_rejected() {
        let e = validate_db_path("data/myapp.db").unwrap_err();
        assert!(e.contains("absolute"), "expected absolute path error, got: {}", e);
    }

    #[test]
    fn path_traversal_rejected() {
        let e = validate_db_path("/home/user/../secret.db").unwrap_err();
        assert!(e.contains(".."), "expected traversal error, got: {}", e);
    }

    #[test]
    fn bad_extension_rejected() {
        let e = validate_db_path("/home/user/authorized_keys").unwrap_err();
        assert!(e.contains(".db") || e.contains("sqlite"), "expected extension error, got: {}", e);
    }

    #[test]
    fn txt_extension_rejected() {
        let e = validate_db_path("/home/user/data.txt").unwrap_err();
        assert!(e.contains(".db") || e.contains("sqlite"), "expected extension error, got: {}", e);
    }

    #[test]
    fn valid_db_path_accepted() {
        assert!(validate_db_path("/home/user/myapp.db").is_ok());
    }

    #[test]
    fn valid_sqlite_path_accepted() {
        assert!(validate_db_path("/var/lib/app/store.sqlite").is_ok());
    }

    #[test]
    fn valid_sqlite3_path_accepted() {
        assert!(validate_db_path("/tmp/test.sqlite3").is_ok());
    }

    #[tokio::test]
    async fn private_ip_connection_url_rejected_under_strict_default() {
        use crate::nodes::util::SsrfPolicy;
        let urls = [
            "postgres://x:x@127.0.0.1:5432/db",
            "postgres://x:x@10.0.0.1:5432/db",
            "redis://169.254.169.254:6379",
            "mysql://x:x@::1:3306/db",
        ];
        for url in &urls {
            assert!(
                crate::nodes::util::check_db_url_ssrf(url, SsrfPolicy::Strict).await.is_err(),
                "Expected SSRF block for: {url}"
            );
        }
    }

    // Proves the allow_local escape hatch actually permits a private-IP
    // connection when the policy is AllowLocal.
    #[tokio::test]
    async fn private_ip_connection_url_allowed_when_policy_is_allow_local() {
        use crate::nodes::util::SsrfPolicy;
        let urls = [
            "postgres://x:x@127.0.0.1:5432/db",
            "postgres://x:x@10.0.0.1:5432/db",
            "redis://192.168.1.50:6379",
        ];
        for url in &urls {
            assert!(
                crate::nodes::util::check_db_url_ssrf(url, SsrfPolicy::AllowLocal).await.is_ok(),
                "Expected {url} to be permitted under SsrfPolicy::AllowLocal"
            );
        }
    }

    // Cloud-metadata / link-local targets must stay blocked even with the
    // opt-in set — allow_local widens "my own machine/LAN", not "anything".
    #[tokio::test]
    async fn cloud_metadata_ip_still_rejected_even_with_allow_local() {
        use crate::nodes::util::SsrfPolicy;
        assert!(
            crate::nodes::util::check_db_url_ssrf(
                "redis://169.254.169.254:6379",
                SsrfPolicy::AllowLocal,
            )
            .await
            .is_err(),
            "Cloud metadata IP must remain blocked under AllowLocal"
        );
    }
}

#[cfg(test)]
mod read_only_query_tests {
    use super::enforce_read_only_query;

    #[test]
    fn bare_select_accepted() {
        assert!(enforce_read_only_query("SELECT * FROM users").is_ok());
    }

    #[test]
    fn lowercase_select_accepted() {
        assert!(enforce_read_only_query("select * from users").is_ok());
    }

    #[test]
    fn leading_whitespace_and_comment_accepted() {
        assert!(enforce_read_only_query("  -- get all users\n  SELECT * FROM users").is_ok());
    }

    #[test]
    fn insert_rejected() {
        assert!(enforce_read_only_query("INSERT INTO users (name) VALUES ('x')").is_err());
    }

    #[test]
    fn update_rejected() {
        assert!(enforce_read_only_query("UPDATE users SET name = 'x'").is_err());
    }

    #[test]
    fn delete_rejected() {
        assert!(enforce_read_only_query("DELETE FROM users").is_err());
    }

    #[test]
    fn drop_rejected() {
        assert!(enforce_read_only_query("DROP TABLE users").is_err());
    }

    #[test]
    fn pragma_rejected() {
        assert!(enforce_read_only_query("PRAGMA journal_mode=WAL").is_err());
    }

    #[test]
    fn plain_with_select_accepted() {
        assert!(enforce_read_only_query(
            "WITH recent AS (SELECT * FROM orders WHERE total > 100) \
             SELECT * FROM recent WHERE total > 100"
        ).is_ok());
    }

    #[test]
    fn with_recursive_select_accepted() {
        assert!(enforce_read_only_query(
            "WITH RECURSIVE cnt(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM cnt WHERE x<10) \
             SELECT x FROM cnt"
        ).is_ok());
    }

    #[test]
    fn multiple_ctes_accepted() {
        assert!(enforce_read_only_query(
            "WITH a AS (SELECT 1), b AS (SELECT 2) SELECT * FROM a, b"
        ).is_ok());
    }

    #[test]
    fn cte_with_column_list_accepted() {
        assert!(enforce_read_only_query(
            "WITH x (a, b) AS (SELECT 1, 2) SELECT * FROM x"
        ).is_ok());
    }

    #[test]
    fn nested_with_all_select_accepted() {
        assert!(enforce_read_only_query(
            "WITH a AS (WITH b AS (SELECT 1) SELECT * FROM b) SELECT * FROM a"
        ).is_ok());
    }

    // exact bypass: a WITH clause whose CTE body is a harmless SELECT,
    // but whose trailing statement (the thing the WITH clause actually
    // prefixes) is a DELETE. The original sqlite check only inspected the
    // first keyword ("WITH") and let this straight through.
    #[test]
    fn with_select_then_delete_rejected() {
        assert!(enforce_read_only_query(
            "WITH x AS (SELECT 1) DELETE FROM users"
        ).is_err());
    }

    #[test]
    fn with_select_then_update_rejected() {
        assert!(enforce_read_only_query(
            "WITH x AS (SELECT 1) UPDATE users SET name = 'x'"
        ).is_err());
    }

    #[test]
    fn with_select_then_insert_rejected() {
        assert!(enforce_read_only_query(
            "WITH x AS (SELECT 1) INSERT INTO users (name) VALUES ('x')"
        ).is_err());
    }

    // PostgreSQL-specific bypass: a data-modifying CTE body (legal Postgres
    // syntax -- INSERT/UPDATE/DELETE with RETURNING inside a WITH) whose
    // trailing statement is an innocent-looking SELECT. Only checking the
    // trailing keyword is not enough once
    // postgres is in scope -- the fix must also reject the body itself.
    #[test]
    fn postgres_writable_cte_delete_rejected() {
        assert!(enforce_read_only_query(
            "WITH deleted AS (DELETE FROM users RETURNING *) SELECT * FROM deleted"
        ).is_err());
    }

    #[test]
    fn postgres_writable_cte_update_rejected() {
        assert!(enforce_read_only_query(
            "WITH t AS (UPDATE products SET price = price * 1.05 RETURNING *) SELECT * FROM t"
        ).is_err());
    }

    #[test]
    fn postgres_writable_cte_insert_rejected() {
        assert!(enforce_read_only_query(
            "WITH moved AS (INSERT INTO archive SELECT * FROM staging RETURNING *) \
             SELECT * FROM moved"
        ).is_err());
    }

    // Nested variant: the data-modifying statement is two CTE levels deep.
    #[test]
    fn nested_writable_cte_rejected() {
        assert!(enforce_read_only_query(
            "WITH a AS (WITH b AS (DELETE FROM users RETURNING *) SELECT * FROM b) \
             SELECT * FROM a"
        ).is_err());
    }

    #[test]
    fn quoted_identifier_cte_name_accepted() {
        assert!(enforce_read_only_query(
            "WITH \"my cte\" AS (SELECT 1) SELECT * FROM \"my cte\""
        ).is_ok());
    }

    #[test]
    fn semicolon_stacked_statement_after_with_rejected() {
        // Trailing content after the CTE list must itself start with SELECT --
        // a bare ';' (or anything else) is not, so this is rejected too, not
        // just tolerated as "no second statement to worry about".
        assert!(enforce_read_only_query(
            "WITH x AS (SELECT 1); DROP TABLE users;"
        ).is_err());
    }

    #[test]
    fn malformed_with_clause_rejected() {
        assert!(enforce_read_only_query("WITH x AS SELECT 1 SELECT * FROM x").is_err());
    }

    #[test]
    fn unterminated_paren_rejected() {
        assert!(enforce_read_only_query("WITH x AS (SELECT 1 SELECT * FROM x").is_err());
    }

    #[test]
    fn empty_query_rejected() {
        assert!(enforce_read_only_query("").is_err());
    }

    #[test]
    fn deeply_nested_ctes_beyond_cap_rejected_not_stack_overflowing() {
        // 40 levels of nesting exceeds MAX_CTE_NESTING_DEPTH (32) -- must
        // reject cleanly rather than recurse without bound.
        let mut q = String::new();
        for i in 0..40 {
            q.push_str(&format!("WITH c{} AS (", i));
        }
        q.push_str("SELECT 1");
        for _ in 0..40 {
            q.push(')');
        }
        q.push_str(" SELECT 1");
        assert!(enforce_read_only_query(&q).is_err());
    }
}

#[cfg(test)]
mod scan_tests {
    use super::{
        check_double_quoted_literal, enforce_read_only_query, f32_json, reject_path_escaping_statement,
        undecodable_placeholder,
    };

    #[test]
    fn read_only_check_rejects_syntax_the_dialects_read_differently() {
        let rejected = [
            "WITH x AS (SELECT $a$ \" $a$), y AS (DELETE FROM t RETURNING 1) SELECT 1 FROM y -- \") SELECT 1",
            "SELECT 1 FROM t WHERE a = E'\\'' AND b = 1 -- '",
            "SELECT * INTO copy FROM t",
            "SELECT 1 INTO OUTFILE \"/tmp/x\"",
            "SELECT 1--1 INTO OUTFILE \"/tmp/x\"",
            "SELECT 1 /*! INTO OUTFILE \"/tmp/x\" */",
            "SELECT 1 /* a /* b */ \" */ INTO x FROM t -- \"",
            "SELECT 1 # note",
            "SELECT 1; DELETE FROM t",
            "SELECT \"a\\\"b\" FROM t",
        ];
        for q in rejected {
            assert!(enforce_read_only_query(q).is_err(), "should reject: {q}");
        }
    }

    #[test]
    fn read_only_check_accepts_plain_selects_with_comments_and_parameters() {
        let accepted = [
            "SELECT a, \"b c\" FROM t WHERE x = ? AND y = $1 -- trailing note\n",
            "SELECT 1 /* inline */ FROM t;",
            "WITH c AS (SELECT price$ FROM t) SELECT * FROM c;  -- done",
        ];
        for q in accepted {
            assert!(enforce_read_only_query(q).is_ok(), "should accept: {q}");
        }
    }

    #[test]
    fn execute_rejects_statements_that_open_or_write_other_files() {
        let rejected = [
            "ATTACH DATABASE ? AS o",
            "  /* x */ detach database o",
            "VACUUM INTO ?",
            "VACUUM main INTO '/tmp/copy.db'",
        ];
        for q in rejected {
            assert!(reject_path_escaping_statement(q).is_err(), "should reject: {q}");
        }
        for q in ["VACUUM", "VACUUM main;", "INSERT INTO t VALUES (1)", "CREATE INDEX i ON t(a)"] {
            assert!(reject_path_escaping_statement(q).is_ok(), "should accept: {q}");
        }
    }

    #[test]
    fn sqlite_execute_refuses_attach_with_a_bound_path() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let err = super::sqlite::sqlite_run_execute(
            &conn,
            "ATTACH DATABASE ? AS o",
            &[serde_json::json!("/tmp/outside.db")],
        )
        .unwrap_err();
        assert!(err.contains("not allowed"), "got: {err}");
    }

    #[test]
    fn mysql_double_quoted_literal_is_flagged_but_backticks_are_not() {
        assert!(check_double_quoted_literal("SELECT * FROM t WHERE a = \"x\"").is_some());
        assert!(check_double_quoted_literal("SELECT `a` FROM t WHERE b = ?").is_none());
    }

    #[test]
    fn unsupported_column_types_are_named_and_f32_keeps_its_short_form() {
        assert!(undecodable_placeholder("NUMERIC").contains("NUMERIC"));
        assert_eq!(f32_json(0.1), serde_json::json!(0.1));
    }

    #[test]
    fn unc_db_path_is_rejected() {
        let e = super::pool::validate_db_path("//host/share/x.db").unwrap_err();
        assert!(e.contains("network"), "got: {e}");
    }
}
