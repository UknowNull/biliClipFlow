//! 投稿队列调度、后台任务与启动恢复。

use super::*;

pub(super) fn build_submission_queue_context(state: &State<'_, AppState>) -> SubmissionQueueContext {
    SubmissionQueueContext {
        db: state.db.clone(),
        bilibili: state.bilibili.clone(),
        login_store: state.login_store.clone(),
        app_log_path: state.app_log_path.clone(),
        edit_upload_state: state.edit_upload_state.clone(),
        remote_refresh_pause_count: state.submission_remote_refresh_pause_count.clone(),
    }
}

pub fn start_submission_background_tasks(
    db: Arc<Db>,
    bilibili: Arc<BilibiliClient>,
    login_store: Arc<LoginStore>,
    app_log_path: Arc<PathBuf>,
    edit_upload_state: Arc<Mutex<EditUploadState>>,
    remote_refresh_pause_count: Arc<AtomicUsize>,
) {
    static SUBMISSION_BACKGROUND_TASKS_STARTED: AtomicBool = AtomicBool::new(false);
    if SUBMISSION_BACKGROUND_TASKS_STARTED.swap(true, Ordering::SeqCst) {
        append_log(
            app_log_path.as_ref(),
            "submission_background_tasks_skip reason=already_started",
        );
        return;
    }
    let context = SubmissionQueueContext {
        db,
        bilibili,
        login_store,
        app_log_path,
        edit_upload_state,
        remote_refresh_pause_count,
    };
    let queue_context = context.clone();
    tauri::async_runtime::spawn(async move {
        recover_submission_tasks(queue_context.clone()).await;
        submission_queue_loop(queue_context).await;
    });
    let refresh_context = context.clone();
    tauri::async_runtime::spawn(async move {
        submission_remote_refresh_loop(refresh_context).await;
    });
}

#[tauri::command]
pub async fn submission_queue_prioritize(
    state: State<'_, AppState>,
    task_id: String,
) -> Result<ApiResponse<String>, String> {
    let context = SubmissionContext::new(&state);
    let task_id = task_id.trim().to_string();
    if task_id.is_empty() {
        return Ok(ApiResponse::error("任务ID不能为空"));
    }
    let now = now_rfc3339();
    if let Err(err) = context.db.with_conn(|conn| {
        let updated = conn.execute(
            "UPDATE submission_task SET priority = 1, priority_at = ?1, updated_at = ?2 WHERE task_id = ?3",
            (&now, &now, &task_id),
        )?;
        if updated == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        Ok(())
    }) {
        let message = match err {
            DbError::Sql(rusqlite::Error::QueryReturnedNoRows) => "未找到投稿任务".to_string(),
            other => format!("设置优先投稿失败: {}", other),
        };
        return Ok(ApiResponse::error(message));
    }
    Ok(ApiResponse::success("已设置为优先投稿".to_string()))
}

pub(super) fn submission_queue_retry_delay_secs(attempt: u32) -> u64 {
    let safe_attempt = attempt.max(1);
    let exponent = safe_attempt.saturating_sub(1);
    let multiplier = 1u64 << exponent.min(10);
    let wait = SUBMISSION_QUEUE_RETRY_BASE_DELAY_SECS.saturating_mul(multiplier);
    wait.min(SUBMISSION_QUEUE_RETRY_MAX_DELAY_SECS)
}

pub(super) async fn submission_queue_loop(context: SubmissionQueueContext) {
    let submission_context = SubmissionContext {
        db: context.db.clone(),
        app_log_path: context.app_log_path.clone(),
        edit_upload_state: context.edit_upload_state.clone(),
        local_path_prefix: load_local_path_prefix(context.db.as_ref()),
    };
    let active_owners = Arc::new(AsyncMutex::new(HashMap::<(i64, String), String>::new()));
    let mut last_stale_check = Instant::now()
        .checked_sub(Duration::from_secs(
            SUBMISSION_UPLOAD_STALE_CHECK_INTERVAL_SECS,
        ))
        .unwrap_or_else(Instant::now);
    loop {
        if last_stale_check.elapsed()
            >= Duration::from_secs(SUBMISSION_UPLOAD_STALE_CHECK_INTERVAL_SECS)
        {
            if let Err(err) = recover_stalled_uploading_tasks(&submission_context) {
                append_log(
                    &context.app_log_path,
                    &format!("submission_queue_stale_recover_fail err={}", err),
                );
            }
            last_stale_check = Instant::now();
        }
        let queued_tasks = match load_next_queued_tasks(&submission_context) {
            Ok(tasks) => tasks,
            Err(err) => {
                append_log(
                    &context.app_log_path,
                    &format!("submission_queue_load_fail err={}", err),
                );
                sleep(Duration::from_secs(2)).await;
                continue;
            }
        };
        let active_owner_snapshot = {
            let owners = active_owners.lock().await;
            owners.keys().cloned().collect::<HashSet<(i64, String)>>()
        };
        let ready_tasks: Vec<QueuedSubmissionTask> = queued_tasks
            .into_iter()
            .filter(|task| !active_owner_snapshot.contains(&task.owner_key()))
            .collect();
        if ready_tasks.is_empty() {
            sleep(Duration::from_secs(2)).await;
            continue;
        }
        append_log(
            &context.app_log_path,
            &format!(
                "submission_queue_batch size={} active_owners={}",
                ready_tasks.len(),
                active_owner_snapshot.len()
            ),
        );
        for queued_task in ready_tasks {
            let owner_key = queued_task.owner_key();
            let worker_id = uuid::Uuid::new_v4().to_string();
            let should_spawn = {
                let mut owners = active_owners.lock().await;
                try_acquire_submission_owner(&mut owners, owner_key.clone(), &worker_id)
            };
            if !should_spawn {
                continue;
            }
            let context_clone = context.clone();
            let submission_context_clone = submission_context.clone();
            let active_owners_clone = active_owners.clone();
            tauri::async_runtime::spawn(async move {
                process_submission_queue_task(context_clone, submission_context_clone, queued_task)
                    .await;
                let mut owners = active_owners_clone.lock().await;
                release_submission_owner(&mut owners, owner_key, &worker_id);
            });
        }
        sleep(Duration::from_secs(2)).await;
    }
}

pub(super) async fn process_submission_queue_task(
    context: SubmissionQueueContext,
    submission_context: SubmissionContext,
    queued_task: QueuedSubmissionTask,
) {
    append_log(
        &context.app_log_path,
        &format!(
            "submission_queue_pick task_id={} bilibili_uid={}",
            queued_task.task_id,
            queued_task
                .bilibili_uid
                .map(|value| value.to_string())
                .unwrap_or_else(|| "NULL".to_string())
        ),
    );
    let upload_context = UploadContext {
        db: context.db.clone(),
        bilibili: context.bilibili.clone(),
        login_store: context.login_store.clone(),
        app_log_path: context.app_log_path.clone(),
        edit_upload_state: context.edit_upload_state.clone(),
    };
    let task_id = queued_task.task_id;
    let mut queue_retry_round: u32 = 0;
    loop {
        let result = run_submission_upload(upload_context.clone(), task_id.clone()).await;
        match result {
            Ok(()) => break,
            Err(err) => {
                if is_workflow_superseded_error(&err) {
                    append_log(
                        &context.app_log_path,
                        &format!(
                            "submission_queue_upload_skip_superseded task_id={}",
                            task_id
                        ),
                    );
                    break;
                }
                append_log(
                    &context.app_log_path,
                    &format!(
                        "submission_queue_upload_fail task_id={} err={}",
                        task_id, err
                    ),
                );
                if !is_retryable_submission_error(&err) {
                    break;
                }
                let local_address_unavailable = is_local_address_unavailable_error(&err);
                if let Err(reset_err) =
                    reset_failed_segments_to_pending(&submission_context, &task_id)
                {
                    append_log(
                        &context.app_log_path,
                        &format!(
                            "submission_queue_retry_reset_fail task_id={} err={}",
                            task_id, reset_err
                        ),
                    );
                }
                let retry_status = if local_address_unavailable {
                    "WAITING_UPLOAD"
                } else {
                    "UPLOADING"
                };
                if let Err(status_err) =
                    update_submission_status(&submission_context, &task_id, retry_status)
                {
                    append_log(
                        &context.app_log_path,
                        &format!(
                            "submission_queue_retry_keep_active_fail task_id={} err={}",
                            task_id, status_err
                        ),
                    );
                }
                if local_address_unavailable {
                    queue_retry_round = 0;
                    append_log(
                        &context.app_log_path,
                        &format!(
                            "submission_queue_local_address_backoff task_id={} wait_secs={}",
                            task_id, UPLOAD_LOCAL_ADDRESS_RECOVERY_WAIT_SECS
                        ),
                    );
                    sleep(Duration::from_secs(UPLOAD_LOCAL_ADDRESS_RECOVERY_WAIT_SECS)).await;
                    continue;
                }
                queue_retry_round = queue_retry_round.saturating_add(1);
                if queue_retry_round >= SUBMISSION_QUEUE_RETRY_LIMIT {
                    let has_other =
                        has_other_queued_tasks(&submission_context, &task_id).unwrap_or(false);
                    append_log(
                        &context.app_log_path,
                        &format!(
                            "submission_queue_retry_threshold task_id={} round={} has_other={}",
                            task_id, queue_retry_round, has_other
                        ),
                    );
                    if has_other {
                        if let Err(status_err) = update_submission_status(
                            &submission_context,
                            &task_id,
                            "WAITING_UPLOAD",
                        ) {
                            append_log(
                                &context.app_log_path,
                                &format!(
                                    "submission_queue_retry_move_tail_fail task_id={} err={}",
                                    task_id, status_err
                                ),
                            );
                        }
                        append_log(
                            &context.app_log_path,
                            &format!("submission_queue_retry_move_tail task_id={}", task_id),
                        );
                        break;
                    }
                    queue_retry_round = 0;
                }
                let wait_secs = submission_queue_retry_delay_secs(queue_retry_round);
                append_log(
                    &context.app_log_path,
                    &format!(
                        "submission_queue_retry_wait task_id={} wait_secs={} round={}",
                        task_id, wait_secs, queue_retry_round
                    ),
                );
                sleep(Duration::from_secs(wait_secs)).await;
            }
        }
    }
}

pub(super) fn enqueue_submission_after_segment_retry(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<bool, String> {
    let remaining = count_incomplete_segments(context, task_id)?;
    if remaining > 0 {
        let status = load_task_status(context, task_id)?;
        let failed_segments = count_segments_by_upload_status(context, task_id, "FAILED")?;
        if status == "FAILED" && failed_segments == 0 {
            update_submission_status(context, task_id, "WAITING_UPLOAD")?;
            update_workflow_status(context, task_id, "COMPLETED", None, 100.0)?;
            append_log(
                context.app_log_path.as_ref(),
                &format!(
                    "submission_retry_segment_enqueue_continue task_id={} remaining={}",
                    task_id, remaining
                ),
            );
            return Ok(true);
        }
        append_log(
            context.app_log_path.as_ref(),
            &format!(
                "submission_retry_segment_enqueue_skip task_id={} reason=incomplete_segments remaining={} failed_segments={} status={}",
                task_id, remaining, failed_segments, status
            ),
        );
        return Ok(false);
    }

    let status = load_task_status(context, task_id)?;
    if status == "COMPLETED" {
        append_log(
            context.app_log_path.as_ref(),
            &format!(
                "submission_retry_segment_enqueue_skip task_id={} reason=completed",
                task_id
            ),
        );
        return Ok(false);
    }
    if status == "UPLOADING" {
        append_log(
            context.app_log_path.as_ref(),
            &format!(
                "submission_retry_segment_enqueue_skip task_id={} reason=uploading",
                task_id
            ),
        );
        return Ok(false);
    }

    if status != "WAITING_UPLOAD" {
        update_submission_status(context, task_id, "WAITING_UPLOAD")?;
    }
    update_workflow_status(context, task_id, "COMPLETED", None, 100.0)?;
    append_log(
        context.app_log_path.as_ref(),
        &format!(
            "submission_retry_segment_enqueue_ok task_id={} previous_status={}",
            task_id, status
        ),
    );
    Ok(true)
}

pub(super) fn load_auth_from_queue_context(context: &SubmissionQueueContext) -> Result<AuthInfo, String> {
    context
        .login_store
        .load_auth_info(&context.db)
        .ok()
        .flatten()
        .ok_or_else(|| "请先登录".to_string())
}

pub(super) fn load_next_queued_tasks(
    context: &SubmissionContext,
) -> Result<Vec<QueuedSubmissionTask>, String> {
    context
        .db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT task_id, bilibili_uid, COALESCE(source_type, 'NORMAL') \
         FROM submission_task \
         WHERE status = 'WAITING_UPLOAD' \
         ORDER BY \
           priority DESC, \
           CASE WHEN COALESCE(priority, 0) = 1 THEN COALESCE(priority_at, updated_at) ELSE NULL END DESC, \
           CASE WHEN COALESCE(priority, 0) = 0 THEN updated_at ELSE NULL END ASC",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(QueuedSubmissionTask {
                    task_id: row.get(0)?,
                    bilibili_uid: row.get(1)?,
                    source_type: row.get(2)?,
                })
            })?;
            let mut seen_owners = HashSet::new();
            let mut result = Vec::new();
            for row in rows {
                let item = row?;
                if seen_owners.insert(item.owner_key()) {
                    result.push(item);
                }
            }
            Ok(result)
        })
        .map_err(|err| err.to_string())
}

pub(super) fn has_other_queued_tasks(context: &SubmissionContext, task_id: &str) -> Result<bool, String> {
    let (owner_uid, source_type) = load_task_owner_key(context.db.as_ref(), task_id)?;
    context
        .db
        .with_conn(|conn| {
            let count: i64 = match owner_uid {
                Some(owner_uid) => conn.query_row(
                    "SELECT COUNT(*) FROM submission_task \
           WHERE status = 'WAITING_UPLOAD' AND task_id != ?1 AND bilibili_uid = ?2 \
             AND COALESCE(source_type, 'NORMAL') = ?3",
                    params![task_id, owner_uid, source_type],
                    |row| row.get(0),
                )?,
                None => conn.query_row(
                    "SELECT COUNT(*) FROM submission_task \
           WHERE status = 'WAITING_UPLOAD' AND task_id != ?1 AND bilibili_uid IS NULL \
             AND COALESCE(source_type, 'NORMAL') = ?2",
                    params![task_id, source_type],
                    |row| row.get(0),
                )?,
            };
            Ok(count > 0)
        })
        .map_err(|err| err.to_string())
}
