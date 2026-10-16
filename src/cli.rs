//! Command line surface. Without a subcommand the desktop app starts.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::app::{AppRuntime, FfmpegStatus};
use crate::config::{ProviderKind, ProviderProfile};
use crate::jobs::{JobOptions, JobOrigin, JobStatus};
use crate::llm::chatgpt::MANAGE_USAGE_URL;
use crate::llm::{format_token_count, format_usd};

#[derive(Debug, Parser)]
#[command(name = "llm-subs", version, about = "Watches media folders and translates subtitles with an LLM.")]
pub struct Cli {
    /// Start with the window hidden; only the tray icon is shown.
    #[arg(long)]
    pub hidden: bool,

    /// Portable mode: keep configuration, history and downloads in this directory.
    #[arg(long, value_name = "DIR", env = crate::paths::PORTABLE_HOME_ENV)]
    pub home: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Translate files or folders now, without the window, and exit when done.
    Translate {
        /// Media files, subtitle files or folders.
        paths: Vec<PathBuf>,
        /// Translate even when the target language already exists.
        #[arg(long)]
        force: bool,
    },
    /// Watch the configured folders without showing a window (for servers).
    Watch,
    /// Show where files live and whether FFmpeg and the model configuration are usable.
    Doctor,
    /// Print the path of the configuration file.
    ConfigPath,
    /// Connect a ChatGPT plan with Sign in with ChatGPT, or check the connection.
    #[command(subcommand)]
    Chatgpt(ChatGptCommand),
    /// Show tokens and cost per model.
    Usage {
        /// Only the last N days, today included.
        #[arg(long)]
        days: Option<u64>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ChatGptCommand {
    /// Open the browser to connect a ChatGPT account. Adds a `chatgpt` provider when there is none.
    SignIn {
        /// The provider entry to sign in; defaults to the first ChatGPT provider.
        #[arg(long)]
        provider: Option<String>,
        /// Ask again for permission to use the plan, after it was declined.
        #[arg(long)]
        enable_usage_sharing: bool,
        /// Forget the account connected before and connect another one.
        #[arg(long, conflicts_with = "enable_usage_sharing")]
        switch_account: bool,
    },
    /// Disconnect the account and revoke its session.
    SignOut {
        #[arg(long)]
        provider: Option<String>,
    },
    /// Show the connected accounts and the models they can use.
    Status,
}

pub async fn run_translate(runtime: Arc<AppRuntime>, paths: Vec<PathBuf>, force: bool) -> Result<ExitCode> {
    if paths.is_empty() {
        eprintln!("nothing to translate: pass at least one file or folder");
        return Ok(ExitCode::from(2));
    }
    let problems = runtime.validation_problems(&runtime.config());
    if !problems.is_empty() {
        eprintln!("configuration is incomplete:");
        for problem in problems {
            eprintln!("  - {problem}");
        }
        eprintln!("edit {} or start the app to fix this", runtime.paths().config_file().display());
        return Ok(ExitCode::from(2));
    }
    let queued = runtime.submit_paths(&paths, JobOrigin::CommandLine, JobOptions { force });
    println!("queued {} file(s)", queued.len());
    if queued.is_empty() {
        return Ok(ExitCode::SUCCESS);
    }

    let mut announced: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut failures = 0;
    loop {
        let records: Vec<_> = runtime.jobs().snapshot().into_iter().filter(|r| queued.contains(&r.id)).collect();
        for record in &records {
            let line = match &record.status {
                JobStatus::Queued => continue,
                JobStatus::Running => format!("{} ({:>3}%)", record.activity, (record.progress * 100.0) as u32),
                other => other.label().to_string(),
            };
            if announced.get(&record.id) != Some(&line) {
                println!("{:<40} {}", record.input.file_name(), line);
                announced.insert(record.id.clone(), line);
            }
        }
        if runtime.jobs().active_count() == 0 {
            for record in &records {
                match &record.status {
                    JobStatus::Failed { error } => {
                        failures += 1;
                        eprintln!("FAILED  {}: {error}", record.input.file_name());
                    }
                    JobStatus::Skipped { reason, .. } => println!("skipped {}: {reason}", record.input.file_name()),
                    JobStatus::Succeeded => {
                        if let Some(output) = &record.output_path {
                            println!("wrote   {}", output.display());
                        }
                    }
                    _ => {}
                }
            }
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Ok(if failures == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

pub async fn run_watch(runtime: Arc<AppRuntime>) -> Result<ExitCode> {
    let folders = runtime.config().watch.folders.clone();
    if folders.is_empty() {
        eprintln!("no folders configured; add [[watch.folders]] entries to {}", runtime.paths().config_file().display());
        return Ok(ExitCode::from(2));
    }
    for folder in &folders {
        println!("watching {}", folder.path.display());
    }
    println!("press Ctrl+C to stop");
    tokio::signal::ctrl_c().await?;
    Ok(ExitCode::SUCCESS)
}

pub fn run_doctor(runtime: &AppRuntime) -> ExitCode {
    let paths = runtime.paths();
    println!("LLM Subs {}", env!("CARGO_PKG_VERSION"));
    println!("config file    {}", paths.config_file().display());
    println!("job history    {}", paths.history_file().display());
    println!("logs           {}", paths.log_dir().display());
    println!("managed ffmpeg {}", paths.managed_ffmpeg_dir().display());
    match runtime.ffmpeg_status() {
        FfmpegStatus::Available { tools, version } => {
            println!("ffmpeg         {} ({})", tools.ffmpeg.display(), tools.origin.label());
            if let Some(version) = version {
                println!("               {version}");
            }
        }
        FfmpegStatus::Missing => println!("ffmpeg         NOT FOUND - install it, set ffmpeg.path_override, or download it in the app"),
    }
    let config = runtime.config();
    println!("target language {}", config.general.target_language);
    println!("providers      {}", if config.providers.is_empty() { "none".to_string() } else { config.providers.iter().map(|p| format!("{} ({})", p.name, p.kind.label())).collect::<Vec<_>>().join(", ") });
    println!("translation    {} / {}", config.models.translation.provider, config.models.translation.model);
    for provider in config.providers.iter().filter(|p| p.kind == ProviderKind::ChatGpt) {
        let state = match runtime.llm().chatgpt.account(&provider.name) {
            Some(account) if account.usage_sharing => format!("signed in as {}", account.display_name()),
            Some(account) => format!("signed in as {}, but usage sharing is off", account.display_name()),
            None => "not signed in (llm-subs chatgpt sign-in)".to_string(),
        };
        println!("chatgpt        {}: {state}", provider.name);
    }
    println!("watch folders  {}", if config.watch.folders.is_empty() { "none".to_string() } else { config.watch.folders.iter().map(|f| f.path.display().to_string()).collect::<Vec<_>>().join(", ") });
    let problems = runtime.validation_problems(&config);
    if problems.is_empty() {
        println!("status         ready");
        ExitCode::SUCCESS
    } else {
        println!("status         incomplete");
        for problem in problems {
            println!("  - {problem}");
        }
        ExitCode::FAILURE
    }
}

pub async fn run_chatgpt(runtime: Arc<AppRuntime>, command: ChatGptCommand) -> Result<ExitCode> {
    match command {
        ChatGptCommand::SignIn { provider, enable_usage_sharing, switch_account } => {
            let provider = match chatgpt_provider(&runtime, provider.as_deref()) {
                Some(provider) => provider,
                None if provider.is_none() => add_chatgpt_provider(&runtime)?,
                None => return Ok(unknown_provider(provider.as_deref())),
            };
            let accounts = runtime.llm().chatgpt.clone();
            if switch_account {
                accounts.forget(&provider.name).await?;
            }
            let announce = |url: &str| {
                println!("Opening your browser to connect a ChatGPT account. If it does not open, visit:\n\n  {url}\n");
                if let Err(error) = open::that_detached(url) {
                    tracing::debug!(%error, "could not open the browser");
                }
            };
            let account = accounts.sign_in(&provider.name, enable_usage_sharing, announce).await?;
            println!("Signed in as {}.", account.display_name());
            if !account.usage_sharing {
                println!("Usage sharing was not allowed, so LLM Subs cannot translate with this plan yet.");
                println!("Run `llm-subs chatgpt sign-in --enable-usage-sharing` to allow it.");
                return Ok(ExitCode::FAILURE);
            }
            if !account.plan_notice_acknowledged {
                println!("Eligible usage in this app uses your ChatGPT plan. Manage usage in your ChatGPT settings: {MANAGE_USAGE_URL}");
                accounts.acknowledge_plan_notice(&provider.name);
            }
            print_chatgpt_models(&runtime, &provider).await;
            let config = runtime.config();
            if config.models.translation.provider != provider.name {
                println!("\nTo translate with it, set in {}:", runtime.paths().config_file().display());
                println!("  [models.translation]\n  provider = \"{}\"\n  model = \"<one of the models above>\"", provider.name);
            }
            Ok(ExitCode::SUCCESS)
        }
        ChatGptCommand::SignOut { provider } => {
            let Some(provider) = chatgpt_provider(&runtime, provider.as_deref()) else { return Ok(unknown_provider(provider.as_deref())) };
            runtime.llm().chatgpt.sign_out(&provider.name).await?;
            println!("Signed out of ChatGPT for provider '{}'.", provider.name);
            Ok(ExitCode::SUCCESS)
        }
        ChatGptCommand::Status => {
            let config = runtime.config();
            let providers: Vec<&ProviderProfile> = config.providers.iter().filter(|p| p.kind == ProviderKind::ChatGpt).collect();
            if providers.is_empty() {
                println!("No ChatGPT provider is configured. Run `llm-subs chatgpt sign-in` to add one.");
                return Ok(ExitCode::FAILURE);
            }
            let mut ready = true;
            for provider in providers {
                match runtime.llm().chatgpt.account(&provider.name) {
                    Some(account) => {
                        println!("{}: signed in as {}", provider.name, account.display_name());
                        if account.usage_sharing {
                            print_chatgpt_models(&runtime, provider).await;
                        } else {
                            ready = false;
                            println!("  usage sharing is off; run `llm-subs chatgpt sign-in --enable-usage-sharing`");
                        }
                    }
                    None => {
                        ready = false;
                        println!("{}: not signed in", provider.name);
                    }
                }
            }
            println!("Plan usage: {MANAGE_USAGE_URL}");
            Ok(if ready { ExitCode::SUCCESS } else { ExitCode::FAILURE })
        }
    }
}

fn chatgpt_provider(runtime: &AppRuntime, name: Option<&str>) -> Option<ProviderProfile> {
    let config = runtime.config();
    config.providers.iter().filter(|p| p.kind == ProviderKind::ChatGpt).find(|p| name.is_none_or(|name| p.name == name)).cloned()
}

fn add_chatgpt_provider(runtime: &AppRuntime) -> Result<ProviderProfile> {
    let mut config = (*runtime.config()).clone();
    let mut name = "chatgpt".to_string();
    let mut counter = 2;
    while config.provider(&name).is_some() {
        name = format!("chatgpt-{counter}");
        counter += 1;
    }
    let provider = ProviderProfile { name, kind: ProviderKind::ChatGpt, base_url: ProviderKind::ChatGpt.default_base_url().to_string(), ..ProviderProfile::default() };
    config.providers.push(provider.clone());
    runtime.save_config(config)?;
    println!("Added provider '{}' to {}.", provider.name, runtime.paths().config_file().display());
    Ok(provider)
}

fn unknown_provider(name: Option<&str>) -> ExitCode {
    match name {
        Some(name) => eprintln!("there is no ChatGPT provider named '{name}'"),
        None => eprintln!("no ChatGPT provider is configured; run `llm-subs chatgpt sign-in` first"),
    }
    ExitCode::from(2)
}

async fn print_chatgpt_models(runtime: &AppRuntime, provider: &ProviderProfile) {
    match runtime.llm().chatgpt.load_models(&provider.name).await {
        Ok(models) if models.is_empty() => println!("  no models are available to this account"),
        Ok(models) => {
            println!("  models:");
            for model in models {
                let efforts = if model.reasoning_levels.is_empty() { String::new() } else { format!("  (effort: {})", model.reasoning_levels.join(", ")) };
                println!("    {:<28} {}{efforts}", model.slug, model.display_name);
            }
        }
        Err(error) => println!("  could not list the models: {error:#}"),
    }
}

pub fn run_usage(runtime: &AppRuntime, days: Option<u64>) -> ExitCode {
    let since = days.map(|days| chrono::Local::now().date_naive() - chrono::Days::new(days.saturating_sub(1)));
    let summary = runtime.llm().usage.summary(since);
    let period = days.map(|days| format!("the last {days} day{}", if days == 1 { "" } else { "s" })).unwrap_or_else(|| "all time".to_string());
    println!("Usage over {period}: {} request{}, {}", summary.requests, if summary.requests == 1 { "" } else { "s" }, summary.usage.describe());
    let requests = |count: u64| format!("{count} request{}", if count == 1 { "" } else { "s" });
    let mut cost = format_usd(summary.cost_usd);
    if summary.plan_requests > 0 {
        cost.push_str(&format!(" + {} on a ChatGPT plan", requests(summary.plan_requests)));
    }
    if summary.unpriced_requests > 0 {
        cost.push_str(&format!(" + {} at an unknown price", requests(summary.unpriced_requests)));
    }
    println!("Cost: {cost}");
    if !summary.models.is_empty() {
        println!();
        println!("{:<28} {:<14} {:>8} {:>10} {:>10}  cost", "model", "provider", "requests", "input", "output");
        for row in &summary.models {
            let cost = if row.provider_kind == ProviderKind::ChatGpt {
                "ChatGPT plan".to_string()
            } else if row.unpriced_requests == row.requests {
                "unknown".to_string()
            } else {
                format_usd(row.cost_usd)
            };
            println!(
                "{:<28} {:<14} {:>8} {:>10} {:>10}  {cost}",
                row.model,
                row.provider,
                row.requests,
                format_token_count(row.usage.input_tokens),
                format_token_count(row.usage.output_tokens)
            );
        }
    }
    ExitCode::SUCCESS
}
