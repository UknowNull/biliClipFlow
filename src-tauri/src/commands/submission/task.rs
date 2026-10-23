//! 投稿任务的创建、更新、查询、删除与启动命令。

use super::*;

#[tauri::command]
pub async fn submission_create(
    state: State<'_, AppState>,
    request: SubmissionCreateRequest,
) -> Result<ApiResponse<TaskCreationResult>, String> {
    let context = SubmissionContext::new(&state);
    if let Err(err) = validate_source_video_inputs(&request.source_videos) {
        return Ok(ApiResponse::error(err));
    }
    let bilibili_uid = match resolve_requested_bilibili_uid(&state, request.task.bilibili_uid) {
        Ok(value) => value,
        Err(err) => return Ok(ApiResponse::error(err)),
    };
    let task_baidu_uid = if request.task.baidu_sync_enabled.unwrap_or(false) {
        match resolve_bound_baidu_uid(context.db.as_ref(), bilibili_uid) {
            Ok(Some(value)) => Some(value),
            Ok(None) => return Ok(ApiResponse::error("当前B站账号未绑定网盘账号")),
            Err(err) => return Ok(ApiResponse::error(err)),
        }
    } else {
        None
    };
    let task_id = uuid::Uuid::new_v4().to_string();
    let now = now_rfc3339();
    let immediate_submit = request.task.immediate_submit.unwrap_or(true);
    let initial_status = if immediate_submit {
        "PENDING"
    } else {
        "PENDING_SUBMIT"
    };
    let normalized_collection_id = request.task.collection_id.filter(|value| *value > 0);
    let normalized_collection_section_id = normalized_collection_id.and_then(|_| {
        request
            .task
            .collection_section_id
            .filter(|value| *value > 0)
    });
    let normalized_cover_url = normalize_optional_text(request.task.cover_url.clone());
    let normalized_cover_data_url = normalize_optional_text(request.task.cover_data_url.clone());
    let cover_local_path = match normalized_cover_data_url.as_deref() {
        Some(data_url) => match save_task_cover_image_from_data_url(&context, &task_id, data_url) {
            Ok(path) => Some(path),
            Err(err) => return Ok(ApiResponse::error(err)),
        },
        None => None,
    };
    append_log(
        &state.app_log_path,
        &format!("submission_create_start task_id={}", task_id),
    );
    let stored_source_paths = request
        .source_videos
        .iter()
        .map(|source| {
            let source_path = if normalize_source_type(source) == "BAIDU"
                && source.source_file_path.trim().is_empty()
            {
                resolve_baidu_source_path_for_task(
                    &context,
                    &task_id,
                    source.remote_path.as_deref().unwrap_or(""),
                )
                .to_string_lossy()
                .to_string()
            } else {
                source.source_file_path.clone()
            };
            to_stored_local_path_with_prefix(context.local_path_prefix.as_path(), &source_path)
        })
        .collect::<Vec<_>>();
    append_log(
        &state.app_log_path,
        &format!(
            "submission_create_prepared task_id={} sources={}",
            task_id,
            stored_source_paths.len()
        ),
    );

    let result = context.db.with_conn(|conn| {
    append_log(
      &state.app_log_path,
      &format!("submission_create_db_lock_acquired task_id={}", task_id),
    );
    let normalized_baidu_sync_filename =
      normalize_baidu_sync_filename(request.task.baidu_sync_filename.as_deref());
    let source_type = request
      .task
      .source_type
      .clone()
      .unwrap_or_else(|| "NORMAL".to_string())
      .trim()
      .to_ascii_uppercase();
    let priority_enabled = request.task.priority.unwrap_or(false);
    let priority_at = if priority_enabled {
      Some(now.as_str())
    } else {
      None
    };
    conn.execute(
      "INSERT INTO submission_task (task_id, status, priority, priority_at, title, description, cover_url, cover_local_path, partition_id, tags, topic_id, mission_id, activity_title, video_type, collection_id, collection_section_id, bvid, aid, created_at, updated_at, bilibili_uid, baidu_uid, segment_prefix, baidu_sync_enabled, baidu_sync_path, baidu_sync_filename, source_type) \
       VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, NULL, NULL, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)",
      params![
        &task_id,
        initial_status,
        if priority_enabled { 1 } else { 0 },
        priority_at,
        &request.task.title,
        request.task.description.as_deref(),
        normalized_cover_url.as_deref(),
        cover_local_path.as_deref(),
        request.task.partition_id,
        request.task.tags.as_deref(),
        request.task.topic_id,
        request.task.mission_id,
        request.task.activity_title.as_deref(),
        &request.task.video_type,
        normalized_collection_id,
        normalized_collection_section_id,
        &now,
        &now,
        bilibili_uid,
        task_baidu_uid.as_deref(),
        request.task.segment_prefix.as_deref(),
        if request.task.baidu_sync_enabled.unwrap_or(false) {
          1
        } else {
          0
        },
        request.task.baidu_sync_path.as_deref(),
        normalized_baidu_sync_filename.as_deref(),
        source_type.as_str(),
      ],
    )?;
    append_log(
      &state.app_log_path,
      &format!("submission_create_task_inserted task_id={}", task_id),
    );

    for (source, stored_source_path) in request.source_videos.iter().zip(stored_source_paths.iter()) {
      let source_id = uuid::Uuid::new_v4().to_string();
      let (remote_bvid, remote_aid, remote_cid, remote_part_title, remote_video_url) =
        normalize_source_remote_binding(source);
      let source_type = normalize_source_type(source);
      let remote_path = normalize_optional_text(source.remote_path.clone());
      let (upload_part_title, upload_part_title_mode) = normalize_upload_part_title(source);
      conn.execute(
        "INSERT INTO task_source_video (id, task_id, source_file_path, source_type, remote_path, sort_order, start_time, end_time, remote_video_url, remote_bvid, remote_aid, remote_cid, remote_part_title, upload_part_title, upload_part_title_mode) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        (
          source_id,
          &task_id,
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
    append_log(
      &state.app_log_path,
      &format!(
        "submission_create_sources_inserted task_id={} count={}",
        task_id,
        request.source_videos.len()
      ),
    );

    Ok(())
  });

    if let Err(err) = result {
        return Ok(ApiResponse::error(format!(
            "Failed to create task: {}",
            err
        )));
    }
    append_log(
        &state.app_log_path,
        &format!("submission_create_ok task_id={}", task_id),
    );

    let mut result = TaskCreationResult {
        task_id: task_id.clone(),
        workflow_instance_id: None,
        workflow_status: None,
        workflow_error: None,
    };

    if let Some(config) = request.workflow_config {
        match create_workflow_instance(&context, &task_id, &config) {
            Ok((instance_id, status)) => {
                result.workflow_instance_id = Some(instance_id);
                result.workflow_status = Some(status);
            }
            Err(err) => {
                result.workflow_error = Some(err);
            }
        }
    }

    if result.workflow_instance_id.is_some() {
        let workflow_instance_id = result.workflow_instance_id.clone().unwrap_or_default();
        let source_paths = load_source_videos(&context, &task_id).unwrap_or_default();
        let source_paths = source_paths
            .into_iter()
            .map(|source| source.input_path)
            .collect::<Vec<_>>();
        let created_download_ids = match prepare_integrated_source_downloads(
            &context,
            &task_id,
            &workflow_instance_id,
            &source_paths,
        ) {
            Ok(value) => value,
            Err(err) => {
                result.workflow_error = Some(err);
                Vec::new()
            }
        };
        if !created_download_ids.is_empty() {
            if immediate_submit {
                if let Err(err) = crate::commands::download::requeue_integrated_downloads(
                    &state,
                    &created_download_ids,
                )
                .await
                {
                    result.workflow_error = Some(format!("创建远程源视频下载任务失败: {}", err));
                }
            } else if let Err(err) = defer_integrated_downloads(&context, &created_download_ids) {
                result.workflow_error = Some(format!("暂存远程源视频下载任务失败: {}", err));
            }
            return Ok(ApiResponse::success(result));
        }
        if !immediate_submit {
            return Ok(ApiResponse::success(result));
        }
        let context_clone = context.clone();
        let task_id_clone = task_id.clone();
        tauri::async_runtime::spawn(async move {
            run_submission_workflow_guarded(context_clone, task_id_clone).await;
        });
    }

    Ok(ApiResponse::success(result))
}

#[tauri::command]
pub async fn submission_update(
    state: State<'_, AppState>,
    request: SubmissionUpdateRequest,
) -> Result<ApiResponse<String>, String> {
    let context = SubmissionContext::new(&state);
    let task_id = request.task_id.trim().to_string();
    if task_id.is_empty() {
        return Ok(ApiResponse::error("任务ID不能为空"));
    }
    let detail = match load_task_detail(&context, &task_id) {
        Ok(detail) => detail,
        Err(err) => return Ok(ApiResponse::error(err)),
    };
    if let Err(err) = ensure_local_editable_detail(&detail) {
        return Ok(ApiResponse::error(err));
    }
    if request.source_videos.is_empty() {
        return Ok(ApiResponse::error("请至少添加一个源视频"));
    }
    if let Err(err) = validate_source_video_inputs(&request.source_videos) {
        return Ok(ApiResponse::error(err));
    }
    let workflow_config = match request.workflow_config {
        Some(config) => config,
        None => return Ok(ApiResponse::error("工作流配置不能为空")),
    };
    let workflow_config = attach_update_sources(workflow_config, &request.source_videos);
    append_log(
        &state.app_log_path,
        &format!("submission_update_start task_id={}", task_id),
    );
    if let Err(err) = append_source_videos(&context, &task_id, &request.source_videos) {
        return Ok(ApiResponse::error(format!("追加源视频失败: {}", err)));
    }
    if let Err(err) = update_baidu_sync_config(
        &context,
        &task_id,
        request.baidu_sync_enabled,
        normalize_optional_text(request.baidu_sync_path),
        normalize_optional_text(request.baidu_sync_filename),
    ) {
        return Ok(ApiResponse::error(format!("更新百度同步配置失败: {}", err)));
    }
    if let Err(err) = reset_workflow_instances(&context, &task_id) {
        return Ok(ApiResponse::error(format!("重置工作流失败: {}", err)));
    }
    if let Err(err) = update_submission_status(&context, &task_id, "PENDING") {
        return Ok(ApiResponse::error(format!("更新任务状态失败: {}", err)));
    }
    let (workflow_instance_id, _) = match create_workflow_instance_for_task_with_type(
        context.db.as_ref(),
        &task_id,
        &workflow_config,
        "VIDEO_UPDATE",
    ) {
        Ok(value) => value,
        Err(err) => return Ok(ApiResponse::error(err)),
    };
    let new_source_paths = request
        .source_videos
        .iter()
        .map(|source| {
            let source_type = normalize_source_type(source);
            if source_type == "BAIDU" && source.source_file_path.trim().is_empty() {
                resolve_baidu_source_path_for_task(
                    &context,
                    &task_id,
                    source.remote_path.as_deref().unwrap_or(""),
                )
                .to_string_lossy()
                .to_string()
            } else {
                source.source_file_path.trim().to_string()
            }
        })
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    let created_download_ids = match prepare_integrated_source_downloads(
        &context,
        &task_id,
        &workflow_instance_id,
        &new_source_paths,
    ) {
        Ok(value) => value,
        Err(err) => return Ok(ApiResponse::error(err)),
    };
    if let Err(err) =
        crate::commands::download::requeue_integrated_downloads(&state, &created_download_ids).await
    {
        return Ok(ApiResponse::error(format!(
            "创建远程源视频下载任务失败: {}",
            err
        )));
    }
    if !created_download_ids.is_empty() {
        append_log(
            &state.app_log_path,
            &format!(
                "submission_update_wait_integrated_downloads task_id={} count={}",
                task_id,
                created_download_ids.len()
            ),
        );
        return Ok(ApiResponse::success(format!(
            "更新任务已创建，{} 个远程源视频已进入下载队列，下载完成后自动继续",
            created_download_ids.len()
        )));
    }
    start_submission_workflow(
        context.db.clone(),
        context.app_log_path.clone(),
        context.edit_upload_state.clone(),
        task_id,
    );
    Ok(ApiResponse::success("更新任务已启动".to_string()))
}

#[tauri::command]
#[allow(non_snake_case)]
pub async fn submission_list(
    state: State<'_, AppState>,
    page: Option<i64>,
    page_size: Option<i64>,
    pageSize: Option<i64>,
    query: Option<String>,
    refresh_remote: Option<bool>,
    source_type: Option<String>,
    sourceType: Option<String>,
    bilibili_uid: Option<i64>,
    bilibiliUid: Option<i64>,
) -> Result<ApiResponse<PaginatedSubmissionTasks>, String> {
    let context = SubmissionContext::new(&state);
    let page = page.unwrap_or(1).max(1);
    let page_size = page_size.or(pageSize).unwrap_or(20).max(1);
    let bilibili_uid =
        match resolve_submission_list_bilibili_uid(&state, &context, bilibili_uid.or(bilibiliUid))?
        {
            Some(value) => value,
            None => {
                return Ok(ApiResponse::success(PaginatedSubmissionTasks {
                    items: Vec::new(),
                    total: 0,
                    page,
                    page_size,
                }))
            }
        };
    if refresh_remote.unwrap_or(false) {
        let queue_context = build_submission_queue_context(&state);
        if let Err(err) = refresh_submission_remote_state(&queue_context).await {
            append_log(
                &state.app_log_path,
                &format!("submission_list_refresh_remote_fail err={}", err),
            );
        }
    }
    let response = match load_tasks(
        &context,
        None,
        page,
        page_size,
        query,
        bilibili_uid,
        source_type.or(sourceType),
    ) {
        Ok(result) => ApiResponse::success(result),
        Err(err) => ApiResponse::error(format!("Failed to load tasks: {}", err)),
    };
    Ok(response)
}

#[tauri::command]
#[allow(non_snake_case)]
pub async fn submission_list_by_status(
    state: State<'_, AppState>,
    status: String,
    page: Option<i64>,
    page_size: Option<i64>,
    pageSize: Option<i64>,
    query: Option<String>,
    refresh_remote: Option<bool>,
    source_type: Option<String>,
    sourceType: Option<String>,
    bilibili_uid: Option<i64>,
    bilibiliUid: Option<i64>,
) -> Result<ApiResponse<PaginatedSubmissionTasks>, String> {
    let context = SubmissionContext::new(&state);
    let page = page.unwrap_or(1).max(1);
    let page_size = page_size.or(pageSize).unwrap_or(20).max(1);
    let bilibili_uid =
        match resolve_submission_list_bilibili_uid(&state, &context, bilibili_uid.or(bilibiliUid))?
        {
            Some(value) => value,
            None => {
                return Ok(ApiResponse::success(PaginatedSubmissionTasks {
                    items: Vec::new(),
                    total: 0,
                    page,
                    page_size,
                }))
            }
        };
    if refresh_remote.unwrap_or(false) {
        let queue_context = build_submission_queue_context(&state);
        if let Err(err) = refresh_submission_remote_state(&queue_context).await {
            append_log(
                &state.app_log_path,
                &format!(
                    "submission_list_by_status_refresh_remote_fail status={} err={}",
                    status, err
                ),
            );
        }
    }
    let response = match load_tasks(
        &context,
        Some(status),
        page,
        page_size,
        query,
        bilibili_uid,
        source_type.or(sourceType),
    ) {
        Ok(result) => ApiResponse::success(result),
        Err(err) => ApiResponse::error(format!("Failed to load tasks: {}", err)),
    };
    Ok(response)
}

#[tauri::command(async)]
pub fn submission_task_dir(state: State<'_, AppState>, task_id: String) -> ApiResponse<String> {
    let trimmed = task_id.trim();
    if trimmed.is_empty() {
        return ApiResponse::error("任务ID不能为空");
    }
    let context = SubmissionContext::new(&state);
    let dir = match resolve_task_dir_for_open(&context, trimmed) {
        Ok(path) => path,
        Err(err) => return ApiResponse::error(err),
    };
    ApiResponse::success(dir.to_string_lossy().to_string())
}

#[tauri::command(async)]
pub fn submission_open_task_dir(
    state: State<'_, AppState>,
    task_id: String,
) -> ApiResponse<String> {
    let trimmed = task_id.trim();
    if trimmed.is_empty() {
        return ApiResponse::error("任务ID不能为空");
    }
    let context = SubmissionContext::new(&state);
    let dir = match resolve_task_dir_for_open(&context, trimmed) {
        Ok(path) => path,
        Err(err) => return ApiResponse::error(err),
    };

    let open_result: Result<(), String> = {
        #[cfg(target_os = "windows")]
        {
            let mut command = Command::new("explorer");
            apply_no_window(&mut command);
            command
                .arg(dir.as_os_str())
                .spawn()
                .map(|_| ())
                .map_err(|err| format!("打开任务目录失败: {}", err))
        }
        #[cfg(target_os = "macos")]
        {
            Command::new("open")
                .arg(dir.as_os_str())
                .spawn()
                .map(|_| ())
                .map_err(|err| format!("打开任务目录失败: {}", err))
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            Command::new("xdg-open")
                .arg(dir.as_os_str())
                .spawn()
                .map(|_| ())
                .map_err(|err| format!("打开任务目录失败: {}", err))
        }
    };

    match open_result {
        Ok(()) => ApiResponse::success(dir.to_string_lossy().to_string()),
        Err(err) => ApiResponse::error(err),
    }
}

#[tauri::command(async)]
pub fn submission_delete_preview(
    state: State<'_, AppState>,
    task_id: String,
) -> ApiResponse<SubmissionDeletePreview> {
    let trimmed = task_id.trim();
    if trimmed.is_empty() {
        return ApiResponse::error("任务ID不能为空");
    }
    let context = SubmissionContext::new(&state);
    let mut files: Vec<DeleteFilePreview> = Vec::new();
    let mut file_seen = HashSet::new();
    let base_dir = resolve_submission_base_dir(&context, trimmed);
    if path_exists(&base_dir) {
        let path = base_dir.to_string_lossy().to_string();
        if file_seen.insert(path.clone()) {
            files.push(DeleteFilePreview {
                path,
                conflicts: Vec::new(),
            });
        }
    }
    let source_paths = match load_source_video_paths(&context, trimmed) {
        Ok(list) => list,
        Err(err) => return ApiResponse::error(err),
    };
    for path in source_paths {
        if !path_exists(Path::new(&path)) {
            continue;
        }
        let conflicts = match find_active_references(&context, trimmed, &path) {
            Ok(conflicts) => conflicts,
            Err(err) => return ApiResponse::error(err),
        };
        if file_seen.insert(path.clone()) {
            files.push(DeleteFilePreview { path, conflicts });
        }
    }
    let cover_paths = match load_task_cover_local_paths(&context, trimmed) {
        Ok(list) => list,
        Err(err) => return ApiResponse::error(err),
    };
    for path in cover_paths {
        if !path_exists(Path::new(&path)) {
            continue;
        }
        let conflicts = match find_active_references(&context, trimmed, &path) {
            Ok(conflicts) => conflicts,
            Err(err) => return ApiResponse::error(err),
        };
        if file_seen.insert(path.clone()) {
            files.push(DeleteFilePreview { path, conflicts });
        }
    }
    ApiResponse::success(SubmissionDeletePreview {
        task_id: trimmed.to_string(),
        files,
    })
}

#[tauri::command]
pub async fn submission_detail(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<ApiResponse<SubmissionTaskDetail>, String> {
    let context = SubmissionContext::new(&state);
    let upload_context = UploadContext::new(&state);
    append_log(
        &state.app_log_path,
        &format!("submission_detail_request task_id={}", task_id),
    );
    match load_task_detail(&context, &task_id) {
        Ok(mut detail) => {
            hydrate_task_cover_for_display(&context, &upload_context, &mut detail.task).await;
            append_log(
                &state.app_log_path,
                &format!(
                    "submission_detail_ok task_id={} sources={} merged={} segments={} workflow={}",
                    task_id,
                    detail.source_videos.len(),
                    detail.merged_videos.len(),
                    detail.output_segments.len(),
                    if detail.workflow_config.is_some() {
                        1
                    } else {
                        0
                    }
                ),
            );
            Ok(ApiResponse::success(detail))
        }
        Err(err) => {
            append_log(
                &state.app_log_path,
                &format!("submission_detail_fail task_id={} err={}", task_id, err),
            );
            Ok(ApiResponse::error(format!(
                "Failed to load task detail: {}",
                err
            )))
        }
    }
}

#[tauri::command(async)]
pub fn submission_reorder_source_videos(
    state: State<'_, AppState>,
    request: SubmissionReorderSourceVideosRequest,
) -> ApiResponse<String> {
    let context = SubmissionContext::new(&state);
    let task_id = request.task_id.trim().to_string();
    if task_id.is_empty() {
        return ApiResponse::error("任务ID不能为空");
    }
    if let Err(err) = ensure_editable_status(&context, &task_id) {
        return ApiResponse::error(err);
    }
    let current_bilibili_uid = match require_current_bilibili_uid(&state) {
        Ok(value) => value,
        Err(err) => return ApiResponse::error(err),
    };
    let task_owner = context.db.with_conn(|conn| {
        conn.query_row(
            "SELECT bilibili_uid FROM submission_task WHERE task_id = ?1",
            [&task_id],
            |row| row.get::<_, Option<i64>>(0),
        )
        .optional()
    });
    let task_owner = match task_owner {
        Ok(Some(value)) => value,
        Ok(None) => return ApiResponse::error("投稿任务不存在"),
        Err(err) => return ApiResponse::error(format!("读取投稿任务失败: {}", err)),
    };
    if task_owner != Some(current_bilibili_uid) {
        return ApiResponse::error("当前账号无权修改该投稿任务");
    }

    let mut seen_ids = HashSet::new();
    let source_ids = request
        .source_ids
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .filter(|value| seen_ids.insert((*value).to_string()))
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    if source_ids.is_empty() {
        return ApiResponse::error("请至少保留一个源视频");
    }

    let existing_ids = context.db.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id FROM task_source_video WHERE task_id = ?1 ORDER BY sort_order ASC, id ASC",
        )?;
        let rows = stmt.query_map([&task_id], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    });
    let existing_ids = match existing_ids {
        Ok(value) => value,
        Err(err) => return ApiResponse::error(format!("读取源视频失败: {}", err)),
    };
    if existing_ids.len() != source_ids.len() {
        return ApiResponse::error("排序列表与源视频数量不一致");
    }
    let existing_set = existing_ids.iter().cloned().collect::<HashSet<_>>();
    let incoming_set = source_ids.iter().cloned().collect::<HashSet<_>>();
    if existing_set != incoming_set {
        return ApiResponse::error("排序列表存在不属于当前任务的源视频");
    }

    let now = now_rfc3339();
    let result = context.db.with_conn_mut(|conn| {
        let tx = conn.transaction()?;
        for (index, source_id) in source_ids.iter().enumerate() {
            tx.execute(
                "UPDATE task_source_video SET sort_order = ?1 WHERE task_id = ?2 AND id = ?3",
                ((index + 1) as i64, &task_id, source_id),
            )?;
        }
        tx.execute(
            "UPDATE submission_task SET updated_at = ?1 WHERE task_id = ?2",
            (&now, &task_id),
        )?;
        tx.commit()?;
        Ok(())
    });
    if let Err(err) = result {
        return ApiResponse::error(format!("更新源视频排序失败: {}", err));
    }
    ApiResponse::success("源视频排序已更新".to_string())
}

#[tauri::command(async)]
pub fn submission_delete(
    state: State<'_, AppState>,
    request: SubmissionDeleteRequest,
) -> ApiResponse<SubmissionDeleteResult> {
    let task_id = request.task_id.trim().to_string();
    if task_id.is_empty() {
        return ApiResponse::error("任务ID不能为空");
    }
    if !request.delete_task && !request.delete_files {
        return ApiResponse::error("至少选择删除任务或删除文件");
    }
    let context = SubmissionContext::new(&state);
    let base_dir = resolve_submission_base_dir(&context, &task_id);
    append_log(
        &state.app_log_path,
        &format!(
            "submission_delete_start task_id={} delete_task={} delete_files={} force_delete={}",
            task_id, request.delete_task, request.delete_files, request.force_delete
        ),
    );

    let delete_paths = if request.delete_files {
        normalize_delete_paths(&request.delete_paths)
    } else {
        Vec::new()
    };
    if request.delete_files && delete_paths.is_empty() && !request.delete_task {
        append_log(
            &state.app_log_path,
            &format!("submission_delete_skip_no_paths task_id={}", task_id),
        );
        return ApiResponse::success(SubmissionDeleteResult {
            blocked: false,
            conflicts: Vec::new(),
            deleted_paths: Vec::new(),
            missing_paths: Vec::new(),
        });
    }

    let mut conflict_files = Vec::new();
    if request.delete_files && !request.force_delete {
        for path in &delete_paths {
            if !path_exists(Path::new(path)) {
                continue;
            }
            if let Ok(conflicts) = find_active_references(&context, &task_id, path) {
                if !conflicts.is_empty() {
                    conflict_files.push(DeleteFilePreview {
                        path: path.to_string(),
                        conflicts,
                    });
                }
            }
        }
        if !conflict_files.is_empty() {
            append_log(
                &state.app_log_path,
                &format!(
                    "submission_delete_blocked task_id={} conflicts={}",
                    task_id,
                    conflict_files.len()
                ),
            );
            return ApiResponse::success(SubmissionDeleteResult {
                blocked: true,
                conflicts: conflict_files,
                deleted_paths: Vec::new(),
                missing_paths: Vec::new(),
            });
        }
    }

    let mut source_video_set = HashSet::new();
    if request.delete_files {
        match load_source_video_paths(&context, &task_id) {
            Ok(paths) => {
                source_video_set = paths.into_iter().collect();
            }
            Err(err) => {
                append_log(
                    &state.app_log_path,
                    &format!(
                        "submission_delete_source_paths_fail task_id={} err={}",
                        task_id, err
                    ),
                );
            }
        }
    }

    let mut deleted_paths = Vec::new();
    let mut missing_paths = Vec::new();
    if request.delete_task {
        let result = context.db.with_conn_mut(|conn| {
      let tx = conn.transaction()?;
      tx.execute(
        "DELETE FROM workflow_execution_logs WHERE instance_id IN (SELECT instance_id FROM workflow_instances WHERE task_id = ?1)",
        [&task_id],
      )?;
      tx.execute(
        "DELETE FROM workflow_performance_metrics WHERE instance_id IN (SELECT instance_id FROM workflow_instances WHERE task_id = ?1)",
        [&task_id],
      )?;
      tx.execute(
        "DELETE FROM workflow_steps WHERE instance_id IN (SELECT instance_id FROM workflow_instances WHERE task_id = ?1)",
        [&task_id],
      )?;
      tx.execute("DELETE FROM workflow_instances WHERE task_id = ?1", [&task_id])?;
      tx.execute("DELETE FROM task_relations WHERE submission_task_id = ?1", [&task_id])?;
      tx.execute(
        "DELETE FROM baidu_sync_task WHERE source_type = 'submission_merged' AND source_id = ?1",
        [&task_id],
      )?;
      tx.execute("DELETE FROM task_output_segment WHERE task_id = ?1", [&task_id])?;
      tx.execute("DELETE FROM merged_video WHERE task_id = ?1", [&task_id])?;
      tx.execute("DELETE FROM task_source_video WHERE task_id = ?1", [&task_id])?;
      tx.execute("DELETE FROM video_clip WHERE task_id = ?1", [&task_id])?;
      let deleted = tx.execute("DELETE FROM submission_task WHERE task_id = ?1", [&task_id])?;
      if deleted == 0 {
        return Err(rusqlite::Error::QueryReturnedNoRows);
      }
      tx.commit()?;
      Ok(())
    });
        if let Err(err) = result {
            append_log(
                &state.app_log_path,
                &format!("submission_delete_fail task_id={} err={}", task_id, err),
            );
            return ApiResponse::error(format!("Failed to delete: {}", err));
        }
        if let Err(err) = cleanup_submission_files(&state.app_log_path, &base_dir) {
            append_log(
                &state.app_log_path,
                &format!(
                    "submission_delete_cleanup_fail task_id={} err={}",
                    task_id, err
                ),
            );
            return ApiResponse::error(format!("任务已删除，但清理文件失败: {}", err));
        }
    }

    if request.delete_files {
        for path in &delete_paths {
            let target = Path::new(path);
            if !path_exists(target) {
                missing_paths.push(path.to_string());
                continue;
            }
            let is_file = match fs::metadata(target) {
                Ok(metadata) => metadata.is_file(),
                Err(err) => return ApiResponse::error(format!("读取路径失败: {}", err)),
            };
            if let Err(err) = remove_path_if_exists_with_retry(
                &state.app_log_path,
                "custom",
                target,
                5,
                Duration::from_millis(300),
            ) {
                return ApiResponse::error(err);
            }
            deleted_paths.push(path.to_string());
            if is_file && source_video_set.contains(path) {
                cleanup_empty_parent_dir(&state.app_log_path, target);
            }
        }
        if !request.delete_task {
            if let Err(err) = baidu_sync::fail_missing_local_submission_sync_tasks(
                context.db.as_ref(),
                &state.app_log_path,
                &task_id,
            ) {
                append_log(
                    &state.app_log_path,
                    &format!(
                        "submission_delete_sync_reconcile_fail task_id={} err={}",
                        task_id, err
                    ),
                );
            }
            let _ = clear_missing_task_cover_local_path(&context, &task_id);
        }
    }

    append_log(
        &state.app_log_path,
        &format!("submission_delete_ok task_id={}", task_id),
    );
    ApiResponse::success(SubmissionDeleteResult {
        blocked: false,
        conflicts: Vec::new(),
        deleted_paths,
        missing_paths,
    })
}

#[tauri::command]
pub async fn submission_execute(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<ApiResponse<String>, String> {
    append_log(
        &state.app_log_path,
        &format!("submission_execute_request task_id={}", task_id),
    );
    let context = SubmissionContext::new(&state);
    start_submission_workflow(
        context.db.clone(),
        context.app_log_path.clone(),
        context.edit_upload_state.clone(),
        task_id,
    );

    Ok(ApiResponse::success("Workflow started".to_string()))
}

#[tauri::command]
pub async fn submission_start(
    state: State<'_, AppState>,
    request: SubmissionTaskIdsRequest,
) -> Result<ApiResponse<String>, String> {
    let context = SubmissionContext::new(&state);
    let mut task_ids = request
        .task_ids
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    task_ids.sort();
    task_ids.dedup();
    if task_ids.is_empty() {
        return Ok(ApiResponse::error("请选择要投稿的任务"));
    }

    let mut tasks = Vec::new();
    let mut blocked_reconciliation_count = 0usize;
    for task_id in task_ids {
        let detail = load_task_detail(&context, &task_id)?;
        if detail.task.status != "PENDING_SUBMIT" && detail.task.status != "FAILED" {
            continue;
        }
        if should_block_submission_retry(&detail.task) {
            blocked_reconciliation_count = blocked_reconciliation_count.saturating_add(1);
            continue;
        }
        tasks.push(detail.task);
    }
    tasks.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    if tasks.is_empty() {
        if blocked_reconciliation_count > 0 {
            return Ok(ApiResponse::error(submission_retry_block_message()));
        }
        return Ok(ApiResponse::error("没有可开始投稿的任务"));
    }

    for task in tasks {
        let _ = context.db.with_conn(|conn| {
            conn.execute(
                "UPDATE video_download SET status = 0, update_time = ?1 \
                 WHERE status = 5 AND id IN ( \
                   SELECT download_task_id FROM task_relations \
                   WHERE submission_task_id = ?2 AND relation_type = 'INTEGRATED' \
                 )",
                (now_rfc3339(), task.task_id.as_str()),
            )?;
            Ok(())
        });
        let _ = update_submission_status(&context, &task.task_id, "PENDING");
        if let Some(stats) = load_integrated_download_stats(&context, &task.task_id)? {
            if stats.failed > 0 {
                let _ = mark_submission_failure(
                    &context,
                    &task.task_id,
                    None,
                    "存在下载失败的分P，请先重试下载",
                );
                continue;
            }
            if stats.completed < stats.total {
                let _ =
                    update_workflow_status(&context, &task.task_id, "VIDEO_DOWNLOADING", None, 0.0);
                let _ = update_integrated_relation_workflow_status(
                    &context,
                    &task.task_id,
                    "PENDING_DOWNLOAD",
                );
                append_log(
                    &context.app_log_path,
                    &format!(
                        "submission_start_wait_download task_id={} completed={} total={}",
                        task.task_id, stats.completed, stats.total
                    ),
                );
                continue;
            }
        }
        let workflow_config = load_latest_workflow_config(&context, &task.task_id)?;
        if workflow_config.is_some()
            && load_workflow_status(&context, &task.task_id)
                .ok()
                .flatten()
                .map(|status| status.status != "RUNNING")
                .unwrap_or(true)
        {
            start_submission_workflow(
                context.db.clone(),
                context.app_log_path.clone(),
                context.edit_upload_state.clone(),
                task.task_id.clone(),
            );
        }
    }

    Ok(ApiResponse::success("已加入投稿流程".to_string()))
}

#[tauri::command]
pub async fn submission_integrated_execute(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<ApiResponse<String>, String> {
    let context = SubmissionContext::new(&state);
    let task_id = task_id.trim().to_string();
    if task_id.is_empty() {
        return Ok(ApiResponse::error("任务ID不能为空"));
    }

    let status = match load_task_status(&context, &task_id) {
        Ok(status) => status,
        Err(err) => return Ok(ApiResponse::error(format!("读取任务状态失败: {}", err))),
    };
    if status != "FAILED" {
        return Ok(ApiResponse::error("当前任务状态不支持一键投稿"));
    }
    let detail = load_task_detail(&context, &task_id)?;
    if should_block_submission_retry(&detail.task) {
        return Ok(ApiResponse::error(submission_retry_block_message()));
    }

    let stats = match load_integrated_download_stats(&context, &task_id) {
        Ok(value) => value,
        Err(err) => return Ok(ApiResponse::error(format!("读取下载状态失败: {}", err))),
    };

    let stats = match stats {
        Some(value) => value,
        None => return Ok(ApiResponse::error("该任务未关联下载记录")),
    };

    if stats.failed > 0 {
        return Ok(ApiResponse::error("存在下载失败的分P，请先重试下载"));
    }
    if stats.completed != stats.total {
        return Ok(ApiResponse::error("仍有分P下载未完成"));
    }

    if let Ok(Some(workflow_status)) = load_workflow_status(&context, &task_id) {
        if workflow_status.status == "RUNNING" {
            return Ok(ApiResponse::error("工作流执行中"));
        }
    }

    start_submission_workflow(
        context.db.clone(),
        context.app_log_path.clone(),
        context.edit_upload_state.clone(),
        task_id,
    );
    Ok(ApiResponse::success("Workflow started".to_string()))
}
