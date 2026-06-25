// Image Generation Node
//
// Providers (config string → implementation):
//
//   "gpt_image_1" / "dalle3" → GPT Image 1 (recommended OpenAI option)
//     POST https://api.openai.com/v1/images/generations, model="gpt-image-1"
//     n=1-10 native batch (one call). Returns data[].b64_json by default.
//     size: 1024x1024 | 1536x1024 | 1024x1536 | auto  (different from DALL-E 3 sizes)
//     quality: auto | low | medium | high
//     "dalle3" alias → gpt-image-1. DALL-E 3 retired May 12 2026.
//     VERIFIED: OpenAI API reference (developers.openai.com/api/reference), June 2026
//
//   "gpt_image_2" → GPT Image 2
//     Same endpoint, model="gpt-image-2". n=1-8 native.
//     size: arbitrary WxH divisible by 16 (ratio 1:3-3:1, max 2560x1440), or same presets.
//     VERIFIED: OpenAI API reference (developers.openai.com/api/reference), June 2026
//
//   "imagen4" / "nano_banana" → NanoBanana (gemini-2.5-flash-image)
//     POST https://generativelanguage.googleapis.com/v1beta/models/
//          gemini-2.5-flash-image:generateContent
//     Auth: x-goog-api-key header
//     Body: {contents:[{parts:[{text}]}], generationConfig:{responseModalities:["IMAGE"],
//            imageConfig:{aspectRatio}}}
//     Response: candidates[0].content.parts[].inlineData.{data, mimeType}
//     n>1: loop (no native batch). Partial success on failure (G4 pattern).
//     "imagen4" alias preserved — Imagen 4 direct API shuts down Aug 17 2026.
//     NOTE: Google now recommends gemini-3.1-flash-image over gemini-2.5-flash-image.
//           API shapes identical. Upgrade = change model name in endpoint URL only.
//     VERIFIED: Google AI developer docs (ai.google.dev/gemini-api/docs/image-generation),
//               June 2026
//
//   "flux_pro" → Black Forest Labs FLUX1.1 [pro]
//     POST https://api.bfl.ai/v1/flux-pro-1.1
//     Auth: x-key header. width/height: 256-1440, multiple of 32 (enforced here).
//     ASYNC: POST -> {id, polling_url}. GET polling_url until "Ready" or terminal status.
//     result.sample URL (expires 10 min) -> download -> base64. n>1: loop.
//     VERIFIED: docs.bfl.ml OpenAPI spec, June 2026
//
//   "flux_2_pro" → Black Forest Labs FLUX.2 [pro]
//     POST https://api.bfl.ai/v1/flux-2-pro. Same async polling pattern.
//     width/height: min 64, no multipleOf constraint.
//     VERIFIED: docs.bfl.ml OpenAPI spec, June 2026
//
//   "a1111" → Automatic1111 / Stable Diffusion WebUI (local)
//     POST {base_url}/sdapi/v1/txt2img
//     Auth: optional HTTP Basic auth — URL credentials (user:pass@host) or
//           separate username/password config fields (explicit fields take priority)
//     Body: {prompt, negative_prompt, width, height, steps, cfg_scale, batch_size}
//     Response: {images: [base64_string, ...]} — strips data: URI prefix if present
//     n>1: batch_size (native batch, single API call). No api_key required.
//     timeout_seconds (optional, default 300, clamped 10-1800): per-request HTTP
//     timeout override. The shared image_client() default (120s) is too short for
//     large batch_size / high step counts on local hardware -- this field lets the
//     request run longer without raising the timeout for cloud providers sharing
//     the same client.
//     VERIFIED: AUTOMATIC1111/stable-diffusion-webui wiki + API docs, June 2026
//
//   "comfyui" → ComfyUI local server (local)
//     POST {base_url}/prompt with {"prompt": workflow_json} → {"prompt_id": "..."}
//     Poll: GET {base_url}/history/{prompt_id} until status.completed == true
//     Output: traverse outputs nodes, GET {base_url}/view?filename=...&subfolder=...&type=output
//     SSRF check: validated once on base_url (covers all derived /prompt, /history, /view URLs).
//     No api_key required. Workflow JSON must be in ComfyUI API format (not UI format).
//     VERIFIED: ComfyUI API reference (runflow.io/blog/comfyui-api-endpoints), June 2026
//
// Output (DATA CONTRACT - unchanged from v1):
//   { "files": [{"filename":"...","data":"<raw b64>","mime_type":"image/..."}],
//     "count": N, "source": "<provider-string>" }

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use chrono::Utc;
use serde_json::{json, Value};
use std::sync::OnceLock;

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts};
use url::Url;

use crate::nodes::util::{check_host_ssrf_from_url, check_host_ssrf_from_url_allow_local};

static IMAGE_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn image_client() -> reqwest::Client {
    IMAGE_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .pool_max_idle_per_host(10)
            // Redirects disabled: a spoofed API could redirect to an internal
            // address and bypass the SSRF check in download_to_base64.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("Failed to build image generation HTTP client")
    }).clone()
}

pub struct ImageGenNode;

#[async_trait]
impl Node for ImageGenNode {
    fn type_id(&self) -> &'static str { "image_gen" }
    fn display_name(&self) -> &'static str { "Image Generation" }
    fn node_type(&self) -> NodeType { NodeType::Ai }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str {
        "Generate images from a text prompt. Cloud: GPT Image 1/2 (OpenAI), Nano Banana/Gemini (Google), Flux Pro/2 Pro (BFL). Local: Automatic1111, ComfyUI."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["prompt", "provider", "api_key"],
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "Image generation prompt"
                },
                "provider": {
                    "type": "string",
                    "enum": ["gpt_image_1", "gpt_image_2", "flux_pro", "flux_2_pro", "imagen4", "nano_banana", "dalle3", "a1111", "comfyui"],
                    "description": "Provider. gpt_image_1 recommended for cloud. a1111/comfyui for local inference. imagen4/nano_banana -> NanoBanana; dalle3 -> gpt-image-1 (legacy aliases)."
                },
                "n": {
                    "type": "number",
                    "description": "Images to generate (1-4, default 1). GPT Image sends all in one call; NanoBanana and Flux loop per image."
                },
                "size": {
                    "type": "string",
                    "description": "GPT Image only. gpt_image_1: 1024x1024|1536x1024|1024x1536|auto. gpt_image_2: WxH divisible by 16, or same presets. Default: 1024x1024"
                },
                "quality": {
                    "type": "string",
                    "enum": ["auto", "low", "medium", "high"],
                    "description": "GPT Image only. Default: auto"
                },
                "aspect_ratio": {
                    "type": "string",
                    "description": "NanoBanana / Gemini only. Supported: 1:1 1:4 1:8 2:3 3:2 3:4 4:1 4:3 4:5 5:4 8:1 9:16 16:9 21:9. Default: 1:1"
                },
                "width": {
                    "type": "number",
                    "description": "Flux only. flux_pro: 256-1440 rounded to nearest 32. flux_2_pro: min 64. Default: 1024"
                },
                "height": {
                    "type": "number",
                    "description": "Flux only. flux_pro: 256-1440 rounded to nearest 32. flux_2_pro: min 64. Default: 768 (flux_pro) / 1024 (flux_2_pro)"
                },
                "api_key": {
                    "type": "string",
                    "description": "API key from Connections. OpenAI key for GPT Image; Google AI key for NanoBanana; BFL key for Flux. Not used for a1111 or comfyui."
                },
                "base_url": {
                    "type": "string",
                    "description": "Local provider base URL (a1111/comfyui). e.g. http://127.0.0.1:7860 for a1111, http://127.0.0.1:8188 for comfyui. May embed credentials: http://user:pass@host:port"
                },
                "negative_prompt": {
                    "type": "string",
                    "description": "a1111 only. Things to exclude from the image."
                },
                "steps": {
                    "type": "number",
                    "description": "a1111 only. Sampling steps (1-150, default 20)."
                },
                "cfg_scale": {
                    "type": "number",
                    "description": "a1111 only. Classifier-free guidance scale (default 7.0)."
                },
                "username": {
                    "type": "string",
                    "description": "a1111 only. Basic auth username. Overrides any username in base_url."
                },
                "password": {
                    "type": "string",
                    "description": "a1111 only. Basic auth password. Overrides any password in base_url."
                },
                "timeout_seconds": {
                    "type": "number",
                    "description": "a1111 only. HTTP request timeout in seconds. Raise for large batch_size or high step counts on slower hardware. Default 300, max 1800 (30 min)."
                },
                "workflow": {
                    "type": "object",
                    "description": "comfyui only. Workflow in ComfyUI API format (export via Settings → Enable Dev Mode → Save API Format)."
                }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "files":  { "type": "array",  "description": "Generated images as media objects" },
                "count":  { "type": "number", "description": "Number of images successfully generated" },
                "source": { "type": "string", "description": "Provider string that generated the images" }
            }
        })
    }

    fn ports(&self) -> NodePorts { NodePorts::default() }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let cfg = &input.input;

        // Resolve provider first so local providers can skip api_key and prompt requirements.
        let provider = cfg["provider"].as_str().unwrap_or("gpt_image_1");
        let is_local = matches!(provider, "a1111" | "comfyui");

        // comfyui: prompt not required (text is baked into the workflow JSON).
        // a1111 and all cloud providers require prompt.
        let prompt = match cfg["prompt"].as_str() {
            Some(p) if !p.trim().is_empty() => p.to_string(),
            _ => {
                if provider == "comfyui" {
                    String::new()
                } else {
                    return NodeOutput::failure(NodeError::unrecoverable("MISSING_PROMPT", "prompt is required"));
                }
            }
        };

        // api_key required for cloud providers; local providers use base_url + optional basic auth.
        let api_key = if !is_local {
            match cfg["api_key"].as_str() {
                Some(k) if !k.trim().is_empty() => k.to_string(),
                _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_API_KEY", "api_key is required -- add one in Connections")),
            }
        } else {
            String::new()
        };

        let n = cfg["n"].as_u64().unwrap_or(1).clamp(1, 4) as usize;

        match provider {
            // -- NanoBanana / Gemini -----------------------------------------
            // "imagen4" is a legacy config alias preserved for saved-workflow compat.
            "imagen4" | "nano_banana" => {
                let aspect = cfg["aspect_ratio"].as_str().unwrap_or("1:1").to_string();
                gen_nano_banana(image_client(), &prompt, n, &aspect, &api_key).await
            }

            // -- OpenAI GPT Image 1 (recommended) ----------------------------
            // "dalle3" redirected here -- DALL-E 3 was retired May 12 2026.
            // output.source preserves the config alias for workflow compat.
            "gpt_image_1" | "dalle3" => {
                let size    = cfg["size"].as_str().unwrap_or("1024x1024").to_string();
                let quality = cfg["quality"].as_str().unwrap_or("auto").to_string();
                gen_gpt_image(image_client(), GptImageRequest {
                    model: "gpt-image-1", source: provider, prompt: &prompt, n, size: &size, quality: &quality, api_key: &api_key,
                }).await
            }

            // -- OpenAI GPT Image 2 ------------------------------------------
            "gpt_image_2" => {
                let size    = cfg["size"].as_str().unwrap_or("1024x1024").to_string();
                let quality = cfg["quality"].as_str().unwrap_or("auto").to_string();
                gen_gpt_image(image_client(), GptImageRequest {
                    model: "gpt-image-2", source: provider, prompt: &prompt, n, size: &size, quality: &quality, api_key: &api_key,
                }).await
            }

            // -- BFL FLUX1.1 [pro] -------------------------------------------
            "flux_pro" => {
                let raw_w = cfg["width"].as_u64().unwrap_or(1024).clamp(256, 1440) as u32;
                let raw_h = cfg["height"].as_u64().unwrap_or(768).clamp(256, 1440) as u32;
                // flux-pro-1.1 requires width and height as multiples of 32
                let width  = ((raw_w / 32) * 32).max(256);
                let height = ((raw_h / 32) * 32).max(256);
                gen_flux(image_client(), FluxRequest {
                    endpoint: "/v1/flux-pro-1.1", source: "flux_pro", prompt: &prompt, width, height, api_key: &api_key,
                }, n).await
            }

            // -- BFL FLUX.2 [pro] --------------------------------------------
            "flux_2_pro" => {
                let width  = cfg["width"].as_u64().unwrap_or(1024).clamp(64, 4096) as u32;
                let height = cfg["height"].as_u64().unwrap_or(1024).clamp(64, 4096) as u32;
                gen_flux(image_client(), FluxRequest {
                    endpoint: "/v1/flux-2-pro", source: "flux_2_pro", prompt: &prompt, width, height, api_key: &api_key,
                }, n).await
            }

            // -- Automatic1111 / Stable Diffusion WebUI (local) ---------------
            "a1111" => {
                let base_url = match cfg["base_url"].as_str() {
                    Some(u) if !u.trim().is_empty() => u.trim().to_string(),
                    _ => return NodeOutput::failure(NodeError::unrecoverable(
                        "MISSING_BASE_URL",
                        "base_url is required for a1111 (e.g. http://127.0.0.1:7860)",
                    )),
                };
                let neg       = cfg["negative_prompt"].as_str().unwrap_or("").to_string();
                let width     = cfg["width"].as_u64().unwrap_or(512).clamp(64, 2048) as u32;
                let height    = cfg["height"].as_u64().unwrap_or(512).clamp(64, 2048) as u32;
                let steps     = cfg["steps"].as_u64().unwrap_or(20).clamp(1, 150) as u32;
                let cfg_scale = cfg["cfg_scale"].as_f64().unwrap_or(7.0);
                let username  = cfg["username"].as_str().filter(|s| !s.is_empty()).map(|s| s.to_string());
                let password  = cfg["password"].as_str().filter(|s| !s.is_empty()).map(|s| s.to_string());
                let timeout_secs = cfg["timeout_seconds"].as_u64().unwrap_or(300).clamp(10, 1800);
                gen_a1111(image_client(), A1111Params {
                    base_url, prompt, negative_prompt: neg, n, width, height, steps, cfg_scale, username, password, timeout_secs,
                }).await
            }

            // -- ComfyUI (local) ---------------------------------------------
            "comfyui" => {
                let base_url = match cfg["base_url"].as_str() {
                    Some(u) if !u.trim().is_empty() => u.trim().to_string(),
                    _ => return NodeOutput::failure(NodeError::unrecoverable(
                        "MISSING_BASE_URL",
                        "base_url is required for comfyui (e.g. http://127.0.0.1:8188)",
                    )),
                };
                let workflow = match cfg.get("workflow") {
                    Some(w) if !w.is_null() && w.as_object().map(|m| !m.is_empty()).unwrap_or(false) => w.clone(),
                    _ => return NodeOutput::failure(NodeError::unrecoverable(
                        "MISSING_WORKFLOW",
                        "workflow is required for comfyui -- export your workflow in API format via Settings → Enable Dev Mode → Save (API Format)",
                    )),
                };
                gen_comfyui(image_client(), &base_url, workflow).await
            }

            other => NodeOutput::failure(NodeError::unrecoverable(
                "UNKNOWN_PROVIDER",
                format!(
                    "Unknown provider '{}'. Valid: gpt_image_1, gpt_image_2, flux_pro, flux_2_pro, nano_banana, imagen4 (->NanoBanana), dalle3 (->gpt-image-1), a1111, comfyui",
                    other
                ),
            )),
        }
    }
}

// -- NanoBanana (Google gemini-2.5-flash-image) -------------------------------
//
// Config aliases "imagen4" and "nano_banana" both route here.
// No native batch -- loop one request per image; partial success per G4 pattern.
// output.source = "imagen4" always (DATA CONTRACT: preserves saved-workflow compat).

async fn gen_nano_banana(
    client: reqwest::Client,
    prompt: &str,
    n: usize,
    aspect_ratio: &str,
    api_key: &str,
) -> NodeOutput {
    let ts = Utc::now().timestamp_millis();
    let mut files: Vec<Value>     = Vec::new();
    let mut logs:  Vec<String>    = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    let mut last_error: Option<NodeError> = None;

    for i in 0..n {
        match call_nano_banana_once(&client, prompt, aspect_ratio, api_key, ts, i).await {
            Ok(media_obj) => {
                logs.push(format!("NanoBanana image {}/{} generated", i + 1, n));
                files.push(media_obj);
            }
            Err(e) => {
                let msg = format!("NanoBanana image {}/{} failed: [{}] {}", i + 1, n, e.code, e.message);
                logs.push(msg.clone());
                failures.push(msg);
                let is_terminal = e.code == "INVALID_API_KEY" || e.code == "CONTENT_POLICY_VIOLATION";
                last_error = Some(e);
                if is_terminal { break; }
            }
        }
    }

    if files.is_empty() {
        let err = last_error.unwrap_or_else(|| {
            NodeError::unrecoverable("ALL_IMAGES_FAILED", failures.join("; "))
        });
        return NodeOutput::failure_with_logs(err, logs);
    }

    if !failures.is_empty() {
        logs.push(format!(
            "Partial success: {}/{} images generated. {} failed.",
            files.len(), n, failures.len()
        ));
    }

    NodeOutput::success_with_logs(
        json!({ "files": files, "count": files.len(), "source": "imagen4" }),
        logs,
    )
}

async fn call_nano_banana_once(
    client: &reqwest::Client,
    prompt: &str,
    aspect_ratio: &str,
    api_key: &str,
    ts: i64,
    index: usize,
) -> Result<Value, NodeError> {
    let body = json!({
        "contents": [{ "parts": [{ "text": prompt }] }],
        "generationConfig": {
            "responseModalities": ["IMAGE"],
            "imageConfig": { "aspectRatio": aspect_ratio }
        }
    });

    let resp = client
        .post("https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash-image:generateContent")
        .header("x-goog-api-key", api_key)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(network_err)?;

    let status = resp.status().as_u16();
    let json: Value = resp.json().await.map_err(|e| {
        NodeError::unrecoverable("PARSE_ERROR", e.to_string())
    })?;

    if let Some(err) = json["error"].as_object() {
        let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("Unknown error").to_string();
        return Err(match status {
            429 => NodeError::recoverable("RATE_LIMITED", format!("NanoBanana rate limit exceeded. {}", msg)),
            401 | 403 => NodeError::unrecoverable("INVALID_API_KEY", "Invalid or unauthorized Google AI API key. Check your key in Connections."),
            400 if msg.to_ascii_lowercase().contains("safety") || msg.to_ascii_lowercase().contains("policy") => {
                NodeError::unrecoverable("CONTENT_POLICY_VIOLATION", format!("Prompt rejected by safety filters: {}", msg))
            }
            _ => NodeError::unrecoverable("API_ERROR", msg),
        });
    }

    let parts = match json["candidates"][0]["content"]["parts"].as_array() {
        Some(p) if !p.is_empty() => p,
        _ => return Err(NodeError::unrecoverable(
            "EMPTY_RESPONSE",
            "NanoBanana returned no candidates or content parts",
        )),
    };

    // Parts may contain text alongside image -- find the first inlineData entry.
    for part in parts {
        if let Some(inline) = part.get("inlineData") {
            let b64 = inline["data"].as_str().unwrap_or("");
            if b64.is_empty() { continue; }
            let mime     = inline["mimeType"].as_str().unwrap_or("image/png");
            let ext      = mime_to_ext(mime);
            let filename = format!("imagen4_{}_{}.{}", ts, index, ext);
            return Ok(json!({
                "filename":  filename,
                "data":      strip_data_uri_prefix(b64),
                "mime_type": mime
            }));
        }
    }

    Err(NodeError::unrecoverable(
        "EMPTY_RESPONSE",
        "NanoBanana returned no inlineData in response parts",
    ))
}

// -- OpenAI GPT Image (gpt-image-1 and gpt-image-2) --------------------------
//
// Single API call with n -- both models support native batching.
//   gpt-image-1: n=1-10   gpt-image-2: n=1-8
// Returns data[].b64_json by default; no response_format parameter needed.
// output.source = config alias string (preserves "dalle3" for backward compat).

// Bundles gen_gpt_image's per-request fields -- clippy::too_many_arguments
// (max 7) was exceeded (8 args).
struct GptImageRequest<'a> {
    model:   &'a str, // actual API model string: "gpt-image-1" or "gpt-image-2"
    source:  &'a str, // config alias used for output.source (preserves "dalle3" etc.)
    prompt:  &'a str,
    n:       usize,
    size:    &'a str,
    quality: &'a str,
    api_key: &'a str,
}

async fn gen_gpt_image(client: reqwest::Client, req: GptImageRequest<'_>) -> NodeOutput {
    let ts = Utc::now().timestamp_millis();

    let body = json!({
        "model":   req.model,
        "prompt":  req.prompt,
        "n":       req.n,
        "size":    req.size,
        "quality": req.quality
    });

    let resp = match client
        .post("https://api.openai.com/v1/images/generations")
        .header("Authorization", format!("Bearer {}", req.api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
    {
        Ok(r)  => r,
        Err(e) => return NodeOutput::failure(network_err(e)),
    };

    let status = resp.status().as_u16();
    let json: Value = match resp.json().await {
        Ok(v)  => v,
        Err(e) => return NodeOutput::failure(NodeError::unrecoverable("PARSE_ERROR", e.to_string())),
    };

    if let Some(err) = json["error"].as_object() {
        return NodeOutput::failure(map_openai_image_error(status, err));
    }

    let data = match json["data"].as_array() {
        Some(arr) if !arr.is_empty() => arr,
        _ => return NodeOutput::failure(NodeError::unrecoverable(
            "EMPTY_RESPONSE",
            "GPT Image API returned empty or missing data array",
        )),
    };

    let mut files: Vec<Value>  = Vec::new();
    let mut logs:  Vec<String> = Vec::new();

    for (i, item) in data.iter().enumerate() {
        if let Some(b64) = item["b64_json"].as_str() {
            let filename = format!("{}_{}_{}.png", req.source, ts, i);
            files.push(json!({
                "filename":  filename,
                "data":      strip_data_uri_prefix(b64),
                "mime_type": "image/png"
            }));
            logs.push(format!("GPT Image {}/{} generated", i + 1, data.len()));
        } else {
            logs.push(format!("GPT Image item {} missing b64_json -- skipped", i));
        }
    }

    if files.is_empty() {
        return NodeOutput::failure_with_logs(
            NodeError::unrecoverable("EMPTY_RESPONSE", "GPT Image returned no b64_json in any data item"),
            logs,
        );
    }

    NodeOutput::success_with_logs(
        json!({ "files": files, "count": files.len(), "source": req.source }),
        logs,
    )
}

fn map_openai_image_error(status: u16, err: &serde_json::Map<String, Value>) -> NodeError {
    let msg      = err.get("message").and_then(|m| m.as_str()).unwrap_or("Unknown error").to_string();
    let code     = err.get("code").and_then(|c| c.as_str()).unwrap_or("");
    let err_type = err.get("type").and_then(|t| t.as_str()).unwrap_or("");

    match status {
        429 => NodeError::recoverable("RATE_LIMITED", format!("OpenAI rate limit exceeded. {}", msg)),
        400 if code == "content_policy_violation" || err_type.contains("content_policy") => {
            NodeError::unrecoverable("CONTENT_POLICY_VIOLATION", format!("Prompt rejected by content policy: {}", msg))
        }
        401 => NodeError::unrecoverable("INVALID_API_KEY", "Invalid or expired OpenAI API key. Check your key in Connections."),
        _   => NodeError::unrecoverable("API_ERROR", msg),
    }
}

// -- BFL FLUX (flux-pro-1.1 and flux-2-pro) -----------------------------------
//
// Both models share the same async polling protocol:
//   1. POST submit -> { id, polling_url }
//   2. GET polling_url (x-key auth) until status == "Ready" or terminal failure
//   3. Download result.sample (expires 10 min) -> base64 -> return in contract
// n>1: loop sequentially (no native batch endpoint).

const BFL_POLL_MAX_ITERS: u32  = 240;  // 120 s at 500 ms per poll
const BFL_POLL_INTERVAL_MS: u64 = 500;

// Bundles the per-request fields shared by gen_flux and call_flux_one --
// clippy::too_many_arguments (max 7) was exceeded by both (8 and 9 args).
struct FluxRequest<'a> {
    endpoint: &'a str, // "/v1/flux-pro-1.1" or "/v1/flux-2-pro"
    source:   &'a str, // "flux_pro" or "flux_2_pro"
    prompt:   &'a str,
    width:    u32,
    height:   u32,
    api_key:  &'a str,
}

async fn gen_flux(client: reqwest::Client, req: FluxRequest<'_>, n: usize) -> NodeOutput {
    let ts = Utc::now().timestamp_millis();
    let mut files: Vec<Value>     = Vec::new();
    let mut logs:  Vec<String>    = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    let mut last_error: Option<NodeError> = None;

    for i in 0..n {
        match call_flux_one(&client, &req, ts, i).await {
            Ok(media_obj) => {
                logs.push(format!("{} image {}/{} generated", req.source, i + 1, n));
                files.push(media_obj);
            }
            Err(e) => {
                let msg = format!("{} image {}/{} failed: [{}] {}", req.source, i + 1, n, e.code, e.message);
                logs.push(msg.clone());
                failures.push(msg);
                let is_terminal = matches!(e.code.as_str(), "INVALID_API_KEY" | "INSUFFICIENT_CREDITS");
                last_error = Some(e);
                if is_terminal { break; }
            }
        }
    }

    if files.is_empty() {
        let err = last_error.unwrap_or_else(|| {
            NodeError::unrecoverable("ALL_IMAGES_FAILED", failures.join("; "))
        });
        return NodeOutput::failure_with_logs(err, logs);
    }

    if !failures.is_empty() {
        logs.push(format!(
            "Partial success: {}/{} images generated. {} failed.",
            files.len(), n, failures.len()
        ));
    }

    NodeOutput::success_with_logs(
        json!({ "files": files, "count": files.len(), "source": req.source }),
        logs,
    )
}

async fn call_flux_one(
    client: &reqwest::Client,
    req: &FluxRequest<'_>,
    ts: i64,
    index: usize,
) -> Result<Value, NodeError> {
    let url = format!("https://api.bfl.ai{}", req.endpoint);

    let body = json!({
        "prompt":        req.prompt,
        "width":         req.width,
        "height":        req.height,
        "output_format": "png"
    });

    let resp = client
        .post(&url)
        .header("x-key", req.api_key)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(network_err)?;

    let status = resp.status().as_u16();
    let json: Value = resp.json().await.map_err(|e| {
        NodeError::unrecoverable("PARSE_ERROR", e.to_string())
    })?;

    if status >= 400 {
        // BFL validation errors use "detail" (string or array); other errors use "message".
        let detail = match &json["detail"] {
            Value::String(s) => s.clone(),
            Value::Null      => json["message"].as_str().unwrap_or("Unknown error").to_string(),
            other            => other.to_string(),
        };
        return Err(match status {
            401 | 403 => NodeError::unrecoverable("INVALID_API_KEY", "Invalid or unauthorized BFL API key. Check your key in Connections."),
            402        => NodeError::unrecoverable("INSUFFICIENT_CREDITS", "Insufficient BFL API credits. Add credits at bfl.ai."),
            422        => NodeError::unrecoverable("VALIDATION_ERROR", format!("Invalid BFL request parameters: {}", detail)),
            429        => NodeError::recoverable("RATE_LIMITED", format!("BFL rate limit exceeded. {}", detail)),
            _          => NodeError::unrecoverable("API_ERROR", format!("BFL API error {}: {}", status, detail)),
        });
    }

    let polling_url = json["polling_url"]
        .as_str()
        .ok_or_else(|| NodeError::unrecoverable("PROTOCOL_ERROR", "BFL response missing polling_url"))?
        .to_string();

    let image_url = bfl_poll(client, &polling_url, req.api_key).await?;
    let b64       = download_to_base64(client, &image_url).await?;
    let filename  = format!("{}_{}_{}.png", req.source, ts, index);

    Ok(json!({ "filename": filename, "data": b64, "mime_type": "image/png" }))
}

async fn bfl_poll(
    client: &reqwest::Client,
    polling_url: &str,
    api_key: &str,
) -> Result<String, NodeError> {
    // Validate the polling URL before using it. A compromised or MITM'd API
    // response could supply an internal address (e.g. cloud metadata endpoint).
    check_host_ssrf_from_url(polling_url).await.map_err(|e| {
        NodeError::unrecoverable("SSRF_BLOCKED", e)
    })?;

    for _ in 0..BFL_POLL_MAX_ITERS {
        tokio::time::sleep(std::time::Duration::from_millis(BFL_POLL_INTERVAL_MS)).await;

        let resp = client
            .get(polling_url)
            .header("x-key", api_key)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(network_err)?;

        let json: Value = resp.json().await.map_err(|e| {
            NodeError::unrecoverable("PARSE_ERROR", format!("BFL poll parse error: {}", e))
        })?;

        match json["status"].as_str() {
            Some("Ready") => {
                return json["result"]["sample"]
                    .as_str()
                    .map(|s| s.to_string())
                    .ok_or_else(|| NodeError::unrecoverable(
                        "EMPTY_RESPONSE",
                        "BFL returned Ready but result.sample is missing",
                    ));
            }
            Some(s) if matches!(s, "Error" | "Failed" | "Content Moderated" | "Request Moderated") => {
                let detail = json["result"].as_str()
                    .or_else(|| json["error"].as_str())
                    .unwrap_or("No detail");
                return Err(NodeError::unrecoverable(
                    "GENERATION_FAILED",
                    format!("BFL generation {}: {}", s, detail),
                ));
            }
            _ => {} // Pending / Processing -- continue polling
        }
    }

    Err(NodeError::unrecoverable(
        "TIMEOUT",
        format!(
            "BFL generation timed out after {}s",
            BFL_POLL_MAX_ITERS as u64 * BFL_POLL_INTERVAL_MS / 1000
        ),
    ))
}

// -- Automatic1111 / Stable Diffusion WebUI -----------------------------------
//
// Single POST call with batch_size=n (native batch).
// Optional HTTP Basic auth: URL-embedded credentials (user:pass@host) OR
// separate username/password config fields (explicit fields take priority).
// Response images may carry a data: URI prefix -- stripped via strip_data_uri_prefix.
// timeout_secs overrides the shared image_client()'s 120s default for this request
// only (RequestBuilder::timeout supersedes ClientBuilder::timeout, reqwest VERIFIED
// behavior) -- local batch generation can legitimately exceed 120s.

// Bundles gen_a1111's per-request fields -- clippy::too_many_arguments
// (max 7) was exceeded (12 args).
struct A1111Params {
    base_url:        String,
    prompt:          String,
    negative_prompt: String,
    n:               usize,
    width:           u32,
    height:          u32,
    steps:           u32,
    cfg_scale:       f64,
    username:        Option<String>,
    password:        Option<String>,
    timeout_secs:    u64,
}

async fn gen_a1111(client: reqwest::Client, p: A1111Params) -> NodeOutput {
    // a1111 is a local-only provider -- base_url (e.g. 127.0.0.1:7860) is the
    // user's explicit trust signal for this node, so loopback/private/localhost
    // are permitted here. Link-local, cloud metadata, and RFC 6598 stay blocked
    // (check_ssrf_ip_allow_local in util.rs). Cloud providers in this file keep
    // the strict check_host_ssrf_from_url unchanged.
    if let Err(e) = check_host_ssrf_from_url_allow_local(&p.base_url).await {
        return NodeOutput::failure(NodeError::unrecoverable("SSRF_BLOCKED", e));
    }

    let parsed = match Url::parse(&p.base_url) {
        Ok(u)  => u,
        Err(e) => return NodeOutput::failure(NodeError::unrecoverable(
            "INVALID_BASE_URL",
            format!("Invalid base_url: {}", e),
        )),
    };

    // Extract URL-embedded credentials; explicit config fields take priority.
    let url_user = parsed.username().to_string();
    let url_pass = parsed.password().map(|s| s.to_string());

    let auth_user = p.username.filter(|s| !s.is_empty())
        .or(if url_user.is_empty() { None } else { Some(url_user) });
    let auth_pass = p.password.filter(|s| !s.is_empty()).or(url_pass);

    // Build endpoint URL without embedded credentials.
    let mut clean = parsed.clone();
    clean.set_username("").ok();
    clean.set_password(None).ok();
    let endpoint = format!("{}/sdapi/v1/txt2img", clean.as_str().trim_end_matches('/'));

    let ts   = Utc::now().timestamp_millis();
    let body = json!({
        "prompt":          p.prompt,
        "negative_prompt": p.negative_prompt,
        "width":           p.width,
        "height":          p.height,
        "steps":           p.steps,
        "cfg_scale":       p.cfg_scale,
        "batch_size":      p.n
    });

    let mut req = client.post(&endpoint)
        .json(&body)
        .timeout(std::time::Duration::from_secs(p.timeout_secs));
    if let Some(user) = auth_user {
        req = req.basic_auth(user, auth_pass);
    }

    let resp = match req.send().await {
        Ok(r)  => r,
        Err(e) => return NodeOutput::failure(network_err(e)),
    };

    let status = resp.status().as_u16();
    let json: Value = match resp.json().await {
        Ok(v)  => v,
        Err(e) => return NodeOutput::failure(NodeError::unrecoverable("PARSE_ERROR", e.to_string())),
    };

    if status >= 400 {
        let detail = json["detail"].as_str()
            .or_else(|| json["error"].as_str())
            .unwrap_or("Unknown error")
            .to_string();
        return NodeOutput::failure(match status {
            401 | 403 => NodeError::unrecoverable(
                "AUTH_FAILED",
                "A1111 authentication failed. Check username/password or --api-auth on the server.",
            ),
            422 => NodeError::unrecoverable("VALIDATION_ERROR", format!("A1111 validation error: {}", detail)),
            _   => NodeError::unrecoverable("API_ERROR", format!("A1111 error {}: {}", status, detail)),
        });
    }

    let images = match json["images"].as_array() {
        Some(arr) if !arr.is_empty() => arr,
        _ => return NodeOutput::failure(NodeError::unrecoverable(
            "EMPTY_RESPONSE",
            "A1111 returned no images",
        )),
    };

    let mut files: Vec<Value>  = Vec::new();
    let mut logs:  Vec<String> = Vec::new();

    for (i, item) in images.iter().enumerate() {
        match item.as_str() {
            Some(b64) => {
                let clean_b64 = strip_data_uri_prefix(b64);
                if clean_b64.is_empty() {
                    logs.push(format!("A1111 image {} empty -- skipped", i));
                    continue;
                }
                let filename = format!("a1111_{}_{}.png", ts, i);
                files.push(json!({
                    "filename":  filename,
                    "data":      clean_b64,
                    "mime_type": "image/png"
                }));
                logs.push(format!("A1111 image {}/{} generated", i + 1, images.len()));
            }
            None => logs.push(format!("A1111 image {} not a string -- skipped", i)),
        }
    }

    if files.is_empty() {
        return NodeOutput::failure_with_logs(
            NodeError::unrecoverable("EMPTY_RESPONSE", "A1111 returned no valid base64 images"),
            logs,
        );
    }

    NodeOutput::success_with_logs(
        json!({ "files": files, "count": files.len(), "source": "a1111" }),
        logs,
    )
}

// -- ComfyUI ------------------------------------------------------------------
//
// Async poll pattern:
//   1. POST /prompt {prompt: workflow} → {prompt_id}
//   2. GET /history/{prompt_id} until status.completed == true (or error/timeout)
//   3. Traverse outputs nodes → images → GET /view?filename=...&subfolder=...&type=output
// SSRF: validated once on base_url (all derived URLs share the same host).
// workflow must be in ComfyUI API format (not the UI JSON format).

const COMFYUI_POLL_MAX_ITERS: u32  = 300;   // 600 s at 2000 ms per poll
const COMFYUI_POLL_INTERVAL_MS: u64 = 2000;

async fn gen_comfyui(
    client: reqwest::Client,
    base_url: &str,
    workflow: Value,
) -> NodeOutput {
    // ComfyUI is local-only, same rationale as gen_a1111 above. Single check
    // on base_url covers /prompt, /history, and /view (all derive from the
    // same host).
    if let Err(e) = check_host_ssrf_from_url_allow_local(base_url).await {
        return NodeOutput::failure(NodeError::unrecoverable("SSRF_BLOCKED", e));
    }

    let base = base_url.trim_end_matches('/');
    let ts   = Utc::now().timestamp_millis();

    // Submit workflow
    let submit_url = format!("{}/prompt", base);
    let body       = json!({ "prompt": workflow });

    let resp = match client.post(&submit_url).json(&body).send().await {
        Ok(r)  => r,
        Err(e) => return NodeOutput::failure(network_err(e)),
    };

    let status = resp.status().as_u16();
    let json: Value = match resp.json().await {
        Ok(v)  => v,
        Err(e) => return NodeOutput::failure(NodeError::unrecoverable(
            "PARSE_ERROR",
            format!("ComfyUI submit parse error: {}", e),
        )),
    };

    if status >= 400 {
        let detail = json["error"].as_str()
            .or_else(|| json["message"].as_str())
            .unwrap_or("Unknown error");
        return NodeOutput::failure(NodeError::unrecoverable(
            "API_ERROR",
            format!("ComfyUI submit error {}: {}", status, detail),
        ));
    }

    let prompt_id = match json["prompt_id"].as_str() {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => return NodeOutput::failure(NodeError::unrecoverable(
            "PROTOCOL_ERROR",
            "ComfyUI submit response missing prompt_id",
        )),
    };

    // Poll /history/{prompt_id} until completed
    let history_url = format!("{}/history/{}", base, prompt_id);

    let outputs = match comfyui_poll(&client, &history_url, &prompt_id).await {
        Ok(v)  => v,
        Err(e) => return NodeOutput::failure(e),
    };

    // Traverse output nodes → collect images
    let view_base = format!("{}/view", base);
    let mut files: Vec<Value>  = Vec::new();
    let mut logs:  Vec<String> = Vec::new();
    let mut idx = 0usize;

    if let Some(nodes) = outputs.as_object() {
        for (node_id, node_out) in nodes {
            if let Some(images) = node_out["images"].as_array() {
                for img_meta in images {
                    let filename  = img_meta["filename"].as_str().unwrap_or("");
                    let subfolder = img_meta["subfolder"].as_str().unwrap_or("");
                    let img_type  = img_meta["type"].as_str().unwrap_or("output");

                    if filename.is_empty() {
                        logs.push(format!("ComfyUI node {} image missing filename -- skipped", node_id));
                        continue;
                    }

                    match comfyui_fetch_image(&client, &view_base, filename, subfolder, img_type).await {
                        Ok(b64) => {
                            let ext = filename.rsplit('.').next().unwrap_or("png");
                            let mime = match ext {
                                "jpg" | "jpeg" => "image/jpeg",
                                "webp"         => "image/webp",
                                "gif"          => "image/gif",
                                _              => "image/png",
                            };
                            let out_name = format!("comfyui_{}_{}.{}", ts, idx, ext);
                            files.push(json!({
                                "filename":  out_name,
                                "data":      b64,
                                "mime_type": mime
                            }));
                            logs.push(format!("ComfyUI image {} downloaded", filename));
                            idx += 1;
                        }
                        Err(e) => {
                            logs.push(format!(
                                "ComfyUI image {} download failed: [{}] {}",
                                filename, e.code, e.message
                            ));
                        }
                    }
                }
            }
        }
    }

    if files.is_empty() {
        return NodeOutput::failure_with_logs(
            NodeError::unrecoverable("EMPTY_RESPONSE", "ComfyUI workflow produced no images"),
            logs,
        );
    }

    NodeOutput::success_with_logs(
        json!({ "files": files, "count": files.len(), "source": "comfyui" }),
        logs,
    )
}

async fn comfyui_poll(
    client: &reqwest::Client,
    history_url: &str,
    prompt_id: &str,
) -> Result<Value, NodeError> {
    for _ in 0..COMFYUI_POLL_MAX_ITERS {
        tokio::time::sleep(std::time::Duration::from_millis(COMFYUI_POLL_INTERVAL_MS)).await;

        let resp = client
            .get(history_url)
            .send()
            .await
            .map_err(network_err)?;

        let json: Value = resp.json().await.map_err(|e| {
            NodeError::unrecoverable("PARSE_ERROR", format!("ComfyUI history parse error: {}", e))
        })?;

        // Empty object {} means the job is not yet in history (queued or running).
        if json.as_object().map(|m| m.is_empty()).unwrap_or(true) {
            continue;
        }

        let entry = &json[prompt_id];

        let status_str = entry["status"]["status_str"].as_str().unwrap_or("");
        if status_str == "error" {
            let msg = entry["status"]["messages"]
                .as_array()
                .and_then(|msgs| msgs.last())
                .and_then(|m| m.as_array())
                .and_then(|pair| pair.get(1))
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown error");
            return Err(NodeError::unrecoverable(
                "GENERATION_FAILED",
                format!("ComfyUI generation error: {}", msg),
            ));
        }

        if entry["status"]["completed"].as_bool().unwrap_or(false) {
            let outputs = entry["outputs"].clone();
            if outputs.is_null() || outputs.as_object().map(|m| m.is_empty()).unwrap_or(true) {
                return Err(NodeError::unrecoverable(
                    "EMPTY_RESPONSE",
                    "ComfyUI completed but produced no outputs",
                ));
            }
            return Ok(outputs);
        }

        // status_str is "success" but completed is false, or still in progress — keep polling.
    }

    Err(NodeError::unrecoverable(
        "TIMEOUT",
        format!(
            "ComfyUI generation timed out after {}s",
            COMFYUI_POLL_MAX_ITERS as u64 * COMFYUI_POLL_INTERVAL_MS / 1000
        ),
    ))
}

async fn comfyui_fetch_image(
    client: &reqwest::Client,
    view_base: &str,
    filename: &str,
    subfolder: &str,
    img_type: &str,
) -> Result<String, NodeError> {
    // Use query() for proper URL encoding of filename/subfolder values.
    // Host is the same as base_url (SSRF-checked once in gen_comfyui).
    let bytes = client
        .get(view_base)
        .query(&[("filename", filename), ("subfolder", subfolder), ("type", img_type)])
        .send()
        .await
        .map_err(network_err)?
        .bytes()
        .await
        .map_err(|e| NodeError::unrecoverable("DOWNLOAD_ERROR", e.to_string()))?;
    Ok(BASE64.encode(&bytes))
}

// -- Utilities (unchanged from v1) --------------------------------------------

fn network_err(e: reqwest::Error) -> NodeError {
    let recoverable = e.is_timeout() || e.is_connect();
    if recoverable {
        NodeError::recoverable("NETWORK_ERROR", e.to_string())
    } else {
        NodeError::unrecoverable("NETWORK_ERROR", e.to_string())
    }
}

/// Strip a "data:<mime>;base64," prefix if present.
/// DATA CONTRACT requires raw base64 without any data: URI prefix.
fn strip_data_uri_prefix(s: &str) -> &str {
    if let Some(comma_pos) = s.find(',') {
        if s[..comma_pos].starts_with("data:") {
            return &s[comma_pos + 1..];
        }
    }
    s
}

fn mime_to_ext(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp"               => "webp",
        "image/gif"                => "gif",
        _                          => "png",
    }
}

/// Download image from URL -> raw base64 (no data: URI prefix).
/// Validates host against SSRF before fetch.
async fn download_to_base64(client: &reqwest::Client, url: &str) -> Result<String, NodeError> {
    // Validates URL host is not RFC-1918/loopback -- a spoofed API could return
    // an internal address (e.g. AWS metadata) to exfiltrate infrastructure data.
    check_host_ssrf_from_url(url).await.map_err(|e| {
        NodeError::unrecoverable("SSRF_BLOCKED", e)
    })?;
    let bytes = client
        .get(url)
        .send()
        .await
        .map_err(network_err)?
        .bytes()
        .await
        .map_err(|e| NodeError::unrecoverable("DOWNLOAD_ERROR", e.to_string()))?;
    Ok(BASE64.encode(&bytes))
}
