use async_trait::async_trait;
use base64::Engine as _;
use s3::bucket::Bucket;
use s3::creds::Credentials;
use s3::Region;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortDefinition, PortPosition};

pub struct S3Node;

#[async_trait]
impl Node for S3Node {
    fn type_id(&self)      -> &'static str { "s3_storage" }
    fn display_name(&self) -> &'static str { "S3 Storage" }
    fn node_type(&self)    -> NodeType     { NodeType::Action }
    fn version(&self)      -> &'static str { "1.0.0" }

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
                    "description": "Presigned URL expiry in seconds. Default: 3600."
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
                id: "input".to_string(), label: "In".to_string(), position: PortPosition::Left,
            }],
            outputs: vec![
                PortDefinition { id: "output".to_string(),   label: "Out".to_string(),   position: PortPosition::Right },
                PortDefinition { id: "on_error".to_string(), label: "Error".to_string(), position: PortPosition::Right },
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

        let bucket = match build_bucket(&input.input) {
            Ok(b)  => b,
            Err(e) => return NodeOutput::failure(NodeError::unrecoverable("CONFIG_ERROR", e)),
        };

        match operation.as_str() {
            "upload"      => op_upload(bucket, &input.input).await,
            "download"    => op_download(bucket, &input.input).await,
            "list"        => op_list(bucket, &input.input).await,
            "delete"      => op_delete(bucket, &input.input).await,
            "presign_url" => op_presign(bucket, &input.input).await,
            other => NodeOutput::failure(NodeError::unrecoverable(
                "UNKNOWN_OPERATION",
                format!("Unknown operation '{}'. Valid: upload, download, list, delete, presign_url", other),
            )),
        }
    }
}

// ── Bucket construction ────────────────────────────────────────────────────────

fn build_bucket(cfg: &Value) -> Result<Box<Bucket>, String> {
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
        .ok_or("bucket is required")?;

    let region_str = cfg["region"].as_str().filter(|s| !s.is_empty()).unwrap_or("us-east-1");
    let provider   = cfg["provider"].as_str().unwrap_or("aws");

    let creds = Credentials::new(Some(access_key), Some(secret_key), None, None, None)
        .map_err(|e| format!("Invalid credentials: {}", e))?;

    // R2 and MinIO require path-style addressing. AWS uses virtual-hosted style (default).
    let (region, path_style) = match provider {
        "r2" => {
            let endpoint = cfg["endpoint"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or("endpoint is required for R2. Example: https://ACCOUNT_ID.r2.cloudflarestorage.com")?;
            (Region::Custom { region: region_str.to_string(), endpoint: endpoint.to_string() }, true)
        }
        "minio" => {
            let endpoint = cfg["endpoint"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or("endpoint is required for MinIO. Example: http://host:9000")?;
            (Region::Custom { region: region_str.to_string(), endpoint: endpoint.to_string() }, true)
        }
        _ => {
            let region = region_str.parse::<Region>()
                .map_err(|e| format!("Invalid AWS region '{}': {}", region_str, e))?;
            (region, false)
        }
    };

    let bucket = Bucket::new(bucket_name, region, creds)
        .map_err(|e| format!("Could not initialise S3 client: {}", e))?;

    if path_style {
        Ok(bucket.with_path_style())
    } else {
        Ok(bucket)
    }
}

// ── Operations ─────────────────────────────────────────────────────────────────

async fn op_upload(bucket: Box<Bucket>, cfg: &Value) -> NodeOutput {
    let key = match cfg["key"].as_str().filter(|s| !s.is_empty()) {
        Some(k) => k.to_string(),
        None    => return NodeOutput::failure(NodeError::unrecoverable("MISSING_KEY", "key is required for upload")),
    };

    let content_raw = match cfg["content"].as_str() {
        Some(c) => c.to_string(),
        None    => return NodeOutput::failure(NodeError::unrecoverable("MISSING_CONTENT", "content is required for upload")),
    };

    let content_encoding = cfg["content_encoding"].as_str().unwrap_or("text");
    let content_type     = cfg["content_type"].as_str().unwrap_or("application/octet-stream");

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

    match bucket.put_object_with_content_type(&key, &bytes, content_type).await {
        Err(e) => NodeOutput::failure(NodeError::unrecoverable("UPLOAD_ERROR", format!("Upload failed: {}", e))),
        Ok(response) => {
            let status = response.status_code();
            if (200..300).contains(&status) {
                NodeOutput::success_with_logs(
                    json!({ "key": key, "size": bytes.len(), "content_type": content_type }),
                    vec![format!("Uploaded {} bytes to '{}'", bytes.len(), key)],
                )
            } else {
                NodeOutput::failure(NodeError::unrecoverable(
                    "UPLOAD_ERROR",
                    format!("Upload returned HTTP {}", status),
                ))
            }
        }
    }
}

async fn op_download(bucket: Box<Bucket>, cfg: &Value) -> NodeOutput {
    let key = match cfg["key"].as_str().filter(|s| !s.is_empty()) {
        Some(k) => k.to_string(),
        None    => return NodeOutput::failure(NodeError::unrecoverable("MISSING_KEY", "key is required for download")),
    };

    match bucket.get_object(&key).await {
        Err(e) => NodeOutput::failure(NodeError::unrecoverable(
            "DOWNLOAD_ERROR",
            format!("Download failed for key '{}': {}", key, e),
        )),
        Ok(response) => {
            let status = response.status_code();
            if (200..300).contains(&status) {
                let bytes   = response.bytes();
                let size    = bytes.len();
                let content = base64::engine::general_purpose::STANDARD.encode(bytes);
                NodeOutput::success_with_logs(
                    json!({
                        "key":              key,
                        "content":          content,
                        "content_encoding": "base64",
                        "size":             size
                    }),
                    vec![format!("Downloaded {} bytes from '{}'", size, key)],
                )
            } else {
                NodeOutput::failure(NodeError::unrecoverable(
                    "DOWNLOAD_ERROR",
                    format!("Download returned HTTP {}", status),
                ))
            }
        }
    }
}

async fn op_list(bucket: Box<Bucket>, cfg: &Value) -> NodeOutput {
    let prefix = cfg["prefix"].as_str().unwrap_or("").to_string();

    match bucket.list(prefix.clone(), None).await {
        Err(e) => NodeOutput::failure(NodeError::unrecoverable("LIST_ERROR", format!("List failed: {}", e))),
        Ok(results) => {
            let objects: Vec<Value> = results
                .iter()
                .flat_map(|page| page.contents.iter())
                .map(|obj| json!({
                    "key":           obj.key,
                    "size":          obj.size,
                    "last_modified": obj.last_modified,
                }))
                .collect();
            let count = objects.len();
            NodeOutput::success_with_logs(
                json!({ "objects": objects, "prefix": prefix, "count": count }),
                vec![format!("Listed {} object(s) with prefix '{}'", count, prefix)],
            )
        }
    }
}

async fn op_delete(bucket: Box<Bucket>, cfg: &Value) -> NodeOutput {
    let key = match cfg["key"].as_str().filter(|s| !s.is_empty()) {
        Some(k) => k.to_string(),
        None    => return NodeOutput::failure(NodeError::unrecoverable("MISSING_KEY", "key is required for delete")),
    };

    match bucket.delete_object(&key).await {
        Err(e) => NodeOutput::failure(NodeError::unrecoverable(
            "DELETE_ERROR",
            format!("Delete failed for key '{}': {}", key, e),
        )),
        Ok(response) => {
            let status = response.status_code();
            if (200..300).contains(&status) {
                NodeOutput::success_with_logs(
                    json!({ "key": key, "deleted": true }),
                    vec![format!("Deleted '{}'", key)],
                )
            } else {
                NodeOutput::failure(NodeError::unrecoverable(
                    "DELETE_ERROR",
                    format!("Delete returned HTTP {}", status),
                ))
            }
        }
    }
}

async fn op_presign(bucket: Box<Bucket>, cfg: &Value) -> NodeOutput {
    let key = match cfg["key"].as_str().filter(|s| !s.is_empty()) {
        Some(k) => k.to_string(),
        None    => return NodeOutput::failure(NodeError::unrecoverable(
            "MISSING_KEY",
            "key is required for presign_url",
        )),
    };

    let expiry_secs = cfg["expiry_secs"]
        .as_u64()
        .or_else(|| cfg["expiry_secs"].as_f64().map(|f| f.max(0.0) as u64))
        .unwrap_or(3600)
        .min(u32::MAX as u64) as u32;

    match bucket.presign_get(&key, expiry_secs, None::<HashMap<String, String>>).await {
        Err(e) => NodeOutput::failure(NodeError::unrecoverable(
            "PRESIGN_ERROR",
            format!("Could not generate presigned URL for '{}': {}", key, e),
        )),
        Ok(url) => NodeOutput::success_with_logs(
            json!({ "url": url, "key": key, "expires_in": expiry_secs }),
            vec![format!("Generated presigned URL for '{}' (expires in {}s)", key, expiry_secs)],
        ),
    }
}
