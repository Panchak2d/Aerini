use async_trait::async_trait;
use serde_json::{json, Value};
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

        let title = match cfg["title"].as_str().filter(|s| !s.is_empty()) {
            Some(t) => t.to_string(),
            None => return NodeOutput::failure(NodeError::unrecoverable(
                "MISSING_TITLE", "title is required",
            )),
        };
        let body    = cfg["body"].as_str().unwrap_or("").to_string();
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

async fn send_notification(title: &str, body: &str, #[allow(unused_variables)] urgency: &str) -> Result<&'static str, String> {
    #[cfg(target_os = "linux")]
    {
        let mut cmd = Command::new("notify-send");
        cmd.arg("--urgency").arg(urgency);
        cmd.arg("--");
        cmd.arg(title);
        if !body.is_empty() {
            cmd.arg(body);
        }
        let status = cmd.status().await.map_err(|e| e.to_string())?;
        if status.success() {
            return Ok("notify-send");
        }
        return Err(format!("notify-send exited with {status}"));
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
        let status = cmd.status().await.map_err(|e| e.to_string())?;
        if status.success() {
            return Ok("osascript");
        }
        return Err(format!("osascript exited with {status}"));
    }

    #[cfg(target_os = "windows")]
    {
        // Title and body are written into an XML temp file.
        // No user-controlled data is interpolated into the PowerShell -Command
        // string — the command only loads from the temp file path (a UUID-named
        // OS temp file, not attacker-controlled).
        //
        // XML special characters in title/body are entity-escaped so the XML
        // document remains well-formed even with arbitrary input.
        fn xml_escape(s: &str) -> String {
            s.chars().fold(String::with_capacity(s.len()), |mut a, c| {
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
        let xml = format!(
            "<toast><visual><binding template='ToastText02'>\
             <text id='1'>{title}</text>\
             <text id='2'>{body}</text>\
             </binding></visual></toast>",
            title = xml_escape(title),
            body  = xml_escape(body),
        );
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
        let status = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &ps_cmd])
            .status().await.map_err(|e| e.to_string())?;
        // tmp drops here, deleting the file.
        if status.success() {
            return Ok("powershell");
        }
        return Err(format!("PowerShell notification exited with {status}"));
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
    async fn unknown_urgency_is_rejected() {
        let out = NotificationNode
            .execute(make_input(json!({ "title": "t", "urgency": "urgent" })))
            .await;
        assert_eq!(out.error.unwrap().code, "INVALID_URGENCY");
    }
}
