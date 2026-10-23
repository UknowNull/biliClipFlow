//! 切片生成：重编码裁切，起点精确、开头无黑屏。macOS 优先硬件编码，失败回退 libx264。

use std::path::Path;

use crate::ffmpeg::run_ffmpeg;

fn args(source: &str, start: f64, duration: f64, output: &Path, hardware: bool) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "-hide_banner", "-loglevel", "error", "-y", "-nostdin",
        "-ss", &format!("{:.3}", start.max(0.0)),
        "-i", source,
        "-t", &format!("{:.3}", duration),
        "-map", "0:v:0", "-map", "0:a:0?",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    if hardware {
        a.extend(["-c:v", "h264_videotoolbox", "-b:v", "8000k", "-maxrate", "12000k"].map(String::from));
    } else {
        a.extend(["-c:v", "libx264", "-preset", "veryfast", "-crf", "20"].map(String::from));
    }
    a.extend(["-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart"].map(String::from));
    a.push(output.to_string_lossy().to_string());
    a
}

/// 从录播文件的 `start` 秒起裁出 `duration` 秒，重编码写到 `output`
pub fn cut(source: &str, start: f64, duration: f64, output: &Path) -> Result<(), String> {
    if cfg!(target_os = "macos") && run_ffmpeg(&args(source, start, duration, output, true)).is_ok() {
        return Ok(());
    }
    run_ffmpeg(&args(source, start, duration, output, false)).map_err(|e| format!("剪辑失败: {}", e))
}
