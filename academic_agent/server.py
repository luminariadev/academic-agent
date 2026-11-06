"""
server.py — FastAPI Webhook Bridge & REST API for Academic Agent.
Allows bidirectional integration with n8n, local triggers, and external webhooks.
"""

from pathlib import Path
from typing import Optional, Dict, Any

from fastapi import FastAPI, UploadFile, File, Form, HTTPException
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import FileResponse
from pydantic import BaseModel

from academic_agent.agent import analyze_document_content
from academic_agent.calendar_sync import create_ics_event
from academic_agent.config import CALENDAR_DIR, WATCH_DIR, BACKUP_DIR
from academic_agent.discord_notifier import send_discord_message
from academic_agent.parser import extract_text_from_file
from academic_agent.task_db import add_task, list_tasks, update_task_status

app = FastAPI(
    title="Academic Agent API",
    description="Bridge microservice connecting n8n orchestrator with local AI & Discord",
    version="2.0.0"
)

app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)


class TextExtractRequest(BaseModel):
    text: str
    filename: Optional[str] = "input_text.txt"


class NotifyRequest(BaseModel):
    task: Dict[str, Any]
    generate_calendar: bool = True


@app.get("/health")
def health_check():
    return {
        "status": "online",
        "service": "Academic Agent Core",
        "version": "2.0.0",
        "ai_gateway": "9Router / Hermes / OpenAI Compatible",
        "discord_bot": "HermesAgent Active"
    }


@app.get("/api/tasks")
def get_all_tasks(status: Optional[str] = None):
    return {"tasks": list_tasks(status)}


@app.patch("/api/tasks/{task_id}/status")
def patch_task_status(task_id: int, status: str):
    success = update_task_status(task_id, status)
    if not success:
        raise HTTPException(status_code=404, detail="Task not found")
    return {"success": True, "task_id": task_id, "status": status.upper()}


@app.post("/api/extract")
def extract_from_text(payload: TextExtractRequest):
    """Analyze raw academic text and return structured JSON."""
    analysis = analyze_document_content(payload.text, filename=payload.filename or "direct_input.txt")
    return {"success": True, "data": analysis}


@app.post("/api/upload")
async def upload_document(file: UploadFile = File(...)):
    """Upload a PDF/DOCX document directly to trigger full agent pipeline."""
    temp_path = WATCH_DIR / file.filename
    with open(temp_path, "wb") as f:
        content = await file.read()
        f.write(content)

    parsed = extract_text_from_file(temp_path)
    if not parsed["success"]:
        raise HTTPException(status_code=400, detail=f"Failed to read file: {parsed.get('error')}")

    task_data = analyze_document_content(parsed["content"], filename=file.filename)
    
    cal_info = None
    if task_data.get("deadline"):
        cal_info = create_ics_event(
            title=task_data.get("title", file.filename),
            deadline=task_data["deadline"],
            course=task_data.get("course", "Perkuliahan"),
            description=task_data.get("summary", ""),
            duration_hours=task_data.get("estimated_hours", 2),
            priority=task_data.get("priority", "MEDIUM")
        )

    task_id = add_task(task_data, ics_file=cal_info["filename"] if cal_info else None, source_file=file.filename)
    task_data["task_id"] = task_id

    # Dispatch Discord notification
    discord_res = send_discord_message(task_data, cal_info)

    return {
        "success": True,
        "task_id": task_id,
        "task": task_data,
        "calendar": cal_info,
        "discord": discord_res
    }


@app.post("/api/notify")
def notify_discord(payload: NotifyRequest):
    """Directly send a task notification to Discord."""
    cal_info = None
    task_data = payload.task
    if payload.generate_calendar and task_data.get("deadline"):
        cal_info = create_ics_event(
            title=task_data.get("title", "Tugas Kuliah"),
            deadline=task_data["deadline"],
            course=task_data.get("course", "Perkuliahan"),
            description=task_data.get("summary", ""),
            duration_hours=task_data.get("estimated_hours", 2),
            priority=task_data.get("priority", "MEDIUM")
        )

    discord_res = send_discord_message(task_data, cal_info)
    return {"success": True, "discord": discord_res, "calendar": cal_info}


@app.get("/api/calendar/{filename}")
def download_calendar(filename: str):
    file_path = CALENDAR_DIR / filename
    if not file_path.exists():
        raise HTTPException(status_code=404, detail="Calendar file not found")
    return FileResponse(
        path=str(file_path),
        filename=filename,
        media_type="text/calendar"
    )
