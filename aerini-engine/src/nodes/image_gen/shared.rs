use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde_json::Value;

use crate::error::NodeError;
use crate::nodes::util::{check_host_ssrf_from_url, SsrfPolicy};

pub(super) fn network_err(e: reqwest::Error) -> NodeError {
    let recoverable = e.is_timeout() || e.is_connect();
    if recoverable {
        NodeError::recoverable("NETWORK_ERROR", e.to_string())
    } else {
        NodeError::unrecoverable("NETWORK_ERROR", e.to_string())
    }
}

/// Strip a "data:<mime>;base64," prefix if present.
/// DATA CONTRACT requires raw base64 without any data: URI prefix.
pub(super) fn strip_data_uri_prefix(s: &str) -> &str {
    if let Some(comma_pos) = s.find(',') {
        if s[..comma_pos].starts_with("data:") {
            return &s[comma_pos + 1..];
        }
    }
    s
}

pub(super) fn mime_to_ext(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp"               => "webp",
        "image/gif"                => "gif",
        _                          => "png",
    }
}

/// Download image from URL -> raw base64 (no data: URI prefix).
/// Validates host against SSRF before fetch.
pub(super) async fn download_to_base64(client: &reqwest::Client, url: &str) -> Result<String, NodeError> {
    // Validates URL host is not RFC-1918/loopback -- a spoofed API could return
    // an internal address (e.g. AWS metadata) to exfiltrate infrastructure data.
    check_host_ssrf_from_url(url, SsrfPolicy::Strict).await.map_err(|e| {
        NodeError::unrecoverable("SSRF_BLOCKED", e)
    })?;
    let bytes = client
        .get(url)
        .send()
        .await
        .map_err(network_err)?
        .bytes()
        .await
        .map_err(|e| NodeError::unrecoverable("DOWNLOAD_ERROR", e.to_string()))?;
    Ok(BASE64.encode(&bytes))
}

// Parses the reference_images_expr config key written by Canvas.ts when a wire
// is connected to the "Reference Images" port.  The expression engine serialises
// resolved arrays as JSON strings; this mirrors extract_port_attachments() in
// ai_prompt.rs.  Unification with a shared utility is deferred to P25.
pub(super) fn extract_reference_images(val: &Value) -> Vec<Value> {
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
mod tests {
    use super::*;
    use serde_json::json;

    // ── mime_to_ext ───────────────────────────────────────────────────────────

    #[test]
    fn jpeg_maps_to_jpg() { assert_eq!(mime_to_ext("image/jpeg"), "jpg"); }

    #[test]
    fn jpg_alias_maps_to_jpg() { assert_eq!(mime_to_ext("image/jpg"), "jpg"); }

    #[test]
    fn webp_maps_to_webp() { assert_eq!(mime_to_ext("image/webp"), "webp"); }

    #[test]
    fn gif_maps_to_gif() { assert_eq!(mime_to_ext("image/gif"), "gif"); }

    #[test]
    fn png_maps_to_png() { assert_eq!(mime_to_ext("image/png"), "png"); }

    #[test]
    fn unknown_mime_falls_back_to_png() { assert_eq!(mime_to_ext("image/avif"), "png"); }

    #[test]
    fn empty_mime_falls_back_to_png() { assert_eq!(mime_to_ext(""), "png"); }

    // ── strip_data_uri_prefix ─────────────────────────────────────────────────

    #[test]
    fn strips_data_uri_prefix() {
        assert_eq!(strip_data_uri_prefix("data:image/png;base64,ABC"), "ABC");
    }

    #[test]
    fn no_prefix_unchanged() {
        assert_eq!(strip_data_uri_prefix("ABC"), "ABC");
    }

    #[test]
    fn empty_string_unchanged() {
        assert_eq!(strip_data_uri_prefix(""), "");
    }

    #[test]
    fn comma_without_data_prefix_unchanged() {
        // comma present but prefix is not "data:" — must NOT strip
        assert_eq!(
            strip_data_uri_prefix("not:image/png;base64,ABC"),
            "not:image/png;base64,ABC",
        );
    }

    #[test]
    fn strips_jpeg_data_uri() {
        assert_eq!(strip_data_uri_prefix("data:image/jpeg;base64,/9j/xyz"), "/9j/xyz");
    }

    // ── extract_reference_images ──────────────────────────────────────────────
    // NOTE: CommonImageParams::from_input was not extracted in P15 (width/height
    // defaults diverge per provider — deferred to P25). Tests cover
    // extract_reference_images instead, which is the other pure-logic utility here.

    #[test]
    fn bare_array_value_returned_as_is() {
        let val = json!([{"filename": "a.png"}, {"filename": "b.png"}]);
        let out = extract_reference_images(&val);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["filename"], "a.png");
    }

    #[test]
    fn bare_object_with_files_key_extracted() {
        let val = json!({"files": [{"filename": "x.png"}], "count": 1});
        let out = extract_reference_images(&val);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["filename"], "x.png");
    }

    #[test]
    fn bare_object_without_files_key_returns_empty() {
        let val = json!({"count": 0});
        assert!(extract_reference_images(&val).is_empty());
    }

    #[test]
    fn string_json_array_parsed() {
        let val = json!(r#"[{"filename":"a.png"}]"#);
        let out = extract_reference_images(&val);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["filename"], "a.png");
    }

    #[test]
    fn string_json_object_with_files_parsed() {
        let val = json!(r#"{"files":[{"filename":"b.png"}]}"#);
        let out = extract_reference_images(&val);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["filename"], "b.png");
    }

    #[test]
    fn empty_string_returns_empty() {
        let val = json!("");
        assert!(extract_reference_images(&val).is_empty());
    }

    #[test]
    fn whitespace_string_returns_empty() {
        let val = json!("   ");
        assert!(extract_reference_images(&val).is_empty());
    }

    #[test]
    fn invalid_json_string_returns_empty() {
        let val = json!("{not valid json");
        assert!(extract_reference_images(&val).is_empty());
    }

    #[test]
    fn null_value_returns_empty() {
        assert!(extract_reference_images(&Value::Null).is_empty());
    }
}
