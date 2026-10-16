use thiserror::Error;

use super::{Cue, Timecode};

#[derive(Debug, Error)]
pub enum SrtParseError {
    #[error("no subtitle entries found")]
    Empty,
    #[error("line {line}: unreadable timing '{text}'")]
    BadTiming { line: usize, text: String },
}

/// Parses SubRip text. Tolerates a BOM, CRLF endings, missing index numbers,
/// `.` as the millisecond separator and coordinates after the timing line.
pub fn parse_srt(input: &str) -> Result<Vec<Cue>, SrtParseError> {
    let input = input.strip_prefix('\u{feff}').unwrap_or(input);
    let lines: Vec<&str> = input.lines().map(|l| l.trim_end_matches('\r')).collect();

    let mut cues = Vec::new();
    let mut position = 0;
    while position < lines.len() {
        let Some(timing_offset) = lines[position..].iter().position(|l| l.contains("-->")) else {
            break;
        };
        let timing_line_number = position + timing_offset;
        let (start, end) = parse_timing(lines[timing_line_number]).ok_or_else(|| SrtParseError::BadTiming {
            line: timing_line_number + 1,
            text: lines[timing_line_number].to_string(),
        })?;

        let mut text_lines = Vec::new();
        let mut cursor = timing_line_number + 1;
        while cursor < lines.len() {
            let line = lines[cursor];
            if line.trim().is_empty() {
                break;
            }
            let next_is_timing = lines.get(cursor + 1).is_some_and(|l| l.contains("-->"));
            if next_is_timing && line.trim().chars().all(|c| c.is_ascii_digit()) {
                break;
            }
            text_lines.push(line.trim());
            cursor += 1;
        }
        position = cursor;

        let text = text_lines.join("\n");
        if !text.trim().is_empty() {
            cues.push(Cue::new(start, end, text));
        }
    }

    if cues.is_empty() { Err(SrtParseError::Empty) } else { Ok(cues) }
}

fn parse_timing(line: &str) -> Option<(Timecode, Timecode)> {
    let (left, right) = line.split_once("-->")?;
    let start = parse_timecode(left.trim())?;
    let end_token = right.split_whitespace().next()?;
    let end = parse_timecode(end_token)?;
    Some((start, end))
}

fn parse_timecode(token: &str) -> Option<Timecode> {
    let (clock, millis) = match token.rsplit_once([',', '.']) {
        Some((clock, millis)) => (clock, millis),
        None => (token, "0"),
    };
    let millis = parse_millis(millis)?;
    let mut parts: Vec<u64> = clock.split(':').map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
    while parts.len() < 3 {
        parts.insert(0, 0);
    }
    if parts.len() != 3 {
        return None;
    }
    Some(Timecode::from_parts(parts[0], parts[1], parts[2], millis))
}

fn parse_millis(digits: &str) -> Option<u64> {
    if digits.is_empty() || digits.len() > 3 || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let value: u64 = digits.parse().ok()?;
    Some(value * 10u64.pow(3 - digits.len() as u32))
}

pub fn serialize_srt(cues: &[Cue]) -> String {
    let mut out = String::new();
    for (index, cue) in cues.iter().enumerate() {
        out.push_str(&(index + 1).to_string());
        out.push('\n');
        out.push_str(&format!("{} --> {}\n", cue.start.to_srt(), cue.end.to_srt()));
        out.push_str(cue.text.trim_end());
        out.push_str("\n\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standard_file() {
        let text = "\u{feff}1\r\n00:00:01,000 --> 00:00:02,500\r\nHello\r\nWorld\r\n\r\n2\r\n00:00:03,000 --> 00:00:04,000 X1:0\r\n- Hi.\r\n- Bye.\r\n";
        let cues = parse_srt(text).unwrap();
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].start, Timecode::from_ms(1000));
        assert_eq!(cues[0].end, Timecode::from_ms(2500));
        assert_eq!(cues[0].text, "Hello\nWorld");
        assert_eq!(cues[1].text, "- Hi.\n- Bye.");
    }

    #[test]
    fn tolerates_missing_blank_lines_and_dot_millis() {
        let text = "1\n00:00:01.000 --> 00:00:02.000\nFirst\n2\n00:00:03.00 --> 00:00:04.5\nSecond\n";
        let cues = parse_srt(text).unwrap();
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[1].start, Timecode::from_ms(3000));
        assert_eq!(cues[1].end, Timecode::from_ms(4500));
        assert_eq!(cues[1].text, "Second");
    }

    #[test]
    fn serialises_back_to_srt() {
        let cues = vec![Cue::new(Timecode::from_ms(61_005), Timecode::from_ms(62_000), "Line")];
        assert_eq!(serialize_srt(&cues), "1\n00:01:01,005 --> 00:01:02,000\nLine\n\n");
    }

    #[test]
    fn empty_input_is_an_error() {
        assert!(matches!(parse_srt("nothing here"), Err(SrtParseError::Empty)));
    }
}
