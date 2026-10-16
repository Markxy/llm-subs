//! Anthropic Messages API.

use std::time::Duration;

use serde_json::{json, Value};

use super::http::{error_from_response, streaming_client, summarize_error_body};
use super::sse::SseReader;
use super::types::*;

const API_VERSION: &str = "2023-06-01";
/// `max_tokens` is mandatory for this API. Current models write up to 128k tokens;
/// this leaves room for a whole film's translation plus the model's thinking.
const DEFAULT_MAX_TOKENS: u32 = 64_000;

pub struct AnthropicChatModel {
    client: reqwest::Client,
    endpoint: String,
    api_key: String,
    model: String,
    extra_headers: Vec<(String, String)>,
}

impl AnthropicChatModel {
    pub fn new(
        base_url: &str,
        api_key: String,
        model: String,
        extra_headers: Vec<(String, String)>,
        idle_timeout: Duration,
    ) -> Result<Self, LlmError> {
        let base = base_url.trim_end_matches('/');
        let endpoint = if base.ends_with("/v1") { format!("{base}/messages") } else { format!("{base}/v1/messages") };
        Ok(Self { client: streaming_client(idle_timeout)?, endpoint, api_key, model, extra_headers })
    }

    fn build_body(&self, request: &ChatRequest) -> Value {
        let messages: Vec<Value> =
            request.messages.iter().map(|message| json!({ "role": message.role.api_name(), "content": message.text })).collect();
        let mut body = json!({
            "model": self.model,
            "max_tokens": request.max_output_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            "messages": messages,
            "stream": true,
        });
        if let Some(system) = &request.system {
            body["system"] = json!(system);
        }
        if let Some(temperature) = request.temperature {
            body["temperature"] = json!(temperature);
        }
        if let Some(effort) = request.effort.as_deref().filter(|e| !e.is_empty()) {
            body["output_config"] = json!({ "effort": effort });
        }
        body
    }
}

/// Accumulates the streamed events of one message.
#[derive(Default)]
struct StreamedMessage {
    text: String,
    usage: TokenUsage,
    stop_reason: Option<String>,
    refusal_explanation: Option<String>,
    complete: bool,
}

impl StreamedMessage {
    /// Applies one event and returns the answer text it added.
    fn apply_event(&mut self, event: &Value) -> Result<Option<String>, LlmError> {
        match event["type"].as_str().unwrap_or_default() {
            "message_start" => {
                let usage = &event["message"]["usage"];
                let cache_read = usage["cache_read_input_tokens"].as_u64().unwrap_or(0);
                let cache_write = usage["cache_creation_input_tokens"].as_u64().unwrap_or(0);
                self.usage.input_tokens = usage["input_tokens"].as_u64().unwrap_or(0) + cache_read + cache_write;
                self.usage.cached_input_tokens = cache_read;
                self.usage.output_tokens = usage["output_tokens"].as_u64().unwrap_or(0);
            }
            "content_block_delta" if event["delta"]["type"].as_str() == Some("text_delta") => {
                let delta = event["delta"]["text"].as_str().unwrap_or_default();
                self.text.push_str(delta);
                return Ok((!delta.is_empty()).then(|| delta.to_string()));
            }
            "message_delta" => {
                if let Some(reason) = event["delta"]["stop_reason"].as_str() {
                    self.stop_reason = Some(reason.to_string());
                }
                if let Some(explanation) = event["delta"]["stop_details"]["explanation"].as_str() {
                    self.refusal_explanation = Some(explanation.to_string());
                }
                if let Some(output) = event["usage"]["output_tokens"].as_u64() {
                    self.usage.output_tokens = output;
                }
            }
            "message_stop" => self.complete = true,
            "error" => {
                let message = summarize_error_body(&event.to_string());
                let status = if event["error"]["type"].as_str() == Some("overloaded_error") { 529 } else { 500 };
                return Err(LlmError::Http { status, body: message });
            }
            _ => {}
        }
        Ok(None)
    }

    fn finish(self) -> Result<ChatResponse, LlmError> {
        if !self.complete {
            return Err(LlmError::Network("the answer stream ended early".into()));
        }
        let finish = match self.stop_reason.as_deref() {
            Some("end_turn") | Some("stop_sequence") | None => FinishReason::Stop,
            Some("max_tokens") => FinishReason::Length,
            Some("refusal") => {
                return Err(LlmError::Refused(self.refusal_explanation.unwrap_or_else(|| "no explanation given".into())));
            }
            Some(other) => FinishReason::Other(other.to_string()),
        };
        Ok(ChatResponse { text: self.text, usage: self.usage, finish })
    }
}

impl ChatModel for AnthropicChatModel {
    fn complete<'a>(&'a self, request: ChatRequest) -> LlmFuture<'a, ChatResponse> {
        Box::pin(async move {
            let body = self.build_body(&request);
            let mut http_request = self
                .client
                .post(&self.endpoint)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", API_VERSION)
                .json(&body);
            for (name, value) in &self.extra_headers {
                http_request = http_request.header(name, value);
            }
            let response = http_request.send().await.map_err(LlmError::from_reqwest)?;
            if !response.status().is_success() {
                return Err(error_from_response(response).await);
            }
            let mut reader = SseReader::new(response);
            let mut message = StreamedMessage::default();
            while let Some(event) = reader.next_event().await? {
                let value: Value = serde_json::from_str(&event.data)
                    .map_err(|e| LlmError::InvalidResponse(format!("{e}: {}", event.data.chars().take(300).collect::<String>())))?;
                if let Some(delta) = message.apply_event(&value)? {
                    request.stream_text(&delta);
                }
            }
            message.finish()
        })
    }

    fn describe(&self) -> String {
        format!("{} via {}", self.model, self.endpoint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(events: &[&str]) -> Result<ChatResponse, LlmError> {
        let mut message = StreamedMessage::default();
        for event in events {
            message.apply_event(&serde_json::from_str(event).unwrap())?;
        }
        message.finish()
    }

    #[test]
    fn assembles_streamed_message() {
        let response = feed(&[
            r#"{"type":"message_start","message":{"usage":{"input_tokens":12,"cache_read_input_tokens":100,"output_tokens":1}}}"#,
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r##"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"#1: Hallo"}}"##,
            r#"{"type":"ping"}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":4}}"#,
            r#"{"type":"message_stop"}"#,
        ])
        .unwrap();
        assert_eq!(response.text, "#1: Hallo");
        assert_eq!(response.usage, TokenUsage { input_tokens: 112, output_tokens: 4, cached_input_tokens: 100, reasoning_tokens: 0 });
        assert_eq!(response.finish, FinishReason::Stop);
    }

    #[test]
    fn refusal_and_errors_become_errors() {
        let refusal = feed(&[
            r#"{"type":"message_delta","delta":{"stop_reason":"refusal","stop_details":{"type":"refusal","explanation":"nope"}},"usage":{"output_tokens":0}}"#,
            r#"{"type":"message_stop"}"#,
        ]);
        assert!(matches!(refusal, Err(LlmError::Refused(_))));
        let overloaded = feed(&[r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#]);
        assert!(matches!(overloaded, Err(LlmError::Http { status: 529, .. })));
        assert!(matches!(feed(&[r#"{"type":"message_start","message":{"usage":{}}}"#]), Err(LlmError::Network(_))));
    }
}
