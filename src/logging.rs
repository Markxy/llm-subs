//! Log to a daily rotating file in the data directory, and to stderr when a console
//! is attached.

use std::path::Path;

use anyhow::Result;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// Keep this alive for as long as logging should flush.
pub struct LogGuard {
    _worker: tracing_appender::non_blocking::WorkerGuard,
}

/// `stderr_level` additionally mirrors messages at or above that level to stderr; the
/// command line uses `WARN` so progress output stays readable.
pub fn init(log_dir: &Path, level: &str, stderr_level: Option<LevelFilter>) -> Result<LogGuard> {
    std::fs::create_dir_all(log_dir)?;
    let file_appender = tracing_appender::rolling::daily(log_dir, "llm-subs.log");
    let (file_writer, worker) = tracing_appender::non_blocking(file_appender);

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(format!(
            "{level},hyper=warn,hyper_util=warn,reqwest=warn,rustls=warn,notify=warn,egui=warn,eframe=warn,\
             egui_glow=warn,winit=warn,zbus=warn,tracing=warn,h2=warn"
        ))
    });
    let file_layer = tracing_subscriber::fmt::layer().with_ansi(false).with_target(false).with_writer(file_writer);
    let registry = tracing_subscriber::registry().with(filter).with(file_layer);
    match stderr_level {
        Some(level) => registry
            .with(tracing_subscriber::fmt::layer().with_target(false).without_time().with_writer(std::io::stderr).with_filter(level))
            .init(),
        None => registry.init(),
    }
    Ok(LogGuard { _worker: worker })
}
