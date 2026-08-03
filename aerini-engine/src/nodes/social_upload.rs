// Social Upload Node
//
// Uploads media contract files to YouTube, Instagram, or TikTok.
//
// Gaps addressed:
//   G2  Instagram always requires publicly accessible URL — base64 data returns
//       INSTAGRAM_NEEDS_PUBLIC_URL immediately.
//   G6  OAuth handled by oauth_listener, fixed port 42069.
//   G7  TikTok: 10 MB chunks (5 MB min required by API — enforced per spec).
//       Files < 5 MB uploaded as single chunk. Chunk retry: 3 attempts (G7).
//   G8  File size checks: Instagram 100 MB, TikTok 4 GB.
//
// VERIFIED endpoints (May 2026):
//   YouTube upload:
//     POST https://www.googleapis.com/upload/youtube/v3/videos
//          ?uploadType=multipart&part=snippet,status
//   Instagram content publish:
//     POST https://graph.instagram.com/{user_id}/media
//     POST https://graph.instagram.com/{user_id}/media_publish
//   TikTok Direct Post:
//     POST https://open.tiktokapis.com/v2/post/publish/video/init/
//     PUT  {upload_url}  (returned by init, Content-Range per chunk)
//     POST https://open.tiktokapis.com/v2/post/publish/status/fetch/

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use once_cell::sync::Lazy;
use serde_json::{json, Value};
use std::time::Duration;
use uuid::Uuid;

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts};
use crate::nodes::oauth_listener;

// ── Constants ─────────────────────────────────────────────────────────────────

const TIKTOK_MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024; // 4 GB (G8)
const TIKTOK_CHUNK_SIZE: usize = 10 * 1024 * 1024;    // 10 MB (G7)
const TIKTOK_CHUNK_RETRIES: u32 = 3;                   // retries per chunk (G7)

// ── Shared HTTP client ────────────────────────────────────────────────────────

static UPLOAD_CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("Social upload HTTP client build failed")
});

// ── Node struct ───────────────────────────────────────────────────────────────

pub struct SocialUploadNode;

#[async_trait]
impl Node for SocialUploadNode {
    fn type_id(&self) -> &'static str { "social_upload" }
    fn display_name(&self) -> &'static str { "Social Upload" }
    fn node_type(&self) -> NodeType { NodeType::Action }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Upload video or image content to YouTube, Instagram, or TikTok." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["files", "platform", "title"],
            "properties": {
                "files":         { "type": "array",  "description": "Media contract files array" },
                "platform":      { "type": "string",  "enum": ["youtube", "instagram", "tiktok"] },
                "title":         { "type": "string",  "description": "Post title" },
                "description":   { "type": "string",  "description": "Post description (optional)" },
                "tags":          { "type": "string",  "description": "Comma-separated tags (YouTube)" },
                "privacy":       {
                    "type": "string",
                    "description": "Privacy level. YouTube: public|private|unlisted. TikTok: public_to_everyone|mutual_follow_friends|self_only"
                },
                "client_id":     { "type": "string",  "description": "OAuth client ID / client key — resolved from Connections (credential store)" },
                "client_secret": { "type": "string",  "description": "OAuth client secret — resolved from Connections (credential store)" }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "uploaded": { "type": "array",  "description": "Successfully uploaded files" },
                "count":    { "type": "number", "description": "Number of files uploaded" },
                "platform": { "type": "string", "description": "Target platform" },
                "errors":   { "type": "array",  "description": "Upload errors with explanation and action" }
            }
        })
    }

    fn ports(&self) -> NodePorts { NodePorts::default() }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let cfg = &input.input;

        let platform = match cfg["platform"].as_str() {
            Some(p) => p,
            None => return NodeOutput::failure(NodeError::unrecoverable(
                "MISSING_FIELD", "Missing required field: platform",
            )),
        };
        let title = cfg["title"].as_str().unwrap_or("").to_string();
        let description = cfg["description"].as_str().unwrap_or("").to_string();
        let tags = cfg["tags"].as_str().unwrap_or("").to_string();
        let privacy = cfg["privacy"].as_str().unwrap_or("private").to_string();
        let client_id = match cfg["client_id"].as_str() {
            Some(v) => v,
            None => return NodeOutput::failure(NodeError::unrecoverable(
                "MISSING_FIELD", "Missing required field: client_id",
            )),
        };
        let client_secret = match cfg["client_secret"].as_str() {
            Some(v) => v,
            None => return NodeOutput::failure(NodeError::unrecoverable(
                "MISSING_FIELD", "Missing required field: client_secret",
            )),
        };
        let files = match cfg["files"].as_array() {
            Some(f) => f,
            None => return NodeOutput::failure(NodeError::unrecoverable(
                "MISSING_FIELD", "Missing required field: files (array)",
            )),
        };

        if files.is_empty() {
            return NodeOutput::failure(NodeError::unrecoverable(
                "NO_FILES", "No files to upload.",
            ));
        }

        // Obtain OAuth tokens (handles storage, refresh, full OAuth flow).
        let tokens = match oauth_listener::get_tokens(platform, client_id, client_secret).await {
            Ok(t) => t,
            Err(e) => return NodeOutput::failure(e),
        };

        let mut uploaded: Vec<Value> = Vec::new();
        let mut errors: Vec<Value> = Vec::new();

        for file_val in files {
            let filename = file_val["filename"].as_str().unwrap_or("file");
            let data = file_val["data"].as_str().unwrap_or("");
            let mime_type = file_val["mime_type"].as_str().unwrap_or("application/octet-stream");

            let result = match platform {
                "youtube" => {
                    upload_to_youtube(
                        filename, data, mime_type,
                        &title, &description, &tags, &privacy,
                        &tokens.access_token,
                    ).await
                }
                "instagram" => {
                    upload_to_instagram(filename, data, mime_type, &title).await
                }
                "tiktok" => {
                    upload_to_tiktok(
                        filename, data, mime_type,
                        &title, &description, &privacy,
                        &tokens.access_token,
                    ).await
                }
                p => Err(UploadError {
                    code: "UNKNOWN_PLATFORM".into(),
                    message: format!("Unknown platform: {}", p),
                    explanation: format!("Platform '{}' is not supported. Use: youtube, instagram, tiktok.", p),
                    action: "Set platform to a supported value.".into(),
                }),
            };

            match result {
                Ok(record) => uploaded.push(record),
                Err(e) => errors.push(json!({
                    "filename": filename,
                    "code": e.code,
                    "message": e.message,
                    "explanation": e.explanation,
                    "action": e.action,
                })),
            }
        }

        let count = uploaded.len();
        NodeOutput::success(json!({
            "uploaded": uploaded,
            "count": count,
            "platform": platform,
            "errors": errors,
        }))
    }
}

// ── Internal error type ───────────────────────────────────────────────────────

struct UploadError {
    code: String,
    message: String,
    explanation: String,
    action: String,
}

fn upload_err(
    code: &str,
    message: impl Into<String>,
    explanation: impl Into<String>,
    action: impl Into<String>,
) -> UploadError {
    UploadError {
        code: code.into(),
        message: message.into(),
        explanation: explanation.into(),
        action: action.into(),
    }
}

fn map_http_error(status: u16, platform: &str) -> UploadError {
    match status {
        401 => upload_err(
            "AUTH_FAILED",
            format!("{} returned 401 Unauthorized", platform),
            "The access token was rejected — it may have been revoked or expired.",
            "Re-authenticate: open the node, clear stored credentials, and reconnect.",
        ),
        403 => upload_err(
            "QUOTA_EXCEEDED",
            format!("{} returned 403 Forbidden", platform),
            "Upload was rejected — quota may be exceeded or the scope is missing.",
            "Check your API quota in the developer console. Wait 24h or request a quota increase.",
        ),
        413 => upload_err(
            "FILE_TOO_LARGE",
            format!("{} returned 413 Payload Too Large", platform),
            "The file exceeds the platform's size limit.",
            "Compress or split the file before uploading.",
        ),
        429 => upload_err(
            "RATE_LIMITED",
            format!("{} returned 429 Too Many Requests", platform),
            "The API rate limit was hit.",
            "Wait a few minutes and retry.",
        ),
        s if s >= 500 => upload_err(
            "PLATFORM_ERROR",
            format!("{} returned {}", platform, s),
            "The platform's servers encountered an error.",
            "Try again later. If the problem persists, check the platform's status page.",
        ),
        s => upload_err(
            "UPLOAD_FAILED",
            format!("{} returned HTTP {}", platform, s),
            "Upload was rejected with an unexpected status code.",
            "Check the platform's API documentation for this status code.",
        ),
    }
}

// ── YouTube ───────────────────────────────────────────────────────────────────

/// Uploads a single file to YouTube using the multipart upload protocol.
///
/// Endpoint (May 2026):
///   POST https://www.googleapis.com/upload/youtube/v3/videos?uploadType=multipart&part=snippet,status
///
/// Body format: multipart/related with metadata JSON part and binary video part.
#[allow(clippy::too_many_arguments)]
async fn upload_to_youtube(
    filename: &str,
    data: &str,
    mime_type: &str,
    title: &str,
    description: &str,
    tags_csv: &str,
    privacy: &str,
    access_token: &str,
) -> Result<Value, UploadError> {
    let file_bytes = BASE64.decode(data).map_err(|_| upload_err(
        "DECODE_ERROR",
        "Failed to decode base64 file data",
        "The file data in the media contract is not valid base64.",
        "Check the upstream node that produced this file.",
    ))?;

    let tags: Vec<&str> = if tags_csv.is_empty() {
        vec![]
    } else {
        tags_csv.split(',').map(str::trim).collect()
    };

    let metadata = json!({
        "snippet": {
            "title": title,
            "description": description,
            "tags": tags,
        },
        "status": {
            "privacyStatus": privacy,
        }
    });
    let metadata_json = metadata.to_string();

    let boundary = format!("aerini_yt_{}", Uuid::new_v4().simple());

    let mut body: Vec<u8> = Vec::new();
    // Part 1: metadata
    body.extend_from_slice(format!("--{boundary}\r\n", boundary = boundary).as_bytes());
    body.extend_from_slice(b"Content-Type: application/json; charset=UTF-8\r\n\r\n");
    body.extend_from_slice(metadata_json.as_bytes());
    body.extend_from_slice(b"\r\n");
    // Part 2: video bytes
    body.extend_from_slice(format!("--{boundary}\r\n", boundary = boundary).as_bytes());
    body.extend_from_slice(format!("Content-Type: {}\r\n\r\n", mime_type).as_bytes());
    body.extend_from_slice(&file_bytes);
    body.extend_from_slice(b"\r\n");
    // Closing boundary
    body.extend_from_slice(format!("--{boundary}--\r\n", boundary = boundary).as_bytes());

    let content_type = format!("multipart/related; boundary=\"{}\"", boundary);

    let resp = UPLOAD_CLIENT
        .post("https://www.googleapis.com/upload/youtube/v3/videos")
        .query(&[("uploadType", "multipart"), ("part", "snippet,status")])
        .bearer_auth(access_token)
        .header("Content-Type", content_type)
        .body(body)
        .send()
        .await
        .map_err(|e| upload_err(
            "NETWORK_ERROR",
            e.to_string(),
            "Failed to reach YouTube's servers.",
            "Check your internet connection and retry.",
        ))?;

    let status = resp.status().as_u16();
    let body_val: Value = resp.json().await.unwrap_or(Value::Null);

    if status == 200 || status == 201 {
        let video_id = body_val["id"].as_str().unwrap_or("unknown");
        Ok(json!({
            "filename": filename,
            "platform_id": video_id,
            "url": format!("https://www.youtube.com/watch?v={}", video_id),
        }))
    } else {
        Err(map_http_error(status, "YouTube"))
    }
}

// ── Instagram ─────────────────────────────────────────────────────────────────

/// Instagram requires media hosted at a publicly accessible URL (G2).
/// Base64-encoded local data cannot be uploaded directly — return a clear error.
async fn upload_to_instagram(
    filename: &str,
    _data: &str,
    _mime_type: &str,
    _caption: &str,
) -> Result<Value, UploadError> {
    Err(upload_err(
        "INSTAGRAM_NEEDS_PUBLIC_URL",
        format!("Instagram upload of '{}' requires a public URL", filename),
        "Instagram's API requires media to be hosted at a publicly accessible URL. \
         Raw file data (base64) cannot be sent directly to Instagram.",
        "Upload your file to a web server or CDN first, then use the public URL \
         as the data field value instead of local base64 data.",
    ))
}

// ── TikTok ────────────────────────────────────────────────────────────────────

/// Uploads a single file to TikTok using the Direct Post chunked upload flow.
///
/// Flow (May 2026):
///   1. POST /v2/post/publish/video/init/  → publish_id + upload_url
///   2. PUT {upload_url} per chunk with Content-Range header
///   3. Return publish_id (processing is async on TikTok's side)
async fn upload_to_tiktok(
    filename: &str,
    data: &str,
    _mime_type: &str,
    title: &str,
    description: &str,
    privacy: &str,
    access_token: &str,
) -> Result<Value, UploadError> {
    let file_bytes = BASE64.decode(data).map_err(|_| upload_err(
        "DECODE_ERROR",
        "Failed to decode base64 file data",
        "The file data in the media contract is not valid base64.",
        "Check the upstream node that produced this file.",
    ))?;

    let total_size = file_bytes.len() as u64;

    // File size check (G8): TikTok maximum 4 GB.
    if total_size > TIKTOK_MAX_BYTES {
        return Err(upload_err(
            "FILE_TOO_LARGE",
            format!("File '{}' ({:.1} GB) exceeds TikTok's 4 GB limit", filename, total_size as f64 / 1e9),
            "TikTok's Content Posting API rejects files larger than 4 GB.",
            "Compress or trim the video before uploading.",
        ));
    }

    // Chunk calculation (G7).
    // TikTok API spec: total_chunk_count = floor(video_size / chunk_size).
    // The final chunk receives all remaining bytes (may exceed chunk_size).
    // Minimum chunk size: 5 MB. Files <= chunk_size are uploaded as a single chunk.
    // Files between 5 MB and TIKTOK_CHUNK_SIZE: single chunk, chunk_size = file size.
    let (chunk_size, total_chunks) = if total_size <= TIKTOK_CHUNK_SIZE as u64 {
        // Single chunk — chunk_size == video_size per API docs for small files.
        (total_size as usize, 1u64)
    } else {
        let cs = TIKTOK_CHUNK_SIZE;
        let tc = total_size / cs as u64; // floor division per API spec
        // tc >= 1 because total_size > TIKTOK_CHUNK_SIZE
        (cs, tc)
    };

    // Map privacy level string to TikTok API enum value.
    let privacy_level = match privacy {
        "public_to_everyone"   => "PUBLIC_TO_EVERYONE",
        "mutual_follow_friends" => "MUTUAL_FOLLOW_FRIENDS",
        _ => "SELF_ONLY",
    };

    // Step 1: Initialize direct post.
    let init_body = json!({
        "post_info": {
            "title": title,
            "description": description,
            "privacy_level": privacy_level,
            "disable_duet": false,
            "disable_comment": false,
            "disable_stitch": false,
        },
        "source_info": {
            "source": "FILE_UPLOAD",
            "video_size": total_size,
            "chunk_size": chunk_size,
            "total_chunk_count": total_chunks,
        }
    });

    let init_resp = UPLOAD_CLIENT
        .post("https://open.tiktokapis.com/v2/post/publish/video/init/")
        .bearer_auth(access_token)
        .header("Content-Type", "application/json; charset=UTF-8")
        .json(&init_body)
        .send()
        .await
        .map_err(|e| upload_err(
            "NETWORK_ERROR",
            e.to_string(),
            "Failed to reach TikTok's servers.",
            "Check your internet connection and retry.",
        ))?;

    let init_status = init_resp.status().as_u16();
    if init_status != 200 {
        return Err(map_http_error(init_status, "TikTok"));
    }

    let init_body_val: Value = init_resp.json().await.map_err(|_| upload_err(
        "PARSE_ERROR",
        "Failed to parse TikTok init response",
        "TikTok's init response was not valid JSON.",
        "Retry. If the problem persists, check your API credentials.",
    ))?;

    let publish_id = init_body_val["data"]["publish_id"]
        .as_str()
        .ok_or_else(|| upload_err(
            "PARSE_ERROR",
            "No publish_id in TikTok init response",
            "TikTok did not return a publish_id — the init call may have failed silently.",
            "Retry. If the error persists, check your TikTok developer console.",
        ))?
        .to_string();

    let upload_url = init_body_val["data"]["upload_url"]
        .as_str()
        .ok_or_else(|| upload_err(
            "PARSE_ERROR",
            "No upload_url in TikTok init response",
            "TikTok did not return an upload_url.",
            "Retry.",
        ))?
        .to_string();

    // Step 2: Upload chunks with retry (G7).
    for chunk_idx in 0..total_chunks {
        let start = (chunk_idx as usize) * chunk_size;
        // Last chunk takes all remaining bytes (handles remainder from floor division).
        let end_exclusive = if chunk_idx == total_chunks - 1 {
            file_bytes.len()
        } else {
            std::cmp::min(start + chunk_size, file_bytes.len())
        };
        let end_inclusive = end_exclusive - 1; // Content-Range is inclusive
        let chunk = file_bytes[start..end_exclusive].to_vec();
        let content_range = format!(
            "bytes {}-{}/{}",
            start, end_inclusive, total_size
        );

        let mut last_error: Option<UploadError> = None;
        for attempt in 0..TIKTOK_CHUNK_RETRIES {
            let resp = UPLOAD_CLIENT
                .put(&upload_url)
                .header("Content-Range", &content_range)
                .header("Content-Type", "video/mp4")
                .body(chunk.clone())
                .send()
                .await;

            match resp {
                Ok(r) if r.status().is_success() => {
                    last_error = None;
                    break;
                }
                Ok(r) => {
                    let s = r.status().as_u16();
                    last_error = Some(map_http_error(s, "TikTok"));
                    if attempt < TIKTOK_CHUNK_RETRIES - 1 {
                        tokio::time::sleep(Duration::from_secs(2u64.pow(attempt))).await;
                    }
                }
                Err(e) => {
                    last_error = Some(upload_err(
                        "NETWORK_ERROR",
                        e.to_string(),
                        format!("Network error uploading chunk {} of {} to TikTok.", chunk_idx + 1, total_chunks),
                        "Check your connection and retry.",
                    ));
                    if attempt < TIKTOK_CHUNK_RETRIES - 1 {
                        tokio::time::sleep(Duration::from_secs(2u64.pow(attempt))).await;
                    }
                }
            }
        }

        if let Some(err) = last_error {
            return Err(err);
        }
    }

    // TikTok processes video asynchronously after all chunks arrive.
    // Return the publish_id immediately without polling (processing can take minutes).
    Ok(json!({
        "filename": filename,
        "platform_id": publish_id,
        "url": format!("https://www.tiktok.com/ (processing — use publish_id: {})", publish_id),
    }))
}

