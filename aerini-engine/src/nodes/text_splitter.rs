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
            inputs:  vec![PortDefinition { id: "input".to_string(),  label: "In".to_string(),  position: PortPosition::Left }],
            outputs: vec![PortDefinition { id: "output".to_string(), label: "Chunks".to_string(), position: PortPosition::Right }],
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
