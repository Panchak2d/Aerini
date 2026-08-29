use aerini_engine::nodes::oauth_listener;

/// Reports the OAuth callback port that would be used right now if a social-platform
/// (or Google Sheets) OAuth flow were started this instant — 42069 if free, otherwise
/// whatever OS-assigned port the flow would actually fall back to.
///
/// `SocialSetupGuide.ts` otherwise has no way to know whether the redirect
/// URI it displays (`http://127.0.0.1:42069/callback`) will actually be the
/// port used — if 42069 is unavailable when a flow starts, the OAuth
/// provider would reject the callback with an opaque redirect_uri mismatch.
/// This command lets the guide show the real, current port instead.
#[tauri::command]
pub async fn get_oauth_redirect_port() -> Result<u16, String> {
    oauth_listener::probe_redirect_port()
        .await
        .map_err(|e| e.message)
}
