"""
Tests for document parser module.
"""

from pathlib import Path
import pytest
from academic_agent.parser import extract_text_from_file, extract_text_from_pdf, extract_text_from_docx

SAMPLE_DIR = Path(__file__).resolve().parent.parent / "sample_docs"


def test_extract_pdf():
    pdf_path = SAMPLE_DIR / "tugas_metode_penelitian.pdf"
    assert pdf_path.exists(), "Sample PDF must exist"
    res = extract_text_from_file(pdf_path)
    assert res["success"] is True
    assert "Metode Penelitian" in res["content"]
    assert res["extension"] == ".pdf"
    assert res["word_count"] > 10


def test_extract_docx():
    docx_path = SAMPLE_DIR / "tugas_kecerdasan_buatan.docx"
    assert docx_path.exists(), "Sample DOCX must exist"
    res = extract_text_from_file(docx_path)
    assert res["success"] is True
    assert "Kecerdasan Buatan" in res["content"]
    assert "Random Forest" in res["content"]
    assert res["extension"] == ".docx"


def test_extract_txt():
    txt_path = SAMPLE_DIR / "pengumuman_kuis.txt"
    assert txt_path.exists(), "Sample TXT must exist"
    res = extract_text_from_file(txt_path)
    assert res["success"] is True
    assert "Jaringan Komputer" in res["content"]
    assert res["extension"] == ".txt"


def test_extract_nonexistent_file():
    with pytest.raises(FileNotFoundError):
        extract_text_from_file(SAMPLE_DIR / "nonexistent_file.pdf")
