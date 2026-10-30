use serde_json::Value;

use crate::error::NodeError;
use crate::nodes::util::{cfg_bool_opt, cfg_u64_opt};

use super::web_search::{WebSearch, parse_search_domains};

/// How hard the model thinks, in the node's own words. Each provider gets the
/// closest name it accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Effort {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl Effort {
    pub(super) fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "none"    => Self::Off,
            "minimal" => Self::Minimal,
            "low"     => Self::Low,
            "medium"  => Self::Medium,
            "high"    => Self::High,
            "xhigh"   => Self::Xhigh,
            "max"     => Self::Max,
            _ => return None,
        })
    }

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Off     => "none",
            Self::Minimal => "minimal",
            Self::Low     => "low",
            Self::Medium  => "medium",
            Self::High    => "high",
            Self::Xhigh   => "xhigh",
            Self::Max     => "max",
        }
    }
}

/// The node's web search and effort settings, read once from the config.
#[derive(Clone, Debug)]
pub(super) struct ModelSettings {
    pub(super) web_search: Option<WebSearch>,
    pub(super) effort:     Option<Effort>,
}

impl ModelSettings {
    pub(super) fn from_config(input: &Value) -> Result<Self, NodeError> {
        let effort = match &input["effort"] {
            Value::Null => None,
            Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
                "" | "default" => None,
                other => Some(Effort::parse(other).ok_or_else(|| NodeError::unrecoverable(
                    "INVALID_EFFORT",
                    format!("effort must be default, none, minimal, low, medium, high, xhigh or max, not \"{other}\""),
                ))?),
            },
            _ => return Err(NodeError::unrecoverable(
                "INVALID_EFFORT",
                "effort must be default, none, minimal, low, medium, high, xhigh or max",
            )),
        };

        let web_search = if cfg_bool_opt(&input["web_search"], "web_search")?.unwrap_or(false) {
            let max_uses = cfg_u64_opt(&input["web_search_max_uses"], "web_search_max_uses")?;
            if max_uses == Some(0) {
                return Err(NodeError::unrecoverable("INVALID_WEB_SEARCH", "web_search_max_uses must be 1 or more"));
            }
            let domains = parse_search_domains(&input["web_search_domains"])
                .map_err(|e| NodeError::unrecoverable("INVALID_WEB_SEARCH", e))?;
            Some(WebSearch { max_uses, domains })
        } else {
            None
        };

        Ok(Self { web_search, effort })
    }
}

/// Rejects an effort level the provider has no name for, so it fails with a
/// reason instead of an opaque HTTP 400. Whether a particular model accepts the
/// level is left to the provider.
pub(super) fn check_effort(effort: Effort, provider_id: &str) -> Result<(), String> {
    let allowed = match provider_id {
        "anthropic" => matches!(effort, Effort::Low | Effort::Medium | Effort::High | Effort::Xhigh | Effort::Max),
        "gemini"    => matches!(effort, Effort::Minimal | Effort::Low | Effort::Medium | Effort::High),
        _ => true,
    };
    if allowed {
        return Ok(());
    }
    let (name, accepted) = if provider_id == "anthropic" {
        ("Anthropic", "low, medium, high, xhigh or max")
    } else {
        ("Gemini", "minimal, low, medium or high")
    };
    Err(format!("effort \"{}\" is not available with {name}; it takes {accepted}", effort.as_str()))
}
