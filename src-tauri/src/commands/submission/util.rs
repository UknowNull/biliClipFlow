//! 投稿模块内部共用的辅助函数。

use super::*;

pub(super) fn is_submission_terminal_status(status: &str) -> bool {
    status == "COMPLETED" || status == "FAILED"
}

pub(super) fn apply_integrate_current_bvid(config: &mut Value, integrate_current_bvid: bool) {
    if !config.is_object() {
        *config = Value::Object(Map::new());
    }
    let Some(map) = config.as_object_mut() else {
        return;
    };
    map.insert(
        "integrateCurrentBvid".to_string(),
        Value::Bool(integrate_current_bvid),
    );
}

pub(super) fn prepare_integrated_source_downloads(
    context: &SubmissionContext,
    task_id: &str,
    workflow_instance_id: &str,
    source_paths: &[String],
) -> Result<Vec<i64>, String> {
    let missing_sources = collect_missing_source_paths(source_paths);
    if missing_sources.is_empty() {
        return Ok(Vec::new());
    }
    let (retry_records, unresolved_sources) =
        collect_retry_download_records_for_missing_sources(context, task_id, &missing_sources)?;
    if !unresolved_sources.is_empty() {
        return Err(
            "存在本地缺失且未绑定可下载远程信息的源视频，请先补充远程BV/CID绑定或改为本地文件"
                .to_string(),
        );
    }
    if retry_records.is_empty() {
        return Ok(Vec::new());
    }
    create_retry_download_records(context, task_id, workflow_instance_id, &retry_records)
}

pub(super) fn defer_integrated_downloads(
    context: &SubmissionContext,
    download_ids: &[i64],
) -> Result<(), String> {
    if download_ids.is_empty() {
        return Ok(());
    }
    context
        .db
        .with_conn(|conn| {
            for id in download_ids {
                conn.execute(
                    "UPDATE video_download SET status = 5, update_time = ?1 WHERE id = ?2 AND status = 0",
                    (now_rfc3339(), id),
                )?;
            }
            Ok(())
        })
        .map_err(|err| err.to_string())
}

pub(super) fn extract_source_paths_from_task(detail: &SubmissionTaskDetail) -> Vec<String> {
    detail
        .source_videos
        .iter()
        .map(|source| source.source_file_path.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

pub(super) fn extract_update_source_paths_from_config(config: &Value) -> Vec<String> {
    let Some(list) = config
        .get("updateSources")
        .and_then(|value| value.as_array())
    else {
        return Vec::new();
    };
    let mut sources = Vec::new();
    for (index, item) in list.iter().enumerate() {
        let input_path = item
            .get("sourceFilePath")
            .or_else(|| item.get("source_file_path"))
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if input_path.is_empty() {
            continue;
        }
        let order = item
            .get("sortOrder")
            .or_else(|| item.get("sort_order"))
            .and_then(|value| value.as_i64())
            .unwrap_or((index + 1) as i64);
        sources.push((order, input_path));
    }
    sources.sort_by_key(|item| item.0);
    sources.into_iter().map(|item| item.1).collect()
}

pub(super) fn extract_update_sources_from_config(config: &Value) -> Vec<ClipSource> {
    let Some(list) = config
        .get("updateSources")
        .and_then(|value| value.as_array())
    else {
        return Vec::new();
    };
    let mut sources = Vec::new();
    for (index, item) in list.iter().enumerate() {
        let input_path = item
            .get("sourceFilePath")
            .or_else(|| item.get("source_file_path"))
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if input_path.is_empty() {
            continue;
        }
        let start_time = item
            .get("startTime")
            .or_else(|| item.get("start_time"))
            .and_then(|value| value.as_str())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let end_time = item
            .get("endTime")
            .or_else(|| item.get("end_time"))
            .and_then(|value| value.as_str())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let order = item
            .get("sortOrder")
            .or_else(|| item.get("sort_order"))
            .and_then(|value| value.as_i64())
            .unwrap_or((index + 1) as i64);
        sources.push(ClipSource {
            input_path,
            start_time,
            end_time,
            order,
        });
    }
    sources.sort_by_key(|item| item.order);
    sources
}

pub(super) fn strip_update_sources(config: &Value) -> Value {
    let mut next = config.clone();
    if let Value::Object(map) = &mut next {
        map.remove("updateSources");
    }
    next
}

pub(super) fn normalize_runtime_source_path(context: &SubmissionContext, source_file_path: &str) -> String {
    to_runtime_submission_path(context, source_file_path)
}

pub(super) fn create_retry_download_records(
    context: &SubmissionContext,
    task_id: &str,
    workflow_instance_id: &str,
    records: &[IntegratedDownloadRecord],
) -> Result<Vec<i64>, String> {
    if records.is_empty() {
        return Ok(Vec::new());
    }
    for record in records {
        if record.local_path.trim().is_empty() {
            return Err("下载记录缺少本地路径，无法重新下载".to_string());
        }
        let source_type = record.source_type.trim().to_ascii_uppercase();
        if source_type == "BAIDU" && record.download_url.trim().is_empty() {
            return Err("网盘下载记录缺少远程路径，无法重新下载".to_string());
        }
        if source_type == crate::commands::download::DOWNLOAD_SOURCE_DIRECT {
            if record.download_url.trim().is_empty() {
                return Err("第三方下载记录缺少下载链接，无法重新下载".to_string());
            }
            continue;
        }
        if source_type != "BAIDU" {
            let has_bvid = record
                .bvid
                .as_deref()
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false);
            let has_aid = record
                .aid
                .as_deref()
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false);
            if record.cid.is_none() || (!has_bvid && !has_aid) {
                return Err("源视频缺少BVID/AID或CID，无法重新下载".to_string());
            }
        }
    }
    let now = now_rfc3339();
    context
    .db
    .with_conn(|conn| {
      let mut new_ids = Vec::with_capacity(records.len());
      for record in records {
        let stored_local_path = to_stored_submission_path(context, &record.local_path);
        let source_type = record.source_type.trim().to_ascii_uppercase();
        let download_url = normalize_optional_text(Some(record.download_url.clone()));
        conn.execute(
          "INSERT INTO video_download (bvid, aid, title, part_title, part_count, current_part, download_url, local_path, status, progress, progress_total, progress_done, create_time, update_time, resolution, codec, format, cid, content, source_type) \
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, 0, 0, 0, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
          (
            record.bvid.as_deref(),
            record.aid.as_deref(),
            record.title.as_deref(),
            record.part_title.as_deref(),
            record.part_count,
            record.current_part,
            download_url.as_deref(),
            stored_local_path.as_str(),
            &now,
            &now,
            record.resolution.as_deref(),
            record.codec.as_deref(),
            record.format.as_deref(),
            record.cid,
            record.content.as_deref(),
            source_type.as_str(),
          ),
        )?;
        let new_id = conn.last_insert_rowid();
        if record.id > 0 {
          conn.execute(
            "DELETE FROM task_relations WHERE submission_task_id = ?1 AND download_task_id = ?2 AND relation_type = 'INTEGRATED'",
            (task_id, record.id),
          )?;
        }
        conn.execute(
          "INSERT INTO task_relations (download_task_id, submission_task_id, relation_type, status, created_at, updated_at, workflow_instance_id, workflow_status, retry_count) \
           VALUES (?1, ?2, 'INTEGRATED', 'ACTIVE', ?3, ?4, ?5, 'PENDING_DOWNLOAD', 0)",
          (new_id, task_id, &now, &now, workflow_instance_id),
        )?;
        new_ids.push(new_id);
      }
      Ok(new_ids)
    })
    .map_err(|err| err.to_string())
}

pub(super) fn normalize_dir_for_open(path: &Path) -> PathBuf {
    let resolved = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    #[cfg(target_os = "windows")]
    {
        let raw = resolved.to_string_lossy();
        let normalized = raw.strip_prefix(r"\\?\").unwrap_or(&raw).to_string();
        return PathBuf::from(normalized);
    }
    #[cfg(not(target_os = "windows"))]
    {
        resolved
    }
}

pub(super) fn resolve_task_dir_for_open(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<PathBuf, String> {
    let dir = resolve_submission_base_dir(context, task_id);
    fs::create_dir_all(&dir).map_err(|err| format!("创建任务目录失败: {}", err))?;
    let meta = fs::metadata(&dir).map_err(|err| format!("任务目录不存在: {}", err))?;
    if !meta.is_dir() {
        return Err("任务目录不是有效文件夹".to_string());
    }
    Ok(normalize_dir_for_open(&dir))
}

pub(super) fn cleanup_submission_files(log_path: &PathBuf, base_dir: &Path) -> Result<(), String> {
    let targets = [
        ("cut", base_dir.join("cut")),
        ("merge", base_dir.join("merge")),
        ("output", base_dir.join("output")),
    ];
    for (label, path) in targets {
        remove_path_if_exists(log_path, label, &path)?;
    }
    remove_path_if_exists(log_path, "base", base_dir)?;
    Ok(())
}

pub(super) fn remove_path_if_exists_with_retry(
    log_path: &PathBuf,
    label: &str,
    path: &Path,
    max_attempts: usize,
    retry_delay: Duration,
) -> Result<(), String> {
    let attempts = max_attempts.max(1);
    let mut last_err = None;
    for attempt in 1..=attempts {
        match remove_path_if_exists(log_path, label, path) {
            Ok(()) => return Ok(()),
            Err(err) => {
                last_err = Some(err.clone());
                if attempt < attempts {
                    append_log(
                        log_path,
                        &format!(
                            "submission_cleanup_retry label={} path={} attempt={}/{} err={}",
                            label,
                            path.to_string_lossy(),
                            attempt,
                            attempts,
                            err
                        ),
                    );
                    std::thread::sleep(retry_delay);
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| format!("清理{}失败", label)))
}

pub(super) fn cleanup_submission_derived_files_with_retry(
    log_path: &PathBuf,
    base_dir: &Path,
) -> Result<(), String> {
    let targets = [
        ("cut", base_dir.join("cut")),
        ("merge", base_dir.join("merge")),
        ("output", base_dir.join("output")),
    ];
    for (label, path) in targets {
        remove_path_if_exists_with_retry(log_path, label, &path, 5, Duration::from_millis(300))?;
    }
    Ok(())
}

pub(super) fn path_exists(path: &Path) -> bool {
    fs::metadata(path).is_ok()
}

pub(super) fn normalize_delete_paths(paths: &Option<Vec<String>>) -> Vec<String> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    for path in paths.as_ref().unwrap_or(&Vec::new()) {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            continue;
        }
        let normalized = trimmed.to_string();
        if seen.insert(normalized.clone()) {
            result.push(normalized);
        }
    }
    result
}

pub(super) fn remove_path_if_exists(log_path: &PathBuf, label: &str, path: &Path) -> Result<(), String> {
    match fs::metadata(path) {
        Ok(metadata) => {
            append_log(
                log_path,
                &format!(
                    "submission_cleanup_start label={} path={}",
                    label,
                    path.to_string_lossy()
                ),
            );
            let result = if metadata.is_dir() {
                fs::remove_dir_all(path)
            } else {
                fs::remove_file(path)
            };
            match result {
                Ok(()) => {
                    append_log(
                        log_path,
                        &format!(
                            "submission_cleanup_ok label={} path={}",
                            label,
                            path.to_string_lossy()
                        ),
                    );
                    Ok(())
                }
                Err(err) => Err(format!("清理{}失败: {}", label, err.to_string())),
            }
        }
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("读取{}路径失败: {}", label, err.to_string())),
    }
}

pub(super) fn cleanup_empty_parent_dir(log_path: &PathBuf, path: &Path) {
    let parent = match path.parent() {
        Some(dir) => dir,
        None => return,
    };
    let mut entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return,
        Err(err) => {
            append_log(
                log_path,
                &format!(
                    "submission_delete_parent_scan_fail path={} err={}",
                    parent.to_string_lossy(),
                    err
                ),
            );
            return;
        }
    };
    if entries.next().is_some() {
        return;
    }
    match fs::remove_dir(parent) {
        Ok(()) => {
            append_log(
                log_path,
                &format!(
                    "submission_delete_parent_ok path={}",
                    parent.to_string_lossy()
                ),
            );
        }
        Err(err) => {
            append_log(
                log_path,
                &format!(
                    "submission_delete_parent_fail path={} err={}",
                    parent.to_string_lossy(),
                    err
                ),
            );
        }
    }
}

pub(super) fn require_current_bilibili_uid(state: &State<'_, AppState>) -> Result<i64, String> {
    let auth = state
        .login_store
        .load_auth_info(&state.db)
        .ok()
        .flatten()
        .ok_or_else(|| "请先登录B站账号".to_string())?;
    auth.user_id.ok_or_else(|| "请先登录B站账号".to_string())
}

pub(super) fn resolve_requested_bilibili_uid(
    state: &State<'_, AppState>,
    requested_uid: Option<i64>,
) -> Result<i64, String> {
    let requested_uid = requested_uid.unwrap_or(require_current_bilibili_uid(state)?);
    let exists = state
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM login_info WHERE user_id = ?1)",
                [requested_uid],
                |row| row.get::<_, i64>(0),
            )
        })
        .map_err(|err| err.to_string())?;
    if exists == 0 {
        return Err("指定的B站账号未登录".to_string());
    }
    Ok(requested_uid)
}

pub(super) fn resolve_submission_list_bilibili_uid(
    state: &State<'_, AppState>,
    context: &SubmissionContext,
    preferred_uid: Option<i64>,
) -> Result<Option<i64>, String> {
    if let Some(preferred_uid) = preferred_uid {
        let requested_uid = resolve_requested_bilibili_uid(state, Some(preferred_uid))?;
        return Ok(Some(requested_uid));
    }
    if let Ok(uid) = require_current_bilibili_uid(state) {
        let current_uid_has_tasks = context
            .db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM submission_task WHERE bilibili_uid = ?1 LIMIT 1)",
                    [uid],
                    |row| row.get::<_, i64>(0),
                )
            })
            .map_err(|err| err.to_string())?
            != 0;
        if current_uid_has_tasks {
            return Ok(Some(uid));
        }
        append_log(
            &state.app_log_path,
            &format!("submission_list_uid_no_tasks current_uid={}", uid),
        );
    }
    let fallback_uid = context
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT bilibili_uid FROM submission_task ORDER BY created_at DESC LIMIT 1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .optional()
        })
        .map_err(|err| err.to_string())?;
    if let Some(uid) = fallback_uid {
        append_log(
            &state.app_log_path,
            &format!("submission_list_uid_fallback uid={}", uid),
        );
    }
    Ok(fallback_uid)
}

pub(super) fn is_safe_identifier(identifier: &str) -> bool {
    !identifier.is_empty()
        && identifier
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

pub(super) fn bytes_to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

pub(super) fn hex_to_bytes(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if bytes.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 2);
    let decode = |value: u8| -> Option<u8> {
        match value {
            b'0'..=b'9' => Some(value - b'0'),
            b'a'..=b'f' => Some(value - b'a' + 10),
            b'A'..=b'F' => Some(value - b'A' + 10),
            _ => None,
        }
    };
    let mut index = 0;
    while index < bytes.len() {
        let high = decode(bytes[index])?;
        let low = decode(bytes[index + 1])?;
        out.push((high << 4) | low);
        index += 2;
    }
    Some(out)
}

pub(super) fn sqlite_value_ref_to_json(value_ref: ValueRef<'_>) -> Value {
    match value_ref {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(value) => Value::Number(Number::from(value)),
        ValueRef::Real(value) => Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        ValueRef::Text(value) => Value::String(String::from_utf8_lossy(value).to_string()),
        ValueRef::Blob(value) => Value::String(format!("__blob__{}", bytes_to_hex(value))),
    }
}

pub(super) fn json_to_sql_value(value: &Value) -> SqlValue {
    match value {
        Value::Null => SqlValue::Null,
        Value::Bool(value) => SqlValue::Integer(if *value { 1 } else { 0 }),
        Value::Number(value) => {
            if let Some(int_value) = value.as_i64() {
                SqlValue::Integer(int_value)
            } else if let Some(float_value) = value.as_f64() {
                SqlValue::Real(float_value)
            } else {
                SqlValue::Null
            }
        }
        Value::String(value) => {
            if let Some(blob_hex) = value.strip_prefix("__blob__") {
                match hex_to_bytes(blob_hex) {
                    Some(bytes) => SqlValue::Blob(bytes),
                    None => SqlValue::Text(value.clone()),
                }
            } else {
                SqlValue::Text(value.clone())
            }
        }
        _ => SqlValue::Text(value.to_string()),
    }
}

pub(super) fn row_to_json(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let mut map = Map::new();
    let row_ref = row.as_ref();
    for index in 0..row_ref.column_count() {
        let column = row_ref.column_name(index).unwrap_or("").to_string();
        let value_ref = row.get_ref(index)?;
        map.insert(column, sqlite_value_ref_to_json(value_ref));
    }
    Ok(Value::Object(map))
}

pub(super) fn query_rows_json<P: rusqlite::Params>(
    conn: &rusqlite::Connection,
    sql: &str,
    params: P,
) -> rusqlite::Result<Vec<Value>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params, row_to_json)?;
    rows.collect::<Result<Vec<_>, _>>()
}

pub(super) fn query_optional_row_json<P: rusqlite::Params>(
    conn: &rusqlite::Connection,
    sql: &str,
    params: P,
) -> rusqlite::Result<Option<Value>> {
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query(params)?;
    match rows.next()? {
        Some(row) => row_to_json(row).map(Some),
        None => Ok(None),
    }
}

pub(super) fn json_string_field(record: &Value, key: &str) -> Option<String> {
    record
        .get(key)
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
}

pub(super) fn json_string_field_non_empty(record: &Value, key: &str) -> Option<String> {
    json_string_field(record, key).and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

pub(super) fn json_i64_field(record: &Value, key: &str) -> Option<i64> {
    record.get(key).and_then(|value| value.as_i64())
}

pub(super) fn normalize_json_path_field_with_prefix(record: &mut Value, key: &str, path_prefix: &Path) {
    if let Some(path_value) = json_string_field(record, key) {
        let normalized = to_portable_relative_path_with_prefix(path_prefix, &path_value);
        set_json_field(record, key, Value::String(normalized));
    }
}

pub(super) fn row_exists_text(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    column: &str,
    value: &str,
) -> Result<bool, String> {
    if !is_safe_identifier(table) || !is_safe_identifier(column) {
        return Err("表字段不安全".to_string());
    }
    let sql = format!("SELECT 1 FROM {} WHERE {} = ?1 LIMIT 1", table, column);
    let exists = tx
        .query_row(sql.as_str(), [value], |_| Ok(1))
        .optional()
        .map_err(|err| err.to_string())?
        .is_some();
    Ok(exists)
}

pub(super) fn generate_unique_text_id(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    column: &str,
) -> Result<String, String> {
    for _ in 0..20 {
        let id = uuid::Uuid::new_v4().to_string();
        if !row_exists_text(tx, table, column, &id)? {
            return Ok(id);
        }
    }
    Err(format!("无法生成{}表唯一ID", table))
}

pub(super) fn json_i64_compatible(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    if let Some(parsed) = value.as_i64() {
        return Some(parsed);
    }
    if let Some(parsed) = value.as_u64() {
        if parsed <= i64::MAX as u64 {
            return Some(parsed as i64);
        }
    }
    value
        .as_str()
        .and_then(|raw| raw.trim().parse::<i64>().ok())
}

pub(super) fn json_string_compatible(value: Option<&Value>) -> Option<String> {
    value
        .and_then(|raw| raw.as_str())
        .map(|raw| raw.trim().to_string())
        .filter(|raw| !raw.is_empty())
}

pub(super) fn normalize_csv_text(raw: &str) -> Option<String> {
    let normalized = raw
        .split(',')
        .map(|item| item.trim())
        .filter(|item| !item.is_empty())
        .collect::<Vec<_>>()
        .join(",");
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

pub(super) fn extract_bvid_from_input(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(index) = trimmed.find("BV") {
        let value = &trimmed[index..];
        let end = value
            .find(|ch: char| !ch.is_ascii_alphanumeric())
            .unwrap_or(value.len());
        let bvid = value[..end].trim();
        if bvid.len() > 2 {
            return Some(bvid.to_string());
        }
    }
    None
}

pub(super) fn extract_aid_from_input(input: &str) -> Option<i64> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.chars().all(|ch| ch.is_ascii_digit()) {
        return trimmed.parse::<i64>().ok();
    }
    if let Some(index) = trimmed.find("av") {
        let value = &trimmed[index + 2..];
        let digits: String = value.chars().take_while(|ch| ch.is_ascii_digit()).collect();
        if !digits.is_empty() {
            return digits.parse::<i64>().ok();
        }
    }
    None
}

pub(super) fn seconds_to_hms(seconds: i64) -> String {
    let safe = seconds.max(0);
    let hours = safe / 3600;
    let minutes = (safe % 3600) / 60;
    let secs = safe % 60;
    format!("{:02}:{:02}:{:02}", hours, minutes, secs)
}

pub(super) fn normalize_hms(value: Option<&str>, default_value: &str) -> String {
    let parsed = value
        .map(|raw| raw.trim())
        .filter(|raw| !raw.is_empty())
        .and_then(|raw| parse_time_to_seconds(raw));
    match parsed {
        Some(seconds) => seconds_to_hms(seconds.max(0.0) as i64),
        None => default_value.to_string(),
    }
}

pub(super) fn map_submission_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<SubmissionTaskRecord> {
    let import_mode_raw: Option<String> = row.get(22)?;
    let import_mode = normalize_import_mode(import_mode_raw.as_deref());
    let source_type_raw: Option<String> = row.get(23)?;
    let source_type = source_type_raw
        .as_deref()
        .unwrap_or("NORMAL")
        .trim()
        .to_string();
    let workflow_config_raw: Option<String> = row.get(24)?;
    let workflow_settings = parse_workflow_settings(
        workflow_config_raw.and_then(|raw| serde_json::from_str::<Value>(&raw).ok()),
    );
    let remote_status_ignored = row.get::<_, i64>(28)? != 0;
    let has_integrated_downloads: i64 = row.get(29)?;
    let workflow_status = row.get::<_, Option<String>>(30)?;
    let workflow_step = row.get::<_, Option<String>>(31)?;
    let workflow_progress: Option<f64> = row.get(32)?;
    let workflow_error_message: Option<String> = row.get(33)?;
    let workflow_status = workflow_status.map(|status| WorkflowStatusRecord {
        status,
        current_step: workflow_step,
        progress: workflow_progress.unwrap_or(0.0),
        error_message: workflow_error_message,
    });
    let enable_segmentation =
        workflow_settings.enable_segmentation || import_mode == IMPORT_MODE_SEGMENTED;

    Ok(SubmissionTaskRecord {
        task_id: row.get(0)?,
        status: row.get(1)?,
        priority: row.get::<_, i64>(2)? != 0,
        priority_at: row.get(3)?,
        title: row.get(4)?,
        description: row.get(5)?,
        cover_url: row.get(6)?,
        cover_local_path: row.get(7)?,
        partition_id: row.get(8)?,
        tags: row.get(9)?,
        topic_id: row.get(10)?,
        mission_id: row.get(11)?,
        activity_title: row.get(12)?,
        video_type: row.get(13)?,
        collection_id: row.get(14)?,
        bvid: row.get(15)?,
        aid: row.get(16)?,
        remote_state: row.get(17)?,
        remote_state_desc: row.get(35)?,
        reject_reason: row.get(18)?,
        remote_status_ignored,
        created_at: row.get(19)?,
        updated_at: row.get(20)?,
        segment_prefix: row.get(21)?,
        import_mode,
        source_type,
        enable_segmentation,
        baidu_sync_enabled: row.get::<_, i64>(25)? != 0,
        baidu_sync_path: row.get(26)?,
        baidu_sync_filename: row.get(27)?,
        has_integrated_downloads: has_integrated_downloads != 0,
        workflow_status,
        collection_section_id: row.get(34)?,
    })
}

pub(super) fn format_timecode_seconds(seconds: f64) -> String {
    let total = if seconds.is_finite() {
        seconds.max(0.0)
    } else {
        0.0
    };
    let total_millis = if total >= (i64::MAX as f64 / 1000.0) {
        i64::MAX
    } else {
        (total * 1000.0).round() as i64
    };

    let hours = total_millis / 3_600_000;
    let minutes = (total_millis % 3_600_000) / 60_000;
    let secs = (total_millis % 60_000) / 1000;
    let millis = total_millis % 1000;
    if millis == 0 {
        format!("{:02}:{:02}:{:02}", hours, minutes, secs)
    } else {
        format!("{:02}:{:02}:{:02}.{:03}", hours, minutes, secs, millis)
    }
}

pub(super) async fn check_sources_ready(
    context: &SubmissionContext,
    task_id: &str,
    sources: &[ClipSource],
) -> Result<Vec<ClipSource>, String> {
    let mut infos = Vec::with_capacity(sources.len());
    for source in sources {
        let path = Path::new(&source.input_path);
        let metadata = fs::metadata(path)
            .map_err(|err| format!("源文件不存在 input={} err={}", source.input_path, err))?;
        let size = metadata.len();
        if size == 0 {
            return Err(format!("源文件大小为0 input={}", source.input_path));
        }
        infos.push(SourceReadyInfo {
            source: source.clone(),
            path: source.input_path.clone(),
            size,
        });
    }

    sleep(Duration::from_secs(SOURCE_READY_STABLE_DELAY_SECS)).await;
    for info in &infos {
        let metadata = fs::metadata(&info.path)
            .map_err(|err| format!("源文件不存在 input={} err={}", info.path, err))?;
        if metadata.len() != info.size {
            return Err(format!("源文件仍在写入 input={}", info.path));
        }
    }

    let mut normalized = Vec::with_capacity(infos.len());
    for info in infos {
        let duration = probe_duration_seconds(Path::new(&info.path))
            .map_err(|err| format!("源文件不可读 input={} err={}", info.path, err))?;
        let mut start = info
            .source
            .start_time
            .as_deref()
            .and_then(|value| parse_time_to_seconds(value))
            .unwrap_or(0.0);
        let end_config = info
            .source
            .end_time
            .as_deref()
            .and_then(|value| parse_time_to_seconds(value));
        let mut end = end_config.unwrap_or(duration);
        let mut reset = false;

        if end <= 0.0 {
            end = duration;
            reset = true;
        }
        if let Some(config_end) = end_config {
            if config_end > duration {
                append_log(
                    &context.app_log_path,
                    &format!(
                        "submission_clip_time_clamp task_id={} input={} end={} duration={}",
                        task_id, info.path, config_end, duration
                    ),
                );
                let end_time = format_timecode_seconds(duration);
                let stored_path = to_stored_submission_path(context, &info.path);
                let raw_path = info.path.trim().to_string();
                let dual_path = !raw_path.is_empty() && raw_path != stored_path;
                let update_result = context.db.with_conn(|conn| {
                    conn.execute(
                        "UPDATE task_source_video SET end_time = ?1 \
             WHERE task_id = ?2 \
               AND (source_file_path = ?3 OR (?4 = 1 AND source_file_path = ?5)) \
               AND sort_order = ?6",
                        (
                            &end_time,
                            task_id,
                            stored_path.as_str(),
                            if dual_path { 1 } else { 0 },
                            raw_path.as_str(),
                            info.source.order,
                        ),
                    )
                });
                if let Err(err) = update_result {
                    append_log(
                        &context.app_log_path,
                        &format!(
                            "submission_clip_time_update_fail task_id={} input={} err={}",
                            task_id, info.path, err
                        ),
                    );
                }
                end = duration;
            }
        } else {
            end = duration;
        }
        if start < 0.0 || start >= end {
            start = 0.0;
            if end_config.is_none() {
                end = duration;
            }
            reset = true;
        }

        if reset {
            append_log(
                &context.app_log_path,
                &format!(
                    "submission_clip_time_reset task_id={} input={} start={} end={} duration={}",
                    task_id, info.path, start, end, duration
                ),
            );
        }

        let start_time = if start <= 0.0 {
            Some("00:00:00".to_string())
        } else {
            Some(format_timecode_seconds(start))
        };
        let end_time = Some(format_timecode_seconds(end));
        normalized.push(ClipSource {
            input_path: info.source.input_path,
            start_time,
            end_time,
            order: info.source.order,
        });
    }

    Ok(normalized)
}

pub(super) async fn ensure_sources_ready(
    context: &SubmissionContext,
    task_id: &str,
    sources: &[ClipSource],
    workflow_instance_id: &str,
) -> Result<Vec<ClipSource>, String> {
    let mut attempt = 0;
    let mut wait_secs = SOURCE_READY_STABLE_DELAY_SECS;
    loop {
        if !is_workflow_instance_latest(context, task_id, workflow_instance_id)? {
            append_log(
                &context.app_log_path,
                &format!(
                    "submission_workflow_superseded task_id={} instance_id={}",
                    task_id, workflow_instance_id
                ),
            );
            return Err("WORKFLOW_SUPERSEDED".to_string());
        }
        let _ = wait_for_workflow_ready(context, task_id).await?;
        match check_sources_ready(context, task_id, sources).await {
            Ok(normalized) => return Ok(normalized),
            Err(err) => {
                if !is_workflow_instance_latest(context, task_id, workflow_instance_id)? {
                    append_log(
                        &context.app_log_path,
                        &format!(
                            "submission_workflow_superseded task_id={} instance_id={}",
                            task_id, workflow_instance_id
                        ),
                    );
                    return Err("WORKFLOW_SUPERSEDED".to_string());
                }
                attempt += 1;
                append_log(
                    &context.app_log_path,
                    &format!(
                        "submission_sources_not_ready task_id={} attempt={} err={}",
                        task_id, attempt, err
                    ),
                );
                let _ = update_workflow_status(context, task_id, "VIDEO_DOWNLOADING", None, 0.0);
                let _ = update_submission_status(context, task_id, "PENDING");
                if attempt >= SOURCE_READY_MAX_RETRIES {
                    let _ = mark_submission_failure(context, task_id, None, &err);
                    return Err(err);
                }
                let sleep_secs = wait_secs.min(SOURCE_READY_MAX_WAIT_SECS);
                sleep(Duration::from_secs(sleep_secs)).await;
                wait_secs = (wait_secs * 2).min(SOURCE_READY_MAX_WAIT_SECS);
            }
        }
    }
}

pub(super) fn submission_batch_end_indexes(total_parts: usize) -> Vec<usize> {
    let mut indexes = Vec::new();
    let mut end_index = std::cmp::min(SUBMISSION_BATCH_PARTS, total_parts);
    while end_index > 0 {
        indexes.push(end_index);
        if end_index >= total_parts {
            break;
        }
        end_index = std::cmp::min(end_index + SUBMISSION_BATCH_PARTS, total_parts);
    }
    indexes
}

pub(super) fn is_rate_limit_error(err: &str) -> bool {
    err.contains("21540") || is_bilibili_risk_control_error(err)
}

pub(super) fn is_submission_created_error(err: &str) -> bool {
    err.contains(SUBMISSION_CREATED_PREFIX)
}

pub(super) fn is_local_address_unavailable_error(err: &str) -> bool {
    let lower = err.to_lowercase();
    lower.contains("can't assign requested address")
        || lower.contains("cannot assign requested address")
        || lower.contains("os error 49")
        || lower.contains("os error 99")
        || lower.contains("os error 10049")
}

pub(super) fn is_retryable_submission_error(err: &str) -> bool {
    if is_submit_result_unknown_error(err) || is_reupload_required_submission_error(err) {
        return false;
    }
    if is_already_submitted_submission_error(err) {
        return false;
    }
    let lower = err.to_lowercase();
    let keywords = [
        "预上传请求失败:",
        "上传元数据失败:",
        "上传分片失败:",
        "结束上传失败:",
        "error sending request",
        "timed out",
        "timeout",
        "connection reset",
        "connection closed",
        "connection refused",
        "broken pipe",
        "failed to resolve",
        "dns",
        "network",
        "unexpected eof",
        "tls",
        "http2",
        "os error",
    ];
    let cn_keywords = [
        "网络",
        "超时",
        "连接",
        "预上传解析失败重试次数已达上限",
        "存在分段上传失败",
    ];
    keywords.iter().any(|keyword| lower.contains(keyword))
        || cn_keywords.iter().any(|keyword| err.contains(keyword))
}

pub(super) fn is_already_submitted_submission_error(err: &str) -> bool {
    err.contains("稿件已成功投稿，请勿重新提交")
        || err.contains("同一视频不能重复投稿")
        || err.contains("请勿重新提交哦")
        || err.contains("(code: 21612)")
        || err.contains("(code: 21503)")
}

pub(super) fn is_submit_result_unknown_error(err: &str) -> bool {
    err.contains("SUBMIT_RESULT_UNKNOWN:")
}

pub(super) fn is_submit_network_busy_error(err: &str) -> bool {
    err.contains("(code: 69800)") || err.contains("网络繁忙")
}

pub(super) fn build_submit_result_unknown_error(err: &str) -> String {
    format!(
        "SUBMIT_RESULT_UNKNOWN: 最终提交返回网络繁忙，系统已停止自动重试并等待远端对账，避免重复投稿。请先执行远端校验；仅在确认远端未生成稿件后重新投稿。原始错误: {}",
        err
    )
}

pub(super) fn should_block_submission_retry(task: &SubmissionTaskRecord) -> bool {
    should_reconcile_submission(task)
}

pub(super) fn truncate_log_text(value: &str) -> String {
    const LIMIT: usize = 2000;
    if value.len() <= LIMIT {
        return value.to_string();
    }
    let mut truncated = value.chars().take(LIMIT).collect::<String>();
    truncated.push_str("...<truncated>");
    truncated
}

pub(super) fn format_error_chain(err: &dyn Error) -> String {
    let mut parts = vec![err.to_string()];
    let mut current = err.source();
    while let Some(source) = current {
        parts.push(source.to_string());
        current = source.source();
    }
    parts.join(" <- ")
}

pub(super) fn format_reqwest_error(err: &reqwest::Error) -> String {
    let mut flags = Vec::new();
    if err.is_builder() {
        flags.push("builder");
    }
    if err.is_connect() {
        flags.push("connect");
    }
    if err.is_timeout() {
        flags.push("timeout");
    }
    if err.is_request() {
        flags.push("request");
    }
    if err.is_body() {
        flags.push("body");
    }
    if err.is_decode() {
        flags.push("decode");
    }
    if err.is_redirect() {
        flags.push("redirect");
    }
    let kind = if flags.is_empty() {
        "unknown".to_string()
    } else {
        flags.join(",")
    };
    format!("{} [kind={}]", format_error_chain(err), kind)
}

pub(super) async fn try_complete_existing_submission(
    context: &UploadContext,
    submission_context: &SubmissionContext,
    task_id: &str,
    auth: &AuthInfo,
    expected_parts: usize,
    reason: &str,
) -> Result<bool, String> {
    let detail = load_task_detail(submission_context, task_id)?;
    let bvid = detail.task.bvid.clone().unwrap_or_default();
    let mut aid = detail.task.aid.unwrap_or(0);
    if bvid.trim().is_empty() && aid <= 0 {
        append_log(
            &context.app_log_path,
            &format!(
                "submission_existing_complete_skip task_id={} reason={} missing_refs=true",
                task_id, reason
            ),
        );
        return Ok(false);
    }
    if aid <= 0 && !bvid.trim().is_empty() {
        if let Some(resolved_aid) = fetch_aid_with_refresh(context, auth, bvid.trim()).await {
            aid = resolved_aid;
            let _ = update_submission_aid(submission_context, task_id, aid);
        }
    }
    if detail.task.collection_section_id.unwrap_or(0) > 0
        && detail
            .task
            .workflow_status
            .as_ref()
            .and_then(|status| status.error_message.as_deref())
            .map(|error| {
                is_collection_bind_pending_error(error)
                    || is_submission_created_error(error)
                    || is_submit_result_unknown_error(error)
            })
            .unwrap_or(false)
    {
        if let Err(err) =
            retry_pending_collection_binding(context, submission_context, task_id, auth, None).await
        {
            let reason = build_collection_bind_pending_error(&err);
            update_submission_status_with_reason(
                submission_context,
                task_id,
                SUBMISSION_STATUS_VERIFY_PENDING,
                Some(&reason),
            )?;
            return Err(reason);
        }
    }
    match verify_remote_part_count(context, auth, task_id, bvid.trim(), aid, expected_parts).await {
        Ok(()) => {
            update_submission_status(submission_context, task_id, "COMPLETED")?;
            append_log(
                &context.app_log_path,
                &format!(
                    "submission_existing_complete_ok task_id={} reason={} bvid={} aid={} parts={}",
                    task_id, reason, bvid, aid, expected_parts
                ),
            );
            Ok(true)
        }
        Err(err) => {
            let status = if is_remote_part_count_mismatch_error(&err) {
                "FAILED"
            } else {
                SUBMISSION_STATUS_VERIFY_PENDING
            };
            update_submission_status_with_reason(submission_context, task_id, status, Some(&err))?;
            append_log(
                &context.app_log_path,
                &format!(
                    "submission_existing_complete_verify_pending task_id={} reason={} bvid={} aid={} expected_parts={} status={} err={}",
                    task_id, reason, bvid, aid, expected_parts, status, err
                ),
            );
            Ok(true)
        }
    }
}

pub(super) fn build_part_title(prefix: Option<&str>, index: usize) -> String {
    let prefix = prefix.unwrap_or("").trim();
    if prefix.is_empty() {
        return format!("P{}", index);
    }
    format!("{}_part_{}", prefix, index)
}

pub(super) fn resolve_existing_part_title(
    task: &SubmissionTaskRecord,
    part_name: &str,
    index: usize,
) -> String {
    let trimmed = part_name.trim();
    if trimmed.is_empty() {
        return build_part_title(task.segment_prefix.as_deref(), index);
    }
    if trimmed == format!("Part {}", index) {
        return build_part_title(task.segment_prefix.as_deref(), index);
    }
    trimmed.to_string()
}

pub(super) fn is_default_part_name(part_name: &str, order: i64, segment_prefix: Option<&str>) -> bool {
    let trimmed = part_name.trim();
    if trimmed.is_empty() {
        return true;
    }
    if trimmed == format!("Part {}", order) {
        return true;
    }
    if order > 0 {
        let expected = build_part_title(segment_prefix, order as usize);
        return trimmed == expected;
    }
    false
}

pub(super) fn build_progress_snapshot(
    uploaded_bytes: u64,
    total_bytes: u64,
    last_part_index: u64,
) -> UploadProgressSnapshot {
    let progress = if total_bytes > 0 {
        (uploaded_bytes as f64 / total_bytes as f64) * 100.0
    } else {
        0.0
    };
    UploadProgressSnapshot {
        uploaded_bytes,
        total_bytes,
        progress: progress.min(100.0).max(0.0),
        last_part_index,
    }
}

pub(super) fn retry_after_seconds(headers: &HeaderMap) -> Option<u64> {
    headers
        .get("retry-after")
        .or_else(|| headers.get("Retry-After"))
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
}

pub(super) async fn wait_on_rate_limit(
    context: &SubmissionContext,
    task_id: &str,
    target: &UploadTarget,
    log_path: &PathBuf,
    retry_after: Option<u64>,
    stage: &str,
) {
    let wait = {
        let state = TASK_UPLOAD_RATE_LIMITS.get_or_init(|| AsyncMutex::new(HashMap::new()));
        let mut state = state.lock().await;
        let entry = state.entry(task_id.to_string()).or_default();
        let now = Instant::now();
        if let Some(blocked_until) = entry.blocked_until {
            if let Some(remaining) = blocked_until.checked_duration_since(now) {
                (remaining, entry.limiter.consecutive_406.max(1), true)
            } else {
                entry.blocked_until = None;
                let wait_secs = entry.limiter.next_wait_seconds(retry_after);
                let wait = Duration::from_secs(wait_secs);
                entry.blocked_until = now.checked_add(wait);
                (wait, entry.limiter.consecutive_406, false)
            }
        } else {
            let wait_secs = entry.limiter.next_wait_seconds(retry_after);
            let wait = Duration::from_secs(wait_secs);
            entry.blocked_until = now.checked_add(wait);
            (wait, entry.limiter.consecutive_406, false)
        }
    };
    let wait_secs = duration_to_wait_secs(wait.0);
    let _ = update_upload_status_for_target(context, target, "RATE_LIMITED");
    let _ = touch_submission_updated_at_for_target(context, target);
    append_log(
        log_path,
        &format!(
            "upload_rate_limited task_id={} stage={} target={} wait_secs={} count={} joined={}",
            task_id,
            stage,
            upload_target_label(target),
            wait_secs,
            wait.1,
            wait.2
        ),
    );
    wait_for_task_upload_rate_limit(log_path, task_id, target, stage).await;
    {
        let state = TASK_UPLOAD_RATE_LIMITS.get_or_init(|| AsyncMutex::new(HashMap::new()));
        let mut state = state.lock().await;
        if let Some(entry) = state.get_mut(task_id) {
            if entry
                .blocked_until
                .and_then(|blocked_until| blocked_until.checked_duration_since(Instant::now()))
                .is_none()
            {
                entry.blocked_until = None;
            }
            if entry.blocked_until.is_none() && entry.limiter.consecutive_406 == 0 {
                state.remove(task_id);
            }
        }
    }
    let _ = restore_upload_status_after_rate_limit(context, target);
    let _ = touch_submission_updated_at_for_target(context, target);
}

pub(super) async fn post_video_meta(
    context: &SubmissionContext,
    task_id: &str,
    target: &UploadTarget,
    client: &Client,
    auth: &AuthInfo,
    preupload: &PreuploadInfo,
    file_size: u64,
    log_path: &PathBuf,
) -> Result<String, String> {
    let url = build_upload_url(&preupload.endpoint, &preupload.upos_uri);
    let params = vec![
        ("uploads", "".to_string()),
        ("output", "json".to_string()),
        ("profile", "ugcfx/bup".to_string()),
        ("filesize", file_size.to_string()),
        ("partsize", preupload.chunk_size.to_string()),
        ("biz_id", preupload.biz_id.to_string()),
    ];
    loop {
        wait_for_task_upload_rate_limit(log_path, task_id, target, "post_meta").await;
        let mut headers = build_headers(Some(&auth.cookie))?;
        headers.insert(
            "X-Upos-Auth",
            HeaderValue::from_str(&preupload.auth).map_err(|_| "无效的X-Upos-Auth".to_string())?,
        );
        let response = client
            .post(url.clone())
            .headers(headers)
            .query(&params)
            .send()
            .await
            .map_err(|err| {
                let detail = format_reqwest_error(&err);
                append_log(
                    log_path,
                    &format!(
                        "upload_meta_request_error target={} file_size={} err={}",
                        upload_target_label(target),
                        file_size,
                        detail
                    ),
                );
                format!("上传元数据失败: {}", detail)
            })?;
        if response.status() == StatusCode::NOT_ACCEPTABLE {
            let retry_after = retry_after_seconds(response.headers());
            wait_on_rate_limit(context, task_id, target, log_path, retry_after, "post_meta").await;
            continue;
        }
        let value: Value = response
            .json()
            .await
            .map_err(|err| format!("上传元数据解析失败: {}", err))?;
        if let Some(code) = value.get("code").and_then(|val| val.as_i64()) {
            if code != 0 {
                let message = value
                    .get("message")
                    .and_then(|val| val.as_str())
                    .unwrap_or("上传元数据失败");
                return Err(format!("{} (code: {})", message, code));
            }
        }
        if let Some(ok) = value.get("OK").and_then(|val| val.as_i64()) {
            if ok != 1 {
                return Err("上传元数据失败".to_string());
            }
        }
        let upload_id = value
            .get("upload_id")
            .and_then(|val| val.as_str())
            .ok_or_else(|| "上传元数据缺少upload_id".to_string())?;
        reset_task_upload_rate_limit(task_id).await;
        return Ok(upload_id.to_string());
    }
}

pub(super) fn remove_file_extension(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((base, _)) => base.to_string(),
        None => name.to_string(),
    }
}

pub(super) fn decode_image_data_url(image_data_url: &str) -> Result<Vec<u8>, String> {
    let raw = image_data_url.trim();
    if !raw.starts_with("data:image/") {
        return Err("封面数据格式不合法，请重新选择图片".to_string());
    }
    let (meta, encoded) = raw
        .split_once(',')
        .ok_or_else(|| "封面数据格式不合法，请重新选择图片".to_string())?;
    if !meta.contains(";base64") {
        return Err("封面数据格式不合法，请重新选择图片".to_string());
    }
    STANDARD
        .decode(encoded)
        .map_err(|_| "封面数据解码失败，请重新选择图片".to_string())
}

pub(super) fn detect_image_mime(bytes: &[u8], path: &Path) -> Option<&'static str> {
    if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        return Some("image/jpeg");
    }
    if bytes.len() >= 8
        && bytes[0] == 0x89
        && bytes[1] == 0x50
        && bytes[2] == 0x4E
        && bytes[3] == 0x47
        && bytes[4] == 0x0D
        && bytes[5] == 0x0A
        && bytes[6] == 0x1A
        && bytes[7] == 0x0A
    {
        return Some("image/png");
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if bytes.len() >= 6 && (&bytes[0..6] == b"GIF87a" || &bytes[0..6] == b"GIF89a") {
        return Some("image/gif");
    }

    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());
    match ext.as_deref() {
        Some("jpg") | Some("jpeg") => Some("image/jpeg"),
        Some("png") => Some("image/png"),
        Some("webp") => Some("image/webp"),
        Some("gif") => Some("image/gif"),
        _ => None,
    }
}

pub(super) fn build_submission_videos(parts: &[UploadedVideoPart]) -> Vec<Value> {
    parts
        .iter()
        .map(|part| {
            let mut map = Map::new();
            if !part.filename.trim().is_empty() {
                map.insert("filename".to_string(), Value::String(part.filename.clone()));
            }
            map.insert("title".to_string(), Value::String(part.title.clone()));
            map.insert("desc".to_string(), Value::String(String::new()));
            map.insert("cid".to_string(), Value::Number(Number::from(part.cid)));
            Value::Object(map)
        })
        .collect()
}

pub(super) fn build_add_payload(task: &SubmissionTaskRecord, parts: &[UploadedVideoPart]) -> Value {
    let copyright = if task.video_type == "ORIGINAL" { 1 } else { 2 };
    let human_type2 = task.partition_id;
    let tags = task.tags.clone().unwrap_or_default();
    let desc = task.description.clone().unwrap_or_default();
    let cover = task.cover_url.clone().unwrap_or_default();
    let videos = build_submission_videos(parts);

    let mut payload = serde_json::json!({
      "videos": videos,
      "cover": cover,
      "cover43": "",
      "title": task.title,
      "copyright": copyright,
      "tid": task.partition_id,
      "human_type2": human_type2,
      "tag": tags,
      "desc_format_id": 9999,
      "desc": desc,
      "recreate": -1,
      "dynamic": "",
      "interactive": 0,
      "act_reserve_create": 0,
      "no_disturbance": 0,
      "no_reprint": 1,
      "subtitle": { "open": 0, "lan": "" },
      "dolby": 0,
      "lossless_music": 0,
      "up_selection_reply": false,
      "up_close_reply": false,
      "up_close_danmu": false,
      "web_os": 3
    });

    if task.collection_section_id.unwrap_or(0) <= 0 {
        if let Some(collection_id) = task.collection_id {
            if collection_id > 0 {
                payload["season_id"] = serde_json::json!(collection_id);
            }
        }
    }
    if let Some(topic_id) = task.topic_id {
        if topic_id > 0 {
            payload["topic_id"] = serde_json::json!(topic_id);
        }
    }
    if let Some(mission_id) = task.mission_id {
        if mission_id > 0 {
            payload["mission_id"] = serde_json::json!(mission_id);
        }
    }

    payload
}

pub(super) fn build_headers(cookie: Option<&str>) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    headers.insert(
    USER_AGENT,
    HeaderValue::from_static(
      "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/132.0.0.0 Safari/537.36 Edg/132.0.0.0",
    ),
  );
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/json, text/javascript, */*; q=0.01"),
    );
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

pub(super) fn normalize_binding_sources(mut sources: Vec<ClipSource>) -> Vec<ClipSource> {
    let mut seen = HashSet::new();
    sources.retain(|source| {
        let key = (source.input_path.clone(), source.order);
        seen.insert(key)
    });
    for (index, source) in sources.iter_mut().enumerate() {
        source.order = (index + 1) as i64;
    }
    sources
}

pub(super) fn validate_source_video_inputs(sources: &[SourceVideoInput]) -> Result<(), String> {
    fn parse_source_time_seconds(value: &str) -> Option<f64> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return None;
        }
        if trimmed == "00:00:00" {
            return Some(0.0);
        }
        parse_time_to_seconds(trimmed)
    }

    if sources.is_empty() {
        return Err("请至少添加一个源视频".to_string());
    }
    for (index, source) in sources.iter().enumerate() {
        let row = index + 1;
        let source_path = source.source_file_path.trim();
        let source_type = normalize_source_type(source);
        let remote_path = source.remote_path.as_deref().unwrap_or("").trim();
        if source_type == "BAIDU" && remote_path.is_empty() {
            return Err(format!("第{}行百度网盘源视频缺少远程路径", row));
        }
        if source_path.is_empty() && source_type != "BAIDU" {
            return Err(format!("第{}行源视频路径不能为空", row));
        }
        let start = match source
            .start_time
            .as_deref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        {
            Some(value) => parse_source_time_seconds(value)
                .ok_or_else(|| format!("第{}行开始时间格式不合法", row))?,
            None => 0.0,
        };
        let end = match source
            .end_time
            .as_deref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        {
            Some(value) => parse_source_time_seconds(value)
                .ok_or_else(|| format!("第{}行结束时间格式不合法", row))?,
            None if source_type == "BAIDU" => 0.0,
            None => return Err(format!("第{}行结束时间不能为空", row)),
        };
        if start < 0.0 || (source_type != "BAIDU" && end <= 0.0) {
            return Err(format!("第{}行时间范围不合法", row));
        }
        if end > 0.0 && start >= end {
            return Err(format!("第{}行开始时间必须小于结束时间", row));
        }
        if let Some(remote_cid) = source.remote_cid {
            if remote_cid <= 0 {
                return Err(format!("第{}行绑定分P无效", row));
            }
        }
    }
    Ok(())
}

pub(super) fn normalize_source_type(source: &SourceVideoInput) -> String {
    let value = source
        .source_type
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_ascii_uppercase();
    match value.as_str() {
        "BAIDU" => "BAIDU".to_string(),
        "DIRECT" => "DIRECT".to_string(),
        "BILIBILI" => "BILIBILI".to_string(),
        _ => {
            if source
                .remote_path
                .as_deref()
                .unwrap_or("")
                .trim()
                .is_empty()
                && source
                    .remote_bvid
                    .as_deref()
                    .unwrap_or("")
                    .trim()
                    .is_empty()
                && source
                    .remote_video_url
                    .as_deref()
                    .unwrap_or("")
                    .trim()
                    .is_empty()
            {
                "LOCAL".to_string()
            } else if source
                .remote_path
                .as_deref()
                .unwrap_or("")
                .trim()
                .starts_with('/')
            {
                "BAIDU".to_string()
            } else if source
                .remote_bvid
                .as_deref()
                .unwrap_or("")
                .trim()
                .is_empty()
            {
                "DIRECT".to_string()
            } else {
                "BILIBILI".to_string()
            }
        }
    }
}

pub(super) fn replace_source_videos(
    context: &SubmissionContext,
    task_id: &str,
    sources: &[SourceVideoInput],
) -> Result<(), String> {
    context
    .db
    .with_conn(|conn| {
      conn.execute("DELETE FROM task_source_video WHERE task_id = ?1", [task_id])?;
      for source in sources {
        let source_id = uuid::Uuid::new_v4().to_string();
        let source_type = normalize_source_type(source);
        let runtime_source_path = if source_type == "BAIDU" && source.source_file_path.trim().is_empty() {
          resolve_baidu_source_path_for_task(
            context,
            task_id,
            source.remote_path.as_deref().unwrap_or(""),
          )
          .to_string_lossy()
          .to_string()
        } else {
          source.source_file_path.trim().to_string()
        };
        let stored_source_path = to_stored_submission_path(context, &runtime_source_path);
        let (remote_bvid, remote_aid, remote_cid, remote_part_title, remote_video_url) =
          normalize_source_remote_binding(source);
        let remote_path = normalize_optional_text(source.remote_path.clone());
        let (upload_part_title, upload_part_title_mode) = normalize_upload_part_title(source);
        conn.execute(
          "INSERT INTO task_source_video (id, task_id, source_file_path, source_type, remote_path, sort_order, start_time, end_time, remote_video_url, remote_bvid, remote_aid, remote_cid, remote_part_title, upload_part_title, upload_part_title_mode) \
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
          (
            source_id,
            task_id,
            stored_source_path.as_str(),
            source_type.as_str(),
            remote_path.as_deref(),
            source.sort_order,
            source.start_time.as_deref(),
            source.end_time.as_deref(),
            remote_video_url.as_deref(),
            remote_bvid.as_deref(),
            remote_aid,
            remote_cid,
            remote_part_title.as_deref(),
            upload_part_title.as_deref(),
            upload_part_title_mode.as_str(),
          ),
        )?;
      }
      Ok(())
    })
    .map_err(|err| err.to_string())
}

pub(super) fn append_source_videos(
    context: &SubmissionContext,
    task_id: &str,
    sources: &[SourceVideoInput],
) -> Result<(), String> {
    context
    .db
    .with_conn(|conn| {
      let base_order: i64 = conn
        .query_row(
          "SELECT COALESCE(MAX(sort_order), 0) FROM task_source_video WHERE task_id = ?1",
          [task_id],
          |row| row.get(0),
        )
        .unwrap_or(0);
      for (index, source) in sources.iter().enumerate() {
        let source_id = uuid::Uuid::new_v4().to_string();
        let sort_order = base_order + index as i64 + 1;
        let source_type = normalize_source_type(source);
        let runtime_source_path = if source_type == "BAIDU" && source.source_file_path.trim().is_empty() {
          resolve_baidu_source_path_for_task(
            context,
            task_id,
            source.remote_path.as_deref().unwrap_or(""),
          )
          .to_string_lossy()
          .to_string()
        } else {
          source.source_file_path.trim().to_string()
        };
        let stored_source_path = to_stored_submission_path(context, &runtime_source_path);
        let (remote_bvid, remote_aid, remote_cid, remote_part_title, remote_video_url) =
          normalize_source_remote_binding(source);
        let remote_path = normalize_optional_text(source.remote_path.clone());
        let (upload_part_title, upload_part_title_mode) = normalize_upload_part_title(source);
        conn.execute(
          "INSERT INTO task_source_video (id, task_id, source_file_path, source_type, remote_path, sort_order, start_time, end_time, remote_video_url, remote_bvid, remote_aid, remote_cid, remote_part_title, upload_part_title, upload_part_title_mode) \
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
          (
            source_id,
            task_id,
            stored_source_path.as_str(),
            source_type.as_str(),
            remote_path.as_deref(),
            sort_order,
            source.start_time.as_deref(),
            source.end_time.as_deref(),
            remote_video_url.as_deref(),
            remote_bvid.as_deref(),
            remote_aid,
            remote_cid,
            remote_part_title.as_deref(),
            upload_part_title.as_deref(),
            upload_part_title_mode.as_str(),
          ),
        )?;
      }
      Ok(())
    })
    .map_err(|err| err.to_string())
}

pub(super) fn attach_update_sources(config: Value, sources: &[SourceVideoInput]) -> Value {
    let list = sources
        .iter()
        .enumerate()
        .map(|(index, source)| {
            let mut map = Map::new();
            map.insert(
                "sourceFilePath".to_string(),
                Value::String(source.source_file_path.clone()),
            );
            if let Some(remote_video_url) = source.remote_video_url.as_deref() {
                let trimmed = remote_video_url.trim();
                if !trimmed.is_empty() {
                    map.insert(
                        "remoteVideoUrl".to_string(),
                        Value::String(trimmed.to_string()),
                    );
                }
            }
            if let Some(remote_bvid) = source.remote_bvid.as_deref() {
                let trimmed = remote_bvid.trim();
                if !trimmed.is_empty() {
                    map.insert("remoteBvid".to_string(), Value::String(trimmed.to_string()));
                }
            }
            if let Some(remote_aid) = source.remote_aid.filter(|value| *value > 0) {
                map.insert(
                    "remoteAid".to_string(),
                    Value::Number(Number::from(remote_aid)),
                );
            }
            if let Some(remote_cid) = source.remote_cid.filter(|value| *value > 0) {
                map.insert(
                    "remoteCid".to_string(),
                    Value::Number(Number::from(remote_cid)),
                );
            }
            if let Some(remote_part_title) = source.remote_part_title.as_deref() {
                let trimmed = remote_part_title.trim();
                if !trimmed.is_empty() {
                    map.insert(
                        "remotePartTitle".to_string(),
                        Value::String(trimmed.to_string()),
                    );
                }
            }
            if let Some(start) = source.start_time.as_deref() {
                let trimmed = start.trim();
                if !trimmed.is_empty() {
                    map.insert("startTime".to_string(), Value::String(trimmed.to_string()));
                }
            }
            if let Some(end) = source.end_time.as_deref() {
                let trimmed = end.trim();
                if !trimmed.is_empty() {
                    map.insert("endTime".to_string(), Value::String(trimmed.to_string()));
                }
            }
            map.insert(
                "sortOrder".to_string(),
                Value::Number(Number::from(source.sort_order.max(1).max(index as i64 + 1))),
            );
            Value::Object(map)
        })
        .collect::<Vec<_>>();
    match config {
        Value::Object(mut map) => {
            map.insert("updateSources".to_string(), Value::Array(list));
            Value::Object(map)
        }
        _ => {
            let mut map = Map::new();
            map.insert("updateSources".to_string(), Value::Array(list));
            Value::Object(map)
        }
    }
}

pub(super) fn resolve_update_name_start_index(
    context: &SubmissionContext,
    task_id: &str,
    existing_count: usize,
    prefix: Option<&str>,
) -> Result<usize, String> {
    let prefix = prefix.unwrap_or("").trim();
    if !prefix.is_empty() {
        return Ok(1);
    }
    if existing_count > 0 {
        if let Ok(Some(max_index)) = load_max_part_index_from_names(context, task_id) {
            append_log(
                &context.app_log_path,
                &format!(
                    "submission_update_name_start_index task_id={} source=part_name max_index={}",
                    task_id, max_index
                ),
            );
            return Ok(max_index + 1);
        }
        if let Ok(Some(max_order)) = load_max_part_order(context, task_id) {
            append_log(
                &context.app_log_path,
                &format!(
                    "submission_update_name_start_index task_id={} source=part_order max_order={}",
                    task_id, max_order
                ),
            );
            return Ok(max_order + 1);
        }
        append_log(
            &context.app_log_path,
            &format!(
                "submission_update_name_start_index task_id={} source=existing_count value={}",
                task_id, existing_count
            ),
        );
        return Ok(existing_count + 1);
    }
    let has_uploaded_merged = context
    .db
    .with_conn(|conn| {
      let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM merged_video WHERE task_id = ?1 AND upload_cid IS NOT NULL AND upload_cid > 0",
        [task_id],
        |row| row.get(0),
      )?;
      Ok(count > 0)
    })
    .map_err(|err| err.to_string())?;
    if has_uploaded_merged {
        return Ok(2);
    }
    Ok(1)
}

pub(super) fn parse_part_index(name: &str) -> Option<usize> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(rest) = trimmed.strip_prefix('第') {
        let rest = rest.trim();
        let rest = rest
            .strip_suffix('P')
            .or_else(|| rest.strip_suffix('p'))
            .unwrap_or(rest);
        if let Some(value) = parse_leading_number(rest) {
            return Some(value);
        }
    }
    let upper = trimmed.to_uppercase();
    if let Some(rest) = upper.strip_prefix('P') {
        if let Some(value) = parse_leading_number(rest) {
            return Some(value);
        }
    }
    if let Some(rest) = upper.strip_prefix("PART") {
        if let Some(value) = parse_leading_number(rest) {
            return Some(value);
        }
    }
    let lower = trimmed.to_lowercase();
    if let Some(pos) = lower.rfind("part_") {
        let rest = &lower[(pos + "part_".len())..];
        if let Some(value) = parse_leading_number(rest) {
            return Some(value);
        }
    }
    None
}

pub(super) fn parse_leading_number(value: &str) -> Option<usize> {
    let trimmed = value.trim().trim_start_matches(['_', '-', ' ']);
    let digits: String = trimmed
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse::<usize>().ok()
}

pub(super) fn touch_submission_updated_at(context: &SubmissionContext, task_id: &str) -> Result<(), String> {
    let now = now_rfc3339();
    context
        .db
        .with_conn(|conn| {
            conn.execute(
                "UPDATE submission_task SET updated_at = ?1 WHERE task_id = ?2",
                (&now, task_id),
            )?;
            Ok(())
        })
        .map_err(|err| err.to_string())
}

pub(super) fn touch_submission_updated_at_for_target(
    context: &SubmissionContext,
    target: &UploadTarget,
) -> Result<(), String> {
    match target {
        UploadTarget::Segment(segment_id) => {
            let task_id: Option<String> = context
                .db
                .with_conn(|conn| {
                    conn.query_row(
                        "SELECT task_id FROM task_output_segment WHERE segment_id = ?1",
                        [segment_id],
                        |row| row.get(0),
                    )
                    .optional()
                })
                .map_err(|err| err.to_string())?;
            match task_id {
                Some(task_id) => touch_submission_updated_at(context, &task_id),
                None => Ok(()),
            }
        }
        UploadTarget::Merged(merged_id) => {
            let task_id: Option<String> = context
                .db
                .with_conn(|conn| {
                    conn.query_row(
                        "SELECT task_id FROM merged_video WHERE id = ?1",
                        [merged_id],
                        |row| row.get(0),
                    )
                    .optional()
                })
                .map_err(|err| err.to_string())?;
            match task_id {
                Some(task_id) => touch_submission_updated_at(context, &task_id),
                None => Ok(()),
            }
        }
        UploadTarget::EditSegment(_) => Ok(()),
    }
}

pub(super) fn default_part_name_from_path(path: &str) -> String {
    let name = Path::new(path)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("P")
        .trim();
    if name.is_empty() {
        "P".to_string()
    } else {
        name.to_string()
    }
}

pub(super) fn normalize_optional_text(value: Option<String>) -> Option<String> {
    value.and_then(|raw| {
        let trimmed = raw.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}

pub(super) async fn fetch_aid_by_bvid(
    context: &UploadContext,
    auth: Option<&AuthInfo>,
    bvid: &str,
) -> Option<i64> {
    let trimmed = bvid.trim();
    if trimmed.is_empty() {
        return None;
    }
    let url = format!("{}/x/web-interface/view", context.bilibili.base_url());
    let params = vec![("bvid".to_string(), trimmed.to_string())];
    let data = context
        .bilibili
        .get_json(&url, &params, auth, false)
        .await
        .ok()?;
    data.get("aid").and_then(|value| value.as_i64())
}

pub(super) async fn fetch_aid_with_refresh(
    context: &UploadContext,
    auth: &AuthInfo,
    bvid: &str,
) -> Option<i64> {
    if let Some(aid) = fetch_aid_by_bvid(context, Some(auth), bvid).await {
        return Some(aid);
    }
    let refreshed = refresh_auth(context, "fetch_aid").await.ok()?;
    fetch_aid_by_bvid(context, Some(&refreshed), bvid).await
}

pub(super) async fn refresh_auth_for_task(
    context: &UploadContext,
    task_id: &str,
    reason: &str,
) -> Result<AuthInfo, String> {
    append_log(
        &context.app_log_path,
        &format!(
            "submission_cookie_refresh_start task_id={} reason={}",
            task_id, reason
        ),
    );
    if let Some(owner_uid) = load_task_owner_uid(context.db.as_ref(), task_id)? {
        match login_refresh::refresh_cookie_by_uid(
            &context.bilibili,
            &context.login_store,
            &context.db,
            &context.app_log_path,
            owner_uid,
        )
        .await
        {
            Ok(auth) => {
                append_log(
                    &context.app_log_path,
                    &format!(
                        "submission_cookie_refresh_ok task_id={} owner_uid={} reason={}",
                        task_id, owner_uid, reason
                    ),
                );
                return Ok(auth);
            }
            Err(err) => {
                append_log(
                    &context.app_log_path,
                    &format!(
            "submission_cookie_refresh_owner_fail task_id={} owner_uid={} reason={} err={}",
            task_id, owner_uid, reason, err
          ),
                );
            }
        }
    }
    match login_refresh::refresh_cookie(
        &context.bilibili,
        &context.login_store,
        &context.db,
        &context.app_log_path,
    )
    .await
    {
        Ok(auth) => {
            append_log(
                &context.app_log_path,
                &format!(
                    "submission_cookie_refresh_ok task_id={} reason={}",
                    task_id, reason
                ),
            );
            Ok(auth)
        }
        Err(err) => {
            append_log(
                &context.app_log_path,
                &format!(
                    "submission_cookie_refresh_fail task_id={} reason={} err={}",
                    task_id, reason, err
                ),
            );
            Err(err)
        }
    }
}

pub(super) async fn refresh_auth(context: &UploadContext, reason: &str) -> Result<AuthInfo, String> {
    append_log(
        &context.app_log_path,
        &format!("submission_cookie_refresh_start reason={}", reason),
    );
    match login_refresh::refresh_cookie(
        &context.bilibili,
        &context.login_store,
        &context.db,
        &context.app_log_path,
    )
    .await
    {
        Ok(auth) => {
            append_log(
                &context.app_log_path,
                &format!("submission_cookie_refresh_ok reason={}", reason),
            );
            Ok(auth)
        }
        Err(err) => {
            append_log(
                &context.app_log_path,
                &format!(
                    "submission_cookie_refresh_fail reason={} err={}",
                    reason, err
                ),
            );
            Err(err)
        }
    }
}

pub(super) fn is_auth_error(err: &str) -> bool {
    err.contains("code: -101")
        || err.contains("code: -111")
        || err.contains("code: 86095")
        || err.contains("账号未登录")
        || err.contains("请先登录")
}

pub(super) fn normalize_submission_failure_reason(reason: &str) -> String {
    const LIMIT: usize = 1000;
    let trimmed = reason.trim();
    if trimmed.is_empty() {
        return "任务执行失败，请查看日志".to_string();
    }
    let mut normalized = trimmed.chars().take(LIMIT).collect::<String>();
    if trimmed.chars().count() > LIMIT {
        normalized.push_str("...");
    }
    normalized
}

pub(super) fn cleanup_unbound_run_dirs_for_task(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<(), String> {
    let runs_dir = resolve_submission_base_dir(context, task_id).join("runs");
    let entries = match fs::read_dir(&runs_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(format!(
                "读取运行目录失败: path={} err={}",
                runs_dir.to_string_lossy(),
                err
            ))
        }
    };

    let bound_instances: HashSet<String> = context
    .db
    .with_conn(|conn| {
      let mut set = HashSet::new();
      // 收集工作流实例绑定的 run 目录
      let mut stmt = conn.prepare("SELECT instance_id FROM workflow_instances WHERE task_id = ?1")?;
      let rows = stmt.query_map([task_id], |row| row.get::<_, String>(0))?;
      for row in rows {
        let instance_id = row?;
        let normalized = sanitize_filename(instance_id.trim());
        if !normalized.is_empty() {
          set.insert(normalized);
        }
      }
      // 收集 merged_video 引用的 run 目录（路径格式：{task_id}/runs/{instance_id}/...）
      let mut stmt2 = conn.prepare("SELECT video_path FROM merged_video WHERE task_id = ?1 AND video_path IS NOT NULL AND video_path != ''")?;
      let paths = stmt2.query_map([task_id], |row| row.get::<_, String>(0))?;
      for path in paths {
        let video_path = path?;
        // 从路径中提取 runs/{instance_id} 部分，格式: {task_id}/runs/{instance_id}/...
        let normalized_path = video_path.replace('\\', "/");
        let parts: Vec<&str> = normalized_path.splitn(5, '/').collect();
        if parts.len() >= 3 && parts[1] == "runs" {
          let instance_dir = sanitize_filename(parts[2].trim());
          if !instance_dir.is_empty() {
            set.insert(instance_dir);
          }
        }
      }
      Ok(set)
    })
    .map_err(|err| err.to_string())?;

    let mut removed_count = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().to_string();
        if bound_instances.contains(&dir_name) {
            continue;
        }
        remove_path_if_exists_with_retry(
            &context.app_log_path,
            "runs_unbound",
            &path,
            5,
            Duration::from_millis(300),
        )?;
        removed_count = removed_count.saturating_add(1);
    }

    if removed_count > 0 {
        append_log(
            &context.app_log_path,
            &format!(
                "submission_cleanup_unbound_runs_ok task_id={} removed={}",
                task_id, removed_count
            ),
        );
    }

    Ok(())
}

pub(super) fn resolve_submission_base_dir(context: &SubmissionContext, task_id: &str) -> PathBuf {
    let configured = load_download_settings_from_db(&context.db)
        .map(|settings| settings.download_path)
        .ok()
        .unwrap_or_default();
    let base = if configured.trim().is_empty() {
        default_download_dir()
    } else {
        PathBuf::from(configured.trim())
    };
    base.join(task_id)
}

pub(super) fn to_runtime_submission_path(context: &SubmissionContext, value: &str) -> String {
    let path = to_absolute_local_path_with_prefix(context.local_path_prefix.as_path(), value);
    if path.as_os_str().is_empty() {
        String::new()
    } else {
        path.to_string_lossy().to_string()
    }
}

pub(super) fn to_runtime_submission_path_opt(
    context: &SubmissionContext,
    value: Option<String>,
) -> Option<String> {
    to_absolute_local_path_opt_with_prefix(context.local_path_prefix.as_path(), value)
}

pub(super) fn to_stored_submission_path(context: &SubmissionContext, value: &str) -> String {
    to_stored_local_path_with_prefix(context.local_path_prefix.as_path(), value)
}

pub(super) fn is_process_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

pub(super) fn send_signal_to_process(pid: u32, signal: &str) -> Result<(), String> {
    let status = Command::new("kill")
        .args([signal, &pid.to_string()])
        .status()
        .map_err(|err| format!("发送 {} 信号失败 pid={} err={}", signal, pid, err))?;
    if status.success() {
        return Ok(());
    }
    Err(format!(
        "发送 {} 信号失败 pid={} status={}",
        signal, pid, status
    ))
}

pub(super) fn build_query_params(params: &[(String, String)]) -> String {
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for (key, value) in params {
        serializer.append_pair(key, value);
    }
    serializer.finish()
}

pub(super) fn truncate_log_value(value: &Value) -> String {
    let raw = value.to_string();
    const LIMIT: usize = 4000;
    if raw.len() <= LIMIT {
        return raw;
    }
    let mut truncated = raw.chars().take(LIMIT).collect::<String>();
    truncated.push_str("...<truncated>");
    truncated
}

pub(super) fn try_acquire_submission_owner(
    active_owners: &mut HashMap<(i64, String), String>,
    owner_key: (i64, String),
    worker_id: &str,
) -> bool {
    match active_owners.entry(owner_key) {
        std::collections::hash_map::Entry::Vacant(entry) => {
            entry.insert(worker_id.to_string());
            true
        }
        std::collections::hash_map::Entry::Occupied(_) => false,
    }
}

pub(super) fn release_submission_owner(
    active_owners: &mut HashMap<(i64, String), String>,
    owner_key: (i64, String),
    worker_id: &str,
) -> bool {
    if active_owners.get(&owner_key).map(String::as_str) != Some(worker_id) {
        return false;
    }
    active_owners.remove(&owner_key);
    true
}
