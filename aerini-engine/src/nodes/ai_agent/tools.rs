use serde_json::{json, Value};

use crate::error::NodeError;
use crate::nodes::util::cfg_bool_opt;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ToolChoice {
    #[default]
    Auto,
    Required,
    NoTools,
}

/// The node's tool settings, read once from the config.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct ToolControls {
    pub(super) choice:      ToolChoice,
    pub(super) single_call: bool,
    pub(super) strict:      bool,
}

impl ToolControls {
    pub(super) fn from_config(input: &Value) -> Result<Self, NodeError> {
        let choice = match &input["tool_choice"] {
            Value::Null => ToolChoice::Auto,
            Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
                "" | "auto" => ToolChoice::Auto,
                "required"  => ToolChoice::Required,
                "none"      => ToolChoice::NoTools,
                other => return Err(NodeError::unrecoverable(
                    "INVALID_TOOL_CONTROLS",
                    format!("tool_choice must be auto, required or none, not \"{other}\""),
                )),
            },
            _ => return Err(NodeError::unrecoverable(
                "INVALID_TOOL_CONTROLS",
                "tool_choice must be auto, required or none",
            )),
        };
        Ok(Self {
            choice,
            single_call: cfg_bool_opt(&input["single_tool_call"], "single_tool_call")?.unwrap_or(false),
            strict:      cfg_bool_opt(&input["strict_tools"], "strict_tools")?.unwrap_or(false),
        })
    }

    pub(super) fn requires_tool(self, tools: &[Value]) -> bool {
        self.choice == ToolChoice::Required && !tools.is_empty()
    }
}

/// Anthropic accepts at most this many tools with `strict` set.
const MAX_ANTHROPIC_STRICT_TOOLS: usize = 20;

/// Rejects a setting the provider cannot honor, so it fails with a reason
/// instead of being dropped or surfacing as an opaque HTTP 400. Settings are
/// ignored when no tools are declared, except `required`, which has nothing
/// to require.
pub(super) fn check_tool_controls(controls: ToolControls, provider_id: &str, tools: &[Value]) -> Result<(), String> {
    if tools.is_empty() {
        return if controls.choice == ToolChoice::Required {
            Err("tool_choice \"required\" needs at least one tool in Tools".to_string())
        } else {
            Ok(())
        };
    }
    match provider_id {
        "anthropic" => {
            if controls.choice == ToolChoice::Required {
                return Err("tool_choice \"required\" is not available with Anthropic: current Claude models accept only auto and none".to_string());
            }
            if controls.strict && tools.len() > MAX_ANTHROPIC_STRICT_TOOLS {
                return Err(format!(
                    "strict_tools is limited to {MAX_ANTHROPIC_STRICT_TOOLS} tools with Anthropic; this node declares {}",
                    tools.len()
                ));
            }
        }
        "gemini" => {
            if controls.single_call {
                return Err("single_tool_call is not available with Gemini: the API has no setting to limit a reply to one tool request".to_string());
            }
            if controls.strict {
                return Err("strict_tools is not available with Gemini: the API has no strict tool mode".to_string());
            }
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn with_strict_flag(tool: &Value) -> Value {
    let mut def = tool.clone();
    def["strict"] = json!(true);
    def
}

/// A schema that takes no arguments: no properties and, if a type is given, an object.
pub(super) fn is_blank_schema(schema: &Value) -> bool {
    match schema {
        Value::Null => true,
        Value::Object(m) => {
            m.keys().all(|k| k == "type" || k == "properties")
                && m.get("type").is_none_or(|t| t == "object")
                && m.get("properties").is_none_or(|p| p.as_object().is_some_and(|o| o.is_empty()))
        }
        _ => false,
    }
}

/// Marks every tool strict. A tool that takes no arguments gets the empty
/// object schema strict mode needs; any other schema must already meet the
/// provider's strict rules, or this fails naming the tool and the place in its
/// schema, because changing a schema the author wrote would change what the
/// model may send. `schema_key` is where the tool keeps its schema, and
/// `require_all` adds OpenAI's rule that every property is listed in `required`.
pub(super) fn strict_defs(tools: &[Value], schema_key: &str, require_all: bool) -> Result<Vec<Value>, String> {
    tools.iter().map(|t| {
        let name = t["name"].as_str().unwrap_or("");
        let mut def = with_strict_flag(t);
        if is_blank_schema(&def[schema_key]) {
            def[schema_key] = json!({ "type": "object", "properties": {}, "additionalProperties": false });
        } else if let Some(problem) = strict_schema_problem(&def[schema_key], "schema", require_all) {
            return Err(format!("tool \"{name}\" cannot use strict_tools: {problem}"));
        }
        Ok(def)
    }).collect()
}

/// The first place `node` breaks strict mode: an object without
/// `additionalProperties` set to false and, with `require_all`, a property
/// missing from `required`.
pub(super) fn strict_schema_problem(node: &Value, path: &str, require_all: bool) -> Option<String> {
    let obj = node.as_object()?;
    let is_object = match obj.get("type") {
        Some(Value::String(t)) => t == "object",
        Some(Value::Array(ts)) => ts.iter().any(|t| t == "object"),
        _ => obj.contains_key("properties"),
    };
    if is_object {
        if obj.get("additionalProperties") != Some(&Value::Bool(false)) {
            return Some(format!("{path} must set additionalProperties to false"));
        }
        if require_all {
            let required: Vec<&str> = obj.get("required")
                .and_then(Value::as_array)
                .map(|r| r.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let missing = obj.get("properties")
                .and_then(Value::as_object)
                .and_then(|props| props.keys().find(|k| !required.contains(&k.as_str())));
            if let Some(key) = missing {
                return Some(format!(
                    "{path} must list every property in required, and \"{key}\" is missing; make an optional one nullable with a type such as [\"string\", \"null\"]"
                ));
            }
        }
    }

    let mut children: Vec<(String, &Value)> = Vec::new();
    if let Some(Value::Object(props)) = obj.get("properties") {
        children.extend(props.iter().map(|(k, v)| (format!("{path}/properties/{k}"), v)));
    }
    for key in ["$defs", "definitions"] {
        if let Some(Value::Object(defs)) = obj.get(key) {
            children.extend(defs.iter().map(|(k, v)| (format!("{path}/{key}/{k}"), v)));
        }
    }
    match obj.get("items") {
        Some(v @ Value::Object(_)) => children.push((format!("{path}/items"), v)),
        Some(Value::Array(list)) => children.extend(list.iter().enumerate().map(|(i, v)| (format!("{path}/items/{i}"), v))),
        _ => {}
    }
    for key in ["prefixItems", "anyOf", "oneOf", "allOf"] {
        if let Some(Value::Array(list)) = obj.get(key) {
            children.extend(list.iter().enumerate().map(|(i, v)| (format!("{path}/{key}/{i}"), v)));
        }
    }
    children.into_iter().find_map(|(p, v)| strict_schema_problem(v, &p, require_all))
}

/// The text of a tool call's arguments. Providers send a JSON string, but some
/// local servers send the arguments as an object, and a call with no arguments
/// may omit them.
pub(super) fn arguments_text(raw: &Value) -> String {
    match raw {
        Value::String(s) => s.clone(),
        Value::Null => "{}".to_string(),
        other => other.to_string(),
    }
}

/// Parses the Tools field into `{name, description, parameters}` entries. It
/// takes the documented flat shape, the OpenAI `{type, function}` shape and
/// Anthropic's `input_schema` spelling of `parameters`, as JSON text or an
/// array, and rejects anything else, including a repeated name, instead of
/// running the agent with no tools.
pub(super) fn parse_tools(raw: &Value) -> Result<Vec<Value>, String> {
    let list: Vec<Value> = match raw {
        Value::Null => return Ok(Vec::new()),
        Value::String(s) if s.trim().is_empty() => return Ok(Vec::new()),
        Value::String(s) => match serde_json::from_str::<Value>(s) {
            Ok(Value::Array(a)) => a,
            Ok(_) => return Err("tools must be a JSON array of tool definitions".to_string()),
            Err(e) => return Err(format!("tools is not valid JSON: {e}")),
        },
        Value::Array(a) => a.clone(),
        _ => return Err("tools must be a JSON array of tool definitions".to_string()),
    };
    let mut out: Vec<Value> = Vec::with_capacity(list.len());
    for (i, tool) in list.iter().enumerate() {
        let def = if tool["function"].is_object() { &tool["function"] } else { tool };
        let name = match def["name"].as_str().map(str::trim) {
            Some(n) if !n.is_empty() => n,
            _ => return Err(format!("tools[{i}] needs a non-empty \"name\"")),
        };
        if out.iter().any(|d| d["name"].as_str().map(str::trim) == Some(name)) {
            return Err(format!("tools[{i}]: the name \"{name}\" is used by more than one tool"));
        }
        let mut def = def.clone();
        if def["parameters"].is_null() {
            if let Some(schema) = def.as_object_mut().and_then(|m| m.remove("input_schema")) {
                def["parameters"] = schema;
            }
        }
        out.push(def);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::nodes::ai_agent::output::{RequestedTool, check_tool_call};

    #[test]
    fn tools_field_accepts_flat_and_function_shapes_and_rejects_the_rest() {
        let flat = json!(r#"[{"name":"a","description":"d","parameters":{}}]"#);
        let wrapped = json!([{ "type": "function", "function": { "name": "b" } }]);
        assert_eq!(parse_tools(&flat).unwrap()[0]["name"], "a");
        assert_eq!(parse_tools(&wrapped).unwrap()[0]["name"], "b");
        assert!(parse_tools(&Value::Null).unwrap().is_empty());
        assert!(parse_tools(&json!("  ")).unwrap().is_empty());
        assert!(parse_tools(&json!("not json")).is_err());
        assert!(parse_tools(&json!(r#"{"name":"a"}"#)).is_err());
        assert!(parse_tools(&json!([{ "description": "no name" }])).is_err());
    }

    #[test]
    fn object_arguments_are_kept_and_absent_or_blank_arguments_mean_no_arguments() {
        assert_eq!(arguments_text(&json!({ "q": "y" })), r#"{"q":"y"}"#);
        assert_eq!(arguments_text(&Value::Null), "{}");
        let call = RequestedTool { id: String::new(), name: "ping".into(), arguments: "  ".into() };
        let check = check_tool_call(&call, &[json!({ "name": "ping" })]);
        assert!(check.error.is_none());
        assert_eq!(check.parsed, json!({}));
    }

    #[test]
    fn tools_accept_input_schema_spelling_and_reject_repeated_names() {
        let parsed = parse_tools(&json!([{ "name": "a", "input_schema": { "type": "object" } }])).expect("valid");
        assert_eq!(parsed[0]["parameters"], json!({ "type": "object" }));
        assert!(parsed[0].get("input_schema").is_none());
        let err = parse_tools(&json!([{ "name": "a" }, { "type": "function", "function": { "name": "a" } }]))
            .expect_err("repeated name");
        assert!(err.contains("\"a\""), "{err}");
    }

    #[test]
    fn strict_tools_fill_blank_schemas_and_reject_schemas_that_do_not_fit() {
        let blank = strict_defs(&[json!({ "name": "ping" })], "parameters", true).expect("blank schema");
        assert_eq!(blank[0]["strict"], true);
        assert_eq!(blank[0]["parameters"]["additionalProperties"], false);

        let open = json!({ "name": "lookup", "parameters": { "type": "object", "properties": { "q": { "type": "string" } } } });
        let err = strict_defs(&[open], "parameters", false).expect_err("open object");
        assert!(err.contains("lookup") && err.contains("additionalProperties"), "{err}");

        let nested = json!({ "name": "t", "parameters": { "type": "object", "additionalProperties": false, "required": ["f"],
            "properties": { "f": { "type": "object", "properties": {} } } } });
        let err = strict_defs(&[nested], "parameters", true).expect_err("nested object");
        assert!(err.contains("schema/properties/f"), "{err}");

        let optional = json!({ "name": "t", "parameters": { "type": "object", "additionalProperties": false,
            "properties": { "q": { "type": "string" } } } });
        assert!(strict_defs(std::slice::from_ref(&optional), "parameters", false).is_ok());
        let err = strict_defs(&[optional], "parameters", true).expect_err("optional property");
        assert!(err.contains("\"q\" is missing"), "{err}");

        let many: Vec<Value> = (0..21).map(|i| json!({ "name": format!("t{i}") })).collect();
        let controls = ToolControls { strict: true, ..ToolControls::default() };
        let err = check_tool_controls(controls, "anthropic", &many).expect_err("over the limit");
        assert!(err.contains("20"), "{err}");
    }
}
