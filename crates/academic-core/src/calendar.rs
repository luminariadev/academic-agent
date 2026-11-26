use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use regex::Regex;
use serde::{Deserialize, Serialize};
use url::form_urlencoded;
use uuid::Uuid;

use crate::error::Result;
use crate::extract::Priority;
use crate::parser::safe_join_within;

/// Generated `.ics` calendar file metadata and 1-click Google Calendar link.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarArtifact {
    pub uid: String,
    pub summary: String,
    pub filename: String,
    pub file_path: PathBuf,
    pub start_utc: DateTime<Utc>,
    pub end_utc: DateTime<Utc>,
    pub ics_content: String,
    pub google_calendar_url: String,
}

fn month_number_from_name(raw: &str) -> Option<u32> {
    match raw.trim().to_lowercase().as_str() {
        "jan" | "januari" | "january" => Some(1),
        "feb" | "februari" | "february" => Some(2),
        "mar" | "maret" | "march" => Some(3),
        "apr" | "april" => Some(4),
        "mei" | "may" => Some(5),
        "jun" | "juni" | "june" => Some(6),
        "jul" | "juli" | "july" => Some(7),
        "agu" | "ags" | "agustus" | "aug" | "august" => Some(8),
        "sep" | "sept" | "september" => Some(9),
        "okt" | "oktober" | "oct" | "october" => Some(10),
        "nov" | "november" => Some(11),
        "des" | "desember" | "dec" | "december" => Some(12),
        _ => None,
    }
}

fn parse_time_hint(raw: Option<&str>) -> NaiveTime {
    if let Some(t) = raw {
        let normalized = t.trim().replace('.', ":");
        if let Ok(parsed) = NaiveTime::parse_from_str(&normalized, "%H:%M") {
            return parsed;
        }
        if let Ok(parsed) = NaiveTime::parse_from_str(&normalized, "%H:%M:%S") {
            return parsed;
        }
    }
    NaiveTime::from_hms_opt(23, 59, 0).expect("valid default end-of-day time")
}

/// Parses deadline strings in ISO, RFC 3339, `DD/MM/YYYY`, Indonesian, or English month forms,
/// returning a timezone-aware timestamp in `tz`.
pub fn parse_deadline_in_tz(raw: &str, tz: Tz) -> Option<DateTime<Tz>> {
    let cleaned = raw.trim();
    if cleaned.is_empty() {
        return None;
    }

    if let Ok(dt) = DateTime::parse_from_rfc3339(cleaned) {
        return Some(dt.with_timezone(&tz));
    }

    let dt_formats = [
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%d %H.%M",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%d/%m/%Y %H:%M",
        "%d-%m-%Y %H:%M",
    ];
    for fmt in dt_formats {
        if let Ok(naive) = NaiveDateTime::parse_from_str(cleaned, fmt) {
            if let Some(dt) = tz.from_local_datetime(&naive).earliest() {
                return Some(dt);
            }
        }
    }

    let date_formats = ["%Y-%m-%d", "%d/%m/%Y", "%d-%m-%Y"];
    for fmt in date_formats {
        if let Ok(date) = NaiveDate::parse_from_str(cleaned, fmt) {
            let naive = date.and_hms_opt(23, 59, 0)?;
            if let Some(dt) = tz.from_local_datetime(&naive).earliest() {
                return Some(dt);
            }
        }
    }

    // Try Indonesian or day-first month names: "15 Juli 2026" / "15 Juli 2026 23:59"
    let dmy_re = Regex::new(
        r"(?i)\b(\d{1,2})\s+([A-Za-z]+)\s+(20\d{2})(?:[\s,]+(?:pukul\s+|at\s+)?(\d{1,2}[:.]\d{2}))?",
    )
    .expect("valid dmy month regex");
    if let Some(caps) = dmy_re.captures(cleaned) {
        let day: u32 = caps[1].parse().ok()?;
        let month = month_number_from_name(&caps[2])?;
        let year: i32 = caps[3].parse().ok()?;
        if let Some(date) = NaiveDate::from_ymd_opt(year, month, day) {
            let time = parse_time_hint(caps.get(4).map(|m| m.as_str()));
            let naive = NaiveDateTime::new(date, time);
            if let Some(dt) = tz.from_local_datetime(&naive).earliest() {
                return Some(dt);
            }
        }
    }

    // Try English month-first dates: "July 15, 2026" / "Dec 15 2026 18:00"
    let mdy_re = Regex::new(
        r"(?i)\b([A-Za-z]+)\s+(\d{1,2}),?\s+(20\d{2})(?:[\s,]+(?:at\s+)?(\d{1,2}[:.]\d{2}))?",
    )
    .expect("valid mdy month regex");
    if let Some(caps) = mdy_re.captures(cleaned) {
        let month = month_number_from_name(&caps[1])?;
        let day: u32 = caps[2].parse().ok()?;
        let year: i32 = caps[3].parse().ok()?;
        if let Some(date) = NaiveDate::from_ymd_opt(year, month, day) {
            let time = parse_time_hint(caps.get(4).map(|m| m.as_str()));
            let naive = NaiveDateTime::new(date, time);
            if let Some(dt) = tz.from_local_datetime(&naive).earliest() {
                return Some(dt);
            }
        }
    }

    // Fallback regex anywhere inside longer strings
    let iso_re =
        Regex::new(r"(20\d{2}-\d{2}-\d{2})(?:[ T](\d{1,2}[:.]\d{2}))?").expect("valid iso regex");
    if let Some(caps) = iso_re.captures(cleaned) {
        if let Ok(date) = NaiveDate::parse_from_str(&caps[1], "%Y-%m-%d") {
            let time = parse_time_hint(caps.get(2).map(|m| m.as_str()));
            let naive = NaiveDateTime::new(date, time);
            if let Some(dt) = tz.from_local_datetime(&naive).earliest() {
                return Some(dt);
            }
        }
    }

    None
}

/// Escapes special characters according to RFC 5545 Section 3.3.11.
pub fn escape_ics(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace("\r\n", "\\n")
        .replace('\n', "\\n")
        .replace(',', "\\,")
        .replace(';', "\\;")
}

/// Builds a filesystem-safe identifier slug bounded to `max_len` bytes.
pub fn safe_slug(text: &str, max_len: usize) -> String {
    let mut slug = String::new();
    let mut prev_underscore = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            slug.push(ch);
            prev_underscore = ch == '_';
        } else if !prev_underscore && !slug.is_empty() {
            slug.push('_');
            prev_underscore = true;
        }
        if slug.len() >= max_len {
            break;
        }
    }
    let trimmed = slug.trim_matches('_').to_string();
    if trimmed.is_empty() {
        "academic_event".to_string()
    } else {
        trimmed
    }
}

/// Generates a 1-click Google Calendar web link with proper UTC conversion and a hard
/// 512-character total URL bound so Discord link buttons never fail validation.
pub fn generate_google_calendar_url(
    title: &str,
    start_time: DateTime<Tz>,
    end_time: DateTime<Tz>,
    description: &str,
    location: &str,
) -> String {
    let start_utc = start_time.with_timezone(&Utc).format("%Y%m%dT%H%M%SZ");
    let end_utc = end_time.with_timezone(&Utc).format("%Y%m%dT%H%M%SZ");
    let dates = format!("{start_utc}/{end_utc}");

    let clean_title: String = title.trim().chars().take(70).collect();
    let clean_location: String = location.trim().chars().take(40).collect();
    let base_desc: String = description.replace(['\r', '\n'], " ").trim().to_string();

    for desc_limit in [140usize, 80, 40, 0] {
        let truncated_desc: String = base_desc.chars().take(desc_limit).collect();
        let query = form_urlencoded::Serializer::new(String::new())
            .append_pair("action", "TEMPLATE")
            .append_pair(
                "text",
                if clean_title.is_empty() {
                    "Academic Task"
                } else {
                    &clean_title
                },
            )
            .append_pair("dates", &dates)
            .append_pair("details", &truncated_desc)
            .append_pair("location", &clean_location)
            .finish();
        let url = format!("https://calendar.google.com/calendar/render?{query}");
        if url.len() <= 512 || desc_limit == 0 {
            return url;
        }
    }

    // Defensive fallback guaranteed well under 512 characters
    let query = form_urlencoded::Serializer::new(String::new())
        .append_pair("action", "TEMPLATE")
        .append_pair("text", "Academic Task")
        .append_pair("dates", &dates)
        .finish();
    format!("https://calendar.google.com/calendar/render?{query}")
}

/// Input specification for generating an `.ics` calendar artifact.
#[derive(Debug, Clone, Copy)]
pub struct CalendarEventSpec<'a> {
    pub calendar_dir: &'a Path,
    pub title: &'a str,
    pub deadline_str: Option<&'a str>,
    pub tz: Tz,
    pub course: &'a str,
    pub description: &'a str,
    pub duration_hours: u32,
    pub priority: Priority,
}

/// Creates an RFC 5545 `.ics` file in `spec.calendar_dir` (with UTC timestamps and dual VALARM
/// triggers `-P1D` and `-PT1H`) plus a matching Google Calendar 1-click URL.
pub fn create_ics_event(spec: CalendarEventSpec<'_>) -> Result<CalendarArtifact> {
    let tz = spec.tz;
    let end_tz = spec
        .deadline_str
        .and_then(|s| parse_deadline_in_tz(s, tz))
        .unwrap_or_else(|| {
            let future = Utc::now().with_timezone(&tz) + Duration::days(7);
            future
                .date_naive()
                .and_hms_opt(23, 59, 0)
                .and_then(|ndt| tz.from_local_datetime(&ndt).earliest())
                .unwrap_or(future)
        });
    let start_tz = end_tz - Duration::hours(i64::from(spec.duration_hours.max(1)));
    let start_utc = start_tz.with_timezone(&Utc);
    let end_utc = end_tz.with_timezone(&Utc);
    let now_utc = Utc::now();

    let uid = Uuid::new_v4().to_string();
    let summary = if spec.course.trim().is_empty() {
        spec.title.trim().to_string()
    } else {
        format!("[{}] {}", spec.course.trim(), spec.title.trim())
    };
    let prio_val = match spec.priority {
        Priority::High => 1,
        Priority::Medium => 5,
        Priority::Low => 9,
    };

    let filename = format!(
        "{}_{}.ics",
        end_tz.format("%Y%m%d_%H%M"),
        safe_slug(&summary, 48)
    );
    let file_path = safe_join_within(spec.calendar_dir, &filename)?;

    let lines = [
        "BEGIN:VCALENDAR".to_string(),
        "VERSION:2.0".to_string(),
        "PRODID:-//luminariadev//academic-agent//EN".to_string(),
        "CALSCALE:GREGORIAN".to_string(),
        "METHOD:PUBLISH".to_string(),
        "BEGIN:VEVENT".to_string(),
        format!("UID:{uid}"),
        format!("DTSTAMP:{}", now_utc.format("%Y%m%dT%H%M%SZ")),
        format!("DTSTART:{}", start_utc.format("%Y%m%dT%H%M%SZ")),
        format!("DTEND:{}", end_utc.format("%Y%m%dT%H%M%SZ")),
        format!("SUMMARY:{}", escape_ics(&summary)),
        format!("DESCRIPTION:{}", escape_ics(spec.description)),
        "CATEGORIES:ACADEMIC,ASSIGNMENT".to_string(),
        format!("PRIORITY:{prio_val}"),
        "STATUS:CONFIRMED".to_string(),
        "BEGIN:VALARM".to_string(),
        "TRIGGER:-P1D".to_string(),
        "ACTION:DISPLAY".to_string(),
        format!(
            "DESCRIPTION:{}",
            escape_ics(&format!("24h reminder: {summary}"))
        ),
        "END:VALARM".to_string(),
        "BEGIN:VALARM".to_string(),
        "TRIGGER:-PT1H".to_string(),
        "ACTION:DISPLAY".to_string(),
        format!(
            "DESCRIPTION:{}",
            escape_ics(&format!("1h reminder: {summary}"))
        ),
        "END:VALARM".to_string(),
        "END:VEVENT".to_string(),
        "END:VCALENDAR".to_string(),
        String::new(),
    ];

    let ics_content = lines.join("\r\n");
    std::fs::write(&file_path, &ics_content)?;

    let google_calendar_url = generate_google_calendar_url(
        &summary,
        start_tz,
        end_tz,
        spec.description,
        "Online / Campus LMS",
    );

    Ok(CalendarArtifact {
        uid,
        summary,
        filename,
        file_path,
        start_utc,
        end_utc,
        ics_content,
        google_calendar_url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::Asia::Jakarta;
    use tempfile::tempdir;

    #[test]
    fn test_wib_to_utc_google_calendar_and_ics_conversion() {
        let dt = parse_deadline_in_tz("2026-11-27 23:59", Jakarta).expect("parses ISO deadline");
        assert_eq!(
            dt.with_timezone(&Utc).format("%Y%m%dT%H%M%SZ").to_string(),
            "20261127T165900Z"
        );

        let dir = tempdir().unwrap();
        let cal = create_ics_event(CalendarEventSpec {
            calendar_dir: dir.path(),
            title: "Raft Consensus Lab",
            deadline_str: Some("2026-11-27 23:59"),
            tz: Jakarta,
            course: "Distributed Systems",
            description: "Implement leader election; include benchmarks.",
            duration_hours: 2,
            priority: Priority::High,
        })
        .expect("creates ics");

        assert!(cal.file_path.exists());
        assert!(cal.ics_content.contains("DTSTART:20261127T145900Z"));
        assert!(cal.ics_content.contains("DTEND:20261127T165900Z"));
        assert!(cal
            .ics_content
            .contains("Implement leader election\\; include benchmarks."));
        assert!(cal
            .google_calendar_url
            .contains("20261127T145900Z%2F20261127T165900Z"));
        assert!(cal.google_calendar_url.len() <= 512);
    }

    #[test]
    fn test_bilingual_deadline_formats() {
        assert_eq!(
            parse_deadline_in_tz("15 Juli 2026 18:30", Jakarta)
                .unwrap()
                .format("%Y-%m-%d %H:%M")
                .to_string(),
            "2026-07-15 18:30"
        );
        assert_eq!(
            parse_deadline_in_tz("December 20, 2026", Jakarta)
                .unwrap()
                .format("%Y-%m-%d %H:%M")
                .to_string(),
            "2026-12-20 23:59"
        );
        assert_eq!(
            parse_deadline_in_tz("25/11/2026 09:00", Jakarta)
                .unwrap()
                .format("%Y-%m-%d %H:%M")
                .to_string(),
            "2026-11-25 09:00"
        );
    }
}
