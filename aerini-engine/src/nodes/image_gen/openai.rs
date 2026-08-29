use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use chrono::Utc;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::NodeOutput;
use crate::nodes::util::read_json_response_capped;

use super::shared::{network_err, strip_data_uri_prefix};

// -- OpenAI GPT Image (gpt-image-1 and gpt-image-2) --------------------------
//
// Single API call with n -- both models support native batching.
//   gpt-image-1: n=1-10   gpt-image-2: n=1-8
// Returns data[].b64_json by default; no response_format parameter needed.
// output.source = config alias string (preserves "dalle3" for backward compat).

// Bundles gen_gpt_image's per-request fields -- clippy::too_many_arguments
// (max 7) was exceeded (8 args).
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

pub(super) async fn gen_gpt_image(client: reqwest::Client, req: GptImageRequest<'_>) -> NodeOutput {
    if !req.ref_images.is_empty() {
        return gen_gpt_image_edit(client, req).await;
    }

    let ts = Utc::now().timestamp_millis();

    let body = json!({
        "model":   req.model,
        "prompt":  req.prompt,
        "n":       req.n,
        "size":    req.size,
        "quality": req.quality
    });

    let record = crate::provider::ProviderRegistry::global()
        .get(req.source)
        .expect("gpt_image record always registered");
    // registry-managed: auth header
    let resp = match crate::provider::ProviderRegistry::apply_auth(
        record,
        client.post("https://api.openai.com/v1/images/generations"),
        req.api_key,
    )
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
    {
        Ok(r)  => r,
        Err(e) => return NodeOutput::failure(network_err(e)),
    };

    let status = resp.status().as_u16();
    let json: Value = match read_json_response_capped(resp).await {
        Ok(v)  => v,
        Err(e) => return NodeOutput::failure(NodeError::unrecoverable("PARSE_ERROR", e)),
    };

    if let Some(err) = json["error"].as_object() {
        return NodeOutput::failure(map_openai_image_error(status, err));
    }

    let data = match json["data"].as_array() {
        Some(arr) if !arr.is_empty() => arr,
        _ => return NodeOutput::failure(NodeError::unrecoverable(
            "EMPTY_RESPONSE",
            "GPT Image API returned empty or missing data array",
        )),
    };

    let mut files: Vec<Value>  = Vec::new();
    let mut logs:  Vec<String> = Vec::new();

    for (i, item) in data.iter().enumerate() {
        if let Some(b64) = item["b64_json"].as_str() {
            let filename = format!("{}_{}_{}.png", req.source, ts, i);
            files.push(json!({
                "filename":  filename,
                "data":      strip_data_uri_prefix(b64),
                "mime_type": "image/png"
            }));
            logs.push(format!("GPT Image {}/{} generated", i + 1, data.len()));
        } else {
            logs.push(format!("GPT Image item {} missing b64_json -- skipped", i));
        }
    }

    if files.is_empty() {
        return NodeOutput::failure_with_logs(
            NodeError::unrecoverable("EMPTY_RESPONSE", "GPT Image returned no b64_json in any data item"),
            logs,
        );
    }

    NodeOutput::success_with_logs(
        json!({ "files": files, "count": files.len(), "source": req.source }),
        logs,
    )
}

// GPT Image edit endpoint -- used when reference_images port has files connected.
//
// POST https://api.openai.com/v1/images/edits  (multipart/form-data)
// Fields: model, prompt, n, size, quality, image[] (one part per reference image).
// Response: same {"data":[{"b64_json":"..."}]} format as the generations endpoint.
// developers.openai.com/api/reference/resources/images/methods/edit, June 2026.
// Supports up to 16 reference images per OpenAI spec.
// reqwest 0.13 "multipart" feature required (enabled in Cargo.toml).
async fn gen_gpt_image_edit(client: reqwest::Client, req: GptImageRequest<'_>) -> NodeOutput {
    let ts   = Utc::now().timestamp_millis();
    let mut logs: Vec<String> = Vec::new();

    let mut form = reqwest::multipart::Form::new()
        .text("model",   req.model.to_string())
        .text("prompt",  req.prompt.to_string())
        .text("n",       req.n.to_string())
        .text("size",    req.size.to_string())
        .text("quality", req.quality.to_string());

    for (idx, img) in req.ref_images.iter().enumerate() {
        let data_str = img["data"].as_str().unwrap_or("");
        let clean    = strip_data_uri_prefix(data_str);
        if clean.is_empty() {
            logs.push(format!("reference image {} skipped: empty data", idx));
            continue;
        }
        let raw_bytes = match BASE64.decode(clean) {
            Ok(b)  => b,
            Err(_) => {
                logs.push(format!("reference image {} skipped: invalid base64", idx));
                continue;
            }
        };
        let mime     = img["mime_type"].as_str().unwrap_or("image/png");
        let filename = img["filename"].as_str().unwrap_or("image.png").to_string();
        // Constrain mime to known-valid image types before passing to mime_str.
        let safe_mime = match mime {
            "image/png" | "image/jpeg" | "image/jpg" | "image/webp" | "image/gif" => mime,
            _ => "image/png",
        };
        let part = reqwest::multipart::Part::bytes(raw_bytes)
            .file_name(filename)
            .mime_str(safe_mime)
            .expect("hardcoded safe MIME type, cannot fail");
        form = form.part("image[]", part);
    }

    let record = crate::provider::ProviderRegistry::global()
        .get(req.source)
        .expect("gpt_image record always registered");
    // registry-managed: auth header
    let resp = match crate::provider::ProviderRegistry::apply_auth(
        record,
        client.post("https://api.openai.com/v1/images/edits"),
        req.api_key,
    )
        .multipart(form)
        .send()
        .await
    {
        Ok(r)  => r,
        Err(e) => return NodeOutput::failure_with_logs(network_err(e), logs),
    };

    let status = resp.status().as_u16();
    let json: Value = match read_json_response_capped(resp).await {
        Ok(v)  => v,
        Err(e) => return NodeOutput::failure_with_logs(
            NodeError::unrecoverable("PARSE_ERROR", e), logs,
        ),
    };

    if let Some(err) = json["error"].as_object() {
        return NodeOutput::failure_with_logs(map_openai_image_error(status, err), logs);
    }

    let data = match json["data"].as_array() {
        Some(arr) if !arr.is_empty() => arr,
        _ => return NodeOutput::failure_with_logs(
            NodeError::unrecoverable("EMPTY_RESPONSE", "GPT Image edit API returned empty or missing data array"),
            logs,
        ),
    };

    let mut files: Vec<Value> = Vec::new();
    for (i, item) in data.iter().enumerate() {
        if let Some(b64) = item["b64_json"].as_str() {
            let filename = format!("{}_edit_{}_{}.png", req.source, ts, i);
            files.push(json!({
                "filename":  filename,
                "data":      strip_data_uri_prefix(b64),
                "mime_type": "image/png"
            }));
            logs.push(format!("GPT Image edit {}/{} generated", i + 1, data.len()));
        } else {
            logs.push(format!("GPT Image edit item {} missing b64_json -- skipped", i));
        }
    }

    if files.is_empty() {
        return NodeOutput::failure_with_logs(
            NodeError::unrecoverable("EMPTY_RESPONSE", "GPT Image edit returned no b64_json in any data item"),
            logs,
        );
    }

    NodeOutput::success_with_logs(
        json!({ "files": files, "count": files.len(), "source": req.source }),
        logs,
    )
}

fn map_openai_image_error(status: u16, err: &serde_json::Map<String, Value>) -> NodeError {
    let msg      = err.get("message").and_then(|m| m.as_str()).unwrap_or("Unknown error").to_string();
    let code     = err.get("code").and_then(|c| c.as_str()).unwrap_or("");
    let err_type = err.get("type").and_then(|t| t.as_str()).unwrap_or("");

    match status {
        429 => NodeError::recoverable("RATE_LIMITED", format!("OpenAI rate limit exceeded. {}", msg)),
        400 if code == "content_policy_violation" || err_type.contains("content_policy") => {
            NodeError::unrecoverable("CONTENT_POLICY_VIOLATION", format!("Prompt rejected by content policy: {}", msg))
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
}
