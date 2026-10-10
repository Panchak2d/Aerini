use serde_json::json;

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput};
use crate::nodes::util::{cfg_u64_opt, Replay};

// ── Redis execution ───────────────────────────────────────────────────────────

pub(super) async fn execute_redis(input: NodeInput) -> NodeOutput {
    let url = match input.input["connection_url"].as_str() {
        Some(u) if !u.is_empty() => u.to_string(),
        _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_URL",
            "connection_url is required for redis. Example: redis://host:6379")),
    };
    let operation = match input.input["operation"].as_str() {
        Some(op) if !op.is_empty() => op.to_string(),
        _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_OPERATION",
            "operation is required for redis. One of: get, set, del, lpush, rpush, lpop, rpop, hget, hset")),
    };
    let key = match input.input["key"].as_str() {
        Some(k) if !k.is_empty() => k.to_string(),
        _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_KEY",
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
                None => return NodeOutput::failure(NodeError::unrecoverable("MISSING_VALUE",
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
                None    => return NodeOutput::failure(NodeError::unrecoverable("MISSING_VALUE",
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
            let field = match input.input["field"].as_str() {
                Some(f) if !f.is_empty() => f.to_string(),
                _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_FIELD",
                    "field is required for hget")),
            };
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
            let field = match input.input["field"].as_str() {
                Some(f) if !f.is_empty() => f.to_string(),
                _ => return NodeOutput::failure(NodeError::unrecoverable("MISSING_FIELD",
                    "field is required for hset")),
            };
            let value = match scalar_arg(&input.input["value"]) {
                Some(v) => v,
                None => return NodeOutput::failure(NodeError::unrecoverable("MISSING_VALUE",
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

#[cfg(test)]
mod tests {
    use super::scalar_arg;
    use serde_json::json;

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
