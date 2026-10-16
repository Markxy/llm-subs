use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};

use crate::config::{Config, ModelSelection, ProviderKind, ProviderProfile};

use super::anthropic::AnthropicChatModel;
use super::chatgpt::{ChatGptAccounts, ChatGptChatModel};
use super::ledger::UsageLedger;
use super::openai::OpenAiChatModel;
use super::pricing::{cost_for, Cost};
use super::types::*;

/// Long-lived state the model clients share: ChatGPT sign-ins and the usage tally.
#[derive(Clone)]
pub struct LlmServices {
    pub chatgpt: ChatGptAccounts,
    pub usage: UsageLedger,
}

/// A configured model ready to answer requests, with the selection's per-call settings.
#[derive(Clone)]
pub struct ModelHandle {
    pub client: Arc<dyn ChatModel>,
    pub selection: ModelSelection,
    pub provider_kind: ProviderKind,
    pub max_retries: u32,
}

impl ModelHandle {
    pub fn request(&self, system: Option<String>, messages: Vec<ChatMessage>) -> ChatRequest {
        ChatRequest {
            system,
            messages,
            max_output_tokens: self.selection.max_output_tokens,
            temperature: self.selection.temperature,
            effort: self.selection.effort.clone(),
            listener: None,
        }
    }

    pub fn describe(&self) -> String {
        self.client.describe()
    }

    pub fn cost_of(&self, usage: &TokenUsage) -> Option<Cost> {
        cost_for(self.provider_kind, &self.selection, usage)
    }
}

pub fn build_model(config: &Config, selection: &ModelSelection, services: &LlmServices) -> Result<ModelHandle> {
    if selection.model.trim().is_empty() {
        bail!("no model name configured");
    }
    let provider = config
        .provider(&selection.provider)
        .with_context(|| format!("provider '{}' is not defined", selection.provider))?;
    let idle_timeout = Duration::from_secs(config.translation.request_timeout_seconds.max(30));
    let client = build_client(provider, &selection.model, idle_timeout, services)?;
    let metered = Arc::new(MeteredModel {
        inner: client,
        ledger: services.usage.clone(),
        provider: provider.name.clone(),
        provider_kind: provider.kind,
        selection: selection.clone(),
    });
    Ok(ModelHandle { client: metered, selection: selection.clone(), provider_kind: provider.kind, max_retries: config.translation.max_retries })
}

pub fn build_client(provider: &ProviderProfile, model: &str, idle_timeout: Duration, services: &LlmServices) -> Result<Arc<dyn ChatModel>> {
    let api_key = provider.resolved_api_key();
    let headers = provider.extra_headers.clone();
    let model = model.to_string();
    let client: Arc<dyn ChatModel> = match provider.kind {
        ProviderKind::OpenAi => Arc::new(OpenAiChatModel::new(&provider.base_url, api_key, model, headers, true, idle_timeout)?),
        ProviderKind::OpenAiCompatible => Arc::new(OpenAiChatModel::new(&provider.base_url, api_key, model, headers, false, idle_timeout)?),
        ProviderKind::Anthropic => Arc::new(AnthropicChatModel::new(&provider.base_url, api_key, model, headers, idle_timeout)?),
        ProviderKind::ChatGpt => Arc::new(ChatGptChatModel::new(services.chatgpt.clone(), provider.name.clone(), model, idle_timeout)?),
    };
    Ok(client)
}

/// Records the usage of every answered request in the ledger.
struct MeteredModel {
    inner: Arc<dyn ChatModel>,
    ledger: UsageLedger,
    provider: String,
    provider_kind: ProviderKind,
    selection: ModelSelection,
}

impl ChatModel for MeteredModel {
    fn complete<'a>(&'a self, request: ChatRequest) -> LlmFuture<'a, ChatResponse> {
        Box::pin(async move {
            let response = self.inner.complete(request).await?;
            let cost = cost_for(self.provider_kind, &self.selection, &response.usage);
            self.ledger.record(&self.provider, self.provider_kind, &self.selection.model, &response.usage, cost);
            Ok(response)
        })
    }

    fn describe(&self) -> String {
        self.inner.describe()
    }
}

/// Sends a one-word request to verify credentials, endpoint and model name.
pub async fn test_connection(provider: &ProviderProfile, model: &str, effort: Option<String>, services: &LlmServices) -> Result<String> {
    let client = build_client(provider, model, Duration::from_secs(60), services)?;
    let request = ChatRequest {
        messages: vec![ChatMessage::user("Reply with the single word OK.")],
        max_output_tokens: (provider.kind != ProviderKind::ChatGpt).then_some(64),
        effort,
        ..ChatRequest::default()
    };
    let response = client.complete(request).await?;
    Ok(format!("Connected. {} answered: {}", model, response.text.trim().chars().take(60).collect::<String>()))
}
