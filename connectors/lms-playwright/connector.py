"""
Optional Playwright LMS connector for academic-agent.
Logs into a simulated or configured campus portal, scrapes assignments and attachments,
and forwards them to `academic-agent serve` via `/api/upload` or `/api/tasks`.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import time
import urllib.parse
from pathlib import Path
from typing import Any, Dict, List, Optional

import requests
from playwright.sync_api import sync_playwright

SUPPORTED_UPLOAD_EXTS = {".pdf", ".docx", ".txt", ".md"}


def resolve_chromium_executable() -> Optional[str]:
    """Finds an installed Chromium/Chrome/Edge executable or falls back to Playwright's bundle."""
    env_override = os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE", "").strip()
    if env_override and Path(env_override).is_file():
        return env_override

    candidates = [
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        str(Path.home() / r"AppData\Local\Google\Chrome\Application\chrome.exe"),
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        "/usr/bin/google-chrome",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    ]
    for path in candidates:
        if Path(path).is_file():
            return path
    return None


def load_profile(profile_path: Path, base_url_override: Optional[str] = None) -> Dict[str, Any]:
    with open(profile_path, "r", encoding="utf-8") as fh:
        profile = json.load(fh)
    if base_url_override:
        profile["base_url"] = base_url_override.rstrip("/")
    else:
        profile["base_url"] = str(profile["base_url"]).rstrip("/")
    return profile


def safe_text(locator) -> str:
    try:
        if locator.count() == 0:
            return ""
        return locator.first.inner_text().strip()
    except Exception:
        return ""


def filename_from_headers(url: str, headers: Dict[str, str]) -> str:
    disp = headers.get("content-disposition") or headers.get("Content-Disposition") or ""
    match = re.search(r'filename="?([^";]+)"?', disp)
    if match:
        return Path(match.group(1).strip()).name
    parsed_name = Path(urllib.parse.urlparse(url).path).name
    return parsed_name or "lms_attachment.txt"


def ensure_logged_in(
    page,
    context,
    profile: Dict[str, Any],
    username: Optional[str],
    password: Optional[str],
    storage_state_path: Optional[Path],
    headless: bool,
    manual_wait_secs: int = 120,
) -> None:
    base_url = profile["base_url"]
    dash_url = urllib.parse.urljoin(base_url + "/", profile.get("dashboard_path", "/dashboard").lstrip("/"))
    logged_in_sel = profile["logged_in_selector"]

    page.goto(dash_url, wait_until="domcontentloaded")
    if page.locator(logged_in_sel).count() > 0:
        return

    login_url = urllib.parse.urljoin(base_url + "/", profile["login_path"].lstrip("/"))
    page.goto(login_url, wait_until="domcontentloaded")

    if username and password:
        page.fill(profile["username_selector"], username)
        page.fill(profile["password_selector"], password)
        page.click(profile["submit_selector"])
        page.wait_for_selector(logged_in_sel, timeout=15_000)
    elif not headless:
        print(f"Complete login in the opened browser window ({login_url})...")
        deadline = time.monotonic() + manual_wait_secs
        while time.monotonic() < deadline:
            if page.locator(logged_in_sel).count() > 0:
                break
            time.sleep(0.5)
        if page.locator(logged_in_sel).count() == 0:
            raise TimeoutError("Timed out waiting for manual LMS login.")
    else:
        raise RuntimeError(
            "Not logged in and no credentials or valid storage_state were provided."
        )

    if storage_state_path:
        storage_state_path.parent.mkdir(parents=True, exist_ok=True)
        context.storage_state(path=str(storage_state_path))


def scrape_courses(page, context, profile: Dict[str, Any]) -> List[Dict[str, Any]]:
    base_url = profile["base_url"]
    items: List[Dict[str, Any]] = []

    for course_path in profile.get("course_paths", []):
        course_url = urllib.parse.urljoin(base_url + "/", course_path.lstrip("/"))
        page.goto(course_url, wait_until="domcontentloaded")
        cards = page.locator(profile["item_selector"]).all()

        for card in cards:
            title = safe_text(card.locator(profile["title_selector"]))
            course = safe_text(card.locator(profile["course_selector"]))
            deadline = safe_text(card.locator(profile["deadline_selector"]))
            summary = safe_text(card.locator(profile["summary_selector"]))

            att_name: Optional[str] = None
            att_bytes: Optional[bytes] = None
            att_sel = profile.get("attachment_selector")
            if att_sel:
                att_loc = card.locator(att_sel)
                if att_loc.count() > 0:
                    href = att_loc.first.get_attribute("href")
                    if href:
                        abs_url = urllib.parse.urljoin(page.url, href)
                        resp = context.request.get(abs_url)
                        if resp.ok:
                            cand_name = filename_from_headers(abs_url, dict(resp.headers))
                            if Path(cand_name).suffix.lower() in SUPPORTED_UPLOAD_EXTS:
                                att_name = cand_name
                                att_bytes = resp.body()

            items.append(
                {
                    "title": title or "LMS Assignment",
                    "course": course or "Campus Course",
                    "deadline": deadline or None,
                    "summary": summary or title,
                    "source_url": course_url,
                    "attachment_name": att_name,
                    "attachment_bytes": att_bytes,
                }
            )

    return items


def forward_item(
    item: Dict[str, Any],
    api_url: str,
    token: Optional[str],
    heuristic: bool = True,
    notify: bool = False,
    dry_run: bool = False,
) -> Dict[str, Any]:
    headers: Dict[str, str] = {}
    if token:
        headers["Authorization"] = f"Bearer {token}"

    if dry_run:
        return {
            "mode": "dry_run",
            "title": item["title"],
            "course": item["course"],
            "deadline": item["deadline"],
            "attachment": item["attachment_name"],
        }

    base = api_url.rstrip("/")
    if item.get("attachment_bytes") and item.get("attachment_name"):
        resp = requests.post(
            f"{base}/api/upload",
            params={"heuristic": str(heuristic).lower(), "notify": str(notify).lower()},
            headers=headers,
            files={"file": (item["attachment_name"], item["attachment_bytes"])},
            timeout=30,
        )
        resp.raise_for_status()
        return resp.json()

    resp = requests.post(
        f"{base}/api/tasks",
        headers=headers,
        json={
            "title": item["title"],
            "course": item["course"],
            "deadline": item["deadline"],
            "priority": "MEDIUM",
            "summary": item["summary"],
            "notify": notify,
        },
        timeout=30,
    )
    resp.raise_for_status()
    return resp.json()


def sync_lms(
    profile_path: Path,
    api_url: str,
    token: Optional[str] = None,
    base_url_override: Optional[str] = None,
    storage_state_path: Optional[Path] = None,
    username: Optional[str] = None,
    password: Optional[str] = None,
    headless: bool = True,
    heuristic: bool = True,
    notify: bool = False,
    dry_run: bool = False,
) -> List[Dict[str, Any]]:
    profile = load_profile(profile_path, base_url_override=base_url_override)
    executable = resolve_chromium_executable()

    launch_kwargs: Dict[str, Any] = {"headless": headless}
    if executable:
        launch_kwargs["executable_path"] = executable

    ctx_kwargs: Dict[str, Any] = {}
    if storage_state_path and storage_state_path.exists():
        ctx_kwargs["storage_state"] = str(storage_state_path)

    results: List[Dict[str, Any]] = []
    with sync_playwright() as p:
        browser = p.chromium.launch(**launch_kwargs)
        try:
            context = browser.new_context(**ctx_kwargs)
            page = context.new_page()
            ensure_logged_in(
                page=page,
                context=context,
                profile=profile,
                username=username,
                password=password,
                storage_state_path=storage_state_path,
                headless=headless,
            )
            scraped = scrape_courses(page, context, profile)
            for item in scraped:
                res = forward_item(
                    item,
                    api_url=api_url,
                    token=token,
                    heuristic=heuristic,
                    notify=notify,
                    dry_run=dry_run,
                )
                results.append(res)
        finally:
            browser.close()

    return results


def main() -> None:
    parser = argparse.ArgumentParser(description="Playwright LMS connector for academic-agent")
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_login = sub.add_parser("login", help="Sign in to LMS and save Playwright storage state")
    p_login.add_argument("--profile", type=Path, required=True)
    p_login.add_argument("--state", type=Path, default=Path("state.json"))
    p_login.add_argument("--base-url")
    p_login.add_argument("--username")
    p_login.add_argument("--password")
    p_login.add_argument("--headless", action="store_true")

    p_sync = sub.add_parser("sync", help="Scrape assignments and forward to academic-agent serve")
    p_sync.add_argument("--profile", type=Path, required=True)
    p_sync.add_argument("--api-url", default="http://127.0.0.1:8080")
    p_sync.add_argument("--token", default=os.environ.get("ACADEMIC_API_TOKEN", ""))
    p_sync.add_argument("--state", type=Path, default=Path("state.json"))
    p_sync.add_argument("--base-url")
    p_sync.add_argument("--username")
    p_sync.add_argument("--password")
    p_sync.add_argument("--headed", action="store_true")
    p_sync.add_argument("--heuristic", action="store_true", default=True)
    p_sync.add_argument("--notify", action="store_true")
    p_sync.add_argument("--dry-run", action="store_true")

    p_demo = sub.add_parser("demo", help="Run offline simulation against bundled mock_lms.py")
    p_demo.add_argument("--api-url", default="http://127.0.0.1:8080")
    p_demo.add_argument("--token", default=os.environ.get("ACADEMIC_API_TOKEN", ""))
    p_demo.add_argument("--dry-run", action="store_true")

    args = parser.parse_args()

    if args.cmd == "login":
        profile = load_profile(args.profile, base_url_override=args.base_url)
        executable = resolve_chromium_executable()
        launch_kwargs: Dict[str, Any] = {"headless": args.headless}
        if executable:
            launch_kwargs["executable_path"] = executable
        with sync_playwright() as p:
            browser = p.chromium.launch(**launch_kwargs)
            try:
                ctx = browser.new_context()
                page = ctx.new_page()
                ensure_logged_in(
                    page=page,
                    context=ctx,
                    profile=profile,
                    username=args.username,
                    password=args.password,
                    storage_state_path=args.state,
                    headless=args.headless,
                )
                print(f"Saved authenticated session state to {args.state}")
            finally:
                browser.close()
        return

    if args.cmd == "sync":
        out = sync_lms(
            profile_path=args.profile,
            api_url=args.api_url,
            token=args.token or None,
            base_url_override=args.base_url,
            storage_state_path=args.state,
            username=args.username,
            password=args.password,
            headless=not args.headed,
            heuristic=args.heuristic,
            notify=args.notify,
            dry_run=args.dry_run,
        )
        print(json.dumps(out, indent=2, default=str))
        return

    if args.cmd == "demo":
        from mock_lms import DEMO_PASS, DEMO_USER, start_mock_lms_in_thread

        server, base_url = start_mock_lms_in_thread()
        try:
            profile_path = Path(__file__).resolve().parent / "profiles" / "mock-lms.json"
            out = sync_lms(
                profile_path=profile_path,
                api_url=args.api_url,
                token=args.token or None,
                base_url_override=base_url,
                username=DEMO_USER,
                password=DEMO_PASS,
                headless=True,
                dry_run=args.dry_run,
            )
            print(json.dumps(out, indent=2, default=str))
        finally:
            server.shutdown()


if __name__ == "__main__":
    main()
