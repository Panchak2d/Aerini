use std::collections::HashMap;
use super::{AuthStyle, Capability, ProviderRecord};

/// Build the complete map of built-in provider records.
///
/// Called once at startup from `ProviderRegistry::global()`.
/// All id strings match existing workflow JSON values exactly — do not rename.
pub(super) fn built_in() -> HashMap<&'static str, ProviderRecord> {
    let mut m = HashMap::new();

    // ── TextGen ───────────────────────────────────────────────────────────────

    m.insert("openai", ProviderRecord {
        id: "openai",
        display_name: "OpenAI",
        capabilities: &[Capability::TextGen],
        default_base_url: "https://api.openai.com/v1",
        auth_style: AuthStyle::BearerToken,
        requires_key: false, // OpenAI-compatible endpoints (Ollama, Groq, etc.) may run keyless
        extra_headers: &[],
    });

    m.insert("anthropic", ProviderRecord {
        id: "anthropic",
        display_name: "Anthropic",
        capabilities: &[Capability::TextGen],
        default_base_url: "https://api.anthropic.com",
        auth_style: AuthStyle::HeaderKey("x-api-key"),
        requires_key: true,
        extra_headers: &[("anthropic-version", "2023-06-01")],
    });

    m.insert("gemini", ProviderRecord {
        id: "gemini",
        display_name: "Google Gemini",
        capabilities: &[Capability::TextGen, Capability::ImageGen],
        default_base_url: "https://generativelanguage.googleapis.com/v1beta",
        auth_style: AuthStyle::HeaderKey("x-goog-api-key"),
        requires_key: true,
        extra_headers: &[],
    });

    m.insert("local", ProviderRecord {
        id: "local",
        display_name: "Local Model (local)",
        capabilities: &[Capability::TextGen],
        default_base_url: "", // user-supplied only; no cloud default
        auth_style: AuthStyle::BearerToken,
        requires_key: false, // same OpenAI-compatible wire behavior as `openai`
        extra_headers: &[],
    });

    // ── ImageGen ──────────────────────────────────────────────────────────────

    m.insert("gpt_image_1", ProviderRecord {
        id: "gpt_image_1",
        display_name: "GPT Image 1",
        capabilities: &[Capability::ImageGen],
        default_base_url: "https://api.openai.com/v1",
        auth_style: AuthStyle::BearerToken,
        requires_key: true,
        extra_headers: &[],
    });

    m.insert("gpt_image_2", ProviderRecord {
        id: "gpt_image_2",
        display_name: "GPT Image 2",
        capabilities: &[Capability::ImageGen],
        default_base_url: "https://api.openai.com/v1",
        auth_style: AuthStyle::BearerToken,
        requires_key: true,
        extra_headers: &[],
    });

    // Legacy alias: "dalle3" → GPT Image 1 behavior (DALL-E 3 retired May 12 2026)
    m.insert("dalle3", ProviderRecord {
        id: "dalle3",
        display_name: "DALL-E 3 (legacy alias → GPT Image 1)",
        capabilities: &[Capability::ImageGen],
        default_base_url: "https://api.openai.com/v1",
        auth_style: AuthStyle::BearerToken,
        requires_key: true,
        extra_headers: &[],
    });

    m.insert("nano_banana", ProviderRecord {
        id: "nano_banana",
        display_name: "NanoBanana (Gemini Flash Image)",
        capabilities: &[Capability::ImageGen],
        default_base_url: "https://generativelanguage.googleapis.com/v1beta",
        auth_style: AuthStyle::HeaderKey("x-goog-api-key"),
        requires_key: true,
        extra_headers: &[],
    });

    // Legacy alias: "imagen4" → NanoBanana (Imagen 4 direct API shuts down Aug 17 2026)
    m.insert("imagen4", ProviderRecord {
        id: "imagen4",
        display_name: "Imagen 4 (legacy alias → NanoBanana)",
        capabilities: &[Capability::ImageGen],
        default_base_url: "https://generativelanguage.googleapis.com/v1beta",
        auth_style: AuthStyle::HeaderKey("x-goog-api-key"),
        requires_key: true,
        extra_headers: &[],
    });

    m.insert("flux_pro", ProviderRecord {
        id: "flux_pro",
        display_name: "FLUX1.1 [pro]",
        capabilities: &[Capability::ImageGen],
        default_base_url: "https://api.bfl.ai",
        auth_style: AuthStyle::HeaderKey("x-key"),
        requires_key: true,
        extra_headers: &[],
    });

    m.insert("flux_2_pro", ProviderRecord {
        id: "flux_2_pro",
        display_name: "FLUX.2 [pro]",
        capabilities: &[Capability::ImageGen],
        default_base_url: "https://api.bfl.ai",
        auth_style: AuthStyle::HeaderKey("x-key"),
        requires_key: true,
        extra_headers: &[],
    });

    // Local providers — auth handled entirely within their node functions.
    // apply_auth() returns the builder unchanged for BasicAuth and None.

    m.insert("a1111", ProviderRecord {
        id: "a1111",
        display_name: "Automatic1111 (local)",
        capabilities: &[Capability::ImageGen],
        default_base_url: "", // user-supplied only; no cloud default
        auth_style: AuthStyle::BasicAuth,
        requires_key: false,
        extra_headers: &[],
    });

    m.insert("comfyui", ProviderRecord {
        id: "comfyui",
        display_name: "ComfyUI (local)",
        capabilities: &[Capability::ImageGen],
        default_base_url: "", // user-supplied only; no cloud default
        auth_style: AuthStyle::None,
        requires_key: false,
        extra_headers: &[],
    });

    m
}
