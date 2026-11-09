//! 切片生成：重编码裁切，起点精确、开头无黑屏。macOS 优先硬件编码，失败回退 libx264。

use std::path::Path;

use crate::ffmpeg::run_ffmpeg;

fn args(source: &str, start: f64, duration: f64, output: &Path, hardware: bool, ass: Option<&Path>) -> Vec<String> {
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
    if let Some(ass) = ass {
        // 滤镜参数里 \ : ' 有特殊含义，需要转义
        let escaped = ass.to_string_lossy().replace('\\', "\\\\").replace(':', "\\:").replace('\'', "\\'");
        a.extend(["-vf".to_string(), format!("ass=filename={}", escaped)]);
    }
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
    cut_with(source, start, duration, output, None)
}

/// 同 `cut`，并把 `ass` 字幕文件压进画面
pub fn cut_with(source: &str, start: f64, duration: f64, output: &Path, ass: Option<&Path>) -> Result<(), String> {
    if cfg!(target_os = "macos") && run_ffmpeg(&args(source, start, duration, output, true, ass)).is_ok() {
        return Ok(());
    }
    run_ffmpeg(&args(source, start, duration, output, false, ass)).map_err(|e| format!("剪辑失败: {}", e))
}

pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// 按评阅页的字幕样式生成 ASS。样式尺寸按 1080p 像素记，位置是字幕框中心占画面的百分比，boxWidth 是字幕框宽度占比；
/// cue 时间是相对切片起点的秒数
pub fn build_ass(cues: &[Cue], style: &serde_json::Value, width: u32, height: u32) -> String {
    let num = |k: &str, d: f64| style.get(k).and_then(|v| v.as_f64()).unwrap_or(d);
    let flag = |k: &str| style.get(k).and_then(|v| v.as_bool()).unwrap_or(false);
    // #RRGGBB + 不透明度 → &HAABBGGRR
    let color = |k: &str, d: &str, opacity: f64| {
        let hex = style.get(k).and_then(|v| v.as_str()).map(|v| v.trim_start_matches('#')).filter(|h| h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit()));
        let hex = hex.unwrap_or(d.trim_start_matches('#'));
        let alpha = ((1.0 - opacity.clamp(0.0, 1.0)) * 255.0).round() as u8;
        format!("&H{:02X}{}{}{}", alpha, &hex[4..6], &hex[2..4], &hex[0..2])
    };
    let res_y = 1080.0;
    let res_x = (res_y * width.max(1) as f64 / height.max(1) as f64).round();
    let font = style.get("fontFamily").and_then(|v| v.as_str()).and_then(|f| f.split(',').next()).map(str::trim).filter(|f| !f.is_empty()).unwrap_or("PingFang SC");
    let size = num("fontSize", 54.0);
    let bg = num("bgOpacity", 0.0);
    // 有背景条时用 ASS 的不透明框（BorderStyle 3，框色取 OutlineColour），否则普通描边 + 阴影
    let (border_style, outline_color, outline, shadow) = if bg > 0.0 {
        (3, color("bgColor", "#000000", bg), 0.15 * size, 0.0)
    } else {
        (1, color("outlineColor", "#000000", 1.0), num("outlineWidth", 3.0), num("shadow", 2.0))
    };
    let margin = (res_x * (100.0 - num("boxWidth", 80.0).clamp(15.0, 100.0)) / 200.0).round();
    let (x, y) = (res_x * num("x", 50.0) / 100.0, res_y * num("y", 82.0) / 100.0);
    let mut out = format!(
        "[Script Info]\nScriptType: v4.00+\nPlayResX: {res_x}\nPlayResY: {res_y}\nWrapStyle: 0\nScaledBorderAndShadow: yes\n\n\
         [V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
         Style: Default,{font},{size},{primary},{primary},{outline_color},&H66000000,{bold},{italic},0,0,100,100,0,0,{border_style},{outline},{shadow},5,{margin},{margin},0,1\n\n\
         [Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n",
        primary = color("color", "#FFFFFF", 1.0),
        bold = if flag("bold") { -1 } else { 0 },
        italic = if flag("italic") { -1 } else { 0 },
    );
    let ts = |t: f64| {
        let cs = (t.max(0.0) * 100.0).round() as i64;
        format!("{}:{:02}:{:02}.{:02}", cs / 360000, cs / 6000 % 60, cs / 100 % 60, cs % 100)
    };
    for c in cues.iter().filter(|c| c.end > c.start && !c.text.trim().is_empty()) {
        // 花括号和反斜杠在 ASS 文本里是控制符，换成全角
        let text = c.text.trim().replace('{', "｛").replace('}', "｝").replace('\\', "＼").replace('\n', "\\N");
        out.push_str(&format!("Dialogue: 0,{},{},Default,,0,0,0,,{{\\pos({x:.0},{y:.0})}}{text}\n", ts(c.start), ts(c.end)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ass_follows_review_style() {
        let style = serde_json::json!({"fontFamily": "PingFang SC, sans-serif", "fontSize": 60, "color": "#FFCC00", "x": 50, "y": 80, "boxWidth": 60, "bold": true});
        let ass = build_ass(&[Cue { start: 1.5, end: 3.25, text: "你好{世界}".into() }], &style, 1920, 1080);
        assert!(ass.contains("Style: Default,PingFang SC,60,&H0000CCFF,"));
        assert!(ass.contains(",-1,0,0,0,100,100,0,0,1,3,2,5,384,384,0,1"));
        assert!(ass.contains("Dialogue: 0,0:00:01.50,0:00:03.25,Default,,0,0,0,,{\\pos(960,864)}你好｛世界｝"));
    }
}
