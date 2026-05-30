// OAuth 2.0 token manager for social platform authentication.
//
// Handles:
//   - Token storage via OS keychain (keyring v3)                         — G1
//   - Per-credential refresh mutex (DashMap<key, Arc<Mutex<()>>>)        — G3
//   - Fixed OAuth redirect port 42069, no dynamic fallback               — G6
//   - TCP listener closed on every exit path (defer-style cleanup)       — G10
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

const KEYCHAIN_SERVICE: &str = "flowo-oauth";
const OAUTH_PORT: u16 = 42069;
const REDIRECT_URI: &str = "http://127.0.0.1:42069/callback";
const CALLBACK_TIMEOUT_SECS: u64 = 60;
/// Margin before expiry to trigger proactive refresh (5 minutes).
const EXPIRY_MARGIN_SECS: u64 = 300;
/// Instagram long-lived token lifetime (60 days), stored with a 5-day safety margin.
const INSTAGRAM_LL_EXPIRY_SECS: u64 = 55 * 24 * 3600;
/// TikTok access token lifetime (24 h).
const TIKTOK_ACCESS_EXPIRY_SECS: u64 = 86400;
/// YouTube access token lifetime (1 h).
const YOUTUBE_ACCESS_EXPIRY_SECS: u64 = 3600;

// ── Refresh mutex map (G3) ────────────────────────────────────────────────────

// Key = "{platform}:{client_id}". Ensures only one goroutine refreshes a token
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
    tokio::task::spawn_blocking(move || {
        if let Ok(entry) = keyring::Entry::new(&svc, &user) {
            let _ = entry.set_password(&json);
        }
    })
    .await
    .ok();
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
) -> Result<StoredTokens, NodeError> {
    match platform {
        "youtube" => exchange_code_youtube(code, client_id, client_secret).await,
        "instagram" => exchange_code_instagram(code, client_id, client_secret).await,
        "tiktok" => exchange_code_tiktok(code, client_id, client_secret).await,
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
        "youtube" => refresh_tokens_youtube(client_id, client_secret, stored).await,
        "instagram" => refresh_tokens_instagram(stored).await,
        "tiktok" => refresh_tokens_tiktok(client_id, client_secret, stored).await,
        p => Err(NodeError::unrecoverable("UNKNOWN_PLATFORM", format!("Unknown platform: {}", p))),
    }
}

// ─── YouTube ──────────────────────────────────────────────────────────────────

async fn exchange_code_youtube(
    code: &str,
    client_id: &str,
    client_secret: &str,
) -> Result<StoredTokens, NodeError> {
    let params = [
        ("code", code),
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("redirect_uri", REDIRECT_URI),
        ("grant_type", "authorization_code"),
    ];
    let resp = super::shared_http_client()
        .post("https://oauth2.googleapis.com/token")
        .form(&params)
        .send()
        .await
        .map_err(|e| NodeError::recoverable("OAUTH_NETWORK_ERROR", e.to_string()))?;

    parse_google_token_response(resp).await
}

async fn refresh_tokens_youtube(
    client_id: &str,
    client_secret: &str,
    stored: &StoredTokens,
) -> Result<StoredTokens, NodeError> {
    let refresh_token = stored.refresh_token.as_deref().ok_or_else(|| {
        NodeError::unrecoverable("OAUTH_NO_REFRESH_TOKEN", "No YouTube refresh token stored")
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
        .map_err(|e| NodeError::recoverable("OAUTH_NETWORK_ERROR", e.to_string()))?;

    let mut new_tokens = parse_google_token_response(resp).await?;
    // Google may not return a new refresh token on refresh — keep the old one.
    if new_tokens.refresh_token.is_none() {
        new_tokens.refresh_token = stored.refresh_token.clone();
    }
    Ok(new_tokens)
}

async fn parse_google_token_response(
    resp: reqwest::Response,
) -> Result<StoredTokens, NodeError> {
    let status = resp.status();
    let body: Value = resp
        .json()
        .await
        .map_err(|e| NodeError::unrecoverable("OAUTH_PARSE_ERROR", e.to_string()))?;

    if !status.is_success() {
        let err_desc = body["error_description"]
            .as_str()
            .or_else(|| body["error"].as_str())
            .unwrap_or("Token exchange failed");
        if status.as_u16() == 401 {
            return Err(NodeError::unrecoverable(
                "OAUTH_INVALID_TOKEN",
                format!("Google OAuth: {}. Re-authenticate in node settings.", err_desc),
            ));
        }
        return Err(NodeError::recoverable(
            "OAUTH_TOKEN_EXCHANGE_FAILED",
            format!("Google OAuth error: {}", err_desc),
        ));
    }

    let access_token = body["access_token"]
        .as_str()
        .ok_or_else(|| NodeError::unrecoverable("OAUTH_PARSE_ERROR", "No access_token in response"))?
        .to_string();
    let expires_in = body["expires_in"].as_u64().unwrap_or(YOUTUBE_ACCESS_EXPIRY_SECS);
    let refresh_token = body["refresh_token"].as_str().map(str::to_string);

    Ok(StoredTokens {
        access_token,
        refresh_token,
        expires_at: now_secs() + expires_in,
    })
}

// ─── Instagram ────────────────────────────────────────────────────────────────

async fn exchange_code_instagram(
    code: &str,
    client_id: &str,
    client_secret: &str,
) -> Result<StoredTokens, NodeError> {
    // Step 1: short-lived token
    let params = [
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("grant_type", "authorization_code"),
        ("redirect_uri", REDIRECT_URI),
        ("code", code),
    ];
    let short_resp = super::shared_http_client()
        .post("https://api.instagram.com/oauth/access_token")
        .form(&params)
        .send()
        .await
        .map_err(|e| NodeError::recoverable("OAUTH_NETWORK_ERROR", e.to_string()))?;

    if !short_resp.status().is_success() {
        let body: Value = short_resp.json().await.unwrap_or(Value::Null);
        let msg = body["error_message"].as_str().unwrap_or("Token exchange failed");
        return Err(NodeError::unrecoverable("OAUTH_TOKEN_EXCHANGE_FAILED", format!("Instagram: {}", msg)));
    }
    let short_body: Value = short_resp
        .json()
        .await
        .map_err(|e| NodeError::unrecoverable("OAUTH_PARSE_ERROR", e.to_string()))?;
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
        .map_err(|e| NodeError::recoverable("OAUTH_NETWORK_ERROR", e.to_string()))?;

    if !ll_resp.status().is_success() {
        let body: Value = ll_resp.json().await.unwrap_or(Value::Null);
        let msg = body["error"]["message"].as_str().unwrap_or("Long-lived token exchange failed");
        return Err(NodeError::unrecoverable("OAUTH_TOKEN_EXCHANGE_FAILED", format!("Instagram: {}", msg)));
    }
    let ll_body: Value = ll_resp
        .json()
        .await
        .map_err(|e| NodeError::unrecoverable("OAUTH_PARSE_ERROR", e.to_string()))?;
    let ll_token = ll_body["access_token"]
        .as_str()
        .ok_or_else(|| NodeError::unrecoverable("OAUTH_PARSE_ERROR", "No long-lived access_token from Instagram"))?
        .to_string();

    Ok(StoredTokens {
        access_token: ll_token,
        refresh_token: None,
        expires_at: now_secs() + INSTAGRAM_LL_EXPIRY_SECS,
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
        .map_err(|e| NodeError::recoverable("OAUTH_NETWORK_ERROR", e.to_string()))?;

    if !resp.status().is_success() {
        return Err(NodeError::unrecoverable(
            "OAUTH_REFRESH_FAILED",
            "Instagram token refresh failed — re-authenticate in node settings.",
        ));
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|e| NodeError::unrecoverable("OAUTH_PARSE_ERROR", e.to_string()))?;
    let new_token = body["access_token"]
        .as_str()
        .ok_or_else(|| NodeError::unrecoverable("OAUTH_PARSE_ERROR", "No access_token in Instagram refresh response"))?
        .to_string();

    Ok(StoredTokens {
        access_token: new_token,
        refresh_token: None,
        expires_at: now_secs() + INSTAGRAM_LL_EXPIRY_SECS,
    })
}

// ─── TikTok ───────────────────────────────────────────────────────────────────

async fn exchange_code_tiktok(
    code: &str,
    client_id: &str,
    client_secret: &str,
) -> Result<StoredTokens, NodeError> {
    // TikTok uses client_key instead of client_id in token requests.
    let params = [
        ("client_key", client_id),
        ("client_secret", client_secret),
        ("code", code),
        ("grant_type", "authorization_code"),
        ("redirect_uri", REDIRECT_URI),
    ];
    let resp = super::shared_http_client()
        .post("https://open.tiktokapis.com/v2/oauth/token/")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&params)
        .send()
        .await
        .map_err(|e| NodeError::recoverable("OAUTH_NETWORK_ERROR", e.to_string()))?;

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
        .map_err(|e| NodeError::recoverable("OAUTH_NETWORK_ERROR", e.to_string()))?;

    parse_tiktok_token_response(resp).await
}

async fn parse_tiktok_token_response(
    resp: reqwest::Response,
) -> Result<StoredTokens, NodeError> {
    let status = resp.status();
    let body: Value = resp
        .json()
        .await
        .map_err(|e| NodeError::unrecoverable("OAUTH_PARSE_ERROR", e.to_string()))?;

    if !status.is_success() {
        let msg = body["error_description"]
            .as_str()
            .or_else(|| body["error"].as_str())
            .unwrap_or("Token exchange failed");
        return Err(NodeError::unrecoverable(
            "OAUTH_TOKEN_EXCHANGE_FAILED",
            format!("TikTok OAuth: {}", msg),
        ));
    }

    let access_token = body["access_token"]
        .as_str()
        .ok_or_else(|| NodeError::unrecoverable("OAUTH_PARSE_ERROR", "No access_token in TikTok response"))?
        .to_string();
    let refresh_token = body["refresh_token"].as_str().map(str::to_string);
    let expires_in = body["expires_in"].as_u64().unwrap_or(TIKTOK_ACCESS_EXPIRY_SECS);

    Ok(StoredTokens {
        access_token,
        refresh_token,
        expires_at: now_secs() + expires_in,
    })
}

// ── Auth URL builders ─────────────────────────────────────────────────────────

fn build_auth_url(platform: &str, client_id: &str, state: &str) -> String {
    let redirect = pct_encode(REDIRECT_URI);
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

fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

// ── Local OAuth callback listener (G10: closed on every exit path) ─────────────

/// Binds port 42069. On success: returns the authorization code from the redirect.
/// On any error or timeout: the TcpListener is dropped (closed) before returning.
async fn listen_for_callback(expected_state: &str) -> Result<String, NodeError> {
    let listener = TcpListener::bind(("127.0.0.1", OAUTH_PORT)).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::AddrInUse {
            NodeError::unrecoverable(
                "OAUTH_PORT_CONFLICT",
                format!(
                    "Port {} is in use by another application. \
                    Close it and retry.",
                    OAUTH_PORT
                ),
            )
        } else {
            NodeError::unrecoverable("OAUTH_LISTENER_ERROR", e.to_string())
        }
    })?;

    // `listener` is RAII — dropped (closed) on every return path below. (G10)
    let result = timeout(
        Duration::from_secs(CALLBACK_TIMEOUT_SECS),
        accept_one_callback(&listener, expected_state),
    )
    .await;

    // `listener` drops here in all paths.
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

async fn accept_one_callback(
    listener: &TcpListener,
    expected_state: &str,
) -> Result<String, NodeError> {
    let (mut stream, _) = listener.accept().await.map_err(|e| {
        NodeError::unrecoverable("OAUTH_LISTENER_ERROR", e.to_string())
    })?;

    let mut buf = vec![0u8; 4096];
    let n = stream
        .read(&mut buf)
        .await
        .map_err(|e| NodeError::unrecoverable("OAUTH_LISTENER_ERROR", e.to_string()))?;
    let request = String::from_utf8_lossy(&buf[..n]);

    // Extract query string from "GET /callback?code=xxx&state=yyy HTTP/1.1"
    let params = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|path| path.split_once('?').map(|(_, qs)| qs))
        .unwrap_or("");

    let code = extract_query_param(params, "code")
        .map(|c| c.trim_end_matches("#_").to_string()); // Instagram appends #_ to auth codes
    let state = extract_query_param(params, "state");

    // Always write the success page, even on error, so the browser isn't hung.
    let body = "<html><body style='font-family:sans-serif;padding:2rem'>\
        <h2>Authentication successful</h2>\
        <p>You can close this tab and return to Flowo.</p></body></html>";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=UTF-8\r\n\
        Content-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes()).await;

    if state.as_deref() != Some(expected_state) {
        return Err(NodeError::unrecoverable(
            "OAUTH_STATE_MISMATCH",
            "OAuth state parameter mismatch — possible CSRF. Retry.",
        ));
    }

    code.ok_or_else(|| {
        NodeError::unrecoverable("OAUTH_NO_CODE", "No authorization code in OAuth callback.")
    })
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
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            let h1 = chars.next().unwrap_or('0');
            let h2 = chars.next().unwrap_or('0');
            if let Ok(b) = u8::from_str_radix(&format!("{}{}", h1, h2), 16) {
                out.push(b as char);
            }
        } else if c == '+' {
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out
}

// ── Full OAuth flow ───────────────────────────────────────────────────────────

async fn run_full_oauth_flow(
    platform: &str,
    client_id: &str,
    client_secret: &str,
) -> Result<StoredTokens, NodeError> {
    let state = Uuid::new_v4().to_string();
    let auth_url = build_auth_url(platform, client_id, &state);
    if auth_url.is_empty() {
        return Err(NodeError::unrecoverable(
            "UNKNOWN_PLATFORM",
            format!("Unknown OAuth platform: {}", platform),
        ));
    }

    open_browser(&auth_url);

    let code = listen_for_callback(&state).await.map_err(|e| {
        // Map token-exchange-specific errors cleanly (G10: listener already closed above).
        e
    })?;

    exchange_code(platform, &code, client_id, client_secret)
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
/// 1. Load stored tokens from OS keychain.
/// 2. If access token is valid (not expired): return it.
/// 3. If expired and refresh available: refresh under per-credential mutex (G3).
///    Re-read keychain after acquiring lock — another caller may have already refreshed.
/// 4. If no stored tokens or refresh fails: run full OAuth flow (browser → callback → exchange).
pub async fn get_tokens(
    platform: &str,
    client_id: &str,
    client_secret: &str,
) -> Result<OAuthTokens, NodeError> {
    let now = now_secs();

    // ── 1. Try stored tokens ──────────────────────────────────────────────────
    if let Some(stored) = load_tokens(platform, client_id).await {
        if stored.expires_at > now + EXPIRY_MARGIN_SECS {
            return Ok(OAuthTokens { access_token: stored.access_token });
        }

        // ── 2. Token expired — refresh under mutex (G3) ───────────────────────
        let lock = refresh_lock(platform, client_id);
        let _guard = lock.lock().await;

        // Re-read after acquiring — another waiter may have already refreshed.
        if let Some(fresh) = load_tokens(platform, client_id).await {
            if fresh.expires_at > now + EXPIRY_MARGIN_SECS {
                return Ok(OAuthTokens { access_token: fresh.access_token });
            }
            // Still expired; this holder refreshes.
            match refresh_tokens(platform, client_id, client_secret, &fresh).await {
                Ok(new_tokens) => {
                    save_tokens(platform, client_id, &new_tokens).await;
                    return Ok(OAuthTokens { access_token: new_tokens.access_token });
                }
                Err(_) => {
                    // Refresh failed — fall through to full OAuth flow.
                    clear_tokens(platform, client_id).await;
                }
            }
        }
    }

    // ── 3. No tokens or refresh failed — full OAuth flow ─────────────────────
    let tokens = run_full_oauth_flow(platform, client_id, client_secret).await?;
    save_tokens(platform, client_id, &tokens).await;
    Ok(OAuthTokens { access_token: tokens.access_token })
}
