use crate::subtitles::{CUE_BREAK_MARKER, LINE_BREAK_MARKER};

use super::TranslationUnit;

pub struct PromptContext<'a> {
    pub target_language_name: &'a str,
    pub source_language_name: Option<&'a str>,
    pub extra_instructions: &'a str,
}

pub fn system_prompt(context: &PromptContext<'_>) -> String {
    let source = context.source_language_name.map(|name| format!(" from {name}")).unwrap_or_default();
    let mut prompt = format!(
        "You are a professional subtitle translator. Translate subtitle lines{source} into {target}.\n\n\
You receive the dialogue of a film or episode, all of it or a run of consecutive lines at a time. Before writing anything, \
read everything you were given: work out the setting, the characters, who speaks to whom, their relationships and how \
they address each other, and the recurring names, terms and jokes. Then translate with that picture in mind and keep \
every choice consistent from the first line to the last.\n\n\
Rules:\n\
- Produce natural, idiomatic {target} as a native subtitler would write it: match register, slang, humour and tone; \
never translate word by word.\n\
- Sentences run across lines; pronouns, jokes and callbacks point backwards and forwards. Settle ambiguity, gender, \
formality and who is speaking to whom from the surrounding dialogue.\n\
- Keep each line's meaning to its own line. Never merge, split, reorder or drop lines.\n\
- A line containing `{cue_marker}` is one sentence that the screen shows in consecutive parts. Translate it as one \
natural sentence, then put back exactly as many `{cue_marker}` as the source line has, where the screen should move on, \
so each part reads well on its own. Every word belongs to exactly one part.\n\
- When a sentence runs on into the next numbered line, divide your translation between those lines the same way and \
never repeat words from one line in the next.\n\
- Keep inline tags like <i>...</i> where they were, keep numbers, and keep the `{marker}` separator where a line break \
belongs (dialogue dashes stay on separate lines). Otherwise write each translation as one line.\n\
- Keep names and untranslatable terms as in the source. Do not add explanations.\n\
- Subtitles must be concise: aim for a similar length to the source.\n\
- Answer with exactly one line per input line, in the form `#<id>: <translation>`, in the order given, and nothing else.",
        target = context.target_language_name,
        marker = LINE_BREAK_MARKER.trim(),
        cue_marker = CUE_BREAK_MARKER.trim(),
    );
    if !context.extra_instructions.trim().is_empty() {
        prompt.push_str("\n\nAdditional instructions from the viewer:\n");
        prompt.push_str(context.extra_instructions.trim());
    }
    prompt
}

#[derive(Debug, Clone)]
pub struct ContinuityLine {
    pub source: String,
    pub translation: Option<String>,
}

pub fn batch_message(units: &[&TranslationUnit], continuity: &[ContinuityLine]) -> String {
    let mut message = String::new();
    if !continuity.is_empty() {
        message.push_str("Continuity - the lines immediately before this batch (do not translate these):\n");
        for line in continuity {
            match &line.translation {
                Some(translation) => message.push_str(&format!("  {} => {}\n", line.source, translation)),
                None => message.push_str(&format!("  {}\n", line.source)),
            }
        }
        message.push('\n');
    }
    message.push_str(&format!("Translate these {} lines:\n", units.len()));
    for unit in units {
        message.push_str(&format!("#{}: {}\n", unit.id, unit.text));
    }
    message
}
