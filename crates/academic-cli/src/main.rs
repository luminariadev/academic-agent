mod server;
mod watcher;

use std::io::Write;
use std::path::{Path, PathBuf};

use academic_core::config::{load_dotenv_if_present, redact_url_secrets, Config, LmsSourceKind};
use academic_core::cron_engine::run_cron_tick;
use academic_core::discord_bot::{register_slash_commands, run_discord_gateway};
use academic_core::doctor::{run_doctor, CheckStatus};
use academic_core::extract::extract_from_text;
use academic_core::lms_sync::{sync_all_lms_sources, sync_lms_source};
use academic_core::notify::{build_task_embed, send_discord_embed, send_via_hermes};
use academic_core::store::{ExportFormat, TaskStatus, TaskStore};
use academic_core::{
    create_ics_event, filter_tasks_for_feed, generate_ics_feed, AcademicPipeline,
    CalendarEventSpec, LmsSourceConfig,
};
use academic_mcp::McpServer;
use chrono::{Duration, Utc};
use clap::{Parser, Subcommand};
use serde_json::json;
use tokio::sync::watch;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "academic-agent",
    version,
    about = "Single-binary academic task extractor, calendar sync, cron reminder, Discord notifier, and MCP server"
)]
struct Cli {
    /// Path to TOML configuration file
    #[arg(long, global = true, default_value = "academic-agent.toml")]
    config: PathBuf,

    /// Optional override path for the redb database file
    #[arg(long, global = true)]
    db: Option<PathBuf>,

    /// Emit machine-readable JSON output
    #[arg(long, global = true)]
    json: bool,

    /// Enable scoped debug logs to stderr
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a starter academic-agent.toml configuration file
    Init {
        #[arg(long)]
        force: bool,
    },
    /// Validate configuration file syntax, timezone, cron expression, and URLs
    Validate,
    /// Parse, extract, deduplicate, generate .ics, and optionally notify for a document
    Scan {
        file: PathBuf,
        #[arg(long)]
        heuristic: bool,
        #[arg(long)]
        notify: bool,
    },
    /// Watch the configured inbox folder for incoming academic documents
    Watch,
    /// List or export tracked tasks
    Tasks {
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        export: Option<String>,
    },
    /// List incomplete tasks due within the next N hours
    Upcoming {
        #[arg(long, default_value_t = 72)]
        hours: i64,
    },
    /// Mark a task as COMPLETED
    Done { task_id: u64 },
    /// Update a task status (PENDING, IN_PROGRESS, COMPLETED, ARCHIVED)
    Status { task_id: u64, status: String },
    /// Snooze scheduled deadline alerts for a task by N hours
    Snooze {
        task_id: u64,
        #[arg(long, default_value_t = 24)]
        hours: i64,
    },
    /// Delete a task by ID
    Delete { task_id: u64 },
    /// Preview or send a digest of open academic tasks
    Digest {
        /// Actually dispatch notifications instead of dry-run preview
        #[arg(long)]
        send: bool,
    },
    /// Evaluate due deadline reminders and daily digest once
    CronTick {
        #[arg(long)]
        dry_run: bool,
    },
    /// Test extraction using configured LLM gateway (or heuristic fallback)
    TestAi {
        #[arg(long)]
        text: Option<String>,
        #[arg(long)]
        file: Option<PathBuf>,
        #[arg(long)]
        heuristic: bool,
    },
    /// Send a synthetic test notification via configured Discord or Hermes transport
    TestDiscord,
    /// Manage interactive Discord slash commands and the persistent Gateway bot
    Discord {
        #[command(subcommand)]
        action: DiscordCommands,
    },
    /// Inspect system health (config, redb store, timezone, LLM/heuristic, Discord, LMS, feeds)
    Doctor,
    /// Interactive or flagged setup wizard generating academic-agent.toml and .env.example
    Setup {
        #[arg(long)]
        non_interactive: bool,
        #[arg(long, default_value = "student")]
        role: String,
        #[arg(long, default_value = "Asia/Jakarta")]
        timezone: String,
        #[arg(long, default_value = "ics_feed")]
        lms_kind: String,
        #[arg(long)]
        lms_url: Option<String>,
        #[arg(long)]
        discord_webhook: Option<String>,
        #[arg(long)]
        force: bool,
    },
    /// Manage and synchronize native campus LMS sources (.ics feeds, Moodle REST, Canvas REST)
    Lms {
        #[command(subcommand)]
        action: LmsCommands,
    },
    /// Start the web dashboard, REST API, webdelta webhook receiver, and /mcp server
    Serve {
        #[arg(long)]
        bind: Option<String>,
        #[arg(long)]
        token: Option<String>,
        #[arg(long)]
        with_cron: bool,
        #[arg(long)]
        with_watch: bool,
        #[arg(long)]
        with_discord_bot: bool,
        #[arg(long)]
        with_lms_sync: bool,
        #[arg(long)]
        init_if_missing: bool,
    },
    /// Start the Model Context Protocol server (stdio by default, or Streamable HTTP with --http)
    Mcp {
        #[arg(long)]
        http: Option<String>,
        #[arg(long)]
        token: Option<String>,
    },
    /// Export an RFC 5545 iCalendar (.ics) subscription feed for tracked tasks
    CalendarFeed {
        #[arg(long)]
        include_completed: bool,
        #[arg(long)]
        course: Option<String>,
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum DiscordCommands {
    /// Register slash commands (/tasks, /upcoming, /done, /snooze, /add, /digest) with Discord
    RegisterCommands {
        #[arg(long)]
        guild: Option<String>,
    },
    /// Start the interactive Discord Gateway WebSocket listener
    Bot,
}

#[derive(Subcommand)]
enum LmsCommands {
    /// List configured campus LMS sources and cached sync counts (secrets redacted)
    List,
    /// Pull deadlines from configured [lms.sources] or an ad-hoc .ics/Moodle/Canvas URL
    Sync {
        #[arg(long)]
        source: Option<String>,
        #[arg(long)]
        url: Option<String>,
        #[arg(long, default_value = "ics_feed")]
        kind: String,
        #[arg(long)]
        token: Option<String>,
        #[arg(long)]
        course: Option<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        notify: bool,
    },
}

fn prompt_with_default(label: &str, default: &str) -> String {
    if default.is_empty() {
        print!("{label}: ");
    } else {
        print!("{label} [{default}]: ");
    }
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_ok() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    default.to_string()
}

fn init_logging(verbose: bool) {
    let filter_str = if verbose {
        "warn,academic_core=debug,academic_mcp=debug,academic_agent=debug"
    } else {
        "warn,academic_agent=info"
    };
    let filter = EnvFilter::try_new(filter_str).unwrap_or_else(|_| EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

fn load_config(path: &Path, db_override: Option<&Path>) -> academic_core::Result<Config> {
    let _ = load_dotenv_if_present(Path::new(".env"));
    let mut cfg = Config::load(path)?;
    if let Some(db) = db_override {
        cfg.general.db_path = db.to_path_buf();
    }
    cfg.ensure_runtime_dirs()?;
    Ok(cfg)
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    init_logging(cli.verbose);

    if let Err(err) = run(cli).await {
        eprintln!("Error: {err}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> academic_core::Result<()> {
    match cli.command {
        Commands::Init { force } => {
            if cli.config.exists() && !force {
                return Err(academic_core::Error::Config(format!(
                    "'{}' already exists; pass --force to overwrite",
                    cli.config.display()
                )));
            }
            if let Some(parent) = cli.config.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)?;
                }
            }
            std::fs::write(&cli.config, Config::starter_toml())?;
            if cli.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "initialized": cli.config.display().to_string()
                    }))?
                );
            } else {
                println!("Wrote starter configuration to {}", cli.config.display());
            }
            Ok(())
        }
        Commands::Validate => {
            let cfg = load_config(&cli.config, cli.db.as_deref())?;
            if cli.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "valid": true,
                        "timezone": cfg.general.timezone,
                        "digest_cron": cfg.cron.digest_cron,
                        "discord_enabled": cfg.discord.enabled,
                        "hermes_enabled": cfg.hermes.enabled
                    }))?
                );
            } else {
                println!(
                    "Valid configuration: timezone={}, digest_cron='{}', reminders={:?}",
                    cfg.general.timezone, cfg.cron.digest_cron, cfg.cron.reminder_hours
                );
            }
            Ok(())
        }
        Commands::Scan {
            file,
            heuristic,
            notify,
        } => {
            let cfg = load_config(&cli.config, cli.db.as_deref())?;
            let store = TaskStore::open(&cfg.general.db_path)?;
            let pipeline = AcademicPipeline::new(&cfg, &store);
            let res = pipeline.ingest_file(&file, heuristic, notify).await?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&res)?);
            } else {
                let tag = if res.duplicate {
                    "DUPLICATE"
                } else {
                    "INSERTED"
                };
                println!(
                    "[{tag}] #{} [{}] {} (deadline: {}, priority: {})",
                    res.task.id,
                    res.task.course,
                    res.task.title,
                    res.task.deadline_raw.as_deref().unwrap_or("unspecified"),
                    res.task.priority.as_str()
                );
                if let Some(cal) = res.calendar {
                    println!("  calendar: {}", cal.file_path.display());
                }
            }
            Ok(())
        }
        Commands::Watch => {
            let cfg = load_config(&cli.config, cli.db.as_deref())?;
            let store = TaskStore::open(&cfg.general.db_path)?;
            let (tx, rx) = watch::channel(false);
            let ctrl_tx = tx.clone();
            tokio::spawn(async move {
                let _ = tokio::signal::ctrl_c().await;
                let _ = ctrl_tx.send(true);
            });
            watcher::run_watcher_loop(cfg, store, rx).await
        }
        Commands::Tasks { status, export } => {
            let cfg = load_config(&cli.config, cli.db.as_deref())?;
            let store = TaskStore::open(&cfg.general.db_path)?;
            let filter = match status.as_deref() {
                Some(s) => Some(TaskStatus::parse_loose(s)?),
                None => None,
            };
            if let Some(fmt_str) = export {
                let fmt = ExportFormat::parse_loose(&fmt_str)?;
                println!("{}", store.export_tasks(fmt, filter)?);
                return Ok(());
            }
            let tasks = store.list_tasks(filter)?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&tasks)?);
            } else {
                println!("Total tasks: {}", tasks.len());
                for t in tasks {
                    println!(
                        "#{} [{}] [{}] {} | due={} | {}",
                        t.id,
                        t.status.as_str(),
                        t.priority.as_str(),
                        t.title,
                        t.deadline_raw.as_deref().unwrap_or("none"),
                        t.course
                    );
                }
            }
            Ok(())
        }
        Commands::Upcoming { hours } => {
            let cfg = load_config(&cli.config, cli.db.as_deref())?;
            let store = TaskStore::open(&cfg.general.db_path)?;
            let tasks = store.upcoming_tasks(hours.max(1), Utc::now())?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&tasks)?);
            } else {
                println!("Upcoming tasks within {hours}h: {}", tasks.len());
                for t in tasks {
                    println!(
                        "#{} [{}] {} ({})",
                        t.id,
                        t.course,
                        t.title,
                        t.deadline_raw.as_deref().unwrap_or("none")
                    );
                }
            }
            Ok(())
        }
        Commands::Done { task_id } => {
            let cfg = load_config(&cli.config, cli.db.as_deref())?;
            let store = TaskStore::open(&cfg.general.db_path)?;
            let updated = store.update_status(task_id, TaskStatus::Completed)?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&updated)?);
            } else {
                println!("Task #{} marked as COMPLETED.", updated.id);
            }
            Ok(())
        }
        Commands::Status { task_id, status } => {
            let cfg = load_config(&cli.config, cli.db.as_deref())?;
            let store = TaskStore::open(&cfg.general.db_path)?;
            let parsed = TaskStatus::parse_loose(&status)?;
            let updated = store.update_status(task_id, parsed)?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&updated)?);
            } else {
                println!(
                    "Task #{} status updated to {}.",
                    updated.id,
                    updated.status.as_str()
                );
            }
            Ok(())
        }
        Commands::Snooze { task_id, hours } => {
            let cfg = load_config(&cli.config, cli.db.as_deref())?;
            let store = TaskStore::open(&cfg.general.db_path)?;
            let until = Utc::now() + Duration::hours(hours.max(1));
            let updated = store.snooze_task(task_id, until)?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&updated)?);
            } else {
                println!(
                    "Task #{} snoozed for {hours}h until {}.",
                    updated.id,
                    until.to_rfc3339()
                );
            }
            Ok(())
        }
        Commands::Delete { task_id } => {
            let cfg = load_config(&cli.config, cli.db.as_deref())?;
            let store = TaskStore::open(&cfg.general.db_path)?;
            let removed = store.delete_task(task_id)?;
            if !removed {
                return Err(academic_core::Error::NotFound(format!(
                    "task #{task_id} not found"
                )));
            }
            if cli.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({ "deleted": true, "id": task_id }))?
                );
            } else {
                println!("Deleted task #{task_id}.");
            }
            Ok(())
        }
        Commands::Digest { send } => {
            let cfg = load_config(&cli.config, cli.db.as_deref())?;
            let store = TaskStore::open(&cfg.general.db_path)?;
            let pipeline = AcademicPipeline::new(&cfg, &store);
            let res = pipeline.send_digest(Utc::now(), !send).await?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&res)?);
            } else {
                println!(
                    "Digest ({}): {} open task(s), {} dispatch receipt(s).",
                    if res.dry_run { "dry-run" } else { "sent" },
                    res.open_task_count,
                    res.receipts.len()
                );
            }
            Ok(())
        }
        Commands::CronTick { dry_run } => {
            let cfg = load_config(&cli.config, cli.db.as_deref())?;
            let store = TaskStore::open(&cfg.general.db_path)?;
            let report = run_cron_tick(&store, &cfg, Utc::now(), dry_run).await?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "Cron tick ({}): digest_due={}, digest_sent={}, reminders_due={}, reminders_sent={}",
                    if report.dry_run { "dry-run" } else { "live" },
                    report.digest_due,
                    report.digest_sent,
                    report.due_reminders.len(),
                    report.reminders_sent
                );
            }
            Ok(())
        }
        Commands::TestAi {
            text,
            file,
            heuristic,
        } => {
            let cfg = load_config(&cli.config, cli.db.as_deref())?;
            let tz = cfg.timezone_tz()?;
            let (content, fname) = if let Some(path) = file {
                let parsed = academic_core::parse_document(&path)?;
                (parsed.content, parsed.file_name)
            } else {
                let sample = text.unwrap_or_else(|| {
                    "Course: Distributed Systems\nTitle: Raft Consensus Lab\nDeadline: 2026-12-10 23:59\nFormat: PDF report and repository."
                        .to_string()
                });
                (sample, "sample_task.txt".to_string())
            };
            let extracted = extract_from_text(&cfg.llm, &content, &fname, tz, heuristic).await?;
            println!("{}", serde_json::to_string_pretty(&extracted)?);
            Ok(())
        }
        Commands::TestDiscord => {
            let cfg = load_config(&cli.config, cli.db.as_deref())?;
            let tz = cfg.timezone_tz()?;
            let sample = academic_core::TaskRecord {
                id: 0,
                title: "Connectivity Test Task".to_string(),
                course: "Academic Agent Check".to_string(),
                kind: academic_core::TaskKind::Announcement,
                deadline_raw: Some("2026-12-15 23:59".to_string()),
                deadline_utc: Some(Utc::now() + Duration::days(3)),
                priority: academic_core::Priority::Medium,
                status: TaskStatus::Pending,
                estimated_hours: 1,
                summary: "Synthetic test notification verifying Discord or Hermes configuration."
                    .to_string(),
                subtasks: vec!["Verify embed formatting".to_string()],
                subtasks_done: Vec::new(),
                deliverables: vec!["None".to_string()],
                ics_file: None,
                google_calendar_url: None,
                source_file: Some("test-discord".to_string()),
                content_hash: None,
                extractor: "test".to_string(),
                snoozed_until: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            };
            let cal = create_ics_event(CalendarEventSpec {
                calendar_dir: &cfg.general.calendar_dir,
                title: &sample.title,
                deadline_str: sample.deadline_raw.as_deref(),
                tz,
                course: &sample.course,
                description: &sample.summary,
                duration_hours: 1,
                priority: sample.priority,
            })?;

            let mut receipts = Vec::new();
            if cfg.discord.enabled {
                let embed = build_task_embed(&sample, Some(&cal.google_calendar_url), true);
                receipts.push(
                    send_discord_embed(&cfg.discord, embed, Some(&cal.google_calendar_url)).await?,
                );
            }
            if cfg.hermes.enabled {
                receipts
                    .push(send_via_hermes(&cfg.hermes, "academic-agent connectivity test").await?);
            }
            println!("{}", serde_json::to_string_pretty(&receipts)?);
            Ok(())
        }
        Commands::Discord { action } => {
            let cfg = load_config(&cli.config, cli.db.as_deref())?;
            match action {
                DiscordCommands::RegisterCommands { guild } => {
                    let count = register_slash_commands(&cfg.discord, guild.as_deref()).await?;
                    if cli.json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&json!({ "registered_commands": count }))?
                        );
                    } else {
                        println!("Registered {count} Discord slash command(s).");
                    }
                    Ok(())
                }
                DiscordCommands::Bot => {
                    let store = TaskStore::open(&cfg.general.db_path)?;
                    let (tx, rx) = watch::channel(false);
                    let ctrl_tx = tx.clone();
                    tokio::spawn(async move {
                        let _ = tokio::signal::ctrl_c().await;
                        let _ = ctrl_tx.send(true);
                    });
                    run_discord_gateway(cfg, store, rx).await
                }
            }
        }
        Commands::Doctor => {
            let _ = load_dotenv_if_present(Path::new(".env"));
            let mut cfg = if cli.config.exists() {
                Config::load(&cli.config)?
            } else {
                let mut d = Config::default();
                d.resolve_env_from_process();
                d.validate()?;
                d
            };
            if let Some(db) = cli.db.as_deref() {
                cfg.general.db_path = db.to_path_buf();
            }
            cfg.ensure_runtime_dirs()?;
            let store = TaskStore::open(&cfg.general.db_path)?;
            let report = run_doctor(&cli.config, &cfg, &store);
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "academic-agent doctor (v{}, timezone={})",
                    report.version, report.timezone
                );
                for check in &report.checks {
                    let badge = match check.status {
                        CheckStatus::Pass => "PASS",
                        CheckStatus::Warn => "WARN",
                        CheckStatus::Fail => "FAIL",
                    };
                    println!("[{badge}] {:<14} {}", check.name, check.summary);
                    if check.status != CheckStatus::Pass {
                        println!("       hint: {}", check.hint);
                    }
                }
            }
            Ok(())
        }
        Commands::Setup {
            non_interactive,
            role,
            timezone,
            lms_kind,
            lms_url,
            discord_webhook,
            force,
        } => {
            if cli.config.exists() && !force {
                return Err(academic_core::Error::Config(format!(
                    "'{}' already exists; pass --force to overwrite",
                    cli.config.display()
                )));
            }

            let (final_role, final_tz, final_kind, final_url, final_webhook) = if non_interactive {
                (
                    role,
                    timezone,
                    lms_kind,
                    lms_url.unwrap_or_default(),
                    discord_webhook.unwrap_or_default(),
                )
            } else {
                println!("academic-agent setup wizard (press Enter to accept defaults)");
                let r = prompt_with_default("Role (student/lecturer)", &role);
                let tz = prompt_with_default("Timezone (IANA, e.g. Asia/Jakarta)", &timezone);
                let lk = prompt_with_default(
                    "Campus LMS type (ics_feed/moodle_api/canvas_api/skip)",
                    &lms_kind,
                );
                let lu = if lk.eq_ignore_ascii_case("skip") {
                    String::new()
                } else {
                    prompt_with_default(
                        "LMS URL (leave blank to configure later in Connect LMS UI)",
                        lms_url.as_deref().unwrap_or(""),
                    )
                };
                let dw = prompt_with_default(
                    "Discord Webhook URL (optional, leave blank if using Bot env vars)",
                    discord_webhook.as_deref().unwrap_or(""),
                );
                (r, tz, lk, lu, dw)
            };

            let mut toml_out = Config::starter_toml().replace(
                "timezone = \"Asia/Jakarta\"",
                &format!("timezone = \"{}\"", final_tz.trim()),
            );
            let role_note = if final_role.eq_ignore_ascii_case("lecturer") {
                "# Role profile: lecturer (share /api/calendar/feed.ics?course=... with classes)\n"
            } else {
                "# Role profile: student\n"
            };
            toml_out = format!("{role_note}{toml_out}");

            if !final_webhook.trim().is_empty() {
                toml_out = toml_out.replace(
                    "webhook_url = \"${DISCORD_WEBHOOK_URL}\"",
                    &format!("webhook_url = \"{}\"", final_webhook.trim()),
                );
            }

            if !final_url.trim().is_empty() && !final_kind.eq_ignore_ascii_case("skip") {
                let parsed_kind = LmsSourceKind::parse_loose(&final_kind)?;
                toml_out = toml_out.replace("[lms]\nenabled = false", "[lms]\nenabled = true");
                toml_out.push_str(&format!(
                    "\n[[lms.sources]]\nname = \"campus-lms\"\nkind = \"{}\"\nurl = \"{}\"\ndefault_course = \"Campus LMS\"\nenabled = true\n",
                    parsed_kind.as_str(),
                    final_url.trim()
                ));
            }

            let parsed_cfg = Config::from_toml_str(&toml_out)?;
            parsed_cfg.validate()?;

            if let Some(parent) = cli.config.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)?;
                }
            }
            std::fs::write(&cli.config, &toml_out)?;

            let env_example_path = cli
                .config
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .map(|p| p.join(".env.example"))
                .unwrap_or_else(|| PathBuf::from(".env.example"));
            if !env_example_path.exists() {
                let sample_env = concat!(
                    "# Copy to .env and fill in any secrets you use\n",
                    "ACADEMIC_API_TOKEN=\n",
                    "DISCORD_BOT_TOKEN=\n",
                    "DISCORD_CLIENT_ID=\n",
                    "DISCORD_GUILD_ID=\n",
                    "DISCORD_CHANNEL_ID=\n",
                    "DISCORD_WEBHOOK_URL=\n",
                    "ACADEMIC_LLM_API_BASE=\n",
                    "ACADEMIC_LLM_API_KEY=\n",
                );
                std::fs::write(&env_example_path, sample_env)?;
            }

            if cli.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "configured": cli.config.display().to_string(),
                        "role": final_role,
                        "timezone": final_tz,
                        "lms_enabled": !final_url.trim().is_empty(),
                    }))?
                );
            } else {
                println!("Wrote configuration to {}", cli.config.display());
                println!(
                    "Next: run `academic-agent doctor` or `academic-agent serve --with-cron --with-lms-sync` and open http://127.0.0.1:8080"
                );
            }
            Ok(())
        }
        Commands::Lms { action } => {
            let cfg = load_config(&cli.config, cli.db.as_deref())?;
            let store = TaskStore::open(&cfg.general.db_path)?;
            match action {
                LmsCommands::List => {
                    let mut rows = Vec::with_capacity(cfg.lms.sources.len());
                    for src in &cfg.lms.sources {
                        let key =
                            format!("src:{}", academic_core::calendar::safe_slug(&src.name, 32));
                        let meta = store.get_lms_sync_meta(&key)?;
                        rows.push(json!({
                            "name": src.name,
                            "kind": src.kind.as_str(),
                            "url": redact_url_secrets(&src.url),
                            "default_course": src.default_course,
                            "enabled": src.enabled,
                            "meta": meta,
                        }));
                    }
                    if cli.json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&json!({ "sources": rows }))?
                        );
                    } else if rows.is_empty() {
                        println!("No [lms.sources] configured in {}", cli.config.display());
                    } else {
                        for r in &rows {
                            println!(
                                "{} [{}] enabled={} url={}",
                                r["name"].as_str().unwrap_or(""),
                                r["kind"].as_str().unwrap_or(""),
                                r["enabled"],
                                r["url"].as_str().unwrap_or("")
                            );
                        }
                    }
                    Ok(())
                }
                LmsCommands::Sync {
                    source,
                    url,
                    kind,
                    token,
                    course,
                    name,
                    notify,
                } => {
                    let notify_opt = if notify { Some(true) } else { None };
                    let reports = if let Some(raw_url) = url {
                        let parsed_kind = LmsSourceKind::parse_loose(&kind)?;
                        let src = LmsSourceConfig {
                            name: name.unwrap_or_else(|| "campus-lms".to_string()),
                            kind: parsed_kind,
                            url: raw_url,
                            token: token.unwrap_or_default(),
                            default_course: course.unwrap_or_else(|| "Campus LMS".to_string()),
                            enabled: true,
                        };
                        vec![sync_lms_source(&store, &cfg, &src, notify_opt).await?]
                    } else {
                        sync_all_lms_sources(&store, &cfg, source.as_deref(), notify_opt).await?
                    };

                    if cli.json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&json!({ "reports": reports }))?
                        );
                    } else if reports.is_empty() {
                        println!("No enabled LMS sources to synchronize.");
                    } else {
                        for r in &reports {
                            println!(
                                "[{}] kind={} not_modified={} fetched={} inserted={} updated={} unchanged={}",
                                r.source_name,
                                r.kind,
                                r.not_modified,
                                r.fetched_events,
                                r.inserted,
                                r.updated,
                                r.unchanged
                            );
                        }
                    }
                    Ok(())
                }
            }
        }
        Commands::Serve {
            bind,
            token,
            with_cron,
            with_watch,
            with_discord_bot,
            with_lms_sync,
            init_if_missing,
        } => {
            if init_if_missing && !cli.config.exists() {
                if let Some(parent) = cli.config.parent() {
                    if !parent.as_os_str().is_empty() {
                        std::fs::create_dir_all(parent)?;
                    }
                }
                std::fs::write(&cli.config, Config::starter_toml())?;
            }
            let cfg = load_config(&cli.config, cli.db.as_deref())?;
            server::run_serve(
                cli.config,
                cfg,
                bind,
                token,
                with_cron,
                with_watch,
                with_discord_bot,
                with_lms_sync,
            )
            .await
        }
        Commands::Mcp { http, token } => {
            let cfg = load_config(&cli.config, cli.db.as_deref())?;
            if let Some(bind) = http {
                academic_mcp::run_http(cli.config, cfg, &bind, token).await
            } else {
                let srv = McpServer::open(&cli.config, cfg)?;
                srv.run_stdio().await
            }
        }
        Commands::CalendarFeed {
            include_completed,
            course,
            output,
        } => {
            let cfg = load_config(&cli.config, cli.db.as_deref())?;
            let store = TaskStore::open(&cfg.general.db_path)?;
            let tasks = store.list_tasks(None)?;
            let filtered = filter_tasks_for_feed(&tasks, include_completed, course.as_deref());
            let tz = cfg.timezone_tz()?;
            let feed = generate_ics_feed(
                &filtered,
                tz,
                cfg.general.default_duration_hours,
                "academic-agent Deadlines",
            );
            if let Some(out_path) = output {
                if let Some(parent) = out_path.parent() {
                    if !parent.as_os_str().is_empty() {
                        std::fs::create_dir_all(parent)?;
                    }
                }
                std::fs::write(&out_path, &feed)?;
                if cli.json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&json!({
                            "written": out_path.display().to_string(),
                            "events": feed.matches("BEGIN:VEVENT").count(),
                        }))?
                    );
                } else {
                    println!(
                        "Wrote iCalendar feed ({} event(s)) to {}",
                        feed.matches("BEGIN:VEVENT").count(),
                        out_path.display()
                    );
                }
            } else {
                print!("{feed}");
            }
            Ok(())
        }
    }
}
