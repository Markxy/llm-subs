use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use parking_lot::{Mutex, RwLock};
use tokio::sync::mpsc;

use crate::config::{Config, ConfigStore, ProviderKind, ProviderProfile, WatchConfig};
use crate::ffmpeg::{self, DownloadProgress, FfmpegTools};
use crate::jobs::{JobHistory, JobId, JobInput, JobOptions, JobOrigin, JobQueue, QueueDependencies};
use crate::llm::chatgpt::{ChatGptAccounts, MANAGE_USAGE_URL};
use crate::llm::{LlmServices, UsageLedger};
use crate::media::{classify_path, video_for_subtitle, MediaKind};
use crate::paths::AppPaths;
use crate::watch::{scan_existing_files, FolderWatcher, PendingFile, WatchEvent};

#[derive(Debug, Clone)]
pub enum FfmpegStatus {
    Missing,
    Available { tools: FfmpegTools, version: Option<String> },
}

impl FfmpegStatus {
    pub fn tools(&self) -> Option<&FfmpegTools> {
        match self {
            FfmpegStatus::Available { tools, .. } => Some(tools),
            FfmpegStatus::Missing => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum DownloadStatus {
    Idle,
    Running(DownloadProgress),
    Finished,
    Failed(String),
}

#[derive(Debug, Clone)]
pub enum ProviderTestState {
    Running,
    Succeeded(String),
    Failed(String),
}

pub type ProviderTest = Arc<Mutex<ProviderTestState>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatGptSignIn {
    Connect,
    /// Ask again for permission to use the plan after the user declined it.
    EnableUsageSharing,
    /// Drop the previous account's registration so another account can connect.
    SwitchAccount,
}

#[derive(Debug, Clone, Copy)]
pub struct StartOptions {
    /// Watch folders and process new files. Off for one-shot command-line runs.
    pub enable_watching: bool,
}

#[derive(Default)]
struct UiNotifier(Mutex<Option<Arc<dyn Fn() + Send + Sync>>>);

impl UiNotifier {
    fn notify(&self) {
        if let Some(callback) = self.0.lock().clone() {
            callback();
        }
    }
}

pub struct AppRuntime {
    paths: AppPaths,
    config: ConfigStore,
    tokio: tokio::runtime::Runtime,
    jobs: Arc<JobQueue>,
    llm: LlmServices,
    watcher: Mutex<Option<FolderWatcher>>,
    ffmpeg: RwLock<FfmpegStatus>,
    download: Mutex<DownloadStatus>,
    notifier: Arc<UiNotifier>,
    watch_paused: AtomicBool,
    config_file_watcher: Mutex<Option<notify_debouncer_full::Debouncer<notify::RecommendedWatcher, notify_debouncer_full::RecommendedCache>>>,
}

impl AppRuntime {
    pub fn start(paths: AppPaths, options: StartOptions) -> Result<Arc<Self>> {
        crate::llm::install_crypto_provider();
        paths.ensure_directories().context("creating application directories")?;
        let config = ConfigStore::open(&paths.config_file())?;
        let tokio = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("llm-subs-worker")
            .enable_all()
            .build()
            .context("starting the async runtime")?;

        let notifier = Arc::new(UiNotifier::default());
        let managed_dir = paths.managed_ffmpeg_dir();
        let locate_tools: Arc<dyn Fn() -> Option<FfmpegTools> + Send + Sync> = {
            let config = config.clone();
            let managed_dir = managed_dir.clone();
            Arc::new(move || ffmpeg::locate(&config.current().ffmpeg, &managed_dir))
        };
        let on_change: Arc<dyn Fn() + Send + Sync> = {
            let notifier = notifier.clone();
            Arc::new(move || notifier.notify())
        };
        let llm = LlmServices { chatgpt: ChatGptAccounts::open(&paths.chatgpt_auth_file()), usage: UsageLedger::open(&paths.usage_file()) };
        llm.chatgpt.set_change_listener(on_change.clone());
        llm.usage.set_change_listener(on_change.clone());
        let history = JobHistory::load(&paths.history_file(), config.current().general.history_limit);
        let jobs = JobQueue::start(
            QueueDependencies { config: config.clone(), llm: llm.clone(), locate_tools, on_change },
            history,
            tokio.handle(),
        );

        let runtime = Arc::new(Self {
            ffmpeg: RwLock::new(match ffmpeg::locate(&config.current().ffmpeg, &managed_dir) {
                Some(tools) => FfmpegStatus::Available { tools, version: None },
                None => FfmpegStatus::Missing,
            }),
            paths,
            config,
            tokio,
            jobs,
            llm,
            watcher: Mutex::new(None),
            download: Mutex::new(DownloadStatus::Idle),
            notifier,
            watch_paused: AtomicBool::new(false),
            config_file_watcher: Mutex::new(None),
        });
        runtime.fetch_ffmpeg_version();
        runtime.start_config_file_watcher();
        runtime.start_config_change_listener();
        if options.enable_watching
            && let Err(error) = runtime.start_folder_watching()
        {
            tracing::warn!(%error, "folder watching is unavailable; files can still be added manually");
        }
        Ok(runtime)
    }

    pub fn paths(&self) -> &AppPaths {
        &self.paths
    }

    pub fn config(&self) -> Arc<Config> {
        self.config.current()
    }

    pub fn jobs(&self) -> &Arc<JobQueue> {
        &self.jobs
    }

    pub fn llm(&self) -> &LlmServices {
        &self.llm
    }

    /// Everything that keeps `config` from translating, including ChatGPT sign-ins.
    pub fn validation_problems(&self, config: &Config) -> Vec<String> {
        let mut problems = config.validation_problems();
        if let Some(provider) = config.provider(&config.models.translation.provider)
            && provider.kind == ProviderKind::ChatGpt
        {
            match self.llm.chatgpt.account(&provider.name) {
                None => problems.push(format!("Provider '{}' is not signed in to ChatGPT.", provider.name)),
                Some(account) if !account.usage_sharing => {
                    problems.push(format!("Provider '{}' is signed in, but usage sharing with LLM Subs is turned off.", provider.name))
                }
                Some(_) => {}
            }
        }
        problems
    }

    /// Starts the browser sign-in for a ChatGPT provider; progress shows in `AccountView`.
    pub fn chatgpt_sign_in(&self, provider: &str, kind: ChatGptSignIn) {
        let accounts = self.llm.chatgpt.clone();
        let provider = provider.to_string();
        self.tokio.spawn(async move {
            if kind == ChatGptSignIn::SwitchAccount
                && let Err(error) = accounts.forget(&provider).await
            {
                tracing::warn!(error = %format!("{error:#}"), "could not forget the previous ChatGPT account");
            }
            let open_browser = |url: &str| {
                if let Err(error) = open::that_detached(url) {
                    tracing::warn!(%error, "could not open the browser for the ChatGPT sign-in");
                }
            };
            match accounts.sign_in(&provider, kind == ChatGptSignIn::EnableUsageSharing, open_browser).await {
                Ok(_) => {
                    if let Err(error) = accounts.load_models(&provider).await {
                        tracing::warn!(error = %format!("{error:#}"), "could not load the ChatGPT model list");
                    }
                }
                Err(error) => tracing::warn!(error = %format!("{error:#}"), "ChatGPT sign-in did not complete"),
            }
        });
    }

    pub fn chatgpt_cancel_sign_in(&self, provider: &str) {
        self.llm.chatgpt.cancel_sign_in(provider);
    }

    pub fn chatgpt_sign_out(&self, provider: &str) {
        let accounts = self.llm.chatgpt.clone();
        let provider = provider.to_string();
        self.tokio.spawn(async move {
            if let Err(error) = accounts.sign_out(&provider).await {
                tracing::warn!(error = %format!("{error:#}"), "ChatGPT sign-out failed");
            }
        });
    }

    pub fn chatgpt_load_models(&self, provider: &str) {
        let accounts = self.llm.chatgpt.clone();
        let provider = provider.to_string();
        self.tokio.spawn(async move {
            if let Err(error) = accounts.load_models(&provider).await {
                tracing::warn!(error = %format!("{error:#}"), "could not load the ChatGPT model list");
            }
        });
    }

    pub fn open_chatgpt_usage_page(&self) -> Result<()> {
        open::that_detached(MANAGE_USAGE_URL).context("opening the browser")
    }

    pub fn tokio_handle(&self) -> tokio::runtime::Handle {
        self.tokio.handle().clone()
    }

    pub fn set_ui_notifier(&self, callback: Arc<dyn Fn() + Send + Sync>) {
        *self.notifier.0.lock() = Some(callback);
    }

    pub fn save_config(&self, config: Config) -> Result<()> {
        self.config.save(config)
    }

    pub fn ffmpeg_status(&self) -> FfmpegStatus {
        self.ffmpeg.read().clone()
    }

    pub fn refresh_ffmpeg(self: &Arc<Self>) {
        let status = match ffmpeg::locate(&self.config.current().ffmpeg, &self.paths.managed_ffmpeg_dir()) {
            Some(tools) => FfmpegStatus::Available { tools, version: None },
            None => FfmpegStatus::Missing,
        };
        *self.ffmpeg.write() = status;
        self.fetch_ffmpeg_version();
        self.notifier.notify();
    }

    fn fetch_ffmpeg_version(self: &Arc<Self>) {
        let Some(tools) = self.ffmpeg.read().tools().cloned() else { return };
        let runtime = self.clone();
        self.tokio.spawn(async move {
            let version = tools.version().await;
            let mut status = runtime.ffmpeg.write();
            if let FfmpegStatus::Available { tools: current, version: slot } = &mut *status
                && *current == tools
            {
                *slot = version;
            }
            drop(status);
            runtime.notifier.notify();
        });
    }

    pub fn download_status(&self) -> DownloadStatus {
        self.download.lock().clone()
    }

    pub fn start_ffmpeg_download(self: &Arc<Self>) {
        if matches!(*self.download.lock(), DownloadStatus::Running(_)) {
            return;
        }
        *self.download.lock() = DownloadStatus::Running(DownloadProgress {
            phase: ffmpeg::DownloadPhase::Downloading,
            downloaded_bytes: 0,
            total_bytes: None,
        });
        let runtime = self.clone();
        let target_dir = self.paths.managed_ffmpeg_dir();
        self.tokio.spawn(async move {
            let progress_runtime = runtime.clone();
            let progress = Arc::new(move |progress: DownloadProgress| {
                *progress_runtime.download.lock() = DownloadStatus::Running(progress);
                progress_runtime.notifier.notify();
            });
            let result = ffmpeg::download_managed_ffmpeg(&target_dir, progress, tokio_util::sync::CancellationToken::new()).await;
            *runtime.download.lock() = match result {
                Ok(_) => DownloadStatus::Finished,
                Err(error) => DownloadStatus::Failed(format!("{error:#}")),
            };
            runtime.refresh_ffmpeg();
        });
    }

    pub fn test_provider(&self, provider: ProviderProfile, model: String, effort: Option<String>) -> ProviderTest {
        let state: ProviderTest = Arc::new(Mutex::new(ProviderTestState::Running));
        let result_slot = state.clone();
        let notifier = self.notifier.clone();
        let services = self.llm.clone();
        self.tokio.spawn(async move {
            let outcome = crate::llm::test_connection(&provider, &model, effort, &services).await;
            *result_slot.lock() = match outcome {
                Ok(message) => ProviderTestState::Succeeded(message),
                Err(error) => ProviderTestState::Failed(format!("{error:#}")),
            };
            notifier.notify();
        });
        state
    }

    pub fn is_watch_paused(&self) -> bool {
        self.watch_paused.load(Ordering::Relaxed)
    }

    pub fn set_watch_paused(&self, paused: bool) {
        self.watch_paused.store(paused, Ordering::Relaxed);
        self.notifier.notify();
    }

    pub fn watched_folders(&self) -> Vec<PathBuf> {
        self.watcher.lock().as_ref().map(|w| w.watched_folders()).unwrap_or_default()
    }

    /// Files in watched folders that are still being written, downloaded or checked.
    pub fn pending_files(&self) -> Vec<PendingFile> {
        self.watcher.lock().as_ref().map(|w| w.pending()).unwrap_or_default()
    }

    /// Queues files and folders dropped on the window or passed on the command line.
    /// Folders are scanned. Returns the ids of the jobs that were queued.
    pub fn submit_paths(&self, paths: &[PathBuf], origin: JobOrigin, options: JobOptions) -> Vec<JobId> {
        let config = self.config.current();
        let mut queued = Vec::new();
        for path in paths {
            if path.is_dir() {
                for file in without_subtitles_of_videos(scan_existing_files(path, true, &config.watch), &config.watch) {
                    queued.extend(self.submit_file(&file, &config.watch, origin, options));
                }
            } else {
                queued.extend(self.submit_file(path, &config.watch, origin, options));
            }
        }
        queued
    }

    fn submit_file(&self, path: &Path, watch: &WatchConfig, origin: JobOrigin, options: JobOptions) -> Option<JobId> {
        let Some(kind) = classify_path(path, watch) else {
            tracing::info!(path = %path.display(), "ignoring file with unknown extension");
            return None;
        };
        let input = match kind {
            MediaKind::Video => JobInput::Media(path.to_path_buf()),
            MediaKind::Subtitle => JobInput::Subtitle(path.to_path_buf()),
        };
        self.jobs.submit(input, origin, options)
    }

    fn start_folder_watching(self: &Arc<Self>) -> Result<()> {
        let (sender, mut receiver) = mpsc::unbounded_channel::<WatchEvent>();
        let notifier = self.notifier.clone();
        let written = self.jobs.written_files().clone();
        let watcher = FolderWatcher::start(
            self.config.current().watch.clone(),
            self.tokio_handle(),
            sender,
            Arc::new(move || notifier.notify()),
            Arc::new(move |path: &Path| written.contains(path)),
        )?;
        *self.watcher.lock() = Some(watcher);

        let runtime = self.clone();
        self.tokio.spawn(async move {
            while let Some(event) = receiver.recv().await {
                if runtime.is_watch_paused() {
                    tracing::info!(path = %event.path.display(), "watching is paused, ignoring new file");
                    continue;
                }
                if event.kind == MediaKind::Subtitle
                    && let Some(video) = video_for_subtitle(&event.path, &runtime.config.current().watch)
                {
                    tracing::info!(subtitle = %event.path.display(), video = %video.display(), "subtitle belongs to a video, checking the video instead");
                    if let Some(watcher) = runtime.watcher.lock().as_ref() {
                        watcher.consider(video);
                    }
                    continue;
                }
                let input = match event.kind {
                    MediaKind::Video => JobInput::Media(event.path),
                    MediaKind::Subtitle => JobInput::Subtitle(event.path),
                };
                runtime.jobs.submit(input, JobOrigin::WatchedFolder, JobOptions::default());
            }
        });
        self.scan_folders_with_existing_files(&self.config.current().watch, None);
        Ok(())
    }

    /// Feeds pre-existing files of `process_existing` folders through the settle check.
    /// With `previous` given, only folders that are new since then are scanned.
    fn scan_folders_with_existing_files(self: &Arc<Self>, watch: &WatchConfig, previous: Option<&WatchConfig>) {
        if !watch.enabled {
            return;
        }
        let folders: Vec<_> = watch
            .folders
            .iter()
            .filter(|folder| folder.process_existing && folder.path.is_dir())
            .filter(|folder| previous.is_none_or(|old| !old.folders.iter().any(|f| f.path == folder.path && f.process_existing)))
            .cloned()
            .collect();
        if folders.is_empty() {
            return;
        }
        let runtime = self.clone();
        let watch = watch.clone();
        self.tokio.spawn_blocking(move || {
            for folder in folders {
                let files = without_subtitles_of_videos(scan_existing_files(&folder.path, folder.recursive, &watch), &watch);
                tracing::info!(folder = %folder.path.display(), count = files.len(), "queueing existing files");
                if let Some(watcher) = runtime.watcher.lock().as_ref() {
                    for file in files {
                        watcher.consider(file);
                    }
                }
            }
        });
    }

    fn start_config_file_watcher(self: &Arc<Self>) {
        let config_path = self.paths.config_file();
        let runtime = self.clone();
        let handler = move |result: notify_debouncer_full::DebounceEventResult| {
            let Ok(events) = result else { return };
            if !events.iter().any(|e| e.paths.iter().any(|p| p == &config_path)) {
                return;
            }
            match runtime.config.reload_from_disk() {
                Ok(true) => tracing::info!("configuration reloaded from disk"),
                Ok(false) => {}
                Err(error) => tracing::warn!(%error, "configuration file changed but could not be read; keeping previous settings"),
            }
        };
        match notify_debouncer_full::new_debouncer(Duration::from_millis(500), None, handler) {
            Ok(mut debouncer) => {
                if let Err(error) = debouncer.watch(self.paths.config_dir(), notify::RecursiveMode::NonRecursive) {
                    tracing::warn!(%error, "cannot watch the configuration file");
                }
                *self.config_file_watcher.lock() = Some(debouncer);
            }
            Err(error) => tracing::warn!(%error, "cannot start the configuration file watcher"),
        }
    }

    /// Renaming a ChatGPT provider keeps its sign-in. A rename shows up as one ChatGPT
    /// entry disappearing while another appears in the same save.
    fn follow_chatgpt_renames(&self, previous: &Config, current: &Config) {
        let chatgpt_names = |config: &Config| -> Vec<String> {
            config.providers.iter().filter(|p| p.kind == ProviderKind::ChatGpt).map(|p| p.name.clone()).collect()
        };
        let (before, after) = (chatgpt_names(previous), chatgpt_names(current));
        let removed: Vec<&String> = before.iter().filter(|name| !after.contains(name)).collect();
        let added: Vec<&String> = after.iter().filter(|name| !before.contains(name)).collect();
        if let ([from], [to]) = (removed.as_slice(), added.as_slice()) {
            self.llm.chatgpt.rename(from, to);
        }
    }

    fn start_config_change_listener(self: &Arc<Self>) {
        let runtime = self.clone();
        let mut receiver = self.config.subscribe();
        self.tokio.spawn(async move {
            let mut previous = receiver.borrow().clone();
            while receiver.changed().await.is_ok() {
                let current = receiver.borrow().clone();
                if let Some(watcher) = runtime.watcher.lock().as_mut() {
                    watcher.apply(&current.watch);
                }
                if current.watch != previous.watch {
                    runtime.scan_folders_with_existing_files(&current.watch, Some(&previous.watch));
                }
                if current.ffmpeg != previous.ffmpeg {
                    runtime.refresh_ffmpeg();
                }
                if current.general.max_concurrent_jobs != previous.general.max_concurrent_jobs {
                    runtime.jobs.reschedule();
                }
                runtime.follow_chatgpt_renames(&previous, &current);
                previous = current;
                runtime.notifier.notify();
            }
        });
    }
}

/// Drops subtitle files that belong to a video in the same scan; the video's own job
/// picks the best of them, so a film is translated once instead of once per file.
fn without_subtitles_of_videos(files: Vec<PathBuf>, watch: &WatchConfig) -> Vec<PathBuf> {
    let scanned: HashSet<PathBuf> = files.iter().cloned().collect();
    files
        .into_iter()
        .filter(|file| {
            classify_path(file, watch) != Some(MediaKind::Subtitle)
                || !video_for_subtitle(file, watch).is_some_and(|video| scanned.contains(&video))
        })
        .collect()
}
