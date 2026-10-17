//! Linux desktops take an app's name and icon from the desktop entry named after its
//! window's app id; executables carry no icon of their own. Without an entry the dock
//! shows a generic one, so the app keeps an entry for the current user that points at
//! the running executable.

use std::path::Path;

use anyhow::{Context, Result};

use super::APP_ID;

/// Characters that make the desktop entry spec require an `Exec` argument to be quoted.
const RESERVED_EXEC_CHARS: &[char] = &[' ', '\t', '\n', '"', '\'', '\\', '>', '<', '~', '|', '&', ';', '$', '*', '?', '#', '(', ')', '`'];

pub fn install(icon_png: &Path) {
    if let Err(error) = write_entry(icon_png) {
        tracing::warn!(error = %format!("{error:#}"), "could not install the desktop entry");
    }
}

fn write_entry(icon_png: &Path) -> Result<()> {
    let executable = std::env::current_exe().context("locating the running executable")?;
    let dirs = directories::BaseDirs::new().context("could not determine the home directory")?;
    let path = dirs.data_dir().join("applications").join(format!("{APP_ID}.desktop"));
    let entry = entry_text(utf8(&executable)?, utf8(icon_png)?);
    if std::fs::read_to_string(&path).is_ok_and(|current| current == entry) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    crate::config::store::write_atomically(&path, entry.as_bytes()).with_context(|| format!("writing {}", path.display()))?;
    tracing::info!(path = %path.display(), "installed the desktop entry");
    Ok(())
}

fn utf8(path: &Path) -> Result<&str> {
    path.to_str().with_context(|| format!("{} is not valid UTF-8, which desktop entries require", path.display()))
}

fn entry_text(executable: &str, icon_png: &str) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=LLM Subs\n\
         Comment={comment}\n\
         Exec={exec}\n\
         Icon={icon}\n\
         Terminal=false\n\
         Categories=AudioVideo;Video;\n\
         Keywords=subtitles;translation;translate;srt;\n\
         StartupWMClass={APP_ID}\n",
        comment = env!("CARGO_PKG_DESCRIPTION"),
        exec = escape_string(&quote_exec_argument(executable)),
        icon = escape_string(icon_png),
    )
}

/// Quotes an argument as the `Exec` key requires; `%` starts a field code, so it is doubled.
fn quote_exec_argument(argument: &str) -> String {
    let argument = argument.replace('%', "%%");
    if !argument.contains(RESERVED_EXEC_CHARS) {
        return argument;
    }
    let mut quoted = String::with_capacity(argument.len() + 2);
    quoted.push('"');
    for character in argument.chars() {
        if matches!(character, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        quoted.push(character);
    }
    quoted.push('"');
    quoted
}

/// The escapes every string value in a desktop entry goes through.
fn escape_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\n', "\\n").replace('\t', "\\t").replace('\r', "\\r")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_paths_stay_unquoted() {
        assert_eq!(quote_exec_argument("/home/user/llm-subs"), "/home/user/llm-subs");
    }

    #[test]
    fn paths_with_reserved_characters_are_quoted_and_escaped() {
        assert_eq!(quote_exec_argument("/opt/LLM Subs/llm-subs"), "\"/opt/LLM Subs/llm-subs\"");
        assert_eq!(quote_exec_argument("/opt/$HOME \"x\"/llm-subs"), "\"/opt/\\$HOME \\\"x\\\"/llm-subs\"");
        assert_eq!(quote_exec_argument("/opt/100%/llm-subs"), "/opt/100%%/llm-subs");
    }

    #[test]
    fn a_backslash_in_the_path_takes_four_in_the_file() {
        let exec = escape_string(&quote_exec_argument("/opt/a\\b/llm-subs"));
        assert_eq!(exec, "\"/opt/a\\\\\\\\b/llm-subs\"");
    }

    #[test]
    fn the_entry_names_the_window_class_and_icon() {
        let entry = entry_text("/opt/llm subs/llm-subs", "/home/user/.local/share/llmsubs/icon.png");
        assert!(entry.starts_with("[Desktop Entry]\n"));
        assert!(entry.contains("\nExec=\"/opt/llm subs/llm-subs\"\n"));
        assert!(entry.contains("\nIcon=/home/user/.local/share/llmsubs/icon.png\n"));
        assert!(entry.contains("\nStartupWMClass=llm-subs\n"));
    }
}
