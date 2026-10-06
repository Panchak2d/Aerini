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
// OpenAI API reference (developers.openai.com/api/reference), June 2026
//
//   "gpt_image_2" → GPT Image 2
//     Same endpoint, model="gpt-image-2". n=1-8 native.
//     size: arbitrary WxH divisible by 16 (ratio 1:3-3:1, max 2560x1440), or same presets.
// OpenAI API reference (developers.openai.com/api/reference), June 2026
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
// Google AI developer docs (ai.google.dev/gemini-api/docs/image-generation),
//               June 2026
//
//   "flux_pro" → Black Forest Labs FLUX1.1 [pro]
//     POST https://api.bfl.ai/v1/flux-pro-1.1
//     Auth: x-key header. width/height: 256-1440, multiple of 32 (enforced here).
//     ASYNC: POST -> {id, polling_url}. GET polling_url until "Ready" or terminal status.
//     result.sample URL (expires 10 min) -> download -> base64. n>1: loop.
// docs.bfl.ml OpenAPI spec, June 2026
//
//   "flux_2_pro" → Black Forest Labs FLUX.2 [pro]
//     POST https://api.bfl.ai/v1/flux-2-pro. Same async polling pattern.
//     width/height: min 64, no multipleOf constraint.
// docs.bfl.ml OpenAPI spec, June 2026
//
//   "a1111" → Automatic1111 / Stable Diffusion WebUI (local)
//     POST {base_url}/sdapi/v1/txt2img
//     Auth: optional HTTP Basic auth — URL credentials (user:pass@host) or
//           separate username/password config fields (explicit fields take priority)
//     Body: {prompt, negative_prompt, width, height, steps, cfg_scale, batch_size}
//     Response: {images: [base64_string, ...]} — strips data: URI prefix if present
//     n>1: batch_size (native batch, single API call). No api_key required.
//     timeout_seconds (optional, default 300, clamped 10-1800): per-request HTTP
//     timeout override. The shared_ai_client() default (120s) is too short for
//     large batch_size / high step counts on local hardware -- this field lets the
//     request run longer without raising the timeout for cloud providers sharing
//     the same client.
// AUTOMATIC1111/stable-diffusion-webui wiki + API docs, June 2026
//
//   "comfyui" → ComfyUI local server (local)
//     POST {base_url}/prompt with {"prompt": workflow_json} → {"prompt_id": "..."}
//     Poll: GET {base_url}/history/{prompt_id} until status.completed == true
//     Output: traverse outputs nodes, GET {base_url}/view?filename=...&subfolder=...&type=output
//     SSRF check: validated once on base_url (covers all derived /prompt, /history, /view URLs).
//     No api_key required. Workflow JSON must be in ComfyUI API format (not UI format).
// ComfyUI API reference (runflow.io/blog/comfyui-api-endpoints), June 2026
//
// Output (DATA CONTRACT - unchanged from v1):
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
use self::nanobanana::gen_nano_banana;
use self::flux::{gen_flux, FluxRequest};
use self::a1111::{gen_a1111, A1111Params};
use self::comfyui::gen_comfyui;
use crate::nodes::ai_prompt::extract_port_attachments;

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
            "required": ["prompt", "provider"],
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

        // Reference images from the "Reference Images" input port.
        // Canvas injects {{SourceNode.output.files}} into config["reference_images_expr"].
        let ref_images = extract_port_attachments(&cfg["reference_images_expr"]);

        match provider {
            // -- NanoBanana / Gemini -----------------------------------------
            // "imagen4" is a legacy config alias preserved for saved-workflow compat.
            "imagen4" | "nano_banana" => {
                let aspect = cfg["aspect_ratio"].as_str().unwrap_or("1:1").to_string();
                gen_nano_banana(crate::provider::shared_ai_client(), &prompt, n, &aspect, &api_key, &ref_images).await
            }

            // -- OpenAI GPT Image 1 (recommended) ----------------------------
            // "dalle3" redirected here -- DALL-E 3 was retired May 12 2026.
            // output.source preserves the config alias for workflow compat.
            "gpt_image_1" | "dalle3" => {
                let size    = cfg["size"].as_str().unwrap_or("1024x1024").to_string();
                let quality = cfg["quality"].as_str().unwrap_or("auto").to_string();
                gen_gpt_image(crate::provider::shared_ai_client(), GptImageRequest {
                    model: "gpt-image-1", source: provider, prompt: &prompt, n, size: &size, quality: &quality, api_key: &api_key, ref_images: &ref_images,
                }).await
            }

            // -- OpenAI GPT Image 2 ------------------------------------------
            "gpt_image_2" => {
                let size    = cfg["size"].as_str().unwrap_or("1024x1024").to_string();
                let quality = cfg["quality"].as_str().unwrap_or("auto").to_string();
                gen_gpt_image(crate::provider::shared_ai_client(), GptImageRequest {
                    model: "gpt-image-2", source: provider, prompt: &prompt, n, size: &size, quality: &quality, api_key: &api_key, ref_images: &ref_images,
                }).await
            }

            // -- BFL FLUX1.1 [pro] -------------------------------------------
            "flux_pro" => {
                let raw_w = cfg["width"].as_u64().unwrap_or(1024).clamp(256, 1440) as u32;
                let raw_h = cfg["height"].as_u64().unwrap_or(768).clamp(256, 1440) as u32;
                // flux-pro-1.1 requires width and height as multiples of 32
                let width  = ((raw_w / 32) * 32).max(256);
                let height = ((raw_h / 32) * 32).max(256);
                let mut out = gen_flux(crate::provider::shared_ai_client(), FluxRequest {
                    endpoint: "/v1/flux-pro-1.1", source: "flux_pro", prompt: &prompt, width, height, api_key: &api_key,
                }, n).await;
                if !ref_images.is_empty() {
                    out.logs.insert(0, "[WARN] reference_images ignored: flux_pro does not support image input. Use nano_banana, gpt_image_1, or gpt_image_2 for image editing.".to_string());
                }
                out
            }

            // -- BFL FLUX.2 [pro] --------------------------------------------
            "flux_2_pro" => {
                let width  = cfg["width"].as_u64().unwrap_or(1024).clamp(64, 4096) as u32;
                let height = cfg["height"].as_u64().unwrap_or(1024).clamp(64, 4096) as u32;
                let mut out = gen_flux(crate::provider::shared_ai_client(), FluxRequest {
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
                let width     = cfg["width"].as_u64().unwrap_or(512).clamp(64, 2048) as u32;
                let height    = cfg["height"].as_u64().unwrap_or(512).clamp(64, 2048) as u32;
                let steps     = cfg["steps"].as_u64().unwrap_or(20).clamp(1, 150) as u32;
                let cfg_scale = cfg["cfg_scale"].as_f64().unwrap_or(7.0);
                let username  = cfg["username"].as_str().filter(|s| !s.is_empty()).map(|s| s.to_string());
                let password  = cfg["password"].as_str().filter(|s| !s.is_empty()).map(|s| s.to_string());
                let timeout_secs = cfg["timeout_seconds"].as_u64().unwrap_or(300).clamp(10, 1800);
                let mut out = gen_a1111(crate::provider::shared_ai_client(), A1111Params {
                    base_url, prompt, negative_prompt: neg, n, width, height, steps, cfg_scale, username, password, timeout_secs,
                }).await;
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
                if !ref_images.is_empty() {
                    out.logs.insert(0, "[WARN] reference_images ignored: comfyui does not support image input via this port. Embed image nodes directly in your ComfyUI workflow JSON.".to_string());
                }
                out
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
