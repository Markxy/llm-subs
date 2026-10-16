//! Small operating-system specific helpers.

mod autostart;
mod console;
pub mod notifications;
mod window_scaling;

pub use autostart::{is_run_on_startup_enabled, set_run_on_startup, HIDDEN_START_FLAG};
pub use console::attach_parent_console;
pub use window_scaling::follow_suggested_rect_on_scale_change;

use std::path::Path;

/// Opens a file or folder with the system's default handler.
pub fn open_path(path: &Path) -> anyhow::Result<()> {
    open::that_detached(path).map_err(|e| anyhow::anyhow!("could not open {}: {e}", path.display()))
}

/// Shows the folder that contains `path` in the file manager.
pub fn reveal_in_folder(path: &Path) -> anyhow::Result<()> {
    let target = if path.is_dir() { path } else { path.parent().unwrap_or(path) };
    open_path(target)
}
