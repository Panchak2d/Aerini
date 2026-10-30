use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortArity, PortDefinition, PortPosition};
use crate::nodes::util::{cfg_f64_opt, cfg_u64_opt};
use crate::nodes::ai_prompt::{
    process_attachments, extract_port_attachments, ATTACHMENT_ONLY_PROMPT, is_official_openai, model_or_default, anthropic_text, request_timeout,
};

use super::anthropic::{anthropic_assistant, anthropic_grounding, anthropic_tool_requests, anthropic_tools, call_anthropic_agent};
use super::conversation::{build_conversation, capture_transcript, user_message};
use super::gemini::run_gemini_agent;
use super::messages::Continuation;
use super::openai::{call_openai_agent, openai_assistant, openai_grounding};
use super::output::{AgentReply, RequestedTool, Usage, agent_output};
use super::settings::{ModelSettings, check_effort};
use super::structured::{check_response_schema_for_provider, parse_response_schema};
use super::tools::{ToolControls, arguments_text, check_tool_controls, parse_tools, strict_defs, with_strict_flag};
use super::web_search::{OPENAI_SEARCH_MODEL, SearchSite, check_web_search};

/// AI Agent node: gives a model a goal and a set of tools and returns its answer.
///
/// The node makes one model call. It cannot run the tools it is given: when the
/// model asks for one, the request comes back in `tool_calls` with
/// `awaiting_tools` set, and the workflow's own nodes do the work. Inventing a
/// tool result so the model could continue would let it act on made-up data and
/// bill a call per round.
///
/// **Provider behaviour:**
/// - OpenAI / OpenAI-compatible, Anthropic and Gemini: tool definitions are
///   sent; the reply carries the text and any tool requests in the same
///   `tool_calls` shape, so a workflow routes them the same way for every
///   provider.
///
/// **Tool controls** (`tool_choice`, `single_tool_call`, `strict_tools`) map to
/// each provider's own request fields. A setting a provider cannot honor fails
/// the node with `INVALID_TOOL_CONTROLS` instead of being dropped.
///
/// **Structured output** (`response_schema`): the schema goes to the provider's
/// JSON-schema reply mode, and the reply is parsed and checked against it here
/// too, because a server may accept the setting without enforcing it. The
/// result is `structured`, `structured_valid` and `structured_error`.
///
/// **Web search** (`web_search`) uses the provider's own search, which runs on
/// the provider's side inside the same call, so the node still makes one
/// request: Anthropic's `web_search` server tool, Gemini's `google_search`
/// tool, and `web_search_options` on OpenAI's search models. The queries and
/// the pages the reply drew on come back in `search_queries` and `sources`.
/// Combinations a provider is not known to accept fail with `INVALID_WEB_SEARCH`.
///
/// **Effort** (`effort`) maps to Anthropic's `output_config.effort`, Gemini's
/// `thinkingConfig.thinkingLevel` and `reasoning_effort` for OpenAI-compatible
/// servers. A level the chosen model rejects comes back as the provider's error.
///
/// **Continuation** (`messages`, `tool_results`, `return_messages`): the node
/// keeps no state between runs, so a conversation goes on only through what the
/// workflow hands back in. With `return_messages` on, the output carries the
/// conversation as the provider's own messages; wired into a later AI Agent's
/// `messages` together with `tool_results`, it lets the model go on from its
/// tool requests. Those messages travel untouched, because each vendor wants
/// its reasoning state replayed exactly as it sent it. A list of plain
/// `{role, content}` turns, as AI Memory returns, is accepted too. Neither
/// input can set system instructions: those come only from the node's own
/// System field.
pub struct AiAgentNode;

#[async_trait]
impl Node for AiAgentNode {
    fn type_id(&self) -> &'static str { "ai_agent" }
    fn display_name(&self) -> &'static str { "AI Agent" }
    fn node_type(&self) -> NodeType { NodeType::Ai }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Give an AI model a goal and tool definitions. It makes one model call and returns the answer, or the tools the model asks for; it does not run them." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["goal", "provider"],
            "properties": {
                "goal": {
                    "type": "string",
                    "description": "What the agent should accomplish. Be specific and include constraints."
                },
                "system": {
                    "type": "string",
                    "description": "System instructions for the agent's persona and behavior."
                },
                "tools": {
                    "type": "string",
                    "x-aerini-tool-builder": true,
                    "description": "JSON array of tool definitions. Each tool: {name, description, parameters}. Sent to every provider; the node returns the model's tool requests and does not run them. Anthropic accepts names of letters, digits, underscores and hyphens, up to 64 characters."
                },
                "tool_choice": {
                    "type": "string",
                    "enum": ["auto", "required", "none"],
                    "description": "Whether the model may, must or must not ask for a tool. auto: the model decides (default). required: the model must ask for at least one tool (not available with Anthropic, whose current models accept only auto and none). none: the model answers in text only. Needs Tools."
                },
                "single_tool_call": {
                    "type": "boolean",
                    "description": "Allow at most one tool request per reply. Off by default, so the model may ask for several. Not available with Gemini, which has no such setting."
                },
                "strict_tools": {
                    "type": "boolean",
                    "description": "Make the model's tool arguments follow each tool's parameters schema exactly. Off by default. Every object in a schema needs additionalProperties false; for OpenAI, every property must also be in required. Not available with Gemini. Anthropic allows 20 strict tools."
                },
                "response_schema": {
                    "type": "string",
                    "x-aerini-multiline": true,
                    "description": "JSON Schema the reply must follow, written as a JSON object whose top level has type object. The reply comes back parsed in structured and is checked against the schema in structured_valid. Sent to every provider. OpenAI and Anthropic need additionalProperties false on every object, and OpenAI also needs every property listed in required. A long reply needs a higher max_tokens."
                },
                "web_search": {
                    "type": "boolean",
                    "description": "Let the model search the web before it answers, using the provider's own search. Off by default. OpenAI needs a search model such as gpt-5-search-api (used when Model is blank); Anthropic and Gemini work with any model that supports search. Not available with local servers. Gemini cannot combine it with Tools, and Anthropic cannot combine it with Response Schema. The provider sees your prompt, charges per search, and the pages it finds are untrusted content."
                },
                "web_search_max_uses": {
                    "type": "number",
                    "description": "Most searches the model may run in one reply (Anthropic only, default 5). Needs Web Search."
                },
                "web_search_domains": {
                    "type": "string",
                    "description": "Search only these sites (Anthropic only). Separate with commas or new lines and write example.com, not https://example.com; subdomains are included. Blank allows any site. Needs Web Search."
                },
                "effort": {
                    "type": "string",
                    "enum": ["default", "none", "minimal", "low", "medium", "high", "xhigh", "max"],
                    "description": "How hard the model thinks. default: the provider's own default. Anthropic takes low, medium, high, xhigh or max; Gemini takes minimal, low, medium or high; OpenAI-compatible and local servers take none, minimal, low, medium, high, xhigh or max. A level the chosen model does not accept comes back as the provider's API_ERROR. Higher effort uses more of Max Tokens and costs more."
                },
                "context": {
                    "type": "string",
                    "description": "Additional context to give the agent (from previous nodes, variables, etc.)"
                },
                "messages": {
                    "type": "string",
                    "x-aerini-multiline": true,
                    "description": "Earlier conversation to continue, as JSON. Either a list of {role, content} turns where role is user or assistant (AI Memory's messages output fits), or the messages output of an earlier AI Agent run, passed in unchanged. Roles system and developer are refused: use System. Keep Provider, Model, System and Tools the same from turn to turn. At most 4 MiB."
                },
                "tool_results": {
                    "type": "string",
                    "x-aerini-multiline": true,
                    "description": "Results for the tools the previous run asked for, as a JSON array of {id, tool, content, is_error}. Needs Messages from an earlier run that had Return Messages on. Every tool request must be answered exactly once, matched by id (by tool name for a provider that sends no ids). content may be text or any JSON; a result over 64 KiB is cut and marked. Goal and Context are not sent again."
                },
                "return_messages": {
                    "type": "boolean",
                    "description": "Return the conversation so far in the messages output so a later AI Agent node can continue it. Off by default: the transcript can hold the model's reasoning and any files sent, and it grows with every turn."
                },
                "provider": {
                    "type": "string",
                    "enum": ["openai", "anthropic", "gemini", "local", "auto"],
                    "description": "AI provider. Tool definitions are sent and tool requests returned for every provider."
                },
                "model": {
                    "type": "string",
                    "description": "Model name, for example gpt-5.6, claude-sonnet-5-5 or gemini-3.8-flash.",
                    "x-aerini-model-picker": true
                },
                "base_url": {
                    "type": "string",
                    "description": "API base URL (leave blank for default)"
                },
                "api_key": {
                    "type": "string",
                    "description": "API key — resolved from Connections"
                },
                "max_iterations": {
                    "type": "number",
                    "description": "Not used: the agent makes one call. Kept so saved workflows still load."
                },
                "max_tokens": {
                    "type": "number",
                    "description": "Maximum tokens per agent response (default: 2048). Higher values allow longer output but increase cost."
                },
                "temperature": {
                    "type": "number",
                    "description": "Creativity (0.0-1.0). Agents work best at 0.2-0.5."
                },
                "timeout_secs": {
                    "type": "number",
                    "description": "How long to wait for the model's reply, in seconds. Default 3600 (1 hour), range 10-86400. A request that runs past it fails with TIMEOUT and is not retried. Raise it for slow local models."
                }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "result":        { "type": "string", "description": "Final answer or conclusion from the agent" },
                "iterations":    { "type": "number", "description": "Always 1: the agent makes one model call" },
                "tool_calls":    { "type": "array",  "description": "Tools the model asked for: {id, tool, arguments, parsed_arguments, arguments_valid, known_tool, error}. The node does not run them. arguments_valid is false when the arguments are not a JSON object or do not match the tool's parameters; known_tool is false when the name is not a declared tool; error says why, or is null." },
                "tool_calls_valid": { "type": "boolean", "description": "True when every tool call names a declared tool and has valid arguments, and when there are no tool calls. Branch on it before running the tools." },
                "awaiting_tools":{ "type": "boolean","description": "True when the model asked for tools and has not given a final answer" },
                "reasoning":     { "type": "array",  "description": "Trace of the run: attachment warnings, the reply, and each tool request" },
                "finished":      { "type": "boolean","description": "True if the model answered cleanly; false if it asked for tools or the reply was cut off" },
                "truncated":     { "type": "boolean","description": "True if the provider cut off the response mid-generation (max_tokens hit). Raise max_tokens to fix." },
                "model":         { "type": "string",  "description": "Model that answered, as the provider reports it" },
                "provider":      { "type": "string",  "description": "openai_compatible, anthropic or gemini" },
                "input_tokens":  { "type": "number",  "description": "Input tokens the provider reported" },
                "output_tokens": { "type": "number",  "description": "Output tokens the provider reported; for Gemini this includes thinking tokens" },
                "search_queries": { "type": "array", "description": "The web searches the provider ran, as text. Empty when Web Search is off, or when the provider does not report its queries (OpenAI)." },
                "search_suggestions": { "type": "string", "description": "Gemini only: the Search Suggestions HTML Google requires to be shown with a grounded answer. Empty otherwise. It is HTML from Google: show it as Google's terms ask, and do not paste it into a page you do not control without checking it." },
                "sources":       { "type": "array",  "description": "Pages the reply drew on, as {title, url}, without repeats. Empty when Web Search is off. Gemini reports a redirect address for each page. Treat titles and pages as untrusted content." },
                "structured":    { "description": "The reply parsed as JSON when response_schema is set and the reply fits it; otherwise null" },
                "structured_valid": { "type": "boolean", "description": "True when response_schema is set and the reply is JSON that fits it. False when no schema is set, when the model asked for tools, or when the reply does not fit." },
                "structured_error": { "type": ["string", "null"], "description": "Why the reply does not fit response_schema, or null. Also null when no schema is set or the model asked for tools." },
                "messages":      { "type": ["object", "null"], "description": "The conversation after this reply, as {provider, items}, when return_messages is on; otherwise null. Also null when the reply was cut off at max_tokens while asking for a tool, because that request cannot be answered. Pass it unchanged to a later AI Agent's messages. input_tokens and output_tokens count this call only." }
            }
        })
    }

    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![
                PortDefinition { id: "input".to_string(),       label: "In".to_string(),    position: PortPosition::Left, port_type: None, arity: PortArity::Single },
                // Runtime files — wired from image_gen, collect_files, text_to_file, etc.
                // Merged with config["attachments"] (static design-time files) before the first
                // agent message is built. Same merge/classify pattern as ai_prompt.rs.
                PortDefinition { id: "attachments".to_string(), label: "Files".to_string(), position: PortPosition::Left, port_type: Some("files".to_string()), arity: PortArity::Single },
            ],
            outputs: vec![
                PortDefinition { id: "output".to_string(),   label: "Done".to_string(),  position: PortPosition::Right , port_type: None, arity: PortArity::Single },
                PortDefinition { id: "on_error".to_string(), label: "Error".to_string(), position: PortPosition::Right, port_type: None, arity: PortArity::Single },
            ],
        }
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        // Attachment handling — same merge/classify pattern as ai_prompt.
        // Two sources:
        //   1. config["attachments"]      — static files added at design time.
        //   2. config["attachments_expr"] — dynamic files from the "Files" input port.
        //      Canvas.ts writes {{SourceNode.output.files}} here when a wire is connected.
        let static_atts: Vec<Value> = input.input["attachments"].as_array().cloned().unwrap_or_default();
        let port_atts: Vec<Value>   = extract_port_attachments(&input.input["attachments_expr"]);
        let mut attachments_raw = static_atts;
        attachments_raw.extend(port_atts);
        let pa = process_attachments(&attachments_raw);

        // A file-only message resolves the goal expression to "". Readable
        // attachments stand in for the text; otherwise the goal stays required.
        let goal = match input.input["goal"].as_str() {
            Some(g) if !g.trim().is_empty() => g.to_string(),
            _ if pa.has_content() => ATTACHMENT_ONLY_PROMPT.to_string(),
            _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_GOAL", "goal is required")),
        };

        let provider_str = input.input["provider"].as_str().unwrap_or("auto");
        let user_url_raw = input.input["base_url"].as_str().filter(|s| !s.trim().is_empty()).unwrap_or("");
        let provider_id: &str = if provider_str == "auto" {
            crate::provider::ProviderRegistry::detect_from_url(user_url_raw)
        } else {
            provider_str
        };
        let is_anthropic = provider_id == "anthropic";
        let is_gemini    = provider_id == "gemini";

        // No default model exists for "local" — a blank model would otherwise
        // silently inherit the OpenAI-flagship default below and get sent to
        // whatever server base_url points to, which is very unlikely to have it.
        if provider_id == "local"
            && input.input["model"].as_str().map(|s| s.trim().is_empty()).unwrap_or(true)
        {
            return NodeOutput::failure(NodeError::unrecoverable(
                "MISSING_MODEL",
                "model is required when provider is \"local\"",
            ));
        }

        let settings = match ModelSettings::from_config(&input.input) {
            Ok(s)  => s,
            Err(e) => return NodeOutput::failure(e),
        };

        let base_url = crate::provider::ProviderRegistry::resolve_base_url(provider_id, user_url_raw);

        let default_model = if is_anthropic {
            "claude-sonnet-5"
        } else if is_gemini {
            "gemini-3.6-flash"
        } else if settings.web_search.is_some() && is_official_openai(&base_url) {
            OPENAI_SEARCH_MODEL
        } else {
            "gpt-5.6"
        };

        let model   = model_or_default(&input.input["model"], default_model);
        let api_key = input.input["api_key"].as_str().unwrap_or("").to_string();

        // AllowLocal (not Strict): same policy as ai_prompt/mod.rs —
        // local inference servers (Ollama etc.) must be reachable via base_url.
        // Single check site, upstream of the is_gemini branch, so it covers all
        // three providers (OpenAI-compatible, Anthropic, Gemini).
        if let Err(e) = crate::nodes::util::check_host_ssrf_from_url(&base_url, crate::nodes::util::SsrfPolicy::AllowLocal).await {
            return NodeOutput::failure(NodeError::unrecoverable("SSRF_BLOCKED", e));
        }

        if let Err(e) = cfg_u64_opt(&input.input["max_iterations"], "max_iterations") {
            return NodeOutput::failure(e);
        }
        let max_tokens = match cfg_u64_opt(&input.input["max_tokens"], "max_tokens") {
            Ok(v) => v.unwrap_or(2048),
            Err(e) => return NodeOutput::failure(e),
        };
        let temperature = match cfg_f64_opt(&input.input["temperature"], "temperature") {
            Ok(v) => v.unwrap_or(0.3),
            Err(e) => return NodeOutput::failure(e),
        };
        let timeout = match request_timeout(&input.input["timeout_secs"]) {
            Ok(t)  => t,
            Err(e) => return NodeOutput::failure(e),
        };

        let mut system = input.input["system"].as_str()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("You are a helpful AI agent. Complete the given goal step by step. When you have finished, provide a clear final answer.")
            .to_string();

        let context_str = input.input["context"].as_str().unwrap_or("").to_string();

        let tools = match parse_tools(&input.input["tools"]) {
            Ok(t) => t,
            Err(e) => return NodeOutput::failure(NodeError::unrecoverable("INVALID_TOOLS", e)),
        };

        let controls = match ToolControls::from_config(&input.input) {
            Ok(c)  => c,
            Err(e) => return NodeOutput::failure(e),
        };
        if let Err(e) = check_tool_controls(controls, provider_id, &tools) {
            return NodeOutput::failure(NodeError::unrecoverable("INVALID_TOOL_CONTROLS", e));
        }

        let response_schema = match parse_response_schema(&input.input["response_schema"]) {
            Ok(s)  => s,
            Err(e) => return NodeOutput::failure(NodeError::unrecoverable("INVALID_RESPONSE_SCHEMA", e)),
        };
        if let Some(schema) = &response_schema {
            if let Err(e) = check_response_schema_for_provider(schema, provider_id, &base_url) {
                return NodeOutput::failure(NodeError::unrecoverable("INVALID_RESPONSE_SCHEMA", e));
            }
        }

        if let Some(effort) = settings.effort {
            if let Err(e) = check_effort(effort, provider_id) {
                return NodeOutput::failure(NodeError::unrecoverable("INVALID_EFFORT", e));
            }
        }
        if let Some(search) = &settings.web_search {
            let site = SearchSite { provider_id, base_url: &base_url, model: &model };
            if let Err(e) = check_web_search(search, site, &tools, controls, response_schema.is_some()) {
                return NodeOutput::failure(NodeError::unrecoverable("INVALID_WEB_SEARCH", e));
            }
        }

        let provider_name: &'static str = if is_anthropic {
            "anthropic"
        } else if is_gemini {
            "gemini"
        } else {
            "openai_compatible"
        };
        let continuation = match Continuation::from_config(&input.input, provider_name) {
            Ok(c)  => c,
            Err(e) => return NodeOutput::failure(e),
        };

        let anthropic_tool_defs = if is_anthropic {
            let defs = match anthropic_tools(&tools) {
                Ok(t)  => t,
                Err(e) => return NodeOutput::failure(NodeError::unrecoverable("INVALID_TOOLS", e)),
            };
            if controls.strict {
                match strict_defs(&defs, "input_schema", false) {
                    Ok(t)  => t,
                    Err(e) => return NodeOutput::failure(NodeError::unrecoverable("INVALID_TOOL_CONTROLS", e)),
                }
            } else {
                defs
            }
        } else {
            Vec::new()
        };

        // Only the hosted OpenAI API is known to reject a schema that breaks
        // strict mode; other compatible servers get the flag and decide.
        let openai_tool_defs = if !is_anthropic && !is_gemini && controls.strict {
            let strict = if is_official_openai(&base_url) {
                strict_defs(&tools, "parameters", true)
            } else {
                Ok(tools.iter().map(with_strict_flag).collect())
            };
            match strict {
                Ok(t)  => t,
                Err(e) => return NodeOutput::failure(NodeError::unrecoverable("INVALID_TOOL_CONTROLS", e)),
            }
        } else {
            tools.clone()
        };

        let client = crate::provider::shared_ai_client();

        let image_attachments   = pa.images;
        let doc_attachments     = pa.docs;
        let attachment_warnings = pa.logs;
        if !pa.warning.is_empty() {
            system.push_str(&pa.warning);
        }

        let user_content = if context_str.is_empty() {
            goal.clone()
        } else {
            format!("Context:\n{}\n\nGoal:\n{}", context_str, goal)
        };

        let has_files = !image_attachments.is_empty() || !doc_attachments.is_empty();
        let new_user = user_message(provider_name, &user_content, &image_attachments, &doc_attachments);
        let conversation = match build_conversation(&continuation, provider_name, new_user, has_files) {
            Ok(c)  => c,
            Err(e) => return NodeOutput::failure(e),
        };
        // Attachment warnings go first in the reasoning trace.
        let mut reasoning_trace: Vec<String> = attachment_warnings
            .into_iter()
            .map(|w| format!("[Attachment Warning] {}", w))
            .collect();
        reasoning_trace.extend(conversation.notes);
        let messages = conversation.messages;

        // Gemini uses a completely different message format, so it has its own function.
        if is_gemini {
            return run_gemini_agent(
                client, base_url, api_key, model, system,
                messages, tools, controls, response_schema, settings, max_tokens, temperature,
                continuation.return_messages, reasoning_trace, timeout,
            ).await;
        }

        let response = if is_anthropic {
            call_anthropic_agent(
                &client, &base_url, &api_key, &model, &system,
                &messages, &anthropic_tool_defs, controls, response_schema.as_ref(), &settings, temperature, max_tokens, timeout,
            ).await
        } else {
            call_openai_agent(
                &client, &base_url, &api_key, &model, &system,
                &messages, &openai_tool_defs, controls, response_schema.as_ref(), &settings, temperature, max_tokens, provider_id, timeout,
            ).await
        };
        let resp = match response {
            Ok(r)  => r,
            Err(e) => return e.into_node_output(),
        };

        let usage = if is_anthropic {
            Usage {
                model:         resp["model"].as_str().unwrap_or(model.as_str()).to_string(),
                provider:      "anthropic",
                input_tokens:  resp["usage"]["input_tokens"].as_u64().unwrap_or(0),
                output_tokens: resp["usage"]["output_tokens"].as_u64().unwrap_or(0),
            }
        } else {
            Usage {
                model:         resp["model"].as_str().unwrap_or(model.as_str()).to_string(),
                provider:      "openai_compatible",
                input_tokens:  resp["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
                output_tokens: resp["usage"]["completion_tokens"].as_u64().unwrap_or(0),
            }
        };

        let (content, finish_reason, requested, assistant): (String, String, Vec<RequestedTool>, Option<Value>) = if is_anthropic {
            (
                anthropic_text(&resp),
                resp["stop_reason"].as_str().unwrap_or("end_turn").to_string(),
                anthropic_tool_requests(&resp),
                anthropic_assistant(&resp),
            )
        } else {
            let choice = &resp["choices"][0];
            let calls = choice["message"]["tool_calls"].as_array()
                .map(|calls| calls.iter().map(|c| RequestedTool {
                    id:        c["id"].as_str().unwrap_or("").to_string(),
                    name:      c["function"]["name"].as_str().unwrap_or("unknown").to_string(),
                    arguments: arguments_text(&c["function"]["arguments"]),
                }).collect())
                .unwrap_or_default();
            if choice["message"]["content"].as_str().unwrap_or("").is_empty() {
                if let Some(refusal) = choice["message"]["refusal"].as_str().filter(|r| !r.is_empty()) {
                    return NodeOutput::failure(NodeError::unrecoverable("REFUSED", format!("The model refused the request: {refusal}")));
                }
            }
            (
                choice["message"]["content"].as_str().unwrap_or("").to_string(),
                choice["finish_reason"].as_str().unwrap_or("stop").to_string(),
                calls,
                openai_assistant(&choice["message"]),
            )
        };
        if is_anthropic && content.is_empty() && finish_reason == "refusal" {
            return NodeOutput::failure(NodeError::unrecoverable("REFUSED", "The model declined to answer this request."));
        }
        let paused = finish_reason == "pause_turn";
        if paused {
            if content.is_empty() && requested.is_empty() && !continuation.return_messages {
                return NodeOutput::failure(NodeError::unrecoverable(
                    "SEARCH_PAUSED",
                    "Anthropic paused the reply while it was searching, before any text was written. Run the node again, or lower web_search_max_uses.",
                ));
            }
            reasoning_trace.push(format!(
                "[Warning] Anthropic paused the reply while it was searching, so the answer is incomplete. {}",
                if continuation.return_messages {
                    "Pass messages to another AI Agent node to let it continue, or lower web_search_max_uses."
                } else {
                    "Run the node again, or lower web_search_max_uses."
                }
            ));
        }
        let grounding = if is_anthropic {
            anthropic_grounding(&resp)
        } else {
            openai_grounding(&resp["choices"][0]["message"])
        };

        let truncated = finish_reason == "length" || finish_reason == "max_tokens";
        if truncated && content.is_empty() && requested.is_empty() {
            return NodeOutput::failure(NodeError::unrecoverable(
                "OUTPUT_TRUNCATED",
                format!("The model used all {max_tokens} tokens before writing a reply (reasoning models count their thinking). Raise max_tokens."),
            ));
        }
        if truncated && !requested.is_empty() {
            reasoning_trace.push("[Warning] The reply was cut off at max_tokens; the last tool request may be incomplete. Raise max_tokens.".to_string());
        }
        let finished = matches!(finish_reason.as_str(), "stop" | "end_turn") && !truncated && requested.is_empty();
        let transcript = capture_transcript(
            continuation.return_messages, provider_name, messages, assistant,
            paused, truncated && !requested.is_empty(), &mut reasoning_trace,
        );
        agent_output(
            AgentReply { content, requested, finished, truncated, usage, tool_required: controls.requires_tool(&tools), response_schema, grounding, transcript },
            &tools,
            reasoning_trace,
        )
    }
}
