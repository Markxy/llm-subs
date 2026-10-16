use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::llm::{Cost, TokenUsage};

pub type JobId = String;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "path", rename_all = "snake_case")]
pub enum JobInput {
    Media(PathBuf),
    Subtitle(PathBuf),
}

impl JobInput {
    pub fn path(&self) -> &Path {
        match self {
            JobInput::Media(path) | JobInput::Subtitle(path) => path,
        }
    }

    pub fn file_name(&self) -> String {
        self.path().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| self.path().display().to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobOrigin {
    WatchedFolder,
    Manual,
    CommandLine,
}

impl JobOrigin {
    pub fn label(self) -> &'static str {
        match self {
            JobOrigin::WatchedFolder => "watched folder",
            JobOrigin::Manual => "added manually",
            JobOrigin::CommandLine => "command line",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct JobOptions {
    /// Translate even if the target language already exists or the file was done before.
    pub force: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStage {
    Queued,
    Probing,
    Extracting,
    Translating,
    Writing,
    Finished,
}

impl JobStage {
    pub fn label(self) -> &'static str {
        match self {
            JobStage::Queued => "Queued",
            JobStage::Probing => "Inspecting media",
            JobStage::Extracting => "Extracting subtitles",
            JobStage::Translating => "Translating",
            JobStage::Writing => "Writing output",
            JobStage::Finished => "Finished",
        }
    }

    /// Where this stage starts on the overall progress bar and how much of it it owns.
    fn progress_span(self) -> (f32, f32) {
        match self {
            JobStage::Queued => (0.0, 0.0),
            JobStage::Probing => (0.0, 0.03),
            JobStage::Extracting => (0.03, 0.04),
            JobStage::Translating => (0.07, 0.90),
            JobStage::Writing => (0.97, 0.03),
            JobStage::Finished => (1.0, 0.0),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Succeeded,
    Skipped {
        reason: String,
        /// Nothing to translate from at the time; a subtitle file added later is worth another try.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        no_usable_source: bool,
    },
    Failed { error: String },
    Cancelled,
}

impl JobStatus {
    pub fn is_active(&self) -> bool {
        matches!(self, JobStatus::Queued | JobStatus::Running)
    }

    pub fn label(&self) -> &'static str {
        match self {
            JobStatus::Queued => "Queued",
            JobStatus::Running => "Running",
            JobStatus::Succeeded => "Done",
            JobStatus::Skipped { .. } => "Skipped",
            JobStatus::Failed { .. } => "Failed",
            JobStatus::Cancelled => "Cancelled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobLogLine {
    pub at: DateTime<Utc>,
    pub level: LogLevel,
    pub message: String,
}

/// Size and modification time used to recognise a file that was already handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileFingerprint {
    pub size: u64,
    pub modified_unix: i64,
}

impl FileFingerprint {
    pub fn of(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        let modified = metadata.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
        Some(Self { size: metadata.len(), modified_unix: modified })
    }
}

/// Everything known about a job. Persisted to the history file when finished.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: JobId,
    pub input: JobInput,
    pub origin: JobOrigin,
    pub options: JobOptions,
    pub target_language: String,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub status: JobStatus,
    pub stage: JobStage,
    /// 0.0 .. 1.0 across the whole job.
    pub progress: f32,
    /// Short description of the current activity.
    pub activity: String,
    pub source_description: Option<String>,
    pub output_path: Option<PathBuf>,
    pub cue_count: usize,
    pub translated_cues: usize,
    pub untranslated_cues: usize,
    #[serde(default)]
    pub usage: TokenUsage,
    #[serde(default)]
    pub cost: Option<Cost>,
    pub requests: usize,
    pub input_fingerprint: Option<FileFingerprint>,
    #[serde(default)]
    pub log: Vec<JobLogLine>,
}

const MAX_LOG_LINES: usize = 300;

impl JobRecord {
    pub fn new(input: JobInput, origin: JobOrigin, options: JobOptions, target_language: String) -> Self {
        let input_fingerprint = FileFingerprint::of(input.path());
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            input,
            origin,
            options,
            target_language,
            created_at: Utc::now(),
            started_at: None,
            finished_at: None,
            status: JobStatus::Queued,
            stage: JobStage::Queued,
            progress: 0.0,
            activity: "Waiting".to_string(),
            source_description: None,
            output_path: None,
            cue_count: 0,
            translated_cues: 0,
            untranslated_cues: 0,
            usage: TokenUsage::default(),
            cost: None,
            requests: 0,
            input_fingerprint,
            log: Vec::new(),
        }
    }

    pub fn duration(&self) -> Option<chrono::Duration> {
        Some(self.finished_at.unwrap_or_else(Utc::now) - self.started_at?)
    }
}

/// Live, shared view of a job used by the pipeline to report and by the UI to display.
#[derive(Clone)]
pub struct JobHandle {
    record: Arc<Mutex<JobRecord>>,
    cancel: CancellationToken,
    on_change: Arc<dyn Fn() + Send + Sync>,
}

impl JobHandle {
    pub fn new(record: JobRecord, on_change: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self { record: Arc::new(Mutex::new(record)), cancel: CancellationToken::new(), on_change }
    }

    pub fn id(&self) -> JobId {
        self.record.lock().id.clone()
    }

    pub fn input(&self) -> JobInput {
        self.record.lock().input.clone()
    }

    pub fn options(&self) -> JobOptions {
        self.record.lock().options
    }

    pub fn snapshot(&self) -> JobRecord {
        self.record.lock().clone()
    }

    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    pub fn is_active(&self) -> bool {
        self.record.lock().status.is_active()
    }

    pub fn update(&self, change: impl FnOnce(&mut JobRecord)) {
        change(&mut self.record.lock());
        (self.on_change)();
    }

    pub fn set_stage(&self, stage: JobStage, activity: impl Into<String>) {
        let activity = activity.into();
        self.update(|record| {
            record.stage = stage;
            record.progress = stage.progress_span().0;
            record.activity = activity;
        });
        self.log(LogLevel::Info, stage.label());
    }

    /// Progress inside the current stage, 0.0 .. 1.0.
    pub fn set_stage_progress(&self, fraction: f32, activity: impl Into<String>) {
        let activity = activity.into();
        self.update(|record| {
            let (start, span) = record.stage.progress_span();
            record.progress = (start + span * fraction.clamp(0.0, 1.0)).clamp(0.0, 1.0);
            record.activity = activity;
        });
    }

    pub fn log(&self, level: LogLevel, message: impl Into<String>) {
        let message = message.into();
        match level {
            LogLevel::Info => tracing::info!(job = %self.id(), "{message}"),
            LogLevel::Warning => tracing::warn!(job = %self.id(), "{message}"),
            LogLevel::Error => tracing::error!(job = %self.id(), "{message}"),
        }
        self.update(|record| {
            record.log.push(JobLogLine { at: Utc::now(), level, message });
            if record.log.len() > MAX_LOG_LINES {
                let overflow = record.log.len() - MAX_LOG_LINES;
                record.log.drain(0..overflow);
            }
        });
    }

    pub fn info(&self, message: impl Into<String>) {
        self.log(LogLevel::Info, message);
    }

    pub fn warn(&self, message: impl Into<String>) {
        self.log(LogLevel::Warning, message);
    }
}
