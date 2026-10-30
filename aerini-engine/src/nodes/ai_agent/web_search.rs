use serde_json::Value;

use crate::nodes::ai_prompt::is_official_openai;

use super::tools::{ToolChoice, ToolControls};

/// What the provider's web search reported: the queries it ran, the pages the
/// reply drew on (no page twice), any search failures it flagged, and the
/// Search Suggestions HTML Google asks to be shown with a grounded answer.
#[derive(Default)]
pub(super) struct Grounding {
    pub(super) queries:     Vec<String>,
    pub(super) sources:     Vec<(String, String)>,
    pub(super) warnings:    Vec<String>,
    pub(super) suggestions: String,
}

impl Grounding {
    pub(super) fn add_query(&mut self, query: &str) {
        let query = query.trim();
        if !query.is_empty() {
            self.queries.push(query.to_string());
        }
    }

    pub(super) fn add_source(&mut self, title: &str, url: &str) {
        let url = url.trim();
        if !url.is_empty() && !self.sources.iter().any(|(_, u)| u == url) {
            self.sources.push((title.trim().to_string(), url.to_string()));
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct WebSearch {
    pub(super) max_uses: Option<u64>,
    pub(super) domains:  Vec<String>,
}

/// The Web Search Domains field as a list: names split on commas, semicolons
/// and white space, lower-cased, without repeats. A scheme, a wildcard outside
/// a trailing `/*`, or a non-ASCII name is refused, because Anthropic rejects
/// those with an opaque HTTP 400, and a look-alike Unicode name could allow a
/// site the author did not mean.
pub(super) fn parse_search_domains(raw: &Value) -> Result<Vec<String>, String> {
    let text = match raw {
        Value::Null => return Ok(Vec::new()),
        Value::String(s) => s,
        _ => return Err("web_search_domains must be text: site names separated by commas or new lines".to_string()),
    };
    let mut out: Vec<String> = Vec::new();
    for entry in text.split(|c: char| c == ',' || c == ';' || c.is_whitespace()).filter(|e| !e.is_empty()) {
        let domain = entry.to_ascii_lowercase();
        if domain.contains("://") {
            return Err(format!("web_search_domains: write \"{entry}\" without http:// or https://"));
        }
        if !domain.is_ascii() {
            return Err(format!("web_search_domains: \"{entry}\" must use ASCII characters only"));
        }
        if domain.trim_end_matches("/*").contains('*') {
            return Err(format!("web_search_domains: \"{entry}\" may use * only at the end of a path, as in example.com/docs/*"));
        }
        if !out.contains(&domain) {
            out.push(domain);
        }
    }
    Ok(out)
}

/// Where a request is going, for the checks that depend on it.
pub(super) struct SearchSite<'a> {
    pub(super) provider_id: &'a str,
    pub(super) base_url:    &'a str,
    pub(super) model:       &'a str,
}

/// Rejects a web search setup the provider cannot honor, or is not known to
/// accept, so it fails with a reason instead of quietly not searching or
/// surfacing as an opaque HTTP 400.
pub(super) fn check_web_search(
    search: &WebSearch,
    site: SearchSite<'_>,
    tools: &[Value],
    controls: ToolControls,
    has_schema: bool,
) -> Result<(), String> {
    match site.provider_id {
        "anthropic" => {
            if tools.iter().any(|t| t["name"].as_str() == Some(WEB_SEARCH_TOOL_NAME)) {
                return Err(format!("a tool named \"{WEB_SEARCH_TOOL_NAME}\" clashes with Web Search; rename that tool"));
            }
            if has_schema {
                return Err("Web Search cannot be combined with Response Schema for Anthropic: web search replies carry citations, which Anthropic refuses together with a JSON schema. Search in one AI Agent node and pass its result to a second one that has the schema".to_string());
            }
            if controls.choice == ToolChoice::NoTools && !tools.is_empty() {
                return Err("tool_choice \"none\" would also switch off Web Search with Anthropic; remove Tools or use auto".to_string());
            }
        }
        "gemini" => {
            if !tools.is_empty() {
                return Err("Web Search cannot be combined with Tools for Gemini here; use one or the other".to_string());
            }
            if search.max_uses.is_some() || !search.domains.is_empty() {
                return Err("web_search_max_uses and web_search_domains are available only with Anthropic".to_string());
            }
        }
        _ if is_official_openai(site.base_url) => {
            if !site.model.to_ascii_lowercase().contains("search") {
                return Err(format!(
                    "OpenAI searches the web only with a search model such as {OPENAI_SEARCH_MODEL}; \"{}\" is not one. Change Model, or leave it blank",
                    site.model
                ));
            }
            if search.max_uses.is_some() || !search.domains.is_empty() {
                return Err("web_search_max_uses and web_search_domains are available only with Anthropic".to_string());
            }
        }
        _ => {
            return Err("Web Search is available with OpenAI (api.openai.com), Anthropic and Gemini, not with local or other OpenAI-compatible servers".to_string());
        }
    }
    Ok(())
}

pub(super) const WEB_SEARCH_TOOL_NAME: &str = "web_search";

pub(super) const OPENAI_SEARCH_MODEL: &str = "gpt-5-search-api";

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    #[test]
    fn openai_web_search_needs_a_search_model_and_takes_no_anthropic_only_settings() {
        let search = WebSearch { max_uses: None, domains: Vec::new() };
        let site = |model| SearchSite { provider_id: "openai", base_url: "https://api.openai.com/v1", model };
        let err = check_web_search(&search, site("gpt-5.6"), &[], ToolControls::default(), false).expect_err("not a search model");
        assert!(err.contains(OPENAI_SEARCH_MODEL), "{err}");
        assert!(check_web_search(&search, site(OPENAI_SEARCH_MODEL), &[], ToolControls::default(), false).is_ok());
        let limited = WebSearch { max_uses: Some(2), domains: Vec::new() };
        assert!(check_web_search(&limited, site(OPENAI_SEARCH_MODEL), &[], ToolControls::default(), false).is_err());
    }

    #[test]
    fn search_domains_are_split_lower_cased_and_deduplicated_and_unsafe_ones_refused() {
        let got = parse_search_domains(&json!("A.example, docs.b.example/guide\na.example;c.example/blog/*")).expect("domains");
        assert_eq!(got, vec!["a.example", "docs.b.example/guide", "c.example/blog/*"]);
        assert!(parse_search_domains(&json!("")).expect("blank").is_empty());
        for bad in [json!("https://x.com"), json!("*.x.com"), json!("x.com/*/y"), json!("bücher.de"), json!(5)] {
            assert!(parse_search_domains(&bad).is_err(), "{bad}");
        }
    }
}
