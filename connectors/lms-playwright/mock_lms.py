"""
Simulated Campus LMS web portal used for offline testing and local demonstrations
of the Playwright LMS connector.
"""

from __future__ import annotations

import argparse
import html
import io
import threading
import urllib.parse
import zipfile
from http.cookies import SimpleCookie
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Tuple


SESSION_COOKIE = "lms_session"
SESSION_VALUE = "mock-student-session"
DEMO_USER = "student"
DEMO_PASS = "campus-demo"


def build_sample_docx() -> bytes:
    """Builds a minimal valid .docx archive in memory using only Python stdlib."""
    doc_xml = """<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:r><w:t>Mata Kuliah: Jaringan Komputer Lanjut</w:t></w:r></w:p>
    <w:p><w:r><w:t>Judul: Kuis 2 SDN dan Protokol BGP</w:t></w:r></w:p>
    <w:p><w:r><w:t>Deadline: 2026-12-14 10:00</w:t></w:r></w:p>
    <w:p><w:r><w:t>Ketentuan: Kerjakan daring dan unggah ringkasan PDF.</w:t></w:r></w:p>
  </w:body>
</w:document>"""
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", compression=zipfile.ZIP_DEFLATED) as zf:
        zf.writestr(
            "[Content_Types].xml",
            '<?xml version="1.0" encoding="UTF-8"?>'
            '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
            '<Default Extension="xml" ContentType="application/xml"/>'
            '<Override PartName="/word/document.xml" '
            'ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>'
            "</Types>",
        )
        zf.writestr("word/document.xml", doc_xml)
    return buf.getvalue()


class MockLmsHandler(BaseHTTPRequestHandler):
    def log_message(self, format: str, *args) -> None:  # noqa: A002
        return

    def _is_authenticated(self) -> bool:
        raw_cookie = self.headers.get("Cookie", "")
        cookie = SimpleCookie()
        cookie.load(raw_cookie)
        morsel = cookie.get(SESSION_COOKIE)
        return morsel is not None and morsel.value == SESSION_VALUE

    def _redirect_login(self) -> None:
        self.send_response(302)
        self.send_header("Location", "/login")
        self.end_headers()

    def _send_html(self, body: str, status: int = 200) -> None:
        encoded = body.encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)

    def do_GET(self) -> None:  # noqa: N802
        parsed = urllib.parse.urlparse(self.path)
        route = parsed.path

        if route in ("/", "/login"):
            err = ""
            if "error=1" in parsed.query:
                err = '<p class="error">Invalid campus credentials.</p>'
            self._send_html(
                f"""<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>Campus LMS Sign In</title></head>
<body>
  <h1>Simulated Campus LMS</h1>
  {err}
  <form method="post" action="/login">
    <label>Student ID <input id="username" name="username" type="text" required></label>
    <label>Password <input id="password" name="password" type="password" required></label>
    <button id="login-btn" type="submit">Sign In</button>
  </form>
</body>
</html>"""
            )
            return

        if not self._is_authenticated():
            self._redirect_login()
            return

        if route == "/dashboard":
            self._send_html(
                """<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>LMS Dashboard</title></head>
<body>
  <header id="lms-user-menu">Signed in as student</header>
  <nav>
    <ul>
      <li><a class="course-link" href="/courses/cs301">CS301 - Distributed Systems</a></li>
      <li><a class="course-link" href="/courses/if402">IF402 - Jaringan Komputer Lanjut</a></li>
    </ul>
  </nav>
</body>
</html>"""
            )
            return

        if route == "/courses/cs301":
            self._send_html(
                """<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>CS301 Distributed Systems</title></head>
<body>
  <h1>Distributed Systems</h1>
  <article class="assignment-card">
    <h2 class="task-title">Raft Log Replication Lab</h2>
    <div class="task-course">Distributed Systems</div>
    <div class="task-deadline">2026-12-10 23:59</div>
    <p class="task-summary">Implement RequestVote and AppendEntries RPCs and verify partition recovery.</p>
    <a class="attachment-link" href="/files/raft_lab.txt">Download Assignment Brief</a>
  </article>
</body>
</html>"""
            )
            return

        if route == "/courses/if402":
            self._send_html(
                """<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>IF402 Jaringan Komputer Lanjut</title></head>
<body>
  <h1>Jaringan Komputer Lanjut</h1>
  <article class="assignment-card">
    <h2 class="task-title">Kuis 2 SDN dan BGP</h2>
    <div class="task-course">Jaringan Komputer Lanjut</div>
    <div class="task-deadline">2026-12-14 10:00</div>
    <p class="task-summary">Kuis daring materi Software-Defined Networking dan BGP routing.</p>
    <a class="attachment-link" href="/files/kuis_sdn.docx">Unduh Silabus Kuis (.docx)</a>
  </article>
  <article class="assignment-card">
    <h2 class="task-title">Firewall Policy Review</h2>
    <div class="task-course">Jaringan Komputer Lanjut</div>
    <div class="task-deadline">2026-12-19 23:59</div>
    <p class="task-summary">Submit written firewall policy analysis via portal text form.</p>
  </article>
</body>
</html>"""
            )
            return

        if route == "/files/raft_lab.txt":
            txt = (
                "Course: Distributed Systems\n"
                "Title: Raft Log Replication Lab\n"
                "Deadline: 2026-12-10 23:59\n"
                "Deliverable: GitHub repository and PDF benchmark report.\n"
            ).encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "text/plain; charset=utf-8")
            self.send_header("Content-Disposition", 'attachment; filename="raft_lab.txt"')
            self.send_header("Content-Length", str(len(txt)))
            self.end_headers()
            self.wfile.write(txt)
            return

        if route == "/files/kuis_sdn.docx":
            data = build_sample_docx()
            self.send_response(200)
            self.send_header(
                "Content-Type",
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            )
            self.send_header("Content-Disposition", 'attachment; filename="kuis_sdn.docx"')
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            return

        self._send_html(f"<p>Not found: {html.escape(route)}</p>", status=404)

    def do_POST(self) -> None:  # noqa: N802
        if self.path != "/login":
            self.send_response(404)
            self.end_headers()
            return
        length = int(self.headers.get("Content-Length", "0"))
        raw_body = self.rfile.read(length).decode("utf-8", errors="ignore")
        form = urllib.parse.parse_qs(raw_body)
        user = form.get("username", [""])[0].strip()
        passwd = form.get("password", [""])[0].strip()
        if user == DEMO_USER and passwd == DEMO_PASS:
            self.send_response(302)
            self.send_header(
                "Set-Cookie", f"{SESSION_COOKIE}={SESSION_VALUE}; Path=/; HttpOnly"
            )
            self.send_header("Location", "/dashboard")
            self.end_headers()
        else:
            self.send_response(302)
            self.send_header("Location", "/login?error=1")
            self.end_headers()


def start_mock_lms_in_thread(
    host: str = "127.0.0.1", port: int = 0
) -> Tuple[ThreadingHTTPServer, str]:
    """Starts the simulated LMS server in a daemon thread and returns `(server, base_url)`."""
    server = ThreadingHTTPServer((host, port), MockLmsHandler)
    actual_port = server.server_address[1]
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, f"http://{host}:{actual_port}"


def main() -> None:
    parser = argparse.ArgumentParser(description="Run local simulated Campus LMS server")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8099)
    args = parser.parse_args()

    server = ThreadingHTTPServer((args.host, args.port), MockLmsHandler)
    print(
        f"Simulated Campus LMS running at http://{args.host}:{args.port} "
        f"(demo user={DEMO_USER}, password={DEMO_PASS})"
    )
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        server.shutdown()


if __name__ == "__main__":
    main()
