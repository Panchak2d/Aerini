use chrono::Utc;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::NodeOutput;
use crate::nodes::util::{check_host_ssrf_from_url, SsrfPolicy};

use super::shared::{network_err, download_to_base64};

// -- BFL FLUX (flux-pro-1.1 and flux-2-pro) -----------------------------------
//
// Both models share the same async polling protocol:
//   1. POST submit -> { id, polling_url }
//   2. GET polling_url (x-key auth) until status == "Ready" or terminal failure
//   3. Download result.sample (expires 10 min) -> base64 -> return in contract
// n>1: loop sequentially (no native batch endpoint).

const BFL_POLL_MAX_ITERS: u32  = 240;  // 120 s at 500 ms per poll
const BFL_POLL_INTERVAL_MS: u64 = 500;

// Bundles the per-request fields shared by gen_flux and call_flux_one --
// clippy::too_many_arguments (max 7) was exceeded by both (8 and 9 args).
pub(super) struct FluxRequest<'a> {
    pub(super) endpoint: &'a str, // "/v1/flux-pro-1.1" or "/v1/flux-2-pro"
    pub(super) source:   &'a str, // "flux_pro" or "flux_2_pro"
    pub(super) prompt:   &'a str,
    pub(super) width:    u32,
    pub(super) height:   u32,
    pub(super) api_key:  &'a str,
}

pub(super) async fn gen_flux(client: reqwest::Client, req: FluxRequest<'_>, n: usize) -> NodeOutput {
    let ts = Utc::now().timestamp_millis();
    let mut files: Vec<Value>     = Vec::new();
    let mut logs:  Vec<String>    = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    let mut last_error: Option<NodeError> = None;

    for i in 0..n {
        match call_flux_one(&client, &req, ts, i).await {
            Ok(media_obj) => {
                logs.push(format!("{} image {}/{} generated", req.source, i + 1, n));
                files.push(media_obj);
            }
            Err(e) => {
                let msg = format!("{} image {}/{} failed: [{}] {}", req.source, i + 1, n, e.code, e.message);
                logs.push(msg.clone());
                failures.push(msg);
                let is_terminal = matches!(e.code.as_str(), "INVALID_API_KEY" | "INSUFFICIENT_CREDITS");
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
        json!({ "files": files, "count": files.len(), "source": req.source }),
        logs,
    )
}

async fn call_flux_one(
    client: &reqwest::Client,
    req: &FluxRequest<'_>,
    ts: i64,
    index: usize,
) -> Result<Value, NodeError> {
    let url = format!("https://api.bfl.ai{}", req.endpoint);

    let body = json!({
        "prompt":        req.prompt,
        "width":         req.width,
        "height":        req.height,
        "output_format": "png"
    });

    let resp = client
        .post(&url)
        .header("x-key", req.api_key)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(network_err)?;

    let status = resp.status().as_u16();
    let json: Value = resp.json().await.map_err(|e| {
        NodeError::unrecoverable("PARSE_ERROR", e.to_string())
    })?;

    if status >= 400 {
        // BFL validation errors use "detail" (string or array); other errors use "message".
        let detail = match &json["detail"] {
            Value::String(s) => s.clone(),
            Value::Null      => json["message"].as_str().unwrap_or("Unknown error").to_string(),
            other            => other.to_string(),
        };
        return Err(match status {
            401 | 403 => NodeError::unrecoverable("INVALID_API_KEY", "Invalid or unauthorized BFL API key. Check your key in Connections."),
            402        => NodeError::unrecoverable("INSUFFICIENT_CREDITS", "Insufficient BFL API credits. Add credits at bfl.ai."),
            422        => NodeError::unrecoverable("VALIDATION_ERROR", format!("Invalid BFL request parameters: {}", detail)),
            429        => NodeError::recoverable("RATE_LIMITED", format!("BFL rate limit exceeded. {}", detail)),
            _          => NodeError::unrecoverable("API_ERROR", format!("BFL API error {}: {}", status, detail)),
        });
    }

    let polling_url = json["polling_url"]
        .as_str()
        .ok_or_else(|| NodeError::unrecoverable("PROTOCOL_ERROR", "BFL response missing polling_url"))?
        .to_string();

    let image_url = bfl_poll(client, &polling_url, req.api_key).await?;
    let b64       = download_to_base64(client, &image_url).await?;
    let filename  = format!("{}_{}_{}.png", req.source, ts, index);

    Ok(json!({ "filename": filename, "data": b64, "mime_type": "image/png" }))
}

async fn bfl_poll(
    client: &reqwest::Client,
    polling_url: &str,
    api_key: &str,
) -> Result<String, NodeError> {
    // Validate the polling URL before using it. A compromised or MITM'd API
    // response could supply an internal address (e.g. cloud metadata endpoint).
    check_host_ssrf_from_url(polling_url, SsrfPolicy::Strict).await.map_err(|e| {
        NodeError::unrecoverable("SSRF_BLOCKED", e)
    })?;

    for _ in 0..BFL_POLL_MAX_ITERS {
        tokio::time::sleep(std::time::Duration::from_millis(BFL_POLL_INTERVAL_MS)).await;

        let resp = client
            .get(polling_url)
            .header("x-key", api_key)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(network_err)?;

        let json: Value = resp.json().await.map_err(|e| {
            NodeError::unrecoverable("PARSE_ERROR", format!("BFL poll parse error: {}", e))
        })?;

        match json["status"].as_str() {
            Some("Ready") => {
                return json["result"]["sample"]
                    .as_str()
                    .map(|s| s.to_string())
                    .ok_or_else(|| NodeError::unrecoverable(
                        "EMPTY_RESPONSE",
                        "BFL returned Ready but result.sample is missing",
                    ));
            }
            Some(s) if matches!(s, "Error" | "Failed" | "Content Moderated" | "Request Moderated") => {
                let detail = json["result"].as_str()
                    .or_else(|| json["error"].as_str())
                    .unwrap_or("No detail");
                return Err(NodeError::unrecoverable(
                    "GENERATION_FAILED",
                    format!("BFL generation {}: {}", s, detail),
                ));
            }
            _ => {} // Pending / Processing -- continue polling
        }
    }

    Err(NodeError::unrecoverable(
        "TIMEOUT",
        format!(
            "BFL generation timed out after {}s",
            BFL_POLL_MAX_ITERS as u64 * BFL_POLL_INTERVAL_MS / 1000
        ),
    ))
}
