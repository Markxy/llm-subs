#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Result;
use clap::Parser;

use llm_subs::app::{AppRuntime, StartOptions};
use llm_subs::cli::{Cli, Command};
use llm_subs::paths::AppPaths;

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("llm-subs: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    let paths = match &cli.home {
        Some(home) => AppPaths::from_single_root(home),
        None => AppPaths::discover()?,
    };
    paths.ensure_directories()?;

    let headless = cli.command.is_some();
    if headless {
        llm_subs::platform::attach_parent_console();
    }
    let log_level = llm_subs::config::store::load_config_file(&paths.config_file()).map(|c| c.general.log_level).unwrap_or_else(|_| "info".to_string());
    let stderr_level = if headless {
        Some(tracing_subscriber::filter::LevelFilter::WARN)
    } else if cfg!(debug_assertions) {
        Some(tracing_subscriber::filter::LevelFilter::INFO)
    } else {
        None
    };
    let _log_guard = llm_subs::logging::init(&paths.log_dir(), &log_level, stderr_level)?;
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "starting");

    match cli.command {
        None => {
            let runtime = AppRuntime::start(paths, StartOptions { enable_watching: true })?;
            let start_hidden = cli.hidden || runtime.config().general.start_hidden;
            llm_subs::ui::run(runtime, start_hidden)?;
            Ok(ExitCode::SUCCESS)
        }
        Some(Command::Translate { paths: inputs, force }) => {
            let runtime = AppRuntime::start(paths, StartOptions { enable_watching: false })?;
            let inputs = inputs.into_iter().map(|p| std::path::absolute(&p).unwrap_or(p)).collect();
            block_on(&runtime, llm_subs::cli::run_translate(runtime.clone(), inputs, force))
        }
        Some(Command::Watch) => {
            let runtime = AppRuntime::start(paths, StartOptions { enable_watching: true })?;
            block_on(&runtime, llm_subs::cli::run_watch(runtime.clone()))
        }
        Some(Command::Doctor) => {
            let runtime = AppRuntime::start(paths, StartOptions { enable_watching: false })?;
            std::thread::sleep(std::time::Duration::from_millis(400));
            Ok(llm_subs::cli::run_doctor(&runtime))
        }
        Some(Command::ConfigPath) => {
            println!("{}", paths.config_file().display());
            Ok(ExitCode::SUCCESS)
        }
        Some(Command::Chatgpt(command)) => {
            let runtime = AppRuntime::start(paths, StartOptions { enable_watching: false })?;
            block_on(&runtime, llm_subs::cli::run_chatgpt(runtime.clone(), command))
        }
        Some(Command::Usage { days }) => {
            let runtime = AppRuntime::start(paths, StartOptions { enable_watching: false })?;
            Ok(llm_subs::cli::run_usage(&runtime, days))
        }
    }
}

fn block_on(runtime: &Arc<AppRuntime>, future: impl std::future::Future<Output = Result<ExitCode>>) -> Result<ExitCode> {
    runtime.tokio_handle().block_on(future)
}
