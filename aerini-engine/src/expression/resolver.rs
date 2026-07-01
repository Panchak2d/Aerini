//! Public entry points and recursive value resolver.

use serde_json::Value;
use std::collections::{HashMap, HashSet};

use crate::model::{ExecutionContext, Workflow};
use super::functions::apply_function;
use super::parser::{find_matching_close, split_args};

/// Resolve all `{{...}}` expressions inside a single string template.
///
/// Returns `(resolved_string, warning_messages)`.
/// If the template contains no `{{`, it is returned unchanged immediately (fast path).
pub fn resolve_string(
    template:      &str,
    workflow:      &Workflow,
    ctx:           &ExecutionContext,
    env_allowlist: Option<&HashSet<String>>,
) -> (String, Vec<String>) {
    if !template.contains("{{") {
        return (template.to_string(), vec![]);
    }

    let name_to_id = build_name_map(workflow);
    let mut warnings = Vec::new();
    let mut result = String::with_capacity(template.len());
    let mut chars = template.char_indices().peekable();

    while let Some((i, ch)) = chars.next() {
        if ch == '{' {
            if let Some(&(_, '{')) = chars.peek() {
                chars.next();
                let mut expr = String::new();
                let mut closed = false;
                while let Some((_, ec)) = chars.next() {
                    if ec == '}' {
                        if let Some(&(_, '}')) = chars.peek() {
                            chars.next();
                            closed = true;
                            break;
                        }
                    }
                    expr.push(ec);
                }
                if !closed {
                    result.push_str(&template[i..]);
                    return (result, warnings);
                }
                let expr = expr.trim();
                let (val, mut w) = resolve_expression(expr, workflow, &name_to_id, ctx, env_allowlist);
                warnings.append(&mut w);
                result.push_str(&val);
                continue;
            }
        }
        result.push(ch);
    }

    (result, warnings)
}

fn has_expressions(value: &Value) -> bool {
    match value {
        Value::String(s)   => s.contains("{{"),
        Value::Array(arr)  => arr.iter().any(has_expressions),
        Value::Object(map) => map.values().any(has_expressions),
        _ => false,
    }
}

/// Recursively walk a JSON `Value`, applying `resolve_string` to every string leaf.
pub fn resolve_all_strings(
    value:         &Value,
    workflow:      &Workflow,
    ctx:           &ExecutionContext,
    env_allowlist: Option<&HashSet<String>>,
) -> (Value, Vec<String>) {
    if !has_expressions(value) {
        return (value.clone(), vec![]);
    }

    let mut warnings = Vec::new();
    let resolved = resolve_value_recursive(value, workflow, ctx, &mut warnings, env_allowlist);
    (resolved, warnings)
}

pub(super) fn resolve_value_recursive(
    value:         &Value,
    workflow:      &Workflow,
    ctx:           &ExecutionContext,
    warnings:      &mut Vec<String>,
    env_allowlist: Option<&HashSet<String>>,
) -> Value {
    match value {
        Value::String(s) => {
            let (resolved, mut w) = resolve_string(s, workflow, ctx, env_allowlist);
            warnings.append(&mut w);
            Value::String(resolved)
        }
        Value::Array(arr) => Value::Array(
            arr.iter().map(|v| resolve_value_recursive(v, workflow, ctx, warnings, env_allowlist)).collect(),
        ),
        Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                out.insert(k.clone(), resolve_value_recursive(v, workflow, ctx, warnings, env_allowlist));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

pub(super) fn build_name_map(workflow: &Workflow) -> HashMap<String, String> {
    let mut map = HashMap::with_capacity(workflow.nodes.len());
    for node in &workflow.nodes {
        map.entry(node.name.clone()).or_insert_with(|| node.id.clone());
    }
    map
}

pub(super) fn resolve_expression(
    expr:          &str,
    workflow:      &Workflow,
    name_to_id:    &HashMap<String, String>,
    ctx:           &ExecutionContext,
    env_allowlist: Option<&HashSet<String>>,
) -> (String, Vec<String>) {
    let mut warnings = Vec::new();

    if expr.is_empty() || expr == "." {
        warnings.push(format!(
            "Expression '{{{{{}}}}}' is not valid — empty or bare dot. Replaced with empty string.",
            expr
        ));
        return (String::new(), warnings);
    }

    if let Some(result) = try_resolve_function(expr, workflow, name_to_id, ctx, env_allowlist) {
        return result;
    }

    let segments: Vec<&str> = expr.split('.').collect();

    if segments[0].starts_with('$') {
        return resolve_special(expr, &segments, workflow, ctx, &mut warnings, env_allowlist);
    }

    if segments.len() < 3 {
        warnings.push(format!(
            "Expression '{{{{{}}}}}': path too short — expected '{{{{node_name.output.field}}}}'. Replaced with empty string.",
            expr
        ));
        return (String::new(), warnings);
    }

    let node_name = segments[0];

    let ambiguous = workflow.nodes.iter().filter(|n| n.name == node_name).count() > 1;
    if ambiguous {
        warnings.push(format!(
            "Expression '{{{{{}}}}}': multiple nodes named '{}' — using the first one.",
            expr, node_name
        ));
    }

    let node_id = match name_to_id.get(node_name) {
        Some(id) => id,
        None => {
            warnings.push(format!(
                "Expression '{{{{{}}}}}': no node named '{}' in this workflow. Replaced with empty string.",
                expr, node_name
            ));
            return (String::new(), warnings);
        }
    };

    if segments[1] != "output" {
        warnings.push(format!(
            "Expression '{{{{{}}}}}': expected 'output' as second segment, got '{}'. Continuing anyway.",
            expr, segments[1]
        ));
    }

    let node_output = match ctx.node_outputs.get(node_id) {
        Some(v) => v,
        None => {
            warnings.push(format!(
                "Expression '{{{{{}}}}}': node '{}' has not produced output yet. Replaced with empty string.",
                expr, node_name
            ));
            return (String::new(), warnings);
        }
    };

    let field_path = &segments[2..];
    if field_path.is_empty() {
        return (value_to_string(node_output), warnings);
    }

    match traverse_path(node_output, field_path) {
        Some(v) => (value_to_string(&v), warnings),
        None => {
            warnings.push(format!(
                "Expression '{{{{{}}}}}': field path '{}' not found in node '{}' output. Replaced with empty string.",
                expr, field_path.join("."), node_name
            ));
            (String::new(), warnings)
        }
    }
}

fn resolve_special(
    expr:          &str,
    segments:      &[&str],
    workflow:      &Workflow,
    ctx:           &ExecutionContext,
    warnings:      &mut Vec<String>,
    env_allowlist: Option<&HashSet<String>>,
) -> (String, Vec<String>) {
    match segments[0] {
        "$run" => {
            let key = segments.get(1).copied().unwrap_or("");
            let val = match key {
                "id" => ctx.metadata.get("execution_id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                "timestamp" => chrono::Utc::now().to_rfc3339(),
                "workflow_name" => workflow.name.clone(),
                other => {
                    warnings.push(format!(
                        "Expression '{{{{{}}}}}': unknown $run field '{}'. Valid: id, timestamp, workflow_name.",
                        expr, other
                    ));
                    String::new()
                }
            };
            (val, warnings.to_owned())
        }
        "$env" => {
            let var_name = segments.get(1).copied().unwrap_or("");
            if var_name.is_empty() {
                warnings.push(format!("Expression '{{{{{}}}}}': $env requires a variable name.", expr));
                return (String::new(), warnings.to_owned());
            }
            match env_allowlist {
                None => {
                    warnings.push(format!(
                        "Expression '{{{{{}}}}}': $env expressions are disabled. \
                         Pass --allow-env-vars {} to enable this variable.",
                        expr, var_name
                    ));
                    (String::new(), warnings.to_owned())
                }
                Some(allowed) => {
                    if !allowed.contains(var_name) {
                        warnings.push(format!(
                            "Expression '{{{{{}}}}}': $env.{} is not in the allowed list. \
                             Add it to --allow-env-vars to enable.",
                            expr, var_name
                        ));
                        return (String::new(), warnings.to_owned());
                    }
                    (std::env::var(var_name).unwrap_or_default(), warnings.to_owned())
                }
            }
        }
        "$vars" => {
            let var_name = segments.get(1).copied().unwrap_or("");
            if var_name.is_empty() {
                warnings.push(format!("Expression '{{{{{}}}}}': $vars requires a variable name.", expr));
                return (String::new(), warnings.to_owned());
            }
            match ctx.variables.get(var_name) {
                None => {
                    warnings.push(format!(
                        "Expression '{{{{{}}}}}': $vars.{} not found in workflow variables. Replaced with empty string.",
                        expr, var_name
                    ));
                    (String::new(), warnings.to_owned())
                }
                Some(val) => {
                    let field_path = &segments[2..];
                    if field_path.is_empty() {
                        (value_to_string(val), warnings.to_owned())
                    } else {
                        match traverse_path(val, field_path) {
                            Some(v) => (value_to_string(&v), warnings.to_owned()),
                            None => {
                                warnings.push(format!(
                                    "Expression '{{{{{}}}}}': field path '{}' not found in $vars.{}. Replaced with empty string.",
                                    expr, field_path.join("."), var_name
                                ));
                                (String::new(), warnings.to_owned())
                            }
                        }
                    }
                }
            }
        }
        other => {
            warnings.push(format!(
                "Expression '{{{{{}}}}}': unknown special variable '{}'. Known: $run, $env, $vars.",
                expr, other
            ));
            (String::new(), warnings.to_owned())
        }
    }
}

pub(super) fn traverse_path(root: &Value, segments: &[&str]) -> Option<Value> {
    let mut current = root.clone();
    for seg in segments {
        if let Some(bracket) = seg.find('[') {
            let field = &seg[..bracket];
            let rest  = &seg[bracket..];
            if !field.is_empty() {
                current = current.get(field)?.clone();
            }
            let mut idx_slice = rest;
            while idx_slice.starts_with('[') {
                let close = idx_slice.find(']')?;
                let idx: usize = idx_slice[1..close].parse().ok()?;
                current = current.get(idx)?.clone();
                idx_slice = &idx_slice[close + 1..];
            }
        } else {
            current = current.get(seg)?.clone();
        }
    }
    Some(current)
}

pub(super) fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null      => String::new(),
        Value::Bool(b)   => b.to_string(),
        Value::Number(n) => n.to_string(),
        other            => other.to_string(),
    }
}

fn try_resolve_function(
    expr:          &str,
    workflow:      &Workflow,
    name_to_id:    &HashMap<String, String>,
    ctx:           &ExecutionContext,
    env_allowlist: Option<&HashSet<String>>,
) -> Option<(String, Vec<String>)> {
    let paren_pos = expr.find('(')?;
    let func_name = &expr[..paren_pos];

    let mut chars = func_name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return None,
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }

    let after_paren = &expr[paren_pos + 1..];
    let args_str = find_matching_close(after_paren)?;
    let raw_args = split_args(args_str);

    let mut warnings = Vec::new();
    let mut resolved_args: Vec<String> = Vec::with_capacity(raw_args.len());
    for arg in &raw_args {
        let arg = arg.trim();
        if arg.is_empty() {
            resolved_args.push(String::new());
            continue;
        }
        if arg.starts_with('"') && arg.ends_with('"') && arg.len() >= 2 {
            resolved_args.push(arg[1..arg.len() - 1].to_string());
            continue;
        }
        if arg.starts_with('\'') && arg.ends_with('\'') && arg.len() >= 2 {
            resolved_args.push(arg[1..arg.len() - 1].to_string());
            continue;
        }
        if arg.parse::<f64>().is_ok() {
            resolved_args.push(arg.to_string());
            continue;
        }
        let (val, mut w) = resolve_expression(arg, workflow, name_to_id, ctx, env_allowlist);
        warnings.append(&mut w);
        resolved_args.push(val);
    }

    let result = apply_function(func_name, &resolved_args, &mut warnings);
    Some((result, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration::CURRENT_VERSION;
    use crate::model::{ExecutionContext, WorkflowNode};
    use serde_json::json;

    fn make_workflow(nodes: Vec<(&str, &str)>) -> Workflow {
        Workflow {
            schema_version: CURRENT_VERSION.to_string(),
            id: "test_wf".to_string(),
            name: "Test Workflow".to_string(),
            description: String::new(),
            nodes: nodes.into_iter().map(|(id, name)| WorkflowNode {
                id: id.to_string(),
                node_type_id: "http_request".to_string(),
                node_type: crate::model::NodeType::Action,
                name: name.to_string(),
                config: json!({}),
                credentials: Default::default(),
                input_schema: json!({}),
                output_schema: json!({}),
                retry: Default::default(),
                fallback_node: None,
                disabled: false,
                position: Default::default(),
            }).collect(),
            edges: vec![],
            metadata: Default::default(),
            max_duration_secs: None,
            parallel_execution: false,
            max_concurrent_nodes: None,
            settings: Default::default(),
        }
    }

    fn make_ctx(outputs: Vec<(&str, Value)>, vars: Vec<(&str, Value)>) -> ExecutionContext {
        let mut node_outputs = std::collections::HashMap::new();
        for (k, v) in outputs { node_outputs.insert(k.to_string(), v); }
        let mut variables = std::collections::HashMap::new();
        for (k, v) in vars { variables.insert(k.to_string(), v); }
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("execution_id".to_string(), json!("exec_abc123"));
        ExecutionContext { variables, node_outputs: std::sync::Arc::new(node_outputs), metadata }
    }

    #[test]
    fn no_expression_returns_unchanged() {
        let wf = make_workflow(vec![]);
        let ctx = make_ctx(vec![], vec![]);
        let (out, warns) = resolve_string("hello world", &wf, &ctx, None);
        assert_eq!(out, "hello world");
        assert!(warns.is_empty());
    }

    #[test]
    fn resolves_node_output_field() {
        let wf = make_workflow(vec![("n1", "Fetch Data")]);
        let ctx = make_ctx(vec![("n1", json!({"body": "ok"}))], vec![]);
        let (out, warns) = resolve_string("result: {{Fetch Data.output.body}}", &wf, &ctx, None);
        assert_eq!(out, "result: ok");
        assert!(warns.is_empty());
    }

    #[test]
    fn resolves_array_index() {
        let wf = make_workflow(vec![("n1", "List Items")]);
        let ctx = make_ctx(vec![("n1", json!({"items": ["a", "b", "c"]}))], vec![]);
        let (out, _) = resolve_string("{{List Items.output.items[1]}}", &wf, &ctx, None);
        assert_eq!(out, "b");
    }

    #[test]
    fn missing_node_produces_warning() {
        let wf = make_workflow(vec![]);
        let ctx = make_ctx(vec![], vec![]);
        let (out, warns) = resolve_string("{{Ghost Node.output.field}}", &wf, &ctx, None);
        assert_eq!(out, "");
        assert!(!warns.is_empty());
    }

    #[test]
    fn run_id_resolves() {
        let wf = make_workflow(vec![]);
        let ctx = make_ctx(vec![], vec![]);
        let (out, warns) = resolve_string("id={{$run.id}}", &wf, &ctx, None);
        assert_eq!(out, "id=exec_abc123");
        assert!(warns.is_empty());
    }

    #[test]
    fn env_disabled_produces_warning() {
        let wf = make_workflow(vec![]);
        let ctx = make_ctx(vec![], vec![]);
        let (out, warns) = resolve_string("{{$env.SECRET}}", &wf, &ctx, None);
        assert_eq!(out, "");
        assert!(!warns.is_empty());
    }

    #[test]
    #[serial_test::serial]
    fn env_allowed_resolves_from_env() {
        let wf = make_workflow(vec![]);
        let ctx = make_ctx(vec![], vec![]);
        std::env::set_var("AERINI_TEST_VAR", "hello_from_env");
        let allowed: HashSet<String> = ["AERINI_TEST_VAR".to_string()].into();
        let (out, warns) = resolve_string("{{$env.AERINI_TEST_VAR}}", &wf, &ctx, Some(&allowed));
        assert_eq!(out, "hello_from_env");
        assert!(warns.is_empty());
        std::env::remove_var("AERINI_TEST_VAR");
    }

    #[test]
    fn env_not_in_allowlist_produces_warning() {
        let wf = make_workflow(vec![]);
        let ctx = make_ctx(vec![], vec![]);
        let allowed: HashSet<String> = HashSet::new();
        let (out, warns) = resolve_string("{{$env.SECRET}}", &wf, &ctx, Some(&allowed));
        assert_eq!(out, "");
        assert!(!warns.is_empty());
    }

    #[test]
    fn vars_scalar_resolves() {
        let wf = make_workflow(vec![]);
        let ctx = make_ctx(vec![], vec![("session_id", json!("abc-123"))]);
        let (out, warns) = resolve_string("{{$vars.session_id}}", &wf, &ctx, None);
        assert_eq!(out, "abc-123");
        assert!(warns.is_empty());
    }

    #[test]
    fn vars_nested_field_path_resolves() {
        let wf = make_workflow(vec![]);
        let ctx = make_ctx(vec![], vec![(
            "__aerini_webhook_payload",
            json!({"node_id": "n1", "payload": {"body": {"email": "a@b.com"}}}),
        )]);
        let (out, warns) = resolve_string(
            "{{$vars.__aerini_webhook_payload.payload.body.email}}", &wf, &ctx, None,
        );
        assert_eq!(out, "a@b.com");
        assert!(warns.is_empty());
    }

    #[test]
    fn vars_missing_key_produces_warning() {
        let wf = make_workflow(vec![]);
        let ctx = make_ctx(vec![], vec![]);
        let (out, warns) = resolve_string("{{$vars.nope}}", &wf, &ctx, None);
        assert_eq!(out, "");
        assert!(!warns.is_empty());
    }

    #[test]
    fn resolve_all_strings_skips_non_template_values() {
        let wf = make_workflow(vec![]);
        let ctx = make_ctx(vec![], vec![]);
        let input = json!({"num": 42, "flag": true, "s": "plain"});
        let (out, warns) = resolve_all_strings(&input, &wf, &ctx, None);
        assert_eq!(out, input);
        assert!(warns.is_empty());
    }

    #[test]
    fn resolve_all_strings_resolves_nested_template() {
        let wf = make_workflow(vec![("n1", "Step 1")]);
        let ctx = make_ctx(vec![("n1", json!({"val": "world"}))], vec![]);
        let input = json!({"msg": "hello {{Step 1.output.val}}"});
        let (out, _) = resolve_all_strings(&input, &wf, &ctx, None);
        assert_eq!(out["msg"], "hello world");
    }

    #[test]
    fn unclosed_expression_returns_remainder_unchanged() {
        let wf = make_workflow(vec![]);
        let ctx = make_ctx(vec![], vec![]);
        let template = "start {{unclosed";
        let (out, _) = resolve_string(template, &wf, &ctx, None);
        assert!(out.contains("start "));
    }

    #[test]
    fn upper_function_works() {
        let wf = make_workflow(vec![("n1", "Step 1")]);
        let ctx = make_ctx(vec![("n1", json!({"name": "alice"}))], vec![]);
        let (out, _) = resolve_string("{{upper(Step 1.output.name)}}", &wf, &ctx, None);
        assert_eq!(out, "ALICE");
    }

    #[test]
    fn multiple_expressions_in_one_string() {
        let wf = make_workflow(vec![("n1", "N1"), ("n2", "N2")]);
        let ctx = make_ctx(
            vec![("n1", json!({"a": "hello"})), ("n2", json!({"b": "world"}))],
            vec![],
        );
        let (out, _) = resolve_string("{{N1.output.a}} {{N2.output.b}}", &wf, &ctx, None);
        assert_eq!(out, "hello world");
    }

    #[test]
    fn simple_field_access() {
        let wf = make_workflow(vec![("n1", "HTTP Request")]);
        let ctx = make_ctx(vec![("n1", json!({"body": {"name": "Alice"}}))], vec![]);
        let (out, warns) = resolve_string("Hello {{HTTP Request.output.body.name}}", &wf, &ctx, None);
        assert_eq!(out, "Hello Alice");
        assert!(warns.is_empty());
    }

    #[test]
    fn plain_path_still_resolves() {
        let wf = make_workflow(vec![("n1", "Step 1")]);
        let ctx = make_ctx(vec![("n1", json!({"val": "hello"}))], vec![]);
        let (out, warns) = resolve_string("{{Step 1.output.val}}", &wf, &ctx, None);
        assert_eq!(out, "hello");
        assert!(warns.is_empty());
    }

    #[test]
    fn fn_lower_literal() {
        let wf = make_workflow(vec![]);
        let ctx = make_ctx(vec![], vec![]);
        let (out, _) = resolve_string(r#"{{lower("HELLO")}}"#, &wf, &ctx, None);
        assert_eq!(out, "hello");
    }

    #[test]
    fn fn_trim() {
        let wf = make_workflow(vec![]);
        let ctx = make_ctx(vec![], vec![]);
        let (out, _) = resolve_string(r#"{{trim("  hi  ")}}"#, &wf, &ctx, None);
        assert_eq!(out, "hi");
    }

    #[test]
    fn fn_slice() {
        let wf = make_workflow(vec![]);
        let ctx = make_ctx(vec![], vec![]);
        let (out, _) = resolve_string(r#"{{slice("hello world", 0, 5)}}"#, &wf, &ctx, None);
        assert_eq!(out, "hello");
    }

    #[test]
    fn fn_replace() {
        let wf = make_workflow(vec![]);
        let ctx = make_ctx(vec![], vec![]);
        let (out, _) = resolve_string(r#"{{replace("hello world", "world", "aerini")}}"#, &wf, &ctx, None);
        assert_eq!(out, "hello aerini");
    }

    #[test]
    fn fn_replace_all() {
        let wf = make_workflow(vec![]);
        let ctx = make_ctx(vec![], vec![]);
        let (out, _) = resolve_string(r#"{{replace_all("aaa", "a", "b")}}"#, &wf, &ctx, None);
        assert_eq!(out, "bbb");
    }

    #[test]
    fn fn_len_string() {
        let wf = make_workflow(vec![]);
        let ctx = make_ctx(vec![], vec![]);
        let (out, _) = resolve_string(r#"{{len("hello")}}"#, &wf, &ctx, None);
        assert_eq!(out, "5");
    }

    #[test]
    fn fn_contains_true() {
        let wf = make_workflow(vec![]);
        let ctx = make_ctx(vec![], vec![]);
        let (out, _) = resolve_string(r#"{{contains("hello world", "world")}}"#, &wf, &ctx, None);
        assert_eq!(out, "true");
    }

    #[test]
    fn fn_round() {
        let wf = make_workflow(vec![]);
        let ctx = make_ctx(vec![], vec![]);
        let (out, _) = resolve_string("{{round(3.14159, 2)}}", &wf, &ctx, None);
        assert_eq!(out, "3.14");
    }

    #[test]
    fn fn_abs() {
        let wf = make_workflow(vec![]);
        let ctx = make_ctx(vec![], vec![]);
        let (out, _) = resolve_string("{{abs(-42)}}", &wf, &ctx, None);
        assert_eq!(out, "42");
    }

    #[test]
    fn fn_min_max() {
        let wf = make_workflow(vec![]);
        let ctx = make_ctx(vec![], vec![]);
        let (mn, _) = resolve_string("{{min(3, 7)}}", &wf, &ctx, None);
        let (mx, _) = resolve_string("{{max(3, 7)}}", &wf, &ctx, None);
        assert_eq!(mn, "3");
        assert_eq!(mx, "7");
    }

    #[test]
    fn fn_if_true() {
        let wf = make_workflow(vec![]);
        let ctx = make_ctx(vec![], vec![]);
        let (out, _) = resolve_string(r#"{{if("yes", "A", "B")}}"#, &wf, &ctx, None);
        assert_eq!(out, "A");
    }

    #[test]
    fn fn_if_false_empty() {
        let wf = make_workflow(vec![]);
        let ctx = make_ctx(vec![], vec![]);
        let (out, _) = resolve_string(r#"{{if("", "A", "B")}}"#, &wf, &ctx, None);
        assert_eq!(out, "B");
    }

    #[test]
    fn fn_now_nonempty() {
        let wf = make_workflow(vec![]);
        let ctx = make_ctx(vec![], vec![]);
        let (out, warns) = resolve_string("{{now()}}", &wf, &ctx, None);
        assert!(!out.is_empty());
        assert!(warns.is_empty());
    }

    #[test]
    fn fn_format_date() {
        let wf = make_workflow(vec![]);
        let ctx = make_ctx(vec![], vec![]);
        let (out, warns) = resolve_string(r#"{{format_date("2024-03-15T10:30:00Z", "YYYY-MM-DD")}}"#, &wf, &ctx, None);
        assert_eq!(out, "2024-03-15");
        assert!(warns.is_empty());
    }

    #[test]
    fn fn_unknown_warns() {
        let wf = make_workflow(vec![]);
        let ctx = make_ctx(vec![], vec![]);
        let (out, warns) = resolve_string(r#"{{nonexistent_func("x")}}"#, &wf, &ctx, None);
        assert_eq!(out, "");
        assert!(!warns.is_empty());
    }

    #[test]
    fn resolve_all_strings_walks_object() {
        let wf = make_workflow(vec![("n1", "Step")]);
        let ctx = make_ctx(vec![("n1", json!({"result": "hello"}))], vec![]);
        let config = json!({ "url": "https://example.com/{{Step.output.result}}", "count": 3 });
        let (resolved, warns) = resolve_all_strings(&config, &wf, &ctx, None);
        assert_eq!(resolved["url"], "https://example.com/hello");
        assert_eq!(resolved["count"], 3);
        assert!(warns.is_empty());
    }

    #[test]
    fn null_field_differs_from_missing_field() {
        let wf = make_workflow(vec![("n1", "Step")]);
        let ctx = make_ctx(vec![("n1", json!({"field": null}))], vec![]);

        // null field: resolves to empty string, no warning
        let (out_null, warns_null) = resolve_string("{{Step.output.field}}", &wf, &ctx, None);
        assert_eq!(out_null, "");
        assert!(warns_null.is_empty(), "null field should not warn");

        // missing field: resolves to empty string, WITH warning
        let (out_miss, warns_miss) = resolve_string("{{Step.output.nonexistent}}", &wf, &ctx, None);
        assert_eq!(out_miss, "");
        assert!(!warns_miss.is_empty(), "missing field must warn");
    }

    // ── P23: expression/resolver.rs tests ─────────────────────────────────────

    // Undefined node reference: {{nonexistent.output.value}} → empty string + warning,
    // no panic. Verifies the "no node named" branch in resolve_expression.
    #[test]
    fn undefined_node_reference_returns_empty_with_warning_no_panic() {
        let wf = make_workflow(vec![]);
        let ctx = make_ctx(vec![], vec![]);
        let (out, warns) = resolve_string("{{nonexistent.output.value}}", &wf, &ctx, None);
        assert_eq!(out, "", "undefined node must resolve to empty string");
        assert!(!warns.is_empty(), "must emit a warning for undefined node");
        assert!(
            warns.iter().any(|w| w.contains("nonexistent")),
            "warning must name the missing node; got: {:?}", warns
        );
    }

    // Deep null path: {{Step.output.a.b.c}} where intermediate key b is absent.
    // traverse_path returns None → empty string + warning, no panic.
    #[test]
    fn deep_null_path_intermediate_absent_returns_empty_no_panic() {
        let wf = make_workflow(vec![("n1", "Step")]);
        // "a" exists but has no "b" key → path a.b.c is not traversable.
        let ctx = make_ctx(vec![("n1", json!({ "a": {} }))], vec![]);
        let (out, warns) = resolve_string("{{Step.output.a.b.c}}", &wf, &ctx, None);
        assert_eq!(out, "", "absent intermediate key must resolve to empty string");
        assert!(!warns.is_empty(), "must warn about missing path");
    }

    // Expression inside array element: mixed static + {{...}} values in an array
    // all resolve correctly via resolve_all_strings.
    #[test]
    fn expression_inside_array_element_resolves_correctly() {
        let wf = make_workflow(vec![("n1", "Step")]);
        let ctx = make_ctx(vec![("n1", json!({ "val": "hello" }))], vec![]);
        let input = json!(["static", "{{Step.output.val}}", 42, true]);
        let (out, warns) = resolve_all_strings(&input, &wf, &ctx, None);
        assert_eq!(out[0], "static", "static string must be preserved");
        assert_eq!(out[1], "hello",  "expression in array element must resolve");
        assert_eq!(out[2], 42,       "non-string number must be preserved");
        assert_eq!(out[3], true,     "non-string bool must be preserved");
        assert!(warns.is_empty());
    }

    // resolve_all_strings on a deeply nested object: all leaf strings resolved,
    // non-string values preserved, structure unchanged.
    #[test]
    fn resolve_all_strings_deeply_nested_structure_preserved() {
        let wf = make_workflow(vec![("n1", "Step")]);
        let ctx = make_ctx(vec![("n1", json!({ "result": "world" }))], vec![]);
        let input = json!({
            "a": {
                "b": {
                    "c": "hello {{Step.output.result}}",
                    "d": 99
                }
            },
            "e": [1, "{{Step.output.result}}", true]
        });
        let (out, warns) = resolve_all_strings(&input, &wf, &ctx, None);
        assert_eq!(out["a"]["b"]["c"], "hello world", "nested string must resolve");
        assert_eq!(out["a"]["b"]["d"], 99,            "non-string number must survive");
        assert_eq!(out["e"][0], 1,                    "array non-string must survive");
        assert_eq!(out["e"][1], "world",              "array expression must resolve");
        assert_eq!(out["e"][2], true,                 "array bool must survive");
        assert!(warns.is_empty());
    }
}
