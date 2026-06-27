use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::NodeOutput;

use super::attachments::{ImageAttachment, DocAttachment};
use super::shared::{send_and_parse, extract_err_msg};

// Gemini
// Docs: https://ai.google.dev/api/generate-content
// Auth: x-goog-api-key (applied via ProviderRegistry::apply_auth)
// Endpoint: POST {base_url}/models/{model}:generateContent
// Body: { contents: [{ role, parts: [{text}] }], systemInstruction: { parts: [{text}] }, generationConfig }
// Response: candidates[0].content.parts[0].text
// Inline data: { "inline_data": { "mime_type": <mime>, "data": <b64> } } — same shape for images and PDFs.
// VERIFIED: ai.google.dev/gemini-api/docs, June 2026.

#[allow(clippy::too_many_arguments)]
pub(super) async fn call_gemini(
    client: reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    system: &str,
    prompt: &str,
    temperature: f64,
    max_tokens: u64,
    image_attachments: &[ImageAttachment],
    doc_attachments: &[DocAttachment],
) -> NodeOutput {
    if api_key.is_empty() {
        return NodeOutput::failure(NodeError::unrecoverable(
            "MISSING_API_KEY",
            "Gemini requires an API key. Add one in Connections.",
        ));
    }

    let endpoint = format!("{}/models/{}:generateContent", base_url, model);

    let mut parts: Vec<Value> = vec![json!({ "text": prompt })];
    for (mime, data) in image_attachments {
        parts.push(json!({ "inline_data": { "mime_type": mime, "data": data } }));
    }
    for (mime, data, _filename) in doc_attachments {
        parts.push(json!({ "inline_data": { "mime_type": mime, "data": data } }));
    }

    let mut body = json!({
        "contents": [
            { "role": "user", "parts": parts }
        ],
        "generationConfig": {
            "temperature":       temperature,
            "maxOutputTokens":   max_tokens
        }
    });

    if !system.is_empty() && system != "You are a helpful assistant." {
        body["systemInstruction"] = json!({
            "parts": [{ "text": system }]
        });
    }

    let record = crate::provider::ProviderRegistry::global()
        .get("gemini")
        .expect("gemini always registered");
    let req = crate::provider::ProviderRegistry::apply_auth(
        record,
        client.post(&endpoint).header("Content-Type", "application/json"),
        api_key,
    ).json(&body);

    let (status, resp_json) = match send_and_parse(req).await {
        Ok(v)  => v,
        Err(e) => return e,
    };

    // Gemini error: { "error": { "code": 400, "message": "...", "status": "..." } }
    if let Some(err_obj) = resp_json["error"].as_object() {
        let msg = extract_err_msg(err_obj, "Unknown Gemini error");
        return if status == 429 {
            NodeOutput::failure(NodeError::recoverable("RATE_LIMITED", msg))
        } else {
            NodeOutput::failure(NodeError::unrecoverable("API_ERROR", msg))
        };
    }

    let content = resp_json["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_string();

    let model_used = resp_json["modelVersion"].as_str().unwrap_or(model).to_string();
    let input_tok  = resp_json["usageMetadata"]["promptTokenCount"].as_u64().unwrap_or(0);
    let output_tok = resp_json["usageMetadata"]["candidatesTokenCount"].as_u64().unwrap_or(0);

    NodeOutput::success_with_logs(
        json!({
            "content":       content,
            "model":         model_used,
            "provider":      "gemini",
            "input_tokens":  input_tok,
            "output_tokens": output_tok
        }),
        vec![format!("Gemini responded ({} chars, {}in/{}out tokens)", content.len(), input_tok, output_tok)],
    )
}
