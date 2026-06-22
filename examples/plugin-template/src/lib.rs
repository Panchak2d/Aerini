wit_bindgen::generate!({ world: "aerini-node" });

use exports::aerini::plugin::node::{Guest, NodeDescriptor, NodeInput, NodeOutput};

struct EchoPlugin;

impl Guest for EchoPlugin {
    fn describe() -> NodeDescriptor {
        NodeDescriptor {
            type_id: "com.example.echo".to_string(),
            display_name: "Echo".to_string(),
            category: "utility".to_string(),
            description: "Echoes the message parameter to output.".to_string(),
            input_schema: r#"{"type":"object","properties":{"message":{"type":"string","description":"Text to echo"}},"required":["message"]}"#.to_string(),
            output_schema: r#"{"type":"object","properties":{"echo":{"type":"string"}}}"#.to_string(),
        }
    }

    fn execute(input: NodeInput) -> NodeOutput {
        let message = input
            .params
            .iter()
            .find(|p| p.key == "message")
            .map(|p| p.value.as_str())
            .unwrap_or("");

        // Escape backslash and double-quote before embedding in JSON.
        let escaped = message.replace('\\', "\\\\").replace('"', "\\\"");

        NodeOutput {
            success: true,
            data: format!(r#"{{"echo":"{escaped}"}}"#),
            error_code: String::new(),
            error_message: String::new(),
            recoverable: false,
        }
    }
}

export!(EchoPlugin);
