use std::path::{Path, PathBuf};

use crate::commands::settings::load_download_settings_from_db;
use crate::config::default_download_dir;
use crate::db::Db;

fn normalize_relative_path_text(value: &str) -> String {
    let normalized = value.replace('\\', "/");
    let mut parts: Vec<&str> = Vec::new();
    for part in normalized.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        parts.push(part);
    }
    parts.join("/")
}

fn is_windows_drive_absolute(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

fn is_unc_absolute(value: &str) -> bool {
    value.starts_with("\\\\") || value.starts_with("//")
}

pub fn is_absolute_like_path(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return false;
    }
    Path::new(trimmed).is_absolute()
        || is_windows_drive_absolute(trimmed)
        || is_unc_absolute(trimmed)
}

pub fn load_local_path_prefix(db: &Db) -> PathBuf {
    let configured = load_download_settings_from_db(db)
        .map(|settings| settings.download_path)
        .ok()
        .unwrap_or_default();
    if configured.trim().is_empty() {
        return default_download_dir();
    }
    PathBuf::from(configured.trim())
}

pub fn to_stored_local_path_with_prefix(prefix: &Path, value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if !is_absolute_like_path(trimmed) {
        return normalize_relative_path_text(trimmed);
    }
    let absolute = PathBuf::from(trimmed);
    if absolute.starts_with(prefix) {
        if let Ok(relative) = absolute.strip_prefix(prefix) {
            let normalized = normalize_relative_path_text(&relative.to_string_lossy());
            if !normalized.is_empty() {
                return normalized;
            }
        }
    }
    trimmed.to_string()
}

fn absolute_like_to_portable_relative_text(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let normalized = trimmed.replace('\\', "/");
    let without_drive = if is_windows_drive_absolute(trimmed) && normalized.len() >= 2 {
        normalized[2..].to_string()
    } else {
        normalized
    };
    let without_root = without_drive.trim_start_matches('/');
    normalize_relative_path_text(without_root)
}

pub fn to_portable_relative_path_with_prefix(prefix: &Path, value: &str) -> String {
    let stored = to_stored_local_path_with_prefix(prefix, value);
    if stored.trim().is_empty() {
        return String::new();
    }
    if is_absolute_like_path(&stored) {
        return absolute_like_to_portable_relative_text(&stored);
    }
    normalize_relative_path_text(&stored)
}

pub fn to_stored_local_path(db: &Db, value: &str) -> String {
    let prefix = load_local_path_prefix(db);
    to_stored_local_path_with_prefix(prefix.as_path(), value)
}

pub fn to_absolute_local_path_with_prefix(prefix: &Path, value: &str) -> PathBuf {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return PathBuf::new();
    }
    if is_absolute_like_path(trimmed) {
        return PathBuf::from(trimmed);
    }
    let normalized = normalize_relative_path_text(trimmed);
    if normalized.is_empty() {
        return prefix.to_path_buf();
    }
    let mut relative = PathBuf::new();
    for part in normalized.split('/') {
        relative.push(part);
    }
    prefix.join(relative)
}

pub fn to_absolute_local_path(db: &Db, value: &str) -> PathBuf {
    let prefix = load_local_path_prefix(db);
    to_absolute_local_path_with_prefix(prefix.as_path(), value)
}

pub fn to_absolute_local_path_opt_with_prefix(
    prefix: &Path,
    value: Option<String>,
) -> Option<String> {
    value.and_then(|raw| {
        let path = to_absolute_local_path_with_prefix(prefix, &raw);
        if path.as_os_str().is_empty() {
            None
        } else {
            Some(path.to_string_lossy().to_string())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_relative_for_path_under_prefix() {
        let prefix = Path::new("/data/videos");
        let actual =
            to_portable_relative_path_with_prefix(prefix, "/data/videos/task/output/part-01.mp4");
        assert_eq!(actual, "task/output/part-01.mp4");
    }

    #[test]
    fn portable_relative_for_windows_absolute_path() {
        let prefix = Path::new("/Users/demo/Downloads");
        let actual =
            to_portable_relative_path_with_prefix(prefix, r"D:\work\cut\mqnl\recording\source.mp4");
        assert_eq!(actual, "work/cut/mqnl/recording/source.mp4");
    }

    #[test]
    fn portable_relative_for_relative_path() {
        let prefix = Path::new("/Users/demo/Downloads");
        let actual = to_portable_relative_path_with_prefix(prefix, r"task\output\part-01.mp4");
        assert_eq!(actual, "task/output/part-01.mp4");
    }
}
