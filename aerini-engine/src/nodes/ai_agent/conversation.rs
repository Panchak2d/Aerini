use serde_json::{json, Value};

use crate::error::NodeError;
use crate::nodes::ai_prompt::{
    ImageAttachment, DocAttachment,
};

use super::messages::{Continuation, History, MAX_TOOL_RESULT_BYTES, ToolResult, Transcript, tool_results_mismatch};

struct PendingCall {
    id:   String,
    name: String,
}

/// The client tool requests the transcript's last turn leaves unanswered.
/// Anthropic's server tools (web search) are not among them: Anthropic ran
/// those itself.
fn pending_calls(t: &Transcript) -> Vec<PendingCall> {
    let Some(last) = t.items.last() else { return Vec::new() };
    let call = |id: &Value, name: &Value| PendingCall {
        id:   id.as_str().unwrap_or("").to_string(),
        name: name.as_str().unwrap_or("unknown").to_string(),
    };
    match t.provider {
        "anthropic" if last["role"] == "assistant" => last["content"].as_array().into_iter().flatten()
            .filter(|b| b["type"] == "tool_use")
            .map(|b| call(&b["id"], &b["name"]))
            .collect(),
        "gemini" if last["role"] == "model" => last["parts"].as_array().into_iter().flatten()
            .filter(|p| !p["functionCall"].is_null())
            .map(|p| call(&p["functionCall"]["id"], &p["functionCall"]["name"]))
            .collect(),
        "openai_compatible" if last["role"] == "assistant" => last["tool_calls"].as_array().into_iter().flatten()
            .map(|c| call(&c["id"], &c["function"]["name"]))
            .collect(),
        _ => Vec::new(),
    }
}

fn answers(call: &PendingCall, result: &ToolResult) -> bool {
    if call.id.is_empty() {
        result.id.is_empty() && result.tool == call.name
    } else {
        result.id == call.id
    }
}

fn id_list(labels: &[String]) -> String {
    const SHOWN: usize = 5;
    let mut text = labels.iter().take(SHOWN).map(|l| format!("{l:.64}")).collect::<Vec<_>>().join(", ");
    if labels.len() > SHOWN {
        text.push_str(&format!(" and {} more", labels.len() - SHOWN));
    }
    text
}

/// One result per pending call, in the order the model asked. Fails when a
/// call has no result, a result matches no call, or a call is answered twice.
fn match_results<'a>(pending: &[PendingCall], results: &'a [ToolResult]) -> Result<Vec<&'a ToolResult>, String> {
    let mut used = vec![false; results.len()];
    let mut matched: Vec<&ToolResult> = Vec::with_capacity(pending.len());
    let mut missing: Vec<String> = Vec::new();
    for call in pending {
        let label = if call.id.is_empty() { format!("{} (no id)", call.name) } else { call.id.clone() };
        match results.iter().enumerate().find(|(i, r)| !used[*i] && answers(call, r)) {
            Some((i, r)) => {
                used[i] = true;
                matched.push(r);
            }
            None => missing.push(label),
        }
    }
    let (mut unknown, mut duplicate): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    for (_, r) in results.iter().enumerate().filter(|(i, _)| !used[*i]) {
        let label = if r.id.is_empty() { format!("{} (no id)", r.tool) } else { r.id.clone() };
        if pending.iter().any(|c| answers(c, r)) {
            duplicate.push(label);
        } else {
            unknown.push(label);
        }
    }
    if missing.is_empty() && unknown.is_empty() && duplicate.is_empty() {
        return Ok(matched);
    }
    let mut problems: Vec<String> = Vec::new();
    for (what, list) in [("missing", &missing), ("unknown", &unknown), ("duplicate", &duplicate)] {
        if !list.is_empty() {
            problems.push(format!("{what}: {}", id_list(list)));
        }
    }
    Err(format!("tool_results must answer every pending tool request exactly once ({})", problems.join("; ")))
}

/// The turn that returns tool results, in each provider's shape.
fn results_messages(provider: &str, pending: &[PendingCall], results: &[&ToolResult]) -> Vec<Value> {
    let pairs = pending.iter().zip(results.iter());
    match provider {
        "anthropic" => {
            let blocks: Vec<Value> = pairs.map(|(call, r)| {
                let mut block = json!({ "type": "tool_result", "tool_use_id": call.id, "content": r.text });
                if r.is_error {
                    block["is_error"] = json!(true);
                }
                block
            }).collect();
            vec![json!({ "role": "user", "content": blocks })]
        }
        "gemini" => {
            let parts: Vec<Value> = pairs.map(|(call, r)| {
                let mut body = serde_json::Map::new();
                body.insert((if r.is_error { "error" } else { "result" }).to_string(), json!(r.text));
                let mut response = json!({ "name": call.name, "response": Value::Object(body) });
                if !call.id.is_empty() {
                    response["id"] = json!(call.id);
                }
                json!({ "functionResponse": response })
            }).collect();
            vec![json!({ "role": "user", "parts": parts })]
        }
        _ => pairs.map(|(call, r)| json!({
            "role": "tool",
            "tool_call_id": call.id,
            "content": if r.is_error { format!("Error: {}", r.text) } else { r.text.clone() },
        })).collect(),
    }
}

/// Plain turns in a provider's message shape.
fn plain_items(turns: &[(String, String)], provider: &str) -> Vec<Value> {
    turns.iter().map(|(role, text)| if provider == "gemini" {
        json!({ "role": if role == "assistant" { "model" } else { "user" }, "parts": [{ "text": text }] })
    } else {
        json!({ "role": role, "content": text })
    }).collect()
}

/// The user turn that carries the goal: plain text without files, otherwise
/// each provider's own multimodal block format.
pub(super) fn user_message(provider: &str, text: &str, images: &[ImageAttachment], docs: &[DocAttachment]) -> Value {
    let no_files = images.is_empty() && docs.is_empty();
    match provider {
        "gemini" => {
            let mut parts: Vec<Value> = vec![json!({ "text": text })];
            for (mime, data) in images {
                parts.push(json!({ "inline_data": { "mime_type": mime, "data": data } }));
            }
            for (mime, data, _filename) in docs {
                parts.push(json!({ "inline_data": { "mime_type": mime, "data": data } }));
            }
            json!({ "role": "user", "parts": parts })
        }
        "anthropic" => {
            let content: Value = if no_files {
                json!(text)
            } else {
                let mut blocks: Vec<Value> = Vec::new();
                for (mime, data) in images {
                    blocks.push(json!({ "type": "image", "source": { "type": "base64", "media_type": mime, "data": data } }));
                }
                for (mime, data, _filename) in docs {
                    blocks.push(json!({ "type": "document", "source": { "type": "base64", "media_type": mime, "data": data } }));
                }
                blocks.push(json!({ "type": "text", "text": text }));
                json!(blocks)
            };
            json!({ "role": "user", "content": content })
        }
        _ => {
            let content: Value = if no_files {
                json!(text)
            } else {
                let mut blocks: Vec<Value> = vec![json!({ "type": "text", "text": text })];
                for (mime, data) in images {
                    blocks.push(json!({ "type": "image_url", "image_url": { "url": format!("data:{};base64,{}", mime, data) } }));
                }
                for (mime, data, filename) in docs {
                    blocks.push(json!({ "type": "file", "file": { "filename": filename, "file_data": format!("data:{};base64,{}", mime, data) } }));
                }
                json!(blocks)
            };
            json!({ "role": "user", "content": content })
        }
    }
}

pub(super) struct Conversation {
    /// Provider-shaped messages to send, without the system prompt.
    pub(super) messages: Vec<Value>,
    /// Lines for the start of the reasoning trace.
    pub(super) notes:    Vec<String>,
}

/// The messages to send: any earlier conversation, then either this run's user
/// turn, the results of the tools the model asked for, or nothing (a paused
/// reply resumes as it stands).
pub(super) fn build_conversation(cont: &Continuation, provider: &str, new_user: Value, has_files: bool) -> Result<Conversation, NodeError> {
    let mut notes: Vec<String> = Vec::new();
    if cont.dropped_empty > 0 {
        notes.push(format!("[Messages Warning] Dropped {} message(s) with no text.", cont.dropped_empty));
    }
    let mut messages: Vec<Value> = Vec::new();
    match (&cont.history, &cont.results) {
        (None, _) => messages.push(new_user),
        (Some(History::Plain(turns)), None) => {
            messages.extend(plain_items(turns, provider));
            messages.push(new_user);
        }
        (Some(History::Plain(_)), Some(_)) => {
            return Err(tool_results_mismatch(
                "tool_results needs the messages output of an AI Agent run that asked for tools; a list of plain turns has none pending",
            ));
        }
        (Some(History::Native(t)), None) => {
            let pending = pending_calls(t);
            if !pending.is_empty() {
                let labels: Vec<String> = pending.iter()
                    .map(|c| if c.id.is_empty() { format!("{} (no id)", c.name) } else { c.id.clone() })
                    .collect();
                return Err(tool_results_mismatch(format!(
                    "the conversation ends with tool requests that have no result ({}); pass them in tool_results",
                    id_list(&labels)
                )));
            }
            messages.extend(t.items.iter().cloned());
            if t.paused {
                notes.push("[Messages] Resumed a paused reply; the goal was not sent again.".to_string());
            } else {
                messages.push(new_user);
            }
        }
        (Some(History::Native(t)), Some(results)) => {
            let pending = pending_calls(t);
            if pending.is_empty() {
                return Err(tool_results_mismatch(
                    "tool_results was given but the conversation does not end with a tool request; pass the messages output of the run that asked for the tools",
                ));
            }
            let matched = match_results(&pending, results).map_err(tool_results_mismatch)?;
            messages.extend(t.items.iter().cloned());
            messages.extend(results_messages(provider, &pending, &matched));
            notes.push("[Messages] Sent tool_results; the goal and context are already in the conversation, so they were not sent again.".to_string());
            if has_files {
                notes.push("[Attachment Warning] Files are not sent together with tool_results; the model has the ones from the first turn.".to_string());
            }
            for r in results.iter().filter(|r| r.cut > 0) {
                let label = if r.id.is_empty() { r.tool.as_str() } else { r.id.as_str() };
                notes.push(format!(
                    "[Messages Warning] The result for \"{label:.64}\" was cut at {} KiB ({} bytes removed).",
                    MAX_TOOL_RESULT_BYTES / 1024, r.cut
                ));
            }
        }
    }
    Ok(Conversation { messages, notes })
}

/// The conversation to return: what was sent plus the model's reply. `None`
/// when the setting is off, when the reply is a tool request cut off at
/// max_tokens (it cannot be answered), or when the provider sent no message.
pub(super) fn capture_transcript(
    wanted: bool,
    provider: &'static str,
    mut sent: Vec<Value>,
    assistant: Option<Value>,
    paused: bool,
    unanswerable: bool,
    trace: &mut Vec<String>,
) -> Option<Transcript> {
    if !wanted {
        return None;
    }
    if unanswerable {
        trace.push("[Warning] messages is null: the reply was cut off while asking for a tool, so the request cannot be answered. Raise max_tokens.".to_string());
        return None;
    }
    sent.push(assistant?);
    Some(Transcript { provider, items: sent, paused })
}
