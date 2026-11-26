use std::time::Duration;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use hmac::{Hmac, Mac};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;
use tokio::process::Command;
use tracing::warn;

use crate::calendar::CalendarArtifact;
use crate::config::{DiscordConfig, DiscordMode, HermesConfig, N8nConfig};
use crate::error::{Error, Result};
use crate::extract::Priority;
use crate::store::TaskRecord;

type HmacSha256 = Hmac<Sha256>;

/// Receipt returned by a notification transport attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchReceipt {
    pub success: bool,
    pub transport: String,
    pub status_code: Option<u16>,
    pub detail: String,
}

fn truncate_chars(raw: &str, max_chars: usize) -> String {
    let trimmed = raw.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let prefix: String = trimmed.chars().take(max_chars.saturating_sub(3)).collect();
    format!("{prefix}...")
}

fn priority_color(priority: Priority) -> u32 {
    match priority {
        Priority::High => 0xED4245,
        Priority::Medium => 0xFEE75C,
        Priority::Low => 0x57F287,
    }
}

fn build_validated_fields(candidates: Vec<(&str, String, bool)>) -> Vec<Value> {
    candidates
        .into_iter()
        .filter(|(_, val, _)| !val.trim().is_empty())
        .take(25)
        .map(|(name, val, inline)| {
            json!({
                "name": truncate_chars(name, 256),
                "value": truncate_chars(&val, 1024),
                "inline": inline
            })
        })
        .collect()
}

/// Builds a Discord embed payload for a newly registered academic task.
pub fn build_task_embed(
    task: &TaskRecord,
    gcal_url: Option<&str>,
    include_markdown_link: bool,
) -> Value {
    let mut candidates = vec![
        ("Course", task.course.clone(), true),
        (
            "Deadline",
            task.deadline_raw
                .clone()
                .unwrap_or_else(|| "Unspecified".to_string()),
            true,
        ),
        ("Priority", task.priority.as_str().to_string(), true),
        (
            "Estimated Effort",
            format!("{}h", task.estimated_hours),
            true,
        ),
    ];

    if !task.subtasks.is_empty() {
        let joined = task
            .subtasks
            .iter()
            .take(6)
            .map(|s| format!("- {s}"))
            .collect::<Vec<_>>()
            .join("\n");
        candidates.push(("Checklist", joined, false));
    }

    if !task.deliverables.is_empty() {
        candidates.push(("Deliverables", task.deliverables.join(", "), true));
    }

    if let Some(ref ics) = task.ics_file {
        candidates.push(("Calendar File", format!("`{ics}`"), true));
    }

    if include_markdown_link {
        if let Some(url) = gcal_url {
            if !url.trim().is_empty() {
                candidates.push(("Google Calendar", format!("[Add Event]({url})"), false));
            }
        }
    }

    json!({
        "title": truncate_chars(&format!("[{}] {}", task.course, task.title), 256),
        "description": truncate_chars(&task.summary, 4096),
        "color": priority_color(task.priority),
        "fields": build_validated_fields(candidates),
        "footer": {
            "text": format!("academic-agent | task #{}", task.id)
        }
    })
}

/// Builds a Discord embed payload for a scheduled deadline reminder or overdue alert.
pub fn build_reminder_embed(task: &TaskRecord, label: &str, tz: Tz) -> Value {
    let local_deadline = task
        .deadline_utc
        .map(|d| d.with_timezone(&tz).format("%Y-%m-%d %H:%M %Z").to_string())
        .or_else(|| task.deadline_raw.clone())
        .unwrap_or_else(|| "Unspecified".to_string());

    let mut candidates = vec![
        ("Course", task.course.clone(), true),
        ("Alert Window", label.to_string(), true),
        ("Deadline", local_deadline, true),
        ("Status", task.status.as_str().to_string(), true),
    ];

    if let Some(ref url) = task.google_calendar_url {
        candidates.push(("Google Calendar", format!("[Open Event]({url})"), false));
    }

    json!({
        "title": truncate_chars(&format!("[{label}] {}", task.title), 256),
        "description": truncate_chars(&task.summary, 4096),
        "color": if label == "OVERDUE" { 0xED4245 } else { priority_color(task.priority) },
        "fields": build_validated_fields(candidates),
        "footer": {
            "text": format!("academic-agent cron | task #{}", task.id)
        }
    })
}

/// Builds a Discord embed payload summarizing active and upcoming academic tasks.
pub fn build_digest_embed(active_tasks: &[TaskRecord], tz: Tz, now_utc: DateTime<Utc>) -> Value {
    let today_str = now_utc.with_timezone(&tz).format("%Y-%m-%d").to_string();
    let lines: Vec<String> = active_tasks
        .iter()
        .take(12)
        .map(|t| {
            let dl = t
                .deadline_utc
                .map(|d| d.with_timezone(&tz).format("%m-%d %H:%M").to_string())
                .unwrap_or_else(|| "no deadline".to_string());
            format!(
                "**#{}** [{}] {} (due `{}`, {})",
                t.id,
                t.priority.as_str(),
                t.title,
                dl,
                t.status.as_str()
            )
        })
        .collect();

    let desc = if lines.is_empty() {
        "No pending or in-progress tasks scheduled.".to_string()
    } else {
        lines.join("\n")
    };

    json!({
        "title": format!("Daily Academic Digest ({today_str})"),
        "description": truncate_chars(&desc, 4096),
        "color": 0x5865F2,
        "fields": build_validated_fields(vec![
            ("Open Tasks", active_tasks.len().to_string(), true),
            ("Timezone", tz.name().to_string(), true),
        ]),
        "footer": {
            "text": "academic-agent daily digest"
        }
    })
}

fn is_configured_value(raw: &str) -> bool {
    let t = raw.trim();
    !t.is_empty() && !t.contains("${")
}

/// Sends a Discord embed payload via Bot REST API v10 (with optional link button) or Webhook,
/// handling HTTP `429 Retry-After` and transient 5xx responses with bounded retries.
pub async fn send_discord_embed(
    cfg: &DiscordConfig,
    embed: Value,
    button_url: Option<&str>,
) -> Result<DispatchReceipt> {
    if !cfg.enabled {
        return Err(Error::Notify("discord notifier is disabled".to_string()));
    }

    let has_bot = is_configured_value(&cfg.bot_token) && is_configured_value(&cfg.channel_id);
    let has_webhook = is_configured_value(&cfg.webhook_url);

    let use_bot = match cfg.mode {
        DiscordMode::Bot => {
            if !has_bot {
                return Err(Error::Notify(
                    "discord.mode='bot' requires bot_token and channel_id".to_string(),
                ));
            }
            true
        }
        DiscordMode::Webhook => {
            if !has_webhook {
                return Err(Error::Notify(
                    "discord.mode='webhook' requires webhook_url".to_string(),
                ));
            }
            false
        }
        DiscordMode::Auto => {
            if has_bot {
                true
            } else if has_webhook {
                false
            } else {
                return Err(Error::Notify(
                    "neither discord bot credentials nor webhook_url are configured".to_string(),
                ));
            }
        }
    };

    let (target_url, auth_header, transport_name, body) = if use_bot {
        let base = cfg.api_base.trim().trim_end_matches('/');
        let url = format!("{base}/channels/{}/messages", cfg.channel_id.trim());
        let mut payload = json!({ "embeds": [embed] });
        if let Some(link) = button_url {
            if !link.trim().is_empty() && link.len() <= 512 {
                payload["components"] = json!([{
                    "type": 1,
                    "components": [{
                        "type": 2,
                        "style": 5,
                        "label": "Add to Google Calendar",
                        "url": link
                    }]
                }]);
            }
        }
        (
            url,
            Some(format!("Bot {}", cfg.bot_token.trim())),
            "discord_bot".to_string(),
            payload,
        )
    } else {
        (
            cfg.webhook_url.trim().to_string(),
            None,
            "discord_webhook".to_string(),
            json!({ "embeds": [embed] }),
        )
    };

    let client = Client::builder()
        .timeout(Duration::from_secs(cfg.timeout_secs.max(1)))
        .build()
        .map_err(|e| Error::Http(format!("failed building Discord HTTP client: {e}")))?;

    let max_attempts = cfg.max_retries.saturating_add(1).max(1);
    for attempt in 1..=max_attempts {
        let mut req = client
            .post(&target_url)
            .header("User-Agent", "DiscordBot (academic-agent, 0.1.0)")
            .json(&body);
        if let Some(ref auth) = auth_header {
            req = req.header("Authorization", auth);
        }

        match req.send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    return Ok(DispatchReceipt {
                        success: true,
                        transport: transport_name,
                        status_code: Some(status.as_u16()),
                        detail: "dispatched".to_string(),
                    });
                }

                let retry_after_hdr = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<f64>().ok());
                let resp_text = resp.text().await.unwrap_or_default();

                if (status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error())
                    && attempt < max_attempts
                {
                    let body_retry = serde_json::from_str::<Value>(&resp_text)
                        .ok()
                        .and_then(|v| v["retry_after"].as_f64());
                    let wait_secs = retry_after_hdr
                        .or(body_retry)
                        .unwrap_or(0.25 * f64::from(attempt))
                        .clamp(0.01, 10.0);
                    warn!(
                        "Discord returned HTTP {status} on attempt {attempt}/{max_attempts}; retrying in {wait_secs:.2}s"
                    );
                    tokio::time::sleep(Duration::from_secs_f64(wait_secs)).await;
                    continue;
                }

                return Err(Error::Notify(format!(
                    "Discord returned HTTP {status}: {resp_text}"
                )));
            }
            Err(err) if attempt < max_attempts => {
                warn!("Discord transport error on attempt {attempt}/{max_attempts}: {err}");
                tokio::time::sleep(Duration::from_millis(200 * u64::from(attempt))).await;
            }
            Err(err) => {
                return Err(Error::Notify(format!("Discord request failed: {err}")));
            }
        }
    }

    Err(Error::Notify("Discord retry loop exhausted".to_string()))
}

/// Dispatches a plain text message via the configured Hermes CLI target with strict timeout.
pub async fn send_via_hermes(cfg: &HermesConfig, message: &str) -> Result<DispatchReceipt> {
    if !cfg.enabled {
        return Err(Error::Notify("hermes bridge is disabled".to_string()));
    }
    if !is_configured_value(&cfg.target) {
        return Err(Error::Notify(
            "hermes.target is empty or unconfigured".to_string(),
        ));
    }
    let bin = cfg.binary_path.trim();
    if bin.is_empty() {
        return Err(Error::Notify("hermes.binary_path is empty".to_string()));
    }

    let timeout = Duration::from_secs(cfg.timeout_secs.max(1));
    let mut cmd = Command::new(bin);
    cmd.args(["send", "--to", cfg.target.trim(), message]);

    let output = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| Error::Notify(format!("hermes CLI timed out after {}s", cfg.timeout_secs)))?
        .map_err(|e| Error::Notify(format!("failed executing '{bin}': {e}")))?;

    if output.status.success() {
        Ok(DispatchReceipt {
            success: true,
            transport: "hermes".to_string(),
            status_code: output.status.code().map(|c| c as u16),
            detail: String::from_utf8_lossy(&output.stdout).trim().to_string(),
        })
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(Error::Notify(format!(
            "hermes exited with {}: {}",
            output.status, stderr
        )))
    }
}

/// Dispatches an outbound event to n8n with optional HMAC-SHA256 `X-Academic-Signature`.
pub async fn send_n8n_event(cfg: &N8nConfig, payload: &Value) -> Result<DispatchReceipt> {
    if !cfg.enabled || !is_configured_value(&cfg.webhook_url) {
        return Err(Error::Notify(
            "n8n webhook is disabled or unconfigured".to_string(),
        ));
    }
    let body_bytes = serde_json::to_vec(payload)?;
    let client = Client::builder()
        .timeout(Duration::from_secs(cfg.timeout_secs.max(1)))
        .build()
        .map_err(|e| Error::Http(format!("failed building n8n HTTP client: {e}")))?;

    let mut req = client
        .post(cfg.webhook_url.trim())
        .header("Content-Type", "application/json")
        .body(body_bytes.clone());

    if is_configured_value(&cfg.secret) {
        let mut mac = HmacSha256::new_from_slice(cfg.secret.trim().as_bytes())
            .map_err(|e| Error::Notify(format!("invalid HMAC secret: {e}")))?;
        mac.update(&body_bytes);
        let sig = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
        req = req.header("X-Academic-Signature", sig);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| Error::Http(format!("n8n webhook failed: {e}")))?;
    let status = resp.status();
    if status.is_success() {
        Ok(DispatchReceipt {
            success: true,
            transport: "n8n".to_string(),
            status_code: Some(status.as_u16()),
            detail: "triggered".to_string(),
        })
    } else {
        let txt = resp.text().await.unwrap_or_default();
        Err(Error::Notify(format!("n8n returned HTTP {status}: {txt}")))
    }
}

/// Convenience dispatcher for new task announcements across Discord, Hermes, and n8n.
pub async fn dispatch_new_task(
    discord: &DiscordConfig,
    hermes: &HermesConfig,
    n8n: &N8nConfig,
    task: &TaskRecord,
    calendar: Option<&CalendarArtifact>,
) -> Vec<DispatchReceipt> {
    let mut receipts = Vec::new();
    let gcal = calendar
        .map(|c| c.google_calendar_url.as_str())
        .or(task.google_calendar_url.as_deref());

    let mut discord_ok = false;
    if discord.enabled {
        let is_bot_mode = matches!(discord.mode, DiscordMode::Bot)
            || (matches!(discord.mode, DiscordMode::Auto)
                && is_configured_value(&discord.bot_token)
                && is_configured_value(&discord.channel_id));
        let embed = build_task_embed(task, gcal, !is_bot_mode);
        match send_discord_embed(discord, embed, if is_bot_mode { gcal } else { None }).await {
            Ok(r) => {
                discord_ok = r.success;
                receipts.push(r);
            }
            Err(e) => receipts.push(DispatchReceipt {
                success: false,
                transport: "discord".to_string(),
                status_code: None,
                detail: e.to_string(),
            }),
        }
    }

    if hermes.enabled && (!discord_ok || !hermes.fallback_on_discord_error) {
        let msg = format!(
            "[{}] {} | Due: {} | Priority: {}",
            task.course,
            task.title,
            task.deadline_raw.as_deref().unwrap_or("unspecified"),
            task.priority.as_str()
        );
        match send_via_hermes(hermes, &msg).await {
            Ok(r) => receipts.push(r),
            Err(e) => receipts.push(DispatchReceipt {
                success: false,
                transport: "hermes".to_string(),
                status_code: None,
                detail: e.to_string(),
            }),
        }
    }

    if n8n.enabled {
        let payload = json!({
            "event": "new_academic_task",
            "task": task,
            "calendar": calendar,
        });
        match send_n8n_event(n8n, &payload).await {
            Ok(r) => receipts.push(r),
            Err(e) => receipts.push(DispatchReceipt {
                success: false,
                transport: "n8n".to_string(),
                status_code: None,
                detail: e.to_string(),
            }),
        }
    }

    receipts
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, StatusCode as AxumStatus};
    use axum::routing::post;
    use axum::{Json, Router};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn test_discord_429_retry_and_embed_limits() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_ref = attempts.clone();

        let app = Router::new().route(
            "/channels/{id}/messages",
            post(move |Json(body): Json<Value>| {
                let c = attempts_ref.fetch_add(1, Ordering::SeqCst);
                async move {
                    assert!(
                        body["embeds"][0]["fields"][0]["value"]
                            .as_str()
                            .unwrap()
                            .chars()
                            .count()
                            <= 1024
                    );
                    if c == 0 {
                        let mut headers = HeaderMap::new();
                        headers.insert("retry-after", "0.02".parse().unwrap());
                        (
                            AxumStatus::TOO_MANY_REQUESTS,
                            headers,
                            Json(json!({"retry_after": 0.02})),
                        )
                    } else {
                        (
                            AxumStatus::OK,
                            HeaderMap::new(),
                            Json(json!({"id": "msg-1"})),
                        )
                    }
                }
            }),
        );

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let cfg = DiscordConfig {
            enabled: true,
            mode: DiscordMode::Bot,
            bot_token: "test-token".to_string(),
            channel_id: "999".to_string(),
            webhook_url: String::new(),
            api_base: format!("http://{addr}"),
            allowed_users: vec![],
            timeout_secs: 5,
            max_retries: 2,
        };

        let long_field = "x".repeat(2000);
        let embed = json!({
            "title": "Test",
            "fields": build_validated_fields(vec![("Notes", long_field, false), ("Empty", "  ".to_string(), true)])
        });
        assert_eq!(embed["fields"].as_array().unwrap().len(), 1);

        let receipt = send_discord_embed(
            &cfg,
            embed,
            Some("https://calendar.google.com/calendar/render?action=TEMPLATE"),
        )
        .await
        .expect("retry recovers on second attempt");
        assert!(receipt.success);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_hermes_rejects_empty_target() {
        let cfg = HermesConfig {
            enabled: true,
            binary_path: "hermes".to_string(),
            target: "".to_string(),
            timeout_secs: 5,
            fallback_on_discord_error: false,
        };
        let err = send_via_hermes(&cfg, "hello").await.unwrap_err();
        assert!(err.to_string().contains("empty"));
    }
}
