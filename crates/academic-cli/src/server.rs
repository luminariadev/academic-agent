use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use academic_core::calendar::{
    create_ics_event, filter_tasks_for_feed, generate_ics_feed, parse_deadline_in_tz,
    CalendarEventSpec,
};
use academic_core::config::Config;
use academic_core::cron_engine::run_cron_tick;
use academic_core::discord_bot::run_discord_gateway;
use academic_core::error::Error;
use academic_core::extract::{extract_from_text, Priority, TaskKind};
use academic_core::notify::dispatch_new_task;
use academic_core::parser::{safe_join_within, sanitize_filename, sha256_hex};
use academic_core::pipeline::{verify_hmac_sha256, AcademicPipeline, WebdeltaWebhookPayload};
use academic_core::store::{ExportFormat, InsertOutcome, NewTask, TaskStatus, TaskStore};
use academic_mcp::{
    constant_time_secret_eq, is_bearer_authorized, mcp_route, McpHttpState, McpServer,
};
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Multipart, Path as AxumPath, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tracing::{error, info};

use crate::watcher::run_watcher_loop;

#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub store: TaskStore,
    pub api_token: Option<String>,
    pub webdelta_secret: Option<String>,
}

enum ApiError {
    Unauthorized(String),
    BadRequest(String),
    NotFound(String),
    Internal(String),
}

impl From<Error> for ApiError {
    fn from(err: Error) -> Self {
        match err {
            Error::InvalidPath(m)
            | Error::UnsupportedFormat(m)
            | Error::Parse(m)
            | Error::Config(m) => Self::BadRequest(m),
            Error::NotFound(m) => Self::NotFound(m),
            Error::Unauthorized(m) => Self::Unauthorized(if m.is_empty() {
                "Unauthorized".to_string()
            } else {
                m
            }),
            other => Self::Internal(other.to_string()),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            Self::Unauthorized(m) => (StatusCode::UNAUTHORIZED, m),
            Self::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            Self::NotFound(m) => (StatusCode::NOT_FOUND, m),
            Self::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
        };
        (status, Json(json!({ "error": msg }))).into_response()
    }
}

fn require_bearer(state: &AppState, headers: &HeaderMap) -> std::result::Result<(), ApiError> {
    if is_bearer_authorized(headers, state.api_token.as_deref()) {
        Ok(())
    } else {
        Err(ApiError::Unauthorized(
            "Unauthorized: invalid or missing bearer token".to_string(),
        ))
    }
}

fn require_bearer_or_query_token(
    state: &AppState,
    headers: &HeaderMap,
    query_token: Option<&str>,
) -> std::result::Result<(), ApiError> {
    if is_bearer_authorized(headers, state.api_token.as_deref()) {
        return Ok(());
    }
    if let (Some(expected), Some(provided)) = (state.api_token.as_deref(), query_token) {
        if constant_time_secret_eq(provided.trim(), expected.trim()) {
            return Ok(());
        }
    }
    Err(ApiError::Unauthorized(
        "Unauthorized: invalid or missing token".to_string(),
    ))
}

fn build_cors_headers(state: &AppState, req_headers: &HeaderMap) -> HeaderMap {
    let mut out = HeaderMap::new();
    if let Some(origin) = req_headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
    {
        if state
            .config
            .server
            .allowed_origins
            .iter()
            .any(|allowed| allowed == origin)
        {
            if let Ok(val) = HeaderValue::from_str(origin) {
                out.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, val);
                out.insert(
                    header::ACCESS_CONTROL_ALLOW_METHODS,
                    HeaderValue::from_static("GET, POST, PATCH, DELETE, OPTIONS"),
                );
                out.insert(
                    header::ACCESS_CONTROL_ALLOW_HEADERS,
                    HeaderValue::from_static("Authorization, Content-Type, X-WebDelta-Signature"),
                );
            }
        }
    }
    out
}

async fn health_handler(State(state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": "academic-agent",
        "version": env!("CARGO_PKG_VERSION"),
        "timezone": state.config.general.timezone
    }))
}

#[derive(Deserialize)]
struct ListQuery {
    status: Option<String>,
}

async fn list_tasks_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> std::result::Result<impl IntoResponse, ApiError> {
    require_bearer(&state, &headers)?;
    let status = match query.status.as_deref() {
        Some(s) if !s.trim().is_empty() => Some(TaskStatus::parse_loose(s)?),
        _ => None,
    };
    let tasks = state.store.list_tasks(status)?;
    let cors = build_cors_headers(&state, &headers);
    Ok((cors, Json(json!({ "count": tasks.len(), "tasks": tasks }))))
}

#[derive(Deserialize)]
struct CreateTaskBody {
    title: String,
    #[serde(default)]
    course: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    deadline: Option<String>,
    #[serde(default)]
    priority: Option<String>,
    #[serde(default)]
    estimated_hours: Option<u32>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    subtasks: Vec<String>,
    #[serde(default)]
    deliverables: Vec<String>,
    #[serde(default)]
    notify: bool,
}

async fn create_task_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<CreateTaskBody>,
) -> std::result::Result<impl IntoResponse, ApiError> {
    require_bearer(&state, &headers)?;
    let title = body.title.trim().to_string();
    if title.is_empty() {
        return Err(ApiError::BadRequest(
            "'title' must not be empty".to_string(),
        ));
    }
    let tz = state.config.timezone_tz()?;
    let course = body
        .course
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "General Course".to_string());
    let kind = body
        .kind
        .as_deref()
        .map(TaskKind::parse_loose)
        .unwrap_or(TaskKind::Assignment);
    let priority = body
        .priority
        .as_deref()
        .map(Priority::parse_loose)
        .unwrap_or(Priority::Medium);
    let deadline_raw = body
        .deadline
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let deadline_utc = deadline_raw
        .as_deref()
        .and_then(|d| parse_deadline_in_tz(d, tz))
        .map(|dt| dt.with_timezone(&Utc));
    let summary = body
        .summary
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{course}: {title}"));

    let dedup_seed = format!(
        "api:{course}:{title}:{}",
        deadline_raw.as_deref().unwrap_or("")
    );
    let outcome = state.store.insert_or_dedup(NewTask {
        title,
        course,
        kind,
        deadline_raw,
        deadline_utc,
        priority,
        estimated_hours: body.estimated_hours.unwrap_or(2).max(1),
        summary,
        subtasks: body.subtasks,
        deliverables: body.deliverables,
        ics_file: None,
        google_calendar_url: None,
        source_file: Some("http:api".to_string()),
        content_hash: Some(sha256_hex(&dedup_seed)),
        extractor: "manual".to_string(),
    })?;

    let (duplicate, mut task) = match outcome {
        InsertOutcome::Duplicate(t) => (true, t),
        InsertOutcome::Inserted(t) => (false, t),
    };

    let mut calendar = None;
    if !duplicate && (task.deadline_raw.is_some() || task.deadline_utc.is_some()) {
        let cal = create_ics_event(CalendarEventSpec {
            calendar_dir: &state.config.general.calendar_dir,
            title: &task.title,
            deadline_str: task.deadline_raw.as_deref(),
            tz,
            course: &task.course,
            description: &task.summary,
            duration_hours: state.config.general.default_duration_hours,
            priority: task.priority,
        })?;
        task = state.store.update_calendar_info(
            task.id,
            Some(cal.filename.clone()),
            Some(cal.google_calendar_url.clone()),
        )?;
        calendar = Some(cal);
    }

    let notifications = if !duplicate && body.notify {
        dispatch_new_task(
            &state.config.discord,
            &state.config.hermes,
            &state.config.n8n,
            &task,
            calendar.as_ref(),
        )
        .await
    } else {
        Vec::new()
    };

    Ok((
        if duplicate {
            StatusCode::OK
        } else {
            StatusCode::CREATED
        },
        Json(json!({
            "duplicate": duplicate,
            "task": task,
            "calendar": calendar,
            "notifications": notifications
        })),
    ))
}

#[derive(Deserialize)]
struct ExportQuery {
    format: Option<String>,
    status: Option<String>,
}

async fn export_tasks_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ExportQuery>,
) -> std::result::Result<Response, ApiError> {
    require_bearer(&state, &headers)?;
    let fmt = ExportFormat::parse_loose(query.format.as_deref().unwrap_or("json"))?;
    let status = match query.status.as_deref() {
        Some(s) if !s.trim().is_empty() => Some(TaskStatus::parse_loose(s)?),
        _ => None,
    };
    let body = state.store.export_tasks(fmt, status)?;
    let mime = match fmt {
        ExportFormat::Json => "application/json",
        ExportFormat::Csv => "text/csv; charset=utf-8",
    };
    Ok(([(header::CONTENT_TYPE, mime)], body).into_response())
}

#[derive(Deserialize)]
struct UpdateStatusBody {
    status: String,
}

async fn update_status_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<u64>,
    Json(body): Json<UpdateStatusBody>,
) -> std::result::Result<Json<Value>, ApiError> {
    require_bearer(&state, &headers)?;
    let status = TaskStatus::parse_loose(&body.status)?;
    let updated = state.store.update_status(id, status)?;
    Ok(Json(json!({ "task": updated })))
}

#[derive(Deserialize)]
struct SnoozeBody {
    hours: Option<i64>,
}

async fn snooze_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<u64>,
    Json(body): Json<SnoozeBody>,
) -> std::result::Result<Json<Value>, ApiError> {
    require_bearer(&state, &headers)?;
    let hours = body.hours.unwrap_or(24).max(1);
    let until = Utc::now() + chrono::Duration::hours(hours);
    let updated = state.store.snooze_task(id, until)?;
    Ok(Json(json!({ "task": updated })))
}

async fn delete_task_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<u64>,
) -> std::result::Result<Json<Value>, ApiError> {
    require_bearer(&state, &headers)?;
    let deleted = state.store.delete_task(id)?;
    if !deleted {
        return Err(ApiError::NotFound(format!("task #{id} not found")));
    }
    Ok(Json(json!({ "deleted": true, "id": id })))
}

#[derive(Deserialize)]
struct UpcomingQuery {
    within_hours: Option<i64>,
}

async fn upcoming_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<UpcomingQuery>,
) -> std::result::Result<Json<Value>, ApiError> {
    require_bearer(&state, &headers)?;
    let within_hours = query.within_hours.unwrap_or(72).max(1);
    let tasks = state.store.upcoming_tasks(within_hours, Utc::now())?;
    Ok(Json(json!({
        "within_hours": within_hours,
        "count": tasks.len(),
        "tasks": tasks
    })))
}

#[derive(Deserialize)]
struct ExtractBody {
    text: String,
    #[serde(default)]
    filename: Option<String>,
    #[serde(default)]
    heuristic: bool,
}

async fn extract_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<ExtractBody>,
) -> std::result::Result<Json<Value>, ApiError> {
    require_bearer(&state, &headers)?;
    let tz = state.config.timezone_tz()?;
    let res = extract_from_text(
        &state.config.llm,
        &body.text,
        body.filename.as_deref().unwrap_or("input.txt"),
        tz,
        body.heuristic,
    )
    .await?;
    Ok(Json(json!({ "extraction": res })))
}

#[derive(Deserialize)]
struct UploadQuery {
    heuristic: Option<bool>,
    notify: Option<bool>,
}

async fn upload_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<UploadQuery>,
    mut multipart: Multipart,
) -> std::result::Result<Json<Value>, ApiError> {
    require_bearer(&state, &headers)?;

    let mut saved_path: Option<PathBuf> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(format!("invalid multipart payload: {e}")))?
    {
        if field.name() == Some("file") {
            let raw_name = field
                .file_name()
                .ok_or_else(|| ApiError::BadRequest("missing uploaded filename".to_string()))?
                .to_string();
            let clean_name = sanitize_filename(&raw_name)?;
            let target_path = safe_join_within(&state.config.general.inbox_dir, &clean_name)?;
            let data = field
                .bytes()
                .await
                .map_err(|e| ApiError::BadRequest(format!("failed reading upload stream: {e}")))?;
            if data.len() > state.config.server.max_upload_bytes {
                return Err(ApiError::BadRequest(format!(
                    "upload exceeds max_upload_bytes ({} bytes)",
                    state.config.server.max_upload_bytes
                )));
            }
            std::fs::write(&target_path, &data)
                .map_err(|e| ApiError::Internal(format!("failed writing uploaded file: {e}")))?;
            saved_path = Some(target_path);
            break;
        }
    }

    let path = saved_path
        .ok_or_else(|| ApiError::BadRequest("multipart field 'file' is required".to_string()))?;

    let pipeline = AcademicPipeline::new(&state.config, &state.store);
    let res = pipeline
        .ingest_file(
            &path,
            query.heuristic.unwrap_or(false),
            query.notify.unwrap_or(true),
        )
        .await?;

    Ok(Json(
        serde_json::to_value(res).map_err(|e| ApiError::Internal(e.to_string()))?,
    ))
}

async fn webdelta_webhook_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> std::result::Result<Json<Value>, ApiError> {
    if let Some(ref secret) = state.webdelta_secret {
        let sig = headers
            .get("x-webdelta-signature")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !verify_hmac_sha256(secret, &body, sig) {
            return Err(ApiError::Unauthorized(
                "Unauthorized: invalid or missing X-WebDelta-Signature".to_string(),
            ));
        }
    } else {
        require_bearer(&state, &headers)?;
    }

    let payload: WebdeltaWebhookPayload = serde_json::from_slice(&body)
        .map_err(|e| ApiError::BadRequest(format!("invalid webdelta JSON payload: {e}")))?;
    let pipeline = AcademicPipeline::new(&state.config, &state.store);
    let res = pipeline.ingest_webdelta_event(payload, false, true).await?;
    Ok(Json(
        serde_json::to_value(res).map_err(|e| ApiError::Internal(e.to_string()))?,
    ))
}

#[derive(Deserialize)]
struct DigestBody {
    #[serde(default = "default_true")]
    dry_run: bool,
}

fn default_true() -> bool {
    true
}

async fn digest_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<DigestBody>,
) -> std::result::Result<Json<Value>, ApiError> {
    require_bearer(&state, &headers)?;
    let pipeline = AcademicPipeline::new(&state.config, &state.store);
    let res = pipeline.send_digest(Utc::now(), body.dry_run).await?;
    Ok(Json(
        serde_json::to_value(res).map_err(|e| ApiError::Internal(e.to_string()))?,
    ))
}

const DASHBOARD_HTML: &str = include_str!("dashboard.html");

async fn dashboard_handler() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}

#[derive(Deserialize)]
struct CronTickBody {
    #[serde(default = "default_true")]
    dry_run: bool,
}

async fn cron_tick_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<CronTickBody>,
) -> std::result::Result<Json<Value>, ApiError> {
    require_bearer(&state, &headers)?;
    let report = run_cron_tick(&state.store, &state.config, Utc::now(), body.dry_run).await?;
    Ok(Json(
        serde_json::to_value(report).map_err(|e| ApiError::Internal(e.to_string()))?,
    ))
}

#[derive(Deserialize)]
struct CalendarFeedQuery {
    token: Option<String>,
    include_completed: Option<bool>,
    course: Option<String>,
}

async fn calendar_feed_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<CalendarFeedQuery>,
) -> std::result::Result<Response, ApiError> {
    require_bearer_or_query_token(&state, &headers, query.token.as_deref())?;
    let include_completed = query.include_completed.unwrap_or(false);
    let tasks = state.store.list_tasks(None)?;
    let filtered = filter_tasks_for_feed(&tasks, include_completed, query.course.as_deref());
    let tz = state.config.timezone_tz()?;
    let feed = generate_ics_feed(
        &filtered,
        tz,
        state.config.general.default_duration_hours,
        "academic-agent Deadlines",
    );
    Ok((
        [
            (header::CONTENT_TYPE, "text/calendar; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                "inline; filename=\"academic-agent-feed.ics\"",
            ),
            (header::CACHE_CONTROL, "no-cache, max-age=300"),
        ],
        feed,
    )
        .into_response())
}

async fn download_calendar_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumPath(filename): AxumPath<String>,
) -> std::result::Result<Response, ApiError> {
    require_bearer(&state, &headers)?;
    let path = safe_join_within(&state.config.general.calendar_dir, &filename)?;
    if !path.exists() {
        return Err(ApiError::NotFound(format!(
            "calendar file '{}' not found",
            filename
        )));
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|e| ApiError::Internal(format!("failed reading calendar file: {e}")))?;
    Ok((
        [
            (header::CONTENT_TYPE, "text/calendar; charset=utf-8"),
            (header::CONTENT_DISPOSITION, "attachment"),
        ],
        content,
    )
        .into_response())
}

async fn options_preflight_handler(
    State(state): State<Arc<AppState>>,
    req_headers: HeaderMap,
    method: Method,
) -> Response {
    if method == Method::OPTIONS {
        let cors = build_cors_headers(&state, &req_headers);
        return (StatusCode::NO_CONTENT, cors).into_response();
    }
    StatusCode::METHOD_NOT_ALLOWED.into_response()
}

fn clean_secret(raw: &str) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() || t.contains("${") {
        None
    } else {
        Some(t.to_string())
    }
}

/// Constructs the complete Axum router combining REST endpoints and Streamable HTTP `/mcp`.
pub fn build_app_router(
    config_path: PathBuf,
    config: Config,
    store: TaskStore,
    token_override: Option<String>,
) -> Router {
    let api_token = token_override
        .and_then(|t| clean_secret(&t))
        .or_else(|| clean_secret(&config.server.api_token));
    let webdelta_secret = clean_secret(&config.server.webdelta_webhook_secret);
    let max_body = config.server.max_upload_bytes.saturating_add(64 * 1024);

    let mcp_server = Arc::new(McpServer::with_store(
        config_path.clone(),
        config.clone(),
        store.clone(),
    ));
    let mcp_state = Arc::new(McpHttpState {
        server: mcp_server,
        token: api_token.clone(),
    });

    let app_state = Arc::new(AppState {
        config,
        store,
        api_token,
        webdelta_secret,
    });

    Router::new()
        .route("/", get(dashboard_handler))
        .route("/health", get(health_handler))
        .route(
            "/api/tasks",
            get(list_tasks_handler)
                .post(create_task_handler)
                .options(options_preflight_handler),
        )
        .route("/api/tasks/export", get(export_tasks_handler))
        .route("/api/tasks/{id}/status", patch(update_status_handler))
        .route("/api/tasks/{id}/snooze", post(snooze_handler))
        .route("/api/tasks/{id}", delete(delete_task_handler))
        .route("/api/upcoming", get(upcoming_handler))
        .route("/api/extract", post(extract_handler))
        .route("/api/upload", post(upload_handler))
        .route("/api/webhooks/webdelta", post(webdelta_webhook_handler))
        .route("/api/digest", post(digest_handler))
        .route("/api/cron-tick", post(cron_tick_handler))
        .route("/api/calendar/feed.ics", get(calendar_feed_handler))
        .route("/api/calendar/{filename}", get(download_calendar_handler))
        .layer(DefaultBodyLimit::max(max_body))
        .with_state(app_state)
        .merge(mcp_route(mcp_state))
}

/// Runs the embedded HTTP server with optional background cron scheduler, folder watcher,
/// and interactive Discord Gateway bot.
pub async fn run_serve(
    config_path: PathBuf,
    config: Config,
    bind_override: Option<String>,
    token_override: Option<String>,
    with_cron: bool,
    with_watch: bool,
    with_discord_bot: bool,
) -> academic_core::Result<()> {
    config.ensure_runtime_dirs()?;
    let store = TaskStore::open(&config.general.db_path)?;
    let bind = bind_override.unwrap_or_else(|| config.server.bind.clone());
    let router = build_app_router(config_path, config.clone(), store.clone(), token_override);

    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let mut cron_handle = None;
    if with_cron {
        let cron_cfg = config.clone();
        let cron_store = store.clone();
        let mut cron_rx = shutdown_rx.clone();
        let tick_secs = cron_cfg.cron.tick_interval_secs.max(1);
        cron_handle = Some(tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(tick_secs));
            loop {
                tokio::select! {
                    biased;
                    _ = cron_rx.changed() => {
                        if *cron_rx.borrow() {
                            break;
                        }
                    }
                    _ = ticker.tick() => {
                        if let Err(e) = run_cron_tick(&cron_store, &cron_cfg, Utc::now(), false).await {
                            error!("Cron tick failed: {e}");
                        }
                    }
                }
            }
        }));
    }

    let mut watch_handle = None;
    if with_watch {
        let w_cfg = config.clone();
        let w_store = store.clone();
        let w_rx = shutdown_rx.clone();
        watch_handle = Some(tokio::spawn(async move {
            if let Err(e) = run_watcher_loop(w_cfg, w_store, w_rx).await {
                error!("Watcher loop exited with error: {e}");
            }
        }));
    }

    let mut bot_handle = None;
    if with_discord_bot {
        let b_cfg = config.clone();
        let b_store = store.clone();
        let b_rx = shutdown_rx.clone();
        bot_handle = Some(tokio::spawn(async move {
            if let Err(e) = run_discord_gateway(b_cfg, b_store, b_rx).await {
                error!("Discord Gateway exited with error: {e}");
            }
        }));
    }

    let listener = TcpListener::bind(&bind).await?;
    let local_addr: SocketAddr = listener.local_addr()?;
    info!("academic-agent server listening on http://{local_addr}");

    let shutdown_tx_ctrl = shutdown_tx.clone();
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            let _ = shutdown_tx_ctrl.send(true);
        })
        .await?;

    let _ = shutdown_tx.send(true);
    if let Some(h) = cron_handle {
        let _ = h.await;
    }
    if let Some(h) = watch_handle {
        let _ = h.await;
    }
    if let Some(h) = bot_handle {
        let _ = h.await;
    }
    Ok(())
}
