use std::collections::{HashMap, HashSet, VecDeque};
use std::error::Error;
use std::fs;
use std::io::{ErrorKind, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use futures_util::stream::{FuturesUnordered, StreamExt};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, CONTENT_TYPE, USER_AGENT};
use reqwest::{Client, StatusCode};
use rusqlite::params;
use rusqlite::params_from_iter;
use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};
use tauri::State;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::{watch, Mutex as AsyncMutex};
use tokio::time::{sleep, timeout};
use url::form_urlencoded;

use crate::api::ApiResponse;
use crate::baidu_sync;
use crate::bilibili::client::{is_bilibili_risk_control_error, BilibiliClient};
use crate::commands::settings::{
    load_download_settings_from_db, DEFAULT_SUBMISSION_REMOTE_REFRESH_MINUTES,
    DEFAULT_UPLOAD_CONCURRENCY,
};
use crate::config::default_download_dir;
use crate::db::{Db, DbError};
use crate::ffmpeg::{run_ffprobe_capture, run_ffprobe_json};
use crate::login_refresh;
use crate::login_store::{AuthInfo, LoginStore};
use crate::path_store::{
    load_local_path_prefix, to_absolute_local_path_opt_with_prefix,
    to_absolute_local_path_with_prefix, to_portable_relative_path_with_prefix,
    to_stored_local_path_with_prefix,
};
use crate::processing::{
    clip_sources, clip_sources_transcode, decide_clip_copy, merge_files, merge_files_transcode,
    parse_time_to_seconds, probe_duration_seconds, segment_file, segment_file_with_options,
    ClipSource, TranscodeProfile,
};
#[cfg(target_os = "windows")]
use crate::utils::apply_no_window;
use crate::utils::{append_log, now_rfc3339, sanitize_filename};
use crate::AppState;

// 按职责拆分的子模块；各子模块通过 `use super::*` 共享本模块的类型与导入，
// 这里再整体重导出，保持 `commands::submission::xxx` 的对外路径不变。
mod baidu;
mod edit;
mod import_export;
mod media;
mod queue;
mod remote;
mod repost;
mod store;
mod task;
mod upload;
mod util;
mod workflow;
pub use self::baidu::*;
pub use self::edit::*;
pub use self::import_export::*;
pub use self::media::*;
pub use self::queue::*;
pub use self::remote::*;
pub use self::repost::*;
use self::store::*;
pub use self::task::*;
pub use self::upload::*;
use self::util::*;
pub use self::workflow::*;

#[derive(Clone)]
struct SubmissionContext {
    db: Arc<Db>,
    app_log_path: Arc<PathBuf>,
    edit_upload_state: Arc<Mutex<EditUploadState>>,
    local_path_prefix: PathBuf,
}

impl SubmissionContext {
    fn new(state: &State<'_, AppState>) -> Self {
        Self {
            db: state.db.clone(),
            app_log_path: state.app_log_path.clone(),
            edit_upload_state: state.edit_upload_state.clone(),
            local_path_prefix: load_local_path_prefix(state.db.as_ref()),
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
    remote_refresh_pause_count: Arc<AtomicUsize>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionTaskInput {
    pub title: String,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    pub cover_data_url: Option<String>,
    pub partition_id: i64,
    pub collection_id: Option<i64>,
    pub collection_section_id: Option<i64>,
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
    pub immediate_submit: Option<bool>,
    pub source_type: Option<String>,
    pub bilibili_uid: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceVideoInput {
    pub source_file_path: String,
    pub source_type: Option<String>,
    pub remote_path: Option<String>,
    pub sort_order: i64,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub remote_bvid: Option<String>,
    pub remote_aid: Option<i64>,
    pub remote_cid: Option<i64>,
    pub remote_part_title: Option<String>,
    pub remote_video_url: Option<String>,
    pub upload_part_title: Option<String>,
    pub upload_part_title_mode: Option<String>,
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
pub struct SubmissionTaskIdsRequest {
    pub task_ids: Vec<String>,
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionTranscodeRepostRequest {
    pub task_id: String,
    pub integrate_current_bvid: bool,
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum MediaProcessMode {
    CopyOnly,
    ClipAndMergeTranscode,
}

const FALLBACK_REASON_REPOST_MISSING_MERGED: &str = "合并视频缺失（重新投稿已回退源视频重下）";
const FALLBACK_REASON_RESEGMENT_MISSING_MERGED: &str = "合并视频缺失（重新分段已回退源视频重下）";
const IMPORT_MODE_NON_SEGMENTED: &str = "NON_SEGMENTED";
const IMPORT_MODE_SEGMENTED: &str = "SEGMENTED";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionEditTaskInput {
    pub title: String,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    pub cover_data_url: Option<String>,
    pub partition_id: i64,
    pub collection_id: Option<i64>,
    pub collection_section_id: Option<i64>,
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
    pub upload_status: Option<String>,
    pub upload_progress: Option<f64>,
    pub upload_uploaded_bytes: Option<i64>,
    pub upload_total_bytes: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionEditSubmitRequest {
    pub task_id: String,
    pub task: SubmissionEditTaskInput,
    pub segments: Vec<SubmissionEditSegmentInput>,
    pub sync_remote_update: Option<bool>,
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
pub struct SubmissionEditUpdateSourcesRequest {
    pub task_id: String,
    pub source_videos: Vec<SourceVideoInput>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionUploadCoverRequest {
    pub image_data_url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionCoverLocalPreviewRequest {
    pub file_path: String,
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionBindMergedRemoteRequest {
    pub task_id: String,
    pub merged_id: i64,
    pub remote_path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionClearMergedRemoteRequest {
    pub task_id: String,
    pub merged_id: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionDeleteMergedRequest {
    pub task_id: String,
    pub merged_id: i64,
    pub delete_local_file: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionBatchBindSegmentsMergedRequest {
    pub task_id: String,
    pub segment_ids: Vec<String>,
    pub merged_id: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionBindMergedSourcesRequest {
    pub task_id: String,
    pub merged_id: i64,
    pub source_ids: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionReorderSourceVideosRequest {
    pub task_id: String,
    pub source_ids: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionReorderMergedVideosRequest {
    pub task_id: String,
    pub merged_ids: Vec<i64>,
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
    pub error_message: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionTaskRecord {
    pub task_id: String,
    pub status: String,
    pub priority: bool,
    pub priority_at: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    pub cover_local_path: Option<String>,
    pub partition_id: i64,
    pub tags: Option<String>,
    pub topic_id: Option<i64>,
    pub mission_id: Option<i64>,
    pub activity_title: Option<String>,
    pub video_type: String,
    pub collection_id: Option<i64>,
    pub collection_section_id: Option<i64>,
    pub bvid: Option<String>,
    pub aid: Option<i64>,
    pub remote_state: Option<i64>,
    pub remote_state_desc: Option<String>,
    pub reject_reason: Option<String>,
    pub remote_status_ignored: bool,
    pub created_at: String,
    pub updated_at: String,
    pub segment_prefix: Option<String>,
    pub import_mode: String,
    pub source_type: String,
    pub enable_segmentation: bool,
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
    pub source_type: String,
    pub remote_path: Option<String>,
    pub remote_video_url: Option<String>,
    pub sort_order: i64,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub remote_bvid: Option<String>,
    pub remote_aid: Option<i64>,
    pub remote_cid: Option<i64>,
    pub remote_part_title: Option<String>,
    pub upload_part_title: Option<String>,
    pub upload_part_title_mode: Option<String>,
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionDeleteMergedResult {
    pub deleted: bool,
    pub deleted_local_file: bool,
    pub archived_local_path: Option<String>,
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
    pub source_video_ids: Vec<String>,
    pub source_video_paths: Vec<String>,
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

const SUBMISSION_EXPORT_MAGIC: &str = "reaction-cut-submission-export";
const SUBMISSION_EXPORT_VERSION: u32 = 1;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionExportRequest {
    pub task_ids: Option<Vec<String>>,
    pub export_all: Option<bool>,
    pub save_path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionExportResult {
    pub file_path: String,
    pub task_count: usize,
    pub exported_at: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionImportRequest {
    pub file_path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionImportResult {
    pub total_tasks: usize,
    pub imported_tasks: usize,
    pub skipped_tasks: usize,
    pub failed_tasks: usize,
    pub skipped_reasons: Vec<String>,
    pub failed_reasons: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionRemoteVideoPreviewRequest {
    pub input: String,
    pub enforce_owner_match: Option<bool>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionRemoteVideoPage {
    pub source_key: String,
    pub bvid: String,
    pub aid: i64,
    pub cid: i64,
    pub page: i64,
    pub part_name: String,
    pub duration_seconds: i64,
    pub start_time: String,
    pub end_time: String,
    pub file_name: String,
    pub remote_file_name: Option<String>,
    pub source_file_path: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionRemoteVideoPreview {
    pub input: String,
    pub bvid: String,
    pub aid: i64,
    pub title: String,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    pub partition_id: Option<i64>,
    pub partition_name: Option<String>,
    pub tags: Option<String>,
    pub collection_id: Option<i64>,
    pub collection_name: Option<String>,
    pub enable_segmentation: bool,
    pub segment_duration_seconds: i64,
    pub owner_mid: Option<i64>,
    pub owner_name: Option<String>,
    pub pages: Vec<SubmissionRemoteVideoPage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionRemoteImportBasicInput {
    pub origin_bvid: String,
    pub origin_aid: Option<i64>,
    pub title: String,
    pub description: Option<String>,
    pub cover_url: Option<String>,
    pub partition_id: i64,
    pub tags: Option<String>,
    pub video_type: Option<String>,
    pub collection_id: Option<i64>,
    pub collection_section_id: Option<i64>,
    pub segment_prefix: Option<String>,
    pub import_mode: Option<String>,
    pub enable_segmentation: Option<bool>,
    pub segment_duration_seconds: Option<i64>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionRemoteImportSourceInput {
    pub source_key: String,
    pub bvid: String,
    pub cid: i64,
    pub part_name: String,
    pub duration_seconds: Option<i64>,
    pub sort_order: i64,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub source_file_path: Option<String>,
    pub remote_file_name: Option<String>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionRemoteImportMergedInput {
    pub file_name: Option<String>,
    pub remote_path: Option<String>,
    pub source_keys: Vec<String>,
    pub sort_order: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionRemoteImportSaveRequest {
    pub basic: SubmissionRemoteImportBasicInput,
    pub sources: Vec<SubmissionRemoteImportSourceInput>,
    pub merged_videos: Vec<SubmissionRemoteImportMergedInput>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionRemoteImportSaveResult {
    pub task_id: String,
    pub source_count: usize,
    pub merged_count: usize,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubmissionExportTaskBundle {
    task_id: String,
    submission_task: Value,
    task_source_videos: Vec<Value>,
    video_clips: Vec<Value>,
    merged_videos: Vec<Value>,
    merged_source_videos: Vec<Value>,
    task_output_segments: Vec<Value>,
    workflow_instances: Vec<Value>,
    workflow_steps: Vec<Value>,
    workflow_execution_logs: Vec<Value>,
    workflow_performance_metrics: Vec<Value>,
    task_relations: Vec<Value>,
    video_downloads: Vec<Value>,
    baidu_sync_tasks: Vec<Value>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubmissionExportBundle {
    magic: String,
    version: u32,
    exported_at: String,
    task_count: usize,
    tasks: Vec<SubmissionExportTaskBundle>,
}

#[derive(Clone)]
struct MergeGroupPlan {
    order: i64,
    sources: Vec<ClipSource>,
    /// 「上传分P标题」自定义值（仅 CUSTOM 且非空时为 Some）。
    /// 分段关闭时作为该最终上传单元的 B 站分P标题；None 时兜底 P{index}。
    upload_part_title: Option<String>,
}

enum BaiduRestoreResult {
    NotBound,
    Ready(PathBuf),
    Queued,
}

const BAIDU_RESTORE_CHECK_TIMEOUT_SECS: u64 = 8;

const SOURCE_READY_STABLE_DELAY_SECS: u64 = 2;
const SOURCE_READY_MAX_RETRIES: u32 = 30;
const SOURCE_READY_MAX_WAIT_SECS: u64 = 30;

struct SourceReadyInfo {
    source: ClipSource,
    path: String,
    size: u64,
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
        let bytes_delta = snapshot
            .uploaded_bytes
            .saturating_sub(self.last_saved_bytes);
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
    source_type: String,
}

#[derive(Clone)]
struct SourceRetryBinding {
    source_file_path: String,
    source_type: String,
    remote_path: Option<String>,
    remote_video_url: Option<String>,
    remote_bvid: Option<String>,
    remote_aid: Option<String>,
    remote_cid: Option<i64>,
    remote_part_title: Option<String>,
}

const SUBMISSION_BATCH_PARTS: usize = 50;
const MAX_PARTS_PER_BVID: usize = 200;
const SUBMISSION_STATUS_VERIFY_PENDING: &str = "VERIFY_PENDING";
const SUBMISSION_STATUS_SUBMIT_UNKNOWN: &str = "SUBMIT_UNKNOWN";
const SUBMISSION_STATUS_REUPLOAD_PENDING: &str = "REUPLOAD_PENDING";
const SUBMISSION_REMOTE_MISSING_CONFIRMED_REASON: &str = "已人工确认远端未生成稿件";
const RESET_ALL_SEGMENT_UPLOAD_SQL: &str = "UPDATE task_output_segment SET upload_status = 'PENDING', cid = NULL, file_name = NULL, upload_progress = 0, upload_uploaded_bytes = 0, upload_total_bytes = 0, upload_session_id = NULL, upload_biz_id = 0, upload_endpoint = NULL, upload_auth = NULL, upload_uri = NULL, upload_chunk_size = 0, upload_last_part_index = 0 WHERE task_id = ?1";
const SUBMISSION_COLLECTION_BIND_PENDING_PREFIX: &str = "COLLECTION_BIND_PENDING:";
const SUBMISSION_EDIT_BATCH_WAIT_SECS: u64 = 10;
const SUBMISSION_EDIT_RATE_LIMIT_BASE_WAIT_SECS: u64 = 10;
const SUBMISSION_EDIT_RATE_LIMIT_MAX_WAIT_SECS: u64 = 120;
const SUBMISSION_EDIT_RATE_LIMIT_RETRY_LIMIT: u32 = 3;
const RATE_LIMIT_BASE_WAIT_SECS: u64 = 60;
const RATE_LIMIT_MAX_WAIT_SECS: u64 = 30 * 60;
const UPLOAD_SEGMENT_RETRY_LIMIT: u32 = 5;
const SUBMISSION_QUEUE_RETRY_LIMIT: u32 = 3;
const SUBMISSION_UPLOAD_STALE_TIMEOUT_SECS: i64 = 45 * 60;
const SUBMISSION_UPLOAD_STALE_CHECK_INTERVAL_SECS: u64 = 30;
const SUBMISSION_QUEUE_RETRY_BASE_DELAY_SECS: u64 = 10;
const SUBMISSION_QUEUE_RETRY_MAX_DELAY_SECS: u64 = 120;
const COLLECTION_BIND_RETRY_LIMIT: u32 = 3;
const COLLECTION_BIND_RETRY_BASE_DELAY_SECS: u64 = 5;
const COLLECTION_BIND_RETRY_MAX_DELAY_SECS: u64 = 30;
const REMOTE_AUDIT_STATUS_REVIEWING: &str = "is_pubing";
const REMOTE_AUDIT_STATUS_REJECTED: &str = "not_pubed";
const REMOTE_AUDIT_STATUS_PUBLISHED: &str = "pubed";
const REMOTE_AUDIT_STATUS_SCOPE: &str = "is_pubing|not_pubed|pubed(reconcile_only)";
const REMOTE_STATE_TRANSCODE_FAILED: i64 = -16;
const REMOTE_DEBUG_BVID: &str = "BV1VJkFBZENQ";
const REMOTE_PART_VERIFY_RETRY_LIMIT: u32 = 5;
const REMOTE_PART_VERIFY_RETRY_WAIT_SECS: u64 = 2;
const SUBMISSION_MANUAL_VERIFY_TIMEOUT_SECS: u64 = 30;
const REMOTE_PUBLISHED_PAGE_WAIT_SECS: u64 = 2;
const REMOTE_UNKNOWN_MATCH_WINDOW_SECS: i64 = 6 * 60 * 60;
const UPLOAD_RETRY_BASE_DELAY_SECS: u64 = 5;
const UPLOAD_RETRY_MAX_DELAY_SECS: u64 = 60;
const UPLOAD_CONNECT_TIMEOUT_SECS: u64 = 20;
const UPLOAD_REQUEST_TIMEOUT_SECS: u64 = 180;
const UPLOAD_TRANSPORT_SESSION_REBUILD_ATTEMPT: u32 = 2;
const UPLOAD_LOCAL_ADDRESS_RECOVERY_WAIT_SECS: u64 = 120;
const UPLOAD_MAX_CHUNK_SIZE_BYTES: u64 = 5 * 1024 * 1024;
const UPLOAD_SESSION_RESET_REQUIRED_PREFIX: &str = "UPLOAD_SESSION_RESET_REQUIRED:";
const SUBMISSION_CREATED_PREFIX: &str = "SUBMISSION_CREATED:";
const PREUPLOAD_PARSE_RETRY_BASE_SECS: u64 = 60;
const PREUPLOAD_PARSE_RETRY_MAX_SECS: u64 = 30 * 60;
const PREUPLOAD_PARSE_RETRY_LIMIT: u32 = 6;
const PREUPLOAD_MIN_INTERVAL_MS: u64 = 1000;

static PREUPLOAD_THROTTLE: OnceLock<AsyncMutex<HashMap<i64, Instant>>> = OnceLock::new();
static TASK_UPLOAD_RATE_LIMITS: OnceLock<AsyncMutex<HashMap<String, TaskUploadRateLimitState>>> =
    OnceLock::new();

struct UploadRateLimiter {
    consecutive_406: u32,
}

impl UploadRateLimiter {
    fn new() -> Self {
        Self { consecutive_406: 0 }
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

impl Default for UploadRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

struct TaskUploadRateLimitState {
    limiter: UploadRateLimiter,
    blocked_until: Option<Instant>,
    wake_tx: watch::Sender<u64>,
}

impl TaskUploadRateLimitState {
    fn new() -> Self {
        let (wake_tx, _wake_rx) = watch::channel(0);
        Self {
            limiter: UploadRateLimiter::default(),
            blocked_until: None,
            wake_tx,
        }
    }
}

impl Default for TaskUploadRateLimitState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RemoteAuditInfo {
    aid: i64,
    bvid: String,
    title: String,
    part_count: usize,
    ctime: i64,
    state: i64,
    state_desc: Option<String>,
    reject_reason: Option<String>,
}

#[derive(Clone, Debug)]
struct RemoteTrackedTask {
    task_id: String,
    bvid: String,
    aid: i64,
    title: String,
    status: String,
    remote_state: Option<i64>,
    remote_status_ignored: bool,
    created_at: String,
    reconciliation_at: String,
    error_message: String,
    expected_parts: usize,
    collection_section_id: i64,
}

impl RemoteTrackedTask {
    fn needs_remote_refresh(&self) -> bool {
        !self.remote_status_ignored
            && (self.needs_reconciliation()
                || (self.status == "COMPLETED"
                    && !self.bvid.trim().is_empty()
                    && matches!(
                        self.remote_state,
                        None | Some(-1) | Some(-30) | Some(-60) | Some(-2) | Some(-4)
                    )))
    }

    fn needs_reconciliation(&self) -> bool {
        if is_submission_reconciliation_status(&self.status) {
            return true;
        }
        if self.status != "FAILED" {
            return false;
        }
        if is_remote_transcode_failed_state(self.remote_state) {
            return false;
        }
        is_submit_result_unknown_error(&self.error_message)
            || is_submission_created_error(&self.error_message)
            || (!self.bvid.trim().is_empty()
                && (is_rate_limit_error(&self.error_message)
                    || is_remote_part_count_mismatch_error(&self.error_message)))
    }

    fn needs_published_lookup(&self) -> bool {
        self.needs_reconciliation()
            && (self.bvid.trim().is_empty()
                || is_collection_bind_pending_error(&self.error_message)
                || is_submission_created_error(&self.error_message))
    }

    fn published_lookup_since(&self) -> Option<i64> {
        if !self.needs_published_lookup() {
            return None;
        }
        DateTime::parse_from_rfc3339(self.created_at.trim())
            .ok()
            .map(|value| {
                value
                    .timestamp()
                    .saturating_sub(REMOTE_UNKNOWN_MATCH_WINDOW_SECS)
            })
    }
}

#[derive(Clone, Debug)]
struct RemoteReconciliationUpdate {
    task_id: String,
    bvid: String,
    aid: i64,
    remote_state: i64,
    remote_state_desc: Option<String>,
    reject_reason: Option<String>,
}

#[derive(Clone, Debug)]
struct RemoteReconciliationFailure {
    task_id: String,
    bvid: String,
    aid: i64,
    remote_state: i64,
    remote_state_desc: Option<String>,
    reject_reason: Option<String>,
    reason: String,
}

struct IntegratedDownloadStats {
    total: i64,
    completed: i64,
    failed: i64,
}

struct WorkflowSettings {
    enable_segmentation: bool,
    segment_duration_seconds: i64,
    segment_prefix: Option<String>,
}

#[derive(Clone)]
struct QueuedSubmissionTask {
    task_id: String,
    bilibili_uid: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use serde_json::json;

    #[test]
    fn load_tasks_applies_filter_combinations() {
        let db = Arc::new(Db::new(PathBuf::from(":memory:")).expect("init db"));
        db.with_conn(|conn| {
            for (id, uid, status, source, title) in [
                ("a", 1, "PENDING", Some("NORMAL"), "Alpha"),
                ("b", 1, "COMPLETED", None, "beta"),
                ("c", 1, "PENDING", Some("LIVE"), "alpha live"),
                ("d", 2, "PENDING", Some("NORMAL"), "alpha"),
            ] {
                conn.execute(
                    "INSERT INTO submission_task (task_id, bilibili_uid, status, source_type, title, partition_id, video_type, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, 21, 'ORIGINAL', '2026-01-01', '2026-01-01')",
                    (id, uid, status, source, title),
                )?;
            }
            Ok(())
        })
        .expect("seed tasks");
        let context = SubmissionContext {
            db,
            app_log_path: Arc::new(PathBuf::new()),
            edit_upload_state: Arc::new(Mutex::new(EditUploadState::default())),
            local_path_prefix: PathBuf::new(),
        };
        let total = |status: Option<&str>, query: Option<&str>, source: Option<&str>| {
            load_tasks(
                &context,
                status.map(str::to_string),
                1,
                20,
                query.map(str::to_string),
                1,
                source.map(str::to_string),
            )
            .expect("load tasks")
            .total
        };
        assert_eq!(total(None, None, None), 3);
        assert_eq!(total(Some("PENDING"), None, None), 2);
        assert_eq!(total(None, None, Some("normal")), 2);
        assert_eq!(total(None, Some(" ALPHA "), None), 2);
        assert_eq!(total(Some("PENDING"), Some("alpha"), Some("NORMAL")), 1);
        let page = load_tasks(&context, None, 2, 2, None, 1, None).expect("page 2");
        assert_eq!((page.total, page.items.len()), (3, 1));
    }

    fn build_test_task(import_mode: &str, enable_segmentation: bool) -> SubmissionTaskRecord {
        SubmissionTaskRecord {
            task_id: "task-1".to_string(),
            status: "PENDING".to_string(),
            priority: false,
            priority_at: None,
            title: "test".to_string(),
            description: None,
            cover_url: None,
            cover_local_path: None,
            partition_id: 21,
            tags: Some("tag".to_string()),
            topic_id: None,
            mission_id: None,
            activity_title: None,
            video_type: "ORIGINAL".to_string(),
            collection_id: None,
            collection_section_id: None,
            bvid: Some("BV1test".to_string()),
            aid: Some(1),
            remote_state: None,
            remote_state_desc: None,
            reject_reason: None,
            remote_status_ignored: false,
            created_at: "2026-03-10 09:18:11".to_string(),
            updated_at: "2026-03-10 09:18:11".to_string(),
            segment_prefix: None,
            import_mode: import_mode.to_string(),
            source_type: "NORMAL".to_string(),
            enable_segmentation,
            baidu_sync_enabled: false,
            baidu_sync_path: None,
            baidu_sync_filename: None,
            has_integrated_downloads: false,
            workflow_status: None,
        }
    }

    fn build_test_detail(merged_count: usize, segment_count: usize) -> SubmissionTaskDetail {
        let merged_videos = (0..merged_count)
            .map(|index| MergedVideoRecord {
                id: index as i64 + 1,
                task_id: "task-1".to_string(),
                file_name: Some(format!("merged-{}.mp4", index + 1)),
                video_path: Some(format!("merged/{}.mp4", index + 1)),
                source_video_ids: Vec::new(),
                source_video_paths: Vec::new(),
                remote_dir: None,
                remote_name: None,
                duration: None,
                status: 0,
                upload_progress: 0.0,
                upload_uploaded_bytes: 0,
                upload_total_bytes: 0,
                upload_cid: Some(index as i64 + 100),
                upload_file_name: Some(format!("uploaded-{}", index + 1)),
                upload_session_id: None,
                upload_biz_id: 0,
                upload_endpoint: None,
                upload_auth: None,
                upload_uri: None,
                upload_chunk_size: 0,
                upload_last_part_index: 0,
                create_time: "2026-03-10 09:18:11".to_string(),
                update_time: "2026-03-10 09:18:11".to_string(),
            })
            .collect();
        let output_segments = (0..segment_count)
            .map(|index| TaskOutputSegmentRecord {
                segment_id: format!("segment-{}", index + 1),
                task_id: "task-1".to_string(),
                merged_id: Some(index as i64 + 1),
                part_name: format!("P{}", index + 1),
                segment_file_path: format!("segments/{}.mp4", index + 1),
                part_order: index as i64 + 1,
                upload_status: "SUCCESS".to_string(),
                cid: Some(index as i64 + 1000),
                file_name: Some(format!("file-{}", index + 1)),
                upload_progress: 100.0,
                upload_uploaded_bytes: 1,
                upload_total_bytes: 1,
                upload_session_id: None,
                upload_biz_id: 0,
                upload_endpoint: None,
                upload_auth: None,
                upload_uri: None,
                upload_chunk_size: 0,
                upload_last_part_index: 0,
            })
            .collect();
        SubmissionTaskDetail {
            task: build_test_task(IMPORT_MODE_NON_SEGMENTED, false),
            source_videos: Vec::new(),
            output_segments,
            merged_videos,
            workflow_config: None,
        }
    }

    fn setup_import_test_db() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory sqlite");
        conn.execute_batch(
            "
        CREATE TABLE submission_task (
          task_id TEXT PRIMARY KEY,
          status TEXT,
          title TEXT,
          cover_local_path TEXT
        );
        CREATE TABLE task_source_video (
          id TEXT PRIMARY KEY,
          task_id TEXT,
          source_file_path TEXT
        );
        CREATE TABLE merged_video (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          task_id TEXT,
          video_path TEXT
        );
        CREATE TABLE task_output_segment (
          segment_id TEXT PRIMARY KEY,
          task_id TEXT,
          merged_id INTEGER,
          segment_file_path TEXT
        );
        ",
        )
        .expect("create test schema");
        conn
    }

    #[test]
    fn normalize_json_path_field_converts_windows_absolute_to_relative() {
        let mut row = json!({
          "source_file_path": r"D:\work\cut\first\mqnl\recording.mp4"
        });
        normalize_json_path_field_with_prefix(
            &mut row,
            "source_file_path",
            Path::new("/Users/test/Downloads"),
        );
        assert_eq!(
            json_string_field(&row, "source_file_path").unwrap_or_default(),
            "work/cut/first/mqnl/recording.mp4"
        );
    }

    #[test]
    fn normalize_export_bundle_paths_includes_segments_and_sources() {
        let mut bundle = SubmissionExportTaskBundle {
            task_id: "task-1".to_string(),
            submission_task: json!({
              "task_id": "task-1",
              "cover_local_path": r"D:\cover\cover.jpg"
            }),
            task_source_videos: vec![json!({
              "id": "source-1",
              "task_id": "task-1",
              "source_file_path": r"D:\sources\source-1.mp4"
            })],
            video_clips: vec![json!({
              "id": 1,
              "task_id": "task-1",
              "clip_path": r"D:\clips\clip-1.mp4"
            })],
            merged_videos: vec![json!({
              "id": 1,
              "task_id": "task-1",
              "video_path": r"D:\merged\merged-1.mp4"
            })],
            merged_source_videos: vec![json!({
              "id": 1,
              "task_id": "task-1",
              "source_file_path": r"D:\merged-sources\source-1.mp4"
            })],
            task_output_segments: vec![json!({
              "segment_id": "seg-1",
              "task_id": "task-1",
              "segment_file_path": r"D:\segments\part-001.mp4"
            })],
            workflow_instances: Vec::new(),
            workflow_steps: Vec::new(),
            workflow_execution_logs: Vec::new(),
            workflow_performance_metrics: Vec::new(),
            task_relations: Vec::new(),
            video_downloads: vec![json!({
              "id": 1,
              "local_path": r"D:\downloads\video.mp4"
            })],
            baidu_sync_tasks: vec![json!({
              "id": 1,
              "local_path": r"D:\sync\merged.mp4"
            })],
        };

        normalize_submission_export_task_bundle_paths(
            &mut bundle,
            Path::new("/Users/test/Downloads"),
        );

        assert_eq!(
            json_string_field(&bundle.submission_task, "cover_local_path").unwrap_or_default(),
            "cover/cover.jpg"
        );
        assert_eq!(
            json_string_field(&bundle.task_source_videos[0], "source_file_path")
                .unwrap_or_default(),
            "sources/source-1.mp4"
        );
        assert_eq!(
            json_string_field(&bundle.video_clips[0], "clip_path").unwrap_or_default(),
            "clips/clip-1.mp4"
        );
        assert_eq!(
            json_string_field(&bundle.merged_videos[0], "video_path").unwrap_or_default(),
            "merged/merged-1.mp4"
        );
        assert_eq!(
            json_string_field(&bundle.merged_source_videos[0], "source_file_path")
                .unwrap_or_default(),
            "merged-sources/source-1.mp4"
        );
        assert_eq!(
            json_string_field(&bundle.task_output_segments[0], "segment_file_path")
                .unwrap_or_default(),
            "segments/part-001.mp4"
        );
        assert_eq!(
            json_string_field(&bundle.video_downloads[0], "local_path").unwrap_or_default(),
            "downloads/video.mp4"
        );
        assert_eq!(
            json_string_field(&bundle.baidu_sync_tasks[0], "local_path").unwrap_or_default(),
            "sync/merged.mp4"
        );
    }

    #[test]
    fn import_bundle_stores_relative_paths_and_segments() {
        let mut conn = setup_import_test_db();
        let bundle = SubmissionExportTaskBundle {
            task_id: "task-1".to_string(),
            submission_task: json!({
              "task_id": "task-1",
              "status": "COMPLETED",
              "title": "test",
              "cover_local_path": r"D:\cover\cover.jpg"
            }),
            task_source_videos: vec![json!({
              "id": "source-old-1",
              "task_id": "task-1",
              "source_file_path": r"D:\source\input-1.mp4"
            })],
            video_clips: Vec::new(),
            merged_videos: vec![json!({
              "id": 101,
              "task_id": "task-1",
              "video_path": r"D:\merged\merged-1.mp4"
            })],
            merged_source_videos: Vec::new(),
            task_output_segments: vec![json!({
              "segment_id": "segment-old-1",
              "task_id": "task-1",
              "merged_id": 101,
              "segment_file_path": r"D:\segments\part-001.mp4"
            })],
            workflow_instances: Vec::new(),
            workflow_steps: Vec::new(),
            workflow_execution_logs: Vec::new(),
            workflow_performance_metrics: Vec::new(),
            task_relations: Vec::new(),
            video_downloads: Vec::new(),
            baidu_sync_tasks: Vec::new(),
        };

        let tx = conn.transaction().expect("begin tx");
        import_submission_task_bundle(&tx, &bundle, Path::new("/Users/test/Downloads"))
            .expect("import bundle");
        tx.commit().expect("commit tx");

        let stored_cover: String = conn
            .query_row(
                "SELECT cover_local_path FROM submission_task WHERE task_id = 'task-1'",
                [],
                |row| row.get(0),
            )
            .expect("load cover path");
        assert_eq!(stored_cover, "cover/cover.jpg");

        let stored_source: String = conn
            .query_row(
                "SELECT source_file_path FROM task_source_video WHERE task_id = 'task-1' LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("load source path");
        assert_eq!(stored_source, "source/input-1.mp4");

        let merged_row_id: i64 = conn
            .query_row(
                "SELECT id FROM merged_video WHERE task_id = 'task-1' LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("load merged id");
        let (stored_segment_path, segment_merged_id): (String, Option<i64>) = conn
      .query_row(
        "SELECT segment_file_path, merged_id FROM task_output_segment WHERE task_id = 'task-1' LIMIT 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
      )
      .expect("load segment");
        assert_eq!(stored_segment_path, "segments/part-001.mp4");
        assert_eq!(segment_merged_id, Some(merged_row_id));
    }

    #[test]
    fn validate_source_video_inputs_accepts_zero_start_time() {
        let sources = vec![SourceVideoInput {
            source_file_path: "demo/source.mp4".to_string(),
            source_type: None,
            remote_path: None,
            sort_order: 1,
            start_time: Some("00:00:00".to_string()),
            end_time: Some("00:00:05".to_string()),
            remote_bvid: None,
            remote_aid: None,
            remote_cid: None,
            remote_part_title: None,
            remote_video_url: None,
            upload_part_title: None,
            upload_part_title_mode: None,
        }];
        let result = validate_source_video_inputs(&sources);
        assert!(result.is_ok());
    }

    #[test]
    fn validate_source_video_inputs_rejects_zero_end_time() {
        let sources = vec![SourceVideoInput {
            source_file_path: "demo/source.mp4".to_string(),
            source_type: None,
            remote_path: None,
            sort_order: 1,
            start_time: Some("00:00:00".to_string()),
            end_time: Some("00:00:00".to_string()),
            remote_bvid: None,
            remote_aid: None,
            remote_cid: None,
            remote_part_title: None,
            remote_video_url: None,
            upload_part_title: None,
            upload_part_title_mode: None,
        }];
        let result = validate_source_video_inputs(&sources);
        assert_eq!(result.unwrap_err(), "第1行时间范围不合法");
    }

    #[test]
    fn validate_source_video_inputs_accepts_baidu_without_end_time() {
        let sources = vec![SourceVideoInput {
            source_file_path: String::new(),
            source_type: Some("BAIDU".to_string()),
            remote_path: Some("/视频/示例.mp4".to_string()),
            sort_order: 1,
            start_time: Some("00:00:00".to_string()),
            end_time: None,
            remote_bvid: None,
            remote_aid: None,
            remote_cid: None,
            remote_part_title: None,
            remote_video_url: None,
            upload_part_title: None,
            upload_part_title_mode: None,
        }];
        assert!(validate_source_video_inputs(&sources).is_ok());
    }

    #[test]
    fn validate_source_video_inputs_rejects_baidu_without_remote_path() {
        let sources = vec![SourceVideoInput {
            source_file_path: String::new(),
            source_type: Some("BAIDU".to_string()),
            remote_path: None,
            sort_order: 1,
            start_time: Some("00:00:00".to_string()),
            end_time: None,
            remote_bvid: None,
            remote_aid: None,
            remote_cid: None,
            remote_part_title: None,
            remote_video_url: None,
            upload_part_title: None,
            upload_part_title_mode: None,
        }];
        assert_eq!(
            validate_source_video_inputs(&sources).unwrap_err(),
            "第1行百度网盘源视频缺少远程路径"
        );
    }

    #[test]
    fn format_timecode_seconds_handles_rounding_carry() {
        assert_eq!(format_timecode_seconds(7199.999667), "02:00:00");
        assert_eq!(format_timecode_seconds(59.9996), "00:01:00");
    }

    #[test]
    fn detect_merged_video_decode_issue_matches_h264_corruption_patterns() {
        let stderr = "\
[h264 @ 0x1] missing picture in access unit with size 451
[h264 @ 0x1] non-existing PPS 7 referenced
[h264 @ 0x1] illegal POC type 20
";
        assert_eq!(
            detect_merged_video_decode_issue(stderr),
            Some("missing_picture_in_access_unit")
        );
    }

    #[test]
    fn detect_merged_video_decode_issue_matches_hevc_nal_corruption() {
        let stderr = "\
[hevc @ 0x1] Invalid NAL unit size (1677734572 > 125991).
[hevc @ 0x1] Error splitting the input into NAL units.
";
        assert_eq!(
            detect_merged_video_decode_issue(stderr),
            Some("invalid_nal_unit_size")
        );
    }

    #[test]
    fn non_segmented_initial_upload_uses_output_segments_when_p1_exists() {
        let detail = build_test_detail(1, 1);
        assert!(should_use_output_segments_upload(&detail, false));
    }

    #[test]
    fn validate_non_segmented_submission_consistency_rejects_count_mismatch() {
        let detail = build_test_detail(3, 2);
        let parts = vec![
            UploadedVideoPart {
                filename: "file-1".to_string(),
                cid: 1,
                title: "P1".to_string(),
            },
            UploadedVideoPart {
                filename: "file-2".to_string(),
                cid: 2,
                title: "P2".to_string(),
            },
        ];
        let err = validate_non_segmented_submission_consistency(&detail, &parts)
            .expect_err("should reject mismatch");
        assert!(err.contains("合并视频数与分P记录数不一致"));
    }

    #[test]
    fn validate_non_segmented_submission_consistency_accepts_aligned_counts() {
        let detail = build_test_detail(3, 3);
        let parts = vec![
            UploadedVideoPart {
                filename: "file-1".to_string(),
                cid: 1,
                title: "P1".to_string(),
            },
            UploadedVideoPart {
                filename: "file-2".to_string(),
                cid: 2,
                title: "P2".to_string(),
            },
            UploadedVideoPart {
                filename: "file-3".to_string(),
                cid: 3,
                title: "P3".to_string(),
            },
        ];
        validate_non_segmented_submission_consistency(&detail, &parts)
            .expect("aligned counts should pass");
    }

    #[test]
    fn rate_limit_detection_includes_submission_code_minus_702() {
        assert!(is_rate_limit_error("请求频率过高，请稍后再试 (code: -702)"));
        assert!(is_rate_limit_error("request was banned (code: -412)"));
        assert!(is_rate_limit_error("请求过于频繁"));
        assert!(!is_rate_limit_error("网络繁忙 (code: 69800)"));
    }

    #[test]
    fn submission_batch_indexes_limit_initial_add_to_fifty_parts() {
        assert_eq!(submission_batch_end_indexes(0), Vec::<usize>::new());
        assert_eq!(submission_batch_end_indexes(1), vec![1]);
        assert_eq!(submission_batch_end_indexes(50), vec![50]);
        assert_eq!(submission_batch_end_indexes(51), vec![50, 51]);
        assert_eq!(submission_batch_end_indexes(95), vec![50, 95]);
        assert_eq!(submission_batch_end_indexes(115), vec![50, 100, 115]);
        assert_eq!(submission_batch_end_indexes(200), vec![50, 100, 150, 200]);
    }

    #[test]
    fn remote_part_verification_stops_retrying_after_risk_control() {
        assert!(!should_retry_remote_part_verification(Some(
            "request was banned (code: -412)"
        )));
        assert!(!should_retry_remote_part_verification(Some(
            "系统限流 (code: -509)"
        )));
        assert!(should_retry_remote_part_verification(Some(
            "暂未读取到远程稿件分P信息"
        )));
    }

    #[test]
    fn parent_collection_submission_keeps_season_id_in_payload() {
        let mut task = build_test_task(IMPORT_MODE_NON_SEGMENTED, false);
        task.collection_id = Some(1001);
        task.collection_section_id = None;
        let parts = vec![UploadedVideoPart {
            filename: "uploaded-file".to_string(),
            cid: 2001,
            title: "P1".to_string(),
        }];

        let payload = build_add_payload(&task, &parts);

        assert_eq!(payload.get("season_id").and_then(Value::as_i64), Some(1001));
    }

    #[test]
    fn explicit_child_collection_submission_omits_season_id_until_binding() {
        let mut task = build_test_task(IMPORT_MODE_NON_SEGMENTED, false);
        task.collection_id = Some(1001);
        task.collection_section_id = Some(3001);
        let parts = vec![UploadedVideoPart {
            filename: "uploaded-file".to_string(),
            cid: 2001,
            title: "P1".to_string(),
        }];

        let add_payload = build_add_payload(&task, &parts);
        let edit_payload = build_edit_payload(&task, &parts, 4001, false);

        assert!(add_payload.get("season_id").is_none());
        assert!(edit_payload.get("season_id").is_none());
    }

    #[test]
    fn existing_child_collection_edit_keeps_parent_season_id() {
        let mut task = build_test_task(IMPORT_MODE_NON_SEGMENTED, false);
        task.collection_id = Some(1001);
        task.collection_section_id = Some(3001);
        let parts = vec![UploadedVideoPart {
            filename: "uploaded-file".to_string(),
            cid: 2001,
            title: "P1".to_string(),
        }];

        let payload = build_edit_payload(&task, &parts, 4001, true);

        assert_eq!(payload.get("season_id").and_then(Value::as_i64), Some(1001));
    }

    #[test]
    fn collection_bind_pending_error_is_retryable_without_resubmission() {
        let error = build_collection_bind_pending_error("temporary failure");
        assert!(is_collection_bind_pending_error(&error));
    }

    #[test]
    fn legacy_failed_submission_with_bvid_and_minus_702_requires_reconciliation() {
        let mut task = build_test_task(IMPORT_MODE_NON_SEGMENTED, false);
        task.status = "FAILED".to_string();
        task.workflow_status = Some(WorkflowStatusRecord {
            status: "FAILED".to_string(),
            current_step: None,
            progress: 0.0,
            error_message: Some("请求频率过高，请稍后再试 (code: -702)".to_string()),
        });
        assert!(should_reconcile_submission(&task));
        assert!(should_block_submission_retry(&task));
    }

    #[test]
    fn failed_submission_with_remote_transcode_failure_allows_repost() {
        let mut task = build_test_task(IMPORT_MODE_NON_SEGMENTED, false);
        task.status = "FAILED".to_string();
        task.remote_state = Some(REMOTE_STATE_TRANSCODE_FAILED);
        task.workflow_status = Some(WorkflowStatusRecord {
            status: "FAILED".to_string(),
            current_step: None,
            progress: 0.0,
            error_message: Some("远程分P数量与本地不一致（本地=158，远程=50）".to_string()),
        });
        assert!(!should_reconcile_submission(&task));
        assert!(!should_block_submission_retry(&task));
    }

    #[test]
    fn submission_owner_lock_does_not_replace_active_worker() {
        let mut active_owners = HashMap::new();
        assert!(try_acquire_submission_owner(
            &mut active_owners,
            82679456,
            "worker-1"
        ));
        assert!(!try_acquire_submission_owner(
            &mut active_owners,
            82679456,
            "worker-2"
        ));
        assert_eq!(
            active_owners.get(&82679456).map(String::as_str),
            Some("worker-1")
        );
    }

    #[test]
    fn submission_owner_lock_can_only_be_released_by_holder() {
        let mut active_owners = HashMap::from([(82679456, "worker-2".to_string())]);
        assert!(!release_submission_owner(
            &mut active_owners,
            82679456,
            "worker-1"
        ));
        assert!(active_owners.contains_key(&82679456));
        assert!(release_submission_owner(
            &mut active_owners,
            82679456,
            "worker-2"
        ));
        assert!(!active_owners.contains_key(&82679456));
    }

    #[test]
    fn upload_retry_delay_uses_bounded_exponential_backoff() {
        assert_eq!(upload_retry_delay_secs(1), 5);
        assert_eq!(upload_retry_delay_secs(2), 10);
        assert_eq!(upload_retry_delay_secs(3), 20);
        assert_eq!(upload_retry_delay_secs(4), 40);
        assert_eq!(upload_retry_delay_secs(5), 60);
        assert_eq!(upload_retry_delay_secs(10), 60);
    }

    #[test]
    fn transient_upload_response_preserves_session() {
        assert!(!should_reset_upload_session_for_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "upstream reset"
        ));
        assert!(!should_reset_upload_session_for_response(
            StatusCode::REQUEST_TIMEOUT,
            "timeout"
        ));
        assert!(should_reset_upload_session_for_response(
            StatusCode::BAD_REQUEST,
            "upload session expired"
        ));
    }

    #[test]
    fn upload_transport_error_detection_matches_cdn_disconnects() {
        assert!(is_upload_transport_error(
            "上传分片失败: error sending request <- connection error <- Connection reset by peer"
        ));
        assert!(is_upload_transport_error(
            "上传分片失败: peer closed connection without sending TLS close_notify: unexpected EOF"
        ));
        assert!(!is_upload_transport_error(
            "预上传请求失败: error sending request"
        ));
        assert!(!is_upload_transport_error("上传分片鉴权失败: code=-101"));
    }

    #[test]
    fn local_address_unavailable_detection_matches_supported_platforms() {
        assert!(is_local_address_unavailable_error(
            "tcp connect error <- Can't assign requested address (os error 49)"
        ));
        assert!(is_local_address_unavailable_error(
            "Cannot assign requested address (os error 99)"
        ));
        assert!(!is_local_address_unavailable_error(
            "Connection reset by peer (os error 54)"
        ));
    }

    #[test]
    fn upload_chunk_size_is_capped_for_slow_links() {
        assert_eq!(
            effective_upload_chunk_size(10 * 1024 * 1024),
            Some(5 * 1024 * 1024)
        );
        assert_eq!(
            effective_upload_chunk_size(4 * 1024 * 1024),
            Some(4 * 1024 * 1024)
        );
        assert_eq!(effective_upload_chunk_size(0), None);
    }

    #[test]
    fn failed_segment_upload_error_is_retryable() {
        assert!(is_retryable_submission_error(
            "存在分段上传失败，请重试失败分P"
        ));
    }

    #[test]
    fn confirmed_remote_missing_can_be_enqueued_without_reprocessing() {
        assert!(supports_direct_upload_enqueue(
            SUBMISSION_STATUS_REUPLOAD_PENDING
        ));
        assert!(supports_direct_upload_enqueue("FAILED"));
        assert!(supports_direct_upload_enqueue("WAITING_UPLOAD"));
        assert!(!supports_direct_upload_enqueue("PENDING_SUBMIT"));
        assert!(!supports_direct_upload_enqueue("SUBMIT_UNKNOWN"));
        assert!(is_confirmed_remote_missing_retry(
            "FAILED",
            Some("已人工确认远端未生成稿件，请重新上传后投稿")
        ));
        assert!(is_confirmed_remote_missing_retry(
            SUBMISSION_STATUS_REUPLOAD_PENDING,
            None
        ));
        assert!(!is_confirmed_remote_missing_retry(
            "FAILED",
            Some("普通上传失败")
        ));
    }

    #[test]
    fn failed_submission_retry_uses_single_upload_worker() {
        assert_eq!(
            effective_submission_upload_concurrency(3, Some("FAILED")),
            1
        );
        assert_eq!(
            effective_submission_upload_concurrency(3, Some("RUNNING")),
            3
        );
        assert_eq!(
            effective_submission_upload_concurrency(3, Some(SUBMISSION_STATUS_REUPLOAD_PENDING)),
            1
        );
        assert_eq!(effective_submission_upload_concurrency(0, None), 1);
    }

    #[test]
    fn remote_audit_parser_reads_identity_time_and_part_count() {
        let info = parse_remote_audit_info(&json!({
            "Archive": {
                "aid": 123,
                "bvid": "BV1test",
                "title": "测试投稿",
                "ctime": 1_800_000_000_i64,
                "state": -1,
                "state_desc": "审核中"
            },
            "cid_list": [1, 2, 3]
        }))
        .expect("parse remote audit");
        assert_eq!(info.aid, 123);
        assert_eq!(info.bvid, "BV1test");
        assert_eq!(info.title, "测试投稿");
        assert_eq!(info.part_count, 3);
        assert_eq!(info.ctime, 1_800_000_000);
        assert_eq!(info.state, -1);
        assert_eq!(info.state_desc.as_deref(), Some("审核中"));
    }

    #[test]
    fn unknown_submission_match_requires_a_unique_unbound_candidate() {
        let task = RemoteTrackedTask {
            task_id: "task-unknown".to_string(),
            bvid: String::new(),
            aid: 0,
            title: "测试投稿".to_string(),
            status: SUBMISSION_STATUS_SUBMIT_UNKNOWN.to_string(),
            remote_state: None,
            remote_status_ignored: false,
            created_at: "2027-01-15T07:59:00+00:00".to_string(),
            reconciliation_at: "2027-01-16T11:00:00+00:00".to_string(),
            error_message: "SUBMIT_RESULT_UNKNOWN: test".to_string(),
            expected_parts: 3,
            collection_section_id: 0,
        };
        let candidate = RemoteAuditInfo {
            aid: 123,
            bvid: "BV1new".to_string(),
            title: "测试投稿".to_string(),
            part_count: 3,
            ctime: 1_800_000_000,
            state: 0,
            state_desc: Some("已通过".to_string()),
            reject_reason: None,
        };
        let mut remote_map = HashMap::new();
        remote_map.insert(candidate.bvid.clone(), candidate.clone());
        let bound_bvids = HashSet::new();
        let matched =
            find_unique_unknown_submission_match(&task, &remote_map, &bound_bvids, 1_800_000_100)
                .expect("unique candidate should match");
        assert_eq!(matched.bvid, "BV1new");

        remote_map.get_mut("BV1new").expect("candidate").part_count = 2;
        assert!(find_unique_unknown_submission_match(
            &task,
            &remote_map,
            &bound_bvids,
            1_800_000_100,
        )
        .is_none());

        remote_map.get_mut("BV1new").expect("candidate").part_count = 3;

        let mut duplicate = candidate;
        duplicate.bvid = "BV1duplicate".to_string();
        duplicate.aid = 456;
        remote_map.insert(duplicate.bvid.clone(), duplicate);
        assert!(find_unique_unknown_submission_match(
            &task,
            &remote_map,
            &bound_bvids,
            1_800_000_100,
        )
        .is_none());
    }

    fn build_remote_refresh_task(
        status: &str,
        bvid: &str,
        remote_state: Option<i64>,
    ) -> RemoteTrackedTask {
        RemoteTrackedTask {
            task_id: "task-refresh".to_string(),
            bvid: bvid.to_string(),
            aid: 123,
            title: "测试投稿".to_string(),
            status: status.to_string(),
            remote_state,
            remote_status_ignored: false,
            created_at: "2027-01-15T07:59:00+00:00".to_string(),
            reconciliation_at: "2027-01-15T07:59:00+00:00".to_string(),
            error_message: String::new(),
            expected_parts: 1,
            collection_section_id: 0,
        }
    }

    #[test]
    fn completed_task_with_missing_remote_state_requires_refresh() {
        assert!(build_remote_refresh_task("COMPLETED", "BV1test", None).needs_remote_refresh());
        assert!(build_remote_refresh_task("COMPLETED", "BV1test", Some(-1)).needs_remote_refresh());
        assert!(build_remote_refresh_task("COMPLETED", "BV1test", Some(-30)).needs_remote_refresh());
        assert!(build_remote_refresh_task("COMPLETED", "BV1test", Some(-60)).needs_remote_refresh());
        assert!(build_remote_refresh_task("COMPLETED", "BV1test", Some(-2)).needs_remote_refresh());
        assert!(build_remote_refresh_task("COMPLETED", "BV1test", Some(-4)).needs_remote_refresh());
        assert!(!build_remote_refresh_task("COMPLETED", "BV1test", Some(0)).needs_remote_refresh());
        assert!(!build_remote_refresh_task("COMPLETED", "", None).needs_remote_refresh());
    }

    #[test]
    fn ignored_task_is_not_refreshed_even_when_state_is_missing() {
        let mut task = build_remote_refresh_task("COMPLETED", "BV1test", None);
        task.remote_status_ignored = true;
        assert!(!task.needs_remote_refresh());
    }
}
