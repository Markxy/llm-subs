use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::config::WatchConfig;
use crate::media::classify_path;

/// Media and subtitle files already present in a folder.
pub fn scan_existing_files(folder: &Path, recursive: bool, config: &WatchConfig) -> Vec<PathBuf> {
    let max_depth = if recursive { usize::MAX } else { 1 };
    WalkDir::new(folder)
        .max_depth(max_depth)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !is_hidden(entry.path()))
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .filter(|path| classify_path(path, config).is_some() && !crate::media::kinds::looks_like_incomplete_file(path))
        .collect()
}

fn is_hidden(path: &Path) -> bool {
    path.file_name().and_then(|n| n.to_str()).is_some_and(|name| name.starts_with('.') && name.len() > 1)
}
