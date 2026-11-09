use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::{DateTime, Duration, Utc};
use rusqlite::params_from_iter;
use rusqlite::types::Value as SqlValue;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::State;

use crate::api::ApiResponse;
use crate::commands::settings::{default_live_settings, load_live_settings_from_db};
use crate::live_recorder::{fetch_room_info, start_recording, stop_recording, LiveContext};
use crate::utils::{append_log, now_rfc3339};
use crate::AppState;

const LIVE_ROOM_INFO_URL: &str = "https://api.live.bilibili.com/room/v1/Room/get_info";
const LIVE_USER_INFO_URL: &str = "https://api.live.bilibili.com/live_user/v1/Master/info";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeRequest {
    pub uids: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Anchor {
    pub id: i64,
    pub uid: String,
    pub nickname: Option<String>,
    pub live_status: i64,
    pub last_check_time: Option<String>,
    pub create_time: String,
    pub update_time: String,
    pub avatar_url: Option<String>,
    pub live_title: Option<String>,
    pub category: Option<String>,
    pub auto_record: bool,
    pub baidu_sync_enabled: bool,
    pub baidu_sync_path: Option<String>,
    pub record_mode: String,
    pub record_dates: Vec<String>,
    pub keep_record_after: bool,
    /// 该主播是否开启自动切片（主播级设置，未设置时回落到全局开关）
    pub auto_clip: bool,
    pub recording_status: Option<String>,
    pub recording_file: Option<String>,
    pub recording_start_time: Option<String>,
}

/// 全局自动切片开关，用于主播未单独设置时的回落值
fn global_auto_clip_enabled(db: &crate::db::Db) -> bool {
    crate::highlight::settings::load(db).clip.enabled
}

struct AnchorLiveInfo {
    nickname: Option<String>,
    live_status: i64,
    avatar_url: Option<String>,
    live_title: Option<String>,
    category: Option<String>,
}

#[tauri::command]
pub async fn anchor_subscribe(
    state: State<'_, AppState>,
    payload: SubscribeRequest,
) -> Result<ApiResponse<Vec<Anchor>>, String> {
    let now = now_rfc3339();
    let settings =
        load_live_settings_from_db(&state.db).unwrap_or_else(|_| default_live_settings());
    let context = LiveContext {
        db: state.db.clone(),
        bilibili: state.bilibili.clone(),
        login_store: state.login_store.clone(),
        app_log_path: state.app_log_path.clone(),
        live_runtime: state.live_runtime.clone(),
    };
    append_log(
        &state.app_log_path,
        &format!("anchor_subscribe_start uids={}", payload.uids.join(",")),
    );

    for uid in payload.uids {
        let uid = uid.trim().to_string();
        if uid.is_empty() {
            continue;
        }

        let info = match fetch_live_info(&state, &uid).await {
            Ok(value) => value,
            Err(_) => AnchorLiveInfo {
                nickname: None,
                live_status: 0,
                avatar_url: None,
                live_title: None,
                category: None,
            },
        };

        let result = state.db.with_conn(|conn| {
      conn.execute(
        "INSERT INTO anchor (uid, nickname, live_status, last_check_time, create_time, update_time) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT(uid) DO UPDATE SET \
         nickname = excluded.nickname, \
         live_status = excluded.live_status, \
         last_check_time = excluded.last_check_time, \
         update_time = excluded.update_time",
        (
          &uid,
          info.nickname.as_deref(),
          info.live_status,
          &now,
          &now,
          &now,
        ),
      )?;
      conn.execute(
        "INSERT INTO live_room_settings (room_id, auto_record, update_time) VALUES (?1, 1, ?2) \
         ON CONFLICT(room_id) DO UPDATE SET update_time = excluded.update_time",
        (&uid, &now),
      )?;
      Ok(())
    });

        if let Err(err) = result {
            append_log(
                &state.app_log_path,
                &format!("anchor_subscribe_error uid={} err={}", uid, err),
            );
            return Ok(ApiResponse::error("Failed to subscribe anchor"));
        }

        if info.live_status == 1 {
            if let Ok(room_info) = fetch_room_info(&state.bilibili, &uid).await {
                if !state.live_runtime.is_recording(&uid)
                    && crate::live_recorder::should_auto_record_now(
                        &state.db,
                        &uid,
                        &state.app_log_path,
                    )
                {
                    if let Err(err) =
                        start_recording(context.clone(), &uid, room_info, settings.clone())
                    {
                        append_log(
                            &state.app_log_path,
                            &format!("auto_record_subscribe_failed room={} err={}", uid, err),
                        );
                    } else {
                        append_log(
                            &state.app_log_path,
                            &format!("auto_record_subscribe_start room={}", uid),
                        );
                    }
                }
            }
        }
    }

    Ok(anchor_list(state))
}

#[tauri::command(async)]
pub fn anchor_list(state: State<'_, AppState>) -> ApiResponse<Vec<Anchor>> {
    // 全局开关要在加连接锁之前读好：settings::load 内部也会 with_conn，
    // 若放进下面的查询闭包里会重复加锁导致死锁。
    let global_auto_clip = global_auto_clip_enabled(state.db.as_ref());
    match state.db.with_conn(|conn| {
    let mut stmt = conn.prepare(
      "SELECT a.id, a.uid, a.nickname, a.live_status, a.last_check_time, a.create_time, a.update_time, IFNULL(l.auto_record, 1), IFNULL(l.baidu_sync_enabled, 0), l.baidu_sync_path, IFNULL(l.record_mode, 'DAILY'), l.record_dates, IFNULL(l.keep_record_after, 0), l.auto_clip \
       FROM anchor a LEFT JOIN live_room_settings l ON a.uid = l.room_id ORDER BY a.id DESC",
    )?;
    let anchors = stmt
      .query_map([], |row| {
        let uid: String = row.get(1)?;
        let auto_record: i64 = row.get(7)?;
        let sync_enabled: i64 = row.get(8)?;
        let record_info = state.live_runtime.get_record_info(&uid);
        Ok(Anchor {
          id: row.get(0)?,
          uid: uid.clone(),
          nickname: row.get(2)?,
          live_status: row.get::<_, i64>(3)?,
          last_check_time: row.get(4)?,
          create_time: row.get(5)?,
          update_time: row.get(6)?,
          avatar_url: None,
          live_title: None,
          category: None,
          auto_record: auto_record != 0,
          baidu_sync_enabled: sync_enabled != 0,
          baidu_sync_path: row.get(9)?,
          record_mode: row.get(10)?,
          record_dates: crate::live_recorder::parse_record_dates_public(row.get(11)?),
          keep_record_after: row.get::<_, i64>(12)? != 0,
          auto_clip: row.get::<_, Option<i64>>(13)?.map(|v| v != 0).unwrap_or(global_auto_clip),
          recording_status: record_info.as_ref().map(|_| "RECORDING".to_string()),
          recording_file: record_info.as_ref().map(|info| info.file_path.clone()),
          recording_start_time: record_info.map(|info| info.start_time),
        })
      })?
      .collect::<Result<Vec<_>, _>>()?;
    Ok(anchors)
  }) {
    Ok(list) => ApiResponse::success(list),
    Err(err) => ApiResponse::error(format!("Failed to load anchors: {}", err)),
  }
}

#[tauri::command(async)]
pub fn anchor_unsubscribe(state: State<'_, AppState>, uid: String) -> ApiResponse<String> {
    let context = LiveContext {
        db: state.db.clone(),
        bilibili: state.bilibili.clone(),
        login_store: state.login_store.clone(),
        app_log_path: state.app_log_path.clone(),
        live_runtime: state.live_runtime.clone(),
    };
    stop_recording(context, &uid, "取消订阅");
    let uid_value = uid;
    match state.db.with_conn(|conn| {
        conn.execute("DELETE FROM anchor WHERE uid = ?1", [uid_value.as_str()])?;
        conn.execute(
            "DELETE FROM live_room_settings WHERE room_id = ?1",
            [uid_value.as_str()],
        )?;
        Ok(())
    }) {
        Ok(()) => ApiResponse::success("Unsubscribed".to_string()),
        Err(err) => ApiResponse::error(format!("Failed to unsubscribe: {}", err)),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordTaskRecord {
    pub id: i64,
    pub room_id: String,
    pub status: String,
    pub file_path: String,
    pub segment_index: i64,
    pub start_time: String,
    pub end_time: Option<String>,
    pub file_size: i64,
    pub title: Option<String>,
    pub error_message: Option<String>,
    pub create_time: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveClipTaskStatus {
    pub status: String,
    pub clip_count: i64,
    pub error_message: Option<String>,
    pub stage: Option<String>,
    pub progress: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveClipItem {
    pub id: i64,
    pub source_file_path: Option<String>,
    pub status: String,
    pub file_path: String,
    pub start_offset: f64,
    pub end_offset: f64,
    pub duration: f64,
    pub peak_count: i64,
    pub create_time: String,
    pub score: f64,
    pub signals: Option<Value>,
    pub category: Option<String>,
    pub titles: Vec<String>,
    pub description: Option<String>,
    pub tags: Option<String>,
    pub cover_text: Option<String>,
    pub reason: Option<String>,
    pub evidence: Vec<String>,
    pub ai_status: Option<String>,
    pub mark: String,
    /// 封面参数（画面帧 + 各文字块的位置、大小、旋转、颜色），投稿时按此渲染
    pub cover_spec: Option<Value>,
}

const CLIP_ITEM_COLUMNS: &str = "id, source_file_path, status, file_path, start_offset, end_offset, duration, peak_count, create_time, \
     score, signals, category, titles, description, tags, cover_text, reason, evidence, ai_status, mark, cover_spec";

fn map_clip_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<LiveClipItem> {
    let json_list = |v: Option<String>| -> Vec<String> { v.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default() };
    Ok(LiveClipItem {
        id: row.get(0)?,
        source_file_path: row.get(1)?,
        status: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
        file_path: row.get(3)?,
        start_offset: row.get(4)?,
        end_offset: row.get(5)?,
        duration: row.get(6)?,
        peak_count: row.get::<_, Option<i64>>(7)?.unwrap_or(0),
        create_time: row.get(8)?,
        score: row.get::<_, Option<f64>>(9)?.unwrap_or(0.0),
        signals: row.get::<_, Option<String>>(10)?.and_then(|s| serde_json::from_str(&s).ok()),
        category: row.get(11)?,
        titles: json_list(row.get(12)?),
        description: row.get(13)?,
        tags: row.get(14)?,
        cover_text: row.get(15)?,
        reason: row.get(16)?,
        evidence: json_list(row.get(17)?),
        ai_status: row.get(18)?,
        mark: row.get::<_, Option<String>>(19)?.unwrap_or_else(|| "PENDING".to_string()),
        cover_spec: row.get::<_, Option<String>>(20)?.and_then(|s| serde_json::from_str(&s).ok()),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnchorSubmissionConfig {
    pub room_id: String,
    pub title: String,
    pub description: Option<String>,
    pub partition_id: i64,
    pub collection_id: Option<i64>,
    pub tags: Option<String>,
    pub topic_id: Option<i64>,
    pub mission_id: Option<i64>,
    pub activity_title: Option<String>,
    pub video_type: String,
    pub create_time: String,
    pub update_time: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnchorSubmissionConfigInput {
    pub room_id: String,
    pub title: String,
    pub description: Option<String>,
    pub partition_id: Option<i64>,
    pub collection_id: Option<i64>,
    pub tags: Option<String>,
    pub topic_id: Option<i64>,
    pub mission_id: Option<i64>,
    pub activity_title: Option<String>,
    pub video_type: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveClipListResponse {
    pub task_id: i64,
    pub room_id: String,
    pub date_label: String,
    pub status: String,
    pub clip_count: i64,
    pub error_message: Option<String>,
    pub items: Vec<LiveClipItem>,
}

#[derive(Clone)]
pub(crate) struct LiveRecordSegment {
    pub(crate) record_id: i64,
    pub(crate) file_path: String,
    pub(crate) start_time: DateTime<Utc>,
    pub(crate) end_time: DateTime<Utc>,
    pub(crate) offset_start: i64,
    pub(crate) offset_end: i64,
}

#[tauri::command(async)]
pub fn anchor_record_list(
    state: State<'_, AppState>,
    room_id: String,
) -> ApiResponse<Vec<RecordTaskRecord>> {
    match state.db.with_conn(|conn| {
    let mut stmt = conn.prepare(
      "SELECT id, room_id, status, file_path, segment_index, start_time, end_time, file_size, title, error_message, create_time \
       FROM live_record_task WHERE room_id = ?1 ORDER BY id DESC LIMIT 200",
    )?;
    let rows = stmt.query_map([room_id.as_str()], |row| {
      Ok(RecordTaskRecord {
        id: row.get(0)?,
        room_id: row.get(1)?,
        status: row.get(2)?,
        file_path: row.get(3)?,
        segment_index: row.get(4)?,
        start_time: row.get(5)?,
        end_time: row.get(6)?,
        file_size: row.get::<_, Option<i64>>(7)?.unwrap_or(0),
        title: row.get(8)?,
        error_message: row.get(9)?,
        create_time: row.get(10)?,
      })
    })?;
    let mut records = Vec::new();
    for row in rows {
      records.push(row?);
    }
    Ok(records)
  }) {
    Ok(records) => ApiResponse::success(records),
    Err(err) => ApiResponse::error(format!("查询录播记录失败: {}", err)),
  }
}

#[tauri::command(async)]
pub fn anchor_submission_config_get(
    state: State<'_, AppState>,
    room_id: String,
) -> ApiResponse<Option<AnchorSubmissionConfig>> {
    let room_id = room_id.trim().to_string();
    if room_id.is_empty() {
        return ApiResponse::error("room_id 不能为空");
    }
    let result = state.db.with_conn(|conn| {
    conn.query_row(
      "SELECT room_id, title, description, partition_id, collection_id, tags, topic_id, mission_id, activity_title, video_type, create_time, update_time \
       FROM anchor_submission_config WHERE room_id = ?1",
      [room_id.as_str()],
      |row| {
        Ok(AnchorSubmissionConfig {
          room_id: row.get(0)?,
          title: row.get(1)?,
          description: row.get(2)?,
          partition_id: row.get(3)?,
          collection_id: row.get(4)?,
          tags: row.get(5)?,
          topic_id: row.get(6)?,
          mission_id: row.get(7)?,
          activity_title: row.get(8)?,
          video_type: row.get(9)?,
          create_time: row.get(10)?,
          update_time: row.get(11)?,
        })
      },
    )
  });
    match result {
        Ok(config) => ApiResponse::success(Some(config)),
        Err(crate::db::DbError::Sql(rusqlite::Error::QueryReturnedNoRows)) => {
            ApiResponse::success(None)
        }
        Err(err) => ApiResponse::error(format!("读取投稿配置失败: {}", err)),
    }
}

#[tauri::command(async)]
pub fn anchor_submission_config_save(
    state: State<'_, AppState>,
    config: AnchorSubmissionConfigInput,
) -> ApiResponse<String> {
    let room_id = config.room_id.trim().to_string();
    if room_id.is_empty() {
        return ApiResponse::error("room_id 不能为空");
    }
    let title = config.title.trim().to_string();
    if title.is_empty() {
        return ApiResponse::error("请输入投稿标题");
    }
    if title.chars().count() > 80 {
        return ApiResponse::error("投稿标题不能超过 80 个字符");
    }
    let partition_id = config.partition_id.unwrap_or(0);
    if partition_id <= 0 {
        return ApiResponse::error("请选择B站分区");
    }
    let video_type = config.video_type.trim().to_string();
    if video_type.is_empty() {
        return ApiResponse::error("请选择视频类型");
    }
    let tags_raw = config.tags.unwrap_or_default();
    let tags = tags_raw
        .split(',')
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
        .collect::<Vec<_>>();
    if tags.is_empty() {
        return ApiResponse::error("请填写至少一个投稿标签");
    }
    let tags_joined = tags.join(",");
    let description = config
        .description
        .as_ref()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if let Some(desc) = description.as_ref() {
        if desc.chars().count() > 2000 {
            return ApiResponse::error("视频描述不能超过 2000 个字符");
        }
    }
    let now = now_rfc3339();
    let result = state.db.with_conn_mut(|conn| {
    conn.execute(
      "INSERT INTO anchor_submission_config (room_id, title, description, partition_id, collection_id, tags, topic_id, mission_id, activity_title, video_type, create_time, update_time) \
       VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12) \
       ON CONFLICT(room_id) DO UPDATE SET \
         title = excluded.title, \
         description = excluded.description, \
         partition_id = excluded.partition_id, \
         collection_id = excluded.collection_id, \
         tags = excluded.tags, \
         topic_id = excluded.topic_id, \
         mission_id = excluded.mission_id, \
         activity_title = excluded.activity_title, \
         video_type = excluded.video_type, \
         update_time = excluded.update_time",
      (
        room_id.as_str(),
        title.as_str(),
        description.as_deref(),
        partition_id,
        config.collection_id,
        tags_joined.as_str(),
        config.topic_id,
        config.mission_id,
        config.activity_title.as_deref(),
        video_type.as_str(),
        &now,
        &now,
      ),
    )?;
    Ok(())
  });
    match result {
        Ok(()) => ApiResponse::success("保存成功".to_string()),
        Err(err) => ApiResponse::error(format!("保存投稿配置失败: {}", err)),
    }
}

#[tauri::command(async)]
pub fn anchor_open_record_dir(file_path: String) -> ApiResponse<String> {
    let path = Path::new(file_path.trim());
    let dir = path.parent().unwrap_or(path);
    if !dir.exists() {
        return ApiResponse::error(format!("目录不存在: {}", dir.to_string_lossy()));
    }
    let result: Result<(), String> = {
        #[cfg(target_os = "macos")]
        {
            Command::new("open")
                .arg(dir.as_os_str())
                .spawn()
                .map(|_| ())
                .map_err(|err| format!("打开目录失败: {}", err))
        }
        #[cfg(target_os = "windows")]
        {
            Command::new("explorer")
                .arg(dir.as_os_str())
                .spawn()
                .map(|_| ())
                .map_err(|err| format!("打开目录失败: {}", err))
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            Command::new("xdg-open")
                .arg(dir.as_os_str())
                .spawn()
                .map(|_| ())
                .map_err(|err| format!("打开目录失败: {}", err))
        }
    };
    match result {
        Ok(()) => ApiResponse::success(dir.to_string_lossy().to_string()),
        Err(err) => ApiResponse::error(err),
    }
}

#[tauri::command(async)]
pub fn anchor_open_record_file(file_path: String) -> ApiResponse<String> {
    let path = Path::new(file_path.trim());
    match tauri_plugin_opener::open_path(path, None::<&str>)
        .map_err(|err| format!("打开文件失败: {}", err))
    {
        Ok(()) => ApiResponse::success("已打开".to_string()),
        Err(err) => ApiResponse::error(err),
    }
}

#[tauri::command(async)]
pub fn anchor_analyze_clips(
    state: State<'_, AppState>,
    room_id: String,
    record_ids: Vec<i64>,
) -> ApiResponse<i64> {
    let room_id = room_id.trim().to_string();
    if room_id.is_empty() {
        return ApiResponse::error("room_id 不能为空");
    }
    let normalized_ids = normalize_record_ids(&record_ids);
    if normalized_ids.is_empty() {
        return ApiResponse::error("record_ids 无效");
    }
    match crate::highlight::create_task(&state.db, &room_id, &normalized_ids, "MANUAL", None) {
        Ok(task_id) => ApiResponse::success(task_id),
        Err(err) => ApiResponse::error(err),
    }
}

#[tauri::command(async)]
pub fn anchor_clip_task_status(
    state: State<'_, AppState>,
    task_id: i64,
) -> ApiResponse<LiveClipTaskStatus> {
    if task_id <= 0 {
        return ApiResponse::error("task_id 无效");
    }
    let result = state.db.with_conn(|conn| {
        conn.query_row(
            "SELECT status, clip_count, error_message, stage, progress FROM live_clip_task WHERE id = ?1",
            [task_id],
            |row| {
                Ok(LiveClipTaskStatus {
                    status: row.get(0)?,
                    clip_count: row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                    error_message: row.get(2)?,
                    stage: row.get(3)?,
                    progress: row.get::<_, Option<f64>>(4)?.unwrap_or(0.0),
                })
            },
        )
    });
    match result {
        Ok(value) => ApiResponse::success(value),
        Err(err) => ApiResponse::error(format!("读取切片任务失败: {}", err)),
    }
}

#[tauri::command(async)]
pub fn anchor_clip_update_time(
    state: State<'_, AppState>,
    clip_id: i64,
    start_offset: f64,
    end_offset: f64,
) -> ApiResponse<LiveClipItem> {
    if clip_id <= 0 {
        return ApiResponse::error("clip_id 无效");
    }
    if start_offset < 0.0 || end_offset <= start_offset {
        return ApiResponse::error("时间段无效");
    }
    let duration = end_offset - start_offset;
    let update_result = state.db.with_conn_mut(|conn| {
        let changed = conn.execute(
      "UPDATE live_clip_item SET start_offset = ?1, end_offset = ?2, duration = ?3 WHERE id = ?4",
      (start_offset, end_offset, duration, clip_id),
    )?;
        Ok(changed)
    });
    if let Err(err) = update_result {
        return ApiResponse::error(format!("更新切片时间失败: {}", err));
    }

    let item_result = state.db.with_conn(|conn| {
        conn.query_row(
            &format!("SELECT {} FROM live_clip_item WHERE id = ?1", CLIP_ITEM_COLUMNS),
            [clip_id],
            map_clip_item,
        )
    });
    match item_result {
        Ok(item) => ApiResponse::success(item),
        Err(err) => ApiResponse::error(format!("读取切片记录失败: {}", err)),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordOverlap {
    pub id: i64,
    pub file_path: String,
    pub start_time: String,
    pub end_time: Option<String>,
    pub file_exists: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordImportItem {
    pub path: String,
    pub file_name: String,
    pub title: String,
    pub start_time: String,
    pub end_time: String,
    pub duration: i64,
    pub size: u64,
    pub has_srt: bool,
    pub has_danmaku: bool,
    /// new 新录播 / imported 已导入过 / invalid 解析不了或房间号不符 / overlap 与库里录播时间重叠
    pub status: String,
    pub reason: Option<String>,
    pub overlaps: Vec<RecordOverlap>,
}

/// 解析录播文件名：录制-房间号-日期-时间[-毫秒]-标题。
/// 带毫秒的是 B 站录播姬格式，时间为录制机本地时间，按北京时间处理；不带毫秒的是本软件格式，时间为 UTC
fn parse_record_file_name(stem: &str) -> Option<(String, DateTime<Utc>, String)> {
    let parts: Vec<&str> = stem.strip_prefix("录制-")?.split('-').collect();
    if parts.len() < 3 || parts[1].len() != 8 || parts[2].len() != 6 {
        return None;
    }
    let naive = chrono::NaiveDateTime::parse_from_str(&format!("{}{}", parts[1], parts[2]), "%Y%m%d%H%M%S").ok()?;
    let ms = parts.get(3).filter(|p| (1..=3).contains(&p.len()) && p.chars().all(|c| c.is_ascii_digit()));
    let start = match ms {
        Some(ms) => {
            let beijing = chrono::FixedOffset::east_opt(8 * 3600)?;
            naive.and_local_timezone(beijing).single()?.with_timezone(&Utc) + Duration::milliseconds(ms.parse().ok()?)
        }
        None => naive.and_utc(),
    };
    let title = parts[if ms.is_some() { 4 } else { 3 }..].join("-");
    Some((parts[0].to_string(), start, title))
}

/// 扫描目录里的录播（mp4/flv），识别同名字幕和弹幕，检查是否已导入或与库里录播时间重叠。只读，不写库
#[tauri::command]
pub async fn anchor_record_import_scan(state: State<'_, AppState>, room_id: String, dir: String) -> Result<ApiResponse<Vec<RecordImportItem>>, String> {
    let db = state.db.clone();
    let result = tauri::async_runtime::spawn_blocking(move || -> Result<Vec<RecordImportItem>, String> {
        let room_id = room_id.trim().to_string();
        let existing: Vec<(i64, String, String, Option<String>)> = db
            .with_conn(|conn| {
                let mut stmt = conn.prepare("SELECT id, file_path, start_time, end_time FROM live_record_task WHERE room_id = ?1")?;
                let rows = stmt.query_map([&room_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
                rows.collect()
            })
            .map_err(|e| format!("读取录播记录失败: {}", e))?;
        let parse = |s: &str| DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc));
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map_err(|e| format!("读取目录失败: {}", e))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file() && matches!(p.extension().and_then(|e| e.to_str()).map(str::to_lowercase).as_deref(), Some("mp4" | "flv")))
            .collect();
        files.sort();
        let mut items = Vec::new();
        for path in files {
            let path_str = path.to_string_lossy().to_string();
            let file_name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let (jsonl, xml) = crate::highlight::danmaku::danmaku_paths(&path_str);
            let mut item = RecordImportItem {
                path: path_str.clone(),
                file_name,
                title: String::new(),
                start_time: String::new(),
                end_time: String::new(),
                duration: 0,
                size: std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
                has_srt: path.with_extension("srt").is_file(),
                has_danmaku: jsonl.is_file() || xml.is_file(),
                status: "new".into(),
                reason: None,
                overlaps: Vec::new(),
            };
            let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let Some((room, start, title)) = parse_record_file_name(&stem) else {
                item.status = "invalid".into();
                item.reason = Some("文件名里解析不出房间号和开始时间".into());
                items.push(item);
                continue;
            };
            item.title = title;
            if room != room_id {
                item.status = "invalid".into();
                item.reason = Some(format!("房间号 {} 与当前主播不符", room));
                items.push(item);
                continue;
            }
            let Some(duration) = probe_duration_seconds(&path_str).filter(|d| *d > 0) else {
                item.status = "invalid".into();
                item.reason = Some("读取不到视频时长".into());
                items.push(item);
                continue;
            };
            let end = start + Duration::seconds(duration);
            item.start_time = start.to_rfc3339();
            item.end_time = end.to_rfc3339();
            item.duration = duration;
            if existing.iter().any(|(_, p, _, _)| *p == path_str) {
                item.status = "imported".into();
                items.push(item);
                continue;
            }
            for (id, file_path, s, e) in &existing {
                let Some(s0) = parse(s) else { continue };
                let e0 = e.as_deref().and_then(parse).unwrap_or(s0);
                if s0 < end && e0 > start {
                    item.overlaps.push(RecordOverlap {
                        id: *id,
                        file_path: file_path.clone(),
                        start_time: s.clone(),
                        end_time: e.clone(),
                        file_exists: Path::new(file_path).exists(),
                    });
                }
            }
            if !item.overlaps.is_empty() {
                item.status = "overlap".into();
            }
            items.push(item);
        }
        Ok(items)
    })
    .await
    .map_err(|e| format!("后台任务异常: {}", e))?;
    Ok(match result {
        Ok(items) => ApiResponse::success(items),
        Err(err) => ApiResponse::error(err),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordImportEntry {
    pub path: String,
    pub title: String,
    pub start_time: String,
    pub end_time: String,
    pub size: i64,
}

/// 登记导入的录播：先删除要替换的旧记录（只删记录，不删文件），再写入新记录；同一文件已登记过则跳过。返回新登记的段数
#[tauri::command]
pub async fn anchor_record_import_commit(
    state: State<'_, AppState>,
    room_id: String,
    entries: Vec<RecordImportEntry>,
    replace_ids: Vec<i64>,
) -> Result<ApiResponse<usize>, String> {
    let db = state.db.clone();
    let log = state.app_log_path.clone();
    let summary = format!("record_import room_id={} replaced={:?}", room_id, replace_ids);
    let result = tauri::async_runtime::spawn_blocking(move || {
        db.with_conn_mut(|conn| {
            let tx = conn.transaction()?;
            for id in &replace_ids {
                tx.execute("DELETE FROM live_record_task WHERE id = ?1 AND room_id = ?2", (id, room_id.trim()))?;
            }
            let now = now_rfc3339();
            let mut inserted = 0;
            for e in &entries {
                let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM live_record_task WHERE file_path = ?1)", [&e.path], |r| r.get(0))?;
                if exists {
                    continue;
                }
                tx.execute(
                    "INSERT INTO live_record_task (room_id, status, file_path, segment_index, start_time, end_time, file_size, title, create_time, update_time, remux_status) \
                     VALUES (?1, 'STOPPED', ?2, 1, ?3, ?4, ?5, ?6, ?7, ?7, 'DONE')",
                    (room_id.trim(), &e.path, &e.start_time, &e.end_time, e.size, &e.title, &now),
                )?;
                inserted += 1;
            }
            tx.commit()?;
            Ok(inserted)
        })
    })
    .await
    .map_err(|e| format!("后台任务异常: {}", e))?;
    Ok(match result {
        Ok(n) => {
            append_log(&log, &format!("{} inserted={}", summary, n));
            ApiResponse::success(n)
        }
        Err(err) => ApiResponse::error(format!("登记录播失败: {}", err)),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverFrame {
    /// 场次时间轴上的秒数
    pub time: f64,
    pub path: String,
    pub data_url: String,
}

fn image_data_url(path: &Path) -> Result<String, String> {
    use base64::Engine;
    let bytes = std::fs::read(path).map_err(|e| format!("读取封面画面失败: {}", e))?;
    Ok(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

/// 封面用的画面帧：从录播截取切片内某一时刻（场次秒数，缺省取切片中点），存到切片目录的 .cache/covers 下
#[tauri::command]
pub async fn anchor_clip_cover_frame(state: State<'_, AppState>, clip_id: i64, time: Option<f64>) -> Result<ApiResponse<CoverFrame>, String> {
    let db = state.db.clone();
    let result = tauri::async_runtime::spawn_blocking(move || -> Result<CoverFrame, String> {
        let (clip_path, source_path, start_in_file, start_offset, end_offset) = clip_source(&db, clip_id)?;
        let time = time.unwrap_or((start_offset + end_offset) / 2.0).clamp(start_offset, end_offset);
        let dir = Path::new(&clip_path).parent().ok_or("切片目录无效")?.join(".cache").join("covers");
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建封面目录失败: {}", e))?;
        let output = dir.join(format!("clip_{}_{}.jpg", clip_id, (time * 1000.0).round() as i64));
        let seek = format!("{:.3}", (start_in_file + time - start_offset).max(0.0));
        let out = output.to_string_lossy().to_string();
        crate::ffmpeg::run_ffmpeg(
            &["-hide_banner", "-loglevel", "error", "-y", "-nostdin", "-ss", &seek, "-i", &source_path, "-frames:v", "1", "-q:v", "2", &out].map(String::from),
        )
        .map_err(|e| format!("截取画面失败: {}", e))?;
        Ok(CoverFrame { time, data_url: image_data_url(&output)?, path: out })
    })
    .await
    .map_err(|e| format!("后台任务异常: {}", e))?;
    Ok(match result {
        Ok(frame) => ApiResponse::success(frame),
        Err(err) => ApiResponse::error(err),
    })
}

/// 读取已截取的封面画面（data URL，供编辑器和投稿渲染使用）
#[tauri::command(async)]
pub fn anchor_clip_cover_image(path: String) -> ApiResponse<String> {
    match image_data_url(Path::new(path.trim())) {
        Ok(url) => ApiResponse::success(url),
        Err(err) => ApiResponse::error(err),
    }
}

/// 保存切片的封面参数
#[tauri::command(async)]
pub fn anchor_clip_cover_save(state: State<'_, AppState>, clip_id: i64, spec: Value) -> ApiResponse<String> {
    let result = state.db.with_conn(|conn| conn.execute("UPDATE live_clip_item SET cover_spec = ?1 WHERE id = ?2", (spec.to_string(), clip_id)));
    match result {
        Ok(_) => ApiResponse::success("已保存".to_string()),
        Err(err) => ApiResponse::error(format!("保存封面失败: {}", err)),
    }
}

fn cover_person_box_key(room_id: &str) -> String {
    format!("cover_person_box:{}", room_id.trim())
}

/// 主播的人物区域（占画面百分比的矩形 {x, y, w, h}），自动排版时避开
#[tauri::command(async)]
pub fn anchor_cover_person_box_get(state: State<'_, AppState>, room_id: String) -> ApiResponse<Option<Value>> {
    let raw = state.db.with_conn(|conn| conn.query_row("SELECT value FROM app_settings WHERE key = ?1", [cover_person_box_key(&room_id)], |r| r.get::<_, String>(0)));
    ApiResponse::success(raw.ok().and_then(|s| serde_json::from_str(&s).ok()))
}

#[tauri::command(async)]
pub fn anchor_cover_person_box_set(state: State<'_, AppState>, room_id: String, rect: Value) -> ApiResponse<String> {
    let result = state.db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO app_settings (key, value, updated_at) VALUES (?1, ?2, ?3) ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            (cover_person_box_key(&room_id), rect.to_string(), now_rfc3339()),
        )
    });
    match result {
        Ok(_) => ApiResponse::success("已保存".to_string()),
        Err(err) => ApiResponse::error(format!("保存人物区域失败: {}", err)),
    }
}

/// 切片对应的录播源：(切片文件, 源文件, 源文件内起点, 场次起点, 场次终点)
fn clip_source(db: &crate::db::Db, clip_id: i64) -> Result<(String, String, f64, f64, f64), String> {
    if clip_id <= 0 {
        return Err("clip_id 无效".to_string());
    }
    let record = db.with_conn(|conn| {
        conn.query_row(
            "SELECT source_file_path, file_path, start_offset, end_offset, task_id, room_id \
       FROM live_clip_item WHERE id = ?1",
            [clip_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, f64>(2)?,
                    row.get::<_, f64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
    });

    let (source_path, clip_path, start_offset, end_offset, task_id, room_id) = match record {
        Ok(value) => value,
        Err(err) => return Err(format!("读取切片记录失败: {}", err)),
    };
    if start_offset < 0.0 || end_offset <= start_offset {
        return Err("时间段无效".to_string());
    }
    // 切片起止是场次时间轴上的秒数，需换算到所在录播文件内的偏移
    let (source_path, start_in_file) = match resolve_source_path_for_clip(&db, task_id, &room_id, start_offset) {
        Some(found) => found,
        None => match source_path.filter(|v| !v.trim().is_empty()) {
            Some(path) => (path, start_offset),
            None => return Err("源视频路径为空，无法剪辑".to_string()),
        },
    };
    let source_path =
        resolve_existing_record_path(&source_path).unwrap_or_else(|| source_path.clone());
    if !Path::new(&source_path).exists() {
        return Err(format!("源视频不存在: {}", source_path));
    }
    Ok((clip_path, source_path, start_in_file, start_offset, end_offset))
}

#[derive(Deserialize)]
pub(crate) struct SubtitleCueInput {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// 投稿工作流用：按切片当前起止从录播重新剪辑；cues 非空时把字幕压进画面。
/// 返回 (生成的视频路径, 时长秒)。cues 的时间是场次时间轴上的秒数（与评阅页一致）。阻塞执行
pub(crate) fn render_subtitled(db: &crate::db::Db, clip_id: i64, cues: Vec<SubtitleCueInput>, style: &Value) -> Result<(String, i64), String> {
    let (clip_path, source_path, start_in_file, start_offset, end_offset) = clip_source(db, clip_id)?;
    let duration = end_offset - start_offset;
    let clip = Path::new(&clip_path);
    let dir = clip.parent().ok_or("切片目录无效")?;
    // 有字幕才生成 ASS，没有就只重剪
    let ass = if cues.is_empty() {
        None
    } else {
        let (width, height) = probe_video_size(&source_path).unwrap_or((1920, 1080));
        let cues: Vec<crate::highlight::cutter::Cue> = cues
            .into_iter()
            .map(|c| crate::highlight::cutter::Cue {
                start: (c.start - start_offset).clamp(0.0, duration),
                end: (c.end - start_offset).clamp(0.0, duration),
                text: c.text,
            })
            .collect();
        let ass = dir.join(".cache").join(format!("burn_{}.ass", clip_id));
        std::fs::create_dir_all(ass.parent().unwrap_or(dir)).map_err(|e| format!("创建字幕目录失败: {}", e))?;
        std::fs::write(&ass, crate::highlight::cutter::build_ass(&cues, style, width, height))
            .map_err(|e| format!("写入字幕文件失败: {}", e))?;
        Some(ass)
    };
    let out_dir = dir.join("subtitled");
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("创建输出目录失败: {}", e))?;
    let output = out_dir.join(clip.file_name().ok_or("切片文件名无效")?);
    crate::highlight::cutter::cut_with(&source_path, start_in_file, duration, &output, ass.as_deref())
        .map_err(|e| format!("生成切片视频失败: {}", e))?;
    let output = output.to_string_lossy().to_string();
    let seconds = probe_duration_seconds(&output).unwrap_or(duration.ceil() as i64);
    Ok((output, seconds))
}

/// 视频画面宽高（ffprobe 读取第一条视频流）
fn probe_video_size(path: &str) -> Option<(u32, u32)> {
    let mut command = Command::new(crate::config::resolve_ffprobe_path());
    crate::utils::apply_no_window(&mut command);
    let output = command
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height", "-of", "csv=p=0:s=x", path])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let (w, h) = text.trim().split_once('x')?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
}

#[tauri::command(async)]
pub fn anchor_clip_reclip(state: State<'_, AppState>, clip_id: i64) -> ApiResponse<String> {
    let (clip_path, source_path, start_in_file, start_offset, end_offset) = match clip_source(&state.db, clip_id) {
        Ok(value) => value,
        Err(err) => return ApiResponse::error(err),
    };
    let _ = state.db.with_conn_mut(|conn| {
        conn.execute(
            "UPDATE live_clip_item SET status = 'RUNNING', source_file_path = ?1 WHERE id = ?2",
            (&source_path, clip_id),
        )?;
        Ok(())
    });
    let duration = end_offset - start_offset;
    let db = state.db.clone();
    let clip_id_copy = clip_id;
    let clip_path_copy = clip_path.clone();
    let source_path_copy = source_path.clone();
    std::thread::spawn(move || {
        match crate::highlight::cutter::cut(&source_path_copy, start_in_file, duration, std::path::Path::new(&clip_path_copy)) {
            Ok(_) => {
                let actual_duration = probe_duration_seconds(&clip_path_copy).map(|d| d as f64).unwrap_or(duration);
                let _ = db.with_conn_mut(|conn| {
                    conn.execute(
                        "UPDATE live_clip_item SET duration = ?1, status = 'SUCCESS' WHERE id = ?2",
                        (actual_duration, clip_id_copy),
                    )?;
                    Ok(())
                });
            }
            Err(_) => {
                let _ = db.with_conn_mut(|conn| {
                    conn.execute(
                        "UPDATE live_clip_item SET status = 'FAILED' WHERE id = ?1",
                        [clip_id_copy],
                    )?;
                    Ok(())
                });
            }
        }
    });
    ApiResponse::success("已开始重新剪辑".to_string())
}

#[tauri::command(async)]
pub fn anchor_clip_delete(state: State<'_, AppState>, clip_id: i64) -> ApiResponse<String> {
    if clip_id <= 0 {
        return ApiResponse::error("clip_id 无效");
    }
    let record = state.db.with_conn(|conn| {
        conn.query_row(
            "SELECT file_path, task_id FROM live_clip_item WHERE id = ?1",
            [clip_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
    });
    let (file_path, task_id) = match record {
        Ok(value) => value,
        Err(err) => return ApiResponse::error(format!("读取切片记录失败: {}", err)),
    };
    let file_path = file_path.trim().to_string();
    if file_path.is_empty() {
        return ApiResponse::error("切片文件路径为空，无法删除");
    }
    let file = Path::new(&file_path);
    if file.exists() {
        if let Err(err) = std::fs::remove_file(file) {
            return ApiResponse::error(format!("删除切片文件失败: {}", err));
        }
    }
    let delete_result = state.db.with_conn_mut(|conn| {
        conn.execute("DELETE FROM live_clip_item WHERE id = ?1", [clip_id])?;
        Ok(())
    });
    if let Err(err) = delete_result {
        return ApiResponse::error(format!("删除切片记录失败: {}", err));
    }
    let now = now_rfc3339();
    let _ = state.db.with_conn_mut(|conn| {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM live_clip_item WHERE task_id = ?1",
                [task_id],
                |row| row.get(0),
            )
            .unwrap_or(0);
        conn.execute(
            "UPDATE live_clip_task SET clip_count = ?1, update_time = ?2 WHERE id = ?3",
            (count, &now, task_id),
        )?;
        Ok(())
    });
    ApiResponse::success("删除成功".to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveClipSessionSummary {
    pub task_id: i64,
    pub record_ids: Vec<i64>,
    pub status: String,
    pub stage: Option<String>,
    pub progress: f64,
    pub error_message: Option<String>,
    pub clip_count: i64,
    pub reviewed: i64,
    pub passed: i64,
}

/// 房间每批录播最近一次切片任务的状态与评阅进度，供录播管理页的场次列表使用
#[tauri::command(async)]
pub fn anchor_clip_sessions(state: State<'_, AppState>, room_id: String) -> ApiResponse<Vec<LiveClipSessionSummary>> {
    let result = state.db.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT t.id, t.record_ids, t.status, t.stage, t.progress, t.error_message, \
               (SELECT COUNT(*) FROM live_clip_item i WHERE i.task_id = t.id), \
               (SELECT COUNT(*) FROM live_clip_item i WHERE i.task_id = t.id AND i.mark <> 'PENDING'), \
               (SELECT COUNT(*) FROM live_clip_item i WHERE i.task_id = t.id AND i.mark = 'KEEP') \
             FROM live_clip_task t WHERE t.room_id = ?1 AND t.id IN \
               (SELECT MAX(id) FROM live_clip_task WHERE room_id = ?1 GROUP BY record_ids) \
             ORDER BY t.id DESC",
        )?;
        let rows = stmt.query_map([room_id.trim()], |row| {
            Ok(LiveClipSessionSummary {
                task_id: row.get(0)?,
                record_ids: serde_json::from_str(&row.get::<_, String>(1)?).unwrap_or_default(),
                status: row.get(2)?,
                stage: row.get(3)?,
                progress: row.get::<_, Option<f64>>(4)?.unwrap_or(0.0),
                error_message: row.get(5)?,
                clip_count: row.get(6)?,
                reviewed: row.get(7)?,
                passed: row.get(8)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
    });
    match result {
        Ok(list) => ApiResponse::success(list),
        Err(err) => ApiResponse::error(format!("读取切片任务失败: {}", err)),
    }
}

/// 评阅页的场次上下文：每分钟弹幕人数、录播文件位置、整场字幕（可选）
#[tauri::command(async)]
pub fn anchor_clip_session_context(
    state: State<'_, AppState>,
    room_id: String,
    record_ids: Vec<i64>,
    include_transcript: bool,
) -> ApiResponse<Value> {
    let ids = normalize_record_ids(&record_ids);
    if ids.is_empty() {
        return ApiResponse::error("record_ids 无效");
    }
    match crate::highlight::session_context(&state.db, room_id.trim(), &ids, include_transcript) {
        Ok(value) => ApiResponse::success(value),
        Err(err) => ApiResponse::error(err),
    }
}

/// 一场直播的弹幕、SC、礼物、上舰记录
#[tauri::command(async)]
pub fn anchor_session_events(state: State<'_, AppState>, room_id: String, record_ids: Vec<i64>) -> ApiResponse<Value> {
    let ids = normalize_record_ids(&record_ids);
    if ids.is_empty() {
        return ApiResponse::error("record_ids 无效");
    }
    match crate::highlight::session_events(&state.db, room_id.trim(), &ids) {
        Ok(value) => ApiResponse::success(value),
        Err(err) => ApiResponse::error(err),
    }
}

/// 保存一句字幕的人工修订，text 为空时恢复识别原文
#[tauri::command(async)]
pub fn anchor_clip_subtitle_edit(
    state: State<'_, AppState>,
    room_id: String,
    record_ids: Vec<i64>,
    key: String,
    text: String,
    cues: Option<Vec<crate::highlight::EditedCue>>,
) -> ApiResponse<String> {
    let ids = normalize_record_ids(&record_ids);
    if ids.is_empty() || key.trim().is_empty() {
        return ApiResponse::error("参数无效");
    }
    match crate::highlight::save_subtitle_edit(&state.db, room_id.trim(), &ids, key.trim(), &text, cues) {
        Ok(()) => ApiResponse::success("已保存".to_string()),
        Err(err) => ApiResponse::error(err),
    }
}

/// 评阅页播放用的预览片段（场次时间轴上的 [start, end)）
#[tauri::command(async)]
pub fn anchor_clip_preview(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    room_id: String,
    record_ids: Vec<i64>,
    start: f64,
    end: f64,
) -> ApiResponse<Value> {
    let ids = normalize_record_ids(&record_ids);
    if ids.is_empty() || end <= start {
        return ApiResponse::error("预览范围无效");
    }
    use tauri::Manager;
    let dir = match app.path().app_cache_dir() {
        Ok(dir) => dir.join("clip-preview"),
        Err(err) => return ApiResponse::error(format!("无法定位缓存目录: {}", err)),
    };
    match crate::highlight::preview_clip(&state.db, room_id.trim(), &ids, start, end, &dir) {
        Ok(value) => ApiResponse::success(value),
        Err(err) => ApiResponse::error(err),
    }
}

pub(super) const SUBTITLE_STYLE_KEY: &str = "clip_subtitle_style";

/// 评阅页字幕样式（全局一份，前端定义结构，后端只存取 JSON）
#[tauri::command(async)]
pub fn clip_subtitle_style_get(state: State<'_, AppState>) -> ApiResponse<Option<Value>> {
    let raw = state.db.with_conn(|conn| {
        conn.query_row("SELECT value FROM app_settings WHERE key = ?1", [SUBTITLE_STYLE_KEY], |r| r.get::<_, String>(0))
    });
    ApiResponse::success(raw.ok().and_then(|s| serde_json::from_str(&s).ok()))
}

#[tauri::command(async)]
pub fn clip_subtitle_style_set(state: State<'_, AppState>, style: Value) -> ApiResponse<String> {
    let now = crate::utils::now_rfc3339();
    let result = state.db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO app_settings (key, value, updated_at) VALUES (?1, ?2, ?3) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            (SUBTITLE_STYLE_KEY, style.to_string(), &now),
        )
    });
    match result {
        Ok(_) => ApiResponse::success("已保存".to_string()),
        Err(err) => ApiResponse::error(format!("保存字幕样式失败: {}", err)),
    }
}

/// 保存评阅时修改的投稿信息；选定的标题放到第一位
#[tauri::command(async)]
pub fn anchor_clip_update_meta(
    state: State<'_, AppState>,
    clip_id: i64,
    title: String,
    cover_text: String,
    description: String,
    tags: String,
) -> ApiResponse<String> {
    let title = title.trim().to_string();
    if title.is_empty() {
        return ApiResponse::error("标题不能为空");
    }
    let result = state.db.with_conn(|conn| {
        let old: Option<String> = conn.query_row("SELECT titles FROM live_clip_item WHERE id = ?1", [clip_id], |r| r.get(0))?;
        let mut titles: Vec<String> = old.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        titles.retain(|t| t != &title);
        titles.insert(0, title.clone());
        conn.execute(
            "UPDATE live_clip_item SET titles = ?1, cover_text = ?2, description = ?3, tags = ?4 WHERE id = ?5",
            (serde_json::to_string(&titles).unwrap_or_default(), cover_text.trim(), description.trim(), tags.trim(), clip_id),
        )
    });
    match result {
        Ok(_) => ApiResponse::success("已保存".to_string()),
        Err(err) => ApiResponse::error(format!("保存投稿信息失败: {}", err)),
    }
}

#[tauri::command(async)]
pub fn anchor_clip_mark(state: State<'_, AppState>, clip_id: i64, mark: String) -> ApiResponse<String> {
    let mark = mark.trim().to_ascii_uppercase();
    if !["PENDING", "KEEP", "DISCARD", "SUBMITTED"].contains(&mark.as_str()) {
        return ApiResponse::error("标记无效");
    }
    match state.db.with_conn(|c| c.execute("UPDATE live_clip_item SET mark = ?1 WHERE id = ?2", (&mark, clip_id))) {
        Ok(_) => ApiResponse::success(mark),
        Err(err) => ApiResponse::error(format!("更新标记失败: {}", err)),
    }
}

#[tauri::command(async)]
pub fn anchor_clip_list(
    state: State<'_, AppState>,
    room_id: String,
    record_ids: Vec<i64>,
) -> ApiResponse<LiveClipListResponse> {
    let room_id = room_id.trim().to_string();
    if room_id.is_empty() {
        return ApiResponse::error("room_id 不能为空");
    }
    let normalized_ids = normalize_record_ids(&record_ids);
    if normalized_ids.is_empty() {
        return ApiResponse::error("record_ids 无效");
    }
    let mut task_record_ids: Vec<i64> = Vec::new();
    // 自动任务按场次划分，可能与按日期分组的录播不完全一致：优先完全匹配，否则取与这些录播有交集的最近任务
    let wanted: std::collections::HashSet<i64> = normalized_ids.iter().copied().collect();
    let task_row = state.db.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, room_id, date_label, status, clip_count, error_message, record_ids \
       FROM live_clip_task WHERE room_id = ?1 ORDER BY id DESC LIMIT 200",
        )?;
        let rows = stmt.query_map([&room_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<i64>>(4)?.unwrap_or(0),
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
            ))
        })?;
        let tasks = rows.collect::<Result<Vec<_>, _>>()?;
        let ids_of = |raw: &str| -> Vec<i64> { serde_json::from_str(raw).unwrap_or_default() };
        let exact = tasks.iter().find(|t| ids_of(&t.6) == normalized_ids);
        let overlap = tasks.iter().find(|t| ids_of(&t.6).iter().any(|id| wanted.contains(id)));
        exact.or(overlap).cloned().ok_or(rusqlite::Error::QueryReturnedNoRows)
    });
    let task_row = task_row.map(|t| {
        let ids: Vec<i64> = serde_json::from_str(&t.6).unwrap_or_default();
        ((t.0, t.1, t.2, t.3, t.4, t.5), ids)
    });
    let task_row = match task_row {
        Ok((row, ids)) => {
            task_record_ids = ids;
            Ok(row)
        }
        Err(err) => Err(err),
    };

    let (task_id, task_room_id, date_label, status, clip_count, error_message) = match task_row {
        Ok(value) => value,
        Err(_) => {
            let empty = LiveClipListResponse {
                task_id: 0,
                room_id: room_id.clone(),
                date_label: "".to_string(),
                status: "NONE".to_string(),
                clip_count: 0,
                error_message: None,
                items: Vec::new(),
            };
            return ApiResponse::success(empty);
        }
    };

    let items_result = state.db.with_conn(|conn| {
        let mut stmt = conn.prepare(&format!(
            "SELECT {} FROM live_clip_item WHERE task_id = ?1 ORDER BY start_offset ASC, id ASC",
            CLIP_ITEM_COLUMNS
        ))?;
        let rows = stmt.query_map([task_id], map_clip_item)?;
        rows.collect::<Result<Vec<_>, _>>()
    });

    let items = match items_result {
        Ok(value) => value,
        Err(err) => return ApiResponse::error(format!("读取切片记录失败: {}", err)),
    };

    let mut items = items;
    if items
        .iter()
        .any(|item| item.source_file_path.as_deref().unwrap_or("").is_empty())
    {
        if let Ok(segments) = load_record_segments(&state.db, &task_room_id, &task_record_ids) {
            let segments = normalize_segments_for_analysis(segments, None);
            if !segments.is_empty() {
                for item in &mut items {
                    if !item.source_file_path.as_deref().unwrap_or("").is_empty() {
                        continue;
                    }
                    if let Some((segment, _, _)) =
                        locate_segment_for_offset(&segments, item.start_offset.floor() as i64)
                    {
                        item.source_file_path = Some(segment.file_path.clone());
                        let _ = state.db.with_conn_mut(|conn| {
                            conn.execute(
                                "UPDATE live_clip_item SET source_file_path = ?1 WHERE id = ?2",
                                (segment.file_path.as_str(), item.id),
                            )?;
                            Ok(())
                        });
                    }
                }
            }
        }
    }
    for item in &mut items {
        if item.status.trim().is_empty() {
            item.status = "SUCCESS".to_string();
            let _ = state.db.with_conn_mut(|conn| {
                conn.execute(
                    "UPDATE live_clip_item SET status = 'SUCCESS' WHERE id = ?1",
                    [item.id],
                )?;
                Ok(())
            });
        }
    }

    ApiResponse::success(LiveClipListResponse {
        task_id,
        room_id: task_room_id,
        date_label,
        status,
        clip_count,
        error_message,
        items,
    })
}

#[tauri::command]
pub async fn anchor_check(state: State<'_, AppState>) -> Result<ApiResponse<Vec<Anchor>>, String> {
    let settings =
        load_live_settings_from_db(&state.db).unwrap_or_else(|_| default_live_settings());
    let context = LiveContext {
        db: state.db.clone(),
        bilibili: state.bilibili.clone(),
        login_store: state.login_store.clone(),
        app_log_path: state.app_log_path.clone(),
        live_runtime: state.live_runtime.clone(),
    };
    let global = global_auto_clip_enabled(state.db.as_ref());
    let anchors = match state.db.with_conn(|conn| {
    let mut stmt = conn.prepare(
      "SELECT a.id, a.uid, a.nickname, a.live_status, a.last_check_time, a.create_time, a.update_time, IFNULL(l.auto_record, 1), IFNULL(l.baidu_sync_enabled, 0), l.baidu_sync_path, IFNULL(l.record_mode, 'DAILY'), l.record_dates, IFNULL(l.keep_record_after, 0), l.auto_clip \
       FROM anchor a LEFT JOIN live_room_settings l ON a.uid = l.room_id ORDER BY a.id DESC",
    )?;
    let list = stmt
      .query_map([], |row| {
        let sync_enabled: i64 = row.get(8)?;
        Ok(Anchor {
          id: row.get(0)?,
          uid: row.get(1)?,
          nickname: row.get(2)?,
          live_status: row.get(3)?,
          last_check_time: row.get(4)?,
          create_time: row.get(5)?,
          update_time: row.get(6)?,
          avatar_url: None,
          live_title: None,
          category: None,
          auto_record: row.get::<_, i64>(7)? != 0,
          baidu_sync_enabled: sync_enabled != 0,
          baidu_sync_path: row.get(9)?,
          record_mode: row.get(10)?,
          record_dates: crate::live_recorder::parse_record_dates_public(row.get(11)?),
          keep_record_after: row.get::<_, i64>(12)? != 0,
          auto_clip: row.get::<_, Option<i64>>(13)?.map(|v| v != 0).unwrap_or(global),
          recording_status: None,
          recording_file: None,
          recording_start_time: None,
        })
      })?
      .collect::<Result<Vec<_>, _>>()?;
    Ok(list)
  }) {
    Ok(list) => list,
    Err(err) => return Ok(ApiResponse::error(format!("Failed to read anchors: {}", err))),
  };

    let now = now_rfc3339();
    let mut updated = Vec::new();
    for anchor in anchors {
        let info = match fetch_live_info(&state, &anchor.uid).await {
            Ok(value) => value,
            Err(_) => AnchorLiveInfo {
                nickname: anchor.nickname.clone(),
                live_status: anchor.live_status,
                avatar_url: None,
                live_title: None,
                category: None,
            },
        };

        let _ = state.db.with_conn(|conn| {
      conn.execute(
        "UPDATE anchor SET nickname = ?1, live_status = ?2, last_check_time = ?3, update_time = ?4 WHERE id = ?5",
        (
          info.nickname.as_deref(),
          info.live_status,
          &now,
          &now,
          anchor.id,
        ),
      )?;
      Ok(())
    });

        let room_id = anchor.uid.clone();
        let record_info = state.live_runtime.get_record_info(&room_id);
        updated.push(Anchor {
            id: anchor.id,
            uid: room_id.clone(),
            nickname: info.nickname,
            live_status: info.live_status,
            last_check_time: Some(now.clone()),
            create_time: anchor.create_time,
            update_time: now.clone(),
            avatar_url: info.avatar_url,
            live_title: info.live_title,
            category: info.category,
            auto_record: anchor.auto_record,
            baidu_sync_enabled: anchor.baidu_sync_enabled,
            baidu_sync_path: anchor.baidu_sync_path,
            record_mode: anchor.record_mode,
            record_dates: anchor.record_dates,
            keep_record_after: anchor.keep_record_after,
            auto_clip: anchor.auto_clip,
            recording_status: record_info.as_ref().map(|_| "RECORDING".to_string()),
            recording_file: record_info.as_ref().map(|info| info.file_path.clone()),
            recording_start_time: record_info.map(|info| info.start_time),
        });

        if anchor.auto_record
            && info.live_status == 1
            && !state.live_runtime.is_recording(&room_id)
            && crate::live_recorder::should_auto_record_now(
                &state.db,
                &room_id,
                &state.app_log_path,
            )
        {
            if let Ok(room_info) = fetch_room_info(&state.bilibili, &room_id).await {
                if let Err(err) =
                    start_recording(context.clone(), &room_id, room_info, settings.clone())
                {
                    append_log(
                        &state.app_log_path,
                        &format!("auto_record_check_failed room={} err={}", room_id, err),
                    );
                } else {
                    append_log(
                        &state.app_log_path,
                        &format!("auto_record_check_start room={}", room_id),
                    );
                }
            }
        }
    }

    Ok(ApiResponse::success(updated))
}

async fn fetch_live_info(state: &State<'_, AppState>, uid: &str) -> Result<AnchorLiveInfo, String> {
    let params = vec![("room_id".to_string(), uid.to_string())];
    let data = state
        .bilibili
        .get_json(LIVE_ROOM_INFO_URL, &params, None, false)
        .await?;

    let live_status = data
        .get("live_status")
        .and_then(|value| value.as_i64())
        .unwrap_or(0);

    let live_title = data
        .get("title")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string());

    let area_name = data
        .get("area_name")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string());
    let parent_area = data
        .get("parent_area_name")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string());
    let category = match (parent_area, area_name) {
        (Some(parent), Some(child)) => Some(format!("{} / {}", parent, child)),
        (Some(parent), None) => Some(parent),
        (None, Some(child)) => Some(child),
        _ => None,
    };

    let uid_value = data
        .get("uid")
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    let (nickname, avatar_url) = if uid_value > 0 {
        let user_params = vec![("uid".to_string(), uid_value.to_string())];
        let user_data = state
            .bilibili
            .get_json(LIVE_USER_INFO_URL, &user_params, None, false)
            .await?;
        let info = user_data.get("info").cloned().unwrap_or(Value::Null);
        let nickname = info
            .get("uname")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string());
        let avatar_url = info
            .get("face")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string());
        (nickname, avatar_url)
    } else {
        (None, None)
    };

    Ok(AnchorLiveInfo {
        nickname,
        live_status,
        avatar_url,
        live_title,
        category,
    })
}

pub(crate) fn normalize_record_ids(record_ids: &[i64]) -> Vec<i64> {
    let mut ids = record_ids
        .iter()
        .copied()
        .filter(|value| *value > 0)
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids.dedup();
    ids
}

fn parse_rfc3339_utc(value: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|err| format!("时间解析失败: {}", err))
}

pub(crate) fn load_record_segments(
    db: &crate::db::Db,
    room_id: &str,
    record_ids: &[i64],
) -> Result<Vec<LiveRecordSegment>, String> {
    let placeholders = record_ids
        .iter()
        .enumerate()
        .map(|(index, _)| format!("?{}", index + 2))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT id, file_path, start_time, end_time \
     FROM live_record_task WHERE room_id = ?1 AND id IN ({}) ORDER BY start_time ASC",
        placeholders
    );
    let mut values = Vec::with_capacity(record_ids.len() + 1);
    values.push(SqlValue::Text(room_id.to_string()));
    for record_id in record_ids {
        values.push(SqlValue::Integer(*record_id));
    }
    let mut raw_rows: Vec<(i64, String, String, Option<String>)> = db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(sql.as_str())?;
            let rows = stmt.query_map(params_from_iter(values.iter()), |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })?;
            let mut records = Vec::new();
            for row in rows {
                records.push(row?);
            }
            Ok(records)
        })
        .map_err(|err| format!("读取录播记录失败: {}", err))?;

    if raw_rows.is_empty() {
        return Ok(Vec::new());
    }

    let mut parsed = Vec::with_capacity(raw_rows.len());
    for (record_id, file_path, start_time, end_time) in raw_rows.drain(..) {
        let start_time = parse_rfc3339_utc(&start_time)
            .map_err(|err| format!("解析录播开始时间失败: {}", err))?;
        let end_time = match end_time {
            Some(value) if !value.trim().is_empty() => Some(parse_rfc3339_utc(&value)?),
            _ => None,
        };
        let resolved_path = resolve_existing_record_path(&file_path).unwrap_or(file_path);
        parsed.push((record_id, resolved_path, start_time, end_time));
    }
    parsed.sort_by_key(|item| item.2);

    let mut segments = Vec::with_capacity(parsed.len());
    for index in 0..parsed.len() {
        let (record_id, file_path, start_time, end_time) = parsed[index].clone();
        let end_time = if let Some(value) = end_time {
            value
        } else if index + 1 < parsed.len() {
            parsed[index + 1].2
        } else {
            let duration = probe_duration_seconds(&file_path)
                .ok_or_else(|| format!("无法获取录播时长: {}", file_path))?;
            start_time + Duration::seconds(duration)
        };
        if end_time <= start_time {
            return Err(format!("录播结束时间异常: {}", file_path));
        }
        segments.push(LiveRecordSegment {
            record_id,
            file_path,
            start_time,
            end_time,
            offset_start: 0,
            offset_end: 0,
        });
    }

    let base_time = segments
        .iter()
        .map(|item| item.start_time)
        .min()
        .unwrap_or_else(|| segments[0].start_time);

    for segment in &mut segments {
        let start_offset = (segment.start_time - base_time).num_seconds();
        let end_offset = (segment.end_time - base_time).num_seconds();
        segment.offset_start = start_offset.max(0);
        segment.offset_end = end_offset.max(segment.offset_start);
    }

    Ok(segments)
}

fn probe_duration_seconds(file_path: &str) -> Option<i64> {
    let args = vec![
        "-v".to_string(),
        "error".to_string(),
        "-show_entries".to_string(),
        "format=duration".to_string(),
        "-of".to_string(),
        "json".to_string(),
        file_path.to_string(),
    ];
    let data = crate::ffmpeg::run_ffprobe_json(&args).ok()?;
    let duration = data
        .get("format")
        .and_then(|value| value.get("duration"))
        .and_then(|value| value.as_str())
        .and_then(|value| value.parse::<f64>().ok())
        .or_else(|| {
            data.get("format")
                .and_then(|value| value.get("duration"))
                .and_then(|value| value.as_f64())
        })?;
    if duration.is_finite() && duration > 0.0 {
        Some(duration.floor() as i64)
    } else {
        None
    }
}

pub(crate) fn resolve_clips_dir(file_path: &str) -> Result<PathBuf, String> {
    let path = Path::new(file_path);
    let dir = path.parent().unwrap_or(path);
    Ok(dir.join("clips"))
}

pub(crate) fn normalize_segments_for_analysis(
    segments: Vec<LiveRecordSegment>,
    app_log_path: Option<&Path>,
) -> Vec<LiveRecordSegment> {
    let mut filtered = Vec::with_capacity(segments.len());
    for mut segment in segments {
        let resolved = resolve_existing_record_path(&segment.file_path);
        if let Some(path) = resolved {
            segment.file_path = path;
            filtered.push(segment);
        } else if let Some(log_path) = app_log_path {
            append_log(
                log_path,
                &format!(
                    "anchor_clip_record_missing record_id={} path={}",
                    segment.record_id, segment.file_path
                ),
            );
        }
    }

    if filtered.is_empty() {
        return filtered;
    }

    for segment in &mut filtered {
        if let Some(duration) = probe_duration_seconds(&segment.file_path) {
            let computed = (segment.end_time - segment.start_time).num_seconds();
            if computed <= 0 || (duration - computed).abs() > 30 {
                segment.end_time = segment.start_time + Duration::seconds(duration);
            }
        }
    }

    let base_time = filtered
        .iter()
        .map(|item| item.start_time)
        .min()
        .unwrap_or_else(|| filtered[0].start_time);

    for segment in &mut filtered {
        let start_offset = (segment.start_time - base_time).num_seconds();
        let end_offset = (segment.end_time - base_time).num_seconds();
        segment.offset_start = start_offset.max(0);
        segment.offset_end = end_offset.max(segment.offset_start);
    }

    filtered
}

pub(crate) fn resolve_existing_record_path(file_path: &str) -> Option<String> {
    let path = Path::new(file_path);
    if path.exists() {
        return Some(file_path.to_string());
    }
    let parent = path.parent()?;
    let stem = path.file_stem()?.to_string_lossy();
    for ext in ["mp4", "flv", "ts"] {
        let candidate = parent.join(format!("{}.{}", stem, ext));
        if candidate.exists() {
            return Some(candidate.to_string_lossy().to_string());
        }
    }
    if let Some(room_dir) = parent.parent() {
        if let Ok(entries) = std::fs::read_dir(room_dir) {
            for entry in entries.flatten() {
                let entry_path = entry.path();
                if !entry_path.is_dir() {
                    continue;
                }
                for ext in ["mp4", "flv", "ts"] {
                    let candidate = entry_path.join(format!("{}.{}", stem, ext));
                    if candidate.exists() {
                        return Some(candidate.to_string_lossy().to_string());
                    }
                }
            }
        }
    }
    None
}

/// 返回切片起点所在的录播文件，以及起点在该文件内的秒数
fn resolve_source_path_for_clip(
    db: &crate::db::Db,
    task_id: i64,
    room_id: &str,
    start_offset: f64,
) -> Option<(String, f64)> {
    if task_id <= 0 || room_id.trim().is_empty() || start_offset < 0.0 {
        return None;
    }
    let record_ids_json = db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT record_ids FROM live_clip_task WHERE id = ?1",
                [task_id],
                |row| row.get::<_, String>(0),
            )
        })
        .ok()?;
    let record_ids: Vec<i64> = serde_json::from_str(&record_ids_json).unwrap_or_default();
    if record_ids.is_empty() {
        return None;
    }
    let segments = load_record_segments(db, room_id, &record_ids).ok()?;
    let segments = normalize_segments_for_analysis(segments, None);
    let (segment, _, _) = locate_segment_for_offset(&segments, start_offset.floor() as i64)?;
    Some((segment.file_path, start_offset - segment.offset_start as f64))
}

pub(crate) fn locate_segment_for_offset(
    segments: &[LiveRecordSegment],
    offset: i64,
) -> Option<(LiveRecordSegment, i64, i64)> {
    for segment in segments {
        if offset >= segment.offset_start && offset < segment.offset_end {
            let start_in_file = offset - segment.offset_start;
            let max_available = segment.offset_end - offset;
            return Some((segment.clone(), start_in_file, max_available));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_recorder_and_own_file_names() {
        // 录播姬格式（带毫秒）按北京时间，本软件格式按 UTC
        let (room, start, title) = parse_record_file_name("录制-25034104-20261028-124006-263-困了就睡，醒了就播-别过蓝狗生活！").unwrap();
        assert_eq!((room.as_str(), title.as_str()), ("25034104", "困了就睡，醒了就播-别过蓝狗生活！"));
        assert_eq!(start.to_rfc3339(), "2026-10-28T04:40:06.263+00:00");
        let (_, start, title) = parse_record_file_name("录制-25034104-20261024-110010-大唐创业史0.0").unwrap();
        assert_eq!((start.to_rfc3339().as_str(), title.as_str()), ("2026-10-24T11:00:10+00:00", "大唐创业史0.0"));
    }

    /// 回归：全局自动切片开关必须在连接锁外读取。
    /// 早期把它放进 with_conn 闭包里，而 settings::load 内部又会 with_conn，
    /// 同一把 Mutex 重入导致 anchor_list 失败、界面数据全空。
    #[test]
    fn global_auto_clip_is_read_outside_conn_lock() {
        let db = crate::db::Db::new(std::path::PathBuf::from(":memory:")).expect("init db");
        let global = global_auto_clip_enabled(&db);
        let count: i64 = db
            .with_conn(|conn| conn.query_row("SELECT COUNT(*) FROM anchor", [], |r| r.get(0)))
            .expect("锁外读全局值后应能正常查询");
        assert_eq!(count, 0);
        let _ = global;
    }
}
