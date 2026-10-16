//! Knowledge about media files on disk: what counts as media, how sidecar subtitle
//! files are named, which subtitle track to translate and how to embed the result.

mod embed;
pub mod kinds;
mod sidecar;
mod source_selection;

pub use embed::embed_subtitle_track;
pub use kinds::{classify_path, MediaKind};
pub use sidecar::{find_sidecar_subtitles, sidecar_path_for_language, video_for_subtitle, SidecarSubtitle};
pub use source_selection::{select_source, SelectionReport, SubtitleSource};
