pub mod util;
pub mod collect_files;
pub mod image_gen;
pub mod oauth_listener;
pub mod save_to_folder;
pub mod social_upload;
pub mod ai_agent;
pub mod ai_memory;
pub mod ai_prompt;
pub mod code_node;
pub mod database;
pub mod delay;
pub mod discord;
pub mod email;
pub mod file;
pub mod github;
pub mod google_sheets;
pub mod http;
pub mod if_condition;
pub mod json_node;
pub mod loop_node;
pub mod manual_trigger;
pub mod merge;
pub mod notion;
pub mod notification;
pub mod output_node;
pub mod schedule;
pub mod sendgrid;
pub mod shell;
pub mod slack;
pub mod s3;
pub mod stop;
pub mod stripe;
pub mod switch;
pub mod telegram;
pub mod text_splitter;
pub mod text_to_file;
pub mod transform;
pub mod variables;
pub mod wait_node;
pub mod webhook;

use std::sync::Arc;
use crate::db::WorkflowDb;
use crate::node::NodeRegistry;

static SHARED_HTTP_CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();

/// Shared HTTP client for integration nodes with 30-second timeout.
/// ai_prompt, ai_agent, image_gen, social_upload keep their own 120s clients.
/// http.rs keeps its own client (SSRF-safe: redirect=none).
pub(crate) fn shared_http_client() -> &'static reqwest::Client {
    SHARED_HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent(concat!("aerini-engine/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("shared HTTP client init failed")
    })
}

/// Node type ids capable of arbitrary command/code execution or direct
/// database access — the single canonical list consulted by every execution
/// entry point: desktop `run_workflow`, desktop `SchedulerDaemon` job-start,
/// `aerini-server` `serve_mode`/`api_mode` startup gating, and export
/// packaging. AUDIT_REPORT.md T2-1/T2-6 — previously six independently
/// written, inconsistent checks; every one of them now reads this list.
///
/// The frontend keeps its own UI-only list, `DANGEROUS_NODE_IDS` in
/// `src/node-ids.ts` — the two crates share no build step, so keep them in
/// sync by hand. `node-ids.ts` currently lists `file` instead of `database`;
/// that is a separate, already-tracked frontend gap, not fixed here.
pub const DANGEROUS_NODE_TYPE_IDS: &[&str] = &["shell_exec", "code", "database"];

/// Returns every id from [`DANGEROUS_NODE_TYPE_IDS`] present in `nodes`, in
/// the list's own canonical order (not workflow order), deduplicated. Empty
/// when none are present.
pub fn dangerous_node_types_present(nodes: &[crate::model::WorkflowNode]) -> Vec<&'static str> {
    DANGEROUS_NODE_TYPE_IDS.iter()
        .copied()
        .filter(|dangerous_id| nodes.iter().any(|n| n.node_type_id == *dangerous_id))
        .collect()
}

/// Register all built-in node implementations.
///
/// `db` is `None` in aerini-server's single-workflow serve mode, where no WorkflowDb
/// is available. Variable nodes still function — persist=true is a no-op with a warning.
pub fn register_builtins(
    registry: &mut NodeRegistry,
    data_dir: &std::path::Path,
    db: Option<Arc<WorkflowDb>>,
) {
    // ── Triggers ─────────────────────────────────────────────────────────
    registry.register(Arc::new(manual_trigger::ManualTriggerNode));
    registry.register(Arc::new(webhook::WebhookNode));
    registry.register(Arc::new(schedule::ScheduleNode));

    // ── Logic ────────────────────────────────────────────────────────────
    registry.register(Arc::new(if_condition::IfConditionNode));
    registry.register(Arc::new(switch::SwitchNode));
    registry.register(Arc::new(loop_node::LoopNode));
    registry.register(Arc::new(stop::StopNode));
    registry.register(Arc::new(merge::MergeNode));
    registry.register(Arc::new(delay::DelayNode));
    registry.register(Arc::new(wait_node::WaitNode));

    // ── AI ───────────────────────────────────────────────────────────────
    registry.register(Arc::new(ai_prompt::AiPromptNode));
    registry.register(Arc::new(ai_agent::AiAgentNode));
    registry.register(Arc::new(ai_memory::AiMemoryNode::new(
        data_dir.join("ai_memory.db"),
    )));
    registry.register(Arc::new(text_splitter::TextSplitterNode));
    registry.register(Arc::new(image_gen::ImageGenNode));

    // ── Files ────────────────────────────────────────────────────────────
    registry.register(Arc::new(collect_files::CollectFilesNode));

    // ── Utility / Files / Integrations (mixed -- category noted inline
    //    only where it diverges from this block's Utility default) ───────
    registry.register(Arc::new(http::HttpRequestNode));
    registry.register(Arc::new(shell::ShellExecNode));
    registry.register(Arc::new(code_node::CodeNode));
    registry.register(Arc::new(email::EmailNode));
    registry.register(Arc::new(file::FileNode));                    // Files
    registry.register(Arc::new(notification::NotificationNode));
    registry.register(Arc::new(save_to_folder::SaveToFolderNode));  // Files
    registry.register(Arc::new(social_upload::SocialUploadNode));   // Integrations
    registry.register(Arc::new(database::DatabaseNode));
    registry.register(Arc::new(s3::S3Node));                        // Files

    // ── Integrations ─────────────────────────────────────────────────────
    registry.register(Arc::new(slack::SlackNode));
    registry.register(Arc::new(discord::DiscordNode));
    registry.register(Arc::new(github::GitHubNode));
    registry.register(Arc::new(google_sheets::GoogleSheetsNode));
    registry.register(Arc::new(notion::NotionNode));
    registry.register(Arc::new(telegram::TelegramNode));
    registry.register(Arc::new(sendgrid::SendGridNode));
    registry.register(Arc::new(stripe::StripeNode));

    // ── Utility ──────────────────────────────────────────────────────────
    registry.register(Arc::new(text_to_file::TextToFileNode));      // Files
    registry.register(Arc::new(transform::TransformNode));
    registry.register(Arc::new(json_node::JsonNode));
    registry.register(Arc::new(variables::SetVariableNode { db: db.clone() }));
    registry.register(Arc::new(variables::GetVariableNode { db }));
    registry.register(Arc::new(output_node::OutputNode));
}

#[cfg(test)]
mod dangerous_node_tests {
    use super::*;
    use crate::model::{NodeType, WorkflowNode};
    use std::collections::HashMap;

    fn node(id: &str, node_type_id: &str) -> WorkflowNode {
        WorkflowNode {
            id:            id.to_string(),
            node_type_id:  node_type_id.to_string(),
            node_type:     NodeType::Utility,
            name:          id.to_string(),
            config:        serde_json::json!({}),
            credentials:   HashMap::new(),
            input_schema:  serde_json::json!({}),
            output_schema: serde_json::json!({}),
            retry:         Default::default(),
            fallback_node: None,
            disabled:      false,
            position:      Default::default(),
        }
    }

    #[test]
    fn empty_when_no_dangerous_nodes_present() {
        let nodes = vec![node("n1", "http_request"), node("n2", "manual_trigger")];
        assert!(dangerous_node_types_present(&nodes).is_empty());
    }

    #[test]
    fn finds_each_dangerous_type_independently() {
        assert_eq!(dangerous_node_types_present(&[node("n1", "shell_exec")]), vec!["shell_exec"]);
        assert_eq!(dangerous_node_types_present(&[node("n1", "code")]), vec!["code"]);
        assert_eq!(dangerous_node_types_present(&[node("n1", "database")]), vec!["database"]);
    }

    #[test]
    fn dedupes_and_orders_by_canonical_list_not_workflow_order() {
        // Workflow order is database, then shell_exec, then a second shell_exec —
        // result must still be [shell_exec, database] (canonical list order),
        // and the duplicate shell_exec must not produce a duplicate entry.
        let nodes = vec![
            node("n1", "database"),
            node("n2", "shell_exec"),
            node("n3", "shell_exec"),
        ];
        assert_eq!(dangerous_node_types_present(&nodes), vec!["shell_exec", "database"]);
    }

    #[test]
    fn ignores_non_dangerous_types_mixed_in() {
        let nodes = vec![node("n1", "http_request"), node("n2", "code"), node("n3", "if_condition")];
        assert_eq!(dangerous_node_types_present(&nodes), vec!["code"]);
    }
}
