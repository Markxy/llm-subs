//! Runs jobs with bounded concurrency and keeps the list the UI displays.

use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::config::ConfigStore;
use crate::ffmpeg::FfmpegTools;
use crate::llm::LlmServices;

use super::{run_pipeline, JobHandle, JobHistory, JobId, JobInput, JobOptions, JobOrigin, JobRecord, JobStage, JobStatus, LogLevel, PipelineContext, PipelineOutcome, WrittenFiles};

pub struct QueueDependencies {
    pub config: ConfigStore,
    pub llm: LlmServices,
    /// Resolves the FFmpeg binaries at job start so a download or config change is picked up.
    pub locate_tools: Arc<dyn Fn() -> Option<FfmpegTools> + Send + Sync>,
    /// Called whenever any job changes, so the UI can repaint.
    pub on_change: Arc<dyn Fn() + Send + Sync>,
}

struct QueueState {
    jobs: Vec<JobHandle>,
    running: usize,
    paused: bool,
}

pub struct JobQueue {
    state: Mutex<QueueState>,
    history: Mutex<JobHistory>,
    deps: QueueDependencies,
    wake: Notify,
    /// Cancelled when translations are paused, which sends running jobs back to the queue.
    pause_interrupt: Mutex<CancellationToken>,
    written: WrittenFiles,
}

impl JobQueue {
    /// Creates the queue and starts its scheduler on the given runtime.
    pub fn start(deps: QueueDependencies, history: JobHistory, runtime: &tokio::runtime::Handle) -> Arc<Self> {
        let queue = Arc::new(Self {
            state: Mutex::new(QueueState { jobs: Vec::new(), running: 0, paused: false }),
            history: Mutex::new(history),
            deps,
            wake: Notify::new(),
            pause_interrupt: Mutex::new(CancellationToken::new()),
            written: WrittenFiles::default(),
        });
        let scheduler = queue.clone();
        runtime.spawn(async move { scheduler.scheduler_loop().await });
        queue
    }

    /// Queues a file unless it is already queued or running. Returns the new job id.
    pub fn submit(&self, input: JobInput, origin: JobOrigin, options: JobOptions) -> Option<JobId> {
        let config = self.deps.config.current();
        {
            let state = self.state.lock();
            if state.jobs.iter().any(|job| job.is_active() && job.input().path() == input.path()) {
                return None;
            }
        }
        if !options.force
            && origin == JobOrigin::WatchedFolder
            && let Some(previous) = self.history.lock().previous_run_for(input.path())
        {
            tracing::debug!(path = %input.path().display(), status = ?previous.status, "file already handled, not queueing");
            return None;
        }
        let record = JobRecord::new(input, origin, options, config.general.target_language.clone());
        let id = record.id.clone();
        let handle = JobHandle::new(record, self.deps.on_change.clone());
        self.state.lock().jobs.push(handle);
        (self.deps.on_change)();
        self.wake.notify_one();
        Some(id)
    }

    pub fn cancel(&self, id: &str) {
        let handle = self.state.lock().jobs.iter().find(|job| job.id() == id).cloned();
        if let Some(handle) = handle {
            let was_queued = matches!(handle.snapshot().status, JobStatus::Queued);
            handle.cancel();
            if was_queued {
                handle.update(|record| {
                    record.status = JobStatus::Cancelled;
                    record.finished_at = Some(chrono::Utc::now());
                    record.activity = "Cancelled".to_string();
                });
                self.finish(&handle);
            }
        }
    }

    /// Active jobs first (queue order), then history newest first.
    pub fn snapshot(&self) -> Vec<JobRecord> {
        let mut records: Vec<JobRecord> = self.state.lock().jobs.iter().map(|job| job.snapshot()).collect();
        records.extend(self.history.lock().records().iter().cloned());
        records
    }

    /// Subtitle and media files the jobs have written.
    pub fn written_files(&self) -> &WrittenFiles {
        &self.written
    }

    pub fn active_count(&self) -> usize {
        self.state.lock().jobs.len()
    }

    pub fn is_paused(&self) -> bool {
        self.state.lock().paused
    }

    /// While paused no job starts, and jobs that were running go back to the queue
    /// and start over once translations resume.
    pub fn set_paused(&self, paused: bool) {
        {
            let mut state = self.state.lock();
            if state.paused == paused {
                return;
            }
            state.paused = paused;
        }
        if paused {
            tracing::info!("translations paused");
            self.pause_interrupt.lock().cancel();
        } else {
            tracing::info!("translations resumed");
            *self.pause_interrupt.lock() = CancellationToken::new();
            self.wake.notify_one();
        }
        (self.deps.on_change)();
    }

    /// Starts queued jobs that fit now, e.g. after more files at once were allowed.
    pub fn reschedule(&self) {
        self.wake.notify_one();
    }

    pub fn clear_history(&self) {
        if let Err(error) = self.history.lock().clear() {
            tracing::warn!(%error, "could not clear history");
        }
        (self.deps.on_change)();
    }

    pub fn remove_from_history(&self, id: &str) {
        if let Err(error) = self.history.lock().remove(id) {
            tracing::warn!(%error, "could not update history");
        }
        (self.deps.on_change)();
    }

    async fn scheduler_loop(self: Arc<Self>) {
        loop {
            self.start_ready_jobs();
            self.wake.notified().await;
        }
    }

    fn start_ready_jobs(self: &Arc<Self>) {
        let max_concurrent = self.deps.config.current().general.max_concurrent_jobs.max(1);
        loop {
            let next = {
                let mut state = self.state.lock();
                if state.paused || state.running >= max_concurrent {
                    return;
                }
                let Some(handle) = state.jobs.iter().find(|job| matches!(job.snapshot().status, JobStatus::Queued)).cloned() else {
                    return;
                };
                state.running += 1;
                handle
            };
            next.update(|record| {
                record.status = JobStatus::Running;
                record.started_at = Some(chrono::Utc::now());
            });
            let queue = self.clone();
            tokio::spawn(async move {
                queue.run_one(next).await;
                queue.state.lock().running -= 1;
                queue.wake.notify_one();
            });
        }
    }

    async fn run_one(&self, handle: JobHandle) {
        let ctx = PipelineContext {
            config: self.deps.config.current(),
            tools: (self.deps.locate_tools)(),
            llm: self.deps.llm.clone(),
            written: self.written.clone(),
        };
        let cancel = handle.cancel_token();
        let pause_interrupt = self.pause_interrupt.lock().clone();
        let result = tokio::select! {
            result = run_pipeline(&handle, &ctx) => Some(result),
            _ = cancel.cancelled() => None,
            _ = pause_interrupt.cancelled() => {
                handle.info("Translations paused; this job starts over when they resume");
                handle.update(|record| {
                    record.status = JobStatus::Queued;
                    record.stage = JobStage::Queued;
                    record.progress = 0.0;
                    record.started_at = None;
                    record.activity = "Paused".to_string();
                });
                return;
            }
        };
        let finished_at = chrono::Utc::now();
        match result {
            None => handle.update(|record| {
                record.status = JobStatus::Cancelled;
                record.activity = "Cancelled".to_string();
            }),
            Some(Ok(PipelineOutcome::Completed { output_path })) => handle.update(|record| {
                record.status = JobStatus::Succeeded;
                record.stage = JobStage::Finished;
                record.progress = 1.0;
                record.activity = format!("Wrote {}", output_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            }),
            Some(Ok(PipelineOutcome::Skipped { reason, no_usable_source })) => {
                handle.log(LogLevel::Info, format!("Skipped: {reason}"));
                handle.update(|record| {
                    record.status = JobStatus::Skipped { reason: reason.clone(), no_usable_source };
                    record.stage = JobStage::Finished;
                    record.activity = reason;
                });
            }
            Some(Err(error)) => {
                let message = format!("{error:#}");
                handle.log(LogLevel::Error, message.clone());
                handle.update(|record| {
                    record.status = if cancel.is_cancelled() { JobStatus::Cancelled } else { JobStatus::Failed { error: message.clone() } };
                    record.activity = message;
                });
            }
        }
        handle.update(|record| record.finished_at = Some(finished_at));
        self.finish(&handle);
    }

    fn finish(&self, handle: &JobHandle) {
        let record = handle.snapshot();
        {
            let mut state = self.state.lock();
            state.jobs.retain(|job| job.id() != record.id);
        }
        let mut history = self.history.lock();
        history.set_limit(self.deps.config.current().general.history_limit);
        if let Err(error) = history.push(record) {
            tracing::warn!(%error, "could not save job history");
        }
        (self.deps.on_change)();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use super::*;
    use crate::llm::chatgpt::ChatGptAccounts;
    use crate::llm::UsageLedger;

    fn wait_for(what: &str, condition: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !condition() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// A provider that accepts every request and never answers, so a job stays in the
    /// middle of translating until something interrupts it. Returns its address and
    /// the number of requests it has received.
    fn silent_provider(runtime: &tokio::runtime::Runtime) -> (std::net::SocketAddr, Arc<AtomicUsize>) {
        let listener = runtime.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let counter = requests.clone();
        runtime.spawn(async move {
            let mut open_connections = Vec::new();
            while let Ok((connection, _)) = listener.accept().await {
                counter.fetch_add(1, Ordering::SeqCst);
                open_connections.push(connection);
            }
        });
        (address, requests)
    }

    #[test]
    fn pausing_holds_the_queue_and_sends_running_jobs_back() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let (address, requests) = silent_provider(&runtime);
        let config_path = dir.path().join("config.toml");
        let config = format!(
            "[general]\ntarget_language = \"et\"\n\n[[providers]]\nname = \"silent\"\nkind = \"openai_compatible\"\nbase_url = \"http://{address}/v1\"\n\n\
             [models.translation]\nprovider = \"silent\"\nmodel = \"test\"\n"
        );
        std::fs::write(&config_path, config).unwrap();
        let subtitle = dir.path().join("Clip.en.srt");
        std::fs::write(&subtitle, "1\n00:00:01,000 --> 00:00:02,000\nHello there.\n").unwrap();
        let dependencies = QueueDependencies {
            config: ConfigStore::open(&config_path).unwrap(),
            llm: LlmServices { chatgpt: ChatGptAccounts::open(&dir.path().join("auth.dat")), usage: UsageLedger::in_memory() },
            locate_tools: Arc::new(|| None),
            on_change: Arc::new(|| {}),
        };
        let queue = JobQueue::start(dependencies, JobHistory::load(&dir.path().join("jobs.json"), 10), runtime.handle());
        let status = || queue.snapshot()[0].status.clone();

        queue.set_paused(true);
        let id = queue.submit(JobInput::Subtitle(subtitle), JobOrigin::Manual, JobOptions::default()).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!((status(), requests.load(Ordering::SeqCst)), (JobStatus::Queued, 0));

        queue.set_paused(false);
        wait_for("the first request", || requests.load(Ordering::SeqCst) == 1);
        assert_eq!(status(), JobStatus::Running);

        queue.set_paused(true);
        wait_for("the job to return to the queue", || status() == JobStatus::Queued && queue.state.lock().running == 0);
        assert_eq!(queue.snapshot()[0].activity, "Paused");

        queue.set_paused(false);
        wait_for("the job to start over", || requests.load(Ordering::SeqCst) == 2);
        queue.cancel(&id);
        wait_for("the cancellation", || status() == JobStatus::Cancelled);
    }
}
