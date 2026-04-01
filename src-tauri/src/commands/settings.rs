use chrono::Utc;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use tauri::Manager;
use tauri::State;

use crate::api::ApiResponse;
use crate::config::default_download_dir;
use crate::db::Db;
use crate::AppState;
use std::fs;
use std::path::{Path, PathBuf};

pub const DEFAULT_THREADS: i64 = 3;
pub const DEFAULT_QUEUE_SIZE: i64 = 10;
pub const DEFAULT_UPLOAD_CONCURRENCY: i64 = 3;
pub const MAX_UPLOAD_CONCURRENCY: i64 = 5;
pub const DEFAULT_SUBMISSION_REMOTE_REFRESH_MINUTES: i64 = 10;
pub const DEFAULT_BLOCK_PCDN: bool = true;
#[allow(dead_code)]
pub const DEFAULT_ENABLE_ARIA2C: bool = true;
pub const DEFAULT_ARIA2C_CONNECTIONS: i64 = 4;
pub const DEFAULT_ARIA2C_SPLIT: i64 = 4;
pub const DEFAULT_BAIDU_MAX_PARALLEL: i64 = 3;
pub const LOG_DIR_SETTING_KEY: &str = "log_dir";
pub const LEGACY_LIVE_FILE_TEMPLATE: &str =
    "live/{{ roomId }}/录制-{{ roomId }}-{{ now }}-{{ title }}.flv";
pub const LEGACY_LIVE_FILE_TEMPLATE_DATE: &str =
    "live/{{ roomId }}/{{ date }}/录制-{{ roomId }}-{{ now }}-{{ title }}.flv";
pub const DEFAULT_LIVE_FILE_TEMPLATE: &str =
    "live/{{ roomId }}/{{ liveDate }}/录制-{{ roomId }}-{{ now }}-{{ title }}.flv";
pub const CACHE_KEY_LOG: &str = "log";
pub const CACHE_KEY_TEMP: &str = "temp";
pub const CACHE_KEY_DATABASE: &str = "database";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadSettings {
    pub threads: i64,
    pub queue_size: i64,
    pub download_path: String,
    pub log_dir: String,
    pub upload_concurrency: i64,
    pub submission_remote_refresh_minutes: i64,
    pub block_pcdn: bool,
    pub enable_aria2c: bool,
    pub aria2c_connections: i64,
    pub aria2c_split: i64,
    pub baidu_max_parallel: i64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveSettings {
    pub file_name_template: String,
    pub record_path: String,
    pub write_metadata: bool,
    pub save_cover: bool,
    pub recording_quality: String,
    pub record_mode: i64,
    pub cutting_mode: i64,
    pub cutting_number: i64,
    pub cutting_by_title: bool,
    pub title_split_min_seconds: i64,
    pub danmaku_transport: i64,
    pub record_danmaku: bool,
    pub record_danmaku_raw: bool,
    pub record_danmaku_superchat: bool,
    pub record_danmaku_gift: bool,
    pub record_danmaku_guard: bool,
    pub stream_retry_ms: i64,
    pub stream_retry_no_qn_sec: i64,
    pub stream_connect_timeout_ms: i64,
    pub stream_read_timeout_ms: i64,
    pub check_interval_sec: i64,
    pub flv_fix_split_on_missing: bool,
    pub flv_fix_adjust_timestamp_jump: bool,
    pub flv_fix_split_on_timestamp_jump: bool,
    pub flv_fix_disable_on_annexb: bool,
    pub baidu_sync_enabled: bool,
    pub baidu_sync_path: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheItemInfo {
    pub key: String,
    pub path: String,
    pub size_bytes: u64,
    pub exists: bool,
}

#[tauri::command]
pub fn get_download_settings(state: State<'_, AppState>) -> ApiResponse<DownloadSettings> {
    match load_download_settings_from_db(&state.db) {
        Ok(settings) => ApiResponse::success(settings),
        Err(err) => ApiResponse::error(format!("Failed to load download settings: {}", err)),
    }
}

#[tauri::command]
pub fn get_live_settings(state: State<'_, AppState>) -> ApiResponse<LiveSettings> {
    match load_live_settings_from_db(&state.db) {
        Ok(settings) => ApiResponse::success(settings),
        Err(err) => ApiResponse::error(format!("Failed to load live settings: {}", err)),
    }
}

#[tauri::command]
pub fn settings_cache_overview(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> ApiResponse<Vec<CacheItemInfo>> {
    match build_cache_overview(state.inner(), &app) {
        Ok(items) => ApiResponse::success(items),
        Err(err) => ApiResponse::error(err),
    }
}

#[tauri::command]
pub fn settings_cache_clear(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    key: String,
) -> ApiResponse<String> {
    let normalized = key.trim().to_lowercase();
    let result = match normalized.as_str() {
        CACHE_KEY_DATABASE => compact_database_cache(state.inner(), &app),
        _ => resolve_cache_path_by_key(state.inner(), &app, &normalized)
            .and_then(|path| clear_path_children(path.as_path())),
    };
    match result {
        Ok(()) => ApiResponse::success("清理完成".to_string()),
        Err(err) => ApiResponse::error(err),
    }
}

#[tauri::command]
pub fn settings_cache_open(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    key: String,
) -> ApiResponse<String> {
    let normalized = key.trim().to_lowercase();
    let result = resolve_cache_path_by_key(state.inner(), &app, &normalized).and_then(|path| {
        if !path.exists() {
            return Err(format!("缓存路径不存在: {}", path.to_string_lossy()));
        }
        tauri_plugin_opener::open_path(path, None::<&str>)
            .map_err(|err| format!("打开缓存路径失败: {}", err))
    });
    match result {
        Ok(()) => ApiResponse::success("已打开".to_string()),
        Err(err) => ApiResponse::error(err),
    }
}

#[tauri::command]
pub fn update_download_settings(
    state: State<'_, AppState>,
    threads: i64,
    queue_size: i64,
    download_path: String,
    log_dir: String,
    upload_concurrency: i64,
    submission_remote_refresh_minutes: i64,
    block_pcdn: bool,
    aria2c_connections: i64,
    aria2c_split: i64,
    baidu_max_parallel: i64,
    _enable_aria2c: bool,
) -> ApiResponse<DownloadSettings> {
    if threads <= 0
        || queue_size <= 0
        || submission_remote_refresh_minutes <= 0
        || aria2c_connections <= 0
        || aria2c_split <= 0
        || baidu_max_parallel <= 0
    {
        return ApiResponse::error("Values must be greater than 0");
    }
    if upload_concurrency <= 0 || upload_concurrency > MAX_UPLOAD_CONCURRENCY {
        return ApiResponse::error("投稿并发上传数需在 1-5 之间");
    }

    let normalized_path = if download_path.trim().is_empty() {
        default_download_dir().to_string_lossy().to_string()
    } else {
        download_path.trim().to_string()
    };
    let normalized_log_dir = if log_dir.trim().is_empty() {
        std::path::PathBuf::from(&normalized_path)
            .join("log")
            .to_string_lossy()
            .to_string()
    } else {
        log_dir.trim().to_string()
    };
    let normalized_aria2c_connections = aria2c_connections.clamp(1, 32);
    let normalized_aria2c_split = aria2c_split.clamp(1, 32);
    let normalized_baidu_max_parallel = baidu_max_parallel.clamp(1, 100);

    let now = Utc::now().to_rfc3339();
    let enable_aria2c = true;
    let result = state.db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO app_settings (key, value, updated_at) VALUES (?1, ?2, ?3) \
       ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            ("download_threads", threads.to_string(), &now),
        )?;
        conn.execute(
            "INSERT INTO app_settings (key, value, updated_at) VALUES (?1, ?2, ?3) \
       ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            ("download_queue_size", queue_size.to_string(), &now),
        )?;
        conn.execute(
            "INSERT INTO app_settings (key, value, updated_at) VALUES (?1, ?2, ?3) \
       ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            ("download_path", &normalized_path, &now),
        )?;
        conn.execute(
            "INSERT INTO app_settings (key, value, updated_at) VALUES (?1, ?2, ?3) \
       ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            (LOG_DIR_SETTING_KEY, &normalized_log_dir, &now),
        )?;
        conn.execute(
            "INSERT INTO app_settings (key, value, updated_at) VALUES (?1, ?2, ?3) \
       ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            (
                "submission_upload_concurrency",
                upload_concurrency.to_string(),
                &now,
            ),
        )?;
        conn.execute(
            "INSERT INTO app_settings (key, value, updated_at) VALUES (?1, ?2, ?3) \
       ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            (
                "submission_remote_refresh_minutes",
                submission_remote_refresh_minutes.to_string(),
                &now,
            ),
        )?;
        conn.execute(
            "INSERT INTO app_settings (key, value, updated_at) VALUES (?1, ?2, ?3) \
       ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            (
                "download_block_pcdn",
                if block_pcdn { "1" } else { "0" },
                &now,
            ),
        )?;
        conn.execute(
            "INSERT INTO app_settings (key, value, updated_at) VALUES (?1, ?2, ?3) \
       ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            ("download_enable_aria2c", "1", &now),
        )?;
        conn.execute(
            "INSERT INTO app_settings (key, value, updated_at) VALUES (?1, ?2, ?3) \
       ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            (
                "download_aria2c_connections",
                normalized_aria2c_connections.to_string(),
                &now,
            ),
        )?;
        conn.execute(
            "INSERT INTO app_settings (key, value, updated_at) VALUES (?1, ?2, ?3) \
       ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            (
                "download_aria2c_split",
                normalized_aria2c_split.to_string(),
                &now,
            ),
        )?;
        conn.execute(
            "INSERT INTO app_settings (key, value, updated_at) VALUES (?1, ?2, ?3) \
       ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            (
                "download_baidu_max_parallel",
                normalized_baidu_max_parallel.to_string(),
                &now,
            ),
        )?;
        Ok(())
    });

    if let Err(err) = result {
        return ApiResponse::error(format!("Failed to update download settings: {}", err));
    }

    ApiResponse::success(DownloadSettings {
        threads,
        queue_size,
        download_path: normalized_path,
        log_dir: normalized_log_dir,
        upload_concurrency,
        submission_remote_refresh_minutes,
        block_pcdn,
        enable_aria2c,
        aria2c_connections: normalized_aria2c_connections,
        aria2c_split: normalized_aria2c_split,
        baidu_max_parallel: normalized_baidu_max_parallel,
    })
}

#[tauri::command]
pub fn update_live_settings(
    state: State<'_, AppState>,
    payload: LiveSettings,
) -> ApiResponse<LiveSettings> {
    let now = Utc::now().to_rfc3339();
    let result = state.db.with_conn(|conn| {
    conn.execute(
      "INSERT INTO live_settings (id, file_name_template, record_path, write_metadata, save_cover, recording_quality, record_mode, cutting_mode, cutting_number, cutting_by_title, title_split_min_seconds, danmaku_transport, record_danmaku, record_danmaku_raw, record_danmaku_superchat, record_danmaku_gift, record_danmaku_guard, stream_retry_ms, stream_retry_no_qn_sec, stream_connect_timeout_ms, stream_read_timeout_ms, check_interval_sec, flv_fix_split_on_missing, flv_fix_adjust_timestamp_jump, flv_fix_split_on_timestamp_jump, flv_fix_disable_on_annexb, baidu_sync_enabled, baidu_sync_path, create_time, update_time) \
       VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29) \
       ON CONFLICT(id) DO UPDATE SET \
       file_name_template = excluded.file_name_template, \
       record_path = excluded.record_path, \
       write_metadata = excluded.write_metadata, \
       save_cover = excluded.save_cover, \
       recording_quality = excluded.recording_quality, \
       record_mode = excluded.record_mode, \
       cutting_mode = excluded.cutting_mode, \
       cutting_number = excluded.cutting_number, \
       cutting_by_title = excluded.cutting_by_title, \
       title_split_min_seconds = excluded.title_split_min_seconds, \
       danmaku_transport = excluded.danmaku_transport, \
       record_danmaku = excluded.record_danmaku, \
       record_danmaku_raw = excluded.record_danmaku_raw, \
       record_danmaku_superchat = excluded.record_danmaku_superchat, \
       record_danmaku_gift = excluded.record_danmaku_gift, \
       record_danmaku_guard = excluded.record_danmaku_guard, \
       stream_retry_ms = excluded.stream_retry_ms, \
       stream_retry_no_qn_sec = excluded.stream_retry_no_qn_sec, \
       stream_connect_timeout_ms = excluded.stream_connect_timeout_ms, \
       stream_read_timeout_ms = excluded.stream_read_timeout_ms, \
       check_interval_sec = excluded.check_interval_sec, \
       flv_fix_split_on_missing = excluded.flv_fix_split_on_missing, \
       flv_fix_adjust_timestamp_jump = excluded.flv_fix_adjust_timestamp_jump, \
       flv_fix_split_on_timestamp_jump = excluded.flv_fix_split_on_timestamp_jump, \
       flv_fix_disable_on_annexb = excluded.flv_fix_disable_on_annexb, \
       baidu_sync_enabled = excluded.baidu_sync_enabled, \
       baidu_sync_path = excluded.baidu_sync_path, \
       update_time = excluded.update_time",
      params![
        payload.file_name_template.as_str(),
        payload.record_path.as_str(),
        payload.write_metadata as i64,
        payload.save_cover as i64,
        payload.recording_quality.as_str(),
        payload.record_mode,
        payload.cutting_mode,
        payload.cutting_number,
        payload.cutting_by_title as i64,
        payload.title_split_min_seconds,
        payload.danmaku_transport,
        payload.record_danmaku as i64,
        payload.record_danmaku_raw as i64,
        payload.record_danmaku_superchat as i64,
        payload.record_danmaku_gift as i64,
        payload.record_danmaku_guard as i64,
        payload.stream_retry_ms,
        payload.stream_retry_no_qn_sec,
        payload.stream_connect_timeout_ms,
        payload.stream_read_timeout_ms,
        payload.check_interval_sec,
        payload.flv_fix_split_on_missing as i64,
        payload.flv_fix_adjust_timestamp_jump as i64,
        payload.flv_fix_split_on_timestamp_jump as i64,
        payload.flv_fix_disable_on_annexb as i64,
        payload.baidu_sync_enabled as i64,
        payload.baidu_sync_path.as_str(),
        &now,
        &now,
      ],
    )?;
    Ok(())
  });

    if let Err(err) = result {
        return ApiResponse::error(format!("Failed to update live settings: {}", err));
    }

    ApiResponse::success(payload)
}

pub fn load_download_settings_from_db(db: &Db) -> Result<DownloadSettings, crate::db::DbError> {
    db.with_conn(|conn| {
        let threads: Option<String> = conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'download_threads'",
                [],
                |row| row.get(0),
            )
            .ok();
        let queue_size: Option<String> = conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'download_queue_size'",
                [],
                |row| row.get(0),
            )
            .ok();
        let download_path: Option<String> = conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'download_path'",
                [],
                |row| row.get(0),
            )
            .ok();
        let upload_concurrency: Option<String> = conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'submission_upload_concurrency'",
                [],
                |row| row.get(0),
            )
            .ok();
        let submission_remote_refresh_minutes: Option<String> = conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'submission_remote_refresh_minutes'",
                [],
                |row| row.get(0),
            )
            .ok();
        let block_pcdn: Option<String> = conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'download_block_pcdn'",
                [],
                |row| row.get(0),
            )
            .ok();
        let _enable_aria2c: Option<String> = conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'download_enable_aria2c'",
                [],
                |row| row.get(0),
            )
            .ok();
        let aria2c_connections: Option<String> = conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'download_aria2c_connections'",
                [],
                |row| row.get(0),
            )
            .ok();
        let aria2c_split: Option<String> = conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'download_aria2c_split'",
                [],
                |row| row.get(0),
            )
            .ok();
        let baidu_max_parallel: Option<String> = conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'download_baidu_max_parallel'",
                [],
                |row| row.get(0),
            )
            .ok();
        let log_dir: Option<String> = conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = ?1",
                [LOG_DIR_SETTING_KEY],
                |row| row.get(0),
            )
            .ok();
        let enable_aria2c = true;

        let resolved_download_path =
            download_path.unwrap_or_else(|| default_download_dir().to_string_lossy().to_string());
        let resolved_log_dir = log_dir.unwrap_or_else(|| {
            std::path::PathBuf::from(&resolved_download_path)
                .join("log")
                .to_string_lossy()
                .to_string()
        });

        Ok(DownloadSettings {
            threads: threads
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or(DEFAULT_THREADS),
            queue_size: queue_size
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or(DEFAULT_QUEUE_SIZE),
            download_path: resolved_download_path,
            log_dir: resolved_log_dir,
            upload_concurrency: upload_concurrency
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or(DEFAULT_UPLOAD_CONCURRENCY)
                .clamp(1, MAX_UPLOAD_CONCURRENCY),
            submission_remote_refresh_minutes: submission_remote_refresh_minutes
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or(DEFAULT_SUBMISSION_REMOTE_REFRESH_MINUTES)
                .max(1),
            block_pcdn: block_pcdn
                .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
                .unwrap_or(DEFAULT_BLOCK_PCDN),
            enable_aria2c,
            aria2c_connections: aria2c_connections
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or(DEFAULT_ARIA2C_CONNECTIONS)
                .clamp(1, 32),
            aria2c_split: aria2c_split
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or(DEFAULT_ARIA2C_SPLIT)
                .clamp(1, 32),
            baidu_max_parallel: baidu_max_parallel
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or(DEFAULT_BAIDU_MAX_PARALLEL)
                .clamp(1, 100),
        })
    })
}

pub fn ensure_log_dir(db: &Db, download_dir: &std::path::Path) -> String {
    let fallback_value = download_dir.join("log").to_string_lossy().to_string();
    let (current, needs_update) = db
        .with_conn(|conn| {
            let value: Option<String> = conn
                .query_row(
                    "SELECT value FROM app_settings WHERE key = ?1",
                    [LOG_DIR_SETTING_KEY],
                    |row| row.get(0),
                )
                .ok();
            let value = value.filter(|item| !item.trim().is_empty());
            let needs_update = value.is_none();
            Ok((value, needs_update))
        })
        .unwrap_or((None, true));
    let resolved = current.unwrap_or_else(|| fallback_value.clone());
    if needs_update {
        let now = Utc::now().to_rfc3339();
        let _ = db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO app_settings (key, value, updated_at) VALUES (?1, ?2, ?3) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                (LOG_DIR_SETTING_KEY, &resolved, &now),
            )?;
            Ok(())
        });
    }
    let _ = std::fs::create_dir_all(&resolved);
    resolved
}

pub fn load_live_settings_from_db(db: &Db) -> Result<LiveSettings, crate::db::DbError> {
    db.with_conn(|conn| {
    let mut stmt = conn.prepare(
      "SELECT file_name_template, record_path, write_metadata, save_cover, recording_quality, record_mode, cutting_mode, cutting_number, cutting_by_title, title_split_min_seconds, danmaku_transport, record_danmaku, record_danmaku_raw, record_danmaku_superchat, record_danmaku_gift, record_danmaku_guard, stream_retry_ms, stream_retry_no_qn_sec, stream_connect_timeout_ms, stream_read_timeout_ms, check_interval_sec, flv_fix_split_on_missing, flv_fix_adjust_timestamp_jump, flv_fix_split_on_timestamp_jump, flv_fix_disable_on_annexb, baidu_sync_enabled, baidu_sync_path \
       FROM live_settings WHERE id = 1",
    )?;

    let result = stmt.query_row([], |row| {
      Ok(LiveSettings {
        file_name_template: row.get(0)?,
        record_path: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
        write_metadata: row.get::<_, i64>(2)? != 0,
        save_cover: row.get::<_, i64>(3)? != 0,
        recording_quality: row.get(4)?,
        record_mode: row.get(5)?,
        cutting_mode: row.get(6)?,
        cutting_number: row.get(7)?,
        cutting_by_title: row.get::<_, i64>(8)? != 0,
        title_split_min_seconds: row.get(9)?,
        danmaku_transport: row.get(10)?,
        record_danmaku: row.get::<_, i64>(11)? != 0,
        record_danmaku_raw: row.get::<_, i64>(12)? != 0,
        record_danmaku_superchat: row.get::<_, i64>(13)? != 0,
        record_danmaku_gift: row.get::<_, i64>(14)? != 0,
        record_danmaku_guard: row.get::<_, i64>(15)? != 0,
        stream_retry_ms: row.get(16)?,
        stream_retry_no_qn_sec: row.get(17)?,
        stream_connect_timeout_ms: row.get(18)?,
        stream_read_timeout_ms: row.get(19)?,
        check_interval_sec: row.get(20)?,
        flv_fix_split_on_missing: row.get::<_, i64>(21)? != 0,
        flv_fix_adjust_timestamp_jump: row.get::<_, i64>(22)? != 0,
        flv_fix_split_on_timestamp_jump: row.get::<_, i64>(23)? != 0,
        flv_fix_disable_on_annexb: row.get::<_, i64>(24)? != 0,
        baidu_sync_enabled: row.get::<_, i64>(25)? != 0,
        baidu_sync_path: row.get::<_, Option<String>>(26)?.unwrap_or_default(),
      })
    });

    match result {
      Ok(mut settings) => {
        let template = settings.file_name_template.trim();
        if template == LEGACY_LIVE_FILE_TEMPLATE || template == LEGACY_LIVE_FILE_TEMPLATE_DATE {
          let now = Utc::now().to_rfc3339();
          let _ = conn.execute(
            "UPDATE live_settings SET file_name_template = ?1, update_time = ?2 WHERE id = 1",
            (DEFAULT_LIVE_FILE_TEMPLATE, &now),
          );
          settings.file_name_template = DEFAULT_LIVE_FILE_TEMPLATE.to_string();
        }
        Ok(settings)
      }
      Err(_) => Ok(default_live_settings()),
    }
  })
}

pub fn default_live_settings() -> LiveSettings {
    LiveSettings {
        file_name_template: DEFAULT_LIVE_FILE_TEMPLATE.to_string(),
        record_path: String::new(),
        write_metadata: true,
        save_cover: false,
        recording_quality: "avc10000,hevc10000".to_string(),
        record_mode: 0,
        cutting_mode: 0,
        cutting_number: 100,
        cutting_by_title: false,
        title_split_min_seconds: 1800,
        danmaku_transport: 0,
        record_danmaku: false,
        record_danmaku_raw: false,
        record_danmaku_superchat: true,
        record_danmaku_gift: false,
        record_danmaku_guard: true,
        stream_retry_ms: 6000,
        stream_retry_no_qn_sec: 90,
        stream_connect_timeout_ms: 5000,
        stream_read_timeout_ms: 15000,
        check_interval_sec: 180,
        flv_fix_split_on_missing: false,
        flv_fix_adjust_timestamp_jump: false,
        flv_fix_split_on_timestamp_jump: true,
        flv_fix_disable_on_annexb: false,
        baidu_sync_enabled: false,
        baidu_sync_path: "/录播".to_string(),
    }
}

fn build_cache_overview(
    state: &AppState,
    app: &tauri::AppHandle,
) -> Result<Vec<CacheItemInfo>, String> {
    let mut items = Vec::with_capacity(3);

    let log_path = resolve_log_dir_for_cache(state)?;
    items.push(build_cache_item(CACHE_KEY_LOG, log_path.as_path()));

    let temp_path = resolve_temp_dir_for_cache(state)?;
    items.push(build_cache_item(CACHE_KEY_TEMP, temp_path.as_path()));

    let database_path = resolve_database_path(app)?;
    items.push(build_cache_item(
        CACHE_KEY_DATABASE,
        database_path.as_path(),
    ));

    Ok(items)
}

fn resolve_cache_path_by_key(
    state: &AppState,
    app: &tauri::AppHandle,
    key: &str,
) -> Result<PathBuf, String> {
    match key {
        CACHE_KEY_LOG => resolve_log_dir_for_cache(state),
        CACHE_KEY_TEMP => resolve_temp_dir_for_cache(state),
        CACHE_KEY_DATABASE => resolve_database_path(app),
        _ => Err(format!("不支持的缓存类型: {}", key)),
    }
}

fn build_cache_item(key: &str, path: &Path) -> CacheItemInfo {
    let exists = path.exists();
    CacheItemInfo {
        key: key.to_string(),
        path: path.to_string_lossy().to_string(),
        size_bytes: calculate_path_size(path),
        exists,
    }
}

fn resolve_log_dir_for_cache(state: &AppState) -> Result<PathBuf, String> {
    load_download_settings_from_db(&state.db)
        .map(|settings| PathBuf::from(settings.log_dir))
        .map_err(|err| format!("读取日志目录失败: {}", err))
}

fn resolve_temp_dir_for_cache(state: &AppState) -> Result<PathBuf, String> {
    load_download_settings_from_db(&state.db)
        .map(|settings| PathBuf::from(settings.download_path).join("temp"))
        .map_err(|err| format!("读取临时目录失败: {}", err))
}

fn resolve_database_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|path| crate::config::database_path(path.as_path()))
        .map_err(|err| format!("解析数据库路径失败: {}", err))
}

fn calculate_path_size(path: &Path) -> u64 {
    if !path.exists() {
        return 0;
    }
    if path.is_file() {
        return fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    }
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(current) else {
            continue;
        };
        for entry in entries.flatten() {
            let entry_path = entry.path();
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_file() {
                total = total.saturating_add(meta.len());
            } else if meta.is_dir() {
                stack.push(entry_path);
            }
        }
    }
    total
}

fn clear_path_children(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    if path.is_file() {
        return fs::remove_file(path).map_err(|err| format!("清理文件失败: {}", err));
    }
    let entries = fs::read_dir(path).map_err(|err| format!("读取缓存目录失败: {}", err))?;
    for entry in entries {
        let entry = entry.map_err(|err| format!("读取缓存项失败: {}", err))?;
        let entry_path = entry.path();
        let meta = entry
            .metadata()
            .map_err(|err| format!("读取缓存项元数据失败: {}", err))?;
        if meta.is_dir() {
            fs::remove_dir_all(&entry_path).map_err(|err| format!("删除缓存目录失败: {}", err))?;
        } else {
            fs::remove_file(&entry_path).map_err(|err| format!("删除缓存文件失败: {}", err))?;
        }
    }
    Ok(())
}

fn compact_database_cache(state: &AppState, app: &tauri::AppHandle) -> Result<(), String> {
    state
        .db
        .with_conn(|conn| {
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM;")?;
            Ok(())
        })
        .map_err(|err| format!("压缩数据库失败: {}", err))?;

    let db_path = resolve_database_path(app)?;
    let db_wal = PathBuf::from(format!("{}-wal", db_path.to_string_lossy()));
    let db_shm = PathBuf::from(format!("{}-shm", db_path.to_string_lossy()));
    if db_wal.exists() {
        let _ = fs::remove_file(db_wal);
    }
    if db_shm.exists() {
        let _ = fs::remove_file(db_shm);
    }
    Ok(())
}
