wit_bindgen::generate!({ world: "aerini-node" });

use exports::aerini::plugin::node::{Guest, NodeDescriptor, NodeInput, NodeOutput};

/// Escape a string for embedding in a JSON string literal, per RFC 8259 §7.
///
/// Every control character (U+0000-U+001F) must be escaped, not just `"` and
/// `\` — a raw newline/tab/etc. inside a JSON string literal is invalid JSON.
/// Characters above U+001F (including all non-ASCII text) are valid,
/// unescaped UTF-8 inside a JSON string and pass through unchanged.
fn escape_json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

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
        // This template only reads a flat string param -- see `input.params`'s
        // documentation in wit/node.wit for the reserved "__aerini_input_json"
        // key, which carries the whole merged input as one JSON string for
        // nodes with nested/structured config to parse once.
        let message = input
            .params
            .iter()
            .find(|p| p.key == "message")
            .map(|p| p.value.as_str())
            .unwrap_or("");

        let escaped = escape_json_string(message);

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
