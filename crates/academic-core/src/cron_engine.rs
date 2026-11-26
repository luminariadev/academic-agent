use chrono::{DateTime, Duration, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::config::{parse_cron_schedule, Config};
use crate::error::Result;
use crate::notify::{
    build_digest_embed, build_reminder_embed, build_task_action_components, send_discord_embed,
    send_discord_message_with_components, send_via_hermes,
};
use crate::store::{TaskRecord, TaskStatus, TaskStore};

/// Tier classification for scheduled deadline reminders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReminderKind {
    UpcomingHours(i64),
    Overdue,
}

impl ReminderKind {
    pub fn label(&self) -> String {
        match self {
            Self::UpcomingHours(h) => format!("H-{h}h"),
            Self::Overdue => "OVERDUE".to_string(),
        }
    }
}

/// Candidate deadline alert produced during cron evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DueReminder {
    pub task: TaskRecord,
    pub kind: ReminderKind,
    pub key: String,
    pub bypassed_keys: Vec<String>,
    pub hours_remaining: f64,
}

/// Compact summary returned inside `CronTickReport`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DueReminderSummary {
    pub task_id: u64,
    pub title: String,
    pub kind: ReminderKind,
    pub key: String,
    pub hours_remaining: f64,
}

/// Report emitted by `run_cron_tick`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronTickReport {
    pub checked_at: DateTime<Utc>,
    pub digest_due: bool,
    pub digest_sent: bool,
    pub due_reminders: Vec<DueReminderSummary>,
    pub reminders_sent: usize,
    pub dry_run: bool,
    pub errors: Vec<String>,
}

/// Evaluates all incomplete tasks and returns at most one unsent reminder per task
/// (the most urgent tier reached), tracking bypassed wider-window keys so higher-hour alerts
/// are never fired retroactively on subsequent ticks.
pub fn evaluate_due_reminders(
    store: &TaskStore,
    config: &Config,
    now_utc: DateTime<Utc>,
) -> Result<Vec<DueReminder>> {
    if !config.cron.enabled {
        return Ok(Vec::new());
    }

    let mut tiers: Vec<i64> = config
        .cron
        .reminder_hours
        .iter()
        .copied()
        .filter(|&h| h > 0)
        .collect();
    tiers.sort_unstable_by(|a, b| b.cmp(a));
    tiers.dedup();

    let tasks = store.list_tasks(None)?;
    let mut due = Vec::new();

    for task in tasks {
        if !matches!(task.status, TaskStatus::Pending | TaskStatus::InProgress) {
            continue;
        }
        if let Some(snooze_until) = task.snoozed_until {
            if snooze_until > now_utc {
                continue;
            }
        }
        let Some(deadline_utc) = task.deadline_utc else {
            continue;
        };

        let delta = deadline_utc - now_utc;
        let hours_remaining = delta.num_seconds() as f64 / 3600.0;

        if delta <= Duration::zero() {
            if config.cron.alert_overdue {
                let key = format!("task:{}:overdue", task.id);
                if !store.has_sent_reminder(&key)? {
                    let bypassed = tiers
                        .iter()
                        .map(|h| format!("task:{}:h{h}", task.id))
                        .collect();
                    due.push(DueReminder {
                        task,
                        kind: ReminderKind::Overdue,
                        key,
                        bypassed_keys: bypassed,
                        hours_remaining,
                    });
                }
            }
            continue;
        }

        // Select the tightest matching reminder tier (`hours_remaining <= tier`)
        let matching: Vec<i64> = tiers
            .iter()
            .copied()
            .filter(|&h| hours_remaining <= h as f64)
            .collect();
        if let Some(&most_urgent) = matching.last() {
            let key = format!("task:{}:h{most_urgent}", task.id);
            if !store.has_sent_reminder(&key)? {
                let bypassed = matching
                    .iter()
                    .filter(|&&h| h != most_urgent)
                    .map(|h| format!("task:{}:h{h}", task.id))
                    .collect();
                due.push(DueReminder {
                    task,
                    kind: ReminderKind::UpcomingHours(most_urgent),
                    key,
                    bypassed_keys: bypassed,
                    hours_remaining,
                });
            }
        }
    }

    Ok(due)
}

/// Checks whether today's configured daily digest cron window has been reached in local timezone
/// and today's digest key (`digest:YYYY-MM-DD`) has not yet been dispatched.
pub fn is_digest_due(
    store: &TaskStore,
    config: &Config,
    now_utc: DateTime<Utc>,
) -> Result<Option<String>> {
    if !config.cron.enabled {
        return Ok(None);
    }
    let tz = config.timezone_tz()?;
    let now_local = now_utc.with_timezone(&tz);
    let schedule = parse_cron_schedule(&config.cron.digest_cron)?;

    let today_start_naive = now_local
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .expect("midnight valid");
    let today_start_tz = tz
        .from_local_datetime(&today_start_naive)
        .earliest()
        .unwrap_or(now_local)
        - Duration::seconds(1);

    let fired_today = schedule
        .after(&today_start_tz)
        .take_while(|dt| *dt <= now_local)
        .next()
        .is_some();

    if !fired_today {
        return Ok(None);
    }

    let key = format!("digest:{}", now_local.format("%Y-%m-%d"));
    if store.has_sent_reminder(&key)? {
        Ok(None)
    } else {
        Ok(Some(key))
    }
}

/// Evaluates and optionally dispatches due deadline reminders and daily digest once.
pub async fn run_cron_tick(
    store: &TaskStore,
    config: &Config,
    now_utc: DateTime<Utc>,
    dry_run: bool,
) -> Result<CronTickReport> {
    let tz = config.timezone_tz()?;
    let due_reminders = evaluate_due_reminders(store, config, now_utc)?;
    let digest_key = is_digest_due(store, config, now_utc)?;

    let mut report = CronTickReport {
        checked_at: now_utc,
        digest_due: digest_key.is_some(),
        digest_sent: false,
        due_reminders: due_reminders
            .iter()
            .map(|r| DueReminderSummary {
                task_id: r.task.id,
                title: r.task.title.clone(),
                kind: r.kind,
                key: r.key.clone(),
                hours_remaining: (r.hours_remaining * 100.0).round() / 100.0,
            })
            .collect(),
        reminders_sent: 0,
        dry_run,
        errors: Vec::new(),
    };

    if dry_run {
        return Ok(report);
    }

    // 1. Dispatch daily digest if due
    if let Some(dkey) = digest_key {
        let mut active: Vec<TaskRecord> = store
            .list_tasks(None)?
            .into_iter()
            .filter(|t| matches!(t.status, TaskStatus::Pending | TaskStatus::InProgress))
            .collect();
        active.truncate(20);
        let embed = build_digest_embed(&active, tz, now_utc);
        let mut sent_ok = false;

        if config.discord.enabled {
            match send_discord_embed(&config.discord, embed, None).await {
                Ok(_) => sent_ok = true,
                Err(e) => report.errors.push(format!("digest discord error: {e}")),
            }
        }
        if config.hermes.enabled && (!sent_ok || !config.hermes.fallback_on_discord_error) {
            let msg = format!(
                "Academic Digest ({}): {} open task(s).",
                now_utc.with_timezone(&tz).format("%Y-%m-%d"),
                active.len()
            );
            match send_via_hermes(&config.hermes, &msg).await {
                Ok(_) => sent_ok = true,
                Err(e) => report.errors.push(format!("digest hermes error: {e}")),
            }
        }

        if sent_ok {
            store.mark_reminder_sent(&dkey, now_utc)?;
            report.digest_sent = true;
        }
    }

    // 2. Dispatch individual deadline alerts
    for item in due_reminders {
        let label = item.kind.label();
        let embed = build_reminder_embed(&item.task, &label, tz);
        let mut sent_ok = false;

        if config.discord.enabled {
            let components = build_task_action_components(
                item.task.id,
                item.task.google_calendar_url.as_deref(),
            );
            match send_discord_message_with_components(&config.discord, embed, Some(components))
                .await
            {
                Ok(_) => sent_ok = true,
                Err(e) => {
                    warn!("Failed dispatching reminder {}: {e}", item.key);
                    report
                        .errors
                        .push(format!("{} discord error: {e}", item.key));
                }
            }
        }

        if config.hermes.enabled && (!sent_ok || !config.hermes.fallback_on_discord_error) {
            let msg = format!(
                "[{label}] [{}] {} (due {})",
                item.task.course,
                item.task.title,
                item.task.deadline_raw.as_deref().unwrap_or("unspecified")
            );
            match send_via_hermes(&config.hermes, &msg).await {
                Ok(_) => sent_ok = true,
                Err(e) => report
                    .errors
                    .push(format!("{} hermes error: {e}", item.key)),
            }
        }

        if sent_ok {
            store.mark_reminder_sent(&item.key, now_utc)?;
            for bypassed in item.bypassed_keys {
                store.mark_reminder_sent(&bypassed, now_utc)?;
            }
            report.reminders_sent += 1;
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::{Priority, TaskKind};
    use crate::store::{InsertOutcome, NewTask};
    use tempfile::tempdir;

    fn task_due_in(hours: i64, hash: &str) -> NewTask {
        NewTask {
            title: format!("Task due in {hours}h"),
            course: "Algorithms".to_string(),
            kind: TaskKind::Assignment,
            deadline_raw: None,
            deadline_utc: Some(Utc::now() + Duration::hours(hours)),
            priority: Priority::High,
            estimated_hours: 2,
            summary: "Complete exercises".to_string(),
            subtasks: vec![],
            deliverables: vec![],
            ics_file: None,
            google_calendar_url: None,
            source_file: None,
            content_hash: Some(hash.to_string()),
            extractor: "heuristic".to_string(),
        }
    }

    #[test]
    fn test_cron_reminder_tiers_and_snooze_antispam() {
        let dir = tempdir().unwrap();
        let store = TaskStore::open(&dir.path().join("state.redb")).unwrap();
        let mut cfg = Config::default();
        cfg.cron.digest_cron = "0 7 * * *".to_string();

        let id_urgent = match store.insert_or_dedup(task_due_in(2, "h-2")).unwrap() {
            InsertOutcome::Inserted(t) => t.id,
            _ => unreachable!(),
        };
        let id_snoozed = match store.insert_or_dedup(task_due_in(10, "h-10")).unwrap() {
            InsertOutcome::Inserted(t) => t.id,
            _ => unreachable!(),
        };
        store
            .snooze_task(id_snoozed, Utc::now() + Duration::hours(24))
            .unwrap();

        let now = Utc::now();
        let due = evaluate_due_reminders(&store, &cfg, now).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].task.id, id_urgent);
        assert_eq!(due[0].kind, ReminderKind::UpcomingHours(3));
        assert!(due[0]
            .bypassed_keys
            .contains(&format!("task:{id_urgent}:h24")));
        assert!(due[0]
            .bypassed_keys
            .contains(&format!("task:{id_urgent}:h72")));

        // Mark sent + bypassed; re-evaluating immediately yields empty list (no spam)
        store.mark_reminder_sent(&due[0].key, now).unwrap();
        for b in &due[0].bypassed_keys {
            store.mark_reminder_sent(b, now).unwrap();
        }
        assert!(evaluate_due_reminders(&store, &cfg, now)
            .unwrap()
            .is_empty());
    }
}
