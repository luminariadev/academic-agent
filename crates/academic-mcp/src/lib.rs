use std::path::{Path, PathBuf};
use std::sync::Arc;

use academic_core::calendar::{create_ics_event, parse_deadline_in_tz, CalendarEventSpec};
use academic_core::config::{redact_config_toml, Config};
use academic_core::error::{Error, Result};
use academic_core::extract::{Priority, TaskKind};
use academic_core::notify::dispatch_new_task;
use academic_core::parser::{safe_join_within, sha256_hex};
use academic_core::pipeline::AcademicPipeline;
use academic_core::store::{InsertOutcome, NewTask, TaskRecord, TaskStatus, TaskStore};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{Duration, Utc};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::RwLock;

const PROTOCOL_VERSION: &str = "2024-11-05";

/// Built-in Model Context Protocol server backed by `TaskStore` and `Config`.
#[derive(Debug, Clone)]
pub struct McpServer {
    config_path: PathBuf,
    config: Arc<RwLock<Config>>,
    store: TaskStore,
}

impl McpServer {
    /// Opens the underlying `TaskStore` and builds a new `McpServer`.
    pub fn open(config_path: &Path, config: Config) -> Result<Self> {
        let store = TaskStore::open(&config.general.db_path)?;
        Ok(Self::with_store(config_path.to_path_buf(), config, store))
    }

    /// Reuses an already-opened `TaskStore` so `redb` file locks never conflict across endpoints.
    pub fn with_store(config_path: PathBuf, config: Config, store: TaskStore) -> Self {
        Self {
            config_path,
            config: Arc::new(RwLock::new(config)),
            store,
        }
    }

    /// Returns a handle to the underlying `TaskStore`.
    pub fn store(&self) -> &TaskStore {
        &self.store
    }

    /// Processes a single JSON-RPC 2.0 request line and returns an optional response payload.
    pub async fn handle_message(&self, line: &str) -> Option<Value> {
        let parsed: Value = match serde_json::from_str(line.trim()) {
            Ok(v) => v,
            Err(e) => {
                return Some(json_rpc_err(
                    Value::Null,
                    -32700,
                    &format!("Parse error: {e}"),
                ));
            }
        };

        let id = parsed.get("id").cloned();
        let method = parsed.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = parsed.get("params").cloned().unwrap_or_else(|| json!({}));

        // Notifications without an `id` do not produce a JSON-RPC response
        if id.is_none() && method.starts_with("notifications/") {
            return None;
        }
        let req_id = id.unwrap_or(Value::Null);

        let result = match method {
            "initialize" => Ok(json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {
                    "tools": {},
                    "resources": {}
                },
                "serverInfo": {
                    "name": "academic-agent",
                    "version": env!("CARGO_PKG_VERSION")
                }
            })),
            "notifications/initialized" => return None,
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tool_definitions() })),
            "tools/call" => self.handle_tool_call(&params).await,
            "resources/list" => Ok(json!({ "resources": static_resource_definitions() })),
            "resources/templates/list" => {
                Ok(json!({ "resourceTemplates": resource_template_definitions() }))
            }
            "resources/read" => self.handle_resource_read(&params).await,
            other => Err((-32601, format!("Method not found: {other}"))),
        };

        Some(match result {
            Ok(payload) => json!({
                "jsonrpc": "2.0",
                "id": req_id,
                "result": payload
            }),
            Err((code, msg)) => json_rpc_err(req_id, code, &msg),
        })
    }

    /// Runs the newline-delimited JSON-RPC 2.0 loop over `stdin` and `stdout`.
    pub async fn run_stdio(&self) -> Result<()> {
        let stdin = tokio::io::stdin();
        let mut stdout = tokio::io::stdout();
        let mut lines = BufReader::new(stdin).lines();

        while let Some(line) = lines.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }
            if let Some(resp) = self.handle_message(&line).await {
                let serialized = serde_json::to_string(&resp)?;
                stdout.write_all(serialized.as_bytes()).await?;
                stdout.write_all(b"\n").await?;
                stdout.flush().await?;
            }
        }
        Ok(())
    }

    async fn handle_tool_call(&self, params: &Value) -> std::result::Result<Value, (i64, String)> {
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| (-32602, "missing tool 'name'".to_string()))?;
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));

        let outcome: Result<Value> = match name {
            "list_tasks" => self.tool_list_tasks(&args),
            "add_task" => self.tool_add_task(&args).await,
            "update_status" => self.tool_update_status(&args),
            "snooze_task" => self.tool_snooze_task(&args),
            "scan_document" => self.tool_scan_document(&args).await,
            "upcoming_deadlines" => self.tool_upcoming_deadlines(&args),
            "export_ics" => self.tool_export_ics(&args).await,
            "send_digest" => self.tool_send_digest(&args).await,
            unknown => Err(Error::NotFound(format!("unknown tool '{unknown}'"))),
        };

        match outcome {
            Ok(data) => {
                let text = serde_json::to_string_pretty(&data)
                    .map_err(|e| (-32603, format!("serialize failed: {e}")))?;
                Ok(json!({
                    "content": [{ "type": "text", "text": text }],
                    "isError": false
                }))
            }
            Err(err) => {
                let text = serde_json::to_string(&json!({ "error": err.to_string() }))
                    .unwrap_or_else(|_| err.to_string());
                Ok(json!({
                    "content": [{ "type": "text", "text": text }],
                    "isError": true
                }))
            }
        }
    }

    fn tool_list_tasks(&self, args: &Value) -> Result<Value> {
        let status_filter = match args.get("status").and_then(|v| v.as_str()) {
            Some(s) if !s.trim().is_empty() => Some(TaskStatus::parse_loose(s)?),
            _ => None,
        };
        let tasks = self.store.list_tasks(status_filter)?;
        Ok(json!({
            "count": tasks.len(),
            "tasks": tasks
        }))
    }

    async fn tool_add_task(&self, args: &Value) -> Result<Value> {
        let title = args
            .get("title")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| Error::Config("'title' is required".to_string()))?;

        let cfg = self.config.read().await.clone();
        let tz = cfg.timezone_tz()?;

        let course = args
            .get("course")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "General Course".to_string());
        let kind = args
            .get("kind")
            .and_then(|v| v.as_str())
            .map(TaskKind::parse_loose)
            .unwrap_or(TaskKind::Assignment);
        let priority = args
            .get("priority")
            .and_then(|v| v.as_str())
            .map(Priority::parse_loose)
            .unwrap_or(Priority::Medium);
        let deadline_raw = args
            .get("deadline")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let deadline_utc = deadline_raw
            .as_deref()
            .and_then(|d| parse_deadline_in_tz(d, tz))
            .map(|dt| dt.with_timezone(&Utc));
        let estimated_hours = args
            .get("estimated_hours")
            .and_then(|v| v.as_u64())
            .unwrap_or(2)
            .max(1) as u32;
        let summary = args
            .get("summary")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("{course}: {title}"));
        let subtasks = json_string_list(args.get("subtasks"));
        let deliverables = json_string_list(args.get("deliverables"));
        let notify = args
            .get("notify")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let dedup_seed = format!(
            "manual:{course}:{title}:{}",
            deadline_raw.as_deref().unwrap_or("")
        );
        let inserted = self.store.insert_or_dedup(NewTask {
            title: title.clone(),
            course: course.clone(),
            kind,
            deadline_raw: deadline_raw.clone(),
            deadline_utc,
            priority,
            estimated_hours,
            summary: summary.clone(),
            subtasks,
            deliverables,
            ics_file: None,
            google_calendar_url: None,
            source_file: Some("mcp:add_task".to_string()),
            content_hash: Some(sha256_hex(&dedup_seed)),
            extractor: "manual".to_string(),
        })?;

        let (duplicate, mut task) = match inserted {
            InsertOutcome::Duplicate(t) => (true, t),
            InsertOutcome::Inserted(t) => (false, t),
        };

        let mut calendar = None;
        if !duplicate && (task.deadline_raw.is_some() || task.deadline_utc.is_some()) {
            let cal = create_ics_event(CalendarEventSpec {
                calendar_dir: &cfg.general.calendar_dir,
                title: &task.title,
                deadline_str: task.deadline_raw.as_deref(),
                tz,
                course: &task.course,
                description: &task.summary,
                duration_hours: cfg.general.default_duration_hours,
                priority: task.priority,
            })?;
            task = self.store.update_calendar_info(
                task.id,
                Some(cal.filename.clone()),
                Some(cal.google_calendar_url.clone()),
            )?;
            calendar = Some(cal);
        }

        let notifications = if !duplicate && notify {
            dispatch_new_task(
                &cfg.discord,
                &cfg.hermes,
                &cfg.n8n,
                &task,
                calendar.as_ref(),
            )
            .await
        } else {
            Vec::new()
        };

        Ok(json!({
            "duplicate": duplicate,
            "task": task,
            "calendar": calendar,
            "notifications": notifications
        }))
    }

    fn tool_update_status(&self, args: &Value) -> Result<Value> {
        let task_id = req_u64(args, "task_id")?;
        let status_raw = args
            .get("status")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::Config("'status' is required".to_string()))?;
        let status = TaskStatus::parse_loose(status_raw)?;
        let updated = self.store.update_status(task_id, status)?;
        Ok(json!({ "task": updated }))
    }

    fn tool_snooze_task(&self, args: &Value) -> Result<Value> {
        let task_id = req_u64(args, "task_id")?;
        let hours = args
            .get("hours")
            .and_then(|v| v.as_i64())
            .unwrap_or(24)
            .max(1);
        let until = Utc::now() + Duration::hours(hours);
        let updated = self.store.snooze_task(task_id, until)?;
        Ok(json!({ "task": updated }))
    }

    async fn tool_scan_document(&self, args: &Value) -> Result<Value> {
        let cfg = self.config.read().await.clone();
        let pipeline = AcademicPipeline::new(&cfg, &self.store);
        let force_heuristic = args
            .get("heuristic")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let notify = args
            .get("notify")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if let Some(path_str) = args.get("path").and_then(|v| v.as_str()) {
            let p = Path::new(path_str);
            let res = pipeline.ingest_file(p, force_heuristic, notify).await?;
            return Ok(serde_json::to_value(res)?);
        }

        if let Some(text) = args.get("text").and_then(|v| v.as_str()) {
            let filename = args
                .get("filename")
                .and_then(|v| v.as_str())
                .unwrap_or("mcp_input.txt");
            let res = pipeline
                .ingest_text(
                    text,
                    filename,
                    Some(filename),
                    None,
                    force_heuristic,
                    notify,
                )
                .await?;
            return Ok(serde_json::to_value(res)?);
        }

        Err(Error::Config(
            "either 'path' or 'text' must be provided to scan_document".to_string(),
        ))
    }

    fn tool_upcoming_deadlines(&self, args: &Value) -> Result<Value> {
        let within_hours = args
            .get("within_hours")
            .and_then(|v| v.as_i64())
            .unwrap_or(72)
            .max(1);
        let tasks = self.store.upcoming_tasks(within_hours, Utc::now())?;
        Ok(json!({
            "within_hours": within_hours,
            "count": tasks.len(),
            "tasks": tasks
        }))
    }

    async fn tool_export_ics(&self, args: &Value) -> Result<Value> {
        let task_id = req_u64(args, "task_id")?;
        let task = self
            .store
            .get_task(task_id)?
            .ok_or_else(|| Error::NotFound(format!("task #{task_id} not found")))?;
        let cfg = self.config.read().await.clone();
        let artifact = ensure_calendar_artifact(&cfg, &self.store, &task)?;
        Ok(serde_json::to_value(artifact)?)
    }

    async fn tool_send_digest(&self, args: &Value) -> Result<Value> {
        let dry_run = args
            .get("dry_run")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let cfg = self.config.read().await.clone();
        let pipeline = AcademicPipeline::new(&cfg, &self.store);
        let res = pipeline.send_digest(Utc::now(), dry_run).await?;
        Ok(serde_json::to_value(res)?)
    }

    async fn handle_resource_read(
        &self,
        params: &Value,
    ) -> std::result::Result<Value, (i64, String)> {
        let uri = params
            .get("uri")
            .and_then(|v| v.as_str())
            .ok_or_else(|| (-32602, "missing resource 'uri'".to_string()))?;

        let (mime, text) = match uri {
            "academic://tasks" => {
                let tasks = self
                    .store
                    .list_tasks(None)
                    .map_err(|e| (-32603, e.to_string()))?;
                let json =
                    serde_json::to_string_pretty(&tasks).map_err(|e| (-32603, e.to_string()))?;
                ("application/json", json)
            }
            "academic://upcoming" => {
                let tasks = self
                    .store
                    .upcoming_tasks(72, Utc::now())
                    .map_err(|e| (-32603, e.to_string()))?;
                let json =
                    serde_json::to_string_pretty(&tasks).map_err(|e| (-32603, e.to_string()))?;
                ("application/json", json)
            }
            "academic://config" => {
                let cfg = self.config.read().await.clone();
                let toml = redact_config_toml(&cfg).map_err(|e| (-32603, e.to_string()))?;
                (
                    "text/plain",
                    format!("# {}\n{}", self.config_path.display(), toml),
                )
            }
            other if other.starts_with("academic://calendar/") => {
                let id_str = other.trim_start_matches("academic://calendar/").trim();
                let task_id: u64 = id_str.parse().map_err(|_| {
                    (
                        -32602,
                        format!("invalid task ID '{id_str}' in calendar URI"),
                    )
                })?;
                let task = self
                    .store
                    .get_task(task_id)
                    .map_err(|e| (-32603, e.to_string()))?
                    .ok_or_else(|| (-32002, format!("task #{task_id} not found")))?;
                let cfg = self.config.read().await.clone();
                let ics = load_or_generate_ics(&cfg, &self.store, &task)
                    .map_err(|e| (-32603, e.to_string()))?;
                ("text/calendar", ics)
            }
            other => return Err((-32002, format!("Resource not found: {other}"))),
        };

        Ok(json!({
            "contents": [{
                "uri": uri,
                "mimeType": mime,
                "text": text
            }]
        }))
    }
}

fn ensure_calendar_artifact(
    cfg: &Config,
    store: &TaskStore,
    task: &TaskRecord,
) -> Result<academic_core::CalendarArtifact> {
    let tz = cfg.timezone_tz()?;
    let artifact = create_ics_event(CalendarEventSpec {
        calendar_dir: &cfg.general.calendar_dir,
        title: &task.title,
        deadline_str: task.deadline_raw.as_deref(),
        tz,
        course: &task.course,
        description: &task.summary,
        duration_hours: cfg.general.default_duration_hours,
        priority: task.priority,
    })?;
    let _ = store.update_calendar_info(
        task.id,
        Some(artifact.filename.clone()),
        Some(artifact.google_calendar_url.clone()),
    )?;
    Ok(artifact)
}

fn load_or_generate_ics(cfg: &Config, store: &TaskStore, task: &TaskRecord) -> Result<String> {
    if let Some(ref existing) = task.ics_file {
        if let Ok(path) = safe_join_within(&cfg.general.calendar_dir, existing) {
            if let Ok(content) = std::fs::read_to_string(path) {
                return Ok(content);
            }
        }
    }
    Ok(ensure_calendar_artifact(cfg, store, task)?.ics_content)
}

fn req_u64(args: &Value, key: &str) -> Result<u64> {
    args.get(key)
        .and_then(|v| v.as_u64())
        .ok_or_else(|| Error::Config(format!("'{key}' integer argument is required")))
}

fn json_string_list(val: Option<&Value>) -> Vec<String> {
    val.and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn json_rpc_err(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message
        }
    })
}

fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "list_tasks",
            "description": "List tracked academic tasks sorted by soonest deadline first.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "status": {
                        "type": "string",
                        "enum": ["PENDING", "IN_PROGRESS", "COMPLETED", "ARCHIVED"],
                        "description": "Optional status filter."
                    }
                }
            }
        }),
        json!({
            "name": "add_task",
            "description": "Manually register an academic task and generate a timezone-aware .ics calendar artifact.",
            "inputSchema": {
                "type": "object",
                "required": ["title"],
                "properties": {
                    "title": { "type": "string" },
                    "course": { "type": "string" },
                    "kind": { "type": "string", "enum": ["assignment", "quiz", "exam", "syllabus", "announcement", "general"] },
                    "deadline": { "type": "string", "description": "Deadline string (e.g. '2026-12-01 23:59' or '15 Juli 2026')." },
                    "priority": { "type": "string", "enum": ["HIGH", "MEDIUM", "LOW"] },
                    "estimated_hours": { "type": "integer", "minimum": 1 },
                    "summary": { "type": "string" },
                    "subtasks": { "type": "array", "items": { "type": "string" } },
                    "deliverables": { "type": "array", "items": { "type": "string" } },
                    "notify": { "type": "boolean", "description": "Dispatch Discord/Hermes alert if configured (defaults to false)." }
                }
            }
        }),
        json!({
            "name": "update_status",
            "description": "Update a task's status (PENDING, IN_PROGRESS, COMPLETED, ARCHIVED).",
            "inputSchema": {
                "type": "object",
                "required": ["task_id", "status"],
                "properties": {
                    "task_id": { "type": "integer", "minimum": 1 },
                    "status": { "type": "string", "enum": ["PENDING", "IN_PROGRESS", "COMPLETED", "ARCHIVED"] }
                }
            }
        }),
        json!({
            "name": "snooze_task",
            "description": "Postpone scheduled cron deadline reminders for a task by N hours (default 24).",
            "inputSchema": {
                "type": "object",
                "required": ["task_id"],
                "properties": {
                    "task_id": { "type": "integer", "minimum": 1 },
                    "hours": { "type": "integer", "minimum": 1 }
                }
            }
        }),
        json!({
            "name": "scan_document",
            "description": "Parse and ingest a local document (.pdf, .docx, .txt, .md) or raw text into the task store.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Local file path to ingest." },
                    "text": { "type": "string", "description": "Raw assignment or announcement text." },
                    "filename": { "type": "string", "description": "Optional virtual filename when supplying raw text." },
                    "heuristic": { "type": "boolean", "description": "Force offline heuristic extraction instead of LLM." },
                    "notify": { "type": "boolean", "description": "Dispatch notifications for newly inserted tasks (defaults to false)." }
                }
            }
        }),
        json!({
            "name": "upcoming_deadlines",
            "description": "List incomplete tasks due within the next N hours (default 72, including overdue).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "within_hours": { "type": "integer", "minimum": 1 }
                }
            }
        }),
        json!({
            "name": "export_ics",
            "description": "Generate and return the RFC 5545 .ics calendar payload and Google Calendar URL for a task.",
            "inputSchema": {
                "type": "object",
                "required": ["task_id"],
                "properties": {
                    "task_id": { "type": "integer", "minimum": 1 }
                }
            }
        }),
        json!({
            "name": "send_digest",
            "description": "Preview or dispatch a digest summary of currently open academic tasks.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "dry_run": { "type": "boolean", "description": "When true (default), counts open tasks without sending Discord/Hermes messages." }
                }
            }
        }),
    ]
}

fn static_resource_definitions() -> Vec<Value> {
    vec![
        json!({
            "uri": "academic://tasks",
            "name": "All Academic Tasks",
            "description": "Full task list stored in redb ordered by deadline.",
            "mimeType": "application/json"
        }),
        json!({
            "uri": "academic://upcoming",
            "name": "Upcoming Deadlines (72h)",
            "description": "Incomplete tasks due within 72 hours or already overdue.",
            "mimeType": "application/json"
        }),
        json!({
            "uri": "academic://config",
            "name": "Redacted Configuration",
            "description": "Active academic-agent.toml with literal secrets redacted.",
            "mimeType": "text/plain"
        }),
    ]
}

fn resource_template_definitions() -> Vec<Value> {
    vec![json!({
        "uriTemplate": "academic://calendar/{task_id}",
        "name": "Task iCalendar Event",
        "description": "RFC 5545 .ics calendar event for the requested task ID.",
        "mimeType": "text/calendar"
    })]
}

/// Shared state for Streamable HTTP MCP transport.
#[derive(Debug, Clone)]
pub struct McpHttpState {
    pub server: Arc<McpServer>,
    pub token: Option<String>,
}

/// Checks `Authorization: Bearer <token>` using constant-time byte equality when expected token is set.
pub fn is_bearer_authorized(headers: &HeaderMap, expected: Option<&str>) -> bool {
    let Some(secret) = expected.map(str::trim).filter(|s| !s.is_empty()) else {
        return true;
    };
    let Some(raw) = headers.get("authorization").and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let Some(provided) = raw.trim().strip_prefix("Bearer ").map(str::trim) else {
        return false;
    };
    if provided.len() != secret.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in provided.as_bytes().iter().zip(secret.as_bytes().iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

async fn http_mcp_handler(
    State(state): State<Arc<McpHttpState>>,
    headers: HeaderMap,
    body: String,
) -> Response {
    if !is_bearer_authorized(&headers, state.token.as_deref()) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Unauthorized: invalid or missing bearer token" })),
        )
            .into_response();
    }

    match state.server.handle_message(&body).await {
        Some(resp) => (StatusCode::OK, Json(resp)).into_response(),
        None => StatusCode::ACCEPTED.into_response(),
    }
}

/// Returns only the `POST /mcp` route so callers can merge it into an existing Axum router.
pub fn mcp_route(state: Arc<McpHttpState>) -> Router {
    Router::new()
        .route("/mcp", post(http_mcp_handler))
        .with_state(state)
}

/// Returns a standalone Streamable HTTP MCP router including `POST /mcp` and `GET /health`.
pub fn mcp_http_router(state: Arc<McpHttpState>) -> Router {
    mcp_route(state).route(
        "/health",
        get(|| async { Json(json!({ "status": "ok", "service": "academic-mcp" })) }),
    )
}

/// Serves the MCP endpoint over Streamable HTTP (`POST /mcp`) until interrupted.
pub async fn run_http(
    config_path: PathBuf,
    config: Config,
    bind: &str,
    token: Option<String>,
) -> Result<()> {
    let server = Arc::new(McpServer::open(&config_path, config)?);
    let state = Arc::new(McpHttpState {
        server,
        token: token.filter(|s| !s.trim().is_empty()),
    });
    let app = mcp_http_router(state);
    let listener = TcpListener::bind(bind).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_mcp_tools_and_resources_offline() {
        let dir = tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.resolve_paths_relative_to(dir.path());
        cfg.ensure_runtime_dirs().unwrap();
        cfg.discord.bot_token = "literal-secret-token".to_string();

        let srv = McpServer::open(&dir.path().join("academic-agent.toml"), cfg).unwrap();

        let init = srv
            .handle_message(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#)
            .await
            .unwrap();
        assert_eq!(init["result"]["serverInfo"]["name"], "academic-agent");

        // Notifications without id must not emit a response
        assert!(srv
            .handle_message(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .await
            .is_none());

        let tools = srv
            .handle_message(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)
            .await
            .unwrap();
        assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 8);

        let added = srv
            .handle_message(
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"add_task","arguments":{"title":"Compiler Design Homework","course":"Compilers","deadline":"2026-12-01 23:59","priority":"HIGH"}}}"#,
            )
            .await
            .unwrap();
        let added_body: Value =
            serde_json::from_str(added["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(added_body["duplicate"], false);
        assert_eq!(added_body["task"]["id"], 1);

        let scanned = srv
            .handle_message(
                r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"scan_document","arguments":{"text":"Course: Databases\nTitle: B-Tree Index Lab\nDeadline: 2026-12-05 23:59","heuristic":true}}}"#,
            )
            .await
            .unwrap();
        let scanned_body: Value =
            serde_json::from_str(scanned["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(scanned_body["task"]["course"], "Databases");

        let cfg_res = srv
            .handle_message(
                r#"{"jsonrpc":"2.0","id":5,"method":"resources/read","params":{"uri":"academic://config"}}"#,
            )
            .await
            .unwrap();
        let cfg_text = cfg_res["result"]["contents"][0]["text"].as_str().unwrap();
        assert!(!cfg_text.contains("literal-secret-token"));
        assert!(cfg_text.contains("<redacted>"));

        let ics_res = srv
            .handle_message(
                r#"{"jsonrpc":"2.0","id":6,"method":"resources/read","params":{"uri":"academic://calendar/1"}}"#,
            )
            .await
            .unwrap();
        let ics_text = ics_res["result"]["contents"][0]["text"].as_str().unwrap();
        assert!(ics_text.contains("BEGIN:VCALENDAR"));
        assert!(ics_text.contains("Compiler Design Homework"));
    }
}
