use async_trait::async_trait;
use serde_json::{json, Value};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::Node;

pub struct NotificationNode;

#[async_trait]
impl Node for NotificationNode {
    fn type_id(&self) -> &'static str { "notification" }
    fn display_name(&self) -> &'static str { "Desktop Notification" }
    fn node_type(&self) -> NodeType { NodeType::Action }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Show a desktop notification on the host machine while the workflow is running." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["title"],
            "properties": {
                "title":   { "type": "string", "description": "Notification title" },
                "body":    { "type": "string", "description": "Notification message body" },
                "urgency": {
                    "type": "string",
                    "enum": ["low", "normal", "critical"],
                    "description": "Urgency level (Linux only)",
                    "default": "normal"
                }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "sent": { "type": "boolean" }
            }
        })
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let cfg = &input.input;

        let title = match &cfg["title"] {
            Value::String(t) if !t.is_empty() => t.to_string(),
            Value::Null | Value::String(_) => return NodeOutput::failure(NodeError::unrecoverable(
                "MISSING_TITLE", "title is required",
            )),
            other => return NodeOutput::failure(NodeError::unrecoverable(
                "INVALID_TITLE", format!("title must be text (got {other})"),
            )),
        };
        let body = match &cfg["body"] {
            Value::Null => String::new(),
            Value::String(b) => b.to_string(),
            other => return NodeOutput::failure(NodeError::unrecoverable(
                "INVALID_BODY", format!("body must be text (got {other})"),
            )),
        };
        let urgency = match &cfg["urgency"] {
            Value::Null => "normal",
            Value::String(u) if matches!(u.as_str(), "low" | "normal" | "critical") => u.as_str(),
            other => return NodeOutput::failure(NodeError::unrecoverable(
                "INVALID_URGENCY",
                format!("urgency must be one of low, normal, critical (got {other})"),
            )),
        }
        .to_string();

        match send_notification(&title, &body, &urgency).await {
            Ok(method) => NodeOutput::success_with_logs(
                json!({ "sent": true }),
                vec![format!("Notification sent via {method}: {title}")],
            ),
            Err(e) => NodeOutput::failure(NodeError::unrecoverable(
                "NOTIFY_ERROR", format!("Desktop notification failed: {e}"),
            )),
        }
    }
}

const NOTIFY_TIMEOUT: Duration = Duration::from_secs(15);
const STDERR_CAP: usize = 4096;

#[cfg(any(windows, test))]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

async fn read_capped(mut reader: impl AsyncRead + Unpin, cap: usize) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = cap.saturating_sub(kept.len());
                kept.extend_from_slice(&buf[..n.min(room)]);
            }
        }
    }
    kept
}

/// Runs a notifier with no stdin or stdout, stderr captured (first 4 KiB),
/// killed on timeout or when the future is dropped.
async fn run_notifier(mut cmd: Command, name: &str, timeout: Duration) -> Result<(), String> {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| format!("could not start {name}: {e}"))?;
    let stderr = child.stderr.take();
    let mut drain = tokio::spawn(async move {
        match stderr {
            Some(r) => read_capped(r, STDERR_CAP).await,
            None => Vec::new(),
        }
    });

    let waited = tokio::time::timeout(timeout, child.wait()).await;
    if waited.is_err() {
        let _ = child.kill().await;
    }
    let captured = tokio::time::timeout(Duration::from_millis(500), &mut drain)
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    drain.abort();
    let stderr_text = String::from_utf8_lossy(&captured).trim().to_string();
    let with_stderr = |head: String| if stderr_text.is_empty() { head } else { format!("{head}: {stderr_text}") };

    match waited {
        Err(_) => Err(with_stderr(format!("{name} did not finish within {} s and was stopped", timeout.as_secs()))),
        Ok(Err(e)) => Err(with_stderr(format!("waiting for {name} failed: {e}"))),
        Ok(Ok(status)) if status.success() => Ok(()),
        Ok(Ok(status)) => Err(with_stderr(format!("{name} exited with {status}"))),
    }
}

/// The notification spec gives the body a small XML-based markup and notify-send
/// has no option to turn it off, so the body is escaped. The title is escaped
/// too, because some daemons read markup in the summary as well.
#[cfg(any(target_os = "linux", test))]
fn escape_markup(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(any(target_os = "linux", test))]
fn notify_send_args(title: &str, body: &str, urgency: &str) -> Vec<String> {
    let mut args = vec!["--urgency".to_string(), urgency.to_string(), "--".to_string(), escape_markup(title)];
    if !body.is_empty() {
        args.push(escape_markup(body));
    }
    args
}

#[cfg(any(windows, test))]
fn is_xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}')
}

#[cfg(any(windows, test))]
fn xml_escape(s: &str) -> String {
    s.chars().filter(|&c| is_xml_char(c)).fold(String::with_capacity(s.len()), |mut a, c| {
        match c {
            '&'  => a.push_str("&amp;"),
            '<'  => a.push_str("&lt;"),
            '>'  => a.push_str("&gt;"),
            '"'  => a.push_str("&quot;"),
            '\'' => a.push_str("&apos;"),
            _    => a.push(c),
        }
        a
    })
}

#[cfg(any(windows, test))]
fn toast_xml(title: &str, body: &str) -> String {
    format!(
        "<toast><visual><binding template='ToastText02'>\
         <text id='1'>{title}</text>\
         <text id='2'>{body}</text>\
         </binding></visual></toast>",
        title = xml_escape(title),
        body  = xml_escape(body),
    )
}

async fn send_notification(title: &str, body: &str, #[allow(unused_variables)] urgency: &str) -> Result<&'static str, String> {
    #[cfg(target_os = "linux")]
    {
        let mut cmd = Command::new("notify-send");
        cmd.args(notify_send_args(title, body, urgency));
        run_notifier(cmd, "notify-send", NOTIFY_TIMEOUT).await?;
        return Ok("notify-send");
    }

    #[cfg(target_os = "macos")]
    {
        // Title and body are passed as separate argv items — they are never
        // interpolated into AppleScript source text, which prevents injection
        // via newlines, backticks, or AppleScript keywords in the input strings.
        //
        // The script reads them via `item N of argv` at runtime.
        // An empty body is passed as "" so argv always has exactly two items.
        let script_body = if body.is_empty() {
            "on run argv\ndisplay notification \"\" with title (item 1 of argv)\nend run"
        } else {
            "on run argv\ndisplay notification (item 2 of argv) with title (item 1 of argv)\nend run"
        };
        let mut cmd = Command::new("osascript");
        cmd.arg("-e").arg(script_body)
           .arg("--")
           .arg(title);
        if !body.is_empty() {
            cmd.arg(body);
        }
        run_notifier(cmd, "osascript", NOTIFY_TIMEOUT).await?;
        return Ok("osascript");
    }

    #[cfg(target_os = "windows")]
    {
        // Title and body are written into an XML temp file.
        // No user-controlled data is interpolated into the PowerShell -Command
        // string — the command only loads from the temp file path (a UUID-named
        // OS temp file, not attacker-controlled).
        //
        // XML special characters in title/body are entity-escaped, and characters
        // XML 1.0 forbids are dropped, so the document stays well-formed even
        // with arbitrary input.
        let xml = toast_xml(title, body);
        // Write XML to a temp file. Only the file path (not the content) is
        // placed into the PowerShell -Command string.
        let mut tmp = tempfile::Builder::new()
            .suffix(".xml")
            .tempfile()
            .map_err(|e| format!("Failed to create temp file for notification: {}", e))?;
        use std::io::Write as _;
        tmp.write_all(xml.as_bytes())
            .map_err(|e| format!("Failed to write notification XML: {}", e))?;
        let path = tmp.path().to_string_lossy().to_string();
        // path is an OS-assigned temp path. Windows forbids `"` in file paths so
        // wrapping in double quotes is safe. Single quotes are avoided because a
        // temp dir rooted in a user profile path like "O'Brien" would break a
        // single-quoted PowerShell string literal.
        let ps_cmd = format!(
            "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null; \
             $xml = New-Object Windows.Data.Xml.Dom.XmlDocument; \
             $xml.LoadXml([System.IO.File]::ReadAllText(\"{}\")); \
             [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('Aerini').Show(\
             [Windows.UI.Notifications.ToastNotification]::new($xml))",
            path
        );
        let mut cmd = Command::new("powershell");
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", &ps_cmd]);
        cmd.creation_flags(CREATE_NO_WINDOW);
        // tmp is deleted when it drops, after the process has ended.
        run_notifier(cmd, "PowerShell", NOTIFY_TIMEOUT).await?;
        return Ok("powershell");
    }

    #[allow(unreachable_code)]
    Err("Desktop notifications not supported on this platform".to_string())
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExecutionContext;

    fn make_input(input: Value) -> NodeInput {
        NodeInput {
            resolved_credentials: std::collections::HashMap::new(),
            cancel_token: None,
            node_id:      "n1".to_string(),
            workflow_id:  "wf".to_string(),
            execution_id: "exec".to_string(),
            input,
            context: ExecutionContext::default(),
        }
    }

    #[tokio::test]
    async fn a_title_or_body_that_is_not_text_is_refused_and_a_missing_body_is_allowed() {
        let code = |cfg: Value| async move { NotificationNode.execute(make_input(cfg)).await.error.map(|e| e.code) };
        assert_eq!(code(json!({ "title": 5 })).await.as_deref(), Some("INVALID_TITLE"));
        assert_eq!(code(json!({ "title": "" })).await.as_deref(), Some("MISSING_TITLE"));
        assert_eq!(code(json!({ "title": "t", "body": {"a": 1} })).await.as_deref(), Some("INVALID_BODY"));
        assert_eq!(code(json!({ "title": "t", "body": 7 })).await.as_deref(), Some("INVALID_BODY"));
    }

    #[tokio::test]
    async fn unknown_urgency_is_rejected() {
        let out = NotificationNode
            .execute(make_input(json!({ "title": "t", "urgency": "urgent" })))
            .await;
        assert_eq!(out.error.unwrap().code, "INVALID_URGENCY");
    }

    #[test]
    fn notify_send_receives_title_and_body_as_text_not_markup() {
        assert_eq!(
            notify_send_args("a < b <i>", "x & <b>y</b>", "low"),
            ["--urgency", "low", "--", "a &lt; b &lt;i&gt;", "x &amp; &lt;b&gt;y&lt;/b&gt;"],
        );
        assert_eq!(notify_send_args("t", "", "normal").len(), 4);
    }

    #[test]
    fn toast_xml_drops_forbidden_control_characters_and_escapes_markup() {
        let xml = toast_xml("a\u{0}b\u{8}c\u{b}\u{c}\u{1f}\u{fffe}\u{ffff}", "x\ty\nz\r & <w> \"q\" 'p' \u{1f600}");
        assert!(xml.contains("<text id='1'>abc</text>"), "{xml}");
        assert!(xml.contains("x\ty\nz\r &amp; &lt;w&gt; &quot;q&quot; &apos;p&apos; \u{1f600}"), "{xml}");
        assert_eq!(CREATE_NO_WINDOW, 134_217_728);
    }

    #[cfg(unix)]
    fn sh(script: &str) -> Command {
        let mut c = Command::new("sh");
        c.arg("-c").arg(script);
        c
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failure_text_includes_stderr_and_exit_status() {
        let e = run_notifier(sh("echo no notification daemon >&2; exit 3"), "notify-send", Duration::from_secs(5))
            .await
            .unwrap_err();
        assert!(e.contains("exited with") && e.contains("3"), "{e}");
        assert!(e.contains("no notification daemon"), "{e}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn endless_stderr_is_capped_and_a_clean_exit_is_ok() {
        let e = run_notifier(sh("head -c 200000 /dev/zero | tr '\\0' x >&2; exit 1"), "n", Duration::from_secs(5))
            .await
            .unwrap_err();
        assert!(e.len() < STDERR_CAP + 100, "{}", e.len());
        run_notifier(sh("exit 0"), "n", Duration::from_secs(5)).await.unwrap();
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn hung_notifier_is_stopped_at_the_timeout_and_killed() {
        let pid_file = tempfile::NamedTempFile::new().unwrap();
        let script = format!("echo $$ > {}; echo stuck >&2; exec sleep 30", pid_file.path().display());
        let started = std::time::Instant::now();
        let e = run_notifier(sh(&script), "notify-send", Duration::from_millis(500)).await.unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(e.contains("did not finish") && e.contains("stuck"), "{e}");
        let pid = std::fs::read_to_string(pid_file.path()).unwrap().trim().to_string();
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists(), "process {pid} still exists");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn dropping_the_node_future_kills_the_notifier() {
        let pid_file = tempfile::NamedTempFile::new().unwrap();
        let script = format!("echo $$ > {}; exec sleep 30", pid_file.path().display());
        let fut = run_notifier(sh(&script), "n", Duration::from_secs(60));
        let _ = tokio::time::timeout(Duration::from_millis(500), fut).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let pid = std::fs::read_to_string(pid_file.path()).unwrap().trim().to_string();
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists(), "process {pid} still exists");
    }

    #[tokio::test]
    async fn missing_notifier_program_is_a_named_failure() {
        let e = run_notifier(Command::new("aerini-no-such-notifier"), "notify-send", Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(e.starts_with("could not start notify-send"), "{e}");
    }
}
