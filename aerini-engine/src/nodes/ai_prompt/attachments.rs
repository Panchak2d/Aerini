use serde_json::Value;

/// `(mime_type, base64 data)` — image ready for a provider content block.
pub(crate) type ImageAttachment = (String, String);

/// `(mime_type, base64 data, filename)` — filename required by OpenAI's `file` block.
pub(crate) type DocAttachment = (String, String, String);

/// Output of `process_attachments` — replaces the previous 4-tuple return.
pub(crate) struct ProcessedAttachments {
    pub(crate) images:  Vec<ImageAttachment>,
    pub(crate) docs:    Vec<DocAttachment>,
    /// Non-empty when text/markdown files were decoded; caller appends to system prompt.
    pub(crate) warning: String,
    /// Per-item skip notices forwarded into NodeOutput.logs.
    pub(crate) logs:    Vec<String>,
}

impl ProcessedAttachments {
    /// True when at least one attachment produced content a model can read:
    /// an image, a PDF, or decoded text/markdown.
    pub(crate) fn has_content(&self) -> bool {
        !self.images.is_empty() || !self.docs.is_empty() || !self.warning.is_empty()
    }
}

/// Stands in for the user's text when a request carries readable attachments but
/// no text of its own (e.g. a Chat panel message that is only a file).
pub(crate) const ATTACHMENT_ONLY_PROMPT: &str =
    "The user attached file(s) with no message. Look at what was attached and respond helpfully.";

/// MIME types `process_attachments` turns into content a model can read.
pub(crate) const SUPPORTED_ATTACHMENT_MIMES: &[&str] = &[
    "image/png", "image/jpeg", "image/webp", "image/gif",
    "application/pdf", "text/plain", "text/markdown",
];

/// Splits the raw `attachments` array into provider-agnostic buckets.
///
/// Each item follows Aerini's file-object contract:
/// `{filename, data, mime_type}` — `data` is raw base64 without a `data:` URI prefix.
/// Unsupported or malformed entries are skipped with a log entry; one bad attachment
/// must never block a prompt that would otherwise succeed.
///
/// Text/markdown content is decoded and returned as `warning` (a string the caller
/// appends to the system prompt). Cheaper and more reliable than provider-specific
/// document uploads for plain text.
pub(crate) fn process_attachments(attachments: &[Value]) -> ProcessedAttachments {
    let mut images:  Vec<ImageAttachment> = Vec::new();
    let mut docs:    Vec<DocAttachment>   = Vec::new();
    let mut warning  = String::new();
    let mut logs:    Vec<String>          = Vec::new();

    for att in attachments {
        let filename = att["filename"].as_str().unwrap_or("file").to_string();
        let mime     = att["mime_type"].as_str().unwrap_or("").to_string();
        let data     = att["data"].as_str().unwrap_or("");

        if data.is_empty() {
            logs.push(format!("Skipped attachment '{}': missing data", filename));
            continue;
        }

        match mime.as_str() {
            "image/png" | "image/jpeg" | "image/webp" | "image/gif" => {
                images.push((mime, data.to_string()));
            }
            "application/pdf" => {
                docs.push((mime, data.to_string(), filename));
            }
            "text/plain" | "text/markdown" => {
                use base64::Engine as _;
                match base64::engine::general_purpose::STANDARD.decode(data) {
                    Ok(bytes) => match String::from_utf8(bytes) {
                        Ok(text) => {
                            warning.push_str(&format!("\n\n--- Attached file: {} ---\n{}", filename, text));
                        }
                        Err(_) => logs.push(format!("Skipped attachment '{}': not valid UTF-8 text", filename)),
                    },
                    Err(_) => logs.push(format!("Skipped attachment '{}': invalid base64 data", filename)),
                }
            }
            "" => logs.push(format!("Skipped attachment '{}': missing mime_type", filename)),
            other => logs.push(format!("Skipped attachment '{}': unsupported type '{}'", filename, other)),
        }
    }

    ProcessedAttachments { images, docs, warning, logs }
}

/// Parses a resolved port-expression value back into an attachment item list.
///
/// The executor serialises resolved arrays to JSON strings. This function
/// reverses that, handling both shapes produced by Aerini file-output nodes:
///
///   `[{filename, data, mime_type}, …]`           — bare array (collect_files, text_to_file)
///   `{"files":[{filename, data, mime_type}], …}` — media-contract object (image_gen)
///
/// Returns an empty vec on any error so a misconfigured port never blocks execution.
pub(crate) fn extract_port_attachments(val: &Value) -> Vec<Value> {
    if let Some(s) = val.as_str() {
        let trimmed = s.trim();
        if trimmed.is_empty() { return vec![]; }
        let parsed: Value = match serde_json::from_str(trimmed) {
            Ok(v)  => v,
            Err(_) => return vec![],
        };
        return match parsed {
            Value::Array(arr)    => arr,
            Value::Object(ref o) => o.get("files")
                .and_then(|f| f.as_array())
                .cloned()
                .unwrap_or_default(),
            _                    => vec![],
        };
    }
    match val {
        Value::Array(arr)    => arr.clone(),
        Value::Object(ref o) => o.get("files")
            .and_then(|f| f.as_array())
            .cloned()
            .unwrap_or_default(),
        _                    => vec![],
    }
}

#[cfg(test)]
mod attachment_tests {
    use super::*;
    use base64::Engine as _;
    use serde_json::json;

    fn b64(s: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(s.as_bytes())
    }

    #[test]
    fn empty_input_yields_all_empty() {
        let pa = process_attachments(&[]);
        assert!(pa.images.is_empty());
        assert!(pa.docs.is_empty());
        assert_eq!(pa.warning, "");
        assert!(pa.logs.is_empty());
    }

    #[test]
    fn classifies_image_pdf_and_text_correctly() {
        let attachments = vec![
            json!({ "filename": "a.png", "mime_type": "image/png",         "data": "abc123" }),
            json!({ "filename": "b.pdf", "mime_type": "application/pdf",   "data": "def456" }),
            json!({ "filename": "c.txt", "mime_type": "text/plain",        "data": b64("hello world") }),
        ];
        let pa = process_attachments(&attachments);
        assert_eq!(pa.images, vec![("image/png".to_string(), "abc123".to_string())]);
        assert_eq!(pa.docs,   vec![("application/pdf".to_string(), "def456".to_string(), "b.pdf".to_string())]);
        assert!(pa.warning.contains("c.txt"));
        assert!(pa.warning.contains("hello world"));
        assert!(pa.logs.is_empty());
    }

    #[test]
    fn every_supported_mime_type_yields_readable_content() {
        for mime in SUPPORTED_ATTACHMENT_MIMES {
            let pa = process_attachments(&[json!({ "filename": "f", "mime_type": mime, "data": b64("hello") })]);
            assert!(pa.has_content(), "{} is listed as supported but produced no content", mime);
        }
    }

    #[test]
    fn unsupported_type_warns_without_dropping_others() {
        let attachments = vec![
            json!({ "filename": "x.exe", "mime_type": "application/octet-stream", "data": "zz" }),
            json!({ "filename": "a.png", "mime_type": "image/png",                "data": "abc123" }),
        ];
        let pa = process_attachments(&attachments);
        assert_eq!(pa.images.len(), 1, "valid attachment after a bad one must still be processed");
        assert_eq!(pa.logs.len(), 1);
        assert!(pa.logs[0].contains("x.exe"));
        assert!(pa.logs[0].contains("unsupported type"));
    }

    #[test]
    fn missing_data_warns_and_skips() {
        let attachments = vec![json!({ "filename": "empty.png", "mime_type": "image/png" })];
        let pa = process_attachments(&attachments);
        assert!(pa.images.is_empty());
        assert_eq!(pa.logs.len(), 1);
        assert!(pa.logs[0].contains("missing data"));
    }

    #[test]
    fn invalid_base64_text_warns_and_skips() {
        let attachments = vec![json!({ "filename": "bad.txt", "mime_type": "text/plain", "data": "not-valid-base64!!!" })];
        let pa = process_attachments(&attachments);
        assert_eq!(pa.warning, "");
        assert_eq!(pa.logs.len(), 1);
        assert!(pa.logs[0].contains("invalid base64"));
    }

    #[test]
    fn multiple_text_files_accumulate_in_warning() {
        let attachments = vec![
            json!({ "filename": "one.txt", "mime_type": "text/plain",    "data": b64("first") }),
            json!({ "filename": "two.md",  "mime_type": "text/markdown", "data": b64("second") }),
        ];
        let pa = process_attachments(&attachments);
        assert!(pa.logs.is_empty());
        assert!(pa.warning.contains("first"));
        assert!(pa.warning.contains("second"));
        assert!(pa.warning.contains("one.txt"));
        assert!(pa.warning.contains("two.md"));
    }

    #[test]
    fn null_json_entry_skipped_gracefully() {
        // A null Value in the array must not panic — treated as missing data.
        let attachments = vec![Value::Null];
        let pa = process_attachments(&attachments);
        assert!(pa.images.is_empty());
        assert!(pa.docs.is_empty());
        assert_eq!(pa.logs.len(), 1, "null entry must emit a skip log");
        assert!(pa.logs[0].contains("missing data"));
    }

    // ── extract_port_attachments ─────────────────────────────────────────────

    #[test]
    fn bare_array_value_returned_as_is() {
        let val = json!([{"filename": "a.png"}, {"filename": "b.png"}]);
        let out = extract_port_attachments(&val);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["filename"], "a.png");
    }

    #[test]
    fn bare_object_with_files_key_extracted() {
        let val = json!({"files": [{"filename": "x.png"}], "count": 1});
        let out = extract_port_attachments(&val);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["filename"], "x.png");
    }

    #[test]
    fn bare_object_without_files_key_returns_empty() {
        let val = json!({"count": 0});
        assert!(extract_port_attachments(&val).is_empty());
    }

    #[test]
    fn string_json_array_parsed() {
        let val = json!(r#"[{"filename":"a.png"}]"#);
        let out = extract_port_attachments(&val);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["filename"], "a.png");
    }

    #[test]
    fn string_json_object_with_files_parsed() {
        let val = json!(r#"{"files":[{"filename":"b.png"}]}"#);
        let out = extract_port_attachments(&val);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["filename"], "b.png");
    }

    #[test]
    fn empty_string_returns_empty() {
        let val = json!("");
        assert!(extract_port_attachments(&val).is_empty());
    }

    #[test]
    fn whitespace_string_returns_empty() {
        let val = json!("   ");
        assert!(extract_port_attachments(&val).is_empty());
    }

    #[test]
    fn invalid_json_string_returns_empty() {
        let val = json!("{not valid json");
        assert!(extract_port_attachments(&val).is_empty());
    }

    #[test]
    fn null_value_returns_empty() {
        assert!(extract_port_attachments(&Value::Null).is_empty());
    }
}
