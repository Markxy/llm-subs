//! Subtitle data model, SRT parsing and the text normalisation that sits between
//! raw subtitle files and the translator.

mod cue;
mod srt;
mod text;
mod wrap;

pub use cue::{Cue, Timecode};
pub use srt::{parse_srt, serialize_srt, SrtParseError};
pub use text::{
    dialogue_text, normalise_for_translation, split_position_tag, CUE_BREAK_MARKER,
    LINE_BREAK_MARKER, translation_to_lines,
};
pub use wrap::wrap_lines;
