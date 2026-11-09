//! 投稿任务的导入导出。

use super::*;

pub(super) fn normalize_import_mode(value: Option<&str>) -> String {
    match value.unwrap_or("").trim().to_ascii_uppercase().as_str() {
        IMPORT_MODE_SEGMENTED => IMPORT_MODE_SEGMENTED.to_string(),
        _ => IMPORT_MODE_NON_SEGMENTED.to_string(),
    }
}

pub(super) fn normalize_submission_export_task_bundle_paths(
    bundle: &mut SubmissionExportTaskBundle,
    path_prefix: &Path,
) {
    normalize_json_path_field_with_prefix(
        &mut bundle.submission_task,
        "cover_local_path",
        path_prefix,
    );

    for source in &mut bundle.task_source_videos {
        normalize_json_path_field_with_prefix(source, "source_file_path", path_prefix);
    }
    for clip in &mut bundle.video_clips {
        normalize_json_path_field_with_prefix(clip, "clip_path", path_prefix);
    }
    for merged in &mut bundle.merged_videos {
        normalize_json_path_field_with_prefix(merged, "video_path", path_prefix);
    }
    for merged_source in &mut bundle.merged_source_videos {
        normalize_json_path_field_with_prefix(merged_source, "source_file_path", path_prefix);
    }
    for segment in &mut bundle.task_output_segments {
        normalize_json_path_field_with_prefix(segment, "segment_file_path", path_prefix);
    }
    for download in &mut bundle.video_downloads {
        normalize_json_path_field_with_prefix(download, "local_path", path_prefix);
    }
    for sync_task in &mut bundle.baidu_sync_tasks {
        normalize_json_path_field_with_prefix(sync_task, "local_path", path_prefix);
    }
}

pub(super) fn collect_submission_export_task_bundle(
    conn: &rusqlite::Connection,
    task_id: &str,
    path_prefix: &Path,
) -> rusqlite::Result<Option<SubmissionExportTaskBundle>> {
    let submission_task = match query_optional_row_json(
        conn,
        "SELECT * FROM submission_task WHERE task_id = ?1",
        [task_id],
    )? {
        Some(value) => value,
        None => return Ok(None),
    };
    let task_source_videos = query_rows_json(
        conn,
        "SELECT * FROM task_source_video WHERE task_id = ?1 ORDER BY sort_order ASC",
        [task_id],
    )?;
    let video_clips = query_rows_json(
        conn,
        "SELECT * FROM video_clip WHERE task_id = ?1 ORDER BY sequence ASC, id ASC",
        [task_id],
    )?;
    let merged_videos = query_rows_json(
    conn,
    "SELECT * FROM merged_video WHERE task_id = ?1 ORDER BY CASE WHEN COALESCE(sort_order, 0) > 0 THEN 0 ELSE 1 END ASC, COALESCE(sort_order, 0) ASC, create_time ASC, id ASC",
    [task_id],
  )?;
    let merged_source_videos = query_rows_json(
        conn,
        "SELECT * FROM merged_source_video WHERE task_id = ?1 ORDER BY sort_order ASC, id ASC",
        [task_id],
    )?;
    let task_output_segments = query_rows_json(
    conn,
    "SELECT * FROM task_output_segment WHERE task_id = ?1 ORDER BY part_order ASC, segment_id ASC",
    [task_id],
  )?;
    let workflow_instances = query_rows_json(
    conn,
    "SELECT * FROM workflow_instances WHERE task_id = ?1 ORDER BY created_at ASC, instance_id ASC",
    [task_id],
  )?;
    let mut workflow_steps = Vec::new();
    let mut workflow_execution_logs = Vec::new();
    let mut workflow_performance_metrics = Vec::new();
    for instance in &workflow_instances {
        if let Some(instance_id) = json_string_field_non_empty(instance, "instance_id") {
            workflow_steps.extend(query_rows_json(
        conn,
        "SELECT * FROM workflow_steps WHERE instance_id = ?1 ORDER BY step_order ASC, step_id ASC",
        [instance_id.as_str()],
      )?);
            workflow_execution_logs.extend(query_rows_json(
        conn,
        "SELECT * FROM workflow_execution_logs WHERE instance_id = ?1 ORDER BY created_at ASC, log_id ASC",
        [instance_id.as_str()],
      )?);
            workflow_performance_metrics.extend(query_rows_json(
        conn,
        "SELECT * FROM workflow_performance_metrics WHERE instance_id = ?1 ORDER BY measurement_time ASC, metric_id ASC",
        [instance_id.as_str()],
      )?);
        }
    }
    let task_relations = query_rows_json(
        conn,
        "SELECT * FROM task_relations WHERE submission_task_id = ?1 ORDER BY id ASC",
        [task_id],
    )?;
    let mut download_ids = HashSet::new();
    for relation in &task_relations {
        if let Some(download_id) = json_i64_field(relation, "download_task_id") {
            download_ids.insert(download_id);
        }
    }
    let mut video_downloads = Vec::new();
    let mut ordered_download_ids = download_ids.into_iter().collect::<Vec<_>>();
    ordered_download_ids.sort_unstable();
    for download_id in ordered_download_ids {
        if let Some(record) = query_optional_row_json(
            conn,
            "SELECT * FROM video_download WHERE id = ?1",
            [download_id],
        )? {
            video_downloads.push(record);
        }
    }
    let baidu_sync_tasks = query_rows_json(
    conn,
    "SELECT * FROM baidu_sync_task WHERE source_type = 'submission_merged' AND source_id = ?1 ORDER BY created_at ASC, id ASC",
    [task_id],
  )?;
    let mut bundle = SubmissionExportTaskBundle {
        task_id: task_id.to_string(),
        submission_task,
        task_source_videos,
        video_clips,
        merged_videos,
        merged_source_videos,
        task_output_segments,
        workflow_instances,
        workflow_steps,
        workflow_execution_logs,
        workflow_performance_metrics,
        task_relations,
        video_downloads,
        baidu_sync_tasks,
    };
    normalize_submission_export_task_bundle_paths(&mut bundle, path_prefix);
    Ok(Some(bundle))
}

pub(super) fn import_submission_task_bundle(
    tx: &rusqlite::Transaction<'_>,
    bundle: &SubmissionExportTaskBundle,
    path_prefix: &Path,
) -> Result<(), String> {
    let mut submission_task = bundle.submission_task.clone();
    let task_id = json_string_field_non_empty(&submission_task, "task_id")
        .unwrap_or_else(|| bundle.task_id.clone());
    set_json_field(
        &mut submission_task,
        "task_id",
        Value::String(task_id.clone()),
    );
    normalize_json_path_field_with_prefix(&mut submission_task, "cover_local_path", path_prefix);
    insert_row_object(tx, "submission_task", &submission_task, &[])?;

    let mut source_id_map: HashMap<String, String> = HashMap::new();
    for source in &bundle.task_source_videos {
        let old_source_id = json_string_field_non_empty(source, "id");
        let mut source_row = source.clone();
        let new_source_id = generate_unique_text_id(tx, "task_source_video", "id")?;
        set_json_field(&mut source_row, "id", Value::String(new_source_id.clone()));
        set_json_field(&mut source_row, "task_id", Value::String(task_id.clone()));
        normalize_json_path_field_with_prefix(&mut source_row, "source_file_path", path_prefix);
        insert_row_object(tx, "task_source_video", &source_row, &[])?;
        if let Some(old_id) = old_source_id {
            source_id_map.insert(old_id, new_source_id);
        }
    }

    for clip in &bundle.video_clips {
        let mut clip_row = clip.clone();
        set_json_field(&mut clip_row, "task_id", Value::String(task_id.clone()));
        normalize_json_path_field_with_prefix(&mut clip_row, "clip_path", path_prefix);
        insert_row_object(tx, "video_clip", &clip_row, &["id"])?;
    }

    let mut merged_id_map: HashMap<i64, i64> = HashMap::new();
    for merged in &bundle.merged_videos {
        let old_merged_id = json_i64_field(merged, "id");
        let mut merged_row = merged.clone();
        set_json_field(&mut merged_row, "task_id", Value::String(task_id.clone()));
        normalize_json_path_field_with_prefix(&mut merged_row, "video_path", path_prefix);
        insert_row_object(tx, "merged_video", &merged_row, &["id"])?;
        let new_merged_id = tx.last_insert_rowid();
        if let Some(old_id) = old_merged_id {
            merged_id_map.insert(old_id, new_merged_id);
        }
    }

    for segment in &bundle.task_output_segments {
        let mut segment_row = segment.clone();
        set_json_field(&mut segment_row, "task_id", Value::String(task_id.clone()));
        if let Some(old_merged_id) = json_i64_field(segment, "merged_id") {
            if let Some(new_merged_id) = merged_id_map.get(&old_merged_id) {
                set_json_field(
                    &mut segment_row,
                    "merged_id",
                    Value::Number(Number::from(*new_merged_id)),
                );
            } else {
                set_json_field(&mut segment_row, "merged_id", Value::Null);
            }
        }
        let old_segment_id = json_string_field_non_empty(segment, "segment_id");
        let mut next_segment_id =
            old_segment_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        while row_exists_text(tx, "task_output_segment", "segment_id", &next_segment_id)? {
            next_segment_id = uuid::Uuid::new_v4().to_string();
        }
        set_json_field(
            &mut segment_row,
            "segment_id",
            Value::String(next_segment_id),
        );
        normalize_json_path_field_with_prefix(&mut segment_row, "segment_file_path", path_prefix);
        insert_row_object(tx, "task_output_segment", &segment_row, &[])?;
    }

    for merged_source in &bundle.merged_source_videos {
        let mut merged_source_row = merged_source.clone();
        set_json_field(
            &mut merged_source_row,
            "task_id",
            Value::String(task_id.clone()),
        );
        if let Some(old_merged_id) = json_i64_field(merged_source, "merged_id") {
            let Some(new_merged_id) = merged_id_map.get(&old_merged_id).copied() else {
                return Err(format!("找不到 merged_id 映射: {}", old_merged_id));
            };
            set_json_field(
                &mut merged_source_row,
                "merged_id",
                Value::Number(Number::from(new_merged_id)),
            );
        }
        if let Some(old_source_id) = json_string_field_non_empty(merged_source, "source_id") {
            if let Some(new_source_id) = source_id_map.get(old_source_id.as_str()) {
                set_json_field(
                    &mut merged_source_row,
                    "source_id",
                    Value::String(new_source_id.clone()),
                );
            } else {
                set_json_field(&mut merged_source_row, "source_id", Value::Null);
            }
        }
        normalize_json_path_field_with_prefix(
            &mut merged_source_row,
            "source_file_path",
            path_prefix,
        );
        insert_row_object(tx, "merged_source_video", &merged_source_row, &["id"])?;
    }

    let mut instance_id_map: HashMap<String, String> = HashMap::new();
    for instance in &bundle.workflow_instances {
        let old_instance_id = json_string_field_non_empty(instance, "instance_id")
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let mut new_instance_id = old_instance_id.clone();
        while row_exists_text(tx, "workflow_instances", "instance_id", &new_instance_id)? {
            new_instance_id = uuid::Uuid::new_v4().to_string();
        }
        let mut instance_row = instance.clone();
        set_json_field(
            &mut instance_row,
            "instance_id",
            Value::String(new_instance_id.clone()),
        );
        set_json_field(&mut instance_row, "task_id", Value::String(task_id.clone()));
        insert_row_object(tx, "workflow_instances", &instance_row, &[])?;
        instance_id_map.insert(old_instance_id, new_instance_id);
    }

    let mut step_id_map: HashMap<String, String> = HashMap::new();
    for step in &bundle.workflow_steps {
        let old_instance_id = json_string_field_non_empty(step, "instance_id")
            .ok_or_else(|| "workflow_steps.instance_id 缺失".to_string())?;
        let mapped_instance_id = instance_id_map
            .get(&old_instance_id)
            .cloned()
            .unwrap_or(old_instance_id);
        let old_step_id = json_string_field_non_empty(step, "step_id")
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let mut new_step_id = old_step_id.clone();
        while row_exists_text(tx, "workflow_steps", "step_id", &new_step_id)? {
            new_step_id = uuid::Uuid::new_v4().to_string();
        }
        let mut step_row = step.clone();
        set_json_field(
            &mut step_row,
            "instance_id",
            Value::String(mapped_instance_id),
        );
        set_json_field(&mut step_row, "step_id", Value::String(new_step_id.clone()));
        insert_row_object(tx, "workflow_steps", &step_row, &[])?;
        step_id_map.insert(old_step_id, new_step_id);
    }

    for log in &bundle.workflow_execution_logs {
        let old_instance_id = json_string_field_non_empty(log, "instance_id")
            .ok_or_else(|| "workflow_execution_logs.instance_id 缺失".to_string())?;
        let mapped_instance_id = instance_id_map
            .get(&old_instance_id)
            .cloned()
            .unwrap_or(old_instance_id);
        let mut log_row = log.clone();
        set_json_field(
            &mut log_row,
            "instance_id",
            Value::String(mapped_instance_id),
        );
        if let Some(old_step_id) = json_string_field_non_empty(log, "step_id") {
            if let Some(new_step_id) = step_id_map.get(&old_step_id) {
                set_json_field(&mut log_row, "step_id", Value::String(new_step_id.clone()));
            } else {
                set_json_field(&mut log_row, "step_id", Value::Null);
            }
        }
        insert_row_object(tx, "workflow_execution_logs", &log_row, &["log_id"])?;
    }

    for metric in &bundle.workflow_performance_metrics {
        let old_instance_id = json_string_field_non_empty(metric, "instance_id")
            .ok_or_else(|| "workflow_performance_metrics.instance_id 缺失".to_string())?;
        let mapped_instance_id = instance_id_map
            .get(&old_instance_id)
            .cloned()
            .unwrap_or(old_instance_id);
        let mut metric_row = metric.clone();
        set_json_field(
            &mut metric_row,
            "instance_id",
            Value::String(mapped_instance_id),
        );
        if let Some(old_step_id) = json_string_field_non_empty(metric, "step_id") {
            if let Some(new_step_id) = step_id_map.get(&old_step_id) {
                set_json_field(
                    &mut metric_row,
                    "step_id",
                    Value::String(new_step_id.clone()),
                );
            } else {
                set_json_field(&mut metric_row, "step_id", Value::Null);
            }
        }
        insert_row_object(
            tx,
            "workflow_performance_metrics",
            &metric_row,
            &["metric_id"],
        )?;
    }

    let mut download_id_map: HashMap<i64, i64> = HashMap::new();
    for download in &bundle.video_downloads {
        let old_download_id = json_i64_field(download, "id");
        let mut download_row = download.clone();
        normalize_json_path_field_with_prefix(&mut download_row, "local_path", path_prefix);
        insert_row_object(tx, "video_download", &download_row, &["id"])?;
        if let Some(old_id) = old_download_id {
            let new_id = tx.last_insert_rowid();
            download_id_map.insert(old_id, new_id);
        }
    }

    for relation in &bundle.task_relations {
        let mut relation_row = relation.clone();
        set_json_field(
            &mut relation_row,
            "submission_task_id",
            Value::String(task_id.clone()),
        );
        if let Some(old_download_id) = json_i64_field(relation, "download_task_id") {
            let Some(new_download_id) = download_id_map.get(&old_download_id).copied() else {
                continue;
            };
            set_json_field(
                &mut relation_row,
                "download_task_id",
                Value::Number(Number::from(new_download_id)),
            );
        } else {
            continue;
        }
        if let Some(old_instance_id) = json_string_field_non_empty(relation, "workflow_instance_id")
        {
            if let Some(new_instance_id) = instance_id_map.get(&old_instance_id) {
                set_json_field(
                    &mut relation_row,
                    "workflow_instance_id",
                    Value::String(new_instance_id.clone()),
                );
            } else {
                set_json_field(&mut relation_row, "workflow_instance_id", Value::Null);
            }
        }
        insert_row_object(tx, "task_relations", &relation_row, &["id"])?;
    }

    for sync_task in &bundle.baidu_sync_tasks {
        let mut sync_task_row = sync_task.clone();
        if let Some(source_type) = json_string_field_non_empty(sync_task, "source_type") {
            if source_type == "submission_merged" {
                set_json_field(
                    &mut sync_task_row,
                    "source_id",
                    Value::String(task_id.clone()),
                );
            }
        }
        normalize_json_path_field_with_prefix(&mut sync_task_row, "local_path", path_prefix);
        insert_row_object(tx, "baidu_sync_task", &sync_task_row, &["id"])?;
    }

    Ok(())
}

#[tauri::command(async)]
pub fn submission_export(
    state: State<'_, AppState>,
    request: SubmissionExportRequest,
) -> ApiResponse<SubmissionExportResult> {
    let context = SubmissionContext::new(&state);
    let save_path = request.save_path.trim().to_string();
    if save_path.is_empty() {
        return ApiResponse::error("导出路径不能为空");
    }
    let export_all = request.export_all.unwrap_or(false);
    let selected_ids = request
        .task_ids
        .unwrap_or_default()
        .into_iter()
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect::<Vec<_>>();

    let bundle_tasks = context.db.with_conn(|conn| {
        let task_ids = if export_all {
            let mut stmt =
                conn.prepare("SELECT task_id FROM submission_task ORDER BY created_at DESC")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        } else {
            let mut unique = Vec::new();
            let mut seen = HashSet::new();
            for task_id in &selected_ids {
                if seen.insert(task_id.clone()) {
                    unique.push(task_id.clone());
                }
            }
            unique
        };
        let mut tasks = Vec::new();
        for task_id in task_ids {
            if let Some(task_bundle) = collect_submission_export_task_bundle(
                conn,
                &task_id,
                context.local_path_prefix.as_path(),
            )? {
                tasks.push(task_bundle);
            }
        }
        Ok(tasks)
    });

    let tasks = match bundle_tasks {
        Ok(items) => items,
        Err(err) => return ApiResponse::error(format!("导出失败: {}", err)),
    };
    if tasks.is_empty() {
        return ApiResponse::error("没有可导出的投稿任务");
    }

    let exported_at = now_rfc3339();
    let export_bundle = SubmissionExportBundle {
        magic: SUBMISSION_EXPORT_MAGIC.to_string(),
        version: SUBMISSION_EXPORT_VERSION,
        exported_at: exported_at.clone(),
        task_count: tasks.len(),
        tasks,
    };
    let content = match serde_json::to_string_pretty(&export_bundle) {
        Ok(value) => value,
        Err(err) => return ApiResponse::error(format!("序列化导出内容失败: {}", err)),
    };
    let target_path = PathBuf::from(&save_path);
    if let Some(parent) = target_path.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(err) = fs::create_dir_all(parent) {
                return ApiResponse::error(format!("创建导出目录失败: {}", err));
            }
        }
    }
    if let Err(err) = fs::write(&target_path, content) {
        return ApiResponse::error(format!("写入导出文件失败: {}", err));
    }
    ApiResponse::success(SubmissionExportResult {
        file_path: target_path.to_string_lossy().to_string(),
        task_count: export_bundle.task_count,
        exported_at,
    })
}

#[tauri::command(async)]
pub fn submission_import(
    state: State<'_, AppState>,
    request: SubmissionImportRequest,
) -> ApiResponse<SubmissionImportResult> {
    let context = SubmissionContext::new(&state);
    let file_path = request.file_path.trim().to_string();
    if file_path.is_empty() {
        return ApiResponse::error("导入文件路径不能为空");
    }
    let content = match fs::read_to_string(&file_path) {
        Ok(value) => value,
        Err(err) => return ApiResponse::error(format!("读取导入文件失败: {}", err)),
    };
    let export_bundle: SubmissionExportBundle = match serde_json::from_str(&content) {
        Ok(value) => value,
        Err(err) => return ApiResponse::error(format!("导入文件格式错误: {}", err)),
    };
    if export_bundle.magic != SUBMISSION_EXPORT_MAGIC {
        return ApiResponse::error("导入文件不是当前系统导出格式");
    }
    if export_bundle.version != SUBMISSION_EXPORT_VERSION {
        return ApiResponse::error("导入文件版本不兼容");
    }

    let bundle_tasks = export_bundle.tasks;
    let import_result = context.db.with_conn_mut(|conn| {
        let mut result = SubmissionImportResult {
            total_tasks: bundle_tasks.len(),
            imported_tasks: 0,
            skipped_tasks: 0,
            failed_tasks: 0,
            skipped_reasons: Vec::new(),
            failed_reasons: Vec::new(),
        };

        for bundle in &bundle_tasks {
            let task_id = json_string_field_non_empty(&bundle.submission_task, "task_id")
                .unwrap_or_else(|| bundle.task_id.clone());
            let title = json_string_field_non_empty(&bundle.submission_task, "title")
                .unwrap_or_else(|| task_id.clone());
            let bvid = json_string_field_non_empty(&bundle.submission_task, "bvid");

            let exists = if let Some(bvid_value) = bvid.as_deref() {
                conn.query_row(
                    "SELECT 1 FROM submission_task WHERE bvid = ?1 LIMIT 1",
                    [bvid_value],
                    |_| Ok(1),
                )
                .optional()?
                .is_some()
            } else {
                conn.query_row(
                    "SELECT 1 FROM submission_task WHERE title = ?1 LIMIT 1",
                    [title.as_str()],
                    |_| Ok(1),
                )
                .optional()?
                .is_some()
            };
            let exists_by_task_id = conn
                .query_row(
                    "SELECT 1 FROM submission_task WHERE task_id = ?1 LIMIT 1",
                    [task_id.as_str()],
                    |_| Ok(1),
                )
                .optional()?
                .is_some();

            if exists || exists_by_task_id {
                result.skipped_tasks += 1;
                result
                    .skipped_reasons
                    .push(format!("{}：已存在，跳过", title));
                continue;
            }

            let tx = conn.transaction()?;
            match import_submission_task_bundle(&tx, bundle, context.local_path_prefix.as_path()) {
                Ok(()) => {
                    tx.commit()?;
                    result.imported_tasks += 1;
                }
                Err(err) => {
                    let _ = tx.rollback();
                    result.failed_tasks += 1;
                    result.failed_reasons.push(format!("{}：{}", title, err));
                }
            }
        }

        Ok(result)
    });

    match import_result {
        Ok(value) => ApiResponse::success(value),
        Err(err) => ApiResponse::error(format!("导入失败: {}", err)),
    }
}
