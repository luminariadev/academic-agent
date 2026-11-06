"""
agent.py — AI Academic Agent powered by 9Router / Hermes / OpenAI-compatible API.
"""

import json
import logging
import re
from datetime import datetime, timedelta
from typing import Dict, Any, Optional

import requests

from academic_agent.config import ROUTER_API_BASE, ROUTER_API_KEY, ROUTER_MODEL

logger = logging.getLogger(__name__)

SYSTEM_PROMPT = """Kamu adalah "Akademik Agent", asisten AI mahasiswa tingkat tinggi yang bertugas mengekstrak informasi terstruktur dari silabus, dokumen tugas perkuliahan, atau pengumuman akademik.

Analisis teks yang diberikan dan kembalikan HANYA format JSON valid tanpa tanda markdown (```json ... ```) dengan skema berikut:
{
  "is_academic": true,
  "kind": "assignment" | "syllabus" | "announcement" | "general",
  "course": "Nama Mata Kuliah (contoh: Metode Penelitian, Pemrograman Web)",
  "title": "Judul Tugas yang Jelas dan Spesifik",
  "deadline": "YYYY-MM-DD HH:MM" (jika hanya tanggal, gunakan jam 23:59. Jika tidak ada, gunakan null),
  "estimated_hours": 3,
  "priority": "HIGH" | "MEDIUM" | "LOW",
  "summary": "Ringkasan eksekutif 2-4 kalimat mengenai instruksi tugas atau materi",
  "subtasks": [
    "Langkah kerja 1 yang dapat dicentang",
    "Langkah kerja 2",
    "Langkah kerja 3"
  ],
  "deliverables": [
    "Format file yang diminta (misal: PDF laporan, link GitHub, file .zip)"
  ]
}

Aturan Penting:
1. Prioritaskan ekstraksi tanggal dan waktu pengumpulan (deadline).
2. Jika ada tanggal dalam Bahasa Indonesia (misal: "15 Juli 2026", "Jumat depan"), konversikan ke format ISO YYYY-MM-DD HH:MM.
3. Subtasks harus actionable dan berupa langkah nyata penyelesaian tugas.
4. Output HANYA raw JSON. Jangan tambahkan penjelasan teks di luar JSON.
"""


def _clean_json_response(raw_text: str) -> Optional[Dict[str, Any]]:
    """Clean markdown codeblocks and parse JSON safely."""
    if not raw_text or not raw_text.strip():
        return None

    cleaned = raw_text.strip()
    # Strip markdown block wrappers
    cleaned = re.sub(r"^```(?:json)?\s*", "", cleaned, flags=re.IGNORECASE)
    cleaned = re.sub(r"\s*```$", "", cleaned)

    try:
        return json.loads(cleaned)
    except json.JSONDecodeError:
        # Regex search for the outermost {...}
        match = re.search(r"\{[\s\S]*\}", cleaned)
        if match:
            try:
                return json.loads(match.group(0))
            except json.JSONDecodeError:
                pass
    return None


def fallback_rule_based_extraction(text: str, filename: str = "") -> Dict[str, Any]:
    """
    Fallback deterministic parser using regex and NLP heuristics
    when LLM inference is offline.
    """
    lowered = text.lower()
    
    # Check academic nature
    academic_keywords = [
        "tugas", "deadline", "pengumpulan", "silabus", "mata kuliah",
        "dosen", "ujian", "uts", "uas", "praktikum", "laporan", "jurnal",
        "kuliah", "perkuliahan", "pengumuman", "mahasiswa", "kampus", "akademik"
    ]
    is_academic = any(kw in lowered for kw in academic_keywords)

    # Detect kind
    if any(kw in lowered for kw in ["tugas", "deadline", "kumpulkan", "latihan", "proyek", "project"]):
        kind = "assignment"
        priority = "HIGH"
    elif any(kw in lowered for kw in ["silabus", "rps", "pertemuan", "rencana pembelajaran"]):
        kind = "syllabus"
        priority = "LOW"
    elif any(kw in lowered for kw in ["pengumuman", "jadwal", "informasi"]):
        kind = "announcement"
        priority = "MEDIUM"
    else:
        kind = "general"
        priority = "MEDIUM"

    # Detect Course
    course_match = re.search(
        r"(?:mata kuliah|matakuliah|course|mk)\s*[:=\-]?\s*([^\n\r,]+)",
        text,
        re.IGNORECASE,
    )
    if course_match:
        course = course_match.group(1).strip()
    else:
        # Check filename
        course = "Perkuliahan"
        for kw in ["Metode Penelitian", "Machine Learning", "Basis Data", "Pemrograman Web", "Kecerdasan Buatan"]:
            if kw.lower() in lowered or kw.lower() in filename.lower():
                course = kw
                break

    # Detect Title
    title_match = re.search(
        r"(?:judul|topik|tugas)\s*[:=\-]?\s*([^\n\r]+)",
        text,
        re.IGNORECASE,
    )
    if title_match:
        title = title_match.group(1).strip()
    else:
        clean_fn = re.sub(r"\.[^.]+$", "", filename).replace("_", " ").title()
        title = clean_fn if clean_fn else "Tugas Akademik"

    # Detect Deadline
    deadline = None
    # 1. ISO format: YYYY-MM-DD
    iso_match = re.search(r"(20\d{2}-\d{2}-\d{2})(?:[ T](\d{1,2}[:.]\d{2}))?", text)
    if iso_match:
        d = iso_match.group(1)
        t = (iso_match.group(2) or "23:59").replace(".", ":")
        if len(t.split(":")[0]) == 1:
            t = "0" + t
        deadline = f"{d} {t}"
    else:
        # Default 7 days ahead
        future_dt = datetime.now() + timedelta(days=7)
        deadline = future_dt.strftime("%Y-%m-%d 23:59")

    # Generate basic subtasks
    subtasks = [
        f"Pelajari instruksi dan modul {course}",
        "Buat draft dan kerjakan penyelesaian tugas",
        "Lakukan reviu dan format dokumen sesuai standar",
        "Kumpulkan sebelum batas waktu pengumpulan"
    ]

    summary = (
        f"Dokumen akademik terkait {course}: '{title}'. "
        f"Batas waktu pengumpulan tercatat pada {deadline}."
    )

    return {
        "is_academic": is_academic,
        "kind": kind,
        "course": course,
        "title": title,
        "deadline": deadline,
        "estimated_hours": 3,
        "priority": priority,
        "summary": summary,
        "subtasks": subtasks,
        "deliverables": ["Dokumen Laporan / Source Code"],
        "extractor": "heuristic_fallback"
    }


def analyze_document_content(
    content: str,
    filename: str = "",
    timeout: int = 45,
    force_fallback: bool = False
) -> Dict[str, Any]:
    """
    Main extraction function. Calls 9Router / Hermes / OpenAI API.
    Gracefully falls back to heuristic extraction on error.
    """
    if force_fallback or not ROUTER_API_BASE:
        logger.info("Using heuristic fallback extractor.")
        return fallback_rule_based_extraction(content, filename)

    # Truncate content to avoid exceeding context window
    truncated_content = content[:12000].strip()
    user_prompt = (
        f"Nama Dokumen: {filename}\n\n"
        f"Isi Dokumen:\n{truncated_content}\n\n"
        "Ekstrak informasi tugas sesuai skema JSON yang telah ditentukan."
    )

    url = f"{ROUTER_API_BASE}/chat/completions"
    headers = {
        "Authorization": f"Bearer {ROUTER_API_KEY}",
        "Content-Type": "application/json",
    }
    body = {
        "model": ROUTER_MODEL,
        "messages": [
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": user_prompt},
        ],
        "temperature": 0.1,
        "stream": False,
    }

    try:
        response = requests.post(url, json=body, headers=headers, timeout=timeout)
        if response.status_code == 200:
            data = response.json()
            raw_result = ""
            if "choices" in data and len(data["choices"]) > 0:
                raw_result = data["choices"][0]["message"].get("content", "")
            
            parsed = _clean_json_response(raw_result)
            if parsed and isinstance(parsed, dict) and "title" in parsed:
                parsed["extractor"] = "9router_ai"
                return parsed
            else:
                logger.warning("AI response was not valid JSON. Falling back to heuristic.")
        else:
            logger.warning(f"9Router returned HTTP {response.status_code}: {response.text}")
    except Exception as exc:
        logger.warning(f"Error querying 9Router ({exc}). Using heuristic fallback.")

    # Graceful fallback
    result = fallback_rule_based_extraction(content, filename)
    result["extractor"] = "heuristic_fallback"
    return result
