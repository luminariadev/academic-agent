# Model Context Protocol (MCP) Integration

`academic-agent` includes a built-in JSON-RPC 2.0 MCP server supporting `stdio` and Streamable HTTP (`POST /mcp`).

## Starting the Server

### Standard I/O (`stdio`)

```bash
academic-agent mcp --config academic-agent.toml
```

Logs are routed to `stderr` so `stdout` carries strictly newline-delimited JSON-RPC 2.0 messages.

### Streamable HTTP (`POST /mcp`)

Standalone MCP HTTP endpoint:

```bash
academic-agent mcp --http 127.0.0.1:8081 --token "${ACADEMIC_API_TOKEN}"
```

Or embedded directly inside `academic-agent serve` (sharing the single `redb` database handle with the REST API, cron scheduler, and folder watcher):

```bash
academic-agent serve --bind 127.0.0.1:8080 --token "${ACADEMIC_API_TOKEN}" --with-cron
```

## Available Tools (10)

| Tool | Required Args | Optional Args | Description |
| :--- | :--- | :--- | :--- |
| `list_tasks` | none | `status` | List tracked tasks ordered by deadline (`PENDING`, `IN_PROGRESS`, `COMPLETED`, `ARCHIVED`). |
| `add_task` | `title` | `course`, `kind`, `deadline`, `priority`, `estimated_hours`, `summary`, `subtasks`, `deliverables`, `notify` | Register a task manually and generate an `.ics` file. |
| `update_task` | `task_id` | `title`, `course`, `kind`, `deadline`, `priority`, `status`, `estimated_hours`, `summary`, `subtasks`, `subtasks_done`, `deliverables` | Edit task fields, toggle subtask checklist indices, and refresh `.ics` when schedule changes. |
| `update_status` | `task_id`, `status` | none | Change a task status. |
| `snooze_task` | `task_id` | `hours` | Suppress cron deadline reminders for N hours (default `24`). |
| `scan_document` | `path` or `text` | `filename`, `heuristic`, `notify` | Parse `.pdf`/`.docx`/`.txt`/`.md` or raw text into a tracked task. |
| `upcoming_deadlines` | none | `within_hours` | List incomplete tasks due within N hours (default `72`). |
| `export_ics` | `task_id` | none | Generate/read RFC 5545 `.ics` content and Google Calendar link for a task. |
| `send_digest` | none | `dry_run` | Summarize open tasks (`dry_run` defaults to `true`). |
| `sync_lms` | none | `source`, `url`, `kind`, `token`, `course`, `name`, `notify` | Pull tasks from configured `[lms.sources]` or an ad-hoc `.ics`/Moodle/Canvas endpoint. |

## Read-Only Resources (5)

| URI / Template | MIME Type | Content |
| :--- | :--- | :--- |
| `academic://tasks` | `application/json` | Full JSON list of all stored tasks. |
| `academic://upcoming` | `application/json` | Incomplete tasks due within 72 hours or already overdue. |
| `academic://config` | `text/plain` | Active TOML configuration with literal secrets redacted. |
| `academic://calendar/feed` | `text/calendar` | Live RFC 5545 `.ics` subscription feed of all open tasks with deadlines. |
| `academic://calendar/{task_id}` | `text/calendar` | RFC 5545 `.ics` calendar payload for `{task_id}`. |
