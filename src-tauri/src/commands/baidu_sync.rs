use std::collections::HashMap;
use std::env;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use serde::{Deserialize, Serialize};
use tauri::webview::cookie;
use tauri::{AppHandle, State, Url, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tokio::sync::oneshot;
use tokio::time::{sleep, Duration};

use crate::api::ApiResponse;
use crate::app_log;
use crate::baidu_sync;
use crate::config;
use crate::utils::{append_log, apply_no_window, now_rfc3339};
use crate::AppState;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaiduLoginRequest {
  pub login_type: String,
  pub cookie: Option<String>,
  pub bduss: Option<String>,
  pub stoken: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaiduSyncListRequest {
  pub status: Option<String>,
  pub page: Option<i64>,
  pub page_size: Option<i64>,
}

#[derive(Default)]
pub struct BaiduLoginRuntime {
  status: String,
  prompt: Option<String>,
  captcha_path: Option<String>,
  captcha_url: Option<String>,
  output: Vec<String>,
  last_error: Option<String>,
  child: Option<Child>,
  stdin: Option<ChildStdin>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BaiduAccountLoginStatus {
  pub status: String,
  pub prompt: Option<String>,
  pub captcha_path: Option<String>,
  pub captcha_url: Option<String>,
  pub output: Vec<String>,
  pub last_error: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaiduRemoteListRequest {
  pub path: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaiduSyncUpdateRequest {
  pub concurrency: Option<i64>,
  pub target_path: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BaiduRemoteEntry {
  pub name: String,
  pub path: String,
  pub is_dir: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaiduRemoteCreateRequest {
  pub parent_path: Option<String>,
  pub name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaiduRemoteRenameRequest {
  pub from_path: Option<String>,
  pub name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaiduAccountLoginRequest {
  pub username: String,
  pub password: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaiduAccountLoginInput {
  pub input: String,
}

#[tauri::command]
pub fn baidu_sync_settings(state: State<'_, AppState>) -> ApiResponse<baidu_sync::BaiduSyncSettings> {
  match baidu_sync::load_baidu_sync_settings(&state.db) {
    Ok(settings) => ApiResponse::success(settings),
    Err(err) => ApiResponse::error(err),
  }
}

#[tauri::command]
pub fn baidu_sync_status(
  state: State<'_, AppState>,
) -> ApiResponse<baidu_sync::BaiduLoginInfo> {
  match baidu_sync::check_baidu_login(&state.db) {
    Ok(info) => ApiResponse::success(info),
    Err(err) => ApiResponse::error(err),
  }
}

#[tauri::command]
pub fn baidu_accounts_list(
  state: State<'_, AppState>,
) -> ApiResponse<Vec<baidu_sync::BaiduAccountSummary>> {
  match baidu_sync::list_baidu_accounts(&state.db) {
    Ok(accounts) => ApiResponse::success(accounts),
    Err(err) => ApiResponse::error(err),
  }
}

#[tauri::command]
pub fn baidu_account_switch(
  state: State<'_, AppState>,
  uid: String,
) -> ApiResponse<baidu_sync::BaiduLoginInfo> {
  match baidu_sync::switch_baidu_account(&state.db, &uid) {
    Ok(Some(info)) => ApiResponse::success(info),
    Ok(None) => ApiResponse::error("网盘账号不存在"),
    Err(err) => ApiResponse::error(err),
  }
}

#[tauri::command]
pub fn baidu_account_logout(state: State<'_, AppState>, uid: String) -> ApiResponse<String> {
  match baidu_sync::logout_baidu_by_uid(&state.db, &uid) {
    Ok(()) => ApiResponse::success("ok".to_string()),
    Err(err) => ApiResponse::error(err),
  }
}

#[tauri::command]
pub fn baidu_sync_login(
  state: State<'_, AppState>,
  request: BaiduLoginRequest,
) -> ApiResponse<baidu_sync::BaiduLoginInfo> {
  let login_type = request.login_type.to_lowercase();
  append_log(
    state.app_log_path.as_ref(),
    &format!(
      "baidu_login_request type={} cookie_len={} bduss_len={} stoken_len={}",
      login_type,
      request.cookie.as_deref().unwrap_or("").trim().len(),
      request.bduss.as_deref().unwrap_or("").trim().len(),
      request.stoken.as_deref().unwrap_or("").trim().len()
    ),
  );
  let result = if login_type == "cookie" {
    let cookie = request.cookie.unwrap_or_default();
    baidu_sync::login_baidu_with_cookie(&state.db, &cookie)
  } else if login_type == "bduss" {
    let bduss = request.bduss.unwrap_or_default();
    let stoken = request.stoken.unwrap_or_default();
    append_log(
      state.app_log_path.as_ref(),
      &format!(
        "baidu_login_bduss_start bduss={} stoken={}",
        summarize_baidu_token(&bduss, "BDUSS"),
        summarize_baidu_token(&stoken, "STOKEN"),
      ),
    );
    let result = baidu_sync::login_baidu_with_bduss(&state.db, &bduss, Some(stoken.as_str()));
    if let Err(err) = &result {
      append_log(
        state.app_log_path.as_ref(),
        &format!("baidu_login_bduss_fail err={}", err),
      );
    } else {
      append_log(state.app_log_path.as_ref(), "baidu_login_bduss_ok");
    }
    result
  } else {
    Err("未知的登录类型".to_string())
  };
  match result {
    Ok(info) => ApiResponse::success(info),
    Err(err) => ApiResponse::error(err),
  }
}

fn summarize_baidu_token(raw: &str, label: &str) -> String {
  let trimmed = raw.trim();
  let lower = trimmed.to_ascii_lowercase();
  let prefix = format!("{}=", label.to_ascii_lowercase());
  format!(
    "len={},has_eq={},has_semicolon={},has_prefix={}",
    trimmed.len(),
    trimmed.contains('='),
    trimmed.contains(';'),
    lower.starts_with(&prefix),
  )
}

#[tauri::command]
pub fn baidu_sync_logout(state: State<'_, AppState>) -> ApiResponse<String> {
  match baidu_sync::logout_baidu(&state.db) {
    Ok(()) => ApiResponse::success("ok".to_string()),
    Err(err) => ApiResponse::error(err),
  }
}

#[tauri::command]
pub fn baidu_sync_list(
  state: State<'_, AppState>,
  request: Option<BaiduSyncListRequest>,
) -> ApiResponse<Vec<baidu_sync::BaiduSyncTaskRecord>> {
  let request = request.unwrap_or(BaiduSyncListRequest {
    status: None,
    page: Some(1),
    page_size: Some(50),
  });
  let page = request.page.unwrap_or(1).max(1);
  let page_size = request.page_size.unwrap_or(50).clamp(1, 200);
  match baidu_sync::list_baidu_sync_tasks(&state.db, request.status, page, page_size) {
    Ok(list) => ApiResponse::success(list),
    Err(err) => ApiResponse::error(err),
  }
}

#[tauri::command]
pub fn baidu_sync_remote_dirs(
  state: State<'_, AppState>,
  request: Option<BaiduRemoteListRequest>,
) -> ApiResponse<Vec<BaiduRemoteEntry>> {
  let path = request
    .and_then(|value| value.path)
    .unwrap_or_else(|| "/".to_string());
  match baidu_sync::list_baidu_remote_dirs(&state.db, &path) {
    Ok(list) => ApiResponse::success(
      list
        .into_iter()
        .map(|item| BaiduRemoteEntry {
          name: item.name,
          path: item.path,
          is_dir: true,
        })
        .collect(),
    ),
    Err(err) => ApiResponse::error(err),
  }
}

#[tauri::command]
pub fn baidu_sync_remote_entries(
  state: State<'_, AppState>,
  request: Option<BaiduRemoteListRequest>,
) -> ApiResponse<Vec<BaiduRemoteEntry>> {
  let path = request
    .and_then(|value| value.path)
    .unwrap_or_else(|| "/".to_string());
  match baidu_sync::list_baidu_remote_entries(&state.db, &path) {
    Ok(list) => ApiResponse::success(
      list
        .into_iter()
        .map(|item| BaiduRemoteEntry {
          name: item.name,
          path: item.path,
          is_dir: item.is_dir,
        })
        .collect(),
    ),
    Err(err) => ApiResponse::error(err),
  }
}

#[tauri::command]
pub fn baidu_sync_create_dir(
  state: State<'_, AppState>,
  request: Option<BaiduRemoteCreateRequest>,
) -> ApiResponse<BaiduRemoteEntry> {
  let request = request.unwrap_or(BaiduRemoteCreateRequest {
    parent_path: Some("/".to_string()),
    name: Some("新建文件夹".to_string()),
  });
  let parent_path = request.parent_path.unwrap_or_else(|| "/".to_string());
  let name = request.name.unwrap_or_else(|| "新建文件夹".to_string());
  match baidu_sync::create_baidu_remote_dir(&state.db, &parent_path, &name) {
    Ok(entry) => ApiResponse::success(BaiduRemoteEntry {
      name: entry.name,
      path: entry.path,
      is_dir: true,
    }),
    Err(err) => ApiResponse::error(err),
  }
}

#[tauri::command]
pub fn baidu_sync_rename_dir(
  state: State<'_, AppState>,
  request: Option<BaiduRemoteRenameRequest>,
) -> ApiResponse<BaiduRemoteEntry> {
  let request = request.ok_or_else(|| "请求参数不能为空".to_string());
  let request = match request {
    Ok(value) => value,
    Err(err) => return ApiResponse::error(err),
  };
  let from_path = request.from_path.unwrap_or_default();
  if from_path.trim().is_empty() {
    return ApiResponse::error("原目录不能为空".to_string());
  }
  let name = request.name.unwrap_or_default();
  match baidu_sync::rename_baidu_remote_dir(&state.db, &from_path, &name) {
    Ok(entry) => ApiResponse::success(BaiduRemoteEntry {
      name: entry.name,
      path: entry.path,
      is_dir: true,
    }),
    Err(err) => ApiResponse::error(err),
  }
}

#[tauri::command]
pub fn baidu_sync_retry(state: State<'_, AppState>, task_id: i64) -> ApiResponse<String> {
  match baidu_sync::retry_baidu_sync_task(&state.db, task_id) {
    Ok(()) => ApiResponse::success("ok".to_string()),
    Err(err) => ApiResponse::error(err),
  }
}

#[tauri::command]
pub fn baidu_sync_cancel(state: State<'_, AppState>, task_id: i64) -> ApiResponse<String> {
  match baidu_sync::cancel_baidu_sync_task(&state.db, task_id) {
    Ok(()) => ApiResponse::success("ok".to_string()),
    Err(err) => ApiResponse::error(err),
  }
}

#[tauri::command]
pub fn baidu_sync_pause(state: State<'_, AppState>, task_id: i64) -> ApiResponse<String> {
  match baidu_sync::pause_baidu_sync_task(&state.db, task_id) {
    Ok(()) => ApiResponse::success("ok".to_string()),
    Err(err) => ApiResponse::error(err),
  }
}

#[tauri::command]
pub fn baidu_sync_delete(state: State<'_, AppState>, task_id: i64) -> ApiResponse<String> {
  match baidu_sync::delete_baidu_sync_task(&state.db, task_id) {
    Ok(()) => ApiResponse::success("ok".to_string()),
    Err(err) => ApiResponse::error(err),
  }
}

#[tauri::command]
pub fn baidu_sync_update_settings(
  state: State<'_, AppState>,
  request: Option<BaiduSyncUpdateRequest>,
) -> ApiResponse<String> {
  let request = request.unwrap_or(BaiduSyncUpdateRequest {
    concurrency: None,
    target_path: None,
  });
  let mut settings = match baidu_sync::load_baidu_sync_settings(&state.db) {
    Ok(value) => value,
    Err(err) => return ApiResponse::error(err),
  };
  if let Some(concurrency) = request.concurrency {
    settings.concurrency = concurrency.max(1);
  }
  if let Some(target_path) = request.target_path {
    let trimmed = target_path.trim();
    let normalized = if trimmed.is_empty() {
      "/录播"
    } else {
      trimmed
    };
    settings.target_path = baidu_sync::normalize_baidu_path(normalized);
  }
  match baidu_sync::update_baidu_sync_settings(&state.db, &settings) {
    Ok(()) => ApiResponse::success("ok".to_string()),
    Err(err) => ApiResponse::error(err),
  }
}

#[tauri::command]
pub fn baidu_sync_account_login_status(
  state: State<'_, AppState>,
) -> ApiResponse<BaiduAccountLoginStatus> {
  let runtime = state
    .baidu_login_runtime
    .lock()
    .map_err(|_| "登录状态不可用".to_string());
  match runtime {
    Ok(runtime) => ApiResponse::success(snapshot_baidu_login_status(&runtime)),
    Err(err) => ApiResponse::error(err),
  }
}

#[tauri::command]
pub fn baidu_sync_account_login_start(
  state: State<'_, AppState>,
  request: BaiduAccountLoginRequest,
) -> ApiResponse<BaiduAccountLoginStatus> {
  let username = request.username.trim().to_string();
  let password = request.password;
  if username.is_empty() || password.trim().is_empty() {
    return ApiResponse::error("账号或密码不能为空".to_string());
  }

  {
    let mut runtime = match state.baidu_login_runtime.lock() {
      Ok(value) => value,
      Err(_) => return ApiResponse::error("登录状态不可用".to_string()),
    };
    if runtime.status == "RUNNING" || runtime.status == "WAIT_INPUT" {
      return ApiResponse::error("登录进行中，请先完成或取消".to_string());
    }
    reset_baidu_login_runtime(&mut runtime);
    runtime.status = "RUNNING".to_string();
  }

  let settings = match baidu_sync::load_baidu_sync_settings(&state.db) {
    Ok(value) => value,
    Err(err) => return ApiResponse::error(err),
  };
  let exec_path = resolve_baidu_exec_path(&settings.exec_path);
  let config_dir = env::var("BAIDUPCS_GO_CONFIG_DIR").unwrap_or_default();
  append_log(
    state.app_log_path.as_ref(),
    &format!(
      "baidu_account_login_start exec_path={} config_dir={}",
      exec_path.to_string_lossy(),
      config_dir
    ),
  );
  if !exec_path.exists() {
    let mut runtime = state.baidu_login_runtime.lock().unwrap();
    runtime.status = "FAILED".to_string();
    runtime.last_error = Some("BaiduPCS-Go 不存在".to_string());
    return ApiResponse::error("BaiduPCS-Go 不存在".to_string());
  }

  let mut command = Command::new(&exec_path);
  apply_no_window(&mut command);
  if !config_dir.is_empty() {
    command.env("BAIDUPCS_GO_CONFIG_DIR", &config_dir);
  }
  let mut child = match command
    .arg("login")
    .arg(format!("-username={}", username))
    .arg(format!("-password={}", password))
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
  {
    Ok(value) => value,
    Err(err) => {
      append_log(
        state.app_log_path.as_ref(),
        &format!("baidu_account_login_spawn_failed err={}", err),
      );
      let mut runtime = state.baidu_login_runtime.lock().unwrap();
      runtime.status = "FAILED".to_string();
      runtime.last_error = Some(format!("启动登录失败: {}", err));
      return ApiResponse::error(format!("启动登录失败: {}", err));
    }
  };

  let stdin = match child.stdin.take() {
    Some(value) => value,
    None => {
      let mut runtime = state.baidu_login_runtime.lock().unwrap();
      runtime.status = "FAILED".to_string();
      runtime.last_error = Some("无法获取登录输入通道".to_string());
      return ApiResponse::error("无法获取登录输入通道".to_string());
    }
  };
  let stdout = match child.stdout.take() {
    Some(value) => value,
    None => {
      let mut runtime = state.baidu_login_runtime.lock().unwrap();
      runtime.status = "FAILED".to_string();
      runtime.last_error = Some("无法获取登录输出".to_string());
      return ApiResponse::error("无法获取登录输出".to_string());
    }
  };
  let stderr = match child.stderr.take() {
    Some(value) => value,
    None => {
      let mut runtime = state.baidu_login_runtime.lock().unwrap();
      runtime.status = "FAILED".to_string();
      runtime.last_error = Some("无法获取登录错误输出".to_string());
      return ApiResponse::error("无法获取登录错误输出".to_string());
    }
  };

  {
    let mut runtime = state.baidu_login_runtime.lock().unwrap();
    runtime.stdin = Some(stdin);
    runtime.child = Some(child);
  }

  let runtime_for_stdout = Arc::clone(&state.baidu_login_runtime);
  thread::spawn(move || {
    read_login_output(runtime_for_stdout, stdout);
  });
  let runtime_for_stderr = Arc::clone(&state.baidu_login_runtime);
  thread::spawn(move || {
    read_login_output(runtime_for_stderr, stderr);
  });

  let runtime_for_wait = Arc::clone(&state.baidu_login_runtime);
  let db_for_wait = Arc::clone(&state.db);
  let log_for_wait = Arc::clone(&state.app_log_path);
  thread::spawn(move || {
    monitor_login_process(runtime_for_wait, db_for_wait, log_for_wait);
  });

  let runtime = state.baidu_login_runtime.lock().unwrap();
  ApiResponse::success(snapshot_baidu_login_status(&runtime))
}

#[tauri::command]
pub fn baidu_sync_account_login_input(
  state: State<'_, AppState>,
  request: BaiduAccountLoginInput,
) -> ApiResponse<String> {
  let input = request.input.trim();
  if input.is_empty() {
    return ApiResponse::error("请输入内容".to_string());
  }
  let mut runtime = match state.baidu_login_runtime.lock() {
    Ok(value) => value,
    Err(_) => return ApiResponse::error("登录状态不可用".to_string()),
  };
  let stdin = match runtime.stdin.as_mut() {
    Some(value) => value,
    None => return ApiResponse::error("登录未开始".to_string()),
  };
  if let Err(err) = stdin.write_all(format!("{}\n", input).as_bytes()) {
    runtime.last_error = Some(format!("发送输入失败: {}", err));
    return ApiResponse::error(format!("发送输入失败: {}", err));
  }
  let _ = stdin.flush();
  runtime.status = "RUNNING".to_string();
  runtime.prompt = None;
  ApiResponse::success("ok".to_string())
}

#[tauri::command]
pub fn baidu_sync_account_login_cancel(state: State<'_, AppState>) -> ApiResponse<String> {
  let mut runtime = match state.baidu_login_runtime.lock() {
    Ok(value) => value,
    Err(_) => return ApiResponse::error("登录状态不可用".to_string()),
  };
  if let Some(child) = runtime.child.as_mut() {
    let _ = child.kill();
  }
  runtime.child = None;
  runtime.stdin = None;
  runtime.status = "CANCELLED".to_string();
  runtime.prompt = None;
  runtime.last_error = Some("已取消".to_string());
  ApiResponse::success("ok".to_string())
}

#[tauri::command]
pub async fn baidu_sync_web_login(
  app: AppHandle,
  state: State<'_, AppState>,
) -> Result<ApiResponse<String>, String> {
  let url = match Url::parse(
    "https://passport.baidu.com/v2/?login&tpl=netdisk&u=https%3A%2F%2Fpan.baidu.com%2Fdisk%2Fmain",
  ) {
    Ok(value) => value,
    Err(err) => return Ok(ApiResponse::error(format!("登录地址无效: {}", err))),
  };
  let passport_url = Url::parse("https://passport.baidu.com/").unwrap_or_else(|_| url.clone());
  let label = format!("baidu_login_{}", app_log::now_millis());
  let app_log_path = Arc::clone(&state.app_log_path);
  append_log(
    &state.app_log_path,
    &format!("baidu_login_window_open label={}", label),
  );
  let (tx, rx) = oneshot::channel::<Result<String, String>>();
  let sender = Arc::new(Mutex::new(Some(tx)));

  let window = match WebviewWindowBuilder::new(&app, &label, WebviewUrl::External(url.clone()))
    .title("百度网盘登录")
    .inner_size(1200.0, 800.0)
    .resizable(true)
    .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
    .build()
  {
    Ok(value) => value,
    Err(err) => return Ok(ApiResponse::error(format!("打开登录窗口失败: {}", err))),
  };

  let window_for_event = window.clone();
  let sender_for_event = Arc::clone(&sender);
  let url_for_event = url.clone();
  let passport_for_event = passport_url.clone();
  window.on_window_event(move |event| {
    if !matches!(event, WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed) {
      return;
    }
    let sender = {
      let mut guard = match sender_for_event.lock() {
        Ok(value) => value,
        Err(_) => return,
      };
      guard.take()
    };
    if let Some(sender) = sender {
      let window_for_fetch = window_for_event.clone();
      let url_for_fetch = url_for_event.clone();
      let passport_for_fetch = passport_for_event.clone();
      tauri::async_runtime::spawn(async move {
        let result =
          fetch_baidu_cookie_header(&window_for_fetch, &url_for_fetch, &passport_for_fetch).await;
        let _ = sender.send(result);
      });
    }
  });

  let sender_for_poll = Arc::clone(&sender);
  let window_for_poll = window.clone();
  let url_for_poll = url.clone();
  let passport_for_poll = passport_url.clone();
  let app_log_for_poll = Arc::clone(&app_log_path);
  tauri::async_runtime::spawn(async move {
    let mut tick = 0usize;
    let mut missing_bduss_count = 0usize;
    let mut jump_count = 0usize;
    let mut cooldown_ticks = 0usize;
    let jump_urls = [
      "https://tieba.baidu.com/",
      "https://passport.baidu.com/v3/login/api/auth/?tpl=netdisk&return_type=3",
      "https://pan.baidu.com/disk/home",
    ];
    loop {
      tick += 1;
      let sender_available = sender_for_poll
        .lock()
        .map(|guard| guard.is_some())
        .unwrap_or(false);
      if !sender_available {
        break;
      }
      let result = fetch_baidu_cookie_header_with_flag(
        &window_for_poll,
        &url_for_poll,
        &passport_for_poll,
      )
      .await;
      match result {
        Ok((header, has_auth, names)) => {
          let has_stoken = names.iter().any(|name| {
            let upper = name.to_uppercase();
            upper == "STOKEN" || upper == "STOKEN_BFESS"
          });
          if tick % 5 == 0 {
            append_log(
              app_log_for_poll.as_ref(),
              &format!(
                "baidu_login_cookie_snapshot count={} has_auth={} has_stoken={} names={}",
                names.len(),
                has_auth,
                has_stoken,
                summarize_cookie_names(&names)
              ),
            );
          }
          if has_auth {
            let sender = sender_for_poll
              .lock()
              .ok()
              .and_then(|mut guard| guard.take());
            if let Some(sender) = sender {
              append_log(app_log_for_poll.as_ref(), "baidu_login_cookie_detected");
              let _ = sender.send(Ok(header));
              let _ = window_for_poll.close();
            }
            break;
          }
          if cooldown_ticks > 0 {
            cooldown_ticks = cooldown_ticks.saturating_sub(1);
          }
          if !has_stoken {
            missing_bduss_count = 0;
          }
          if has_stoken && cooldown_ticks == 0 {
            missing_bduss_count += 1;
            if missing_bduss_count >= 3 && jump_count < 2 && jump_count < jump_urls.len() {
              let target_url = jump_urls[jump_count];
              missing_bduss_count = 0;
              jump_count += 1;
              cooldown_ticks = 3;
              if let Err(err) = window_for_poll.eval(&format!(
                "window.location.replace('{}')",
                target_url
              )) {
                append_log(
                  app_log_for_poll.as_ref(),
                  &format!("baidu_login_jump_fail url={} err={}", target_url, err),
                );
              } else {
                append_log(
                  app_log_for_poll.as_ref(),
                  &format!(
                    "baidu_login_jump_ok url={} count={}",
                    target_url, jump_count
                  ),
                );
              }
            }
          }
        }
        Err(err) => {
          if tick % 5 == 0 {
            append_log(
              app_log_for_poll.as_ref(),
              &format!("baidu_login_cookie_check_fail err={}", err),
            );
          }
        }
      }
      sleep(Duration::from_secs(1)).await;
    }
  });

  match rx.await {
    Ok(Ok(cookie)) => Ok(ApiResponse::success(cookie)),
    Ok(Err(err)) => Ok(ApiResponse::error(err)),
    Err(_) => Ok(ApiResponse::error("登录窗口已关闭".to_string())),
  }
}

async fn fetch_baidu_cookie_header(
  window: &tauri::WebviewWindow,
  url: &Url,
  passport_url: &Url,
) -> Result<String, String> {
  fetch_baidu_cookie_header_with_flag(window, url, passport_url)
    .await
    .map(|(header, _, _)| header)
}

async fn fetch_baidu_cookie_header_with_flag(
  window: &tauri::WebviewWindow,
  url: &Url,
  passport_url: &Url,
) -> Result<(String, bool, Vec<String>), String> {
  let cookie_map = fetch_baidu_cookie_map(window, url, passport_url).await?;
  build_cookie_header_with_flag(&cookie_map)
}

async fn fetch_baidu_cookie_map(
  window: &tauri::WebviewWindow,
  url: &Url,
  passport_url: &Url,
) -> Result<HashMap<String, String>, String> {
  let (tx, rx) = oneshot::channel::<HashMap<String, String>>();
  let window_clone = window.clone();
  let url_clone = url.clone();
  let passport_clone = passport_url.clone();
  window
    .run_on_main_thread(move || {
      let map = collect_baidu_cookie_map(&window_clone, &url_clone, &passport_clone);
      let _ = tx.send(map);
    })
    .map_err(|err| format!("读取 Cookie 失败: {}", err))?;
  rx.await.map_err(|_| "读取 Cookie 失败".to_string())
}

fn build_cookie_header_with_flag(
  map: &HashMap<String, String>,
) -> Result<(String, bool, Vec<String>), String> {
  if map.is_empty() {
    return Err("未获取到 Cookie".to_string());
  }
  let mut names: Vec<String> = map.keys().cloned().collect();
  names.sort();
  let has_auth = names.iter().any(|name| name.to_uppercase().contains("BDUSS"));
  let header = names
    .iter()
    .map(|key| {
      let value = map.get(key).map(|item| item.as_str()).unwrap_or("");
      format!("{}={}", key, value)
    })
    .collect::<Vec<_>>()
    .join("; ");
  if header.trim().is_empty() {
    return Err("未获取到 Cookie".to_string());
  }
  Ok((header, has_auth, names))
}

fn collect_baidu_cookie_map(
  window: &tauri::WebviewWindow,
  url: &Url,
  passport_url: &Url,
) -> HashMap<String, String> {
  let mut cookies = Vec::new();
  if let Ok(list) = window.cookies_for_url(url.clone()) {
    cookies.extend(list);
  }
  if let Ok(list) = window.cookies_for_url(passport_url.clone()) {
    cookies.extend(list);
  }
  if cookies.is_empty() {
    if let Ok(list) = window.cookies() {
      cookies.extend(list);
    }
  }
  let mut map: HashMap<String, String> = HashMap::new();
  for item in cookies {
    if !is_baidu_cookie(&item) {
      continue;
    }
    let name = item.name().trim();
    if name.is_empty() {
      continue;
    }
    map.insert(name.to_string(), item.value().to_string());
  }
  map
}

fn is_baidu_cookie(item: &cookie::Cookie<'static>) -> bool {
  let name = item.name().to_ascii_lowercase();
  if name.contains("bduss") || name.contains("stoken") || name.contains("baiduid") {
    return true;
  }
  if let Some(domain) = item.domain() {
    return domain.ends_with("baidu.com");
  }
  false
}

fn summarize_cookie_names(names: &[String]) -> String {
  let mut list = names.to_vec();
  list.sort();
  let preview: Vec<String> = list.into_iter().take(12).collect();
  preview.join(",")
}

fn has_baidu_busy_output(output: &[String]) -> bool {
  output.iter().any(|line| line.contains("50052") || line.contains("系统繁忙"))
}

fn snapshot_baidu_login_status(runtime: &BaiduLoginRuntime) -> BaiduAccountLoginStatus {
  let status = if runtime.status.is_empty() {
    "IDLE".to_string()
  } else {
    runtime.status.clone()
  };
  BaiduAccountLoginStatus {
    status,
    prompt: runtime.prompt.clone(),
    captcha_path: runtime.captcha_path.clone(),
    captcha_url: runtime.captcha_url.clone(),
    output: runtime.output.clone(),
    last_error: runtime.last_error.clone(),
  }
}

fn reset_baidu_login_runtime(runtime: &mut BaiduLoginRuntime) {
  runtime.status.clear();
  runtime.prompt = None;
  runtime.captcha_path = None;
  runtime.captcha_url = None;
  runtime.output.clear();
  runtime.last_error = None;
  runtime.child = None;
  runtime.stdin = None;
}

fn resolve_baidu_exec_path(exec_path: &str) -> PathBuf {
  if !exec_path.trim().is_empty() {
    return PathBuf::from(exec_path.trim());
  }
  config::resolve_baidu_pcs_path()
}

fn read_login_output<R>(runtime: Arc<Mutex<BaiduLoginRuntime>>, reader: R)
where
  R: std::io::Read + Send + 'static,
{
  let mut expect_captcha_path = false;
  let mut expect_captcha_url = false;
  let buf = BufReader::new(reader);
  for line in buf.lines().flatten() {
    let text = line.trim_end().to_string();
    if text.is_empty() {
      continue;
    }
    let mut runtime = match runtime.lock() {
      Ok(value) => value,
      Err(_) => continue,
    };
    push_login_output(&mut runtime, &text);

    if expect_captcha_path {
      runtime.captcha_path = Some(text.clone());
      expect_captcha_path = false;
    }
    if expect_captcha_url {
      runtime.captcha_url = Some(text.clone());
      expect_captcha_url = false;
    }
    if text.contains("打开以下路径") {
      expect_captcha_path = true;
    }
    if text.contains("或者打开以下的网址") {
      expect_captcha_url = true;
    }
    if let Some(prompt) = detect_login_prompt(&text) {
      runtime.prompt = Some(prompt);
      runtime.status = "WAIT_INPUT".to_string();
    }
    if text.contains("百度帐号登录成功") {
      runtime.status = "SUCCESS".to_string();
      runtime.prompt = None;
    }
    if text.contains("错误代码") || text.contains("登录失败") {
      runtime.last_error = Some(text.clone());
    }
  }
}

fn detect_login_prompt(text: &str) -> Option<String> {
  if text.contains("请输入验证码") {
    return Some("验证码".to_string());
  }
  if text.contains("请输入接收到的验证码") {
    return Some("短信/邮箱验证码".to_string());
  }
  if text.contains("请输入验证方式") {
    return Some("验证方式(1手机/2邮箱)".to_string());
  }
  if text.contains("请输入百度用户名") {
    return Some("用户名".to_string());
  }
  if text.contains("请输入密码") {
    return Some("密码".to_string());
  }
  None
}

fn push_login_output(runtime: &mut BaiduLoginRuntime, line: &str) {
  runtime.output.push(line.to_string());
  if runtime.output.len() > 200 {
    let drain = runtime.output.len() - 200;
    runtime.output.drain(0..drain);
  }
}

fn monitor_login_process(
  runtime: Arc<Mutex<BaiduLoginRuntime>>,
  db: Arc<crate::db::Db>,
  app_log_path: Arc<PathBuf>,
) {
  loop {
    let mut finished = false;
    let mut success = false;
    {
      let mut runtime = match runtime.lock() {
        Ok(value) => value,
        Err(_) => return,
      };
      if let Some(child) = runtime.child.as_mut() {
        match child.try_wait() {
          Ok(Some(status)) => {
            finished = true;
            success = status.success();
            runtime.child = None;
            runtime.stdin = None;
            runtime.prompt = None;
            if success {
              runtime.status = "SUCCESS".to_string();
            } else {
              runtime.status = "FAILED".to_string();
              runtime.last_error = Some(format!("登录失败: {}", status));
            }
          }
          Ok(None) => {}
          Err(err) => {
            finished = true;
            runtime.child = None;
            runtime.stdin = None;
            runtime.prompt = None;
            runtime.status = "FAILED".to_string();
            runtime.last_error = Some(format!("登录异常: {}", err));
          }
        }
      } else {
        return;
      }
    }

    if finished {
      if success {
        let now = now_rfc3339();
        let placeholder = baidu_sync::BaiduLoginInfo {
          status: "LOGGED_IN".to_string(),
          uid: None,
          username: None,
          login_type: Some("account".to_string()),
          login_time: Some(now.clone()),
          last_check_time: Some(now),
        };
        let _ = baidu_sync::upsert_baidu_login_info(&db, &placeholder);
        match baidu_sync::check_baidu_login(&db) {
          Ok(info) => {
            if info.status != "LOGGED_IN" {
              let busy_output = runtime
                .lock()
                .map(|guard| has_baidu_busy_output(&guard.output))
                .unwrap_or(false);
              if busy_output {
                let now = now_rfc3339();
                let mut pending = placeholder.clone();
                pending.last_check_time = Some(now);
                let _ = baidu_sync::upsert_baidu_login_info(&db, &pending);
                if let Ok(mut runtime) = runtime.lock() {
                  runtime.status = "SUCCESS".to_string();
                  runtime.last_error = Some("系统繁忙，登录状态稍后校验".to_string());
                }
              } else if let Ok(mut runtime) = runtime.lock() {
                runtime.status = "FAILED".to_string();
                runtime.last_error = Some("未检测到登录状态".to_string());
              }
            }
          }
          Err(err) => {
            if err.contains("50052") || err.contains("系统繁忙") {
              let now = now_rfc3339();
              let mut pending = placeholder.clone();
              pending.last_check_time = Some(now);
              let _ = baidu_sync::upsert_baidu_login_info(&db, &pending);
              if let Ok(mut runtime) = runtime.lock() {
                runtime.status = "SUCCESS".to_string();
                runtime.last_error = Some("系统繁忙，登录状态稍后校验".to_string());
              }
              append_log(app_log_path.as_ref(), "baidu_account_login_busy");
              append_log(app_log_path.as_ref(), "baidu_account_login_done");
              return;
            }
            if let Ok(mut runtime) = runtime.lock() {
              runtime.status = "FAILED".to_string();
              runtime.last_error = Some(err);
            }
          }
        }
      }
      append_log(app_log_path.as_ref(), "baidu_account_login_done");
      return;
    }
    thread::sleep(Duration::from_millis(300));
  }
}
