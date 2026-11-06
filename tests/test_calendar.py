"""
Tests for iCalendar generation and Google Calendar sync link.
"""

from datetime import datetime
from pathlib import Path
from academic_agent.calendar_sync import (
    parse_deadline,
    create_ics_event,
    generate_google_calendar_url,
    escape_ics
)


def test_parse_deadline_formats():
    assert parse_deadline("2026-12-05 23:59") == datetime(2026, 12, 5, 23, 59)
    assert parse_deadline("2026-12-05") == datetime(2026, 12, 5, 23, 59)
    assert parse_deadline("05/12/2026 14:00") == datetime(2026, 12, 5, 14, 0)
    assert parse_deadline(None) is None
    assert parse_deadline("invalid date string") is None


def test_create_ics_event(tmp_path, monkeypatch):
    monkeypatch.setattr("academic_agent.calendar_sync.CALENDAR_DIR", tmp_path)
    
    event = create_ics_event(
        title="Laporan Akhir Machine Learning",
        deadline="2026-11-30 23:59",
        course="Machine Learning",
        description="Pengumpulan laporan tugas besar",
        priority="HIGH"
    )
    
    assert event["filename"].endswith(".ics")
    assert Path(event["file_path"]).exists()
    
    content = event["ics_content"]
    assert "BEGIN:VCALENDAR" in content
    assert "BEGIN:VEVENT" in content
    assert "SUMMARY:[Machine Learning] Laporan Akhir Machine Learning" in content
    assert "PRIORITY:1" in content  # High priority = 1
    assert "BEGIN:VALARM" in content
    assert "END:VCALENDAR" in content


def test_google_calendar_url_length():
    start = datetime(2026, 11, 30, 21, 59)
    end = datetime(2026, 11, 30, 23, 59)
    long_desc = "A" * 500
    long_title = "B" * 200
    
    url = generate_google_calendar_url(long_title, start, end, long_desc)
    assert url.startswith("https://calendar.google.com/calendar/render?")
    # Check that Discord 512 char limit is respected
    assert len(url) <= 512


def test_escape_ics():
    raw = "Tugas; satu, dua\nbaris baru"
    escaped = escape_ics(raw)
    assert "\\;" in escaped
    assert "\\," in escaped
    assert "\\n" in escaped
