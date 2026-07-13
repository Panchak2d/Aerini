use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::Node;

pub struct StripeNode;

#[async_trait]
impl Node for StripeNode {
    fn type_id(&self) -> &'static str { "stripe" }
    fn display_name(&self) -> &'static str { "Stripe" }
    fn node_type(&self) -> NodeType { NodeType::Action }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Interact with the Stripe API: create charges, customers, payment intents, and more." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action":   { "type": "string", "enum": ["create_payment_intent"], "description": "Stripe operation to perform" },
                "amount":   { "type": "number", "description": "Amount in the smallest currency unit (e.g. 1000 for $10.00 USD)" },
                "currency": { "type": "string", "description": "Three-letter ISO currency code, lowercase (e.g. 'usd')" },
                "description": { "type": "string", "description": "Optional description for this payment intent" },
                "api_key":  { "type": "string", "description": "Stripe secret key (sk_live_... or sk_test_...)" },
                "idempotency_key": { "type": "string", "description": "Optional value identifying this specific charge (e.g. an order ID) — prevents Stripe from creating a duplicate PaymentIntent if this node is retried or re-run with the same value. If omitted, a key is derived automatically from this run and the resolved amount/currency/description." }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id":            { "type": "string", "description": "PaymentIntent ID (pi_...)" },
                "client_secret": { "type": "string", "description": "Client secret for frontend confirmation" },
                "status":        { "type": "string" },
                "amount":        { "type": "number" },
                "currency":      { "type": "string" }
            }
        })
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let api_key = match input.input["api_key"].as_str().filter(|s| !s.is_empty()) {
            Some(k) => k.to_string(),
            None => return NodeOutput::failure(NodeError::unrecoverable(
                "MISSING_API_KEY",
                "Stripe secret key is required — add it via the credential store",
            )),
        };

        let action = input.input["action"].as_str().unwrap_or("create_payment_intent");

        match action {
            "create_payment_intent" => {
                let amount = match input.input["amount"].as_u64() {
                    Some(a) if a > 0 => a,
                    _ => return NodeOutput::failure(NodeError::unrecoverable(
                        "MISSING_AMOUNT",
                        "amount is required and must be a positive integer (smallest currency unit)",
                    )),
                };

                let currency = match input.input["currency"].as_str().filter(|s| !s.is_empty()) {
                    Some(c) => c.to_lowercase(),
                    None => return NodeOutput::failure(NodeError::unrecoverable("MISSING_CURRENCY", "currency is required (e.g. 'usd')")),
                };

                let description = input.input["description"].as_str().unwrap_or("");
                let caller_key  = input.input["idempotency_key"].as_str().filter(|s| !s.is_empty());
                let idempotency_key = resolve_idempotency_key(
                    caller_key, &input.execution_id, &input.node_id, amount, &currency, description,
                );

                // Stripe API requires application/x-www-form-urlencoded, not JSON.
                let mut params = vec![
                    ("amount", amount.to_string()),
                    ("currency", currency),
                ];

                if !description.is_empty() {
                    params.push(("description", description.to_string()));
                }

                match super::shared_http_client()
                    .post("https://api.stripe.com/v1/payment_intents")
                    .header("Authorization", format!("Bearer {}", api_key))
                    .header("Idempotency-Key", idempotency_key)
                    .form(&params)
                    .send()
                    .await
                {
                    Ok(resp) => {
                        let status = resp.status().as_u16();
                        match resp.json::<Value>().await {
                            Ok(v) => {
                                if status == 200 || status == 201 {
                                    let intent_id = v["id"].as_str().unwrap_or("").to_string();
                                    NodeOutput::success_with_logs(
                                        v,
                                        vec![format!("Stripe PaymentIntent created: {}", intent_id)],
                                    )
                                } else {
                                    let msg = v["error"]["message"].as_str().unwrap_or("unknown error").to_string();
                                    NodeOutput::failure(NodeError::unrecoverable("STRIPE_ERROR", format!("HTTP {}: {}", status, msg)))
                                }
                            }
                            Err(e) => NodeOutput::failure(NodeError::unrecoverable("PARSE_ERROR", e.to_string())),
                        }
                    }
                    Err(e) => {
                        super::util::http_err_output(&e)
                    }
                }
            }
            other => NodeOutput::failure(NodeError::unrecoverable(
                "INVALID_ACTION",
                format!("Unknown action '{}'. Valid values: create_payment_intent", other),
            )),
        }
    }
}

/// Derive the `Idempotency-Key` header value for a `create_payment_intent` call.
///
/// Must be stable across `execute_with_retry`'s clone-and-retry of one
/// invocation (same `execution_id`, `node_id`, and resolved params on every
/// attempt — see `executor/mod.rs::execute_with_retry`), but distinct across
/// genuinely separate invocations. Two invocation shapes share `execution_id`
/// and `node_id` yet must still get different keys: loop iterations of this
/// same node (typically resolve a different amount/currency/description per
/// item) and separate scheduled runs of the same node (get a fresh
/// `execution_id` per run — see `context.rs`). Combining
/// `execution_id + node_id + amount + currency + description` satisfies both.
///
/// A caller-supplied key always takes precedence (Stripe's own recommended
/// pattern — tie it to an order/cart ID). Either way the result is hashed
/// through blake3 (already a workspace dependency, same idiom as
/// `database/pool.rs`) so the header value is always a fixed-length ASCII
/// hex string, regardless of what raw text the description or a
/// caller-supplied key contain — raw text could otherwise fail reqwest's
/// `HeaderValue` conversion (e.g. an embedded newline).
fn resolve_idempotency_key(
    caller_supplied: Option<&str>,
    execution_id: &str,
    node_id: &str,
    amount: u64,
    currency: &str,
    description: &str,
) -> String {
    let key_material = match caller_supplied {
        Some(k) => k.to_string(),
        None => format!("{}|{}|{}|{}|{}", execution_id, node_id, amount, currency, description),
    };
    blake3::hash(key_material.as_bytes()).to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // T1-13 (S1-2): no request previously carried an Idempotency-Key header
    // at all, so a recoverable failure retried by `execute_with_retry`
    // (executor/mod.rs) created a second, distinct PaymentIntent for the
    // same purchase. These tests exercise the real `resolve_idempotency_key`
    // function `execute()` calls — not a duplicated copy — covering the two
    // properties the fix requires: stable across retries, distinct across
    // genuinely separate invocations.
    //
    // Scope note (Rule 6/7): `execute()` posts to the hardcoded
    // `https://api.stripe.com` — there is no injectable base_url, so
    // end-to-end header-on-the-wire coverage would require either a real
    // network call to Stripe or a refactor to make the endpoint
    // configurable, both out of scope for this fix. The pre-existing
    // `Authorization` header a few lines above `Idempotency-Key` in the same
    // request-builder chain has never had that coverage either; holding
    // this one-line addition to a higher bar than its neighbor would be
    // inconsistent. Manually traced: `.header("Idempotency-Key", idempotency_key)`
    // sits in the same builder chain as the already-working `Authorization`
    // header, using the identical `reqwest` header-setting idiom.

    #[test]
    fn idempotency_key_stable_across_retries() {
        // execute_with_retry clones the identical NodeInput (same execution_id,
        // node_id, and resolved config) across every retry attempt — so the
        // derived key must be identical for two "attempts" with the same inputs.
        let k1 = resolve_idempotency_key(None, "exec-1", "stripe_node", 1000, "usd", "order #1");
        let k2 = resolve_idempotency_key(None, "exec-1", "stripe_node", 1000, "usd", "order #1");
        assert_eq!(k1, k2, "same execution_id/node_id/params must yield the same Idempotency-Key across retries");
    }

    #[test]
    fn idempotency_key_distinct_for_distinct_requests() {
        let base = resolve_idempotency_key(None, "exec-1", "stripe_node", 1000, "usd", "order #1");
        let different_amount = resolve_idempotency_key(None, "exec-1", "stripe_node", 2000, "usd", "order #1");
        let different_run     = resolve_idempotency_key(None, "exec-2", "stripe_node", 1000, "usd", "order #1");
        assert_ne!(base, different_amount, "different amount must yield a different key (e.g. distinct loop iterations)");
        assert_ne!(base, different_run, "different execution_id must yield a different key (e.g. separate scheduled runs)");
    }

    #[test]
    fn caller_supplied_key_overrides_and_is_hashed() {
        let k1 = resolve_idempotency_key(Some("order-42"), "exec-1", "stripe_node", 1000, "usd", "order #1");
        let k2 = resolve_idempotency_key(Some("order-42"), "exec-2", "stripe_node", 9999, "eur", "different");
        assert_eq!(k1, k2, "a caller-supplied key must take precedence over execution_id/node_id/params");
        assert_ne!(k1, "order-42", "the raw caller-supplied value must be hashed, never sent as a literal header value");
        assert!(k1.chars().all(|c| c.is_ascii_hexdigit()), "hashed key must be a plain hex string, always a valid header value");
    }

    #[test]
    fn empty_caller_supplied_key_falls_back_to_default() {
        // execute() filters out an empty-string idempotency_key before calling
        // this function (`.filter(|s| !s.is_empty())`) — confirm the fallback
        // path (None) still produces the deterministic default, not a
        // caller-empty-string special case this function would need to guard.
        let k = resolve_idempotency_key(None, "exec-1", "stripe_node", 1000, "usd", "");
        assert!(!k.is_empty());
        assert!(k.chars().all(|c| c.is_ascii_hexdigit()));
    }
}

