//! Fetches a static FFmpeg build for the current platform into the app's own
//! directory, so users without FFmpeg can get going with one click.

use std::io::{BufReader, BufWriter};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use futures::StreamExt;
use tokio_util::sync::CancellationToken;

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

#[derive(Debug, Clone, Copy)]
enum ArchiveKind {
    Zip,
    TarXz,
}

struct ReleaseArchive {
    url: &'static str,
    kind: ArchiveKind,
}

/// Static builds published by well-known community maintainers.
fn release_archives() -> Result<Vec<ReleaseArchive>> {
    let archives = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => vec![ReleaseArchive {
            url: "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip",
            kind: ArchiveKind::Zip,
        }],
        ("windows", "aarch64") => vec![ReleaseArchive {
            url: "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-winarm64-gpl.zip",
            kind: ArchiveKind::Zip,
        }],
        ("linux", "x86_64") => vec![ReleaseArchive {
            url: "https://johnvansickle.com/ffmpeg/releases/ffmpeg-release-amd64-static.tar.xz",
            kind: ArchiveKind::TarXz,
        }],
        ("linux", "aarch64") => vec![ReleaseArchive {
            url: "https://johnvansickle.com/ffmpeg/releases/ffmpeg-release-arm64-static.tar.xz",
            kind: ArchiveKind::TarXz,
        }],
        ("macos", _) => vec![
            ReleaseArchive { url: "https://evermeet.cx/ffmpeg/getrelease/zip", kind: ArchiveKind::Zip },
            ReleaseArchive { url: "https://evermeet.cx/ffmpeg/getrelease/ffprobe/zip", kind: ArchiveKind::Zip },
        ],
        (os, arch) => bail!("no managed FFmpeg build is available for {os}/{arch}; install FFmpeg and set its path"),
    };
    Ok(archives)
}

/// Downloads and unpacks `ffmpeg` and `ffprobe` into `target_dir`.
pub async fn download_managed_ffmpeg(
    target_dir: &Path,
    progress: ProgressCallback,
    cancel: CancellationToken,
) -> Result<FfmpegTools> {
    let archives = release_archives()?;
    tokio::fs::create_dir_all(target_dir).await?;
    let staging = tempfile::tempdir_in(target_dir).context("creating staging directory")?;
    let client = reqwest::Client::builder()
        .user_agent(concat!("llm-subs/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()?;

    let wanted = [executable_name("ffmpeg"), executable_name("ffprobe")];
    let mut downloaded_archives = Vec::new();
    for (index, archive) in archives.iter().enumerate() {
        let archive_path = staging.path().join(format!("archive-{index}"));
        download_to_file(&client, archive.url, &archive_path, &progress, &cancel).await?;
        downloaded_archives.push((archive_path, archive.kind));
    }

    progress(DownloadProgress { phase: DownloadPhase::Extracting, downloaded_bytes: 0, total_bytes: None });
    let extraction_dir = staging.path().join("extracted");
    std::fs::create_dir_all(&extraction_dir)?;
    let wanted_for_task = wanted.clone();
    let extraction_dir_for_task = extraction_dir.clone();
    tokio::task::spawn_blocking(move || -> Result<()> {
        for (archive_path, kind) in &downloaded_archives {
            match kind {
                ArchiveKind::Zip => extract_from_zip(archive_path, &wanted_for_task, &extraction_dir_for_task)?,
                ArchiveKind::TarXz => extract_from_tar_xz(archive_path, &wanted_for_task, &extraction_dir_for_task)?,
            }
        }
        Ok(())
    })
    .await??;

    for name in &wanted {
        let extracted = extraction_dir.join(name);
        if !extracted.is_file() {
            bail!("the downloaded archive did not contain {name}");
        }
        make_executable(&extracted)?;
        let final_path = target_dir.join(name);
        replace_file(&extracted, &final_path)?;
    }

    progress(DownloadProgress { phase: DownloadPhase::Finished, downloaded_bytes: 0, total_bytes: None });
    Ok(FfmpegTools {
        ffmpeg: target_dir.join(&wanted[0]),
        ffprobe: target_dir.join(&wanted[1]),
        origin: ToolOrigin::Managed,
    })
}

async fn download_to_file(
    client: &reqwest::Client,
    url: &str,
    destination: &Path,
    progress: &ProgressCallback,
    cancel: &CancellationToken,
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
        progress(DownloadProgress { phase: DownloadPhase::Downloading, downloaded_bytes, total_bytes });
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
    Ok(())
}

fn extract_from_tar_xz(archive_path: &Path, wanted: &[String], destination: &Path) -> Result<()> {
    let tar_path = archive_path.with_extension("tar");
    {
        let mut reader = BufReader::new(std::fs::File::open(archive_path)?);
        let mut writer = BufWriter::new(std::fs::File::create(&tar_path)?);
        lzma_rs::xz_decompress(&mut reader, &mut writer).context("decompressing xz archive")?;
    }
    let mut archive = tar::Archive::new(std::fs::File::open(&tar_path)?);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
        if entry.header().entry_type().is_file() && wanted.contains(&name) {
            let mut out = std::fs::File::create(destination.join(&name))?;
            std::io::copy(&mut entry, &mut out)?;
        }
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
