//! Sign in with ChatGPT: translations run on the user's ChatGPT plan through the
//! Responses API, authorised by an OAuth sign-in instead of an API key.

mod accounts;
pub mod oauth;
mod protect;

use std::time::Duration;

use serde_json::{json, Value};

use super::http::{http_error, streaming_client};
use super::sse::{is_event_stream, SseReader};
use super::types::*;

pub use accounts::{AccountSummary, AccountView, ChatGptAccounts, ChatGptModel, ModelCatalog, SignInProgress};

/// Plan sign-ins are only valid here, so a ChatGPT provider never talks to another URL.
pub const BASE_URL: &str = "https://api.openai.com/v1";
/// Where users see and limit what apps use of their plan.
pub const MANAGE_USAGE_URL: &str = "https://chatgpt.com/settings/usage";
/// Model chosen when a ChatGPT provider becomes the translator.
pub const DEFAULT_MODEL: &str = "gpt-6-luna";

pub struct ChatGptChatModel {
    client: reqwest::Client,
    endpoint: String,
    accounts: ChatGptAccounts,
    provider_name: String,
    model: String,
}

impl ChatGptChatModel {
    pub fn new(accounts: ChatGptAccounts, provider_name: String, model: String, idle_timeout: Duration) -> Result<Self, LlmError> {
        Ok(Self {
            client: streaming_client(idle_timeout)?,
            endpoint: format!("{BASE_URL}/responses"),
            accounts,
            provider_name,
            model,
        })
    }

    /// The plan route takes no output limit or temperature, and only stateless,
    /// streamed requests with the whole conversation in `input`.
    fn build_body(&self, request: &ChatRequest) -> Value {
        let input: Vec<Value> =
            request.messages.iter().map(|message| json!({ "role": message.role.api_name(), "content": message.text })).collect();
        let mut body = json!({ "model": self.model, "input": input, "store": false, "stream": true });
        if let Some(system) = &request.system {
            body["instructions"] = json!(system);
        }
        if let Some(effort) = request.effort.as_deref().filter(|e| !e.is_empty()) {
            body["reasoning"] = json!({ "effort": effort });
        }
        body
    }

    async fn send(&self, token: &str, body: &Value) -> Result<reqwest::Response, LlmError> {
        self.client
            .post(&self.endpoint)
            .bearer_auth(token)
            .header("accept", "text/event-stream")
            .json(body)
            .send()
            .await
            .map_err(LlmError::from_reqwest)
    }
}

impl ChatModel for ChatGptChatModel {
    fn complete<'a>(&'a self, request: ChatRequest) -> LlmFuture<'a, ChatResponse> {
        Box::pin(async move {
            let body = self.build_body(&request);
            let token = self.accounts.access_token(&self.provider_name).await?;
            let mut response = self.send(&token, &body).await?;
            if response.status().as_u16() == 401 {
                let token = self.accounts.refresh_rejected(&self.provider_name, &token).await?;
                response = self.send(&token, &body).await?;
            }
            if !response.status().is_success() {
                let status = response.status().as_u16();
                let text = response.text().await.unwrap_or_default();
                return Err(response_error(status, &text));
            }
            if !is_event_stream(&response) {
                return Err(LlmError::InvalidResponse("ChatGPT did not answer with an event stream".into()));
            }
            let mut reader = SseReader::new(response);
            let mut answer = StreamedResponse::default();
            while let Some(event) = reader.next_event().await? {
                if event.data.trim() == "[DONE]" {
                    break;
                }
                let value: Value = serde_json::from_str(&event.data)
                    .map_err(|e| LlmError::InvalidResponse(format!("{e}: {}", event.data.chars().take(300).collect::<String>())))?;
                if let Some(delta) = answer.apply_event(&value)? {
                    request.stream_text(&delta);
                }
            }
            answer.finish()
        })
    }

    fn describe(&self) -> String {
        let account = self.accounts.account(&self.provider_name).map(|a| a.display_name()).unwrap_or_else(|| "not signed in".into());
        format!("{} on the ChatGPT plan of {account}", self.model)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum StreamEnd {
    Completed,
    Incomplete(String),
}

/// Accumulates the streamed events of one response.
#[derive(Default)]
struct StreamedResponse {
    text: String,
    usage: TokenUsage,
    end: Option<StreamEnd>,
}

impl StreamedResponse {
    /// Applies one event and returns the answer text it added.
    fn apply_event(&mut self, event: &Value) -> Result<Option<String>, LlmError> {
        match event["type"].as_str().unwrap_or_default() {
            "response.output_text.delta" => {
                let delta = event["delta"].as_str().unwrap_or_default();
                self.text.push_str(delta);
                return Ok((!delta.is_empty()).then(|| delta.to_string()));
            }
            "response.completed" => {
                self.usage = parse_usage(&event["response"]["usage"]);
                self.end = Some(StreamEnd::Completed);
            }
            "response.incomplete" => {
                self.usage = parse_usage(&event["response"]["usage"]);
                let reason = event["response"]["incomplete_details"]["reason"].as_str().unwrap_or("unknown");
                self.end = Some(StreamEnd::Incomplete(reason.to_string()));
            }
            "response.failed" => return Err(error_from_object(&event["response"]["error"], 500)),
            "error" => return Err(error_from_object(event, 500)),
            _ => {}
        }
        Ok(None)
    }

    fn finish(self) -> Result<ChatResponse, LlmError> {
        let finish = match self.end {
            Some(StreamEnd::Completed) => FinishReason::Stop,
            Some(StreamEnd::Incomplete(reason)) if reason == "max_output_tokens" => FinishReason::Length,
            Some(StreamEnd::Incomplete(reason)) if reason == "content_filter" => FinishReason::Refusal,
            Some(StreamEnd::Incomplete(reason)) => FinishReason::Other(reason),
            None => return Err(LlmError::Network("the answer stream ended early".into())),
        };
        Ok(ChatResponse { text: self.text, usage: self.usage, finish })
    }
}

fn parse_usage(usage: &Value) -> TokenUsage {
    TokenUsage {
        input_tokens: usage["input_tokens"].as_u64().unwrap_or(0),
        output_tokens: usage["output_tokens"].as_u64().unwrap_or(0),
        cached_input_tokens: usage["input_tokens_details"]["cached_tokens"].as_u64().unwrap_or(0),
        reasoning_tokens: usage["output_tokens_details"]["reasoning_tokens"].as_u64().unwrap_or(0),
    }
}

/// Turns an error body of the plan route into an error the user can act on.
pub(crate) fn response_error(status: u16, body: &str) -> LlmError {
    match serde_json::from_str::<Value>(body) {
        Ok(value) => {
            let error = if value["error"].is_object() { &value["error"] } else { &value };
            known_error(error).unwrap_or_else(|| http_error(status, body, None))
        }
        Err(_) => http_error(status, body, None),
    }
}

fn error_from_object(error: &Value, status: u16) -> LlmError {
    known_error(error).unwrap_or_else(|| http_error(status, &json!({ "error": error }).to_string(), None))
}

fn known_error(error: &Value) -> Option<LlmError> {
    let code = error["code"].as_str().or_else(|| error["error"].as_str())?.replace("subscription_sharing_v2_", "subscription_sharing_");
    let message = error["message"].as_str().unwrap_or(&code).to_string();
    Some(match code.as_str() {
        "subscription_sharing_usage_limit_exceeded" => {
            LlmError::UsageLimit(format!("ChatGPT usage limit reached. See and manage your plan's usage at {MANAGE_USAGE_URL}"))
        }
        "subscription_sharing_usage_unavailable" | "subscription_sharing_user_unavailable" => LlmError::Http { status: 503, body: message },
        "subscription_sharing_user_not_eligible" => {
            LlmError::SignInRequired("this ChatGPT account cannot be used by apps; it needs a ChatGPT Plus or Pro plan".into())
        }
        "subscription_sharing_invalid_user" => LlmError::SignInRequired("ChatGPT did not accept this account; sign in again under Models".into()),
        "subscription_sharing_unsupported_capability" => {
            let param = error["param"].as_str().map(|p| format!("'{p}'")).unwrap_or_else(|| "an option".into());
            LlmError::Unsupported(format!("ChatGPT does not support {param} for apps ({message}); try another effort setting or model"))
        }
        "subscription_sharing_route_not_supported"
        | "subscription_sharing_client_not_enabled"
        | "chatpass_v2_scope_not_authorized"
        | "chatpass_v2_invalid_authorization_context" => LlmError::SignInRequired(format!(
            "ChatGPT refused the request ({code}: {message}); choose \"Enable usage sharing\" or sign in again under Models"
        )),
        "model_not_found" => LlmError::Http { status: 404, body: format!("model not found; refresh the model list ({message})") },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(events: &[&str]) -> Result<ChatResponse, LlmError> {
        let mut answer = StreamedResponse::default();
        for event in events {
            answer.apply_event(&serde_json::from_str(event).unwrap())?;
        }
        answer.finish()
    }

    #[test]
    fn assembles_streamed_response() {
        let response = feed(&[
            r#"{"type":"response.created","response":{"status":"in_progress"}}"#,
            r##"{"type":"response.output_text.delta","delta":"#1: Hal"}"##,
            r#"{"type":"response.output_text.delta","delta":"lo"}"#,
            r#"{"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":50,"input_tokens_details":{"cached_tokens":10},"output_tokens":20,"output_tokens_details":{"reasoning_tokens":12}}}}"#,
        ])
        .unwrap();
        assert_eq!(response.text, "#1: Hallo");
        assert_eq!(response.usage, TokenUsage { input_tokens: 50, output_tokens: 20, cached_input_tokens: 10, reasoning_tokens: 12 });
        assert_eq!(response.finish, FinishReason::Stop);
    }

    #[test]
    fn truncated_response_keeps_its_text() {
        let response = feed(&[
            r##"{"type":"response.output_text.delta","delta":"#1: Hallo\n"}"##,
            r#"{"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"},"usage":{"input_tokens":5,"output_tokens":5}}}"#,
        ])
        .unwrap();
        assert_eq!(response.finish, FinishReason::Length);
        assert!(matches!(feed(&[r#"{"type":"response.output_text.delta","delta":"x"}"#]), Err(LlmError::Network(_))));
    }

    #[test]
    fn maps_plan_errors() {
        let failed = feed(&[r#"{"type":"response.failed","response":{"error":{"code":"subscription_sharing_usage_limit_exceeded","message":"limit"}}}"#]);
        assert!(matches!(failed, Err(LlmError::UsageLimit(_))));
        let limit = response_error(429, r#"{"error":{"code":"subscription_sharing_v2_usage_limit_exceeded","message":"limit"}}"#);
        assert!(matches!(limit, LlmError::UsageLimit(_)) && !limit.is_retryable());
        assert!(response_error(503, r#"{"error":{"code":"subscription_sharing_usage_unavailable","message":"busy"}}"#).is_retryable());
        assert!(matches!(response_error(403, r#"{"detail":"blocked in region"}"#), LlmError::Unauthorized { status: 403, .. }));
    }

    #[test]
    fn request_body_follows_the_plan_route_rules() {
        let accounts = ChatGptAccounts::open(&tempfile::tempdir().unwrap().path().join("auth.dat"));
        let model = ChatGptChatModel::new(accounts, "chatgpt".into(), "gpt-6.1-sol".into(), Duration::from_secs(5)).unwrap();
        let request = ChatRequest {
            system: Some("translate".into()),
            messages: vec![ChatMessage::user("#1: Hello")],
            max_output_tokens: Some(100),
            temperature: Some(0.2),
            effort: Some("high".into()),
            listener: None,
        };
        let body = model.build_body(&request);
        assert_eq!(body["store"], false);
        assert_eq!(body["stream"], true);
        assert_eq!(body["instructions"], "translate");
        assert_eq!(body["input"][0]["role"], "user");
        assert_eq!(body["reasoning"]["effort"], "high");
        assert!(body.get("max_output_tokens").is_none() && body.get("temperature").is_none());
    }
}
