use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::Utc;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

pub fn sanitize_filename(name: &str) -> String {
    let mut sanitized = String::with_capacity(name.len());
    for ch in name.chars() {
        let is_invalid = matches!(ch, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|');
        sanitized.push(if is_invalid { '_' } else { ch });
    }
    sanitized
}

pub fn build_output_path(base_dir: &str, folder: &str, file_name: &str) -> PathBuf {
    let mut path = PathBuf::from(base_dir);
    path.push(folder);
    path.push(file_name);
    path
}

pub fn append_log(path: &Path, message: &str) {
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "ts={} {}", now_rfc3339(), message);
    }
}

#[cfg(target_os = "windows")]
pub fn apply_no_window(command: &mut Command) {
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(target_os = "windows"))]
pub fn apply_no_window(_command: &mut Command) {}
