use async_trait::async_trait;
use aws_sdk_s3::{
    Client,
    config::{BehaviorVersion, Credentials, Region},
    error::{ProvideErrorMetadata, SdkError},
    presigning::PresigningConfig,
    primitives::ByteStream,
};
use aws_smithy_http_client::{tls, Builder as HttpClientBuilder};
use aws_smithy_runtime_api::client::dns::{DnsFuture, ResolveDns, ResolveDnsError};
use aws_smithy_runtime_api::client::orchestrator::HttpResponse;
use base64::Engine as _;
use serde_json::{json, Value};
use std::time::Duration;

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortArity, PortDefinition, PortPosition};
use crate::nodes::util::{
    cfg_f64_opt, cfg_u64_opt, check_host_ssrf_from_url, decode_file_data, resolve_validated_addrs,
    SsrfPolicy,
};

/// Hard cap on an S3 object's size when downloading it into memory as base64.
/// Mirrors `http.rs`'s `MAX_RESPONSE_BYTES` — same reasoning: a multi-GB
/// object (or a misconfigured/malicious custom r2/minio endpoint) must not
/// be able to exhaust process memory via a single download.
const MAX_DOWNLOAD_BYTES: usize = 10 * 1024 * 1024;

/// Most entries (objects plus common prefixes) one `list` returns, and the
/// default for `max_objects`. S3 serves at most 1,000 keys per request, so
/// this is ten requests; the output stays in the low megabytes for typical key
/// lengths.
const MAX_LIST_OBJECTS: usize = 10_000;

/// Largest object one presigned PUT may declare: S3's single-PUT limit (5 GiB).
const MAX_PRESIGN_PUT_BYTES: u64 = 5 * 1024 * 1024 * 1024;

/// Longest service or transport error text put in a failure message.
const MAX_ERROR_TEXT_CHARS: usize = 500;

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
                    "description": "Custom endpoint URL. Required for r2 (https://ACCOUNT_ID.r2.cloudflarestorage.com) and minio (http://host:9000). Must be empty for aws."
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
                    "description": "MIME type for upload (default application/octet-stream). With presign_url and PUT, signed into the URL: the uploader must send the same Content-Type. Not allowed with GET."
                },
                "prefix": {
                    "type": "string",
                    "description": "Key prefix filter for list. Empty string lists all objects."
                },
                "max_objects": {
                    "type": "number",
                    "description": "Most entries a list returns, 1 to 10000. Default: 10000."
                },
                "delimiter": {
                    "type": "string",
                    "description": "List only: group keys that share the text between the prefix and the first delimiter (usually '/') into common_prefixes, like folders. Empty lists every key."
                },
                "content_length": {
                    "type": "number",
                    "description": "presign_url with PUT: exact size in bytes the upload must have, up to 5 GiB. Signed into the URL; the uploader must send the same Content-Length."
                },
                "method": {
                    "type": "string",
                    "enum": ["GET", "PUT"],
                    "description": "HTTP method the presigned URL is valid for: GET downloads the object, PUT uploads it. Default: GET."
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
                "common_prefixes": { "type": "array",   "description": "Prefixes that group keys at the delimiter, e.g. ['photos/2026/'] (list only; empty without a delimiter)." },
                "truncated":        { "type": "boolean", "description": "True when the bucket holds more matching entries than were returned (list only)." },
                "max_objects":      { "type": "number",  "description": "Most entries this list could return, from the max_objects setting (list only)." },
                "prefix":           { "type": "string",  "description": "Prefix used for the list query." },
                "count":            { "type": "number",  "description": "Number of objects returned (list only)." },
                "deleted":          { "type": "boolean", "description": "True on successful delete." },
                "url":              { "type": "string",  "description": "Presigned URL (presign_url only)." },
                "method":           { "type": "string",  "description": "HTTP method the presigned URL is valid for (presign_url only)." },
                "expires_in":       { "type": "number",  "description": "Presigned URL TTL in seconds." },
                "headers":          { "type": "object",  "description": "Headers the caller must send with the presigned request, name to value; empty when none (presign_url only)." }
            }
        })
    }

    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![PortDefinition {
                id: "input".to_string(), label: "In".to_string(), position: PortPosition::Left, port_type: None,
                arity: PortArity::Single,
            }],
            outputs: vec![
                PortDefinition { id: "output".to_string(),   label: "Out".to_string(),   position: PortPosition::Right, port_type: None, arity: PortArity::Single },
                PortDefinition { id: "on_error".to_string(), label: "Error".to_string(), position: PortPosition::Right, port_type: None, arity: PortArity::Single },
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

        if !OPERATIONS.contains(&operation.as_str()) {
            return NodeOutput::failure(unknown_operation(&operation));
        }

        let (client, bucket) = match build_client(&input.input).await {
            Ok(b)  => b,
            Err(e) => return NodeOutput::failure(e),
        };

        match operation.as_str() {
            "upload"      => op_upload(&client, &bucket, &input.input).await,
            "download"    => op_download(&client, &bucket, &input.input).await,
            "list"        => op_list(&client, &bucket, &input.input).await,
            "delete"      => op_delete(&client, &bucket, &input.input).await,
            "presign_url" => op_presign(&client, &bucket, &input.input).await,
            other => NodeOutput::failure(unknown_operation(other)),
        }
    }
}

const OPERATIONS: [&str; 5] = ["upload", "download", "list", "delete", "presign_url"];

fn unknown_operation(operation: &str) -> NodeError {
    NodeError::unrecoverable(
        "UNKNOWN_OPERATION",
        format!("Unknown operation '{}'. Valid: {}", operation, OPERATIONS.join(", ")),
    )
}

// ── Client construction ───────────────────────────────────────────────────────

/// DNS resolver for custom-endpoint clients: the SDK dials only addresses
/// that pass the SSRF policy. TLS still verifies the endpoint's host name,
/// since only the address lookup is replaced.
#[derive(Clone, Debug)]
struct SsrfDnsResolver(SsrfPolicy);

impl ResolveDns for SsrfDnsResolver {
    fn resolve_dns<'a>(&'a self, name: &'a str) -> DnsFuture<'a> {
        let policy = self.0;
        DnsFuture::new(async move {
            resolve_validated_addrs(name, 0, policy)
                .await
                .map(|addrs| addrs.into_iter().map(|a| a.ip()).collect())
                .map_err(ResolveDnsError::new)
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Provider {
    Aws,
    R2,
    Minio,
}

impl Provider {
    fn as_str(self) -> &'static str {
        match self {
            Provider::Aws   => "aws",
            Provider::R2    => "r2",
            Provider::Minio => "minio",
        }
    }
}

/// An unset or blank `provider` means `aws`; anything else must be a provider
/// name, in any letter case.
fn parse_provider(cfg: &Value) -> Result<Provider, NodeError> {
    let unknown = |shown: String| NodeError::unrecoverable(
        "UNKNOWN_PROVIDER",
        format!("Unknown provider '{}'. Valid: aws, r2, minio", shown.chars().take(64).collect::<String>()),
    );
    match &cfg["provider"] {
        Value::Null => Ok(Provider::Aws),
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "" | "aws" => Ok(Provider::Aws),
            "r2"       => Ok(Provider::R2),
            "minio"    => Ok(Provider::Minio),
            _          => Err(unknown(s.clone())),
        },
        other => Err(unknown(other.to_string())),
    }
}

/// Builds an aws-sdk-s3 Client from node config and returns it with the
/// bucket name. R2 and MinIO use path-style addressing and require a custom
/// endpoint; AWS uses the SDK's default virtual-hosted-style endpoints and
/// refuses a configured endpoint rather than ignoring it.
///
/// SSRF protection: custom endpoints (R2, MinIO) are validated with
/// `check_host_ssrf_from_url` before the client is constructed. MinIO's
/// primary real-world deployment is self-hosted on localhost/LAN, so it is
/// checked under `SsrfPolicy::AllowLocal` — same precedent already
/// established by `image_gen/a1111.rs`/`image_gen/comfyui.rs`. R2 is
/// always a remote Cloudflare-hosted endpoint and stays `SsrfPolicy::Strict`.
/// The same policy is enforced again at connect time by [`SsrfDnsResolver`],
/// so a name that resolves to a blocked address after the early check is
/// refused. AWS endpoints are derived from the region string by the SDK — no
/// user-controlled URL is used — so no SSRF check is needed for the `aws`
/// provider.
async fn build_client(cfg: &Value) -> Result<(Client, String), NodeError> {
    let config_error = |message: String| NodeError::unrecoverable("CONFIG_ERROR", message);

    let provider = parse_provider(cfg)?;

    let access_key = cfg["access_key_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| config_error("access_key_id is required".to_string()))?;

    let secret_key = cfg["secret_access_key"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| config_error("secret_access_key is required".to_string()))?;

    let bucket_name = cfg["bucket"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| config_error("bucket is required".to_string()))?
        .to_string();

    let region_str = cfg["region"].as_str().filter(|s| !s.is_empty()).unwrap_or("us-east-1");

    let creds = Credentials::new(access_key, secret_key, None, None, "static");

    let mut config_builder = aws_sdk_s3::config::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .credentials_provider(creds)
        .region(Region::new(region_str.to_string()));

    if provider == Provider::Aws {
        let endpoint_set = match &cfg["endpoint"] {
            Value::Null      => false,
            Value::String(s) => !s.trim().is_empty(),
            _                => true,
        };
        if endpoint_set {
            return Err(NodeError::unrecoverable(
                "ENDPOINT_NOT_SUPPORTED",
                "endpoint is not used with provider 'aws' (the AWS endpoint comes from region). \
                 Clear endpoint, or set provider to r2 or minio.",
            ));
        }
    } else {
        let name = provider.as_str();
        let endpoint = cfg["endpoint"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| config_error(format!(
                "endpoint is required for {}. Example: {}",
                name,
                if provider == Provider::R2 { "https://ACCOUNT_ID.r2.cloudflarestorage.com" } else { "http://host:9000" }
            )))?;

        // MinIO is a local/self-hosted-only provider in practice (its default
        // deployment is http://localhost:9000 or a LAN address) — same
        // rationale image_gen/a1111.rs and image_gen/comfyui.rs already use
        // for their own local-only base_url checks. R2 is always a remote
        // Cloudflare-hosted endpoint, so it keeps the strict check.
        let ssrf_policy = if provider == Provider::Minio { SsrfPolicy::AllowLocal } else { SsrfPolicy::Strict };
        check_host_ssrf_from_url(endpoint, ssrf_policy).await
            .map_err(|e| config_error(format!("SSRF check failed for {} endpoint: {}", name, e)))?;

        let http_client = HttpClientBuilder::new()
            .tls_provider(tls::Provider::Rustls(tls::rustls_provider::CryptoMode::AwsLc))
            .build_with_resolver(SsrfDnsResolver(ssrf_policy));

        config_builder = config_builder
            .endpoint_url(endpoint)
            .force_path_style(true)
            .http_client(http_client);
    }

    let client = Client::from_conf(config_builder.build());
    Ok((client, bucket_name))
}

/// Failure text for an S3 call: the service's error code and message
/// (`AccessDenied: Access Denied`), which the SDK's own `Display` leaves out.
/// Without a service code (connection refused, timeout, empty error body) it
/// is the SDK label plus its source chain and, when there was a response, the
/// HTTP status. Headers and bodies are never included.
fn sdk_error_text<E>(err: &SdkError<E, HttpResponse>) -> String
where
    E: ProvideErrorMetadata + std::error::Error + 'static,
{
    let meta = err.meta();
    let text = match (meta.code(), meta.message()) {
        (Some(code), Some(message)) => format!("{code}: {message}"),
        (Some(code), None)          => code.to_string(),
        (None, Some(message))       => message.to_string(),
        (None, None) => {
            let mut text = error_chain_text(err);
            if let Some(response) = err.raw_response() {
                text.push_str(&format!(" (HTTP {})", response.status().as_u16()));
            }
            text
        }
    };
    cap_error_text(text)
}

/// An error's own text followed by each underlying cause, `outer: inner`.
/// Transport errors keep their useful detail (connection reset, timeout) in
/// the sources; their own `Display` is a generic label.
fn error_chain_text(err: &(dyn std::error::Error + 'static)) -> String {
    let mut text = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

fn cap_error_text(text: String) -> String {
    if text.chars().count() > MAX_ERROR_TEXT_CHARS {
        text.chars().take(MAX_ERROR_TEXT_CHARS).collect::<String>() + "…"
    } else {
        text
    }
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

    let content_type = cfg["content_type"].as_str().unwrap_or("application/octet-stream").to_string();

    let unknown_encoding = |shown: String| NodeError::unrecoverable(
        "UNKNOWN_CONTENT_ENCODING",
        format!("Unknown content_encoding '{}'. Valid: text, base64", shown.chars().take(64).collect::<String>()),
    );
    let is_base64 = match &cfg["content_encoding"] {
        Value::Null => false,
        Value::String(e) => match e.trim().to_ascii_lowercase().as_str() {
            "" | "text" => false,
            "base64"    => true,
            _           => return NodeOutput::failure(unknown_encoding(e.clone())),
        },
        other => return NodeOutput::failure(unknown_encoding(other.to_string())),
    };
    let bytes: Vec<u8> = if is_base64 {
        match decode_file_data(&content_raw) {
            Ok(b)  => b,
            Err(e) => return NodeOutput::failure(NodeError::unrecoverable(
                "DECODE_ERROR",
                format!("Failed to decode base64 content: {}", e),
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
        Err(e) => NodeOutput::failure(NodeError::unrecoverable("UPLOAD_ERROR", format!("Upload failed: {}", sdk_error_text(&e)))),
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
            format!("Download failed for key '{}': {}", key, sdk_error_text(&e)),
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
                format!(
                    "Failed to read response body for key '{}': {}",
                    key,
                    cap_error_text(error_chain_text(&e))
                ),
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
    let cap = match parse_max_objects(&cfg["max_objects"]) {
        Ok(c)  => c,
        Err(e) => return NodeOutput::failure(e),
    };
    let delimiter = match parse_delimiter(&cfg["delimiter"]) {
        Ok(d)  => d,
        Err(e) => return NodeOutput::failure(e),
    };

    match list_objects(client, bucket, &prefix, delimiter.as_deref(), cap).await {
        Err(text) => NodeOutput::failure(NodeError::unrecoverable("LIST_ERROR", format!("List failed: {}", text))),
        Ok(listing) => {
            let count = listing.objects.len();
            let mut logs = vec![format!(
                "Listed {} object(s) and {} common prefix(es) with prefix '{}'",
                count,
                listing.common_prefixes.len(),
                prefix
            )];
            if listing.truncated {
                logs.push(format!(
                    "More entries match than the {} returned; narrow the prefix or raise max_objects to see the rest",
                    cap
                ));
            }
            NodeOutput::success_with_logs(
                json!({
                    "objects":         listing.objects,
                    "common_prefixes": listing.common_prefixes,
                    "prefix":          prefix,
                    "count":           count,
                    "truncated":       listing.truncated,
                    "max_objects":     cap
                }),
                logs,
            )
        }
    }
}

/// `max_objects`: unset means the default, otherwise a whole number from 1 to
/// `MAX_LIST_OBJECTS`. Out-of-range values fail rather than being clamped, so
/// a typo cannot silently return a different amount than asked for.
fn parse_max_objects(v: &Value) -> Result<usize, NodeError> {
    let invalid = || NodeError::unrecoverable(
        "INVALID_CONFIG",
        format!("max_objects must be a whole number from 1 to {}", MAX_LIST_OBJECTS),
    );
    match cfg_f64_opt(v, "max_objects")? {
        None => Ok(MAX_LIST_OBJECTS),
        Some(n) if n.is_finite() && n.fract() == 0.0 && n >= 1.0 && n <= MAX_LIST_OBJECTS as f64 => Ok(n as usize),
        Some(_) => Err(invalid()),
    }
}

/// `delimiter`: unset or empty means none; otherwise it must be text.
fn parse_delimiter(v: &Value) -> Result<Option<String>, NodeError> {
    match v {
        Value::Null => Ok(None),
        Value::String(s) if s.is_empty() => Ok(None),
        Value::String(s) => Ok(Some(s.clone())),
        _ => Err(NodeError::unrecoverable("INVALID_CONFIG", "delimiter must be text")),
    }
}

struct Listing {
    objects: Vec<Value>,
    common_prefixes: Vec<String>,
    truncated: bool,
}

/// Follows continuation tokens until the listing ends or `cap` entries
/// (objects plus common prefixes, which S3 counts together against its own
/// key limit) are held. `truncated` is true when matching entries were left out.
async fn list_objects(
    client: &Client,
    bucket: &str,
    prefix: &str,
    delimiter: Option<&str>,
    cap: usize,
) -> Result<Listing, String> {
    let mut objects: Vec<Value> = Vec::new();
    let mut common_prefixes: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let wanted = (cap - objects.len() - common_prefixes.len()).min(1000) as i32;
        let page = client
            .list_objects_v2()
            .bucket(bucket)
            .prefix(prefix)
            .set_delimiter(delimiter.map(str::to_string))
            .max_keys(wanted)
            .set_continuation_token(token.take())
            .send()
            .await
            .map_err(|e| sdk_error_text(&e))?;

        objects.extend(page.contents().iter().map(object_json));
        common_prefixes.extend(page.common_prefixes().iter().filter_map(|p| p.prefix().map(str::to_string)));
        let held = objects.len() + common_prefixes.len();
        if held > cap {
            objects.truncate(cap);
            common_prefixes.truncate(cap - objects.len());
            return Ok(Listing { objects, common_prefixes, truncated: true });
        }
        if !page.is_truncated().unwrap_or(false) {
            return Ok(Listing { objects, common_prefixes, truncated: false });
        }
        match page.next_continuation_token().filter(|t| !t.is_empty()) {
            Some(next) if held < cap => token = Some(next.to_string()),
            _ => return Ok(Listing { objects, common_prefixes, truncated: true }),
        }
    }
}

fn object_json(obj: &aws_sdk_s3::types::Object) -> Value {
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
}

async fn op_delete(client: &Client, bucket: &str, cfg: &Value) -> NodeOutput {
    let key = match cfg["key"].as_str().filter(|s| !s.is_empty()) {
        Some(k) => k.to_string(),
        None    => return NodeOutput::failure(NodeError::unrecoverable("MISSING_KEY", "key is required for delete")),
    };

    match client.delete_object().bucket(bucket).key(&key).send().await {
        Err(e) => NodeOutput::failure(NodeError::unrecoverable(
            "DELETE_ERROR",
            format!("Delete failed for key '{}': {}", key, sdk_error_text(&e)),
        )),
        Ok(_) => NodeOutput::success_with_logs(
            json!({ "key": key, "deleted": true }),
            vec![format!("Deleted '{}'", key)],
        ),
    }
}

/// Extra headers a presigned PUT signs into the URL.
#[derive(Default)]
struct PutHeaders {
    content_type: Option<String>,
    content_length: Option<u64>,
}

async fn presigned_request(
    client: &Client,
    bucket: &str,
    key: &str,
    method: &str,
    put: &PutHeaders,
    config: PresigningConfig,
) -> Result<aws_sdk_s3::presigning::PresignedRequest, String> {
    if method == "PUT" {
        client
            .put_object()
            .bucket(bucket)
            .key(key)
            .set_content_type(put.content_type.clone())
            .set_content_length(put.content_length.map(|n| n as i64))
            .presigned(config)
            .await
            .map_err(|e| sdk_error_text(&e))
    } else {
        client.get_object().bucket(bucket).key(key).presigned(config).await.map_err(|e| sdk_error_text(&e))
    }
}

/// Reads `content_type` and `content_length` for a presigned URL. Both are
/// only meaningful for PUT, so a GET that sets either is refused instead of
/// silently ignoring a restriction the caller expects to be enforced.
fn parse_put_headers(cfg: &Value, method: &str) -> Result<PutHeaders, NodeError> {
    let content_type = match &cfg["content_type"] {
        Value::Null => None,
        Value::String(s) if s.trim().is_empty() => None,
        Value::String(s) => {
            let t = s.trim();
            if t.chars().count() > 255 || t.chars().any(|c| c.is_control()) {
                return Err(NodeError::unrecoverable(
                    "INVALID_CONTENT_TYPE",
                    "content_type must be at most 255 characters with no control characters",
                ));
            }
            Some(t.to_string())
        }
        _ => return Err(NodeError::unrecoverable("INVALID_CONTENT_TYPE", "content_type must be text")),
    };
    let content_length = cfg_u64_opt(&cfg["content_length"], "content_length")?;
    if let Some(n) = content_length {
        if n > MAX_PRESIGN_PUT_BYTES {
            return Err(NodeError::unrecoverable(
                "INVALID_CONFIG",
                format!("content_length must be at most {} bytes (5 GiB, the S3 limit for one PUT)", MAX_PRESIGN_PUT_BYTES),
            ));
        }
    }
    if method != "PUT" && (content_type.is_some() || content_length.is_some()) {
        return Err(NodeError::unrecoverable(
            "PRESIGN_PUT_ONLY",
            "content_type and content_length apply only to presign_url with method PUT",
        ));
    }
    Ok(PutHeaders { content_type, content_length })
}

async fn op_presign(client: &Client, bucket: &str, cfg: &Value) -> NodeOutput {
    let key = match cfg["key"].as_str().filter(|s| !s.is_empty()) {
        Some(k) => k.to_string(),
        None    => return NodeOutput::failure(NodeError::unrecoverable(
            "MISSING_KEY",
            "key is required for presign_url",
        )),
    };

    let unknown_method = |shown: String| NodeError::unrecoverable(
        "UNKNOWN_METHOD",
        format!("Unknown method '{}'. Valid: GET, PUT", shown.chars().take(64).collect::<String>()),
    );
    let method = match &cfg["method"] {
        Value::Null => "GET",
        Value::String(m) => match m.trim().to_ascii_uppercase().as_str() {
            "" | "GET" => "GET",
            "PUT"      => "PUT",
            _          => return NodeOutput::failure(unknown_method(m.clone())),
        },
        other => return NodeOutput::failure(unknown_method(other.to_string())),
    };

    let put_headers = match parse_put_headers(cfg, method) {
        Ok(h)  => h,
        Err(e) => return NodeOutput::failure(e),
    };

    // AWS S3 presigned URL maximum is 7 days = 604 800 seconds.
    let expiry_secs: u64 = match cfg_f64_opt(&cfg["expiry_secs"], "expiry_secs") {
        Ok(v) => v.map_or(3600, |f| f.max(1.0) as u64).min(604_800),
        Err(e) => return NodeOutput::failure(e),
    };

    let presigning_config = match PresigningConfig::expires_in(Duration::from_secs(expiry_secs)) {
        Ok(c)  => c,
        Err(e) => return NodeOutput::failure(NodeError::unrecoverable(
            "PRESIGN_ERROR",
            format!("Invalid presign expiry: {}", e),
        )),
    };

    let presigned = presigned_request(client, bucket, &key, method, &put_headers, presigning_config).await;

    match presigned {
        Err(text) => NodeOutput::failure(NodeError::unrecoverable(
            "PRESIGN_ERROR",
            format!("Could not generate presigned URL for '{}': {}", key, text),
        )),
        Ok(presigned) => {
            let headers: serde_json::Map<String, Value> = presigned
                .headers()
                .map(|(name, value)| (name.to_string(), Value::String(value.to_string())))
                .collect();
            NodeOutput::success_with_logs(
                json!({
                    "url":        presigned.uri().to_string(),
                    "key":        key,
                    "method":     method,
                    "expires_in": expiry_secs,
                    "headers":    headers
                }),
                vec![format!("Generated presigned {} URL for '{}' (expires in {}s)", method, key, expiry_secs)],
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_client, list_objects, op_delete, op_download, op_list, op_presign, op_upload,
        parse_provider, presigned_request, Provider, PutHeaders, SsrfDnsResolver,
        MAX_LIST_OBJECTS,
    };
    use crate::nodes::util::SsrfPolicy;
    use aws_sdk_s3::presigning::PresigningConfig;
    use aws_smithy_runtime_api::client::dns::ResolveDns;
    use serde_json::{json, Value};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    };
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

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
            result.unwrap_err().message.contains("SSRF check failed"),
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
            result.unwrap_err().message.contains("SSRF check failed"),
            "rejection must come from the SSRF check, not some other config error"
        );
    }

    #[tokio::test]
    async fn dns_resolver_applies_the_endpoint_policy() {
        assert!(
            SsrfDnsResolver(SsrfPolicy::Strict).resolve_dns("localhost").await.is_err(),
            "loopback by name must be refused at connect time"
        );
        let ips = SsrfDnsResolver(SsrfPolicy::AllowLocal)
            .resolve_dns("localhost")
            .await
            .expect("AllowLocal must resolve localhost");
        assert!(!ips.is_empty() && ips.iter().all(|ip| ip.is_loopback()), "{ips:?}");
    }

    struct Stub {
        port: u16,
        seen: Arc<Mutex<Vec<(String, Vec<u8>)>>>,
    }

    /// Serves `responses` in order (the last repeats) on a loopback port and
    /// records each request's head and body.
    async fn stub_server(responses: Vec<(u16, String)>) -> Stub {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen: Arc<Mutex<Vec<(String, Vec<u8>)>>> = Arc::default();
        let next = Arc::new(AtomicUsize::new(0));
        let log = seen.clone();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = match listener.accept().await {
                    Ok(c) => c,
                    Err(_) => return,
                };
                let (responses, next, log) = (responses.clone(), next.clone(), log.clone());
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    let head_end = loop {
                        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            break p + 4;
                        }
                        match sock.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    };
                    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                    let length = head
                        .lines()
                        .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                        .unwrap_or(0);
                    while buf.len() < head_end + length {
                        match sock.read(&mut chunk).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => buf.extend_from_slice(&chunk[..n]),
                        }
                    }
                    log.lock().unwrap().push((head, buf[head_end..].to_vec()));
                    let i = next.fetch_add(1, Ordering::SeqCst).min(responses.len() - 1);
                    let (status, body) = &responses[i];
                    let reply = format!(
                        "HTTP/1.1 {status} X\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(reply.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        Stub { port, seen }
    }

    fn minio_cfg(port: u16) -> Value {
        cfg_for("minio", &format!("http://127.0.0.1:{port}"))
    }

    fn error_xml(code: &str, message: &str) -> String {
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><Error><Code>{code}</Code><Message>{message}</Message><RequestId>R1</RequestId></Error>")
    }

    fn failure_text(out: &crate::model::NodeOutput) -> String {
        assert!(!out.success, "expected a failure, got {:?}", out.output);
        let e = out.error.as_ref().unwrap();
        assert!(!e.recoverable, "S3 failures stay unrecoverable");
        format!("{}|{}", e.code, e.message)
    }

    fn list_page(keys: &[&str], next_token: Option<&str>) -> String {
        let contents: String = keys
            .iter()
            .map(|k| format!("<Contents><Key>{k}</Key><LastModified>2026-01-02T03:04:05.000Z</LastModified><Size>7</Size></Contents>"))
            .collect();
        let tail = match next_token {
            Some(t) => format!("<IsTruncated>true</IsTruncated><NextContinuationToken>{t}</NextContinuationToken>"),
            None => "<IsTruncated>false</IsTruncated>".to_string(),
        };
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><ListBucketResult><Name>test-bucket</Name><Prefix></Prefix><KeyCount>{}</KeyCount><MaxKeys>1000</MaxKeys>{contents}{tail}</ListBucketResult>", keys.len())
    }

    #[tokio::test]
    async fn unknown_provider_fails_naming_the_valid_providers() {
        for bad in [json!("gcs"), json!("awss"), json!(7)] {
            let mut cfg = cfg_for("aws", "");
            cfg["provider"] = bad.clone();
            let err = build_client(&cfg).await.err().unwrap_or_else(|| panic!("{bad} must fail"));
            assert_eq!(err.code, "UNKNOWN_PROVIDER", "{bad}");
            assert!(!err.recoverable);
            assert!(err.message.contains("aws, r2, minio"), "{}", err.message);
        }
        let accepted = [
            (Value::Null,      Provider::Aws),
            (json!(""),        Provider::Aws),
            (json!("aws"),     Provider::Aws),
            (json!("AWS"),     Provider::Aws),
            (json!(" R2 "),    Provider::R2),
            (json!("MinIO"),   Provider::Minio),
        ];
        for (value, expected) in accepted {
            let mut cfg = cfg_for("aws", "");
            cfg["provider"] = value.clone();
            assert_eq!(parse_provider(&cfg).ok(), Some(expected), "{value}");
        }
        for value in [json!("aws"), json!("AWS"), Value::Null, json!("")] {
            let mut cfg = cfg_for("aws", "");
            cfg["provider"] = value.clone();
            assert!(build_client(&cfg).await.is_ok(), "{value} builds an AWS client");
        }
    }

    #[tokio::test]
    async fn endpoint_on_aws_is_refused_but_blank_endpoint_is_not() {
        let err = build_client(&cfg_for("aws", "https://example.com")).await.err().expect("must fail");
        assert_eq!(err.code, "ENDPOINT_NOT_SUPPORTED");
        assert!(!err.recoverable);
        assert!(err.message.contains("r2 or minio"), "{}", err.message);

        assert!(build_client(&cfg_for("aws", "")).await.is_ok());
        let mut cfg = cfg_for("aws", "");
        cfg["endpoint"] = Value::Null;
        assert!(build_client(&cfg).await.is_ok());
    }

    #[tokio::test]
    async fn service_error_code_and_message_reach_every_failure_text() {
        let stub = stub_server(vec![
            (403, error_xml("SignatureDoesNotMatch", "The request signature we calculated does not match")),
            (404, error_xml("NoSuchKey", "The specified key does not exist.")),
            (404, error_xml("NoSuchBucket", "The specified bucket does not exist")),
            (403, error_xml("AccessDenied", "Access Denied")),
        ])
        .await;
        let cfg = minio_cfg(stub.port);
        let (client, bucket) = build_client(&cfg).await.unwrap();
        let with = |extra: Value| {
            let mut c = cfg.clone();
            for (k, v) in extra.as_object().unwrap() {
                c[k] = v.clone();
            }
            c
        };

        let up = failure_text(&op_upload(&client, &bucket, &with(json!({"key": "k", "content": "x"}))).await);
        assert!(up.starts_with("UPLOAD_ERROR|") && up.contains("SignatureDoesNotMatch: The request signature"), "{up}");

        let down = failure_text(&op_download(&client, &bucket, &with(json!({"key": "k"}))).await);
        assert!(down.starts_with("DOWNLOAD_ERROR|") && down.contains("NoSuchKey: The specified key does not exist."), "{down}");

        let list = failure_text(&op_list(&client, &bucket, &cfg).await);
        assert!(list.starts_with("LIST_ERROR|") && list.contains("NoSuchBucket: The specified bucket"), "{list}");

        let del = failure_text(&op_delete(&client, &bucket, &with(json!({"key": "k"}))).await);
        assert!(del.starts_with("DELETE_ERROR|") && del.contains("AccessDenied: Access Denied"), "{del}");
        for text in [&up, &down, &list, &del] {
            assert!(!text.to_ascii_lowercase().contains("x-amz-") && !text.contains("RequestId"), "no headers or body dump: {text}");
        }
    }

    #[tokio::test]
    async fn failure_text_is_capped_and_falls_back_to_the_http_status_without_a_service_code() {
        let stub = stub_server(vec![
            (403, error_xml("AccessDenied", &"a".repeat(3000))),
            (403, String::new()),
        ])
        .await;
        let cfg = minio_cfg(stub.port);
        let (client, bucket) = build_client(&cfg).await.unwrap();
        let mut c = cfg.clone();
        c["key"] = json!("k");

        let long = failure_text(&op_delete(&client, &bucket, &c).await);
        assert!(long.chars().count() < 700, "{} chars", long.chars().count());

        let empty = failure_text(&op_delete(&client, &bucket, &c).await);
        assert!(empty.starts_with("DELETE_ERROR|Delete failed for key 'k': "), "{empty}");
        assert!(empty.contains("HTTP 403"), "{empty}");
        assert!(!empty.to_ascii_lowercase().contains("x-amz-"), "{empty}");
    }

    #[tokio::test]
    async fn unreachable_endpoint_failure_says_more_than_dispatch_failure() {
        let closed = {
            let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
            l.local_addr().unwrap().port()
        };
        let cfg = minio_cfg(closed);
        let (client, bucket) = build_client(&cfg).await.unwrap();
        let text = failure_text(&op_list(&client, &bucket, &cfg).await);
        assert!(text.starts_with("LIST_ERROR|List failed: dispatch failure: "), "{text}");
        assert!(text.len() > "LIST_ERROR|List failed: dispatch failure".len() + 5, "{text}");
    }

    #[tokio::test]
    async fn upload_decodes_data_uri_and_line_broken_base64() {
        let stub = stub_server(vec![(200, String::new())]).await;
        let cfg = minio_cfg(stub.port);
        let (client, bucket) = build_client(&cfg).await.unwrap();
        let mut c = cfg.clone();
        c["key"] = json!("a.txt");
        c["content"] = json!("data:text/plain;base64,SGVs\r\nbG8gd29ybGQ=");
        c["content_encoding"] = json!("base64");
        let out = op_upload(&client, &bucket, &c).await;
        assert!(out.success, "{:?}", out.error);
        assert_eq!(out.output.as_ref().unwrap()["size"], 11);
        let seen = stub.seen.lock().unwrap();
        let body = &seen[0].1;
        assert!(body.windows(11).any(|w| w == b"Hello world"), "{:?}", String::from_utf8_lossy(body));
    }

    #[tokio::test]
    async fn upload_with_unknown_content_encoding_fails_before_any_request() {
        let stub = stub_server(vec![(200, String::new())]).await;
        let cfg = minio_cfg(stub.port);
        let (client, bucket) = build_client(&cfg).await.unwrap();
        for bad in [json!("base-64"), json!("utf8"), json!(true)] {
            let mut c = cfg.clone();
            c["key"] = json!("a.txt");
            c["content"] = json!("hello");
            c["content_encoding"] = bad.clone();
            let text = failure_text(&op_upload(&client, &bucket, &c).await);
            assert!(text.starts_with("UNKNOWN_CONTENT_ENCODING|") && text.contains("text, base64"), "{bad}: {text}");
        }
        assert!(stub.seen.lock().unwrap().is_empty(), "no request may leave on a config error");
    }

    #[tokio::test]
    async fn list_follows_continuation_tokens_and_reports_not_truncated() {
        let stub = stub_server(vec![
            (200, list_page(&["a", "b"], Some("tok-1"))),
            (200, list_page(&["c"], None)),
        ])
        .await;
        let cfg = minio_cfg(stub.port);
        let (client, bucket) = build_client(&cfg).await.unwrap();
        let out = op_list(&client, &bucket, &cfg).await;
        assert!(out.success, "{:?}", out.error);
        let o = out.output.unwrap();
        assert_eq!(o["count"], 3);
        assert_eq!(o["truncated"], false);
        assert_eq!(o["max_objects"], MAX_LIST_OBJECTS);
        assert_eq!(o["objects"][2]["key"], "c");
        assert_eq!(o["objects"][0]["last_modified"], "2026-01-02T03:04:05+00:00");
        let seen = stub.seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert!(!seen[0].0.contains("continuation-token"), "{}", seen[0].0);
        assert!(seen[1].0.contains("continuation-token=tok-1"), "{}", seen[1].0);
    }

    #[tokio::test]
    async fn list_over_the_cap_returns_what_it_has_with_truncated_true() {
        let pages = || {
            vec![
                (200, list_page(&["a", "b"], Some("tok-1"))),
                (200, list_page(&["c", "d"], Some("tok-2"))),
                (200, list_page(&["e"], None)),
            ]
        };

        let stub = stub_server(pages()).await;
        let cfg = minio_cfg(stub.port);
        let (client, bucket) = build_client(&cfg).await.unwrap();
        let listing = list_objects(&client, &bucket, "", None, 3).await.unwrap();
        assert_eq!(listing.objects.len(), 3);
        assert!(listing.truncated, "a server page that overshoots the cap is cut and flagged");

        let stub = stub_server(pages()).await;
        let cfg = minio_cfg(stub.port);
        let (client, bucket) = build_client(&cfg).await.unwrap();
        let listing = list_objects(&client, &bucket, "", None, 2).await.unwrap();
        assert_eq!(listing.objects.len(), 2);
        assert!(listing.truncated, "more pages exist but the cap is reached");
        assert_eq!(stub.seen.lock().unwrap().len(), 1, "no request is made once the cap is reached");
    }

    #[tokio::test]
    async fn list_error_on_a_later_page_fails_instead_of_returning_a_partial_list() {
        let stub = stub_server(vec![
            (200, list_page(&["a"], Some("tok-1"))),
            (403, error_xml("AccessDenied", "Access Denied")),
        ])
        .await;
        let cfg = minio_cfg(stub.port);
        let (client, bucket) = build_client(&cfg).await.unwrap();
        let text = failure_text(&op_list(&client, &bucket, &cfg).await);
        assert!(text.starts_with("LIST_ERROR|") && text.contains("AccessDenied: Access Denied"), "{text}");
    }

    #[tokio::test]
    async fn presign_signs_for_the_requested_method_and_rejects_unknown_ones() {
        let cfg = minio_cfg(1);
        let (client, bucket) = build_client(&cfg).await.unwrap();
        let config = || PresigningConfig::expires_in(Duration::from_secs(60)).unwrap();
        let get = presigned_request(&client, &bucket, "k", "GET", &PutHeaders::default(), config()).await.unwrap();
        let put = presigned_request(&client, &bucket, "k", "PUT", &PutHeaders::default(), config()).await.unwrap();
        assert_eq!(get.method(), "GET");
        assert_eq!(put.method(), "PUT");

        let with = |m: Value| {
            let mut c = cfg.clone();
            c["key"] = json!("k");
            c["method"] = m;
            c
        };
        let default = op_presign(&client, &bucket, &with(Value::Null)).await;
        assert_eq!(default.output.unwrap()["method"], "GET");
        let put_out = op_presign(&client, &bucket, &with(json!("PUT"))).await;
        let o = put_out.output.unwrap();
        assert_eq!(o["method"], "PUT");
        assert!(put.headers().next().is_none(), "a PUT URL needs no extra signed headers from the caller");
        let lower = op_presign(&client, &bucket, &with(json!(" put "))).await;
        assert_eq!(lower.output.unwrap()["method"], "PUT");
        for bad in [json!("POST"), json!(1)] {
            let text = failure_text(&op_presign(&client, &bucket, &with(bad.clone())).await);
            assert!(text.starts_with("UNKNOWN_METHOD|") && text.contains("GET, PUT"), "{bad}: {text}");
        }
    }
}
