use chrono::Utc;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::NodeOutput;
use crate::nodes::util::{decode_file_data, read_json_response_capped, MAX_JSON_RESPONSE_BYTES};

use super::shared::{network_err, strip_data_uri_prefix, ImageRun};

// -- OpenAI GPT Image (gpt-image-1 and gpt-image-2) --------------------------
//
// One request per image: n>1 sends n requests with n=1 and merges the results,
// so one response never carries several base64 images (a response is capped at
// 10 MB). Both models accept n natively (gpt-image-1: 1-10, gpt-image-2: 1-8),
// but a batch of large images can pass that cap.
// Returns data[].b64_json by default; no response_format parameter needed.
// output.source = config alias string (preserves "dalle3" for backward compat).
//
// With reference images the call goes to the edits endpoint instead:
// POST https://api.openai.com/v1/images/edits  (multipart/form-data)
// Fields: model, prompt, n, size, quality, image[] (one part per reference image).
// Response: same {"data":[{"b64_json":"..."}]} format as the generations endpoint.
// developers.openai.com/api/reference/resources/images/methods/edit, June 2026.
// Supports up to 16 reference images per OpenAI spec.
// reqwest "multipart" feature required (enabled in Cargo.toml).

pub(super) struct GptImageRequest<'a> {
    pub(super) model:      &'a str,     // actual API model string: "gpt-image-1" or "gpt-image-2"
    pub(super) source:     &'a str,     // config alias used for output.source (preserves "dalle3" etc.)
    pub(super) prompt:     &'a str,
    pub(super) n:          usize,
    pub(super) size:       &'a str,
    pub(super) quality:    &'a str,
    pub(super) api_key:    &'a str,
    pub(super) ref_images: &'a [Value],
}

struct RefImage {
    bytes:    Vec<u8>,
    filename: String,
    mime:     &'static str,
}

fn invalid_reference(idx: usize, why: &str) -> NodeError {
    NodeError::unrecoverable(
        "INVALID_REFERENCE_IMAGE",
        format!("Reference image {} is not usable ({}). Nothing was sent to OpenAI.", idx + 1, why),
    )
}

/// Decodes every reference image before any request is sent, so a bad one
/// fails the node instead of being dropped from a call that is still billed.
fn decode_reference_images(ref_images: &[Value]) -> Result<Vec<RefImage>, NodeError> {
    ref_images
        .iter()
        .enumerate()
        .map(|(idx, img)| {
            let bytes = decode_file_data(img["data"].as_str().unwrap_or(""))
                .map_err(|e| invalid_reference(idx, &e))?;
            if bytes.is_empty() {
                return Err(invalid_reference(idx, "no data"));
            }
            let mime = match img["mime_type"].as_str().unwrap_or("image/png") {
                "image/jpeg" => "image/jpeg",
                "image/jpg"  => "image/jpg",
                "image/webp" => "image/webp",
                "image/gif"  => "image/gif",
                _            => "image/png",
            };
            let filename = img["filename"].as_str().unwrap_or("image.png").to_string();
            Ok(RefImage { bytes, filename, mime })
        })
        .collect()
}

pub(super) async fn gen_gpt_image(client: reqwest::Client, req: GptImageRequest<'_>) -> NodeOutput {
    let refs = match decode_reference_images(req.ref_images) {
        Ok(r)  => r,
        Err(e) => return NodeOutput::failure(e),
    };

    let ts = Utc::now().timestamp_millis();
    let mut run = ImageRun::new("GPT Image", req.n);

    for i in 0..req.n {
        let result = match request_one(&client, &req, &refs).await {
            Ok(json) => image_file(&json, req.source, ts, i, !refs.is_empty()),
            Err(e)   => Err(e),
        };
        match result {
            Ok(file) => run.generated(i, file),
            Err(e) => {
                if run.failed(i, e) { break; }
            }
        }
    }

    run.finish(req.source)
}

/// Sends one single-image request (generations, or edits when there are
/// reference images) and returns the parsed body once it has no `error` object.
async fn request_one(
    client: &reqwest::Client,
    req: &GptImageRequest<'_>,
    refs: &[RefImage],
) -> Result<Value, NodeError> {
    let record = crate::provider::ProviderRegistry::global()
        .get(req.source)
        .expect("gpt_image record always registered");

    let builder = if refs.is_empty() {
        let body = json!({
            "model":   req.model,
            "prompt":  req.prompt,
            "n":       1,
            "size":    req.size,
            "quality": req.quality
        });
        client
            .post("https://api.openai.com/v1/images/generations")
            .header("Content-Type", "application/json")
            .json(&body)
    } else {
        let mut form = reqwest::multipart::Form::new()
            .text("model",   req.model.to_string())
            .text("prompt",  req.prompt.to_string())
            .text("n",       "1")
            .text("size",    req.size.to_string())
            .text("quality", req.quality.to_string());
        for r in refs {
            let part = reqwest::multipart::Part::bytes(r.bytes.clone())
                .file_name(r.filename.clone())
                .mime_str(r.mime)
                .expect("hardcoded safe MIME type, cannot fail");
            form = form.part("image[]", part);
        }
        client.post("https://api.openai.com/v1/images/edits").multipart(form)
    };

    // registry-managed: auth header
    let resp = crate::provider::ProviderRegistry::apply_auth(record, builder, req.api_key)
        .send()
        .await
        .map_err(network_err)?;

    let status = resp.status().as_u16();
    let json: Value = read_json_response_capped(resp)
        .await
        .map_err(|e| response_read_error(status, e))?;

    if let Some(err) = json["error"].as_object() {
        return Err(map_openai_image_error(status, err));
    }
    Ok(json)
}

fn response_read_error(status: u16, e: String) -> NodeError {
    if e.contains("MB limit") {
        return NodeError::unrecoverable(
            "RESPONSE_TOO_LARGE",
            format!(
                "OpenAI returned one image larger than the {} MB response limit, so it was discarded and may still have been billed. Lower size or quality. ({})",
                MAX_JSON_RESPONSE_BYTES / (1024 * 1024),
                e
            ),
        );
    }
    crate::nodes::util::provider_error(status, "PARSE_ERROR", e)
}

fn image_file(json: &Value, source: &str, ts: i64, index: usize, edited: bool) -> Result<Value, NodeError> {
    let b64 = json["data"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|item| item["b64_json"].as_str())
        .map(strip_data_uri_prefix)
        .filter(|s| !s.is_empty());
    match b64 {
        Some(data) => {
            let filename = if edited {
                format!("{}_edit_{}_{}.png", source, ts, index)
            } else {
                format!("{}_{}_{}.png", source, ts, index)
            };
            Ok(json!({ "filename": filename, "data": data, "mime_type": "image/png" }))
        }
        None => Err(NodeError::unrecoverable("EMPTY_RESPONSE", "GPT Image returned no b64_json image")),
    }
}

fn map_openai_image_error(status: u16, err: &serde_json::Map<String, Value>) -> NodeError {
    let msg      = err.get("message").and_then(|m| m.as_str()).unwrap_or("Unknown error").to_string();
    let code     = err.get("code").and_then(|c| c.as_str()).unwrap_or("");
    let err_type = err.get("type").and_then(|t| t.as_str()).unwrap_or("");

    match status {
        429 if code == "insufficient_quota" || err_type == "insufficient_quota" => {
            NodeError::unrecoverable("INSUFFICIENT_QUOTA", format!("OpenAI credit balance or spending limit exhausted; check billing at platform.openai.com. {}", msg))
        }
        429 => NodeError::recoverable("RATE_LIMITED", format!("OpenAI rate limit exceeded. {}", msg)),
        400 if code == "content_policy_violation" || err_type.contains("content_policy") => {
            NodeError::unrecoverable("CONTENT_POLICY_VIOLATION", format!("Prompt rejected by content policy: {}", msg))
        }
        400 if code == "moderation_blocked" => {
            NodeError::unrecoverable("CONTENT_POLICY_VIOLATION", format!("Request rejected by OpenAI's safety system: {}", msg))
        }
        401 => NodeError::unrecoverable("INVALID_API_KEY", "Invalid or expired OpenAI API key. Check your key in Connections."),
        _   => NodeError::unrecoverable("API_ERROR", msg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_err(
        code: Option<&str>,
        typ:  Option<&str>,
        msg:  Option<&str>,
    ) -> serde_json::Map<String, Value> {
        let mut map = serde_json::Map::new();
        if let Some(c) = code { map.insert("code".into(),    json!(c)); }
        if let Some(t) = typ  { map.insert("type".into(),    json!(t)); }
        if let Some(m) = msg  { map.insert("message".into(), json!(m)); }
        map
    }

    #[test]
    fn status_429_returns_rate_limited_recoverable() {
        let e = map_openai_image_error(429, &make_err(None, None, Some("too many")));
        assert_eq!(e.code, "RATE_LIMITED");
        assert!(e.recoverable);
    }

    #[test]
    fn status_400_content_policy_code_returns_violation() {
        let e = map_openai_image_error(400, &make_err(Some("content_policy_violation"), None, Some("bad prompt")));
        assert_eq!(e.code, "CONTENT_POLICY_VIOLATION");
        assert!(!e.recoverable);
    }

    #[test]
    fn status_400_content_policy_type_returns_violation() {
        let e = map_openai_image_error(400, &make_err(None, Some("content_policy_violation"), Some("bad")));
        assert_eq!(e.code, "CONTENT_POLICY_VIOLATION");
        assert!(!e.recoverable);
    }

    #[test]
    fn status_400_type_contains_content_policy_substring() {
        // err_type.contains("content_policy") must match partial substrings
        let e = map_openai_image_error(400, &make_err(None, Some("content_policy_check"), Some("bad")));
        assert_eq!(e.code, "CONTENT_POLICY_VIOLATION");
    }

    #[test]
    fn status_401_returns_invalid_api_key_unrecoverable() {
        let e = map_openai_image_error(401, &make_err(None, None, None));
        assert_eq!(e.code, "INVALID_API_KEY");
        assert!(!e.recoverable);
    }

    #[test]
    fn unknown_status_returns_api_error_unrecoverable() {
        let e = map_openai_image_error(500, &make_err(None, None, Some("internal error")));
        assert_eq!(e.code, "API_ERROR");
        assert!(!e.recoverable);
    }

    #[test]
    fn missing_message_field_uses_unknown_error_fallback() {
        let e = map_openai_image_error(500, &make_err(None, None, None));
        assert_eq!(e.code, "API_ERROR");
        assert_eq!(e.message, "Unknown error");
    }

    #[test]
    fn status_400_without_content_policy_returns_api_error() {
        // 400 with unrelated code must NOT produce CONTENT_POLICY_VIOLATION
        let e = map_openai_image_error(400, &make_err(Some("billing_hard_limit_reached"), None, Some("limit")));
        assert_eq!(e.code, "API_ERROR");
    }

    #[test]
    fn status_429_insufficient_quota_is_unrecoverable_not_a_rate_limit() {
        for map in [
            make_err(Some("insufficient_quota"), Some("insufficient_quota"), Some("You exceeded your current quota")),
            make_err(None, Some("insufficient_quota"), Some("quota")),
        ] {
            let e = map_openai_image_error(429, &map);
            assert_eq!(e.code, "INSUFFICIENT_QUOTA");
            assert!(!e.recoverable);
        }
    }

    #[test]
    fn status_400_moderation_blocked_returns_violation() {
        let e = map_openai_image_error(400, &make_err(Some("moderation_blocked"), Some("user_error"), Some("rejected by the safety system")));
        assert_eq!(e.code, "CONTENT_POLICY_VIOLATION");
        assert!(!e.recoverable);
        assert!(e.message.contains("safety system"));
    }

    #[test]
    fn a_reference_image_that_will_not_decode_fails_before_anything_is_sent() {
        let good = json!({ "data": "data:image/png;base64,aGVsbG8=", "mime_type": "image/png" });
        let bad  = json!({ "data": "!!not base64!!" });
        let empty = json!({ "data": "" });

        let e = decode_reference_images(&[good.clone(), bad]).err().expect("must fail");
        assert_eq!(e.code, "INVALID_REFERENCE_IMAGE");
        assert!(e.message.contains("Reference image 2"), "{}", e.message);
        assert!(!e.recoverable);

        let e = decode_reference_images(&[empty]).err().expect("must fail");
        assert!(e.message.contains("no data"), "{}", e.message);

        let ok = decode_reference_images(&[good]).expect("valid image decodes");
        assert_eq!(ok[0].bytes, b"hello");
    }

    #[test]
    fn reference_images_accept_line_breaks_and_missing_padding() {
        let img = json!({ "data": "aGVs\nbG8" });
        assert_eq!(decode_reference_images(&[img]).expect("lenient base64").remove(0).bytes, b"hello");
    }

    #[test]
    fn a_response_over_the_size_cap_names_size_and_quality_not_n() {
        let e = response_read_error(200, "Response body exceeds 10 MB limit".to_string());
        assert_eq!(e.code, "RESPONSE_TOO_LARGE");
        assert!(!e.recoverable);
        assert!(e.message.contains("Lower size or quality"), "{}", e.message);

        let other = response_read_error(200, "Failed to parse response as JSON: x".to_string());
        assert_eq!(other.code, "PARSE_ERROR");
    }

    #[tokio::test]
    async fn the_real_cap_error_from_the_reader_is_the_one_that_is_recognised() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut discard = [0u8; 1024];
            let _ = stream.read(&mut discard).await;
            let head = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n", MAX_JSON_RESPONSE_BYTES + 1);
            let _ = stream.write_all(head.as_bytes()).await;
            let _ = stream.shutdown().await;
        });
        let resp = reqwest::Client::new().get(format!("http://127.0.0.1:{port}/")).send().await.unwrap();
        let reader_error = read_json_response_capped(resp).await.expect_err("over the cap");
        assert_eq!(response_read_error(200, reader_error).code, "RESPONSE_TOO_LARGE");
    }

    #[test]
    fn the_image_is_read_from_the_first_data_item_and_data_uri_prefixes_are_stripped() {
        let ok = json!({ "data": [{ "b64_json": "data:image/png;base64,QUJD" }] });
        let f = image_file(&ok, "gpt_image_1", 7, 2, false).expect("image");
        assert_eq!(f["data"], "QUJD");
        assert_eq!(f["filename"], "gpt_image_1_7_2.png");
        assert_eq!(image_file(&ok, "dalle3", 7, 0, true).unwrap()["filename"], "dalle3_edit_7_0.png");

        for bad in [json!({}), json!({ "data": [] }), json!({ "data": [{}] }), json!({ "data": [{ "b64_json": "" }] })] {
            assert_eq!(image_file(&bad, "gpt_image_1", 7, 0, false).unwrap_err().code, "EMPTY_RESPONSE");
        }
    }
}
