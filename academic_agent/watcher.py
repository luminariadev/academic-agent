"""
watcher.py — Directory Watcher Daemon for Automatic Ingestion.
Uses watchdog to detect new academic documents dropped into the monitored folder.
"""

import logging
import os
import shutil
import sys
import time
from pathlib import Path
from typing import Optional

# Ensure UTF-8 output on Windows consoles
if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8")
if hasattr(sys.stderr, "reconfigure"):
    sys.stderr.reconfigure(encoding="utf-8")

import requests
from watchdog.events import FileSystemEventHandler, FileCreatedEvent
from watchdog.observers import Observer

from academic_agent.agent import analyze_document_content
from academic_agent.calendar_sync import create_ics_event
from academic_agent.config import (
    WATCH_DIR,
    BACKUP_DIR,
    N8N_WEBHOOK_URL,
)
from academic_agent.discord_notifier import send_discord_message
from academic_agent.parser import extract_text_from_file
from academic_agent.task_db import add_task

logging.basicConfig(level=logging.INFO, format="%(asctime)s [%(levelname)s] %(message)s")
logger = logging.getLogger("academic_agent.watcher")

VALID_EXTENSIONS = {".pdf", ".docx", ".doc", ".txt", ".md"}


def process_academic_file(file_path: Path) -> Optional[dict]:
    """Complete end-to-end processing pipeline for a single file."""
    if not file_path.exists() or file_path.name.startswith("~$") or file_path.name.startswith("."):
        return None

    if file_path.suffix.lower() not in VALID_EXTENSIONS:
        return None

    logger.info(f"📁 Processing new document: {file_path.name}")
    
    # Wait briefly for file write completion
    time.sleep(1.0)

    try:
        # 1. Parse text from document
        parsed_doc = extract_text_from_file(file_path)
        if not parsed_doc["success"] or not parsed_doc["content"]:
            logger.warning(f"Could not extract text from {file_path.name}: {parsed_doc.get('error')}")
            return None

        # 2. Analyze via AI Agent (9Router / Hermes / Fallback)
        task_data = analyze_document_content(
            content=parsed_doc["content"],
            filename=file_path.name
        )
        logger.info(f"✨ AI Extracted: [{task_data.get('course')}] {task_data.get('title')} (Deadline: {task_data.get('deadline')})")

        # 3. Generate .ics Calendar Event
        cal_info = None
        if task_data.get("deadline"):
            cal_info = create_ics_event(
                title=task_data.get("title", "Tugas Kuliah"),
                deadline=task_data["deadline"],
                course=task_data.get("course", ""),
                description=task_data.get("summary", ""),
                duration_hours=task_data.get("estimated_hours", 2),
                priority=task_data.get("priority", "MEDIUM")
            )
            logger.info(f"📅 iCalendar event created: {cal_info['filename']}")

        # 4. Save to Persistent Task DB
        task_id = add_task(
            task_data=task_data,
            ics_file=cal_info.get("filename") if cal_info else None,
            source_file=file_path.name
        )
        task_data["task_id"] = task_id

        # 5. Send Rich Discord Notification
        discord_res = send_discord_message(task_data, cal_info)
        logger.info(f"💬 Discord notification status: {discord_res.get('success')}")

        # 6. Optional: Trigger n8n Webhook for secondary automations
        if N8N_WEBHOOK_URL:
            try:
                payload = {
                    "event": "new_academic_task",
                    "task_id": task_id,
                    "task": task_data,
                    "calendar": cal_info,
                    "source_file": file_path.name,
                }
                resp = requests.post(N8N_WEBHOOK_URL, json=payload, timeout=5)
                logger.info(f"🔗 n8n Webhook triggered (HTTP {resp.status_code})")
            except Exception as e:
                logger.debug(f"n8n webhook notification skipped or offline: {e}")

        # 7. Backup file
        BACKUP_DIR.mkdir(parents=True, exist_ok=True)
        backup_dest = BACKUP_DIR / file_path.name
        shutil.copy2(file_path, backup_dest)
        logger.info(f"📦 Archived copy saved to {backup_dest.name}")

        return {
            "task_id": task_id,
            "task_data": task_data,
            "calendar_info": cal_info,
            "discord_result": discord_res
        }

    except Exception as exc:
        logger.error(f"Failed to process {file_path.name}: {exc}", exc_info=True)
        return None


class AcademicFileHandler(FileSystemEventHandler):
    def on_created(self, event):
        if not event.is_directory:
            process_academic_file(Path(event.src_path))


def start_watcher(watch_path: Optional[Path] = None, blocking: bool = True):
    """Starts the watchdog observer."""
    target_dir = watch_path or WATCH_DIR
    target_dir.mkdir(parents=True, exist_ok=True)
    
    event_handler = AcademicFileHandler()
    observer = Observer()
    observer.schedule(event_handler, str(target_dir), recursive=False)
    observer.start()
    logger.info(f"👀 Academic Agent Watcher actively monitoring: {target_dir.resolve()}")
    
    if blocking:
        try:
            while True:
                time.sleep(1)
        except KeyboardInterrupt:
            observer.stop()
            logger.info("Watcher stopped by user.")
        observer.join()
    return observer


if __name__ == "__main__":
    start_watcher()
