# Optional Playwright LMS Connector

This directory provides a standalone Python Playwright connector that logs into JavaScript-heavy university LMS portals, collects assignments and downloadable briefs (`.pdf`, `.docx`, `.txt`, `.md`), and forwards them into `academic-agent serve`.

Keeping Playwright optional outside the core Rust binary avoids forcing a 400 MB browser download on users who only rely on folder watching or webhooks.

## Setup

```bash
cd connectors/lms-playwright
pip install -r requirements.txt
# Optional if Chrome or Edge is not already installed on your machine:
playwright install chromium
```

`connector.py` automatically checks `PLAYWRIGHT_CHROMIUM_EXECUTABLE`, installed Chrome/Edge paths, and Playwright's bundled browser.

## Testing Against the Simulated Campus LMS

Run the self-contained simulator demo without touching any real campus portal:

```bash
# Terminal 1: start academic-agent REST API and dashboard
academic-agent serve --bind 127.0.0.1:8080

# Terminal 2: run the simulated LMS + Playwright sync demo
python connector.py demo --api-url http://127.0.0.1:8080
```

Or run the automated test suite:

```bash
python -m pytest tests/test_lms_connector.py -v
```

## Adapting to Your Own Campus LMS

1. Copy `profiles/moodle-template.json` or `profiles/campus-generic.json` to `profiles/my-campus.local.json` (matching `.gitignore` so private campus URLs never enter Git).
2. Authenticate interactively once and persist cookies to a gitignored state file:

```bash
python connector.py login --profile profiles/my-campus.local.json --state state.local.json
```

3. Run scheduled syncs headless with the saved session:

```bash
python connector.py sync --profile profiles/my-campus.local.json --state state.local.json --api-url http://127.0.0.1:8080 --token "${ACADEMIC_API_TOKEN}"
```
