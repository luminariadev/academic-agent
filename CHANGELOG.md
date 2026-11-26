# Changelog

All notable changes to `academic-agent` are documented in this file.

The format follows Keep a Changelog, and this project adheres to Semantic Versioning.

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
