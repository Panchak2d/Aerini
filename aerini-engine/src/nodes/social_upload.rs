// Social Upload Node
//
// Uploads media contract files to YouTube, Instagram, or TikTok.
//
// Behavior:
//   - Every input is validated before any sign-in starts: platform, title,
//     privacy, client id and secret, and each file entry. A bad value fails
//     fast instead of opening a browser first.
//   - Instagram publishes from a public http(s) URL given as the file's
//     `data`; base64 data cannot be sent to it.
//   - TikTok uploads in 10 MB chunks (the API's minimum is 5 MB); files under
//     10 MB go up as a single chunk. A chunk is retried up to 3 times for a
//     network error, 408, 429 or 5xx; any other 4xx fails at once.
//   - YouTube uses the resumable protocol with the whole file in one request.
//   - Request timeouts scale with the amount of data sent.
//   - The node fails when no file was uploaded. When some files uploaded and
//     others failed, it succeeds and lists the failures in `errors`.
//   - OAuth is handled by oauth_listener on fixed port 42069.
//
// Endpoints:
//   YouTube:   POST https://www.googleapis.com/upload/youtube/v3/videos
//                   ?uploadType=resumable&part=snippet,status  (Location header),
//              then PUT the bytes to that session URL
//   Instagram: GET  https://graph.instagram.com/{version}/me
//              POST {graph}/{ig_user_id}/media, GET {graph}/{container}?fields=status_code,
//              POST {graph}/{ig_user_id}/media_publish
//   TikTok:    POST https://open.tiktokapis.com/v2/post/publish/video/init/
//              PUT  {upload_url}  (returned by init, Content-Range per chunk)
//              POST https://open.tiktokapis.com/v2/post/publish/status/fetch/

use async_trait::async_trait;
use once_cell::sync::Lazy;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts};
use crate::nodes::oauth_listener;
use crate::nodes::util::{
    decode_file_data, is_ssrf_blocked, read_json_response_capped, read_text_capped, reqwest_err_msg,
    scrub_url_in_error,
};

// ── Constants ─────────────────────────────────────────────────────────────────

const TIKTOK_MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024; // 4 GB
const TIKTOK_CHUNK_SIZE: usize = 10 * 1024 * 1024;    // 10 MB
const TIKTOK_CHUNK_RETRIES: u32 = 3;                   // attempts per chunk
const TIKTOK_CAPTION_MAX_UTF16: usize = 2200;
const TIKTOK_STATUS_WAIT: Duration = Duration::from_secs(90);

const YOUTUBE_UPLOAD_URL: &str = "https://www.googleapis.com/upload/youtube/v3/videos";
const YOUTUBE_TITLE_MAX_CHARS: usize = 100;
const YOUTUBE_DESCRIPTION_MAX_BYTES: usize = 5000;

const INSTAGRAM_GRAPH: &str = "https://graph.instagram.com/v26.0";
const INSTAGRAM_CAPTION_MAX_CHARS: usize = 2200;
const INSTAGRAM_IMAGE_WAIT: Duration = Duration::from_secs(120);
const INSTAGRAM_VIDEO_WAIT: Duration = Duration::from_secs(600);
const INSTAGRAM_POLL_FAULTS: u32 = 3;

const API_TIMEOUT: Duration = Duration::from_secs(60);
const MIN_TRANSFER_SECS: u64 = 120;
const MAX_TRANSFER_SECS: u64 = 12 * 3600;
const ASSUMED_MIN_BYTES_PER_SEC: u64 = 128 * 1024;

const ERROR_BODY_READ_BYTES: usize = 4096;
const ERROR_DETAIL_MAX_CHARS: usize = 300;

// ── Shared HTTP client ────────────────────────────────────────────────────────

// No total timeout: each request sets its own through `API_TIMEOUT` or
// `transfer_timeout`, so a large upload is not cut off by a fixed limit.
static UPLOAD_CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    crate::nodes::util::guarded_client_builder(crate::nodes::util::SsrfPolicy::Strict)
        .connect_timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("Social upload HTTP client build failed")
});

/// Time allowed for one request that sends `bytes` of body, assuming the
/// connection sustains at least 128 KiB/s, within 2 minutes to 12 hours.
fn transfer_timeout(bytes: usize) -> Duration {
    let secs = (bytes as u64 / ASSUMED_MIN_BYTES_PER_SEC).clamp(MIN_TRANSFER_SECS, MAX_TRANSFER_SECS);
    Duration::from_secs(secs)
}

// ── Validated input ───────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Platform {
    YouTube,
    Instagram,
    TikTok,
}

impl Platform {
    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "youtube" => Some(Self::YouTube),
            "instagram" => Some(Self::Instagram),
            "tiktok" => Some(Self::TikTok),
            _ => None,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::YouTube => "youtube",
            Self::Instagram => "instagram",
            Self::TikTok => "tiktok",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::YouTube => "YouTube",
            Self::Instagram => "Instagram",
            Self::TikTok => "TikTok",
        }
    }
}

#[derive(Debug)]
struct FileSpec {
    filename: String,
    data: String,
    mime: String,
}

#[derive(Debug)]
struct Plan {
    platform: Platform,
    title: String,
    description: String,
    caption: String,
    tags: Vec<String>,
    /// The platform's own wire value; empty for Instagram, which has no privacy field.
    privacy: &'static str,
    client_id: String,
    client_secret: String,
    files: Vec<FileSpec>,
}

fn invalid(code: &str, message: impl Into<String>) -> NodeError {
    NodeError::unrecoverable(code, message)
}

fn optional_text(cfg: &Value, key: &str) -> Result<String, NodeError> {
    match &cfg[key] {
        Value::Null => Ok(String::new()),
        Value::String(s) => Ok(s.trim().to_string()),
        _ => Err(invalid("INVALID_CONFIG", format!("{key} must be text"))),
    }
}

fn required_text(cfg: &Value, key: &str) -> Result<String, NodeError> {
    let value = optional_text(cfg, key)?;
    if value.is_empty() {
        return Err(invalid("MISSING_FIELD", format!("Missing required field: {key}")));
    }
    Ok(value)
}

fn caption_of(title: &str, description: &str) -> String {
    if description.is_empty() {
        title.to_string()
    } else {
        format!("{title}\n\n{description}")
    }
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

fn short(s: &str) -> String {
    s.chars().take(40).collect()
}

fn parse_privacy(platform: Platform, raw: &str) -> Result<&'static str, NodeError> {
    let key = raw.trim().to_ascii_lowercase();
    let choice = match (platform, key.as_str()) {
        (Platform::Instagram, _) => Some(""),
        (Platform::YouTube, "" | "private") => Some("private"),
        (Platform::YouTube, "public") => Some("public"),
        (Platform::YouTube, "unlisted") => Some("unlisted"),
        (Platform::TikTok, "" | "self_only") => Some("SELF_ONLY"),
        (Platform::TikTok, "public_to_everyone") => Some("PUBLIC_TO_EVERYONE"),
        (Platform::TikTok, "mutual_follow_friends") => Some("MUTUAL_FOLLOW_FRIENDS"),
        (Platform::TikTok, "follower_of_creator") => Some("FOLLOWER_OF_CREATOR"),
        _ => None,
    };
    choice.ok_or_else(|| {
        let allowed = match platform {
            Platform::YouTube => "public, private, unlisted",
            _ => "public_to_everyone, mutual_follow_friends, follower_of_creator, self_only",
        };
        invalid(
            "INVALID_PRIVACY",
            format!("Unknown privacy '{}' for {}. Use one of: {allowed}.", short(raw), platform.label()),
        )
    })
}

fn parse_tags(csv: &str) -> Vec<String> {
    csv.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

fn parse_files(cfg: &Value) -> Result<Vec<FileSpec>, NodeError> {
    let entries = cfg["files"]
        .as_array()
        .ok_or_else(|| invalid("MISSING_FIELD", "Missing required field: files (array)"))?;
    if entries.is_empty() {
        return Err(invalid("NO_FILES", "No files to upload."));
    }
    let mut files = Vec::with_capacity(entries.len());
    for (i, entry) in entries.iter().enumerate() {
        let obj = entry
            .as_object()
            .ok_or_else(|| invalid("INVALID_FILE", format!("files[{i}] is not a file object")))?;
        let filename = obj
            .get("filename")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("file")
            .to_string();
        let data = obj.get("data").and_then(Value::as_str).map(str::trim).unwrap_or("");
        if data.is_empty() {
            return Err(invalid("INVALID_FILE", format!("files[{i}] ('{filename}') has no data")));
        }
        let mime = safe_mime_type(obj.get("mime_type").and_then(Value::as_str)).to_string();
        files.push(FileSpec { filename, data: data.to_string(), mime });
    }
    Ok(files)
}

/// Validates the whole node input before any sign-in or network call.
fn parse_plan(cfg: &Value) -> Result<Plan, NodeError> {
    let platform_raw = required_text(cfg, "platform")?;
    let platform = Platform::parse(&platform_raw).ok_or_else(|| {
        invalid(
            "UNKNOWN_PLATFORM",
            format!("Unknown platform '{}'. Use: youtube, instagram, tiktok.", short(&platform_raw)),
        )
    })?;
    let title = required_text(cfg, "title")?;
    let description = optional_text(cfg, "description")?;
    let tags = if platform == Platform::YouTube {
        parse_tags(&optional_text(cfg, "tags")?)
    } else {
        Vec::new()
    };
    let privacy = parse_privacy(platform, &optional_text(cfg, "privacy")?)?;
    let client_id = required_text(cfg, "client_id")?;
    let client_secret = required_text(cfg, "client_secret")?;
    let caption = caption_of(&title, &description);

    match platform {
        Platform::YouTube => {
            if title.chars().count() > YOUTUBE_TITLE_MAX_CHARS {
                return Err(invalid(
                    "INVALID_FIELD",
                    format!("YouTube titles are limited to {YOUTUBE_TITLE_MAX_CHARS} characters."),
                ));
            }
            if title.contains(['<', '>']) || description.contains(['<', '>']) {
                return Err(invalid("INVALID_FIELD", "YouTube titles and descriptions cannot contain < or >."));
            }
            if description.len() > YOUTUBE_DESCRIPTION_MAX_BYTES {
                return Err(invalid(
                    "INVALID_FIELD",
                    format!("YouTube descriptions are limited to {YOUTUBE_DESCRIPTION_MAX_BYTES} bytes."),
                ));
            }
        }
        Platform::TikTok => {
            if utf16_len(&caption) > TIKTOK_CAPTION_MAX_UTF16 {
                return Err(invalid(
                    "INVALID_FIELD",
                    format!("TikTok captions (title plus description) are limited to {TIKTOK_CAPTION_MAX_UTF16} UTF-16 characters."),
                ));
            }
        }
        Platform::Instagram => {
            if caption.chars().count() > INSTAGRAM_CAPTION_MAX_CHARS {
                return Err(invalid(
                    "INVALID_FIELD",
                    format!("Instagram captions (title plus description) are limited to {INSTAGRAM_CAPTION_MAX_CHARS} characters."),
                ));
            }
        }
    }

    let files = parse_files(cfg)?;
    for file in &files {
        let checked = match platform {
            Platform::Instagram => instagram_target(file).map(|_| ()),
            Platform::TikTok => tiktok_content_type(file).map(|_| ()),
            Platform::YouTube => Ok(()),
        };
        checked.map_err(UploadError::into_node_error)?;
    }

    Ok(Plan { platform, title, description, caption, tags, privacy, client_id, client_secret, files })
}

/// The MIME type is sent as a request header value, so one carrying CR/LF or
/// other control bytes must not get through. Anything that is not printable
/// ASCII falls back to a generic type.
fn safe_mime_type(raw: Option<&str>) -> &str {
    match raw {
        Some(m) if !m.is_empty() && m.bytes().all(|b| (0x20..0x7f).contains(&b)) => m,
        _ => "application/octet-stream",
    }
}

fn extension_of(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default()
}

// ── Node struct ───────────────────────────────────────────────────────────────

pub struct SocialUploadNode;

#[async_trait]
impl Node for SocialUploadNode {
    fn type_id(&self) -> &'static str { "social_upload" }
    fn display_name(&self) -> &'static str { "Social Upload" }
    fn node_type(&self) -> NodeType { NodeType::Action }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str {
        "Upload video or image content to YouTube, Instagram, or TikTok. Instagram takes a public URL as the file's data. Fails when no file was uploaded."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["files", "platform", "title"],
            "properties": {
                "files":         { "type": "array",  "description": "Media contract files array. For Instagram, each file's data is a public http(s) URL." },
                "platform":      { "type": "string",  "enum": ["youtube", "instagram", "tiktok"] },
                "title":         { "type": "string",  "description": "Post title. TikTok and Instagram use it as the start of the caption." },
                "description":   { "type": "string",  "description": "Post description (optional). Added after the title in a TikTok or Instagram caption." },
                "tags":          { "type": "string",  "description": "Comma-separated tags (YouTube)" },
                "privacy":       {
                    "type": "string",
                    "description": "Privacy level (default private). YouTube: public|private|unlisted. TikTok: public_to_everyone|mutual_follow_friends|follower_of_creator|self_only. Ignored by Instagram."
                },
                "client_id":     { "type": "string",  "description": "OAuth client ID / client key. Entered directly here; not currently offered as a saved-credential picker field." },
                "client_secret": { "type": "string",  "description": "OAuth client secret. Entered directly here; not currently offered as a saved-credential picker field." }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "uploaded": { "type": "array",  "description": "Successfully uploaded files" },
                "count":    { "type": "number", "description": "Number of files uploaded" },
                "failed":   { "type": "number", "description": "Number of files that failed" },
                "platform": { "type": "string", "description": "Target platform" },
                "errors":   { "type": "array",  "description": "Upload errors with explanation and action" }
            }
        })
    }

    fn ports(&self) -> NodePorts { NodePorts::default() }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let plan = match parse_plan(&input.input) {
            Ok(p) => p,
            Err(e) => return NodeOutput::failure(e),
        };

        let tokens = match oauth_listener::get_tokens(plan.platform.key(), &plan.client_id, &plan.client_secret).await {
            Ok(t) => t,
            Err(e) => return NodeOutput::failure(e),
        };

        let instagram_user = if plan.platform == Platform::Instagram {
            match instagram_user_id(&tokens.access_token).await {
                Ok(id) => Some(id),
                Err(e) => {
                    discard_rejected_tokens(&plan, &[&e]).await;
                    return NodeOutput::failure(e.into_node_error());
                }
            }
        } else {
            None
        };

        let mut uploaded: Vec<Value> = Vec::new();
        let mut failed: Vec<(String, UploadError)> = Vec::new();

        for file in &plan.files {
            let result = match plan.platform {
                Platform::YouTube => upload_to_youtube(file, &plan, &tokens.access_token).await,
                Platform::Instagram => {
                    let user_id = instagram_user.as_deref().unwrap_or_default();
                    upload_to_instagram(file, &plan, user_id, &tokens.access_token).await
                }
                Platform::TikTok => upload_to_tiktok(file, &plan, &tokens.access_token).await,
            };
            match result {
                Ok(record) => uploaded.push(record),
                Err(e) => failed.push((file.filename.clone(), e)),
            }
        }

        let errors: Vec<&UploadError> = failed.iter().map(|(_, e)| e).collect();
        discard_rejected_tokens(&plan, &errors).await;
        build_output(plan.platform, uploaded, failed)
    }
}

/// A provider that answers 401 has rejected the stored access token. Dropping
/// the stored tokens makes the next run sign in again; keeping them would fail
/// every run the same way until the keychain entry was removed by hand.
async fn discard_rejected_tokens(plan: &Plan, errors: &[&UploadError]) {
    if errors.iter().any(|e| e.code == "AUTH_FAILED") {
        oauth_listener::clear_tokens(plan.platform.key(), &plan.client_id).await;
    }
}

fn build_output(platform: Platform, uploaded: Vec<Value>, failed: Vec<(String, UploadError)>) -> NodeOutput {
    if uploaded.is_empty() && !failed.is_empty() {
        return NodeOutput::failure(all_failed_error(&failed));
    }
    let count = uploaded.len();
    let errors: Vec<Value> = failed
        .iter()
        .map(|(filename, e)| json!({
            "filename": filename,
            "code": e.code,
            "message": e.message,
            "explanation": e.explanation,
            "action": e.action,
        }))
        .collect();
    NodeOutput::success(json!({
        "uploaded": uploaded,
        "count": count,
        "failed": errors.len(),
        "platform": platform.key(),
        "errors": errors,
    }))
}

/// Nothing was posted, so a retry cannot duplicate a post; only a rate limit
/// is worth retrying automatically.
fn all_failed_error(failed: &[(String, UploadError)]) -> NodeError {
    let first_code = failed[0].1.code.as_str();
    let same_code = failed.iter().all(|(_, e)| e.code == first_code);
    let retry = failed.iter().all(|(_, e)| e.code == "RATE_LIMITED");
    let code = if same_code { first_code } else { "UPLOAD_FAILED" };

    let (first_name, first) = &failed[0];
    let mut message = format!("{first_name}: {}", first.message);
    if failed.len() == 1 {
        for part in [&first.explanation, &first.action] {
            if !part.is_empty() {
                message.push(' ');
                message.push_str(part);
            }
        }
    } else {
        for (name, e) in failed.iter().skip(1).take(2) {
            message.push_str(&format!("; {name}: {}", e.message));
        }
        if failed.len() > 3 {
            message.push_str(&format!("; and {} more", failed.len() - 3));
        }
    }
    if retry {
        NodeError::recoverable(code, message)
    } else {
        NodeError::unrecoverable(code, message)
    }
}

// ── Internal error type ───────────────────────────────────────────────────────

#[derive(Debug)]
struct UploadError {
    code: String,
    message: String,
    explanation: String,
    action: String,
}

impl UploadError {
    fn into_node_error(self) -> NodeError {
        let mut message = self.message;
        for part in [&self.explanation, &self.action] {
            if !part.is_empty() {
                message.push(' ');
                message.push_str(part);
            }
        }
        if self.code == "RATE_LIMITED" {
            NodeError::recoverable(self.code, message)
        } else {
            NodeError::unrecoverable(self.code, message)
        }
    }

    fn is_transient(&self) -> bool {
        matches!(self.code.as_str(), "NETWORK_ERROR" | "PLATFORM_ERROR" | "RATE_LIMITED")
    }
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

fn network_error(e: &reqwest::Error, platform: &str) -> UploadError {
    if is_ssrf_blocked(e) {
        return upload_err(
            "SSRF_BLOCKED",
            reqwest_err_msg(e),
            "The address was refused by Aerini's network policy.",
            "Use a public address.",
        );
    }
    upload_err(
        "NETWORK_ERROR",
        reqwest_err_msg(e),
        format!("Failed to reach {platform}'s servers."),
        "Check your internet connection and retry.",
    )
}

fn decode_error() -> UploadError {
    upload_err(
        "DECODE_ERROR",
        "Failed to decode base64 file data",
        "The file data in the media contract is not valid base64.",
        "Check the upstream node that produced this file.",
    )
}

fn decode_media(file: &FileSpec) -> Result<Vec<u8>, UploadError> {
    let bytes = decode_file_data(&file.data).map_err(|_| decode_error())?;
    if bytes.is_empty() {
        return Err(upload_err(
            "EMPTY_FILE",
            format!("File '{}' has no content", file.filename),
            "The file data decoded to zero bytes.",
            "Check the upstream node that produced this file.",
        ));
    }
    Ok(bytes)
}

fn with_detail(message: String, detail: &str) -> String {
    if detail.is_empty() { message } else { format!("{message}: {detail}") }
}

fn map_http_error(status: u16, platform: &str, detail: &str) -> UploadError {
    match status {
        401 => upload_err(
            "AUTH_FAILED",
            with_detail(format!("{platform} returned 401 Unauthorized"), detail),
            "The access token was rejected; it may have been revoked or expired.",
            "The stored sign-in was discarded. Run the node again to sign in.",
        ),
        403 if detail.to_ascii_lowercase().contains("quota") => upload_err(
            "QUOTA_EXCEEDED",
            with_detail(format!("{platform} returned 403 Forbidden"), detail),
            "The API quota is used up.",
            "Wait for the quota to reset (daily) or request an increase in the developer console.",
        ),
        403 => upload_err(
            "FORBIDDEN",
            with_detail(format!("{platform} returned 403 Forbidden"), detail),
            "The platform refused the request: the account may lack permission, a scope may be missing, or the app may not be approved for this action.",
            "Read the reason above and check the app's permissions in the developer console.",
        ),
        413 => upload_err(
            "FILE_TOO_LARGE",
            with_detail(format!("{platform} returned 413 Payload Too Large"), detail),
            "The file exceeds the platform's size limit.",
            "Compress or split the file before uploading.",
        ),
        429 => upload_err(
            "RATE_LIMITED",
            with_detail(format!("{platform} returned 429 Too Many Requests"), detail),
            "The API rate limit was hit.",
            "Wait a few minutes and retry.",
        ),
        s if s >= 500 => upload_err(
            "PLATFORM_ERROR",
            with_detail(format!("{platform} returned {s}"), detail),
            "The platform's servers encountered an error.",
            "Try again later. If the problem persists, check the platform's status page.",
        ),
        s => upload_err(
            "UPLOAD_FAILED",
            with_detail(format!("{platform} returned HTTP {s}"), detail),
            "The platform rejected the request.",
            "Read the reason above, or check the platform's API documentation for this status code.",
        ),
    }
}

async fn failure_from_response(resp: reqwest::Response, platform: &str) -> UploadError {
    let status = resp.status().as_u16();
    let text = read_text_capped(resp, ERROR_BODY_READ_BYTES).await;
    map_http_error(status, platform, &error_detail(&text))
}

fn scalar_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Pulls the human-readable reason out of a provider error body. Handles the
/// Google (`error.message`, `error.errors[0].reason`), TikTok (`error.code`,
/// `error.message`) and Meta (`error.message`, numeric `error.code`) shapes.
fn json_error_detail(v: &Value) -> String {
    let err = &v["error"];
    let message = scalar_text(&err["message"])
        .or_else(|| scalar_text(&err["error_user_msg"]))
        .or_else(|| scalar_text(&v["error_description"]))
        .or_else(|| scalar_text(&v["message"]))
        .or_else(|| if err.is_string() { scalar_text(err) } else { None });
    let code = scalar_text(&err["code"]);
    let reason = scalar_text(&err["errors"][0]["reason"]);

    let mut out = String::new();
    if let Some(c) = code {
        out.push_str(&c);
    }
    if let Some(m) = message {
        if !out.is_empty() {
            out.push_str(": ");
        }
        out.push_str(&m);
    }
    if let Some(r) = reason {
        if out.is_empty() {
            out = r;
        } else {
            out = format!("{out} ({r})");
        }
    }
    out
}

/// A short, secret-free reason for a failed provider call. A body that is not
/// JSON is shown as plain text unless it looks like an HTML gateway page.
fn error_detail(body: &str) -> String {
    let body = body.trim();
    let detail = match serde_json::from_str::<Value>(body) {
        Ok(v) => json_error_detail(&v),
        Err(_) if body.is_empty() || body.starts_with('<') => String::new(),
        Err(_) => body.to_string(),
    };
    let detail = scrub_url_in_error(&detail);
    let detail: String = detail.split_whitespace().collect::<Vec<_>>().join(" ");
    if detail.chars().count() > ERROR_DETAIL_MAX_CHARS {
        let cut: String = detail.chars().take(ERROR_DETAIL_MAX_CHARS).collect();
        format!("{cut}…")
    } else {
        detail
    }
}

fn parse_error(platform: &str, what: &str) -> UploadError {
    upload_err(
        "PARSE_ERROR",
        format!("{platform} sent an unreadable {what}"),
        format!("{platform}'s reply was not in the expected format."),
        "Retry. If the problem persists, check your API credentials and the platform's status page.",
    )
}

// ── YouTube ───────────────────────────────────────────────────────────────────

/// The resumable session URL carries the bearer token on the next request, so
/// it is only followed when it stays on a Google API host over https.
fn youtube_session_url(location: &str) -> Option<url::Url> {
    let parsed = url::Url::parse(location).ok()?;
    let host = parsed.host_str()?;
    let google = host == "googleapis.com" || host.ends_with(".googleapis.com");
    (parsed.scheme() == "https" && google).then_some(parsed)
}

/// Uploads a single file to YouTube with the resumable protocol: one request
/// opens a session and returns its URL in `Location`, a second sends the
/// whole file there.
async fn upload_to_youtube(file: &FileSpec, plan: &Plan, access_token: &str) -> Result<Value, UploadError> {
    let bytes = decode_media(file)?;

    let metadata = json!({
        "snippet": {
            "title": plan.title,
            "description": plan.description,
            "tags": plan.tags,
        },
        "status": {
            "privacyStatus": plan.privacy,
        }
    });

    let session = UPLOAD_CLIENT
        .post(YOUTUBE_UPLOAD_URL)
        .query(&[("uploadType", "resumable"), ("part", "snippet,status")])
        .bearer_auth(access_token)
        .header("X-Upload-Content-Length", bytes.len().to_string())
        .header("X-Upload-Content-Type", file.mime.as_str())
        .json(&metadata)
        .timeout(API_TIMEOUT)
        .send()
        .await
        .map_err(|e| network_error(&e, "YouTube"))?;

    if !session.status().is_success() {
        return Err(failure_from_response(session, "YouTube").await);
    }
    let location = session
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .and_then(youtube_session_url)
        .ok_or_else(|| parse_error("YouTube", "upload session address"))?;

    let timeout = transfer_timeout(bytes.len());
    let resp = UPLOAD_CLIENT
        .put(location.as_str())
        .bearer_auth(access_token)
        .header(reqwest::header::CONTENT_TYPE, file.mime.as_str())
        .timeout(timeout)
        .body(bytes)
        .send()
        .await
        .map_err(|e| network_error(&e, "YouTube"))?;

    let status = resp.status().as_u16();
    if status != 200 && status != 201 {
        return Err(failure_from_response(resp, "YouTube").await);
    }
    let body: Value = read_json_response_capped(resp)
        .await
        .map_err(|_| parse_error("YouTube", "upload reply"))?;
    let video_id = body["id"].as_str().ok_or_else(|| parse_error("YouTube", "upload reply (no video id)"))?;
    Ok(json!({
        "filename": file.filename,
        "platform_id": video_id,
        "url": format!("https://www.youtube.com/watch?v={video_id}"),
        "status": "uploaded",
    }))
}

// ── Instagram ─────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InstagramKind {
    Image,
    Video,
}

fn needs_public_url(file: &FileSpec) -> UploadError {
    upload_err(
        "INSTAGRAM_NEEDS_PUBLIC_URL",
        format!("Instagram upload of '{}' requires a public URL", file.filename),
        "Instagram fetches the media itself, so each file's data must be a publicly reachable http(s) URL. Base64 data cannot be sent.",
        "Upload the file to a web server or CDN first and pass its URL as the file's data.",
    )
}

/// Returns the media URL and whether it is an image or a video. The kind comes
/// from the MIME type, or from the URL's file extension when the MIME type is
/// generic.
fn instagram_target(file: &FileSpec) -> Result<(String, InstagramKind), UploadError> {
    let url = url::Url::parse(&file.data).map_err(|_| needs_public_url(file))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(needs_public_url(file));
    }
    let mime = file.mime.to_ascii_lowercase();
    let kind = if mime.starts_with("video/") {
        Some(InstagramKind::Video)
    } else if mime.starts_with("image/") {
        Some(InstagramKind::Image)
    } else {
        match extension_of(url.path()).as_str() {
            "mp4" | "mov" | "m4v" => Some(InstagramKind::Video),
            "jpg" | "jpeg" | "png" | "gif" | "webp" => Some(InstagramKind::Image),
            _ => None,
        }
    };
    let kind = kind.ok_or_else(|| upload_err(
        "INVALID_MEDIA_TYPE",
        format!("Cannot tell whether '{}' is an image or a video", file.filename),
        "The file's MIME type is not image/* or video/*, and its URL has no known extension.",
        "Set the file's mime_type (for example image/jpeg or video/mp4).",
    ))?;
    Ok((url.to_string(), kind))
}

/// Graph ids are numeric and are placed in a request path, so anything else
/// is refused rather than interpolated.
fn graph_id(raw: Option<String>, what: &str) -> Result<String, UploadError> {
    match raw {
        Some(id) if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) => Ok(id),
        _ => Err(parse_error("Instagram", what)),
    }
}

async fn instagram_request(req: reqwest::RequestBuilder) -> Result<Value, UploadError> {
    let resp = req
        .timeout(API_TIMEOUT)
        .send()
        .await
        .map_err(|e| network_error(&e, "Instagram"))?;
    if !resp.status().is_success() {
        return Err(failure_from_response(resp, "Instagram").await);
    }
    read_json_response_capped(resp)
        .await
        .map_err(|_| parse_error("Instagram", "reply"))
}

/// The professional account id the access token belongs to; every publishing
/// call is addressed to it.
async fn instagram_user_id(access_token: &str) -> Result<String, UploadError> {
    let body = instagram_request(
        UPLOAD_CLIENT
            .get(format!("{INSTAGRAM_GRAPH}/me"))
            .query(&[("fields", "user_id,username"), ("access_token", access_token)]),
    )
    .await?;
    graph_id(
        scalar_text(&body["user_id"]).or_else(|| scalar_text(&body["id"])),
        "account id",
    )
}

async fn upload_to_instagram(
    file: &FileSpec,
    plan: &Plan,
    user_id: &str,
    access_token: &str,
) -> Result<Value, UploadError> {
    let (media_url, kind) = instagram_target(file)?;

    let mut params: Vec<(&str, &str)> = vec![
        ("access_token", access_token),
        ("caption", plan.caption.as_str()),
    ];
    match kind {
        InstagramKind::Image => params.push(("image_url", media_url.as_str())),
        InstagramKind::Video => {
            params.push(("media_type", "REELS"));
            params.push(("video_url", media_url.as_str()));
        }
    }
    let created = instagram_request(
        UPLOAD_CLIENT.post(format!("{INSTAGRAM_GRAPH}/{user_id}/media")).form(&params),
    )
    .await?;
    let container = graph_id(scalar_text(&created["id"]), "media container id")?;

    let wait = match kind {
        InstagramKind::Image => INSTAGRAM_IMAGE_WAIT,
        InstagramKind::Video => INSTAGRAM_VIDEO_WAIT,
    };
    if wait_for_container(&container, access_token, wait).await? {
        return Ok(json!({
            "filename": file.filename,
            "platform_id": container,
            "url": Value::Null,
            "status": "published",
        }));
    }

    let published = instagram_request(
        UPLOAD_CLIENT
            .post(format!("{INSTAGRAM_GRAPH}/{user_id}/media_publish"))
            .form(&[("creation_id", container.as_str()), ("access_token", access_token)]),
    )
    .await?;
    let media_id = graph_id(scalar_text(&published["id"]), "published media id")?;

    let permalink = instagram_request(
        UPLOAD_CLIENT
            .get(format!("{INSTAGRAM_GRAPH}/{media_id}"))
            .query(&[("fields", "permalink"), ("access_token", access_token)]),
    )
    .await
    .ok()
    .and_then(|v| v["permalink"].as_str().map(str::to_string));

    Ok(json!({
        "filename": file.filename,
        "platform_id": media_id,
        "url": permalink,
        "status": "published",
    }))
}

/// Polls a media container until Instagram has finished fetching and
/// processing it. Returns `true` when the container was already published.
/// Three transient failures in a row end the wait.
async fn wait_for_container(container: &str, access_token: &str, wait: Duration) -> Result<bool, UploadError> {
    let deadline = Instant::now() + wait;
    let mut faults = 0u32;
    let mut round = 0u64;
    loop {
        match instagram_request(
            UPLOAD_CLIENT
                .get(format!("{INSTAGRAM_GRAPH}/{container}"))
                .query(&[("fields", "status_code"), ("access_token", access_token)]),
        )
        .await
        {
            Ok(body) => {
                faults = 0;
                match body["status_code"].as_str().unwrap_or("") {
                    "FINISHED" => return Ok(false),
                    "PUBLISHED" => return Ok(true),
                    "ERROR" => {
                        return Err(upload_err(
                            "INSTAGRAM_MEDIA_REJECTED",
                            "Instagram could not process the media",
                            "Instagram fetched the URL but rejected the media (unsupported format, size, duration or aspect ratio), or could not fetch it.",
                            "Check that the URL is public and the file meets Instagram's media requirements, then retry.",
                        ));
                    }
                    "EXPIRED" => {
                        return Err(upload_err(
                            "INSTAGRAM_CONTAINER_EXPIRED",
                            "The Instagram media container expired before it was published",
                            "A container must be published within 24 hours.",
                            "Run the node again.",
                        ));
                    }
                    _ => {}
                }
            }
            Err(e) if e.is_transient() => {
                faults += 1;
                if faults >= INSTAGRAM_POLL_FAULTS {
                    return Err(e);
                }
            }
            Err(e) => return Err(e),
        }
        if Instant::now() >= deadline {
            return Err(upload_err(
                "INSTAGRAM_PROCESSING_TIMEOUT",
                format!("Instagram was still processing the media after {} seconds", wait.as_secs()),
                "Nothing was published.",
                "Retry later, or use a smaller file.",
            ));
        }
        round += 1;
        tokio::time::sleep(Duration::from_secs((round + 1).min(8))).await;
    }
}

// ── TikTok ────────────────────────────────────────────────────────────────────

/// Tracks a TikTok Direct Post session between the moment `publish_id` is
/// obtained and the moment the upload either completes or fails. TikTok has
/// no abort endpoint for the `FILE_UPLOAD` source type this flow uses — if
/// this task is dropped mid-upload (workflow cancellation, panic, process
/// exit), TikTok is left processing a session this codebase can no longer
/// reach. Logging `publish_id` on drop is the only trail left if that
/// happens; there is nothing else to do about the orphaned session itself.
struct PublishIdGuard {
    publish_id: String,
    filename: String,
    disarmed: bool,
}

impl PublishIdGuard {
    fn new(publish_id: String, filename: String) -> Self {
        Self { publish_id, filename, disarmed: false }
    }

    fn disarm(&mut self) {
        self.disarmed = true;
    }
}

impl Drop for PublishIdGuard {
    fn drop(&mut self) {
        if !self.disarmed {
            tracing::warn!(
                "TikTok publish session for '{}' (publish_id={}) did not complete \
                 — no abort endpoint exists for this flow, so TikTok may still process it",
                self.filename,
                self.publish_id,
            );
        }
    }
}

fn tiktok_content_type(file: &FileSpec) -> Result<&'static str, UploadError> {
    match file.mime.to_ascii_lowercase().as_str() {
        "video/mp4" => return Ok("video/mp4"),
        "video/quicktime" => return Ok("video/quicktime"),
        "video/webm" => return Ok("video/webm"),
        "application/octet-stream" => match extension_of(&file.filename).as_str() {
            "mp4" => return Ok("video/mp4"),
            "mov" => return Ok("video/quicktime"),
            "webm" => return Ok("video/webm"),
            _ => {}
        },
        _ => {}
    }
    Err(upload_err(
        "INVALID_MEDIA_TYPE",
        format!("TikTok cannot take '{}' ({})", file.filename, file.mime),
        "TikTok accepts only video/mp4, video/quicktime and video/webm.",
        "Convert the file, or set its mime_type if it is one of those formats.",
    ))
}

/// `(chunk_size, total_chunk_count)`: the count is the floor of size over chunk
/// size and the last chunk takes the remainder; a file up to 10 MB is one chunk.
fn tiktok_chunking(total_size: u64) -> (usize, u64) {
    if total_size <= TIKTOK_CHUNK_SIZE as u64 {
        (total_size as usize, 1)
    } else {
        (TIKTOK_CHUNK_SIZE, total_size / TIKTOK_CHUNK_SIZE as u64)
    }
}

fn chunk_status_retryable(status: u16) -> bool {
    matches!(status, 408 | 429 | 500..=599)
}

/// The upload URL is valid for an hour and is called without credentials, so
/// it must be an https address.
fn tiktok_upload_url(raw: &str) -> Option<url::Url> {
    let parsed = url::Url::parse(raw).ok()?;
    (parsed.scheme() == "https" && parsed.host_str().is_some()).then_some(parsed)
}

/// TikTok can answer HTTP 200 with a failure in the body's `error.code`.
fn tiktok_api_error(body: &Value) -> Option<UploadError> {
    let code = body["error"]["code"].as_str().filter(|c| !c.is_empty() && *c != "ok")?;
    let detail = error_detail(&body.to_string());
    Some(match code {
        "access_token_invalid" | "scope_not_authorized" => map_http_error(401, "TikTok", &detail),
        "rate_limit_exceeded" => map_http_error(429, "TikTok", &detail),
        "unaudited_client_can_only_post_to_private_accounts" => upload_err(
            "TIKTOK_APP_UNAUDITED",
            with_detail("TikTok refused a non-private post".to_string(), &detail),
            "Posts from a TikTok app that has not passed TikTok's audit must be private.",
            "Set privacy to self_only, or submit the app for audit.",
        ),
        _ => upload_err(
            "UPLOAD_FAILED",
            with_detail("TikTok rejected the request".to_string(), &detail),
            "TikTok refused the request.",
            "Read the reason above, then check the field values and your app's permissions.",
        ),
    })
}

async fn send_chunk(
    url: &url::Url,
    chunk: &[u8],
    content_range: &str,
    content_type: &str,
    index: u64,
    total: u64,
) -> Result<(), UploadError> {
    let mut last: Option<UploadError> = None;
    for attempt in 0..TIKTOK_CHUNK_RETRIES {
        let resp = UPLOAD_CLIENT
            .put(url.as_str())
            .header("Content-Range", content_range)
            .header("Content-Type", content_type)
            .timeout(transfer_timeout(chunk.len()))
            .body(chunk.to_vec())
            .send()
            .await;

        let failure = match resp {
            Ok(r) if r.status().is_success() => return Ok(()),
            Ok(r) => {
                let status = r.status().as_u16();
                let e = failure_from_response(r, "TikTok").await;
                if !chunk_status_retryable(status) {
                    return Err(e);
                }
                e
            }
            Err(e) => {
                let mut up = network_error(&e, "TikTok");
                if is_ssrf_blocked(&e) {
                    return Err(up);
                }
                up.explanation = format!("Network error uploading chunk {} of {} to TikTok.", index + 1, total);
                up
            }
        };
        last = Some(failure);
        if attempt + 1 < TIKTOK_CHUNK_RETRIES {
            tokio::time::sleep(Duration::from_secs(2u64.pow(attempt))).await;
        }
    }
    Err(last.unwrap_or_else(|| upload_err(
        "UPLOAD_FAILED",
        "TikTok chunk upload failed",
        "The chunk was not accepted.",
        "Retry.",
    )))
}

/// Uploads a single file to TikTok using the Direct Post chunked upload flow.
///
/// Flow:
///   1. POST /v2/post/publish/video/init/  → publish_id + upload_url
///   2. PUT {upload_url} per chunk with Content-Range header
///   3. Poll /v2/post/publish/status/fetch/ for up to 90 s so a processing
///      failure is reported; a post still processing after that is returned
///      with status `processing`.
async fn upload_to_tiktok(file: &FileSpec, plan: &Plan, access_token: &str) -> Result<Value, UploadError> {
    let content_type = tiktok_content_type(file)?;
    let bytes = decode_media(file)?;
    let total_size = bytes.len() as u64;

    if total_size > TIKTOK_MAX_BYTES {
        return Err(upload_err(
            "FILE_TOO_LARGE",
            format!("File '{}' ({:.1} GB) exceeds TikTok's 4 GB limit", file.filename, total_size as f64 / 1e9),
            "TikTok's Content Posting API rejects files larger than 4 GB.",
            "Compress or trim the video before uploading.",
        ));
    }

    let (chunk_size, total_chunks) = tiktok_chunking(total_size);

    let init_body = json!({
        "post_info": {
            "title": plan.caption,
            "privacy_level": plan.privacy,
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
        .timeout(API_TIMEOUT)
        .send()
        .await
        .map_err(|e| network_error(&e, "TikTok"))?;

    if init_resp.status().as_u16() != 200 {
        return Err(failure_from_response(init_resp, "TikTok").await);
    }
    let init_val: Value = read_json_response_capped(init_resp)
        .await
        .map_err(|_| parse_error("TikTok", "init reply"))?;
    if let Some(e) = tiktok_api_error(&init_val) {
        return Err(e);
    }

    let publish_id = init_val["data"]["publish_id"]
        .as_str()
        .ok_or_else(|| parse_error("TikTok", "init reply (no publish_id)"))?
        .to_string();

    let mut publish_guard = PublishIdGuard::new(publish_id.clone(), file.filename.clone());

    let upload_url = init_val["data"]["upload_url"]
        .as_str()
        .and_then(tiktok_upload_url)
        .ok_or_else(|| parse_error("TikTok", "init reply (no https upload_url)"))?;

    for idx in 0..total_chunks {
        let start = (idx as usize) * chunk_size;
        let end = if idx == total_chunks - 1 { bytes.len() } else { start + chunk_size };
        let content_range = format!("bytes {}-{}/{}", start, end - 1, total_size);
        send_chunk(&upload_url, &bytes[start..end], &content_range, content_type, idx, total_chunks).await?;
    }

    publish_guard.disarm();

    let status = tiktok_final_status(&publish_id, access_token).await?;
    Ok(json!({
        "filename": file.filename,
        "platform_id": publish_id,
        "url": Value::Null,
        "status": status,
    }))
}

/// Waits for TikTok to finish with an uploaded video. Returns `published`,
/// `inbox` or `processing`; only a reported failure is an error, because the
/// upload itself already succeeded and a status lookup can fail for reasons
/// unrelated to the post.
async fn tiktok_final_status(publish_id: &str, access_token: &str) -> Result<&'static str, UploadError> {
    let deadline = Instant::now() + TIKTOK_STATUS_WAIT;
    loop {
        let resp = UPLOAD_CLIENT
            .post("https://open.tiktokapis.com/v2/post/publish/status/fetch/")
            .bearer_auth(access_token)
            .header("Content-Type", "application/json; charset=UTF-8")
            .json(&json!({ "publish_id": publish_id }))
            .timeout(API_TIMEOUT)
            .send()
            .await;
        let body = match resp {
            Ok(r) if r.status().is_success() => read_json_response_capped(r).await.ok(),
            _ => None,
        };
        let Some(body) = body.filter(|b| tiktok_api_error(b).is_none()) else {
            return Ok("processing");
        };
        match body["data"]["status"].as_str().unwrap_or("") {
            "PUBLISH_COMPLETE" => return Ok("published"),
            "SEND_TO_USER_INBOX" => return Ok("inbox"),
            "FAILED" => {
                let reason = body["data"]["fail_reason"].as_str().unwrap_or("no reason given");
                return Err(upload_err(
                    "TIKTOK_PUBLISH_FAILED",
                    format!("TikTok could not publish '{publish_id}': {reason}"),
                    "TikTok accepted the upload but failed while processing or publishing it.",
                    "Check the reason above (for example an unsupported codec, duration or account setting) and retry.",
                ));
            }
            _ => {}
        }
        if Instant::now() >= deadline {
            return Ok("processing");
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt;
    use std::sync::{Arc, Mutex};
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Metadata};

    fn base(platform: &str) -> Value {
        let (data, mime) = match platform {
            "instagram" => ("https://cdn.example.com/clip.mp4", "video/mp4"),
            _ => ("QUJD", "video/mp4"),
        };
        json!({
            "platform": platform,
            "title": "Title",
            "client_id": "id",
            "client_secret": "secret",
            "files": [{ "filename": "clip.mp4", "data": data, "mime_type": mime }],
        })
    }

    fn with(mut cfg: Value, key: &str, value: Value) -> Value {
        cfg[key] = value;
        cfg
    }

    fn code_of(cfg: &Value) -> String {
        parse_plan(cfg).unwrap_err().code
    }

    fn file(name: &str, data: &str, mime: &str) -> FileSpec {
        FileSpec { filename: name.into(), data: data.into(), mime: mime.into() }
    }

    #[test]
    fn safe_mime_type_keeps_ordinary_values_and_defaults_when_absent() {
        assert_eq!(safe_mime_type(Some("video/mp4")), "video/mp4");
        assert_eq!(safe_mime_type(None), "application/octet-stream");
    }

    #[test]
    fn safe_mime_type_rejects_header_injection() {
        assert_eq!(
            safe_mime_type(Some("video/mp4\r\nX-Injected: 1")),
            "application/octet-stream"
        );
    }

    #[test]
    fn valid_input_is_accepted_for_every_platform() {
        for p in ["youtube", "instagram", "tiktok"] {
            assert!(parse_plan(&base(p)).is_ok(), "platform {p}");
        }
    }

    #[test]
    fn platform_is_checked_before_anything_else_that_needs_the_network() {
        assert_eq!(code_of(&with(base("youtube"), "platform", json!("myspace"))), "UNKNOWN_PLATFORM");
        assert_eq!(
            parse_plan(&with(base("youtube"), "platform", json!(" TikTok "))).unwrap().platform,
            Platform::TikTok
        );
    }

    #[test]
    fn blank_required_fields_are_missing_fields() {
        for key in ["platform", "title", "client_id", "client_secret"] {
            assert_eq!(code_of(&with(base("youtube"), key, json!("  "))), "MISSING_FIELD", "field {key}");
        }
    }

    #[test]
    fn files_must_be_a_non_empty_array_of_objects_with_data() {
        let cases = [
            (json!([]), "NO_FILES"),
            (json!([1]), "INVALID_FILE"),
            (json!([{ "filename": "a" }]), "INVALID_FILE"),
            (json!([{ "filename": "a", "data": "  " }]), "INVALID_FILE"),
        ];
        for (files, code) in cases {
            assert_eq!(code_of(&with(base("youtube"), "files", files.clone())), code, "files {files}");
        }
        let mut cfg = base("youtube");
        cfg.as_object_mut().unwrap().remove("files");
        assert_eq!(code_of(&cfg), "MISSING_FIELD");
    }

    #[test]
    fn privacy_maps_to_each_platforms_wire_values() {
        let cases = [
            (Platform::YouTube, "", "private"),
            (Platform::YouTube, "Unlisted", "unlisted"),
            (Platform::YouTube, "public", "public"),
            (Platform::TikTok, "", "SELF_ONLY"),
            (Platform::TikTok, "public_to_everyone", "PUBLIC_TO_EVERYONE"),
            (Platform::TikTok, "mutual_follow_friends", "MUTUAL_FOLLOW_FRIENDS"),
            (Platform::TikTok, "FOLLOWER_OF_CREATOR", "FOLLOWER_OF_CREATOR"),
            (Platform::Instagram, "anything", ""),
        ];
        for (platform, raw, expected) in cases {
            assert_eq!(parse_privacy(platform, raw).unwrap(), expected, "{platform:?} {raw:?}");
        }
    }

    #[test]
    fn unknown_privacy_is_rejected_instead_of_downgraded() {
        assert_eq!(code_of(&with(base("tiktok"), "privacy", json!("public"))), "INVALID_PRIVACY");
        assert_eq!(code_of(&with(base("youtube"), "privacy", json!("friends"))), "INVALID_PRIVACY");
    }

    #[test]
    fn tags_drop_empty_entries() {
        assert_eq!(parse_tags("a, ,b,,"), vec!["a".to_string(), "b".to_string()]);
        assert!(parse_tags(" , ").is_empty());
    }

    #[test]
    fn youtube_title_and_description_rules_are_enforced_up_front() {
        assert_eq!(code_of(&with(base("youtube"), "title", json!("x".repeat(101)))), "INVALID_FIELD");
        assert_eq!(code_of(&with(base("youtube"), "title", json!("a <b> c"))), "INVALID_FIELD");
        assert_eq!(code_of(&with(base("youtube"), "description", json!("x".repeat(5001)))), "INVALID_FIELD");
        assert!(parse_plan(&with(base("youtube"), "title", json!("x".repeat(100)))).is_ok());
    }

    #[test]
    fn tiktok_caption_limit_counts_utf16_units() {
        let at_limit = with(base("tiktok"), "title", json!("😀".repeat(1100)));
        assert!(parse_plan(&at_limit).is_ok());
        let over = with(base("tiktok"), "title", json!("😀".repeat(1101)));
        assert_eq!(code_of(&over), "INVALID_FIELD");
    }

    #[test]
    fn caption_puts_the_description_after_the_title() {
        assert_eq!(caption_of("T", ""), "T");
        assert_eq!(caption_of("T", "D"), "T\n\nD");
    }

    #[test]
    fn instagram_takes_public_urls_and_classifies_the_media() {
        let cases = [
            ("https://x.io/a.jpg", "image/jpeg", InstagramKind::Image),
            ("https://x.io/a.mp4", "application/octet-stream", InstagramKind::Video),
            ("http://x.io/v", "video/mp4", InstagramKind::Video),
            ("https://x.io/a.PNG?sig=1", "application/octet-stream", InstagramKind::Image),
        ];
        for (url, mime, kind) in cases {
            let (_, got) = instagram_target(&file("f", url, mime)).unwrap();
            assert_eq!(got, kind, "{url}");
        }
    }

    #[test]
    fn instagram_refuses_anything_that_is_not_a_public_url_before_sign_in() {
        for data in ["QUJD", "data:video/mp4;base64,AAAA", "ftp://x.io/a.mp4", "file:///etc/passwd"] {
            let e = instagram_target(&file("f", data, "video/mp4")).unwrap_err();
            assert_eq!(e.code, "INSTAGRAM_NEEDS_PUBLIC_URL", "{data}");
        }
        let cfg = with(base("instagram"), "files", json!([{ "filename": "a.mp4", "data": "QUJD", "mime_type": "video/mp4" }]));
        assert_eq!(code_of(&cfg), "INSTAGRAM_NEEDS_PUBLIC_URL");
    }

    #[test]
    fn instagram_media_of_unknown_kind_is_rejected() {
        let e = instagram_target(&file("f", "https://x.io/file", "application/octet-stream")).unwrap_err();
        assert_eq!(e.code, "INVALID_MEDIA_TYPE");
    }

    #[test]
    fn graph_ids_must_be_numeric() {
        assert_eq!(graph_id(Some("1784".into()), "x").unwrap(), "1784");
        for bad in [None, Some(String::new()), Some("12/../x".into()), Some("1a".into())] {
            assert!(graph_id(bad, "x").is_err());
        }
    }

    #[test]
    fn tiktok_accepts_only_its_three_video_types() {
        let ok = [
            ("a.bin", "video/mp4", "video/mp4"),
            ("a.bin", "VIDEO/QUICKTIME", "video/quicktime"),
            ("a.bin", "video/webm", "video/webm"),
            ("a.mov", "application/octet-stream", "video/quicktime"),
            ("a.MP4", "application/octet-stream", "video/mp4"),
        ];
        for (name, mime, expected) in ok {
            assert_eq!(tiktok_content_type(&file(name, "QUJD", mime)).unwrap(), expected, "{name} {mime}");
        }
        for (name, mime) in [("a.mp4", "image/png"), ("a.avi", "application/octet-stream"), ("a.avi", "video/x-msvideo")] {
            assert_eq!(tiktok_content_type(&file(name, "QUJD", mime)).unwrap_err().code, "INVALID_MEDIA_TYPE");
        }
        let cfg = with(base("tiktok"), "files", json!([{ "filename": "a.png", "data": "QUJD", "mime_type": "image/png" }]));
        assert_eq!(code_of(&cfg), "INVALID_MEDIA_TYPE");
    }

    #[test]
    fn tiktok_chunking_uses_floor_division_with_the_remainder_in_the_last_chunk() {
        let mb = 1024 * 1024u64;
        let cases = [
            (1, (1usize, 1u64)),
            (10 * mb, (10 * 1024 * 1024, 1)),
            (10 * mb + 1, (10 * 1024 * 1024, 1)),
            (25 * mb, (10 * 1024 * 1024, 2)),
            (30 * mb, (10 * 1024 * 1024, 3)),
        ];
        for (size, expected) in cases {
            assert_eq!(tiktok_chunking(size), expected, "size {size}");
        }
    }

    #[test]
    fn only_transient_statuses_are_retried_per_chunk() {
        for s in [408u16, 429, 500, 502, 503] {
            assert!(chunk_status_retryable(s), "{s}");
        }
        for s in [400u16, 401, 403, 404, 413, 416] {
            assert!(!chunk_status_retryable(s), "{s}");
        }
    }

    #[test]
    fn transfer_timeout_scales_with_size_and_stays_bounded() {
        assert_eq!(transfer_timeout(0), Duration::from_secs(120));
        assert_eq!(transfer_timeout(1024 * 1024 * 1024), Duration::from_secs(8192));
        assert_eq!(transfer_timeout(usize::MAX / 2), Duration::from_secs(12 * 3600));
    }

    #[test]
    fn provider_reasons_are_pulled_out_of_each_error_shape() {
        let google = r#"{"error":{"code":403,"message":"You have exceeded your quota.","errors":[{"reason":"quotaExceeded"}]}}"#;
        let d = error_detail(google);
        assert!(d.contains("exceeded your quota") && d.contains("quotaExceeded"), "{d}");

        let tiktok = r#"{"error":{"code":"invalid_param","message":"bad privacy","log_id":"x"}}"#;
        assert_eq!(error_detail(tiktok), "invalid_param: bad privacy");

        let meta = r#"{"error":{"message":"Invalid OAuth access token.","type":"OAuthException","code":190}}"#;
        assert_eq!(error_detail(meta), "190: Invalid OAuth access token.");

        assert_eq!(error_detail("<html>Bad Gateway</html>"), "");
        assert_eq!(error_detail(""), "");
        assert_eq!(error_detail("upstream   said\nno"), "upstream said no");
        assert!(error_detail(&"x".repeat(2000)).chars().count() <= ERROR_DETAIL_MAX_CHARS + 1);
    }

    #[test]
    fn http_errors_carry_the_provider_reason_and_the_right_code() {
        let e = map_http_error(400, "TikTok", "invalid_param: bad");
        assert_eq!(e.code, "UPLOAD_FAILED");
        assert!(e.message.contains("invalid_param: bad"), "{}", e.message);
        assert_eq!(map_http_error(403, "YouTube", "403: x (quotaExceeded)").code, "QUOTA_EXCEEDED");
        assert_eq!(map_http_error(403, "Instagram", "10: permission denied").code, "FORBIDDEN");
        assert_eq!(map_http_error(401, "YouTube", "").code, "AUTH_FAILED");
        assert_eq!(map_http_error(429, "YouTube", "").code, "RATE_LIMITED");
        assert_eq!(map_http_error(503, "YouTube", "").code, "PLATFORM_ERROR");
        assert_eq!(map_http_error(401, "YouTube", "").message, "YouTube returned 401 Unauthorized");
    }

    #[test]
    fn tiktok_failures_reported_with_http_200_are_errors() {
        assert!(tiktok_api_error(&json!({ "error": { "code": "ok", "message": "" } })).is_none());
        assert!(tiktok_api_error(&json!({ "data": {} })).is_none());
        let cases = [
            ("access_token_invalid", "AUTH_FAILED"),
            ("rate_limit_exceeded", "RATE_LIMITED"),
            ("unaudited_client_can_only_post_to_private_accounts", "TIKTOK_APP_UNAUDITED"),
            ("invalid_param", "UPLOAD_FAILED"),
        ];
        for (code, expected) in cases {
            let e = tiktok_api_error(&json!({ "error": { "code": code, "message": "m" } })).unwrap();
            assert_eq!(e.code, expected, "{code}");
        }
    }

    #[test]
    fn upload_addresses_must_stay_on_https_and_known_hosts() {
        assert!(tiktok_upload_url("https://upload.example.com/x?id=1").is_some());
        assert!(tiktok_upload_url("http://upload.example.com/x").is_none());
        assert!(tiktok_upload_url("not a url").is_none());

        assert!(youtube_session_url("https://www.googleapis.com/upload/youtube/v3/videos?upload_id=1").is_some());
        assert!(youtube_session_url("https://youtube.googleapis.com/upload/x").is_some());
        for bad in ["https://evil.example/upload", "http://www.googleapis.com/x", "https://googleapis.com.evil.io/x"] {
            assert!(youtube_session_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn empty_media_is_an_error_not_a_zero_byte_upload() {
        let e = decode_media(&file("a.mp4", "data:video/mp4;base64,", "video/mp4")).unwrap_err();
        assert_eq!(e.code, "EMPTY_FILE");
        assert_eq!(decode_media(&file("a.mp4", "!!!", "video/mp4")).unwrap_err().code, "DECODE_ERROR");
        assert_eq!(decode_media(&file("a.mp4", "data:video/mp4;base64,QUJD", "video/mp4")).unwrap(), b"ABC");
    }

    #[test]
    fn a_run_that_uploaded_nothing_fails_and_a_partial_run_succeeds() {
        let rate = || upload_err("RATE_LIMITED", "slow down", "", "");
        let out = build_output(Platform::TikTok, vec![], vec![("a.mp4".into(), rate()), ("b.mp4".into(), rate())]);
        assert!(!out.success);
        let err = out.error.unwrap();
        assert_eq!(err.code, "RATE_LIMITED");
        assert!(err.recoverable);
        assert!(err.message.contains("a.mp4") && err.message.contains("b.mp4"), "{}", err.message);

        let mixed = build_output(
            Platform::TikTok,
            vec![],
            vec![("a.mp4".into(), rate()), ("b.mp4".into(), upload_err("UPLOAD_FAILED", "no", "", ""))],
        );
        let err = mixed.error.unwrap();
        assert_eq!(err.code, "UPLOAD_FAILED");
        assert!(!err.recoverable);

        let partial = build_output(
            Platform::YouTube,
            vec![json!({ "filename": "a.mp4" })],
            vec![("b.mp4".into(), upload_err("UPLOAD_FAILED", "no", "why", "do this"))],
        );
        assert!(partial.success);
        let body = partial.output.unwrap();
        assert_eq!(body["count"], 1);
        assert_eq!(body["failed"], 1);
        assert_eq!(body["errors"][0]["action"], "do this");
    }

    #[test]
    fn a_single_failure_message_includes_the_explanation_and_action() {
        let err = all_failed_error(&[("a.mp4".to_string(), upload_err("UPLOAD_FAILED", "m", "because", "do this"))]);
        assert_eq!(err.message, "a.mp4: m because do this");
    }

    /// Minimal hand-rolled `Subscriber` that records every event's fields as
    /// one `"name=value "`-pair string per event. `aerini-engine` depends on
    /// `tracing` (Cargo.toml:56) but not `tracing-subscriber`, so this avoids
    /// adding a dev-dependency just to capture a `warn!` line in a test.
    #[derive(Default)]
    struct CapturingSubscriber {
        events: Arc<Mutex<Vec<String>>>,
    }

    struct FieldPrinter<'a> {
        out: &'a mut String,
    }

    impl<'a> Visit for FieldPrinter<'a> {
        fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
            use fmt::Write;
            let _ = write!(self.out, "{}={:?} ", field.name(), value);
        }
    }

    impl tracing::Subscriber for CapturingSubscriber {
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool { true }
        fn new_span(&self, _span: &Attributes<'_>) -> Id { Id::from_u64(1) }
        fn record(&self, _span: &Id, _values: &Record<'_>) {}
        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}
        fn enter(&self, _span: &Id) {}
        fn exit(&self, _span: &Id) {}

        fn event(&self, event: &Event<'_>) {
            let mut line = String::new();
            event.record(&mut FieldPrinter { out: &mut line });
            self.events.lock().unwrap().push(line);
        }
    }

    // The bug this guard exists for: a publish session dropped before
    // disarm() runs (workflow cancellation, panic, early return) must leave
    // a trail, since TikTok has no abort endpoint to call instead.
    #[test]
    fn publish_id_guard_warns_on_drop_when_not_disarmed() {
        let subscriber = CapturingSubscriber::default();
        let events = subscriber.events.clone();

        tracing::subscriber::with_default(subscriber, || {
            let guard = PublishIdGuard::new("pub_123".to_string(), "clip.mp4".to_string());
            drop(guard);
        });

        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1, "expected exactly one warn! event, got {:?}", *events);
        assert!(events[0].contains("pub_123"), "warning did not mention publish_id: {}", events[0]);
        assert!(events[0].contains("clip.mp4"), "warning did not mention filename: {}", events[0]);
    }

    // A disarmed guard (every chunk was accepted) must not warn on drop.
    #[test]
    fn publish_id_guard_silent_on_drop_when_disarmed() {
        let subscriber = CapturingSubscriber::default();
        let events = subscriber.events.clone();

        tracing::subscriber::with_default(subscriber, || {
            let mut guard = PublishIdGuard::new("pub_456".to_string(), "clip2.mp4".to_string());
            guard.disarm();
            drop(guard);
        });

        assert!(events.lock().unwrap().is_empty(), "disarmed guard must not warn on drop");
    }
}
