//! Expression resolver for `{{...}}` template syntax.
//!
//! Supported syntax:
//!   {{node_name.output.field}}          — field from a predecessor node's output
//!   {{node_name.output.field.nested}}   — dot-path traversal
//!   {{node_name.output.array_field[0]}} — array index
//!   {{$run.id}}                         — current execution ID
//!   {{$run.timestamp}}                  — ISO timestamp (current time at resolution)
//!   {{$run.workflow_name}}              — workflow name
//!   {{$env.VAR_NAME}}                   — environment variable (empty if unset)
//!   {{upper(node_name.output.field)}}   — inline function call
//!   {{format_date($run.timestamp, "YYYY-MM-DD")}} — function with literal args
//!
//! Fallback: any expression that cannot be resolved produces an empty string
//! and an INFO-level message in the returned warnings list. No panics, no errors.

use serde_json::Value;
use std::collections::{HashMap, HashSet};

use crate::model::{ExecutionContext, Workflow};

/// Resolve all `{{...}}` expressions inside a single string template.
///
/// `env_allowlist` controls `{{$env.VAR}}` resolution:
/// - `None`      → `$env` expressions are disabled; they resolve to `""` with a warning.
/// - `Some(set)` → only variables whose names are in `set` resolve; others produce `""` with a warning.
///
/// Returns `(resolved_string, warning_messages)`.
/// If the template contains no `{{`, it is returned unchanged immediately (fast path).
pub fn resolve_string(
    template: &str,
    workflow: &Workflow,
    ctx: &ExecutionContext,
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
                chars.next(); // consume second '{'
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

/// Recursively walk a JSON `Value`, applying `resolve_string` to every string leaf.
pub fn resolve_all_strings(
    value: &Value,
    workflow: &Workflow,
    ctx: &ExecutionContext,
    env_allowlist: Option<&HashSet<String>>,
) -> (Value, Vec<String>) {
    let raw = value.to_string();
    if !raw.contains("{{") {
        return (value.clone(), vec![]);
    }

    let mut warnings = Vec::new();
    let resolved = resolve_value_recursive(value, workflow, ctx, &mut warnings, env_allowlist);
    (resolved, warnings)
}

fn resolve_value_recursive(
    value: &Value,
    workflow: &Workflow,
    ctx: &ExecutionContext,
    warnings: &mut Vec<String>,
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

fn build_name_map(workflow: &Workflow) -> HashMap<String, String> {
    let mut map = HashMap::with_capacity(workflow.nodes.len());
    for node in &workflow.nodes {
        map.entry(node.name.clone()).or_insert_with(|| node.id.clone());
    }
    map
}

fn resolve_expression(
    expr: &str,
    workflow: &Workflow,
    name_to_id: &HashMap<String, String>,
    ctx: &ExecutionContext,
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

    let segments: Vec<&str> = expr.splitn(32, '.').collect();

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
    expr: &str,
    segments: &[&str],
    workflow: &Workflow,
    ctx: &ExecutionContext,
    warnings: &mut Vec<String>,
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
        other => {
            warnings.push(format!(
                "Expression '{{{{{}}}}}': unknown special variable '{}'. Known: $run, $env.",
                expr, other
            ));
            (String::new(), warnings.to_owned())
        }
    }
}

fn traverse_path(root: &Value, segments: &[&str]) -> Option<Value> {
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
    if current.is_null() { None } else { Some(current) }
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null      => String::new(),
        Value::Bool(b)   => b.to_string(),
        Value::Number(n) => n.to_string(),
        other            => other.to_string(),
    }
}

fn try_resolve_function(
    expr: &str,
    workflow: &Workflow,
    name_to_id: &HashMap<String, String>,
    ctx: &ExecutionContext,
    env_allowlist: Option<&HashSet<String>>,
) -> Option<(String, Vec<String>)> {
    let paren_pos = expr.find('(')?;
    let func_name = &expr[..paren_pos];

    // Validate: func_name must be a valid identifier [a-zA-Z_][a-zA-Z0-9_]*
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

/// Returns the string inside the outermost `()`.
/// `input` begins immediately after the opening `(`.
fn find_matching_close(input: &str) -> Option<&str> {
    let mut depth = 1i32;
    let mut in_single = false;
    let mut in_double = false;
    let mut escape_next = false;

    for (i, c) in input.char_indices() {
        if escape_next { escape_next = false; continue; }
        if c == '\\' && (in_single || in_double) { escape_next = true; continue; }
        if c == '\'' && !in_double { in_single = !in_single; continue; }
        if c == '"'  && !in_single { in_double = !in_double; continue; }
        if !in_single && !in_double {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 { return Some(&input[..i]); }
                }
                _ => {}
            }
        }
    }
    None
}

/// Split `args` by commas, respecting nested parens and string literals.
fn split_args(args: &str) -> Vec<&str> {
    if args.trim().is_empty() { return vec![]; }
    let mut result = Vec::new();
    let mut depth = 0i32;
    let mut in_single = false;
    let mut in_double = false;
    let mut escape_next = false;
    let mut start = 0;

    for (i, c) in args.char_indices() {
        if escape_next { escape_next = false; continue; }
        if c == '\\' && (in_single || in_double) { escape_next = true; continue; }
        if c == '\'' && !in_double { in_single = !in_single; continue; }
        if c == '"'  && !in_single { in_double = !in_double; continue; }
        if !in_single && !in_double {
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                ',' if depth == 0 => { result.push(&args[start..i]); start = i + 1; }
                _ => {}
            }
        }
    }
    result.push(&args[start..]);
    result
}

/// Apply a named function to resolved string arguments.
/// `apply_function` returns `String` — no `?` operator is used here.
fn apply_function(name: &str, args: &[String], warnings: &mut Vec<String>) -> String {
    // Helper: check minimum arg count; push warning and return empty string if violated.
    macro_rules! need {
        ($n:expr) => {
            if args.len() < $n {
                warnings.push(format!(
                    "{}() requires at least {} argument(s), got {}",
                    name, $n, args.len()
                ));
                return String::new();
            }
        };
    }

    match name {
        // ── String ────────────────────────────────────────────────────────────
        "upper"      => return args.first().map(|s| s.to_uppercase()).unwrap_or_default(),
        "lower"      => return args.first().map(|s| s.to_lowercase()).unwrap_or_default(),
        "trim"       => return args.first().map(|s| s.trim().to_string()).unwrap_or_default(),
        "trim_start" => return args.first().map(|s| s.trim_start().to_string()).unwrap_or_default(),
        "trim_end"   => return args.first().map(|s| s.trim_end().to_string()).unwrap_or_default(),
        _ => {}
    }

    match name {
        "len" => {
            let s = args.first().map(String::as_str).unwrap_or("");
            if let Ok(Value::Array(a)) = serde_json::from_str::<Value>(s) {
                return a.len().to_string();
            }
            s.chars().count().to_string()
        }
        "contains" => {
            need!(2);
            args[0].contains(args[1].as_str()).to_string()
        }
        "starts_with" => {
            need!(2);
            args[0].starts_with(args[1].as_str()).to_string()
        }
        "ends_with" => {
            need!(2);
            args[0].ends_with(args[1].as_str()).to_string()
        }
        "slice" => {
            need!(3);
            let chars: Vec<char> = args[0].chars().collect();
            let len = chars.len();
            let start = parse_usize_clamped(&args[1], len);
            let end   = parse_usize_clamped(&args[2], len);
            if start > end { return String::new(); }
            chars[start..end].iter().collect()
        }
        "replace" => {
            need!(3);
            match args[0].find(args[1].as_str()) {
                Some(pos) => {
                    let mut r = args[0][..pos].to_string();
                    r.push_str(&args[2]);
                    r.push_str(&args[0][pos + args[1].len()..]);
                    r
                }
                None => args[0].clone(),
            }
        }
        "replace_all" => {
            need!(3);
            args[0].replace(args[1].as_str(), &args[2])
        }
        "split" => {
            need!(2);
            let parts: Vec<Value> = args[0].split(args[1].as_str())
                .map(|p| Value::String(p.to_string()))
                .collect();
            Value::Array(parts).to_string()
        }
        "join" => {
            need!(2);
            match serde_json::from_str::<Value>(&args[0]) {
                Ok(Value::Array(arr)) => arr.iter()
                    .map(value_to_string)
                    .collect::<Vec<_>>()
                    .join(&args[1]),
                _ => args[0].clone(),
            }
        }
        "pad_start" => {
            need!(3);
            let target = parse_usize_clamped(&args[1], 10_000);
            let pad_ch = args[2].chars().next().unwrap_or(' ');
            let n = args[0].chars().count();
            if n >= target { return args[0].clone(); }
            let padding: String = std::iter::repeat_n(pad_ch, target - n).collect();
            format!("{}{}", padding, args[0])
        }
        "pad_end" => {
            need!(3);
            let target = parse_usize_clamped(&args[1], 10_000);
            let pad_ch = args[2].chars().next().unwrap_or(' ');
            let n = args[0].chars().count();
            if n >= target { return args[0].clone(); }
            let padding: String = std::iter::repeat_n(pad_ch, target - n).collect();
            format!("{}{}", args[0], padding)
        }

        // ── Number ────────────────────────────────────────────────────────────
        "round" => {
            let n = parse_f64_arg(args.first());
            let decimals = args.get(1).and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
            let factor = 10f64.powi(decimals as i32);
            format!("{:.prec$}", (n * factor).round() / factor, prec = decimals as usize)
        }
        "floor" => parse_f64_arg(args.first()).floor().to_string(),
        "ceil"  => parse_f64_arg(args.first()).ceil().to_string(),
        "abs"   => parse_f64_arg(args.first()).abs().to_string(),
        "min"   => {
            need!(2);
            parse_f64_arg(Some(&args[0])).min(parse_f64_arg(Some(&args[1]))).to_string()
        }
        "max"   => {
            need!(2);
            parse_f64_arg(Some(&args[0])).max(parse_f64_arg(Some(&args[1]))).to_string()
        }
        "to_int" => {
            let s = args.first().map(|s| s.trim()).unwrap_or("");
            s.parse::<i64>()
                .or_else(|_| s.parse::<f64>().map(|f| f as i64))
                .map(|n| n.to_string())
                .unwrap_or_default()
        }
        "to_float" => {
            args.first().map(|s| s.trim()).unwrap_or("")
                .parse::<f64>().map(|n| n.to_string()).unwrap_or_default()
        }

        // ── Date ──────────────────────────────────────────────────────────────
        "now" => chrono::Utc::now().to_rfc3339(),
        "format_date" => {
            if args.is_empty() {
                warnings.push("format_date() requires at least 1 argument: format_date(timestamp, format)".to_string());
                return String::new();
            }
            let fmt = args.get(1).map(String::as_str).unwrap_or("YYYY-MM-DD");
            format_date_string(&args[0], fmt, warnings)
        }
        "parse_date" => {
            if args.is_empty() {
                warnings.push("parse_date() requires 1 argument: parse_date(string)".to_string());
                return String::new();
            }
            parse_date_to_iso(&args[0], warnings)
        }
        "add_days" => {
            need!(2);
            date_add(&args[0], &args[1], "days", warnings)
        }
        "add_hours" => {
            need!(2);
            date_add(&args[0], &args[1], "hours", warnings)
        }
        "add_minutes" => {
            need!(2);
            date_add(&args[0], &args[1], "minutes", warnings)
        }
        "date_diff" => {
            need!(3);
            date_diff(&args[0], &args[1], &args[2], warnings)
        }

        // ── Array / Object ────────────────────────────────────────────────────
        "first" => {
            let s = args.first().map(String::as_str).unwrap_or("[]");
            match serde_json::from_str::<Value>(s) {
                Ok(Value::Array(arr)) => arr.first().map(value_to_string).unwrap_or_default(),
                _ => String::new(),
            }
        }
        "last" => {
            let s = args.first().map(String::as_str).unwrap_or("[]");
            match serde_json::from_str::<Value>(s) {
                Ok(Value::Array(arr)) => arr.last().map(value_to_string).unwrap_or_default(),
                _ => String::new(),
            }
        }
        "nth" => {
            need!(2);
            let idx = parse_usize_clamped(&args[1], usize::MAX);
            match serde_json::from_str::<Value>(&args[0]) {
                Ok(Value::Array(arr)) => arr.get(idx).map(value_to_string).unwrap_or_default(),
                _ => String::new(),
            }
        }
        "keys" => {
            let s = args.first().map(String::as_str).unwrap_or("{}");
            match serde_json::from_str::<Value>(s) {
                Ok(Value::Object(map)) => {
                    let keys: Vec<Value> = map.keys().map(|k| Value::String(k.clone())).collect();
                    Value::Array(keys).to_string()
                }
                _ => "[]".to_string(),
            }
        }
        "values" => {
            let s = args.first().map(String::as_str).unwrap_or("{}");
            match serde_json::from_str::<Value>(s) {
                Ok(Value::Object(map)) => {
                    let vals: Vec<Value> = map.values().cloned().collect();
                    Value::Array(vals).to_string()
                }
                _ => "[]".to_string(),
            }
        }

        // ── Conditional ───────────────────────────────────────────────────────
        "if" => {
            need!(3);
            if is_truthy(&args[0]) { args[1].clone() } else { args[2].clone() }
        }

        // ── Unknown ───────────────────────────────────────────────────────────
        other => {
            warnings.push(format!(
                "Unknown function '{}'. See the Functions section in the expression picker for all available functions.",
                other
            ));
            String::new()
        }
    }
}

fn format_date_string(ts: &str, fmt: &str, warnings: &mut Vec<String>) -> String {
    use chrono::{Datelike, DateTime, Timelike, Utc};
    let dt: DateTime<Utc> = match ts.parse::<DateTime<Utc>>()
        .or_else(|_| chrono::DateTime::parse_from_rfc2822(ts).map(|d| d.with_timezone(&Utc)))
    {
        Ok(d) => d,
        Err(_) => {
            warnings.push(format!("format_date(): could not parse '{}' as an ISO 8601 timestamp", ts));
            return String::new();
        }
    };
    fmt.replace("YYYY", &format!("{:04}", dt.year()))
       .replace("MM",   &format!("{:02}", dt.month()))
       .replace("DD",   &format!("{:02}", dt.day()))
       .replace("HH",   &format!("{:02}", dt.hour()))
       .replace("mm",   &format!("{:02}", dt.minute()))
       .replace("ss",   &format!("{:02}", dt.second()))
}

fn parse_date_to_iso(s: &str, warnings: &mut Vec<String>) -> String {
    use chrono::{DateTime, NaiveDate, TimeZone, Utc};
    if let Ok(d) = s.parse::<DateTime<Utc>>() { return d.to_rfc3339(); }
    if let Ok(d) = chrono::DateTime::parse_from_rfc2822(s).map(|d| d.with_timezone(&Utc)) {
        return d.to_rfc3339();
    }
    if let Ok(nd) = NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d") {
        if let Some(naive_dt) = nd.and_hms_opt(0, 0, 0) {
            return Utc.from_utc_datetime(&naive_dt).to_rfc3339();
        }
    }
    warnings.push(format!("parse_date(): could not parse '{}' as a date", s));
    String::new()
}

fn date_add(ts: &str, n_str: &str, unit: &str, warnings: &mut Vec<String>) -> String {
    use chrono::{DateTime, Duration, Utc};
    let dt: DateTime<Utc> = match ts.parse() {
        Ok(d) => d,
        Err(_) => { warnings.push(format!("date arithmetic: invalid timestamp '{}'", ts)); return String::new(); }
    };
    let n: i64 = match n_str.trim().parse() {
        Ok(v) => v,
        Err(_) => { warnings.push(format!("date arithmetic: '{}' is not a valid integer", n_str)); return String::new(); }
    };
    let result = match unit {
        "days"    => dt + Duration::days(n),
        "hours"   => dt + Duration::hours(n),
        "minutes" => dt + Duration::minutes(n),
        _         => dt,
    };
    result.to_rfc3339()
}

fn date_diff(ts1: &str, ts2: &str, unit: &str, warnings: &mut Vec<String>) -> String {
    use chrono::{DateTime, Utc};
    let d1: DateTime<Utc> = match ts1.parse() {
        Ok(d) => d,
        Err(_) => { warnings.push(format!("date_diff(): invalid timestamp '{}'", ts1)); return String::new(); }
    };
    let d2: DateTime<Utc> = match ts2.parse() {
        Ok(d) => d,
        Err(_) => { warnings.push(format!("date_diff(): invalid timestamp '{}'", ts2)); return String::new(); }
    };
    let diff = d1.signed_duration_since(d2);
    match unit {
        "days"    => diff.num_days().to_string(),
        "hours"   => diff.num_hours().to_string(),
        "minutes" => diff.num_minutes().to_string(),
        "seconds" => diff.num_seconds().to_string(),
        _ => {
            warnings.push(format!("date_diff(): unknown unit '{}'. Valid: days, hours, minutes, seconds", unit));
            String::new()
        }
    }
}

fn parse_f64_arg(s: Option<&String>) -> f64 {
    s.and_then(|v| v.trim().parse::<f64>().ok()).unwrap_or(0.0)
}

fn parse_usize_clamped(s: &str, max: usize) -> usize {
    s.trim().parse::<i64>().map(|n| (n.max(0) as usize).min(max)).unwrap_or(0)
}

fn is_truthy(s: &str) -> bool {
    !s.is_empty() && s != "false" && s != "0" && s != "null"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ExecutionContext, WorkflowNode};
    use serde_json::json;

    fn make_workflow(nodes: Vec<(&str, &str)>) -> Workflow {
        Workflow {
            schema_version: "1.0".to_string(),
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
        }
    }

    fn make_ctx(outputs: Vec<(&str, Value)>, vars: Vec<(&str, Value)>) -> ExecutionContext {
        let mut node_outputs = std::collections::HashMap::new();
        for (k, v) in outputs { node_outputs.insert(k.to_string(), v); }
        let mut variables = std::collections::HashMap::new();
        for (k, v) in vars { variables.insert(k.to_string(), v); }
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("execution_id".to_string(), json!("exec_abc123"));
        ExecutionContext { variables, node_outputs, metadata }
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
    fn simple_field_access() {
        let wf = make_workflow(vec![("n1", "HTTP Request")]);
        let ctx = make_ctx(vec![("n1", json!({"body": {"name": "Alice"}}))], vec![]);
        let (out, warns) = resolve_string("Hello {{HTTP Request.output.body.name}}", &wf, &ctx, None);
        assert_eq!(out, "Hello Alice");
        assert!(warns.is_empty());
    }

    #[test]
    fn array_index_access() {
        let wf = make_workflow(vec![("n1", "Fetch")]);
        let ctx = make_ctx(vec![("n1", json!({"items": ["a", "b", "c"]}))], vec![]);
        let (out, warns) = resolve_string("{{Fetch.output.items[1]}}", &wf, &ctx, None);
        assert_eq!(out, "b");
        assert!(warns.is_empty());
    }

    #[test]
    fn missing_node_warns_and_returns_empty() {
        let wf = make_workflow(vec![]);
        let ctx = make_ctx(vec![], vec![]);
        let (out, warns) = resolve_string("{{Ghost.output.field}}", &wf, &ctx, None);
        assert_eq!(out, "");
        assert!(!warns.is_empty());
    }

    #[test]
    fn run_id_special() {
        let wf = make_workflow(vec![]);
        let ctx = make_ctx(vec![], vec![]);
        let (out, warns) = resolve_string("run={{$run.id}}", &wf, &ctx, None);
        assert_eq!(out, "run=exec_abc123");
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
    fn fn_upper() {
        let wf = make_workflow(vec![("n1", "Step")]);
        let ctx = make_ctx(vec![("n1", json!({"name": "alice"}))], vec![]);
        let (out, warns) = resolve_string("{{upper(Step.output.name)}}", &wf, &ctx, None);
        assert_eq!(out, "ALICE");
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
        let (out, _) = resolve_string(r#"{{replace("hello world", "world", "flowo")}}"#, &wf, &ctx, None);
        assert_eq!(out, "hello flowo");
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

    // $env disabled by default (allowlist = None)
    #[test]
    fn env_disabled_by_default() {
        std::env::set_var("FLOWO_TEST_ENV_DISABLED", "test_value_123");
        let wf = make_workflow(vec![]);
        let ctx = make_ctx(vec![], vec![]);
        let (out, warns) = resolve_string("{{$env.FLOWO_TEST_ENV_DISABLED}}", &wf, &ctx, None);
        assert_eq!(out, "");
        assert!(!warns.is_empty());
        assert!(warns[0].contains("disabled"));
        std::env::remove_var("FLOWO_TEST_ENV_DISABLED");
    }

    // $env allowed when var is in allowlist
    #[test]
    fn env_resolves_when_allowed() {
        std::env::set_var("FLOWO_TEST_ENV_ALLOWED", "test_value_123");
        let wf = make_workflow(vec![]);
        let ctx = make_ctx(vec![], vec![]);
        let allowlist: HashSet<String> = ["FLOWO_TEST_ENV_ALLOWED".to_string()].into();
        let (out, warns) = resolve_string("{{$env.FLOWO_TEST_ENV_ALLOWED}}", &wf, &ctx, Some(&allowlist));
        assert_eq!(out, "test_value_123");
        assert!(warns.is_empty());
        std::env::remove_var("FLOWO_TEST_ENV_ALLOWED");
    }

    // $env blocked when var not in allowlist
    #[test]
    fn env_blocked_when_not_in_allowlist() {
        std::env::set_var("FLOWO_TEST_ENV_BLOCKED", "test_value_123");
        let wf = make_workflow(vec![]);
        let ctx = make_ctx(vec![], vec![]);
        let allowlist: HashSet<String> = ["OTHER_VAR".to_string()].into();
        let (out, warns) = resolve_string("{{$env.FLOWO_TEST_ENV_BLOCKED}}", &wf, &ctx, Some(&allowlist));
        assert_eq!(out, "");
        assert!(!warns.is_empty());
        assert!(warns[0].contains("not in the allowed list"));
        std::env::remove_var("FLOWO_TEST_ENV_BLOCKED");
    }

    #[test]
    fn unclosed_expression_treated_as_literal() {
        let wf = make_workflow(vec![]);
        let ctx = make_ctx(vec![], vec![]);
        let (out, _) = resolve_string("text {{ unclosed", &wf, &ctx, None);
        assert!(out.contains("text "));
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
}
