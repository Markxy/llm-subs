//! Conversions between the text stored in cues and the text shown to the model.

use crate::config::LineWrapConfig;

use super::wrap_lines;

/// Stands in for a line break inside a cue when it is sent to the model, so the
/// model can keep intentional breaks (dialogue dashes) without seeing raw newlines.
pub const LINE_BREAK_MARKER: &str = " | ";

/// Separates the parts of one sentence that the subtitles show over several cues, so
/// the model translates the sentence as a whole and says where each cue's part ends.
pub const CUE_BREAK_MARKER: &str = " ‖ ";

/// Splits leading ASS override blocks like `{\an8}` off the text so they can be
/// re-attached to the translation unchanged.
pub fn split_position_tag(text: &str) -> (String, &str) {
    let mut prefix = String::new();
    let mut rest = text;
    while let Some(after_open) = rest.strip_prefix('{') {
        let Some(close) = after_open.find('}') else { break };
        prefix.push_str(&rest[..close + 2]);
        rest = &rest[close + 2..];
    }
    (prefix, rest)
}

/// The single-line form of a cue that is sent to the model: override tags removed,
/// whitespace collapsed, line breaks replaced by [`LINE_BREAK_MARKER`].
pub fn dialogue_text(raw: &str) -> String {
    let without_overrides = strip_override_blocks(raw);
    without_overrides
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(LINE_BREAK_MARKER)
}

/// Key used to recognise identical lines that only differ in whitespace or case of tags.
pub fn normalise_for_translation(dialogue: &str) -> String {
    dialogue.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Turns a translated single line back into display lines, honouring the model's
/// line-break markers when present and wrapping otherwise.
pub fn translation_to_lines(translated: &str, wrap: &LineWrapConfig) -> Vec<String> {
    let trimmed = translated.trim();
    if trimmed.contains('|') {
        let pieces: Vec<String> = trimmed
            .split('|')
            .map(|piece| piece.trim().to_string())
            .filter(|piece| !piece.is_empty())
            .collect();
        if pieces.len() > 1 && pieces.iter().all(|p| p.chars().count() <= wrap.max_line_length.max(1) * 2) {
            return pieces;
        }
    }
    wrap_lines(&trimmed.replace('|', " "), wrap.max_line_length, wrap.max_lines)
}

fn strip_override_blocks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut depth = 0usize;
    for ch in text.chars() {
        match ch {
            '{' => depth += 1,
            '}' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_position_tags() {
        let (prefix, rest) = split_position_tag("{\\an8}{\\i1}Sign text");
        assert_eq!(prefix, "{\\an8}{\\i1}");
        assert_eq!(rest, "Sign text");
        assert_eq!(split_position_tag("plain"), (String::new(), "plain"));
    }

    #[test]
    fn dialogue_text_flattens_lines() {
        assert_eq!(dialogue_text("{\\an8}- Where?\n-  Home. "), "- Where? | - Home.");
    }

    #[test]
    fn translation_lines_use_markers_when_present() {
        let wrap = LineWrapConfig::default();
        assert_eq!(translation_to_lines("- Wo? | - Zuhause.", &wrap), vec!["- Wo?", "- Zuhause."]);
        assert_eq!(translation_to_lines("Kurz.", &wrap), vec!["Kurz."]);
    }
}
