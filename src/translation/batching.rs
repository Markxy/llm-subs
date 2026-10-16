use super::TranslationUnit;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
    pub unit_ids: Vec<usize>,
}

/// Groups units in order, closing a batch when either the line or the character
/// budget would be exceeded.
pub fn make_batches(units: &[TranslationUnit], lines_per_request: usize, max_chars_per_request: usize) -> Vec<Batch> {
    let lines_per_request = lines_per_request.max(1);
    let max_chars = max_chars_per_request.max(100);
    let mut batches = Vec::new();
    let mut current = Vec::new();
    let mut current_chars = 0;
    for unit in units {
        let chars = unit.char_count() + 8;
        if !current.is_empty() && (current.len() >= lines_per_request || current_chars + chars > max_chars) {
            batches.push(Batch { unit_ids: std::mem::take(&mut current) });
            current_chars = 0;
        }
        current.push(unit.id);
        current_chars += chars;
    }
    if !current.is_empty() {
        batches.push(Batch { unit_ids: current });
    }
    batches
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(id: usize, text: &str) -> TranslationUnit {
        TranslationUnit { id, text: text.to_string(), occurrences: vec![id], parts: 1 }
    }

    #[test]
    fn splits_on_line_count_and_char_budget() {
        let units: Vec<TranslationUnit> = (0..10).map(|i| unit(i, "twelve chars")).collect();
        let by_lines = make_batches(&units, 4, 10_000);
        assert_eq!(by_lines.len(), 3);
        let by_chars = make_batches(&units, 100, 200);
        assert_eq!(by_chars.len(), 1);
        let by_smaller_budget = make_batches(&units, 100, 100);
        assert_eq!(by_smaller_budget.len(), 2);
        assert_eq!(by_smaller_budget.iter().map(|b| b.unit_ids.len()).sum::<usize>(), 10);
    }
}
