use serde_json::{json, Value};

use crate::model::NodeOutput;

use super::messages::{MAX_MESSAGES_BYTES, Transcript};
use super::structured::{StructuredCheck, check_structured, schema_errors};
use super::web_search::Grounding;

/// A tool the model asked for. The node cannot run it: the request goes to the
/// workflow in the output, so the nodes that do the work stay visible and
/// under the user's control.
pub(super) struct RequestedTool {
    pub(super) id:        String,
    pub(super) name:      String,
    pub(super) arguments: String,
}

pub(super) struct Usage {
    pub(super) model:         String,
    pub(super) provider:      &'static str,
    pub(super) input_tokens:  u64,
    pub(super) output_tokens: u64,
}

pub(super) struct AgentReply {
    pub(super) content:   String,
    pub(super) requested: Vec<RequestedTool>,
    pub(super) finished:  bool,
    pub(super) truncated: bool,
    pub(super) usage:     Usage,
    /// `tool_choice` is `required` and tools were declared.
    pub(super) tool_required: bool,
    pub(super) response_schema: Option<Value>,
    pub(super) grounding: Grounding,
    pub(super) transcript: Option<Transcript>,
}

const REQUIRED_TOOL_MISSING: &str = "tool_choice is \"required\" but the model asked for no tool. A server or model that ignores tool_choice can do this.";

pub(super) struct ToolCheck {
    pub(super) parsed:          Value,
    arguments_valid: bool,
    known_tool:      bool,
    pub(super) error:           Option<String>,
}

/// Checks a tool request before the workflow acts on it: the name must be a
/// declared tool, and the arguments must be a JSON object that fits the tool's
/// `parameters` schema.
pub(super) fn check_tool_call(call: &RequestedTool, declared: &[Value]) -> ToolCheck {
    let mut problems: Vec<String> = Vec::new();
    let text = if call.arguments.trim().is_empty() { "{}" } else { call.arguments.as_str() };
    let (parsed, mut arguments_valid) = match serde_json::from_str::<Value>(text) {
        Ok(v) if v.is_object() => (v, true),
        Ok(_) => {
            problems.push("arguments must be a JSON object".to_string());
            (Value::Null, false)
        }
        Err(e) => {
            problems.push(format!("arguments are not valid JSON: {e}"));
            (Value::Null, false)
        }
    };

    let definition = declared.iter().find(|t| t["name"].as_str() == Some(call.name.as_str()));
    let known_tool = definition.is_some();
    if !known_tool {
        problems.push(format!("the model asked for \"{}\", which is not a declared tool", call.name));
    }
    if let (true, Some(def)) = (arguments_valid, definition) {
        let errors = schema_errors(&def["parameters"], &parsed);
        if !errors.is_empty() {
            arguments_valid = false;
            problems.extend(errors);
        }
    }

    ToolCheck {
        parsed,
        arguments_valid,
        known_tool,
        error: (!problems.is_empty()).then(|| problems.join("; ")),
    }
}

pub(super) fn agent_output(reply: AgentReply, declared: &[Value], mut reasoning: Vec<String>) -> NodeOutput {
    let AgentReply { content, requested, finished, truncated, usage, tool_required, response_schema, grounding, transcript } = reply;
    if !content.is_empty() {
        reasoning.push(format!("[Reply] {}", content));
    }
    let awaiting_tools = !requested.is_empty();
    let tool_missing = tool_required && !awaiting_tools;
    if tool_missing {
        reasoning.push(format!("[Warning] {REQUIRED_TOOL_MISSING}"));
    }
    let structured = match &response_schema {
        Some(schema) if !awaiting_tools => check_structured(schema, &content, truncated),
        _ => StructuredCheck::absent(),
    };
    if let Some(problem) = &structured.error {
        reasoning.push(format!("[Structured Output Warning] {problem}"));
    }
    if let Some(t) = &transcript {
        let bytes = serde_json::to_string(&t.items).map_or(0, |text| text.len());
        if bytes > MAX_MESSAGES_BYTES {
            reasoning.push(format!(
                "[Messages Warning] The conversation is {bytes} bytes, over the {} MiB that Messages accepts, so the next turn would fail with MESSAGES_TOO_LARGE. Start a new conversation or send smaller files.",
                MAX_MESSAGES_BYTES / (1024 * 1024)
            ));
        }
    }
    for query in &grounding.queries {
        reasoning.push(format!("[Web Search] {query}"));
    }
    for warning in &grounding.warnings {
        reasoning.push(format!("[Web Search Warning] {warning}"));
    }
    let mut invalid_calls = 0usize;
    let tool_calls: Vec<Value> = requested.iter().map(|t| {
        reasoning.push(format!("[Tool Call] {} with args: {}", t.name, t.arguments));
        let check = check_tool_call(t, declared);
        if let Some(problem) = &check.error {
            invalid_calls += 1;
            reasoning.push(format!("[Tool Call Warning] {}: {}", t.name, problem));
        }
        json!({
            "id":               t.id,
            "tool":             t.name,
            "arguments":        t.arguments,
            "parsed_arguments": check.parsed,
            "arguments_valid":  check.arguments_valid,
            "known_tool":       check.known_tool,
            "error":            check.error
        })
    }).collect();

    let tokens = format!("{}in/{}out tokens", usage.input_tokens, usage.output_tokens);
    let mut logs = vec![if awaiting_tools {
        format!("Agent requested {} tool call(s); it does not run tools, route them with the workflow ({tokens})", tool_calls.len())
    } else {
        format!("Agent replied ({tokens})")
    }];
    if tool_missing {
        logs.push(REQUIRED_TOOL_MISSING.to_string());
    }
    if invalid_calls > 0 {
        logs.push(format!("{invalid_calls} tool call(s) have invalid arguments or name an undeclared tool; see tool_calls[].error"));
    }
    if structured.error.is_some() {
        logs.push("The reply does not fit response_schema; see structured_error".to_string());
    }
    if !grounding.queries.is_empty() || !grounding.sources.is_empty() {
        logs.push(format!("Web search: queries {}, sources {}", grounding.queries.len(), grounding.sources.len()));
    }
    if !grounding.warnings.is_empty() {
        logs.push(format!("{} web search(es) failed; see reasoning", grounding.warnings.len()));
    }

    NodeOutput::success_with_logs(
        json!({
            "result":           content,
            "iterations":       1,
            "tool_calls":       tool_calls,
            "tool_calls_valid": invalid_calls == 0,
            "awaiting_tools":   awaiting_tools,
            "reasoning":        reasoning,
            "finished":         finished,
            "truncated":        truncated,
            "model":            usage.model,
            "provider":         usage.provider,
            "input_tokens":     usage.input_tokens,
            "output_tokens":    usage.output_tokens,
            "search_queries":   grounding.queries,
            "search_suggestions": grounding.suggestions,
            "sources":          grounding.sources.iter().map(|(title, url)| json!({ "title": title, "url": url })).collect::<Vec<_>>(),
            "structured":       structured.value,
            "structured_valid": structured.valid,
            "structured_error": structured.error,
            "messages":         transcript.map(Transcript::into_value)
        }),
        logs,
    )
}
