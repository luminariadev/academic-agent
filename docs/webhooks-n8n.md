# Webhooks, n8n, and webdelta Integration

`academic-agent` supports both inbound webpage-change webhooks from `webdelta` and outbound task webhooks to n8n.

## Receiving Alerts from `webdelta`

You can configure `webdelta` to watch university portal pages, course announcement boards, or department calendars and push detected updates directly into `academic-agent` via `POST /api/webhooks/webdelta`.

1. Start `academic-agent serve` with a shared secret:

```bash
export WEBDELTA_WEBHOOK_SECRET="shared-hmac-secret"
academic-agent serve --bind 127.0.0.1:8080 --with-cron
```

2. Point a `webdelta` webhook notifier to `academic-agent`:

```toml
[notifiers.academic]
type = "webhook"
url = "http://127.0.0.1:8080/api/webhooks/webdelta"
secret = "${WEBDELTA_WEBHOOK_SECRET}"
```

`academic-agent` verifies the `X-WebDelta-Signature: sha256=<hex>` header in constant time, hashes the update payload to avoid duplicate tasks, extracts deadlines and subtasks, builds the `.ics` event, and notifies Discord.

## n8n Workflow Template

An updated workflow is provided at [workflows/academic-agent.n8n.json](../workflows/academic-agent.n8n.json). See [workflows/README.md](../workflows/README.md) for setup instructions and test payloads.

To forward new tasks extracted by `academic-agent` outward to n8n (with optional HMAC verification header `X-Academic-Signature`):

```toml
[n8n]
enabled = true
webhook_url = "http://127.0.0.1:5678/webhook/academic-intake"
secret = "${N8N_WEBHOOK_SECRET}"
timeout_secs = 10
```
