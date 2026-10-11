// Image Generation Node
//
// Providers (config string → implementation):
//
//   "gpt_image_1" / "dalle3" → GPT Image 1 (recommended OpenAI option)
//     POST https://api.openai.com/v1/images/generations, model="gpt-image-1"
//     n>1: one request per image, results merged (keeps each response under the
//     10 MB cap). Returns data[].b64_json by default.
//     size: 1024x1024 | 1536x1024 | 1024x1536 | auto  (different from DALL-E 3 sizes)
//     quality: auto | low | medium | high
//     "dalle3" alias → gpt-image-1. DALL-E 3 retired May 12 2026.
// OpenAI API reference (developers.openai.com/api/reference), June 2026
//
//   "gpt_image_2" → GPT Image 2
//     Same endpoint, model="gpt-image-2". n>1: one request per image.
//     size: arbitrary WxH divisible by 16 (ratio 1:3-3:1, max 2560x1440), or same presets.
// OpenAI API reference (developers.openai.com/api/reference), June 2026
//
//   "imagen4" / "nano_banana" → NanoBanana (Gemini image model from the `model` field:
//     gemini-2.5-flash-image (default), gemini-nano-banana-2.1, gemini-3.1-flash-lite-image)
//     POST https://generativelanguage.googleapis.com/v1beta/models/
//          {model}:generateContent
//     Auth: x-goog-api-key header
//     Body: {contents:[{parts:[{text}]}], generationConfig:{responseModalities:["IMAGE"],
//            imageConfig:{aspectRatio}}}
//     Response: candidates[0].content.parts[].inlineData.{data, mimeType}
//     n>1: loop (no native batch). Partial success when only some images fail.
//     output.source is the configured alias ("imagen4" or "nano_banana").
//     Safety blocks and text-only replies arrive as HTTP 200 and become the named
//     errors PROMPT_BLOCKED, IMAGE_BLOCKED and NO_IMAGE_RETURNED.
//     "imagen4" alias preserved — the Imagen 4 direct API shut down Aug 17 2026.
//     gemini-2.5-flash-image: Google's deprecations page (updated 2026-10-09) lists
//     its earliest shutdown as March 15 2027, replacement gemini-3.1-flash-lite-image.
//     Error replies map to named codes (INVALID_API_KEY, API_KEY_LEAKED, PROJECT_DENIED,
//     API_KEY_RESTRICTED, PERMISSION_DENIED, REGION_NOT_SUPPORTED) by message text.
// Google AI developer docs (ai.google.dev/gemini-api/docs/deprecations and
//               /image-generation), October 2026
//
//   "flux_pro" → Black Forest Labs FLUX1.1 [pro]
//     POST https://api.bfl.ai/v1/flux-pro-1.1
//     Auth: x-key header. width/height: 256-1440, multiple of 32 (enforced here).
//     ASYNC: POST -> {id, polling_url}. GET polling_url until "Ready" or terminal status.
//     result.sample URL (expires 10 min) -> download (3 tries) -> base64. n>1: loop.
// docs.bfl.ml OpenAPI spec, June 2026
//
//   "flux_2_pro" → Black Forest Labs FLUX.2 [pro]
//     POST https://api.bfl.ai/v1/flux-2-pro. Same async polling pattern.
//     width/height: min 64 (enforced here).
// docs.bfl.ml OpenAPI spec, June 2026
//
//   "a1111" → Automatic1111 / Stable Diffusion WebUI (local)
//     POST {base_url}/sdapi/v1/txt2img
//     Auth: optional HTTP Basic auth — URL credentials (user:pass@host) or
//           separate username/password config fields (explicit fields take priority)
//     Body: {prompt, negative_prompt, width, height, steps, cfg_scale, batch_size}
//     Response: {images: [base64_string, ...]} — strips data: URI prefix if present;
//     a leading grid image (batch_size > 1) is skipped using info.index_of_first_image
//     n>1: batch_size (native batch, single API call). No api_key required.
//     timeout_seconds (optional, default 300, clamped 10-1800): per-request HTTP
//     timeout override. The shared_ai_client() default (120s) is too short for
//     large batch_size / high step counts on local hardware -- this field lets the
//     request run longer without raising the client-level timeout.
// AUTOMATIC1111/stable-diffusion-webui wiki + API docs, June 2026
//
//   "comfyui" → ComfyUI local server (local)
//     POST {base_url}/prompt with {"prompt": workflow_json} → {"prompt_id": "..."}
//     Poll: GET {base_url}/history/{prompt_id} until status.completed == true
//     Output: traverse outputs nodes, GET {base_url}/view?filename=...&subfolder=...&type=output
//     Preview (type "temp") images are skipped when the workflow also saves images.
//     n is ignored (a log line says so): the workflow decides how many images it saves.
//     SSRF check: validated once on base_url (covers all derived /prompt, /history, /view URLs).
//     No api_key required. Workflow JSON must be in ComfyUI API format (not UI format).
// ComfyUI API reference (runflow.io/blog/comfyui-api-endpoints), June 2026
//
// Output (DATA CONTRACT):
//   { "files": [{"filename":"...","data":"<raw b64>","mime_type":"image/..."}],
//     "count": N, "source": "<provider-string>" }

mod shared;
mod openai;
mod nanobanana;
mod flux;
mod a1111;
mod comfyui;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortArity, PortDefinition, PortPosition};

use self::openai::{gen_gpt_image, GptImageRequest};
use self::nanobanana::{gemini_model_of, gen_nano_banana, GEMINI_MODELS};
use self::flux::{gen_flux, FluxRequest};
use self::a1111::{gen_a1111, A1111Params};
use self::comfyui::gen_comfyui;
use crate::nodes::ai_prompt::extract_port_attachments;
use crate::nodes::util::{cfg_f64_opt, cfg_u64_opt};

const VALID_PROVIDERS: [&str; 9] = [
    "gpt_image_1", "gpt_image_2", "flux_pro", "flux_2_pro", "nano_banana",
    "imagen4", "dalle3", "a1111", "comfyui",
];

const VALID_PROVIDER_HELP: &str = "gpt_image_1, gpt_image_2, flux_pro, flux_2_pro, nano_banana, imagen4 (->NanoBanana), dalle3 (->gpt-image-1), a1111, comfyui";

fn unknown_provider(name: &str) -> NodeError {
    NodeError::unrecoverable(
        "UNKNOWN_PROVIDER",
        format!("Unknown provider '{}'. Valid: {}", name, VALID_PROVIDER_HELP),
    )
}

fn provider_of(cfg: &Value) -> Result<&str, NodeError> {
    match cfg["provider"].as_str().map(str::trim).filter(|p| !p.is_empty()) {
        None => Err(NodeError::unrecoverable(
            "MISSING_PROVIDER",
            format!("provider is required. Valid: {}", VALID_PROVIDER_HELP),
        )),
        Some(p) if VALID_PROVIDERS.contains(&p) => Ok(p),
        Some(p) => Err(unknown_provider(p)),
    }
}

fn cfg_clamped_u64(cfg: &Value, key: &str, default: u64, lo: u64, hi: u64) -> Result<u64, NodeError> {
    Ok(cfg_u64_opt(&cfg[key], key)?.unwrap_or(default).clamp(lo, hi))
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
            "required": ["provider"],
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "Image generation prompt. Required for every provider except comfyui, where the text lives in the workflow JSON."
                },
                "provider": {
                    "type": "string",
                    "enum": ["gpt_image_1", "gpt_image_2", "flux_pro", "flux_2_pro", "imagen4", "nano_banana", "dalle3", "a1111", "comfyui"],
                    "description": "Provider. gpt_image_1 recommended for cloud. a1111/comfyui for local inference. imagen4/nano_banana -> NanoBanana; dalle3 -> gpt-image-1 (legacy aliases)."
                },
                "n": {
                    "type": "number",
                    "description": "Images to generate (1-4, default 1). GPT Image, NanoBanana and Flux send one request per image; a1111 sends one batch. Not used by comfyui: the workflow decides."
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
                "model": {
                    "type": "string",
                    "enum": GEMINI_MODELS,
                    "description": "NanoBanana / Gemini only. Default gemini-2.5-flash-image, which Google shuts down on March 15 2027. gemini-nano-banana-2.1 and gemini-3.1-flash-lite-image (1K images only) are Google's replacements; Google's docs do not confirm they work through this node's request, so a refusal comes back as an API error with Google's message."
                },
                "aspect_ratio": {
                    "type": "string",
                    "description": "NanoBanana / Gemini only. Supported: 1:1 1:4 1:8 2:3 3:2 3:4 4:1 4:3 4:5 5:4 8:1 9:16 16:9 21:9. Default: 1:1"
                },
                "width": {
                    "type": "number",
                    "description": "flux_pro: 256-1440 rounded to nearest 32. flux_2_pro: min 64. a1111: 64-2048 rounded to nearest 8. Not used by other providers. Default: 1024 (a1111: 512)"
                },
                "height": {
                    "type": "number",
                    "description": "flux_pro: 256-1440 rounded to nearest 32. flux_2_pro: min 64. a1111: 64-2048 rounded to nearest 8. Not used by other providers. Default: 768 (flux_pro) / 1024 (flux_2_pro) / 512 (a1111)"
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

    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![
                PortDefinition { id: "input".to_string(),            label: "In".to_string(),               position: PortPosition::Left,  port_type: None, arity: PortArity::Single },
                PortDefinition { id: "reference_images".to_string(), label: "Reference Images".to_string(), position: PortPosition::Left,  port_type: Some("files".to_string()), arity: PortArity::Single },
            ],
            outputs: vec![
                PortDefinition { id: "output".to_string(), label: "Out".to_string(), position: PortPosition::Right, port_type: None, arity: PortArity::Single },
            ],
        }
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let cfg = &input.input;

        // Resolve provider first so local providers can skip api_key and prompt requirements.
        let provider = match provider_of(cfg) {
            Ok(p)  => p,
            Err(e) => return NodeOutput::failure(e),
        };
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

        let n = match cfg_clamped_u64(cfg, "n", 1, 1, 4) {
            Ok(v) => v as usize,
            Err(e) => return NodeOutput::failure(e),
        };

        // Reference images from the "Reference Images" input port.
        // Canvas injects {{SourceNode.output.files}} into config["reference_images_expr"].
        let ref_images = extract_port_attachments(&cfg["reference_images_expr"]);

        match provider {
            // -- NanoBanana / Gemini -----------------------------------------
            // "imagen4" is a legacy config alias preserved for saved-workflow compat.
            "imagen4" | "nano_banana" => {
                let model = match gemini_model_of(cfg) {
                    Ok(m)  => m,
                    Err(e) => return NodeOutput::failure(e),
                };
                let aspect = cfg["aspect_ratio"].as_str().unwrap_or("1:1").to_string();
                gen_nano_banana(crate::provider::shared_remote_ai_client(), model, &prompt, n, &aspect, &api_key, &ref_images, provider).await
            }

            // -- OpenAI GPT Image 1 (recommended) ----------------------------
            // "dalle3" redirected here -- DALL-E 3 was retired May 12 2026.
            // output.source preserves the config alias for workflow compat.
            "gpt_image_1" | "dalle3" => {
                let size    = cfg["size"].as_str().unwrap_or("1024x1024").to_string();
                let quality = cfg["quality"].as_str().unwrap_or("auto").to_string();
                gen_gpt_image(crate::provider::shared_remote_ai_client(), GptImageRequest {
                    model: "gpt-image-1", source: provider, prompt: &prompt, n, size: &size, quality: &quality, api_key: &api_key, ref_images: &ref_images,
                }).await
            }

            // -- OpenAI GPT Image 2 ------------------------------------------
            "gpt_image_2" => {
                let size    = cfg["size"].as_str().unwrap_or("1024x1024").to_string();
                let quality = cfg["quality"].as_str().unwrap_or("auto").to_string();
                gen_gpt_image(crate::provider::shared_remote_ai_client(), GptImageRequest {
                    model: "gpt-image-2", source: provider, prompt: &prompt, n, size: &size, quality: &quality, api_key: &api_key, ref_images: &ref_images,
                }).await
            }

            // -- BFL FLUX1.1 [pro] -------------------------------------------
            "flux_pro" => {
                let raw_w = match cfg_clamped_u64(cfg, "width", 1024, 256, 1440) {
                    Ok(v) => v as u32,
                    Err(e) => return NodeOutput::failure(e),
                };
                let raw_h = match cfg_clamped_u64(cfg, "height", 768, 256, 1440) {
                    Ok(v) => v as u32,
                    Err(e) => return NodeOutput::failure(e),
                };
                // flux-pro-1.1 requires width and height as multiples of 32
                let width  = round_to_multiple(raw_w, 32);
                let height = round_to_multiple(raw_h, 32);
                let mut out = gen_flux(crate::provider::shared_remote_ai_client(), FluxRequest {
                    endpoint: "/v1/flux-pro-1.1", source: "flux_pro", prompt: &prompt, width, height, api_key: &api_key,
                }, n).await;
                if !ref_images.is_empty() {
                    out.logs.insert(0, "[WARN] reference_images ignored: flux_pro does not support image input. Use nano_banana, gpt_image_1, or gpt_image_2 for image editing.".to_string());
                }
                out
            }

            // -- BFL FLUX.2 [pro] --------------------------------------------
            "flux_2_pro" => {
                let width = match cfg_clamped_u64(cfg, "width", 1024, 64, 4096) {
                    Ok(v) => v as u32,
                    Err(e) => return NodeOutput::failure(e),
                };
                let height = match cfg_clamped_u64(cfg, "height", 1024, 64, 4096) {
                    Ok(v) => v as u32,
                    Err(e) => return NodeOutput::failure(e),
                };
                let mut out = gen_flux(crate::provider::shared_remote_ai_client(), FluxRequest {
                    endpoint: "/v1/flux-2-pro", source: "flux_2_pro", prompt: &prompt, width, height, api_key: &api_key,
                }, n).await;
                if !ref_images.is_empty() {
                    out.logs.insert(0, "[WARN] reference_images ignored: flux_2_pro does not support image input. Use nano_banana, gpt_image_1, or gpt_image_2 for image editing.".to_string());
                }
                out
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
                let raw_w = match cfg_clamped_u64(cfg, "width", 512, 64, 2048) {
                    Ok(v) => v as u32,
                    Err(e) => return NodeOutput::failure(e),
                };
                let raw_h = match cfg_clamped_u64(cfg, "height", 512, 64, 2048) {
                    Ok(v) => v as u32,
                    Err(e) => return NodeOutput::failure(e),
                };
                let width  = round_to_multiple(raw_w, 8);
                let height = round_to_multiple(raw_h, 8);
                let size_note = ((width, height) != (raw_w, raw_h)).then(|| {
                    format!("a1111 size {raw_w}x{raw_h} adjusted to {width}x{height} (multiple of 8)")
                });
                let steps = match cfg_clamped_u64(cfg, "steps", 20, 1, 150) {
                    Ok(v) => v as u32,
                    Err(e) => return NodeOutput::failure(e),
                };
                let cfg_scale = match cfg_f64_opt(&cfg["cfg_scale"], "cfg_scale") {
                    Ok(v) => v.unwrap_or(7.0),
                    Err(e) => return NodeOutput::failure(e),
                };
                let username  = cfg["username"].as_str().filter(|s| !s.is_empty()).map(|s| s.to_string());
                let password  = cfg["password"].as_str().filter(|s| !s.is_empty()).map(|s| s.to_string());
                let timeout_secs = match cfg_clamped_u64(cfg, "timeout_seconds", 300, 10, 1800) {
                    Ok(v) => v,
                    Err(e) => return NodeOutput::failure(e),
                };
                let mut out = gen_a1111(crate::provider::shared_ai_client(), A1111Params {
                    base_url, prompt, negative_prompt: neg, n, width, height, steps, cfg_scale, username, password, timeout_secs,
                }).await;
                if let Some(note) = size_note {
                    out.logs.insert(0, note);
                }
                if !ref_images.is_empty() {
                    out.logs.insert(0, "[WARN] reference_images ignored: a1111 txt2img does not support image input. Use nano_banana, gpt_image_1, or gpt_image_2 for image editing.".to_string());
                }
                out
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
                let mut out = gen_comfyui(crate::provider::shared_ai_client(), &base_url, workflow).await;
                if n > 1 {
                    out.logs.insert(0, "[WARN] n ignored: comfyui returns the images its workflow saves. Set the batch size inside the workflow.".to_string());
                }
                if !ref_images.is_empty() {
                    out.logs.insert(0, "[WARN] reference_images ignored: comfyui does not support image input via this port. Embed image nodes directly in your ComfyUI workflow JSON.".to_string());
                }
                out
            }

            other => NodeOutput::failure(unknown_provider(other)),
        }
    }
}

/// Nearest multiple of `step`; a tie rounds up.
fn round_to_multiple(raw: u32, step: u32) -> u32 {
    ((raw + step / 2) / step) * step
}

#[cfg(test)]
mod tests {
    #[test]
    fn sizes_round_to_the_nearest_multiple_and_ties_go_up() {
        assert_eq!(round_to_multiple(515, 8), 512);
        assert_eq!(round_to_multiple(516, 8), 520);
        assert_eq!(round_to_multiple(512, 8), 512);
        assert_eq!(round_to_multiple(64, 8), 64);
        assert_eq!(round_to_multiple(1439, 32), 1440);
        assert_eq!(round_to_multiple(1423, 32), 1408);
        assert_eq!(round_to_multiple(1424, 32), 1440);
        assert_eq!(round_to_multiple(1440, 32), 1440);
    }

    use super::*;
    use crate::model::ExecutionContext;

    async fn run(cfg: Value) -> NodeOutput {
        ImageGenNode
            .execute(NodeInput {
                resolved_credentials: std::collections::HashMap::new(),
                cancel_token: None,
                node_id:      "test_node".to_string(),
                workflow_id:  String::new(),
                execution_id: "exec".to_string(),
                input:        cfg,
                context:      ExecutionContext::default(),
            })
            .await
    }

    fn code(out: &NodeOutput) -> String {
        out.error.as_ref().expect("must fail").code.clone()
    }

    #[tokio::test]
    async fn an_unknown_provider_is_reported_before_the_missing_key_and_names_the_valid_ones() {
        let out = run(json!({ "provider": "dall-e", "prompt": "a cat" })).await;
        assert_eq!(code(&out), "UNKNOWN_PROVIDER");
        let e = out.error.unwrap();
        assert!(!e.recoverable);
        assert!(e.message.contains("dall-e") && e.message.contains("gpt_image_1") && e.message.contains("comfyui"), "{}", e.message);
    }

    #[tokio::test]
    async fn a_missing_or_blank_provider_fails_instead_of_defaulting() {
        for cfg in [json!({ "prompt": "a cat", "api_key": "k" }), json!({ "provider": "  ", "prompt": "a cat", "api_key": "k" })] {
            let out = run(cfg).await;
            assert_eq!(code(&out), "MISSING_PROVIDER");
            assert!(out.error.unwrap().message.contains("gpt_image_1"));
        }
    }

    #[tokio::test]
    async fn a_known_provider_still_gets_the_prompt_and_key_checks() {
        assert_eq!(code(&run(json!({ "provider": "flux_pro" })).await), "MISSING_PROMPT");
        assert_eq!(code(&run(json!({ "provider": "flux_pro", "prompt": "a cat" })).await), "MISSING_API_KEY");
    }

    #[tokio::test]
    async fn an_unknown_gemini_model_fails_before_any_request_is_sent() {
        let out = run(json!({ "provider": "nano_banana", "prompt": "a cat", "api_key": "k", "model": "gemini-9" })).await;
        assert_eq!(code(&out), "UNKNOWN_MODEL");
    }

    #[test]
    fn every_schema_provider_is_known_and_prompt_is_not_required_by_the_schema() {
        let schema = ImageGenNode.input_schema();
        let listed: Vec<&str> = schema["properties"]["provider"]["enum"].as_array().unwrap().iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(listed.len(), VALID_PROVIDERS.len());
        assert!(listed.iter().all(|p| VALID_PROVIDERS.contains(p)));
        assert_eq!(schema["required"], json!(["provider"]));
    }
}
