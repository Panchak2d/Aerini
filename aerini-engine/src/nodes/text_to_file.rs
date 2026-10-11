use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde_json::{json, Value};

use crate::error::NodeError;
use crate::model::{NodeInput, NodeOutput, NodeType};
use crate::node::Node;

pub struct TextToFileNode;

const FORMATS: &[&str] = &["txt", "md", "html", "csv", "json", "pdf", "docx"];

fn default_mime(format: &str) -> &'static str {
    match format {
        "pdf" => "application/pdf",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "md" => "text/markdown",
        "html" => "text/html",
        "csv" => "text/csv",
        "json" => "application/json",
        _ => "text/plain",
    }
}

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
                    "description": "Output filename (default: output.<format>)"
                },
                "mime_type": {
                    "type": "string",
                    "description": "MIME type (default: chosen from the format, e.g. text/csv for csv)"
                },
                "format": {
                    "type": "string",
                    "enum": FORMATS,
                    "description": "Output format. Falls back to the filename extension, then 'txt', when omitted. pdf/docx produce real binary documents (headings rendered from leading #/##/###; docx also renders **bold**/*italic*); other formats pass the text through unchanged. PDF uses a built-in Latin-1 font: other characters (CJK, emoji) are replaced with '?' and reported in the node log."
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

        // An explicit `format` wins; otherwise the filename extension decides, and
        // an unrecognised extension is simply passed through as plain text.
        let explicit_format = input.input["format"].as_str().map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty());
        if let Some(f) = &explicit_format {
            if !FORMATS.contains(&f.as_str()) {
                return NodeOutput::failure(NodeError::unrecoverable(
                    "INVALID_FORMAT",
                    format!("format must be one of {}, got '{}'", FORMATS.join(", "), f),
                ));
            }
        }
        let format_owned: String = explicit_format.unwrap_or_else(|| {
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

        let mut logs: Vec<String> = Vec::new();
        let (data, fallback_mime): (String, &str) = match format_owned.as_str() {
            "pdf" => {
                let replaced = export::unencodable_chars(content);
                if replaced > 0 {
                    logs.push(format!(
                        "{replaced} character(s) outside the PDF's Latin-1 font were replaced with '?'"
                    ));
                }
                (BASE64.encode(export::text_to_pdf_bytes(content)), default_mime("pdf"))
            }
            "docx" => {
                let bytes = match export::text_to_docx_bytes(content) {
                    Ok(b) => b,
                    Err(e) => return NodeOutput::failure(
                        NodeError::unrecoverable("DOCX_BUILD_FAILED", format!("Failed to build DOCX: {e}"))
                    ),
                };
                (BASE64.encode(bytes), default_mime("docx"))
            }
            other => (BASE64.encode(content.as_bytes()), default_mime(other)),
        };
        let mime_type = input.input["mime_type"].as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or(fallback_mime);

        NodeOutput::success_with_logs(
            json!({ "files": [{ "filename": filename, "data": data, "mime_type": mime_type }] }),
            logs,
        )
    }
}

// ---------------------------------------------------------------------------
// PDF / DOCX export
// ---------------------------------------------------------------------------
mod export {
    use pdf_writer::{Content, Finish, Name, Pdf, Rect, Ref, Str};
    use docx_rs::{BreakType, Docx, Paragraph, Run};
    use std::io::Cursor;

    /// Exact mm→pt conversion (72pt / 25.4mm per inch).
    const MM_TO_PT: f64 = 2.834_645_669;

    /// Advance widths in 1/1000 em for WinAnsi codes 32..=255, taken from the
    /// Adobe core-14 AFM metrics of Helvetica and Helvetica-Bold.
    const HELVETICA_WIDTHS: [u16; 224] = [
        278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278,
        556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556,
        1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778,
        667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556,
        333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556,
        556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584, 761,
        556, 556, 222, 556, 333, 1000, 556, 556, 333, 1000, 667, 333, 1000, 556, 611, 556,
        556, 222, 222, 333, 333, 350, 556, 1000, 333, 1000, 500, 333, 944, 556, 500, 667,
        278, 333, 556, 556, 556, 556, 260, 556, 333, 737, 370, 556, 584, 333, 737, 333,
        400, 584, 333, 333, 333, 556, 537, 278, 333, 333, 365, 556, 834, 834, 834, 611,
        667, 667, 667, 667, 667, 667, 1000, 722, 667, 667, 667, 667, 278, 278, 278, 278,
        722, 722, 778, 778, 778, 778, 778, 584, 778, 722, 722, 722, 722, 667, 667, 611,
        556, 556, 556, 556, 556, 556, 889, 500, 556, 556, 556, 556, 278, 278, 278, 278,
        556, 556, 556, 556, 556, 556, 556, 584, 611, 556, 556, 556, 556, 500, 556, 500,
    ];

    const HELVETICA_BOLD_WIDTHS: [u16; 224] = [
        278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278,
        556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 333, 333, 584, 584, 584, 611,
        975, 722, 722, 722, 722, 667, 611, 778, 722, 278, 556, 722, 611, 833, 722, 778,
        667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 333, 278, 333, 584, 556,
        333, 556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556, 278, 889, 611, 611,
        611, 611, 389, 556, 333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584, 761,
        556, 611, 278, 556, 500, 1000, 556, 556, 333, 1000, 667, 333, 1000, 611, 611, 611,
        611, 278, 278, 500, 500, 350, 556, 1000, 333, 1000, 556, 333, 944, 611, 500, 667,
        278, 333, 556, 556, 556, 556, 280, 556, 333, 737, 370, 556, 584, 333, 737, 333,
        400, 584, 333, 333, 333, 611, 556, 278, 333, 333, 365, 556, 834, 834, 834, 611,
        722, 722, 722, 722, 722, 722, 1000, 722, 667, 667, 667, 667, 278, 278, 278, 278,
        722, 722, 778, 778, 778, 778, 778, 584, 778, 722, 722, 722, 722, 667, 667, 611,
        556, 556, 556, 556, 556, 556, 889, 556, 556, 556, 556, 556, 278, 278, 278, 278,
        611, 611, 611, 611, 611, 611, 611, 584, 611, 611, 611, 611, 611, 556, 611, 556,
    ];

    fn glyph_width(code: u8, bold: bool) -> f64 {
        let table = if bold { &HELVETICA_BOLD_WIDTHS } else { &HELVETICA_WIDTHS };
        f64::from(table[usize::from(code.max(32)) - 32])
    }

    fn text_width_pt(s: &str, bold: bool, size_pt: f64) -> f64 {
        s.chars().map(|c| glyph_width(encode_char(c), bold)).sum::<f64>() * size_pt / 1000.0
    }

    /// Greedy word wrap by real glyph widths. A word wider than a whole line is
    /// split between characters so nothing runs past the right margin.
    fn wrap_line(line: &str, max_width_pt: f64, size_pt: f64, bold: bool) -> Vec<String> {
        if line.trim().is_empty() {
            return vec![String::new()];
        }
        let space = text_width_pt(" ", bold, size_pt);
        let mut out: Vec<String> = Vec::new();
        let mut current = String::new();
        let mut current_w = 0.0;

        for word in line.split_whitespace() {
            let word_w = text_width_pt(word, bold, size_pt);
            let needed = if current.is_empty() { word_w } else { space + word_w };
            if current_w + needed <= max_width_pt {
                if !current.is_empty() {
                    current.push(' ');
                }
                current.push_str(word);
                current_w += needed;
                continue;
            }
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            if word_w <= max_width_pt {
                current.push_str(word);
                current_w = word_w;
                continue;
            }
            current_w = 0.0;
            for c in word.chars() {
                let w = text_width_pt(c.encode_utf8(&mut [0u8; 4]), bold, size_pt);
                if current_w + w > max_width_pt && !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                    current_w = 0.0;
                }
                current.push(c);
                current_w += w;
            }
        }
        if !current.is_empty() || out.is_empty() {
            out.push(current);
        }
        out
    }

    struct Span {
        text: String,
        bold: bool,
        italic: bool,
    }

    enum Token {
        Text(char),
        Mark { bold: bool, can_open: bool, can_close: bool, raw: &'static str },
    }

    /// Splits one line into styled spans on `**bold**` and `*italic*`. A marker
    /// only counts when it is flanked like markdown requires (an opener is
    /// followed by a non-space, a closer preceded by one) and a partner exists
    /// later in the line, so `2 * 3 * 4` and `*.txt` stay literal text.
    fn inline_spans(line: &str) -> Vec<Span> {
        let chars: Vec<char> = line.chars().collect();
        let mut tokens: Vec<Token> = Vec::with_capacity(chars.len());
        let mut i = 0;
        while i < chars.len() {
            if chars[i] != '*' {
                tokens.push(Token::Text(chars[i]));
                i += 1;
                continue;
            }
            let mut run = 0;
            while i + run < chars.len() && chars[i + run] == '*' {
                run += 1;
            }
            let mut consumed = 0;
            while consumed < run {
                let width = if run - consumed >= 2 { 2 } else { 1 };
                let before = if consumed == 0 { i.checked_sub(1).map(|k| chars[k]) } else { Some('*') };
                let after = chars.get(i + consumed + width).copied();
                tokens.push(Token::Mark {
                    bold: width == 2,
                    can_open: after.is_some_and(|c| !c.is_whitespace()),
                    can_close: before.is_some_and(|c| !c.is_whitespace()),
                    raw: if width == 2 { "**" } else { "*" },
                });
                consumed += width;
            }
            i += run;
        }

        let mut closer_ahead = vec![[false; 2]; tokens.len() + 1];
        for k in (0..tokens.len()).rev() {
            closer_ahead[k] = closer_ahead[k + 1];
            if let Token::Mark { bold, can_close: true, .. } = tokens[k] {
                closer_ahead[k][usize::from(bold)] = true;
            }
        }

        let mut spans: Vec<Span> = Vec::new();
        let mut buf = String::new();
        let (mut bold_on, mut italic_on) = (false, false);
        let flush = |buf: &mut String, bold: bool, italic: bool, spans: &mut Vec<Span>| {
            if !buf.is_empty() {
                spans.push(Span { text: std::mem::take(buf), bold, italic });
            }
        };
        for (k, token) in tokens.iter().enumerate() {
            match *token {
                Token::Text(c) => buf.push(c),
                Token::Mark { bold, can_open, can_close, raw } => {
                    let active = if bold { &mut bold_on } else { &mut italic_on };
                    if *active && can_close {
                        flush(&mut buf, bold_on, italic_on, &mut spans);
                        if bold { bold_on = false } else { italic_on = false }
                    } else if !*active && can_open && closer_ahead[k + 1][usize::from(bold)] {
                        flush(&mut buf, bold_on, italic_on, &mut spans);
                        if bold { bold_on = true } else { italic_on = true }
                    } else {
                        buf.push_str(raw);
                    }
                }
            }
        }
        flush(&mut buf, bold_on, italic_on, &mut spans);
        spans
    }

    fn plain_text(line: &str) -> String {
        inline_spans(line).into_iter().map(|s| s.text).collect()
    }

    /// Detects a leading markdown heading marker (#, ##, ###) and strips
    /// matched emphasis markers from the remaining text. Returns
    /// (heading_level, text); heading_level is 0 for body text.
    fn classify_line(raw: &str) -> (usize, String) {
        let trimmed = raw.trim_start();
        for (prefix, level) in [("### ", 3usize), ("## ", 2), ("# ", 1)] {
            if let Some(rest) = trimmed.strip_prefix(prefix) {
                return (level, plain_text(rest));
            }
        }
        (0, plain_text(raw))
    }

    /// Maps a char to its byte under /Encoding /WinAnsiEncoding (Windows-1252):
    /// ASCII and Latin-1 as-is, plus the 0x80-0x9F punctuation block (smart
    /// quotes, dashes, ellipsis, euro) that AI-generated markdown commonly
    /// contains. Anything the standard non-embedded fonts cannot show is '?'.
    fn encode_char(c: char) -> u8 {
        match c {
            '\u{20}'..='\u{7E}' => c as u8,
            '\u{20AC}' => 0x80,
            '\u{201A}' => 0x82,
            '\u{0192}' => 0x83,
            '\u{201E}' => 0x84,
            '\u{2026}' => 0x85,
            '\u{2020}' => 0x86,
            '\u{2021}' => 0x87,
            '\u{02C6}' => 0x88,
            '\u{2030}' => 0x89,
            '\u{0160}' => 0x8A,
            '\u{2039}' => 0x8B,
            '\u{0152}' => 0x8C,
            '\u{017D}' => 0x8E,
            '\u{2018}' => 0x91,
            '\u{2019}' => 0x92,
            '\u{201C}' => 0x93,
            '\u{201D}' => 0x94,
            '\u{2022}' => 0x95,
            '\u{2013}' => 0x96,
            '\u{2014}' => 0x97,
            '\u{02DC}' => 0x98,
            '\u{2122}' => 0x99,
            '\u{0161}' => 0x9A,
            '\u{203A}' => 0x9B,
            '\u{0153}' => 0x9C,
            '\u{017E}' => 0x9E,
            '\u{0178}' => 0x9F,
            '\u{00A0}'..='\u{00FF}' => c as u8,
            _ => b'?',
        }
    }

    fn encode_winansi(s: &str) -> Vec<u8> {
        s.chars().map(encode_char).collect()
    }

    /// Number of visible characters the PDF font cannot show (whitespace is
    /// laid out, not drawn, so it never counts).
    pub fn unencodable_chars(content: &str) -> usize {
        content
            .chars()
            .filter(|c| !c.is_whitespace() && encode_char(*c) == b'?' && *c != '?')
            .count()
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

        // The page count is not known up front, so object ids for pages and
        // content streams are handed out lazily as each page is opened.
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
            let (font_name, size_pt, line_h_mm, bold) = if level > 0 {
                (font_bold_name, HEADING_SIZE_PT[level], HEADING_LINE_HEIGHT_MM[level], true)
            } else {
                (font_regular_name, BODY_SIZE_PT, BODY_LINE_HEIGHT_MM, false)
            };

            let wrapped = wrap_line(&text, usable_width_pt, size_pt, bold);

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
                    // text line matrix to identity.
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

    /// Drops characters that XML 1.0 forbids; one of them in a run makes Word
    /// refuse to open the whole document.
    fn xml_safe(s: &str) -> String {
        s.chars()
            .filter(|&c| matches!(c, '\t' | '\n' | '\r') || (c >= ' ' && c != '\u{FFFE}' && c != '\u{FFFF}'))
            .collect()
    }

    fn runs_for(line: &str) -> Vec<Run> {
        inline_spans(line)
            .into_iter()
            .map(|span| {
                let mut run = Run::new().add_text(span.text);
                if span.bold {
                    run = run.bold();
                }
                if span.italic {
                    run = run.italic();
                }
                run
            })
            .collect()
    }

    /// Renders plain text to a DOCX. Paragraphs are split on blank lines
    /// (`\n\n`); a leading #/##/### marks a heading (bold, larger size);
    /// `**bold**` / `*italic*` are rendered as real DOCX run formatting.
    pub fn text_to_docx_bytes(content: &str) -> Result<Vec<u8>, String> {
        let content = xml_safe(&content.replace("\r\n", "\n"));
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
                // Whole-paragraph styling, no inline runs: further lines in the
                // same blank-line-delimited chunk are joined into the heading.
                let mut heading_text = first_text;
                for rest in lines {
                    heading_text.push(' ');
                    heading_text.push_str(&plain_text(rest.trim_start()));
                }
                let size_half_pt: usize = match level { 1 => 36, 2 => 30, _ => 26 };
                let run = Run::new().add_text(heading_text).bold().size(size_half_pt);
                docx = docx.add_paragraph(Paragraph::new().add_run(run));
                continue;
            }

            let body_lines: Vec<&str> = raw_para.lines().collect();
            let mut paragraph = Paragraph::new();
            for (i, line) in body_lines.iter().enumerate() {
                for run in runs_for(line) {
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

    #[cfg(test)]
    mod tests {
        use super::*;

        fn render(line: &str) -> String {
            inline_spans(line)
                .into_iter()
                .map(|s| match (s.bold, s.italic) {
                    (true, true) => format!("<b><i>{}</i></b>", s.text),
                    (true, false) => format!("<b>{}</b>", s.text),
                    (false, true) => format!("<i>{}</i>", s.text),
                    (false, false) => s.text,
                })
                .collect()
        }

        #[test]
        fn inline_markers_only_apply_when_matched_and_flanked() {
            let cases = [
                ("plain", "plain"),
                ("**bold** and *it*", "<b>bold</b> and <i>it</i>"),
                ("***both***", "<b><i>both</i></b>"),
                ("2 * 3 * 4", "2 * 3 * 4"),
                ("match *.txt and *.md", "match *.txt and *.md"),
                ("**unclosed", "**unclosed"),
                ("a*b*c", "a<i>b</i>c"),
                ("**bold *nested* bold**", "<b>bold </b><b><i>nested</i></b><b> bold</b>"),
            ];
            for (input, expected) in cases {
                assert_eq!(render(input), expected, "input: {input:?}");
            }
        }

        #[test]
        fn pdf_lines_never_exceed_the_text_width_even_for_wide_glyphs_and_long_words() {
            let max = 400.0;
            let caps = "WORLD ".repeat(60);
            let long_word = "M".repeat(300);
            for text in [caps.as_str(), long_word.as_str()] {
                let lines = wrap_line(text, max, 11.0, false);
                assert!(lines.len() > 1);
                for line in &lines {
                    assert!(text_width_pt(line, false, 11.0) <= max + 0.001, "overflowing line: {line}");
                }
            }
            let rejoined: String = wrap_line(&long_word, max, 11.0, false).concat();
            assert_eq!(rejoined, long_word, "splitting a word must not drop characters");
        }
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
            resolved_credentials: std::collections::HashMap::new(),
            cancel_token: None,
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

    // ── format, MIME, markdown handling ─────────────────────────────────────

    #[tokio::test]
    async fn default_mime_follows_the_format_and_unknown_formats_are_rejected() {
        for (format, mime) in [("csv", "text/csv"), ("json", "application/json"), ("html", "text/html"), ("md", "text/markdown"), ("txt", "text/plain")] {
            let out = TextToFileNode.execute(make_input(json!({ "content": "x", "format": format }))).await;
            assert_eq!(out.output.unwrap()["files"][0]["mime_type"], json!(mime), "format {format}");
        }
        let out = TextToFileNode.execute(make_input(json!({ "content": "x", "format": "exe" }))).await;
        assert_eq!(out.error.unwrap().code, "INVALID_FORMAT");

        let inferred = TextToFileNode.execute(make_input(json!({ "content": "x", "filename": "data.weird" }))).await;
        assert!(inferred.success, "an unknown filename extension is just plain text");
    }

    #[tokio::test]
    async fn pdf_reports_characters_the_font_cannot_show() {
        let out = TextToFileNode
            .execute(make_input(json!({ "content": "Cafe \u{e9} \u{2014} \u{4f60}\u{597d} \u{1f600}", "format": "pdf" })))
            .await;
        assert!(out.success, "{:?}", out.error);
        assert!(out.logs.iter().any(|l| l.starts_with("3 character(s)")), "logs: {:?}", out.logs);
        let data = out.output.unwrap()["files"][0]["data"].as_str().unwrap().to_string();
        assert!(BASE64.decode(data).unwrap().starts_with(b"%PDF-"));
    }

    #[tokio::test]
    async fn docx_survives_control_characters_that_xml_forbids() {
        let out = TextToFileNode
            .execute(make_input(json!({ "content": "# Title\n\nbad\u{0}char\u{b} and **bold**", "format": "docx" })))
            .await;
        assert!(out.success, "{:?}", out.error);
        let data = out.output.unwrap()["files"][0]["data"].as_str().unwrap().to_string();
        assert!(BASE64.decode(data).unwrap().starts_with(b"PK"));
    }
}
