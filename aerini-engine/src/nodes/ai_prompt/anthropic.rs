use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::NodeOutput;

use super::attachments::{ImageAttachment, DocAttachment};
use super::shared::{send_and_parse, extract_err_msg};

// Anthropic (Claude)
// Docs: https://docs.anthropic.com/en/api/messages
// Auth: x-api-key + anthropic-version (applied via ProviderRegistry::apply_auth)
// Endpoint: POST /v1/messages
// Image block:    {"type":"image","source":{"type":"base64","media_type":<mime>,"data":<b64>}}
// Document block: {"type":"document","source":{"type":"base64","media_type":"application/pdf","data":<b64>}}
// Both are GA — no beta header required. VERIFIED: platform.claude.com docs, June 2026.

#[allow(clippy::too_many_arguments)]
pub(super) async fn call_anthropic(
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
            "Anthropic requires an API key. Add one in Connections.",
        ));
    }

    let endpoint = format!("{}/v1/messages", base_url);

    let user_content: Value = if image_attachments.is_empty() && doc_attachments.is_empty() {
        json!(prompt)
    } else {
        let mut blocks: Vec<Value> = Vec::new();
        for (mime, data) in image_attachments {
            blocks.push(json!({
                "type": "image",
                "source": { "type": "base64", "media_type": mime, "data": data }
            }));
        }
        for (mime, data, _filename) in doc_attachments {
            blocks.push(json!({
                "type": "document",
                "source": { "type": "base64", "media_type": mime, "data": data }
            }));
        }
        blocks.push(json!({ "type": "text", "text": prompt }));
        json!(blocks)
    };

    let body = json!({
        "model":       model,
        "system":      system,
        "messages":    [{ "role": "user", "content": user_content }],
        "max_tokens":  max_tokens,
        "temperature": temperature
    });

    let record = crate::provider::ProviderRegistry::global()
        .get("anthropic")
        .expect("anthropic always registered");
    let req = crate::provider::ProviderRegistry::apply_auth(
        record,
        client.post(&endpoint).header("Content-Type", "application/json"),
        api_key,
    ).json(&body);

    let (status, resp_json) = match send_and_parse(req).await {
        Ok(v)  => v,
        Err(e) => return e,
    };

    // Anthropic error: { "type": "error", "error": { "type": "...", "message": "..." } }
    if let Some(err_obj) = resp_json["error"].as_object() {
        let msg = extract_err_msg(err_obj, "Unknown Anthropic error");
        return if status == 429 || status == 529 {
            NodeOutput::failure(NodeError::recoverable("RATE_LIMITED", msg))
        } else {
            NodeOutput::failure(NodeError::unrecoverable("API_ERROR", msg))
        };
    }

    let content = resp_json["content"]
        .as_array()
        .and_then(|arr| arr.iter().find(|b| b["type"] == "text"))
        .and_then(|b| b["text"].as_str())
        .unwrap_or("")
        .to_string();

    let model_used = resp_json["model"].as_str().unwrap_or(model).to_string();
    let input_tok  = resp_json["usage"]["input_tokens"].as_u64().unwrap_or(0);
    let output_tok = resp_json["usage"]["output_tokens"].as_u64().unwrap_or(0);

    NodeOutput::success_with_logs(
        json!({
            "content":       content,
            "model":         model_used,
            "provider":      "anthropic",
            "input_tokens":  input_tok,
            "output_tokens": output_tok
        }),
        vec![format!("Anthropic responded ({} chars, {}in/{}out tokens)", content.len(), input_tok, output_tok)],
    )
}
