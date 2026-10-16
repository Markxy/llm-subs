//! Adds a subtitle track to a container by remuxing (stream copy, no re-encode).

use std::path::Path;

use anyhow::{Context, Result, bail};
use tokio_util::sync::CancellationToken;

use crate::config::Language;
use crate::ffmpeg::{self, FfmpegTools, MediaInfo};

use super::kinds::REMUX_TEMP_MARKER;

/// Remuxes `media` with `subtitle_file` appended as a new track, then replaces the
/// original file atomically. Existing tracks are copied untouched.
pub async fn embed_subtitle_track(
    tools: &FfmpegTools,
    media: &Path,
    info: &MediaInfo,
    subtitle_file: &Path,
    language: &Language,
    title: &str,
    cancel: &CancellationToken,
) -> Result<()> {
    let subtitle_codec = if info.is_matroska() {
        "srt"
    } else if info.is_mp4_family() {
        "mov_text"
    } else {
        bail!("embedding is only supported for Matroska and MP4 files; use sidecar output for {}", info.format_name)
    };
    let extension = media.extension().and_then(|e| e.to_str()).unwrap_or("mkv");
    let temp_output = media.with_extension(format!("{REMUX_TEMP_MARKER}.{extension}"));
    let new_track_index = info.subtitle_streams.len();
    let codec_flag = format!("-c:s:{new_track_index}");
    let language_flag = format!("-metadata:s:s:{new_track_index}");
    let language_value = format!("language={}", language.code3);
    let title_value = format!("title={title}");
    let disposition_flag = format!("-disposition:s:{new_track_index}");

    let args: Vec<&std::ffi::OsStr> = vec![
        "-v".as_ref(),
        "error".as_ref(),
        "-nostdin".as_ref(),
        "-y".as_ref(),
        "-i".as_ref(),
        media.as_os_str(),
        "-i".as_ref(),
        subtitle_file.as_os_str(),
        "-map".as_ref(),
        "0".as_ref(),
        "-map".as_ref(),
        "1:0".as_ref(),
        "-c".as_ref(),
        "copy".as_ref(),
        codec_flag.as_ref(),
        subtitle_codec.as_ref(),
        language_flag.as_ref(),
        language_value.as_ref(),
        language_flag.as_ref(),
        title_value.as_ref(),
        disposition_flag.as_ref(),
        "0".as_ref(),
        temp_output.as_os_str(),
    ];
    let result = ffmpeg::run_command(&tools.ffmpeg, &args, cancel).await;
    if let Err(error) = result {
        let _ = tokio::fs::remove_file(&temp_output).await;
        return Err(error).context("remuxing with the new subtitle track");
    }

    let original_size = tokio::fs::metadata(media).await?.len();
    let new_size = tokio::fs::metadata(&temp_output).await?.len();
    if new_size < original_size / 2 {
        let _ = tokio::fs::remove_file(&temp_output).await;
        bail!("remuxed file is suspiciously small ({new_size} bytes vs {original_size}); original left untouched");
    }
    tokio::fs::rename(&temp_output, media).await.context("replacing the original media file")?;
    Ok(())
}
