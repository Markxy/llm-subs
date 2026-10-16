//! Desktop notifications when a translation starts and when it ends.

use std::collections::HashSet;

use crate::config::language_name;
use crate::jobs::{JobId, JobRecord, JobStage, JobStatus};
use crate::platform::notifications;

/// Longest error text put into a notification; the job's log has all of it.
const MAX_ERROR_CHARS: usize = 180;

/// Announces each translation once when it starts translating and once when it ends.
/// Jobs that were already in the history at start-up stay silent, and so do jobs that
/// are skipped or fail before any translating starts.
#[derive(Default)]
pub struct JobAnnouncer {
    /// Jobs seen queued or running during this session.
    seen_active: HashSet<JobId>,
    /// Jobs whose start was announced.
    started: HashSet<JobId>,
}

impl JobAnnouncer {
    /// Compares a fresh job list with what was seen before.
    pub fn update(&mut self, records: &[JobRecord]) {
        for record in records {
            if record.status.is_active() {
                self.seen_active.insert(record.id.clone());
                if record.status == JobStatus::Running && record.stage >= JobStage::Translating && self.started.insert(record.id.clone()) {
                    notifications::show(format!("Translating into {}", language_name(&record.target_language)), record.input.file_name());
                }
                continue;
            }
            if !self.seen_active.remove(&record.id) {
                continue;
            }
            let started = self.started.remove(&record.id);
            match &record.status {
                JobStatus::Succeeded => notifications::show(
                    "Translation finished",
                    format!("{}\n{} cues translated into {}", record.input.file_name(), record.translated_cues, language_name(&record.target_language)),
                ),
                JobStatus::Failed { error } if started => {
                    notifications::show("Translation failed", format!("{}\n{}", record.input.file_name(), shorten(error, MAX_ERROR_CHARS)))
                }
                _ => {}
            }
        }
    }
}

fn shorten(text: &str, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        Some((cut, _)) => format!("{}…", text[..cut].trim_end()),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortens_long_errors_only() {
        assert_eq!(shorten("short", 10), "short");
        assert_eq!(shorten("a long error message", 6), "a long…");
    }
}
