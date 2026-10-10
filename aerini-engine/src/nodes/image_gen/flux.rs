use chrono::Utc;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::NodeOutput;
use crate::nodes::util::{
    check_host_ssrf_from_url, poll_network_fault, poll_status_fault,
    read_json_response_capped, PollFault, PollRetry, SsrfPolicy,
};

use super::shared::{network_err, download_to_base64, ImageRun};

// -- BFL FLUX (flux-pro-1.1 and flux-2-pro) -----------------------------------
//
// Both models share the same async polling protocol:
//   1. POST submit -> { id, polling_url }
//   2. GET polling_url (x-key auth) until status == "Ready" or terminal failure
//   3. Download result.sample (expires 10 min) -> base64 -> return in contract
// n>1: loop sequentially (no native batch endpoint). The loop stops on the codes
// in shared::stops_the_loop; any other failure is skipped and the rest run.

const BFL_POLL_MAX_ITERS: u32  = 240;  // 120 s at 500 ms per poll
const BFL_POLL_INTERVAL_MS: u64 = 500;

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
    let mut run = ImageRun::new(format!("{} image", req.source), n);

    for i in 0..n {
        match call_flux_one(&client, &req, ts, i).await {
            Ok(media_obj) => run.generated(i, media_obj),
            Err(e) => {
                if run.failed(i, e) { break; }
            }
        }
    }

    run.finish(req.source)
}

const BFL_NOT_FOUND_MAX: u32 = 5;
const BFL_NOT_FOUND_DELAY_CAP_MS: u64 = 8_000;

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

    let record = crate::provider::ProviderRegistry::global()
        .get(req.source)
        .expect("flux_pro/flux_2_pro always registered");

    let resp = crate::provider::ProviderRegistry::apply_auth(
        record,
        client.post(&url),
        req.api_key,
    )
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(network_err)?;

    let status = resp.status().as_u16();
    let json: Value = read_json_response_capped(resp).await.map_err(|e| {
        crate::nodes::util::provider_error(status, "PARSE_ERROR", e)
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
    let job_id = json["id"].as_str().unwrap_or("unknown");

    let image_url = bfl_poll(client, record, &polling_url, req.api_key)
        .await
        .map_err(|e| timeout_with_job_ref(e, job_id, &polling_url))?;
    let b64       = download_to_base64(client, &image_url).await?;
    let filename  = format!("{}_{}_{}.png", req.source, ts, index);

    Ok(json!({ "filename": filename, "data": b64, "mime_type": "image/png" }))
}

fn timeout_with_job_ref(e: NodeError, job_id: &str, polling_url: &str) -> NodeError {
    if e.code != "TIMEOUT" && e.code != "TASK_NOT_FOUND" {
        return e;
    }
    NodeError::unrecoverable(
        e.code,
        format!("{} Job id: {}. Polling URL: {}", e.message, job_id, polling_url),
    )
}

fn moderated_error(status: &str, json: &Value) -> NodeError {
    let what = if status == "Request Moderated" { "rejected the prompt" } else { "withheld the generated image" };
    let detail = match &json["details"] {
        Value::Null => String::new(),
        Value::Object(m) if m.is_empty() => String::new(),
        d => format!(": {}", d.to_string().chars().take(300).collect::<String>()),
    };
    NodeError::unrecoverable("CONTENT_POLICY_VIOLATION", format!("BFL {} ({}){}", what, status, detail))
}

async fn bfl_poll(
    client: &reqwest::Client,
    record: &crate::provider::ProviderRecord,
    polling_url: &str,
    api_key: &str,
) -> Result<String, NodeError> {
    // Validate the polling URL before using it. A compromised or MITM'd API
    // response could supply an internal address (e.g. cloud metadata endpoint).
    check_host_ssrf_from_url(polling_url, SsrfPolicy::Strict).await.map_err(|e| {
        NodeError::unrecoverable("SSRF_BLOCKED", e)
    })?;

    bfl_poll_loop(client, record, polling_url, api_key, BFL_POLL_INTERVAL_MS, BFL_POLL_MAX_ITERS).await
}

async fn bfl_poll_loop(
    client: &reqwest::Client,
    record: &crate::provider::ProviderRecord,
    polling_url: &str,
    api_key: &str,
    interval_ms: u64,
    max_iters: u32,
) -> Result<String, NodeError> {
    let mut retry = PollRetry::new("BFL");
    let mut not_found: u32 = 0;

    for _ in 0..max_iters {
        let not_found_delay = interval_ms
            .saturating_mul(1u64 << not_found.min(4))
            .min(BFL_NOT_FOUND_DELAY_CAP_MS.max(interval_ms));
        let delay = retry.delay_ms(interval_ms).max(if not_found > 0 { not_found_delay } else { 0 });
        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;

        let resp = match crate::provider::ProviderRegistry::apply_auth(
            record,
            client.get(polling_url),
            api_key,
        )
            .header("Accept", "application/json")
            .send()
            .await
        {
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

        if json["status"].as_str() == Some("Task not found") {
            not_found += 1;
            if not_found >= BFL_NOT_FOUND_MAX {
                return Err(NodeError::unrecoverable(
                    "TASK_NOT_FOUND",
                    format!(
                        "BFL reported \"Task not found\" {} times in a row. The job was not resubmitted.",
                        not_found
                    ),
                ));
            }
            continue;
        }
        not_found = 0;

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
            Some(s @ ("Request Moderated" | "Content Moderated")) => {
                return Err(moderated_error(s, &json));
            }
            Some(s @ ("Error" | "Failed")) => {
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
            "BFL generation timed out after {}s. The job was not cancelled and may still finish and be billed by the provider.",
            max_iters as u64 * interval_ms / 1000
        ),
    ))
}

#[cfg(test)]
mod tests {
    use crate::provider::{AuthStyle, ProviderRegistry};

    /// flux_pro/flux_2_pro are registered with AuthStyle::HeaderKey("x-key")
    /// — the precondition call_flux_one/bfl_poll depend on, since both
    /// route auth through ProviderRegistry::apply_auth() rather than
    /// setting the header manually.
    #[test]
    fn flux_pro_and_flux_2_pro_use_x_key_header_auth() {
        let registry = ProviderRegistry::global();
        let flux_pro = registry.get("flux_pro").expect("flux_pro must be registered");
        let flux_2_pro = registry.get("flux_2_pro").expect("flux_2_pro must be registered");
        assert!(matches!(flux_pro.auth_style, AuthStyle::HeaderKey("x-key")));
        assert!(matches!(flux_2_pro.auth_style, AuthStyle::HeaderKey("x-key")));
    }

    /// Confirms apply_auth() actually attaches "x-key" with the real key
    /// value when called with the flux_pro record — not just that the
    /// record is configured correctly.
    #[test]
    fn flux_apply_auth_sends_x_key_header() {
        let record = ProviderRegistry::global().get("flux_pro").unwrap();
        let client = reqwest::Client::new();
        let builder = client.post("https://api.bfl.ai/v1/flux-pro-1.1");
        let req = ProviderRegistry::apply_auth(record, builder, "bfl-test-key")
            .build()
            .unwrap();
        assert_eq!(
            req.headers().get("x-key").map(|v| v.to_str().unwrap()),
            Some("bfl-test-key")
        );
    }
    use super::bfl_poll_loop;
    use crate::nodes::util::poll_test_support::spawn_sequence_mock;

    const READY: &str = r#"{"status":"Ready","result":{"sample":"https://cdn.example/img.png"}}"#;
    const PENDING: &str = r#"{"status":"Pending"}"#;

    async fn run_poll(responses: Vec<(u16, &'static str)>) -> Result<String, crate::error::NodeError> {
        let url = spawn_sequence_mock(responses).await;
        let record = ProviderRegistry::global().get("flux_pro").unwrap();
        bfl_poll_loop(&reqwest::Client::new(), record, &url, "k", 1, 30).await
    }

    #[tokio::test]
    async fn bfl_poll_survives_transient_failures_and_a_good_poll_resets_the_streak() {
        let sample = run_poll(vec![
            (503, "x"), (502, "x"), (200, "<html>"), (503, "x"),
            (200, PENDING),
            (503, "x"), (503, "x"), (503, "x"), (503, "x"),
            (200, READY),
        ])
        .await
        .expect("should reach Ready");
        assert_eq!(sample, "https://cdn.example/img.png");
    }

    #[tokio::test]
    async fn bfl_poll_json_4xx_without_status_fails_immediately() {
        let e = run_poll(vec![(422, r#"{"detail":"bad"}"#)]).await.expect_err("must fail");
        assert_eq!(e.code, "POLL_REJECTED");
        assert!(!e.recoverable);
    }

    #[tokio::test]
    async fn bfl_poll_gives_up_unrecoverable_when_the_server_goes_away() {
        let e = run_poll(vec![(200, PENDING)]).await.expect_err("must give up");
        assert_eq!(e.code, "POLL_FAILED");
        assert!(!e.recoverable);
        assert!(e.message.contains("not resubmitted"));
    }

    use super::{moderated_error, timeout_with_job_ref};
    use crate::error::NodeError;

    const NOT_FOUND: &str = r#"{"status":"Task not found"}"#;

    #[tokio::test]
    async fn bfl_poll_gives_up_after_five_task_not_found_in_a_row() {
        let e = run_poll(vec![(200, NOT_FOUND); 5]).await.expect_err("must fail");
        assert_eq!(e.code, "TASK_NOT_FOUND");
        assert!(!e.recoverable);
        assert!(e.message.contains("not resubmitted"), "{}", e.message);
    }

    #[tokio::test]
    async fn bfl_poll_tolerates_a_brief_task_not_found_and_any_other_status_resets_the_count() {
        let mut replies = vec![(200, NOT_FOUND); 4];
        replies.push((200, PENDING));
        replies.extend(vec![(200, NOT_FOUND); 4]);
        replies.push((200, READY));
        let sample = run_poll(replies).await.expect("should reach Ready");
        assert_eq!(sample, "https://cdn.example/img.png");
    }

    #[test]
    fn a_task_not_found_error_gets_the_job_reference_too() {
        let e = timeout_with_job_ref(NodeError::unrecoverable("TASK_NOT_FOUND", "m"), "job-1", "https://u");
        assert_eq!(e.code, "TASK_NOT_FOUND");
        assert!(e.message.contains("job-1") && e.message.contains("https://u"), "{}", e.message);
    }

    #[tokio::test]
    async fn bfl_poll_moderated_statuses_end_as_a_content_policy_violation() {
        for status in ["Request Moderated", "Content Moderated"] {
            let body: &'static str = Box::leak(
                format!(r#"{{"status":"{status}","details":{{"moderation_reasons":["x"]}}}}"#).into_boxed_str(),
            );
            let e = run_poll(vec![(200, body)]).await.expect_err("must fail");
            assert_eq!(e.code, "CONTENT_POLICY_VIOLATION", "{status}");
            assert!(!e.recoverable);
            assert!(e.message.contains(status) && e.message.contains("moderation_reasons"), "{}", e.message);
        }
    }

    #[test]
    fn moderated_error_without_details_has_no_trailing_detail() {
        let e = moderated_error("Request Moderated", &serde_json::json!({"status": "Request Moderated", "details": {}}));
        assert!(e.message.ends_with("(Request Moderated)"), "{}", e.message);
    }

    #[tokio::test]
    async fn bfl_poll_timeout_says_the_job_may_still_be_billed() {
        let url = spawn_sequence_mock(vec![(200, PENDING), (200, PENDING)]).await;
        let record = ProviderRegistry::global().get("flux_pro").unwrap();
        let e = bfl_poll_loop(&reqwest::Client::new(), record, &url, "k", 1, 2).await.expect_err("must time out");
        assert_eq!(e.code, "TIMEOUT");
        assert!(e.message.contains("may still finish and be billed"), "{}", e.message);
    }

    #[test]
    fn a_timeout_keeps_the_job_id_and_polling_url_and_other_errors_are_untouched() {
        let timeout = NodeError::unrecoverable("TIMEOUT", "BFL generation timed out after 120s.");
        let e = timeout_with_job_ref(timeout, "job-1", "https://api.bfl.ai/v1/get_result?id=job-1");
        assert_eq!(e.code, "TIMEOUT");
        assert!(!e.recoverable);
        assert!(e.message.contains("job-1") && e.message.contains("get_result?id=job-1"), "{}", e.message);

        let other = timeout_with_job_ref(NodeError::unrecoverable("POLL_FAILED", "m"), "job-1", "u");
        assert_eq!(other.message, "m");
    }
}
