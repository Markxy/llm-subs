//! HTTP plumbing shared by the providers.

use std::time::Duration;

use serde_json::Value;

use super::types::LlmError;

/// A client for streamed answers: instead of capping the whole request, which can
/// legitimately take many minutes for a full film, it gives up when the server
/// goes quiet for `idle_timeout`.
pub fn streaming_client(idle_timeout: Duration) -> Result<reqwest::Client, LlmError> {
    super::install_crypto_provider();
    reqwest::Client::builder()
        .read_timeout(idle_timeout)
        .connect_timeout(Duration::from_secs(20))
        .build()
        .map_err(LlmError::from_reqwest)
}

pub async fn error_from_response(response: reqwest::Response) -> LlmError {
    let status = response.status().as_u16();
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs);
    let body = response.text().await.unwrap_or_default();
    http_error(status, &body, retry_after)
}

pub fn http_error(status: u16, body: &str, retry_after: Option<Duration>) -> LlmError {
    let message = summarize_error_body(body);
    if looks_like_context_overflow(body) {
        return LlmError::ContextTooLong(message);
    }
    match status {
        401 | 403 => LlmError::Unauthorized { status, body: message },
        429 => LlmError::RateLimited { retry_after, body: message },
        _ => LlmError::Http { status, body: message },
    }
}

fn looks_like_context_overflow(body: &str) -> bool {
    let body = body.to_ascii_lowercase();
    ["context_length_exceeded", "maximum context length", "context window", "prompt is too long", "too many input tokens"]
        .iter()
        .any(|marker| body.contains(marker))
}

/// Providers wrap errors differently; pull out the human readable message when possible.
pub fn summarize_error_body(body: &str) -> String {
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        for path in [&["error", "message"][..], &["message"], &["error"], &["detail", "message"], &["detail"]] {
            let mut cursor = &value;
            for key in path {
                cursor = &cursor[*key];
            }
            if let Some(message) = cursor.as_str() {
                return message.chars().take(500).collect();
            }
        }
    }
    body.chars().take(500).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_error_messages() {
        assert_eq!(summarize_error_body(r#"{"error":{"message":"bad key"}}"#), "bad key");
        assert_eq!(summarize_error_body(r#"{"detail":"blocked"}"#), "blocked");
        assert_eq!(summarize_error_body("plain text"), "plain text");
    }

    #[test]
    fn recognises_context_overflow() {
        let body = r#"{"error":{"code":"context_length_exceeded","message":"too long"}}"#;
        assert!(matches!(http_error(400, body, None), LlmError::ContextTooLong(_)));
        assert!(matches!(http_error(400, r#"{"error":{"message":"bad"}}"#, None), LlmError::Http { status: 400, .. }));
    }
}
