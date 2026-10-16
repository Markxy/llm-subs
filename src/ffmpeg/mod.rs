//! Everything that shells out to FFmpeg: locating or downloading the binaries,
//! probing media and extracting subtitle streams.

mod command;
mod downloader;
mod locator;
mod probe;
mod subtitles;

pub use command::CommandError;
pub use downloader::{download_managed_ffmpeg, DownloadPhase, DownloadProgress};
pub use locator::{locate, FfmpegTools, ToolOrigin};
pub use probe::{probe, AudioStream, MediaInfo, SubtitleStream};
pub use subtitles::{extract_subtitle_stream, read_subtitle_file};

/// Runs the given FFmpeg binary with arguments, discarding stdout.
pub async fn run_command(
    program: &std::path::Path,
    args: &[impl AsRef<std::ffi::OsStr>],
    cancel: &tokio_util::sync::CancellationToken,
) -> anyhow::Result<()> {
    command::run(program, args, cancel).await?;
    Ok(())
}
