use std::path::PathBuf;
use std::sync::Mutex;

use rusqlite::Connection;
use rusqlite::OptionalExtension;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DbError {
    #[error("database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database lock poisoned")]
    Lock,
}

pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    pub fn new(db_path: PathBuf) -> Result<Self, DbError> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(db_path)?;
        let _ = conn.execute(
            "ALTER TABLE task_output_segment ADD COLUMN merged_id INTEGER",
            [],
        );
        let _ = conn.execute("ALTER TABLE live_settings ADD COLUMN record_path TEXT", []);
        let _ = conn.execute(
            "ALTER TABLE live_settings ADD COLUMN baidu_sync_enabled INTEGER DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE live_settings ADD COLUMN baidu_sync_path TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE live_settings ADD COLUMN title_split_min_seconds INTEGER DEFAULT 1800",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE live_settings ADD COLUMN stream_read_timeout_ms INTEGER DEFAULT 15000",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE live_settings ADD COLUMN flv_fix_adjust_timestamp_jump INTEGER DEFAULT 0",
            [],
        );
        let _ = conn.execute(
      "ALTER TABLE live_settings ADD COLUMN flv_fix_split_on_timestamp_jump INTEGER DEFAULT 1",
      [],
    );
        let _ = conn.execute("ALTER TABLE submission_task ADD COLUMN aid INTEGER", []);
        let _ = conn.execute(
            "ALTER TABLE submission_task ADD COLUMN remote_state INTEGER",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE submission_task ADD COLUMN reject_reason TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE submission_task ADD COLUMN bilibili_uid INTEGER",
            [],
        );
        let _ = conn.execute("ALTER TABLE submission_task ADD COLUMN baidu_uid TEXT", []);
        let _ = conn.execute(
            "ALTER TABLE submission_task ADD COLUMN priority INTEGER DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE submission_task ADD COLUMN baidu_sync_enabled INTEGER DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE submission_task ADD COLUMN baidu_sync_path TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE submission_task ADD COLUMN baidu_sync_filename TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE submission_task ADD COLUMN topic_id INTEGER",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE submission_task ADD COLUMN mission_id INTEGER",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE submission_task ADD COLUMN activity_title TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE submission_task ADD COLUMN cover_local_path TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE submission_task ADD COLUMN import_mode TEXT DEFAULT 'NON_SEGMENTED'",
            [],
        );
        let _ = conn.execute("ALTER TABLE video_download ADD COLUMN cid INTEGER", []);
        let _ = conn.execute("ALTER TABLE video_download ADD COLUMN content TEXT", []);
        let _ = conn.execute(
            "ALTER TABLE video_download ADD COLUMN source_type TEXT DEFAULT 'BILIBILI'",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE video_download ADD COLUMN progress_total INTEGER DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE video_download ADD COLUMN progress_done INTEGER DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE merged_video ADD COLUMN upload_progress REAL DEFAULT 0.0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE merged_video ADD COLUMN upload_uploaded_bytes INTEGER DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE merged_video ADD COLUMN upload_total_bytes INTEGER DEFAULT 0",
            [],
        );
        let _ = conn.execute("ALTER TABLE merged_video ADD COLUMN upload_cid INTEGER", []);
        let _ = conn.execute(
            "ALTER TABLE merged_video ADD COLUMN upload_file_name TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE merged_video ADD COLUMN upload_session_id TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE merged_video ADD COLUMN upload_biz_id INTEGER DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE merged_video ADD COLUMN upload_endpoint TEXT",
            [],
        );
        let _ = conn.execute("ALTER TABLE merged_video ADD COLUMN upload_auth TEXT", []);
        let _ = conn.execute("ALTER TABLE merged_video ADD COLUMN upload_uri TEXT", []);
        let _ = conn.execute(
            "ALTER TABLE merged_video ADD COLUMN upload_chunk_size INTEGER DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE merged_video ADD COLUMN upload_last_part_index INTEGER DEFAULT 0",
            [],
        );
        let _ = conn.execute("ALTER TABLE merged_video ADD COLUMN remote_dir TEXT", []);
        let _ = conn.execute("ALTER TABLE merged_video ADD COLUMN remote_name TEXT", []);
        let _ = conn.execute("ALTER TABLE merged_video ADD COLUMN baidu_uid TEXT", []);
        let _ = conn.execute(
            "ALTER TABLE merged_video ADD COLUMN sort_order INTEGER DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE task_output_segment ADD COLUMN upload_progress REAL DEFAULT 0.0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE task_output_segment ADD COLUMN upload_uploaded_bytes INTEGER DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE task_output_segment ADD COLUMN upload_total_bytes INTEGER DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE task_output_segment ADD COLUMN upload_session_id TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE task_output_segment ADD COLUMN upload_biz_id INTEGER DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE task_output_segment ADD COLUMN upload_endpoint TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE task_output_segment ADD COLUMN upload_auth TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE task_output_segment ADD COLUMN upload_uri TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE task_output_segment ADD COLUMN upload_chunk_size INTEGER DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE task_output_segment ADD COLUMN upload_last_part_index INTEGER DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE task_source_video ADD COLUMN remote_video_url TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE task_source_video ADD COLUMN remote_bvid TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE task_source_video ADD COLUMN remote_aid INTEGER",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE task_source_video ADD COLUMN remote_cid INTEGER",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE task_source_video ADD COLUMN remote_part_title TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE live_room_settings ADD COLUMN baidu_sync_path TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE live_room_settings ADD COLUMN baidu_sync_enabled INTEGER DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE live_clip_item ADD COLUMN source_file_path TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE live_clip_item ADD COLUMN status TEXT DEFAULT 'SUCCESS'",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE anchor_submission_config ADD COLUMN activity_title TEXT",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE submission_task ADD COLUMN source_type TEXT DEFAULT 'NORMAL'",
            [],
        );
        let _ = conn.execute("ALTER TABLE baidu_sync_task ADD COLUMN baidu_uid TEXT", []);
        conn.execute_batch(include_str!("db/schema.sql"))?;
        let _ = conn.execute(
      "INSERT OR IGNORE INTO app_settings (key, value, updated_at) VALUES ('active_bilibili_uid', '', datetime('now'))",
      [],
    );
        let _ = conn.execute(
      "INSERT OR IGNORE INTO app_settings (key, value, updated_at) VALUES ('primary_bilibili_uid', '', datetime('now'))",
      [],
    );
        let _ = conn.execute(
      "INSERT OR IGNORE INTO app_settings (key, value, updated_at) VALUES ('active_baidu_uid', '', datetime('now'))",
      [],
    );
        let baidu_account_count = conn
            .query_row("SELECT COUNT(*) FROM baidu_account_info", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap_or(0);
        if baidu_account_count == 0 {
            let _ = conn.execute(
        "INSERT INTO baidu_account_info (uid, status, username, login_type, login_time, last_check_time, create_time, update_time) \
         SELECT uid, status, username, login_type, login_time, last_check_time, create_time, update_time \
         FROM baidu_login_info \
         WHERE uid IS NOT NULL AND TRIM(uid) <> '' \
         ON CONFLICT(uid) DO UPDATE SET \
           status = excluded.status, \
           username = excluded.username, \
           login_type = excluded.login_type, \
           login_time = excluded.login_time, \
           last_check_time = excluded.last_check_time, \
           update_time = excluded.update_time",
        [],
      );
        }
        let baidu_credential_count = conn
            .query_row("SELECT COUNT(*) FROM baidu_account_credential", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap_or(0);
        if baidu_credential_count == 0 {
            let legacy_baidu_uid = conn
        .query_row(
          "SELECT uid FROM baidu_login_info WHERE uid IS NOT NULL AND TRIM(uid) <> '' LIMIT 1",
          [],
          |row| row.get::<_, String>(0),
        )
        .optional()?;
            if let Some(uid) = legacy_baidu_uid.as_deref() {
                let _ = conn.execute(
          "INSERT INTO baidu_account_credential (baidu_uid, login_type, cookie, bduss, stoken, last_attempt_time, last_attempt_error, create_time, update_time) \
           SELECT ?1, login_type, cookie, bduss, stoken, last_attempt_time, last_attempt_error, create_time, update_time \
           FROM baidu_login_credential WHERE id = 1 \
           ON CONFLICT(baidu_uid) DO UPDATE SET \
             login_type = excluded.login_type, \
             cookie = excluded.cookie, \
             bduss = excluded.bduss, \
             stoken = excluded.stoken, \
             last_attempt_time = excluded.last_attempt_time, \
             last_attempt_error = excluded.last_attempt_error, \
             update_time = excluded.update_time",
          [uid],
        );
            }
        }
        let active_bilibili_uid = conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'active_bilibili_uid'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_default();
        if active_bilibili_uid.trim().is_empty() {
            if let Some(uid) = conn
                .query_row(
                    "SELECT CAST(user_id AS TEXT) FROM login_info ORDER BY login_time DESC LIMIT 1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
            {
                let _ = conn.execute(
          "UPDATE app_settings SET value = ?1, updated_at = datetime('now') WHERE key = 'active_bilibili_uid'",
          [uid],
        );
            }
        }
        let primary_bilibili_uid = conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'primary_bilibili_uid'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_default();
        if primary_bilibili_uid.trim().is_empty() {
            let fallback_uid = if !active_bilibili_uid.trim().is_empty() {
                Some(active_bilibili_uid.clone())
            } else {
                conn.query_row(
                    "SELECT CAST(user_id AS TEXT) FROM login_info ORDER BY login_time DESC LIMIT 1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
            };
            if let Some(uid) = fallback_uid {
                let _ = conn.execute(
          "UPDATE app_settings SET value = ?1, updated_at = datetime('now') WHERE key = 'primary_bilibili_uid'",
          [uid],
        );
            }
        }
        let active_baidu_uid = conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = 'active_baidu_uid'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_default();
        if active_baidu_uid.trim().is_empty() {
            if let Some(uid) = conn
        .query_row(
          "SELECT uid FROM baidu_account_info ORDER BY login_time DESC, update_time DESC LIMIT 1",
          [],
          |row| row.get::<_, String>(0),
        )
        .optional()?
      {
        let _ = conn.execute(
          "UPDATE app_settings SET value = ?1, updated_at = datetime('now') WHERE key = 'active_baidu_uid'",
          [uid],
        );
      }
        }
        let _ = conn.execute(
            "UPDATE submission_task SET import_mode = 'SEGMENTED' \
       WHERE import_mode IS NULL OR TRIM(import_mode) = ''",
            [],
        );
        let _ = conn.execute(
            "UPDATE submission_task \
       SET import_mode = 'NON_SEGMENTED' \
       WHERE task_id IN ( \
         SELECT st.task_id \
         FROM submission_task st \
         LEFT JOIN ( \
           SELECT task_id, COUNT(*) AS merged_count \
           FROM merged_video \
           GROUP BY task_id \
         ) mv ON mv.task_id = st.task_id \
         LEFT JOIN ( \
           SELECT task_id, COUNT(*) AS segment_count \
           FROM task_output_segment \
           GROUP BY task_id \
         ) os ON os.task_id = st.task_id \
         LEFT JOIN ( \
           SELECT task_id, COUNT(*) AS unbound_count \
           FROM task_output_segment \
           WHERE merged_id IS NULL OR merged_id = 0 \
           GROUP BY task_id \
         ) ub ON ub.task_id = st.task_id \
         LEFT JOIN ( \
           SELECT task_id, COUNT(*) AS multi_bind_merged_count \
           FROM ( \
             SELECT task_id, merged_id, COUNT(*) AS c \
             FROM task_output_segment \
             WHERE merged_id IS NOT NULL AND merged_id != 0 \
             GROUP BY task_id, merged_id \
             HAVING c > 1 \
           ) tmp \
           GROUP BY task_id \
         ) mb ON mb.task_id = st.task_id \
         WHERE COALESCE(mv.merged_count, 0) > 0 \
           AND COALESCE(os.segment_count, 0) = COALESCE(mv.merged_count, 0) \
           AND COALESCE(ub.unbound_count, 0) = 0 \
           AND COALESCE(mb.multi_bind_merged_count, 0) = 0 \
       )",
            [],
        );
        let _ = conn.execute(
            "INSERT OR IGNORE INTO app_settings (key, value, updated_at) \
       VALUES ('baidu_sync_concurrency', '3', datetime('now'))",
            [],
        );
        let _ = conn.execute(
            "UPDATE app_settings SET value = '3', updated_at = datetime('now') \
       WHERE key = 'baidu_sync_concurrency' AND value = '1'",
            [],
        );

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn with_conn<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, rusqlite::Error>,
    ) -> Result<T, DbError> {
        let conn = self.conn.lock().map_err(|_| DbError::Lock)?;
        Ok(f(&conn)?)
    }

    pub fn with_conn_mut<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T, rusqlite::Error>,
    ) -> Result<T, DbError> {
        let mut conn = self.conn.lock().map_err(|_| DbError::Lock)?;
        Ok(f(&mut conn)?)
    }
}
