//! Fetches a static FFmpeg build for the current platform into the app's own
//! directory, so users without FFmpeg can get going with one click.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use futures::StreamExt;
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use super::command;
use super::locator::executable_name;
use super::{FfmpegTools, ToolOrigin};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadPhase {
    Downloading,
    Extracting,
    Finished,
}

#[derive(Debug, Clone, Copy)]
pub struct DownloadProgress {
    pub phase: DownloadPhase,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
}

pub type ProgressCallback = Arc<dyn Fn(DownloadProgress) + Send + Sync>;

/// Static builds published on GitHub by well-known maintainers.
enum Source {
    /// A zip archive from github.com/BtbN/FFmpeg-Builds with both executables inside.
    BtbN { target: &'static str },
    /// Separate executables from github.com/shaka-project/static-ffmpeg-binaries.
    Shaka { target: &'static str },
}

fn source() -> Result<Source> {
    let source = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Source::Shaka { target: "win-x64" },
        ("windows", "aarch64") => Source::BtbN { target: "winarm64" },
        ("linux", "x86_64") => Source::Shaka { target: "linux-x64" },
        ("linux", "aarch64") => Source::Shaka { target: "linux-arm64" },
        ("macos", "x86_64") => Source::Shaka { target: "osx-x64" },
        ("macos", "aarch64") => Source::Shaka { target: "osx-arm64" },
        (os, arch) => bail!("no managed FFmpeg build is available for {os}/{arch}; install FFmpeg and set its path"),
    };
    Ok(source)
}

/// Downloads `ffmpeg` and `ffprobe` into `target_dir`.
pub async fn download_managed_ffmpeg(
    target_dir: &Path,
    progress: ProgressCallback,
    cancel: CancellationToken,
) -> Result<FfmpegTools> {
    let source = source()?;
    tokio::fs::create_dir_all(target_dir).await?;
    let staging = tempfile::tempdir_in(target_dir).context("creating staging directory")?;
    let client = reqwest::Client::builder()
        .user_agent(concat!("llm-subs/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()?;

    let wanted = [executable_name("ffmpeg"), executable_name("ffprobe")];
    match source {
        Source::BtbN { target } => {
            let url = format!("https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-{target}-gpl.zip");
            let archive_path = staging.path().join("ffmpeg.zip");
            download_all(&client, &[(url, archive_path.clone())], &progress, &cancel).await?;
            progress(DownloadProgress { phase: DownloadPhase::Extracting, downloaded_bytes: 0, total_bytes: None });
            let wanted = wanted.clone();
            let destination = staging.path().to_path_buf();
            tokio::task::spawn_blocking(move || extract_from_zip(&archive_path, &wanted, &destination)).await??;
        }
        Source::Shaka { target } => {
            let downloads = ["ffmpeg", "ffprobe"].map(|tool| {
                let url = format!("https://github.com/shaka-project/static-ffmpeg-binaries/releases/latest/download/{tool}-{target}{}", std::env::consts::EXE_SUFFIX);
                (url, staging.path().join(executable_name(tool)))
            });
            download_all(&client, &downloads, &progress, &cancel).await?;
        }
    }

    // A build this system cannot run, such as one made for a newer macOS, must not replace one that works.
    for name in &wanted {
        let staged = staging.path().join(name);
        make_executable(&staged)?;
        command::run(&staged, &["-version"], &cancel).await.with_context(|| format!("the downloaded {name} does not run on this system"))?;
    }
    for name in &wanted {
        replace_file(&staging.path().join(name), &target_dir.join(name))?;
    }

    progress(DownloadProgress { phase: DownloadPhase::Finished, downloaded_bytes: 0, total_bytes: None });
    Ok(FfmpegTools {
        ffmpeg: target_dir.join(&wanted[0]),
        ffprobe: target_dir.join(&wanted[1]),
        origin: ToolOrigin::Managed,
    })
}

/// Downloads each URL to its path, all at once, and reports their combined progress.
async fn download_all(
    client: &reqwest::Client,
    downloads: &[(String, PathBuf)],
    progress: &ProgressCallback,
    cancel: &CancellationToken,
) -> Result<()> {
    // Bytes received and expected per download; the total stays unknown until every response has given its size.
    let transfers = Mutex::new(vec![(0, None); downloads.len()]);
    let report = |index: usize, received: u64, expected: Option<u64>| {
        let combined = {
            let mut transfers = transfers.lock();
            transfers[index] = (received, expected);
            DownloadProgress {
                phase: DownloadPhase::Downloading,
                downloaded_bytes: transfers.iter().map(|(received, _)| received).sum(),
                total_bytes: transfers.iter().map(|(_, expected)| *expected).sum(),
            }
        };
        progress(combined);
    };
    let report = &report;
    futures::future::try_join_all(downloads.iter().enumerate().map(|(index, (url, destination))| {
        download_to_file(client, url, destination, cancel, move |received, expected| report(index, received, expected))
    }))
    .await?;
    Ok(())
}

async fn download_to_file(
    client: &reqwest::Client,
    url: &str,
    destination: &Path,
    cancel: &CancellationToken,
    on_progress: impl Fn(u64, Option<u64>),
) -> Result<()> {
    let response = client.get(url).send().await.with_context(|| format!("requesting {url}"))?.error_for_status()?;
    let total_bytes = response.content_length();
    let mut file = tokio::fs::File::create(destination).await?;
    let mut stream = response.bytes_stream();
    let mut downloaded_bytes = 0u64;
    while let Some(chunk) = tokio::select! {
        chunk = stream.next() => chunk,
        _ = cancel.cancelled() => bail!("download cancelled"),
    } {
        let chunk = chunk.context("download interrupted")?;
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
        downloaded_bytes += chunk.len() as u64;
        on_progress(downloaded_bytes, total_bytes);
    }
    tokio::io::AsyncWriteExt::flush(&mut file).await?;
    Ok(())
}

fn extract_from_zip(archive_path: &Path, wanted: &[String], destination: &Path) -> Result<()> {
    let file = std::fs::File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(file).context("opening zip archive")?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let Some(name) = Path::new(entry.name()).file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        if entry.is_file() && wanted.contains(&name) {
            let mut out = std::fs::File::create(destination.join(&name))?;
            std::io::copy(&mut entry, &mut out)?;
        }
    }
    if let Some(missing) = wanted.iter().find(|name| !destination.join(name).is_file()) {
        bail!("the downloaded archive did not contain {missing}");
    }
    Ok(())
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

fn replace_file(from: &Path, to: &Path) -> Result<()> {
    if to.exists() {
        std::fs::remove_file(to).with_context(|| format!("replacing {}", to.display()))?;
    }
    if std::fs::rename(from, to).is_err() {
        std::fs::copy(from, to)?;
    }
    Ok(())
}
