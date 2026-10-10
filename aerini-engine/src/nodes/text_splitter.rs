use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortArity, PortDefinition, PortPosition};
use super::util::{cfg_u64_opt, traverse_dotpath};

/// Text Splitter node — splits long text into overlapping chunks suitable for AI processing.
///
/// Essential for RAG (Retrieval-Augmented Generation) pipelines: you split a document,
/// then feed each chunk to an AI node or embed it for vector search.
pub struct TextSplitterNode;

/// Hard cap on input text length. Without it, and combined with an
/// `overlap >= chunk_size` misconfiguration (see the clamp in `execute()`
/// below), a large document could produce a chunk count approaching its own
/// character count — e.g. an uncapped 200,000-character document with
/// `overlap >= chunk_size` would produce roughly 200,000 near-duplicate
/// chunks. Capping the input bounds the worst case regardless of chunk_size/
/// overlap configuration.
const MAX_TEXT_CHARS: usize = 2_000_000;

/// Hard cap on the number of chunks one run may return. A tiny `chunk_size` on
/// a document near `MAX_TEXT_CHARS` would otherwise return millions of chunks.
const MAX_CHUNKS: usize = 100_000;

const MODES: [&str; 4] = ["chars", "words", "sentences", "paragraphs"];

/// Terminators of scripts written without a space after the sentence mark.
const CJK_TERMINATORS: [char; 4] = ['。', '！', '？', '｡'];

/// Other Unicode Sentence_Terminal characters that are never a decimal point or
/// an abbreviation dot: Arabic question mark and full stop, Armenian full stop,
/// Devanagari danda and double danda, Myanmar section mark, Ethiopic full stop,
/// Khmer khan, and the double and combined ! and ? marks. They end a sentence
/// wherever they appear. The Greek question mark U+037E is left out: Unicode
/// normalization turns it into the ASCII semicolon, so it cannot be told apart.
const OTHER_TERMINATORS: [char; 12] = [
    '\u{061F}', '\u{06D4}', '\u{0589}', '\u{0964}', '\u{0965}', '\u{104B}',
    '\u{1362}', '\u{17D4}', '\u{203C}', '\u{2047}', '\u{2048}', '\u{2049}',
];

/// Abbreviations whose dot does not end a sentence, matched case-insensitively
/// on the word just before the dot. Kept short on purpose: words such as `no`,
/// `etc`, `inc` and `co` end sentences as often as they abbreviate, and `U.S.`,
/// `a.m.` and `p.m.` often end one too.
const ABBREVIATIONS: [&str; 28] = [
    "mr", "mrs", "ms", "mx", "dr", "prof", "sr", "jr", "st", "mt", "vs", "cf", "capt", "col",
    "gen", "lt", "sgt", "rev", "hon", "mme", "mlle", "sra", "srta", "herr", "fr", "approx",
    "dept", "fig",
];
const DOTTED_ABBREVIATIONS: [&str; 6] = ["e.g", "i.e", "ph.d", "z.b", "d.h", "u.a"];

/// Adds a non-empty chunk. Returns false once the list is past `MAX_CHUNKS`, so
/// a splitter stops allocating chunks for a result that will be refused.
fn push_chunk(chunks: &mut Vec<String>, chunk: String) -> bool {
    if chunk.is_empty() {
        return true;
    }
    chunks.push(chunk);
    chunks.len() <= MAX_CHUNKS
}

fn is_cjk_letter(ch: char) -> bool {
    matches!(ch as u32, 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF)
}

#[async_trait]
impl Node for TextSplitterNode {
    fn type_id(&self) -> &'static str { "text_splitter" }
    fn display_name(&self) -> &'static str { "Text Splitter" }
    fn node_type(&self) -> NodeType { NodeType::Ai }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Split a long text into smaller chunks by characters, words, sentences, or paragraphs." }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["text"],
            "properties": {
                "text": {
                    "type": "string",
                    "description": "The text to split into chunks"
                },
                "mode": {
                    "type": "string",
                    "enum": ["chars", "words", "sentences", "paragraphs"],
                    "description": "How to split: chars (by character count), words, sentences, or paragraphs. Default: chars"
                },
                "chunk_size": {
                    "type": "number",
                    "description": "Maximum size of each chunk (in the unit specified by mode), 1 or more. Default: 1000 chars / 200 words / 5 sentences / 3 paragraphs"
                },
                "overlap": {
                    "type": "number",
                    "description": "How much each chunk overlaps with the previous one (same unit as chunk_size). Overlap helps AI maintain context. Default: 100 chars / 20 words / 1 sentence / 0 paragraphs"
                },
                "source_field": {
                    "type": "string",
                    "description": "Optional: dot-path to a string field in the connected node's output, e.g. 'body' or 'content'. Fails if the field is not found. Overrides text."
                }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "chunks":        { "type": "array",  "description": "Array of text chunks" },
                "total_chunks":  { "type": "number", "description": "Total number of chunks" },
                "total_chars":   { "type": "number", "description": "Total character count of input text" },
                "mode":          { "type": "string" },
                "chunk_size":    { "type": "number" },
                "overlap":       { "type": "number" }
            }
        })
    }

    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs:  vec![PortDefinition { id: "input".to_string(),  label: "In".to_string(),  position: PortPosition::Left , port_type: None, arity: PortArity::Single }],
            outputs: vec![PortDefinition { id: "output".to_string(), label: "Chunks".to_string(), position: PortPosition::Right, port_type: None, arity: PortArity::Single }],
        }
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let source_field = input.input["source_field"].as_str().filter(|f| !f.is_empty());
        let text: String = match source_field {
            Some(field) => {
                let found = match input.context.metadata.get("__direct_input") {
                    Some(direct) => traverse_dotpath(direct, field),
                    None => Value::Null,
                };
                match found {
                    Value::String(s) => s,
                    _ => return NodeOutput::failure(NodeError::unrecoverable(
                        "SOURCE_FIELD_NOT_FOUND",
                        format!(
                            "source_field '{}' is not a string in the connected node's output (missing, null, or another type). Fix the field name, or clear source_field to use text.",
                            field
                        ),
                    )),
                }
            }
            None => input.input["text"].as_str().unwrap_or("").to_string(),
        };

        if text.trim().is_empty() {
            return NodeOutput::failure(NodeError::unrecoverable("MISSING_TEXT",
                "text is required. Either configure it directly or set source_field to read it from the connected node."));
        }

        let mode = match input.input["mode"].as_str() {
            None if input.input["mode"].is_null() => "chars",
            Some(m) if m.trim().is_empty() => "chars",
            Some(m) if MODES.contains(&m) => m,
            _ => return NodeOutput::failure(NodeError::unrecoverable(
                "UNKNOWN_MODE",
                format!("Unknown mode {}. Valid modes: {}", input.input["mode"], MODES.join(", ")),
            )),
        };
        let total_chars = text.chars().count();

        if total_chars > MAX_TEXT_CHARS {
            return NodeOutput::failure(NodeError::unrecoverable(
                "TEXT_TOO_LARGE",
                format!(
                    "Input text is {} chars, exceeds the {}-char limit for this node",
                    total_chars, MAX_TEXT_CHARS
                ),
            ));
        }

        let chunk_size = match cfg_u64_opt(&input.input["chunk_size"], "chunk_size") {
            Ok(Some(0)) => return NodeOutput::failure(NodeError::unrecoverable(
                "INVALID_CONFIG",
                "chunk_size must be a whole number, 1 or more",
            )),
            Ok(Some(v)) => v as usize,
            Ok(None) => match mode {
                "sentences" => 5, "paragraphs" => 3, "words" => 200, _ => 1000
            },
            Err(e) => return NodeOutput::failure(e),
        };
        let mut overlap = match cfg_u64_opt(&input.input["overlap"], "overlap") {
            Ok(v) => v.unwrap_or(match mode {
                "sentences" => 1, "paragraphs" => 0, "words" => 20, _ => 100
            }) as usize,
            Err(e) => return NodeOutput::failure(e),
        };

        let mut logs: Vec<String> = Vec::new();
        if overlap >= chunk_size {
            // If overlap >= chunk_size, split_by_*'s own `step = 1` fallback
            // would silently produce a chunk count approaching total_chars
            // for a large document, with no indication anything was wrong —
            // so overlap is clamped and logged instead.
            //
            // Clamping to chunk_size - 1 would still leave step = chunk_size
            // - overlap = 1 — i.e. it stops overlap from being *invalid* but
            // not the *blowup* this guards against; a 1000-char document
            // with chunk_size=100 would still produce ~900 near-duplicate
            // chunks. Clamping to chunk_size / 2 instead bounds step to a
            // meaningful fraction of chunk_size regardless of how large the
            // misconfigured overlap was, capping the worst case at roughly
            // double the zero-overlap chunk count.
            let clamped = chunk_size / 2;
            logs.push(format!(
                "overlap ({}) >= chunk_size ({}) — clamped overlap to {}",
                overlap, chunk_size, clamped
            ));
            overlap = clamped;
        }

        let chunks: Vec<String> = match mode {
            "sentences"  => split_by_sentences(&text, chunk_size, overlap),
            "paragraphs" => split_by_paragraphs(&text, chunk_size, overlap),
            "words"      => split_by_words(&text, chunk_size, overlap),
            _            => split_by_chars(&text, chunk_size, overlap),
        };

        let total_chunks = chunks.len();
        if total_chunks > MAX_CHUNKS {
            return NodeOutput::failure(NodeError::unrecoverable(
                "TOO_MANY_CHUNKS",
                format!(
                    "Splitting would produce more than {} chunks. Raise chunk_size.",
                    MAX_CHUNKS
                ),
            ));
        }
        logs.push(format!("Split {} chars into {} chunks ({} mode)", total_chars, total_chunks, mode));

        NodeOutput::success_with_logs(
            json!({
                "chunks":       chunks,
                "total_chunks": total_chunks,
                "total_chars":  total_chars,
                "mode":         mode,
                "chunk_size":   chunk_size,
                "overlap":      overlap
            }),
            logs,
        )
    }
}

fn split_by_chars(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut chunks = Vec::new();
    let step = if chunk_size > overlap { chunk_size - overlap } else { 1 };
    let mut start = 0;
    while start < chars.len() {
        let end = (start + chunk_size).min(chars.len());
        let chunk: String = chars[start..end].iter().collect();
        if !push_chunk(&mut chunks, chunk.trim().to_string()) { break; }
        if end >= chars.len() { break; }
        start += step;
    }
    chunks
}

fn split_by_words(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut chunks = Vec::new();
    let step = if chunk_size > overlap { chunk_size - overlap } else { 1 };
    let mut start = 0;
    while start < words.len() {
        let end = (start + chunk_size).min(words.len());
        if !push_chunk(&mut chunks, words[start..end].join(" ")) { break; }
        if end >= words.len() { break; }
        start += step;
    }
    chunks
}

fn is_sentence_terminator(ch: char) -> bool {
    matches!(ch, '.' | '!' | '?') || CJK_TERMINATORS.contains(&ch) || OTHER_TERMINATORS.contains(&ch)
}

/// True when the dot at `dot` closes a known abbreviation (`Mrs.`, `e.g.`) or
/// an initial (`J. K. Rowling`), so it does not end a sentence.
fn dot_follows_abbreviation(chars: &[char], dot: usize) -> bool {
    let mut start = dot;
    while start > 0 && (chars[start - 1].is_alphabetic() || chars[start - 1] == '.') {
        start -= 1;
    }
    if start == dot {
        return false;
    }
    if start > 0 && !(chars[start - 1].is_whitespace() || matches!(chars[start - 1], '(' | '[' | '"' | '\'' | '“' | '‘')) {
        return false;
    }
    let token: String = chars[start..dot].iter().flat_map(|c| c.to_lowercase()).collect();
    if ABBREVIATIONS.contains(&token.as_str()) || DOTTED_ABBREVIATIONS.contains(&token.as_str()) {
        return true;
    }
    let is_initial = dot - start == 1 && chars[start].is_uppercase();
    is_initial
        && chars[dot + 1..]
            .iter()
            .find(|c| !c.is_whitespace())
            .is_some_and(|c| c.is_uppercase())
}

fn is_closing_mark(ch: char) -> bool {
    matches!(ch, '"' | '\'' | ')' | ']' | '”' | '’' | '」' | '』' | '）')
}

fn push_trimmed(out: &mut Vec<String>, chars: &[char]) {
    let s: String = chars.iter().collect();
    let s = s.trim();
    if !s.is_empty() { out.push(s.to_string()); }
}

fn find_sentences(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut sentences = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < chars.len() {
        if !is_sentence_terminator(chars[i]) {
            i += 1;
            continue;
        }
        let mut end = i + 1;
        while end < chars.len() && (is_sentence_terminator(chars[end]) || is_closing_mark(chars[end])) {
            end += 1;
        }
        let cjk = chars[i..end].iter().any(|c| CJK_TERMINATORS.contains(c));
        let unambiguous = cjk || chars[i..end].iter().any(|c| OTHER_TERMINATORS.contains(c));
        let at_boundary = unambiguous || end == chars.len() || chars[end].is_whitespace() || is_cjk_letter(chars[end]);
        let abbreviation = end == i + 1 && chars[i] == '.' && dot_follows_abbreviation(&chars, i);
        if at_boundary && !abbreviation && (unambiguous || end - start > 3) {
            push_trimmed(&mut sentences, &chars[start..end]);
            start = end;
        }
        i = end;
    }
    push_trimmed(&mut sentences, &chars[start..]);
    sentences
}

fn ends_with_cjk_terminator(sentence: &str) -> bool {
    sentence
        .trim_end_matches(is_closing_mark)
        .chars()
        .last()
        .is_some_and(|c| CJK_TERMINATORS.contains(&c))
}

fn join_sentences(sentences: &[String]) -> String {
    let mut out = String::new();
    for (i, s) in sentences.iter().enumerate() {
        if i > 0 && !ends_with_cjk_terminator(&sentences[i - 1]) { out.push(' '); }
        out.push_str(s);
    }
    out
}

fn split_by_sentences(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    let sentences = find_sentences(text);
    let step = if chunk_size > overlap { chunk_size - overlap } else { 1 };
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < sentences.len() {
        let end = (start + chunk_size).min(sentences.len());
        if !push_chunk(&mut chunks, join_sentences(&sentences[start..end])) { break; }
        if end >= sentences.len() { break; }
        start += step;
    }
    chunks
}

fn find_paragraphs(text: &str) -> Vec<String> {
    let mut paragraphs = Vec::new();
    let mut lines: Vec<&str> = Vec::new();
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    for line in normalized.lines().chain(std::iter::once("")) {
        if line.trim().is_empty() {
            if !lines.is_empty() {
                paragraphs.push(lines.join("\n").trim().to_string());
                lines.clear();
            }
        } else {
            lines.push(line);
        }
    }
    paragraphs
}

fn split_by_paragraphs(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    let paragraphs = find_paragraphs(text);
    let step = if chunk_size > overlap { chunk_size - overlap } else { 1 };
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < paragraphs.len() {
        let end = (start + chunk_size).min(paragraphs.len());
        if !push_chunk(&mut chunks, paragraphs[start..end].join("\n\n")) { break; }
        if end >= paragraphs.len() { break; }
        start += step;
    }
    chunks
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExecutionContext;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn make_input(input: Value) -> NodeInput {
        NodeInput {
            resolved_credentials: std::collections::HashMap::new(),
            cancel_token: None,
            node_id: "n1".to_string(),
            workflow_id: "wf".to_string(),
            execution_id: "exec".to_string(),
            input,
            context: ExecutionContext::default(),
        }
    }

    fn make_input_with_context(
        input: Value,
        direct: Option<Value>,
        node_outputs: HashMap<String, Value>,
    ) -> NodeInput {
        let mut metadata = HashMap::new();
        if let Some(d) = direct {
            metadata.insert("__direct_input".to_string(), d);
        }
        let order = node_outputs.keys().cloned().collect();
        NodeInput {
            resolved_credentials: std::collections::HashMap::new(),
            cancel_token: None,
            node_id: "n1".to_string(),
            workflow_id: "wf".to_string(),
            execution_id: "exec".to_string(),
            input,
            context: ExecutionContext {
                variables: HashMap::new(),
                node_outputs: Arc::new(node_outputs),
                metadata,
                execution_order: Arc::new(order),
            },
        }
    }

    // ── Failure paths ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn empty_text_returns_failure() {
        let out = TextSplitterNode.execute(make_input(json!({ "text": "" }))).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "MISSING_TEXT");
    }

    #[tokio::test]
    async fn whitespace_only_text_returns_failure() {
        let out = TextSplitterNode.execute(make_input(json!({ "text": "   " }))).await;
        assert!(!out.success);
    }

    #[tokio::test]
    async fn null_text_field_returns_failure() {
        let out = TextSplitterNode.execute(make_input(json!({ "text": null }))).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "MISSING_TEXT");
    }

    // ── source_field resolution ─────────────────────────────────────────────

    #[tokio::test]
    async fn source_field_reads_the_connected_input_not_another_nodes_output() {
        let mut outputs = HashMap::new();
        outputs.insert("a_decoy".to_string(), json!({ "body": "wrong" }));
        let input = make_input_with_context(
            json!({ "mode": "chars", "chunk_size": 100, "overlap": 0, "source_field": "body" }),
            Some(json!({ "body": "right" })),
            outputs,
        );
        let out = TextSplitterNode.execute(input).await;
        assert!(out.success);
        let chunks = out.output.unwrap()["chunks"].as_array().unwrap().clone();
        assert_eq!(chunks[0].as_str().unwrap(), "right");
    }

    #[tokio::test]
    async fn source_field_that_is_not_found_fails_instead_of_using_text() {
        let mut outputs = HashMap::new();
        outputs.insert("other".to_string(), json!({ "body": "elsewhere" }));
        let cases = [
            Some(json!({ "body": "x" })),
            Some(json!({ "body": 42 })),
            None,
        ];
        for direct in cases {
            let input = make_input_with_context(
                json!({ "text": "fallback", "source_field": "body_text" }),
                direct,
                outputs.clone(),
            );
            let out = TextSplitterNode.execute(input).await;
            assert!(!out.success);
            let e = out.error.unwrap();
            assert_eq!(e.code, "SOURCE_FIELD_NOT_FOUND");
            assert!(!e.recoverable);
            assert!(e.message.contains("body_text"), "{}", e.message);
        }
    }

    // ── chars mode ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn chars_mode_chunk_size_respected() {
        let text = "a".repeat(250);
        let out = TextSplitterNode.execute(make_input(json!({
            "text": text,
            "mode": "chars",
            "chunk_size": 100,
            "overlap": 0
        }))).await;
        assert!(out.success);
        let data = out.output.unwrap();
        let chunks = data["chunks"].as_array().unwrap();
        assert_eq!(chunks.len(), 3); // 250/100 = 3 chunks (100, 100, 50)
        assert!(chunks[0].as_str().unwrap().len() <= 100);
    }

    #[tokio::test]
    async fn chars_mode_overlap_produces_more_chunks() {
        let text = "a".repeat(300);
        let no_overlap_out = TextSplitterNode.execute(make_input(json!({
            "text": text.clone(),
            "mode": "chars",
            "chunk_size": 100,
            "overlap": 0
        }))).await;
        let overlap_out = TextSplitterNode.execute(make_input(json!({
            "text": text,
            "mode": "chars",
            "chunk_size": 100,
            "overlap": 50
        }))).await;
        assert!(no_overlap_out.success && overlap_out.success);
        let no_ov_count = no_overlap_out.output.unwrap()["chunks"].as_array().unwrap().len();
        let ov_count    = overlap_out.output.unwrap()["chunks"].as_array().unwrap().len();
        assert!(ov_count > no_ov_count, "overlap should produce more chunks");
    }

    #[tokio::test]
    async fn single_word_shorter_than_chunk_size_produces_one_chunk() {
        let out = TextSplitterNode.execute(make_input(json!({
            "text": "hello",
            "mode": "chars",
            "chunk_size": 1000,
            "overlap": 0
        }))).await;
        assert!(out.success);
        let data = out.output.unwrap();
        let chunks = data["chunks"].as_array().unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].as_str().unwrap(), "hello");
    }

    #[tokio::test]
    async fn utf8_multibyte_not_split_mid_char() {
        // Each '€' is 3 UTF-8 bytes; chunk_size is in chars, not bytes.
        let text = "€".repeat(10); // 10 chars, 30 bytes
        let out = TextSplitterNode.execute(make_input(json!({
            "text": text,
            "mode": "chars",
            "chunk_size": 3,
            "overlap": 0
        }))).await;
        assert!(out.success);
        let chunks = out.output.unwrap();
        for chunk in chunks["chunks"].as_array().unwrap() {
            let s = chunk.as_str().unwrap();
            // Every chunk must be valid UTF-8 (no mid-byte cut)
            // and consist only of '€' characters
            assert!(s.chars().all(|c| c == '€'), "chunk contains non-€: {:?}", s);
        }
    }

    // ── words mode ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn words_mode_splits_correctly() {
        let text = "one two three four five six";
        let out = TextSplitterNode.execute(make_input(json!({
            "text": text,
            "mode": "words",
            "chunk_size": 3,
            "overlap": 0
        }))).await;
        assert!(out.success);
        let data = out.output.unwrap();
        let chunks = data["chunks"].as_array().unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].as_str().unwrap(), "one two three");
        assert_eq!(chunks[1].as_str().unwrap(), "four five six");
    }

    // ── sentences mode ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn sentences_mode_splits_on_punctuation() {
        let text = "Hello. World! How are you?";
        let out = TextSplitterNode.execute(make_input(json!({
            "text": text,
            "mode": "sentences",
            "chunk_size": 1,
            "overlap": 0
        }))).await;
        assert!(out.success);
        let chunks = out.output.unwrap()["chunks"].as_array().unwrap().clone();
        assert_eq!(chunks.len(), 3);
    }

    // ── paragraphs mode ────────────────────────────────────────────────────

    #[tokio::test]
    async fn paragraphs_mode_splits_on_blank_lines() {
        let text = "Para one.\n\nPara two.\n\nPara three.";
        let out = TextSplitterNode.execute(make_input(json!({
            "text": text,
            "mode": "paragraphs",
            "chunk_size": 1,
            "overlap": 0
        }))).await;
        assert!(out.success);
        let chunks = out.output.unwrap()["chunks"].as_array().unwrap().clone();
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].as_str().unwrap(), "Para one.");
    }

    // ── metadata ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn output_includes_metadata_fields() {
        let out = TextSplitterNode.execute(make_input(json!({
            "text": "abc def",
            "mode": "chars",
            "chunk_size": 3,
            "overlap": 0
        }))).await;
        assert!(out.success);
        let data = out.output.unwrap();
        assert!(data["total_chunks"].as_u64().unwrap() > 0);
        assert_eq!(data["total_chars"].as_u64().unwrap(), 7);
        assert_eq!(data["mode"].as_str().unwrap(), "chars");
    }

    // ── Memory caps ──────────────────────────────────────────────────

    #[tokio::test]
    async fn text_over_max_chars_fails_cleanly() {
        let text = "a".repeat(MAX_TEXT_CHARS + 1);
        let out = TextSplitterNode.execute(make_input(json!({
            "text": text, "mode": "chars", "chunk_size": 1000, "overlap": 0
        }))).await;
        assert!(!out.success, "expected failure once text exceeds MAX_TEXT_CHARS");
        assert_eq!(out.error.unwrap().code, "TEXT_TOO_LARGE");
    }

    #[tokio::test]
    async fn text_at_max_chars_succeeds() {
        let text = "a".repeat(MAX_TEXT_CHARS);
        let out = TextSplitterNode.execute(make_input(json!({
            "text": text, "mode": "chars", "chunk_size": 1000, "overlap": 0
        }))).await;
        assert!(out.success, "exactly MAX_TEXT_CHARS must still be accepted");
    }

    #[tokio::test]
    async fn overlap_equal_to_chunk_size_is_clamped_not_silently_stepped() {
        let text = "a".repeat(1000);
        let out = TextSplitterNode.execute(make_input(json!({
            "text": text, "mode": "chars", "chunk_size": 100, "overlap": 100
        }))).await;
        assert!(out.success);
        let data = out.output.unwrap();
        // Clamped overlap (50) keeps the chunk count small and sane, rather
        // than the ≈900 near-duplicate chunks an unclamped step=1 fallback
        // would produce.
        assert_eq!(data["overlap"].as_u64().unwrap(), 50);
        assert!(data["total_chunks"].as_u64().unwrap() < 50,
            "clamp should prevent the near-one-chunk-per-char blowup, got {} chunks",
            data["total_chunks"]);
    }

    #[tokio::test]
    async fn overlap_greater_than_chunk_size_is_clamped_and_logged() {
        let text = "a".repeat(500);
        let out = TextSplitterNode.execute(make_input(json!({
            "text": text, "mode": "chars", "chunk_size": 50, "overlap": 9999
        }))).await;
        assert!(out.success);
        assert_eq!(out.output.unwrap()["overlap"].as_u64().unwrap(), 25);
        assert!(
            out.logs.iter().any(|l| l.contains("clamped overlap")),
            "clamp must be logged, not silent: {:?}", out.logs
        );
    }

    // ── paragraphs mode ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn paragraphs_mode_with_chunk_size_at_or_above_paragraph_count_gives_one_chunk() {
        for cfg in [
            json!({ "text": "One.\n\nTwo.", "mode": "paragraphs" }),
            json!({ "text": "One.\n\nTwo.", "mode": "paragraphs", "chunk_size": 2, "overlap": 0 }),
        ] {
            let out = TextSplitterNode.execute(make_input(cfg)).await;
            assert!(out.success);
            let chunks = out.output.unwrap()["chunks"].as_array().unwrap().clone();
            assert_eq!(chunks.len(), 1);
            assert_eq!(chunks[0].as_str().unwrap(), "One.\n\nTwo.");
        }
    }

    #[tokio::test]
    async fn paragraphs_mode_splits_on_crlf_and_on_blank_lines_holding_spaces_or_tabs() {
        let out = TextSplitterNode.execute(make_input(json!({
            "text": "A1\r\nA2\r\n\r\nB\n \t \nC",
            "mode": "paragraphs",
            "chunk_size": 1,
            "overlap": 0
        }))).await;
        assert!(out.success);
        let chunks: Vec<String> = out.output.unwrap()["chunks"].as_array().unwrap()
            .iter().map(|c| c.as_str().unwrap().to_string()).collect();
        assert_eq!(chunks, vec!["A1\nA2", "B", "C"]);
    }

    // ── sentences mode ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn sentences_mode_splits_cjk_terminators_without_adding_spaces() {
        let text = "你好。世界！再见？";
        let run = |chunk_size: u64| {
            TextSplitterNode.execute(make_input(json!({
                "text": text, "mode": "sentences", "chunk_size": chunk_size, "overlap": 0
            })))
        };
        let one: Vec<String> = run(1).await.output.unwrap()["chunks"].as_array().unwrap()
            .iter().map(|c| c.as_str().unwrap().to_string()).collect();
        assert_eq!(one, vec!["你好。", "世界！", "再见？"]);
        let two: Vec<String> = run(2).await.output.unwrap()["chunks"].as_array().unwrap()
            .iter().map(|c| c.as_str().unwrap().to_string()).collect();
        assert_eq!(two, vec!["你好。世界！", "再见？"]);
    }

    #[tokio::test]
    async fn sentences_mode_does_not_split_inside_decimals_or_urls_and_keeps_closing_quotes() {
        let text = "Pi is 3.14 today. See https://a.com/x?y=1 now. He said \"stop.\" Then left.";
        let out = TextSplitterNode.execute(make_input(json!({
            "text": text, "mode": "sentences", "chunk_size": 1, "overlap": 0
        }))).await;
        assert!(out.success);
        let chunks: Vec<String> = out.output.unwrap()["chunks"].as_array().unwrap()
            .iter().map(|c| c.as_str().unwrap().to_string()).collect();
        assert_eq!(chunks, vec![
            "Pi is 3.14 today.",
            "See https://a.com/x?y=1 now.",
            "He said \"stop.\"",
            "Then left.",
        ]);
    }

    #[tokio::test]
    async fn sentences_mode_short_sentence_guard_counts_characters_not_bytes() {
        let count = |text: &'static str| async move {
            let out = TextSplitterNode.execute(make_input(json!({
                "text": text, "mode": "sentences", "chunk_size": 1, "overlap": 0
            }))).await;
            out.output.unwrap()["total_chunks"].as_u64().unwrap()
        };
        assert_eq!(count("El. Ca va.").await, count("Él. Ça va.").await);
    }

    // ── mode and limits ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn unknown_or_non_string_mode_fails_naming_the_valid_modes_and_blank_means_chars() {
        for bad in [json!("token"), json!("Words"), json!(5)] {
            let out = TextSplitterNode.execute(make_input(json!({ "text": "abc", "mode": bad }))).await;
            assert!(!out.success);
            let e = out.error.unwrap();
            assert_eq!(e.code, "UNKNOWN_MODE");
            assert!(!e.recoverable);
            for valid in ["chars", "words", "sentences", "paragraphs"] {
                assert!(e.message.contains(valid), "{}", e.message);
            }
        }
        let out = TextSplitterNode.execute(make_input(json!({ "text": "abc", "mode": " " }))).await;
        assert_eq!(out.output.unwrap()["mode"].as_str().unwrap(), "chars");
    }

    #[tokio::test]
    async fn chunk_size_zero_fails_instead_of_meaning_no_split() {
        for zero in [json!(0), json!("0")] {
            let out = TextSplitterNode.execute(make_input(json!({ "text": "abc def", "chunk_size": zero }))).await;
            assert!(!out.success);
            let e = out.error.unwrap();
            assert_eq!(e.code, "INVALID_CONFIG");
            assert!(e.message.contains("chunk_size"), "{}", e.message);
        }
    }

    #[tokio::test]
    async fn more_than_max_chunks_fails_cleanly_and_exactly_max_succeeds() {
        let run = |n: usize| TextSplitterNode.execute(make_input(json!({
            "text": "a".repeat(n), "mode": "chars", "chunk_size": 1, "overlap": 0
        })));
        assert!(run(MAX_CHUNKS).await.success);
        let out = run(MAX_CHUNKS + 1).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "TOO_MANY_CHUNKS");
    }

    #[test]
    fn a_bare_carriage_return_is_a_line_break_in_paragraph_mode() {
        assert_eq!(find_paragraphs("A1\rA2\r\rB"), ["A1\nA2", "B"]);
        assert_eq!(find_paragraphs("A1\r\nA2\r\n\r\nB"), ["A1\nA2", "B"]);
    }

    #[test]
    fn a_latin_terminator_directly_before_cjk_ends_the_sentence_but_a_decimal_does_not() {
        assert_eq!(find_sentences("Hello world.你好世界"), ["Hello world.", "你好世界"]);
        assert_eq!(find_sentences("Pi is 3.14 today."), ["Pi is 3.14 today."]);
    }

    #[test]
    fn abbreviations_and_initials_do_not_end_a_sentence_but_ordinary_words_do() {
        assert_eq!(find_sentences("Mrs. Smith left. Then Dr. Who? Yes."), ["Mrs. Smith left.", "Then Dr. Who?", "Yes."]);
        assert_eq!(find_sentences("See e.g. this one. Done."), ["See e.g. this one.", "Done."]);
        assert_eq!(find_sentences("J. K. Rowling wrote it. Fine."), ["J. K. Rowling wrote it.", "Fine."]);
        assert_eq!(find_sentences("He said no. Then he left."), ["He said no.", "Then he left."]);
        assert_eq!(find_sentences("We met (Prof. Lee) today. Ok."), ["We met (Prof. Lee) today.", "Ok."]);
        assert_eq!(find_sentences("It was etc. Then more."), ["It was etc.", "Then more."]);
    }

    #[test]
    fn other_script_terminators_end_a_sentence_and_are_joined_with_a_space() {
        assert_eq!(find_sentences("كيف حالك؟ أنا بخير۔ شكرا"), ["كيف حالك؟", "أنا بخير۔", "شكرا"]);
        assert_eq!(find_sentences("यह पहला है। यह दूसरा है॥ तीसरा"), ["यह पहला है।", "यह दूसरा है॥", "तीसरा"]);
        assert_eq!(find_sentences("Really⁇ Yes‼ Fine"), ["Really⁇", "Yes‼", "Fine"]);
        assert_eq!(join_sentences(&["यह पहला है।".to_string(), "दूसरा".to_string()]), "यह पहला है। दूसरा");
    }

    #[test]
    fn whitespace_only_chunks_do_not_count_toward_the_chunk_cap() {
        let mut chunks = Vec::new();
        for _ in 0..(MAX_CHUNKS + 5) {
            assert!(push_chunk(&mut chunks, String::new()));
        }
        assert!(chunks.is_empty());
        for _ in 0..MAX_CHUNKS {
            assert!(push_chunk(&mut chunks, "a".to_string()));
        }
        assert!(!push_chunk(&mut chunks, "a".to_string()));
    }
}
