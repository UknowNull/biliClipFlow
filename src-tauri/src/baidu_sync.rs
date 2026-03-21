use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use rusqlite::OptionalExtension;
use serde::Serialize;
use tokio::time::{sleep, Duration};

use crate::commands::settings::DEFAULT_BAIDU_MAX_PARALLEL;
use crate::config::resolve_baidu_pcs_path;
use crate::db::Db;
use crate::path_store::{
  load_local_path_prefix, to_absolute_local_path_opt_with_prefix, to_absolute_local_path_with_prefix,
  to_stored_local_path,
};
use crate::utils::{append_log, apply_no_window, now_rfc3339, sanitize_filename};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BaiduSyncSettings {
  pub enabled: bool,
  pub exec_path: String,
  pub target_path: String,
  pub policy: String,
  pub retry: i64,
  pub concurrency: i64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BaiduLoginInfo {
  pub status: String,
  pub uid: Option<String>,
  pub username: Option<String>,
  pub login_type: Option<String>,
  pub login_time: Option<String>,
  pub last_check_time: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BaiduAccountSummary {
  pub uid: String,
  pub status: String,
  pub username: Option<String>,
  pub login_type: Option<String>,
  pub login_time: Option<String>,
  pub last_check_time: Option<String>,
  pub is_active: bool,
}

#[derive(Clone)]
struct BaiduLoginCredential {
  baidu_uid: String,
  login_type: String,
  cookie: Option<String>,
  bduss: Option<String>,
  stoken: Option<String>,
  last_attempt_time: Option<String>,
  last_attempt_error: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BaiduSyncTaskRecord {
  pub id: i64,
  pub source_type: String,
  pub source_id: Option<String>,
  pub baidu_uid: Option<String>,
  pub source_title: Option<String>,
  pub local_path: String,
  pub remote_dir: String,
  pub remote_name: String,
  pub status: String,
  pub progress: f64,
  pub error: Option<String>,
  pub retry_count: i64,
  pub policy: Option<String>,
  pub created_at: String,
  pub updated_at: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BaiduRemoteDir {
  pub name: String,
  pub path: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BaiduRemoteEntry {
  pub name: String,
  pub path: String,
  pub is_dir: bool,
}

#[derive(Clone)]
pub struct BaiduSyncContext {
  pub db: Arc<Db>,
  pub app_log_path: Arc<PathBuf>,
  pub runtime: Arc<BaiduSyncRuntime>,
}

pub struct BaiduSyncRuntime {
  active_count: Mutex<i64>,
}

impl BaiduSyncRuntime {
  pub fn new() -> Self {
    Self {
      active_count: Mutex::new(0),
    }
  }
}

#[derive(Clone)]
struct BaiduSyncTask {
  id: i64,
  source_type: String,
  source_id: Option<String>,
  baidu_uid: Option<String>,
  local_path: String,
  remote_dir: String,
  remote_name: String,
  retry_count: i64,
  policy: Option<String>,
}

pub fn load_baidu_sync_settings(db: &Db) -> Result<BaiduSyncSettings, String> {
  db.with_conn(|conn| {
    let enabled = true;
    let exec_path = read_setting(conn, "baidu_sync_exec_path").unwrap_or_default();
    let target_path = read_setting(conn, "baidu_sync_target_path").unwrap_or_else(|| "/录播".to_string());
    let policy = read_setting(conn, "baidu_sync_policy").unwrap_or_else(|| "overwrite".to_string());
    let retry = read_setting(conn, "baidu_sync_retry")
      .and_then(|value| value.parse::<i64>().ok())
      .unwrap_or(2);
    let concurrency_value = read_setting(conn, "baidu_sync_concurrency");
    let concurrency = concurrency_value
      .as_deref()
      .and_then(|value| value.parse::<i64>().ok())
      .unwrap_or(3)
      .max(1);
    if concurrency_value.is_none() {
      let now = now_rfc3339();
      let _ = upsert_setting(conn, "baidu_sync_concurrency", "3", &now);
    }
    Ok(BaiduSyncSettings {
      enabled,
      exec_path,
      target_path,
      policy,
      retry,
      concurrency,
    })
  })
  .map_err(|err| err.to_string())
}

pub fn load_baidu_login_info(db: &Db) -> Result<Option<BaiduLoginInfo>, String> {
  if let Some(uid) = get_active_baidu_uid(db)? {
    return load_baidu_login_info_by_uid(db, &uid);
  }
  db.with_conn(|conn| {
    conn
      .query_row(
        "SELECT status, uid, username, login_type, login_time, last_check_time \
         FROM baidu_account_info ORDER BY login_time DESC, update_time DESC LIMIT 1",
        [],
        |row| {
          Ok(BaiduLoginInfo {
            status: row.get(0)?,
            uid: row.get(1)?,
            username: row.get(2)?,
            login_type: row.get(3)?,
            login_time: row.get(4)?,
            last_check_time: row.get(5)?,
          })
        },
      )
      .optional()
  })
  .map_err(|err| err.to_string())
}

pub fn upsert_baidu_login_info(db: &Db, info: &BaiduLoginInfo) -> Result<(), String> {
  let Some(uid) = info.uid.as_deref().map(|value| value.trim()).filter(|value| !value.is_empty()) else {
    return Ok(());
  };
  let now = now_rfc3339();
  db.with_conn(|conn| {
    conn.execute(
      "INSERT INTO baidu_account_info (uid, status, username, login_type, login_time, last_check_time, create_time, update_time) \
       VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
       ON CONFLICT(uid) DO UPDATE SET \
         status = excluded.status, \
         username = excluded.username, \
         login_type = excluded.login_type, \
         login_time = excluded.login_time, \
         last_check_time = excluded.last_check_time, \
         update_time = excluded.update_time",
      (
        uid,
        info.status.as_str(),
        info.username.as_deref(),
        info.login_type.as_deref(),
        info.login_time.as_deref(),
        info.last_check_time.as_deref(),
        &now,
        &now,
      ),
    )?;
    Ok(())
  })
  .map_err(|err| err.to_string())?;
  set_active_baidu_uid(db, Some(uid))?;
  Ok(())
}

pub fn load_baidu_login_info_by_uid(db: &Db, uid: &str) -> Result<Option<BaiduLoginInfo>, String> {
  let trimmed_uid = uid.trim();
  if trimmed_uid.is_empty() {
    return Ok(None);
  }
  db.with_conn(|conn| {
    conn
      .query_row(
        "SELECT status, uid, username, login_type, login_time, last_check_time \
         FROM baidu_account_info WHERE uid = ?1",
        [trimmed_uid],
        |row| {
          Ok(BaiduLoginInfo {
            status: row.get(0)?,
            uid: row.get(1)?,
            username: row.get(2)?,
            login_type: row.get(3)?,
            login_time: row.get(4)?,
            last_check_time: row.get(5)?,
          })
        },
      )
      .optional()
  })
  .map_err(|err| err.to_string())
}

pub fn list_baidu_accounts(db: &Db) -> Result<Vec<BaiduAccountSummary>, String> {
  let active_uid = crate::account_store::get_active_baidu_uid(db)?;
  db.with_conn(|conn| {
    let mut stmt = conn.prepare(
      "SELECT uid, status, username, login_type, login_time, last_check_time \
       FROM baidu_account_info ORDER BY login_time DESC, update_time DESC",
    )?;
    let rows = stmt.query_map([], |row| {
      let uid: String = row.get(0)?;
      Ok(BaiduAccountSummary {
        is_active: active_uid.as_deref() == Some(uid.as_str()),
        uid,
        status: row.get(1)?,
        username: row.get(2)?,
        login_type: row.get(3)?,
        login_time: row.get(4)?,
        last_check_time: row.get(5)?,
      })
    })?;
    rows.collect::<Result<Vec<_>, _>>()
  })
  .map_err(|err| err.to_string())
}

pub fn get_active_baidu_uid(db: &Db) -> Result<Option<String>, String> {
  crate::account_store::get_active_baidu_uid(db)
}

pub fn set_active_baidu_uid(db: &Db, uid: Option<&str>) -> Result<(), String> {
  crate::account_store::set_active_baidu_uid(db, uid)
}

pub fn update_baidu_sync_settings(db: &Db, settings: &BaiduSyncSettings) -> Result<(), String> {
  let now = now_rfc3339();
  db.with_conn(|conn| {
    upsert_setting(conn, "baidu_sync_exec_path", &settings.exec_path, &now)?;
    upsert_setting(conn, "baidu_sync_target_path", &settings.target_path, &now)?;
    upsert_setting(conn, "baidu_sync_policy", &settings.policy, &now)?;
    upsert_setting(conn, "baidu_sync_retry", &settings.retry.to_string(), &now)?;
    upsert_setting(conn, "baidu_sync_concurrency", &settings.concurrency.to_string(), &now)?;
    Ok(())
  })
  .map_err(|err| err.to_string())
}

pub fn list_baidu_sync_tasks(
  db: &Db,
  status: Option<String>,
  page: i64,
  page_size: i64,
) -> Result<Vec<BaiduSyncTaskRecord>, String> {
  let status_filter = status.filter(|value| !value.trim().is_empty());
  let page_size = page_size.clamp(1, 200);
  let offset = (page - 1).max(0) * page_size;
  let storage_prefix = load_local_path_prefix(db);
  db.with_conn(|conn| {
    let mut stmt = if status_filter.is_some() {
      conn.prepare(
        "SELECT id, source_type, source_id, baidu_uid, source_title, local_path, remote_dir, remote_name, status, progress, error, retry_count, policy, created_at, updated_at \
         FROM baidu_sync_task WHERE status = ?1 ORDER BY created_at DESC LIMIT ?2 OFFSET ?3",
      )?
    } else {
      conn.prepare(
        "SELECT id, source_type, source_id, baidu_uid, source_title, local_path, remote_dir, remote_name, status, progress, error, retry_count, policy, created_at, updated_at \
         FROM baidu_sync_task ORDER BY created_at DESC LIMIT ?1 OFFSET ?2",
      )?
    };

    let rows = if let Some(status) = status_filter {
      stmt.query_map((status, page_size, offset), map_baidu_sync_task)?
    } else {
      stmt.query_map((page_size, offset), map_baidu_sync_task)?
    };
    let mut list = rows.collect::<Result<Vec<_>, _>>()?;
    for item in &mut list {
      item.local_path =
        to_absolute_local_path_opt_with_prefix(storage_prefix.as_path(), Some(item.local_path.clone()))
          .unwrap_or_default();
    }
    Ok(list)
  })
  .map_err(|err| err.to_string())
}

pub fn list_baidu_remote_dirs(
  db: &Db,
  path: &str,
) -> Result<Vec<BaiduRemoteDir>, String> {
  let entries = list_baidu_remote_entries(db, path)?;
  Ok(
    entries
      .into_iter()
      .filter(|item| item.is_dir)
      .map(|item| BaiduRemoteDir {
        name: item.name,
        path: item.path,
      })
      .collect(),
  )
}

pub fn list_baidu_remote_entries(
  db: &Db,
  path: &str,
) -> Result<Vec<BaiduRemoteEntry>, String> {
  let settings = load_baidu_sync_settings(db)?;
  let exec_path = resolve_baidu_exec_path(&settings.exec_path);
  let target_path = normalize_baidu_path(path);
  let output = run_baidu_pcs_command(&exec_path, &["ls".to_string(), target_path.clone()])?;
  let content = if output.stdout.trim().is_empty() {
    output.stderr
  } else {
    output.stdout
  };
  Ok(parse_baidu_ls_entries(&content, &target_path))
}

pub fn check_baidu_remote_file_exists(db: &Db, remote_path: &str) -> Result<bool, String> {
  let settings = load_baidu_sync_settings(db)?;
  let exec_path = resolve_baidu_exec_path(&settings.exec_path);
  let target_path = normalize_baidu_path(remote_path);
  match run_baidu_pcs_command(&exec_path, &["meta".to_string(), target_path]) {
    Ok(_) => Ok(true),
    Err(err) => {
      if is_baidu_not_found_error(&err) {
        Ok(false)
      } else {
        Err(err)
      }
    }
  }
}

pub fn fetch_baidu_remote_file_size(db: &Db, remote_path: &str) -> Result<u64, String> {
  let settings = load_baidu_sync_settings(db)?;
  let exec_path = resolve_baidu_exec_path(&settings.exec_path);
  let target_path = normalize_baidu_path(remote_path);
  let output = run_baidu_pcs_command(&exec_path, &["meta".to_string(), target_path])?;
  Ok(parse_meta_size(&output.stdout).unwrap_or(0))
}

fn load_baidu_download_max_parallel(db: &Db) -> i64 {
  db.with_conn(|conn| {
    let value: Option<String> = conn
      .query_row(
        "SELECT value FROM app_settings WHERE key = 'download_baidu_max_parallel'",
        [],
        |row| row.get(0),
      )
      .ok();
    Ok(
      value
        .and_then(|item| item.parse::<i64>().ok())
        .unwrap_or(DEFAULT_BAIDU_MAX_PARALLEL)
        .clamp(1, 100),
    )
  })
  .unwrap_or(DEFAULT_BAIDU_MAX_PARALLEL)
}

fn apply_baidu_download_max_parallel(db: &Db, exec_path: &Path) -> Result<(), String> {
  let max_parallel = load_baidu_download_max_parallel(db);
  run_baidu_pcs_command(
    exec_path,
    &[
      "config".to_string(),
      "set".to_string(),
      "-max_parallel".to_string(),
      max_parallel.to_string(),
    ],
  )?;
  Ok(())
}

pub fn download_baidu_file(
  db: &Db,
  remote_path: &str,
  local_path: &Path,
) -> Result<PathBuf, String> {
  download_baidu_file_with_hook(db, remote_path, local_path, |_| {})
}

pub fn download_baidu_file_with_hook<F>(
  db: &Db,
  remote_path: &str,
  local_path: &Path,
  on_spawn: F,
) -> Result<PathBuf, String>
where
  F: FnOnce(Arc<Mutex<Child>>),
{
  let settings = load_baidu_sync_settings(db)?;
  let exec_path = resolve_baidu_exec_path(&settings.exec_path);
  let target_path = normalize_baidu_path(remote_path);
  let local_dir = match local_path.parent() {
    Some(value) => value,
    None => return Err("下载目标目录无效".to_string()),
  };
  if let Err(err) = fs::create_dir_all(local_dir) {
    return Err(format!("创建下载目录失败: {}", err));
  }
  apply_baidu_download_max_parallel(db, &exec_path)?;
  let remote_name = target_path
    .rsplit('/')
    .find(|value| !value.is_empty())
    .unwrap_or("")
    .to_string();
  if remote_name.is_empty() {
    return Err("网盘文件名为空".to_string());
  }
  let _ = run_baidu_pcs_download_with_hook(&exec_path, &target_path, local_dir, on_spawn)?;
  if local_path.exists() {
    return Ok(local_path.to_path_buf());
  }
  let direct_path = local_dir.join(&remote_name);
  if direct_path.exists() {
    if direct_path == local_path {
      return Ok(direct_path);
    }
    if local_path.exists() {
      return Ok(local_path.to_path_buf());
    }
    fs::rename(&direct_path, local_path)
      .map_err(|err| format!("重命名下载文件失败: {}", err))?;
    return Ok(local_path.to_path_buf());
  }
  if let Some(found) = find_file_by_name(local_dir, &remote_name) {
    if found == local_path {
      return Ok(found);
    }
    if local_path.exists() {
      return Ok(local_path.to_path_buf());
    }
    fs::rename(&found, local_path)
      .map_err(|err| format!("重命名下载文件失败: {}", err))?;
    return Ok(local_path.to_path_buf());
  }
  Err("网盘文件下载完成但未找到本地文件".to_string())
}

pub fn create_baidu_remote_dir(
  db: &Db,
  parent_path: &str,
  name: &str,
) -> Result<BaiduRemoteDir, String> {
  let settings = load_baidu_sync_settings(db)?;
  let exec_path = resolve_baidu_exec_path(&settings.exec_path);
  let safe_name = sanitize_filename(name.trim());
  if safe_name.is_empty() {
    return Err("目录名称不能为空".to_string());
  }
  let base_path = normalize_baidu_path(parent_path);
  let full_path = join_baidu_path(&base_path, &safe_name);
  run_baidu_pcs_command(&exec_path, &["mkdir".to_string(), full_path.clone()])?;
  Ok(BaiduRemoteDir {
    name: safe_name,
    path: full_path,
  })
}

pub fn rename_baidu_remote_dir(
  db: &Db,
  from_path: &str,
  name: &str,
) -> Result<BaiduRemoteDir, String> {
  let settings = load_baidu_sync_settings(db)?;
  let exec_path = resolve_baidu_exec_path(&settings.exec_path);
  let normalized_from = normalize_baidu_path(from_path);
  if normalized_from == "/" {
    return Err("无法重命名根目录".to_string());
  }
  let safe_name = sanitize_filename(name.trim());
  if safe_name.is_empty() {
    return Err("目录名称不能为空".to_string());
  }
  let parent_path = {
    let mut segments: Vec<&str> = normalized_from.split('/').filter(|value| !value.is_empty()).collect();
    segments.pop();
    if segments.is_empty() {
      "/".to_string()
    } else {
      format!("/{}", segments.join("/"))
    }
  };
  let target_path = join_baidu_path(&parent_path, &safe_name);
  if target_path != normalized_from {
    run_baidu_pcs_command(
      &exec_path,
      &["mv".to_string(), normalized_from.clone(), target_path.clone()],
    )?;
    update_submission_sync_paths(db, &normalized_from, &target_path)?;
  }
  Ok(BaiduRemoteDir {
    name: safe_name,
    path: target_path,
  })
}

fn update_submission_sync_paths(db: &Db, from_path: &str, to_path: &str) -> Result<(), String> {
  let from_path = normalize_baidu_path(from_path);
  let to_path = normalize_baidu_path(to_path);
  let like_pattern = format!("{}/%", from_path.trim_end_matches('/'));
  db.with_conn(|conn| {
    conn.execute(
      "UPDATE submission_task \
       SET baidu_sync_path = CASE \
         WHEN baidu_sync_path = ?1 THEN ?2 \
         WHEN baidu_sync_path LIKE ?3 THEN ?2 || SUBSTR(baidu_sync_path, LENGTH(?1) + 1) \
         ELSE baidu_sync_path \
       END \
       WHERE baidu_sync_path = ?1 OR baidu_sync_path LIKE ?3",
      (&from_path, &to_path, &like_pattern),
    )?;
    Ok(())
  })
  .map_err(|err| err.to_string())
}

pub fn enqueue_submission_sync(
  db: &Db,
  app_log_path: &Path,
  task_id: &str,
) -> Result<(), String> {
  let settings = load_baidu_sync_settings(db)?;
  append_log(
    app_log_path,
    &format!("baidu_sync_enqueue_submission_start task_id={}", task_id),
  );
  let task = db.with_conn(|conn| {
    conn.query_row(
      "SELECT title, baidu_sync_enabled, baidu_sync_path, baidu_sync_filename, baidu_uid FROM submission_task WHERE task_id = ?1",
      [task_id],
      |row| {
        let title: String = row.get(0)?;
        let enabled: i64 = row.get(1)?;
        let path: Option<String> = row.get(2)?;
        let filename: Option<String> = row.get(3)?;
        let baidu_uid: Option<String> = row.get(4)?;
        Ok((title, enabled != 0, path, filename, baidu_uid))
      },
    )
  });
  let (title, task_enabled, task_path, task_filename, task_baidu_uid) = match task {
    Ok(value) => value,
    Err(err) => return Err(err.to_string()),
  };
  if !task_enabled {
    append_log(
      app_log_path,
      &format!("baidu_sync_enqueue_skip task_id={} reason=disabled", task_id),
    );
    return Ok(());
  }
  let merged = db
    .with_conn(|conn| {
      conn.query_row(
        "SELECT video_path, file_name FROM merged_video WHERE task_id = ?1 ORDER BY id DESC LIMIT 1",
        [task_id],
        |row| {
          let path: Option<String> = row.get(0)?;
          let name: Option<String> = row.get(1)?;
          Ok((path, name))
        },
      )
    })
    .map_err(|err| err.to_string())?;
  let local_path = match merged.0 {
    Some(path) if !path.trim().is_empty() => path,
    _ => {
      append_log(
        app_log_path,
        &format!("baidu_sync_enqueue_skip task_id={} reason=missing_merged_path", task_id),
      );
      return Ok(());
    }
  };
  let local_name = merged
    .1
    .or_else(|| Path::new(&local_path).file_name().and_then(|v| v.to_str()).map(|v| v.to_string()))
    .unwrap_or_else(|| "merged.mp4".to_string());
  let base_path = normalize_baidu_path(task_path.as_deref().unwrap_or(&settings.target_path));
  let remote_dir = base_path;
  let remote_name = task_filename
    .as_deref()
    .map(|name| name.trim())
    .filter(|name| !name.is_empty())
    .map(sanitize_filename)
    .unwrap_or_else(|| sanitize_filename(&local_name));
  if let Err(err) = bind_submission_merged_remote(
    db,
    task_id,
    &local_path,
    &remote_dir,
    &remote_name,
    task_baidu_uid.as_deref(),
  )
  {
    append_log(
      app_log_path,
      &format!(
        "baidu_sync_bind_merged_pending_fail task_id={} local={} remote_dir={} remote_name={} err={}",
        task_id, local_path, remote_dir, remote_name, err
      ),
    );
  } else {
    append_log(
      app_log_path,
      &format!(
        "baidu_sync_bind_merged_pending_ok task_id={} remote_dir={} remote_name={}",
        task_id, remote_dir, remote_name
      ),
    );
  }
  append_log(
    app_log_path,
    &format!(
      "baidu_sync_enqueue_submission task_id={} local={} remote_dir={} remote_name={}",
      task_id, local_path, remote_dir, remote_name
    ),
  );
  insert_baidu_sync_task(
    db,
    "submission_merged",
    Some(task_id.to_string()),
    task_baidu_uid,
    Some(title),
    &local_path,
    &remote_dir,
    &remote_name,
    &settings.policy,
  )?;
  Ok(())
}

pub fn enqueue_live_sync(
  db: &Db,
  app_log_path: &Path,
  record_id: i64,
) -> Result<(), String> {
  let settings = load_baidu_sync_settings(db)?;
  append_log(
    app_log_path,
    &format!("baidu_sync_enqueue_live_start record_id={}", record_id),
  );
  let record = db.with_conn(|conn| {
    conn.query_row(
      "SELECT room_id, title, file_path, start_time FROM live_record_task WHERE id = ?1",
      [record_id],
      |row| {
        let room_id: String = row.get(0)?;
        let title: Option<String> = row.get(1)?;
        let file_path: String = row.get(2)?;
        let start_time: String = row.get(3)?;
        Ok((room_id, title, file_path, start_time))
      },
    )
  });
  let (room_id, title, file_path, start_time) = match record {
    Ok(value) => value,
    Err(err) => return Err(err.to_string()),
  };
  if file_path.trim().is_empty() {
    append_log(
      app_log_path,
      &format!("baidu_sync_enqueue_skip record_id={} reason=missing_record_path", record_id),
    );
    return Ok(());
  }
  let local_name = Path::new(&file_path)
    .file_name()
    .and_then(|value| value.to_str())
    .unwrap_or("record.mp4")
    .to_string();
  let live_date = parse_date(&start_time).unwrap_or_else(|| Utc::now().format("%Y%m%d").to_string());
  let (sync_enabled, room_path) =
    load_room_baidu_sync_config(db, &room_id).unwrap_or((false, None));
  if !sync_enabled {
    append_log(
      app_log_path,
      &format!("baidu_sync_enqueue_skip record_id={} reason=disabled", record_id),
    );
    return Ok(());
  }
  let base_path = match room_path {
    Some(value) if !value.trim().is_empty() => normalize_baidu_path(&value),
    _ => {
      append_log(
        app_log_path,
        &format!("baidu_sync_enqueue_skip record_id={} reason=missing_path", record_id),
      );
      return Ok(());
    }
  };
  let remote_dir = join_baidu_path(&base_path, &live_date);
  let remote_name = render_filename(None, &local_name, &live_date, None, &local_name);
  append_log(
    app_log_path,
    &format!(
      "baidu_sync_enqueue_live record_id={} local={} remote_dir={} remote_name={}",
      record_id, file_path, remote_dir, remote_name
    ),
  );
  let current_baidu_uid = load_baidu_login_info(db)?
    .and_then(|info| info.uid)
    .map(|value| value.trim().to_string())
    .filter(|value| !value.is_empty());
  insert_baidu_sync_task(
    db,
    "live_segment",
    Some(record_id.to_string()),
    current_baidu_uid,
    title,
    &file_path,
    &remote_dir,
    &remote_name,
    &settings.policy,
  )?;
  Ok(())
}

pub fn start_baidu_sync_loop(context: BaiduSyncContext) {
  recover_baidu_sync_tasks(context.db.as_ref(), context.app_log_path.as_ref());
  tauri::async_runtime::spawn(async move {
    loop {
      let settings = match load_baidu_sync_settings(context.db.as_ref()) {
        Ok(value) => value,
        Err(_) => {
          sleep(Duration::from_secs(10)).await;
          continue;
        }
      };
      let mut launched = 0;
      loop {
        let active = context.runtime.active_count.lock().map(|value| *value).unwrap_or(0);
        if active >= settings.concurrency {
          break;
        }
        let task = match load_next_pending_task(context.db.as_ref()) {
          Ok(Some(task)) => task,
          Ok(None) => break,
          Err(_) => break,
        };
        if let Ok(mut guard) = context.runtime.active_count.lock() {
          *guard += 1;
        }
        let task_context = context.clone();
        let runtime = Arc::clone(&context.runtime);
        let app_log_path = Arc::clone(&context.app_log_path);
        let settings_clone = settings.clone();
        tauri::async_runtime::spawn(async move {
          let result = run_baidu_sync_task(task_context, settings_clone, task).await;
          if let Ok(mut guard) = runtime.active_count.lock() {
            *guard = (*guard - 1).max(0);
          }
          if let Err(err) = result {
            append_log(
              app_log_path.as_ref(),
              &format!("baidu_sync_task_fail err={}", err),
            );
          }
        });
        launched += 1;
        if launched >= settings.concurrency {
          break;
        }
      }
      sleep(Duration::from_secs(3)).await;
    }
  });
}

pub fn recover_baidu_sync_tasks(db: &Db, app_log_path: &Path) {
  let now = now_rfc3339();
  let result = db.with_conn(|conn| {
    conn.execute(
      "UPDATE baidu_sync_task SET status = 'PENDING', progress = 0.0, updated_at = ?1 WHERE status = 'UPLOADING'",
      [&now],
    )?;
    Ok(())
  });
  if result.is_ok() {
    append_log(app_log_path, "baidu_sync_recover_ok");
  }
}

pub fn retry_baidu_sync_task(db: &Db, task_id: i64) -> Result<(), String> {
  let now = now_rfc3339();
  db.with_conn(|conn| {
    conn.execute(
      "UPDATE baidu_sync_task SET status = 'PENDING', progress = 0.0, error = NULL, updated_at = ?1 WHERE id = ?2",
      (&now, task_id),
    )?;
    Ok(())
  })
  .map_err(|err| err.to_string())
}

pub fn cancel_baidu_sync_task(db: &Db, task_id: i64) -> Result<(), String> {
  let now = now_rfc3339();
  db.with_conn(|conn| {
    conn.execute(
      "UPDATE baidu_sync_task SET status = 'CANCELLED', updated_at = ?1 WHERE id = ?2",
      (&now, task_id),
    )?;
    Ok(())
  })
  .map_err(|err| err.to_string())
}

pub fn pause_baidu_sync_task(db: &Db, task_id: i64) -> Result<(), String> {
  let now = now_rfc3339();
  db.with_conn(|conn| {
    conn.execute(
      "UPDATE baidu_sync_task SET status = 'PAUSED', updated_at = ?1 WHERE id = ?2",
      (&now, task_id),
    )?;
    Ok(())
  })
  .map_err(|err| err.to_string())
}

pub fn delete_baidu_sync_task(db: &Db, task_id: i64) -> Result<(), String> {
  db.with_conn(|conn| {
    conn.execute("DELETE FROM baidu_sync_task WHERE id = ?1", [task_id])?;
    Ok(())
  })
  .map_err(|err| err.to_string())
}

pub fn check_baidu_login(db: &Db) -> Result<BaiduLoginInfo, String> {
  check_baidu_login_internal(db, true)
}

fn check_baidu_login_internal(db: &Db, allow_auto_relogin: bool) -> Result<BaiduLoginInfo, String> {
  let settings = load_baidu_sync_settings(db)?;
  let exec_path = resolve_baidu_exec_path(&settings.exec_path);
  let now = now_rfc3339();
  let active_uid = get_active_baidu_uid(db)?;
  let previous = load_baidu_login_info(db)?.unwrap_or(BaiduLoginInfo {
    status: "LOGGED_OUT".to_string(),
    uid: active_uid.clone(),
    username: None,
    login_type: None,
    login_time: None,
    last_check_time: None,
  });
  let who_output = match run_baidu_pcs_command(&exec_path, &["who".to_string()]) {
    Ok(output) => output,
    Err(err) => {
      if is_baidu_busy_error(&err) && previous.login_type.is_some() {
        let mut info = previous.clone();
        info.last_check_time = Some(now);
        let _ = upsert_baidu_login_info(db, &info);
        return Ok(info);
      }
      return Err(err);
    }
  };
  if is_baidu_busy_error(&who_output.stdout) && previous.login_type.is_some() {
    let mut info = previous.clone();
    info.last_check_time = Some(now);
    let _ = upsert_baidu_login_info(db, &info);
    return Ok(info);
  }
  let (logged_in, uid, username) = parse_who_output(&who_output.stdout);
  let current_uid = uid.clone().or(active_uid.clone());
  if !logged_in && allow_auto_relogin {
    if let Some(credential) = load_baidu_login_credential(db)? {
      if should_attempt_relogin(&credential, &now) {
        let attempt_result = relogin_with_credential(db, &exec_path, &credential);
        let _ = update_baidu_login_credential_attempt(db, attempt_result.as_ref().err());
        if let Ok(info) = attempt_result {
          return Ok(info);
        }
      }
    }
  }
  let info = BaiduLoginInfo {
    status: if logged_in { "LOGGED_IN" } else { "LOGGED_OUT" }.to_string(),
    uid: current_uid,
    username,
    login_type: previous.login_type,
    login_time: previous.login_time,
    last_check_time: Some(now),
  };
  upsert_baidu_login_info(db, &info)?;
  Ok(info)
}

fn is_baidu_busy_error(err: &str) -> bool {
  err.contains("50052") || err.contains("系统繁忙")
}

pub fn login_baidu_with_cookie(db: &Db, cookie: &str) -> Result<BaiduLoginInfo, String> {
  let settings = load_baidu_sync_settings(db)?;
  let exec_path = resolve_baidu_exec_path(&settings.exec_path);
  let cookie = normalize_baidu_cookie(cookie)?;
  run_baidu_pcs_command(
    &exec_path,
    &["login".to_string(), format!("-cookies={}", cookie)],
  )?;
  let info = check_baidu_login_internal(db, false)?;
  let mut next = info.clone();
  next.login_type = Some("cookie".to_string());
  if next.login_time.is_none() {
    next.login_time = Some(now_rfc3339());
  }
  upsert_baidu_login_info(db, &next)?;
  let baidu_uid = next
    .uid
    .clone()
    .ok_or_else(|| "未识别到网盘账号 UID".to_string())?;
  let credential = BaiduLoginCredential {
    baidu_uid,
    login_type: "cookie".to_string(),
    cookie: Some(cookie),
    bduss: None,
    stoken: None,
    last_attempt_time: None,
    last_attempt_error: None,
  };
  upsert_baidu_login_credential(db, &credential)?;
  Ok(next)
}

fn normalize_baidu_cookie(input: &str) -> Result<String, String> {
  let raw = input.trim();
  if raw.is_empty() {
    return Err("Cookie 不能为空".to_string());
  }
  let mut cleaned = raw.to_string();
  if raw.to_ascii_lowercase().starts_with("cookie:") {
    cleaned = raw[7..].trim().to_string();
  }
  cleaned = cleaned.replace('\r', ";").replace('\n', ";");
  let mut items: Vec<String> = Vec::new();
  let mut has_bduss = false;
  let mut has_bduss_bfess = false;
  for part in cleaned.split(';') {
    let token = part.trim();
    if token.is_empty() || !token.contains('=') {
      continue;
    }
    let mut iter = token.splitn(2, '=');
    let key = iter.next().unwrap_or("").trim();
    let value = iter.next().unwrap_or("").trim();
    if key.is_empty() || value.is_empty() {
      continue;
    }
    let key_lower = key.to_ascii_lowercase();
    if matches!(
      key_lower.as_str(),
      "path"
        | "domain"
        | "expires"
        | "max-age"
        | "secure"
        | "httponly"
        | "samesite"
        | "priority"
    ) {
      continue;
    }
    if key_lower == "bduss" {
      has_bduss = true;
    }
    if key_lower == "bduss_bfess" {
      has_bduss_bfess = true;
    }
    items.push(format!("{}={}", key, value));
  }
  if items.is_empty() {
    return Err("Cookie 无有效字段".to_string());
  }
  if !has_bduss {
    if has_bduss_bfess {
      return Err("Cookie 缺少 BDUSS，仅包含 BDUSS_BFESS".to_string());
    }
    return Err("Cookie 缺少 BDUSS".to_string());
  }
  Ok(items.join("; "))
}

pub fn login_baidu_with_bduss(
  db: &Db,
  bduss: &str,
  stoken: Option<&str>,
) -> Result<BaiduLoginInfo, String> {
  let settings = load_baidu_sync_settings(db)?;
  let exec_path = resolve_baidu_exec_path(&settings.exec_path);
  let bduss = normalize_baidu_token(bduss, "BDUSS")?;
  let stoken = match stoken {
    Some(value) => normalize_baidu_token_optional(value, "STOKEN")?,
    None => None,
  };
  let mut args = vec!["login".to_string(), format!("-bduss={}", bduss)];
  if let Some(stoken_value) = stoken.as_deref() {
    args.push(format!("-stoken={}", stoken_value));
  }
  run_baidu_pcs_command(&exec_path, &args)?;
  let info = check_baidu_login_internal(db, false)?;
  let mut next = info.clone();
  next.login_type = Some("bduss".to_string());
  if next.login_time.is_none() {
    next.login_time = Some(now_rfc3339());
  }
  upsert_baidu_login_info(db, &next)?;
  let baidu_uid = next
    .uid
    .clone()
    .ok_or_else(|| "未识别到网盘账号 UID".to_string())?;
  let credential = BaiduLoginCredential {
    baidu_uid,
    login_type: "bduss".to_string(),
    cookie: None,
    bduss: Some(bduss),
    stoken,
    last_attempt_time: None,
    last_attempt_error: None,
  };
  upsert_baidu_login_credential(db, &credential)?;
  Ok(next)
}

fn normalize_baidu_token(input: &str, label: &str) -> Result<String, String> {
  let raw = input.trim();
  if raw.is_empty() {
    return Err(format!("{} 不能为空", label));
  }
  let cleaned = raw.replace('\r', ";").replace('\n', ";");
  let label_lower = label.to_ascii_lowercase();
  if cleaned.contains(';') || cleaned.contains('=') {
    for part in cleaned.split(';') {
      let token = part.trim();
      if token.is_empty() || !token.contains('=') {
        continue;
      }
      let mut iter = token.splitn(2, '=');
      let key = iter.next().unwrap_or("").trim();
      let value = iter.next().unwrap_or("").trim();
      if key.eq_ignore_ascii_case(label) && !value.is_empty() {
        return Ok(value.to_string());
      }
    }
  }
  let lower = cleaned.to_ascii_lowercase();
  let prefix = format!("{}=", label_lower);
  if lower.starts_with(&prefix) {
    let value = cleaned[label.len() + 1..].trim();
    if value.is_empty() {
      return Err(format!("{} 不能为空", label));
    }
    return Ok(value.to_string());
  }
  Ok(cleaned.trim().to_string())
}

fn normalize_baidu_token_optional(input: &str, label: &str) -> Result<Option<String>, String> {
  let raw = input.trim();
  if raw.is_empty() {
    return Ok(None);
  }
  normalize_baidu_token(raw, label).map(Some)
}

pub fn logout_baidu(db: &Db) -> Result<(), String> {
  let settings = load_baidu_sync_settings(db)?;
  let exec_path = resolve_baidu_exec_path(&settings.exec_path);
  let _ = run_baidu_pcs_command(&exec_path, &["logout".to_string()]);
  if let Some(uid) = get_active_baidu_uid(db)? {
    db.with_conn(|conn| {
      conn.execute("DELETE FROM baidu_account_info WHERE uid = ?1", [uid.as_str()])?;
      conn.execute(
        "DELETE FROM baidu_account_credential WHERE baidu_uid = ?1",
        [uid.as_str()],
      )?;
      conn.execute(
        "DELETE FROM bilibili_baidu_binding WHERE baidu_uid = ?1",
        [uid.as_str()],
      )?;
      Ok(())
    })
    .map_err(|err| err.to_string())?;
  }
  let next_active = db
    .with_conn(|conn| {
      conn
        .query_row(
          "SELECT uid FROM baidu_account_info ORDER BY login_time DESC, update_time DESC LIMIT 1",
          [],
          |row| row.get::<_, String>(0),
        )
        .optional()
    })
    .map_err(|err| err.to_string())?;
  set_active_baidu_uid(db, next_active.as_deref())?;
  Ok(())
}

pub fn switch_baidu_account(db: &Db, baidu_uid: &str) -> Result<Option<BaiduLoginInfo>, String> {
  let trimmed_uid = baidu_uid.trim();
  if trimmed_uid.is_empty() {
    return Ok(None);
  }
  set_active_baidu_uid(db, Some(trimmed_uid))?;
  if let Some(credential) = load_baidu_login_credential_by_uid(db, trimmed_uid)? {
    let settings = load_baidu_sync_settings(db)?;
    let exec_path = resolve_baidu_exec_path(&settings.exec_path);
    let _ = relogin_with_credential(db, &exec_path, &credential);
  }
  load_baidu_login_info_by_uid(db, trimmed_uid)
}

pub fn logout_baidu_by_uid(db: &Db, baidu_uid: &str) -> Result<(), String> {
  let trimmed_uid = baidu_uid.trim();
  if trimmed_uid.is_empty() {
    return Ok(());
  }
  let active_uid = get_active_baidu_uid(db)?;
  if active_uid.as_deref() == Some(trimmed_uid) {
    return logout_baidu(db);
  }
  db.with_conn(|conn| {
    conn.execute("DELETE FROM baidu_account_info WHERE uid = ?1", [trimmed_uid])?;
    conn.execute(
      "DELETE FROM baidu_account_credential WHERE baidu_uid = ?1",
      [trimmed_uid],
    )?;
    conn.execute(
      "DELETE FROM bilibili_baidu_binding WHERE baidu_uid = ?1",
      [trimmed_uid],
    )?;
    Ok(())
  })
  .map_err(|err| err.to_string())
}

fn load_baidu_login_credential(db: &Db) -> Result<Option<BaiduLoginCredential>, String> {
  let Some(uid) = get_active_baidu_uid(db)? else {
    return Ok(None);
  };
  load_baidu_login_credential_by_uid(db, &uid)
}

fn load_baidu_login_credential_by_uid(
  db: &Db,
  baidu_uid: &str,
) -> Result<Option<BaiduLoginCredential>, String> {
  db.with_conn(|conn| {
    conn
      .query_row(
        "SELECT baidu_uid, login_type, cookie, bduss, stoken, last_attempt_time, last_attempt_error \
         FROM baidu_account_credential WHERE baidu_uid = ?1",
        [baidu_uid],
        |row| {
          Ok(BaiduLoginCredential {
            baidu_uid: row.get(0)?,
            login_type: row.get(1)?,
            cookie: row.get(2)?,
            bduss: row.get(3)?,
            stoken: row.get(4)?,
            last_attempt_time: row.get(5)?,
            last_attempt_error: row.get(6)?,
          })
        },
      )
      .optional()
  })
  .map_err(|err| err.to_string())
}

fn upsert_baidu_login_credential(db: &Db, credential: &BaiduLoginCredential) -> Result<(), String> {
  let now = now_rfc3339();
  db.with_conn(|conn| {
    conn.execute(
      "INSERT INTO baidu_account_credential \
       (baidu_uid, login_type, cookie, bduss, stoken, last_attempt_time, last_attempt_error, create_time, update_time) \
       VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
       ON CONFLICT(baidu_uid) DO UPDATE SET \
         login_type = excluded.login_type, \
         cookie = excluded.cookie, \
         bduss = excluded.bduss, \
         stoken = excluded.stoken, \
         last_attempt_time = excluded.last_attempt_time, \
         last_attempt_error = excluded.last_attempt_error, \
         update_time = excluded.update_time",
      (
        &credential.baidu_uid,
        &credential.login_type,
        &credential.cookie,
        &credential.bduss,
        &credential.stoken,
        &credential.last_attempt_time,
        &credential.last_attempt_error,
        &now,
        &now,
      ),
    )?;
    Ok(())
  })
  .map_err(|err| err.to_string())
}

fn update_baidu_login_credential_attempt(
  db: &Db,
  err: Option<&String>,
) -> Result<(), String> {
  let mut credential = match load_baidu_login_credential(db)? {
    Some(value) => value,
    None => return Ok(()),
  };
  credential.last_attempt_time = Some(now_rfc3339());
  credential.last_attempt_error = err.map(|value| value.to_string());
  upsert_baidu_login_credential(db, &credential)
}

fn should_attempt_relogin(credential: &BaiduLoginCredential, now: &str) -> bool {
  let Some(last_attempt) = credential.last_attempt_time.as_deref() else {
    return true;
  };
  let Some(last) = parse_rfc3339(last_attempt) else {
    return true;
  };
  let Some(current) = parse_rfc3339(now) else {
    return true;
  };
  (current - last).num_seconds() >= 600
}

fn relogin_with_credential(
  db: &Db,
  exec_path: &Path,
  credential: &BaiduLoginCredential,
) -> Result<BaiduLoginInfo, String> {
  match credential.login_type.as_str() {
    "cookie" => {
      let cookie = credential.cookie.as_deref().unwrap_or("");
      let cookie = normalize_baidu_cookie(cookie)?;
      run_baidu_pcs_command(
        exec_path,
        &["login".to_string(), format!("-cookies={}", cookie)],
      )?;
    }
    "bduss" => {
      let bduss = credential.bduss.as_deref().unwrap_or("");
      let bduss = normalize_baidu_token(bduss, "BDUSS")?;
      let stoken = credential.stoken.as_deref().unwrap_or("");
      let stoken = normalize_baidu_token_optional(stoken, "STOKEN")?;
      let mut args = vec!["login".to_string(), format!("-bduss={}", bduss)];
      if let Some(stoken_value) = stoken.as_deref() {
        args.push(format!("-stoken={}", stoken_value));
      }
      run_baidu_pcs_command(exec_path, &args)?;
    }
    _ => return Err("未知的登录类型".to_string()),
  }
  let info = check_baidu_login_internal(db, false)?;
  if info.status != "LOGGED_IN" {
    return Err("自动重登失败".to_string());
  }
  let mut next = info.clone();
  next.login_type = Some(credential.login_type.clone());
  if next.login_time.is_none() {
    next.login_time = Some(now_rfc3339());
  }
  upsert_baidu_login_info(db, &next)?;
  Ok(next)
}

fn parse_rfc3339(value: &str) -> Option<DateTime<Utc>> {
  DateTime::parse_from_rfc3339(value)
    .ok()
    .map(|value| value.with_timezone(&Utc))
}

async fn run_baidu_sync_task(
  context: BaiduSyncContext,
  settings: BaiduSyncSettings,
  task: BaiduSyncTask,
) -> Result<(), String> {
  update_baidu_sync_status(context.db.as_ref(), task.id, "UPLOADING", 0.0, None)?;
  let exec_path = resolve_baidu_exec_path(&settings.exec_path);
  let policy = normalize_baidu_upload_policy(task.policy.as_deref().or(Some(&settings.policy)))
    .unwrap_or_else(|| "overwrite".to_string());
  append_log(
    context.app_log_path.as_ref(),
    &format!(
      "baidu_sync_task_start id={} local={} remote_dir={} remote_name={} policy={}",
      task.id, task.local_path, task.remote_dir, task.remote_name, policy
    ),
  );
  let upload_with_args = |args: &[String]| {
    run_baidu_pcs_upload(&exec_path, args, |progress| {
      let _ = update_baidu_sync_progress(context.db.as_ref(), task.id, progress);
    })
  };
  let primary_args = vec![
    "upload".to_string(),
    format!("-policy={}", policy),
    "-p=4".to_string(),
    "-l=1".to_string(),
    task.local_path.clone(),
    task.remote_dir.clone(),
  ];
  let upload_result = match upload_with_args(&primary_args) {
    Ok(output) => Ok(output),
    Err(err) if is_baidu_upload_parallel_limit_error(&err) => {
      append_log(
        context.app_log_path.as_ref(),
        &format!(
          "baidu_sync_task_retry_limited id={} err={} fallback=p1_l1_norapid",
          task.id, err
        ),
      );
      let fallback_args = vec![
        "upload".to_string(),
        format!("-policy={}", policy),
        "-p=1".to_string(),
        "-l=1".to_string(),
        "--norapid".to_string(),
        task.local_path.clone(),
        task.remote_dir.clone(),
      ];
      upload_with_args(&fallback_args)
    }
    Err(err) => Err(err),
  };
  match upload_result {
    Ok(output) => {
      let local_name = Path::new(&task.local_path)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_string();
      let mut remote_path = join_baidu_path(&task.remote_dir, &local_name);
      if task.remote_name != local_name {
        let target_path = join_baidu_path(&task.remote_dir, &task.remote_name);
        let _ = run_baidu_pcs_command(
          &exec_path,
          &["mv".to_string(), remote_path.clone(), target_path.clone()],
        );
        remote_path = target_path;
      }
      append_log(
        context.app_log_path.as_ref(),
        &format!("baidu_sync_task_uploaded id={} remote={}", task.id, remote_path),
      );
      let meta = run_baidu_pcs_command(&exec_path, &["meta".to_string(), remote_path.clone()]);
      match meta {
        Ok(meta_output) => {
          let size = parse_meta_size(&meta_output.stdout).unwrap_or(0);
          if size == 0 {
            let err = "上传后文件大小为0";
            append_log(
              context.app_log_path.as_ref(),
              &format!("baidu_sync_task_error id={} err={}", task.id, err),
            );
            return handle_baidu_sync_failure(context.db.as_ref(), task, settings.retry, err);
          }
        }
        Err(err) => {
          append_log(
            context.app_log_path.as_ref(),
            &format!("baidu_sync_task_error id={} err={}", task.id, err),
          );
          return handle_baidu_sync_failure(context.db.as_ref(), task, settings.retry, &err);
        }
      }
      update_baidu_sync_status(context.db.as_ref(), task.id, "SUCCESS", 100.0, None)?;
      if task.source_type == "submission_merged" {
        if let Some(task_id) = task.source_id.as_deref().map(|value| value.trim()).filter(|value| !value.is_empty()) {
          if let Err(err) = bind_submission_merged_remote(
            context.db.as_ref(),
            task_id,
            &task.local_path,
            &task.remote_dir,
            &task.remote_name,
            task.baidu_uid.as_deref(),
          ) {
            append_log(
              context.app_log_path.as_ref(),
              &format!(
                "baidu_sync_bind_merged_fail task_id={} err={}",
                task_id, err
              ),
            );
          }
        }
      }
      append_log(
        context.app_log_path.as_ref(),
        &format!("baidu_sync_task_ok id={} output={}", task.id, output.stdout.len()),
      );
      Ok(())
    }
    Err(err) => {
      append_log(
        context.app_log_path.as_ref(),
        &format!("baidu_sync_task_error id={} err={}", task.id, err),
      );
      handle_baidu_sync_failure(context.db.as_ref(), task, settings.retry, &err)
    }
  }
}

fn handle_baidu_sync_failure(
  db: &Db,
  task: BaiduSyncTask,
  max_retry: i64,
  err: &str,
) -> Result<(), String> {
  let next_retry = task.retry_count + 1;
  if next_retry <= max_retry {
    let now = now_rfc3339();
    db.with_conn(|conn| {
      conn.execute(
        "UPDATE baidu_sync_task SET status = 'PENDING', progress = 0.0, retry_count = ?1, error = ?2, updated_at = ?3 WHERE id = ?4",
        (next_retry, err, &now, task.id),
      )?;
      Ok(())
    })
    .map_err(|err| err.to_string())?;
    Ok(())
  } else {
    update_baidu_sync_status(db, task.id, "FAILED", 0.0, Some(err.to_string()))
  }
}

fn update_baidu_sync_status(
  db: &Db,
  task_id: i64,
  status: &str,
  progress: f64,
  error: Option<String>,
) -> Result<(), String> {
  let now = now_rfc3339();
  db.with_conn(|conn| {
    conn.execute(
      "UPDATE baidu_sync_task SET status = ?1, progress = ?2, error = ?3, updated_at = ?4 WHERE id = ?5",
      (status, progress, error.as_deref(), &now, task_id),
    )?;
    Ok(())
  })
  .map_err(|err| err.to_string())
}

fn update_baidu_sync_progress(db: &Db, task_id: i64, progress: f64) -> Result<(), String> {
  let now = now_rfc3339();
  db.with_conn(|conn| {
    conn.execute(
      "UPDATE baidu_sync_task SET progress = ?1, updated_at = ?2 WHERE id = ?3",
      (progress, &now, task_id),
    )?;
    Ok(())
  })
  .map_err(|err| err.to_string())
}

fn insert_baidu_sync_task(
  db: &Db,
  source_type: &str,
  source_id: Option<String>,
  baidu_uid: Option<String>,
  source_title: Option<String>,
  local_path: &str,
  remote_dir: &str,
  remote_name: &str,
  policy: &str,
) -> Result<(), String> {
  let now = now_rfc3339();
  let stored_local_path = to_stored_local_path(db, local_path);
  db.with_conn(|conn| {
    conn.execute(
      "INSERT INTO baidu_sync_task (source_type, source_id, baidu_uid, source_title, local_path, remote_dir, remote_name, status, progress, error, retry_count, policy, created_at, updated_at) \
       VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'PENDING', 0.0, NULL, 0, ?8, ?9, ?10)",
      (
        source_type,
        source_id.as_deref(),
        baidu_uid.as_deref(),
        source_title.as_deref(),
        stored_local_path.as_str(),
        remote_dir,
        remote_name,
        policy,
        &now,
        &now,
      ),
    )?;
    Ok(())
  })
  .map_err(|err| err.to_string())
}

fn bind_submission_merged_remote(
  db: &Db,
  task_id: &str,
  local_path: &str,
  remote_dir: &str,
  remote_name: &str,
  baidu_uid: Option<&str>,
) -> Result<(), String> {
  let now = now_rfc3339();
  let stored_local_path = to_stored_local_path(db, local_path);
  let updated = db
    .with_conn(|conn| {
      conn.execute(
        "UPDATE merged_video SET remote_dir = ?1, remote_name = ?2, baidu_uid = ?3, update_time = ?4 \
         WHERE task_id = ?5 AND video_path = ?6",
        (
          remote_dir,
          remote_name,
          baidu_uid,
          &now,
          task_id,
          stored_local_path.as_str(),
        ),
      )
    })
    .map_err(|err| err.to_string())?;
  if updated == 0 {
    return Err("未找到合并视频记录".to_string());
  }
  Ok(())
}

fn load_next_pending_task(db: &Db) -> Result<Option<BaiduSyncTask>, String> {
  let now = now_rfc3339();
  let storage_prefix = load_local_path_prefix(db);
  db.with_conn(|conn| {
    let mut stmt = conn.prepare(
      "SELECT id, source_type, source_id, baidu_uid, local_path, remote_dir, remote_name, retry_count, policy \
       FROM baidu_sync_task WHERE status = 'PENDING' ORDER BY created_at ASC LIMIT 1",
    )?;
    let mut rows = stmt.query([])?;
    if let Some(row) = rows.next()? {
      let task_id: i64 = row.get(0)?;
      conn.execute(
        "UPDATE baidu_sync_task SET status = 'UPLOADING', updated_at = ?1 WHERE id = ?2 AND status = 'PENDING'",
        (&now, task_id),
      )?;
      let task = BaiduSyncTask {
        id: task_id,
        source_type: row.get(1)?,
        source_id: row.get(2)?,
        baidu_uid: row.get(3)?,
        local_path: to_absolute_local_path_with_prefix(
          storage_prefix.as_path(),
          row.get::<_, String>(4)?.as_str(),
        )
        .to_string_lossy()
        .to_string(),
        remote_dir: row.get(5)?,
        remote_name: row.get(6)?,
        retry_count: row.get(7)?,
        policy: row.get(8)?,
      };
      Ok(Some(task))
    } else {
      Ok(None)
    }
  })
  .map_err(|err| err.to_string())
}

fn map_baidu_sync_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<BaiduSyncTaskRecord> {
  Ok(BaiduSyncTaskRecord {
    id: row.get(0)?,
    source_type: row.get(1)?,
    source_id: row.get(2)?,
    baidu_uid: row.get(3)?,
    source_title: row.get(4)?,
    local_path: row.get(5)?,
    remote_dir: row.get(6)?,
    remote_name: row.get(7)?,
    status: row.get(8)?,
    progress: row.get(9)?,
    error: row.get(10)?,
    retry_count: row.get(11)?,
    policy: row.get(12)?,
    created_at: row.get(13)?,
    updated_at: row.get(14)?,
  })
}

fn read_setting(conn: &rusqlite::Connection, key: &str) -> Option<String> {
  conn
    .query_row("SELECT value FROM app_settings WHERE key = ?1", [key], |row| row.get(0))
    .ok()
}

fn upsert_setting(
  conn: &rusqlite::Connection,
  key: &str,
  value: &str,
  now: &str,
) -> rusqlite::Result<()> {
  conn.execute(
    "INSERT INTO app_settings (key, value, updated_at) VALUES (?1, ?2, ?3) \
     ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
    (key, value, now),
  )?;
  Ok(())
}

pub fn normalize_baidu_path(path: &str) -> String {
  let trimmed = path.trim();
  if trimmed.is_empty() {
    return "/".to_string();
  }
  let mut output = trimmed.replace('\\', "/");
  output = output.trim_end_matches('/').to_string();
  if !output.starts_with('/') {
    output = format!("/{}", output);
  }
  if output.is_empty() {
    "/".to_string()
  } else {
    output
  }
}

pub fn join_baidu_path(base: &str, segment: &str) -> String {
  let base = normalize_baidu_path(base);
  let segment = segment.trim().trim_matches('/');
  if segment.is_empty() {
    return base;
  }
  format!("{}/{}", base.trim_end_matches('/'), segment)
}

fn render_filename(
  template: Option<&str>,
  title: &str,
  date: &str,
  index: Option<i64>,
  fallback: &str,
) -> String {
  let raw = template.unwrap_or("").trim();
  if raw.is_empty() {
    return sanitize_filename(fallback);
  }
  let mut output = raw.to_string();
  output = output.replace("{{ title }}", title);
  output = output.replace("{{ date }}", date);
  if let Some(index) = index {
    output = output.replace("{{ index }}", &index.to_string());
    output = output.replace("{{ part }}", &index.to_string());
  }
  let trimmed = output.trim();
  if trimmed.is_empty() {
    sanitize_filename(fallback)
  } else {
    sanitize_filename(trimmed)
  }
}

fn load_room_baidu_sync_config(
  db: &Db,
  room_id: &str,
) -> Result<(bool, Option<String>), String> {
  db.with_conn(|conn| {
    let mut stmt = conn.prepare(
      "SELECT IFNULL(baidu_sync_enabled, 0), baidu_sync_path \
       FROM live_room_settings WHERE room_id = ?1",
    )?;
    let result = stmt.query_row([room_id], |row| {
      let enabled: i64 = row.get(0)?;
      let path: Option<String> = row.get(1)?;
      Ok((enabled != 0, path))
    });
    match result {
      Ok(value) => Ok(value),
      Err(rusqlite::Error::QueryReturnedNoRows) => Ok((false, None)),
      Err(err) => Err(err),
    }
  })
  .map_err(|err| err.to_string())
}

fn parse_baidu_ls_entries(output: &str, base_path: &str) -> Vec<BaiduRemoteEntry> {
  let mut entries = Vec::new();
  for line in output.lines() {
    let trimmed = strip_ansi(line).trim().to_string();
    let trimmed = trimmed.trim();
    if trimmed.is_empty() {
      continue;
    }
    if trimmed.starts_with("当前目录") || trimmed.starts_with("----") {
      continue;
    }
    if trimmed.contains("文件总数") || trimmed.contains("目录总数") || trimmed.contains("总:") {
      continue;
    }
    let (name, is_dir) = if trimmed.contains('|') {
      let columns: Vec<&str> = trimmed
        .split('|')
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .collect();
      if columns.is_empty() {
        continue;
      }
      let last = columns[columns.len() - 1];
      if last.contains("文件(目录)") {
        continue;
      }
      let is_dir = last.ends_with('/');
      (last.trim_end_matches('/').trim().to_string(), is_dir)
    } else {
      let last = extract_last_column(trimmed).unwrap_or(trimmed);
      let is_dir = last.ends_with('/');
      (last.trim_end_matches('/').trim().to_string(), is_dir)
    };
    if name.is_empty() {
      continue;
    }
    let path = join_baidu_path(base_path, &name);
    entries.push(BaiduRemoteEntry {
      name,
      path,
      is_dir,
    });
  }
  entries
}

fn extract_last_column(line: &str) -> Option<&str> {
  let bytes = line.as_bytes();
  let mut last_boundary = None;
  let mut i = 0;
  while i + 1 < bytes.len() {
    if bytes[i].is_ascii_whitespace() && bytes[i + 1].is_ascii_whitespace() {
      let mut j = i + 2;
      while j < bytes.len() && bytes[j].is_ascii_whitespace() {
        j += 1;
      }
      last_boundary = Some(j);
      i = j;
    } else {
      i += 1;
    }
  }
  match last_boundary {
    Some(idx) if idx < line.len() => Some(line[idx..].trim()),
    _ => None,
  }
}

fn parse_date(input: &str) -> Option<String> {
  if let Ok(value) = DateTime::parse_from_rfc3339(input) {
    return Some(value.format("%Y%m%d").to_string());
  }
  None
}

fn resolve_baidu_exec_path(custom: &str) -> PathBuf {
  if !custom.trim().is_empty() {
    return PathBuf::from(custom.trim());
  }
  resolve_baidu_pcs_path()
}

#[derive(Default)]
struct CommandOutput {
  stdout: String,
  stderr: String,
}

fn run_baidu_pcs_command(exec_path: &Path, args: &[String]) -> Result<CommandOutput, String> {
  let mut command = Command::new(exec_path);
  apply_no_window(&mut command);
  let output = command
    .args(args)
    .output()
    .map_err(|err| format!("BaiduPCS-Go 执行失败: {}", err))?;
  let stdout = String::from_utf8_lossy(&output.stdout).to_string();
  let stderr = String::from_utf8_lossy(&output.stderr).to_string();
  if output.status.success() {
    if stdout.contains("文件上传失败") {
      return Err(format!("上传失败: {}", stdout.trim()));
    }
    return Ok(CommandOutput { stdout, stderr });
  }
  Err(format!("BaiduPCS-Go 执行失败: {}", stderr.trim()))
}

fn run_baidu_pcs_download_with_hook<F>(
  exec_path: &Path,
  remote_path: &str,
  local_dir: &Path,
  on_spawn: F,
) -> Result<CommandOutput, String>
where
  F: FnOnce(Arc<Mutex<Child>>),
{
  let save_dir = local_dir.to_string_lossy().to_string();
  let mut command = Command::new(exec_path);
  apply_no_window(&mut command);
  let mut child = command
    .current_dir(local_dir)
    .args([
      "download".to_string(),
      "--saveto".to_string(),
      save_dir,
      remote_path.to_string(),
    ])
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .map_err(|err| format!("BaiduPCS-Go 执行失败: {}", err))?;

  let stdout = child.stdout.take();
  let stderr = child.stderr.take();
  let child_handle = Arc::new(Mutex::new(child));
  on_spawn(Arc::clone(&child_handle));

  let stdout_handle = stdout.map(|mut reader| {
    std::thread::spawn(move || {
      let mut buffer = String::new();
      let _ = reader.read_to_string(&mut buffer);
      buffer
    })
  });
  let stderr_handle = stderr.map(|mut reader| {
    std::thread::spawn(move || {
      let mut buffer = String::new();
      let _ = reader.read_to_string(&mut buffer);
      buffer
    })
  });

  let status = loop {
    let result = {
      let mut guard = child_handle
        .lock()
        .map_err(|_| "BaiduPCS-Go 进程锁失败".to_string())?;
      guard
        .try_wait()
        .map_err(|err| format!("BaiduPCS-Go 执行失败: {}", err))?
    };
    if let Some(status) = result {
      break status;
    }
    std::thread::sleep(std::time::Duration::from_millis(200));
  };

  let stdout = stdout_handle
    .and_then(|handle| handle.join().ok())
    .unwrap_or_default();
  let stderr = stderr_handle
    .and_then(|handle| handle.join().ok())
    .unwrap_or_default();

  if status.success() {
    return Ok(CommandOutput { stdout, stderr });
  }
  Err(format!("BaiduPCS-Go 执行失败: {}", stderr.trim()))
}

fn run_baidu_pcs_download(
  exec_path: &Path,
  remote_path: &str,
  local_dir: &Path,
) -> Result<CommandOutput, String> {
  run_baidu_pcs_download_with_hook(exec_path, remote_path, local_dir, |_| {})
}

fn run_baidu_pcs_upload<F>(
  exec_path: &Path,
  args: &[String],
  mut on_progress: F,
) -> Result<CommandOutput, String>
where
  F: FnMut(f64),
{
  let mut command = Command::new(exec_path);
  apply_no_window(&mut command);
  let mut child = command
    .args(args)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .map_err(|err| format!("BaiduPCS-Go 执行失败: {}", err))?;

  let mut stdout = child
    .stdout
    .take()
    .ok_or_else(|| "无法获取 BaiduPCS-Go stdout".to_string())?;
  let mut stderr = child
    .stderr
    .take()
    .ok_or_else(|| "无法获取 BaiduPCS-Go stderr".to_string())?;

  let (stderr_tx, stderr_rx) = std::sync::mpsc::channel();
  std::thread::spawn(move || {
    let mut buffer = String::new();
    let _ = stderr.read_to_string(&mut buffer);
    let _ = stderr_tx.send(buffer);
  });

  let mut stdout_buf = String::new();
  let mut pending: Vec<u8> = Vec::new();
  let mut chunk = [0u8; 4096];
  loop {
    let read_size = stdout
      .read(&mut chunk)
      .map_err(|err| format!("BaiduPCS-Go 执行失败: {}", err))?;
    if read_size == 0 {
      break;
    }
    let slice = &chunk[..read_size];
    stdout_buf.push_str(&String::from_utf8_lossy(slice));
    pending.extend_from_slice(slice);

    loop {
      let split_pos = pending
        .iter()
        .position(|value| *value == b'\n' || *value == b'\r');
      let Some(pos) = split_pos else { break; };
      let mut line_bytes: Vec<u8> = pending.drain(..=pos).collect();
      while matches!(line_bytes.last(), Some(b'\n' | b'\r')) {
        line_bytes.pop();
      }
      if line_bytes.is_empty() {
        continue;
      }
      let line = String::from_utf8_lossy(&line_bytes);
      if let Some(progress) = parse_progress_line(&line) {
        on_progress(progress);
      }
    }
  }
  if !pending.is_empty() {
    let line = String::from_utf8_lossy(&pending);
    if let Some(progress) = parse_progress_line(&line) {
      on_progress(progress);
    }
  }

  let status = child
    .wait()
    .map_err(|err| format!("BaiduPCS-Go 执行失败: {}", err))?;
  let stderr_output = stderr_rx.recv().unwrap_or_default();

  if status.success() {
    if stdout_buf.contains("文件上传失败") {
      return Err(format!("上传失败: {}", stdout_buf.trim()));
    }
    return Ok(CommandOutput {
      stdout: stdout_buf,
      stderr: stderr_output,
    });
  }
  Err(format!("BaiduPCS-Go 执行失败: {}", stderr_output.trim()))
}

fn is_baidu_not_found_error(err: &str) -> bool {
  let lower = err.to_lowercase();
  lower.contains("not found")
    || lower.contains("no such file")
    || err.contains("未找到")
    || err.contains("不存在")
}

fn is_baidu_upload_parallel_limit_error(err: &str) -> bool {
  err.contains("当前上传单个文件最大并发量")
    || err.contains("最大同时上传文件数")
    || err.contains("文件上传失败")
}

fn find_file_by_name(base_dir: &Path, file_name: &str) -> Option<PathBuf> {
  let entries = fs::read_dir(base_dir).ok()?;
  for entry in entries.flatten() {
    let path = entry.path();
    if path.is_dir() {
      if let Some(found) = find_file_by_name(&path, file_name) {
        return Some(found);
      }
    } else if path
      .file_name()
      .and_then(|value| value.to_str())
      .map(|value| value == file_name)
      .unwrap_or(false)
    {
      return Some(path);
    }
  }
  None
}

fn parse_progress_line(line: &str) -> Option<f64> {
  let cleaned = strip_ansi(line).replace('\r', " ").replace('\n', " ");
  let arrow_pos = cleaned.find('↑')?;
  let after = cleaned[arrow_pos + '↑'.len_utf8()..].trim_start();
  let size_part = after.split_whitespace().find(|value| value.contains('/'))?;
  let sizes: Vec<&str> = size_part.split('/').collect();
  if sizes.len() != 2 {
    return None;
  }
  let uploaded = parse_size(sizes[0])?;
  let total = parse_size(sizes[1])?;
  if total == 0 {
    return None;
  }
  let mut percent = (uploaded as f64 / total as f64) * 100.0;
  if percent > 99.0 {
    percent = 99.0;
  }
  Some(percent)
}

fn strip_ansi(input: &str) -> String {
  let mut output = String::with_capacity(input.len());
  let mut chars = input.chars().peekable();
  while let Some(ch) = chars.next() {
    if ch == '\u{1b}' {
      if matches!(chars.peek(), Some('[')) {
        chars.next();
        while let Some(value) = chars.next() {
          if value.is_ascii_alphabetic() {
            break;
          }
        }
      }
      continue;
    }
    output.push(ch);
  }
  output
}

fn normalize_baidu_upload_policy(value: Option<&str>) -> Option<String> {
  match value.map(|value| value.trim()).filter(|value| !value.is_empty())? {
    "skip" => Some("skip".to_string()),
    "overwrite" => Some("overwrite".to_string()),
    "rsync" => Some("rsync".to_string()),
    _ => None,
  }
}

fn parse_size(value: &str) -> Option<u64> {
  let value = value.trim();
  if value.is_empty() {
    return None;
  }
  let mut digits = String::new();
  let mut unit = String::new();
  for ch in value.chars() {
    if ch.is_ascii_digit() || ch == '.' {
      digits.push(ch);
    } else {
      unit.push(ch);
    }
  }
  let number: f64 = digits.parse().ok()?;
  let bytes = match unit.as_str() {
    "KB" => number * 1024.0,
    "MB" => number * 1024.0 * 1024.0,
    "GB" => number * 1024.0 * 1024.0 * 1024.0,
    "TB" => number * 1024.0 * 1024.0 * 1024.0 * 1024.0,
    "PB" => number * 1024.0 * 1024.0 * 1024.0 * 1024.0 * 1024.0,
    "B" => number,
    _ => return None,
  };
  Some(bytes.round() as u64)
}

fn parse_meta_size(output: &str) -> Option<u64> {
  for line in output.lines() {
    if line.contains("文件大小") {
      let parts: Vec<&str> = line.split_whitespace().collect();
      if parts.len() >= 2 {
        if let Ok(value) = parts[1].trim().trim_end_matches(',').parse::<u64>() {
          return Some(value);
        }
      }
    }
  }
  None
}

fn parse_who_output(output: &str) -> (bool, Option<String>, Option<String>) {
  if output.contains("请先登录") || output.contains("uid: 0") {
    return (false, None, None);
  }
  let uid = extract_between(output, "uid:", ",");
  let username = extract_between(output, "用户名:", ",");
  (uid.is_some(), uid, username)
}

fn extract_between(content: &str, start: &str, end: &str) -> Option<String> {
  let start_index = content.find(start)?;
  let after_start = &content[start_index + start.len()..];
  let end_index = after_start.find(end)?;
  let value = after_start[..end_index].trim();
  if value.is_empty() {
    None
  } else {
    Some(value.to_string())
  }
}
