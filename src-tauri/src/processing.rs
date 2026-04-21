use std::fs;
use std::path::{Path, PathBuf};

use crate::ffmpeg::{run_ffmpeg, run_ffprobe_json};
use serde_json::Value;

#[derive(Clone)]
pub struct ClipSource {
    pub input_path: String,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub order: i64,
}

pub struct ClipCopyDecision {
    pub use_copy: bool,
    pub reason: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TranscodeProfile {
    ClipAndMergeClean,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SegmentCopyMode {
    Fast,
    Precise,
}

pub fn clip_sources(
    sources: &[ClipSource],
    output_dir: &Path,
    use_copy: bool,
) -> Result<Vec<PathBuf>, String> {
    fs::create_dir_all(output_dir)
        .map_err(|err| format!("Failed to create output dir: {}", err))?;

    let mut outputs = Vec::new();
    for source in sources {
        let output_path = output_dir.join(format!("clip_{:03}.mp4", source.order));
        clip_single(source, &output_path, use_copy, None)?;
        outputs.push(output_path);
    }

    Ok(outputs)
}

pub fn clip_sources_transcode(
    sources: &[ClipSource],
    output_dir: &Path,
    profile: TranscodeProfile,
) -> Result<Vec<PathBuf>, String> {
    fs::create_dir_all(output_dir)
        .map_err(|err| format!("Failed to create output dir: {}", err))?;

    let mut outputs = Vec::new();
    for source in sources {
        let output_path = output_dir.join(format!("clip_{:03}.mp4", source.order));
        clip_single(source, &output_path, false, Some(&profile))?;
        outputs.push(output_path);
    }

    Ok(outputs)
}

pub fn merge_files(files: &[PathBuf], output_path: &Path) -> Result<(), String> {
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("Failed to create output dir: {}", err))?;
    }

    let list_path = output_path.with_extension("txt");
    let list_content = files
        .iter()
        .map(|path| format!("file '{}'", path.to_string_lossy()))
        .collect::<Vec<_>>()
        .join("\n");

    fs::write(&list_path, list_content)
        .map_err(|err| format!("Failed to write concat file: {}", err))?;

    let mut args = vec![
        "-f".to_string(),
        "concat".to_string(),
        "-safe".to_string(),
        "0".to_string(),
        "-i".to_string(),
        list_path.to_string_lossy().to_string(),
    ];

    args.push("-c".to_string());
    args.push("copy".to_string());

    args.push(output_path.to_string_lossy().to_string());

    run_ffmpeg(&args)?;
    let _ = fs::remove_file(list_path);
    Ok(())
}

pub fn merge_files_transcode(
    files: &[PathBuf],
    output_path: &Path,
    profile: TranscodeProfile,
) -> Result<(), String> {
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("Failed to create output dir: {}", err))?;
    }

    let list_path = output_path.with_extension("txt");
    let list_content = files
        .iter()
        .map(|path| format!("file '{}'", path.to_string_lossy()))
        .collect::<Vec<_>>()
        .join("\n");

    fs::write(&list_path, list_content)
        .map_err(|err| format!("Failed to write concat file: {}", err))?;

    let input_args = vec![
        "-f".to_string(),
        "concat".to_string(),
        "-safe".to_string(),
        "0".to_string(),
        "-fflags".to_string(),
        "+genpts".to_string(),
        "-i".to_string(),
        list_path.to_string_lossy().to_string(),
    ];

    let result =
        run_transcode_with_fallback(&input_args, output_path, profile, "merge_transcode_fail");
    let _ = fs::remove_file(list_path);
    result
}

pub fn decide_clip_copy(_sources: &[ClipSource]) -> Result<ClipCopyDecision, String> {
    Ok(ClipCopyDecision {
        use_copy: true,
        reason: Some("forced_copy_mode".to_string()),
    })
}

pub fn parse_time_to_seconds(value: &str) -> Option<f64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let parts: Vec<&str> = trimmed.split(':').collect();
    let (hours, minutes, seconds) = match parts.len() {
        3 => (
            parts[0].parse::<f64>().ok()?,
            parts[1].parse::<f64>().ok()?,
            parts[2].parse::<f64>().ok()?,
        ),
        2 => (
            0.0,
            parts[0].parse::<f64>().ok()?,
            parts[1].parse::<f64>().ok()?,
        ),
        1 => (0.0, 0.0, parts[0].parse::<f64>().ok()?),
        _ => return None,
    };
    Some(hours * 3600.0 + minutes * 60.0 + seconds)
}

fn finalize_clip_output(_source: &ClipSource, _output_path: &Path) -> Result<(), String> {
    Ok(())
}

fn transcode_video_codec_args(profile: TranscodeProfile, use_hardware: bool) -> Vec<String> {
    match profile {
        TranscodeProfile::ClipAndMergeClean => {
            if use_hardware {
                vec![
                    "-c:v".to_string(),
                    "h264_videotoolbox".to_string(),
                    "-profile:v".to_string(),
                    "high".to_string(),
                    "-pix_fmt".to_string(),
                    "yuv420p".to_string(),
                    "-g".to_string(),
                    "60".to_string(),
                ]
            } else {
                vec![
                    "-c:v".to_string(),
                    "libx264".to_string(),
                    "-preset".to_string(),
                    "veryfast".to_string(),
                    "-crf".to_string(),
                    "18".to_string(),
                    "-pix_fmt".to_string(),
                    "yuv420p".to_string(),
                    "-g".to_string(),
                    "60".to_string(),
                ]
            }
        }
    }
}

fn transcode_audio_filter(profile: TranscodeProfile) -> Option<&'static str> {
    match profile {
        // Rebuild the audio timeline during transcode so broken AAC packet timestamps
        // do not leak into clipped / merged / segmented outputs.
        TranscodeProfile::ClipAndMergeClean => Some("aresample=async=1:first_pts=0"),
    }
}

fn build_transcode_attempts(
    input_args: &[String],
    output_path: &Path,
    profile: TranscodeProfile,
) -> [Vec<String>; 2] {
    let output = output_path.to_string_lossy().to_string();
    let mut hardware_args = input_args.to_vec();
    hardware_args.extend([
        "-map".to_string(),
        "0:v:0".to_string(),
        "-map".to_string(),
        "0:a?".to_string(),
    ]);
    hardware_args.extend(transcode_video_codec_args(profile, true));
    if let Some(filter) = transcode_audio_filter(profile) {
        hardware_args.extend(["-af".to_string(), filter.to_string()]);
    }
    hardware_args.extend([
        "-c:a".to_string(),
        "aac".to_string(),
        "-ar".to_string(),
        "48000".to_string(),
        "-ac".to_string(),
        "2".to_string(),
        "-b:a".to_string(),
        "192k".to_string(),
        "-movflags".to_string(),
        "+faststart".to_string(),
        "-avoid_negative_ts".to_string(),
        "make_zero".to_string(),
        output.clone(),
    ]);

    let mut software_args = input_args.to_vec();
    software_args.extend([
        "-map".to_string(),
        "0:v:0".to_string(),
        "-map".to_string(),
        "0:a?".to_string(),
    ]);
    software_args.extend(transcode_video_codec_args(profile, false));
    if let Some(filter) = transcode_audio_filter(profile) {
        software_args.extend(["-af".to_string(), filter.to_string()]);
    }
    software_args.extend([
        "-c:a".to_string(),
        "aac".to_string(),
        "-ar".to_string(),
        "48000".to_string(),
        "-ac".to_string(),
        "2".to_string(),
        "-b:a".to_string(),
        "192k".to_string(),
        "-movflags".to_string(),
        "+faststart".to_string(),
        "-avoid_negative_ts".to_string(),
        "make_zero".to_string(),
        output,
    ]);

    [hardware_args, software_args]
}

fn build_segment_batch_transcode_attempts(
    input_path: &Path,
    output_dir: &Path,
    segment_seconds: i64,
    profile: TranscodeProfile,
) -> [Vec<String>; 2] {
    let pattern = output_dir.join("part_%03d.mp4");
    let force_key_frames_expr = format!("expr:gte(t,n_forced*{})", segment_seconds.max(1));
    let build_args = |use_hardware: bool| {
        let mut args = vec![
            "-fflags".to_string(),
            "+genpts".to_string(),
            "-i".to_string(),
            input_path.to_string_lossy().to_string(),
            "-map".to_string(),
            "0:v:0".to_string(),
            "-map".to_string(),
            "0:a?".to_string(),
        ];
        args.extend(transcode_video_codec_args(profile, use_hardware));
        if let Some(filter) = transcode_audio_filter(profile) {
            args.extend(["-af".to_string(), filter.to_string()]);
        }
        args.extend([
            "-c:a".to_string(),
            "aac".to_string(),
            "-ar".to_string(),
            "48000".to_string(),
            "-ac".to_string(),
            "2".to_string(),
            "-b:a".to_string(),
            "192k".to_string(),
            "-movflags".to_string(),
            "+faststart".to_string(),
            "-avoid_negative_ts".to_string(),
            "make_zero".to_string(),
            "-force_key_frames".to_string(),
            force_key_frames_expr.clone(),
            "-f".to_string(),
            "segment".to_string(),
            "-segment_time".to_string(),
            segment_seconds.to_string(),
            "-segment_time_delta".to_string(),
            "0.05".to_string(),
            "-reset_timestamps".to_string(),
            "1".to_string(),
            pattern.to_string_lossy().to_string(),
        ]);
        args
    };

    [build_args(true), build_args(false)]
}

fn run_transcode_with_fallback(
    input_args: &[String],
    output_path: &Path,
    profile: TranscodeProfile,
    error_prefix: &str,
) -> Result<(), String> {
    let attempts = build_transcode_attempts(input_args, output_path, profile);
    let mut errors = Vec::new();
    for args in attempts {
        if output_path.exists() {
            let _ = fs::remove_file(output_path);
        }
        match run_ffmpeg(&args) {
            Ok(_) => return Ok(()),
            Err(err) => errors.push(err),
        }
    }
    Err(format!("{}: {}", error_prefix, errors.join(" | ")))
}

fn segment_file_batch_transcode(
    input_path: &Path,
    output_dir: &Path,
    segment_seconds: i64,
    profile: TranscodeProfile,
) -> Result<Vec<PathBuf>, String> {
    cleanup_segment_outputs(output_dir)?;
    let attempts =
        build_segment_batch_transcode_attempts(input_path, output_dir, segment_seconds, profile);
    let mut errors = Vec::new();
    for args in attempts {
        cleanup_segment_outputs(output_dir)?;
        match run_ffmpeg(&args) {
            Ok(_) => return collect_segment_outputs(output_dir),
            Err(err) => errors.push(err),
        }
    }
    Err(format!(
        "segment_batch_transcode_fail: {}",
        errors.join(" | ")
    ))
}

fn format_ffmpeg_time(seconds: f64) -> String {
    let safe = if seconds.is_finite() {
        seconds.max(0.0)
    } else {
        0.0
    };
    let total_millis = (safe * 1000.0).round() as i64;
    let hours = total_millis / 3_600_000;
    let minutes = (total_millis % 3_600_000) / 60_000;
    let secs = (total_millis % 60_000) / 1000;
    let millis = total_millis % 1000;
    format!("{:02}:{:02}:{:02}.{:03}", hours, minutes, secs, millis)
}

fn last_keyframe_at_or_before(data: &Value, start_seconds: f64) -> Option<f64> {
    let frames = data.get("frames")?.as_array()?;
    let mut last_match = None;
    for frame in frames {
        let timestamp = frame
            .get("best_effort_timestamp_time")
            .and_then(|value| value.as_str())
            .and_then(|value| value.parse::<f64>().ok())?;
        if timestamp <= start_seconds + 0.001 {
            last_match = Some(timestamp);
        } else {
            break;
        }
    }
    last_match
}

fn probe_previous_keyframe_seconds(path: &Path, start_seconds: f64) -> Result<Option<f64>, String> {
    if start_seconds <= 0.0 {
        return Ok(None);
    }

    let mut probe_end = start_seconds + 0.5;
    let mut window = 12.0;

    for _ in 0..4 {
        let probe_start = (probe_end - window).max(0.0);
        let interval = format!("{:.3}%+{:.3}", probe_start, probe_end - probe_start);
        let args = vec![
            "-v".to_string(),
            "error".to_string(),
            "-read_intervals".to_string(),
            interval,
            "-skip_frame".to_string(),
            "nokey".to_string(),
            "-select_streams".to_string(),
            "v:0".to_string(),
            "-show_frames".to_string(),
            "-show_entries".to_string(),
            "frame=best_effort_timestamp_time".to_string(),
            "-of".to_string(),
            "json".to_string(),
            path.to_string_lossy().to_string(),
        ];
        let data = run_ffprobe_json(&args)?;
        if let Some(keyframe) = last_keyframe_at_or_before(&data, start_seconds) {
            return Ok(Some(keyframe));
        }

        if probe_start <= 0.0 {
            break;
        }
        probe_end = probe_start;
        window = (window * 2.0).min(start_seconds + 0.5);
    }

    Ok(None)
}

pub fn probe_duration_seconds(path: &Path) -> Result<f64, String> {
    let args = vec![
        "-v".to_string(),
        "error".to_string(),
        "-show_entries".to_string(),
        "format=duration".to_string(),
        "-of".to_string(),
        "json".to_string(),
        path.to_string_lossy().to_string(),
    ];
    let data = run_ffprobe_json(&args)?;
    let duration = data
        .get("format")
        .and_then(|value| value.get("duration"))
        .and_then(|value| value.as_str())
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(0.0);
    if duration <= 0.0 {
        return Err("无法读取视频时长".to_string());
    }
    Ok(duration)
}

fn segment_single_copy_fast(
    input_path: &Path,
    output_path: &Path,
    start_seconds: f64,
    duration_seconds: f64,
) -> Result<(), String> {
    let args = vec![
        "-ss".to_string(),
        format_ffmpeg_time(start_seconds),
        "-i".to_string(),
        input_path.to_string_lossy().to_string(),
        "-t".to_string(),
        format_ffmpeg_time(duration_seconds),
        "-c".to_string(),
        "copy".to_string(),
        output_path.to_string_lossy().to_string(),
    ];
    run_ffmpeg(&args)
}

fn segment_single_copy_precise(
    input_path: &Path,
    output_path: &Path,
    start_seconds: f64,
    duration_seconds: f64,
) -> Result<(), String> {
    let args = vec![
        "-i".to_string(),
        input_path.to_string_lossy().to_string(),
        "-ss".to_string(),
        format_ffmpeg_time(start_seconds),
        "-t".to_string(),
        format_ffmpeg_time(duration_seconds),
        "-map".to_string(),
        "0".to_string(),
        "-c".to_string(),
        "copy".to_string(),
        "-avoid_negative_ts".to_string(),
        "make_zero".to_string(),
        "-muxpreload".to_string(),
        "0".to_string(),
        "-muxdelay".to_string(),
        "0".to_string(),
        output_path.to_string_lossy().to_string(),
    ];
    run_ffmpeg(&args)
}

fn segment_single_copy(
    mode: SegmentCopyMode,
    input_path: &Path,
    output_path: &Path,
    start_seconds: f64,
    duration_seconds: f64,
) -> Result<(), String> {
    match mode {
        SegmentCopyMode::Fast => {
            segment_single_copy_fast(input_path, output_path, start_seconds, duration_seconds)
        }
        SegmentCopyMode::Precise => {
            segment_single_copy_precise(input_path, output_path, start_seconds, duration_seconds)
        }
    }
}

fn segment_single_transcode(
    input_path: &Path,
    output_path: &Path,
    start_seconds: f64,
    duration_seconds: f64,
    profile: TranscodeProfile,
) -> Result<(), String> {
    let args = vec![
        "-fflags".to_string(),
        "+genpts".to_string(),
        "-i".to_string(),
        input_path.to_string_lossy().to_string(),
        "-ss".to_string(),
        format_ffmpeg_time(start_seconds),
        "-t".to_string(),
        format_ffmpeg_time(duration_seconds),
    ];
    run_transcode_with_fallback(&args, output_path, profile, "segment_transcode_fail").map_err(
        |err| {
            format!(
                "segment_transcode_fail input={} output={} start={} duration={} err={}",
                input_path.to_string_lossy(),
                output_path.to_string_lossy(),
                format_ffmpeg_time(start_seconds),
                format_ffmpeg_time(duration_seconds),
                err
            )
        },
    )
}

fn collect_segment_outputs(output_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut outputs = fs::read_dir(output_dir)
        .map_err(|err| format!("Failed to read segment dir: {}", err))?
        .filter_map(|entry| entry.ok().map(|item| item.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.starts_with("part_") && name.ends_with(".mp4"))
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    outputs.sort();
    Ok(outputs)
}

fn cleanup_segment_outputs(output_dir: &Path) -> Result<(), String> {
    if !output_dir.exists() {
        return Ok(());
    }
    for path in collect_segment_outputs(output_dir)? {
        let _ = fs::remove_file(path);
    }
    Ok(())
}

fn segment_file_batch_copy(
    input_path: &Path,
    output_dir: &Path,
    segment_seconds: i64,
) -> Result<Vec<PathBuf>, String> {
    cleanup_segment_outputs(output_dir)?;
    let pattern = output_dir.join("part_%03d.mp4");
    let args = vec![
        "-i".to_string(),
        input_path.to_string_lossy().to_string(),
        "-map".to_string(),
        "0".to_string(),
        "-c".to_string(),
        "copy".to_string(),
        "-f".to_string(),
        "segment".to_string(),
        "-segment_time".to_string(),
        segment_seconds.to_string(),
        "-reset_timestamps".to_string(),
        "1".to_string(),
        pattern.to_string_lossy().to_string(),
    ];
    run_ffmpeg(&args)?;
    collect_segment_outputs(output_dir)
}

fn merge_last_short_segment(outputs: &mut Vec<PathBuf>, min_seconds: f64) -> Result<(), String> {
    if outputs.len() < 2 {
        return Ok(());
    }
    let last_index = outputs.len() - 1;
    let prev_index = outputs.len() - 2;
    let last_path = outputs[last_index].clone();
    let prev_path = outputs[prev_index].clone();
    let last_duration = probe_duration_seconds(&last_path)?;
    if last_duration >= min_seconds {
        return Ok(());
    }

    let output_dir = prev_path
        .parent()
        .ok_or_else(|| "无法读取分段目录".to_string())?;
    let list_path = output_dir.join("concat_tail.txt");
    let list_content = format!(
        "file '{}'\nfile '{}'",
        prev_path.to_string_lossy(),
        last_path.to_string_lossy()
    );
    fs::write(&list_path, list_content)
        .map_err(|err| format!("Failed to write concat file: {}", err))?;
    let merged_temp = output_dir.join("tail_merge.mp4");
    let args = vec![
        "-f".to_string(),
        "concat".to_string(),
        "-safe".to_string(),
        "0".to_string(),
        "-i".to_string(),
        list_path.to_string_lossy().to_string(),
        "-c".to_string(),
        "copy".to_string(),
        merged_temp.to_string_lossy().to_string(),
    ];
    run_ffmpeg(&args)?;
    let _ = fs::remove_file(&list_path);

    fs::rename(&merged_temp, &prev_path)
        .map_err(|err| format!("Failed to replace merged segment: {}", err))?;
    let _ = fs::remove_file(&last_path);
    outputs.pop();
    Ok(())
}

pub fn segment_file(
    input_path: &Path,
    output_dir: &Path,
    segment_seconds: i64,
) -> Result<Vec<PathBuf>, String> {
    segment_file_with_options(input_path, output_dir, segment_seconds, false, false)
}

pub fn segment_file_with_options(
    input_path: &Path,
    output_dir: &Path,
    segment_seconds: i64,
    prefer_precise_copy: bool,
    force_transcode: bool,
) -> Result<Vec<PathBuf>, String> {
    let metadata = fs::metadata(input_path).map_err(|err| {
        format!(
            "segment_input_missing path={} err={}",
            input_path.to_string_lossy(),
            err
        )
    })?;
    if !metadata.is_file() {
        return Err(format!(
            "segment_input_invalid path={} reason=not_file",
            input_path.to_string_lossy()
        ));
    }
    if metadata.len() == 0 {
        return Err(format!(
            "segment_input_invalid path={} reason=empty_file",
            input_path.to_string_lossy()
        ));
    }
    let total_duration = probe_duration_seconds(input_path).map_err(|err| {
        format!(
            "segment_input_unreadable path={} err={}",
            input_path.to_string_lossy(),
            err
        )
    })?;
    fs::create_dir_all(output_dir)
        .map_err(|err| format!("Failed to create segment dir: {}", err))?;

    let mut outputs = Vec::new();
    let mut part_index = 0usize;
    let mut start_seconds = 0.0;
    let segment_length = segment_seconds as f64;
    if !force_transcode {
        if let Ok(mut batch_outputs) =
            segment_file_batch_copy(input_path, output_dir, segment_seconds)
        {
            if !batch_outputs.is_empty() {
                merge_last_short_segment(&mut batch_outputs, 10.0)?;
                return Ok(batch_outputs);
            }
            cleanup_segment_outputs(output_dir)?;
        }
    } else if let Ok(mut batch_outputs) = segment_file_batch_transcode(
        input_path,
        output_dir,
        segment_seconds,
        TranscodeProfile::ClipAndMergeClean,
    ) {
        if !batch_outputs.is_empty() {
            merge_last_short_segment(&mut batch_outputs, 10.0)?;
            return Ok(batch_outputs);
        }
        cleanup_segment_outputs(output_dir)?;
    }
    while start_seconds < total_duration - 0.001 {
        let remaining = total_duration - start_seconds;
        let current_duration = remaining.min(segment_length);
        let output_path = output_dir.join(format!("part_{:03}.mp4", part_index));
        if force_transcode {
            if let Err(err) = segment_single_transcode(
                input_path,
                &output_path,
                start_seconds,
                current_duration,
                TranscodeProfile::ClipAndMergeClean,
            ) {
                let _ = fs::remove_file(&output_path);
                return Err(err);
            }
        } else {
            let mut copy_failures = Vec::new();
            let primary_mode = if prefer_precise_copy {
                SegmentCopyMode::Precise
            } else {
                SegmentCopyMode::Fast
            };
            let secondary_mode = if primary_mode == SegmentCopyMode::Precise {
                SegmentCopyMode::Fast
            } else {
                SegmentCopyMode::Precise
            };
            let copy_modes = [primary_mode, secondary_mode];

            for mode in copy_modes {
                let copy_result = segment_single_copy(
                    mode,
                    input_path,
                    &output_path,
                    start_seconds,
                    current_duration,
                );
                match copy_result {
                    Ok(_) if output_path.exists() => break,
                    Ok(_) => {
                        copy_failures.push(format!(
                            "{}:output_missing",
                            match mode {
                                SegmentCopyMode::Fast => "fast_copy_fail",
                                SegmentCopyMode::Precise => "precise_copy_fail",
                            }
                        ));
                        let _ = fs::remove_file(&output_path);
                    }
                    Err(err) => {
                        copy_failures.push(format!(
                            "{}:{}",
                            match mode {
                                SegmentCopyMode::Fast => "fast_copy_fail",
                                SegmentCopyMode::Precise => "precise_copy_fail",
                            },
                            err
                        ));
                        let _ = fs::remove_file(&output_path);
                    }
                }
            }

            if !output_path.exists() {
                return Err(format!(
                    "segment_copy_only_fail input={} output={} start={} duration={} copy_errs={}",
                    input_path.to_string_lossy(),
                    output_path.to_string_lossy(),
                    format_ffmpeg_time(start_seconds),
                    format_ffmpeg_time(current_duration),
                    copy_failures.join(" | ")
                ));
            }
        }

        outputs.push(output_path);
        part_index += 1;
        start_seconds += current_duration;
    }

    outputs.sort();
    merge_last_short_segment(&mut outputs, 10.0)?;
    Ok(outputs)
}

fn clip_single(
    source: &ClipSource,
    output_path: &Path,
    use_copy: bool,
    profile: Option<&TranscodeProfile>,
) -> Result<(), String> {
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("Failed to create clip output dir: {}", err))?;
    }

    let input_path = Path::new(&source.input_path);
    let original_start_seconds = source.start_time.as_deref().and_then(parse_time_to_seconds);
    let clip_end_seconds = source.end_time.as_deref().and_then(parse_time_to_seconds);
    let aligned_start_seconds = if use_copy {
        match original_start_seconds {
            Some(start_seconds) if start_seconds > 0.0 => {
                probe_previous_keyframe_seconds(input_path, start_seconds)?
            }
            _ => None,
        }
    } else {
        None
    };
    let effective_start_seconds = aligned_start_seconds.or(original_start_seconds);
    let clip_duration_seconds = match (effective_start_seconds, clip_end_seconds) {
        (Some(start_seconds), Some(end_seconds)) if end_seconds > start_seconds => {
            Some(end_seconds - start_seconds)
        }
        (None, Some(end_seconds)) if end_seconds > 0.0 => Some(end_seconds),
        _ => None,
    };

    let mut args = Vec::new();
    if let Some(active_profile) = profile {
        args.push("-fflags".to_string());
        args.push("+genpts".to_string());
        args.push("-i".to_string());
        args.push(source.input_path.clone());

        if let Some(start_seconds) = effective_start_seconds {
            if start_seconds > 0.0 {
                args.push("-ss".to_string());
                args.push(format_ffmpeg_time(start_seconds));
            }
        }

        if let Some(duration_seconds) = clip_duration_seconds {
            args.push("-t".to_string());
            args.push(format_ffmpeg_time(duration_seconds));
        }

        let args_line = args.join(" ");
        run_transcode_with_fallback(&args, output_path, *active_profile, "clip_transcode_fail")
            .map_err(|err| {
                format!(
                    "clip_ffmpeg_fail input={} output={} start_aligned={} duration={} args={} err={}",
                    source.input_path,
                    output_path.to_string_lossy(),
                    aligned_start_seconds
                        .map(format_ffmpeg_time)
                        .unwrap_or_else(|| "-".to_string()),
                    clip_duration_seconds
                        .map(format_ffmpeg_time)
                        .unwrap_or_else(|| "-".to_string()),
                    args_line,
                    err
                )
            })?;
    } else {
        if let Some(start_seconds) = effective_start_seconds {
            if start_seconds > 0.0 {
                args.push("-ss".to_string());
                args.push(format_ffmpeg_time(start_seconds));
            }
        }
        args.push("-i".to_string());
        args.push(source.input_path.clone());

        if let Some(duration_seconds) = clip_duration_seconds {
            args.push("-t".to_string());
            args.push(format_ffmpeg_time(duration_seconds));
        }

        args.extend(["-c".to_string(), "copy".to_string()]);
        args.push(output_path.to_string_lossy().to_string());

        let args_line = args.join(" ");
        run_ffmpeg(&args).map_err(|err| {
            format!(
                "clip_ffmpeg_fail input={} output={} start_aligned={} duration={} args={} err={}",
                source.input_path,
                output_path.to_string_lossy(),
                aligned_start_seconds
                    .map(format_ffmpeg_time)
                    .unwrap_or_else(|| "-".to_string()),
                clip_duration_seconds
                    .map(format_ffmpeg_time)
                    .unwrap_or_else(|| "-".to_string()),
                args_line,
                err
            )
        })?;
    }

    finalize_clip_output(source, output_path)
}

#[cfg(test)]
mod tests {
    use super::{format_ffmpeg_time, last_keyframe_at_or_before};
    use serde_json::json;

    #[test]
    fn format_ffmpeg_time_keeps_millis() {
        assert_eq!(format_ffmpeg_time(2545.833), "00:42:25.833");
        assert_eq!(format_ffmpeg_time(0.0), "00:00:00.000");
    }

    #[test]
    fn last_keyframe_finds_previous_frame_before_start() {
        let payload = json!({
            "frames": [
                { "best_effort_timestamp_time": "2540.833" },
                { "best_effort_timestamp_time": "2545.833" },
                { "best_effort_timestamp_time": "2550.833" }
            ]
        });

        assert_eq!(last_keyframe_at_or_before(&payload, 2544.0), Some(2540.833));
        assert_eq!(
            last_keyframe_at_or_before(&payload, 2545.833),
            Some(2545.833)
        );
        assert_eq!(last_keyframe_at_or_before(&payload, 2539.0), None);
    }
}
