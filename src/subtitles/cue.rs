use std::fmt;

/// A point in time inside the media, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Timecode(pub u64);

impl Timecode {
    pub fn from_ms(ms: u64) -> Self {
        Self(ms)
    }

    pub fn from_parts(hours: u64, minutes: u64, seconds: u64, millis: u64) -> Self {
        Self(((hours * 60 + minutes) * 60 + seconds) * 1000 + millis)
    }

    pub fn as_ms(self) -> u64 {
        self.0
    }

    /// `HH:MM:SS,mmm` as used by SRT.
    pub fn to_srt(self) -> String {
        let ms = self.0 % 1000;
        let total_seconds = self.0 / 1000;
        let seconds = total_seconds % 60;
        let minutes = (total_seconds / 60) % 60;
        let hours = total_seconds / 3600;
        format!("{hours:02}:{minutes:02}:{seconds:02},{ms:03}")
    }
}

impl fmt::Display for Timecode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_srt())
    }
}

/// One subtitle entry. `text` keeps the original line breaks and inline tags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cue {
    pub start: Timecode,
    pub end: Timecode,
    pub text: String,
}

impl Cue {
    pub fn new(start: Timecode, end: Timecode, text: impl Into<String>) -> Self {
        Self { start, end, text: text.into() }
    }
}
