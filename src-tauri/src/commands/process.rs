use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::State;

use crate::api::ApiResponse;
use crate::config::{default_download_dir, default_temp_dir};
use crate::db::Db;
use crate::processing::{clip_sources, decide_clip_copy, merge_files, ClipSource};
use crate::utils::{now_rfc3339, sanitize_filename};
use crate::AppState;

#[derive(Clone)]
struct ProcessContext {
    db: Arc<Db>,
}

impl ProcessContext {
    fn new(state: &State<'_, AppState>) -> Self {
        Self {
            db: state.db.clone(),
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessClipRequest {
    pub file_path: String,
    pub file_name: Option<String>,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub sequence: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessRequest {
    pub task_name: String,
    pub clips: Vec<ProcessClipRequest>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoProcessTask {
    pub id: i64,
    pub task_name: Option<String>,
    pub status: i64,
    pub progress: i64,
    pub input_files: Option<String>,
    pub output_path: Option<String>,
    pub upload_status: i64,
    pub bilibili_url: Option<String>,
    pub create_time: String,
    pub update_time: String,
}

#[tauri::command]
pub async fn process_create(
    state: State<'_, AppState>,
    request: ProcessRequest,
) -> Result<ApiResponse<i64>, String> {
    if request.task_name.trim().is_empty() {
        return Ok(ApiResponse::error("Task name is required"));
    }
    if request.clips.is_empty() {
        return Ok(ApiResponse::error("At least one clip is required"));
    }

    let now = now_rfc3339();
    let input_files = json!(request.clips).to_string();

    let task_id = match state.db.with_conn(|conn| {
    conn.execute(
      "INSERT INTO video_process_task (task_name, status, progress, input_files, output_path, upload_status, bilibili_url, create_time, update_time) \
       VALUES (?1, 0, 0, ?2, NULL, 0, NULL, ?3, ?4)",
      (&request.task_name, &input_files, &now, &now),
    )?;
    Ok(conn.last_insert_rowid())
  }) {
    Ok(id) => id,
    Err(err) => {
      return Ok(ApiResponse::error(format!(
        "Failed to create process task: {}",
        err
      )));
    }
  };

    let context = ProcessContext::new(&state);
    tauri::async_runtime::spawn(async move {
        let _ = run_process_task(context, task_id, request).await;
    });

    Ok(ApiResponse::success(task_id))
}

#[tauri::command]
pub fn process_status(state: State<'_, AppState>, task_id: i64) -> ApiResponse<VideoProcessTask> {
    match state.db.with_conn(|conn| {
    conn.query_row(
      "SELECT id, task_name, status, progress, input_files, output_path, upload_status, bilibili_url, create_time, update_time \
       FROM video_process_task WHERE id = ?1",
      [task_id],
      |row| {
        Ok(VideoProcessTask {
          id: row.get(0)?,
          task_name: row.get(1)?,
          status: row.get(2)?,
          progress: row.get(3)?,
          input_files: row.get(4)?,
          output_path: row.get(5)?,
          upload_status: row.get(6)?,
          bilibili_url: row.get(7)?,
          create_time: row.get(8)?,
          update_time: row.get(9)?,
        })
      },
    )
  }) {
    Ok(task) => ApiResponse::success(task),
    Err(err) => ApiResponse::error(format!("Failed to load task: {}", err)),
  }
}

async fn run_process_task(
    context: ProcessContext,
    task_id: i64,
    request: ProcessRequest,
) -> Result<(), String> {
    update_process_status(&context, task_id, 1, 0)?;

    let sources: Vec<ClipSource> = request
        .clips
        .iter()
        .enumerate()
        .map(|(index, clip)| ClipSource {
            input_path: clip.file_path.clone(),
            start_time: clip.start_time.clone(),
            end_time: clip.end_time.clone(),
            order: clip.sequence.unwrap_or((index + 1) as i64),
        })
        .collect();

    let temp_dir = default_temp_dir().join(format!("process_{}", task_id));
    let copy_decision =
        decide_clip_copy(&sources).unwrap_or_else(|_| crate::processing::ClipCopyDecision {
            use_copy: false,
            reason: None,
        });
    let use_copy = copy_decision.use_copy;
    let clip_outputs =
        tauri::async_runtime::spawn_blocking(move || clip_sources(&sources, &temp_dir, use_copy))
            .await
            .map_err(|_| "Failed to clip videos".to_string())??;

    let output_name = format!("{}_merged.mp4", sanitize_filename(&request.task_name));
    let output_path = default_download_dir().join(output_name);
    let output_path_clone = output_path.clone();
    tauri::async_runtime::spawn_blocking(move || merge_files(&clip_outputs, &output_path_clone))
        .await
        .map_err(|_| "Failed to merge videos".to_string())??;

    let output_path_string = output_path.to_string_lossy().to_string();
    update_process_output(&context, task_id, &output_path_string, 100)?;
    update_process_status(&context, task_id, 2, 100)?;

    Ok(())
}

fn update_process_status(
    context: &ProcessContext,
    task_id: i64,
    status: i64,
    progress: i64,
) -> Result<(), String> {
    let now = now_rfc3339();
    context
        .db
        .with_conn(|conn| {
            conn.execute(
        "UPDATE video_process_task SET status = ?1, progress = ?2, update_time = ?3 WHERE id = ?4",
        (status, progress, &now, task_id),
      )?;
            Ok(())
        })
        .map_err(|err| format!("Failed to update task: {}", err))
}

fn update_process_output(
    context: &ProcessContext,
    task_id: i64,
    output_path: &str,
    progress: i64,
) -> Result<(), String> {
    let now = now_rfc3339();
    context
    .db
    .with_conn(|conn| {
      conn.execute(
        "UPDATE video_process_task SET output_path = ?1, progress = ?2, update_time = ?3 WHERE id = ?4",
        (output_path, progress, &now, task_id),
      )?;
      Ok(())
    })
    .map_err(|err| format!("Failed to update output: {}", err))
}
