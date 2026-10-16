use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use super::{command, FfmpegTools};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtitleStream {
    /// Absolute stream index inside the container (`0:<stream_index>`).
    pub stream_index: usize,
    /// Position among subtitle streams only (`0:s:<relative_index>`).
    pub relative_index: usize,
    pub codec: String,
    pub language: Option<String>,
    pub title: Option<String>,
    pub forced: bool,
    pub hearing_impaired: bool,
    pub is_default: bool,
}

impl SubtitleStream {
    /// Bitmap formats (PGS, VobSub, DVB) cannot be turned into text without OCR.
    pub fn is_text_based(&self) -> bool {
        matches!(
            self.codec.as_str(),
            "subrip" | "srt" | "ass" | "ssa" | "webvtt" | "mov_text" | "text" | "microdvd" | "mpl2"
                | "subviewer" | "subviewer1" | "vplayer" | "jacosub" | "realtext" | "sami" | "stl" | "ttml"
                | "pjs"
        )
    }

    /// True when the title marks the track as signs & songs, commentary or similar.
    pub fn looks_like_non_dialogue(&self) -> bool {
        let Some(title) = self.title.as_deref() else { return false };
        let title = title.to_ascii_lowercase();
        ["sign", "song", "commentary", "karaoke", "lyrics", "forced"].iter().any(|word| title.contains(word))
    }

    pub fn describe(&self) -> String {
        let mut parts = vec![format!("#{}", self.relative_index), self.codec.clone()];
        if let Some(language) = &self.language {
            parts.push(language.clone());
        }
        if let Some(title) = &self.title {
            parts.push(format!("\"{title}\""));
        }
        if self.forced {
            parts.push("forced".to_string());
        }
        if self.hearing_impaired {
            parts.push("SDH".to_string());
        }
        parts.join(" ")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioStream {
    pub language: Option<String>,
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaInfo {
    pub format_name: String,
    pub subtitle_streams: Vec<SubtitleStream>,
    pub audio_streams: Vec<AudioStream>,
}

impl MediaInfo {
    pub fn is_matroska(&self) -> bool {
        self.format_name.contains("matroska")
    }

    pub fn is_mp4_family(&self) -> bool {
        self.format_name.contains("mp4") || self.format_name.contains("mov")
    }

    /// The language most likely spoken in the media: the default audio track's tag.
    pub fn primary_audio_language(&self) -> Option<&str> {
        self.audio_streams
            .iter()
            .find(|a| a.is_default)
            .or(self.audio_streams.first())
            .and_then(|a| a.language.as_deref())
            .filter(|lang| !is_undetermined(lang))
    }
}

pub fn is_undetermined(language: &str) -> bool {
    matches!(language, "und" | "mis" | "zxx" | "")
}

#[derive(Deserialize)]
struct ProbeOutput {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    format: Option<ProbeFormat>,
}

#[derive(Deserialize)]
struct ProbeStream {
    index: usize,
    codec_type: Option<String>,
    codec_name: Option<String>,
    #[serde(default)]
    tags: ProbeTags,
    #[serde(default)]
    disposition: ProbeDisposition,
}

#[derive(Deserialize, Default)]
struct ProbeTags {
    language: Option<String>,
    title: Option<String>,
}

#[derive(Deserialize, Default)]
struct ProbeDisposition {
    #[serde(default)]
    default: u8,
    #[serde(default)]
    forced: u8,
    #[serde(default)]
    hearing_impaired: u8,
}

#[derive(Deserialize)]
struct ProbeFormat {
    format_name: Option<String>,
}

pub async fn probe(tools: &FfmpegTools, media: &Path, cancel: &CancellationToken) -> Result<MediaInfo> {
    let args: Vec<&std::ffi::OsStr> = vec![
        "-v".as_ref(),
        "error".as_ref(),
        "-print_format".as_ref(),
        "json".as_ref(),
        "-show_format".as_ref(),
        "-show_streams".as_ref(),
        "-i".as_ref(),
        media.as_os_str(),
    ];
    let output = command::run(&tools.ffprobe, &args, cancel).await?;
    let parsed: ProbeOutput = serde_json::from_slice(&output.stdout).context("unreadable ffprobe output")?;

    let mut subtitle_streams = Vec::new();
    let mut audio_streams = Vec::new();
    for stream in parsed.streams {
        let language = stream.tags.language.filter(|l| !is_undetermined(l));
        match stream.codec_type.as_deref() {
            Some("subtitle") => subtitle_streams.push(SubtitleStream {
                stream_index: stream.index,
                relative_index: subtitle_streams.len(),
                codec: stream.codec_name.unwrap_or_default(),
                language,
                title: stream.tags.title,
                forced: stream.disposition.forced == 1,
                hearing_impaired: stream.disposition.hearing_impaired == 1,
                is_default: stream.disposition.default == 1,
            }),
            Some("audio") => audio_streams.push(AudioStream { language, is_default: stream.disposition.default == 1 }),
            _ => {}
        }
    }

    Ok(MediaInfo {
        format_name: parsed.format.and_then(|f| f.format_name).unwrap_or_default(),
        subtitle_streams,
        audio_streams,
    })
}
