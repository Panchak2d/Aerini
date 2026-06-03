// Image Generation Node
//
// Supports two providers:
//   "dalle3"  — OpenAI DALL-E 3
//              POST https://api.openai.com/v1/images/generations
//              n=1 per request only; loop for n>1 (partial success on loop failure — G4)
//              VERIFIED: OpenAI API reference, May 2026
//
//   "imagen4" — Google Imagen 4 (Imagen 3 was shut down April 2026)
//              POST https://generativelanguage.googleapis.com/v1beta/models/imagen-4.0-generate-001:predict
//              sampleCount = n (1-4 in one request)
//              VERIFIED: Google AI developer docs, May 2026
//
// Output: standard media contract (PLAN.md DATA CONTRACT)
//   { "files": [{"filename": "...", "data": "<raw b64>", "mime_type": "image/png"}], "count": N, "source": "dalle3"|"imagen4" }

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use chrono::Utc;
use serde_json::{json, Value};
use std::sync::OnceLock;

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts};
use crate::nodes::util::check_host_ssrf_from_url;

static IMAGE_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn image_client() -> reqwest::Client {
    IMAGE_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .pool_max_idle_per_host(10)
            // Redirects disabled: a spoofed AI API could redirect to an internal
            // address and bypass the SSRF check in download_to_base64.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("Failed to build image generation HTTP client")
    }).clone()
}

pub struct ImageGenNode;

#[async_trait]
impl Node for ImageGenNode {
    fn type_id(&self) -> &'static str { "image_gen" }
    fn display_name(&self) -> &'static str { "Image Generation" }
    fn node_type(&self) -> NodeType { NodeType::Ai }
    fn version(&self) -> &'static str { "1.0.0" }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["prompt", "provider", "api_key"],
            "properties": {
                "prompt":       { "type": "string",  "description": "Image generation prompt" },
                "provider":     { "type": "string",  "enum": ["dalle3", "imagen4"], "description": "Image provider" },
                "n":            { "type": "number",  "description": "Images to generate (1-4, default 1). DALL-E 3 generates 1 per API call and loops." },
                "size":         { "type": "string",  "enum": ["1024x1024", "1792x1024", "1024x1792"], "description": "DALL-E 3 only. Default: 1024x1024" },
                "quality":      { "type": "string",  "enum": ["standard", "hd"], "description": "DALL-E 3 only. Default: standard" },
                "aspect_ratio": { "type": "string",  "enum": ["1:1", "3:4", "4:3", "9:16", "16:9"], "description": "Imagen 4 only. Default: 1:1" },
                "api_key":      { "type": "string",  "description": "API key — resolved from Connections" }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "files":  { "type": "array",  "description": "Generated images as media objects" },
                "count":  { "type": "number", "description": "Number of images successfully generated" },
                "source": { "type": "string", "description": "Provider that generated the images" }
            }
        })
    }

    fn ports(&self) -> NodePorts { NodePorts::default() }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let cfg = &input.input;

        let prompt = match cfg["prompt"].as_str() {
            Some(p) if !p.trim().is_empty() => p.to_string(),
            _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_PROMPT", "prompt is required")),
        };
        let api_key = match cfg["api_key"].as_str() {
            Some(k) if !k.trim().is_empty() => k.to_string(),
            _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_API_KEY", "api_key is required — add one in Connections")),
        };
        let n = cfg["n"].as_u64().unwrap_or(1).clamp(1, 4) as usize;
        let provider = cfg["provider"].as_str().unwrap_or("dalle3");

        match provider {
            "dalle3" => {
                let size    = cfg["size"].as_str().unwrap_or("1024x1024").to_string();
                let quality = cfg["quality"].as_str().unwrap_or("standard").to_string();
                gen_dalle3(image_client(), &prompt, n, &size, &quality, &api_key).await
            }
            "imagen4" => {
                let aspect = cfg["aspect_ratio"].as_str().unwrap_or("1:1").to_string();
                gen_imagen4(image_client(), &prompt, n, &aspect, &api_key).await
            }
            other => NodeOutput::failure(NodeError::unrecoverable(
                "UNKNOWN_PROVIDER",
                format!("Unknown provider '{}'. Valid: dalle3, imagen4", other),
            )),
        }
    }
}

// ── DALL-E 3 ─────────────────────────────────────────────────────────────────
// DALL-E 3 supports only n=1 per request.
// For n>1: loop, collect successes. On partial failure: return successes + warnings (G4).

async fn gen_dalle3(
    client: reqwest::Client,
    prompt: &str,
    n: usize,
    size: &str,
    quality: &str,
    api_key: &str,
) -> NodeOutput {
    let ts = Utc::now().timestamp_millis();
    let mut files: Vec<Value> = Vec::new();
    let mut logs: Vec<String> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    let mut last_error: Option<NodeError> = None;

    for i in 0..n {
        match call_dalle3_once(&client, prompt, size, quality, api_key, ts, i).await {
            Ok(media_obj) => {
                logs.push(format!("DALL-E 3 image {}/{} generated", i + 1, n));
                files.push(media_obj);
            }
            Err(e) => {
                // G4: log failure, continue collecting other successes.
                // Stop early only for terminal errors where retry won't help.
                let msg = format!("DALL-E 3 image {}/{} failed: [{}] {}", i + 1, n, e.code, e.message);
                logs.push(msg.clone());
                failures.push(msg);
                let is_terminal = e.code == "CONTENT_POLICY_VIOLATION"
                    || e.code == "INVALID_API_KEY"
                    || e.code == "PROMPT_TOO_LONG";
                last_error = Some(e);
                if is_terminal { break; }
            }
        }
    }

    if files.is_empty() {
        let err = last_error.unwrap_or_else(|| NodeError::unrecoverable("ALL_IMAGES_FAILED", failures.join("; ")));
        return NodeOutput::failure_with_logs(err, logs);
    }

    if !failures.is_empty() {
        logs.push(format!(
            "Partial success: {}/{} images generated. {} failed.",
            files.len(), n, failures.len()
        ));
    }

    NodeOutput::success_with_logs(
        json!({ "files": files, "count": files.len(), "source": "dalle3" }),
        logs,
    )
}

async fn call_dalle3_once(
    client: &reqwest::Client,
    prompt: &str,
    size: &str,
    quality: &str,
    api_key: &str,
    ts: i64,
    index: usize,
) -> Result<Value, NodeError> {
    let body = json!({
        "model":           "dall-e-3",
        "prompt":          prompt,
        "n":               1,
        "size":            size,
        "quality":         quality,
        "response_format": "b64_json"
    });

    let resp = client
        .post("https://api.openai.com/v1/images/generations")
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(network_err)?;

    let status = resp.status().as_u16();
    let json: Value = resp.json().await.map_err(|e| {
        NodeError::unrecoverable("PARSE_ERROR", e.to_string())
    })?;

    if let Some(err) = json["error"].as_object() {
        return Err(map_dalle3_error(status, err));
    }

    let data = &json["data"][0];

    if let Some(b64) = data["b64_json"].as_str() {
        // Strip any accidental data: URI prefix
        let raw = strip_data_uri_prefix(b64);
        let mime = "image/png";
        let filename = format!("dalle3_{}_{}.png", ts, index);
        return Ok(json!({ "filename": filename, "data": raw, "mime_type": mime }));
    }

    // Fallback: if b64_json missing but url present (OpenAI returned URL despite b64_json request)
    if let Some(url) = data["url"].as_str() {
        let b64 = download_to_base64(client, url).await?;
        let filename = format!("dalle3_{}_{}.png", ts, index);
        return Ok(json!({ "filename": filename, "data": b64, "mime_type": "image/png" }));
    }

    Err(NodeError::unrecoverable(
        "EMPTY_RESPONSE",
        "DALL-E 3 returned no image data (neither b64_json nor url present)",
    ))
}

fn map_dalle3_error(status: u16, err: &serde_json::Map<String, Value>) -> NodeError {
    let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("Unknown error").to_string();
    let code = err.get("code").and_then(|c| c.as_str()).unwrap_or("").to_string();
    let err_type = err.get("type").and_then(|t| t.as_str()).unwrap_or("").to_string();

    match status {
        429 => NodeError::recoverable("RATE_LIMITED", format!("DALL-E 3 rate limit exceeded. {}", msg)),
        400 if code == "content_policy_violation" || err_type.contains("content_policy") => {
            NodeError::unrecoverable("CONTENT_POLICY_VIOLATION", format!("Prompt rejected by content policy: {}", msg))
        }
        400 if msg.len() > 4000 => {
            NodeError::unrecoverable("PROMPT_TOO_LONG", format!("Prompt exceeds DALL-E 3 character limit: {}", msg))
        }
        401 => NodeError::unrecoverable("INVALID_API_KEY", "Invalid or expired API key. Check your OpenAI key in Connections."),
        _ => NodeError::unrecoverable("API_ERROR", msg),
    }
}

// ── Imagen 4 ──────────────────────────────────────────────────────────────────
// Single request with sampleCount = n.
// Endpoint: POST https://generativelanguage.googleapis.com/v1beta/models/imagen-4.0-generate-001:predict
// Auth: x-goog-api-key header
// Request: { "instances": [{"prompt": "..."}], "parameters": {"sampleCount": N, "aspectRatio": "..."} }
// Response: { "predictions": [{"bytesBase64Encoded": "...", "mimeType": "image/png"}] }
// VERIFIED: Google AI developer docs (ai.google.dev/gemini-api/docs/imagen), May 2026

async fn gen_imagen4(
    client: reqwest::Client,
    prompt: &str,
    n: usize,
    aspect_ratio: &str,
    api_key: &str,
) -> NodeOutput {
    let ts = Utc::now().timestamp_millis();

    let body = json!({
        "instances": [{ "prompt": prompt }],
        "parameters": {
            "sampleCount": n,
            "aspectRatio": aspect_ratio
        }
    });

    let resp = match client
        .post("https://generativelanguage.googleapis.com/v1beta/models/imagen-4.0-generate-001:predict")
        .header("x-goog-api-key", api_key)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return NodeOutput::failure(network_err(e)),
    };

    let status = resp.status().as_u16();
    let json: Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => return NodeOutput::failure(NodeError::unrecoverable("PARSE_ERROR", e.to_string())),
    };

    // Imagen error format: { "error": { "code": 400, "message": "...", "status": "..." } }
    if let Some(err) = json["error"].as_object() {
        let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("Unknown error").to_string();
        return NodeOutput::failure(match status {
            429 => NodeError::recoverable("RATE_LIMITED", format!("Imagen 4 rate limit exceeded. {}", msg)),
            401 | 403 => NodeError::unrecoverable("INVALID_API_KEY", "Invalid or unauthorized API key. Check your Google AI key in Connections."),
            _ => NodeError::unrecoverable("API_ERROR", msg),
        });
    }

    let predictions = match json["predictions"].as_array() {
        Some(p) if !p.is_empty() => p,
        _ => {
            return NodeOutput::failure(NodeError::unrecoverable(
                "EMPTY_RESPONSE",
                "Imagen 4 returned no predictions",
            ))
        }
    };

    let mut files: Vec<Value> = Vec::new();
    let mut logs: Vec<String> = Vec::new();

    for (i, pred) in predictions.iter().enumerate() {
        let b64 = match pred["bytesBase64Encoded"].as_str() {
            Some(b) if !b.is_empty() => strip_data_uri_prefix(b).to_string(),
            _ => {
                logs.push(format!("Imagen 4 prediction {} missing bytesBase64Encoded — skipped", i));
                continue;
            }
        };
        let mime = pred["mimeType"].as_str().unwrap_or("image/png");
        let ext = mime_to_ext(mime);
        let filename = format!("imagen4_{}_{}.{}", ts, i, ext);
        files.push(json!({ "filename": filename, "data": b64, "mime_type": mime }));
        logs.push(format!("Imagen 4 image {}/{} generated", i + 1, predictions.len()));
    }

    if files.is_empty() {
        return NodeOutput::failure_with_logs(
            NodeError::unrecoverable("ALL_IMAGES_FAILED", "No usable predictions in Imagen 4 response"),
            logs,
        );
    }

    NodeOutput::success_with_logs(
        json!({ "files": files, "count": files.len(), "source": "imagen4" }),
        logs,
    )
}

// ── Utilities ─────────────────────────────────────────────────────────────────

fn network_err(e: reqwest::Error) -> NodeError {
    let recoverable = e.is_timeout() || e.is_connect();
    if recoverable {
        NodeError::recoverable("NETWORK_ERROR", e.to_string())
    } else {
        NodeError::unrecoverable("NETWORK_ERROR", e.to_string())
    }
}

/// Strip a "data:<mime>;base64," prefix if present.
/// The DATA CONTRACT requires raw base64 bytes without any data: URI prefix.
fn strip_data_uri_prefix(s: &str) -> &str {
    if let Some(comma_pos) = s.find(',') {
        if s[..comma_pos].starts_with("data:") {
            return &s[comma_pos + 1..];
        }
    }
    s
}

fn mime_to_ext(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp"               => "webp",
        "image/gif"                => "gif",
        _                          => "png",
    }
}

/// Download image from URL and return raw base64 bytes (no data: URI prefix).
async fn download_to_base64(client: &reqwest::Client, url: &str) -> Result<String, NodeError> {
    // Validate the URL before fetching — a compromised or spoofed AI API could
    // return an internal URL (e.g. AWS metadata endpoint) to extract infrastructure
    // data via the image download path.
    check_host_ssrf_from_url(url).await.map_err(|e| {
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
