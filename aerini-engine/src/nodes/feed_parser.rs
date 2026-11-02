use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use feed_rs::model::{Entry, Feed, FeedType, Link, Text};
use feed_rs::parser;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::Arc;

use crate::db::WorkflowDb;
use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::Node;
use super::util::{cfg_bool_opt, cfg_u64_opt, scrub_url_in_error};

/// Largest feed accepted, the same ceiling the HTTP Request node puts on a response body.
const MAX_INPUT_BYTES: usize = 10 * 1024 * 1024;
const DEFAULT_MAX_ITEMS: usize = 20;
const MAX_ITEMS_CAP: usize = 500;
const DEFAULT_MAX_TEXT_CHARS: usize = 2000;
/// Seen-item keys kept per node for `only_new`; the oldest are dropped first.
const SEEN_CAP: usize = 1000;

const SORTS: [&str; 2] = ["newest", "feed"];
const FIRST_RUNS: [&str; 2] = ["baseline", "emit"];
const DEDUP_MODES: [&str; 3] = ["id", "link", "title"];

/// Feed Parser node: turns the text of an RSS, Atom or JSON Feed document into
/// normalized items, with filtering, sorting and optional persistent de-duplication.
pub struct FeedParserNode {
    /// None in serve mode without a database, where `only_new` cannot keep its state.
    pub db: Option<Arc<WorkflowDb>>,
}

#[derive(Clone, Copy, PartialEq)]
enum DedupBy {
    Id,
    Link,
    Title,
}

#[derive(Clone)]
struct Opts {
    base_url: Option<String>,
    max_items: usize,
    newest_first: bool,
    include: Vec<String>,
    exclude: Vec<String>,
    since: Option<DateTime<Utc>>,
    only_new: bool,
    baseline_first_run: bool,
    dedup_by: DedupBy,
    max_text_chars: usize,
}

struct Item {
    id: String,
    link: String,
    title: String,
    date: Option<DateTime<Utc>>,
    search: String,
    value: Value,
}

struct Selection {
    items: Vec<Item>,
    new_count: usize,
    seen_after: Option<Vec<String>>,
}

struct Pipeline {
    feed: Value,
    total: usize,
    selection: Selection,
}

// ── Config ───────────────────────────────────────────────────────────────────

fn invalid(field: &str, expected: &str) -> NodeError {
    NodeError::unrecoverable("INVALID_CONFIG", format!("{field} must be {expected}"))
}

fn cfg_choice(cfg: &Value, field: &str, allowed: &[&'static str]) -> Result<&'static str, NodeError> {
    match &cfg[field] {
        Value::Null => Ok(allowed[0]),
        Value::String(s) if s.trim().is_empty() => Ok(allowed[0]),
        Value::String(s) => allowed
            .iter()
            .copied()
            .find(|a| a.eq_ignore_ascii_case(s.trim()))
            .ok_or_else(|| invalid(field, &format!("one of: {}", allowed.join(", ")))),
        _ => Err(invalid(field, &format!("one of: {}", allowed.join(", ")))),
    }
}

fn cfg_text(cfg: &Value, field: &str) -> Result<Option<String>, NodeError> {
    match &cfg[field] {
        Value::Null => Ok(None),
        Value::String(s) => {
            let t = s.trim();
            Ok(if t.is_empty() { None } else { Some(t.to_string()) })
        }
        _ => Err(invalid(field, "text")),
    }
}

fn cfg_count(cfg: &Value, field: &str, default: usize, max: usize) -> Result<usize, NodeError> {
    match cfg_u64_opt(&cfg[field], field)? {
        None => Ok(default),
        Some(n) if (1..=max as u64).contains(&n) => Ok(n as usize),
        Some(_) => Err(invalid(field, &format!("a whole number from 1 to {max}"))),
    }
}

fn cfg_keywords(cfg: &Value, field: &str) -> Result<Vec<String>, NodeError> {
    Ok(cfg_text(cfg, field)?
        .map(|s| {
            s.split(',')
                .map(|k| k.trim().to_lowercase())
                .filter(|k| !k.is_empty())
                .collect::<Vec<String>>()
        })
        .unwrap_or_default())
}

fn parse_since(raw: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Some(dt.with_timezone(&Utc));
    }
    NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .ok()?
        .and_hms_opt(0, 0, 0)
        .map(|d| d.and_utc())
}

fn parse_opts(cfg: &Value) -> Result<Opts, NodeError> {
    let base_url = cfg_text(cfg, "base_url")?;
    if let Some(u) = &base_url {
        if url::Url::parse(u).is_err() {
            return Err(invalid("base_url", "an absolute URL such as https://example.com/"));
        }
    }
    let since = match cfg_text(cfg, "since")? {
        None => None,
        Some(raw) => Some(parse_since(&raw).ok_or_else(|| {
            invalid("since", "an ISO-8601 date or timestamp, such as 2026-10-31 or 2026-10-31T08:00:00Z")
        })?),
    };
    let dedup_by = match cfg_choice(cfg, "dedup_by", &DEDUP_MODES)? {
        "link" => DedupBy::Link,
        "title" => DedupBy::Title,
        _ => DedupBy::Id,
    };
    Ok(Opts {
        base_url,
        max_items: cfg_count(cfg, "max_items", DEFAULT_MAX_ITEMS, MAX_ITEMS_CAP)?,
        newest_first: cfg_choice(cfg, "sort", &SORTS)? == "newest",
        include: cfg_keywords(cfg, "include_keywords")?,
        exclude: cfg_keywords(cfg, "exclude_keywords")?,
        since,
        only_new: cfg_bool_opt(&cfg["only_new"], "only_new")?.unwrap_or(false),
        baseline_first_run: cfg_choice(cfg, "on_first_run", &FIRST_RUNS)? == "baseline",
        dedup_by,
        max_text_chars: cfg_count(cfg, "max_text_chars", DEFAULT_MAX_TEXT_CHARS, MAX_INPUT_BYTES)?,
    })
}

// ── Input text ───────────────────────────────────────────────────────────────

fn feed_text(value: &Value) -> Result<String, NodeError> {
    let text = match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    if text.len() > MAX_INPUT_BYTES {
        return Err(NodeError::unrecoverable(
            "INPUT_TOO_LARGE",
            format!("The feed is larger than the {} MiB limit.", MAX_INPUT_BYTES / (1024 * 1024)),
        ));
    }
    if text.trim().is_empty() {
        return Err(NodeError::unrecoverable(
            "MISSING_INPUT",
            "text is required. Set it to the feed content, for example the body of an HTTP Request node.",
        ));
    }
    Ok(text)
}

/// The text is already UTF-8 by the time it reaches this node, so a prolog that
/// declares another encoding would make the parser decode it a second time.
fn normalize_prolog(text: &str) -> String {
    let rest = text.trim_start_matches('\u{feff}').trim_start();
    if !rest.starts_with("<?xml") {
        return rest.to_string();
    }
    let Some(end) = rest.find("?>") else {
        return rest.to_string();
    };
    let decl = &rest[..end];
    let Some(pos) = decl.find("encoding") else {
        return rest.to_string();
    };
    let after_keyword = pos + "encoding".len();
    let tail = &decl[after_keyword..];
    let Some(quote_offset) = tail.find(['"', '\'']) else {
        return rest.to_string();
    };
    let quote = tail.as_bytes()[quote_offset] as char;
    let value_start = after_keyword + quote_offset + 1;
    let Some(value_len) = decl[value_start..].find(quote) else {
        return rest.to_string();
    };
    let mut out = String::with_capacity(rest.len());
    out.push_str(&rest[..value_start]);
    out.push_str("UTF-8");
    out.push_str(&rest[value_start + value_len..]);
    out
}

// ── Plain text ───────────────────────────────────────────────────────────────

fn is_block_tag(name: &str) -> bool {
    matches!(
        name,
        "p" | "br" | "div" | "li" | "ul" | "ol" | "tr" | "td" | "th" | "table" | "blockquote"
            | "pre" | "section" | "article" | "header" | "footer" | "hr"
            | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
    )
}

fn find_ignore_case(haystack: &str, needle: &str) -> Option<usize> {
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || h.len() < n.len() {
        return None;
    }
    (0..=h.len() - n.len()).find(|&i| h[i..i + n.len()].eq_ignore_ascii_case(n))
}

fn decode_entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '),
        "hellip" => Some('…'),
        "mdash" => Some('—'),
        "ndash" => Some('–'),
        "lsquo" => Some('‘'),
        "rsquo" => Some('’'),
        "ldquo" => Some('“'),
        "rdquo" => Some('”'),
        _ => {
            let number = name.strip_prefix('#')?;
            let code = match number.strip_prefix('x').or_else(|| number.strip_prefix('X')) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => number.parse::<u32>().ok()?,
            };
            char::from_u32(code).filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        }
    }
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let decoded = tail[1..]
            .find(';')
            .filter(|&j| j <= 10)
            .and_then(|j| decode_entity(&tail[1..1 + j]).map(|c| (c, j)));
        match decoded {
            Some((c, j)) => {
                out.push(c);
                rest = &tail[j + 2..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank_lines = 0;
    for line in s.split('\n') {
        let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if collapsed.is_empty() {
            blank_lines += 1;
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
            if blank_lines > 0 {
                out.push('\n');
            }
        }
        out.push_str(&collapsed);
        blank_lines = 0;
    }
    out
}

/// Plain text of an HTML fragment: tags removed, `script` and `style` bodies and
/// comments dropped, block elements turned into line breaks, entities decoded.
fn strip_html(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut seg = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        let starts_tag = bytes
            .get(i + 1)
            .is_some_and(|&c| c.is_ascii_alphabetic() || c == b'/' || c == b'!' || c == b'?');
        if !starts_tag {
            i += 1;
            continue;
        }
        out.push_str(&input[seg..i]);
        if input[i..].starts_with("<!--") {
            i = match input[i + 4..].find("-->") {
                Some(j) => i + 4 + j + 3,
                None => bytes.len(),
            };
            seg = i;
            continue;
        }
        let end = match input[i..].find('>') {
            Some(j) => i + j + 1,
            None => bytes.len(),
        };
        let tag = &input[i + 1..end];
        let closing = tag.starts_with('/');
        let name = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_lowercase())
            .collect::<String>();
        i = end;
        if !closing && (name == "script" || name == "style") {
            let close = format!("</{name}");
            i = match find_ignore_case(&input[end..], &close) {
                Some(j) => {
                    let from = end + j;
                    match input[from..].find('>') {
                        Some(k) => from + k + 1,
                        None => bytes.len(),
                    }
                }
                None => bytes.len(),
            };
        } else if is_block_tag(&name) {
            out.push('\n');
        }
        seg = i;
    }
    if seg < bytes.len() {
        out.push_str(&input[seg..]);
    }
    collapse_whitespace(&decode_entities(&out))
}

fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((idx, _)) => format!("{}…", s[..idx].trim_end()),
        None => s.to_string(),
    }
}

fn is_html_type(content_type: &str) -> bool {
    content_type.starts_with("text/html") || content_type.starts_with("application/xhtml")
}

/// Plain text, and the raw markup when the source declared it as HTML.
fn split_text(raw: &str, content_type: &str) -> (String, Option<String>) {
    if is_html_type(content_type) {
        let html = if raw.trim().is_empty() { None } else { Some(raw.to_string()) };
        (strip_html(raw), html)
    } else {
        (raw.trim().to_string(), None)
    }
}

fn text_parts(text: Option<&Text>) -> (String, Option<String>) {
    match text {
        Some(t) => split_text(&t.content, t.content_type.as_str()),
        None => (String::new(), None),
    }
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn stable_hash(parts: &[&str]) -> String {
    let mut hasher = blake3::Hasher::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update(b"\0");
    }
    hasher.finalize().to_string()[..32].to_string()
}

// ── Feed to items ────────────────────────────────────────────────────────────

fn fmt_date(date: Option<DateTime<Utc>>) -> Option<String> {
    date.map(|d| d.to_rfc3339_opts(SecondsFormat::Secs, true))
}

fn is_enclosure(link: &Link, json_feed: bool) -> bool {
    link.rel.as_deref() == Some("enclosure")
        || (json_feed && link.rel.is_none() && link.media_type.is_some())
}

fn pick_link(links: &[Link], json_feed: bool) -> String {
    let is_main = |l: &Link| {
        l.target.is_none()
            && matches!(l.rel.as_deref(), None | Some("alternate"))
            && !is_enclosure(l, json_feed)
    };
    links
        .iter()
        .find(|l| is_main(l))
        .or_else(|| {
            links.iter().find(|l| {
                l.target.is_none()
                    && !is_enclosure(l, json_feed)
                    && l.rel.as_deref() != Some("self")
            })
        })
        .or_else(|| links.first())
        .map(|l| l.href.clone())
        .unwrap_or_default()
}

fn enclosures(entry: &Entry, json_feed: bool) -> Vec<Value> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for media in &entry.media {
        for content in &media.content {
            if let Some(url) = &content.url {
                if seen.insert(url.as_str().to_string()) {
                    out.push(json!({
                        "url":  url.as_str(),
                        "type": content.content_type.as_ref().map(|t| t.as_str().to_string()),
                        "size": content.size,
                    }));
                }
            }
        }
    }
    for link in entry.links.iter().filter(|l| is_enclosure(l, json_feed)) {
        if seen.insert(link.href.clone()) {
            out.push(json!({
                "url":  link.href,
                "type": link.media_type,
                "size": link.length,
            }));
        }
    }
    out
}

fn pick_image(entry: &Entry, json_feed: bool) -> Option<String> {
    for media in &entry.media {
        if let Some(thumb) = media.thumbnails.first() {
            if !thumb.image.uri.is_empty() {
                return Some(thumb.image.uri.clone());
            }
        }
    }
    for media in &entry.media {
        for content in &media.content {
            let is_image = content
                .content_type
                .as_ref()
                .is_some_and(|t| t.as_str().starts_with("image/"));
            if is_image {
                if let Some(url) = &content.url {
                    return Some(url.as_str().to_string());
                }
            }
        }
    }
    entry
        .links
        .iter()
        .find(|l| {
            is_enclosure(l, json_feed)
                && l.media_type.as_deref().is_some_and(|t| t.starts_with("image/"))
        })
        .map(|l| l.href.clone())
}

fn to_item(entry: &Entry, json_feed: bool, max_chars: usize) -> Item {
    let title = one_line(&text_parts(entry.title.as_ref()).0);
    let (summary_plain, summary_html) = text_parts(entry.summary.as_ref());
    let (content_plain, content_html) = match entry
        .content
        .as_ref()
        .and_then(|c| c.body.as_deref().map(|body| (body, c.content_type.as_str())))
    {
        Some((body, content_type)) => split_text(body, content_type),
        None => (String::new(), None),
    };

    let published = fmt_date(entry.published);
    let date = entry.published.or(entry.updated);
    let link = pick_link(&entry.links, json_feed);
    let id = if entry.id.trim().is_empty() {
        stable_hash(&[
            "entry",
            published.as_deref().unwrap_or(""),
            &truncate_chars(&summary_plain, 200),
        ])
    } else {
        entry.id.trim().to_string()
    };

    let authors: Vec<String> = entry
        .authors
        .iter()
        .filter_map(|p| p.name.as_deref())
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .collect();
    let categories: Vec<String> = entry
        .categories
        .iter()
        .map(|c| {
            c.label
                .as_deref()
                .filter(|l| !l.is_empty())
                .unwrap_or(c.term.as_str())
                .trim()
                .to_string()
        })
        .filter(|c| !c.is_empty())
        .collect();

    let search = format!("{} {}", title, summary_plain).to_lowercase();
    let value = json!({
        "id":           id,
        "title":        title,
        "link":         link,
        "published":    published,
        "updated":      fmt_date(entry.updated),
        "author":       authors.first(),
        "authors":      authors,
        "categories":   categories,
        "summary":      truncate_chars(&summary_plain, max_chars),
        "content":      truncate_chars(&content_plain, max_chars),
        "summary_html": summary_html,
        "content_html": content_html,
        "image":        pick_image(entry, json_feed),
        "enclosures":   enclosures(entry, json_feed),
    });
    Item { id, link, title, date, search, value }
}

fn feed_type_label(feed_type: &FeedType) -> &'static str {
    match feed_type {
        FeedType::Atom => "atom",
        FeedType::JSON => "json",
        FeedType::RSS0 => "rss0",
        FeedType::RSS1 => "rss1",
        FeedType::RSS2 => "rss2",
    }
}

fn feed_meta(feed: &Feed, json_feed: bool) -> Value {
    json!({
        "title":       one_line(&text_parts(feed.title.as_ref()).0),
        "link":        pick_link(&feed.links, json_feed),
        "description": text_parts(feed.description.as_ref()).0,
        "language":    feed.language,
        "updated":     fmt_date(feed.updated),
        "type":        feed_type_label(&feed.feed_type),
    })
}

fn parse_feed(text: &str, base_url: Option<&str>) -> Result<Feed, NodeError> {
    let normalized = normalize_prolog(text);
    let feed_parser = parser::Builder::new()
        .base_uri(base_url)
        .id_generator(|links: &[Link], title: &Option<Text>, _uri: Option<&str>| {
            let link = links.first().map_or("", |l| l.href.as_str());
            let title = title.as_ref().map_or("", |t| t.content.as_str());
            if link.is_empty() && title.is_empty() {
                String::new()
            } else {
                stable_hash(&[link, title])
            }
        })
        .build();
    feed_parser.parse(normalized.as_bytes()).map_err(|e| {
        let detail: String = scrub_url_in_error(&e.to_string()).chars().take(200).collect();
        NodeError::unrecoverable(
            "NOT_A_FEED",
            format!(
                "The text is not an RSS, Atom or JSON Feed document ({detail}). \
                 Check that it is the feed itself and not an HTML page or an error response."
            ),
        )
    })
}

// ── Filter, sort, de-duplicate ───────────────────────────────────────────────

fn keep(item: &Item, opts: &Opts) -> bool {
    if let (Some(since), Some(date)) = (opts.since, item.date) {
        if date < since {
            return false;
        }
    }
    if !opts.include.is_empty() && !opts.include.iter().any(|k| item.search.contains(k.as_str())) {
        return false;
    }
    !opts.exclude.iter().any(|k| item.search.contains(k.as_str()))
}

fn newest_first(a: Option<DateTime<Utc>>, b: Option<DateTime<Utc>>) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (Some(x), Some(y)) => y.cmp(&x),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn dedup_key(item: &Item, by: DedupBy) -> String {
    let raw = match by {
        DedupBy::Id => item.id.as_str(),
        DedupBy::Link if !item.link.is_empty() => item.link.as_str(),
        DedupBy::Title if !item.title.is_empty() => item.title.as_str(),
        _ => item.id.as_str(),
    };
    stable_hash(&[raw])
}

/// `seen` is the stored key list, `None` before the first run of this node.
fn select(mut items: Vec<Item>, opts: &Opts, seen: Option<Vec<String>>) -> Selection {
    items.retain(|item| keep(item, opts));
    if opts.newest_first {
        items.sort_by(|a, b| newest_first(a.date, b.date));
    }
    if !opts.only_new {
        items.truncate(opts.max_items);
        return Selection { items, new_count: 0, seen_after: None };
    }

    let first_run = seen.is_none();
    let mut known = seen.unwrap_or_default();
    let known_set: HashSet<String> = known.iter().cloned().collect();
    let mut in_run: HashSet<String> = HashSet::new();
    let mut fresh: Vec<(String, Item)> = Vec::new();
    for item in items {
        let key = dedup_key(&item, opts.dedup_by);
        if !known_set.contains(&key) && in_run.insert(key.clone()) {
            fresh.push((key, item));
        }
    }

    let new_count = fresh.len();
    let baseline = first_run && opts.baseline_first_run;
    let emit_count = if baseline { 0 } else { new_count.min(opts.max_items) };
    let marked = if baseline { new_count } else { emit_count };

    for (key, _) in fresh[..marked].iter().rev() {
        known.push(key.clone());
    }
    if known.len() > SEEN_CAP {
        let excess = known.len() - SEEN_CAP;
        known.drain(..excess);
    }
    fresh.truncate(emit_count);
    Selection {
        items: fresh.into_iter().map(|(_, item)| item).collect(),
        new_count,
        seen_after: Some(known),
    }
}

fn digest(items: &[Item]) -> String {
    items
        .iter()
        .map(|item| {
            let label = if !item.title.is_empty() {
                item.title.as_str()
            } else if !item.link.is_empty() {
                item.link.as_str()
            } else {
                "(untitled)"
            };
            if item.link.is_empty() {
                format!("- {label}")
            } else {
                format!(
                    "- [{}]({})",
                    label.replace('[', "\\[").replace(']', "\\]"),
                    item.link.replace(' ', "%20").replace('(', "%28").replace(')', "%29")
                )
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn run_pipeline(text: &str, opts: &Opts, seen: Option<Vec<String>>) -> Result<Pipeline, NodeError> {
    let feed = parse_feed(text, opts.base_url.as_deref())?;
    let json_feed = feed.feed_type == FeedType::JSON;
    let items: Vec<Item> = feed
        .entries
        .iter()
        .map(|entry| to_item(entry, json_feed, opts.max_text_chars))
        .collect();
    let total = items.len();
    Ok(Pipeline {
        feed: feed_meta(&feed, json_feed),
        total,
        selection: select(items, opts, seen),
    })
}

// ── Seen-item state ──────────────────────────────────────────────────────────

fn state_name(node_id: &str) -> String {
    format!("__feed_seen__{node_id}")
}

fn state_error(message: String) -> NodeError {
    NodeError::unrecoverable("STATE_ERROR", message)
}

async fn load_seen(
    db: &Arc<WorkflowDb>,
    workflow_id: &str,
    node_id: &str,
) -> Result<Option<Vec<String>>, NodeError> {
    let db = Arc::clone(db);
    let workflow_id = workflow_id.to_string();
    let name = state_name(node_id);
    match tokio::task::spawn_blocking(move || db.get_variable(&workflow_id, &name)).await {
        Ok(Ok(Some(Value::Array(keys)))) => Ok(Some(
            keys.into_iter()
                .filter_map(|v| match v {
                    Value::String(s) => Some(s),
                    _ => None,
                })
                .collect(),
        )),
        Ok(Ok(_)) => Ok(None),
        Ok(Err(e)) => Err(state_error(format!("Could not read the list of seen items: {e}"))),
        Err(e) => Err(state_error(format!("The task reading the list of seen items failed: {e}"))),
    }
}

async fn store_seen(
    db: &Arc<WorkflowDb>,
    workflow_id: &str,
    node_id: &str,
    keys: Vec<String>,
) -> Result<(), NodeError> {
    let db = Arc::clone(db);
    let workflow_id = workflow_id.to_string();
    let name = state_name(node_id);
    let value = Value::from(keys);
    match tokio::task::spawn_blocking(move || db.set_variable(&workflow_id, &name, &value)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(state_error(format!("Could not save the list of seen items: {e}"))),
        Err(e) => Err(state_error(format!("The task saving the list of seen items failed: {e}"))),
    }
}

// ── Node ─────────────────────────────────────────────────────────────────────

#[async_trait]
impl Node for FeedParserNode {
    fn type_id(&self) -> &'static str { "feed_parser" }
    fn display_name(&self) -> &'static str { "Feed Parser" }
    fn node_type(&self) -> NodeType { NodeType::Utility }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str {
        "Read an RSS, Atom or JSON Feed into normalized items, with filters, sorting, a ready-to-use digest and an option to return only items not seen in earlier runs."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["text"],
            "properties": {
                "text": {
                    "x-aerini-multiline": true,
                    "description": "The feed content as text: an RSS, Atom or JSON Feed document, normally the body of an HTTP Request node. Required."
                },
                "base_url": {
                    "type": "string",
                    "description": "Address of the feed, used to turn relative links into absolute ones. Optional."
                },
                "max_items": {
                    "type": "number",
                    "default": DEFAULT_MAX_ITEMS,
                    "description": "Most items to return, 1 to 500."
                },
                "sort": {
                    "type": "string",
                    "enum": SORTS,
                    "default": "newest",
                    "description": "newest puts the most recent items first and items without a date last; feed keeps the order of the document."
                },
                "include_keywords": {
                    "type": "string",
                    "description": "Comma-separated words. Keep only items whose title or summary contains at least one of them, ignoring case."
                },
                "exclude_keywords": {
                    "type": "string",
                    "description": "Comma-separated words. Drop items whose title or summary contains any of them, ignoring case."
                },
                "since": {
                    "type": "string",
                    "description": "Drop items dated before this ISO-8601 date or timestamp, such as 2026-10-31 or 2026-10-31T08:00:00Z. Items without a date are kept."
                },
                "only_new": {
                    "type": "boolean",
                    "default": false,
                    "description": "Return only items not seen in earlier runs of this node. Needs the app's database; fails in a server without one. Items are marked as seen when the node runs, so an item is delivered at most once even if a later node fails."
                },
                "on_first_run": {
                    "type": "string",
                    "enum": FIRST_RUNS,
                    "default": "baseline",
                    "description": "With only_new, what the first run does. baseline marks every current item as seen and returns none; emit returns them."
                },
                "dedup_by": {
                    "type": "string",
                    "enum": DEDUP_MODES,
                    "default": "id",
                    "description": "With only_new, what identifies an item. Use link or title for feeds whose ids change."
                },
                "max_text_chars": {
                    "type": "number",
                    "default": DEFAULT_MAX_TEXT_CHARS,
                    "description": "Longest plain-text summary and content, in characters. Longer text is cut and ends with an ellipsis."
                }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "feed":      { "type": "object", "description": "title, link, description, language, updated and type (rss2, rss1, rss0, atom or json)" },
                "items":     { "type": "array",  "description": "The returned items; each has id, title, link, published, updated, author, authors, categories, summary, content, summary_html, content_html, image and enclosures" },
                "latest":    { "type": "object", "description": "The first returned item, or null when there are none" },
                "count":     { "type": "number", "description": "Number of items returned" },
                "total":     { "type": "number", "description": "Number of items in the feed before filtering" },
                "new_count": { "type": "number", "description": "Only with only_new: items not seen before, ahead of the max_items limit" },
                "digest":    { "type": "string", "description": "Markdown bullet list of the returned items, one line each", "x-aerini-primary-output": true }
            }
        })
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let opts = match parse_opts(&input.input) {
            Ok(o) => o,
            Err(e) => return NodeOutput::failure(e),
        };
        let text = match feed_text(&input.input["text"]) {
            Ok(t) => t,
            Err(e) => return NodeOutput::failure(e),
        };

        let state_db = if opts.only_new {
            match &self.db {
                Some(db) => Some(Arc::clone(db)),
                None => {
                    return NodeOutput::failure(NodeError::unrecoverable(
                        "PERSIST_UNAVAILABLE",
                        "only_new needs the workflow database to remember seen items, and none is available here. \
                         Turn only_new off, or run the workflow where a database exists.",
                    ))
                }
            }
        } else {
            None
        };
        let mut seen: Option<Vec<String>> = None;
        if let Some(db) = &state_db {
            match load_seen(db, &input.workflow_id, &input.node_id).await {
                Ok(keys) => seen = keys,
                Err(e) => return NodeOutput::failure(e),
            }
        }

        let only_new = opts.only_new;
        let joined = tokio::task::spawn_blocking(move || run_pipeline(&text, &opts, seen)).await;
        let pipeline = match joined {
            Ok(Ok(p)) => p,
            Ok(Err(e)) => return NodeOutput::failure(e),
            Err(_) => {
                return NodeOutput::failure(NodeError::unrecoverable(
                    "NOT_A_FEED",
                    "The feed parser stopped unexpectedly on this input.",
                ))
            }
        };
        let Pipeline { feed, total, selection } = pipeline;
        let Selection { items, new_count, seen_after } = selection;

        if let (Some(db), Some(keys)) = (&state_db, seen_after) {
            if let Err(e) = store_seen(db, &input.workflow_id, &input.node_id, keys).await {
                return NodeOutput::failure(e);
            }
        }

        let count = items.len();
        let digest_text = digest(&items);
        let values: Vec<Value> = items.into_iter().map(|item| item.value).collect();
        let latest = values.first().cloned().unwrap_or(Value::Null);
        let kind = feed.get("type").and_then(Value::as_str).unwrap_or("feed").to_string();
        let mut output = json!({
            "feed":   feed,
            "items":  values,
            "latest": latest,
            "count":  count,
            "total":  total,
            "digest": digest_text,
        });
        let mut logs = vec![format!("Parsed {kind} feed: {total} items found, {count} returned")];
        if only_new {
            output["new_count"] = json!(new_count);
            logs.push(format!("{new_count} items not seen before"));
            if count == 0 && new_count > 0 {
                logs.push(
                    "First run: current items were marked as seen and none were returned. \
                     Set on_first_run to emit to return them."
                        .to_string(),
                );
            }
        }
        NodeOutput::success_with_logs(output, logs)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExecutionContext;
    use std::collections::HashMap;

    const RSS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Example Feed</title>
    <link>https://example.com/</link>
    <description>Latest posts</description>
    <language>en</language>
    <item>
      <title>Rust news</title>
      <link>https://example.com/rust</link>
      <description>&lt;p&gt;Hello &amp;amp; welcome&lt;/p&gt;&lt;script&gt;alert(1)&lt;/script&gt;</description>
      <pubDate>Wed, 28 Oct 2026 09:00:00 GMT</pubDate>
      <guid>guid-rust</guid>
      <category>dev</category>
      <enclosure url="https://example.com/a.mp3" length="1234" type="audio/mpeg"/>
    </item>
    <item>
      <title>Undated sponsor post</title>
      <link>https://example.com/sponsor</link>
      <description>Buy things</description>
      <guid>guid-sponsor</guid>
    </item>
    <item>
      <title>Cooking tips</title>
      <link>https://example.com/cooking</link>
      <description>Boil water</description>
      <pubDate>Fri, 30 Oct 2026 10:00:00 GMT</pubDate>
      <guid>guid-cooking</guid>
    </item>
    <item>
      <title>Python release</title>
      <link>https://example.com/python</link>
      <description>New version</description>
      <pubDate>Thu, 29 Oct 2026 08:00:00 GMT</pubDate>
      <guid>guid-python</guid>
    </item>
  </channel>
</rss>"#;

    const ATOM: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Atom Example</title>
  <id>urn:feed:1</id>
  <updated>2026-10-30T10:00:00Z</updated>
  <link rel="alternate" href="https://example.org/"/>
  <entry>
    <title>Atom entry</title>
    <id>urn:entry:1</id>
    <updated>2026-10-30T10:00:00Z</updated>
    <published>2026-10-29T09:00:00Z</published>
    <link rel="alternate" type="text/html" href="https://example.org/entry-1"/>
    <link rel="enclosure" type="audio/ogg" href="https://example.org/a.ogg" length="99"/>
    <author><name>Jane Doe</name></author>
    <category term="news" label="News"/>
    <summary>Short summary</summary>
    <content type="html">&lt;p&gt;Full &lt;b&gt;text&lt;/b&gt;&lt;/p&gt;</content>
  </entry>
</feed>"#;

    const RSS1: &str = r#"<?xml version="1.0"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns="http://purl.org/rss/1.0/">
  <channel rdf:about="https://example.net/">
    <title>RDF Example</title>
    <link>https://example.net/</link>
    <description>RSS 1.0</description>
  </channel>
  <item rdf:about="https://example.net/one">
    <title>RDF item</title>
    <link>https://example.net/one</link>
    <description>First</description>
  </item>
</rdf:RDF>"#;

    const JSON_FEED: &str = r#"{
  "version": "https://jsonfeed.org/version/1.1",
  "title": "JSON Example",
  "home_page_url": "https://example.io/",
  "items": [
    { "id": "j1", "url": "https://example.io/1", "title": "JSON item",
      "content_text": "plain body", "date_published": "2026-10-30T10:00:00Z" }
  ]
}"#;

    fn make_input(cfg: Value) -> NodeInput {
        make_input_for("feed1", cfg)
    }

    fn make_input_for(node_id: &str, cfg: Value) -> NodeInput {
        NodeInput {
            resolved_credentials: HashMap::new(),
            cancel_token: None,
            node_id: node_id.to_string(),
            workflow_id: "wf".to_string(),
            execution_id: "exec".to_string(),
            input: cfg,
            context: ExecutionContext::default(),
        }
    }

    async fn run(cfg: Value) -> NodeOutput {
        FeedParserNode { db: None }.execute(make_input(cfg)).await
    }

    fn titles(out: &NodeOutput) -> Vec<String> {
        out.output.as_ref().unwrap()["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["title"].as_str().unwrap().to_string())
            .collect()
    }

    fn test_db() -> (tempfile::TempDir, Arc<WorkflowDb>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("feed.db");
        let db = Arc::new(WorkflowDb::open(&path, 2).unwrap());
        (dir, db)
    }

    fn fake_item(n: usize) -> Item {
        Item {
            id: format!("id{n}"),
            link: String::new(),
            title: format!("t{n}"),
            date: None,
            search: String::new(),
            value: json!({}),
        }
    }

    // ── Formats ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn rss2_feed_is_normalized() {
        let out = run(json!({ "text": RSS })).await;
        assert!(out.success, "{:?}", out.error);
        let data = out.output.unwrap();
        assert_eq!(data["feed"]["title"], json!("Example Feed"));
        assert_eq!(data["feed"]["type"], json!("rss2"));
        assert_eq!(data["feed"]["language"], json!("en"));
        assert_eq!(data["total"], json!(4));
        assert_eq!(data["count"], json!(4));

        let first = &data["items"][0];
        assert_eq!(first["title"], json!("Cooking tips"));
        assert_eq!(first["id"], json!("guid-cooking"));
        assert_eq!(first["published"], json!("2026-10-30T10:00:00Z"));
        assert_eq!(data["latest"], *first);

        let rust = data["items"].as_array().unwrap().iter().find(|i| i["id"] == json!("guid-rust")).unwrap();
        assert_eq!(rust["summary"], json!("Hello & welcome"));
        assert!(rust["summary_html"].as_str().unwrap().contains("<script>"));
        assert_eq!(rust["categories"], json!(["dev"]));
        assert_eq!(rust["enclosures"][0]["url"], json!("https://example.com/a.mp3"));
        assert_eq!(rust["enclosures"][0]["type"], json!("audio/mpeg"));
        assert_eq!(rust["enclosures"][0]["size"], json!(1234));

        let digest = data["digest"].as_str().unwrap();
        assert_eq!(digest.lines().count(), 4);
        assert!(digest.starts_with("- [Cooking tips](https://example.com/cooking)"));
    }

    #[tokio::test]
    async fn atom_feed_is_normalized() {
        let out = run(json!({ "text": ATOM })).await;
        assert!(out.success, "{:?}", out.error);
        let data = out.output.unwrap();
        assert_eq!(data["feed"]["type"], json!("atom"));
        assert_eq!(data["feed"]["link"], json!("https://example.org/"));
        let item = &data["items"][0];
        assert_eq!(item["id"], json!("urn:entry:1"));
        assert_eq!(item["link"], json!("https://example.org/entry-1"));
        assert_eq!(item["author"], json!("Jane Doe"));
        assert_eq!(item["categories"], json!(["News"]));
        assert_eq!(item["published"], json!("2026-10-29T09:00:00Z"));
        assert_eq!(item["updated"], json!("2026-10-30T10:00:00Z"));
        assert_eq!(item["summary"], json!("Short summary"));
        assert_eq!(item["content"], json!("Full text"));
        assert_eq!(item["content_html"], json!("<p>Full <b>text</b></p>"));
        assert_eq!(item["enclosures"][0]["url"], json!("https://example.org/a.ogg"));
        assert_eq!(item["enclosures"][0]["size"], json!(99));
    }

    #[tokio::test]
    async fn rss1_feed_is_parsed() {
        let out = run(json!({ "text": RSS1 })).await;
        assert!(out.success, "{:?}", out.error);
        let data = out.output.unwrap();
        assert_eq!(data["feed"]["type"], json!("rss1"));
        assert_eq!(data["items"][0]["title"], json!("RDF item"));
        assert_eq!(data["items"][0]["link"], json!("https://example.net/one"));
    }

    #[tokio::test]
    async fn json_feed_parses_from_text_and_from_an_already_parsed_object() {
        let from_text = run(json!({ "text": JSON_FEED })).await;
        let parsed: Value = serde_json::from_str(JSON_FEED).unwrap();
        let from_object = run(json!({ "text": parsed })).await;
        assert!(from_text.success, "{:?}", from_text.error);
        assert!(from_object.success, "{:?}", from_object.error);
        let a = from_text.output.unwrap();
        assert_eq!(a["feed"]["type"], json!("json"));
        assert_eq!(a["items"][0]["id"], json!("j1"));
        assert_eq!(a["items"][0]["link"], json!("https://example.io/1"));
        assert_eq!(a["items"][0]["content"], json!("plain body"));
        assert_eq!(a["items"], from_object.output.unwrap()["items"]);
    }

    #[tokio::test]
    async fn relative_links_resolve_against_base_url() {
        let xml = r#"<rss version="2.0"><channel><title>t</title>
            <item><title>a</title><link>/post/1</link><guid>g1</guid></item></channel></rss>"#;
        let out = run(json!({ "text": xml, "base_url": "https://example.com/feed.xml" })).await;
        assert!(out.success, "{:?}", out.error);
        assert_eq!(out.output.unwrap()["items"][0]["link"], json!("https://example.com/post/1"));
    }

    // ── Identity ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn items_without_a_guid_get_the_same_id_on_every_run() {
        let xml = r#"<rss version="2.0"><channel><title>t</title>
            <item><title>No guid</title><link>https://example.com/x</link></item>
            <item><title>No link and no guid</title><description>body</description></item>
            </channel></rss>"#;
        let a = run(json!({ "text": xml, "sort": "feed" })).await.output.unwrap();
        let b = run(json!({ "text": xml, "sort": "feed" })).await.output.unwrap();
        let ids = |v: &Value| -> Vec<String> {
            v["items"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap().to_string()).collect()
        };
        assert_eq!(ids(&a), ids(&b));
        assert!(ids(&a).iter().all(|id| !id.is_empty()));
        assert_ne!(ids(&a)[0], ids(&a)[1]);
    }

    // ── Input handling ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn html_page_is_rejected_as_not_a_feed() {
        let out = run(json!({ "text": "<html><body><h1>404 Not Found</h1></body></html>" })).await;
        assert!(!out.success);
        let err = out.error.unwrap();
        assert_eq!(err.code, "NOT_A_FEED");
        assert!(!err.recoverable);
    }

    #[tokio::test]
    async fn blank_text_is_rejected_as_missing_input() {
        for text in [json!(""), json!("  \n\t "), Value::Null] {
            let out = run(json!({ "text": text })).await;
            assert!(!out.success);
            assert_eq!(out.error.unwrap().code, "MISSING_INPUT");
        }
        assert_eq!(run(json!({})).await.error.unwrap().code, "MISSING_INPUT");
    }

    #[tokio::test]
    async fn input_over_the_size_limit_is_rejected() {
        let out = run(json!({ "text": "a".repeat(MAX_INPUT_BYTES + 1) })).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "INPUT_TOO_LARGE");
    }

    #[tokio::test]
    async fn custom_entities_are_not_expanded() {
        let xml = r#"<?xml version="1.0"?>
<!DOCTYPE rss [<!ENTITY secret "EXPANDED">]>
<rss version="2.0"><channel><title>t</title>
<item><title>a &secret; b</title><link>https://example.com/e</link><guid>e</guid></item>
</channel></rss>"#;
        let out = run(json!({ "text": xml })).await;
        if out.success {
            assert!(!titles(&out)[0].contains("EXPANDED"));
        } else {
            assert_eq!(out.error.unwrap().code, "NOT_A_FEED");
        }
    }

    #[tokio::test]
    async fn declared_latin1_prolog_does_not_garble_text_that_is_already_utf8() {
        let xml = "<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?>\
                   <rss version=\"2.0\"><channel><title>t</title>\
                   <item><title>Café crème</title><link>https://example.com/c</link><guid>c</guid></item>\
                   </channel></rss>";
        let out = run(json!({ "text": xml })).await;
        assert!(out.success, "{:?}", out.error);
        assert_eq!(titles(&out), vec!["Café crème".to_string()]);
    }

    #[test]
    fn prolog_rewrite_keeps_the_rest_of_the_document() {
        let rewritten = normalize_prolog("\u{feff}\n<?xml version='1.0' encoding='windows-1252' standalone='yes'?><a/>");
        assert_eq!(rewritten, "<?xml version='1.0' encoding='UTF-8' standalone='yes'?><a/>");
        assert_eq!(normalize_prolog("<rss/>"), "<rss/>");
    }

    // ── Plain text ──────────────────────────────────────────────────────────

    #[test]
    fn strip_html_removes_markup_scripts_and_decodes_entities() {
        let html = "<style>p{color:red}</style><p>Fish &amp; chips &#8217;s</p><SCRIPT>x()</SCRIPT>\
                    <!-- hidden --><div>Next<br>line</div> 1 &lt; 2 &bogus; a < b";
        assert_eq!(strip_html(html), "Fish & chips ’s\n\nNext\nline\n1 < 2 &bogus; a < b");
    }

    #[test]
    fn truncation_cuts_on_a_character_boundary() {
        assert_eq!(truncate_chars("héllo wörld", 5), "héllo…");
        assert_eq!(truncate_chars("日本語のテキスト", 3), "日本語…");
        assert_eq!(truncate_chars("short", 5), "short");
    }

    #[tokio::test]
    async fn max_text_chars_limits_summary_text() {
        let out = run(json!({ "text": RSS, "max_text_chars": 4 })).await;
        let data = out.output.unwrap();
        let cooking = data["items"].as_array().unwrap().iter().find(|i| i["id"] == json!("guid-cooking")).unwrap();
        assert_eq!(cooking["summary"], json!("Boil…"));
    }

    // ── Filter, sort, limit ─────────────────────────────────────────────────

    #[tokio::test]
    async fn items_sort_newest_first_with_undated_items_last_and_respect_max_items() {
        let all = run(json!({ "text": RSS })).await;
        assert_eq!(
            titles(&all),
            vec!["Cooking tips", "Python release", "Rust news", "Undated sponsor post"]
        );
        let feed_order = run(json!({ "text": RSS, "sort": "feed", "max_items": 2 })).await;
        assert_eq!(titles(&feed_order), vec!["Rust news", "Undated sponsor post"]);
        let limited = run(json!({ "text": RSS, "max_items": 1 })).await.output.unwrap();
        assert_eq!(limited["count"], json!(1));
        assert_eq!(limited["total"], json!(4));
    }

    #[tokio::test]
    async fn keyword_and_date_filters_apply() {
        let included = run(json!({ "text": RSS, "include_keywords": "RUST, python" })).await;
        assert_eq!(titles(&included), vec!["Python release", "Rust news"]);
        let excluded = run(json!({ "text": RSS, "exclude_keywords": "sponsor,cooking" })).await;
        assert_eq!(titles(&excluded), vec!["Python release", "Rust news"]);
        let since = run(json!({ "text": RSS, "since": "2026-10-29" })).await;
        assert_eq!(titles(&since), vec!["Cooking tips", "Python release", "Undated sponsor post"]);
    }

    #[tokio::test]
    async fn bad_config_values_are_rejected() {
        for cfg in [
            json!({ "text": RSS, "max_items": 0 }),
            json!({ "text": RSS, "max_items": 501 }),
            json!({ "text": RSS, "sort": "oldest" }),
            json!({ "text": RSS, "since": "last week" }),
            json!({ "text": RSS, "base_url": "not a url" }),
            json!({ "text": RSS, "only_new": "maybe" }),
        ] {
            let out = run(cfg).await;
            assert!(!out.success);
            assert_eq!(out.error.unwrap().code, "INVALID_CONFIG");
        }
    }

    // ── only_new ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn only_new_without_a_database_fails_instead_of_repeating_every_item() {
        let out = run(json!({ "text": RSS, "only_new": true })).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "PERSIST_UNAVAILABLE");
    }

    #[tokio::test]
    async fn only_new_baselines_first_run_then_returns_just_the_new_items() {
        let (_dir, db) = test_db();
        let node = FeedParserNode { db: Some(db) };
        let cfg = json!({ "text": RSS, "only_new": true });

        let first = node.execute(make_input(cfg.clone())).await;
        assert!(first.success, "{:?}", first.error);
        let first_data = first.output.unwrap();
        assert_eq!(first_data["count"], json!(0));
        assert_eq!(first_data["new_count"], json!(4));
        assert_eq!(first_data["latest"], Value::Null);

        let again = node.execute(make_input(cfg)).await;
        assert_eq!(again.output.unwrap()["new_count"], json!(0));

        let newer = RSS.replace(
            "<item>\n      <title>Rust news</title>",
            "<item><title>Brand new</title><link>https://example.com/new</link><guid>guid-new</guid></item>\
             <item>\n      <title>Rust news</title>",
        );
        let third = node.execute(make_input(json!({ "text": newer, "only_new": true }))).await;
        let third_data = third.output.unwrap();
        assert_eq!(third_data["count"], json!(1));
        assert_eq!(third_data["items"][0]["title"], json!("Brand new"));
    }

    #[tokio::test]
    async fn only_new_can_emit_on_first_run_and_leaves_items_beyond_max_items_for_later() {
        let (_dir, db) = test_db();
        let node = FeedParserNode { db: Some(db) };
        let cfg = json!({ "text": RSS, "only_new": true, "on_first_run": "emit", "max_items": 3 });

        let first = node.execute(make_input(cfg.clone())).await.output.unwrap();
        assert_eq!(first["count"], json!(3));
        assert_eq!(first["new_count"], json!(4));

        let second = node.execute(make_input(cfg)).await.output.unwrap();
        assert_eq!(second["count"], json!(1));
        assert_eq!(second["items"][0]["title"], json!("Undated sponsor post"));
    }

    #[tokio::test]
    async fn only_new_state_is_kept_per_node_and_dedup_by_link_ignores_changed_ids() {
        let (_dir, db) = test_db();
        let node = FeedParserNode { db: Some(db) };

        let a = json!({ "text": RSS, "only_new": true });
        node.execute(make_input_for("feed_a", a)).await;
        let b = json!({ "text": RSS, "only_new": true, "on_first_run": "emit" });
        let other_node = node.execute(make_input_for("feed_b", b)).await.output.unwrap();
        assert_eq!(other_node["count"], json!(4));

        let rotated = RSS.replace("guid-", "rotated-");
        let by_id = node
            .execute(make_input_for("feed_a", json!({ "text": rotated.clone(), "only_new": true })))
            .await
            .output
            .unwrap();
        assert_eq!(by_id["count"], json!(4));

        let node_c = FeedParserNode { db: node.db.clone() };
        let by_link_cfg = json!({ "text": RSS, "only_new": true, "dedup_by": "link" });
        node_c.execute(make_input_for("feed_c", by_link_cfg)).await;
        let by_link = node_c
            .execute(make_input_for("feed_c", json!({ "text": rotated, "only_new": true, "dedup_by": "link" })))
            .await
            .output
            .unwrap();
        assert_eq!(by_link["count"], json!(0));
    }

    #[tokio::test]
    async fn only_new_fails_with_state_error_when_the_database_table_is_unusable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("feed.db");
        let db = Arc::new(WorkflowDb::open(&path, 2).unwrap());
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute("DROP TABLE workflow_variables", [])
            .unwrap();
        let node = FeedParserNode { db: Some(db) };
        let out = node.execute(make_input(json!({ "text": RSS, "only_new": true }))).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "STATE_ERROR");
    }

    #[test]
    fn seen_list_is_capped_and_drops_the_oldest_keys_first() {
        let opts = parse_opts(&json!({ "only_new": true, "sort": "feed", "max_items": 500 })).unwrap();
        let items: Vec<Item> = (0..1200).map(fake_item).collect();
        let selection = select(items, &opts, None);
        let seen = selection.seen_after.unwrap();
        assert_eq!(selection.new_count, 1200);
        assert!(selection.items.is_empty());
        assert_eq!(seen.len(), SEEN_CAP);
        assert!(seen.contains(&dedup_key(&fake_item(0), DedupBy::Id)));
        assert!(!seen.contains(&dedup_key(&fake_item(1199), DedupBy::Id)));
    }
}
