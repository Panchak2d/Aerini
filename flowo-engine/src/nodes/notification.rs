use async_trait::async_trait;
use serde_json::{json, Value};
use std::process::Stdio;
use tokio::process::Command;

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortDefinition, PortPosition};

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
                "body":    { "type": "string", "description": "Notification message (optional)" },
                "urgency": { "type": "string", "enum": ["low", "normal", "critical"], "description": "Urgency level (Linux only)" }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({ "type": "object", "properties": { "sent": { "type": "boolean" }, "platform": { "type": "string" } } })
    }

    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs:  vec![PortDefinition { id: "input".to_string(),  label: "In".to_string(),  position: PortPosition::Left }],
            outputs: vec![PortDefinition { id: "output".to_string(), label: "Out".to_string(), position: PortPosition::Right }],
        }
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let title = match input.input["title"].as_str() {
            Some(t) if !t.is_empty() => t.to_string(),
            _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_TITLE", "title is required")),
        };
        let body    = input.input["body"].as_str().unwrap_or("").to_string();
        let urgency = input.input["urgency"].as_str().unwrap_or("normal").to_string();

        #[cfg(target_os = "linux")]
        {
            let has_display = std::env::var("DISPLAY").map(|v| !v.is_empty()).unwrap_or(false);
            let has_wayland = std::env::var("WAYLAND_DISPLAY").map(|v| !v.is_empty()).unwrap_or(false);
            if !has_display && !has_wayland {
                return NodeOutput::success_with_logs(
                    json!({"sent": false, "platform": "headless", "note": "No display available — notification skipped"}),
                    vec!["Notification skipped: no display environment detected (headless server)".to_string()],
                );
            }
        }

        let result = send_notification(&title, &body, &urgency).await;

        match result {
            Ok(platform) => NodeOutput::success_with_logs(
                json!({ "sent": true, "platform": platform }),
                vec![format!("Notification sent on {}", platform)],
            ),
            Err(e) => NodeOutput::failure(NodeError::unrecoverable("NOTIFY_ERROR", e)),
        }
    }
}

/// Escape a string for safe embedding inside a PowerShell double-quoted here-string (`@"..."@`).
/// `$` triggers variable/subexpression expansion (`$(cmd)`) and must be escaped as `` `$ ``.
/// Backtick followed by certain letters produces control sequences and must be escaped as ` `` `.
#[cfg(target_os = "windows")]
fn escape_powershell(s: &str) -> String {
    s.replace('`', "``").replace('$', "`$")
}

async fn send_notification(title: &str, body: &str, _urgency: &str) -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        let safe_title = escape_powershell(title);
        let safe_body  = escape_powershell(body);

        // Windows: use PowerShell toast notification
        let script = format!(
            r#"
[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null
[Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] | Out-Null
$template = @"
<toast>
  <visual>
    <binding template='ToastGeneric'>
      <text>{}</text>
      <text>{}</text>
    </binding>
  </visual>
</toast>
"@
$xml = New-Object Windows.Data.Xml.Dom.XmlDocument
$xml.LoadXml($template)
$toast = New-Object Windows.UI.Notifications.ToastNotification $xml
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('Flowo').Show($toast)
"#,
            safe_title,
            safe_body
        );

        let status = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map_err(|e| format!("PowerShell error: {}", e))?;

        if status.success() {
            return Ok("windows".to_string());
        }
        return Err("Windows notification failed — check PowerShell permissions".to_string());
    }

    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "display notification \"{}\" with title \"{}\"",
            body.replace('"', "\\\""),
            title.replace('"', "\\\"")
        );
        let status = Command::new("osascript")
            .args(["-e", &script])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map_err(|e| format!("osascript error: {}", e))?;
        if !status.success() {
            return Err(
                "macOS notification failed — check notification permissions or Do Not Disturb settings"
                    .to_string(),
            );
        }
        return Ok("macos".to_string());
    }

    #[cfg(target_os = "linux")]
    {
        let mut cmd = Command::new("notify-send");
        cmd.arg("--urgency").arg(_urgency);
        cmd.arg(title);
        if !body.is_empty() {
            cmd.arg(body);
        }
        let status = cmd
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map_err(|e| format!("notify-send error: {}. Is libnotify installed?", e))?;

        if status.success() {
            return Ok("linux".to_string());
        }
        return Err("notify-send failed. Install libnotify-bin: sudo apt install libnotify-bin".to_string());
    }

    #[allow(unreachable_code)]
    Err("Desktop notifications not supported on this platform".to_string())
}
