use std::collections::HashMap;

use crate::subtitles::{dialogue_text, normalise_for_translation, split_position_tag, Cue, CUE_BREAK_MARKER, LINE_BREAK_MARKER};

/// Longest pause between two cues that still counts as one sentence carried on.
const MAX_SENTENCE_GAP_MS: u64 = 1000;
/// Most cues one sentence unit may cover.
const MAX_SENTENCE_CUES: usize = 3;

/// One distinct piece of text to translate. Identical lines (a recurring "Yes." or a
/// repeated song chorus) collapse into one unit so they are translated once and
/// consistently. A sentence the subtitles spread over consecutive cues is one unit
/// whose parts are separated by [`CUE_BREAK_MARKER`], so the model sees the whole
/// sentence and cannot repeat words across the parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslationUnit {
    pub id: usize,
    pub text: String,
    /// Index of the first cue of every place this text appears.
    pub occurrences: Vec<usize>,
    /// Consecutive cues each occurrence covers.
    pub parts: usize,
}

impl TranslationUnit {
    pub fn char_count(&self) -> usize {
        self.text.chars().count()
    }

    /// The source text of each cue the unit covers.
    pub fn source_parts(&self) -> Vec<&str> {
        if self.parts == 1 {
            return vec![self.text.as_str()];
        }
        self.text.split(CUE_BREAK_MARKER.trim()).map(str::trim).collect()
    }
}

pub fn build_units(cues: &[Cue]) -> Vec<TranslationUnit> {
    let mut units: Vec<TranslationUnit> = Vec::new();
    let mut lookup: HashMap<String, usize> = HashMap::new();
    let mut index = 0;
    while index < cues.len() {
        let parts = sentence_length_at(cues, index);
        let first_cue = index;
        index += parts;
        let texts: Vec<String> = cues[first_cue..first_cue + parts].iter().map(|cue| dialogue_text(&cue.text)).collect();
        if parts == 1 && (texts[0].is_empty() || !texts[0].chars().any(|c| c.is_alphabetic())) {
            continue;
        }
        let text = if parts == 1 {
            texts.into_iter().next().unwrap_or_default()
        } else {
            texts.iter().map(|part| part.replace(LINE_BREAK_MARKER, " ")).collect::<Vec<_>>().join(CUE_BREAK_MARKER)
        };
        let key = format!("{parts}:{}", normalise_for_translation(&text));
        match lookup.get(&key) {
            Some(&unit_id) => units[unit_id].occurrences.push(first_cue),
            None => {
                let id = units.len();
                lookup.insert(key, id);
                units.push(TranslationUnit { id, text, occurrences: vec![first_cue], parts });
            }
        }
    }
    units
}

/// How many cues from `start` belong to the sentence that begins there.
fn sentence_length_at(cues: &[Cue], start: usize) -> usize {
    let mut length = 1;
    while length < MAX_SENTENCE_CUES
        && let Some(next) = cues.get(start + length)
        && sentence_continues(&cues[start + length - 1], next)
    {
        length += 1;
    }
    length
}

/// True when `current` stops mid-sentence and `next` picks the same sentence up: the
/// first ends on a letter, digit or comma, the second starts in lowercase, both are
/// one speaker and the gap between them is short. Scripts without letter case never
/// match, which keeps them one cue per unit.
fn sentence_continues(current: &Cue, next: &Cue) -> bool {
    let (current_position, current_text) = split_position_tag(&current.text);
    let (next_position, next_text) = split_position_tag(&next.text);
    let current_text = without_markup(current_text);
    let next_text = without_markup(next_text);
    let ends_open = current_text.trim_end().chars().last().is_some_and(|c| c.is_alphanumeric() || c == ',');
    let starts_lowercase = next_text.trim_start().chars().next().is_some_and(char::is_lowercase);
    ends_open
        && starts_lowercase
        && current_position == next_position
        && next.start.as_ms() <= current.end.as_ms() + MAX_SENTENCE_GAP_MS
        && !has_dialogue_dash(&current_text)
        && !has_dialogue_dash(&next_text)
}

fn without_markup(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut inside_tag = false;
    for character in text.chars() {
        match character {
            '<' => inside_tag = true,
            '>' if inside_tag => inside_tag = false,
            _ if !inside_tag => plain.push(character),
            _ => {}
        }
    }
    plain
}

fn has_dialogue_dash(text: &str) -> bool {
    text.lines().any(|line| line.trim_start().starts_with(['-', '–', '—']))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subtitles::Timecode;

    fn cue(text: &str) -> Cue {
        Cue::new(Timecode::from_ms(0), Timecode::from_ms(1000), text)
    }

    fn timed(start_ms: u64, end_ms: u64, text: &str) -> Cue {
        Cue::new(Timecode::from_ms(start_ms), Timecode::from_ms(end_ms), text)
    }

    #[test]
    fn deduplicates_identical_lines_and_skips_non_text() {
        let cues = vec![cue("Yes."), cue("No."), cue("Yes. "), cue("♪♪"), cue("...")];
        let units = build_units(&cues);
        assert_eq!(units.len(), 2);
        assert_eq!(units[0].occurrences, vec![0, 2]);
    }

    #[test]
    fn a_sentence_split_over_cues_becomes_one_unit() {
        let cues = vec![
            timed(0, 1000, "You have to figure out\nhow to protect yourself"),
            timed(1001, 2000, "from the Acolytes."),
            timed(2500, 3000, "Every time you come\nin contact with them,"),
            timed(3100, 4000, "<i>they get stronger.</i>"),
        ];
        let units = build_units(&cues);
        assert_eq!(units.len(), 2);
        assert_eq!(units[0].text, "You have to figure out how to protect yourself ‖ from the Acolytes.");
        assert_eq!((units[0].occurrences.clone(), units[0].parts), (vec![0], 2));
        assert_eq!(units[1].source_parts(), vec!["Every time you come in contact with them,", "<i>they get stronger.</i>"]);
    }

    #[test]
    fn finished_sentences_dialogue_and_long_pauses_stay_apart() {
        let cues = vec![
            timed(0, 1000, "I know."),
            timed(1000, 2000, "you do?"),
            timed(2000, 3000, "- Wait for"),
            timed(3000, 4000, "me here."),
            timed(4000, 5000, "And then"),
            timed(8000, 9000, "nothing happened."),
        ];
        assert!(build_units(&cues).iter().all(|unit| unit.parts == 1));
    }
}
