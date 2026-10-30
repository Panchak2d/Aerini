use serde_json::Value;

use crate::error::NodeError;
use crate::nodes::util::cfg_bool_opt;

// ── Continuation ─────────────────────────────────────────────────────────────
//
// The node keeps no state between runs. A conversation continues when the
// workflow passes the previous run's `messages` output back in, with the
// results of any tools the model asked for. The transcript holds the
// provider's own messages. Anthropic's and Gemini's turns are replayed as
// received, because thinking blocks, thought signatures and the server-tool
// blocks of a web search are rejected, or silently dropped, if they are
// rebuilt. An OpenAI-compatible turn is rebuilt from the fields a request
// accepts, since the reply carries response-only fields.

/// Largest `messages` input accepted, in bytes.
pub(super) const MAX_MESSAGES_BYTES: usize = 4 * 1024 * 1024;

/// Largest tool result sent to the model, in bytes. The rest is cut.
pub(super) const MAX_TOOL_RESULT_BYTES: usize = 64 * 1024;

/// A conversation as the provider's own messages. `provider` is the same name
/// the node reports in its output.
pub(super) struct Transcript {
    pub(super) provider: &'static str,
    pub(super) items:    Vec<Value>,
    /// Anthropic stopped a search part-way; sending the messages back as they
    /// are lets the model carry on, so no new user turn is added.
    pub(super) paused:   bool,
}

impl Transcript {
    pub(super) fn into_value(self) -> Value {
        let mut map = serde_json::Map::new();
        map.insert("provider".to_string(), Value::String(self.provider.to_string()));
        map.insert("items".to_string(), Value::Array(self.items));
        if self.paused {
            map.insert("paused".to_string(), Value::Bool(true));
        }
        Value::Object(map)
    }
}

/// The `messages` input.
pub(super) enum History {
    /// `{role, content}` turns, as AI Memory returns them.
    Plain(Vec<(String, String)>),
    Native(Transcript),
}

pub(super) struct ToolResult {
    pub(super) id:       String,
    pub(super) tool:     String,
    pub(super) text:     String,
    pub(super) is_error: bool,
    /// Bytes cut from the result to fit `MAX_TOOL_RESULT_BYTES`.
    pub(super) cut:      usize,
}

/// The continuation settings, read once from the config.
pub(super) struct Continuation {
    pub(super) history:         Option<History>,
    pub(super) results:         Option<Vec<ToolResult>>,
    pub(super) dropped_empty:   usize,
    pub(super) return_messages: bool,
}

impl Continuation {
    pub(super) fn from_config(input: &Value, provider: &'static str) -> Result<Self, NodeError> {
        let return_messages = cfg_bool_opt(&input["return_messages"], "return_messages")?.unwrap_or(false);
        let (history, dropped_empty) = parse_messages(&input["messages"], provider)?;
        let results = parse_tool_results(&input["tool_results"])?;
        if results.is_some() && history.is_none() {
            return Err(invalid_messages(
                "tool_results needs Messages: pass the messages output of the AI Agent run that asked for the tools",
            ));
        }
        Ok(Self { history, results, dropped_empty, return_messages })
    }
}

fn invalid_messages(detail: impl Into<String>) -> NodeError {
    NodeError::unrecoverable("INVALID_MESSAGES", detail)
}

pub(super) fn tool_results_mismatch(detail: impl Into<String>) -> NodeError {
    NodeError::unrecoverable("TOOL_RESULTS_MISMATCH", detail)
}

fn check_messages_size(bytes: usize) -> Result<(), NodeError> {
    if bytes > MAX_MESSAGES_BYTES {
        return Err(NodeError::unrecoverable(
            "MESSAGES_TOO_LARGE",
            format!("messages is {bytes} bytes; the limit is {} MiB. Start a new conversation or send smaller files", MAX_MESSAGES_BYTES / (1024 * 1024)),
        ));
    }
    Ok(())
}

/// Reads the Messages field: JSON text or an already-parsed value, or nothing.
/// The second value counts plain turns dropped for having no text.
fn parse_messages(raw: &Value, provider: &'static str) -> Result<(Option<History>, usize), NodeError> {
    let value = match raw {
        Value::Null => return Ok((None, 0)),
        Value::String(s) if s.trim().is_empty() => return Ok((None, 0)),
        Value::String(s) => {
            check_messages_size(s.len())?;
            serde_json::from_str::<Value>(s)
                .map_err(|e| invalid_messages(format!("messages is not valid JSON: {e}")))?
        }
        other => {
            check_messages_size(other.to_string().len())?;
            other.clone()
        }
    };
    match value {
        Value::Array(list) => plain_history(&list),
        Value::Object(_) => native_history(&value, provider).map(|t| (Some(History::Native(t)), 0)),
        _ => Err(invalid_messages(
            "messages must be a list of {role, content} turns, or the messages output of an earlier AI Agent run",
        )),
    }
}

/// `{role, content}` turns. Only user and assistant are accepted, so a
/// webhook body wired here cannot add system instructions; those come from the
/// node's System field. A turn with no text is dropped.
fn plain_history(list: &[Value]) -> Result<(Option<History>, usize), NodeError> {
    let mut turns: Vec<(String, String)> = Vec::new();
    let mut dropped = 0usize;
    for (i, entry) in list.iter().enumerate() {
        let Some(obj) = entry.as_object() else {
            return Err(invalid_messages(format!("messages[{i}] must be an object with role and content")));
        };
        if obj.contains_key("items") && obj.contains_key("provider") {
            return Err(invalid_messages(format!(
                "messages[{i}] is a transcript from an AI Agent run; pass it on its own, not inside a list"
            )));
        }
        let Some(role) = obj.get("role").and_then(Value::as_str).map(|r| r.trim().to_ascii_lowercase()) else {
            return Err(invalid_messages(format!("messages[{i}] needs a text \"role\"")));
        };
        if role != "user" && role != "assistant" {
            return Err(invalid_messages(format!(
                "messages[{i}] has the role \"{role:.32}\"; only user and assistant are accepted, because system instructions come from the System field"
            )));
        }
        let Some(content) = obj.get("content").and_then(Value::as_str) else {
            return Err(invalid_messages(format!("messages[{i}] needs text \"content\"")));
        };
        if content.trim().is_empty() {
            dropped += 1;
        } else {
            turns.push((role, content.to_string()));
        }
    }
    Ok(((!turns.is_empty()).then_some(History::Plain(turns)), dropped))
}

/// Roles a transcript item may carry for each provider. An item with any
/// other role, a system one above all, is refused instead of replayed.
fn native_role_ok(provider: &str, role: &str) -> bool {
    match provider {
        "anthropic" => matches!(role, "user" | "assistant"),
        "gemini"    => matches!(role, "user" | "model"),
        _           => matches!(role, "user" | "assistant" | "tool"),
    }
}

fn native_history(value: &Value, provider: &'static str) -> Result<Transcript, NodeError> {
    let (Some(from), Some(items)) = (value["provider"].as_str(), value["items"].as_array()) else {
        return Err(invalid_messages(
            "a transcript needs \"provider\" and \"items\"; pass the messages output of an AI Agent run unchanged",
        ));
    };
    if from != provider {
        return Err(NodeError::unrecoverable(
            "MESSAGES_PROVIDER_MISMATCH",
            format!("messages came from provider \"{from:.32}\" but this node uses \"{provider}\". Keep the provider and model the same in every turn"),
        ));
    }
    if items.is_empty() {
        return Err(invalid_messages("the transcript in messages has no items"));
    }
    for (i, item) in items.iter().enumerate() {
        if !item.is_object() {
            return Err(invalid_messages(format!("messages.items[{i}] must be an object")));
        }
        let role = item["role"].as_str().unwrap_or("");
        if !native_role_ok(provider, role) {
            return Err(invalid_messages(format!(
                "messages.items[{i}] has the role \"{role:.32}\", which is not accepted; system instructions come from the System field"
            )));
        }
    }
    Ok(Transcript {
        provider,
        items:  items.clone(),
        paused: value["paused"].as_bool().unwrap_or(false),
    })
}

/// Reads the Tool Results field. An empty list counts as absent, so the first
/// turn of a chain can wire one in.
fn parse_tool_results(raw: &Value) -> Result<Option<Vec<ToolResult>>, NodeError> {
    let list: Vec<Value> = match raw {
        Value::Null => return Ok(None),
        Value::String(s) if s.trim().is_empty() => return Ok(None),
        Value::String(s) => match serde_json::from_str::<Value>(s) {
            Ok(Value::Array(a)) => a,
            Ok(_) => return Err(invalid_messages("tool_results must be a JSON array of results")),
            Err(e) => return Err(invalid_messages(format!("tool_results is not valid JSON: {e}"))),
        },
        Value::Array(a) => a.clone(),
        _ => return Err(invalid_messages("tool_results must be a JSON array of results")),
    };
    if list.is_empty() {
        return Ok(None);
    }
    let mut out: Vec<ToolResult> = Vec::with_capacity(list.len());
    for (i, entry) in list.iter().enumerate() {
        let Some(obj) = entry.as_object() else {
            return Err(invalid_messages(format!("tool_results[{i}] must be an object {{id, tool, content, is_error}}")));
        };
        let text_field = |key: &str| match obj.get(key) {
            None | Some(Value::Null) => Ok(String::new()),
            Some(Value::String(s)) => Ok(s.clone()),
            Some(_) => Err(invalid_messages(format!("tool_results[{i}].{key} must be text"))),
        };
        let id = text_field("id")?;
        let tool = text_field("tool")?;
        if id.is_empty() && tool.is_empty() {
            return Err(invalid_messages(format!(
                "tool_results[{i}] needs an id, or a tool name when the provider sent no ids"
            )));
        }
        let Some(content) = obj.get("content") else {
            return Err(invalid_messages(format!("tool_results[{i}] needs content")));
        };
        let is_error = match obj.get("is_error") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(b)) => *b,
            Some(_) => return Err(invalid_messages(format!("tool_results[{i}].is_error must be true or false"))),
        };
        let text = match content {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let (text, cut) = cap_tool_result(text);
        out.push(ToolResult { id, tool, text, is_error, cut });
    }
    Ok(Some(out))
}

/// Cuts a result to `MAX_TOOL_RESULT_BYTES` at a character boundary and says
/// so in the text itself, so the model and the author both see it.
fn cap_tool_result(text: String) -> (String, usize) {
    if text.len() <= MAX_TOOL_RESULT_BYTES {
        return (text, 0);
    }
    let mut end = MAX_TOOL_RESULT_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let cut = text.len() - end;
    (format!("{}…[truncated {cut} bytes]", &text[..end]), cut)
}
