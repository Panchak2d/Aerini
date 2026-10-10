use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use chrono::Utc;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::NodeOutput;
use crate::nodes::util::{
    check_host_ssrf_from_url, poll_network_fault, poll_status_fault, read_json_response_capped,
    reqwest_err_msg, PollFault, PollRetry, SsrfPolicy,
};

use super::shared::{fetch_image_bytes, network_err, DOWNLOAD_BACKOFF_MS};

// -- ComfyUI ------------------------------------------------------------------
//
// Async poll pattern:
//   1. POST /prompt {prompt: workflow} → {prompt_id}
//   2. GET /history/{prompt_id} until status.completed == true (or error/timeout)
//   3. Traverse outputs nodes → images → GET /view?filename=...&subfolder=...&type=output
//      Preview images (type "temp") are skipped when the workflow also saves
//      images; a workflow that only previews still returns them.
//      Any image that cannot be downloaded fails the node.
// A job still queued or running when the node gives up or is cancelled is
// cancelled on the server (GET /queue, then POST /interrupt or POST /queue).
// SSRF: validated once on base_url (all derived URLs share the same host).
// workflow must be in ComfyUI API format (not the UI JSON format).

const COMFYUI_POLL_MAX_ITERS: u32  = 300;   // 600 s at 2000 ms per poll
const COMFYUI_POLL_INTERVAL_MS: u64 = 2000;
const CANCEL_REQUEST_TIMEOUT_SECS: u64 = 5;

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
        Err(e) => return NodeOutput::failure(crate::nodes::util::provider_error(
            status,
            "PARSE_ERROR",
            format!("ComfyUI submit parse error: {}", e),
        )),
    };

    if status >= 400 {
        let code = if status == 400 { "VALIDATION_ERROR" } else { "API_ERROR" };
        return NodeOutput::failure(NodeError::unrecoverable(
            code,
            format!("ComfyUI submit error {}: {}", status, submit_error_detail(&json)),
        ));
    }

    let prompt_id = match json["prompt_id"].as_str() {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => return NodeOutput::failure(NodeError::unrecoverable(
            "PROTOCOL_ERROR",
            "ComfyUI submit response missing prompt_id",
        )),
    };

    let mut guard = CancelOnDrop::new(client.clone(), base, &prompt_id);

    // Poll /history/{prompt_id} until completed
    let history_url = format!("{}/history/{}", base, prompt_id);

    let outputs = match comfyui_poll(&client, &history_url, &prompt_id, COMFYUI_POLL_INTERVAL_MS).await {
        Ok(v) => {
            guard.disarm();
            v
        }
        Err(e) => {
            guard.disarm();
            if matches!(e.code.as_str(), "TIMEOUT" | "POLL_FAILED" | "POLL_REJECTED") {
                let outcome = cancel_job(&client, base, &prompt_id).await;
                return NodeOutput::failure(NodeError::unrecoverable(
                    e.code,
                    format!("{} {}", e.message, outcome),
                ));
            }
            return NodeOutput::failure(e);
        }
    };

    // Traverse output nodes → collect images
    let view_base = format!("{}/view", base);
    let mut files: Vec<Value>  = Vec::new();
    let (images, mut logs) = collect_images(&outputs);

    for (idx, img) in images.iter().enumerate() {
        match comfyui_fetch_image(&client, &view_base, &img.filename, &img.subfolder, &img.img_type).await {
            Ok(b64) => {
                let ext = img.filename.rsplit('.').next().unwrap_or("png");
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
                logs.push(format!("ComfyUI image {} downloaded", img.filename));
            }
            Err(e) => {
                return NodeOutput::failure_with_logs(
                    NodeError::unrecoverable(
                        e.code,
                        format!("ComfyUI image {} could not be downloaded: {}", img.filename, e.message),
                    ),
                    logs,
                );
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

/// Cancels a job on the ComfyUI server and describes what happened. A running
/// job is interrupted by id, a pending one is removed from the queue. When the
/// queue cannot be read, nothing is interrupted: an older server may ignore
/// the id and stop another person's running job.
async fn cancel_job(client: &reqwest::Client, base: &str, prompt_id: &str) -> String {
    let timeout = std::time::Duration::from_secs(CANCEL_REQUEST_TIMEOUT_SECS);
    let unconfirmed = |why: String| {
        format!("The job could not be cancelled ({why}) and may still run on the ComfyUI server.")
    };

    let resp = match client.get(format!("{base}/queue")).timeout(timeout).send().await {
        Ok(r) => r,
        Err(e) => return unconfirmed(format!("queue not readable: {}", reqwest_err_msg(&e))),
    };
    let status = resp.status();
    if !status.is_success() {
        return unconfirmed(format!("queue not readable: HTTP {}", status.as_u16()));
    }
    let queue = match read_json_response_capped(resp).await {
        Ok(v) => v,
        Err(e) => return unconfirmed(format!("queue not readable: {e}")),
    };

    let holds = |key: &str| {
        queue[key]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item.get(1).and_then(Value::as_str) == Some(prompt_id)))
    };

    let (request, done) = if holds("queue_running") {
        (
            client.post(format!("{base}/interrupt")).json(&json!({ "prompt_id": prompt_id })),
            "The running job was interrupted on the ComfyUI server.",
        )
    } else if holds("queue_pending") {
        (
            client.post(format!("{base}/queue")).json(&json!({ "delete": [prompt_id] })),
            "The queued job was removed from the ComfyUI queue.",
        )
    } else {
        return "The job had already finished on the ComfyUI server.".to_string();
    };

    match request.timeout(timeout).send().await {
        Ok(r) if r.status().is_success() => done.to_string(),
        Ok(r) => unconfirmed(format!("cancel refused: HTTP {}", r.status().as_u16())),
        Err(e) => unconfirmed(format!("cancel request failed: {}", reqwest_err_msg(&e))),
    }
}

/// Cancels the submitted job if the node's future is dropped (run cancelled)
/// before the job ended. Dropping is the only signal a cancelled node gets.
struct CancelOnDrop {
    client: reqwest::Client,
    base: String,
    prompt_id: String,
    armed: bool,
}

impl CancelOnDrop {
    fn new(client: reqwest::Client, base: &str, prompt_id: &str) -> Self {
        Self { client, base: base.to_string(), prompt_id: prompt_id.to_string(), armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else { return };
        let (client, base, prompt_id) = (self.client.clone(), self.base.clone(), self.prompt_id.clone());
        handle.spawn(async move {
            cancel_job(&client, &base, &prompt_id).await;
        });
    }
}

struct OutputImage {
    filename:  String,
    subfolder: String,
    img_type:  String,
}

/// Collects every image the workflow reported. Images of type `temp` come from
/// preview nodes; they are dropped when at least one image is not a preview.
fn collect_images(outputs: &Value) -> (Vec<OutputImage>, Vec<String>) {
    let mut found: Vec<OutputImage> = Vec::new();
    let mut logs: Vec<String> = Vec::new();

    if let Some(nodes) = outputs.as_object() {
        for (node_id, node_out) in nodes {
            let Some(images) = node_out["images"].as_array() else { continue };
            for meta in images {
                let filename = meta["filename"].as_str().unwrap_or("");
                if filename.is_empty() {
                    logs.push(format!("ComfyUI node {} image missing filename -- skipped", node_id));
                    continue;
                }
                found.push(OutputImage {
                    filename:  filename.to_string(),
                    subfolder: meta["subfolder"].as_str().unwrap_or("").to_string(),
                    img_type:  meta["type"].as_str().unwrap_or("output").to_string(),
                });
            }
        }
    }

    if found.iter().any(|i| i.img_type != "temp") {
        let before = found.len();
        found.retain(|i| i.img_type != "temp");
        if found.len() < before {
            logs.push(format!("ComfyUI preview images skipped: {}", before - found.len()));
        }
    }
    (found, logs)
}

const MAX_NODE_ERRORS: usize = 5;
const MAX_ERROR_DETAIL_CHARS: usize = 1500;

fn one_error(e: &Value) -> String {
    let message = e["message"].as_str().unwrap_or("");
    let details = e["details"].as_str().unwrap_or("");
    let kind    = e["type"].as_str().unwrap_or("");
    match (message.is_empty(), details.is_empty()) {
        (false, false) => format!("{}: {}", message, details),
        (false, true)  => message.to_string(),
        (true, false)  => details.to_string(),
        (true, true)   => kind.to_string(),
    }
}

/// Reads a `/prompt` rejection: `error` (an object with type, message,
/// details; a string is accepted too) plus `node_errors`, a map of node id to
/// `{ class_type, errors: [{type, message, details}] }` naming the failing nodes.
fn submit_error_detail(json: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();

    let top = match &json["error"] {
        Value::String(s) => s.clone(),
        Value::Object(_) => one_error(&json["error"]),
        _ => json["message"].as_str().unwrap_or("").to_string(),
    };
    if !top.is_empty() {
        parts.push(top);
    }

    if let Some(nodes) = json["node_errors"].as_object() {
        let mut lines: Vec<String> = Vec::new();
        for (node_id, entry) in nodes {
            let class = entry["class_type"].as_str().unwrap_or("unknown class");
            let reasons: Vec<String> = entry["errors"]
                .as_array()
                .map(|errs| errs.iter().map(one_error).filter(|r| !r.is_empty()).collect())
                .unwrap_or_default();
            if !reasons.is_empty() {
                lines.push(format!("node {} ({}): {}", node_id, class, reasons.join(", ")));
            }
        }
        let shown = lines.len().min(MAX_NODE_ERRORS);
        if shown > 0 {
            let mut text = lines[..shown].join("; ");
            if lines.len() > shown {
                text.push_str(&format!("; and {} more", lines.len() - shown));
            }
            parts.push(text);
        }
    }

    if parts.is_empty() {
        return "Unknown error".to_string();
    }
    let joined = parts.join(" | ");
    if joined.chars().count() > MAX_ERROR_DETAIL_CHARS {
        let cut: String = joined.chars().take(MAX_ERROR_DETAIL_CHARS).collect();
        return format!("{}…", cut);
    }
    joined
}

/// Reads why a job failed from its history entry: the last `execution_error`
/// message (`["execution_error", {exception_message, node_type, node_id, ...}]`),
/// or the name of the last message when none is an `execution_error`.
fn failure_message(entry: &Value) -> String {
    let messages = entry["status"]["messages"].as_array();
    let pairs = || messages.into_iter().flatten().filter_map(|m| m.as_array());

    if let Some(err) = pairs().rfind(|p| p.first().and_then(|n| n.as_str()) == Some("execution_error")) {
        let detail = err.get(1).cloned().unwrap_or(Value::Null);
        let text = detail["exception_message"].as_str().unwrap_or("").trim();
        let node_type = detail["node_type"].as_str().unwrap_or("");
        let node_id = match &detail["node_id"] {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            _ => String::new(),
        };
        let where_ = match (node_type.is_empty(), node_id.is_empty()) {
            (false, false) => format!(" (node {}, {})", node_id, node_type),
            (false, true)  => format!(" ({})", node_type),
            (true, false)  => format!(" (node {})", node_id),
            (true, true)   => String::new(),
        };
        if !text.is_empty() {
            return format!("{}{}", text, where_);
        }
    }

    match pairs().next_back().and_then(|p| p.first()).and_then(|n| n.as_str()) {
        Some(name) => format!("job ended with {}", name),
        None => "Unknown error".to_string(),
    }
}

async fn comfyui_poll(
    client: &reqwest::Client,
    history_url: &str,
    prompt_id: &str,
    interval_ms: u64,
) -> Result<Value, NodeError> {
    let mut retry = PollRetry::new("ComfyUI");

    for _ in 0..COMFYUI_POLL_MAX_ITERS {
        tokio::time::sleep(std::time::Duration::from_millis(retry.delay_ms(interval_ms))).await;

        let resp = match client.get(history_url).send().await {
            Ok(r) => r,
            Err(e) => {
                retry.fault(poll_network_fault(&e))?;
                continue;
            }
        };

        if let Some(fault) = poll_status_fault(resp.status().as_u16()) {
            retry.fault(fault)?;
            continue;
        }

        let json: Value = match read_json_response_capped(resp).await {
            Ok(v) => v,
            Err(e) => {
                retry.fault(PollFault::Transient(format!("unreadable body: {}", e)))?;
                continue;
            }
        };
        retry.good();

        // Empty object {} means the job is not yet in history (queued or running).
        if json.as_object().map(|m| m.is_empty()).unwrap_or(true) {
            continue;
        }

        let entry = &json[prompt_id];

        let status_str = entry["status"]["status_str"].as_str().unwrap_or("");
        if status_str == "error" {
            return Err(NodeError::unrecoverable(
                "GENERATION_FAILED",
                format!("ComfyUI generation error: {}", failure_message(entry)),
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
            "ComfyUI generation timed out after {}s (prompt_id {}).",
            COMFYUI_POLL_MAX_ITERS as u64 * interval_ms / 1000,
            prompt_id
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
    // query() URL-encodes filename and subfolder. The host is base_url's,
    // SSRF-checked once in gen_comfyui. The read is capped whatever the local
    // server's Content-Length says (see shared.rs).
    let bytes = fetch_image_bytes(
        || client.get(view_base).query(&[("filename", filename), ("subfolder", subfolder), ("type", img_type)]),
        DOWNLOAD_BACKOFF_MS,
    )
    .await?;
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
    async fn a_missing_image_is_an_error_not_a_png() {
        let body = "{\"error\": \"file not found\"}";
        let raw = format!(
            "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        let view_base = spawn_raw_mock(raw.into_bytes()).await;

        let client = reqwest::Client::new();
        let err = comfyui_fetch_image(&client, &view_base, "f.png", "", "output")
            .await
            .expect_err("a 404 must not become an image");
        assert_eq!(err.code, "DOWNLOAD_ERROR");
        assert!(err.message.contains("404"), "{}", err.message);
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

#[cfg(test)]
mod comfyui_poll_tests {
    use super::*;
    use crate::nodes::util::poll_test_support::spawn_sequence_mock;

    const DONE: &str = r#"{"p1":{"status":{"status_str":"success","completed":true},"outputs":{"9":{"images":[]}}}}"#;

    async fn run_poll(responses: Vec<(u16, &'static str)>) -> Result<Value, NodeError> {
        let url = spawn_sequence_mock(responses).await;
        comfyui_poll(&reqwest::Client::new(), &format!("{url}/history/p1"), "p1", 1).await
    }

    #[tokio::test]
    async fn comfyui_poll_survives_transient_failures_and_a_good_poll_resets_the_streak() {
        let outputs = run_poll(vec![
            (503, "x"), (200, "<html>"), (500, "x"), (503, "x"),
            (200, "{}"),
            (503, "x"), (503, "x"), (503, "x"), (503, "x"),
            (200, DONE),
        ])
        .await
        .expect("should complete");
        assert!(outputs["9"].is_object());
    }

    #[tokio::test]
    async fn comfyui_poll_json_4xx_fails_immediately() {
        let e = run_poll(vec![(404, "{}")]).await.expect_err("must fail");
        assert_eq!(e.code, "POLL_REJECTED");
        assert!(!e.recoverable);
    }

    #[tokio::test]
    async fn comfyui_poll_network_errors_are_unrecoverable_so_no_job_is_resubmitted() {
        let e = run_poll(vec![]).await.expect_err("must give up");
        assert_eq!(e.code, "POLL_FAILED");
        assert!(!e.recoverable);
        assert!(e.message.contains("not resubmitted"));
    }
}

#[cfg(test)]
mod comfyui_shape_tests {
    use super::*;

    #[test]
    fn submit_error_object_and_node_errors_show_the_cause() {
        let body = json!({
            "error": { "type": "prompt_outputs_failed_validation", "message": "Prompt outputs failed validation", "details": "", "extra_info": {} },
            "node_errors": {
                "4": {
                    "class_type": "CheckpointLoaderSimple",
                    "dependent_outputs": ["9"],
                    "errors": [{ "type": "value_not_in_list", "message": "Value not in list", "details": "ckpt_name: 'x.safetensors' not in ['a.safetensors']", "extra_info": {} }]
                }
            }
        });
        let detail = submit_error_detail(&body);
        assert!(detail.contains("Prompt outputs failed validation"), "{detail}");
        assert!(detail.contains("node 4 (CheckpointLoaderSimple)"), "{detail}");
        assert!(detail.contains("ckpt_name: 'x.safetensors'"), "{detail}");
    }

    #[test]
    fn submit_error_accepts_a_string_a_message_and_an_empty_body() {
        assert_eq!(submit_error_detail(&json!({ "error": "bad" })), "bad");
        assert_eq!(submit_error_detail(&json!({ "message": "oops" })), "oops");
        assert_eq!(submit_error_detail(&json!({})), "Unknown error");
    }

    #[test]
    fn submit_error_lists_five_nodes_and_counts_the_rest() {
        let mut nodes = serde_json::Map::new();
        for i in 0..8 {
            nodes.insert(i.to_string(), json!({ "class_type": "K", "errors": [{ "message": "bad" }] }));
        }
        let detail = submit_error_detail(&json!({ "node_errors": nodes }));
        assert_eq!(detail.matches("(K)").count(), 5, "{detail}");
        assert!(detail.contains("and 3 more"), "{detail}");
    }

    #[test]
    fn failed_job_message_reads_the_execution_error_object() {
        let entry = json!({ "status": { "status_str": "error", "messages": [
            ["execution_start", { "prompt_id": "p1" }],
            ["execution_error", { "node_id": "3", "node_type": "KSampler", "exception_message": "CUDA out of memory", "exception_type": "OutOfMemoryError" }],
        ] } });
        assert_eq!(failure_message(&entry), "CUDA out of memory (node 3, KSampler)");
    }

    #[test]
    fn failed_job_without_an_execution_error_names_the_last_message() {
        let entry = json!({ "status": { "messages": [["execution_start", {}], ["execution_interrupted", {}]] } });
        assert_eq!(failure_message(&entry), "job ended with execution_interrupted");
        assert_eq!(failure_message(&json!({})), "Unknown error");
    }

    fn out(images: Value) -> Value { json!({ "9": { "images": images }, "12": { "images": [] } }) }

    #[test]
    fn preview_images_are_dropped_when_the_workflow_also_saves_images() {
        let outputs = json!({
            "9":  { "images": [{ "filename": "saved.png", "subfolder": "", "type": "output" }] },
            "10": { "images": [{ "filename": "preview.png", "subfolder": "", "type": "temp" }] },
        });
        let (images, logs) = collect_images(&outputs);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].filename, "saved.png");
        assert!(logs.iter().any(|l| l.contains("preview images skipped: 1")), "{logs:?}");
    }

    #[test]
    fn a_workflow_that_only_previews_still_returns_its_images() {
        let (images, _) = collect_images(&out(json!([{ "filename": "p.png", "type": "temp" }])));
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].img_type, "temp");
    }

    #[test]
    fn an_image_without_a_filename_is_logged_and_skipped() {
        let (images, logs) = collect_images(&out(json!([{ "subfolder": "" }, { "filename": "ok.png" }])));
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].img_type, "output");
        assert!(logs.iter().any(|l| l.contains("missing filename")), "{logs:?}");
    }
}

#[cfg(test)]
mod comfyui_cancel_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    const RUNNING: &str = r#"{"queue_running":[[0,"p1",{},{},["9"]]],"queue_pending":[]}"#;
    const PENDING: &str = r#"{"queue_running":[[0,"other",{},{},["9"]]],"queue_pending":[[1,"p1",{},{},["9"]]]}"#;
    const NEITHER: &str = r#"{"queue_running":[],"queue_pending":[]}"#;

    async fn recording_mock(queue_status: u16, queue_body: &'static str) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind failed");
        let port = listener.local_addr().expect("local_addr failed").port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            loop {
                let Ok((mut stream, _)) = listener.accept().await else { return };
                let mut buf: Vec<u8> = Vec::new();
                let mut chunk = [0u8; 1024];
                let (head_end, content_len) = loop {
                    let n = stream.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break (buf.len(), 0);
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..pos]).to_ascii_lowercase();
                        let len = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok()))
                            .unwrap_or(0);
                        break (pos + 4, len);
                    }
                };
                while buf.len() < head_end + content_len {
                    let n = stream.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                let text = String::from_utf8_lossy(&buf).to_string();
                let request_line = text.lines().next().unwrap_or("").to_string();
                let body = text.get(head_end..).unwrap_or("").to_string();
                let is_queue_read = request_line.starts_with("GET /queue");
                log.lock().expect("log lock").push(format!(
                    "{} {}",
                    request_line.trim_end_matches(" HTTP/1.1"),
                    body
                ).trim_end().to_string());
                let (status, reply) = if is_queue_read { (queue_status, queue_body) } else { (200, "") };
                let raw = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
                let _ = stream.write_all(raw.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });
        (format!("http://127.0.0.1:{port}"), seen)
    }

    fn requests(seen: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
        seen.lock().expect("log lock").clone()
    }

    #[tokio::test]
    async fn a_running_job_is_interrupted_by_id() {
        let (base, seen) = recording_mock(200, RUNNING).await;
        let outcome = cancel_job(&reqwest::Client::new(), &base, "p1").await;
        assert!(outcome.contains("interrupted"), "{outcome}");
        assert_eq!(requests(&seen), vec!["GET /queue", r#"POST /interrupt {"prompt_id":"p1"}"#]);
    }

    #[tokio::test]
    async fn a_pending_job_is_deleted_from_the_queue_and_never_interrupted() {
        let (base, seen) = recording_mock(200, PENDING).await;
        let outcome = cancel_job(&reqwest::Client::new(), &base, "p1").await;
        assert!(outcome.contains("removed"), "{outcome}");
        assert_eq!(requests(&seen), vec!["GET /queue", r#"POST /queue {"delete":["p1"]}"#]);
    }

    #[tokio::test]
    async fn an_unknown_job_gets_no_cancel_request() {
        let (base, seen) = recording_mock(200, NEITHER).await;
        let outcome = cancel_job(&reqwest::Client::new(), &base, "p1").await;
        assert!(outcome.contains("already finished"), "{outcome}");
        assert_eq!(requests(&seen), vec!["GET /queue"]);
    }

    #[tokio::test]
    async fn an_unreadable_queue_means_no_interrupt() {
        let (base, seen) = recording_mock(500, "boom").await;
        let outcome = cancel_job(&reqwest::Client::new(), &base, "p1").await;
        assert!(outcome.contains("could not be cancelled") && outcome.contains("500"), "{outcome}");
        assert_eq!(requests(&seen), vec!["GET /queue"]);
    }

    #[tokio::test]
    async fn dropping_an_armed_guard_cancels_the_job() {
        let (base, seen) = recording_mock(200, RUNNING).await;
        drop(CancelOnDrop::new(reqwest::Client::new(), &base, "p1"));
        for _ in 0..40 {
            if requests(&seen).len() == 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(requests(&seen), vec!["GET /queue", r#"POST /interrupt {"prompt_id":"p1"}"#]);
    }

    #[tokio::test]
    async fn dropping_a_disarmed_guard_sends_nothing() {
        let (base, seen) = recording_mock(200, RUNNING).await;
        let mut guard = CancelOnDrop::new(reqwest::Client::new(), &base, "p1");
        guard.disarm();
        drop(guard);
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(requests(&seen).is_empty());
    }
}
