"""
cli.py — Command Line Interface for Academic Agent.
"""

import argparse
import sys
from pathlib import Path
from pprint import pprint

# Ensure UTF-8 output on Windows consoles
if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8")
if hasattr(sys.stderr, "reconfigure"):
    sys.stderr.reconfigure(encoding="utf-8")

from academic_agent.agent import analyze_document_content
from academic_agent.calendar_sync import create_ics_event
from academic_agent.config import ROUTER_API_BASE, ROUTER_MODEL, DISCORD_CHANNEL_ID
from academic_agent.discord_notifier import send_discord_message, send_via_hermes
from academic_agent.parser import extract_text_from_file
from academic_agent.task_db import add_task, list_tasks, update_task_status
from academic_agent.watcher import process_academic_file, start_watcher


def cmd_scan(args):
    """Scan and process a single file."""
    path = Path(args.file)
    if not path.exists():
        print(f"❌ File not found: {path}")
        sys.exit(1)

    print(f"🔍 Parsing document: {path.name}...")
    result = process_academic_file(path)
    if result:
        print("\n✅ Task Successfully Processed & Registered:")
        pprint(result["task_data"])
        if result.get("calendar_info"):
            print(f"📅 Calendar event: {result['calendar_info']['file_path']}")
        print(f"💬 Discord Notification Sent: {result['discord_result'].get('success')}")
    else:
        print("❌ Failed to process document.")


def cmd_tasks(args):
    """List registered academic tasks."""
    tasks = list_tasks(args.status)
    print(f"\n📋 Total Tasks ({args.status or 'ALL'}): {len(tasks)}")
    print("=" * 70)
    for t in tasks:
        status_emoji = "✅" if t["status"] == "COMPLETED" else ("⏳" if t["status"] == "IN_PROGRESS" else "📌")
        print(f"[{t['id']}] {status_emoji} [{t['course']}] {t['title']}")
        print(f"     Deadline : {t['deadline'] or 'N/A'} | Priority: {t['priority']} | Status: {t['status']}")
        if t["subtasks"]:
            print(f"     Subtasks : {', '.join(t['subtasks'][:3])}...")
        print("-" * 70)


def cmd_done(args):
    """Mark a task as completed."""
    success = update_task_status(args.task_id, "COMPLETED")
    if success:
        print(f"✅ Task #{args.task_id} marked as COMPLETED!")
    else:
        print(f"❌ Task #{args.task_id} not found.")


def cmd_test_ai(args):
    """Test 9Router / LLM connection."""
    print(f"🤖 Testing 9Router Gateway at {ROUTER_API_BASE} (Model: {ROUTER_MODEL})...")
    test_text = """
    Tugas Mata Kuliah: Metode Penelitian
    Dosen Pengampu: Dr. Ir. Hendra, M.Kom
    Tugas: Membuat draft BAB 1 dan BAB 2 tentang Implementasi Multi-Agent LLM.
    Batas Pengumpulan: 2026-11-20 23:59 WIB melalui LMS Kampus.
    Format pengumpulan: File PDF maksimal 10 halaman.
    """
    res = analyze_document_content(test_text, filename="tugas_metode_penelitian.txt")
    print("\n✅ AI Response:")
    pprint(res)


def cmd_test_discord(args):
    """Send test notification to Discord."""
    print(f"💬 Sending test embed to Discord channel {DISCORD_CHANNEL_ID}...")
    sample_task = {
        "course": "Sistem Terdistribusi",
        "title": "Simulasi Consensus Raft & Paxos",
        "deadline": "2026-11-25 23:59",
        "priority": "HIGH",
        "estimated_hours": 4,
        "summary": "Implementasikan algoritma konsensus Raft sederhana menggunakan Python atau Go.",
        "subtasks": [
            "Buat struktur state node (Follower, Candidate, Leader)",
            "Implementasi mekanisme Heartbeat dan Election Timeout",
            "Buat skenario test partisi jaringan",
            "Tulis laporan analisis throughput"
        ],
        "deliverables": ["Source Code Repository", "PDF Laporan Benchmark"]
    }
    cal = create_ics_event(
        title=sample_task["title"],
        deadline=sample_task["deadline"],
        course=sample_task["course"],
        description=sample_task["summary"],
        priority="HIGH"
    )
    res = send_discord_message(sample_task, cal)
    print("Discord send result:", res)


def cmd_serve(args):
    """Start FastAPI bridge server."""
    import uvicorn
    from academic_agent.server import app
    print(f"🚀 Starting Academic Agent Webhook Server on {args.host}:{args.port}...")
    uvicorn.run(app, host=args.host, port=args.port)


def cmd_watch(args):
    """Start file watcher."""
    print("👀 Starting Academic Agent Folder Watcher...")
    start_watcher()


def main():
    parser = argparse.ArgumentParser(description="Academic Agent CLI Management Tool")
    subparsers = parser.add_subparsers(dest="command", required=True)

    # scan
    p_scan = subparsers.add_parser("scan", help="Scan and process a file")
    p_scan.add_argument("file", help="Path to PDF/DOCX/TXT file")
    p_scan.set_defaults(func=cmd_scan)

    # tasks
    p_tasks = subparsers.add_parser("tasks", help="List tasks")
    p_tasks.add_argument("--status", choices=["PENDING", "IN_PROGRESS", "COMPLETED"], help="Filter by status")
    p_tasks.set_defaults(func=cmd_tasks)

    # done
    p_done = subparsers.add_parser("done", help="Mark task as complete")
    p_done.add_argument("task_id", type=int, help="Task ID")
    p_done.set_defaults(func=cmd_done)

    # test-ai
    p_ai = subparsers.add_parser("test-ai", help="Test 9Router LLM connection")
    p_ai.set_defaults(func=cmd_test_ai)

    # test-discord
    p_disc = subparsers.add_parser("test-discord", help="Test Discord bot embed")
    p_disc.set_defaults(func=cmd_test_discord)

    # serve
    p_serve = subparsers.add_parser("serve", help="Run FastAPI microservice")
    p_serve.add_argument("--host", default="0.0.0.0", help="Host address")
    p_serve.add_argument("--port", type=int, default=8000, help="Port number")
    p_serve.set_defaults(func=cmd_serve)

    # watch
    p_watch = subparsers.add_parser("watch", help="Start real-time directory watcher")
    p_watch.set_defaults(func=cmd_watch)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
