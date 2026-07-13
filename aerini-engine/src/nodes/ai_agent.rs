use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortDefinition, PortPosition};
use super::ai_prompt::{process_attachments, extract_port_attachments, ImageAttachment, DocAttachment};

/// AI Agent node — autonomous ReAct loop (Reason → Act → Observe).
///
/// The agent is given a goal and a set of available tools (other node outputs or
/// inline tool definitions). It reasons about what to do, takes an action, observes
/// the result, and repeats until it decides it's done or hits max_iterations.
///
/// **Provider behaviour:**
/// - OpenAI / OpenAI-compatible: full ReAct loop with tool_calls. The agent can call
///   tools across multiple iterations before returning a final answer.
/// - Anthropic: pure reasoning mode — one pass only, no tool loop. Anthropic's
///   tool_use response format is not used here; the agent reasons and returns a
///   final answer in a single call. Connect the output to downstream nodes to
///   take action on the result.
/// - Gemini: full ReAct loop with functionCall/functionResponse. Uses
///   generateContent with functionDeclarations. Supports multi-iteration tool loop.
pub struct AiAgentNode;

#[async_trait]
impl Node for AiAgentNode {
    fn type_id(&self) -> &'static str { "ai_agent" }
    fn display_name(&self) -> &'static str { "AI Agent" }
    fn node_type(&self) -> NodeType { NodeType::Ai }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Give an AI model a set of tools it can invoke to complete a goal autonomously over multiple steps." }

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
                    "description": "JSON array of tool definitions. Each tool: {name, description, parameters}. Used with OpenAI-compatible and Gemini providers. Anthropic runs in pure reasoning mode."
                },
                "context": {
                    "type": "string",
                    "description": "Additional context to give the agent (from previous nodes, variables, etc.)"
                },
                "provider": {
                    "type": "string",
                    "enum": ["openai", "anthropic", "gemini", "auto"],
                    "description": "AI provider. OpenAI/Gemini: full tool-calling ReAct loop. Anthropic: single reasoning pass, no tool loop."
                },
                "model": {
                    "type": "string",
                    "description": "Model name. OpenAI: gpt-4o. Anthropic: claude-opus-4-5. Gemini: gemini-2.5-flash."
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
                    "description": "Maximum think-act-observe cycles (default: 5, max: 20). Ignored for Anthropic (always 1)."
                },
                "max_tokens": {
                    "type": "number",
                    "description": "Maximum tokens per agent response (default: 2048). Higher values allow longer output per cycle but increase cost — total spend scales with max_iterations."
                },
                "temperature": {
                    "type": "number",
                    "description": "Creativity (0.0-1.0). Agents work best at 0.2-0.5."
                }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "result":        { "type": "string", "description": "Final answer or conclusion from the agent" },
                "iterations":    { "type": "number", "description": "Number of reasoning cycles used" },
                "tool_calls":    { "type": "array",  "description": "List of tools the agent called" },
                "reasoning":     { "type": "array",  "description": "Step-by-step reasoning trace" },
                "finished":      { "type": "boolean","description": "True if agent completed goal cleanly, false if hit max_iterations or response was truncated" },
                "truncated":     { "type": "boolean","description": "True if the provider cut off the response mid-generation (max_tokens hit). Raise max_tokens to fix." }
            }
        })
    }

    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![
                PortDefinition { id: "input".to_string(),       label: "In".to_string(),    position: PortPosition::Left, port_type: None },
                // Runtime files — wired from image_gen, collect_files, text_to_file, etc.
                // Merged with config["attachments"] (static design-time files) before the first
                // agent message is built. Same merge/classify pattern as ai_prompt.rs.
                PortDefinition { id: "attachments".to_string(), label: "Files".to_string(), position: PortPosition::Left, port_type: Some("files".to_string()) },
            ],
            outputs: vec![
                PortDefinition { id: "output".to_string(),   label: "Done".to_string(),  position: PortPosition::Right , port_type: None },
                PortDefinition { id: "on_error".to_string(), label: "Error".to_string(), position: PortPosition::Right, port_type: None },
            ],
        }
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let goal = match input.input["goal"].as_str() {
            Some(g) if !g.is_empty() => g.to_string(),
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

        let default_model = if is_anthropic {
            "claude-opus-4-5"
        } else if is_gemini {
            "gemini-2.5-flash"
        } else {
            "gpt-4o"
        };

        let model   = input.input["model"].as_str().unwrap_or(default_model).to_string();
        let api_key = input.input["api_key"].as_str().unwrap_or("").to_string();

        let base_url = crate::provider::ProviderRegistry::resolve_base_url(provider_id, user_url_raw);

        // AllowLocal (not Strict): same fix as ai_prompt/mod.rs (T1-11 / S2-2) —
        // local inference servers (Ollama etc.) must be reachable via base_url.
        // Single check site, upstream of the is_gemini branch, so it covers all
        // three providers (OpenAI-compatible, Anthropic, Gemini).
        if let Err(e) = crate::nodes::util::check_host_ssrf_from_url(&base_url, crate::nodes::util::SsrfPolicy::AllowLocal).await {
            return NodeOutput::failure(NodeError::unrecoverable("SSRF_BLOCKED", e));
        }

        let max_iterations = input.input["max_iterations"].as_u64().unwrap_or(5).min(20) as usize;
        let max_tokens     = input.input["max_tokens"].as_u64().unwrap_or(2048);
        let temperature    = input.input["temperature"].as_f64().unwrap_or(0.3);

        let mut system = input.input["system"].as_str()
            .unwrap_or("You are a helpful AI agent. Complete the given goal step by step. When you have finished, provide a clear final answer.")
            .to_string();

        let context_str = input.input["context"].as_str().unwrap_or("").to_string();

        let tools: Vec<Value> = input.input["tools"]
            .as_str()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();

        let client = crate::provider::shared_ai_client();

        // Attachment handling — same merge/classify pattern as ai_prompt.rs.
        // Two sources:
        //   1. config["attachments"]      — static files added at design time.
        //   2. config["attachments_expr"] — dynamic files from the "Files" input port.
        //      Canvas.ts writes {{SourceNode.output.files}} here when a wire is connected.
        let static_atts: Vec<Value> = input.input["attachments"].as_array().cloned().unwrap_or_default();
        let port_atts: Vec<Value>   = extract_port_attachments(&input.input["attachments_expr"]);
        let mut attachments_raw = static_atts;
        attachments_raw.extend(port_atts);
        let pa = process_attachments(&attachments_raw);
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

        // Gemini uses a completely different message format — delegate to dedicated loop.
        if is_gemini {
            return run_gemini_agent(
                client, base_url, api_key, model, system,
                user_content, tools, max_iterations, max_tokens, temperature,
                image_attachments, doc_attachments, attachment_warnings,
            ).await;
        }

        let mut messages: Vec<Value> = Vec::new();
        // Attachment warnings surface here so they appear in every output's reasoning trace
        // without needing to thread them through every return point in the loop.
        let mut reasoning_trace: Vec<String> = attachment_warnings
            .into_iter()
            .map(|w| format!("[Attachment Warning] {}", w))
            .collect();
        let mut tool_calls_log: Vec<Value> = Vec::new();
        let mut iterations = 0;

        // Build the first user message. When attachments are present, the content
        // must use each provider's multimodal block format. When absent, a plain
        // string is used — identical to the pre-attachment-era shape.
        let first_content: Value = if is_anthropic {
            if image_attachments.is_empty() && doc_attachments.is_empty() {
                json!(user_content)
            } else {
                let mut blocks: Vec<Value> = Vec::new();
                for (mime, data) in &image_attachments {
                    blocks.push(json!({ "type": "image", "source": { "type": "base64", "media_type": mime, "data": data } }));
                }
                for (mime, data, _filename) in &doc_attachments {
                    blocks.push(json!({ "type": "document", "source": { "type": "base64", "media_type": mime, "data": data } }));
                }
                blocks.push(json!({ "type": "text", "text": user_content }));
                json!(blocks)
            }
        } else {
            // OpenAI-compatible: image_url + file blocks
            if image_attachments.is_empty() && doc_attachments.is_empty() {
                json!(user_content)
            } else {
                let mut blocks: Vec<Value> = vec![json!({ "type": "text", "text": user_content })];
                for (mime, data) in &image_attachments {
                    blocks.push(json!({ "type": "image_url", "image_url": { "url": format!("data:{};base64,{}", mime, data) } }));
                }
                for (mime, data, filename) in &doc_attachments {
                    blocks.push(json!({ "type": "file", "file": { "filename": filename, "file_data": format!("data:{};base64,{}", mime, data) } }));
                }
                json!(blocks)
            }
        };
        messages.push(json!({ "role": "user", "content": first_content }));

        loop {
            if iterations >= max_iterations {
                messages.push(json!({
                    "role": "user",
                    "content": "You have reached the maximum number of iterations. Please provide your best final answer based on what you've gathered so far."
                }));
            }

            let response = if is_anthropic {
                call_anthropic_agent(
                    &client, &base_url, &api_key, &model, &system,
                    &messages, temperature, max_tokens,
                ).await
            } else {
                call_openai_agent(
                    &client, &base_url, &api_key, &model, &system,
                    &messages, &tools, temperature, max_tokens,
                ).await
            };

            match response {
                Err(e) => return NodeOutput::failure(NodeError::unrecoverable("API_ERROR", e)),
                Ok(resp) => {
                    let (assistant_message, content, finish_reason) = if is_anthropic {
                        let text = resp["content"][0]["text"].as_str().unwrap_or("").to_string();
                        let stop = resp["stop_reason"].as_str().unwrap_or("end_turn").to_string();
                        let msg = json!({ "role": "assistant", "content": text });
                        (msg, text, stop)
                    } else {
                        let msg = resp["choices"][0]["message"].clone();
                        let text = msg["content"].as_str().unwrap_or("").to_string();
                        let stop = resp["choices"][0]["finish_reason"].as_str().unwrap_or("stop").to_string();
                        (msg, text, stop)
                    };
                    let finish_reason = finish_reason.as_str();
                    let assistant_message = &assistant_message;

                    messages.push(assistant_message.clone());

                    if !content.is_empty() {
                        reasoning_trace.push(format!("[Iteration {}] {}", iterations + 1, content));
                    }

                    let truncated = finish_reason == "length" || finish_reason == "max_tokens";

                    if let Some(calls) = assistant_message["tool_calls"].as_array() {
                        if calls.is_empty() || finish_reason == "stop" || iterations >= max_iterations {
                            let finished = finish_reason == "stop" && !truncated && iterations < max_iterations;
                            return NodeOutput::success_with_logs(
                                json!({
                                    "result": content,
                                    "iterations": iterations + 1,
                                    "tool_calls": tool_calls_log,
                                    "reasoning": reasoning_trace,
                                    "finished": finished,
                                    "truncated": truncated
                                }),
                                vec![format!("Agent completed in {} iteration(s)", iterations + 1)],
                            );
                        }

                        let mut tool_results = Vec::new();
                        for call in calls {
                            let tool_name = call["function"]["name"].as_str().unwrap_or("unknown");
                            let tool_args = call["function"]["arguments"].as_str().unwrap_or("{}");
                            let call_id   = call["id"].as_str().unwrap_or("call_0");

                            tool_calls_log.push(json!({
                                "tool": tool_name,
                                "arguments": tool_args,
                                "iteration": iterations + 1
                            }));
                            reasoning_trace.push(format!("[Tool Call] {} with args: {}", tool_name, tool_args));

                            tool_results.push(json!({
                                "role": "tool",
                                "tool_call_id": call_id,
                                "content": format!("Tool '{}' called with args: {}. (Tool execution is handled by the workflow — connect the agent's output to the appropriate nodes.)", tool_name, tool_args)
                            }));
                        }

                        for result in tool_results {
                            messages.push(result);
                        }

                        iterations += 1;
                    } else {
                        let finished = finish_reason == "stop" || finish_reason == "end_turn";
                        return NodeOutput::success_with_logs(
                            json!({
                                "result": content,
                                "iterations": iterations + 1,
                                "tool_calls": tool_calls_log,
                                "reasoning": reasoning_trace,
                                "finished": finished,
                                "truncated": truncated
                            }),
                            vec![format!("Agent completed in {} iteration(s)", iterations + 1)],
                        );
                    }

                    if iterations >= max_iterations {
                        break;
                    }
                }
            }
        }

        NodeOutput::success_with_logs(
            json!({
                "result": "Agent reached maximum iterations without a definitive conclusion.",
                "iterations": max_iterations,
                "tool_calls": tool_calls_log,
                "reasoning": reasoning_trace,
                "finished": false,
                "truncated": false
            }),
            vec![format!("Agent stopped after {} iterations (limit reached)", max_iterations)],
        )
    }
}

// ── Gemini agent loop ────────────────────────────────────────────────────────
//
// Gemini message format differs fundamentally from OpenAI:
//   - contents[].role is "user" | "model" (not "user" | "assistant" | "tool")
//   - parts[] holds text or functionCall objects
//   - tool results are sent as role:"user" with parts[].functionResponse
//   - tools declared as {"functionDeclarations":[{name, description, parameters}]}
//   - finish is candidates[0].finishReason == "STOP"
//
// Docs: https://ai.google.dev/api/generate-content
//       https://ai.google.dev/gemini-api/docs/function-calling

#[allow(clippy::too_many_arguments)]
async fn run_gemini_agent(
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    system: String,
    user_content: String,
    tools: Vec<Value>,
    max_iterations: usize,
    max_tokens: u64,
    temperature: f64,
    image_attachments: Vec<ImageAttachment>,
    doc_attachments: Vec<DocAttachment>,
    attachment_warnings: Vec<String>,
) -> NodeOutput {
    if api_key.is_empty() {
        return NodeOutput::failure(NodeError::unrecoverable(
            "MISSING_API_KEY",
            "Gemini requires an API key. Add one in Connections.",
        ));
    }

    let mut first_parts: Vec<Value> = vec![json!({ "text": user_content })];
    for (mime, data) in &image_attachments {
        first_parts.push(json!({ "inline_data": { "mime_type": mime, "data": data } }));
    }
    for (mime, data, _filename) in &doc_attachments {
        first_parts.push(json!({ "inline_data": { "mime_type": mime, "data": data } }));
    }
    let mut contents: Vec<Value> = vec![
        json!({ "role": "user", "parts": first_parts }),
    ];
    let mut reasoning_trace: Vec<String> = attachment_warnings
        .into_iter()
        .map(|w| format!("[Attachment Warning] {}", w))
        .collect();
    let mut tool_calls_log: Vec<Value> = Vec::new();
    let mut iterations = 0;

    loop {
        if iterations >= max_iterations {
            contents.push(json!({
                "role": "user",
                "parts": [{ "text": "You have reached the maximum number of iterations. Please provide your best final answer based on what you have gathered so far." }]
            }));
        }

        let resp = match call_gemini_agent(
            &client, &base_url, &api_key, &model, &system,
            &contents, &tools, temperature, max_tokens,
        ).await {
            Ok(r)  => r,
            Err(e) => return NodeOutput::failure(NodeError::unrecoverable("API_ERROR", e)),
        };

        // Push the full model content block verbatim into history for multi-turn continuity.
        let model_content = resp["candidates"][0]["content"].clone();
        if model_content.is_null() {
            return NodeOutput::failure(NodeError::unrecoverable(
                "GEMINI_NO_CONTENT",
                "Gemini returned no content — request may have been blocked by safety filters.",
            ));
        }
        contents.push(model_content.clone());

        let finish_reason = resp["candidates"][0]["finishReason"]
            .as_str()
            .unwrap_or("STOP");

        let empty_parts = vec![];
        let parts = model_content["parts"].as_array().unwrap_or(&empty_parts);

        let text_content: String = parts.iter()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");

        let func_calls: Vec<&Value> = parts.iter()
            .filter(|p| !p["functionCall"].is_null())
            .collect();

        if !text_content.is_empty() {
            reasoning_trace.push(format!("[Iteration {}] {}", iterations + 1, text_content));
        }

        // Done: no function calls, finish signal, or iteration limit reached
        if func_calls.is_empty() || finish_reason == "STOP" || iterations >= max_iterations {
            let result = if text_content.is_empty() {
                "Agent completed without a text response.".to_string()
            } else {
                text_content
            };
            return NodeOutput::success_with_logs(
                json!({
                    "result":     result,
                    "iterations": iterations + 1,
                    "tool_calls": tool_calls_log,
                    "reasoning":  reasoning_trace,
                    "finished":   finish_reason == "STOP",
                    "truncated":  finish_reason == "MAX_TOKENS"
                }),
                vec![format!("Gemini agent completed in {} iteration(s)", iterations + 1)],
            );
        }

        // Build functionResponse parts for next turn.
        // Gemini tool results: role:"user", parts:[{functionResponse:{name, response}}]
        let mut response_parts: Vec<Value> = Vec::new();
        for fc in &func_calls {
            let fn_name = fc["functionCall"]["name"].as_str().unwrap_or("unknown");
            let fn_args = &fc["functionCall"]["args"];

            tool_calls_log.push(json!({
                "tool":      fn_name,
                "arguments": fn_args.to_string(),
                "iteration": iterations + 1
            }));
            reasoning_trace.push(format!("[Tool Call] {} with args: {}", fn_name, fn_args));

            response_parts.push(json!({
                "functionResponse": {
                    "name": fn_name,
                    "response": {
                        "result": format!(
                            "Function '{}' called with args: {}. (Tool execution is handled by the workflow — connect the agent's output to the appropriate nodes.)",
                            fn_name, fn_args
                        )
                    }
                }
            }));
        }

        contents.push(json!({ "role": "user", "parts": response_parts }));
        iterations += 1;

        if iterations >= max_iterations {
            break;
        }
    }

    NodeOutput::success_with_logs(
        json!({
            "result":     "Agent reached maximum iterations without a definitive conclusion.",
            "iterations": max_iterations,
            "tool_calls": tool_calls_log,
            "reasoning":  reasoning_trace,
            "finished":   false,
            "truncated":  false
        }),
        vec![format!("Gemini agent stopped after {} iterations (limit reached)", max_iterations)],
    )
}

// Gemini generateContent HTTP call.
//
// Endpoint: POST {base_url}/models/{model}:generateContent
// Auth:     x-goog-api-key header
// Tools:    {"functionDeclarations":[{name, description, parameters}]}
// System:   top-level "systemInstruction" field
#[allow(clippy::too_many_arguments)]
async fn call_gemini_agent(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    system: &str,
    contents: &[Value],
    tools: &[Value],
    temperature: f64,
    max_tokens: u64,
) -> Result<Value, String> {
    let endpoint = format!("{}/models/{}:generateContent", base_url, model);

    let mut body = json!({
        "contents": contents,
        "generationConfig": {
            "temperature":     temperature,
            "maxOutputTokens": max_tokens
        }
    });

    if !system.is_empty() {
        body["systemInstruction"] = json!({ "parts": [{ "text": system }] });
    }

    // Convert tools from {name, description, parameters} to Gemini functionDeclarations format.
    // Input tools array is already in the right shape — wrap in the required object.
    if !tools.is_empty() {
        body["tools"] = json!([{ "functionDeclarations": tools }]);
    }

    let record = crate::provider::ProviderRegistry::global()
        .get("gemini")
        .expect("gemini always registered");
    // registry-managed: auth header (P14d)
    let response = crate::provider::ProviderRegistry::apply_auth(
        record,
        client.post(&endpoint).header("Content-Type", "application/json"),
        api_key,
    )
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let json: Value = response.json().await.map_err(|e| e.to_string())?;

    if let Some(err) = json["error"].as_object() {
        return Err(err.get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("Unknown Gemini API error")
            .to_string());
    }

    Ok(json)
}

// ── OpenAI-compatible agent call ─────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn call_openai_agent(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    system: &str,
    messages: &[Value],
    tools: &[Value],
    temperature: f64,
    max_tokens: u64,
) -> Result<Value, String> {
    let mut all_messages = vec![json!({ "role": "system", "content": system })];
    all_messages.extend_from_slice(messages);

    let mut body = json!({
        "model": model,
        "messages": all_messages,
        "temperature": temperature,
        "max_tokens": max_tokens
    });

    if !tools.is_empty() {
        body["tools"] = json!(tools);
        body["tool_choice"] = json!("auto");
    }

    let record = crate::provider::ProviderRegistry::global()
        .get("openai")
        .expect("openai always registered");
    // registry-managed: auth header (P14d)
    let req = crate::provider::ProviderRegistry::apply_auth(
        record,
        client.post(format!("{}/chat/completions", base_url))
            .header("Content-Type", "application/json"),
        api_key,
    ).json(&body);

    let response = req.send().await.map_err(|e| e.to_string())?;
    let json: Value = response.json().await.map_err(|e| e.to_string())?;

    if let Some(err) = json["error"].as_object() {
        return Err(err.get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("Unknown API error")
            .to_string());
    }

    Ok(json)
}

// ── Anthropic agent call ──────────────────────────────────────────────────────
//
// Anthropic uses /v1/messages with x-api-key header and a different
// message/response format from OpenAI.
//
// Tools are intentionally omitted — the Anthropic provider runs in pure reasoning
// mode. The agent reasons through the goal in a single pass and returns a final
// answer. It does not iterate via tool_calls. Use OpenAI-compatible providers
// or Gemini if you need a multi-step tool-calling loop.
#[allow(clippy::too_many_arguments)]
async fn call_anthropic_agent(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    system: &str,
    messages: &[Value],
    temperature: f64,
    max_tokens: u64,
) -> Result<Value, String> {
    let body = serde_json::json!({
        "model": model,
        "system": system,
        "messages": messages,
        "temperature": temperature,
        "max_tokens": max_tokens
    });

    let record = crate::provider::ProviderRegistry::global()
        .get("anthropic")
        .expect("anthropic always registered");
    // registry-managed: auth header (P14d)
    let req = crate::provider::ProviderRegistry::apply_auth(
        record,
        client.post(format!("{}/v1/messages", base_url))
            .header("Content-Type", "application/json"),
        api_key,
    ).json(&body);

    let response = req.send().await.map_err(|e| e.to_string())?;
    let json: Value = response.json().await.map_err(|e| e.to_string())?;

    if let Some(err) = json["error"].as_object() {
        return Err(err.get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("Unknown Anthropic API error")
            .to_string());
    }

    Ok(json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExecutionContext;
    use crate::node::Node;

    /// T1-11 (S2-2): same SSRF-policy fix and same regression-proof pattern as
    /// ai_prompt/mod.rs's `loopback_base_url_is_no_longer_ssrf_blocked` — a
    /// loopback `base_url` must reach the local server (any outcome other than
    /// `SSRF_BLOCKED` proves the pre-flight check no longer rejects it).
    async fn spawn_minimal_openai_agent_mock() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock bind failed");
        let port = listener.local_addr().expect("local_addr failed").port();
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
            let (mut stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => return,
            };
            let (r, mut w) = stream.split();
            let mut reader = BufReader::new(r);
            let mut req_line = String::new();
            let _ = reader.read_line(&mut req_line).await;
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).await.unwrap_or(0) == 0 { break; }
                if line.trim().is_empty() { break; }
                if let Some((k, v)) = line.trim().split_once(':') {
                    if k.trim().eq_ignore_ascii_case("content-length") {
                        content_length = v.trim().parse().unwrap_or(0);
                    }
                }
            }
            let mut body = vec![0u8; content_length];
            if content_length > 0 {
                let _ = reader.read_exact(&mut body).await;
            }
            let resp_body = r#"{"choices":[{"message":{"content":"hello"},"finish_reason":"stop"}]}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                resp_body.len(), resp_body
            );
            let _ = w.write_all(resp.as_bytes()).await;
        });
        format!("http://127.0.0.1:{}", port)
    }

    fn make_input(val: serde_json::Value) -> NodeInput {
        NodeInput {
            node_id:      "n1".into(),
            workflow_id:  "w1".into(),
            execution_id: "e1".into(),
            input:        val,
            context:      ExecutionContext::default(),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn loopback_base_url_is_no_longer_ssrf_blocked() {
        let base_url = spawn_minimal_openai_agent_mock().await;
        let out = AiAgentNode.execute(make_input(json!({
            "goal": "say hi",
            "provider": "openai",
            "base_url": base_url
        }))).await;
        if let Some(err) = &out.error {
            assert_ne!(err.code, "SSRF_BLOCKED", "loopback base_url must be allowed under SsrfPolicy::AllowLocal");
        }
        assert!(out.success, "request should reach the local mock server and succeed: {:?}", out.error);
    }
}

