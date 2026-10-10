use async_trait::async_trait;
use serde_json::{json, Value};
use std::process::Stdio;
use tokio::process::Command;

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::Node;

use super::util::{cfg_u64_opt, drain_capped, kill_group_on_drop, scrub_url_in_error, FloodLimit, Keep, OUTPUT_CAP_BYTES, OUTPUT_FLOOD_BYTES};

const MAX_TIMEOUT_SECS: u64 = 86_400;
/// Redaction rescans its input several times, so only this much of the end of stderr is redacted.
const ERROR_STDERR_WINDOW: usize = 16 * 1024;
/// How much of that redacted tail goes into a failure message.
const ERROR_STDERR_SHOWN: usize = 4 * 1024;

pub struct ShellExecNode;

#[async_trait]
impl Node for ShellExecNode {
    fn type_id(&self) -> &'static str { "shell_exec" }
    fn display_name(&self) -> &'static str { "Shell Command" }
    fn node_type(&self) -> NodeType { NodeType::Action }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Run a shell command on the host machine and capture its stdout, stderr, and exit code." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command": { "type": "string", "description": "Shell command to run" },
                "cwd":     { "type": "string", "description": "Working directory" },
                "env":     { "type": "object", "description": "Environment variables" },
                "timeout_secs": { "type": "number", "default": 30, "description": "Max run time in seconds (1 to 86400, default 30)" }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "stdout":      { "type": "string" },
                "stderr":      { "type": "string" },
                "exit_code":   { "type": "number" },
                "success":     { "type": "boolean" },
                "truncated":   { "type": "boolean", "description": "True when stdout or stderr exceeded 10 MB and the rest was discarded" }
            }
        })
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        if input.context.metadata.get("__shell_disabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            return NodeOutput::failure(NodeError::unrecoverable(
                "SHELL_DISABLED",
                "Shell Command node is disabled in this deployment. \
                 Pass --allow-shell to the server to enable it.",
            ));
        }

        let command = match input.input["command"].as_str() {
            Some(c) => c.to_string(),
            None => return NodeOutput::failure(
                NodeError::unrecoverable("MISSING_COMMAND", "command field is required")
            ),
        };

        let timeout_secs = match cfg_u64_opt(&input.input["timeout_secs"], "timeout_secs") {
            Ok(v) => v.unwrap_or(30).clamp(1, MAX_TIMEOUT_SECS),
            Err(e) => return NodeOutput::failure(e),
        };

        #[cfg(target_os = "windows")]
        let mut cmd = {
            let mut c = Command::new("cmd");
            c.args(["/C", &command]);
            c.kill_on_drop(true);
            c
        };

        #[cfg(not(target_os = "windows"))]
        let mut cmd = {
            let mut c = Command::new("sh");
            c.args(["-c", &command]);
            c.kill_on_drop(true);
            c
        };

        #[cfg(unix)]
        cmd.process_group(0);

        if let Some(cwd) = input.input["cwd"].as_str() {
            cmd.current_dir(cwd);
        }

        if let Some(env_obj) = input.input["env"].as_object() {
            for (k, v) in env_obj {
                if let Some(v_str) = v.as_str() {
                    cmd.env(k, v_str);
                }
            }
        }

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = match cmd.spawn() {
            Ok(c)  => c,
            Err(e) => return NodeOutput::failure(
                NodeError::unrecoverable("SPAWN_FAILED", e.to_string())
            ),
        };
        let group_guard = kill_group_on_drop(child.id());

        // Drained concurrently with wait(): a child that fills a pipe nobody reads blocks forever.
        let flood = FloodLimit::new(OUTPUT_FLOOD_BYTES);
        let mut stdout_task = tokio::spawn({
            let (reader, flood) = (child.stdout.take(), flood.clone());
            async move { drain_capped(reader, OUTPUT_CAP_BYTES, Keep::Head, &flood).await }
        });
        let mut stderr_task = tokio::spawn({
            let (reader, flood) = (child.stderr.take(), flood.clone());
            async move { drain_capped(reader, OUTPUT_CAP_BYTES, Keep::Head, &flood).await }
        });

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            async {
                let status = tokio::select! {
                    s = child.wait() => s,
                    _ = flood.tripped() => return None,
                };
                let stdout_bytes = (&mut stdout_task).await.unwrap_or_default();
                let stderr_bytes = (&mut stderr_task).await.unwrap_or_default();
                if stdout_bytes.flooded || stderr_bytes.flooded { return None; }
                Some((status, stdout_bytes, stderr_bytes))
            },
        ).await;

        match result {
            Ok(None) => {
                drop(group_guard);
                let _ = child.kill().await;
                stdout_task.abort();
                stderr_task.abort();
                NodeOutput::failure(NodeError::unrecoverable(
                    "OUTPUT_FLOOD",
                    format!(
                        "Command wrote more than {} MB to one output stream and was stopped. \
                         Redirect large output to a file instead.",
                        OUTPUT_FLOOD_BYTES / (1024 * 1024)
                    ),
                ))
            }
            Err(_) => {
                drop(group_guard);
                let _ = child.kill().await;
                stdout_task.abort();
                stderr_task.abort();
                // Not recoverable: the command may already have had side effects, and a retry would repeat them.
                NodeOutput::failure(
                    NodeError::unrecoverable("TIMEOUT", format!("Command timed out after {}s", timeout_secs))
                )
            }
            Ok(Some((Err(e), _, _))) => {
                group_guard.disarm();
                stdout_task.abort();
                stderr_task.abort();
                NodeOutput::failure(NodeError::unrecoverable("EXEC_ERROR", e.to_string()))
            }
            Ok(Some((Ok(status), stdout_cap, stderr_cap))) => {
                group_guard.disarm();
                let truncated = stdout_cap.truncated || stderr_cap.truncated;
                let stdout    = String::from_utf8_lossy(&stdout_cap.bytes).to_string();
                let stderr    = String::from_utf8_lossy(&stderr_cap.bytes).to_string();
                let exit_code = status.code().unwrap_or(-1);
                let success   = status.success();

                let mut logs = vec![
                    format!("Command: {}", redact_command_log(&command)),
                    format!("Exit code: {}", exit_code),
                ];
                if truncated {
                    logs.push(format!("Output truncated at {} MB per stream", OUTPUT_CAP_BYTES / (1024 * 1024)));
                }

                if success {
                    NodeOutput::success_with_logs(
                        json!({ "stdout": stdout, "stderr": stderr, "exit_code": exit_code, "success": true, "truncated": truncated }),
                        logs,
                    )
                } else {
                    NodeOutput::failure_with_logs(
                        NodeError::unrecoverable(
                            "COMMAND_FAILED",
                            format!("Exit code {}: {}", exit_code, failure_stderr(&stderr)),
                        ),
                        logs,
                    )
                }
            }
        }
    }
}

/// Redacts sensitive patterns from free text (a command line or a command's stderr).
/// Applied in order: Authorization headers → URL credentials → secret flags
/// (`--password x`, `--passphrase=x`) → `--key=` → `sshpass -p` → curl `-u`
/// credentials → attached database passwords → env assignments.
/// URL-credential redaction delegates to the shared `scrub_url_in_error` so the
/// two code paths can't drift apart.
fn redact_text(text: &str) -> String {
    let s = redact_authorization_header(text);
    let s = scrub_url_in_error(&s);
    let s = redact_secret_flags(&s);
    let s = redact_after_prefix(&s, "--key=");
    let s = redact_after_prefix(&s, "sshpass -p ");
    let s = redact_curl_credentials(&s);
    let s = redact_attached_db_password(&s);
    redact_env_assignments(&s)
}

/// The end of `s` (where a failing command prints its error) that fits in `max` bytes, from a char boundary.
fn tail_clip(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

/// stderr for a failure message: redacted, and cut to its last `ERROR_STDERR_SHOWN` bytes.
pub(super) fn failure_stderr(stderr: &str) -> String {
    let window = tail_clip(stderr, ERROR_STDERR_WINDOW);
    let redacted = redact_text(window);
    let shown = tail_clip(&redacted, ERROR_STDERR_SHOWN);
    if shown.len() < stderr.len() && (window.len() < stderr.len() || shown.len() < redacted.len()) {
        format!("[earlier output omitted] {}", shown)
    } else {
        shown.to_string()
    }
}

/// `redact_text` for run history, truncated to 500 chars.
fn redact_command_log(cmd: &str) -> String {
    let s = redact_text(cmd);
    if s.len() > 500 {
        let mut end = 500;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}... [truncated]", &s[..end])
    } else {
        s
    }
}

/// Redacts `Authorization:` header values. Handles double-quoted, single-quoted
/// and unquoted forms; a closing quote is kept so the rest of the command still
/// reads correctly.
/// Example: `-H 'Authorization: Bearer sk-abc'` → `-H 'Authorization: [REDACTED]'`
fn redact_authorization_header(cmd: &str) -> String {
    let mut result = String::with_capacity(cmd.len());
    // ASCII-only lowering: "authorization:" is a fixed ASCII literal,
    // and `to_ascii_lowercase()` only ever remaps 'A'-'Z' to 'a'-'z' byte-for-byte
    // -- every other byte, and therefore every char boundary, is left exactly
    // where it was in `cmd`. Unicode-aware `to_lowercase()` can expand some
    // characters (e.g. 'İ' U+0130, 2 bytes, lowercases to "i̇", 3 bytes),
    // which shifts `lower`'s byte offsets out of alignment with `cmd`'s the
    // moment such a character appears anywhere earlier in the string -- `i`
    // below is stepped along `cmd`'s own char boundaries and used to index
    // into `lower`, so that divergence either misses/misplaces the redaction
    // or slices `lower` off a char boundary and panics.
    let lower = cmd.to_ascii_lowercase();
    let mut i = 0;
    while i < cmd.len() {
        if lower[i..].starts_with("authorization:") {
            result.push_str("Authorization: [REDACTED]");
            let quote = cmd[..i].chars().next_back().filter(|c| matches!(c, '"' | '\''));
            i += 14 + authorization_value_len(&cmd[i + 14..], quote);
        } else {
            let ch = cmd[i..].chars().next().expect("valid UTF-8 offset");
            result.push(ch);
            i += ch.len_utf8();
        }
    }
    result
}

/// Byte length of the header value that follows `Authorization:`, not
/// including a closing quote.
/// - Quoted (`quote` is the quote char right before the header name): everything
///   up to the closing quote, or the end of the string when unterminated.
/// - Unquoted: one word, or two when the first is an auth scheme
///   (`Bearer <token>`). Never runs on to the URL or flags that follow.
fn authorization_value_len(rest: &str, quote: Option<char>) -> usize {
    if let Some(q) = quote {
        return rest.find(q).unwrap_or(rest.len());
    }
    const SCHEMES: &[&str] = &["bearer", "basic", "digest", "token"];
    let mut end = 0;
    for pass in 0..2 {
        let tail = &rest[end..];
        let word_start = end + (tail.len() - tail.trim_start().len());
        end = rest[word_start..]
            .find(|c: char| c.is_ascii_whitespace())
            .map_or(rest.len(), |n| word_start + n);
        if pass == 0 && !SCHEMES.iter().any(|s| rest[word_start..end].eq_ignore_ascii_case(s)) {
            break;
        }
    }
    end
}

/// Redacts the value that immediately follows `prefix` (case-insensitive).
/// Matches only at word boundaries (start of string or preceded by whitespace/quote).
/// Value ends at the next whitespace, quote, or end of string.
///
/// Used for flags like `--password=VALUE` (prefix = `--password=`)
/// and `sshpass -p VALUE` (prefix = `sshpass -p `, space included).
fn redact_after_prefix(cmd: &str, prefix: &str) -> String {
    // ASCII-only lowering: every `prefix` this function is ever called
    // with (`--password=`, `-p `, etc.) is a fixed ASCII literal, so ASCII
    // case-insensitive matching is exactly what's needed. `to_ascii_lowercase()`
    // never changes a string's byte length or char-boundary positions (unlike
    // Unicode-aware `to_lowercase()`), so `abs`/`val_start` below -- computed
    // from `lc_cmd` and then used to index directly into `cmd` -- always land
    // on a valid position in `cmd` too. See `redact_authorization_header`'s
    // comment above for the concrete divergence example this avoids.
    let lc_cmd    = cmd.to_ascii_lowercase();
    let lc_prefix = prefix.to_ascii_lowercase();
    let mut result = String::with_capacity(cmd.len());
    let mut last   = 0;
    let mut search = 0;

    while search < lc_cmd.len() {
        match lc_cmd[search..].find(&lc_prefix) {
            None => break,
            Some(rel) => {
                let abs = search + rel;
                let at_boundary = abs == 0
                    || matches!(cmd.as_bytes()[abs - 1], b' ' | b'\t' | b'"' | b'\'');

                if at_boundary {
                    let val_start = abs + prefix.len();
                    if val_start > cmd.len() {
                        search = abs + 1;
                        continue;
                    }
                    let val_len = cmd[val_start..]
                        .find(|c: char| c.is_ascii_whitespace() || matches!(c, '"' | '\''))
                        .unwrap_or(cmd.len() - val_start);

                    if val_len > 0 {
                        result.push_str(&cmd[last..abs + prefix.len()]);
                        result.push_str("[REDACTED]");
                        last   = val_start + val_len;
                        search = last;
                    } else {
                        // Empty value — nothing to redact.
                        search = abs + 1;
                    }
                } else {
                    search = abs + 1;
                }
            }
        }
    }

    result.push_str(&cmd[last..]);
    result
}

/// Redacts the password in `-pSECRET` (no space) for the MySQL/MariaDB client family.
/// Only these clients take the password attached to `-p`; for them `-p SECRET`
/// with a space prompts and reads SECRET as the database name, and in other
/// tools `-p` is a port or `mkdir -p`, so the match is tied to the program name
/// and lasts until the next `|`, `;` or `&`.
fn redact_attached_db_password(cmd: &str) -> String {
    const CLIENTS: &[&str] = &[
        "mysql", "mysqldump", "mysqladmin", "mysqlpump", "mysqlimport",
        "mysqlcheck", "mysqlshow", "mariadb", "mariadb-dump", "mariadb-admin",
    ];

    let mut result    = String::with_capacity(cmd.len());
    let mut last      = 0;
    let mut pos       = 0;
    let mut in_client = false;

    while pos < cmd.len() {
        let start = pos + (cmd[pos..].len() - cmd[pos..].trim_start().len());
        if start >= cmd.len() {
            break;
        }
        let mut end = cmd[start..]
            .find(|c: char| c.is_ascii_whitespace())
            .map_or(cmd.len(), |n| start + n);
        let token = &cmd[start..end];

        let program = token
            .trim_matches(|c| matches!(c, '"' | '\''))
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("");
        let program = program.strip_suffix(".exe").unwrap_or(program);
        if CLIENTS.iter().any(|c| program.eq_ignore_ascii_case(c)) {
            in_client = true;
        } else if in_client && token.len() > 2 && token.starts_with("-p") {
            let val_start = start + 2;
            match cmd[val_start..].chars().next() {
                Some(q @ ('"' | '\'')) => {
                    let close = cmd[val_start + 1..].find(q).map_or(cmd.len(), |n| val_start + 1 + n);
                    result.push_str(&cmd[last..val_start + 1]);
                    result.push_str("[REDACTED]");
                    last = close;
                    end = end.max(close);
                }
                _ => {
                    result.push_str(&cmd[last..val_start]);
                    result.push_str("[REDACTED]");
                    last = end;
                }
            }
        }

        if matches!(token, "|" | "||" | "&" | "&&" | ";") || token.ends_with(';') {
            in_client = false;
        }
        pos = end;
    }

    result.push_str(&cmd[last..]);
    result
}

/// Long flags whose value is a secret. `--key` is left to `--key=` only: given
/// a space it is usually a key file path.
const SECRET_FLAGS: &[&str] = &[
    "--password", "--passwd", "--pass", "--passphrase", "--token", "--secret",
    "--client-secret", "--access-token", "--auth-token", "--api-key", "--api_key",
    "--http-password", "--ftp-password", "--proxy-password",
];

/// Byte range of the secret in a value that starts at `start`: inside the quotes
/// when quoted, otherwise up to the next whitespace. Also returns where scanning resumes.
fn value_span(cmd: &str, start: usize) -> (usize, usize, usize) {
    match cmd[start..].chars().next() {
        Some(q @ ('"' | '\'')) => {
            let inner = start + 1;
            let end = cmd[inner..].find(q).map_or(cmd.len(), |n| inner + n);
            (inner, end, (end + 1).min(cmd.len()))
        }
        _ => {
            let end = cmd[start..]
                .find(|c: char| c.is_ascii_whitespace())
                .map_or(cmd.len(), |n| start + n);
            (start, end, end)
        }
    }
}

/// Redacts the value of every `SECRET_FLAGS` flag, in both `--flag=value` and
/// `--flag value` forms, quoted or not. A flag that is not followed by a value
/// (end of command, or another flag) is left alone, as is `--password-file`.
fn redact_secret_flags(cmd: &str) -> String {
    let lc = cmd.to_ascii_lowercase();
    let bytes = cmd.as_bytes();
    let mut result = String::with_capacity(cmd.len());
    let mut last = 0;
    let mut i = 0;

    while let Some(rel) = lc[i..].find("--") {
        let at = i + rel;
        let boundary = at == 0 || matches!(bytes[at - 1], b' ' | b'\t' | b'\n' | b'"' | b'\'');
        let hit = boundary
            .then(|| {
                SECRET_FLAGS.iter().find_map(|flag| {
                    let after = at + flag.len();
                    (lc[at..].starts_with(*flag) && after < bytes.len()
                        && (bytes[after] == b'=' || matches!(bytes[after], b' ' | b'\t')))
                        .then_some(after)
                })
            })
            .flatten();
        let Some(after) = hit else {
            i = at + 2;
            continue;
        };

        let val_start = if bytes[after] == b'=' {
            after + 1
        } else {
            after + cmd[after..].len() - cmd[after..].trim_start_matches([' ', '\t']).len()
        };
        let spaced = bytes[after] != b'=';
        if val_start >= cmd.len() || (spaced && bytes[val_start] == b'-') {
            i = at + 2;
            continue;
        }
        let (from, to, resume) = value_span(cmd, val_start);
        if from == to {
            i = resume.max(at + 2);
            continue;
        }
        result.push_str(&cmd[last..from]);
        result.push_str("[REDACTED]");
        last = to;
        i = resume;
    }

    result.push_str(&cmd[last..]);
    result
}

/// Redacts the password in curl's `user:password` credentials (`-u`, `--user`,
/// `--proxy-user`, `-U`, attached `-uuser:pass`, and short clusters ending in
/// `u` such as `-sSu`). Tied to the program name like `redact_attached_db_password`,
/// since `-u` means something else elsewhere (`docker run -u 1000:1000`). The
/// user name stays; a value with no `:` carries no password and is left alone.
fn redact_curl_credentials(cmd: &str) -> String {
    let mut result  = String::with_capacity(cmd.len());
    let mut last    = 0;
    let mut pos     = 0;
    let mut in_curl = false;

    while pos < cmd.len() {
        let start = pos + (cmd[pos..].len() - cmd[pos..].trim_start().len());
        if start >= cmd.len() {
            break;
        }
        let end = cmd[start..]
            .find(|c: char| c.is_ascii_whitespace())
            .map_or(cmd.len(), |n| start + n);
        let token = &cmd[start..end];
        let mut next = end;

        let program = token
            .trim_matches(|c| matches!(c, '"' | '\''))
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("");
        let program = program.strip_suffix(".exe").unwrap_or(program);
        if program.eq_ignore_ascii_case("curl") {
            in_curl = true;
        } else if in_curl {
            let separate = matches!(token, "--user" | "--proxy-user")
                || (token.len() >= 2
                    && token.starts_with('-')
                    && !token.starts_with("--")
                    && token[1..].bytes().all(|b| b.is_ascii_alphabetic())
                    && (token.ends_with('u') || token.ends_with('U')));
            let attached = ["--user=", "--proxy-user="]
                .iter()
                .find(|p| token.starts_with(**p))
                .map(|p| start + p.len())
                .or_else(|| {
                    (token.len() > 2 && (token.starts_with("-u") || token.starts_with("-U")) && token.contains(':'))
                        .then_some(start + 2)
                });
            let val_start = if separate {
                let rest = &cmd[end..];
                Some(end + rest.len() - rest.trim_start().len())
            } else {
                attached
            };
            if let Some(vs) = val_start.filter(|vs| *vs < cmd.len()) {
                let (from, to, resume) = value_span(cmd, vs);
                if let Some(colon) = cmd[from..to].find(':') {
                    let secret = from + colon + 1;
                    if secret < to {
                        result.push_str(&cmd[last..secret]);
                        result.push_str("[REDACTED]");
                        last = to;
                    }
                }
                next = next.max(resume);
            }
        }

        if matches!(token, "|" | "||" | "&" | "&&" | ";") || token.ends_with(';') {
            in_curl = false;
        }
        pos = next;
    }

    result.push_str(&cmd[last..]);
    result
}

/// Redacts `KEY=[REDACTED]` where KEY ends with a sensitive suffix.
/// Matches at word boundaries (start of string or preceded by whitespace).
/// Suffixes checked: `_KEY`, `_SECRET`, `_TOKEN`, `_PASSWORD`, `_PASS`.
///
/// Example: `MY_API_KEY=abc123 command` → `MY_API_KEY=[REDACTED] command`
fn redact_env_assignments(cmd: &str) -> String {
    const SENSITIVE_SUFFIXES: &[&str] = &["_KEY", "_SECRET", "_TOKEN", "_PASSWORD", "_PASS"];

    let mut result = String::with_capacity(cmd.len());
    let mut last   = 0;
    let bytes      = cmd.as_bytes();
    let mut i      = 0;

    while i < bytes.len() {
        if bytes[i] != b'=' {
            i += 1;
            continue;
        }

        // Walk back to find the key start (last whitespace before this '=').
        let key_start = cmd[..i]
            .rfind(|c: char| c.is_ascii_whitespace())
            .map(|p| p + 1)
            .unwrap_or(0);

        let at_boundary = key_start == 0
            || (key_start > 0 && bytes[key_start - 1].is_ascii_whitespace());

        let key = &cmd[key_start..i];
        let key_valid = !key.is_empty()
            && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        let key_upper = key.to_uppercase();
        let is_sensitive = SENSITIVE_SUFFIXES.iter().any(|s| key_upper.ends_with(s));

        if at_boundary && key_valid && is_sensitive {
            let val_start = i + 1;
            let val_len   = cmd[val_start..]
                .find(|c: char| c.is_ascii_whitespace())
                .unwrap_or(cmd.len() - val_start);

            result.push_str(&cmd[last..val_start]);
            result.push_str("[REDACTED]");
            last = val_start + val_len;
            i    = last;
        } else {
            i += 1;
        }
    }

    result.push_str(&cmd[last..]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn shell_input(command: &str, timeout_secs: u64) -> NodeInput {
        NodeInput {
            node_id: "n".to_string(),
            workflow_id: "wf".to_string(),
            execution_id: "exec".to_string(),
            input: json!({ "command": command, "timeout_secs": timeout_secs }),
            resolved_credentials: std::collections::HashMap::new(),
            context: crate::model::ExecutionContext::default(),
            cancel_token: None,
        }
    }

    #[cfg(unix)]
    fn pid_is_running(pid: i32) -> bool {
        if unsafe { libc::kill(pid, 0) } != 0 {
            return false;
        }
        match std::fs::read_to_string(format!("/proc/{}/stat", pid)) {
            Ok(stat) => stat
                .rsplit(')')
                .next()
                .and_then(|rest| rest.trim_start().chars().next())
                != Some('Z'),
            Err(_) => true,
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dropping_shell_node_future_kills_backgrounded_descendants() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("bg.pid");
        let input = shell_input(
            &format!("sleep 30 & echo $! > '{}'; wait", pid_file.display()),
            60,
        );

        let outcome = tokio::time::timeout(
            std::time::Duration::from_millis(700),
            ShellExecNode.execute(input),
        )
        .await;
        assert!(outcome.is_err(), "command should still be running when the future is dropped");

        let bg_pid: i32 = std::fs::read_to_string(&pid_file)
            .expect("background pid file written")
            .trim()
            .parse()
            .unwrap();
        let mut gone = false;
        for _ in 0..40 {
            if !pid_is_running(bg_pid) {
                gone = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        if !gone {
            unsafe { libc::kill(bg_pid, libc::SIGKILL); }
        }
        assert!(gone, "backgrounded descendant survived the node future being dropped");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shell_node_times_out_when_a_backgrounded_process_holds_the_output_pipe() {
        let started = std::time::Instant::now();
        let out = ShellExecNode.execute(shell_input("sleep 30 &", 1)).await;
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        assert!(!out.success);
        let err = out.error.expect("expected NodeError");
        assert_eq!(err.code, "TIMEOUT");
        assert!(!err.recoverable, "a timed-out command must not be retried");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_of_zero_is_raised_to_one_second() {
        let out = ShellExecNode.execute(shell_input("sleep 0.3; echo done", 0)).await;
        assert!(out.success, "{:?}", out.error);
        assert_eq!(out.output.unwrap()["stdout"], "done\n");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn output_beyond_the_cap_is_drained_and_flagged_truncated() {
        let started = std::time::Instant::now();
        let out = ShellExecNode
            .execute(shell_input("head -c 11000000 /dev/zero | tr '\\0' x", 30))
            .await;
        assert!(out.success, "{:?}", out.error);
        assert!(started.elapsed() < std::time::Duration::from_secs(20));
        let o = out.output.unwrap();
        assert_eq!(o["stdout"].as_str().unwrap().len(), OUTPUT_CAP_BYTES);
        assert_eq!(o["truncated"], true);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn endless_output_is_stopped_with_output_flood_before_the_timeout() {
        let started = std::time::Instant::now();
        let out = ShellExecNode.execute(shell_input("yes", 60)).await;
        let err = out.error.expect("expected NodeError");
        assert_eq!(err.code, "OUTPUT_FLOOD");
        assert!(!err.recoverable);
        assert!(started.elapsed() < std::time::Duration::from_secs(45));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failure_message_shows_only_the_end_of_a_huge_stderr() {
        let started = std::time::Instant::now();
        let out = ShellExecNode
            .execute(shell_input("yes '://' | head -c 5000000 >&2; echo 'real error' >&2; exit 1", 30))
            .await;
        let err = out.error.expect("expected NodeError");
        assert!(started.elapsed() < std::time::Duration::from_secs(20));
        assert!(err.message.len() < 5_000, "len {}", err.message.len());
        assert!(err.message.contains("[earlier output omitted]"));
        assert!(err.message.trim_end().ends_with("real error"), "got tail: {}", &err.message[err.message.len() - 40..]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failure_message_redacts_secrets_echoed_on_stderr() {
        let out = ShellExecNode
            .execute(shell_input("echo 'cannot reach postgres://app:hunter2@db/prod' >&2; exit 3", 10))
            .await;
        let err = out.error.expect("expected NodeError");
        assert_eq!(err.code, "COMMAND_FAILED");
        assert!(err.message.contains("Exit code 3"), "got: {}", err.message);
        assert!(!err.message.contains("hunter2"), "got: {}", err.message);
        assert!(err.message.contains("[REDACTED]"), "got: {}", err.message);
    }

    // ── URL credential redaction ────────────────────────────────────────────

    // Exercises the shared `scrub_url_in_error` redactor that `redact_command_log` delegates to.
    #[test]
    fn url_credentials_redacted() {
        let cmd = "mysql -h mysql://user:pass@host:3306/db";
        let out = scrub_url_in_error(cmd);
        assert_eq!(out, "mysql -h mysql://user:[REDACTED]@host:3306/db");
    }

    #[test]
    fn postgres_credentials_redacted() {
        let cmd = "pg_dump postgres://admin:secret@localhost/mydb";
        let out = scrub_url_in_error(cmd);
        assert_eq!(out, "pg_dump postgres://admin:[REDACTED]@localhost/mydb");
    }

    #[test]
    fn url_without_credentials_unchanged() {
        let cmd = "curl https://api.example.com/v1/data";
        let out = scrub_url_in_error(cmd);
        assert_eq!(out, cmd);
    }

    // ── Flag-value redaction ────────────────────────────────────────────────

    #[test]
    fn password_flag_redacted() {
        let cmd = "mysql --password=secret -h localhost";
        let out = redact_after_prefix(cmd, "--password=");
        assert_eq!(out, "mysql --password=[REDACTED] -h localhost");
    }

    #[test]
    fn password_flag_end_of_string_redacted() {
        let cmd = "mysql --password=secret";
        let out = redact_after_prefix(cmd, "--password=");
        assert_eq!(out, "mysql --password=[REDACTED]");
    }

    #[test]
    fn short_p_flag_redacts_only_where_it_carries_a_password() {
        let redacted = [
            ("sshpass -p hunter2 ssh host uptime", "sshpass -p [REDACTED] ssh host uptime"),
            ("mysqldump -u root -phunter2 db > out.sql", "mysqldump -u root -p[REDACTED] db > out.sql"),
            ("/usr/bin/mysql -p'hunter 2' db", "/usr/bin/mysql -p'[REDACTED]' db"),
            ("cat q.sql | MYSQL -phunter2", "cat q.sql | MYSQL -p[REDACTED]"),
        ];
        for (cmd, want) in redacted {
            assert_eq!(redact_command_log(cmd), want);
        }
        let unchanged = [
            "mkdir -p /srv/app/logs",
            "ls -p /tmp",
            "ssh -p 2222 host",
            "ssh -p22 host",
            "mysql -p mydb",
            "mysql -u root -p",
            "mysql -u root ; ssh -p22 host",
        ];
        for cmd in unchanged {
            assert_eq!(redact_command_log(cmd), cmd);
        }
    }

    #[test]
    fn secret_flags_are_redacted_in_equals_and_space_forms() {
        let redacted = [
            ("cli --password hunter2 run", "cli --password [REDACTED] run"),
            ("cli --password=hunter2 run", "cli --password=[REDACTED] run"),
            ("gpg --passphrase s3cret --batch", "gpg --passphrase [REDACTED] --batch"),
            ("gpg --passphrase=s3cret --batch", "gpg --passphrase=[REDACTED] --batch"),
            ("cli --token 'a b c' go", "cli --token '[REDACTED]' go"),
            ("cli --secret=\"x y\" go", "cli --secret=\"[REDACTED]\" go"),
            ("cli --Client-Secret abc", "cli --Client-Secret [REDACTED]"),
            ("wget --http-password pw url", "wget --http-password [REDACTED] url"),
        ];
        for (cmd, want) in redacted {
            assert_eq!(redact_command_log(cmd), want);
        }
        let unchanged = [
            "cli --password-file /run/pw",
            "cli --token",
            "cli --token --verbose",
            "cli --password= run",
            "ssh --key id.pem host",
            "cli --passes 3",
        ];
        for cmd in unchanged {
            assert_eq!(redact_command_log(cmd), cmd);
        }
    }

    #[test]
    fn curl_user_credentials_lose_the_password_but_keep_the_user_name() {
        let redacted = [
            ("curl -u admin:s3cret https://x.test", "curl -u admin:[REDACTED] https://x.test"),
            ("curl --user admin:s3cret https://x.test", "curl --user admin:[REDACTED] https://x.test"),
            ("curl --user=admin:s3cret https://x.test", "curl --user=admin:[REDACTED] https://x.test"),
            ("curl -uadmin:s3cret https://x.test", "curl -uadmin:[REDACTED] https://x.test"),
            ("curl -sSu admin:s3cret https://x.test", "curl -sSu admin:[REDACTED] https://x.test"),
            ("curl -u 'admin:pa ss' https://x.test", "curl -u 'admin:[REDACTED]' https://x.test"),
            ("curl --proxy-user p:q -x host url", "curl --proxy-user p:[REDACTED] -x host url"),
            ("/usr/bin/curl -u a:b url | wc", "/usr/bin/curl -u a:[REDACTED] url | wc"),
        ];
        for (cmd, want) in redacted {
            assert_eq!(redact_command_log(cmd), want);
        }
        let unchanged = [
            "docker run -u 1000:1000 img",
            "curl -u admin https://x.test",
            "curl -sS -o out.txt https://x.test",
            "curl url | docker run -u 1000:1000 img",
        ];
        for cmd in unchanged {
            assert_eq!(redact_command_log(cmd), cmd);
        }
    }

    #[test]
    fn token_flag_redacted() {
        let cmd = "cli --token=abc123 list";
        let out = redact_after_prefix(cmd, "--token=");
        assert_eq!(out, "cli --token=[REDACTED] list");
    }

    #[test]
    fn unrelated_flag_unchanged() {
        let cmd = "ls --color=auto /home";
        let out = redact_after_prefix(cmd, "--password=");
        assert_eq!(out, cmd);
    }

    // 'İ' (U+0130) is 2 bytes in `cmd` but Unicode-aware `to_lowercase()` expands
    // it to the 3-byte "i̇" -- byte offsets computed from a Unicode-lowered copy
    // would then misalign with `cmd`, which is indexed directly.
    // `to_ascii_lowercase()` leaves 'İ' untouched (non-ASCII), keeping both
    // strings byte-for-byte aligned throughout.
    #[test]
    fn unicode_before_flag_value_does_not_misalign_or_panic() {
        let cmd = "echo İ && mysql --password=secret -h localhost";
        let out = redact_after_prefix(cmd, "--password=");
        assert_eq!(out, "echo İ && mysql --password=[REDACTED] -h localhost");
    }

    // ── Env assignment redaction ────────────────────────────────────────────

    #[test]
    fn env_secret_assignment_redacted() {
        let cmd = "MY_SECRET=abc123 command --flag";
        let out = redact_env_assignments(cmd);
        assert_eq!(out, "MY_SECRET=[REDACTED] command --flag");
    }

    #[test]
    fn env_api_key_assignment_redacted() {
        let cmd = "MY_API_KEY=sk-abc curl https://api.example.com";
        let out = redact_env_assignments(cmd);
        assert_eq!(out, "MY_API_KEY=[REDACTED] curl https://api.example.com");
    }

    #[test]
    fn env_non_sensitive_assignment_unchanged() {
        let cmd = "LOG_LEVEL=debug command";
        let out = redact_env_assignments(cmd);
        assert_eq!(out, cmd);
    }

    // ── Authorization header redaction ──────────────────────────────────────

    #[test]
    fn authorization_header_redacted() {
        let cmd = "curl -H \"Authorization: Bearer sk-abc123\" https://api.example.com";
        let out = redact_authorization_header(cmd);
        assert!(out.contains("[REDACTED]"), "expected redaction, got: {}", out);
        assert!(!out.contains("sk-abc123"), "secret should be gone, got: {}", out);
        // URL that follows the header must not be swallowed.
        assert!(out.contains("https://api.example.com"), "URL must be preserved, got: {}", out);
        assert_eq!(out, "curl -H \"Authorization: [REDACTED]\" https://api.example.com");
    }

    #[test]
    fn single_quoted_authorization_header_is_redacted() {
        let out = redact_authorization_header("curl -H 'Authorization: Bearer sk-abc123' https://x");
        assert_eq!(out, "curl -H 'Authorization: [REDACTED]' https://x");
    }

    #[test]
    fn unquoted_authorization_header_redacts_scheme_and_token_only() {
        let out = redact_authorization_header("curl -H Authorization: Bearer sk-abc123 https://x");
        assert!(!out.contains("sk-abc123"), "token must be gone, got: {}", out);
        assert!(out.ends_with(" https://x"), "trailing args must survive, got: {}", out);
    }

    // Same Unicode/ASCII alignment concern as
    // unicode_before_flag_value_does_not_misalign_or_panic above, applied to
    // redact_authorization_header: a byte offset from a Unicode-lowered copy
    // can slice `lower` off a char boundary and panic. Must not panic, and
    // must still redact correctly with unrelated Unicode content preserved.
    #[test]
    fn unicode_before_authorization_header_does_not_panic_and_still_redacts() {
        let cmd = "echo İ && curl -H \"Authorization: Bearer sk-abc123\" https://x";
        let out = redact_authorization_header(cmd);
        assert!(out.contains("[REDACTED]"), "expected redaction, got: {}", out);
        assert!(!out.contains("sk-abc123"), "secret should be gone, got: {}", out);
        assert!(out.contains('İ'), "unrelated unicode content must be preserved, got: {}", out);
    }

    // ── Full pipeline ───────────────────────────────────────────────────────

    #[test]
    fn pipeline_no_secrets_unchanged() {
        let cmd = "ls -la /home/user";
        assert_eq!(redact_command_log(cmd), cmd);
    }

    #[test]
    fn pipeline_url_creds_redacted() {
        let out = redact_command_log("mysqldump mysql://admin:hunter2@localhost/prod");
        assert!(out.contains("[REDACTED]"));
        assert!(!out.contains("hunter2"));
    }

    #[test]
    fn pipeline_password_flag_redacted() {
        let out = redact_command_log("pg_dump --password=topsecret -h localhost");
        assert!(out.contains("[REDACTED]"));
        assert!(!out.contains("topsecret"));
    }

    #[test]
    fn pipeline_truncation_applied() {
        let long_cmd = "echo ".to_string() + &"x".repeat(600);
        let out = redact_command_log(&long_cmd);
        assert!(out.ends_with("[truncated]"));
        assert!(out.len() <= 520); // "... [truncated]" = 15 chars, 500 + 15
    }

    #[test]
    fn truncation_does_not_split_multibyte_char() {
        // Byte 500 falls inside the 2-byte 'é' that starts at byte 499.
        let long_cmd = format!("a{}", "é".repeat(400));
        let out = redact_command_log(&long_cmd);
        assert!(out.ends_with("[truncated]"));
    }
}

