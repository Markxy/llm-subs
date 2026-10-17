use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;

use super::job::FileFingerprint;

/// Files that jobs wrote. A folder watcher sees them appear like any other file; this
/// tells them apart, so a translation does not set off another one.
#[derive(Clone, Default)]
pub struct WrittenFiles(Arc<Mutex<HashMap<PathBuf, FileFingerprint>>>);

impl WrittenFiles {
    /// Remembers `path` as it is on disk now.
    pub fn record(&self, path: &Path) {
        if let Some(fingerprint) = FileFingerprint::of(path) {
            self.0.lock().insert(path.to_path_buf(), fingerprint);
        }
    }

    /// True when `path` is a file a job wrote and nothing else has written it since,
    /// including when the job removed it again. A file that changed is forgotten.
    pub fn contains(&self, path: &Path) -> bool {
        let current = FileFingerprint::of(path);
        let mut files = self.0.lock();
        let Some(&written) = files.get(path) else { return false };
        if current.is_some_and(|current| current != written) {
            files.remove(path);
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_written_file_counts_until_something_else_changes_it() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("Movie.et.srt");
        let written = WrittenFiles::default();
        std::fs::write(&output, "translated").unwrap();
        assert!(!written.contains(&output));

        written.record(&output);
        assert!(written.contains(&output));

        std::fs::write(&output, "replaced by hand").unwrap();
        assert!(!written.contains(&output));
        std::fs::write(&output, "translated").unwrap();
        assert!(!written.contains(&output), "a changed file stays forgotten");
    }

    #[test]
    fn a_written_file_that_was_removed_still_counts() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("Movie.et.srt");
        let written = WrittenFiles::default();
        std::fs::write(&output, "translated").unwrap();
        written.record(&output);
        std::fs::remove_file(&output).unwrap();
        assert!(written.contains(&output));
    }
}
