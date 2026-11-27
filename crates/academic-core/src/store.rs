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
const LMS_SYNC_META_TABLE: TableDefinition<&str, &str> = TableDefinition::new("lms_sync_meta");
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
    #[serde(default)]
    pub subtasks_done: Vec<usize>,
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

/// Partial update payload applied to an existing tracked task.
#[derive(Debug, Clone, Default)]
pub struct TaskPatch {
    pub title: Option<String>,
    pub course: Option<String>,
    pub kind: Option<TaskKind>,
    pub deadline_raw: Option<Option<String>>,
    pub deadline_utc: Option<Option<DateTime<Utc>>>,
    pub priority: Option<Priority>,
    pub status: Option<TaskStatus>,
    pub estimated_hours: Option<u32>,
    pub summary: Option<String>,
    pub subtasks: Option<Vec<String>>,
    pub subtasks_done: Option<Vec<usize>>,
    pub deliverables: Option<Vec<String>>,
}

/// Persistent sync state for an LMS source (`ETag`, `Last-Modified`, and event counts).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LmsSyncMeta {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub last_synced_at: Option<DateTime<Utc>>,
    pub last_event_count: usize,
}

/// Result of an atomic single-transaction LMS batch upsert.
#[derive(Debug, Clone)]
pub enum BatchItemOutcome {
    Inserted(TaskRecord),
    Updated(TaskRecord),
    Unchanged(TaskRecord),
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
            let _ = write_txn.open_table(LMS_SYNC_META_TABLE)?;
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
                    subtasks_done: Vec::new(),
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

    /// Partially updates a task record (`title`, `course`, `deadline`, `priority`, `status`,
    /// `subtasks`, `subtasks_done`, `deliverables`). Clears stale reminder keys when the deadline shifts.
    pub fn update_task(&self, id: u64, patch: TaskPatch) -> Result<TaskRecord> {
        let write_txn = self.db.begin_write()?;
        let updated = {
            let mut tasks = write_txn.open_table(TASKS_TABLE)?;
            let mut reminders = write_txn.open_table(SENT_REMINDERS_TABLE)?;
            let existing_json = tasks
                .get(id)?
                .map(|g| g.value().to_string())
                .ok_or_else(|| Error::NotFound(format!("task #{id} not found")))?;
            let mut rec: TaskRecord = serde_json::from_str(&existing_json)?;
            let prev_deadline = rec.deadline_utc;
            let prev_deadline_raw = rec.deadline_raw.clone();

            if let Some(title) = patch.title {
                let trimmed = title.trim().to_string();
                if !trimmed.is_empty() {
                    rec.title = trimmed;
                }
            }
            if let Some(course) = patch.course {
                let trimmed = course.trim().to_string();
                if !trimmed.is_empty() {
                    rec.course = trimmed;
                }
            }
            if let Some(kind) = patch.kind {
                rec.kind = kind;
            }
            if let Some(dl_raw) = patch.deadline_raw {
                rec.deadline_raw = dl_raw;
            }
            if let Some(dl_utc) = patch.deadline_utc {
                rec.deadline_utc = dl_utc;
            }
            if let Some(priority) = patch.priority {
                rec.priority = priority;
            }
            if let Some(status) = patch.status {
                rec.status = status;
            }
            if let Some(hours) = patch.estimated_hours {
                rec.estimated_hours = hours.max(1);
            }
            if let Some(summary) = patch.summary {
                rec.summary = summary.trim().to_string();
            }
            if let Some(subtasks) = patch.subtasks {
                rec.subtasks = subtasks;
                rec.subtasks_done = normalize_subtasks_done(&rec.subtasks_done, rec.subtasks.len());
            }
            if let Some(done_indices) = patch.subtasks_done {
                rec.subtasks_done = normalize_subtasks_done(&done_indices, rec.subtasks.len());
            }
            if let Some(deliverables) = patch.deliverables {
                rec.deliverables = deliverables;
            }

            if rec.deadline_utc != prev_deadline || rec.deadline_raw != prev_deadline_raw {
                clear_task_reminders(&mut reminders, id)?;
            }

            rec.updated_at = Utc::now();
            let serialized = serde_json::to_string(&rec)?;
            tasks.insert(id, serialized.as_str())?;
            rec
        };
        write_txn.commit()?;
        Ok(updated)
    }

    /// Atomically inserts or updates a batch of LMS items in a single `redb` write transaction.
    /// Existing active tasks whose deadline, title, course, or summary shifted are updated in place
    /// and their stale reminder keys are cleared.
    pub fn upsert_lms_batch(&self, items: Vec<NewTask>) -> Result<Vec<BatchItemOutcome>> {
        if items.is_empty() {
            return Ok(Vec::new());
        }

        let write_txn = self.db.begin_write()?;
        let mut mutated = false;
        let mut outcomes = Vec::with_capacity(items.len());

        {
            let mut tasks = write_txn.open_table(TASKS_TABLE)?;
            let mut hashes = write_txn.open_table(HASH_INDEX_TABLE)?;
            let mut reminders = write_txn.open_table(SENT_REMINDERS_TABLE)?;
            let mut meta = write_txn.open_table(META_TABLE)?;
            let mut next_id = meta.get(META_NEXT_TASK_ID)?.map(|g| g.value()).unwrap_or(1);
            let now = Utc::now();

            for item in items {
                let existing_id_opt = item
                    .content_hash
                    .as_deref()
                    .map(str::trim)
                    .filter(|h| !h.is_empty())
                    .and_then(|hash_key| {
                        hashes
                            .get(hash_key)
                            .ok()
                            .flatten()
                            .map(|guard| guard.value())
                    });

                let existing_opt: Option<TaskRecord> = if let Some(eid) = existing_id_opt {
                    tasks
                        .get(eid)?
                        .map(|g| serde_json::from_str(g.value()))
                        .transpose()?
                } else {
                    None
                };

                if let Some(mut existing) = existing_opt {
                    if matches!(
                        existing.status,
                        TaskStatus::Completed | TaskStatus::Archived
                    ) {
                        outcomes.push(BatchItemOutcome::Unchanged(existing));
                        continue;
                    }

                    let deadline_changed = existing.deadline_utc.map(|d| d.timestamp())
                        != item.deadline_utc.map(|d| d.timestamp())
                        || existing.deadline_raw != item.deadline_raw;
                    let text_changed = existing.title != item.title
                        || existing.course != item.course
                        || existing.summary != item.summary;

                    if !deadline_changed && !text_changed {
                        outcomes.push(BatchItemOutcome::Unchanged(existing));
                        continue;
                    }

                    existing.title = item.title;
                    existing.course = item.course;
                    existing.kind = item.kind;
                    existing.deadline_raw = item.deadline_raw;
                    existing.deadline_utc = item.deadline_utc;
                    existing.priority = item.priority;
                    existing.summary = item.summary;
                    if let Some(ics) = item.ics_file {
                        existing.ics_file = Some(ics);
                    }
                    if let Some(gcal) = item.google_calendar_url {
                        existing.google_calendar_url = Some(gcal);
                    }
                    existing.updated_at = now;

                    if deadline_changed {
                        clear_task_reminders(&mut reminders, existing.id)?;
                    }

                    let serialized = serde_json::to_string(&existing)?;
                    tasks.insert(existing.id, serialized.as_str())?;
                    mutated = true;
                    outcomes.push(BatchItemOutcome::Updated(existing));
                } else {
                    let assigned_id = next_id;
                    next_id += 1;

                    let record = TaskRecord {
                        id: assigned_id,
                        title: item.title,
                        course: item.course,
                        kind: item.kind,
                        deadline_raw: item.deadline_raw,
                        deadline_utc: item.deadline_utc,
                        priority: item.priority,
                        status: TaskStatus::Pending,
                        estimated_hours: item.estimated_hours.max(1),
                        summary: item.summary,
                        subtasks: item.subtasks,
                        subtasks_done: Vec::new(),
                        deliverables: item.deliverables,
                        ics_file: item.ics_file,
                        google_calendar_url: item.google_calendar_url,
                        source_file: item.source_file,
                        content_hash: item.content_hash.clone(),
                        extractor: item.extractor,
                        snoozed_until: None,
                        created_at: now,
                        updated_at: now,
                    };

                    let serialized = serde_json::to_string(&record)?;
                    tasks.insert(assigned_id, serialized.as_str())?;
                    if let Some(ref hash) = record.content_hash {
                        let hash_key = hash.trim();
                        if !hash_key.is_empty() {
                            hashes.insert(hash_key, assigned_id)?;
                        }
                    }
                    mutated = true;
                    outcomes.push(BatchItemOutcome::Inserted(record));
                }
            }

            if mutated {
                meta.insert(META_NEXT_TASK_ID, next_id)?;
            }
        }

        if mutated {
            write_txn.commit()?;
        } else {
            write_txn.abort()?;
        }
        Ok(outcomes)
    }

    /// Reads persistent LMS synchronization metadata (`ETag`, `Last-Modified`, timestamp) for a source key.
    pub fn get_lms_sync_meta(&self, key: &str) -> Result<Option<LmsSyncMeta>> {
        let read_txn = self.db.begin_read()?;
        let table = read_txn.open_table(LMS_SYNC_META_TABLE)?;
        let Some(guard) = table.get(key)? else {
            return Ok(None);
        };
        let meta: LmsSyncMeta = serde_json::from_str(guard.value())?;
        Ok(Some(meta))
    }

    /// Persists LMS synchronization metadata for `key`.
    pub fn set_lms_sync_meta(&self, key: &str, meta: &LmsSyncMeta) -> Result<()> {
        let write_txn = self.db.begin_write()?;
        {
            let mut table = write_txn.open_table(LMS_SYNC_META_TABLE)?;
            let serialized = serde_json::to_string(meta)?;
            table.insert(key, serialized.as_str())?;
        }
        write_txn.commit()?;
        Ok(())
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

fn normalize_subtasks_done(indices: &[usize], subtask_count: usize) -> Vec<usize> {
    let mut seen = std::collections::BTreeSet::new();
    for &idx in indices {
        if idx < subtask_count {
            seen.insert(idx);
        }
    }
    seen.into_iter().collect()
}

fn clear_task_reminders(
    table: &mut redb::Table<'_, &'static str, &'static str>,
    task_id: u64,
) -> Result<()> {
    let prefix = format!("task:{task_id}:");
    let mut to_remove = Vec::new();
    for item in table.iter()? {
        let (k, _) = item?;
        let key = k.value();
        if key.starts_with(&prefix) {
            to_remove.push(key.to_string());
        }
    }
    for key in to_remove {
        let _ = table.remove(key.as_str())?;
    }
    Ok(())
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

    #[test]
    fn test_update_task_subtasks_and_batch_lms_upsert() {
        let dir = tempdir().unwrap();
        let store = TaskStore::open(&dir.path().join("state.redb")).unwrap();
        let fixed_dl = Utc::now() + Duration::hours(36);

        let mut t1 = sample_new_task(Some("lms:moodle:evt-1"));
        t1.deadline_utc = Some(fixed_dl);
        let outcomes1 = store.upsert_lms_batch(vec![t1]).unwrap();
        let inserted_id = match &outcomes1[0] {
            BatchItemOutcome::Inserted(t) => t.id,
            other => panic!("expected Inserted, got {other:?}"),
        };

        // Mark a reminder key as sent, toggle subtasks, then shift deadline
        store
            .mark_reminder_sent(&format!("task:{inserted_id}:h24"), Utc::now())
            .unwrap();
        assert!(store
            .has_sent_reminder(&format!("task:{inserted_id}:h24"))
            .unwrap());

        let updated = store
            .update_task(
                inserted_id,
                TaskPatch {
                    subtasks_done: Some(vec![1, 0, 99]),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(updated.subtasks_done, vec![0, 1]);

        // Unchanged re-sync in batch
        let mut same = sample_new_task(Some("lms:moodle:evt-1"));
        same.deadline_utc = Some(fixed_dl);
        let outcomes2 = store.upsert_lms_batch(vec![same]).unwrap();
        assert!(matches!(outcomes2[0], BatchItemOutcome::Unchanged(_)));

        // Shifted deadline in batch updates task and clears stale reminder key
        let mut shifted = sample_new_task(Some("lms:moodle:evt-1"));
        shifted.deadline_raw = Some("2026-12-03 23:59".to_string());
        shifted.deadline_utc = Some(fixed_dl + Duration::hours(48));
        let outcomes3 = store.upsert_lms_batch(vec![shifted]).unwrap();
        assert!(matches!(outcomes3[0], BatchItemOutcome::Updated(_)));
        assert!(!store
            .has_sent_reminder(&format!("task:{inserted_id}:h24"))
            .unwrap());

        // LmsSyncMeta round-trip
        let meta = LmsSyncMeta {
            etag: Some("\"etag-42\"".to_string()),
            last_modified: None,
            last_synced_at: Some(Utc::now()),
            last_event_count: 1,
        };
        store.set_lms_sync_meta("moodle", &meta).unwrap();
        let loaded = store.get_lms_sync_meta("moodle").unwrap().unwrap();
        assert_eq!(loaded.etag.as_deref(), Some("\"etag-42\""));
        assert_eq!(loaded.last_event_count, 1);
    }
}
