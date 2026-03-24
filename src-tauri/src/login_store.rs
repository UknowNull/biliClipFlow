use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use chrono::{DateTime, Duration, TimeZone, Utc};
use rusqlite::OptionalExtension;
use serde::Serialize;
use serde_json::{json, Value};
use thiserror::Error;
use url::Url;

use crate::db::Db;

#[derive(Debug, Error)]
pub enum LoginStoreError {
  #[error("Failed to read file: {0}")]
  Io(#[from] std::io::Error),
  #[error("Database error: {0}")]
  Db(#[from] crate::db::DbError),
  #[error("Failed to parse JSON: {0}")]
  Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone)]
pub struct AuthInfo {
  pub cookie: String,
  #[allow(dead_code)]
  pub csrf: Option<String>,
  pub user_id: Option<i64>,
  pub data: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BilibiliAccountSummary {
  pub user_id: i64,
  pub username: Option<String>,
  pub nickname: Option<String>,
  pub avatar_url: Option<String>,
  pub login_time: String,
  pub expire_time: Option<String>,
  pub is_active: bool,
  pub is_primary: bool,
}

pub struct LoginStore {
  file_path: PathBuf,
}

impl LoginStore {
  pub fn new(file_path: PathBuf) -> Self {
    Self { file_path }
  }

  pub fn load_auth_info(&self, db: &Db) -> Result<Option<AuthInfo>, LoginStoreError> {
    let auth_info = if let Some(user_id) = self.get_active_account_uid(db)? {
      self.load_from_db_by_uid(db, user_id)?
    } else {
      self.load_from_db(db)?
    };
    if let Some(ref auth_info) = auth_info {
      self.write_cache(&auth_info.data)?;
    }

    Ok(auth_info)
  }

  pub fn load_login_data(&self, db: &Db) -> Result<Option<Value>, LoginStoreError> {
    if let Some(user_id) = self.get_active_account_uid(db)? {
      return self.load_login_data_by_uid(db, user_id);
    }
    self.load_login_data_from_latest(db)
  }

  pub fn load_login_data_by_uid(
    &self,
    db: &Db,
    user_id: i64,
  ) -> Result<Option<Value>, LoginStoreError> {
    let record = db.with_conn(|conn| {
      conn
        .query_row(
          "SELECT cookie_info FROM login_info WHERE user_id = ?1",
          [user_id],
          |row| row.get::<_, String>(0),
        )
        .optional()
    })?;
    Ok(match record {
      Some(info) => Some(serde_json::from_str(&info)?),
      None => None,
    })
  }

  pub fn load_refresh_token(&self, db: &Db) -> Result<Option<String>, LoginStoreError> {
    if let Some(user_id) = self.get_active_account_uid(db)? {
      return self.load_refresh_token_by_uid(db, user_id);
    }
    if let Some(data) = self.load_login_data_from_latest(db)? {
      if let Some(token) = extract_refresh_token(&data) {
        return Ok(Some(token));
      }
    }
    let record = db.with_conn(|conn| {
      conn
        .query_row(
          "SELECT refresh_token FROM login_info ORDER BY login_time DESC LIMIT 1",
          [],
          |row| row.get::<_, Option<String>>(0),
        )
        .optional()
    })?;
    Ok(record.flatten())
  }

  pub fn load_refresh_token_by_uid(
    &self,
    db: &Db,
    user_id: i64,
  ) -> Result<Option<String>, LoginStoreError> {
    if let Some(data) = self.load_login_data_by_uid(db, user_id)? {
      if let Some(token) = extract_refresh_token(&data) {
        return Ok(Some(token));
      }
    }
    let record = db.with_conn(|conn| {
      conn
        .query_row(
          "SELECT refresh_token FROM login_info WHERE user_id = ?1",
          [user_id],
          |row| row.get::<_, Option<String>>(0),
        )
        .optional()
    })?;
    Ok(record.flatten())
  }

  pub fn save_login_info(&self, db: &Db, login_data: &Value) -> Result<Option<i64>, LoginStoreError> {
    let user_id = extract_user_id(login_data);
    if user_id.is_none() {
      return Ok(None);
    }

    let now = Utc::now();
    let expire_time = extract_expire_time(login_data)
      .unwrap_or_else(|| now + Duration::hours(24));

    let username = extract_string(login_data, &["uname", "username", "name"]);
    let nickname = extract_string(login_data, &["nickname", "uname", "username", "name"]);
    let avatar_url = extract_string(login_data, &["avatar", "avatar_url", "face"]);

    let access_token = extract_url_param(login_data, "SESSDATA");
    let refresh_token = extract_refresh_token(login_data);

    let cookie_info = serde_json::to_string(login_data)?;

    let user_id_value = user_id.unwrap();
    let login_time_str = now.to_rfc3339();
    let expire_time_str = expire_time.to_rfc3339();
    let create_time_str = now.to_rfc3339();
    let update_time_str = now.to_rfc3339();

    db.with_conn(|conn| {
      conn.execute(
        "INSERT INTO login_info (user_id, username, nickname, avatar_url, access_token, refresh_token, cookie_info, login_time, expire_time, create_time, update_time) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) \
         ON CONFLICT(user_id) DO UPDATE SET \
         username = excluded.username, \
         nickname = excluded.nickname, \
         avatar_url = excluded.avatar_url, \
         access_token = excluded.access_token, \
         refresh_token = COALESCE(excluded.refresh_token, login_info.refresh_token), \
         cookie_info = excluded.cookie_info, \
         login_time = excluded.login_time, \
         expire_time = excluded.expire_time, \
         update_time = excluded.update_time",
        (
          user_id_value,
          username,
          nickname,
          avatar_url,
          access_token,
          refresh_token,
          cookie_info,
          login_time_str,
          expire_time_str,
          create_time_str,
          update_time_str,
        ),
      )?;
      Ok(())
    })?;
    self.set_active_account(db, user_id_value)?;
    self.ensure_primary_account(db, Some(user_id_value))?;
    self.write_cache(login_data)?;

    Ok(Some(user_id_value))
  }

  pub fn save_login_info_without_switching(
    &self,
    db: &Db,
    login_data: &Value,
  ) -> Result<Option<i64>, LoginStoreError> {
    let previous_active_uid = self.get_active_account_uid(db)?;
    let previous_primary_uid = self.get_primary_account_uid(db)?;
    let saved_user_id = self.save_login_info(db, login_data)?;

    match previous_active_uid {
      Some(user_id) if Some(user_id) != saved_user_id => {
        self.set_active_account(db, user_id)?;
      }
      None if saved_user_id.is_some() => {
        self.clear_active_account(db)?;
        self.clear_cache()?;
      }
      _ => {}
    }

    if previous_primary_uid != saved_user_id {
      crate::account_store::set_primary_bilibili_uid(db, previous_primary_uid).map_err(|err| {
        LoginStoreError::Io(std::io::Error::new(std::io::ErrorKind::Other, err))
      })?;
    }

    match previous_active_uid {
      Some(user_id) => {
        if let Some(data) = self.load_login_data_by_uid(db, user_id)? {
          self.write_cache(&data)?;
        } else {
          self.clear_cache()?;
        }
      }
      None => {
        self.clear_cache()?;
      }
    }

    Ok(saved_user_id)
  }

  pub fn logout(&self, db: &Db) -> Result<(), LoginStoreError> {
    if let Some(user_id) = self.get_active_account_uid(db)? {
      self.logout_by_uid(db, user_id)?;
    } else {
      self.clear_active_account(db)?;
      self.clear_cache()?;
    }
    Ok(())
  }

  pub fn load_auth_info_by_uid(
    &self,
    db: &Db,
    user_id: i64,
  ) -> Result<Option<AuthInfo>, LoginStoreError> {
    self.load_from_db_by_uid(db, user_id)
  }

  pub fn load_primary_auth_info(&self, db: &Db) -> Result<Option<AuthInfo>, LoginStoreError> {
    let auth_info = if let Some(user_id) = self.get_primary_account_uid(db)? {
      match self.load_from_db_by_uid(db, user_id)? {
        Some(info) => Some(info),
        None => {
          if let Some(active_uid) = self.get_active_account_uid(db)? {
            self.load_from_db_by_uid(db, active_uid)?
          } else {
            self.load_from_db(db)?
          }
        }
      }
    } else if let Some(user_id) = self.get_active_account_uid(db)? {
      self.load_from_db_by_uid(db, user_id)?
    } else {
      self.load_from_db(db)?
    };
    Ok(auth_info)
  }

  pub fn list_accounts(&self, db: &Db) -> Result<Vec<BilibiliAccountSummary>, LoginStoreError> {
    let active_uid = self.get_active_account_uid(db)?;
    let primary_uid = self.get_primary_account_uid(db)?;
    let records = db.with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT user_id, username, nickname, avatar_url, login_time, expire_time, cookie_info \
         FROM login_info ORDER BY login_time DESC",
      )?;
      let rows = stmt.query_map([], |row| {
        Ok((
          row.get::<_, i64>(0)?,
          row.get::<_, Option<String>>(1)?,
          row.get::<_, Option<String>>(2)?,
          row.get::<_, Option<String>>(3)?,
          row.get::<_, String>(4)?,
          row.get::<_, Option<String>>(5)?,
          row.get::<_, Option<String>>(6)?,
        ))
      })?;
      rows.collect::<Result<Vec<_>, _>>()
    })?;
    Ok(records
      .into_iter()
      .map(
        |(user_id, username, nickname, avatar_url, login_time, expire_time, cookie_info)| {
          let parsed_cookie_info = cookie_info
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok());
          let fallback_name = parsed_cookie_info
            .as_ref()
            .and_then(|data| extract_string(data, &["nickname", "uname", "username", "name"]));
          let fallback_avatar = parsed_cookie_info
            .as_ref()
            .and_then(|data| extract_string(data, &["avatar", "avatar_url", "face"]));
          BilibiliAccountSummary {
            user_id,
            username: username.or_else(|| fallback_name.clone()),
            nickname: nickname.or(fallback_name),
            avatar_url: avatar_url.filter(|value| !value.trim().is_empty()).or(fallback_avatar),
            login_time,
            expire_time,
            is_active: active_uid == Some(user_id),
            is_primary: primary_uid == Some(user_id),
          }
        },
      )
      .collect())
  }

  pub fn list_account_user_ids(&self, db: &Db) -> Result<Vec<i64>, LoginStoreError> {
    Ok(db.with_conn(|conn| {
      let mut stmt = conn.prepare("SELECT user_id FROM login_info ORDER BY login_time DESC")?;
      let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
      rows.collect::<Result<Vec<_>, _>>()
    })?)
  }

  pub fn get_active_account_uid(&self, db: &Db) -> Result<Option<i64>, LoginStoreError> {
    crate::account_store::get_active_bilibili_uid(db).map_err(|err| {
      LoginStoreError::Io(std::io::Error::new(std::io::ErrorKind::Other, err))
    })
  }

  pub fn set_active_account(&self, db: &Db, user_id: i64) -> Result<(), LoginStoreError> {
    let exists = db.with_conn(|conn| {
      conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM login_info WHERE user_id = ?1)",
        [user_id],
        |row| row.get::<_, i64>(0),
      )
    })?;
    if exists == 0 {
      return Ok(());
    }
    crate::account_store::set_active_bilibili_uid(db, Some(user_id)).map_err(|err| {
      LoginStoreError::Io(std::io::Error::new(std::io::ErrorKind::Other, err))
    })?;
    if let Some(data) = self.load_login_data_by_uid(db, user_id)? {
      self.write_cache(&data)?;
    }
    Ok(())
  }

  pub fn get_primary_account_uid(&self, db: &Db) -> Result<Option<i64>, LoginStoreError> {
    crate::account_store::get_primary_bilibili_uid(db).map_err(|err| {
      LoginStoreError::Io(std::io::Error::new(std::io::ErrorKind::Other, err))
    })
  }

  pub fn set_primary_account(&self, db: &Db, user_id: i64) -> Result<(), LoginStoreError> {
    let exists = db.with_conn(|conn| {
      conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM login_info WHERE user_id = ?1)",
        [user_id],
        |row| row.get::<_, i64>(0),
      )
    })?;
    if exists == 0 {
      return Ok(());
    }
    crate::account_store::set_primary_bilibili_uid(db, Some(user_id)).map_err(|err| {
      LoginStoreError::Io(std::io::Error::new(std::io::ErrorKind::Other, err))
    })?;
    Ok(())
  }

  pub fn logout_by_uid(&self, db: &Db, user_id: i64) -> Result<(), LoginStoreError> {
    db.with_conn(|conn| {
      conn.execute("DELETE FROM login_info WHERE user_id = ?1", [user_id])?;
      Ok(())
    })?;
    let active_uid = self.get_active_account_uid(db)?;
    let primary_uid = self.get_primary_account_uid(db)?;
    if active_uid == Some(user_id) {
      let next_uid = db.with_conn(|conn| {
        conn
          .query_row(
            "SELECT user_id FROM login_info ORDER BY login_time DESC LIMIT 1",
            [],
            |row| row.get::<_, i64>(0),
          )
          .optional()
      })?;
      match next_uid {
        Some(next_uid) => self.set_active_account(db, next_uid)?,
        None => {
          self.clear_active_account(db)?;
          self.clear_cache()?;
        }
      }
    }
    if primary_uid == Some(user_id) {
      let next_primary_uid = if let Some(active_uid) = self.get_active_account_uid(db)? {
        Some(active_uid)
      } else {
        db.with_conn(|conn| {
          conn
            .query_row(
              "SELECT user_id FROM login_info ORDER BY login_time DESC LIMIT 1",
              [],
              |row| row.get::<_, i64>(0),
            )
            .optional()
        })?
      };
      crate::account_store::set_primary_bilibili_uid(db, next_primary_uid).map_err(|err| {
        LoginStoreError::Io(std::io::Error::new(std::io::ErrorKind::Other, err))
      })?;
    }
    Ok(())
  }

  fn load_login_data_from_latest(&self, db: &Db) -> Result<Option<Value>, LoginStoreError> {
    let record = db.with_conn(|conn| {
      conn
        .query_row(
          "SELECT cookie_info FROM login_info ORDER BY login_time DESC LIMIT 1",
          [],
          |row| row.get::<_, String>(0),
        )
        .optional()
    })?;
    Ok(match record {
      Some(info) => Some(serde_json::from_str(&info)?),
      None => None,
    })
  }

  fn load_from_db(&self, db: &Db) -> Result<Option<AuthInfo>, LoginStoreError> {
    let user_id = db.with_conn(|conn| {
      conn
        .query_row(
          "SELECT user_id FROM login_info ORDER BY login_time DESC LIMIT 1",
          [],
          |row| row.get::<_, i64>(0),
        )
        .optional()
    })?;
    match user_id {
      Some(user_id) => self.load_from_db_by_uid(db, user_id),
      None => Ok(None),
    }
  }

  fn load_from_db_by_uid(&self, db: &Db, user_id: i64) -> Result<Option<AuthInfo>, LoginStoreError> {
    let record = db.with_conn(|conn| {
      conn
        .query_row(
          "SELECT cookie_info, expire_time FROM login_info WHERE user_id = ?1",
          [user_id],
          |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()
    })?;

    let (cookie_info, expire_time) = match record {
      Some(record) => record,
      None => return Ok(None),
    };

    if let Some(expire_time) = expire_time {
      if let Ok(expire_dt) = DateTime::parse_from_rfc3339(&expire_time) {
        if expire_dt.with_timezone(&Utc) <= Utc::now() {
          return Ok(None);
        }
      }
    }

    let data: Value = serde_json::from_str(&cookie_info)?;
    if let Some(mut auth_info) = build_auth_info(&data, None) {
      auth_info.user_id = Some(user_id);
      return Ok(Some(auth_info));
    }
    Ok(None)
  }

  fn write_cache(&self, login_data: &Value) -> Result<(), LoginStoreError> {
    let login_time_ms = Utc::now().timestamp_millis();
    let file_value = json!({
      "loginTime": login_time_ms,
      "data": login_data,
    });
    fs::write(&self.file_path, serde_json::to_string(&file_value)?)?;
    Ok(())
  }

  fn clear_cache(&self) -> Result<(), LoginStoreError> {
    if self.file_path.exists() {
      fs::remove_file(&self.file_path)?;
    }
    Ok(())
  }

  fn clear_active_account(&self, db: &Db) -> Result<(), LoginStoreError> {
    crate::account_store::set_active_bilibili_uid(db, None).map_err(|err| {
      LoginStoreError::Io(std::io::Error::new(std::io::ErrorKind::Other, err))
    })?;
    Ok(())
  }

  pub fn ensure_primary_account(&self, db: &Db, fallback_user_id: Option<i64>) -> Result<(), LoginStoreError> {
    if self.get_primary_account_uid(db)?.is_some() {
      return Ok(());
    }
    let target_uid = if let Some(user_id) = fallback_user_id {
      Some(user_id)
    } else if let Some(active_uid) = self.get_active_account_uid(db)? {
      Some(active_uid)
    } else {
      db.with_conn(|conn| {
        conn
          .query_row(
            "SELECT user_id FROM login_info ORDER BY login_time DESC LIMIT 1",
            [],
            |row| row.get::<_, i64>(0),
          )
          .optional()
      })?
    };
    crate::account_store::set_primary_bilibili_uid(db, target_uid).map_err(|err| {
      LoginStoreError::Io(std::io::Error::new(std::io::ErrorKind::Other, err))
    })?;
    Ok(())
  }
}

fn build_auth_info(data: &Value, login_time_ms: Option<i64>) -> Option<AuthInfo> {
  let cookie = extract_cookie(data)?;

  if is_login_expired(data, login_time_ms) {
    return None;
  }

  let user_id = extract_user_id(data);
  let csrf = extract_csrf(&cookie);

  Some(AuthInfo {
    cookie,
    csrf,
    user_id,
    data: data.clone(),
  })
}

fn is_login_expired(data: &Value, login_time_ms: Option<i64>) -> bool {
  if let Some(expire_time) = extract_expire_time(data) {
    if expire_time <= Utc::now() {
      return true;
    }
  }

  if let Some(login_time_ms) = login_time_ms {
    if let Some(login_time) = Utc.timestamp_millis_opt(login_time_ms).single() {
      if login_time + Duration::hours(24) <= Utc::now() {
        return true;
      }
    }
  }

  false
}

pub(crate) fn extract_cookie(data: &Value) -> Option<String> {
  if let Some(cookie) = data.get("cookie").and_then(|value| value.as_str()) {
    return Some(cookie.to_string());
  }

  if let Some(cookie) = data.get("cookies").and_then(|value| value.as_str()) {
    return Some(cookie.to_string());
  }

  if let Some(url) = data.get("url").and_then(|value| value.as_str()) {
    return build_cookie_from_url(url);
  }

  if let Some(inner) = data.get("data") {
    return extract_cookie(inner);
  }

  None
}

fn build_cookie_from_url(url: &str) -> Option<String> {
  let params = parse_url_params(url)?;
  let sessdata = params.get("SESSDATA")?;
  let bili_jct = params.get("bili_jct")?;
  if let Some(dede_user_id) = params.get("DedeUserID") {
    return Some(format!(
      "SESSDATA={}; bili_jct={}; DedeUserID={}",
      sessdata, bili_jct, dede_user_id
    ));
  }
  Some(format!("SESSDATA={}; bili_jct={}", sessdata, bili_jct))
}

pub(crate) fn extract_csrf(cookie: &str) -> Option<String> {
  cookie
    .split(';')
    .find_map(|item| {
      let part = item.trim();
      if let Some(value) = part.strip_prefix("bili_jct=") {
        return Some(value.to_string());
      }
      None
    })
}

fn extract_user_id(data: &Value) -> Option<i64> {
  if let Some(url) = data.get("url").and_then(|value| value.as_str()) {
    if let Some(params) = parse_url_params(url) {
      if let Some(user_id) = params.get("DedeUserID") {
        if let Ok(parsed) = user_id.parse::<i64>() {
          return Some(parsed);
        }
      }
    }
  }

  data
    .get("mid")
    .and_then(|value| value.as_i64())
    .or_else(|| data.get("user_id").and_then(|value| value.as_i64()))
}

fn extract_expire_time(data: &Value) -> Option<DateTime<Utc>> {
  if let Some(url) = data.get("url").and_then(|value| value.as_str()) {
    if let Some(params) = parse_url_params(url) {
      if let Some(expires) = params.get("Expires") {
        if let Ok(timestamp) = expires.parse::<i64>() {
          return Utc.timestamp_opt(timestamp, 0).single();
        }
      }
    }
  }

  if let Some(cookie) = extract_cookie(data) {
    if let Some(expire) = extract_sessdata_expire(&cookie) {
      return Some(expire);
    }
  }

  None
}

fn extract_sessdata_expire(cookie: &str) -> Option<DateTime<Utc>> {
  let sessdata = cookie
    .split(';')
    .find_map(|item| item.trim().strip_prefix("SESSDATA="))?;
  if sessdata.is_empty() {
    return None;
  }
  let normalized = sessdata.replace("%2C", ",").replace("%2c", ",");
  let mut parts = normalized.split(',');
  let _ = parts.next()?;
  let expires = parts.next()?;
  let timestamp = expires.parse::<i64>().ok()?;
  Utc.timestamp_opt(timestamp, 0).single()
}

fn extract_url_param(data: &Value, key: &str) -> Option<String> {
  data.get("url")
    .and_then(|value| value.as_str())
    .and_then(|url| parse_url_params(url))
    .and_then(|params| params.get(key).cloned())
}

fn extract_string(data: &Value, keys: &[&str]) -> Option<String> {
  for key in keys {
    if let Some(value) = data.get(*key).and_then(|value| value.as_str()) {
      return Some(value.to_string());
    }
  }

  if let Some(inner) = data.get("data") {
    return extract_string(inner, keys);
  }

  None
}

fn extract_refresh_token(data: &Value) -> Option<String> {
  extract_string(data, &["refresh_token"])
    .or_else(|| extract_url_param(data, "refresh_token"))
}

fn parse_url_params(url: &str) -> Option<HashMap<String, String>> {
  let parsed = Url::parse(url).ok()?;
  let mut params = HashMap::new();
  for (key, value) in parsed.query_pairs() {
    params.insert(key.to_string(), value.to_string());
  }
  Some(params)
}
