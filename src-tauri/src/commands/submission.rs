use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{ErrorKind, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::Utc;
use futures_util::stream::{FuturesUnordered, StreamExt};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, CONTENT_TYPE, USER_AGENT};
use reqwest::{Client, StatusCode};
use rusqlite::params;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};
use tauri::State;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::sleep;
use url::form_urlencoded;

use crate::api::ApiResponse;
use crate::baidu_sync;
use crate::bilibili::client::BilibiliClient;
use crate::commands::settings::{
  load_download_settings_from_db, DEFAULT_SUBMISSION_REMOTE_REFRESH_MINUTES,
  DEFAULT_UPLOAD_CONCURRENCY,
};
use crate::config::default_download_dir;
use crate::db::Db;
use crate::login_refresh;
use crate::login_store::{AuthInfo, LoginStore};
use crate::processing::{
  clip_sources, decide_clip_copy, merge_files, parse_time_to_seconds, probe_duration_seconds,
  segment_file, ClipSource,
};
use crate::utils::{append_log, now_rfc3339, sanitize_filename};
use crate::AppState;

#[derive(Clone)]
struct SubmissionContext {
  db: Arc<Db>,
  app_log_path: Arc<PathBuf>,
  edit_upload_state: Arc<Mutex<EditUploadState>>,
}

impl SubmissionContext {
  fn new(state: &State<'_, AppState>) -> Self {
    Self {
      db: state.db.clone(),
      app_log_path: state.app_log_path.clone(),
      edit_upload_state: state.edit_upload_state.clone(),
    }
  }
}

#[derive(Clone)]
struct UploadContext {
  db: Arc<Db>,
  bilibili: Arc<BilibiliClient>,
  login_store: Arc<LoginStore>,
  app_log_path: Arc<PathBuf>,
  edit_upload_state: Arc<Mutex<EditUploadState>>,
}

impl UploadContext {
  fn new(state: &State<'_, AppState>) -> Self {
    Self {
      db: state.db.clone(),
      bilibili: state.bilibili.clone(),
      login_store: state.login_store.clone(),
      app_log_path: state.app_log_path.clone(),
      edit_upload_state: state.edit_upload_state.clone(),
    }
  }
}

#[derive(Clone)]
struct SubmissionQueueContext {
  db: Arc<Db>,
  bilibili: Arc<BilibiliClient>,
  login_store: Arc<LoginStore>,
  app_log_path: Arc<PathBuf>,
  edit_upload_state: Arc<Mutex<EditUploadState>>,
}

fn build_submission_queue_context(state: &State<'_, AppState>) -> SubmissionQueueContext {
  SubmissionQueueContext {
    db: state.db.clone(),
    bilibili: state.bilibili.clone(),
    login_store: state.login_store.clone(),
    app_log_path: state.app_log_path.clone(),
    edit_upload_state: state.edit_upload_state.clone(),
  }
}

pub fn start_submission_background_tasks(
  db: Arc<Db>,
  bilibili: Arc<BilibiliClient>,
  login_store: Arc<LoginStore>,
  app_log_path: Arc<PathBuf>,
  edit_upload_state: Arc<Mutex<EditUploadState>>,
) {
  let context = SubmissionQueueContext {
    db,
    bilibili,
    login_store,
    app_log_path,
    edit_upload_state,
  };
  let recovery_context = context.clone();
  tauri::async_runtime::spawn(async move {
    recover_submission_tasks(recovery_context).await;
  });
  let queue_context = context.clone();
  tauri::async_runtime::spawn(async move {
    submission_queue_loop(queue_context).await;
  });
  let refresh_context = context.clone();
  tauri::async_runtime::spawn(async move {
    submission_remote_refresh_loop(refresh_context).await;
  });
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionTaskInput {
  pub title: String,
  pub description: Option<String>,
  pub cover_url: Option<String>,
  pub partition_id: i64,
  pub collection_id: Option<i64>,
  pub tags: Option<String>,
  pub topic_id: Option<i64>,
  pub mission_id: Option<i64>,
  pub activity_title: Option<String>,
  pub video_type: String,
  pub segment_prefix: Option<String>,
  pub priority: Option<bool>,
  pub baidu_sync_enabled: Option<bool>,
  pub baidu_sync_path: Option<String>,
  pub baidu_sync_filename: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceVideoInput {
  pub source_file_path: String,
  pub sort_order: i64,
  pub start_time: Option<String>,
  pub end_time: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionCreateRequest {
  pub task: SubmissionTaskInput,
  pub source_videos: Vec<SourceVideoInput>,
  pub workflow_config: Option<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionUpdateRequest {
  pub task_id: String,
  pub source_videos: Vec<SourceVideoInput>,
  pub workflow_config: Option<Value>,
  pub baidu_sync_enabled: Option<bool>,
  pub baidu_sync_path: Option<String>,
  pub baidu_sync_filename: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionResegmentRequest {
  pub task_id: String,
  pub segment_duration_seconds: i64,
  pub mode: Option<String>,
  pub merged_video_id: Option<i64>,
  pub integrate_current_bvid: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionRepostRequest {
  pub task_id: String,
  pub integrate_current_bvid: bool,
  pub mode: Option<String>,
  pub merged_video_id: Option<i64>,
  pub baidu_sync_enabled: Option<bool>,
  pub baidu_sync_path: Option<String>,
  pub baidu_sync_filename: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum ReprocessMode {
  Legacy,
  Specified,
  MergeAll,
  FullReprocess,
}

fn parse_reprocess_mode(mode: Option<&str>) -> ReprocessMode {
  let normalized = mode.unwrap_or("").trim().to_ascii_uppercase();
  match normalized.as_str() {
    "SPECIFIED" => ReprocessMode::Specified,
    "MERGE_ALL" => ReprocessMode::MergeAll,
    "FULL_REPROCESS" => ReprocessMode::FullReprocess,
    _ => ReprocessMode::Legacy,
  }
}

fn reprocess_mode_to_str(mode: ReprocessMode) -> &'static str {
  match mode {
    ReprocessMode::Specified => "SPECIFIED",
    ReprocessMode::MergeAll => "MERGE_ALL",
    ReprocessMode::FullReprocess => "FULL_REPROCESS",
    ReprocessMode::Legacy => "LEGACY",
  }
}

fn apply_reprocess_metadata(
  config: &mut Value,
  mode: ReprocessMode,
  merged_video_id: Option<i64>,
) {
  if !config.is_object() {
    *config = Value::Object(Map::new());
  }
  let Some(map) = config.as_object_mut() else {
    return;
  };
  map.insert(
    "reprocessMode".to_string(),
    Value::String(reprocess_mode_to_str(mode).to_string()),
  );
  if let Some(merged_id) = merged_video_id {
    map.insert("reprocessMergedId".to_string(), Value::Number(Number::from(merged_id)));
  } else {
    map.remove("reprocessMergedId");
  }
}

fn load_reprocess_metadata(config: Option<&Value>) -> (ReprocessMode, Option<i64>) {
  let Some(config) = config else {
    return (ReprocessMode::Legacy, None);
  };
  let mode = config
    .get("reprocessMode")
    .and_then(|value| value.as_str())
    .map(|value| parse_reprocess_mode(Some(value)))
    .unwrap_or(ReprocessMode::Legacy);
  let merged_id = config
    .get("reprocessMergedId")
    .and_then(|value| value.as_i64());
  (mode, merged_id)
}

fn apply_integrate_current_bvid(config: &mut Value, integrate_current_bvid: bool) {
  if !config.is_object() {
    *config = Value::Object(Map::new());
  }
  let Some(map) = config.as_object_mut() else {
    return;
  };
  map.insert(
    "integrateCurrentBvid".to_string(),
    Value::Bool(integrate_current_bvid),
  );
}

fn load_integrate_current_bvid(config: Option<&Value>) -> bool {
  let Some(config) = config else {
    return false;
  };
  config
    .get("integrateCurrentBvid")
    .and_then(|value| value.as_bool())
    .unwrap_or(false)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionEditTaskInput {
  pub title: String,
  pub description: Option<String>,
  pub partition_id: i64,
  pub collection_id: Option<i64>,
  pub tags: Option<String>,
  pub topic_id: Option<i64>,
  pub mission_id: Option<i64>,
  pub activity_title: Option<String>,
  pub video_type: String,
  pub segment_prefix: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionEditSegmentInput {
  pub segment_id: String,
  pub part_name: String,
  pub part_order: i64,
  pub segment_file_path: String,
  pub cid: Option<i64>,
  pub file_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionEditSubmitRequest {
  pub task_id: String,
  pub task: SubmissionEditTaskInput,
  pub segments: Vec<SubmissionEditSegmentInput>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionEditAddSegmentRequest {
  pub task_id: String,
  pub file_path: String,
  pub part_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionEditReuploadSegmentRequest {
  pub task_id: String,
  pub segment_id: String,
  pub file_path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionEditUploadStatusRequest {
  pub task_id: String,
  pub segment_ids: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionEditUploadClearRequest {
  pub task_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionDeleteRequest {
  pub task_id: String,
  pub delete_task: bool,
  pub delete_files: bool,
  pub delete_paths: Option<Vec<String>>,
  pub force_delete: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskCreationResult {
  pub task_id: String,
  pub workflow_instance_id: Option<String>,
  pub workflow_status: Option<String>,
  pub workflow_error: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowStatusRecord {
  pub status: String,
  pub current_step: Option<String>,
  pub progress: f64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionTaskRecord {
  pub task_id: String,
  pub status: String,
  pub priority: bool,
  pub title: String,
  pub description: Option<String>,
  pub cover_url: Option<String>,
  pub partition_id: i64,
  pub tags: Option<String>,
  pub topic_id: Option<i64>,
  pub mission_id: Option<i64>,
  pub activity_title: Option<String>,
  pub video_type: String,
  pub collection_id: Option<i64>,
  pub bvid: Option<String>,
  pub aid: Option<i64>,
  pub remote_state: Option<i64>,
  pub reject_reason: Option<String>,
  pub created_at: String,
  pub updated_at: String,
  pub segment_prefix: Option<String>,
  pub baidu_sync_enabled: bool,
  pub baidu_sync_path: Option<String>,
  pub baidu_sync_filename: Option<String>,
  pub has_integrated_downloads: bool,
  pub workflow_status: Option<WorkflowStatusRecord>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaginatedSubmissionTasks {
  pub items: Vec<SubmissionTaskRecord>,
  pub total: i64,
  pub page: i64,
  pub page_size: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSourceVideoRecord {
  pub id: String,
  pub task_id: String,
  pub source_file_path: String,
  pub sort_order: i64,
  pub start_time: Option<String>,
  pub end_time: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteConflictRef {
  pub task_id: String,
  pub status: String,
  pub title: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteFilePreview {
  pub path: String,
  pub conflicts: Vec<DeleteConflictRef>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionDeletePreview {
  pub task_id: String,
  pub files: Vec<DeleteFilePreview>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionDeleteResult {
  pub blocked: bool,
  pub conflicts: Vec<DeleteFilePreview>,
  pub deleted_paths: Vec<String>,
  pub missing_paths: Vec<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskOutputSegmentRecord {
  pub segment_id: String,
  pub task_id: String,
  pub merged_id: Option<i64>,
  pub part_name: String,
  pub segment_file_path: String,
  pub part_order: i64,
  pub upload_status: String,
  pub cid: Option<i64>,
  pub file_name: Option<String>,
  pub upload_progress: f64,
  pub upload_uploaded_bytes: i64,
  pub upload_total_bytes: i64,
  pub upload_session_id: Option<String>,
  pub upload_biz_id: i64,
  pub upload_endpoint: Option<String>,
  pub upload_auth: Option<String>,
  pub upload_uri: Option<String>,
  pub upload_chunk_size: i64,
  pub upload_last_part_index: i64,
}

#[derive(Default)]
pub struct EditUploadState {
  segments: HashMap<String, TaskOutputSegmentRecord>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergedVideoRecord {
  pub id: i64,
  pub task_id: String,
  pub file_name: Option<String>,
  pub video_path: Option<String>,
  pub remote_dir: Option<String>,
  pub remote_name: Option<String>,
  pub duration: Option<i64>,
  pub status: i64,
  pub upload_progress: f64,
  pub upload_uploaded_bytes: i64,
  pub upload_total_bytes: i64,
  pub upload_cid: Option<i64>,
  pub upload_file_name: Option<String>,
  pub upload_session_id: Option<String>,
  pub upload_biz_id: i64,
  pub upload_endpoint: Option<String>,
  pub upload_auth: Option<String>,
  pub upload_uri: Option<String>,
  pub upload_chunk_size: i64,
  pub upload_last_part_index: i64,
  pub create_time: String,
  pub update_time: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionTaskDetail {
  pub task: SubmissionTaskRecord,
  pub source_videos: Vec<TaskSourceVideoRecord>,
  pub output_segments: Vec<TaskOutputSegmentRecord>,
  pub merged_videos: Vec<MergedVideoRecord>,
  pub workflow_config: Option<Value>,
}

fn with_edit_upload_state<T>(
  context: &SubmissionContext,
  action: impl FnOnce(&mut EditUploadState) -> T,
) -> Result<T, String> {
  let mut guard = context
    .edit_upload_state
    .lock()
    .map_err(|_| "编辑上传状态不可用".to_string())?;
  Ok(action(&mut guard))
}

fn upsert_edit_upload_segment(
  context: &SubmissionContext,
  segment: TaskOutputSegmentRecord,
) -> Result<TaskOutputSegmentRecord, String> {
  with_edit_upload_state(context, |state| {
    state
      .segments
      .insert(segment.segment_id.clone(), segment.clone());
    segment
  })
}

fn load_edit_upload_segment(
  context: &SubmissionContext,
  segment_id: &str,
) -> Result<Option<TaskOutputSegmentRecord>, String> {
  with_edit_upload_state(context, |state| state.segments.get(segment_id).cloned())
}

fn update_edit_upload_segment(
  context: &SubmissionContext,
  segment_id: &str,
  updater: impl FnOnce(&mut TaskOutputSegmentRecord),
) -> Result<(), String> {
  with_edit_upload_state(context, |state| {
    if let Some(segment) = state.segments.get_mut(segment_id) {
      updater(segment);
      return true;
    }
    false
  })?
  .then_some(())
  .ok_or_else(|| "未找到编辑分P".to_string())
}

fn list_edit_upload_segments_by_task(
  context: &SubmissionContext,
  task_id: &str,
  segment_ids: Option<&[String]>,
) -> Result<Vec<TaskOutputSegmentRecord>, String> {
  with_edit_upload_state(context, |state| {
    let filter_ids = segment_ids.map(|ids| ids.iter().cloned().collect::<HashSet<_>>());
    state
      .segments
      .values()
      .filter(|segment| segment.task_id == task_id)
      .filter(|segment| match &filter_ids {
        Some(ids) => ids.contains(&segment.segment_id),
        None => true,
      })
      .cloned()
      .collect::<Vec<_>>()
  })
}

fn clear_edit_upload_segments_by_task(
  context: &SubmissionContext,
  task_id: &str,
) -> Result<(), String> {
  with_edit_upload_state(context, |state| {
    state.segments.retain(|_, segment| segment.task_id != task_id);
  })
}

#[tauri::command]
pub async fn submission_create(
  state: State<'_, AppState>,
  request: SubmissionCreateRequest,
) -> Result<ApiResponse<TaskCreationResult>, String> {
  let context = SubmissionContext::new(&state);
  let task_id = uuid::Uuid::new_v4().to_string();
  let now = now_rfc3339();
  append_log(
    &state.app_log_path,
    &format!("submission_create_start task_id={}", task_id),
  );

  let result = context.db.with_conn(|conn| {
    let normalized_baidu_sync_filename =
      normalize_baidu_sync_filename(request.task.baidu_sync_filename.as_deref());
    conn.execute(
      "INSERT INTO submission_task (task_id, status, priority, title, description, cover_url, partition_id, tags, topic_id, mission_id, activity_title, video_type, collection_id, bvid, aid, created_at, updated_at, segment_prefix, baidu_sync_enabled, baidu_sync_path, baidu_sync_filename) \
       VALUES (?1, 'PENDING', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, NULL, NULL, ?13, ?14, ?15, ?16, ?17, ?18)",
      params![
        &task_id,
        if request.task.priority.unwrap_or(false) { 1 } else { 0 },
        &request.task.title,
        request.task.description.as_deref(),
        request.task.cover_url.as_deref(),
        request.task.partition_id,
        request.task.tags.as_deref(),
        request.task.topic_id,
        request.task.mission_id,
        request.task.activity_title.as_deref(),
        &request.task.video_type,
        request.task.collection_id,
        &now,
        &now,
        request.task.segment_prefix.as_deref(),
        if request.task.baidu_sync_enabled.unwrap_or(false) {
          1
        } else {
          0
        },
        request.task.baidu_sync_path.as_deref(),
        normalized_baidu_sync_filename.as_deref(),
      ],
    )?;

    for source in &request.source_videos {
      let source_id = uuid::Uuid::new_v4().to_string();
      conn.execute(
        "INSERT INTO task_source_video (id, task_id, source_file_path, sort_order, start_time, end_time) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        (
          source_id,
          &task_id,
          &source.source_file_path,
          source.sort_order,
          source.start_time.as_deref(),
          source.end_time.as_deref(),
        ),
      )?;
    }

    Ok(())
  });

  if let Err(err) = result {
    return Ok(ApiResponse::error(format!("Failed to create task: {}", err)));
  }
  append_log(
    &state.app_log_path,
    &format!("submission_create_ok task_id={}", task_id),
  );

  let mut result = TaskCreationResult {
    task_id: task_id.clone(),
    workflow_instance_id: None,
    workflow_status: None,
    workflow_error: None,
  };

  if let Some(config) = request.workflow_config {
    match create_workflow_instance(&context, &task_id, &config) {
      Ok((instance_id, status)) => {
        result.workflow_instance_id = Some(instance_id);
        result.workflow_status = Some(status);
      }
      Err(err) => {
        result.workflow_error = Some(err);
      }
    }
  }

  if result.workflow_instance_id.is_some() {
    let context_clone = context.clone();
    let task_id_clone = task_id.clone();
    tauri::async_runtime::spawn(async move {
      let _ = run_submission_workflow(context_clone, task_id_clone).await;
    });
  }

  Ok(ApiResponse::success(result))
}

#[tauri::command]
pub async fn submission_update(
  state: State<'_, AppState>,
  request: SubmissionUpdateRequest,
) -> Result<ApiResponse<String>, String> {
  let context = SubmissionContext::new(&state);
  let task_id = request.task_id.trim().to_string();
  if task_id.is_empty() {
    return Ok(ApiResponse::error("任务ID不能为空"));
  }
  let detail = match load_task_detail(&context, &task_id) {
    Ok(detail) => detail,
    Err(err) => return Ok(ApiResponse::error(err)),
  };
  if let Err(err) = ensure_editable_detail(&detail) {
    return Ok(ApiResponse::error(err));
  }
  if request.source_videos.is_empty() {
    return Ok(ApiResponse::error("请至少添加一个源视频"));
  }
  let workflow_config = match request.workflow_config {
    Some(config) => config,
    None => return Ok(ApiResponse::error("工作流配置不能为空")),
  };
  let workflow_config = attach_update_sources(workflow_config, &request.source_videos);
  append_log(
    &state.app_log_path,
    &format!("submission_update_start task_id={}", task_id),
  );
  if let Err(err) = append_source_videos(&context, &task_id, &request.source_videos) {
    return Ok(ApiResponse::error(format!("追加源视频失败: {}", err)));
  }
  if let Err(err) = update_baidu_sync_config(
    &context,
    &task_id,
    request.baidu_sync_enabled,
    normalize_optional_text(request.baidu_sync_path),
    normalize_optional_text(request.baidu_sync_filename),
  ) {
    return Ok(ApiResponse::error(format!("更新百度同步配置失败: {}", err)));
  }
  if let Err(err) = reset_workflow_instances(&context, &task_id) {
    return Ok(ApiResponse::error(format!("重置工作流失败: {}", err)));
  }
  if let Err(err) = update_submission_status(&context, &task_id, "PENDING") {
    return Ok(ApiResponse::error(format!("更新任务状态失败: {}", err)));
  }
  if let Err(err) = create_workflow_instance_for_task_with_type(
    context.db.as_ref(),
    &task_id,
    &workflow_config,
    "VIDEO_UPDATE",
  ) {
    return Ok(ApiResponse::error(err));
  }
  start_submission_workflow(
    context.db.clone(),
    context.app_log_path.clone(),
    context.edit_upload_state.clone(),
    task_id,
  );
  Ok(ApiResponse::success("更新任务已启动".to_string()))
}

#[tauri::command]
pub async fn submission_repost(
  state: State<'_, AppState>,
  request: SubmissionRepostRequest,
) -> Result<ApiResponse<String>, String> {
  let context = SubmissionContext::new(&state);
  let task_id = request.task_id.trim().to_string();
  if task_id.is_empty() {
    return Ok(ApiResponse::error("任务ID不能为空"));
  }
  let detail = match load_task_detail(&context, &task_id) {
    Ok(detail) => detail,
    Err(err) => return Ok(ApiResponse::error(err)),
  };
  let mode = parse_reprocess_mode(request.mode.as_deref());
  if mode == ReprocessMode::FullReprocess {
    let mut workflow_config = match detail.workflow_config {
      Some(ref config) => config.clone(),
      None => return Ok(ApiResponse::error("未找到工作流配置")),
    };
    apply_reprocess_metadata(&mut workflow_config, mode, request.merged_video_id);
    let workflow_config = strip_update_sources(&workflow_config);
    let integrate_current_bvid = request.integrate_current_bvid;
    if integrate_current_bvid {
      let has_bvid = detail
        .task
        .bvid
        .as_deref()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
      if !has_bvid {
        return Ok(ApiResponse::error("当前任务没有BV号，无法集成投稿"));
      }
    }
    if let Err(err) = update_baidu_sync_config(
      &context,
      &task_id,
      request.baidu_sync_enabled,
      normalize_optional_text(request.baidu_sync_path),
      normalize_optional_text(request.baidu_sync_filename),
    ) {
      return Ok(ApiResponse::error(format!("更新百度同步配置失败: {}", err)));
    }
    append_log(
      &state.app_log_path,
      &format!(
        "submission_repost_start task_id={} mode=FULL_REPROCESS type={}",
        task_id,
        if integrate_current_bvid { "UPDATE" } else { "NEW" }
      ),
    );
    let missing_sources = collect_missing_source_files(&detail.source_videos);
    if !missing_sources.is_empty() {
      append_log(
        &state.app_log_path,
        &format!(
          "submission_repost_missing_sources task_id={} count={}",
          task_id,
          missing_sources.len()
        ),
      );
      for path in &missing_sources {
        append_log(
          &state.app_log_path,
          &format!("submission_repost_missing_source task_id={} path={}", task_id, path),
        );
      }
      let integrated_records = load_integrated_download_records(&context, &task_id)?;
      if integrated_records.is_empty() {
        return Ok(ApiResponse::error("源视频不存在，请先下载"));
      }
      let mut records_by_path: HashMap<String, IntegratedDownloadRecord> = HashMap::new();
      for record in integrated_records {
        if !record.local_path.trim().is_empty() {
          records_by_path.insert(record.local_path.clone(), record);
        }
      }
      let mut missing_records = Vec::new();
      let mut missing_without_download = Vec::new();
      for path in &missing_sources {
        if let Some(record) = records_by_path.get(path) {
          missing_records.push(record.clone());
        } else {
          missing_without_download.push(path.clone());
        }
      }
      if !missing_without_download.is_empty() {
        append_log(
          &state.app_log_path,
          &format!(
            "submission_repost_missing_unbound task_id={} count={}",
            task_id,
            missing_without_download.len()
          ),
        );
        return Ok(ApiResponse::error("源视频不存在，请先下载"));
      }
      let workflow_instance_id = reset_submission_for_repost(
        &context,
        &state.app_log_path,
        &task_id,
        &workflow_config,
        if integrate_current_bvid { "VIDEO_UPDATE" } else { "VIDEO_SUBMISSION" },
        !integrate_current_bvid,
        true,
      )?;
      let new_download_ids =
        create_retry_download_records(&context, &task_id, &workflow_instance_id, &missing_records)?;
      crate::commands::download::requeue_integrated_downloads(&state, &new_download_ids).await?;
      return Ok(ApiResponse::success(
        "源视频缺失，已创建下载任务，下载完成后自动重新投稿".to_string(),
      ));
    }
    let _ = reset_submission_for_repost(
      &context,
      &state.app_log_path,
      &task_id,
      &workflow_config,
      if integrate_current_bvid { "VIDEO_UPDATE" } else { "VIDEO_SUBMISSION" },
      !integrate_current_bvid,
      true,
    )?;
    start_submission_workflow(
      context.db.clone(),
      context.app_log_path.clone(),
      context.edit_upload_state.clone(),
      task_id,
    );
    return Ok(ApiResponse::success("全部重新投稿已启动".to_string()));
  }
  if mode != ReprocessMode::Legacy {
    let mut workflow_config = match detail.workflow_config {
      Some(ref config) => config.clone(),
      None => return Ok(ApiResponse::error("未找到工作流配置")),
    };
    apply_reprocess_metadata(&mut workflow_config, mode, request.merged_video_id);
    let integrate_current_bvid = request.integrate_current_bvid;
    if integrate_current_bvid {
      let has_bvid = detail
        .task
        .bvid
        .as_deref()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
      if !has_bvid {
        return Ok(ApiResponse::error("当前任务没有BV号，无法集成投稿"));
      }
    }
    if let Err(err) = update_baidu_sync_config(
      &context,
      &task_id,
      request.baidu_sync_enabled,
      normalize_optional_text(request.baidu_sync_path),
      normalize_optional_text(request.baidu_sync_filename),
    ) {
      return Ok(ApiResponse::error(format!("更新百度同步配置失败: {}", err)));
    }
    append_log(
      &state.app_log_path,
      &format!(
        "submission_repost_start task_id={} mode={} type={}",
        task_id,
        if mode == ReprocessMode::MergeAll { "MERGE_ALL" } else { "SPECIFIED" },
        if integrate_current_bvid { "UPDATE" } else { "NEW" }
      ),
    );
    let mut specified_merged = None;
    let mut merge_all_list = Vec::new();
    let workflow_settings = load_workflow_settings(&context, &task_id);
    let enable_segmentation = workflow_settings.enable_segmentation;
    let segment_seconds = workflow_settings.segment_duration_seconds.max(1);
    let should_segment = integrate_current_bvid || enable_segmentation;
    let reprocess_source_paths = resolve_reprocess_source_paths(
      &context,
      &detail,
      &workflow_config,
      mode,
      request.merged_video_id,
    );
    let sources_ready = !reprocess_source_paths.is_empty()
      && collect_missing_source_paths(&reprocess_source_paths).is_empty();
    let source_override = if mode == ReprocessMode::Specified {
      Some(reprocess_source_paths.clone())
    } else {
      None
    };
    let base_dir = resolve_submission_base_dir(&context, &task_id);
    let output_dir = base_dir
      .join("repost")
      .join(sanitize_filename(&format!("repost_{}", now_rfc3339())))
      .join("output");
    if mode == ReprocessMode::Specified {
      let mut merged = match resolve_target_merged_video(
        &context,
        &task_id,
        request.merged_video_id,
        &state.app_log_path,
      ) {
        Ok(merged) => merged,
        Err(_) => {
          return handle_repost_missing_assets(
            &state,
            &context,
            &detail,
            &task_id,
            &workflow_config,
            integrate_current_bvid,
            "合并视频缺失",
            source_override.clone(),
          )
          .await;
        }
      };
      let mut merged_path = merged.video_path.clone().unwrap_or_default();
      if merged_path.trim().is_empty() || !PathBuf::from(&merged_path).exists() {
        if sources_ready {
          return handle_repost_missing_assets(
            &state,
            &context,
            &detail,
            &task_id,
            &workflow_config,
            integrate_current_bvid,
            "合并视频缺失",
            source_override.clone(),
          )
          .await;
        }
        match try_restore_merged_from_baidu(&context, &state.app_log_path, &merged, &base_dir).await? {
          BaiduRestoreResult::Ready(restored) => {
            merged_path = restored.to_string_lossy().to_string();
            merged.video_path = Some(merged_path.clone());
          }
          BaiduRestoreResult::Queued => {
            if let Err(err) = prepare_workflow_for_baidu_restore(
              &context,
              &state.app_log_path,
              &task_id,
              &workflow_config,
              if integrate_current_bvid { "VIDEO_UPDATE" } else { "VIDEO_SUBMISSION" },
              !integrate_current_bvid,
            ) {
              return Ok(ApiResponse::error(format!("准备网盘恢复失败: {}", err)));
            }
            return Ok(ApiResponse::success(
              "合并视频缺失，已创建网盘下载任务，下载完成后自动重新投稿".to_string(),
            ));
          }
          BaiduRestoreResult::NotBound => {}
        }
      }
      if merged_path.trim().is_empty() {
        return handle_repost_missing_assets(
          &state,
          &context,
          &detail,
          &task_id,
          &workflow_config,
          integrate_current_bvid,
          "合并视频缺失",
          source_override.clone(),
        )
        .await;
      }
      let merged_path_buf = PathBuf::from(merged_path.clone());
      if !merged_path_buf.exists() {
        return handle_repost_missing_assets(
          &state,
          &context,
          &detail,
          &task_id,
          &workflow_config,
          integrate_current_bvid,
          "合并视频缺失",
          source_override.clone(),
        )
        .await;
      }
      specified_merged = Some(merged);
    } else {
      let mut merged_videos = match load_merged_videos_by_task(&context, &task_id) {
        Ok(list) => list,
        Err(err) => return Ok(ApiResponse::error(err)),
      };
      if merged_videos.is_empty() {
        return handle_repost_missing_assets(
          &state,
          &context,
          &detail,
          &task_id,
          &workflow_config,
          integrate_current_bvid,
          "合并视频缺失",
          None,
        )
        .await;
      }
      for merged in &mut merged_videos {
        let mut path = merged.video_path.clone().unwrap_or_default();
        if path.trim().is_empty() || !PathBuf::from(&path).exists() {
          if sources_ready {
            return handle_repost_missing_assets(
              &state,
              &context,
              &detail,
              &task_id,
              &workflow_config,
              integrate_current_bvid,
              "合并视频缺失",
              None,
            )
            .await;
          }
          match try_restore_merged_from_baidu(&context, &state.app_log_path, merged, &base_dir).await? {
            BaiduRestoreResult::Ready(restored) => {
              path = restored.to_string_lossy().to_string();
              merged.video_path = Some(path.clone());
            }
            BaiduRestoreResult::Queued => {
              if let Err(err) = prepare_workflow_for_baidu_restore(
                &context,
                &state.app_log_path,
                &task_id,
                &workflow_config,
                if integrate_current_bvid { "VIDEO_UPDATE" } else { "VIDEO_SUBMISSION" },
                !integrate_current_bvid,
              ) {
                return Ok(ApiResponse::error(format!("准备网盘恢复失败: {}", err)));
              }
              return Ok(ApiResponse::success(
                "合并视频缺失，已创建网盘下载任务，下载完成后自动重新投稿".to_string(),
              ));
            }
            BaiduRestoreResult::NotBound => {}
          }
        }
        if path.trim().is_empty() {
          return handle_repost_missing_assets(
            &state,
            &context,
            &detail,
            &task_id,
            &workflow_config,
            integrate_current_bvid,
            "合并视频缺失",
            None,
          )
          .await;
        }
        let path_buf = PathBuf::from(&path);
        if !path_buf.exists() {
          return handle_repost_missing_assets(
            &state,
            &context,
            &detail,
            &task_id,
            &workflow_config,
            integrate_current_bvid,
            "合并视频缺失",
            None,
          )
          .await;
        }
      }
      merge_all_list = merged_videos;
    }

    if let Err(err) = clear_edit_upload_segments_by_task(&context, &task_id) {
      append_log(
        &state.app_log_path,
        &format!(
          "submission_repost_clear_cache_fail task_id={} err={}",
          task_id, err
        ),
      );
    }
    if let Err(err) = reset_workflow_instances(&context, &task_id) {
      return Ok(ApiResponse::error(format!("重置工作流失败: {}", err)));
    }
    let workflow_type = if integrate_current_bvid { "VIDEO_UPDATE" } else { "VIDEO_SUBMISSION" };
    let (instance_id, _) =
      match create_workflow_instance_for_task_with_type(
        context.db.as_ref(),
        &task_id,
        &workflow_config,
        workflow_type,
      ) {
        Ok(value) => value,
        Err(err) => return Ok(ApiResponse::error(format!("创建工作流失败: {}", err))),
      };
    let now = now_rfc3339();
    let update_result = context.db.with_conn(|conn| {
      if integrate_current_bvid {
        conn.execute(
          "UPDATE submission_task SET status = 'SEGMENTING', remote_state = NULL, reject_reason = NULL, updated_at = ?1 WHERE task_id = ?2",
          (&now, &task_id),
        )?;
      } else {
        conn.execute(
          "UPDATE submission_task SET status = 'SEGMENTING', bvid = NULL, aid = NULL, remote_state = NULL, reject_reason = NULL, updated_at = ?1 WHERE task_id = ?2",
          (&now, &task_id),
        )?;
      }
      Ok(())
    });
    if let Err(err) = update_result {
      return Ok(ApiResponse::error(format!("重置任务状态失败: {}", err)));
    }
    let _ = context.db.with_conn(|conn| {
      conn.execute(
        "UPDATE task_relations SET workflow_instance_id = ?1, updated_at = ?2 WHERE submission_task_id = ?3 AND relation_type = 'INTEGRATED'",
        (&instance_id, &now, &task_id),
      )?;
      Ok(())
    });

    if mode == ReprocessMode::Specified {
      let merged = match specified_merged {
        Some(value) => value,
        None => return Ok(ApiResponse::error("未找到合并视频")),
      };
      let merged_path = merged.video_path.clone().unwrap_or_default();
      let merged_path_buf = PathBuf::from(merged_path.clone());
      let segment_prefix = detail.task.segment_prefix.clone();
      let context_clone = context.clone();
      let task_id_clone = task_id.clone();
      let merged_path_clone = merged_path_buf.clone();
      let output_dir_clone = output_dir.clone();
      let app_log_path = state.app_log_path.clone();
      let merged_id = merged.id;
      tauri::async_runtime::spawn(async move {
        let _ = update_workflow_status(
          &context_clone,
          &task_id_clone,
          "RUNNING",
          Some("SEGMENTING"),
          70.0,
        );
        let segment_outputs = if should_segment {
          if enable_segmentation {
            let merged_path_for_segment = merged_path_clone.clone();
            match tauri::async_runtime::spawn_blocking(move || {
              segment_file(&merged_path_for_segment, &output_dir_clone, segment_seconds)
            })
            .await
            {
              Ok(result) => result,
              Err(_) => Err("Failed to segment video".to_string()),
            }
          } else {
            Ok(vec![merged_path_clone.clone()])
          }
        } else {
          Ok(Vec::new())
        };
        match segment_outputs {
          Ok(outputs) => {
            if !should_segment {
              let latest_id = load_latest_merged_video(&context_clone, &task_id_clone)
                .ok()
                .flatten()
                .map(|value| value.id);
              if latest_id != Some(merged_id) {
                if let Err(err) = context_clone.db.with_conn(|conn| {
                  conn.execute("DELETE FROM merged_video WHERE task_id = ?1", [&task_id_clone])?;
                  Ok(())
                }) {
                  let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
                  let _ = update_workflow_status(
                    &context_clone,
                    &task_id_clone,
                    "FAILED",
                    Some("SEGMENTING"),
                    0.0,
                  );
                  append_log(
                    app_log_path.as_ref(),
                    &format!(
                      "submission_repost_cleanup_fail task_id={} err={}",
                      task_id_clone, err
                    ),
                  );
                  return;
                }
                let new_merged_id = match save_merged_video(
                  &context_clone,
                  &task_id_clone,
                  merged_path_clone.as_path(),
                ) {
                  Ok(merged_id) => merged_id,
                  Err(err) => {
                  let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
                  let _ = update_workflow_status(
                    &context_clone,
                    &task_id_clone,
                    "FAILED",
                    Some("SEGMENTING"),
                    0.0,
                  );
                  append_log(
                    app_log_path.as_ref(),
                    &format!(
                      "submission_repost_save_fail task_id={} err={}",
                      task_id_clone, err
                    ),
                  );
                  return;
                  }
                };
                if let Err(err) = copy_merged_source_bindings(
                  &context_clone,
                  &task_id_clone,
                  merged_id,
                  new_merged_id,
                ) {
                  append_log(
                    app_log_path.as_ref(),
                    &format!(
                      "submission_repost_bind_sources_fail task_id={} merged_id={} err={}",
                      task_id_clone, new_merged_id, err
                    ),
                  );
                }
              }
              let _ = update_submission_status(&context_clone, &task_id_clone, "WAITING_UPLOAD");
              let _ =
                update_workflow_status(&context_clone, &task_id_clone, "COMPLETED", None, 100.0);
              append_log(
                app_log_path.as_ref(),
                &format!("submission_repost_ok task_id={}", task_id_clone),
              );
              return;
            }
            if should_segment {
              if outputs.is_empty() {
                let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
                let _ = update_workflow_status(
                  &context_clone,
                  &task_id_clone,
                  "FAILED",
                  Some("SEGMENTING"),
                  0.0,
                );
                append_log(
                  app_log_path.as_ref(),
                  &format!(
                    "submission_repost_empty_outputs task_id={}",
                    task_id_clone
                  ),
                );
                return;
              }
              if let Err(err) = replace_segments_for_merged(
                &context_clone,
                &task_id_clone,
                merged_id,
                &outputs,
                segment_prefix.as_deref(),
              )
              {
                let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
                let _ = update_workflow_status(
                  &context_clone,
                  &task_id_clone,
                  "FAILED",
                  Some("SEGMENTING"),
                  0.0,
                );
                append_log(
                  app_log_path.as_ref(),
                  &format!(
                    "submission_repost_save_fail task_id={} err={}",
                    task_id_clone, err
                  ),
                );
                return;
              }
            }
            let _ = update_submission_status(&context_clone, &task_id_clone, "WAITING_UPLOAD");
            let _ =
              update_workflow_status(&context_clone, &task_id_clone, "COMPLETED", None, 100.0);
            append_log(
              app_log_path.as_ref(),
              &format!("submission_repost_ok task_id={}", task_id_clone),
            );
          }
          Err(err) => {
            let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
            let _ = update_workflow_status(
              &context_clone,
              &task_id_clone,
              "FAILED",
              Some("SEGMENTING"),
              0.0,
            );
            append_log(
              app_log_path.as_ref(),
              &format!(
                "submission_repost_segment_fail task_id={} err={}",
                task_id_clone, err
              ),
            );
          }
        }
      });
      return Ok(ApiResponse::success("重新投稿已启动".to_string()));
    }
    let merge_all_sources = collect_sources_for_merge_all(&context, &task_id, &merge_all_list);
    let mut merge_inputs = Vec::with_capacity(merge_all_list.len());
    for merged in &merge_all_list {
      let path = merged.video_path.clone().unwrap_or_default();
      if path.trim().is_empty() {
        return Ok(ApiResponse::error("合并视频路径为空"));
      }
      let path_buf = PathBuf::from(path);
      if !path_buf.exists() {
        return Ok(ApiResponse::error("合并视频文件不存在"));
      }
      merge_inputs.push(path_buf);
    }
    let merge_output = build_merge_output_path(&base_dir, &task_id);
    let context_clone = context.clone();
    let task_id_clone = task_id.clone();
  let output_dir_clone = output_dir.clone();
  let merge_output_for_merge = merge_output.clone();
  let merge_output_for_segment = merge_output.clone();
  let merge_output_for_save = merge_output.clone();
  let segment_prefix = detail.task.segment_prefix.clone();
  let app_log_path = state.app_log_path.clone();
  tauri::async_runtime::spawn(async move {
      let _ = update_workflow_status(
        &context_clone,
        &task_id_clone,
        "RUNNING",
        Some("SEGMENTING"),
        70.0,
      );
      let merge_inputs_clone = merge_inputs.clone();
      let merge_result = tauri::async_runtime::spawn_blocking(move || {
        merge_files(&merge_inputs_clone, &merge_output_for_merge)
      })
      .await;
      if let Err(err) = merge_result {
        let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
        let _ = update_workflow_status(
          &context_clone,
          &task_id_clone,
          "FAILED",
          Some("SEGMENTING"),
          0.0,
        );
        append_log(
          app_log_path.as_ref(),
          &format!("submission_repost_merge_fail task_id={} err={}", task_id_clone, err),
        );
        return;
      }
      let segment_outputs = if should_segment {
        if enable_segmentation {
          let merge_output_for_segment_clone = merge_output_for_segment.clone();
          match tauri::async_runtime::spawn_blocking(move || {
            segment_file(&merge_output_for_segment_clone, &output_dir_clone, segment_seconds)
          })
          .await
          {
            Ok(result) => result,
            Err(_) => Err("Failed to segment video".to_string()),
          }
        } else {
          Ok(vec![merge_output_for_segment.clone()])
        }
      } else {
        Ok(Vec::new())
      };
      match segment_outputs {
        Ok(outputs) => {
          if should_segment && outputs.is_empty() {
            let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
            let _ = update_workflow_status(
              &context_clone,
              &task_id_clone,
              "FAILED",
              Some("SEGMENTING"),
              0.0,
            );
            append_log(
              app_log_path.as_ref(),
              &format!(
                "submission_repost_empty_outputs task_id={}",
                task_id_clone
              ),
            );
            return;
          }
          if let Err(err) = context_clone.db.with_conn(|conn| {
            conn.execute("DELETE FROM merged_video WHERE task_id = ?1", [&task_id_clone])?;
            conn.execute("DELETE FROM task_output_segment WHERE task_id = ?1", [&task_id_clone])?;
            Ok(())
          }) {
            let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
            let _ = update_workflow_status(
              &context_clone,
              &task_id_clone,
              "FAILED",
              Some("SEGMENTING"),
              0.0,
            );
            append_log(
              app_log_path.as_ref(),
              &format!(
                "submission_repost_cleanup_fail task_id={} err={}",
                task_id_clone, err
              ),
            );
            return;
          }
          let merged_id = match save_merged_video(
            &context_clone,
            &task_id_clone,
            &merge_output_for_save,
          ) {
            Ok(merged_id) => merged_id,
            Err(err) => {
              let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
              let _ = update_workflow_status(
                &context_clone,
                &task_id_clone,
                "FAILED",
                Some("SEGMENTING"),
                0.0,
              );
              append_log(
                app_log_path.as_ref(),
                &format!(
                  "submission_repost_save_fail task_id={} err={}",
                  task_id_clone, err
                ),
              );
                return;
              }
            };
          if let Err(err) = save_merged_source_bindings(
            &context_clone,
            &task_id_clone,
            merged_id,
            &merge_all_sources,
          ) {
            append_log(
              app_log_path.as_ref(),
              &format!(
                "submission_repost_bind_sources_fail task_id={} merged_id={} err={}",
                task_id_clone, merged_id, err
              ),
            );
          }
          if should_segment {
            if let Err(err) =
              save_output_segments(
                &context_clone,
                &task_id_clone,
                &outputs,
                Some(merged_id),
                segment_prefix.as_deref(),
              )
            {
              let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
              let _ = update_workflow_status(
                &context_clone,
                &task_id_clone,
                "FAILED",
                Some("SEGMENTING"),
                0.0,
              );
              append_log(
                app_log_path.as_ref(),
                &format!(
                  "submission_repost_save_fail task_id={} err={}",
                  task_id_clone, err
                ),
              );
              return;
            }
          }
          let _ = update_submission_status(&context_clone, &task_id_clone, "WAITING_UPLOAD");
          let _ =
            update_workflow_status(&context_clone, &task_id_clone, "COMPLETED", None, 100.0);
          append_log(
            app_log_path.as_ref(),
            &format!("submission_repost_ok task_id={}", task_id_clone),
          );
        }
        Err(err) => {
          let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
          let _ = update_workflow_status(
            &context_clone,
            &task_id_clone,
            "FAILED",
            Some("SEGMENTING"),
            0.0,
          );
          append_log(
            app_log_path.as_ref(),
            &format!(
              "submission_repost_segment_fail task_id={} err={}",
              task_id_clone, err
            ),
          );
        }
      }
    });
    return Ok(ApiResponse::success("重新投稿已启动".to_string()));
  }
  if detail.source_videos.is_empty() {
    return Ok(ApiResponse::error("请至少添加一个源视频"));
  }
  let workflow_config = match detail.workflow_config {
    Some(config) => config,
    None => return Ok(ApiResponse::error("未找到工作流配置")),
  };
  let integrate_current_bvid = request.integrate_current_bvid;
  if integrate_current_bvid {
    let has_bvid = detail
      .task
      .bvid
      .as_deref()
      .map(|value| !value.trim().is_empty())
      .unwrap_or(false);
    if !has_bvid {
      return Ok(ApiResponse::error("当前任务没有BV号，无法集成投稿"));
    }
  }
  if let Err(err) = update_baidu_sync_config(
    &context,
    &task_id,
    request.baidu_sync_enabled,
    normalize_optional_text(request.baidu_sync_path),
    normalize_optional_text(request.baidu_sync_filename),
  ) {
    return Ok(ApiResponse::error(format!("更新百度同步配置失败: {}", err)));
  }

  let missing_sources = collect_missing_source_files(&detail.source_videos);
  if !missing_sources.is_empty() {
    append_log(
      &state.app_log_path,
      &format!(
        "submission_repost_missing_sources task_id={} count={}",
        task_id,
        missing_sources.len()
      ),
    );
    for path in &missing_sources {
      append_log(
        &state.app_log_path,
        &format!("submission_repost_missing_source task_id={} path={}", task_id, path),
      );
    }
    let integrated_records = load_integrated_download_records(&context, &task_id)?;
    if integrated_records.is_empty() {
      return Ok(ApiResponse::error("源视频不存在，请先下载"));
    }
    let mut records_by_path: HashMap<String, IntegratedDownloadRecord> = HashMap::new();
    for record in integrated_records {
      if !record.local_path.trim().is_empty() {
        records_by_path.insert(record.local_path.clone(), record);
      }
    }
    let mut missing_records = Vec::new();
    let mut missing_without_download = Vec::new();
    for path in &missing_sources {
      if let Some(record) = records_by_path.get(path) {
        missing_records.push(record.clone());
      } else {
        missing_without_download.push(path.clone());
      }
    }
    if !missing_without_download.is_empty() {
      append_log(
        &state.app_log_path,
        &format!(
          "submission_repost_missing_unbound task_id={} count={}",
          task_id,
          missing_without_download.len()
        ),
      );
      return Ok(ApiResponse::error("源视频不存在，请先下载"));
    }
    let workflow_instance_id = reset_submission_for_repost(
      &context,
      &state.app_log_path,
      &task_id,
      &workflow_config,
      if integrate_current_bvid { "VIDEO_UPDATE" } else { "VIDEO_SUBMISSION" },
      !integrate_current_bvid,
      false,
    )?;
    let new_download_ids =
      create_retry_download_records(&context, &task_id, &workflow_instance_id, &missing_records)?;
    crate::commands::download::requeue_integrated_downloads(&state, &new_download_ids).await?;
    return Ok(ApiResponse::success(
      "源视频缺失，已创建下载任务，下载完成后自动重新投稿".to_string(),
    ));
  }

  let _ = reset_submission_for_repost(
    &context,
    &state.app_log_path,
    &task_id,
    &workflow_config,
    if integrate_current_bvid { "VIDEO_UPDATE" } else { "VIDEO_SUBMISSION" },
    !integrate_current_bvid,
    false,
  )?;
  start_submission_workflow(
    context.db.clone(),
    context.app_log_path.clone(),
    context.edit_upload_state.clone(),
    task_id,
  );
  Ok(ApiResponse::success("重新投稿已启动".to_string()))
}

fn collect_missing_source_files(sources: &[TaskSourceVideoRecord]) -> Vec<String> {
  let mut missing = Vec::new();
  for source in sources {
    if source.source_file_path.trim().is_empty() {
      continue;
    }
    let path = Path::new(&source.source_file_path);
    if !path.exists() {
      missing.push(source.source_file_path.clone());
    }
  }
  missing
}

fn collect_missing_source_paths(paths: &[String]) -> Vec<String> {
  let mut missing = Vec::new();
  for path in paths {
    if path.trim().is_empty() {
      continue;
    }
    let path_buf = Path::new(path);
    if !path_buf.exists() {
      missing.push(path.clone());
    }
  }
  missing
}

fn extract_source_paths_from_task(detail: &SubmissionTaskDetail) -> Vec<String> {
  detail
    .source_videos
    .iter()
    .map(|source| source.source_file_path.trim().to_string())
    .filter(|value| !value.is_empty())
    .collect()
}

fn extract_update_source_paths_from_config(config: &Value) -> Vec<String> {
  let Some(list) = config.get("updateSources").and_then(|value| value.as_array()) else {
    return Vec::new();
  };
  let mut sources = Vec::new();
  for (index, item) in list.iter().enumerate() {
    let input_path = item
      .get("sourceFilePath")
      .or_else(|| item.get("source_file_path"))
      .and_then(|value| value.as_str())
      .unwrap_or("")
      .trim()
      .to_string();
    if input_path.is_empty() {
      continue;
    }
    let order = item
      .get("sortOrder")
      .or_else(|| item.get("sort_order"))
      .and_then(|value| value.as_i64())
      .unwrap_or((index + 1) as i64);
    sources.push((order, input_path));
  }
  sources.sort_by_key(|item| item.0);
  sources.into_iter().map(|item| item.1).collect()
}

fn extract_update_sources_from_config(config: &Value) -> Vec<ClipSource> {
  let Some(list) = config.get("updateSources").and_then(|value| value.as_array()) else {
    return Vec::new();
  };
  let mut sources = Vec::new();
  for (index, item) in list.iter().enumerate() {
    let input_path = item
      .get("sourceFilePath")
      .or_else(|| item.get("source_file_path"))
      .and_then(|value| value.as_str())
      .unwrap_or("")
      .trim()
      .to_string();
    if input_path.is_empty() {
      continue;
    }
    let start_time = item
      .get("startTime")
      .or_else(|| item.get("start_time"))
      .and_then(|value| value.as_str())
      .map(|value| value.trim().to_string())
      .filter(|value| !value.is_empty());
    let end_time = item
      .get("endTime")
      .or_else(|| item.get("end_time"))
      .and_then(|value| value.as_str())
      .map(|value| value.trim().to_string())
      .filter(|value| !value.is_empty());
    let order = item
      .get("sortOrder")
      .or_else(|| item.get("sort_order"))
      .and_then(|value| value.as_i64())
      .unwrap_or((index + 1) as i64);
    sources.push(ClipSource {
      input_path,
      start_time,
      end_time,
      order,
    });
  }
  sources.sort_by_key(|item| item.order);
  sources
}

fn task_sources_to_clip_sources(sources: &[TaskSourceVideoRecord]) -> Vec<ClipSource> {
  sources
    .iter()
    .map(|source| ClipSource {
      input_path: source.source_file_path.trim().to_string(),
      start_time: source.start_time.clone(),
      end_time: source.end_time.clone(),
      order: source.sort_order,
    })
    .filter(|source| !source.input_path.is_empty())
    .collect()
}

fn resolve_reprocess_sources(
  context: &SubmissionContext,
  detail: &SubmissionTaskDetail,
  config: &Value,
  mode: ReprocessMode,
  merged_id: Option<i64>,
) -> Vec<ClipSource> {
  if mode == ReprocessMode::Specified {
    if let Some(merged_id) = merged_id {
      if let Ok(sources) = load_merged_source_clips(context, &detail.task.task_id, merged_id) {
        if !sources.is_empty() {
          return sources;
        }
      }
    }
    let update_sources = extract_update_sources_from_config(config);
    if !update_sources.is_empty() {
      return update_sources;
    }
  }
  let fallback = task_sources_to_clip_sources(&detail.source_videos);
  normalize_binding_sources(fallback)
}

fn resolve_reprocess_source_paths(
  context: &SubmissionContext,
  detail: &SubmissionTaskDetail,
  config: &Value,
  mode: ReprocessMode,
  merged_id: Option<i64>,
) -> Vec<String> {
  if mode == ReprocessMode::Specified {
    if let Some(merged_id) = merged_id {
      if let Ok(paths) = load_merged_source_paths(context, &detail.task.task_id, merged_id) {
        if !paths.is_empty() {
          return paths;
        }
      }
    }
    let update_sources = extract_update_source_paths_from_config(config);
    if !update_sources.is_empty() {
      return update_sources;
    }
  }
  extract_source_paths_from_task(detail)
}

async fn handle_repost_missing_assets(
  state: &State<'_, AppState>,
  context: &SubmissionContext,
  detail: &SubmissionTaskDetail,
  task_id: &str,
  workflow_config: &Value,
  integrate_current_bvid: bool,
  reason: &str,
  source_paths_override: Option<Vec<String>>,
) -> Result<ApiResponse<String>, String> {
  append_log(
    &state.app_log_path,
    &format!(
      "submission_repost_fallback task_id={} reason={}",
      task_id, reason
    ),
  );
  let use_override = source_paths_override.as_ref().map(|list| !list.is_empty()).unwrap_or(false);
  let workflow_config = if use_override {
    workflow_config.clone()
  } else {
    strip_update_sources(workflow_config)
  };
  let source_paths = source_paths_override.unwrap_or_else(|| extract_source_paths_from_task(detail));
  let missing_sources = collect_missing_source_paths(&source_paths);
  if !missing_sources.is_empty() {
    append_log(
      &state.app_log_path,
      &format!(
        "submission_repost_missing_sources task_id={} count={}",
        task_id,
        missing_sources.len()
      ),
    );
    for path in &missing_sources {
      append_log(
        &state.app_log_path,
        &format!("submission_repost_missing_source task_id={} path={}", task_id, path),
      );
    }
    let integrated_records = load_integrated_download_records(context, task_id)?;
    if integrated_records.is_empty() {
      return Ok(ApiResponse::error("源视频不存在，请先下载"));
    }
    let mut records_by_path: HashMap<String, IntegratedDownloadRecord> = HashMap::new();
    for record in integrated_records {
      if !record.local_path.trim().is_empty() {
        records_by_path.insert(record.local_path.clone(), record);
      }
    }
    let mut missing_records = Vec::new();
    let mut missing_without_download = Vec::new();
    for path in &missing_sources {
      if let Some(record) = records_by_path.get(path) {
        missing_records.push(record.clone());
      } else {
        missing_without_download.push(path.clone());
      }
    }
    if !missing_without_download.is_empty() {
      append_log(
        &state.app_log_path,
        &format!(
          "submission_repost_missing_unbound task_id={} count={}",
          task_id,
          missing_without_download.len()
        ),
      );
      return Ok(ApiResponse::error("源视频不存在，请先下载"));
    }
    let workflow_instance_id = reset_submission_for_repost(
      context,
      &state.app_log_path,
      task_id,
      &workflow_config,
      if integrate_current_bvid { "VIDEO_UPDATE" } else { "VIDEO_SUBMISSION" },
      !integrate_current_bvid,
      true,
    )?;
    let new_download_ids = create_retry_download_records(
      context,
      task_id,
      &workflow_instance_id,
      &missing_records,
    )?;
    crate::commands::download::requeue_integrated_downloads(state, &new_download_ids).await?;
    return Ok(ApiResponse::success(format!(
      "{}，已创建下载任务，下载完成后自动重新投稿",
      reason
    )));
  }
  let _ = reset_submission_for_repost(
    context,
    &state.app_log_path,
    task_id,
    &workflow_config,
    if integrate_current_bvid { "VIDEO_UPDATE" } else { "VIDEO_SUBMISSION" },
    !integrate_current_bvid,
    true,
  )?;
  start_submission_workflow(
    context.db.clone(),
    context.app_log_path.clone(),
    context.edit_upload_state.clone(),
    task_id.to_string(),
  );
  Ok(ApiResponse::success(format!(
    "{}，已自动重新剪辑并重新投稿",
    reason
  )))
}

fn strip_update_sources(config: &Value) -> Value {
  let mut next = config.clone();
  if let Value::Object(map) = &mut next {
    map.remove("updateSources");
  }
  next
}

enum BaiduRestoreResult {
  NotBound,
  Ready(PathBuf),
  Queued,
}

fn extract_baidu_binding(merged: &MergedVideoRecord) -> Option<(String, String)> {
  let dir = merged.remote_dir.as_deref().map(|value| value.trim()).unwrap_or("");
  let name = merged.remote_name.as_deref().map(|value| value.trim()).unwrap_or("");
  if dir.is_empty() || name.is_empty() {
    return None;
  }
  Some((dir.to_string(), name.to_string()))
}

fn resolve_baidu_restore_path(
  merged: &MergedVideoRecord,
  base_dir: &Path,
  remote_name: &str,
) -> PathBuf {
  if let Some(path) = merged
    .video_path
    .as_deref()
    .filter(|value| !value.trim().is_empty())
  {
    return PathBuf::from(path);
  }
  let file_name = merged
    .file_name
    .as_deref()
    .filter(|value| !value.trim().is_empty())
    .unwrap_or(remote_name);
  base_dir.join("merge").join(file_name)
}

fn update_merged_video_after_baidu_download(
  context: &SubmissionContext,
  merged_id: i64,
  local_path: &Path,
  remote_dir: &str,
  remote_name: &str,
) -> Result<(), String> {
  let now = now_rfc3339();
  let file_name = local_path
    .file_name()
    .and_then(|value| value.to_str())
    .filter(|value| !value.trim().is_empty())
    .map(|value| value.to_string())
    .unwrap_or_else(|| remote_name.trim().to_string());
  context
    .db
    .with_conn(|conn| {
      conn.execute(
        "UPDATE merged_video SET file_name = ?1, video_path = ?2, remote_dir = ?3, remote_name = ?4, update_time = ?5 WHERE id = ?6",
        (
          file_name,
          local_path.to_string_lossy().to_string(),
          remote_dir,
          remote_name,
          &now,
          merged_id,
        ),
      )?;
      Ok(())
    })
    .map_err(|err| err.to_string())
}

fn reset_baidu_download_record(
  context: &SubmissionContext,
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
    .map_err(|err| err.to_string())
}

fn ensure_remote_restore_relation(
  context: &SubmissionContext,
  task_id: &str,
  record_id: i64,
) -> Result<(), String> {
  let now = now_rfc3339();
  let exists = context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT 1 FROM task_relations WHERE submission_task_id = ?1 AND download_task_id = ?2 AND relation_type = 'REMOTE_RESTORE' LIMIT 1",
      )?;
      let mut rows = stmt.query((task_id, record_id))?;
      Ok(rows.next()?.is_some())
    })
    .map_err(|err| err.to_string())?;
  if exists {
    let _ = context.db.with_conn(|conn| {
      conn.execute(
        "UPDATE task_relations SET workflow_status = 'PENDING_DOWNLOAD', updated_at = ?1 WHERE submission_task_id = ?2 AND download_task_id = ?3 AND relation_type = 'REMOTE_RESTORE'",
        (&now, task_id, record_id),
      )?;
      Ok(())
    });
    return Ok(());
  }
  context
    .db
    .with_conn(|conn| {
      conn.execute(
        "INSERT INTO task_relations (download_task_id, submission_task_id, relation_type, status, created_at, updated_at, workflow_status, retry_count) \
         VALUES (?1, ?2, 'REMOTE_RESTORE', 'ACTIVE', ?3, ?4, 'PENDING_DOWNLOAD', 0)",
        (record_id, task_id, &now, &now),
      )?;
      Ok(())
    })
    .map_err(|err| err.to_string())
}

async fn try_restore_merged_from_baidu(
  context: &SubmissionContext,
  app_log_path: &PathBuf,
  merged: &MergedVideoRecord,
  base_dir: &Path,
) -> Result<BaiduRestoreResult, String> {
  let Some((remote_dir, remote_name)) = extract_baidu_binding(merged) else {
    return Ok(BaiduRestoreResult::NotBound);
  };
  let remote_path = baidu_sync::join_baidu_path(&remote_dir, &remote_name);
  append_log(
    app_log_path,
    &format!(
      "submission_baidu_restore_check merged_id={} remote={}",
      merged.id, remote_path
    ),
  );
  let exists = match baidu_sync::check_baidu_remote_file_exists(context.db.as_ref(), &remote_path) {
    Ok(value) => value,
    Err(err) => {
      append_log(
        app_log_path,
        &format!(
          "submission_baidu_restore_check_fail merged_id={} err={}",
          merged.id, err
        ),
      );
      return Ok(BaiduRestoreResult::NotBound);
    }
  };
  if !exists {
    append_log(
      app_log_path,
      &format!(
        "submission_baidu_restore_missing merged_id={} remote={}",
        merged.id, remote_path
      ),
    );
    return Ok(BaiduRestoreResult::NotBound);
  }
  let expected_path = resolve_baidu_restore_path(merged, base_dir, remote_name.trim());
  let expected_path_str = expected_path.to_string_lossy().to_string();
  let record = context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT vd.id, vd.local_path, vd.status, tr.id \
         FROM video_download vd \
         LEFT JOIN task_relations tr \
           ON tr.download_task_id = vd.id \
          AND tr.submission_task_id = ?1 \
          AND tr.relation_type = 'REMOTE_RESTORE' \
         WHERE vd.download_url = ?2 AND vd.source_type = ?3 \
         ORDER BY vd.id DESC LIMIT 1",
      )?;
      let row = stmt
        .query_row(
          (&merged.task_id, &remote_path, crate::commands::download::DOWNLOAD_SOURCE_BAIDU),
          |row| {
            Ok((
              row.get::<_, i64>(0)?,
              row.get::<_, Option<String>>(1)?,
              row.get::<_, i64>(2)?,
              row.get::<_, Option<i64>>(3)?,
            ))
          },
        )
        .optional()?;
      Ok(row)
    })
    .map_err(|err| err.to_string())?;
  if let Some((record_id, local_path, status, relation_id)) = record {
    let mut local_path_value = local_path.unwrap_or_default();
    if local_path_value.trim().is_empty() {
      local_path_value = expected_path_str.clone();
      let now = now_rfc3339();
      let _ = context.db.with_conn(|conn| {
        conn.execute(
          "UPDATE video_download SET local_path = ?1, update_time = ?2 WHERE id = ?3",
          (&local_path_value, &now, record_id),
        )?;
        Ok(())
      });
    }
    if status == 2 && !local_path_value.trim().is_empty() {
      let local_path_buf = PathBuf::from(&local_path_value);
      if local_path_buf.exists() {
        update_merged_video_after_baidu_download(
          context,
          merged.id,
          &local_path_buf,
          &remote_dir,
          &remote_name,
        )?;
        append_log(
          app_log_path,
          &format!(
            "submission_baidu_restore_cached merged_id={} local={}",
            merged.id, local_path_value
          ),
        );
        return Ok(BaiduRestoreResult::Ready(local_path_buf));
      }
    }
    if status == 3 {
      let _ = reset_baidu_download_record(context, record_id);
    }
    if relation_id.is_none() {
      let _ = ensure_remote_restore_relation(context, &merged.task_id, record_id);
    }
    return Ok(BaiduRestoreResult::Queued);
  }

  let now = now_rfc3339();
  let title = if !remote_name.trim().is_empty() {
    remote_name.trim()
  } else {
    merged
      .file_name
      .as_deref()
      .filter(|value| !value.trim().is_empty())
      .unwrap_or("网盘文件")
  };
  let record_id = context
    .db
    .with_conn(|conn| {
      conn.execute(
        "INSERT INTO video_download (bvid, aid, title, part_title, part_count, current_part, download_url, local_path, status, progress, progress_total, progress_done, create_time, update_time, resolution, codec, format, cid, content, source_type) \
         VALUES (NULL, NULL, ?1, ?2, 1, 1, ?3, ?4, 0, 0, 0, 0, ?5, ?6, NULL, NULL, NULL, NULL, NULL, ?7)",
        (
          title,
          remote_name.trim(),
          remote_path.as_str(),
          expected_path_str.as_str(),
          &now,
          &now,
          crate::commands::download::DOWNLOAD_SOURCE_BAIDU,
        ),
      )?;
      Ok(conn.last_insert_rowid())
    })
    .map_err(|err| err.to_string())?;
  ensure_remote_restore_relation(context, &merged.task_id, record_id)?;
  Ok(BaiduRestoreResult::Queued)
}

fn prepare_workflow_for_baidu_restore(
  context: &SubmissionContext,
  app_log_path: &PathBuf,
  task_id: &str,
  workflow_config: &Value,
  workflow_type: &str,
  clear_bvid: bool,
) -> Result<(), String> {
  if let Err(err) = clear_edit_upload_segments_by_task(context, task_id) {
    append_log(
      app_log_path,
      &format!(
        "submission_baidu_restore_clear_cache_fail task_id={} err={}",
        task_id, err
      ),
    );
  }
  reset_workflow_instances(context, task_id)
    .map_err(|err| format!("重置工作流失败: {}", err))?;
  let (instance_id, _) = create_workflow_instance_for_task_with_type(
    context.db.as_ref(),
    task_id,
    workflow_config,
    workflow_type,
  )
  .map_err(|err| format!("创建工作流失败: {}", err))?;
  let now = now_rfc3339();
  context
    .db
    .with_conn(|conn| {
      if clear_bvid {
        conn.execute(
          "UPDATE submission_task SET status = 'PENDING', bvid = NULL, aid = NULL, remote_state = NULL, reject_reason = NULL, updated_at = ?1 WHERE task_id = ?2",
          (&now, task_id),
        )?;
      } else {
        conn.execute(
          "UPDATE submission_task SET status = 'PENDING', remote_state = NULL, reject_reason = NULL, updated_at = ?1 WHERE task_id = ?2",
          (&now, task_id),
        )?;
      }
      conn.execute(
        "UPDATE task_relations SET workflow_instance_id = ?1, updated_at = ?2 WHERE submission_task_id = ?3 AND relation_type = 'INTEGRATED'",
        (&instance_id, &now, task_id),
      )?;
      Ok(())
    })
    .map_err(|err| err.to_string())?;
  let _ = update_workflow_status(context, task_id, "VIDEO_DOWNLOADING", None, 0.0);
  Ok(())
}

fn split_baidu_path(remote_path: &str) -> Option<(String, String)> {
  let trimmed = remote_path.trim();
  if trimmed.is_empty() {
    return None;
  }
  let mut parts = trimmed.rsplitn(2, '/');
  let name = parts.next()?.trim().to_string();
  if name.is_empty() {
    return None;
  }
  let dir = parts.next().unwrap_or("").trim().to_string();
  let dir = if dir.is_empty() { "/".to_string() } else { dir };
  Some((dir, name))
}

fn update_merged_video_by_remote_binding(
  context: &SubmissionContext,
  task_id: &str,
  remote_dir: &str,
  remote_name: &str,
  local_path: &str,
) -> Result<bool, String> {
  let now = now_rfc3339();
  let updated = context
    .db
    .with_conn(|conn| {
      conn.execute(
        "UPDATE merged_video SET file_name = ?1, video_path = ?2, update_time = ?3 \
         WHERE task_id = ?4 AND remote_dir = ?5 AND remote_name = ?6",
        (remote_name, local_path, &now, task_id, remote_dir, remote_name),
      )
    })
    .map_err(|err| err.to_string())?;
  Ok(updated > 0)
}

fn load_remote_restore_stats(
  context: &SubmissionContext,
  task_id: &str,
) -> Result<(i64, i64, i64), String> {
  context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT \
          COUNT(*) AS total, \
          SUM(CASE WHEN vd.status = 2 THEN 1 ELSE 0 END) AS completed, \
          SUM(CASE WHEN vd.status = 3 THEN 1 ELSE 0 END) AS failed \
         FROM task_relations tr \
         JOIN video_download vd ON tr.download_task_id = vd.id \
         WHERE tr.submission_task_id = ?1 AND tr.relation_type = 'REMOTE_RESTORE'",
      )?;
      let row = stmt.query_row([task_id], |row| {
        let total: i64 = row.get(0)?;
        let completed: Option<i64> = row.get(1)?;
        let failed: Option<i64> = row.get(2)?;
        Ok((total, completed.unwrap_or(0), failed.unwrap_or(0)))
      })?;
      Ok(row)
    })
    .map_err(|err| err.to_string())
}

fn update_remote_restore_status(
  context: &SubmissionContext,
  task_id: &str,
  workflow_status: &str,
) -> Result<(), String> {
  let now = now_rfc3339();
  context
    .db
    .with_conn(|conn| {
      conn.execute(
        "UPDATE task_relations SET workflow_status = ?1, updated_at = ?2 \
         WHERE submission_task_id = ?3 AND relation_type = 'REMOTE_RESTORE'",
        (workflow_status, &now, task_id),
      )?;
      Ok(())
    })
    .map_err(|err| err.to_string())
}

async fn resume_resegment_after_restore(
  context: &SubmissionContext,
  app_log_path: &PathBuf,
  task_id: &str,
  mode: ReprocessMode,
  merged_video_id: Option<i64>,
) -> Result<(), String> {
  let workflow_settings = load_workflow_settings(context, task_id);
  let segment_seconds = workflow_settings.segment_duration_seconds.max(1);
  let segment_prefix = workflow_settings.segment_prefix.clone();
  let config = load_latest_workflow_config(context, task_id).ok().flatten();
  let integrate_current_bvid = load_integrate_current_bvid(config.as_ref());
  let base_dir = resolve_submission_base_dir(context, task_id);
  let output_dir = if mode == ReprocessMode::Legacy {
    base_dir.join("output")
  } else {
    base_dir
      .join("resegment")
      .join(sanitize_filename(&format!("resegment_{}", now_rfc3339())))
      .join("output")
  };

  let update_segmenting = |clear_segments: bool| -> Result<(), String> {
    let now = now_rfc3339();
    context
      .db
      .with_conn(|conn| {
        if clear_segments {
          conn.execute("DELETE FROM task_output_segment WHERE task_id = ?1", [task_id])?;
        }
        if integrate_current_bvid {
          conn.execute(
            "UPDATE submission_task SET status = 'SEGMENTING', remote_state = NULL, reject_reason = NULL, updated_at = ?1 WHERE task_id = ?2",
            (&now, task_id),
          )?;
        } else {
          conn.execute(
            "UPDATE submission_task SET status = 'SEGMENTING', bvid = NULL, aid = NULL, remote_state = NULL, reject_reason = NULL, updated_at = ?1 WHERE task_id = ?2",
            (&now, task_id),
          )?;
        }
        Ok(())
      })
      .map_err(|err| err.to_string())
  };

  if let Err(err) = clear_edit_upload_segments_by_task(context, task_id) {
    append_log(
      app_log_path,
      &format!(
        "submission_resegment_clear_cache_fail task_id={} err={}",
        task_id, err
      ),
    );
  }

  match mode {
    ReprocessMode::Specified => {
      let merged = resolve_target_merged_video(context, task_id, merged_video_id, app_log_path)?;
      let merged_path = merged.video_path.clone().unwrap_or_default();
      if merged_path.trim().is_empty() || !PathBuf::from(&merged_path).exists() {
        return Err("合并视频缺失".to_string());
      }
      update_segmenting(false)?;
      let merged_path_buf = PathBuf::from(merged_path);
      let context_clone = context.clone();
      let task_id_clone = task_id.to_string();
      let merged_path_clone = merged_path_buf.clone();
      let output_dir_clone = output_dir.clone();
      let app_log_path = app_log_path.to_path_buf();
      let merged_id = merged.id;
      tauri::async_runtime::spawn(async move {
        let _ = update_workflow_status(
          &context_clone,
          &task_id_clone,
          "RUNNING",
          Some("SEGMENTING"),
          70.0,
        );
        let segment_outputs = match tauri::async_runtime::spawn_blocking(move || {
          segment_file(&merged_path_clone, &output_dir_clone, segment_seconds)
        })
        .await
        {
          Ok(result) => result,
          Err(_) => Err("Failed to segment video".to_string()),
        };
        match segment_outputs {
          Ok(outputs) => {
            if outputs.is_empty() {
              let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
              let _ = update_workflow_status(
                &context_clone,
                &task_id_clone,
                "FAILED",
                Some("SEGMENTING"),
                0.0,
              );
              append_log(
                app_log_path.as_ref(),
                &format!("submission_resegment_empty_outputs task_id={}", task_id_clone),
              );
              return;
            }
            if let Err(err) = replace_segments_for_merged(
              &context_clone,
              &task_id_clone,
              merged_id,
              &outputs,
              segment_prefix.as_deref(),
            ) {
              let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
              let _ = update_workflow_status(
                &context_clone,
                &task_id_clone,
                "FAILED",
                Some("SEGMENTING"),
                0.0,
              );
              append_log(
                app_log_path.as_ref(),
                &format!(
                  "submission_resegment_save_fail task_id={} err={}",
                  task_id_clone, err
                ),
              );
              return;
            }
            if !integrate_current_bvid {
              if let Err(err) = reset_segments_for_new_bvid(&context_clone, &task_id_clone) {
                let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
                let _ = update_workflow_status(
                  &context_clone,
                  &task_id_clone,
                  "FAILED",
                  Some("SEGMENTING"),
                  0.0,
                );
                append_log(
                  app_log_path.as_ref(),
                  &format!(
                    "submission_resegment_reset_segments_fail task_id={} err={}",
                    task_id_clone, err
                  ),
                );
                return;
              }
            }
            let _ = update_submission_status(&context_clone, &task_id_clone, "WAITING_UPLOAD");
            let _ = update_workflow_status(&context_clone, &task_id_clone, "COMPLETED", None, 100.0);
            append_log(
              app_log_path.as_ref(),
              &format!("submission_resegment_ok task_id={}", task_id_clone),
            );
          }
          Err(err) => {
            let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
            let _ = update_workflow_status(
              &context_clone,
              &task_id_clone,
              "FAILED",
              Some("SEGMENTING"),
              0.0,
            );
            append_log(
              app_log_path.as_ref(),
              &format!(
                "submission_resegment_segment_fail task_id={} err={}",
                task_id_clone, err
              ),
            );
          }
        }
      });
    }
    ReprocessMode::MergeAll => {
      let merged_videos = load_merged_videos_by_task(context, task_id)?;
      if merged_videos.is_empty() {
        return Err("未找到合并视频".to_string());
      }
      let merge_all_sources = collect_sources_for_merge_all(context, task_id, &merged_videos);
      let mut merge_inputs = Vec::with_capacity(merged_videos.len());
      for merged in &merged_videos {
        let path = merged.video_path.clone().unwrap_or_default();
        if path.trim().is_empty() || !PathBuf::from(&path).exists() {
          return Err("合并视频缺失".to_string());
        }
        merge_inputs.push(PathBuf::from(path));
      }
      update_segmenting(false)?;
      let merge_output = build_merge_output_path(&base_dir, task_id);
      let context_clone = context.clone();
      let task_id_clone = task_id.to_string();
      let output_dir_clone = output_dir.clone();
      let merge_output_for_merge = merge_output.clone();
      let merge_output_for_segment = merge_output.clone();
      let merge_output_for_save = merge_output.clone();
      let app_log_path = app_log_path.to_path_buf();
      tauri::async_runtime::spawn(async move {
        let _ = update_workflow_status(
          &context_clone,
          &task_id_clone,
          "RUNNING",
          Some("SEGMENTING"),
          70.0,
        );
        let merge_inputs_clone = merge_inputs.clone();
        let merge_result = tauri::async_runtime::spawn_blocking(move || {
          merge_files(&merge_inputs_clone, &merge_output_for_merge)
        })
        .await;
        if let Err(err) = merge_result {
          let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
          let _ = update_workflow_status(
            &context_clone,
            &task_id_clone,
            "FAILED",
            Some("SEGMENTING"),
            0.0,
          );
          append_log(
            app_log_path.as_ref(),
            &format!(
              "submission_resegment_merge_fail task_id={} err={}",
              task_id_clone, err
            ),
          );
          return;
        }
        let merge_output_for_segment_clone = merge_output_for_segment.clone();
        let segment_outputs = match tauri::async_runtime::spawn_blocking(move || {
          segment_file(&merge_output_for_segment_clone, &output_dir_clone, segment_seconds)
        })
        .await
        {
          Ok(result) => result,
          Err(_) => Err("Failed to segment video".to_string()),
        };
        match segment_outputs {
          Ok(outputs) => {
            if outputs.is_empty() {
              let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
              let _ = update_workflow_status(
                &context_clone,
                &task_id_clone,
                "FAILED",
                Some("SEGMENTING"),
                0.0,
              );
              append_log(
                app_log_path.as_ref(),
                &format!(
                  "submission_resegment_empty_outputs task_id={}",
                  task_id_clone
                ),
              );
              return;
            }
            if let Err(err) = context_clone.db.with_conn(|conn| {
              conn.execute("DELETE FROM merged_video WHERE task_id = ?1", [&task_id_clone])?;
              conn.execute("DELETE FROM task_output_segment WHERE task_id = ?1", [&task_id_clone])?;
              Ok(())
            }) {
              let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
              let _ = update_workflow_status(
                &context_clone,
                &task_id_clone,
                "FAILED",
                Some("SEGMENTING"),
                0.0,
              );
              append_log(
                app_log_path.as_ref(),
                &format!(
                  "submission_resegment_cleanup_fail task_id={} err={}",
                  task_id_clone, err
                ),
              );
              return;
            }
            let merged_id = match save_merged_video(
              &context_clone,
              &task_id_clone,
              &merge_output_for_save,
            ) {
              Ok(merged_id) => merged_id,
              Err(err) => {
                let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
                let _ = update_workflow_status(
                  &context_clone,
                  &task_id_clone,
                  "FAILED",
                  Some("SEGMENTING"),
                  0.0,
                );
                append_log(
                  app_log_path.as_ref(),
                  &format!(
                    "submission_resegment_save_fail task_id={} err={}",
                    task_id_clone, err
                  ),
                );
                return;
              }
            };
            if let Err(err) = save_merged_source_bindings(
              &context_clone,
              &task_id_clone,
              merged_id,
              &merge_all_sources,
            ) {
              append_log(
                app_log_path.as_ref(),
                &format!(
                  "submission_resegment_bind_sources_fail task_id={} merged_id={} err={}",
                  task_id_clone, merged_id, err
                ),
              );
            }
            if let Err(err) =
              save_output_segments(
                &context_clone,
                &task_id_clone,
                &outputs,
                Some(merged_id),
                segment_prefix.as_deref(),
              )
            {
              let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
              let _ = update_workflow_status(
                &context_clone,
                &task_id_clone,
                "FAILED",
                Some("SEGMENTING"),
                0.0,
              );
              append_log(
                app_log_path.as_ref(),
                &format!(
                  "submission_resegment_save_fail task_id={} err={}",
                  task_id_clone, err
                ),
              );
              return;
            }
            if !integrate_current_bvid {
              if let Err(err) = reset_segments_for_new_bvid(&context_clone, &task_id_clone) {
                let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
                let _ = update_workflow_status(
                  &context_clone,
                  &task_id_clone,
                  "FAILED",
                  Some("SEGMENTING"),
                  0.0,
                );
                append_log(
                  app_log_path.as_ref(),
                  &format!(
                    "submission_resegment_reset_segments_fail task_id={} err={}",
                    task_id_clone, err
                  ),
                );
                return;
              }
            }
            let _ = update_submission_status(&context_clone, &task_id_clone, "WAITING_UPLOAD");
            let _ = update_workflow_status(&context_clone, &task_id_clone, "COMPLETED", None, 100.0);
            append_log(
              app_log_path.as_ref(),
              &format!("submission_resegment_ok task_id={}", task_id_clone),
            );
          }
          Err(err) => {
            let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
            let _ = update_workflow_status(
              &context_clone,
              &task_id_clone,
              "FAILED",
              Some("SEGMENTING"),
              0.0,
            );
            append_log(
              app_log_path.as_ref(),
              &format!(
                "submission_resegment_segment_fail task_id={} err={}",
                task_id_clone, err
              ),
            );
          }
        }
      });
    }
    ReprocessMode::Legacy | ReprocessMode::FullReprocess => {
      let merged = load_latest_merged_video(context, task_id)?
        .ok_or_else(|| "未找到合并视频".to_string())?;
      let merged_path = merged.video_path.clone().unwrap_or_default();
      if merged_path.trim().is_empty() || !PathBuf::from(&merged_path).exists() {
        return Err("合并视频缺失".to_string());
      }
      update_segmenting(true)?;
      if let Err(err) = remove_path_if_exists(app_log_path, "output", &output_dir) {
        append_log(
          app_log_path,
          &format!(
            "submission_resegment_cleanup_fail task_id={} err={}",
            task_id, err
          ),
        );
      }
      let merged_path_buf = PathBuf::from(merged_path);
      let merged_id = merged.id;
      let context_clone = context.clone();
      let task_id_clone = task_id.to_string();
      let merged_path_clone = merged_path_buf.clone();
      let output_dir_clone = output_dir.clone();
      let app_log_path = app_log_path.to_path_buf();
      tauri::async_runtime::spawn(async move {
        let _ = update_workflow_status(
          &context_clone,
          &task_id_clone,
          "RUNNING",
          Some("SEGMENTING"),
          70.0,
        );
        let segment_outputs = match tauri::async_runtime::spawn_blocking(move || {
          segment_file(&merged_path_clone, &output_dir_clone, segment_seconds)
        })
        .await
        {
          Ok(result) => result,
          Err(_) => Err("Failed to segment video".to_string()),
        };
        match segment_outputs {
          Ok(outputs) => {
            if outputs.is_empty() {
              let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
              let _ = update_workflow_status(
                &context_clone,
                &task_id_clone,
                "FAILED",
                Some("SEGMENTING"),
                0.0,
              );
              append_log(
                app_log_path.as_ref(),
                &format!(
                  "submission_resegment_empty_outputs task_id={}",
                  task_id_clone
                ),
              );
              return;
            }
            if let Err(err) =
              save_output_segments(
                &context_clone,
                &task_id_clone,
                &outputs,
                Some(merged_id),
                segment_prefix.as_deref(),
              )
            {
              let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
              let _ = update_workflow_status(
                &context_clone,
                &task_id_clone,
                "FAILED",
                Some("SEGMENTING"),
                0.0,
              );
              append_log(
                app_log_path.as_ref(),
                &format!(
                  "submission_resegment_save_fail task_id={} err={}",
                  task_id_clone, err
                ),
              );
              return;
            }
            if !integrate_current_bvid {
              if let Err(err) = reset_segments_for_new_bvid(&context_clone, &task_id_clone) {
                let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
                let _ = update_workflow_status(
                  &context_clone,
                  &task_id_clone,
                  "FAILED",
                  Some("SEGMENTING"),
                  0.0,
                );
                append_log(
                  app_log_path.as_ref(),
                  &format!(
                    "submission_resegment_reset_segments_fail task_id={} err={}",
                    task_id_clone, err
                  ),
                );
                return;
              }
            }
            let _ = update_submission_status(&context_clone, &task_id_clone, "WAITING_UPLOAD");
            let _ = update_workflow_status(&context_clone, &task_id_clone, "COMPLETED", None, 100.0);
            append_log(
              app_log_path.as_ref(),
              &format!("submission_resegment_ok task_id={}", task_id_clone),
            );
          }
          Err(err) => {
            let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
            let _ = update_workflow_status(
              &context_clone,
              &task_id_clone,
              "FAILED",
              Some("SEGMENTING"),
              0.0,
            );
            append_log(
              app_log_path.as_ref(),
              &format!(
                "submission_resegment_segment_fail task_id={} err={}",
                task_id_clone, err
              ),
            );
          }
        }
      });
    }
  }
  Ok(())
}

async fn resume_repost_after_restore(
  context: &SubmissionContext,
  app_log_path: &PathBuf,
  task_id: &str,
  mode: ReprocessMode,
  merged_video_id: Option<i64>,
  workflow_type: &str,
) -> Result<(), String> {
  let integrate_current_bvid = workflow_type == "VIDEO_UPDATE";
  let workflow_settings = load_workflow_settings(context, task_id);
  let enable_segmentation = workflow_settings.enable_segmentation;
  let segment_seconds = workflow_settings.segment_duration_seconds.max(1);
  let segment_prefix = workflow_settings.segment_prefix.clone();
  let should_segment = integrate_current_bvid || enable_segmentation;
  let base_dir = resolve_submission_base_dir(context, task_id);
  let output_dir = base_dir
    .join("repost")
    .join(sanitize_filename(&format!("repost_{}", now_rfc3339())))
    .join("output");

  let now = now_rfc3339();
  context
    .db
    .with_conn(|conn| {
      if integrate_current_bvid {
        conn.execute(
          "UPDATE submission_task SET status = 'SEGMENTING', remote_state = NULL, reject_reason = NULL, updated_at = ?1 WHERE task_id = ?2",
          (&now, task_id),
        )?;
      } else {
        conn.execute(
          "UPDATE submission_task SET status = 'SEGMENTING', bvid = NULL, aid = NULL, remote_state = NULL, reject_reason = NULL, updated_at = ?1 WHERE task_id = ?2",
          (&now, task_id),
        )?;
      }
      Ok(())
    })
    .map_err(|err| err.to_string())?;

  match mode {
    ReprocessMode::Specified => {
      let merged = resolve_target_merged_video(context, task_id, merged_video_id, app_log_path)?;
      let merged_path = merged.video_path.clone().unwrap_or_default();
      if merged_path.trim().is_empty() || !PathBuf::from(&merged_path).exists() {
        return Err("合并视频缺失".to_string());
      }
      let merged_path_buf = PathBuf::from(merged_path);
      let context_clone = context.clone();
      let task_id_clone = task_id.to_string();
      let merged_path_clone = merged_path_buf.clone();
      let output_dir_clone = output_dir.clone();
      let app_log_path = app_log_path.to_path_buf();
      let merged_id = merged.id;
      tauri::async_runtime::spawn(async move {
        let _ = update_workflow_status(
          &context_clone,
          &task_id_clone,
          "RUNNING",
          Some("SEGMENTING"),
          70.0,
        );
        let segment_outputs = if should_segment {
          if enable_segmentation {
            let merged_path_for_segment = merged_path_clone.clone();
            match tauri::async_runtime::spawn_blocking(move || {
              segment_file(&merged_path_for_segment, &output_dir_clone, segment_seconds)
            })
            .await
            {
              Ok(result) => result,
              Err(_) => Err("Failed to segment video".to_string()),
            }
          } else {
            Ok(vec![merged_path_clone.clone()])
          }
        } else {
          Ok(Vec::new())
        };
        match segment_outputs {
          Ok(outputs) => {
            if !should_segment {
              let latest_id = load_latest_merged_video(&context_clone, &task_id_clone)
                .ok()
                .flatten()
                .map(|value| value.id);
              if latest_id != Some(merged_id) {
                if let Err(err) = context_clone.db.with_conn(|conn| {
                  conn.execute("DELETE FROM merged_video WHERE task_id = ?1", [&task_id_clone])?;
                  Ok(())
                }) {
                  let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
                  let _ = update_workflow_status(
                    &context_clone,
                    &task_id_clone,
                    "FAILED",
                    Some("SEGMENTING"),
                    0.0,
                  );
                  append_log(
                    app_log_path.as_ref(),
                    &format!(
                      "submission_repost_cleanup_fail task_id={} err={}",
                      task_id_clone, err
                    ),
                  );
                  return;
                }
                let new_merged_id = match save_merged_video(
                  &context_clone,
                  &task_id_clone,
                  merged_path_clone.as_path(),
                ) {
                  Ok(merged_id) => merged_id,
                  Err(err) => {
                  let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
                  let _ = update_workflow_status(
                    &context_clone,
                    &task_id_clone,
                    "FAILED",
                    Some("SEGMENTING"),
                    0.0,
                  );
                  append_log(
                    app_log_path.as_ref(),
                    &format!(
                      "submission_repost_save_fail task_id={} err={}",
                      task_id_clone, err
                    ),
                  );
                  return;
                  }
                };
                if let Err(err) = copy_merged_source_bindings(
                  &context_clone,
                  &task_id_clone,
                  merged_id,
                  new_merged_id,
                ) {
                  append_log(
                    app_log_path.as_ref(),
                    &format!(
                      "submission_repost_bind_sources_fail task_id={} merged_id={} err={}",
                      task_id_clone, new_merged_id, err
                    ),
                  );
                }
              }
              let _ = update_submission_status(&context_clone, &task_id_clone, "WAITING_UPLOAD");
              let _ = update_workflow_status(&context_clone, &task_id_clone, "COMPLETED", None, 100.0);
              append_log(
                app_log_path.as_ref(),
                &format!("submission_repost_ok task_id={}", task_id_clone),
              );
              return;
            }
            if should_segment {
              if outputs.is_empty() {
                let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
                let _ = update_workflow_status(
                  &context_clone,
                  &task_id_clone,
                  "FAILED",
                  Some("SEGMENTING"),
                  0.0,
                );
                append_log(
                  app_log_path.as_ref(),
                  &format!("submission_repost_empty_outputs task_id={}", task_id_clone),
                );
                return;
              }
              if let Err(err) = replace_segments_for_merged(
                &context_clone,
                &task_id_clone,
                merged_id,
                &outputs,
                segment_prefix.as_deref(),
              ) {
                let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
                let _ = update_workflow_status(
                  &context_clone,
                  &task_id_clone,
                  "FAILED",
                  Some("SEGMENTING"),
                  0.0,
                );
                append_log(
                  app_log_path.as_ref(),
                  &format!(
                    "submission_repost_save_fail task_id={} err={}",
                    task_id_clone, err
                  ),
                );
                return;
              }
            }
            let _ = update_submission_status(&context_clone, &task_id_clone, "WAITING_UPLOAD");
            let _ = update_workflow_status(&context_clone, &task_id_clone, "COMPLETED", None, 100.0);
            append_log(
              app_log_path.as_ref(),
              &format!("submission_repost_ok task_id={}", task_id_clone),
            );
          }
          Err(err) => {
            let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
            let _ = update_workflow_status(
              &context_clone,
              &task_id_clone,
              "FAILED",
              Some("SEGMENTING"),
              0.0,
            );
            append_log(
              app_log_path.as_ref(),
              &format!(
                "submission_repost_segment_fail task_id={} err={}",
                task_id_clone, err
              ),
            );
          }
        }
      });
    }
    ReprocessMode::MergeAll => {
      let merged_videos = load_merged_videos_by_task(context, task_id)?;
      if merged_videos.is_empty() {
        return Err("未找到合并视频".to_string());
      }
      let merge_all_sources = collect_sources_for_merge_all(context, task_id, &merged_videos);
      let mut merge_inputs = Vec::with_capacity(merged_videos.len());
      for merged in &merged_videos {
        let path = merged.video_path.clone().unwrap_or_default();
        if path.trim().is_empty() || !PathBuf::from(&path).exists() {
          return Err("合并视频缺失".to_string());
        }
        merge_inputs.push(PathBuf::from(path));
      }
      let merge_output = build_merge_output_path(&base_dir, task_id);
      let context_clone = context.clone();
      let task_id_clone = task_id.to_string();
      let output_dir_clone = output_dir.clone();
      let merge_output_for_merge = merge_output.clone();
      let merge_output_for_segment = merge_output.clone();
      let merge_output_for_save = merge_output.clone();
      let app_log_path = app_log_path.to_path_buf();
      tauri::async_runtime::spawn(async move {
        let _ = update_workflow_status(
          &context_clone,
          &task_id_clone,
          "RUNNING",
          Some("SEGMENTING"),
          70.0,
        );
        let merge_inputs_clone = merge_inputs.clone();
        let merge_result = tauri::async_runtime::spawn_blocking(move || {
          merge_files(&merge_inputs_clone, &merge_output_for_merge)
        })
        .await;
        if let Err(err) = merge_result {
          let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
          let _ = update_workflow_status(
            &context_clone,
            &task_id_clone,
            "FAILED",
            Some("SEGMENTING"),
            0.0,
          );
          append_log(
            app_log_path.as_ref(),
            &format!("submission_repost_merge_fail task_id={} err={}", task_id_clone, err),
          );
          return;
        }
        let segment_outputs = if should_segment {
          if enable_segmentation {
            let merge_output_for_segment_clone = merge_output_for_segment.clone();
            match tauri::async_runtime::spawn_blocking(move || {
              segment_file(&merge_output_for_segment_clone, &output_dir_clone, segment_seconds)
            })
            .await
            {
              Ok(result) => result,
              Err(_) => Err("Failed to segment video".to_string()),
            }
          } else {
            Ok(vec![merge_output_for_segment.clone()])
          }
        } else {
          Ok(Vec::new())
        };
        match segment_outputs {
          Ok(outputs) => {
            if should_segment && outputs.is_empty() {
              let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
              let _ = update_workflow_status(
                &context_clone,
                &task_id_clone,
                "FAILED",
                Some("SEGMENTING"),
                0.0,
              );
              append_log(
                app_log_path.as_ref(),
                &format!("submission_repost_empty_outputs task_id={}", task_id_clone),
              );
              return;
            }
            if let Err(err) = context_clone.db.with_conn(|conn| {
              conn.execute("DELETE FROM merged_video WHERE task_id = ?1", [&task_id_clone])?;
              conn.execute("DELETE FROM task_output_segment WHERE task_id = ?1", [&task_id_clone])?;
              Ok(())
            }) {
              let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
              let _ = update_workflow_status(
                &context_clone,
                &task_id_clone,
                "FAILED",
                Some("SEGMENTING"),
                0.0,
              );
              append_log(
                app_log_path.as_ref(),
                &format!(
                  "submission_repost_cleanup_fail task_id={} err={}",
                  task_id_clone, err
                ),
              );
              return;
            }
            let merged_id = match save_merged_video(
              &context_clone,
              &task_id_clone,
              &merge_output_for_save,
            ) {
              Ok(merged_id) => merged_id,
              Err(err) => {
                let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
                let _ = update_workflow_status(
                  &context_clone,
                  &task_id_clone,
                  "FAILED",
                  Some("SEGMENTING"),
                  0.0,
                );
                append_log(
                  app_log_path.as_ref(),
                  &format!(
                    "submission_repost_save_fail task_id={} err={}",
                    task_id_clone, err
                  ),
                );
                return;
              }
            };
            if let Err(err) = save_merged_source_bindings(
              &context_clone,
              &task_id_clone,
              merged_id,
              &merge_all_sources,
            ) {
              append_log(
                app_log_path.as_ref(),
                &format!(
                  "submission_repost_bind_sources_fail task_id={} merged_id={} err={}",
                  task_id_clone, merged_id, err
                ),
              );
            }
            if should_segment {
              if let Err(err) =
                save_output_segments(
                  &context_clone,
                  &task_id_clone,
                  &outputs,
                  Some(merged_id),
                  segment_prefix.as_deref(),
                )
              {
                let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
                let _ = update_workflow_status(
                  &context_clone,
                  &task_id_clone,
                  "FAILED",
                  Some("SEGMENTING"),
                  0.0,
                );
                append_log(
                  app_log_path.as_ref(),
                  &format!(
                    "submission_repost_save_fail task_id={} err={}",
                    task_id_clone, err
                  ),
                );
                return;
              }
            }
            let _ = update_submission_status(&context_clone, &task_id_clone, "WAITING_UPLOAD");
            let _ = update_workflow_status(&context_clone, &task_id_clone, "COMPLETED", None, 100.0);
            append_log(
              app_log_path.as_ref(),
              &format!("submission_repost_ok task_id={}", task_id_clone),
            );
          }
          Err(err) => {
            let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
            let _ = update_workflow_status(
              &context_clone,
              &task_id_clone,
              "FAILED",
              Some("SEGMENTING"),
              0.0,
            );
            append_log(
              app_log_path.as_ref(),
              &format!(
                "submission_repost_segment_fail task_id={} err={}",
                task_id_clone, err
              ),
            );
          }
        }
      });
    }
    ReprocessMode::Legacy | ReprocessMode::FullReprocess => {
      append_log(
        app_log_path,
        &format!("submission_repost_resume_skip task_id={} mode=LEGACY", task_id),
      );
    }
  }
  Ok(())
}

pub async fn resume_reprocess_after_baidu_restore(
  db: Arc<Db>,
  app_log_path: Arc<PathBuf>,
  edit_upload_state: Arc<Mutex<EditUploadState>>,
  task_id: String,
  download_record_id: i64,
) {
  let context = SubmissionContext {
    db,
    app_log_path: app_log_path.clone(),
    edit_upload_state,
  };
  let record = context.db.with_conn(|conn| {
    conn
      .query_row(
        "SELECT download_url, local_path, status FROM video_download WHERE id = ?1",
        [download_record_id],
        |row| {
          Ok((
            row.get::<_, Option<String>>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, i64>(2)?,
          ))
        },
      )
      .optional()
  });
  let record = match record {
    Ok(Some(value)) => value,
    _ => return,
  };
  let (download_url, local_path, status) = record;
  if status != 2 {
    let _ = update_submission_status(&context, &task_id, "PENDING");
    let _ = update_workflow_status(&context, &task_id, "VIDEO_DOWNLOADING", None, 0.0);
    return;
  }
  let local_path = match local_path {
    Some(value) if !value.trim().is_empty() => value,
    _ => return,
  };
  let local_path_buf = PathBuf::from(&local_path);
  if !local_path_buf.exists() {
    let _ = update_submission_status(&context, &task_id, "PENDING");
    let _ = update_workflow_status(&context, &task_id, "VIDEO_DOWNLOADING", None, 0.0);
    return;
  }
  if let Some(remote_path) = download_url {
    if let Some((remote_dir, remote_name)) = split_baidu_path(&remote_path) {
      let _ = update_merged_video_by_remote_binding(
        &context,
        &task_id,
        &remote_dir,
        &remote_name,
        &local_path,
      );
    }
  }
  let (total, completed, failed) =
    load_remote_restore_stats(&context, &task_id).unwrap_or((0, 0, 0));
  if total == 0 {
    return;
  }
  if failed > 0 || completed < total {
    let _ = update_submission_status(&context, &task_id, "PENDING");
    let _ = update_workflow_status(&context, &task_id, "VIDEO_DOWNLOADING", None, 0.0);
    let _ = update_remote_restore_status(&context, &task_id, "PENDING_DOWNLOAD");
    return;
  }
  let _ = update_remote_restore_status(&context, &task_id, "READY");
  let workflow_type = load_latest_workflow_type(&context, &task_id)
    .ok()
    .flatten()
    .unwrap_or_default();
  let config = load_latest_workflow_config(&context, &task_id).ok().flatten();
  let (mode, merged_id) = load_reprocess_metadata(config.as_ref());
  let app_log_path = app_log_path.as_ref();
  if workflow_type == "VIDEO_RESEGMENT" {
    let _ = resume_resegment_after_restore(&context, app_log_path, &task_id, mode, merged_id).await;
  } else if workflow_type == "VIDEO_UPDATE" || workflow_type == "VIDEO_SUBMISSION" {
    let _ = resume_repost_after_restore(
      &context,
      app_log_path,
      &task_id,
      mode,
      merged_id,
      &workflow_type,
    )
    .await;
  } else {
    append_log(
      app_log_path,
      &format!(
        "submission_baidu_restore_skip task_id={} workflow_type={}",
        task_id, workflow_type
      ),
    );
  }
}

fn load_integrated_download_records(
  context: &SubmissionContext,
  task_id: &str,
) -> Result<Vec<IntegratedDownloadRecord>, String> {
  context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT vd.id, vd.download_url, vd.bvid, vd.aid, vd.title, vd.part_title, \
                vd.part_count, vd.current_part, vd.local_path, vd.resolution, vd.codec, \
                vd.format, vd.cid, vd.content \
         FROM task_relations tr \
         JOIN video_download vd ON tr.download_task_id = vd.id \
         WHERE tr.submission_task_id = ?1 AND tr.relation_type = 'INTEGRATED'",
      )?;
      let rows = stmt.query_map([task_id], |row| {
        Ok(IntegratedDownloadRecord {
          id: row.get(0)?,
          download_url: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
          bvid: row.get(2)?,
          aid: row.get(3)?,
          title: row.get(4)?,
          part_title: row.get(5)?,
          part_count: row.get(6)?,
          current_part: row.get(7)?,
          local_path: row.get::<_, Option<String>>(8)?.unwrap_or_default(),
          resolution: row.get(9)?,
          codec: row.get(10)?,
          format: row.get(11)?,
          cid: row.get(12)?,
          content: row.get(13)?,
        })
      })?;
      Ok(rows.collect::<Result<Vec<_>, _>>()?)
    })
    .map_err(|err| err.to_string())
}

fn create_retry_download_records(
  context: &SubmissionContext,
  task_id: &str,
  workflow_instance_id: &str,
  records: &[IntegratedDownloadRecord],
) -> Result<Vec<i64>, String> {
  if records.is_empty() {
    return Ok(Vec::new());
  }
  for record in records {
    if record.download_url.trim().is_empty() {
      return Err("下载记录缺少下载地址，无法重新下载".to_string());
    }
    if record.local_path.trim().is_empty() {
      return Err("下载记录缺少本地路径，无法重新下载".to_string());
    }
  }
  let now = now_rfc3339();
  context
    .db
    .with_conn(|conn| {
      let mut new_ids = Vec::with_capacity(records.len());
      for record in records {
        conn.execute(
          "INSERT INTO video_download (bvid, aid, title, part_title, part_count, current_part, download_url, local_path, status, progress, progress_total, progress_done, create_time, update_time, resolution, codec, format, cid, content, source_type) \
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, 0, 0, 0, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
          (
            record.bvid.as_deref(),
            record.aid.as_deref(),
            record.title.as_deref(),
            record.part_title.as_deref(),
            record.part_count,
            record.current_part,
            record.download_url.as_str(),
            record.local_path.as_str(),
            &now,
            &now,
            record.resolution.as_deref(),
            record.codec.as_deref(),
            record.format.as_deref(),
            record.cid,
            record.content.as_deref(),
            "BILIBILI",
          ),
        )?;
        let new_id = conn.last_insert_rowid();
        conn.execute(
          "DELETE FROM task_relations WHERE submission_task_id = ?1 AND download_task_id = ?2 AND relation_type = 'INTEGRATED'",
          (task_id, record.id),
        )?;
        conn.execute(
          "INSERT INTO task_relations (download_task_id, submission_task_id, relation_type, status, created_at, updated_at, workflow_instance_id, workflow_status, retry_count) \
           VALUES (?1, ?2, 'INTEGRATED', 'ACTIVE', ?3, ?4, ?5, 'PENDING_DOWNLOAD', 0)",
          (new_id, task_id, &now, &now, workflow_instance_id),
        )?;
        new_ids.push(new_id);
      }
      Ok(new_ids)
    })
    .map_err(|err| err.to_string())
}

fn reset_submission_for_repost(
  context: &SubmissionContext,
  app_log_path: &PathBuf,
  task_id: &str,
  workflow_config: &Value,
  workflow_type: &str,
  clear_bvid: bool,
  clean_updates: bool,
) -> Result<String, String> {
  append_log(
    app_log_path,
    &format!("submission_repost_start task_id={} type={}", task_id, workflow_type),
  );
  if let Err(err) = clear_edit_upload_segments_by_task(context, task_id) {
    append_log(
      app_log_path,
      &format!(
        "submission_repost_clear_cache_fail task_id={} err={}",
        task_id, err
      ),
    );
  }
  reset_workflow_instances(context, task_id)
    .map_err(|err| format!("重置工作流失败: {}", err))?;
  let cleanup_result = context.db.with_conn(|conn| {
    conn.execute("DELETE FROM task_output_segment WHERE task_id = ?1", [task_id])?;
    conn.execute("DELETE FROM video_clip WHERE task_id = ?1", [task_id])?;
    Ok(())
  });
  if let Err(err) = cleanup_result {
    return Err(format!("清理任务数据失败: {}", err));
  }
  let base_dir = resolve_submission_base_dir(context, task_id);
  if let Err(err) = cleanup_submission_derived_files(app_log_path, &base_dir) {
    append_log(
      app_log_path,
      &format!(
        "submission_repost_cleanup_fail task_id={} err={}",
        task_id, err
      ),
    );
  }
  if clean_updates {
    let updates_dir = base_dir.join("updates");
    if let Err(err) = remove_path_if_exists(app_log_path, "updates", &updates_dir) {
      append_log(
        app_log_path,
        &format!(
          "submission_repost_cleanup_updates_fail task_id={} err={}",
          task_id, err
        ),
      );
    }
  }
  let now = now_rfc3339();
  let update_result = context.db.with_conn(|conn| {
    if clear_bvid {
      conn.execute(
        "UPDATE submission_task SET status = 'PENDING', bvid = NULL, aid = NULL, remote_state = NULL, reject_reason = NULL, updated_at = ?1 WHERE task_id = ?2",
        (&now, task_id),
      )?;
    } else {
      conn.execute(
        "UPDATE submission_task SET status = 'PENDING', remote_state = NULL, reject_reason = NULL, updated_at = ?1 WHERE task_id = ?2",
        (&now, task_id),
      )?;
    }
    Ok(())
  });
  if let Err(err) = update_result {
    return Err(format!("重置任务状态失败: {}", err));
  }
  let (instance_id, _) =
    create_workflow_instance_for_task_with_type(context.db.as_ref(), task_id, workflow_config, workflow_type)
      .map_err(|err| format!("创建工作流失败: {}", err))?;
  let _ = context.db.with_conn(|conn| {
    conn.execute(
      "UPDATE task_relations SET workflow_instance_id = ?1, updated_at = ?2 WHERE submission_task_id = ?3 AND relation_type = 'INTEGRATED'",
      (&instance_id, &now, task_id),
    )?;
    Ok(())
  });
  Ok(instance_id)
}

#[tauri::command]
pub async fn submission_resegment(
  state: State<'_, AppState>,
  request: SubmissionResegmentRequest,
) -> Result<ApiResponse<String>, String> {
  let context = SubmissionContext::new(&state);
  let task_id = request.task_id.trim().to_string();
  if task_id.is_empty() {
    return Ok(ApiResponse::error("任务ID不能为空"));
  }
  if request.segment_duration_seconds <= 0 {
    return Ok(ApiResponse::error("分段时长必须大于0"));
  }
  let detail = match load_task_detail(&context, &task_id) {
    Ok(detail) => detail,
    Err(err) => return Ok(ApiResponse::error(err)),
  };
  let base_dir = resolve_submission_base_dir(&context, &task_id);
  let mode = parse_reprocess_mode(request.mode.as_deref());
  let integrate_current_bvid = request.integrate_current_bvid.unwrap_or(false);
  if integrate_current_bvid && detail.task.bvid.as_deref().unwrap_or("").trim().is_empty() {
    return Ok(ApiResponse::error(
      "当前任务暂无BVID，无法集成投稿，请选择新建BV".to_string(),
    ));
  }
  if mode != ReprocessMode::Legacy {
    append_log(
      &state.app_log_path,
      &format!(
        "submission_resegment_start task_id={} mode={}",
        task_id,
        if mode == ReprocessMode::MergeAll { "MERGE_ALL" } else { "SPECIFIED" }
      ),
    );
    let mut updated_config = build_resegment_workflow_config(
      detail.workflow_config.clone(),
      request.segment_duration_seconds,
    );
    apply_reprocess_metadata(&mut updated_config, mode, request.merged_video_id);
    apply_integrate_current_bvid(&mut updated_config, integrate_current_bvid);
    if let Err(err) = clear_edit_upload_segments_by_task(&context, &task_id) {
      append_log(
        &state.app_log_path,
        &format!(
          "submission_resegment_clear_cache_fail task_id={} err={}",
          task_id, err
        ),
      );
    }
    if let Err(err) = reset_workflow_instances(&context, &task_id) {
      return Ok(ApiResponse::error(format!("重置工作流失败: {}", err)));
    }
    if let Err(err) = create_workflow_instance_for_task_with_type(
      context.db.as_ref(),
      &task_id,
      &updated_config,
      "VIDEO_RESEGMENT",
    ) {
      return Ok(ApiResponse::error(format!("创建工作流失败: {}", err)));
    }
    let now = now_rfc3339();
    let update_result = context.db.with_conn(|conn| {
      if integrate_current_bvid {
        conn.execute(
          "UPDATE submission_task SET status = 'SEGMENTING', remote_state = NULL, reject_reason = NULL, updated_at = ?1 WHERE task_id = ?2",
          (&now, &task_id),
        )?;
      } else {
        conn.execute(
          "UPDATE submission_task SET status = 'SEGMENTING', bvid = NULL, aid = NULL, remote_state = NULL, reject_reason = NULL, updated_at = ?1 WHERE task_id = ?2",
          (&now, &task_id),
        )?;
      }
      Ok(())
    });
    if let Err(err) = update_result {
      return Ok(ApiResponse::error(format!("重置任务数据失败: {}", err)));
    }
    let output_dir = base_dir
      .join("resegment")
      .join(sanitize_filename(&format!("resegment_{}", now_rfc3339())))
      .join("output");
    let segment_seconds = request.segment_duration_seconds;
    let reprocess_sources = resolve_reprocess_sources(
      &context,
      &detail,
      &updated_config,
      mode,
      request.merged_video_id,
    );
    let reprocess_source_paths = reprocess_sources
      .iter()
      .map(|source| source.input_path.clone())
      .collect::<Vec<_>>();
    let sources_ready = !reprocess_sources.is_empty()
      && collect_missing_source_paths(&reprocess_source_paths).is_empty();
    let source_override = if mode == ReprocessMode::Specified {
      Some(reprocess_source_paths.clone())
    } else {
      None
    };
    if mode == ReprocessMode::Specified {
      let mut merged = match resolve_target_merged_video(
        &context,
        &task_id,
        request.merged_video_id,
        &state.app_log_path,
      ) {
        Ok(merged) => merged,
        Err(err) => {
          if sources_ready {
            return handle_repost_missing_assets(
              &state,
              &context,
              &detail,
              &task_id,
              &updated_config,
              false,
              "合并视频缺失",
              source_override.clone(),
            )
            .await;
          }
          return Ok(ApiResponse::error(err));
        }
      };
      let mut merged_path = merged.video_path.clone().unwrap_or_default();
      if merged_path.trim().is_empty() || !PathBuf::from(&merged_path).exists() {
        if sources_ready {
          let rebuilt = match rebuild_missing_merged_video(
            &context,
            &task_id,
            &merged,
            &reprocess_sources,
            &base_dir,
          ) {
            Ok(path) => path,
            Err(err) => return Ok(ApiResponse::error(err)),
          };
          merged_path = rebuilt.to_string_lossy().to_string();
          merged.video_path = Some(merged_path.clone());
        } else {
          match try_restore_merged_from_baidu(&context, &state.app_log_path, &merged, &base_dir).await? {
            BaiduRestoreResult::Ready(restored) => {
              merged_path = restored.to_string_lossy().to_string();
              merged.video_path = Some(merged_path.clone());
            }
            BaiduRestoreResult::Queued => {
              let _ = update_submission_status(&context, &task_id, "PENDING");
              let _ = update_workflow_status(&context, &task_id, "VIDEO_DOWNLOADING", None, 0.0);
              return Ok(ApiResponse::success(
                "合并视频缺失，已创建网盘下载任务，下载完成后自动重新分段".to_string(),
              ));
            }
            BaiduRestoreResult::NotBound => {}
          }
        }
      }
      if merged_path.trim().is_empty() {
        return Ok(ApiResponse::error("合并视频路径为空"));
      }
      let merged_path_buf = PathBuf::from(merged_path.clone());
      if !merged_path_buf.exists() {
        return Ok(ApiResponse::error("合并视频缺失"));
      }
      let segment_prefix = detail.task.segment_prefix.clone();
      let context_clone = context.clone();
      let task_id_clone = task_id.clone();
      let merged_path_clone = merged_path_buf.clone();
      let output_dir_clone = output_dir.clone();
      let app_log_path = state.app_log_path.clone();
      let merged_id = merged.id;
      tauri::async_runtime::spawn(async move {
        let _ = update_workflow_status(
          &context_clone,
          &task_id_clone,
          "RUNNING",
          Some("SEGMENTING"),
          70.0,
        );
        let merged_path_for_segment = merged_path_clone.clone();
        let segment_outputs = match tauri::async_runtime::spawn_blocking(move || {
          segment_file(&merged_path_for_segment, &output_dir_clone, segment_seconds)
        })
        .await
        {
          Ok(result) => result,
          Err(_) => Err("Failed to segment video".to_string()),
        };
        match segment_outputs {
          Ok(outputs) => {
            if outputs.is_empty() {
              let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
              let _ = update_workflow_status(
                &context_clone,
                &task_id_clone,
                "FAILED",
                Some("SEGMENTING"),
                0.0,
              );
              append_log(
                app_log_path.as_ref(),
                &format!(
                  "submission_resegment_empty_outputs task_id={}",
                  task_id_clone
                ),
              );
              return;
            }
            if let Err(err) = replace_segments_for_merged(
              &context_clone,
              &task_id_clone,
              merged_id,
              &outputs,
              segment_prefix.as_deref(),
            )
            {
              let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
              let _ = update_workflow_status(
                &context_clone,
                &task_id_clone,
                "FAILED",
                Some("SEGMENTING"),
                0.0,
              );
              append_log(
                app_log_path.as_ref(),
                &format!(
                  "submission_resegment_save_fail task_id={} err={}",
                  task_id_clone, err
                ),
              );
              return;
            }
            if !integrate_current_bvid {
              if let Err(err) = reset_segments_for_new_bvid(&context_clone, &task_id_clone) {
                let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
                let _ = update_workflow_status(
                  &context_clone,
                  &task_id_clone,
                  "FAILED",
                  Some("SEGMENTING"),
                  0.0,
                );
                append_log(
                  app_log_path.as_ref(),
                  &format!(
                    "submission_resegment_reset_segments_fail task_id={} err={}",
                    task_id_clone, err
                  ),
                );
                return;
              }
            }
            let _ = update_submission_status(&context_clone, &task_id_clone, "WAITING_UPLOAD");
            let _ =
              update_workflow_status(&context_clone, &task_id_clone, "COMPLETED", None, 100.0);
            append_log(
              app_log_path.as_ref(),
              &format!("submission_resegment_ok task_id={}", task_id_clone),
            );
          }
          Err(err) => {
            let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
            let _ = update_workflow_status(
              &context_clone,
              &task_id_clone,
              "FAILED",
              Some("SEGMENTING"),
              0.0,
            );
            append_log(
              app_log_path.as_ref(),
              &format!(
                "submission_resegment_segment_fail task_id={} err={}",
                task_id_clone, err
              ),
            );
          }
        }
      });
      return Ok(ApiResponse::success("重新分段已启动".to_string()));
    }
    let mut merged_videos = match load_merged_videos_by_task(&context, &task_id) {
      Ok(list) => list,
      Err(err) => return Ok(ApiResponse::error(err)),
    };
    if merged_videos.is_empty() {
      if sources_ready {
        return handle_repost_missing_assets(
          &state,
          &context,
          &detail,
          &task_id,
          &updated_config,
          false,
          "合并视频缺失",
          None,
        )
        .await;
      }
      return Ok(ApiResponse::error("未找到合并视频"));
    }
    let merge_all_sources = collect_sources_for_merge_all(&context, &task_id, &merged_videos);
    let mut merge_inputs = Vec::with_capacity(merged_videos.len());
    for merged in &mut merged_videos {
      let mut path = merged.video_path.clone().unwrap_or_default();
      if path.trim().is_empty() || !PathBuf::from(&path).exists() {
        if sources_ready {
          return handle_repost_missing_assets(
            &state,
            &context,
            &detail,
            &task_id,
            &updated_config,
            false,
            "合并视频缺失",
            None,
          )
          .await;
        }
        match try_restore_merged_from_baidu(&context, &state.app_log_path, merged, &base_dir).await? {
          BaiduRestoreResult::Ready(restored) => {
            path = restored.to_string_lossy().to_string();
            merged.video_path = Some(path.clone());
          }
          BaiduRestoreResult::Queued => {
            let _ = update_submission_status(&context, &task_id, "PENDING");
            let _ = update_workflow_status(&context, &task_id, "VIDEO_DOWNLOADING", None, 0.0);
            return Ok(ApiResponse::success(
              "合并视频缺失，已创建网盘下载任务，下载完成后自动重新分段".to_string(),
            ));
          }
          BaiduRestoreResult::NotBound => {}
        }
      }
      if path.trim().is_empty() {
        return handle_repost_missing_assets(
          &state,
          &context,
          &detail,
          &task_id,
          &updated_config,
          false,
          "合并视频缺失",
          None,
        )
        .await;
      }
      let path_buf = PathBuf::from(&path);
      if !path_buf.exists() {
        return handle_repost_missing_assets(
          &state,
          &context,
          &detail,
          &task_id,
          &updated_config,
          false,
          "合并视频缺失",
          None,
        )
        .await;
      }
      merge_inputs.push(path_buf);
    }
    let merge_output = build_merge_output_path(&base_dir, &task_id);
    let context_clone = context.clone();
    let task_id_clone = task_id.clone();
    let output_dir_clone = output_dir.clone();
    let merge_output_for_merge = merge_output.clone();
    let merge_output_for_segment = merge_output.clone();
    let merge_output_for_save = merge_output.clone();
    let segment_prefix = detail.task.segment_prefix.clone();
    let app_log_path = state.app_log_path.clone();
    tauri::async_runtime::spawn(async move {
      let _ = update_workflow_status(
        &context_clone,
        &task_id_clone,
        "RUNNING",
        Some("SEGMENTING"),
        70.0,
      );
      let merge_inputs_clone = merge_inputs.clone();
      let merge_result = tauri::async_runtime::spawn_blocking(move || {
        merge_files(&merge_inputs_clone, &merge_output_for_merge)
      })
      .await;
      if let Err(err) = merge_result {
        let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
        let _ = update_workflow_status(
          &context_clone,
          &task_id_clone,
          "FAILED",
          Some("SEGMENTING"),
          0.0,
        );
        append_log(
          app_log_path.as_ref(),
          &format!(
            "submission_resegment_merge_fail task_id={} err={}",
            task_id_clone, err
          ),
        );
        return;
      }
      let merge_output_for_segment_clone = merge_output_for_segment.clone();
      let segment_outputs = match tauri::async_runtime::spawn_blocking(move || {
        segment_file(&merge_output_for_segment_clone, &output_dir_clone, segment_seconds)
      })
      .await
      {
        Ok(result) => result,
        Err(_) => Err("Failed to segment video".to_string()),
      };
      match segment_outputs {
        Ok(outputs) => {
          if outputs.is_empty() {
            let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
            let _ = update_workflow_status(
              &context_clone,
              &task_id_clone,
              "FAILED",
              Some("SEGMENTING"),
              0.0,
            );
            append_log(
              app_log_path.as_ref(),
              &format!(
                "submission_resegment_empty_outputs task_id={}",
                task_id_clone
              ),
            );
            return;
          }
          if let Err(err) = context_clone.db.with_conn(|conn| {
            conn.execute("DELETE FROM merged_video WHERE task_id = ?1", [&task_id_clone])?;
            conn.execute("DELETE FROM task_output_segment WHERE task_id = ?1", [&task_id_clone])?;
            Ok(())
          }) {
            let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
            let _ = update_workflow_status(
              &context_clone,
              &task_id_clone,
              "FAILED",
              Some("SEGMENTING"),
              0.0,
            );
            append_log(
              app_log_path.as_ref(),
              &format!(
                "submission_resegment_cleanup_fail task_id={} err={}",
                task_id_clone, err
              ),
            );
            return;
          }
          let merged_id = match save_merged_video(
            &context_clone,
            &task_id_clone,
            &merge_output_for_save,
          ) {
            Ok(merged_id) => merged_id,
            Err(err) => {
              let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
              let _ = update_workflow_status(
                &context_clone,
                &task_id_clone,
                "FAILED",
                Some("SEGMENTING"),
                0.0,
              );
              append_log(
                app_log_path.as_ref(),
                &format!(
                  "submission_resegment_save_fail task_id={} err={}",
                  task_id_clone, err
                ),
              );
              return;
            }
          };
          if let Err(err) = save_merged_source_bindings(
            &context_clone,
            &task_id_clone,
            merged_id,
            &merge_all_sources,
          ) {
            append_log(
              app_log_path.as_ref(),
              &format!(
                "submission_resegment_bind_sources_fail task_id={} merged_id={} err={}",
                task_id_clone, merged_id, err
              ),
            );
          }
          if let Err(err) =
            save_output_segments(
              &context_clone,
              &task_id_clone,
              &outputs,
              Some(merged_id),
              segment_prefix.as_deref(),
            )
          {
            let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
            let _ = update_workflow_status(
              &context_clone,
              &task_id_clone,
              "FAILED",
              Some("SEGMENTING"),
              0.0,
            );
            append_log(
              app_log_path.as_ref(),
              &format!(
                "submission_resegment_save_fail task_id={} err={}",
                task_id_clone, err
              ),
            );
            return;
          }
          if !integrate_current_bvid {
            if let Err(err) = reset_segments_for_new_bvid(&context_clone, &task_id_clone) {
              let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
              let _ = update_workflow_status(
                &context_clone,
                &task_id_clone,
                "FAILED",
                Some("SEGMENTING"),
                0.0,
              );
              append_log(
                app_log_path.as_ref(),
                &format!(
                  "submission_resegment_reset_segments_fail task_id={} err={}",
                  task_id_clone, err
                ),
              );
              return;
            }
          }
          let _ = update_submission_status(&context_clone, &task_id_clone, "WAITING_UPLOAD");
          let _ =
            update_workflow_status(&context_clone, &task_id_clone, "COMPLETED", None, 100.0);
          append_log(
            app_log_path.as_ref(),
            &format!("submission_resegment_ok task_id={}", task_id_clone),
          );
        }
        Err(err) => {
          let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
          let _ = update_workflow_status(
            &context_clone,
            &task_id_clone,
            "FAILED",
            Some("SEGMENTING"),
            0.0,
          );
          append_log(
            app_log_path.as_ref(),
            &format!(
              "submission_resegment_segment_fail task_id={} err={}",
              task_id_clone, err
            ),
          );
        }
      }
    });
    return Ok(ApiResponse::success("重新分段已启动".to_string()));
  }
  let mut updated_config = build_resegment_workflow_config(
    detail.workflow_config.clone(),
    request.segment_duration_seconds,
  );
  apply_reprocess_metadata(&mut updated_config, ReprocessMode::Legacy, None);
  apply_integrate_current_bvid(&mut updated_config, integrate_current_bvid);
  let mut merged = match load_latest_merged_video(&context, &task_id) {
    Ok(Some(merged)) => merged,
    Ok(None) => return Ok(ApiResponse::error("未找到合并视频")),
    Err(err) => return Ok(ApiResponse::error(err)),
  };
  let mut merged_path = merged.video_path.clone().unwrap_or_default();
  if merged_path.trim().is_empty() || !PathBuf::from(&merged_path).exists() {
    match try_restore_merged_from_baidu(&context, &state.app_log_path, &merged, &base_dir).await? {
      BaiduRestoreResult::Ready(restored) => {
        merged_path = restored.to_string_lossy().to_string();
        merged.video_path = Some(merged_path.clone());
      }
      BaiduRestoreResult::Queued => {
        if let Err(err) = prepare_workflow_for_baidu_restore(
          &context,
          &state.app_log_path,
          &task_id,
          &updated_config,
          "VIDEO_RESEGMENT",
          true,
        ) {
          return Ok(ApiResponse::error(format!("准备网盘恢复失败: {}", err)));
        }
        return Ok(ApiResponse::success(
          "合并视频缺失，已创建网盘下载任务，下载完成后自动重新分段".to_string(),
        ));
      }
      BaiduRestoreResult::NotBound => {}
    }
  }
  if merged_path.trim().is_empty() {
    return handle_repost_missing_assets(
      &state,
      &context,
      &detail,
      &task_id,
      &updated_config,
      false,
      "合并视频缺失",
      None,
    )
    .await;
  }
  let merged_path_buf = PathBuf::from(merged_path.clone());
  let merged_id = merged.id;
  if !merged_path_buf.exists() {
    return handle_repost_missing_assets(
      &state,
      &context,
      &detail,
      &task_id,
      &updated_config,
      false,
      "合并视频缺失",
      None,
    )
    .await;
  }
  append_log(
    &state.app_log_path,
    &format!("submission_resegment_start task_id={}", task_id),
  );
  if let Err(err) = clear_edit_upload_segments_by_task(&context, &task_id) {
    append_log(
      &state.app_log_path,
      &format!(
        "submission_resegment_clear_cache_fail task_id={} err={}",
        task_id, err
      ),
    );
  }
  if let Err(err) = reset_workflow_instances(&context, &task_id) {
    return Ok(ApiResponse::error(format!("重置工作流失败: {}", err)));
  }
  if let Err(err) = create_workflow_instance_for_task_with_type(
    context.db.as_ref(),
    &task_id,
    &updated_config,
    "VIDEO_RESEGMENT",
  ) {
    return Ok(ApiResponse::error(format!("创建工作流失败: {}", err)));
  }
  let now = now_rfc3339();
  let cleanup_result = context.db.with_conn(|conn| {
    conn.execute("DELETE FROM task_output_segment WHERE task_id = ?1", [&task_id])?;
    if integrate_current_bvid {
      conn.execute(
        "UPDATE submission_task SET status = 'SEGMENTING', remote_state = NULL, reject_reason = NULL, updated_at = ?1 WHERE task_id = ?2",
        (&now, &task_id),
      )?;
    } else {
      conn.execute(
        "UPDATE submission_task SET status = 'SEGMENTING', bvid = NULL, aid = NULL, remote_state = NULL, reject_reason = NULL, updated_at = ?1 WHERE task_id = ?2",
        (&now, &task_id),
      )?;
    }
    Ok(())
  });
  if let Err(err) = cleanup_result {
    return Ok(ApiResponse::error(format!("重置任务数据失败: {}", err)));
  }
  let base_dir = resolve_submission_base_dir(&context, &task_id);
  let output_dir = base_dir.join("output");
  if let Err(err) = remove_path_if_exists(state.app_log_path.as_ref(), "output", &output_dir) {
    append_log(
      &state.app_log_path,
      &format!(
        "submission_resegment_cleanup_fail task_id={} err={}",
        task_id, err
      ),
    );
  }
  let context_clone = context.clone();
  let task_id_clone = task_id.clone();
  let merged_path_clone = merged_path_buf.clone();
  let output_dir_clone = output_dir.clone();
  let integrate_current_bvid = integrate_current_bvid;
  let app_log_path = state.app_log_path.clone();
  let segment_seconds = request.segment_duration_seconds;
  let segment_prefix = detail.task.segment_prefix.clone();
  tauri::async_runtime::spawn(async move {
    let _ = update_workflow_status(
      &context_clone,
      &task_id_clone,
      "RUNNING",
      Some("SEGMENTING"),
      70.0,
    );
    let segment_outputs = match tauri::async_runtime::spawn_blocking(move || {
      segment_file(&merged_path_clone, &output_dir_clone, segment_seconds)
    })
    .await
    {
      Ok(result) => result,
      Err(_) => Err("Failed to segment video".to_string()),
    };
    match segment_outputs {
      Ok(outputs) => {
        if outputs.is_empty() {
          let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
          let _ = update_workflow_status(
            &context_clone,
            &task_id_clone,
            "FAILED",
            Some("SEGMENTING"),
            0.0,
          );
          append_log(
            app_log_path.as_ref(),
            &format!(
              "submission_resegment_empty_outputs task_id={}",
              task_id_clone
            ),
          );
          return;
        }
        if let Err(err) = save_output_segments(
          &context_clone,
          &task_id_clone,
          &outputs,
          Some(merged_id),
          segment_prefix.as_deref(),
        )
        {
          let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
          let _ = update_workflow_status(
            &context_clone,
            &task_id_clone,
            "FAILED",
            Some("SEGMENTING"),
            0.0,
          );
          append_log(
            app_log_path.as_ref(),
            &format!(
              "submission_resegment_save_fail task_id={} err={}",
              task_id_clone, err
            ),
          );
          return;
        }
        if !integrate_current_bvid {
          if let Err(err) = reset_segments_for_new_bvid(&context_clone, &task_id_clone) {
            let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
            let _ = update_workflow_status(
              &context_clone,
              &task_id_clone,
              "FAILED",
              Some("SEGMENTING"),
              0.0,
            );
            append_log(
              app_log_path.as_ref(),
              &format!(
                "submission_resegment_reset_segments_fail task_id={} err={}",
                task_id_clone, err
              ),
            );
            return;
          }
        }
        let _ = update_submission_status(&context_clone, &task_id_clone, "WAITING_UPLOAD");
        let _ =
          update_workflow_status(&context_clone, &task_id_clone, "COMPLETED", None, 100.0);
        append_log(
          app_log_path.as_ref(),
          &format!("submission_resegment_ok task_id={}", task_id_clone),
        );
      }
      Err(err) => {
        let _ = update_submission_status(&context_clone, &task_id_clone, "FAILED");
        let _ = update_workflow_status(
          &context_clone,
          &task_id_clone,
          "FAILED",
          Some("SEGMENTING"),
          0.0,
        );
        append_log(
          app_log_path.as_ref(),
          &format!(
            "submission_resegment_segment_fail task_id={} err={}",
            task_id_clone, err
          ),
        );
      }
    }
  });
  Ok(ApiResponse::success("重新分段已启动".to_string()))
}

#[tauri::command]
pub async fn submission_list(
  state: State<'_, AppState>,
  page: Option<i64>,
  page_size: Option<i64>,
  query: Option<String>,
  refresh_remote: Option<bool>,
) -> Result<ApiResponse<PaginatedSubmissionTasks>, String> {
  let context = SubmissionContext::new(&state);
  if refresh_remote.unwrap_or(false) {
    let queue_context = build_submission_queue_context(&state);
    if let Err(err) = refresh_submission_remote_state(&queue_context).await {
      append_log(
        &state.app_log_path,
        &format!("submission_list_refresh_remote_fail err={}", err),
      );
    }
  }
  let page = page.unwrap_or(1).max(1);
  let page_size = page_size.unwrap_or(20).max(1);
  let response = match load_tasks(&context, None, page, page_size, query) {
    Ok(result) => ApiResponse::success(result),
    Err(err) => ApiResponse::error(format!("Failed to load tasks: {}", err)),
  };
  Ok(response)
}

#[tauri::command]
pub async fn submission_list_by_status(
  state: State<'_, AppState>,
  status: String,
  page: Option<i64>,
  page_size: Option<i64>,
  query: Option<String>,
  refresh_remote: Option<bool>,
) -> Result<ApiResponse<PaginatedSubmissionTasks>, String> {
  let context = SubmissionContext::new(&state);
  if refresh_remote.unwrap_or(false) {
    let queue_context = build_submission_queue_context(&state);
    if let Err(err) = refresh_submission_remote_state(&queue_context).await {
      append_log(
        &state.app_log_path,
        &format!(
          "submission_list_by_status_refresh_remote_fail status={} err={}",
          status, err
        ),
      );
    }
  }
  let page = page.unwrap_or(1).max(1);
  let page_size = page_size.unwrap_or(20).max(1);
  let response = match load_tasks(&context, Some(status), page, page_size, query) {
    Ok(result) => ApiResponse::success(result),
    Err(err) => ApiResponse::error(format!("Failed to load tasks: {}", err)),
  };
  Ok(response)
}

#[tauri::command]
pub fn submission_task_dir(state: State<'_, AppState>, task_id: String) -> ApiResponse<String> {
  let trimmed = task_id.trim();
  if trimmed.is_empty() {
    return ApiResponse::error("任务ID不能为空");
  }
  let context = SubmissionContext::new(&state);
  let dir = resolve_submission_base_dir(&context, trimmed);
  match fs::metadata(&dir) {
    Ok(meta) => {
      if meta.is_dir() {
        ApiResponse::success(dir.to_string_lossy().to_string())
      } else {
        ApiResponse::error("任务目录不是有效文件夹".to_string())
      }
    }
    Err(err) => ApiResponse::error(format!("任务目录不存在: {}", err)),
  }
}

#[tauri::command]
pub fn submission_delete_preview(
  state: State<'_, AppState>,
  task_id: String,
) -> ApiResponse<SubmissionDeletePreview> {
  let trimmed = task_id.trim();
  if trimmed.is_empty() {
    return ApiResponse::error("任务ID不能为空");
  }
  let context = SubmissionContext::new(&state);
  let mut files: Vec<DeleteFilePreview> = Vec::new();
  let base_dir = resolve_submission_base_dir(&context, trimmed);
  if path_exists(&base_dir) {
    files.push(DeleteFilePreview {
      path: base_dir.to_string_lossy().to_string(),
      conflicts: Vec::new(),
    });
  }
  let source_paths = match load_source_video_paths(&context, trimmed) {
    Ok(list) => list,
    Err(err) => return ApiResponse::error(err),
  };
  for path in source_paths {
    if !path_exists(Path::new(&path)) {
      continue;
    }
    let conflicts = match find_active_references(&context, trimmed, &path) {
      Ok(conflicts) => conflicts,
      Err(err) => return ApiResponse::error(err),
    };
    files.push(DeleteFilePreview { path, conflicts });
  }
  ApiResponse::success(SubmissionDeletePreview {
    task_id: trimmed.to_string(),
    files,
  })
}

#[tauri::command]
pub fn submission_detail(
  state: State<'_, AppState>,
  task_id: String,
) -> ApiResponse<SubmissionTaskDetail> {
  let context = SubmissionContext::new(&state);
  append_log(
    &state.app_log_path,
    &format!("submission_detail_request task_id={}", task_id),
  );
  match load_task_detail(&context, &task_id) {
    Ok(detail) => {
      append_log(
        &state.app_log_path,
        &format!(
          "submission_detail_ok task_id={} sources={} merged={} segments={} workflow={}",
          task_id,
          detail.source_videos.len(),
          detail.merged_videos.len(),
          detail.output_segments.len(),
          if detail.workflow_config.is_some() { 1 } else { 0 }
        ),
      );
      ApiResponse::success(detail)
    }
    Err(err) => {
      append_log(
        &state.app_log_path,
        &format!("submission_detail_fail task_id={} err={}", task_id, err),
      );
      ApiResponse::error(format!("Failed to load task detail: {}", err))
    }
  }
}

#[tauri::command]
pub fn submission_edit_prepare(
  state: State<'_, AppState>,
  task_id: String,
) -> ApiResponse<SubmissionTaskDetail> {
  let context = SubmissionContext::new(&state);
  let task_id = task_id.trim();
  if task_id.is_empty() {
    return ApiResponse::error("任务ID不能为空");
  }
  let mut detail = match load_task_detail(&context, task_id) {
    Ok(detail) => detail,
    Err(err) => return ApiResponse::error(format!("Failed to load task detail: {}", err)),
  };
  if let Err(err) = ensure_editable_detail(&detail) {
    return ApiResponse::error(err);
  }
  if !detail.output_segments.is_empty() {
    return ApiResponse::success(detail);
  }
  let merged = match load_latest_merged_video(&context, task_id) {
    Ok(Some(merged)) => merged,
    Ok(None) => return ApiResponse::error("未找到合并视频"),
    Err(err) => return ApiResponse::error(err),
  };
  let merged_path = merged.video_path.clone().unwrap_or_default();
  if merged_path.trim().is_empty() {
    return ApiResponse::error("合并视频路径为空");
  }
  let part_name = build_part_title(detail.task.segment_prefix.as_deref(), 1);
  let has_upload = merged.upload_cid.unwrap_or(0) > 0
    && merged
      .upload_file_name
      .as_deref()
      .map(|value| !value.trim().is_empty())
      .unwrap_or(false);
  let upload_status = if has_upload { "SUCCESS" } else { "PENDING" };
  let total_bytes = if merged.upload_total_bytes > 0 {
    merged.upload_total_bytes
  } else {
    fs::metadata(&merged_path)
      .map(|meta| meta.len() as i64)
      .unwrap_or(0)
  };
  let upload_progress = if has_upload {
    100.0
  } else {
    merged.upload_progress
  };
  let segment_id = uuid::Uuid::new_v4().to_string();
  detail.output_segments.push(TaskOutputSegmentRecord {
    segment_id,
    task_id: task_id.to_string(),
    merged_id: Some(merged.id),
    part_name,
    segment_file_path: merged_path,
    part_order: 1,
    upload_status: upload_status.to_string(),
    cid: merged.upload_cid,
    file_name: merged.upload_file_name.clone(),
    upload_progress,
    upload_uploaded_bytes: merged.upload_uploaded_bytes,
    upload_total_bytes: total_bytes,
    upload_session_id: None,
    upload_biz_id: 0,
    upload_endpoint: None,
    upload_auth: None,
    upload_uri: None,
    upload_chunk_size: 0,
    upload_last_part_index: 0,
  });
  ApiResponse::success(detail)
}

#[tauri::command]
pub async fn submission_edit_add_segment(
  state: State<'_, AppState>,
  request: SubmissionEditAddSegmentRequest,
) -> Result<ApiResponse<TaskOutputSegmentRecord>, String> {
  let context = SubmissionContext::new(&state);
  let task_id = request.task_id.trim().to_string();
  if task_id.is_empty() {
    return Ok(ApiResponse::error("任务ID不能为空"));
  }
  let file_path = request.file_path.trim().to_string();
  if file_path.is_empty() {
    return Ok(ApiResponse::error("分P文件路径不能为空"));
  }
  append_log(
    &state.app_log_path,
    &format!(
      "submission_edit_add_segment_start task_id={} file_path={}",
      task_id, file_path
    ),
  );
  let path = Path::new(&file_path);
  if !path.exists() {
    append_log(
      &state.app_log_path,
      &format!(
        "submission_edit_add_segment_fail task_id={} reason=file_missing",
        task_id
      ),
    );
    return Ok(ApiResponse::error("分P文件不存在"));
  }
  if let Err(err) = ensure_editable_status(&context, &task_id) {
    append_log(
      &state.app_log_path,
      &format!(
        "submission_edit_add_segment_fail task_id={} reason={}",
        task_id, err
      ),
    );
    return Ok(ApiResponse::error(err));
  }
  let part_name = request
    .part_name
    .as_deref()
    .map(|value| value.trim())
    .filter(|value| !value.is_empty())
    .map(|value| value.to_string())
    .unwrap_or_else(|| default_part_name_from_path(&file_path));
  let total_bytes = fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
  let segment_id = uuid::Uuid::new_v4().to_string();
  let segment = TaskOutputSegmentRecord {
    segment_id: segment_id.clone(),
    task_id: task_id.clone(),
    merged_id: None,
    part_name,
    segment_file_path: file_path,
    part_order: 0,
    upload_status: "UPLOADING".to_string(),
    cid: None,
    file_name: None,
    upload_progress: 0.0,
    upload_uploaded_bytes: 0,
    upload_total_bytes: total_bytes as i64,
    upload_session_id: None,
    upload_biz_id: 0,
    upload_endpoint: None,
    upload_auth: None,
    upload_uri: None,
    upload_chunk_size: 0,
    upload_last_part_index: 0,
  };
  let segment = match upsert_edit_upload_segment(&context, segment) {
    Ok(segment) => segment,
    Err(err) => {
      append_log(
        &state.app_log_path,
        &format!(
          "submission_edit_add_segment_fail task_id={} reason=state err={}",
          task_id, err
        ),
      );
      return Ok(ApiResponse::error(err));
    }
  };
  append_log(
    &state.app_log_path,
    &format!(
      "submission_edit_add_segment_cached task_id={} segment_id={}",
      task_id, segment.segment_id
    ),
  );
  let upload_context = UploadContext::new(&state);
  let auth = match load_auth_or_refresh(&upload_context, "submission_edit_add_segment").await {
    Ok(auth) => auth,
    Err(err) => return Ok(ApiResponse::error(err)),
  };
  let context_clone = context.clone();
  let upload_context_clone = upload_context.clone();
  let segment_id_clone = segment.segment_id.clone();
  append_log(
    &state.app_log_path,
    &format!(
      "submission_edit_add_segment_upload_queued task_id={} segment_id={}",
      task_id, segment_id
    ),
  );
  tauri::async_runtime::spawn(async move {
    append_log(
      upload_context_clone.app_log_path.as_ref(),
      &format!(
        "submission_edit_add_segment_upload_start segment_id={}",
        segment_id_clone
      ),
    );
    let client = Client::new();
    let result = upload_edit_segment_with_retry(
      &context_clone,
      &upload_context_clone,
      &client,
      &auth,
      &segment_id_clone,
      upload_context_clone.app_log_path.as_ref(),
      UPLOAD_SEGMENT_RETRY_LIMIT,
    )
    .await;
    match result {
      Ok(upload_result) => {
        let _ = update_edit_upload_segment(&context_clone, &segment_id_clone, |segment| {
          segment.upload_status = "SUCCESS".to_string();
          segment.cid = Some(upload_result.cid);
          segment.file_name = Some(upload_result.filename);
        });
        append_log(
          upload_context_clone.app_log_path.as_ref(),
          &format!(
            "submission_edit_add_segment_upload_ok segment_id={}",
            segment_id_clone
          ),
        );
      }
      Err(err) => {
        let _ = update_edit_upload_segment(&context_clone, &segment_id_clone, |segment| {
          segment.upload_status = "FAILED".to_string();
        });
        append_log(
          upload_context_clone.app_log_path.as_ref(),
          &format!("submission_edit_add_segment_fail segment_id={} err={}", segment_id_clone, err),
        );
      }
    }
  });
  append_log(
    &state.app_log_path,
    &format!(
      "submission_edit_add_segment_return task_id={} segment_id={} status={}",
      task_id, segment.segment_id, segment.upload_status
    ),
  );
  Ok(ApiResponse::success(segment))
}

#[tauri::command]
pub async fn submission_edit_reupload_segment(
  state: State<'_, AppState>,
  request: SubmissionEditReuploadSegmentRequest,
) -> Result<ApiResponse<TaskOutputSegmentRecord>, String> {
  let context = SubmissionContext::new(&state);
  let task_id = request.task_id.trim().to_string();
  if task_id.is_empty() {
    return Ok(ApiResponse::error("任务ID不能为空"));
  }
  let segment_id = request.segment_id.trim().to_string();
  if segment_id.is_empty() {
    return Ok(ApiResponse::error("分P ID不能为空"));
  }
  let file_path = request.file_path.trim().to_string();
  if file_path.is_empty() {
    return Ok(ApiResponse::error("分P文件路径不能为空"));
  }
  let path = Path::new(&file_path);
  if !path.exists() {
    return Ok(ApiResponse::error("分P文件不存在"));
  }
  if let Err(err) = ensure_editable_status(&context, &task_id) {
    return Ok(ApiResponse::error(err));
  }
  let existing = match load_edit_upload_segment(&context, &segment_id) {
    Ok(Some(segment)) => Some(segment),
    Ok(None) => load_output_segment_by_id(&context, &segment_id)
      .map_err(|err| err.to_string())?,
    Err(err) => return Ok(ApiResponse::error(err)),
  };
  if let Some(segment) = &existing {
    if segment.task_id != task_id {
      return Ok(ApiResponse::error("分P不属于当前任务"));
    }
  }
  let part_name = default_part_name_from_path(&file_path);
  let total_bytes = fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
  let mut segment = existing.unwrap_or(TaskOutputSegmentRecord {
    segment_id: segment_id.clone(),
    task_id: task_id.clone(),
    merged_id: None,
    part_name: part_name.clone(),
    segment_file_path: file_path.clone(),
    part_order: 0,
    upload_status: "UPLOADING".to_string(),
    cid: None,
    file_name: None,
    upload_progress: 0.0,
    upload_uploaded_bytes: 0,
    upload_total_bytes: total_bytes as i64,
    upload_session_id: None,
    upload_biz_id: 0,
    upload_endpoint: None,
    upload_auth: None,
    upload_uri: None,
    upload_chunk_size: 0,
    upload_last_part_index: 0,
  });
  segment.part_name = part_name;
  segment.segment_file_path = file_path;
  segment.upload_status = "UPLOADING".to_string();
  segment.cid = None;
  segment.file_name = None;
  segment.upload_progress = 0.0;
  segment.upload_uploaded_bytes = 0;
  segment.upload_total_bytes = total_bytes as i64;
  segment.upload_session_id = None;
  segment.upload_biz_id = 0;
  segment.upload_endpoint = None;
  segment.upload_auth = None;
  segment.upload_uri = None;
  segment.upload_chunk_size = 0;
  segment.upload_last_part_index = 0;
  let segment = match upsert_edit_upload_segment(&context, segment) {
    Ok(segment) => segment,
    Err(err) => return Ok(ApiResponse::error(err)),
  };
  let upload_context = UploadContext::new(&state);
  let auth = match load_auth_or_refresh(&upload_context, "submission_edit_reupload").await {
    Ok(auth) => auth,
    Err(err) => return Ok(ApiResponse::error(err)),
  };
  let context_clone = context.clone();
  let upload_context_clone = upload_context.clone();
  let segment_id_clone = segment.segment_id.clone();
  tauri::async_runtime::spawn(async move {
    let client = Client::new();
    let result = upload_edit_segment_with_retry(
      &context_clone,
      &upload_context_clone,
      &client,
      &auth,
      &segment_id_clone,
      upload_context_clone.app_log_path.as_ref(),
      UPLOAD_SEGMENT_RETRY_LIMIT,
    )
    .await;
    match result {
      Ok(upload_result) => {
        let _ = update_edit_upload_segment(&context_clone, &segment_id_clone, |segment| {
          segment.upload_status = "SUCCESS".to_string();
          segment.cid = Some(upload_result.cid);
          segment.file_name = Some(upload_result.filename);
        });
      }
      Err(err) => {
        let _ = update_edit_upload_segment(&context_clone, &segment_id_clone, |segment| {
          segment.upload_status = "FAILED".to_string();
        });
        append_log(
          upload_context_clone.app_log_path.as_ref(),
          &format!(
            "submission_edit_reupload_fail segment_id={} err={}",
            segment_id_clone, err
          ),
        );
      }
    }
  });
  Ok(ApiResponse::success(segment))
}

#[tauri::command]
pub fn submission_edit_upload_status(
  state: State<'_, AppState>,
  request: SubmissionEditUploadStatusRequest,
) -> Result<ApiResponse<Vec<TaskOutputSegmentRecord>>, String> {
  let context = SubmissionContext::new(&state);
  let task_id = request.task_id.trim();
  if task_id.is_empty() {
    return Ok(ApiResponse::error("任务ID不能为空"));
  }
  let segment_ids = request.segment_ids.unwrap_or_default();
  let segments = if segment_ids.is_empty() {
    list_edit_upload_segments_by_task(&context, task_id, None)
  } else {
    list_edit_upload_segments_by_task(&context, task_id, Some(&segment_ids))
  };
  let segments = match segments {
    Ok(segments) => segments,
    Err(err) => return Ok(ApiResponse::error(err)),
  };
  Ok(ApiResponse::success(segments))
}

#[tauri::command]
pub fn submission_edit_upload_clear(
  state: State<'_, AppState>,
  request: SubmissionEditUploadClearRequest,
) -> Result<ApiResponse<String>, String> {
  let context = SubmissionContext::new(&state);
  let task_id = request.task_id.trim();
  if task_id.is_empty() {
    return Ok(ApiResponse::error("任务ID不能为空"));
  }
  clear_edit_upload_segments_by_task(&context, task_id)?;
  Ok(ApiResponse::success("OK".to_string()))
}

#[tauri::command]
pub async fn submission_edit_submit(
  state: State<'_, AppState>,
  request: SubmissionEditSubmitRequest,
) -> Result<ApiResponse<String>, String> {
  let context = SubmissionContext::new(&state);
  let task_id = request.task_id.trim().to_string();
  if task_id.is_empty() {
    return Ok(ApiResponse::error("任务ID不能为空"));
  }
  let mut detail = match load_task_detail(&context, &task_id) {
    Ok(detail) => detail,
    Err(err) => return Ok(ApiResponse::error(err)),
  };
  if let Err(err) = ensure_editable_detail(&detail) {
    return Ok(ApiResponse::error(err));
  }
  let title = request.task.title.trim();
  if title.is_empty() {
    return Ok(ApiResponse::error("投稿标题不能为空"));
  }
  if title.len() > 80 {
    return Ok(ApiResponse::error("投稿标题不能超过 80 个字符"));
  }
  if request.task.partition_id <= 0 {
    return Ok(ApiResponse::error("请选择B站分区"));
  }
  if request.task.video_type.trim().is_empty() {
    return Ok(ApiResponse::error("请选择视频类型"));
  }
  if let Some(description) = request.task.description.as_deref() {
    if description.len() > 2000 {
      return Ok(ApiResponse::error("视频描述不能超过 2000 个字符"));
    }
  }
  let tags = request.task.tags.clone().unwrap_or_default();
  if tags.trim().is_empty() {
    return Ok(ApiResponse::error("请填写至少一个投稿标签"));
  }
  if request.segments.is_empty() {
    return Ok(ApiResponse::error("至少需要保留一个分P"));
  }
  let mut ordered_segments = request.segments.clone();
  ordered_segments.sort_by_key(|segment| segment.part_order);
  let mut parts = Vec::new();
  let mut seen = HashSet::new();
  for segment in &ordered_segments {
    let segment_id = segment.segment_id.trim();
    if segment_id.is_empty() {
      return Ok(ApiResponse::error("分P ID不能为空"));
    }
    if !seen.insert(segment_id.to_string()) {
      return Ok(ApiResponse::error("存在重复的分P"));
    }
    let part_name = segment.part_name.trim();
    if part_name.is_empty() {
      return Ok(ApiResponse::error("分P名称不能为空"));
    }
    if segment.segment_file_path.trim().is_empty() {
      return Ok(ApiResponse::error("分P文件路径不能为空"));
    }
    let cid = match segment.cid {
      Some(cid) if cid > 0 => cid,
      _ => return Ok(ApiResponse::error("分P上传信息缺失，请重新上传")),
    };
    let filename = match segment
      .file_name
      .as_deref()
      .map(|value| value.trim())
      .filter(|value| !value.is_empty())
    {
      Some(value) => value.to_string(),
      None => return Ok(ApiResponse::error("分P上传信息缺失，请重新上传")),
    };
    parts.push(UploadedVideoPart {
      filename,
      cid,
      title: part_name.to_string(),
    });
  }
  let upload_context = UploadContext::new(&state);
  let mut auth = match load_auth_or_refresh(&upload_context, "submission_edit_prepare").await {
    Ok(auth) => auth,
    Err(err) => return Ok(ApiResponse::error(err)),
  };
  let csrf = match auth.csrf.clone() {
    Some(value) => value,
    None => {
      auth = match refresh_auth(&upload_context, "submission_edit_prepare_csrf").await {
        Ok(auth) => auth,
        Err(err) => return Ok(ApiResponse::error(err)),
      };
      match auth.csrf.clone() {
        Some(value) => value,
        None => return Ok(ApiResponse::error("登录信息缺少CSRF")),
      }
    }
  };
  let mut aid = detail.task.aid.unwrap_or(0);
  if aid <= 0 {
    let bvid = detail.task.bvid.clone().unwrap_or_default();
    aid = fetch_aid_with_refresh(&upload_context, &auth, &bvid)
      .await
      .unwrap_or(0);
    if aid <= 0 {
      return Ok(ApiResponse::error("无法获取AID，无法编辑"));
    }
    let _ = update_submission_aid(&context, &task_id, aid);
    detail.task.aid = Some(aid);
  }
  let original_collection_id = detail.task.collection_id.unwrap_or(0);
  let mut task = detail.task.clone();
  task.title = title.to_string();
  task.description = request.task.description.clone();
  task.partition_id = request.task.partition_id;
  task.collection_id = request.task.collection_id;
  task.tags = Some(tags.clone());
  task.video_type = request.task.video_type.clone();
  task.segment_prefix = request.task.segment_prefix.clone();
  if let Some(topic_id) = request.task.topic_id {
    task.topic_id = if topic_id > 0 { Some(topic_id) } else { None };
  }
  if let Some(mission_id) = request.task.mission_id {
    task.mission_id = if mission_id > 0 { Some(mission_id) } else { None };
  }
  if let Some(activity_title) = request.task.activity_title.clone() {
    let trimmed = activity_title.trim().to_string();
    task.activity_title = if trimmed.is_empty() { None } else { Some(trimmed) };
  }
  task.aid = Some(aid);
  if let Err(err) =
    submit_video_edit_with_refresh(&upload_context, &auth, &task, &parts, aid, &csrf).await
  {
    return Ok(ApiResponse::error(err));
  }
  let next_collection_id = task.collection_id.unwrap_or(0);
  if next_collection_id != original_collection_id {
    append_log(
      &upload_context.app_log_path,
      &format!(
        "submission_edit_collection_change task_id={} from={} to={}",
        task_id, original_collection_id, next_collection_id
      ),
    );
    if next_collection_id > 0 {
      if let Err(err) = switch_video_collection_with_refresh(
        &upload_context,
        &auth,
        &task.title,
        next_collection_id,
        aid,
        &csrf,
      )
      .await
      {
        if is_collection_not_found_error(&err) {
          append_log(
            &upload_context.app_log_path,
            &format!(
              "submission_edit_collection_switch_skip task_id={} collection_id={} err={}",
              task_id, next_collection_id, err
            ),
          );
        } else {
          append_log(
            &upload_context.app_log_path,
            &format!(
              "submission_edit_collection_switch_fail task_id={} collection_id={} err={}",
              task_id, next_collection_id, err
            ),
          );
          return Ok(ApiResponse::error(err));
        }
      }
    } else {
      append_log(
        &upload_context.app_log_path,
        &format!(
          "submission_edit_collection_switch_skip task_id={} collection_id=0",
          task_id
        ),
      );
    }
  } else {
    append_log(
      &upload_context.app_log_path,
      &format!(
        "submission_edit_collection_skip task_id={} from={} to={}",
        task_id, original_collection_id, next_collection_id
      ),
    );
  }
  if let Err(err) = update_submission_task_for_edit(&context, &task_id, &task) {
    return Ok(ApiResponse::error(err));
  }
  if let Err(err) = update_output_segments_for_edit(&context, &task_id, &ordered_segments) {
    return Ok(ApiResponse::error(err));
  }
  if let Err(err) = clear_edit_upload_segments_by_task(&context, &task_id) {
    append_log(
      &upload_context.app_log_path,
      &format!(
        "submission_edit_clear_cache_fail task_id={} err={}",
        task_id, err
      ),
    );
  }
  Ok(ApiResponse::success("编辑投稿成功".to_string()))
}

#[tauri::command]
pub fn submission_delete(
  state: State<'_, AppState>,
  request: SubmissionDeleteRequest,
) -> ApiResponse<SubmissionDeleteResult> {
  let task_id = request.task_id.trim().to_string();
  if task_id.is_empty() {
    return ApiResponse::error("任务ID不能为空");
  }
  if !request.delete_task && !request.delete_files {
    return ApiResponse::error("至少选择删除任务或删除文件");
  }
  let context = SubmissionContext::new(&state);
  let base_dir = resolve_submission_base_dir(&context, &task_id);
  append_log(
    &state.app_log_path,
    &format!(
      "submission_delete_start task_id={} delete_task={} delete_files={} force_delete={}",
      task_id, request.delete_task, request.delete_files, request.force_delete
    ),
  );

  let mut conflict_files = Vec::new();
  if request.delete_files && !request.force_delete {
    let delete_paths = normalize_delete_paths(&request.delete_paths);
    for path in &delete_paths {
      if !path_exists(Path::new(path)) {
        continue;
      }
      if let Ok(conflicts) = find_active_references(&context, &task_id, path) {
        if !conflicts.is_empty() {
          conflict_files.push(DeleteFilePreview {
            path: path.to_string(),
            conflicts,
          });
        }
      }
    }
    if !conflict_files.is_empty() {
      append_log(
        &state.app_log_path,
        &format!(
          "submission_delete_blocked task_id={} conflicts={}",
          task_id,
          conflict_files.len()
        ),
      );
      return ApiResponse::success(SubmissionDeleteResult {
        blocked: true,
        conflicts: conflict_files,
        deleted_paths: Vec::new(),
        missing_paths: Vec::new(),
      });
    }
  }

  let mut source_video_set = HashSet::new();
  if request.delete_files {
    match load_source_video_paths(&context, &task_id) {
      Ok(paths) => {
        source_video_set = paths.into_iter().collect();
      }
      Err(err) => {
        append_log(
          &state.app_log_path,
          &format!(
            "submission_delete_source_paths_fail task_id={} err={}",
            task_id, err
          ),
        );
      }
    }
  }

  let mut deleted_paths = Vec::new();
  let mut missing_paths = Vec::new();
  if request.delete_task {
    let result = context.db.with_conn_mut(|conn| {
      let tx = conn.transaction()?;
      tx.execute(
        "DELETE FROM workflow_execution_logs WHERE instance_id IN (SELECT instance_id FROM workflow_instances WHERE task_id = ?1)",
        [&task_id],
      )?;
      tx.execute(
        "DELETE FROM workflow_performance_metrics WHERE instance_id IN (SELECT instance_id FROM workflow_instances WHERE task_id = ?1)",
        [&task_id],
      )?;
      tx.execute(
        "DELETE FROM workflow_steps WHERE instance_id IN (SELECT instance_id FROM workflow_instances WHERE task_id = ?1)",
        [&task_id],
      )?;
      tx.execute("DELETE FROM workflow_instances WHERE task_id = ?1", [&task_id])?;
      tx.execute("DELETE FROM task_relations WHERE submission_task_id = ?1", [&task_id])?;
      tx.execute("DELETE FROM task_output_segment WHERE task_id = ?1", [&task_id])?;
      tx.execute("DELETE FROM merged_video WHERE task_id = ?1", [&task_id])?;
      tx.execute("DELETE FROM task_source_video WHERE task_id = ?1", [&task_id])?;
      tx.execute("DELETE FROM video_clip WHERE task_id = ?1", [&task_id])?;
      let deleted = tx.execute("DELETE FROM submission_task WHERE task_id = ?1", [&task_id])?;
      if deleted == 0 {
        return Err(rusqlite::Error::QueryReturnedNoRows);
      }
      tx.commit()?;
      Ok(())
    });
    if let Err(err) = result {
      append_log(
        &state.app_log_path,
        &format!("submission_delete_fail task_id={} err={}", task_id, err),
      );
      return ApiResponse::error(format!("Failed to delete: {}", err));
    }
    if let Err(err) = cleanup_submission_files(&state.app_log_path, &base_dir) {
      append_log(
        &state.app_log_path,
        &format!("submission_delete_cleanup_fail task_id={} err={}", task_id, err),
      );
      return ApiResponse::error(format!("任务已删除，但清理文件失败: {}", err));
    }
  }

  if request.delete_files {
    let delete_paths = normalize_delete_paths(&request.delete_paths);
    for path in &delete_paths {
      let target = Path::new(path);
      if !path_exists(target) {
        missing_paths.push(path.to_string());
        continue;
      }
      let is_file = match fs::metadata(target) {
        Ok(metadata) => metadata.is_file(),
        Err(err) => return ApiResponse::error(format!("读取路径失败: {}", err)),
      };
      if let Err(err) = remove_path_if_exists(&state.app_log_path, "custom", target) {
        return ApiResponse::error(err);
      }
      deleted_paths.push(path.to_string());
      if is_file && source_video_set.contains(path) {
        cleanup_empty_parent_dir(&state.app_log_path, target);
      }
    }
  }

  append_log(
    &state.app_log_path,
    &format!("submission_delete_ok task_id={}", task_id),
  );
  ApiResponse::success(SubmissionDeleteResult {
    blocked: false,
    conflicts: Vec::new(),
    deleted_paths,
    missing_paths,
  })
}

fn cleanup_submission_files(log_path: &PathBuf, base_dir: &Path) -> Result<(), String> {
  let targets = [
    ("cut", base_dir.join("cut")),
    ("merge", base_dir.join("merge")),
    ("output", base_dir.join("output")),
  ];
  for (label, path) in targets {
    remove_path_if_exists(log_path, label, &path)?;
  }
  remove_path_if_exists(log_path, "base", base_dir)?;
  Ok(())
}

fn cleanup_submission_derived_files(log_path: &PathBuf, base_dir: &Path) -> Result<(), String> {
  let targets = [
    ("cut", base_dir.join("cut")),
    ("merge", base_dir.join("merge")),
    ("output", base_dir.join("output")),
  ];
  for (label, path) in targets {
    remove_path_if_exists(log_path, label, &path)?;
  }
  Ok(())
}

fn path_exists(path: &Path) -> bool {
  fs::metadata(path).is_ok()
}

fn normalize_delete_paths(paths: &Option<Vec<String>>) -> Vec<String> {
  let mut result = Vec::new();
  let mut seen = HashSet::new();
  for path in paths.as_ref().unwrap_or(&Vec::new()) {
    let trimmed = path.trim();
    if trimmed.is_empty() {
      continue;
    }
    let normalized = trimmed.to_string();
    if seen.insert(normalized.clone()) {
      result.push(normalized);
    }
  }
  result
}

fn load_source_video_paths(
  context: &SubmissionContext,
  task_id: &str,
) -> Result<Vec<String>, String> {
  context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT DISTINCT source_file_path FROM task_source_video WHERE task_id = ?1 ORDER BY sort_order ASC",
      )?;
      let rows = stmt.query_map([task_id], |row| row.get(0))?;
      let mut paths = Vec::new();
      for path in rows {
        if let Ok(value) = path {
          paths.push(value);
        }
      }
      Ok(paths)
    })
    .map_err(|err| err.to_string())
}

fn find_active_references(
  context: &SubmissionContext,
  current_task_id: &str,
  file_path: &str,
) -> Result<Vec<DeleteConflictRef>, String> {
  context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT st.task_id, st.status, st.title \
         FROM task_source_video tsv \
         JOIN submission_task st ON st.task_id = tsv.task_id \
         WHERE tsv.source_file_path = ?1 AND tsv.task_id != ?2 AND st.status != 'COMPLETED'",
      )?;
      let rows = stmt.query_map((file_path, current_task_id), |row| {
        Ok(DeleteConflictRef {
          task_id: row.get(0)?,
          status: row.get(1)?,
          title: row.get(2)?,
        })
      })?;
      let mut result = Vec::new();
      for item in rows {
        if let Ok(value) = item {
          result.push(value);
        }
      }
      Ok(result)
    })
    .map_err(|err| err.to_string())
}

fn remove_path_if_exists(log_path: &PathBuf, label: &str, path: &Path) -> Result<(), String> {
  match fs::metadata(path) {
    Ok(metadata) => {
      append_log(
        log_path,
        &format!(
          "submission_cleanup_start label={} path={}",
          label,
          path.to_string_lossy()
        ),
      );
      let result = if metadata.is_dir() {
        fs::remove_dir_all(path)
      } else {
        fs::remove_file(path)
      };
      match result {
        Ok(()) => {
          append_log(
            log_path,
            &format!(
              "submission_cleanup_ok label={} path={}",
              label,
              path.to_string_lossy()
            ),
          );
          Ok(())
        }
        Err(err) => Err(format!(
          "清理{}失败: {}",
          label,
          err.to_string()
        )),
      }
    }
    Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
    Err(err) => Err(format!(
      "读取{}路径失败: {}",
      label,
      err.to_string()
    )),
  }
}

fn cleanup_empty_parent_dir(log_path: &PathBuf, path: &Path) {
  let parent = match path.parent() {
    Some(dir) => dir,
    None => return,
  };
  let mut entries = match fs::read_dir(parent) {
    Ok(entries) => entries,
    Err(err) if err.kind() == ErrorKind::NotFound => return,
    Err(err) => {
      append_log(
        log_path,
        &format!(
          "submission_delete_parent_scan_fail path={} err={}",
          parent.to_string_lossy(),
          err
        ),
      );
      return;
    }
  };
  if entries.next().is_some() {
    return;
  }
  match fs::remove_dir(parent) {
    Ok(()) => {
      append_log(
        log_path,
        &format!(
          "submission_delete_parent_ok path={}",
          parent.to_string_lossy()
        ),
      );
    }
    Err(err) => {
      append_log(
        log_path,
        &format!(
          "submission_delete_parent_fail path={} err={}",
          parent.to_string_lossy(),
          err
        ),
      );
    }
  }
}

#[tauri::command]
pub async fn submission_execute(
  state: State<'_, AppState>,
  task_id: String,
) -> Result<ApiResponse<String>, String> {
  append_log(
    &state.app_log_path,
    &format!("submission_execute_request task_id={}", task_id),
  );
  let context = SubmissionContext::new(&state);
  start_submission_workflow(
    context.db.clone(),
    context.app_log_path.clone(),
    context.edit_upload_state.clone(),
    task_id,
  );

  Ok(ApiResponse::success("Workflow started".to_string()))
}

#[tauri::command]
pub async fn submission_integrated_execute(
  state: State<'_, AppState>,
  task_id: String,
) -> Result<ApiResponse<String>, String> {
  let context = SubmissionContext::new(&state);
  let task_id = task_id.trim().to_string();
  if task_id.is_empty() {
    return Ok(ApiResponse::error("任务ID不能为空"));
  }

  let status = match load_task_status(&context, &task_id) {
    Ok(status) => status,
    Err(err) => return Ok(ApiResponse::error(format!("读取任务状态失败: {}", err))),
  };
  if status != "FAILED" {
    return Ok(ApiResponse::error("当前任务状态不支持一键投稿"));
  }

  let stats = match load_integrated_download_stats(&context, &task_id) {
    Ok(value) => value,
    Err(err) => return Ok(ApiResponse::error(format!("读取下载状态失败: {}", err))),
  };

  let stats = match stats {
    Some(value) => value,
    None => return Ok(ApiResponse::error("该任务未关联下载记录")),
  };

  if stats.failed > 0 {
    return Ok(ApiResponse::error("存在下载失败的分P，请先重试下载"));
  }
  if stats.completed != stats.total {
    return Ok(ApiResponse::error("仍有分P下载未完成"));
  }


  if let Ok(Some(workflow_status)) = load_workflow_status(&context, &task_id) {
    if workflow_status.status == "RUNNING" {
      return Ok(ApiResponse::error("工作流执行中"));
    }
  }

  start_submission_workflow(
    context.db.clone(),
    context.app_log_path.clone(),
    context.edit_upload_state.clone(),
    task_id,
  );
  Ok(ApiResponse::success("Workflow started".to_string()))
}

#[tauri::command]
pub async fn submission_upload_execute(
  state: State<'_, AppState>,
  task_id: String,
) -> Result<ApiResponse<String>, String> {
  let context = SubmissionContext::new(&state);
  let task_id = task_id.trim().to_string();
  if task_id.is_empty() {
    return Ok(ApiResponse::error("任务ID不能为空"));
  }
  let status = match load_task_status(&context, &task_id) {
    Ok(status) => status,
    Err(err) => return Ok(ApiResponse::error(format!("读取任务状态失败: {}", err))),
  };
  append_log(
    &state.app_log_path,
    &format!("submission_upload_request task_id={} status={}", task_id, status),
  );
  if status == "UPLOADING" {
    append_log(
      &state.app_log_path,
      &format!("submission_upload_reject task_id={} reason=uploading", task_id),
    );
    return Ok(ApiResponse::error("任务正在投稿中"));
  }
  if status != "WAITING_UPLOAD" && status != "FAILED" {
    append_log(
      &state.app_log_path,
      &format!(
        "submission_upload_reject task_id={} reason=invalid_status status={}",
        task_id, status
      ),
    );
    return Ok(ApiResponse::error("任务状态不支持投稿"));
  }

  if let Err(err) = update_submission_status(&context, &task_id, "WAITING_UPLOAD") {
    return Ok(ApiResponse::error(format!("提交到投稿队列失败: {}", err)));
  }

  Ok(ApiResponse::success("投稿任务已加入队列".to_string()))
}

#[tauri::command]
pub async fn submission_queue_prioritize(
  state: State<'_, AppState>,
  task_id: String,
) -> Result<ApiResponse<String>, String> {
  let context = SubmissionContext::new(&state);
  let task_id = task_id.trim().to_string();
  if task_id.is_empty() {
    return Ok(ApiResponse::error("任务ID不能为空"));
  }
  let status = match load_task_status(&context, &task_id) {
    Ok(status) => status,
    Err(err) => return Ok(ApiResponse::error(format!("读取任务状态失败: {}", err))),
  };
  if status != "WAITING_UPLOAD" {
    return Ok(ApiResponse::error("仅支持对投稿队列中的任务进行优先投稿"));
  }
  let now = now_rfc3339();
  if let Err(err) = context.db.with_conn(|conn| {
    conn.execute(
      "UPDATE submission_task SET priority = 1, updated_at = ?1 WHERE task_id = ?2",
      (&now, &task_id),
    )?;
    Ok(())
  }) {
    return Ok(ApiResponse::error(format!("设置优先投稿失败: {}", err)));
  }
  Ok(ApiResponse::success("已设置为优先投稿".to_string()))
}

#[tauri::command]
pub async fn submission_retry_segment_upload(
  state: State<'_, AppState>,
  segment_id: String,
) -> Result<ApiResponse<String>, String> {
  let context = SubmissionContext::new(&state);
  let segment_id = segment_id.trim().to_string();
  if segment_id.is_empty() {
    return Ok(ApiResponse::error("分段ID不能为空"));
  }
  let segment = match load_output_segment_by_id(&context, &segment_id) {
    Ok(Some(segment)) => segment,
    Ok(None) => return Ok(ApiResponse::error("未找到分段信息")),
    Err(err) => return Ok(ApiResponse::error(err)),
  };
  if segment.upload_status == "SUCCESS" {
    return Ok(ApiResponse::success("分段已上传成功".to_string()));
  }
  let status = match load_task_status(&context, &segment.task_id) {
    Ok(status) => status,
    Err(err) => return Ok(ApiResponse::error(format!("读取任务状态失败: {}", err))),
  };
  if status == "UPLOADING" {
    return Ok(ApiResponse::error("任务正在投稿中，请稍后重试"));
  }

  let upload_context = UploadContext::new(&state);
  let auth = match load_auth_or_refresh(&upload_context, "submission_retry_segment").await {
    Ok(auth) => auth,
    Err(err) => return Ok(ApiResponse::error(err)),
  };

  update_segment_upload_status(&context, &segment_id, "UPLOADING")?;
  let client = Client::new();
  let result = upload_segment_with_retry(
    &context,
    &upload_context,
    &client,
    &auth,
    &segment_id,
    upload_context.app_log_path.as_ref(),
    UPLOAD_SEGMENT_RETRY_LIMIT,
  )
  .await;

  match result {
    Ok(upload_result) => {
      update_segment_upload_result(
        &context,
        &segment_id,
        "SUCCESS",
        Some(upload_result.cid),
        Some(upload_result.filename),
      )?;
      let remaining = count_incomplete_segments(&context, &segment.task_id)?;
      if remaining == 0 {
        if let Ok(status) = load_task_status(&context, &segment.task_id) {
          if status == "FAILED" {
            update_submission_status(&context, &segment.task_id, "WAITING_UPLOAD")?;
          }
        }
      }
      Ok(ApiResponse::success("分段上传成功".to_string()))
    }
    Err(err) => {
      update_segment_upload_status(&context, &segment_id, "FAILED")?;
      Ok(ApiResponse::error(err))
    }
  }
}

#[tauri::command]
pub fn workflow_status(
  state: State<'_, AppState>,
  task_id: String,
) -> ApiResponse<Option<WorkflowStatusRecord>> {
  let context = SubmissionContext::new(&state);
  match load_workflow_status(&context, &task_id) {
    Ok(status) => ApiResponse::success(status),
    Err(err) => ApiResponse::error(format!("Failed to load workflow status: {}", err)),
  }
}

#[tauri::command]
pub fn workflow_pause(state: State<'_, AppState>, task_id: String) -> ApiResponse<String> {
  let context = SubmissionContext::new(&state);
  match load_workflow_status(&context, &task_id) {
    Ok(Some(status)) => {
      if status.status != "RUNNING" {
        return ApiResponse::error("当前工作流无法暂停");
      }
      match set_workflow_instance_status(&context, &task_id, "PAUSED") {
        Ok(()) => ApiResponse::success("Paused".to_string()),
        Err(err) => ApiResponse::error(err),
      }
    }
    Ok(None) => ApiResponse::error("未找到工作流实例"),
    Err(err) => ApiResponse::error(err),
  }
}

#[tauri::command]
pub fn workflow_resume(state: State<'_, AppState>, task_id: String) -> ApiResponse<String> {
  let context = SubmissionContext::new(&state);
  match load_workflow_status(&context, &task_id) {
    Ok(Some(status)) => {
      if status.status != "PAUSED" {
        return ApiResponse::error("当前工作流无法恢复");
      }
      match set_workflow_instance_status(&context, &task_id, "RUNNING") {
        Ok(()) => ApiResponse::success("Resumed".to_string()),
        Err(err) => ApiResponse::error(err),
      }
    }
    Ok(None) => ApiResponse::error("未找到工作流实例"),
    Err(err) => ApiResponse::error(err),
  }
}

#[tauri::command]
pub fn workflow_cancel(state: State<'_, AppState>, task_id: String) -> ApiResponse<String> {
  let context = SubmissionContext::new(&state);
  match set_workflow_instance_status(&context, &task_id, "CANCELLED") {
    Ok(()) => {
      let _ = update_submission_status(&context, &task_id, "CANCELLED");
      ApiResponse::success("Cancelled".to_string())
    }
    Err(err) => ApiResponse::error(err),
  }
}

fn load_tasks(
  context: &SubmissionContext,
  status: Option<String>,
  page: i64,
  page_size: i64,
  query: Option<String>,
) -> Result<PaginatedSubmissionTasks, String> {
  context
    .db
    .with_conn(|conn| {
      let query = query.and_then(|value| {
        let trimmed = value.trim().to_string();
        if trimmed.is_empty() {
          None
        } else {
          Some(trimmed)
        }
      });
      let like_query = query.as_ref().map(|value| format!("%{}%", value));
      let total = match (status.as_ref(), like_query.as_ref()) {
        (Some(status), Some(pattern)) => conn.query_row(
          "SELECT COUNT(*) FROM submission_task WHERE status = ?1 AND (title LIKE ?2 COLLATE NOCASE OR bvid LIKE ?2 COLLATE NOCASE)",
          (status, pattern),
          |row| row.get(0),
        )?,
        (Some(status), None) => conn.query_row(
          "SELECT COUNT(*) FROM submission_task WHERE status = ?1",
          [status],
          |row| row.get(0),
        )?,
        (None, Some(pattern)) => conn.query_row(
          "SELECT COUNT(*) FROM submission_task WHERE title LIKE ?1 COLLATE NOCASE OR bvid LIKE ?1 COLLATE NOCASE",
          [pattern],
          |row| row.get(0),
        )?,
        (None, None) => conn.query_row("SELECT COUNT(*) FROM submission_task", [], |row| row.get(0))?,
      };
      let offset = (page - 1).saturating_mul(page_size);
      let order_by = "ORDER BY \
        CASE \
          WHEN st.status <> 'COMPLETED' THEN 0 \
          WHEN st.status = 'COMPLETED' AND (wi.status IS NULL OR wi.status <> 'COMPLETED') THEN 1 \
          WHEN st.status = 'COMPLETED' AND wi.status = 'COMPLETED' \
               AND (st.remote_state IS NULL OR st.remote_state = -30 OR st.remote_state IN (-2, -4)) THEN 2 \
          ELSE 3 \
        END, \
        CASE \
          WHEN st.status = 'COMPLETED' AND (wi.status IS NULL OR wi.status <> 'COMPLETED') THEN \
            CASE wi.current_step \
              WHEN 'CLIPPING' THEN 0 \
              WHEN 'MERGING' THEN 1 \
              WHEN 'SEGMENTING' THEN 2 \
              ELSE 9 \
            END \
          ELSE 9 \
        END, \
        CASE \
          WHEN st.status = 'COMPLETED' AND wi.status = 'COMPLETED' THEN \
            CASE \
              WHEN st.remote_state IS NULL OR st.remote_state = -30 THEN 0 \
              WHEN st.remote_state IN (-2, -4) THEN 1 \
              ELSE 2 \
            END \
          ELSE 9 \
        END, \
        CASE \
          WHEN st.status = 'WAITING_UPLOAD' THEN st.priority \
          ELSE 0 \
        END DESC, \
        st.created_at DESC";
      let sql = match (status.as_ref(), like_query.as_ref()) {
        (Some(_), Some(_)) => format!(
          "SELECT st.task_id, st.status, st.priority, st.title, st.description, st.cover_url, st.partition_id, st.tags, st.topic_id, st.mission_id, st.activity_title, st.video_type, st.collection_id, st.bvid, st.aid, st.remote_state, st.reject_reason, st.created_at, st.updated_at, st.segment_prefix, st.baidu_sync_enabled, st.baidu_sync_path, st.baidu_sync_filename, \
                  CASE WHEN EXISTS (SELECT 1 FROM task_relations tr WHERE tr.submission_task_id = st.task_id) THEN 1 ELSE 0 END, \
                  wi.status, wi.current_step, wi.progress \
           FROM submission_task st \
           LEFT JOIN workflow_instances wi ON wi.task_id = st.task_id \
           WHERE st.status = ?1 AND (st.title LIKE ?2 COLLATE NOCASE OR st.bvid LIKE ?2 COLLATE NOCASE) {} LIMIT ?3 OFFSET ?4",
          order_by
        ),
        (Some(_), None) => format!(
          "SELECT st.task_id, st.status, st.priority, st.title, st.description, st.cover_url, st.partition_id, st.tags, st.topic_id, st.mission_id, st.activity_title, st.video_type, st.collection_id, st.bvid, st.aid, st.remote_state, st.reject_reason, st.created_at, st.updated_at, st.segment_prefix, st.baidu_sync_enabled, st.baidu_sync_path, st.baidu_sync_filename, \
                  CASE WHEN EXISTS (SELECT 1 FROM task_relations tr WHERE tr.submission_task_id = st.task_id) THEN 1 ELSE 0 END, \
                  wi.status, wi.current_step, wi.progress \
           FROM submission_task st \
           LEFT JOIN workflow_instances wi ON wi.task_id = st.task_id \
           WHERE st.status = ?1 {} LIMIT ?2 OFFSET ?3",
          order_by
        ),
        (None, Some(_)) => format!(
          "SELECT st.task_id, st.status, st.priority, st.title, st.description, st.cover_url, st.partition_id, st.tags, st.topic_id, st.mission_id, st.activity_title, st.video_type, st.collection_id, st.bvid, st.aid, st.remote_state, st.reject_reason, st.created_at, st.updated_at, st.segment_prefix, st.baidu_sync_enabled, st.baidu_sync_path, st.baidu_sync_filename, \
                  CASE WHEN EXISTS (SELECT 1 FROM task_relations tr WHERE tr.submission_task_id = st.task_id) THEN 1 ELSE 0 END, \
                  wi.status, wi.current_step, wi.progress \
           FROM submission_task st \
           LEFT JOIN workflow_instances wi ON wi.task_id = st.task_id \
           WHERE st.title LIKE ?1 COLLATE NOCASE OR st.bvid LIKE ?1 COLLATE NOCASE {} LIMIT ?2 OFFSET ?3",
          order_by
        ),
        (None, None) => format!(
          "SELECT st.task_id, st.status, st.priority, st.title, st.description, st.cover_url, st.partition_id, st.tags, st.topic_id, st.mission_id, st.activity_title, st.video_type, st.collection_id, st.bvid, st.aid, st.remote_state, st.reject_reason, st.created_at, st.updated_at, st.segment_prefix, st.baidu_sync_enabled, st.baidu_sync_path, st.baidu_sync_filename, \
                  CASE WHEN EXISTS (SELECT 1 FROM task_relations tr WHERE tr.submission_task_id = st.task_id) THEN 1 ELSE 0 END, \
                  wi.status, wi.current_step, wi.progress \
           FROM submission_task st \
           LEFT JOIN workflow_instances wi ON wi.task_id = st.task_id \
           {} LIMIT ?1 OFFSET ?2",
          order_by
        ),
      };

      let mut stmt = conn.prepare(&sql)?;
      let rows = match (status, like_query) {
        (Some(status), Some(pattern)) => {
          stmt.query_map((status, pattern, page_size, offset), map_submission_task)?
        }
        (Some(status), None) => stmt.query_map((status, page_size, offset), map_submission_task)?,
        (None, Some(pattern)) => {
          stmt.query_map((pattern, page_size, offset), map_submission_task)?
        }
        (None, None) => stmt.query_map((page_size, offset), map_submission_task)?,
      };

      let list = rows.collect::<Result<Vec<_>, _>>()?;
      Ok(PaginatedSubmissionTasks {
        items: list,
        total,
        page,
        page_size,
      })
    })
    .map_err(|err| err.to_string())
}

fn map_submission_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<SubmissionTaskRecord> {
  let has_integrated_downloads: i64 = row.get(23)?;
  let workflow_status = row.get::<_, Option<String>>(24)?;
  let workflow_step = row.get::<_, Option<String>>(25)?;
  let workflow_progress: Option<f64> = row.get(26)?;
  let workflow_status = workflow_status.map(|status| WorkflowStatusRecord {
    status,
    current_step: workflow_step,
    progress: workflow_progress.unwrap_or(0.0),
  });

  Ok(SubmissionTaskRecord {
    task_id: row.get(0)?,
    status: row.get(1)?,
    priority: row.get::<_, i64>(2)? != 0,
    title: row.get(3)?,
    description: row.get(4)?,
    cover_url: row.get(5)?,
    partition_id: row.get(6)?,
    tags: row.get(7)?,
    topic_id: row.get(8)?,
    mission_id: row.get(9)?,
    activity_title: row.get(10)?,
    video_type: row.get(11)?,
    collection_id: row.get(12)?,
    bvid: row.get(13)?,
    aid: row.get(14)?,
    remote_state: row.get(15)?,
    reject_reason: row.get(16)?,
    created_at: row.get(17)?,
    updated_at: row.get(18)?,
    segment_prefix: row.get(19)?,
    baidu_sync_enabled: row.get::<_, i64>(20)? != 0,
    baidu_sync_path: row.get(21)?,
    baidu_sync_filename: row.get(22)?,
    has_integrated_downloads: has_integrated_downloads != 0,
    workflow_status,
  })
}

fn load_task_detail(
  context: &SubmissionContext,
  task_id: &str,
) -> Result<SubmissionTaskDetail, String> {
  if let Err(err) = ensure_merged_video_records(context, task_id) {
    append_log(
      &context.app_log_path,
      &format!(
        "submission_merge_history_sync_fail task_id={} err={}",
        task_id, err
      ),
    );
  }
  context
    .db
    .with_conn(|conn| {
      let task = conn.query_row(
        "SELECT st.task_id, st.status, st.priority, st.title, st.description, st.cover_url, st.partition_id, st.tags, st.topic_id, st.mission_id, st.activity_title, st.video_type, st.collection_id, st.bvid, st.aid, st.remote_state, st.reject_reason, st.created_at, st.updated_at, st.segment_prefix, st.baidu_sync_enabled, st.baidu_sync_path, st.baidu_sync_filename, \
                CASE WHEN EXISTS (SELECT 1 FROM task_relations tr WHERE tr.submission_task_id = st.task_id) THEN 1 ELSE 0 END, \
                wi.status, wi.current_step, wi.progress \
         FROM submission_task st \
         LEFT JOIN workflow_instances wi ON wi.task_id = st.task_id \
         WHERE st.task_id = ?1",
        [task_id],
        map_submission_task,
      )?;

      let mut source_stmt = conn.prepare(
        "SELECT id, task_id, source_file_path, sort_order, start_time, end_time FROM task_source_video WHERE task_id = ?1 ORDER BY sort_order ASC",
      )?;
      let source_videos = source_stmt
        .query_map([task_id], |row| {
          Ok(TaskSourceVideoRecord {
            id: row.get(0)?,
            task_id: row.get(1)?,
            source_file_path: row.get(2)?,
            sort_order: row.get(3)?,
            start_time: row.get(4)?,
            end_time: row.get(5)?,
          })
        })?
        .collect::<Result<Vec<_>, _>>()?;

      let mut segment_stmt = conn.prepare(
        "SELECT segment_id, task_id, merged_id, part_name, segment_file_path, part_order, upload_status, cid, file_name, \
                upload_progress, upload_uploaded_bytes, upload_total_bytes, upload_session_id, upload_biz_id, \
                upload_endpoint, upload_auth, upload_uri, upload_chunk_size, upload_last_part_index \
         FROM task_output_segment WHERE task_id = ?1 ORDER BY part_order ASC",
      )?;
      let output_segments = segment_stmt
        .query_map([task_id], |row| {
          Ok(TaskOutputSegmentRecord {
            segment_id: row.get(0)?,
            task_id: row.get(1)?,
            merged_id: row.get(2)?,
            part_name: row.get(3)?,
            segment_file_path: row.get(4)?,
            part_order: row.get(5)?,
            upload_status: row.get(6)?,
            cid: row.get(7)?,
            file_name: row.get(8)?,
            upload_progress: row.get(9)?,
            upload_uploaded_bytes: row.get(10)?,
            upload_total_bytes: row.get(11)?,
            upload_session_id: row.get(12)?,
            upload_biz_id: row.get(13)?,
            upload_endpoint: row.get(14)?,
            upload_auth: row.get(15)?,
            upload_uri: row.get(16)?,
            upload_chunk_size: row.get(17)?,
            upload_last_part_index: row.get(18)?,
          })
        })?
        .collect::<Result<Vec<_>, _>>()?;

      let mut merged_stmt = conn.prepare(
        "SELECT id, task_id, file_name, video_path, remote_dir, remote_name, duration, status, \
                upload_progress, upload_uploaded_bytes, upload_total_bytes, upload_cid, upload_file_name, \
                upload_session_id, upload_biz_id, upload_endpoint, upload_auth, upload_uri, upload_chunk_size, \
                upload_last_part_index, create_time, update_time \
         FROM merged_video WHERE task_id = ?1 ORDER BY create_time DESC, id DESC",
      )?;
      let merged_videos = merged_stmt
        .query_map([task_id], |row| {
          Ok(MergedVideoRecord {
            id: row.get(0)?,
            task_id: row.get(1)?,
            file_name: row.get(2)?,
            video_path: row.get(3)?,
            remote_dir: row.get(4)?,
            remote_name: row.get(5)?,
            duration: row.get(6)?,
            status: row.get(7)?,
            upload_progress: row.get(8)?,
            upload_uploaded_bytes: row.get(9)?,
            upload_total_bytes: row.get(10)?,
            upload_cid: row.get(11)?,
            upload_file_name: row.get(12)?,
            upload_session_id: row.get(13)?,
            upload_biz_id: row.get(14)?,
            upload_endpoint: row.get(15)?,
            upload_auth: row.get(16)?,
            upload_uri: row.get(17)?,
            upload_chunk_size: row.get(18)?,
            upload_last_part_index: row.get(19)?,
            create_time: row.get(20)?,
            update_time: row.get(21)?,
          })
        })?
        .collect::<Result<Vec<_>, _>>()?;

      let workflow_config_raw: Option<String> = conn
        .query_row(
          "SELECT wc.configuration_data FROM workflow_instances wi \
           JOIN workflow_configurations wc ON wi.configuration_id = wc.config_id \
           WHERE wi.task_id = ?1 ORDER BY wi.created_at DESC LIMIT 1",
          [task_id],
          |row| row.get(0),
        )
        .ok();
      let workflow_config =
        workflow_config_raw.and_then(|value| serde_json::from_str::<Value>(&value).ok());

      Ok(SubmissionTaskDetail {
        task,
        source_videos,
        output_segments,
        merged_videos,
        workflow_config,
      })
    })
    .map_err(|err| err.to_string())
}

pub fn create_workflow_instance_for_task_with_type(
  db: &Db,
  task_id: &str,
  config: &Value,
  workflow_type: &str,
) -> Result<(String, String), String> {
  let config_json = serde_json::to_string(config).map_err(|err| err.to_string())?;
  let now = now_rfc3339();
  let instance_id = uuid::Uuid::new_v4().to_string();

  db.with_conn(|conn| {
      conn.execute(
        "INSERT INTO workflow_configurations (config_name, config_type, workflow_type, configuration_data, description, is_active, version, created_at, updated_at) \
         VALUES (?1, 'INSTANCE_SPECIFIC', ?2, ?3, NULL, 1, 1, ?4, ?5)",
        (format!("workflow_{}", task_id), workflow_type, config_json, &now, &now),
      )?;

      let config_id = conn.last_insert_rowid();

      conn.execute(
        "INSERT INTO workflow_instances (instance_id, task_id, workflow_type, status, current_step, progress, configuration_id, created_at, updated_at) \
         VALUES (?1, ?2, ?3, 'PENDING', NULL, 0, ?4, ?5, ?6)",
        (&instance_id, task_id, workflow_type, config_id, &now, &now),
      )?;

      Ok(())
    })
    .map_err(|err| format!("Failed to create workflow: {}", err))?;

  Ok((instance_id, "PENDING".to_string()))
}

pub fn create_workflow_instance_for_task(
  db: &Db,
  task_id: &str,
  config: &Value,
) -> Result<(String, String), String> {
  create_workflow_instance_for_task_with_type(db, task_id, config, "VIDEO_SUBMISSION")
}

fn create_workflow_instance(
  context: &SubmissionContext,
  task_id: &str,
  config: &Value,
) -> Result<(String, String), String> {
  create_workflow_instance_for_task(context.db.as_ref(), task_id, config)
}

const SOURCE_READY_STABLE_DELAY_SECS: u64 = 2;
const SOURCE_READY_MAX_RETRIES: u32 = 30;
const SOURCE_READY_MAX_WAIT_SECS: u64 = 30;

struct SourceReadyInfo {
  source: ClipSource,
  path: String,
  size: u64,
}

fn format_timecode_seconds(seconds: f64) -> String {
  let total = if seconds.is_finite() { seconds.max(0.0) } else { 0.0 };
  let hours = (total / 3600.0).floor() as i64;
  let minutes = ((total - (hours as f64 * 3600.0)) / 60.0).floor() as i64;
  let secs = total - (hours as f64 * 3600.0) - (minutes as f64 * 60.0);
  if secs.fract().abs() < 0.001 {
    format!("{:02}:{:02}:{:02}", hours, minutes, secs.floor() as i64)
  } else {
    format!("{:02}:{:02}:{:06.3}", hours, minutes, secs)
  }
}

async fn check_sources_ready(
  context: &SubmissionContext,
  task_id: &str,
  sources: &[ClipSource],
) -> Result<Vec<ClipSource>, String> {
  let mut infos = Vec::with_capacity(sources.len());
  for source in sources {
    let path = Path::new(&source.input_path);
    let metadata =
      fs::metadata(path).map_err(|err| format!("源文件不存在 input={} err={}", source.input_path, err))?;
    let size = metadata.len();
    if size == 0 {
      return Err(format!("源文件大小为0 input={}", source.input_path));
    }
    infos.push(SourceReadyInfo {
      source: source.clone(),
      path: source.input_path.clone(),
      size,
    });
  }

  sleep(Duration::from_secs(SOURCE_READY_STABLE_DELAY_SECS)).await;
  for info in &infos {
    let metadata = fs::metadata(&info.path)
      .map_err(|err| format!("源文件不存在 input={} err={}", info.path, err))?;
    if metadata.len() != info.size {
      return Err(format!("源文件仍在写入 input={}", info.path));
    }
  }

  let mut normalized = Vec::with_capacity(infos.len());
  for info in infos {
    let duration = probe_duration_seconds(Path::new(&info.path))
      .map_err(|err| format!("源文件不可读 input={} err={}", info.path, err))?;
    let mut start = info
      .source
      .start_time
      .as_deref()
      .and_then(|value| parse_time_to_seconds(value))
      .unwrap_or(0.0);
    let end_config = info
      .source
      .end_time
      .as_deref()
      .and_then(|value| parse_time_to_seconds(value));
    let mut end = end_config.unwrap_or(duration);
    let mut reset = false;

    if end <= 0.0 {
      end = duration;
      reset = true;
    }
    if let Some(config_end) = end_config {
      if config_end > duration {
        append_log(
          &context.app_log_path,
          &format!(
            "submission_clip_time_clamp task_id={} input={} end={} duration={}",
            task_id, info.path, config_end, duration
          ),
        );
        let end_time = format_timecode_seconds(duration);
        let update_result = context.db.with_conn(|conn| {
          conn.execute(
            "UPDATE task_source_video SET end_time = ?1 WHERE task_id = ?2 AND source_file_path = ?3 AND sort_order = ?4",
            (&end_time, task_id, &info.path, info.source.order),
          )
        });
        if let Err(err) = update_result {
          append_log(
            &context.app_log_path,
            &format!(
              "submission_clip_time_update_fail task_id={} input={} err={}",
              task_id, info.path, err
            ),
          );
        }
        end = duration;
      }
    } else {
      end = duration;
    }
    if start < 0.0 || start >= end {
      start = 0.0;
      if end_config.is_none() {
        end = duration;
      }
      reset = true;
    }

    if reset {
      append_log(
        &context.app_log_path,
        &format!(
          "submission_clip_time_reset task_id={} input={} start={} end={} duration={}",
          task_id, info.path, start, end, duration
        ),
      );
    }

    let start_time = if start <= 0.0 {
      Some("00:00:00".to_string())
    } else {
      Some(format_timecode_seconds(start))
    };
    let end_time = Some(format_timecode_seconds(end));
    normalized.push(ClipSource {
      input_path: info.source.input_path,
      start_time,
      end_time,
      order: info.source.order,
    });
  }

  Ok(normalized)
}

async fn ensure_sources_ready(
  context: &SubmissionContext,
  task_id: &str,
  sources: &[ClipSource],
  workflow_instance_id: &str,
) -> Result<Vec<ClipSource>, String> {
  let mut attempt = 0;
  let mut wait_secs = SOURCE_READY_STABLE_DELAY_SECS;
  loop {
    if !is_workflow_instance_latest(context, task_id, workflow_instance_id)? {
      append_log(
        &context.app_log_path,
        &format!(
          "submission_workflow_superseded task_id={} instance_id={}",
          task_id, workflow_instance_id
        ),
      );
      return Err("WORKFLOW_SUPERSEDED".to_string());
    }
    let _ = wait_for_workflow_ready(context, task_id).await?;
    match check_sources_ready(context, task_id, sources).await {
      Ok(normalized) => return Ok(normalized),
      Err(err) => {
        if !is_workflow_instance_latest(context, task_id, workflow_instance_id)? {
          append_log(
            &context.app_log_path,
            &format!(
              "submission_workflow_superseded task_id={} instance_id={}",
              task_id, workflow_instance_id
            ),
          );
          return Err("WORKFLOW_SUPERSEDED".to_string());
        }
        attempt += 1;
        append_log(
          &context.app_log_path,
          &format!(
            "submission_sources_not_ready task_id={} attempt={} err={}",
            task_id, attempt, err
          ),
        );
        let _ = update_workflow_status(context, task_id, "VIDEO_DOWNLOADING", None, 0.0);
        let _ = update_submission_status(context, task_id, "PENDING");
        if attempt >= SOURCE_READY_MAX_RETRIES {
          let _ = update_workflow_status(context, task_id, "FAILED", None, 0.0);
          let _ = update_submission_status(context, task_id, "FAILED");
          return Err(err);
        }
        let sleep_secs = wait_secs.min(SOURCE_READY_MAX_WAIT_SECS);
        sleep(Duration::from_secs(sleep_secs)).await;
        wait_secs = (wait_secs * 2).min(SOURCE_READY_MAX_WAIT_SECS);
      }
    }
  }
}

async fn run_submission_workflow(
  context: SubmissionContext,
  task_id: String,
) -> Result<(), String> {
  let (workflow_instance_id, workflow_type, workflow_config) =
    load_latest_workflow_runtime(&context, &task_id)?
      .ok_or_else(|| "Workflow instance not found".to_string())?;
  let is_update_workflow = workflow_type == "VIDEO_UPDATE";
  let (reprocess_mode, reprocess_merged_id) = load_reprocess_metadata(workflow_config.as_ref());
  let _ = wait_for_workflow_ready(&context, &task_id).await?;

  let sources = if is_update_workflow {
    match load_update_sources(&context, &task_id)? {
      Some(update_sources) => update_sources,
      None => load_source_videos(&context, &task_id)?,
    }
  } else if reprocess_mode == ReprocessMode::Specified {
    let mut specified_sources = Vec::new();
    if let Some(merged_id) = reprocess_merged_id {
      if let Ok(bound_sources) = load_merged_source_clips(&context, &task_id, merged_id) {
        if !bound_sources.is_empty() {
          specified_sources = normalize_binding_sources(bound_sources);
        }
      }
    }
    if specified_sources.is_empty() {
      if let Some(config) = workflow_config.as_ref() {
        let update_sources = extract_update_sources_from_config(config);
        if !update_sources.is_empty() {
          specified_sources = normalize_binding_sources(update_sources);
        }
      }
    }
    if specified_sources.is_empty() {
      load_source_videos(&context, &task_id)?
    } else {
      specified_sources
    }
  } else {
    load_source_videos(&context, &task_id)?
  };
  if sources.is_empty() {
    update_submission_status(&context, &task_id, "FAILED")?;
    return Err("No source videos".to_string());
  }

  let sources = match ensure_sources_ready(&context, &task_id, &sources, &workflow_instance_id).await {
    Ok(value) => value,
    Err(err) if err == "WORKFLOW_SUPERSEDED" => return Ok(()),
    Err(err) => return Err(err),
  };
  if !is_workflow_instance_latest(&context, &task_id, &workflow_instance_id)? {
    append_log(
      &context.app_log_path,
      &format!(
        "submission_workflow_superseded task_id={} instance_id={}",
        task_id, workflow_instance_id
      ),
    );
    return Ok(());
  }
  let _ = wait_for_workflow_ready(&context, &task_id).await?;
  let _ = update_workflow_status(&context, &task_id, "RUNNING", Some("CLIPPING"), 0.0);
  update_submission_status(&context, &task_id, "CLIPPING")?;

  let base_dir = resolve_submission_base_dir(&context, &task_id);
  let workflow_dir = if is_update_workflow {
    let update_stamp = sanitize_filename(&format!("update_{}", now_rfc3339()));
    base_dir.join("updates").join(update_stamp)
  } else {
    base_dir.clone()
  };
  let clip_dir = workflow_dir.join("cut");
  let copy_decision = match decide_clip_copy(&sources) {
    Ok(decision) => decision,
    Err(err) => {
      append_log(
        &context.app_log_path,
        &format!("submission_clip_copy_check_err task_id={} err={}", task_id, err),
      );
      crate::processing::ClipCopyDecision {
        use_copy: false,
        reason: Some(format!("timestamp_probe_failed err={}", err)),
      }
    }
  };
  let use_copy = copy_decision.use_copy;
  if let Some(reason) = copy_decision.reason.as_deref() {
    append_log(
      &context.app_log_path,
      &format!(
        "submission_clip_copy_decision task_id={} use_copy={} reason={}",
        task_id, use_copy, reason
      ),
    );
  }
  append_log(
    &context.app_log_path,
    &format!(
      "submission_clip_start task_id={} sources={} use_copy={} output_dir={}",
      task_id,
      sources.len(),
      use_copy,
      clip_dir.to_string_lossy()
    ),
  );
  for source in &sources {
    append_log(
      &context.app_log_path,
      &format!(
        "submission_clip_source task_id={} order={} input={} start={} end={}",
        task_id,
        source.order,
        source.input_path,
        source.start_time.as_deref().unwrap_or(""),
        source.end_time.as_deref().unwrap_or("")
      ),
    );
  }
  let sources_clone = sources.clone();
  let clip_dir_clone = clip_dir.clone();
  let clip_outputs = match tauri::async_runtime::spawn_blocking(move || {
    clip_sources(&sources_clone, &clip_dir_clone, use_copy)
  })
  .await
  {
    Ok(Ok(outputs)) => outputs,
    Ok(Err(err)) => {
      append_log(
        &context.app_log_path,
        &format!("submission_clip_fail task_id={} err={}", task_id, err),
      );
      return Err(err);
    }
    Err(_) => {
      append_log(
        &context.app_log_path,
        &format!("submission_clip_fail task_id={} err=spawn_blocking_failed", task_id),
      );
      return Err("Failed to clip videos".to_string());
    }
  };
  append_log(
    &context.app_log_path,
    &format!(
      "submission_clip_done task_id={} outputs={} output_dir={}",
      task_id,
      clip_outputs.len(),
      clip_dir.to_string_lossy()
    ),
  );

  let _ = wait_for_workflow_ready(&context, &task_id).await?;
  save_video_clips(
    &context,
    &task_id,
    &sources,
    &clip_outputs,
    !is_update_workflow,
  )?;

  update_submission_status(&context, &task_id, "MERGING")?;
  let _ = update_workflow_status(&context, &task_id, "RUNNING", Some("MERGING"), 40.0);
  let merge_output = build_merge_output_path(&workflow_dir, &task_id);
  let merge_list_path = merge_output.with_extension("txt");
  append_log(
    &context.app_log_path,
    &format!(
      "submission_merge_start task_id={} inputs={} output={} list={} mode=concat_copy",
      task_id,
      clip_outputs.len(),
      merge_output.to_string_lossy(),
      merge_list_path.to_string_lossy()
    ),
  );
  for path in &clip_outputs {
    append_log(
      &context.app_log_path,
      &format!(
        "submission_merge_input task_id={} path={}",
        task_id,
        path.to_string_lossy()
      ),
    );
  }
  let merge_output_clone = merge_output.clone();
  tauri::async_runtime::spawn_blocking(move || merge_files(&clip_outputs, &merge_output_clone))
    .await
    .map_err(|_| "Failed to merge videos".to_string())??;
  append_log(
    &context.app_log_path,
    &format!(
      "submission_merge_done task_id={} output={}",
      task_id,
      merge_output.to_string_lossy()
    ),
  );

  let _ = wait_for_workflow_ready(&context, &task_id).await?;
  let merged_id = save_merged_video(&context, &task_id, &merge_output)?;
  append_log(
    &context.app_log_path,
    &format!(
      "submission_merge_saved task_id={} merged_id={} path={}",
      task_id,
      merged_id,
      merge_output.to_string_lossy()
    ),
  );
  if let Err(err) = save_merged_source_bindings(&context, &task_id, merged_id, &sources) {
    append_log(
      &context.app_log_path,
      &format!(
        "submission_merge_bind_sources_fail task_id={} merged_id={} err={}",
        task_id, merged_id, err
      ),
    );
  }
  if let Err(err) = baidu_sync::enqueue_submission_sync(
    context.db.as_ref(),
    context.app_log_path.as_ref(),
    &task_id,
  ) {
    append_log(
      &context.app_log_path,
      &format!("baidu_sync_enqueue_fail task_id={} err={}", task_id, err),
    );
  }

  let workflow_settings = load_workflow_settings(&context, &task_id);
  if workflow_settings.enable_segmentation {
    let _ = wait_for_workflow_ready(&context, &task_id).await?;
    update_submission_status(&context, &task_id, "SEGMENTING")?;
    let _ = update_workflow_status(&context, &task_id, "RUNNING", Some("SEGMENTING"), 70.0);
    let segment_dir = workflow_dir.join("output");
    let merge_output_segment = merge_output.clone();
    append_log(
      &context.app_log_path,
      &format!(
        "submission_segment_start task_id={} input={} output_dir={} segment_seconds={} mode=segment_copy",
        task_id,
        merge_output_segment.to_string_lossy(),
        segment_dir.to_string_lossy(),
        workflow_settings.segment_duration_seconds
      ),
    );
    let segment_dir_clone = segment_dir.clone();
    let segment_outputs = tauri::async_runtime::spawn_blocking(move || {
      segment_file(
        &merge_output_segment,
        &segment_dir_clone,
        workflow_settings.segment_duration_seconds,
      )
    })
    .await
    .map_err(|_| "Failed to segment video".to_string())??;
    append_log(
      &context.app_log_path,
      &format!(
        "submission_segment_done task_id={} outputs={} output_dir={}",
        task_id,
        segment_outputs.len(),
        segment_dir.to_string_lossy()
      ),
    );

    if is_update_workflow {
      let (existing_count, max_order) = load_output_segment_stats(&context, &task_id)?;
      let name_start_index = resolve_update_name_start_index(
        &context,
        &task_id,
        existing_count,
        workflow_settings.segment_prefix.as_deref(),
      )?;
      append_output_segments(
        &context,
        &task_id,
        &segment_outputs,
        Some(merged_id),
        workflow_settings.segment_prefix.as_deref(),
        max_order + 1,
        name_start_index,
      )?;
    } else {
      save_output_segments(
        &context,
        &task_id,
        &segment_outputs,
        Some(merged_id),
        workflow_settings.segment_prefix.as_deref(),
      )?;
    }
  }
  if is_update_workflow && !workflow_settings.enable_segmentation {
    let (existing_count, max_order) = load_output_segment_stats(&context, &task_id)?;
    let name_start_index = resolve_update_name_start_index(
      &context,
      &task_id,
      existing_count,
      workflow_settings.segment_prefix.as_deref(),
    )?;
    append_output_segments(
      &context,
      &task_id,
      &[merge_output.clone()],
      Some(merged_id),
      workflow_settings.segment_prefix.as_deref(),
      max_order + 1,
      name_start_index,
    )?;
  }

  update_submission_status(&context, &task_id, "WAITING_UPLOAD")?;
  let workflow_status = match load_integrated_download_stats(&context, &task_id)? {
    Some(stats) if stats.completed < stats.total => "VIDEO_DOWNLOADING",
    _ => "COMPLETED",
  };
  let _ = update_workflow_status(&context, &task_id, workflow_status, None, 100.0);
  Ok(())
}

pub fn start_submission_workflow(
  db: Arc<Db>,
  app_log_path: Arc<PathBuf>,
  edit_upload_state: Arc<Mutex<EditUploadState>>,
  task_id: String,
) {
  let context = SubmissionContext {
    db,
    app_log_path,
    edit_upload_state,
  };
  tauri::async_runtime::spawn(async move {
    let _ = run_submission_workflow(context, task_id).await;
  });
}

struct PreuploadInfo {
  auth: String,
  biz_id: i64,
  chunk_size: u64,
  endpoint: String,
  upos_uri: String,
}

#[derive(Clone)]
struct UploadSessionInfo {
  upload_id: String,
  biz_id: i64,
  chunk_size: u64,
  endpoint: String,
  auth: String,
  upos_uri: String,
  uploaded_bytes: u64,
  total_bytes: u64,
  last_part_index: u64,
}

struct UploadProgressSnapshot {
  uploaded_bytes: u64,
  total_bytes: u64,
  progress: f64,
  last_part_index: u64,
}

struct UploadProgressLimiter {
  last_saved_at: Instant,
  last_saved_progress: f64,
  last_saved_bytes: u64,
  initialized: bool,
}

impl UploadProgressLimiter {
  fn new() -> Self {
    Self {
      last_saved_at: Instant::now(),
      last_saved_progress: 0.0,
      last_saved_bytes: 0,
      initialized: false,
    }
  }

  fn should_persist(&self, snapshot: &UploadProgressSnapshot) -> bool {
    if !self.initialized {
      return true;
    }
    if snapshot.progress >= 100.0 {
      return true;
    }
    let elapsed = self.last_saved_at.elapsed();
    let progress_delta = snapshot.progress - self.last_saved_progress;
    let bytes_delta = snapshot.uploaded_bytes.saturating_sub(self.last_saved_bytes);
    elapsed >= Duration::from_secs(2) || progress_delta >= 1.0 || bytes_delta >= 2 * 1024 * 1024
  }

  fn mark_saved(&mut self, snapshot: &UploadProgressSnapshot) {
    self.last_saved_at = Instant::now();
    self.last_saved_progress = snapshot.progress;
    self.last_saved_bytes = snapshot.uploaded_bytes;
    self.initialized = true;
  }
}

enum UploadTarget {
  Segment(String),
  Merged(i64),
  EditSegment(String),
}

struct UploadFileResult {
  cid: i64,
  filename: String,
}

#[derive(Clone)]
struct UploadedVideoPart {
  filename: String,
  cid: i64,
  title: String,
}

struct SubmissionSubmitResult {
  bvid: String,
  aid: i64,
}

#[derive(Clone)]
struct IntegratedDownloadRecord {
  id: i64,
  download_url: String,
  bvid: Option<String>,
  aid: Option<String>,
  title: Option<String>,
  part_title: Option<String>,
  part_count: Option<i64>,
  current_part: Option<i64>,
  local_path: String,
  resolution: Option<String>,
  codec: Option<String>,
  format: Option<String>,
  cid: Option<i64>,
  content: Option<String>,
}

const MAX_PARTS_PER_SUBMISSION: usize = 100;
const MAX_PARTS_PER_BVID: usize = 200;
const SUBMISSION_EDIT_BATCH_WAIT_SECS: u64 = 10;
const SUBMISSION_EDIT_RATE_LIMIT_BASE_WAIT_SECS: u64 = 10;
const SUBMISSION_EDIT_RATE_LIMIT_MAX_WAIT_SECS: u64 = 120;
const SUBMISSION_EDIT_RATE_LIMIT_RETRY_LIMIT: u32 = 3;
const RATE_LIMIT_BASE_WAIT_SECS: u64 = 60;
const RATE_LIMIT_MAX_WAIT_SECS: u64 = 30 * 60;
const UPLOAD_SEGMENT_RETRY_LIMIT: u32 = 3;
const SUBMISSION_QUEUE_RETRY_LIMIT: u32 = 3;
const SUBMISSION_QUEUE_RETRY_BASE_DELAY_SECS: u64 = 10;
const SUBMISSION_QUEUE_RETRY_MAX_DELAY_SECS: u64 = 120;
const REMOTE_AUDIT_STATUS: &str = "is_pubing,not_pubed";
const REMOTE_DEBUG_BVID: &str = "BV1VJkFBZENQ";
const UPLOAD_RETRY_BASE_DELAY_SECS: u64 = 2;
const UPLOAD_RETRY_MAX_DELAY_SECS: u64 = 30;
const PREUPLOAD_PARSE_RETRY_BASE_SECS: u64 = 60;
const PREUPLOAD_PARSE_RETRY_MAX_SECS: u64 = 30 * 60;
const PREUPLOAD_PARSE_RETRY_LIMIT: u32 = 6;
const PREUPLOAD_MIN_INTERVAL_MS: u64 = 1000;

static PREUPLOAD_THROTTLE: OnceLock<AsyncMutex<Option<Instant>>> = OnceLock::new();

struct UploadRateLimiter {
  consecutive_406: u32,
}

impl UploadRateLimiter {
  fn new() -> Self {
    Self { consecutive_406: 0 }
  }

  fn reset(&mut self) {
    self.consecutive_406 = 0;
  }

  fn next_wait_seconds(&mut self, retry_after: Option<u64>) -> u64 {
    self.consecutive_406 = self.consecutive_406.saturating_add(1);
    if let Some(wait) = retry_after {
      if wait > 0 {
        return wait.min(RATE_LIMIT_MAX_WAIT_SECS);
      }
    }
    let exponent = self.consecutive_406.saturating_sub(1);
    let multiplier = 1u64 << exponent.min(10);
    let wait = RATE_LIMIT_BASE_WAIT_SECS.saturating_mul(multiplier);
    wait.min(RATE_LIMIT_MAX_WAIT_SECS)
  }
}

fn is_rate_limit_error(err: &str) -> bool {
  err.contains("21540") || err.contains("请求过于频繁")
}

fn upload_retry_delay_secs(attempt: u32) -> u64 {
  let exponent = attempt.saturating_sub(1);
  let multiplier = 1u64 << exponent.min(5);
  let wait = UPLOAD_RETRY_BASE_DELAY_SECS.saturating_mul(multiplier);
  wait.min(UPLOAD_RETRY_MAX_DELAY_SECS)
}

fn submission_queue_retry_delay_secs(attempt: u32) -> u64 {
  let safe_attempt = attempt.max(1);
  let exponent = safe_attempt.saturating_sub(1);
  let multiplier = 1u64 << exponent.min(10);
  let wait = SUBMISSION_QUEUE_RETRY_BASE_DELAY_SECS.saturating_mul(multiplier);
  wait.min(SUBMISSION_QUEUE_RETRY_MAX_DELAY_SECS)
}

fn preupload_parse_retry_delay_secs(attempt: u32) -> u64 {
  let exponent = attempt.saturating_sub(1);
  let multiplier = 1u64 << exponent.min(10);
  let wait = PREUPLOAD_PARSE_RETRY_BASE_SECS.saturating_mul(multiplier);
  wait.min(PREUPLOAD_PARSE_RETRY_MAX_SECS)
}

fn is_preupload_parse_error(err: &str) -> bool {
  err.contains("预上传解析失败") || err.contains("error decoding response body")
}

fn is_retryable_submission_error(err: &str) -> bool {
  let lower = err.to_lowercase();
  let keywords = [
    "预上传请求失败:",
    "上传元数据失败:",
    "上传分片失败:",
    "结束上传失败:",
    "error sending request",
    "timed out",
    "timeout",
    "connection reset",
    "connection closed",
    "connection refused",
    "broken pipe",
    "failed to resolve",
    "dns",
    "network",
    "unexpected eof",
    "tls",
    "http2",
    "os error",
  ];
  let cn_keywords = ["网络", "超时", "连接", "预上传解析失败重试次数已达上限"];
  keywords.iter().any(|keyword| lower.contains(keyword))
    || cn_keywords.iter().any(|keyword| err.contains(keyword))
}

fn upload_target_label(target: &UploadTarget) -> String {
  match target {
    UploadTarget::Segment(segment_id) => format!("segment:{}", segment_id),
    UploadTarget::Merged(merged_id) => format!("merged:{}", merged_id),
    UploadTarget::EditSegment(segment_id) => format!("edit:{}", segment_id),
  }
}

fn truncate_log_text(value: &str) -> String {
  const LIMIT: usize = 2000;
  if value.len() <= LIMIT {
    return value.to_string();
  }
  let mut truncated = value.chars().take(LIMIT).collect::<String>();
  truncated.push_str("...<truncated>");
  truncated
}

async fn wait_preupload_throttle(
  log_path: &PathBuf,
  target: &UploadTarget,
  file_name: &str,
) {
  let throttle = PREUPLOAD_THROTTLE.get_or_init(|| AsyncMutex::new(None));
  let mut last_at = throttle.lock().await;
  let now = Instant::now();
  let wait = last_at
    .and_then(|prev| prev.checked_add(Duration::from_millis(PREUPLOAD_MIN_INTERVAL_MS)))
    .and_then(|next| next.checked_duration_since(now))
    .unwrap_or_default();
  if wait > Duration::ZERO {
    append_log(
      log_path,
      &format!(
        "preupload_throttle target={} wait_ms={} file={}",
        upload_target_label(target),
        wait.as_millis(),
        file_name
      ),
    );
    sleep(wait).await;
  }
  *last_at = Some(Instant::now());
}

fn build_uploaded_parts(
  detail: &SubmissionTaskDetail,
  is_update_workflow: bool,
) -> Result<Vec<UploadedVideoPart>, String> {
  let mut parts = Vec::with_capacity(detail.output_segments.len());
  for (index, segment) in detail.output_segments.iter().enumerate() {
    if segment.upload_status != "SUCCESS" {
      return Err("存在分段未上传完成".to_string());
    }
    let cid = segment
      .cid
      .ok_or_else(|| format!("分段缺少CID segment_id={}", segment.segment_id))?;
    let has_upload_session = segment.upload_biz_id > 0
      || segment
        .upload_session_id
        .as_deref()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
      || segment
        .upload_uri
        .as_deref()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    let filename = if is_update_workflow && !has_upload_session {
      String::new()
    } else {
      segment
        .file_name
        .clone()
        .ok_or_else(|| format!("分段缺少文件名 segment_id={}", segment.segment_id))?
    };
    let title = if is_update_workflow {
      resolve_existing_part_title(&detail.task, &segment.part_name, index + 1)
    } else {
      build_part_title(detail.task.segment_prefix.as_deref(), index + 1)
    };
    parts.push(UploadedVideoPart {
      filename,
      cid,
      title,
    });
  }
  Ok(parts)
}

async fn run_submission_upload(
  context: UploadContext,
  task_id: String,
) -> Result<(), String> {
  let submission_context = SubmissionContext {
    db: context.db.clone(),
    app_log_path: context.app_log_path.clone(),
    edit_upload_state: context.edit_upload_state.clone(),
  };
  append_log(
    &context.app_log_path,
    &format!("submission_upload_start task_id={}", task_id),
  );

  let mut auth = match load_auth_or_refresh(&context, "submission_upload").await {
    Ok(auth) => auth,
    Err(err) => {
      update_submission_status(&submission_context, &task_id, "FAILED")?;
      return Err(err);
    }
  };
  let csrf = match auth.csrf.clone() {
    Some(value) => value,
    None => {
      auth = match refresh_auth(&context, "submission_upload_csrf").await {
        Ok(auth) => auth,
        Err(err) => {
          update_submission_status(&submission_context, &task_id, "FAILED")?;
          return Err(err);
        }
      };
      auth
        .csrf
        .clone()
        .ok_or_else(|| "登录信息缺少CSRF".to_string())?
    }
  };

  let detail = load_task_detail(&submission_context, &task_id)?;
  let tags = detail.task.tags.clone().unwrap_or_default();
  if tags.trim().is_empty() {
    update_submission_status(&submission_context, &task_id, "FAILED")?;
    return Err("投稿标签不能为空".to_string());
  }
  let workflow_type = load_latest_workflow_type(&submission_context, &task_id)?
    .unwrap_or_else(|| "VIDEO_SUBMISSION".to_string());
  let integrate_current_bvid = load_integrate_current_bvid(detail.workflow_config.as_ref());
  if integrate_current_bvid && detail.task.bvid.as_deref().unwrap_or("").trim().is_empty() {
    update_submission_status(&submission_context, &task_id, "FAILED")?;
    return Err("当前任务暂无BVID，无法集成投稿".to_string());
  }
  let is_update_workflow = workflow_type == "VIDEO_UPDATE" || integrate_current_bvid;
  if is_update_workflow {
    match reset_segments_without_upload_session(&submission_context, &task_id) {
      Ok(affected) => {
        if affected > 0 {
          append_log(
            &context.app_log_path,
            &format!(
              "submission_update_reset_missing_upload_session task_id={} count={}",
              task_id, affected
            ),
          );
        }
      }
      Err(err) => {
        update_submission_status(&submission_context, &task_id, "FAILED")?;
        return Err(err);
      }
    }
  }

  update_submission_status(&submission_context, &task_id, "UPLOADING")?;

  let settings = load_workflow_settings(&submission_context, &task_id);
  let upload_concurrency = load_download_settings_from_db(&submission_context.db)
    .map(|settings| settings.upload_concurrency)
    .unwrap_or(DEFAULT_UPLOAD_CONCURRENCY)
    .max(1) as usize;
  let client = Client::new();
  let mut parts: Vec<UploadedVideoPart> = Vec::new();

  if is_update_workflow || settings.enable_segmentation {
    if detail.output_segments.is_empty() {
      update_submission_status(&submission_context, &task_id, "FAILED")?;
      return Err("未找到分段文件".to_string());
    }
    let mut preupload_retry_round: u32 = 0;
    loop {
      let detail = load_task_detail(&submission_context, &task_id)?;
      if detail.output_segments.is_empty() {
        update_submission_status(&submission_context, &task_id, "FAILED")?;
        return Err("未找到分段文件".to_string());
      }
      let failed_count = detail
        .output_segments
        .iter()
        .filter(|segment| segment.upload_status == "FAILED")
        .count();
      if failed_count > 0 {
        update_submission_status(&submission_context, &task_id, "FAILED")?;
        return Err("存在分段上传失败，请重试失败分P".to_string());
      }
      let pending: Vec<(usize, String)> = detail
        .output_segments
        .iter()
        .enumerate()
        .filter(|(_, segment)| segment.upload_status != "SUCCESS")
        .map(|(index, segment)| (index, segment.segment_id.clone()))
        .collect();
      if pending.is_empty() {
        match build_uploaded_parts(&detail, is_update_workflow) {
          Ok(list) => {
            parts = list;
            break;
          }
          Err(err) => {
            update_submission_status(&submission_context, &task_id, "FAILED")?;
            return Err(err);
          }
        }
      }
      let pending_count = pending.len();
      let batch: Vec<(usize, String)> =
        pending.into_iter().take(upload_concurrency).collect();
      append_log(
        &context.app_log_path,
        &format!(
          "submission_segment_batch_start task_id={} pending={} batch={}",
          task_id,
          pending_count,
          batch.len()
        ),
      );
      for (_, segment_id) in &batch {
        update_segment_upload_status(&submission_context, segment_id, "UPLOADING")?;
      }
      let mut futures = FuturesUnordered::new();
      for (_, segment_id) in batch {
        let context_clone = submission_context.clone();
        let upload_context_clone = context.clone();
        let client_clone = client.clone();
        let auth_clone = auth.clone();
        let log_path = context.app_log_path.clone();
        futures.push(async move {
          let result = upload_segment_with_retry(
            &context_clone,
            &upload_context_clone,
            &client_clone,
            &auth_clone,
            &segment_id,
            log_path.as_ref(),
            UPLOAD_SEGMENT_RETRY_LIMIT,
          )
          .await;
          (segment_id, result)
        });
      }
      let mut has_preupload_parse_error = false;
      let mut has_other_error = false;
      let mut last_error: Option<String> = None;
      while let Some((segment_id, result)) = futures.next().await {
        match result {
          Ok(upload_result) => {
            update_segment_upload_result(
              &submission_context,
              &segment_id,
              "SUCCESS",
              Some(upload_result.cid),
              Some(upload_result.filename.clone()),
            )?;
          }
          Err(err) => {
            if is_preupload_parse_error(&err) {
              let _ = clear_upload_session(
                &submission_context,
                &UploadTarget::Segment(segment_id.clone()),
              );
              update_segment_upload_status(&submission_context, &segment_id, "PENDING")?;
              has_preupload_parse_error = true;
            } else {
              update_segment_upload_status(&submission_context, &segment_id, "FAILED")?;
              has_other_error = true;
            }
            if last_error.is_none() {
              last_error = Some(err.clone());
            }
            append_log(
              &context.app_log_path,
              &format!(
                "submission_segment_upload_fail segment_id={} err={}",
                segment_id, err
              ),
            );
          }
        }
      }
      if has_other_error {
        let error_message = last_error
          .unwrap_or_else(|| "存在分段上传失败，请重试失败分P".to_string());
        update_submission_status(&submission_context, &task_id, "FAILED")?;
        return Err(error_message);
      }
      if has_preupload_parse_error {
        preupload_retry_round = preupload_retry_round.saturating_add(1);
        if preupload_retry_round > PREUPLOAD_PARSE_RETRY_LIMIT {
          update_submission_status(&submission_context, &task_id, "FAILED")?;
          return Err("预上传解析失败重试次数已达上限".to_string());
        }
        let wait_secs = preupload_parse_retry_delay_secs(preupload_retry_round);
        append_log(
          &context.app_log_path,
          &format!(
            "submission_segment_preupload_retry task_id={} wait_secs={} round={}",
            task_id, wait_secs, preupload_retry_round
          ),
        );
        sleep(Duration::from_secs(wait_secs)).await;
      } else {
        preupload_retry_round = 0;
      }
    }
  } else {
    let merged = load_latest_merged_video(&submission_context, &task_id)?;
    let Some(merged) = merged else {
      update_submission_status(&submission_context, &task_id, "FAILED")?;
      return Err("未找到合并视频".to_string());
    };
    let merged_path = merged.video_path.as_deref().unwrap_or("").to_string();
    if merged_path.trim().is_empty() {
      update_submission_status(&submission_context, &task_id, "FAILED")?;
      return Err("合并视频路径为空".to_string());
    }
    let target = UploadTarget::Merged(merged.id);
    let resume_session = build_upload_session_from_merged(&merged);
    let mut current_auth = auth.clone();
    let result = loop {
      match upload_single_file(
        &submission_context,
        &target,
        &client,
        &current_auth,
        Path::new(&merged_path),
        &context.app_log_path,
        resume_session.clone(),
      )
      .await
      {
        Ok(result) => break Ok(result),
        Err(err) => {
          if is_auth_error(&err) {
            match refresh_auth(&context, "upload_merged").await {
              Ok(auth) => {
                current_auth = auth;
                continue;
              }
              Err(refresh_err) => break Err(refresh_err),
            }
          }
          break Err(err);
        }
      }
    }?;
    update_merged_upload_result(
      &submission_context,
      merged.id,
      Some(result.cid),
      Some(result.filename.clone()),
    )?;
    parts.push(UploadedVideoPart {
      filename: result.filename,
      cid: result.cid,
      title: build_part_title(detail.task.segment_prefix.as_deref(), 1),
    });
  }

  if parts.is_empty() {
    update_submission_status(&submission_context, &task_id, "FAILED")?;
    return Err("投稿文件为空".to_string());
  }

  if !is_update_workflow && parts.len() > MAX_PARTS_PER_BVID {
    update_submission_status(&submission_context, &task_id, "FAILED")?;
    append_log(
      &context.app_log_path,
      &format!(
        "submission_upload_part_limit_exceeded task_id={} parts={}",
        task_id,
        parts.len()
      ),
    );
    return Err("Bilibili 不支持单 BV 超过 200 分P，请调整分段或拆分投稿".to_string());
  }

  if is_update_workflow {
    let mut aid = detail.task.aid.unwrap_or(0);
    if aid <= 0 {
      let bvid = detail.task.bvid.clone().unwrap_or_default();
      aid = fetch_aid_with_refresh(&context, &auth, &bvid)
        .await
        .unwrap_or(0);
      if aid > 0 {
        let _ = update_submission_aid(&submission_context, &task_id, aid);
      }
    }
    if aid <= 0 {
      update_submission_status(&submission_context, &task_id, "FAILED")?;
      return Err("无法获取AID，无法更新".to_string());
    }
    let missing_filename_count = parts
      .iter()
      .filter(|part| part.filename.trim().is_empty())
      .count();
    if missing_filename_count > 0 {
      append_log(
        &context.app_log_path,
        &format!(
          "submission_update_missing_filename task_id={} count={}",
          task_id, missing_filename_count
        ),
      );
      update_submission_status(&submission_context, &task_id, "FAILED")?;
      return Err("存在分段缺少上传信息，请重新上传".to_string());
    }
    let submit_result =
      submit_video_update_in_batches(&context, &auth, &detail.task, &parts, aid, &csrf).await;
    match submit_result {
      Ok(()) => {
        update_submission_status(&submission_context, &task_id, "COMPLETED")?;
        append_log(
          &context.app_log_path,
          &format!(
            "submission_update_ok task_id={} bvid={} aid={}",
            task_id,
            detail.task.bvid.as_deref().unwrap_or(""),
            aid
          ),
        );
        Ok(())
      }
      Err(err) => {
        update_submission_status(&submission_context, &task_id, "FAILED")?;
        append_log(
          &context.app_log_path,
          &format!("submission_update_submit_fail task_id={} err={}", task_id, err),
        );
        Err(err)
      }
    }
  } else {
    let submit_result = submit_video_in_batches(&context, &auth, &detail.task, &parts, &csrf).await;
    match submit_result {
      Ok(result) => {
        update_submission_bvid_and_aid(&submission_context, &task_id, &result.bvid, result.aid)?;
        if let Some(collection_id) = detail.task.collection_id {
          if collection_id > 0 {
            let cid = parts.first().map(|item| item.cid).unwrap_or(0);
            let add_result = add_video_to_collection_with_refresh(
              &context,
              &auth,
              &detail.task.title,
              collection_id,
              result.aid,
              cid,
              &csrf,
            )
            .await;
            if let Err(err) = add_result {
              if is_collection_not_found_error(&err) {
                append_log(
                  &context.app_log_path,
                  &format!(
                    "submission_collection_skip task_id={} collection_id={} err={}",
                    task_id, collection_id, err
                  ),
                );
              } else {
                update_submission_status(&submission_context, &task_id, "FAILED")?;
                append_log(
                  &context.app_log_path,
                  &format!(
                    "submission_collection_fail task_id={} collection_id={} err={}",
                    task_id, collection_id, err
                  ),
                );
                return Err(err);
              }
            }
          }
        }
        update_submission_status(&submission_context, &task_id, "COMPLETED")?;
        append_log(
          &context.app_log_path,
          &format!(
            "submission_upload_ok task_id={} bvid={} aid={}",
            task_id, result.bvid, result.aid
          ),
        );
        Ok(())
      }
      Err(err) => {
        update_submission_status(&submission_context, &task_id, "FAILED")?;
        append_log(
          &context.app_log_path,
          &format!("submission_upload_submit_fail task_id={} err={}", task_id, err),
        );
        Err(err)
      }
    }
  }
}

async fn submission_queue_loop(context: SubmissionQueueContext) {
  let submission_context = SubmissionContext {
    db: context.db.clone(),
    app_log_path: context.app_log_path.clone(),
    edit_upload_state: context.edit_upload_state.clone(),
  };
  loop {
    let task_id = match load_next_queued_task(&submission_context) {
      Ok(task_id) => task_id,
      Err(err) => {
        append_log(
          &context.app_log_path,
          &format!("submission_queue_load_fail err={}", err),
        );
        sleep(Duration::from_secs(2)).await;
        continue;
      }
    };
    let Some(task_id) = task_id else {
      sleep(Duration::from_secs(2)).await;
      continue;
    };
    append_log(
      &context.app_log_path,
      &format!("submission_queue_pick task_id={}", task_id),
    );
    let upload_context = UploadContext {
      db: context.db.clone(),
      bilibili: context.bilibili.clone(),
      login_store: context.login_store.clone(),
      app_log_path: context.app_log_path.clone(),
      edit_upload_state: context.edit_upload_state.clone(),
    };
    let mut queue_retry_round: u32 = 0;
    loop {
      let result = run_submission_upload(upload_context.clone(), task_id.clone()).await;
      match result {
        Ok(()) => break,
        Err(err) => {
          append_log(
            &context.app_log_path,
            &format!("submission_queue_upload_fail task_id={} err={}", task_id, err),
          );
          if !is_retryable_submission_error(&err) {
            break;
          }
          if let Err(reset_err) = reset_failed_segments_to_pending(&submission_context, &task_id) {
            append_log(
              &context.app_log_path,
              &format!(
                "submission_queue_retry_reset_fail task_id={} err={}",
                task_id, reset_err
              ),
            );
          }
          if let Err(status_err) =
            update_submission_status(&submission_context, &task_id, "WAITING_UPLOAD")
          {
            append_log(
              &context.app_log_path,
              &format!(
                "submission_queue_retry_status_fail task_id={} err={}",
                task_id, status_err
              ),
            );
          }
          queue_retry_round = queue_retry_round.saturating_add(1);
          if queue_retry_round >= SUBMISSION_QUEUE_RETRY_LIMIT {
            let has_other =
              has_other_queued_tasks(&submission_context, &task_id).unwrap_or(false);
            append_log(
              &context.app_log_path,
              &format!(
                "submission_queue_retry_threshold task_id={} round={} has_other={}",
                task_id, queue_retry_round, has_other
              ),
            );
            if has_other {
              if let Err(status_err) =
                update_submission_status(&submission_context, &task_id, "WAITING_UPLOAD")
              {
                append_log(
                  &context.app_log_path,
                  &format!(
                    "submission_queue_retry_move_tail_fail task_id={} err={}",
                    task_id, status_err
                  ),
                );
              }
              append_log(
                &context.app_log_path,
                &format!("submission_queue_retry_move_tail task_id={}", task_id),
              );
              break;
            }
            queue_retry_round = 0;
          }
          let wait_secs = submission_queue_retry_delay_secs(queue_retry_round);
          append_log(
            &context.app_log_path,
            &format!(
              "submission_queue_retry_wait task_id={} wait_secs={} round={}",
              task_id, wait_secs, queue_retry_round
            ),
          );
          sleep(Duration::from_secs(wait_secs)).await;
        }
      }
    }
  }
}

#[derive(Clone)]
struct RemoteAuditInfo {
  state: i64,
  reject_reason: Option<String>,
}

async fn submission_remote_refresh_loop(context: SubmissionQueueContext) {
  loop {
    let interval_minutes = load_download_settings_from_db(&context.db)
      .map(|settings| settings.submission_remote_refresh_minutes)
      .unwrap_or(DEFAULT_SUBMISSION_REMOTE_REFRESH_MINUTES)
      .max(1);
    if let Err(err) = refresh_submission_remote_state(&context).await {
      append_log(
        &context.app_log_path,
        &format!("submission_remote_refresh_fail err={}", err),
      );
    }
    sleep(Duration::from_secs((interval_minutes as u64) * 60)).await;
  }
}

async fn refresh_submission_remote_state(
  context: &SubmissionQueueContext,
) -> Result<(), String> {
  let auth = match load_auth_from_queue_context(context) {
    Ok(auth) => auth,
    Err(err) => {
      append_log(
        &context.app_log_path,
        &format!("submission_remote_refresh_skip reason={}", err),
      );
      return Ok(());
    }
  };
  let remote_map = fetch_remote_audit_map(context, &auth).await?;
  let task_bvids = load_task_bvids(context)?;
  if task_bvids.is_empty() {
    return Ok(());
  }
  let missing_bvids: Vec<String> = task_bvids
    .iter()
    .filter(|(_, bvid)| !remote_map.contains_key(bvid))
    .map(|(_, bvid)| bvid.clone())
    .collect();
  append_log(
    &context.app_log_path,
    &format!(
      "submission_remote_refresh_summary tasks={} remote_items={} missing={} status={}",
      task_bvids.len(),
      remote_map.len(),
      missing_bvids.len(),
      REMOTE_AUDIT_STATUS
    ),
  );
  if remote_map.is_empty() {
    append_log(
      &context.app_log_path,
      &format!(
        "submission_remote_refresh_remote_empty tasks={} status={}",
        task_bvids.len(),
        REMOTE_AUDIT_STATUS
      ),
    );
  } else if !missing_bvids.is_empty() {
    let sample = missing_bvids
      .iter()
      .take(5)
      .cloned()
      .collect::<Vec<_>>()
      .join(",");
    append_log(
      &context.app_log_path,
      &format!(
        "submission_remote_refresh_missing count={} sample={}",
        missing_bvids.len(),
        sample
      ),
    );
  }
  context
    .db
    .with_conn_mut(|conn| {
      let tx = conn.transaction()?;
      for (task_id, bvid) in task_bvids {
        if bvid == REMOTE_DEBUG_BVID {
          if let Some(info) = remote_map.get(&bvid) {
            append_log(
              &context.app_log_path,
              &format!(
                "submission_remote_refresh_debug bvid={} state={} reject_reason={}",
                bvid,
                info.state,
                info.reject_reason.as_deref().unwrap_or("")
              ),
            );
          } else {
            append_log(
              &context.app_log_path,
              &format!("submission_remote_refresh_debug_missing bvid={}", bvid),
            );
          }
        }
        if let Some(info) = remote_map.get(&bvid) {
          tx.execute(
            "UPDATE submission_task SET remote_state = ?1, reject_reason = ?2 WHERE task_id = ?3",
            (info.state, info.reject_reason.as_deref(), &task_id),
          )?;
        } else {
          tx.execute(
            "UPDATE submission_task SET remote_state = ?1, reject_reason = NULL WHERE task_id = ?2",
            (0_i64, &task_id),
          )?;
        }
      }
      tx.commit()?;
      Ok(())
    })
    .map_err(|err| err.to_string())?;
  Ok(())
}

fn load_task_bvids(context: &SubmissionQueueContext) -> Result<Vec<(String, String)>, String> {
  context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT task_id, bvid FROM submission_task WHERE bvid IS NOT NULL AND TRIM(bvid) != ''",
      )?;
      let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
      let list = rows.collect::<Result<Vec<(String, String)>, _>>()?;
      Ok(list)
    })
    .map_err(|err| err.to_string())
}

async fn fetch_remote_audit_map(
  context: &SubmissionQueueContext,
  auth: &AuthInfo,
) -> Result<HashMap<String, RemoteAuditInfo>, String> {
  let status = REMOTE_AUDIT_STATUS;
  let mut page = 1_i64;
  let page_size = 20_i64;
  let mut result = HashMap::new();

  loop {
    let params = vec![
      ("status".to_string(), status.to_string()),
      ("pn".to_string(), page.to_string()),
      ("ps".to_string(), page_size.to_string()),
      ("coop".to_string(), "1".to_string()),
      ("interactive".to_string(), "1".to_string()),
    ];
    let query = build_query_params(&params);
    append_log(
      &context.app_log_path,
      &format!(
        "submission_remote_fetch_request url=https://member.bilibili.com/x/web/archives?{}",
        query
      ),
    );
    let data = context
      .bilibili
      .get_json(
        "https://member.bilibili.com/x/web/archives",
        &params,
        Some(auth),
        false,
      )
      .await?;
    append_log(
      &context.app_log_path,
      &format!(
        "submission_remote_fetch_response page={} data={}",
        page,
        truncate_log_value(&data)
      ),
    );
    let arc_audits = data
      .get("arc_audits")
      .and_then(|value| value.as_array())
      .cloned()
      .unwrap_or_default();
    for item in arc_audits.iter() {
      let archive = match item.get("Archive") {
        Some(value) => value,
        None => continue,
      };
      let bvid = archive
        .get("bvid")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
      if bvid.is_empty() {
        continue;
      }
      let state = archive.get("state").and_then(|value| value.as_i64()).unwrap_or(0);
      let reject_reason = item
        .get("problem_detail")
        .and_then(|value| value.as_array())
        .and_then(|items| {
          items.iter().find_map(|detail| {
            detail
              .get("reject_reason")
              .and_then(|value| value.as_str())
          })
        })
        .or_else(|| {
          archive
            .get("reject_reason")
            .and_then(|value| value.as_str())
        })
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
      result.insert(
        bvid,
        RemoteAuditInfo {
          state,
          reject_reason,
        },
      );
    }

    let total_count = data
      .get("page")
      .and_then(|value| value.get("count"))
      .and_then(|value| value.as_i64())
      .unwrap_or(0);
    if total_count <= 0 {
      break;
    }
    if page * page_size >= total_count {
      break;
    }
    if arc_audits.is_empty() {
      break;
    }
    page += 1;
  }

  Ok(result)
}

async fn recover_submission_tasks(context: SubmissionQueueContext) {
  let submission_context = SubmissionContext {
    db: context.db.clone(),
    app_log_path: context.app_log_path.clone(),
    edit_upload_state: context.edit_upload_state.clone(),
  };
  let mut processing_ids = Vec::new();
  for status in ["PENDING", "CLIPPING", "MERGING", "SEGMENTING"] {
    if let Ok(list) = load_task_ids_by_status(&submission_context, status) {
      processing_ids.extend(list);
    }
  }
  let uploading_ids = load_task_ids_by_status(&submission_context, "UPLOADING").unwrap_or_default();

  for task_id in uploading_ids {
    let _ = update_submission_status(&submission_context, &task_id, "WAITING_UPLOAD");
    append_log(
      &context.app_log_path,
      &format!("submission_recover_uploading task_id={}", task_id),
    );
  }

  for task_id in processing_ids {
    let _ = update_submission_status(&submission_context, &task_id, "PENDING");
    let _ = set_workflow_instance_status(&submission_context, &task_id, "PENDING");
    let context_clone = submission_context.clone();
    let task_id_clone = task_id.clone();
    append_log(
      &context.app_log_path,
      &format!("submission_recover_workflow task_id={}", task_id),
    );
    tauri::async_runtime::spawn(async move {
      let _ = run_submission_workflow(context_clone, task_id_clone).await;
    });
  }
}

fn build_part_title(prefix: Option<&str>, index: usize) -> String {
  let prefix = prefix.unwrap_or("").trim();
  if prefix.is_empty() {
    return format!("P{}", index);
  }
  format!("{}_part_{}", prefix, index)
}

fn build_segment_file_name(prefix: &str, index: usize) -> String {
  format!("{}_part_{}.mp4", prefix, index)
}

fn rename_segment_outputs_with_prefix(
  segments: &[PathBuf],
  prefix: Option<&str>,
  start_index: usize,
) -> Result<Vec<PathBuf>, String> {
  let prefix = prefix.unwrap_or("").trim();
  if prefix.is_empty() {
    return Ok(segments.to_vec());
  }
  let sanitized_prefix = sanitize_filename(prefix);
  let base_index = if start_index == 0 { 1 } else { start_index };
  let mut renamed = Vec::with_capacity(segments.len());
  for (index, segment) in segments.iter().enumerate() {
    let file_name = segment
      .file_name()
      .and_then(|name| name.to_str())
      .unwrap_or("");
    if !file_name.starts_with("part_") {
      renamed.push(segment.clone());
      continue;
    }
    let dir = segment
      .parent()
      .ok_or_else(|| "无法读取分段目录".to_string())?;
    let part_index = base_index + index;
    let new_name = build_segment_file_name(&sanitized_prefix, part_index);
    let target = dir.join(new_name);
    if target != *segment {
      if target.exists() {
        let _ = fs::remove_file(&target);
      }
      fs::rename(segment, &target).map_err(|err| {
        format!(
          "重命名分段文件失败: {} -> {} err={}",
          segment.to_string_lossy(),
          target.to_string_lossy(),
          err
        )
      })?;
    }
    renamed.push(target);
  }
  Ok(renamed)
}

fn resolve_existing_part_title(
  task: &SubmissionTaskRecord,
  part_name: &str,
  index: usize,
) -> String {
  let trimmed = part_name.trim();
  if trimmed.is_empty() {
    return build_part_title(task.segment_prefix.as_deref(), index);
  }
  if trimmed == format!("Part {}", index) {
    return build_part_title(task.segment_prefix.as_deref(), index);
  }
  trimmed.to_string()
}

fn is_default_part_name(
  part_name: &str,
  order: i64,
  segment_prefix: Option<&str>,
) -> bool {
  let trimmed = part_name.trim();
  if trimmed.is_empty() {
    return true;
  }
  if trimmed == format!("Part {}", order) {
    return true;
  }
  if order > 0 {
    let expected = build_part_title(segment_prefix, order as usize);
    return trimmed == expected;
  }
  false
}

fn build_progress_snapshot(
  uploaded_bytes: u64,
  total_bytes: u64,
  last_part_index: u64,
) -> UploadProgressSnapshot {
  let progress = if total_bytes > 0 {
    (uploaded_bytes as f64 / total_bytes as f64) * 100.0
  } else {
    0.0
  };
  UploadProgressSnapshot {
    uploaded_bytes,
    total_bytes,
    progress: progress.min(100.0).max(0.0),
    last_part_index,
  }
}

fn build_upload_session_from_segment(
  segment: &TaskOutputSegmentRecord,
) -> Option<UploadSessionInfo> {
  let upload_id = segment.upload_session_id.as_ref()?.trim().to_string();
  let endpoint = segment.upload_endpoint.as_ref()?.trim().to_string();
  let auth = segment.upload_auth.as_ref()?.trim().to_string();
  let upos_uri = segment.upload_uri.as_ref()?.trim().to_string();
  if upload_id.is_empty()
    || endpoint.is_empty()
    || auth.is_empty()
    || upos_uri.is_empty()
    || segment.upload_chunk_size <= 0
    || segment.upload_biz_id <= 0
  {
    return None;
  }
  Some(UploadSessionInfo {
    upload_id,
    biz_id: segment.upload_biz_id,
    chunk_size: segment.upload_chunk_size.max(0) as u64,
    endpoint,
    auth,
    upos_uri,
    uploaded_bytes: segment.upload_uploaded_bytes.max(0) as u64,
    total_bytes: segment.upload_total_bytes.max(0) as u64,
    last_part_index: segment.upload_last_part_index.max(0) as u64,
  })
}

fn build_upload_session_from_edit_segment(
  segment: &TaskOutputSegmentRecord,
) -> Option<UploadSessionInfo> {
  build_upload_session_from_segment(segment)
}

fn build_upload_session_from_merged(merged: &MergedVideoRecord) -> Option<UploadSessionInfo> {
  let upload_id = merged.upload_session_id.as_ref()?.trim().to_string();
  let endpoint = merged.upload_endpoint.as_ref()?.trim().to_string();
  let auth = merged.upload_auth.as_ref()?.trim().to_string();
  let upos_uri = merged.upload_uri.as_ref()?.trim().to_string();
  if upload_id.is_empty()
    || endpoint.is_empty()
    || auth.is_empty()
    || upos_uri.is_empty()
    || merged.upload_chunk_size <= 0
    || merged.upload_biz_id <= 0
  {
    return None;
  }
  Some(UploadSessionInfo {
    upload_id,
    biz_id: merged.upload_biz_id,
    chunk_size: merged.upload_chunk_size.max(0) as u64,
    endpoint,
    auth,
    upos_uri,
    uploaded_bytes: merged.upload_uploaded_bytes.max(0) as u64,
    total_bytes: merged.upload_total_bytes.max(0) as u64,
    last_part_index: merged.upload_last_part_index.max(0) as u64,
  })
}

fn retry_after_seconds(headers: &HeaderMap) -> Option<u64> {
  headers
    .get("retry-after")
    .or_else(|| headers.get("Retry-After"))
    .and_then(|value| value.to_str().ok())
    .and_then(|value| value.parse::<u64>().ok())
}

async fn wait_on_rate_limit(
  context: &SubmissionContext,
  target: &UploadTarget,
  limiter: &mut UploadRateLimiter,
  log_path: &PathBuf,
  retry_after: Option<u64>,
  stage: &str,
) {
  let wait_secs = limiter.next_wait_seconds(retry_after);
  let _ = update_upload_status_for_target(context, target, "RATE_LIMITED");
  append_log(
    log_path,
    &format!(
      "upload_rate_limited stage={} wait_secs={} count={}",
      stage, wait_secs, limiter.consecutive_406
    ),
  );
  sleep(Duration::from_secs(wait_secs)).await;
  let _ = restore_upload_status_after_rate_limit(context, target);
}

fn sanitize_upload_session(
  resume_session: Option<UploadSessionInfo>,
  file_size: u64,
) -> Option<UploadSessionInfo> {
  let mut session = resume_session?;
  if session.total_bytes == 0 {
    session.total_bytes = file_size;
  }
  if session.total_bytes != file_size {
    return None;
  }
  if session.upload_id.trim().is_empty()
    || session.endpoint.trim().is_empty()
    || session.auth.trim().is_empty()
    || session.upos_uri.trim().is_empty()
    || session.chunk_size == 0
    || session.biz_id <= 0
  {
    return None;
  }
  Some(session)
}

async fn upload_file_with_session(
  context: &SubmissionContext,
  target: &UploadTarget,
  client: &Client,
  auth: &AuthInfo,
  path: &Path,
  file_name: &str,
  file_size: u64,
  log_path: &PathBuf,
  resume_session: Option<UploadSessionInfo>,
) -> Result<UploadFileResult, String> {
  let mut limiter = UploadRateLimiter::new();
  let (preupload, upload_id, resume_state) = if let Some(session) = resume_session.clone() {
    let preupload = PreuploadInfo {
      auth: session.auth.clone(),
      biz_id: session.biz_id,
      chunk_size: session.chunk_size,
      endpoint: session.endpoint.clone(),
      upos_uri: session.upos_uri.clone(),
    };
    update_upload_session(context, target, &session)?;
    (preupload, session.upload_id.clone(), resume_session)
  } else {
    let preupload = preupload_video(
      context,
      target,
      client,
      auth,
      file_name,
      file_size,
      log_path,
      &mut limiter,
    )
    .await?;
    let upload_id =
      post_video_meta(context, target, client, auth, &preupload, file_size, log_path, &mut limiter)
        .await?;
    let session = UploadSessionInfo {
      upload_id: upload_id.clone(),
      biz_id: preupload.biz_id,
      chunk_size: preupload.chunk_size,
      endpoint: preupload.endpoint.clone(),
      auth: preupload.auth.clone(),
      upos_uri: preupload.upos_uri.clone(),
      uploaded_bytes: 0,
      total_bytes: file_size,
      last_part_index: 0,
    };
    update_upload_session(context, target, &session)?;
    (preupload, upload_id, None)
  };

  let total_chunks = upload_video_chunks(
    context,
    target,
    client,
    auth,
    path,
    &preupload,
    &upload_id,
    file_size,
    log_path,
    &mut limiter,
    resume_state.as_ref(),
  )
  .await?;
  let end_result = end_upload(
    context,
    target,
    client,
    auth,
    &preupload,
    &upload_id,
    file_name,
    total_chunks,
    log_path,
    &mut limiter,
  )
  .await?;
  let cid = end_result
    .get("data")
    .and_then(|value| value.get("cid"))
    .and_then(|value| value.as_i64())
    .unwrap_or(preupload.biz_id);
  let filename = parse_upload_filename(&end_result, file_name);
  if file_size > 0 {
    let final_index = total_chunks.saturating_sub(1);
    let snapshot = build_progress_snapshot(file_size, file_size, final_index);
    update_upload_progress(context, target, &snapshot)?;
  }

  Ok(UploadFileResult { cid, filename })
}

async fn upload_single_file(
  context: &SubmissionContext,
  target: &UploadTarget,
  client: &Client,
  auth: &AuthInfo,
  path: &Path,
  log_path: &PathBuf,
  resume_session: Option<UploadSessionInfo>,
) -> Result<UploadFileResult, String> {
  let file_name = path
    .file_name()
    .and_then(|name| name.to_str())
    .ok_or_else(|| "无法读取文件名".to_string())?;
  let metadata = tokio::fs::metadata(path)
    .await
    .map_err(|err| format!("读取文件失败: {}", err))?;
  let file_size = metadata.len();
  let session = sanitize_upload_session(resume_session, file_size);

  if session.is_some() {
    if let Ok(result) = upload_file_with_session(
      context,
      target,
      client,
      auth,
      path,
      file_name,
      file_size,
      log_path,
      session.clone(),
    )
    .await
    {
      return Ok(result);
    }
    let _ = clear_upload_session(context, target);
  }

  upload_file_with_session(
    context,
    target,
    client,
    auth,
    path,
    file_name,
    file_size,
    log_path,
    None,
  )
  .await
}

async fn upload_segment_with_retry(
  context: &SubmissionContext,
  upload_context: &UploadContext,
  client: &Client,
  auth: &AuthInfo,
  segment_id: &str,
  log_path: &PathBuf,
  max_retries: u32,
) -> Result<UploadFileResult, String> {
  let mut attempt: u32 = 0;
  let mut current_auth = auth.clone();
  loop {
    attempt = attempt.saturating_add(1);
    let segment = load_output_segment_by_id(context, segment_id)?
      .ok_or_else(|| "分段不存在".to_string())?;
    let path = Path::new(&segment.segment_file_path);
    if segment.segment_file_path.trim().is_empty() || !path.exists() {
      return Err("分段文件不存在".to_string());
    }

    let target = UploadTarget::Segment(segment.segment_id.clone());
    let resume_session = build_upload_session_from_segment(&segment);
    match upload_single_file(
      context,
      &target,
      client,
      &current_auth,
      path,
      log_path,
      resume_session,
    )
    .await
    {
      Ok(result) => return Ok(result),
      Err(err) => {
        if is_auth_error(&err) {
          match refresh_auth(upload_context, "upload_segment").await {
            Ok(auth) => {
              current_auth = auth;
              continue;
            }
            Err(refresh_err) => return Err(refresh_err),
          }
        }
        append_log(
          log_path,
          &format!(
            "submission_segment_retry_fail segment_id={} attempt={} err={}",
            segment_id, attempt, err
          ),
        );
        if attempt >= max_retries {
          return Err(err);
        }
        let wait_secs = upload_retry_delay_secs(attempt);
        sleep(Duration::from_secs(wait_secs)).await;
      }
    }
  }
}

async fn upload_edit_segment_with_retry(
  context: &SubmissionContext,
  upload_context: &UploadContext,
  client: &Client,
  auth: &AuthInfo,
  segment_id: &str,
  log_path: &PathBuf,
  max_retries: u32,
) -> Result<UploadFileResult, String> {
  let mut attempt: u32 = 0;
  let mut current_auth = auth.clone();
  loop {
    attempt = attempt.saturating_add(1);
    let segment = load_edit_upload_segment(context, segment_id)?
      .ok_or_else(|| "分段不存在".to_string())?;
    let path = Path::new(&segment.segment_file_path);
    if segment.segment_file_path.trim().is_empty() || !path.exists() {
      return Err("分段文件不存在".to_string());
    }

    let target = UploadTarget::EditSegment(segment.segment_id.clone());
    let resume_session = build_upload_session_from_edit_segment(&segment);
    match upload_single_file(
      context,
      &target,
      client,
      &current_auth,
      path,
      log_path,
      resume_session,
    )
    .await
    {
      Ok(result) => return Ok(result),
      Err(err) => {
        if is_auth_error(&err) {
          match refresh_auth(upload_context, "upload_edit_segment").await {
            Ok(auth) => {
              current_auth = auth;
              continue;
            }
            Err(refresh_err) => return Err(refresh_err),
          }
        }
        append_log(
          log_path,
          &format!(
            "submission_edit_segment_retry_fail segment_id={} attempt={} err={}",
            segment_id, attempt, err
          ),
        );
        if attempt >= max_retries {
          return Err(err);
        }
        let wait_secs = upload_retry_delay_secs(attempt);
        sleep(Duration::from_secs(wait_secs)).await;
      }
    }
  }
}

async fn preupload_video(
  context: &SubmissionContext,
  target: &UploadTarget,
  client: &Client,
  auth: &AuthInfo,
  file_name: &str,
  file_size: u64,
  log_path: &PathBuf,
  limiter: &mut UploadRateLimiter,
) -> Result<PreuploadInfo, String> {
  let url = "https://member.bilibili.com/preupload";
  let params = vec![
    ("name", file_name.to_string()),
    ("r", "upos".to_string()),
    ("profile", "ugcfx/bup".to_string()),
    ("version", "2.14.0.0".to_string()),
    ("size", file_size.to_string()),
  ];

  loop {
    wait_preupload_throttle(log_path, target, file_name).await;
    let headers = build_headers(Some(&auth.cookie))?;
    let response = client
      .get(url)
      .headers(headers)
      .query(&params)
      .send()
      .await
      .map_err(|err| format!("预上传请求失败: {}", err))?;
    let status = response.status();
    if status == StatusCode::NOT_ACCEPTABLE {
      let retry_after = retry_after_seconds(response.headers());
      wait_on_rate_limit(context, target, limiter, log_path, retry_after, "preupload").await;
      continue;
    }
    let content_type = response
      .headers()
      .get(CONTENT_TYPE)
      .and_then(|val| val.to_str().ok())
      .unwrap_or("-")
      .to_string();
    let body = response
      .text()
      .await
      .map_err(|err| format!("预上传读取失败: {}", err))?;
    if is_rate_limit_error(&body) || status == StatusCode::TOO_MANY_REQUESTS {
      append_log(
        log_path,
        &format!(
          "preupload_rate_limited target={} status={} content_type={} body={}",
          upload_target_label(target),
          status.as_u16(),
          content_type,
          truncate_log_text(&body)
        ),
      );
      wait_on_rate_limit(context, target, limiter, log_path, None, "preupload").await;
      continue;
    }
    let value: Value = serde_json::from_str(&body).map_err(|err| {
      append_log(
        log_path,
        &format!(
          "preupload_parse_fail target={} status={} content_type={} body={}",
          upload_target_label(target),
          status.as_u16(),
          content_type,
          truncate_log_text(&body)
        ),
      );
      format!("预上传解析失败: {}", err)
    })?;
    if let Some(code) = value.get("code").and_then(|val| val.as_i64()) {
      if code != 0 {
        let message = value
          .get("message")
          .and_then(|val| val.as_str())
          .unwrap_or("预上传失败");
        return Err(format!("{} (code: {})", message, code));
      }
    }
    if let Some(ok) = value.get("OK").and_then(|val| val.as_i64()) {
      if ok != 1 {
        return Err("预上传失败".to_string());
      }
    }
    limiter.reset();
    return Ok(PreuploadInfo {
      auth: value
        .get("auth")
        .and_then(|val| val.as_str())
        .ok_or_else(|| "预上传缺少auth".to_string())?
        .to_string(),
      biz_id: value
        .get("biz_id")
        .and_then(|val| val.as_i64())
        .ok_or_else(|| "预上传缺少biz_id".to_string())?,
      chunk_size: value
        .get("chunk_size")
        .and_then(|val| val.as_u64())
        .ok_or_else(|| "预上传缺少chunk_size".to_string())?,
      endpoint: value
        .get("endpoint")
        .and_then(|val| val.as_str())
        .ok_or_else(|| "预上传缺少endpoint".to_string())?
        .to_string(),
      upos_uri: value
        .get("upos_uri")
        .and_then(|val| val.as_str())
        .ok_or_else(|| "预上传缺少upos_uri".to_string())?
        .to_string(),
    });
  }
}

async fn post_video_meta(
  context: &SubmissionContext,
  target: &UploadTarget,
  client: &Client,
  auth: &AuthInfo,
  preupload: &PreuploadInfo,
  file_size: u64,
  log_path: &PathBuf,
  limiter: &mut UploadRateLimiter,
) -> Result<String, String> {
  let url = build_upload_url(&preupload.endpoint, &preupload.upos_uri);
  let params = vec![
    ("uploads", "".to_string()),
    ("output", "json".to_string()),
    ("profile", "ugcfx/bup".to_string()),
    ("filesize", file_size.to_string()),
    ("partsize", preupload.chunk_size.to_string()),
    ("biz_id", preupload.biz_id.to_string()),
  ];
  loop {
    let mut headers = build_headers(Some(&auth.cookie))?;
    headers.insert(
      "X-Upos-Auth",
      HeaderValue::from_str(&preupload.auth).map_err(|_| "无效的X-Upos-Auth".to_string())?,
    );
    let response = client
      .post(url.clone())
      .headers(headers)
      .query(&params)
      .send()
      .await
      .map_err(|err| format!("上传元数据失败: {}", err))?;
    if response.status() == StatusCode::NOT_ACCEPTABLE {
      let retry_after = retry_after_seconds(response.headers());
      wait_on_rate_limit(context, target, limiter, log_path, retry_after, "post_meta").await;
      continue;
    }
    let value: Value = response
      .json()
      .await
      .map_err(|err| format!("上传元数据解析失败: {}", err))?;
    if let Some(code) = value.get("code").and_then(|val| val.as_i64()) {
      if code != 0 {
        let message = value
          .get("message")
          .and_then(|val| val.as_str())
          .unwrap_or("上传元数据失败");
        return Err(format!("{} (code: {})", message, code));
      }
    }
    if let Some(ok) = value.get("OK").and_then(|val| val.as_i64()) {
      if ok != 1 {
        return Err("上传元数据失败".to_string());
      }
    }
    let upload_id = value
      .get("upload_id")
      .and_then(|val| val.as_str())
      .ok_or_else(|| "上传元数据缺少upload_id".to_string())?;
    limiter.reset();
    return Ok(upload_id.to_string());
  }
}

async fn upload_video_chunks(
  context: &SubmissionContext,
  target: &UploadTarget,
  client: &Client,
  auth: &AuthInfo,
  path: &Path,
  preupload: &PreuploadInfo,
  upload_id: &str,
  file_size: u64,
  log_path: &PathBuf,
  limiter: &mut UploadRateLimiter,
  resume_state: Option<&UploadSessionInfo>,
) -> Result<u64, String> {
  let upload_url = build_upload_url(&preupload.endpoint, &preupload.upos_uri);
  let mut file = tokio::fs::File::open(path)
    .await
    .map_err(|err| format!("读取视频文件失败: {}", err))?;
  let chunk_size = preupload.chunk_size;
  let total_chunks = (file_size + chunk_size - 1) / chunk_size;
  let mut start_index: u64 = 0;
  if let Some(state) = resume_state {
    if state.uploaded_bytes > 0 && state.chunk_size == chunk_size {
      start_index = state.last_part_index.saturating_add(1);
    }
  }
  if start_index > total_chunks {
    start_index = total_chunks;
  }
  let mut offset = start_index.saturating_mul(chunk_size);
  if offset > file_size {
    offset = file_size;
  }
  if offset > 0 {
    file
      .seek(SeekFrom::Start(offset))
      .await
      .map_err(|err| format!("跳转文件位置失败: {}", err))?;
  }

  let mut progress_limiter = UploadProgressLimiter::new();
  if offset > 0 {
    let snapshot = build_progress_snapshot(offset, file_size, start_index.saturating_sub(1));
    if update_upload_progress(context, target, &snapshot).is_ok() {
      progress_limiter.mark_saved(&snapshot);
    } else {
      append_log(
        log_path,
        &format!("upload_progress_skip target_offset={} file_size={}", offset, file_size),
      );
    }
  }

  let mut index = start_index;
  while index < total_chunks {
    let remaining = file_size.saturating_sub(offset);
    if remaining == 0 {
      break;
    }
    let current_size = std::cmp::min(chunk_size, remaining) as usize;
    let mut buffer = vec![0u8; current_size];
    file
      .read_exact(&mut buffer)
      .await
      .map_err(|err| format!("读取分片失败: {}", err))?;
    let start = offset;
    let end = offset + current_size as u64;
    let params = vec![
      ("partNumber", (index + 1).to_string()),
      ("uploadId", upload_id.to_string()),
      ("chunk", index.to_string()),
      ("chunks", total_chunks.to_string()),
      ("size", current_size.to_string()),
      ("start", start.to_string()),
      ("end", end.to_string()),
      ("total", file_size.to_string()),
    ];

    loop {
      let mut headers = build_headers(Some(&auth.cookie))?;
      headers.insert(
        "X-Upos-Auth",
        HeaderValue::from_str(&preupload.auth).map_err(|_| "无效的X-Upos-Auth".to_string())?,
      );
      headers.insert(
        "Content-Type",
        HeaderValue::from_static("application/octet-stream"),
      );

      let response = client
        .put(upload_url.clone())
        .headers(headers)
        .query(&params)
        .body(buffer.clone())
        .send()
        .await
        .map_err(|err| format!("上传分片失败: {}", err))?;
      if response.status() == StatusCode::NOT_ACCEPTABLE {
        let retry_after = retry_after_seconds(response.headers());
        wait_on_rate_limit(context, target, limiter, log_path, retry_after, "upload_chunk").await;
        continue;
      }
      let text = response
        .text()
        .await
        .map_err(|err| format!("读取分片响应失败: {}", err))?;
      if !text.contains("MULTIPART_PUT_SUCCESS") {
        return Err("分片上传失败".to_string());
      }
      limiter.reset();
      break;
    }

    offset = end;
    let snapshot = build_progress_snapshot(offset, file_size, index);
    if progress_limiter.should_persist(&snapshot) {
      if update_upload_progress(context, target, &snapshot).is_ok() {
        progress_limiter.mark_saved(&snapshot);
      } else {
        append_log(
          log_path,
          &format!(
            "upload_progress_skip offset={} file_size={} part={}",
            offset, file_size, index
          ),
        );
      }
    }
    index = index.saturating_add(1);
  }

  Ok(total_chunks)
}

async fn end_upload(
  context: &SubmissionContext,
  target: &UploadTarget,
  client: &Client,
  auth: &AuthInfo,
  preupload: &PreuploadInfo,
  upload_id: &str,
  file_name: &str,
  total_chunks: u64,
  log_path: &PathBuf,
  limiter: &mut UploadRateLimiter,
) -> Result<Value, String> {
  let upload_url = build_upload_url(&preupload.endpoint, &preupload.upos_uri);
  let params = vec![
    ("output", "json".to_string()),
    ("name", file_name.to_string()),
    ("profile", "ugcfx/bup".to_string()),
    ("uploadId", upload_id.to_string()),
    ("biz_id", preupload.biz_id.to_string()),
  ];
  let mut parts = Vec::new();
  for index in 0..total_chunks {
    parts.push(serde_json::json!({
      "partNumber": index + 1,
      "eTag": "etag"
    }));
  }
  let body = serde_json::json!({ "parts": parts });
  loop {
    let mut headers = build_headers(Some(&auth.cookie))?;
    headers.insert(
      "X-Upos-Auth",
      HeaderValue::from_str(&preupload.auth).map_err(|_| "无效的X-Upos-Auth".to_string())?,
    );

    let response = client
      .post(upload_url.clone())
      .headers(headers)
      .query(&params)
      .json(&body)
      .send()
      .await
      .map_err(|err| format!("结束上传失败: {}", err))?;
    if response.status() == StatusCode::NOT_ACCEPTABLE {
      let retry_after = retry_after_seconds(response.headers());
      wait_on_rate_limit(context, target, limiter, log_path, retry_after, "end_upload").await;
      continue;
    }
    let value: Value = response
      .json()
      .await
      .map_err(|err| format!("结束上传解析失败: {}", err))?;
    if let Some(ok) = value.get("OK").and_then(|val| val.as_i64()) {
      if ok != 1 {
        return Err("结束上传失败".to_string());
      }
    }
    limiter.reset();
    return Ok(value);
  }
}

fn parse_upload_filename(end_result: &Value, fallback: &str) -> String {
  let fallback_name = remove_file_extension(fallback);
  let key = end_result.get("key").and_then(|value| value.as_str());
  let Some(key) = key else {
    return fallback_name;
  };
  let trimmed = key.trim_start_matches('/');
  let name = trimmed
    .rsplit('/')
    .next()
    .unwrap_or(trimmed)
    .to_string();
  remove_file_extension(&name)
}

fn remove_file_extension(name: &str) -> String {
  match name.rsplit_once('.') {
    Some((base, _)) => base.to_string(),
    None => name.to_string(),
  }
}

fn build_upload_url(endpoint: &str, upos_uri: &str) -> String {
  let mut path = upos_uri.trim_start_matches("upos://").to_string();
  if !path.starts_with('/') {
    path = format!("/{}", path);
  }
  format!("https:{}{}", endpoint, path)
}

async fn submit_video_add_with_refresh(
  context: &UploadContext,
  auth: &AuthInfo,
  task: &SubmissionTaskRecord,
  parts: &[UploadedVideoPart],
  csrf: &str,
) -> Result<SubmissionSubmitResult, String> {
  match submit_video_add(context, auth, task, parts, csrf).await {
    Ok(result) => Ok(result),
    Err(err) => {
      if !is_auth_error(&err) {
        return Err(err);
      }
      let auth = refresh_auth(context, "submit_video_add").await?;
      let csrf = auth
        .csrf
        .clone()
        .ok_or_else(|| "登录信息缺少CSRF".to_string())?;
      submit_video_add(context, &auth, task, parts, &csrf).await
    }
  }
}

async fn submit_video_edit_with_refresh(
  context: &UploadContext,
  auth: &AuthInfo,
  task: &SubmissionTaskRecord,
  parts: &[UploadedVideoPart],
  aid: i64,
  csrf: &str,
) -> Result<(), String> {
  match submit_video_edit(context, auth, task, parts, aid, csrf).await {
    Ok(()) => Ok(()),
    Err(err) => {
      if !is_auth_error(&err) {
        return Err(err);
      }
      let auth = refresh_auth(context, "submit_video_edit").await?;
      let csrf = auth
        .csrf
        .clone()
        .ok_or_else(|| "登录信息缺少CSRF".to_string())?;
      submit_video_edit(context, &auth, task, parts, aid, &csrf).await
    }
  }
}

async fn submit_video_edit_with_rate_limit_retry(
  context: &UploadContext,
  auth: &AuthInfo,
  task: &SubmissionTaskRecord,
  parts: &[UploadedVideoPart],
  aid: i64,
  csrf: &str,
) -> Result<(), String> {
  let mut attempt: u32 = 0;
  let mut wait_secs = SUBMISSION_EDIT_RATE_LIMIT_BASE_WAIT_SECS;
  loop {
    match submit_video_edit_with_refresh(context, auth, task, parts, aid, csrf).await {
      Ok(()) => return Ok(()),
      Err(err) => {
        if !is_rate_limit_error(&err) {
          return Err(err);
        }
        attempt = attempt.saturating_add(1);
        if attempt > SUBMISSION_EDIT_RATE_LIMIT_RETRY_LIMIT {
          return Err(err);
        }
        let current_wait = wait_secs.min(SUBMISSION_EDIT_RATE_LIMIT_MAX_WAIT_SECS);
        append_log(
          &context.app_log_path,
          &format!(
            "submission_edit_rate_limited aid={} attempt={} wait_secs={} err={}",
            aid, attempt, current_wait, err
          ),
        );
        sleep(Duration::from_secs(current_wait)).await;
        wait_secs = wait_secs.saturating_mul(2);
      }
    }
  }
}

async fn submit_video_in_batches(
  context: &UploadContext,
  auth: &AuthInfo,
  task: &SubmissionTaskRecord,
  parts: &[UploadedVideoPart],
  csrf: &str,
) -> Result<SubmissionSubmitResult, String> {
  if parts.len() <= MAX_PARTS_PER_SUBMISSION {
    return submit_video_add_with_refresh(context, auth, task, parts, csrf).await;
  }

  let total_batches = (parts.len() + MAX_PARTS_PER_SUBMISSION - 1) / MAX_PARTS_PER_SUBMISSION;
  append_log(
    &context.app_log_path,
    &format!(
      "submission_submit_batches_start title={} total_parts={} total_batches={}",
      task.title,
      parts.len(),
      total_batches
    ),
  );

  let first_parts = &parts[..MAX_PARTS_PER_SUBMISSION];
  let result = submit_video_add_with_refresh(context, auth, task, first_parts, csrf).await?;
  let mut end_index = MAX_PARTS_PER_SUBMISSION;
  let mut batch_index = 2;

  while end_index < parts.len() {
    let next_end = std::cmp::min(end_index + MAX_PARTS_PER_SUBMISSION, parts.len());
    append_log(
      &context.app_log_path,
      &format!(
        "submission_edit_batch_start aid={} batch={}/{} parts=1-{}",
        result.aid, batch_index, total_batches, next_end
      ),
    );
    append_log(
      &context.app_log_path,
      &format!(
        "submission_edit_batch_wait aid={} batch={}/{} wait_secs={}",
        result.aid, batch_index, total_batches, SUBMISSION_EDIT_BATCH_WAIT_SECS
      ),
    );
    sleep(Duration::from_secs(SUBMISSION_EDIT_BATCH_WAIT_SECS)).await;
    submit_video_edit_with_rate_limit_retry(
      context,
      auth,
      task,
      &parts[..next_end],
      result.aid,
      csrf,
    )
    .await?;
    append_log(
      &context.app_log_path,
      &format!(
        "submission_edit_batch_ok aid={} batch={}/{} parts=1-{}",
        result.aid, batch_index, total_batches, next_end
      ),
    );
    end_index = next_end;
    batch_index += 1;
  }

  Ok(result)
}

async fn submit_video_update_in_batches(
  context: &UploadContext,
  auth: &AuthInfo,
  task: &SubmissionTaskRecord,
  parts: &[UploadedVideoPart],
  aid: i64,
  csrf: &str,
) -> Result<(), String> {
  if parts.len() <= MAX_PARTS_PER_SUBMISSION {
    submit_video_edit_with_refresh(context, auth, task, parts, aid, csrf).await?;
    return Ok(());
  }

  let total_batches = (parts.len() + MAX_PARTS_PER_SUBMISSION - 1) / MAX_PARTS_PER_SUBMISSION;
  append_log(
    &context.app_log_path,
    &format!(
      "submission_update_batches_start title={} total_parts={} total_batches={}",
      task.title,
      parts.len(),
      total_batches
    ),
  );
  let mut end_index = MAX_PARTS_PER_SUBMISSION;
  let mut batch_index = 1;

  loop {
    let next_end = std::cmp::min(end_index, parts.len());
    append_log(
      &context.app_log_path,
      &format!(
        "submission_update_batch_start aid={} batch={}/{} parts=1-{}",
        aid, batch_index, total_batches, next_end
      ),
    );
    if batch_index > 1 {
      append_log(
        &context.app_log_path,
        &format!(
          "submission_update_batch_wait aid={} batch={}/{} wait_secs={}",
          aid, batch_index, total_batches, SUBMISSION_EDIT_BATCH_WAIT_SECS
        ),
      );
      sleep(Duration::from_secs(SUBMISSION_EDIT_BATCH_WAIT_SECS)).await;
    }
    submit_video_edit_with_rate_limit_retry(context, auth, task, &parts[..next_end], aid, csrf)
      .await?;
    append_log(
      &context.app_log_path,
      &format!(
        "submission_update_batch_ok aid={} batch={}/{} parts=1-{}",
        aid, batch_index, total_batches, next_end
      ),
    );
    if next_end >= parts.len() {
      break;
    }
    end_index = next_end + MAX_PARTS_PER_SUBMISSION;
    batch_index += 1;
  }
  Ok(())
}

async fn submit_video_add(
  context: &UploadContext,
  auth: &AuthInfo,
  task: &SubmissionTaskRecord,
  parts: &[UploadedVideoPart],
  csrf: &str,
) -> Result<SubmissionSubmitResult, String> {
  let payload = build_add_payload(task, parts);
  append_log(
    &context.app_log_path,
    &format!(
      "submission_submit_start title={} season_id={} parts={}",
      task.title,
      task.collection_id.unwrap_or(0),
      parts.len()
    ),
  );
  let params = vec![
    ("ts".to_string(), Utc::now().timestamp_millis().to_string()),
    ("csrf".to_string(), csrf.to_string()),
  ];
  let url = "https://member.bilibili.com/x/vu/web/add/v3";
  let data = context
    .bilibili
    .post_json(url, &params, &payload, Some(auth))
    .await?;
  let bvid = data
    .get("bvid")
    .and_then(|val| val.as_str())
    .ok_or_else(|| "投稿响应缺少BVID".to_string())?;
  let aid = data
    .get("aid")
    .and_then(|val| val.as_i64())
    .ok_or_else(|| "投稿响应缺少AID".to_string())?;
  append_log(
    &context.app_log_path,
    &format!(
      "submission_submit_ok title={} season_id={} bvid={} aid={}",
      task.title,
      task.collection_id.unwrap_or(0),
      bvid,
      aid
    ),
  );
  Ok(SubmissionSubmitResult {
    bvid: bvid.to_string(),
    aid,
  })
}

async fn submit_video_edit(
  context: &UploadContext,
  auth: &AuthInfo,
  task: &SubmissionTaskRecord,
  parts: &[UploadedVideoPart],
  aid: i64,
  csrf: &str,
) -> Result<(), String> {
  let payload = build_edit_payload(task, parts, aid);
  let params = vec![
    ("t".to_string(), Utc::now().timestamp_millis().to_string()),
    ("csrf".to_string(), csrf.to_string()),
  ];
  let url = "https://member.bilibili.com/x/vu/web/edit";
  let _ = context
    .bilibili
    .post_json(url, &params, &payload, Some(auth))
    .await?;
  Ok(())
}

fn build_submission_videos(parts: &[UploadedVideoPart]) -> Vec<Value> {
  parts
    .iter()
    .map(|part| {
      let mut map = Map::new();
      if !part.filename.trim().is_empty() {
        map.insert("filename".to_string(), Value::String(part.filename.clone()));
      }
      map.insert("title".to_string(), Value::String(part.title.clone()));
      map.insert("desc".to_string(), Value::String(String::new()));
      map.insert("cid".to_string(), Value::Number(Number::from(part.cid)));
      Value::Object(map)
    })
    .collect()
}

fn build_add_payload(task: &SubmissionTaskRecord, parts: &[UploadedVideoPart]) -> Value {
  let copyright = if task.video_type == "ORIGINAL" { 1 } else { 2 };
  let tags = task.tags.clone().unwrap_or_default();
  let desc = task.description.clone().unwrap_or_default();
  let cover = task.cover_url.clone().unwrap_or_default();
  let videos = build_submission_videos(parts);

  let mut payload = serde_json::json!({
    "videos": videos,
    "cover": cover,
    "cover43": "",
    "title": task.title,
    "copyright": copyright,
    "tid": task.partition_id,
    "human_type2": task.partition_id,
    "tag": tags,
    "desc_format_id": 9999,
    "desc": desc,
    "recreate": -1,
    "dynamic": "",
    "interactive": 0,
    "act_reserve_create": 0,
    "no_disturbance": 0,
    "no_reprint": 1,
    "subtitle": { "open": 0, "lan": "" },
    "dolby": 0,
    "lossless_music": 0,
    "up_selection_reply": false,
    "up_close_reply": false,
    "up_close_danmu": false,
    "web_os": 3
  });

  if let Some(collection_id) = task.collection_id {
    if collection_id > 0 {
      payload["season_id"] = serde_json::json!(collection_id);
    }
  }
  if let Some(topic_id) = task.topic_id {
    if topic_id > 0 {
      payload["topic_id"] = serde_json::json!(topic_id);
    }
  }
  if let Some(mission_id) = task.mission_id {
    if mission_id > 0 {
      payload["mission_id"] = serde_json::json!(mission_id);
    }
  }

  payload
}

fn build_edit_payload(task: &SubmissionTaskRecord, parts: &[UploadedVideoPart], aid: i64) -> Value {
  let copyright = if task.video_type == "ORIGINAL" { 1 } else { 2 };
  let tags = task.tags.clone().unwrap_or_default();
  let desc = task.description.clone().unwrap_or_default();
  let cover = task.cover_url.clone().unwrap_or_default();
  let videos = build_submission_videos(parts);

  let mut payload = serde_json::json!({
    "aid": aid,
    "videos": videos,
    "cover": cover,
    "cover43": "",
    "title": task.title,
    "copyright": copyright,
    "tid": task.partition_id,
    "tag": tags,
    "desc_format_id": 9999,
    "desc": desc,
    "recreate": -1,
    "dynamic": "",
    "interactive": 0,
    "act_reserve_create": 0,
    "no_disturbance": 0,
    "no_reprint": 1,
    "subtitle": { "open": 0, "lan": "" },
    "dolby": 0,
    "lossless_music": 0,
    "up_selection_reply": false,
    "up_close_reply": false,
    "up_close_danmu": false,
    "web_os": 1
  });

  if let Some(collection_id) = task.collection_id {
    if collection_id > 0 {
      payload["season_id"] = serde_json::json!(collection_id);
    }
  }
  if let Some(topic_id) = task.topic_id {
    if topic_id > 0 {
      payload["topic_id"] = serde_json::json!(topic_id);
    }
  }
  if let Some(mission_id) = task.mission_id {
    if mission_id > 0 {
      payload["mission_id"] = serde_json::json!(mission_id);
    }
  }

  payload
}

async fn add_video_to_collection_with_refresh(
  context: &UploadContext,
  auth: &AuthInfo,
  title: &str,
  season_id: i64,
  aid: i64,
  cid: i64,
  csrf: &str,
) -> Result<(), String> {
  match add_video_to_collection(context, auth, title, season_id, aid, cid, csrf).await {
    Ok(()) => Ok(()),
    Err(err) => {
      if !is_auth_error(&err) {
        return Err(err);
      }
      let auth = refresh_auth(context, "add_video_collection").await?;
      let csrf = auth
        .csrf
        .clone()
        .ok_or_else(|| "登录信息缺少CSRF".to_string())?;
      add_video_to_collection(context, &auth, title, season_id, aid, cid, &csrf).await
    }
  }
}

async fn switch_video_collection_with_refresh(
  context: &UploadContext,
  auth: &AuthInfo,
  title: &str,
  season_id: i64,
  aid: i64,
  csrf: &str,
) -> Result<(), String> {
  match switch_video_collection(context, auth, title, season_id, aid, csrf).await {
    Ok(()) => Ok(()),
    Err(err) => {
      if !is_auth_error(&err) {
        return Err(err);
      }
      let auth = refresh_auth(context, "switch_video_collection").await?;
      let csrf = auth
        .csrf
        .clone()
        .ok_or_else(|| "登录信息缺少CSRF".to_string())?;
      switch_video_collection(context, &auth, title, season_id, aid, &csrf).await
    }
  }
}

async fn add_video_to_collection(
  context: &UploadContext,
  auth: &AuthInfo,
  title: &str,
  season_id: i64,
  aid: i64,
  cid: i64,
  csrf: &str,
) -> Result<(), String> {
  if aid <= 0 || cid <= 0 {
    return Err("合集绑定缺少AID或CID".to_string());
  }
  let section_id = fetch_collection_section_id(context, auth, season_id)
    .await
    .unwrap_or(0);
  append_log(
    &context.app_log_path,
    &format!(
      "submission_collection_start season_id={} section_id={} aid={} cid={}",
      season_id, section_id, aid, cid
    ),
  );

  let url = "https://member.bilibili.com/x2/creative/web/season/section/episodes/add";
  let params = vec![("csrf".to_string(), csrf.to_string())];
  let payload = serde_json::json!({
    "sectionId": section_id,
    "episodes": [
      {
        "title": title,
        "aid": aid,
        "cid": cid,
        "charging_pay": 0
      }
    ]
  });

  let _ = context
    .bilibili
    .post_json(url, &params, &payload, Some(auth))
    .await?;

  append_log(
    &context.app_log_path,
    &format!(
      "submission_collection_ok season_id={} section_id={} aid={}",
      season_id, section_id, aid
    ),
  );
  Ok(())
}

fn is_collection_not_found_error(err: &str) -> bool {
  err.contains("code: -404") || err.contains("啥都木有")
}

async fn switch_video_collection(
  context: &UploadContext,
  auth: &AuthInfo,
  title: &str,
  season_id: i64,
  aid: i64,
  csrf: &str,
) -> Result<(), String> {
  if season_id <= 0 || aid <= 0 {
    return Err("合集切换缺少season_id或aid".to_string());
  }
  let section_id = fetch_collection_section_id(context, auth, season_id)
    .await
    .unwrap_or(0);
  append_log(
    &context.app_log_path,
    &format!(
      "submission_collection_switch_start season_id={} section_id={} aid={}",
      season_id, section_id, aid
    ),
  );
  let url = "https://member.bilibili.com/x2/creative/web/season/switch";
  let params = vec![("csrf".to_string(), csrf.to_string())];
  let payload = serde_json::json!({
    "season_id": season_id,
    "section_id": section_id,
    "title": title,
    "aid": aid,
    "csrf": csrf
  });
  let _ = context
    .bilibili
    .post_json(url, &params, &payload, Some(auth))
    .await?;
  append_log(
    &context.app_log_path,
    &format!(
      "submission_collection_switch_ok season_id={} section_id={} aid={}",
      season_id, section_id, aid
    ),
  );
  Ok(())
}

async fn fetch_collection_section_id(
  context: &UploadContext,
  auth: &AuthInfo,
  season_id: i64,
) -> Option<i64> {
  let url = "https://member.bilibili.com/x2/creative/web/seasons";
  let params = vec![
    ("pn".to_string(), "1".to_string()),
    ("ps".to_string(), "100".to_string()),
    ("order".to_string(), "desc".to_string()),
    ("sort".to_string(), "mtime".to_string()),
    ("filter".to_string(), "1".to_string()),
  ];
  let data = context.bilibili.get_json(url, &params, Some(auth), false).await.ok()?;
  let seasons = data.get("seasons").and_then(|value| value.as_array())?;
  for item in seasons {
    let season = item.get("season")?;
    let id = season.get("id").and_then(|value| value.as_i64())?;
    if id != season_id {
      continue;
    }
    let sections = item
      .get("sections")
      .and_then(|value| value.get("sections"))
      .and_then(|value| value.as_array())
      .and_then(|list| list.first())
      .and_then(|section| section.get("id"))
      .and_then(|value| value.as_i64());
    return Some(sections.unwrap_or(0));
  }
  None
}


fn build_headers(cookie: Option<&str>) -> Result<HeaderMap, String> {
  let mut headers = HeaderMap::new();
  headers.insert(
    USER_AGENT,
    HeaderValue::from_static(
      "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/132.0.0.0 Safari/537.36 Edg/132.0.0.0",
    ),
  );
  headers.insert(ACCEPT, HeaderValue::from_static("application/json, text/javascript, */*; q=0.01"));
  headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("zh-CN"));
  if let Some(cookie) = cookie {
    if !cookie.trim().is_empty() {
      headers.insert(
        "Cookie",
        HeaderValue::from_str(cookie).map_err(|_| "无效的Cookie".to_string())?,
      );
    }
  }
  Ok(headers)
}

fn load_source_videos(
  context: &SubmissionContext,
  task_id: &str,
) -> Result<Vec<ClipSource>, String> {
  context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT source_file_path, start_time, end_time, sort_order FROM task_source_video WHERE task_id = ?1 ORDER BY sort_order ASC",
      )?;
      let rows = stmt.query_map([task_id], |row| {
        Ok(ClipSource {
          input_path: row.get(0)?,
          start_time: row.get(1)?,
          end_time: row.get(2)?,
          order: row.get(3)?,
        })
      })?;

      let list = rows.collect::<Result<Vec<_>, _>>()?;
      Ok(list)
    })
    .map_err(|err| err.to_string())
}

fn load_task_source_video_records(
  context: &SubmissionContext,
  task_id: &str,
) -> Result<Vec<TaskSourceVideoRecord>, String> {
  context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT id, task_id, source_file_path, sort_order, start_time, end_time \
         FROM task_source_video WHERE task_id = ?1 ORDER BY sort_order ASC",
      )?;
      let rows = stmt.query_map([task_id], |row| {
        Ok(TaskSourceVideoRecord {
          id: row.get(0)?,
          task_id: row.get(1)?,
          source_file_path: row.get(2)?,
          sort_order: row.get(3)?,
          start_time: row.get(4)?,
          end_time: row.get(5)?,
        })
      })?;
      let list = rows.collect::<Result<Vec<_>, _>>()?;
      Ok(list)
    })
    .map_err(|err| err.to_string())
}

fn normalize_binding_sources(mut sources: Vec<ClipSource>) -> Vec<ClipSource> {
  let mut seen = HashSet::new();
  sources.retain(|source| {
    let key = (source.input_path.clone(), source.order);
    seen.insert(key)
  });
  for (index, source) in sources.iter_mut().enumerate() {
    source.order = (index + 1) as i64;
  }
  sources
}

fn save_merged_source_bindings(
  context: &SubmissionContext,
  task_id: &str,
  merged_id: i64,
  sources: &[ClipSource],
) -> Result<(), String> {
  if sources.is_empty() {
    return Ok(());
  }
  let source_records = load_task_source_video_records(context, task_id)?;
  let mut records_by_key: HashMap<(String, i64), String> = HashMap::new();
  let mut records_by_path: HashMap<String, String> = HashMap::new();
  for record in source_records {
    records_by_key.insert(
      (record.source_file_path.clone(), record.sort_order),
      record.id.clone(),
    );
    records_by_path.entry(record.source_file_path).or_insert(record.id);
  }
  let now = now_rfc3339();
  let mut seen = HashSet::new();
  let mut normalized_sources = Vec::new();
  for source in sources {
    let key = (source.input_path.clone(), source.order);
    if seen.insert(key) {
      normalized_sources.push(source.clone());
    }
  }
  context
    .db
    .with_conn(|conn| {
      conn.execute(
        "DELETE FROM merged_source_video WHERE merged_id = ?1",
        [merged_id],
      )?;
      for source in &normalized_sources {
        if source.input_path.trim().is_empty() {
          continue;
        }
        let source_id = records_by_key
          .get(&(source.input_path.clone(), source.order))
          .or_else(|| records_by_path.get(&source.input_path))
          .cloned();
        conn.execute(
          "INSERT INTO merged_source_video (task_id, merged_id, source_id, source_file_path, sort_order, start_time, end_time, create_time, update_time) \
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
          (
            task_id,
            merged_id,
            source_id,
            &source.input_path,
            source.order,
            source.start_time.as_deref(),
            source.end_time.as_deref(),
            &now,
            &now,
          ),
        )?;
      }
      Ok(())
    })
    .map_err(|err| err.to_string())
}

fn load_merged_source_clips(
  context: &SubmissionContext,
  task_id: &str,
  merged_id: i64,
) -> Result<Vec<ClipSource>, String> {
  context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT source_file_path, start_time, end_time, sort_order \
         FROM merged_source_video WHERE task_id = ?1 AND merged_id = ?2 \
         ORDER BY sort_order ASC",
      )?;
      let rows = stmt.query_map((task_id, merged_id), |row| {
        Ok(ClipSource {
          input_path: row.get(0)?,
          start_time: row.get(1)?,
          end_time: row.get(2)?,
          order: row.get(3)?,
        })
      })?;
      let list = rows.collect::<Result<Vec<_>, _>>()?;
      Ok(list)
    })
    .map_err(|err| err.to_string())
}

fn load_merged_source_paths(
  context: &SubmissionContext,
  task_id: &str,
  merged_id: i64,
) -> Result<Vec<String>, String> {
  let sources = load_merged_source_clips(context, task_id, merged_id)?;
  Ok(
    sources
      .into_iter()
      .map(|source| source.input_path)
      .filter(|value| !value.trim().is_empty())
      .collect(),
  )
}

fn collect_sources_for_merge_all(
  context: &SubmissionContext,
  task_id: &str,
  merged_videos: &[MergedVideoRecord],
) -> Vec<ClipSource> {
  let mut sources = Vec::new();
  for merged in merged_videos {
    if let Ok(mut merged_sources) = load_merged_source_clips(context, task_id, merged.id) {
      sources.append(&mut merged_sources);
    }
  }
  if sources.is_empty() {
    if let Ok(fallback) = load_source_videos(context, task_id) {
      return normalize_binding_sources(fallback);
    }
  }
  normalize_binding_sources(sources)
}

fn resolve_merged_target_path(
  base_dir: &Path,
  merged: &MergedVideoRecord,
) -> Option<PathBuf> {
  if let Some(path) = merged.video_path.as_deref() {
    if !path.trim().is_empty() {
      return Some(PathBuf::from(path));
    }
  }
  if let Some(file_name) = merged.file_name.as_deref() {
    if !file_name.trim().is_empty() {
      return Some(base_dir.join("merge").join(file_name));
    }
  }
  None
}

fn update_merged_video_path(
  context: &SubmissionContext,
  merged_id: i64,
  target_path: &Path,
) -> Result<(), String> {
  let now = now_rfc3339();
  let file_name = target_path
    .file_name()
    .and_then(|name| name.to_str())
    .unwrap_or("merged.mp4")
    .to_string();
  let path_str = target_path.to_string_lossy().to_string();
  context
    .db
    .with_conn(|conn| {
      conn.execute(
        "UPDATE merged_video SET file_name = ?1, video_path = ?2, update_time = ?3 WHERE id = ?4",
        (&file_name, &path_str, &now, merged_id),
      )?;
      Ok(())
    })
    .map_err(|err| err.to_string())
}

fn rebuild_missing_merged_video(
  context: &SubmissionContext,
  task_id: &str,
  merged: &MergedVideoRecord,
  sources: &[ClipSource],
  base_dir: &Path,
) -> Result<PathBuf, String> {
  if sources.is_empty() {
    return Err("源视频为空，无法重建合并视频".to_string());
  }
  let target_path =
    resolve_merged_target_path(base_dir, merged).ok_or_else(|| "合并视频路径为空".to_string())?;
  if let Some(parent) = target_path.parent() {
    fs::create_dir_all(parent).map_err(|err| format!("创建合并目录失败: {}", err))?;
  }
  let rebuild_dir = base_dir
    .join("resegment")
    .join(sanitize_filename(&format!("rebuild_{}", now_rfc3339())))
    .join("cut");
  let copy_decision = decide_clip_copy(sources).unwrap_or(crate::processing::ClipCopyDecision {
    use_copy: false,
    reason: Some("rebuild_copy_decision_failed".to_string()),
  });
  let clip_outputs = clip_sources(sources, &rebuild_dir, copy_decision.use_copy)?;
  merge_files(&clip_outputs, &target_path)?;
  update_merged_video_path(context, merged.id, &target_path)?;
  save_merged_source_bindings(context, task_id, merged.id, sources)?;
  Ok(target_path)
}

fn copy_merged_source_bindings(
  context: &SubmissionContext,
  task_id: &str,
  from_merged_id: i64,
  to_merged_id: i64,
) -> Result<(), String> {
  let sources = load_merged_source_clips(context, task_id, from_merged_id)?;
  if sources.is_empty() {
    return Ok(());
  }
  save_merged_source_bindings(context, task_id, to_merged_id, &sources)
}

fn load_latest_workflow_config(
  context: &SubmissionContext,
  task_id: &str,
) -> Result<Option<Value>, String> {
  context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT wc.configuration_data FROM workflow_instances wi \
         JOIN workflow_configurations wc ON wi.configuration_id = wc.config_id \
         WHERE wi.task_id = ?1 ORDER BY wi.created_at DESC LIMIT 1",
      )?;
      let result: Option<String> = stmt.query_row([task_id], |row| row.get(0)).ok();
      Ok(result)
    })
    .map_err(|err| err.to_string())
    .map(|value| value.and_then(|raw| serde_json::from_str::<Value>(&raw).ok()))
}

fn load_update_sources(
  context: &SubmissionContext,
  task_id: &str,
) -> Result<Option<Vec<ClipSource>>, String> {
  let config = load_latest_workflow_config(context, task_id)?;
  let Some(config) = config else {
    return Ok(None);
  };
  let Some(list) = config.get("updateSources").and_then(|value| value.as_array()) else {
    return Ok(None);
  };
  let mut sources = Vec::new();
  for (index, item) in list.iter().enumerate() {
    let input_path = item
      .get("sourceFilePath")
      .or_else(|| item.get("source_file_path"))
      .and_then(|value| value.as_str())
      .unwrap_or("")
      .trim()
      .to_string();
    if input_path.is_empty() {
      continue;
    }
    let start_time = item
      .get("startTime")
      .or_else(|| item.get("start_time"))
      .and_then(|value| value.as_str())
      .map(|value| value.trim().to_string())
      .filter(|value| !value.is_empty());
    let end_time = item
      .get("endTime")
      .or_else(|| item.get("end_time"))
      .and_then(|value| value.as_str())
      .map(|value| value.trim().to_string())
      .filter(|value| !value.is_empty());
    let order = item
      .get("sortOrder")
      .or_else(|| item.get("sort_order"))
      .and_then(|value| value.as_i64())
      .unwrap_or((index + 1) as i64);
    sources.push(ClipSource {
      input_path,
      start_time,
      end_time,
      order,
    });
  }
  if sources.is_empty() {
    return Ok(None);
  }
  sources.sort_by_key(|item| item.order);
  Ok(Some(sources))
}

#[allow(dead_code)]
fn replace_source_videos(
  context: &SubmissionContext,
  task_id: &str,
  sources: &[SourceVideoInput],
) -> Result<(), String> {
  context
    .db
    .with_conn(|conn| {
      conn.execute("DELETE FROM task_source_video WHERE task_id = ?1", [task_id])?;
      for source in sources {
        let source_id = uuid::Uuid::new_v4().to_string();
        conn.execute(
          "INSERT INTO task_source_video (id, task_id, source_file_path, sort_order, start_time, end_time) \
           VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
          (
            source_id,
            task_id,
            &source.source_file_path,
            source.sort_order,
            source.start_time.as_deref(),
            source.end_time.as_deref(),
          ),
        )?;
      }
      Ok(())
    })
    .map_err(|err| err.to_string())
}

fn append_source_videos(
  context: &SubmissionContext,
  task_id: &str,
  sources: &[SourceVideoInput],
) -> Result<(), String> {
  context
    .db
    .with_conn(|conn| {
      let base_order: i64 = conn
        .query_row(
          "SELECT COALESCE(MAX(sort_order), 0) FROM task_source_video WHERE task_id = ?1",
          [task_id],
          |row| row.get(0),
        )
        .unwrap_or(0);
      for (index, source) in sources.iter().enumerate() {
        let source_id = uuid::Uuid::new_v4().to_string();
        let sort_order = base_order + index as i64 + 1;
        conn.execute(
          "INSERT INTO task_source_video (id, task_id, source_file_path, sort_order, start_time, end_time) \
           VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
          (
            source_id,
            task_id,
            &source.source_file_path,
            sort_order,
            source.start_time.as_deref(),
            source.end_time.as_deref(),
          ),
        )?;
      }
      Ok(())
    })
    .map_err(|err| err.to_string())
}

fn attach_update_sources(config: Value, sources: &[SourceVideoInput]) -> Value {
  let list = sources
    .iter()
    .enumerate()
    .map(|(index, source)| {
      let mut map = Map::new();
      map.insert(
        "sourceFilePath".to_string(),
        Value::String(source.source_file_path.clone()),
      );
      if let Some(start) = source.start_time.as_deref() {
        let trimmed = start.trim();
        if !trimmed.is_empty() {
          map.insert("startTime".to_string(), Value::String(trimmed.to_string()));
        }
      }
      if let Some(end) = source.end_time.as_deref() {
        let trimmed = end.trim();
        if !trimmed.is_empty() {
          map.insert("endTime".to_string(), Value::String(trimmed.to_string()));
        }
      }
      map.insert(
        "sortOrder".to_string(),
        Value::Number(Number::from(
          source.sort_order.max(1).max(index as i64 + 1),
        )),
      );
      Value::Object(map)
    })
    .collect::<Vec<_>>();
  match config {
    Value::Object(mut map) => {
      map.insert("updateSources".to_string(), Value::Array(list));
      Value::Object(map)
    }
    _ => {
      let mut map = Map::new();
      map.insert("updateSources".to_string(), Value::Array(list));
      Value::Object(map)
    }
  }
}

fn save_video_clips(
  context: &SubmissionContext,
  task_id: &str,
  sources: &[ClipSource],
  outputs: &[PathBuf],
  replace_existing: bool,
) -> Result<(), String> {
  let now = now_rfc3339();
  context
    .db
    .with_conn(|conn| {
      if replace_existing {
        conn.execute("DELETE FROM video_clip WHERE task_id = ?1", [task_id])?;
      }
      for (index, output) in outputs.iter().enumerate() {
        let source = sources.get(index).cloned();
        conn.execute(
          "INSERT INTO video_clip (task_id, file_name, start_time, end_time, clip_path, sequence, status, create_time, update_time) \
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, 2, ?7, ?8)",
          (
            task_id,
            output.file_name().and_then(|name| name.to_str()).unwrap_or("clip.mp4"),
            source.as_ref().and_then(|s| s.start_time.as_deref()),
            source.as_ref().and_then(|s| s.end_time.as_deref()),
            output.to_string_lossy().to_string(),
            (index + 1) as i64,
            &now,
            &now,
          ),
        )?;
      }
      Ok(())
    })
    .map_err(|err| err.to_string())
}

fn save_merged_video(
  context: &SubmissionContext,
  task_id: &str,
  merged_path: &Path,
) -> Result<i64, String> {
  let now = now_rfc3339();
  let file_name = merged_path
    .file_name()
    .and_then(|name| name.to_str())
    .unwrap_or("merged.mp4");
  let total_bytes = fs::metadata(merged_path).map(|meta| meta.len()).unwrap_or(0);

  context
    .db
    .with_conn(|conn| {
      conn.execute(
        "INSERT INTO merged_video (task_id, file_name, video_path, duration, status, upload_progress, upload_uploaded_bytes, upload_total_bytes, upload_cid, upload_file_name, upload_session_id, upload_biz_id, upload_endpoint, upload_auth, upload_uri, upload_chunk_size, upload_last_part_index, create_time, update_time) \
         VALUES (?1, ?2, ?3, NULL, 2, 0, 0, ?4, NULL, NULL, NULL, 0, NULL, NULL, NULL, 0, 0, ?5, ?6)",
        (
          task_id,
          file_name,
          merged_path.to_string_lossy().to_string(),
          total_bytes as i64,
          &now,
          &now,
        ),
      )?;
      Ok(conn.last_insert_rowid())
    })
    .map_err(|err| err.to_string())
}

fn load_output_segment_stats(
  context: &SubmissionContext,
  task_id: &str,
) -> Result<(usize, i64), String> {
  context
    .db
    .with_conn(|conn| {
      conn.query_row(
        "SELECT COUNT(*), COALESCE(MAX(part_order), 0) FROM task_output_segment WHERE task_id = ?1",
        [task_id],
        |row| {
          let count: i64 = row.get(0)?;
          let max_order: i64 = row.get(1)?;
          Ok((count.max(0) as usize, max_order.max(0)))
        },
      )
    })
    .map_err(|err| err.to_string())
}

fn resolve_update_name_start_index(
  context: &SubmissionContext,
  task_id: &str,
  existing_count: usize,
  prefix: Option<&str>,
) -> Result<usize, String> {
  let prefix = prefix.unwrap_or("").trim();
  if !prefix.is_empty() {
    return Ok(1);
  }
  if existing_count > 0 {
    if let Ok(Some(max_index)) = load_max_part_index_from_names(context, task_id) {
      append_log(
        &context.app_log_path,
        &format!(
          "submission_update_name_start_index task_id={} source=part_name max_index={}",
          task_id, max_index
        ),
      );
      return Ok(max_index + 1);
    }
    if let Ok(Some(max_order)) = load_max_part_order(context, task_id) {
      append_log(
        &context.app_log_path,
        &format!(
          "submission_update_name_start_index task_id={} source=part_order max_order={}",
          task_id, max_order
        ),
      );
      return Ok(max_order + 1);
    }
    append_log(
      &context.app_log_path,
      &format!(
        "submission_update_name_start_index task_id={} source=existing_count value={}",
        task_id, existing_count
      ),
    );
    return Ok(existing_count + 1);
  }
  let has_uploaded_merged = context
    .db
    .with_conn(|conn| {
      let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM merged_video WHERE task_id = ?1 AND upload_cid IS NOT NULL AND upload_cid > 0",
        [task_id],
        |row| row.get(0),
      )?;
      Ok(count > 0)
    })
    .map_err(|err| err.to_string())?;
  if has_uploaded_merged {
    return Ok(2);
  }
  Ok(1)
}

fn load_max_part_index_from_names(
  context: &SubmissionContext,
  task_id: &str,
) -> Result<Option<usize>, String> {
  context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT part_name FROM task_output_segment WHERE task_id = ?1",
      )?;
      let rows = stmt.query_map([task_id], |row| row.get::<_, String>(0))?;
      let mut max_index: Option<usize> = None;
      for row in rows {
        let name = row?;
        if let Some(index) = parse_part_index(&name) {
          max_index = Some(max_index.map_or(index, |current| current.max(index)));
        }
      }
      Ok(max_index)
    })
    .map_err(|err| err.to_string())
}

fn load_max_part_order(
  context: &SubmissionContext,
  task_id: &str,
) -> Result<Option<usize>, String> {
  context
    .db
    .with_conn(|conn| {
      let max_order: i64 = conn.query_row(
        "SELECT COALESCE(MAX(part_order), 0) FROM task_output_segment WHERE task_id = ?1",
        [task_id],
        |row| row.get(0),
      )?;
      if max_order <= 0 {
        return Ok(None);
      }
      Ok(Some(max_order as usize))
    })
    .map_err(|err| err.to_string())
}

fn parse_part_index(name: &str) -> Option<usize> {
  let trimmed = name.trim();
  if trimmed.is_empty() {
    return None;
  }
  if let Some(rest) = trimmed.strip_prefix('第') {
    let rest = rest.trim();
    let rest = rest.strip_suffix('P').or_else(|| rest.strip_suffix('p')).unwrap_or(rest);
    if let Some(value) = parse_leading_number(rest) {
      return Some(value);
    }
  }
  let upper = trimmed.to_uppercase();
  if let Some(rest) = upper.strip_prefix('P') {
    if let Some(value) = parse_leading_number(rest) {
      return Some(value);
    }
  }
  if let Some(rest) = upper.strip_prefix("PART") {
    if let Some(value) = parse_leading_number(rest) {
      return Some(value);
    }
  }
  let lower = trimmed.to_lowercase();
  if let Some(pos) = lower.rfind("part_") {
    let rest = &lower[(pos + "part_".len())..];
    if let Some(value) = parse_leading_number(rest) {
      return Some(value);
    }
  }
  None
}

fn parse_leading_number(value: &str) -> Option<usize> {
  let trimmed = value.trim().trim_start_matches(['_', '-', ' ']);
  let digits: String = trimmed.chars().take_while(|ch| ch.is_ascii_digit()).collect();
  if digits.is_empty() {
    return None;
  }
  digits.parse::<usize>().ok()
}

fn append_output_segments(
  context: &SubmissionContext,
  task_id: &str,
  segments: &[PathBuf],
  merged_id: Option<i64>,
  prefix: Option<&str>,
  part_order_start: i64,
  name_start_index: usize,
) -> Result<(), String> {
  let segments = rename_segment_outputs_with_prefix(segments, prefix, name_start_index)?;
  context
    .db
    .with_conn(|conn| {
      for (index, segment) in segments.iter().enumerate() {
        let segment_id = uuid::Uuid::new_v4().to_string();
        let file_name = segment.file_name().and_then(|name| name.to_str()).unwrap_or("segment.mp4");
        let total_bytes = fs::metadata(segment).map(|meta| meta.len()).unwrap_or(0);
        let part_order = part_order_start + index as i64;
        let part_name = build_part_title(prefix, name_start_index + index);
        conn.execute(
          "INSERT INTO task_output_segment (segment_id, task_id, merged_id, part_name, segment_file_path, part_order, upload_status, cid, file_name, upload_progress, upload_uploaded_bytes, upload_total_bytes, upload_session_id, upload_biz_id, upload_endpoint, upload_auth, upload_uri, upload_chunk_size, upload_last_part_index) \
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'PENDING', NULL, ?7, 0, 0, ?8, NULL, 0, NULL, NULL, NULL, 0, 0)",
          (
            segment_id,
            task_id,
            merged_id,
            part_name,
            segment.to_string_lossy().to_string(),
            part_order,
            file_name,
            total_bytes as i64,
          ),
        )?;
      }
      Ok(())
    })
    .map_err(|err| err.to_string())
}

fn save_output_segments(
  context: &SubmissionContext,
  task_id: &str,
  segments: &[PathBuf],
  merged_id: Option<i64>,
  prefix: Option<&str>,
) -> Result<(), String> {
  let segments = rename_segment_outputs_with_prefix(segments, prefix, 1)?;
  let has_prefix = prefix.map(|value| !value.trim().is_empty()).unwrap_or(false);
  context
    .db
    .with_conn(|conn| {
      conn.execute("DELETE FROM task_output_segment WHERE task_id = ?1", [task_id])?;
      for (index, segment) in segments.iter().enumerate() {
        let segment_id = uuid::Uuid::new_v4().to_string();
        let file_name = segment.file_name().and_then(|name| name.to_str()).unwrap_or("segment.mp4");
        let total_bytes = fs::metadata(segment).map(|meta| meta.len()).unwrap_or(0);
        let part_name = if has_prefix {
          build_part_title(prefix, index + 1)
        } else {
          format!("Part {}", index + 1)
        };
        conn.execute(
          "INSERT INTO task_output_segment (segment_id, task_id, merged_id, part_name, segment_file_path, part_order, upload_status, cid, file_name, upload_progress, upload_uploaded_bytes, upload_total_bytes, upload_session_id, upload_biz_id, upload_endpoint, upload_auth, upload_uri, upload_chunk_size, upload_last_part_index) \
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'PENDING', NULL, ?7, 0, 0, ?8, NULL, 0, NULL, NULL, NULL, 0, 0)",
          (
            segment_id,
            task_id,
            merged_id,
            part_name,
            segment.to_string_lossy().to_string(),
            (index + 1) as i64,
            file_name,
            total_bytes as i64,
          ),
        )?;
      }
      Ok(())
    })
    .map_err(|err| err.to_string())
}

fn update_upload_progress(
  context: &SubmissionContext,
  target: &UploadTarget,
  snapshot: &UploadProgressSnapshot,
) -> Result<(), String> {
  match target {
    UploadTarget::Segment(segment_id) => context
      .db
      .with_conn(|conn| {
        conn.execute(
          "UPDATE task_output_segment SET upload_progress = ?1, upload_uploaded_bytes = ?2, upload_total_bytes = ?3, upload_last_part_index = ?4 WHERE segment_id = ?5",
          (
            snapshot.progress,
            snapshot.uploaded_bytes as i64,
            snapshot.total_bytes as i64,
            snapshot.last_part_index as i64,
            segment_id,
          ),
        )?;
        Ok(())
      })
      .map_err(|err| err.to_string()),
    UploadTarget::Merged(merged_id) => context
      .db
      .with_conn(|conn| {
        conn.execute(
          "UPDATE merged_video SET upload_progress = ?1, upload_uploaded_bytes = ?2, upload_total_bytes = ?3, upload_last_part_index = ?4 WHERE id = ?5",
          (
            snapshot.progress,
            snapshot.uploaded_bytes as i64,
            snapshot.total_bytes as i64,
            snapshot.last_part_index as i64,
            merged_id,
          ),
        )?;
        Ok(())
      })
      .map_err(|err| err.to_string()),
    UploadTarget::EditSegment(segment_id) => update_edit_upload_segment(
      context,
      segment_id,
      |segment| {
        segment.upload_progress = snapshot.progress;
        segment.upload_uploaded_bytes = snapshot.uploaded_bytes as i64;
        segment.upload_total_bytes = snapshot.total_bytes as i64;
        segment.upload_last_part_index = snapshot.last_part_index as i64;
      },
    ),
  }
}

fn update_upload_status_for_target(
  context: &SubmissionContext,
  target: &UploadTarget,
  status: &str,
) -> Result<(), String> {
  match target {
    UploadTarget::Segment(segment_id) => update_segment_upload_status(context, segment_id, status),
    UploadTarget::Merged(_) => Ok(()),
    UploadTarget::EditSegment(segment_id) => update_edit_upload_segment(
      context,
      segment_id,
      |segment| {
        segment.upload_status = status.to_string();
      },
    ),
  }
}

fn restore_upload_status_after_rate_limit(
  context: &SubmissionContext,
  target: &UploadTarget,
) -> Result<(), String> {
  match target {
    UploadTarget::Segment(segment_id) => {
      let segment = load_output_segment_by_id(context, segment_id)?;
      if let Some(segment) = segment {
        if segment.upload_status == "RATE_LIMITED" {
          return update_segment_upload_status(context, segment_id, "UPLOADING");
        }
      }
      Ok(())
    }
    UploadTarget::Merged(_) => Ok(()),
    UploadTarget::EditSegment(segment_id) => {
      let segment = load_edit_upload_segment(context, segment_id)?;
      if let Some(segment) = segment {
        if segment.upload_status == "RATE_LIMITED" {
          return update_edit_upload_segment(context, segment_id, |segment| {
            segment.upload_status = "UPLOADING".to_string();
          });
        }
      }
      Ok(())
    }
  }
}

fn update_upload_session(
  context: &SubmissionContext,
  target: &UploadTarget,
  session: &UploadSessionInfo,
) -> Result<(), String> {
  let progress = if session.total_bytes > 0 {
    (session.uploaded_bytes as f64 / session.total_bytes as f64) * 100.0
  } else {
    0.0
  };
  match target {
    UploadTarget::Segment(segment_id) => context
      .db
      .with_conn(|conn| {
        conn.execute(
          "UPDATE task_output_segment SET upload_session_id = ?1, upload_biz_id = ?2, upload_endpoint = ?3, upload_auth = ?4, upload_uri = ?5, upload_chunk_size = ?6, upload_uploaded_bytes = ?7, upload_total_bytes = ?8, upload_progress = ?9, upload_last_part_index = ?10 WHERE segment_id = ?11",
          (
            &session.upload_id,
            session.biz_id,
            &session.endpoint,
            &session.auth,
            &session.upos_uri,
            session.chunk_size as i64,
            session.uploaded_bytes as i64,
            session.total_bytes as i64,
            progress,
            session.last_part_index as i64,
            segment_id,
          ),
        )?;
        Ok(())
      })
      .map_err(|err| err.to_string()),
    UploadTarget::Merged(merged_id) => context
      .db
      .with_conn(|conn| {
        conn.execute(
          "UPDATE merged_video SET upload_session_id = ?1, upload_biz_id = ?2, upload_endpoint = ?3, upload_auth = ?4, upload_uri = ?5, upload_chunk_size = ?6, upload_uploaded_bytes = ?7, upload_total_bytes = ?8, upload_progress = ?9, upload_last_part_index = ?10 WHERE id = ?11",
          (
            &session.upload_id,
            session.biz_id,
            &session.endpoint,
            &session.auth,
            &session.upos_uri,
            session.chunk_size as i64,
            session.uploaded_bytes as i64,
            session.total_bytes as i64,
            progress,
            session.last_part_index as i64,
            merged_id,
          ),
        )?;
        Ok(())
      })
      .map_err(|err| err.to_string()),
    UploadTarget::EditSegment(segment_id) => update_edit_upload_segment(
      context,
      segment_id,
      |segment| {
        segment.upload_session_id = Some(session.upload_id.clone());
        segment.upload_biz_id = session.biz_id;
        segment.upload_endpoint = Some(session.endpoint.clone());
        segment.upload_auth = Some(session.auth.clone());
        segment.upload_uri = Some(session.upos_uri.clone());
        segment.upload_chunk_size = session.chunk_size as i64;
        segment.upload_uploaded_bytes = session.uploaded_bytes as i64;
        segment.upload_total_bytes = session.total_bytes as i64;
        segment.upload_progress = progress;
        segment.upload_last_part_index = session.last_part_index as i64;
      },
    ),
  }
}

fn clear_upload_session(context: &SubmissionContext, target: &UploadTarget) -> Result<(), String> {
  match target {
    UploadTarget::Segment(segment_id) => context
      .db
      .with_conn(|conn| {
        conn.execute(
          "UPDATE task_output_segment SET upload_session_id = NULL, upload_biz_id = 0, upload_endpoint = NULL, upload_auth = NULL, upload_uri = NULL, upload_chunk_size = 0, upload_uploaded_bytes = 0, upload_total_bytes = 0, upload_progress = 0, upload_last_part_index = 0 WHERE segment_id = ?1",
          [segment_id],
        )?;
        Ok(())
      })
      .map_err(|err| err.to_string()),
    UploadTarget::Merged(merged_id) => context
      .db
      .with_conn(|conn| {
        conn.execute(
          "UPDATE merged_video SET upload_session_id = NULL, upload_biz_id = 0, upload_endpoint = NULL, upload_auth = NULL, upload_uri = NULL, upload_chunk_size = 0, upload_uploaded_bytes = 0, upload_total_bytes = 0, upload_progress = 0, upload_last_part_index = 0 WHERE id = ?1",
          [merged_id],
        )?;
        Ok(())
      })
      .map_err(|err| err.to_string()),
    UploadTarget::EditSegment(segment_id) => update_edit_upload_segment(
      context,
      segment_id,
      |segment| {
        segment.upload_session_id = None;
        segment.upload_biz_id = 0;
        segment.upload_endpoint = None;
        segment.upload_auth = None;
        segment.upload_uri = None;
        segment.upload_chunk_size = 0;
        segment.upload_uploaded_bytes = 0;
        segment.upload_total_bytes = 0;
        segment.upload_progress = 0.0;
        segment.upload_last_part_index = 0;
      },
    ),
  }
}

fn reset_segments_for_new_bvid(
  context: &SubmissionContext,
  task_id: &str,
) -> Result<(), String> {
  context
    .db
    .with_conn(|conn| {
      conn.execute(
        "UPDATE task_output_segment SET upload_status = 'PENDING', cid = NULL, upload_progress = 0, upload_uploaded_bytes = 0, upload_total_bytes = 0, upload_session_id = NULL, upload_biz_id = 0, upload_endpoint = NULL, upload_auth = NULL, upload_uri = NULL, upload_chunk_size = 0, upload_last_part_index = 0 WHERE task_id = ?1",
        [task_id],
      )?;
      Ok(())
    })
    .map_err(|err| err.to_string())
}

fn reset_segments_without_upload_session(
  context: &SubmissionContext,
  task_id: &str,
) -> Result<usize, String> {
  context
    .db
    .with_conn(|conn| {
      let affected = conn.execute(
        "UPDATE task_output_segment SET upload_status = 'PENDING', cid = NULL, file_name = NULL, upload_progress = 0, upload_uploaded_bytes = 0, upload_total_bytes = 0, upload_session_id = NULL, upload_biz_id = 0, upload_endpoint = NULL, upload_auth = NULL, upload_uri = NULL, upload_chunk_size = 0, upload_last_part_index = 0 WHERE task_id = ?1 AND upload_status = 'SUCCESS' AND (upload_biz_id IS NULL OR upload_biz_id = 0) AND (upload_session_id IS NULL OR TRIM(upload_session_id) = '') AND (upload_uri IS NULL OR TRIM(upload_uri) = '')",
        [task_id],
      )?;
      Ok(affected)
    })
    .map_err(|err| err.to_string())
}

fn update_segment_upload_result(
  context: &SubmissionContext,
  segment_id: &str,
  status: &str,
  cid: Option<i64>,
  file_name: Option<String>,
) -> Result<(), String> {
  context
    .db
    .with_conn(|conn| {
      conn.execute(
        "UPDATE task_output_segment SET upload_status = ?1, cid = ?2, file_name = ?3 WHERE segment_id = ?4",
        (status, cid, file_name, segment_id),
      )?;
      Ok(())
    })
    .map_err(|err| err.to_string())
}

fn update_merged_upload_result(
  context: &SubmissionContext,
  merged_id: i64,
  cid: Option<i64>,
  file_name: Option<String>,
) -> Result<(), String> {
  context
    .db
    .with_conn(|conn| {
      conn.execute(
        "UPDATE merged_video SET upload_cid = ?1, upload_file_name = ?2 WHERE id = ?3",
        (cid, file_name, merged_id),
      )?;
      Ok(())
    })
    .map_err(|err| err.to_string())
}

fn update_segment_upload_status(
  context: &SubmissionContext,
  segment_id: &str,
  status: &str,
) -> Result<(), String> {
  context
    .db
    .with_conn(|conn| {
      conn.execute(
        "UPDATE task_output_segment SET upload_status = ?1 WHERE segment_id = ?2",
        (status, segment_id),
      )?;
      Ok(())
    })
    .map_err(|err| err.to_string())
}

fn count_incomplete_segments(
  context: &SubmissionContext,
  task_id: &str,
) -> Result<i64, String> {
  context
    .db
    .with_conn(|conn| {
      let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM task_output_segment WHERE task_id = ?1 AND upload_status != 'SUCCESS'",
        [task_id],
        |row| row.get(0),
      )?;
      Ok(count)
    })
    .map_err(|err| err.to_string())
}

fn load_output_segment_by_id(
  context: &SubmissionContext,
  segment_id: &str,
) -> Result<Option<TaskOutputSegmentRecord>, String> {
  context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT segment_id, task_id, merged_id, part_name, segment_file_path, part_order, upload_status, cid, file_name, \
                upload_progress, upload_uploaded_bytes, upload_total_bytes, upload_session_id, upload_biz_id, \
                upload_endpoint, upload_auth, upload_uri, upload_chunk_size, upload_last_part_index \
         FROM task_output_segment WHERE segment_id = ?1",
      )?;
      let result = stmt
        .query_row([segment_id], |row| {
          Ok(TaskOutputSegmentRecord {
            segment_id: row.get(0)?,
            task_id: row.get(1)?,
            merged_id: row.get(2)?,
            part_name: row.get(3)?,
            segment_file_path: row.get(4)?,
            part_order: row.get(5)?,
            upload_status: row.get(6)?,
            cid: row.get(7)?,
            file_name: row.get(8)?,
            upload_progress: row.get(9)?,
            upload_uploaded_bytes: row.get(10)?,
            upload_total_bytes: row.get(11)?,
            upload_session_id: row.get(12)?,
            upload_biz_id: row.get(13)?,
            upload_endpoint: row.get(14)?,
            upload_auth: row.get(15)?,
            upload_uri: row.get(16)?,
            upload_chunk_size: row.get(17)?,
            upload_last_part_index: row.get(18)?,
          })
        })
        .ok();
      Ok(result)
    })
    .map_err(|err| err.to_string())
}

fn default_part_name_from_path(path: &str) -> String {
  let name = Path::new(path)
    .file_stem()
    .and_then(|value| value.to_str())
    .unwrap_or("P")
    .trim();
  if name.is_empty() {
    "P".to_string()
  } else {
    name.to_string()
  }
}

fn normalize_optional_text(value: Option<String>) -> Option<String> {
  value.and_then(|raw| {
    let trimmed = raw.trim().to_string();
    if trimmed.is_empty() {
      None
    } else {
      Some(trimmed)
    }
  })
}

pub fn normalize_baidu_sync_filename(value: Option<&str>) -> Option<String> {
  let trimmed = value.unwrap_or("").trim();
  if trimmed.is_empty() {
    return None;
  }
  if trimmed.to_ascii_lowercase().ends_with(".mp4") {
    return Some(trimmed.to_string());
  }
  Some(format!("{}.mp4", trimmed))
}

fn update_submission_task_for_edit(
  context: &SubmissionContext,
  task_id: &str,
  task: &SubmissionTaskRecord,
) -> Result<(), String> {
  let now = now_rfc3339();
  context
    .db
    .with_conn(|conn| {
      conn.execute(
        "UPDATE submission_task SET title = ?1, description = ?2, partition_id = ?3, tags = ?4, topic_id = ?5, mission_id = ?6, activity_title = ?7, video_type = ?8, collection_id = ?9, segment_prefix = ?10, updated_at = ?11 WHERE task_id = ?12",
        (
          &task.title,
          task.description.as_deref(),
          task.partition_id,
          task.tags.as_deref(),
          task.topic_id,
          task.mission_id,
          task.activity_title.as_deref(),
          &task.video_type,
          task.collection_id,
          task.segment_prefix.as_deref(),
          &now,
          task_id,
        ),
      )?;
      Ok(())
    })
    .map_err(|err| err.to_string())
}

fn update_baidu_sync_config(
  context: &SubmissionContext,
  task_id: &str,
  enabled: Option<bool>,
  path: Option<String>,
  filename: Option<String>,
) -> Result<(), String> {
  if enabled.is_none() && path.is_none() && filename.is_none() {
    return Ok(());
  }
  let now = now_rfc3339();
  context
    .db
    .with_conn(|conn| {
      let (current_enabled, current_path, current_filename) = conn.query_row(
        "SELECT baidu_sync_enabled, baidu_sync_path, baidu_sync_filename FROM submission_task WHERE task_id = ?1",
        [task_id],
        |row| {
          let enabled: i64 = row.get(0)?;
          let path: Option<String> = row.get(1)?;
          let filename: Option<String> = row.get(2)?;
          Ok((enabled != 0, path, filename))
        },
      )?;
      let next_enabled = enabled.unwrap_or(current_enabled);
      let next_path = path.or(current_path);
      let normalized_filename = normalize_baidu_sync_filename(filename.as_deref());
      let next_filename = normalized_filename.or(current_filename);
      conn.execute(
        "UPDATE submission_task SET baidu_sync_enabled = ?1, baidu_sync_path = ?2, baidu_sync_filename = ?3, updated_at = ?4 WHERE task_id = ?5",
        (
          if next_enabled { 1 } else { 0 },
          next_path.as_deref(),
          next_filename.as_deref(),
          &now,
          task_id,
        ),
      )?;
      Ok(())
    })
    .map_err(|err| err.to_string())
}

fn update_output_segments_for_edit(
  context: &SubmissionContext,
  task_id: &str,
  segments: &[SubmissionEditSegmentInput],
) -> Result<(), String> {
  let mut ordered = segments.to_vec();
  ordered.sort_by_key(|segment| segment.part_order);
  context
    .db
    .with_conn_mut(|conn| {
      let tx = conn.transaction()?;
      let mut keep_ids = HashSet::new();
      let existing_ids = {
        let mut stmt =
          tx.prepare("SELECT segment_id FROM task_output_segment WHERE task_id = ?1")?;
        let rows = stmt.query_map([task_id], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<HashSet<_>, _>>()?
      };
      for (index, segment) in ordered.iter().enumerate() {
        let part_order = (index + 1) as i64;
        let segment_id = segment.segment_id.trim();
        let part_name = segment.part_name.trim();
        let file_path = segment.segment_file_path.trim();
        let cid = segment.cid.unwrap_or(0);
        let file_name = segment
          .file_name
          .as_deref()
          .map(|value| value.trim())
          .unwrap_or("");
        let total_bytes = if file_path.is_empty() {
          0
        } else {
          fs::metadata(file_path)
            .map(|meta| meta.len() as i64)
            .unwrap_or(0)
        };
        if existing_ids.contains(segment_id) {
          tx.execute(
            "UPDATE task_output_segment SET part_name = ?1, part_order = ?2, segment_file_path = ?3, upload_status = 'SUCCESS', cid = ?4, file_name = ?5, upload_progress = 100, upload_uploaded_bytes = ?6, upload_total_bytes = ?7, upload_session_id = NULL, upload_biz_id = 0, upload_endpoint = NULL, upload_auth = NULL, upload_uri = NULL, upload_chunk_size = 0, upload_last_part_index = 0 WHERE segment_id = ?8 AND task_id = ?9",
            (
              part_name,
              part_order,
              file_path,
              cid,
              file_name,
              total_bytes,
              total_bytes,
              segment_id,
              task_id,
            ),
          )?;
        } else {
          tx.execute(
            "INSERT INTO task_output_segment (segment_id, task_id, merged_id, part_name, segment_file_path, part_order, upload_status, cid, file_name, upload_progress, upload_uploaded_bytes, upload_total_bytes, upload_session_id, upload_biz_id, upload_endpoint, upload_auth, upload_uri, upload_chunk_size, upload_last_part_index) \
             VALUES (?1, ?2, NULL, ?3, ?4, ?5, 'SUCCESS', ?6, ?7, 100, ?8, ?9, NULL, 0, NULL, NULL, NULL, 0, 0)",
            (
              segment_id,
              task_id,
              part_name,
              file_path,
              part_order,
              cid,
              file_name,
              total_bytes,
              total_bytes,
            ),
          )?;
        }
        keep_ids.insert(segment_id.to_string());
      }
      for segment_id in existing_ids {
        if !keep_ids.contains(&segment_id) {
          tx.execute(
            "DELETE FROM task_output_segment WHERE segment_id = ?1",
            [segment_id],
          )?;
        }
      }
      tx.commit()?;
      Ok(())
    })
    .map_err(|err| err.to_string())
}

async fn fetch_aid_by_bvid(
  context: &UploadContext,
  auth: Option<&AuthInfo>,
  bvid: &str,
) -> Option<i64> {
  let trimmed = bvid.trim();
  if trimmed.is_empty() {
    return None;
  }
  let url = format!("{}/x/web-interface/view", context.bilibili.base_url());
  let params = vec![("bvid".to_string(), trimmed.to_string())];
  let data = context
    .bilibili
    .get_json(&url, &params, auth, false)
    .await
    .ok()?;
  data.get("aid").and_then(|value| value.as_i64())
}

async fn fetch_aid_with_refresh(
  context: &UploadContext,
  auth: &AuthInfo,
  bvid: &str,
) -> Option<i64> {
  if let Some(aid) = fetch_aid_by_bvid(context, Some(auth), bvid).await {
    return Some(aid);
  }
  let refreshed = refresh_auth(context, "fetch_aid").await.ok()?;
  fetch_aid_by_bvid(context, Some(&refreshed), bvid).await
}

fn update_submission_bvid_and_aid(
  context: &SubmissionContext,
  task_id: &str,
  bvid: &str,
  aid: i64,
) -> Result<(), String> {
  let now = now_rfc3339();
  context
    .db
    .with_conn(|conn| {
      conn.execute(
        "UPDATE submission_task SET bvid = ?1, aid = ?2, updated_at = ?3 WHERE task_id = ?4",
        (bvid, aid, &now, task_id),
      )?;
      Ok(())
    })
    .map_err(|err| err.to_string())
}

fn update_submission_aid(
  context: &SubmissionContext,
  task_id: &str,
  aid: i64,
) -> Result<(), String> {
  let now = now_rfc3339();
  context
    .db
    .with_conn(|conn| {
      conn.execute(
        "UPDATE submission_task SET aid = ?1, updated_at = ?2 WHERE task_id = ?3",
        (aid, &now, task_id),
      )?;
      Ok(())
    })
    .map_err(|err| err.to_string())
}

fn load_task_status(context: &SubmissionContext, task_id: &str) -> Result<String, String> {
  context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare("SELECT status FROM submission_task WHERE task_id = ?1")?;
      let status = stmt.query_row([task_id], |row| row.get(0))?;
      Ok(status)
    })
    .map_err(|err| err.to_string())
}

struct IntegratedDownloadStats {
  total: i64,
  completed: i64,
  failed: i64,
}

fn load_integrated_download_stats(
  context: &SubmissionContext,
  task_id: &str,
) -> Result<Option<IntegratedDownloadStats>, String> {
  context
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
      let row = stmt.query_row([task_id], |row| {
        let total: i64 = row.get(0)?;
        let completed: Option<i64> = row.get(1)?;
        let failed: Option<i64> = row.get(2)?;
        Ok(IntegratedDownloadStats {
          total,
          completed: completed.unwrap_or(0),
          failed: failed.unwrap_or(0),
        })
      })?;
      if row.total == 0 {
        return Ok(None);
      }
      Ok(Some(row))
    })
    .map_err(|err| err.to_string())
}

fn ensure_editable_status(context: &SubmissionContext, task_id: &str) -> Result<(), String> {
  let status = load_task_status(context, task_id)?;
  if status == "UPLOADING" {
    return Err("任务正在投稿中，请稍后再试".to_string());
  }
  if status != "COMPLETED" {
    return Err("任务未完成，无法编辑".to_string());
  }
  Ok(())
}

fn ensure_editable_detail(detail: &SubmissionTaskDetail) -> Result<(), String> {
  if detail.task.status == "UPLOADING" {
    return Err("任务正在投稿中，请稍后再试".to_string());
  }
  if detail.task.status != "COMPLETED" {
    return Err("任务未完成，无法编辑".to_string());
  }
  if detail.task.bvid.as_deref().unwrap_or("").is_empty() {
    return Err("缺少BVID，无法编辑".to_string());
  }
  Ok(())
}

async fn load_auth_or_refresh(
  context: &UploadContext,
  reason: &str,
) -> Result<AuthInfo, String> {
  if let Some(auth) = context
    .login_store
    .load_auth_info(&context.db)
    .ok()
    .flatten()
  {
    return Ok(auth);
  }
  refresh_auth(context, reason).await
}

async fn refresh_auth(
  context: &UploadContext,
  reason: &str,
) -> Result<AuthInfo, String> {
  append_log(
    &context.app_log_path,
    &format!("submission_cookie_refresh_start reason={}", reason),
  );
  match login_refresh::refresh_cookie(
    &context.bilibili,
    &context.login_store,
    &context.db,
    &context.app_log_path,
  )
  .await
  {
    Ok(auth) => {
      append_log(
        &context.app_log_path,
        &format!("submission_cookie_refresh_ok reason={}", reason),
      );
      Ok(auth)
    }
    Err(err) => {
      append_log(
        &context.app_log_path,
        &format!("submission_cookie_refresh_fail reason={} err={}", reason, err),
      );
      Err(err)
    }
  }
}

fn is_auth_error(err: &str) -> bool {
  err.contains("code: -101")
    || err.contains("code: -111")
    || err.contains("code: 86095")
    || err.contains("账号未登录")
    || err.contains("请先登录")
}

fn load_auth_from_queue_context(
  context: &SubmissionQueueContext,
) -> Result<AuthInfo, String> {
  context
    .login_store
    .load_auth_info(&context.db)
    .ok()
    .flatten()
    .ok_or_else(|| "请先登录".to_string())
}

fn load_latest_merged_video(
  context: &SubmissionContext,
  task_id: &str,
) -> Result<Option<MergedVideoRecord>, String> {
  context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT id, task_id, file_name, video_path, remote_dir, remote_name, duration, status, \
                upload_progress, upload_uploaded_bytes, upload_total_bytes, upload_cid, upload_file_name, \
                upload_session_id, upload_biz_id, upload_endpoint, upload_auth, upload_uri, upload_chunk_size, \
                upload_last_part_index, create_time, update_time \
         FROM merged_video WHERE task_id = ?1 ORDER BY id DESC LIMIT 1",
      )?;
      let result = stmt
        .query_row([task_id], |row| {
          Ok(MergedVideoRecord {
            id: row.get(0)?,
            task_id: row.get(1)?,
            file_name: row.get(2)?,
            video_path: row.get(3)?,
            remote_dir: row.get(4)?,
            remote_name: row.get(5)?,
            duration: row.get(6)?,
            status: row.get(7)?,
            upload_progress: row.get(8)?,
            upload_uploaded_bytes: row.get(9)?,
            upload_total_bytes: row.get(10)?,
            upload_cid: row.get(11)?,
            upload_file_name: row.get(12)?,
            upload_session_id: row.get(13)?,
            upload_biz_id: row.get(14)?,
            upload_endpoint: row.get(15)?,
            upload_auth: row.get(16)?,
            upload_uri: row.get(17)?,
            upload_chunk_size: row.get(18)?,
            upload_last_part_index: row.get(19)?,
            create_time: row.get(20)?,
            update_time: row.get(21)?,
          })
        })
        .ok();
      Ok(result)
    })
    .map_err(|err| err.to_string())
}

fn load_merged_video_by_id(
  context: &SubmissionContext,
  task_id: &str,
  merged_id: i64,
) -> Result<Option<MergedVideoRecord>, String> {
  context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT id, task_id, file_name, video_path, remote_dir, remote_name, duration, status, \
                upload_progress, upload_uploaded_bytes, upload_total_bytes, upload_cid, upload_file_name, \
                upload_session_id, upload_biz_id, upload_endpoint, upload_auth, upload_uri, upload_chunk_size, \
                upload_last_part_index, create_time, update_time \
         FROM merged_video WHERE task_id = ?1 AND id = ?2",
      )?;
      let result = stmt
        .query_row((task_id, merged_id), |row| {
          Ok(MergedVideoRecord {
            id: row.get(0)?,
            task_id: row.get(1)?,
            file_name: row.get(2)?,
            video_path: row.get(3)?,
            remote_dir: row.get(4)?,
            remote_name: row.get(5)?,
            duration: row.get(6)?,
            status: row.get(7)?,
            upload_progress: row.get(8)?,
            upload_uploaded_bytes: row.get(9)?,
            upload_total_bytes: row.get(10)?,
            upload_cid: row.get(11)?,
            upload_file_name: row.get(12)?,
            upload_session_id: row.get(13)?,
            upload_biz_id: row.get(14)?,
            upload_endpoint: row.get(15)?,
            upload_auth: row.get(16)?,
            upload_uri: row.get(17)?,
            upload_chunk_size: row.get(18)?,
            upload_last_part_index: row.get(19)?,
            create_time: row.get(20)?,
            update_time: row.get(21)?,
          })
        })
        .ok();
      Ok(result)
    })
    .map_err(|err| err.to_string())
}

fn collect_merge_video_paths(base_dir: &Path) -> Vec<PathBuf> {
  let mut result = Vec::new();
  collect_merge_files_from_dir(&base_dir.join("merge"), &mut result);
  let updates_dir = base_dir.join("updates");
  if let Ok(entries) = fs::read_dir(&updates_dir) {
    for entry in entries.flatten() {
      let update_dir = entry.path();
      if update_dir.is_dir() {
        collect_merge_files_from_dir(&update_dir.join("merge"), &mut result);
      }
    }
  }
  result
}

fn collect_merge_files_from_dir(dir: &Path, output: &mut Vec<PathBuf>) {
  let entries = match fs::read_dir(dir) {
    Ok(entries) => entries,
    Err(_) => return,
  };
  for entry in entries.flatten() {
    let path = entry.path();
    if path
      .extension()
      .and_then(|ext| ext.to_str())
      .map(|ext| ext.eq_ignore_ascii_case("mp4"))
      .unwrap_or(false)
    {
      output.push(path);
    }
  }
}

fn insert_merged_video_record(
  context: &SubmissionContext,
  task_id: &str,
  merged_path: &Path,
  timestamp: &str,
) -> Result<i64, String> {
  let file_name = merged_path
    .file_name()
    .and_then(|name| name.to_str())
    .unwrap_or("merged.mp4");
  let total_bytes = fs::metadata(merged_path).map(|meta| meta.len()).unwrap_or(0);
  context
    .db
    .with_conn(|conn| {
      conn.execute(
        "INSERT INTO merged_video (task_id, file_name, video_path, duration, status, upload_progress, upload_uploaded_bytes, upload_total_bytes, upload_cid, upload_file_name, upload_session_id, upload_biz_id, upload_endpoint, upload_auth, upload_uri, upload_chunk_size, upload_last_part_index, create_time, update_time) \
         VALUES (?1, ?2, ?3, NULL, 2, 0, 0, ?4, NULL, NULL, NULL, 0, NULL, NULL, NULL, 0, 0, ?5, ?6)",
        (
          task_id,
          file_name,
          merged_path.to_string_lossy().to_string(),
          total_bytes as i64,
          timestamp,
          timestamp,
        ),
      )?;
      Ok(conn.last_insert_rowid())
    })
    .map_err(|err| err.to_string())
}

fn ensure_merged_video_records(
  context: &SubmissionContext,
  task_id: &str,
) -> Result<(), String> {
  let base_dir = resolve_submission_base_dir(context, task_id);
  let candidates = collect_merge_video_paths(&base_dir);
  if candidates.is_empty() {
    return Ok(());
  }
  let existing: HashSet<String> = context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT video_path FROM merged_video WHERE task_id = ?1 AND video_path IS NOT NULL",
      )?;
      let rows = stmt.query_map([task_id], |row| row.get::<_, String>(0))?;
      Ok(rows.collect::<Result<HashSet<_>, _>>()?)
    })
    .map_err(|err| err.to_string())?;
  let mut known = existing;
  for path in candidates {
    let path_str = path.to_string_lossy().to_string();
    if known.contains(&path_str) {
      continue;
    }
    if !path.exists() {
      continue;
    }
    let timestamp = fs::metadata(&path)
      .and_then(|meta| meta.modified())
      .map(|time| chrono::DateTime::<Utc>::from(time).to_rfc3339())
      .unwrap_or_else(|_| now_rfc3339());
    let _ = insert_merged_video_record(context, task_id, &path, &timestamp)?;
    known.insert(path_str);
  }
  Ok(())
}

fn load_merged_videos_by_task(
  context: &SubmissionContext,
  task_id: &str,
) -> Result<Vec<MergedVideoRecord>, String> {
  context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT id, task_id, file_name, video_path, remote_dir, remote_name, duration, status, \
                upload_progress, upload_uploaded_bytes, upload_total_bytes, upload_cid, upload_file_name, \
                upload_session_id, upload_biz_id, upload_endpoint, upload_auth, upload_uri, upload_chunk_size, \
                upload_last_part_index, create_time, update_time \
         FROM merged_video WHERE task_id = ?1 ORDER BY create_time ASC, id ASC",
      )?;
      let rows = stmt.query_map([task_id], |row| {
        Ok(MergedVideoRecord {
          id: row.get(0)?,
          task_id: row.get(1)?,
          file_name: row.get(2)?,
          video_path: row.get(3)?,
          remote_dir: row.get(4)?,
          remote_name: row.get(5)?,
          duration: row.get(6)?,
          status: row.get(7)?,
          upload_progress: row.get(8)?,
          upload_uploaded_bytes: row.get(9)?,
          upload_total_bytes: row.get(10)?,
          upload_cid: row.get(11)?,
          upload_file_name: row.get(12)?,
          upload_session_id: row.get(13)?,
          upload_biz_id: row.get(14)?,
          upload_endpoint: row.get(15)?,
          upload_auth: row.get(16)?,
          upload_uri: row.get(17)?,
          upload_chunk_size: row.get(18)?,
          upload_last_part_index: row.get(19)?,
          create_time: row.get(20)?,
          update_time: row.get(21)?,
        })
      })?;
      Ok(rows.collect::<Result<Vec<_>, _>>()?)
    })
    .map_err(|err| err.to_string())
}

fn load_output_segment_range_by_merged_id(
  context: &SubmissionContext,
  task_id: &str,
  merged_id: i64,
) -> Result<Option<(i64, i64, i64)>, String> {
  context
    .db
    .with_conn(|conn| {
      let (min_order, max_order, count): (Option<i64>, Option<i64>, i64) = conn.query_row(
        "SELECT MIN(part_order), MAX(part_order), COUNT(*) FROM task_output_segment WHERE task_id = ?1 AND merged_id = ?2",
        (task_id, merged_id),
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
      )?;
      if count <= 0 {
        return Ok(None);
      }
      let min_order = min_order.unwrap_or(1);
      let max_order = max_order.unwrap_or(min_order);
      Ok(Some((min_order, max_order, count)))
    })
    .map_err(|err| err.to_string())
}

fn ensure_segments_bound_to_merged(
  context: &SubmissionContext,
  task_id: &str,
  merged_id: i64,
) -> Result<(), String> {
  let (total_segments, bound_segments, null_segments, merged_count) = context
    .db
    .with_conn(|conn| {
      let total: i64 = conn.query_row(
        "SELECT COUNT(*) FROM task_output_segment WHERE task_id = ?1",
        [task_id],
        |row| row.get(0),
      )?;
      let bound: i64 = conn.query_row(
        "SELECT COUNT(*) FROM task_output_segment WHERE task_id = ?1 AND merged_id = ?2",
        (task_id, merged_id),
        |row| row.get(0),
      )?;
      let null_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM task_output_segment WHERE task_id = ?1 AND (merged_id IS NULL OR merged_id = 0)",
        [task_id],
        |row| row.get(0),
      )?;
      let merged_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM merged_video WHERE task_id = ?1",
        [task_id],
        |row| row.get(0),
      )?;
      Ok((total, bound, null_count, merged_count))
    })
    .map_err(|err| err.to_string())?;
  if total_segments == 0 || bound_segments > 0 {
    return Ok(());
  }
  if merged_count == 1 || null_segments == total_segments {
    context
      .db
      .with_conn(|conn| {
        conn.execute(
          "UPDATE task_output_segment SET merged_id = ?1 WHERE task_id = ?2 AND (merged_id IS NULL OR merged_id = 0)",
          (merged_id, task_id),
        )?;
        Ok(())
      })
      .map_err(|err| err.to_string())?;
  }
  Ok(())
}

fn replace_segments_for_merged(
  context: &SubmissionContext,
  task_id: &str,
  merged_id: i64,
  outputs: &[PathBuf],
  segment_prefix: Option<&str>,
) -> Result<(), String> {
  if outputs.is_empty() {
    return Err("重新分段输出为空".to_string());
  }
  let range = load_output_segment_range_by_merged_id(context, task_id, merged_id)?;
  let (start_order, old_count, end_order) = match range {
    Some((min_order, max_order, count)) => (min_order, count, max_order),
    None => {
      let (existing_count, _max_order) = load_output_segment_stats(context, task_id)?;
      if existing_count == 0 {
        return save_output_segments(context, task_id, outputs, Some(merged_id), segment_prefix);
      }
      return Err("未找到合并视频对应的分段范围".to_string());
    }
  };
  let name_start_index = if start_order > 0 { start_order as usize } else { 1 };
  let outputs = rename_segment_outputs_with_prefix(outputs, segment_prefix, name_start_index)?;
  let delta = outputs.len() as i64 - old_count;
  context
    .db
    .with_conn_mut(|conn| {
      let tx = conn.transaction()?;
      tx.execute(
        "DELETE FROM task_output_segment WHERE task_id = ?1 AND merged_id = ?2 AND part_order >= ?3 AND part_order <= ?4",
        (task_id, merged_id, start_order, end_order),
      )?;
      let mut rename_targets: Vec<(String, i64)> = Vec::new();
      if delta != 0 {
        let mut stmt = tx.prepare(
          "SELECT segment_id, part_name, part_order FROM task_output_segment WHERE task_id = ?1 AND part_order > ?2",
        )?;
        let rows = stmt.query_map((task_id, end_order), |row| {
          Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
          ))
        })?;
        for row in rows {
          let (segment_id, part_name, part_order) = row?;
          if is_default_part_name(&part_name, part_order, segment_prefix) {
            rename_targets.push((segment_id, part_order));
          }
        }
        tx.execute(
          "UPDATE task_output_segment SET part_order = part_order + ?1 WHERE task_id = ?2 AND part_order > ?3",
          (delta, task_id, end_order),
        )?;
      }
      for (index, segment) in outputs.iter().enumerate() {
        let segment_id = uuid::Uuid::new_v4().to_string();
        let file_name = segment.file_name().and_then(|name| name.to_str()).unwrap_or("segment.mp4");
        let total_bytes = fs::metadata(segment).map(|meta| meta.len()).unwrap_or(0);
        let part_order = start_order + index as i64;
        let part_index = if part_order > 0 { part_order as usize } else { 1 };
        let part_name = build_part_title(segment_prefix, part_index);
        tx.execute(
          "INSERT INTO task_output_segment (segment_id, task_id, merged_id, part_name, segment_file_path, part_order, upload_status, cid, file_name, upload_progress, upload_uploaded_bytes, upload_total_bytes, upload_session_id, upload_biz_id, upload_endpoint, upload_auth, upload_uri, upload_chunk_size, upload_last_part_index) \
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'PENDING', NULL, ?7, 0, 0, ?8, NULL, 0, NULL, NULL, NULL, 0, 0)",
          (
            segment_id,
            task_id,
            merged_id,
            part_name,
            segment.to_string_lossy().to_string(),
            part_order,
            file_name,
            total_bytes as i64,
          ),
        )?;
      }
      for (segment_id, part_order) in rename_targets {
        let new_order = part_order + delta;
        if new_order > 0 {
          let new_name = build_part_title(segment_prefix, new_order as usize);
          tx.execute(
            "UPDATE task_output_segment SET part_name = ?1 WHERE segment_id = ?2",
            (new_name, segment_id),
          )?;
        }
      }
      tx.commit()?;
      Ok(())
    })
    .map_err(|err| err.to_string())
}

fn build_merge_output_path(workflow_dir: &Path, task_id: &str) -> PathBuf {
  let stamp = sanitize_filename(&now_rfc3339());
  let task = sanitize_filename(task_id);
  workflow_dir
    .join("merge")
    .join(format!("{}_merged_{}.mp4", task, stamp))
}

fn resolve_target_merged_video(
  context: &SubmissionContext,
  task_id: &str,
  requested_id: Option<i64>,
  app_log_path: &PathBuf,
) -> Result<MergedVideoRecord, String> {
  if let Some(merged_id) = requested_id {
    if let Ok(Some(merged)) = load_merged_video_by_id(context, task_id, merged_id) {
      ensure_segments_bound_to_merged(context, task_id, merged.id)?;
      return Ok(merged);
    }
    append_log(
      app_log_path,
      &format!(
        "submission_reprocess_merge_fallback task_id={} merged_id={} reason=missing",
        task_id, merged_id
      ),
    );
  }
  let latest = load_latest_merged_video(context, task_id)?
    .ok_or_else(|| "未找到合并视频".to_string())?;
  ensure_segments_bound_to_merged(context, task_id, latest.id)?;
  Ok(latest)
}

fn update_submission_status(
  context: &SubmissionContext,
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

fn reset_failed_segments_to_pending(
  context: &SubmissionContext,
  task_id: &str,
) -> Result<(), String> {
  context
    .db
    .with_conn(|conn| {
      conn.execute(
        "UPDATE task_output_segment SET upload_status = 'PENDING' WHERE task_id = ?1 AND upload_status = 'FAILED'",
        [task_id],
      )?;
      Ok(())
    })
    .map_err(|err| err.to_string())
}

fn resolve_submission_base_dir(context: &SubmissionContext, task_id: &str) -> PathBuf {
  let configured = load_download_settings_from_db(&context.db)
    .map(|settings| settings.download_path)
    .ok()
    .unwrap_or_default();
  let base = if configured.trim().is_empty() {
    default_download_dir()
  } else {
    PathBuf::from(configured.trim())
  };
  base.join(task_id)
}

fn load_workflow_status(
  context: &SubmissionContext,
  task_id: &str,
) -> Result<Option<WorkflowStatusRecord>, String> {
  context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT status, current_step, progress FROM workflow_instances WHERE task_id = ?1 ORDER BY created_at DESC LIMIT 1",
      )?;
      let result = stmt
        .query_row([task_id], |row| {
          let progress: Option<f64> = row.get(2)?;
          Ok(WorkflowStatusRecord {
            status: row.get(0)?,
            current_step: row.get(1)?,
            progress: progress.unwrap_or(0.0),
          })
        })
        .ok();
      Ok(result)
    })
    .map_err(|err| err.to_string())
}

fn load_latest_workflow_type(
  context: &SubmissionContext,
  task_id: &str,
) -> Result<Option<String>, String> {
  context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT workflow_type FROM workflow_instances WHERE task_id = ?1 ORDER BY created_at DESC LIMIT 1",
      )?;
      let result = stmt.query_row([task_id], |row| row.get(0)).optional()?;
      Ok(result)
    })
    .map_err(|err| err.to_string())
}

fn load_latest_workflow_runtime(
  context: &SubmissionContext,
  task_id: &str,
) -> Result<Option<(String, String, Option<Value>)>, String> {
  context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT wi.instance_id, wi.workflow_type, wc.configuration_data \
         FROM workflow_instances wi \
         LEFT JOIN workflow_configurations wc ON wi.configuration_id = wc.config_id \
         WHERE wi.task_id = ?1 ORDER BY wi.created_at DESC LIMIT 1",
      )?;
      let row: Option<(String, String, Option<String>)> = stmt
        .query_row([task_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .optional()?;
      Ok(row)
    })
    .map_err(|err| err.to_string())
    .map(|row| {
      row.map(|(instance_id, workflow_type, config_raw)| {
        let config = config_raw.and_then(|raw| serde_json::from_str::<Value>(&raw).ok());
        (instance_id, workflow_type, config)
      })
    })
}

fn is_workflow_instance_latest(
  context: &SubmissionContext,
  task_id: &str,
  workflow_instance_id: &str,
) -> Result<bool, String> {
  context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT instance_id FROM workflow_instances WHERE task_id = ?1 ORDER BY created_at DESC LIMIT 1",
      )?;
      let latest: Option<String> = stmt.query_row([task_id], |row| row.get(0)).optional()?;
      Ok(latest)
    })
    .map_err(|err| err.to_string())
    .map(|latest| {
      latest
        .as_deref()
        .map(|instance_id| instance_id == workflow_instance_id)
        .unwrap_or(false)
    })
}

fn reset_workflow_instances(
  context: &SubmissionContext,
  task_id: &str,
) -> Result<(), String> {
  context
    .db
    .with_conn(|conn| {
      conn.execute(
        "DELETE FROM workflow_execution_logs WHERE instance_id IN (SELECT instance_id FROM workflow_instances WHERE task_id = ?1)",
        [task_id],
      )?;
      conn.execute(
        "DELETE FROM workflow_performance_metrics WHERE instance_id IN (SELECT instance_id FROM workflow_instances WHERE task_id = ?1)",
        [task_id],
      )?;
      conn.execute(
        "DELETE FROM workflow_steps WHERE instance_id IN (SELECT instance_id FROM workflow_instances WHERE task_id = ?1)",
        [task_id],
      )?;
      conn.execute("DELETE FROM workflow_instances WHERE task_id = ?1", [task_id])?;
      Ok(())
    })
    .map_err(|err| err.to_string())
}

fn set_workflow_instance_status(
  context: &SubmissionContext,
  task_id: &str,
  status: &str,
) -> Result<(), String> {
  let now = now_rfc3339();
  let updated = context
    .db
    .with_conn(|conn| {
      let updated = conn.execute(
        "UPDATE workflow_instances SET status = ?1, updated_at = ?2 WHERE task_id = ?3",
        (status, &now, task_id),
      )?;
      Ok(updated)
    })
    .map_err(|err| err.to_string())?;

  if updated == 0 {
    return Err("Workflow instance not found".to_string());
  }

  Ok(())
}

async fn wait_for_workflow_ready(
  context: &SubmissionContext,
  task_id: &str,
) -> Result<(), String> {
  loop {
    let status = load_workflow_status(context, task_id)?;
    if let Some(status) = status {
      if status.status == "CANCELLED" {
        update_submission_status(context, task_id, "CANCELLED")?;
        return Err("Workflow cancelled".to_string());
      }
      if status.status == "PAUSED" {
        sleep(Duration::from_secs(1)).await;
        continue;
      }
    }
    return Ok(());
  }
}

struct WorkflowSettings {
  enable_segmentation: bool,
  segment_duration_seconds: i64,
  segment_prefix: Option<String>,
}

fn load_workflow_settings(context: &SubmissionContext, task_id: &str) -> WorkflowSettings {
  let config_value = load_latest_workflow_config(context, task_id)
    .ok()
    .flatten();

  parse_workflow_settings(config_value)
}

fn parse_workflow_settings(config: Option<Value>) -> WorkflowSettings {
  if let Some(config) = config {
    let segmentation = config.get("segmentationConfig");
    let enable_segmentation = segmentation
      .and_then(|value| value.get("enabled"))
      .and_then(|value| value.as_bool())
      .unwrap_or_else(|| {
        config
          .get("enableSegmentation")
          .and_then(|value| value.as_bool())
          .unwrap_or(false)
      });

    let segment_duration_seconds = segmentation
      .and_then(|value| value.get("segmentDurationSeconds"))
      .and_then(|value| value.as_i64())
      .unwrap_or(133);
    let segment_prefix = config
      .get("segmentPrefix")
      .and_then(|value| value.as_str())
      .map(|value| value.trim().to_string())
      .filter(|value| !value.is_empty());

    return WorkflowSettings {
      enable_segmentation,
      segment_duration_seconds,
      segment_prefix,
    };
  }

  WorkflowSettings {
    enable_segmentation: false,
    segment_duration_seconds: 133,
    segment_prefix: None,
  }
}

fn build_resegment_workflow_config(
  config: Option<Value>,
  segment_duration_seconds: i64,
) -> Value {
  let mut config = match config {
    Some(Value::Object(map)) => Value::Object(map),
    Some(_) => Value::Object(Map::new()),
    None => Value::Object(Map::new()),
  };
  if !config.is_object() {
    config = Value::Object(Map::new());
  }
  if let Some(config_map) = config.as_object_mut() {
    config_map.insert("enableSegmentation".to_string(), Value::Bool(true));
    let segmentation = config_map
      .entry("segmentationConfig".to_string())
      .or_insert_with(|| Value::Object(Map::new()));
    if !segmentation.is_object() {
      *segmentation = Value::Object(Map::new());
    }
    if let Some(seg_map) = segmentation.as_object_mut() {
      seg_map.insert("enabled".to_string(), Value::Bool(true));
      seg_map.insert(
        "segmentDurationSeconds".to_string(),
        Value::Number(Number::from(segment_duration_seconds.max(1))),
      );
    }
  }
  config
}

fn build_query_params(params: &[(String, String)]) -> String {
  let mut serializer = form_urlencoded::Serializer::new(String::new());
  for (key, value) in params {
    serializer.append_pair(key, value);
  }
  serializer.finish()
}

fn truncate_log_value(value: &Value) -> String {
  let raw = value.to_string();
  const LIMIT: usize = 4000;
  if raw.len() <= LIMIT {
    return raw;
  }
  let mut truncated = raw.chars().take(LIMIT).collect::<String>();
  truncated.push_str("...<truncated>");
  truncated
}

fn update_workflow_status(
  context: &SubmissionContext,
  task_id: &str,
  status: &str,
  current_step: Option<&str>,
  progress: f64,
) -> Result<(), String> {
  let now = now_rfc3339();
  context
    .db
    .with_conn(|conn| {
      conn.execute(
        "UPDATE workflow_instances SET status = ?1, current_step = ?2, progress = ?3, updated_at = ?4 WHERE task_id = ?5",
        (status, current_step, progress, &now, task_id),
      )?;
      Ok(())
    })
    .map_err(|err| err.to_string())
}

fn load_task_ids_by_status(
  context: &SubmissionContext,
  status: &str,
) -> Result<Vec<String>, String> {
  context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT task_id FROM submission_task WHERE status = ?1 ORDER BY updated_at ASC",
      )?;
      let rows = stmt.query_map([status], |row| row.get(0))?;
      let list = rows.collect::<Result<Vec<String>, _>>()?;
      Ok(list)
    })
    .map_err(|err| err.to_string())
}

fn load_next_queued_task(context: &SubmissionContext) -> Result<Option<String>, String> {
  context
    .db
    .with_conn(|conn| {
      let result = conn
        .query_row(
          "SELECT task_id FROM submission_task WHERE status = 'WAITING_UPLOAD' ORDER BY priority DESC, updated_at ASC LIMIT 1",
          [],
          |row| row.get(0),
        )
        .ok();
      Ok(result)
    })
    .map_err(|err| err.to_string())
}

fn has_other_queued_tasks(
  context: &SubmissionContext,
  task_id: &str,
) -> Result<bool, String> {
  context
    .db
    .with_conn(|conn| {
      let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM submission_task WHERE status = 'WAITING_UPLOAD' AND task_id != ?1",
        [task_id],
        |row| row.get(0),
      )?;
      Ok(count > 0)
    })
    .map_err(|err| err.to_string())
}
