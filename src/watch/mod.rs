//! Folder watching. Uses the operating system's native change notifications, so an
//! idle watcher costs nothing; new files are only queued once they stop changing, no
//! other program is writing them and no part of them is left undownloaded.

mod readiness;
mod scanner;
mod watcher;

pub use scanner::scan_existing_files;
pub use watcher::{FolderWatcher, PendingFile, PendingState, WatchEvent};
