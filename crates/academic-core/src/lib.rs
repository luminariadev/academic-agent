pub mod calendar;
pub mod config;
pub mod cron_engine;
pub mod discord_bot;
pub mod error;
pub mod extract;
pub mod notify;
pub mod parser;
pub mod pipeline;
pub mod store;

pub use calendar::{
    create_ics_event, filter_tasks_for_feed, generate_google_calendar_url, generate_ics_feed,
    parse_deadline_in_tz, CalendarArtifact, CalendarEventSpec,
};
pub use config::{
    expand_env_vars_with, load_dotenv_if_present, normalize_cron_expr, parse_cron_schedule,
    redact_config_toml, Config, CronConfig, DiscordConfig, DiscordMode, GeneralConfig,
    HermesConfig, LlmConfig, N8nConfig, ServerConfig,
};
pub use cron_engine::{
    evaluate_due_reminders, is_digest_due, run_cron_tick, CronTickReport, DueReminder, ReminderKind,
};
pub use discord_bot::{
    handle_discord_interaction, is_user_allowed, register_slash_commands, run_discord_gateway,
    slash_command_definitions,
};
pub use error::{Error, Result};
pub use extract::{extract_from_text, heuristic_extract, Priority, TaskExtraction, TaskKind};
pub use notify::{
    build_digest_embed, build_reminder_embed, build_task_action_components, build_task_embed,
    dispatch_new_task, send_discord_embed, send_discord_message_with_components, send_n8n_event,
    send_via_hermes, DispatchReceipt,
};
pub use parser::{parse_document, safe_join_within, sanitize_filename, sha256_hex, ParsedDocument};
pub use pipeline::{
    verify_hmac_sha256, AcademicPipeline, DigestResult, IngestResult, WebdeltaWebhookPayload,
};
pub use store::{ExportFormat, InsertOutcome, NewTask, TaskRecord, TaskStatus, TaskStore};
