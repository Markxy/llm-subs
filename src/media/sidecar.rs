use std::path::{Path, PathBuf};

use crate::config::{find_language, find_language_by_name, Language, WatchConfig};

use super::kinds::looks_like_incomplete_file;
use super::{classify_path, MediaKind};

/// Folders release groups put a film's subtitles in: `Movie/Subs/English.srt`, or for a
/// season pack `Show/Subs/Show.S01E01/2_English.srt`.
const SUBTITLE_FOLDER_NAMES: &[&str] = &["subs", "subtitles", "sub", "subtitle"];

/// A subtitle file that belongs to a video, e.g. `Movie.en.srt` or `Subs/English.srt`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarSubtitle {
    pub path: PathBuf,
    /// Language parsed from the file name, when there was one.
    pub language: Option<String>,
    pub forced: bool,
    pub hearing_impaired: bool,
}

/// `Movie.mkv` + `de` -> `Movie.de.srt`, the naming every player auto-loads.
pub fn sidecar_path_for_language(media: &Path, language_code: &str) -> PathBuf {
    let stem = media.file_stem().and_then(|s| s.to_str()).unwrap_or("subtitles");
    media.with_file_name(format!("{stem}.{language_code}.srt"))
}

/// Subtitle files that belong to `media`: files named after it next to it or in a
/// subtitle folder beside it, everything in `Subs/<media name>/`, and, when it is the
/// only video in its folder, every other subtitle file there and in `Subs/` as well.
pub fn find_sidecar_subtitles(media: &Path, watch: &WatchConfig) -> Vec<SidecarSubtitle> {
    let Some(folder) = media.parent() else { return Vec::new() };
    let Some(media_stem) = media.file_stem().and_then(|s| s.to_str()) else { return Vec::new() };
    let only_video_in_folder = videos_in(folder, watch).len() == 1;

    let mut found = Vec::new();
    let mut collect = |directory: &Path, unrelated_names_belong: bool| {
        for path in subtitle_files_in(directory, watch) {
            let Some(file_stem) = path.file_stem().and_then(|s| s.to_str()) else { continue };
            let name = match strip_media_stem(file_stem, media_stem) {
                Some(suffix) => suffix.to_string(),
                None if unrelated_names_belong => file_stem.to_string(),
                None => continue,
            };
            found.push(describe_sidecar(path, &name));
        }
    };
    collect(folder, only_video_in_folder);
    for subtitle_folder in subtitle_folders_in(folder) {
        collect(&subtitle_folder, only_video_in_folder);
        collect(&subtitle_folder.join(media_stem), true);
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

/// The video a subtitle file belongs to, by the same rules a video's job uses to find
/// its subtitles, so a watched folder translates once per video and not once per file.
pub fn video_for_subtitle(subtitle: &Path, watch: &WatchConfig) -> Option<PathBuf> {
    let folder = subtitle.parent()?;
    let video_folder = if is_subtitle_folder(folder) {
        folder.parent()?
    } else if folder.parent().is_some_and(is_subtitle_folder) {
        folder.parent()?.parent()?
    } else {
        folder
    };
    videos_in(video_folder, watch)
        .into_iter()
        .find(|video| find_sidecar_subtitles(video, watch).iter().any(|sidecar| sidecar.path == subtitle))
}

/// Reads language and flags from the part of a file name that is not the video's name.
/// Words like `SDH` or `forced` are flags; the last other word decides the language, so
/// `2_English`, `SDH.eng` and `Brazilian.por` work while `It.Follows.2014` has none.
fn describe_sidecar(path: PathBuf, name: &str) -> SidecarSubtitle {
    let mut sidecar = SidecarSubtitle { path, language: None, forced: false, hearing_impaired: false };
    let words = name.split(['.', '_', ' ', '(', ')', '[', ']', ',']).filter(|word| !word.is_empty() && !word.chars().all(|c| c.is_ascii_digit()));
    for word in words {
        match word.to_ascii_lowercase().as_str() {
            "forced" | "signs" | "songs" => sidecar.forced = true,
            "sdh" | "hi" | "cc" => sidecar.hearing_impaired = true,
            "default" | "full" => {}
            _ => sidecar.language = language_named(word).map(|language| language.code.to_string()),
        }
    }
    sidecar
}

fn language_named(word: &str) -> Option<&'static Language> {
    find_language(word).or_else(|| find_language_by_name(word))
}

fn strip_media_stem<'a>(file_stem: &'a str, media_stem: &str) -> Option<&'a str> {
    if file_stem.len() < media_stem.len() || !file_stem.is_char_boundary(media_stem.len()) || !file_stem[..media_stem.len()].eq_ignore_ascii_case(media_stem) {
        return None;
    }
    let suffix = &file_stem[media_stem.len()..];
    (suffix.is_empty() || suffix.starts_with('.')).then_some(suffix)
}

fn is_subtitle_folder(folder: &Path) -> bool {
    folder.file_name().and_then(|n| n.to_str()).is_some_and(|name| SUBTITLE_FOLDER_NAMES.iter().any(|known| known.eq_ignore_ascii_case(name)))
}

fn subtitle_folders_in(folder: &Path) -> Vec<PathBuf> {
    entries_of(folder).filter(|path| path.is_dir() && is_subtitle_folder(path)).collect()
}

fn videos_in(folder: &Path, watch: &WatchConfig) -> Vec<PathBuf> {
    files_of_kind(folder, MediaKind::Video, watch)
}

fn subtitle_files_in(folder: &Path, watch: &WatchConfig) -> Vec<PathBuf> {
    files_of_kind(folder, MediaKind::Subtitle, watch)
}

fn files_of_kind(folder: &Path, kind: MediaKind, watch: &WatchConfig) -> Vec<PathBuf> {
    entries_of(folder)
        .filter(|path| path.is_file() && classify_path(path, watch) == Some(kind) && !looks_like_incomplete_file(path))
        .collect()
}

fn entries_of(folder: &Path) -> impl Iterator<Item = PathBuf> {
    std::fs::read_dir(folder).into_iter().flatten().flatten().map(|entry| entry.path())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create(root: &Path, relative_paths: &[&str]) {
        for relative in relative_paths {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "x").unwrap();
        }
    }

    fn sidecar_names(media: &Path) -> Vec<String> {
        let root = media.parent().unwrap();
        find_sidecar_subtitles(media, &WatchConfig::default())
            .iter()
            .map(|s| s.path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"))
            .collect()
    }

    #[test]
    fn sidecar_path_uses_language_code() {
        assert_eq!(sidecar_path_for_language(Path::new("/x/Movie (2020).mkv"), "de"), PathBuf::from("/x/Movie (2020).de.srt"));
    }

    #[test]
    fn finds_and_labels_sidecars_named_after_the_video() {
        let dir = tempfile::tempdir().unwrap();
        create(dir.path(), &["Show.S01E01.mkv", "Show.S01E02.mkv", "Show.S01E01.srt", "Show.S01E01.en.forced.srt", "Show.S01E01.ger.sdh.srt", "Show.S01E02.en.srt", "Other.srt"]);
        let found = find_sidecar_subtitles(&dir.path().join("Show.S01E01.mkv"), &WatchConfig::default());
        assert_eq!(found.len(), 3);
        let german = found.iter().find(|s| s.language.as_deref() == Some("de")).unwrap();
        assert!(german.hearing_impaired);
        let forced = found.iter().find(|s| s.forced).unwrap();
        assert_eq!(forced.language.as_deref(), Some("en"));
        assert!(found.iter().any(|s| s.language.is_none()));
    }

    #[test]
    fn a_lone_video_owns_its_subs_folder() {
        let dir = tempfile::tempdir().unwrap();
        create(dir.path(), &["Movie.2026.mp4", "Movie.2026.srt", "Subs/English.srt", "Subs/SDH.eng.srt", "Subs/Brazilian.por.srt", "Subs/2_English.srt"]);
        let media = dir.path().join("Movie.2026.mp4");
        assert_eq!(sidecar_names(&media), vec!["Movie.2026.srt", "Subs/2_English.srt", "Subs/Brazilian.por.srt", "Subs/English.srt", "Subs/SDH.eng.srt"]);
        let found = find_sidecar_subtitles(&media, &WatchConfig::default());
        let languages: Vec<Option<&str>> = found.iter().map(|s| s.language.as_deref()).collect();
        assert_eq!(languages, vec![None, Some("en"), Some("pt"), Some("en"), Some("en")]);
        assert!(found[4].hearing_impaired);
        assert_eq!(video_for_subtitle(&dir.path().join("Subs/English.srt"), &WatchConfig::default()), Some(media));
    }

    #[test]
    fn season_packs_match_subtitle_folders_by_episode_name() {
        let dir = tempfile::tempdir().unwrap();
        create(dir.path(), &["Show.S01E01.mkv", "Show.S01E02.mkv", "Subs/Show.S01E01/2_English.srt", "Subs/Show.S01E02/2_English.srt", "Subs/Unrelated.srt"]);
        let first = dir.path().join("Show.S01E01.mkv");
        assert_eq!(sidecar_names(&first), vec!["Subs/Show.S01E01/2_English.srt"]);
        let watch = WatchConfig::default();
        assert_eq!(video_for_subtitle(&dir.path().join("Subs/Show.S01E02/2_English.srt"), &watch), Some(dir.path().join("Show.S01E02.mkv")));
        assert_eq!(video_for_subtitle(&dir.path().join("Subs/Unrelated.srt"), &watch), None);
    }

    #[test]
    fn a_subtitle_without_a_video_belongs_to_nothing() {
        let dir = tempfile::tempdir().unwrap();
        create(dir.path(), &["Lecture.en.srt"]);
        assert_eq!(video_for_subtitle(&dir.path().join("Lecture.en.srt"), &WatchConfig::default()), None);
    }
}
