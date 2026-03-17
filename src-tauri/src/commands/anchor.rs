use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use rusqlite::params_from_iter;
use rusqlite::types::Value as SqlValue;
use tauri::State;

use crate::api::ApiResponse;
use crate::commands::settings::{default_live_settings, load_live_settings_from_db};
use crate::ffmpeg::run_ffmpeg;
use crate::live_recorder::{fetch_room_info, start_recording, stop_recording, LiveContext};
use crate::utils::{append_log, now_rfc3339, sanitize_filename};
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
  pub recording_status: Option<String>,
  pub recording_file: Option<String>,
  pub recording_start_time: Option<String>,
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
  let settings = load_live_settings_from_db(&state.db).unwrap_or_else(|_| default_live_settings());
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
        if !state.live_runtime.is_recording(&uid) {
          if let Err(err) = start_recording(context.clone(), &uid, room_info, settings.clone()) {
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

#[tauri::command]
pub fn anchor_list(state: State<'_, AppState>) -> ApiResponse<Vec<Anchor>> {
  match state.db.with_conn(|conn| {
    let mut stmt = conn.prepare(
      "SELECT a.id, a.uid, a.nickname, a.live_status, a.last_check_time, a.create_time, a.update_time, IFNULL(l.auto_record, 1), IFNULL(l.baidu_sync_enabled, 0), l.baidu_sync_path \
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

#[tauri::command]
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
    conn.execute("DELETE FROM live_room_settings WHERE room_id = ?1", [uid_value.as_str()])?;
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
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveClipItem {
  pub id: i64,
  pub source_file_path: Option<String>,
  pub status: String,
  pub file_path: String,
  pub start_offset: i64,
  pub end_offset: i64,
  pub duration: i64,
  pub peak_count: i64,
  pub create_time: String,
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
struct LiveRecordSegment {
  record_id: i64,
  file_path: String,
  start_time: DateTime<Utc>,
  end_time: DateTime<Utc>,
  offset_start: i64,
  offset_end: i64,
}

#[derive(Clone)]
struct ClipInterval {
  start_offset: i64,
  end_offset: i64,
  peak_count: i64,
}

#[tauri::command]
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

#[tauri::command]
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

#[tauri::command]
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

#[tauri::command]
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

#[tauri::command]
pub fn anchor_open_record_file(file_path: String) -> ApiResponse<String> {
  let path = Path::new(file_path.trim());
  match tauri_plugin_opener::open_path(path, None::<&str>)
    .map_err(|err| format!("打开文件失败: {}", err))
  {
    Ok(()) => ApiResponse::success("已打开".to_string()),
    Err(err) => ApiResponse::error(err),
  }
}

#[tauri::command]
pub async fn anchor_analyze_clips(
  state: State<'_, AppState>,
  room_id: String,
  record_ids: Vec<i64>,
) -> Result<ApiResponse<i64>, String> {
  let room_id = room_id.trim().to_string();
  if room_id.is_empty() {
    return Ok(ApiResponse::error("room_id 不能为空"));
  }
  if record_ids.is_empty() {
    return Ok(ApiResponse::error("record_ids 不能为空"));
  }
  let normalized_ids = normalize_record_ids(&record_ids);
  if normalized_ids.is_empty() {
    return Ok(ApiResponse::error("record_ids 无效"));
  }

  let segments = match load_record_segments(&state.db, &room_id, &normalized_ids) {
    Ok(value) => value,
    Err(err) => return Ok(ApiResponse::error(err)),
  };
  let segments = normalize_segments_for_analysis(segments, Some(state.app_log_path.as_path()));
  if segments.is_empty() {
    return Ok(ApiResponse::error("未找到录播记录"));
  }
  let date_label = segments[0].start_time.format("%Y-%m-%d").to_string();
  let record_ids_json = serde_json::to_string(&normalized_ids).unwrap_or_else(|_| "[]".to_string());
  let now = now_rfc3339();
  let task_id = match state.db.with_conn_mut(|conn| {
    conn.execute(
      "INSERT INTO live_clip_task (room_id, record_ids, date_label, status, clip_count, error_message, create_time, update_time) \
       VALUES (?1, ?2, ?3, ?4, 0, NULL, ?5, ?6)",
      (&room_id, &record_ids_json, &date_label, "RUNNING", &now, &now),
    )?;
    Ok(conn.last_insert_rowid())
  }) {
    Ok(value) => value,
    Err(err) => return Ok(ApiResponse::error(format!("创建切片任务失败: {}", err))),
  };

  let db = state.db.clone();
  let app_log_path = state.app_log_path.clone();
  let room_id_clone = room_id.clone();
  let record_ids_clone = normalized_ids.clone();
  tauri::async_runtime::spawn_blocking(move || {
    if let Err(err) = run_clip_analysis(
      &db,
      app_log_path.as_path(),
      &room_id_clone,
      &record_ids_clone,
      task_id,
    ) {
      let now = now_rfc3339();
      let _ = db.with_conn_mut(|conn| {
        conn.execute(
          "UPDATE live_clip_task SET status = ?1, error_message = ?2, update_time = ?3 WHERE id = ?4",
          ("FAILED", err.as_str(), &now, task_id),
        )?;
        Ok(())
      });
      append_log(
        app_log_path.as_path(),
        &format!("anchor_clip_task_failed task_id={} err={}", task_id, err),
      );
    }
  });

  Ok(ApiResponse::success(task_id))
}

#[tauri::command]
pub fn anchor_clip_task_status(
  state: State<'_, AppState>,
  task_id: i64,
) -> ApiResponse<LiveClipTaskStatus> {
  if task_id <= 0 {
    return ApiResponse::error("task_id 无效");
  }
  let result = state.db.with_conn(|conn| {
    conn.query_row(
      "SELECT status, clip_count, error_message FROM live_clip_task WHERE id = ?1",
      [task_id],
      |row| {
        Ok(LiveClipTaskStatus {
          status: row.get(0)?,
          clip_count: row.get::<_, Option<i64>>(1)?.unwrap_or(0),
          error_message: row.get(2)?,
        })
      },
    )
  });
  match result {
    Ok(value) => ApiResponse::success(value),
    Err(err) => ApiResponse::error(format!("读取切片任务失败: {}", err)),
  }
}

#[tauri::command]
pub fn anchor_clip_update_time(
  state: State<'_, AppState>,
  clip_id: i64,
  start_offset: i64,
  end_offset: i64,
) -> ApiResponse<LiveClipItem> {
  if clip_id <= 0 {
    return ApiResponse::error("clip_id 无效");
  }
  if start_offset < 0 || end_offset <= start_offset {
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
      "SELECT id, source_file_path, status, file_path, start_offset, end_offset, duration, peak_count, create_time \
       FROM live_clip_item WHERE id = ?1",
      [clip_id],
      |row| {
        Ok(LiveClipItem {
          id: row.get(0)?,
          source_file_path: row.get(1)?,
          status: row.get(2)?,
          file_path: row.get(3)?,
          start_offset: row.get(4)?,
          end_offset: row.get(5)?,
          duration: row.get(6)?,
          peak_count: row.get::<_, Option<i64>>(7)?.unwrap_or(0),
          create_time: row.get(8)?,
        })
      },
    )
  });
  match item_result {
    Ok(item) => ApiResponse::success(item),
    Err(err) => ApiResponse::error(format!("读取切片记录失败: {}", err)),
  }
}

#[tauri::command]
pub fn anchor_clip_reclip(state: State<'_, AppState>, clip_id: i64) -> ApiResponse<String> {
  if clip_id <= 0 {
    return ApiResponse::error("clip_id 无效");
  }
  let record = state.db.with_conn(|conn| {
    conn.query_row(
      "SELECT source_file_path, file_path, start_offset, end_offset, task_id, room_id \
       FROM live_clip_item WHERE id = ?1",
      [clip_id],
      |row| {
        Ok((
          row.get::<_, Option<String>>(0)?,
          row.get::<_, String>(1)?,
          row.get::<_, i64>(2)?,
          row.get::<_, i64>(3)?,
          row.get::<_, i64>(4)?,
          row.get::<_, String>(5)?,
        ))
      },
    )
  });

  let (source_path, clip_path, start_offset, end_offset, task_id, room_id) = match record {
    Ok(value) => value,
    Err(err) => return ApiResponse::error(format!("读取切片记录失败: {}", err)),
  };
  let mut resolved_source = source_path.and_then(|value| {
    if value.trim().is_empty() {
      None
    } else {
      Some(value)
    }
  });
  if resolved_source.is_none() {
    resolved_source = resolve_source_path_for_clip(&state.db, task_id, &room_id, start_offset);
    if let Some(path) = resolved_source.as_ref() {
      let _ = state.db.with_conn_mut(|conn| {
        conn.execute(
          "UPDATE live_clip_item SET source_file_path = ?1 WHERE id = ?2",
          (path, clip_id),
        )?;
        Ok(())
      });
    }
  }
  let Some(source_path) = resolved_source else {
    return ApiResponse::error("源视频路径为空，无法重新剪辑");
  };
  let source_path = resolve_existing_record_path(&source_path)
    .unwrap_or_else(|| source_path.clone());
  if !Path::new(&source_path).exists() {
    return ApiResponse::error(format!("源视频不存在: {}", source_path));
  }
  if start_offset < 0 || end_offset <= start_offset {
    return ApiResponse::error("时间段无效");
  }
  let duration = end_offset - start_offset;
  let _ = state.db.with_conn_mut(|conn| {
    conn.execute(
      "UPDATE live_clip_item SET status = 'RUNNING' WHERE id = ?1",
      [clip_id],
    )?;
    Ok(())
  });
  let db = state.db.clone();
  let clip_id_copy = clip_id;
  let clip_path_copy = clip_path.clone();
  let source_path_copy = source_path.clone();
  std::thread::spawn(move || {
    let args = vec![
      "-y".to_string(),
      "-ss".to_string(),
      start_offset.to_string(),
      "-t".to_string(),
      duration.to_string(),
      "-accurate_seek".to_string(),
      "-i".to_string(),
      source_path_copy,
      "-c".to_string(),
      "copy".to_string(),
      "-avoid_negative_ts".to_string(),
      "1".to_string(),
      clip_path_copy.clone(),
    ];
    match run_ffmpeg(&args) {
      Ok(_) => {
        let actual_duration = probe_duration_seconds(&clip_path_copy).unwrap_or(duration);
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

#[tauri::command]
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

#[tauri::command]
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
  let record_ids_json = serde_json::to_string(&normalized_ids).unwrap_or_else(|_| "[]".to_string());

  let task_row = state.db.with_conn(|conn| {
    conn.query_row(
      "SELECT id, room_id, date_label, status, clip_count, error_message \
       FROM live_clip_task WHERE room_id = ?1 AND record_ids = ?2 ORDER BY id DESC LIMIT 1",
      (&room_id, &record_ids_json),
      |row| {
        Ok((
          row.get::<_, i64>(0)?,
          row.get::<_, String>(1)?,
          row.get::<_, String>(2)?,
          row.get::<_, String>(3)?,
          row.get::<_, Option<i64>>(4)?.unwrap_or(0),
          row.get::<_, Option<String>>(5)?,
        ))
      },
    )
  });

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
    let mut stmt = conn.prepare(
      "SELECT id, source_file_path, status, file_path, start_offset, end_offset, duration, peak_count, create_time \
       FROM live_clip_item WHERE task_id = ?1 ORDER BY id ASC",
    )?;
    let rows = stmt.query_map([task_id], |row| {
      Ok(LiveClipItem {
        id: row.get(0)?,
        source_file_path: row.get(1)?,
        status: row.get(2)?,
        file_path: row.get(3)?,
        start_offset: row.get(4)?,
        end_offset: row.get(5)?,
        duration: row.get(6)?,
        peak_count: row.get::<_, Option<i64>>(7)?.unwrap_or(0),
        create_time: row.get(8)?,
      })
    })?;
    let mut items = Vec::new();
    for row in rows {
      items.push(row?);
    }
    Ok(items)
  });

  let items = match items_result {
    Ok(value) => value,
    Err(err) => return ApiResponse::error(format!("读取切片记录失败: {}", err)),
  };

  let mut items = items;
  if items.iter().any(|item| item.source_file_path.as_deref().unwrap_or("").is_empty()) {
    if let Ok(segments) = load_record_segments(&state.db, &task_room_id, &normalized_ids) {
      let segments = normalize_segments_for_analysis(segments, None);
      if !segments.is_empty() {
        for item in &mut items {
          if !item.source_file_path.as_deref().unwrap_or("").is_empty() {
            continue;
          }
          if let Some((segment, _, _)) = locate_segment_for_offset(&segments, item.start_offset) {
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
  let settings = load_live_settings_from_db(&state.db).unwrap_or_else(|_| default_live_settings());
  let context = LiveContext {
    db: state.db.clone(),
    bilibili: state.bilibili.clone(),
    login_store: state.login_store.clone(),
    app_log_path: state.app_log_path.clone(),
    live_runtime: state.live_runtime.clone(),
  };
  let anchors = match state.db.with_conn(|conn| {
    let mut stmt = conn.prepare(
      "SELECT a.id, a.uid, a.nickname, a.live_status, a.last_check_time, a.create_time, a.update_time, IFNULL(l.auto_record, 1), IFNULL(l.baidu_sync_enabled, 0), l.baidu_sync_path \
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
      recording_status: record_info.as_ref().map(|_| "RECORDING".to_string()),
      recording_file: record_info.as_ref().map(|info| info.file_path.clone()),
      recording_start_time: record_info.map(|info| info.start_time),
    });

    if anchor.auto_record && info.live_status == 1 && !state.live_runtime.is_recording(&room_id) {
      if let Ok(room_info) = fetch_room_info(&state.bilibili, &room_id).await {
        if let Err(err) = start_recording(context.clone(), &room_id, room_info, settings.clone()) {
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


async fn fetch_live_info(
  state: &State<'_, AppState>,
  uid: &str,
) -> Result<AnchorLiveInfo, String> {
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

fn normalize_record_ids(record_ids: &[i64]) -> Vec<i64> {
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

fn load_record_segments(
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
      data
        .get("format")
        .and_then(|value| value.get("duration"))
        .and_then(|value| value.as_f64())
    })?;
  if duration.is_finite() && duration > 0.0 {
    Some(duration.floor() as i64)
  } else {
    None
  }
}

fn run_clip_analysis(
  db: &crate::db::Db,
  app_log_path: &Path,
  room_id: &str,
  record_ids: &[i64],
  task_id: i64,
) -> Result<(), String> {
  let segments = load_record_segments(db, room_id, record_ids)?;
  let segments = normalize_segments_for_analysis(segments, Some(app_log_path));
  if segments.is_empty() {
    return Err("未找到录播记录".to_string());
  }

  let base_time = segments
    .iter()
    .map(|item| item.start_time)
    .min()
    .unwrap_or_else(|| segments[0].start_time);

  let mut buckets = std::collections::BTreeMap::<i64, i64>::new();
  let mut timestamps = Vec::<i64>::new();
  for segment in &segments {
    let danmaku_jsonl_path = Path::new(&segment.file_path).with_extension("danmaku.jsonl");
    let danmaku_xml_path = Path::new(&segment.file_path).with_extension("xml");
    let mut loaded = false;

    if danmaku_jsonl_path.exists() {
      let file = File::open(&danmaku_jsonl_path).map_err(|err| {
        format!(
          "读取弹幕失败: {} {}",
          danmaku_jsonl_path.to_string_lossy(),
          err
        )
      })?;
      let reader = BufReader::new(file);
      for line in reader.lines().flatten() {
        let value: Value = match serde_json::from_str(&line) {
          Ok(value) => value,
          Err(_) => continue,
        };
        let cmd = value.get("cmd").and_then(|v| v.as_str()).unwrap_or("");
        if cmd != "DANMU_MSG" && cmd != "SUPER_CHAT_MESSAGE" {
          continue;
        }
        let timestamp = value.get("timestamp").and_then(|v| v.as_str());
        let Some(timestamp) = timestamp else {
          continue;
        };
        let Ok(dt) = DateTime::parse_from_rfc3339(timestamp) else {
          continue;
        };
        let offset = (dt.with_timezone(&Utc) - base_time).num_seconds();
        push_danmaku_offset(offset, &mut buckets, &mut timestamps);
      }
      loaded = true;
    } else if danmaku_xml_path.exists() {
      collect_danmaku_xml_offsets(
        &danmaku_xml_path,
        segment.offset_start,
        &mut buckets,
        &mut timestamps,
      )?;
      loaded = true;
    }

    if !loaded {
      append_log(
        app_log_path,
        &format!(
          "anchor_clip_danmaku_missing record_id={} path={}",
          segment.record_id,
          danmaku_jsonl_path.to_string_lossy()
        ),
      );
    }
  }

  let mut intervals = Vec::new();
  let mut threshold_intervals = compute_clip_intervals(&buckets);
  if !threshold_intervals.is_empty() {
    intervals.append(&mut threshold_intervals);
  }
  let mut sliding_intervals = compute_sliding_intervals(&timestamps, 60, 3, 30, 1);
  if !sliding_intervals.is_empty() {
    intervals.append(&mut sliding_intervals);
  }
  if !intervals.is_empty() {
    intervals = merge_overlapping_intervals(intervals);
  }
  if intervals.is_empty() {
    let now = now_rfc3339();
    let _ = db.with_conn_mut(|conn| {
      conn.execute(
        "UPDATE live_clip_task SET status = ?1, clip_count = 0, update_time = ?2 WHERE id = ?3",
        ("COMPLETED", &now, task_id),
      )?;
      Ok(())
    });
    return Ok(());
  }

  let clips_dir = resolve_clips_dir(&segments[0].file_path)?;
  fs::create_dir_all(&clips_dir)
    .map_err(|err| format!("创建切片目录失败: {} {}", clips_dir.to_string_lossy(), err))?;
  cleanup_old_clip_files(&clips_dir, app_log_path)?;

  let mut clip_count = 0;
  for (index, interval) in intervals.iter().enumerate() {
    let Some((segment, start_in_file, max_available)) =
      locate_segment_for_offset(&segments, interval.start_offset)
    else {
      append_log(
        app_log_path,
        &format!(
          "anchor_clip_skip_no_segment task_id={} offset={}",
          task_id, interval.start_offset
        ),
      );
      continue;
    };
    let mut end_offset = interval.end_offset;
    let max_end = interval.start_offset + max_available;
    if end_offset > max_end {
      end_offset = max_end;
    }
    let duration = end_offset - interval.start_offset;
    if duration < 60 {
      continue;
    }

    let file_name = build_clip_file_name(index + 1, interval.start_offset, end_offset);
    let output_path = clips_dir.join(file_name);
    let input_path = resolve_existing_record_path(&segment.file_path)
      .unwrap_or_else(|| segment.file_path.clone());
    let args = vec![
      "-y".to_string(),
      "-ss".to_string(),
      start_in_file.to_string(),
      "-t".to_string(),
      duration.to_string(),
      "-accurate_seek".to_string(),
      "-i".to_string(),
      input_path.clone(),
      "-c".to_string(),
      "copy".to_string(),
      "-avoid_negative_ts".to_string(),
      "1".to_string(),
      output_path.to_string_lossy().to_string(),
    ];
    run_ffmpeg(&args)
      .map_err(|err| format!("剪辑失败: {} {}", output_path.to_string_lossy(), err))?;

    let now = now_rfc3339();
    db.with_conn_mut(|conn| {
      conn.execute(
        "INSERT INTO live_clip_item (task_id, room_id, source_file_path, file_path, start_offset, end_offset, duration, peak_count, create_time) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        (
          task_id,
          room_id,
          input_path.clone(),
          output_path.to_string_lossy(),
          interval.start_offset,
          end_offset,
          duration,
          interval.peak_count,
          &now,
        ),
      )?;
      Ok(())
    })
    .map_err(|err| format!("写入切片记录失败: {}", err))?;
    clip_count += 1;
  }

  let now = now_rfc3339();
  db.with_conn_mut(|conn| {
    conn.execute(
      "UPDATE live_clip_task SET status = ?1, clip_count = ?2, update_time = ?3 WHERE id = ?4",
      ("COMPLETED", clip_count, &now, task_id),
    )?;
    Ok(())
  })
  .map_err(|err| format!("更新切片任务失败: {}", err))?;

  Ok(())
}

fn resolve_clips_dir(file_path: &str) -> Result<PathBuf, String> {
  let path = Path::new(file_path);
  let dir = path.parent().unwrap_or(path);
  Ok(dir.join("clips"))
}

fn compute_clip_intervals(buckets: &std::collections::BTreeMap<i64, i64>) -> Vec<ClipInterval> {
  let up_ratio = 2.5;
  let down_ratio = 0.75;
  let mut intervals = Vec::new();
  let mut active: Option<(i64, i64, i64)> = None;
  let mut prev_count: Option<i64> = None;
  let mut last_bucket_start = None;

  for (bucket_start, count) in buckets.iter() {
    let count = *count;
    last_bucket_start = Some(*bucket_start);
    match active {
      Some((start, peak, last_count)) => {
        let next_peak = peak.max(count);
        if last_count > 0 && (count as f64) <= (last_count as f64) * down_ratio {
          intervals.push(ClipInterval {
            start_offset: start,
            end_offset: bucket_start + 60,
            peak_count: next_peak,
          });
          active = None;
        } else {
          active = Some((start, next_peak, count));
        }
      }
      None => {
        if let Some(prev) = prev_count {
          let should_start = if prev == 0 {
            count > 0
          } else {
            (count as f64) >= (prev as f64) * up_ratio
          };
          if should_start {
            active = Some((*bucket_start, count, count));
          }
        }
      }
    }
    prev_count = Some(count);
  }

  if let Some((start, peak, _)) = active {
    if let Some(last_bucket_start) = last_bucket_start {
      intervals.push(ClipInterval {
        start_offset: start,
        end_offset: last_bucket_start + 60,
        peak_count: peak,
      });
    }
  }

  intervals
    .into_iter()
    .map(|interval| {
      let start = (interval.start_offset - 30).max(0);
      let end = interval.end_offset + 30;
      ClipInterval {
        start_offset: start,
        end_offset: end,
        peak_count: interval.peak_count,
      }
    })
    .filter(|interval| interval.end_offset - interval.start_offset >= 60)
    .collect()
}

fn push_danmaku_offset(
  offset: i64,
  buckets: &mut std::collections::BTreeMap<i64, i64>,
  timestamps: &mut Vec<i64>,
) {
  if offset < 0 {
    return;
  }
  timestamps.push(offset);
  let bucket = (offset / 60) * 60;
  *buckets.entry(bucket).or_insert(0) += 1;
}

fn normalize_segments_for_analysis(
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

fn resolve_existing_record_path(file_path: &str) -> Option<String> {
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

fn resolve_source_path_for_clip(
  db: &crate::db::Db,
  task_id: i64,
  room_id: &str,
  start_offset: i64,
) -> Option<String> {
  if task_id <= 0 || room_id.trim().is_empty() || start_offset < 0 {
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
  let (segment, _, _) = locate_segment_for_offset(&segments, start_offset)?;
  Some(segment.file_path)
}

fn cleanup_old_clip_files(clips_dir: &Path, app_log_path: &Path) -> Result<(), String> {
  if !clips_dir.exists() {
    return Ok(());
  }
  let entries = std::fs::read_dir(clips_dir)
    .map_err(|err| format!("读取切片目录失败: {} {}", clips_dir.to_string_lossy(), err))?;
  for entry in entries {
    let entry = entry.map_err(|err| format!("读取切片目录失败: {}", err))?;
    let path = entry.path();
    if !path.is_file() {
      continue;
    }
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
      continue;
    };
    if !name.starts_with("clip_") || !name.ends_with(".mp4") {
      continue;
    }
    if let Err(err) = std::fs::remove_file(&path) {
      append_log(
        app_log_path,
        &format!(
          "anchor_clip_cleanup_failed path={} err={}",
          path.to_string_lossy(),
          err
        ),
      );
      return Err(format!("清理切片文件失败: {} {}", path.to_string_lossy(), err));
    }
  }
  Ok(())
}

fn collect_danmaku_xml_offsets(
  path: &Path,
  base_offset: i64,
  buckets: &mut std::collections::BTreeMap<i64, i64>,
  timestamps: &mut Vec<i64>,
) -> Result<(), String> {
  let file = File::open(path)
    .map_err(|err| format!("读取弹幕失败: {} {}", path.to_string_lossy(), err))?;
  let reader = BufReader::new(file);
  for line in reader.lines().flatten() {
    let mut remain = line.as_str();
    while let Some(p_index) = remain.find("p=\"") {
      let after_p = &remain[(p_index + 3)..];
      let Some(end_index) = after_p.find('"') else {
        break;
      };
      let p_value = &after_p[..end_index];
      if let Some(first) = p_value.split(',').next() {
        if let Ok(value) = first.parse::<f64>() {
          let offset = base_offset + value.floor() as i64;
          push_danmaku_offset(offset, buckets, timestamps);
        }
      }
      remain = &after_p[(end_index + 1)..];
    }
  }
  Ok(())
}

fn compute_sliding_intervals(
  timestamps: &[i64],
  window_size: i64,
  top_n: usize,
  max_overlap: i64,
  step: usize,
) -> Vec<ClipInterval> {
  if timestamps.is_empty() || window_size <= 0 || top_n == 0 || step == 0 {
    return Vec::new();
  }
  let mut time_counts = std::collections::BTreeMap::<i64, i64>::new();
  for &ts in timestamps {
    if ts >= 0 {
      *time_counts.entry(ts).or_insert(0) += 1;
    }
  }
  if time_counts.is_empty() {
    return Vec::new();
  }
  let mut times = time_counts.keys().copied().collect::<Vec<_>>();
  times.sort_unstable();

  let mut density_periods: Vec<(i64, i64)> = Vec::new();
  for i in (0..times.len()).step_by(step) {
    let start_time = times[i];
    let end_time = start_time + window_size;
    let mut current_density = 0;
    for (&time, &count) in time_counts.iter() {
      if time >= start_time && time < end_time {
        current_density += count;
      }
      if time >= end_time {
        break;
      }
    }
    density_periods.push((start_time, current_density));
  }
  density_periods.sort_by(|a, b| b.1.cmp(&a.1));

  let mut filtered: Vec<(i64, i64)> = Vec::new();
  for (start_time, density) in density_periods {
    let mut valid = true;
    for (selected_start, _) in &filtered {
      let overlap = (selected_start + window_size).min(start_time + window_size)
        - (*selected_start).max(start_time);
      if overlap > max_overlap {
        valid = false;
        break;
      }
    }
    if valid {
      filtered.push((start_time, density));
      if filtered.len() == top_n {
        break;
      }
    }
  }

  filtered
    .into_iter()
    .map(|(start_time, density): (i64, i64)| {
      let start = (start_time - 30).max(0);
      let end = start_time + window_size + 30;
      ClipInterval {
        start_offset: start,
        end_offset: end.max(start),
        peak_count: density,
      }
    })
    .filter(|interval| interval.end_offset - interval.start_offset >= 60)
    .collect()
}

fn merge_overlapping_intervals(mut intervals: Vec<ClipInterval>) -> Vec<ClipInterval> {
  if intervals.is_empty() {
    return intervals;
  }
  intervals.sort_by_key(|interval| interval.start_offset);
  let mut merged = Vec::new();
  let mut current = intervals[0].clone();
  for interval in intervals.into_iter().skip(1) {
    if interval.start_offset <= current.end_offset {
      if interval.end_offset > current.end_offset {
        current.end_offset = interval.end_offset;
      }
      if interval.peak_count > current.peak_count {
        current.peak_count = interval.peak_count;
      }
    } else {
      merged.push(current);
      current = interval;
    }
  }
  merged.push(current);
  merged
}

fn locate_segment_for_offset(
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

fn build_clip_file_name(index: usize, start_offset: i64, end_offset: i64) -> String {
  let start = format_hms(start_offset);
  let end = format_hms(end_offset);
  let raw = format!("clip_{:03}_{}-{}", index, start, end);
  format!("{}.mp4", sanitize_filename(&raw))
}

fn format_hms(seconds: i64) -> String {
  let total = seconds.max(0);
  let hours = total / 3600;
  let minutes = (total % 3600) / 60;
  let secs = total % 60;
  format!("{:02}_{:02}_{:02}", hours, minutes, secs)
}
