use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::Node;
use super::util::cfg_u64;

pub struct StripeNode;

#[async_trait]
impl Node for StripeNode {
    fn type_id(&self) -> &'static str { "stripe" }
    fn display_name(&self) -> &'static str { "Stripe" }
    fn node_type(&self) -> NodeType { NodeType::Action }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Create a Stripe PaymentIntent via the Stripe API." }

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
                let amount = match cfg_u64(&input.input["amount"]) {
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
                // Distinguishes loop-body iterations that happen to resolve
                // identical amount/currency/description — see
                // resolve_idempotency_key's doc comment.
                let loop_iteration = input.context.metadata.get("__loop_iteration_index")
                    .and_then(|v| v.as_u64());
                let idempotency_key = resolve_idempotency_key(
                    caller_key, &input.execution_id, &input.node_id, amount, &currency, description,
                    loop_iteration,
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
                        match super::util::read_json_response_capped(resp).await {
                            Ok(v) => {
                                if status == 200 || status == 201 {
                                    let intent_id = v["id"].as_str().unwrap_or("").to_string();
                                    NodeOutput::success_with_logs(
                                        v,
                                        vec![format!("Stripe PaymentIntent created: {}", intent_id)],
                                    )
                                } else {
                                    let msg = v["error"]["message"].as_str().unwrap_or("unknown error").to_string();
                                    // Safe only because the Idempotency-Key above is the same on every retry.
                                    NodeOutput::failure(super::util::provider_error_for(super::util::Replay::Safe, status, "STRIPE_ERROR", format!("HTTP {}: {}", status, msg)))
                                }
                            }
                            Err(e) => NodeOutput::failure(super::util::provider_error_for(super::util::Replay::Safe, status, "PARSE_ERROR", e)),
                        }
                    }
                    Err(e) => {
                        super::util::http_err_output(super::util::Replay::Safe, &e)
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
/// pattern — tie it to an order/cart ID) and is never combined with
/// `loop_iteration` — an explicit caller value is the user's own choice to
/// make unique (typically via a `{{...}}` expression resolving per-item
/// data), matching this key's existing "caller value overrides everything
/// else" contract. Either way the result is hashed through blake3 (already a
/// workspace dependency, same idiom as `database/pool.rs`) so the header
/// value is always a fixed-length ASCII hex string, regardless of what raw
/// text the description or a caller-supplied key contain — raw text could
/// otherwise fail reqwest's `HeaderValue` conversion (e.g. an embedded
/// newline).
///
/// `execution_id + node_id + amount + currency + description` alone does
/// not guarantee distinctness across iterations of a loop body
/// containing this node — two iterations can resolve identical
/// amount/currency/description (e.g. charging the same flat fee to N
/// different customers, where only an upstream node's customer id differs
/// and that id never enters this key). Without a per-iteration signal, that
/// collision makes Stripe treat the second charge as a retry of the first
/// and silently return the first PaymentIntent instead of creating a new
/// one — a real charge silently never happens. `loop_iteration` is
/// `Some(i)` (the current 0-based loop-body iteration) when this node
/// executes inside a Loop, sourced from `__loop_iteration_index`
/// (`executor/mod.rs::build_input`); folding it into the key material closes
/// the collision instead of relying on the other fields happening to differ.
fn resolve_idempotency_key(
    caller_supplied: Option<&str>,
    execution_id: &str,
    node_id: &str,
    amount: u64,
    currency: &str,
    description: &str,
    loop_iteration: Option<u64>,
) -> String {
    let key_material = match caller_supplied {
        Some(k) => k.to_string(),
        None => match loop_iteration {
            Some(i) => format!("{}|{}|{}|{}|{}|iter{}", execution_id, node_id, amount, currency, description, i),
            None => format!("{}|{}|{}|{}|{}", execution_id, node_id, amount, currency, description),
        },
    };
    blake3::hash(key_material.as_bytes()).to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests exercise the real `resolve_idempotency_key` function
    // `execute()` calls — not a duplicated copy — covering: stable across
    // retries, distinct across genuinely separate invocations.
    //
    // `execute` posts to the hardcoded `https://api.stripe.com` with no
    // injectable base_url, so header-on-the-wire coverage would require a
    // real network call or an endpoint-configurability refactor — the
    // neighboring `Authorization` header, set in the same builder chain,
    // has never had that coverage either. `.header("Idempotency-Key",
    // idempotency_key)` uses the identical `reqwest` header-setting idiom.

    #[tokio::test]
    async fn amount_given_as_numeric_string_passes_amount_validation() {
        use crate::model::ExecutionContext;

        async fn run(amount: &str) -> NodeOutput {
            StripeNode.execute(NodeInput {
                resolved_credentials: std::collections::HashMap::new(),
                cancel_token: None,
                node_id:      "n1".into(),
                workflow_id:  "w1".into(),
                execution_id: "e1".into(),
                input:        json!({ "api_key": "sk_test_x", "action": "create_payment_intent", "amount": amount }),
                context:      ExecutionContext::default(),
            }).await
        }

        assert_eq!(run("1000").await.error.expect("must fail without currency").code, "MISSING_CURRENCY");
        assert_eq!(run("10.5").await.error.expect("must reject a fraction").code, "MISSING_AMOUNT");
    }

    #[test]
    fn idempotency_key_stable_across_retries() {
        // execute_with_retry clones the identical NodeInput (same execution_id,
        // node_id, and resolved config) across every retry attempt — so the
        // derived key must be identical for two "attempts" with the same inputs.
        let k1 = resolve_idempotency_key(None, "exec-1", "stripe_node", 1000, "usd", "order #1", None);
        let k2 = resolve_idempotency_key(None, "exec-1", "stripe_node", 1000, "usd", "order #1", None);
        assert_eq!(k1, k2, "same execution_id/node_id/params must yield the same Idempotency-Key across retries");
    }

    #[test]
    fn idempotency_key_distinct_for_distinct_requests() {
        let base = resolve_idempotency_key(None, "exec-1", "stripe_node", 1000, "usd", "order #1", None);
        let different_amount = resolve_idempotency_key(None, "exec-1", "stripe_node", 2000, "usd", "order #1", None);
        let different_run     = resolve_idempotency_key(None, "exec-2", "stripe_node", 1000, "usd", "order #1", None);
        assert_ne!(base, different_amount, "different amount must yield a different key");
        assert_ne!(base, different_run, "different execution_id must yield a different key (e.g. separate scheduled runs)");
    }

    #[test]
    fn caller_supplied_key_overrides_and_is_hashed() {
        let k1 = resolve_idempotency_key(Some("order-42"), "exec-1", "stripe_node", 1000, "usd", "order #1", None);
        let k2 = resolve_idempotency_key(Some("order-42"), "exec-2", "stripe_node", 9999, "eur", "different", None);
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
        let k = resolve_idempotency_key(None, "exec-1", "stripe_node", 1000, "usd", "", None);
        assert!(!k.is_empty());
        assert!(k.chars().all(|c| c.is_ascii_hexdigit()));
    }

    // ── __loop_iteration_index folded into the auto-derived key ──

    #[test]
    fn loop_iterations_with_identical_params_get_distinct_keys() {
        // Same node, same run, same amount/currency/description (e.g. a flat
        // per-item fee) across two iterations — only the iteration index
        // differs. Without folding it in, these would hash to the *same* key
        // and Stripe would silently treat charge #2 as a duplicate of charge #1.
        let iter0 = resolve_idempotency_key(None, "exec-1", "stripe_node", 500, "usd", "flat fee", Some(0));
        let iter1 = resolve_idempotency_key(None, "exec-1", "stripe_node", 500, "usd", "flat fee", Some(1));
        assert_ne!(iter0, iter1, "identical params but different loop iteration must yield different keys");
    }

    #[test]
    fn same_loop_iteration_stable_across_retries() {
        // execute_with_retry reuses the same NodeInput (same __loop_iteration_index)
        // across retry attempts within one iteration — the key must stay stable,
        // same property as idempotency_key_stable_across_retries but with a
        // loop_iteration present.
        let k1 = resolve_idempotency_key(None, "exec-1", "stripe_node", 500, "usd", "flat fee", Some(3));
        let k2 = resolve_idempotency_key(None, "exec-1", "stripe_node", 500, "usd", "flat fee", Some(3));
        assert_eq!(k1, k2, "same loop_iteration across retries must yield the same key");
    }

    #[test]
    fn absent_loop_iteration_differs_from_present_zero() {
        // None (top-level, not in a loop) and Some(0) (first loop iteration)
        // must not collide with each other, even though "0" and "absent" could
        // otherwise be conflated by a naive string-format implementation.
        let top_level = resolve_idempotency_key(None, "exec-1", "stripe_node", 500, "usd", "flat fee", None);
        let iter_zero  = resolve_idempotency_key(None, "exec-1", "stripe_node", 500, "usd", "flat fee", Some(0));
        assert_ne!(top_level, iter_zero, "no-loop-context and iteration-0 must not produce the same key");
    }

    #[test]
    fn caller_supplied_key_ignores_loop_iteration() {
        // Caller-supplied keys are the user's own uniqueness contract (per the
        // existing caller_supplied_key_overrides_and_is_hashed test) and are
        // deliberately never combined with loop_iteration.
        let k1 = resolve_idempotency_key(Some("order-42"), "exec-1", "stripe_node", 500, "usd", "flat fee", Some(0));
        let k2 = resolve_idempotency_key(Some("order-42"), "exec-1", "stripe_node", 500, "usd", "flat fee", Some(1));
        assert_eq!(k1, k2, "a caller-supplied key must stay stable regardless of loop_iteration");
    }
}

