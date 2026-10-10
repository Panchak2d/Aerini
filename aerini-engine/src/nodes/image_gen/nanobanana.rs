use chrono::Utc;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::NodeOutput;
use crate::nodes::util::read_json_response_capped;

use super::shared::{network_err, strip_data_uri_prefix, mime_to_ext, ImageRun};

// -- NanoBanana (Google Gemini image models) ---------------------------------
//
// Config aliases "imagen4" and "nano_banana" both route here.
// No native batch -- loop one request per image; some may fail while others succeed.
// output.source is the alias the workflow configured.
//
// The model comes from the `model` field (default gemini-2.5-flash-image). Google's
// deprecations page (updated 2026-10-09) lists gemini-2.5-flash-image's earliest
// shutdown as March 15 2027. The two newer ids are called through the same
// generateContent request: Google's docs show them only on the Interactions API
// and do not say whether generateContent accepts them, so a refusal from Google
// is reported as API_ERROR with Google's own message.
//
// Gemini reports safety blocks inside an HTTP 200 reply: no candidates with
// promptFeedback.blockReason, or a candidate whose finishReason says why there
// is no image. gemini_image turns each of those into a named error.

const MAX_MODEL_TEXT_CHARS: usize = 500;

pub(super) const DEFAULT_GEMINI_MODEL: &str = "gemini-2.5-flash-image";
pub(super) const GEMINI_MODELS: [&str; 3] = [
    "gemini-2.5-flash-image",
    "gemini-nano-banana-2.1",
    "gemini-3.1-flash-lite-image",
];
const LEGACY_GEMINI_MODEL: &str = "gemini-2.5-flash-image";

pub(super) fn gemini_model_of(cfg: &Value) -> Result<&'static str, NodeError> {
    match cfg["model"].as_str().map(|m| m.trim().to_ascii_lowercase()).filter(|m| !m.is_empty()) {
        None => match &cfg["model"] {
            Value::Null | Value::String(_) => Ok(DEFAULT_GEMINI_MODEL),
            _ => Err(unknown_gemini_model(&cfg["model"].to_string())),
        },
        Some(m) => GEMINI_MODELS
            .iter()
            .find(|known| **known == m)
            .copied()
            .ok_or_else(|| unknown_gemini_model(&m)),
    }
}

fn unknown_gemini_model(name: &str) -> NodeError {
    NodeError::unrecoverable(
        "UNKNOWN_MODEL",
        format!("Unknown model '{}'. Valid: {}", name, GEMINI_MODELS.join(", ")),
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn gen_nano_banana(
    client: reqwest::Client,
    model: &str,
    prompt: &str,
    n: usize,
    aspect_ratio: &str,
    api_key: &str,
    ref_images: &[Value],
    source: &str,
) -> NodeOutput {
    let stem = format!("{}_{}", source, Utc::now().timestamp_millis());
    let mut run = ImageRun::new("NanoBanana image", n);

    for i in 0..n {
        match call_nano_banana_once(&client, model, prompt, aspect_ratio, api_key, ref_images, &stem, i).await {
            Ok(media_obj) => run.generated(i, media_obj),
            Err(e) => {
                if run.failed(i, e) { break; }
            }
        }
    }

    let mut out = run.finish(source);
    if model == LEGACY_GEMINI_MODEL {
        out.logs.insert(0, format!(
            "[WARN] {} shuts down on March 15 2027 (Google). Set model to gemini-nano-banana-2.1 or gemini-3.1-flash-lite-image before then.",
            LEGACY_GEMINI_MODEL
        ));
    }
    out
}

/// Maps a Gemini error reply to a named error. Google documents no stable
/// machine code for these cases, so the message text decides; the raw message
/// is always kept.
fn gemini_api_error(status: u16, msg: &str) -> NodeError {
    let m = msg.to_ascii_lowercase();
    let key_not_valid = m.contains("api key not valid") || m.contains("api_key_invalid");
    match status {
        429 => NodeError::recoverable("RATE_LIMITED", format!("NanoBanana rate limit exceeded. {}", msg)),
        401 => NodeError::unrecoverable("INVALID_API_KEY", format!("Invalid or unauthorized Google AI API key. Check your key in Connections. Google said: {}", msg)),
        400 | 403 if key_not_valid => NodeError::unrecoverable("INVALID_API_KEY", format!("Invalid Google AI API key. Check your key in Connections. Google said: {}", msg)),
        400 | 403 if m.contains("leaked") => NodeError::unrecoverable("API_KEY_LEAKED", format!("Google reports this API key as leaked and has disabled it. Create a new key. Google said: {}", msg)),
        403 if m.contains("denied access") => NodeError::unrecoverable("PROJECT_DENIED", format!("Google denied access for this project. Check the project and billing in Google AI Studio, or contact Google support. Google said: {}", msg)),
        403 if m.contains("are blocked") => NodeError::unrecoverable("API_KEY_RESTRICTED", format!("This API key's restrictions block the Generative Language API. Allow it in the key's API restrictions. Google said: {}", msg)),
        403 => NodeError::unrecoverable("PERMISSION_DENIED", format!("Google refused the request. Check the API key's permissions and project access. Google said: {}", msg)),
        400 if m.contains("location is not supported") || m.contains("not available in your country") => {
            NodeError::unrecoverable("REGION_NOT_SUPPORTED", format!("The Gemini API is not available from this location. Google said: {}", msg))
        }
        400 if m.contains("safety") || m.contains("policy") => {
            NodeError::unrecoverable("CONTENT_POLICY_VIOLATION", format!("Prompt rejected by safety filters: {}", msg))
        }
        _ => NodeError::unrecoverable("API_ERROR", msg.to_string()),
    }
}

/// Returns `(mime_type, base64)` of the first image in the reply, or the named
/// error that explains why there is none.
fn gemini_image(json: &Value) -> Result<(String, String), NodeError> {
    let candidate = &json["candidates"][0];
    let parts = candidate["content"]["parts"].as_array().filter(|p| !p.is_empty());

    for part in parts.into_iter().flatten() {
        if let Some(inline) = part.get("inlineData") {
            let b64 = inline["data"].as_str().unwrap_or("");
            if b64.is_empty() { continue; }
            let mime = inline["mimeType"].as_str().unwrap_or("image/png");
            return Ok((mime.to_string(), strip_data_uri_prefix(b64).to_string()));
        }
    }

    let model_said = {
        let text: String = parts
            .into_iter()
            .flatten()
            .filter(|p| !p["thought"].as_bool().unwrap_or(false))
            .filter_map(|p| p["text"].as_str())
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if text.is_empty() {
            String::new()
        } else if text.chars().count() > MAX_MODEL_TEXT_CHARS {
            format!(" The model said: {}…", text.chars().take(MAX_MODEL_TEXT_CHARS).collect::<String>())
        } else {
            format!(" The model said: {}", text)
        }
    };

    if let Some(reason) = json["promptFeedback"]["blockReason"].as_str().filter(|r| !r.is_empty()) {
        let detail = json["promptFeedback"]["blockReasonMessage"]
            .as_str()
            .filter(|m| !m.is_empty())
            .map(|m| format!(" ({})", m))
            .unwrap_or_default();
        return Err(NodeError::unrecoverable(
            "PROMPT_BLOCKED",
            format!("Gemini blocked the prompt: {}{}.{}", reason, detail, model_said),
        ));
    }

    match candidate["finishReason"].as_str().filter(|r| !r.is_empty()) {
        Some(r @ ("IMAGE_SAFETY" | "IMAGE_PROHIBITED_CONTENT")) => Err(NodeError::unrecoverable(
            "IMAGE_BLOCKED",
            format!("Gemini blocked the generated image (finishReason {}).{}", r, model_said),
        )),
        Some(r) if r != "STOP" => Err(NodeError::unrecoverable(
            "NO_IMAGE_RETURNED",
            format!("Gemini returned no image (finishReason {}).{}", r, model_said),
        )),
        _ if !model_said.is_empty() => Err(NodeError::unrecoverable(
            "NO_IMAGE_RETURNED",
            format!("Gemini returned text instead of an image.{}", model_said),
        )),
        _ if parts.is_none() => Err(NodeError::unrecoverable(
            "EMPTY_RESPONSE",
            "NanoBanana returned no candidates or content parts",
        )),
        _ => Err(NodeError::unrecoverable(
            "EMPTY_RESPONSE",
            "NanoBanana returned no inlineData in response parts",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
async fn call_nano_banana_once(
    client: &reqwest::Client,
    model: &str,
    prompt: &str,
    aspect_ratio: &str,
    api_key: &str,
    ref_images: &[Value],
    stem: &str,
    index: usize,
) -> Result<Value, NodeError> {
    // Build parts array: reference images first (as inlineData), then the text prompt.
    // Gemini generateContent accepts inlineData parts alongside text parts in
    // the same content for image editing. Files API fails silently — inlineData required.
    // Source: ai.google.dev/gemini-api/docs/image-generation, June 2026.
    let mut parts: Vec<Value> = Vec::new();
    for img in ref_images {
        let data = img["data"].as_str().unwrap_or("");
        let clean = strip_data_uri_prefix(data);
        if clean.is_empty() { continue; }
        let mime = img["mime_type"].as_str().unwrap_or("image/png");
        parts.push(json!({ "inlineData": { "mimeType": mime, "data": clean } }));
    }
    parts.push(json!({ "text": prompt }));

    let body = json!({
        "contents": [{ "parts": parts }],
        "generationConfig": {
            "responseModalities": ["IMAGE"],
            "imageConfig": { "aspectRatio": aspect_ratio }
        }
    });

    let record = crate::provider::ProviderRegistry::global()
        .get("nano_banana")
        .expect("nano_banana always registered");
    // registry-managed: auth header
    let resp = crate::provider::ProviderRegistry::apply_auth(
        record,
        client.post(format!("https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent", model)),
        api_key,
    )
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(network_err)?;

    let status = resp.status().as_u16();
    let json: Value = read_json_response_capped(resp).await.map_err(|e| {
        crate::nodes::util::provider_error(status, "PARSE_ERROR", e)
    })?;

    if let Some(err) = json["error"].as_object() {
        let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("Unknown error").to_string();
        return Err(gemini_api_error(status, &msg));
    }

    let (mime, b64) = gemini_image(&json)?;
    Ok(json!({
        "filename":  format!("{}_{}.{}", stem, index, mime_to_ext(&mime)),
        "data":      b64,
        "mime_type": mime
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_inline_image_is_returned_even_with_text_beside_it() {
        let reply = json!({ "candidates": [{ "finishReason": "STOP", "content": { "parts": [
            { "text": "Here you go" },
            { "inlineData": { "mimeType": "image/jpeg", "data": "data:image/jpeg;base64,QUJD" } },
        ] } }] });
        assert_eq!(gemini_image(&reply).unwrap(), ("image/jpeg".to_string(), "QUJD".to_string()));
    }

    #[test]
    fn a_blocked_prompt_names_the_reason() {
        let reply = json!({ "promptFeedback": { "blockReason": "PROHIBITED_CONTENT", "blockReasonMessage": "no" } });
        let e = gemini_image(&reply).unwrap_err();
        assert_eq!(e.code, "PROMPT_BLOCKED");
        assert!(!e.recoverable);
        assert!(e.message.contains("PROHIBITED_CONTENT") && e.message.contains("(no)"), "{}", e.message);
    }

    #[test]
    fn image_safety_finish_reasons_are_image_blocked_and_carry_the_reason() {
        for reason in ["IMAGE_SAFETY", "IMAGE_PROHIBITED_CONTENT"] {
            let reply = json!({ "candidates": [{ "finishReason": reason }] });
            let e = gemini_image(&reply).unwrap_err();
            assert_eq!(e.code, "IMAGE_BLOCKED", "{reason}");
            assert!(e.message.contains(reason), "{}", e.message);
        }
    }

    #[test]
    fn no_image_and_other_finish_reasons_are_named_in_the_error() {
        for reason in ["NO_IMAGE", "IMAGE_OTHER", "MAX_TOKENS"] {
            let reply = json!({ "candidates": [{ "finishReason": reason, "content": { "parts": [] } }] });
            let e = gemini_image(&reply).unwrap_err();
            assert_eq!(e.code, "NO_IMAGE_RETURNED", "{reason}");
            assert!(e.message.contains(reason), "{}", e.message);
        }
    }

    #[test]
    fn a_text_only_reply_is_surfaced_not_dropped() {
        let reply = json!({ "candidates": [{ "finishReason": "STOP", "content": { "parts": [
            { "text": "I can't draw that" }, { "text": "internal thought", "thought": true },
        ] } }] });
        let e = gemini_image(&reply).unwrap_err();
        assert_eq!(e.code, "NO_IMAGE_RETURNED");
        assert!(e.message.contains("I can't draw that"), "{}", e.message);
        assert!(!e.message.contains("internal thought"), "{}", e.message);
    }

    #[test]
    fn a_long_text_reply_is_cut_to_a_readable_length() {
        let long = "x".repeat(MAX_MODEL_TEXT_CHARS * 3);
        let reply = json!({ "candidates": [{ "finishReason": "STOP", "content": { "parts": [{ "text": long }] } }] });
        let e = gemini_image(&reply).unwrap_err();
        assert!(e.message.chars().count() < MAX_MODEL_TEXT_CHARS + 120, "{}", e.message.chars().count());
        assert!(e.message.ends_with('…'));
    }

    #[test]
    fn a_reply_with_nothing_in_it_is_still_empty_response() {
        assert_eq!(gemini_image(&json!({})).unwrap_err().code, "EMPTY_RESPONSE");
        let no_inline = json!({ "candidates": [{ "finishReason": "STOP", "content": { "parts": [{ "inlineData": { "data": "" } }] } }] });
        assert_eq!(gemini_image(&no_inline).unwrap_err().code, "EMPTY_RESPONSE");
    }

    #[test]
    fn google_error_replies_map_to_named_codes_that_keep_googles_text() {
        let cases = [
            (401, "anything", "INVALID_API_KEY"),
            (400, "API key not valid. Please pass a valid API key.", "INVALID_API_KEY"),
            (403, "Your API key was reported as leaked. Please use another API key.", "API_KEY_LEAKED"),
            (403, "Your project has been denied access. Please contact support.", "PROJECT_DENIED"),
            (403, "Requests to this API generativelanguage.googleapis.com are blocked.", "API_KEY_RESTRICTED"),
            (403, "Permission denied on resource", "PERMISSION_DENIED"),
            (400, "User location is not supported for the API use.", "REGION_NOT_SUPPORTED"),
            (400, "Gemini API free tier is not available in your country", "REGION_NOT_SUPPORTED"),
            (400, "blocked by safety settings", "CONTENT_POLICY_VIOLATION"),
            (400, "something else", "API_ERROR"),
            (500, "boom", "API_ERROR"),
        ];
        for (status, msg, code) in cases {
            let e = gemini_api_error(status, msg);
            assert_eq!(e.code, code, "{status} {msg}");
            assert!(!e.recoverable, "{status} {msg}");
            assert!(e.message.contains(msg) || status == 401, "{}", e.message);
        }
        let e = gemini_api_error(429, "slow down");
        assert_eq!((e.code.as_str(), e.recoverable), ("RATE_LIMITED", true));
    }

    #[test]
    fn the_model_defaults_to_the_current_one_and_unknown_ids_fail() {
        assert_eq!(gemini_model_of(&json!({})).unwrap(), DEFAULT_GEMINI_MODEL);
        assert_eq!(gemini_model_of(&json!({ "model": "  " })).unwrap(), DEFAULT_GEMINI_MODEL);
        assert_eq!(gemini_model_of(&json!({ "model": "Gemini-Nano-Banana-2.1" })).unwrap(), "gemini-nano-banana-2.1");
        for bad in [json!({ "model": "gemini-9" }), json!({ "model": 5 })] {
            let e = gemini_model_of(&bad).unwrap_err();
            assert_eq!(e.code, "UNKNOWN_MODEL");
            assert!(!e.recoverable);
        }
    }
}
