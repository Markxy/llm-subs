use std::ffi::OsStr;
use std::path::Path;
use std::process::Stdio;

use thiserror::Error;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Error)]
pub enum CommandError {
    #[error("could not start {program}: {source}")]
    Spawn { program: String, source: std::io::Error },
    #[error("{program} failed (exit code {code:?}): {stderr}")]
    Failed { program: String, code: Option<i32>, stderr: String },
    #[error("{program} was cancelled")]
    Cancelled { program: String },
    #[error("io error while talking to {program}: {source}")]
    Io { program: String, source: std::io::Error },
}

pub struct CommandOutput {
    pub stdout: Vec<u8>,
}

/// Runs a command to completion and returns its output. Kills the process when
/// `cancel` fires.
pub async fn run(program: &Path, args: &[impl AsRef<OsStr>], cancel: &CancellationToken) -> Result<CommandOutput, CommandError> {
    let program_name = program.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    hide_console_window(&mut command);

    let child = command.spawn().map_err(|source| CommandError::Spawn { program: program_name.clone(), source })?;
    let output = tokio::select! {
        result = child.wait_with_output() => result,
        _ = cancel.cancelled() => return Err(CommandError::Cancelled { program: program_name }),
    }
    .map_err(|source| CommandError::Io { program: program_name.clone(), source })?;

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !output.status.success() {
        return Err(CommandError::Failed { program: program_name, code: output.status.code(), stderr: last_lines(&stderr, 8) });
    }
    Ok(CommandOutput { stdout: output.stdout })
}

fn last_lines(text: &str, count: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(count);
    lines[start..].join("\n")
}

#[cfg(windows)]
fn hide_console_window(command: &mut Command) {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console_window(_command: &mut Command) {}
