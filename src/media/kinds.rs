use std::path::Path;

use crate::config::WatchConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Video,
    Subtitle,
}

/// Decides whether a path is something LLM Subs can translate, by extension.
pub fn classify_path(path: &Path, watch: &WatchConfig) -> Option<MediaKind> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    if watch.media_extensions.iter().any(|e| e.eq_ignore_ascii_case(&extension)) {
        Some(MediaKind::Video)
    } else if watch.subtitle_extensions.iter().any(|e| e.eq_ignore_ascii_case(&extension)) {
        Some(MediaKind::Subtitle)
    } else {
        None
    }
}

/// Marks the copy of a media file that is being remuxed with a new subtitle track.
pub const REMUX_TEMP_MARKER: &str = "llm-subs-tmp";

/// Temporary and partial files that downloaders, copy tools and the remux leave behind.
pub fn looks_like_incomplete_file(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    name.starts_with('.')
        || name.contains(&format!(".{REMUX_TEMP_MARKER}."))
        || name.starts_with("~$")
        || name.ends_with(".part")
        || name.ends_with(".crdownload")
        || name.ends_with(".tmp")
        || name.ends_with(".!qB")
        || name.ends_with(".download")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_by_extension() {
        let watch = WatchConfig::default();
        assert_eq!(classify_path(Path::new("a/Movie.MKV"), &watch), Some(MediaKind::Video));
        assert_eq!(classify_path(Path::new("a/Movie.srt"), &watch), Some(MediaKind::Subtitle));
        assert_eq!(classify_path(Path::new("a/notes.txt"), &watch), None);
        assert!(looks_like_incomplete_file(Path::new("a/Movie.mkv.part")));
        assert!(looks_like_incomplete_file(Path::new("a/Movie.llm-subs-tmp.mkv")));
    }
}
