# Docker Deployment

`academic-agent` ships a multi-stage non-root `Dockerfile` and `docker-compose.yml` running the embedded web dashboard, REST API, live iCalendar feed (`GET /api/calendar/feed.ics`), cron reminder scheduler, inbox watcher, and Streamable HTTP `/mcp` server.

## Quick Start with Docker Compose

1. Create the host `./data` directory and place a configuration file at `./data/academic-agent.toml`:

```bash
mkdir -p data/inbox data/archive data/calendar
cp examples/academic-agent.toml data/academic-agent.toml
```

Inside the container, the working directory is `/data` and the entrypoint defaults to `--config /data/academic-agent.toml --db /data/state.redb`. If you want calendar files written directly to `./data/calendar` on the host, set relative container paths under `[general]` (`inbox_dir = "inbox"`, `archive_dir = "archive"`, `calendar_dir = "calendar"`).

2. Configure environment variables for authentication, LLM extraction, and notifications:

```bash
export ACADEMIC_API_TOKEN="change-me-to-a-strong-token"
export DISCORD_BOT_TOKEN="your-discord-bot-token"
export DISCORD_CHANNEL_ID="123456789012345678"
export DISCORD_APPLICATION_ID="123456789012345678"
export ACADEMIC_LLM_API_BASE="https://llm-gateway.example.com/v1"
export ACADEMIC_LLM_API_KEY="your-llm-key"
```

3. Build and start the container:

```bash
docker compose up -d --build
```

The service listens on port `8080`:

- Embedded web dashboard: `http://127.0.0.1:8080/`
- Live iCalendar subscription feed: `http://127.0.0.1:8080/api/calendar/feed.ics`
- Health check endpoint: `http://127.0.0.1:8080/health`
- Streamable HTTP MCP endpoint: `http://127.0.0.1:8080/mcp`

## Enabling the Interactive Discord Gateway Bot in Compose

The default `CMD` enables `--with-cron` and `--with-watch`. To also run the persistent two-way Discord WebSocket Gateway listener (`--with-discord-bot`) inside the same container, add `command:` to `docker-compose.yml`:

```yaml
    command:
      - "--config"
      - "/data/academic-agent.toml"
      - "--db"
      - "/data/state.redb"
      - "serve"
      - "--bind"
      - "0.0.0.0:8080"
      - "--with-cron"
      - "--with-watch"
      - "--with-discord-bot"
```

## Prebuilt GHCR Container Images

Version tags automatically build and publish container images to GitHub Container Registry via `.github/workflows/docker.yml`:

```bash
docker pull ghcr.io/luminariadev/academic-agent:latest
docker run -d \
  -p 8080:8080 \
  -v "$(pwd)/data:/data" \
  -e ACADEMIC_API_TOKEN="secret" \
  ghcr.io/luminariadev/academic-agent:latest
```

## Health Checks and Persistence

- Container execution runs as non-root user `academic`.
- Database state (`state.redb`), inbox uploads, archives, and `.ics` files persist inside the mounted `./data` volume.
- Docker checks `GET http://127.0.0.1:8080/health` every 30 seconds and expects HTTP `200`.
- When `ACADEMIC_API_TOKEN` is set, `/api/*` and `/mcp` endpoints require `Authorization: Bearer <token>` (while `/api/calendar/feed.ics` also supports `?token=<token>` for standard calendar clients).
