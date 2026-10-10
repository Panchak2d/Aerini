// OAuth 2.0 token manager for social platform authentication.
//
// Handles:
//   - Token storage via OS keychain (keyring v3)
//   - Per-credential refresh mutex (DashMap<key, Arc<Mutex<()>>>)
//   - OAuth redirect port 42069 preferred; falls back to OS-assigned port
//   - `probe_redirect_port()` reports that port ahead of time, without
//     starting a flow, so callers (e.g. the Setup Guide) can show the
//     real redirect URI instead of an unconditional "42069"
//   - TCP listener closed on every exit path (defer-style cleanup)
//   - 60-second callback timeout
//
// VERIFIED endpoints (May 2026):
//   YouTube:
//     auth    https://accounts.google.com/o/oauth2/v2/auth
//     token   https://oauth2.googleapis.com/token
//     scope   https://www.googleapis.com/auth/youtube.upload
//   Instagram:
//     auth    https://api.instagram.com/oauth/authorize
//     token   https://api.instagram.com/oauth/access_token
//     scope   instagram_business_basic,instagram_business_content_publish
//     ll-xchg https://graph.instagram.com/access_token
//     refresh https://graph.instagram.com/refresh_access_token
//   TikTok:
//     auth    https://www.tiktok.com/v2/auth/authorize/
//     token   https://open.tiktokapis.com/v2/oauth/token/
//     scope   video.publish
//   Google Sheets:
//     auth    https://accounts.google.com/o/oauth2/v2/auth
//     token   https://oauth2.googleapis.com/token   (shared with YouTube)
//     scope   https://www.googleapis.com/auth/spreadsheets
//
// `google_sheets.rs` goes through this same store/refresh/full-flow pipeline
// as a fourth platform, "google_sheets", reusing the Google token-exchange and
// refresh functions (they hardcode no scope; only `build_auth_url` does).
//
// Refresh policy: a refresh that fails because the provider rejected the stored
// credential discards it and starts the interactive flow; a refresh that fails
// for any other reason (network, 5xx, 429, unreadable reply) keeps the stored
// tokens and returns the error, so a transient outage never costs a login.

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio::time::timeout;
use uuid::Uuid;

use crate::error::NodeError;

// ── Constants ─────────────────────────────────────────────────────────────────

const KEYCHAIN_SERVICE: &str = "aerini-oauth";
const OAUTH_PORT: u16 = 42069;
const CALLBACK_TIMEOUT_SECS: u64 = 60;
/// Margin before expiry to trigger proactive refresh (5 minutes).
const EXPIRY_MARGIN_SECS: u64 = 300;
/// Instagram long-lived token lifetime (60 days), stored with a 5-day safety margin.
const INSTAGRAM_LL_EXPIRY_SECS: u64 = 55 * 24 * 3600;
/// TikTok access token lifetime (24 h).
const TIKTOK_ACCESS_EXPIRY_SECS: u64 = 86400;
/// Google access token lifetime (1 h) — shared default for youtube and google_sheets,
/// both issued by the same Google OAuth2 token endpoint.
const GOOGLE_ACCESS_EXPIRY_SECS: u64 = 3600;

// ── Refresh mutex map ────────────────────────────────────────────────────

// Key = "{platform}:{client_id}". Ensures only one caller refreshes a token
// at a time; waiters re-read the keychain after acquiring, avoiding redundant
// refresh calls if the leader already completed.
static REFRESH_LOCKS: OnceLock<DashMap<String, Arc<Mutex<()>>>> = OnceLock::new();

fn refresh_locks() -> &'static DashMap<String, Arc<Mutex<()>>> {
    REFRESH_LOCKS.get_or_init(DashMap::new)
}

fn refresh_lock(platform: &str, client_id: &str) -> Arc<Mutex<()>> {
    let key = format!("{}:{}", platform, client_id);
    refresh_locks()
        .entry(key)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

// ── Token storage types ───────────────────────────────────────────────────────

/// Tokens stored as a single JSON blob per credential in the OS keychain.
#[derive(Debug, Serialize, Deserialize)]
struct StoredTokens {
    access_token: String,
    /// None for platforms that don't issue refresh tokens (not currently used).
    refresh_token: Option<String>,
    /// Unix timestamp (seconds) when the access token expires.
    expires_at: u64,
}

/// Resolved access token returned to callers.
#[derive(Clone)]
pub struct OAuthTokens {
    pub access_token: String,
}

// ── Keychain helpers (blocking — wrapped in spawn_blocking) ───────────────────

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

async fn load_tokens(platform: &str, client_id: &str) -> Option<StoredTokens> {
    let svc = KEYCHAIN_SERVICE.to_string();
    let user = format!("{}:{}", platform, client_id);
    tokio::task::spawn_blocking(move || -> Option<StoredTokens> {
        let entry = keyring::Entry::new(&svc, &user).ok()?;
        let json = entry.get_password().ok()?;
        serde_json::from_str(&json).ok()
    })
    .await
    .ok()
    .flatten()
}

async fn save_tokens(platform: &str, client_id: &str, tokens: &StoredTokens) {
    let svc = KEYCHAIN_SERVICE.to_string();
    let user = format!("{}:{}", platform, client_id);
    let json = match serde_json::to_string(tokens) {
        Ok(j) => j,
        Err(_) => return,
    };
    let stored = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let entry = keyring::Entry::new(&svc, &user).map_err(|e| e.to_string())?;
        entry.set_password(&json).map_err(|e| e.to_string())
    })
    .await;
    match stored {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!(
            "OAuth tokens for {platform} could not be saved to the OS keychain ({e}); the next run will have to sign in again"
        ),
        Err(_) => tracing::warn!("OAuth token save task for {platform} did not finish"),
    }
}

pub async fn clear_tokens(platform: &str, client_id: &str) {
    let svc = KEYCHAIN_SERVICE.to_string();
    let user = format!("{}:{}", platform, client_id);
    tokio::task::spawn_blocking(move || {
        if let Ok(entry) = keyring::Entry::new(&svc, &user) {
            let _ = entry.delete_credential();
        }
    })
    .await
    .ok();
}

// ── Platform-specific token exchange & refresh ────────────────────────────────

/// Exchange OAuth authorization code for tokens. Platform-specific.
/// Returns `StoredTokens` ready to persist.
async fn exchange_code(
    platform: &str,
    code: &str,
    client_id: &str,
    client_secret: &str,
    redirect_uri: &str,
) -> Result<StoredTokens, NodeError> {
    match platform {
        // "google_sheets" reuses the youtube fn: both are plain Google OAuth2 token
        // exchange against the same endpoint, with no scope hardcoded in this fn —
        // the scope is chosen per-platform in `build_auth_url`.
        "youtube" | "google_sheets" => {
            exchange_code_youtube(code, client_id, client_secret, redirect_uri).await
        }
        "instagram" => exchange_code_instagram(code, client_id, client_secret, redirect_uri).await,
        "tiktok" => exchange_code_tiktok(code, client_id, client_secret, redirect_uri).await,
        p => Err(NodeError::unrecoverable("UNKNOWN_PLATFORM", format!("Unknown platform: {}", p))),
    }
}

/// Refresh an expired access token. Platform-specific.
/// Returns updated `StoredTokens` ready to persist.
async fn refresh_tokens(
    platform: &str,
    client_id: &str,
    client_secret: &str,
    stored: &StoredTokens,
) -> Result<StoredTokens, NodeError> {
    match platform {
        // Same reuse rationale as exchange_code above.
        "youtube" | "google_sheets" => refresh_tokens_youtube(client_id, client_secret, stored).await,
        "instagram" => refresh_tokens_instagram(stored).await,
        "tiktok" => refresh_tokens_tiktok(client_id, client_secret, stored).await,
        p => Err(NodeError::unrecoverable("UNKNOWN_PLATFORM", format!("Unknown platform: {}", p))),
    }
}

// ─── YouTube (also reused by google_sheets — both are plain Google OAuth2) ────

async fn exchange_code_youtube(
    code: &str,
    client_id: &str,
    client_secret: &str,
    redirect_uri: &str,
) -> Result<StoredTokens, NodeError> {
    let params = [
        ("code", code),
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("redirect_uri", redirect_uri),
        ("grant_type", "authorization_code"),
    ];
    let resp = super::shared_http_client()
        .post("https://oauth2.googleapis.com/token")
        .form(&params)
        .send()
        .await
        .map_err(|e| NodeError::recoverable("OAUTH_NETWORK_ERROR", crate::nodes::util::reqwest_err_msg(&e)))?;

    parse_google_token_response(resp).await
}

async fn refresh_tokens_youtube(
    client_id: &str,
    client_secret: &str,
    stored: &StoredTokens,
) -> Result<StoredTokens, NodeError> {
    let refresh_token = stored.refresh_token.as_deref().ok_or_else(|| {
        NodeError::unrecoverable("OAUTH_NO_REFRESH_TOKEN", "No Google refresh token stored — re-authenticate.")
    })?;
    let params = [
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("refresh_token", refresh_token),
        ("grant_type", "refresh_token"),
    ];
    let resp = super::shared_http_client()
        .post("https://oauth2.googleapis.com/token")
        .form(&params)
        .send()
        .await
        .map_err(|e| NodeError::recoverable("OAUTH_NETWORK_ERROR", crate::nodes::util::reqwest_err_msg(&e)))?;

    let mut new_tokens = parse_google_token_response(resp).await?;
    // Google may not return a new refresh token on refresh — keep the old one.
    if new_tokens.refresh_token.is_none() {
        new_tokens.refresh_token = stored.refresh_token.clone();
    }
    Ok(new_tokens)
}

/// Classifies a failed token-endpoint reply. 429 and 5xx are transient; any
/// other failure (including `invalid_grant` and `invalid_client`) means the
/// provider rejected what was sent, so retrying cannot help.
fn endpoint_failure(provider: &str, status: u16, body: &Value) -> NodeError {
    let desc = body["error_description"]
        .as_str()
        .or_else(|| body["error"].as_str())
        .or_else(|| body["error_message"].as_str())
        .or_else(|| body["error"]["message"].as_str())
        .unwrap_or("request failed");
    let desc: String = desc.chars().take(300).collect();
    let message = format!("{provider} OAuth: {desc}");
    if status == 429 || (500..600).contains(&status) {
        NodeError::recoverable("OAUTH_TOKEN_EXCHANGE_FAILED", message)
    } else {
        NodeError::unrecoverable(
            "OAUTH_TOKEN_EXCHANGE_FAILED",
            format!("{message}. Re-authenticate in node settings."),
        )
    }
}

/// True when the provider refused the stored credential itself, as opposed to
/// the refresh merely not completing.
fn requires_reauth(e: &NodeError) -> bool {
    matches!(e.code.as_str(), "OAUTH_NO_REFRESH_TOKEN") || (e.code == "OAUTH_TOKEN_EXCHANGE_FAILED" && !e.recoverable)
}

/// Reads a token-endpoint reply. A failure status with an unreadable body (an
/// HTML gateway page) is still classified by its status.
async fn read_token_reply(resp: reqwest::Response, provider: &str) -> Result<Value, NodeError> {
    let status = resp.status();
    let parsed = crate::nodes::util::read_json_response_capped(resp).await;
    if !status.is_success() {
        return Err(endpoint_failure(provider, status.as_u16(), &parsed.unwrap_or(Value::Null)));
    }
    parsed.map_err(|e| NodeError::unrecoverable("OAUTH_PARSE_ERROR", e))
}

fn expiry_after(expires_in: u64) -> u64 {
    now_secs().saturating_add(expires_in)
}

async fn parse_google_token_response(
    resp: reqwest::Response,
) -> Result<StoredTokens, NodeError> {
    let body = read_token_reply(resp, "Google").await?;
    let access_token = body["access_token"]
        .as_str()
        .ok_or_else(|| NodeError::unrecoverable("OAUTH_PARSE_ERROR", "No access_token in response"))?
        .to_string();
    let expires_in = body["expires_in"].as_u64().unwrap_or(GOOGLE_ACCESS_EXPIRY_SECS);
    let refresh_token = body["refresh_token"].as_str().map(str::to_string);

    Ok(StoredTokens {
        access_token,
        refresh_token,
        expires_at: expiry_after(expires_in),
    })
}

// ─── Instagram ────────────────────────────────────────────────────────────────

async fn exchange_code_instagram(
    code: &str,
    client_id: &str,
    client_secret: &str,
    redirect_uri: &str,
) -> Result<StoredTokens, NodeError> {
    // Step 1: short-lived token
    let params = [
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("grant_type", "authorization_code"),
        ("redirect_uri", redirect_uri),
        ("code", code),
    ];
    let short_resp = super::shared_http_client()
        .post("https://api.instagram.com/oauth/access_token")
        .form(&params)
        .send()
        .await
        .map_err(|e| NodeError::recoverable("OAUTH_NETWORK_ERROR", crate::nodes::util::reqwest_err_msg(&e)))?;

    let short_body = read_token_reply(short_resp, "Instagram").await?;
    let short_token = short_body["access_token"]
        .as_str()
        .ok_or_else(|| NodeError::unrecoverable("OAUTH_PARSE_ERROR", "No access_token in Instagram response"))?;

    // Step 2: exchange for long-lived token (60-day lifetime)
    let ll_resp = super::shared_http_client()
        .get("https://graph.instagram.com/access_token")
        .query(&[
            ("grant_type", "ig_exchange_token"),
            ("client_secret", client_secret),
            ("access_token", short_token),
        ])
        .send()
        .await
        .map_err(|e| NodeError::recoverable("OAUTH_NETWORK_ERROR", crate::nodes::util::reqwest_err_msg(&e)))?;

    let ll_body = read_token_reply(ll_resp, "Instagram").await?;
    let ll_token = ll_body["access_token"]
        .as_str()
        .ok_or_else(|| NodeError::unrecoverable("OAUTH_PARSE_ERROR", "No long-lived access_token from Instagram"))?
        .to_string();

    Ok(StoredTokens {
        access_token: ll_token,
        refresh_token: None,
        expires_at: expiry_after(INSTAGRAM_LL_EXPIRY_SECS),
    })
}

async fn refresh_tokens_instagram(stored: &StoredTokens) -> Result<StoredTokens, NodeError> {
    let resp = super::shared_http_client()
        .get("https://graph.instagram.com/refresh_access_token")
        .query(&[
            ("grant_type", "ig_refresh_token"),
            ("access_token", &stored.access_token),
        ])
        .send()
        .await
        .map_err(|e| NodeError::recoverable("OAUTH_NETWORK_ERROR", crate::nodes::util::reqwest_err_msg(&e)))?;

    let body = read_token_reply(resp, "Instagram").await?;
    let new_token = body["access_token"]
        .as_str()
        .ok_or_else(|| NodeError::unrecoverable("OAUTH_PARSE_ERROR", "No access_token in Instagram refresh response"))?
        .to_string();

    Ok(StoredTokens {
        access_token: new_token,
        refresh_token: None,
        expires_at: expiry_after(INSTAGRAM_LL_EXPIRY_SECS),
    })
}

// ─── TikTok ───────────────────────────────────────────────────────────────────

async fn exchange_code_tiktok(
    code: &str,
    client_id: &str,
    client_secret: &str,
    redirect_uri: &str,
) -> Result<StoredTokens, NodeError> {
    // TikTok uses client_key instead of client_id in token requests.
    let params = [
        ("client_key", client_id),
        ("client_secret", client_secret),
        ("code", code),
        ("grant_type", "authorization_code"),
        ("redirect_uri", redirect_uri),
    ];
    let resp = super::shared_http_client()
        .post("https://open.tiktokapis.com/v2/oauth/token/")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&params)
        .send()
        .await
        .map_err(|e| NodeError::recoverable("OAUTH_NETWORK_ERROR", crate::nodes::util::reqwest_err_msg(&e)))?;

    parse_tiktok_token_response(resp).await
}

async fn refresh_tokens_tiktok(
    client_id: &str,
    client_secret: &str,
    stored: &StoredTokens,
) -> Result<StoredTokens, NodeError> {
    let refresh_token = stored.refresh_token.as_deref().ok_or_else(|| {
        NodeError::unrecoverable("OAUTH_NO_REFRESH_TOKEN", "No TikTok refresh token stored — re-authenticate.")
    })?;
    let params = [
        ("client_key", client_id),
        ("client_secret", client_secret),
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
    ];
    let resp = super::shared_http_client()
        .post("https://open.tiktokapis.com/v2/oauth/token/")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&params)
        .send()
        .await
        .map_err(|e| NodeError::recoverable("OAUTH_NETWORK_ERROR", crate::nodes::util::reqwest_err_msg(&e)))?;

    parse_tiktok_token_response(resp).await
}

async fn parse_tiktok_token_response(
    resp: reqwest::Response,
) -> Result<StoredTokens, NodeError> {
    let status = resp.status().as_u16();
    let body = read_token_reply(resp, "TikTok").await?;

    // The endpoint documents its error body but not an HTTP status for it, so
    // a reply without an access token is a failure whatever the status was.
    let Some(access_token) = body["access_token"].as_str().filter(|t| !t.is_empty()) else {
        return Err(endpoint_failure("TikTok", status.max(400), &body));
    };
    let refresh_token = body["refresh_token"].as_str().map(str::to_string);
    let expires_in = body["expires_in"].as_u64().unwrap_or(TIKTOK_ACCESS_EXPIRY_SECS);

    Ok(StoredTokens {
        access_token: access_token.to_string(),
        refresh_token,
        expires_at: expiry_after(expires_in),
    })
}

// ── Auth URL builders ─────────────────────────────────────────────────────────

fn build_auth_url(platform: &str, client_id: &str, state: &str, redirect_uri: &str) -> String {
    let redirect = pct_encode(redirect_uri);
    let client_id = pct_encode(client_id);
    let state = pct_encode(state);
    match platform {
        "youtube" => format!(
            "https://accounts.google.com/o/oauth2/v2/auth\
            ?client_id={client_id}\
            &redirect_uri={redirect}\
            &response_type=code\
            &scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fyoutube.upload\
            &access_type=offline\
            &prompt=consent\
            &state={state}"
        ),
        "instagram" => format!(
            "https://api.instagram.com/oauth/authorize\
            ?client_id={client_id}\
            &redirect_uri={redirect}\
            &response_type=code\
            &scope=instagram_business_basic%2Cinstagram_business_content_publish\
            &state={state}"
        ),
        "tiktok" => format!(
            "https://www.tiktok.com/v2/auth/authorize/\
            ?client_key={client_id}\
            &redirect_uri={redirect}\
            &response_type=code\
            &scope=video.publish\
            &state={state}"
        ),
        "google_sheets" => format!(
            "https://accounts.google.com/o/oauth2/v2/auth\
            ?client_id={client_id}\
            &redirect_uri={redirect}\
            &response_type=code\
            &scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fspreadsheets\
            &access_type=offline\
            &prompt=consent\
            &state={state}"
        ),
        _ => String::new(),
    }
}

fn pct_encode(s: &str) -> String {
    s.bytes().fold(String::new(), |mut acc, b| {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                acc.push(b as char);
            }
            _ => acc.push_str(&format!("%{:02X}", b)),
        }
        acc
    })
}

// ── Browser launch (no new deps — std::process::Command) ─────────────────────

/// Opens `url` in the default browser. The Windows launcher is `rundll32`, not
/// `cmd /C start`: cmd treats the `&` between query parameters as a command
/// separator and would cut the URL after the first parameter.
fn open_browser(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = std::process::Command::new("rundll32");
        c.args(["url.dll,FileProtocolHandler", url]);
        c
    };
    #[cfg(target_os = "linux")]
    let mut command = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    return Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "no browser launcher for this platform"));

    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    {
        let mut child = command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
}

// ── Local OAuth callback listener (closed on every exit path) ─────────────

/// Waits for the OAuth callback on an already-bound `TcpListener`.
/// The caller owns the listener; it is dropped when this function returns.
async fn listen_for_callback(
    listener: &TcpListener,
    expected_state: &str,
) -> Result<String, NodeError> {
    let result = timeout(
        Duration::from_secs(CALLBACK_TIMEOUT_SECS),
        accept_callback(listener, expected_state),
    )
    .await;

    match result {
        Ok(inner) => inner,
        Err(_) => Err(NodeError::unrecoverable(
            "OAUTH_TIMEOUT",
            format!(
                "No OAuth callback received within {} seconds. \
                Try again.",
                CALLBACK_TIMEOUT_SECS
            ),
        )),
    }
}

/// Keeps accepting connections until one carries a valid callback. Anything
/// else (a favicon request, a port scan, a request with the wrong `state`)
/// gets an error page and is ignored, so a stray connection can neither end
/// the login nor be mistaken for it; only the overall timeout ends the wait.
async fn accept_callback(
    listener: &TcpListener,
    expected_state: &str,
) -> Result<String, NodeError> {
    loop {
        let (mut stream, _) = listener.accept().await.map_err(|e| {
            NodeError::unrecoverable("OAUTH_LISTENER_ERROR", e.to_string())
        })?;
        let request = match timeout(Duration::from_secs(5), read_request_head(&mut stream)).await {
            Ok(Some(r)) => r,
            _ => continue,
        };
        let (status_line, page, outcome) = classify_callback(&request, expected_state);
        let response = format!(
            "HTTP/1.1 {status_line}\r\nContent-Type: text/html; charset=UTF-8\r\n\
            Content-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{}",
            page.len(),
            page
        );
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.shutdown().await;
        if let Some(result) = outcome {
            return result;
        }
    }
}

/// Reads up to the end of the HTTP request head, capped at 16 KB (no
/// legitimate OAuth redirect needs more). `None` when nothing usable arrived.
async fn read_request_head(stream: &mut tokio::net::TcpStream) -> Option<String> {
    const MAX_HEADER_BYTES: usize = 16 * 1024;
    let mut buf = Vec::with_capacity(4096);
    let mut tmp = [0u8; 4096];
    loop {
        let n = stream.read(&mut tmp).await.ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() >= MAX_HEADER_BYTES {
            break;
        }
    }
    if buf.is_empty() {
        None
    } else {
        Some(String::from_utf8_lossy(&buf).into_owned())
    }
}

type CallbackDecision = (&'static str, &'static str, Option<Result<String, NodeError>>);

const PAGE_OK: &str = "<html><body style='font-family:sans-serif;padding:2rem'>\
    <h2>Authentication successful</h2>\
    <p>You can close this tab and return to Aerini.</p></body></html>";
const PAGE_FAILED: &str = "<html><body style='font-family:sans-serif;padding:2rem'>\
    <h2>Authentication failed</h2>\
    <p>Return to Aerini for details and try again.</p></body></html>";
const PAGE_NOT_FOUND: &str = "<html><body>Not found</body></html>";

/// Decides what to answer a request on the callback port and whether it ends
/// the wait. Only a `GET /callback` whose `state` matches can end it.
fn classify_callback(request: &str, expected_state: &str) -> CallbackDecision {
    let mut parts = request.lines().next().unwrap_or("").split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if method != "GET" || path != "/callback" {
        return ("404 Not Found", PAGE_NOT_FOUND, None);
    }

    let state_matches = extract_query_param(query, "state")
        .is_some_and(|s| constant_time_eq(&s, expected_state));
    if !state_matches {
        return ("400 Bad Request", PAGE_FAILED, None);
    }

    if let Some(error) = extract_query_param(query, "error") {
        let detail = extract_query_param(query, "error_description").unwrap_or_default();
        let message: String = format!("The provider reported '{error}'. {detail}").chars().take(300).collect();
        return (
            "200 OK",
            PAGE_FAILED,
            Some(Err(NodeError::unrecoverable("OAUTH_DENIED", message.trim().to_string()))),
        );
    }

    match extract_query_param(query, "code") {
        // Instagram appends #_ to auth codes.
        Some(code) if !code.is_empty() => (
            "200 OK",
            PAGE_OK,
            Some(Ok(code.trim_end_matches("#_").to_string())),
        ),
        _ => (
            "200 OK",
            PAGE_FAILED,
            Some(Err(NodeError::unrecoverable(
                "OAUTH_NO_CODE",
                "No authorization code in OAuth callback.",
            ))),
        ),
    }
}

/// Constant-time equality check, used to compare the OAuth CSRF `state` value
/// so the comparison can't leak information via a short-circuiting `==`.
fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn extract_query_param(qs: &str, key: &str) -> Option<String> {
    for pair in qs.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                return Some(pct_decode(v));
            }
        }
    }
    None
}

fn pct_decode(s: &str) -> String {
    let raw = s.as_bytes();
    let mut bytes: Vec<u8> = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        match raw[i] {
            b'%' => {
                let escaped = raw
                    .get(i + 1..i + 3)
                    .and_then(|h| std::str::from_utf8(h).ok())
                    .and_then(|h| u8::from_str_radix(h, 16).ok());
                match escaped {
                    Some(b) => {
                        bytes.push(b);
                        i += 3;
                    }
                    None => {
                        bytes.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                bytes.push(b' ');
                i += 1;
            }
            b => {
                bytes.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

// ── Full OAuth flow ───────────────────────────────────────────────────────────

/// Binds the OAuth callback listener: preferred port first, falling back to any
/// OS-assigned port on conflict. Shared by `run_full_oauth_flow` (which keeps
/// and uses the listener) and `probe_redirect_port` (which reads the port and drops
/// it immediately) so the fallback policy is defined in exactly one place.
async fn bind_oauth_listener() -> Result<TcpListener, NodeError> {
    match TcpListener::bind(("127.0.0.1", OAUTH_PORT)).await {
        Ok(l) => Ok(l),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            TcpListener::bind(("127.0.0.1", 0)).await.map_err(|e2| {
                NodeError::unrecoverable("OAUTH_LISTENER_ERROR", e2.to_string())
            })
        }
        Err(e) => Err(NodeError::unrecoverable("OAUTH_LISTENER_ERROR", e.to_string())),
    }
}

/// Reports the OAuth callback port that would be used right now if a flow were
/// started this instant — `OAUTH_PORT` (42069) if free, otherwise whatever
/// OS-assigned port `bind_oauth_listener` would fall back to. Binds and immediately
/// drops the listener; there is an inherent, small TOCTOU race against whatever binds
/// next (another process, or a real flow started moments later) — same caveat as any
/// "is this port free" probe. Used by the Setup Guide so the redirect
/// URI a user is told to register reflects live reality instead of an unconditional
/// "42069" that silently stops matching once that port is ever unavailable.
pub async fn probe_redirect_port() -> Result<u16, NodeError> {
    let listener = bind_oauth_listener().await?;
    listener
        .local_addr()
        .map(|addr| addr.port())
        .map_err(|e| NodeError::unrecoverable("OAUTH_LISTENER_ERROR", e.to_string()))
}

async fn run_full_oauth_flow(
    platform: &str,
    client_id: &str,
    client_secret: &str,
) -> Result<StoredTokens, NodeError> {
    let listener = bind_oauth_listener().await?;

    let actual_port = listener
        .local_addr()
        .map_err(|e| NodeError::unrecoverable("OAUTH_LISTENER_ERROR", e.to_string()))?
        .port();

    let redirect_uri = format!("http://127.0.0.1:{}/callback", actual_port);

    let state = Uuid::new_v4().to_string();
    let auth_url = build_auth_url(platform, client_id, &state, &redirect_uri);
    if auth_url.is_empty() {
        return Err(NodeError::unrecoverable(
            "UNKNOWN_PLATFORM",
            format!("Unknown OAuth platform: {}", platform),
        ));
    }

    if let Err(e) = open_browser(&auth_url) {
        return Err(NodeError::unrecoverable(
            "OAUTH_BROWSER_UNAVAILABLE",
            format!(
                "Could not open a browser on this machine ({e}). Sign in from a machine with a desktop and a browser."
            ),
        ));
    }

    let code = listen_for_callback(&listener, &state).await?;
    drop(listener);

    exchange_code(platform, &code, client_id, client_secret, &redirect_uri)
        .await
        .map_err(|e| {
            NodeError::unrecoverable(
                "OAUTH_TOKEN_EXCHANGE_FAILED",
                format!("OAuth token exchange failed: {}", e.message),
            )
        })
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Obtain a valid access token for the given platform credential.
///
/// Flow:
/// 1. Return the stored access token if it is not near expiry.
/// 2. Otherwise take the per-credential lock and re-read the keychain, since
///    another caller may have refreshed or signed in while this one waited.
/// 3. Refresh if a stored token exists. A transient failure returns the error
///    and keeps the stored tokens; a rejected credential discards them.
/// 4. With no usable tokens, run the full OAuth flow (browser → callback →
///    exchange) while still holding the lock, so concurrent callers share one
///    sign-in instead of each opening a browser.
pub async fn get_tokens(
    platform: &str,
    client_id: &str,
    client_secret: &str,
) -> Result<OAuthTokens, NodeError> {
    let fresh_enough = |t: &StoredTokens| t.expires_at > now_secs().saturating_add(EXPIRY_MARGIN_SECS);

    if let Some(stored) = load_tokens(platform, client_id).await {
        if fresh_enough(&stored) {
            return Ok(OAuthTokens { access_token: stored.access_token });
        }
    }

    let lock = refresh_lock(platform, client_id);
    let _guard = lock.lock().await;

    if let Some(stored) = load_tokens(platform, client_id).await {
        if fresh_enough(&stored) {
            return Ok(OAuthTokens { access_token: stored.access_token });
        }
        match refresh_tokens(platform, client_id, client_secret, &stored).await {
            Ok(new_tokens) => {
                save_tokens(platform, client_id, &new_tokens).await;
                return Ok(OAuthTokens { access_token: new_tokens.access_token });
            }
            Err(e) if !requires_reauth(&e) => return Err(e),
            Err(_) => clear_tokens(platform, client_id).await,
        }
    }

    let tokens = run_full_oauth_flow(platform, client_id, client_secret).await?;
    save_tokens(platform, client_id, &tokens).await;
    Ok(OAuthTokens { access_token: tokens.access_token })
}

// ── Tests ───────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn constant_time_eq_matches_for_equal_strings() {
        assert!(constant_time_eq("abc123", "abc123"));
    }

    #[test]
    fn constant_time_eq_rejects_different_length() {
        assert!(!constant_time_eq("abc", "abcd"));
    }

    #[test]
    fn constant_time_eq_rejects_same_length_different_content() {
        assert!(!constant_time_eq("abc123", "abc124"));
    }

    #[test]
    fn build_auth_url_google_sheets_has_correct_scope_and_params() {
        let url = build_auth_url("google_sheets", "abc123", "state-xyz", "http://127.0.0.1:42069/callback");
        assert!(url.starts_with("https://accounts.google.com/o/oauth2/v2/auth"));
        assert!(url.contains("client_id=abc123"));
        assert!(url.contains("state=state-xyz"));
        assert!(url.contains("scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fspreadsheets"));
        // redirect_uri must be percent-encoded, not embedded raw (would break query parsing).
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A42069%2Fcallback"));
        assert!(!url.contains("127.0.0.1:42069/callback&")); // raw, unencoded form must not appear
    }

    #[test]
    fn build_auth_url_unknown_platform_still_empty() {
        // Guards the "unrecognized platform" fallback that run_full_oauth_flow's
        // own empty-string check relies on.
        assert_eq!(build_auth_url("bogus", "id", "state", "http://x/callback"), "");
    }

    #[tokio::test]
    async fn probe_redirect_port_falls_back_then_recovers() {
        // Force the conflict ourselves (rather than assuming 42069 happens to be
        // free or occupied in whatever environment runs this test) so both the
        // fallback and normal-case behavior are deterministic, not environment-
        // dependent. This is the only test in the crate that touches port 42069
        // (grep-confirmed at write time) so it doesn't race a sibling test.
        let held = TcpListener::bind(("127.0.0.1", OAUTH_PORT))
            .await
            .expect("test setup: must be able to bind 42069 when no other listener holds it");

        // Edge case: preferred port is taken — probe must fall back, not error.
        let fallback_port = probe_redirect_port()
            .await
            .expect("probe should fall back to an OS-assigned port, not fail");
        assert_ne!(fallback_port, OAUTH_PORT, "must not report the port it couldn't actually bind");
        assert_ne!(fallback_port, 0, "must report the real assigned port, not the wildcard");

        drop(held);

        // Normal case: preferred port is free again — probe must report it.
        let preferred_port = probe_redirect_port()
            .await
            .expect("probe should succeed once 42069 is free");
        assert_eq!(preferred_port, OAUTH_PORT);
    }

    fn is_ok_with(decision: &CallbackDecision, code: &str) -> bool {
        matches!(&decision.2, Some(Ok(c)) if c == code) && decision.0.starts_with("200")
    }

    #[test]
    fn only_a_get_callback_with_the_right_state_can_end_the_wait() {
        let req = |line: &str| format!("{line} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");

        assert!(is_ok_with(&classify_callback(&req("GET /callback?code=abc%2F1&state=S1"), "S1"), "abc/1"));
        assert!(is_ok_with(&classify_callback(&req("GET /callback?state=S1&code=x%23_"), "S1"), "x"));

        let ignored = [
            "GET /favicon.ico",
            "POST /callback?code=abc&state=S1",
            "GET /callback?code=abc&state=WRONG",
            "GET /callback?code=abc",
            "GET /callback?error=access_denied&state=WRONG",
            "",
        ];
        for line in ignored {
            let (status, _, outcome) = classify_callback(&req(line), "S1");
            assert!(outcome.is_none(), "{line:?} must not end the wait");
            assert!(status.starts_with('4'), "{line:?} -> {status}");
        }

        let denied = classify_callback(&req("GET /callback?error=access_denied&error_description=User+said+no&state=S1"), "S1");
        let err = denied.2.unwrap().unwrap_err();
        assert_eq!(err.code, "OAUTH_DENIED");
        assert!(err.message.contains("access_denied") && err.message.contains("User said no"), "{}", err.message);
        assert_eq!(denied.1, PAGE_FAILED, "a refused login must not show the success page");

        let no_code = classify_callback(&req("GET /callback?state=S1"), "S1");
        assert_eq!(no_code.2.unwrap().unwrap_err().code, "OAUTH_NO_CODE");
    }

    async fn send(port: u16, request: &str) -> String {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut out = String::new();
        stream.read_to_string(&mut out).await.unwrap();
        out
    }

    #[tokio::test]
    async fn stray_connections_do_not_consume_the_single_callback() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let waiting = tokio::spawn(async move { accept_callback(&listener, "S1").await });

        let favicon = send(port, "GET /favicon.ico HTTP/1.1\r\n\r\n").await;
        assert!(favicon.starts_with("HTTP/1.1 404"), "{favicon}");
        let forged = send(port, "GET /callback?code=evil&state=guess HTTP/1.1\r\n\r\n").await;
        assert!(forged.starts_with("HTTP/1.1 400") && forged.contains("Authentication failed"), "{forged}");
        let silent = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        drop(silent);
        assert!(!waiting.is_finished(), "the wait must survive stray requests");

        let real = send(port, "GET /callback?code=GOOD&state=S1 HTTP/1.1\r\n\r\n").await;
        assert!(real.starts_with("HTTP/1.1 200") && real.contains("Authentication successful"), "{real}");
        assert_eq!(waiting.await.unwrap().unwrap(), "GOOD");
    }

    #[test]
    fn token_endpoint_failures_are_classified_by_status_and_reauth_is_narrow() {
        let body = json!({ "error": "invalid_grant", "error_description": "Token has been revoked." });
        let cases = [
            (400, true),
            (401, true),
            (403, true),
            (429, false),
            (500, false),
            (503, false),
        ];
        for (status, reauth) in cases {
            let e = endpoint_failure("Google", status, &body);
            assert_eq!(e.recoverable, !reauth, "status {status}");
            assert_eq!(requires_reauth(&e), reauth, "status {status}");
            assert!(e.message.contains("Token has been revoked."));
        }
        assert!(endpoint_failure("Google", 400, &body).message.contains("Re-authenticate"));
        assert!(endpoint_failure("Instagram", 502, &Value::Null).message.contains("request failed"));
        let ig = json!({ "error": { "message": "Invalid OAuth access token", "code": 190 } });
        assert!(endpoint_failure("Instagram", 400, &ig).message.contains("Invalid OAuth access token"));

        for code in ["OAUTH_PARSE_ERROR", "OAUTH_NETWORK_ERROR", "OAUTH_LISTENER_ERROR"] {
            assert!(!requires_reauth(&NodeError::unrecoverable(code, "x")), "{code} must keep the stored tokens");
        }
        assert!(requires_reauth(&NodeError::unrecoverable("OAUTH_NO_REFRESH_TOKEN", "x")));
    }

    #[test]
    fn percent_decoding_keeps_malformed_escapes_literal() {
        let cases = [
            ("a%2Fb", "a/b"),
            ("a+b", "a b"),
            ("100%", "100%"),
            ("%zz", "%zz"),
            ("%4", "%4"),
            ("%E2%9C%93", "\u{2713}"),
        ];
        for (input, expected) in cases {
            assert_eq!(pct_decode(input), expected, "input: {input:?}");
        }
    }

    #[test]
    fn auth_url_encodes_client_id_so_it_cannot_inject_parameters() {
        let url = build_auth_url("youtube", "id&scope=evil#frag", "s", "http://127.0.0.1:1/callback");
        assert!(url.contains("client_id=id%26scope%3Devil%23frag&"), "{url}");
        assert_eq!(url.matches("scope=").count(), 1);
    }

    #[test]
    fn expiry_arithmetic_saturates_on_hostile_lifetimes() {
        assert_eq!(expiry_after(u64::MAX), u64::MAX);
    }
}
