"""
task_db.py — Persistent SQLite Task Registry for Academic Agent.
"""

import json
import sqlite3
from datetime import datetime
from typing import List, Dict, Any, Optional

from academic_agent.config import DB_PATH


def get_connection() -> sqlite3.Connection:
    """Creates a connection to the SQLite database with row factory."""
    DB_PATH.parent.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(str(DB_PATH), check_same_thread=False)
    conn.row_factory = sqlite3.Row
    return conn


def init_db() -> None:
    """Initializes the tasks table if not existing."""
    with get_connection() as conn:
        conn.execute("""
            CREATE TABLE IF NOT EXISTS tasks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                title TEXT NOT NULL,
                course TEXT NOT NULL,
                deadline TEXT,
                priority TEXT DEFAULT 'MEDIUM',
                status TEXT DEFAULT 'PENDING',
                subtasks_json TEXT,
                deliverables_json TEXT,
                summary TEXT,
                ics_file TEXT,
                source_file TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )
        """)
        conn.commit()


def add_task(
    task_data: Dict[str, Any],
    ics_file: Optional[str] = None,
    source_file: Optional[str] = None
) -> int:
    """Inserts a new academic task into SQLite."""
    init_db()
    now_iso = datetime.now().isoformat()
    subtasks = json.dumps(task_data.get("subtasks", []), ensure_ascii=False)
    deliverables = json.dumps(task_data.get("deliverables", []), ensure_ascii=False)

    with get_connection() as conn:
        cursor = conn.execute("""
            INSERT INTO tasks (
                title, course, deadline, priority, status,
                subtasks_json, deliverables_json, summary,
                ics_file, source_file, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        """, (
            task_data.get("title", "Tugas Akademik"),
            task_data.get("course", "Perkuliahan"),
            task_data.get("deadline"),
            task_data.get("priority", "MEDIUM"),
            "PENDING",
            subtasks,
            deliverables,
            task_data.get("summary", ""),
            ics_file or "",
            source_file or "",
            now_iso,
            now_iso
        ))
        conn.commit()
        return cursor.lastrowid


def list_tasks(status: Optional[str] = None) -> List[Dict[str, Any]]:
    """Returns a list of tasks, optionally filtered by status."""
    init_db()
    with get_connection() as conn:
        if status:
            cursor = conn.execute("SELECT * FROM tasks WHERE status = ? ORDER BY deadline ASC", (status,))
        else:
            cursor = conn.execute("SELECT * FROM tasks ORDER BY deadline ASC")
        rows = cursor.fetchall()
        
    tasks = []
    for r in rows:
        item = dict(r)
        item["subtasks"] = json.loads(item["subtasks_json"]) if item["subtasks_json"] else []
        item["deliverables"] = json.loads(item["deliverables_json"]) if item["deliverables_json"] else []
        tasks.append(item)
    return tasks


def update_task_status(task_id: int, new_status: str) -> bool:
    """Updates status ('PENDING', 'IN_PROGRESS', 'COMPLETED') of a task."""
    init_db()
    now_iso = datetime.now().isoformat()
    with get_connection() as conn:
        cursor = conn.execute("""
            UPDATE tasks SET status = ?, updated_at = ? WHERE id = ?
        """, (new_status.upper(), now_iso, task_id))
        conn.commit()
        return cursor.rowcount > 0


def delete_task(task_id: int) -> bool:
    """Deletes a task by ID."""
    init_db()
    with get_connection() as conn:
        cursor = conn.execute("DELETE FROM tasks WHERE id = ?", (task_id,))
        conn.commit()
        return cursor.rowcount > 0
