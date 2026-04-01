use std::collections::{hash_map::DefaultHasher, HashMap};
use std::fs::{File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};
use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use futures_util::{SinkExt, StreamExt};
use reqwest::blocking::Client;
use reqwest::header::{
    HeaderValue, ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, REFERER,
    USER_AGENT,
};
use rusqlite::OptionalExtension;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;
use url::Url;

use crate::baidu_sync;
use crate::bilibili::client::BilibiliClient;
use crate::commands::settings::{
    load_download_settings_from_db, load_live_settings_from_db, LiveSettings,
};
use crate::config::{default_download_dir, resolve_ffmpeg_path};
use crate::db::Db;
use crate::ffmpeg::run_ffmpeg;
use crate::ffmpeg::run_ffprobe_json;
use crate::login_store::{AuthInfo, LoginStore};
use crate::utils::{append_log, apply_no_window, now_rfc3339, sanitize_filename};

pub struct LiveRuntime {
    records: Mutex<HashMap<String, LiveRecordHandle>>,
}

pub struct LiveRecordHandle {
    pub stop_flag: Arc<AtomicBool>,
    pub split_flag: Arc<AtomicBool>,
    pub title_split_flag: Arc<AtomicBool>,
    pub last_title: Arc<Mutex<String>>,
    pub current_file: Arc<Mutex<String>>,
    pub start_time: String,
    pub start_date: String,
}

pub struct LiveRecordInfo {
    pub file_path: String,
    pub start_time: String,
}

#[derive(Clone)]
pub struct LiveContext {
    pub db: Arc<Db>,
    pub bilibili: Arc<BilibiliClient>,
    pub login_store: Arc<LoginStore>,
    pub app_log_path: Arc<PathBuf>,
    pub live_runtime: Arc<LiveRuntime>,
}

#[derive(Clone)]
pub struct LiveRoomInfo {
    pub room_id: String,
    pub uid: String,
    pub live_status: i64,
    pub title: String,
    pub cover: Option<String>,
    #[allow(dead_code)]
    pub area_name: Option<String>,
    #[allow(dead_code)]
    pub parent_area_name: Option<String>,
}

const INVALID_STREAM_TAG_LIMIT: usize = 300;
const INVALID_STREAM_STALL_SECS: u64 = 10;
const STREAM_URL_REFRESH_LEAD_SECS: u64 = 30;
const MISSING_SEGMENT_WINDOW_SECS: u64 = 60;
const TIMESTAMP_JUMP_THRESHOLD_MS: i64 = 500;
const TIMESTAMP_JUMP_DISCONNECT_THRESHOLD_MS: i64 = 30_000;
const TIMESTAMP_AUDIO_FALLBACK_MS: i64 = 22;
const TIMESTAMP_AUDIO_MIN_STEP_MS: i64 = 20;
const TIMESTAMP_AUDIO_MAX_STEP_MS: i64 = 24;
const TIMESTAMP_VIDEO_FALLBACK_MS: i64 = 33;
const TIMESTAMP_VIDEO_MIN_STEP_MS: i64 = 15;
const TIMESTAMP_VIDEO_MAX_STEP_MS: i64 = 50;
const TIMESTAMP_MIN_STEP_MS: i64 = 1;
const REMUX_SUSPECT_SOURCE_MIN_BYTES: u64 = 100 * 1024 * 1024;
const REMUX_SUSPECT_RATIO_NUM: u64 = 1;
const REMUX_SUSPECT_RATIO_DEN: u64 = 10;
const REMUX_MIN_VALID_OUTPUT_BYTES: u64 = 2 * 1024 * 1024;

pub fn new_live_runtime() -> LiveRuntime {
    LiveRuntime {
        records: Mutex::new(HashMap::new()),
    }
}

impl LiveRuntime {
    pub fn is_recording(&self, room_id: &str) -> bool {
        self.records
            .lock()
            .map(|map| map.contains_key(room_id))
            .unwrap_or(false)
    }

    pub fn get_record_info(&self, room_id: &str) -> Option<LiveRecordInfo> {
        let map = self.records.lock().ok()?;
        let handle = map.get(room_id)?;
        let file_path = handle.current_file.lock().ok()?.clone();
        Some(LiveRecordInfo {
            file_path,
            start_time: handle.start_time.clone(),
        })
    }

    #[allow(dead_code)]
    pub fn mark_split(&self, room_id: &str) {
        if let Ok(map) = self.records.lock() {
            if let Some(handle) = map.get(room_id) {
                handle.split_flag.store(true, Ordering::SeqCst);
            }
        }
    }

    pub fn stop(&self, room_id: &str) {
        if let Ok(map) = self.records.lock() {
            if let Some(handle) = map.get(room_id) {
                handle.stop_flag.store(true, Ordering::SeqCst);
            }
        }
    }
}

const STALE_RECORD_REMUX_MAX_AGE_SECS: u64 = 36 * 60 * 60;
const STALE_RECORD_IDLE_SECS: u64 = 30 * 60;
const STALE_RECORD_RECOVERY_INTERVAL_SECS: u64 = 10 * 60;

pub fn recover_stale_recordings(context: LiveContext) {
    let records = context
        .db
        .with_conn(|conn| {
            let mut stmt = conn
                .prepare("SELECT id, file_path FROM live_record_task WHERE status = 'RECORDING'")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?;
            Ok(rows.collect::<Result<Vec<(i64, String)>, _>>()?)
        })
        .unwrap_or_default();

    if records.is_empty() {
        append_log(&context.app_log_path, "record_recover_stale none");
        return;
    }

    append_log(
        &context.app_log_path,
        &format!("record_recover_stale start count={}", records.len()),
    );

    let mut remux_targets = Vec::new();
    for (record_id, file_path) in records {
        let path = PathBuf::from(&file_path);
        let file_meta = match std::fs::metadata(&path) {
            Ok(meta) => meta,
            Err(_) => {
                let _ = update_record_task(
                    &context.db,
                    record_id,
                    "FAILED",
                    Some(now_rfc3339()),
                    0,
                    Some("录制恢复失败: 文件缺失"),
                );
                append_log(
                    &context.app_log_path,
                    &format!(
                        "record_recover_missing record_id={} path={}",
                        record_id, file_path
                    ),
                );
                continue;
            }
        };

        let file_size = file_meta.len();
        let fallback_end_time = now_rfc3339();
        let metadata_path = path.with_extension("metadata.json");
        let end_time = derive_recovered_record_end_time(
            &path,
            metadata_path.to_string_lossy().as_ref(),
            &fallback_end_time,
        );
        let (status, error_message) = if file_size == 0 {
            ("FAILED", Some("录制恢复失败: 空文件"))
        } else {
            ("STOPPED", None)
        };

        if let Err(err) = update_record_task(
            &context.db,
            record_id,
            status,
            Some(end_time.clone()),
            file_size,
            error_message,
        ) {
            append_log(
                &context.app_log_path,
                &format!(
                    "record_recover_update_fail record_id={} err={}",
                    record_id, err
                ),
            );
            continue;
        }

        if metadata_path.exists() {
            if let Err(err) = update_metadata_file(
                metadata_path.to_string_lossy().as_ref(),
                &end_time,
                file_size,
            ) {
                append_log(
                    &context.app_log_path,
                    &format!(
                        "record_metadata_update_failed record_id={} err={}",
                        record_id, err
                    ),
                );
            }
        }

        let mp4_path = path.with_extension("mp4");
        if mp4_path.exists() {
            let mp4_size = std::fs::metadata(&mp4_path)
                .map(|meta| meta.len())
                .unwrap_or(0);
            let mp4_path_str = mp4_path.to_string_lossy().to_string();
            if let Err(err) =
                update_record_task_file_path(&context.db, record_id, &mp4_path_str, mp4_size)
            {
                append_log(
                    &context.app_log_path,
                    &format!(
                        "record_recover_mp4_update_fail record_id={} err={}",
                        record_id, err
                    ),
                );
            }
            continue;
        }

        if status == "FAILED" {
            continue;
        }

        let should_remux = file_meta
            .modified()
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .map(|age| age <= Duration::from_secs(STALE_RECORD_REMUX_MAX_AGE_SECS))
            .unwrap_or(false);
        if should_remux {
            remux_targets.push((record_id, file_path));
        }
    }

    if remux_targets.is_empty() {
        append_log(&context.app_log_path, "record_recover_stale remux=none");
        return;
    }

    append_log(
        &context.app_log_path,
        &format!("record_recover_stale remux={}", remux_targets.len()),
    );
    for (record_id, file_path) in remux_targets {
        spawn_segment_remux(context.clone(), record_id, file_path);
    }
}

async fn recover_idle_recordings(context: LiveContext) {
    let records = context
        .db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, room_id, file_path FROM live_record_task WHERE status = 'RECORDING'",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            Ok(rows.collect::<Result<Vec<(i64, String, String)>, _>>()?)
        })
        .unwrap_or_default();

    if records.is_empty() {
        append_log(&context.app_log_path, "record_recover_idle none");
        return;
    }

    append_log(
        &context.app_log_path,
        &format!("record_recover_idle start count={}", records.len()),
    );

    let mut live_status_cache: HashMap<String, i64> = HashMap::new();
    let mut remux_targets = Vec::new();

    for (record_id, room_id, file_path) in records {
        if let Some(info) = context.live_runtime.get_record_info(&room_id) {
            if info.file_path == file_path {
                continue;
            }
        }

        let path = PathBuf::from(&file_path);
        let file_meta = match std::fs::metadata(&path) {
            Ok(meta) => meta,
            Err(_) => {
                let _ = update_record_task(
                    &context.db,
                    record_id,
                    "FAILED",
                    Some(now_rfc3339()),
                    0,
                    Some("录制恢复失败: 文件缺失"),
                );
                append_log(
                    &context.app_log_path,
                    &format!(
                        "record_recover_missing record_id={} path={}",
                        record_id, file_path
                    ),
                );
                continue;
            }
        };

        let file_size = file_meta.len();
        let idle_secs = file_meta
            .modified()
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .map(|age| age.as_secs())
            .unwrap_or(0);
        if idle_secs < STALE_RECORD_IDLE_SECS {
            continue;
        }

        let live_status = if let Some(status) = live_status_cache.get(&room_id) {
            *status
        } else {
            match fetch_room_info(&context.bilibili, &room_id).await {
                Ok(info) => {
                    live_status_cache.insert(room_id.clone(), info.live_status);
                    info.live_status
                }
                Err(err) => {
                    append_log(
                        &context.app_log_path,
                        &format!(
                            "record_recover_live_status_fail room={} err={}",
                            room_id, err
                        ),
                    );
                    continue;
                }
            }
        };

        let fallback_end_time = now_rfc3339();
        let metadata_path = path.with_extension("metadata.json");
        let end_time = derive_recovered_record_end_time(
            &path,
            metadata_path.to_string_lossy().as_ref(),
            &fallback_end_time,
        );
        let (status, error_message) = if file_size == 0 {
            ("FAILED", Some("录制恢复失败: 空文件"))
        } else if live_status == 1 {
            ("FAILED", Some("录制失活: 长时间无写入"))
        } else {
            ("STOPPED", None)
        };

        if let Err(err) = update_record_task(
            &context.db,
            record_id,
            status,
            Some(end_time.clone()),
            file_size,
            error_message,
        ) {
            append_log(
                &context.app_log_path,
                &format!(
                    "record_recover_update_fail record_id={} err={}",
                    record_id, err
                ),
            );
            continue;
        }

        if metadata_path.exists() {
            if let Err(err) = update_metadata_file(
                metadata_path.to_string_lossy().as_ref(),
                &end_time,
                file_size,
            ) {
                append_log(
                    &context.app_log_path,
                    &format!(
                        "record_metadata_update_failed record_id={} err={}",
                        record_id, err
                    ),
                );
            }
        }

        let mp4_path = path.with_extension("mp4");
        if mp4_path.exists() {
            let mp4_size = std::fs::metadata(&mp4_path)
                .map(|meta| meta.len())
                .unwrap_or(0);
            let mp4_path_str = mp4_path.to_string_lossy().to_string();
            if let Err(err) =
                update_record_task_file_path(&context.db, record_id, &mp4_path_str, mp4_size)
            {
                append_log(
                    &context.app_log_path,
                    &format!(
                        "record_recover_mp4_update_fail record_id={} err={}",
                        record_id, err
                    ),
                );
            }
            continue;
        }

        if file_size == 0 {
            continue;
        }

        let should_remux = file_meta
            .modified()
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .map(|age| age <= Duration::from_secs(STALE_RECORD_REMUX_MAX_AGE_SECS))
            .unwrap_or(false);
        if should_remux {
            remux_targets.push((record_id, file_path));
        }
    }

    if remux_targets.is_empty() {
        append_log(&context.app_log_path, "record_recover_idle remux=none");
        return;
    }

    append_log(
        &context.app_log_path,
        &format!("record_recover_idle remux={}", remux_targets.len()),
    );
    for (record_id, file_path) in remux_targets {
        spawn_segment_remux(context.clone(), record_id, file_path);
    }
}

pub fn start_record_recovery_loop(context: LiveContext) {
    tauri::async_runtime::spawn(async move {
        loop {
            recover_idle_recordings(context.clone()).await;
            tokio::time::sleep(Duration::from_secs(STALE_RECORD_RECOVERY_INTERVAL_SECS)).await;
        }
    });
}

pub fn start_auto_record_loop(context: LiveContext) {
    tauri::async_runtime::spawn(async move {
        loop {
            let settings = load_live_settings_from_db(&context.db)
                .unwrap_or_else(|_| crate::commands::settings::default_live_settings());
            let interval_sec = settings.check_interval_sec.max(10);
            if let Ok(rooms) = load_anchor_room_ids(&context.db) {
                for room_id in rooms {
                    match fetch_room_info(&context.bilibili, &room_id).await {
                        Ok(info) => {
                            let _ = update_anchor_status(&context.db, &room_id, info.live_status);
                            let auto_record =
                                load_room_auto_record(&context.db, &room_id).unwrap_or(true);
                            let recording = context.live_runtime.is_recording(&room_id);
                            if info.live_status == 1 && auto_record && !recording {
                                match start_recording(
                                    context.clone(),
                                    &room_id,
                                    info.clone(),
                                    settings.clone(),
                                ) {
                                    Ok(()) => {
                                        append_log(
                                            &context.app_log_path,
                                            &format!("auto_record_start room={}", room_id),
                                        );
                                    }
                                    Err(err) => {
                                        append_log(
                                            &context.app_log_path,
                                            &format!(
                                                "auto_record_start_failed room={} err={}",
                                                room_id, err
                                            ),
                                        );
                                    }
                                }
                            } else if info.live_status != 1 && recording {
                                stop_recording(context.clone(), &room_id, "直播结束自动停止");
                            }
                            if recording && settings.cutting_by_title {
                                if let Ok(mut map) = context.live_runtime.records.lock() {
                                    if let Some(handle) = map.get_mut(&room_id) {
                                        let mut last_title = handle
                                            .last_title
                                            .lock()
                                            .unwrap_or_else(|e| e.into_inner());
                                        if *last_title != info.title {
                                            *last_title = info.title.clone();
                                            handle.title_split_flag.store(true, Ordering::SeqCst);
                                        }
                                    }
                                }
                            }
                        }
                        Err(err) => {
                            append_log(
                                &context.app_log_path,
                                &format!("live_check_error room={} err={}", room_id, err),
                            );
                        }
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(interval_sec as u64)).await;
        }
    });
}

pub fn start_recording(
    context: LiveContext,
    room_id: &str,
    room_info: LiveRoomInfo,
    settings: LiveSettings,
) -> Result<(), String> {
    if context.live_runtime.is_recording(room_id) {
        return Ok(());
    }

    if room_info.live_status != 1 {
        return Err("当前未开播".to_string());
    }

    let nickname = load_anchor_nickname(&context.db, room_id).ok().flatten();
    let stop_flag = Arc::new(AtomicBool::new(false));
    let split_flag = Arc::new(AtomicBool::new(false));
    let title_split_flag = Arc::new(AtomicBool::new(false));
    let current_title = room_info.title.clone();
    let start_time = Utc::now();
    let handle = LiveRecordHandle {
        stop_flag: Arc::clone(&stop_flag),
        split_flag: Arc::clone(&split_flag),
        title_split_flag: Arc::clone(&title_split_flag),
        last_title: Arc::new(Mutex::new(current_title)),
        current_file: Arc::new(Mutex::new(String::new())),
        start_time: start_time.to_rfc3339(),
        start_date: start_time.format("%Y%m%d").to_string(),
    };

    if let Ok(mut map) = context.live_runtime.records.lock() {
        map.insert(room_id.to_string(), handle);
    }

    let runtime = Arc::clone(&context.live_runtime);
    let room_id_owned = room_id.to_string();
    tauri::async_runtime::spawn_blocking(move || {
        let mut retry_count = 0;
        let mut current_room_info = room_info;
        loop {
            let started_at = Instant::now();
            let result = run_record_loop(
                context.clone(),
                room_id_owned.clone(),
                current_room_info.clone(),
                nickname.clone(),
                settings.clone(),
            );
            if let Err(err) = result {
                append_log(
                    &context.app_log_path,
                    &format!("record_loop_error room={} err={}", room_id_owned, err),
                );
            } else {
                break;
            }

            if stop_flag.load(Ordering::SeqCst) {
                break;
            }

            if started_at.elapsed().as_secs() < 60 {
                append_log(
                    &context.app_log_path,
                    &format!(
                        "record_retry_skip room={} reason=short_session",
                        room_id_owned
                    ),
                );
                break;
            }

            if retry_count >= 10 {
                append_log(
                    &context.app_log_path,
                    &format!(
                        "record_retry_skip room={} reason=retry_limit",
                        room_id_owned
                    ),
                );
                break;
            }

            let next_info =
                tauri::async_runtime::block_on(fetch_room_info(&context.bilibili, &room_id_owned));
            let next_info = match next_info {
                Ok(info) if info.live_status == 1 => info,
                Ok(_) => {
                    append_log(
                        &context.app_log_path,
                        &format!("record_retry_skip room={} reason=not_living", room_id_owned),
                    );
                    break;
                }
                Err(err) => {
                    append_log(
                        &context.app_log_path,
                        &format!(
                            "record_retry_skip room={} reason=live_info_err err={}",
                            room_id_owned, err
                        ),
                    );
                    break;
                }
            };

            retry_count += 1;
            append_log(
                &context.app_log_path,
                &format!(
                    "record_retry_start room={} retry={}",
                    room_id_owned, retry_count
                ),
            );
            if let Ok(mut map) = runtime.records.lock() {
                if let Some(handle) = map.get_mut(&room_id_owned) {
                    let mut last_title =
                        handle.last_title.lock().unwrap_or_else(|e| e.into_inner());
                    *last_title = next_info.title.clone();
                }
            }
            current_room_info = next_info;
        }
        if let Ok(mut map) = runtime.records.lock() {
            map.remove(&room_id_owned);
        }
    });

    Ok(())
}

pub fn stop_recording(context: LiveContext, room_id: &str, reason: &str) {
    append_log(
        &context.app_log_path,
        &format!("record_stop room={} reason={}", room_id, reason),
    );
    context.live_runtime.stop(room_id);
}

fn run_record_loop(
    context: LiveContext,
    room_id: String,
    room_info: LiveRoomInfo,
    nickname: Option<String>,
    settings: LiveSettings,
) -> Result<(), String> {
    let enable_timestamp_fix =
        settings.flv_fix_adjust_timestamp_jump || settings.flv_fix_split_on_timestamp_jump;
    let prefer_split_reconnect = settings.record_mode == 0;
    let apply_timestamp_fix = enable_timestamp_fix;
    append_log(
    &context.app_log_path,
    &format!(
      "record_settings room={} record_mode={} fix_enabled={} fix_adjust={} fix_split={} fix_split_missing={} fix_disable_annexb={} strict_split_reconnect={} jump_disconnect_threshold_ms={}",
      room_id,
      settings.record_mode,
      enable_timestamp_fix,
      settings.flv_fix_adjust_timestamp_jump,
      settings.flv_fix_split_on_timestamp_jump,
      settings.flv_fix_split_on_missing,
      settings.flv_fix_disable_on_annexb,
      prefer_split_reconnect,
      TIMESTAMP_JUMP_DISCONNECT_THRESHOLD_MS
    ),
  );
    let base_dir = if settings.record_path.trim().is_empty() {
        let download_dir = load_download_settings_from_db(&context.db)
            .map(|settings| settings.download_path)
            .unwrap_or_else(|_| default_download_dir().to_string_lossy().to_string());
        PathBuf::from(download_dir).join("live_recordings")
    } else {
        PathBuf::from(settings.record_path.trim())
    };
    let _ = std::fs::create_dir_all(&base_dir);

    let stop_flag = {
        let map = context
            .live_runtime
            .records
            .lock()
            .map_err(|_| "Lock error")?;
        map.get(&room_id)
            .map(|handle| Arc::clone(&handle.stop_flag))
            .ok_or_else(|| "Record handle missing".to_string())?
    };
    let split_flag = {
        let map = context
            .live_runtime
            .records
            .lock()
            .map_err(|_| "Lock error")?;
        map.get(&room_id)
            .map(|handle| Arc::clone(&handle.split_flag))
            .ok_or_else(|| "Record handle missing".to_string())?
    };
    let title_split_flag = {
        let map = context
            .live_runtime
            .records
            .lock()
            .map_err(|_| "Lock error")?;
        map.get(&room_id)
            .map(|handle| Arc::clone(&handle.title_split_flag))
            .ok_or_else(|| "Record handle missing".to_string())?
    };

    let mut segment_index = 1;
    let mut current_title = room_info.title.clone();
    let record_start_date = load_record_start_date(&context, &room_id);
    let mut current_file_path = build_record_path(
        &settings.file_name_template,
        &base_dir,
        &room_info,
        nickname.as_deref(),
        &record_start_date,
        segment_index,
    );
    update_current_file(&context, &room_id, &current_file_path);
    let mut segment: Option<SegmentWriter> = None;
    let mut segment_start = Instant::now();
    let mut pending_split = false;
    let mut pending_title: Option<String> = None;
    let mut missing_started_at: Option<Instant> = None;
    let title_split_min = settings.title_split_min_seconds.max(0) as u64;

    if settings.save_cover {
        if let Some(cover) = room_info.cover.as_ref() {
            let _ = download_cover(&current_file_path, cover);
        }
    }

    if should_record_danmaku(&settings) {
        let danmaku_settings = settings.clone();
        let danmaku_context = context.clone();
        let runtime_room = room_id.clone();
        let danmaku_room = room_info.room_id.clone();
        let danmaku_file = current_file_path.clone();
        let danmaku_stop = Arc::clone(&stop_flag);
        tauri::async_runtime::spawn(async move {
            let _ = run_danmaku_loop(
                danmaku_context,
                runtime_room,
                danmaku_room,
                danmaku_file,
                danmaku_settings,
                danmaku_stop,
            )
            .await;
        });
    }

    let client = Client::builder()
        .connect_timeout(Duration::from_millis(
            settings.stream_connect_timeout_ms.max(1000) as u64,
        ))
        .build()
        .map_err(|err| format!("Failed to build client: {}", err))?;
    let auth = context
        .login_store
        .load_primary_auth_info(&context.db)
        .ok()
        .flatten();
    let mut stream_urls: Vec<String> = Vec::new();
    let mut stream_url_index: usize = 0;
    let mut force_no_qn_until: Option<i64> = None;
    let mut pipeline = LiveFlvPipeline::new(
        LivePipelineSettings {
            split_on_script_tag: false,
            disable_split_on_h264_annexb: settings.flv_fix_disable_on_annexb,
        },
        enable_timestamp_fix,
        apply_timestamp_fix,
    );
    let mut disconnect_retries: usize = 0;
    macro_rules! rotate_for_reconnect {
        ($reason:expr) => {
            let _ = rotate_segment_for_reconnect(
                &context,
                &room_id,
                &room_info,
                nickname.as_deref(),
                &settings,
                &base_dir,
                &record_start_date,
                &mut current_title,
                &mut current_file_path,
                &mut segment,
                &mut segment_index,
                &mut pending_split,
                &mut pending_title,
                &mut missing_started_at,
                $reason,
            )?;
        };
    }

    loop {
        if stop_flag.load(Ordering::SeqCst) {
            if let Some(mut seg) = segment.take() {
                let record_id = seg.record_id;
                let file_path = seg.file_path.clone();
                seg.finish("STOPPED", None)?;
                drop(seg);
                spawn_segment_remux(context.clone(), record_id, file_path);
            }
            break;
        }

        if stream_urls.is_empty() {
            let now = Utc::now().timestamp();
            let use_quality = match force_no_qn_until {
                Some(until) if now < until => false,
                _ => {
                    force_no_qn_until = None;
                    true
                }
            };
            if !use_quality {
                append_log(
                    &context.app_log_path,
                    &format!("stream_fetch_no_qn room={} reason=forced", room_id),
                );
            }
            stream_urls = match fetch_stream_urls(
                &context.bilibili,
                &room_info.room_id,
                &settings,
                auth.as_ref(),
                use_quality,
            ) {
                Ok(urls) => urls,
                Err(err) => {
                    append_log(
                        &context.app_log_path,
                        &format!("fetch_stream_url_error room={} err={}", room_id, err),
                    );
                    if settings.stream_retry_no_qn_sec > 0 {
                        std::thread::sleep(Duration::from_secs(
                            settings.stream_retry_no_qn_sec.max(1) as u64,
                        ));
                        match fetch_stream_urls(
                            &context.bilibili,
                            &room_info.room_id,
                            &settings,
                            auth.as_ref(),
                            false,
                        ) {
                            Ok(urls) => urls,
                            Err(err) => {
                                append_log(
                                    &context.app_log_path,
                                    &format!(
                                        "fetch_stream_url_fallback_error room={} err={}",
                                        room_id, err
                                    ),
                                );
                                rotate_for_reconnect!("获取流地址失败，重连前切段");
                                std::thread::sleep(Duration::from_millis(
                                    settings.stream_retry_ms.max(1000) as u64,
                                ));
                                continue;
                            }
                        }
                    } else {
                        rotate_for_reconnect!("获取流地址失败，重连前切段");
                        std::thread::sleep(Duration::from_millis(
                            settings.stream_retry_ms.max(1000) as u64,
                        ));
                        continue;
                    }
                }
            };
            stream_url_index = 0;
        }

        let stream_url = match stream_urls.get(stream_url_index) {
            Some(url) => url.clone(),
            None => {
                stream_urls.clear();
                continue;
            }
        };
        if !stream_urls.is_empty() {
            stream_url_index = (stream_url_index + 1) % stream_urls.len();
        }

        if let Some((expire, now)) =
            should_refresh_stream_url(&stream_url, STREAM_URL_REFRESH_LEAD_SECS)
        {
            append_log(
                &context.app_log_path,
                &format!(
                    "stream_url_expired room={} expire={} now={}",
                    room_id, expire, now
                ),
            );
            rotate_for_reconnect!("流地址过期，重连前切段");
            stream_urls.clear();
            std::thread::sleep(Duration::from_millis(
                settings.stream_retry_ms.max(1000) as u64
            ));
            continue;
        }

        if is_hls_url(&stream_url) {
            let hls_file_path = normalize_hls_path(&current_file_path);
            update_current_file(&context, &room_id, &hls_file_path);
            append_log(
                &context.app_log_path,
                &format!(
                    "stream_hls_detected room={} path={}",
                    room_id, hls_file_path
                ),
            );
            if let Err(err) = record_hls_stream(
                &context,
                &room_id,
                &room_info,
                nickname.as_deref(),
                &current_title,
                &hls_file_path,
                segment_index,
                &settings,
                &stop_flag,
                &stream_url,
            ) {
                append_log(
                    &context.app_log_path,
                    &format!("stream_hls_error room={} err={}", room_id, err),
                );
            }
            if stop_flag.load(Ordering::SeqCst) {
                return Ok(());
            }
            stream_urls.clear();
            segment_index += 1;
            current_title = load_current_title(&context, &room_id, &current_title);
            current_file_path = build_record_path(
                &settings.file_name_template,
                &base_dir,
                &room_info,
                nickname.as_deref(),
                &record_start_date,
                segment_index,
            );
            update_current_file(&context, &room_id, &current_file_path);
            std::thread::sleep(Duration::from_millis(
                settings.stream_retry_ms.max(1000) as u64
            ));
            continue;
        }

        append_log(
            &context.app_log_path,
            &format!(
                "stream_url_info room={} {}",
                room_id,
                summarize_stream_url(&stream_url)
            ),
        );
        let referer_value = format!("https://live.bilibili.com/{}", room_info.room_id);
        let mut request = client.get(&stream_url);
        request = request.header(
            USER_AGENT,
            HeaderValue::from_static(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
            ),
        );
        request = request.header(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
        if let Ok(value) = HeaderValue::from_str(&referer_value) {
            request = request.header(REFERER, value);
        }
        if let Some(auth) = auth.as_ref() {
            if let Ok(value) = HeaderValue::from_str(&auth.cookie) {
                request = request.header("Cookie", value);
            }
        }
        let response = request.send();
        let mut response = match response {
            Ok(resp) => resp,
            Err(err) => {
                append_log(
                    &context.app_log_path,
                    &format!("stream_connect_error room={} err={}", room_id, err),
                );
                rotate_for_reconnect!("连接流失败，重连前切段");
                stream_urls.clear();
                std::thread::sleep(Duration::from_millis(
                    settings.stream_retry_ms.max(1000) as u64
                ));
                continue;
            }
        };

        if !response.status().is_success() {
            let content_type = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("-");
            let content_encoding = response
                .headers()
                .get(CONTENT_ENCODING)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("-");
            let content_length = response
                .headers()
                .get(CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("-");
            append_log(
        &context.app_log_path,
        &format!(
          "stream_response_error room={} status={} content_type={} content_encoding={} content_length={}",
          room_id,
          response.status().as_u16(),
          content_type,
          content_encoding,
          content_length
        ),
      );
            mark_force_no_qn(
                &mut force_no_qn_until,
                &settings,
                context.app_log_path.as_ref(),
                room_id.as_str(),
                "response_status",
            );
            rotate_for_reconnect!("响应异常，重连前切段");
            stream_urls.clear();
            std::thread::sleep(Duration::from_millis(
                settings.stream_retry_ms.max(1000) as u64
            ));
            continue;
        }

        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("-");
        let content_encoding = response
            .headers()
            .get(CONTENT_ENCODING)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("-");
        let normalized_type = content_type.to_ascii_lowercase();
        let normalized_encoding = content_encoding.to_ascii_lowercase();
        let has_unexpected_type = normalized_type.starts_with("text/")
            || normalized_type.contains("json")
            || normalized_type.contains("html");
        let has_unexpected_encoding = normalized_encoding != "-"
            && normalized_encoding != "identity"
            && !normalized_encoding.is_empty();
        if has_unexpected_type || has_unexpected_encoding {
            append_log(
                &context.app_log_path,
                &format!(
                    "stream_response_unexpected room={} content_type={} content_encoding={}",
                    room_id, content_type, content_encoding
                ),
            );
            mark_force_no_qn(
                &mut force_no_qn_until,
                &settings,
                context.app_log_path.as_ref(),
                room_id.as_str(),
                "response_unexpected",
            );
            rotate_for_reconnect!("响应格式异常，重连前切段");
            stream_urls.clear();
            std::thread::sleep(Duration::from_millis(
                settings.stream_retry_ms.max(1000) as u64
            ));
            continue;
        }

        let mut buf = vec![0u8; 8192];
        let mut parser = FlvStreamParser::new();
        let mut cache = FlvHeaderCache::new();
        loop {
            if stop_flag.load(Ordering::SeqCst) {
                if let Some(mut seg) = segment.take() {
                    let record_id = seg.record_id;
                    let file_path = seg.file_path.clone();
                    seg.finish("STOPPED", None)?;
                    drop(seg);
                    spawn_segment_remux(context.clone(), record_id, file_path);
                }
                return Ok(());
            }

            match response.read(&mut buf) {
                Ok(0) => {
                    let missing_since = missing_started_at.get_or_insert_with(Instant::now);
                    let missing_elapsed = missing_since.elapsed().as_secs();
                    if missing_elapsed >= MISSING_SEGMENT_WINDOW_SECS {
                        disconnect_retries += 1;
                        append_log(
                            &context.app_log_path,
                            &format!(
                                "stream_read_end_disconnect room={} elapsed={} retry={}",
                                room_id, missing_elapsed, disconnect_retries
                            ),
                        );
                        stream_urls.clear();
                        if disconnect_retries >= 3 {
                            if let Some(mut seg) = segment.take() {
                                let record_id = seg.record_id;
                                let file_path = seg.file_path.clone();
                                seg.finish("FAILED", Some("流断开超过阈值，录制会话终止"))?;
                                drop(seg);
                                spawn_segment_remux(context.clone(), record_id, file_path);
                            }
                            return Err("直播流中断超过阈值".to_string());
                        }
                        rotate_for_reconnect!("流断开超过窗口，重连前切段");
                        std::thread::sleep(Duration::from_millis(
                            settings.stream_retry_ms.max(1000) as u64,
                        ));
                        break;
                    }
                    append_log(
                        &context.app_log_path,
                        &format!(
                            "stream_read_end_rotate_segment room={} elapsed={} window={}",
                            room_id, missing_elapsed, MISSING_SEGMENT_WINDOW_SECS
                        ),
                    );
                    rotate_for_reconnect!("读取结束，重连前切段");
                    stream_urls.clear();
                    std::thread::sleep(Duration::from_millis(
                        settings.stream_retry_ms.max(1000) as u64
                    ));
                    break;
                }
                Ok(n) => {
                    missing_started_at = None;
                    let items = match parser.push(&buf[..n]) {
                        Ok(items) => items,
                        Err(err) => {
                            append_log(
                                &context.app_log_path,
                                &format!("stream_invalid_header room={} err={}", room_id, err),
                            );
                            mark_force_no_qn(
                                &mut force_no_qn_until,
                                &settings,
                                context.app_log_path.as_ref(),
                                room_id.as_str(),
                                "invalid_header",
                            );
                            rotate_for_reconnect!("流头异常，重连前切段");
                            stream_urls.clear();
                            std::thread::sleep(Duration::from_millis(
                                settings.stream_retry_ms.max(1000) as u64,
                            ));
                            break;
                        }
                    };

                    let mut invalid_stream = false;
                    let mut invalid_reason: Option<String> = None;
                    for item in items {
                        match item {
                            FlvParsedItem::Header(header) => {
                                cache.set_header(header.clone());
                                pipeline.on_stream_header();
                            }
                            FlvParsedItem::Tag(mut tag) => {
                                let mut decision = pipeline.process_tag(&mut tag);
                                for line in decision.logs.drain(..) {
                                    append_log(
                                        &context.app_log_path,
                                        &format!("stream_pipeline room={} {}", room_id, line),
                                    );
                                }
                                if let Some(reason) = decision.request_split {
                                    pending_split = true;
                                    append_log(
                                        &context.app_log_path,
                                        &format!(
                                            "stream_pipeline_split room={} reason={}",
                                            room_id, reason
                                        ),
                                    );
                                }
                                if let Some(reason) = decision.disconnect_reason.take() {
                                    append_log(
                                        &context.app_log_path,
                                        &format!(
                                            "stream_pipeline_disconnect room={} reason={}",
                                            room_id, reason
                                        ),
                                    );
                                    mark_force_no_qn(
                                        &mut force_no_qn_until,
                                        &settings,
                                        context.app_log_path.as_ref(),
                                        room_id.as_str(),
                                        "pipeline_disconnect",
                                    );
                                    invalid_reason = Some(reason);
                                    invalid_stream = true;
                                    break;
                                }
                                cache.update_from_tag(&tag);
                                let request_split = split_flag.swap(false, Ordering::SeqCst);
                                let title_split_requested =
                                    title_split_flag.swap(false, Ordering::SeqCst);
                                if title_split_requested {
                                    let latest_title =
                                        load_current_title(&context, &room_id, &current_title);
                                    if latest_title != current_title {
                                        if title_split_min > 0
                                            && segment_start.elapsed().as_secs() < title_split_min
                                        {
                                            pending_title = Some(latest_title);
                                            append_log(
                                                &context.app_log_path,
                                                &format!(
                          "stream_split_defer room={} reason=title_min elapsed={} min={}",
                          room_id,
                          segment_start.elapsed().as_secs(),
                          title_split_min
                        ),
                                            );
                                        } else {
                                            pending_title = Some(latest_title);
                                            pending_split = true;
                                        }
                                    }
                                }
                                if request_split {
                                    if pending_title.is_none() && settings.cutting_by_title {
                                        let latest_title =
                                            load_current_title(&context, &room_id, &current_title);
                                        if latest_title != current_title {
                                            pending_title = Some(latest_title);
                                        }
                                    }
                                    pending_split = true;
                                }
                                if pending_title.is_some() && title_split_min > 0 {
                                    if segment_start.elapsed().as_secs() >= title_split_min {
                                        pending_split = true;
                                    }
                                }

                                if pending_split && is_video_keyframe(&tag) {
                                    current_title = pending_title.take().unwrap_or_else(|| {
                                        load_current_title(&context, &room_id, &current_title)
                                    });
                                    if segment.is_some() {
                                        if cache.has_header() {
                                            if let Some(mut seg) = segment.take() {
                                                let record_id = seg.record_id;
                                                let file_path = seg.file_path.clone();
                                                seg.finish("COMPLETED", Some("分段切换"))?;
                                                drop(seg);
                                                spawn_segment_remux(
                                                    context.clone(),
                                                    record_id,
                                                    file_path,
                                                );
                                            }
                                            segment_index += 1;
                                            current_file_path = build_record_path(
                                                &settings.file_name_template,
                                                &base_dir,
                                                &room_info,
                                                nickname.as_deref(),
                                                &record_start_date,
                                                segment_index,
                                            );
                                            update_current_file(
                                                &context,
                                                &room_id,
                                                &current_file_path,
                                            );
                                        } else {
                                            append_log(
                                                &context.app_log_path,
                                                &format!(
                                                    "stream_split_skip room={} reason=no_header",
                                                    room_id
                                                ),
                                            );
                                        }
                                    }
                                    pending_split = false;
                                }

                                if segment.is_none() {
                                    if !should_open_segment_on_tag(&tag) {
                                        continue;
                                    }
                                    if !cache.has_header() {
                                        append_log(
                                            &context.app_log_path,
                                            &format!(
                                                "stream_split_skip room={} reason=no_header",
                                                room_id
                                            ),
                                        );
                                        continue;
                                    }
                                    let mut new_segment = open_segment(
                                        &context,
                                        &room_id,
                                        &current_file_path,
                                        &current_title,
                                        segment_index,
                                        &settings,
                                        &room_info,
                                        nickname.as_deref(),
                                    )?;
                                    cache.write_preamble(&mut new_segment)?;
                                    pipeline.reset_on_new_segment();
                                    segment_start = Instant::now();
                                    segment = Some(new_segment);
                                }

                                if let Some(seg) = segment.as_mut() {
                                    seg.write(&tag.bytes)?;
                                    if decision.progressed {
                                        disconnect_retries = 0;
                                    }
                                    if settings.cutting_mode == 1 {
                                        let limit = settings.cutting_number.max(1) as u64;
                                        if segment_start.elapsed().as_secs() >= limit {
                                            split_flag.store(true, Ordering::SeqCst);
                                        }
                                    } else if settings.cutting_mode == 2 {
                                        let limit =
                                            settings.cutting_number.max(1) as u64 * 1024 * 1024;
                                        if seg.bytes_written >= limit {
                                            split_flag.store(true, Ordering::SeqCst);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if invalid_stream {
                        disconnect_retries += 1;
                        if let Some(reason) = invalid_reason.as_deref() {
                            append_log(
                                &context.app_log_path,
                                &format!(
                                    "stream_disconnect_retry room={} retry={} reason={}",
                                    room_id, disconnect_retries, reason
                                ),
                            );
                        }
                        let rotate_reason = invalid_reason
                            .as_ref()
                            .map(|reason| format!("流异常（{}），重连前切段", reason))
                            .unwrap_or_else(|| "流异常，重连前切段".to_string());
                        stream_urls.clear();
                        if disconnect_retries >= 3 {
                            if let Some(mut seg) = segment.take() {
                                let record_id = seg.record_id;
                                let file_path = seg.file_path.clone();
                                seg.finish("FAILED", Some("流异常超过阈值，录制会话终止"))?;
                                drop(seg);
                                spawn_segment_remux(context.clone(), record_id, file_path);
                            }
                            return Err("流异常超过阈值".to_string());
                        }
                        rotate_for_reconnect!(&rotate_reason);
                        std::thread::sleep(Duration::from_millis(
                            settings.stream_retry_ms.max(1000) as u64,
                        ));
                        break;
                    }
                }
                Err(err) => {
                    let err_text = err.to_string();
                    let is_timeout = err.kind() == ErrorKind::TimedOut
                        || err_text.to_ascii_lowercase().contains("timed out");
                    if is_timeout {
                        append_log(
                            &context.app_log_path,
                            &format!("stream_read_timeout room={} err={}", room_id, err_text),
                        );
                    } else {
                        append_log(
                            &context.app_log_path,
                            &format!("stream_read_error room={} err={}", room_id, err_text),
                        );
                        mark_force_no_qn(
                            &mut force_no_qn_until,
                            &settings,
                            context.app_log_path.as_ref(),
                            room_id.as_str(),
                            "read_error",
                        );
                    }
                    let missing_since = missing_started_at.get_or_insert_with(Instant::now);
                    let missing_elapsed = missing_since.elapsed().as_secs();
                    if missing_elapsed >= MISSING_SEGMENT_WINDOW_SECS {
                        disconnect_retries += 1;
                        append_log(
                            &context.app_log_path,
                            &format!(
                                "stream_read_error_disconnect room={} elapsed={} retry={}",
                                room_id, missing_elapsed, disconnect_retries
                            ),
                        );
                        stream_urls.clear();
                        if disconnect_retries >= 3 {
                            if let Some(mut seg) = segment.take() {
                                let record_id = seg.record_id;
                                let file_path = seg.file_path.clone();
                                seg.finish("FAILED", Some("读取异常超过阈值，录制会话终止"))?;
                                drop(seg);
                                spawn_segment_remux(context.clone(), record_id, file_path);
                            }
                            return Err("读取异常超过阈值".to_string());
                        }
                        rotate_for_reconnect!("读取异常超过窗口，重连前切段");
                        std::thread::sleep(Duration::from_millis(
                            settings.stream_retry_ms.max(1000) as u64,
                        ));
                        break;
                    }
                    append_log(
                        &context.app_log_path,
                        &format!(
                            "stream_read_error_rotate_segment room={} elapsed={} window={}",
                            room_id, missing_elapsed, MISSING_SEGMENT_WINDOW_SECS
                        ),
                    );
                    rotate_for_reconnect!("读取异常，重连前切段");
                    stream_urls.clear();
                    std::thread::sleep(Duration::from_millis(
                        settings.stream_retry_ms.max(1000) as u64
                    ));
                    break;
                }
            }
        }
    }

    Ok(())
}

fn rotate_segment_for_reconnect(
    context: &LiveContext,
    room_id: &str,
    room_info: &LiveRoomInfo,
    nickname: Option<&str>,
    settings: &LiveSettings,
    base_dir: &Path,
    record_start_date: &str,
    current_title: &mut String,
    current_file_path: &mut String,
    segment: &mut Option<SegmentWriter>,
    segment_index: &mut i64,
    pending_split: &mut bool,
    pending_title: &mut Option<String>,
    missing_started_at: &mut Option<Instant>,
    reason: &str,
) -> Result<bool, String> {
    if let Some(mut seg) = segment.take() {
        let record_id = seg.record_id;
        let file_path = seg.file_path.clone();
        seg.finish("COMPLETED", Some(reason))?;
        drop(seg);
        spawn_segment_remux(context.clone(), record_id, file_path);
        *segment_index += 1;
        *current_title = load_current_title(context, room_id, current_title);
        *current_file_path = build_record_path(
            &settings.file_name_template,
            base_dir,
            room_info,
            nickname,
            record_start_date,
            *segment_index,
        );
        update_current_file(context, room_id, current_file_path);
        *pending_split = false;
        *pending_title = None;
        *missing_started_at = None;
        append_log(
            &context.app_log_path,
            &format!(
                "stream_reconnect_rotate room={} reason={} next_file={}",
                room_id, reason, current_file_path
            ),
        );
        return Ok(true);
    }
    Ok(false)
}

struct FlvTag {
    tag_type: u8,
    bytes: Vec<u8>,
    data_offset: usize,
    data_len: usize,
}

impl FlvTag {
    fn data(&self) -> &[u8] {
        &self.bytes[self.data_offset..self.data_offset + self.data_len]
    }
}

fn write_flv_timestamp(tag: &mut FlvTag, timestamp: u32) {
    if tag.bytes.len() < 8 {
        return;
    }
    tag.bytes[4] = ((timestamp >> 16) & 0xff) as u8;
    tag.bytes[5] = ((timestamp >> 8) & 0xff) as u8;
    tag.bytes[6] = (timestamp & 0xff) as u8;
    tag.bytes[7] = ((timestamp >> 24) & 0xff) as u8;
}

fn is_video_keyframe(tag: &FlvTag) -> bool {
    if tag.tag_type != 9 {
        return false;
    }
    let data = tag.data();
    if data.is_empty() {
        return false;
    }
    let frame_type = data[0] >> 4;
    if frame_type != 1 {
        return false;
    }
    let codec_id = data[0] & 0x0f;
    if (codec_id == 7 || codec_id == 12) && (data.len() < 2 || data[1] != 1) {
        return false;
    }
    true
}

fn should_open_segment_on_tag(tag: &FlvTag) -> bool {
    if tag.tag_type == 18 {
        return false;
    }
    if tag.tag_type == 8 && is_audio_header_tag(tag.data()) {
        return false;
    }
    if tag.tag_type == 9 && is_video_header_tag(tag.data()) {
        return false;
    }
    true
}

fn is_audio_header_tag(data: &[u8]) -> bool {
    if data.len() < 2 {
        return false;
    }
    let sound_format = data[0] >> 4;
    sound_format == 10 && data[1] == 0
}

fn is_video_header_tag(data: &[u8]) -> bool {
    if data.len() < 2 {
        return false;
    }
    let codec_id = data[0] & 0x0f;
    let packet_type = data[1];
    (codec_id == 7 || codec_id == 12) && packet_type == 0
}

fn clamp_timestamp(value: i64) -> u32 {
    if value <= 0 {
        return 0;
    }
    if value > u32::MAX as i64 {
        return u32::MAX;
    }
    value as u32
}

struct TimestampJumpInfo {
    diff: i64,
    original: i64,
    fixed: i64,
    offset: i64,
}

struct TimestampChannelState {
    last_original: Option<i64>,
    last_fixed: Option<i64>,
    last_step: i64,
    fallback: i64,
    min_step: i64,
    max_step: i64,
}

impl TimestampChannelState {
    fn new(fallback: i64, min_step: i64, max_step: i64) -> Self {
        Self {
            last_original: None,
            last_fixed: None,
            last_step: fallback,
            fallback,
            min_step,
            max_step,
        }
    }

    fn reset(&mut self) {
        self.last_original = None;
        self.last_fixed = None;
        self.last_step = self.fallback;
    }

    fn update_step(&mut self, current: i64) -> i64 {
        let step = match self.last_original {
            Some(prev) => {
                let diff = current - prev;
                if diff >= self.min_step && diff <= self.max_step {
                    diff
                } else {
                    self.fallback
                }
            }
            None => self.fallback,
        };
        self.last_original = Some(current);
        self.last_step = step;
        step
    }

    fn update_fixed(&mut self, fixed: i64) {
        self.last_fixed = Some(fixed);
    }
}

struct TimestampFixer {
    enabled: bool,
    apply_fix: bool,
    last_original: Option<i64>,
    last_fixed: Option<i64>,
    current_offset: i64,
    next_target: i64,
    audio: TimestampChannelState,
    video: TimestampChannelState,
}

impl TimestampFixer {
    fn new(enabled: bool, apply_fix: bool) -> Self {
        Self {
            enabled,
            apply_fix,
            last_original: None,
            last_fixed: None,
            current_offset: 0,
            next_target: 0,
            audio: TimestampChannelState::new(
                TIMESTAMP_AUDIO_FALLBACK_MS,
                TIMESTAMP_AUDIO_MIN_STEP_MS,
                TIMESTAMP_AUDIO_MAX_STEP_MS,
            ),
            video: TimestampChannelState::new(
                TIMESTAMP_VIDEO_FALLBACK_MS,
                TIMESTAMP_VIDEO_MIN_STEP_MS,
                TIMESTAMP_VIDEO_MAX_STEP_MS,
            ),
        }
    }

    fn reset(&mut self) {
        self.last_original = None;
        self.last_fixed = None;
        self.current_offset = 0;
        self.next_target = 0;
        self.audio.reset();
        self.video.reset();
    }

    fn fix_tag(&mut self, tag: &mut FlvTag, is_header: bool) -> Option<TimestampJumpInfo> {
        if !self.enabled {
            return None;
        }
        let original = parse_flv_timestamp(tag) as i64;
        if !self.apply_fix {
            if is_header {
                return None;
            }
            let last_original = match self.last_original {
                Some(value) => value,
                None => {
                    self.last_original = Some(original);
                    self.last_fixed = Some(original);
                    return None;
                }
            };
            let diff = original - last_original;
            self.last_original = Some(original);
            self.last_fixed = Some(original);
            if diff < -TIMESTAMP_JUMP_THRESHOLD_MS || diff > TIMESTAMP_JUMP_THRESHOLD_MS {
                return Some(TimestampJumpInfo {
                    diff,
                    original,
                    fixed: original,
                    offset: 0,
                });
            }
            return None;
        }
        if is_header {
            let stamp = if tag.tag_type == 18 {
                self.next_target
            } else {
                self.last_fixed.unwrap_or(0)
            };
            write_flv_timestamp(tag, clamp_timestamp(stamp));
            return None;
        }

        let step = match tag.tag_type {
            8 => self.audio.update_step(original),
            9 => self.video.update_step(original),
            _ => TIMESTAMP_MIN_STEP_MS,
        }
        .max(TIMESTAMP_MIN_STEP_MS);

        let last_original = match self.last_original {
            Some(value) => value,
            None => {
                self.current_offset = original;
                let fixed = 0;
                self.last_original = Some(original);
                self.last_fixed = Some(fixed);
                match tag.tag_type {
                    8 => self.audio.update_fixed(fixed),
                    9 => self.video.update_fixed(fixed),
                    _ => {}
                }
                self.recalculate_next_target();
                write_flv_timestamp(tag, fixed as u32);
                return None;
            }
        };
        let last_fixed = self.last_fixed.unwrap_or(0);
        let diff = original - last_original;
        let mut jump_info = None;

        if diff < -TIMESTAMP_JUMP_THRESHOLD_MS || diff > TIMESTAMP_JUMP_THRESHOLD_MS {
            self.current_offset = original - self.next_target;
        }

        let mut fixed = original - self.current_offset;
        if fixed <= last_fixed {
            fixed = last_fixed + step;
            self.current_offset = original - fixed;
        }
        self.last_original = Some(original);
        self.last_fixed = Some(fixed);
        match tag.tag_type {
            8 => self.audio.update_fixed(fixed),
            9 => self.video.update_fixed(fixed),
            _ => {}
        }
        self.recalculate_next_target();
        write_flv_timestamp(tag, clamp_timestamp(fixed));
        if diff < -TIMESTAMP_JUMP_THRESHOLD_MS || diff > TIMESTAMP_JUMP_THRESHOLD_MS {
            jump_info = Some(TimestampJumpInfo {
                diff,
                original,
                fixed,
                offset: self.current_offset,
            });
        }
        jump_info
    }

    fn recalculate_next_target(&mut self) {
        let audio_next = self
            .audio
            .last_fixed
            .map(|value| value + self.audio.last_step)
            .unwrap_or(0);
        let video_next = self
            .video
            .last_fixed
            .map(|value| value + self.video.last_step)
            .unwrap_or(0);
        self.next_target = audio_next.max(video_next);
    }
}

#[derive(Clone, Copy)]
struct LivePipelineSettings {
    split_on_script_tag: bool,
    disable_split_on_h264_annexb: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PipelineAnnexBState {
    Unknown,
    Pending,
    IsAnnexB,
}

#[derive(Default)]
struct LivePipelineDecision {
    request_split: Option<&'static str>,
    disconnect_reason: Option<String>,
    logs: Vec<String>,
    progressed: bool,
}

impl LivePipelineDecision {
    fn request_split_if_safe(&mut self, reason: &'static str) {
        if self.disconnect_reason.is_none() {
            self.request_split.get_or_insert(reason);
        }
    }
}

struct LiveFlvPipeline {
    settings: LivePipelineSettings,
    timestamp_fixer: TimestampFixer,
    metadata_received: bool,
    annexb_state: PipelineAnnexBState,
    last_audio_header: Option<Vec<u8>>,
    last_video_header: Option<Vec<u8>>,
    last_chunk_hash: Option<u64>,
    duplicate_chunk_count: usize,
    last_progress_timestamp: Option<u32>,
    stagnant_count: usize,
    last_progress_at: Instant,
}

impl LiveFlvPipeline {
    fn new(
        settings: LivePipelineSettings,
        enable_timestamp_fix: bool,
        apply_timestamp_fix: bool,
    ) -> Self {
        Self {
            settings,
            timestamp_fixer: TimestampFixer::new(enable_timestamp_fix, apply_timestamp_fix),
            metadata_received: false,
            annexb_state: PipelineAnnexBState::Unknown,
            last_audio_header: None,
            last_video_header: None,
            last_chunk_hash: None,
            duplicate_chunk_count: 0,
            last_progress_timestamp: None,
            stagnant_count: 0,
            last_progress_at: Instant::now(),
        }
    }

    fn reset_on_new_segment(&mut self) {
        self.timestamp_fixer.reset();
        self.last_progress_timestamp = None;
        self.stagnant_count = 0;
        self.last_progress_at = Instant::now();
        self.last_chunk_hash = None;
        self.duplicate_chunk_count = 0;
    }

    fn on_stream_header(&mut self) {
        if self.settings.disable_split_on_h264_annexb {
            self.annexb_state = PipelineAnnexBState::Unknown;
        }
    }

    fn process_tag(&mut self, tag: &mut FlvTag) -> LivePipelineDecision {
        let mut decision = LivePipelineDecision::default();

        self.apply_annexb_rule(tag, &mut decision);

        let is_header_tag = tag.tag_type == 18
            || is_audio_header_tag(tag.data())
            || is_video_header_tag(tag.data());
        self.apply_timestamp_jump_rule(tag, is_header_tag, &mut decision);
        self.apply_script_tag_rule(tag, &mut decision);
        self.apply_header_change_rule(tag, &mut decision);

        self.apply_duplicate_chunk_rule(tag, &mut decision);
        self.apply_progress_rule(tag, &mut decision);

        decision
    }

    fn apply_annexb_rule(&mut self, tag: &FlvTag, decision: &mut LivePipelineDecision) {
        if !(self.settings.disable_split_on_h264_annexb
            && tag.tag_type == 9
            && is_video_keyframe(tag))
        {
            return;
        }
        if !contains_annexb_signature(tag.data()) {
            return;
        }
        self.annexb_state = match self.annexb_state {
            PipelineAnnexBState::Unknown => {
                decision
                    .logs
                    .push("pipeline_annexb_detected state=pending".to_string());
                PipelineAnnexBState::Pending
            }
            PipelineAnnexBState::Pending | PipelineAnnexBState::IsAnnexB => {
                decision
                    .logs
                    .push("pipeline_annexb_detected state=locked".to_string());
                PipelineAnnexBState::IsAnnexB
            }
        };
    }

    fn apply_timestamp_jump_rule(
        &mut self,
        tag: &mut FlvTag,
        is_header_tag: bool,
        decision: &mut LivePipelineDecision,
    ) {
        let Some(jump) = self.timestamp_fixer.fix_tag(tag, is_header_tag) else {
            return;
        };
        let jump_abs = jump.diff.abs();
        let is_large_jump = jump_abs >= TIMESTAMP_JUMP_DISCONNECT_THRESHOLD_MS;
        let jump_class = if is_large_jump { "large" } else { "normal" };
        decision.logs.push(format!(
            "pipeline_timestamp_jump diff={} abs_diff={} original={} fixed={} offset={} class={}",
            jump.diff, jump_abs, jump.original, jump.fixed, jump.offset, jump_class
        ));
        decision.logs.push(format!(
            "pipeline_timestamp_jump_handled mode=adjust_only class={}",
            jump_class
        ));
    }

    fn apply_script_tag_rule(&mut self, tag: &FlvTag, decision: &mut LivePipelineDecision) {
        if tag.tag_type != 18 {
            return;
        }
        if !self.metadata_received {
            self.metadata_received = true;
            return;
        }
        if self.settings.split_on_script_tag {
            decision.request_split_if_safe("script_tag");
        }
    }

    fn apply_header_change_rule(&mut self, tag: &FlvTag, decision: &mut LivePipelineDecision) {
        let audio_changed =
            is_audio_header_tag(tag.data()) && self.process_header_change(true, tag);
        if audio_changed {
            if self.settings.disable_split_on_h264_annexb
                && self.annexb_state == PipelineAnnexBState::IsAnnexB
            {
                decision
                    .logs
                    .push("pipeline_header_changed skip_split=annexb".to_string());
            } else {
                decision.request_split_if_safe("audio_header_changed");
            }
        }

        let video_changed =
            is_video_header_tag(tag.data()) && self.process_header_change(false, tag);
        if video_changed {
            if self.settings.disable_split_on_h264_annexb
                && self.annexb_state == PipelineAnnexBState::IsAnnexB
            {
                decision
                    .logs
                    .push("pipeline_header_changed skip_split=annexb".to_string());
            } else {
                decision.request_split_if_safe("video_header_changed");
            }
        }
    }

    fn apply_duplicate_chunk_rule(&mut self, tag: &FlvTag, decision: &mut LivePipelineDecision) {
        let chunk_hash = calculate_tag_hash(&tag.bytes);
        if self.last_chunk_hash == Some(chunk_hash) {
            self.duplicate_chunk_count += 1;
        } else {
            self.last_chunk_hash = Some(chunk_hash);
            self.duplicate_chunk_count = 0;
        }
        if self.duplicate_chunk_count >= INVALID_STREAM_TAG_LIMIT
            && self.last_progress_at.elapsed().as_secs() >= INVALID_STREAM_STALL_SECS
        {
            decision
                .disconnect_reason
                .get_or_insert_with(|| "连续重复数据块，触发断流重连".to_string());
        }
    }

    fn apply_progress_rule(&mut self, tag: &FlvTag, decision: &mut LivePipelineDecision) {
        let timestamp = parse_flv_timestamp(tag);
        if let Some(prev) = self.last_progress_timestamp {
            if timestamp > prev {
                self.last_progress_timestamp = Some(timestamp);
                self.stagnant_count = 0;
                self.last_progress_at = Instant::now();
                decision.progressed = true;
            } else {
                self.stagnant_count += 1;
            }
        } else {
            self.last_progress_timestamp = Some(timestamp);
            self.stagnant_count = 0;
            self.last_progress_at = Instant::now();
            decision.progressed = true;
        }

        if self.stagnant_count >= INVALID_STREAM_TAG_LIMIT
            && self.last_progress_at.elapsed().as_secs() >= INVALID_STREAM_STALL_SECS
        {
            decision
                .disconnect_reason
                .get_or_insert_with(|| format!("时间戳停滞，触发断流重连 timestamp={}", timestamp));
        }
    }

    fn process_header_change(&mut self, is_audio: bool, tag: &FlvTag) -> bool {
        let normalized = normalize_header_tag(&tag.bytes);
        let slot = if is_audio {
            &mut self.last_audio_header
        } else {
            &mut self.last_video_header
        };
        let changed = slot
            .as_ref()
            .map(|prev| prev != &normalized)
            .unwrap_or(false);
        *slot = Some(normalized);
        changed
    }
}

enum FlvParsedItem {
    Header(Vec<u8>),
    Tag(FlvTag),
}

struct FlvStreamParser {
    buffer: Vec<u8>,
    header_parsed: bool,
}

impl FlvStreamParser {
    fn new() -> Self {
        Self {
            buffer: Vec::new(),
            header_parsed: false,
        }
    }

    fn push(&mut self, data: &[u8]) -> Result<Vec<FlvParsedItem>, String> {
        if !data.is_empty() {
            self.buffer.extend_from_slice(data);
        }
        let mut items = Vec::new();
        let mut offset = 0;
        if !self.header_parsed {
            if self.buffer.len() < 3 {
                return Ok(items);
            }
            if self.buffer[..3] != *b"FLV" {
                return Err("FLV header mismatch".to_string());
            }
            if self.buffer.len() < 13 {
                return Ok(items);
            }
            let header = self.buffer[offset..offset + 13].to_vec();
            offset += 13;
            self.header_parsed = true;
            items.push(FlvParsedItem::Header(header));
        }

        loop {
            if self.buffer.len().saturating_sub(offset) < 11 {
                break;
            }
            let header_start = offset;
            let data_size = read_u24_be(&self.buffer[header_start + 1..header_start + 4]);
            let total = 11 + data_size + 4;
            if self.buffer.len().saturating_sub(offset) < total {
                break;
            }
            let bytes = self.buffer[offset..offset + total].to_vec();
            let tag_type = bytes[0];
            let data_offset = 11;
            let data_len = data_size;
            items.push(FlvParsedItem::Tag(FlvTag {
                tag_type,
                bytes,
                data_offset,
                data_len,
            }));
            offset += total;
        }

        if offset > 0 {
            self.buffer.drain(0..offset);
        }
        Ok(items)
    }
}

struct FlvHeaderCache {
    header: Option<Vec<u8>>,
    script_tag: Option<Vec<u8>>,
    audio_header: Option<Vec<u8>>,
    video_header: Option<Vec<u8>>,
}

impl FlvHeaderCache {
    fn new() -> Self {
        Self {
            header: None,
            script_tag: None,
            audio_header: None,
            video_header: None,
        }
    }

    fn set_header(&mut self, header: Vec<u8>) {
        self.header = Some(header);
    }

    fn has_header(&self) -> bool {
        self.header.is_some()
    }

    fn update_from_tag(&mut self, tag: &FlvTag) {
        match tag.tag_type {
            18 => {
                if self.script_tag.is_none() {
                    self.script_tag = Some(normalize_header_tag(&tag.bytes));
                }
            }
            8 => {
                if is_audio_header(tag.data(), self.audio_header.is_some()) {
                    self.audio_header = Some(normalize_header_tag(&tag.bytes));
                }
            }
            9 => {
                if is_video_header(tag.data(), self.video_header.is_some()) {
                    self.video_header = Some(normalize_header_tag(&tag.bytes));
                }
            }
            _ => {}
        }
    }

    fn write_preamble(&self, segment: &mut SegmentWriter) -> Result<(), String> {
        let header = self
            .header
            .as_ref()
            .ok_or_else(|| "缺少FLV头信息".to_string())?;
        segment.write(header)?;
        if let Some(tag) = self.script_tag.as_ref() {
            segment.write(tag)?;
        }
        if let Some(tag) = self.video_header.as_ref() {
            segment.write(tag)?;
        }
        if let Some(tag) = self.audio_header.as_ref() {
            segment.write(tag)?;
        }
        Ok(())
    }
}

fn read_u24_be(slice: &[u8]) -> usize {
    if slice.len() < 3 {
        return 0;
    }
    ((slice[0] as usize) << 16) | ((slice[1] as usize) << 8) | slice[2] as usize
}

fn parse_flv_timestamp(tag: &FlvTag) -> u32 {
    if tag.bytes.len() < 8 {
        return 0;
    }
    let ts = ((tag.bytes[7] as u32) << 24)
        | ((tag.bytes[4] as u32) << 16)
        | ((tag.bytes[5] as u32) << 8)
        | (tag.bytes[6] as u32);
    ts
}

fn calculate_tag_hash(data: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    data.hash(&mut hasher);
    hasher.finish()
}

fn contains_annexb_signature(data: &[u8]) -> bool {
    if data.len() < 6 {
        return false;
    }
    let payload = &data[2..];
    let mut has_sps = false;
    let mut has_pps = false;
    let mut i = 0usize;
    while i + 4 < payload.len() {
        if payload[i] == 0 && payload[i + 1] == 0 && payload[i + 2] == 0 && payload[i + 3] == 1 {
            let nalu_type = payload[i + 4] & 0x1f;
            if nalu_type == 7 {
                has_sps = true;
            } else if nalu_type == 8 {
                has_pps = true;
            }
            if has_sps && has_pps {
                return true;
            }
            i += 4;
            continue;
        }
        i += 1;
    }
    false
}

fn normalize_header_tag(tag: &[u8]) -> Vec<u8> {
    let mut normalized = tag.to_vec();
    if normalized.len() >= 11 {
        normalized[4] = 0;
        normalized[5] = 0;
        normalized[6] = 0;
        normalized[7] = 0;
    }
    normalized
}

fn is_audio_header(data: &[u8], has_header: bool) -> bool {
    if data.len() < 2 {
        return false;
    }
    let sound_format = data[0] >> 4;
    if sound_format == 10 {
        data[1] == 0
    } else {
        !has_header
    }
}

fn is_video_header(data: &[u8], has_header: bool) -> bool {
    if data.len() < 2 {
        return false;
    }
    let codec_id = data[0] & 0x0f;
    let packet_type = data[1];
    if codec_id == 7 || codec_id == 12 {
        packet_type == 0
    } else {
        !has_header
    }
}

struct SegmentWriter {
    db: Arc<Db>,
    log_path: Arc<PathBuf>,
    record_id: i64,
    file_path: String,
    file: File,
    bytes_written: u64,
    #[allow(dead_code)]
    title: String,
    metadata_path: Option<String>,
}

impl SegmentWriter {
    fn write(&mut self, buf: &[u8]) -> Result<(), String> {
        self.file
            .write_all(buf)
            .map_err(|err| format!("写入失败: {}", err))?;
        self.bytes_written += buf.len() as u64;
        Ok(())
    }

    fn finish(&mut self, status: &str, error: Option<&str>) -> Result<(), String> {
        let end_time = now_rfc3339();
        update_record_task(
            &self.db,
            self.record_id,
            status,
            Some(end_time.clone()),
            self.bytes_written,
            error,
        )?;
        if let Some(path) = self.metadata_path.as_ref() {
            if let Err(err) = update_metadata_file(path, &end_time, self.bytes_written) {
                append_log(
                    self.log_path.as_ref(),
                    &format!(
                        "record_metadata_update_failed record_id={} err={}",
                        self.record_id, err
                    ),
                );
            }
        }
        Ok(())
    }
}

fn open_segment(
    context: &LiveContext,
    room_id: &str,
    file_path: &str,
    title: &str,
    segment_index: i64,
    settings: &LiveSettings,
    room_info: &LiveRoomInfo,
    nickname: Option<&str>,
) -> Result<SegmentWriter, String> {
    if let Some(parent) = Path::new(file_path).parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("创建目录失败: {}", err))?;
    }

    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(file_path)
        .map_err(|err| format!("创建文件失败: {}", err))?;

    let record_id = insert_record_task(&context.db, room_id, file_path, segment_index, title)?;
    let metadata_path = if settings.write_metadata {
        Some(write_metadata_file(file_path, room_info, nickname, title)?)
    } else {
        None
    };
    Ok(SegmentWriter {
        db: Arc::clone(&context.db),
        log_path: Arc::clone(&context.app_log_path),
        record_id,
        file_path: file_path.to_string(),
        file,
        bytes_written: 0,
        title: title.to_string(),
        metadata_path,
    })
}

fn spawn_segment_remux(context: LiveContext, record_id: i64, file_path: String) {
    let source_path = PathBuf::from(file_path);
    let ext = source_path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_string();
    if !ext.eq_ignore_ascii_case("flv") {
        return;
    }
    let target_path = source_path.with_extension("mp4");
    let source = source_path.to_string_lossy().to_string();
    let target = target_path.to_string_lossy().to_string();
    let source_size = std::fs::metadata(&source_path)
        .map(|meta| meta.len())
        .unwrap_or(0);
    let log_path = context.app_log_path.clone();
    let db = context.db.clone();
    tauri::async_runtime::spawn(async move {
        append_log(
            log_path.as_ref(),
            &format!(
                "live_remux_start record_id={} source={} target={}",
                record_id, source, target
            ),
        );
        let copy_args = build_live_remux_copy_args(&source, &target);
        let copy_result = tauri::async_runtime::spawn_blocking(move || run_ffmpeg(&copy_args))
            .await
            .map_err(|_| "转封装执行失败".to_string());

        let mut final_target_size = 0u64;
        let mut used_fallback_transcode = false;
        let mut remux_ok = matches!(copy_result, Ok(Ok(())));
        if remux_ok {
            final_target_size = std::fs::metadata(&target)
                .map(|meta| meta.len())
                .unwrap_or(0);
            if is_suspicious_remux_output(source_size, final_target_size) {
                append_log(
          log_path.as_ref(),
          &format!(
            "live_remux_suspicious record_id={} source_size={} target_size={} action=fallback_transcode",
            record_id, source_size, final_target_size
          ),
        );
                used_fallback_transcode = true;
                let fallback_args = build_live_remux_transcode_args(&source, &target);
                let fallback_result =
                    tauri::async_runtime::spawn_blocking(move || run_ffmpeg(&fallback_args))
                        .await
                        .map_err(|_| "兜底转码执行失败".to_string());
                remux_ok = matches!(fallback_result, Ok(Ok(())));
                if remux_ok {
                    final_target_size = std::fs::metadata(&target)
                        .map(|meta| meta.len())
                        .unwrap_or(0);
                } else if let Ok(Err(err)) = fallback_result {
                    append_log(
                        log_path.as_ref(),
                        &format!(
                            "live_remux_fallback_fail record_id={} err={}",
                            record_id, err
                        ),
                    );
                } else if let Err(err) = fallback_result {
                    append_log(
                        log_path.as_ref(),
                        &format!(
                            "live_remux_fallback_fail record_id={} err={}",
                            record_id, err
                        ),
                    );
                }
            }
        } else if let Ok(Err(err)) = copy_result {
            append_log(
                log_path.as_ref(),
                &format!("live_remux_copy_fail record_id={} err={}", record_id, err),
            );
            used_fallback_transcode = true;
            let fallback_args = build_live_remux_transcode_args(&source, &target);
            let fallback_result =
                tauri::async_runtime::spawn_blocking(move || run_ffmpeg(&fallback_args))
                    .await
                    .map_err(|_| "兜底转码执行失败".to_string());
            remux_ok = matches!(fallback_result, Ok(Ok(())));
            if remux_ok {
                final_target_size = std::fs::metadata(&target)
                    .map(|meta| meta.len())
                    .unwrap_or(0);
            } else if let Ok(Err(fallback_err)) = fallback_result {
                append_log(
                    log_path.as_ref(),
                    &format!(
                        "live_remux_fallback_fail record_id={} err={}",
                        record_id, fallback_err
                    ),
                );
            } else if let Err(fallback_err) = fallback_result {
                append_log(
                    log_path.as_ref(),
                    &format!(
                        "live_remux_fallback_fail record_id={} err={}",
                        record_id, fallback_err
                    ),
                );
            }
        } else if let Err(err) = copy_result {
            append_log(
                log_path.as_ref(),
                &format!("live_remux_copy_fail record_id={} err={}", record_id, err),
            );
        }

        let final_suspicious = !remux_ok
            || final_target_size < REMUX_MIN_VALID_OUTPUT_BYTES
            || is_suspicious_remux_output(source_size, final_target_size);
        if final_suspicious {
            if let Err(err) = std::fs::remove_file(&target) {
                append_log(
                    log_path.as_ref(),
                    &format!(
                        "live_remux_cleanup_fail record_id={} err={}",
                        record_id, err
                    ),
                );
            }
            append_log(
                log_path.as_ref(),
                &format!(
                    "live_remux_done record_id={} status=warn keep=flv fallback={}",
                    record_id, used_fallback_transcode
                ),
            );
            if let Err(err) = baidu_sync::enqueue_live_sync(&db, log_path.as_ref(), record_id) {
                append_log(
                    log_path.as_ref(),
                    &format!(
                        "baidu_sync_enqueue_fail record_id={} err={}",
                        record_id, err
                    ),
                );
            }
            return;
        }

        if let Err(err) = update_record_task_file_path(&db, record_id, &target, final_target_size) {
            append_log(
                log_path.as_ref(),
                &format!("live_remux_update_fail record_id={} err={}", record_id, err),
            );
        }
        append_log(
            log_path.as_ref(),
            &format!(
                "live_remux_done record_id={} status=ok fallback={} size={}",
                record_id, used_fallback_transcode, final_target_size
            ),
        );
        if let Err(err) = baidu_sync::enqueue_live_sync(&db, log_path.as_ref(), record_id) {
            append_log(
                log_path.as_ref(),
                &format!(
                    "baidu_sync_enqueue_fail record_id={} err={}",
                    record_id, err
                ),
            );
        }
    });
}

fn build_live_remux_copy_args(source: &str, target: &str) -> Vec<String> {
    vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-y".to_string(),
        "-fflags".to_string(),
        "+genpts".to_string(),
        "-i".to_string(),
        source.to_string(),
        "-map".to_string(),
        "0".to_string(),
        "-dn".to_string(),
        "-c".to_string(),
        "copy".to_string(),
        target.to_string(),
    ]
}

fn build_live_remux_transcode_args(source: &str, target: &str) -> Vec<String> {
    vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-y".to_string(),
        "-fflags".to_string(),
        "+genpts".to_string(),
        "-i".to_string(),
        source.to_string(),
        "-map".to_string(),
        "0:v:0".to_string(),
        "-map".to_string(),
        "0:a:0?".to_string(),
        "-dn".to_string(),
        "-c:v".to_string(),
        "libx264".to_string(),
        "-preset".to_string(),
        "veryfast".to_string(),
        "-crf".to_string(),
        "18".to_string(),
        "-pix_fmt".to_string(),
        "yuv420p".to_string(),
        "-c:a".to_string(),
        "aac".to_string(),
        "-b:a".to_string(),
        "192k".to_string(),
        target.to_string(),
    ]
}

fn is_suspicious_remux_output(source_size: u64, target_size: u64) -> bool {
    source_size >= REMUX_SUSPECT_SOURCE_MIN_BYTES
        && (target_size as u128) * (REMUX_SUSPECT_RATIO_DEN as u128)
            < (source_size as u128) * (REMUX_SUSPECT_RATIO_NUM as u128)
}

fn insert_record_task(
    db: &Db,
    room_id: &str,
    file_path: &str,
    segment_index: i64,
    title: &str,
) -> Result<i64, String> {
    let now = now_rfc3339();
    db.with_conn(|conn| {
    conn.execute(
      "INSERT INTO live_record_task (room_id, status, file_path, segment_index, start_time, title, create_time, update_time) \
       VALUES (?1, 'RECORDING', ?2, ?3, ?4, ?5, ?6, ?7)",
      (room_id, file_path, segment_index, &now, title, &now, &now),
    )?;
    Ok(conn.last_insert_rowid())
  })
  .map_err(|err| format!("写入录制任务失败: {}", err))
}

fn update_record_task(
    db: &Db,
    record_id: i64,
    status: &str,
    end_time: Option<String>,
    file_size: u64,
    error: Option<&str>,
) -> Result<(), String> {
    let now = now_rfc3339();
    let end_time_value = end_time.unwrap_or_else(|| now.clone());
    let error_message = error.map(|value| value.to_string());
    db.with_conn(|conn| {
    conn.execute(
      "UPDATE live_record_task SET status = ?1, end_time = ?2, file_size = ?3, error_message = ?4, update_time = ?5 WHERE id = ?6",
      (status, &end_time_value, file_size as i64, error_message, &now, record_id),
    )?;
    Ok(())
  })
  .map_err(|err| format!("更新录制任务失败: {}", err))
}

fn update_record_task_file_path(
    db: &Db,
    record_id: i64,
    file_path: &str,
    file_size: u64,
) -> Result<(), String> {
    let now = now_rfc3339();
    db.with_conn(|conn| {
        conn.execute(
      "UPDATE live_record_task SET file_path = ?1, file_size = ?2, update_time = ?3 WHERE id = ?4",
      (file_path, file_size as i64, &now, record_id),
    )?;
        Ok(())
    })
    .map_err(|err| format!("更新录播路径失败: {}", err))
}

fn load_anchor_room_ids(db: &Db) -> Result<Vec<String>, String> {
    db.with_conn(|conn| {
        let mut stmt = conn.prepare("SELECT uid FROM anchor ORDER BY id DESC")?;
        let rows = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(rows)
    })
    .map_err(|err| err.to_string())
}

fn load_anchor_nickname(db: &Db, room_id: &str) -> Result<Option<String>, String> {
    db.with_conn(|conn| {
        conn.query_row(
            "SELECT nickname FROM anchor WHERE uid = ?1",
            [room_id],
            |row| row.get(0),
        )
        .optional()
    })
    .map_err(|err| err.to_string())
}

fn load_room_auto_record(db: &Db, room_id: &str) -> Result<bool, String> {
    db.with_conn(|conn| {
        conn.query_row(
            "SELECT auto_record FROM live_room_settings WHERE room_id = ?1",
            [room_id],
            |row| row.get::<_, i64>(0),
        )
        .map(|value| value != 0)
        .or(Ok(true))
    })
    .map_err(|err| err.to_string())
}

fn update_anchor_status(db: &Db, room_id: &str, live_status: i64) -> Result<(), String> {
    let now = now_rfc3339();
    db.with_conn(|conn| {
        conn.execute(
      "UPDATE anchor SET live_status = ?1, last_check_time = ?2, update_time = ?3 WHERE uid = ?4",
      (live_status, &now, &now, room_id),
    )?;
        Ok(())
    })
    .map_err(|err| err.to_string())
}

fn update_current_file(context: &LiveContext, room_id: &str, file_path: &str) {
    if let Ok(map) = context.live_runtime.records.lock() {
        if let Some(handle) = map.get(room_id) {
            if let Ok(mut path) = handle.current_file.lock() {
                *path = file_path.to_string();
            }
        }
    }
}

fn load_current_title(context: &LiveContext, room_id: &str, fallback: &str) -> String {
    if let Ok(map) = context.live_runtime.records.lock() {
        if let Some(handle) = map.get(room_id) {
            if let Ok(title) = handle.last_title.lock() {
                return title.clone();
            }
        }
    }
    fallback.to_string()
}

fn load_record_start_date(context: &LiveContext, room_id: &str) -> String {
    if let Ok(map) = context.live_runtime.records.lock() {
        if let Some(handle) = map.get(room_id) {
            if !handle.start_date.trim().is_empty() {
                return handle.start_date.clone();
            }
        }
    }
    Utc::now().format("%Y%m%d").to_string()
}

fn build_record_path(
    template: &str,
    base_dir: &Path,
    info: &LiveRoomInfo,
    nickname: Option<&str>,
    record_start_date: &str,
    segment_index: i64,
) -> String {
    let now = Utc::now();
    let now_str = now.format("%Y%m%d-%H%M%S").to_string();
    let date_str = now.format("%Y%m%d").to_string();
    let time_str = now.format("%H%M%S").to_string();
    let ms_str = format!("{:03}", now.timestamp_subsec_millis());
    let mut output = template.to_string();
    output = output.replace("{{ roomId }}", &info.room_id);
    output = output.replace("{{ uid }}", &info.uid);
    output = output.replace("{{ name }}", nickname.unwrap_or("主播"));
    output = output.replace("{{ title }}", &info.title);
    output = output.replace("{{ now }}", &now_str);
    output = output.replace("{{ date }}", &date_str);
    output = output.replace("{{ liveDate }}", record_start_date);
    output = output.replace("{{ live_date }}", record_start_date);
    output = output.replace("{{ time }}", &time_str);
    output = output.replace("{{ ms }}", &ms_str);
    output = output.replace(
        "{{ \"now\" | format_date: \"yyyyMMdd-HHmmss-fff\" }}",
        &format!("{}-{}", now.format("%Y%m%d-%H%M%S"), ms_str),
    );

    let relative = sanitize_path(&output);
    let mut path = if Path::new(&relative).is_absolute() {
        PathBuf::from(relative)
    } else {
        base_dir.join(relative)
    };

    if path.extension().is_none() {
        path.set_extension("flv");
    }

    if segment_index > 1 {
        let stem = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("record")
            .to_string();
        let ext = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("flv");
        path.set_file_name(format!("{}_part{}.{}", stem, segment_index, ext));
    }

    path.to_string_lossy().to_string()
}

fn sanitize_path(path: &str) -> String {
    let mut parts = Vec::new();
    for part in path.split(['/', '\\']) {
        if part.is_empty() {
            continue;
        }
        parts.push(sanitize_filename(part));
    }
    parts.join(std::path::MAIN_SEPARATOR_STR)
}

fn write_metadata_file(
    file_path: &str,
    room_info: &LiveRoomInfo,
    nickname: Option<&str>,
    title: &str,
) -> Result<String, String> {
    let metadata_path = Path::new(file_path)
        .with_extension("metadata.json")
        .to_string_lossy()
        .to_string();
    let payload = serde_json::json!({
      "roomId": room_info.room_id,
      "uid": room_info.uid,
      "nickname": nickname,
      "title": title,
      "startTime": now_rfc3339(),
    });
    let mut file =
        File::create(&metadata_path).map_err(|err| format!("创建 metadata 失败: {}", err))?;
    file.write_all(payload.to_string().as_bytes())
        .map_err(|err| format!("写入 metadata 失败: {}", err))?;
    Ok(metadata_path)
}

fn update_metadata_file(path: &str, end_time: &str, file_size: u64) -> Result<(), String> {
    let mut value = if let Ok(content) = std::fs::read_to_string(path) {
        serde_json::from_str::<Value>(&content).unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    if !value.is_object() {
        value = serde_json::json!({});
    }
    let obj = value
        .as_object_mut()
        .ok_or_else(|| "metadata 结构异常".to_string())?;
    obj.insert("endTime".to_string(), Value::String(end_time.to_string()));
    obj.insert("fileSize".to_string(), Value::Number(file_size.into()));
    if let Some(parent) = Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("创建 metadata 目录失败: {}", err))?;
        }
    }
    std::fs::write(path, value.to_string())
        .map_err(|err| format!("更新 metadata 失败: {}", err))?;
    Ok(())
}

fn derive_recovered_record_end_time(
    file_path: &Path,
    metadata_path: &str,
    fallback: &str,
) -> String {
    if let Some(value) = derive_end_time_from_media_duration(file_path, metadata_path) {
        return value;
    }
    if let Ok(modified) = std::fs::metadata(file_path).and_then(|meta| meta.modified()) {
        let modified_time: DateTime<Utc> = modified.into();
        return modified_time.to_rfc3339();
    }
    fallback.to_string()
}

fn derive_end_time_from_media_duration(file_path: &Path, metadata_path: &str) -> Option<String> {
    let content = std::fs::read_to_string(metadata_path).ok()?;
    let value = serde_json::from_str::<Value>(&content).ok()?;
    let start_time = value.get("startTime")?.as_str()?;
    let start_at = DateTime::parse_from_rfc3339(start_time)
        .ok()?
        .with_timezone(&Utc);
    let duration_secs = probe_media_duration_seconds(file_path)?;
    let duration_ms = (duration_secs * 1000.0).round() as i64;
    let end_at = start_at.checked_add_signed(ChronoDuration::milliseconds(duration_ms))?;
    Some(end_at.to_rfc3339())
}

fn probe_media_duration_seconds(file_path: &Path) -> Option<f64> {
    let args = vec![
        "-v".to_string(),
        "error".to_string(),
        "-show_entries".to_string(),
        "format=duration".to_string(),
        "-of".to_string(),
        "json".to_string(),
        file_path.to_string_lossy().to_string(),
    ];
    let data = run_ffprobe_json(&args).ok()?;
    data.get("format")?
        .get("duration")?
        .as_str()?
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value >= 0.0)
}

fn download_cover(target_file: &str, cover_url: &str) -> Result<(), String> {
    let response = Client::new()
        .get(cover_url)
        .send()
        .map_err(|err| format!("下载封面失败: {}", err))?;

    let mut ext = "jpg".to_string();
    if let Some(content_type) = response.headers().get(reqwest::header::CONTENT_TYPE) {
        if let Ok(content_type) = content_type.to_str() {
            if content_type.contains("png") {
                ext = "png".to_string();
            } else if content_type.contains("webp") {
                ext = "webp".to_string();
            }
        }
    }

    let cover_path = Path::new(target_file)
        .with_extension(format!("cover.{}", ext))
        .to_string_lossy()
        .to_string();
    let mut file = File::create(&cover_path).map_err(|err| format!("创建封面失败: {}", err))?;
    let bytes = response
        .bytes()
        .map_err(|err| format!("读取封面失败: {}", err))?;
    file.write_all(&bytes)
        .map_err(|err| format!("保存封面失败: {}", err))?;
    Ok(())
}

fn fetch_stream_urls(
    client: &BilibiliClient,
    room_id: &str,
    settings: &LiveSettings,
    auth: Option<&AuthInfo>,
    with_quality: bool,
) -> Result<Vec<String>, String> {
    let qn = if with_quality {
        parse_quality(&settings.recording_quality)
    } else {
        0
    };
    let params = vec![
        ("room_id".to_string(), room_id.to_string()),
        ("no_playurl".to_string(), "0".to_string()),
        ("mask".to_string(), "1".to_string()),
        ("qn".to_string(), qn.to_string()),
        ("platform".to_string(), "web".to_string()),
        ("protocol".to_string(), "0,1".to_string()),
        ("format".to_string(), "0,1,2".to_string()),
        ("codec".to_string(), "0,1,2".to_string()),
        ("dolby".to_string(), "5".to_string()),
        ("panorama".to_string(), "1".to_string()),
        ("hdr_type".to_string(), "0,1".to_string()),
        ("web_location".to_string(), "444.8".to_string()),
    ];

    let data = tauri::async_runtime::block_on(client.get_json(
        "https://api.live.bilibili.com/xlive/web-room/v2/index/getRoomPlayInfo",
        &params,
        auth,
        true,
    ))?;

    let streams = data
        .pointer("/playurl_info/playurl/stream")
        .and_then(|value| value.as_array())
        .ok_or("缺少直播流信息 playurl_info.playurl.stream")?;
    let protocol = streams
        .iter()
        .find(|item| {
            item.get("protocol_name").and_then(|value| value.as_str()) == Some("http_stream")
        })
        .ok_or("未找到 http_stream 协议直播流")?;
    let formats = protocol
        .get("format")
        .and_then(|value| value.as_array())
        .ok_or("缺少直播流格式信息 format")?;
    let format = formats
        .iter()
        .find(|item| item.get("format_name").and_then(|value| value.as_str()) == Some("flv"))
        .ok_or("未找到 flv 格式直播流")?;
    let codecs = format
        .get("codec")
        .and_then(|value| value.as_array())
        .ok_or("缺少直播流编码信息 codec")?;
    if codecs.is_empty() {
        return Err("直播流编码为空".to_string());
    }

    let mut selected_codecs: Vec<&Value> = Vec::new();
    if with_quality {
        selected_codecs.extend(codecs.iter().filter(|codec| codec_matches_qn(codec, qn)));
    }
    if selected_codecs.is_empty() {
        selected_codecs.extend(codecs.iter());
    }

    let urls = build_stream_urls(&selected_codecs);
    if urls.is_empty() {
        return Err("直播流地址为空".to_string());
    }
    Ok(urls)
}

fn codec_matches_qn(codec: &Value, target_qn: i64) -> bool {
    if target_qn <= 0 {
        return true;
    }
    let current_qn = codec.get("current_qn").and_then(value_to_i64);
    if current_qn == Some(target_qn) {
        return true;
    }
    codec
        .get("accept_qn")
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .any(|value| value_to_i64(value) == Some(target_qn))
        })
        .unwrap_or(false)
}

fn build_stream_urls(codecs: &[&Value]) -> Vec<String> {
    let mut preferred = Vec::new();
    let mut fallback = Vec::new();
    for codec in codecs {
        let base_url = codec
            .get("base_url")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let Some(url_infos) = codec.get("url_info").and_then(|value| value.as_array()) else {
            continue;
        };
        for info in url_infos {
            let host = info
                .get("host")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            let extra = info
                .get("extra")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            let Some(url) = join_stream_url(host, base_url, extra) else {
                continue;
            };
            if host.contains(".mcdn.") {
                push_unique_url(&mut fallback, url);
            } else {
                push_unique_url(&mut preferred, url);
            }
        }
    }
    preferred.extend(fallback);
    preferred
}

fn push_unique_url(target: &mut Vec<String>, url: String) {
    if !target.iter().any(|item| item == &url) {
        target.push(url);
    }
}

fn join_stream_url(host: &str, base_url: &str, extra: &str) -> Option<String> {
    if base_url.is_empty() {
        return None;
    }
    if base_url.starts_with("http://") || base_url.starts_with("https://") {
        return Some(format!("{}{}", base_url, extra));
    }
    if host.is_empty() {
        return None;
    }
    let host = host.trim_end_matches('/');
    let base = if base_url.starts_with('/') {
        base_url.to_string()
    } else {
        format!("/{}", base_url)
    };
    Some(format!("{}{}{}", host, base, extra))
}

fn is_hls_url(url: &str) -> bool {
    url.contains(".m3u8")
}

fn normalize_hls_path(path: &str) -> String {
    let mut target = PathBuf::from(path);
    let ext = target
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext == "ts" || ext == "m4s" || ext == "mp4" {
        return target.to_string_lossy().to_string();
    }
    target.set_extension("ts");
    target.to_string_lossy().to_string()
}

fn record_hls_stream(
    context: &LiveContext,
    room_id: &str,
    room_info: &LiveRoomInfo,
    nickname: Option<&str>,
    title: &str,
    file_path: &str,
    segment_index: i64,
    settings: &LiveSettings,
    stop_flag: &Arc<AtomicBool>,
    stream_url: &str,
) -> Result<(), String> {
    if let Some(parent) = Path::new(file_path).parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("创建目录失败: {}", err))?;
    }

    let record_id = insert_record_task(&context.db, room_id, file_path, segment_index, title)?;
    let metadata_path = if settings.write_metadata {
        Some(write_metadata_file(file_path, room_info, nickname, title)?)
    } else {
        None
    };

    let referer_value = format!(
        "Referer:https://live.bilibili.com/{}\r\n",
        room_info.room_id
    );
    let args = vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-rw_timeout".to_string(),
        "10000000".to_string(),
        "-timeout".to_string(),
        "10000000".to_string(),
        "-reconnect".to_string(),
        "1".to_string(),
        "-reconnect_streamed".to_string(),
        "1".to_string(),
        "-reconnect_delay_max".to_string(),
        "3".to_string(),
        "-user_agent".to_string(),
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36".to_string(),
        "-headers".to_string(),
        referer_value,
        "-i".to_string(),
        stream_url.to_string(),
        "-c".to_string(),
        "copy".to_string(),
        "-f".to_string(),
        "mpegts".to_string(),
        file_path.to_string(),
    ];

    let mut command = Command::new(resolve_ffmpeg_path());
    apply_no_window(&mut command);
    let mut child = command
        .args(&args)
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("启动FFmpeg失败: {}", err))?;

    let stderr = child.stderr.take();
    let (stderr_tx, stderr_rx) = mpsc::channel();
    std::thread::spawn(move || {
        if let Some(mut stderr) = stderr {
            let mut buffer = String::new();
            let _ = stderr.read_to_string(&mut buffer);
            let _ = stderr_tx.send(buffer);
        }
    });

    let mut stdin = child.stdin.take();
    #[allow(unused_assignments)]
    let mut exit_status = None;
    loop {
        if stop_flag.load(Ordering::SeqCst) {
            if let Some(mut input) = stdin.take() {
                let _ = input.write_all(b"q");
            }
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                exit_status = Some(status);
                break;
            }
            Ok(None) => {}
            Err(err) => {
                return Err(format!("FFmpeg运行失败: {}", err));
            }
        }

        std::thread::sleep(Duration::from_millis(500));
    }

    let status = match exit_status {
        Some(status) => status,
        None => child
            .wait()
            .map_err(|err| format!("等待FFmpeg退出失败: {}", err))?,
    };
    let stderr_output = stderr_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap_or_default();

    let file_size = std::fs::metadata(file_path)
        .map(|meta| meta.len())
        .unwrap_or(0);
    let end_time = now_rfc3339();
    let mut record_status = if stop_flag.load(Ordering::SeqCst) {
        "STOPPED"
    } else if status.success() {
        "COMPLETED"
    } else {
        "FAILED"
    };
    let mut error_message = stderr_output.trim().to_string();
    if !stop_flag.load(Ordering::SeqCst) && !status.success() {
        if let Ok(info) =
            tauri::async_runtime::block_on(fetch_room_info(&context.bilibili, room_id))
        {
            if info.live_status != 1 {
                record_status = "COMPLETED";
            }
        }
    }
    if record_status != "FAILED" {
        error_message.clear();
    }

    update_record_task(
        &context.db,
        record_id,
        record_status,
        Some(end_time.clone()),
        file_size,
        if error_message.is_empty() {
            None
        } else {
            Some(error_message.as_str())
        },
    )?;
    if let Some(path) = metadata_path.as_ref() {
        if let Err(err) = update_metadata_file(path, &end_time, file_size) {
            append_log(
                context.app_log_path.as_ref(),
                &format!(
                    "record_metadata_update_failed record_id={} err={}",
                    record_id, err
                ),
            );
        }
    }
    if record_status == "COMPLETED" {
        if let Err(err) =
            baidu_sync::enqueue_live_sync(&context.db, context.app_log_path.as_ref(), record_id)
        {
            append_log(
                context.app_log_path.as_ref(),
                &format!(
                    "baidu_sync_enqueue_fail record_id={} err={}",
                    record_id, err
                ),
            );
        }
    }
    Ok(())
}

fn summarize_stream_url(url: &str) -> String {
    if let Ok(parsed) = Url::parse(url) {
        let host = parsed.host_str().unwrap_or("-");
        let path = parsed.path();
        let mut expires = String::from("-");
        let mut tx_time = String::from("-");
        let mut ws_time = String::from("-");
        for (key, value) in parsed.query_pairs() {
            if key == "expires" || key == "expire" {
                expires = value.to_string();
            } else if key == "txTime" {
                tx_time = value.to_string();
            } else if key == "wsTime" {
                ws_time = value.to_string();
            }
        }
        return format!(
            "host={} path={} expires={} txTime={} wsTime={}",
            host, path, expires, tx_time, ws_time
        );
    }
    "host=- path=- expires=- txTime=- wsTime=-".to_string()
}

fn parse_stream_expire_value(value: &str) -> Option<u64> {
    if value.is_empty() {
        return None;
    }
    if value.chars().all(|ch| ch.is_ascii_digit()) {
        value.parse::<u64>().ok()
    } else {
        u64::from_str_radix(value, 16)
            .ok()
            .or_else(|| value.parse::<u64>().ok())
    }
}

fn stream_url_expire_at(url: &str) -> Option<u64> {
    let parsed = Url::parse(url).ok()?;
    let mut result: Option<u64> = None;
    for (key, value) in parsed.query_pairs() {
        let key = key.as_ref();
        if key == "expires"
            || key == "expire"
            || key == "deadline"
            || key == "txTime"
            || key == "wsTime"
        {
            if let Some(ts) = parse_stream_expire_value(value.as_ref()) {
                result = Some(result.map_or(ts, |prev| prev.min(ts)));
            }
        }
    }
    result
}

fn should_refresh_stream_url(url: &str, lead_secs: u64) -> Option<(u64, u64)> {
    let expire = stream_url_expire_at(url)?;
    let now = Utc::now().timestamp();
    if now < 0 {
        return None;
    }
    let now = now as u64;
    if expire <= now.saturating_add(lead_secs) {
        Some((expire, now))
    } else {
        None
    }
}

fn mark_force_no_qn(
    force_no_qn_until: &mut Option<i64>,
    settings: &LiveSettings,
    log_path: &Path,
    room_id: &str,
    reason: &str,
) {
    if settings.stream_retry_no_qn_sec <= 0 {
        return;
    }
    let now = Utc::now().timestamp();
    let until = now + settings.stream_retry_no_qn_sec.max(1);
    *force_no_qn_until = Some(until);
    append_log(
        log_path,
        &format!(
            "stream_force_no_qn room={} reason={} until={}",
            room_id, reason, until
        ),
    );
}

fn parse_quality(value: &str) -> i64 {
    for part in value.split(',') {
        let digits: String = part.chars().filter(|ch| ch.is_ascii_digit()).collect();
        if let Ok(qn) = digits.parse::<i64>() {
            if qn > 0 {
                return qn;
            }
        }
    }
    10000
}

fn value_to_i64(value: &Value) -> Option<i64> {
    if let Some(raw) = value.as_i64() {
        return Some(raw);
    }
    if let Some(raw) = value.as_u64() {
        return i64::try_from(raw).ok();
    }
    value.as_str().and_then(|raw| raw.parse::<i64>().ok())
}

pub async fn fetch_room_info(
    client: &BilibiliClient,
    room_id: &str,
) -> Result<LiveRoomInfo, String> {
    let params = vec![
        ("room_id".to_string(), room_id.to_string()),
        ("web_location".to_string(), "444.8".to_string()),
    ];
    let data = client
        .get_json(
            "https://api.live.bilibili.com/xlive/web-room/v1/index/getInfoByRoom",
            &params,
            None,
            true,
        )
        .await?;
    let room_info = data.get("room_info").ok_or("缺少直播间信息 room_info")?;
    let room_id = room_info
        .get("room_id")
        .and_then(value_to_i64)
        .map(|value| value.to_string())
        .unwrap_or_else(|| room_id.to_string());
    let uid = room_info
        .get("uid")
        .and_then(value_to_i64)
        .map(|value| value.to_string())
        .unwrap_or_else(|| "0".to_string());
    let live_status = room_info
        .get("live_status")
        .and_then(value_to_i64)
        .unwrap_or(0);
    let title = room_info
        .get("title")
        .and_then(|value| value.as_str())
        .unwrap_or("直播标题")
        .to_string();
    let cover = room_info
        .get("cover")
        .and_then(|value| value.as_str())
        .or_else(|| room_info.get("keyframe").and_then(|value| value.as_str()))
        .or_else(|| room_info.get("user_cover").and_then(|value| value.as_str()))
        .map(|value| value.to_string());
    let area_name = room_info
        .get("area_name")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string());
    let parent_area_name = room_info
        .get("parent_area_name")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string());

    Ok(LiveRoomInfo {
        room_id,
        uid,
        live_status,
        title,
        cover,
        area_name,
        parent_area_name,
    })
}

struct DanmakuWriter {
    live_runtime: Arc<LiveRuntime>,
    runtime_room_id: String,
    fallback_path: String,
    current_path: Option<String>,
    file: Option<File>,
}

impl DanmakuWriter {
    fn new(live_runtime: Arc<LiveRuntime>, runtime_room_id: String, fallback_path: String) -> Self {
        Self {
            live_runtime,
            runtime_room_id,
            fallback_path,
            current_path: None,
            file: None,
        }
    }

    fn ensure_file(&mut self) -> Result<(), String> {
        let mut candidates = Vec::new();
        if let Some(info) = self.live_runtime.get_record_info(&self.runtime_room_id) {
            if !info.file_path.trim().is_empty() {
                candidates.push(info.file_path);
            }
        }
        if !self.fallback_path.trim().is_empty() {
            candidates.push(self.fallback_path.clone());
        }
        candidates.dedup();

        let mut last_error: Option<String> = None;
        for candidate in candidates {
            let target_path = Path::new(&candidate)
                .with_extension("danmaku.jsonl")
                .to_string_lossy()
                .to_string();
            if self.current_path.as_deref() == Some(target_path.as_str()) {
                return Ok(());
            }
            if let Some(parent) = Path::new(&target_path).parent() {
                if !parent.as_os_str().is_empty() {
                    if let Err(err) = std::fs::create_dir_all(parent) {
                        last_error =
                            Some(format!("创建弹幕目录失败: {} path={}", err, target_path));
                        continue;
                    }
                }
            }
            match OpenOptions::new()
                .create(true)
                .append(true)
                .open(&target_path)
            {
                Ok(file) => {
                    self.current_path = Some(target_path);
                    self.file = Some(file);
                    return Ok(());
                }
                Err(err) => {
                    last_error = Some(format!("创建弹幕文件失败: {} path={}", err, target_path));
                }
            }
        }
        Err(last_error.unwrap_or_else(|| "弹幕文件路径为空".to_string()))
    }

    fn write_line(&mut self, line: &str) -> Result<(), String> {
        self.ensure_file()?;
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| "弹幕文件未就绪".to_string())?;
        writeln!(file, "{}", line).map_err(|err| format!("写入弹幕失败: {}", err))?;
        Ok(())
    }
}

async fn run_danmaku_loop(
    context: LiveContext,
    runtime_room_id: String,
    danmaku_room_id: String,
    record_file: String,
    settings: LiveSettings,
    stop_flag: Arc<AtomicBool>,
) -> Result<(), String> {
    if !should_record_danmaku(&settings) {
        return Ok(());
    }

    let writer = Arc::new(Mutex::new(DanmakuWriter::new(
        Arc::clone(&context.live_runtime),
        runtime_room_id.clone(),
        record_file,
    )));
    {
        let mut writer_guard = writer.lock().map_err(|_| "弹幕文件锁定失败")?;
        if let Err(err) = writer_guard.ensure_file() {
            append_log(
                &context.app_log_path,
                &format!(
                    "danmaku_file_prepare_failed room={} err={}",
                    runtime_room_id, err
                ),
            );
        } else if let Some(path) = writer_guard.current_path.as_ref() {
            append_log(
                &context.app_log_path,
                &format!(
                    "danmaku_file_prepare_ok room={} path={}",
                    runtime_room_id, path
                ),
            );
        }
    }

    let auth = context
        .login_store
        .load_primary_auth_info(&context.db)
        .ok()
        .flatten();
    let uid = auth.as_ref().and_then(|info| info.user_id).unwrap_or(0);
    loop {
        if stop_flag.load(Ordering::SeqCst) {
            break;
        }
        let danmaku_info =
            match fetch_danmaku_info(&context.bilibili, &danmaku_room_id, auth.as_ref()).await {
                Ok(info) => info,
                Err(err) => {
                    append_log(
                        &context.app_log_path,
                        &format!("danmaku_info_error room={} err={}", runtime_room_id, err),
                    );
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
            };
        let host = danmaku_info
            .get("host_list")
            .and_then(|value| value.as_array())
            .and_then(|list| list.first())
            .cloned()
            .unwrap_or(Value::Null);
        let host_name = host
            .get("host")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let wss_port = host
            .get("wss_port")
            .and_then(|value| value.as_i64())
            .unwrap_or(0);
        let ws_port = host
            .get("ws_port")
            .and_then(|value| value.as_i64())
            .unwrap_or(0);
        let tcp_port = host
            .get("port")
            .and_then(|value| value.as_i64())
            .unwrap_or(0);
        let token = danmaku_info
            .get("token")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let transport = settings.danmaku_transport;
        let mut buvid3 = auth
            .as_ref()
            .and_then(|info| extract_cookie_value(&info.cookie, "buvid3"));
        if buvid3.is_none() {
            buvid3 = context.bilibili.cached_buvid3();
        }

        let url = match transport {
            1 => format!("tcp://{}:{}", host_name, tcp_port),
            2 => format!("ws://{}:{}/sub", host_name, ws_port),
            3 => format!("wss://{}:{}/sub", host_name, wss_port),
            _ => {
                if wss_port > 0 {
                    format!("wss://{}:{}/sub", host_name, wss_port)
                } else if ws_port > 0 {
                    format!("ws://{}:{}/sub", host_name, ws_port)
                } else {
                    format!("tcp://{}:{}", host_name, tcp_port)
                }
            }
        };

        let result = if url.starts_with("tcp://") {
            run_danmaku_tcp(
                &url,
                &danmaku_room_id,
                token,
                uid,
                buvid3.clone(),
                &settings,
                &stop_flag,
                &writer,
            )
            .await
        } else {
            run_danmaku_ws(
                &url,
                &danmaku_room_id,
                token,
                uid,
                buvid3.clone(),
                &settings,
                &stop_flag,
                &writer,
            )
            .await
        };

        if result.is_err() {
            append_log(
                &context.app_log_path,
                &format!(
                    "danmaku_error room={} err={}",
                    runtime_room_id,
                    result.clone().unwrap_err()
                ),
            );
        }

        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    Ok(())
}

fn should_record_danmaku(settings: &LiveSettings) -> bool {
    settings.record_danmaku
        || settings.record_danmaku_raw
        || settings.record_danmaku_superchat
        || settings.record_danmaku_gift
        || settings.record_danmaku_guard
}

async fn fetch_danmaku_info(
    client: &BilibiliClient,
    room_id: &str,
    auth: Option<&AuthInfo>,
) -> Result<Value, String> {
    let params = vec![
        ("id".to_string(), room_id.to_string()),
        ("type".to_string(), "0".to_string()),
        ("web_location".to_string(), "444.8".to_string()),
    ];
    client
        .get_json(
            "https://api.live.bilibili.com/xlive/web-room/v1/index/getDanmuInfo",
            &params,
            auth,
            true,
        )
        .await
}

async fn run_danmaku_ws(
    url: &str,
    room_id: &str,
    token: &str,
    uid: i64,
    buvid3: Option<String>,
    settings: &LiveSettings,
    stop_flag: &Arc<AtomicBool>,
    output: &Arc<Mutex<DanmakuWriter>>,
) -> Result<(), String> {
    let (ws_stream, _) = tokio_tungstenite::connect_async(url)
        .await
        .map_err(|err| format!("连接弹幕失败: {}", err))?;
    let (mut write, mut read) = ws_stream.split();
    let auth_packet =
        build_danmaku_packet(7, build_danmaku_auth_payload(room_id, token, uid, buvid3));
    write
        .send(Message::Binary(auth_packet))
        .await
        .map_err(|err| format!("弹幕鉴权失败: {}", err))?;

    let mut heartbeat = tokio::time::interval(Duration::from_secs(30));
    loop {
        if stop_flag.load(Ordering::SeqCst) {
            break;
        }

        tokio::select! {
          _ = heartbeat.tick() => {
            let packet = build_danmaku_packet(2, Vec::new());
            let _ = write.send(Message::Binary(packet)).await;
          }
          msg = read.next() => {
            match msg {
              Some(Ok(Message::Binary(data))) => {
                handle_danmaku_payload(&data, settings, output)?;
              }
              Some(Ok(_)) => {}
              Some(Err(err)) => return Err(format!("弹幕读取失败: {}", err)),
              None => break,
            }
          }
        }
    }
    Ok(())
}

async fn run_danmaku_tcp(
    url: &str,
    room_id: &str,
    token: &str,
    uid: i64,
    buvid3: Option<String>,
    settings: &LiveSettings,
    stop_flag: &Arc<AtomicBool>,
    output: &Arc<Mutex<DanmakuWriter>>,
) -> Result<(), String> {
    let addr = url.trim_start_matches("tcp://");
    let mut stream = tokio::net::TcpStream::connect(addr)
        .await
        .map_err(|err| format!("连接弹幕失败: {}", err))?;

    let auth_packet =
        build_danmaku_packet(7, build_danmaku_auth_payload(room_id, token, uid, buvid3));
    stream
        .write_all(&auth_packet)
        .await
        .map_err(|err| format!("弹幕鉴权失败: {}", err))?;

    let mut heartbeat = tokio::time::interval(Duration::from_secs(30));
    let mut buffer = vec![0u8; 16];
    loop {
        if stop_flag.load(Ordering::SeqCst) {
            break;
        }

        tokio::select! {
          _ = heartbeat.tick() => {
            let packet = build_danmaku_packet(2, Vec::new());
            let _ = stream.write_all(&packet).await;
          }
          read = stream.read_exact(&mut buffer) => {
            if read.is_err() {
              break;
            }
            let packet_len = u32::from_be_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]) as usize;
            let header_len = u16::from_be_bytes([buffer[4], buffer[5]]) as usize;
            let mut body = vec![0u8; packet_len - header_len];
            stream.read_exact(&mut body).await.map_err(|err| format!("读取弹幕失败: {}", err))?;
            let mut full = Vec::with_capacity(packet_len);
            full.extend_from_slice(&buffer);
            full.extend_from_slice(&body);
            handle_danmaku_payload(&full, settings, output)?;
          }
        }
    }

    Ok(())
}

fn handle_danmaku_payload(
    data: &[u8],
    settings: &LiveSettings,
    output: &Arc<Mutex<DanmakuWriter>>,
) -> Result<(), String> {
    for payload in parse_danmaku_packets(data)? {
        if payload.op != 5 {
            continue;
        }
        let text = String::from_utf8_lossy(&payload.body).to_string();
        if let Ok(value) = serde_json::from_str::<Value>(&text) {
            let cmd = value
                .get("cmd")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let should_write = if settings.record_danmaku_raw {
                true
            } else {
                match cmd {
                    "DANMU_MSG" => settings.record_danmaku,
                    "SUPER_CHAT_MESSAGE" | "SUPER_CHAT_MESSAGE_JPN" => {
                        settings.record_danmaku_superchat
                    }
                    "SEND_GIFT" => settings.record_danmaku_gift,
                    "GUARD_BUY" | "USER_TOAST_MSG" => settings.record_danmaku_guard,
                    _ => false,
                }
            };
            if should_write {
                let line = serde_json::json!({
                  "cmd": cmd,
                  "data": value,
                  "timestamp": now_rfc3339(),
                });
                let mut writer = output.lock().map_err(|_| "弹幕文件锁定失败")?;
                writer.write_line(&line.to_string())?;
            }
        } else if settings.record_danmaku_raw {
            let mut writer = output.lock().map_err(|_| "弹幕文件锁定失败")?;
            writer.write_line(&text)?;
        }
    }
    Ok(())
}

struct DanmakuPacket {
    op: u32,
    #[allow(dead_code)]
    version: u16,
    body: Vec<u8>,
}

fn parse_danmaku_packets(data: &[u8]) -> Result<Vec<DanmakuPacket>, String> {
    let mut packets = Vec::new();
    let mut offset = 0usize;
    while offset + 16 <= data.len() {
        let packet_len = u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        let header_len =
            u16::from_be_bytes(data[offset + 4..offset + 6].try_into().unwrap()) as usize;
        let version = u16::from_be_bytes(data[offset + 6..offset + 8].try_into().unwrap());
        let op = u32::from_be_bytes(data[offset + 8..offset + 12].try_into().unwrap());
        let body_start = offset + header_len;
        let body_end = offset + packet_len;
        if body_end > data.len() || body_start > data.len() {
            break;
        }
        let body = data[body_start..body_end].to_vec();
        if version == 2 {
            let decompressed = decompress_zlib(&body)?;
            let inner = parse_danmaku_packets(&decompressed)?;
            packets.extend(inner);
        } else if version == 3 {
            let decompressed = decompress_brotli(&body)?;
            let inner = parse_danmaku_packets(&decompressed)?;
            packets.extend(inner);
        } else {
            packets.push(DanmakuPacket { op, version, body });
        }
        offset += packet_len;
    }
    Ok(packets)
}

fn decompress_zlib(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut decoder = flate2::read::ZlibDecoder::new(data);
    let mut output = Vec::new();
    decoder
        .read_to_end(&mut output)
        .map_err(|err| format!("zlib 解压失败: {}", err))?;
    Ok(output)
}

fn decompress_brotli(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut decoder = brotli::Decompressor::new(data, 4096);
    let mut output = Vec::new();
    decoder
        .read_to_end(&mut output)
        .map_err(|err| format!("brotli 解压失败: {}", err))?;
    Ok(output)
}

fn build_danmaku_auth_payload(
    room_id: &str,
    token: &str,
    uid: i64,
    buvid3: Option<String>,
) -> Vec<u8> {
    let mut payload = serde_json::Map::new();
    payload.insert(
        "uid".to_string(),
        Value::Number(serde_json::Number::from(uid)),
    );
    payload.insert(
        "roomid".to_string(),
        Value::Number(serde_json::Number::from(
            room_id.parse::<i64>().unwrap_or(0),
        )),
    );
    payload.insert(
        "protover".to_string(),
        Value::Number(serde_json::Number::from(3)),
    );
    payload.insert("platform".to_string(), Value::String("web".to_string()));
    payload.insert(
        "type".to_string(),
        Value::Number(serde_json::Number::from(2)),
    );
    payload.insert("key".to_string(), Value::String(token.to_string()));
    if let Some(buvid3) = buvid3 {
        payload.insert("buvid".to_string(), Value::String(buvid3));
    }
    Value::Object(payload).to_string().into_bytes()
}

fn extract_cookie_value(cookie: &str, key: &str) -> Option<String> {
    let needle = format!("{}=", key);
    cookie.split(';').find_map(|item| {
        let part = item.trim();
        part.strip_prefix(&needle).map(|value| value.to_string())
    })
}

fn build_danmaku_packet(op: u32, body: Vec<u8>) -> Vec<u8> {
    let header_len = 16u16;
    let packet_len = header_len as u32 + body.len() as u32;
    let mut buf = Vec::with_capacity(packet_len as usize);
    buf.extend_from_slice(&packet_len.to_be_bytes());
    buf.extend_from_slice(&header_len.to_be_bytes());
    buf.extend_from_slice(&1u16.to_be_bytes());
    buf.extend_from_slice(&op.to_be_bytes());
    buf.extend_from_slice(&1u32.to_be_bytes());
    buf.extend_from_slice(&body);
    buf
}
