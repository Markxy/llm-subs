use anyhow::{Context, Result};
use auto_launch::AutoLaunchBuilder;

/// Passed on the autostart command line so the window starts hidden in the tray.
pub const HIDDEN_START_FLAG: &str = "--hidden";

fn launcher() -> Result<auto_launch::AutoLaunch> {
    let exe = std::env::current_exe().context("locating the running executable")?;
    let mut builder = AutoLaunchBuilder::new();
    builder.set_app_name("LLM Subs").set_app_path(&exe.to_string_lossy()).set_args(&[HIDDEN_START_FLAG]);
    #[cfg(target_os = "macos")]
    builder.set_use_launch_agent(true);
    builder.build().context("preparing the autostart entry")
}

pub fn is_run_on_startup_enabled() -> bool {
    launcher().and_then(|l| l.is_enabled().map_err(Into::into)).unwrap_or(false)
}

pub fn set_run_on_startup(enabled: bool) -> Result<()> {
    let launcher = launcher()?;
    if enabled {
        launcher.enable().context("enabling run on startup")
    } else if launcher.is_enabled().unwrap_or(false) {
        launcher.disable().context("disabling run on startup")
    } else {
        Ok(())
    }
}
