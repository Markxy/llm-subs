//! Small operating-system specific helpers.

mod autostart;
mod console;
#[cfg(target_os = "linux")]
mod desktop_entry;
pub mod notifications;
mod window_scaling;

pub use autostart::{is_run_on_startup_enabled, set_run_on_startup, HIDDEN_START_FLAG};
pub use console::attach_parent_console;
pub use window_scaling::follow_suggested_rect_on_scale_change;

use std::path::Path;

/// Names the app's windows to the desktop: the Wayland app id and the X11 window class.
pub const APP_ID: &str = "llm-subs";

/// Gives the app's windows their name and `icon_png` in the Linux dock and app grid.
pub fn install_desktop_entry(icon_png: &Path) {
    #[cfg(target_os = "linux")]
    desktop_entry::install(icon_png);
    #[cfg(not(target_os = "linux"))]
    let _ = icon_png;
}

/// Opens a file or folder with the system's default handler.
pub fn open_path(path: &Path) -> anyhow::Result<()> {
    open::that_detached(path).map_err(|e| anyhow::anyhow!("could not open {}: {e}", path.display()))
}

/// Shows the folder that contains `path` in the file manager.
pub fn reveal_in_folder(path: &Path) -> anyhow::Result<()> {
    let target = if path.is_dir() { path } else { path.parent().unwrap_or(path) };
    open_path(target)
}
