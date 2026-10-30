//! AI Agent node (`ai_agent`).
//!
//! `node` holds the node definition and `execute`. The other modules are
//! split by concern: reply and output building (`output`), model, search and
//! tool settings with their provider checks (`settings`, `web_search`,
//! `tools`, `structured`), conversation continuation (`messages`,
//! `conversation`), and one module per provider call (`openai`, `anthropic`,
//! `gemini`) sharing the error type in `api_error`.

mod anthropic;
mod api_error;
mod conversation;
mod gemini;
mod messages;
mod node;
mod openai;
mod output;
mod settings;
mod structured;
mod tools;
mod web_search;

#[cfg(test)]
mod tests;

pub use node::AiAgentNode;
