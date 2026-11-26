# Configuration Reference

`academic-agent` reads settings from `academic-agent.toml` (generated via `academic-agent init`) and expands `${VAR_NAME}` placeholders from environment variables or an adjacent `.env` file.

## Sections

### `[general]`

| Key | Default | Description |
| :--- | :--- | :--- |
| `inbox_dir` | `"inbox"` | Directory monitored by `academic-agent watch`. |
| `archive_dir` | `"archive"` | Destination for copied documents after ingestion. |
| `calendar_dir` | `"data/calendar"` | Directory where RFC 5545 `.ics` files are stored. |
| `db_path` | `"data/state.redb"` | Embedded `redb` database file path. |
| `timezone` | `"Asia/Jakarta"` | IANA timezone used for deadline parsing, cron digests, and UTC conversions. |
| `backup_processed` | `true` | Copy newly ingested files from `ingest_file` into `archive_dir`. |
| `default_duration_hours` | `2` | Calendar event duration ending at the parsed deadline. |

### `[llm]`

| Key | Default | Description |
| :--- | :--- | :--- |
| `api_base` | `"${ACADEMIC_LLM_API_BASE}"` | Base URL for any Chat Completions HTTP API (falls back to `ROUTER_API_BASE` if set). |
| `api_key` | `"${ACADEMIC_LLM_API_KEY}"` | Bearer token (falls back to `ROUTER_API_KEY`). |
| `model` | `"gpt-4o-mini"` | Model identifier sent in completion requests (or `ROUTER_MODEL` env override). |
| `timeout_secs` | `45` | Request timeout in seconds. |
| `fallback_to_heuristic` | `true` | Fall back to bilingual heuristic extraction if LLM inference fails. |
| `temperature` | `0.1` | Sampling temperature. |

### `[discord]`

| Key | Default | Description |
| :--- | :--- | :--- |
| `enabled` | `true` | Enable Discord dispatch. |
| `mode` | `"auto"` | `auto` (prefers Bot API when credentials exist, else Webhook), `bot`, or `webhook`. |
| `bot_token` | `"${DISCORD_BOT_TOKEN}"` | Discord Bot token for REST API v10 and Gateway WebSocket. |
| `channel_id` | `"${DISCORD_CHANNEL_ID}"` | Target Discord channel snowflake ID. |
| `application_id` | `"${DISCORD_APPLICATION_ID}"` | Discord Application ID required for slash command registration. |
| `guild_id` | `"${DISCORD_GUILD_ID}"` | Optional Discord server (guild) snowflake ID for instant guild-scoped slash command sync. |
| `webhook_url` | `"${DISCORD_WEBHOOK_URL}"` | Incoming Discord Webhook URL alternative. |
| `allowed_users` | `[]` | Allowlist of Discord user IDs permitted to invoke slash commands and buttons (empty allows all server members). |
| `gateway_url` | `""` | Optional custom Discord Gateway WebSocket URL override (defaults to `wss://gateway.discord.gg/?v=10&encoding=json`). |
| `timeout_secs` | `15` | HTTP timeout per attempt. |
| `max_retries` | `3` | Retry count on HTTP `429 Retry-After` and 5xx errors. |

### `[hermes]`

| Key | Default | Description |
| :--- | :--- | :--- |
| `enabled` | `false` | Enable sending alerts via the `hermes` CLI binary. |
| `binary_path` | `"hermes"` | Executable name or full path. |
| `target` | `"${HERMES_TARGET}"` | Required dispatch target when enabled (for example `discord:alerts`). |
| `timeout_secs` | `15` | Subprocess execution timeout. |
| `fallback_on_discord_error` | `false` | Only invoke Hermes if Discord dispatch failed. |

### `[cron]`

| Key | Default | Description |
| :--- | :--- | :--- |
| `enabled` | `true` | Enable cron digest and reminder evaluation. |
| `digest_cron` | `"0 7 * * *"` | Standard 5-field (or 6-field) cron expression evaluated in `[general].timezone`. |
| `reminder_hours` | `[72, 24, 3]` | Alert tiers in hours before deadline. |
| `alert_overdue` | `true` | Send one-time alert if an incomplete task passes its deadline. |
| `tick_interval_secs` | `60` | Background polling interval in `academic-agent serve --with-cron`. |

### `[server]` and `[n8n]`

| Key | Default | Description |
| :--- | :--- | :--- |
| `server.bind` | `"127.0.0.1:8080"` | Bind address for `academic-agent serve`. |
| `server.api_token` | `"${ACADEMIC_API_TOKEN}"` | Optional Bearer secret protecting `/api/*` and `/mcp`. |
| `server.allowed_origins` | `["http://127.0.0.1:8080", "http://localhost:8080"]` | Allowed CORS origins. |
| `server.max_upload_bytes` | `16777216` | Maximum multipart upload payload size (16 MiB). |
| `server.webdelta_webhook_secret` | `"${WEBDELTA_WEBHOOK_SECRET}"` | Secret verifying `X-WebDelta-Signature` on `/api/webhooks/webdelta`. |
| `n8n.enabled` | `false` | Forward newly extracted tasks to an n8n webhook. |
| `n8n.webhook_url` | `"${N8N_WEBHOOK_URL}"` | Target n8n webhook URL. |
| `n8n.secret` | `"${N8N_WEBHOOK_SECRET}"` | Optional HMAC-SHA256 signing secret sent in `X-Academic-Signature`. |
