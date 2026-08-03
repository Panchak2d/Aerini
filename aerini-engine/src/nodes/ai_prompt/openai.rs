use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::NodeOutput;

use super::attachments::{ImageAttachment, DocAttachment};
use super::shared::{send_and_parse, extract_err_msg};

// OpenAI Chat Completions — multimodal content blocks
// Docs: developers.openai.com/api/docs/guides/images-vision and .../file-inputs
//   image: {"type":"image_url","image_url":{"url":"data:<mime>;base64,<data>"}}
//   pdf:   {"type":"file","file":{"filename":<name>,"file_data":"data:application/pdf;base64,<data>"}}
// `file` blocks are OpenAI-specific — third-party compatible endpoints (Groq, Ollama)
// may not support them; image_url is broadly supported.

#[allow(clippy::too_many_arguments)]
pub(super) async fn call_openai_compatible(
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
    let user_content: Value = if image_attachments.is_empty() && doc_attachments.is_empty() {
        json!(prompt)
    } else {
        let mut blocks: Vec<Value> = vec![json!({ "type": "text", "text": prompt })];
        for (mime, data) in image_attachments {
            blocks.push(json!({
                "type": "image_url",
                "image_url": { "url": format!("data:{};base64,{}", mime, data) }
            }));
        }
        for (mime, data, filename) in doc_attachments {
            blocks.push(json!({
                "type": "file",
                "file": { "filename": filename, "file_data": format!("data:{};base64,{}", mime, data) }
            }));
        }
        json!(blocks)
    };

    let body = json!({
        "model": model,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user",   "content": user_content }
        ],
        "temperature": temperature,
        "max_tokens":  max_tokens
    });

    let record = crate::provider::ProviderRegistry::global()
        .get("openai")
        .expect("openai always registered");
    let req = crate::provider::ProviderRegistry::apply_auth(
        record,
        client.post(format!("{}/chat/completions", base_url))
            .header("Content-Type", "application/json"),
        api_key,
    ).json(&body);

    let (status, resp_json) = match send_and_parse(req).await {
        Ok(v)  => v,
        Err(e) => return e,
    };

    if let Some(err_obj) = resp_json["error"].as_object() {
        let msg = extract_err_msg(err_obj, "Unknown API error");
        return if status == 429 {
            NodeOutput::failure(NodeError::recoverable("RATE_LIMITED", msg))
        } else {
            NodeOutput::failure(NodeError::unrecoverable("API_ERROR", msg))
        };
    }

    let content    = resp_json["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string();
    let model_used = resp_json["model"].as_str().unwrap_or(model).to_string();
    let input_tok  = resp_json["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
    let output_tok = resp_json["usage"]["completion_tokens"].as_u64().unwrap_or(0);

    NodeOutput::success_with_logs(
        json!({
            "content":       content,
            "model":         model_used,
            "provider":      "openai_compatible",
            "input_tokens":  input_tok,
            "output_tokens": output_tok
        }),
        vec![format!("AI responded ({} chars, {}in/{}out tokens)", content.len(), input_tok, output_tok)],
    )
}


#[cfg(test)]
mod backward_compat_tests {
    use super::*;
    use serde_json::json;

    async fn spawn_capturing_mock_server(
        response_body: &'static str,
    ) -> (String, tokio::sync::oneshot::Receiver<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock server bind failed");
        let port = listener.local_addr().expect("local_addr failed").port();
        let (tx, rx) = tokio::sync::oneshot::channel();

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

            let mut body_bytes = vec![0u8; content_length];
            if content_length > 0 {
                let _ = reader.read_exact(&mut body_bytes).await;
            }
            let _ = tx.send(String::from_utf8_lossy(&body_bytes).to_string());

            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            let _ = w.write_all(resp.as_bytes()).await;
        });

        (format!("http://127.0.0.1:{}", port), rx)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn openai_no_attachments_request_body_matches_pre_attachment_shape() {
        let canned = r#"{"choices":[{"message":{"content":"ok"}}],"model":"test-model","usage":{"prompt_tokens":1,"completion_tokens":1}}"#;
        let (base_url, rx) = spawn_capturing_mock_server(canned).await;

        let client = reqwest::Client::new();
        let out = call_openai_compatible(
            client, &base_url, "", "test-model",
            "You are a helpful assistant.", "hello world",
            0.7, 2048, &[], &[],
        ).await;
        assert!(out.success, "call_openai_compatible failed: {:?}", out.error);

        let captured = tokio::time::timeout(std::time::Duration::from_secs(5), rx)
            .await
            .expect("mock server timed out waiting for a request")
            .expect("mock server's capture channel was dropped");
        let body: Value = serde_json::from_str(&captured)
            .unwrap_or_else(|e| panic!("captured body was not valid JSON: {e}\nbody: {captured}"));

        let expected = json!({
            "model": "test-model",
            "messages": [
                { "role": "system", "content": "You are a helpful assistant." },
                { "role": "user",   "content": "hello world" }
            ],
            "temperature": 0.7,
            "max_tokens": 2048
        });
        assert_eq!(
            body, expected,
            "request body with no attachments must be byte-identical to the pre-attachment-era shape"
        );
    }
}
