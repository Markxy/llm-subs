//! Reads `text/event-stream` bodies: the streaming format of every supported provider.

use std::collections::VecDeque;

use super::types::LlmError;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SseEvent {
    /// The `event:` field; empty when the server only sends `data:` lines.
    pub event: String,
    pub data: String,
}

/// Turns arbitrary byte chunks into complete events.
#[derive(Default)]
pub struct SseParser {
    buffer: Vec<u8>,
    pending: SseEvent,
}

impl SseParser {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        self.buffer.extend_from_slice(bytes);
        let mut events = Vec::new();
        while let Some(newline) = self.buffer.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=newline).collect();
            events.extend(self.feed_line(&line));
        }
        events
    }

    /// Flushes an event the server did not terminate with a blank line.
    pub fn finish(&mut self) -> Option<SseEvent> {
        let rest = std::mem::take(&mut self.buffer);
        if !rest.is_empty()
            && let Some(event) = self.feed_line(&rest)
        {
            return Some(event);
        }
        self.take_pending()
    }

    fn feed_line(&mut self, raw: &[u8]) -> Option<SseEvent> {
        let line = String::from_utf8_lossy(raw);
        let line = line.trim_end_matches(['\n', '\r']);
        if line.is_empty() {
            return self.take_pending();
        }
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => self.pending.event = value.to_string(),
            "data" => {
                if !self.pending.data.is_empty() {
                    self.pending.data.push('\n');
                }
                self.pending.data.push_str(value);
            }
            _ => {}
        }
        None
    }

    fn take_pending(&mut self) -> Option<SseEvent> {
        let event = std::mem::take(&mut self.pending);
        (!event.data.is_empty() || !event.event.is_empty()).then_some(event)
    }
}

pub struct SseReader {
    response: reqwest::Response,
    parser: SseParser,
    ready: VecDeque<SseEvent>,
    finished: bool,
}

impl SseReader {
    pub fn new(response: reqwest::Response) -> Self {
        Self { response, parser: SseParser::default(), ready: VecDeque::new(), finished: false }
    }

    /// The next complete event, or `None` once the body has ended.
    pub async fn next_event(&mut self) -> Result<Option<SseEvent>, LlmError> {
        loop {
            if let Some(event) = self.ready.pop_front() {
                return Ok(Some(event));
            }
            if self.finished {
                return Ok(self.parser.finish());
            }
            match self.response.chunk().await.map_err(LlmError::from_reqwest)? {
                Some(chunk) => self.ready.extend(self.parser.push(&chunk)),
                None => self.finished = true,
            }
        }
    }
}

/// True when the response announces an event stream (or announces nothing, which
/// some servers do for valid streams).
pub fn is_event_stream(response: &reqwest::Response) -> bool {
    match response.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()) {
        Some(content_type) => content_type.starts_with("text/event-stream"),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_events_split_across_chunks() {
        let mut parser = SseParser::default();
        assert!(parser.push(b"event: delta\ndata: {\"a\"").is_empty());
        let events = parser.push(b":1}\r\n\r\n: keep-alive\n\ndata: one\ndata: two\n\ndata: [DONE]");
        assert_eq!(events, vec![
            SseEvent { event: "delta".into(), data: "{\"a\":1}".into() },
            SseEvent { event: String::new(), data: "one\ntwo".into() },
        ]);
        assert_eq!(parser.finish(), Some(SseEvent { event: String::new(), data: "[DONE]".into() }));
        assert_eq!(parser.finish(), None);
    }
}
