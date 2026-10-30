use serde_json::Value;

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput};
use crate::plugin_loader::bindings::wit;

/// Reserved `Param` key carrying the entire merged input as one JSON-encoded
/// object string (see `wit/node.wit`'s `node-input.params` doc comment). Must
/// match that doc comment exactly.
const FULL_INPUT_JSON_PARAM_KEY: &str = "__aerini_input_json";

/// Flatten the merged JSON input into a `Vec<Param>` for the WIT interface, and
/// the resolved credential map into its own `Vec<Param>`.
///
/// The executor merges config and resolved credentials into `input.input` as a
/// JSON object. Each top-level key becomes a `Param`. Non-string values are
/// serialized to their JSON text representation (e.g. numbers, booleans, objects) --
/// kept for plugins written before structured access existed. One additional
/// `FULL_INPUT_JSON_PARAM_KEY` entry carries the whole object as a single JSON
/// string, so a plugin can parse nested config once instead of re-parsing each
/// individually flattened field. `input.resolved_credentials` carries the same
/// secret values again, keyed by config field name; it is flattened separately
/// into `credentials` for plugins that read structured credentials directly.
pub(super) fn engine_input_to_wit(input: &NodeInput) -> wit::NodeInput {
    let params = match &input.input {
        Value::Object(map) => {
            let mut params: Vec<wit::Param> = map
                .iter()
                .map(|(k, v)| wit::Param {
                    key: k.clone(),
                    value: match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    },
                })
                .collect();

            if map.contains_key(FULL_INPUT_JSON_PARAM_KEY) {
                tracing::warn!(
                    "plugin_loader: node config already defines a field named {:?}; \
                     skipping the host-synthesized full-input JSON param to avoid \
                     shadowing the author's own value",
                    FULL_INPUT_JSON_PARAM_KEY
                );
            } else {
                params.push(wit::Param {
                    key: FULL_INPUT_JSON_PARAM_KEY.to_string(),
                    value: input.input.to_string(),
                });
            }

            params
        }
        _ => vec![],
    };

    let credentials: Vec<wit::Param> = input
        .resolved_credentials
        .iter()
        .map(|(k, v)| wit::Param { key: k.clone(), value: v.as_str().to_owned() })
        .collect();

    wit::NodeInput { params, credentials }
}

#[cfg(test)]
mod engine_input_to_wit_tests {
    use super::{engine_input_to_wit, FULL_INPUT_JSON_PARAM_KEY};
    use crate::model::{ExecutionContext, NodeInput};
    use serde_json::json;
    use std::collections::HashMap;

    fn make_input(input: serde_json::Value) -> NodeInput {
        NodeInput {
            cancel_token: None,
            node_id: "n1".to_string(),
            workflow_id: "wf".to_string(),
            execution_id: "exec".to_string(),
            input,
            resolved_credentials: HashMap::new(),
            context: ExecutionContext::default(),
        }
    }

    /// Nested config must survive intact in the reserved full-JSON param,
    /// not just as the pre-existing per-key flattened text.
    #[test]
    fn nested_config_available_as_single_json_param() {
        let input = make_input(json!({
            "url": "https://example.com",
            "options": { "retries": 3, "headers": { "x-a": "b" } },
        }));

        let wit_input = engine_input_to_wit(&input);

        let full = wit_input
            .params
            .iter()
            .find(|p| p.key == FULL_INPUT_JSON_PARAM_KEY)
            .expect("reserved full-input param must be present");
        let parsed: serde_json::Value =
            serde_json::from_str(&full.value).expect("reserved param value must be valid JSON");
        assert_eq!(parsed, input.input, "reserved param must round-trip the whole merged input");

        // Pre-existing flattened behavior is unchanged for backward compat.
        let url = wit_input.params.iter().find(|p| p.key == "url").expect("url param missing");
        assert_eq!(url.value, "https://example.com");
    }

    /// A node config that happens to already use the reserved key must not be
    /// shadowed by the host-synthesized value -- the author's own field wins.
    #[test]
    fn existing_field_with_reserved_key_is_not_overwritten() {
        let input = make_input(json!({ FULL_INPUT_JSON_PARAM_KEY: "author-value" }));

        let wit_input = engine_input_to_wit(&input);

        let matches: Vec<_> =
            wit_input.params.iter().filter(|p| p.key == FULL_INPUT_JSON_PARAM_KEY).collect();
        assert_eq!(matches.len(), 1, "must not add a second entry under the same key");
        assert_eq!(matches[0].value, "author-value", "author's own value must win, not the synthesized JSON blob");
    }

    /// `resolved_credentials` must flatten into `wit::NodeInput.credentials`,
    /// independently of whatever the same secret also did in `params`.
    #[test]
    fn resolved_credentials_becomes_wit_credentials() {
        let mut input = make_input(json!({ "api_key": "sk_live_abc123" }));
        input
            .resolved_credentials
            .insert("api_key".to_string(), crate::executor::SecretString::new("sk_live_abc123".to_string()));

        let wit_input = engine_input_to_wit(&input);

        assert_eq!(wit_input.credentials.len(), 1, "exactly one resolved credential was provided");
        let cred = &wit_input.credentials[0];
        assert_eq!(cred.key, "api_key");
        assert_eq!(cred.value, "sk_live_abc123");

        // Backward-compat merge into params is untouched by this change.
        let param = wit_input.params.iter().find(|p| p.key == "api_key").expect("api_key param missing");
        assert_eq!(param.value, "sk_live_abc123");
    }
}

/// Max chars of a malformed plugin `data` string echoed into the error log
/// (`wit_output_to_engine`) — avoids dumping an arbitrarily large/binary
/// blob into logs while still giving an operator enough to diagnose.
const INVALID_OUTPUT_LOG_PREVIEW_CHARS: usize = 200;

/// Largest `data` string accepted from a plugin: 10 MiB, the same bound the
/// HTTP, S3, JSON and Shell nodes apply to what they return.
const PLUGIN_OUTPUT_LIMIT_BYTES: usize = 10 * 1024 * 1024;

/// Longest error code or message kept from a plugin, in characters.
const PLUGIN_ERROR_TEXT_LIMIT_CHARS: usize = 4096;

fn truncate_chars(text: String, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text;
    }
    text.chars().take(max_chars).collect()
}

/// Convert a WIT `NodeOutput` record to the engine's [`NodeOutput`].
pub(super) fn wit_output_to_engine(out: wit::NodeOutput) -> NodeOutput {
    if out.success {
        if out.data.len() > PLUGIN_OUTPUT_LIMIT_BYTES {
            return NodeOutput::failure(NodeError::unrecoverable(
                "wasm_output_too_large",
                format!(
                    "plugin output is {} bytes, over the {} byte limit",
                    out.data.len(),
                    PLUGIN_OUTPUT_LIMIT_BYTES
                ),
            ));
        }
        match serde_json::from_str::<Value>(&out.data) {
            Ok(value) => NodeOutput::success(value),
            Err(parse_err) => {
                // A plugin reporting success with non-JSON `data` is a hard failure,
                // not an empty result — logged here so it's distinguishable from a
                // plugin that legitimately returns no data.
                let preview: String = out.data.chars().take(INVALID_OUTPUT_LOG_PREVIEW_CHARS).collect();
                tracing::error!(
                    "plugin_loader: plugin reported success but `data` is not valid JSON: {} (data preview: {:?})",
                    parse_err,
                    preview
                );
                NodeOutput::failure(NodeError::unrecoverable(
                    "wasm_invalid_output",
                    format!(
                        "plugin reported success but its output was not valid JSON: {parse_err}"
                    ),
                ))
            }
        }
    } else {
        NodeOutput::failure(NodeError {
            code: truncate_chars(out.error_code, PLUGIN_ERROR_TEXT_LIMIT_CHARS),
            message: truncate_chars(out.error_message, PLUGIN_ERROR_TEXT_LIMIT_CHARS),
            recoverable: out.recoverable,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Edge case: a successful plugin result over the size limit becomes a
    /// failure, and an over-long error message is cut; ordinary output passes.
    #[test]
    fn plugin_output_over_the_limit_is_rejected_and_error_text_is_cut() {
        let ok = wit::NodeOutput {
            success: true,
            data: "{\"a\":1}".to_string(),
            error_code: String::new(),
            error_message: String::new(),
            recoverable: false,
        };
        assert!(wit_output_to_engine(ok).success);

        let huge = wit::NodeOutput {
            success: true,
            data: format!("\"{}\"", "a".repeat(PLUGIN_OUTPUT_LIMIT_BYTES)),
            error_code: String::new(),
            error_message: String::new(),
            recoverable: false,
        };
        let failed = wit_output_to_engine(huge);
        assert!(!failed.success);
        assert_eq!(failed.error.expect("error missing").code, "wasm_output_too_large");

        let noisy = wit::NodeOutput {
            success: false,
            data: String::new(),
            error_code: "E".to_string(),
            error_message: "m".repeat(PLUGIN_ERROR_TEXT_LIMIT_CHARS * 2),
            recoverable: true,
        };
        let cut = wit_output_to_engine(noisy).error.expect("error missing");
        assert_eq!(cut.message.chars().count(), PLUGIN_ERROR_TEXT_LIMIT_CHARS);
    }
}
