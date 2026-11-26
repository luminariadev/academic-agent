use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde_json::{json, Value};
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{debug, info, warn};

use crate::calendar::{create_ics_event, parse_deadline_in_tz, CalendarEventSpec};
use crate::config::{Config, DiscordConfig};
use crate::error::{Error, Result};
use crate::extract::{Priority, TaskKind};
use crate::notify::{build_digest_embed, build_task_embed};
use crate::parser::sha256_hex;
use crate::store::{InsertOutcome, NewTask, TaskStatus, TaskStore};

/// Returns Discord Application Command schemas for `/tasks`, `/upcoming`, `/done`, `/snooze`,
/// `/add`, and `/digest`.
pub fn slash_command_definitions() -> Value {
    json!([
        {
            "name": "tasks",
            "type": 1,
            "description": "List tracked academic tasks",
            "options": [
                {
                    "name": "status",
                    "description": "Optional status filter",
                    "type": 3,
                    "required": false,
                    "choices": [
                        { "name": "Pending", "value": "PENDING" },
                        { "name": "In Progress", "value": "IN_PROGRESS" },
                        { "name": "Completed", "value": "COMPLETED" },
                        { "name": "Archived", "value": "ARCHIVED" }
                    ]
                }
            ]
        },
        {
            "name": "upcoming",
            "type": 1,
            "description": "Show incomplete deadlines due within N hours (default 72)",
            "options": [
                {
                    "name": "hours",
                    "description": "Lookahead window in hours (default 72)",
                    "type": 4,
                    "required": false,
                    "min_value": 1
                }
            ]
        },
        {
            "name": "done",
            "type": 1,
            "description": "Mark a task as COMPLETED",
            "options": [
                {
                    "name": "task_id",
                    "description": "Numeric task ID",
                    "type": 4,
                    "required": true,
                    "min_value": 1
                }
            ]
        },
        {
            "name": "snooze",
            "type": 1,
            "description": "Snooze cron deadline reminders for a task",
            "options": [
                {
                    "name": "task_id",
                    "description": "Numeric task ID",
                    "type": 4,
                    "required": true,
                    "min_value": 1
                },
                {
                    "name": "hours",
                    "description": "Hours to snooze (default 24)",
                    "type": 4,
                    "required": false,
                    "min_value": 1
                }
            ]
        },
        {
            "name": "add",
            "type": 1,
            "description": "Register an academic task and generate a calendar event",
            "options": [
                {
                    "name": "title",
                    "description": "Task title",
                    "type": 3,
                    "required": true
                },
                {
                    "name": "course",
                    "description": "Course name",
                    "type": 3,
                    "required": false
                },
                {
                    "name": "deadline",
                    "description": "Deadline string (e.g. 2026-12-01 23:59)",
                    "type": 3,
                    "required": false
                },
                {
                    "name": "priority",
                    "description": "Task urgency level",
                    "type": 3,
                    "required": false,
                    "choices": [
                        { "name": "HIGH", "value": "HIGH" },
                        { "name": "MEDIUM", "value": "MEDIUM" },
                        { "name": "LOW", "value": "LOW" }
                    ]
                }
            ]
        },
        {
            "name": "digest",
            "type": 1,
            "description": "Show a live digest of currently open academic tasks"
        }
    ])
}

fn is_configured(raw: &str) -> bool {
    let t = raw.trim();
    !t.is_empty() && !t.contains("${")
}

/// Registers all slash commands on Discord globally or for a specific guild (`guild_id`).
pub async fn register_slash_commands(
    cfg: &DiscordConfig,
    guild_override: Option<&str>,
) -> Result<usize> {
    if !is_configured(&cfg.bot_token) {
        return Err(Error::Config(
            "discord.bot_token must be configured to register slash commands".to_string(),
        ));
    }
    if !is_configured(&cfg.application_id) {
        return Err(Error::Config(
            "discord.application_id (DISCORD_CLIENT_ID) must be configured to register slash commands"
                .to_string(),
        ));
    }

    let base = cfg.api_base.trim().trim_end_matches('/');
    let app_id = cfg.application_id.trim();
    let guild = guild_override
        .map(str::trim)
        .filter(|s| is_configured(s))
        .or_else(|| {
            if is_configured(&cfg.guild_id) {
                Some(cfg.guild_id.trim())
            } else {
                None
            }
        });

    let url = match guild {
        Some(gid) => format!("{base}/applications/{app_id}/guilds/{gid}/commands"),
        None => format!("{base}/applications/{app_id}/commands"),
    };

    let commands = slash_command_definitions();
    let expected_count = commands.as_array().map(|a| a.len()).unwrap_or(0);

    let client = Client::builder()
        .timeout(Duration::from_secs(cfg.timeout_secs.max(1)))
        .build()
        .map_err(|e| Error::Http(format!("failed building Discord command client: {e}")))?;

    let resp = client
        .put(&url)
        .header("Authorization", format!("Bot {}", cfg.bot_token.trim()))
        .header(
            "User-Agent",
            concat!(
                "DiscordBot (academic-agent, ",
                env!("CARGO_PKG_VERSION"),
                ")"
            ),
        )
        .json(&commands)
        .send()
        .await
        .map_err(|e| Error::Http(format!("command registration request failed: {e}")))?;

    let status = resp.status();
    if !status.is_success() {
        let txt = resp.text().await.unwrap_or_default();
        return Err(Error::Notify(format!(
            "Discord rejected slash command registration ({status}): {txt}"
        )));
    }

    let body: Value = resp.json().await.unwrap_or(Value::Null);
    Ok(body.as_array().map(|a| a.len()).unwrap_or(expected_count))
}

/// Checks whether a Discord user ID is allowed to run interactive commands and buttons.
/// An empty allowlist allows any user; a non-empty allowlist requires an exact match.
pub fn is_user_allowed(cfg: &DiscordConfig, user_id: Option<&str>) -> bool {
    if cfg.allowed_users.is_empty() {
        return true;
    }
    let Some(uid) = user_id.map(str::trim).filter(|s| !s.is_empty()) else {
        return false;
    };
    cfg.allowed_users
        .iter()
        .any(|allowed| allowed.trim() == uid)
}

fn extract_interaction_user_id(interaction: &Value) -> Option<&str> {
    interaction["member"]["user"]["id"]
        .as_str()
        .or_else(|| interaction["user"]["id"].as_str())
}

fn option_val<'a>(interaction: &'a Value, name: &str) -> Option<&'a Value> {
    interaction["data"]["options"]
        .as_array()?
        .iter()
        .find(|opt| opt["name"].as_str() == Some(name))
        .map(|opt| &opt["value"])
}

fn ephemeral_msg(text: &str) -> Value {
    json!({
        "type": 4,
        "data": {
            "content": text,
            "flags": 64
        }
    })
}

fn channel_embed_msg(embed: Value, content: Option<&str>) -> Value {
    let mut data = json!({ "embeds": [embed] });
    if let Some(txt) = content {
        data["content"] = json!(txt);
    }
    json!({
        "type": 4,
        "data": data
    })
}

/// Processes a Discord `INTERACTION_CREATE` payload synchronously against `TaskStore` and
/// returns the JSON body for Discord's Interaction Callback endpoint (`type: 4`).
pub fn handle_discord_interaction(
    config: &Config,
    store: &TaskStore,
    interaction: &Value,
) -> Value {
    let kind = interaction["type"].as_u64().unwrap_or(0);
    if kind == 1 {
        return json!({ "type": 1 });
    }

    let user_id = extract_interaction_user_id(interaction);
    if !is_user_allowed(&config.discord, user_id) {
        return ephemeral_msg(
            "Permission denied: your Discord user ID is not listed in discord.allowed_users.",
        );
    }

    match kind {
        2 => handle_slash_command(config, store, interaction),
        3 => handle_component_button(store, interaction),
        other => ephemeral_msg(&format!("Unsupported Discord interaction type: {other}")),
    }
}

fn handle_slash_command(config: &Config, store: &TaskStore, interaction: &Value) -> Value {
    let cmd = interaction["data"]["name"].as_str().unwrap_or("");
    let tz = match config.timezone_tz() {
        Ok(tz) => tz,
        Err(e) => return ephemeral_msg(&format!("Configuration error: {e}")),
    };

    match cmd {
        "tasks" => {
            let status_filter = match option_val(interaction, "status").and_then(|v| v.as_str()) {
                Some(s) if !s.trim().is_empty() => match TaskStatus::parse_loose(s) {
                    Ok(st) => Some(st),
                    Err(e) => return ephemeral_msg(&e.to_string()),
                },
                _ => None,
            };
            match store.list_tasks(status_filter) {
                Ok(tasks) => {
                    let lines: Vec<String> = tasks
                        .iter()
                        .take(15)
                        .map(|t| {
                            format!(
                                "**#{}** [{}] [{}] {} (due `{}`)",
                                t.id,
                                t.status.as_str(),
                                t.priority.as_str(),
                                t.title,
                                t.deadline_raw.as_deref().unwrap_or("none")
                            )
                        })
                        .collect();
                    let desc = if lines.is_empty() {
                        "No matching tasks recorded.".to_string()
                    } else {
                        lines.join("\n")
                    };
                    channel_embed_msg(
                        json!({
                            "title": format!("Academic Tasks ({})", tasks.len()),
                            "description": desc,
                            "color": 0x5865F2
                        }),
                        None,
                    )
                }
                Err(e) => ephemeral_msg(&format!("Failed reading tasks: {e}")),
            }
        }
        "upcoming" => {
            let hours = option_val(interaction, "hours")
                .and_then(|v| v.as_i64())
                .unwrap_or(72)
                .max(1);
            match store.upcoming_tasks(hours, Utc::now()) {
                Ok(tasks) => {
                    let lines: Vec<String> = tasks
                        .iter()
                        .take(15)
                        .map(|t| {
                            let local_dl = t
                                .deadline_utc
                                .map(|d| d.with_timezone(&tz).format("%Y-%m-%d %H:%M").to_string())
                                .unwrap_or_else(|| "none".to_string());
                            format!(
                                "**#{}** [{}] {} (`{}`)",
                                t.id,
                                t.priority.as_str(),
                                t.title,
                                local_dl
                            )
                        })
                        .collect();
                    let desc = if lines.is_empty() {
                        format!("No incomplete tasks due within {hours}h.")
                    } else {
                        lines.join("\n")
                    };
                    channel_embed_msg(
                        json!({
                            "title": format!("Upcoming Deadlines (next {hours}h: {})", tasks.len()),
                            "description": desc,
                            "color": 0xFEE75C
                        }),
                        None,
                    )
                }
                Err(e) => ephemeral_msg(&format!("Failed querying deadlines: {e}")),
            }
        }
        "done" => {
            let Some(id) = option_val(interaction, "task_id").and_then(|v| v.as_u64()) else {
                return ephemeral_msg("Missing integer 'task_id' option.");
            };
            match store.update_status(id, TaskStatus::Completed) {
                Ok(t) => ephemeral_msg(&format!(
                    "Task #{} ([{}] {}) marked as COMPLETED.",
                    t.id, t.course, t.title
                )),
                Err(e) => ephemeral_msg(&format!("Could not update task #{id}: {e}")),
            }
        }
        "snooze" => {
            let Some(id) = option_val(interaction, "task_id").and_then(|v| v.as_u64()) else {
                return ephemeral_msg("Missing integer 'task_id' option.");
            };
            let hours = option_val(interaction, "hours")
                .and_then(|v| v.as_i64())
                .unwrap_or(24)
                .max(1);
            let until = Utc::now() + chrono::Duration::hours(hours);
            match store.snooze_task(id, until) {
                Ok(t) => ephemeral_msg(&format!(
                    "Snoozed reminders for task #{} ({}) for {hours}h.",
                    t.id, t.title
                )),
                Err(e) => ephemeral_msg(&format!("Could not snooze task #{id}: {e}")),
            }
        }
        "add" => {
            let Some(title) = option_val(interaction, "title")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                return ephemeral_msg("'title' is required.");
            };
            let course = option_val(interaction, "course")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("General Course");
            let deadline_raw = option_val(interaction, "deadline")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            let priority = option_val(interaction, "priority")
                .and_then(|v| v.as_str())
                .map(Priority::parse_loose)
                .unwrap_or(Priority::Medium);

            let deadline_utc = deadline_raw
                .as_deref()
                .and_then(|d| parse_deadline_in_tz(d, tz))
                .map(|dt| dt.with_timezone(&Utc));
            let dedup_seed = format!(
                "discord:{course}:{title}:{}",
                deadline_raw.as_deref().unwrap_or("")
            );

            let outcome = match store.insert_or_dedup(NewTask {
                title: title.to_string(),
                course: course.to_string(),
                kind: TaskKind::Assignment,
                deadline_raw,
                deadline_utc,
                priority,
                estimated_hours: 2,
                summary: format!("{course}: {title}"),
                subtasks: Vec::new(),
                deliverables: Vec::new(),
                ics_file: None,
                google_calendar_url: None,
                source_file: Some("discord:slash".to_string()),
                content_hash: Some(sha256_hex(&dedup_seed)),
                extractor: "discord_bot".to_string(),
            }) {
                Ok(out) => out,
                Err(e) => return ephemeral_msg(&format!("Failed saving task: {e}")),
            };

            let (duplicate, mut task) = match outcome {
                InsertOutcome::Duplicate(t) => (true, t),
                InsertOutcome::Inserted(t) => (false, t),
            };

            if !duplicate && (task.deadline_raw.is_some() || task.deadline_utc.is_some()) {
                if let Ok(cal) = create_ics_event(CalendarEventSpec {
                    calendar_dir: &config.general.calendar_dir,
                    title: &task.title,
                    deadline_str: task.deadline_raw.as_deref(),
                    tz,
                    course: &task.course,
                    description: &task.summary,
                    duration_hours: config.general.default_duration_hours,
                    priority: task.priority,
                }) {
                    if let Ok(updated) = store.update_calendar_info(
                        task.id,
                        Some(cal.filename),
                        Some(cal.google_calendar_url),
                    ) {
                        task = updated;
                    }
                }
            }

            let gcal = task.google_calendar_url.as_deref();
            let embed = build_task_embed(&task, gcal, true);
            let banner = if duplicate {
                Some("Task already existed (deduplicated):")
            } else {
                Some("Registered new task:")
            };
            channel_embed_msg(embed, banner)
        }
        "digest" => {
            let now = Utc::now();
            match store.list_tasks(None) {
                Ok(all) => {
                    let active: Vec<_> = all
                        .into_iter()
                        .filter(|t| {
                            matches!(t.status, TaskStatus::Pending | TaskStatus::InProgress)
                        })
                        .collect();
                    channel_embed_msg(build_digest_embed(&active, tz, now), None)
                }
                Err(e) => ephemeral_msg(&format!("Failed generating digest: {e}")),
            }
        }
        other => ephemeral_msg(&format!("Unknown slash command '/{other}'.")),
    }
}

fn handle_component_button(store: &TaskStore, interaction: &Value) -> Value {
    let custom_id = interaction["data"]["custom_id"].as_str().unwrap_or("");
    if let Some(id_str) = custom_id.strip_prefix("academic:done:") {
        let Ok(task_id) = id_str.parse::<u64>() else {
            return ephemeral_msg("Invalid task ID in button custom_id.");
        };
        return match store.update_status(task_id, TaskStatus::Completed) {
            Ok(t) => ephemeral_msg(&format!(
                "Task #{} ([{}] {}) marked as COMPLETED.",
                t.id, t.course, t.title
            )),
            Err(e) => ephemeral_msg(&format!("Failed completing task #{task_id}: {e}")),
        };
    }

    if let Some(id_str) = custom_id.strip_prefix("academic:snooze24:") {
        let Ok(task_id) = id_str.parse::<u64>() else {
            return ephemeral_msg("Invalid task ID in button custom_id.");
        };
        let until = Utc::now() + chrono::Duration::hours(24);
        return match store.snooze_task(task_id, until) {
            Ok(t) => ephemeral_msg(&format!(
                "Snoozed task #{} ({}) reminders for 24 hours.",
                t.id, t.title
            )),
            Err(e) => ephemeral_msg(&format!("Failed snoozing task #{task_id}: {e}")),
        };
    }

    ephemeral_msg(&format!("Unknown button action '{custom_id}'."))
}

async fn resolve_gateway_ws_url(cfg: &DiscordConfig, client: &Client) -> Result<String> {
    if is_configured(&cfg.gateway_url) {
        return Ok(cfg.gateway_url.trim().to_string());
    }
    let base = cfg.api_base.trim().trim_end_matches('/');
    let url = format!("{base}/gateway/bot");
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bot {}", cfg.bot_token.trim()))
        .header(
            "User-Agent",
            concat!(
                "DiscordBot (academic-agent, ",
                env!("CARGO_PKG_VERSION"),
                ")"
            ),
        )
        .send()
        .await
        .map_err(|e| Error::Http(format!("failed querying Discord gateway endpoint: {e}")))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(Error::Notify(format!(
            "Discord /gateway/bot returned HTTP {status}: {body}"
        )));
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|e| Error::Parse(format!("invalid /gateway/bot JSON: {e}")))?;
    let ws_base = body["url"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| Error::Notify("Discord /gateway/bot response omitted 'url'".to_string()))?;
    if ws_base.contains('?') {
        Ok(ws_base.to_string())
    } else {
        Ok(format!("{ws_base}?v=10&encoding=json"))
    }
}

async fn send_interaction_callback(
    client: &Client,
    cfg: &DiscordConfig,
    interaction_id: &str,
    interaction_token: &str,
    payload: &Value,
) -> Result<()> {
    let base = cfg.api_base.trim().trim_end_matches('/');
    let url = format!("{base}/interactions/{interaction_id}/{interaction_token}/callback");
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bot {}", cfg.bot_token.trim()))
        .header(
            "User-Agent",
            concat!(
                "DiscordBot (academic-agent, ",
                env!("CARGO_PKG_VERSION"),
                ")"
            ),
        )
        .json(payload)
        .send()
        .await
        .map_err(|e| Error::Http(format!("interaction callback request failed: {e}")))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let txt = resp.text().await.unwrap_or_default();
        return Err(Error::Notify(format!(
            "interaction callback returned HTTP {status}: {txt}"
        )));
    }
    Ok(())
}

/// Runs the persistent Discord Gateway v10 WebSocket loop to receive slash command and button
/// interactions in real time with automatic reconnect and clean shutdown on `shutdown_rx`.
pub async fn run_discord_gateway(
    config: Config,
    store: TaskStore,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    if !config.discord.enabled {
        return Err(Error::Config("discord.enabled is false".to_string()));
    }
    if !is_configured(&config.discord.bot_token) {
        return Err(Error::Config(
            "discord.bot_token must be configured to start Discord Gateway".to_string(),
        ));
    }

    let client = Client::builder()
        .timeout(Duration::from_secs(config.discord.timeout_secs.max(1)))
        .build()
        .map_err(|e| Error::Http(format!("failed building Discord Gateway HTTP client: {e}")))?;

    let mut backoff_secs = 1u64;
    loop {
        if *shutdown_rx.borrow() {
            break;
        }

        match run_single_gateway_session(&config, &store, &client, &mut shutdown_rx).await {
            Ok(SessionExit::Shutdown) => break,
            Ok(SessionExit::Reconnect) => {
                info!("Discord Gateway requested reconnect; reconnecting...");
                backoff_secs = 1;
            }
            Err(e) => {
                warn!("Discord Gateway session error ({e}); reconnecting in {backoff_secs}s");
                tokio::select! {
                    biased;
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() { break; }
                    }
                    _ = tokio::time::sleep(Duration::from_secs(backoff_secs)) => {}
                }
                backoff_secs = (backoff_secs * 2).min(30);
            }
        }
    }

    info!("Discord Gateway shut down cleanly.");
    Ok(())
}

enum SessionExit {
    Shutdown,
    Reconnect,
}

async fn run_single_gateway_session(
    config: &Config,
    store: &TaskStore,
    client: &Client,
    shutdown_rx: &mut watch::Receiver<bool>,
) -> Result<SessionExit> {
    let ws_url = resolve_gateway_ws_url(&config.discord, client).await?;
    let (ws_stream, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .map_err(|e| Error::Http(format!("WebSocket connect to '{ws_url}' failed: {e}")))?;
    let (mut write, mut read) = ws_stream.split();

    // First frame from Discord Gateway must be Op 10 (Hello) containing heartbeat_interval
    let hello_raw = read
        .next()
        .await
        .ok_or_else(|| Error::Notify("Discord Gateway closed before sending Hello".to_string()))?
        .map_err(|e| Error::Http(format!("WebSocket read error waiting for Hello: {e}")))?;
    let hello_text = hello_raw
        .into_text()
        .map_err(|e| Error::Notify(format!("non-text Hello payload from Discord Gateway: {e}")))?;
    let hello_json: Value = serde_json::from_str(&hello_text)?;
    let heartbeat_ms = hello_json["d"]["heartbeat_interval"]
        .as_u64()
        .unwrap_or(41_250)
        .max(10);

    // Send Op 2 Identify with zero privileged intents (slash commands & component buttons need 0)
    let identify = json!({
        "op": 2,
        "d": {
            "token": config.discord.bot_token.trim(),
            "intents": 0,
            "properties": {
                "os": std::env::consts::OS,
                "browser": "academic-agent",
                "device": "academic-agent"
            }
        }
    });
    write
        .send(WsMessage::Text(identify.to_string().into()))
        .await
        .map_err(|e| Error::Http(format!("failed sending Discord Identify: {e}")))?;

    let seq = AtomicI64::new(-1);
    let mut hb_interval = tokio::time::interval(Duration::from_millis(heartbeat_ms));
    // Skip immediate zero-delay tick
    hb_interval.tick().await;

    loop {
        tokio::select! {
            biased;
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    let _ = write.close().await;
                    return Ok(SessionExit::Shutdown);
                }
            }
            _ = hb_interval.tick() => {
                let s = seq.load(Ordering::SeqCst);
                let hb = json!({
                    "op": 1,
                    "d": if s >= 0 { json!(s) } else { Value::Null }
                });
                write
                    .send(WsMessage::Text(hb.to_string().into()))
                    .await
                    .map_err(|e| Error::Http(format!("failed sending heartbeat: {e}")))?;
            }
            maybe_msg = read.next() => {
                let Some(frame) = maybe_msg else {
                    return Ok(SessionExit::Reconnect);
                };
                let msg = frame.map_err(|e| Error::Http(format!("WebSocket read error: {e}")))?;
                match msg {
                    WsMessage::Text(txt) => {
                        let parsed: Value = match serde_json::from_str(txt.as_str()) {
                            Ok(v) => v,
                            Err(e) => {
                                debug!("Ignoring malformed gateway frame: {e}");
                                continue;
                            }
                        };
                        if let Some(s) = parsed["s"].as_i64() {
                            seq.store(s, Ordering::SeqCst);
                        }
                        let op = parsed["op"].as_u64().unwrap_or(999);
                        match op {
                            0 => {
                                if parsed["t"].as_str() == Some("INTERACTION_CREATE") {
                                    let data = &parsed["d"];
                                    let int_id = data["id"].as_str().unwrap_or("").to_string();
                                    let int_token = data["token"].as_str().unwrap_or("").to_string();
                                    if !int_id.is_empty() && !int_token.is_empty() {
                                        let reply = handle_discord_interaction(config, store, data);
                                        if let Err(e) = send_interaction_callback(
                                            client,
                                            &config.discord,
                                            &int_id,
                                            &int_token,
                                            &reply,
                                        )
                                        .await
                                        {
                                            warn!("Failed responding to interaction {int_id}: {e}");
                                        }
                                    }
                                }
                            }
                            1 => {
                                let s = seq.load(Ordering::SeqCst);
                                let hb = json!({
                                    "op": 1,
                                    "d": if s >= 0 { json!(s) } else { Value::Null }
                                });
                                let _ = write.send(WsMessage::Text(hb.to_string().into())).await;
                            }
                            7 | 9 => return Ok(SessionExit::Reconnect),
                            11 => debug!("Discord heartbeat ACK received"),
                            _ => {}
                        }
                    }
                    WsMessage::Close(_) => return Ok(SessionExit::Reconnect),
                    WsMessage::Ping(payload) => {
                        let _ = write.send(WsMessage::Pong(payload)).await;
                    }
                    _ => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::ws::{Message as AxumWsMsg, WebSocket, WebSocketUpgrade};
    use axum::extract::Path;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use std::sync::{Arc, Mutex};
    use tempfile::tempdir;
    use tokio::net::TcpListener;

    #[test]
    fn test_interaction_handlers_allowlist_commands_and_buttons() {
        let dir = tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.resolve_paths_relative_to(dir.path());
        cfg.ensure_runtime_dirs().unwrap();
        cfg.discord.allowed_users = vec!["111".to_string()];

        let store = TaskStore::open(&cfg.general.db_path).unwrap();

        // Unauthorized user rejected
        let denied = handle_discord_interaction(
            &cfg,
            &store,
            &json!({
                "type": 2,
                "member": { "user": { "id": "999" } },
                "data": { "name": "tasks" }
            }),
        );
        assert_eq!(denied["data"]["flags"], 64);
        assert!(denied["data"]["content"]
            .as_str()
            .unwrap()
            .contains("Permission denied"));

        // Authorized /add
        let added = handle_discord_interaction(
            &cfg,
            &store,
            &json!({
                "type": 2,
                "member": { "user": { "id": "111" } },
                "data": {
                    "name": "add",
                    "options": [
                        { "name": "title", "value": "Security Audit" },
                        { "name": "course", "value": "InfoSec" },
                        { "name": "deadline", "value": "2026-12-20 23:59" },
                        { "name": "priority", "value": "HIGH" }
                    ]
                }
            }),
        );
        assert!(added["data"]["embeds"][0]["title"]
            .as_str()
            .unwrap()
            .contains("Security Audit"));

        // /tasks lists the task
        let listed = handle_discord_interaction(
            &cfg,
            &store,
            &json!({
                "type": 2,
                "user": { "id": "111" },
                "data": { "name": "tasks" }
            }),
        );
        assert!(listed["data"]["embeds"][0]["description"]
            .as_str()
            .unwrap()
            .contains("Security Audit"));

        // Component button snooze + done
        let snooze_btn = handle_discord_interaction(
            &cfg,
            &store,
            &json!({
                "type": 3,
                "member": { "user": { "id": "111" } },
                "data": { "custom_id": "academic:snooze24:1" }
            }),
        );
        assert!(snooze_btn["data"]["content"]
            .as_str()
            .unwrap()
            .contains("Snoozed"));

        let done_btn = handle_discord_interaction(
            &cfg,
            &store,
            &json!({
                "type": 3,
                "member": { "user": { "id": "111" } },
                "data": { "custom_id": "academic:done:1" }
            }),
        );
        assert!(done_btn["data"]["content"]
            .as_str()
            .unwrap()
            .contains("COMPLETED"));
        assert_eq!(
            store.get_task(1).unwrap().unwrap().status,
            TaskStatus::Completed
        );
    }

    #[tokio::test]
    async fn test_gateway_handles_hello_identify_and_interaction_callback() {
        let dir = tempdir().unwrap();
        let callbacks = Arc::new(Mutex::new(Vec::<Value>::new()));
        let cb_ref = callbacks.clone();

        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
        let done_tx = Arc::new(Mutex::new(Some(done_tx)));

        let app = Router::new()
            .route(
                "/gateway",
                get(|ws: WebSocketUpgrade| async move {
                    ws.on_upgrade(|mut socket: WebSocket| async move {
                        let hello = json!({ "op": 10, "d": { "heartbeat_interval": 5000 } });
                        socket
                            .send(AxumWsMsg::Text(hello.to_string().into()))
                            .await
                            .unwrap();

                        // Expect Op 2 Identify
                        if let Some(Ok(AxumWsMsg::Text(txt))) = socket.next().await {
                            let msg: Value = serde_json::from_str(txt.as_str()).unwrap();
                            assert_eq!(msg["op"], 2);
                        }

                        // Emit INTERACTION_CREATE for /digest
                        let event = json!({
                            "op": 0,
                            "s": 1,
                            "t": "INTERACTION_CREATE",
                            "d": {
                                "id": "int-42",
                                "token": "tok-abc",
                                "type": 2,
                                "user": { "id": "777" },
                                "data": { "name": "digest" }
                            }
                        });
                        socket
                            .send(AxumWsMsg::Text(event.to_string().into()))
                            .await
                            .unwrap();
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    })
                }),
            )
            .route(
                "/interactions/{id}/{token}/callback",
                post(
                    move |Path((id, token)): Path<(String, String)>, Json(body): Json<Value>| {
                        let cb_ref = cb_ref.clone();
                        let done_tx = done_tx.clone();
                        async move {
                            assert_eq!(id, "int-42");
                            assert_eq!(token, "tok-abc");
                            cb_ref.lock().unwrap().push(body);
                            if let Some(tx) = done_tx.lock().unwrap().take() {
                                let _ = tx.send(());
                            }
                            axum::http::StatusCode::NO_CONTENT
                        }
                    },
                ),
            );

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let mut cfg = Config::default();
        cfg.resolve_paths_relative_to(dir.path());
        cfg.ensure_runtime_dirs().unwrap();
        cfg.discord.enabled = true;
        cfg.discord.bot_token = "bot-secret".to_string();
        cfg.discord.api_base = format!("http://{addr}");
        cfg.discord.gateway_url = format!("ws://{addr}/gateway");

        let store = TaskStore::open(&cfg.general.db_path).unwrap();
        let (shut_tx, shut_rx) = watch::channel(false);

        let gw_handle = tokio::spawn(run_discord_gateway(cfg, store, shut_rx));

        tokio::time::timeout(Duration::from_secs(5), done_rx)
            .await
            .expect("gateway processed interaction callback cleanly")
            .unwrap();

        let _ = shut_tx.send(true);
        gw_handle.await.unwrap().unwrap();

        let recorded = callbacks.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0]["type"], 4);
        assert!(recorded[0]["data"]["embeds"][0]["title"]
            .as_str()
            .unwrap()
            .contains("Daily Academic Digest"));
    }
}
