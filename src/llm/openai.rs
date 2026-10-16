//! OpenAI chat completions protocol, also spoken by most local servers.

use std::time::Duration;

use serde_json::{json, Value};

use super::http::{error_from_response, streaming_client, summarize_error_body};
use super::sse::{is_event_stream, SseReader};
use super::types::*;

pub struct OpenAiChatModel {
    client: reqwest::Client,
    endpoint: String,
    api_key: String,
    model: String,
    extra_headers: Vec<(String, String)>,
    /// The official API renamed `max_tokens`, knows reasoning effort and reports usage
    /// in streams on request; compatible servers vary, so they get the plain request.
    official_api: bool,
}

impl OpenAiChatModel {
    pub fn new(
        base_url: &str,
        api_key: String,
        model: String,
        extra_headers: Vec<(String, String)>,
        official_api: bool,
        idle_timeout: Duration,
    ) -> Result<Self, LlmError> {
        Ok(Self {
            client: streaming_client(idle_timeout)?,
            endpoint: format!("{}/chat/completions", base_url.trim_end_matches('/')),
            api_key,
            model,
            extra_headers,
            official_api,
        })
    }

    fn build_body(&self, request: &ChatRequest) -> Value {
        let mut messages = Vec::new();
        if let Some(system) = &request.system {
            messages.push(json!({ "role": "system", "content": system }));
        }
        for message in &request.messages {
            messages.push(json!({ "role": message.role.api_name(), "content": message.text }));
        }

        let mut body = json!({ "model": self.model, "messages": messages, "stream": true });
        if self.official_api {
            body["stream_options"] = json!({ "include_usage": true });
        }
        if let Some(limit) = request.max_output_tokens {
            let key = if self.official_api { "max_completion_tokens" } else { "max_tokens" };
            body[key] = json!(limit);
        }
        if let Some(temperature) = request.temperature {
            body["temperature"] = json!(temperature);
        }
        if self.official_api
            && let Some(effort) = request.effort.as_deref().filter(|e| !e.is_empty())
        {
            body["reasoning_effort"] = json!(effort);
        }
        body
    }

    async fn read_stream(&self, response: reqwest::Response, request: &ChatRequest) -> Result<ChatResponse, LlmError> {
        let mut reader = SseReader::new(response);
        let mut answer = StreamedAnswer::default();
        while let Some(event) = reader.next_event().await? {
            if event.data.trim() == "[DONE]" {
                break;
            }
            let value: Value = serde_json::from_str(&event.data).map_err(|e| LlmError::InvalidResponse(format!("{e}: {}", truncate(&event.data))))?;
            if value.get("error").is_some() {
                return Err(LlmError::Http { status: 500, body: summarize_error_body(&event.data) });
            }
            if let Some(delta) = answer.apply_chunk(&value) {
                request.stream_text(&delta);
            }
        }
        Ok(answer.finish())
    }
}

/// Accumulates streamed chunks into one answer.
#[derive(Default)]
struct StreamedAnswer {
    text: String,
    finish_reason: Option<String>,
    usage: TokenUsage,
}

impl StreamedAnswer {
    /// Applies one chunk and returns the text it added.
    fn apply_chunk(&mut self, chunk: &Value) -> Option<String> {
        if chunk["usage"].is_object() {
            self.usage = parse_usage(&chunk["usage"]);
        }
        let choice = chunk["choices"].get(0)?;
        if let Some(reason) = choice["finish_reason"].as_str() {
            self.finish_reason = Some(reason.to_string());
        }
        let delta = choice["delta"]["content"].as_str().filter(|d| !d.is_empty())?;
        self.text.push_str(delta);
        Some(delta.to_string())
    }

    fn finish(self) -> ChatResponse {
        ChatResponse { text: strip_thinking_blocks(&self.text), usage: self.usage, finish: finish_reason(self.finish_reason.as_deref()) }
    }
}

fn finish_reason(reason: Option<&str>) -> FinishReason {
    match reason {
        Some("stop") | None => FinishReason::Stop,
        Some("length") => FinishReason::Length,
        Some("content_filter") => FinishReason::Refusal,
        Some(other) => FinishReason::Other(other.to_string()),
    }
}

fn parse_usage(usage: &Value) -> TokenUsage {
    TokenUsage {
        input_tokens: usage["prompt_tokens"].as_u64().unwrap_or(0),
        output_tokens: usage["completion_tokens"].as_u64().unwrap_or(0),
        cached_input_tokens: usage["prompt_tokens_details"]["cached_tokens"].as_u64().unwrap_or(0),
        reasoning_tokens: usage["completion_tokens_details"]["reasoning_tokens"].as_u64().unwrap_or(0),
    }
}

fn truncate(text: &str) -> String {
    text.chars().take(300).collect()
}

impl ChatModel for OpenAiChatModel {
    fn complete<'a>(&'a self, request: ChatRequest) -> LlmFuture<'a, ChatResponse> {
        Box::pin(async move {
            let body = self.build_body(&request);
            let mut http_request = self.client.post(&self.endpoint).json(&body);
            if !self.api_key.is_empty() {
                http_request = http_request.bearer_auth(&self.api_key);
            }
            for (name, value) in &self.extra_headers {
                http_request = http_request.header(name, value);
            }
            let response = http_request.send().await.map_err(LlmError::from_reqwest)?;
            if !response.status().is_success() {
                return Err(error_from_response(response).await);
            }
            if is_event_stream(&response) {
                return self.read_stream(response, &request).await;
            }
            // Some compatible servers ignore `stream` and answer in one piece.
            let text = response.text().await.map_err(LlmError::from_reqwest)?;
            let response = parse_complete_response(&text)?;
            request.stream_text(&response.text);
            Ok(response)
        })
    }

    fn describe(&self) -> String {
        format!("{} via {}", self.model, self.endpoint)
    }
}

fn parse_complete_response(text: &str) -> Result<ChatResponse, LlmError> {
    let value: Value = serde_json::from_str(text).map_err(|e| LlmError::InvalidResponse(format!("{e}: {}", truncate(text))))?;
    let choice = value["choices"].get(0).ok_or_else(|| LlmError::InvalidResponse("no choices in response".into()))?;
    let content = match &choice["message"]["content"] {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join(""),
        _ => String::new(),
    };
    Ok(ChatResponse {
        text: strip_thinking_blocks(&content),
        usage: parse_usage(&value["usage"]),
        finish: finish_reason(choice["finish_reason"].as_str()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_chat_completion() {
        let body = r##"{"choices":[{"message":{"content":"#1: Hallo"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":3}}"##;
        let response = parse_complete_response(body).unwrap();
        assert_eq!(response.text, "#1: Hallo");
        assert_eq!(response.usage.input_tokens, 10);
        assert_eq!(response.finish, FinishReason::Stop);
    }

    #[test]
    fn accumulates_streamed_chunks() {
        let mut answer = StreamedAnswer::default();
        let chunks = [
            r##"{"choices":[{"delta":{"role":"assistant","content":""}}]}"##,
            r##"{"choices":[{"delta":{"content":"#1: Hal"}}]}"##,
            r##"{"choices":[{"delta":{"content":"lo\n"},"finish_reason":"length"}]}"##,
            r##"{"choices":[],"usage":{"prompt_tokens":20,"completion_tokens":5,"prompt_tokens_details":{"cached_tokens":8}}}"##,
        ];
        let deltas: Vec<String> = chunks.iter().filter_map(|c| answer.apply_chunk(&serde_json::from_str(c).unwrap())).collect();
        assert_eq!(deltas, vec!["#1: Hal", "lo\n"]);
        let response = answer.finish();
        assert_eq!(response.text, "#1: Hallo");
        assert_eq!(response.finish, FinishReason::Length);
        assert_eq!(response.usage.cached_input_tokens, 8);
    }
}
