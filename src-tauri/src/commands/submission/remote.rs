//! 远端稿件状态拉取、审核状态对账与合集操作。

use super::*;

pub(super) fn update_merged_video_by_remote_binding(
    context: &SubmissionContext,
    task_id: &str,
    remote_dir: &str,
    remote_name: &str,
    local_path: &str,
    baidu_uid: Option<&str>,
) -> Result<bool, String> {
    let now = now_rfc3339();
    let stored_local_path = to_stored_submission_path(context, local_path);
    let updated = context
        .db
        .with_conn(|conn| {
            conn.execute(
        "UPDATE merged_video SET file_name = ?1, video_path = ?2, baidu_uid = ?3, update_time = ?4 \
         WHERE task_id = ?5 AND remote_dir = ?6 AND remote_name = ?7",
        (
          remote_name,
          stored_local_path,
          baidu_uid,
          &now,
          task_id,
          remote_dir,
          remote_name,
        ),
      )
        })
        .map_err(|err| err.to_string())?;
    Ok(updated > 0)
}

#[tauri::command(async)]
pub fn submission_bind_merged_remote_file(
    state: State<'_, AppState>,
    request: SubmissionBindMergedRemoteRequest,
) -> ApiResponse<String> {
    let context = SubmissionContext::new(&state);
    let task_id = request.task_id.trim().to_string();
    if task_id.is_empty() {
        return ApiResponse::error("任务ID不能为空");
    }
    if request.merged_id <= 0 {
        return ApiResponse::error("合并视频ID无效");
    }
    let remote_path = request.remote_path.trim().to_string();
    if remote_path.is_empty() {
        return ApiResponse::error("网盘路径不能为空");
    }
    let (remote_dir, remote_name) = match split_baidu_path(&remote_path) {
        Some(value) => value,
        None => return ApiResponse::error("网盘路径格式无效"),
    };
    let current_bilibili_uid = match require_current_bilibili_uid(&state) {
        Ok(value) => value,
        Err(err) => return ApiResponse::error(err),
    };
    let current_baidu_uid = match require_logged_baidu_uid(context.db.as_ref()) {
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
        return ApiResponse::error("当前账号无权绑定该投稿任务");
    }

    let merged_exists = context.db.with_conn(|conn| {
        conn.query_row(
            "SELECT 1 FROM merged_video WHERE id = ?1 AND task_id = ?2",
            (request.merged_id, &task_id),
            |_| Ok(()),
        )
        .optional()
    });
    match merged_exists {
        Ok(Some(())) => {}
        Ok(None) => return ApiResponse::error("合并视频不存在"),
        Err(err) => return ApiResponse::error(format!("读取合并视频失败: {}", err)),
    }

    let now = now_rfc3339();
    let update_result = context.db.with_conn(|conn| {
    conn.execute(
      "UPDATE merged_video SET remote_dir = ?1, remote_name = ?2, baidu_uid = ?3, update_time = ?4 \
       WHERE id = ?5 AND task_id = ?6",
      (
        &remote_dir,
        &remote_name,
        &current_baidu_uid,
        &now,
        request.merged_id,
        &task_id,
      ),
    )?;
    conn.execute(
      "UPDATE baidu_sync_task SET baidu_uid = ?1, remote_dir = ?2, remote_name = ?3, updated_at = ?4 \
       WHERE source_type = 'submission_merged' AND source_id = ?5 AND status IN ('PENDING', 'FAILED', 'PAUSED', 'CANCELLED')",
      (&current_baidu_uid, &remote_dir, &remote_name, &now, &task_id),
    )?;
    Ok(())
  });
    if let Err(err) = update_result {
        return ApiResponse::error(format!("绑定网盘文件失败: {}", err));
    }

    ApiResponse::success("OK".to_string())
}

#[tauri::command(async)]
pub fn submission_clear_merged_remote_file(
    state: State<'_, AppState>,
    request: SubmissionClearMergedRemoteRequest,
) -> ApiResponse<String> {
    let context = SubmissionContext::new(&state);
    let task_id = request.task_id.trim().to_string();
    if task_id.is_empty() {
        return ApiResponse::error("任务ID不能为空");
    }
    if request.merged_id <= 0 {
        return ApiResponse::error("合并视频ID无效");
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

    let merged_exists = context.db.with_conn(|conn| {
        conn.query_row(
            "SELECT 1 FROM merged_video WHERE id = ?1 AND task_id = ?2",
            (request.merged_id, &task_id),
            |_| Ok(()),
        )
        .optional()
    });
    match merged_exists {
        Ok(Some(())) => {}
        Ok(None) => return ApiResponse::error("合并视频不存在"),
        Err(err) => return ApiResponse::error(format!("读取合并视频失败: {}", err)),
    }

    let now = now_rfc3339();
    let clear_result = context.db.with_conn(|conn| {
        conn.execute(
            "UPDATE merged_video SET remote_dir = NULL, remote_name = NULL, baidu_uid = NULL, update_time = ?1 \
             WHERE id = ?2 AND task_id = ?3",
            (&now, request.merged_id, &task_id),
        )?;
        conn.execute(
            "UPDATE baidu_sync_task SET baidu_uid = NULL, remote_dir = NULL, remote_name = NULL, updated_at = ?1 \
             WHERE source_type = 'submission_merged' AND source_id = ?2 AND status IN ('PENDING', 'FAILED', 'PAUSED', 'CANCELLED')",
            (&now, &task_id),
        )?;
        Ok(())
    });
    if let Err(err) = clear_result {
        return ApiResponse::error(format!("清空网盘绑定失败: {}", err));
    }

    ApiResponse::success("OK".to_string())
}

#[tauri::command]
pub async fn submission_ignore_remote_status(
    state: State<'_, AppState>,
    request: SubmissionTaskIdsRequest,
) -> Result<ApiResponse<String>, String> {
    let context = SubmissionContext::new(&state);
    let now = now_rfc3339();
    let task_ids = request
        .task_ids
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if task_ids.is_empty() {
        return Ok(ApiResponse::error("请选择要忽略的任务"));
    }

    context
        .db
        .with_conn_mut(|conn| {
            let tx = conn.transaction()?;
            for task_id in task_ids {
                tx.execute(
                    "UPDATE submission_task SET remote_state = 0, remote_state_desc = NULL, reject_reason = NULL, remote_status_ignored = 1, updated_at = ?1 WHERE task_id = ?2 AND remote_state IN (-2, -4)",
                    (&now, &task_id),
                )?;
            }
            tx.commit()?;
            Ok(())
        })
        .map_err(|err| err.to_string())?;

    Ok(ApiResponse::success("已忽略远程投稿状态".to_string()))
}

#[tauri::command]
pub async fn submission_verify_remote(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<ApiResponse<String>, String> {
    let submission_context = SubmissionContext::new(&state);
    let upload_context = UploadContext::new(&state);
    let task_id = task_id.trim().to_string();
    if task_id.is_empty() {
        return Ok(ApiResponse::error("任务ID不能为空"));
    }

    let detail = match load_task_detail(&submission_context, &task_id) {
        Ok(detail) => detail,
        Err(err) => return Ok(ApiResponse::error(err)),
    };
    if detail.task.status == "COMPLETED" {
        return Ok(ApiResponse::success(
            "该任务已完成，无需再次校验".to_string(),
        ));
    }
    if !should_reconcile_submission(&detail.task) {
        return Ok(ApiResponse::error("当前任务不需要远端校验"));
    }
    let expected_parts = detail.output_segments.len();
    if expected_parts == 0 {
        return Ok(ApiResponse::error("当前任务没有本地分P记录，无法校验"));
    }
    let auth = match load_auth_or_refresh_for_task(
        &upload_context,
        &task_id,
        "manual_remote_verification",
    )
    .await
    {
        Ok(auth) => auth,
        Err(err) => {
            return Ok(ApiResponse::error(format!(
                "读取投稿账号登录信息失败: {}",
                err
            )))
        }
    };

    if detail
        .task
        .bvid
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .is_empty()
    {
        let queue_context = build_submission_queue_context(&state);
        match reconcile_unknown_submission_by_listing(
            &queue_context,
            &detail,
            &auth,
            expected_parts,
        )
        .await
        {
            Ok(true) => {
                return Ok(ApiResponse::success(format!(
                    "已根据标题匹配远端稿件，确认{}个分P",
                    expected_parts
                )));
            }
            Ok(false) => {
                return Ok(ApiResponse::error(
                    "当前账号的审核中、未通过和已发布列表中未找到唯一匹配稿件，请稍后重试或在B站创作中心确认",
                ));
            }
            Err(err) => return Ok(ApiResponse::error(format!("远端校验失败: {}", err))),
        }
    }

    append_log(
        &state.app_log_path,
        &format!(
            "submission_manual_remote_verify_start task_id={} expected_parts={}",
            task_id, expected_parts
        ),
    );
    let verify_result = timeout(
        Duration::from_secs(SUBMISSION_MANUAL_VERIFY_TIMEOUT_SECS),
        try_complete_existing_submission(
            &upload_context,
            &submission_context,
            &task_id,
            &auth,
            expected_parts,
            "manual_remote_verification",
        ),
    )
    .await;
    match verify_result {
        Err(_) => {
            append_log(
                &state.app_log_path,
                &format!(
                    "submission_manual_remote_verify_timeout task_id={} timeout_secs={}",
                    task_id, SUBMISSION_MANUAL_VERIFY_TIMEOUT_SECS
                ),
            );
            return Ok(ApiResponse::error(format!(
                "远端校验超过{}秒，请稍后重试",
                SUBMISSION_MANUAL_VERIFY_TIMEOUT_SECS
            )));
        }
        Ok(Err(err)) => return Ok(ApiResponse::error(format!("远端校验失败: {}", err))),
        Ok(Ok(false)) => return Ok(ApiResponse::error("未找到可校验的远端稿件")),
        Ok(Ok(true)) => {}
    }

    let updated = match load_task_detail(&submission_context, &task_id) {
        Ok(detail) => detail,
        Err(err) => return Ok(ApiResponse::error(err)),
    };
    if updated.task.status == "COMPLETED" {
        append_log(
            &state.app_log_path,
            &format!(
                "submission_manual_remote_verify_ok task_id={} parts={}",
                task_id, expected_parts
            ),
        );
        return Ok(ApiResponse::success(format!(
            "远端校验通过，已确认{}个分P",
            expected_parts
        )));
    }
    let reason = updated
        .task
        .workflow_status
        .as_ref()
        .and_then(|status| status.error_message.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("远端稿件状态暂时无法确认");
    Ok(ApiResponse::error(format!("远端校验未通过: {}", reason)))
}

pub(super) async fn reconcile_unknown_submission_by_listing(
    context: &SubmissionQueueContext,
    detail: &SubmissionTaskDetail,
    auth: &AuthInfo,
    expected_parts: usize,
) -> Result<bool, String> {
    let published_since = DateTime::parse_from_rfc3339(detail.task.created_at.trim())
        .map_err(|err| format!("任务创建时间格式无效: {}", err))?
        .timestamp()
        .saturating_sub(REMOTE_UNKNOWN_MATCH_WINDOW_SECS);
    let remote_map = fetch_remote_audit_map(context, auth, Some(published_since)).await?;
    let task = RemoteTrackedTask {
        task_id: detail.task.task_id.clone(),
        bvid: String::new(),
        aid: detail.task.aid.unwrap_or(0),
        title: detail.task.title.clone(),
        status: detail.task.status.clone(),
        remote_state: detail.task.remote_state,
        remote_status_ignored: detail.task.remote_status_ignored,
        created_at: detail.task.created_at.clone(),
        reconciliation_at: detail.task.updated_at.clone(),
        error_message: detail
            .task
            .workflow_status
            .as_ref()
            .and_then(|status| status.error_message.clone())
            .unwrap_or_default(),
        expected_parts,
        collection_section_id: detail.task.collection_section_id.unwrap_or(0),
    };
    let matched = find_unique_unknown_submission_match(
        &task,
        &remote_map,
        &HashSet::new(),
        Utc::now().timestamp(),
    );
    let Some(info) = matched else {
        append_log(
            &context.app_log_path,
            &format!(
                "submission_manual_remote_match_missing task_id={} title={} expected_parts={}",
                detail.task.task_id, detail.task.title, expected_parts
            ),
        );
        return Ok(false);
    };
    if info.part_count == 0 {
        return Ok(false);
    }
    if info.part_count != expected_parts {
        let reason = format!(
            "远程分P数量与本地不一致（本地={}，远程={}）",
            expected_parts, info.part_count
        );
        update_submission_status_with_reason(
            &SubmissionContext {
                db: context.db.clone(),
                app_log_path: context.app_log_path.clone(),
                edit_upload_state: context.edit_upload_state.clone(),
                local_path_prefix: load_local_path_prefix(context.db.as_ref()),
            },
            &detail.task.task_id,
            "FAILED",
            Some(&reason),
        )?;
        return Err(reason);
    }
    let now = now_rfc3339();
    context
        .db
        .with_conn_mut(|conn| {
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE submission_task SET status = 'COMPLETED', bvid = ?1, aid = ?2, remote_state = ?3, remote_state_desc = ?4, reject_reason = ?5, priority = 0, priority_at = NULL, updated_at = ?6 WHERE task_id = ?7",
            (
                info.bvid.as_str(),
                if info.aid > 0 { info.aid } else { detail.task.aid.unwrap_or(0) },
                info.state,
                info.state_desc.as_deref(),
                info.reject_reason.as_deref(),
                now.as_str(),
                detail.task.task_id.as_str(),
            ),
        )?;
        tx.execute(
            "UPDATE workflow_instances SET status = 'COMPLETED', current_step = NULL, progress = 100, error_message = NULL, completed_at = ?1, updated_at = ?1 WHERE task_id = ?2",
            (now.as_str(), detail.task.task_id.as_str()),
        )?;
        tx.commit()?;
            Ok(())
        })
        .map_err(|err| err.to_string())?;
    append_log(
        &context.app_log_path,
        &format!(
            "submission_manual_remote_match_ok task_id={} bvid={} aid={} parts={}",
            detail.task.task_id, info.bvid, info.aid, expected_parts
        ),
    );
    Ok(true)
}

#[tauri::command(async)]
pub fn submission_confirm_remote_missing(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<ApiResponse<String>, String> {
    let context = SubmissionContext::new(&state);
    let task_id = task_id.trim().to_string();
    if task_id.is_empty() {
        return Ok(ApiResponse::error("任务ID不能为空"));
    }
    let detail = match load_task_detail(&context, &task_id) {
        Ok(detail) => detail,
        Err(err) => return Ok(ApiResponse::error(err)),
    };
    if detail
        .task
        .bvid
        .as_deref()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
    {
        return Ok(ApiResponse::error("任务已有BVID，不能按远端未生成处理"));
    }
    let error_message = detail
        .task
        .workflow_status
        .as_ref()
        .and_then(|status| status.error_message.as_deref())
        .unwrap_or("");
    if detail.task.status != SUBMISSION_STATUS_SUBMIT_UNKNOWN
        && !is_submit_result_unknown_error(error_message)
    {
        return Ok(ApiResponse::error("当前任务不处于提交结果待确认状态"));
    }

    let now = now_rfc3339();
    context
        .db
        .with_conn_mut(|conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "UPDATE task_output_segment SET upload_status = 'PENDING', cid = NULL, file_name = NULL, upload_progress = 0, upload_uploaded_bytes = 0, upload_total_bytes = 0, upload_session_id = NULL, upload_biz_id = 0, upload_endpoint = NULL, upload_auth = NULL, upload_uri = NULL, upload_chunk_size = 0, upload_last_part_index = 0 WHERE task_id = ?1 AND NOT (upload_status = 'SUCCESS' AND cid IS NOT NULL AND cid > 0 AND file_name IS NOT NULL AND TRIM(file_name) != '')",
                [task_id.as_str()],
            )?;
            tx.execute(
                "UPDATE submission_task SET status = ?1, remote_state = NULL, remote_state_desc = NULL, reject_reason = NULL, updated_at = ?2 WHERE task_id = ?3",
                (
                    "WAITING_UPLOAD",
                    now.as_str(),
                    task_id.as_str(),
                ),
            )?;
            tx.execute(
                "UPDATE workflow_instances SET status = ?1, current_step = NULL, progress = 0, error_message = NULL, updated_at = ?2 WHERE task_id = ?3",
                (
                    SUBMISSION_STATUS_REUPLOAD_PENDING,
                    now.as_str(),
                    task_id.as_str(),
                ),
            )?;
            tx.commit()?;
            Ok(())
        })
        .map_err(|err| err.to_string())?;
    append_log(
        &context.app_log_path,
        &format!(
            "submission_confirm_remote_missing task_id={} reset_uploads=false queued=true",
            task_id
        ),
    );
    Ok(ApiResponse::success(
        "已确认远端未生成稿件，已复用成功分P并重新加入投稿队列".to_string(),
    ))
}

pub(super) fn build_remote_source_path(bvid: &str, cid: i64, part_name: &str) -> String {
    let safe_bvid = sanitize_filename(bvid.trim());
    let safe_part = sanitize_filename(part_name.trim());
    let fallback_part = if safe_part.trim().is_empty() {
        "part".to_string()
    } else {
        safe_part
    };
    format!("remote/{}/{}_{}.mp4", safe_bvid, cid.max(0), fallback_part)
}

pub(super) async fn fetch_remote_collection_info_by_aid(
    state: &State<'_, AppState>,
    auth: Option<&AuthInfo>,
    aid: i64,
) -> Option<(i64, String)> {
    if aid <= 0 {
        return None;
    }
    let params = vec![
        ("id".to_string(), aid.to_string()),
        ("t".to_string(), Utc::now().timestamp_millis().to_string()),
    ];
    let url = "https://member.bilibili.com/x2/creative/web/season/aid";
    match state.bilibili.get_json(url, &params, auth, false).await {
        Ok(data) => {
            let collection_id = json_i64_compatible(data.get("id")).filter(|value| *value > 0)?;
            let collection_name = json_string_compatible(data.get("title"))?;
            append_log(
                &state.app_log_path,
                &format!(
                    "remote_preview_collection_hit aid={} season_id={} season_name={}",
                    aid, collection_id, collection_name
                ),
            );
            Some((collection_id, collection_name))
        }
        Err(err) => {
            append_log(
                &state.app_log_path,
                &format!("remote_preview_collection_miss aid={} err={}", aid, err),
            );
            None
        }
    }
}

pub(super) async fn fetch_public_remote_video_preview(
    state: &State<'_, AppState>,
    auth: Option<&AuthInfo>,
    input: &str,
    bvid: Option<String>,
    aid: Option<i64>,
) -> Result<SubmissionRemoteVideoPreview, String> {
    let mut params = Vec::new();
    if let Some(bvid) = bvid.as_ref() {
        params.push(("bvid".to_string(), bvid.to_string()));
    }
    if let Some(aid) = aid {
        params.push(("aid".to_string(), aid.to_string()));
    }
    let url = format!("{}/x/web-interface/view", state.bilibili.base_url());
    let data = state
        .bilibili
        .get_json(&url, &params, auth, false)
        .await
        .map_err(|err| format!("拉取公开视频详情失败: {}", err))?;

    let resolved_bvid = json_string_compatible(data.get("bvid"))
        .or_else(|| bvid.clone())
        .ok_or_else(|| "公开视频详情缺少BVID".to_string())?;
    let resolved_aid = json_i64_compatible(data.get("aid")).or(aid).unwrap_or(0);
    if resolved_aid <= 0 || resolved_bvid.trim().is_empty() {
        return Err("公开视频详情无效".to_string());
    }

    let title = json_string_compatible(data.get("title"))
        .or_else(|| bvid.clone())
        .unwrap_or_else(|| "未命名视频".to_string());
    let description = json_string_compatible(data.get("desc"));
    let cover_url = json_string_compatible(data.get("pic"));
    let partition_id = json_i64_compatible(data.get("tid")).filter(|value| *value > 0);
    let partition_name = json_string_compatible(data.get("tname"));
    let owner_mid = data
        .get("owner")
        .and_then(|value| json_i64_compatible(value.get("mid")))
        .filter(|value| *value > 0);
    let owner_name = data
        .get("owner")
        .and_then(|value| json_string_compatible(value.get("name")));

    let pages = data
        .get("pages")
        .and_then(|value| value.as_array())
        .map(|list| {
            list.iter()
                .enumerate()
                .filter_map(|(index, page)| {
                    let cid = json_i64_compatible(page.get("cid"))?;
                    if cid <= 0 {
                        return None;
                    }
                    let page_no =
                        json_i64_compatible(page.get("page")).unwrap_or((index + 1) as i64);
                    let part_name = json_string_compatible(page.get("part"))
                        .or_else(|| json_string_compatible(page.get("title")))
                        .unwrap_or_else(|| format!("P{}", page_no.max(1)));
                    let duration_seconds = json_i64_compatible(page.get("duration"))
                        .unwrap_or(0)
                        .max(0);
                    let safe_part = sanitize_filename(&part_name);
                    let file_name = if safe_part.trim().is_empty() {
                        format!("{}_P{}.mp4", resolved_bvid, page_no.max(1))
                    } else {
                        format!("{}_P{}_{}.mp4", resolved_bvid, page_no.max(1), safe_part)
                    };
                    Some(SubmissionRemoteVideoPage {
                        source_key: format!("{}:{}", resolved_bvid, cid),
                        bvid: resolved_bvid.clone(),
                        aid: resolved_aid,
                        cid,
                        page: page_no.max(1),
                        part_name: part_name.clone(),
                        duration_seconds,
                        start_time: "00:00:00".to_string(),
                        end_time: seconds_to_hms(duration_seconds),
                        file_name,
                        remote_file_name: None,
                        source_file_path: build_remote_source_path(&resolved_bvid, cid, &part_name),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(SubmissionRemoteVideoPreview {
        input: input.trim().to_string(),
        bvid: resolved_bvid,
        aid: resolved_aid,
        title,
        description,
        cover_url,
        partition_id,
        partition_name,
        tags: None,
        collection_id: None,
        collection_name: None,
        enable_segmentation: false,
        segment_duration_seconds: 133,
        owner_mid,
        owner_name,
        pages,
    })
}

pub(super) async fn fetch_remote_video_preview(
    state: &State<'_, AppState>,
    input: &str,
) -> Result<SubmissionRemoteVideoPreview, String> {
    let bvid = extract_bvid_from_input(input);
    let aid = extract_aid_from_input(input);
    if bvid.is_none() && aid.is_none() {
        return Err("请输入有效的视频链接或BVID".to_string());
    }
    let mut params = Vec::new();
    if let Some(bvid) = bvid.as_ref() {
        params.push(("bvid".to_string(), bvid.to_string()));
    }
    if let Some(aid) = aid {
        params.push(("aid".to_string(), aid.to_string()));
    }
    let auth = state.login_store.load_auth_info(&state.db).ok().flatten();
    let url = "https://member.bilibili.com/x/vupre/web/archive/view";
    let data = state
        .bilibili
        .get_json(url, &params, auth.as_ref(), false)
        .await
        .map_err(|err| format!("拉取视频详情失败: {}", err))?;
    let raw_preview_json = serde_json::to_string(&data).unwrap_or_else(|_| "{}".to_string());
    append_log(
        &state.app_log_path,
        &format!(
            "remote_preview_raw input={} payload={}",
            input.trim(),
            raw_preview_json
        ),
    );

    let creator_archive = data.get("archive");
    let creator_aid = creator_archive
        .and_then(|value| json_i64_compatible(value.get("aid")))
        .unwrap_or(0);
    let creator_bvid = creator_archive
        .and_then(|value| json_string_compatible(value.get("bvid")))
        .unwrap_or_default();
    let creator_pages = data
        .get("videos")
        .and_then(|value| value.as_array())
        .or_else(|| data.get("pages").and_then(|value| value.as_array()))
        .map(|value| value.len())
        .unwrap_or(0);
    if creator_aid <= 0 || creator_bvid.trim().is_empty() || creator_pages == 0 {
        append_log(
            &state.app_log_path,
            &format!(
                "remote_preview_fallback_public input={} creator_aid={} creator_bvid={} creator_pages={}",
                input.trim(),
                creator_aid,
                creator_bvid,
                creator_pages
            ),
        );
        return fetch_public_remote_video_preview(state, auth.as_ref(), input, bvid, aid).await;
    }

    let archive = data
        .get("archive")
        .ok_or_else(|| "视频详情缺少archive字段".to_string())?;
    let resolved_bvid = json_string_compatible(archive.get("bvid"))
        .or_else(|| bvid.clone())
        .ok_or_else(|| "视频详情缺少BVID".to_string())?;
    let resolved_aid = json_i64_compatible(archive.get("aid")).or(aid).unwrap_or(0);
    let title = json_string_compatible(archive.get("title"))
        .or_else(|| json_string_compatible(data.get("title")))
        .or_else(|| bvid.clone())
        .unwrap_or_else(|| "未命名视频".to_string());
    let description = json_string_compatible(archive.get("desc"));
    let description = description.or_else(|| {
        archive
            .get("desc_v2")
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("raw_text").and_then(|value| value.as_str()))
                    .map(|value| value.trim())
                    .filter(|value| !value.is_empty())
                    .collect::<Vec<_>>()
                    .join("")
            })
            .filter(|value| !value.is_empty())
    });
    let cover_url = json_string_compatible(archive.get("cover")).map(|value| {
        if value.starts_with("//") {
            format!("https:{}", value)
        } else {
            value
        }
    });
    let human_type2 = archive.get("human_type2");
    let tp_info = archive.get("tp_info");
    let partition_id = json_i64_compatible(human_type2.and_then(|value| value.get("id")))
        .or_else(|| {
            json_i64_compatible(
                tp_info
                    .and_then(|value| value.get("tp"))
                    .and_then(|value| value.get("id")),
            )
        })
        .or_else(|| json_i64_compatible(archive.get("tid")))
        .or_else(|| json_i64_compatible(archive.get("type_id")))
        .filter(|value| *value > 0);
    let partition_name = json_string_compatible(human_type2.and_then(|value| value.get("name")))
        .or_else(|| {
            json_string_compatible(
                tp_info
                    .and_then(|value| value.get("tp"))
                    .and_then(|value| value.get("name")),
            )
        })
        .or_else(|| json_string_compatible(archive.get("type_name")));
    append_log(
    &state.app_log_path,
    &format!(
      "remote_preview_partition input={} bvid={} human_type2_id={} tid={} tp_id={} human_type2_name={} tp_name={} picked_tid={} picked_name={}",
      input.trim(),
      resolved_bvid,
      json_i64_compatible(human_type2.and_then(|value| value.get("id"))).unwrap_or_default(),
      json_i64_compatible(archive.get("tid")).unwrap_or_default(),
      json_i64_compatible(tp_info.and_then(|value| value.get("tp")).and_then(|value| value.get("id")))
        .unwrap_or_default(),
      human_type2
        .and_then(|value| value.get("name"))
        .and_then(|value| value.as_str())
        .unwrap_or("-"),
      tp_info
        .and_then(|value| value.get("tp"))
        .and_then(|value| value.get("name"))
        .and_then(|value| value.as_str())
        .unwrap_or("-"),
      partition_id.unwrap_or_default(),
      partition_name.as_deref().unwrap_or("-")
    ),
  );
    let owner_mid = json_i64_compatible(archive.get("mid"))
        .or_else(|| json_i64_compatible(archive.get("author_mid")))
        .or_else(|| {
            archive
                .get("owner")
                .and_then(|value| json_i64_compatible(value.get("mid")))
        })
        .or_else(|| {
            archive
                .get("author")
                .and_then(|value| json_i64_compatible(value.get("mid")))
        })
        .filter(|value| *value > 0);
    let owner_name = json_string_compatible(archive.get("author"))
        .or_else(|| {
            archive
                .get("owner")
                .and_then(|value| json_string_compatible(value.get("name")))
        })
        .or_else(|| {
            archive
                .get("author")
                .and_then(|value| json_string_compatible(value.get("name")))
        });
    if resolved_aid <= 0 || resolved_bvid.trim().is_empty() {
        return Err("视频详情不存在或无权限".to_string());
    }
    let collection_info =
        fetch_remote_collection_info_by_aid(state, auth.as_ref(), resolved_aid).await;
    let collection_id = collection_info.as_ref().map(|item| item.0);
    let collection_name = collection_info.as_ref().map(|item| item.1.clone());

    let tags = if let Some(raw_tags) = json_string_compatible(archive.get("tag")) {
        normalize_csv_text(&raw_tags)
    } else {
        let tag_url = format!("{}/x/tag/archive/tags", state.bilibili.base_url());
        let tag_params = vec![("bvid".to_string(), resolved_bvid.clone())];
        state
            .bilibili
            .get_json(&tag_url, &tag_params, auth.as_ref(), false)
            .await
            .ok()
            .and_then(|value| value.as_array().cloned())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("tag_name").and_then(|value| value.as_str()))
                    .map(|value| value.trim())
                    .filter(|value| !value.is_empty())
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .and_then(|value| normalize_csv_text(&value))
    };

    let page_list = data
        .get("videos")
        .and_then(|value| value.as_array())
        .or_else(|| data.get("pages").and_then(|value| value.as_array()));
    let pages = page_list
        .map(|list| {
            list.iter()
                .enumerate()
                .filter_map(|(index, page)| {
                    let cid = json_i64_compatible(page.get("cid"))?;
                    if cid <= 0 {
                        return None;
                    }
                    let page_no = json_i64_compatible(page.get("index"))
                        .or_else(|| json_i64_compatible(page.get("page")))
                        .unwrap_or((index + 1) as i64);
                    let part_name = json_string_compatible(page.get("title"))
                        .or_else(|| json_string_compatible(page.get("part")))
                        .unwrap_or_else(|| format!("P{}", page_no.max(1)));
                    let duration_seconds = json_i64_compatible(page.get("duration"))
                        .unwrap_or(0)
                        .max(0);
                    let safe_part = sanitize_filename(&part_name);
                    let file_name = if safe_part.trim().is_empty() {
                        format!("{}_P{}.mp4", resolved_bvid, page_no.max(1))
                    } else {
                        format!("{}_P{}_{}.mp4", resolved_bvid, page_no.max(1), safe_part)
                    };
                    let source_key = format!("{}:{}", resolved_bvid, cid);
                    let remote_file_name = json_string_compatible(page.get("filename"));
                    Some(SubmissionRemoteVideoPage {
                        source_key,
                        bvid: resolved_bvid.clone(),
                        aid: resolved_aid,
                        cid,
                        page: page_no.max(1),
                        part_name: part_name.clone(),
                        duration_seconds,
                        start_time: "00:00:00".to_string(),
                        end_time: seconds_to_hms(duration_seconds),
                        file_name,
                        remote_file_name,
                        source_file_path: build_remote_source_path(&resolved_bvid, cid, &part_name),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(SubmissionRemoteVideoPreview {
        input: input.trim().to_string(),
        bvid: resolved_bvid,
        aid: resolved_aid,
        title,
        description,
        cover_url,
        partition_id,
        partition_name,
        tags,
        collection_id,
        collection_name,
        enable_segmentation: false,
        segment_duration_seconds: 133,
        owner_mid,
        owner_name,
        pages,
    })
}

#[tauri::command]
pub async fn submission_remote_video_preview(
    state: State<'_, AppState>,
    request: SubmissionRemoteVideoPreviewRequest,
) -> Result<ApiResponse<SubmissionRemoteVideoPreview>, String> {
    let input = request.input.trim().to_string();
    if input.is_empty() {
        return Ok(ApiResponse::error("请输入视频链接或BVID"));
    }
    let preview = match fetch_remote_video_preview(&state, &input).await {
        Ok(value) => value,
        Err(err) => return Ok(ApiResponse::error(err)),
    };
    let enforce_owner_match = request.enforce_owner_match.unwrap_or(true);
    if enforce_owner_match {
        let current_uid = match require_current_bilibili_uid(&state) {
            Ok(uid) => uid,
            Err(err) => return Ok(ApiResponse::error(err)),
        };
        match preview.owner_mid {
            Some(owner_mid) if owner_mid == current_uid => {}
            Some(_) => return Ok(ApiResponse::error("该视频非当前账号视频")),
            None => return Ok(ApiResponse::error("无法校验视频所属UP主")),
        }
    }
    Ok(ApiResponse::success(preview))
}

#[tauri::command(async)]
pub fn submission_remote_import_save(
    state: State<'_, AppState>,
    request: SubmissionRemoteImportSaveRequest,
) -> ApiResponse<SubmissionRemoteImportSaveResult> {
    let context = SubmissionContext::new(&state);
    let current_uid = match require_current_bilibili_uid(&state) {
        Ok(uid) => uid,
        Err(err) => return ApiResponse::error(err),
    };
    let title = request.basic.title.trim().to_string();
    if title.is_empty() {
        return ApiResponse::error("投稿标题不能为空");
    }
    if request.basic.partition_id <= 0 {
        return ApiResponse::error("请先选择分区");
    }
    if request.sources.is_empty() {
        return ApiResponse::error("请至少配置一条源视频");
    }

    let origin_bvid = normalize_optional_text(Some(request.basic.origin_bvid.clone()));
    let import_mode = normalize_import_mode(request.basic.import_mode.as_deref());
    if let Some(bvid) = origin_bvid.as_deref() {
        let existing = match context.db.with_conn(|conn| {
            conn.query_row(
                "SELECT task_id FROM submission_task WHERE bvid = ?1 LIMIT 1",
                [bvid],
                |row| row.get::<_, String>(0),
            )
            .optional()
        }) {
            Ok(value) => value,
            Err(err) => return ApiResponse::error(format!("查询投稿任务失败: {}", err)),
        };
        if let Some(task_id) = existing {
            return ApiResponse::error(format!("该视频已存在投稿任务: {}", task_id));
        }
    }

    #[derive(Clone)]
    struct NormalizedSourceDraft {
        key: String,
        bvid: String,
        cid: i64,
        part_name: String,
        duration_seconds: i64,
        sort_order: i64,
        start_time: String,
        end_time: String,
        source_file_path: String,
        remote_file_name: Option<String>,
    }

    let mut normalized_sources = Vec::new();
    let mut source_key_set = HashSet::new();
    for item in &request.sources {
        let bvid = item.bvid.trim().to_string();
        if bvid.is_empty() {
            return ApiResponse::error("源视频BVID不能为空");
        }
        if item.cid <= 0 {
            return ApiResponse::error("源视频CID不合法");
        }
        let key = {
            let raw = item.source_key.trim();
            if raw.is_empty() {
                format!("{}:{}", bvid, item.cid)
            } else {
                raw.to_string()
            }
        };
        if !source_key_set.insert(key.clone()) {
            return ApiResponse::error(format!("源视频重复: {}", key));
        }
        let part_name = item.part_name.trim().to_string();
        let part_name = if part_name.is_empty() {
            format!("P{}", item.sort_order.max(1))
        } else {
            part_name
        };
        let duration_seconds = item.duration_seconds.unwrap_or(0).max(0);
        let default_end_time = seconds_to_hms(duration_seconds);
        let source_file_path = normalize_optional_text(item.source_file_path.clone())
            .unwrap_or_else(|| build_remote_source_path(&bvid, item.cid, &part_name));
        normalized_sources.push(NormalizedSourceDraft {
            key,
            bvid: bvid.clone(),
            cid: item.cid,
            part_name: part_name.clone(),
            duration_seconds,
            sort_order: item.sort_order.max(1),
            start_time: normalize_hms(item.start_time.as_deref(), "00:00:00"),
            end_time: normalize_hms(item.end_time.as_deref(), &default_end_time),
            source_file_path,
            remote_file_name: normalize_optional_text(item.remote_file_name.clone()),
        });
    }
    normalized_sources.sort_by_key(|item| item.sort_order);

    #[derive(Clone)]
    struct NormalizedMergedDraft {
        sort_order: i64,
        file_name: String,
        remote_dir: Option<String>,
        remote_name: Option<String>,
        source_keys: Vec<String>,
    }

    let mut normalized_merged = Vec::new();
    let mut globally_used_source_keys = HashSet::new();
    let source_keys_ref = normalized_sources
        .iter()
        .map(|item| item.key.clone())
        .collect::<HashSet<_>>();
    let safe_title = {
        let value = sanitize_filename(&title);
        if value.trim().is_empty() {
            "remote_import".to_string()
        } else {
            value
        }
    };
    for (index, merged) in request.merged_videos.iter().enumerate() {
        let mut local_unique = HashSet::new();
        let mut selected_keys = Vec::new();
        for key in &merged.source_keys {
            let key = key.trim().to_string();
            if key.is_empty() {
                continue;
            }
            if !source_keys_ref.contains(&key) {
                return ApiResponse::error(format!("合并视频包含未知源视频: {}", key));
            }
            if !local_unique.insert(key.clone()) {
                return ApiResponse::error(format!("合并视频内存在重复源视频: {}", key));
            }
            if !globally_used_source_keys.insert(key.clone()) {
                return ApiResponse::error(format!("同一源视频不能重复加入多个合并视频: {}", key));
            }
            selected_keys.push(key);
        }
        if selected_keys.is_empty() {
            continue;
        }
        if import_mode == IMPORT_MODE_NON_SEGMENTED && selected_keys.len() != 1 {
            return ApiResponse::error("不分段模式下每个合并视频必须且只能绑定一个源视频");
        }
        let default_file_name = format!("{}_merged_{:02}.mp4", safe_title, index + 1);
        let file_name = normalize_optional_text(merged.file_name.clone())
            .map(|value| {
                if value.to_ascii_lowercase().ends_with(".mp4") {
                    value
                } else {
                    format!("{}.mp4", value)
                }
            })
            .unwrap_or(default_file_name);
        let file_name = {
            let normalized = sanitize_filename(file_name.trim());
            if normalized.trim().is_empty() {
                format!("{}_merged_{:02}.mp4", safe_title, index + 1)
            } else {
                normalized
            }
        };
        let (remote_dir, remote_name) = match normalize_optional_text(merged.remote_path.clone()) {
            Some(remote_path) => match split_baidu_path(&remote_path) {
                Some((dir, name)) => (Some(dir), Some(name)),
                None => return ApiResponse::error("网盘路径格式不合法"),
            },
            None => (None, None),
        };
        normalized_merged.push(NormalizedMergedDraft {
            sort_order: merged.sort_order.unwrap_or((index + 1) as i64).max(1),
            file_name,
            remote_dir,
            remote_name,
            source_keys: selected_keys,
        });
    }
    normalized_merged.sort_by_key(|item| item.sort_order);
    if import_mode == IMPORT_MODE_NON_SEGMENTED
        && (normalized_merged.len() != normalized_sources.len()
            || globally_used_source_keys.len() != normalized_sources.len())
    {
        return ApiResponse::error("不分段模式下需保持合并视频与源视频数量一致并一一绑定");
    }

    let task_id = uuid::Uuid::new_v4().to_string();
    let now = now_rfc3339();
    let description = normalize_optional_text(request.basic.description.clone());
    let cover_url = normalize_optional_text(request.basic.cover_url.clone());
    let tags = normalize_optional_text(request.basic.tags.clone());
    let video_type = normalize_optional_text(request.basic.video_type.clone())
        .unwrap_or_else(|| "ORIGINAL".to_string());
    let segment_prefix = normalize_optional_text(request.basic.segment_prefix.clone());
    let enable_segmentation = request.basic.enable_segmentation.unwrap_or(false);
    let segment_duration_seconds = request
        .basic
        .segment_duration_seconds
        .unwrap_or(133)
        .clamp(1, 24 * 3600);
    let origin_aid = request.basic.origin_aid.filter(|value| *value > 0);
    let workflow_config_value = serde_json::json!({
      "enableDirectSubmission": true,
      "enableClipping": true,
      "enableMerging": true,
      "enableSegmentation": enable_segmentation,
      "segmentationConfig": {
        "segmentDurationSeconds": segment_duration_seconds,
        "maxSegments": 50,
        "enableSegmentation": enable_segmentation
      },
      "retainOriginalFiles": true,
      "processingPriority": "NORMAL",
      "maxRetries": 3,
      "timeoutMinutes": 30
    });
    let workflow_config_json = match serde_json::to_string(&workflow_config_value) {
        Ok(value) => value,
        Err(err) => return ApiResponse::error(format!("工作流配置序列化失败: {}", err)),
    };
    let workflow_instance_id = uuid::Uuid::new_v4().to_string();
    let merge_workflow_dir = resolve_submission_base_dir(&context, &task_id)
        .join("runs")
        .join(sanitize_filename(&workflow_instance_id))
        .join("merge");
    append_log(
        &context.app_log_path,
        &format!(
            "submission_remote_import_save_start task_id={} sources={} merged={}",
            task_id,
            normalized_sources.len(),
            normalized_merged.len()
        ),
    );
    let task_id_for_log = task_id.clone();

    let save_result = context.db.with_conn_mut(|conn| {
    let tx = conn.transaction()?;
    tx.execute(
      "INSERT INTO submission_task (task_id, status, priority, priority_at, title, description, cover_url, partition_id, tags, topic_id, mission_id, activity_title, video_type, collection_id, collection_section_id, bvid, aid, remote_state, reject_reason, created_at, updated_at, bilibili_uid, import_mode, baidu_sync_enabled, source_type) \
       VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7, ?8, NULL, NULL, NULL, ?9, ?10, ?11, ?12, ?13, 0, NULL, ?14, ?15, ?16, ?17, 0, ?18)",
      params![
        &task_id,
        "COMPLETED",
        0i64,
        &title,
        description.as_deref(),
        cover_url.as_deref(),
        request.basic.partition_id,
        tags.as_deref(),
        &video_type,
        request.basic.collection_id,
        request
          .basic
          .collection_id
          .filter(|value| *value > 0)
          .and_then(|_| request.basic.collection_section_id.filter(|value| *value > 0)),
        origin_bvid.as_deref(),
        origin_aid,
        &now,
        &now,
        current_uid,
        &import_mode,
        "NORMAL",
      ],
    )?;
    if segment_prefix.is_some() {
      tx.execute(
        "UPDATE submission_task SET segment_prefix = ?1 WHERE task_id = ?2",
        (segment_prefix.as_deref(), &task_id),
      )?;
    }

    #[derive(Clone)]
    struct PersistedSourceRecord {
      id: String,
      stored_path: String,
      start_time: String,
      end_time: String,
      part_name: String,
      cid: i64,
      duration_seconds: i64,
      remote_file_name: Option<String>,
    }
    let mut persisted_source_map: HashMap<String, PersistedSourceRecord> = HashMap::new();
    for (index, source) in normalized_sources.iter().enumerate() {
      let source_id = uuid::Uuid::new_v4().to_string();
      let stored_source_path = to_stored_submission_path(&context, &source.source_file_path);
      let remote_video_url = format!("https://www.bilibili.com/video/{}", source.bvid);
      tx.execute(
        "INSERT INTO task_source_video (id, task_id, source_file_path, source_type, remote_video_url, remote_bvid, remote_aid, remote_cid, remote_part_title, sort_order, start_time, end_time) \
         VALUES (?1, ?2, ?3, 'BILIBILI', ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        (
          &source_id,
          &task_id,
          &stored_source_path,
          &remote_video_url,
          &source.bvid,
          origin_aid,
          source.cid,
          &source.part_name,
          (index + 1) as i64,
          &source.start_time,
          &source.end_time,
        ),
      )?;
      persisted_source_map.insert(
        source.key.clone(),
        PersistedSourceRecord {
          id: source_id,
          stored_path: stored_source_path,
          start_time: source.start_time.clone(),
          end_time: source.end_time.clone(),
          part_name: source.part_name.clone(),
          cid: source.cid,
          duration_seconds: source.duration_seconds,
          remote_file_name: source.remote_file_name.clone(),
        },
      );
    }

    let mut next_output_part_order = 1i64;
    for merged in &normalized_merged {
      let total_duration = merged
        .source_keys
        .iter()
        .filter_map(|key| persisted_source_map.get(key))
        .map(|item| item.duration_seconds.max(0))
        .sum::<i64>();
      let generated_video_path = merge_workflow_dir.join(&merged.file_name);
      let generated_video_path = generated_video_path.to_string_lossy().to_string();
      let stored_video_path = to_stored_submission_path(&context, &generated_video_path);
      let primary_source = merged
        .source_keys
        .first()
        .and_then(|key| persisted_source_map.get(key));
      let primary_source_cid = primary_source.map(|item| item.cid).unwrap_or(0);
      let primary_source_remote_file_name =
        primary_source.and_then(|item| item.remote_file_name.as_deref());
      let merged_upload_progress =
        if primary_source_cid > 0 && primary_source_remote_file_name.is_some() {
          100.0
        } else {
          0.0
        };
      tx.execute(
        "INSERT INTO merged_video (task_id, file_name, video_path, remote_dir, remote_name, duration, status, upload_progress, upload_uploaded_bytes, upload_total_bytes, upload_cid, upload_file_name, create_time, update_time) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, 0, ?9, ?10, ?11, ?12)",
        (
          &task_id,
          &merged.file_name,
          &stored_video_path,
          merged.remote_dir.as_deref(),
          merged.remote_name.as_deref(),
          total_duration,
          2i64,
          merged_upload_progress,
          if primary_source_cid > 0 {
            Some(primary_source_cid)
          } else {
            None
          },
          primary_source_remote_file_name,
          &now,
          &now,
        ),
      )?;
      let merged_id = tx.last_insert_rowid();
      for (source_index, source_key) in merged.source_keys.iter().enumerate() {
        let Some(source_record) = persisted_source_map.get(source_key) else {
          continue;
        };
        tx.execute(
          "INSERT INTO merged_source_video (task_id, merged_id, source_id, source_file_path, sort_order, start_time, end_time, create_time, update_time) \
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
          (
            &task_id,
            merged_id,
            &source_record.id,
            &source_record.stored_path,
            (source_index + 1) as i64,
            &source_record.start_time,
            &source_record.end_time,
            &now,
            &now,
          ),
        )?;
      }
      if import_mode == IMPORT_MODE_NON_SEGMENTED {
        let source_key = merged.source_keys.first().cloned().unwrap_or_default();
        let Some(source_record) = persisted_source_map.get(&source_key) else {
          continue;
        };
        let segment_id = uuid::Uuid::new_v4().to_string();
        let stored_remote_file_name = source_record
          .remote_file_name
          .as_deref()
          .unwrap_or(merged.file_name.as_str());
        tx.execute(
          "INSERT INTO task_output_segment (segment_id, task_id, merged_id, part_name, segment_file_path, part_order, upload_status, cid, file_name, upload_progress, upload_uploaded_bytes, upload_total_bytes, upload_session_id, upload_biz_id, upload_endpoint, upload_auth, upload_uri, upload_chunk_size, upload_last_part_index) \
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'SUCCESS', ?7, ?8, 100, 0, 0, NULL, 0, NULL, NULL, NULL, 0, 0)",
          (
            &segment_id,
            &task_id,
            merged_id,
            &source_record.part_name,
            &stored_video_path,
            next_output_part_order,
            source_record.cid,
            stored_remote_file_name,
          ),
        )?;
        next_output_part_order += 1;
      }
    }

    let config_name = format!("remote_import_{}", task_id);
    tx.execute(
      "INSERT INTO workflow_configurations (config_name, config_type, workflow_type, configuration_data, description, is_active, version, created_at, updated_at) \
       VALUES (?1, 'INSTANCE_SPECIFIC', 'VIDEO_SUBMISSION', ?2, NULL, 1, 1, ?3, ?4)",
      (&config_name, &workflow_config_json, &now, &now),
    )?;
    let config_id = tx.last_insert_rowid();
    tx.execute(
      "INSERT INTO workflow_instances (instance_id, task_id, workflow_type, status, current_step, progress, configuration_id, error_message, started_at, completed_at, created_at, updated_at) \
       VALUES (?1, ?2, 'VIDEO_SUBMISSION', 'COMPLETED', NULL, 100.0, ?3, NULL, ?4, ?5, ?6, ?7)",
      (
        &workflow_instance_id,
        &task_id,
        config_id,
        &now,
        &now,
        &now,
        &now,
      ),
    )?;
    tx.commit()?;
    Ok(SubmissionRemoteImportSaveResult {
      task_id,
      source_count: normalized_sources.len(),
      merged_count: normalized_merged.len(),
    })
  });

    match save_result {
        Ok(value) => {
            append_log(
                &context.app_log_path,
                &format!(
                    "submission_remote_import_save_ok task_id={} sources={} merged={}",
                    value.task_id, value.source_count, value.merged_count
                ),
            );
            ApiResponse::success(value)
        }
        Err(err) => {
            append_log(
                &context.app_log_path,
                &format!(
                    "submission_remote_import_save_fail task_id={} err={}",
                    task_id_for_log, err
                ),
            );
            ApiResponse::error(format!("保存失败: {}", err))
        }
    }
}

pub(super) fn is_remote_part_count_mismatch_error(err: &str) -> bool {
    err.contains("远程分P数量与本地不一致")
}

pub(super) fn is_remote_transcode_failed_state(state: Option<i64>) -> bool {
    matches!(state, Some(REMOTE_STATE_TRANSCODE_FAILED))
}

pub(super) fn should_retry_remote_part_verification(error: Option<&str>) -> bool {
    error
        .map(|error| !is_bilibili_risk_control_error(error))
        .unwrap_or(true)
}

pub(super) fn is_submission_reconciliation_status(status: &str) -> bool {
    matches!(
        status,
        SUBMISSION_STATUS_VERIFY_PENDING | SUBMISSION_STATUS_SUBMIT_UNKNOWN
    )
}

pub(super) fn collection_bind_retry_delay_secs(attempt: u32) -> u64 {
    let safe_attempt = attempt.max(1);
    let exponent = safe_attempt.saturating_sub(1);
    let multiplier = 1u64 << exponent.min(5);
    let wait = COLLECTION_BIND_RETRY_BASE_DELAY_SECS.saturating_mul(multiplier);
    wait.min(COLLECTION_BIND_RETRY_MAX_DELAY_SECS)
}

pub(super) fn is_collection_bind_pending_error(err: &str) -> bool {
    err.contains(SUBMISSION_COLLECTION_BIND_PENDING_PREFIX)
}

pub(super) fn build_collection_bind_pending_error(err: &str) -> String {
    format!(
        "{} 投稿已成功，但绑定到所选子合集失败；系统将只重试合集绑定，不会重新投稿。原始错误: {}",
        SUBMISSION_COLLECTION_BIND_PENDING_PREFIX, err
    )
}

pub(super) fn should_reconcile_submission(task: &SubmissionTaskRecord) -> bool {
    if is_submission_reconciliation_status(&task.status) {
        return true;
    }
    if task.status != "FAILED" {
        return false;
    }
    if is_remote_transcode_failed_state(task.remote_state) {
        return false;
    }
    let error = task
        .workflow_status
        .as_ref()
        .and_then(|status| status.error_message.as_deref())
        .unwrap_or("");
    is_submit_result_unknown_error(error)
        || is_submission_created_error(error)
        || (task
            .bvid
            .as_deref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
            && (is_rate_limit_error(error) || is_remote_part_count_mismatch_error(error)))
}

pub(super) fn is_confirmed_remote_missing_retry(status: &str, error_message: Option<&str>) -> bool {
    status == SUBMISSION_STATUS_REUPLOAD_PENDING
        || (status == "FAILED"
            && error_message
                .map(|message| message.contains(SUBMISSION_REMOTE_MISSING_CONFIRMED_REASON))
                .unwrap_or(false))
}

pub(super) async fn verify_remote_part_count(
    context: &UploadContext,
    auth: &AuthInfo,
    task_id: &str,
    bvid: &str,
    aid: i64,
    expected_parts: usize,
) -> Result<(), String> {
    let mut last_error: Option<String> = None;
    for attempt in 1..=REMOTE_PART_VERIFY_RETRY_LIMIT {
        match fetch_remote_archive_part_count(context, auth, bvid, aid).await {
            Ok(Some(actual_parts)) if actual_parts == expected_parts => {
                append_log(
                    &context.app_log_path,
                    &format!(
            "submission_remote_part_count_verified task_id={} attempt={} expected={} actual={}",
            task_id, attempt, expected_parts, actual_parts
          ),
                );
                return Ok(());
            }
            Ok(Some(actual_parts)) => {
                last_error = Some(format!(
                    "远程分P数量与本地不一致（本地={}，远程={}）",
                    expected_parts, actual_parts
                ));
            }
            Ok(None) => {
                last_error = Some("暂未读取到远程稿件分P信息".to_string());
            }
            Err(err) => {
                if is_rate_limit_error(&err) {
                    append_log(
                        &context.app_log_path,
                        &format!(
                            "submission_remote_part_count_rate_limited task_id={} attempt={} err={}",
                            task_id, attempt, err
                        ),
                    );
                    return Err(err);
                }
                last_error = Some(err);
            }
        }
        if attempt < REMOTE_PART_VERIFY_RETRY_LIMIT
            && should_retry_remote_part_verification(last_error.as_deref())
        {
            append_log(
                &context.app_log_path,
                &format!(
                    "submission_remote_part_count_retry task_id={} attempt={} reason={}",
                    task_id,
                    attempt,
                    last_error.as_deref().unwrap_or("unknown")
                ),
            );
            sleep(Duration::from_secs(REMOTE_PART_VERIFY_RETRY_WAIT_SECS)).await;
        }
    }
    Err(last_error.unwrap_or_else(|| "远程分P数量校验失败".to_string()))
}

pub(super) fn parse_remote_audit_info(item: &Value) -> Option<RemoteAuditInfo> {
    let archive = item.get("Archive")?;
    let bvid = archive
        .get("bvid")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if bvid.is_empty() {
        return None;
    }
    let cid_count = item
        .get("cid_list")
        .and_then(|value| value.as_array())
        .map(|items| items.len())
        .or_else(|| {
            archive
                .get("cid_list")
                .and_then(|value| value.as_array())
                .map(|items| items.len())
        })
        .unwrap_or(0);
    let video_count = item
        .get("Videos")
        .and_then(|value| value.as_array())
        .map(|items| items.len())
        .or_else(|| {
            archive
                .get("videos")
                .and_then(|value| value.as_array())
                .map(|items| items.len())
        })
        .unwrap_or(0);
    let reject_reason = item
        .get("problem_detail")
        .and_then(|value| value.as_array())
        .and_then(|items| {
            items
                .iter()
                .find_map(|detail| detail.get("reject_reason").and_then(|value| value.as_str()))
        })
        .or_else(|| {
            archive
                .get("reject_reason")
                .and_then(|value| value.as_str())
        })
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let state_desc = archive
        .get("state_desc")
        .and_then(|value| value.as_str())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    Some(RemoteAuditInfo {
        aid: archive
            .get("aid")
            .and_then(|value| value.as_i64())
            .unwrap_or(0),
        bvid,
        title: archive
            .get("title")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        part_count: cid_count.max(video_count),
        ctime: archive
            .get("ctime")
            .and_then(|value| value.as_i64())
            .unwrap_or(0),
        state: archive
            .get("state")
            .and_then(|value| value.as_i64())
            .unwrap_or(0),
        state_desc,
        reject_reason,
    })
}

pub(super) async fn submission_remote_refresh_loop(context: SubmissionQueueContext) {
    loop {
        let interval_minutes = load_download_settings_from_db(&context.db)
            .map(|settings| settings.submission_remote_refresh_minutes)
            .unwrap_or(DEFAULT_SUBMISSION_REMOTE_REFRESH_MINUTES)
            .max(1);
        let pause_count = context.remote_refresh_pause_count.load(Ordering::SeqCst);
        if pause_count > 0 {
            append_log(
                &context.app_log_path,
                &format!(
                    "submission_remote_refresh_skip reason=toolbox_bilibili_season_operation_active count={}",
                    pause_count
                ),
            );
        } else if let Err(err) = refresh_submission_remote_state(&context).await {
            append_log(
                &context.app_log_path,
                &format!("submission_remote_refresh_fail err={}", err),
            );
        }
        sleep(Duration::from_secs((interval_minutes as u64) * 60)).await;
    }
}

pub(super) async fn refresh_submission_remote_state(context: &SubmissionQueueContext) -> Result<(), String> {
    let task_groups = load_remote_tasks_by_owner(context)?;
    if task_groups.is_empty() {
        return Ok(());
    }
    let submission_context = SubmissionContext {
        db: context.db.clone(),
        app_log_path: context.app_log_path.clone(),
        edit_upload_state: context.edit_upload_state.clone(),
        local_path_prefix: load_local_path_prefix(context.db.as_ref()),
    };
    let upload_context = UploadContext {
        db: context.db.clone(),
        bilibili: context.bilibili.clone(),
        login_store: context.login_store.clone(),
        app_log_path: context.app_log_path.clone(),
        edit_upload_state: context.edit_upload_state.clone(),
    };

    let mut updates: Vec<(String, i64, Option<String>, Option<String>)> = Vec::new();
    let mut reconciled = Vec::<RemoteReconciliationUpdate>::new();
    let mut reconciliation_failures = Vec::<RemoteReconciliationFailure>::new();
    for (owner_uid, tasks) in task_groups {
        let auth = match load_remote_refresh_auth(context, owner_uid) {
            Ok(Some(auth)) => auth,
            Ok(None) => {
                append_log(
                    &context.app_log_path,
                    &format!(
                        "submission_remote_refresh_skip owner_uid={} reason=no_auth",
                        upload_owner_label(owner_uid)
                    ),
                );
                continue;
            }
            Err(err) => {
                append_log(
                    &context.app_log_path,
                    &format!(
                        "submission_remote_refresh_skip owner_uid={} reason={}",
                        upload_owner_label(owner_uid),
                        err
                    ),
                );
                continue;
            }
        };
        let published_since = tasks
            .iter()
            .filter_map(RemoteTrackedTask::published_lookup_since)
            .min();
        let remote_map = match fetch_remote_audit_map(context, &auth, published_since).await {
            Ok(map) => map,
            Err(err) => {
                append_log(
                    &context.app_log_path,
                    &format!(
                        "submission_remote_refresh_fetch_fail owner_uid={} auth_uid={} err={}",
                        upload_owner_label(owner_uid),
                        upload_owner_label(auth.user_id),
                        err
                    ),
                );
                continue;
            }
        };
        let missing_bvids: Vec<String> = tasks
            .iter()
            .filter(|task| {
                !task.bvid.trim().is_empty() && !remote_map.contains_key(task.bvid.trim())
            })
            .map(|task| task.bvid.clone())
            .collect();
        append_log(
            &context.app_log_path,
            &format!(
                "submission_remote_refresh_summary owner_uid={} auth_uid={} tasks={} remote_items={} missing={} status={}",
                upload_owner_label(owner_uid),
                upload_owner_label(auth.user_id),
                tasks.len(),
                remote_map.len(),
                missing_bvids.len(),
                REMOTE_AUDIT_STATUS_SCOPE
            ),
        );
        if remote_map.is_empty() {
            append_log(
                &context.app_log_path,
                &format!(
                    "submission_remote_refresh_remote_empty owner_uid={} tasks={} status={}",
                    upload_owner_label(owner_uid),
                    tasks.len(),
                    REMOTE_AUDIT_STATUS_SCOPE
                ),
            );
        } else if !missing_bvids.is_empty() {
            let sample = missing_bvids
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join(",");
            append_log(
                &context.app_log_path,
                &format!(
                    "submission_remote_refresh_missing owner_uid={} count={} sample={}",
                    upload_owner_label(owner_uid),
                    missing_bvids.len(),
                    sample
                ),
            );
        }

        let bound_bvids = tasks
            .iter()
            .map(|task| task.bvid.trim())
            .filter(|bvid| !bvid.is_empty())
            .map(ToString::to_string)
            .collect::<HashSet<_>>();
        let now_timestamp = Utc::now().timestamp();
        for task in tasks {
            let bvid = task.bvid.trim();
            if bvid == REMOTE_DEBUG_BVID {
                if let Some(info) = remote_map.get(bvid) {
                    append_log(
                        &context.app_log_path,
                        &format!(
                            "submission_remote_refresh_debug owner_uid={} bvid={} state={} reject_reason={}",
                            upload_owner_label(owner_uid),
                            bvid,
                            info.state,
                            info.reject_reason.as_deref().unwrap_or("")
                        ),
                    );
                } else {
                    append_log(
                        &context.app_log_path,
                        &format!(
                            "submission_remote_refresh_debug_missing owner_uid={} bvid={}",
                            upload_owner_label(owner_uid),
                            bvid
                        ),
                    );
                }
            }

            let exact_remote = if bvid.is_empty() {
                None
            } else {
                remote_map.get(bvid)
            };
            if let Some(info) = exact_remote {
                updates.push((
                    task.task_id.clone(),
                    info.state,
                    info.state_desc.clone(),
                    info.reject_reason.clone(),
                ));
            } else if !bvid.is_empty() {
                updates.push((task.task_id.clone(), 0_i64, None, None));
            }

            if !task.needs_reconciliation() {
                continue;
            }
            if task.status == SUBMISSION_STATUS_SUBMIT_UNKNOWN && !bvid.is_empty() {
                append_log(
                    &context.app_log_path,
                    &format!(
                        "submission_remote_reconcile_manual_required task_id={} bvid={} reason=existing_bvid_submit_unknown",
                        task.task_id, bvid
                    ),
                );
                continue;
            }
            let matched_remote = exact_remote.or_else(|| {
                if bvid.is_empty()
                    && (task.status == SUBMISSION_STATUS_SUBMIT_UNKNOWN
                        || is_submit_result_unknown_error(&task.error_message))
                {
                    find_unique_unknown_submission_match(
                        &task,
                        &remote_map,
                        &bound_bvids,
                        now_timestamp,
                    )
                } else {
                    None
                }
            });
            let Some(info) = matched_remote else {
                append_log(
                    &context.app_log_path,
                    &format!(
                        "submission_remote_reconcile_pending task_id={} bvid={} expected_parts={} status={}",
                        task.task_id, bvid, task.expected_parts, task.status
                    ),
                );
                continue;
            };
            let needs_collection_binding = task.collection_section_id > 0
                && (is_collection_bind_pending_error(&task.error_message)
                    || is_submission_created_error(&task.error_message)
                    || is_submit_result_unknown_error(&task.error_message));
            if needs_collection_binding {
                if let Err(err) = retry_pending_collection_binding(
                    &upload_context,
                    &submission_context,
                    &task.task_id,
                    &auth,
                    Some(info.aid),
                )
                .await
                {
                    let reason = build_collection_bind_pending_error(&err);
                    let _ = update_submission_status_with_reason(
                        &submission_context,
                        &task.task_id,
                        SUBMISSION_STATUS_VERIFY_PENDING,
                        Some(&reason),
                    );
                    append_log(
                        &context.app_log_path,
                        &format!(
                            "submission_collection_pending_retry_fail task_id={} err={}",
                            task.task_id, err
                        ),
                    );
                    continue;
                }
            }
            if info.part_count == 0 {
                append_log(
                    &context.app_log_path,
                    &format!(
                        "submission_remote_reconcile_pending task_id={} bvid={} reason=remote_part_count_missing",
                        task.task_id, info.bvid
                    ),
                );
                continue;
            }
            if info.part_count != task.expected_parts {
                reconciliation_failures.push(RemoteReconciliationFailure {
                    task_id: task.task_id.clone(),
                    bvid: info.bvid.clone(),
                    aid: if info.aid > 0 { info.aid } else { task.aid },
                    remote_state: info.state,
                    remote_state_desc: info.state_desc.clone(),
                    reject_reason: info.reject_reason.clone(),
                    reason: format!(
                        "远程分P数量与本地不一致（本地={}，远程={}）",
                        task.expected_parts, info.part_count
                    ),
                });
                continue;
            }
            reconciled.push(RemoteReconciliationUpdate {
                task_id: task.task_id,
                bvid: info.bvid.clone(),
                aid: if info.aid > 0 { info.aid } else { task.aid },
                remote_state: info.state,
                remote_state_desc: info.state_desc.clone(),
                reject_reason: info.reject_reason.clone(),
            });
        }
    }

    let now = now_rfc3339();
    context
        .db
        .with_conn_mut(|conn| {
            let tx = conn.transaction()?;
            for (task_id, state, state_desc, reject_reason) in updates {
                if let Some(reason) = reject_reason.as_deref() {
                    tx.execute(
                        "UPDATE submission_task SET remote_state = ?1, remote_state_desc = ?2, reject_reason = ?3 WHERE task_id = ?4",
                        (state, state_desc.as_deref(), reason, &task_id),
                    )?;
                } else {
                    tx.execute(
                        "UPDATE submission_task SET remote_state = ?1, remote_state_desc = ?2, reject_reason = NULL WHERE task_id = ?3",
                        (state, state_desc.as_deref(), &task_id),
                    )?;
                }
            }
            for item in &reconciled {
                tx.execute(
                    "UPDATE submission_task SET status = 'COMPLETED', bvid = ?1, aid = ?2, remote_state = ?3, remote_state_desc = ?4, reject_reason = ?5, priority = 0, priority_at = NULL, updated_at = ?6 WHERE task_id = ?7",
                    (
                        item.bvid.as_str(),
                        item.aid,
                        item.remote_state,
                        item.remote_state_desc.as_deref(),
                        item.reject_reason.as_deref(),
                        now.as_str(),
                        item.task_id.as_str(),
                    ),
                )?;
                tx.execute(
                    "UPDATE workflow_instances SET status = 'COMPLETED', current_step = NULL, progress = 100, error_message = NULL, completed_at = ?1, updated_at = ?1 WHERE task_id = ?2",
                    (now.as_str(), item.task_id.as_str()),
                )?;
            }
            for item in &reconciliation_failures {
                tx.execute(
                    "UPDATE submission_task SET status = 'FAILED', bvid = ?1, aid = ?2, remote_state = ?3, remote_state_desc = ?4, reject_reason = ?5, updated_at = ?6 WHERE task_id = ?7",
                    (
                        item.bvid.as_str(),
                        item.aid,
                        item.remote_state,
                        item.remote_state_desc.as_deref(),
                        item.reject_reason.as_deref(),
                        now.as_str(),
                        item.task_id.as_str(),
                    ),
                )?;
                tx.execute(
                    "UPDATE workflow_instances SET status = 'FAILED', progress = 0, error_message = ?1, completed_at = COALESCE(completed_at, ?2), updated_at = ?2 WHERE task_id = ?3",
                    (item.reason.as_str(), now.as_str(), item.task_id.as_str()),
                )?;
            }
            tx.commit()?;
            Ok(())
        })
        .map_err(|err| err.to_string())?;
    for item in reconciled {
        append_log(
            &context.app_log_path,
            &format!(
                "submission_remote_reconcile_completed task_id={} bvid={} aid={} remote_state={}",
                item.task_id, item.bvid, item.aid, item.remote_state
            ),
        );
    }
    for item in reconciliation_failures {
        append_log(
            &context.app_log_path,
            &format!(
                "submission_remote_reconcile_failed task_id={} reason={}",
                item.task_id, item.reason
            ),
        );
    }
    Ok(())
}

pub(super) fn load_remote_tasks_by_owner(
    context: &SubmissionQueueContext,
) -> Result<HashMap<Option<i64>, Vec<RemoteTrackedTask>>, String> {
    context
        .db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT st.task_id, COALESCE(st.bvid, ''), COALESCE(st.aid, 0), st.title, st.status, st.remote_state, COALESCE(st.remote_status_ignored, 0), st.created_at, st.updated_at, COALESCE((SELECT wi.error_message FROM workflow_instances wi WHERE wi.task_id = st.task_id ORDER BY wi.created_at DESC LIMIT 1), ''), (SELECT COUNT(*) FROM task_output_segment tos WHERE tos.task_id = st.task_id), st.bilibili_uid, COALESCE(st.collection_section_id, 0) FROM submission_task st WHERE st.status IN ('VERIFY_PENDING', 'SUBMIT_UNKNOWN', 'FAILED') OR (st.status = 'COMPLETED' AND COALESCE(TRIM(st.bvid), '') <> '' AND (st.remote_state IS NULL OR st.remote_state IN (-1, -30, -60, -2, -4)))",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    RemoteTrackedTask {
                        task_id: row.get(0)?,
                        bvid: row.get(1)?,
                        aid: row.get(2)?,
                        title: row.get(3)?,
                        status: row.get(4)?,
                        remote_state: row.get(5)?,
                        remote_status_ignored: row.get::<_, i64>(6)? != 0,
                        created_at: row.get(7)?,
                        reconciliation_at: row.get(8)?,
                        error_message: row.get(9)?,
                        expected_parts: row.get::<_, i64>(10)?.max(0) as usize,
                        collection_section_id: row.get(12)?,
                    },
                    row.get::<_, Option<i64>>(11)?,
                ))
            })?;
            let mut groups = HashMap::<Option<i64>, Vec<RemoteTrackedTask>>::new();
            for row in rows {
                let (task, owner_uid) = row?;
                if task.needs_remote_refresh() {
                    groups.entry(owner_uid).or_default().push(task);
                }
            }
            Ok(groups)
        })
        .map_err(|err| err.to_string())
}

pub(super) fn load_remote_refresh_auth(
    context: &SubmissionQueueContext,
    owner_uid: Option<i64>,
) -> Result<Option<AuthInfo>, String> {
    match owner_uid {
        Some(uid) => context
            .login_store
            .load_auth_info_by_uid(&context.db, uid)
            .map_err(|err| err.to_string()),
        None => load_auth_from_queue_context(context).map(Some),
    }
}

pub(super) async fn fetch_remote_audit_map(
    context: &SubmissionQueueContext,
    auth: &AuthInfo,
    published_since: Option<i64>,
) -> Result<HashMap<String, RemoteAuditInfo>, String> {
    let mut result =
        fetch_remote_audit_map_by_status(context, auth, REMOTE_AUDIT_STATUS_REJECTED, None).await?;
    let reviewing_map =
        fetch_remote_audit_map_by_status(context, auth, REMOTE_AUDIT_STATUS_REVIEWING, None)
            .await?;
    let rejected_count = result.len();
    let reviewing_count = reviewing_map.len();
    for (bvid, info) in reviewing_map {
        result.insert(bvid, info);
    }
    let mut published_count = 0usize;
    if let Some(min_ctime) = published_since {
        sleep(Duration::from_secs(REMOTE_PUBLISHED_PAGE_WAIT_SECS)).await;
        let published_map = fetch_remote_audit_map_by_status(
            context,
            auth,
            REMOTE_AUDIT_STATUS_PUBLISHED,
            Some(min_ctime),
        )
        .await?;
        published_count = published_map.len();
        for (bvid, info) in published_map {
            result.insert(bvid, info);
        }
    }
    append_log(
    &context.app_log_path,
    &format!(
      "submission_remote_fetch_merge_summary auth_uid={} rejected_items={} reviewing_items={} published_items={} merged_items={} published_since={}",
      upload_owner_label(auth.user_id),
      rejected_count,
      reviewing_count,
      published_count,
      result.len(),
      published_since.map(|value| value.to_string()).unwrap_or_else(|| "none".to_string())
    ),
  );
    Ok(result)
}

pub(super) async fn fetch_remote_audit_map_by_status(
    context: &SubmissionQueueContext,
    auth: &AuthInfo,
    status: &str,
    min_ctime: Option<i64>,
) -> Result<HashMap<String, RemoteAuditInfo>, String> {
    let mut page = 1_i64;
    let page_size = 20_i64;
    let mut result = HashMap::new();

    loop {
        let params = vec![
            ("status".to_string(), status.to_string()),
            ("pn".to_string(), page.to_string()),
            ("ps".to_string(), page_size.to_string()),
            ("coop".to_string(), "1".to_string()),
            ("interactive".to_string(), "1".to_string()),
        ];
        let query = build_query_params(&params);
        append_log(
      &context.app_log_path,
      &format!(
        "submission_remote_fetch_request auth_uid={} status={} url=https://member.bilibili.com/x/web/archives?{}",
        upload_owner_label(auth.user_id),
        status,
        query
      ),
    );
        let data = context
            .bilibili
            .get_json(
                "https://member.bilibili.com/x/web/archives",
                &params,
                Some(auth),
                false,
            )
            .await?;
        append_log(
            &context.app_log_path,
            &format!(
                "submission_remote_fetch_response auth_uid={} status={} page={} data={}",
                upload_owner_label(auth.user_id),
                status,
                page,
                truncate_log_value(&data)
            ),
        );
        let arc_audits = data
            .get("arc_audits")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let mut page_ctimes = Vec::with_capacity(arc_audits.len());
        for item in arc_audits.iter() {
            let Some(info) = parse_remote_audit_info(item) else {
                continue;
            };
            page_ctimes.push(info.ctime);
            if min_ctime.map(|cutoff| info.ctime >= cutoff).unwrap_or(true) {
                result.insert(info.bvid.clone(), info);
            }
        }

        let reached_older_page = min_ctime
            .map(|cutoff| {
                !page_ctimes.is_empty() && page_ctimes.iter().all(|ctime| *ctime < cutoff)
            })
            .unwrap_or(false);

        let total_count = data
            .get("page")
            .and_then(|value| value.get("count"))
            .and_then(|value| value.as_i64())
            .unwrap_or(0);
        if total_count <= 0 {
            break;
        }
        if page * page_size >= total_count {
            break;
        }
        if arc_audits.is_empty() {
            break;
        }
        if reached_older_page {
            append_log(
                &context.app_log_path,
                &format!(
                    "submission_remote_fetch_cutoff auth_uid={} status={} page={} min_ctime={}",
                    upload_owner_label(auth.user_id),
                    status,
                    page,
                    min_ctime.unwrap_or_default()
                ),
            );
            break;
        }
        page += 1;
        sleep(Duration::from_secs(REMOTE_PUBLISHED_PAGE_WAIT_SECS)).await;
    }

    append_log(
        &context.app_log_path,
        &format!(
            "submission_remote_fetch_status_summary auth_uid={} status={} items={}",
            upload_owner_label(auth.user_id),
            status,
            result.len()
        ),
    );
    Ok(result)
}

pub(super) async fn add_video_to_collection_with_refresh(
    context: &UploadContext,
    auth: &AuthInfo,
    title: &str,
    season_id: i64,
    section_id: Option<i64>,
    aid: i64,
    cid: i64,
    csrf: &str,
) -> Result<(), String> {
    match add_video_to_collection(context, auth, title, season_id, section_id, aid, cid, csrf).await
    {
        Ok(()) => Ok(()),
        Err(err) => {
            if !is_auth_error(&err) {
                return Err(err);
            }
            let auth = refresh_auth(context, "add_video_collection").await?;
            let csrf = auth
                .csrf
                .clone()
                .ok_or_else(|| "登录信息缺少CSRF".to_string())?;
            add_video_to_collection(
                context, &auth, title, season_id, section_id, aid, cid, &csrf,
            )
            .await
        }
    }
}

pub(super) async fn add_video_to_collection_with_retry(
    context: &UploadContext,
    auth: &AuthInfo,
    task_id: &str,
    title: &str,
    season_id: i64,
    section_id: Option<i64>,
    aid: i64,
    cid: i64,
    csrf: &str,
) -> Result<(), String> {
    let mut attempt: u32 = 1;
    loop {
        match add_video_to_collection_with_refresh(
            context, auth, title, season_id, section_id, aid, cid, csrf,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(err) => {
                if section_id.unwrap_or(0) <= 0 && is_collection_not_found_error(&err) {
                    append_log(
                        &context.app_log_path,
                        &format!(
                            "submission_collection_skip task_id={} collection_id={} err={}",
                            task_id, season_id, err
                        ),
                    );
                    return Ok(());
                }
                if attempt >= COLLECTION_BIND_RETRY_LIMIT {
                    append_log(
            &context.app_log_path,
            &format!(
              "submission_collection_retry_exhausted task_id={} collection_id={} attempts={} err={}",
              task_id, season_id, attempt, err
            ),
          );
                    return Err(err);
                }
                let wait_secs = collection_bind_retry_delay_secs(attempt);
                append_log(
          &context.app_log_path,
          &format!(
            "submission_collection_retry_wait task_id={} collection_id={} round={} wait_secs={} err={}",
            task_id, season_id, attempt, wait_secs, err
          ),
        );
                sleep(Duration::from_secs(wait_secs)).await;
                attempt = attempt.saturating_add(1);
            }
        }
    }
}

pub(super) async fn retry_pending_collection_binding(
    context: &UploadContext,
    submission_context: &SubmissionContext,
    task_id: &str,
    auth: &AuthInfo,
    aid_override: Option<i64>,
) -> Result<(), String> {
    let detail = load_task_detail(submission_context, task_id)?;
    let season_id = detail.task.collection_id.unwrap_or(0);
    let section_id = detail.task.collection_section_id.unwrap_or(0);
    let aid = aid_override
        .filter(|value| *value > 0)
        .or(detail.task.aid)
        .unwrap_or(0);
    let cid = detail
        .output_segments
        .first()
        .and_then(|segment| segment.cid)
        .unwrap_or(0);
    if season_id <= 0 || section_id <= 0 {
        return Err("合集绑定待重试任务缺少父合集或子合集ID".to_string());
    }
    if aid <= 0 || cid <= 0 {
        return Err("合集绑定待重试任务缺少AID或CID".to_string());
    }
    let csrf = auth
        .csrf
        .as_deref()
        .ok_or_else(|| "登录信息缺少CSRF".to_string())?;
    append_log(
        &context.app_log_path,
        &format!(
            "submission_collection_pending_retry_start task_id={} collection_id={} section_id={} aid={} cid={}",
            task_id, season_id, section_id, aid, cid
        ),
    );
    add_video_to_collection_with_retry(
        context,
        auth,
        task_id,
        &detail.task.title,
        season_id,
        Some(section_id),
        aid,
        cid,
        csrf,
    )
    .await?;
    update_submission_status_with_reason(
        submission_context,
        task_id,
        SUBMISSION_STATUS_VERIFY_PENDING,
        Some("所选子合集绑定成功，等待远端稿件校验"),
    )?;
    append_log(
        &context.app_log_path,
        &format!(
            "submission_collection_pending_retry_ok task_id={} collection_id={} section_id={} aid={}",
            task_id, season_id, section_id, aid
        ),
    );
    Ok(())
}

pub(super) async fn switch_video_collection_with_refresh(
    context: &UploadContext,
    auth: &AuthInfo,
    title: &str,
    season_id: i64,
    section_id: Option<i64>,
    aid: i64,
    csrf: &str,
) -> Result<(), String> {
    match switch_video_collection(context, auth, title, season_id, section_id, aid, csrf).await {
        Ok(()) => Ok(()),
        Err(err) => {
            if !is_auth_error(&err) {
                return Err(err);
            }
            let auth = refresh_auth(context, "switch_video_collection").await?;
            let csrf = auth
                .csrf
                .clone()
                .ok_or_else(|| "登录信息缺少CSRF".to_string())?;
            switch_video_collection(context, &auth, title, season_id, section_id, aid, &csrf).await
        }
    }
}

pub(super) async fn add_video_to_collection(
    context: &UploadContext,
    auth: &AuthInfo,
    title: &str,
    season_id: i64,
    requested_section_id: Option<i64>,
    aid: i64,
    cid: i64,
    csrf: &str,
) -> Result<(), String> {
    if aid <= 0 || cid <= 0 {
        return Err("合集绑定缺少AID或CID".to_string());
    }
    let section_id =
        resolve_collection_section_id(context, auth, season_id, requested_section_id).await?;
    append_log(
        &context.app_log_path,
        &format!(
            "submission_collection_start season_id={} section_id={} aid={} cid={}",
            season_id, section_id, aid, cid
        ),
    );

    let url = "https://member.bilibili.com/x2/creative/web/season/section/episodes/add";
    let params = vec![("csrf".to_string(), csrf.to_string())];
    let payload = serde_json::json!({
      "sectionId": section_id,
      "episodes": [
        {
          "title": title,
          "aid": aid,
          "cid": cid,
          "charging_pay": 0
        }
      ]
    });

    let _ = context
        .bilibili
        .post_json(url, &params, &payload, Some(auth))
        .await?;

    append_log(
        &context.app_log_path,
        &format!(
            "submission_collection_ok season_id={} section_id={} aid={}",
            season_id, section_id, aid
        ),
    );
    Ok(())
}

pub(super) fn is_collection_not_found_error(err: &str) -> bool {
    err.contains("code: -404") || err.contains("啥都木有")
}

pub(super) async fn switch_video_collection(
    context: &UploadContext,
    auth: &AuthInfo,
    title: &str,
    season_id: i64,
    requested_section_id: Option<i64>,
    aid: i64,
    csrf: &str,
) -> Result<(), String> {
    if season_id <= 0 || aid <= 0 {
        return Err("合集切换缺少season_id或aid".to_string());
    }
    let section_id =
        resolve_collection_section_id(context, auth, season_id, requested_section_id).await?;
    append_log(
        &context.app_log_path,
        &format!(
            "submission_collection_switch_start season_id={} section_id={} aid={}",
            season_id, section_id, aid
        ),
    );
    let url = "https://member.bilibili.com/x2/creative/web/season/switch";
    let params = vec![("csrf".to_string(), csrf.to_string())];
    let payload = serde_json::json!({
      "season_id": season_id,
      "section_id": section_id,
      "title": title,
      "aid": aid,
      "csrf": csrf
    });
    let _ = context
        .bilibili
        .post_json(url, &params, &payload, Some(auth))
        .await?;
    append_log(
        &context.app_log_path,
        &format!(
            "submission_collection_switch_ok season_id={} section_id={} aid={}",
            season_id, section_id, aid
        ),
    );
    Ok(())
}

pub(super) async fn resolve_collection_section_id(
    context: &UploadContext,
    auth: &AuthInfo,
    season_id: i64,
    requested_section_id: Option<i64>,
) -> Result<i64, String> {
    let url = "https://member.bilibili.com/x2/creative/web/seasons";
    let params = vec![
        ("pn".to_string(), "1".to_string()),
        ("ps".to_string(), "100".to_string()),
        ("order".to_string(), "desc".to_string()),
        ("sort".to_string(), "mtime".to_string()),
        ("filter".to_string(), "1".to_string()),
    ];
    let data = context
        .bilibili
        .get_json(url, &params, Some(auth), false)
        .await?;
    let seasons = data
        .get("seasons")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "合集响应缺少列表".to_string())?;
    for item in seasons {
        let Some(season) = item.get("season") else {
            continue;
        };
        let Some(id) = season.get("id").and_then(|value| value.as_i64()) else {
            continue;
        };
        if id != season_id {
            continue;
        }
        let sections = item
            .get("sections")
            .and_then(|value| value.get("sections"))
            .and_then(|value| value.as_array())
            .ok_or_else(|| format!("合集{}缺少子合集列表", season_id))?;
        if let Some(requested_section_id) = requested_section_id.filter(|value| *value > 0) {
            let exists = sections.iter().any(|section| {
                section.get("id").and_then(|value| value.as_i64()) == Some(requested_section_id)
            });
            if exists {
                return Ok(requested_section_id);
            }
            return Err(format!(
                "所选子合集{}不属于父合集{}，请刷新合集后重新选择",
                requested_section_id, season_id
            ));
        }
        return sections
            .iter()
            .filter_map(|section| section.get("id").and_then(|value| value.as_i64()))
            .find(|section_id| *section_id > 0)
            .ok_or_else(|| format!("合集{}缺少可用子合集", season_id));
    }
    Err(format!("未找到合集{}，请刷新合集后重新选择", season_id))
}

pub(super) fn normalize_source_remote_binding(
    source: &SourceVideoInput,
) -> (
    Option<String>,
    Option<i64>,
    Option<i64>,
    Option<String>,
    Option<String>,
) {
    let remote_bvid = source
        .remote_bvid
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());
    let remote_video_url = source
        .remote_video_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
        .or_else(|| {
            if remote_bvid.is_some() {
                Some(format!(
                    "https://www.bilibili.com/video/{}",
                    remote_bvid.as_deref().unwrap_or("")
                ))
            } else {
                None
            }
        });
    let remote_aid = if remote_bvid.is_some() {
        source.remote_aid.filter(|value| *value > 0)
    } else {
        None
    };
    let remote_cid = if remote_bvid.is_some() {
        source.remote_cid.filter(|value| *value > 0)
    } else {
        None
    };
    let remote_part_title = if remote_bvid.is_some() {
        source
            .remote_part_title
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_string())
    } else {
        None
    };
    (
        remote_bvid,
        remote_aid,
        remote_cid,
        remote_part_title,
        remote_video_url,
    )
}

pub(super) fn verify_generated_media_output(
    context: &SubmissionContext,
    task_id: &str,
    path: &Path,
    stage: &str,
    group: Option<usize>,
) -> Result<f64, String> {
    let mut last_err = None;
    let group_suffix = group
        .map(|value| format!(" group={}", value))
        .unwrap_or_default();
    append_log(
        &context.app_log_path,
        &format!(
            "{}_start task_id={} path={}{}",
            stage,
            task_id,
            path.to_string_lossy(),
            group_suffix
        ),
    );
    for attempt in 1..=5 {
        let result = match fs::metadata(path) {
            Ok(metadata) if !metadata.is_file() => {
                Err(format!("path_is_not_file path={}", path.to_string_lossy()))
            }
            Ok(metadata) if metadata.len() == 0 => {
                Err(format!("path_is_empty path={}", path.to_string_lossy()))
            }
            Ok(metadata) => probe_duration_seconds(path)
                .map(|duration| (metadata.len(), duration))
                .map_err(|err| {
                    format!("ffprobe_failed path={} err={}", path.to_string_lossy(), err)
                }),
            Err(err) => Err(format!(
                "metadata_failed path={} err={}",
                path.to_string_lossy(),
                err
            )),
        };
        match result {
            Ok((size, duration)) => {
                append_log(
                    &context.app_log_path,
                    &format!(
                        "{}_ok task_id={} path={} size={} duration={:.3} attempt={}{}",
                        stage,
                        task_id,
                        path.to_string_lossy(),
                        size,
                        duration,
                        attempt,
                        group_suffix
                    ),
                );
                return Ok(duration);
            }
            Err(err) => {
                last_err = Some(err);
                if attempt < 5 {
                    std::thread::sleep(Duration::from_millis(300));
                }
            }
        }
    }
    let err = last_err
        .unwrap_or_else(|| format!("unknown_media_verify_error path={}", path.to_string_lossy()));
    append_log(
        &context.app_log_path,
        &format!(
            "{}_fail task_id={} path={} err={}{}",
            stage,
            task_id,
            path.to_string_lossy(),
            err,
            group_suffix
        ),
    );
    Err(err)
}

pub(super) fn build_merge_output_path_with_remote_name(
    workflow_dir: &Path,
    remote_name: Option<&str>,
    fallback_task_id: &str,
) -> PathBuf {
    if let Some(remote_name) = normalize_baidu_sync_filename(remote_name) {
        let file_name = sanitize_filename(&remote_name);
        if !file_name.trim().is_empty() {
            return workflow_dir.join("merge").join(file_name);
        }
    }
    build_merge_output_path(workflow_dir, fallback_task_id)
}
