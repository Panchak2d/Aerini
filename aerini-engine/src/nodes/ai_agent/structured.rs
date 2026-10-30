use serde_json::Value;

use crate::nodes::ai_prompt::is_official_openai;

use super::tools::strict_schema_problem;

/// Why `instance` does not fit `schema`, at most five reasons. A missing, empty
/// or uncompilable schema checks nothing: a tool definition the workflow author
/// wrote loosely must not make every call of that tool look invalid.
pub(super) fn schema_errors(schema: &Value, instance: &Value) -> Vec<String> {
    if schema.as_object().is_none_or(|m| m.is_empty()) {
        return Vec::new();
    }
    let Ok(validator) = jsonschema::options()
        .with_draft(jsonschema::Draft::Draft7)
        .build(schema)
    else {
        return Vec::new();
    };
    validator
        .iter_errors(instance)
        .take(5)
        .map(|e| format!("{} (at path: {})", e, e.instance_path()))
        .collect()
}

/// Reads the Response Schema field: JSON text or an object, or nothing. The top
/// level must be an object schema, which every provider's JSON-schema mode
/// accepts (OpenAI requires it), and the schema must compile.
pub(super) fn parse_response_schema(raw: &Value) -> Result<Option<Value>, String> {
    let schema = match raw {
        Value::Null => return Ok(None),
        Value::String(s) if s.trim().is_empty() => return Ok(None),
        Value::String(s) => serde_json::from_str::<Value>(s)
            .map_err(|e| format!("response_schema is not valid JSON: {e}"))?,
        other => other.clone(),
    };
    let Some(obj) = schema.as_object() else {
        return Err("response_schema must be a JSON Schema object".to_string());
    };
    if obj.get("type").and_then(Value::as_str) != Some("object") {
        return Err("the top level of response_schema must have \"type\": \"object\"; put a list inside a property of an object".to_string());
    }
    jsonschema::options()
        .with_draft(jsonschema::Draft::Draft7)
        .build(&schema)
        .map_err(|e| format!("response_schema is not a valid JSON Schema: {e}"))?;
    Ok(Some(schema))
}

/// Anthropic and the hosted OpenAI API refuse a schema that breaks their
/// structured-output rules with an opaque HTTP 400, so the rules that can be
/// checked locally are checked here, naming the place in the schema. Other
/// servers receive the schema as written.
pub(super) fn check_response_schema_for_provider(schema: &Value, provider_id: &str, base_url: &str) -> Result<(), String> {
    let (name, require_all) = match provider_id {
        "anthropic" => ("Anthropic", false),
        "gemini" => return Ok(()),
        _ if is_official_openai(base_url) => ("OpenAI", true),
        _ => return Ok(()),
    };
    match strict_schema_problem(schema, "response_schema", require_all) {
        Some(problem) => Err(format!("response_schema cannot be used with {name}: {problem}")),
        None => Ok(()),
    }
}

const STRUCTURED_CUT_OFF: &str = "the reply was cut off at max_tokens before the JSON was complete. Raise max_tokens.";

pub(super) struct StructuredCheck {
    pub(super) value: Value,
    pub(super) valid: bool,
    pub(super) error: Option<String>,
}

impl StructuredCheck {
    pub(super) fn absent() -> Self {
        Self { value: Value::Null, valid: false, error: None }
    }
}

/// The reply without a Markdown code fence, which servers that do not enforce
/// a schema sometimes wrap JSON in.
fn strip_json_fence(text: &str) -> &str {
    let text = text.trim();
    let Some(rest) = text.strip_prefix("```") else { return text };
    let rest = rest.strip_prefix("json").or_else(|| rest.strip_prefix("JSON")).unwrap_or(rest);
    match rest.trim_end().strip_suffix("```") {
        Some(body) => body.trim(),
        None => text,
    }
}

/// Parses the reply and checks it against `response_schema`. The provider may
/// have enforced the schema already, but a server can accept the setting and
/// ignore it, so the answer is always checked. `value` is set only for a reply
/// that fits.
pub(super) fn check_structured(schema: &Value, content: &str, truncated: bool) -> StructuredCheck {
    let fail = |error: String| StructuredCheck { value: Value::Null, valid: false, error: Some(error) };
    let text = strip_json_fence(content);
    if text.is_empty() {
        return fail(if truncated {
            STRUCTURED_CUT_OFF.to_string()
        } else {
            "the model returned no text, so there is no JSON to check".to_string()
        });
    }
    let value: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => return fail(if truncated {
            STRUCTURED_CUT_OFF.to_string()
        } else {
            format!("the reply is not valid JSON: {e}")
        }),
    };
    let problems = schema_errors(schema, &value);
    if problems.is_empty() {
        StructuredCheck { value, valid: true, error: None }
    } else {
        fail(problems.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    #[test]
    fn hosted_openai_needs_every_property_required_and_other_servers_are_not_checked() {
        let optional = json!({ "type": "object", "additionalProperties": false, "properties": { "n": { "type": "integer" } } });
        let err = check_response_schema_for_provider(&optional, "openai", "https://api.openai.com/v1").expect_err("optional property");
        assert!(err.contains("OpenAI") && err.contains("\"n\" is missing"), "{err}");
        assert!(check_response_schema_for_provider(&optional, "local", "http://localhost:11434/v1").is_ok());
        assert!(check_response_schema_for_provider(&optional, "gemini", "https://generativelanguage.googleapis.com/v1beta").is_ok());
        assert!(check_response_schema_for_provider(&optional, "anthropic", "https://api.anthropic.com").is_ok());
    }
}
