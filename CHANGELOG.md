# Changelog

All notable changes to `academic-agent` are documented in this file.

The format follows Keep a Changelog, and this project adheres to Semantic Versioning.

## [0.3.0] - 2026-11-27

### Added

- Multi-event RFC 5545 iCalendar subscription feed generator (`academic_core::calendar::generate_ics_feed` and `filter_tasks_for_feed`) with deterministic task UIDs, `X-WR-CALNAME` / `X-WR-TIMEZONE` / `REFRESH-INTERVAL` headers, dual alarms on open tasks, and RFC 5545 text escaping.
- Live feed exposure across every interface: `GET /api/calendar/feed.ics` (supporting Bearer header or `?token=` query auth plus `?include_completed=` and `?course=` filters), MCP resource `academic://calendar/feed`, CLI `academic-agent calendar-feed`, and a one-click `Calendar Feed` button in the embedded dashboard header.
- Multi-stage non-root `Dockerfile`, `.dockerignore`, `docker-compose.yml`, and `.github/workflows/docker.yml` publishing GHCR images on semantic version tags.
- Package manifests for Windows Scoop ([packaging/scoop/academic-agent.json](packaging/scoop/academic-agent.json)) and macOS/Linux Homebrew ([packaging/homebrew/academic-agent.rb](packaging/homebrew/academic-agent.rb)) pinned with verified release SHA-256 checksums.
- Container deployment, distribution, and measured performance documentation ([docs/docker.md](docs/docker.md), [docs/distribution.md](docs/distribution.md), [docs/benchmarks.md](docs/benchmarks.md)).

### Changed

- Expanded read-only MCP resources to 5 total (4 static resources in `resources/list` including `academic://calendar/feed`, plus the `academic://calendar/{task_id}` template) and extended E2E tests (`serve_http_e2e`, `mcp_e2e`).
- Extended GitHub Actions CI validation to check `packaging/scoop/academic-agent.json`, `Dockerfile`, and `docker-compose.yml`.

## [0.2.0] - 2026-11-27

### Added

- Two-way Discord Bot Gateway (`academic_core::discord_bot`) over WebSocket (`intents: 0`) handling `/tasks`, `/upcoming`, `/done`, `/snooze`, `/add`, and `/digest` slash commands plus `Mark Done` (`academic:done:<id>`) and `Snooze 24h` (`academic:snooze24:<id>`) button callbacks, with `[discord]` allowlist support (`allowed_users`), `application_id`, `guild_id`, and `gateway_url`.
- New CLI commands `academic-agent discord register-commands [--guild]`, `academic-agent discord bot`, and `academic-agent serve --with-discord-bot`.
- Embedded single-page local web task dashboard served at `GET /` with live summary cards (`Open Tasks`, `Due Within 72h`, `Overdue`, `Completed`), filters, document upload modal, manual task creation modal, `.ics` / JSON / CSV export, Bearer token entry, and `POST /api/cron-tick`.
- Optional Python Playwright LMS connector (`connectors/lms-playwright/`) featuring configurable portal CSS selector profiles (`mock-lms.json`, `moodle-template.json`, `campus-generic.json`), persistent session cookie reuse, attachment downloads, a self-contained simulated campus LMS (`mock_lms.py`), offline pytest E2E tests, and dedicated setup guide ([docs/lms-playwright.md](docs/lms-playwright.md)).
- High-DPI dashboard screenshots (`docs/assets/dashboard-overview.png`, `docs/assets/dashboard-detail.png`) and GitHub Actions `connector` CI job.

### Changed

- Attached `Mark Done` and `Snooze 24h` action buttons alongside Google Calendar links on task creation embeds and individual cron reminder alerts in Bot mode.
- Updated `README.md`, `docs/configuration.md`, `docs/discord-and-hermes.md`, and `CHANGELOG.md` for `v0.2.0`.

## [0.1.1] - 2026-11-27

### Added

- Automated multi-OS GitHub Release workflow (`.github/workflows/release.yml`) publishing prebuilt binaries for Linux, Windows, and macOS (Intel and Apple Silicon) along with `SHA256SUMS.txt` on every `v*.*.*` tag.
- Prebuilt release installation instructions in `README.md`.

### Changed

- Dynamic `CARGO_PKG_VERSION` string in Discord Bot API `User-Agent` header.

## [0.1.0] - 2026-11-27

### Added

- Clean Rust workspace split into `academic-core`, `academic-mcp`, and `academic-cli` producing a single `academic-agent` binary.
- Pure-Rust document parsing for `.pdf` (`lopdf`), `.docx` paragraphs and tables (`zip` + `quick-xml`), `.txt`, and `.md`, with explicit rejection of unsupported binary `.doc` files and strict path sanitization blocking traversal sequences.
- Hybrid extraction engine querying any Chat Completions HTTP endpoint with schema parsing and deterministic Indonesian/English heuristic fallback (`YYYY-MM-DD`, `DD/MM/YYYY`, Indonesian and English month names).
- Timezone-aware RFC 5545 `.ics` generator (`-P1D` and `-PT1H` alarms) and Google Calendar 1-click links converted accurately from configured local timezone to UTC and bounded within Discord's 512-character button limit.
- Embedded transactional `redb` task store (`TASKS`, `CONTENT_HASHES`, `SENT_REMINDERS`, `META`) with SHA-256 content deduplication, snooze timestamps, and JSON/CSV export.
- Cron reminder scheduler supporting configurable local-time daily digests (`digest_cron`), anti-spam reminder windows (`reminder_hours`), and overdue task detection.
- Discord REST API v10 (Bot token with link buttons) and Webhook transport with field length clamping and HTTP `429 Retry-After` backoff, plus configurable Hermes CLI and HMAC-signed outbound n8n bridge support.
- Axum HTTP server (`academic-agent serve`) with optional Bearer auth, origin allowlist, multipart upload sanitization, traversal-safe calendar downloads, incoming `webdelta` HMAC-SHA256 webhook verification, optional background `--with-cron` / `--with-watch` loops, and mounted `POST /mcp`.
- Built-in JSON-RPC 2.0 MCP server (`stdio` and Streamable HTTP) exposing 8 tools and 4 read-only `academic://` resources.
- Offline unit, CLI E2E, MCP `stdio` E2E, and Axum HTTP E2E tests, fixed `workflows/academic-agent.n8n.json`, GitHub Actions CI, and English documentation.
