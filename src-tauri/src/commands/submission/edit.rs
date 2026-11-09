//! 已投稿稿件的编辑流程。

use super::*;

#[tauri::command]
pub async fn submission_edit_prepare(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<ApiResponse<SubmissionTaskDetail>, String> {
    let context = SubmissionContext::new(&state);
    let upload_context = UploadContext::new(&state);
    let task_id = task_id.trim();
    if task_id.is_empty() {
        return Ok(ApiResponse::error("任务ID不能为空"));
    }
    let mut detail = match load_task_detail(&context, task_id) {
        Ok(detail) => detail,
        Err(err) => {
            return Ok(ApiResponse::error(format!(
                "Failed to load task detail: {}",
                err
            )))
        }
    };
    hydrate_task_cover_for_display(&context, &upload_context, &mut detail.task).await;
    if let Err(err) = ensure_edit_prepare_detail(&detail) {
        return Ok(ApiResponse::error(err));
    }
    if !detail.output_segments.is_empty() {
        return Ok(ApiResponse::success(detail));
    }
    let merged = match load_latest_merged_video(&context, task_id) {
        Ok(Some(merged)) => merged,
        Ok(None) => return Ok(ApiResponse::error("未找到合并视频")),
        Err(err) => return Ok(ApiResponse::error(err)),
    };
    let merged_path = merged.video_path.clone().unwrap_or_default();
    if merged_path.trim().is_empty() {
        return Ok(ApiResponse::error("合并视频路径为空"));
    }
    let part_name = build_part_title(detail.task.segment_prefix.as_deref(), 1);
    let has_upload = merged.upload_cid.unwrap_or(0) > 0
        && merged
            .upload_file_name
            .as_deref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false);
    let upload_status = if has_upload { "SUCCESS" } else { "PENDING" };
    let total_bytes = if merged.upload_total_bytes > 0 {
        merged.upload_total_bytes
    } else {
        fs::metadata(&merged_path)
            .map(|meta| meta.len() as i64)
            .unwrap_or(0)
    };
    let upload_progress = if has_upload {
        100.0
    } else {
        merged.upload_progress
    };
    let segment_id = uuid::Uuid::new_v4().to_string();
    detail.output_segments.push(TaskOutputSegmentRecord {
        segment_id,
        task_id: task_id.to_string(),
        merged_id: Some(merged.id),
        part_name,
        segment_file_path: merged_path,
        part_order: 1,
        upload_status: upload_status.to_string(),
        cid: merged.upload_cid,
        file_name: merged.upload_file_name.clone(),
        upload_progress,
        upload_uploaded_bytes: merged.upload_uploaded_bytes,
        upload_total_bytes: total_bytes,
        upload_session_id: None,
        upload_biz_id: 0,
        upload_endpoint: None,
        upload_auth: None,
        upload_uri: None,
        upload_chunk_size: 0,
        upload_last_part_index: 0,
    });
    Ok(ApiResponse::success(detail))
}

#[tauri::command(async)]
pub fn submission_edit_update_sources(
    state: State<'_, AppState>,
    request: SubmissionEditUpdateSourcesRequest,
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
    if let Err(err) = validate_source_video_inputs(&request.source_videos) {
        return Ok(ApiResponse::error(err));
    }
    if let Err(err) = replace_source_videos(&context, &task_id, &request.source_videos) {
        return Ok(ApiResponse::error(format!("更新源视频失败: {}", err)));
    }
    let now = now_rfc3339();
    if let Err(err) = context.db.with_conn(|conn| {
        conn.execute(
            "UPDATE submission_task SET updated_at = ?1 WHERE task_id = ?2",
            (&now, &task_id),
        )?;
        Ok(())
    }) {
        return Ok(ApiResponse::error(format!("更新任务时间失败: {}", err)));
    }
    Ok(ApiResponse::success("源视频更新成功".to_string()))
}

#[tauri::command]
pub async fn submission_edit_submit(
    state: State<'_, AppState>,
    request: SubmissionEditSubmitRequest,
) -> Result<ApiResponse<String>, String> {
    let context = SubmissionContext::new(&state);
    let sync_remote_update = request.sync_remote_update.unwrap_or(true);
    let task_id = request.task_id.trim().to_string();
    if task_id.is_empty() {
        return Ok(ApiResponse::error("任务ID不能为空"));
    }
    let detail = match load_task_detail(&context, &task_id) {
        Ok(detail) => detail,
        Err(err) => return Ok(ApiResponse::error(err)),
    };
    if sync_remote_update {
        if let Err(err) = ensure_editable_detail(&detail) {
            return Ok(ApiResponse::error(err));
        }
    } else if let Err(err) = ensure_local_editable_detail(&detail) {
        return Ok(ApiResponse::error(err));
    }
    let title = request.task.title.trim();
    if title.is_empty() {
        return Ok(ApiResponse::error("投稿标题不能为空"));
    }
    if request.task.partition_id <= 0 {
        return Ok(ApiResponse::error("请选择B站分区"));
    }
    if request.task.video_type.trim().is_empty() {
        return Ok(ApiResponse::error("请选择视频类型"));
    }
    if let Some(description) = request.task.description.as_deref() {
        if description.len() > 2000 {
            return Ok(ApiResponse::error("视频描述不能超过 2000 个字符"));
        }
    }
    let tags = request.task.tags.clone().unwrap_or_default();
    if tags.trim().is_empty() {
        return Ok(ApiResponse::error("请填写至少一个投稿标签"));
    }
    if request.segments.is_empty() {
        return Ok(ApiResponse::error("至少需要保留一个分P"));
    }
    let mut ordered_segments = request.segments.clone();
    ordered_segments.sort_by_key(|segment| segment.part_order);
    let mut seen = HashSet::new();
    for segment in &ordered_segments {
        let segment_id = segment.segment_id.trim();
        if segment_id.is_empty() {
            return Ok(ApiResponse::error("分P ID不能为空"));
        }
        if !seen.insert(segment_id.to_string()) {
            return Ok(ApiResponse::error("存在重复的分P"));
        }
        let part_name = segment.part_name.trim();
        if part_name.is_empty() {
            return Ok(ApiResponse::error("分P名称不能为空"));
        }
        if segment.segment_file_path.trim().is_empty() {
            return Ok(ApiResponse::error("分P文件路径不能为空"));
        }
    }
    let original_collection_id = detail.task.collection_id.unwrap_or(0);
    let original_collection_section_id = detail.task.collection_section_id.unwrap_or(0);
    let mut task = detail.task.clone();
    task.title = title.to_string();
    task.description = request.task.description.clone();
    let next_cover_url = normalize_optional_text(request.task.cover_url.clone());
    let next_cover_data_url = normalize_optional_text(request.task.cover_data_url.clone());
    task.cover_url = next_cover_url.clone();
    if let Some(data_url) = next_cover_data_url.as_deref() {
        let stored_path = match save_task_cover_image_from_data_url(&context, &task_id, data_url) {
            Ok(path) => path,
            Err(err) => return Ok(ApiResponse::error(err)),
        };
        task.cover_local_path = Some(stored_path);
    } else if next_cover_url.is_none() {
        if let Err(err) =
            remove_task_cover_image_for_record(&context, &task_id, task.cover_local_path.as_deref())
        {
            return Ok(ApiResponse::error(err));
        }
        task.cover_local_path = None;
    }
    task.partition_id = request.task.partition_id;
    task.collection_id = request.task.collection_id.filter(|value| *value > 0);
    task.collection_section_id = task.collection_id.and_then(|_| {
        request
            .task
            .collection_section_id
            .filter(|value| *value > 0)
    });
    task.tags = Some(tags.clone());
    task.video_type = request.task.video_type.clone();
    task.segment_prefix = request.task.segment_prefix.clone();
    if let Some(topic_id) = request.task.topic_id {
        task.topic_id = if topic_id > 0 { Some(topic_id) } else { None };
    }
    if let Some(mission_id) = request.task.mission_id {
        task.mission_id = if mission_id > 0 {
            Some(mission_id)
        } else {
            None
        };
    }
    if let Some(activity_title) = request.task.activity_title.clone() {
        let trimmed = activity_title.trim().to_string();
        task.activity_title = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        };
    }
    if sync_remote_update {
        if detail.task.bvid.as_deref().unwrap_or("").trim().is_empty() {
            return Ok(ApiResponse::error(
                "当前任务暂无BVID，请取消勾选同步远程后仅保存本地数据",
            ));
        }
        let mut parts = Vec::new();
        for segment in &ordered_segments {
            let cid = match segment.cid {
                Some(cid) if cid > 0 => cid,
                _ => return Ok(ApiResponse::error("分P上传信息缺失，请重新上传")),
            };
            let filename = match segment
                .file_name
                .as_deref()
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
            {
                Some(value) => value.to_string(),
                None => return Ok(ApiResponse::error("分P上传信息缺失，请重新上传")),
            };
            parts.push(UploadedVideoPart {
                filename,
                cid,
                title: segment.part_name.trim().to_string(),
            });
        }
        let upload_context = UploadContext::new(&state);
        let mut auth = match load_auth_or_refresh_for_task(
            &upload_context,
            &task_id,
            "submission_edit_prepare",
        )
        .await
        {
            Ok(auth) => auth,
            Err(err) => return Ok(ApiResponse::error(err)),
        };
        let csrf = match auth.csrf.clone() {
            Some(value) => value,
            None => {
                auth = match refresh_auth(&upload_context, "submission_edit_prepare_csrf").await {
                    Ok(auth) => auth,
                    Err(err) => return Ok(ApiResponse::error(err)),
                };
                match auth.csrf.clone() {
                    Some(value) => value,
                    None => return Ok(ApiResponse::error("登录信息缺少CSRF")),
                }
            }
        };
        let mut aid = detail.task.aid.unwrap_or(0);
        if aid <= 0 {
            let bvid = detail.task.bvid.clone().unwrap_or_default();
            aid = fetch_aid_with_refresh(&upload_context, &auth, &bvid)
                .await
                .unwrap_or(0);
            if aid <= 0 {
                return Ok(ApiResponse::error("无法获取AID，无法编辑"));
            }
            let _ = update_submission_aid(&context, &task_id, aid);
        }
        task.aid = Some(aid);
        if let Err(err) =
            submit_video_edit_with_refresh(&upload_context, &auth, &task, &parts, aid, &csrf, true)
                .await
        {
            return Ok(ApiResponse::error(err));
        }
        let next_collection_id = task.collection_id.unwrap_or(0);
        let next_collection_section_id = task.collection_section_id.unwrap_or(0);
        if next_collection_id != original_collection_id
            || next_collection_section_id != original_collection_section_id
        {
            append_log(
                &upload_context.app_log_path,
                &format!(
                    "submission_edit_collection_change task_id={} from={}/{} to={}/{}",
                    task_id,
                    original_collection_id,
                    original_collection_section_id,
                    next_collection_id,
                    next_collection_section_id
                ),
            );
            if next_collection_id > 0 {
                if let Err(err) = switch_video_collection_with_refresh(
                    &upload_context,
                    &auth,
                    &task.title,
                    next_collection_id,
                    task.collection_section_id,
                    aid,
                    &csrf,
                )
                .await
                {
                    if is_collection_not_found_error(&err) {
                        append_log(
                            &upload_context.app_log_path,
                            &format!(
                "submission_edit_collection_switch_skip task_id={} collection_id={} err={}",
                task_id, next_collection_id, err
              ),
                        );
                    } else {
                        append_log(
                            &upload_context.app_log_path,
                            &format!(
                "submission_edit_collection_switch_fail task_id={} collection_id={} err={}",
                task_id, next_collection_id, err
              ),
                        );
                        return Ok(ApiResponse::error(err));
                    }
                }
            } else {
                append_log(
                    &upload_context.app_log_path,
                    &format!(
                        "submission_edit_collection_switch_skip task_id={} collection_id=0",
                        task_id
                    ),
                );
            }
        } else {
            append_log(
                &upload_context.app_log_path,
                &format!(
                    "submission_edit_collection_skip task_id={} from={}/{} to={}/{}",
                    task_id,
                    original_collection_id,
                    original_collection_section_id,
                    next_collection_id,
                    next_collection_section_id
                ),
            );
        }
    } else {
        append_log(
            &context.app_log_path,
            &format!(
                "submission_edit_remote_skip task_id={} mode=local_only",
                task_id
            ),
        );
    }
    if let Err(err) = update_submission_task_for_edit(&context, &task_id, &task) {
        return Ok(ApiResponse::error(err));
    }
    if let Err(err) =
        update_output_segments_for_edit(&context, &task_id, &ordered_segments, sync_remote_update)
    {
        return Ok(ApiResponse::error(err));
    }
    if let Err(err) = clear_edit_upload_segments_by_task(&context, &task_id) {
        append_log(
            &context.app_log_path,
            &format!(
                "submission_edit_clear_cache_fail task_id={} err={}",
                task_id, err
            ),
        );
    }
    Ok(ApiResponse::success("编辑投稿成功".to_string()))
}

pub(super) fn build_edit_payload(
    task: &SubmissionTaskRecord,
    parts: &[UploadedVideoPart],
    aid: i64,
    include_parent_collection: bool,
) -> Value {
    let copyright = if task.video_type == "ORIGINAL" { 1 } else { 2 };
    let human_type2 = task.partition_id;
    let tags = task.tags.clone().unwrap_or_default();
    let desc = task.description.clone().unwrap_or_default();
    let cover = task.cover_url.clone().unwrap_or_default();
    let videos = build_submission_videos(parts);

    let mut payload = serde_json::json!({
      "aid": aid,
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
      "web_os": 1
    });

    if include_parent_collection {
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

pub(super) fn update_submission_task_for_edit(
    context: &SubmissionContext,
    task_id: &str,
    task: &SubmissionTaskRecord,
) -> Result<(), String> {
    let now = now_rfc3339();
    context
    .db
    .with_conn(|conn| {
      conn.execute(
        "UPDATE submission_task SET title = ?1, description = ?2, cover_url = ?3, cover_local_path = ?4, partition_id = ?5, tags = ?6, topic_id = ?7, mission_id = ?8, activity_title = ?9, video_type = ?10, collection_id = ?11, collection_section_id = ?12, segment_prefix = ?13, updated_at = ?14 WHERE task_id = ?15",
        (
          &task.title,
          task.description.as_deref(),
          task.cover_url.as_deref(),
          task.cover_local_path.as_deref(),
          task.partition_id,
          task.tags.as_deref(),
          task.topic_id,
          task.mission_id,
          task.activity_title.as_deref(),
          &task.video_type,
          task.collection_id,
          task.collection_section_id,
          task.segment_prefix.as_deref(),
          &now,
          task_id,
        ),
      )?;
      Ok(())
    })
    .map_err(|err| err.to_string())
}

pub(super) fn ensure_editable_status(context: &SubmissionContext, task_id: &str) -> Result<(), String> {
    let status = load_task_status(context, task_id)?;
    if status == "UPLOADING" {
        return Err("任务正在投稿中，请稍后再试".to_string());
    }
    if status != "COMPLETED" {
        return Err("任务未完成，无法编辑".to_string());
    }
    Ok(())
}

pub(super) fn ensure_editable_detail(detail: &SubmissionTaskDetail) -> Result<(), String> {
    if detail.task.bvid.as_deref().unwrap_or("").is_empty() {
        return Err("当前任务暂无BVID，无法同步远程，请取消勾选后仅保存本地数据".to_string());
    }
    Ok(())
}

pub(super) fn ensure_local_editable_detail(detail: &SubmissionTaskDetail) -> Result<(), String> {
    Ok(())
}

pub(super) fn ensure_edit_prepare_detail(detail: &SubmissionTaskDetail) -> Result<(), String> {
    Ok(())
}
