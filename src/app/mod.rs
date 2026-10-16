//! The running application: owns the async runtime, configuration, job queue and
//! folder watcher, and exposes the operations the UI and CLI need.

mod runtime;

pub use runtime::{AppRuntime, ChatGptSignIn, DownloadStatus, FfmpegStatus, ProviderTest, ProviderTestState, StartOptions};
