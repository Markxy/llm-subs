//! Where LLM Subs keeps its files on each platform.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::ProjectDirs;

/// Environment variable that switches LLM Subs into portable mode: every file
/// (config, history, managed FFmpeg, logs) lives under the given directory.
pub const PORTABLE_HOME_ENV: &str = "LLM_SUBS_HOME";

#[derive(Debug, Clone)]
pub struct AppPaths {
    config_dir: PathBuf,
    data_dir: PathBuf,
}

impl AppPaths {
    pub fn discover() -> Result<Self> {
        if let Some(home) = std::env::var_os(PORTABLE_HOME_ENV) {
            let home = PathBuf::from(home);
            return Ok(Self { config_dir: home.clone(), data_dir: home });
        }
        let dirs = ProjectDirs::from("", "", "LLM Subs")
            .context("could not determine the platform application directories")?;
        Ok(Self { config_dir: dirs.config_dir().to_path_buf(), data_dir: dirs.data_dir().to_path_buf() })
    }

    pub fn from_single_root(root: &Path) -> Self {
        Self { config_dir: root.to_path_buf(), data_dir: root.to_path_buf() }
    }

    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    pub fn history_file(&self) -> PathBuf {
        self.data_dir.join("jobs.json")
    }

    /// ChatGPT sign-ins; encrypted for the current user where the platform allows.
    pub fn chatgpt_auth_file(&self) -> PathBuf {
        self.data_dir.join("chatgpt-auth.dat")
    }

    /// Model usage and cost per day, provider and model.
    pub fn usage_file(&self) -> PathBuf {
        self.data_dir.join("usage.json")
    }

    pub fn managed_ffmpeg_dir(&self) -> PathBuf {
        self.data_dir.join("bin")
    }

    pub fn log_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }

    pub fn ensure_directories(&self) -> std::io::Result<()> {
        for dir in [
            self.config_dir.as_path(),
            self.data_dir.as_path(),
            &self.managed_ffmpeg_dir(),
            &self.log_dir(),
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}
