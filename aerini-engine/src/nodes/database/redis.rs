use serde_json::json;

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput};

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

    // Use redis::cmd() raw API for all operations — stable across crate versions.
    // redis_cmd_with_retry handles connection caching and evict-and-retry on IoError.
    // residual: explicit, ungated opt-in — see util.rs::check_db_url_ssrf's
    // doc comment for why this is not gated behind __caller_is_admin the way
    // allow_raw_sql is (that gate is never set true on desktop today).
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
            match super::pool::redis_cmd_with_retry::<Option<String>, _>(&url, || {
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
            let value = input.input["value"].as_str().unwrap_or("").to_string();
            let ttl = input.input["expire"].as_u64();
            match super::pool::redis_cmd_with_retry::<(), _>(&url, || {
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
            match super::pool::redis_cmd_with_retry::<i64, _>(&url, || {
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
            let value = match input.input["value"].as_str() {
                Some(v) => v.to_string(),
                None    => return NodeOutput::failure(NodeError::unrecoverable("MISSING_VALUE",
                    "value is required for lpush and rpush")),
            };
            let cmd_name = if operation == "lpush" { "LPUSH" } else { "RPUSH" };
            match super::pool::redis_cmd_with_retry::<i64, _>(&url, || {
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
            match super::pool::redis_cmd_with_retry::<Option<String>, _>(&url, || {
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
            match super::pool::redis_cmd_with_retry::<Option<String>, _>(&url, || {
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
            let value = input.input["value"].as_str().unwrap_or("").to_string();
            match super::pool::redis_cmd_with_retry::<(), _>(&url, || {
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
