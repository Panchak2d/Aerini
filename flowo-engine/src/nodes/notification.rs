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
        let urgency = cfg["urgency"].as_str().unwrap_or("normal").to_string();

        match send_notification(&title, &body, &urgency).await {
            Ok(method) => NodeOutput::success_with_logs(
                json!({ "sent": true }),
                vec![format!("Notification sent via {method}: {title}")],
            ),
            Err(e) => NodeOutput::failure(NodeError::unrecoverable(
                "NOTIFY_ERROR", &format!("Desktop notification failed: {e}"),
            )),
        }
    }
}

async fn send_notification(title: &str, body: &str, urgency: &str) -> Result<&'static str, String> {
    #[cfg(target_os = "linux")]
    {
        let mut cmd = Command::new("notify-send");
        cmd.arg("--urgency").arg(urgency);
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
        let script = if body.is_empty() {
            format!("display notification \"\" with title \"{}\"", title.replace('"', "\\\""))
        } else {
            format!(
                "display notification \"{}\" with title \"{}\"",
                body.replace('"', "\\\""),
                title.replace('"', "\\\""),
            )
        };
        let status = Command::new("osascript")
            .arg("-e").arg(&script)
            .status().await.map_err(|e| e.to_string())?;
        if status.success() {
            return Ok("osascript");
        }
        return Err(format!("osascript exited with {status}"));
    }

    #[cfg(target_os = "windows")]
    {
        // PowerShell toast notification (Windows 10+)
        let script = format!(
            "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null; \
             $xml = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02); \
             $xml.GetElementsByTagName('text')[0].AppendChild($xml.CreateTextNode('{}')) | Out-Null; \
             $xml.GetElementsByTagName('text')[1].AppendChild($xml.CreateTextNode('{}')) | Out-Null; \
             [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('Flowo').Show([Windows.UI.Notifications.ToastNotification]::new($xml))",
            title.replace('\'', "''"),
            body.replace('\'', "''"),
        );
        let status = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .status().await.map_err(|e| e.to_string())?;
        if status.success() {
            return Ok("powershell");
        }
        return Err(format!("PowerShell notification exited with {status}"));
    }

    #[allow(unreachable_code)]
    Err("Desktop notifications not supported on this platform".to_string())
}
