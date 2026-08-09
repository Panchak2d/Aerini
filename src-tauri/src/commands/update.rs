use serde::{Deserialize, Serialize};
use url::Url;

const REPO_OWNER: &str = "Panchak2d";
const REPO_NAME: &str = "aerini";

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
}

#[derive(Serialize)]
pub struct UpdateCheckResult {
    pub current_version: String,
    pub latest_version: String,
    pub is_newer: bool,
    pub release_url: String,
}

fn parse_semver(raw: &str) -> Option<(u32, u32, u32)> {
    let s = raw.strip_prefix('v').unwrap_or(raw);
    let mut parts = s.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    Some((major, minor, patch))
}

/// User-triggered only — see `docs/security.md`'s update-check exception.
/// Never called automatically; nothing schedules or polls this.
#[tauri::command]
pub async fn check_for_update() -> Result<UpdateCheckResult, String> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("Aerini/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;

    let api_url = format!("https://api.github.com/repos/{REPO_OWNER}/{REPO_NAME}/releases/latest");
    let resp = client
        .get(&api_url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("GitHub returned {}", resp.status()));
    }

    let release: GithubRelease = resp
        .json()
        .await
        .map_err(|e| format!("Unexpected response from GitHub: {e}"))?;

    // GitHub's own API always returns this shape; reject anything else rather
    // than handing the frontend a URL to open that didn't come from GitHub.
    let host_ok = Url::parse(&release.html_url)
        .map(|u| u.scheme() == "https" && u.host_str() == Some("github.com"))
        .unwrap_or(false);
    if !host_ok {
        return Err("Unexpected release URL from GitHub".to_string());
    }

    let current = env!("CARGO_PKG_VERSION");
    let current_v = parse_semver(current)
        .ok_or_else(|| "Could not parse current app version".to_string())?;
    let latest_v = parse_semver(&release.tag_name)
        .ok_or_else(|| format!("Unexpected version format from GitHub: {}", release.tag_name))?;

    Ok(UpdateCheckResult {
        current_version: current.to_string(),
        latest_version: release.tag_name.trim_start_matches('v').to_string(),
        is_newer: latest_v > current_v,
        release_url: release.html_url,
    })
}
