use std::collections::HashMap;

/// Reads `#12: text` lines from model output. Lines without an id continue the
/// previous translation, which handles models that wrap long lines.
pub fn parse_numbered_lines(text: &str) -> HashMap<usize, String> {
    let mut translations: HashMap<usize, String> = HashMap::new();
    let mut last_id = None;
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with("```") {
            continue;
        }
        match split_id(line) {
            Some((id, rest)) => {
                translations.insert(id, rest.trim().to_string());
                last_id = Some(id);
            }
            None => {
                if let Some(id) = last_id
                    && let Some(existing) = translations.get_mut(&id)
                {
                    existing.push(' ');
                    existing.push_str(line);
                }
            }
        }
    }
    translations.retain(|_, value| !value.is_empty());
    translations
}

fn split_id(line: &str) -> Option<(usize, &str)> {
    let without_markup = line.trim_start_matches(['*', '-', ' ', '`']);
    let digits_start = without_markup.strip_prefix('#').unwrap_or(without_markup);
    let digits_len = digits_start.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits_len == 0 || digits_len > 6 {
        return None;
    }
    let (digits, rest) = digits_start.split_at(digits_len);
    let rest = rest.trim_start_matches(['*', '`']).trim_start();
    let rest = rest.strip_prefix(':').or_else(|| rest.strip_prefix('.')).or_else(|| rest.strip_prefix(')'))?;
    Some((digits.parse().ok()?, rest.trim_start_matches(['*', '`', ' '])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_numbered_lines_in_various_styles() {
        let text = "Here you go:\n#1: Hallo\n**#2:** Welt\n3: Drei\ncontinued\n#4: Vier\nno id here\n";
        let parsed = parse_numbered_lines(text);
        assert_eq!(parsed[&1], "Hallo");
        assert_eq!(parsed[&2], "Welt");
        assert_eq!(parsed[&3], "Drei continued");
        assert_eq!(parsed[&4], "Vier no id here");
        assert_eq!(parsed.len(), 4);
    }
}
