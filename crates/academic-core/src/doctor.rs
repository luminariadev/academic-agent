use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::calendar::safe_slug;
use crate::config::{redact_url_secrets, Config};
use crate::store::TaskStore;

/// Health state for an individual diagnostic check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

/// Single system readiness check emitted by `academic-agent doctor`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorCheck {
    pub name: String,
    pub status: CheckStatus,
    pub summary: String,
    pub hint: String,
}

/// Complete diagnostic report returned by CLI `doctor` and `GET /api/doctor`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    pub ok: bool,
    pub version: String,
    pub timezone: String,
    pub checks: Vec<DoctorCheck>,
}

fn is_configured_secret(raw: &str) -> bool {
    let t = raw.trim();
    !t.is_empty() && !(t.starts_with("${") && t.ends_with('}'))
}

/// Inspects configuration, database readability, AI/heuristic extractor, Discord setup,
/// cron scheduler, LMS sources, and calendar subscription readiness.
pub fn run_doctor(config_path: &Path, config: &Config, store: &TaskStore) -> DoctorReport {
    let mut checks = Vec::new();

    // 1. Configuration file
    if config_path.exists() {
        checks.push(DoctorCheck {
            name: "config_file".to_string(),
            status: CheckStatus::Pass,
            summary: format!("Loaded configuration from '{}'", config_path.display()),
            hint: "OK".to_string(),
        });
    } else {
        checks.push(DoctorCheck {
            name: "config_file".to_string(),
            status: CheckStatus::Warn,
            summary: format!("'{}' does not exist on disk", config_path.display()),
            hint: "Run `academic-agent setup` or `academic-agent init` to persist settings."
                .to_string(),
        });
    }

    // 2. Timezone
    match config.timezone_tz() {
        Ok(tz) => checks.push(DoctorCheck {
            name: "timezone".to_string(),
            status: CheckStatus::Pass,
            summary: format!("Timezone configured as '{}'", tz.name()),
            hint: "OK".to_string(),
        }),
        Err(e) => checks.push(DoctorCheck {
            name: "timezone".to_string(),
            status: CheckStatus::Fail,
            summary: format!("Invalid timezone: {e}"),
            hint: "Set [general].timezone to a valid IANA name such as 'Asia/Jakarta'.".to_string(),
        }),
    }

    // 3. Embedded redb database
    match store.list_tasks(None) {
        Ok(tasks) => checks.push(DoctorCheck {
            name: "database".to_string(),
            status: CheckStatus::Pass,
            summary: format!(
                "redb store at '{}' reachable ({} task(s))",
                config.general.db_path.display(),
                tasks.len()
            ),
            hint: "OK".to_string(),
        }),
        Err(e) => checks.push(DoctorCheck {
            name: "database".to_string(),
            status: CheckStatus::Fail,
            summary: format!("Failed reading redb task store: {e}"),
            hint: "Check filesystem permissions or --db path.".to_string(),
        }),
    }

    // 4. Document extractor (LLM + deterministic heuristic)
    if is_configured_secret(&config.llm.api_base) && is_configured_secret(&config.llm.api_key) {
        checks.push(DoctorCheck {
            name: "extractor".to_string(),
            status: CheckStatus::Pass,
            summary: format!(
                "LLM gateway configured (model '{}') with offline heuristic fallback={}",
                config.llm.model, config.llm.fallback_to_heuristic
            ),
            hint: "OK".to_string(),
        });
    } else {
        checks.push(DoctorCheck {
            name: "extractor".to_string(),
            status: CheckStatus::Warn,
            summary: "LLM gateway credentials not set; running in offline Indonesian/English heuristic mode".to_string(),
            hint: "Set ACADEMIC_LLM_API_BASE and ACADEMIC_LLM_API_KEY in .env to enable LLM extraction.".to_string(),
        });
    }

    // 5. Discord integration
    let has_bot = is_configured_secret(&config.discord.bot_token)
        && is_configured_secret(&config.discord.channel_id);
    let has_app = is_configured_secret(&config.discord.application_id);
    let has_webhook = is_configured_secret(&config.discord.webhook_url);
    if !config.discord.enabled {
        checks.push(DoctorCheck {
            name: "discord".to_string(),
            status: CheckStatus::Warn,
            summary: "Discord notifications are disabled ([discord].enabled = false)".to_string(),
            hint: "Set [discord].enabled = true and run `academic-agent test-discord` to enable alerts.".to_string(),
        });
    } else if has_bot && has_app {
        checks.push(DoctorCheck {
            name: "discord".to_string(),
            status: CheckStatus::Pass,
            summary: format!(
                "Discord Bot REST + two-way slash commands ready (allowed_users={})",
                if config.discord.allowed_users.is_empty() {
                    "open".to_string()
                } else {
                    config.discord.allowed_users.len().to_string()
                }
            ),
            hint: "Run `academic-agent discord register-commands` once if commands are not yet visible.".to_string(),
        });
    } else if has_bot || has_webhook {
        checks.push(DoctorCheck {
            name: "discord".to_string(),
            status: CheckStatus::Pass,
            summary: if has_bot {
                "Discord Bot REST API configured (set DISCORD_CLIENT_ID for slash commands)"
                    .to_string()
            } else {
                "Discord Webhook configured".to_string()
            },
            hint: "Verify delivery with `academic-agent test-discord`.".to_string(),
        });
    } else {
        checks.push(DoctorCheck {
            name: "discord".to_string(),
            status: CheckStatus::Warn,
            summary:
                "Discord enabled but DISCORD_BOT_TOKEN/CHANNEL_ID or DISCORD_WEBHOOK_URL missing"
                    .to_string(),
            hint: "Configure Discord secrets in .env or switch [discord].enabled = false."
                .to_string(),
        });
    }

    // 6. Cron scheduler
    if config.cron.enabled {
        checks.push(DoctorCheck {
            name: "cron".to_string(),
            status: CheckStatus::Pass,
            summary: format!(
                "Daily digest '{}', reminders {:?}h before deadline",
                config.cron.digest_cron, config.cron.reminder_hours
            ),
            hint: "Start with `academic-agent serve --with-cron` to keep reminders running."
                .to_string(),
        });
    } else {
        checks.push(DoctorCheck {
            name: "cron".to_string(),
            status: CheckStatus::Warn,
            summary: "Cron reminder scheduler is disabled".to_string(),
            hint: "Set [cron].enabled = true to automate deadline alerts.".to_string(),
        });
    }

    // 7. Native LMS sources
    let enabled_sources: Vec<_> = config.lms.sources.iter().filter(|s| s.enabled).collect();
    if enabled_sources.is_empty() {
        checks.push(DoctorCheck {
            name: "lms_sources".to_string(),
            status: CheckStatus::Warn,
            summary: "No enabled campus LMS sources configured".to_string(),
            hint: "Paste a Moodle/Canvas/Google Classroom .ics URL in Connect LMS or run `academic-agent setup`.".to_string(),
        });
    } else {
        let mut details = Vec::new();
        for src in &enabled_sources {
            let key = format!("src:{}", safe_slug(&src.name, 32));
            let count = store
                .get_lms_sync_meta(&key)
                .ok()
                .flatten()
                .map(|m| m.last_event_count)
                .unwrap_or(0);
            details.push(format!(
                "{} ({}, url={}, cached_events={})",
                src.name,
                src.kind.as_str(),
                redact_url_secrets(&src.url),
                count
            ));
        }
        checks.push(DoctorCheck {
            name: "lms_sources".to_string(),
            status: CheckStatus::Pass,
            summary: format!(
                "{} enabled LMS source(s): {}",
                enabled_sources.len(),
                details.join("; ")
            ),
            hint: "Run `academic-agent lms sync` or `serve --with-lms-sync` to pull updates."
                .to_string(),
        });
    }

    // 8. Calendar output dir & feed readiness
    if config.general.calendar_dir.exists() {
        let auth_mode = if is_configured_secret(&config.server.api_token) {
            "Bearer token or ?token= protected"
        } else {
            "open local access (set ACADEMIC_API_TOKEN before exposing publicly)"
        };
        checks.push(DoctorCheck {
            name: "calendar_feed".to_string(),
            status: CheckStatus::Pass,
            summary: format!(
                "Calendar dir '{}' ready; live feed /api/calendar/feed.ics ({auth_mode})",
                config.general.calendar_dir.display()
            ),
            hint: "OK".to_string(),
        });
    } else {
        checks.push(DoctorCheck {
            name: "calendar_feed".to_string(),
            status: CheckStatus::Fail,
            summary: format!(
                "Calendar directory '{}' is missing",
                config.general.calendar_dir.display()
            ),
            hint: "Create the directory or run any command once to initialize runtime folders."
                .to_string(),
        });
    }

    let ok = !checks.iter().any(|c| c.status == CheckStatus::Fail);
    DoctorReport {
        ok,
        version: env!("CARGO_PKG_VERSION").to_string(),
        timezone: config.general.timezone.clone(),
        checks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{LmsSourceConfig, LmsSourceKind};
    use tempfile::tempdir;

    #[test]
    fn test_doctor_report_summarizes_system_health() {
        let dir = tempdir().unwrap();
        let cfg_path = dir.path().join("academic-agent.toml");
        std::fs::write(&cfg_path, Config::starter_toml()).unwrap();

        let mut cfg = Config::default();
        cfg.resolve_paths_relative_to(dir.path());
        cfg.ensure_runtime_dirs().unwrap();
        cfg.lms.sources.push(LmsSourceConfig {
            name: "moodle-test".to_string(),
            kind: LmsSourceKind::IcsFeed,
            url: "https://lms.example.edu/cal.php?authtoken=secretval".to_string(),
            token: String::new(),
            default_course: "Campus LMS".to_string(),
            enabled: true,
        });

        let store = TaskStore::open(&cfg.general.db_path).unwrap();
        let report = run_doctor(&cfg_path, &cfg, &store);
        assert!(report.ok);
        assert_eq!(report.checks.len(), 8);

        let lms_check = report
            .checks
            .iter()
            .find(|c| c.name == "lms_sources")
            .unwrap();
        assert_eq!(lms_check.status, CheckStatus::Pass);
        assert!(!lms_check.summary.contains("secretval"));
        assert!(lms_check
            .summary
            .contains("https://lms.example.edu/cal.php?<redacted>"));
    }
}
