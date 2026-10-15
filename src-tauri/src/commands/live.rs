use tauri::State;

use crate::api::ApiResponse;
use crate::commands::settings::{default_live_settings, load_live_settings_from_db};
use crate::live_recorder::{fetch_room_info, start_recording, stop_recording, LiveContext};
use crate::utils::{append_log, now_rfc3339};
use crate::AppState;

#[tauri::command]
pub async fn live_record_start(
    state: State<'_, AppState>,
    room_id: String,
) -> Result<ApiResponse<String>, String> {
    let settings =
        load_live_settings_from_db(&state.db).unwrap_or_else(|_| default_live_settings());
    let room_info = fetch_room_info(&state.bilibili, &room_id).await?;
    let context = LiveContext {
        db: state.db.clone(),
        bilibili: state.bilibili.clone(),
        login_store: state.login_store.clone(),
        app_log_path: state.app_log_path.clone(),
        live_runtime: state.live_runtime.clone(),
    };
    match start_recording(context, &room_id, room_info, settings) {
        Ok(()) => Ok(ApiResponse::success("录制已启动".to_string())),
        Err(err) => Ok(ApiResponse::error(err)),
    }
}

#[tauri::command]
pub fn live_record_stop(state: State<'_, AppState>, room_id: String) -> ApiResponse<String> {
    let context = LiveContext {
        db: state.db.clone(),
        bilibili: state.bilibili.clone(),
        login_store: state.login_store.clone(),
        app_log_path: state.app_log_path.clone(),
        live_runtime: state.live_runtime.clone(),
    };
    stop_recording(context, &room_id, "手动停止");
    ApiResponse::success("录制已停止".to_string())
}

#[tauri::command]
pub async fn live_room_auto_record_update(
    state: State<'_, AppState>,
    room_id: String,
    auto_record: bool,
) -> Result<ApiResponse<String>, String> {
    let now = now_rfc3339();
    let result = state.db.with_conn(|conn| {
    conn.execute(
      "INSERT INTO live_room_settings (room_id, auto_record, update_time) VALUES (?1, ?2, ?3) \
       ON CONFLICT(room_id) DO UPDATE SET auto_record = excluded.auto_record, update_time = excluded.update_time",
      (room_id.as_str(), auto_record as i64, &now),
    )?;
    Ok(())
  });
    if let Err(err) = result {
        return Ok(ApiResponse::error(format!(
            "Failed to update auto record: {}",
            err
        )));
    }

    if auto_record {
        let settings =
            load_live_settings_from_db(&state.db).unwrap_or_else(|_| default_live_settings());
        let room_info = fetch_room_info(&state.bilibili, &room_id).await?;
        if room_info.live_status == 1
            && crate::live_recorder::should_auto_record_now(
                &state.db,
                &room_id,
                &state.app_log_path,
            )
        {
            let context = LiveContext {
                db: state.db.clone(),
                bilibili: state.bilibili.clone(),
                login_store: state.login_store.clone(),
                app_log_path: state.app_log_path.clone(),
                live_runtime: state.live_runtime.clone(),
            };
            match start_recording(context, &room_id, room_info, settings) {
                Ok(()) => {
                    append_log(
                        &state.app_log_path,
                        &format!("auto_record_toggle_start room={}", room_id),
                    );
                }
                Err(err) => {
                    append_log(
                        &state.app_log_path,
                        &format!("auto_record_toggle_failed room={} err={}", room_id, err),
                    );
                }
            }
        }
    }

    Ok(ApiResponse::success("已更新".to_string()))
}

#[tauri::command]
pub fn live_room_baidu_sync_update(
    state: State<'_, AppState>,
    room_id: String,
    baidu_sync_path: String,
) -> ApiResponse<String> {
    let now = now_rfc3339();
    let trimmed = baidu_sync_path.trim().to_string();
    let enabled = !trimmed.is_empty();
    let value = if enabled { Some(trimmed) } else { None };
    let result = state.db.with_conn(|conn| {
    conn.execute(
      "INSERT INTO live_room_settings (room_id, auto_record, baidu_sync_enabled, baidu_sync_path, update_time) \
       VALUES (?1, 1, ?2, ?3, ?4) \
       ON CONFLICT(room_id) DO UPDATE SET \
         baidu_sync_enabled = excluded.baidu_sync_enabled, \
         baidu_sync_path = excluded.baidu_sync_path, \
         update_time = excluded.update_time",
      (room_id.as_str(), enabled as i64, value.as_deref(), &now),
    )?;
    Ok(())
  });
    match result {
        Ok(()) => ApiResponse::success("已更新".to_string()),
        Err(err) => ApiResponse::error(format!("Failed to update sync path: {}", err)),
    }
}

#[tauri::command]
pub fn live_room_baidu_sync_toggle(
    state: State<'_, AppState>,
    room_id: String,
    enabled: bool,
) -> ApiResponse<String> {
    let now = now_rfc3339();
    let path = state.db.with_conn(|conn| {
        conn.query_row(
            "SELECT baidu_sync_path FROM live_room_settings WHERE room_id = ?1",
            [room_id.as_str()],
            |row| row.get::<_, Option<String>>(0),
        )
    });
    let current_path = match path {
        Ok(value) => value,
        Err(_) => None,
    };
    if enabled {
        let valid = current_path
            .as_deref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false);
        if !valid {
            return ApiResponse::error("请先配置同步路径".to_string());
        }
    }
    let result = state.db.with_conn(|conn| {
        conn.execute(
      "INSERT INTO live_room_settings (room_id, auto_record, baidu_sync_enabled, update_time) \
       VALUES (?1, 1, ?2, ?3) \
       ON CONFLICT(room_id) DO UPDATE SET \
         baidu_sync_enabled = excluded.baidu_sync_enabled, \
         update_time = excluded.update_time",
      (room_id.as_str(), enabled as i64, &now),
    )?;
        Ok(())
    });
    match result {
        Ok(()) => ApiResponse::success("已更新".to_string()),
        Err(err) => ApiResponse::error(format!("Failed to update sync toggle: {}", err)),
    }
}

#[tauri::command]
pub fn live_room_record_config_update(
    state: State<'_, AppState>,
    room_id: String,
    mode: String,
    dates: Vec<String>,
    keep_record_after: bool,
) -> ApiResponse<String> {
    let room_id = room_id.trim().to_string();
    if room_id.is_empty() {
        return ApiResponse::error("房间号不能为空".to_string());
    }
    let mode = mode.trim().to_ascii_uppercase();
    if mode != "DAILY" && mode != "SELECTED" {
        return ApiResponse::error("录制模式无效".to_string());
    }

    let mut normalized = Vec::new();
    for item in dates {
        let value = item.trim().to_string();
        if value.is_empty() {
            continue;
        }
        if !crate::live_recorder::is_valid_record_date(&value) {
            return ApiResponse::error(format!("日期格式无效: {}", value));
        }
        normalized.push(value);
    }
    normalized.sort();
    normalized.dedup();

    if mode == "SELECTED" && normalized.is_empty() {
        return ApiResponse::error("「仅指定日期」至少需要选择 1 天".to_string());
    }

    let dates_json = serde_json::to_string(&normalized).unwrap_or_else(|_| "[]".to_string());
    let keep = if mode == "SELECTED" && keep_record_after {
        1_i64
    } else {
        0_i64
    };
    let now = now_rfc3339();
    let result = state.db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO live_room_settings (room_id, auto_record, record_mode, record_dates, keep_record_after, update_time) \
             VALUES (?1, 1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(room_id) DO UPDATE SET \
               record_mode = excluded.record_mode, \
               record_dates = excluded.record_dates, \
               keep_record_after = excluded.keep_record_after, \
               update_time = excluded.update_time",
            (room_id.as_str(), mode.as_str(), dates_json.as_str(), keep, &now),
        )?;
        Ok(())
    });
    match result {
        Ok(()) => ApiResponse::success("已更新".to_string()),
        Err(err) => ApiResponse::error(format!("Failed to update record config: {}", err)),
    }
}
