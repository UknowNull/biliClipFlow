use rusqlite::OptionalExtension;

use crate::db::Db;
use crate::utils::now_rfc3339;

pub fn get_active_bilibili_uid(db: &Db) -> Result<Option<i64>, String> {
  db.with_conn(|conn| {
    conn
      .query_row(
        "SELECT value FROM app_settings WHERE key = 'active_bilibili_uid'",
        [],
        |row| row.get::<_, String>(0),
      )
      .optional()
  })
  .map_err(|err| err.to_string())
  .map(|value| value.and_then(|item| item.trim().parse::<i64>().ok()))
}

pub fn get_primary_bilibili_uid(db: &Db) -> Result<Option<i64>, String> {
  db.with_conn(|conn| {
    conn
      .query_row(
        "SELECT value FROM app_settings WHERE key = 'primary_bilibili_uid'",
        [],
        |row| row.get::<_, String>(0),
      )
      .optional()
  })
  .map_err(|err| err.to_string())
  .map(|value| value.and_then(|item| item.trim().parse::<i64>().ok()))
}

pub fn set_active_bilibili_uid(db: &Db, user_id: Option<i64>) -> Result<(), String> {
  let now = now_rfc3339();
  let value = user_id.map(|item| item.to_string()).unwrap_or_default();
  db.with_conn(|conn| {
    conn.execute(
      "INSERT INTO app_settings (key, value, updated_at) VALUES ('active_bilibili_uid', ?1, ?2) \
       ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
      (&value, &now),
    )?;
    Ok(())
  })
  .map_err(|err| err.to_string())
}

pub fn set_primary_bilibili_uid(db: &Db, user_id: Option<i64>) -> Result<(), String> {
  let now = now_rfc3339();
  let value = user_id.map(|item| item.to_string()).unwrap_or_default();
  db.with_conn(|conn| {
    conn.execute(
      "INSERT INTO app_settings (key, value, updated_at) VALUES ('primary_bilibili_uid', ?1, ?2) \
       ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
      (&value, &now),
    )?;
    Ok(())
  })
  .map_err(|err| err.to_string())
}

pub fn get_active_baidu_uid(db: &Db) -> Result<Option<String>, String> {
  db.with_conn(|conn| {
    conn
      .query_row(
        "SELECT value FROM app_settings WHERE key = 'active_baidu_uid'",
        [],
        |row| row.get::<_, String>(0),
      )
      .optional()
  })
  .map_err(|err| err.to_string())
  .map(|value| value.filter(|item| !item.trim().is_empty()))
}

pub fn set_active_baidu_uid(db: &Db, uid: Option<&str>) -> Result<(), String> {
  let now = now_rfc3339();
  let value = uid.unwrap_or("").trim().to_string();
  db.with_conn(|conn| {
    conn.execute(
      "INSERT INTO app_settings (key, value, updated_at) VALUES ('active_baidu_uid', ?1, ?2) \
       ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
      (&value, &now),
    )?;
    Ok(())
  })
  .map_err(|err| err.to_string())
}

pub fn get_bound_baidu_uid(db: &Db, bilibili_uid: i64) -> Result<Option<String>, String> {
  db.with_conn(|conn| {
    conn
      .query_row(
        "SELECT baidu_uid FROM bilibili_baidu_binding WHERE bilibili_uid = ?1",
        [bilibili_uid],
        |row| row.get::<_, String>(0),
      )
      .optional()
  })
  .map_err(|err| err.to_string())
  .map(|value| value.map(|item| item.trim().to_string()).filter(|item| !item.is_empty()))
}

pub fn set_account_binding(
  db: &Db,
  bilibili_uid: i64,
  baidu_uid: &str,
) -> Result<(), String> {
  let trimmed_baidu_uid = baidu_uid.trim();
  if trimmed_baidu_uid.is_empty() {
    return Err("网盘账号不能为空".to_string());
  }
  let now = now_rfc3339();
  db.with_conn(|conn| {
    conn.execute(
      "INSERT INTO bilibili_baidu_binding (bilibili_uid, baidu_uid, create_time, update_time) \
       VALUES (?1, ?2, ?3, ?4) \
       ON CONFLICT(bilibili_uid) DO UPDATE SET baidu_uid = excluded.baidu_uid, update_time = excluded.update_time",
      (bilibili_uid, trimmed_baidu_uid, &now, &now),
    )?;
    Ok(())
  })
  .map_err(|err| err.to_string())
}
