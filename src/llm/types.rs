use std::fmt;
use std::future::Future;
use std::ops::AddAssign;
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

impl Role {
    pub fn api_name(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: Role,
    pub text: String,
}

impl ChatMessage {
    pub fn user(text: impl Into<String>) -> Self {
        Self { role: Role::User, text: text.into() }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self { role: Role::Assistant, text: text.into() }
    }
}

/// Receives the answer while it is being generated.
pub trait StreamListener: Send + Sync {
    fn text(&self, delta: &str);

    /// A retry starts from scratch; text received from the failed attempt is void.
    fn restart(&self) {}
}

impl fmt::Debug for dyn StreamListener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("StreamListener")
    }
}

#[derive(Debug, Clone, Default)]
pub struct ChatRequest {
    pub system: Option<String>,
    pub messages: Vec<ChatMessage>,
    pub max_output_tokens: Option<u32>,
    pub temperature: Option<f32>,
    /// Reasoning effort in the provider's own vocabulary.
    pub effort: Option<String>,
    pub listener: Option<Arc<dyn StreamListener>>,
}

impl ChatRequest {
    pub(crate) fn stream_text(&self, delta: &str) {
        if let Some(listener) = &self.listener {
            listener.text(delta);
        }
    }
}

/// Token counts of one or more requests. `input_tokens` includes cached input.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Part of `input_tokens` that was read from the provider's prompt cache.
    pub cached_input_tokens: u64,
    /// Part of `output_tokens` spent on reasoning before the answer.
    pub reasoning_tokens: u64,
}

impl AddAssign for TokenUsage {
    fn add_assign(&mut self, other: Self) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cached_input_tokens += other.cached_input_tokens;
        self.reasoning_tokens += other.reasoning_tokens;
    }
}

impl TokenUsage {
    pub fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }

    /// `12.4k in (3.0k cached), 15.1k out (2.2k reasoning)`
    pub fn describe(&self) -> String {
        let mut text = format!("{} in", format_token_count(self.input_tokens));
        if self.cached_input_tokens > 0 {
            text.push_str(&format!(" ({} cached)", format_token_count(self.cached_input_tokens)));
        }
        text.push_str(&format!(", {} out", format_token_count(self.output_tokens)));
        if self.reasoning_tokens > 0 {
            text.push_str(&format!(" ({} reasoning)", format_token_count(self.reasoning_tokens)));
        }
        text
    }
}

pub fn format_token_count(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinishReason {
    Stop,
    /// The output limit was hit; the text is truncated.
    Length,
    Refusal,
    Other(String),
}

#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub text: String,
    pub usage: TokenUsage,
    pub finish: FinishReason,
}

#[derive(Debug, Error)]
pub enum LlmError {
    #[error("authentication failed ({status}): {body}")]
    Unauthorized { status: u16, body: String },
    #[error("rate limited by the provider")]
    RateLimited { retry_after: Option<std::time::Duration>, body: String },
    #[error("{0}")]
    UsageLimit(String),
    #[error("the request is larger than the model's context window: {0}")]
    ContextTooLong(String),
    #[error("provider returned HTTP {status}: {body}")]
    Http { status: u16, body: String },
    #[error("network error: {0}")]
    Network(String),
    #[error("the provider sent nothing for too long")]
    Timeout,
    #[error("unexpected response: {0}")]
    InvalidResponse(String),
    #[error("the model refused the request: {0}")]
    Refused(String),
    #[error("{0}")]
    SignInRequired(String),
    #[error("{0}")]
    Unsupported(String),
}

impl LlmError {
    pub fn is_retryable(&self) -> bool {
        match self {
            LlmError::RateLimited { .. } | LlmError::Network(_) | LlmError::Timeout => true,
            LlmError::Http { status, .. } => *status >= 500 || *status == 408 || *status == 409,
            _ => false,
        }
    }

    pub fn from_reqwest(error: reqwest::Error) -> Self {
        if error.is_timeout() {
            LlmError::Timeout
        } else {
            LlmError::Network(error.to_string())
        }
    }
}

pub type LlmFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, LlmError>> + Send + 'a>>;

pub trait ChatModel: Send + Sync {
    fn complete<'a>(&'a self, request: ChatRequest) -> LlmFuture<'a, ChatResponse>;

    fn describe(&self) -> String;
}

impl fmt::Debug for dyn ChatModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.describe())
    }
}

/// Removes `<think>...</think>` blocks that local reasoning models emit inline.
pub fn strip_thinking_blocks(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<think>") {
        result.push_str(&rest[..start]);
        match rest[start..].find("</think>") {
            Some(end) => rest = &rest[start + end + "</think>".len()..],
            None => {
                rest = "";
            }
        }
    }
    result.push_str(rest);
    result.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_thinking_blocks() {
        assert_eq!(strip_thinking_blocks("<think>hmm</think>\n#1: Hallo"), "#1: Hallo");
        assert_eq!(strip_thinking_blocks("plain"), "plain");
    }

    #[test]
    fn describes_usage() {
        let usage = TokenUsage { input_tokens: 12_400, output_tokens: 900, cached_input_tokens: 3_000, reasoning_tokens: 0 };
        assert_eq!(usage.describe(), "12.4k in (3.0k cached), 900 out");
    }
}
