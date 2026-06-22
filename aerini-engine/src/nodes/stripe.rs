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
                "api_key":  { "type": "string", "description": "Stripe secret key (sk_live_... or sk_test_...)" }
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

                // Stripe API requires application/x-www-form-urlencoded, not JSON.
                let mut params = vec![
                    ("amount", amount.to_string()),
                    ("currency", currency),
                ];

                if let Some(desc) = input.input["description"].as_str().filter(|s| !s.is_empty()) {
                    params.push(("description", desc.to_string()));
                }

                match super::shared_http_client()
                    .post("https://api.stripe.com/v1/payment_intents")
                    .header("Authorization", format!("Bearer {}", api_key))
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

