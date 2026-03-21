use std::collections::{HashMap, HashSet};
use std::io::{BufReader, Read};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::blocking::Client;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use rusqlite::params;
use rusqlite::OptionalExtension;
use tauri::State;
use tokio::time::{interval, sleep};
use url::Url;

use crate::api::ApiResponse;
use crate::baidu_sync;
use crate::config::{default_download_dir, resolve_aria2c_candidates};
use crate::commands::settings::load_download_settings_from_db;
use crate::ffmpeg::{run_ffmpeg, run_ffmpeg_with_progress, run_ffprobe_json};
use crate::login_store::AuthInfo;
use crate::path_store::{
  load_local_path_prefix, to_absolute_local_path, to_absolute_local_path_opt_with_prefix,
  to_absolute_local_path_with_prefix, to_stored_local_path, to_stored_local_path_with_prefix,
};
use crate::utils::{append_log, apply_no_window, build_output_path, now_rfc3339, sanitize_filename};
use crate::bilibili::client::BilibiliClient;
use crate::db::Db;
use crate::login_store::LoginStore;
use crate::AppState;

pub const DOWNLOAD_SOURCE_BILIBILI: &str = "BILIBILI";
pub const DOWNLOAD_SOURCE_BAIDU: &str = "BAIDU";
const BAIDU_DOWNLOAD_SUFFIX: &str = ".BaiduPCS-Go-downloading";

#[derive(Clone)]
struct DownloadContext {
  db: Arc<Db>,
  bilibili: Arc<BilibiliClient>,
  login_store: Arc<LoginStore>,
  download_runtime: Arc<crate::DownloadRuntime>,
  app_log_path: Arc<std::path::PathBuf>,
  edit_upload_state: Arc<std::sync::Mutex<crate::commands::submission::EditUploadState>>,
}

impl DownloadContext {
  fn new(state: &State<'_, AppState>) -> Self {
    Self {
      db: state.db.clone(),
      bilibili: state.bilibili.clone(),
      login_store: state.login_store.clone(),
      download_runtime: state.download_runtime.clone(),
      app_log_path: state.app_log_path.clone(),
      edit_upload_state: state.edit_upload_state.clone(),
    }
  }

  fn from_state(state: &AppState) -> Self {
    Self {
      db: state.db.clone(),
      bilibili: state.bilibili.clone(),
      login_store: state.login_store.clone(),
      download_runtime: state.download_runtime.clone(),
      app_log_path: state.app_log_path.clone(),
      edit_upload_state: state.edit_upload_state.clone(),
    }
  }
}

fn register_baidu_download_process(
  context: &DownloadContext,
  record_id: i64,
  child: Arc<Mutex<Child>>,
) {
  if let Ok(mut map) = context.download_runtime.baidu_children.lock() {
    map.insert(record_id, child);
  }
}

fn remove_baidu_download_process(context: &DownloadContext, record_id: i64) {
  if let Ok(mut map) = context.download_runtime.baidu_children.lock() {
    map.remove(&record_id);
  }
}

fn cancel_baidu_download_process(
  context: &DownloadContext,
  record_id: i64,
) -> Result<bool, String> {
  let handle = {
    let mut map = context
      .download_runtime
      .baidu_children
      .lock()
      .map_err(|_| "BaiduPCS-Go 进程锁失败".to_string())?;
    map.remove(&record_id)
  };
  let Some(handle) = handle else {
    return Ok(false);
  };
  let mut guard = handle
    .lock()
    .map_err(|_| "BaiduPCS-Go 进程锁失败".to_string())?;
  let _ = guard.kill();
  let _ = guard.wait();
  Ok(true)
}

#[derive(Clone)]
struct StreamCandidate {
  id: Option<i64>,
  bandwidth: i64,
  codec: Option<String>,
  urls: Vec<String>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DownloadConfig {
  pub download_name: Option<String>,
  pub download_path: Option<String>,
  pub resolution: Option<String>,
  pub codec: Option<String>,
  pub format: Option<String>,
  pub content: Option<String>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DownloadPart {
  pub cid: i64,
  pub title: String,
  pub duration: Option<i64>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DownloadRequest {
  pub video_url: String,
  pub parts: Vec<DownloadPart>,
  pub config: DownloadConfig,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionVideoPart {
  #[allow(dead_code)]
  pub original_title: String,
  pub file_path: String,
  pub start_time: Option<String>,
  pub end_time: Option<String>,
  #[allow(dead_code)]
  pub cid: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionRequest {
  pub title: String,
  pub description: Option<String>,
  pub partition_id: i64,
  pub tags: Option<String>,
  pub topic_id: Option<i64>,
  pub mission_id: Option<i64>,
  pub activity_title: Option<String>,
  pub video_type: String,
  pub collection_id: Option<i64>,
  pub segment_prefix: Option<String>,
  pub priority: Option<bool>,
  pub baidu_sync_enabled: Option<bool>,
  pub baidu_sync_path: Option<String>,
  pub baidu_sync_filename: Option<String>,
  pub video_parts: Vec<SubmissionVideoPart>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationRequest {
  #[allow(dead_code)]
  pub enable_submission: bool,
  #[allow(dead_code)]
  pub workflow_config: Option<Value>,
  pub download_request: Option<DownloadRequest>,
  pub download_requests: Option<Vec<DownloadRequest>>,
  pub submission_request: SubmissionRequest,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoDownloadRecord {
  pub id: i64,
  pub bvid: Option<String>,
  pub aid: Option<String>,
  pub title: Option<String>,
  pub part_title: Option<String>,
  pub part_count: Option<i64>,
  pub current_part: Option<i64>,
  pub download_url: Option<String>,
  pub local_path: Option<String>,
  pub resolution: Option<String>,
  pub codec: Option<String>,
  pub format: Option<String>,
  pub status: i64,
  pub progress: i64,
  pub progress_total: i64,
  pub progress_done: i64,
  pub create_time: String,
  pub update_time: String,
  pub source_type: String,
}

struct PendingDownloadRecord {
  id: i64,
  bvid: Option<String>,
  aid: Option<String>,
  part_title: Option<String>,
  local_path: Option<String>,
  resolution: Option<String>,
  codec: Option<String>,
  format: Option<String>,
  cid: Option<i64>,
  content: Option<String>,
  progress: i64,
  download_url: Option<String>,
  source_type: String,
}

#[derive(Clone)]
struct DownloadTaskCreateResult {
  id: i64,
  cid: i64,
  expected_path: String,
  actual_path: String,
}

fn resolve_runtime_local_path(db: &Db, value: &str) -> String {
  let path = to_absolute_local_path(db, value);
  if path.as_os_str().is_empty() {
    String::new()
  } else {
    path.to_string_lossy().to_string()
  }
}

#[tauri::command]
pub async fn download_video(
  state: State<'_, AppState>,
  payload: Value,
) -> Result<ApiResponse<Value>, String> {
  let context = DownloadContext::new(&state);
  let integration = payload.get("downloadRequest").is_some() || payload.get("downloadRequests").is_some();
  if integration {
    let request: IntegrationRequest = match serde_json::from_value(payload) {
      Ok(request) => request,
      Err(err) => {
        return Ok(ApiResponse::error(format!(
          "Failed to parse download request: {}",
          err
        )));
      }
    };

    return Ok(handle_integration_download(context, request).await);
  }

  let request: DownloadRequest = match serde_json::from_value(payload) {
    Ok(request) => request,
    Err(err) => {
      return Ok(ApiResponse::error(format!(
        "Failed to parse download request: {}",
        err
      )));
    }
  };

  match create_download_task(context, request).await {
    Ok(task_id) => Ok(ApiResponse::success(json!(task_id))),
    Err(err) => Ok(ApiResponse::error(err)),
  }
}

#[tauri::command]
pub fn download_get(state: State<'_, AppState>, task_id: i64) -> ApiResponse<VideoDownloadRecord> {
  let prefix = load_local_path_prefix(&state.db);
  match state.db.with_conn(|conn| {
    conn.query_row(
      "SELECT id, bvid, aid, title, part_title, part_count, current_part, download_url, local_path, resolution, codec, format, status, progress, progress_total, progress_done, create_time, update_time, source_type \
       FROM video_download WHERE id = ?1",
      [task_id],
      |row| {
        Ok(VideoDownloadRecord {
          id: row.get(0)?,
          bvid: row.get(1)?,
          aid: row.get(2)?,
          title: row.get(3)?,
          part_title: row.get(4)?,
          part_count: row.get(5)?,
          current_part: row.get(6)?,
          download_url: row.get(7)?,
          local_path: row.get(8)?,
          resolution: row.get(9)?,
          codec: row.get(10)?,
          format: row.get(11)?,
          status: row.get(12)?,
          progress: row.get(13)?,
          progress_total: row.get(14)?,
          progress_done: row.get(15)?,
          create_time: row.get(16)?,
          update_time: row.get(17)?,
          source_type: row
            .get::<_, Option<String>>(18)?
            .unwrap_or_else(|| DOWNLOAD_SOURCE_BILIBILI.to_string()),
        })
      },
    )
  }) {
    Ok(mut record) => {
      record.local_path =
        to_absolute_local_path_opt_with_prefix(prefix.as_path(), record.local_path);
      ApiResponse::success(record)
    }
    Err(err) => ApiResponse::error(format!("Failed to load download task: {}", err)),
  }
}

#[tauri::command]
pub fn download_list_by_status(
  state: State<'_, AppState>,
  status: i64,
) -> ApiResponse<Vec<VideoDownloadRecord>> {
  let prefix = load_local_path_prefix(&state.db);
  match state.db.with_conn(|conn| {
    let mut stmt = conn.prepare(
      "SELECT id, bvid, aid, title, part_title, part_count, current_part, download_url, local_path, resolution, codec, format, status, progress, progress_total, progress_done, create_time, update_time, source_type \
       FROM video_download WHERE status = ?1 ORDER BY id DESC",
    )?;
    let list = stmt
      .query_map([status], |row| {
        Ok(VideoDownloadRecord {
          id: row.get(0)?,
          bvid: row.get(1)?,
          aid: row.get(2)?,
          title: row.get(3)?,
          part_title: row.get(4)?,
          part_count: row.get(5)?,
          current_part: row.get(6)?,
          download_url: row.get(7)?,
          local_path: row.get(8)?,
          resolution: row.get(9)?,
          codec: row.get(10)?,
          format: row.get(11)?,
          status: row.get(12)?,
          progress: row.get(13)?,
          progress_total: row.get(14)?,
          progress_done: row.get(15)?,
          create_time: row.get(16)?,
          update_time: row.get(17)?,
          source_type: row
            .get::<_, Option<String>>(18)?
            .unwrap_or_else(|| DOWNLOAD_SOURCE_BILIBILI.to_string()),
        })
      })?
      .collect::<Result<Vec<_>, _>>()?;
    Ok(list)
  }) {
    Ok(mut list) => {
      for item in &mut list {
        item.local_path =
          to_absolute_local_path_opt_with_prefix(prefix.as_path(), item.local_path.clone());
      }
      ApiResponse::success(list)
    }
    Err(err) => ApiResponse::error(format!("Failed to load downloads: {}", err)),
  }
}

#[tauri::command]
pub fn download_delete(
  state: State<'_, AppState>,
  task_id: i64,
  delete_file: Option<bool>,
) -> ApiResponse<String> {
  let delete_file = delete_file.unwrap_or(false);
  let record = match state.db.with_conn(|conn| {
    conn.query_row(
      "SELECT local_path, status, source_type FROM video_download WHERE id = ?1",
      [task_id],
      |row| {
        Ok((
          row.get::<_, Option<String>>(0)?,
          row.get::<_, i64>(1)?,
          row.get::<_, Option<String>>(2)?,
        ))
      },
    )
  }) {
    Ok(value) => value,
    Err(err) => return ApiResponse::error(format!("Failed to load download record: {}", err)),
  };
  let (local_path, status, source_type) = record;
  let source_type = source_type
    .unwrap_or_else(|| DOWNLOAD_SOURCE_BILIBILI.to_string())
    .to_ascii_uppercase();
  if status == 1 && source_type != DOWNLOAD_SOURCE_BAIDU {
    return ApiResponse::error("任务正在下载，暂不支持删除".to_string());
  }
  if status == 1 && source_type == DOWNLOAD_SOURCE_BAIDU {
    let context = DownloadContext::new(&state);
    if let Err(err) = cancel_baidu_download_process(&context, task_id) {
      return ApiResponse::error(format!("取消网盘下载失败: {}", err));
    }
  }

  if delete_file {
    let local_path = match local_path {
      Some(value) if !value.trim().is_empty() => value,
      _ => return ApiResponse::error("缺少本地路径，无法删除文件".to_string()),
    };
    let path = to_absolute_local_path(&state.db, &local_path);
    if path.as_os_str().is_empty() {
      return ApiResponse::error("缺少本地路径，无法删除文件".to_string());
    }
    cleanup_download_outputs(&path);
    if path.exists() {
      let remove_result = if path.is_dir() {
        std::fs::remove_dir_all(&path)
      } else {
        std::fs::remove_file(&path)
      };
      if let Err(err) = remove_result {
        return ApiResponse::error(format!("删除文件失败: {}", err));
      }
    }
    let baidu_temp = PathBuf::from(format!(
      "{}{}",
      path.to_string_lossy(),
      BAIDU_DOWNLOAD_SUFFIX
    ));
    let _ = std::fs::remove_file(baidu_temp);

    if let Some(parent) = path.parent() {
      if is_dir_empty(parent) {
        if let Err(err) = std::fs::remove_dir(parent) {
          return ApiResponse::error(format!("删除目录失败: {}", err));
        }
      }
    }
  }

  match state.db.with_conn(|conn| {
    conn.execute("DELETE FROM video_download WHERE id = ?1", [task_id])?;
    Ok(())
  }) {
    Ok(()) => ApiResponse::success("Deleted".to_string()),
    Err(err) => ApiResponse::error(format!("Failed to delete: {}", err)),
  }
}

#[tauri::command]
pub async fn download_retry(
  state: State<'_, AppState>,
  task_id: i64,
) -> Result<ApiResponse<String>, String> {
  let context = DownloadContext::new(&state);
  let record = context
    .db
    .with_conn(|conn| {
      conn.query_row(
        "SELECT bvid, aid, part_title, local_path, resolution, codec, format, cid, content, status, source_type, download_url \
         FROM video_download WHERE id = ?1",
        [task_id],
        |row| {
          Ok((
            row.get::<_, Option<String>>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, Option<String>>(6)?,
            row.get::<_, Option<i64>>(7)?,
            row.get::<_, Option<String>>(8)?,
            row.get::<_, i64>(9)?,
            row.get::<_, Option<String>>(10)?,
            row.get::<_, Option<String>>(11)?,
          ))
        },
      )
    })
    .map_err(|err| format!("读取下载任务失败: {}", err))?;

  let (
    bvid,
    aid,
    part_title,
    local_path,
    resolution,
    codec,
    format,
    cid,
    content,
    status,
    source_type,
    download_url,
  ) = record;
  let source_type = source_type
    .unwrap_or_else(|| DOWNLOAD_SOURCE_BILIBILI.to_string())
    .to_ascii_uppercase();

  if source_type == DOWNLOAD_SOURCE_BAIDU {
    if status == 1 {
      return Ok(ApiResponse::error("任务正在下载"));
    }
    if status == 0 {
      return Ok(ApiResponse::error("任务已在队列中"));
    }
    if status == 4 {
      return Ok(ApiResponse::error("任务已暂停，请使用继续下载"));
    }
    let local_path = match local_path {
      Some(value) => resolve_runtime_local_path(context.db.as_ref(), &value),
      None => return Ok(ApiResponse::error("缺少本地路径，无法重试")),
    };
    if local_path.trim().is_empty() {
      return Ok(ApiResponse::error("缺少本地路径，无法重试"));
    }
    let remote_path = match download_url {
      Some(value) if !value.trim().is_empty() => value,
      _ => return Ok(ApiResponse::error("缺少远端路径，无法重试")),
    };
    cleanup_download_outputs(&PathBuf::from(&local_path));
    reset_download_record_progress(&context, task_id)?;
    let started = try_start_baidu_download_job(
      context.clone(),
      task_id,
      0,
      remote_path,
      PathBuf::from(local_path),
    )?;
    if !started {
      let _ = update_download_status_only(&context, task_id, 0);
    }
    schedule_pending_downloads(context.clone()).await;
    return Ok(ApiResponse::success("Retry started".to_string()));
  }

  if status == 1 {
    return Ok(ApiResponse::error("任务正在下载"));
  }
  if status == 0 {
    return Ok(ApiResponse::error("任务已在队列中"));
  }
  if status == 4 {
    return Ok(ApiResponse::error("任务已暂停，请使用继续下载"));
  }
  let cid = match cid {
    Some(value) => value,
    None => return Ok(ApiResponse::error("该任务缺少CID，无法重试")),
  };
  let local_path = match local_path {
    Some(value) => resolve_runtime_local_path(context.db.as_ref(), &value),
    None => return Ok(ApiResponse::error("缺少本地路径，无法重试")),
  };
  if local_path.trim().is_empty() {
    return Ok(ApiResponse::error("缺少本地路径，无法重试"));
  }
  let _ = reset_integrated_submission_status(&context, task_id);

  let part = DownloadPart {
    cid,
    title: part_title.unwrap_or_else(|| "未命名分P".to_string()),
    duration: None,
  };
  let config = DownloadConfig {
    download_name: None,
    download_path: None,
    resolution,
    codec,
    format,
    content,
  };

  let duration = if bvid.is_some() || aid.is_some() {
    match fetch_play_info(&context, bvid.clone(), aid.clone(), cid, &config).await {
      Ok(play_info) => {
        let duration = extract_play_duration_seconds(&play_info);
        if let Some(value) = duration {
          append_log(
            &context.app_log_path,
            &format!("download_retry_duration task_id={} duration={}", task_id, value),
          );
        } else {
          append_log(
            &context.app_log_path,
            &format!("download_retry_duration task_id={} duration=missing", task_id),
          );
        }
        duration
      }
      Err(err) => {
        append_log(
          &context.app_log_path,
          &format!("download_retry_duration task_id={} err={}", task_id, err),
        );
        None
      }
    }
  } else {
    None
  };

  let part = DownloadPart {
    duration,
    ..part
  };
  let output_path = PathBuf::from(local_path);
  cleanup_download_outputs(&output_path);
  let _ = update_download_status(&context, task_id, 0, 0);
  let started = try_start_download_job(
    context.clone(),
    task_id,
    0,
    bvid,
    aid,
    part,
    config,
    output_path,
    None,
  )?;
  if !started {
    let _ = update_download_status_only(&context, task_id, 0);
  }
  schedule_pending_downloads(context.clone()).await;

  Ok(ApiResponse::success("Retry started".to_string()))
}

#[tauri::command]
pub async fn download_resume(
  state: State<'_, AppState>,
  task_id: i64,
) -> Result<ApiResponse<String>, String> {
  let context = DownloadContext::new(&state);
  let record = context
    .db
    .with_conn(|conn| {
      conn.query_row(
        "SELECT bvid, aid, part_title, local_path, resolution, codec, format, cid, content, status, progress, source_type, download_url \
         FROM video_download WHERE id = ?1",
        [task_id],
        |row| {
          Ok((
            row.get::<_, Option<String>>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, Option<String>>(6)?,
            row.get::<_, Option<i64>>(7)?,
            row.get::<_, Option<String>>(8)?,
            row.get::<_, i64>(9)?,
            row.get::<_, i64>(10)?,
            row.get::<_, Option<String>>(11)?,
            row.get::<_, Option<String>>(12)?,
          ))
        },
      )
    })
    .map_err(|err| format!("读取下载任务失败: {}", err))?;

  let (
    bvid,
    aid,
    part_title,
    local_path,
    resolution,
    codec,
    format,
    cid,
    content,
    status,
    progress,
    source_type,
    download_url,
  ) = record;
  let source_type = source_type
    .unwrap_or_else(|| DOWNLOAD_SOURCE_BILIBILI.to_string())
    .to_ascii_uppercase();

  if source_type == DOWNLOAD_SOURCE_BAIDU {
    if status == 1 {
      return Ok(ApiResponse::error("任务正在下载"));
    }
    if status == 0 {
      return Ok(ApiResponse::error("任务已在队列中"));
    }
    if status != 4 {
      return Ok(ApiResponse::error("任务未处于暂停状态"));
    }
    let local_path = match local_path {
      Some(value) => resolve_runtime_local_path(context.db.as_ref(), &value),
      None => return Ok(ApiResponse::error("缺少本地路径，无法继续下载")),
    };
    if local_path.trim().is_empty() {
      return Ok(ApiResponse::error("缺少本地路径，无法继续下载"));
    }
    let remote_path = match download_url {
      Some(value) if !value.trim().is_empty() => value,
      _ => return Ok(ApiResponse::error("缺少远端路径，无法继续下载")),
    };
    let started = try_start_baidu_download_job(
      context.clone(),
      task_id,
      4,
      remote_path,
      PathBuf::from(local_path),
    )?;
    if !started {
      let _ = update_download_status_only(&context, task_id, 0);
    }
    schedule_pending_downloads(context.clone()).await;
    return Ok(ApiResponse::success("Resume started".to_string()));
  }

  if status == 1 {
    return Ok(ApiResponse::error("任务正在下载"));
  }
  if status == 0 {
    return Ok(ApiResponse::error("任务已在队列中"));
  }
  if status != 4 {
    return Ok(ApiResponse::error("任务未处于暂停状态"));
  }
  let cid = match cid {
    Some(value) => value,
    None => return Ok(ApiResponse::error("该任务缺少CID，无法继续下载")),
  };
  let local_path = match local_path {
    Some(value) => resolve_runtime_local_path(context.db.as_ref(), &value),
    None => return Ok(ApiResponse::error("缺少本地路径，无法继续下载")),
  };
  if local_path.trim().is_empty() {
    return Ok(ApiResponse::error("缺少本地路径，无法继续下载"));
  }

  let part = DownloadPart {
    cid,
    title: part_title.unwrap_or_else(|| "未命名分P".to_string()),
    duration: None,
  };
  let config = DownloadConfig {
    download_name: None,
    download_path: None,
    resolution,
    codec,
    format,
    content,
  };

  let duration = if bvid.is_some() || aid.is_some() {
    match fetch_play_info(&context, bvid.clone(), aid.clone(), cid, &config).await {
      Ok(play_info) => extract_play_duration_seconds(&play_info),
      Err(_) => None,
    }
  } else {
    None
  };

  let part = DownloadPart {
    duration,
    ..part
  };
  let output_path = PathBuf::from(local_path);
  let resume_progress = progress.max(0).min(99);
  let started = try_start_download_job(
    context.clone(),
    task_id,
    4,
    bvid,
    aid,
    part,
    config,
    output_path,
    Some(resume_progress),
  )?;
  let message = if started {
    "Resume started"
  } else {
    let _ = update_download_status_only(&context, task_id, 0);
    "Resume queued"
  };
  schedule_pending_downloads(context.clone()).await;

  Ok(ApiResponse::success(message.to_string()))
}

pub async fn requeue_integrated_downloads(
  state: &State<'_, AppState>,
  download_ids: &[i64],
) -> Result<(), String> {
  if download_ids.is_empty() {
    return Ok(());
  }
  let context = DownloadContext::new(state);
  for record_id in download_ids {
    requeue_download_record(&context, *record_id).await?;
  }
  schedule_pending_downloads(context.clone()).await;
  Ok(())
}

async fn requeue_download_record(
  context: &DownloadContext,
  record_id: i64,
) -> Result<(), String> {
  let record = context
    .db
    .with_conn(|conn| {
      conn.query_row(
        "SELECT bvid, aid, part_title, local_path, resolution, codec, format, cid, content, status \
         FROM video_download WHERE id = ?1",
        [record_id],
        |row| {
          Ok((
            row.get::<_, Option<String>>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, Option<String>>(6)?,
            row.get::<_, Option<i64>>(7)?,
            row.get::<_, Option<String>>(8)?,
            row.get::<_, i64>(9)?,
          ))
        },
      )
    })
    .map_err(|err| format!("读取下载任务失败: {}", err))?;

  let (bvid, aid, part_title, local_path, resolution, codec, format, cid, content, status) =
    record;

  if status == 1 || status == 0 {
    return Ok(());
  }

  let cid = match cid {
    Some(value) => value,
    None => return Err("该任务缺少CID，无法重新下载".to_string()),
  };
  let local_path = match local_path {
    Some(value) => resolve_runtime_local_path(context.db.as_ref(), &value),
    None => return Err("缺少本地路径，无法重新下载".to_string()),
  };
  if local_path.trim().is_empty() {
    return Err("缺少本地路径，无法重新下载".to_string());
  }
  let _ = reset_integrated_submission_status(context, record_id);

  let part = DownloadPart {
    cid,
    title: part_title.unwrap_or_else(|| "未命名分P".to_string()),
    duration: None,
  };
  let config = DownloadConfig {
    download_name: None,
    download_path: None,
    resolution,
    codec,
    format,
    content,
  };

  let duration = if bvid.is_some() || aid.is_some() {
    match fetch_play_info(context, bvid.clone(), aid.clone(), cid, &config).await {
      Ok(play_info) => extract_play_duration_seconds(&play_info),
      Err(_) => None,
    }
  } else {
    None
  };
  let part = DownloadPart { duration, ..part };
  let output_path = PathBuf::from(local_path);
  cleanup_download_outputs(&output_path);
  reset_download_record_progress(context, record_id)?;
  clear_download_progress(context, record_id);

  let started = try_start_download_job(
    context.clone(),
    record_id,
    0,
    bvid,
    aid,
    part,
    config,
    output_path,
    None,
  )?;
  if !started {
    let _ = update_download_status_only(context, record_id, 0);
  }

  Ok(())
}

pub fn recover_stale_downloads(state: &AppState) {
  let context = DownloadContext::from_state(state);
  let stale_ids = context
    .db
    .with_conn(|conn| {
      let mut stmt =
        conn.prepare("SELECT id FROM video_download WHERE status = 1")?;
      let rows = stmt.query_map([], |row| row.get(0))?;
      Ok(rows.collect::<Result<Vec<i64>, _>>()?)
    })
    .unwrap_or_default();

  if stale_ids.is_empty() {
    append_log(&context.app_log_path, "download_recover_stale none");
    return;
  }

  let now = now_rfc3339();
  if context
    .db
    .with_conn(|conn| {
      conn.execute(
        "UPDATE video_download SET status = 4, update_time = ?1 WHERE status = 1",
        [&now],
      )?;
      Ok(())
    })
    .is_ok()
  {
    append_log(
      &context.app_log_path,
      &format!("download_recover_stale status=paused count={}", stale_ids.len()),
    );
  }

  let context_clone = context.clone();
  tauri::async_runtime::spawn(async move {
    for record_id in stale_ids {
      let _ = refresh_integration_status(&context_clone, record_id).await;
    }
  });
}

pub fn start_download_queue_loop(state: &AppState) {
  let context = DownloadContext::from_state(state);
  tauri::async_runtime::spawn(async move {
    schedule_pending_downloads(context.clone()).await;
    loop {
      sleep(Duration::from_secs(5)).await;
      schedule_pending_downloads(context.clone()).await;
    }
  });
}

async fn handle_integration_download(
  context: DownloadContext,
  request: IntegrationRequest,
) -> ApiResponse<Value> {
  let mut download_requests = Vec::new();
  if let Some(requests) = request.download_requests.clone() {
    download_requests.extend(requests);
  }
  if download_requests.is_empty() {
    if let Some(single) = request.download_request.clone() {
      download_requests.push(single);
    }
  }
  if download_requests.is_empty() {
    return ApiResponse::error("Missing download requests".to_string());
  }

  let mut download_results = Vec::new();
  for download_request in download_requests {
    match create_download_tasks(context.clone(), download_request).await {
      Ok(task_results) => download_results.extend(task_results),
      Err(err) => return ApiResponse::error(err),
    }
  }

  let download_ids: Vec<i64> = download_results.iter().map(|record| record.id).collect();
  let submission_id = uuid::Uuid::new_v4().to_string();
  let now = now_rfc3339();
  append_log(
    context.app_log_path.as_ref(),
    &format!(
      "integration_submission_create_start task_id={} downloads={}",
      submission_id,
      download_ids.len()
    ),
  );
  let mut submission = request.submission_request;
  let workflow_config = request.workflow_config.clone();
  let local_path_prefix = load_local_path_prefix(context.db.as_ref());
  let normalized_baidu_sync_filename =
    crate::commands::submission::normalize_baidu_sync_filename(
      submission.baidu_sync_filename.as_deref(),
    );
  let bilibili_uid = match context
    .login_store
    .load_auth_info(context.db.as_ref())
    .ok()
    .flatten()
    .and_then(|info| info.user_id)
  {
    Some(value) => value,
    None => return ApiResponse::error("请先登录B站账号".to_string()),
  };
  let task_baidu_uid = if submission.baidu_sync_enabled.unwrap_or(false) {
    match baidu_sync::load_baidu_login_info(context.db.as_ref())
      .ok()
      .flatten()
      .filter(|info| info.status == "LOGGED_IN")
      .and_then(|info| info.uid)
      .map(|value| value.trim().to_string())
      .filter(|value| !value.is_empty())
    {
      Some(value) => Some(value),
      None => return ApiResponse::error("请先登录网盘账号".to_string()),
    }
  } else {
    None
  };
  if !download_results.is_empty() {
    let mut path_by_cid: HashMap<i64, String> = HashMap::new();
    let mut path_by_expected: HashMap<String, String> = HashMap::new();
    for record in &download_results {
      path_by_cid.insert(record.cid, record.actual_path.clone());
      if record.expected_path != record.actual_path {
        path_by_expected.insert(record.expected_path.clone(), record.actual_path.clone());
      }
    }
    for part in submission.video_parts.iter_mut() {
      if let Some(cid) = part.cid {
        if let Some(actual_path) = path_by_cid.get(&cid) {
          part.file_path = actual_path.clone();
          continue;
        }
      }
      if let Some(actual_path) = path_by_expected.get(&part.file_path) {
        part.file_path = actual_path.clone();
      }
    }
  }

  let mut source_remote_meta_by_cid: HashMap<i64, (Option<String>, Option<i64>, Option<String>)> =
    HashMap::new();
  if !download_results.is_empty() {
    let load_meta_result = context.db.with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT cid, bvid, aid, part_title FROM video_download WHERE id = ?1 LIMIT 1",
      )?;
      let mut rows = Vec::new();
      for record in &download_results {
        let row: Option<(Option<i64>, Option<String>, Option<String>, Option<String>)> = stmt
          .query_row([record.id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
          })
          .optional()?;
        if let Some(value) = row {
          rows.push(value);
        }
      }
      Ok(rows)
    });
    if let Ok(rows) = load_meta_result {
      for (cid, bvid, aid, part_title) in rows {
        let Some(cid) = cid else {
          continue;
        };
        let aid = aid.and_then(|value| value.trim().parse::<i64>().ok());
        source_remote_meta_by_cid.insert(cid, (bvid, aid, part_title));
      }
    }
  }

  let insert_result = context.db.with_conn(|conn| {
    conn.execute(
      "INSERT INTO submission_task (task_id, status, priority, title, description, cover_url, partition_id, tags, topic_id, mission_id, activity_title, video_type, collection_id, bvid, aid, created_at, updated_at, bilibili_uid, baidu_uid, segment_prefix, baidu_sync_enabled, baidu_sync_path, baidu_sync_filename) \
       VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7, ?8, ?9, ?10, ?11, ?12, NULL, NULL, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
      params![
        &submission_id,
        "PENDING",
        if submission.priority.unwrap_or(false) { 1 } else { 0 },
        submission.title,
        submission.description,
        submission.partition_id,
        submission.tags,
        submission.topic_id,
        submission.mission_id,
        submission.activity_title.as_deref(),
        submission.video_type,
        submission.collection_id,
        &now,
        &now,
        bilibili_uid,
        task_baidu_uid.as_deref(),
        submission.segment_prefix,
        if submission.baidu_sync_enabled.unwrap_or(false) {
          1
        } else {
          0
        },
        submission.baidu_sync_path.as_deref(),
        normalized_baidu_sync_filename.as_deref(),
      ],
    )?;

    for (index, part) in submission.video_parts.into_iter().enumerate() {
      let part_id = uuid::Uuid::new_v4().to_string();
      let stored_file_path =
        to_stored_local_path_with_prefix(local_path_prefix.as_path(), &part.file_path);
      let remote_meta = part
        .cid
        .and_then(|cid| source_remote_meta_by_cid.get(&cid).cloned());
      let remote_bvid = remote_meta.as_ref().and_then(|value| value.0.clone());
      let remote_aid = remote_meta.as_ref().and_then(|value| value.1);
      let remote_part_title = remote_meta
        .as_ref()
        .and_then(|value| value.2.clone())
        .or_else(|| {
          let title = part.original_title.trim().to_string();
          if title.is_empty() {
            None
          } else {
            Some(title)
          }
        });
      let remote_video_url =
        remote_bvid.as_ref().map(|value| format!("https://www.bilibili.com/video/{}", value));
      conn.execute(
        "INSERT INTO task_source_video (id, task_id, source_file_path, remote_video_url, remote_bvid, remote_aid, remote_cid, remote_part_title, sort_order, start_time, end_time) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        (
          part_id,
          &submission_id,
          stored_file_path,
          remote_video_url.as_deref(),
          remote_bvid.as_deref(),
          remote_aid,
          part.cid,
          remote_part_title.as_deref(),
          (index + 1) as i64,
          part.start_time,
          part.end_time,
        ),
      )?;
    }
    Ok(())
  });

  if let Err(err) = insert_result {
    return ApiResponse::error(format!("Failed to create submission task: {}", err));
  }
  append_log(
    context.app_log_path.as_ref(),
    &format!("integration_submission_create_ok task_id={}", submission_id),
  );

  let workflow_instance_id = match workflow_config.as_ref() {
    Some(config) => {
      match crate::commands::submission::create_workflow_instance_for_task(
        &context.db,
        &submission_id,
        config,
      ) {
        Ok((instance_id, _)) => Some(instance_id),
        Err(err) => return ApiResponse::error(err),
      }
    }
    None => None,
  };

  let relation_result = context.db.with_conn(|conn| {
    for download_id in &download_ids {
      conn.execute(
        "INSERT INTO task_relations (download_task_id, submission_task_id, relation_type, status, created_at, updated_at, workflow_instance_id, workflow_status, retry_count) \
         VALUES (?1, ?2, 'INTEGRATED', 'ACTIVE', ?3, ?4, ?5, 'PENDING_DOWNLOAD', 0)",
        (
          download_id,
          &submission_id,
          &now,
          &now,
          workflow_instance_id.as_deref(),
        ),
      )?;
    }
    Ok(())
  });

  match relation_result {
    Ok(()) => {
      for record_id in &download_ids {
        let _ = refresh_integration_status(&context, *record_id).await;
      }
      ApiResponse::success(json!({
        "downloadTaskIds": download_ids,
        "submissionTaskId": submission_id,
        "workflowInstanceId": workflow_instance_id,
      }))
    }
    Err(err) => ApiResponse::error(format!("Failed to create submission task: {}", err)),
  }
}

async fn create_download_task(
  context: DownloadContext,
  request: DownloadRequest,
) -> Result<i64, String> {
  let records = create_download_tasks(context, request).await?;
  records
    .first()
    .map(|record| record.id)
    .ok_or_else(|| "No download task created".to_string())
}

async fn create_download_tasks(
  context: DownloadContext,
  request: DownloadRequest,
) -> Result<Vec<DownloadTaskCreateResult>, String> {
  let (bvid, aid) = parse_video_id(&request.video_url);
  let video_title = fetch_video_title(&context, bvid.as_deref(), aid.as_deref()).await;

  let folder_name = request
    .config
    .download_name
    .clone()
    .or(video_title.clone())
    .unwrap_or_else(|| "Unknown".to_string());
  let sanitized_folder = sanitize_filename(&folder_name);

  let now = now_rfc3339();

  let parts = request.parts.clone();
  let mut record_ids = Vec::with_capacity(parts.len());
  let part_count = parts.len() as i64;
  let settings = load_download_settings_from_db(&context.db)
    .map_err(|err| format!("Failed to load download settings: {}", err))?;
  let storage_prefix = load_local_path_prefix(&context.db);
  let base_dir = request
    .config
    .download_path
    .clone()
    .filter(|path| !path.trim().is_empty())
    .unwrap_or_else(|| settings.download_path.clone());
  let base_dir = if base_dir.trim().is_empty() {
    default_download_dir().to_string_lossy().to_string()
  } else {
    base_dir
  };
  for (index, part) in parts.iter().enumerate() {
    let file_name = format!("{}.mp4", sanitize_filename(&part.title));
    let output_path = build_output_path(&base_dir, &sanitized_folder, &file_name);
    let expected_path = output_path.to_string_lossy().to_string();

    if let Some((record_id, actual_path, status)) = find_reusable_download_record(
      &context,
      Some(part.cid),
      request.video_url.as_str(),
      part.title.as_str(),
    )? {
      let actual_path_buf = PathBuf::from(&actual_path);
      let has_file = actual_path_buf.is_file();
      let can_reuse = status == 0 || status == 1 || status == 3 || status == 4;
      if can_reuse || (status == 2 && has_file) {
        record_ids.push(DownloadTaskCreateResult {
          id: record_id,
          cid: part.cid,
          expected_path,
          actual_path,
        });
        continue;
      }
    }

    let output_path = resolve_unique_output_path(&context, output_path)?;
    let actual_path = output_path.to_string_lossy().to_string();
    let stored_actual_path =
      to_stored_local_path_with_prefix(storage_prefix.as_path(), actual_path.as_str());

    let record_id = context
      .db
      .with_conn(|conn| {
        conn.execute(
          "INSERT INTO video_download (bvid, aid, title, part_title, part_count, current_part, download_url, local_path, status, progress, progress_total, progress_done, create_time, update_time, resolution, codec, format, cid, content, source_type) \
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, 0, 0, 0, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
          (
            bvid.as_deref(),
            aid.as_deref(),
            video_title.as_deref(),
            part.title.as_str(),
            part_count,
            (index + 1) as i64,
            request.video_url.as_str(),
            stored_actual_path.as_str(),
            &now,
            &now,
            request.config.resolution.as_deref(),
            request.config.codec.as_deref(),
            request.config.format.as_deref(),
            part.cid,
            request.config.content.as_deref(),
            DOWNLOAD_SOURCE_BILIBILI,
          ),
        )?;
        Ok(conn.last_insert_rowid())
      })
      .map_err(|err| format!("Failed to save download record: {}", err))?;

    record_ids.push(DownloadTaskCreateResult {
      id: record_id,
      cid: part.cid,
      expected_path,
      actual_path,
    });

  }

  if record_ids.is_empty() {
    return Err("No download task created".to_string());
  }
  schedule_pending_downloads(context.clone()).await;
  Ok(record_ids)
}

fn find_reusable_download_record(
  context: &DownloadContext,
  cid: Option<i64>,
  download_url: &str,
  part_title: &str,
) -> Result<Option<(i64, String, i64)>, String> {
  let storage_prefix = load_local_path_prefix(context.db.as_ref());
  context
    .db
    .with_conn(|conn| {
      let mut candidates: Vec<(i64, String, i64)> = Vec::new();
      if let Some(cid) = cid {
        let mut stmt = conn.prepare(
          "SELECT id, local_path, status FROM video_download WHERE cid = ?1 AND source_type = ?2 ORDER BY id DESC",
        )?;
        let rows = stmt.query_map((cid, DOWNLOAD_SOURCE_BILIBILI), |row| {
          Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
        for row in rows {
          candidates.push(row?);
        }
      } else {
        let mut stmt = conn.prepare(
          "SELECT id, local_path, status FROM video_download WHERE download_url = ?1 AND part_title = ?2 AND source_type = ?3 ORDER BY id DESC",
        )?;
        let rows = stmt.query_map((download_url, part_title, DOWNLOAD_SOURCE_BILIBILI), |row| {
          Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
        for row in rows {
          candidates.push(row?);
        }
      }

      for (id, local_path, status) in candidates {
        let runtime_path = to_absolute_local_path_with_prefix(storage_prefix.as_path(), &local_path);
        if runtime_path.is_file() {
          return Ok(Some((
            id,
            runtime_path.to_string_lossy().to_string(),
            status,
          )));
        }
      }
      Ok(None)
    })
    .map_err(|err| format!("Failed to check reusable download record: {}", err))
}

fn resolve_unique_output_path(
  context: &DownloadContext,
  output_path: PathBuf,
) -> Result<PathBuf, String> {
  if !download_path_conflict(context, &output_path)? {
    return Ok(output_path);
  }
  let parent = output_path
    .parent()
    .ok_or_else(|| "Missing output directory".to_string())?;
  let stem = output_path
    .file_stem()
    .and_then(|value| value.to_str())
    .ok_or_else(|| "Missing output file name".to_string())?;
  let ext = output_path.extension().and_then(|value| value.to_str());
  for index in 1..=999 {
    let file_name = match ext {
      Some(ext) if !ext.is_empty() => format!("{} ({}).{}", stem, index, ext),
      _ => format!("{} ({})", stem, index),
    };
    let candidate = parent.join(file_name);
    if !download_path_conflict(context, &candidate)? {
      return Ok(candidate);
    }
  }
  Err("下载路径冲突过多，无法生成新的文件名".to_string())
}

fn download_path_conflict(
  context: &DownloadContext,
  output_path: &Path,
) -> Result<bool, String> {
  let path_value = to_stored_local_path(
    context.db.as_ref(),
    output_path.to_string_lossy().as_ref(),
  );
  let exists_in_db = context
    .db
    .with_conn(|conn| {
      let mut stmt =
        conn.prepare("SELECT 1 FROM video_download WHERE local_path = ?1 LIMIT 1")?;
      let mut rows = stmt.query([&path_value])?;
      Ok(rows.next()?.is_some())
    })
    .map_err(|err| format!("Failed to check download record: {}", err))?;
  if exists_in_db {
    return Ok(true);
  }
  if output_path.exists() {
    return Ok(true);
  }
  if output_path.with_extension("video").exists() {
    return Ok(true);
  }
  if output_path.with_extension("audio").exists() {
    return Ok(true);
  }
  let aria2_path = PathBuf::from(format!("{}.aria2", output_path.to_string_lossy()));
  Ok(aria2_path.exists())
}

async fn schedule_pending_downloads(context: DownloadContext) {
  let available = match available_download_slots(&context) {
    Ok(value) => value,
    Err(err) => {
      append_log(
        &context.app_log_path,
        &format!("download_schedule_skip err={}", err),
      );
      return;
    }
  };
  if available <= 0 {
    return;
  }

  let pending = match load_pending_downloads(&context, available) {
    Ok(records) => records,
    Err(err) => {
      append_log(
        &context.app_log_path,
        &format!("download_schedule_skip err={}", err),
      );
      return;
    }
  };
  if pending.is_empty() {
    return;
  }

  for record in pending {
    match start_pending_download(context.clone(), record) {
      Ok(started) => {
        if !started {
          break;
        }
      }
      Err(err) => {
        append_log(
          &context.app_log_path,
          &format!("download_schedule_error err={}", err),
        );
      }
    }
  }
}

fn load_pending_downloads(
  context: &DownloadContext,
  limit: i64,
) -> Result<Vec<PendingDownloadRecord>, String> {
  if limit <= 0 {
    return Ok(Vec::new());
  }
  let storage_prefix = load_local_path_prefix(context.db.as_ref());
  context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT id, bvid, aid, part_title, local_path, resolution, codec, format, cid, content, progress, download_url, source_type \
         FROM video_download WHERE status = 0 ORDER BY id ASC LIMIT ?1",
      )?;
      let rows = stmt.query_map([limit], |row| {
        Ok(PendingDownloadRecord {
          id: row.get(0)?,
          bvid: row.get(1)?,
          aid: row.get(2)?,
          part_title: row.get(3)?,
          local_path: row.get(4)?,
          resolution: row.get(5)?,
          codec: row.get(6)?,
          format: row.get(7)?,
          cid: row.get(8)?,
          content: row.get(9)?,
          progress: row.get(10)?,
          download_url: row.get(11)?,
          source_type: row
            .get::<_, Option<String>>(12)?
            .unwrap_or_else(|| DOWNLOAD_SOURCE_BILIBILI.to_string()),
        })
      })?;
      let mut list = rows.collect::<Result<Vec<_>, _>>()?;
      for item in &mut list {
        item.local_path = to_absolute_local_path_opt_with_prefix(
          storage_prefix.as_path(),
          item.local_path.clone(),
        );
      }
      Ok(list)
    })
    .map_err(|err| format!("Failed to load pending downloads: {}", err))
}

fn start_pending_download(
  context: DownloadContext,
  record: PendingDownloadRecord,
) -> Result<bool, String> {
  let resume_progress = record.progress.max(0);
  let source_type = record.source_type.trim().to_ascii_uppercase();
  let local_path = match record.local_path {
    Some(value) => value,
    None => {
      append_log(
        &context.app_log_path,
        &format!(
          "download_schedule_invalid record_id={} reason=missing_local_path",
          record.id
        ),
      );
      let _ = update_download_status(&context, record.id, 3, 0);
      let context_clone = context.clone();
      tauri::async_runtime::spawn(async move {
        let _ = refresh_integration_status(&context_clone, record.id).await;
      });
      return Ok(false);
    }
  };
  if source_type == DOWNLOAD_SOURCE_BAIDU {
    let remote_path = match record.download_url {
      Some(value) if !value.trim().is_empty() => value,
      _ => {
        append_log(
          &context.app_log_path,
          &format!(
            "download_schedule_invalid record_id={} reason=missing_remote_path",
            record.id
          ),
        );
        let _ = update_download_status(&context, record.id, 3, 0);
        let context_clone = context.clone();
        tauri::async_runtime::spawn(async move {
          let _ = handle_baidu_restore_after_download(&context_clone, record.id).await;
        });
        return Ok(false);
      }
    };
    return try_start_baidu_download_job(
      context,
      record.id,
      0,
      remote_path,
      PathBuf::from(local_path),
    );
  }
  let cid = match record.cid {
    Some(value) => value,
    None => {
      append_log(
        &context.app_log_path,
        &format!("download_schedule_invalid record_id={} reason=missing_cid", record.id),
      );
      let _ = update_download_status(&context, record.id, 3, 0);
      let context_clone = context.clone();
      tauri::async_runtime::spawn(async move {
        let _ = refresh_integration_status(&context_clone, record.id).await;
      });
      return Ok(false);
    }
  };
  let part = DownloadPart {
    cid,
    title: record.part_title.unwrap_or_else(|| "未命名分P".to_string()),
    duration: None,
  };
  let config = DownloadConfig {
    download_name: None,
    download_path: None,
    resolution: record.resolution,
    codec: record.codec,
    format: record.format,
    content: record.content,
  };

  try_start_download_job(
    context,
    record.id,
    0,
    record.bvid,
    record.aid,
    part,
    config,
    PathBuf::from(local_path),
    if resume_progress > 0 {
      Some(resume_progress.min(99))
    } else {
      None
    },
  )
}

fn try_start_download_job(
  context: DownloadContext,
  record_id: i64,
  expected_status: i64,
  bvid: Option<String>,
  aid: Option<String>,
  part: DownloadPart,
  config: DownloadConfig,
  output_path: PathBuf,
  resume_progress: Option<i64>,
) -> Result<bool, String> {
  if !try_acquire_download_slot(&context)? {
    return Ok(false);
  }
  if !mark_download_running(&context, record_id, expected_status)? {
    release_download_slot(&context);
    return Ok(false);
  }

  let context_clone = context.clone();
  tauri::async_runtime::spawn(async move {
    run_download_job(
      context_clone,
      record_id,
      bvid,
      aid,
      part,
      config,
      output_path,
      resume_progress,
    )
    .await;
  });

  Ok(true)
}

fn try_start_baidu_download_job(
  context: DownloadContext,
  record_id: i64,
  expected_status: i64,
  remote_path: String,
  output_path: PathBuf,
) -> Result<bool, String> {
  if !try_acquire_download_slot(&context)? {
    return Ok(false);
  }
  if !mark_download_running(&context, record_id, expected_status)? {
    release_download_slot(&context);
    return Ok(false);
  }

  let context_clone = context.clone();
  tauri::async_runtime::spawn(async move {
    run_baidu_download_job(context_clone, record_id, remote_path, output_path).await;
  });

  Ok(true)
}

fn mark_download_running(
  context: &DownloadContext,
  record_id: i64,
  expected_status: i64,
) -> Result<bool, String> {
  let now = now_rfc3339();
  context
    .db
    .with_conn(|conn| {
      let updated = conn.execute(
        "UPDATE video_download SET status = 1, update_time = ?1 WHERE id = ?2 AND status = ?3",
        (&now, record_id, expected_status),
      )?;
      Ok(updated > 0)
    })
    .map_err(|err| format!("Failed to update download status: {}", err))
}

fn available_download_slots(context: &DownloadContext) -> Result<i64, String> {
  let settings = load_download_settings_from_db(&context.db)
    .map_err(|err| format!("Failed to load download settings: {}", err))?;
  let threads = settings.threads.max(1);
  let running = load_running_download_count(context).unwrap_or(0);
  let active = context
    .download_runtime
    .active_count
    .lock()
    .map_err(|_| "Download state lock failed".to_string())?;
  let current_running = (*active).max(running);
  Ok((threads - current_running).max(0))
}

fn try_acquire_download_slot(context: &DownloadContext) -> Result<bool, String> {
  let settings = load_download_settings_from_db(&context.db)
    .map_err(|err| format!("Failed to load download settings: {}", err))?;
  let threads = settings.threads.max(1);
  let running = load_running_download_count(context).unwrap_or(0);
  let mut active = context
    .download_runtime
    .active_count
    .lock()
    .map_err(|_| "Download state lock failed".to_string())?;
  let current_running = (*active).max(running);
  if current_running < threads {
    *active = current_running + 1;
    Ok(true)
  } else {
    Ok(false)
  }
}

fn load_running_download_count(context: &DownloadContext) -> Result<i64, String> {
  context
    .db
    .with_conn(|conn| {
      let count: i64 = conn.query_row(
        "SELECT COUNT(1) FROM video_download WHERE status = 1",
        [],
        |row| row.get(0),
      )?;
      Ok(count)
    })
    .map_err(|err| format!("Failed to load running downloads: {}", err))
}

async fn run_download_job(
  context: DownloadContext,
  record_id: i64,
  bvid: Option<String>,
  aid: Option<String>,
  part: DownloadPart,
  config: DownloadConfig,
  output_path: PathBuf,
  resume_progress: Option<i64>,
) {
  clear_download_progress(&context, record_id);
  append_log(
    &context.app_log_path,
    &format!("download_job_start record_id={} cid={}", record_id, part.cid),
  );

  let result =
    download_part(&context, record_id, bvid, aid, part, config, output_path, resume_progress)
      .await;
  release_download_slot(&context);
  let context_clone = context.clone();
  tauri::async_runtime::spawn(async move {
    schedule_pending_downloads(context_clone).await;
  });
  match result {
    Ok(()) => {
      let _ = update_download_status(&context, record_id, 2, 100);
      clear_download_progress(&context, record_id);
      append_log(
        &context.app_log_path,
        &format!("download_job_complete record_id={} status=completed", record_id),
      );
      let _ = refresh_integration_status(&context, record_id).await;
    }
    Err(err) => {
      if is_resume_error(&err) {
        let _ = update_download_status_only(&context, record_id, 4);
        clear_download_progress(&context, record_id);
        append_log(
          &context.app_log_path,
          &format!(
            "download_job_complete record_id={} status=paused err={}",
            record_id, err
          ),
        );
      } else {
        let _ = update_download_status(&context, record_id, 3, 0);
        clear_download_progress(&context, record_id);
        append_log(
          &context.app_log_path,
          &format!("download_job_complete record_id={} status=failed err={}", record_id, err),
        );
      }
      let _ = refresh_integration_status(&context, record_id).await;
    }
  }
}

fn extract_remote_name(remote_path: &str) -> String {
  remote_path
    .rsplit('/')
    .find(|value| !value.is_empty())
    .unwrap_or("")
    .to_string()
}

fn find_file_by_name(dir: &Path, target: &str) -> Option<PathBuf> {
  if target.trim().is_empty() {
    return None;
  }
  let entries = std::fs::read_dir(dir).ok()?;
  for entry in entries.flatten() {
    let path = entry.path();
    if path.is_dir() {
      if let Some(found) = find_file_by_name(&path, target) {
        return Some(found);
      }
    } else if let Some(name) = path.file_name().and_then(|value| value.to_str()) {
      if name == target {
        return Some(path);
      }
    }
  }
  None
}

fn resolve_baidu_download_size(output_path: &Path, output_dir: &Path, remote_name: &str) -> u64 {
  if let Ok(meta) = output_path.metadata() {
    return meta.len();
  }
  let temp_path = output_path
    .file_name()
    .and_then(|name| name.to_str())
    .map(|name| output_dir.join(format!("{}{}", name, BAIDU_DOWNLOAD_SUFFIX)));
  if let Some(temp_path) = temp_path {
    if let Ok(meta) = temp_path.metadata() {
      return meta.len();
    }
  }
  if let Some(found) = find_file_by_name(output_dir, remote_name) {
    return found.metadata().map(|meta| meta.len()).unwrap_or(0);
  }
  let temp_name = format!("{}{}", remote_name, BAIDU_DOWNLOAD_SUFFIX);
  if let Some(found) = find_file_by_name(output_dir, &temp_name) {
    return found.metadata().map(|meta| meta.len()).unwrap_or(0);
  }
  0
}

async fn run_baidu_download_job(
  context: DownloadContext,
  record_id: i64,
  remote_path: String,
  output_path: PathBuf,
) {
  clear_download_progress(&context, record_id);
  let remote_name = extract_remote_name(&remote_path);
  append_log(
    &context.app_log_path,
    &format!(
      "baidu_download_job_start record_id={} remote={}",
      record_id, remote_path
    ),
  );
  let output_dir = match output_path.parent() {
    Some(value) => value.to_path_buf(),
    None => {
      let _ = update_download_status(&context, record_id, 3, 0);
      append_log(
        &context.app_log_path,
        &format!(
          "baidu_download_job_fail record_id={} err=missing_output_dir",
          record_id
        ),
      );
      let _ = handle_baidu_restore_after_download(&context, record_id).await;
      release_download_slot(&context);
      return;
    }
  };

  let total_size =
    baidu_sync::fetch_baidu_remote_file_size(context.db.as_ref(), &remote_path)
      .ok()
      .unwrap_or(0);

  let db = context.db.clone();
  let remote_path_clone = remote_path.clone();
  let output_path_clone = output_path.clone();
  let download_runtime = context.download_runtime.clone();
  let record_id_clone = record_id;
  let mut download_handle = tauri::async_runtime::spawn_blocking(move || {
    baidu_sync::download_baidu_file_with_hook(
      db.as_ref(),
      &remote_path_clone,
      &output_path_clone,
      |child| {
        if let Ok(mut map) = download_runtime.baidu_children.lock() {
          map.insert(record_id_clone, child);
        }
      },
    )
  });

  let mut ticker = interval(Duration::from_secs(1));
  let download_result: Result<PathBuf, String> = loop {
    tokio::select! {
      result = &mut download_handle => {
        break match result {
          Ok(value) => value.map_err(|err| err.to_string()),
          Err(_) => Err("网盘下载任务失败".to_string()),
        };
      }
      _ = ticker.tick() => {
        if total_size > 0 {
          let current_size =
            resolve_baidu_download_size(&output_path, &output_dir, remote_name.as_str());
          let _ = update_download_bytes(&context, record_id, "baidu", total_size, current_size);
        }
      }
    }
  };

  remove_baidu_download_process(&context, record_id);
  release_download_slot(&context);
  let context_clone = context.clone();
  tauri::async_runtime::spawn(async move {
    schedule_pending_downloads(context_clone).await;
  });

  match download_result {
    Ok(path) => {
      if total_size > 0 {
        let current_size = path.metadata().map(|meta| meta.len()).unwrap_or(0);
        let _ = update_download_bytes(&context, record_id, "baidu", total_size, current_size);
      }
      let _ = update_download_status(&context, record_id, 2, 100);
      clear_download_progress(&context, record_id);
      append_log(
        &context.app_log_path,
        &format!("baidu_download_job_complete record_id={} status=completed", record_id),
      );
      let _ = handle_baidu_restore_after_download(&context, record_id).await;
    }
    Err(err) => {
      let _ = update_download_status(&context, record_id, 3, 0);
      clear_download_progress(&context, record_id);
      append_log(
        &context.app_log_path,
        &format!(
          "baidu_download_job_fail record_id={} err={}",
          record_id, err
        ),
      );
      let _ = handle_baidu_restore_after_download(&context, record_id).await;
    }
  }
}

async fn handle_baidu_restore_after_download(
  context: &DownloadContext,
  record_id: i64,
) -> Result<(), String> {
  let task_id = context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT submission_task_id FROM task_relations \
         WHERE download_task_id = ?1 AND relation_type = 'REMOTE_RESTORE' LIMIT 1",
      )?;
      let value: Option<String> = stmt.query_row([record_id], |row| row.get(0)).ok();
      Ok(value)
    })
    .map_err(|err| err.to_string())?;
  let task_id = match task_id {
    Some(value) => value,
    None => return Ok(()),
  };
  crate::commands::submission::resume_reprocess_after_baidu_restore(
    context.db.clone(),
    context.app_log_path.clone(),
    context.edit_upload_state.clone(),
    task_id,
    record_id,
  )
  .await;
  Ok(())
}

fn release_download_slot(context: &DownloadContext) {
  if let Ok(mut active) = context.download_runtime.active_count.lock() {
    if *active > 0 {
      *active -= 1;
    }
  }
}

async fn download_part(
  context: &DownloadContext,
  record_id: i64,
  bvid: Option<String>,
  aid: Option<String>,
  part: DownloadPart,
  config: DownloadConfig,
  output_path: PathBuf,
  resume_progress: Option<i64>,
) -> Result<(), String> {
  let settings = load_download_settings_from_db(&context.db)
    .map_err(|err| format!("Failed to load download settings: {}", err))?;
  let block_pcdn = settings.block_pcdn;
  let enable_aria2c = settings.enable_aria2c;
  let aria2c_connections = settings.aria2c_connections.max(1).min(32);
  let aria2c_split = settings.aria2c_split.max(1).min(32);
  let min_progress = resume_progress.filter(|value| *value > 0).map(|value| value.min(99));
  let play_info = fetch_play_info(context, bvid.clone(), aid.clone(), part.cid, &config).await?;
  let mut format = config.format.clone().unwrap_or_else(|| "dash".to_string());
  let has_dash = play_info.get("dash").is_some();
  let has_durl = play_info.get("durl").is_some();
  if format == "dash" && !has_dash && has_durl {
    append_log(
      &context.app_log_path,
      &format!("playurl_format_fallback record_id={} from=dash to=mp4", record_id),
    );
    format = "mp4".to_string();
  }
  if (format == "mp4" || format == "flv") && !has_durl && has_dash {
    append_log(
      &context.app_log_path,
      &format!(
        "playurl_format_fallback record_id={} from={} to=dash",
        record_id, format
      ),
    );
    format = "dash".to_string();
  }
  let duration = part
    .duration
    .or_else(|| extract_play_duration_seconds(&play_info))
    .unwrap_or(0)
    .max(0);
  let duration_ms = duration.checked_mul(1000);
  let expected_duration_seconds = duration as f64;
  let track_progress = duration_ms.unwrap_or(0) > 0 || enable_aria2c;

  let header = build_ffmpeg_headers(context).unwrap_or_default();
  let output_path_string = output_path.to_string_lossy().to_string();

  if format == "mp4" || format == "flv" {
    let urls = collect_durl_urls(&play_info, block_pcdn)?;
      if enable_aria2c {
      if let Err(err) = download_with_aria2c(
        context,
        record_id,
        track_progress,
        &output_path,
        &urls,
        &header,
        aria2c_connections,
        aria2c_split,
        "main",
      )
      .await
      {
        if has_partial_file(&output_path) {
          append_log(
            &context.app_log_path,
            &format!(
              "aria2c_resume_pending record_id={} output={}",
              record_id,
              output_path.to_string_lossy()
            ),
          );
          return Err("aria2c下载中断，可重试续传".to_string());
        }
        cleanup_aria2c_files(&output_path);
        append_log(
          &context.app_log_path,
          &format!("aria2c_fallback record_id={} err={}", record_id, err),
        );
      } else {
        return Ok(());
      }
    }
    run_ffmpeg_job_with_url_fallback(
      context,
      record_id,
      track_progress,
      duration_ms,
      min_progress,
      &format,
      &output_path,
      &urls,
      |url| {
        let mut args = Vec::new();
        if !header.is_empty() {
          args.push("-headers".to_string());
          args.push(header.clone());
        }
        args.push("-i".to_string());
        args.push(url.to_string());
        args.extend(["-c".to_string(), "copy".to_string()]);
        if track_progress {
          args.push("-progress".to_string());
          args.push("pipe:1".to_string());
          args.push("-nostats".to_string());
        }
        args.push(output_path_string.clone());
        args
      },
    )
    .await?;
    return Ok(());
  }

  let dash = play_info
    .get("dash")
    .ok_or_else(|| "Missing dash info".to_string())?;
  let content = config.content.unwrap_or_else(|| "audio_video".to_string());

  match content.as_str() {
    "video_only" => {
      let video_candidates =
        select_video_candidates(dash, config.resolution.as_deref(), config.codec.as_deref(), block_pcdn)?;
      let video_urls = video_candidates
        .first()
        .map(|candidate| candidate.urls.clone())
        .ok_or_else(|| "Missing video URL".to_string())?;
      if enable_aria2c {
      if let Err(err) = download_with_aria2c(
        context,
        record_id,
        track_progress,
        &output_path,
        &video_urls,
        &header,
        aria2c_connections,
        aria2c_split,
        "main",
      )
      .await
      {
          if has_partial_file(&output_path) {
            append_log(
              &context.app_log_path,
              &format!(
                "aria2c_resume_pending record_id={} output={}",
                record_id,
                output_path.to_string_lossy()
              ),
            );
            return Err("aria2c下载中断，可重试续传".to_string());
          }
          cleanup_aria2c_files(&output_path);
          append_log(
            &context.app_log_path,
            &format!("aria2c_fallback record_id={} err={}", record_id, err),
          );
        } else {
          return Ok(());
        }
      }
      run_ffmpeg_job_with_url_fallback(
        context,
        record_id,
        track_progress,
        duration_ms,
        min_progress,
        &format,
        &output_path,
        &video_urls,
        |url| {
          let mut args = Vec::new();
          if !header.is_empty() {
            args.push("-headers".to_string());
            args.push(header.clone());
          }
          args.push("-i".to_string());
          args.push(url.to_string());
          args.extend(["-c".to_string(), "copy".to_string()]);
          if track_progress {
            args.push("-progress".to_string());
            args.push("pipe:1".to_string());
            args.push("-nostats".to_string());
          }
          args.push(output_path_string.clone());
          args
        },
      )
      .await?;
      Ok(())
    }
    "audio_only" => {
      let audio_candidates = select_audio_candidates(dash, block_pcdn)?;
      let audio_urls = audio_candidates
        .first()
        .map(|candidate| candidate.urls.clone())
        .ok_or_else(|| "Missing audio URL".to_string())?;
      if enable_aria2c {
      if let Err(err) = download_with_aria2c(
        context,
        record_id,
        track_progress,
        &output_path,
        &audio_urls,
        &header,
        aria2c_connections,
        aria2c_split,
        "main",
      )
      .await
      {
          if has_partial_file(&output_path) {
            append_log(
              &context.app_log_path,
              &format!(
                "aria2c_resume_pending record_id={} output={}",
                record_id,
                output_path.to_string_lossy()
              ),
            );
            return Err("aria2c下载中断，可重试续传".to_string());
          }
          cleanup_aria2c_files(&output_path);
          append_log(
            &context.app_log_path,
            &format!("aria2c_fallback record_id={} err={}", record_id, err),
          );
        } else {
          return Ok(());
        }
      }
      run_ffmpeg_job_with_url_fallback(
        context,
        record_id,
        track_progress,
        duration_ms,
        min_progress,
        &format,
        &output_path,
        &audio_urls,
        |url| {
          let mut args = Vec::new();
          if !header.is_empty() {
            args.push("-headers".to_string());
            args.push(header.clone());
          }
          args.push("-i".to_string());
          args.push(url.to_string());
          args.extend(["-c".to_string(), "copy".to_string()]);
          if track_progress {
            args.push("-progress".to_string());
            args.push("pipe:1".to_string());
            args.push("-nostats".to_string());
          }
          args.push(output_path_string.clone());
          args
        },
      )
      .await?;
      Ok(())
    }
    _ => {
      let video_candidates =
        select_video_candidates(dash, config.resolution.as_deref(), config.codec.as_deref(), block_pcdn)?;
      let audio_candidates = select_audio_candidates(dash, block_pcdn)?;
      let mut last_error: Option<String> = None;
      let mut aria2c_enabled = enable_aria2c;
      for (video_index, video_candidate) in video_candidates.iter().enumerate() {
        for (audio_index, audio_candidate) in audio_candidates.iter().enumerate() {
          let mut aria2c_failed = !aria2c_enabled;
          let temp_video_path = output_path.with_extension("video");
          let temp_audio_path = output_path.with_extension("audio");
          if aria2c_enabled {
            let (video_result, audio_result) = tokio::join!(
              download_with_aria2c(
                context,
                record_id,
                track_progress,
                &temp_video_path,
                &video_candidate.urls,
                &header,
                aria2c_connections,
                aria2c_split,
                "video",
              ),
              download_with_aria2c(
                context,
                record_id,
                track_progress,
                &temp_audio_path,
                &audio_candidate.urls,
                &header,
                aria2c_connections,
                aria2c_split,
                "audio",
              ),
            );
            if let Err(err) = &video_result {
              append_log(
                &context.app_log_path,
                &format!("aria2c_fallback record_id={} err={}", record_id, err),
              );
              if is_aria2c_missing_error(err) {
                aria2c_enabled = false;
              }
            }
            if let Err(err) = &audio_result {
              append_log(
                &context.app_log_path,
                &format!("aria2c_fallback record_id={} err={}", record_id, err),
              );
              if is_aria2c_missing_error(err) {
                aria2c_enabled = false;
              }
            }
            if video_result.is_err() || audio_result.is_err() {
              if has_partial_file(&temp_video_path) || has_partial_file(&temp_audio_path) {
                let resume_path = if has_partial_file(&temp_audio_path) {
                  &temp_audio_path
                } else {
                  &temp_video_path
                };
                append_log(
                  &context.app_log_path,
                  &format!(
                    "aria2c_resume_pending record_id={} output={}",
                    record_id,
                    resume_path.to_string_lossy()
                  ),
                );
                return Err("aria2c下载中断，可重试续传".to_string());
              }
              cleanup_aria2c_files(&temp_video_path);
              cleanup_aria2c_files(&temp_audio_path);
              aria2c_failed = true;
            } else {
              let _ = update_download_progress(context, record_id, 95);
              let video_timing = log_ffprobe_source_duration(
                &context.app_log_path,
                record_id,
                "dash_aria2c_video",
                &temp_video_path,
              );
              let audio_timing = log_ffprobe_source_duration(
                &context.app_log_path,
                record_id,
                "dash_aria2c_audio",
                &temp_audio_path,
              );
              let mut video_delay = 0.0;
              let mut audio_trim = 0.0;
              if let (Some(video_timing), Some(audio_timing)) = (video_timing, audio_timing) {
                let offset = video_timing.video_start - audio_timing.audio_start;
                if offset > 0.1 {
                  audio_trim = offset;
                } else if offset < -0.1 {
                  video_delay = -offset;
                }
                append_log(
                  &context.app_log_path,
                  &format!(
                    "ffmpeg_merge_offset record_id={} v_start={:.3} a_start={:.3} v_delay={:.3} a_trim={:.3}",
                    record_id,
                    video_timing.video_start,
                    audio_timing.audio_start,
                    video_delay,
                    audio_trim
                  ),
                );
              }
              let mut args = Vec::new();
              if video_delay > 0.0 {
                args.push("-itsoffset".to_string());
                args.push(format!("{:.3}", video_delay));
              }
              args.push("-i".to_string());
              args.push(temp_video_path.to_string_lossy().to_string());
              args.push("-i".to_string());
              args.push(temp_audio_path.to_string_lossy().to_string());
              if audio_trim > 0.0 {
                args.push("-af".to_string());
                args.push(format!(
                  "atrim=start={:.3},asetpts=PTS-STARTPTS",
                  audio_trim
                ));
              }
              args.extend([
                "-map".to_string(),
                "0:v:0".to_string(),
                "-map".to_string(),
                "1:a:0".to_string(),
                "-c:v".to_string(),
                "copy".to_string(),
                "-c:a".to_string(),
                if audio_trim > 0.0 {
                  "aac".to_string()
                } else {
                  "copy".to_string()
                },
                "-shortest".to_string(),
              ]);
              if output_path.extension().and_then(|value| value.to_str()) == Some("mp4") {
                args.push("-movflags".to_string());
                args.push("+faststart".to_string());
              }
              args.push(output_path_string.clone());
              match run_ffmpeg_job(
                context,
                record_id,
                false,
                duration_ms,
                min_progress,
                &format,
                &output_path,
                args,
              )
              .await
              {
                Ok(_) => {
                  let _ = update_download_progress(context, record_id, 99);
                  match probe_stream_durations(&output_path) {
                  Ok((video_duration, audio_duration)) => {
                    log_ffprobe_av_duration(
                      &context.app_log_path,
                      record_id,
                      "dash_aria2c_merge",
                      &output_path,
                      expected_duration_seconds,
                      video_duration,
                      audio_duration,
                    );
                    log_ffprobe_av_timing(
                      &context.app_log_path,
                      record_id,
                      "dash_aria2c_merge",
                      &output_path,
                    );
                    if !is_video_complete(
                      video_duration,
                      audio_duration,
                      expected_duration_seconds,
                    ) {
                      append_log(
                        &context.app_log_path,
                        &format!(
                          "ffprobe_video_short record_id={} video={:.3} audio={:.3} expected={:.3}",
                          record_id, video_duration, audio_duration, expected_duration_seconds
                        ),
                      );
                      let _ = std::fs::remove_file(&output_path);
                      last_error = Some("Video stream too short".to_string());
                    } else if is_audio_complete(video_duration, audio_duration) {
                      let _ = std::fs::remove_file(&temp_video_path);
                      let _ = std::fs::remove_file(&temp_audio_path);
                      return Ok(());
                    } else {
                      append_log(
                        &context.app_log_path,
                        &format!(
                          "ffprobe_audio_short record_id={} video={:.3} audio={:.3}",
                          record_id, video_duration, audio_duration
                        ),
                      );
                      let _ = std::fs::remove_file(&output_path);
                      last_error = Some("Audio stream too short".to_string());
                    }
                  }
                  Err(err) => {
                    append_log(
                      &context.app_log_path,
                      &format!("ffprobe_check_fail record_id={} err={}", record_id, err),
                    );
                    let _ = std::fs::remove_file(&output_path);
                    last_error = Some(err);
                  }
                  }
                }
                Err(err) => {
                  let _ = std::fs::remove_file(&output_path);
                  last_error = Some(err);
                }
              }
              let _ = std::fs::remove_file(&temp_video_path);
              let _ = std::fs::remove_file(&temp_audio_path);
              if last_error.is_none() {
                return Ok(());
              }
              aria2c_failed = true;
            }
          }

          if !aria2c_failed {
            continue;
          }

          for (video_url_index, video_url) in video_candidate.urls.iter().enumerate() {
            for (audio_url_index, audio_url) in audio_candidate.urls.iter().enumerate() {
              if video_index > 0
                || audio_index > 0
                || video_url_index > 0
                || audio_url_index > 0
              {
                append_log(
                  &context.app_log_path,
                  &format!(
                    "ffmpeg_retry record_id={} video={} audio={}",
                    record_id,
                    video_index + 1,
                    audio_index + 1
                  ),
                );
              }
              let mut args = Vec::new();
              if !header.is_empty() {
                args.push("-headers".to_string());
                args.push(header.clone());
              }
              args.push("-i".to_string());
              args.push(video_url.clone());
              if !header.is_empty() {
                args.push("-headers".to_string());
                args.push(header.clone());
              }
              args.push("-i".to_string());
              args.push(audio_url.clone());
              args.extend([
                "-map".to_string(),
                "0:v:0".to_string(),
                "-map".to_string(),
                "1:a:0".to_string(),
                "-c".to_string(),
                "copy".to_string(),
                "-shortest".to_string(),
              ]);
              if track_progress {
                args.push("-progress".to_string());
                args.push("pipe:1".to_string());
                args.push("-nostats".to_string());
              }
              args.push(output_path_string.clone());
              match run_ffmpeg_job(
                context,
                record_id,
                track_progress,
                duration_ms,
                min_progress,
                &format,
                &output_path,
                args,
              )
              .await
              {
                Ok(_) => match probe_stream_durations(&output_path) {
                  Ok((video_duration, audio_duration)) => {
                    log_ffprobe_av_duration(
                      &context.app_log_path,
                      record_id,
                      "dash_ffmpeg",
                      &output_path,
                      expected_duration_seconds,
                      video_duration,
                      audio_duration,
                    );
                    log_ffprobe_av_timing(
                      &context.app_log_path,
                      record_id,
                      "dash_ffmpeg",
                      &output_path,
                    );
                    if !is_video_complete(
                      video_duration,
                      audio_duration,
                      expected_duration_seconds,
                    ) {
                      append_log(
                        &context.app_log_path,
                        &format!(
                          "ffprobe_video_short record_id={} video={:.3} audio={:.3} expected={:.3}",
                          record_id, video_duration, audio_duration, expected_duration_seconds
                        ),
                      );
                      let _ = std::fs::remove_file(&output_path);
                      last_error = Some("Video stream too short".to_string());
                      break;
                    }
                    if is_audio_complete(video_duration, audio_duration) {
                      return Ok(());
                    }
                    append_log(
                      &context.app_log_path,
                      &format!(
                        "ffprobe_audio_short record_id={} video={:.3} audio={:.3}",
                        record_id, video_duration, audio_duration
                      ),
                    );
                    let _ = std::fs::remove_file(&output_path);
                    last_error = Some("Audio stream too short".to_string());
                    continue;
                  }
                  Err(err) => {
                    append_log(
                      &context.app_log_path,
                      &format!("ffprobe_check_fail record_id={} err={}", record_id, err),
                    );
                    let _ = std::fs::remove_file(&output_path);
                    last_error = Some(err);
                    continue;
                  }
                },
                Err(err) => {
                  let _ = std::fs::remove_file(&output_path);
                  last_error = Some(err);
                  continue;
                }
              }
            }
          }
        }
      }
      Err(last_error.unwrap_or_else(|| "Missing audio streams".to_string()))
    }
  }
}

async fn run_ffmpeg_job(
  context: &DownloadContext,
  record_id: i64,
  track_progress: bool,
  duration_ms: Option<i64>,
  min_progress: Option<i64>,
  format: &str,
  output_path: &Path,
  args: Vec<String>,
) -> Result<(), String> {
  if let Some(parent) = output_path.parent() {
    std::fs::create_dir_all(parent).map_err(|err| format!("Failed to create directory: {}", err))?;
  }
  let _ = reset_download_progress_bytes(context, record_id);

  append_log(
    &context.app_log_path,
    &format!(
      "ffmpeg_start record_id={} progress={} format={} output={}",
      record_id,
      track_progress,
      format,
      output_path.to_string_lossy()
    ),
  );

  let exec_result = if track_progress {
    let min_progress = min_progress.unwrap_or(0).clamp(0, 99);
    let mut last_progress = min_progress;
    let context_clone = context.clone();
    let record_id_clone = record_id;
    tauri::async_runtime::spawn_blocking(move || {
      run_ffmpeg_with_progress(&args, duration_ms, |progress| {
        if progress <= last_progress {
          return;
        }
        let progress = progress.max(min_progress);
        if progress > last_progress {
          last_progress = progress;
          let _ = update_download_progress(&context_clone, record_id_clone, progress);
        }
      })
    })
    .await
    .map_err(|_| "Failed to execute download task".to_string())?
  } else {
    tauri::async_runtime::spawn_blocking(move || run_ffmpeg(&args))
      .await
      .map_err(|_| "Failed to execute download task".to_string())?
  };

  match &exec_result {
    Ok(_) => {
      append_log(
        &context.app_log_path,
        &format!("ffmpeg_done record_id={} status=ok", record_id),
      );
    }
    Err(err) => {
      append_log(
        &context.app_log_path,
        &format!("ffmpeg_done record_id={} status=err msg={}", record_id, err),
      );
    }
  }

  exec_result
}

async fn run_ffmpeg_job_with_url_fallback<F>(
  context: &DownloadContext,
  record_id: i64,
  track_progress: bool,
  duration_ms: Option<i64>,
  min_progress: Option<i64>,
  format: &str,
  output_path: &Path,
  urls: &[String],
  build_args: F,
) -> Result<(), String>
where
  F: Fn(&str) -> Vec<String>,
{
  if urls.is_empty() {
    return Err("Missing stream url".to_string());
  }
  let mut last_error = None;
  for (index, url) in urls.iter().enumerate() {
    if index > 0 {
      append_log(
        &context.app_log_path,
        &format!("ffmpeg_url_retry record_id={} index={}", record_id, index + 1),
      );
    }
    let args = build_args(url);
    match run_ffmpeg_job(
      context,
      record_id,
      track_progress,
      duration_ms,
      min_progress,
      format,
      output_path,
      args,
    )
    .await
    {
      Ok(_) => return Ok(()),
      Err(err) => {
        let _ = std::fs::remove_file(output_path);
        last_error = Some(err);
        continue;
      }
    }
  }
  Err(last_error.unwrap_or_else(|| "Missing stream url".to_string()))
}

fn build_aria2c_args(
  output_path: &Path,
  urls: &[String],
  header: &str,
  connections: i64,
  split: i64,
) -> Result<Vec<String>, String> {
  let parent = output_path
    .parent()
    .ok_or_else(|| "Missing output directory".to_string())?;
  let file_name = output_path
    .file_name()
    .ok_or_else(|| "Missing output file name".to_string())?;
  let mut args = vec![
    "--allow-overwrite=true".to_string(),
    "--auto-file-renaming=false".to_string(),
    "--continue=true".to_string(),
    "--disable-ipv6=true".to_string(),
    "--file-allocation=none".to_string(),
    "--summary-interval=1".to_string(),
    "--console-log-level=warn".to_string(),
    format!("--max-connection-per-server={}", connections),
    format!("--split={}", split),
    "--min-split-size=1M".to_string(),
    "--referer=https://www.bilibili.com/".to_string(),
    format!("--dir={}", parent.to_string_lossy()),
    format!("--out={}", file_name.to_string_lossy()),
  ];
  for line in header.split("\r\n").map(|value| value.trim()) {
    if !line.is_empty() {
      args.push(format!("--header={}", line));
    }
  }
  args.extend(urls.iter().cloned());
  Ok(args)
}

#[derive(Clone)]
struct Aria2cRpcConfig {
  endpoint: String,
  secret: String,
  port: u16,
}

#[derive(Deserialize)]
struct Aria2cRpcError {
  code: i64,
  message: String,
}

#[derive(Deserialize)]
struct Aria2cRpcResponse<T> {
  result: Option<T>,
  error: Option<Aria2cRpcError>,
}

#[derive(Deserialize)]
struct Aria2cTaskStatus {
  status: String,
  #[serde(rename = "totalLength")]
  total_length: String,
  #[serde(rename = "completedLength")]
  completed_length: String,
  #[serde(rename = "errorCode")]
  error_code: Option<String>,
  #[serde(rename = "errorMessage")]
  error_message: Option<String>,
}

fn build_aria2c_rpc_config() -> Result<Aria2cRpcConfig, String> {
  let listener = TcpListener::bind("127.0.0.1:0")
    .map_err(|err| format!("Failed to bind aria2c rpc port: {}", err))?;
  let port = listener
    .local_addr()
    .map_err(|err| format!("Failed to read aria2c rpc port: {}", err))?
    .port();
  drop(listener);
  let nanos = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_nanos();
  let secret = format!("{}-{}", std::process::id(), nanos);
  Ok(Aria2cRpcConfig {
    endpoint: format!("http://127.0.0.1:{}/jsonrpc", port),
    secret,
    port,
  })
}

fn append_aria2c_rpc_args(args: &mut Vec<String>, rpc: &Aria2cRpcConfig) {
  args.push("--enable-rpc".to_string());
  args.push("--rpc-listen-all=false".to_string());
  args.push(format!("--rpc-listen-port={}", rpc.port));
  args.push(format!("--rpc-secret={}", rpc.secret));
}

fn aria2c_rpc_request<T: DeserializeOwned>(
  client: &Client,
  rpc: &Aria2cRpcConfig,
  method: &str,
  mut params: Vec<Value>,
) -> Result<T, String> {
  params.insert(0, Value::String(format!("token:{}", rpc.secret)));
  let payload = json!({
    "jsonrpc": "2.0",
    "id": "1",
    "method": format!("aria2.{}", method),
    "params": params,
  });
  let response = client
    .post(&rpc.endpoint)
    .json(&payload)
    .send()
    .map_err(|err| format!("aria2c rpc {} request failed: {}", method, err))?;
  let body: Aria2cRpcResponse<T> = response
    .json()
    .map_err(|err| format!("aria2c rpc {} decode failed: {}", method, err))?;
  if let Some(err) = body.error {
    return Err(format!(
      "aria2c rpc {} error: {} ({})",
      method, err.message, err.code
    ));
  }
  body
    .result
    .ok_or_else(|| format!("aria2c rpc {} empty result", method))
}

fn aria2c_rpc_fetch_status(
  client: &Client,
  rpc: &Aria2cRpcConfig,
) -> Result<Option<Aria2cTaskStatus>, String> {
  let active: Vec<Aria2cTaskStatus> = aria2c_rpc_request(client, rpc, "tellActive", vec![])?;
  if let Some(status) = active.into_iter().next() {
    return Ok(Some(status));
  }
  let waiting: Vec<Aria2cTaskStatus> =
    aria2c_rpc_request(client, rpc, "tellWaiting", vec![json!(0), json!(1)])?;
  if let Some(status) = waiting.into_iter().next() {
    return Ok(Some(status));
  }
  let stopped: Vec<Aria2cTaskStatus> =
    aria2c_rpc_request(client, rpc, "tellStopped", vec![json!(0), json!(1)])?;
  Ok(stopped.into_iter().next())
}

fn aria2c_status_bytes(status: &Aria2cTaskStatus) -> Option<(u64, u64)> {
  let total: u64 = status.total_length.parse().ok()?;
  let completed: u64 = status.completed_length.parse().ok()?;
  Some((total, completed))
}

fn aria2c_status_error(status: &Aria2cTaskStatus) -> Option<String> {
  if status.status == "error" {
    return Some(
      status
        .error_message
        .clone()
        .unwrap_or_else(|| "aria2c failed".to_string()),
    );
  }
  if let Some(code) = &status.error_code {
    if code != "0" && code != "31" {
      let message = status
        .error_message
        .clone()
        .unwrap_or_else(|| "aria2c failed".to_string());
      return Some(format!("aria2c failed: {}", message));
    }
  }
  None
}

fn has_partial_file(path: &Path) -> bool {
  std::fs::metadata(path)
    .map(|meta| meta.len() > 0)
    .unwrap_or(false)
}

fn cleanup_aria2c_files(path: &Path) {
  let _ = std::fs::remove_file(path);
  let control_path = PathBuf::from(format!("{}.aria2", path.to_string_lossy()));
  let _ = std::fs::remove_file(control_path);
}

fn cleanup_download_outputs(path: &Path) {
  let _ = std::fs::remove_file(path);
  let baidu_temp = PathBuf::from(format!("{}{}", path.to_string_lossy(), BAIDU_DOWNLOAD_SUFFIX));
  let _ = std::fs::remove_file(baidu_temp);
  cleanup_aria2c_files(path);
  let temp_video = path.with_extension("video");
  let temp_audio = path.with_extension("audio");
  cleanup_aria2c_files(&temp_video);
  cleanup_aria2c_files(&temp_audio);
}

fn run_aria2c_with_path<F>(
  app_log_path: &Path,
  record_id: i64,
  progress_key: &str,
  path: &str,
  args: &[String],
  track_progress: bool,
  on_progress: &mut F,
  rpc: &Aria2cRpcConfig,
) -> Result<(), String>
where
  F: FnMut(u64, u64),
{
  let mut command = Command::new(path);
  apply_no_window(&mut command);
  let mut child = command
    .args(args)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .map_err(|err| format!("Failed to start aria2c: {}", err))?;

  let stdout = child
    .stdout
    .take()
    .ok_or_else(|| "Failed to capture aria2c stdout".to_string())?;
  let mut stderr = child
    .stderr
    .take()
    .ok_or_else(|| "Failed to capture aria2c stderr".to_string())?;

  let (stderr_tx, stderr_rx) = std::sync::mpsc::channel();
  thread::spawn(move || {
    let mut buffer = String::new();
    let _ = stderr.read_to_string(&mut buffer);
    let _ = stderr_tx.send(buffer);
  });
  thread::spawn(move || {
    let mut buffer = Vec::new();
    let mut reader = BufReader::new(stdout);
    let _ = reader.read_to_end(&mut buffer);
  });

  let client = Client::builder()
    .timeout(Duration::from_secs(3))
    .build()
    .map_err(|err| format!("Failed to create aria2c rpc client: {}", err))?;

  let mut rpc_ready = false;
  for _ in 0..20 {
    if aria2c_rpc_request::<Value>(&client, rpc, "getVersion", vec![]).is_ok() {
      rpc_ready = true;
      break;
    }
    thread::sleep(Duration::from_millis(100));
  }
  let mut logged_rpc_not_ready = false;
  if !rpc_ready {
    logged_rpc_not_ready = true;
    append_log(
      app_log_path,
      &format!(
        "aria2c_rpc_not_ready record_id={} key={} path={}",
        record_id, progress_key, path
      ),
    );
  }
  let _ = logged_rpc_not_ready;

  let mut last_error: Option<String> = None;
  let mut exit_status: Option<std::process::ExitStatus> = None;
  let mut completed = false;
  let mut logged_no_status = false;
  let mut logged_zero_total = false;
  let mut logged_parse_error = false;
  loop {
    if rpc_ready {
      match aria2c_rpc_fetch_status(&client, rpc) {
        Ok(Some(status)) => {
          if let Some(err) = aria2c_status_error(&status) {
            last_error = Some(err);
            break;
          }
          if track_progress {
            if let Some((content, chunk)) = aria2c_status_bytes(&status) {
              if content == 0 && !logged_zero_total {
                logged_zero_total = true;
                append_log(
                  app_log_path,
                  &format!(
                    "aria2c_rpc_zero_total record_id={} key={} status={} total={} completed={}",
                    record_id,
                    progress_key,
                    status.status,
                    status.total_length,
                    status.completed_length
                  ),
                );
              }
              on_progress(content, chunk);
            } else if !logged_parse_error {
              logged_parse_error = true;
              append_log(
                app_log_path,
                &format!(
                  "aria2c_rpc_bytes_parse_fail record_id={} key={} status={} total={} completed={}",
                  record_id,
                  progress_key,
                  status.status,
                  status.total_length,
                  status.completed_length
                ),
              );
            }
          }
          if status.status == "complete" {
            if let Some((content, _chunk)) = aria2c_status_bytes(&status) {
              on_progress(content, content);
            }
            completed = true;
            break;
          }
        }
        Ok(None) => {
          if !logged_no_status {
            logged_no_status = true;
            append_log(
              app_log_path,
              &format!("aria2c_rpc_no_status record_id={} key={}", record_id, progress_key),
            );
          }
        }
        Err(_) => {}
      }
    } else if aria2c_rpc_request::<Value>(&client, rpc, "getVersion", vec![]).is_ok() {
      rpc_ready = true;
      append_log(
        app_log_path,
        &format!(
          "aria2c_rpc_ready record_id={} key={} path={}",
          record_id, progress_key, path
        ),
      );
    }

    match child
      .try_wait()
      .map_err(|err| format!("Failed to wait for aria2c: {}", err))?
    {
      Some(status) => {
        exit_status = Some(status);
        break;
      }
      None => {}
    }
    thread::sleep(Duration::from_secs(1));
  }

  if completed && rpc_ready {
    let _ = aria2c_rpc_request::<Value>(&client, rpc, "shutdown", vec![]);
  }

  if exit_status.is_none() {
    let _ = child.kill();
    exit_status = Some(
      child
        .wait()
        .map_err(|err| format!("Failed to wait for aria2c: {}", err))?,
    );
  }
  let stderr_output = stderr_rx.recv().unwrap_or_default();
  if let Some(err) = last_error {
    let stderr_trimmed = stderr_output.trim();
    if stderr_trimmed.is_empty() {
      return Err(err);
    }
    return Err(format!("{}; {}", err, stderr_trimmed));
  }
  if completed {
    return Ok(());
  }
  if let Some(status) = exit_status {
    if status.success() {
      return Ok(());
    }
  }

  Err(format!("aria2c failed: {}", stderr_output.trim()))
}

fn run_aria2c_command<F>(
  app_log_path: &Path,
  record_id: i64,
  progress_key: &str,
  args: &[String],
  track_progress: bool,
  on_progress: &mut F,
  rpc: &Aria2cRpcConfig,
) -> Result<(), String>
where
  F: FnMut(u64, u64),
{
  let mut last_error = None;
  for path in resolve_aria2c_candidates() {
    match run_aria2c_with_path(
      app_log_path,
      record_id,
      progress_key,
      &path,
      args,
      track_progress,
      on_progress,
      rpc,
    ) {
      Ok(_) => return Ok(()),
      Err(err) => {
        last_error = Some(err);
      }
    }
  }
  Err(last_error.unwrap_or_else(|| "aria2c not available".to_string()))
}

fn is_aria2c_missing_error(message: &str) -> bool {
  let lower = message.to_lowercase();
  lower.contains("aria2c") && (lower.contains("no such file") || lower.contains("not found"))
}

fn is_resume_error(message: &str) -> bool {
  message.contains("可重试续传") || message.contains("aria2c下载中断")
}

async fn download_with_aria2c(
  context: &DownloadContext,
  record_id: i64,
  track_progress: bool,
  output_path: &Path,
  urls: &[String],
  header: &str,
  aria2c_connections: i64,
  aria2c_split: i64,
  progress_key: &str,
) -> Result<(), String> {
  if urls.is_empty() {
    return Err("Missing stream url".to_string());
  }
  if let Some(parent) = output_path.parent() {
    std::fs::create_dir_all(parent).map_err(|err| format!("Failed to create directory: {}", err))?;
  }

  let mut args = build_aria2c_args(output_path, urls, header, aria2c_connections, aria2c_split)?;
  let rpc_config = build_aria2c_rpc_config()?;
  append_aria2c_rpc_args(&mut args, &rpc_config);
  append_log(
    &context.app_log_path,
    &format!(
      "aria2c_start record_id={} output={}",
      record_id,
      output_path.to_string_lossy()
    ),
  );

  let context_clone = context.clone();
  let progress_key = progress_key.to_string();
  let rpc_config_clone = rpc_config.clone();
  let exec_result = tauri::async_runtime::spawn_blocking(move || {
    let mut update = |content: u64, chunk: u64| {
      let _ = update_download_bytes(&context_clone, record_id, &progress_key, content, chunk);
    };
    run_aria2c_command(
      context_clone.app_log_path.as_ref(),
      record_id,
      &progress_key,
      &args,
      track_progress,
      &mut update,
      &rpc_config_clone,
    )
  })
  .await
  .map_err(|_| "Failed to execute download task".to_string())?;

  match &exec_result {
    Ok(_) => {
      append_log(
        &context.app_log_path,
        &format!("aria2c_done record_id={} status=ok", record_id),
      );
    }
    Err(err) => {
      append_log(
        &context.app_log_path,
        &format!("aria2c_done record_id={} status=err msg={}", record_id, err),
      );
    }
  }

  exec_result
}

async fn fetch_play_info(
  context: &DownloadContext,
  bvid: Option<String>,
  aid: Option<String>,
  cid: i64,
  config: &DownloadConfig,
) -> Result<Value, String> {
  let format = config.format.as_deref().unwrap_or("dash");
  let auth = load_auth(context);
  let is_logged_in = auth.is_some();
  let qn = config
    .resolution
    .clone()
    .unwrap_or_else(|| if is_logged_in { "127".to_string() } else { "64".to_string() });
  let fnval = match format {
    "flv" => "0",
    "mp4" => "1",
    _ => {
      if is_logged_in {
        "4048"
      } else {
        "16"
      }
    }
  };
  let mut params = vec![
    ("cid".to_string(), cid.to_string()),
    ("qn".to_string(), qn),
    ("fnval".to_string(), fnval.to_string()),
    ("fnver".to_string(), "0".to_string()),
    ("fourk".to_string(), "1".to_string()),
  ];

  if let Some(bvid) = bvid {
    params.push(("bvid".to_string(), bvid));
  }
  if let Some(aid) = aid {
    params.push(("avid".to_string(), aid));
  }

  let url = format!("{}/x/player/wbi/playurl", context.bilibili.base_url());
  context
    .bilibili
    .get_json(&url, &params, auth.as_ref(), true)
    .await
}

fn collect_durl_urls(play_info: &Value, block_pcdn: bool) -> Result<Vec<String>, String> {
  let durl = play_info
    .get("durl")
    .and_then(|value| value.as_array())
    .and_then(|list| list.get(0))
    .ok_or_else(|| "Missing mp4 url".to_string())?;
  let mut urls = Vec::new();
  if let Some(url) = durl.get("url").and_then(|value| value.as_str()) {
    urls.push(url.to_string());
  }
  if let Some(list) = durl.get("backup_url").and_then(|value| value.as_array()) {
    for item in list {
      if let Some(url) = item.as_str() {
        urls.push(url.to_string());
      }
    }
  }
  let urls = normalize_stream_urls(urls, block_pcdn);
  if urls.is_empty() {
    return Err("Missing mp4 url".to_string());
  }
  Ok(urls)
}

fn extract_play_duration_seconds(play_info: &Value) -> Option<i64> {
  if let Some(value) = play_info.get("timelength").and_then(|item| item.as_i64()) {
    if value > 0 {
      return Some(((value + 999) / 1000).max(1));
    }
  }
  if let Some(value) = play_info
    .get("durl")
    .and_then(|item| item.as_array())
    .and_then(|list| list.get(0))
    .and_then(|item| item.get("length"))
    .and_then(|item| item.as_i64())
  {
    if value > 0 {
      return Some(((value + 999) / 1000).max(1));
    }
  }
  if let Some(value) = play_info
    .get("dash")
    .and_then(|item| item.get("duration"))
    .and_then(|item| item.as_f64())
  {
    if value > 0.0 {
      return Some(value.ceil() as i64);
    }
  }
  None
}

fn candidate_codec_matches(candidate: &StreamCandidate, codec: &str) -> bool {
  candidate
    .codec
    .as_deref()
    .map(|value| value.contains(codec))
    .unwrap_or(false)
}

fn choose_target_resolution(
  candidates: &[StreamCandidate],
  resolution: Option<&str>,
) -> Option<i64> {
  let mut ids: Vec<i64> = candidates.iter().filter_map(|candidate| candidate.id).collect();
  if ids.is_empty() {
    return None;
  }
  if let Some(resolution) = resolution {
    if let Ok(resolution) = resolution.parse::<i64>() {
      if ids.iter().any(|id| *id == resolution) {
        return Some(resolution);
      }
    }
  }
  ids.sort_unstable();
  ids.pop()
}

fn choose_target_codec(
  candidates: &[StreamCandidate],
  target_resolution: Option<i64>,
  codec: Option<&str>,
) -> Option<String> {
  let filtered: Vec<&StreamCandidate> = candidates
    .iter()
    .filter(|candidate| {
      target_resolution
        .map(|resolution| candidate.id == Some(resolution))
        .unwrap_or(true)
    })
    .collect();
  if filtered.is_empty() {
    return None;
  }
  if let Some(codec) = codec {
    if filtered.iter().any(|candidate| candidate_codec_matches(candidate, codec)) {
      return Some(codec.to_string());
    }
  }
  for codec in ["avc1", "hev1", "hvc1", "vp09", "av01"] {
    if filtered.iter().any(|candidate| candidate_codec_matches(candidate, codec)) {
      return Some(codec.to_string());
    }
  }
  filtered.iter().find_map(|candidate| candidate.codec.clone())
}

fn select_audio_candidates(
  dash: &Value,
  block_pcdn: bool,
) -> Result<Vec<StreamCandidate>, String> {
  let audios = dash
    .get("audio")
    .and_then(|value| value.as_array())
    .ok_or_else(|| "Missing audio streams".to_string())?;
  if audios.is_empty() {
    return Err("Missing audio streams".to_string());
  }
  let mut candidates: Vec<StreamCandidate> = Vec::new();
  for item in audios {
    let bandwidth = item.get("bandwidth").and_then(|value| value.as_i64()).unwrap_or(0);
    let urls = stream_urls_from_item(item, block_pcdn);
    if !urls.is_empty() {
      candidates.push(StreamCandidate {
        id: item.get("id").and_then(|value| value.as_i64()),
        bandwidth,
        codec: None,
        urls,
      });
    }
  }
  candidates.sort_by(|a, b| b.bandwidth.cmp(&a.bandwidth));
  if candidates.is_empty() {
    return Err("Missing audio URL".to_string());
  }
  Ok(candidates)
}

fn select_video_candidates(
  dash: &Value,
  resolution: Option<&str>,
  codec: Option<&str>,
  block_pcdn: bool,
) -> Result<Vec<StreamCandidate>, String> {
  let videos = dash
    .get("video")
    .and_then(|value| value.as_array())
    .ok_or_else(|| "Missing video streams".to_string())?;
  if videos.is_empty() {
    return Err("Missing video streams".to_string());
  }
  let mut candidates: Vec<StreamCandidate> = Vec::new();
  for item in videos {
    let bandwidth = item.get("bandwidth").and_then(|value| value.as_i64()).unwrap_or(0);
    let urls = stream_urls_from_item(item, block_pcdn);
    if urls.is_empty() {
      continue;
    }
    candidates.push(StreamCandidate {
      id: item.get("id").and_then(|value| value.as_i64()),
      bandwidth,
      codec: item
        .get("codecs")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string()),
      urls,
    });
  }
  if candidates.is_empty() {
    return Err("Missing video URL".to_string());
  }
  let target_resolution = choose_target_resolution(&candidates, resolution);
  let target_codec = choose_target_codec(&candidates, target_resolution, codec);
  candidates.sort_by(|a, b| {
    let a_res = target_resolution.map(|resolution| a.id == Some(resolution)).unwrap_or(false);
    let b_res = target_resolution.map(|resolution| b.id == Some(resolution)).unwrap_or(false);
    let a_codec = target_codec
      .as_deref()
      .map(|codec| candidate_codec_matches(a, codec))
      .unwrap_or(false);
    let b_codec = target_codec
      .as_deref()
      .map(|codec| candidate_codec_matches(b, codec))
      .unwrap_or(false);
    let a_priority = if a_res && a_codec {
      0
    } else if a_res {
      1
    } else if a_codec {
      2
    } else {
      3
    };
    let b_priority = if b_res && b_codec {
      0
    } else if b_res {
      1
    } else if b_codec {
      2
    } else {
      3
    };
    a_priority.cmp(&b_priority).then_with(|| b.bandwidth.cmp(&a.bandwidth))
  });
  Ok(candidates)
}

fn probe_stream_durations(path: &Path) -> Result<(f64, f64), String> {
  let args = vec![
    "-v".to_string(),
    "error".to_string(),
    "-show_streams".to_string(),
    "-of".to_string(),
    "json".to_string(),
    path.to_string_lossy().to_string(),
  ];
  let data = run_ffprobe_json(&args)?;
  let streams = data
    .get("streams")
    .and_then(|value| value.as_array())
    .ok_or_else(|| "Missing stream info".to_string())?;
  let mut video_duration = 0.0;
  let mut audio_duration = 0.0;
  let format_duration = data
    .get("format")
    .and_then(|value| value.get("duration"))
    .and_then(|value| value.as_str())
    .and_then(|value| value.parse::<f64>().ok())
    .unwrap_or(0.0);
  for stream in streams {
    let codec_type = stream
      .get("codec_type")
      .and_then(|value| value.as_str())
      .unwrap_or("");
    let duration = stream
      .get("duration")
      .and_then(|value| value.as_str())
      .and_then(|value| value.parse::<f64>().ok())
      .unwrap_or(0.0);
    if codec_type == "video" && video_duration <= 0.0 {
      video_duration = duration;
    }
    if codec_type == "audio" && audio_duration <= 0.0 {
      audio_duration = duration;
    }
  }
  if video_duration <= 0.0 && format_duration > 0.0 {
    video_duration = format_duration;
  }
  Ok((video_duration, audio_duration))
}

fn is_video_complete(video_duration: f64, audio_duration: f64, expected_duration: f64) -> bool {
  if video_duration <= 0.0 {
    return false;
  }
  if expected_duration > 0.0
    && video_duration + 10.0 < expected_duration
    && video_duration / expected_duration < 0.9
  {
    return false;
  }
  if audio_duration > 0.0
    && video_duration + 10.0 < audio_duration
    && video_duration / audio_duration < 0.9
  {
    return false;
  }
  true
}

fn is_audio_complete(video_duration: f64, audio_duration: f64) -> bool {
  if audio_duration <= 0.0 {
    return false;
  }
  if video_duration <= 0.0 {
    return true;
  }
  if audio_duration + 10.0 < video_duration && audio_duration / video_duration < 0.9 {
    return false;
  }
  true
}

fn log_ffprobe_av_duration(
  app_log_path: &Path,
  record_id: i64,
  source: &str,
  output_path: &Path,
  expected_duration: f64,
  video_duration: f64,
  audio_duration: f64,
) {
  let delta = if video_duration > 0.0 && audio_duration > 0.0 {
    audio_duration - video_duration
  } else {
    0.0
  };
  append_log(
    app_log_path,
    &format!(
      "ffprobe_av_duration record_id={} source={} output={} video={:.3} audio={:.3} expected={:.3} delta={:.3}",
      record_id,
      source,
      output_path.to_string_lossy(),
      video_duration,
      audio_duration,
      expected_duration,
      delta
    ),
  );
  if video_duration > 0.0 && audio_duration > 0.0 && delta.abs() >= 1.0 {
    append_log(
      app_log_path,
      &format!(
        "ffprobe_av_mismatch record_id={} source={} output={} video={:.3} audio={:.3} delta={:.3}",
        record_id,
        source,
        output_path.to_string_lossy(),
        video_duration,
        audio_duration,
        delta
      ),
    );
  }
}

fn log_ffprobe_source_duration(
  app_log_path: &Path,
  record_id: i64,
  source: &str,
  path: &Path,
) -> Option<StreamTiming> {
  match probe_stream_timing(path) {
    Ok(timing) => {
      append_log(
        app_log_path,
        &format!(
          "ffprobe_source_duration record_id={} source={} path={} video={:.3} audio={:.3} format={:.3} v_start={:.3} a_start={:.3} f_start={:.3}",
          record_id,
          source,
          path.to_string_lossy(),
          timing.video_duration,
          timing.audio_duration,
          timing.format_duration,
          timing.video_start,
          timing.audio_start,
          timing.format_start
        ),
      );
      Some(timing)
    }
    Err(err) => {
      append_log(
        app_log_path,
        &format!(
          "ffprobe_source_fail record_id={} source={} path={} err={}",
          record_id,
          source,
          path.to_string_lossy(),
          err
        ),
      );
      None
    }
  }
}

#[derive(Default, Clone, Copy)]
struct StreamTiming {
  video_duration: f64,
  audio_duration: f64,
  format_duration: f64,
  video_start: f64,
  audio_start: f64,
  format_start: f64,
}

fn log_ffprobe_av_timing(
  app_log_path: &Path,
  record_id: i64,
  source: &str,
  path: &Path,
) {
  match probe_stream_timing(path) {
    Ok(timing) => {
      let start_delta = timing.audio_start - timing.video_start;
      append_log(
        app_log_path,
        &format!(
          "ffprobe_av_timing record_id={} source={} output={} v_start={:.3} a_start={:.3} f_start={:.3} v_dur={:.3} a_dur={:.3} delta_start={:.3}",
          record_id,
          source,
          path.to_string_lossy(),
          timing.video_start,
          timing.audio_start,
          timing.format_start,
          timing.video_duration,
          timing.audio_duration,
          start_delta
        ),
      );
      if timing.video_duration > 0.0
        && timing.audio_duration > 0.0
        && start_delta.abs() >= 0.1
      {
        append_log(
          app_log_path,
          &format!(
            "ffprobe_av_offset record_id={} source={} output={} delta_start={:.3}",
            record_id,
            source,
            path.to_string_lossy(),
            start_delta
          ),
        );
      }
    }
    Err(err) => {
      append_log(
        app_log_path,
        &format!(
          "ffprobe_av_timing_fail record_id={} source={} output={} err={}",
          record_id,
          source,
          path.to_string_lossy(),
          err
        ),
      );
    }
  }
}

fn probe_stream_timing(path: &Path) -> Result<StreamTiming, String> {
  let args = vec![
    "-v".to_string(),
    "error".to_string(),
    "-show_streams".to_string(),
    "-show_format".to_string(),
    "-of".to_string(),
    "json".to_string(),
    path.to_string_lossy().to_string(),
  ];
  let data = run_ffprobe_json(&args)?;
  let streams = data
    .get("streams")
    .and_then(|value| value.as_array())
    .ok_or_else(|| "Missing stream info".to_string())?;
  let mut timing = StreamTiming::default();
  timing.format_duration = data
    .get("format")
    .and_then(|value| value.get("duration"))
    .and_then(|value| value.as_str())
    .and_then(|value| value.parse::<f64>().ok())
    .unwrap_or(0.0);
  timing.format_start = data
    .get("format")
    .and_then(|value| value.get("start_time"))
    .and_then(|value| value.as_str())
    .and_then(|value| value.parse::<f64>().ok())
    .unwrap_or(0.0);
  for stream in streams {
    let codec_type = stream
      .get("codec_type")
      .and_then(|value| value.as_str())
      .unwrap_or("");
    let duration = stream
      .get("duration")
      .and_then(|value| value.as_str())
      .and_then(|value| value.parse::<f64>().ok())
      .unwrap_or(0.0);
    let start_time = stream
      .get("start_time")
      .and_then(|value| value.as_str())
      .and_then(|value| value.parse::<f64>().ok())
      .unwrap_or(0.0);
    if codec_type == "video" && timing.video_duration <= 0.0 {
      timing.video_duration = duration;
      timing.video_start = start_time;
    }
    if codec_type == "audio" && timing.audio_duration <= 0.0 {
      timing.audio_duration = duration;
      timing.audio_start = start_time;
    }
  }
  if timing.video_duration <= 0.0 && timing.format_duration > 0.0 {
    timing.video_duration = timing.format_duration;
  }
  Ok(timing)
}

fn dedup_urls(urls: Vec<String>) -> Vec<String> {
  let mut seen = HashSet::new();
  let mut result = Vec::new();
  for url in urls {
    if seen.insert(url.clone()) {
      result.push(url);
    }
  }
  result
}

fn filter_pcdn_urls(urls: Vec<String>) -> Vec<String> {
  let mut mirror = Vec::new();
  let mut upos = Vec::new();
  let mut bcache = Vec::new();
  let mut others = Vec::new();
  for raw in urls {
    match Url::parse(&raw) {
      Ok(url) => {
        let host = url.host_str().unwrap_or("");
        let os = url
          .query_pairs()
          .find(|(key, _)| key == "os")
          .map(|(_, value)| value.to_string())
          .unwrap_or_default();
        if host.contains("mirror") && os.ends_with("bv") {
          mirror.push(url);
        } else if os == "upos" {
          upos.push(url);
        } else if host.starts_with("cn") && os == "bcache" {
          bcache.push(url);
        } else {
          others.push(url.to_string());
        }
      }
      Err(_) => {
        others.push(raw);
      }
    }
  }
  if !mirror.is_empty() {
    let mut results = if mirror.len() < 2 {
      let mut combined = mirror;
      combined.extend(upos);
      combined
    } else {
      mirror
    };
    return results.drain(..).map(|url| url.to_string()).collect();
  }
  if !upos.is_empty() || !bcache.is_empty() {
    let mut results = if !upos.is_empty() { upos } else { bcache };
    let mirror_list = ["upos-sz-mirrorali.bilivideo.com", "upos-sz-mirrorcos.bilivideo.com"];
    for (index, url) in results.iter_mut().enumerate() {
      if let Some(host) = mirror_list.get(index) {
        let _ = url.set_host(Some(host));
      }
    }
    return results.drain(..).map(|url| url.to_string()).collect();
  }
  others
}

fn normalize_stream_urls(urls: Vec<String>, block_pcdn: bool) -> Vec<String> {
  let urls = dedup_urls(urls);
  let urls = if block_pcdn {
    filter_pcdn_urls(urls)
  } else {
    urls
  };
  dedup_urls(urls)
}

fn stream_urls_from_item(item: &Value, block_pcdn: bool) -> Vec<String> {
  let mut urls = Vec::new();
  if let Some(url) = item
    .get("base_url")
    .or_else(|| item.get("baseUrl"))
    .and_then(|value| value.as_str())
  {
    urls.push(url.to_string());
  }
  if let Some(list) = item
    .get("backup_url")
    .or_else(|| item.get("backupUrl"))
    .and_then(|value| value.as_array())
  {
    for value in list {
      if let Some(url) = value.as_str() {
        urls.push(url.to_string());
      }
    }
  }
  normalize_stream_urls(urls, block_pcdn)
}

async fn fetch_video_title(
  context: &DownloadContext,
  bvid: Option<&str>,
  aid: Option<&str>,
) -> Option<String> {
  let mut params = Vec::new();
  if let Some(bvid) = bvid {
    params.push(("bvid".to_string(), bvid.to_string()));
  }
  if let Some(aid) = aid {
    params.push(("aid".to_string(), aid.to_string()));
  }

  let auth = load_auth(context);
  let url = format!("{}/x/web-interface/view", context.bilibili.base_url());
  let data = context.bilibili.get_json(&url, &params, auth.as_ref(), false).await.ok()?;
  data
    .get("title")
    .and_then(|value| value.as_str())
    .map(|value| value.to_string())
}

fn parse_video_id(url: &str) -> (Option<String>, Option<String>) {
  if let Some(bvid) = extract_bvid(url) {
    return (Some(bvid), None);
  }

  if let Some(aid) = extract_aid(url) {
    return (None, Some(aid));
  }

  (None, None)
}

fn extract_bvid(input: &str) -> Option<String> {
  if let Some(index) = input.find("BV") {
    let value = &input[index..];
    let end = value
      .find(|ch: char| !ch.is_ascii_alphanumeric())
      .unwrap_or(value.len());
    let bvid = &value[..end];
    if bvid.len() > 2 {
      return Some(bvid.to_string());
    }
  }
  None
}

fn extract_aid(input: &str) -> Option<String> {
  if let Some(index) = input.find("av") {
    let value = &input[index + 2..];
    let digits: String = value.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    if !digits.is_empty() {
      return Some(digits);
    }
  }

  if input.chars().all(|ch| ch.is_ascii_digit()) {
    return Some(input.to_string());
  }

  None
}

fn build_ffmpeg_headers(context: &DownloadContext) -> Option<String> {
  let auth = load_auth(context)?;
  let mut headers = String::new();
  headers.push_str("Referer: https://www.bilibili.com\r\n");
  headers.push_str("Origin: https://www.bilibili.com\r\n");
  headers.push_str("User-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36\r\n");
  headers.push_str(&format!("Cookie: {}\r\n", auth.cookie));
  Some(headers)
}

fn load_auth(context: &DownloadContext) -> Option<AuthInfo> {
  context
    .login_store
    .load_primary_auth_info(&context.db)
    .ok()
    .flatten()
}

fn update_download_status(
  context: &DownloadContext,
  record_id: i64,
  status: i64,
  progress: i64,
) -> Result<(), String> {
  let now = now_rfc3339();
  context
    .db
    .with_conn(|conn| {
      conn.execute(
        "UPDATE video_download SET status = ?1, progress = ?2, update_time = ?3 WHERE id = ?4",
        (status, progress, &now, record_id),
      )?;
      Ok(())
    })
    .map_err(|err| format!("Failed to update download status: {}", err))
}

fn reset_download_record_progress(
  context: &DownloadContext,
  record_id: i64,
) -> Result<(), String> {
  let now = now_rfc3339();
  context
    .db
    .with_conn(|conn| {
      conn.execute(
        "UPDATE video_download SET status = 0, progress = 0, progress_total = 0, progress_done = 0, update_time = ?1 WHERE id = ?2",
        (&now, record_id),
      )?;
      Ok(())
    })
    .map_err(|err| format!("Failed to reset download progress: {}", err))
}

fn update_download_progress(
  context: &DownloadContext,
  record_id: i64,
  progress: i64,
) -> Result<(), String> {
  let now = now_rfc3339();
  context
    .db
    .with_conn(|conn| {
      conn.execute(
        "UPDATE video_download SET progress = CASE WHEN progress < ?1 THEN ?1 ELSE progress END, update_time = ?2 \
         WHERE id = ?3 AND status = 1",
        (progress, &now, record_id),
      )?;
      Ok(())
    })
    .map_err(|err| format!("Failed to update download progress: {}", err))
}

fn reset_download_progress_bytes(
  context: &DownloadContext,
  record_id: i64,
) -> Result<(), String> {
  let now = now_rfc3339();
  context
    .db
    .with_conn(|conn| {
      conn.execute(
        "UPDATE video_download SET progress_total = 0, progress_done = 0, update_time = ?1 \
         WHERE id = ?2 AND status = 1",
        (&now, record_id),
      )?;
      Ok(())
    })
    .map_err(|err| format!("Failed to reset download bytes: {}", err))
}

fn update_download_status_only(
  context: &DownloadContext,
  record_id: i64,
  status: i64,
) -> Result<(), String> {
  let now = now_rfc3339();
  context
    .db
    .with_conn(|conn| {
      conn.execute(
        "UPDATE video_download SET status = ?1, update_time = ?2 WHERE id = ?3",
        (status, &now, record_id),
      )?;
      Ok(())
    })
    .map_err(|err| format!("Failed to update download status: {}", err))
}

fn clear_download_progress(context: &DownloadContext, record_id: i64) {
  if let Ok(mut state) = context.download_runtime.progress_state.lock() {
    state.remove(&record_id);
  }
}

fn update_download_bytes(
  context: &DownloadContext,
  record_id: i64,
  key: &str,
  content: u64,
  chunk: u64,
) -> Result<(), String> {
  let mut state = context
    .download_runtime
    .progress_state
    .lock()
    .map_err(|_| "Download progress lock failed".to_string())?;
  let entry = state.entry(record_id).or_insert_with(HashMap::new);
  entry.insert(key.to_string(), (content, chunk.min(content)));
  let total_content: u64 = entry.values().map(|(value, _)| *value).sum();
  if total_content == 0 {
    return Ok(());
  }
  let total_chunk: u64 = entry.values().map(|(_, value)| *value).sum();
  let progress = ((total_chunk.saturating_mul(100)) / total_content) as i64;
  let progress = progress.min(99);
  let total_content = i64::try_from(total_content).unwrap_or(i64::MAX);
  let total_chunk = i64::try_from(total_chunk).unwrap_or(i64::MAX);
  let now = now_rfc3339();
  context
    .db
    .with_conn(|conn| {
      conn.execute(
        "UPDATE video_download SET progress_total = CASE WHEN progress_total < ?1 THEN ?1 ELSE progress_total END, \
         progress_done = CASE WHEN progress_done < ?2 THEN ?2 ELSE progress_done END, \
         progress = CASE WHEN progress < ?3 THEN ?3 ELSE progress END, update_time = ?4 \
         WHERE id = ?5 AND status = 1",
        (total_content, total_chunk, progress, &now, record_id),
      )?;
      Ok(())
    })
    .map_err(|err| format!("Failed to update download progress: {}", err))
}

fn is_dir_empty(dir: &Path) -> bool {
  let entries = match std::fs::read_dir(dir) {
    Ok(value) => value,
    Err(_) => return false,
  };
  for entry in entries.flatten() {
    let name = entry.file_name().to_string_lossy().to_string();
    if name == ".DS_Store" {
      continue;
    }
    return false;
  }
  true
}

fn load_submission_status(context: &DownloadContext, task_id: &str) -> Result<String, String> {
  context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare("SELECT status FROM submission_task WHERE task_id = ?1")?;
      let status = stmt.query_row([task_id], |row| row.get(0))?;
      Ok(status)
    })
    .map_err(|err| err.to_string())
}

fn load_relation_workflow_status(
  context: &DownloadContext,
  task_id: &str,
) -> Result<Option<String>, String> {
  context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT workflow_status FROM task_relations WHERE submission_task_id = ?1 ORDER BY updated_at DESC LIMIT 1",
      )?;
      let status: Option<String> = stmt.query_row([task_id], |row| row.get(0)).ok();
      Ok(status)
    })
    .map_err(|err| err.to_string())
}

fn update_submission_status(
  context: &DownloadContext,
  task_id: &str,
  status: &str,
) -> Result<(), String> {
  let now = now_rfc3339();
  context
    .db
    .with_conn(|conn| {
      conn.execute(
        "UPDATE submission_task SET status = ?1, updated_at = ?2 WHERE task_id = ?3",
        (status, &now, task_id),
      )?;
      Ok(())
    })
    .map_err(|err| err.to_string())
}

fn update_relation_workflow_status(
  context: &DownloadContext,
  submission_task_id: &str,
  workflow_status: &str,
) -> Result<(), String> {
  let now = now_rfc3339();
  context
    .db
    .with_conn(|conn| {
      conn.execute(
        "UPDATE task_relations SET workflow_status = ?1, updated_at = ?2 WHERE submission_task_id = ?3",
        (workflow_status, &now, submission_task_id),
      )?;
      Ok(())
    })
    .map_err(|err| err.to_string())
}

fn update_workflow_instance_status(
  context: &DownloadContext,
  task_id: &str,
  status: &str,
) -> Result<(), String> {
  let now = now_rfc3339();
  context
    .db
    .with_conn(|conn| {
      let updated = conn.execute(
        "UPDATE workflow_instances SET status = ?1, updated_at = ?2 WHERE task_id = ?3",
        (status, &now, task_id),
      )?;
      Ok(updated)
    })
    .map_err(|err| err.to_string())?;
  Ok(())
}

fn reset_workflow_instance_state(
  context: &DownloadContext,
  task_id: &str,
) -> Result<(), String> {
  let now = now_rfc3339();
  context
    .db
    .with_conn(|conn| {
      conn.execute(
        "UPDATE workflow_instances SET status = 'PENDING', current_step = NULL, progress = 0, updated_at = ?1 WHERE task_id = ?2",
        (&now, task_id),
      )?;
      Ok(())
    })
    .map_err(|err| err.to_string())
}

fn load_workflow_instance_status(
  context: &DownloadContext,
  task_id: &str,
) -> Result<Option<String>, String> {
  context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT status FROM workflow_instances WHERE task_id = ?1 ORDER BY created_at DESC LIMIT 1",
      )?;
      let status: Option<String> = stmt.query_row([task_id], |row| row.get(0)).ok();
      Ok(status)
    })
    .map_err(|err| err.to_string())
}

fn reset_integrated_submission_status(
  context: &DownloadContext,
  record_id: i64,
) -> Result<(), String> {
  let submission_task_id = context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT submission_task_id FROM task_relations WHERE download_task_id = ?1 AND relation_type = 'INTEGRATED' LIMIT 1",
      )?;
      let value: Option<String> = stmt.query_row([record_id], |row| row.get(0)).ok();
      Ok(value)
    })
    .map_err(|err| err.to_string())?;

  let submission_task_id = match submission_task_id {
    Some(task_id) => task_id,
    None => return Ok(()),
  };

  let submission_status = load_submission_status(context, &submission_task_id).unwrap_or_default();
  let relation_status = load_relation_workflow_status(context, &submission_task_id)?;
  let workflow_status = load_workflow_instance_status(context, &submission_task_id)?;
  let should_reset = submission_status == "FAILED"
    || relation_status.as_deref() == Some("DOWNLOAD_FAILED")
    || workflow_status.as_deref() == Some("FAILED");

  if !should_reset {
    return Ok(());
  }

  let _ = update_submission_status(context, &submission_task_id, "PENDING");
  let _ = update_relation_workflow_status(context, &submission_task_id, "PENDING_DOWNLOAD");
  let _ = reset_workflow_instance_state(context, &submission_task_id);
  append_log(
    &context.app_log_path,
    &format!(
      "download_retry_reset_submission task_id={} download_id={}",
      submission_task_id, record_id
    ),
  );
  Ok(())
}

async fn refresh_integration_status(
  context: &DownloadContext,
  record_id: i64,
) -> Result<(), String> {
  let submission_task_id = context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT submission_task_id FROM task_relations WHERE download_task_id = ?1 AND relation_type = 'INTEGRATED' LIMIT 1",
      )?;
      let value: Option<String> = stmt.query_row([record_id], |row| row.get(0)).ok();
      Ok(value)
    })
    .map_err(|err| err.to_string())?;

  let submission_task_id = match submission_task_id {
    Some(task_id) => task_id,
    None => return Ok(()),
  };

  let (total, completed, failed) = context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT \
          COUNT(*) AS total, \
          SUM(CASE WHEN vd.status = 2 THEN 1 ELSE 0 END) AS completed, \
          SUM(CASE WHEN vd.status = 3 THEN 1 ELSE 0 END) AS failed \
         FROM task_relations tr \
         JOIN video_download vd ON tr.download_task_id = vd.id \
         WHERE tr.submission_task_id = ?1 AND tr.relation_type = 'INTEGRATED'",
      )?;
      let row = stmt.query_row([&submission_task_id], |row| {
        let total: i64 = row.get(0)?;
        let completed: Option<i64> = row.get(1)?;
        let failed: Option<i64> = row.get(2)?;
        Ok((total, completed.unwrap_or(0), failed.unwrap_or(0)))
      })?;
      Ok(row)
    })
    .map_err(|err| err.to_string())?;

  if total == 0 {
    return Ok(());
  }

  if failed > 0 {
    let current_status = load_submission_status(context, &submission_task_id).unwrap_or_default();
    if current_status != "COMPLETED" {
      let _ = update_submission_status(context, &submission_task_id, "PENDING");
    }
    let _ = update_relation_workflow_status(context, &submission_task_id, "PENDING_DOWNLOAD");
    return Ok(());
  }

  if completed == total {
    let _ = update_relation_workflow_status(context, &submission_task_id, "READY");
    let submission_status = load_submission_status(context, &submission_task_id)?;
    if submission_status == "FAILED" {
      return Ok(());
    }
    if let Some(status) = load_workflow_instance_status(context, &submission_task_id)? {
      if status == "VIDEO_DOWNLOADING" {
        let _ = update_workflow_instance_status(context, &submission_task_id, "COMPLETED");
        return Ok(());
      }
      if status == "RUNNING" || status == "COMPLETED" {
        return Ok(());
      }
    }
    let task_id = submission_task_id.clone();
    crate::commands::submission::start_submission_workflow(
      context.db.clone(),
      context.app_log_path.clone(),
      context.edit_upload_state.clone(),
      task_id,
    );
    let _ = update_relation_workflow_status(context, &submission_task_id, "WORKFLOW_STARTED");
  }

  Ok(())
}
