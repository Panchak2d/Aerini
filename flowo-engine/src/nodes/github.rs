use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::OnceLock;

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::Node;

const GITHUB_API_VERSION: &str = "2022-11-28";

static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn client() -> reqwest::Client {
    HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("flowo-engine/0.2.0")
            .build()
            .expect("Failed to build GitHub HTTP client")
    }).clone()
}

pub struct GitHubNode;

#[async_trait]
impl Node for GitHubNode {
    fn type_id(&self) -> &'static str { "github" }
    fn display_name(&self) -> &'static str { "GitHub" }
    fn node_type(&self) -> NodeType { NodeType::Action }
    fn version(&self) -> &'static str { "1.0.0" }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action", "owner", "repo"],
            "properties": {
                "action":       { "type": "string", "enum": ["create_issue", "add_comment"], "description": "Operation to perform" },
                "owner":        { "type": "string", "description": "Repository owner (user or org)" },
                "repo":         { "type": "string", "description": "Repository name" },
                "title":        { "type": "string", "description": "Issue title (required for create_issue)" },
                "body":         { "type": "string", "description": "Issue body or comment text" },
                "issue_number": { "type": "number", "description": "Issue number (required for add_comment)" },
                "api_key":      { "type": "string", "description": "GitHub personal access token" }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "number":    { "type": "number", "description": "Issue number" },
                "html_url":  { "type": "string", "description": "URL of the created issue or comment" },
                "id":        { "type": "number" },
                "state":     { "type": "string" }
            }
        })
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let api_key = match input.input["api_key"].as_str().filter(|s| !s.is_empty()) {
            Some(k) => k.to_string(),
            None => return NodeOutput::failure(NodeError::unrecoverable(
                "MISSING_TOKEN",
                "GitHub personal access token is required — add it via the credential store",
            )),
        };

        let action = input.input["action"].as_str().unwrap_or("create_issue");
        let owner  = match input.input["owner"].as_str().filter(|s| !s.is_empty()) {
            Some(o) => o.to_string(),
            None => return NodeOutput::failure(NodeError::unrecoverable("MISSING_OWNER", "owner field is required")),
        };
        let repo = match input.input["repo"].as_str().filter(|s| !s.is_empty()) {
            Some(r) => r.to_string(),
            None => return NodeOutput::failure(NodeError::unrecoverable("MISSING_REPO", "repo field is required")),
        };

        let (url, request_body) = match action {
            "create_issue" => {
                let title = match input.input["title"].as_str().filter(|s| !s.is_empty()) {
                    Some(t) => t.to_string(),
                    None => return NodeOutput::failure(NodeError::unrecoverable("MISSING_TITLE", "title is required for create_issue")),
                };
                let mut body = json!({ "title": title });
                if let Some(b) = input.input["body"].as_str().filter(|s| !s.is_empty()) {
                    body["body"] = Value::String(b.to_string());
                }
                (
                    format!("https://api.github.com/repos/{}/{}/issues", owner, repo),
                    body,
                )
            }
            "add_comment" => {
                let issue_number = match input.input["issue_number"].as_u64() {
                    Some(n) => n,
                    None => return NodeOutput::failure(NodeError::unrecoverable("MISSING_ISSUE_NUMBER", "issue_number is required for add_comment")),
                };
                let comment_body = match input.input["body"].as_str().filter(|s| !s.is_empty()) {
                    Some(b) => b.to_string(),
                    None => return NodeOutput::failure(NodeError::unrecoverable("MISSING_BODY", "body is required for add_comment")),
                };
                (
                    format!("https://api.github.com/repos/{}/{}/issues/{}/comments", owner, repo, issue_number),
                    json!({ "body": comment_body }),
                )
            }
            other => return NodeOutput::failure(NodeError::unrecoverable(
                "INVALID_ACTION",
                format!("Unknown action '{}'. Valid values: create_issue, add_comment", other),
            )),
        };

        match client()
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
            .json(&request_body)
            .send()
            .await
        {
            Ok(resp) => {
                let status = resp.status().as_u16();
                match resp.json::<Value>().await {
                    Ok(v) => {
                        // GitHub returns 201 Created on success.
                        if status == 201 || status == 200 {
                            let log = match action {
                                "create_issue" => format!("GitHub issue #{} created in {}/{}", v["number"].as_u64().unwrap_or(0), owner, repo),
                                _ => format!("GitHub comment added to {}/{}", owner, repo),
                            };
                            NodeOutput::success_with_logs(v, vec![log])
                        } else {
                            let msg = v["message"].as_str().unwrap_or("unknown error").to_string();
                            NodeOutput::failure(NodeError::unrecoverable(
                                "GITHUB_ERROR",
                                format!("HTTP {}: {}", status, msg),
                            ))
                        }
                    }
                    Err(e) => NodeOutput::failure(NodeError::unrecoverable(
                        "PARSE_ERROR",
                        format!("HTTP {}: could not parse GitHub response: {}", status, e),
                    )),
                }
            }
            Err(e) => {
                let recoverable = e.is_timeout() || e.is_connect();
                if recoverable {
                    NodeOutput::failure(NodeError::recoverable("HTTP_ERROR", e.to_string()))
                } else {
                    NodeOutput::failure(NodeError::unrecoverable("HTTP_ERROR", e.to_string()))
                }
            }
        }
    }
}
