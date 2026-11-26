from __future__ import annotations

import json
import os
import re
import socket
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest
import requests

CONNECTOR_DIR = Path(__file__).resolve().parent.parent
REPO_ROOT = CONNECTOR_DIR.parent.parent
sys.path.insert(0, str(CONNECTOR_DIR))

from connector import sync_lms  # noqa: E402
from mock_lms import DEMO_PASS, DEMO_USER, start_mock_lms_in_thread  # noqa: E402


def _free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def _start_capture_api(expected_token: str):
    uploads = []
    tasks = []

    class CaptureHandler(BaseHTTPRequestHandler):
        def log_message(self, format: str, *args) -> None:  # noqa: A002
            return

        def _check_auth(self) -> bool:
            auth = self.headers.get("Authorization", "")
            if auth != f"Bearer {expected_token}":
                self.send_response(401)
                self.end_headers()
                return False
            return True

        def do_POST(self) -> None:  # noqa: N802
            if not self._check_auth():
                return
            length = int(self.headers.get("Content-Length", "0"))
            raw = self.rfile.read(length)
            if self.path.startswith("/api/upload"):
                match = re.search(rb'filename="([^"]+)"', raw)
                fname = match.group(1).decode("utf-8") if match else "unknown"
                uploads.append({"filename": fname, "size": len(raw)})
                body = json.dumps({"duplicate": False, "task": {"id": len(uploads), "title": fname}}).encode("utf-8")
            elif self.path.startswith("/api/tasks"):
                payload = json.loads(raw.decode("utf-8"))
                tasks.append(payload)
                body = json.dumps({"duplicate": False, "task": {"id": 100 + len(tasks), "title": payload["title"]}}).encode("utf-8")
            else:
                self.send_response(404)
                self.end_headers()
                return

            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    server = ThreadingHTTPServer(("127.0.0.1", 0), CaptureHandler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, f"http://127.0.0.1:{server.server_address[1]}", uploads, tasks


def test_profile_manifests_are_valid_json():
    profile_dir = CONNECTOR_DIR / "profiles"
    required = {
        "name",
        "base_url",
        "login_path",
        "username_selector",
        "password_selector",
        "submit_selector",
        "logged_in_selector",
        "course_paths",
        "item_selector",
        "title_selector",
    }
    profiles = list(profile_dir.glob("*.json"))
    assert len(profiles) >= 3
    for path in profiles:
        data = json.loads(path.read_text(encoding="utf-8"))
        assert required.issubset(data.keys()), f"{path.name} missing required keys"


def test_playwright_sync_login_session_reuse_and_attachments(tmp_path: Path):
    lms_server, lms_url = start_mock_lms_in_thread()
    api_server, api_url, uploads, tasks = _start_capture_api("connector-secret")
    state_file = tmp_path / "storage_state.json"
    profile_path = CONNECTOR_DIR / "profiles" / "mock-lms.json"

    try:
        # First sync signs in with demo credentials and saves persistent storage_state
        res1 = sync_lms(
            profile_path=profile_path,
            api_url=api_url,
            token="connector-secret",
            base_url_override=lms_url,
            storage_state_path=state_file,
            username=DEMO_USER,
            password=DEMO_PASS,
            headless=True,
        )
        assert len(res1) == 3
        assert state_file.exists()
        assert {u["filename"] for u in uploads} == {"raft_lab.txt", "kuis_sdn.docx"}
        assert len(tasks) == 1
        assert tasks[0]["title"] == "Firewall Policy Review"

        # Second sync omits credentials cleanly, proving session cookie reuse works
        uploads.clear()
        tasks.clear()
        res2 = sync_lms(
            profile_path=profile_path,
            api_url=api_url,
            token="connector-secret",
            base_url_override=lms_url,
            storage_state_path=state_file,
            username=None,
            password=None,
            headless=True,
        )
        assert len(res2) == 3
        assert {u["filename"] for u in uploads} == {"raft_lab.txt", "kuis_sdn.docx"}
        assert len(tasks) == 1
    finally:
        lms_server.shutdown()
        api_server.shutdown()


def test_playwright_end_to_end_into_rust_server(tmp_path: Path):
    exe_name = "academic-agent.exe" if os.name == "nt" else "academic-agent"
    candidates = [
        REPO_ROOT / "target" / "release" / exe_name,
        REPO_ROOT / "target" / "debug" / exe_name,
    ]
    bin_path = next((p for p in candidates if p.is_file()), None)
    if bin_path is None:
        pytest.skip("Compiled academic-agent binary not present in target/")

    cfg_path = tmp_path / "academic-agent.toml"
    db_path = tmp_path / "state.redb"
    inbox_dir = tmp_path / "inbox"
    archive_dir = tmp_path / "archive"
    cal_dir = tmp_path / "calendar"

    token = "e2e-bridge-token"
    cfg_path.write_text(
        f"""
[general]
inbox_dir = "{inbox_dir.as_posix()}"
archive_dir = "{archive_dir.as_posix()}"
calendar_dir = "{cal_dir.as_posix()}"
db_path = "{db_path.as_posix()}"
timezone = "Asia/Jakarta"

[discord]
enabled = false

[server]
api_token = "{token}"
""",
        encoding="utf-8",
    )

    port = _free_port()
    bind = f"127.0.0.1:{port}"
    api_url = f"http://{bind}"
    proc = subprocess.Popen(
        [str(bin_path), "--config", str(cfg_path), "serve", "--bind", bind],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    lms_server, lms_url = start_mock_lms_in_thread()

    try:
        ready = False
        for _ in range(40):
            try:
                r = requests.get(f"{api_url}/health", timeout=1)
                if r.ok:
                    ready = True
                    break
            except Exception:
                pass
            time.sleep(0.1)
        assert ready, "academic-agent serve did not start in time"

        profile_path = CONNECTOR_DIR / "profiles" / "mock-lms.json"
        sync_lms(
            profile_path=profile_path,
            api_url=api_url,
            token=token,
            base_url_override=lms_url,
            username=DEMO_USER,
            password=DEMO_PASS,
            headless=True,
            heuristic=True,
            notify=False,
        )

        resp = requests.get(
            f"{api_url}/api/tasks",
            headers={"Authorization": f"Bearer {token}"},
            timeout=5,
        )
        resp.raise_for_status()
        stored = resp.json()["tasks"]
        titles = {t["title"] for t in stored}
        assert "Raft Log Replication Lab" in titles
        assert any("SDN" in title for title in titles)
        assert "Firewall Policy Review" in titles
        assert all(t["ics_file"] for t in stored)
    finally:
        lms_server.shutdown()
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
