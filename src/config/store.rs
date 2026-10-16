use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use parking_lot::RwLock;
use tokio::sync::watch;

use super::{Config, ProviderKind};

/// Shared, thread-safe access to the configuration plus persistence to `config.toml`.
#[derive(Clone)]
pub struct ConfigStore {
    path: PathBuf,
    current: Arc<RwLock<Arc<Config>>>,
    sender: watch::Sender<Arc<Config>>,
}

impl ConfigStore {
    /// Loads the config file, creating it with defaults when missing.
    pub fn open(path: &Path) -> Result<Self> {
        let config = if path.exists() {
            load_config_file(path)?
        } else {
            let config = Config::default();
            write_config_file(path, &config)?;
            config
        };
        let config = Arc::new(config);
        let (sender, _) = watch::channel(config.clone());
        Ok(Self { path: path.to_path_buf(), current: Arc::new(RwLock::new(config)), sender })
    }

    pub fn current(&self) -> Arc<Config> {
        self.current.read().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<Arc<Config>> {
        self.sender.subscribe()
    }

    /// Persists a new configuration and notifies subscribers.
    pub fn save(&self, config: Config) -> Result<()> {
        write_config_file(&self.path, &config)?;
        self.replace(config);
        Ok(())
    }

    /// Re-reads the file after it changed on disk. Returns whether anything changed; when
    /// the file cannot be read or parsed, the error is returned and the previous
    /// configuration stays active.
    pub fn reload_from_disk(&self) -> Result<bool> {
        let config = load_config_file(&self.path)?;
        if *self.current() == config {
            return Ok(false);
        }
        self.replace(config);
        Ok(true)
    }

    fn replace(&self, config: Config) {
        let config = Arc::new(config);
        *self.current.write() = config.clone();
        let _ = self.sender.send(config);
    }
}

pub fn load_config_file(path: &Path) -> Result<Config> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut config: Config = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    for provider in config.providers.iter_mut().filter(|p| p.kind == ProviderKind::ChatGpt) {
        provider.base_url = ProviderKind::ChatGpt.default_base_url().to_string();
    }
    Ok(config)
}

pub fn write_config_file(path: &Path, config: &Config) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = toml::to_string_pretty(config).context("serialising configuration")?;
    let text = format!("{}\n{body}", FILE_HEADER);
    write_atomically(path, text.as_bytes()).with_context(|| format!("writing {}", path.display()))
}

const FILE_HEADER: &str = "# LLM Subs configuration. Edited by the app and by hand; changes are picked up live.\n\
# Every key is optional and falls back to its default. See config.example.toml for docs.\n";

/// Writes to a sibling temp file first so a crash never leaves a truncated file behind.
pub fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temp_path = path.with_extension("tmp");
    std::fs::write(&temp_path, bytes)?;
    std::fs::rename(&temp_path, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_round_trips_through_toml() {
        let config = Config::default();
        let text = toml::to_string_pretty(&config).unwrap();
        let parsed: Config = toml::from_str(&text).unwrap();
        assert_eq!(parsed, config);
    }

    #[test]
    fn provider_kinds_use_the_documented_names() {
        let parsed: Config = toml::from_str(
            "[[providers]]\nname = \"a\"\nkind = \"openai\"\n[[providers]]\nname = \"b\"\nkind = \"openai_compatible\"\n[[providers]]\nname = \"c\"\nkind = \"anthropic\"\n",
        )
        .unwrap();
        assert_eq!(parsed.providers[0].kind, ProviderKind::OpenAi);
        assert_eq!(parsed.providers[1].kind, ProviderKind::OpenAiCompatible);
        assert_eq!(parsed.providers[2].kind, ProviderKind::Anthropic);
    }

    #[test]
    fn chatgpt_providers_always_use_the_chatgpt_endpoint() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, "[[providers]]\nname = \"local\"\nkind = \"chatgpt\"\nbase_url = \"http://192.168.1.5:1234/v1\"\n").unwrap();
        let loaded = load_config_file(&path).unwrap();
        assert_eq!(loaded.providers[0].base_url, crate::llm::chatgpt::BASE_URL);
    }

    #[test]
    fn partial_file_uses_defaults_for_missing_keys() {
        let parsed: Config = toml::from_str("[general]\ntarget_language = \"fi\"\n").unwrap();
        assert_eq!(parsed.general.target_language, "fi");
        assert_eq!(parsed.translation.lines_per_request, Config::default().translation.lines_per_request);
    }
}
