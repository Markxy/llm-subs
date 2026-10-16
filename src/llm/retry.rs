use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::types::*;

/// Retries transient failures with exponential backoff, honouring `Retry-After`.
pub async fn complete_with_retries(
    model: &dyn ChatModel,
    request: ChatRequest,
    max_retries: u32,
    cancel: &CancellationToken,
) -> Result<ChatResponse, LlmError> {
    let mut attempt = 0;
    loop {
        let result = tokio::select! {
            result = model.complete(request.clone()) => result,
            _ = cancel.cancelled() => return Err(LlmError::Network("cancelled".into())),
        };
        match result {
            Ok(response) => return Ok(response),
            Err(error) if error.is_retryable() && attempt < max_retries => {
                let backoff = match &error {
                    LlmError::RateLimited { retry_after: Some(wait), .. } => *wait,
                    _ => Duration::from_secs(2u64.pow(attempt + 1)),
                }
                .min(Duration::from_secs(60));
                tracing::warn!(attempt, error = %error, wait_seconds = backoff.as_secs(), "retrying model request");
                tokio::select! {
                    _ = tokio::time::sleep(backoff) => {}
                    _ = cancel.cancelled() => return Err(LlmError::Network("cancelled".into())),
                }
                if let Some(listener) = &request.listener {
                    listener.restart();
                }
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    }
}
