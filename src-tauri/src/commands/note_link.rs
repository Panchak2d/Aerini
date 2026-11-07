use tauri_plugin_opener::OpenerExt;

const MAX_URL_BYTES: usize = 2048;

fn strip_scheme(raw: &str) -> Option<&str> {
    ["https://", "http://"]
        .into_iter()
        .find(|scheme| raw.len() >= scheme.len() && raw[..scheme.len()].eq_ignore_ascii_case(scheme))
        .map(|scheme| &raw[scheme.len()..])
}

fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && !host.starts_with(['.', '-'])
        && !host.ends_with(['.', '-'])
        && host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
}

fn valid_port(port: &str) -> bool {
    !port.is_empty() && port.len() <= 5 && port.bytes().all(|b| b.is_ascii_digit()) && port.parse::<u16>().is_ok()
}

fn valid_authority(authority: &str) -> bool {
    if let Some(rest) = authority.strip_prefix('[') {
        let Some((literal, after)) = rest.split_once(']') else { return false };
        let literal_ok = !literal.is_empty() && literal.bytes().all(|b| b.is_ascii_hexdigit() || b == b':' || b == b'.');
        let port_ok = after.is_empty() || after.strip_prefix(':').is_some_and(valid_port);
        return literal_ok && port_ok;
    }
    match authority.split_once(':') {
        Some((host, port)) => valid_host(host) && valid_port(port),
        None => valid_host(authority),
    }
}

/// Accepts only a plain http(s) address: ASCII, no user name or password, and a
/// host made of letters, digits, dots and hyphens (or an IP literal). The
/// webview validates and normalizes first; this check does not rely on that.
pub(crate) fn validate_note_link(raw: &str) -> Result<&str, &'static str> {
    if raw.len() > MAX_URL_BYTES {
        return Err("The link is too long to open.");
    }
    if !raw.bytes().all(|b| b.is_ascii_graphic()) || raw.contains('\\') {
        return Err("The link contains characters that are not allowed.");
    }
    let rest = strip_scheme(raw).ok_or("Only http and https links can be opened.")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    if authority.contains('@') {
        return Err("Links with a user name or password are not opened.");
    }
    if !valid_authority(authority) {
        return Err("The link does not have a valid address.");
    }
    Ok(raw)
}

/// Opens a link from a note in the default browser after the webview has shown
/// the full address and the person has confirmed.
#[tauri::command]
pub fn open_note_link(app: tauri::AppHandle, url: String) -> Result<(), String> {
    let url = validate_note_link(&url).map_err(str::to_string)?;
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plain_http_and_https_links() {
        for url in [
            "https://example.com",
            "https://example.com/path?q=1#frag",
            "HTTP://Example.com/",
            "http://localhost:8080/x",
            "https://[::1]:443/",
            "https://203.0.113.7/a@b",
        ] {
            assert_eq!(validate_note_link(url), Ok(url), "{url}");
        }
    }

    #[test]
    fn rejects_other_schemes() {
        for url in ["javascript:alert(1)", "file:///etc/passwd", "ftp://example.com", "data:text/html,x", "example.com", ""] {
            assert!(validate_note_link(url).is_err(), "{url}");
        }
    }

    #[test]
    fn rejects_credentials_and_malformed_hosts() {
        for url in [
            "https://user:pw@example.com/",
            "https://user@example.com/",
            "https://",
            "https:///path",
            "https://exa mple.com",
            "https://example.com:99999/",
            "https://example.com:/",
            "https://-example.com/",
            "https://example.com./x\\y",
            "https://exämple.com/",
            "https://exa%6Dple.com/",
            "https://[]/",
        ] {
            assert!(validate_note_link(url).is_err(), "{url}");
        }
    }

    #[test]
    fn rejects_overlong_links() {
        let url = format!("https://example.com/{}", "a".repeat(MAX_URL_BYTES));
        assert!(validate_note_link(&url).is_err());
    }
}
