# Discord and Hermes Integration

`academic-agent` uses Discord as its primary free interactive notification channel and supports Hermes CLI forwarding as either a standalone transport or a fallback.

## Option 1: Discord Bot Token (REST API v10 with Link Button)

Using a Discord Bot token enables both rich color-coded embeds and an interactive **Add to Google Calendar** URL button under every newly ingested task.

1. Open the [Discord Developer Portal](https://discord.com/developers/applications) and create an application.
2. Under the **Bot** tab, copy or reset your Bot Token and store it as `DISCORD_BOT_TOKEN` in `.env`.
3. Under **OAuth2 -> URL Generator**, select scope `bot` and permissions `Send Messages` and `Embed Links`, then invite the bot to your server.
4. Enable Developer Mode in Discord settings, right-click your target channel (for example `#academic-deadlines`), copy its ID, and set `DISCORD_CHANNEL_ID` in `.env`.
5. Verify delivery:

```bash
academic-agent test-discord
```

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
