use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use chrono::{DateTime, Utc};
use flate2::read::GzDecoder;
use rand::rngs::OsRng;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, SET_COOKIE, USER_AGENT};
use reqwest::Client;
use rsa::{pkcs8::DecodePublicKey, Oaep, RsaPublicKey};
use serde_json::Value;
use sha2::Sha256;
use tokio::sync::Mutex;

use crate::bilibili::client::BilibiliClient;
use crate::db::Db;
use crate::login_store::{extract_cookie, extract_csrf, AuthInfo, LoginStore};
use crate::utils::append_log;

const PUBLIC_KEY_PEM: &str = "-----BEGIN PUBLIC KEY-----\n\
MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQDLgd2OAkcGVtoE3ThUREbio0Eg\n\
Uc/prcajMKXvkCKFCWhJYJcLkcM2DKKcSeFpD/j6Boy538YXnR6VhcuUJOhH2x71\n\
nzPjfdTcqMz7djHum0qSZA0AyCBDABUqCrfNgCiJ00Ra7GmRj+YCK1NJEuewlb40\n\
JNrRuoEUXpabUzGB8QIDAQAB\n\
-----END PUBLIC KEY-----";
const DEFAULT_COOKIE_REFRESH_MINUTES: i64 = 60;

#[derive(Clone, Copy)]
struct CookieRefreshInfo {
  refresh: bool,
  timestamp: i64,
}

#[derive(Default)]
struct LoginCheckInfo {
  code: i64,
  is_login: bool,
  message: String,
}

fn refresh_lock() -> &'static Mutex<()> {
  use std::sync::OnceLock;
  static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
  LOCK.get_or_init(|| Mutex::new(()))
}

pub fn start_cookie_refresh_loop(
  db: std::sync::Arc<Db>,
  bilibili: std::sync::Arc<BilibiliClient>,
  login_store: std::sync::Arc<LoginStore>,
  log_path: std::sync::Arc<std::path::PathBuf>,
) {
  tauri::async_runtime::spawn(async move {
    loop {
      let result = refresh_all_cookies_if_needed(
        bilibili.as_ref(),
        login_store.as_ref(),
        db.as_ref(),
        log_path.as_ref(),
      )
      .await;
      if let Err(err) = result {
        append_log(
          log_path.as_ref(),
          &format!("cookie_refresh_loop_fail err={}", err),
        );
      }
      tokio::time::sleep(std::time::Duration::from_secs(
        (DEFAULT_COOKIE_REFRESH_MINUTES.max(1) as u64) * 60,
      ))
      .await;
    }
  });
}

pub async fn refresh_cookie(
  bilibili: &BilibiliClient,
  login_store: &LoginStore,
  db: &Db,
  log_path: &Path,
) -> Result<AuthInfo, String> {
  let _guard = refresh_lock().lock().await;
  let active_uid = login_store
    .get_active_account_uid(db)
    .map_err(|err| format!("读取当前账号失败: {}", err))?;
  let target_uid = active_uid.ok_or_else(|| "请先登录".to_string())?;
  let auth_info = refresh_cookie_by_uid_inner(
    bilibili,
    login_store,
    db,
    log_path,
    target_uid,
    false,
  )
  .await?
  .ok_or_else(|| "刷新后登录信息无效".to_string())?;
  Ok(auth_info)
}

pub async fn refresh_cookie_by_uid(
  bilibili: &BilibiliClient,
  login_store: &LoginStore,
  db: &Db,
  log_path: &Path,
  user_id: i64,
) -> Result<AuthInfo, String> {
  let _guard = refresh_lock().lock().await;
  let auth_info = refresh_cookie_by_uid_inner(
    bilibili,
    login_store,
    db,
    log_path,
    user_id,
    false,
  )
  .await?
  .ok_or_else(|| "刷新后登录信息无效".to_string())?;
  Ok(auth_info)
}

pub async fn refresh_cookie_if_needed(
  bilibili: &BilibiliClient,
  login_store: &LoginStore,
  db: &Db,
  log_path: &Path,
) -> Result<bool, String> {
  let _guard = refresh_lock().lock().await;
  let active_uid = login_store
    .get_active_account_uid(db)
    .map_err(|err| format!("读取当前账号失败: {}", err))?;
  let Some(target_uid) = active_uid else {
    return Ok(false);
  };
  refresh_cookie_by_uid_inner(bilibili, login_store, db, log_path, target_uid, true)
    .await
    .map(|value| value.is_some())
}

pub async fn refresh_all_cookies_if_needed(
  bilibili: &BilibiliClient,
  login_store: &LoginStore,
  db: &Db,
  log_path: &Path,
) -> Result<(), String> {
  let _guard = refresh_lock().lock().await;
  let user_ids = login_store
    .list_account_user_ids(db)
    .map_err(|err| format!("读取账号列表失败: {}", err))?;
  if user_ids.is_empty() {
    return Ok(());
  }
  for user_id in user_ids {
    let result =
      refresh_cookie_by_uid_inner(bilibili, login_store, db, log_path, user_id, true).await;
    match result {
      Ok(Some(_)) => {
        append_log(log_path, &format!("cookie_refresh_account_ok uid={}", user_id));
      }
      Ok(None) => {
        append_log(log_path, &format!("cookie_refresh_account_skip uid={}", user_id));
      }
      Err(err) => {
        append_log(
          log_path,
          &format!("cookie_refresh_account_fail uid={} err={}", user_id, err),
        );
      }
    }
  }
  Ok(())
}

async fn refresh_cookie_by_uid_inner(
  bilibili: &BilibiliClient,
  login_store: &LoginStore,
  db: &Db,
  log_path: &Path,
  user_id: i64,
  only_if_needed: bool,
) -> Result<Option<AuthInfo>, String> {
  append_log(
    log_path,
    &format!(
      "cookie_refresh_start uid={} mode={}",
      user_id,
      if only_if_needed { "if_needed" } else { "force" }
    ),
  );
  let login_data = match login_store
    .load_login_data_by_uid(db, user_id)
    .map_err(|err| format!("读取登录信息失败: {}", err))?
  {
    Some(data) => data,
    None => return Ok(None),
  };
  let cookie = extract_cookie(&login_data).ok_or_else(|| "登录信息缺少Cookie".to_string())?;
  let csrf = extract_csrf(&cookie).ok_or_else(|| "登录信息缺少CSRF".to_string())?;
  let refresh_token = login_store
    .load_refresh_token_by_uid(db, user_id)
    .map_err(|err| format!("读取refresh_token失败: {}", err))?
    .filter(|token| !token.trim().is_empty())
    .ok_or_else(|| "登录信息缺少refresh_token".to_string())?;

  let client = Client::new();
  let expired = load_login_expire_time_by_uid(db, user_id)?
    .map(|expire_time| expire_time <= Utc::now())
    .unwrap_or(false);
  let login_invalid = match check_login_status(&client, bilibili, &cookie).await {
    Ok(info) => {
      append_log(
        log_path,
        &format!(
          "cookie_login_check uid={} code={} is_login={} message={}",
          user_id, info.code, info.is_login, info.message
        ),
      );
      info.code == -101 || !info.is_login
    }
    Err(err) => {
      append_log(
        log_path,
        &format!("cookie_login_check_fail uid={} err={}", user_id, err),
      );
      false
    }
  };
  let info = fetch_cookie_refresh_info(&client, bilibili, &cookie, &csrf).await?;
  append_log(
    log_path,
    &format!(
      "cookie_refresh_check uid={} refresh={} expired={} timestamp={}",
      user_id, info.refresh, expired, info.timestamp
    ),
  );
  if only_if_needed && !info.refresh && !expired && !login_invalid {
    return Ok(None);
  }
  let correspond_path = build_correspond_path(info.timestamp)?;
  let refresh_csrf = fetch_refresh_csrf(&client, &cookie, &correspond_path).await?;
  let old_refresh_token = refresh_token.clone();
  let (new_cookie, new_refresh_token) = do_refresh_cookie(
    bilibili,
    &client,
    &cookie,
    &csrf,
    &refresh_token,
    &old_refresh_token,
    refresh_csrf,
  )
  .await?;
  let new_login_data = build_refreshed_login_data(&login_data, new_cookie, new_refresh_token);
  login_store
    .save_login_info_without_switching(db, &new_login_data)
    .map_err(|err| format!("保存刷新Cookie失败: {}", err))?;
  let auth_info = login_store
    .load_auth_info_by_uid(db, user_id)
    .map_err(|err| format!("读取刷新登录信息失败: {}", err))?;
  append_log(log_path, &format!("cookie_refresh_ok uid={}", user_id));
  Ok(auth_info)
}

fn load_login_expire_time_by_uid(db: &Db, user_id: i64) -> Result<Option<DateTime<Utc>>, String> {
  db.with_conn(|conn| {
    let mut stmt = conn.prepare("SELECT expire_time FROM login_info WHERE user_id = ?1 LIMIT 1")?;
    let mut rows = stmt.query([user_id])?;
    if let Some(row) = rows.next()? {
      let expire_time: Option<String> = row.get(0)?;
      if let Some(expire_time) = expire_time {
        if let Ok(parsed) = DateTime::parse_from_rfc3339(&expire_time) {
          return Ok(Some(parsed.with_timezone(&Utc)));
        }
      }
    }
    Ok(None)
  })
  .map_err(|err| err.to_string())
}

fn build_headers(cookie: Option<&str>) -> Result<HeaderMap, String> {
  let mut headers = HeaderMap::new();
  headers.insert(
    USER_AGENT,
    HeaderValue::from_static(
      "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/132.0.0.0 Safari/537.36 Edg/132.0.0.0",
    ),
  );
  headers.insert(ACCEPT, HeaderValue::from_static("application/json, text/javascript, */*; q=0.01"));
  headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("zh-CN"));
  if let Some(cookie) = cookie {
    if !cookie.trim().is_empty() {
      headers.insert(
        "Cookie",
        HeaderValue::from_str(cookie).map_err(|_| "无效的Cookie".to_string())?,
      );
    }
  }
  Ok(headers)
}

async fn check_login_status(
  client: &Client,
  bilibili: &BilibiliClient,
  cookie: &str,
) -> Result<LoginCheckInfo, String> {
  let url = format!("{}/x/web-interface/nav", bilibili.base_url());
  let response = client
    .get(&url)
    .headers(build_headers(Some(cookie))?)
    .send()
    .await
    .map_err(|err| format!("登录校验请求失败: {}", err))?;
  let body: Value = response
    .json()
    .await
    .map_err(|err| format!("登录校验解析失败: {}", err))?;
  let code = body.get("code").and_then(|value| value.as_i64()).unwrap_or(0);
  let message = body
    .get("message")
    .and_then(|value| value.as_str())
    .unwrap_or_default()
    .to_string();
  let is_login = body
    .get("data")
    .and_then(|value| value.get("isLogin"))
    .and_then(|value| value.as_bool())
    .unwrap_or(code == 0);

  Ok(LoginCheckInfo {
    code,
    is_login,
    message,
  })
}

async fn fetch_cookie_refresh_info(
  client: &Client,
  bilibili: &BilibiliClient,
  cookie: &str,
  csrf: &str,
) -> Result<CookieRefreshInfo, String> {
  let info_url = format!(
    "{}/x/passport-login/web/cookie/info",
    bilibili.passport_base_url()
  );
  let info_value: Value = client
    .get(&info_url)
    .headers(build_headers(Some(cookie))?)
    .query(&[("csrf", csrf.to_string())])
    .send()
    .await
    .map_err(|err| format!("刷新信息请求失败: {}", err))?
    .json()
    .await
    .map_err(|err| format!("刷新信息解析失败: {}", err))?;
  let info_code = info_value.get("code").and_then(|value| value.as_i64()).unwrap_or(0);
  if info_code != 0 {
    return Err(format!("刷新信息返回异常 (code: {})", info_code));
  }
  let info_data = info_value.get("data").unwrap_or(&info_value);
  let timestamp = info_data
    .get("timestamp")
    .and_then(|value| value.as_i64())
    .ok_or_else(|| "刷新信息缺少timestamp".to_string())?;
  let refresh_flag = info_data
    .get("refresh")
    .and_then(|value| value.as_bool())
    .unwrap_or(false);
  Ok(CookieRefreshInfo {
    refresh: refresh_flag,
    timestamp,
  })
}

async fn do_refresh_cookie(
  bilibili: &BilibiliClient,
  client: &Client,
  cookie: &str,
  csrf: &str,
  refresh_token: &str,
  old_refresh_token: &str,
  refresh_csrf: String,
) -> Result<(String, String), String> {
  let refresh_url = format!(
    "{}/x/passport-login/web/cookie/refresh",
    bilibili.passport_base_url()
  );
  let refresh_response = client
    .post(&refresh_url)
    .headers(build_headers(Some(cookie))?)
    .query(&[
      ("csrf", csrf.to_string()),
      ("refresh_csrf", refresh_csrf),
      ("refresh_token", refresh_token.to_string()),
      ("source", "main_web".to_string()),
    ])
    .send()
    .await
    .map_err(|err| format!("刷新Cookie请求失败: {}", err))?;
  let refresh_headers = refresh_response.headers().clone();
  let refresh_body: Value = refresh_response
    .json()
    .await
    .map_err(|err| format!("刷新Cookie解析失败: {}", err))?;
  let refresh_code = refresh_body
    .get("code")
    .and_then(|value| value.as_i64())
    .unwrap_or(0);
  if refresh_code != 0 {
    return Err(format!("刷新Cookie失败 (code: {})", refresh_code));
  }
  let new_refresh_token = refresh_body
    .get("data")
    .and_then(|value| value.get("refresh_token"))
    .and_then(|value| value.as_str())
    .ok_or_else(|| "刷新Cookie缺少refresh_token".to_string())?
    .to_string();
  let new_cookie = merge_cookie(cookie, &collect_set_cookies(&refresh_headers));
  let confirm_csrf = extract_csrf(&new_cookie).unwrap_or_else(|| csrf.to_string());

  let confirm_url = format!(
    "{}/x/passport-login/web/confirm/refresh",
    bilibili.passport_base_url()
  );
  let confirm_value: Value = client
    .post(&confirm_url)
    .headers(build_headers(Some(&new_cookie))?)
    .query(&[
      ("csrf", confirm_csrf),
      ("refresh_token", old_refresh_token.to_string()),
    ])
    .send()
    .await
    .map_err(|err| format!("确认刷新请求失败: {}", err))?
    .json()
    .await
    .map_err(|err| format!("确认刷新解析失败: {}", err))?;
  let confirm_code = confirm_value
    .get("code")
    .and_then(|value| value.as_i64())
    .unwrap_or(0);
  if confirm_code != 0 {
    return Err(format!("确认刷新失败 (code: {})", confirm_code));
  }
  Ok((new_cookie, new_refresh_token))
}

fn build_correspond_path(timestamp: i64) -> Result<String, String> {
  let message = format!("refresh_{}", timestamp);
  let public_key = RsaPublicKey::from_public_key_pem(PUBLIC_KEY_PEM)
    .map_err(|err| format!("加载公钥失败: {}", err))?;
  let mut rng = OsRng;
  let padding = Oaep::new::<Sha256>();
  let encrypted = public_key
    .encrypt(&mut rng, padding, message.as_bytes())
    .map_err(|err| format!("生成签名失败: {}", err))?;
  Ok(bytes_to_hex(&encrypted))
}

async fn fetch_refresh_csrf(
  client: &Client,
  cookie: &str,
  correspond_path: &str,
) -> Result<String, String> {
  let url = format!("https://www.bilibili.com/correspond/1/{}", correspond_path);
  let mut headers = build_headers(Some(cookie))?;
  headers.insert(
    ACCEPT,
    HeaderValue::from_static("text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"),
  );
  headers.insert("Accept-Encoding", HeaderValue::from_static("identity"));
  headers.insert("Origin", HeaderValue::from_static("https://www.bilibili.com"));
  headers.insert("Referer", HeaderValue::from_static("https://www.bilibili.com/"));
  let response = client
    .get(&url)
    .headers(headers)
    .send()
    .await
    .map_err(|err| format!("获取refresh_csrf失败: {}", err))?;
  let status = response.status();
  let content_type = response
    .headers()
    .get(reqwest::header::CONTENT_TYPE)
    .and_then(|value| value.to_str().ok())
    .unwrap_or("unknown")
    .to_string();
  let content_encoding = response
    .headers()
    .get(reqwest::header::CONTENT_ENCODING)
    .and_then(|value| value.to_str().ok())
    .unwrap_or("identity")
    .to_string();
  let body = response
    .bytes()
    .await
    .map_err(|err| format!("读取refresh_csrf失败: {}", err))?;
  let html = decode_refresh_html(&body, &content_encoding)?;
  if !status.is_success() {
    return Err(format!("获取refresh_csrf失败: status={}", status.as_u16()));
  }
  extract_refresh_csrf(&html).ok_or_else(|| {
    let len = html.len();
    let has_1name = html.contains("1-name");
    let has_refresh_key = html.contains("refresh_csrf");
    let login_hint = looks_like_login_page(&html);
    let title = extract_html_title(&html).unwrap_or_else(|| "unknown".to_string());
    let snippet = compact_snippet(&html, 160);
    let detail = format!(
      "status={} content_type={} content_encoding={} len={} has_1name={} has_refresh_csrf={} login_hint={} title={} snippet={}",
      status.as_u16(),
      content_type,
      content_encoding,
      len,
      has_1name,
      has_refresh_key,
      login_hint,
      title,
      snippet
    );
    if login_hint {
      format!("解析refresh_csrf失败: Cookie可能已失效 ({})", detail)
    } else {
      format!("解析refresh_csrf失败 ({})", detail)
    }
  })
}

fn decode_refresh_html(body: &[u8], content_encoding: &str) -> Result<String, String> {
  let mut decoded_bytes = if content_encoding.contains("gzip") || body.starts_with(&[0x1f, 0x8b]) {
    let mut decoder = GzDecoder::new(body);
    let mut decoded = Vec::new();
    decoder
      .read_to_end(&mut decoded)
      .map_err(|err| format!("读取refresh_csrf失败: {}", err))?;
    decoded
  } else {
    return Ok(String::from_utf8_lossy(body).to_string());
  };

  if decoded_bytes.starts_with(&[0x1f, 0x8b]) {
    let mut decoder = GzDecoder::new(decoded_bytes.as_slice());
    let mut decoded = Vec::new();
    decoder
      .read_to_end(&mut decoded)
      .map_err(|err| format!("读取refresh_csrf失败: {}", err))?;
    decoded_bytes = decoded;
  }

  Ok(String::from_utf8_lossy(&decoded_bytes).to_string())
}

fn extract_refresh_csrf(html: &str) -> Option<String> {
  let trimmed = html.trim();
  if trimmed.starts_with('{') {
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
      if let Some(token) = extract_refresh_csrf_from_json(&value) {
        return Some(token);
      }
    }
  }
  if let Some(token) = extract_refresh_csrf_from_marker(trimmed, "refresh_csrf") {
    return Some(token);
  }
  extract_refresh_csrf_from_div(html)
}

fn build_refreshed_login_data(
  login_data: &Value,
  cookie: String,
  refresh_token: String,
) -> Value {
  let mut new_login_data = login_data.clone();
  if let Value::Object(ref mut obj) = new_login_data {
    obj.insert("cookie".to_string(), Value::String(cookie));
    obj.insert(
      "refresh_token".to_string(),
      Value::String(refresh_token),
    );
    obj.remove("url");
  }
  new_login_data
}

fn extract_refresh_csrf_from_json(value: &Value) -> Option<String> {
  value
    .get("data")
    .and_then(|data| data.get("refresh_csrf"))
    .and_then(|value| value.as_str())
    .map(|value| value.to_string())
    .or_else(|| {
      value
        .get("refresh_csrf")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
    })
}

fn extract_refresh_csrf_from_marker(input: &str, key: &str) -> Option<String> {
  let pos = input.find(key)?;
  let rest = &input[pos + key.len()..];
  let sep_pos = rest.find(|c: char| c == ':' || c == '=')?;
  let rest = &rest[sep_pos + 1..];
  let quote_pos = rest.find(|c: char| c == '"' || c == '\'')?;
  let quote = rest[quote_pos..].chars().next()?;
  let value_start = &rest[quote_pos + quote.len_utf8()..];
  let end = value_start.find(quote)?;
  let value = value_start[..end].trim();
  if value.is_empty() {
    return None;
  }
  Some(value.to_string())
}

fn extract_refresh_csrf_from_div(html: &str) -> Option<String> {
  let marker_double = "id=\"1-name\"";
  let marker_single = "id='1-name'";
  let (start, marker) = if let Some(start) = html.find(marker_double) {
    (start, marker_double)
  } else {
    let start = html.find(marker_single)?;
    (start, marker_single)
  };
  let rest = &html[start + marker.len()..];
  let gt_pos = rest.find('>')?;
  let value_start = &rest[gt_pos + 1..];
  let end = value_start.find("</div>")?;
  let value = strip_html_tags(&value_start[..end]).trim().to_string();
  if value.is_empty() {
    return None;
  }
  Some(value)
}

fn strip_html_tags(input: &str) -> String {
  let mut output = String::with_capacity(input.len());
  let mut inside = false;
  for ch in input.chars() {
    if ch == '<' {
      inside = true;
      continue;
    }
    if ch == '>' {
      inside = false;
      continue;
    }
    if !inside {
      output.push(ch);
    }
  }
  output
}

fn looks_like_login_page(html: &str) -> bool {
  let lower = html.to_ascii_lowercase();
  lower.contains("passport") || lower.contains("login") || lower.contains("账号")
}

fn extract_html_title(html: &str) -> Option<String> {
  let lower = html.to_ascii_lowercase();
  let start = lower.find("<title")?;
  let rest = &html[start..];
  let gt_pos = rest.find('>')?;
  let content = &rest[gt_pos + 1..];
  let end = content.find("</title>")?;
  let value = content[..end].trim();
  if value.is_empty() {
    return None;
  }
  Some(value.to_string())
}

fn compact_snippet(input: &str, max_len: usize) -> String {
  let mut output = String::new();
  let mut last_space = false;
  for ch in input.chars() {
    if output.len() >= max_len {
      break;
    }
    if ch.is_whitespace() {
      if !last_space {
        output.push(' ');
        last_space = true;
      }
      continue;
    }
    last_space = false;
    if ch.is_ascii() {
      output.push(ch);
    }
  }
  output.trim().to_string()
}

fn collect_set_cookies(headers: &HeaderMap) -> Vec<String> {
  headers
    .get_all(SET_COOKIE)
    .iter()
    .filter_map(|value| value.to_str().ok())
    .map(|value| value.to_string())
    .collect()
}

fn merge_cookie(base: &str, set_cookies: &[String]) -> String {
  let mut map = parse_cookie_map(base);
  for value in set_cookies {
    if let Some(pair) = value.split(';').next() {
      if let Some((name, val)) = pair.split_once('=') {
        let key = name.trim().to_string();
        let entry = format!("{}={}", key, val.trim());
        map.insert(key, entry);
      }
    }
  }
  let mut cookies: Vec<String> = map.into_values().collect();
  cookies.sort();
  cookies.join("; ")
}

fn parse_cookie_map(cookie: &str) -> HashMap<String, String> {
  let mut map = HashMap::new();
  for part in cookie.split(';') {
    let trimmed = part.trim();
    if trimmed.is_empty() {
      continue;
    }
    if let Some((name, value)) = trimmed.split_once('=') {
      let key = name.trim().to_string();
      map.insert(key.clone(), format!("{}={}", key, value.trim()));
    }
  }
  map
}

fn bytes_to_hex(bytes: &[u8]) -> String {
  let mut output = String::with_capacity(bytes.len() * 2);
  for byte in bytes {
    output.push_str(&format!("{:02x}", byte));
  }
  output
}
