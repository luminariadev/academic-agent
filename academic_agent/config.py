"""
config.py — Central configuration management for Academic Agent.
"""

import os
from pathlib import Path

# Base directories
BASE_DIR = Path(__file__).resolve().parent.parent

def _resolve_path(env_key: str, default_rel: str) -> Path:
    val = os.getenv(env_key)
    if not val:
        return BASE_DIR / default_rel
    p = Path(val)
    if not p.is_absolute():
        return (BASE_DIR / p).resolve()
    return p.resolve()

# Try loading .env if available
try:
    from dotenv import load_dotenv
    load_dotenv(BASE_DIR / ".env")
except ImportError:
    pass

DATA_DIR = _resolve_path("DATA_FOLDER", "data")
WATCH_DIR = _resolve_path("WATCH_FOLDER", "tugas_mahasiswa")
BACKUP_DIR = _resolve_path("BACKUP_FOLDER", "backup_tugas")
CALENDAR_DIR = _resolve_path("CALENDAR_FOLDER", "data/calendar")
DB_PATH = _resolve_path("DATABASE_PATH", "data/tasks.db")

# 9Router / LLM Inference Settings
ROUTER_API_BASE = os.getenv("ROUTER_API_BASE", "http://127.0.0.1:20128/v1").rstrip("/")
ROUTER_API_KEY = os.getenv("ROUTER_API_KEY", "")
ROUTER_MODEL = os.getenv("ROUTER_MODEL", "mr-loverman")

# Discord Bot Credentials
DISCORD_BOT_TOKEN = os.getenv("DISCORD_BOT_TOKEN", "")
DISCORD_CLIENT_ID = os.getenv("DISCORD_CLIENT_ID", "")
DISCORD_CHANNEL_ID = os.getenv("DISCORD_CHANNEL_ID", "")
DISCORD_ALLOWED_USERS = os.getenv("DISCORD_ALLOWED_USERS", "")

# n8n Automation Orchestrator Settings
N8N_WEBHOOK_URL = os.getenv("N8N_WEBHOOK_URL", "http://localhost:5678/webhook/academic-task")

# API Server Settings
SERVER_HOST = os.getenv("SERVER_HOST", "0.0.0.0")
SERVER_PORT = int(os.getenv("SERVER_PORT", "8000"))

# Ensure runtime directories exist
DATA_DIR.mkdir(parents=True, exist_ok=True)
WATCH_DIR.mkdir(parents=True, exist_ok=True)
BACKUP_DIR.mkdir(parents=True, exist_ok=True)
CALENDAR_DIR.mkdir(parents=True, exist_ok=True)
