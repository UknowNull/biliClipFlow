use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::config::{resolve_ffmpeg_path, resolve_ffprobe_path};
use crate::utils::apply_no_window;

pub fn run_ffmpeg(args: &[String]) -> Result<(), String> {
    let ffmpeg_path = resolve_ffmpeg_path();
    let mut command = Command::new(ffmpeg_path);
    apply_no_window(&mut command);
    let output = command
        .args(args)
        .output()
        .map_err(|err| format!("Failed to start FFmpeg: {}", err))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(format!("FFmpeg failed: {}", stderr.trim()))
}

pub fn run_ffmpeg_capture(args: &[String]) -> Result<(String, String), String> {
    let ffmpeg_path = resolve_ffmpeg_path();
    let mut command = Command::new(ffmpeg_path);
    apply_no_window(&mut command);
    let output = command
        .args(args)
        .output()
        .map_err(|err| format!("Failed to start FFmpeg: {}", err))?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    if output.status.success() {
        return Ok((stdout, stderr));
    }

    Err(format!("FFmpeg failed: {}", stderr.trim()))
}

pub fn run_ffmpeg_with_progress<F>(
    args: &[String],
    duration_ms: Option<i64>,
    mut on_progress: F,
) -> Result<(), String>
where
    F: FnMut(i64),
{
    let ffmpeg_path = resolve_ffmpeg_path();
    let mut command = Command::new(ffmpeg_path);
    apply_no_window(&mut command);
    let mut child = command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("Failed to start FFmpeg: {}", err))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Failed to capture FFmpeg stdout".to_string())?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Failed to capture FFmpeg stderr".to_string())?;

    let (stderr_tx, stderr_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = String::new();
        let _ = stderr.read_to_string(&mut buffer);
        let _ = stderr_tx.send(buffer);
    });

    let total_ms = duration_ms.unwrap_or(0);
    let mut last_progress = -1;
    let reader = BufReader::new(stdout);
    for line in reader.lines().flatten() {
        if total_ms <= 0 {
            continue;
        }
        if let Some(value) = line.strip_prefix("out_time=") {
            if let Some(elapsed_ms) = parse_out_time_ms(value) {
                let mut progress = ((elapsed_ms as f64 / total_ms as f64) * 100.0).floor() as i64;
                if progress > 99 {
                    progress = 99;
                }
                if progress > last_progress {
                    last_progress = progress;
                    on_progress(progress);
                }
            }
        } else if let Some(value) = line.strip_prefix("out_time_ms=") {
            if let Ok(raw) = value.trim().parse::<i64>() {
                let elapsed_ms = raw / 1000;
                let mut progress = ((elapsed_ms as f64 / total_ms as f64) * 100.0).floor() as i64;
                if progress > 99 {
                    progress = 99;
                }
                if progress > last_progress {
                    last_progress = progress;
                    on_progress(progress);
                }
            }
        }
    }

    let status = child
        .wait()
        .map_err(|err| format!("Failed to wait for FFmpeg: {}", err))?;
    let stderr_output = stderr_rx.recv().unwrap_or_default();

    if status.success() {
        return Ok(());
    }

    Err(format!("FFmpeg failed: {}", stderr_output.trim()))
}

pub fn run_ffprobe_json(args: &[String]) -> Result<Value, String> {
    let ffprobe_path = resolve_ffprobe_path();
    let mut command = Command::new(ffprobe_path);
    apply_no_window(&mut command);
    let output = command
        .args(args)
        .output()
        .map_err(|err| format!("Failed to start FFprobe: {}", err))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("FFprobe failed: {}", stderr.trim()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).map_err(|err| format!("Failed to parse FFprobe json: {}", err))
}

pub fn run_ffprobe_capture(args: &[String]) -> Result<(String, String), String> {
    let ffprobe_path = resolve_ffprobe_path();
    let mut command = Command::new(ffprobe_path);
    apply_no_window(&mut command);
    let output = command
        .args(args)
        .output()
        .map_err(|err| format!("Failed to start FFprobe: {}", err))?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    if output.status.success() {
        return Ok((stdout, stderr));
    }

    Err(format!("FFprobe failed: {}", stderr.trim()))
}

fn parse_out_time_ms(value: &str) -> Option<i64> {
    let parts: Vec<&str> = value.trim().split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let hours: i64 = parts[0].parse().ok()?;
    let minutes: i64 = parts[1].parse().ok()?;
    let seconds_part = parts[2];
    let (secs, millis) = if let Some((sec_str, frac_str)) = seconds_part.split_once('.') {
        let secs: i64 = sec_str.parse().ok()?;
        let frac = frac_str.chars().take(3).collect::<String>();
        let millis = frac.parse::<i64>().unwrap_or(0);
        (secs, millis)
    } else {
        (seconds_part.parse().ok()?, 0)
    };
    Some((hours * 3600 + minutes * 60 + secs) * 1000 + millis)
}
