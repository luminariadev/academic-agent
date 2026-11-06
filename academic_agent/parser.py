"""
parser.py — Document Parser for PDF, DOCX, TXT, and Markdown files.
"""

import os
from pathlib import Path
from typing import Dict, Any


def extract_text_from_pdf(file_path: Path | str) -> str:
    """Extract text from a PDF file using pypdf."""
    from pypdf import PdfReader
    
    path = Path(file_path)
    if not path.exists():
        raise FileNotFoundError(f"File not found: {path}")

    text_parts = []
    reader = PdfReader(str(path))
    for idx, page in enumerate(reader.pages):
        page_text = page.extract_text() or ""
        if page_text.strip():
            text_parts.append(f"--- [Halaman {idx + 1}] ---\n{page_text.strip()}")
    
    return "\n\n".join(text_parts)


def extract_text_from_docx(file_path: Path | str) -> str:
    """Extract text from a DOCX file using python-docx."""
    import docx
    
    path = Path(file_path)
    if not path.exists():
        raise FileNotFoundError(f"File not found: {path}")

    doc = docx.Document(str(path))
    paragraphs = [p.text.strip() for p in doc.paragraphs if p.text.strip()]
    
    # Also extract text from tables
    table_texts = []
    for table in doc.tables:
        for row in table.rows:
            row_data = [cell.text.strip() for cell in row.cells if cell.text.strip()]
            if row_data:
                table_texts.append(" | ".join(row_data))
    
    full_content = []
    if paragraphs:
        full_content.append("\n".join(paragraphs))
    if table_texts:
        full_content.append("--- [Tabel Dokumen] ---\n" + "\n".join(table_texts))
        
    return "\n\n".join(full_content)


def extract_text_from_file(file_path: Path | str) -> Dict[str, Any]:
    """
    Universal extractor supporting .pdf, .docx, .txt, .md.
    Returns metadata and extracted text.
    """
    path = Path(file_path)
    if not path.exists():
        raise FileNotFoundError(f"File not found: {path}")

    ext = path.suffix.lower()
    raw_text = ""
    error = None

    try:
        if ext == ".pdf":
            raw_text = extract_text_from_pdf(path)
        elif ext in [".docx", ".doc"]:
            raw_text = extract_text_from_docx(path)
        elif ext in [".txt", ".md"]:
            with open(path, "r", encoding="utf-8", errors="ignore") as f:
                raw_text = f.read()
        else:
            error = f"Unsupported file extension: {ext}"
    except Exception as exc:
        error = str(exc)

    return {
        "file_name": path.name,
        "file_path": str(path.resolve()),
        "extension": ext,
        "file_size": path.stat().st_size if path.exists() else 0,
        "content": raw_text.strip(),
        "char_count": len(raw_text.strip()),
        "word_count": len(raw_text.split()),
        "error": error,
        "success": bool(raw_text.strip() and not error)
    }
