//! 设置页「自动切片」tab 与主播内容类型相关命令。

use serde::Serialize;
use tauri::{Manager, State};

use crate::api::ApiResponse;
use crate::highlight::asr::{self, DownloadProgress, ModelFileStatus};
use crate::highlight::llm::{self, TestResult};
use crate::highlight::settings::{self, AutoClipSettings, LlmSettings};
use crate::highlight::{self as hl};
use crate::utils::now_rfc3339;
use crate::AppState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AsrModelState {
    pub model_dir: String,
    pub default_model_dir: String,
    pub models: Vec<ModelFileStatus>,
    pub download: DownloadProgress,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoClipSettingsView {
    pub settings: AutoClipSettings,
    pub asr: AsrModelState,
}

fn asr_state(app: &tauri::AppHandle, cfg: &AutoClipSettings) -> AsrModelState {
    let data_dir = app.path().app_data_dir().unwrap_or_default();
    let dir = hl::model_dir(&data_dir, &cfg.asr);
    AsrModelState {
        model_dir: dir.to_string_lossy().to_string(),
        default_model_dir: hl::model_dir(&data_dir, &Default::default()).to_string_lossy().to_string(),
        models: asr::model_status(&dir),
        download: asr::download_progress(),
    }
}

#[tauri::command(async)]
pub fn auto_clip_settings_get(state: State<'_, AppState>, app: tauri::AppHandle) -> ApiResponse<AutoClipSettingsView> {
    let cfg = settings::load(&state.db);
    ApiResponse::success(AutoClipSettingsView { asr: asr_state(&app, &cfg), settings: settings::masked(&cfg) })
}

#[tauri::command(async)]
pub fn auto_clip_settings_save(state: State<'_, AppState>, settings: AutoClipSettings) -> ApiResponse<AutoClipSettings> {
    match settings::save(&state.db, settings) {
        Ok(saved) => ApiResponse::success(settings::masked(&saved)),
        Err(err) => ApiResponse::error(err),
    }
}

/// 表单里尚未保存的配置也能测试；Key 为掩码时使用已保存的值
fn with_saved_key(state: &State<'_, AppState>, mut llm: LlmSettings) -> LlmSettings {
    let saved = settings::load(&state.db);
    llm.api_key = settings::resolve_api_key(&llm.api_key, &saved.llm.api_key);
    llm.base_url = llm.base_url.trim().trim_end_matches('/').to_string();
    llm
}

#[tauri::command]
pub async fn auto_clip_llm_test(state: State<'_, AppState>, llm: LlmSettings) -> Result<ApiResponse<TestResult>, String> {
    let llm = with_saved_key(&state, llm);
    Ok(ApiResponse::success(llm::test_connection(&llm).await))
}

#[tauri::command]
pub async fn auto_clip_llm_models(state: State<'_, AppState>, llm: LlmSettings) -> Result<ApiResponse<Vec<String>>, String> {
    let llm = with_saved_key(&state, llm);
    Ok(match llm::list_models(&llm).await {
        Ok(list) => ApiResponse::success(list),
        Err(err) => ApiResponse::error(err),
    })
}

#[tauri::command(async)]
pub fn auto_clip_asr_status(state: State<'_, AppState>, app: tauri::AppHandle) -> ApiResponse<AsrModelState> {
    ApiResponse::success(asr_state(&app, &settings::load(&state.db)))
}

/// 后台下载缺失的语音模型，进度通过 auto_clip_asr_status 查询
#[tauri::command(async)]
pub fn auto_clip_asr_download(state: State<'_, AppState>, app: tauri::AppHandle) -> ApiResponse<String> {
    if asr::download_progress().running {
        return ApiResponse::error("模型正在下载中");
    }
    let cfg = settings::load(&state.db);
    let dir = hl::model_dir(&app.path().app_data_dir().unwrap_or_default(), &cfg.asr);
    let source = cfg.asr.download_source.clone();
    let log_path = state.app_log_path.clone();
    std::thread::spawn(move || {
        if let Err(err) = asr::download_models(&dir, &source) {
            crate::utils::append_log(&log_path, &format!("asr_model_download_failed err={}", err));
        }
    });
    ApiResponse::success("已开始下载".to_string())
}

#[tauri::command(async)]
pub fn anchor_clip_profile_get(state: State<'_, AppState>, room_id: String) -> ApiResponse<String> {
    let value = state
        .db
        .with_conn(|c| c.query_row("SELECT clip_profile FROM live_room_settings WHERE room_id = ?1", [room_id.trim()], |r| r.get::<_, Option<String>>(0)))
        .ok()
        .flatten()
        .unwrap_or_else(|| "reaction".to_string());
    ApiResponse::success(value)
}

#[tauri::command(async)]
pub fn anchor_clip_profile_set(state: State<'_, AppState>, room_id: String, profile: String) -> ApiResponse<String> {
    let profile = profile.trim().to_string();
    if !["reaction", "game", "chat"].contains(&profile.as_str()) {
        return ApiResponse::error("内容类型无效");
    }
    let now = now_rfc3339();
    let result = state.db.with_conn(|c| {
        c.execute(
            "INSERT INTO live_room_settings (room_id, clip_profile, update_time) VALUES (?1, ?2, ?3) \
             ON CONFLICT(room_id) DO UPDATE SET clip_profile = excluded.clip_profile, update_time = excluded.update_time",
            (room_id.trim(), &profile, &now),
        )
    });
    match result {
        Ok(_) => ApiResponse::success(profile),
        Err(err) => ApiResponse::error(format!("保存内容类型失败: {}", err)),
    }
}

const VOICEPRINT_SECS: f64 = 120.0;

/// 房间主播声纹登记情况：未登记时返回 None，已登记返回来源说明
#[tauri::command(async)]
pub fn anchor_voiceprint_get(state: State<'_, AppState>, room_id: String) -> ApiResponse<Option<String>> {
    let raw = state
        .db
        .with_conn(|c| c.query_row("SELECT voiceprint FROM live_room_settings WHERE room_id = ?1", [room_id.trim()], |r| r.get::<_, Option<String>>(0)))
        .ok()
        .flatten();
    let note = raw
        .and_then(|r| serde_json::from_str::<serde_json::Value>(&r).ok())
        .and_then(|v| v.get("source").and_then(|s| s.as_str()).map(String::from));
    ApiResponse::success(note)
}

/// 从录播指定位置起取 120 秒（应只有主播在说话）登记主播声纹
#[tauri::command(async)]
pub fn anchor_voiceprint_enroll(state: State<'_, AppState>, app: tauri::AppHandle, room_id: String, record_id: i64, start_secs: f64) -> ApiResponse<String> {
    let room_id = room_id.trim().to_string();
    let cfg = settings::load(&state.db);
    let dir = hl::model_dir(&app.path().app_data_dir().unwrap_or_default(), &cfg.asr);
    if !hl::speaker::model_ready(&dir) {
        return ApiResponse::error("未下载声纹模型，请在设置 > 自动切片中下载模型");
    }
    let row = state.db.with_conn(|c| {
        c.query_row(
            "SELECT file_path FROM live_record_task WHERE id = ?1 AND room_id = ?2",
            (record_id, &room_id),
            |r| r.get::<_, String>(0),
        )
    });
    let Ok(file_path) = row else { return ApiResponse::error("录播记录不存在") };
    let Some(path) = crate::commands::anchor::resolve_existing_record_path(&file_path) else {
        return ApiResponse::error("录播文件不存在");
    };
    let result = asr::decode_range(&path, start_secs.max(0.0), VOICEPRINT_SECS)
        .and_then(|audio| hl::speaker::enroll(&dir, cfg.asr.threads, &audio));
    let embedding = match result {
        Ok(e) => e,
        Err(err) => return ApiResponse::error(err),
    };
    let secs = start_secs.max(0.0) as i64;
    let source = format!("录播 #{} {:02}:{:02}:{:02} 起 {} 秒", record_id, secs / 3600, secs % 3600 / 60, secs % 60, VOICEPRINT_SECS as i64);
    let value = serde_json::json!({ "embedding": embedding, "recordId": record_id, "startSecs": start_secs, "source": source }).to_string();
    let saved = state.db.with_conn(|c| {
        c.execute(
            "INSERT INTO live_room_settings (room_id, voiceprint, update_time) VALUES (?1, ?2, ?3) \
             ON CONFLICT(room_id) DO UPDATE SET voiceprint = excluded.voiceprint, update_time = excluded.update_time",
            (&room_id, &value, &now_rfc3339()),
        )
    });
    match saved {
        Ok(_) => ApiResponse::success(source),
        Err(err) => ApiResponse::error(format!("保存主播声纹失败: {}", err)),
    }
}
