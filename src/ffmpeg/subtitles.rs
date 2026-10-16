use std::path::Path;

use anyhow::{Context, Result};
use tokio_util::sync::CancellationToken;

use super::{command, FfmpegTools, SubtitleStream};

/// Extracts one embedded subtitle stream as SubRip text without touching the media file.
pub async fn extract_subtitle_stream(
    tools: &FfmpegTools,
    media: &Path,
    stream: &SubtitleStream,
    cancel: &CancellationToken,
) -> Result<String> {
    let map = format!("0:{}", stream.stream_index);
    let args: Vec<&std::ffi::OsStr> = vec![
        "-v".as_ref(),
        "error".as_ref(),
        "-nostdin".as_ref(),
        "-i".as_ref(),
        media.as_os_str(),
        "-map".as_ref(),
        map.as_ref(),
        "-c:s".as_ref(),
        "srt".as_ref(),
        "-f".as_ref(),
        "srt".as_ref(),
        "pipe:1".as_ref(),
    ];
    let output = command::run(&tools.ffmpeg, &args, cancel).await?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Reads a subtitle file as SubRip text, converting other text formats with FFmpeg
/// and guessing the character encoding of legacy files.
pub async fn read_subtitle_file(tools: Option<&FfmpegTools>, path: &Path, cancel: &CancellationToken) -> Result<String> {
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if extension == "srt" {
        let bytes = tokio::fs::read(path).await.with_context(|| format!("reading {}", path.display()))?;
        return Ok(decode_text(&bytes));
    }
    let tools = tools.context("FFmpeg is required to read non-SRT subtitle files")?;
    let args: Vec<&std::ffi::OsStr> =
        vec!["-v".as_ref(), "error".as_ref(), "-nostdin".as_ref(), "-i".as_ref(), path.as_os_str(), "-f".as_ref(), "srt".as_ref(), "pipe:1".as_ref()];
    let output = command::run(&tools.ffmpeg, &args, cancel).await?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// UTF-8 when valid, otherwise the most plausible legacy encoding.
pub fn decode_text(bytes: &[u8]) -> String {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_string();
    }
    let mut detector = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
    detector.feed(bytes, true);
    let encoding = detector.guess(None, chardetng::Utf8Detection::Allow);
    let (decoded, _, _) = encoding.decode(bytes);
    decoded.into_owned()
}
