"""
Tests for AI Agent extraction and rule-based fallback.
"""

from academic_agent.agent import (
    analyze_document_content,
    fallback_rule_based_extraction,
    _clean_json_response
)


def test_clean_json_response():
    markdown_wrapped = '```json\n{"title": "Tugas 1", "course": "Basis Data"}\n```'
    parsed = _clean_json_response(markdown_wrapped)
    assert parsed is not None
    assert parsed["title"] == "Tugas 1"
    assert parsed["course"] == "Basis Data"


def test_fallback_rule_based_extraction():
    sample_text = """
    Tugas Mata Kuliah: Basis Data Lanjut
    Deadline: 2026-11-29 23:59
    Buat normalisasi database sampai bentuk 3NF dan BCNF.
    """
    res = fallback_rule_based_extraction(sample_text, filename="tugas_basis_data.pdf")
    assert res["is_academic"] is True
    assert res["kind"] == "assignment"
    assert "Basis Data" in res["course"]
    assert res["deadline"] == "2026-11-29 23:59"
    assert len(res["subtasks"]) >= 3
    assert res["priority"] == "HIGH"


def test_analyze_document_with_fallback():
    sample_text = "Pengumuman: Tidak ada perkuliahan pada tanggal 2026-12-01."
    res = analyze_document_content(sample_text, filename="memo.txt", force_fallback=True)
    assert res["is_academic"] is True
    assert res["extractor"] == "heuristic_fallback"
