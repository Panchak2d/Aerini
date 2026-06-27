use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::{Node, NodePorts, PortDefinition, PortPosition};
use super::util::traverse_dotpath;

/// Text Splitter node — splits long text into overlapping chunks suitable for AI processing.
///
/// Essential for RAG (Retrieval-Augmented Generation) pipelines: you split a document,
/// then feed each chunk to an AI node or embed it for vector search.
pub struct TextSplitterNode;

#[async_trait]
impl Node for TextSplitterNode {
    fn type_id(&self) -> &'static str { "text_splitter" }
    fn display_name(&self) -> &'static str { "Text Splitter" }
    fn node_type(&self) -> NodeType { NodeType::Ai }
    fn version(&self) -> &'static str { "1.0.0" }
    fn description(&self) -> &'static str { "Split a long text into smaller chunks by character count, token count, or paragraph boundaries." }

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
                    "description": "Maximum size of each chunk (in the unit specified by mode). Default: 1000 chars / 200 words / 5 sentences"
                },
                "overlap": {
                    "type": "number",
                    "description": "How much each chunk overlaps with the previous one (same unit as chunk_size). Overlap helps AI maintain context. Default: 100"
                },
                "source_field": {
                    "type": "string",
                    "description": "Optional: dot-path to extract text from the input data, e.g. 'body' or 'content'"
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
            inputs:  vec![PortDefinition { id: "input".to_string(),  label: "In".to_string(),  position: PortPosition::Left , port_type: None }],
            outputs: vec![PortDefinition { id: "output".to_string(), label: "Chunks".to_string(), position: PortPosition::Right, port_type: None }],
        }
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        // Resolve text — either from direct config or from a field in previous node output
        let text: String = input.input["source_field"]
            .as_str()
            .filter(|f| !f.is_empty())
            .and_then(|field| {
                input.context.node_outputs.values().find_map(|v| {
                    let val = traverse_dotpath(v, field);
                    val.as_str().map(|s| s.to_string())
                })
            })
            .unwrap_or_else(|| input.input["text"].as_str().unwrap_or("").to_string());

        if text.trim().is_empty() {
            return NodeOutput::failure(NodeError::unrecoverable("MISSING_TEXT",
                "text is required. Either configure it directly or set source_field to extract it from a previous node."));
        }

        let mode = input.input["mode"].as_str().unwrap_or("chars");
        let total_chars = text.chars().count();

        let chunk_size = input.input["chunk_size"].as_u64().unwrap_or(match mode {
            "sentences" => 5, "paragraphs" => 3, "words" => 200, _ => 1000
        }) as usize;
        let overlap = input.input["overlap"].as_u64().unwrap_or(match mode {
            "sentences" => 1, "paragraphs" => 0, "words" => 20, _ => 100
        }) as usize;

        let chunks: Vec<String> = match mode {
            "sentences"  => split_by_sentences(&text, chunk_size, overlap),
            "paragraphs" => split_by_paragraphs(&text, chunk_size, overlap),
            "words"      => split_by_words(&text, chunk_size, overlap),
            _            => split_by_chars(&text, chunk_size, overlap),
        };

        let total_chunks = chunks.len();

        NodeOutput::success_with_logs(
            json!({
                "chunks":       chunks,
                "total_chunks": total_chunks,
                "total_chars":  total_chars,
                "mode":         mode,
                "chunk_size":   chunk_size,
                "overlap":      overlap
            }),
            vec![format!("Split {} chars into {} chunks ({} mode)", total_chars, total_chunks, mode)],
        )
    }
}

fn split_by_chars(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    if chunk_size == 0 { return vec![text.to_string()]; }
    let chars: Vec<char> = text.chars().collect();
    let mut chunks = Vec::new();
    let step = if chunk_size > overlap { chunk_size - overlap } else { 1 };
    let mut start = 0;
    while start < chars.len() {
        let end = (start + chunk_size).min(chars.len());
        let chunk: String = chars[start..end].iter().collect();
        chunks.push(chunk.trim().to_string());
        if end >= chars.len() { break; }
        start += step;
    }
    chunks.into_iter().filter(|c| !c.is_empty()).collect()
}

fn split_by_words(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    if chunk_size == 0 { return vec![text.to_string()]; }
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut chunks = Vec::new();
    let step = if chunk_size > overlap { chunk_size - overlap } else { 1 };
    let mut start = 0;
    while start < words.len() {
        let end = (start + chunk_size).min(words.len());
        chunks.push(words[start..end].join(" "));
        if end >= words.len() { break; }
        start += step;
    }
    chunks.into_iter().filter(|c| !c.is_empty()).collect()
}

fn split_by_sentences(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    // Simple sentence splitter on . ! ?
    let mut sentences: Vec<String> = Vec::new();
    let mut sentence = String::new();
    for ch in text.chars() {
        sentence.push(ch);
        if matches!(ch, '.' | '!' | '?') && sentence.len() > 3 {
            let trimmed = sentence.trim().to_string();
            if !trimmed.is_empty() { sentences.push(trimmed); }
            sentence = String::new();
        }
    }
    if !sentence.trim().is_empty() { sentences.push(sentence.trim().to_string()); }

    if chunk_size == 0 { return sentences; }
    let step = if chunk_size > overlap { chunk_size - overlap } else { 1 };
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < sentences.len() {
        let end = (start + chunk_size).min(sentences.len());
        chunks.push(sentences[start..end].join(" "));
        if end >= sentences.len() { break; }
        start += step;
    }
    chunks.into_iter().filter(|c| !c.is_empty()).collect()
}

fn split_by_paragraphs(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    let paragraphs: Vec<String> = text
        .split("\n\n")
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();

    if chunk_size == 0 || chunk_size >= paragraphs.len() {
        return paragraphs;
    }

    let step = if chunk_size > overlap { chunk_size - overlap } else { 1 };
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < paragraphs.len() {
        let end = (start + chunk_size).min(paragraphs.len());
        chunks.push(paragraphs[start..end].join("\n\n"));
        if end >= paragraphs.len() { break; }
        start += step;
    }
    chunks.into_iter().filter(|c| !c.is_empty()).collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExecutionContext;
    use serde_json::json;

    fn make_input(input: Value) -> NodeInput {
        NodeInput {
            node_id: "n1".to_string(),
            workflow_id: "wf".to_string(),
            execution_id: "exec".to_string(),
            input,
            context: ExecutionContext::default(),
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

    // ── split_by_chars unit tests ───────────────────────────────────────────

    #[test]
    fn split_by_chars_zero_chunk_size_returns_whole() {
        let chunks = split_by_chars("hello world", 0, 0);
        assert_eq!(chunks, vec!["hello world"]);
    }

    #[test]
    fn split_by_words_zero_chunk_size_returns_whole() {
        let chunks = split_by_words("a b c", 0, 0);
        assert_eq!(chunks, vec!["a b c"]);
    }
}
