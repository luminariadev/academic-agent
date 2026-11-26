# Discord and Hermes Integration

`academic-agent` uses Discord as its primary free interactive notification channel and supports Hermes CLI forwarding as either a standalone transport or a fallback.

## Option 1: Discord Bot Token (REST API v10 with Interactive Buttons)

Using a Discord Bot token enables rich color-coded embeds plus interactive action buttons (`Mark Done`, `Snooze 24h`, and **Add to Google Calendar**) under newly ingested tasks and individual deadline alerts.

1. Open the [Discord Developer Portal](https://discord.com/developers/applications) and create an application.
2. Under the **Bot** tab, copy or reset your Bot Token and store it as `DISCORD_BOT_TOKEN` in `.env`.
3. Under **OAuth2 -> URL Generator**, select scopes `bot` and `applications.commands`, grant permissions `Send Messages` and `Embed Links`, then invite the bot to your server.
4. Enable Developer Mode in Discord settings, right-click your target channel (for example `#academic-deadlines`), copy its ID, and set `DISCORD_CHANNEL_ID` in `.env`.
5. Verify delivery:

```bash
academic-agent test-discord
```

## Two-Way Slash Commands and Interactive Buttons (v0.2.0)

With `academic-agent` v0.2.0, the same Discord bot supports two-way control over your task store via a persistent Discord Gateway WebSocket connection alongside outbound REST API v10 embeds.

### Setup

1. Copy your Application ID from the **General Information** page in the Discord Developer Portal and set `DISCORD_APPLICATION_ID` in `.env`.
2. (Recommended for fast command propagation) Set `DISCORD_GUILD_ID` to your Discord server ID, or omit it to register commands globally.
3. Optionally configure `[discord].allowed_users` in `academic-agent.toml` with your Discord user ID so only approved accounts can mutate tasks.
4. No privileged Gateway intents (`Message Content`, `Server Members`, or `Presence`) are required: `academic-agent` connects with `intents: 0` and handles `INTERACTION_CREATE` events directly.

### Register Slash Commands and Start the Listener

Register slash commands once (pass `--guild` to update a single server immediately):

```bash
academic-agent discord register-commands --guild
```

Run the interactive Gateway listener standalone or embedded inside `serve`:

```bash
# Standalone Gateway listener
academic-agent discord bot

# Combined dashboard, REST API, cron scheduler, inbox watcher, and Discord bot
academic-agent serve --with-cron --with-watch --with-discord-bot
```

### Supported Commands and Button Callbacks

| Interaction | Action |
| :--- | :--- |
| `/tasks [status]` | List tracked tasks (optionally filtered by status). |
| `/upcoming [hours]` | Show incomplete tasks due within the next `N` hours (default 72). |
| `/done <task_id>` | Mark a task `COMPLETED`. |
| `/snooze <task_id> [hours]` | Snooze scheduled deadline alerts for `N` hours (default 24). |
| `/add <course> <title> [deadline] [priority]` | Create a task and generate its `.ics` calendar file immediately. |
| `/digest` | Build and reply with the current academic workload summary. |
| **Mark Done** button | Marks the linked task `COMPLETED` directly from any alert message. |
| **Snooze 24h** button | Shifts alert notifications forward by 24 hours directly from Discord. |

## Option 2: Discord Incoming Webhook (Zero Bot Setup)

If you do not want to register a Discord application:

1. Open your channel settings -> **Integrations -> Webhooks -> New Webhook** and copy the URL.
2. Export `DISCORD_WEBHOOK_URL` (or set `discord.webhook_url` in `academic-agent.toml`).
3. Leave `bot_token` and `channel_id` unset (with `mode = "auto"`, `academic-agent` selects the webhook automatically).

In webhook mode, the Google Calendar link is placed cleanly inside an embed field instead of an action button component so plain Discord webhooks accept the payload without requiring an application-owned webhook.

## Rate Limit and Embed Safety

- Field counts (max 25), title lengths (max 256), descriptions (max 4096), field values (max 1024), and Google Calendar link lengths (max 512) are enforced before sending.
- Empty fields are filtered automatically so Discord never rejects payloads with HTTP 400.
- HTTP `429 Too Many Requests` (`Retry-After` header or JSON `retry_after`) and transient 5xx responses are retried cleanly up to `discord.max_retries` times.

## Optional Hermes CLI Bridge

To dispatch summaries through the `hermes` CLI without hardcoding targets:

```toml
[hermes]
enabled = true
binary_path = "hermes"
target = "${HERMES_TARGET}"
timeout_secs = 15
fallback_on_discord_error = true
```

When `fallback_on_discord_error = true`, `hermes send --to <target> <message>` is invoked only if Discord dispatch failed. Note that Hermes Agent can also manage tasks directly in two directions via the MCP server documented in [docs/mcp.md](mcp.md).
