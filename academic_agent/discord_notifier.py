"""
discord_notifier.py — Discord notification service for Academic Agent.
Supports Discord REST API (Embeds, Components) and Hermes CLI Gateway fallback.
"""

import json
import logging
import os
import subprocess
import urllib.request
from typing import Dict, Any, Optional

from academic_agent.config import (
    DISCORD_BOT_TOKEN,
    DISCORD_CHANNEL_ID,
)

logger = logging.getLogger(__name__)

PRIORITY_COLORS = {
    "HIGH": 0xED4245,    # Red
    "MEDIUM": 0xFEE75C,  # Amber/Yellow
    "LOW": 0x57F287,     # Green
}

PRIORITY_EMOJIS = {
    "HIGH": "🔴 Tinggi",
    "MEDIUM": "🟡 Sedang",
    "LOW": "🟢 Rendah",
}


def build_discord_embed(task_data: Dict[str, Any], calendar_info: Optional[Dict[str, Any]] = None) -> Dict[str, Any]:
    """Constructs a high-fidelity Discord embed for an academic task."""
    priority = (task_data.get("priority") or "MEDIUM").upper()
    color = PRIORITY_COLORS.get(priority, 0x5865F2)
    prio_label = PRIORITY_EMOJIS.get(priority, priority)

    course = task_data.get("course") or "Perkuliahan"
    title = task_data.get("title") or "Tugas Baru"
    deadline = task_data.get("deadline") or "Tidak ditentukan"
    summary = task_data.get("summary") or "Tugas baru telah terdeteksi dan dijadwalkan."
    hours = task_data.get("estimated_hours") or 2

    fields = [
        {"name": "📚 Mata Kuliah", "value": f"**{course}**", "inline": True},
        {"name": "⏰ Batas Waktu", "value": f"`{deadline}`", "inline": True},
        {"name": "⚡ Prioritas", "value": prio_label, "inline": True},
        {"name": "⏳ Estimasi Pengerjaan", "value": f"{hours} Jam", "inline": True},
    ]

    # Subtasks
    subtasks = task_data.get("subtasks", [])
    if subtasks and isinstance(subtasks, list):
        subtasks_text = "\n".join([f"▫️ {st}" for st in subtasks[:6]])
        fields.append({"name": "📋 Sub-task Checklist", "value": subtasks_text, "inline": False})

    # Deliverables
    deliverables = task_data.get("deliverables", [])
    if deliverables and isinstance(deliverables, list):
        deliv_text = ", ".join(deliverables)
        fields.append({"name": "📦 Format Pengumpulan", "value": deliv_text, "inline": True})

    # Calendar status
    if calendar_info and calendar_info.get("filename"):
        fields.append({
            "name": "📅 Status Kalender",
            "value": f"✅ File `.ics` dibuat: `{calendar_info.get('filename')}`",
            "inline": False
        })

    embed = {
        "title": f"🎓 Tugas Baru: {title}",
        "description": summary,
        "color": color,
        "fields": fields,
        "footer": {
            "text": "Academic Agent • Hybrid AI & n8n Orchestrator",
            "icon_url": "https://cdn.jsdelivr.net/gh/twitter/twemoji@14.0.2/assets/72x72/1f393.png"
        }
    }

    return embed


def send_discord_message(
    task_data: Dict[str, Any],
    calendar_info: Optional[Dict[str, Any]] = None,
    channel_id: Optional[str] = None
) -> Dict[str, Any]:
    """
    Sends Discord embed notification using official Discord REST API v10.
    Includes Google Calendar button link if available.
    """
    target_channel = channel_id or DISCORD_CHANNEL_ID
    embed = build_discord_embed(task_data, calendar_info)

    payload = {
        "embeds": [embed],
        "components": []
    }

    # Add 1-click Google Calendar button if URL exists and within Discord limit
    gcal_url = calendar_info.get("google_calendar_url") if calendar_info else None
    if gcal_url and len(gcal_url) <= 512:
        payload["components"] = [
            {
                "type": 1,  # Action Row
                "components": [
                    {
                        "type": 2,  # Button
                        "style": 5,  # Link button
                        "label": "📅 Google Calendar",
                        "url": gcal_url
                    }
                ]
            }
        ]

    url = f"https://discord.com/api/v10/channels/{target_channel}/messages"
    headers = {
        "Authorization": f"Bot {DISCORD_BOT_TOKEN}",
        "Content-Type": "application/json",
        "User-Agent": "DiscordBot (AcademicAgent, 2.0)",
    }

    try:
        req = urllib.request.Request(
            url,
            data=json.dumps(payload).encode("utf-8"),
            headers=headers,
            method="POST"
        )
        with urllib.request.urlopen(req, timeout=15) as resp:
            resp_body = resp.read().decode("utf-8")
            return {
                "success": True,
                "status_code": resp.status,
                "channel_id": target_channel,
                "response": json.loads(resp_body) if resp_body else {}
            }
    except Exception as exc:
        logger.warning(f"Direct Discord API error: {exc}. Attempting Hermes CLI fallback.")
        return send_via_hermes(
            f"🎓 Tugas Baru: [{task_data.get('course')}] {task_data.get('title')} | Deadline: {task_data.get('deadline')}"
        )


def send_via_hermes(message: str, target: str = "discord:general") -> Dict[str, Any]:
    """Fallback method using hermes send CLI."""
    try:
        env = os.environ.copy()
        env["DISCORD_BOT_TOKEN"] = DISCORD_BOT_TOKEN
        result = subprocess.run(
            ["hermes", "send", "--to", target, message],
            capture_output=True,
            text=True,
            env=env,
            timeout=15
        )
        return {
            "success": result.returncode == 0,
            "stdout": result.stdout.strip(),
            "stderr": result.stderr.strip()
        }
    except Exception as exc:
        return {"success": False, "error": str(exc)}
