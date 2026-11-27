use std::time::Duration;

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use reqwest::{header, Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::calendar::{create_ics_event, safe_slug, CalendarEventSpec};
use crate::config::{Config, LmsSourceConfig, LmsSourceKind};
use crate::error::{Error, Result};
use crate::extract::{Priority, TaskKind};
use crate::notify::dispatch_new_task;
use crate::parser::sha256_hex;
use crate::store::{BatchItemOutcome, LmsSyncMeta, NewTask, TaskRecord, TaskStore};

/// Normalized event extracted from an `.ics` feed, Moodle API, or Canvas API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LmsIncomingEvent {
    pub uid: String,
    pub title: String,
    pub course: String,
    pub kind: TaskKind,
    pub deadline_raw: Option<String>,
    pub deadline_utc: Option<DateTime<Utc>>,
    pub priority: Priority,
    pub summary: String,
    pub source_url: Option<String>,
}

/// Summary report produced after synchronizing a single campus LMS source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LmsSyncReport {
    pub source_name: String,
    pub kind: String,
    pub not_modified: bool,
    pub fetched_events: usize,
    pub inserted: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub tasks_changed: Vec<TaskRecord>,
    pub notifications_sent: usize,
}

fn unfold_ics_lines(raw: &str) -> Vec<String> {
    let normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines: Vec<String> = Vec::new();
    for line in normalized.lines() {
        if let Some(rest) = line.strip_prefix(' ').or_else(|| line.strip_prefix('\t')) {
            if let Some(last) = lines.last_mut() {
                last.push_str(rest);
                continue;
            }
        }
        lines.push(line.to_string());
    }
    lines
}

fn unescape_ics_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.peek().copied() {
                Some('n') | Some('N') => {
                    chars.next();
                    out.push('\n');
                }
                Some(',') => {
                    chars.next();
                    out.push(',');
                }
                Some(';') => {
                    chars.next();
                    out.push(';');
                }
                Some('\\') => {
                    chars.next();
                    out.push('\\');
                }
                _ => out.push('\\'),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn extract_tzid(params: &str) -> Option<&str> {
    for part in params.split(';') {
        if let Some((k, v)) = part.split_once('=') {
            if k.trim().eq_ignore_ascii_case("TZID") {
                return Some(v.trim().trim_matches('"'));
            }
        }
    }
    None
}

fn parse_ics_datetime(
    params: &str,
    value: &str,
    default_tz: Tz,
) -> Option<(DateTime<Utc>, String)> {
    let cleaned = value.trim();
    if cleaned.is_empty() {
        return None;
    }

    if let Some(stripped) = cleaned
        .strip_suffix('Z')
        .or_else(|| cleaned.strip_suffix('z'))
    {
        let naive = NaiveDateTime::parse_from_str(stripped, "%Y%m%dT%H%M%S")
            .or_else(|_| NaiveDateTime::parse_from_str(stripped, "%Y%m%dT%H%M"))
            .ok()?;
        let utc_dt = DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc);
        let local_fmt = utc_dt
            .with_timezone(&default_tz)
            .format("%Y-%m-%d %H:%M")
            .to_string();
        return Some((utc_dt, local_fmt));
    }

    let source_tz = extract_tzid(params)
        .and_then(|id| id.parse::<Tz>().ok())
        .unwrap_or(default_tz);

    if cleaned.contains('T') {
        let naive = NaiveDateTime::parse_from_str(cleaned, "%Y%m%dT%H%M%S")
            .or_else(|_| NaiveDateTime::parse_from_str(cleaned, "%Y%m%dT%H%M"))
            .ok()?;
        let dt = source_tz.from_local_datetime(&naive).earliest()?;
        let utc_dt = dt.with_timezone(&Utc);
        let local_fmt = utc_dt
            .with_timezone(&default_tz)
            .format("%Y-%m-%d %H:%M")
            .to_string();
        return Some((utc_dt, local_fmt));
    }

    if cleaned.len() == 8 && cleaned.bytes().all(|b| b.is_ascii_digit()) {
        let date = NaiveDate::parse_from_str(cleaned, "%Y%m%d").ok()?;
        let naive = date.and_hms_opt(23, 59, 0)?;
        let dt = source_tz.from_local_datetime(&naive).earliest()?;
        let utc_dt = dt.with_timezone(&Utc);
        let local_fmt = utc_dt
            .with_timezone(&default_tz)
            .format("%Y-%m-%d %H:%M")
            .to_string();
        return Some((utc_dt, local_fmt));
    }

    None
}

fn clean_lms_title(raw: &str) -> String {
    let mut title = raw.trim().to_string();
    for suffix in [" is due", " (due)", " - Due", " - due"] {
        if let Some(stripped) = title.strip_suffix(suffix) {
            title = stripped.trim().to_string();
        }
    }
    title
}

fn split_bracket_course(title: &str) -> (Option<String>, String) {
    let trimmed = title.trim();
    if let Some(rest) = trimmed.strip_prefix('[') {
        if let Some((course, remainder)) = rest.split_once(']') {
            let c = course.trim();
            let t = remainder.trim();
            if !c.is_empty() && !t.is_empty() {
                return (Some(c.to_string()), t.to_string());
            }
        }
    }
    (None, trimmed.to_string())
}

fn infer_kind_and_priority(text: &str) -> (TaskKind, Priority) {
    let lower = text.to_lowercase();
    if contains_any(&lower, &["ujian", "exam", "uts", "uas", "midterm", "final"]) {
        (TaskKind::Exam, Priority::High)
    } else if contains_any(&lower, &["kuis", "quiz"]) {
        (TaskKind::Quiz, Priority::High)
    } else if contains_any(
        &lower,
        &[
            "tugas",
            "assignment",
            "homework",
            "lab",
            "project",
            "proyek",
            "laporan",
            "due",
            "submission",
        ],
    ) {
        (TaskKind::Assignment, Priority::High)
    } else {
        (TaskKind::Announcement, Priority::Medium)
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
}

/// Strips basic HTML tags and common entities emitted inside Moodle/Canvas descriptions.
pub fn strip_html_tags(raw: &str) -> String {
    let mut normalized = raw
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n")
        .replace("</p>", "\n")
        .replace("</li>", "\n");
    let mut out = String::with_capacity(normalized.len());
    let mut in_tag = false;
    for ch in normalized.drain(..) {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    let decoded = out
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");
    decoded
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Default)]
struct RawVevent {
    uid: Option<String>,
    summary: Option<String>,
    description: Option<String>,
    categories: Vec<String>,
    location: Option<String>,
    url: Option<String>,
    dtstart: Option<(DateTime<Utc>, String)>,
    dtend: Option<(DateTime<Utc>, String)>,
}

/// Parses an RFC 5545 iCalendar feed exported by Moodle, Canvas, Google Classroom, or academic-agent.
pub fn parse_ics_events(
    raw_ics: &str,
    default_tz: Tz,
    default_course: &str,
) -> Vec<LmsIncomingEvent> {
    let lines = unfold_ics_lines(raw_ics);
    let mut events = Vec::new();
    let mut current: Option<RawVevent> = None;
    let mut subcomponent_depth = 0usize;

    for line in lines {
        let trimmed = line.trim();
        if trimmed.eq_ignore_ascii_case("BEGIN:VEVENT") {
            current = Some(RawVevent::default());
            subcomponent_depth = 0;
            continue;
        }
        if trimmed.eq_ignore_ascii_case("END:VEVENT") {
            if let Some(raw_ev) = current.take() {
                if let Some(parsed) = build_ics_incoming_event(raw_ev, default_course) {
                    events.push(parsed);
                }
            }
            subcomponent_depth = 0;
            continue;
        }

        let Some(ref mut ev) = current else {
            continue;
        };

        if trimmed.to_ascii_uppercase().starts_with("BEGIN:") {
            subcomponent_depth += 1;
            continue;
        }
        if trimmed.to_ascii_uppercase().starts_with("END:") {
            subcomponent_depth = subcomponent_depth.saturating_sub(1);
            continue;
        }
        if subcomponent_depth > 0 {
            continue;
        }

        let Some((left, val_raw)) = trimmed.split_once(':') else {
            continue;
        };
        let (prop_name, params) = left
            .split_once(';')
            .map(|(n, p)| (n.trim().to_ascii_uppercase(), p))
            .unwrap_or_else(|| (left.trim().to_ascii_uppercase(), ""));

        match prop_name.as_str() {
            "UID" => ev.uid = Some(unescape_ics_text(val_raw.trim())),
            "SUMMARY" => ev.summary = Some(unescape_ics_text(val_raw.trim())),
            "DESCRIPTION" => ev.description = Some(strip_html_tags(&unescape_ics_text(val_raw))),
            "LOCATION" => ev.location = Some(unescape_ics_text(val_raw.trim())),
            "URL" => ev.url = Some(unescape_ics_text(val_raw.trim())),
            "CATEGORIES" => {
                for cat in val_raw.split(',') {
                    let clean = unescape_ics_text(cat.trim());
                    if !clean.is_empty() {
                        ev.categories.push(clean);
                    }
                }
            }
            "DTSTART" => ev.dtstart = parse_ics_datetime(params, val_raw, default_tz),
            "DTEND" => ev.dtend = parse_ics_datetime(params, val_raw, default_tz),
            _ => {}
        }
    }

    events
}

fn build_ics_incoming_event(raw: RawVevent, default_course: &str) -> Option<LmsIncomingEvent> {
    let raw_summary = raw.summary?.trim().to_string();
    if raw_summary.is_empty() {
        return None;
    }

    let (bracket_course, stripped_title) = split_bracket_course(&raw_summary);
    let clean_title = clean_lms_title(&stripped_title);
    let title = if clean_title.is_empty() {
        raw_summary
    } else {
        clean_title
    };

    let cat_course = raw.categories.into_iter().find(|c| {
        let up = c.trim().to_ascii_uppercase();
        !up.is_empty() && !matches!(up.as_str(), "ACADEMIC" | "ASSIGNMENT" | "EVENT" | "MOODLE")
    });

    let course = bracket_course
        .or(cat_course)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            if default_course.trim().is_empty() {
                "Campus LMS".to_string()
            } else {
                default_course.trim().to_string()
            }
        });

    let deadline = match (raw.dtstart, raw.dtend) {
        (Some((start_utc, _)), Some((end_utc, end_raw))) if end_utc >= start_utc => {
            Some((end_utc, end_raw))
        }
        (_, Some(end)) => Some(end),
        (Some(start), None) => Some(start),
        (None, None) => None,
    };

    let (deadline_utc, deadline_raw) = match deadline {
        Some((utc, raw_str)) => (Some(utc), Some(raw_str)),
        None => (None, None),
    };

    let summary_text = raw
        .description
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            if let Some(ref loc) = raw.location {
                if !loc.trim().is_empty() {
                    return format!("{course}: {title} ({loc})");
                }
            }
            format!("{course}: {title}")
        });

    let (kind, priority) = infer_kind_and_priority(&format!("{title} {summary_text}"));
    let uid = raw.uid.filter(|u| !u.trim().is_empty()).unwrap_or_else(|| {
        sha256_hex(&format!(
            "{course}|{title}|{}",
            deadline_raw.as_deref().unwrap_or("")
        ))
    });

    Some(LmsIncomingEvent {
        uid,
        title,
        course,
        kind,
        deadline_raw,
        deadline_utc,
        priority,
        summary: summary_text,
        source_url: raw.url,
    })
}

async fn fetch_moodle_events(
    client: &Client,
    source: &LmsSourceConfig,
    tz: Tz,
) -> Result<Vec<LmsIncomingEvent>> {
    let token = source.token.trim();
    if token.is_empty() {
        return Err(Error::Config(format!(
            "LMS source '{}' (moodle_api) requires a non-empty token (wstoken)",
            source.name
        )));
    }
    let raw_url = source.url.trim().trim_end_matches('/');
    let endpoint = if raw_url.contains("webservice/rest/server.php") {
        raw_url.to_string()
    } else {
        format!("{raw_url}/webservice/rest/server.php")
    };
    let mut parsed = Url::parse(&endpoint)
        .map_err(|e| Error::Config(format!("invalid Moodle URL '{endpoint}': {e}")))?;
    let timesort_from = (Utc::now() - chrono::Duration::days(7)).timestamp();
    parsed
        .query_pairs_mut()
        .append_pair("wstoken", token)
        .append_pair("wsfunction", "core_calendar_get_action_events_by_timesort")
        .append_pair("moodlewsrestformat", "json")
        .append_pair("timesortfrom", &timesort_from.to_string())
        .append_pair("limitnum", "50");

    let resp = client.get(parsed).send().await?;
    let status = resp.status();
    let payload: Value = resp.json().await?;
    if !status.is_success() {
        return Err(Error::Config(format!(
            "Moodle API HTTP {status} for source '{}'",
            source.name
        )));
    }
    if let Some(ex) = payload.get("exception").and_then(|v| v.as_str()) {
        let msg = payload
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown Moodle error");
        return Err(Error::Config(format!("Moodle API error ({ex}): {msg}")));
    }

    let fallback_course = if source.default_course.trim().is_empty() {
        "Moodle LMS"
    } else {
        source.default_course.trim()
    };

    let events_arr = payload
        .get("events")
        .and_then(|v| v.as_array())
        .ok_or_else(|| Error::Parse("Moodle API response missing 'events' array".to_string()))?;

    let mut out = Vec::with_capacity(events_arr.len());
    for ev in events_arr {
        let title_raw = ev.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
        if title_raw.is_empty() {
            continue;
        }
        let title = clean_lms_title(title_raw);
        let uid = ev
            .get("id")
            .and_then(|v| v.as_u64().map(|n| n.to_string()))
            .or_else(|| ev.get("id").and_then(|v| v.as_str().map(str::to_string)))
            .unwrap_or_else(|| sha256_hex(title_raw));

        let course = ev
            .get("course")
            .and_then(|c| c.get("fullname").or_else(|| c.get("shortname")))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(fallback_course)
            .to_string();

        let (deadline_utc, deadline_raw) = ev
            .get("timesort")
            .and_then(|v| v.as_i64())
            .and_then(|ts| DateTime::<Utc>::from_timestamp(ts, 0))
            .map(|dt| {
                let formatted = dt.with_timezone(&tz).format("%Y-%m-%d %H:%M").to_string();
                (Some(dt), Some(formatted))
            })
            .unwrap_or((None, None));

        let desc = ev
            .get("description")
            .and_then(|v| v.as_str())
            .map(strip_html_tags)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("{course}: {title}"));

        let modulename = ev.get("modulename").and_then(|v| v.as_str()).unwrap_or("");
        let (kind, priority) = infer_kind_and_priority(&format!("{modulename} {title} {desc}"));
        let source_url = ev.get("url").and_then(|v| v.as_str()).map(str::to_string);

        out.push(LmsIncomingEvent {
            uid,
            title,
            course,
            kind,
            deadline_raw,
            deadline_utc,
            priority,
            summary: desc,
            source_url,
        });
    }

    Ok(out)
}

async fn fetch_canvas_events(
    client: &Client,
    source: &LmsSourceConfig,
    tz: Tz,
) -> Result<Vec<LmsIncomingEvent>> {
    let raw_url = source.url.trim().trim_end_matches('/');
    let endpoint = if raw_url.contains("/api/v1/") {
        raw_url.to_string()
    } else {
        format!("{raw_url}/api/v1/users/self/upcoming_events")
    };

    let mut req = client.get(&endpoint);
    if !source.token.trim().is_empty() {
        req = req.bearer_auth(source.token.trim());
    }
    let resp = req.send().await?;
    let status = resp.status();
    let payload: Value = resp.json().await?;
    if !status.is_success() {
        return Err(Error::Config(format!(
            "Canvas API HTTP {status} for source '{}'",
            source.name
        )));
    }
    let events_arr = payload
        .as_array()
        .ok_or_else(|| Error::Parse("Canvas API response was not a JSON array".to_string()))?;

    let fallback_course = if source.default_course.trim().is_empty() {
        "Canvas LMS"
    } else {
        source.default_course.trim()
    };

    let mut out = Vec::with_capacity(events_arr.len());
    for ev in events_arr {
        let title_raw = ev
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if title_raw.is_empty() {
            continue;
        }
        let title = clean_lms_title(title_raw);
        let uid = ev
            .get("id")
            .and_then(|v| {
                v.as_str()
                    .map(str::to_string)
                    .or_else(|| v.as_u64().map(|n| n.to_string()))
            })
            .unwrap_or_else(|| sha256_hex(title_raw));

        let course = ev
            .get("context_name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(fallback_course)
            .to_string();

        let due_iso = ev
            .get("assignment")
            .and_then(|a| a.get("due_at"))
            .and_then(|v| v.as_str())
            .or_else(|| ev.get("end_at").and_then(|v| v.as_str()))
            .or_else(|| ev.get("start_at").and_then(|v| v.as_str()));

        let (deadline_utc, deadline_raw) = due_iso
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| {
                let utc = dt.with_timezone(&Utc);
                let formatted = utc.with_timezone(&tz).format("%Y-%m-%d %H:%M").to_string();
                (Some(utc), Some(formatted))
            })
            .unwrap_or((None, None));

        let desc = ev
            .get("description")
            .and_then(|v| v.as_str())
            .map(strip_html_tags)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("{course}: {title}"));

        let (kind, priority) = infer_kind_and_priority(&format!("{title} {desc}"));
        let source_url = ev
            .get("html_url")
            .and_then(|v| v.as_str())
            .map(str::to_string);

        out.push(LmsIncomingEvent {
            uid,
            title,
            course,
            kind,
            deadline_raw,
            deadline_utc,
            priority,
            summary: desc,
            source_url,
        });
    }

    Ok(out)
}

/// Synchronizes a single LMS source (`IcsFeed`, `MoodleApi`, or `CanvasApi`) into `TaskStore`
/// inside one atomic `redb` batch transaction, preserving ETag caching and detecting shifted deadlines.
pub async fn sync_lms_source(
    store: &TaskStore,
    config: &Config,
    source: &LmsSourceConfig,
    notify_override: Option<bool>,
) -> Result<LmsSyncReport> {
    if source.url.trim().is_empty() {
        return Err(Error::Config(format!(
            "LMS source '{}' has an empty URL",
            source.name
        )));
    }
    let tz = config.timezone_tz()?;
    let client = Client::builder().timeout(Duration::from_secs(20)).build()?;
    let source_slug = safe_slug(&source.name, 32);
    let meta_key = format!("src:{source_slug}");
    let prev_meta = store.get_lms_sync_meta(&meta_key)?.unwrap_or_default();

    let mut new_etag = prev_meta.etag.clone();
    let mut new_last_mod = prev_meta.last_modified.clone();

    let events = match source.kind {
        LmsSourceKind::IcsFeed => {
            let mut req = client.get(source.url.trim());
            if !source.token.trim().is_empty() {
                req = req.bearer_auth(source.token.trim());
            }
            if let Some(ref etag) = prev_meta.etag {
                req = req.header(header::IF_NONE_MATCH, etag);
            }
            if let Some(ref lm) = prev_meta.last_modified {
                req = req.header(header::IF_MODIFIED_SINCE, lm);
            }

            let resp = req.send().await?;
            if resp.status() == StatusCode::NOT_MODIFIED {
                let meta = LmsSyncMeta {
                    etag: new_etag,
                    last_modified: new_last_mod,
                    last_synced_at: Some(Utc::now()),
                    last_event_count: prev_meta.last_event_count,
                };
                store.set_lms_sync_meta(&meta_key, &meta)?;
                return Ok(LmsSyncReport {
                    source_name: source.name.clone(),
                    kind: source.kind.as_str().to_string(),
                    not_modified: true,
                    fetched_events: 0,
                    inserted: 0,
                    updated: 0,
                    unchanged: prev_meta.last_event_count,
                    tasks_changed: Vec::new(),
                    notifications_sent: 0,
                });
            }
            if !resp.status().is_success() {
                return Err(Error::Config(format!(
                    "LMS feed HTTP {} for source '{}'",
                    resp.status(),
                    source.name
                )));
            }

            new_etag = resp
                .headers()
                .get(header::ETAG)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
                .or(new_etag);
            new_last_mod = resp
                .headers()
                .get(header::LAST_MODIFIED)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
                .or(new_last_mod);

            let body = resp.text().await?;
            parse_ics_events(&body, tz, &source.default_course)
        }
        LmsSourceKind::MoodleApi => fetch_moodle_events(&client, source, tz).await?,
        LmsSourceKind::CanvasApi => fetch_canvas_events(&client, source, tz).await?,
    };

    let fetched_events = events.len();
    let batch: Vec<NewTask> = events
        .into_iter()
        .map(|ev| NewTask {
            title: ev.title,
            course: ev.course,
            kind: ev.kind,
            deadline_raw: ev.deadline_raw,
            deadline_utc: ev.deadline_utc,
            priority: ev.priority,
            estimated_hours: 2,
            summary: ev.summary,
            subtasks: Vec::new(),
            deliverables: vec!["LMS submission".to_string()],
            ics_file: None,
            google_calendar_url: None,
            source_file: ev.source_url.or_else(|| Some(format!("lms:{source_slug}"))),
            content_hash: Some(format!("lms:{source_slug}:{}", ev.uid)),
            extractor: source.kind.as_str().to_string(),
        })
        .collect();

    let outcomes = store.upsert_lms_batch(batch)?;
    let should_notify = notify_override.unwrap_or(config.lms.notify_on_sync);

    let mut inserted = 0usize;
    let mut updated = 0usize;
    let mut unchanged = 0usize;
    let mut tasks_changed = Vec::new();
    let mut notifications_sent = 0usize;

    for outcome in outcomes {
        let mut task = match outcome {
            BatchItemOutcome::Inserted(t) => {
                inserted += 1;
                t
            }
            BatchItemOutcome::Updated(t) => {
                updated += 1;
                t
            }
            BatchItemOutcome::Unchanged(_) => {
                unchanged += 1;
                continue;
            }
        };

        let mut cal_opt = None;
        if task.deadline_raw.is_some() || task.deadline_utc.is_some() {
            let cal = create_ics_event(CalendarEventSpec {
                calendar_dir: &config.general.calendar_dir,
                title: &task.title,
                deadline_str: task.deadline_raw.as_deref(),
                tz,
                course: &task.course,
                description: &task.summary,
                duration_hours: config.general.default_duration_hours,
                priority: task.priority,
            })?;
            task = store.update_calendar_info(
                task.id,
                Some(cal.filename.clone()),
                Some(cal.google_calendar_url.clone()),
            )?;
            cal_opt = Some(cal);
        }

        if should_notify {
            let receipts = dispatch_new_task(
                &config.discord,
                &config.hermes,
                &config.n8n,
                &task,
                cal_opt.as_ref(),
            )
            .await;
            notifications_sent += receipts.iter().filter(|r| r.success).count();
        }
        tasks_changed.push(task);
    }

    let meta = LmsSyncMeta {
        etag: new_etag,
        last_modified: new_last_mod,
        last_synced_at: Some(Utc::now()),
        last_event_count: fetched_events,
    };
    store.set_lms_sync_meta(&meta_key, &meta)?;

    Ok(LmsSyncReport {
        source_name: source.name.clone(),
        kind: source.kind.as_str().to_string(),
        not_modified: false,
        fetched_events,
        inserted,
        updated,
        unchanged,
        tasks_changed,
        notifications_sent,
    })
}

/// Synchronizes all enabled LMS sources in `config.lms.sources`, or a specific named source when
/// `source_filter` is provided.
pub async fn sync_all_lms_sources(
    store: &TaskStore,
    config: &Config,
    source_filter: Option<&str>,
    notify_override: Option<bool>,
) -> Result<Vec<LmsSyncReport>> {
    if let Some(target_name) = source_filter.map(str::trim).filter(|s| !s.is_empty()) {
        let matched = config
            .lms
            .sources
            .iter()
            .find(|s| s.name == target_name)
            .ok_or_else(|| Error::NotFound(format!("LMS source '{target_name}' not found")))?;
        let report = sync_lms_source(store, config, matched, notify_override).await?;
        return Ok(vec![report]);
    }

    let mut reports = Vec::new();
    for src in config.lms.sources.iter().filter(|s| s.enabled) {
        reports.push(sync_lms_source(store, config, src, notify_override).await?);
    }
    Ok(reports)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::http::{header, HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::get;
    use axum::{Json, Router};
    use chrono_tz::Asia::Jakarta;
    use serde_json::json;
    use tempfile::tempdir;
    use tokio::net::TcpListener;

    use super::*;

    #[test]
    fn test_parse_ics_feed_moodle_canvas_unfolding_and_timezones() {
        let raw_ics = concat!(
            "BEGIN:VCALENDAR\r\n",
            "VERSION:2.0\r\n",
            "BEGIN:VEVENT\r\n",
            "UID:moodle-evt-101\r\n",
            "DTSTART:20261205T150000Z\r\n",
            "DTEND:20261205T165900Z\r\n",
            "SUMMARY:Quiz 2 Networks is due\r\n",
            "CATEGORIES:Computer Networks\r\n",
            "DESCRIPTION:<p>Study BGP routing\\, OSPF\\; and packet traces.\r\n",
            "  Bring laptop.</p>\r\n",
            "BEGIN:VALARM\r\n",
            "TRIGGER:-P1D\r\n",
            "SUMMARY:Alarm summary must NOT overwrite VEVENT summary\r\n",
            "END:VALARM\r\n",
            "END:VEVENT\r\n",
            "BEGIN:VEVENT\r\n",
            "UID:canvas-evt-202\r\n",
            "DTSTART;TZID=Asia/Jakarta:20261212T200000\r\n",
            "SUMMARY:[Distributed Systems] Raft Lab\r\n",
            "URL:https://lms.campus.ac.id/mod/assign/view.php?id=99\r\n",
            "END:VEVENT\r\n",
            "END:VCALENDAR\r\n",
        );

        let events = parse_ics_events(raw_ics, Jakarta, "Campus LMS");
        assert_eq!(events.len(), 2);

        assert_eq!(events[0].uid, "moodle-evt-101");
        assert_eq!(events[0].title, "Quiz 2 Networks");
        assert_eq!(events[0].course, "Computer Networks");
        assert_eq!(events[0].kind, TaskKind::Quiz);
        assert_eq!(events[0].deadline_raw.as_deref(), Some("2026-12-05 23:59"));
        assert!(events[0]
            .summary
            .contains("Study BGP routing, OSPF; and packet traces."));

        assert_eq!(events[1].uid, "canvas-evt-202");
        assert_eq!(events[1].course, "Distributed Systems");
        assert_eq!(events[1].title, "Raft Lab");
        assert_eq!(events[1].deadline_raw.as_deref(), Some("2026-12-12 20:00"));
        assert_eq!(
            events[1].source_url.as_deref(),
            Some("https://lms.campus.ac.id/mod/assign/view.php?id=99")
        );
    }

    #[tokio::test]
    async fn test_lms_ics_sync_etag_304_and_deadline_update_detection() {
        #[derive(Clone)]
        struct FeedState {
            version: Arc<Mutex<u8>>,
        }

        async fn feed_handler(
            axum::extract::State(st): axum::extract::State<FeedState>,
            headers: HeaderMap,
        ) -> impl IntoResponse {
            let ver = *st.version.lock().unwrap();
            let etag = format!("\"v{ver}\"");
            if headers
                .get(header::IF_NONE_MATCH)
                .and_then(|v| v.to_str().ok())
                == Some(etag.as_str())
            {
                return StatusCode::NOT_MODIFIED.into_response();
            }
            let due_utc = if ver == 1 {
                "20261215T165900Z"
            } else {
                "20261217T165900Z"
            };
            let body = format!(
                "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:moodle-a1\r\nDTSTART:{due_utc}\r\nSUMMARY:Tugas Akhir Sistem Operasi\r\nCATEGORIES:Operating Systems\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
            );
            (
                StatusCode::OK,
                [
                    (header::ETAG, etag),
                    (header::CONTENT_TYPE, "text/calendar".to_string()),
                ],
                body,
            )
                .into_response()
        }

        let state = FeedState {
            version: Arc::new(Mutex::new(1)),
        };
        let app = Router::new()
            .route("/feed.ics", get(feed_handler))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let dir = tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.resolve_paths_relative_to(dir.path());
        cfg.ensure_runtime_dirs().unwrap();
        cfg.discord.enabled = false;
        let store = TaskStore::open(&cfg.general.db_path).unwrap();

        let source = LmsSourceConfig {
            name: "moodle-main".to_string(),
            kind: LmsSourceKind::IcsFeed,
            url: format!("http://{addr}/feed.ics"),
            token: String::new(),
            default_course: "Campus LMS".to_string(),
            enabled: true,
        };

        // 1. Initial sync inserts 1 task and creates .ics artifact
        let rep1 = sync_lms_source(&store, &cfg, &source, Some(false))
            .await
            .unwrap();
        assert!(!rep1.not_modified);
        assert_eq!(rep1.inserted, 1);
        assert_eq!(
            rep1.tasks_changed[0].deadline_raw.as_deref(),
            Some("2026-12-15 23:59")
        );
        assert!(rep1.tasks_changed[0].ics_file.is_some());

        // 2. Second poll with unchanged ETag returns 304 Not Modified
        let rep2 = sync_lms_source(&store, &cfg, &source, Some(false))
            .await
            .unwrap();
        assert!(rep2.not_modified);
        assert_eq!(rep2.inserted, 0);
        assert_eq!(rep2.updated, 0);

        // 3. Lecturer extends deadline on LMS -> ETag v2 -> task updated in place
        *state.version.lock().unwrap() = 2;
        let rep3 = sync_lms_source(&store, &cfg, &source, Some(false))
            .await
            .unwrap();
        assert!(!rep3.not_modified);
        assert_eq!(rep3.updated, 1);
        assert_eq!(
            rep3.tasks_changed[0].deadline_raw.as_deref(),
            Some("2026-12-17 23:59")
        );
        assert_eq!(store.list_tasks(None).unwrap().len(), 1);

        server.abort();
    }

    #[tokio::test]
    async fn test_moodle_and_canvas_rest_adapters_offline() {
        let app = Router::new()
            .route(
                "/webservice/rest/server.php",
                get(|| async {
                    Json(json!({
                        "events": [{
                            "id": 9001,
                            "name": "UTS Basis Data",
                            "description": "<p>Ujian tulis dan query plan.</p>",
                            "timesort": 1797267540i64,
                            "modulename": "quiz",
                            "url": "https://lms.campus.ac.id/mod/quiz/view.php?id=9001",
                            "course": { "fullname": "IF301 Basis Data" }
                        }]
                    }))
                }),
            )
            .route(
                "/api/v1/users/self/upcoming_events",
                get(|headers: HeaderMap| async move {
                    let ok = headers
                        .get(header::AUTHORIZATION)
                        .and_then(|v| v.to_str().ok())
                        == Some("Bearer canvas-secret");
                    if !ok {
                        return (
                            StatusCode::UNAUTHORIZED,
                            Json(json!({ "error": "unauthorized" })),
                        )
                            .into_response();
                    }
                    Json(json!([{
                        "id": "assign_44",
                        "title": "Compiler Frontend Lab",
                        "description": "<p>Build recursive descent parser.</p>",
                        "context_name": "CS410 Compilers",
                        "html_url": "https://canvas.campus.ac.id/courses/1/assignments/44",
                        "assignment": { "due_at": "2026-12-20T16:59:00Z" }
                    }]))
                    .into_response()
                }),
            );

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let dir = tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.resolve_paths_relative_to(dir.path());
        cfg.ensure_runtime_dirs().unwrap();
        cfg.discord.enabled = false;
        cfg.lms.sources = vec![
            LmsSourceConfig {
                name: "moodle-api".to_string(),
                kind: LmsSourceKind::MoodleApi,
                url: format!("http://{addr}"),
                token: "moodle-token".to_string(),
                default_course: "Campus LMS".to_string(),
                enabled: true,
            },
            LmsSourceConfig {
                name: "canvas-api".to_string(),
                kind: LmsSourceKind::CanvasApi,
                url: format!("http://{addr}"),
                token: "canvas-secret".to_string(),
                default_course: "Canvas LMS".to_string(),
                enabled: true,
            },
        ];

        let store = TaskStore::open(&cfg.general.db_path).unwrap();
        let reports = sync_all_lms_sources(&store, &cfg, None, Some(false))
            .await
            .unwrap();
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0].inserted, 1);
        assert_eq!(reports[1].inserted, 1);

        let all = store.list_tasks(None).unwrap();
        assert_eq!(all.len(), 2);
        assert!(all
            .iter()
            .any(|t| t.course == "IF301 Basis Data" && t.title == "UTS Basis Data"));
        assert!(all
            .iter()
            .any(|t| t.course == "CS410 Compilers" && t.title == "Compiler Frontend Lab"));

        server.abort();
    }
}
