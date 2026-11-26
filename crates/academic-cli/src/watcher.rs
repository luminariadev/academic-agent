use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use academic_core::{AcademicPipeline, Config, TaskStore};
use notify::event::ModifyKind;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::{mpsc, watch};
use tracing::{error, info, warn};

const SUPPORTED_EXTS: &[&str] = &["pdf", "docx", "txt", "md"];

fn is_candidate_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if name.starts_with('.') || name.starts_with("~$") {
        return false;
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_default();
    SUPPORTED_EXTS.contains(&ext.as_str())
}

/// Watches `config.general.inbox_dir` for newly created or moved/renamed documents
/// until `shutdown_rx` becomes `true`.
pub async fn run_watcher_loop(
    config: Config,
    store: TaskStore,
    mut shutdown_rx: watch::Receiver<bool>,
) -> academic_core::Result<()> {
    let inbox_dir = config.general.inbox_dir.clone();
    std::fs::create_dir_all(&inbox_dir)?;

    let (tx, mut rx) = mpsc::unbounded_channel::<PathBuf>();
    let mut watcher: RecommendedWatcher =
        notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res {
                let relevant = matches!(
                    event.kind,
                    EventKind::Create(_) | EventKind::Modify(ModifyKind::Name(_))
                );
                if relevant {
                    for path in event.paths {
                        let _ = tx.send(path);
                    }
                }
            }
        })
        .map_err(|e| academic_core::Error::Io(std::io::Error::other(e.to_string())))?;

    watcher
        .watch(&inbox_dir, RecursiveMode::NonRecursive)
        .map_err(|e| academic_core::Error::Io(std::io::Error::other(e.to_string())))?;

    info!("Watching inbox folder: {}", inbox_dir.display());

    let mut recent_paths: HashSet<PathBuf> = HashSet::new();

    loop {
        tokio::select! {
            biased;
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    info!("Folder watcher shutting down cleanly.");
                    break;
                }
            }
            maybe_path = rx.recv() => {
                let Some(path) = maybe_path else { break; };
                if recent_paths.contains(&path) {
                    continue;
                }
                // Wait briefly for browser/editor writes to finish flushing to disk
                tokio::time::sleep(Duration::from_millis(400)).await;
                if !is_candidate_file(&path) {
                    continue;
                }
                recent_paths.insert(path.clone());
                if recent_paths.len() > 512 {
                    recent_paths.clear();
                }

                let pipeline = AcademicPipeline::new(&config, &store);
                match pipeline.ingest_file(&path, false, true).await {
                    Ok(res) => {
                        if res.duplicate {
                            info!("Skipped duplicate document '{}' (existing task #{})", path.display(), res.task.id);
                        } else {
                            info!(
                                "Ingested '{}' -> task #{} [{}] {}",
                                path.display(),
                                res.task.id,
                                res.task.course,
                                res.task.title
                            );
                        }
                    }
                    Err(e) => {
                        warn!("Failed ingesting '{}': {e}", path.display());
                    }
                }
            }
        }
    }

    if let Err(e) = watcher.unwatch(&inbox_dir) {
        error!("Failed unwatching '{}': {e}", inbox_dir.display());
    }
    Ok(())
}
