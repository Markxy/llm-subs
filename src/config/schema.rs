use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub general: GeneralConfig,
    pub watch: WatchConfig,
    pub output: OutputConfig,
    pub ffmpeg: FfmpegConfig,
    pub providers: Vec<ProviderProfile>,
    pub models: ModelAssignments,
    pub translation: TranslationConfig,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    /// ISO 639-1 code of the language subtitles are translated into.
    pub target_language: String,
    /// Start with the window hidden (only the tray icon is visible).
    pub start_hidden: bool,
    /// Closing the window hides it to the tray instead of quitting.
    pub close_to_tray: bool,
    pub max_concurrent_jobs: usize,
    /// `error`, `warn`, `info`, `debug` or `trace`.
    pub log_level: String,
    /// How many finished jobs to keep in the history file.
    pub history_limit: usize,
    /// The first-run guide was finished or skipped. While false, the window opens it
    /// unless the configuration can translate already.
    pub onboarding_complete: bool,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            target_language: super::system_language_code(),
            start_hidden: false,
            close_to_tray: true,
            max_concurrent_jobs: 1,
            log_level: "info".to_string(),
            history_limit: 500,
            onboarding_complete: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WatchConfig {
    pub enabled: bool,
    pub folders: Vec<WatchedFolder>,
    /// A new file is only picked up once its size has stayed the same for this long,
    /// so half-copied downloads are not processed prematurely.
    pub settle_seconds: u64,
    pub media_extensions: Vec<String>,
    pub subtitle_extensions: Vec<String>,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            folders: Vec::new(),
            settle_seconds: 20,
            media_extensions: [
                "mkv", "mp4", "m4v", "mov", "avi", "webm", "ts", "m2ts", "mts", "mpg", "mpeg",
                "wmv", "flv", "ogv", "vob",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            subtitle_extensions: ["srt", "vtt", "ass", "ssa"].iter().map(|s| s.to_string()).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WatchedFolder {
    pub path: PathBuf,
    pub recursive: bool,
    /// Also queue files that are already in the folder when watching starts.
    pub process_existing: bool,
}

impl Default for WatchedFolder {
    fn default() -> Self {
        Self { path: PathBuf::new(), recursive: true, process_existing: false }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OutputMode {
    /// Write `Movie.<lang>.srt` next to the media file. Every player picks it up
    /// automatically and the media file itself is never touched.
    #[default]
    Sidecar,
    /// Remux the media file with the translated track added. Only supported for
    /// Matroska and MP4 containers; the file is rewritten (stream copy, no re-encode).
    Embed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputConfig {
    pub mode: OutputMode,
    /// Keep the sidecar `.srt` as well when embedding.
    pub keep_sidecar_when_embedding: bool,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self { mode: OutputMode::Sidecar, keep_sidecar_when_embedding: false }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FfmpegConfig {
    /// Directory containing `ffmpeg` and `ffprobe`, or the path to the `ffmpeg`
    /// executable. When empty, the managed download and then `PATH` are used.
    pub path_override: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    #[default]
    #[serde(rename = "openai")]
    OpenAi,
    Anthropic,
    /// Any server speaking the OpenAI chat completions protocol
    /// (Ollama, llama.cpp, LM Studio, vLLM, OpenRouter, Gemini's compatibility endpoint, ...).
    #[serde(rename = "openai_compatible")]
    OpenAiCompatible,
    /// A ChatGPT account connected with "Sign in with ChatGPT"; usage counts against
    /// the account's plan instead of an API key.
    #[serde(rename = "chatgpt")]
    ChatGpt,
}

impl ProviderKind {
    pub fn label(self) -> &'static str {
        match self {
            ProviderKind::OpenAi => "OpenAI",
            ProviderKind::Anthropic => "Anthropic",
            ProviderKind::OpenAiCompatible => "OpenAI-compatible",
            ProviderKind::ChatGpt => "ChatGPT account",
        }
    }

    pub fn default_base_url(self) -> &'static str {
        match self {
            ProviderKind::OpenAi => "https://api.openai.com/v1",
            ProviderKind::Anthropic => "https://api.anthropic.com",
            ProviderKind::OpenAiCompatible => "http://localhost:11434/v1",
            ProviderKind::ChatGpt => crate::llm::chatgpt::BASE_URL,
        }
    }

    /// Effort levels the protocol accepts, mildest first. Empty when there is no such setting.
    pub fn effort_levels(self) -> &'static [&'static str] {
        match self {
            ProviderKind::Anthropic => &["low", "medium", "high", "xhigh", "max"],
            ProviderKind::OpenAi | ProviderKind::ChatGpt => &["none", "minimal", "low", "medium", "high", "xhigh"],
            ProviderKind::OpenAiCompatible => &[],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderProfile {
    /// Name used to reference this provider from `[models]`.
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: String,
    /// Stored in plain text. Prefer `api_key_env` if you share your config file.
    pub api_key: String,
    /// Name of an environment variable holding the key; takes precedence over `api_key`.
    pub api_key_env: Option<String>,
    /// Extra HTTP headers, e.g. OpenRouter's attribution headers.
    pub extra_headers: Vec<(String, String)>,
}

impl Default for ProviderProfile {
    fn default() -> Self {
        Self {
            name: "openai".to_string(),
            kind: ProviderKind::OpenAi,
            base_url: ProviderKind::OpenAi.default_base_url().to_string(),
            api_key: String::new(),
            api_key_env: None,
            extra_headers: Vec::new(),
        }
    }
}

impl ProviderProfile {
    pub fn resolved_api_key(&self) -> String {
        if let Some(var) = self.api_key_env.as_deref().filter(|v| !v.is_empty())
            && let Ok(value) = std::env::var(var)
            && !value.is_empty()
        {
            return value;
        }
        self.api_key.clone()
    }
}

/// Prices in US dollars per million tokens, used to show what a translation cost.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelPricing {
    pub input_per_million: f64,
    /// Price of input tokens served from the provider's prompt cache. Defaults to the input price.
    pub cached_input_per_million: Option<f64>,
    pub output_per_million: f64,
}

/// The model that translates, with its per-request settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelSelection {
    pub provider: String,
    pub model: String,
    /// Reasoning effort: Anthropic `output_config.effort`, OpenAI and ChatGPT `reasoning.effort`.
    pub effort: Option<String>,
    pub max_output_tokens: Option<u32>,
    /// Only sent when set; many current models reject a custom temperature.
    pub temperature: Option<f32>,
    /// Overrides the built-in price list, e.g. for a model it does not know.
    pub pricing: Option<ModelPricing>,
}

impl Default for ModelSelection {
    fn default() -> Self {
        Self {
            provider: "openai".to_string(),
            model: String::new(),
            effort: None,
            max_output_tokens: None,
            temperature: None,
            pricing: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelAssignments {
    /// Translates subtitle text.
    pub translation: ModelSelection,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LineWrapConfig {
    pub max_line_length: usize,
    pub max_lines: usize,
}

impl Default for LineWrapConfig {
    fn default() -> Self {
        Self { max_line_length: 42, max_lines: 2 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TranslationConfig {
    /// A film goes out in batches of this many lines, several at once.
    pub lines_per_request: usize,
    pub max_chars_per_request: usize,
    /// Requests in flight at once. With 1, each batch sees the previous batch's translations.
    pub parallel_requests: usize,
    /// Preceding lines included with each batch so meaning carries across batch boundaries.
    pub continuity_lines: usize,
    /// Preferences on top of the built-in instructions, e.g. how to handle swearing or honorifics.
    pub extra_instructions: String,
    /// Source subtitle languages to prefer, in order. Empty means "the audio's language".
    pub preferred_source_languages: Vec<String>,
    /// Skip media that already has subtitles in the target language. When off, it is
    /// translated anyway and an existing translation next to it is replaced.
    pub skip_if_target_exists: bool,
    pub line_wrap: LineWrapConfig,
    /// Gives up on a request when the provider sends nothing for this long.
    pub request_timeout_seconds: u64,
    pub max_retries: u32,
}

impl Default for TranslationConfig {
    fn default() -> Self {
        Self {
            lines_per_request: 100,
            max_chars_per_request: 10_000,
            parallel_requests: 8,
            continuity_lines: 12,
            extra_instructions: String::new(),
            preferred_source_languages: Vec::new(),
            skip_if_target_exists: true,
            line_wrap: LineWrapConfig::default(),
            request_timeout_seconds: 300,
            max_retries: 2,
        }
    }
}

impl Config {
    pub fn provider(&self, name: &str) -> Option<&ProviderProfile> {
        self.providers.iter().find(|p| p.name == name)
    }

    /// Problems that make a translation impossible, in the order a user should fix them.
    pub fn validation_problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.general.target_language.trim().is_empty() {
            problems.push("No target language selected.".to_string());
        }
        let selection = &self.models.translation;
        if selection.model.trim().is_empty() {
            problems.push("No translation model set.".to_string());
        }
        match self.provider(&selection.provider) {
            None => problems.push(format!("The translation model uses provider '{}', which does not exist.", selection.provider)),
            Some(provider) => match provider.kind {
                ProviderKind::OpenAi | ProviderKind::Anthropic if provider.resolved_api_key().is_empty() => {
                    problems.push(format!("Provider '{}' has no API key.", provider.name));
                }
                _ => {}
            },
        }
        problems
    }
}
