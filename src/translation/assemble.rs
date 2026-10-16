use std::collections::HashMap;

use crate::config::LineWrapConfig;
use crate::subtitles::{split_position_tag, translation_to_lines, Cue, CUE_BREAK_MARKER};

use super::TranslationUnit;

#[derive(Debug, Default, Clone)]
pub struct AssemblyReport {
    pub translated_cues: usize,
    /// Cues whose text was kept in the source language because no translation arrived.
    pub untranslated_cues: usize,
}

/// Rebuilds the cue list with translated text, keeping timing and positioning tags.
pub fn assemble_translated_cues(
    cues: &[Cue],
    units: &[TranslationUnit],
    translations: &HashMap<usize, String>,
    wrap: &LineWrapConfig,
) -> (Vec<Cue>, AssemblyReport) {
    let mut covered = vec![false; cues.len()];
    let mut translated_parts: Vec<Option<String>> = vec![None; cues.len()];
    for unit in units {
        let pieces = translations.get(&unit.id).map(|translation| split_into_parts(translation, &unit.source_parts()));
        for &first_cue in &unit.occurrences {
            for part in 0..unit.parts {
                covered[first_cue + part] = true;
                translated_parts[first_cue + part] = pieces.as_ref().map(|pieces| pieces[part].clone());
            }
        }
    }

    let mut report = AssemblyReport::default();
    let mut output: Vec<Cue> = Vec::with_capacity(cues.len());
    for (index, cue) in cues.iter().enumerate() {
        match &translated_parts[index] {
            Some(piece) if piece.is_empty() => {
                report.translated_cues += 1;
                if let Some(previous_part) = output.last_mut() {
                    previous_part.end = cue.end;
                }
            }
            Some(piece) => {
                report.translated_cues += 1;
                let (prefix, _) = split_position_tag(&cue.text);
                let lines = translation_to_lines(piece, wrap);
                output.push(Cue::new(cue.start, cue.end, format!("{prefix}{}", lines.join("\n"))));
            }
            None => {
                if covered[index] {
                    report.untranslated_cues += 1;
                }
                output.push(cue.clone());
            }
        }
    }
    (output, report)
}

/// Divides a translation over the cues its source parts came from. The model marks
/// the divisions itself; when it did not keep one marker per division, the words are
/// shared out in proportion to the length of each source part instead.
fn split_into_parts(translation: &str, source_parts: &[&str]) -> Vec<String> {
    let marker = CUE_BREAK_MARKER.trim();
    if source_parts.len() == 1 {
        return vec![translation.replace(marker, " ").trim().to_string()];
    }
    let pieces: Vec<String> = translation.split(marker).map(|piece| piece.trim().to_string()).collect();
    if pieces.len() == source_parts.len() && pieces.iter().all(|piece| !piece.is_empty()) {
        return pieces;
    }
    share_out_words(&translation.replace(marker, " "), source_parts)
}

fn share_out_words(text: &str, source_parts: &[&str]) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let source_lengths: Vec<usize> = source_parts.iter().map(|part| part.chars().count().max(1)).collect();
    let source_total: usize = source_lengths.iter().sum();
    let text_total: usize = words.iter().map(|word| word.chars().count() + 1).sum();
    let mut pieces = Vec::with_capacity(source_parts.len());
    let mut next_word = 0;
    for (part, source_length) in source_lengths.iter().enumerate() {
        let parts_after = source_lengths.len() - part - 1;
        let first_word = next_word;
        let last_word_allowed = words.len().saturating_sub(parts_after).max((first_word + 1).min(words.len()));
        let share = text_total * source_length / source_total;
        let mut piece_length = 0;
        while next_word < last_word_allowed
            && (parts_after == 0 || next_word == first_word || piece_length + words[next_word].chars().count() / 2 < share)
        {
            piece_length += words[next_word].chars().count() + 1;
            next_word += 1;
        }
        pieces.push(words[first_word..next_word].join(" "));
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subtitles::Timecode;

    fn timed(start_ms: u64, end_ms: u64, text: &str) -> Cue {
        Cue::new(Timecode::from_ms(start_ms), Timecode::from_ms(end_ms), text)
    }

    fn unit(text: &str, occurrences: Vec<usize>, parts: usize) -> TranslationUnit {
        TranslationUnit { id: 0, text: text.into(), occurrences, parts }
    }

    #[test]
    fn keeps_timing_and_position_tags() {
        let cues = vec![timed(0, 900, "{\\an8}Hello\nthere")];
        let units = vec![unit("Hello | there", vec![0], 1)];
        let translations = HashMap::from([(0, "Hallo | du".to_string())]);
        let (output, report) = assemble_translated_cues(&cues, &units, &translations, &LineWrapConfig::default());
        assert_eq!(output[0].text, "{\\an8}Hallo\ndu");
        assert_eq!(output[0].end, Timecode::from_ms(900));
        assert_eq!(report.translated_cues, 1);
    }

    #[test]
    fn a_sentence_is_divided_where_the_model_marked_it() {
        let cues = vec![timed(0, 1000, "You have to figure out\nhow to protect yourself"), timed(1001, 2000, "from the Acolytes.")];
        let units = vec![unit("You have to figure out how to protect yourself ‖ from the Acolytes.", vec![0], 2)];
        let translations = HashMap::from([(0, "Pead välja mõtlema, kuidas end ‖ käsilaste eest kaitsta.".to_string())]);
        let (output, report) = assemble_translated_cues(&cues, &units, &translations, &LineWrapConfig::default());
        assert_eq!(output[0].text, "Pead välja mõtlema, kuidas end");
        assert_eq!(output[1].text, "käsilaste eest kaitsta.");
        assert_eq!(report.translated_cues, 2);
    }

    #[test]
    fn a_missing_marker_shares_the_words_out_by_length() {
        let translation = "Pead välja mõtlema, kuidas end käsilaste eest kaitsta.";
        let pieces = split_into_parts(translation, &["You have to figure out how to protect yourself", "from the Acolytes."]);
        assert_eq!(pieces.len(), 2);
        assert!(pieces[0].len() > pieces[1].len() && !pieces[1].is_empty());
        assert_eq!(pieces.join(" "), translation);
        let too_few_words = split_into_parts("Jah.", &["Well,", "yes."]);
        assert_eq!(too_few_words, vec!["Jah.", ""]);
    }

    #[test]
    fn an_empty_part_hands_its_time_to_the_part_before() {
        let cues = vec![timed(0, 1000, "Well,"), timed(1000, 2000, "yes.")];
        let units = vec![unit("Well, ‖ yes.", vec![0], 2)];
        let translations = HashMap::from([(0, "Jah.".to_string())]);
        let (output, _) = assemble_translated_cues(&cues, &units, &translations, &LineWrapConfig::default());
        assert_eq!(output.len(), 1);
        assert_eq!((output[0].text.as_str(), output[0].end), ("Jah.", Timecode::from_ms(2000)));
    }
}
