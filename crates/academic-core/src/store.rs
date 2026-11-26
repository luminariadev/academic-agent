use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use redb::{Database, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::extract::{Priority, TaskKind};

const TASKS_TABLE: TableDefinition<u64, &str> = TableDefinition::new("tasks");
const HASH_INDEX_TABLE: TableDefinition<&str, u64> = TableDefinition::new("content_hashes");
const SENT_REMINDERS_TABLE: TableDefinition<&str, &str> = TableDefinition::new("sent_reminders");
const META_TABLE: TableDefinition<&str, u64> = TableDefinition::new("meta");

const META_NEXT_TASK_ID: &str = "next_task_id";

/// Lifecycle status of a tracked academic task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TaskStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
    Archived,
}

impl TaskStatus {
    pub fn parse_loose(raw: &str) -> Result<Self> {
        match raw.trim().to_uppercase().as_str() {
            "PENDING" | "TODO" | "OPEN" => Ok(Self::Pending),
            "IN_PROGRESS" | "INPROGRESS" | "DOING" => Ok(Self::InProgress),
            "COMPLETED" | "DONE" | "FINISHED" => Ok(Self::Completed),
            "ARCHIVED" | "ARCHIVE" => Ok(Self::Archived),
            other => Err(Error::Config(format!(
                "invalid task status '{other}'; expected PENDING, IN_PROGRESS, COMPLETED, or ARCHIVED"
            ))),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::InProgress => "IN_PROGRESS",
            Self::Completed => "COMPLETED",
            Self::Archived => "ARCHIVED",
        }
    }
}

/// Persistent academic task record stored in `redb`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRecord {
    pub id: u64,
    pub title: String,
    pub course: String,
    pub kind: TaskKind,
    pub deadline_raw: Option<String>,
    pub deadline_utc: Option<DateTime<Utc>>,
    pub priority: Priority,
    pub status: TaskStatus,
    pub estimated_hours: u32,
    pub summary: String,
    pub subtasks: Vec<String>,
    pub deliverables: Vec<String>,
    pub ics_file: Option<String>,
    pub google_calendar_url: Option<String>,
    pub source_file: Option<String>,
    pub content_hash: Option<String>,
    pub extractor: String,
    pub snoozed_until: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Input payload used when creating a new task record.
#[derive(Debug, Clone)]
pub struct NewTask {
    pub title: String,
    pub course: String,
    pub kind: TaskKind,
    pub deadline_raw: Option<String>,
    pub deadline_utc: Option<DateTime<Utc>>,
    pub priority: Priority,
    pub estimated_hours: u32,
    pub summary: String,
    pub subtasks: Vec<String>,
    pub deliverables: Vec<String>,
    pub ics_file: Option<String>,
    pub google_calendar_url: Option<String>,
    pub source_file: Option<String>,
    pub content_hash: Option<String>,
    pub extractor: String,
}

/// Result of attempting to insert a task with content-hash deduplication.
#[derive(Debug, Clone)]
pub enum InsertOutcome {
    Inserted(TaskRecord),
    Duplicate(TaskRecord),
}

/// Supported export formats for tracked tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Json,
    Csv,
}

impl ExportFormat {
    pub fn parse_loose(raw: &str) -> Result<Self> {
        match raw.trim().to_lowercase().as_str() {
            "json" => Ok(Self::Json),
            "csv" => Ok(Self::Csv),
            other => Err(Error::UnsupportedFormat(format!(
                "unsupported export format '{other}'; expected json or csv"
            ))),
        }
    }
}

/// Embedded ACID transactional task store backed by `redb`.
#[derive(Debug, Clone)]
pub struct TaskStore {
    db: Arc<Database>,
}

impl TaskStore {
    /// Opens or creates the `redb` database file and ensures all required tables exist.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let db = Database::create(path)?;
        let write_txn = db.begin_write()?;
        {
            let _ = write_txn.open_table(TASKS_TABLE)?;
            let _ = write_txn.open_table(HASH_INDEX_TABLE)?;
            let _ = write_txn.open_table(SENT_REMINDERS_TABLE)?;
            let mut meta = write_txn.open_table(META_TABLE)?;
            if meta.get(META_NEXT_TASK_ID)?.is_none() {
                meta.insert(META_NEXT_TASK_ID, 1u64)?;
            }
        }
        write_txn.commit()?;
        Ok(Self { db: Arc::new(db) })
    }

    /// Inserts a task unless an active entry already exists for `input.content_hash`.
    pub fn insert_or_dedup(&self, input: NewTask) -> Result<InsertOutcome> {
        let write_txn = self.db.begin_write()?;
        let outcome = {
            let mut tasks = write_txn.open_table(TASKS_TABLE)?;
            let mut hashes = write_txn.open_table(HASH_INDEX_TABLE)?;
            let mut meta = write_txn.open_table(META_TABLE)?;

            let mut existing_record: Option<TaskRecord> = None;
            if let Some(ref hash) = input.content_hash {
                let hash_key = hash.trim();
                if !hash_key.is_empty() {
                    let existing_id_opt = hashes.get(hash_key)?.map(|g| g.value());
                    if let Some(existing_id) = existing_id_opt {
                        let existing_json_opt =
                            tasks.get(existing_id)?.map(|g| g.value().to_string());
                        if let Some(existing_json) = existing_json_opt {
                            existing_record = Some(serde_json::from_str(&existing_json)?);
                        }
                    }
                }
            }

            if let Some(existing) = existing_record {
                InsertOutcome::Duplicate(existing)
            } else {
                let next_id = meta.get(META_NEXT_TASK_ID)?.map(|g| g.value()).unwrap_or(1);
                meta.insert(META_NEXT_TASK_ID, next_id + 1)?;

                let now = Utc::now();
                let record = TaskRecord {
                    id: next_id,
                    title: input.title,
                    course: input.course,
                    kind: input.kind,
                    deadline_raw: input.deadline_raw,
                    deadline_utc: input.deadline_utc,
                    priority: input.priority,
                    status: TaskStatus::Pending,
                    estimated_hours: input.estimated_hours.max(1),
                    summary: input.summary,
                    subtasks: input.subtasks,
                    deliverables: input.deliverables,
                    ics_file: input.ics_file,
                    google_calendar_url: input.google_calendar_url,
                    source_file: input.source_file,
                    content_hash: input.content_hash.clone(),
                    extractor: input.extractor,
                    snoozed_until: None,
                    created_at: now,
                    updated_at: now,
                };

                let json = serde_json::to_string(&record)?;
                tasks.insert(next_id, json.as_str())?;
                if let Some(ref hash) = record.content_hash {
                    let hash_key = hash.trim();
                    if !hash_key.is_empty() {
                        hashes.insert(hash_key, next_id)?;
                    }
                }

                InsertOutcome::Inserted(record)
            }
        };
        if matches!(outcome, InsertOutcome::Inserted(_)) {
            write_txn.commit()?;
        } else {
            write_txn.abort()?;
        }
        Ok(outcome)
    }

    /// Retrieves a single task by ID.
    pub fn get_task(&self, id: u64) -> Result<Option<TaskRecord>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(TASKS_TABLE)?;
        let Some(guard) = table.get(id)? else {
            return Ok(None);
        };
        let record: TaskRecord = serde_json::from_str(guard.value())?;
        Ok(Some(record))
    }

    /// Lists all tasks ordered by soonest deadline first (`None` last), then by ID.
    pub fn list_tasks(&self, status_filter: Option<TaskStatus>) -> Result<Vec<TaskRecord>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(TASKS_TABLE)?;
        let mut records = Vec::new();
        for item in table.iter()? {
            let (_, val) = item?;
            let rec: TaskRecord = serde_json::from_str(val.value())?;
            if let Some(expected) = status_filter {
                if rec.status != expected {
                    continue;
                }
            }
            records.push(rec);
        }
        records.sort_by(|a, b| match (&a.deadline_utc, &b.deadline_utc) {
            (Some(da), Some(db)) => da.cmp(db).then_with(|| a.id.cmp(&b.id)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.id.cmp(&b.id),
        });
        Ok(records)
    }

    /// Returns incomplete (`PENDING` / `IN_PROGRESS`) tasks with deadlines at or before
    /// `now_utc + within_hours` (including overdue tasks).
    pub fn upcoming_tasks(
        &self,
        within_hours: i64,
        now_utc: DateTime<Utc>,
    ) -> Result<Vec<TaskRecord>> {
        let cutoff = now_utc + Duration::hours(within_hours.max(1));
        let all = self.list_tasks(None)?;
        Ok(all
            .into_iter()
            .filter(|t| matches!(t.status, TaskStatus::Pending | TaskStatus::InProgress))
            .filter(|t| matches!(t.deadline_utc, Some(dl) if dl <= cutoff))
            .collect())
    }

    /// Updates a task's status (`PENDING`, `IN_PROGRESS`, `COMPLETED`, `ARCHIVED`).
    pub fn update_status(&self, id: u64, status: TaskStatus) -> Result<TaskRecord> {
        let write_txn = self.db.begin_write()?;
        let updated = {
            let mut table = write_txn.open_table(TASKS_TABLE)?;
            let existing_json = table
                .get(id)?
                .map(|g| g.value().to_string())
                .ok_or_else(|| Error::NotFound(format!("task #{id} not found")))?;
            let mut rec: TaskRecord = serde_json::from_str(&existing_json)?;
            rec.status = status;
            rec.updated_at = Utc::now();
            let serialized = serde_json::to_string(&rec)?;
            table.insert(id, serialized.as_str())?;
            rec
        };
        write_txn.commit()?;
        Ok(updated)
    }

    /// Snoozes cron deadline alerts for a task until `until`.
    pub fn snooze_task(&self, id: u64, until: DateTime<Utc>) -> Result<TaskRecord> {
        let write_txn = self.db.begin_write()?;
        let updated = {
            let mut table = write_txn.open_table(TASKS_TABLE)?;
            let existing_json = table
                .get(id)?
                .map(|g| g.value().to_string())
                .ok_or_else(|| Error::NotFound(format!("task #{id} not found")))?;
            let mut rec: TaskRecord = serde_json::from_str(&existing_json)?;
            rec.snoozed_until = Some(until);
            rec.updated_at = Utc::now();
            let serialized = serde_json::to_string(&rec)?;
            table.insert(id, serialized.as_str())?;
            rec
        };
        write_txn.commit()?;
        Ok(updated)
    }

    /// Updates calendar metadata attached to an existing task.
    pub fn update_calendar_info(
        &self,
        id: u64,
        ics_file: Option<String>,
        google_calendar_url: Option<String>,
    ) -> Result<TaskRecord> {
        let write_txn = self.db.begin_write()?;
        let updated = {
            let mut table = write_txn.open_table(TASKS_TABLE)?;
            let existing_json = table
                .get(id)?
                .map(|g| g.value().to_string())
                .ok_or_else(|| Error::NotFound(format!("task #{id} not found")))?;
            let mut rec: TaskRecord = serde_json::from_str(&existing_json)?;
            rec.ics_file = ics_file;
            rec.google_calendar_url = google_calendar_url;
            rec.updated_at = Utc::now();
            let serialized = serde_json::to_string(&rec)?;
            table.insert(id, serialized.as_str())?;
            rec
        };
        write_txn.commit()?;
        Ok(updated)
    }

    /// Deletes a task by ID and removes any associated content-hash deduplication entry.
    pub fn delete_task(&self, id: u64) -> Result<bool> {
        let write_txn = self.db.begin_write()?;
        let removed = {
            let mut tasks = write_txn.open_table(TASKS_TABLE)?;
            let mut hashes = write_txn.open_table(HASH_INDEX_TABLE)?;
            let removed_json = tasks.remove(id)?.map(|g| g.value().to_string());
            if let Some(json) = removed_json {
                let rec: TaskRecord = serde_json::from_str(&json)?;
                if let Some(ref hash) = rec.content_hash {
                    let key = hash.trim();
                    if !key.is_empty() {
                        let _ = hashes.remove(key)?;
                    }
                }
                true
            } else {
                false
            }
        };
        write_txn.commit()?;
        Ok(removed)
    }

    /// Checks whether a reminder key (such as `task:1:h24` or `digest:2026-11-27`) was already recorded.
    pub fn has_sent_reminder(&self, key: &str) -> Result<bool> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(SENT_REMINDERS_TABLE)?;
        Ok(table.get(key)?.is_some())
    }

    /// Records that a reminder key was dispatched at `sent_at`.
    pub fn mark_reminder_sent(&self, key: &str, sent_at: DateTime<Utc>) -> Result<()> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(SENT_REMINDERS_TABLE)?;
            let val = sent_at.to_rfc3339();
            table.insert(key, val.as_str())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    /// Exports tasks as pretty JSON or RFC 4180 CSV.
    pub fn export_tasks(
        &self,
        format: ExportFormat,
        status_filter: Option<TaskStatus>,
    ) -> Result<String> {
        let tasks = self.list_tasks(status_filter)?;
        match format {
            ExportFormat::Json => Ok(serde_json::to_string_pretty(&tasks)?),
            ExportFormat::Csv => {
                let mut out = String::from(
                    "id,title,course,kind,priority,status,deadline_raw,deadline_utc,estimated_hours,ics_file,source_file\n",
                );
                for t in tasks {
                    let row = [
                        t.id.to_string(),
                        csv_escape(&t.title),
                        csv_escape(&t.course),
                        csv_escape(&format!("{:?}", t.kind).to_lowercase()),
                        csv_escape(t.priority.as_str()),
                        csv_escape(t.status.as_str()),
                        csv_escape(t.deadline_raw.as_deref().unwrap_or("")),
                        csv_escape(&t.deadline_utc.map(|d| d.to_rfc3339()).unwrap_or_default()),
                        t.estimated_hours.to_string(),
                        csv_escape(t.ics_file.as_deref().unwrap_or("")),
                        csv_escape(t.source_file.as_deref().unwrap_or("")),
                    ];
                    out.push_str(&row.join(","));
                    out.push('\n');
                }
                Ok(out)
            }
        }
    }
}

fn csv_escape(field: &str) -> String {
    if field.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn sample_new_task(hash: Option<&str>) -> NewTask {
        NewTask {
            title: "Raft Paper Summary".to_string(),
            course: "Distributed Systems".to_string(),
            kind: TaskKind::Assignment,
            deadline_raw: Some("2026-12-01 23:59".to_string()),
            deadline_utc: Some(Utc::now() + Duration::hours(36)),
            priority: Priority::High,
            estimated_hours: 3,
            summary: "Summarize sections 1 to 5.".to_string(),
            subtasks: vec!["Read paper".to_string(), "Write notes".to_string()],
            deliverables: vec!["PDF".to_string()],
            ics_file: Some("raft.ics".to_string()),
            google_calendar_url: None,
            source_file: Some("raft.txt".to_string()),
            content_hash: hash.map(|s| s.to_string()),
            extractor: "heuristic".to_string(),
        }
    }

    #[test]
    fn test_store_dedup_snooze_and_export() {
        let dir = tempdir().unwrap();
        let store = TaskStore::open(&dir.path().join("state.redb")).unwrap();

        let first = store
            .insert_or_dedup(sample_new_task(Some("hash-abc")))
            .unwrap();
        let id = match first {
            InsertOutcome::Inserted(t) => t.id,
            InsertOutcome::Duplicate(_) => panic!("first insert should not be duplicate"),
        };

        let second = store
            .insert_or_dedup(sample_new_task(Some("hash-abc")))
            .unwrap();
        match second {
            InsertOutcome::Duplicate(t) => assert_eq!(t.id, id),
            InsertOutcome::Inserted(_) => panic!("same hash should deduplicate"),
        }

        let upcoming = store.upcoming_tasks(48, Utc::now()).unwrap();
        assert_eq!(upcoming.len(), 1);

        let updated = store.update_status(id, TaskStatus::Completed).unwrap();
        assert_eq!(updated.status, TaskStatus::Completed);
        assert!(store.upcoming_tasks(48, Utc::now()).unwrap().is_empty());

        let csv = store.export_tasks(ExportFormat::Csv, None).unwrap();
        assert!(csv.contains("Raft Paper Summary"));
        assert!(csv.contains("COMPLETED"));

        assert!(store.delete_task(id).unwrap());
        // Deleting also clears hash index so re-inserting succeeds cleanly
        let third = store
            .insert_or_dedup(sample_new_task(Some("hash-abc")))
            .unwrap();
        assert!(matches!(third, InsertOutcome::Inserted(_)));
    }
}
