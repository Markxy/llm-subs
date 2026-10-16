//! Chooses which subtitles to translate from and notices when the target language
//! is already there.

use std::path::PathBuf;

use crate::config::same_language;
use crate::ffmpeg::{MediaInfo, SubtitleStream};

use super::SidecarSubtitle;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubtitleSource {
    Embedded(SubtitleStream),
    Sidecar { path: PathBuf, language: Option<String> },
}

impl SubtitleSource {
    pub fn language(&self) -> Option<&str> {
        match self {
            SubtitleSource::Embedded(stream) => stream.language.as_deref(),
            SubtitleSource::Sidecar { language, .. } => language.as_deref(),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            SubtitleSource::Embedded(stream) => format!("embedded track {}", stream.describe()),
            SubtitleSource::Sidecar { path, language } => format!(
                "sidecar file {}{}",
                path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                language.as_deref().map(|l| format!(" ({l})")).unwrap_or_default()
            ),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SelectionReport {
    pub chosen: Option<SubtitleSource>,
    /// Set when subtitles in the target language already exist, with a description.
    pub target_already_present: Option<String>,
    /// Why candidates were rejected, for the job log.
    pub notes: Vec<String>,
}

pub fn select_source(
    info: Option<&MediaInfo>,
    sidecars: &[SidecarSubtitle],
    target_language: &str,
    preferred_source_languages: &[String],
) -> SelectionReport {
    let mut report = SelectionReport::default();
    let audio_language = info.and_then(|i| i.primary_audio_language()).map(str::to_string);

    let mut candidates: Vec<(i32, SubtitleSource)> = Vec::new();
    if let Some(info) = info {
        for stream in &info.subtitle_streams {
            if let Some(language) = &stream.language
                && same_language(language, target_language)
                && !stream.forced
            {
                report.target_already_present.get_or_insert_with(|| format!("embedded track {}", stream.describe()));
                continue;
            }
            if !stream.is_text_based() {
                report.notes.push(format!("skipped {}: bitmap subtitles need OCR", stream.describe()));
                continue;
            }
            if stream.forced || stream.looks_like_non_dialogue() {
                report.notes.push(format!("skipped {}: forced or signs-only track", stream.describe()));
                continue;
            }
            let mut score = language_score(stream.language.as_deref(), audio_language.as_deref(), preferred_source_languages);
            score += codec_score(&stream.codec);
            if stream.hearing_impaired {
                score -= 5;
            }
            if stream.is_default {
                score += 3;
            }
            candidates.push((score, SubtitleSource::Embedded(stream.clone())));
        }
    }
    for sidecar in sidecars {
        if let Some(language) = &sidecar.language
            && same_language(language, target_language)
            && !sidecar.forced
        {
            report.target_already_present.get_or_insert_with(|| format!("sidecar file {}", sidecar.path.display()));
            continue;
        }
        if sidecar.forced {
            continue;
        }
        let mut score = language_score(sidecar.language.as_deref(), audio_language.as_deref(), preferred_source_languages);
        if sidecar.hearing_impaired {
            score -= 5;
        }
        score += 4;
        candidates.push((score, SubtitleSource::Sidecar { path: sidecar.path.clone(), language: sidecar.language.clone() }));
    }

    candidates.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    report.chosen = candidates.into_iter().next().map(|(_, source)| source);
    report
}

fn language_score(language: Option<&str>, audio_language: Option<&str>, preferred: &[String]) -> i32 {
    let Some(language) = language else { return 10 };
    if let Some(position) = preferred.iter().position(|p| same_language(p, language)) {
        return 100 - position as i32 * 10;
    }
    if audio_language.is_some_and(|audio| same_language(audio, language)) {
        return 60;
    }
    if same_language(language, "en") {
        return 30;
    }
    15
}

fn codec_score(codec: &str) -> i32 {
    match codec {
        "subrip" | "srt" => 6,
        "ass" | "ssa" => 5,
        "webvtt" => 4,
        "mov_text" => 3,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffmpeg::AudioStream;

    fn stream(index: usize, codec: &str, language: Option<&str>, forced: bool, sdh: bool) -> SubtitleStream {
        SubtitleStream {
            stream_index: index,
            relative_index: index,
            codec: codec.into(),
            language: language.map(str::to_string),
            title: None,
            forced,
            hearing_impaired: sdh,
            is_default: false,
        }
    }

    fn info(streams: Vec<SubtitleStream>, audio_language: Option<&str>) -> MediaInfo {
        MediaInfo {
            format_name: "matroska".into(),
            subtitle_streams: streams,
            audio_streams: vec![AudioStream { language: audio_language.map(str::to_string), is_default: true }],
        }
    }

    #[test]
    fn prefers_audio_language_dialogue_track() {
        let media = info(
            vec![stream(0, "subrip", Some("eng"), false, true), stream(1, "subrip", Some("jpn"), false, false), stream(2, "hdmv_pgs_subtitle", Some("jpn"), false, false)],
            Some("jpn"),
        );
        let report = select_source(Some(&media), &[], "de", &[]);
        match report.chosen.unwrap() {
            SubtitleSource::Embedded(s) => assert_eq!(s.relative_index, 1),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn detects_existing_target_language() {
        let media = info(vec![stream(0, "subrip", Some("ger"), false, false), stream(1, "subrip", Some("eng"), false, false)], Some("eng"));
        let report = select_source(Some(&media), &[], "de", &[]);
        assert!(report.target_already_present.is_some());
        assert!(matches!(report.chosen, Some(SubtitleSource::Embedded(ref s)) if s.relative_index == 1));
    }

    #[test]
    fn preferred_languages_win() {
        let media = info(vec![stream(0, "subrip", Some("eng"), false, false), stream(1, "ass", Some("fra"), false, false)], Some("eng"));
        let report = select_source(Some(&media), &[], "de", &["fr".to_string()]);
        assert!(matches!(report.chosen, Some(SubtitleSource::Embedded(ref s)) if s.relative_index == 1));
    }
}
