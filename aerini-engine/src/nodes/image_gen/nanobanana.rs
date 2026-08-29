use chrono::Utc;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::NodeOutput;
use crate::nodes::util::read_json_response_capped;

use super::shared::{network_err, strip_data_uri_prefix, mime_to_ext};

// -- NanoBanana (Google gemini-2.5-flash-image) -------------------------------
//
// Config aliases "imagen4" and "nano_banana" both route here.
// No native batch -- loop one request per image; partial success per G4 pattern.
// output.source = "imagen4" always (DATA CONTRACT: preserves saved-workflow compat).

pub(super) async fn gen_nano_banana(
    client: reqwest::Client,
    prompt: &str,
    n: usize,
    aspect_ratio: &str,
    api_key: &str,
    ref_images: &[Value],
) -> NodeOutput {
    let ts = Utc::now().timestamp_millis();
    let mut files: Vec<Value>     = Vec::new();
    let mut logs:  Vec<String>    = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    let mut last_error: Option<NodeError> = None;

    for i in 0..n {
        match call_nano_banana_once(&client, prompt, aspect_ratio, api_key, ref_images, ts, i).await {
            Ok(media_obj) => {
                logs.push(format!("NanoBanana image {}/{} generated", i + 1, n));
                files.push(media_obj);
            }
            Err(e) => {
                let msg = format!("NanoBanana image {}/{} failed: [{}] {}", i + 1, n, e.code, e.message);
                logs.push(msg.clone());
                failures.push(msg);
                let is_terminal = e.code == "INVALID_API_KEY" || e.code == "CONTENT_POLICY_VIOLATION";
                last_error = Some(e);
                if is_terminal { break; }
            }
        }
    }

    if files.is_empty() {
        let err = last_error.unwrap_or_else(|| {
            NodeError::unrecoverable("ALL_IMAGES_FAILED", failures.join("; "))
        });
        return NodeOutput::failure_with_logs(err, logs);
    }

    if !failures.is_empty() {
        logs.push(format!(
            "Partial success: {}/{} images generated. {} failed.",
            files.len(), n, failures.len()
        ));
    }

    NodeOutput::success_with_logs(
        json!({ "files": files, "count": files.len(), "source": "imagen4" }),
        logs,
    )
}

async fn call_nano_banana_once(
    client: &reqwest::Client,
    prompt: &str,
    aspect_ratio: &str,
    api_key: &str,
    ref_images: &[Value],
    ts: i64,
    index: usize,
) -> Result<Value, NodeError> {
    // Build parts array: reference images first (as inlineData), then the text prompt.
    // Gemini generateContent accepts inlineData parts alongside text parts in
    // the same content for image editing. Files API fails silently — inlineData required.
    // Source: ai.google.dev/gemini-api/docs/image-generation, June 2026.
    let mut parts: Vec<Value> = Vec::new();
    for img in ref_images {
        let data = img["data"].as_str().unwrap_or("");
        let clean = strip_data_uri_prefix(data);
        if clean.is_empty() { continue; }
        let mime = img["mime_type"].as_str().unwrap_or("image/png");
        parts.push(json!({ "inlineData": { "mimeType": mime, "data": clean } }));
    }
    parts.push(json!({ "text": prompt }));

    let body = json!({
        "contents": [{ "parts": parts }],
        "generationConfig": {
            "responseModalities": ["IMAGE"],
            "imageConfig": { "aspectRatio": aspect_ratio }
        }
    });

    let record = crate::provider::ProviderRegistry::global()
        .get("nano_banana")
        .expect("nano_banana always registered");
    // registry-managed: auth header
    let resp = crate::provider::ProviderRegistry::apply_auth(
        record,
        client.post("https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash-image:generateContent"),
        api_key,
    )
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(network_err)?;

    let status = resp.status().as_u16();
    let json: Value = read_json_response_capped(resp).await.map_err(|e| {
        NodeError::unrecoverable("PARSE_ERROR", e)
    })?;

    if let Some(err) = json["error"].as_object() {
        let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("Unknown error").to_string();
        return Err(match status {
            429 => NodeError::recoverable("RATE_LIMITED", format!("NanoBanana rate limit exceeded. {}", msg)),
            401 | 403 => NodeError::unrecoverable("INVALID_API_KEY", "Invalid or unauthorized Google AI API key. Check your key in Connections."),
            400 if msg.to_ascii_lowercase().contains("safety") || msg.to_ascii_lowercase().contains("policy") => {
                NodeError::unrecoverable("CONTENT_POLICY_VIOLATION", format!("Prompt rejected by safety filters: {}", msg))
            }
            _ => NodeError::unrecoverable("API_ERROR", msg),
        });
    }

    let parts = match json["candidates"][0]["content"]["parts"].as_array() {
        Some(p) if !p.is_empty() => p,
        _ => return Err(NodeError::unrecoverable(
            "EMPTY_RESPONSE",
            "NanoBanana returned no candidates or content parts",
        )),
    };

    // Parts may contain text alongside image -- find the first inlineData entry.
    for part in parts {
        if let Some(inline) = part.get("inlineData") {
            let b64 = inline["data"].as_str().unwrap_or("");
            if b64.is_empty() { continue; }
            let mime     = inline["mimeType"].as_str().unwrap_or("image/png");
            let ext      = mime_to_ext(mime);
            let filename = format!("imagen4_{}_{}.{}", ts, index, ext);
            return Ok(json!({
                "filename":  filename,
                "data":      strip_data_uri_prefix(b64),
                "mime_type": mime
            }));
        }
    }

    Err(NodeError::unrecoverable(
        "EMPTY_RESPONSE",
        "NanoBanana returned no inlineData in response parts",
    ))
}
