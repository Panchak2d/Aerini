use serde_json::json;

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput};
use crate::nodes::util::{cfg_u64_opt, Replay};

const MAX_VALUE_BYTES: u64 = super::MAX_QUERY_BYTES as u64;

// ── Redis execution ───────────────────────────────────────────────────────────

pub(super) async fn execute_redis(input: NodeInput) -> NodeOutput {
    let url = match input.input["connection_url"].as_str() {
        Some(u) if !u.is_empty() => u.to_string(),
        _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_URL",
            "connection_url is required for redis. Example: redis://host:6379")),
    };
    let url = match super::url_with_password(&url, &input.input["password"]) {
        Ok(u) => u,
        Err(out) => return out,
    };
    let operation = match input.input["operation"].as_str() {
        Some(op) if !op.is_empty() => op.to_string(),
        _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_OPERATION",
            "operation is required for redis. One of: get, set, del, lpush, rpush, lpop, rpop, hget, hset")),
    };
    let key = match scalar_arg(&input.input["key"]) {
        Some(k) if !k.is_empty() => k,
        _ => return NodeOutput::failure(arg_error("key", &input.input["key"],
            "key is required for all Redis operations")),
    };

    // Not gated behind __caller_is_admin the way allow_raw_sql is: that gate is
    // never set on desktop (see util.rs::check_db_url_ssrf).
    let ssrf_policy = if input.input["allow_local"].as_bool().unwrap_or(false) {
        crate::nodes::util::SsrfPolicy::AllowLocal
    } else {
        crate::nodes::util::SsrfPolicy::Strict
    };
    if let Err(e) = crate::nodes::util::check_db_url_ssrf(&url, ssrf_policy).await {
        return NodeOutput::failure(NodeError::unrecoverable("SSRF_BLOCKED", e));
    }
    match operation.as_str() {
        "get" => {
            match super::pool::redis_cmd_with_retry::<u64, _>(&url, ssrf_policy, Replay::Safe, || {
                let mut cmd = redis::cmd("STRLEN");
                cmd.arg(&key);
                cmd
            }).await {
                Err(e)  => return NodeOutput::failure(NodeError::unrecoverable("REDIS_ERROR",
                    format!("GET failed: {}", e))),
                Ok(len) => if let Err(e) = check_value_size(len, "GET") {
                    return NodeOutput::failure(e);
                },
            }
            match super::pool::redis_cmd_with_retry::<Option<String>, _>(&url, ssrf_policy, Replay::Safe, || {
                let mut cmd = redis::cmd("GET");
                cmd.arg(&key);
                cmd
            }).await {
                Err(e)  => NodeOutput::failure(NodeError::unrecoverable("REDIS_ERROR",
                    format!("GET failed: {}", e))),
                Ok(val) => NodeOutput::success(json!({ "value": val })),
            }
        }

        "set" => {
            let value = match scalar_arg(&input.input["value"]) {
                Some(v) => v,
                None => return NodeOutput::failure(arg_error("value", &input.input["value"],
                    "value is required for set. Use an empty string to store an empty value.")),
            };
            let ttl = match cfg_u64_opt(&input.input["expire"], "expire") {
                Ok(v) => v,
                Err(e) => return NodeOutput::failure(e),
            };
            match super::pool::redis_cmd_with_retry::<(), _>(&url, ssrf_policy, Replay::Safe, || {
                let mut cmd = redis::cmd("SET");
                cmd.arg(&key).arg(&value);
                if let Some(t) = ttl {
                    cmd.arg("EX").arg(t);
                }
                cmd
            }).await {
                Err(e) => NodeOutput::failure(NodeError::unrecoverable("REDIS_ERROR",
                    format!("SET failed: {}", e))),
                Ok(_)  => NodeOutput::success(json!({ "ok": true, "key": key })),
            }
        }

        "del" => {
            match super::pool::redis_cmd_with_retry::<i64, _>(&url, ssrf_policy, Replay::Safe, || {
                let mut cmd = redis::cmd("DEL");
                cmd.arg(&key);
                cmd
            }).await {
                Err(e)    => NodeOutput::failure(NodeError::unrecoverable("REDIS_ERROR",
                    format!("DEL failed: {}", e))),
                Ok(count) => NodeOutput::success(json!({ "deleted": count, "key": key })),
            }
        }

        "lpush" | "rpush" => {
            let value = match scalar_arg(&input.input["value"]) {
                Some(v) => v,
                None    => return NodeOutput::failure(arg_error("value", &input.input["value"],
                    "value is required for lpush and rpush")),
            };
            let cmd_name = if operation == "lpush" { "LPUSH" } else { "RPUSH" };
            match super::pool::redis_cmd_with_retry::<i64, _>(&url, ssrf_policy, Replay::Never, || {
                let mut cmd = redis::cmd(cmd_name);
                cmd.arg(&key).arg(&value);
                cmd
            }).await {
                Err(e)  => NodeOutput::failure(NodeError::unrecoverable("REDIS_ERROR",
                    format!("{} failed: {}", cmd_name, e))),
                Ok(len) => NodeOutput::success(json!({ "ok": true, "list_length": len })),
            }
        }

        "lpop" | "rpop" => {
            let cmd_name = if operation == "lpop" { "LPOP" } else { "RPOP" };
            match super::pool::redis_cmd_with_retry::<Option<String>, _>(&url, ssrf_policy, Replay::Never, || {
                let mut cmd = redis::cmd(cmd_name);
                cmd.arg(&key);
                cmd
            }).await {
                Err(e)  => NodeOutput::failure(NodeError::unrecoverable("REDIS_ERROR",
                    format!("{} failed: {}", cmd_name, e))),
                Ok(val) => NodeOutput::success(json!({ "value": val })),
            }
        }

        "hget" => {
            let field = match scalar_arg(&input.input["field"]) {
                Some(f) if !f.is_empty() => f,
                _ => return NodeOutput::failure(arg_error("field", &input.input["field"],
                    "field is required for hget")),
            };
            match super::pool::redis_cmd_with_retry::<u64, _>(&url, ssrf_policy, Replay::Safe, || {
                let mut cmd = redis::cmd("HSTRLEN");
                cmd.arg(&key).arg(&field);
                cmd
            }).await {
                Err(e)  => return NodeOutput::failure(NodeError::unrecoverable("REDIS_ERROR",
                    format!("HGET failed: {}", e))),
                Ok(len) => if let Err(e) = check_value_size(len, "HGET") {
                    return NodeOutput::failure(e);
                },
            }
            match super::pool::redis_cmd_with_retry::<Option<String>, _>(&url, ssrf_policy, Replay::Safe, || {
                let mut cmd = redis::cmd("HGET");
                cmd.arg(&key).arg(&field);
                cmd
            }).await {
                Err(e)  => NodeOutput::failure(NodeError::unrecoverable("REDIS_ERROR",
                    format!("HGET failed: {}", e))),
                Ok(val) => NodeOutput::success(json!({ "value": val })),
            }
        }

        "hset" => {
            let field = match scalar_arg(&input.input["field"]) {
                Some(f) if !f.is_empty() => f,
                _ => return NodeOutput::failure(arg_error("field", &input.input["field"],
                    "field is required for hset")),
            };
            let value = match scalar_arg(&input.input["value"]) {
                Some(v) => v,
                None => return NodeOutput::failure(arg_error("value", &input.input["value"],
                    "value is required for hset. Use an empty string to store an empty value.")),
            };
            match super::pool::redis_cmd_with_retry::<(), _>(&url, ssrf_policy, Replay::Safe, || {
                let mut cmd = redis::cmd("HSET");
                cmd.arg(&key).arg(&field).arg(&value);
                cmd
            }).await {
                Err(e) => NodeOutput::failure(NodeError::unrecoverable("REDIS_ERROR",
                    format!("HSET failed: {}", e))),
                Ok(_)  => NodeOutput::success(json!({ "ok": true })),
            }
        }

        other => NodeOutput::failure(NodeError::unrecoverable("UNKNOWN_OPERATION", format!(
            "Unknown Redis operation '{}'. Valid: get, set, del, lpush, rpush, lpop, rpop, hget, hset",
            other
        ))),
    }
}

fn scalar_arg(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn arg_error(name: &str, v: &serde_json::Value, missing: &str) -> NodeError {
    let upper = name.to_ascii_uppercase();
    match v {
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => NodeError::unrecoverable(
            format!("INVALID_{upper}"),
            format!(
                "{name} must be a string, number or true/false, not {}. Convert it to a JSON string first.",
                if v.is_array() { "an array" } else { "an object" }
            ),
        ),
        _ => NodeError::unrecoverable(format!("MISSING_{upper}"), missing),
    }
}

fn check_value_size(len: u64, command: &str) -> Result<(), NodeError> {
    if len > MAX_VALUE_BYTES {
        return Err(NodeError::unrecoverable(
            "VALUE_TOO_LARGE",
            format!(
                "{command} would return {} MiB; the limit is {} MiB. Store the value in smaller pieces under separate keys.",
                len.div_ceil(1024 * 1024),
                MAX_VALUE_BYTES / (1024 * 1024)
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{arg_error, check_value_size, scalar_arg, MAX_VALUE_BYTES};
    use serde_json::json;

    #[test]
    fn structured_argument_is_invalid_and_absent_argument_is_missing() {
        let invalid = arg_error("value", &json!({"a": 1}), "value is required");
        assert_eq!(invalid.code, "INVALID_VALUE");
        assert!(invalid.message.contains("an object"), "got: {}", invalid.message);
        let missing = arg_error("value", &json!(null), "value is required");
        assert_eq!(missing.code, "MISSING_VALUE");
        assert_eq!(missing.message, "value is required");
    }

    #[test]
    fn value_over_the_size_limit_is_rejected() {
        assert!(check_value_size(MAX_VALUE_BYTES, "GET").is_ok());
        let err = check_value_size(MAX_VALUE_BYTES + 1, "GET").expect_err("over the limit");
        assert_eq!(err.code, "VALUE_TOO_LARGE");
        assert!(err.message.contains("32 MiB"), "got: {}", err.message);
    }

    #[test]
    fn scalar_arg_accepts_strings_numbers_and_bools_only() {
        assert_eq!(scalar_arg(&json!("a")), Some("a".to_string()));
        assert_eq!(scalar_arg(&json!("")), Some(String::new()));
        assert_eq!(scalar_arg(&json!(42)), Some("42".to_string()));
        assert_eq!(scalar_arg(&json!(true)), Some("true".to_string()));
        assert_eq!(scalar_arg(&json!(null)), None);
        assert_eq!(scalar_arg(&json!(["x"])), None);
    }
}
