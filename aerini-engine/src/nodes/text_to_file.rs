use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::Node;

pub struct TextToFileNode;

#[async_trait]
impl Node for TextToFileNode {
    fn type_id(&self)       -> &'static str { "text_to_file" }
    fn display_name(&self)  -> &'static str { "Text to File" }
    fn node_type(&self)     -> NodeType     { NodeType::Utility }
    fn version(&self)       -> &'static str { "1.0.0" }
    fn description(&self)   -> &'static str {
        "Converts a text string into a file object. Use between AI Prompt and Save to Folder to write AI output to disk."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["content"],
            "properties": {
                "content": {
                    "type": "string",
                    "description": "Text content to encode as a file"
                },
                "filename": {
                    "type": "string",
                    "description": "Output filename (default: output.txt)"
                },
                "mime_type": {
                    "type": "string",
                    "description": "MIME type (default: text/plain)"
                },
                "format": {
                    "type": "string",
                    "enum": ["txt", "md", "html", "csv", "json", "pdf", "docx"],
                    "description": "Output format. Falls back to the filename extension, then 'txt', when omitted. pdf/docx produce real binary documents (headings rendered from leading #/##/###; docx also renders **bold**/*italic*); other formats pass the text through unchanged."
                }
            }
        })
    }

    fn output_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "files": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "filename":  { "type": "string" },
                            "data":      { "type": "string", "description": "Base64-encoded file content" },
                            "mime_type": { "type": "string" }
                        }
                    }
                }
            }
        })
    }

    async fn execute(&self, input: NodeInput) -> NodeOutput {
        let content = match input.input["content"].as_str() {
            Some(s) if !s.is_empty() => s,
            _ => return NodeOutput::failure(
                NodeError::unrecoverable("MISSING_CONTENT", "content is required and must not be empty")
            ),
        };

        let filename_in = input.input["filename"].as_str().filter(|s| !s.is_empty());

        // Explicit `format` wins; otherwise infer from the filename extension; otherwise "txt".
        // This keeps older saved workflows (created before this field existed) working unchanged.
        let format_owned: String = input.input["format"].as_str()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_lowercase())
            .unwrap_or_else(|| {
                filename_in
                    .filter(|f| f.contains('.'))
                    .and_then(|f| f.rsplit('.').next())
                    .map(|ext| ext.to_lowercase())
                    .unwrap_or_else(|| "txt".to_string())
            });

        // Default filename uses the resolved format as extension so "output.pdf" is not
        // saved as "output.txt". An explicit filename always wins.
        let default_filename = format!("output.{}", format_owned);
        let filename = filename_in.unwrap_or(default_filename.as_str());

        let (data, mime_type): (String, &str) = match format_owned.as_str() {
            "pdf" => {
                let bytes = export::text_to_pdf_bytes(content);
                let mime = input.input["mime_type"].as_str()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("application/pdf");
                (BASE64.encode(bytes), mime)
            }
            "docx" => {
                let bytes = match export::text_to_docx_bytes(content) {
                    Ok(b) => b,
                    Err(e) => return NodeOutput::failure(
                        NodeError::unrecoverable("DOCX_BUILD_FAILED", format!("Failed to build DOCX: {e}"))
                    ),
                };
                let mime = input.input["mime_type"].as_str()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("application/vnd.openxmlformats-officedocument.wordprocessingml.document");
                (BASE64.encode(bytes), mime)
            }
            _ => {
                let mime = input.input["mime_type"].as_str()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("text/plain");
                (BASE64.encode(content.as_bytes()), mime)
            }
        };

        NodeOutput::success(json!({
            "files": [{ "filename": filename, "data": data, "mime_type": mime_type }]
        }))
    }
}

// ---------------------------------------------------------------------------
// PDF / DOCX export
//
// API usage verified against pdf-writer 0.15.0's actual published source
// (downloaded from static.crates.io and inspected directly, 2026-06-28 —
// every method signature below was read out of src/*.rs, not assumed from
// docs) and against docx-rs 0.4.20 (docs.rs, 2026-06-24). Neither crate could
// be compiled in this session (no Rust toolchain available in this
// environment) — mark UNCERTAIN at the build/runtime level until
// `cargo check` is run; the API *shapes* used below are VERIFIED, not guessed.
// ---------------------------------------------------------------------------
mod export {
    use pdf_writer::{Content, Finish, Name, Pdf, Rect, Ref, Str};
    use docx_rs::{BreakType, Docx, Paragraph, Run};
    use std::io::Cursor;

    /// Exact mm→pt conversion (72pt / 25.4mm per inch). A fixed unit-conversion
    /// constant, not a fact that can go stale.
    const MM_TO_PT: f64 = 2.834_645_669;

    /// Heuristic average glyph advance width for Helvetica, as a fraction of font
    /// size. Real per-glyph AFM metrics for the standard 14 fonts were not
    /// exercised in this patch; this approximation deliberately errs toward
    /// wrapping a line a little early rather than overflowing the page margin.
    const AVG_CHAR_WIDTH_FACTOR: f64 = 0.56;

    fn wrap_line(line: &str, max_width_pt: f64, font_size_pt: f64) -> Vec<String> {
        if line.trim().is_empty() {
            return vec![String::new()];
        }
        let char_width = font_size_pt * AVG_CHAR_WIDTH_FACTOR;
        let max_chars = ((max_width_pt / char_width).floor() as usize).max(1);

        let mut out = Vec::new();
        let mut current = String::new();
        for word in line.split_whitespace() {
            let candidate_len = if current.is_empty() {
                word.chars().count()
            } else {
                current.chars().count() + 1 + word.chars().count()
            };
            if candidate_len > max_chars && !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
            // A single word longer than max_chars is left unbroken — acceptable
            // for this exporter's plain-text use case (no mid-word hyphenation).
        }
        if !current.is_empty() || out.is_empty() {
            out.push(current);
        }
        out
    }

    /// Detects a leading markdown heading marker (#, ##, ###) and strips
    /// emphasis markers from the remaining text. Returns (heading_level, text);
    /// heading_level is 0 for body text.
    fn classify_line(raw: &str) -> (usize, String) {
        let trimmed = raw.trim_start();
        for (prefix, level) in [("### ", 3usize), ("## ", 2), ("# ", 1)] {
            if let Some(rest) = trimmed.strip_prefix(prefix) {
                return (level, strip_emphasis_markers(rest));
            }
        }
        (0, strip_emphasis_markers(raw))
    }

    /// Removes literal `**` / `*` markers without rendering inline bold/italic.
    /// Builtin PDF fonts model bold/italic as separate font variants (e.g.
    /// Helvetica-Bold), not a per-run style toggle within one line of text, and
    /// tracking the text cursor correctly across mixed-style runs on a single
    /// line is real added complexity this exporter doesn't need yet — so it
    /// styles per *line* only (headings). The DOCX exporter below does support
    /// true inline bold/italic.
    fn strip_emphasis_markers(s: &str) -> String {
        s.replace("**", "").replace('*', "")
    }

    /// Encodes text for the standard, non-embedded Type1 fonts under
    /// /Encoding /WinAnsiEncoding. WinAnsi == Windows-1252: identical to
    /// ASCII for 0x20-0x7E and to Latin-1 for 0xA0-0xFF; the only quirk is
    /// 0x80-0x9F, which Latin-1 reserves for C1 controls but WinAnsi instead
    /// uses for punctuation AI-generated markdown commonly produces (smart
    /// quotes, en/em dash, ellipsis). Those are mapped explicitly below;
    /// anything outside Latin-1 (CJK, emoji, etc.) becomes '?' — the same
    /// pre-existing limit any non-embedded standard font has (only 256
    /// glyphs, no Unicode coverage), not a new regression from this patch.
    fn encode_winansi(s: &str) -> Vec<u8> {
        s.chars()
            .map(|c| match c {
                '\u{20}'..='\u{7E}' => c as u8,
                '\u{20AC}' => 0x80, // €
                '\u{201A}' => 0x82, // ‚
                '\u{0192}' => 0x83, // ƒ
                '\u{201E}' => 0x84, // „
                '\u{2026}' => 0x85, // …
                '\u{2020}' => 0x86, // †
                '\u{2021}' => 0x87, // ‡
                '\u{02C6}' => 0x88, // ˆ
                '\u{2030}' => 0x89, // ‰
                '\u{0160}' => 0x8A, // Š
                '\u{2039}' => 0x8B, // ‹
                '\u{0152}' => 0x8C, // Œ
                '\u{017D}' => 0x8E, // Ž
                '\u{2018}' => 0x91, // '
                '\u{2019}' => 0x92, // '
                '\u{201C}' => 0x93, // "
                '\u{201D}' => 0x94, // "
                '\u{2022}' => 0x95, // •
                '\u{2013}' => 0x96, // –
                '\u{2014}' => 0x97, // —
                '\u{02DC}' => 0x98, // ˜
                '\u{2122}' => 0x99, // ™
                '\u{0161}' => 0x9A, // š
                '\u{203A}' => 0x9B, // ›
                '\u{0153}' => 0x9C, // œ
                '\u{017E}' => 0x9E, // ž
                '\u{0178}' => 0x9F, // Ÿ
                '\u{00A0}'..='\u{00FF}' => c as u8, // Latin-1 supplement
                _ => b'?',
            })
            .collect()
    }

    /// Renders plain text (with #/##/### heading lines) to a PDF using only the
    /// 14 standard PDF fonts — no font files are embedded.
    pub fn text_to_pdf_bytes(content: &str) -> Vec<u8> {
        let content = content.replace("\r\n", "\n");
        const PAGE_W_MM: f64 = 210.0;
        const PAGE_H_MM: f64 = 297.0;
        const MARGIN_MM: f64 = 15.0;
        let usable_width_pt = (PAGE_W_MM - 2.0 * MARGIN_MM) * MM_TO_PT;
        let top_y_mm = PAGE_H_MM - MARGIN_MM;
        let bottom_y_mm = MARGIN_MM;
        let page_w_pt = (PAGE_W_MM * MM_TO_PT) as f32;
        let page_h_pt = (PAGE_H_MM * MM_TO_PT) as f32;

        const BODY_SIZE_PT: f64 = 11.0;
        const BODY_LINE_HEIGHT_MM: f64 = 5.5;
        const HEADING_SIZE_PT: [f64; 4] = [0.0, 18.0, 15.0, 13.0];
        const HEADING_LINE_HEIGHT_MM: [f64; 4] = [0.0, 9.0, 7.5, 6.5];

        // Sequential PDF indirect-object id allocator. Object count isn't known
        // up front (page breaks happen as lines are laid out below), so ids for
        // pages/content streams are handed out lazily as each page is opened.
        let mut next_id: i32 = 0;
        macro_rules! alloc_ref {
            () => {{
                next_id += 1;
                Ref::new(next_id)
            }};
        }

        let catalog_id = alloc_ref!();
        let page_tree_id = alloc_ref!();
        let font_regular_id = alloc_ref!();
        let font_bold_id = alloc_ref!();
        let font_regular_name = Name(b"F1");
        let font_bold_name = Name(b"F2");

        let mut finished_pages: Vec<(Ref, Ref, Content)> = Vec::new();
        let mut page_id = alloc_ref!();
        let mut content_id = alloc_ref!();
        let mut page_content = Content::new();
        let mut cursor_y_mm = top_y_mm;

        for raw_line in content.split('\n') {
            let (level, text) = classify_line(raw_line);
            let (font_name, size_pt, line_h_mm) = if level > 0 {
                (font_bold_name, HEADING_SIZE_PT[level], HEADING_LINE_HEIGHT_MM[level])
            } else {
                (font_regular_name, BODY_SIZE_PT, BODY_LINE_HEIGHT_MM)
            };

            let wrapped = wrap_line(&text, usable_width_pt, size_pt);

            for line_text in wrapped {
                if cursor_y_mm - line_h_mm < bottom_y_mm {
                    finished_pages.push((page_id, content_id, page_content));
                    page_id = alloc_ref!();
                    content_id = alloc_ref!();
                    page_content = Content::new();
                    cursor_y_mm = top_y_mm;
                }
                if !line_text.is_empty() {
                    let x_pt = (MARGIN_MM * MM_TO_PT) as f32;
                    let y_pt = (cursor_y_mm * MM_TO_PT) as f32;
                    let bytes = encode_winansi(&line_text);
                    // Each line gets its own BT/ET block so that `next_line`'s
                    // move is absolute from the page origin: BT resets the
                    // text line matrix to identity (verified against
                    // pdf-writer's own examples/hello.rs, which uses this
                    // exact single-line-per-block pattern for its first line).
                    page_content.begin_text();
                    page_content.set_font(font_name, size_pt as f32);
                    page_content.next_line(x_pt, y_pt);
                    page_content.show(Str(&bytes));
                    page_content.end_text();
                }
                cursor_y_mm -= line_h_mm;
            }
        }
        finished_pages.push((page_id, content_id, page_content));

        let mut pdf = Pdf::new();
        pdf.catalog(catalog_id).pages(page_tree_id);

        let page_refs: Vec<Ref> = finished_pages.iter().map(|(pid, _, _)| *pid).collect();
        pdf.pages(page_tree_id)
            .kids(page_refs.iter().copied())
            .count(page_refs.len() as i32);

        pdf.type1_font(font_regular_id)
            .base_font(Name(b"Helvetica"))
            .encoding_predefined(Name(b"WinAnsiEncoding"));
        pdf.type1_font(font_bold_id)
            .base_font(Name(b"Helvetica-Bold"))
            .encoding_predefined(Name(b"WinAnsiEncoding"));

        for (page_id, content_id, _) in &finished_pages {
            let mut page = pdf.page(*page_id);
            page.media_box(Rect::new(0.0, 0.0, page_w_pt, page_h_pt));
            page.parent(page_tree_id);
            page.contents(*content_id);
            page.resources()
                .fonts()
                .pair(font_regular_name, font_regular_id)
                .pair(font_bold_name, font_bold_id);
            page.finish();
        }

        for (_, content_id, page_content) in finished_pages {
            pdf.stream(content_id, &page_content.finish());
        }

        pdf.finish()
    }

    /// Splits `line` on `**bold**` / `*italic*` markers into styled runs.
    /// Non-nested, simple state machine — adequate for typical AI-generated
    /// markdown; unmatched markers toggle state without erroring or panicking.
    fn parse_inline_runs(line: &str) -> Vec<Run> {
        let mut runs = Vec::new();
        let mut chars = line.chars().peekable();
        let mut buf = String::new();
        let mut bold = false;
        let mut italic = false;

        fn flush(buf: &mut String, bold: bool, italic: bool, runs: &mut Vec<Run>) {
            if buf.is_empty() {
                return;
            }
            let mut run = Run::new().add_text(std::mem::take(buf));
            if bold {
                run = run.bold();
            }
            if italic {
                run = run.italic();
            }
            runs.push(run);
        }

        while let Some(c) = chars.next() {
            if c == '*' {
                if chars.peek() == Some(&'*') {
                    chars.next();
                    flush(&mut buf, bold, italic, &mut runs);
                    bold = !bold;
                } else {
                    flush(&mut buf, bold, italic, &mut runs);
                    italic = !italic;
                }
            } else {
                buf.push(c);
            }
        }
        flush(&mut buf, bold, italic, &mut runs);
        runs
    }

    /// Renders plain text to a DOCX. Paragraphs are split on blank lines
    /// (`\n\n`); a leading #/##/### marks a heading (bold, larger size);
    /// `**bold**` / `*italic*` are rendered as real DOCX run formatting.
    pub fn text_to_docx_bytes(content: &str) -> Result<Vec<u8>, String> {
        let content = content.replace("\r\n", "\n");
        let mut docx = Docx::new();

        for raw_para in content.split("\n\n") {
            if raw_para.trim().is_empty() {
                docx = docx.add_paragraph(Paragraph::new());
                continue;
            }

            let mut lines = raw_para.lines();
            let first = lines.next().unwrap_or("");
            let (level, first_text) = classify_line(first);

            if level > 0 {
                // Heading: whole-paragraph styling, no inline-run parsing — any
                // further lines in the same blank-line-delimited chunk are
                // joined into the heading rather than treated as body text.
                let mut heading_text = first_text;
                for rest in lines {
                    heading_text.push(' ');
                    heading_text.push_str(&strip_emphasis_markers(rest.trim_start()));
                }
                let size_half_pt: usize = match level { 1 => 36, 2 => 30, _ => 26 };
                let run = Run::new().add_text(heading_text).bold().size(size_half_pt);
                docx = docx.add_paragraph(Paragraph::new().add_run(run));
                continue;
            }

            let body_lines: Vec<&str> = raw_para.lines().collect();
            let mut paragraph = Paragraph::new();
            for (i, line) in body_lines.iter().enumerate() {
                for run in parse_inline_runs(line) {
                    paragraph = paragraph.add_run(run);
                }
                if i + 1 < body_lines.len() {
                    paragraph = paragraph.add_run(Run::new().add_break(BreakType::TextWrapping));
                }
            }
            docx = docx.add_paragraph(paragraph);
        }

        let mut buf = Cursor::new(Vec::new());
        docx.build()
            .pack(&mut buf)
            .map_err(|e| format!("{e:?}"))?;
        Ok(buf.into_inner())
    }
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

    // ── Base64 correctness ──────────────────────────────────────────────────

    #[tokio::test]
    async fn hello_encodes_to_correct_base64() {
        // "Hello" → SGVsbG8= (RFC 4648, confirmed)
        let out = TextToFileNode.execute(make_input(json!({ "content": "Hello" }))).await;
        assert!(out.success);
        let files = out.output.unwrap()["files"].as_array().unwrap().clone();
        assert_eq!(files[0]["data"], json!("SGVsbG8="));
    }

    #[tokio::test]
    async fn base64_roundtrip_arbitrary_text() {
        let content = "The quick brown fox jumps over the lazy dog.";
        let out = TextToFileNode.execute(make_input(json!({ "content": content }))).await;
        assert!(out.success);
        let b64 = out.output.unwrap()["files"][0]["data"].as_str().unwrap().to_string();
        let decoded = BASE64.decode(&b64).unwrap();
        assert_eq!(String::from_utf8(decoded).unwrap(), content);
    }

    // ── Failure path ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn empty_content_returns_unrecoverable_error() {
        let out = TextToFileNode.execute(make_input(json!({ "content": "" }))).await;
        assert!(!out.success);
        let err = out.error.unwrap();
        assert_eq!(err.code, "MISSING_CONTENT");
        assert!(!err.recoverable);
    }

    #[tokio::test]
    async fn missing_content_field_returns_error() {
        let out = TextToFileNode.execute(make_input(json!({ "filename": "test.txt" }))).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "MISSING_CONTENT");
    }

    #[tokio::test]
    async fn null_content_returns_error() {
        let out = TextToFileNode.execute(make_input(json!({ "content": null }))).await;
        assert!(!out.success);
        assert_eq!(out.error.unwrap().code, "MISSING_CONTENT");
    }

    // ── Defaults ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn default_filename_is_output_txt() {
        let out = TextToFileNode.execute(make_input(json!({ "content": "x" }))).await;
        assert!(out.success);
        let fname = out.output.unwrap()["files"][0]["filename"].as_str().unwrap().to_string();
        assert_eq!(fname, "output.txt");
    }

    #[tokio::test]
    async fn default_mime_type_is_text_plain() {
        let out = TextToFileNode.execute(make_input(json!({ "content": "x" }))).await;
        assert!(out.success);
        let mime = out.output.unwrap()["files"][0]["mime_type"].as_str().unwrap().to_string();
        assert_eq!(mime, "text/plain");
    }

    // ── Explicit overrides ──────────────────────────────────────────────────

    #[tokio::test]
    async fn explicit_filename_used() {
        let out = TextToFileNode.execute(make_input(json!({
            "content": "x",
            "filename": "report.csv"
        }))).await;
        assert!(out.success);
        assert_eq!(out.output.unwrap()["files"][0]["filename"], json!("report.csv"));
    }

    #[tokio::test]
    async fn explicit_mime_type_used() {
        let out = TextToFileNode.execute(make_input(json!({
            "content": "x",
            "mime_type": "text/csv"
        }))).await;
        assert!(out.success);
        assert_eq!(out.output.unwrap()["files"][0]["mime_type"], json!("text/csv"));
    }

    // ── Format field ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn explicit_format_sets_default_filename_extension() {
        // No explicit filename given — format=md → output.md
        let out = TextToFileNode.execute(make_input(json!({
            "content": "# heading",
            "format": "md"
        }))).await;
        assert!(out.success);
        let fname = out.output.unwrap()["files"][0]["filename"].as_str().unwrap().to_string();
        assert_eq!(fname, "output.md");
    }

    #[tokio::test]
    async fn format_inferred_from_filename_extension() {
        // filename=notes.csv, no explicit format → format inferred as "csv"
        let out = TextToFileNode.execute(make_input(json!({
            "content": "a,b,c",
            "filename": "notes.csv"
        }))).await;
        assert!(out.success);
        // mime_type defaults to text/plain (csv uses passthrough); data is base64 of text
        let data = out.output.unwrap();
        let b64 = data["files"][0]["data"].as_str().unwrap();
        let decoded = String::from_utf8(BASE64.decode(b64).unwrap()).unwrap();
        assert_eq!(decoded, "a,b,c");
    }

    // ── Output structure ────────────────────────────────────────────────────

    #[tokio::test]
    async fn output_files_array_has_exactly_one_entry() {
        let out = TextToFileNode.execute(make_input(json!({ "content": "hello" }))).await;
        assert!(out.success);
        let files = out.output.unwrap()["files"].as_array().unwrap().clone();
        assert_eq!(files.len(), 1);
        // All required keys present
        assert!(files[0].get("filename").is_some());
        assert!(files[0].get("data").is_some());
        assert!(files[0].get("mime_type").is_some());
    }
}
