//! Desktop notifications: Windows toasts, the macOS notification centre and the
//! freedesktop notification service on Linux.

use std::path::Path;

/// Identifies the app to Windows, which shows the name and icon registered for it.
#[cfg(windows)]
const APP_USER_MODEL_ID: &str = "LLMSubs";

/// Lets notifications carry the app's name and `icon_png`. Only Windows needs this;
/// without it, toasts from an app that was not installed appear as PowerShell's.
pub fn register(icon_png: &Path) {
    #[cfg(windows)]
    windows::register(icon_png);
    #[cfg(not(windows))]
    let _ = icon_png;
}

/// Shows a notification without blocking the caller; a failure is only logged.
pub fn show(title: impl Into<String>, body: impl Into<String>) {
    let (title, body) = (title.into(), body.into());
    let spawned = std::thread::Builder::new().name("llm-subs-notification".into()).spawn(move || {
        let mut notification = notify_rust::Notification::new();
        notification.summary(&title).body(&body).appname("LLM Subs");
        #[cfg(windows)]
        notification.app_id(APP_USER_MODEL_ID);
        if let Err(error) = notification.show() {
            tracing::warn!(%error, "could not show a notification");
        }
    });
    if let Err(error) = spawned {
        tracing::warn!(%error, "could not show a notification");
    }
}

#[cfg(windows)]
mod windows {
    use std::path::Path;

    use windows_sys::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};

    use super::APP_USER_MODEL_ID;

    /// The registration Windows reads for apps without a Start menu shortcut.
    pub fn register(icon_png: &Path) {
        let key = format!(r"Software\Classes\AppUserModelId\{APP_USER_MODEL_ID}");
        set_string(&key, "DisplayName", "LLM Subs");
        set_string(&key, "IconUri", &icon_png.display().to_string());
    }

    /// Creates the key when it does not exist yet.
    fn set_string(key: &str, name: &str, value: &str) {
        let wide = |text: &str| text.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        let (key_w, name_w, value_w) = (wide(key), wide(name), wide(value));
        // SAFETY: every string is NUL-terminated UTF-16 that outlives the call, and the
        // byte count passed is that of the value including its terminator.
        let status = unsafe {
            RegSetKeyValueW(HKEY_CURRENT_USER, key_w.as_ptr(), name_w.as_ptr(), REG_SZ, value_w.as_ptr().cast(), (value_w.len() * 2) as u32)
        };
        if status != 0 {
            tracing::warn!(status, name, "could not register the app for notifications");
        }
    }
}
