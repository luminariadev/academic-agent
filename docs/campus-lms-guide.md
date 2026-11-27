# Campus and LMS Integration Guide

This guide walks students, lecturers, and university IT teams through setting up `academic-agent` with real campus Learning Management Systems (Moodle, SPADA, Canvas, Google Classroom, and custom SIAKAD portals).

## Quick Decision Matrix: Which Connection Path Should I Use?

| Your Situation | Recommended Path | Setup Time | Requires IT Permission? |
| :--- | :--- | :--- | :--- |
| You have a student or lecturer LMS account (Moodle, SPADA, Canvas, Google Classroom) | **Path 1: Exported `.ics` Calendar Feed** | ~30 seconds | No |
| Your campus enables Moodle Web Services (`wstoken`) or Canvas API tokens | **Path 2: Native Moodle / Canvas REST API** | ~2 minutes | Only a personal API token |
| Your campus uses a custom SIAKAD portal without `.ics` export or public REST API | **Path 3: Optional Playwright Connector + `discover`** | ~5 minutes | No |

## Guide for Students (30-Second Setup)

### Step 1: Launch `academic-agent`

Run the setup wizard or start the local server directly:

```bash
academic-agent setup
academic-agent serve --with-cron --with-lms-sync
```

Or launch with automatic starter config creation:

```bash
academic-agent serve --init-if-missing --with-cron --with-lms-sync
```

Open `http://127.0.0.1:8080/` in your browser. Click **ID / EN** in the header to switch the interface language at any time.

### Step 2: Copy Your LMS Calendar Link

- **Moodle / SPADA**: Sign in to LMS -> open **Calendar** -> scroll to the bottom and click **Export calendar** -> select **All events** and **Custom range / This month** -> click **Get calendar URL** and copy the link.
- **Canvas**: Sign in -> open **Calendar** -> click **Calendar Feed** in the bottom-right sidebar and copy the `.ics` link.

### Step 3: Connect Inside the Dashboard

1. Click **Connect LMS** in the dashboard header.
2. Select **Exported .ics Calendar Feed**.
3. Paste your calendar link and click **Sync Now**.
4. Your deadlines appear immediately in both **List** view and **Calendar** view, complete with urgent countdown badges and interactive subtask checklists.

### Step 4: Subscribe Your Phone Calendar and Verify Health

- Click **Calendar Feed** in the header to copy `http://127.0.0.1:8080/api/calendar/feed.ics` (plus `?token=...` if auth is set) and add it by URL inside Google Calendar, Apple Calendar, or Outlook.
- Click **Health** in the header (or run `academic-agent doctor`) to confirm timezone, database, Discord alerts, and LMS sync status.

## Guide for Lecturers (Sharing Course Feeds with a Class)

Lecturers can turn a course syllabus PDF or DOCX into a continuously updated schedule link for an entire class:

1. Upload your syllabus, RPS, or assignment sheet using the **Upload** drag-and-drop box in the web dashboard (or run `academic-agent scan syllabus.pdf`).
2. Click **Details** -> **Edit Task** to adjust deadlines, priorities, or itemized subtasks (`subtasks`). Changes regenerate `.ics` artifacts automatically.
3. Select the course in the **Course** filter dropdown and click **Share Course Feed** to copy `/api/calendar/feed.ics?course=<CourseName>`.
4. Share that course feed URL or enable Discord Bot notifications (`academic-agent serve --with-cron --with-discord-bot`) in your class server so students see deadline updates automatically.

## Guide for Campus IT and Custom SIAKAD Portals

### Docker Deployment

Deploy a shared or personal container running as non-root user `academic`:

```bash
mkdir -p data/inbox data/archive data/calendar
academic-agent setup --non-interactive --role student --timezone Asia/Jakarta --config data/academic-agent.toml
docker compose up -d --build
```

Verify readiness inside or outside the container:

```bash
academic-agent --config data/academic-agent.toml doctor
```

### Moodle Web Services and Canvas REST API

If your university prefers REST APIs over `.ics` calendar URLs, add a `[[lms.sources]]` block to `academic-agent.toml`:

```toml
[lms]
enabled = true
sync_interval_secs = 900
notify_on_sync = true

[[lms.sources]]
name = "spada-moodle"
kind = "moodle_api"
url = "https://spada.campus.ac.id"
token = "${MOODLE_WS_TOKEN}"
default_course = "Campus Moodle"
enabled = true
```

Run a one-shot sync test from CLI:

```bash
academic-agent lms sync --source spada-moodle
```

### Custom SIAKAD Portals (Playwright Connector + `discover`)

Many campus SIAKAD portals render grades, announcements, and assignments behind custom session logins without `.ics` exports or REST endpoints. Use the optional Playwright connector to auto-detect login form selectors first:

```bash
python connectors/lms-playwright/connector.py discover \
  --url https://siakad.campus.ac.id/login \
  --output connectors/lms-playwright/profiles/my-siakad.local.json
```

Compare the generated file against [connectors/lms-playwright/profiles/siakad-custom-template.json](../connectors/lms-playwright/profiles/siakad-custom-template.json) to adjust assignment card selectors (`item_selector`, `title_selector`, `course_selector`, `deadline_selector`), authenticate interactively once to cache session cookies:

```bash
python connectors/lms-playwright/connector.py login \
  --profile connectors/lms-playwright/profiles/my-siakad.local.json \
  --state connectors/lms-playwright/state.local.json
```

Then run scheduled headless syncs into `academic-agent serve`:

```bash
python connectors/lms-playwright/connector.py sync \
  --profile connectors/lms-playwright/profiles/my-siakad.local.json \
  --state connectors/lms-playwright/state.local.json \
  --api-url http://127.0.0.1:8080
```

All `profiles/*.local.json` and `state*.json` files remain excluded by `.gitignore`.
