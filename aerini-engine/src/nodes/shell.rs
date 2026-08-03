use async_trait::async_trait;
use serde_json::{json, Value};
use std::process::Stdio;
use tokio::process::Command;

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::Node;

use super::util::scrub_url_in_error;

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
                "timeout_secs": { "type": "number", "default": 30 }
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
                "success":     { "type": "boolean" }
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

        let timeout_secs = input.input["timeout_secs"].as_u64().unwrap_or(30);

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

        // Move pipes into tasks so we can read stdout/stderr concurrently with wait().
        // Required: if the child fills the OS pipe buffer (~64 KB) and nothing is reading,
        // it blocks forever — wait() would never return.
        // child.wait() takes &mut self (not self), so child remains owned here for kill().
        const MAX_OUTPUT_BYTES: u64 = 10 * 1024 * 1024; // 10 MB per stream
        let stdout_task = tokio::spawn({
            use tokio::io::AsyncReadExt;
            let mut pipe = child.stdout.take();
            async move {
                let mut buf = Vec::new();
                if let Some(ref mut h) = pipe {
                    let _ = h.take(MAX_OUTPUT_BYTES).read_to_end(&mut buf).await;
                }
                buf
            }
        });
        let stderr_task = tokio::spawn({
            use tokio::io::AsyncReadExt;
            let mut pipe = child.stderr.take();
            async move {
                let mut buf = Vec::new();
                if let Some(ref mut h) = pipe {
                    let _ = h.take(MAX_OUTPUT_BYTES).read_to_end(&mut buf).await;
                }
                buf
            }
        });

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            child.wait(),
        ).await;

        match result {
            Err(_) => {
                let _ = child.kill().await;
                stdout_task.abort();
                stderr_task.abort();
                NodeOutput::failure(
                    NodeError::recoverable("TIMEOUT", format!("Command timed out after {}s", timeout_secs))
                )
            }
            Ok(Err(e)) => {
                stdout_task.abort();
                stderr_task.abort();
                NodeOutput::failure(NodeError::unrecoverable("EXEC_ERROR", e.to_string()))
            }
            Ok(Ok(status)) => {
                let stdout_bytes = stdout_task.await.unwrap_or_default();
                let stderr_bytes = stderr_task.await.unwrap_or_default();
                let stdout    = String::from_utf8_lossy(&stdout_bytes).to_string();
                let stderr    = String::from_utf8_lossy(&stderr_bytes).to_string();
                let exit_code = status.code().unwrap_or(-1);
                let success   = status.success();

                let logs = vec![
                    format!("Command: {}", redact_command_log(&command)),
                    format!("Exit code: {}", exit_code),
                ];

                if success {
                    NodeOutput::success_with_logs(
                        json!({ "stdout": stdout, "stderr": stderr, "exit_code": exit_code, "success": true }),
                        logs,
                    )
                } else {
                    NodeOutput::failure_with_logs(
                        NodeError::unrecoverable(
                            "COMMAND_FAILED",
                            format!("Exit code {}: {}", exit_code, stderr),
                        ),
                        logs,
                    )
                }
            }
        }
    }
}

/// Redacts sensitive patterns from a shell command before writing to run history.
/// Applied in order: Authorization headers → URL credentials → named flags → env assignments.
/// URL-credential redaction delegates to the shared `scrub_url_in_error` so the
/// two code paths can't drift apart.
/// Truncates commands over 500 chars after redaction.
fn redact_command_log(cmd: &str) -> String {
    let s = redact_authorization_header(cmd);
    let s = scrub_url_in_error(&s);
    let s = redact_after_prefix(&s, "--password=");
    let s = redact_after_prefix(&s, "--passwd=");
    let s = redact_after_prefix(&s, "--pass=");
    let s = redact_after_prefix(&s, "--token=");
    let s = redact_after_prefix(&s, "--secret=");
    let s = redact_after_prefix(&s, "--api-key=");
    let s = redact_after_prefix(&s, "--api_key=");
    let s = redact_after_prefix(&s, "--key=");
    let s = redact_after_prefix(&s, "-p ");
    let s = redact_env_assignments(&s);
    if s.len() > 500 {
        format!("{}... [truncated]", &s[..500])
    } else {
        s
    }
}

/// Redacts `Authorization:` header values. Handles both plain and quoted forms.
/// Example: `-H "Authorization: Bearer sk-abc"` → `-H "Authorization: [REDACTED]"`
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
            let rest = &cmd[i + 14..];
            // Skip the authorization value:
            // - If a closing quote is present, consume up to and including it
            //   (handles `-H "Authorization: Bearer token"` patterns).
            // - Otherwise, consume to the next whitespace or end of string.
            // Do NOT use cmd.len()-i as the fallback — that would swallow
            // everything that follows (e.g., the URL after the header arg).
            let skip = if let Some(q) = rest.find('"') {
                14 + q + 1
            } else {
                rest.find(|c: char| c.is_ascii_whitespace())
                    .map(|s| 14 + s)
                    .unwrap_or(14 + rest.len())
            };
            i += skip;
        } else {
            let ch = cmd[i..].chars().next().expect("valid UTF-8 offset");
            result.push(ch);
            i += ch.len_utf8();
        }
    }
    result
}

/// Redacts the value that immediately follows `prefix` (case-insensitive).
/// Matches only at word boundaries (start of string or preceded by whitespace/quote).
/// Value ends at the next whitespace, quote, or end of string.
///
/// Used for flags like `--password=VALUE` (prefix = `--password=`)
/// and short flags like `-p VALUE` (prefix = `-p `, space included).
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
    fn short_p_flag_redacted() {
        let cmd = "mysql -p mypassword -h localhost";
        let out = redact_after_prefix(cmd, "-p ");
        // val_len stops at the space → " -h localhost" preserved intact
        assert_eq!(out, "mysql -p [REDACTED] -h localhost");
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
}

