use std::path::{Path, PathBuf};

use tokio_util::sync::CancellationToken;

use crate::config::FfmpegConfig;

use super::command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolOrigin {
    /// Configured `ffmpeg.path_override`.
    Override,
    /// Downloaded by LLM Subs into its own data directory.
    Managed,
    /// Found on `PATH`.
    SystemPath,
}

impl ToolOrigin {
    pub fn label(self) -> &'static str {
        match self {
            ToolOrigin::Override => "configured path",
            ToolOrigin::Managed => "managed download",
            ToolOrigin::SystemPath => "system PATH",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfmpegTools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub origin: ToolOrigin,
}

impl FfmpegTools {
    /// First line of `ffmpeg -version`, e.g. `ffmpeg version 7.1 ...`.
    pub async fn version(&self) -> Option<String> {
        let output = command::run(&self.ffmpeg, &["-version"], &CancellationToken::new()).await.ok()?;
        String::from_utf8_lossy(&output.stdout).lines().next().map(|line| line.trim().to_string())
    }
}

/// Resolution order: configured override, managed download, then `PATH`.
pub fn locate(config: &FfmpegConfig, managed_dir: &Path) -> Option<FfmpegTools> {
    if let Some(override_path) = config.path_override.as_deref().filter(|p| !p.as_os_str().is_empty()) {
        let directory = if override_path.is_file() { override_path.parent().unwrap_or(override_path) } else { override_path };
        if let Some(tools) = tools_in_directory(directory, ToolOrigin::Override) {
            return Some(tools);
        }
    }
    if let Some(tools) = tools_in_directory(managed_dir, ToolOrigin::Managed) {
        return Some(tools);
    }
    let ffmpeg = find_in_path(&executable_name("ffmpeg"))?;
    let ffprobe = find_in_path(&executable_name("ffprobe"))?;
    Some(FfmpegTools { ffmpeg, ffprobe, origin: ToolOrigin::SystemPath })
}

pub fn executable_name(base: &str) -> String {
    if cfg!(windows) { format!("{base}.exe") } else { base.to_string() }
}

fn tools_in_directory(directory: &Path, origin: ToolOrigin) -> Option<FfmpegTools> {
    let ffmpeg = directory.join(executable_name("ffmpeg"));
    let ffprobe = directory.join(executable_name("ffprobe"));
    (ffmpeg.is_file() && ffprobe.is_file()).then_some(FfmpegTools { ffmpeg, ffprobe, origin })
}

fn find_in_path(executable: &str) -> Option<PathBuf> {
    let path_variable = std::env::var_os("PATH")?;
    std::env::split_paths(&path_variable).map(|dir| dir.join(executable)).find(|candidate| candidate.is_file())
}
