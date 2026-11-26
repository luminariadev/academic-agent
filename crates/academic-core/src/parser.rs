use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use quick_xml::events::Event;
use quick_xml::Reader;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

/// Metadata and extracted text from a local academic document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedDocument {
    pub file_name: String,
    pub extension: String,
    pub file_size: u64,
    pub content: String,
    pub content_hash: String,
    pub char_count: usize,
    pub word_count: usize,
}

/// Validates a bare filename and rejects path traversal, directory separators, drive prefixes,
/// null bytes, and leading-dot hidden names.
pub fn sanitize_filename(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(Error::InvalidPath("filename must not be empty".to_string()));
    }
    if trimmed.len() > 180 {
        return Err(Error::InvalidPath(
            "filename exceeds 180 characters".to_string(),
        ));
    }
    if trimmed.chars().any(|c| c == '\0' || (c as u32) < 32) {
        return Err(Error::InvalidPath(
            "filename contains control characters".to_string(),
        ));
    }
    if trimmed.contains('/') || trimmed.contains('\\') || trimmed.contains(':') {
        return Err(Error::InvalidPath(format!(
            "filename '{trimmed}' must not contain directory separators or drive letters"
        )));
    }
    if trimmed == "." || trimmed == ".." || trimmed.contains("..") || trimmed.starts_with('.') {
        return Err(Error::InvalidPath(format!(
            "filename '{trimmed}' is not permitted"
        )));
    }

    let path = Path::new(trimmed);
    let mut comps = path.components();
    match (comps.next(), comps.next()) {
        (Some(Component::Normal(name)), None) => {
            let valid = name.to_string_lossy().into_owned();
            if valid.trim_end_matches(['.', ' ']) != valid {
                return Err(Error::InvalidPath(format!(
                    "filename '{trimmed}' has invalid trailing dots or spaces"
                )));
            }
            Ok(valid)
        }
        _ => Err(Error::InvalidPath(format!(
            "filename '{trimmed}' is not a single path component"
        ))),
    }
}

/// Safely joins `filename` inside `base_dir`, guaranteeing no traversal outside `base_dir`.
pub fn safe_join_within(base_dir: &Path, filename: &str) -> Result<PathBuf> {
    let clean = sanitize_filename(filename)?;
    std::fs::create_dir_all(base_dir)?;
    let canon_base = base_dir
        .canonicalize()
        .unwrap_or_else(|_| base_dir.to_path_buf());
    let joined = canon_base.join(&clean);
    if joined.exists() {
        let canon_target = joined.canonicalize()?;
        if !canon_target.starts_with(&canon_base) {
            return Err(Error::InvalidPath(format!(
                "path '{}' escapes base directory",
                clean
            )));
        }
    }
    Ok(joined)
}

/// Computes lowercase hex SHA-256 of normalized UTF-8 text.
pub fn sha256_hex(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    hex::encode(hasher.finalize())
}

/// Normalizes line endings and trims extraneous blank spacing.
pub fn normalize_text(raw: &str) -> String {
    let unified = raw.replace("\r\n", "\n").replace('\r', "\n");
    let mut out_lines = Vec::new();
    let mut prev_blank = false;
    for line in unified.lines() {
        let trimmed = line.trim_end();
        if trimmed.trim().is_empty() {
            if !out_lines.is_empty() && !prev_blank {
                out_lines.push("");
                prev_blank = true;
            }
        } else {
            out_lines.push(trimmed);
            prev_blank = false;
        }
    }
    out_lines.join("\n").trim().to_string()
}

/// Extracts UTF-8 text from a `.txt` or `.md` file.
fn parse_plain_text(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Extracts paragraph and table text from `word/document.xml` inside a `.docx` archive.
pub fn extract_text_from_docx_bytes(bytes: &[u8]) -> Result<String> {
    let cursor = std::io::Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor)
        .map_err(|e| Error::Parse(format!("failed to open DOCX archive: {e}")))?;

    let mut xml_str = String::new();
    {
        let mut doc_entry = archive
            .by_name("word/document.xml")
            .map_err(|e| Error::Parse(format!("DOCX missing word/document.xml: {e}")))?;
        doc_entry
            .read_to_string(&mut xml_str)
            .map_err(|e| Error::Parse(format!("failed reading word/document.xml: {e}")))?;
    }

    let mut reader = Reader::from_str(&xml_str);
    reader.config_mut().trim_text(false);

    let mut buf = Vec::new();
    let mut output = String::new();
    let mut in_text = false;
    let mut in_table_row = false;
    let mut first_cell_in_row = true;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let local = e.local_name();
                match local.as_ref() {
                    b"t" => in_text = true,
                    b"tr" => {
                        in_table_row = true;
                        first_cell_in_row = true;
                    }
                    b"tc" => {
                        if in_table_row && !first_cell_in_row && !output.ends_with(" | ") {
                            output.push_str(" | ");
                        }
                        first_cell_in_row = false;
                    }
                    b"tab" => output.push(' '),
                    b"br" | b"cr" => output.push('\n'),
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => {
                let local = e.local_name();
                match local.as_ref() {
                    b"tab" => output.push(' '),
                    b"br" | b"cr" => output.push('\n'),
                    _ => {}
                }
            }
            Ok(Event::Text(e)) => {
                if in_text {
                    let decoded = e
                        .unescape()
                        .map_err(|err| Error::Parse(format!("XML unescape error: {err}")))?;
                    output.push_str(&decoded);
                }
            }
            Ok(Event::End(e)) => {
                let local = e.local_name();
                match local.as_ref() {
                    b"t" => in_text = false,
                    b"p" => {
                        if !in_table_row {
                            output.push('\n');
                        }
                    }
                    b"tr" => {
                        in_table_row = false;
                        output.push('\n');
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(Error::Parse(format!("invalid DOCX XML: {e}"))),
            _ => {}
        }
        buf.clear();
    }

    Ok(output)
}

/// Extracts text from a PDF file using pure-Rust `lopdf`.
fn parse_pdf(path: &Path) -> Result<String> {
    let doc = lopdf::Document::load(path)
        .map_err(|e| Error::Parse(format!("failed to load PDF '{}': {e}", path.display())))?;
    let pages: Vec<u32> = doc.get_pages().keys().copied().collect();
    if pages.is_empty() {
        return Err(Error::Parse("PDF has no pages".to_string()));
    }

    let mut sections = Vec::new();
    for page_num in pages {
        if let Ok(page_text) = doc.extract_text(&[page_num]) {
            let trimmed = page_text.trim();
            if !trimmed.is_empty() {
                sections.push(format!("[Page {page_num}]\n{trimmed}"));
            }
        }
    }

    if sections.is_empty() {
        return Err(Error::Parse(
            "PDF contains no extractable text; scanned image PDFs require OCR".to_string(),
        ));
    }

    Ok(sections.join("\n\n"))
}

/// Parses a local `.txt`, `.md`, `.docx`, or `.pdf` document into normalized text and metadata.
pub fn parse_document(path: &Path) -> Result<ParsedDocument> {
    if !path.exists() {
        return Err(Error::NotFound(format!(
            "document not found: '{}'",
            path.display()
        )));
    }
    let meta = std::fs::metadata(path)?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| format!(".{}", s.to_lowercase()))
        .unwrap_or_default();

    let raw_text = match ext.as_str() {
        ".txt" | ".md" => parse_plain_text(path)?,
        ".docx" => {
            let mut buf = Vec::new();
            File::open(path)?.read_to_end(&mut buf)?;
            extract_text_from_docx_bytes(&buf)?
        }
        ".pdf" => parse_pdf(path)?,
        ".doc" => {
            return Err(Error::UnsupportedFormat(
                "legacy binary .doc files are unsupported; convert to .docx or .pdf".to_string(),
            ));
        }
        other => {
            return Err(Error::UnsupportedFormat(format!(
                "unsupported document extension '{other}'"
            )));
        }
    };

    let content = normalize_text(&raw_text);
    if content.is_empty() {
        return Err(Error::Parse(format!(
            "document '{}' produced empty text after normalization",
            path.display()
        )));
    }

    let content_hash = sha256_hex(&content);
    let char_count = content.chars().count();
    let word_count = content.split_whitespace().count();
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document".to_string());

    Ok(ParsedDocument {
        file_name,
        extension: ext,
        file_size: meta.len(),
        content,
        content_hash,
        char_count,
        word_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;
    use zip::write::SimpleFileOptions;

    #[test]
    fn test_sanitize_filename_blocks_traversal() {
        assert_eq!(sanitize_filename("tugas.pdf").unwrap(), "tugas.pdf");
        assert!(sanitize_filename("../secret.txt").is_err());
        assert!(sanitize_filename("dir/file.txt").is_err());
        assert!(sanitize_filename("dir\\file.txt").is_err());
        assert!(sanitize_filename("C:secret.txt").is_err());
        assert!(sanitize_filename(".env").is_err());
        assert!(sanitize_filename("").is_err());

        let dir = tempdir().unwrap();
        assert!(safe_join_within(dir.path(), "../../etc/passwd").is_err());
        let ok = safe_join_within(dir.path(), "valid_event.ics").unwrap();
        assert!(ok.ends_with("valid_event.ics"));
    }

    #[test]
    fn test_parse_docx_and_reject_doc() {
        let dir = tempdir().unwrap();
        let docx_path = dir.path().join("syllabus.docx");
        {
            let file = File::create(&docx_path).unwrap();
            let mut zw = zip::ZipWriter::new(file);
            zw.start_file("word/document.xml", SimpleFileOptions::default())
                .unwrap();
            let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:r><w:t>Course: Distributed Systems</w:t></w:r></w:p>
    <w:tbl>
      <w:tr>
        <w:tc><w:p><w:r><w:t>Assignment 1</w:t></w:r></w:p></w:tc>
        <w:tc><w:p><w:r><w:t>2026-12-10 23:59</w:t></w:r></w:p></w:tc>
      </w:tr>
    </w:tbl>
  </w:body>
</w:document>"#;
            zw.write_all(xml.as_bytes()).unwrap();
            zw.finish().unwrap();
        }

        let parsed = parse_document(&docx_path).expect("docx parsed");
        assert!(parsed.content.contains("Course: Distributed Systems"));
        assert!(parsed.content.contains("Assignment 1 | 2026-12-10 23:59"));

        let legacy_doc = dir.path().join("old.doc");
        std::fs::write(&legacy_doc, b"binary").unwrap();
        let err = parse_document(&legacy_doc).unwrap_err();
        assert!(matches!(err, Error::UnsupportedFormat(_)));
    }

    #[test]
    fn test_parse_generated_pdf() {
        use lopdf::content::{Content, Operation};
        use lopdf::{dictionary, Document, Object, Stream};

        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
        });
        let resources_id = doc.add_object(dictionary! {
            "Font" => dictionary! { "F1" => font_id },
        });
        let content = Content {
            operations: vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F1".into(), 12.into()]),
                Operation::new("Td", vec![50.into(), 700.into()]),
                Operation::new(
                    "Tj",
                    vec![Object::string_literal("Tugas: Research Methods")],
                ),
                Operation::new("ET", vec![]),
            ],
        };
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
            "Resources" => resources_id,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        });
        let pages = dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1,
        };
        doc.objects.insert(pages_id, Object::Dictionary(pages));
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);

        let dir = tempdir().unwrap();
        let pdf_path = dir.path().join("assignment.pdf");
        doc.save(&pdf_path).unwrap();

        let parsed = parse_document(&pdf_path).expect("pdf extracted");
        assert!(parsed.content.contains("Research Methods"));
    }
}
