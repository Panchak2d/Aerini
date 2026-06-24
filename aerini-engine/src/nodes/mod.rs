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

/// Register all built-in node implementations.
///
/// `db` is `None` in aerini-server's single-workflow serve mode, where no WorkflowDb
/// is available. Variable nodes still function — persist=true is a no-op with a warning.
pub fn register_builtins(
    registry: &mut NodeRegistry,
    data_dir: &std::path::Path,
    db: Option<Arc<WorkflowDb>>,
) {
    // Triggers
    registry.register(Arc::new(manual_trigger::ManualTriggerNode));
    registry.register(Arc::new(webhook::WebhookNode));
    registry.register(Arc::new(schedule::ScheduleNode));

    // Logic
    registry.register(Arc::new(if_condition::IfConditionNode));
    registry.register(Arc::new(switch::SwitchNode));
    registry.register(Arc::new(loop_node::LoopNode));
    registry.register(Arc::new(stop::StopNode));
    registry.register(Arc::new(merge::MergeNode));

    // Flow control
    registry.register(Arc::new(delay::DelayNode));
    registry.register(Arc::new(wait_node::WaitNode));

    // AI
    registry.register(Arc::new(ai_prompt::AiPromptNode));
    registry.register(Arc::new(ai_agent::AiAgentNode));
    registry.register(Arc::new(ai_memory::AiMemoryNode::new(
        data_dir.join("ai_memory.db"),
    )));
    registry.register(Arc::new(text_splitter::TextSplitterNode));
    registry.register(Arc::new(image_gen::ImageGenNode));

    // Logic (fan-in)
    registry.register(Arc::new(collect_files::CollectFilesNode));

    // Actions
    registry.register(Arc::new(http::HttpRequestNode));
    registry.register(Arc::new(shell::ShellExecNode));
    registry.register(Arc::new(code_node::CodeNode));
    registry.register(Arc::new(email::EmailNode));
    registry.register(Arc::new(file::FileNode));
    registry.register(Arc::new(notification::NotificationNode));
    registry.register(Arc::new(save_to_folder::SaveToFolderNode));
    registry.register(Arc::new(social_upload::SocialUploadNode));
    registry.register(Arc::new(database::DatabaseNode));
    registry.register(Arc::new(s3::S3Node));

    // Integrations
    registry.register(Arc::new(slack::SlackNode));
    registry.register(Arc::new(discord::DiscordNode));
    registry.register(Arc::new(github::GitHubNode));
    registry.register(Arc::new(google_sheets::GoogleSheetsNode));
    registry.register(Arc::new(notion::NotionNode));
    registry.register(Arc::new(telegram::TelegramNode));
    registry.register(Arc::new(sendgrid::SendGridNode));
    registry.register(Arc::new(stripe::StripeNode));

    // Data / utility
    registry.register(Arc::new(text_to_file::TextToFileNode));
    registry.register(Arc::new(transform::TransformNode));
    registry.register(Arc::new(json_node::JsonNode));
    registry.register(Arc::new(variables::SetVariableNode { db: db.clone() }));
    registry.register(Arc::new(variables::GetVariableNode { db }));
    registry.register(Arc::new(output_node::OutputNode));
}
