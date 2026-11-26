# Optional Playwright LMS Connector

The optional Python Playwright connector under `connectors/lms-playwright/` bridges JavaScript-heavy university LMS portals into `academic-agent serve` without bundling a large browser runtime inside the core Rust binary.

## Why Keep Browser Automation Optional

Most workflows rely on folder watching, manual CLI scans, REST uploads, or `webdelta` page monitoring, all of which run natively inside the single Rust binary (`academic-agent`). Keeping Playwright in a standalone directory lets users who need authenticated portal scraping install browser automation only when required.

## Installation

Install the Python dependencies inside a virtual environment or user environment:

```bash
cd connectors/lms-playwright
pip install -r requirements.txt
```

`connector.py` automatically checks (in order):

1. `PLAYWRIGHT_CHROMIUM_EXECUTABLE` if explicitly set.
2. Installed system Google Chrome or Microsoft Edge executables on Windows, macOS, or Linux.
3. Playwright's managed Chromium binary (`playwright install chromium`).

If no Chrome/Edge browser is already installed on your host, download Playwright's browser once:

```bash
playwright install chromium
```

## Running the Simulated Campus LMS Demo

A self-contained simulated university portal (`mock_lms.py`) is included so you can verify authenticated scraping, attachment downloads, session cookie reuse, and end-to-end ingestion into Rust without accessing any real campus server.

Start `academic-agent serve` in Terminal 1:

```bash
academic-agent serve --bind 127.0.0.1:8080
```

Run the Playwright demo sync in Terminal 2:

```bash
python connectors/lms-playwright/connector.py demo --api-url http://127.0.0.1:8080
```

To run the full automated offline test suite (including end-to-end delivery into `academic-agent serve`):

```bash
python -m pytest connectors/lms-playwright/tests/test_lms_connector.py -v
```

## Adapting to Your University Portal

1. Copy one of the starter selector manifests (`profiles/moodle-template.json` or `profiles/campus-generic.json`) to a gitignored local profile such as `profiles/my-campus.local.json`.
2. Update the CSS selectors (`username_selector`, `password_selector`, `submit_selector`, `logged_in_selector`, `course_paths`, `item_selector`, `title_selector`, `deadline_selector`, `attachment_selector`) to match your campus portal DOM.
3. Authenticate interactively once and save browser cookies to a gitignored state file:

```bash
python connectors/lms-playwright/connector.py login \
  --profile connectors/lms-playwright/profiles/my-campus.local.json \
  --state connectors/lms-playwright/state.local.json
```

4. Run headless scheduled syncs using the saved session and Bearer token:

```bash
python connectors/lms-playwright/connector.py sync \
  --profile connectors/lms-playwright/profiles/my-campus.local.json \
  --state connectors/lms-playwright/state.local.json \
  --api-url http://127.0.0.1:8080 \
  --token "${ACADEMIC_API_TOKEN}"
```

## Security and Repository Hygiene

- `connectors/lms-playwright/state*.json` and `connectors/lms-playwright/profiles/*.local.json` are excluded in `.gitignore` so portal URLs, session cookies, and credentials are never committed to Git.
- Pass `--token` (or `ACADEMIC_API_TOKEN`) whenever `academic-agent serve` exposes a non-empty `[server].api_token`.
- Downloaded assignment attachments (`.pdf`, `.docx`, `.txt`, `.md`) are uploaded via `POST /api/upload`, while inline text items are normalized via `POST /api/tasks`. Both paths share `redb` SHA-256 deduplication so repeated scheduled syncs never trigger duplicate Discord alerts.
