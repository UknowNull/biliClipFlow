use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde::Serialize;

use crate::api::ApiResponse;
use crate::config::{default_download_dir, resolve_ffmpeg_path};
use crate::utils::apply_no_window;

#[derive(Serialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_directory: bool,
    pub size: u64,
    pub last_modified: u64,
}

fn default_base_path() -> String {
    default_download_dir().to_string_lossy().to_string()
}

#[tauri::command]
pub fn validate_directory(path: String) -> ApiResponse<bool> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return ApiResponse::error("Download path is empty");
    }

    let dir_path = PathBuf::from(trimmed);
    if !dir_path.exists() {
        return ApiResponse::error(format!("Path does not exist: {}", trimmed));
    }

    if !dir_path.is_dir() {
        return ApiResponse::error(format!("Path is not a directory: {}", trimmed));
    }

    ApiResponse::success(true)
}

#[tauri::command]
pub fn scan_path(path: Option<String>) -> ApiResponse<Vec<FileEntry>> {
    let scan_path = path
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(default_base_path);
    let dir_path = PathBuf::from(&scan_path);

    if !dir_path.exists() {
        return ApiResponse::error(format!("Path does not exist: {}", scan_path));
    }

    if !dir_path.is_dir() {
        return ApiResponse::error(format!("Path is not a directory: {}", scan_path));
    }

    let mut entries = Vec::new();
    let read_dir = match fs::read_dir(&dir_path) {
        Ok(read_dir) => read_dir,
        Err(err) => return ApiResponse::error(format!("Failed to read directory: {}", err)),
    };

    for entry in read_dir.flatten() {
        let path = entry.path();
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };

        let name = match path.file_name().and_then(|value| value.to_str()) {
            Some(name) => name.to_string(),
            None => continue,
        };

        let last_modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0);

        entries.push(FileEntry {
            name,
            path: path.to_string_lossy().to_string(),
            is_directory: metadata.is_dir(),
            size: if metadata.is_dir() { 0 } else { metadata.len() },
            last_modified,
        });
    }

    entries.sort_by(|a, b| a.name.cmp(&b.name));

    ApiResponse::success(entries)
}

#[tauri::command]
pub fn video_duration(path: String) -> ApiResponse<i64> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return ApiResponse::error("Path is empty");
    }

    let ffmpeg_path = resolve_ffmpeg_path();
    let mut command = Command::new(ffmpeg_path);
    apply_no_window(&mut command);
    let output = match command.arg("-i").arg(trimmed).output() {
        Ok(output) => output,
        Err(err) => return ApiResponse::error(format!("Failed to start FFmpeg: {}", err)),
    };

    let mut combined = String::new();
    combined.push_str(&String::from_utf8_lossy(&output.stdout));
    combined.push_str(&String::from_utf8_lossy(&output.stderr));

    match parse_ffmpeg_duration(&combined) {
        Some(duration) => ApiResponse::success(duration),
        None => ApiResponse::error("Failed to parse video duration"),
    }
}

fn parse_ffmpeg_duration(text: &str) -> Option<i64> {
    let marker = "Duration:";
    let start = text.find(marker)?;
    let rest = text[start + marker.len()..].trim_start();
    let end = rest.find(',')?;
    let raw = rest[..end].trim();
    if raw == "N/A" {
        return None;
    }
    let parts: Vec<&str> = raw.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let hours: i64 = parts[0].parse().ok()?;
    let minutes: i64 = parts[1].parse().ok()?;
    let seconds: f64 = parts[2].parse().ok()?;
    let total = (hours as f64 * 3600.0) + (minutes as f64 * 60.0) + seconds;
    Some(total.floor() as i64)
}
