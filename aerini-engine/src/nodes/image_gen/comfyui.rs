use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use chrono::Utc;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::NodeOutput;
use crate::nodes::util::{check_host_ssrf_from_url, read_json_response_capped, SsrfPolicy};

use super::shared::{network_err, read_bytes_response_capped};

// -- ComfyUI ------------------------------------------------------------------
//
// Async poll pattern:
//   1. POST /prompt {prompt: workflow} → {prompt_id}
//   2. GET /history/{prompt_id} until status.completed == true (or error/timeout)
//   3. Traverse outputs nodes → images → GET /view?filename=...&subfolder=...&type=output
// SSRF: validated once on base_url (all derived URLs share the same host).
// workflow must be in ComfyUI API format (not the UI JSON format).

const COMFYUI_POLL_MAX_ITERS: u32  = 300;   // 600 s at 2000 ms per poll
const COMFYUI_POLL_INTERVAL_MS: u64 = 2000;

pub(super) async fn gen_comfyui(
    client: reqwest::Client,
    base_url: &str,
    workflow: Value,
) -> NodeOutput {
    // ComfyUI is local-only, same rationale as gen_a1111 above. Single check
    // on base_url covers /prompt, /history, and /view (all derive from the
    // same host).
    if let Err(e) = check_host_ssrf_from_url(base_url, SsrfPolicy::AllowLocal).await {
        return NodeOutput::failure(NodeError::unrecoverable("SSRF_BLOCKED", e));
    }

    let base = base_url.trim_end_matches('/');
    let ts   = Utc::now().timestamp_millis();

    // Submit workflow
    let submit_url = format!("{}/prompt", base);
    let body       = json!({ "prompt": workflow });

    let resp = match client.post(&submit_url).json(&body).send().await {
        Ok(r)  => r,
        Err(e) => return NodeOutput::failure(network_err(e)),
    };

    let status = resp.status().as_u16();
    let json: Value = match read_json_response_capped(resp).await {
        Ok(v)  => v,
        Err(e) => return NodeOutput::failure(NodeError::unrecoverable(
            "PARSE_ERROR",
            format!("ComfyUI submit parse error: {}", e),
        )),
    };

    if status >= 400 {
        let detail = json["error"].as_str()
            .or_else(|| json["message"].as_str())
            .unwrap_or("Unknown error");
        return NodeOutput::failure(NodeError::unrecoverable(
            "API_ERROR",
            format!("ComfyUI submit error {}: {}", status, detail),
        ));
    }

    let prompt_id = match json["prompt_id"].as_str() {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => return NodeOutput::failure(NodeError::unrecoverable(
            "PROTOCOL_ERROR",
            "ComfyUI submit response missing prompt_id",
        )),
    };

    // Poll /history/{prompt_id} until completed
    let history_url = format!("{}/history/{}", base, prompt_id);

    let outputs = match comfyui_poll(&client, &history_url, &prompt_id).await {
        Ok(v)  => v,
        Err(e) => return NodeOutput::failure(e),
    };

    // Traverse output nodes → collect images
    let view_base = format!("{}/view", base);
    let mut files: Vec<Value>  = Vec::new();
    let mut logs:  Vec<String> = Vec::new();
    let mut idx = 0usize;

    if let Some(nodes) = outputs.as_object() {
        for (node_id, node_out) in nodes {
            if let Some(images) = node_out["images"].as_array() {
                for img_meta in images {
                    let filename  = img_meta["filename"].as_str().unwrap_or("");
                    let subfolder = img_meta["subfolder"].as_str().unwrap_or("");
                    let img_type  = img_meta["type"].as_str().unwrap_or("output");

                    if filename.is_empty() {
                        logs.push(format!("ComfyUI node {} image missing filename -- skipped", node_id));
                        continue;
                    }

                    match comfyui_fetch_image(&client, &view_base, filename, subfolder, img_type).await {
                        Ok(b64) => {
                            let ext = filename.rsplit('.').next().unwrap_or("png");
                            let mime = match ext {
                                "jpg" | "jpeg" => "image/jpeg",
                                "webp"         => "image/webp",
                                "gif"          => "image/gif",
                                _              => "image/png",
                            };
                            let out_name = format!("comfyui_{}_{}.{}", ts, idx, ext);
                            files.push(json!({
                                "filename":  out_name,
                                "data":      b64,
                                "mime_type": mime
                            }));
                            logs.push(format!("ComfyUI image {} downloaded", filename));
                            idx += 1;
                        }
                        Err(e) => {
                            logs.push(format!(
                                "ComfyUI image {} download failed: [{}] {}",
                                filename, e.code, e.message
                            ));
                        }
                    }
                }
            }
        }
    }

    if files.is_empty() {
        return NodeOutput::failure_with_logs(
            NodeError::unrecoverable("EMPTY_RESPONSE", "ComfyUI workflow produced no images"),
            logs,
        );
    }

    NodeOutput::success_with_logs(
        json!({ "files": files, "count": files.len(), "source": "comfyui" }),
        logs,
    )
}

async fn comfyui_poll(
    client: &reqwest::Client,
    history_url: &str,
    prompt_id: &str,
) -> Result<Value, NodeError> {
    for _ in 0..COMFYUI_POLL_MAX_ITERS {
        tokio::time::sleep(std::time::Duration::from_millis(COMFYUI_POLL_INTERVAL_MS)).await;

        let resp = client
            .get(history_url)
            .send()
            .await
            .map_err(network_err)?;

        let json: Value = read_json_response_capped(resp).await.map_err(|e| {
            NodeError::unrecoverable("PARSE_ERROR", format!("ComfyUI history parse error: {}", e))
        })?;

        // Empty object {} means the job is not yet in history (queued or running).
        if json.as_object().map(|m| m.is_empty()).unwrap_or(true) {
            continue;
        }

        let entry = &json[prompt_id];

        let status_str = entry["status"]["status_str"].as_str().unwrap_or("");
        if status_str == "error" {
            let msg = entry["status"]["messages"]
                .as_array()
                .and_then(|msgs| msgs.last())
                .and_then(|m| m.as_array())
                .and_then(|pair| pair.get(1))
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown error");
            return Err(NodeError::unrecoverable(
                "GENERATION_FAILED",
                format!("ComfyUI generation error: {}", msg),
            ));
        }

        if entry["status"]["completed"].as_bool().unwrap_or(false) {
            let outputs = entry["outputs"].clone();
            if outputs.is_null() || outputs.as_object().map(|m| m.is_empty()).unwrap_or(true) {
                return Err(NodeError::unrecoverable(
                    "EMPTY_RESPONSE",
                    "ComfyUI completed but produced no outputs",
                ));
            }
            return Ok(outputs);
        }

        // status_str is "success" but completed is false, or still in progress — keep polling.
    }

    Err(NodeError::unrecoverable(
        "TIMEOUT",
        format!(
            "ComfyUI generation timed out after {}s",
            COMFYUI_POLL_MAX_ITERS as u64 * COMFYUI_POLL_INTERVAL_MS / 1000
        ),
    ))
}

async fn comfyui_fetch_image(
    client: &reqwest::Client,
    view_base: &str,
    filename: &str,
    subfolder: &str,
    img_type: &str,
) -> Result<String, NodeError> {
    // Use query() for proper URL encoding of filename/subfolder values.
    // Host is the same as base_url (SSRF-checked once in gen_comfyui).
    let response = client
        .get(view_base)
        .query(&[("filename", filename), ("subfolder", subfolder), ("type", img_type)])
        .send()
        .await
        .map_err(network_err)?;
    // hard-cap the read regardless of what the local ComfyUI
    // server's Content-Length declares (or omits) -- see shared.rs's
    // read_bytes_response_capped doc comment.
    let bytes = read_bytes_response_capped(response)
        .await
        .map_err(|e| NodeError::unrecoverable("DOWNLOAD_ERROR", e))?;
    Ok(BASE64.encode(&bytes))
}

#[cfg(test)]
mod comfyui_fetch_image_tests {
    // confirms comfyui_fetch_image is actually wired to the
    // capped reader end-to-end, not just that shared.rs's helper works in
    // isolation. comfyui_fetch_image does no SSRF check itself (done once
    // upstream in gen_comfyui), so a loopback mock URL reaches it directly
    // -- unlike download_to_base64/flux.rs, no split-out helper is needed
    // here to make this testable.
    use super::*;

    async fn spawn_raw_mock(raw_response: Vec<u8>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock server bind failed");
        let port = listener.local_addr().expect("local_addr failed").port();

        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => return,
            };
            let mut discard = [0u8; 1024];
            let _ = stream.read(&mut discard).await;
            let _ = stream.write_all(&raw_response).await;
            let _ = stream.shutdown().await;
        });

        format!("http://127.0.0.1:{}/view", port)
    }

    #[tokio::test]
    async fn small_image_round_trips_to_base64() {
        let body = b"\x89PNG-not-a-real-png-but-fine-for-this-test";
        let raw = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        let mut raw = raw.into_bytes();
        raw.extend_from_slice(body);
        let view_base = spawn_raw_mock(raw).await;

        let client = reqwest::Client::new();
        let b64 = comfyui_fetch_image(&client, &view_base, "f.png", "", "output")
            .await
            .expect("should succeed");
        assert_eq!(BASE64.decode(&b64).expect("must be valid base64"), body);
    }

    #[tokio::test]
    async fn oversized_image_rejected_not_buffered() {
        let declared = 10 * 1024 * 1024 + 1; // MAX_IMAGE_DOWNLOAD_BYTES + 1
        let raw = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\n\r\n{{}}",
            declared
        );
        let view_base = spawn_raw_mock(raw.into_bytes()).await;

        let client = reqwest::Client::new();
        let err = comfyui_fetch_image(&client, &view_base, "f.png", "", "output")
            .await
            .expect_err("must reject");
        assert_eq!(err.code, "DOWNLOAD_ERROR");
    }
}
