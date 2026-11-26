use std::time::Duration;

use chrono_tz::Tz;
use regex::Regex;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::warn;

use crate::calendar::parse_deadline_in_tz;
use crate::config::LlmConfig;
use crate::error::{Error, Result};

/// Academic document category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    #[default]
    Assignment,
    Quiz,
    Exam,
    Syllabus,
    Announcement,
    General,
}

impl TaskKind {
    pub fn parse_loose(raw: &str) -> Self {
        match raw.trim().to_lowercase().as_str() {
            "assignment" | "tugas" => Self::Assignment,
            "quiz" | "kuis" => Self::Quiz,
            "exam" | "ujian" | "uts" | "uas" => Self::Exam,
            "syllabus" | "silabus" => Self::Syllabus,
            "announcement" | "pengumuman" => Self::Announcement,
            _ => Self::General,
        }
    }
}

/// Task urgency level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Priority {
    High,
    #[default]
    Medium,
    Low,
}

impl Priority {
    pub fn parse_loose(raw: &str) -> Self {
        match raw.trim().to_uppercase().as_str() {
            "HIGH" | "TINGGI" | "URGENT" => Self::High,
            "LOW" | "RENDAH" => Self::Low,
            _ => Self::Medium,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::High => "HIGH",
            Self::Medium => "MEDIUM",
            Self::Low => "LOW",
        }
    }
}

/// Normalized structured task output produced by the LLM or heuristic extractor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskExtraction {
    pub is_academic: bool,
    pub kind: TaskKind,
    pub course: String,
    pub title: String,
    pub deadline: Option<String>,
    pub estimated_hours: u32,
    pub priority: Priority,
    pub summary: String,
    pub subtasks: Vec<String>,
    pub deliverables: Vec<String>,
    pub extractor: String,
}

#[derive(Debug, Deserialize)]
struct RawLlmPayload {
    #[serde(default = "default_true")]
    is_academic: bool,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    course: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    deadline: Option<String>,
    #[serde(default)]
    estimated_hours: Option<u32>,
    #[serde(default)]
    priority: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    subtasks: Option<Vec<String>>,
    #[serde(default)]
    deliverables: Option<Vec<String>>,
}

fn default_true() -> bool {
    true
}

const SYSTEM_PROMPT: &str = r#"You extract structured academic tasks from course announcements, syllabi, and assignments (in Indonesian or English).
Return ONLY valid raw JSON (no markdown fences, no commentary) with this schema:
{
  "is_academic": true,
  "kind": "assignment" | "quiz" | "exam" | "syllabus" | "announcement" | "general",
  "course": "Course name",
  "title": "Specific task title",
  "deadline": "YYYY-MM-DD HH:MM" or null,
  "estimated_hours": 3,
  "priority": "HIGH" | "MEDIUM" | "LOW",
  "summary": "Concise 1-3 sentence summary of requirements",
  "subtasks": ["Actionable step 1", "Actionable step 2"],
  "deliverables": ["Expected submission format"]
}
If only a date is present without a time, default time to 23:59."#;

/// Strips optional markdown code blocks and parses the outermost JSON object from LLM text.
pub fn parse_llm_json(raw_text: &str) -> Option<serde_json::Value> {
    let trimmed = raw_text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let fence_start = Regex::new(r"^```(?:json)?\s*").ok()?;
    let fence_end = Regex::new(r"\s*```$").ok()?;
    let mut cleaned = fence_start.replace(trimmed, "").into_owned();
    cleaned = fence_end.replace(&cleaned, "").into_owned();

    if let Ok(val) = serde_json::from_str::<serde_json::Value>(cleaned.trim()) {
        if val.is_object() {
            return Some(val);
        }
    }

    if let (Some(start), Some(end)) = (cleaned.find('{'), cleaned.rfind('}')) {
        if end > start {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&cleaned[start..=end]) {
                if val.is_object() {
                    return Some(val);
                }
            }
        }
    }
    None
}

fn clean_title_from_filename(filename: &str) -> String {
    let stem = std::path::Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(filename);
    let spaced = stem.replace(['_', '-'], " ");
    let trimmed = spaced.trim();
    if trimmed.is_empty() {
        "Academic Task".to_string()
    } else {
        trimmed
            .split_whitespace()
            .map(|word| {
                let mut chars = word.chars();
                match chars.next() {
                    None => String::new(),
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn find_deadline_candidate(text: &str, tz: Tz) -> Option<String> {
    // 1. ISO date/time patterns
    let iso_re = Regex::new(r"(20\d{2}-\d{2}-\d{2}(?:[ T]\d{1,2}[:.]\d{2}(?::\d{2})?)?)").ok()?;
    if let Some(m) = iso_re.find(text) {
        if let Some(dt) = parse_deadline_in_tz(m.as_str(), tz) {
            return Some(dt.format("%Y-%m-%d %H:%M").to_string());
        }
    }

    // 2. DD/MM/YYYY or DD-MM-YYYY patterns
    let slash_re = Regex::new(r"(\d{1,2}[/-]\d{1,2}[/-]20\d{2}(?:\s+\d{1,2}[:.]\d{2})?)").ok()?;
    if let Some(m) = slash_re.find(text) {
        if let Some(dt) = parse_deadline_in_tz(m.as_str(), tz) {
            return Some(dt.format("%Y-%m-%d %H:%M").to_string());
        }
    }

    // 3. Indonesian / day-first month names: 15 Juli 2026 [18:00]
    let dmy_month = Regex::new(
        r"(?i)\b\d{1,2}\s+(?:Januari|January|Jan|Februari|February|Feb|Maret|March|Mar|April|Apr|Mei|May|Juni|June|Jun|Juli|July|Jul|Agustus|August|Agu|Ags|Aug|September|Sep|Sept|Oktober|October|Okt|Oct|November|Nov|Desember|December|Des|Dec)\s+20\d{2}(?:[\s,]+(?:pukul\s+|at\s+)?\d{1,2}[:.]\d{2})?",
    )
    .ok()?;
    if let Some(m) = dmy_month.find(text) {
        if let Some(dt) = parse_deadline_in_tz(m.as_str(), tz) {
            return Some(dt.format("%Y-%m-%d %H:%M").to_string());
        }
    }

    // 4. English month-first names: July 15, 2026 [18:00]
    let mdy_month = Regex::new(
        r"(?i)\b(?:January|Jan|February|Feb|March|Mar|April|Apr|May|June|Jun|July|Jul|August|Aug|September|Sep|Sept|October|Oct|November|Nov|December|Dec)\s+\d{1,2},?\s+20\d{2}(?:[\s,]+(?:at\s+)?\d{1,2}[:.]\d{2})?",
    )
    .ok()?;
    if let Some(m) = mdy_month.find(text) {
        if let Some(dt) = parse_deadline_in_tz(m.as_str(), tz) {
            return Some(dt.format("%Y-%m-%d %H:%M").to_string());
        }
    }

    None
}

/// Deterministic bilingual (Indonesian + English) heuristic extractor used when offline
/// or when LLM inference fails.
pub fn heuristic_extract(text: &str, filename: &str, tz: Tz) -> TaskExtraction {
    let lowered = text.to_lowercase();

    let academic_re = Regex::new(
        r"(?i)\b(?:tugas|deadline|pengumpulan|silabus|mata\s+kuliah|dosen|ujian|uts|uas|kuis|quiz|exam|assignment|syllabus|course|lecturer|homework|project|proyek|laporan|report|submission|due|kuliah|lab)\b",
    )
    .expect("valid academic keyword regex");
    let exam_re = Regex::new(r"(?i)\b(?:ujian|exam|uts|uas)\b").expect("valid exam regex");
    let quiz_re = Regex::new(r"(?i)\b(?:kuis|quiz)\b").expect("valid quiz regex");
    let assign_re =
        Regex::new(r"(?i)\b(?:tugas|assignment|homework|deadline|due|proyek|project|lab)\b")
            .expect("valid assignment regex");
    let syllabus_re =
        Regex::new(r"(?i)\b(?:silabus|syllabus|rps)\b").expect("valid syllabus regex");
    let announce_re = Regex::new(r"(?i)\b(?:pengumuman|announcement|jadwal|schedule)\b")
        .expect("valid announcement regex");

    let is_academic = academic_re.is_match(text);

    let (kind, priority, est_hours) = if exam_re.is_match(text) {
        (TaskKind::Exam, Priority::High, 5)
    } else if quiz_re.is_match(text) {
        (TaskKind::Quiz, Priority::High, 2)
    } else if assign_re.is_match(text) {
        (TaskKind::Assignment, Priority::High, 3)
    } else if syllabus_re.is_match(text) {
        (TaskKind::Syllabus, Priority::Low, 1)
    } else if announce_re.is_match(text) {
        (TaskKind::Announcement, Priority::Medium, 1)
    } else {
        (TaskKind::General, Priority::Medium, 2)
    };

    let course_re =
        Regex::new(r"(?im)^(?:mata\s+kuliah|matakuliah|course|mk)\s*[:=\-]\s*([^\r\n]+)")
            .expect("valid course regex");
    let course = if let Some(caps) = course_re.captures(text) {
        caps[1].trim().to_string()
    } else {
        let lower_name = filename.to_lowercase();
        let known = [
            "Metode Penelitian",
            "Research Methods",
            "Machine Learning",
            "Artificial Intelligence",
            "Kecerdasan Buatan",
            "Distributed Systems",
            "Sistem Terdistribusi",
            "Computer Networks",
            "Jaringan Komputer",
            "Database Systems",
            "Basis Data",
        ];
        known
            .iter()
            .find(|c| lowered.contains(&c.to_lowercase()) || lower_name.contains(&c.to_lowercase()))
            .map(|c| (*c).to_string())
            .unwrap_or_else(|| "General Course".to_string())
    };

    let title_re = Regex::new(
        r"(?im)^(?:judul|title|topik|topic|tugas|assignment|subject)\s*[:=\-]\s*([^\r\n]+)",
    )
    .expect("valid title regex");
    let title = if let Some(caps) = title_re.captures(text) {
        caps[1].trim().to_string()
    } else if !filename.trim().is_empty() {
        clean_title_from_filename(filename)
    } else {
        text.lines()
            .find(|l| !l.trim().is_empty())
            .map(|l| l.trim().chars().take(80).collect())
            .unwrap_or_else(|| "Academic Task".to_string())
    };

    let deadline = find_deadline_candidate(text, tz);

    let summary = if let Some(ref dl) = deadline {
        format!("{course}: {title} (due {dl}).")
    } else {
        format!("{course}: {title}.")
    };

    let subtasks = vec![
        format!("Review brief and requirements for {course}"),
        "Prepare draft solution or report".to_string(),
        "Verify formatting and deliverables".to_string(),
        "Submit before the deadline".to_string(),
    ];

    let mut deliverables = Vec::new();
    for (needle, label) in [
        ("pdf", "PDF document"),
        ("docx", "DOCX document"),
        ("github", "GitHub repository"),
        ("zip", "ZIP archive"),
        ("slide", "Presentation slides"),
    ] {
        if lowered.contains(needle) {
            deliverables.push(label.to_string());
        }
    }
    if deliverables.is_empty() {
        deliverables.push("Document or submission link".to_string());
    }

    TaskExtraction {
        is_academic,
        kind,
        course,
        title,
        deadline,
        estimated_hours: est_hours,
        priority,
        summary,
        subtasks,
        deliverables,
        extractor: "heuristic".to_string(),
    }
}

/// Calls the configured Chat Completions compatible endpoint to extract structured task data,
/// falling back to the deterministic bilingual heuristic when enabled.
pub async fn extract_from_text(
    llm: &LlmConfig,
    text: &str,
    filename: &str,
    tz: Tz,
    force_heuristic: bool,
) -> Result<TaskExtraction> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(Error::Parse(
            "cannot extract task from empty text".to_string(),
        ));
    }

    let api_base = llm.api_base.trim().trim_end_matches('/');
    if force_heuristic || api_base.is_empty() || api_base.contains("${") {
        return Ok(heuristic_extract(trimmed, filename, tz));
    }

    let snippet: String = trimmed.chars().take(12_000).collect();
    let user_prompt = format!(
        "Document Name: {}\nTimezone: {}\n\nContent:\n{}\n\nReturn JSON only.",
        if filename.is_empty() {
            "input.txt"
        } else {
            filename
        },
        tz.name(),
        snippet
    );

    let url = format!("{api_base}/chat/completions");
    let client = Client::builder()
        .timeout(Duration::from_secs(llm.timeout_secs.max(1)))
        .build()
        .map_err(|e| Error::Http(format!("failed building LLM HTTP client: {e}")))?;

    let mut req = client.post(&url).json(&json!({
        "model": llm.model,
        "temperature": llm.temperature,
        "stream": false,
        "messages": [
            { "role": "system", "content": SYSTEM_PROMPT },
            { "role": "user", "content": user_prompt }
        ]
    }));

    let key = llm.api_key.trim();
    if !key.is_empty() && !key.contains("${") {
        req = req.bearer_auth(key);
    }

    let attempt: Result<TaskExtraction> = async {
        let resp = req
            .send()
            .await
            .map_err(|e| Error::Http(format!("LLM request to '{url}' failed: {e}")))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(Error::Http(format!(
                "LLM endpoint returned HTTP {status}: {body}"
            )));
        }

        let payload: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| Error::Parse(format!("invalid LLM HTTP JSON envelope: {e}")))?;
        let raw_content = payload["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("");
        let json_val = parse_llm_json(raw_content).ok_or_else(|| {
            Error::Parse("LLM message did not contain a valid JSON object".to_string())
        })?;
        let parsed: RawLlmPayload = serde_json::from_value(json_val)?;

        let title = parsed
            .title
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| Error::Parse("LLM JSON omitted required 'title' field".to_string()))?;

        let deadline = parsed
            .deadline
            .and_then(|d| parse_deadline_in_tz(&d, tz))
            .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
            .or_else(|| find_deadline_candidate(trimmed, tz));

        Ok(TaskExtraction {
            is_academic: parsed.is_academic,
            kind: parsed
                .kind
                .as_deref()
                .map(TaskKind::parse_loose)
                .unwrap_or(TaskKind::Assignment),
            course: parsed
                .course
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "General Course".to_string()),
            title,
            deadline,
            estimated_hours: parsed.estimated_hours.unwrap_or(2).max(1),
            priority: parsed
                .priority
                .as_deref()
                .map(Priority::parse_loose)
                .unwrap_or(Priority::Medium),
            summary: parsed
                .summary
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "Extracted academic task.".to_string()),
            subtasks: parsed
                .subtasks
                .unwrap_or_default()
                .into_iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            deliverables: parsed
                .deliverables
                .unwrap_or_default()
                .into_iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            extractor: "llm".to_string(),
        })
    }
    .await;

    match attempt {
        Ok(res) => Ok(res),
        Err(err) if llm.fallback_to_heuristic => {
            warn!("LLM extraction failed ({err}); falling back to heuristic extractor");
            Ok(heuristic_extract(trimmed, filename, tz))
        }
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::Asia::Jakarta;

    #[test]
    fn test_heuristic_bilingual_extraction() {
        let indonesian_text = "\
Mata Kuliah: Jaringan Komputer Lanjut
Judul: Kuis 2 SDN dan BGP
Deadline: 22 November 2026 10:00
Format pengumpulan: PDF melalui portal kampus.";
        let res = heuristic_extract(indonesian_text, "pengumuman.txt", Jakarta);
        assert!(res.is_academic);
        assert_eq!(res.kind, TaskKind::Quiz);
        assert_eq!(res.priority, Priority::High);
        assert_eq!(res.course, "Jaringan Komputer Lanjut");
        assert_eq!(res.title, "Kuis 2 SDN dan BGP");
        assert_eq!(res.deadline.as_deref(), Some("2026-11-22 10:00"));
        assert!(res.deliverables.iter().any(|d| d.contains("PDF")));
    }

    #[test]
    fn test_parse_llm_json_handles_fences() {
        let fenced =
            "Here is the result:\n```json\n{\"title\": \"Lab 3\", \"priority\": \"HIGH\"}\n```";
        let val = parse_llm_json(fenced).expect("parses fenced json");
        assert_eq!(val["title"], "Lab 3");
    }

    #[test]
    fn test_word_boundary_prevents_timeouts_exam_false_positive() {
        let assignment_text = "\
Course: Distributed Systems
Title: Raft Consensus Lab
Deadline: 2026-12-10 23:59
Implement leader election timeouts and RequestVote RPC.";
        let res = heuristic_extract(assignment_text, "sample_assignment.txt", Jakarta);
        assert!(res.is_academic);
        assert_eq!(res.kind, TaskKind::Assignment);
        assert_eq!(res.estimated_hours, 3);
    }
}
