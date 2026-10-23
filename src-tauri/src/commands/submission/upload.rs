//! 视频分片上传、封面上传与稿件提交。

use super::*;

pub(super) fn with_edit_upload_state<T>(
    context: &SubmissionContext,
    action: impl FnOnce(&mut EditUploadState) -> T,
) -> Result<T, String> {
    let mut guard = context
        .edit_upload_state
        .lock()
        .map_err(|_| "编辑上传状态不可用".to_string())?;
    Ok(action(&mut guard))
}

pub(super) fn upsert_edit_upload_segment(
    context: &SubmissionContext,
    segment: TaskOutputSegmentRecord,
) -> Result<TaskOutputSegmentRecord, String> {
    with_edit_upload_state(context, |state| {
        state
            .segments
            .insert(segment.segment_id.clone(), segment.clone());
        crate::change_events::mark_edit_upload();
        segment
    })
}

pub(super) fn load_edit_upload_segment(
    context: &SubmissionContext,
    segment_id: &str,
) -> Result<Option<TaskOutputSegmentRecord>, String> {
    with_edit_upload_state(context, |state| state.segments.get(segment_id).cloned())
}

pub(super) fn update_edit_upload_segment(
    context: &SubmissionContext,
    segment_id: &str,
    updater: impl FnOnce(&mut TaskOutputSegmentRecord),
) -> Result<(), String> {
    with_edit_upload_state(context, |state| {
        if let Some(segment) = state.segments.get_mut(segment_id) {
            updater(segment);
            crate::change_events::mark_edit_upload();
            return true;
        }
        false
    })?
    .then_some(())
    .ok_or_else(|| "未找到编辑分P".to_string())
}

pub(super) fn list_edit_upload_segments_by_task(
    context: &SubmissionContext,
    task_id: &str,
    segment_ids: Option<&[String]>,
) -> Result<Vec<TaskOutputSegmentRecord>, String> {
    with_edit_upload_state(context, |state| {
        let filter_ids = segment_ids.map(|ids| ids.iter().cloned().collect::<HashSet<_>>());
        state
            .segments
            .values()
            .filter(|segment| segment.task_id == task_id)
            .filter(|segment| match &filter_ids {
                Some(ids) => ids.contains(&segment.segment_id),
                None => true,
            })
            .cloned()
            .collect::<Vec<_>>()
    })
}

pub(super) fn clear_edit_upload_segments_by_task(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<(), String> {
    with_edit_upload_state(context, |state| {
        state
            .segments
            .retain(|_, segment| segment.task_id != task_id);
        crate::change_events::mark_edit_upload();
    })
}

#[tauri::command]
pub async fn submission_upload_cover(
    state: State<'_, AppState>,
    request: SubmissionUploadCoverRequest,
) -> Result<ApiResponse<String>, String> {
    let image_data_url = request.image_data_url.trim().to_string();
    if image_data_url.is_empty() {
        return Ok(ApiResponse::error("封面数据不能为空"));
    }
    let upload_context = UploadContext::new(&state);
    let mut auth = match load_auth_or_refresh(&upload_context, "submission_upload_cover").await {
        Ok(auth) => auth,
        Err(err) => return Ok(ApiResponse::error(err)),
    };
    let csrf = match auth.csrf.clone() {
        Some(value) => value,
        None => {
            auth = match refresh_auth(&upload_context, "submission_upload_cover_csrf").await {
                Ok(auth) => auth,
                Err(err) => return Ok(ApiResponse::error(err)),
            };
            match auth.csrf.clone() {
                Some(value) => value,
                None => return Ok(ApiResponse::error("登录信息缺少CSRF")),
            }
        }
    };
    let cover_url =
        match upload_cover_with_refresh(&upload_context, &auth, &csrf, &image_data_url).await {
            Ok(url) => url,
            Err(err) => return Ok(ApiResponse::error(err)),
        };
    Ok(ApiResponse::success(cover_url))
}

#[tauri::command(async)]
pub fn submission_cover_local_preview(
    request: SubmissionCoverLocalPreviewRequest,
) -> Result<ApiResponse<String>, String> {
    let raw_path = request.file_path.trim();
    if raw_path.is_empty() {
        return Ok(ApiResponse::error("封面文件路径不能为空"));
    }

    let path = PathBuf::from(raw_path);
    let bytes = match fs::read(&path) {
        Ok(data) => data,
        Err(err) => return Ok(ApiResponse::error(format!("读取封面文件失败: {}", err))),
    };
    if bytes.is_empty() {
        return Ok(ApiResponse::error("封面文件为空"));
    }

    let mime = match detect_image_mime(&bytes, &path) {
        Some(value) => value,
        None => return Ok(ApiResponse::error("不支持的封面格式，请选择 JPG/PNG/WEBP")),
    };

    let encoded = STANDARD.encode(bytes);
    let data_url = format!("data:{};base64,{}", mime, encoded);
    Ok(ApiResponse::success(data_url))
}

#[tauri::command(async)]
pub fn submission_edit_upload_status(
    state: State<'_, AppState>,
    request: SubmissionEditUploadStatusRequest,
) -> Result<ApiResponse<Vec<TaskOutputSegmentRecord>>, String> {
    let context = SubmissionContext::new(&state);
    let task_id = request.task_id.trim();
    if task_id.is_empty() {
        return Ok(ApiResponse::error("任务ID不能为空"));
    }
    let segment_ids = request.segment_ids.unwrap_or_default();
    let segments = if segment_ids.is_empty() {
        list_edit_upload_segments_by_task(&context, task_id, None)
    } else {
        list_edit_upload_segments_by_task(&context, task_id, Some(&segment_ids))
    };
    let segments = match segments {
        Ok(segments) => segments,
        Err(err) => return Ok(ApiResponse::error(err)),
    };
    Ok(ApiResponse::success(segments))
}

#[tauri::command(async)]
pub fn submission_edit_upload_clear(
    state: State<'_, AppState>,
    request: SubmissionEditUploadClearRequest,
) -> Result<ApiResponse<String>, String> {
    let context = SubmissionContext::new(&state);
    let task_id = request.task_id.trim();
    if task_id.is_empty() {
        return Ok(ApiResponse::error("任务ID不能为空"));
    }
    clear_edit_upload_segments_by_task(&context, task_id)?;
    Ok(ApiResponse::success("OK".to_string()))
}

pub(super) fn should_archive_deleted_merged_path(base_dir: &Path, local_path: &Path) -> bool {
    local_path.starts_with(base_dir.join("merge"))
        || local_path.starts_with(base_dir.join("updates"))
        || local_path.starts_with(base_dir.join("runs"))
}

pub(super) fn archive_merged_local_file(
    log_path: &PathBuf,
    base_dir: &Path,
    source_path: &Path,
) -> Result<String, String> {
    let file_name = source_path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "合并视频文件名无效".to_string())?;
    let archive_dir = base_dir.join("archived_merged");
    fs::create_dir_all(&archive_dir).map_err(|err| format!("创建归档目录失败: {}", err))?;
    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("merged");
    let ext = Path::new(file_name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("mp4");

    let mut target_path = archive_dir.join(file_name);
    if target_path.exists() {
        let stamp = sanitize_filename(&now_rfc3339());
        target_path = archive_dir.join(format!("{}_{}.{}", stem, stamp, ext));
    }
    let mut duplicate_index = 1usize;
    while target_path.exists() {
        target_path = archive_dir.join(format!(
            "{}_{}_{}.{}",
            stem,
            sanitize_filename(&now_rfc3339()),
            duplicate_index,
            ext
        ));
        duplicate_index = duplicate_index.saturating_add(1);
    }

    append_log(
        log_path,
        &format!(
            "submission_merged_archive_start from={} to={}",
            source_path.to_string_lossy(),
            target_path.to_string_lossy()
        ),
    );

    match fs::rename(source_path, &target_path) {
        Ok(()) => {}
        Err(_) => {
            fs::copy(source_path, &target_path)
                .map_err(|err| format!("归档合并视频失败: {}", err))?;
            fs::remove_file(source_path).map_err(|err| format!("清理原始合并视频失败: {}", err))?;
        }
    }

    append_log(
        log_path,
        &format!(
            "submission_merged_archive_ok from={} to={}",
            source_path.to_string_lossy(),
            target_path.to_string_lossy()
        ),
    );
    Ok(target_path.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn submission_upload_execute(
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
    append_log(
        &state.app_log_path,
        &format!(
            "submission_upload_request task_id={} status={}",
            task_id, status
        ),
    );
    if status == "UPLOADING" {
        append_log(
            &state.app_log_path,
            &format!(
                "submission_upload_reject task_id={} reason=uploading",
                task_id
            ),
        );
        return Ok(ApiResponse::error("任务正在投稿中"));
    }
    if !supports_direct_upload_enqueue(&status) {
        append_log(
            &state.app_log_path,
            &format!(
                "submission_upload_reject task_id={} reason=invalid_status status={}",
                task_id, status
            ),
        );
        return Ok(ApiResponse::error("任务状态不支持投稿"));
    }
    let detail = load_task_detail(&context, &task_id)?;
    if should_block_submission_retry(&detail.task) {
        return Ok(ApiResponse::error(submission_retry_block_message()));
    }
    let confirmed_remote_missing = is_confirmed_remote_missing_retry(
        &status,
        detail
            .task
            .workflow_status
            .as_ref()
            .and_then(|workflow| workflow.error_message.as_deref()),
    );

    if let Err(err) = update_submission_status(&context, &task_id, "WAITING_UPLOAD") {
        return Ok(ApiResponse::error(format!("提交到投稿队列失败: {}", err)));
    }
    if confirmed_remote_missing {
        if let Err(err) = update_workflow_status(
            &context,
            &task_id,
            SUBMISSION_STATUS_REUPLOAD_PENDING,
            None,
            0.0,
        ) {
            return Ok(ApiResponse::error(format!("更新工作流状态失败: {}", err)));
        }
    }

    Ok(ApiResponse::success("投稿任务已加入队列".to_string()))
}

#[tauri::command]
pub async fn submission_retry_segment_upload(
    state: State<'_, AppState>,
    segment_id: String,
) -> Result<ApiResponse<String>, String> {
    let context = SubmissionContext::new(&state);
    let segment_id = segment_id.trim().to_string();
    if segment_id.is_empty() {
        return Ok(ApiResponse::error("分段ID不能为空"));
    }
    let segment = match load_output_segment_by_id(&context, &segment_id) {
        Ok(Some(segment)) => segment,
        Ok(None) => return Ok(ApiResponse::error("未找到分段信息")),
        Err(err) => return Ok(ApiResponse::error(err)),
    };
    let status = match load_task_status(&context, &segment.task_id) {
        Ok(status) => status,
        Err(err) => return Ok(ApiResponse::error(format!("读取任务状态失败: {}", err))),
    };
    if status == "UPLOADING" {
        return Ok(ApiResponse::error("任务正在投稿中，请稍后重试"));
    }
    if status == "WAITING_UPLOAD" {
        return Ok(ApiResponse::success("任务已在投稿队列中".to_string()));
    }
    if status == "COMPLETED" {
        return Ok(ApiResponse::error("任务已完成，无需重试分段上传"));
    }
    if segment.upload_status == "SUCCESS" {
        return Ok(ApiResponse::error("该分段已上传成功，无需重试"));
    }

    if let Err(err) = reset_failed_segments_to_pending(&context, &segment.task_id) {
        return Ok(ApiResponse::error(format!("重置失败分段失败: {}", err)));
    }
    let queued = match enqueue_submission_after_segment_retry(&context, &segment.task_id) {
        Ok(value) => value,
        Err(err) => return Ok(ApiResponse::error(format!("加入投稿队列失败: {}", err))),
    };
    if !queued {
        return Ok(ApiResponse::error("任务当前状态不支持重试分段上传"));
    }
    Ok(ApiResponse::success(
        "失败分段已重置，任务已加入投稿队列".to_string(),
    ))
}

pub(super) fn update_submission_cover_url(
    context: &SubmissionContext,
    task_id: &str,
    cover_url: Option<&str>,
) -> Result<(), String> {
    context
        .db
        .with_conn(|conn| {
            conn.execute(
                "UPDATE submission_task SET cover_url = ?1 WHERE task_id = ?2",
                (cover_url, task_id),
            )?;
            Ok(())
        })
        .map_err(|err| err.to_string())
}

pub(super) fn update_submission_cover_local_path(
    context: &SubmissionContext,
    task_id: &str,
    cover_local_path: Option<&str>,
) -> Result<(), String> {
    context
        .db
        .with_conn(|conn| {
            conn.execute(
                "UPDATE submission_task SET cover_local_path = ?1 WHERE task_id = ?2",
                (cover_local_path, task_id),
            )?;
            Ok(())
        })
        .map_err(|err| err.to_string())
}

pub(super) async fn fetch_cover_url_by_bvid_once(
    context: &UploadContext,
    auth: Option<&AuthInfo>,
    bvid: &str,
) -> Result<Option<String>, String> {
    let url = format!("{}/x/web-interface/view", context.bilibili.base_url());
    let params = vec![("bvid".to_string(), bvid.to_string())];
    let data = context
        .bilibili
        .get_json(&url, &params, auth, false)
        .await?;
    Ok(data
        .get("pic")
        .and_then(|value| value.as_str())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty()))
}

pub(super) async fn fetch_cover_url_by_bvid(
    context: &UploadContext,
    auth: Option<&AuthInfo>,
    bvid: &str,
) -> Option<String> {
    let trimmed = bvid.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(current_auth) = auth {
        match fetch_cover_url_by_bvid_once(context, Some(current_auth), trimmed).await {
            Ok(Some(url)) => return Some(url),
            Ok(None) => {}
            Err(err) => {
                append_log(
                    &context.app_log_path,
                    &format!(
                        "submission_cover_fetch_fail mode=auth bvid={} err={}",
                        trimmed, err
                    ),
                );
                if is_auth_error(&err) {
                    match refresh_auth(context, "fetch_cover_by_bvid").await {
                        Ok(refreshed) => {
                            match fetch_cover_url_by_bvid_once(context, Some(&refreshed), trimmed)
                                .await
                            {
                                Ok(Some(url)) => return Some(url),
                                Ok(None) => {}
                                Err(refresh_err) => {
                                    append_log(
                                        &context.app_log_path,
                                        &format!(
                      "submission_cover_fetch_fail mode=refreshed_auth bvid={} err={}",
                      trimmed, refresh_err
                    ),
                                    );
                                }
                            }
                        }
                        Err(refresh_err) => {
                            append_log(
                                &context.app_log_path,
                                &format!(
                                    "submission_cover_fetch_refresh_fail bvid={} err={}",
                                    trimmed, refresh_err
                                ),
                            );
                        }
                    }
                }
            }
        }
    }

    match fetch_cover_url_by_bvid_once(context, None, trimmed).await {
        Ok(Some(url)) => Some(url),
        Ok(None) => None,
        Err(err) => {
            append_log(
                &context.app_log_path,
                &format!(
                    "submission_cover_fetch_fail mode=anonymous bvid={} err={}",
                    trimmed, err
                ),
            );
            None
        }
    }
}

pub(super) async fn hydrate_task_cover_for_display(
    context: &SubmissionContext,
    upload_context: &UploadContext,
    task: &mut SubmissionTaskRecord,
) {
    let raw_cover_local_path = normalize_optional_text(task.cover_local_path.clone());
    let runtime_cover_local_path =
        resolve_runtime_cover_local_path(context, raw_cover_local_path.clone());
    if raw_cover_local_path.is_some() && runtime_cover_local_path.is_none() {
        let _ = update_submission_cover_local_path(context, &task.task_id, None);
    }
    task.cover_local_path = runtime_cover_local_path;

    let normalized_cover_url = normalize_optional_text(task.cover_url.clone());
    if normalized_cover_url.is_some() {
        task.cover_url = normalized_cover_url;
        return;
    }
    if task.cover_local_path.is_some() {
        task.cover_url = None;
        return;
    }

    let Some(bvid) = normalize_optional_text(task.bvid.clone()) else {
        task.cover_url = None;
        return;
    };

    append_log(
        &context.app_log_path,
        &format!(
            "submission_cover_hydrate_fetch task_id={} bvid={} local={} remote={}",
            task.task_id,
            bvid,
            if task.cover_local_path.is_some() {
                1
            } else {
                0
            },
            if normalized_cover_url.is_some() { 1 } else { 0 }
        ),
    );
    let auth = upload_context
        .login_store
        .load_auth_info(&upload_context.db)
        .ok()
        .flatten();
    if let Some(url) = fetch_cover_url_by_bvid(upload_context, auth.as_ref(), &bvid).await {
        let _ = update_submission_cover_url(context, &task.task_id, Some(url.as_str()));
        append_log(
            &context.app_log_path,
            &format!(
                "submission_cover_hydrate_ok task_id={} bvid={} cover={}",
                task.task_id, bvid, url
            ),
        );
        task.cover_url = Some(url);
    } else {
        append_log(
            &context.app_log_path,
            &format!(
                "submission_cover_hydrate_empty task_id={} bvid={}",
                task.task_id, bvid
            ),
        );
        task.cover_url = None;
    }
}

pub(super) fn load_task_cover_local_paths(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<Vec<String>, String> {
    let stored_paths = context
        .db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT cover_local_path FROM submission_task WHERE task_id = ?1 LIMIT 1",
            )?;
            let path: Option<String> = stmt
                .query_row([task_id], |row| row.get::<_, Option<String>>(0))
                .optional()?
                .flatten();
            Ok(path.into_iter().collect::<Vec<_>>())
        })
        .map_err(|err| err.to_string())?;

    let mut result = Vec::new();
    for stored in stored_paths {
        let runtime = to_runtime_submission_path(context, &stored);
        if runtime.trim().is_empty() {
            continue;
        }
        result.push(runtime);
    }
    Ok(result)
}

pub(super) fn clear_missing_task_cover_local_path(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<(), String> {
    let stored_path = context
        .db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT cover_local_path FROM submission_task WHERE task_id = ?1 LIMIT 1",
            )?;
            let value: Option<String> = stmt
                .query_row([task_id], |row| row.get::<_, Option<String>>(0))
                .optional()?
                .flatten();
            Ok(value)
        })
        .map_err(|err| err.to_string())?;

    let Some(stored_path) = normalize_optional_text(stored_path) else {
        return Ok(());
    };
    let runtime_path = to_runtime_submission_path(context, &stored_path);
    if runtime_path.trim().is_empty() {
        update_submission_cover_local_path(context, task_id, None)?;
        return Ok(());
    }
    if !path_exists(Path::new(&runtime_path)) {
        update_submission_cover_local_path(context, task_id, None)?;
    }
    Ok(())
}

pub(super) fn is_upload_rate_limit_response(status: StatusCode, body: &str) -> bool {
    if status == StatusCode::NOT_ACCEPTABLE || status == StatusCode::TOO_MANY_REQUESTS {
        return true;
    }
    let lower = body.to_lowercase();
    is_rate_limit_error(body)
        || lower.contains("rate limit")
        || lower.contains("too many")
        || lower.contains("frequent")
        || body.contains("限流")
        || body.contains("频繁")
}

pub(super) fn is_upload_auth_response(status: StatusCode, body: &str) -> bool {
    status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN || is_auth_error(body)
}

pub(super) fn is_upload_session_response(body: &str) -> bool {
    let lower = body.to_lowercase();
    lower.contains("uploadid")
        || lower.contains("upload_id")
        || lower.contains("upload id")
        || lower.contains("upload session")
        || lower.contains("session")
        || lower.contains("expired")
        || lower.contains("invalid")
        || lower.contains("not exist")
        || lower.contains("no such upload")
        || lower.contains("biz_id")
        || body.contains("会话")
        || body.contains("过期")
        || body.contains("不存在")
        || body.contains("无效")
}

pub(super) fn should_reset_upload_session_for_response(status: StatusCode, body: &str) -> bool {
    if is_upload_session_response(body) {
        return true;
    }
    !(status.is_server_error()
        || status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS)
}

pub(super) fn is_upload_session_reset_required(err: &str) -> bool {
    err.contains(UPLOAD_SESSION_RESET_REQUIRED_PREFIX)
}

pub(super) fn is_upload_transport_error(err: &str) -> bool {
    if !err.contains("上传分片失败:") {
        return false;
    }
    let lower = err.to_lowercase();
    [
        "error sending request",
        "connection reset",
        "connection closed",
        "unexpected eof",
        "timed out",
        "timeout",
        "broken pipe",
    ]
    .iter()
    .any(|keyword| lower.contains(keyword))
}

pub(super) fn effective_upload_chunk_size(remote_chunk_size: u64) -> Option<u64> {
    if remote_chunk_size == 0 {
        return None;
    }
    Some(remote_chunk_size.min(UPLOAD_MAX_CHUNK_SIZE_BYTES))
}

#[cfg(target_os = "macos")]
pub(super) fn detect_submission_upload_local_ipv4() -> Option<std::net::IpAddr> {
    let socket = std::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket
        .connect((std::net::Ipv4Addr::new(1, 1, 1, 1), 80))
        .ok()?;
    match socket.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(address) if !address.is_unspecified() => {
            Some(std::net::IpAddr::V4(address))
        }
        _ => None,
    }
}

#[cfg(not(target_os = "macos"))]
pub(super) fn detect_submission_upload_local_ipv4() -> Option<std::net::IpAddr> {
    None
}

pub(super) fn build_submission_upload_client(log_path: &Path) -> Client {
    let mut builder = Client::builder()
        .no_proxy()
        .http1_only()
        .connect_timeout(Duration::from_secs(UPLOAD_CONNECT_TIMEOUT_SECS))
        .timeout(Duration::from_secs(UPLOAD_REQUEST_TIMEOUT_SECS))
        .pool_max_idle_per_host(1)
        .pool_idle_timeout(Duration::from_secs(30));
    if let Some(local_address) = detect_submission_upload_local_ipv4() {
        append_log(
            log_path,
            &format!(
                "submission_upload_client_bind local_address={}",
                local_address
            ),
        );
        builder = builder.local_address(local_address);
    }
    match builder.build() {
        Ok(client) => client,
        Err(err) => {
            append_log(
                log_path,
                &format!("submission_upload_client_build_fail err={}", err),
            );
            Client::new()
        }
    }
}

pub(super) fn effective_submission_upload_concurrency(
    configured: usize,
    workflow_status: Option<&str>,
) -> usize {
    if matches!(
        workflow_status,
        Some("FAILED" | SUBMISSION_STATUS_REUPLOAD_PENDING)
    ) {
        return 1;
    }
    configured.max(1)
}

pub(super) fn upload_retry_delay_secs(attempt: u32) -> u64 {
    let exponent = attempt.saturating_sub(1);
    let multiplier = 1u64 << exponent.min(5);
    let wait = UPLOAD_RETRY_BASE_DELAY_SECS.saturating_mul(multiplier);
    wait.min(UPLOAD_RETRY_MAX_DELAY_SECS)
}

pub(super) fn supports_direct_upload_enqueue(status: &str) -> bool {
    matches!(
        status,
        "WAITING_UPLOAD" | "FAILED" | SUBMISSION_STATUS_REUPLOAD_PENDING
    )
}

pub(super) fn upload_target_label(target: &UploadTarget) -> String {
    match target {
        UploadTarget::Segment(segment_id) => format!("segment:{}", segment_id),
        UploadTarget::Merged(merged_id) => format!("merged:{}", merged_id),
        UploadTarget::EditSegment(segment_id) => format!("edit:{}", segment_id),
    }
}

pub(super) fn upload_owner_key(owner_uid: Option<i64>) -> i64 {
    owner_uid.unwrap_or_default()
}

pub(super) fn upload_owner_label(owner_uid: Option<i64>) -> String {
    owner_uid
        .map(|value| value.to_string())
        .unwrap_or_else(|| "NULL".to_string())
}

pub(super) fn mask_upload_id(upload_id: &str) -> String {
    let value = upload_id.trim();
    if value.len() <= 12 {
        return format!("*** len={}", value.len());
    }
    format!(
        "{}...{} len={}",
        &value[..8],
        &value[value.len() - 6..],
        value.len()
    )
}

pub(super) async fn wait_for_task_upload_rate_limit(
    log_path: &PathBuf,
    task_id: &str,
    target: &UploadTarget,
    stage: &str,
) {
    loop {
        let wait_state = {
            let state = TASK_UPLOAD_RATE_LIMITS.get_or_init(|| AsyncMutex::new(HashMap::new()));
            let state = state.lock().await;
            state.get(task_id).and_then(|entry| {
                entry.blocked_until.and_then(|blocked_until| {
                    blocked_until
                        .checked_duration_since(Instant::now())
                        .map(|wait| (wait, entry.wake_tx.subscribe()))
                })
            })
        };
        let Some((wait, mut wake_rx)) = wait_state else {
            return;
        };
        if wait <= Duration::ZERO {
            return;
        }
        append_log(
            log_path,
            &format!(
                "upload_rate_limit_task_wait task_id={} stage={} target={} wait_ms={}",
                task_id,
                stage,
                upload_target_label(target),
                wait.as_millis()
            ),
        );
        tokio::select! {
            _ = sleep(wait) => {}
            changed = wake_rx.changed() => {
                if changed.is_err() {
                    sleep(wait).await;
                }
            }
        }
    }
}

pub(super) async fn reset_task_upload_rate_limit(task_id: &str) {
    let state = TASK_UPLOAD_RATE_LIMITS.get_or_init(|| AsyncMutex::new(HashMap::new()));
    let mut state = state.lock().await;
    if let Some(entry) = state.remove(task_id) {
        let next_generation = (*entry.wake_tx.borrow()).wrapping_add(1);
        let _ = entry.wake_tx.send(next_generation);
    }
}

pub(super) fn build_uploaded_parts(
    detail: &SubmissionTaskDetail,
    is_update_workflow: bool,
) -> Result<Vec<UploadedVideoPart>, String> {
    let mut parts = Vec::with_capacity(detail.output_segments.len());
    for (index, segment) in detail.output_segments.iter().enumerate() {
        if segment.upload_status != "SUCCESS" {
            return Err("存在分段未上传完成".to_string());
        }
        let cid = segment
            .cid
            .ok_or_else(|| format!("分段缺少CID segment_id={}", segment.segment_id))?;
        let has_upload_session = segment.upload_biz_id > 0
            || segment
                .upload_session_id
                .as_deref()
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false)
            || segment
                .upload_uri
                .as_deref()
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false);
        let filename = if is_update_workflow {
            if has_upload_session {
                segment
                    .file_name
                    .clone()
                    .ok_or_else(|| format!("分段缺少文件名 segment_id={}", segment.segment_id))?
            } else {
                segment.file_name.clone().unwrap_or_default()
            }
        } else {
            segment
                .file_name
                .clone()
                .ok_or_else(|| format!("分段缺少文件名 segment_id={}", segment.segment_id))?
        };
        let title = if is_update_workflow {
            resolve_existing_part_title(&detail.task, &segment.part_name, index + 1)
        } else if is_non_segmented_task(&detail.task) {
            // 分段关闭：分P标题使用配置值（写库阶段已写入 part_name），空值/默认值兜底 P{index}。
            resolve_existing_part_title(&detail.task, &segment.part_name, index + 1)
        } else {
            build_part_title(detail.task.segment_prefix.as_deref(), index + 1)
        };
        parts.push(UploadedVideoPart {
            filename,
            cid,
            title,
        });
    }
    Ok(parts)
}

pub(super) fn should_use_output_segments_upload(
    detail: &SubmissionTaskDetail,
    is_update_workflow: bool,
) -> bool {
    is_update_workflow
        || detail.task.enable_segmentation
        || (is_non_segmented_task(&detail.task) && !detail.output_segments.is_empty())
}

pub(super) fn parse_remote_archive_part_count(
    item: &Value,
    expected_bvid: &str,
    expected_aid: i64,
) -> Option<(i64, String, usize)> {
    let archive = item.get("Archive")?;
    let archive_aid = archive
        .get("aid")
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    let archive_bvid = archive
        .get("bvid")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let aid_matches = expected_aid > 0 && archive_aid == expected_aid;
    let bvid_matches = !expected_bvid.is_empty() && archive_bvid == expected_bvid;
    if !aid_matches && !bvid_matches {
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
    let video_count = archive
        .get("videos")
        .and_then(|value| value.as_array())
        .map(|items| items.len())
        .unwrap_or(0);
    Some((archive_aid, archive_bvid, cid_count.max(video_count).max(1)))
}

pub(super) async fn fetch_remote_archive_part_count_by_status(
    context: &UploadContext,
    auth: &AuthInfo,
    bvid: &str,
    aid: i64,
    status: &str,
) -> Result<Option<usize>, String> {
    let mut page = 1_i64;
    let page_size = 20_i64;
    let normalized_bvid = bvid.trim();
    loop {
        let params = vec![
            ("status".to_string(), status.to_string()),
            ("pn".to_string(), page.to_string()),
            ("ps".to_string(), page_size.to_string()),
            ("coop".to_string(), "1".to_string()),
            ("interactive".to_string(), "1".to_string()),
        ];
        let data = context
            .bilibili
            .get_json(
                "https://member.bilibili.com/x/web/archives",
                &params,
                Some(auth),
                false,
            )
            .await?;
        let arc_audits = data
            .get("arc_audits")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        for item in &arc_audits {
            let Some((archive_aid, archive_bvid, part_count)) =
                parse_remote_archive_part_count(item, normalized_bvid, aid)
            else {
                continue;
            };
            append_log(
                &context.app_log_path,
                &format!(
                    "submission_remote_part_count_found status={} aid={} bvid={} parts={}",
                    status, archive_aid, archive_bvid, part_count
                ),
            );
            return Ok(Some(part_count));
        }
        let total_count = data
            .get("page")
            .and_then(|value| value.get("count"))
            .and_then(|value| value.as_i64())
            .unwrap_or(0);
        if total_count <= 0 || page * page_size >= total_count || arc_audits.is_empty() {
            break;
        }
        page += 1;
    }
    Ok(None)
}

pub(super) async fn fetch_remote_archive_part_count(
    context: &UploadContext,
    auth: &AuthInfo,
    bvid: &str,
    aid: i64,
) -> Result<Option<usize>, String> {
    for status in [
        REMOTE_AUDIT_STATUS_REVIEWING,
        REMOTE_AUDIT_STATUS_REJECTED,
        REMOTE_AUDIT_STATUS_PUBLISHED,
    ] {
        if let Some(part_count) =
            fetch_remote_archive_part_count_by_status(context, auth, bvid, aid, status).await?
        {
            return Ok(Some(part_count));
        }
    }
    Ok(None)
}

pub(super) async fn upload_output_segments_with_worker_pool(
    submission_context: &SubmissionContext,
    upload_context: &UploadContext,
    client: &Client,
    auth: &AuthInfo,
    task_id: &str,
    workflow_instance_id: Option<String>,
    upload_concurrency: usize,
    is_update_workflow: bool,
) -> Result<Vec<UploadedVideoPart>, String> {
    let detail = load_task_detail(submission_context, task_id)?;
    if detail.output_segments.is_empty() {
        return Err("未找到分段文件".to_string());
    }
    let failed_count = detail
        .output_segments
        .iter()
        .filter(|segment| segment.upload_status == "FAILED")
        .count();
    if failed_count > 0 {
        return Err("存在分段上传失败，请重试失败分P".to_string());
    }

    let mut pending_queue: VecDeque<String> = detail
        .output_segments
        .iter()
        .filter(|segment| segment.upload_status != "SUCCESS")
        .map(|segment| segment.segment_id.clone())
        .collect();
    let mut active_uploads = FuturesUnordered::new();
    let mut preupload_retry_round: u32 = 0;
    let mut fatal_error: Option<String> = None;

    loop {
        while fatal_error.is_none()
            && active_uploads.len() < upload_concurrency
            && !pending_queue.is_empty()
        {
            let segment_id = pending_queue.pop_front().unwrap_or_default();
            if segment_id.is_empty() {
                continue;
            }
            update_segment_upload_status(submission_context, &segment_id, "UPLOADING")?;
            append_log(
                &submission_context.app_log_path,
                &format!(
                    "submission_segment_dispatch task_id={} segment_id={} active={} queued={}",
                    task_id,
                    segment_id,
                    active_uploads.len() + 1,
                    pending_queue.len()
                ),
            );
            let context_clone = submission_context.clone();
            let upload_context_clone = upload_context.clone();
            let client_clone = client.clone();
            let auth_clone = auth.clone();
            let log_path = submission_context.app_log_path.clone();
            let task_id_clone = task_id.to_string();
            let workflow_instance_id_clone = workflow_instance_id.clone();
            active_uploads.push(async move {
                let result = upload_segment_with_retry(
                    &context_clone,
                    &upload_context_clone,
                    &client_clone,
                    &auth_clone,
                    &task_id_clone,
                    workflow_instance_id_clone.as_deref(),
                    &segment_id,
                    log_path.as_ref(),
                    UPLOAD_SEGMENT_RETRY_LIMIT,
                )
                .await;
                (segment_id, result)
            });
        }

        if active_uploads.is_empty() {
            if let Some(error_message) = fatal_error {
                return Err(error_message);
            }
            let detail = load_task_detail(submission_context, task_id)?;
            return build_uploaded_parts(&detail, is_update_workflow);
        }

        let Some((segment_id, result)) = active_uploads.next().await else {
            continue;
        };
        match result {
            Ok(upload_result) => {
                preupload_retry_round = 0;
                update_segment_upload_result(
                    submission_context,
                    &segment_id,
                    "SUCCESS",
                    Some(upload_result.cid),
                    Some(upload_result.filename.clone()),
                )?;
            }
            Err(err) if is_preupload_parse_error(&err) => {
                let _ = clear_upload_session(
                    submission_context,
                    &UploadTarget::Segment(segment_id.clone()),
                );
                update_segment_upload_status(submission_context, &segment_id, "PENDING")?;
                preupload_retry_round = preupload_retry_round.saturating_add(1);
                if preupload_retry_round > PREUPLOAD_PARSE_RETRY_LIMIT {
                    fatal_error = Some("预上传解析失败重试次数已达上限".to_string());
                    continue;
                }
                let wait_secs = preupload_parse_retry_delay_secs(preupload_retry_round);
                append_log(
                    &submission_context.app_log_path,
                    &format!(
                        "submission_segment_preupload_retry task_id={} segment_id={} wait_secs={} round={}",
                        task_id, segment_id, wait_secs, preupload_retry_round
                    ),
                );
                sleep(Duration::from_secs(wait_secs)).await;
                pending_queue.push_back(segment_id);
            }
            Err(err) => {
                update_segment_upload_status(submission_context, &segment_id, "FAILED")?;
                if fatal_error.is_none() {
                    fatal_error = Some(err.clone());
                }
                append_log(
                    &submission_context.app_log_path,
                    &format!(
                        "submission_segment_upload_fail segment_id={} err={}",
                        segment_id, err
                    ),
                );
            }
        }
    }
}

pub(super) async fn run_submission_upload(context: UploadContext, task_id: String) -> Result<(), String> {
    let submission_context = SubmissionContext {
        db: context.db.clone(),
        app_log_path: context.app_log_path.clone(),
        edit_upload_state: context.edit_upload_state.clone(),
        local_path_prefix: load_local_path_prefix(context.db.as_ref()),
    };
    let workflow_instance_id = load_latest_workflow_runtime(&submission_context, &task_id)?
        .map(|(instance_id, _, _)| instance_id);

    let mut auth =
        match load_auth_or_refresh_for_task(&context, &task_id, "submission_upload").await {
            Ok(auth) => auth,
            Err(err) => {
                update_submission_status_with_reason(
                    &submission_context,
                    &task_id,
                    "FAILED",
                    Some(&err),
                )?;
                return Err(err);
            }
        };
    append_log(
        &context.app_log_path,
        &format!(
            "submission_upload_start task_id={} auth_uid={}",
            task_id,
            auth.user_id.unwrap_or_default()
        ),
    );
    let csrf = match auth.csrf.clone() {
        Some(value) => value,
        None => {
            auth = match refresh_auth_for_task(&context, &task_id, "submission_upload_csrf").await {
                Ok(auth) => auth,
                Err(err) => {
                    update_submission_status_with_reason(
                        &submission_context,
                        &task_id,
                        "FAILED",
                        Some(&err),
                    )?;
                    return Err(err);
                }
            };
            auth.csrf
                .clone()
                .ok_or_else(|| "登录信息缺少CSRF".to_string())?
        }
    };

    let detail = load_task_detail(&submission_context, &task_id)?;
    let tags = detail.task.tags.clone().unwrap_or_default();
    if tags.trim().is_empty() {
        update_submission_status_with_reason(
            &submission_context,
            &task_id,
            "FAILED",
            Some("投稿标签不能为空"),
        )?;
        return Err("投稿标签不能为空".to_string());
    }
    let workflow_type = load_latest_workflow_type(&submission_context, &task_id)?
        .unwrap_or_else(|| "VIDEO_SUBMISSION".to_string());
    let integrate_current_bvid = load_integrate_current_bvid(detail.workflow_config.as_ref());
    if integrate_current_bvid && detail.task.bvid.as_deref().unwrap_or("").trim().is_empty() {
        update_submission_status_with_reason(
            &submission_context,
            &task_id,
            "FAILED",
            Some("当前任务暂无BVID，无法集成投稿"),
        )?;
        return Err("当前任务暂无BVID，无法集成投稿".to_string());
    }
    let is_update_workflow = workflow_type == "VIDEO_UPDATE" || integrate_current_bvid;
    if is_update_workflow {
        match reset_segments_without_upload_session(&submission_context, &task_id) {
            Ok(affected) => {
                if affected > 0 {
                    append_log(
                        &context.app_log_path,
                        &format!(
                            "submission_update_reset_missing_upload_session task_id={} count={}",
                            task_id, affected
                        ),
                    );
                }
            }
            Err(err) => {
                update_submission_status_with_reason(
                    &submission_context,
                    &task_id,
                    "FAILED",
                    Some(&err),
                )?;
                return Err(err);
            }
        }
    }

    update_submission_status(&submission_context, &task_id, "UPLOADING")?;

    let configured_upload_concurrency = load_download_settings_from_db(&submission_context.db)
        .map(|settings| settings.upload_concurrency)
        .unwrap_or(DEFAULT_UPLOAD_CONCURRENCY)
        .max(1) as usize;
    let upload_concurrency = effective_submission_upload_concurrency(
        configured_upload_concurrency,
        detail
            .task
            .workflow_status
            .as_ref()
            .map(|status| status.status.as_str()),
    );
    if upload_concurrency < configured_upload_concurrency {
        append_log(
            &context.app_log_path,
            &format!(
                "submission_upload_concurrency_degraded task_id={} configured={} effective={} reason=retry_after_failure",
                task_id, configured_upload_concurrency, upload_concurrency
            ),
        );
    }
    let client = build_submission_upload_client(context.app_log_path.as_ref());
    let mut parts: Vec<UploadedVideoPart> = Vec::new();
    let use_output_segments_upload = should_use_output_segments_upload(&detail, is_update_workflow);

    if use_output_segments_upload {
        auth = match load_auth_or_refresh_for_task(&context, &task_id, "submission_upload_loop")
            .await
        {
            Ok(auth) => auth,
            Err(err) => {
                update_submission_status_with_reason(
                    &submission_context,
                    &task_id,
                    "FAILED",
                    Some(&err),
                )?;
                return Err(err);
            }
        };
        parts = match upload_output_segments_with_worker_pool(
            &submission_context,
            &context,
            &client,
            &auth,
            &task_id,
            workflow_instance_id.clone(),
            upload_concurrency,
            is_update_workflow,
        )
        .await
        {
            Ok(parts) => parts,
            Err(err) => {
                if is_workflow_superseded_error(&err) {
                    append_log(
                        &context.app_log_path,
                        &format!(
                            "submission_upload_skip_superseded task_id={} instance_id={}",
                            task_id,
                            workflow_instance_id.as_deref().unwrap_or("")
                        ),
                    );
                    return Ok(());
                }
                update_submission_status_with_reason(
                    &submission_context,
                    &task_id,
                    "FAILED",
                    Some(&err),
                )?;
                return Err(err);
            }
        };
    } else {
        let merged = load_latest_merged_video(&submission_context, &task_id)?;
        let Some(merged) = merged else {
            update_submission_status_with_reason(
                &submission_context,
                &task_id,
                "FAILED",
                Some("未找到合并视频"),
            )?;
            return Err("未找到合并视频".to_string());
        };
        let merged_path = merged.video_path.as_deref().unwrap_or("").to_string();
        if merged_path.trim().is_empty() {
            update_submission_status_with_reason(
                &submission_context,
                &task_id,
                "FAILED",
                Some("合并视频路径为空"),
            )?;
            return Err("合并视频路径为空".to_string());
        }
        let target = UploadTarget::Merged(merged.id);
        let resume_session = build_upload_session_from_merged(&merged);
        let mut current_auth = auth.clone();
        let result = loop {
            match upload_single_file(
                &submission_context,
                &task_id,
                &target,
                &client,
                &current_auth,
                Path::new(&merged_path),
                &context.app_log_path,
                resume_session.clone(),
            )
            .await
            {
                Ok(result) => break Ok(result),
                Err(err) => {
                    if is_auth_error(&err) {
                        match refresh_auth_for_task(&context, &task_id, "upload_merged").await {
                            Ok(auth) => {
                                current_auth = auth;
                                continue;
                            }
                            Err(refresh_err) => break Err(refresh_err),
                        }
                    }
                    break Err(err);
                }
            }
        }?;
        update_merged_upload_result(
            &submission_context,
            merged.id,
            Some(result.cid),
            Some(result.filename.clone()),
        )?;
        parts.push(UploadedVideoPart {
            filename: result.filename,
            cid: result.cid,
            title: build_part_title(detail.task.segment_prefix.as_deref(), 1),
        });
    }

    if should_validate_non_segmented_consistency(&detail, is_update_workflow) {
        append_log(
            &context.app_log_path,
            &format!(
        "submission_non_segmented_consistency_check task_id={} merged={} segments={} parts={}",
        task_id,
        detail.merged_videos.len(),
        detail.output_segments.len(),
        parts.len()
      ),
        );
        if let Err(err) = validate_non_segmented_submission_consistency(&detail, &parts) {
            update_submission_status_with_reason(
                &submission_context,
                &task_id,
                "FAILED",
                Some(&err),
            )?;
            append_log(
                &context.app_log_path,
                &format!(
                    "submission_non_segmented_consistency_fail task_id={} err={}",
                    task_id, err
                ),
            );
            return Err(err);
        }
    }

    if parts.is_empty() {
        update_submission_status_with_reason(
            &submission_context,
            &task_id,
            "FAILED",
            Some("投稿文件为空"),
        )?;
        return Err("投稿文件为空".to_string());
    }

    if !is_update_workflow
        && try_complete_existing_submission(
            &context,
            &submission_context,
            &task_id,
            &auth,
            parts.len(),
            "pre_submit_guard",
        )
        .await?
    {
        return Ok(());
    }

    if !is_update_workflow && parts.len() > MAX_PARTS_PER_BVID {
        update_submission_status_with_reason(
            &submission_context,
            &task_id,
            "FAILED",
            Some("Bilibili 不支持单 BV 超过 200 分P，请调整分段或拆分投稿"),
        )?;
        append_log(
            &context.app_log_path,
            &format!(
                "submission_upload_part_limit_exceeded task_id={} parts={}",
                task_id,
                parts.len()
            ),
        );
        return Err("Bilibili 不支持单 BV 超过 200 分P，请调整分段或拆分投稿".to_string());
    }

    if is_update_workflow {
        let mut aid = detail.task.aid.unwrap_or(0);
        if aid <= 0 {
            let bvid = detail.task.bvid.clone().unwrap_or_default();
            aid = fetch_aid_with_refresh(&context, &auth, &bvid)
                .await
                .unwrap_or(0);
            if aid > 0 {
                let _ = update_submission_aid(&submission_context, &task_id, aid);
            }
        }
        if aid <= 0 {
            update_submission_status_with_reason(
                &submission_context,
                &task_id,
                "FAILED",
                Some("无法获取AID，无法更新"),
            )?;
            return Err("无法获取AID，无法更新".to_string());
        }
        let missing_filename_count = detail
            .output_segments
            .iter()
            .filter(|segment| {
                let has_upload_session = segment.upload_biz_id > 0
                    || segment
                        .upload_session_id
                        .as_deref()
                        .map(|value| !value.trim().is_empty())
                        .unwrap_or(false)
                    || segment
                        .upload_uri
                        .as_deref()
                        .map(|value| !value.trim().is_empty())
                        .unwrap_or(false);
                if !has_upload_session {
                    return false;
                }
                segment
                    .file_name
                    .as_deref()
                    .map(|value| value.trim().is_empty())
                    .unwrap_or(true)
            })
            .count();
        if missing_filename_count > 0 {
            append_log(
                &context.app_log_path,
                &format!(
                    "submission_update_missing_filename task_id={} count={}",
                    task_id, missing_filename_count
                ),
            );
            update_submission_status_with_reason(
                &submission_context,
                &task_id,
                "FAILED",
                Some("存在分段缺少上传信息，请重新上传"),
            )?;
            return Err("存在分段缺少上传信息，请重新上传".to_string());
        }
        let submit_result =
            submit_video_update_in_batches(&context, &auth, &detail.task, &parts, aid, &csrf).await;
        match submit_result {
            Ok(()) => {
                if let Err(err) = verify_remote_part_count(
                    &context,
                    &auth,
                    &task_id,
                    detail.task.bvid.as_deref().unwrap_or(""),
                    aid,
                    parts.len(),
                )
                .await
                {
                    if !is_remote_part_count_mismatch_error(&err) {
                        update_submission_status_with_reason(
                            &submission_context,
                            &task_id,
                            SUBMISSION_STATUS_VERIFY_PENDING,
                            Some(&err),
                        )?;
                        append_log(
                            &context.app_log_path,
                            &format!(
                                "submission_update_remote_verify_pending task_id={} err={}",
                                task_id, err
                            ),
                        );
                        return Ok(());
                    }
                    update_submission_status_with_reason(
                        &submission_context,
                        &task_id,
                        "FAILED",
                        Some(&err),
                    )?;
                    append_log(
                        &context.app_log_path,
                        &format!(
                            "submission_update_remote_part_mismatch task_id={} err={}",
                            task_id, err
                        ),
                    );
                    return Err(err);
                }
                update_submission_status(&submission_context, &task_id, "COMPLETED")?;
                append_log(
                    &context.app_log_path,
                    &format!(
                        "submission_update_ok task_id={} bvid={} aid={}",
                        task_id,
                        detail.task.bvid.as_deref().unwrap_or(""),
                        aid
                    ),
                );
                Ok(())
            }
            Err(err) => {
                if is_already_submitted_submission_error(&err)
                    && try_complete_existing_submission(
                        &context,
                        &submission_context,
                        &task_id,
                        &auth,
                        parts.len(),
                        "submit_duplicate",
                    )
                    .await?
                {
                    return Ok(());
                }
                let final_err = if is_submit_network_busy_error(&err) {
                    build_submit_result_unknown_error(&err)
                } else if is_already_submitted_submission_error(&err) {
                    build_reupload_required_error(&err)
                } else {
                    err.clone()
                };
                if is_submit_network_busy_error(&err) {
                    update_submission_status_with_reason(
                        &submission_context,
                        &task_id,
                        SUBMISSION_STATUS_SUBMIT_UNKNOWN,
                        Some(&final_err),
                    )?;
                    append_log(
                        &context.app_log_path,
                        &format!(
                            "submission_update_submit_unknown task_id={} err={}",
                            task_id, final_err
                        ),
                    );
                    return Ok(());
                }
                update_submission_status_with_reason(
                    &submission_context,
                    &task_id,
                    "FAILED",
                    Some(&final_err),
                )?;
                append_log(
                    &context.app_log_path,
                    &format!(
                        "submission_update_submit_fail task_id={} err={}",
                        task_id, final_err
                    ),
                );
                Err(final_err)
            }
        }
    } else {
        let submit_result = submit_video_in_batches(
            &context,
            &submission_context,
            &task_id,
            &auth,
            &detail.task,
            &parts,
            &csrf,
        )
        .await;
        match submit_result {
            Ok(result) => {
                update_submission_bvid_and_aid(
                    &submission_context,
                    &task_id,
                    &result.bvid,
                    result.aid,
                )?;
                if let Some(collection_id) = detail.task.collection_id {
                    if collection_id > 0 {
                        let cid = parts.first().map(|item| item.cid).unwrap_or(0);
                        let add_result = add_video_to_collection_with_retry(
                            &context,
                            &auth,
                            &task_id,
                            &detail.task.title,
                            collection_id,
                            detail.task.collection_section_id,
                            result.aid,
                            cid,
                            &csrf,
                        )
                        .await;
                        if let Err(err) = add_result {
                            if detail.task.collection_section_id.unwrap_or(0) > 0 {
                                let reason = build_collection_bind_pending_error(&err);
                                update_submission_status_with_reason(
                                    &submission_context,
                                    &task_id,
                                    SUBMISSION_STATUS_VERIFY_PENDING,
                                    Some(&reason),
                                )?;
                                append_log(
                                    &context.app_log_path,
                                    &format!(
                                        "submission_collection_pending task_id={} collection_id={} section_id={} err={}",
                                        task_id,
                                        collection_id,
                                        detail.task.collection_section_id.unwrap_or(0),
                                        err
                                    ),
                                );
                                return Ok(());
                            }
                            append_log(
                                &context.app_log_path,
                                &format!(
                                    "submission_collection_warn task_id={} collection_id={} err={}",
                                    task_id, collection_id, err
                                ),
                            );
                        }
                    }
                }
                if let Err(err) = verify_remote_part_count(
                    &context,
                    &auth,
                    &task_id,
                    &result.bvid,
                    result.aid,
                    parts.len(),
                )
                .await
                {
                    if !is_remote_part_count_mismatch_error(&err) {
                        update_submission_status_with_reason(
                            &submission_context,
                            &task_id,
                            SUBMISSION_STATUS_VERIFY_PENDING,
                            Some(&err),
                        )?;
                        append_log(
                            &context.app_log_path,
                            &format!(
                                "submission_upload_remote_verify_pending task_id={} bvid={} aid={} err={}",
                                task_id, result.bvid, result.aid, err
                            ),
                        );
                        return Ok(());
                    }
                    update_submission_status_with_reason(
                        &submission_context,
                        &task_id,
                        "FAILED",
                        Some(&err),
                    )?;
                    append_log(
                        &context.app_log_path,
                        &format!(
                            "submission_upload_remote_part_mismatch task_id={} err={}",
                            task_id, err
                        ),
                    );
                    return Err(err);
                }
                update_submission_status(&submission_context, &task_id, "COMPLETED")?;
                append_log(
                    &context.app_log_path,
                    &format!(
                        "submission_upload_ok task_id={} bvid={} aid={}",
                        task_id, result.bvid, result.aid
                    ),
                );
                Ok(())
            }
            Err(err) => {
                if is_already_submitted_submission_error(&err)
                    && try_complete_existing_submission(
                        &context,
                        &submission_context,
                        &task_id,
                        &auth,
                        parts.len(),
                        "submit_duplicate",
                    )
                    .await?
                {
                    return Ok(());
                }
                if is_submission_created_error(&err) {
                    update_submission_status_with_reason(
                        &submission_context,
                        &task_id,
                        SUBMISSION_STATUS_VERIFY_PENDING,
                        Some(&err),
                    )?;
                    append_log(
                        &context.app_log_path,
                        &format!(
                            "submission_upload_created_verify_pending task_id={} err={}",
                            task_id, err
                        ),
                    );
                    return Ok(());
                }
                let final_err = if is_submit_network_busy_error(&err) {
                    build_submit_result_unknown_error(&err)
                } else if is_already_submitted_submission_error(&err) {
                    build_reupload_required_error(&err)
                } else {
                    err.clone()
                };
                if is_submit_network_busy_error(&err) {
                    update_submission_status_with_reason(
                        &submission_context,
                        &task_id,
                        SUBMISSION_STATUS_SUBMIT_UNKNOWN,
                        Some(&final_err),
                    )?;
                    append_log(
                        &context.app_log_path,
                        &format!(
                            "submission_upload_submit_unknown task_id={} err={}",
                            task_id, final_err
                        ),
                    );
                    return Ok(());
                }
                update_submission_status_with_reason(
                    &submission_context,
                    &task_id,
                    "FAILED",
                    Some(&final_err),
                )?;
                append_log(
                    &context.app_log_path,
                    &format!(
                        "submission_upload_submit_fail task_id={} err={}",
                        task_id, final_err
                    ),
                );
                Err(final_err)
            }
        }
    }
}

pub(super) async fn recover_submission_tasks(context: SubmissionQueueContext) {
    let submission_context = SubmissionContext {
        db: context.db.clone(),
        app_log_path: context.app_log_path.clone(),
        edit_upload_state: context.edit_upload_state.clone(),
        local_path_prefix: load_local_path_prefix(context.db.as_ref()),
    };
    let mut processing_ids = Vec::new();
    for status in ["PENDING", "CLIPPING", "MERGING", "SEGMENTING"] {
        if let Ok(list) = load_task_ids_by_status(&submission_context, status) {
            processing_ids.extend(list);
        }
    }
    let startup_recover_before = Utc::now().to_rfc3339();
    if let Err(err) = recover_uploading_tasks_before(
        &submission_context,
        startup_recover_before.as_str(),
        "submission_recover_uploading",
        0,
    ) {
        append_log(
            &context.app_log_path,
            &format!("submission_recover_uploading_fail err={}", err),
        );
    }
    if let Err(err) = recover_failed_task_segments(&submission_context) {
        append_log(
            &context.app_log_path,
            &format!("submission_recover_failed_segments_fail err={}", err),
        );
    }
    match recover_confirmed_remote_missing_tasks(&submission_context) {
        Ok(task_ids) => {
            for task_id in task_ids {
                append_log(
                    &context.app_log_path,
                    &format!(
                        "submission_recover_confirmed_remote_missing task_id={} queued=true",
                        task_id
                    ),
                );
            }
        }
        Err(err) => append_log(
            &context.app_log_path,
            &format!(
                "submission_recover_confirmed_remote_missing_fail err={}",
                err
            ),
        ),
    }

    for task_id in processing_ids {
        match should_defer_recovery_for_remote_restore(&submission_context, &task_id) {
            Ok(true) => {
                append_log(
                    &context.app_log_path,
                    &format!("submission_recover_wait_remote_restore task_id={}", task_id),
                );
                continue;
            }
            Ok(false) => {}
            Err(err) => {
                append_log(
                    &context.app_log_path,
                    &format!(
                        "submission_recover_remote_restore_check_fail task_id={} err={}",
                        task_id, err
                    ),
                );
            }
        }
        if let Err(err) = terminate_submission_ffmpeg_processes_async(&submission_context, &task_id).await {
            append_log(
                &context.app_log_path,
                &format!(
                    "submission_recover_ffmpeg_cleanup_fail task_id={} err={}",
                    task_id, err
                ),
            );
        }
        let _ = update_submission_status(&submission_context, &task_id, "PENDING");
        let _ = set_workflow_instance_status(&submission_context, &task_id, "PENDING");
        let context_clone = submission_context.clone();
        let task_id_clone = task_id.clone();
        append_log(
            &context.app_log_path,
            &format!("submission_recover_workflow task_id={}", task_id),
        );
        tauri::async_runtime::spawn(async move {
            run_submission_workflow_guarded(context_clone, task_id_clone).await;
        });
    }
}

pub(super) fn recover_confirmed_remote_missing_tasks(
    context: &SubmissionContext,
) -> Result<Vec<String>, String> {
    let now = now_rfc3339();
    context
        .db
        .with_conn_mut(|conn| {
            let pattern = format!("%{}%", SUBMISSION_REMOTE_MISSING_CONFIRMED_REASON);
            let mut stmt = conn.prepare(
                "SELECT st.task_id FROM submission_task st
                 WHERE COALESCE(TRIM(st.bvid), '') = ''
                   AND (
                     st.status = ?1
                     OR (
                       st.status = 'FAILED'
                       AND COALESCE((
                         SELECT wi.error_message FROM workflow_instances wi
                         WHERE wi.task_id = st.task_id
                         ORDER BY wi.created_at DESC LIMIT 1
                       ), '') LIKE ?2
                     )
                   )
                   AND EXISTS (
                     SELECT 1 FROM task_output_segment seg WHERE seg.task_id = st.task_id
                   )
                   AND NOT EXISTS (
                     SELECT 1 FROM task_output_segment seg
                     WHERE seg.task_id = st.task_id
                       AND NOT (
                         seg.upload_status = 'PENDING'
                         OR (
                           seg.upload_status = 'SUCCESS'
                           AND seg.cid IS NOT NULL
                           AND seg.cid > 0
                           AND seg.file_name IS NOT NULL
                           AND TRIM(seg.file_name) != ''
                         )
                       )
                   )",
            )?;
            let task_ids = stmt
                .query_map((SUBMISSION_STATUS_REUPLOAD_PENDING, pattern.as_str()), |row| {
                    row.get::<_, String>(0)
                })?
                .collect::<Result<Vec<_>, _>>()?;
            drop(stmt);

            let tx = conn.transaction()?;
            for task_id in &task_ids {
                tx.execute(
                    "UPDATE submission_task SET status = 'WAITING_UPLOAD', remote_state = NULL, remote_state_desc = NULL, reject_reason = NULL, updated_at = ?1 WHERE task_id = ?2",
                    (now.as_str(), task_id.as_str()),
                )?;
                tx.execute(
                    "UPDATE workflow_instances SET status = ?1, current_step = NULL, progress = 0, error_message = NULL, updated_at = ?2 WHERE task_id = ?3",
                    (
                        SUBMISSION_STATUS_REUPLOAD_PENDING,
                        now.as_str(),
                        task_id.as_str(),
                    ),
                )?;
            }
            tx.commit()?;
            Ok(task_ids)
        })
        .map_err(|err| err.to_string())
}

pub(super) fn build_upload_session_from_segment(
    segment: &TaskOutputSegmentRecord,
) -> Option<UploadSessionInfo> {
    let upload_id = segment.upload_session_id.as_ref()?.trim().to_string();
    let endpoint = segment.upload_endpoint.as_ref()?.trim().to_string();
    let auth = segment.upload_auth.as_ref()?.trim().to_string();
    let upos_uri = segment.upload_uri.as_ref()?.trim().to_string();
    if upload_id.is_empty()
        || endpoint.is_empty()
        || auth.is_empty()
        || upos_uri.is_empty()
        || segment.upload_chunk_size <= 0
        || segment.upload_biz_id <= 0
    {
        return None;
    }
    Some(UploadSessionInfo {
        upload_id,
        biz_id: segment.upload_biz_id,
        chunk_size: segment.upload_chunk_size.max(0) as u64,
        endpoint,
        auth,
        upos_uri,
        uploaded_bytes: segment.upload_uploaded_bytes.max(0) as u64,
        total_bytes: segment.upload_total_bytes.max(0) as u64,
        last_part_index: segment.upload_last_part_index.max(0) as u64,
    })
}

pub(super) fn build_upload_session_from_edit_segment(
    segment: &TaskOutputSegmentRecord,
) -> Option<UploadSessionInfo> {
    build_upload_session_from_segment(segment)
}

pub(super) fn build_upload_session_from_merged(merged: &MergedVideoRecord) -> Option<UploadSessionInfo> {
    let upload_id = merged.upload_session_id.as_ref()?.trim().to_string();
    let endpoint = merged.upload_endpoint.as_ref()?.trim().to_string();
    let auth = merged.upload_auth.as_ref()?.trim().to_string();
    let upos_uri = merged.upload_uri.as_ref()?.trim().to_string();
    if upload_id.is_empty()
        || endpoint.is_empty()
        || auth.is_empty()
        || upos_uri.is_empty()
        || merged.upload_chunk_size <= 0
        || merged.upload_biz_id <= 0
    {
        return None;
    }
    Some(UploadSessionInfo {
        upload_id,
        biz_id: merged.upload_biz_id,
        chunk_size: merged.upload_chunk_size.max(0) as u64,
        endpoint,
        auth,
        upos_uri,
        uploaded_bytes: merged.upload_uploaded_bytes.max(0) as u64,
        total_bytes: merged.upload_total_bytes.max(0) as u64,
        last_part_index: merged.upload_last_part_index.max(0) as u64,
    })
}

pub(super) fn sanitize_upload_session(
    resume_session: Option<UploadSessionInfo>,
    file_size: u64,
) -> Option<UploadSessionInfo> {
    let mut session = resume_session?;
    if session.total_bytes == 0 {
        session.total_bytes = file_size;
    }
    if session.total_bytes != file_size {
        return None;
    }
    if session.upload_id.trim().is_empty()
        || session.endpoint.trim().is_empty()
        || session.auth.trim().is_empty()
        || session.upos_uri.trim().is_empty()
        || session.chunk_size == 0
        || session.chunk_size > UPLOAD_MAX_CHUNK_SIZE_BYTES
        || session.biz_id <= 0
    {
        return None;
    }
    Some(session)
}

pub(super) async fn upload_file_with_session(
    context: &SubmissionContext,
    task_id: &str,
    target: &UploadTarget,
    client: &Client,
    auth: &AuthInfo,
    path: &Path,
    file_name: &str,
    file_size: u64,
    log_path: &PathBuf,
    resume_session: Option<UploadSessionInfo>,
) -> Result<UploadFileResult, String> {
    let (preupload, upload_id, resume_state) = if let Some(session) = resume_session.clone() {
        let preupload = PreuploadInfo {
            auth: session.auth.clone(),
            biz_id: session.biz_id,
            chunk_size: session.chunk_size,
            endpoint: session.endpoint.clone(),
            upos_uri: session.upos_uri.clone(),
        };
        update_upload_session(context, target, &session)?;
        (preupload, session.upload_id.clone(), resume_session)
    } else {
        let preupload = preupload_video(
            context, task_id, target, client, auth, file_name, file_size, log_path,
        )
        .await?;
        let upload_id = post_video_meta(
            context, task_id, target, client, auth, &preupload, file_size, log_path,
        )
        .await?;
        let session = UploadSessionInfo {
            upload_id: upload_id.clone(),
            biz_id: preupload.biz_id,
            chunk_size: preupload.chunk_size,
            endpoint: preupload.endpoint.clone(),
            auth: preupload.auth.clone(),
            upos_uri: preupload.upos_uri.clone(),
            uploaded_bytes: 0,
            total_bytes: file_size,
            last_part_index: 0,
        };
        update_upload_session(context, target, &session)?;
        (preupload, upload_id, None)
    };

    let total_chunks = upload_video_chunks(
        context,
        task_id,
        target,
        client,
        auth,
        path,
        &preupload,
        &upload_id,
        file_size,
        log_path,
        resume_state.as_ref(),
    )
    .await?;
    let end_result = end_upload(
        context,
        task_id,
        target,
        client,
        auth,
        &preupload,
        &upload_id,
        file_name,
        total_chunks,
        log_path,
    )
    .await?;
    let cid = end_result
        .get("data")
        .and_then(|value| value.get("cid"))
        .and_then(|value| value.as_i64())
        .unwrap_or(preupload.biz_id);
    let filename = parse_upload_filename(&end_result, file_name);
    if file_size > 0 {
        let final_index = total_chunks.saturating_sub(1);
        let snapshot = build_progress_snapshot(file_size, file_size, final_index);
        update_upload_progress(context, target, &snapshot)?;
    }

    Ok(UploadFileResult { cid, filename })
}

pub(super) async fn upload_single_file(
    context: &SubmissionContext,
    task_id: &str,
    target: &UploadTarget,
    client: &Client,
    auth: &AuthInfo,
    path: &Path,
    log_path: &PathBuf,
    resume_session: Option<UploadSessionInfo>,
) -> Result<UploadFileResult, String> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "无法读取文件名".to_string())?;
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|err| format!("读取文件失败: {}", err))?;
    let file_size = metadata.len();
    let session = sanitize_upload_session(resume_session, file_size);

    if session.is_some() {
        match upload_file_with_session(
            context,
            task_id,
            target,
            client,
            auth,
            path,
            file_name,
            file_size,
            log_path,
            session.clone(),
        )
        .await
        {
            Ok(result) => return Ok(result),
            Err(err) if is_upload_session_reset_required(&err) => {
                clear_upload_session(context, target)?;
            }
            Err(err) => return Err(err),
        }
    }

    upload_file_with_session(
        context, task_id, target, client, auth, path, file_name, file_size, log_path, None,
    )
    .await
}

pub(super) async fn upload_segment_with_retry(
    context: &SubmissionContext,
    upload_context: &UploadContext,
    client: &Client,
    auth: &AuthInfo,
    task_id: &str,
    workflow_instance_id: Option<&str>,
    segment_id: &str,
    log_path: &PathBuf,
    max_retries: u32,
) -> Result<UploadFileResult, String> {
    let mut attempt: u32 = 0;
    let mut current_auth = auth.clone();
    loop {
        attempt = attempt.saturating_add(1);
        let segment = match load_output_segment_by_id(context, segment_id)? {
            Some(segment) => segment,
            None => {
                if workflow_instance_id
                    .map(|id| is_workflow_instance_latest(context, task_id, id))
                    .transpose()?
                    .unwrap_or(true)
                {
                    return Err("分段不存在".to_string());
                }
                append_log(
                    log_path,
                    &format!(
                        "submission_segment_superseded task_id={} segment_id={} reason=missing_record",
                        task_id, segment_id
                    ),
                );
                return Err("WORKFLOW_SUPERSEDED".to_string());
            }
        };
        let path = Path::new(&segment.segment_file_path);
        if segment.segment_file_path.trim().is_empty() || !path.exists() {
            if workflow_instance_id
                .map(|id| is_workflow_instance_latest(context, task_id, id))
                .transpose()?
                .unwrap_or(true)
            {
                return Err("分段文件不存在".to_string());
            }
            append_log(
                log_path,
                &format!(
                    "submission_segment_superseded task_id={} segment_id={} reason=missing_file path={}",
                    task_id, segment_id, segment.segment_file_path
                ),
            );
            return Err("WORKFLOW_SUPERSEDED".to_string());
        }
        if let Some(workflow_instance_id) = workflow_instance_id {
            if !is_workflow_instance_latest(context, task_id, workflow_instance_id)? {
                append_log(
                    log_path,
                    &format!(
                        "submission_segment_superseded task_id={} segment_id={} reason=workflow_changed instance_id={}",
                        task_id, segment_id, workflow_instance_id
                    ),
                );
                return Err("WORKFLOW_SUPERSEDED".to_string());
            }
        }

        let target = UploadTarget::Segment(segment.segment_id.clone());
        let resume_session = build_upload_session_from_segment(&segment);
        match upload_single_file(
            context,
            task_id,
            &target,
            client,
            &current_auth,
            path,
            log_path,
            resume_session,
        )
        .await
        {
            Ok(result) => return Ok(result),
            Err(err) => {
                let local_address_unavailable = is_local_address_unavailable_error(&err);
                if is_auth_error(&err) {
                    match refresh_auth_for_task(upload_context, task_id, "upload_segment").await {
                        Ok(auth) => {
                            current_auth = auth;
                            continue;
                        }
                        Err(refresh_err) => return Err(refresh_err),
                    }
                }
                if !local_address_unavailable
                    && (is_upload_session_reset_required(&err)
                        || (is_upload_transport_error(&err)
                            && attempt >= UPLOAD_TRANSPORT_SESSION_REBUILD_ATTEMPT))
                {
                    let _ = clear_upload_session(context, &target);
                    append_log(
                        log_path,
                        &format!(
                            "submission_segment_session_reset segment_id={} attempt={} err={}",
                            segment_id, attempt, err
                        ),
                    );
                }
                append_log(
                    log_path,
                    &format!(
                        "submission_segment_retry_fail segment_id={} attempt={} err={}",
                        segment_id, attempt, err
                    ),
                );
                if local_address_unavailable {
                    return Err(err);
                }
                if attempt >= max_retries {
                    return Err(err);
                }
                let wait_secs = upload_retry_delay_secs(attempt);
                sleep(Duration::from_secs(wait_secs)).await;
            }
        }
    }
}

pub(super) async fn upload_edit_segment_with_retry(
    context: &SubmissionContext,
    upload_context: &UploadContext,
    client: &Client,
    auth: &AuthInfo,
    task_id: &str,
    workflow_instance_id: Option<&str>,
    segment_id: &str,
    log_path: &PathBuf,
    max_retries: u32,
) -> Result<UploadFileResult, String> {
    let mut attempt: u32 = 0;
    let mut current_auth = auth.clone();
    loop {
        attempt = attempt.saturating_add(1);
        let segment = match load_edit_upload_segment(context, segment_id)? {
            Some(segment) => segment,
            None => {
                if workflow_instance_id
                    .map(|id| is_workflow_instance_latest(context, task_id, id))
                    .transpose()?
                    .unwrap_or(true)
                {
                    return Err("分段不存在".to_string());
                }
                append_log(
                    log_path,
                    &format!(
                        "submission_edit_segment_superseded task_id={} segment_id={} reason=missing_record",
                        task_id, segment_id
                    ),
                );
                return Err("WORKFLOW_SUPERSEDED".to_string());
            }
        };
        let path = Path::new(&segment.segment_file_path);
        if segment.segment_file_path.trim().is_empty() || !path.exists() {
            if workflow_instance_id
                .map(|id| is_workflow_instance_latest(context, task_id, id))
                .transpose()?
                .unwrap_or(true)
            {
                return Err("分段文件不存在".to_string());
            }
            append_log(
                log_path,
                &format!(
                    "submission_edit_segment_superseded task_id={} segment_id={} reason=missing_file path={}",
                    task_id, segment_id, segment.segment_file_path
                ),
            );
            return Err("WORKFLOW_SUPERSEDED".to_string());
        }
        if let Some(workflow_instance_id) = workflow_instance_id {
            if !is_workflow_instance_latest(context, task_id, workflow_instance_id)? {
                append_log(
                    log_path,
                    &format!(
                        "submission_edit_segment_superseded task_id={} segment_id={} reason=workflow_changed instance_id={}",
                        task_id, segment_id, workflow_instance_id
                    ),
                );
                return Err("WORKFLOW_SUPERSEDED".to_string());
            }
        }

        let target = UploadTarget::EditSegment(segment.segment_id.clone());
        let resume_session = build_upload_session_from_edit_segment(&segment);
        match upload_single_file(
            context,
            task_id,
            &target,
            client,
            &current_auth,
            path,
            log_path,
            resume_session,
        )
        .await
        {
            Ok(result) => return Ok(result),
            Err(err) => {
                let local_address_unavailable = is_local_address_unavailable_error(&err);
                if is_auth_error(&err) {
                    match refresh_auth_for_task(upload_context, task_id, "upload_edit_segment")
                        .await
                    {
                        Ok(auth) => {
                            current_auth = auth;
                            continue;
                        }
                        Err(refresh_err) => return Err(refresh_err),
                    }
                }
                if !local_address_unavailable
                    && (is_upload_session_reset_required(&err)
                        || (is_upload_transport_error(&err)
                            && attempt >= UPLOAD_TRANSPORT_SESSION_REBUILD_ATTEMPT))
                {
                    let _ = clear_upload_session(context, &target);
                    append_log(
                        log_path,
                        &format!(
                            "submission_edit_segment_session_reset segment_id={} attempt={} err={}",
                            segment_id, attempt, err
                        ),
                    );
                }
                append_log(
                    log_path,
                    &format!(
                        "submission_edit_segment_retry_fail segment_id={} attempt={} err={}",
                        segment_id, attempt, err
                    ),
                );
                if local_address_unavailable {
                    return Err(err);
                }
                if attempt >= max_retries {
                    return Err(err);
                }
                let wait_secs = upload_retry_delay_secs(attempt);
                sleep(Duration::from_secs(wait_secs)).await;
            }
        }
    }
}

pub(super) async fn upload_video_chunks(
    context: &SubmissionContext,
    task_id: &str,
    target: &UploadTarget,
    client: &Client,
    auth: &AuthInfo,
    path: &Path,
    preupload: &PreuploadInfo,
    upload_id: &str,
    file_size: u64,
    log_path: &PathBuf,
    resume_state: Option<&UploadSessionInfo>,
) -> Result<u64, String> {
    let upload_url = build_upload_url(&preupload.endpoint, &preupload.upos_uri);
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|err| format!("读取视频文件失败: {}", err))?;
    let chunk_size = preupload.chunk_size;
    let total_chunks = (file_size + chunk_size - 1) / chunk_size;
    let mut start_index: u64 = 0;
    if let Some(state) = resume_state {
        if state.uploaded_bytes > 0 && state.chunk_size == chunk_size {
            start_index = state.last_part_index.saturating_add(1);
        }
    }
    if start_index > total_chunks {
        start_index = total_chunks;
    }
    let mut offset = start_index.saturating_mul(chunk_size);
    if offset > file_size {
        offset = file_size;
    }
    if offset > 0 {
        file.seek(SeekFrom::Start(offset))
            .await
            .map_err(|err| format!("跳转文件位置失败: {}", err))?;
    }

    let mut progress_limiter = UploadProgressLimiter::new();
    if offset > 0 {
        let snapshot = build_progress_snapshot(offset, file_size, start_index.saturating_sub(1));
        if update_upload_progress(context, target, &snapshot).is_ok() {
            progress_limiter.mark_saved(&snapshot);
        } else {
            append_log(
                log_path,
                &format!(
                    "upload_progress_skip target_offset={} file_size={}",
                    offset, file_size
                ),
            );
        }
    }

    let mut index = start_index;
    while index < total_chunks {
        let remaining = file_size.saturating_sub(offset);
        if remaining == 0 {
            break;
        }
        let current_size = std::cmp::min(chunk_size, remaining) as usize;
        let mut buffer = vec![0u8; current_size];
        file.read_exact(&mut buffer)
            .await
            .map_err(|err| format!("读取分片失败: {}", err))?;
        let start = offset;
        let end = offset + current_size as u64;
        let params = vec![
            ("partNumber", (index + 1).to_string()),
            ("uploadId", upload_id.to_string()),
            ("chunk", index.to_string()),
            ("chunks", total_chunks.to_string()),
            ("size", current_size.to_string()),
            ("start", start.to_string()),
            ("end", end.to_string()),
            ("total", file_size.to_string()),
        ];

        loop {
            wait_for_task_upload_rate_limit(log_path, task_id, target, "upload_chunk").await;
            let mut headers = build_headers(Some(&auth.cookie))?;
            headers.insert(
                "X-Upos-Auth",
                HeaderValue::from_str(&preupload.auth)
                    .map_err(|_| "无效的X-Upos-Auth".to_string())?,
            );
            headers.insert(
                "Content-Type",
                HeaderValue::from_static("application/octet-stream"),
            );

            let response = client
                .put(upload_url.clone())
                .headers(headers)
                .query(&params)
                .body(buffer.clone())
                .send()
                .await
                .map_err(|err| {
                    let detail = format_reqwest_error(&err);
                    append_log(
                        log_path,
                        &format!(
                            "upload_chunk_request_error target={} chunk={} err={}",
                            upload_target_label(target),
                            index,
                            detail
                        ),
                    );
                    format!("上传分片失败: {}", detail)
                })?;
            let status = response.status();
            let retry_after = retry_after_seconds(response.headers());
            let content_type = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|val| val.to_str().ok())
                .unwrap_or("-")
                .to_string();
            if status == StatusCode::NOT_ACCEPTABLE {
                wait_on_rate_limit(
                    context,
                    task_id,
                    target,
                    log_path,
                    retry_after,
                    "upload_chunk",
                )
                .await;
                continue;
            }
            let text = response
                .text()
                .await
                .map_err(|err| format!("读取分片响应失败: {}", err))?;
            if !text.contains("MULTIPART_PUT_SUCCESS") {
                let truncated_body = truncate_log_text(&text);
                append_log(
                    log_path,
                    &format!(
                        "upload_chunk_unexpected_response task_id={} target={} status={} content_type={} endpoint={} upload_id={} chunk={} part_number={} start={} end={} total={} body={}",
                        task_id,
                        upload_target_label(target),
                        status.as_u16(),
                        content_type,
                        preupload.endpoint,
                        mask_upload_id(upload_id),
                        index,
                        index + 1,
                        start,
                        end,
                        file_size,
                        truncated_body
                    ),
                );
                if is_upload_rate_limit_response(status, &text) {
                    wait_on_rate_limit(
                        context,
                        task_id,
                        target,
                        log_path,
                        retry_after,
                        "upload_chunk_body",
                    )
                    .await;
                    continue;
                }
                if is_upload_auth_response(status, &text) {
                    return Err(format!(
                        "上传分片鉴权失败: status={} body={}",
                        status.as_u16(),
                        truncated_body
                    ));
                }
                let detail = format!(
                    "status={} content_type={} chunk={} part_number={} start={} end={} total={} endpoint={} upload_id={} body={}",
                    status.as_u16(),
                    content_type,
                    index,
                    index + 1,
                    start,
                    end,
                    file_size,
                    preupload.endpoint,
                    mask_upload_id(upload_id),
                    truncated_body
                );
                if should_reset_upload_session_for_response(status, &text) {
                    return Err(format!(
                        "{} 上传会话异常，已准备重建会话: {}",
                        UPLOAD_SESSION_RESET_REQUIRED_PREFIX, detail
                    ));
                }
                return Err(format!(
                    "分片上传暂时失败，保留上传会话以便续传: {}",
                    detail
                ));
            }
            reset_task_upload_rate_limit(task_id).await;
            break;
        }

        offset = end;
        let snapshot = build_progress_snapshot(offset, file_size, index);
        if progress_limiter.should_persist(&snapshot) {
            if update_upload_progress(context, target, &snapshot).is_ok() {
                progress_limiter.mark_saved(&snapshot);
            } else {
                append_log(
                    log_path,
                    &format!(
                        "upload_progress_skip offset={} file_size={} part={}",
                        offset, file_size, index
                    ),
                );
            }
        }
        index = index.saturating_add(1);
    }

    Ok(total_chunks)
}

pub(super) async fn end_upload(
    context: &SubmissionContext,
    task_id: &str,
    target: &UploadTarget,
    client: &Client,
    auth: &AuthInfo,
    preupload: &PreuploadInfo,
    upload_id: &str,
    file_name: &str,
    total_chunks: u64,
    log_path: &PathBuf,
) -> Result<Value, String> {
    let upload_url = build_upload_url(&preupload.endpoint, &preupload.upos_uri);
    let params = vec![
        ("output", "json".to_string()),
        ("name", file_name.to_string()),
        ("profile", "ugcfx/bup".to_string()),
        ("uploadId", upload_id.to_string()),
        ("biz_id", preupload.biz_id.to_string()),
    ];
    let mut parts = Vec::new();
    for index in 0..total_chunks {
        parts.push(serde_json::json!({
          "partNumber": index + 1,
          "eTag": "etag"
        }));
    }
    let body = serde_json::json!({ "parts": parts });
    loop {
        wait_for_task_upload_rate_limit(log_path, task_id, target, "end_upload").await;
        let mut headers = build_headers(Some(&auth.cookie))?;
        headers.insert(
            "X-Upos-Auth",
            HeaderValue::from_str(&preupload.auth).map_err(|_| "无效的X-Upos-Auth".to_string())?,
        );

        let response = client
            .post(upload_url.clone())
            .headers(headers)
            .query(&params)
            .json(&body)
            .send()
            .await
            .map_err(|err| {
                let detail = format_reqwest_error(&err);
                append_log(
                    log_path,
                    &format!(
                        "upload_end_request_error target={} file={} err={}",
                        upload_target_label(target),
                        file_name,
                        detail
                    ),
                );
                format!("结束上传失败: {}", detail)
            })?;
        if response.status() == StatusCode::NOT_ACCEPTABLE {
            let retry_after = retry_after_seconds(response.headers());
            wait_on_rate_limit(
                context,
                task_id,
                target,
                log_path,
                retry_after,
                "end_upload",
            )
            .await;
            continue;
        }
        let value: Value = response
            .json()
            .await
            .map_err(|err| format!("结束上传解析失败: {}", err))?;
        if let Some(ok) = value.get("OK").and_then(|val| val.as_i64()) {
            if ok != 1 {
                return Err("结束上传失败".to_string());
            }
        }
        reset_task_upload_rate_limit(task_id).await;
        return Ok(value);
    }
}

pub(super) fn parse_upload_filename(end_result: &Value, fallback: &str) -> String {
    let fallback_name = remove_file_extension(fallback);
    let key = end_result.get("key").and_then(|value| value.as_str());
    let Some(key) = key else {
        return fallback_name;
    };
    let trimmed = key.trim_start_matches('/');
    let name = trimmed.rsplit('/').next().unwrap_or(trimmed).to_string();
    remove_file_extension(&name)
}

pub(super) fn build_upload_url(endpoint: &str, upos_uri: &str) -> String {
    let mut path = upos_uri.trim_start_matches("upos://").to_string();
    if !path.starts_with('/') {
        path = format!("/{}", path);
    }
    format!("https:{}{}", endpoint, path)
}

pub(super) fn resolve_task_cover_target_path(context: &SubmissionContext, task_id: &str) -> PathBuf {
    resolve_submission_base_dir(context, task_id)
        .join("cover")
        .join("cover.jpg")
}

pub(super) fn save_task_cover_image_from_data_url(
    context: &SubmissionContext,
    task_id: &str,
    image_data_url: &str,
) -> Result<String, String> {
    let bytes = decode_image_data_url(image_data_url)?;
    let target_path = resolve_task_cover_target_path(context, task_id);
    let cover_dir = target_path
        .parent()
        .ok_or_else(|| "封面目录无效".to_string())?;
    fs::create_dir_all(cover_dir).map_err(|err| format!("创建封面目录失败: {}", err))?;

    if let Ok(entries) = fs::read_dir(cover_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            if !name.starts_with("cover.") {
                continue;
            }
            if path == target_path {
                continue;
            }
            let _ = fs::remove_file(path);
        }
    }

    fs::write(&target_path, bytes).map_err(|err| format!("保存本地封面失败: {}", err))?;
    Ok(to_stored_submission_path(
        context,
        target_path.to_string_lossy().as_ref(),
    ))
}

pub(super) fn remove_task_cover_image_for_record(
    context: &SubmissionContext,
    task_id: &str,
    stored_cover_path: Option<&str>,
) -> Result<(), String> {
    let fallback = resolve_task_cover_target_path(context, task_id);
    let target = stored_cover_path
        .map(|value| to_runtime_submission_path(context, value))
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or(fallback);

    if path_exists(&target) {
        fs::remove_file(&target).map_err(|err| format!("删除本地封面失败: {}", err))?;
    }

    if let Some(parent) = target.parent() {
        if parent.ends_with("cover") {
            if fs::read_dir(parent)
                .map(|mut it| it.next().is_none())
                .unwrap_or(false)
            {
                let _ = fs::remove_dir(parent);
            }
        }
    }
    Ok(())
}

pub(super) async fn upload_cover(
    context: &UploadContext,
    auth: &AuthInfo,
    csrf: &str,
    image_data_url: &str,
) -> Result<String, String> {
    let cover = image_data_url.trim();
    if !cover.starts_with("data:image/") || !cover.contains(";base64,") {
        return Err("封面数据格式不合法，请重新选择图片".to_string());
    }
    let url = "https://member.bilibili.com/x/vu/web/cover/up";
    let params = vec![("ts".to_string(), Utc::now().timestamp_millis().to_string())];
    let form = vec![
        ("csrf".to_string(), csrf.to_string()),
        ("cover".to_string(), cover.to_string()),
    ];
    let data = context
        .bilibili
        .post_form(url, &params, &form, Some(auth))
        .await?;
    let cover_url = data
        .get("url")
        .and_then(|value| value.as_str())
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "封面上传成功但响应缺少URL".to_string())?;
    Ok(cover_url.to_string())
}

pub(super) async fn upload_cover_with_refresh(
    context: &UploadContext,
    auth: &AuthInfo,
    csrf: &str,
    image_data_url: &str,
) -> Result<String, String> {
    match upload_cover(context, auth, csrf, image_data_url).await {
        Ok(url) => Ok(url),
        Err(err) => {
            if !is_auth_error(&err) {
                return Err(err);
            }
            let auth = refresh_auth(context, "submission_upload_cover").await?;
            let csrf = auth
                .csrf
                .clone()
                .ok_or_else(|| "登录信息缺少CSRF".to_string())?;
            upload_cover(context, &auth, &csrf, image_data_url).await
        }
    }
}

pub(super) async fn submit_video_add_with_refresh(
    context: &UploadContext,
    auth: &AuthInfo,
    task: &SubmissionTaskRecord,
    parts: &[UploadedVideoPart],
    csrf: &str,
) -> Result<SubmissionSubmitResult, String> {
    match submit_video_add(context, auth, task, parts, csrf).await {
        Ok(result) => Ok(result),
        Err(err) => {
            if !is_auth_error(&err) {
                return Err(err);
            }
            let auth = refresh_auth(context, "submit_video_add").await?;
            let csrf = auth
                .csrf
                .clone()
                .ok_or_else(|| "登录信息缺少CSRF".to_string())?;
            submit_video_add(context, &auth, task, parts, &csrf).await
        }
    }
}

pub(super) async fn submit_video_edit_with_refresh(
    context: &UploadContext,
    auth: &AuthInfo,
    task: &SubmissionTaskRecord,
    parts: &[UploadedVideoPart],
    aid: i64,
    csrf: &str,
    include_parent_collection: bool,
) -> Result<(), String> {
    match submit_video_edit(
        context,
        auth,
        task,
        parts,
        aid,
        csrf,
        include_parent_collection,
    )
    .await
    {
        Ok(()) => Ok(()),
        Err(err) => {
            if !is_auth_error(&err) {
                return Err(err);
            }
            let auth = refresh_auth(context, "submit_video_edit").await?;
            let csrf = auth
                .csrf
                .clone()
                .ok_or_else(|| "登录信息缺少CSRF".to_string())?;
            submit_video_edit(
                context,
                &auth,
                task,
                parts,
                aid,
                &csrf,
                include_parent_collection,
            )
            .await
        }
    }
}

pub(super) async fn submit_video_edit_with_rate_limit_retry(
    context: &UploadContext,
    auth: &AuthInfo,
    task: &SubmissionTaskRecord,
    parts: &[UploadedVideoPart],
    aid: i64,
    csrf: &str,
    include_parent_collection: bool,
) -> Result<(), String> {
    let mut attempt: u32 = 0;
    let mut wait_secs = SUBMISSION_EDIT_RATE_LIMIT_BASE_WAIT_SECS;
    loop {
        match submit_video_edit_with_refresh(
            context,
            auth,
            task,
            parts,
            aid,
            csrf,
            include_parent_collection,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(err) => {
                if !is_rate_limit_error(&err) {
                    return Err(err);
                }
                attempt = attempt.saturating_add(1);
                if attempt > SUBMISSION_EDIT_RATE_LIMIT_RETRY_LIMIT {
                    return Err(err);
                }
                let current_wait = wait_secs.min(SUBMISSION_EDIT_RATE_LIMIT_MAX_WAIT_SECS);
                append_log(
                    &context.app_log_path,
                    &format!(
                        "submission_edit_rate_limited aid={} attempt={} wait_secs={} err={}",
                        aid, attempt, current_wait, err
                    ),
                );
                sleep(Duration::from_secs(current_wait)).await;
                wait_secs = wait_secs.saturating_mul(2);
            }
        }
    }
}

pub(super) async fn submit_video_in_batches(
    context: &UploadContext,
    submission_context: &SubmissionContext,
    task_id: &str,
    auth: &AuthInfo,
    task: &SubmissionTaskRecord,
    parts: &[UploadedVideoPart],
    csrf: &str,
) -> Result<SubmissionSubmitResult, String> {
    let batch_end_indexes = submission_batch_end_indexes(parts.len());
    if batch_end_indexes.len() <= 1 {
        return submit_video_add_with_refresh(context, auth, task, parts, csrf).await;
    }

    let total_batches = batch_end_indexes.len();
    append_log(
        &context.app_log_path,
        &format!(
            "submission_submit_batches_start title={} total_parts={} total_batches={}",
            task.title,
            parts.len(),
            total_batches
        ),
    );

    let first_end = batch_end_indexes[0];
    let first_parts = &parts[..first_end];
    let result = submit_video_add_with_refresh(context, auth, task, first_parts, csrf).await?;
    update_submission_bvid_and_aid(submission_context, task_id, &result.bvid, result.aid)?;
    append_log(
        &context.app_log_path,
        &format!(
            "submission_submit_first_batch_persisted task_id={} bvid={} aid={} parts={}",
            task_id,
            result.bvid,
            result.aid,
            first_parts.len()
        ),
    );
    let mut completed_end = first_end;
    for (batch_offset, next_end) in batch_end_indexes.iter().copied().enumerate().skip(1) {
        let batch_index = batch_offset + 1;
        append_log(
            &context.app_log_path,
            &format!(
                "submission_edit_batch_start aid={} batch={}/{} parts=1-{}",
                result.aid, batch_index, total_batches, next_end
            ),
        );
        append_log(
            &context.app_log_path,
            &format!(
                "submission_edit_batch_wait aid={} batch={}/{} wait_secs={}",
                result.aid, batch_index, total_batches, SUBMISSION_EDIT_BATCH_WAIT_SECS
            ),
        );
        sleep(Duration::from_secs(SUBMISSION_EDIT_BATCH_WAIT_SECS)).await;
        if let Err(err) = submit_video_edit_with_rate_limit_retry(
            context,
            auth,
            task,
            &parts[..next_end],
            result.aid,
            csrf,
            task.collection_section_id.unwrap_or(0) <= 0,
        )
        .await
        {
            return Err(format!(
                "{} BV已创建但后续分P提交结果待确认，bvid={} aid={} completed_parts={} target_parts={}。原始错误: {}",
                SUBMISSION_CREATED_PREFIX, result.bvid, result.aid, completed_end, next_end, err
            ));
        }
        append_log(
            &context.app_log_path,
            &format!(
                "submission_edit_batch_ok aid={} batch={}/{} parts=1-{}",
                result.aid, batch_index, total_batches, next_end
            ),
        );
        completed_end = next_end;
    }

    Ok(result)
}

pub(super) async fn submit_video_update_in_batches(
    context: &UploadContext,
    auth: &AuthInfo,
    task: &SubmissionTaskRecord,
    parts: &[UploadedVideoPart],
    aid: i64,
    csrf: &str,
) -> Result<(), String> {
    let batch_end_indexes = submission_batch_end_indexes(parts.len());
    if batch_end_indexes.len() <= 1 {
        submit_video_edit_with_refresh(context, auth, task, parts, aid, csrf, true).await?;
        return Ok(());
    }

    let total_batches = batch_end_indexes.len();
    append_log(
        &context.app_log_path,
        &format!(
            "submission_update_batches_start title={} total_parts={} total_batches={}",
            task.title,
            parts.len(),
            total_batches
        ),
    );
    for (batch_offset, next_end) in batch_end_indexes.iter().copied().enumerate() {
        let batch_index = batch_offset + 1;
        append_log(
            &context.app_log_path,
            &format!(
                "submission_update_batch_start aid={} batch={}/{} parts=1-{}",
                aid, batch_index, total_batches, next_end
            ),
        );
        if batch_index > 1 {
            append_log(
                &context.app_log_path,
                &format!(
                    "submission_update_batch_wait aid={} batch={}/{} wait_secs={}",
                    aid, batch_index, total_batches, SUBMISSION_EDIT_BATCH_WAIT_SECS
                ),
            );
            sleep(Duration::from_secs(SUBMISSION_EDIT_BATCH_WAIT_SECS)).await;
        }
        submit_video_edit_with_rate_limit_retry(
            context,
            auth,
            task,
            &parts[..next_end],
            aid,
            csrf,
            true,
        )
        .await?;
        append_log(
            &context.app_log_path,
            &format!(
                "submission_update_batch_ok aid={} batch={}/{} parts=1-{}",
                aid, batch_index, total_batches, next_end
            ),
        );
    }
    Ok(())
}

pub(super) async fn submit_video_add(
    context: &UploadContext,
    auth: &AuthInfo,
    task: &SubmissionTaskRecord,
    parts: &[UploadedVideoPart],
    csrf: &str,
) -> Result<SubmissionSubmitResult, String> {
    let payload = build_add_payload(task, parts);
    match context
        .bilibili
        .get_json(
            "https://member.bilibili.com/x/geetest/pre/add",
            &[],
            Some(auth),
            false,
        )
        .await
    {
        Ok(_) => append_log(
            &context.app_log_path,
            &format!(
                "submission_submit_preflight_ok title={} parts={}",
                task.title,
                parts.len()
            ),
        ),
        Err(err) => append_log(
            &context.app_log_path,
            &format!(
                "submission_submit_preflight_skip title={} parts={} err={}",
                task.title,
                parts.len(),
                err
            ),
        ),
    }
    let payload_bytes = serde_json::to_vec(&payload)
        .map(|value| value.len())
        .unwrap_or_default();
    let first_cid = parts.first().map(|part| part.cid).unwrap_or_default();
    let last_cid = parts.last().map(|part| part.cid).unwrap_or_default();
    append_log(
        &context.app_log_path,
        &format!(
            "submission_submit_start title={} season_id={} section_id={} parts={} payload_bytes={} first_cid={} last_cid={}",
            task.title,
            task.collection_id.unwrap_or(0),
            task.collection_section_id.unwrap_or(0),
            parts.len(),
            payload_bytes,
            first_cid,
            last_cid
        ),
    );
    let params = vec![
        ("ts".to_string(), Utc::now().timestamp_millis().to_string()),
        ("csrf".to_string(), csrf.to_string()),
    ];
    let url = "https://member.bilibili.com/x/vu/web/add/v3";
    let data = context
        .bilibili
        .post_json(url, &params, &payload, Some(auth))
        .await?;
    let bvid = data
        .get("bvid")
        .and_then(|val| val.as_str())
        .ok_or_else(|| "投稿响应缺少BVID".to_string())?;
    let aid = data
        .get("aid")
        .and_then(|val| val.as_i64())
        .ok_or_else(|| "投稿响应缺少AID".to_string())?;
    append_log(
        &context.app_log_path,
        &format!(
            "submission_submit_ok title={} season_id={} section_id={} bvid={} aid={}",
            task.title,
            task.collection_id.unwrap_or(0),
            task.collection_section_id.unwrap_or(0),
            bvid,
            aid
        ),
    );
    Ok(SubmissionSubmitResult {
        bvid: bvid.to_string(),
        aid,
    })
}

pub(super) async fn submit_video_edit(
    context: &UploadContext,
    auth: &AuthInfo,
    task: &SubmissionTaskRecord,
    parts: &[UploadedVideoPart],
    aid: i64,
    csrf: &str,
    include_parent_collection: bool,
) -> Result<(), String> {
    let payload = build_edit_payload(task, parts, aid, include_parent_collection);
    let params = vec![
        ("t".to_string(), Utc::now().timestamp_millis().to_string()),
        ("csrf".to_string(), csrf.to_string()),
    ];
    let url = "https://member.bilibili.com/x/vu/web/edit";
    let _ = context
        .bilibili
        .post_json(url, &params, &payload, Some(auth))
        .await?;
    Ok(())
}

/// 归一化源视频「上传分P标题」与其模式。
/// 返回 (title, mode)：title 为 trim 后的非空值（否则 None），mode 归一为 AUTO/CUSTOM。
/// 标题长度超过 80 时按截断处理（前端已校验，这里兜底）。
pub(super) fn normalize_upload_part_title(source: &SourceVideoInput) -> (Option<String>, String) {
    let title = source
        .upload_part_title
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(80).collect::<String>());
    let mode = match source.upload_part_title_mode.as_deref() {
        Some(value) if value.trim().eq_ignore_ascii_case("CUSTOM") => "CUSTOM",
        _ => "AUTO",
    }
    .to_string();
    (title, mode)
}

/// 从合并组 JSON 对象解析自定义「上传分P标题」。
/// 仅当 uploadPartTitleMode == CUSTOM 且标题非空时返回 Some（截断至 80 字符）；
/// AUTO 模式返回 None，由调用方按最终顺序兜底 P{index}。
pub(super) fn extract_upload_part_title_from_json(group: &Value) -> Option<String> {
    let mode = group
        .get("uploadPartTitleMode")
        .or_else(|| group.get("upload_part_title_mode"))
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if !mode.trim().eq_ignore_ascii_case("CUSTOM") {
        return None;
    }
    group
        .get("uploadPartTitle")
        .or_else(|| group.get("upload_part_title"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(80).collect::<String>())
}

/// 加载任务各源视频的自定义「上传分P标题」，返回 runtime 源路径 -> 自定义标题。
/// 仅收录 upload_part_title_mode == CUSTOM 且标题非空的项。
pub(super) fn load_source_upload_titles(
    context: &SubmissionContext,
    task_id: &str,
) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let _ = context.db.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT source_file_path, upload_part_title, upload_part_title_mode FROM task_source_video WHERE task_id = ?1",
        )?;
        let rows = stmt.query_map([task_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;
        for row in rows {
            let (path, title, mode) = row?;
            let is_custom = mode
                .as_deref()
                .map(|value| value.trim().eq_ignore_ascii_case("CUSTOM"))
                .unwrap_or(false);
            if !is_custom {
                continue;
            }
            let Some(title) = title
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| value.chars().take(80).collect::<String>())
            else {
                continue;
            };
            let runtime_path = to_runtime_submission_path(context, &path);
            map.insert(runtime_path, title);
        }
        Ok(())
    });
    map
}

pub(super) fn update_upload_progress(
    context: &SubmissionContext,
    target: &UploadTarget,
    snapshot: &UploadProgressSnapshot,
) -> Result<(), String> {
    match target {
    UploadTarget::Segment(segment_id) => context
      .db
      .with_conn(|conn| {
        conn.execute(
          "UPDATE task_output_segment SET upload_progress = ?1, upload_uploaded_bytes = ?2, upload_total_bytes = ?3, upload_last_part_index = ?4 WHERE segment_id = ?5",
          (
            snapshot.progress,
            snapshot.uploaded_bytes as i64,
            snapshot.total_bytes as i64,
            snapshot.last_part_index as i64,
            segment_id,
          ),
        )?;
        Ok(())
      })
      .map_err(|err| err.to_string()),
    UploadTarget::Merged(merged_id) => context
      .db
      .with_conn(|conn| {
        conn.execute(
          "UPDATE merged_video SET upload_progress = ?1, upload_uploaded_bytes = ?2, upload_total_bytes = ?3, upload_last_part_index = ?4 WHERE id = ?5",
          (
            snapshot.progress,
            snapshot.uploaded_bytes as i64,
            snapshot.total_bytes as i64,
            snapshot.last_part_index as i64,
            merged_id,
          ),
        )?;
        Ok(())
      })
      .map_err(|err| err.to_string()),
    UploadTarget::EditSegment(segment_id) => update_edit_upload_segment(
      context,
      segment_id,
      |segment| {
        segment.upload_progress = snapshot.progress;
        segment.upload_uploaded_bytes = snapshot.uploaded_bytes as i64;
        segment.upload_total_bytes = snapshot.total_bytes as i64;
        segment.upload_last_part_index = snapshot.last_part_index as i64;
      },
    ),
  }?;
    touch_submission_updated_at_for_target(context, target)
}

pub(super) fn update_upload_status_for_target(
    context: &SubmissionContext,
    target: &UploadTarget,
    status: &str,
) -> Result<(), String> {
    match target {
        UploadTarget::Segment(segment_id) => {
            update_segment_upload_status(context, segment_id, status)
        }
        UploadTarget::Merged(_) => Ok(()),
        UploadTarget::EditSegment(segment_id) => {
            update_edit_upload_segment(context, segment_id, |segment| {
                segment.upload_status = status.to_string();
            })
        }
    }?;
    touch_submission_updated_at_for_target(context, target)
}

pub(super) fn update_upload_session(
    context: &SubmissionContext,
    target: &UploadTarget,
    session: &UploadSessionInfo,
) -> Result<(), String> {
    let progress = if session.total_bytes > 0 {
        (session.uploaded_bytes as f64 / session.total_bytes as f64) * 100.0
    } else {
        0.0
    };
    match target {
    UploadTarget::Segment(segment_id) => context
      .db
      .with_conn(|conn| {
        conn.execute(
          "UPDATE task_output_segment SET upload_session_id = ?1, upload_biz_id = ?2, upload_endpoint = ?3, upload_auth = ?4, upload_uri = ?5, upload_chunk_size = ?6, upload_uploaded_bytes = ?7, upload_total_bytes = ?8, upload_progress = ?9, upload_last_part_index = ?10 WHERE segment_id = ?11",
          (
            &session.upload_id,
            session.biz_id,
            &session.endpoint,
            &session.auth,
            &session.upos_uri,
            session.chunk_size as i64,
            session.uploaded_bytes as i64,
            session.total_bytes as i64,
            progress,
            session.last_part_index as i64,
            segment_id,
          ),
        )?;
        Ok(())
      })
      .map_err(|err| err.to_string()),
    UploadTarget::Merged(merged_id) => context
      .db
      .with_conn(|conn| {
        conn.execute(
          "UPDATE merged_video SET upload_session_id = ?1, upload_biz_id = ?2, upload_endpoint = ?3, upload_auth = ?4, upload_uri = ?5, upload_chunk_size = ?6, upload_uploaded_bytes = ?7, upload_total_bytes = ?8, upload_progress = ?9, upload_last_part_index = ?10 WHERE id = ?11",
          (
            &session.upload_id,
            session.biz_id,
            &session.endpoint,
            &session.auth,
            &session.upos_uri,
            session.chunk_size as i64,
            session.uploaded_bytes as i64,
            session.total_bytes as i64,
            progress,
            session.last_part_index as i64,
            merged_id,
          ),
        )?;
        Ok(())
      })
      .map_err(|err| err.to_string()),
    UploadTarget::EditSegment(segment_id) => update_edit_upload_segment(
      context,
      segment_id,
      |segment| {
        segment.upload_session_id = Some(session.upload_id.clone());
        segment.upload_biz_id = session.biz_id;
        segment.upload_endpoint = Some(session.endpoint.clone());
        segment.upload_auth = Some(session.auth.clone());
        segment.upload_uri = Some(session.upos_uri.clone());
        segment.upload_chunk_size = session.chunk_size as i64;
        segment.upload_uploaded_bytes = session.uploaded_bytes as i64;
        segment.upload_total_bytes = session.total_bytes as i64;
        segment.upload_progress = progress;
        segment.upload_last_part_index = session.last_part_index as i64;
      },
    ),
  }
}

pub(super) fn clear_upload_session(context: &SubmissionContext, target: &UploadTarget) -> Result<(), String> {
    match target {
    UploadTarget::Segment(segment_id) => context
      .db
      .with_conn(|conn| {
        conn.execute(
          "UPDATE task_output_segment SET upload_session_id = NULL, upload_biz_id = 0, upload_endpoint = NULL, upload_auth = NULL, upload_uri = NULL, upload_chunk_size = 0, upload_uploaded_bytes = 0, upload_total_bytes = 0, upload_progress = 0, upload_last_part_index = 0 WHERE segment_id = ?1",
          [segment_id],
        )?;
        Ok(())
      })
      .map_err(|err| err.to_string()),
    UploadTarget::Merged(merged_id) => context
      .db
      .with_conn(|conn| {
        conn.execute(
          "UPDATE merged_video SET upload_session_id = NULL, upload_biz_id = 0, upload_endpoint = NULL, upload_auth = NULL, upload_uri = NULL, upload_chunk_size = 0, upload_uploaded_bytes = 0, upload_total_bytes = 0, upload_progress = 0, upload_last_part_index = 0 WHERE id = ?1",
          [merged_id],
        )?;
        Ok(())
      })
      .map_err(|err| err.to_string()),
    UploadTarget::EditSegment(segment_id) => update_edit_upload_segment(
      context,
      segment_id,
      |segment| {
        segment.upload_session_id = None;
        segment.upload_biz_id = 0;
        segment.upload_endpoint = None;
        segment.upload_auth = None;
        segment.upload_uri = None;
        segment.upload_chunk_size = 0;
        segment.upload_uploaded_bytes = 0;
        segment.upload_total_bytes = 0;
        segment.upload_progress = 0.0;
        segment.upload_last_part_index = 0;
      },
    ),
  }
}

pub(super) fn reset_segments_without_upload_session(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<usize, String> {
    context
    .db
    .with_conn(|conn| {
      let affected = conn.execute(
        "UPDATE task_output_segment SET upload_status = 'PENDING', cid = NULL, file_name = NULL, upload_progress = 0, upload_uploaded_bytes = 0, upload_total_bytes = 0, upload_session_id = NULL, upload_biz_id = 0, upload_endpoint = NULL, upload_auth = NULL, upload_uri = NULL, upload_chunk_size = 0, upload_last_part_index = 0 WHERE task_id = ?1 AND upload_status = 'SUCCESS' AND (cid IS NULL OR cid <= 0) AND (upload_biz_id IS NULL OR upload_biz_id = 0) AND (upload_session_id IS NULL OR TRIM(upload_session_id) = '') AND (upload_uri IS NULL OR TRIM(upload_uri) = '')",
        [task_id],
      )?;
      Ok(affected)
    })
    .map_err(|err| err.to_string())
}

pub(super) fn update_segment_upload_result(
    context: &SubmissionContext,
    segment_id: &str,
    status: &str,
    cid: Option<i64>,
    file_name: Option<String>,
) -> Result<(), String> {
    context
    .db
    .with_conn(|conn| {
      conn.execute(
        "UPDATE task_output_segment SET upload_status = ?1, cid = ?2, file_name = ?3 WHERE segment_id = ?4",
        (status, cid, file_name, segment_id),
      )?;
      Ok(())
    })
    .map_err(|err| err.to_string())
}

pub(super) fn update_merged_upload_result(
    context: &SubmissionContext,
    merged_id: i64,
    cid: Option<i64>,
    file_name: Option<String>,
) -> Result<(), String> {
    context
        .db
        .with_conn(|conn| {
            conn.execute(
                "UPDATE merged_video SET upload_cid = ?1, upload_file_name = ?2 WHERE id = ?3",
                (cid, file_name, merged_id),
            )?;
            Ok(())
        })
        .map_err(|err| err.to_string())
}

pub(super) fn update_segment_upload_status(
    context: &SubmissionContext,
    segment_id: &str,
    status: &str,
) -> Result<(), String> {
    context
        .db
        .with_conn(|conn| {
            conn.execute(
                "UPDATE task_output_segment SET upload_status = ?1 WHERE segment_id = ?2",
                (status, segment_id),
            )?;
            Ok(())
        })
        .map_err(|err| err.to_string())
}

pub(super) fn count_segments_by_upload_status(
    context: &SubmissionContext,
    task_id: &str,
    upload_status: &str,
) -> Result<i64, String> {
    context
        .db
        .with_conn(|conn| {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM task_output_segment WHERE task_id = ?1 AND upload_status = ?2",
                (task_id, upload_status),
                |row| row.get(0),
            )?;
            Ok(count)
        })
        .map_err(|err| err.to_string())
}

pub(super) fn resolve_runtime_cover_local_path(
    context: &SubmissionContext,
    stored_path: Option<String>,
) -> Option<String> {
    let runtime_path = to_runtime_submission_path_opt(context, stored_path);
    runtime_path
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .filter(|value| path_exists(Path::new(value)))
}

pub(super) fn recover_stalled_uploading_tasks(context: &SubmissionContext) -> Result<(), String> {
    let stale_before = Utc::now() - ChronoDuration::seconds(SUBMISSION_UPLOAD_STALE_TIMEOUT_SECS);
    let stale_before_rfc3339 = stale_before.to_rfc3339();
    recover_uploading_tasks_before(
        context,
        stale_before_rfc3339.as_str(),
        "submission_queue_recover_stalled_upload",
        SUBMISSION_UPLOAD_STALE_TIMEOUT_SECS,
    )
}

pub(super) fn recover_failed_task_segments(context: &SubmissionContext) -> Result<(), String> {
    let task_ids = context
        .db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT st.task_id FROM submission_task st JOIN task_output_segment tos ON tos.task_id = st.task_id WHERE st.status = 'FAILED' AND tos.upload_status IN ('UPLOADING', 'RATE_LIMITED') ORDER BY st.updated_at ASC",
            )?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .map_err(|err| err.to_string())?;

    for task_id in task_ids {
        let updated = context
            .db
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE task_output_segment SET upload_status = 'FAILED' WHERE task_id = ?1 AND upload_status IN ('UPLOADING', 'RATE_LIMITED')",
                    [task_id.as_str()],
                )
            })
            .map_err(|err| err.to_string())?;
        append_log(
            &context.app_log_path,
            &format!(
                "submission_recover_failed_segments task_id={} updated_segments={}",
                task_id, updated
            ),
        );
    }
    Ok(())
}

pub(super) fn recover_uploading_tasks_before(
    context: &SubmissionContext,
    stale_before_rfc3339: &str,
    log_reason: &str,
    timeout_secs: i64,
) -> Result<(), String> {
    let stale_tasks: Vec<(String, String)> = context
        .db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT task_id, updated_at \
         FROM submission_task \
         WHERE status = 'UPLOADING' AND updated_at != '' AND updated_at < ?1 \
         ORDER BY updated_at ASC",
            )?;
            let rows = stmt.query_map([stale_before_rfc3339], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .map_err(|err| err.to_string())?;

    for (task_id, updated_at) in stale_tasks {
        let reset_segments = context
            .db
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE task_output_segment \
           SET upload_status = 'PENDING' \
           WHERE task_id = ?1 AND upload_status IN ('UPLOADING', 'RATE_LIMITED')",
                    [task_id.as_str()],
                )
            })
            .map_err(|err| err.to_string())?;
        update_submission_status(context, &task_id, "WAITING_UPLOAD")?;
        append_log(
            &context.app_log_path,
            &format!(
                "{} task_id={} updated_at={} reset_segments={} timeout_secs={}",
                log_reason, task_id, updated_at, reset_segments, timeout_secs
            ),
        );
    }

    Ok(())
}
