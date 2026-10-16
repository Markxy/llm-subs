use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::store::write_atomically;

use super::{JobRecord, JobStatus};

#[derive(Debug, Default, Serialize, Deserialize)]
struct HistoryFile {
    version: u32,
    jobs: Vec<JobRecord>,
}

/// Finished jobs, newest first, persisted as JSON. Each file keeps only its latest run,
/// so a file that is checked again and again shows up once.
pub struct JobHistory {
    path: PathBuf,
    records: Vec<JobRecord>,
    limit: usize,
}

impl JobHistory {
    pub fn load(path: &Path, limit: usize) -> Self {
        let records = std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<HistoryFile>(&bytes).ok())
            .map(|file| file.jobs)
            .unwrap_or_default();
        Self { path: path.to_path_buf(), records, limit: limit.max(10) }
    }

    pub fn set_limit(&mut self, limit: usize) {
        self.limit = limit.max(10);
    }

    pub fn records(&self) -> &[JobRecord] {
        &self.records
    }

    pub fn push(&mut self, record: JobRecord) -> Result<()> {
        self.records.retain(|existing| existing.id != record.id && existing.input.path() != record.input.path());
        self.records.insert(0, record);
        self.records.truncate(self.limit);
        self.save()
    }

    pub fn clear(&mut self) -> Result<()> {
        self.records.clear();
        self.save()
    }

    pub fn remove(&mut self, id: &str) -> Result<()> {
        self.records.retain(|record| record.id != id);
        self.save()
    }

    /// The most recent run that settled this file, when the file has not changed since.
    /// Runs that found nothing to translate from do not count.
    pub fn previous_run_for(&self, path: &Path) -> Option<&JobRecord> {
        let current = super::job::FileFingerprint::of(path);
        self.records.iter().find(|record| {
            record.input.path() == path
                && record.input_fingerprint == current
                && matches!(record.status, JobStatus::Succeeded | JobStatus::Skipped { no_usable_source: false, .. })
        })
    }

    fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = HistoryFile { version: 1, jobs: self.records.clone() };
        let bytes = serde_json::to_vec_pretty(&file)?;
        write_atomically(&self.path, &bytes).with_context(|| format!("writing {}", self.path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::{JobInput, JobOptions, JobOrigin};

    fn run_of(path: &str) -> JobRecord {
        JobRecord::new(JobInput::Media(PathBuf::from(path)), JobOrigin::WatchedFolder, JobOptions::default(), "et".into())
    }

    #[test]
    fn a_file_keeps_only_its_latest_run() {
        let dir = tempfile::tempdir().unwrap();
        let mut history = JobHistory::load(&dir.path().join("jobs.json"), 50);
        history.push(run_of("/videos/Movie.mkv")).unwrap();
        history.push(run_of("/videos/Other.mkv")).unwrap();
        let latest = run_of("/videos/Movie.mkv");
        history.push(latest.clone()).unwrap();
        let ids: Vec<&str> = history.records().iter().map(|record| record.id.as_str()).collect();
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0], latest.id);
    }
}
