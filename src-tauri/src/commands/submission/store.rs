//! 投稿相关数据表的读写。

use super::*;

pub(super) fn load_integrate_current_bvid(config: Option<&Value>) -> bool {
    let Some(config) = config else {
        return false;
    };
    config
        .get("integrateCurrentBvid")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

pub(super) fn load_integrated_download_records(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<Vec<IntegratedDownloadRecord>, String> {
    context
        .db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT vd.id, vd.download_url, vd.bvid, vd.aid, vd.title, vd.part_title, \
                vd.part_count, vd.current_part, vd.local_path, vd.resolution, vd.codec, \
                vd.format, vd.cid, vd.content, vd.source_type \
         FROM task_relations tr \
         JOIN video_download vd ON tr.download_task_id = vd.id \
         WHERE tr.submission_task_id = ?1 AND tr.relation_type = 'INTEGRATED'",
            )?;
            let rows = stmt.query_map([task_id], |row| {
                Ok(IntegratedDownloadRecord {
                    id: row.get(0)?,
                    download_url: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    bvid: row.get(2)?,
                    aid: row.get(3)?,
                    title: row.get(4)?,
                    part_title: row.get(5)?,
                    part_count: row.get(6)?,
                    current_part: row.get(7)?,
                    local_path: row.get::<_, Option<String>>(8)?.unwrap_or_default(),
                    resolution: row.get(9)?,
                    codec: row.get(10)?,
                    format: row.get(11)?,
                    cid: row.get(12)?,
                    content: row.get(13)?,
                    source_type: row
                        .get::<_, Option<String>>(14)?
                        .unwrap_or_else(|| "BILIBILI".to_string()),
                })
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .map_err(|err| err.to_string())
        .map(|mut records| {
            for item in &mut records {
                item.local_path = to_runtime_submission_path(context, &item.local_path);
            }
            records
        })
}

pub(super) fn load_source_retry_bindings(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<HashMap<String, SourceRetryBinding>, String> {
    context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT source_file_path, source_type, remote_path, remote_video_url, remote_bvid, remote_aid, remote_cid, remote_part_title \
         FROM task_source_video WHERE task_id = ?1",
      )?;
      let rows = stmt.query_map([task_id], |row| {
          Ok(SourceRetryBinding {
            source_file_path: row.get(0)?,
            source_type: row.get::<_, Option<String>>(1)?.unwrap_or_else(|| "LOCAL".to_string()),
            remote_path: row.get(2)?,
            remote_video_url: row.get(3)?,
            remote_bvid: row.get(4)?,
            remote_aid: row.get::<_, Option<i64>>(5)?.map(|value| value.to_string()),
            remote_cid: row.get(6)?,
            remote_part_title: row.get(7)?,
        })
      })?;
      Ok(rows.collect::<Result<Vec<_>, _>>()?)
    })
    .map_err(|err| err.to_string())
    .map(|rows| {
      let mut bindings = HashMap::new();
      for row in rows {
        let runtime_path = normalize_runtime_source_path(context, &row.source_file_path);
        if runtime_path.trim().is_empty() {
          continue;
        }
        bindings.insert(runtime_path, row);
      }
      bindings
    })
}

pub(super) fn load_source_video_paths(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<Vec<String>, String> {
    context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT DISTINCT source_file_path FROM task_source_video WHERE task_id = ?1 ORDER BY sort_order ASC",
      )?;
      let rows = stmt.query_map([task_id], |row| row.get::<_, String>(0))?;
      let mut paths = Vec::new();
      for path in rows {
        if let Ok(value) = path {
          paths.push(to_runtime_submission_path(context, &value));
        }
      }
      Ok(paths)
    })
    .map_err(|err| err.to_string())
}

pub(super) fn find_active_references(
    context: &SubmissionContext,
    current_task_id: &str,
    file_path: &str,
) -> Result<Vec<DeleteConflictRef>, String> {
    let stored_path = to_stored_submission_path(context, file_path);
    let raw_path = file_path.trim().to_string();
    let dual_path = !raw_path.is_empty() && raw_path != stored_path;
    context
        .db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT st.task_id, st.status, st.title \
         FROM task_source_video tsv \
         JOIN submission_task st ON st.task_id = tsv.task_id \
         WHERE (tsv.source_file_path = ?1 OR (?2 = 1 AND tsv.source_file_path = ?3)) \
           AND tsv.task_id != ?4 AND st.status != 'COMPLETED'",
            )?;
            let rows = stmt.query_map(
                (
                    stored_path.as_str(),
                    if dual_path { 1 } else { 0 },
                    raw_path.as_str(),
                    current_task_id,
                ),
                |row| {
                    Ok(DeleteConflictRef {
                        task_id: row.get(0)?,
                        status: row.get(1)?,
                        title: row.get(2)?,
                    })
                },
            )?;
            let mut result = Vec::new();
            for item in rows {
                if let Ok(value) = item {
                    result.push(value);
                }
            }
            Ok(result)
        })
        .map_err(|err| err.to_string())
}

pub(super) fn set_json_field(record: &mut Value, key: &str, value: Value) {
    if let Some(map) = record.as_object_mut() {
        map.insert(key.to_string(), value);
    }
}

pub(super) fn insert_row_object(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    row: &Value,
    ignored_columns: &[&str],
) -> Result<(), String> {
    if !is_safe_identifier(table) {
        return Err(format!("非法表名: {}", table));
    }
    let row_map = row
        .as_object()
        .ok_or_else(|| format!("{} 记录格式错误", table))?;
    let mut columns = Vec::new();
    let mut values = Vec::new();
    for (column, value) in row_map {
        if ignored_columns.iter().any(|item| item == column) {
            continue;
        }
        if !is_safe_identifier(column) {
            return Err(format!("非法字段名: {}", column));
        }
        columns.push(column.clone());
        values.push(json_to_sql_value(value));
    }
    if columns.is_empty() {
        return Err(format!("{} 缺少可写入字段", table));
    }
    let placeholders = (1..=columns.len())
        .map(|idx| format!("?{}", idx))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "INSERT INTO {} ({}) VALUES ({})",
        table,
        columns.join(", "),
        placeholders
    );
    tx.execute(sql.as_str(), params_from_iter(values.iter()))
        .map_err(|err| err.to_string())?;
    Ok(())
}

pub(super) fn load_tasks(
    context: &SubmissionContext,
    status: Option<String>,
    page: i64,
    page_size: i64,
    query: Option<String>,
    bilibili_uid: i64,
    source_type: Option<String>,
) -> Result<PaginatedSubmissionTasks, String> {
    let non_empty = |value: String| {
        let trimmed = value.trim().to_string();
        (!trimmed.is_empty()).then_some(trimmed)
    };
    let like_query = query.and_then(non_empty).map(|value| format!("%{}%", value));
    let source_type = source_type
        .and_then(non_empty)
        .map(|value| value.to_ascii_uppercase());

    // 按筛选条件动态拼 WHERE，占位符编号与参数下标一一对应。
    let mut params: Vec<rusqlite::types::Value> = vec![bilibili_uid.into()];
    let mut conditions = vec!["st.bilibili_uid = ?1".to_string()];
    if let Some(status) = status {
        params.push(status.into());
        conditions.push(format!("st.status = ?{}", params.len()));
    }
    if let Some(source_type) = source_type {
        params.push(source_type.into());
        conditions.push(format!(
            "COALESCE(st.source_type, 'NORMAL') = ?{}",
            params.len()
        ));
    }
    if let Some(pattern) = like_query {
        params.push(pattern.into());
        conditions.push(format!(
            "(st.title LIKE ?{0} COLLATE NOCASE OR st.bvid LIKE ?{0} COLLATE NOCASE)",
            params.len()
        ));
    }
    let where_clause = conditions.join(" AND ");
    let offset = (page - 1).saturating_mul(page_size);

    context
    .db
    .with_conn(|conn| {
      let total: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM submission_task st WHERE {}", where_clause),
        rusqlite::params_from_iter(params.iter()),
        |row| row.get(0),
      )?;
      let order_by = "ORDER BY \
        CASE \
          WHEN st.status <> 'COMPLETED' THEN 0 \
          WHEN st.status = 'COMPLETED' AND (wi.status IS NULL OR wi.status <> 'COMPLETED') THEN 1 \
          WHEN st.status = 'COMPLETED' AND wi.status = 'COMPLETED' \
               AND (st.remote_state IS NULL OR st.remote_state = -30 OR st.remote_state IN (-2, -4)) THEN 2 \
          ELSE 3 \
        END, \
        CASE \
          WHEN st.status = 'COMPLETED' AND (wi.status IS NULL OR wi.status <> 'COMPLETED') THEN \
            CASE wi.current_step \
              WHEN 'CLIPPING' THEN 0 \
              WHEN 'MERGING' THEN 1 \
              WHEN 'SEGMENTING' THEN 2 \
              ELSE 9 \
            END \
          ELSE 9 \
        END, \
        CASE \
          WHEN st.status = 'COMPLETED' AND wi.status = 'COMPLETED' THEN \
            CASE \
              WHEN st.remote_state IS NULL OR st.remote_state = -30 THEN 0 \
              WHEN st.remote_state IN (-2, -4) THEN 1 \
              ELSE 2 \
            END \
          ELSE 9 \
        END, \
        CASE \
          WHEN st.status = 'WAITING_UPLOAD' THEN st.priority \
          ELSE 0 \
        END DESC, \
        CASE \
          WHEN st.status = 'WAITING_UPLOAD' AND COALESCE(st.priority, 0) = 1 THEN COALESCE(st.priority_at, st.updated_at) \
          ELSE NULL \
        END DESC, \
        CASE \
          WHEN st.status = 'WAITING_UPLOAD' AND COALESCE(st.priority, 0) = 0 THEN st.updated_at \
          ELSE NULL \
        END ASC, \
        st.created_at DESC";
      let sql = format!(
        "SELECT st.task_id, st.status, st.priority, st.priority_at, st.title, st.description, st.cover_url, st.cover_local_path, st.partition_id, st.tags, st.topic_id, st.mission_id, st.activity_title, st.video_type, st.collection_id, st.bvid, st.aid, st.remote_state, st.reject_reason, st.created_at, st.updated_at, st.segment_prefix, st.import_mode, st.source_type, wc.configuration_data, st.baidu_sync_enabled, st.baidu_sync_path, st.baidu_sync_filename, COALESCE(st.remote_status_ignored, 0), \
                CASE WHEN EXISTS (SELECT 1 FROM task_relations tr WHERE tr.submission_task_id = st.task_id) THEN 1 ELSE 0 END, \
                wi.status, wi.current_step, wi.progress, wi.error_message, st.collection_section_id, st.remote_state_desc \
         FROM submission_task st \
         LEFT JOIN workflow_instances wi ON wi.instance_id = ( \
           SELECT latest_wi.instance_id FROM workflow_instances latest_wi \
           WHERE latest_wi.task_id = st.task_id \
           ORDER BY latest_wi.created_at DESC LIMIT 1 \
         ) \
         LEFT JOIN workflow_configurations wc ON wi.configuration_id = wc.config_id \
         WHERE {} {} LIMIT ?{} OFFSET ?{}",
        where_clause,
        order_by,
        params.len() + 1,
        params.len() + 2
      );
      let mut query_params = params.clone();
      query_params.push(page_size.into());
      query_params.push(offset.into());
      let mut stmt = conn.prepare(&sql)?;
      let list = stmt
        .query_map(rusqlite::params_from_iter(query_params.iter()), map_submission_task)?
        .collect::<Result<Vec<_>, _>>()?;
      Ok(PaginatedSubmissionTasks {
        items: list,
        total,
        page,
        page_size,
      })
    })
    .map_err(|err| err.to_string())
}

pub(super) fn load_task_record(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<SubmissionTaskRecord, String> {
    context
    .db
    .with_conn(|conn| {
      conn.query_row(
        "SELECT st.task_id, st.status, st.priority, st.priority_at, st.title, st.description, st.cover_url, st.cover_local_path, st.partition_id, st.tags, st.topic_id, st.mission_id, st.activity_title, st.video_type, st.collection_id, st.bvid, st.aid, st.remote_state, st.reject_reason, st.created_at, st.updated_at, st.segment_prefix, st.import_mode, st.source_type, wc.configuration_data, st.baidu_sync_enabled, st.baidu_sync_path, st.baidu_sync_filename, COALESCE(st.remote_status_ignored, 0), \
                CASE WHEN EXISTS (SELECT 1 FROM task_relations tr WHERE tr.submission_task_id = st.task_id) THEN 1 ELSE 0 END, \
                wi.status, wi.current_step, wi.progress, wi.error_message, st.collection_section_id, st.remote_state_desc \
         FROM submission_task st \
         LEFT JOIN workflow_instances wi ON wi.instance_id = ( \
           SELECT latest_wi.instance_id FROM workflow_instances latest_wi \
           WHERE latest_wi.task_id = st.task_id \
           ORDER BY latest_wi.created_at DESC LIMIT 1 \
         ) \
         LEFT JOIN workflow_configurations wc ON wi.configuration_id = wc.config_id \
         WHERE st.task_id = ?1",
        [task_id],
        map_submission_task,
      )
    })
    .map_err(|err| err.to_string())
}

pub(super) fn load_task_detail(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<SubmissionTaskDetail, String> {
    let path_prefix = context.local_path_prefix.clone();
    let task = load_task_record(context, task_id)?;
    let workflow_config =
        load_effective_workflow_config_record(context, task_id)?.map(|(_, config)| config);
    let mut detail = context
    .db
    .with_conn(|conn| {
      let mut source_stmt = conn.prepare(
                "SELECT id, task_id, source_file_path, source_type, remote_path, remote_video_url, sort_order, start_time, end_time, remote_bvid, remote_aid, remote_cid, remote_part_title, upload_part_title, upload_part_title_mode FROM task_source_video WHERE task_id = ?1 ORDER BY sort_order ASC",
      )?;
      let source_videos = source_stmt
        .query_map([task_id], |row| {
          Ok(TaskSourceVideoRecord {
                    id: row.get(0)?,
                    task_id: row.get(1)?,
                    source_file_path: row.get(2)?,
                    source_type: row.get::<_, Option<String>>(3)?.unwrap_or_else(|| "LOCAL".to_string()),
                    remote_path: row.get(4)?,
                    remote_video_url: row.get(5)?,
                    sort_order: row.get(6)?,
                    start_time: row.get(7)?,
                    end_time: row.get(8)?,
                    remote_bvid: row.get(9)?,
                    remote_aid: row.get(10)?,
                    remote_cid: row.get(11)?,
                    remote_part_title: row.get(12)?,
                    upload_part_title: row.get(13)?,
                    upload_part_title_mode: row.get(14)?,
          })
        })?
        .collect::<Result<Vec<_>, _>>()?;

      let mut segment_stmt = conn.prepare(
        "SELECT segment_id, task_id, merged_id, part_name, segment_file_path, part_order, upload_status, cid, file_name, \
                upload_progress, upload_uploaded_bytes, upload_total_bytes, upload_session_id, upload_biz_id, \
                upload_endpoint, upload_auth, upload_uri, upload_chunk_size, upload_last_part_index \
         FROM task_output_segment WHERE task_id = ?1 ORDER BY part_order ASC",
      )?;
      let output_segments = segment_stmt
        .query_map([task_id], |row| {
          Ok(TaskOutputSegmentRecord {
            segment_id: row.get(0)?,
            task_id: row.get(1)?,
            merged_id: row.get(2)?,
            part_name: row.get(3)?,
            segment_file_path: row.get(4)?,
            part_order: row.get(5)?,
            upload_status: row.get(6)?,
            cid: row.get(7)?,
            file_name: row.get(8)?,
            upload_progress: row.get(9)?,
            upload_uploaded_bytes: row.get(10)?,
            upload_total_bytes: row.get(11)?,
            upload_session_id: row.get(12)?,
            upload_biz_id: row.get(13)?,
            upload_endpoint: row.get(14)?,
            upload_auth: row.get(15)?,
            upload_uri: row.get(16)?,
            upload_chunk_size: row.get(17)?,
            upload_last_part_index: row.get(18)?,
          })
        })?
        .collect::<Result<Vec<_>, _>>()?;

      let mut merged_stmt = conn.prepare(
        "SELECT id, task_id, file_name, video_path, remote_dir, remote_name, duration, status, \
                upload_progress, upload_uploaded_bytes, upload_total_bytes, upload_cid, upload_file_name, \
                upload_session_id, upload_biz_id, upload_endpoint, upload_auth, upload_uri, upload_chunk_size, \
                upload_last_part_index, create_time, update_time \
         FROM merged_video WHERE task_id = ?1 ORDER BY CASE WHEN COALESCE(sort_order, 0) > 0 THEN 0 ELSE 1 END ASC, COALESCE(sort_order, 0) ASC, create_time DESC, id DESC",
      )?;
      let merged_videos = merged_stmt
        .query_map([task_id], |row| {
          Ok(MergedVideoRecord {
            id: row.get(0)?,
            task_id: row.get(1)?,
            file_name: row.get(2)?,
            video_path: row.get(3)?,
            source_video_ids: Vec::new(),
            source_video_paths: Vec::new(),
            remote_dir: row.get(4)?,
            remote_name: row.get(5)?,
            duration: row.get(6)?,
            status: row.get(7)?,
            upload_progress: row.get(8)?,
            upload_uploaded_bytes: row.get(9)?,
            upload_total_bytes: row.get(10)?,
            upload_cid: row.get(11)?,
            upload_file_name: row.get(12)?,
            upload_session_id: row.get(13)?,
            upload_biz_id: row.get(14)?,
            upload_endpoint: row.get(15)?,
            upload_auth: row.get(16)?,
            upload_uri: row.get(17)?,
            upload_chunk_size: row.get(18)?,
            upload_last_part_index: row.get(19)?,
            create_time: row.get(20)?,
            update_time: row.get(21)?,
          })
        })?
        .collect::<Result<Vec<_>, _>>()?;
      let source_id_by_stored_path = source_videos
        .iter()
        .map(|item| (item.source_file_path.clone(), item.id.clone()))
        .collect::<HashMap<_, _>>();
      let mut merged_bindings: HashMap<i64, (Vec<String>, Vec<String>)> = HashMap::new();
      let mut merged_source_stmt = conn.prepare(
        "SELECT merged_id, source_id, source_file_path \
         FROM merged_source_video WHERE task_id = ?1 ORDER BY merged_id ASC, sort_order ASC, id ASC",
      )?;
      let merged_source_rows = merged_source_stmt.query_map([task_id], |row| {
        Ok((
          row.get::<_, i64>(0)?,
          row.get::<_, Option<String>>(1)?,
          row.get::<_, String>(2)?,
        ))
      })?;
      for row in merged_source_rows {
        let (merged_id, source_id_raw, source_path) = row?;
        let entry = merged_bindings
          .entry(merged_id)
          .or_insert_with(|| (Vec::new(), Vec::new()));
        if !source_path.trim().is_empty() && !entry.1.contains(&source_path) {
          entry.1.push(source_path.clone());
        }
        let resolved_source_id = source_id_raw
          .as_deref()
          .map(|value| value.trim().to_string())
          .filter(|value| !value.is_empty())
          .or_else(|| source_id_by_stored_path.get(&source_path).cloned());
        if let Some(source_id) = resolved_source_id {
          if !entry.0.contains(&source_id) {
            entry.0.push(source_id);
          }
        }
      }
      let mut merged_videos = merged_videos;
      for merged in &mut merged_videos {
        if let Some((source_ids, source_paths)) = merged_bindings.remove(&merged.id) {
          merged.source_video_ids = source_ids;
          merged.source_video_paths = source_paths;
        }
      }

      Ok(SubmissionTaskDetail {
        task,
        source_videos,
        output_segments,
        merged_videos,
        workflow_config,
      })
    })
    .map_err(|err| err.to_string())?;

    for source in &mut detail.source_videos {
        let path =
            to_absolute_local_path_with_prefix(path_prefix.as_path(), &source.source_file_path);
        source.source_file_path = if path.as_os_str().is_empty() {
            String::new()
        } else {
            path.to_string_lossy().to_string()
        };
    }
    for segment in &mut detail.output_segments {
        let path =
            to_absolute_local_path_with_prefix(path_prefix.as_path(), &segment.segment_file_path);
        segment.segment_file_path = if path.as_os_str().is_empty() {
            String::new()
        } else {
            path.to_string_lossy().to_string()
        };
    }
    for merged in &mut detail.merged_videos {
        merged.video_path = to_absolute_local_path_opt_with_prefix(
            path_prefix.as_path(),
            merged.video_path.clone(),
        );
        for source_path in &mut merged.source_video_paths {
            *source_path = to_absolute_local_path_with_prefix(path_prefix.as_path(), source_path)
                .to_string_lossy()
                .to_string();
        }
    }

    Ok(detail)
}

pub(super) fn find_unique_unknown_submission_match<'a>(
    task: &RemoteTrackedTask,
    remote_map: &'a HashMap<String, RemoteAuditInfo>,
    bound_bvids: &HashSet<String>,
    now_timestamp: i64,
) -> Option<&'a RemoteAuditInfo> {
    if task.expected_parts == 0 || task.title.trim().is_empty() {
        return None;
    }
    let created_timestamp = DateTime::parse_from_rfc3339(task.created_at.trim())
        .ok()?
        .timestamp();
    let reconciliation_timestamp = DateTime::parse_from_rfc3339(task.reconciliation_at.trim())
        .ok()?
        .timestamp();
    let earliest_timestamp = created_timestamp
        .min(reconciliation_timestamp)
        .saturating_sub(REMOTE_UNKNOWN_MATCH_WINDOW_SECS);
    let latest_timestamp = reconciliation_timestamp
        .saturating_add(REMOTE_UNKNOWN_MATCH_WINDOW_SECS)
        .min(now_timestamp);
    let mut matches = remote_map.values().filter(|info| {
        !bound_bvids.contains(info.bvid.as_str())
            && info.aid > 0
            && info.title.trim() == task.title.trim()
            && (info.part_count == 0 || info.part_count == task.expected_parts)
            && info.ctime >= earliest_timestamp
            && info.ctime <= latest_timestamp
    });
    let first = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(first)
}

pub(super) fn load_source_videos(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<Vec<ClipSource>, String> {
    context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT source_file_path, start_time, end_time, sort_order FROM task_source_video WHERE task_id = ?1 ORDER BY sort_order ASC",
      )?;
      let rows = stmt.query_map([task_id], |row| {
        Ok(ClipSource {
          input_path: row.get(0)?,
          start_time: row.get(1)?,
          end_time: row.get(2)?,
          order: row.get(3)?,
        })
      })?;

      let mut list = rows.collect::<Result<Vec<_>, _>>()?;
      for item in &mut list {
        item.input_path = to_runtime_submission_path(context, &item.input_path);
      }
      Ok(list)
    })
    .map_err(|err| err.to_string())
}

pub(super) fn load_task_source_video_records(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<Vec<TaskSourceVideoRecord>, String> {
    context
        .db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, task_id, source_file_path, source_type, remote_path, remote_video_url, sort_order, start_time, end_time, remote_bvid, remote_aid, remote_cid, remote_part_title \
         FROM task_source_video WHERE task_id = ?1 ORDER BY sort_order ASC",
            )?;
            let rows = stmt.query_map([task_id], |row| {
                Ok(TaskSourceVideoRecord {
                    id: row.get(0)?,
                    task_id: row.get(1)?,
                    source_file_path: row.get(2)?,
                    source_type: row.get::<_, Option<String>>(3)?.unwrap_or_else(|| "LOCAL".to_string()),
                    remote_path: row.get(4)?,
                    remote_video_url: row.get(5)?,
                    sort_order: row.get(6)?,
                    start_time: row.get(7)?,
                    end_time: row.get(8)?,
                    remote_bvid: row.get(9)?,
                    remote_aid: row.get(10)?,
                    remote_cid: row.get(11)?,
                    remote_part_title: row.get(12)?,
                    upload_part_title: None,
                    upload_part_title_mode: None,
                })
            })?;
            let mut list = rows.collect::<Result<Vec<_>, _>>()?;
            for item in &mut list {
                item.source_file_path = to_runtime_submission_path(context, &item.source_file_path);
            }
            Ok(list)
        })
        .map_err(|err| err.to_string())
}

pub(super) fn load_update_sources(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<Option<Vec<ClipSource>>, String> {
    let config = load_latest_workflow_config(context, task_id)?;
    let Some(config) = config else {
        return Ok(None);
    };
    let Some(list) = config
        .get("updateSources")
        .and_then(|value| value.as_array())
    else {
        return Ok(None);
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
    if sources.is_empty() {
        return Ok(None);
    }
    sources.sort_by_key(|item| item.order);
    Ok(Some(sources))
}

pub(super) fn load_max_part_index_from_names(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<Option<usize>, String> {
    context
        .db
        .with_conn(|conn| {
            let mut stmt =
                conn.prepare("SELECT part_name FROM task_output_segment WHERE task_id = ?1")?;
            let rows = stmt.query_map([task_id], |row| row.get::<_, String>(0))?;
            let mut max_index: Option<usize> = None;
            for row in rows {
                let name = row?;
                if let Some(index) = parse_part_index(&name) {
                    max_index = Some(max_index.map_or(index, |current| current.max(index)));
                }
            }
            Ok(max_index)
        })
        .map_err(|err| err.to_string())
}

pub(super) fn load_max_part_order(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<Option<usize>, String> {
    context
        .db
        .with_conn(|conn| {
            let max_order: i64 = conn.query_row(
                "SELECT COALESCE(MAX(part_order), 0) FROM task_output_segment WHERE task_id = ?1",
                [task_id],
                |row| row.get(0),
            )?;
            if max_order <= 0 {
                return Ok(None);
            }
            Ok(Some(max_order as usize))
        })
        .map_err(|err| err.to_string())
}

pub(super) fn update_submission_bvid_and_aid(
    context: &SubmissionContext,
    task_id: &str,
    bvid: &str,
    aid: i64,
) -> Result<(), String> {
    let now = now_rfc3339();
    context
        .db
        .with_conn(|conn| {
            conn.execute(
        "UPDATE submission_task SET bvid = ?1, aid = ?2, updated_at = ?3 WHERE task_id = ?4",
        (bvid, aid, &now, task_id),
      )?;
            Ok(())
        })
        .map_err(|err| err.to_string())
}

pub(super) fn update_submission_aid(
    context: &SubmissionContext,
    task_id: &str,
    aid: i64,
) -> Result<(), String> {
    let now = now_rfc3339();
    context
        .db
        .with_conn(|conn| {
            conn.execute(
                "UPDATE submission_task SET aid = ?1, updated_at = ?2 WHERE task_id = ?3",
                (aid, &now, task_id),
            )?;
            Ok(())
        })
        .map_err(|err| err.to_string())
}

pub(super) fn load_task_status(context: &SubmissionContext, task_id: &str) -> Result<String, String> {
    context
        .db
        .with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT status FROM submission_task WHERE task_id = ?1")?;
            let status = stmt.query_row([task_id], |row| row.get(0))?;
            Ok(status)
        })
        .map_err(|err| err.to_string())
}

pub(super) fn load_integrated_download_stats(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<Option<IntegratedDownloadStats>, String> {
    context
        .db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT \
          COUNT(*) AS total, \
          SUM(CASE WHEN vd.status = 2 THEN 1 ELSE 0 END) AS completed, \
          SUM(CASE WHEN vd.status = 3 THEN 1 ELSE 0 END) AS failed \
         FROM task_relations tr \
         JOIN video_download vd ON tr.download_task_id = vd.id \
         WHERE tr.submission_task_id = ?1 AND tr.relation_type = 'INTEGRATED'",
            )?;
            let row = stmt.query_row([task_id], |row| {
                let total: i64 = row.get(0)?;
                let completed: Option<i64> = row.get(1)?;
                let failed: Option<i64> = row.get(2)?;
                Ok(IntegratedDownloadStats {
                    total,
                    completed: completed.unwrap_or(0),
                    failed: failed.unwrap_or(0),
                })
            })?;
            if row.total == 0 {
                return Ok(None);
            }
            Ok(Some(row))
        })
        .map_err(|err| err.to_string())
}

pub(super) async fn load_auth_or_refresh(context: &UploadContext, reason: &str) -> Result<AuthInfo, String> {
    if let Some(auth) = context
        .login_store
        .load_auth_info(&context.db)
        .ok()
        .flatten()
    {
        return Ok(auth);
    }
    refresh_auth(context, reason).await
}

pub(super) async fn load_auth_or_refresh_for_task(
    context: &UploadContext,
    task_id: &str,
    reason: &str,
) -> Result<AuthInfo, String> {
    if let Some(owner_uid) = load_task_owner_uid(context.db.as_ref(), task_id)? {
        if let Some(auth) = context
            .login_store
            .load_auth_info_by_uid(&context.db, owner_uid)
            .ok()
            .flatten()
        {
            return Ok(auth);
        }
    }
    load_auth_or_refresh(context, reason).await
}

pub(super) fn load_task_owner_uid(db: &Db, task_id: &str) -> Result<Option<i64>, String> {
    db.with_conn(|conn| {
        conn.query_row(
            "SELECT bilibili_uid FROM submission_task WHERE task_id = ?1",
            [task_id],
            |row| row.get::<_, Option<i64>>(0),
        )
        .optional()
        .map(|value| value.flatten())
    })
    .map_err(|err| err.to_string())
}

pub(super) fn update_submission_status(
    context: &SubmissionContext,
    task_id: &str,
    status: &str,
) -> Result<(), String> {
    update_submission_status_with_reason(context, task_id, status, None)
}

pub(super) fn update_submission_status_with_reason(
    context: &SubmissionContext,
    task_id: &str,
    status: &str,
    reason: Option<&str>,
) -> Result<(), String> {
    let now = now_rfc3339();
    let normalized_reason = reason
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());
    let fallback_reason = if status == "FAILED" {
        "任务执行失败，请查看日志"
    } else {
        "等待远端稿件状态确认"
    };
    let final_reason = if status == "FAILED" || is_submission_reconciliation_status(status) {
        Some(normalized_reason.unwrap_or_else(|| fallback_reason.to_string()))
    } else {
        None
    };
    context
        .db
        .with_conn(|conn| {
            if status == "COMPLETED" {
                conn.execute(
                    "UPDATE submission_task SET status = ?1, updated_at = ?2, priority = 0, priority_at = NULL WHERE task_id = ?3",
                    (status, &now, task_id),
                )?;
            } else {
                conn.execute(
                    "UPDATE submission_task SET status = ?1, updated_at = ?2 WHERE task_id = ?3",
                    (status, &now, task_id),
                )?;
            }
            if status == "FAILED" {
                conn.execute(
          "UPDATE workflow_instances SET status = 'FAILED', progress = 0, error_message = ?1, completed_at = COALESCE(completed_at, ?2), updated_at = ?2 WHERE task_id = ?3",
          (final_reason.as_deref(), &now, task_id),
        )?;
            } else if status == "COMPLETED" {
                conn.execute(
          "UPDATE workflow_instances SET status = 'COMPLETED', current_step = NULL, progress = 100, error_message = NULL, completed_at = ?1, updated_at = ?1 WHERE task_id = ?2",
          (&now, task_id),
        )?;
            } else if is_submission_reconciliation_status(status) {
                conn.execute(
          "UPDATE workflow_instances SET status = ?1, current_step = NULL, progress = 100, error_message = ?2, completed_at = NULL, updated_at = ?3 WHERE task_id = ?4",
          (status, final_reason.as_deref(), &now, task_id),
        )?;
            } else {
                conn.execute(
          "UPDATE workflow_instances SET error_message = NULL, updated_at = ?1 WHERE task_id = ?2",
          (&now, task_id),
        )?;
            }
            Ok(())
        })
        .map_err(|err| err.to_string())?;

    if status == "FAILED" || status == "COMPLETED" {
        if let Err(err) = cleanup_unbound_run_dirs_for_task(context, task_id) {
            append_log(
                &context.app_log_path,
                &format!(
                    "submission_cleanup_unbound_runs_fail task_id={} status={} err={}",
                    task_id, status, err
                ),
            );
        }
    }

    Ok(())
}

pub(super) fn mark_submission_failure(
    context: &SubmissionContext,
    task_id: &str,
    current_step: Option<&str>,
    reason: &str,
) -> Result<(), String> {
    let normalized_reason = normalize_submission_failure_reason(reason);
    let mut first_err: Option<String> = None;

    if let Err(err) = update_submission_status_with_reason(
        context,
        task_id,
        "FAILED",
        Some(normalized_reason.as_str()),
    ) {
        first_err = Some(err);
    }
    if let Err(err) = update_workflow_status(context, task_id, "FAILED", current_step, 0.0) {
        if first_err.is_none() {
            first_err = Some(err);
        }
    }

    match first_err {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

pub(super) fn load_task_ids_by_status(
    context: &SubmissionContext,
    status: &str,
) -> Result<Vec<String>, String> {
    context
        .db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT task_id FROM submission_task WHERE status = ?1 ORDER BY updated_at ASC",
            )?;
            let rows = stmt.query_map([status], |row| row.get(0))?;
            let list = rows.collect::<Result<Vec<String>, _>>()?;
            Ok(list)
        })
        .map_err(|err| err.to_string())
}
