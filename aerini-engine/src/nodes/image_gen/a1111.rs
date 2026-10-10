use chrono::Utc;
use serde_json::{json, Value};
use url::Url;

use crate::error::NodeError;
use crate::model::NodeOutput;
use crate::nodes::util::{check_host_ssrf_from_url, read_json_response_capped, SsrfPolicy};

use super::shared::{network_err, strip_data_uri_prefix};

// -- Automatic1111 / Stable Diffusion WebUI -----------------------------------
//
// Single POST call with batch_size=n (native batch).
// Optional HTTP Basic auth: URL-embedded credentials (user:pass@host) OR
// separate username/password config fields (explicit fields take priority).
// Response images may carry a data: URI prefix -- stripped via strip_data_uri_prefix.
// With batch_size > 1 and the return_grid option on (the default), the response
// lists the grid image first; info.index_of_first_image says where the real
// images start, and the entries before it are skipped.
// timeout_secs overrides the shared_ai_client()'s 120s default for this request
// only (RequestBuilder::timeout supersedes ClientBuilder::timeout, reqwest VERIFIED
// behavior) -- local batch generation can legitimately exceed 120s.

pub(super) struct A1111Params {
    pub(super) base_url:        String,
    pub(super) prompt:          String,
    pub(super) negative_prompt: String,
    pub(super) n:               usize,
    pub(super) width:           u32,
    pub(super) height:          u32,
    pub(super) steps:           u32,
    pub(super) cfg_scale:       f64,
    pub(super) username:        Option<String>,
    pub(super) password:        Option<String>,
    pub(super) timeout_secs:    u64,
}

fn first_real_image_index(json: &Value, images_len: usize) -> usize {
    let info = match &json["info"] {
        Value::String(text) => serde_json::from_str::<Value>(text).unwrap_or(Value::Null),
        other => other.clone(),
    };
    match info["index_of_first_image"].as_u64() {
        Some(i) if (i as usize) < images_len => i as usize,
        _ => 0,
    }
}

pub(super) async fn gen_a1111(client: reqwest::Client, p: A1111Params) -> NodeOutput {
    // a1111 is a local-only provider -- base_url (e.g. 127.0.0.1:7860) is the
    // user's explicit trust signal for this node, so loopback/private/localhost
    // are permitted here. Link-local, cloud metadata, and RFC 6598 stay blocked
    // (SsrfPolicy::AllowLocal in util.rs). Cloud providers in this file keep
    // the strict (SsrfPolicy::Strict) check_host_ssrf_from_url unchanged.
    if let Err(e) = check_host_ssrf_from_url(&p.base_url, SsrfPolicy::AllowLocal).await {
        return NodeOutput::failure(NodeError::unrecoverable("SSRF_BLOCKED", e));
    }

    let parsed = match Url::parse(&p.base_url) {
        Ok(u)  => u,
        Err(e) => return NodeOutput::failure(NodeError::unrecoverable(
            "INVALID_BASE_URL",
            format!("Invalid base_url: {}", e),
        )),
    };

    // Extract URL-embedded credentials; explicit config fields take priority.
    let url_user = parsed.username().to_string();
    let url_pass = parsed.password().map(|s| s.to_string());

    let auth_user = p.username.filter(|s| !s.is_empty())
        .or(if url_user.is_empty() { None } else { Some(url_user) });
    let auth_pass = p.password.filter(|s| !s.is_empty()).or(url_pass);

    // Build endpoint URL without embedded credentials.
    let mut clean = parsed.clone();
    clean.set_username("").ok();
    clean.set_password(None).ok();
    let endpoint = format!("{}/sdapi/v1/txt2img", clean.as_str().trim_end_matches('/'));

    let ts   = Utc::now().timestamp_millis();
    let body = json!({
        "prompt":          p.prompt,
        "negative_prompt": p.negative_prompt,
        "width":           p.width,
        "height":          p.height,
        "steps":           p.steps,
        "cfg_scale":       p.cfg_scale,
        "batch_size":      p.n
    });

    let mut req = client.post(&endpoint)
        .json(&body)
        .timeout(std::time::Duration::from_secs(p.timeout_secs));
    if let Some(user) = auth_user {
        req = req.basic_auth(user, auth_pass);
    }

    let resp = match req.send().await {
        Ok(r)  => r,
        Err(e) => return NodeOutput::failure(network_err(e)),
    };

    let status = resp.status().as_u16();
    let json: Value = match read_json_response_capped(resp).await {
        Ok(v)  => v,
        Err(e) => return NodeOutput::failure(crate::nodes::util::provider_error(status, "PARSE_ERROR", e)),
    };

    if status >= 400 {
        let detail = json["detail"].as_str()
            .or_else(|| json["error"].as_str())
            .unwrap_or("Unknown error")
            .to_string();
        return NodeOutput::failure(match status {
            401 | 403 => NodeError::unrecoverable(
                "AUTH_FAILED",
                "A1111 authentication failed. Check username/password or --api-auth on the server.",
            ),
            422 => NodeError::unrecoverable("VALIDATION_ERROR", format!("A1111 validation error: {}", detail)),
            _   => NodeError::unrecoverable("API_ERROR", format!("A1111 error {}: {}", status, detail)),
        });
    }

    let images = match json["images"].as_array() {
        Some(arr) if !arr.is_empty() => arr,
        _ => return NodeOutput::failure(NodeError::unrecoverable(
            "EMPTY_RESPONSE",
            "A1111 returned no images",
        )),
    };

    let mut files: Vec<Value>  = Vec::new();
    let mut logs:  Vec<String> = Vec::new();

    let first = first_real_image_index(&json, images.len());
    if first > 0 {
        logs.push(format!("A1111 grid image skipped ({} entries before the first real image)", first));
    }
    let images = &images[first..];

    for (i, item) in images.iter().enumerate() {
        match item.as_str() {
            Some(b64) => {
                let clean_b64 = strip_data_uri_prefix(b64);
                if clean_b64.is_empty() {
                    logs.push(format!("A1111 image {} empty -- skipped", i));
                    continue;
                }
                let filename = format!("a1111_{}_{}.png", ts, i);
                files.push(json!({
                    "filename":  filename,
                    "data":      clean_b64,
                    "mime_type": "image/png"
                }));
                logs.push(format!("A1111 image {}/{} generated", i + 1, images.len()));
            }
            None => logs.push(format!("A1111 image {} not a string -- skipped", i)),
        }
    }

    if files.is_empty() {
        return NodeOutput::failure_with_logs(
            NodeError::unrecoverable("EMPTY_RESPONSE", "A1111 returned no valid base64 images"),
            logs,
        );
    }

    NodeOutput::success_with_logs(
        json!({ "files": files, "count": files.len(), "source": "a1111" }),
        logs,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_entries_before_index_of_first_image_are_skipped() {
        let json = json!({ "images": ["grid", "a", "b"], "info": "{\"index_of_first_image\": 1}" });
        assert_eq!(first_real_image_index(&json, 3), 1);
        let as_object = json!({ "images": ["grid", "a"], "info": { "index_of_first_image": 1 } });
        assert_eq!(first_real_image_index(&as_object, 2), 1);
    }

    #[test]
    fn missing_unreadable_or_out_of_range_info_skips_nothing() {
        for info in [json!(null), json!("not json"), json!("{}"), json!("{\"index_of_first_image\": 0}"), json!("{\"index_of_first_image\": 5}")] {
            let json = json!({ "images": ["a", "b"], "info": info });
            assert_eq!(first_real_image_index(&json, 2), 0, "{}", json["info"]);
        }
    }
}
