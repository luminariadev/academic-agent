mod server;
mod watcher;

use std::path::{Path, PathBuf};

use academic_core::config::{load_dotenv_if_present, Config};
use academic_core::cron_engine::run_cron_tick;
use academic_core::discord_bot::{register_slash_commands, run_discord_gateway};
use academic_core::extract::extract_from_text;
use academic_core::notify::{build_task_embed, send_discord_embed, send_via_hermes};
use academic_core::store::{ExportFormat, TaskStatus, TaskStore};
use academic_core::{create_ics_event, AcademicPipeline, CalendarEventSpec};
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
    },
    /// Start the Model Context Protocol server (stdio by default, or Streamable HTTP with --http)
    Mcp {
        #[arg(long)]
        http: Option<String>,
        #[arg(long)]
        token: Option<String>,
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
        Commands::Serve {
            bind,
            token,
            with_cron,
            with_watch,
            with_discord_bot,
        } => {
            let cfg = load_config(&cli.config, cli.db.as_deref())?;
            server::run_serve(
                cli.config,
                cfg,
                bind,
                token,
                with_cron,
                with_watch,
                with_discord_bot,
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
    }
}
