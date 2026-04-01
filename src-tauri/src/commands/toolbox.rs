use std::fs;
use std::path::Path;

use serde::Deserialize;
use tauri::State;

use crate::api::ApiResponse;
use crate::ffmpeg::run_ffmpeg;
use crate::utils;
use crate::AppState;

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
