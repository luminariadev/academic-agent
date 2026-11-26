# n8n Workflows

This directory provides a ready-to-import workflow template at [academic-agent.n8n.json](academic-agent.n8n.json) for orchestrating academic task extraction and Discord notifications in n8n.

## Workflow Stages

1. **Webhook Trigger (`POST /webhook/academic-task`)**: Accepts raw document text (`content` or `text`) and `filename`.
2. **Preprocess and Serialize**: Builds a properly serialized JSON request using `JSON.stringify` so quotes and multiline PDF snippets cannot break the LLM payload.
3. **LLM Extraction Request**: Calls the configured Chat Completions gateway (`ACADEMIC_LLM_API_BASE` or `ROUTER_API_BASE`).
4. **Build Calendar and Discord Payload**: Parses the structured JSON, converts local Asia/Jakarta deadlines to UTC for the 1-click Google Calendar link (bounded to 512 chars), and constructs a safe Discord embed payload.
5. **Send Discord Embed**: Dispatches the embed and Google Calendar action button via Discord REST API v10.
6. **Respond to Webhook**: Returns the parsed task JSON and calendar link to the caller.

## Importing into n8n

1. Open your n8n dashboard (`http://localhost:5678` by default).
2. Create a workflow and select **Import from File**.
3. Choose `workflows/academic-agent.n8n.json`.
4. Set environment variables on your n8n host: `ACADEMIC_LLM_API_BASE`, `ACADEMIC_LLM_API_KEY`, `ROUTER_MODEL`, `DISCORD_BOT_TOKEN`, and `DISCORD_CHANNEL_ID`.

To avoid duplicate Discord notifications when running both `academic-agent watch` and n8n together, configure n8n either as a standalone webhook intake or disable `[discord].enabled` in `academic-agent.toml` when routing all notifications through `[n8n]`.

## Testing the Webhook

```bash
curl -X POST http://localhost:5678/webhook/academic-task \
  -H "Content-Type: application/json" \
  -d '{"filename":"research_proposal.txt","text":"Course: Research Methods. Title: Chapter 1 Proposal Draft. Deadline: 2026-12-10 23:59."}'
```
