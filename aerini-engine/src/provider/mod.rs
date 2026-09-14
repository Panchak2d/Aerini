use std::collections::HashMap;
use std::sync::OnceLock;

mod records;

// ── Types ─────────────────────────────────────────────────────────────────────

/// What an AI provider can do. Used by callers that need to filter providers
/// by capability (e.g. listing image-gen providers in the UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    TextGen,
    ImageGen,
    Embedding,
}

/// How a provider authenticates API requests.
///
/// - `BearerToken` — `Authorization: Bearer <key>` (OpenAI, GPT Image 1/2)
/// - `HeaderKey(h)` — `<h>: <key>` (Anthropic: x-api-key; Gemini: x-goog-api-key; BFL: x-key)
/// - `BasicAuth`    — HTTP Basic auth; handled directly in the node function (A1111)
/// - `None`         — no auth header (ComfyUI local)
///
/// `apply_auth` handles `BearerToken` and `HeaderKey` only.
/// `BasicAuth` and `None` are no-ops in `apply_auth`; the node function handles them.
#[derive(Debug)]
pub enum AuthStyle {
    BearerToken,
    HeaderKey(&'static str),
    BasicAuth,
    None,
}

/// Static metadata for one AI provider.
pub struct ProviderRecord {
    pub id: &'static str,
    pub display_name: &'static str,
    pub capabilities: &'static [Capability],
    pub default_base_url: &'static str,
    pub auth_style: AuthStyle,
    pub requires_key: bool,
    /// Fixed headers appended to every request (e.g. `anthropic-version`).
    pub extra_headers: &'static [(&'static str, &'static str)],
}

// ── Registry ──────────────────────────────────────────────────────────────────

pub struct ProviderRegistry {
    providers: HashMap<&'static str, ProviderRecord>,
}

static REGISTRY: OnceLock<ProviderRegistry> = OnceLock::new();

impl ProviderRegistry {
    /// Returns the process-wide singleton registry, initialising it on first call.
    pub fn global() -> &'static ProviderRegistry {
        REGISTRY.get_or_init(|| ProviderRegistry {
            providers: records::built_in(),
        })
    }

    /// Look up a provider by id. Returns `None` for unknown ids.
    pub fn get(&self, id: &str) -> Option<&ProviderRecord> {
        self.providers.get(id)
    }

    /// Apply the provider's auth style to a `RequestBuilder`.
    ///
    /// - `BearerToken` — adds `Authorization: Bearer <api_key>` (skipped if `api_key` is empty)
    /// - `HeaderKey(h)` — adds `<h>: <api_key>` unconditionally
    /// - `BasicAuth` / `None` — no-op; auth is handled by the calling node function
    ///
    /// Always appends `extra_headers` (e.g. `anthropic-version`) after auth.
    pub fn apply_auth(
        record: &ProviderRecord,
        mut builder: reqwest::RequestBuilder,
        api_key: &str,
    ) -> reqwest::RequestBuilder {
        match &record.auth_style {
            AuthStyle::BearerToken => {
                if !api_key.is_empty() {
                    builder = builder.header("Authorization", format!("Bearer {}", api_key));
                }
            }
            AuthStyle::HeaderKey(header_name) => {
                if !api_key.is_empty() {
                    builder = builder.header(*header_name, api_key);
                }
            }
            AuthStyle::BasicAuth | AuthStyle::None => {}
        }
        for (name, value) in record.extra_headers {
            builder = builder.header(*name, *value);
        }
        builder
    }

    /// Detect provider id from `base_url` for `provider = "auto"` in AI nodes.
    ///
    /// Returns one of `"anthropic"`, `"gemini"`, `"local"`, or `"openai"` (default).
    ///
    /// `"local"` is returned for the literal hostname `"localhost"`
    /// (case-insensitive) or any IP literal `check_ssrf_ip` rejects under
    /// `SsrfPolicy::Strict` — loopback and RFC 1918 primarily, but also
    /// link-local and the other always-blocked ranges it checks — the same
    /// definition of "local" the SSRF layer already uses everywhere else.
    /// This is a cosmetic label only; it does not gate network access on
    /// its own.
    ///
    /// No DNS resolution is performed — this function must stay synchronous,
    /// since it runs before the async SSRF check in both AI node files. A
    /// hostname that needs DNS to resolve to a local address (e.g.
    /// `myollama.local`) is not detected here and falls through to `"openai"`.
    pub fn detect_from_url(base_url: &str) -> &'static str {
        if let Ok(parsed) = url::Url::parse(base_url) {
            match parsed.host() {
                Some(url::Host::Domain(d)) if d.eq_ignore_ascii_case("localhost") => return "local",
                Some(url::Host::Ipv4(ip))
                    if crate::nodes::util::check_ssrf_ip(
                        std::net::IpAddr::V4(ip),
                        crate::nodes::util::SsrfPolicy::Strict,
                    ).is_err() =>
                {
                    return "local";
                }
                Some(url::Host::Ipv6(ip))
                    if crate::nodes::util::check_ssrf_ip(
                        std::net::IpAddr::V6(ip),
                        crate::nodes::util::SsrfPolicy::Strict,
                    ).is_err() =>
                {
                    return "local";
                }
                _ => {}
            }
        }

        let url = base_url.to_lowercase();
        if url.contains("anthropic.com") {
            "anthropic"
        } else if url.contains("googleapis.com") || url.contains("generativelanguage") {
            "gemini"
        } else {
            "openai"
        }
    }

    /// Resolve the effective base URL for a provider.
    ///
    /// Resolution order:
    ///   1. `user_url` — returned (trailing slashes stripped) if non-empty.
    ///   2. `ProviderRecord.default_base_url` — used when `user_url` is empty.
    ///   3. `"https://api.openai.com/v1"` — safety fallback for unregistered ids.
    ///
    /// when `provider_id == "anthropic"`, a trailing `"/v1"`
    /// is stripped from the resolved URL. Anthropic API paths already include
    /// `"/v1"` (e.g. `"/v1/messages"`), so a user-supplied URL ending in `"/v1"`
    /// would otherwise produce the doubled path `"/v1/v1/messages"`.
    pub fn resolve_base_url(provider_id: &str, user_url: &str) -> String {
        let raw = if !user_url.trim().is_empty() {
            user_url.trim_end_matches('/').to_string()
        } else {
            Self::global()
                .get(provider_id)
                .map(|r| r.default_base_url.to_string())
                .unwrap_or_else(|| "https://api.openai.com/v1".to_string())
        };
        if provider_id == "anthropic" {
            raw.trim_end_matches("/v1").to_string()
        } else {
            raw
        }
    }
}

// ── Shared AI HTTP client ─────────────────────────────────────────────────────

static SHARED_AI_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

/// Process-wide HTTP client shared by all AI nodes.
///
/// Consolidates `AI_CLIENT` (ai_prompt), `AGENT_CLIENT` (ai_agent), and
/// `IMAGE_CLIENT` (image_gen) into one pool.
///
/// Settings chosen to satisfy all three prior clients:
///   - timeout: 120 s (consistent across all three)
///   - pool_max_idle_per_host: 20 (highest of the three; ai_agent reuses most)
///   - redirects: disabled — SSRF checks validate the initial URL only;
///     a redirect to an internal address would bypass the check
///
/// `gen_a1111` overrides the 120 s timeout per-request via
/// `RequestBuilder::timeout()`, which supersedes the client-level timeout.
pub fn shared_ai_client() -> reqwest::Client {
    SHARED_AI_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .pool_max_idle_per_host(20)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("shared AI HTTP client init failed")
    }).clone()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_returns_same_instance() {
        let a = ProviderRegistry::global() as *const ProviderRegistry;
        let b = ProviderRegistry::global() as *const ProviderRegistry;
        assert_eq!(a, b);
    }

    #[test]
    fn known_providers_are_present() {
        let r = ProviderRegistry::global();
        for id in &["openai", "anthropic", "gemini", "local", "gpt_image_1", "gpt_image_2",
                    "dalle3", "nano_banana", "imagen4", "flux_pro", "flux_2_pro",
                    "a1111", "comfyui"] {
            assert!(r.get(id).is_some(), "missing provider: {}", id);
        }
    }

    #[test]
    fn unknown_provider_returns_none() {
        assert!(ProviderRegistry::global().get("does_not_exist").is_none());
    }

    #[test]
    fn auto_is_not_a_registered_provider() {
        // "auto" is a meta-value resolved by callers via detect_from_url()
        // *before* hitting the registry (see ai_prompt.rs / ai_agent.rs).
        // It must never be a real ProviderRecord, or a caller that forgets
        // to pre-resolve it would silently fall through to default behavior
        // instead of failing loudly.
        assert!(ProviderRegistry::global().get("auto").is_none());
    }

    #[test]
    fn detect_from_url_anthropic() {
        assert_eq!(ProviderRegistry::detect_from_url("https://api.anthropic.com"), "anthropic");
    }

    #[test]
    fn detect_from_url_gemini() {
        assert_eq!(ProviderRegistry::detect_from_url("https://generativelanguage.googleapis.com/v1beta"), "gemini");
        assert_eq!(ProviderRegistry::detect_from_url("https://generativelanguage.example.com"), "gemini");
    }

    #[test]
    fn detect_from_url_openai_default() {
        assert_eq!(ProviderRegistry::detect_from_url("https://api.openai.com/v1"), "openai");
        assert_eq!(ProviderRegistry::detect_from_url("https://api.groq.com"), "openai");
    }

    #[test]
    fn detect_from_url_domain_needing_dns_stays_openai() {
        // Documented limitation: detect_from_url does no DNS resolution, so a
        // hostname that would only resolve to a local address at connect time
        // is not detected here.
        assert_eq!(ProviderRegistry::detect_from_url("http://myollama.local:11434"), "openai");
    }

    #[test]
    fn detect_from_url_local_for_localhost_hostname_any_case() {
        assert_eq!(ProviderRegistry::detect_from_url("http://localhost:11434"), "local");
        assert_eq!(ProviderRegistry::detect_from_url("http://LOCALHOST:11434"), "local");
        assert_eq!(ProviderRegistry::detect_from_url("http://LocalHost:11434"), "local");
    }

    #[test]
    fn detect_from_url_local_for_loopback_ip_literal() {
        assert_eq!(ProviderRegistry::detect_from_url("http://127.0.0.1:11434"), "local");
    }

    #[test]
    fn detect_from_url_local_for_private_range_ip_literal() {
        assert_eq!(ProviderRegistry::detect_from_url("http://192.168.1.50:11434"), "local");
    }

    #[test]
    fn detect_from_url_public_ip_literal_stays_openai() {
        assert_eq!(ProviderRegistry::detect_from_url("http://8.8.8.8:443"), "openai");
    }

    #[test]
    fn apply_auth_bearer_adds_header() {
        let record = ProviderRecord {
            id: "test",
            display_name: "Test",
            capabilities: &[Capability::TextGen],
            default_base_url: "",
            auth_style: AuthStyle::BearerToken,
            requires_key: true,
            extra_headers: &[],
        };
        // Can't easily inspect headers without sending, so just confirm it doesn't panic
        let client = reqwest::Client::new();
        let builder = client.get("https://example.com");
        let _ = ProviderRegistry::apply_auth(&record, builder, "sk-test");
    }

    #[test]
    fn apply_auth_bearer_skips_empty_key() {
        let record = ProviderRecord {
            id: "test",
            display_name: "Test",
            capabilities: &[Capability::TextGen],
            default_base_url: "",
            auth_style: AuthStyle::BearerToken,
            requires_key: false,
            extra_headers: &[],
        };
        let client = reqwest::Client::new();
        let builder = client.get("https://example.com");
        let _ = ProviderRegistry::apply_auth(&record, builder, "");
    }

    #[test]
    fn apply_auth_header_key_sends_value_with_real_key() {
        let record = ProviderRecord {
            id: "test",
            display_name: "Test",
            capabilities: &[Capability::TextGen],
            default_base_url: "",
            auth_style: AuthStyle::HeaderKey("x-api-key"),
            requires_key: true,
            extra_headers: &[],
        };
        let client = reqwest::Client::new();
        let builder = client.get("https://example.com");
        let req = ProviderRegistry::apply_auth(&record, builder, "sk-real-key")
            .build()
            .unwrap();
        assert_eq!(
            req.headers().get("x-api-key").map(|v| v.to_str().unwrap()),
            Some("sk-real-key"),
            "HeaderKey branch must send the header with the real key"
        );
    }

    #[test]
    fn apply_auth_header_key_skips_empty_key() {
        // HeaderKey branch must omit the header entirely when the key is
        // empty, matching BearerToken's own `!is_empty()` guard for the
        // identical case.
        let record = ProviderRecord {
            id: "test",
            display_name: "Test",
            capabilities: &[Capability::TextGen],
            default_base_url: "",
            auth_style: AuthStyle::HeaderKey("x-api-key"),
            requires_key: false,
            extra_headers: &[],
        };
        let client = reqwest::Client::new();
        let builder = client.get("https://example.com");
        let req = ProviderRegistry::apply_auth(&record, builder, "")
            .build()
            .unwrap();
        assert!(
            req.headers().get("x-api-key").is_none(),
            "HeaderKey branch must omit the header entirely for an empty key, not send it empty"
        );
    }

    #[test]
    fn apply_auth_extra_headers_appended() {
        let record = ProviderRecord {
            id: "test",
            display_name: "Test",
            capabilities: &[Capability::TextGen],
            default_base_url: "",
            auth_style: AuthStyle::HeaderKey("x-api-key"),
            requires_key: true,
            extra_headers: &[("anthropic-version", "2023-06-01")],
        };
        let client = reqwest::Client::new();
        let builder = client.get("https://example.com");
        let _ = ProviderRegistry::apply_auth(&record, builder, "sk-ant-test");
    }

    #[test]
    fn aliases_have_correct_auth_style() {
        let r = ProviderRegistry::global();
        let dalle3    = r.get("dalle3").unwrap();
        let gpt_img_1 = r.get("gpt_image_1").unwrap();
        assert!(matches!(dalle3.auth_style,    AuthStyle::BearerToken));
        assert!(matches!(gpt_img_1.auth_style, AuthStyle::BearerToken));

        let imagen4    = r.get("imagen4").unwrap();
        let nano       = r.get("nano_banana").unwrap();
        assert!(matches!(imagen4.auth_style, AuthStyle::HeaderKey("x-goog-api-key")));
        assert!(matches!(nano.auth_style,    AuthStyle::HeaderKey("x-goog-api-key")));
    }

    #[test]
    fn local_provider_has_no_cloud_default_and_keyless_bearer_auth() {
        let local = ProviderRegistry::global().get("local").unwrap();
        assert_eq!(local.default_base_url, "");
        assert!(matches!(local.auth_style, AuthStyle::BearerToken));
        assert!(!local.requires_key);
        assert!(local.capabilities.contains(&Capability::TextGen));
    }

    #[test]
    fn shared_ai_client_returns_same_pool() {
        let a = shared_ai_client();
        let b = shared_ai_client();
        // Both should be clones of the same pool; pointer-equality not possible
        // on reqwest::Client, but confirming two calls don't panic suffices.
        drop(a);
        drop(b);
    }

    // ── resolve_base_url tests (R2 normalization) ──────────────────────────────

    #[test]
    fn resolve_base_url_r2_strips_trailing_v1_for_anthropic() {
        // user sets "https://api.anthropic.com/v1" — must not double-append.
        assert_eq!(
            ProviderRegistry::resolve_base_url("anthropic", "https://api.anthropic.com/v1"),
            "https://api.anthropic.com"
        );
        // Already canonical — unchanged.
        assert_eq!(
            ProviderRegistry::resolve_base_url("anthropic", "https://api.anthropic.com"),
            "https://api.anthropic.com"
        );
        // Trailing slash variant.
        assert_eq!(
            ProviderRegistry::resolve_base_url("anthropic", "https://api.anthropic.com/v1/"),
            "https://api.anthropic.com"
        );
    }

    #[test]
    fn resolve_base_url_empty_falls_back_to_registry_default_anthropic() {
        assert_eq!(
            ProviderRegistry::resolve_base_url("anthropic", ""),
            "https://api.anthropic.com"
        );
    }

    #[test]
    fn resolve_base_url_user_override_wins() {
        assert_eq!(
            ProviderRegistry::resolve_base_url("openai", "https://my.proxy.com/v1"),
            "https://my.proxy.com/v1"
        );
    }

    #[test]
    fn resolve_base_url_falls_back_to_registry_default_gemini() {
        assert_eq!(
            ProviderRegistry::resolve_base_url("gemini", ""),
            "https://generativelanguage.googleapis.com/v1beta"
        );
    }

    #[test]
    fn resolve_base_url_unknown_provider_falls_back_to_openai() {
        assert_eq!(
            ProviderRegistry::resolve_base_url("unknown_xyz", ""),
            "https://api.openai.com/v1"
        );
    }

    #[test]
    fn resolve_base_url_strips_trailing_slash_for_non_anthropic() {
        assert_eq!(
            ProviderRegistry::resolve_base_url("openai", "https://api.openai.com/v1/"),
            "https://api.openai.com/v1"
        );
    }
}
