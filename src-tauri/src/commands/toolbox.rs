use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use base64::{engine::general_purpose, Engine as _};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::time::{sleep, Duration};
use uuid::Uuid;

use crate::api::ApiResponse;
use crate::ffmpeg::{run_ffmpeg, run_ffmpeg_with_progress, run_ffprobe_json};
use crate::login_store::AuthInfo;
use crate::utils;
use crate::AppState;

const BILIBILI_ARCHIVE_STATUS_REVIEWING: &str = "is_pubing";
const BILIBILI_ARCHIVE_STATUS_REJECTED: &str = "not_pubed";
const BILIBILI_ARCHIVE_STATUS_PUBLISHED: &str = "pubed";
const VIDEO_MASK_COPY_SEEK_EPSILON: f64 = 0.001;
const VIDEO_MASK_RENDER_PROGRESS_EVENT: &str = "toolbox://video-mask-render-progress";

struct RemoteRefreshPauseGuard {
    counter: Arc<AtomicUsize>,
}

impl RemoteRefreshPauseGuard {
    fn new(counter: Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self { counter }
    }
}

impl Drop for RemoteRefreshPauseGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemuxPayload {
    pub source_path: String,
    pub target_path: String,
}

#[tauri::command]
pub async fn toolbox_remux(
    state: State<'_, AppState>,
    payload: RemuxPayload,
) -> Result<ApiResponse<bool>, String> {
    let source = payload.source_path.trim();
    if source.is_empty() {
        return Ok(ApiResponse::error("请选择源文件"));
    }

    let source_path = Path::new(source);
    if !source_path.exists() {
        return Ok(ApiResponse::error("源文件不存在"));
    }
    if !source_path.is_file() {
        return Ok(ApiResponse::error("源文件不是文件"));
    }

    let target = payload.target_path.trim();
    if target.is_empty() {
        return Ok(ApiResponse::error("请选择输出路径"));
    }

    let target_path = Path::new(target);
    if let Some(parent) = target_path.parent() {
        if let Err(err) = fs::create_dir_all(parent) {
            return Ok(ApiResponse::error(format!("创建输出目录失败: {}", err)));
        }
    }

    let log_path = state.app_log_path.clone();
    utils::append_log(
        log_path.as_ref(),
        &format!("toolbox_remux_start source={} target={}", source, target),
    );

    let args = vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-y".to_string(),
        "-i".to_string(),
        source.to_string(),
        "-c".to_string(),
        "copy".to_string(),
        target.to_string(),
    ];

    let result = tauri::async_runtime::spawn_blocking(move || run_ffmpeg(&args))
        .await
        .map_err(|_| "转封装执行失败".to_string())?;

    match result {
        Ok(()) => {
            utils::append_log(log_path.as_ref(), "toolbox_remux_done status=ok");
            Ok(ApiResponse::success(true))
        }
        Err(err) => {
            utils::append_log(
                log_path.as_ref(),
                &format!("toolbox_remux_done status=err err={}", err),
            );
            Ok(ApiResponse::error(err))
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoProbePayload {
    pub source_path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoProbeResult {
    pub duration: f64,
    pub width: i64,
    pub height: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMaskProbeResult {
    pub duration: f64,
    pub width: i64,
    pub height: i64,
    pub fps: f64,
    pub video_codec: String,
    pub audio_streams: usize,
    pub subtitle_streams: usize,
    pub chapter_count: usize,
    pub color_space: String,
    pub color_transfer: String,
    pub color_primaries: String,
    pub keyframes: Vec<f64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMaskKeyframesResult {
    pub keyframes: Vec<f64>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMaskSegmentPayload {
    pub id: String,
    pub image_path: String,
    pub start_time: f64,
    pub end_time: f64,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub crop_left: f64,
    pub crop_top: f64,
    pub crop_right: f64,
    pub crop_bottom: f64,
    #[serde(default = "default_mask_opacity")]
    pub opacity: f64,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMaskBuildPlanPayload {
    pub source_path: String,
    #[serde(default)]
    pub duration: f64,
    #[serde(default)]
    pub keyframes: Vec<f64>,
    pub segments: Vec<VideoMaskSegmentPayload>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMaskRenderPayload {
    #[serde(default)]
    pub render_id: String,
    pub source_path: String,
    pub target_path: String,
    #[serde(default)]
    pub duration: f64,
    #[serde(default)]
    pub width: i64,
    #[serde(default)]
    pub height: i64,
    #[serde(default)]
    pub fps: f64,
    #[serde(default)]
    pub video_codec: String,
    #[serde(default)]
    pub color_space: String,
    #[serde(default)]
    pub color_transfer: String,
    #[serde(default)]
    pub color_primaries: String,
    pub segments: Vec<VideoMaskSegmentPayload>,
    pub crf: Option<i64>,
    pub preset: Option<String>,
    #[serde(default)]
    pub keyframes: Vec<f64>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMaskRenderPart {
    pub kind: String,
    pub start_time: f64,
    pub end_time: f64,
    pub duration: f64,
    pub segments: Vec<VideoMaskSegmentPayload>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMaskRenderPlan {
    pub parts: Vec<VideoMaskRenderPart>,
    pub encode_duration: f64,
    pub copy_duration: f64,
    pub snap_tolerance: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMaskRenderResult {
    pub output_path: String,
    pub part_count: usize,
    pub encode_duration: f64,
    pub copy_duration: f64,
    pub output_size: u64,
    pub warnings: Vec<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMaskRenderProgress {
    pub render_id: String,
    pub percent: i64,
    pub stage: String,
    pub part_index: usize,
    pub part_count: usize,
    pub stage_percent: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMaskThumbnailPayload {
    pub source_path: String,
    #[serde(default)]
    pub keyframes: Vec<f64>,
    #[serde(default)]
    pub max_count: Option<usize>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMaskPreviewFramePayload {
    pub source_path: String,
    #[serde(default)]
    pub time: f64,
    #[serde(default)]
    pub width: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMaskImageDataUrlPayload {
    pub image_path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMaskThumbnailItem {
    pub time: f64,
    pub path: String,
    pub data_url: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMaskPreviewFrameResult {
    pub time: f64,
    pub path: String,
    pub data_url: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMaskImageDataUrlResult {
    pub data_url: String,
}

#[tauri::command]
pub async fn toolbox_video_probe(
    payload: VideoProbePayload,
) -> Result<ApiResponse<VideoProbeResult>, String> {
    let source = payload.source_path.trim();
    if source.is_empty() {
        return Ok(ApiResponse::error("请选择视频文件"));
    }
    let source_path = Path::new(source);
    if !source_path.is_file() {
        return Ok(ApiResponse::error("视频文件不存在"));
    }

    let args = vec![
        "-v".to_string(),
        "quiet".to_string(),
        "-print_format".to_string(),
        "json".to_string(),
        "-show_format".to_string(),
        "-show_streams".to_string(),
        source.to_string(),
    ];

    let data = tauri::async_runtime::spawn_blocking(move || run_ffprobe_json(&args))
        .await
        .map_err(|_| "视频信息读取失败".to_string())?;

    match data {
        Ok(value) => Ok(ApiResponse::success(parse_video_probe(&value))),
        Err(err) => Ok(ApiResponse::error(err)),
    }
}

#[tauri::command]
pub async fn toolbox_video_mask_probe(
    payload: VideoProbePayload,
) -> Result<ApiResponse<VideoMaskProbeResult>, String> {
    let source = payload.source_path.trim();
    if source.is_empty() {
        return Ok(ApiResponse::error("请选择视频文件"));
    }
    if !Path::new(source).is_file() {
        return Ok(ApiResponse::error("视频文件不存在"));
    }

    let source_path = source.to_string();
    let result =
        tauri::async_runtime::spawn_blocking(move || probe_video_mask_source(&source_path))
            .await
            .map_err(|_| "视频信息读取失败".to_string())?;

    match result {
        Ok(value) => Ok(ApiResponse::success(value)),
        Err(err) => Ok(ApiResponse::error(err)),
    }
}

#[tauri::command]
pub async fn toolbox_video_mask_keyframes(
    payload: VideoProbePayload,
) -> Result<ApiResponse<VideoMaskKeyframesResult>, String> {
    let source = payload.source_path.trim();
    if source.is_empty() {
        return Ok(ApiResponse::error("请选择视频文件"));
    }
    if !Path::new(source).is_file() {
        return Ok(ApiResponse::error("视频文件不存在"));
    }

    let source_path = source.to_string();
    let result = tauri::async_runtime::spawn_blocking(move || probe_keyframes(&source_path))
        .await
        .map_err(|_| "关键帧分析失败".to_string())?;

    match result {
        Ok(keyframes) => Ok(ApiResponse::success(VideoMaskKeyframesResult { keyframes })),
        Err(err) => Ok(ApiResponse::error(err)),
    }
}

#[tauri::command]
pub async fn toolbox_video_mask_thumbnails(
    app: AppHandle,
    payload: VideoMaskThumbnailPayload,
) -> Result<ApiResponse<Vec<VideoMaskThumbnailItem>>, String> {
    let source = payload.source_path.trim();
    if source.is_empty() {
        return Ok(ApiResponse::error("请选择视频文件"));
    }
    if !Path::new(source).is_file() {
        return Ok(ApiResponse::error("视频文件不存在"));
    }

    let output_dir = match video_mask_thumbnail_dir(&app) {
        Ok(path) => path.join(Uuid::new_v4().to_string()),
        Err(err) => return Ok(ApiResponse::error(err)),
    };
    if let Err(err) = fs::create_dir_all(&output_dir) {
        return Ok(ApiResponse::error(format!(
            "创建关键帧缓存目录失败: {}",
            err
        )));
    }

    let source_path = source.to_string();
    let result = tauri::async_runtime::spawn_blocking(move || {
        extract_video_mask_thumbnails(
            &source_path,
            &payload.keyframes,
            &output_dir,
            payload.max_count.unwrap_or(40),
        )
    })
    .await
    .map_err(|_| "关键帧画面生成失败".to_string())?;

    match result {
        Ok(items) => Ok(ApiResponse::success(items)),
        Err(err) => Ok(ApiResponse::error(err)),
    }
}

#[tauri::command]
pub async fn toolbox_video_mask_preview_frame(
    app: AppHandle,
    payload: VideoMaskPreviewFramePayload,
) -> Result<ApiResponse<VideoMaskPreviewFrameResult>, String> {
    let source = payload.source_path.trim();
    if source.is_empty() {
        return Ok(ApiResponse::error("请选择视频文件"));
    }
    if !Path::new(source).is_file() {
        return Ok(ApiResponse::error("视频文件不存在"));
    }

    let output_dir = match video_mask_thumbnail_dir(&app) {
        Ok(path) => path.join("preview"),
        Err(err) => return Ok(ApiResponse::error(err)),
    };
    if let Err(err) = fs::create_dir_all(&output_dir) {
        return Ok(ApiResponse::error(format!(
            "创建预览帧缓存目录失败: {}",
            err
        )));
    }

    let source_path = source.to_string();
    let time = payload.time.max(0.0);
    let width = payload.width.unwrap_or(720).clamp(180, 1280);
    let result = tauri::async_runtime::spawn_blocking(move || {
        extract_video_mask_preview_frame(&source_path, time, width, &output_dir)
    })
    .await
    .map_err(|_| "预览帧生成失败".to_string())?;

    match result {
        Ok(item) => Ok(ApiResponse::success(item)),
        Err(err) => Ok(ApiResponse::error(err)),
    }
}

#[tauri::command]
pub async fn toolbox_video_mask_image_data_url(
    payload: VideoMaskImageDataUrlPayload,
) -> Result<ApiResponse<VideoMaskImageDataUrlResult>, String> {
    let image_path = payload.image_path.trim();
    if image_path.is_empty() {
        return Ok(ApiResponse::error("请选择遮罩图片"));
    }
    if !Path::new(image_path).is_file() {
        return Ok(ApiResponse::error("遮罩图片不存在"));
    }

    let image_path = PathBuf::from(image_path);
    let result = tauri::async_runtime::spawn_blocking(move || image_file_data_url(&image_path))
        .await
        .map_err(|_| "遮罩图片预览生成失败".to_string())?;

    match result {
        Ok(data_url) => Ok(ApiResponse::success(VideoMaskImageDataUrlResult {
            data_url,
        })),
        Err(err) => Ok(ApiResponse::error(err)),
    }
}

#[tauri::command]
pub async fn toolbox_video_mask_build_plan(
    payload: VideoMaskBuildPlanPayload,
) -> Result<ApiResponse<VideoMaskRenderPlan>, String> {
    let source = payload.source_path.trim();
    if source.is_empty() {
        return Ok(ApiResponse::error("请选择视频文件"));
    }
    if !Path::new(source).is_file() {
        return Ok(ApiResponse::error("视频文件不存在"));
    }

    match build_video_mask_render_plan(payload.duration, &payload.keyframes, &payload.segments) {
        Ok(plan) => Ok(ApiResponse::success(plan)),
        Err(err) => Ok(ApiResponse::error(err)),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMaskMergePayload {
    pub source_path: String,
    pub image_path: String,
    pub target_path: String,
    pub start_time: f64,
    pub end_time: f64,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub crop_left: f64,
    pub crop_top: f64,
    pub crop_right: f64,
    pub crop_bottom: f64,
    pub crf: Option<i64>,
    pub preset: Option<String>,
    pub duration: Option<f64>,
}

#[tauri::command]
pub async fn toolbox_video_mask_merge(
    state: State<'_, AppState>,
    payload: VideoMaskMergePayload,
) -> Result<ApiResponse<String>, String> {
    let source = payload.source_path.trim();
    let image = payload.image_path.trim();
    let target = payload.target_path.trim();
    if source.is_empty() || image.is_empty() || target.is_empty() {
        return Ok(ApiResponse::error("请选择视频、遮罩图片和输出路径"));
    }
    if !Path::new(source).is_file() {
        return Ok(ApiResponse::error("视频文件不存在"));
    }
    if !Path::new(image).is_file() {
        return Ok(ApiResponse::error("遮罩图片不存在"));
    }
    if let Some(parent) = Path::new(target).parent() {
        if let Err(err) = fs::create_dir_all(parent) {
            return Ok(ApiResponse::error(format!("创建输出目录失败: {}", err)));
        }
    }

    let start = payload.start_time.max(0.0);
    let mut end = payload.end_time.max(start);
    if end <= start {
        end = payload.duration.unwrap_or(start + 1.0).max(start + 1.0);
    }

    let width = payload.width.max(8.0).round() as i64;
    let height = payload.height.max(8.0).round() as i64;
    let x = payload.x.max(0.0).round() as i64;
    let y = payload.y.max(0.0).round() as i64;
    let crop_left = normalize_crop(payload.crop_left);
    let crop_top = normalize_crop(payload.crop_top);
    let crop_right = normalize_crop(payload.crop_right);
    let crop_bottom = normalize_crop(payload.crop_bottom);
    let crop_w = (1.0 - crop_left - crop_right).max(0.05);
    let crop_h = (1.0 - crop_top - crop_bottom).max(0.05);
    let preset = match payload.preset.as_deref().unwrap_or("veryfast") {
        "ultrafast" | "superfast" | "veryfast" | "faster" | "fast" | "medium" => {
            payload.preset.unwrap_or_else(|| "veryfast".to_string())
        }
        _ => "veryfast".to_string(),
    };
    let crf = payload.crf.unwrap_or(20).clamp(16, 30).to_string();
    let filter = format!(
        "[1:v]format=rgba,crop=iw*{:.6}:ih*{:.6}:iw*{:.6}:ih*{:.6},scale={}:{}[mask];[0:v][mask]overlay={}:{}:enable='between(t,{:.3},{:.3})':shortest=0:repeatlast=1:format=auto[v]",
        crop_w, crop_h, crop_left, crop_top, width, height, x, y, start, end
    );

    let args = vec![
        "-hide_banner".to_string(),
        "-y".to_string(),
        "-i".to_string(),
        source.to_string(),
        "-i".to_string(),
        image.to_string(),
        "-filter_complex".to_string(),
        filter,
        "-map".to_string(),
        "[v]".to_string(),
        "-map".to_string(),
        "0:a?".to_string(),
        "-c:v".to_string(),
        "libx264".to_string(),
        "-preset".to_string(),
        preset,
        "-crf".to_string(),
        crf,
        "-pix_fmt".to_string(),
        "yuv420p".to_string(),
        "-c:a".to_string(),
        "copy".to_string(),
        "-movflags".to_string(),
        "+faststart".to_string(),
        "-progress".to_string(),
        "pipe:1".to_string(),
        target.to_string(),
    ];

    let log_path = state.app_log_path.clone();
    utils::append_log(
        log_path.as_ref(),
        &format!(
            "toolbox_video_mask_start source={} target={}",
            source, target
        ),
    );
    let duration_ms = payload
        .duration
        .map(|value| (value * 1000.0).round() as i64);
    let result = tauri::async_runtime::spawn_blocking(move || {
        run_ffmpeg_with_progress(&args, duration_ms, |_| {})
    })
    .await
    .map_err(|_| "遮罩合成执行失败".to_string())?;

    match result {
        Ok(()) => {
            utils::append_log(log_path.as_ref(), "toolbox_video_mask_done status=ok");
            Ok(ApiResponse::success(target.to_string()))
        }
        Err(err) => {
            utils::append_log(
                log_path.as_ref(),
                &format!("toolbox_video_mask_done status=err err={}", err),
            );
            Ok(ApiResponse::error(err))
        }
    }
}

#[tauri::command]
pub async fn toolbox_video_mask_render(
    app: AppHandle,
    state: State<'_, AppState>,
    mut payload: VideoMaskRenderPayload,
) -> Result<ApiResponse<VideoMaskRenderResult>, String> {
    let source = payload.source_path.trim();
    let target = payload.target_path.trim();
    if source.is_empty() || target.is_empty() {
        return Ok(ApiResponse::error("请选择视频和输出路径"));
    }
    if !Path::new(source).is_file() {
        return Ok(ApiResponse::error("视频文件不存在"));
    }
    for segment in payload.segments.iter().filter(|item| item.enabled) {
        if !Path::new(segment.image_path.trim()).is_file() {
            return Ok(ApiResponse::error(format!(
                "遮罩图片不存在: {}",
                segment.image_path
            )));
        }
    }
    if let Some(parent) = Path::new(target).parent() {
        if let Err(err) = fs::create_dir_all(parent) {
            return Ok(ApiResponse::error(format!("创建输出目录失败: {}", err)));
        }
    }

    let log_path = state.app_log_path.clone();
    utils::append_log(
        log_path.as_ref(),
        &format!(
            "toolbox_video_mask_render_start source={} target={} segments={}",
            source,
            target,
            payload.segments.len()
        ),
    );

    payload.render_id = normalized_render_id(&payload.render_id);
    let progress_app = app.clone();
    let progress_render_id = payload.render_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let emit_render_id = progress_render_id.clone();
        render_video_mask_segments_with_progress(payload, move |progress| {
            if progress.render_id == emit_render_id {
                let _ = progress_app.emit(VIDEO_MASK_RENDER_PROGRESS_EVENT, progress);
            }
        })
    })
    .await
    .map_err(|_| "遮罩导出执行失败".to_string())?;

    match result {
        Ok(value) => {
            utils::append_log(
                log_path.as_ref(),
                &format!(
                    "toolbox_video_mask_render_done status=ok target={} parts={} encode_duration={:.3} copy_duration={:.3}",
                    value.output_path, value.part_count, value.encode_duration, value.copy_duration
                ),
            );
            Ok(ApiResponse::success(value))
        }
        Err(err) => {
            utils::append_log(
                log_path.as_ref(),
                &format!("toolbox_video_mask_render_done status=err err={}", err),
            );
            Ok(ApiResponse::error(err))
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeasonListItem {
    pub season_id: i64,
    pub title: String,
    pub description: String,
    pub cover: String,
    pub section_count: i64,
    pub episode_count: i64,
    pub complete: bool,
    pub state: i64,
    pub mtime: i64,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeasonEpisodeBackup {
    #[serde(default)]
    pub episode_id: Option<i64>,
    pub title: String,
    pub aid: i64,
    pub cid: i64,
    #[serde(default)]
    pub bvid: Option<String>,
    #[serde(default)]
    pub archive_title: Option<String>,
    #[serde(default)]
    pub video_title: Option<String>,
    #[serde(default)]
    pub sort: i64,
    pub charging_pay: i64,
    pub member_first: i64,
    pub limited_free: bool,
    #[serde(default)]
    pub published_at: Option<i64>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeasonSectionBackup {
    pub section_id: i64,
    #[serde(rename = "type")]
    pub section_type: i64,
    pub title: String,
    pub order: i64,
    #[serde(default)]
    pub cover: String,
    #[serde(default)]
    pub state: i64,
    #[serde(default)]
    pub part_state: i64,
    pub ep_count: i64,
    #[serde(default)]
    pub episodes: Vec<SeasonEpisodeBackup>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeasonBackup {
    pub backup_id: String,
    #[serde(default)]
    pub source_season_id: i64,
    pub title: String,
    pub description: String,
    pub cover: String,
    pub season_price: i64,
    #[serde(default)]
    pub no_section: Option<i64>,
    #[serde(default)]
    pub section_id: i64,
    #[serde(default)]
    pub section_count: i64,
    pub episode_count: i64,
    pub captured_episode_count: i64,
    pub complete: bool,
    #[serde(default)]
    pub sections: Vec<SeasonSectionBackup>,
    #[serde(default)]
    pub episodes: Vec<SeasonEpisodeBackup>,
    pub created_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeasonRestoreResult {
    pub new_season_id: i64,
    pub added_episode_count: usize,
    pub created_section_count: usize,
    pub verified: bool,
    pub restored_section_count: usize,
    pub restored_episode_count: usize,
    pub restored_no_section: Option<i64>,
    pub expected_no_section: i64,
    pub episode_sort_mode: String,
    pub verification: Vec<SeasonRestoreVerificationItem>,
    pub warnings: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeasonRestoreVerificationItem {
    pub title: String,
    pub expected_type: i64,
    pub actual_type: i64,
    pub expected_episodes: usize,
    pub actual_episodes: usize,
    pub actual_show: i64,
    pub matched: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeasonBackupPayload {
    pub season_id: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeasonRestorePayload {
    pub backup_id: String,
    #[serde(default)]
    pub episode_sort_mode: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeasonBackupDeletePayload {
    pub backup_id: String,
}

#[tauri::command]
pub async fn toolbox_bilibili_season_list(
    state: State<'_, AppState>,
) -> Result<ApiResponse<Vec<SeasonListItem>>, String> {
    let auth = match load_active_auth(&state) {
        Ok(auth) => auth,
        Err(err) => return Ok(ApiResponse::error(err)),
    };
    match fetch_all_seasons(&state, &auth).await {
        Ok(items) => Ok(ApiResponse::success(
            items.iter().map(build_season_list_item).collect(),
        )),
        Err(err) => Ok(ApiResponse::error(err)),
    }
}

#[tauri::command]
pub fn toolbox_bilibili_season_backups(app: AppHandle) -> ApiResponse<Vec<SeasonBackup>> {
    match read_season_backups(&app) {
        Ok(items) => ApiResponse::success(items),
        Err(err) => ApiResponse::error(err),
    }
}

#[tauri::command]
pub fn toolbox_bilibili_season_backup_delete(
    app: AppHandle,
    payload: SeasonBackupDeletePayload,
) -> ApiResponse<bool> {
    let mut backups = match read_season_backups(&app) {
        Ok(items) => items,
        Err(err) => return ApiResponse::error(err),
    };
    let before = backups.len();
    backups.retain(|item| item.backup_id != payload.backup_id);
    if backups.len() == before {
        return ApiResponse::error("未找到备份");
    }
    match write_season_backups(&app, &backups) {
        Ok(()) => ApiResponse::success(true),
        Err(err) => ApiResponse::error(err),
    }
}

#[tauri::command]
pub async fn toolbox_bilibili_season_backup(
    app: AppHandle,
    state: State<'_, AppState>,
    payload: SeasonBackupPayload,
) -> Result<ApiResponse<SeasonBackup>, String> {
    let auth = match load_active_auth(&state) {
        Ok(auth) => auth,
        Err(err) => return Ok(ApiResponse::error(err)),
    };
    let detail = match fetch_season_detail(&state, &auth, payload.season_id).await {
        Ok(item) => item,
        Err(err) => return Ok(ApiResponse::error(err)),
    };
    let backup = match build_season_backup(&state, &auth, &detail).await {
        Ok(item) => item,
        Err(err) => return Ok(ApiResponse::error(err)),
    };
    let mut backups = match read_season_backups(&app) {
        Ok(items) => items,
        Err(err) => return Ok(ApiResponse::error(err)),
    };
    backups.retain(|item| item.source_season_id != backup.source_season_id);
    backups.insert(0, backup.clone());
    if let Err(err) = write_season_backups(&app, &backups) {
        return Ok(ApiResponse::error(err));
    }
    Ok(ApiResponse::success(backup))
}

#[tauri::command]
pub async fn toolbox_bilibili_season_restore(
    app: AppHandle,
    state: State<'_, AppState>,
    payload: SeasonRestorePayload,
) -> Result<ApiResponse<SeasonRestoreResult>, String> {
    let auth = match load_active_auth(&state) {
        Ok(auth) => auth,
        Err(err) => return Ok(ApiResponse::error(err)),
    };
    let csrf = match auth.csrf.clone() {
        Some(value) if !value.trim().is_empty() => value,
        _ => return Ok(ApiResponse::error("登录信息缺少CSRF")),
    };
    let backups = match read_season_backups(&app) {
        Ok(items) => items,
        Err(err) => return Ok(ApiResponse::error(err)),
    };
    let Some(backup) = backups
        .into_iter()
        .find(|item| item.backup_id == payload.backup_id)
    else {
        return Ok(ApiResponse::error("未找到备份"));
    };
    if !backup.complete {
        return Ok(ApiResponse::error("该备份不完整，请重新备份后再恢复"));
    }
    let episode_sort_mode = normalize_episode_sort_mode(payload.episode_sort_mode.as_deref());
    let _remote_refresh_pause_guard =
        RemoteRefreshPauseGuard::new(Arc::clone(&state.submission_remote_refresh_pause_count));

    append_toolbox_log(
        &state,
        &format!(
            "toolbox_bilibili_season_restore_start backup_id={} source_season_id={} title={} sections={} episodes={} captured={} no_section={:?} episode_sort_mode={}",
            backup.backup_id,
            backup.source_season_id,
            backup.title,
            backup.section_count,
            backup.episode_count,
            backup.captured_episode_count,
            backup.no_section,
            episode_sort_mode
        ),
    );

    match restore_season(&state, &auth, &csrf, &backup, &episode_sort_mode).await {
        Ok(result) => {
            append_toolbox_log(
                &state,
                &format!(
                    "toolbox_bilibili_season_restore_done new_season_id={} created_sections={} added_episodes={} verified={} restored_sections={} restored_episodes={} restored_no_section={:?} expected_no_section={} warnings={}",
                    result.new_season_id,
                    result.created_section_count,
                    result.added_episode_count,
                    result.verified,
                    result.restored_section_count,
                    result.restored_episode_count,
                    result.restored_no_section,
                    result.expected_no_section,
                    result.warnings.len()
                ),
            );
            for item in &result.verification {
                append_toolbox_log(
                    &state,
                    &format!(
                        "toolbox_bilibili_season_restore_verify title={} expected_type={} actual_type={} expected_episodes={} actual_episodes={} actual_show={} matched={}",
                        item.title,
                        item.expected_type,
                        item.actual_type,
                        item.expected_episodes,
                        item.actual_episodes,
                        item.actual_show,
                        item.matched
                    ),
                );
            }
            for warning in &result.warnings {
                append_toolbox_log(
                    &state,
                    &format!("toolbox_bilibili_season_restore_warning {}", warning),
                );
            }
            Ok(ApiResponse::success(result))
        }
        Err(err) => {
            append_toolbox_log(
                &state,
                &format!("toolbox_bilibili_season_restore_error {}", err),
            );
            Ok(ApiResponse::error(err))
        }
    }
}

fn parse_video_probe(value: &Value) -> VideoProbeResult {
    let mut width = 0;
    let mut height = 0;
    let mut stream_duration = 0.0;
    if let Some(streams) = value.get("streams").and_then(|item| item.as_array()) {
        for stream in streams {
            if stream.get("codec_type").and_then(|item| item.as_str()) == Some("video") {
                width = stream
                    .get("width")
                    .and_then(|item| item.as_i64())
                    .unwrap_or(0);
                height = stream
                    .get("height")
                    .and_then(|item| item.as_i64())
                    .unwrap_or(0);
                stream_duration = stream
                    .get("duration")
                    .and_then(|item| item.as_str())
                    .and_then(|item| item.parse::<f64>().ok())
                    .unwrap_or(0.0);
                break;
            }
        }
    }
    let duration = value
        .get("format")
        .and_then(|item| item.get("duration"))
        .and_then(|item| item.as_str())
        .and_then(|item| item.parse::<f64>().ok())
        .unwrap_or(stream_duration);
    VideoProbeResult {
        duration,
        width,
        height,
    }
}

fn probe_video_mask_source(source: &str) -> Result<VideoMaskProbeResult, String> {
    let args = vec![
        "-v".to_string(),
        "quiet".to_string(),
        "-print_format".to_string(),
        "json".to_string(),
        "-show_format".to_string(),
        "-show_streams".to_string(),
        "-show_chapters".to_string(),
        source.to_string(),
    ];
    let value = run_ffprobe_json(&args)?;
    let base = parse_video_probe(&value);
    let mut fps = 0.0;
    let mut video_codec = String::new();
    let mut audio_streams = 0usize;
    let mut subtitle_streams = 0usize;
    let mut color_space = String::new();
    let mut color_transfer = String::new();
    let mut color_primaries = String::new();

    if let Some(streams) = value.get("streams").and_then(|item| item.as_array()) {
        for stream in streams {
            match stream.get("codec_type").and_then(|item| item.as_str()) {
                Some("video") if video_codec.is_empty() => {
                    video_codec = value_string(stream, "codec_name");
                    fps = parse_fps_value(
                        stream
                            .get("avg_frame_rate")
                            .and_then(|item| item.as_str())
                            .or_else(|| stream.get("r_frame_rate").and_then(|item| item.as_str())),
                    );
                    color_space = optional_value_string(stream, "color_space").unwrap_or_default();
                    color_transfer =
                        optional_value_string(stream, "color_transfer").unwrap_or_default();
                    color_primaries =
                        optional_value_string(stream, "color_primaries").unwrap_or_default();
                }
                Some("audio") => audio_streams += 1,
                Some("subtitle") => subtitle_streams += 1,
                _ => {}
            }
        }
    }

    let chapter_count = value
        .get("chapters")
        .and_then(|item| item.as_array())
        .map(|items| items.len())
        .unwrap_or(0);

    Ok(VideoMaskProbeResult {
        duration: base.duration,
        width: base.width,
        height: base.height,
        fps,
        video_codec,
        audio_streams,
        subtitle_streams,
        chapter_count,
        color_space,
        color_transfer,
        color_primaries,
        keyframes: Vec::new(),
    })
}

/// 读取源视频第一路视频流的 timescale（time_base 分母，如 16000）。
/// 失败或无法解析时返回 0，调用方据此跳过 `-video_track_timescale` 设置。
fn probe_video_timescale(source: &str) -> i64 {
    let args = vec![
        "-v".to_string(),
        "quiet".to_string(),
        "-select_streams".to_string(),
        "v:0".to_string(),
        "-show_entries".to_string(),
        "stream=time_base".to_string(),
        "-print_format".to_string(),
        "json".to_string(),
        source.to_string(),
    ];
    let Ok(value) = run_ffprobe_json(&args) else {
        return 0;
    };
    value
        .get("streams")
        .and_then(|item| item.as_array())
        .and_then(|items| items.first())
        .and_then(|stream| stream.get("time_base"))
        .and_then(|item| item.as_str())
        .and_then(|tb| tb.split_once('/'))
        .and_then(|(_, den)| den.trim().parse::<i64>().ok())
        .filter(|den| *den > 0)
        .unwrap_or(0)
}

fn probe_video_reorder_delay_frames(source: &str) -> i64 {
    let args = vec![
        "-v".to_string(),
        "quiet".to_string(),
        "-select_streams".to_string(),
        "v:0".to_string(),
        "-show_entries".to_string(),
        "stream=has_b_frames".to_string(),
        "-print_format".to_string(),
        "json".to_string(),
        source.to_string(),
    ];
    let Ok(value) = run_ffprobe_json(&args) else {
        return 0;
    };
    value
        .get("streams")
        .and_then(|item| item.as_array())
        .and_then(|items| items.first())
        .and_then(|stream| stream.get("has_b_frames"))
        .and_then(|item| item.as_i64())
        .filter(|frames| *frames > 0)
        .unwrap_or(0)
}

fn probe_keyframes(source: &str) -> Result<Vec<f64>, String> {
    let args = vec![
        "-v".to_string(),
        "quiet".to_string(),
        "-skip_frame".to_string(),
        "nokey".to_string(),
        "-select_streams".to_string(),
        "v:0".to_string(),
        "-show_frames".to_string(),
        "-show_entries".to_string(),
        "frame=best_effort_timestamp_time,pkt_pts_time".to_string(),
        "-of".to_string(),
        "json".to_string(),
        source.to_string(),
    ];
    let value = run_ffprobe_json(&args)?;
    let mut keyframes = value
        .get("frames")
        .and_then(|item| item.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|frame| {
                    frame
                        .get("best_effort_timestamp_time")
                        .or_else(|| frame.get("pkt_pts_time"))
                        .and_then(|item| item.as_str())
                        .and_then(|item| item.parse::<f64>().ok())
                })
                .filter(|item| item.is_finite() && *item >= 0.0)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    keyframes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    keyframes.dedup_by(|a, b| (*a - *b).abs() < 0.001);
    if !keyframes.iter().any(|item| *item <= 0.001) {
        keyframes.insert(0, 0.0);
    }
    Ok(keyframes)
}

fn parse_fps_value(value: Option<&str>) -> f64 {
    let Some(value) = value else {
        return 0.0;
    };
    if let Some((num, den)) = value.split_once('/') {
        let numerator = num.parse::<f64>().unwrap_or(0.0);
        let denominator = den.parse::<f64>().unwrap_or(0.0);
        if denominator > 0.0 {
            return numerator / denominator;
        }
        return 0.0;
    }
    value.parse::<f64>().unwrap_or(0.0)
}

fn extract_video_mask_thumbnails(
    source: &str,
    keyframes: &[f64],
    output_dir: &Path,
    max_count: usize,
) -> Result<Vec<VideoMaskThumbnailItem>, String> {
    let max_count = max_count.clamp(1, 80);
    let times = sampled_keyframe_times(keyframes, max_count);
    if times.is_empty() {
        return Ok(Vec::new());
    }

    let mut items = Vec::new();
    for (index, time) in times.iter().enumerate() {
        let output = output_dir.join(format!("keyframe_{:04}.jpg", index));
        let args = vec![
            "-hide_banner".to_string(),
            "-loglevel".to_string(),
            "error".to_string(),
            "-y".to_string(),
            "-ss".to_string(),
            format_seconds(*time),
            "-i".to_string(),
            source.to_string(),
            "-frames:v".to_string(),
            "1".to_string(),
            "-vf".to_string(),
            "scale=180:-2".to_string(),
            "-q:v".to_string(),
            "4".to_string(),
            output.to_string_lossy().into_owned(),
        ];
        run_ffmpeg(&args)?;
        if output.is_file() {
            items.push(VideoMaskThumbnailItem {
                time: *time,
                path: output.to_string_lossy().into_owned(),
                data_url: image_data_url(&output)?,
            });
        }
    }
    Ok(items)
}

fn extract_video_mask_preview_frame(
    source: &str,
    time: f64,
    width: i64,
    output_dir: &Path,
) -> Result<VideoMaskPreviewFrameResult, String> {
    let output = output_dir.join(format!(
        "preview_{:013}.jpg",
        (time * 1000.0).round() as i64
    ));
    let args = vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-y".to_string(),
        "-ss".to_string(),
        format_seconds(time),
        "-i".to_string(),
        source.to_string(),
        "-frames:v".to_string(),
        "1".to_string(),
        "-vf".to_string(),
        format!("scale={}:-2", width),
        "-q:v".to_string(),
        "3".to_string(),
        output.to_string_lossy().into_owned(),
    ];
    run_ffmpeg(&args)?;
    if !output.is_file() {
        return Err("预览帧生成失败".to_string());
    }
    Ok(VideoMaskPreviewFrameResult {
        time,
        path: output.to_string_lossy().into_owned(),
        data_url: image_data_url(&output)?,
    })
}

fn image_data_url(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|err| format!("读取图片失败: {}", err))?;
    Ok(format!(
        "data:image/jpeg;base64,{}",
        general_purpose::STANDARD.encode(bytes)
    ))
}

fn image_file_data_url(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|err| format!("读取遮罩图片失败: {}", err))?;
    let mime = match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    };
    Ok(format!(
        "data:{};base64,{}",
        mime,
        general_purpose::STANDARD.encode(bytes)
    ))
}

fn sampled_keyframe_times(keyframes: &[f64], max_count: usize) -> Vec<f64> {
    let mut times = keyframes
        .iter()
        .copied()
        .filter(|item| item.is_finite() && *item >= 0.0)
        .collect::<Vec<_>>();
    times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    times.dedup_by(|a, b| (*a - *b).abs() < 0.001);
    if times.len() <= max_count {
        return times;
    }

    let last_index = times.len() - 1;
    (0..max_count)
        .map(|index| {
            let ratio = if max_count <= 1 {
                0.0
            } else {
                index as f64 / (max_count - 1) as f64
            };
            let source_index = (ratio * last_index as f64).round() as usize;
            times[source_index.min(last_index)]
        })
        .collect()
}

fn normalize_crop(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 0.45)
    } else {
        0.0
    }
}

fn default_true() -> bool {
    true
}

fn default_mask_opacity() -> f64 {
    1.0
}

fn build_video_mask_render_plan(
    duration: f64,
    keyframes: &[f64],
    segments: &[VideoMaskSegmentPayload],
) -> Result<VideoMaskRenderPlan, String> {
    let duration = duration.max(0.0);
    let snap_tolerance = 2.0;
    let mut masks = normalized_mask_segments(duration, segments);
    if masks.is_empty() {
        return Ok(VideoMaskRenderPlan {
            parts: if duration > 0.0 {
                vec![VideoMaskRenderPart {
                    kind: "copy".to_string(),
                    start_time: 0.0,
                    end_time: duration,
                    duration,
                    segments: Vec::new(),
                }]
            } else {
                Vec::new()
            },
            encode_duration: 0.0,
            copy_duration: duration,
            snap_tolerance,
        });
    }
    masks.sort_by(|a, b| {
        a.start_time
            .partial_cmp(&b.start_time)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut encode_windows: Vec<(f64, f64)> = Vec::new();
    for segment in &masks {
        let start = snap_start_to_keyframe(segment.start_time, keyframes, snap_tolerance);
        let end = snap_end_to_keyframe(segment.end_time, keyframes, duration, snap_tolerance);
        if end <= start {
            return Err("存在无效遮罩区间".to_string());
        }
        if let Some(last) = encode_windows.last_mut() {
            if start <= last.1 + 0.001 {
                last.1 = last.1.max(end);
                continue;
            }
        }
        encode_windows.push((start, end));
    }

    let mut parts = Vec::new();
    let mut cursor = 0.0;
    for (start, end) in encode_windows {
        if start > cursor + 0.001 {
            parts.push(VideoMaskRenderPart {
                kind: "copy".to_string(),
                start_time: cursor,
                end_time: start,
                duration: start - cursor,
                segments: Vec::new(),
            });
        }
        let part_segments = masks
            .iter()
            .filter(|segment| segment.start_time < end && segment.end_time > start)
            .cloned()
            .collect::<Vec<_>>();
        parts.push(VideoMaskRenderPart {
            kind: "encode".to_string(),
            start_time: start,
            end_time: end,
            duration: end - start,
            segments: part_segments,
        });
        cursor = end;
    }
    if duration > cursor + 0.001 {
        parts.push(VideoMaskRenderPart {
            kind: "copy".to_string(),
            start_time: cursor,
            end_time: duration,
            duration: duration - cursor,
            segments: Vec::new(),
        });
    }

    let encode_duration = parts
        .iter()
        .filter(|part| part.kind == "encode")
        .map(|part| part.duration)
        .sum::<f64>();
    let copy_duration = parts
        .iter()
        .filter(|part| part.kind == "copy")
        .map(|part| part.duration)
        .sum::<f64>();

    Ok(VideoMaskRenderPlan {
        parts,
        encode_duration,
        copy_duration,
        snap_tolerance,
    })
}

fn normalized_mask_segments(
    duration: f64,
    segments: &[VideoMaskSegmentPayload],
) -> Vec<VideoMaskSegmentPayload> {
    segments
        .iter()
        .filter(|item| item.enabled)
        .filter_map(|item| {
            let start_time = item.start_time.max(0.0).min(duration.max(item.start_time));
            let end_limit = if duration > 0.0 {
                duration
            } else {
                item.end_time
            };
            let end_time = item.end_time.max(start_time).min(end_limit.max(start_time));
            if end_time <= start_time + 0.001 {
                return None;
            }
            let mut next = item.clone();
            next.start_time = start_time;
            next.end_time = end_time;
            next.width = next.width.max(8.0);
            next.height = next.height.max(8.0);
            next.x = next.x.max(0.0);
            next.y = next.y.max(0.0);
            next.crop_left = normalize_crop(next.crop_left);
            next.crop_top = normalize_crop(next.crop_top);
            next.crop_right = normalize_crop(next.crop_right);
            next.crop_bottom = normalize_crop(next.crop_bottom);
            next.opacity = if next.opacity.is_finite() {
                next.opacity.clamp(0.0, 1.0)
            } else {
                1.0
            };
            Some(next)
        })
        .collect()
}

fn snap_start_to_keyframe(value: f64, keyframes: &[f64], tolerance: f64) -> f64 {
    let _ = tolerance;
    keyframes
        .iter()
        .copied()
        .filter(|item| *item <= value)
        .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .unwrap_or(value)
        .max(0.0)
}

fn snap_end_to_keyframe(value: f64, keyframes: &[f64], duration: f64, tolerance: f64) -> f64 {
    let _ = tolerance;
    keyframes
        .iter()
        .copied()
        .filter(|item| *item >= value)
        .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .unwrap_or(if duration > 0.0 { duration } else { value })
        .min(if duration > 0.0 { duration } else { value })
}

fn render_video_mask_segments(
    payload: VideoMaskRenderPayload,
) -> Result<VideoMaskRenderResult, String> {
    render_video_mask_segments_with_progress(payload, |_| {})
}

fn render_video_mask_segments_with_progress<F>(
    mut payload: VideoMaskRenderPayload,
    mut on_progress: F,
) -> Result<VideoMaskRenderResult, String>
where
    F: FnMut(VideoMaskRenderProgress),
{
    payload.render_id = normalized_render_id(&payload.render_id);
    let mut warnings = Vec::new();
    let mut duration = payload.duration;
    let mut width = payload.width;
    let mut height = payload.height;
    let mut fps = payload.fps;
    let mut video_codec = payload.video_codec.clone();
    let mut color_space = payload.color_space.clone();
    let mut color_transfer = payload.color_transfer.clone();
    let mut color_primaries = payload.color_primaries.clone();

    if duration <= 0.0 || width <= 0 || height <= 0 || video_codec.is_empty() {
        let probe = probe_video_mask_source(&payload.source_path)?;
        duration = if duration > 0.0 {
            duration
        } else {
            probe.duration
        };
        width = if width > 0 { width } else { probe.width };
        height = if height > 0 { height } else { probe.height };
        fps = if fps > 0.0 { fps } else { probe.fps };
        if video_codec.is_empty() {
            video_codec = probe.video_codec;
        }
        if color_space.is_empty() {
            color_space = probe.color_space;
        }
        if color_transfer.is_empty() {
            color_transfer = probe.color_transfer;
        }
        if color_primaries.is_empty() {
            color_primaries = probe.color_primaries;
        }
    }

    let keyframes = if payload.keyframes.is_empty() {
        probe_keyframes(&payload.source_path).unwrap_or_default()
    } else {
        payload.keyframes.clone()
    };
    let plan = build_video_mask_render_plan(duration, &keyframes, &payload.segments)?;
    if plan.parts.is_empty() {
        return Err("没有可导出的时间段".to_string());
    }
    emit_video_mask_render_progress(
        &payload.render_id,
        0,
        "准备导出",
        0,
        plan.parts.len(),
        0,
        &mut on_progress,
    );

    let target_path = Path::new(&payload.target_path);
    let temp_parent = target_path
        .parent()
        .map(|item| item.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let temp_dir = temp_parent.join(format!("bili_mask_{}", Uuid::new_v4()));
    fs::create_dir_all(&temp_dir).map_err(|err| format!("创建临时目录失败: {}", err))?;

    // 源视频流的 timescale（时间基分母），用于强制重编码分段与直拷贝分段的时间基一致，
    // 避免直拷贝 concat 拼接时因时间基不一致产生 Non-monotonic DTS 导致视频中途截断。
    let video_timescale = probe_video_timescale(&payload.source_path);
    let copy_reorder_delay_frames = probe_video_reorder_delay_frames(&payload.source_path);

    let result = render_video_mask_parts(
        &payload,
        &plan,
        &temp_dir,
        width,
        height,
        fps,
        &video_codec,
        &color_space,
        &color_transfer,
        &color_primaries,
        video_timescale,
        copy_reorder_delay_frames,
        &mut warnings,
        &mut on_progress,
    )
    .and_then(|part_paths| {
        concat_video_mask_parts(&payload, &plan, &temp_dir, &part_paths, &mut on_progress)
    });

    let _ = fs::remove_dir_all(&temp_dir);

    result.map(|output_size| VideoMaskRenderResult {
        output_path: payload.target_path,
        part_count: plan.parts.len(),
        encode_duration: plan.encode_duration,
        copy_duration: plan.copy_duration,
        output_size,
        warnings,
    })
}

fn render_video_mask_parts<F>(
    payload: &VideoMaskRenderPayload,
    plan: &VideoMaskRenderPlan,
    temp_dir: &Path,
    width: i64,
    height: i64,
    fps: f64,
    video_codec: &str,
    color_space: &str,
    color_transfer: &str,
    color_primaries: &str,
    video_timescale: i64,
    copy_reorder_delay_frames: i64,
    warnings: &mut Vec<String>,
    on_progress: &mut F,
) -> Result<Vec<PathBuf>, String>
where
    F: FnMut(VideoMaskRenderProgress),
{
    let mut part_paths = Vec::new();
    let mut completed_duration = 0.0;
    let total_duration = plan
        .parts
        .iter()
        .map(|part| part.duration.max(0.0))
        .sum::<f64>()
        .max(0.001);
    for (index, part) in plan.parts.iter().enumerate() {
        let part_path = temp_dir.join(format!("part_{:04}.mp4", index));
        emit_video_mask_render_progress(
            &payload.render_id,
            video_mask_part_progress_percent(completed_duration, part.duration, 0, total_duration),
            if part.kind == "copy" {
                "直拷贝分段"
            } else {
                "重编码遮罩分段"
            },
            index + 1,
            plan.parts.len(),
            0,
            on_progress,
        );
        if part.kind == "copy" {
            let args = ffmpeg_args_with_progress(copy_part_args_with_timing(
                &payload.source_path,
                &part_path,
                part.start_time,
                part.duration,
                fps,
                copy_reorder_delay_frames,
                plan.parts.len() > 1,
            ));
            let duration_ms = duration_to_millis(part.duration);
            run_ffmpeg_with_progress(&args, duration_ms, |stage_percent| {
                emit_video_mask_render_progress(
                    &payload.render_id,
                    video_mask_part_progress_percent(
                        completed_duration,
                        part.duration,
                        stage_percent,
                        total_duration,
                    ),
                    "直拷贝分段",
                    index + 1,
                    plan.parts.len(),
                    stage_percent,
                    on_progress,
                );
            })?;
        } else {
            let args = ffmpeg_args_with_progress(encode_part_args(
                payload,
                part,
                &part_path,
                width,
                height,
                fps,
                video_codec,
                color_space,
                color_transfer,
                color_primaries,
                video_timescale,
            ));
            let duration_ms = duration_to_millis(part.duration);
            run_ffmpeg_with_progress(&args, duration_ms, |stage_percent| {
                emit_video_mask_render_progress(
                    &payload.render_id,
                    video_mask_part_progress_percent(
                        completed_duration,
                        part.duration,
                        stage_percent,
                        total_duration,
                    ),
                    "重编码遮罩分段",
                    index + 1,
                    plan.parts.len(),
                    stage_percent,
                    on_progress,
                );
            })?;
        }
        if !part_path.is_file() {
            return Err(format!("分段导出失败: {}", part_path.display()));
        }
        part_paths.push(part_path);
        completed_duration += part.duration.max(0.0);
        emit_video_mask_render_progress(
            &payload.render_id,
            video_mask_part_progress_percent(completed_duration, part.duration, 0, total_duration),
            if part.kind == "copy" {
                "直拷贝分段"
            } else {
                "重编码遮罩分段"
            },
            index + 1,
            plan.parts.len(),
            100,
            on_progress,
        );
    }
    if plan.encode_duration <= 0.0 {
        warnings.push("未配置遮罩区间，已按直拷贝计划导出".to_string());
    }
    Ok(part_paths)
}

fn copy_part_args(source: &str, output: &Path, start: f64, duration: f64) -> Vec<String> {
    copy_part_args_with_timing(source, output, start, duration, 0.0, 0, false)
}

fn copy_part_args_with_timing(
    source: &str,
    output: &Path,
    start: f64,
    duration: f64,
    fps: f64,
    reorder_delay_frames: i64,
    trim_for_concat: bool,
) -> Vec<String> {
    let seek_start = if start > 0.0 {
        start + VIDEO_MASK_COPY_SEEK_EPSILON
    } else {
        0.0
    };
    let copy_duration =
        compensated_copy_duration(duration, fps, reorder_delay_frames, trim_for_concat);
    vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-y".to_string(),
        "-ss".to_string(),
        format_seconds(seek_start),
        "-i".to_string(),
        source.to_string(),
        "-t".to_string(),
        format_seconds(copy_duration),
        "-map".to_string(),
        "0".to_string(),
        "-c".to_string(),
        "copy".to_string(),
        "-reset_timestamps".to_string(),
        "1".to_string(),
        output.to_string_lossy().into_owned(),
    ]
}

fn compensated_copy_duration(
    duration: f64,
    fps: f64,
    reorder_delay_frames: i64,
    trim_for_concat: bool,
) -> f64 {
    if !trim_for_concat
        || !duration.is_finite()
        || duration <= 0.0
        || fps <= 0.0
        || reorder_delay_frames <= 0
    {
        return duration.max(0.0);
    }
    let frame_duration = 1.0 / fps;
    let compensation = frame_duration * reorder_delay_frames as f64;
    (duration - compensation).max(frame_duration)
}

fn encode_part_args(
    payload: &VideoMaskRenderPayload,
    part: &VideoMaskRenderPart,
    output: &Path,
    width: i64,
    height: i64,
    fps: f64,
    video_codec: &str,
    color_space: &str,
    color_transfer: &str,
    color_primaries: &str,
    video_timescale: i64,
) -> Vec<String> {
    let preset = normalized_preset(payload.preset.as_deref());
    let crf = payload.crf.unwrap_or(18).clamp(16, 30).to_string();
    let encoder = match video_codec {
        "hevc" | "h265" => "libx265",
        _ => "libx264",
    };
    let mut args = vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-y".to_string(),
        "-ss".to_string(),
        format_seconds(part.start_time),
        "-i".to_string(),
        payload.source_path.clone(),
    ];
    for segment in &part.segments {
        args.push("-i".to_string());
        args.push(segment.image_path.clone());
    }
    args.push("-filter_complex".to_string());
    args.push(build_overlay_filter(part, width, height));
    args.extend([
        "-map".to_string(),
        "[vout]".to_string(),
        "-map".to_string(),
        "0:a?".to_string(),
        "-map".to_string(),
        "0:s?".to_string(),
        "-t".to_string(),
        format_seconds(part.duration),
        "-c:v".to_string(),
        encoder.to_string(),
        "-preset".to_string(),
        preset,
        "-crf".to_string(),
        crf,
        "-pix_fmt".to_string(),
        "yuv420p".to_string(),
        "-c:a".to_string(),
        "aac".to_string(),
        "-b:a".to_string(),
        "192k".to_string(),
        "-c:s".to_string(),
        "copy".to_string(),
    ]);
    if fps > 0.0 {
        args.push("-r".to_string());
        args.push(format_fps(fps));
    }
    append_color_arg(&mut args, "-colorspace", color_space);
    append_color_arg(&mut args, "-color_trc", color_transfer);
    append_color_arg(&mut args, "-color_primaries", color_primaries);
    // 让重编码分段的 mp4 时间基（timescale）与直拷贝分段所继承的源时间基保持一致。
    // 否则 libx264 会自选 timescale（如 60fps 下为 1/15360），与源的 1/16000 不一致；
    // 直拷贝 concat 拼接时会产生 Non-monotonic DTS，遇到较大回跳时强制后的 DTS 超过 PTS，
    // mp4 muxer 会丢弃该分段之后的全部视频包 —— 表现为视频中途截断、音频却完整。
    if video_timescale > 0 {
        args.push("-video_track_timescale".to_string());
        args.push(video_timescale.to_string());
    }
    args.extend([
        "-map_metadata".to_string(),
        "0".to_string(),
        "-avoid_negative_ts".to_string(),
        "make_zero".to_string(),
        "-movflags".to_string(),
        "+faststart".to_string(),
        output.to_string_lossy().into_owned(),
    ]);
    args
}

fn build_overlay_filter(part: &VideoMaskRenderPart, width: i64, height: i64) -> String {
    let mut filters = vec![format!(
        "[0:v]trim=start=0:duration={:.6},setpts=PTS-STARTPTS[vbase]",
        part.duration
    )];
    let mut current = "[vbase]".to_string();
    for (index, segment) in part.segments.iter().enumerate() {
        let input_index = index + 1;
        let mask_label = format!("[mask{}]", index);
        let out_label = if index + 1 == part.segments.len() {
            "[vout]".to_string()
        } else {
            format!("[v{}]", index)
        };
        let crop_left = normalize_crop(segment.crop_left);
        let crop_top = normalize_crop(segment.crop_top);
        let crop_right = normalize_crop(segment.crop_right);
        let crop_bottom = normalize_crop(segment.crop_bottom);
        let crop_w = (1.0 - crop_left - crop_right).max(0.05);
        let crop_h = (1.0 - crop_top - crop_bottom).max(0.05);
        let mask_width = segment.width.max(8.0).min(width.max(8) as f64).round() as i64;
        let mask_height = segment.height.max(8.0).min(height.max(8) as f64).round() as i64;
        let opacity = if segment.opacity.is_finite() {
            segment.opacity.clamp(0.0, 1.0)
        } else {
            1.0
        };
        filters.push(format!(
            "[{}:v]setpts=PTS-STARTPTS,format=rgba,colorchannelmixer=aa={:.4},crop=iw*{:.6}:ih*{:.6}:iw*{:.6}:ih*{:.6},scale={}:{},tpad=stop_mode=clone:stop_duration={:.6}{}",
            input_index,
            opacity,
            crop_w,
            crop_h,
            crop_left,
            crop_top,
            mask_width,
            mask_height,
            part.duration,
            mask_label
        ));
        let enable_start = (segment.start_time - part.start_time).max(0.0);
        let enable_end = (segment.end_time - part.start_time)
            .min(part.duration)
            .max(enable_start);
        filters.push(format!(
            "{}{}overlay={}:{}:enable='between(t,{:.3},{:.3})':shortest=0:repeatlast=1:format=auto{}",
            current,
            mask_label,
            segment.x.max(0.0).round() as i64,
            segment.y.max(0.0).round() as i64,
            enable_start,
            enable_end,
            out_label
        ));
        current = out_label;
    }
    if filters.is_empty() {
        "[0:v]trim=start=0,setpts=PTS-STARTPTS[vout]".to_string()
    } else {
        filters.join(";")
    }
}

fn concat_video_mask_parts<F>(
    payload: &VideoMaskRenderPayload,
    plan: &VideoMaskRenderPlan,
    temp_dir: &Path,
    part_paths: &[PathBuf],
    on_progress: &mut F,
) -> Result<u64, String>
where
    F: FnMut(VideoMaskRenderProgress),
{
    if plan.parts.len() == 1 && plan.parts[0].kind == "copy" {
        emit_video_mask_render_progress(
            &payload.render_id,
            95,
            "写入输出",
            1,
            plan.parts.len(),
            0,
            on_progress,
        );
        let args = ffmpeg_args_with_progress(copy_part_args(
            &payload.source_path,
            Path::new(&payload.target_path),
            0.0,
            plan.parts[0].duration,
        ));
        run_ffmpeg_with_progress(
            &args,
            duration_to_millis(plan.parts[0].duration),
            |stage_percent| {
                emit_video_mask_render_progress(
                    &payload.render_id,
                    95 + (stage_percent.clamp(0, 99) * 4 / 99),
                    "写入输出",
                    1,
                    plan.parts.len(),
                    stage_percent,
                    on_progress,
                );
            },
        )?;
        let size = output_size(&payload.target_path)?;
        emit_video_mask_render_progress(
            &payload.render_id,
            100,
            "完成",
            plan.parts.len(),
            plan.parts.len(),
            100,
            on_progress,
        );
        return Ok(size);
    }

    let list_path = temp_dir.join("concat.txt");
    let list_text = part_paths
        .iter()
        .map(|path| format!("file '{}'", escape_concat_path(path)))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&list_path, list_text).map_err(|err| format!("写入拼接清单失败: {}", err))?;
    let args = vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-y".to_string(),
        "-fflags".to_string(),
        "+genpts".to_string(),
        "-f".to_string(),
        "concat".to_string(),
        "-safe".to_string(),
        "0".to_string(),
        "-i".to_string(),
        list_path.to_string_lossy().into_owned(),
        "-i".to_string(),
        payload.source_path.clone(),
        "-map".to_string(),
        "0".to_string(),
        "-map_metadata".to_string(),
        "1".to_string(),
        "-map_chapters".to_string(),
        "1".to_string(),
        "-t".to_string(),
        format_seconds(payload.duration),
        "-c".to_string(),
        "copy".to_string(),
        "-movflags".to_string(),
        "+faststart".to_string(),
        payload.target_path.clone(),
    ];
    let args = ffmpeg_args_with_progress(args);
    run_ffmpeg_with_progress(
        &args,
        duration_to_millis(payload.duration),
        |stage_percent| {
            emit_video_mask_render_progress(
                &payload.render_id,
                95 + (stage_percent.clamp(0, 99) * 4 / 99),
                "拼接输出",
                plan.parts.len(),
                plan.parts.len(),
                stage_percent,
                on_progress,
            );
        },
    )?;
    let size = output_size(&payload.target_path)?;
    emit_video_mask_render_progress(
        &payload.render_id,
        100,
        "完成",
        plan.parts.len(),
        plan.parts.len(),
        100,
        on_progress,
    );
    Ok(size)
}

fn output_size(path: &str) -> Result<u64, String> {
    fs::metadata(path)
        .map(|item| item.len())
        .map_err(|err| format!("读取输出文件失败: {}", err))
}

fn normalized_render_id(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Uuid::new_v4().to_string()
    } else {
        trimmed.to_string()
    }
}

fn duration_to_millis(duration: f64) -> Option<i64> {
    if duration.is_finite() && duration > 0.0 {
        Some((duration * 1000.0).round().max(1.0) as i64)
    } else {
        None
    }
}

fn ffmpeg_args_with_progress(mut args: Vec<String>) -> Vec<String> {
    let progress_args = vec![
        "-nostats".to_string(),
        "-progress".to_string(),
        "pipe:1".to_string(),
    ];
    if let Some(index) = args.iter().position(|item| item == "-y") {
        args.splice(index + 1..index + 1, progress_args);
        return args;
    }
    args.splice(0..0, progress_args);
    args
}

fn video_mask_part_progress_percent(
    completed_duration: f64,
    part_duration: f64,
    stage_percent: i64,
    total_duration: f64,
) -> i64 {
    let current_done = completed_duration.max(0.0)
        + part_duration.max(0.0) * (stage_percent.clamp(0, 100) as f64 / 100.0);
    ((current_done / total_duration.max(0.001)) * 95.0)
        .floor()
        .clamp(0.0, 95.0) as i64
}

fn emit_video_mask_render_progress<F>(
    render_id: &str,
    percent: i64,
    stage: &str,
    part_index: usize,
    part_count: usize,
    stage_percent: i64,
    on_progress: &mut F,
) where
    F: FnMut(VideoMaskRenderProgress),
{
    on_progress(VideoMaskRenderProgress {
        render_id: normalized_render_id(render_id),
        percent: percent.clamp(0, 100),
        stage: stage.to_string(),
        part_index,
        part_count,
        stage_percent: stage_percent.clamp(0, 100),
    });
}

fn normalized_preset(value: Option<&str>) -> String {
    match value.unwrap_or("veryfast") {
        "ultrafast" | "superfast" | "veryfast" | "faster" | "fast" | "medium" | "slow" => {
            value.unwrap_or("veryfast").to_string()
        }
        _ => "veryfast".to_string(),
    }
}

fn append_color_arg(args: &mut Vec<String>, key: &str, value: &str) {
    let value = value.trim();
    if value.is_empty() || value == "unknown" {
        return;
    }
    args.push(key.to_string());
    args.push(value.to_string());
}

fn escape_concat_path(path: &Path) -> String {
    path.to_string_lossy().replace('\'', "'\\''")
}

fn format_seconds(value: f64) -> String {
    format!("{:.6}", value.max(0.0))
}

fn format_fps(value: f64) -> String {
    if value > 0.0 {
        format!("{:.3}", value)
    } else {
        "30".to_string()
    }
}

fn load_active_auth(state: &State<'_, AppState>) -> Result<AuthInfo, String> {
    state
        .login_store
        .load_auth_info(&state.db)
        .map_err(|err| format!("读取登录信息失败: {}", err))?
        .ok_or_else(|| "请先登录Bilibili账号".to_string())
}

fn append_toolbox_log(state: &State<'_, AppState>, message: &str) {
    utils::append_log(state.app_log_path.as_ref(), message);
}

async fn fetch_all_seasons(
    state: &State<'_, AppState>,
    auth: &AuthInfo,
) -> Result<Vec<Value>, String> {
    let url = "https://member.bilibili.com/x2/creative/web/seasons";
    let mut result = Vec::new();
    let page_size = 30;
    for page in 1..=50 {
        let params = vec![
            ("pn".to_string(), page.to_string()),
            ("ps".to_string(), page_size.to_string()),
            ("order".to_string(), "mtime".to_string()),
            ("sort".to_string(), "desc".to_string()),
            ("draft".to_string(), "1".to_string()),
            ("source".to_string(), "0".to_string()),
        ];
        let data = state
            .bilibili
            .get_json(url, &params, Some(auth), false)
            .await?;
        let page_items = data
            .get("seasons")
            .and_then(|item| item.as_array())
            .cloned()
            .unwrap_or_default();
        let count = page_items.len();
        result.extend(page_items);
        if count < page_size {
            break;
        }
    }
    Ok(result)
}

async fn fetch_season_detail(
    state: &State<'_, AppState>,
    auth: &AuthInfo,
    season_id: i64,
) -> Result<Value, String> {
    let url = "https://member.bilibili.com/x2/creative/web/season";
    let params = vec![("id".to_string(), season_id.to_string())];
    state
        .bilibili
        .get_json(url, &params, Some(auth), false)
        .await
}

async fn fetch_section_detail(
    state: &State<'_, AppState>,
    auth: &AuthInfo,
    section_id: i64,
) -> Result<Value, String> {
    let url = "https://member.bilibili.com/x2/creative/web/season/section";
    let params = vec![("id".to_string(), section_id.to_string())];
    state
        .bilibili
        .get_json(url, &params, Some(auth), false)
        .await
}

fn build_season_list_item(raw: &Value) -> SeasonListItem {
    let season = raw.get("season").unwrap_or(&Value::Null);
    let section_count = season_sections(raw).len() as i64;
    let episode_count = section_episode_count(raw);
    SeasonListItem {
        season_id: season.get("id").and_then(|item| item.as_i64()).unwrap_or(0),
        title: value_string(season, "title"),
        description: value_string(season, "desc"),
        cover: value_string(season, "cover"),
        section_count,
        episode_count,
        complete: section_count > 0,
        state: season
            .get("state")
            .and_then(|item| item.as_i64())
            .unwrap_or(0),
        mtime: season
            .get("mtime")
            .and_then(|item| item.as_i64())
            .unwrap_or(0),
    }
}

async fn build_season_backup(
    state: &State<'_, AppState>,
    auth: &AuthInfo,
    raw: &Value,
) -> Result<SeasonBackup, String> {
    let season = raw.get("season").unwrap_or(&Value::Null);
    let mut sections = Vec::new();
    for section_summary in season_sections(raw) {
        let section_id = section_summary
            .get("id")
            .and_then(|item| item.as_i64())
            .unwrap_or(0);
        if section_id <= 0 {
            continue;
        }
        let detail = fetch_section_detail(state, auth, section_id).await?;
        sections.push(build_section_backup(&detail));
    }

    if sections.is_empty() {
        let episodes = raw
            .get("part_episodes")
            .and_then(|item| item.as_array())
            .map(|items| {
                items
                    .iter()
                    .enumerate()
                    .map(|(index, item)| build_episode_backup(item, index as i64 + 1))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if !episodes.is_empty() {
            sections.push(SeasonSectionBackup {
                section_id: first_section_id(raw),
                section_type: 0,
                title: "正片".to_string(),
                order: 1,
                cover: String::new(),
                state: 0,
                part_state: 0,
                ep_count: episodes.len() as i64,
                episodes,
            });
        }
    }

    sections.sort_by_key(|item| item.order);
    let publish_times = fetch_archive_publish_times_for_backup(state, auth, &sections).await?;
    let filled_publish_time_count =
        fill_section_episode_publish_times(&mut sections, &publish_times);
    append_toolbox_log(
        state,
        &format!(
            "toolbox_bilibili_season_backup_publish_times source_season_id={} found={} filled={}",
            season.get("id").and_then(|item| item.as_i64()).unwrap_or(0),
            publish_times.len(),
            filled_publish_time_count
        ),
    );
    let flattened_episodes = sections
        .iter()
        .flat_map(|section| section.episodes.clone())
        .collect::<Vec<_>>();
    let episode_count = sections.iter().map(|item| item.ep_count).sum::<i64>();
    let captured_episode_count = flattened_episodes.len() as i64;
    let complete = episode_count == 0 || captured_episode_count >= episode_count;

    Ok(SeasonBackup {
        backup_id: Uuid::new_v4().to_string(),
        source_season_id: season.get("id").and_then(|item| item.as_i64()).unwrap_or(0),
        title: value_string(season, "title"),
        description: value_string(season, "desc"),
        cover: value_string(season, "cover"),
        season_price: season
            .get("season_price")
            .and_then(|item| item.as_i64())
            .unwrap_or(0),
        no_section: season.get("no_section").and_then(|item| item.as_i64()),
        section_id: first_section_id(raw),
        section_count: sections.len() as i64,
        episode_count,
        captured_episode_count,
        complete,
        sections,
        episodes: flattened_episodes,
        created_at: Utc::now().to_rfc3339(),
    })
}

fn build_section_backup(raw: &Value) -> SeasonSectionBackup {
    let section = raw.get("section").unwrap_or(&Value::Null);
    let episodes = raw
        .get("episodes")
        .and_then(|item| item.as_array())
        .map(|items| {
            items
                .iter()
                .enumerate()
                .map(|(index, item)| build_episode_backup(item, index as i64 + 1))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    SeasonSectionBackup {
        section_id: section
            .get("id")
            .and_then(|item| item.as_i64())
            .unwrap_or(0),
        section_type: section
            .get("type")
            .and_then(|item| item.as_i64())
            .unwrap_or(0),
        title: value_string(section, "title"),
        order: section
            .get("order")
            .and_then(|item| item.as_i64())
            .unwrap_or(0),
        cover: value_string(section, "cover"),
        state: section
            .get("state")
            .and_then(|item| item.as_i64())
            .unwrap_or(0),
        part_state: section
            .get("partState")
            .and_then(|item| item.as_i64())
            .unwrap_or(0),
        ep_count: section
            .get("epCount")
            .and_then(|item| item.as_i64())
            .unwrap_or(episodes.len() as i64),
        episodes,
    }
}

fn build_episode_backup(raw: &Value, fallback_sort: i64) -> SeasonEpisodeBackup {
    let archive_title = optional_value_string(raw, "archiveTitle");
    let title = archive_title
        .clone()
        .or_else(|| optional_value_string(raw, "title"))
        .unwrap_or_default();
    SeasonEpisodeBackup {
        episode_id: raw.get("id").and_then(|item| item.as_i64()),
        title,
        aid: raw.get("aid").and_then(|item| item.as_i64()).unwrap_or(0),
        cid: raw.get("cid").and_then(|item| item.as_i64()).unwrap_or(0),
        bvid: optional_value_string(raw, "bvid"),
        archive_title,
        video_title: optional_value_string(raw, "videoTitle"),
        sort: raw
            .get("order")
            .and_then(|item| item.as_i64())
            .unwrap_or(fallback_sort),
        charging_pay: raw
            .get("charging_pay")
            .and_then(|item| item.as_i64())
            .unwrap_or(0),
        member_first: raw
            .get("member_first")
            .and_then(|item| item.as_i64())
            .unwrap_or(0),
        limited_free: raw
            .get("limited_free")
            .and_then(|item| item.as_bool())
            .unwrap_or(false),
        published_at: episode_publish_time(raw),
    }
}

fn episode_publish_time(raw: &Value) -> Option<i64> {
    raw.get("publishedAt")
        .and_then(|item| item.as_i64())
        .or_else(|| raw.get("pubdate").and_then(|item| item.as_i64()))
        .or_else(|| raw.get("ctime").and_then(|item| item.as_i64()))
        .or_else(|| raw.get("pubDate").and_then(|item| item.as_i64()))
        .or_else(|| raw.get("publishTime").and_then(|item| item.as_i64()))
}

async fn fetch_archive_publish_times_for_backup(
    state: &State<'_, AppState>,
    auth: &AuthInfo,
    sections: &[SeasonSectionBackup],
) -> Result<HashMap<i64, i64>, String> {
    let mut target_aids = sections
        .iter()
        .flat_map(|section| section.episodes.iter())
        .filter(|episode| episode.aid > 0 && episode.published_at.is_none())
        .map(|episode| episode.aid)
        .collect::<HashSet<_>>();
    if target_aids.is_empty() {
        return Ok(HashMap::new());
    }

    let mut result = HashMap::new();
    for status in [
        BILIBILI_ARCHIVE_STATUS_PUBLISHED,
        BILIBILI_ARCHIVE_STATUS_REVIEWING,
        BILIBILI_ARCHIVE_STATUS_REJECTED,
    ] {
        fetch_archive_publish_times_by_status(state, auth, status, &mut target_aids, &mut result)
            .await?;
        if target_aids.is_empty() {
            break;
        }
    }

    Ok(result)
}

async fn fetch_archive_publish_times_by_status(
    state: &State<'_, AppState>,
    auth: &AuthInfo,
    status: &str,
    target_aids: &mut HashSet<i64>,
    result: &mut HashMap<i64, i64>,
) -> Result<(), String> {
    let url = "https://member.bilibili.com/x/web/archives";
    let page_size = 20_i64;
    let mut page = 1_i64;

    loop {
        let params = vec![
            ("status".to_string(), status.to_string()),
            ("pn".to_string(), page.to_string()),
            ("ps".to_string(), page_size.to_string()),
            ("coop".to_string(), "1".to_string()),
            ("interactive".to_string(), "1".to_string()),
        ];
        let data = state
            .bilibili
            .get_json(url, &params, Some(auth), false)
            .await?;
        let arc_audits = data
            .get("arc_audits")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();

        for item in &arc_audits {
            let Some(aid) = archive_aid_from_audit_item(item) else {
                continue;
            };
            if !target_aids.contains(&aid) {
                continue;
            }
            if let Some(published_at) = archive_publish_time_from_audit_item(item) {
                target_aids.remove(&aid);
                result.insert(aid, published_at);
            }
        }

        append_toolbox_log(
            state,
            &format!(
                "toolbox_bilibili_season_backup_archive_page status={} page={} items={} remaining={}",
                status,
                page,
                arc_audits.len(),
                target_aids.len()
            ),
        );

        if target_aids.is_empty() {
            break;
        }
        let total_count = data
            .get("page")
            .and_then(|value| value.get("count"))
            .and_then(|value| value.as_i64())
            .unwrap_or(0);
        if total_count <= 0 || page * page_size >= total_count || arc_audits.is_empty() {
            break;
        }
        page += 1;
    }

    Ok(())
}

fn archive_aid_from_audit_item(item: &Value) -> Option<i64> {
    item.get("Archive")
        .and_then(|value| value.get("aid"))
        .and_then(|value| value.as_i64())
        .or_else(|| item.get("aid").and_then(|value| value.as_i64()))
}

fn archive_publish_time_from_audit_item(item: &Value) -> Option<i64> {
    let archive = item.get("Archive").unwrap_or(item);
    episode_publish_time(archive)
        .or_else(|| archive.get("ptime").and_then(|value| value.as_i64()))
        .or_else(|| archive.get("submit_time").and_then(|value| value.as_i64()))
        .or_else(|| archive.get("submitTime").and_then(|value| value.as_i64()))
        .or_else(|| archive.get("created_at").and_then(|value| value.as_i64()))
        .or_else(|| archive.get("createdAt").and_then(|value| value.as_i64()))
        .or_else(|| archive.get("mtime").and_then(|value| value.as_i64()))
        .or_else(|| item.get("pubdate").and_then(|value| value.as_i64()))
        .or_else(|| item.get("ctime").and_then(|value| value.as_i64()))
        .or_else(|| item.get("mtime").and_then(|value| value.as_i64()))
}

fn fill_section_episode_publish_times(
    sections: &mut [SeasonSectionBackup],
    publish_times: &HashMap<i64, i64>,
) -> usize {
    let mut filled_count = 0usize;
    for episode in sections
        .iter_mut()
        .flat_map(|section| section.episodes.iter_mut())
    {
        if episode.published_at.is_none() {
            if let Some(value) = publish_times.get(&episode.aid) {
                episode.published_at = Some(*value);
                filled_count += 1;
            }
        }
    }
    filled_count
}

fn normalize_episode_sort_mode(value: Option<&str>) -> String {
    match value.unwrap_or("backup") {
        "publish_asc" | "publish_desc" => value.unwrap().to_string(),
        _ => "backup".to_string(),
    }
}

fn apply_restore_episode_sort(
    sections: &mut [SeasonSectionBackup],
    sort_mode: &str,
    warnings: &mut Vec<String>,
) {
    if sort_mode == "backup" {
        return;
    }

    let mut publish_times = HashMap::new();
    for section in sections.iter() {
        for episode in &section.episodes {
            if let Some(value) = episode.published_at {
                publish_times.insert(episode.aid, value);
            }
        }
    }

    let missing_count_before_sort = sections
        .iter()
        .flat_map(|section| section.episodes.iter())
        .filter(|episode| episode.aid > 0 && !publish_times.contains_key(&episode.aid))
        .count();

    let mut missing_count = 0usize;
    for section in sections.iter_mut() {
        missing_count += sort_section_episodes_by_publish_time(section, &publish_times, sort_mode);
    }

    if missing_count > 0 {
        warnings.push(format!(
            "{} 个视频缺少投稿时间，已排在已知投稿时间之后并保留原顺序；请重新备份后再按投稿时间恢复会更准确",
            missing_count.max(missing_count_before_sort)
        ));
    }
}

fn sort_section_episodes_by_publish_time(
    section: &mut SeasonSectionBackup,
    publish_times: &HashMap<i64, i64>,
    sort_mode: &str,
) -> usize {
    let mut missing_count = 0usize;
    let mut indexed = section
        .episodes
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, episode)| {
            let published_at = episode
                .published_at
                .or_else(|| publish_times.get(&episode.aid).copied());
            if published_at.is_none() {
                missing_count += 1;
            }
            (index, published_at, episode)
        })
        .collect::<Vec<_>>();

    indexed.sort_by(|left, right| match (left.1, right.1) {
        (Some(left_time), Some(right_time)) if sort_mode == "publish_desc" => right_time
            .cmp(&left_time)
            .then_with(|| left.0.cmp(&right.0)),
        (Some(left_time), Some(right_time)) => left_time
            .cmp(&right_time)
            .then_with(|| left.0.cmp(&right.0)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => left.0.cmp(&right.0),
    });

    section.episodes = indexed
        .into_iter()
        .enumerate()
        .map(|(index, (_, published_at, mut episode))| {
            episode.published_at = published_at;
            episode.sort = (index + 1) as i64;
            episode
        })
        .collect();
    missing_count
}

async fn restore_season(
    state: &State<'_, AppState>,
    auth: &AuthInfo,
    csrf: &str,
    backup: &SeasonBackup,
    episode_sort_mode: &str,
) -> Result<SeasonRestoreResult, String> {
    let params = vec![("csrf".to_string(), csrf.to_string())];
    let mut warnings = Vec::new();
    let mut sections = backup_sections_for_restore(backup);
    apply_restore_episode_sort(&mut sections, episode_sort_mode, &mut warnings);
    let expected_no_section = restore_no_section(backup, &sections);
    let new_season_id = create_season(state, auth, &params, backup).await?;
    append_toolbox_log(
        state,
        &format!(
            "toolbox_bilibili_season_restore_create_season_ok new_season_id={} title={} no_section={}",
            new_season_id, backup.title, expected_no_section
        ),
    );
    let default_section = fetch_first_section_id(state, auth, new_season_id)
        .await
        .ok();
    append_toolbox_log(
        state,
        &format!(
            "toolbox_bilibili_season_restore_default_section new_season_id={} default_section_id={:?}",
            new_season_id, default_section
        ),
    );
    if let Err(err) =
        switch_season_section_mode(state, auth, csrf, new_season_id, expected_no_section).await
    {
        return Err(err);
    }
    append_toolbox_log(
        state,
        &format!(
            "toolbox_bilibili_season_restore_switch_section_mode_ok season_id={} no_section={}",
            new_season_id, expected_no_section
        ),
    );
    let mut added_episode_count = 0usize;
    let mut created_section_count = 0usize;
    let mut section_targets = Vec::new();

    if let Some(default_section_id) = default_section {
        append_toolbox_log(
            state,
            &format!(
                "toolbox_bilibili_season_restore_delete_default_section_start season_id={} section_id={}",
                new_season_id, default_section_id
            ),
        );
        delete_section(state, auth, csrf, default_section_id).await?;
        append_toolbox_log(
            state,
            &format!(
                "toolbox_bilibili_season_restore_delete_default_section_ok season_id={} section_id={}",
                new_season_id, default_section_id
            ),
        );
    }

    for section in sections {
        let new_section_id = create_section(state, auth, &params, new_season_id, &section).await?;
        created_section_count += 1;
        if let Ok(detail) = fetch_section_detail(state, auth, new_section_id).await {
            let existing_count = detail
                .get("episodes")
                .and_then(|item| item.as_array())
                .map(|items| items.len())
                .unwrap_or(0);
            let existing_show = detail
                .get("section")
                .and_then(|item| item.get("show"))
                .and_then(|item| item.as_i64())
                .unwrap_or(0);
            append_toolbox_log(
                state,
                &format!(
                    "toolbox_bilibili_season_restore_create_section_snapshot season_id={} section_id={} title={} existing_episodes={} show={}",
                    new_season_id,
                    new_section_id,
                    section.title,
                    existing_count,
                    existing_show
                ),
            );
        }
        append_toolbox_log(
            state,
            &format!(
                "toolbox_bilibili_season_restore_create_section_ok new_season_id={} section_id={} title={} type={} order={}",
                new_season_id,
                new_section_id,
                section.title,
                section.section_type,
                section.order
            ),
        );
        section_targets.push((new_section_id, section));
    }

    if let Err(err) = edit_season_sort(
        state,
        auth,
        &params,
        new_season_id,
        backup,
        &section_targets,
    )
    .await
    {
        warnings.push(format!("合集分组模式保存失败: {}", err));
        append_toolbox_log(
            state,
            &format!(
                "toolbox_bilibili_season_restore_edit_season_sort_before_bind_err season_id={} err={}",
                new_season_id, err
            ),
        );
    } else {
        append_toolbox_log(
            state,
            &format!(
                "toolbox_bilibili_season_restore_edit_season_sort_before_bind_ok season_id={} sections={}",
                new_season_id,
                section_targets.len()
            ),
        );
    }

    for (new_section_id, section) in &section_targets {
        let section_added = add_episodes_to_section(
            state,
            auth,
            &params,
            *new_section_id,
            section,
            &mut warnings,
        )
        .await?;
        added_episode_count += section_added;
        append_toolbox_log(
            state,
            &format!(
                "toolbox_bilibili_season_restore_add_episodes_ok season_id={} section_id={} title={} added={}",
                new_season_id, new_section_id, section.title, section_added
            ),
        );
        if let Err(err) = edit_section_sort(
            state,
            auth,
            &params,
            new_season_id,
            *new_section_id,
            section,
        )
        .await
        {
            warnings.push(format!(
                "子合集「{}」标题或排序恢复失败: {}",
                section.title, err
            ));
            append_toolbox_log(
                state,
                &format!(
                    "toolbox_bilibili_season_restore_edit_section_sort_err season_id={} section_id={} title={} err={}",
                    new_season_id, new_section_id, section.title, err
                ),
            );
        } else {
            append_toolbox_log(
                state,
                &format!(
                    "toolbox_bilibili_season_restore_edit_section_sort_ok season_id={} section_id={} title={}",
                    new_season_id, new_section_id, section.title
                ),
            );
        }
    }

    if let Err(err) = edit_season_sort(
        state,
        auth,
        &params,
        new_season_id,
        backup,
        &section_targets,
    )
    .await
    {
        warnings.push(format!("合集信息或子合集排序保存失败: {}", err));
        append_toolbox_log(
            state,
            &format!(
                "toolbox_bilibili_season_restore_edit_season_sort_after_bind_err season_id={} err={}",
                new_season_id, err
            ),
        );
    } else {
        append_toolbox_log(
            state,
            &format!(
                "toolbox_bilibili_season_restore_edit_season_sort_after_bind_ok season_id={} sections={}",
                new_season_id,
                section_targets.len()
            ),
        );
    }
    let verify_result =
        match verify_restored_season(state, auth, new_season_id, &section_targets).await {
            Ok(result) => result,
            Err(err) => {
                warnings.push(format!("恢复后自动验收查询失败: {}", err));
                append_toolbox_log(
                    state,
                    &format!(
                        "toolbox_bilibili_season_restore_verify_err season_id={} err={}",
                        new_season_id, err
                    ),
                );
                SeasonRestoreVerification {
                    no_section: None,
                    sections: Vec::new(),
                }
            }
        };
    let verification = verify_result.sections;
    let restored_section_count = verification.len();
    let restored_episode_count = verification
        .iter()
        .map(|item| item.actual_episodes)
        .sum::<usize>();
    let restored_no_section = verify_result.no_section;
    let verified = !verification.is_empty() && verification.iter().all(|item| item.matched);
    append_toolbox_log(
        state,
        &format!(
            "toolbox_bilibili_season_restore_verify_summary season_id={} no_section={:?} sections={} verified={}",
            new_season_id,
            verify_result.no_section,
            restored_section_count,
            verified
        ),
    );

    Ok(SeasonRestoreResult {
        new_season_id,
        added_episode_count,
        created_section_count,
        verified,
        restored_section_count,
        restored_episode_count,
        restored_no_section,
        expected_no_section,
        episode_sort_mode: episode_sort_mode.to_string(),
        verification,
        warnings,
    })
}

async fn create_season(
    state: &State<'_, AppState>,
    auth: &AuthInfo,
    params: &[(String, String)],
    backup: &SeasonBackup,
) -> Result<i64, String> {
    let add_url = "https://member.bilibili.com/x2/creative/web/season/add";
    let payload = json!({
        "cover": backup.cover,
        "title": backup.title,
        "desc": backup.description,
        "season_price": backup.season_price,
        "captcha_token": ""
    });
    let data = post_bilibili_json_with_retry(state, auth, add_url, params, &payload).await?;
    data.as_i64()
        .or_else(|| data.get("id").and_then(|item| item.as_i64()))
        .ok_or_else(|| "新增合集成功但未返回合集ID".to_string())
}

fn backup_sections_for_restore(backup: &SeasonBackup) -> Vec<SeasonSectionBackup> {
    if !backup.sections.is_empty() {
        let mut sections = backup.sections.clone();
        sections.sort_by_key(|item| item.order);
        return sections;
    }

    if backup.episodes.is_empty() {
        return Vec::new();
    }

    vec![SeasonSectionBackup {
        section_id: backup.section_id,
        section_type: 0,
        title: "正片".to_string(),
        order: 1,
        cover: String::new(),
        state: 0,
        part_state: 0,
        ep_count: backup.episodes.len() as i64,
        episodes: backup.episodes.clone(),
    }]
}

async fn fetch_first_section_id(
    state: &State<'_, AppState>,
    auth: &AuthInfo,
    season_id: i64,
) -> Result<i64, String> {
    let detail = fetch_season_detail(state, auth, season_id).await?;
    let section = season_sections(&detail)
        .first()
        .copied()
        .ok_or_else(|| "未找到新合集默认子合集".to_string())?;
    section
        .get("id")
        .and_then(|item| item.as_i64())
        .filter(|id| *id > 0)
        .ok_or_else(|| "未找到新合集默认子合集ID".to_string())
}

async fn delete_section(
    state: &State<'_, AppState>,
    auth: &AuthInfo,
    csrf: &str,
    section_id: i64,
) -> Result<(), String> {
    let url = "https://member.bilibili.com/x2/creative/web/season/section/del";
    let form = vec![
        ("id".to_string(), section_id.to_string()),
        ("csrf".to_string(), csrf.to_string()),
    ];
    let _ = post_bilibili_form_with_retry(state, auth, url, &[], &form).await?;
    Ok(())
}

async fn create_section(
    state: &State<'_, AppState>,
    auth: &AuthInfo,
    params: &[(String, String)],
    season_id: i64,
    section: &SeasonSectionBackup,
) -> Result<i64, String> {
    let url = "https://member.bilibili.com/x2/creative/web/season/section/add";
    let payload = json!({
        "type": section.section_type,
        "seasonId": season_id,
        "title": section.title,
        "captcha_token": ""
    });
    let data = post_bilibili_json_with_retry(state, auth, url, params, &payload).await?;
    data.as_i64()
        .or_else(|| data.get("id").and_then(|item| item.as_i64()))
        .ok_or_else(|| "新增子合集成功但未返回子合集ID".to_string())
}

async fn add_episodes_to_section(
    state: &State<'_, AppState>,
    auth: &AuthInfo,
    params: &[(String, String)],
    section_id: i64,
    section: &SeasonSectionBackup,
    warnings: &mut Vec<String>,
) -> Result<usize, String> {
    if section.episodes.is_empty() {
        return Ok(0);
    }

    let url = "https://member.bilibili.com/x2/creative/web/season/section/episodes/add";
    let valid_episodes = section
        .episodes
        .iter()
        .filter(|item| item.aid > 0 && item.cid > 0)
        .collect::<Vec<_>>();
    let mut added_count = 0usize;
    for chunk in valid_episodes.chunks(50) {
        let episodes = chunk
            .iter()
            .map(|item| {
                json!({
                    "title": item.title,
                    "aid": item.aid,
                    "cid": item.cid,
                    "charging_pay": item.charging_pay,
                    "member_first": item.member_first,
                    "limited_free": item.limited_free
                })
            })
            .collect::<Vec<_>>();
        let payload = json!({
            "sectionId": section_id,
            "episodes": episodes
        });
        match post_bilibili_json_with_retry(state, auth, url, params, &payload).await {
            Ok(_) => {
                added_count += chunk.len();
            }
            Err(err) if is_episode_exists_error(&err) => {
                added_count += add_episodes_individually(
                    state,
                    auth,
                    params,
                    section_id,
                    &section.title,
                    chunk,
                    warnings,
                )
                .await?;
            }
            Err(err) => return Err(err),
        }
    }
    Ok(added_count)
}

async fn edit_section_sort(
    state: &State<'_, AppState>,
    auth: &AuthInfo,
    params: &[(String, String)],
    season_id: i64,
    section_id: i64,
    section: &SeasonSectionBackup,
) -> Result<(), String> {
    let detail = fetch_section_detail(state, auth, section_id).await?;
    let added_episodes = detail
        .get("episodes")
        .and_then(|item| item.as_array())
        .cloned()
        .unwrap_or_default();
    let sorts = build_episode_sorts(&section.episodes, &added_episodes);
    let url = "https://member.bilibili.com/x2/creative/web/season/section/edit";
    let payload = json!({
        "section": {
            "id": section_id,
            "type": section.section_type,
            "seasonId": season_id,
            "title": section.title
        },
        "sorts": sorts,
        "captcha_token": ""
    });
    let _ = post_bilibili_json_with_retry(state, auth, url, params, &payload).await?;
    Ok(())
}

async fn edit_season_sort(
    state: &State<'_, AppState>,
    auth: &AuthInfo,
    params: &[(String, String)],
    season_id: i64,
    backup: &SeasonBackup,
    section_targets: &[(i64, SeasonSectionBackup)],
) -> Result<(), String> {
    let url = "https://member.bilibili.com/x2/creative/web/season/edit";
    let sorts = section_targets
        .iter()
        .enumerate()
        .map(|(index, (section_id, _))| {
            json!({
                "id": section_id,
                "sort": (index + 1) as i64
            })
        })
        .collect::<Vec<_>>();
    let payload = json!({
        "season": {
            "id": season_id,
            "title": backup.title,
            "desc": backup.description,
            "cover": backup.cover,
            "isEnd": 0,
            "season_price": backup.season_price,
            "captcha_token": ""
        },
        "sorts": sorts
    });
    let _ = post_bilibili_json_with_retry(state, auth, url, params, &payload).await?;
    Ok(())
}

fn restore_no_section(backup: &SeasonBackup, sections: &[SeasonSectionBackup]) -> i64 {
    backup.no_section.unwrap_or_else(|| {
        if sections.len() > 1 || sections.iter().any(|section| section.section_type == 0) {
            0
        } else {
            1
        }
    })
}

async fn switch_season_section_mode(
    state: &State<'_, AppState>,
    auth: &AuthInfo,
    csrf: &str,
    season_id: i64,
    no_section: i64,
) -> Result<(), String> {
    let url = "https://member.bilibili.com/x2/creative/web/season/section/switch";
    let form = vec![
        ("season_id".to_string(), season_id.to_string()),
        ("no_section".to_string(), no_section.to_string()),
        ("csrf".to_string(), csrf.to_string()),
    ];
    let _ = post_bilibili_form_with_retry(state, auth, url, &[], &form).await?;
    Ok(())
}

struct SeasonRestoreVerification {
    no_section: Option<i64>,
    sections: Vec<SeasonRestoreVerificationItem>,
}

async fn verify_restored_season(
    state: &State<'_, AppState>,
    auth: &AuthInfo,
    season_id: i64,
    expected_sections: &[(i64, SeasonSectionBackup)],
) -> Result<SeasonRestoreVerification, String> {
    let detail = fetch_season_detail(state, auth, season_id).await?;
    let no_section = detail
        .get("season")
        .and_then(|item| item.get("no_section"))
        .and_then(|item| item.as_i64());
    let actual_sections = season_sections(&detail);
    let mut result = Vec::new();
    for (index, (_, expected)) in expected_sections.iter().enumerate() {
        let actual = actual_sections.get(index).copied().unwrap_or(&Value::Null);
        let section_id = actual.get("id").and_then(|item| item.as_i64()).unwrap_or(0);
        let actual_title = value_string(actual, "title");
        let actual_type = actual
            .get("type")
            .and_then(|item| item.as_i64())
            .unwrap_or(-1);
        let section_detail = if section_id > 0 {
            Some(fetch_section_detail(state, auth, section_id).await?)
        } else {
            None
        };
        let actual_show = section_detail
            .as_ref()
            .and_then(|value| value.get("section"))
            .and_then(|item| item.get("show"))
            .and_then(|item| item.as_i64())
            .unwrap_or(0);
        let actual_episodes = section_detail
            .as_ref()
            .and_then(|value| value.get("episodes"))
            .and_then(|item| item.as_array())
            .map(|items| items.len())
            .unwrap_or(0);
        let expected_episodes = expected
            .episodes
            .iter()
            .filter(|item| item.aid > 0 && item.cid > 0)
            .count();
        let matched = actual_title == expected.title
            && actual_type == expected.section_type
            && actual_episodes == expected_episodes
            && actual_show == 1;
        result.push(SeasonRestoreVerificationItem {
            title: expected.title.clone(),
            expected_type: expected.section_type,
            actual_type,
            expected_episodes,
            actual_episodes,
            actual_show,
            matched,
        });
    }
    Ok(SeasonRestoreVerification {
        no_section,
        sections: result,
    })
}

async fn add_episodes_individually(
    state: &State<'_, AppState>,
    auth: &AuthInfo,
    params: &[(String, String)],
    section_id: i64,
    section_title: &str,
    episodes: &[&SeasonEpisodeBackup],
    warnings: &mut Vec<String>,
) -> Result<usize, String> {
    let url = "https://member.bilibili.com/x2/creative/web/season/section/episodes/add";
    let mut added_count = 0usize;
    for item in episodes {
        let payload = json!({
            "sectionId": section_id,
            "episodes": [
                {
                    "title": item.title,
                    "aid": item.aid,
                    "cid": item.cid,
                    "charging_pay": item.charging_pay,
                    "member_first": item.member_first,
                    "limited_free": item.limited_free
                }
            ]
        });
        match post_bilibili_json_with_retry(state, auth, url, params, &payload).await {
            Ok(_) => added_count += 1,
            Err(err) if is_episode_exists_error(&err) => {
                warnings.push(format!(
                    "子合集「{}」视频「{}」已存在在合集中，已跳过",
                    section_title, item.title
                ));
            }
            Err(err) => return Err(err),
        }
    }
    Ok(added_count)
}

async fn post_bilibili_json_with_retry(
    state: &State<'_, AppState>,
    auth: &AuthInfo,
    url: &str,
    params: &[(String, String)],
    payload: &Value,
) -> Result<Value, String> {
    let mut last_error = String::new();
    for attempt in 0..8 {
        match state
            .bilibili
            .post_json(url, params, payload, Some(auth))
            .await
        {
            Ok(data) => return Ok(data),
            Err(err) if is_rate_limited_error(&err) => {
                last_error = err;
                sleep(Duration::from_millis(2500 + attempt * 1000)).await;
            }
            Err(err) => return Err(err),
        }
    }
    Err(last_error)
}

async fn post_bilibili_form_with_retry(
    state: &State<'_, AppState>,
    auth: &AuthInfo,
    url: &str,
    params: &[(String, String)],
    form: &[(String, String)],
) -> Result<Value, String> {
    let mut last_error = String::new();
    for attempt in 0..8 {
        match state
            .bilibili
            .post_form(url, params, form, Some(auth))
            .await
        {
            Ok(data) => return Ok(data),
            Err(err) if is_rate_limited_error(&err) => {
                last_error = err;
                sleep(Duration::from_millis(2500 + attempt * 1000)).await;
            }
            Err(err) => return Err(err),
        }
    }
    Err(last_error)
}

fn is_rate_limited_error(err: &str) -> bool {
    err.contains("20111") || err.contains("过于频繁")
}

fn is_episode_exists_error(err: &str) -> bool {
    err.contains("20080") || err.contains("已存在在合集中")
}

fn build_episode_sorts(original: &[SeasonEpisodeBackup], added: &[Value]) -> Vec<Value> {
    original
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let id = added
                .iter()
                .find(|added_item| {
                    added_item.get("aid").and_then(|value| value.as_i64()) == Some(item.aid)
                        && added_item.get("cid").and_then(|value| value.as_i64()) == Some(item.cid)
                })
                .and_then(|added_item| added_item.get("id"))
                .and_then(|value| value.as_i64())?;
            Some(json!({
                "id": id,
                "sort": (index + 1) as i64
            }))
        })
        .collect()
}

fn section_episode_count(raw: &Value) -> i64 {
    season_sections(raw)
        .iter()
        .map(|item| {
            item.get("epCount")
                .and_then(|value| value.as_i64())
                .unwrap_or(0)
        })
        .sum()
}

fn season_sections(raw: &Value) -> Vec<&Value> {
    raw.get("sections")
        .and_then(|item| item.get("sections"))
        .and_then(|item| item.as_array())
        .map(|items| items.iter().collect())
        .unwrap_or_default()
}

fn first_section_id(raw: &Value) -> i64 {
    raw.get("sections")
        .and_then(|item| item.get("sections"))
        .and_then(|item| item.as_array())
        .and_then(|items| items.first())
        .and_then(|item| item.get("id"))
        .and_then(|item| item.as_i64())
        .unwrap_or(0)
}

fn value_string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|item| item.as_str())
        .unwrap_or_default()
        .to_string()
}

fn optional_value_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|item| item.as_str())
        .map(|item| item.to_string())
}

fn season_backups_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|err| format!("读取应用数据目录失败: {}", err))?
        .join("toolbox");
    fs::create_dir_all(&dir).map_err(|err| format!("创建备份目录失败: {}", err))?;
    Ok(dir.join("bilibili-season-backups.json"))
}

fn video_mask_thumbnail_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|err| format!("读取应用数据目录失败: {}", err))?
        .join("toolbox")
        .join("video-mask-thumbnails");
    fs::create_dir_all(&dir).map_err(|err| format!("创建关键帧缓存目录失败: {}", err))?;
    Ok(dir)
}

fn read_season_backups(app: &AppHandle) -> Result<Vec<SeasonBackup>, String> {
    let path = season_backups_path(app)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(path).map_err(|err| format!("读取备份失败: {}", err))?;
    serde_json::from_str(&text).map_err(|err| format!("解析备份失败: {}", err))
}

fn write_season_backups(app: &AppHandle, backups: &[SeasonBackup]) -> Result<(), String> {
    let path = season_backups_path(app)?;
    let text =
        serde_json::to_string_pretty(backups).map_err(|err| format!("序列化备份失败: {}", err))?;
    fs::write(path, text).map_err(|err| format!("写入备份失败: {}", err))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bili_clip_flow_mask_{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn make_test_source(dir: &Path, duration: i64) -> (PathBuf, PathBuf) {
        make_test_source_with_gop(dir, duration, 30)
    }

    fn make_test_source_with_gop(dir: &Path, duration: i64, gop: i64) -> (PathBuf, PathBuf) {
        let source = dir.join("source.mp4");
        let mask = dir.join("mask.png");
        run_ffmpeg(&[
            "-hide_banner".to_string(),
            "-loglevel".to_string(),
            "error".to_string(),
            "-y".to_string(),
            "-f".to_string(),
            "lavfi".to_string(),
            "-i".to_string(),
            format!("testsrc2=size=320x180:rate=30:duration={}", duration),
            "-f".to_string(),
            "lavfi".to_string(),
            "-i".to_string(),
            format!("sine=frequency=440:duration={}", duration),
            "-c:v".to_string(),
            "libx264".to_string(),
            "-g".to_string(),
            gop.to_string(),
            "-keyint_min".to_string(),
            gop.to_string(),
            "-sc_threshold".to_string(),
            "0".to_string(),
            "-pix_fmt".to_string(),
            "yuv420p".to_string(),
            "-c:a".to_string(),
            "aac".to_string(),
            // 显式固定 timescale，模拟真实直播源常见的 1/16000 时间基，
            // 与 libx264 按帧率自选的默认时间基不同，便于回归覆盖时间基不一致导致的截断。
            "-video_track_timescale".to_string(),
            "16000".to_string(),
            source.to_string_lossy().into_owned(),
        ])
        .expect("generate source video");
        run_ffmpeg(&[
            "-hide_banner".to_string(),
            "-loglevel".to_string(),
            "error".to_string(),
            "-y".to_string(),
            "-f".to_string(),
            "lavfi".to_string(),
            "-i".to_string(),
            "color=c=red:s=80x48".to_string(),
            "-frames:v".to_string(),
            "1".to_string(),
            mask.to_string_lossy().into_owned(),
        ])
        .expect("generate mask image");
        (source, mask)
    }

    /// 读取指定文件第一路视频流的时长（秒）。用于验证遮罩导出未把视频流中途截断。
    fn video_stream_duration(path: &str) -> f64 {
        let value = run_ffprobe_json(&[
            "-v".to_string(),
            "quiet".to_string(),
            "-select_streams".to_string(),
            "v:0".to_string(),
            "-show_entries".to_string(),
            "stream=duration".to_string(),
            "-print_format".to_string(),
            "json".to_string(),
            path.to_string(),
        ])
        .expect("probe video stream duration");
        value
            .get("streams")
            .and_then(|item| item.as_array())
            .and_then(|items| items.first())
            .and_then(|stream| stream.get("duration"))
            .and_then(|item| item.as_str())
            .and_then(|text| text.parse::<f64>().ok())
            .unwrap_or(0.0)
    }

    fn test_segment(mask: &Path, id: &str, start: f64, end: f64) -> VideoMaskSegmentPayload {
        VideoMaskSegmentPayload {
            id: id.to_string(),
            image_path: mask.to_string_lossy().into_owned(),
            start_time: start,
            end_time: end,
            x: 24.0,
            y: 18.0,
            width: 90.0,
            height: 54.0,
            crop_left: 0.0,
            crop_top: 0.0,
            crop_right: 0.0,
            crop_bottom: 0.0,
            opacity: 0.85,
            enabled: true,
        }
    }

    fn test_season_episode(aid: i64, published_at: Option<i64>, sort: i64) -> SeasonEpisodeBackup {
        SeasonEpisodeBackup {
            episode_id: None,
            title: format!("episode-{}", aid),
            aid,
            cid: aid * 10,
            bvid: None,
            archive_title: None,
            video_title: None,
            sort,
            charging_pay: 0,
            member_first: 0,
            limited_free: false,
            published_at,
        }
    }

    fn test_season_section(episodes: Vec<SeasonEpisodeBackup>) -> SeasonSectionBackup {
        SeasonSectionBackup {
            section_id: 1,
            section_type: 0,
            title: "section".to_string(),
            order: 1,
            cover: String::new(),
            state: 0,
            part_state: 0,
            ep_count: episodes.len() as i64,
            episodes,
        }
    }

    #[test]
    fn toolbox_video_mask_probe_defers_keyframe_scan() {
        let dir = test_dir();
        let (source, _) = make_test_source(&dir, 8);

        let probe = probe_video_mask_source(&source.to_string_lossy()).expect("probe source");
        let keyframes = probe_keyframes(&source.to_string_lossy()).expect("probe keyframes");

        assert!(probe.duration > 0.0);
        assert!(probe.keyframes.is_empty());
        assert!(!keyframes.is_empty());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn toolbox_video_mask_plan_splits_copy_and_encode_parts() {
        let dir = test_dir();
        let (source, mask) = make_test_source(&dir, 8);
        let probe = probe_video_mask_source(&source.to_string_lossy()).expect("probe source");
        let keyframes = probe_keyframes(&source.to_string_lossy()).expect("probe keyframes");
        let segments = vec![
            test_segment(&mask, "a", 1.2, 1.5),
            test_segment(&mask, "b", 4.2, 4.5),
            test_segment(&mask, "c", 6.2, 6.5),
        ];
        let plan = build_video_mask_render_plan(probe.duration, &keyframes, &segments)
            .expect("build plan");
        let copy_parts = plan.parts.iter().filter(|part| part.kind == "copy").count();
        let encode_parts = plan
            .parts
            .iter()
            .filter(|part| part.kind == "encode")
            .count();

        assert_eq!(copy_parts, 4);
        assert_eq!(encode_parts, 3);
        assert!(plan.copy_duration > 0.0);
        assert!(plan.encode_duration > 0.0);
        assert!(plan.encode_duration < probe.duration);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn toolbox_video_mask_compensates_copy_reorder_delay() {
        let duration = 3770.031812;

        assert!(
            (compensated_copy_duration(duration, 60.0, 3, true) - 3769.981812).abs() < 0.000001
        );
        assert_eq!(compensated_copy_duration(duration, 60.0, 0, true), duration);
        assert_eq!(
            compensated_copy_duration(duration, 60.0, 3, false),
            duration
        );
    }

    #[test]
    fn toolbox_video_mask_extracts_keyframe_thumbnails() {
        let dir = test_dir();
        let (source, _) = make_test_source(&dir, 8);
        let keyframes = probe_keyframes(&source.to_string_lossy()).expect("probe keyframes");
        let output_dir = dir.join("thumbs");
        fs::create_dir_all(&output_dir).expect("create thumbnail dir");

        let thumbnails =
            extract_video_mask_thumbnails(&source.to_string_lossy(), &keyframes, &output_dir, 4)
                .expect("extract thumbnails");

        assert_eq!(thumbnails.len(), 4);
        for item in thumbnails {
            assert!(item.time >= 0.0);
            assert!(Path::new(&item.path).is_file());
            assert!(item.data_url.starts_with("data:image/jpeg;base64,"));
            assert!(fs::metadata(item.path).expect("thumbnail metadata").len() > 0);
        }

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn toolbox_video_mask_extracts_preview_frame_data_url() {
        let dir = test_dir();
        let (source, _) = make_test_source(&dir, 8);
        let output_dir = dir.join("preview");
        fs::create_dir_all(&output_dir).expect("create preview dir");

        let frame =
            extract_video_mask_preview_frame(&source.to_string_lossy(), 2.0, 360, &output_dir)
                .expect("extract preview frame");

        assert_eq!(frame.time, 2.0);
        assert!(Path::new(&frame.path).is_file());
        assert!(frame.data_url.starts_with("data:image/jpeg;base64,"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn bilibili_season_restore_sorts_episodes_by_publish_time() {
        let episodes = vec![
            test_season_episode(3, Some(300), 1),
            test_season_episode(1, Some(100), 2),
            test_season_episode(2, Some(200), 3),
        ];
        let publish_times = HashMap::new();
        let mut asc_section = test_season_section(episodes.clone());
        let mut desc_section = test_season_section(episodes);

        sort_section_episodes_by_publish_time(&mut asc_section, &publish_times, "publish_asc");
        sort_section_episodes_by_publish_time(&mut desc_section, &publish_times, "publish_desc");

        assert_eq!(
            asc_section
                .episodes
                .iter()
                .map(|item| item.aid)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(
            desc_section
                .episodes
                .iter()
                .map(|item| item.aid)
                .collect::<Vec<_>>(),
            vec![3, 2, 1]
        );
    }

    #[test]
    fn bilibili_season_backup_extracts_archive_publish_time() {
        let item = json!({
            "Archive": {
                "aid": 1001,
                "pubdate": 1784460000
            }
        });

        assert_eq!(archive_aid_from_audit_item(&item), Some(1001));
        assert_eq!(
            archive_publish_time_from_audit_item(&item),
            Some(1784460000)
        );
    }

    #[test]
    fn bilibili_season_backup_fills_publish_times_before_restore() {
        let mut sections = vec![test_season_section(vec![
            test_season_episode(10, None, 1),
            test_season_episode(20, Some(2000), 2),
            test_season_episode(30, None, 3),
        ])];
        let publish_times = HashMap::from([(10, 1000), (30, 3000)]);

        let filled_count = fill_section_episode_publish_times(&mut sections, &publish_times);

        assert_eq!(filled_count, 2);
        assert_eq!(sections[0].episodes[0].published_at, Some(1000));
        assert_eq!(sections[0].episodes[1].published_at, Some(2000));
        assert_eq!(sections[0].episodes[2].published_at, Some(3000));
    }

    #[test]
    fn bilibili_season_restore_keeps_missing_publish_time_at_end() {
        let episodes = vec![
            test_season_episode(1, None, 1),
            test_season_episode(2, Some(200), 2),
            test_season_episode(3, Some(100), 3),
            test_season_episode(4, None, 4),
        ];
        let publish_times = HashMap::new();
        let mut section = test_season_section(episodes);

        let missing_count =
            sort_section_episodes_by_publish_time(&mut section, &publish_times, "publish_asc");

        assert_eq!(missing_count, 2);
        assert_eq!(
            section
                .episodes
                .iter()
                .map(|item| item.aid)
                .collect::<Vec<_>>(),
            vec![3, 2, 1, 4]
        );
    }

    #[test]
    fn toolbox_video_mask_render_outputs_readable_video() {
        let dir = test_dir();
        let (source, mask) = make_test_source(&dir, 8);
        let target = dir.join("masked.mp4");
        let probe = probe_video_mask_source(&source.to_string_lossy()).expect("probe source");
        let segments = vec![
            test_segment(&mask, "a", 1.2, 1.5),
            test_segment(&mask, "b", 4.2, 4.5),
            test_segment(&mask, "c", 6.2, 6.5),
        ];

        let keyframes = probe_keyframes(&source.to_string_lossy()).expect("probe keyframes");
        let result = render_video_mask_segments(VideoMaskRenderPayload {
            render_id: String::new(),
            source_path: source.to_string_lossy().into_owned(),
            target_path: target.to_string_lossy().into_owned(),
            duration: probe.duration,
            width: probe.width,
            height: probe.height,
            fps: probe.fps,
            video_codec: probe.video_codec.clone(),
            color_space: probe.color_space.clone(),
            color_transfer: probe.color_transfer.clone(),
            color_primaries: probe.color_primaries.clone(),
            segments,
            crf: Some(24),
            preset: Some("ultrafast".to_string()),
            keyframes,
        })
        .expect("render masked video");

        let output_probe = probe_video_mask_source(&result.output_path).expect("probe output");
        assert!(Path::new(&result.output_path).is_file());
        assert!(result.output_size > 0);
        assert_eq!(output_probe.width, probe.width);
        assert_eq!(output_probe.height, probe.height);
        assert!((output_probe.duration - probe.duration).abs() < 0.5);
        assert!(output_probe.audio_streams >= 1);
        assert!(result.encode_duration > 0.0);
        assert!(result.copy_duration > 0.0);

        // 视频流本身不能被中途截断：输出视频流时长须与源视频流时长基本一致。
        // 时间基不一致导致的 Non-monotonic DTS 截断只体现在视频流时长上，format 时长会被完整音频掩盖。
        let source_video_duration = video_stream_duration(&source.to_string_lossy());
        let output_video_duration = video_stream_duration(&result.output_path);
        assert!(
            (output_video_duration - source_video_duration).abs() < 0.5,
            "视频流被截断: source={} output={}",
            source_video_duration,
            output_video_duration
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn toolbox_video_mask_encode_part_forces_source_timescale() {
        let dir = test_dir();
        let (source, mask) = make_test_source(&dir, 6);
        let source_path = source.to_string_lossy().into_owned();
        let timescale = probe_video_timescale(&source_path);
        assert_eq!(timescale, 16000, "源视频 timescale 应为固定的 16000");

        let payload = VideoMaskRenderPayload {
            render_id: String::new(),
            source_path: source_path.clone(),
            target_path: dir.join("masked.mp4").to_string_lossy().into_owned(),
            duration: 6.0,
            width: 320,
            height: 180,
            fps: 30.0,
            video_codec: "h264".to_string(),
            color_space: String::new(),
            color_transfer: String::new(),
            color_primaries: String::new(),
            segments: vec![test_segment(&mask, "a", 1.2, 1.5)],
            crf: Some(24),
            preset: Some("ultrafast".to_string()),
            keyframes: Vec::new(),
        };
        let part = VideoMaskRenderPart {
            kind: "encode".to_string(),
            start_time: 1.0,
            end_time: 2.0,
            duration: 1.0,
            segments: vec![test_segment(&mask, "a", 1.2, 1.5)],
        };
        let args = encode_part_args(
            &payload,
            &part,
            &dir.join("part.mp4"),
            320,
            180,
            30.0,
            "h264",
            "",
            "",
            "",
            timescale,
        );
        let idx = args
            .iter()
            .position(|item| item == "-video_track_timescale")
            .expect("encode 分段必须携带 -video_track_timescale");
        assert_eq!(args.get(idx + 1).map(String::as_str), Some("16000"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn toolbox_video_mask_render_keeps_duration_with_long_gop() {
        let dir = test_dir();
        let (source, mask) = make_test_source_with_gop(&dir, 12, 150);
        let target = dir.join("masked_long_gop.mp4");
        let probe = probe_video_mask_source(&source.to_string_lossy()).expect("probe source");
        let keyframes = probe_keyframes(&source.to_string_lossy()).expect("probe keyframes");
        let segments = vec![test_segment(&mask, "long-gop", 6.2, 6.5)];

        let result = render_video_mask_segments(VideoMaskRenderPayload {
            render_id: String::new(),
            source_path: source.to_string_lossy().into_owned(),
            target_path: target.to_string_lossy().into_owned(),
            duration: probe.duration,
            width: probe.width,
            height: probe.height,
            fps: probe.fps,
            video_codec: probe.video_codec.clone(),
            color_space: probe.color_space.clone(),
            color_transfer: probe.color_transfer.clone(),
            color_primaries: probe.color_primaries.clone(),
            segments,
            crf: Some(24),
            preset: Some("ultrafast".to_string()),
            keyframes,
        })
        .expect("render masked video");

        let output_probe = probe_video_mask_source(&result.output_path).expect("probe output");
        assert!((output_probe.duration - probe.duration).abs() < 0.5);
        assert!(result.encode_duration >= 5.0);
        assert!(result.copy_duration > 0.0);

        let source_video_duration = video_stream_duration(&source.to_string_lossy());
        let output_video_duration = video_stream_duration(&result.output_path);
        assert!(
            (output_video_duration - source_video_duration).abs() < 0.5,
            "视频流被截断: source={} output={}",
            source_video_duration,
            output_video_duration
        );

        let _ = fs::remove_dir_all(dir);
    }
}
