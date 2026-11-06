"""
calendar_sync.py — RFC 5545 iCalendar (.ics) Generator & Google Calendar Sync Link.
"""

import os
import re
import urllib.parse
from datetime import datetime, timedelta
from pathlib import Path
from typing import Optional, Dict, Any
from uuid import uuid4

from academic_agent.config import CALENDAR_DIR


def parse_deadline(deadline_str: Optional[str]) -> Optional[datetime]:
    """Parse various deadline string formats into a Python datetime object."""
    if not deadline_str or not isinstance(deadline_str, str):
        return None

    cleaned = deadline_str.strip()
    formats = [
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%d %H.%M",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d",
        "%d/%m/%Y %H:%M",
        "%d-%m-%Y %H:%M",
        "%d/%m/%Y",
        "%d-%m-%Y",
    ]

    for fmt in formats:
        try:
            parsed = datetime.strptime(cleaned, fmt)
            # Default time to 23:59 if only date was provided
            if fmt in ["%Y-%m-%d", "%d/%m/%Y", "%d-%m-%Y"]:
                return parsed.replace(hour=23, minute=59, second=0)
            return parsed
        except ValueError:
            continue

    # Regex search for YYYY-MM-DD
    match = re.search(r"(\d{4}-\d{2}-\d{2})(?:[ T](\d{1,2}[:.]\d{2}))?", cleaned)
    if match:
        d_part = match.group(1)
        t_part = (match.group(2) or "23:59").replace(".", ":")
        try:
            return datetime.strptime(f"{d_part} {t_part}", "%Y-%m-%d %H:%M")
        except ValueError:
            pass

    return None


def format_ics_datetime(dt: datetime) -> str:
    """Format datetime for iCalendar UTC / Local."""
    return dt.strftime("%Y%m%dT%H%M%S")


def escape_ics(text: str) -> str:
    """Escape special characters according to RFC 5545."""
    if not text:
        return ""
    return (
        str(text)
        .replace("\\", "\\\\")
        .replace("\n", "\\n")
        .replace(",", "\\,")
        .replace(";", "\\;")
    )


def safe_slug(text: str, max_len: int = 50) -> str:
    """Create filesystem-safe slug from title."""
    cleaned = re.sub(r"[^a-zA-Z0-9_\-]+", "_", text or "event").strip("_")
    return cleaned[:max_len] or "academic_event"


def generate_google_calendar_url(
    title: str,
    start_time: datetime,
    end_time: datetime,
    description: str = "",
    location: str = "Online / LMS Kampus",
) -> str:
    """Generate a 1-click Google Calendar web creation URL (keeps total URL <= 512 chars for Discord)."""
    fmt = "%Y%m%dT%H%M%SZ"
    dates_param = f"{start_time.strftime(fmt)}/{end_time.strftime(fmt)}"
    
    # Truncate title and description so Discord 512-char URL limit is never breached
    clean_title = (title or "Tugas Kuliah")[:70]
    clean_desc = (description or "")[:120].replace("\n", " ").strip()
    
    params = {
        "action": "TEMPLATE",
        "text": clean_title,
        "dates": dates_param,
        "details": clean_desc,
        "location": location[:40],
    }
    return f"https://calendar.google.com/calendar/render?{urllib.parse.urlencode(params)}"


def create_ics_event(
    title: str,
    deadline: str | datetime,
    course: str = "",
    description: str = "",
    duration_hours: int = 2,
    priority: str = "MEDIUM",
) -> Dict[str, Any]:
    """
    Creates an RFC 5545 compliant .ics file and calendar links.
    Returns path to file, content, and Google Calendar 1-click link.
    """
    if isinstance(deadline, str):
        end_time = parse_deadline(deadline)
    else:
        end_time = deadline

    if not end_time:
        # Default fallback to 7 days from now at 23:59
        end_time = (datetime.now() + timedelta(days=7)).replace(hour=23, minute=59, second=0)

    start_time = end_time - timedelta(hours=max(1, duration_hours))
    uid = str(uuid4())
    summary = f"[{course}] {title}" if course else title
    
    # Priority RFC 5545: 1 (High), 5 (Medium), 9 (Low)
    prio_val = 1 if priority.upper() == "HIGH" else (9 if priority.upper() == "LOW" else 5)

    filename = f"{end_time.strftime('%Y%m%d_%H%M')}_{safe_slug(summary)}.ics"
    file_path = CALENDAR_DIR / filename

    now_utc = datetime.utcnow()
    lines = [
        "BEGIN:VCALENDAR",
        "VERSION:2.0",
        "PRODID:-//Academic Agent//Modern AI Orchestration//ID",
        "CALSCALE:GREGORIAN",
        "METHOD:PUBLISH",
        "BEGIN:VEVENT",
        f"UID:{uid}",
        f"DTSTAMP:{format_ics_datetime(now_utc)}Z",
        f"DTSTART:{format_ics_datetime(start_time)}",
        f"DTEND:{format_ics_datetime(end_time)}",
        f"SUMMARY:{escape_ics(summary)}",
        f"DESCRIPTION:{escape_ics(description)}",
        f"CATEGORIES:ACADEMIC,TUGAS,KULIAH",
        f"PRIORITY:{prio_val}",
        "STATUS:CONFIRMED",
        # 24 Hours Before Reminder
        "BEGIN:VALARM",
        "TRIGGER:-P1D",
        "ACTION:DISPLAY",
        f"DESCRIPTION:{escape_ics('H-1 Deadline: ' + summary)}",
        "END:VALARM",
        # 1 Hour Before Reminder
        "BEGIN:VALARM",
        "TRIGGER:-PT1H",
        "ACTION:DISPLAY",
        f"DESCRIPTION:{escape_ics('Segera Kumpulkan: ' + summary)}",
        "END:VALARM",
        "END:VEVENT",
        "END:VCALENDAR",
        "",
    ]
    
    ics_content = "\r\n".join(lines)
    
    with open(file_path, "w", encoding="utf-8") as f:
        f.write(ics_content)

    gcal_url = generate_google_calendar_url(
        title=summary,
        start_time=start_time,
        end_time=end_time,
        description=description,
    )

    return {
        "uid": uid,
        "summary": summary,
        "filename": filename,
        "file_path": str(file_path.resolve()),
        "start_time": start_time.isoformat(),
        "end_time": end_time.isoformat(),
        "ics_content": ics_content,
        "google_calendar_url": gcal_url,
    }
