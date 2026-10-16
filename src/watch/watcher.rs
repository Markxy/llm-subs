use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use notify::{EventKind, RecursiveMode};
use notify::event::{CreateKind, ModifyKind, RenameMode};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, RecommendedCache};
use parking_lot::Mutex;
use tokio::sync::mpsc;

use crate::config::WatchConfig;
use crate::media::{classify_path, MediaKind};

use super::readiness;

/// A file that appeared in a watched folder and has finished being written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchEvent {
    pub path: PathBuf,
    pub kind: MediaKind,
}

/// A file in a watched folder that is not finished yet, so it is not queued yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingFile {
    pub path: PathBuf,
    pub since: DateTime<Utc>,
    pub size: u64,
    pub state: PendingState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingState {
    /// Just noticed; not looked at twice yet.
    Checking,
    /// Its size or modification time changed since the last look.
    Growing,
    /// Unchanged so far; waiting out the settle time.
    Settling,
    /// Unchanged, but something shows it is not complete, e.g. a paused download.
    Unfinished(&'static str),
}

impl PendingState {
    pub fn describe(self) -> String {
        let text = match self {
            PendingState::Checking => "checking the file",
            PendingState::Growing => "still being written or downloaded",
            PendingState::Settling => "making sure it has finished",
            PendingState::Unfinished(reason) => reason,
        };
        let mut chars = text.chars();
        chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
    }
}

type PendingFiles = Arc<Mutex<HashMap<PathBuf, PendingFile>>>;
type ChangeListener = Arc<dyn Fn() + Send + Sync>;

/// Debounce window applied on top of the OS notifications before a file is checked.
const DEBOUNCE: Duration = Duration::from_secs(2);
/// How long to keep waiting for a file that is still being written or downloaded.
const MAX_SETTLE_WAIT: Duration = Duration::from_secs(3 * 24 * 60 * 60);
/// How often a file that is still being written or downloaded is looked at again.
const UNFINISHED_RECHECK: Duration = Duration::from_secs(10);

pub struct FolderWatcher {
    debouncer: Debouncer<notify::RecommendedWatcher, RecommendedCache>,
    watched: HashMap<PathBuf, bool>,
    pending: PendingFiles,
    on_change: ChangeListener,
    watch_config: Arc<Mutex<WatchConfig>>,
    runtime: tokio::runtime::Handle,
    events: mpsc::UnboundedSender<WatchEvent>,
}

impl FolderWatcher {
    /// Starts watching; settled files are delivered through `events`. `on_change` is
    /// called whenever a file starts or stops waiting, or its state changes.
    pub fn start(
        watch_config: WatchConfig,
        runtime: tokio::runtime::Handle,
        events: mpsc::UnboundedSender<WatchEvent>,
        on_change: ChangeListener,
    ) -> Result<Self> {
        let pending: PendingFiles = Arc::new(Mutex::new(HashMap::new()));
        let shared_config = Arc::new(Mutex::new(watch_config.clone()));
        let handler = {
            let pending = pending.clone();
            let on_change = on_change.clone();
            let config = shared_config.clone();
            let runtime = runtime.clone();
            let events = events.clone();
            move |result: DebounceEventResult| match result {
                Ok(debounced) => {
                    for event in debounced {
                        for path in candidate_paths(&event.event) {
                            schedule_settle_check(path, &config, &pending, &on_change, &runtime, &events);
                        }
                    }
                }
                Err(errors) => {
                    for error in errors {
                        tracing::warn!(%error, "file watcher error");
                    }
                }
            }
        };
        let debouncer = new_debouncer(DEBOUNCE, None, handler).context("starting file watcher")?;
        let mut watcher = Self { debouncer, watched: HashMap::new(), pending, on_change, watch_config: shared_config, runtime, events };
        watcher.apply(&watch_config);
        Ok(watcher)
    }

    /// Brings the set of watched folders in line with the configuration.
    pub fn apply(&mut self, config: &WatchConfig) {
        *self.watch_config.lock() = config.clone();
        let wanted: HashMap<PathBuf, bool> = if config.enabled {
            config.folders.iter().filter(|f| !f.path.as_os_str().is_empty()).map(|f| (f.path.clone(), f.recursive)).collect()
        } else {
            HashMap::new()
        };
        let to_remove: Vec<PathBuf> = self.watched.keys().filter(|p| wanted.get(*p) != self.watched.get(*p)).cloned().collect();
        for path in to_remove {
            if let Err(error) = self.debouncer.unwatch(&path) {
                tracing::debug!(path = %path.display(), %error, "unwatch failed");
            }
            self.watched.remove(&path);
        }
        for (path, recursive) in wanted {
            if self.watched.contains_key(&path) {
                continue;
            }
            let mode = if recursive { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive };
            match self.debouncer.watch(&path, mode) {
                Ok(()) => {
                    tracing::info!(path = %path.display(), recursive, "watching folder");
                    self.watched.insert(path, recursive);
                }
                Err(error) => tracing::warn!(path = %path.display(), %error, "cannot watch folder"),
            }
        }
    }

    pub fn watched_folders(&self) -> Vec<PathBuf> {
        self.watched.keys().cloned().collect()
    }

    /// Queues a settle check for a path found by the initial scan.
    pub fn consider(&self, path: PathBuf) {
        schedule_settle_check(path, &self.watch_config, &self.pending, &self.on_change, &self.runtime, &self.events);
    }

    /// Files that are still being checked, oldest first.
    pub fn pending(&self) -> Vec<PendingFile> {
        let mut files: Vec<PendingFile> = self.pending.lock().values().cloned().collect();
        files.sort_by(|a, b| a.since.cmp(&b.since).then_with(|| a.path.cmp(&b.path)));
        files
    }
}

fn candidate_paths(event: &notify::Event) -> Vec<PathBuf> {
    match &event.kind {
        EventKind::Create(CreateKind::File) | EventKind::Create(CreateKind::Any) => event.paths.clone(),
        EventKind::Modify(ModifyKind::Name(RenameMode::To)) => event.paths.clone(),
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => event.paths.get(1).cloned().into_iter().collect(),
        EventKind::Modify(ModifyKind::Data(_)) | EventKind::Modify(ModifyKind::Any) => event.paths.clone(),
        _ => Vec::new(),
    }
}

fn schedule_settle_check(
    path: PathBuf,
    config: &Arc<Mutex<WatchConfig>>,
    pending: &PendingFiles,
    on_change: &ChangeListener,
    runtime: &tokio::runtime::Handle,
    events: &mpsc::UnboundedSender<WatchEvent>,
) {
    let (kind, settle_seconds) = {
        let config = config.lock();
        if crate::media::kinds::looks_like_incomplete_file(&path) {
            return;
        }
        let Some(kind) = classify_path(&path, &config) else { return };
        (kind, config.settle_seconds.max(1))
    };
    {
        let mut pending = pending.lock();
        if pending.contains_key(&path) {
            return;
        }
        pending.insert(path.clone(), PendingFile { path: path.clone(), since: Utc::now(), size: 0, state: PendingState::Checking });
    }
    on_change();
    let pending = pending.clone();
    let on_change = on_change.clone();
    let events = events.clone();
    runtime.spawn(async move {
        let report = |state: PendingState, size: u64| {
            let changed = match pending.lock().get_mut(&path) {
                Some(file) => {
                    file.size = size;
                    std::mem::replace(&mut file.state, state) != state
                }
                None => false,
            };
            if changed {
                on_change();
            }
        };
        let settled = wait_until_settled(&path, Duration::from_secs(settle_seconds), &report).await;
        pending.lock().remove(&path);
        on_change();
        if settled {
            let _ = events.send(WatchEvent { path, kind });
        }
    });
}

/// True once the file exists, its size and modification time have not changed for
/// `quiet_period`, no other program has it open for writing and no part of it is left
/// unwritten. The last two catch torrent downloads that stall or are paused.
async fn wait_until_settled(path: &Path, quiet_period: Duration, report: &(dyn Fn(PendingState, u64) + Sync)) -> bool {
    let started = std::time::Instant::now();
    let poll = Duration::from_secs(1).min(quiet_period);
    let mut last_state = None;
    let mut stable_for = Duration::ZERO;
    let mut reported_wait = None;
    while started.elapsed() < MAX_SETTLE_WAIT {
        tokio::time::sleep(poll).await;
        let Ok(metadata) = tokio::fs::metadata(path).await else {
            if started.elapsed() > Duration::from_secs(30) {
                return false;
            }
            continue;
        };
        if !metadata.is_file() {
            return false;
        }
        let state = (metadata.len(), metadata.modified().ok());
        if last_state != Some(state) {
            report(if last_state.is_some() { PendingState::Growing } else { PendingState::Checking }, metadata.len());
            last_state = Some(state);
            stable_for = Duration::ZERO;
            continue;
        }
        stable_for += poll;
        if stable_for < quiet_period || metadata.len() == 0 {
            report(PendingState::Settling, metadata.len());
            continue;
        }
        let Some(reason) = why_unfinished(path).await else { return true };
        report(PendingState::Unfinished(reason), metadata.len());
        if reported_wait != Some(reason) {
            tracing::info!(path = %path.display(), "waiting: {reason}");
            reported_wait = Some(reason);
        }
        tokio::time::sleep(UNFINISHED_RECHECK).await;
    }
    false
}

/// Why a file that has stopped changing is still not ready, if it is not.
async fn why_unfinished(path: &Path) -> Option<&'static str> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        if readiness::is_open_for_writing(&path) {
            return Some("another program is still writing the file");
        }
        match readiness::has_unwritten_gaps(&path) {
            Ok(false) => None,
            Ok(true) => Some("parts of the file have not been downloaded yet"),
            Err(_) => Some("the file cannot be read yet"),
        }
    })
    .await
    .unwrap_or(Some("the file cannot be checked"))
}
