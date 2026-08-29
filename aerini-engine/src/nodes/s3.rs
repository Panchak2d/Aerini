use async_trait::async_trait;
use aws_sdk_s3::{
    Client,
    config::{BehaviorVersion, Credentials, Region},
    presigning::PresigningConfig,
    primitives::ByteStream,
};
use base64::Engine as _;
use serde_json::{json, Value};
use std::time::Duration;

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortDefinition, PortPosition};
use crate::nodes::util::{check_host_ssrf_from_url, SsrfPolicy};

/// Hard cap on an S3 object's size when downloading it into memory as base64.
/// Mirrors `http.rs`'s `MAX_RESPONSE_BYTES` — same reasoning: a multi-GB
/// object (or a misconfigured/malicious custom r2/minio endpoint) must not
/// be able to exhaust process memory via a single download.
const MAX_DOWNLOAD_BYTES: usize = 10 * 1024 * 1024;

pub struct S3Node;

#[async_trait]
impl Node for S3Node {
    fn type_id(&self)      -> &'static str { "s3_storage" }
    fn display_name(&self) -> &'static str { "S3 Storage" }
    fn node_type(&self)    -> NodeType     { NodeType::Action }
    fn version(&self)      -> &'static str { "1.0.0" }
    fn description(&self)  -> &'static str { "Upload, download, list, or delete objects in an S3-compatible object store." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["upload", "download", "list", "delete", "presign_url"],
                    "description": "Operation to perform."
                },
                "provider": {
                    "type": "string",
                    "enum": ["aws", "r2", "minio"],
                    "description": "Storage provider. Default: aws."
                },
                "access_key_id": {
                    "type": "string",
                    "description": "Access key ID."
                },
                "secret_access_key": {
                    "type": "string",
                    "description": "Secret access key."
                },
                "region": {
                    "type": "string",
                    "description": "AWS region (e.g. us-east-1). Default: us-east-1. For R2, use 'auto'. Ignored for MinIO — use endpoint."
                },
                "endpoint": {
                    "type": "string",
                    "description": "Custom endpoint URL. Required for r2 (https://ACCOUNT_ID.r2.cloudflarestorage.com) and minio (http://host:9000). Unused for aws."
                },
                "bucket": {
                    "type": "string",
                    "description": "Bucket name."
                },
                "key": {
                    "type": "string",
                    "description": "Object key (path). Required for upload, download, delete, presign_url."
                },
                "content": {
                    "type": "string",
                    "description": "Content to upload. Text or base64 bytes depending on content_encoding."
                },
                "content_encoding": {
                    "type": "string",
                    "enum": ["text", "base64"],
                    "description": "Encoding of content field. Default: text."
                },
                "content_type": {
                    "type": "string",
                    "description": "MIME type for upload. Default: application/octet-stream."
                },
                "prefix": {
                    "type": "string",
                    "description": "Key prefix filter for list. Empty string lists all objects."
                },
                "expiry_secs": {
                    "type": "number",
                    "description": "Presigned URL expiry in seconds. Default: 3600. Maximum: 604800 (7 days)."
                }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "key":              { "type": "string",  "description": "Object key." },
                "size":             { "type": "number",  "description": "Object size in bytes." },
                "content_type":     { "type": "string",  "description": "MIME type (upload only)." },
                "content":          { "type": "string",  "description": "Base64-encoded object bytes (download only)." },
                "content_encoding": { "type": "string",  "description": "Always 'base64' for download output." },
                "objects":          { "type": "array",   "description": "List of objects: [{key, size, last_modified}] (list only)." },
                "prefix":           { "type": "string",  "description": "Prefix used for the list query." },
                "count":            { "type": "number",  "description": "Number of objects returned (list only)." },
                "deleted":          { "type": "boolean", "description": "True on successful delete." },
                "url":              { "type": "string",  "description": "Presigned URL (presign_url only)." },
                "expires_in":       { "type": "number",  "description": "Presigned URL TTL in seconds." }
            }
        })
    }

    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![PortDefinition {
                id: "input".to_string(), label: "In".to_string(), position: PortPosition::Left, port_type: None,
            }],
            outputs: vec![
                PortDefinition { id: "output".to_string(),   label: "Out".to_string(),   position: PortPosition::Right, port_type: None },
                PortDefinition { id: "on_error".to_string(), label: "Error".to_string(), position: PortPosition::Right, port_type: None },
            ],
        }
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let operation = match input.input["operation"].as_str().filter(|s| !s.is_empty()) {
            Some(op) => op.to_string(),
            None => return NodeOutput::failure(NodeError::unrecoverable(
                "MISSING_OPERATION",
                "operation is required. One of: upload, download, list, delete, presign_url",
            )),
        };

        let (client, bucket) = match build_client(&input.input).await {
            Ok(b)  => b,
            Err(e) => return NodeOutput::failure(NodeError::unrecoverable("CONFIG_ERROR", e)),
        };

        match operation.as_str() {
            "upload"      => op_upload(&client, &bucket, &input.input).await,
            "download"    => op_download(&client, &bucket, &input.input).await,
            "list"        => op_list(&client, &bucket, &input.input).await,
            "delete"      => op_delete(&client, &bucket, &input.input).await,
            "presign_url" => op_presign(&client, &bucket, &input.input).await,
            other => NodeOutput::failure(NodeError::unrecoverable(
                "UNKNOWN_OPERATION",
                format!("Unknown operation '{}'. Valid: upload, download, list, delete, presign_url", other),
            )),
        }
    }
}

// ── Client construction ───────────────────────────────────────────────────────

/// Builds an aws-sdk-s3 Client from node config and returns it with the
/// bucket name. R2 and MinIO use path-style addressing and require a custom
/// endpoint; AWS uses the SDK's default virtual-hosted-style endpoints.
///
/// SSRF protection: custom endpoints (R2, MinIO) are validated with
/// `check_host_ssrf_from_url` before the client is constructed. MinIO's
/// primary real-world deployment is self-hosted on localhost/LAN, so it is
/// checked under `SsrfPolicy::AllowLocal` — same precedent already
/// established by `image_gen/a1111.rs`/`image_gen/comfyui.rs`. R2 is
/// always a remote Cloudflare-hosted endpoint and stays `SsrfPolicy::Strict`.
/// AWS endpoints are derived from the region string by the SDK — no
/// user-controlled URL is used — so no SSRF check is needed for the `aws`
/// provider.
async fn build_client(cfg: &Value) -> Result<(Client, String), String> {
    let access_key = cfg["access_key_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("access_key_id is required")?;

    let secret_key = cfg["secret_access_key"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("secret_access_key is required")?;

    let bucket_name = cfg["bucket"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("bucket is required")?
        .to_string();

    let region_str = cfg["region"].as_str().filter(|s| !s.is_empty()).unwrap_or("us-east-1");
    let provider   = cfg["provider"].as_str().unwrap_or("aws");

    let creds = Credentials::new(access_key, secret_key, None, None, "static");

    let mut config_builder = aws_sdk_s3::config::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .credentials_provider(creds)
        .region(Region::new(region_str.to_string()));

    if matches!(provider, "r2" | "minio") {
        let endpoint = cfg["endpoint"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!(
                "endpoint is required for {}. Example: {}",
                provider,
                if provider == "r2" { "https://ACCOUNT_ID.r2.cloudflarestorage.com" } else { "http://host:9000" }
            ))?;

        // MinIO is a local/self-hosted-only provider in practice (its default
        // deployment is http://localhost:9000 or a LAN address) — same
        // rationale image_gen/a1111.rs and image_gen/comfyui.rs already use
        // for their own local-only base_url checks. R2 is always a remote
        // Cloudflare-hosted endpoint, so it keeps the strict check.
        let ssrf_policy = if provider == "minio" { SsrfPolicy::AllowLocal } else { SsrfPolicy::Strict };
        check_host_ssrf_from_url(endpoint, ssrf_policy).await
            .map_err(|e| format!("SSRF check failed for {} endpoint: {}", provider, e))?;

        config_builder = config_builder
            .endpoint_url(endpoint)
            .force_path_style(true);
    }

    let client = Client::from_conf(config_builder.build());
    Ok((client, bucket_name))
}

// ── Operations ────────────────────────────────────────────────────────────────

async fn op_upload(client: &Client, bucket: &str, cfg: &Value) -> NodeOutput {
    let key = match cfg["key"].as_str().filter(|s| !s.is_empty()) {
        Some(k) => k.to_string(),
        None    => return NodeOutput::failure(NodeError::unrecoverable("MISSING_KEY", "key is required for upload")),
    };

    let content_raw = match cfg["content"].as_str() {
        Some(c) => c.to_string(),
        None    => return NodeOutput::failure(NodeError::unrecoverable("MISSING_CONTENT", "content is required for upload")),
    };

    let content_encoding = cfg["content_encoding"].as_str().unwrap_or("text");
    let content_type     = cfg["content_type"].as_str().unwrap_or("application/octet-stream").to_string();

    let bytes: Vec<u8> = if content_encoding == "base64" {
        match base64::engine::general_purpose::STANDARD.decode(&content_raw) {
            Ok(b)  => b,
            Err(e) => return NodeOutput::failure(NodeError::unrecoverable(
                "DECODE_ERROR",
                format!("Failed to base64-decode content: {}", e),
            )),
        }
    } else {
        content_raw.into_bytes()
    };

    let size = bytes.len();
    match client
        .put_object()
        .bucket(bucket)
        .key(&key)
        .body(ByteStream::from(bytes))
        .content_type(&content_type)
        .send()
        .await
    {
        Err(e) => NodeOutput::failure(NodeError::unrecoverable("UPLOAD_ERROR", format!("Upload failed: {}", e))),
        Ok(_)  => NodeOutput::success_with_logs(
            json!({ "key": key, "size": size, "content_type": content_type }),
            vec![format!("Uploaded {} bytes to '{}'", size, key)],
        ),
    }
}

async fn op_download(client: &Client, bucket: &str, cfg: &Value) -> NodeOutput {
    let key = match cfg["key"].as_str().filter(|s| !s.is_empty()) {
        Some(k) => k.to_string(),
        None    => return NodeOutput::failure(NodeError::unrecoverable("MISSING_KEY", "key is required for download")),
    };

    let mut output = match client.get_object().bucket(bucket).key(&key).send().await {
        Err(e) => return NodeOutput::failure(NodeError::unrecoverable(
            "DOWNLOAD_ERROR",
            format!("Download failed for key '{}': {}", key, e),
        )),
        Ok(o)  => o,
    };

    // Reject upfront if the declared size already exceeds the cap — avoids
    // reading anything for an object we know we're going to refuse.
    if let Some(cl) = output.content_length() {
        if cl > 0 && cl as u64 > MAX_DOWNLOAD_BYTES as u64 {
            return NodeOutput::failure(NodeError::unrecoverable(
                "DOWNLOAD_TOO_LARGE",
                format!(
                    "Object '{}' is {} bytes, exceeds {} MB download limit",
                    key, cl, MAX_DOWNLOAD_BYTES / (1024 * 1024)
                ),
            ));
        }
    }

    // Stream chunk-by-chunk instead of `.collect()`'s unbounded single-shot
    // buffer — hard-caps memory regardless of what content_length
    // declared (or omitted), same two-stage guard as http.rs's response
    // reader / nodes/util.rs's read_json_response_capped.
    let capacity = output
        .content_length()
        .filter(|&cl| cl > 0)
        .unwrap_or(0)
        .min(MAX_DOWNLOAD_BYTES as i64) as usize;
    let mut bytes: Vec<u8> = Vec::with_capacity(capacity);
    loop {
        match output.body.next().await {
            Some(Ok(chunk)) => {
                if bytes.len() + chunk.len() > MAX_DOWNLOAD_BYTES {
                    return NodeOutput::failure(NodeError::unrecoverable(
                        "DOWNLOAD_TOO_LARGE",
                        format!(
                            "Object '{}' exceeds {} MB download limit",
                            key, MAX_DOWNLOAD_BYTES / (1024 * 1024)
                        ),
                    ));
                }
                bytes.extend_from_slice(&chunk);
            }
            Some(Err(e)) => return NodeOutput::failure(NodeError::unrecoverable(
                "DOWNLOAD_ERROR",
                format!("Failed to read response body for key '{}': {}", key, e),
            )),
            None => break,
        }
    }

    let size    = bytes.len();
    let content = base64::engine::general_purpose::STANDARD.encode(&bytes);
    NodeOutput::success_with_logs(
        json!({
            "key":              key,
            "content":          content,
            "content_encoding": "base64",
            "size":             size
        }),
        vec![format!("Downloaded {} bytes from '{}'", size, key)],
    )
}

async fn op_list(client: &Client, bucket: &str, cfg: &Value) -> NodeOutput {
    let prefix = cfg["prefix"].as_str().unwrap_or("").to_string();

    match client
        .list_objects_v2()
        .bucket(bucket)
        .prefix(&prefix)
        .send()
        .await
    {
        Err(e) => NodeOutput::failure(NodeError::unrecoverable("LIST_ERROR", format!("List failed: {}", e))),
        Ok(output) => {
            let objects: Vec<Value> = output
                .contents()
                .iter()
                .map(|obj| {
                    let last_modified = obj
                        .last_modified()
                        .and_then(|d| d.to_millis().ok())
                        .map(|ms| {
                            let secs = ms / 1000;
                            let nanos = ((ms % 1000).unsigned_abs() as u32) * 1_000_000;
                            chrono::DateTime::from_timestamp(secs, nanos)
                                .map(|dt: chrono::DateTime<chrono::Utc>| dt.to_rfc3339())
                                .unwrap_or_default()
                        })
                        .unwrap_or_default();
                    json!({
                        "key":           obj.key().unwrap_or_default(),
                        "size":          obj.size().unwrap_or_default(),
                        "last_modified": last_modified,
                    })
                })
                .collect();
            let count = objects.len();
            NodeOutput::success_with_logs(
                json!({ "objects": objects, "prefix": prefix, "count": count }),
                vec![format!("Listed {} object(s) with prefix '{}'", count, prefix)],
            )
        }
    }
}

async fn op_delete(client: &Client, bucket: &str, cfg: &Value) -> NodeOutput {
    let key = match cfg["key"].as_str().filter(|s| !s.is_empty()) {
        Some(k) => k.to_string(),
        None    => return NodeOutput::failure(NodeError::unrecoverable("MISSING_KEY", "key is required for delete")),
    };

    match client.delete_object().bucket(bucket).key(&key).send().await {
        Err(e) => NodeOutput::failure(NodeError::unrecoverable(
            "DELETE_ERROR",
            format!("Delete failed for key '{}': {}", key, e),
        )),
        Ok(_) => NodeOutput::success_with_logs(
            json!({ "key": key, "deleted": true }),
            vec![format!("Deleted '{}'", key)],
        ),
    }
}

async fn op_presign(client: &Client, bucket: &str, cfg: &Value) -> NodeOutput {
    let key = match cfg["key"].as_str().filter(|s| !s.is_empty()) {
        Some(k) => k.to_string(),
        None    => return NodeOutput::failure(NodeError::unrecoverable(
            "MISSING_KEY",
            "key is required for presign_url",
        )),
    };

    // AWS S3 presigned URL maximum is 7 days = 604 800 seconds.
    let expiry_secs: u64 = cfg["expiry_secs"]
        .as_u64()
        .or_else(|| cfg["expiry_secs"].as_f64().map(|f| f.max(1.0) as u64))
        .unwrap_or(3600)
        .min(604_800);

    let presigning_config = match PresigningConfig::expires_in(Duration::from_secs(expiry_secs)) {
        Ok(c)  => c,
        Err(e) => return NodeOutput::failure(NodeError::unrecoverable(
            "PRESIGN_ERROR",
            format!("Invalid presign expiry: {}", e),
        )),
    };

    match client
        .get_object()
        .bucket(bucket)
        .key(&key)
        .presigned(presigning_config)
        .await
    {
        Err(e) => NodeOutput::failure(NodeError::unrecoverable(
            "PRESIGN_ERROR",
            format!("Could not generate presigned URL for '{}': {}", key, e),
        )),
        Ok(presigned) => NodeOutput::success_with_logs(
            json!({ "url": presigned.uri().to_string(), "key": key, "expires_in": expiry_secs }),
            vec![format!("Generated presigned URL for '{}' (expires in {}s)", key, expiry_secs)],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::build_client;
    use serde_json::{json, Value};

    fn cfg_for(provider: &str, endpoint: &str) -> Value {
        json!({
            "provider":          provider,
            "access_key_id":     "test-access-key",
            "secret_access_key": "test-secret-key",
            "bucket":            "test-bucket",
            "endpoint":          endpoint
        })
    }

    // MinIO's primary real-world deployment is self-hosted on
    // localhost/LAN. Under SsrfPolicy::Strict, a loopback endpoint is
    // unconditionally rejected; the AllowLocal escape hatch must actually
    // open for it — `build_client` never touches the network itself (client
    // construction only), so a successful `Ok` here proves the SSRF check
    // permits it, not a live server.
    #[tokio::test]
    async fn minio_endpoint_allows_loopback() {
        let cfg = cfg_for("minio", "http://127.0.0.1:9000");
        let result = build_client(&cfg).await;
        assert!(
            result.is_ok(),
            "minio loopback endpoint must be allowed under SsrfPolicy::AllowLocal, got: {:?}",
            result.err()
        );
    }

    // R2 is always a remote Cloudflare-hosted endpoint — the AllowLocal
    // escape hatch is minio-only. A loopback R2 endpoint (never a
    // legitimate R2 URL) must stay blocked for both providers.
    #[tokio::test]
    async fn r2_endpoint_still_rejects_loopback() {
        let cfg = cfg_for("r2", "http://127.0.0.1:9000");
        let result = build_client(&cfg).await;
        assert!(result.is_err(), "r2 loopback endpoint must still be rejected under SsrfPolicy::Strict");
        assert!(
            result.unwrap_err().contains("SSRF check failed"),
            "rejection must come from the SSRF check, not some other config error"
        );
    }

    // AllowLocal widens only the loopback/RFC1918-private block — it must
    // NOT widen the always-blocked list (link-local / cloud metadata / etc,
    // per check_ssrf_ip_impl). 169.254.169.254 is the AWS/GCP/Azure IMDS
    // range; this must stay blocked even for minio.
    #[tokio::test]
    async fn minio_endpoint_still_rejects_link_local_metadata_ip() {
        let cfg = cfg_for("minio", "http://169.254.169.254:9000");
        let result = build_client(&cfg).await;
        assert!(result.is_err(), "minio must not bypass the always-blocked link-local/metadata range");
        assert!(
            result.unwrap_err().contains("SSRF check failed"),
            "rejection must come from the SSRF check, not some other config error"
        );
    }
}
