//! Minimal write-only WordprocessingML (.docx) writer.
//!
//! This module can only *produce* an archive: it contains no ZIP or XML
//! parser, so it has no "parsing untrusted input" attack surface by
//! construction.
//!
//! Output is a small OPC package (content types, relationships, document,
//! styles, settings) with A4 page geometry.
//! `settings.xml` pins `compatibilityMode` 15; without it Word opens the file
//! in "Compatibility Mode".
//! Well-formed XML is guaranteed here regardless of caller input: characters
//! XML 1.0 forbids are dropped, markup characters are escaped, and tabs and
//! line breaks become `<w:tab/>` / `<w:br/>`.

use std::fmt::Write as _;
use std::io::{Cursor, Write};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const CONTENT_TYPES_XML: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
    r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">"#,
    r#"<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>"#,
    r#"<Default Extension="xml" ContentType="application/xml"/>"#,
    r#"<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>"#,
    r#"<Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>"#,
    r#"<Override PartName="/word/settings.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"/>"#,
    r#"</Types>"#,
);

const ROOT_RELS_XML: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
    r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    r#"<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>"#,
    r#"</Relationships>"#,
);

const DOCUMENT_RELS_XML: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
    r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    r#"<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>"#,
    r#"<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings" Target="settings.xml"/>"#,
    r#"</Relationships>"#,
);

/// Empty defaults plus a Normal style: no font or size is imposed, so Word
/// applies its own defaults.
const STYLES_XML: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
    r#"<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">"#,
    r#"<w:docDefaults><w:rPrDefault><w:rPr/></w:rPrDefault><w:pPrDefault><w:pPr/></w:pPrDefault></w:docDefaults>"#,
    r#"<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style>"#,
    r#"</w:styles>"#,
);

const SETTINGS_XML: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
    r#"<w:settings xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">"#,
    r#"<w:defaultTabStop w:val="840"/>"#,
    r#"<w:compat><w:compatSetting w:name="compatibilityMode" w:uri="http://schemas.microsoft.com/office/word" w:val="15"/></w:compat>"#,
    r#"</w:settings>"#,
);

const DOCUMENT_OPEN_XML: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
    r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>"#,
);

/// A4 page; margins are in twentieths of a point.
const DOCUMENT_CLOSE_XML: &str = concat!(
    r#"<w:sectPr><w:pgSz w:w="11906" w:h="16838"/>"#,
    r#"<w:pgMar w:top="1985" w:right="1701" w:bottom="1701" w:left="1701" w:header="851" w:footer="992" w:gutter="0"/>"#,
    r#"<w:cols w:space="425" w:num="1"/></w:sectPr></w:body></w:document>"#,
);

/// A run of text sharing one set of character formatting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextRun {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    /// Font size in half-points (`36` = 18 pt); `None` inherits the default.
    pub size_half_pt: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    Text(TextRun),
    LineBreak,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Paragraph {
    pub inlines: Vec<Inline>,
}

/// Serialises `paragraphs` into a complete `.docx` archive.
pub fn build(paragraphs: &[Paragraph]) -> Result<Vec<u8>, String> {
    let document_xml = document_xml(paragraphs);

    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    // Without zip's `time` feature the entry timestamp is the fixed DOS epoch,
    // so identical input yields byte-identical output.
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    for (name, body) in [
        ("[Content_Types].xml", CONTENT_TYPES_XML),
        ("_rels/.rels", ROOT_RELS_XML),
        ("word/document.xml", document_xml.as_str()),
        ("word/_rels/document.xml.rels", DOCUMENT_RELS_XML),
        ("word/styles.xml", STYLES_XML),
        ("word/settings.xml", SETTINGS_XML),
    ] {
        zip.start_file(name, options)
            .map_err(|e| format!("docx: cannot start part '{name}': {e}"))?;
        zip.write_all(body.as_bytes())
            .map_err(|e| format!("docx: cannot write part '{name}': {e}"))?;
    }

    let cursor = zip.finish().map_err(|e| format!("docx: cannot finalise archive: {e}"))?;
    Ok(cursor.into_inner())
}

fn document_xml(paragraphs: &[Paragraph]) -> String {
    let mut xml = String::from(DOCUMENT_OPEN_XML);
    for paragraph in paragraphs {
        write_paragraph(&mut xml, paragraph);
    }
    xml.push_str(DOCUMENT_CLOSE_XML);
    xml
}

fn write_paragraph(xml: &mut String, paragraph: &Paragraph) {
    xml.push_str("<w:p>");
    for inline in &paragraph.inlines {
        match inline {
            Inline::Text(run) => write_run(xml, run),
            Inline::LineBreak => xml.push_str("<w:r><w:br/></w:r>"),
        }
    }
    xml.push_str("</w:p>");
}

fn write_run(xml: &mut String, run: &TextRun) {
    let body = run_body(&run.text);
    if body.is_empty() {
        return;
    }
    xml.push_str("<w:r>");
    if run.bold || run.italic || run.size_half_pt.is_some() {
        // Child order follows the CT_RPr schema sequence: b, bCs, i, iCs, sz, szCs.
        xml.push_str("<w:rPr>");
        if run.bold {
            xml.push_str("<w:b/><w:bCs/>");
        }
        if run.italic {
            xml.push_str("<w:i/><w:iCs/>");
        }
        if let Some(size) = run.size_half_pt {
            // Writing to a String cannot fail.
            let _ = write!(xml, r#"<w:sz w:val="{size}"/><w:szCs w:val="{size}"/>"#);
        }
        xml.push_str("</w:rPr>");
    }
    xml.push_str(&body);
    xml.push_str("</w:r>");
}

/// Converts text into the child elements of a `<w:r>`: `<w:t>` segments with
/// `<w:tab/>` and `<w:br/>` where the source has tabs and line breaks.
fn run_body(text: &str) -> String {
    // Lone CR (classic Mac line ending) is a line break, same as CRLF.
    let normalised = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut body = String::new();
    let mut segment = String::new();

    let flush = |segment: &mut String, body: &mut String| {
        if !segment.is_empty() {
            body.push_str(r#"<w:t xml:space="preserve">"#);
            body.push_str(segment);
            body.push_str("</w:t>");
            segment.clear();
        }
    };

    for c in normalised.chars() {
        match c {
            '\t' => {
                flush(&mut segment, &mut body);
                body.push_str("<w:tab/>");
            }
            '\n' => {
                flush(&mut segment, &mut body);
                body.push_str("<w:br/>");
            }
            '&' => segment.push_str("&amp;"),
            '<' => segment.push_str("&lt;"),
            '>' => segment.push_str("&gt;"),
            c if is_xml_char(c) => segment.push(c),
            _ => {} // forbidden by XML 1.0: one such char makes Word reject the file
        }
    }
    flush(&mut segment, &mut body);
    body
}

/// XML 1.0 `Char` production, minus tab/CR/LF which the caller handles.
fn is_xml_char(c: char) -> bool {
    matches!(c, '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn text(s: &str) -> Inline {
        Inline::Text(TextRun { text: s.into(), bold: false, italic: false, size_half_pt: None })
    }

    fn read_part(docx: &[u8], name: &str) -> String {
        let mut archive = zip::ZipArchive::new(Cursor::new(docx)).expect("valid zip");
        let mut part = archive.by_name(name).unwrap_or_else(|_| panic!("missing part {name}"));
        let mut out = String::new();
        part.read_to_string(&mut out).expect("utf-8 part");
        out
    }

    #[test]
    fn package_contains_exactly_the_expected_parts() {
        let docx = build(&[Paragraph { inlines: vec![text("hi")] }]).unwrap();
        let archive = zip::ZipArchive::new(Cursor::new(&docx[..])).unwrap();
        // zip 9 yields entry names as `Result`s because names can be undecodable.
        let mut names: Vec<String> =
            archive.file_names().map(|n| n.expect("utf-8 entry name").into_owned()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "[Content_Types].xml",
                "_rels/.rels",
                "word/_rels/document.xml.rels",
                "word/document.xml",
                "word/settings.xml",
                "word/styles.xml",
            ]
        );
        assert!(docx.starts_with(b"PK"));
    }

    #[test]
    fn every_part_is_declared_and_related() {
        let docx = build(&[]).unwrap();
        let types = read_part(&docx, "[Content_Types].xml");
        let rels = read_part(&docx, "word/_rels/document.xml.rels");
        for part in ["/word/document.xml", "/word/styles.xml", "/word/settings.xml"] {
            assert!(types.contains(&format!(r#"PartName="{part}""#)), "{part} undeclared: {types}");
        }
        for target in ["styles.xml", "settings.xml"] {
            assert!(rels.contains(&format!(r#"Target="{target}""#)), "{target} unrelated: {rels}");
        }
        assert!(read_part(&docx, "word/settings.xml").contains(r#"w:name="compatibilityMode""#));
    }

    #[test]
    fn empty_document_is_still_a_valid_package() {
        let docx = build(&[]).unwrap();
        let xml = read_part(&docx, "word/document.xml");
        assert!(xml.contains("<w:body><w:sectPr>"), "{xml}");
    }

    #[test]
    fn markup_characters_are_escaped() {
        let docx = build(&[Paragraph { inlines: vec![text("a < b && c > d \"q\" 'x'")] }]).unwrap();
        let xml = read_part(&docx, "word/document.xml");
        assert!(xml.contains("a &lt; b &amp;&amp; c &gt; d \"q\" 'x'"), "{xml}");
    }

    #[test]
    fn xml_forbidden_characters_are_dropped() {
        let docx = build(&[Paragraph { inlines: vec![text("bad\u{0}\u{b}\u{1f}\u{fffe}\u{ffff}char")] }]).unwrap();
        let xml = read_part(&docx, "word/document.xml");
        assert!(xml.contains(">badchar<"), "{xml}");
    }

    #[test]
    fn non_bmp_characters_survive() {
        let docx = build(&[Paragraph { inlines: vec![text("smile \u{1f600}")] }]).unwrap();
        assert!(read_part(&docx, "word/document.xml").contains("smile \u{1f600}"));
    }

    #[test]
    fn tabs_and_all_line_endings_become_elements() {
        let docx = build(&[Paragraph { inlines: vec![text("a\tb\r\nc\rd\ne")] }]).unwrap();
        let xml = read_part(&docx, "word/document.xml");
        assert_eq!(xml.matches("<w:tab/>").count(), 1, "{xml}");
        assert_eq!(xml.matches("<w:br/>").count(), 3, "{xml}");
        assert!(!xml.contains('\r'));
    }

    #[test]
    fn whitespace_is_preserved() {
        let docx = build(&[Paragraph { inlines: vec![text("  padded  ")] }]).unwrap();
        assert!(read_part(&docx, "word/document.xml").contains(r#"<w:t xml:space="preserve">  padded  </w:t>"#));
    }

    #[test]
    fn run_formatting_uses_schema_order() {
        let run = TextRun { text: "x".into(), bold: true, italic: true, size_half_pt: Some(36) };
        let docx = build(&[Paragraph { inlines: vec![Inline::Text(run)] }]).unwrap();
        let xml = read_part(&docx, "word/document.xml");
        assert!(
            xml.contains(r#"<w:rPr><w:b/><w:bCs/><w:i/><w:iCs/><w:sz w:val="36"/><w:szCs w:val="36"/></w:rPr>"#),
            "{xml}"
        );
    }

    #[test]
    fn empty_text_run_emits_nothing() {
        let docx = build(&[Paragraph { inlines: vec![text("")] }]).unwrap();
        assert!(read_part(&docx, "word/document.xml").contains("<w:p></w:p>"));
    }

    #[test]
    fn explicit_line_break_inline_is_emitted() {
        let docx = build(&[Paragraph { inlines: vec![text("a"), Inline::LineBreak, text("b")] }]).unwrap();
        assert!(read_part(&docx, "word/document.xml").contains("</w:r><w:r><w:br/></w:r><w:r>"));
    }

    #[test]
    fn output_is_deterministic() {
        let paragraphs = [Paragraph { inlines: vec![text("same")] }];
        assert_eq!(build(&paragraphs).unwrap(), build(&paragraphs).unwrap());
    }
}
