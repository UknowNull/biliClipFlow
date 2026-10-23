//! 投稿任务的百度网盘同步配置。

use super::*;

pub(super) fn extract_baidu_binding(merged: &MergedVideoRecord) -> Option<(String, String)> {
    let dir = merged
        .remote_dir
        .as_deref()
        .map(|value| value.trim())
        .unwrap_or("");
    let name = merged
        .remote_name
        .as_deref()
        .map(|value| value.trim())
        .unwrap_or("");
    if dir.is_empty() || name.is_empty() {
        return None;
    }
    Some((dir.to_string(), name.to_string()))
}

pub(super) fn reset_baidu_download_record(context: &SubmissionContext, record_id: i64) -> Result<(), String> {
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

pub(super) fn split_baidu_path(remote_path: &str) -> Option<(String, String)> {
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

pub(super) fn load_logged_baidu_uid(db: &Db) -> Result<Option<String>, String> {
    baidu_sync::get_active_baidu_uid(db)
}

pub(super) fn require_logged_baidu_uid(db: &Db) -> Result<String, String> {
    load_logged_baidu_uid(db)?.ok_or_else(|| "请先登录网盘账号".to_string())
}

pub(super) fn resolve_bound_baidu_uid(db: &Db, bilibili_uid: i64) -> Result<Option<String>, String> {
    crate::account_store::get_bound_baidu_uid(db, bilibili_uid)
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

pub(super) fn update_baidu_sync_config(
    context: &SubmissionContext,
    task_id: &str,
    enabled: Option<bool>,
    path: Option<String>,
    filename: Option<String>,
) -> Result<(), String> {
    if enabled.is_none() && path.is_none() && filename.is_none() {
        return Ok(());
    }
    let (current_enabled, current_path, current_filename, current_baidu_uid) = context
    .db
    .with_conn(|conn| {
      conn.query_row(
        "SELECT baidu_sync_enabled, baidu_sync_path, baidu_sync_filename, baidu_uid FROM submission_task WHERE task_id = ?1",
        [task_id],
        |row| {
          let enabled: i64 = row.get(0)?;
          let path: Option<String> = row.get(1)?;
          let filename: Option<String> = row.get(2)?;
          let baidu_uid: Option<String> = row.get(3)?;
          Ok((enabled != 0, path, filename, baidu_uid))
        },
      )
    })
    .map_err(|err| err.to_string())?;
    let login_baidu_uid = load_logged_baidu_uid(context.db.as_ref())?;
    let next_enabled = enabled.unwrap_or(current_enabled);
    let next_path = path.or(current_path);
    let normalized_filename = normalize_baidu_sync_filename(filename.as_deref());
    let next_filename = normalized_filename.or(current_filename);
    let next_baidu_uid = if next_enabled {
        login_baidu_uid
            .or(current_baidu_uid)
            .ok_or_else(|| "请先登录网盘账号".to_string())?
    } else {
        String::new()
    };
    let now = now_rfc3339();
    context
    .db
    .with_conn(|conn| {
      conn.execute(
        "UPDATE submission_task SET baidu_sync_enabled = ?1, baidu_sync_path = ?2, baidu_sync_filename = ?3, baidu_uid = ?4, updated_at = ?5 WHERE task_id = ?6",
        (
          if next_enabled { 1 } else { 0 },
          next_path.as_deref(),
          next_filename.as_deref(),
          if next_enabled {
            Some(next_baidu_uid.as_str())
          } else {
            None
          },
          &now,
          task_id,
        ),
      )?;
      Ok(())
    })
    .map_err(|err| err.to_string())
}

pub(super) fn resolve_baidu_source_path_for_task(
    context: &SubmissionContext,
    task_id: &str,
    remote_path: &str,
) -> PathBuf {
    let remote_name = remote_path
        .rsplit('/')
        .find(|value| !value.is_empty())
        .unwrap_or("baidu_source.mp4");
    resolve_submission_base_dir(context, task_id)
        .join("integrated_sources")
        .join(crate::utils::sanitize_filename(remote_name))
}
