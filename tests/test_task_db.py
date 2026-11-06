"""
Tests for SQLite task registry.
"""

from pathlib import Path
from academic_agent import task_db


def test_task_crud(tmp_path, monkeypatch):
    test_db = tmp_path / "test_tasks.db"
    monkeypatch.setattr("academic_agent.task_db.DB_PATH", test_db)
    
    task_db.init_db()
    assert test_db.exists()

    # Add task
    sample = {
        "title": "Tugas Jaringan Komputer",
        "course": "Jaringan Komputer",
        "deadline": "2026-11-25 23:59",
        "priority": "HIGH",
        "summary": "Analisis protokol TCP/IP",
        "subtasks": ["Wireshark capture", "Analisis handshake"],
        "deliverables": ["PDF laporan"]
    }
    task_id = task_db.add_task(sample, ics_file="test.ics", source_file="tugas.pdf")
    assert task_id > 0

    # List tasks
    all_tasks = task_db.list_tasks()
    assert len(all_tasks) == 1
    assert all_tasks[0]["title"] == "Tugas Jaringan Komputer"
    assert all_tasks[0]["status"] == "PENDING"
    assert len(all_tasks[0]["subtasks"]) == 2

    # Update status
    updated = task_db.update_task_status(task_id, "COMPLETED")
    assert updated is True
    
    completed = task_db.list_tasks(status="COMPLETED")
    assert len(completed) == 1

    # Delete task
    deleted = task_db.delete_task(task_id)
    assert deleted is True
    assert len(task_db.list_tasks()) == 0
