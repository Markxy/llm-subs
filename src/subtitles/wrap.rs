//! Splits translated text into display lines that fit typical subtitle limits.

/// Wraps `text` into at most `max_lines` lines of roughly `max_line_length`
/// characters, preferring balanced lines and breaks after punctuation.
pub fn wrap_lines(text: &str, max_line_length: usize, max_lines: usize) -> Vec<String> {
    let max_line_length = max_line_length.max(10);
    let max_lines = max_lines.max(1);
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }
    let total_length: usize = words.iter().map(|w| w.chars().count()).sum::<usize>() + words.len() - 1;
    if total_length <= max_line_length || words.len() == 1 {
        return vec![words.join(" ")];
    }

    if max_lines >= 2
        && total_length <= max_line_length * 2
        && let Some(two_lines) = best_two_line_split(&words, max_line_length)
    {
        return two_lines;
    }
    greedy_fill(&words, max_line_length, max_lines)
}

/// Chooses the break that keeps both lines under the limit and closest in length,
/// with a bonus for breaking after sentence punctuation.
fn best_two_line_split(words: &[&str], max_line_length: usize) -> Option<Vec<String>> {
    let mut best: Option<(i64, usize)> = None;
    for split in 1..words.len() {
        let first = words[..split].join(" ");
        let second = words[split..].join(" ");
        let first_len = first.chars().count();
        let second_len = second.chars().count();
        if first_len > max_line_length || second_len > max_line_length {
            continue;
        }
        let imbalance = (first_len as i64 - second_len as i64).abs();
        let ends_sentence = first.ends_with(['.', ',', '!', '?', ';', ':']);
        let score = imbalance - if ends_sentence { 12 } else { 0 };
        if best.is_none_or(|(best_score, _)| score < best_score) {
            best = Some((score, split));
        }
    }
    best.map(|(_, split)| vec![words[..split].join(" "), words[split..].join(" ")])
}

fn greedy_fill(words: &[&str], max_line_length: usize, max_lines: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in words {
        let candidate_len = current.chars().count() + if current.is_empty() { 0 } else { 1 } + word.chars().count();
        if !current.is_empty() && candidate_len > max_line_length && lines.len() + 1 < max_lines {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_stays_on_one_line() {
        assert_eq!(wrap_lines("Hello there", 42, 2), vec!["Hello there"]);
    }

    #[test]
    fn long_text_splits_in_balanced_halves() {
        let lines = wrap_lines("I never thought I would see you again after all these years, honestly.", 42, 2);
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| l.chars().count() <= 42));
    }

    #[test]
    fn prefers_break_after_punctuation() {
        let lines = wrap_lines("We should go now. The others are waiting outside.", 42, 2);
        assert_eq!(lines[0], "We should go now.");
    }

    #[test]
    fn overflow_fills_greedily_up_to_max_lines() {
        let text = "one two three four five six seven eight nine ten eleven twelve thirteen fourteen";
        let lines = wrap_lines(text, 20, 3);
        assert_eq!(lines.len(), 3);
    }
}
