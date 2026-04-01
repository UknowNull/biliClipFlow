use std::fs;
use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration as StdDuration, Instant};

use chrono::{DateTime, Utc};
use futures_util::FutureExt;
use reqwest::blocking::Client;
use reqwest::header::{COOKIE, REFERER, USER_AGENT};
use rusqlite::OptionalExtension;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::task;
use tokio::time::{sleep, Duration};

use crate::commands::settings::DEFAULT_BAIDU_MAX_PARALLEL;
use crate::config::{default_download_dir, resolve_baidu_pcs_path};
use crate::db::Db;
use crate::path_store::{
    load_local_path_prefix, to_absolute_local_path_opt_with_prefix,
    to_absolute_local_path_with_prefix, to_stored_local_path,
};
use crate::utils::{append_log, apply_no_window, now_rfc3339, sanitize_filename};

const BAIDU_SYNC_UPLOAD_TIMEOUT: StdDuration = StdDuration::from_secs(12 * 60 * 60);
const BAIDU_SYNC_UPLOAD_IDLE_TIMEOUT: StdDuration = StdDuration::from_secs(30 * 60);
const BAIDU_SYNC_STALE_RECOVER_AFTER: StdDuration = StdDuration::from_secs(35 * 60);
const BAIDU_SYNC_PIPE_DRAIN_TIMEOUT: StdDuration = StdDuration::from_secs(3);

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
struct BaiduHttpUserInfo {
    uid: String,
    username: Option<String>,
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
    pub size: u64,
}

#[derive(Clone)]
pub struct BaiduSyncContext {
    pub db: Arc<Db>,
    pub app_log_path: Arc<PathBuf>,
    pub runtime: Arc<BaiduSyncRuntime>,
}

pub struct BaiduSyncRuntime {
    active_task_ids: Mutex<HashSet<i64>>,
}

impl BaiduSyncRuntime {
    pub fn new() -> Self {
        Self {
            active_task_ids: Mutex::new(HashSet::new()),
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
        let target_path =
            read_setting(conn, "baidu_sync_target_path").unwrap_or_else(|| "/录播".to_string());
        let policy =
            read_setting(conn, "baidu_sync_policy").unwrap_or_else(|| "overwrite".to_string());
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
        "SELECT info.status, info.uid, info.username, info.login_type, info.login_time, info.last_check_time \
         FROM baidu_account_info info \
         INNER JOIN baidu_account_credential credential ON credential.baidu_uid = info.uid \
         ORDER BY info.login_time DESC, info.update_time DESC LIMIT 1",
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
    let Some(uid) = info
        .uid
        .as_deref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    else {
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
        conn.query_row(
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
      "SELECT info.uid, info.status, info.username, info.login_type, info.login_time, info.last_check_time \
       FROM baidu_account_info info \
       INNER JOIN baidu_account_credential credential ON credential.baidu_uid = info.uid \
       ORDER BY info.login_time DESC, info.update_time DESC",
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
        upsert_setting(
            conn,
            "baidu_sync_concurrency",
            &settings.concurrency.to_string(),
            &now,
        )?;
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

pub fn list_baidu_remote_dirs(db: &Db, path: &str) -> Result<Vec<BaiduRemoteDir>, String> {
    let entries = list_baidu_remote_entries(db, path)?;
    Ok(entries
        .into_iter()
        .filter(|item| item.is_dir)
        .map(|item| BaiduRemoteDir {
            name: item.name,
            path: item.path,
        })
        .collect())
}

pub fn list_baidu_remote_entries(db: &Db, path: &str) -> Result<Vec<BaiduRemoteEntry>, String> {
    let target_path = normalize_baidu_path(path);
    let credential =
        load_baidu_login_credential(db)?.ok_or_else(|| "请先登录网盘账号".to_string())?;
    fetch_baidu_remote_entries_via_http(&credential, &target_path)
}

pub fn check_baidu_remote_file_exists(db: &Db, remote_path: &str) -> Result<bool, String> {
    let target_path = normalize_baidu_path(remote_path);
    Ok(find_baidu_remote_entry(db, &target_path)?.is_some())
}

pub fn fetch_baidu_remote_file_size(db: &Db, remote_path: &str) -> Result<u64, String> {
    let target_path = normalize_baidu_path(remote_path);
    let entry = find_baidu_remote_entry(db, &target_path)?
        .ok_or_else(|| format!("百度网盘文件不存在: {}", target_path))?;
    Ok(entry.size)
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
        Ok(value
            .and_then(|item| item.parse::<i64>().ok())
            .unwrap_or(DEFAULT_BAIDU_MAX_PARALLEL)
            .clamp(1, 100))
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
    let expected_remote_size = fetch_baidu_remote_file_size(db, &target_path).unwrap_or(0);
    let command_output =
        run_baidu_pcs_download_with_hook(&exec_path, &target_path, local_dir, on_spawn)?;
    if let Some(detail) = detect_baidu_download_failure(&command_output) {
        return Err(format!("BaiduPCS-Go 下载失败: {}", detail));
    }
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
        fs::rename(&found, local_path).map_err(|err| format!("重命名下载文件失败: {}", err))?;
        return Ok(local_path.to_path_buf());
    }
    if expected_remote_size > 0
        && matches!(parse_download_total_size(&command_output.stdout), Some(0))
    {
        return Err(format!(
            "BaiduPCS-Go 未解析出可下载文件: remote={} expected_size={} command={}",
            target_path,
            expected_remote_size,
            summarize_command_output_pair(&command_output.stdout, &command_output.stderr)
        ));
    }
    Err(format!(
        "网盘文件下载完成但未找到本地文件: remote={} local={} dir_snapshot={} command={}",
        target_path,
        local_path.display(),
        summarize_local_dir_snapshot(local_dir, 20),
        summarize_command_output_pair(&command_output.stdout, &command_output.stderr)
    ))
}

pub fn create_baidu_remote_dir(
    db: &Db,
    parent_path: &str,
    name: &str,
) -> Result<BaiduRemoteDir, String> {
    let credential = load_baidu_login_credential(db)?
        .ok_or_else(|| "百度网盘未登录或当前账号缺少可用凭证".to_string())?;
    let safe_name = sanitize_filename(name.trim());
    if safe_name.is_empty() {
        return Err("目录名称不能为空".to_string());
    }
    let base_path = normalize_baidu_path(parent_path);
    let full_path = join_baidu_path(&base_path, &safe_name);
    create_baidu_remote_dir_via_http(&credential, &full_path)?;
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
    let credential = load_baidu_login_credential(db)?
        .ok_or_else(|| "百度网盘未登录或当前账号缺少可用凭证".to_string())?;
    let normalized_from = normalize_baidu_path(from_path);
    if normalized_from == "/" {
        return Err("无法重命名根目录".to_string());
    }
    let safe_name = sanitize_filename(name.trim());
    if safe_name.is_empty() {
        return Err("目录名称不能为空".to_string());
    }
    let parent_path = {
        let mut segments: Vec<&str> = normalized_from
            .split('/')
            .filter(|value| !value.is_empty())
            .collect();
        segments.pop();
        if segments.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", segments.join("/"))
        }
    };
    let target_path = join_baidu_path(&parent_path, &safe_name);
    if target_path != normalized_from {
        rename_baidu_remote_dir_via_http(&credential, &normalized_from, &safe_name)?;
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

pub fn enqueue_submission_sync(db: &Db, app_log_path: &Path, task_id: &str) -> Result<(), String> {
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
            &format!(
                "baidu_sync_enqueue_skip task_id={} reason=disabled",
                task_id
            ),
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
                &format!(
                    "baidu_sync_enqueue_skip task_id={} reason=missing_merged_path",
                    task_id
                ),
            );
            return Ok(());
        }
    };
    let local_name = merged
        .1
        .or_else(|| {
            Path::new(&local_path)
                .file_name()
                .and_then(|v| v.to_str())
                .map(|v| v.to_string())
        })
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
    ) {
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
    let now = now_rfc3339();
    let stored_local_path = to_stored_local_path(db, &local_path);
    let existing = db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT id, status FROM baidu_sync_task \
           WHERE source_type = 'submission_merged' AND source_id = ?1 \
             AND status IN ('PENDING', 'UPLOADING', 'PAUSED', 'FAILED', 'CANCELLED') \
           ORDER BY CASE status \
             WHEN 'UPLOADING' THEN 0 \
             WHEN 'PENDING' THEN 1 \
             WHEN 'PAUSED' THEN 2 \
             WHEN 'FAILED' THEN 3 \
             WHEN 'CANCELLED' THEN 4 \
             ELSE 5 END, id DESC \
           LIMIT 1",
                [task_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
        })
        .map_err(|err| err.to_string())?;
    if let Some((existing_id, existing_status)) = existing {
        match existing_status.as_str() {
            "PENDING" => {
                db.with_conn(|conn| {
          conn.execute(
            "UPDATE baidu_sync_task SET baidu_uid = ?1, source_title = ?2, local_path = ?3, remote_dir = ?4, remote_name = ?5, policy = ?6, updated_at = ?7 \
             WHERE id = ?8",
            (
              task_baidu_uid.as_deref(),
              Some(title.as_str()),
              stored_local_path.as_str(),
              &remote_dir,
              &remote_name,
              &settings.policy,
              &now,
              existing_id,
            ),
          )?;
          Ok(())
        })
        .map_err(|err| err.to_string())?;
                append_log(
                    app_log_path,
                    &format!(
            "baidu_sync_enqueue_submission_skip task_id={} existing_id={} status=PENDING",
            task_id, existing_id
          ),
                );
                return Ok(());
            }
            "UPLOADING" => {
                append_log(
                    app_log_path,
                    &format!(
            "baidu_sync_enqueue_submission_skip task_id={} existing_id={} status=UPLOADING",
            task_id, existing_id
          ),
                );
                return Ok(());
            }
            "PAUSED" => {
                db.with_conn(|conn| {
          conn.execute(
            "UPDATE baidu_sync_task SET baidu_uid = ?1, source_title = ?2, local_path = ?3, remote_dir = ?4, remote_name = ?5, policy = ?6, updated_at = ?7 \
             WHERE id = ?8",
            (
              task_baidu_uid.as_deref(),
              Some(title.as_str()),
              stored_local_path.as_str(),
              &remote_dir,
              &remote_name,
              &settings.policy,
              &now,
              existing_id,
            ),
          )?;
          Ok(())
        })
        .map_err(|err| err.to_string())?;
                append_log(
                    app_log_path,
                    &format!(
            "baidu_sync_enqueue_submission_skip task_id={} existing_id={} status=PAUSED",
            task_id, existing_id
          ),
                );
                return Ok(());
            }
            "FAILED" | "CANCELLED" => {
                db.with_conn(|conn| {
          conn.execute(
            "UPDATE baidu_sync_task SET baidu_uid = ?1, source_title = ?2, local_path = ?3, remote_dir = ?4, remote_name = ?5, status = 'PENDING', progress = 0.0, error = NULL, retry_count = 0, policy = ?6, updated_at = ?7 \
             WHERE id = ?8",
            (
              task_baidu_uid.as_deref(),
              Some(title.as_str()),
              stored_local_path.as_str(),
              &remote_dir,
              &remote_name,
              &settings.policy,
              &now,
              existing_id,
            ),
          )?;
          Ok(())
        })
        .map_err(|err| err.to_string())?;
                append_log(
                    app_log_path,
                    &format!(
                        "baidu_sync_enqueue_submission_reuse task_id={} existing_id={} status={}",
                        task_id, existing_id, existing_status
                    ),
                );
                return Ok(());
            }
            _ => {}
        }
    }
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

pub fn enqueue_live_sync(db: &Db, app_log_path: &Path, record_id: i64) -> Result<(), String> {
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
            &format!(
                "baidu_sync_enqueue_skip record_id={} reason=missing_record_path",
                record_id
            ),
        );
        return Ok(());
    }
    let local_name = Path::new(&file_path)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("record.mp4")
        .to_string();
    let live_date =
        parse_date(&start_time).unwrap_or_else(|| Utc::now().format("%Y%m%d").to_string());
    let (sync_enabled, room_path) =
        load_room_baidu_sync_config(db, &room_id).unwrap_or((false, None));
    if !sync_enabled {
        append_log(
            app_log_path,
            &format!(
                "baidu_sync_enqueue_skip record_id={} reason=disabled",
                record_id
            ),
        );
        return Ok(());
    }
    let base_path = match room_path {
        Some(value) if !value.trim().is_empty() => normalize_baidu_path(&value),
        _ => {
            append_log(
                app_log_path,
                &format!(
                    "baidu_sync_enqueue_skip record_id={} reason=missing_path",
                    record_id
                ),
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
            let result =
                std::panic::AssertUnwindSafe(run_baidu_sync_scheduler_iteration(context.clone()))
                    .catch_unwind()
                    .await;
            match result {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    append_log(
                        context.app_log_path.as_ref(),
                        &format!("baidu_sync_loop_error err={}", err),
                    );
                    sleep(Duration::from_secs(3)).await;
                }
                Err(payload) => {
                    append_log(
                        context.app_log_path.as_ref(),
                        &format!(
                            "baidu_sync_loop_panic panic={}",
                            describe_panic_payload(payload.as_ref())
                        ),
                    );
                    sleep(Duration::from_secs(3)).await;
                }
            }
        }
    });
}

async fn run_baidu_sync_scheduler_iteration(context: BaiduSyncContext) -> Result<(), String> {
    let settings = match load_baidu_sync_settings(context.db.as_ref()) {
        Ok(value) => value,
        Err(_) => {
            sleep(Duration::from_secs(10)).await;
            return Ok(());
        }
    };
    run_recover_stale_baidu_sync_tasks(&context).await;
    let mut launched = 0;
    loop {
        let active = reconcile_runtime_active_tasks(&context);
        if active >= settings.concurrency {
            break;
        }
        let task = match load_next_pending_task(context.db.as_ref()) {
            Ok(Some(task)) => task,
            Ok(None) => break,
            Err(err) => {
                append_log(
                    context.app_log_path.as_ref(),
                    &format!("baidu_sync_load_next_pending_fail err={}", err),
                );
                break;
            }
        };
        if let Ok(mut guard) = context.runtime.active_task_ids.lock() {
            guard.insert(task.id);
        }
        let task_context = context.clone();
        let runtime = Arc::clone(&context.runtime);
        let app_log_path = Arc::clone(&context.app_log_path);
        let settings_clone = settings.clone();
        let task_id = task.id;
        tauri::async_runtime::spawn(async move {
            let result =
                std::panic::AssertUnwindSafe(run_baidu_sync_task(task_context, settings_clone, task))
                    .catch_unwind()
                    .await;
            if let Ok(mut guard) = runtime.active_task_ids.lock() {
                guard.remove(&task_id);
            }
            match result {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    append_log(
                        app_log_path.as_ref(),
                        &format!("baidu_sync_task_fail id={} err={}", task_id, err),
                    );
                }
                Err(payload) => {
                    append_log(
                        app_log_path.as_ref(),
                        &format!(
                            "baidu_sync_task_panic id={} panic={}",
                            task_id,
                            describe_panic_payload(payload.as_ref())
                        ),
                    );
                }
            }
        });
        launched += 1;
        if launched >= settings.concurrency {
            break;
        }
    }
    sleep(Duration::from_secs(3)).await;
    Ok(())
}

async fn run_recover_stale_baidu_sync_tasks(context: &BaiduSyncContext) {
    let db = Arc::clone(&context.db);
    let app_log_path = Arc::clone(&context.app_log_path);
    let result = task::spawn_blocking(move || {
        recover_stale_baidu_sync_tasks(db.as_ref(), app_log_path.as_ref());
    })
    .await;
    if let Err(err) = result {
        append_log(
            context.app_log_path.as_ref(),
            &format!("baidu_sync_recover_join_fail err={}", err),
        );
    }
}

fn describe_panic_payload(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

fn reconcile_runtime_active_tasks(context: &BaiduSyncContext) -> i64 {
    let mut removed_ids = Vec::new();
    let active = if let Ok(mut guard) = context.runtime.active_task_ids.lock() {
        guard.retain(|task_id| match is_baidu_sync_task_uploading(context.db.as_ref(), *task_id) {
            Ok(true) => true,
            Ok(false) => {
                removed_ids.push(*task_id);
                false
            }
            Err(_) => true,
        });
        guard.len() as i64
    } else {
        0
    };
    for task_id in removed_ids {
        append_log(
            context.app_log_path.as_ref(),
            &format!("baidu_sync_runtime_prune task_id={} reason=status_changed", task_id),
        );
    }
    active
}

fn is_baidu_sync_task_uploading(db: &Db, task_id: i64) -> Result<bool, String> {
    db.with_conn(|conn| {
        let status = conn
            .query_row(
                "SELECT status FROM baidu_sync_task WHERE id = ?1",
                [task_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        Ok(matches!(status.as_deref(), Some("UPLOADING")))
    })
    .map_err(|err| err.to_string())
}

pub fn recover_baidu_sync_tasks(db: &Db, app_log_path: &Path) {
    recover_baidu_sync_tasks_internal(db, app_log_path, None, "startup");
}

fn recover_stale_baidu_sync_tasks(db: &Db, app_log_path: &Path) {
    let Ok(stale_window) = chrono::Duration::from_std(BAIDU_SYNC_STALE_RECOVER_AFTER) else {
        return;
    };
    let stale_before = Utc::now() - stale_window;
    recover_baidu_sync_tasks_internal(db, app_log_path, Some(stale_before), "stale");
}

fn recover_baidu_sync_tasks_internal(
    db: &Db,
    app_log_path: &Path,
    stale_before: Option<DateTime<Utc>>,
    reason: &str,
) {
    let storage_prefix = load_local_path_prefix(db);
    let tasks = db.with_conn(|conn| {
    let mut stmt = conn.prepare(
      "SELECT id, source_type, source_id, baidu_uid, local_path, remote_dir, remote_name, retry_count, policy, updated_at \
       FROM baidu_sync_task WHERE status = 'UPLOADING' ORDER BY id ASC",
    )?;
    let mut rows = stmt.query([])?;
    let mut list = Vec::new();
    while let Some(row) = rows.next()? {
      let updated_at: String = row.get(9)?;
      if let Some(stale_before) = stale_before {
        if let Some(updated_time) = parse_rfc3339(&updated_at) {
          if updated_time > stale_before {
            continue;
          }
        }
      }
      list.push(BaiduSyncTask {
        id: row.get(0)?,
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
      });
    }
    Ok(list)
  });
    let Ok(tasks) = tasks else {
        return;
    };
    for task in tasks {
        if let Err(err) = recover_single_baidu_sync_task(db, app_log_path, &task, reason) {
            append_log(
                app_log_path,
                &format!("baidu_sync_recover_unhandled id={} reason={} err={}", task.id, reason, err),
            );
        }
    }
    append_log(app_log_path, &format!("baidu_sync_recover_ok reason={}", reason));
}

fn recover_single_baidu_sync_task(
    db: &Db,
    app_log_path: &Path,
    task: &BaiduSyncTask,
    reason: &str,
) -> Result<(), String> {
    match resolve_existing_uploaded_entry_for_task(db, task) {
        Ok(Some(remote_entry)) => {
            if let Err(err) = finalize_baidu_sync_task_success(db, app_log_path, task, remote_entry, reason) {
                append_log(
                    app_log_path,
                    &format!("baidu_sync_recover_finalize_fail id={} reason={} err={}", task.id, reason, err),
                );
                mark_baidu_sync_task_pending(db, task.id)?;
            }
        }
        Ok(None) => {
            if !Path::new(&task.local_path).exists() {
                let err = format!("本地同步源文件已删除，无法恢复同步: {}", task.local_path);
                update_baidu_sync_status(db, task.id, "FAILED", 0.0, Some(err.clone()))?;
                append_log(
                    app_log_path,
                    &format!("baidu_sync_recover_missing_local id={} reason={} err={}", task.id, reason, err),
                );
            } else {
                let remote_path = join_baidu_path(&task.remote_dir, &task.remote_name);
                mark_baidu_sync_task_pending(db, task.id)?;
                append_log(
                    app_log_path,
                    &format!("baidu_sync_recover_pending id={} reason={} remote={}", task.id, reason, remote_path),
                );
            }
        }
        Err(err) => {
            append_log(
                app_log_path,
                &format!("baidu_sync_recover_lookup_fail id={} reason={} err={}", task.id, reason, err),
            );
            if !Path::new(&task.local_path).exists() {
                let missing_err = format!("本地同步源文件已删除，无法恢复同步: {}", task.local_path);
                update_baidu_sync_status(db, task.id, "FAILED", 0.0, Some(missing_err.clone()))?;
                append_log(
                    app_log_path,
                    &format!("baidu_sync_recover_missing_local id={} reason={} err={}", task.id, reason, missing_err),
                );
            } else {
                let remote_path = join_baidu_path(&task.remote_dir, &task.remote_name);
                mark_baidu_sync_task_pending(db, task.id)?;
                append_log(
                    app_log_path,
                    &format!("baidu_sync_recover_pending id={} reason={} remote={}", task.id, reason, remote_path),
                );
            }
        }
    }
    Ok(())
}

fn mark_baidu_sync_task_pending(db: &Db, task_id: i64) -> Result<(), String> {
    let now = now_rfc3339();
    db.with_conn(|conn| {
        conn.execute(
            "UPDATE baidu_sync_task SET status = 'PENDING', progress = 0.0, updated_at = ?1 WHERE id = ?2",
            (&now, task_id),
        )?;
        Ok(())
    })
    .map_err(|err| err.to_string())
}

pub fn fail_missing_local_submission_sync_tasks(
    db: &Db,
    app_log_path: &Path,
    task_id: &str,
) -> Result<usize, String> {
    let storage_prefix = load_local_path_prefix(db);
    let candidates = db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, local_path, status \
                 FROM baidu_sync_task \
                 WHERE source_type = 'submission_merged' AND source_id = ?1 \
                   AND status IN ('PENDING', 'UPLOADING', 'PAUSED')",
            )?;
            let mut rows = stmt.query([task_id])?;
            let mut items = Vec::new();
            while let Some(row) = rows.next()? {
                let local_path = to_absolute_local_path_with_prefix(
                    storage_prefix.as_path(),
                    row.get::<_, String>(1)?.as_str(),
                )
                .to_string_lossy()
                .to_string();
                items.push((
                    row.get::<_, i64>(0)?,
                    local_path,
                    row.get::<_, String>(2)?,
                ));
            }
            Ok(items)
        })
        .map_err(|err| err.to_string())?;
    let mut affected = 0usize;
    for (sync_task_id, local_path, status) in candidates {
        if Path::new(&local_path).exists() {
            continue;
        }
        let err = format!("本地同步源文件已删除，终止同步: {}", local_path);
        update_baidu_sync_status(db, sync_task_id, "FAILED", 0.0, Some(err.clone()))?;
        append_log(
            app_log_path,
            &format!(
                "baidu_sync_mark_missing_local task_id={} sync_id={} prev_status={} err={}",
                task_id, sync_task_id, status, err
            ),
        );
        affected += 1;
    }
    Ok(affected)
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
    let now = now_rfc3339();
    let active_uid = get_active_baidu_uid(db)?;
    let active_credential = load_baidu_login_credential(db)?;
    let previous = load_baidu_login_info(db)?.unwrap_or(BaiduLoginInfo {
        status: "LOGGED_OUT".to_string(),
        uid: active_uid.clone(),
        username: None,
        login_type: None,
        login_time: None,
        last_check_time: None,
    });
    let Some(credential) = active_credential.as_ref() else {
        let info = BaiduLoginInfo {
            status: "LOGGED_OUT".to_string(),
            uid: active_uid.clone(),
            username: previous.username.clone(),
            login_type: previous.login_type,
            login_time: previous.login_time,
            last_check_time: Some(now),
        };
        upsert_baidu_login_info(db, &info)?;
        return Ok(info);
    };
    match validate_baidu_login_http(
        credential,
        previous.uid.as_deref().or(active_uid.as_deref()),
    ) {
        Ok(http_info) => {
            let info = BaiduLoginInfo {
                status: "LOGGED_IN".to_string(),
                uid: Some(http_info.uid.clone()),
                username: http_info
                    .username
                    .clone()
                    .or_else(|| previous.username.clone()),
                login_type: previous.login_type,
                login_time: previous.login_time,
                last_check_time: Some(now),
            };
            upsert_baidu_login_info(db, &info)?;
            if credential.baidu_uid.trim() != http_info.uid.trim() {
                let mut next_credential = credential.clone();
                next_credential.baidu_uid = http_info.uid.clone();
                let _ = upsert_baidu_login_credential(db, &next_credential);
                let _ = sync_baidu_cli_config(&next_credential);
            }
            Ok(info)
        }
        Err(err) => {
            if allow_auto_relogin && should_attempt_relogin(credential, &now) {
                let settings = load_baidu_sync_settings(db)?;
                let exec_path = resolve_baidu_exec_path(&settings.exec_path);
                let attempt_result = relogin_with_credential(db, &exec_path, credential);
                let _ = update_baidu_login_credential_attempt(db, attempt_result.as_ref().err());
                if let Ok(info) = attempt_result {
                    return Ok(info);
                }
            }
            let info = BaiduLoginInfo {
                status: "LOGGED_OUT".to_string(),
                uid: previous.uid.clone().or(active_uid.clone()),
                username: previous.username.clone(),
                login_type: previous.login_type,
                login_time: previous.login_time,
                last_check_time: Some(now),
            };
            let _ = update_baidu_login_credential_attempt(db, Some(&err));
            let _ = upsert_baidu_login_info(db, &info);
            Ok(info)
        }
    }
}

fn is_baidu_busy_error(err: &str) -> bool {
    err.contains("50052") || err.contains("系统繁忙")
}

pub fn login_baidu_with_cookie(db: &Db, cookie: &str) -> Result<BaiduLoginInfo, String> {
    let cookie = parse_baidu_cookie(cookie)?;
    let bduss = cookie
        .bduss
        .clone()
        .ok_or_else(|| "Cookie 缺少 BDUSS".to_string())?;
    let temp_credential = BaiduLoginCredential {
        baidu_uid: String::new(),
        login_type: "bduss".to_string(),
        cookie: Some(cookie.header.clone()),
        bduss: Some(bduss),
        stoken: cookie.stoken.clone(),
        last_attempt_time: None,
        last_attempt_error: None,
    };
    let info = perform_baidu_login(db, &temp_credential)?;
    let baidu_uid = info
        .uid
        .clone()
        .ok_or_else(|| "未识别到网盘账号 UID".to_string())?;
    let credential = BaiduLoginCredential {
        baidu_uid,
        login_type: "bduss".to_string(),
        cookie: Some(cookie.header),
        bduss: temp_credential.bduss,
        stoken: temp_credential.stoken,
        last_attempt_time: None,
        last_attempt_error: None,
    };
    finalize_baidu_login(db, info, "cookie", credential)
}

#[derive(Clone)]
struct ParsedBaiduCookie {
    header: String,
    bduss: Option<String>,
    stoken: Option<String>,
}

fn parse_baidu_cookie(input: &str) -> Result<ParsedBaiduCookie, String> {
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
    let mut bduss: Option<String> = None;
    let mut stoken: Option<String> = None;
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
            bduss = Some(value.to_string());
        }
        if key_lower == "bduss_bfess" {
            has_bduss_bfess = true;
            if bduss.is_none() {
                bduss = Some(value.to_string());
            }
        }
        if matches!(key_lower.as_str(), "stoken" | "stoken_bfess") && stoken.is_none() {
            stoken = Some(value.to_string());
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
    Ok(ParsedBaiduCookie {
        header: items.join("; "),
        bduss,
        stoken,
    })
}

pub fn login_baidu_with_bduss(
    db: &Db,
    bduss: &str,
    stoken: Option<&str>,
) -> Result<BaiduLoginInfo, String> {
    let bduss = normalize_baidu_token(bduss, "BDUSS")?;
    let stoken = match stoken {
        Some(value) => normalize_baidu_token_optional(value, "STOKEN")?,
        None => None,
    };
    let temp_credential = BaiduLoginCredential {
        baidu_uid: String::new(),
        login_type: "bduss".to_string(),
        cookie: None,
        bduss: Some(bduss.clone()),
        stoken: stoken.clone(),
        last_attempt_time: None,
        last_attempt_error: None,
    };
    let info = perform_baidu_login(db, &temp_credential)?;
    let baidu_uid = info
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
    finalize_baidu_login(db, info, "bduss", credential)
}

fn finalize_baidu_login(
    db: &Db,
    mut info: BaiduLoginInfo,
    display_login_type: &str,
    mut credential: BaiduLoginCredential,
) -> Result<BaiduLoginInfo, String> {
    info.login_type = Some(display_login_type.to_string());
    if info.login_time.is_none() {
        info.login_time = Some(now_rfc3339());
    }
    if credential.baidu_uid.trim().is_empty() {
        credential.baidu_uid = info.uid.clone().unwrap_or_default();
    }
    upsert_baidu_login_info(db, &info)?;
    upsert_baidu_login_credential(db, &credential)?;
    sync_baidu_cli_config(&credential)?;
    Ok(info)
}

fn perform_baidu_login(
    db: &Db,
    credential: &BaiduLoginCredential,
) -> Result<BaiduLoginInfo, String> {
    let http_info = validate_baidu_login_http(credential, None)?;
    Ok(BaiduLoginInfo {
        status: "LOGGED_IN".to_string(),
        uid: Some(http_info.uid),
        username: http_info.username,
        login_type: Some(credential.login_type.clone()),
        login_time: Some(now_rfc3339()),
        last_check_time: Some(now_rfc3339()),
    })
}

fn normalize_baidu_token(input: &str, label: &str) -> Result<String, String> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err(format!("{} 不能为空", label));
    }
    let cleaned = raw.replace('\r', ";").replace('\n', ";");
    let label_lower = label.to_ascii_lowercase();
    if cleaned.contains(';') || cleaned.contains('=') {
        if !cleaned.contains('=') {
            let first = cleaned
                .split(';')
                .map(|part| part.trim())
                .find(|part| !part.is_empty())
                .unwrap_or("");
            if !first.is_empty() {
                return Ok(first.to_string());
            }
        }
        for part in cleaned.split(';') {
            let token = part.trim();
            if token.is_empty() || !token.contains('=') {
                if !token.is_empty() && !token.contains('=') {
                    return Ok(token.to_string());
                }
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
            conn.execute(
                "DELETE FROM baidu_account_info WHERE uid = ?1",
                [uid.as_str()],
            )?;
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
            conn.query_row(
                "SELECT info.uid FROM baidu_account_info info \
           INNER JOIN baidu_account_credential credential ON credential.baidu_uid = info.uid \
           ORDER BY info.login_time DESC, info.update_time DESC LIMIT 1",
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
        conn.execute(
            "DELETE FROM baidu_account_info WHERE uid = ?1",
            [trimmed_uid],
        )?;
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

fn update_baidu_login_credential_attempt(db: &Db, err: Option<&String>) -> Result<(), String> {
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
    _exec_path: &Path,
    credential: &BaiduLoginCredential,
) -> Result<BaiduLoginInfo, String> {
    sync_baidu_cli_config(credential)?;
    let http_info = validate_baidu_login_http(credential, Some(credential.baidu_uid.as_str()))?;
    let mut next = BaiduLoginInfo {
        status: "LOGGED_IN".to_string(),
        uid: Some(http_info.uid),
        username: http_info.username,
        login_type: Some(credential.login_type.clone()),
        login_time: Some(now_rfc3339()),
        last_check_time: Some(now_rfc3339()),
    };
    next.login_type = Some(credential.login_type.clone());
    if next.login_time.is_none() {
        next.login_time = Some(now_rfc3339());
    }
    upsert_baidu_login_info(db, &next)?;
    Ok(next)
}

fn sync_baidu_cli_config(credential: &BaiduLoginCredential) -> Result<(), String> {
    let uid_text = credential.baidu_uid.trim();
    if uid_text.is_empty() {
        return Err("缺少网盘 UID".to_string());
    }
    let bduss = credential
        .bduss
        .as_deref()
        .map(|value| normalize_baidu_token(value, "BDUSS"))
        .transpose()?
        .ok_or_else(|| "缺少网盘 BDUSS".to_string())?;
    let stoken = credential
        .stoken
        .as_deref()
        .map(|value| normalize_baidu_token_optional(value, "STOKEN"))
        .transpose()?
        .flatten()
        .unwrap_or_default();
    let config_dir = resolve_baidu_pcs_config_dir()?;
    fs::create_dir_all(&config_dir).map_err(|err| format!("创建网盘配置目录失败: {}", err))?;
    let config_path = config_dir.join("pcs_config.json");
    let mut root = if config_path.exists() {
        fs::read_to_string(&config_path)
            .ok()
            .and_then(|content| serde_json::from_str::<Value>(&content).ok())
            .unwrap_or_else(|| json!({}))
    } else {
        json!({})
    };
    let uid_number = uid_text.parse::<i64>().ok();
    let normalized_bduss = bduss.clone();
    if let Some(users) = root
        .get_mut("baidu_user_list")
        .and_then(|value| value.as_array_mut())
    {
        users.retain(|item| {
            let value = item
                .get("uid")
                .map(|value| value.to_string().trim_matches('"').to_string())
                .unwrap_or_default();
            if value.trim().is_empty() {
                return false;
            }
            let item_bduss = item
                .get("bduss")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .trim();
            if !item_bduss.is_empty() && item_bduss == normalized_bduss && value.trim() != uid_text
            {
                return false;
            }
            true
        });
    }
    root["baidu_active_uid"] = uid_number
        .map(|value| json!(value))
        .unwrap_or_else(|| json!(uid_text));
    if !root["baidu_user_list"].is_array() {
        root["baidu_user_list"] = json!([]);
    }
    let users = root["baidu_user_list"]
        .as_array_mut()
        .ok_or_else(|| "网盘配置格式无效".to_string())?;
    let mut existing = users.iter().position(|item| {
        item.get("uid")
            .map(|value| value.to_string().trim_matches('"').to_string())
            .as_deref()
            == Some(uid_text)
    });
    if existing.is_none() {
        users.push(json!({}));
        existing = Some(users.len() - 1);
    }
    let user = users
        .get_mut(existing.unwrap())
        .ok_or_else(|| "网盘配置写入失败".to_string())?;
    if user.get("uid").is_none() {
        user["uid"] = uid_number
            .map(|value| json!(value))
            .unwrap_or_else(|| json!(uid_text));
    }
    if user.get("name").is_none() {
        user["name"] = json!("");
    }
    if user.get("sex").is_none() {
        user["sex"] = json!("");
    }
    if user.get("age").is_none() {
        user["age"] = json!(0.0);
    }
    if let Some(cookie) = credential.cookie.as_deref() {
        let parsed = parse_baidu_cookie(cookie)?;
        user["cookies"] = json!(parsed.header);
    } else {
        user["cookies"] = json!("");
    }
    user["bduss"] = json!(bduss);
    user["stoken"] = json!(stoken);
    user["ptoken"] = user.get("ptoken").cloned().unwrap_or_else(|| json!(""));
    user["baiduid"] = user.get("baiduid").cloned().unwrap_or_else(|| json!(""));
    user["sboxtkn"] = user.get("sboxtkn").cloned().unwrap_or_else(|| json!(""));
    user["accesstoken"] = user
        .get("accesstoken")
        .cloned()
        .unwrap_or_else(|| json!(""));
    user["workdir"] = json!("/");
    if root.get("appid").is_none() {
        root["appid"] = json!(266719);
    }
    if root.get("cache_size").is_none() {
        root["cache_size"] = json!(65536);
    }
    if root.get("max_parallel").is_none() {
        root["max_parallel"] = json!(3);
    }
    if root.get("max_upload_parallel").is_none() {
        root["max_upload_parallel"] = json!(4);
    }
    if root.get("max_download_load").is_none() {
        root["max_download_load"] = json!(1);
    }
    if root.get("max_upload_load").is_none() {
        root["max_upload_load"] = json!(4);
    }
    if root.get("max_download_rate").is_none() {
        root["max_download_rate"] = json!(0);
    }
    if root.get("max_upload_rate").is_none() {
        root["max_upload_rate"] = json!(0);
    }
    if root.get("user_agent").is_none() {
        root["user_agent"] = json!("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_13_2) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/63.0.3239.132 Safari/537.36");
    }
    if root.get("pcs_addr").is_none() {
        root["pcs_addr"] = json!("pcs.baidu.com");
    }
    if root.get("pan_ua").is_none() {
        root["pan_ua"] = json!("netdisk;P2SP;3.0.0.8;netdisk;11.12.3;ANG-AN00;android-android;10.0;JSbridge4.4.0;jointBridge;1.1.0;");
    }
    if root.get("savedir").is_none() {
        root["savedir"] = json!(default_download_dir().to_string_lossy().to_string());
    }
    if root.get("enable_https").is_none() {
        root["enable_https"] = json!(true);
    }
    if root.get("fix_pcs_addr").is_none() {
        root["fix_pcs_addr"] = json!(false);
    }
    if root.get("force_login_username").is_none() {
        root["force_login_username"] = json!("");
    }
    if root.get("proxy").is_none() {
        root["proxy"] = json!("");
    }
    if root.get("proxy_hostnames").is_none() {
        root["proxy_hostnames"] = json!("");
    }
    if root.get("local_addrs").is_none() {
        root["local_addrs"] = json!("");
    }
    if root.get("no_check").is_none() {
        root["no_check"] = json!(true);
    }
    if root.get("ignore_illegal").is_none() {
        root["ignore_illegal"] = json!(true);
    }
    if root.get("u_policy").is_none() {
        root["u_policy"] = json!("skip");
    }
    let content = serde_json::to_string_pretty(&root)
        .map_err(|err| format!("序列化网盘配置失败: {}", err))?;
    fs::write(&config_path, content).map_err(|err| format!("写入网盘配置失败: {}", err))?;
    Ok(())
}

fn resolve_baidu_pcs_config_dir() -> Result<PathBuf, String> {
    let path = std::env::var_os("BAIDUPCS_GO_CONFIG_DIR")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty());
    path.ok_or_else(|| "未找到百度网盘配置目录".to_string())
}

fn build_baidu_api_cookie_header(credential: &BaiduLoginCredential) -> Result<String, String> {
    let cookie_minimal =
        |bduss: Option<String>, stoken: Option<String>| -> Result<Option<String>, String> {
            if let Some(bduss_value) = bduss {
                let mut parts = vec![format!("BDUSS={}", bduss_value)];
                if let Some(stoken_value) = stoken {
                    parts.push(format!("STOKEN={}", stoken_value));
                }
                return Ok(Some(parts.join("; ")));
            }
            Ok(None)
        };
    let direct_bduss = credential
        .bduss
        .as_deref()
        .map(|value| normalize_baidu_token(value, "BDUSS"))
        .transpose()?;
    let direct_stoken = credential
        .stoken
        .as_deref()
        .map(|value| normalize_baidu_token_optional(value, "STOKEN"))
        .transpose()?
        .flatten();
    if let Some(header) = cookie_minimal(direct_bduss.clone(), direct_stoken.clone())? {
        return Ok(header);
    }
    if let Some(cookie) = credential.cookie.as_deref() {
        let parsed = parse_baidu_cookie(cookie)?;
        let mut parsed_bduss = None;
        let mut parsed_stoken = None;
        for part in parsed.header.split(';') {
            let token = part.trim();
            if token.is_empty() || !token.contains('=') {
                continue;
            }
            let mut iter = token.splitn(2, '=');
            let key = iter.next().unwrap_or("").trim();
            let value = iter.next().unwrap_or("").trim();
            if key.eq_ignore_ascii_case("BDUSS") || key.eq_ignore_ascii_case("BDUSS_BFESS") {
                if parsed_bduss.is_none() && !value.is_empty() {
                    parsed_bduss = Some(normalize_baidu_token(value, "BDUSS")?);
                }
            } else if key.eq_ignore_ascii_case("STOKEN") && !value.is_empty() {
                parsed_stoken = normalize_baidu_token_optional(value, "STOKEN")?;
            }
        }
        if let Some(header) = cookie_minimal(parsed_bduss, parsed_stoken)? {
            return Ok(header);
        }
    }
    if let Some(header) = cookie_minimal(direct_bduss, direct_stoken)? {
        return Ok(header);
    }
    let cookie = credential.cookie.as_deref().unwrap_or("");
    Ok(parse_baidu_cookie(cookie)?.header)
}

fn build_baidu_api_client() -> Result<Client, String> {
    Client::builder()
        .timeout(StdDuration::from_secs(15))
        .build()
        .map_err(|err| format!("初始化网盘客户端失败: {}", err))
}

fn fetch_baidu_remote_entries_via_http(
    credential: &BaiduLoginCredential,
    path: &str,
) -> Result<Vec<BaiduRemoteEntry>, String> {
    let client = build_baidu_api_client()?;
    let cookie = build_baidu_api_cookie_header(credential)?;
    let response = client
    .get("https://pan.baidu.com/rest/2.0/xpan/file")
    .header(USER_AGENT, "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_13_2) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/63.0.3239.132 Safari/537.36")
    .header(REFERER, "https://pan.baidu.com/disk/main")
    .header(COOKIE, cookie)
    .query(&[
      ("method", "list"),
      ("dir", path),
      ("web", "1"),
      ("order", "name"),
      ("desc", "0"),
      ("start", "0"),
      ("limit", "1000"),
    ])
    .send()
    .map_err(|err| format!("请求百度网盘目录失败: {}", err))?;
    let status = response.status();
    let text = response
        .text()
        .map_err(|err| format!("读取百度网盘目录响应失败: {}", err))?;
    if !status.is_success() {
        return Err(format!(
            "请求百度网盘目录失败: HTTP {} {}",
            status.as_u16(),
            text
        ));
    }
    let payload: Value = serde_json::from_str(&text)
        .map_err(|err| format!("解析百度网盘目录响应失败: {} | {}", err, text))?;
    let errno = payload
        .get("errno")
        .and_then(|value| value.as_i64())
        .unwrap_or(-1);
    if errno != 0 {
        return Err(format!("百度网盘目录读取失败: errno={} {}", errno, text));
    }
    let mut entries = Vec::new();
    for item in payload
        .get("list")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
    {
        let name = item
            .get("server_filename")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if name.is_empty() {
            continue;
        }
        let entry_path = item
            .get("path")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string())
            .unwrap_or_else(|| join_baidu_path(path, &name));
        let is_dir = item
            .get("isdir")
            .and_then(|value| value.as_i64())
            .unwrap_or(0)
            != 0;
        entries.push(BaiduRemoteEntry {
            name,
            path: entry_path,
            is_dir,
            size: item
                .get("size")
                .and_then(|value| value.as_u64())
                .unwrap_or(0),
        });
    }
    Ok(entries)
}

fn split_baidu_parent_and_name(path: &str) -> (String, String) {
    let normalized = normalize_baidu_path(path);
    if normalized == "/" {
        return ("/".to_string(), String::new());
    }
    let trimmed = normalized.trim_end_matches('/');
    if let Some(index) = trimmed.rfind('/') {
        if index == 0 {
            return ("/".to_string(), trimmed[1..].to_string());
        }
        return (
            trimmed[..index].to_string(),
            trimmed[index + 1..].to_string(),
        );
    }
    ("/".to_string(), trimmed.to_string())
}

fn find_baidu_remote_entry(db: &Db, remote_path: &str) -> Result<Option<BaiduRemoteEntry>, String> {
    let target_path = normalize_baidu_path(remote_path);
    if target_path == "/" {
        return Ok(Some(BaiduRemoteEntry {
            name: "/".to_string(),
            path: "/".to_string(),
            is_dir: true,
            size: 0,
        }));
    }
    let (parent_path, entry_name) = split_baidu_parent_and_name(&target_path);
    if entry_name.is_empty() {
        return Ok(None);
    }
    let credential =
        load_baidu_login_credential(db)?.ok_or_else(|| "请先登录网盘账号".to_string())?;
    let entries = fetch_baidu_remote_entries_via_http(&credential, &parent_path)?;
    Ok(entries
        .into_iter()
        .find(|entry| normalize_baidu_path(&entry.path) == target_path || entry.name == entry_name))
}

fn resolve_uploaded_baidu_remote_entry(
    db: &Db,
    remote_dir: &str,
    expected_name: &str,
    fallback_name: &str,
    local_size: u64,
) -> Result<Option<BaiduRemoteEntry>, String> {
    let mut entries = list_baidu_remote_entries(db, remote_dir)?
        .into_iter()
        .filter(|entry| !entry.is_dir)
        .collect::<Vec<_>>();
    if local_size > 0 {
        entries.retain(|entry| entry.size == local_size);
    }
    if entries.is_empty() {
        return Ok(None);
    }
    if let Some(entry) = entries
        .iter()
        .find(|entry| entry.name == expected_name)
        .cloned()
    {
        return Ok(Some(entry));
    }
    if !fallback_name.trim().is_empty() {
        if let Some(entry) = entries
            .iter()
            .find(|entry| entry.name == fallback_name)
            .cloned()
        {
            return Ok(Some(entry));
        }
    }
    if entries.len() == 1 {
        return Ok(entries.into_iter().next());
    }
    Ok(None)
}

fn resolve_existing_uploaded_entry_for_task(
    db: &Db,
    task: &BaiduSyncTask,
) -> Result<Option<BaiduRemoteEntry>, String> {
    let target_path = join_baidu_path(&task.remote_dir, &task.remote_name);
    let local_name = Path::new(&task.local_path)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let local_size = fs::metadata(&task.local_path)
        .map(|meta| meta.len())
        .unwrap_or(0);
    if local_size > 0 || !local_name.is_empty() {
        let entry = resolve_uploaded_baidu_remote_entry(
            db,
            &task.remote_dir,
            &task.remote_name,
            &local_name,
            local_size,
        )?;
        if entry.is_some() {
            return Ok(entry);
        }
    }
    find_baidu_remote_entry(db, &target_path)
}

fn update_baidu_sync_task_remote_binding(
    db: &Db,
    task_id: i64,
    remote_dir: &str,
    remote_name: &str,
) -> Result<(), String> {
    let now = now_rfc3339();
    db.with_conn(|conn| {
        conn.execute(
      "UPDATE baidu_sync_task SET remote_dir = ?1, remote_name = ?2, updated_at = ?3 WHERE id = ?4",
      (remote_dir, remote_name, &now, task_id),
    )?;
        Ok(())
    })
    .map_err(|err| err.to_string())
}

fn finalize_baidu_sync_task_success(
    db: &Db,
    app_log_path: &Path,
    task: &BaiduSyncTask,
    mut remote_entry: BaiduRemoteEntry,
    log_reason: &str,
) -> Result<(), String> {
    if task.remote_name != remote_entry.name {
        let target_path = join_baidu_path(&task.remote_dir, &task.remote_name);
        match load_baidu_login_credential(db)? {
            Some(credential) => {
                match rename_baidu_remote_dir_via_http(&credential, &remote_entry.path, &task.remote_name)
                {
                    Ok(()) => {
                        remote_entry.name = task.remote_name.clone();
                        remote_entry.path = target_path;
                    }
                    Err(err) => {
                        append_log(
                            app_log_path,
                            &format!(
                                "baidu_sync_task_rename_fail id={} from={} to={} err={}",
                                task.id, remote_entry.path, target_path, err
                            ),
                        );
                    }
                }
            }
            None => {
                append_log(
                    app_log_path,
                    &format!(
                        "baidu_sync_task_rename_skip id={} reason=no_baidu_credential from={} to={}",
                        task.id, remote_entry.path, target_path
                    ),
                );
            }
        }
    }

    let remote_path = remote_entry.path.clone();
    let size = fetch_baidu_remote_file_size(db, &remote_path)?;
    if size == 0 {
        return Err("上传后文件大小为0".to_string());
    }

    if remote_entry.name != task.remote_name {
        update_baidu_sync_task_remote_binding(db, task.id, &task.remote_dir, &remote_entry.name)?;
    }
    update_baidu_sync_status(db, task.id, "SUCCESS", 100.0, None)?;

    if task.source_type == "submission_merged" {
        if let Some(task_id) = task
            .source_id
            .as_deref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        {
            if let Err(err) = bind_submission_merged_remote(
                db,
                task_id,
                &task.local_path,
                &task.remote_dir,
                &remote_entry.name,
                task.baidu_uid.as_deref(),
            ) {
                append_log(
                    app_log_path,
                    &format!("baidu_sync_bind_merged_fail task_id={} err={}", task_id, err),
                );
            }
        }
    }

    append_log(
        app_log_path,
        &format!(
            "baidu_sync_task_finalize_ok id={} remote={} size={} reason={}",
            task.id, remote_path, size, log_reason
        ),
    );
    Ok(())
}

fn create_baidu_remote_dir_via_http(
    credential: &BaiduLoginCredential,
    path: &str,
) -> Result<(), String> {
    let client = build_baidu_api_client()?;
    let cookie = build_baidu_api_cookie_header(credential)?;
    let normalized_path = normalize_baidu_path(path);
    let form = vec![
        ("path", normalized_path.clone()),
        ("isdir", "1".to_string()),
        ("block_list", "[]".to_string()),
    ];
    let response = client
    .post("https://pan.baidu.com/api/create")
    .header(USER_AGENT, "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_13_2) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/63.0.3239.132 Safari/537.36")
    .header(REFERER, "https://pan.baidu.com/disk/main")
    .header(COOKIE, cookie)
    .query(&[
      ("a", "commit"),
      ("channel", "chunlei"),
      ("clienttype", "0"),
      ("web", "1"),
      ("app_id", "250528"),
    ])
    .form(&form)
    .send()
    .map_err(|err| format!("请求百度网盘创建目录失败: {}", err))?;
    let status = response.status();
    let text = response
        .text()
        .map_err(|err| format!("读取百度网盘创建目录响应失败: {}", err))?;
    if !status.is_success() {
        return Err(format!(
            "请求百度网盘创建目录失败: HTTP {} {}",
            status.as_u16(),
            text
        ));
    }
    let payload: Value = serde_json::from_str(&text)
        .map_err(|err| format!("解析百度网盘创建目录响应失败: {} | {}", err, text))?;
    let errno = payload
        .get("errno")
        .and_then(|value| value.as_i64())
        .unwrap_or(-1);
    if errno != 0 {
        return Err(format!("百度网盘创建目录失败: errno={} {}", errno, text));
    }
    Ok(())
}

fn rename_baidu_remote_dir_via_http(
    credential: &BaiduLoginCredential,
    from_path: &str,
    new_name: &str,
) -> Result<(), String> {
    let client = build_baidu_api_client()?;
    let cookie = build_baidu_api_cookie_header(credential)?;
    let filelist = serde_json::to_string(&vec![json!({
      "path": normalize_baidu_path(from_path),
      "newname": new_name,
    })])
    .map_err(|err| format!("序列化百度网盘重命名请求失败: {}", err))?;
    let form = vec![("filelist", filelist)];
    let response = client
    .post("https://pan.baidu.com/api/filemanager")
    .header(USER_AGENT, "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_13_2) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/63.0.3239.132 Safari/537.36")
    .header(REFERER, "https://pan.baidu.com/disk/main")
    .header(COOKIE, cookie)
    .query(&[
      ("opera", "rename"),
      ("async", "0"),
      ("onnest", "fail"),
      ("channel", "chunlei"),
      ("clienttype", "0"),
      ("web", "1"),
      ("app_id", "250528"),
    ])
    .form(&form)
    .send()
    .map_err(|err| format!("请求百度网盘重命名目录失败: {}", err))?;
    let status = response.status();
    let text = response
        .text()
        .map_err(|err| format!("读取百度网盘重命名目录响应失败: {}", err))?;
    if !status.is_success() {
        return Err(format!(
            "请求百度网盘重命名目录失败: HTTP {} {}",
            status.as_u16(),
            text
        ));
    }
    let payload: Value = serde_json::from_str(&text)
        .map_err(|err| format!("解析百度网盘重命名目录响应失败: {} | {}", err, text))?;
    let errno = payload
        .get("errno")
        .and_then(|value| value.as_i64())
        .unwrap_or(-1);
    if errno != 0 {
        return Err(format!("百度网盘重命名目录失败: errno={} {}", errno, text));
    }
    Ok(())
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
    if !Path::new(&task.local_path).exists() {
        let err = format!("本地同步源文件不存在: {}", task.local_path);
        append_log(
            context.app_log_path.as_ref(),
            &format!("baidu_sync_task_error id={} err={}", task.id, err),
        );
        update_baidu_sync_status(context.db.as_ref(), task.id, "FAILED", 0.0, Some(err))?;
        return Ok(());
    }
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
            if output.stderr.contains("pipe_drain_timeout") {
                append_log(
                    context.app_log_path.as_ref(),
                    &format!("baidu_sync_task_pipe_warning id={} detail={}", task.id, output.stderr),
                );
            }
            let uploaded_entry =
                resolve_existing_uploaded_entry_for_task_async(Arc::clone(&context.db), task.clone())
                    .await?;
            let Some(remote_entry) = uploaded_entry else {
                let local_size = fs::metadata(&task.local_path)
                    .map(|meta| meta.len())
                    .unwrap_or(0);
                let local_name = Path::new(&task.local_path)
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("")
                    .to_string();
                let err = format!(
                    "上传后未找到远程文件 dir={} expected={} local_name={} size={}",
                    task.remote_dir, task.remote_name, local_name, local_size
                );
                append_log(
                    context.app_log_path.as_ref(),
                    &format!("baidu_sync_task_error id={} err={}", task.id, err),
                );
                return handle_baidu_sync_failure(context.db.as_ref(), task, settings.retry, &err);
            };
            append_log(
                context.app_log_path.as_ref(),
                &format!("baidu_sync_task_uploaded id={} remote={}", task.id, remote_entry.path),
            );
            if let Err(err) = finalize_baidu_sync_task_success_async(
                Arc::clone(&context.db),
                Arc::clone(&context.app_log_path),
                task.clone(),
                remote_entry,
                "upload".to_string(),
            )
            .await
            {
                append_log(
                    context.app_log_path.as_ref(),
                    &format!("baidu_sync_task_error id={} err={}", task.id, err),
                );
                return handle_baidu_sync_failure(context.db.as_ref(), task, settings.retry, &err);
            }
            append_log(
                context.app_log_path.as_ref(),
                &format!(
                    "baidu_sync_task_ok id={} output={}",
                    task.id,
                    output.stdout.len()
                ),
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

async fn resolve_existing_uploaded_entry_for_task_async(
    db: Arc<Db>,
    task: BaiduSyncTask,
) -> Result<Option<BaiduRemoteEntry>, String> {
    let task_id = task.id;
    task::spawn_blocking(move || resolve_existing_uploaded_entry_for_task(db.as_ref(), &task))
        .await
        .map_err(|err| format!("baidu_sync_resolve_uploaded_join_fail id={} err={}", task_id, err))?
}

async fn finalize_baidu_sync_task_success_async(
    db: Arc<Db>,
    app_log_path: Arc<PathBuf>,
    task: BaiduSyncTask,
    remote_entry: BaiduRemoteEntry,
    log_reason: String,
) -> Result<(), String> {
    let task_id = task.id;
    task::spawn_blocking(move || {
        finalize_baidu_sync_task_success(
            db.as_ref(),
            app_log_path.as_ref(),
            &task,
            remote_entry,
            &log_reason,
        )
    })
    .await
    .map_err(|err| format!("baidu_sync_finalize_join_fail id={} err={}", task_id, err))?
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
    conn.query_row(
        "SELECT value FROM app_settings WHERE key = ?1",
        [key],
        |row| row.get(0),
    )
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

fn load_room_baidu_sync_config(db: &Db, room_id: &str) -> Result<(bool, Option<String>), String> {
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
        if trimmed.contains("文件总数") || trimmed.contains("目录总数") || trimmed.contains("总:")
        {
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
            size: 0,
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

fn append_command_warning(target: &Arc<Mutex<String>>, warning: &str) {
    if let Ok(mut guard) = target.lock() {
        if !guard.is_empty() && !guard.ends_with('\n') {
            guard.push('\n');
        }
        guard.push_str(warning);
    }
}

fn summarize_command_output(output: &str, max_lines: usize, max_chars: usize) -> String {
    let lines: Vec<&str> = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() {
        return "无输出".to_string();
    }
    let start = lines.len().saturating_sub(max_lines);
    let mut summary = lines[start..].join(" | ");
    if summary.chars().count() > max_chars {
        summary = summary.chars().take(max_chars).collect::<String>();
        summary.push_str("...");
    }
    summary
}

fn summarize_command_output_pair(stdout: &str, stderr: &str) -> String {
    let stdout_summary = summarize_command_output(stdout, 12, 1200);
    let stderr_summary = summarize_command_output(stderr, 8, 600);
    if stderr_summary == "无输出" {
        stdout_summary
    } else if stdout_summary == "无输出" {
        format!("stderr={}", stderr_summary)
    } else {
        format!("stdout={} | stderr={}", stdout_summary, stderr_summary)
    }
}

fn detect_baidu_download_failure(output: &CommandOutput) -> Option<String> {
    let stdout = output.stdout.trim();
    if stdout.is_empty() {
        return None;
    }
    let markers = [
        "以下文件下载失败",
        "下载文件失败",
        "获取下载路径信息错误",
        "获取下载链接失败",
        "检测文件有效性失败",
        "检测文件大小一致性失败",
        "该文件校验失败",
    ];
    if !markers.iter().any(|marker| stdout.contains(marker)) {
        return None;
    }
    Some(summarize_command_output(stdout, 16, 1600))
}

fn parse_download_total_size(output: &str) -> Option<u64> {
    for line in output.lines() {
        let cleaned = strip_ansi(line).replace('\r', " ").replace('\n', " ");
        let marker = "数据总量:";
        let Some(index) = cleaned.find(marker) else {
            continue;
        };
        let after = cleaned[index + marker.len()..].trim();
        let size_text = after.split_whitespace().next()?;
        return parse_size(size_text);
    }
    None
}

fn summarize_local_dir_snapshot(base_dir: &Path, max_entries: usize) -> String {
    let entries = match fs::read_dir(base_dir) {
        Ok(entries) => entries,
        Err(err) => return format!("读取目录失败: {}", err),
    };
    let mut names = Vec::new();
    for entry in entries.flatten().take(max_entries) {
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("<invalid>");
        let suffix = if path.is_dir() { "/" } else { "" };
        names.push(format!("{}{}", name, suffix));
    }
    if names.is_empty() {
        "empty".to_string()
    } else {
        names.join(", ")
    }
}

fn run_baidu_pcs_command(exec_path: &Path, args: &[String]) -> Result<CommandOutput, String> {
    run_baidu_pcs_command_with_timeout(exec_path, args, StdDuration::from_secs(20))
}

fn run_baidu_pcs_command_with_timeout(
    exec_path: &Path,
    args: &[String],
    timeout: StdDuration,
) -> Result<CommandOutput, String> {
    let mut command = Command::new(exec_path);
    apply_no_window(&mut command);
    let mut child = command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("BaiduPCS-Go 执行失败: {}", err))?;
    let started_at = Instant::now();
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|err| format!("BaiduPCS-Go 执行失败: {}", err))?
        {
            break status;
        }
        if started_at.elapsed() >= timeout {
            let _ = child.kill();
            let output = child
                .wait_with_output()
                .map_err(|err| format!("BaiduPCS-Go 执行失败: {}", err))?;
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let detail = if !stderr.trim().is_empty() {
                stderr.trim().to_string()
            } else if !stdout.trim().is_empty() {
                stdout.trim().to_string()
            } else {
                "无输出".to_string()
            };
            return Err(format!(
                "BaiduPCS-Go 执行超时({}s): {} | {}",
                timeout.as_secs(),
                summarize_baidu_command_args(args),
                detail
            ));
        }
        std::thread::sleep(StdDuration::from_millis(100));
    };
    let output = child
        .wait_with_output()
        .map_err(|err| format!("BaiduPCS-Go 执行失败: {}", err))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if status.success() {
        if stdout.contains("文件上传失败") {
            return Err(format!("上传失败: {}", stdout.trim()));
        }
        return Ok(CommandOutput { stdout, stderr });
    }
    Err(format!(
        "BaiduPCS-Go 执行失败: {}",
        summarize_command_output_pair(&stdout, &stderr)
    ))
}

fn summarize_baidu_command_args(args: &[String]) -> String {
    args.iter()
        .map(|arg| {
            if arg.starts_with("-cookies=") {
                "-cookies=<redacted>".to_string()
            } else if arg.starts_with("-bduss=") {
                "-bduss=<redacted>".to_string()
            } else if arg.starts_with("-stoken=") {
                "-stoken=<redacted>".to_string()
            } else {
                arg.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn check_baidu_remote_access(credential: &BaiduLoginCredential) -> Result<(), String> {
    fetch_baidu_remote_entries_via_http(credential, "/").map(|_| ())
}

fn validate_baidu_login_http(
    credential: &BaiduLoginCredential,
    fallback_uid: Option<&str>,
) -> Result<BaiduHttpUserInfo, String> {
    let user_info = fetch_baidu_user_info_via_http(credential)?;
    check_baidu_remote_access(credential)?;
    let uid = if !credential.baidu_uid.trim().is_empty() {
        credential.baidu_uid.trim().to_string()
    } else if let Some(uid) = fallback_uid
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        uid.to_string()
    } else {
        user_info.uid
    };
    Ok(BaiduHttpUserInfo {
        uid,
        username: user_info.username,
    })
}

fn fetch_baidu_user_info_via_http(
    credential: &BaiduLoginCredential,
) -> Result<BaiduHttpUserInfo, String> {
    let client = build_baidu_api_client()?;
    let cookie = build_baidu_api_cookie_header(credential)?;
    let response = client
    .get("https://pan.baidu.com/rest/2.0/xpan/nas")
    .header(USER_AGENT, "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_13_2) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/63.0.3239.132 Safari/537.36")
    .header(REFERER, "https://pan.baidu.com/disk/main")
    .header(COOKIE, cookie)
    .query(&[("method", "uinfo")])
    .send()
    .map_err(|err| format!("请求百度网盘用户信息失败: {}", err))?;
    let status = response.status();
    let text = response
        .text()
        .map_err(|err| format!("读取百度网盘用户信息响应失败: {}", err))?;
    if !status.is_success() {
        return Err(format!(
            "请求百度网盘用户信息失败: HTTP {} {}",
            status.as_u16(),
            text
        ));
    }
    let payload: Value = serde_json::from_str(&text)
        .map_err(|err| format!("解析百度网盘用户信息失败: {} | {}", err, text))?;
    let errno = payload
        .get("errno")
        .and_then(|value| value.as_i64())
        .unwrap_or(-1);
    if errno != 0 {
        return Err(format!(
            "百度网盘用户信息读取失败: errno={} {}",
            errno, text
        ));
    }
    let uid = payload
        .get("uk")
        .and_then(|value| value.as_i64())
        .map(|value| value.to_string())
        .or_else(|| {
            payload
                .get("uk")
                .and_then(|value| value.as_str())
                .map(|value| value.trim().to_string())
        })
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("百度网盘用户信息缺少 uk: {}", text))?;
    let username = payload
        .get("baidu_name")
        .and_then(|value| value.as_str())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    Ok(BaiduHttpUserInfo { uid, username })
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
        .env("BAIDUPCS_GO_VERBOSE", "1")
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
        if let Some(detail) = detect_baidu_download_failure(&CommandOutput {
            stdout: stdout.clone(),
            stderr: stderr.clone(),
        }) {
            return Err(format!("BaiduPCS-Go 下载失败: {}", detail));
        }
        return Ok(CommandOutput { stdout, stderr });
    }
    Err(format!(
        "BaiduPCS-Go 执行失败: {}",
        summarize_command_output_pair(&stdout, &stderr)
    ))
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

    let stdout_buf = Arc::new(Mutex::new(String::new()));
    let stdout_buf_reader = Arc::clone(&stdout_buf);
    let (stdout_event_tx, stdout_event_rx) = std::sync::mpsc::channel::<Option<f64>>();
    let (stdout_done_tx, stdout_done_rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        let mut pending: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let read_size = match stdout.read(&mut chunk) {
                Ok(size) => size,
                Err(_) => 0,
            };
            if read_size == 0 {
                break;
            }
            let slice = &chunk[..read_size];
            if let Ok(mut guard) = stdout_buf_reader.lock() {
                guard.push_str(&String::from_utf8_lossy(slice));
            }
            let _ = stdout_event_tx.send(None);
            pending.extend_from_slice(slice);

            loop {
                let split_pos = pending
                    .iter()
                    .position(|value| *value == b'\n' || *value == b'\r');
                let Some(pos) = split_pos else {
                    break;
                };
                let mut line_bytes: Vec<u8> = pending.drain(..=pos).collect();
                while matches!(line_bytes.last(), Some(b'\n' | b'\r')) {
                    line_bytes.pop();
                }
                if line_bytes.is_empty() {
                    continue;
                }
                let line = String::from_utf8_lossy(&line_bytes);
                if let Some(progress) = parse_progress_line(&line) {
                    let _ = stdout_event_tx.send(Some(progress));
                }
            }
        }
        if !pending.is_empty() {
            let line = String::from_utf8_lossy(&pending);
            if let Some(progress) = parse_progress_line(&line) {
                let _ = stdout_event_tx.send(Some(progress));
            }
        }
        let _ = stdout_done_tx.send(());
    });

    let stderr_buf = Arc::new(Mutex::new(String::new()));
    let stderr_buf_reader = Arc::clone(&stderr_buf);
    let (stderr_done_tx, stderr_done_rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        loop {
            let read_size = match stderr.read(&mut chunk) {
                Ok(size) => size,
                Err(_) => 0,
            };
            if read_size == 0 {
                break;
            }
            if let Ok(mut guard) = stderr_buf_reader.lock() {
                guard.push_str(&String::from_utf8_lossy(&chunk[..read_size]));
            }
        }
        let _ = stderr_done_tx.send(());
    });

    let started_at = Instant::now();
    let mut last_activity = Instant::now();
    let status = loop {
        for event in stdout_event_rx.try_iter() {
            last_activity = Instant::now();
            if let Some(progress) = event {
                on_progress(progress);
            }
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|err| format!("BaiduPCS-Go 执行失败: {}", err))?
        {
            break status;
        }
        if started_at.elapsed() >= BAIDU_SYNC_UPLOAD_TIMEOUT
            || last_activity.elapsed() >= BAIDU_SYNC_UPLOAD_IDLE_TIMEOUT
        {
            let _ = child.kill();
            let _ = child.wait();
            if stdout_done_rx
                .recv_timeout(BAIDU_SYNC_PIPE_DRAIN_TIMEOUT)
                .is_err()
            {
                append_command_warning(&stdout_buf, "pipe_drain_timeout: stdout");
            }
            if stderr_done_rx
                .recv_timeout(BAIDU_SYNC_PIPE_DRAIN_TIMEOUT)
                .is_err()
            {
                append_command_warning(&stderr_buf, "pipe_drain_timeout: stderr");
            }
            let stdout_output = stdout_buf
                .lock()
                .map(|value| value.clone())
                .unwrap_or_default();
            let stderr_output = stderr_buf
                .lock()
                .map(|value| value.clone())
                .unwrap_or_default();
            let reason = if started_at.elapsed() >= BAIDU_SYNC_UPLOAD_TIMEOUT {
                format!("总时长超过{}秒", BAIDU_SYNC_UPLOAD_TIMEOUT.as_secs())
            } else {
                format!("{}秒无进度输出", BAIDU_SYNC_UPLOAD_IDLE_TIMEOUT.as_secs())
            };
            return Err(format!(
                "BaiduPCS-Go 上传超时: {} | {} | {}",
                reason,
                summarize_baidu_command_args(args),
                summarize_command_output_pair(&stdout_output, &stderr_output)
            ));
        }
        std::thread::sleep(StdDuration::from_millis(200));
    };
    for event in stdout_event_rx.try_iter() {
        if let Some(progress) = event {
            on_progress(progress);
        }
    }
    if stdout_done_rx
        .recv_timeout(BAIDU_SYNC_PIPE_DRAIN_TIMEOUT)
        .is_err()
    {
        append_command_warning(&stdout_buf, "pipe_drain_timeout: stdout");
    }
    if stderr_done_rx
        .recv_timeout(BAIDU_SYNC_PIPE_DRAIN_TIMEOUT)
        .is_err()
    {
        append_command_warning(&stderr_buf, "pipe_drain_timeout: stderr");
    }
    let stdout_buf = stdout_buf
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();
    let stderr_output = stderr_buf
        .lock()
        .map(|value| value.clone())
        .unwrap_or_default();

    if status.success() {
        if stdout_buf.contains("文件上传失败") {
            return Err(format!("上传失败: {}", stdout_buf.trim()));
        }
        return Ok(CommandOutput {
            stdout: stdout_buf,
            stderr: stderr_output,
        });
    }
    Err(format!(
        "BaiduPCS-Go 执行失败: {}",
        summarize_command_output_pair(&stdout_buf, &stderr_output)
    ))
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
    match value
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())?
    {
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
