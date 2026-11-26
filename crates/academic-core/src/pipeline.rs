use std::path::Path;

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::calendar::{
    create_ics_event, parse_deadline_in_tz, CalendarArtifact, CalendarEventSpec,
};
use crate::config::Config;
use crate::error::Result;
use crate::extract::extract_from_text;
use crate::notify::{
    build_digest_embed, dispatch_new_task, send_discord_embed, send_via_hermes, DispatchReceipt,
};
use crate::parser::{parse_document, safe_join_within, sha256_hex};
use crate::store::{InsertOutcome, NewTask, TaskRecord, TaskStatus, TaskStore};

type HmacSha256 = Hmac<Sha256>;

/// Verifies a constant-time HMAC-SHA256 webhook signature header (`sha256=<hex>` or `<hex>`).
pub fn verify_hmac_sha256(secret: &str, body: &[u8], signature_header: &str) -> bool {
    let clean_secret = secret.trim();
    if clean_secret.is_empty() {
        return false;
    }
    let hex_part = signature_header
        .trim()
        .strip_prefix("sha256=")
        .unwrap_or(signature_header.trim());
    let Ok(expected_bytes) = hex::decode(hex_part) else {
        return false;
    };
    let Ok(mut mac) = HmacSha256::new_from_slice(clean_secret.as_bytes()) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&expected_bytes).is_ok()
}

/// Incoming webhook payload emitted by `webdelta` monitors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebdeltaWebhookPayload {
    #[serde(default)]
    pub event: Option<String>,
    pub watch_id: String,
    #[serde(default)]
    pub watch_name: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub current_value: Option<String>,
    #[serde(default)]
    pub diff: Option<String>,
    #[serde(default)]
    pub checked_at: Option<String>,
}

/// Outcome returned after running the full academic ingestion pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestResult {
    pub duplicate: bool,
    pub task: TaskRecord,
    pub calendar: Option<CalendarArtifact>,
    pub archived_path: Option<String>,
    pub notifications: Vec<DispatchReceipt>,
}

/// Outcome returned after sending a daily digest summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DigestResult {
    pub open_task_count: usize,
    pub dry_run: bool,
    pub receipts: Vec<DispatchReceipt>,
}

/// Coordinates parsing, extraction, deduplication, calendar sync, storage, archiving, and alerts.
pub struct AcademicPipeline<'a> {
    config: &'a Config,
    store: &'a TaskStore,
}

impl<'a> AcademicPipeline<'a> {
    pub fn new(config: &'a Config, store: &'a TaskStore) -> Self {
        Self { config, store }
    }

    /// Ingests raw text through extraction, deduplication, calendar generation, and notifications.
    pub async fn ingest_text(
        &self,
        text: &str,
        filename: &str,
        source_label: Option<&str>,
        hash_override: Option<String>,
        force_heuristic: bool,
        dispatch: bool,
    ) -> Result<IngestResult> {
        let tz = self.config.timezone_tz()?;
        let extracted =
            extract_from_text(&self.config.llm, text, filename, tz, force_heuristic).await?;
        let deadline_utc = extracted
            .deadline
            .as_deref()
            .and_then(|d| parse_deadline_in_tz(d, tz))
            .map(|dt| dt.with_timezone(&Utc));

        let content_hash = hash_override.unwrap_or_else(|| sha256_hex(text.trim()));

        let inserted = self.store.insert_or_dedup(NewTask {
            title: extracted.title,
            course: extracted.course,
            kind: extracted.kind,
            deadline_raw: extracted.deadline,
            deadline_utc,
            priority: extracted.priority,
            estimated_hours: extracted.estimated_hours,
            summary: extracted.summary,
            subtasks: extracted.subtasks,
            deliverables: extracted.deliverables,
            ics_file: None,
            google_calendar_url: None,
            source_file: source_label.map(|s| s.to_string()),
            content_hash: Some(content_hash),
            extractor: extracted.extractor,
        })?;

        let mut task = match inserted {
            InsertOutcome::Duplicate(existing) => {
                return Ok(IngestResult {
                    duplicate: true,
                    task: existing,
                    calendar: None,
                    archived_path: None,
                    notifications: Vec::new(),
                });
            }
            InsertOutcome::Inserted(t) => t,
        };

        let calendar = if task.deadline_raw.is_some() || task.deadline_utc.is_some() {
            let cal = create_ics_event(CalendarEventSpec {
                calendar_dir: &self.config.general.calendar_dir,
                title: &task.title,
                deadline_str: task.deadline_raw.as_deref(),
                tz,
                course: &task.course,
                description: &task.summary,
                duration_hours: self.config.general.default_duration_hours,
                priority: task.priority,
            })?;
            task = self.store.update_calendar_info(
                task.id,
                Some(cal.filename.clone()),
                Some(cal.google_calendar_url.clone()),
            )?;
            Some(cal)
        } else {
            None
        };

        let notifications = if dispatch {
            dispatch_new_task(
                &self.config.discord,
                &self.config.hermes,
                &self.config.n8n,
                &task,
                calendar.as_ref(),
            )
            .await
        } else {
            Vec::new()
        };

        Ok(IngestResult {
            duplicate: false,
            task,
            calendar,
            archived_path: None,
            notifications,
        })
    }

    /// Parses and ingests a local document file, archiving a copy when configured.
    pub async fn ingest_file(
        &self,
        path: &Path,
        force_heuristic: bool,
        dispatch: bool,
    ) -> Result<IngestResult> {
        let parsed = parse_document(path)?;
        let mut res = self
            .ingest_text(
                &parsed.content,
                &parsed.file_name,
                Some(&parsed.file_name),
                Some(parsed.content_hash),
                force_heuristic,
                dispatch,
            )
            .await?;

        if !res.duplicate && self.config.general.backup_processed {
            if let Ok(dest) = safe_join_within(&self.config.general.archive_dir, &parsed.file_name)
            {
                if std::fs::copy(path, &dest).is_ok() {
                    res.archived_path = Some(dest.display().to_string());
                }
            }
        }
        Ok(res)
    }

    /// Ingests an incoming change alert forwarded from a `webdelta` monitor.
    pub async fn ingest_webdelta_event(
        &self,
        payload: WebdeltaWebhookPayload,
        force_heuristic: bool,
        dispatch: bool,
    ) -> Result<IngestResult> {
        let watch_label = payload
            .watch_name
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(&payload.watch_id);
        let content_text = format!(
            "Title: {watch_label}\nCourse: {}\nSource URL: {}\n\n{}\n\n{}",
            payload.watch_id,
            payload.url.as_deref().unwrap_or("n/a"),
            payload.current_value.as_deref().unwrap_or(""),
            payload.diff.as_deref().unwrap_or(""),
        );
        let dedup_seed = format!(
            "webdelta:{}:{}",
            payload.watch_id,
            payload.current_value.as_deref().unwrap_or("")
        );
        let hash = sha256_hex(&dedup_seed);
        let source_name = format!("webdelta_{}.txt", payload.watch_id);

        self.ingest_text(
            &content_text,
            &source_name,
            Some(&format!("webdelta:{}", payload.watch_id)),
            Some(hash),
            force_heuristic,
            dispatch,
        )
        .await
    }

    /// Sends a manual or scheduled digest of currently open academic tasks.
    pub async fn send_digest(&self, now_utc: DateTime<Utc>, dry_run: bool) -> Result<DigestResult> {
        let tz = self.config.timezone_tz()?;
        let active: Vec<TaskRecord> = self
            .store
            .list_tasks(None)?
            .into_iter()
            .filter(|t| matches!(t.status, TaskStatus::Pending | TaskStatus::InProgress))
            .collect();

        if dry_run {
            return Ok(DigestResult {
                open_task_count: active.len(),
                dry_run: true,
                receipts: Vec::new(),
            });
        }

        let mut receipts = Vec::new();
        let embed = build_digest_embed(&active, tz, now_utc);
        if self.config.discord.enabled {
            match send_discord_embed(&self.config.discord, embed, None).await {
                Ok(r) => receipts.push(r),
                Err(e) => receipts.push(DispatchReceipt {
                    success: false,
                    transport: "discord".to_string(),
                    status_code: None,
                    detail: e.to_string(),
                }),
            }
        }
        if self.config.hermes.enabled {
            let msg = format!(
                "Academic Digest ({}): {} open task(s).",
                now_utc.with_timezone(&tz).format("%Y-%m-%d"),
                active.len()
            );
            match send_via_hermes(&self.config.hermes, &msg).await {
                Ok(r) => receipts.push(r),
                Err(e) => receipts.push(DispatchReceipt {
                    success: false,
                    transport: "hermes".to_string(),
                    status_code: None,
                    detail: e.to_string(),
                }),
            }
        }

        Ok(DigestResult {
            open_task_count: active.len(),
            dry_run: false,
            receipts,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_hmac_sha256_verification() {
        let secret = "super-secret";
        let body = br#"{"watch_id":"campus-portal","current_value":"Quiz 2"}"#;
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        let sig = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));

        assert!(verify_hmac_sha256(secret, body, &sig));
        assert!(!verify_hmac_sha256(secret, body, "sha256=deadbeef"));
        assert!(!verify_hmac_sha256("", body, &sig));
    }

    #[tokio::test]
    async fn test_pipeline_ingest_dedup_and_webdelta_event() {
        let dir = tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.resolve_paths_relative_to(dir.path());
        cfg.ensure_runtime_dirs().unwrap();
        cfg.discord.enabled = false;

        let store = TaskStore::open(&cfg.general.db_path).unwrap();
        let pipeline = AcademicPipeline::new(&cfg, &store);

        let file = dir.path().join("assignment.txt");
        std::fs::write(
            &file,
            "Course: Distributed Systems\nTitle: Raft Lab\nDeadline: 2026-12-10 23:59",
        )
        .unwrap();

        let first = pipeline.ingest_file(&file, true, false).await.unwrap();
        assert!(!first.duplicate);
        assert!(first.calendar.is_some());
        assert!(first.archived_path.is_some());

        let second = pipeline.ingest_file(&file, true, false).await.unwrap();
        assert!(second.duplicate);
        assert_eq!(second.task.id, first.task.id);

        let webdelta = pipeline
            .ingest_webdelta_event(
                WebdeltaWebhookPayload {
                    event: Some("watch.changed".to_string()),
                    watch_id: "portal-if6a".to_string(),
                    watch_name: Some("Campus Notice Board".to_string()),
                    url: Some("https://lms.example.edu/notices".to_string()),
                    current_value: Some(
                        "Tugas: Proposal Bab 1. Deadline: 2026-12-15 23:59".to_string(),
                    ),
                    diff: None,
                    checked_at: None,
                },
                true,
                false,
            )
            .await
            .unwrap();
        assert!(!webdelta.duplicate);
        assert_eq!(
            webdelta.task.deadline_raw.as_deref(),
            Some("2026-12-15 23:59")
        );
    }
}
