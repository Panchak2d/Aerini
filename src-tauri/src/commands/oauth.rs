use aerini_engine::nodes::oauth_listener;

/// Reports the OAuth callback port that would be used right now if a social-platform
/// (or Google Sheets) OAuth flow were started this instant — 42069 if free, otherwise
/// whatever OS-assigned port the flow would actually fall back to.
///
/// T2-15/S10-2: `SocialSetupGuide.ts` previously hardcoded the redirect URI as
/// `http://127.0.0.1:42069/callback` with no way to know whether that port would
/// really be used — if 42069 was unavailable when a flow started, the OAuth
/// provider would reject the callback with an opaque redirect_uri mismatch. This
/// command lets the guide show the real, current port instead.
#[tauri::command]
pub async fn get_oauth_redirect_port() -> Result<u16, String> {
    oauth_listener::probe_redirect_port()
        .await
        .map_err(|e| e.message)
}
