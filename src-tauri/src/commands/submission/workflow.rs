//! 工作流实例与步骤状态管理。

use super::*;

pub(super) fn load_latest_workflow_config_record(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<Option<(i64, Value)>, String> {
    context
        .db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT wc.config_id, wc.configuration_data FROM workflow_instances wi \
         JOIN workflow_configurations wc ON wi.configuration_id = wc.config_id \
         WHERE wi.task_id = ?1 ORDER BY wi.created_at DESC LIMIT 1",
            )?;
            let row: Option<(i64, String)> = stmt
                .query_row([task_id], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()?;
            Ok(row)
        })
        .map_err(|err| err.to_string())
        .and_then(|row| match row {
            Some((config_id, raw)) => serde_json::from_str::<Value>(&raw)
                .map(|config| Some((config_id, config)))
                .map_err(|err| format!("解析工作流配置失败: {}", err)),
            None => Ok(None),
        })
}

pub(super) fn load_task_specific_workflow_config_record(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<Option<(i64, Value)>, String> {
    let config_name = format!("workflow_{}", task_id);
    context
        .db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT config_id, configuration_data FROM workflow_configurations \
                 WHERE config_name = ?1 ORDER BY updated_at DESC, config_id DESC LIMIT 1",
            )?;
            let row: Option<(i64, String)> = stmt
                .query_row([config_name], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional()?;
            Ok(row)
        })
        .map_err(|err| err.to_string())
        .and_then(|row| match row {
            Some((config_id, raw)) => serde_json::from_str::<Value>(&raw)
                .map(|config| Some((config_id, config)))
                .map_err(|err| format!("解析工作流配置失败: {}", err)),
            None => Ok(None),
        })
}

pub(super) fn load_effective_workflow_config_record(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<Option<(i64, Value)>, String> {
    match load_latest_workflow_config_record(context, task_id)? {
        Some(record) => Ok(Some(record)),
        None => load_task_specific_workflow_config_record(context, task_id),
    }
}

#[tauri::command(async)]
pub fn workflow_status(
    state: State<'_, AppState>,
    task_id: String,
) -> ApiResponse<Option<WorkflowStatusRecord>> {
    let context = SubmissionContext::new(&state);
    match load_workflow_status(&context, &task_id) {
        Ok(status) => ApiResponse::success(status),
        Err(err) => ApiResponse::error(format!("Failed to load workflow status: {}", err)),
    }
}

#[tauri::command(async)]
pub fn workflow_pause(state: State<'_, AppState>, task_id: String) -> ApiResponse<String> {
    let context = SubmissionContext::new(&state);
    match load_workflow_status(&context, &task_id) {
        Ok(Some(status)) => {
            if status.status != "RUNNING" {
                return ApiResponse::error("当前工作流无法暂停");
            }
            match set_workflow_instance_status(&context, &task_id, "PAUSED") {
                Ok(()) => ApiResponse::success("Paused".to_string()),
                Err(err) => ApiResponse::error(err),
            }
        }
        Ok(None) => ApiResponse::error("未找到工作流实例"),
        Err(err) => ApiResponse::error(err),
    }
}

#[tauri::command(async)]
pub fn workflow_resume(state: State<'_, AppState>, task_id: String) -> ApiResponse<String> {
    let context = SubmissionContext::new(&state);
    match load_workflow_status(&context, &task_id) {
        Ok(Some(status)) => {
            if status.status != "PAUSED" {
                return ApiResponse::error("当前工作流无法恢复");
            }
            match set_workflow_instance_status(&context, &task_id, "RUNNING") {
                Ok(()) => ApiResponse::success("Resumed".to_string()),
                Err(err) => ApiResponse::error(err),
            }
        }
        Ok(None) => ApiResponse::error("未找到工作流实例"),
        Err(err) => ApiResponse::error(err),
    }
}

#[tauri::command(async)]
pub fn workflow_cancel(state: State<'_, AppState>, task_id: String) -> ApiResponse<String> {
    let context = SubmissionContext::new(&state);
    match set_workflow_instance_status(&context, &task_id, "CANCELLED") {
        Ok(()) => {
            let _ = update_submission_status(&context, &task_id, "CANCELLED");
            ApiResponse::success("Cancelled".to_string())
        }
        Err(err) => ApiResponse::error(err),
    }
}

pub fn create_workflow_instance_for_task_with_type(
    db: &Db,
    task_id: &str,
    config: &Value,
    workflow_type: &str,
) -> Result<(String, String), String> {
    let config_json = serde_json::to_string(config).map_err(|err| err.to_string())?;
    let now = now_rfc3339();
    let instance_id = uuid::Uuid::new_v4().to_string();

    db.with_conn(|conn| {
      conn.execute(
        "INSERT INTO workflow_configurations (config_name, config_type, workflow_type, configuration_data, description, is_active, version, created_at, updated_at) \
         VALUES (?1, 'INSTANCE_SPECIFIC', ?2, ?3, NULL, 1, 1, ?4, ?5)",
        (format!("workflow_{}", task_id), workflow_type, config_json, &now, &now),
      )?;

      let config_id = conn.last_insert_rowid();

      conn.execute(
        "INSERT INTO workflow_instances (instance_id, task_id, workflow_type, status, current_step, progress, configuration_id, created_at, updated_at) \
         VALUES (?1, ?2, ?3, 'PENDING', NULL, 0, ?4, ?5, ?6)",
        (&instance_id, task_id, workflow_type, config_id, &now, &now),
      )?;

      Ok(())
    })
    .map_err(|err| format!("Failed to create workflow: {}", err))?;

    Ok((instance_id, "PENDING".to_string()))
}

pub fn create_workflow_instance_for_task(
    db: &Db,
    task_id: &str,
    config: &Value,
) -> Result<(String, String), String> {
    create_workflow_instance_for_task_with_type(db, task_id, config, "VIDEO_SUBMISSION")
}

pub(super) fn create_workflow_instance(
    context: &SubmissionContext,
    task_id: &str,
    config: &Value,
) -> Result<(String, String), String> {
    create_workflow_instance_for_task(context.db.as_ref(), task_id, config)
}

pub(super) async fn run_submission_workflow(
    context: SubmissionContext,
    task_id: String,
) -> Result<(), String> {
    let (workflow_instance_id, workflow_type, workflow_config) =
        load_latest_workflow_runtime(&context, &task_id)?
            .ok_or_else(|| "Workflow instance not found".to_string())?;
    let is_update_workflow = workflow_type == "VIDEO_UPDATE";
    let (reprocess_mode, reprocess_merged_id) = load_reprocess_metadata(workflow_config.as_ref());
    let media_process_mode = load_media_process_mode(workflow_config.as_ref());
    let use_transcode_media = media_process_mode == MediaProcessMode::ClipAndMergeTranscode;
    let _ = wait_for_workflow_ready(&context, &task_id).await?;
    burn_clip_subtitles(&context, &task_id, workflow_config.as_ref()).await?;

    let sources = if is_update_workflow {
        match load_update_sources(&context, &task_id)? {
            Some(update_sources) => update_sources,
            None => load_source_videos(&context, &task_id)?,
        }
    } else if reprocess_mode == ReprocessMode::Specified {
        let mut specified_sources = Vec::new();
        if let Some(merged_id) = reprocess_merged_id {
            if let Ok(bound_sources) = load_merged_source_clips(&context, &task_id, merged_id) {
                if !bound_sources.is_empty() {
                    specified_sources = normalize_binding_sources(bound_sources);
                }
            }
        }
        if specified_sources.is_empty() {
            if let Some(config) = workflow_config.as_ref() {
                let update_sources = extract_update_sources_from_config(config);
                if !update_sources.is_empty() {
                    specified_sources = normalize_binding_sources(update_sources);
                }
            }
        }
        if specified_sources.is_empty() {
            load_source_videos(&context, &task_id)?
        } else {
            specified_sources
        }
    } else {
        load_source_videos(&context, &task_id)?
    };
    if sources.is_empty() {
        mark_submission_failure(&context, &task_id, None, "未找到可用的源视频")?;
        return Err("未找到可用的源视频".to_string());
    }

    let merge_groups = if !is_update_workflow {
        resolve_merge_groups(&context, &task_id, workflow_config.as_ref(), &sources)
    } else {
        Vec::new()
    };
    let has_merge_groups_config = workflow_config
        .as_ref()
        .and_then(|config| config.get("mergeGroups"))
        .and_then(|value| value.as_array())
        .map(|list| !list.is_empty())
        .unwrap_or(false);
    if !is_update_workflow && (merge_groups.len() > 1 || has_merge_groups_config) {
        let mut prepared_groups = Vec::new();
        for group in merge_groups {
            let normalized = match ensure_sources_ready(
                &context,
                &task_id,
                &group.sources,
                &workflow_instance_id,
            )
            .await
            {
                Ok(value) => value,
                Err(err) if err == "WORKFLOW_SUPERSEDED" => return Ok(()),
                Err(err) => return Err(err),
            };
            prepared_groups.push(MergeGroupPlan {
                order: group.order,
                sources: normalized,
                upload_part_title: group.upload_part_title.clone(),
            });
        }
        if !is_workflow_instance_latest(&context, &task_id, &workflow_instance_id)? {
            append_log(
                &context.app_log_path,
                &format!(
                    "submission_workflow_superseded task_id={} instance_id={}",
                    task_id, workflow_instance_id
                ),
            );
            return Ok(());
        }
        let _ = wait_for_workflow_ready(&context, &task_id).await?;
        let _ = update_workflow_status(&context, &task_id, "RUNNING", Some("CLIPPING"), 0.0);
        update_submission_status(&context, &task_id, "CLIPPING")?;

        let base_dir = resolve_submission_base_dir(&context, &task_id);
        let workflow_dir = base_dir
            .join("runs")
            .join(sanitize_filename(&workflow_instance_id));
        let clip_dir = workflow_dir.join("cut");
        append_log(
            &context.app_log_path,
            &format!(
                "submission_clip_start task_id={} groups={} output_dir={} mode={}",
                task_id,
                prepared_groups.len(),
                clip_dir.to_string_lossy(),
                if use_transcode_media {
                    "clip_transcode"
                } else {
                    "clip_copy"
                }
            ),
        );
        let mut all_clip_outputs: Vec<PathBuf> = Vec::new();
        let mut all_clip_sources: Vec<ClipSource> = Vec::new();
        let mut group_clip_outputs: Vec<Vec<PathBuf>> = Vec::new();
        for (index, group) in prepared_groups.iter().enumerate() {
            for source in &group.sources {
                append_log(
                    &context.app_log_path,
                    &format!(
                        "submission_clip_source task_id={} order={} input={} start={} end={}",
                        task_id,
                        source.order,
                        source.input_path,
                        source.start_time.as_deref().unwrap_or(""),
                        source.end_time.as_deref().unwrap_or("")
                    ),
                );
            }
            let group_clip_dir = clip_dir.join(format!("group_{:03}", index + 1));
            let sources_clone = group.sources.clone();
            let clip_dir_clone = group_clip_dir.clone();
            let clip_outputs = if use_transcode_media {
                match tauri::async_runtime::spawn_blocking(move || {
                    clip_sources_transcode(
                        &sources_clone,
                        &clip_dir_clone,
                        TranscodeProfile::ClipAndMergeClean,
                    )
                })
                .await
                {
                    Ok(Ok(outputs)) => outputs,
                    Ok(Err(err)) => {
                        append_log(
                            &context.app_log_path,
                            &format!("submission_clip_fail task_id={} err={}", task_id, err),
                        );
                        return Err(err);
                    }
                    Err(_) => {
                        append_log(
                            &context.app_log_path,
                            &format!(
                                "submission_clip_fail task_id={} err=spawn_blocking_failed",
                                task_id
                            ),
                        );
                        return Err("Failed to clip videos".to_string());
                    }
                }
            } else {
                let group_use_copy = match decide_clip_copy(&group.sources) {
                    Ok(decision) => {
                        if let Some(reason) = decision.reason.as_deref() {
                            append_log(
                                &context.app_log_path,
                                &format!(
                                    "submission_clip_copy_decision task_id={} group={} use_copy={} reason={}",
                                    task_id,
                                    index + 1,
                                    decision.use_copy,
                                    reason
                                ),
                            );
                        }
                        decision.use_copy
                    }
                    Err(err) => {
                        append_log(
                            &context.app_log_path,
                            &format!(
                                "submission_clip_copy_check_err task_id={} group={} err={}",
                                task_id,
                                index + 1,
                                err
                            ),
                        );
                        false
                    }
                };
                match tauri::async_runtime::spawn_blocking(move || {
                    clip_sources(&sources_clone, &clip_dir_clone, group_use_copy)
                })
                .await
                {
                    Ok(Ok(outputs)) => outputs,
                    Ok(Err(err)) => {
                        append_log(
                            &context.app_log_path,
                            &format!("submission_clip_fail task_id={} err={}", task_id, err),
                        );
                        return Err(err);
                    }
                    Err(_) => {
                        append_log(
                            &context.app_log_path,
                            &format!(
                                "submission_clip_fail task_id={} err=spawn_blocking_failed",
                                task_id
                            ),
                        );
                        return Err("Failed to clip videos".to_string());
                    }
                }
            };
            all_clip_outputs.extend(clip_outputs.clone());
            all_clip_sources.extend(group.sources.clone());
            group_clip_outputs.push(clip_outputs);
        }
        append_log(
            &context.app_log_path,
            &format!(
                "submission_clip_done task_id={} outputs={} output_dir={}",
                task_id,
                all_clip_outputs.len(),
                clip_dir.to_string_lossy()
            ),
        );

        if !ensure_workflow_instance_latest(&context, &task_id, &workflow_instance_id, "POST_CLIP")?
        {
            return Ok(());
        }

        let _ = wait_for_workflow_ready(&context, &task_id).await?;
        save_video_clips(
            &context,
            &task_id,
            &all_clip_sources,
            &all_clip_outputs,
            true,
        )?;

        update_submission_status(&context, &task_id, "MERGING")?;
        let _ = update_workflow_status(&context, &task_id, "RUNNING", Some("MERGING"), 40.0);
        let merge_stamp = sanitize_filename(&now_rfc3339());
        let mut merged_outputs: Vec<(i64, PathBuf)> = Vec::new();
        for (index, group) in prepared_groups.iter().enumerate() {
            let merge_output =
                build_merge_output_path_for_group(&workflow_dir, &task_id, &merge_stamp, index + 1);
            let merge_list_path = merge_output.with_extension("txt");
            append_log(
                &context.app_log_path,
                &format!(
          "submission_merge_start task_id={} inputs={} output={} list={} mode={} group={}",
          task_id,
          group_clip_outputs
            .get(index)
            .map(|value| value.len())
            .unwrap_or(0),
          merge_output.to_string_lossy(),
          merge_list_path.to_string_lossy(),
          if use_transcode_media {
            "merge_transcode"
          } else {
            "concat_copy"
          },
          index + 1
        ),
            );
            if let Some(paths) = group_clip_outputs.get(index) {
                for path in paths {
                    append_log(
                        &context.app_log_path,
                        &format!(
                            "submission_merge_input task_id={} path={} group={}",
                            task_id,
                            path.to_string_lossy(),
                            index + 1
                        ),
                    );
                }
            }
            let merge_output_clone = merge_output.clone();
            let merge_inputs = group_clip_outputs.get(index).cloned().unwrap_or_default();
            if use_transcode_media {
                tauri::async_runtime::spawn_blocking(move || {
                    merge_files_transcode(
                        &merge_inputs,
                        &merge_output_clone,
                        TranscodeProfile::ClipAndMergeClean,
                    )
                })
                .await
                .map_err(|_| "Failed to merge videos".to_string())??;
            } else {
                tauri::async_runtime::spawn_blocking(move || {
                    merge_files(&merge_inputs, &merge_output_clone)
                })
                .await
                .map_err(|_| "Failed to merge videos".to_string())??;
            }
            append_log(
                &context.app_log_path,
                &format!(
                    "submission_merge_done task_id={} output={} group={}",
                    task_id,
                    merge_output.to_string_lossy(),
                    index + 1
                ),
            );
            verify_generated_media_output(
                &context,
                &task_id,
                &merge_output,
                "submission_merge_verify",
                Some(index + 1),
            )?;
            let merged_id = save_merged_video_with_sort_order(
                &context,
                &task_id,
                &merge_output,
                Some((index + 1) as i64),
            )?;
            append_log(
                &context.app_log_path,
                &format!(
                    "submission_merge_saved task_id={} merged_id={} path={} group={}",
                    task_id,
                    merged_id,
                    merge_output.to_string_lossy(),
                    index + 1
                ),
            );
            if let Err(err) =
                save_merged_source_bindings(&context, &task_id, merged_id, &group.sources)
            {
                append_log(
                    &context.app_log_path,
                    &format!(
                        "submission_merge_bind_sources_fail task_id={} merged_id={} err={}",
                        task_id, merged_id, err
                    ),
                );
            }
            merged_outputs.push((merged_id, merge_output));
        }

        if !ensure_workflow_instance_latest(
            &context,
            &task_id,
            &workflow_instance_id,
            "POST_MERGE",
        )? {
            return Ok(());
        }

        let workflow_settings = load_workflow_settings(&context, &task_id);
        let mut next_part_order: i64 = 1;
        let mut next_name_index: usize = 1;
        let _ = context.db.with_conn(|conn| {
            conn.execute(
                "DELETE FROM task_output_segment WHERE task_id = ?1",
                [&task_id],
            )?;
            Ok(())
        });

        if workflow_settings.enable_segmentation {
            let _ = wait_for_workflow_ready(&context, &task_id).await?;
            update_submission_status(&context, &task_id, "SEGMENTING")?;
            let _ = update_workflow_status(&context, &task_id, "RUNNING", Some("SEGMENTING"), 70.0);
            for (index, (merged_id, merged_path)) in merged_outputs.iter().enumerate() {
                let segment_dir = workflow_dir
                    .join("output")
                    .join(format!("group_{:03}", index + 1));
                append_log(
          &context.app_log_path,
          &format!(
            "submission_segment_start task_id={} input={} output_dir={} segment_seconds={} mode={} group={}",
            task_id,
            merged_path.to_string_lossy(),
            segment_dir.to_string_lossy(),
            workflow_settings.segment_duration_seconds,
            if use_transcode_media {
              "segment_transcode"
            } else {
              "segment_copy"
            },
            index + 1
          ),
        );
                let segment_dir_clone = segment_dir.clone();
                let merged_path_clone = merged_path.clone();
                let segment_outputs = tauri::async_runtime::spawn_blocking(move || {
                    segment_file_with_options(
                        &merged_path_clone,
                        &segment_dir_clone,
                        workflow_settings.segment_duration_seconds,
                        false,
                        use_transcode_media,
                    )
                })
                .await
                .map_err(|_| "Failed to segment video".to_string())??;
                append_log(
                    &context.app_log_path,
                    &format!(
                        "submission_segment_done task_id={} outputs={} output_dir={} group={}",
                        task_id,
                        segment_outputs.len(),
                        segment_dir.to_string_lossy(),
                        index + 1
                    ),
                );
                append_output_segments(
                    &context,
                    &task_id,
                    &segment_outputs,
                    Some(*merged_id),
                    workflow_settings.segment_prefix.as_deref(),
                    next_part_order,
                    next_name_index,
                )?;
                next_part_order += segment_outputs.len() as i64;
                next_name_index += segment_outputs.len();
            }

            if !ensure_workflow_instance_latest(
                &context,
                &task_id,
                &workflow_instance_id,
                "POST_SEGMENT",
            )? {
                return Ok(());
            }
        } else {
            let task = load_task_record(&context, &task_id)?;
            if is_non_segmented_task(&task) {
                for (index, (merged_id, merged_path)) in merged_outputs.iter().enumerate() {
                    append_output_segments(
                        &context,
                        &task_id,
                        &[merged_path.clone()],
                        Some(*merged_id),
                        workflow_settings.segment_prefix.as_deref(),
                        next_part_order,
                        next_name_index,
                    )?;
                    next_part_order += 1;
                    next_name_index += 1;
                    // 分段关闭：用合并组/源视频的自定义上传分P标题覆盖默认 P{index}。
                    if let Some(title) = prepared_groups
                        .get(index)
                        .and_then(|group| group.upload_part_title.as_deref())
                    {
                        apply_non_segmented_part_title(&context, &task_id, *merged_id, title)?;
                    }
                    append_log(
                        &context.app_log_path,
                        &format!(
                            "submission_non_segmented_append task_id={} merged_id={} group={}",
                            task_id,
                            merged_id,
                            index + 1
                        ),
                    );
                }
            }
        }
        if let Err(err) = baidu_sync::enqueue_submission_sync(
            context.db.as_ref(),
            context.app_log_path.as_ref(),
            &task_id,
        ) {
            append_log(
                &context.app_log_path,
                &format!("baidu_sync_enqueue_fail task_id={} err={}", task_id, err),
            );
        }

        if !ensure_workflow_instance_latest(
            &context,
            &task_id,
            &workflow_instance_id,
            "PRE_WAITING_UPLOAD",
        )? {
            return Ok(());
        }

        update_submission_status(&context, &task_id, "WAITING_UPLOAD")?;
        let workflow_status = match load_integrated_download_stats(&context, &task_id)? {
            Some(stats) if stats.completed < stats.total => "VIDEO_DOWNLOADING",
            _ => "COMPLETED",
        };
        let _ = update_workflow_status(&context, &task_id, workflow_status, None, 100.0);
        return Ok(());
    }

    let sources =
        match ensure_sources_ready(&context, &task_id, &sources, &workflow_instance_id).await {
            Ok(value) => value,
            Err(err) if err == "WORKFLOW_SUPERSEDED" => return Ok(()),
            Err(err) => return Err(err),
        };
    if !is_workflow_instance_latest(&context, &task_id, &workflow_instance_id)? {
        append_log(
            &context.app_log_path,
            &format!(
                "submission_workflow_superseded task_id={} instance_id={}",
                task_id, workflow_instance_id
            ),
        );
        return Ok(());
    }
    let _ = wait_for_workflow_ready(&context, &task_id).await?;
    let _ = update_workflow_status(&context, &task_id, "RUNNING", Some("CLIPPING"), 0.0);
    update_submission_status(&context, &task_id, "CLIPPING")?;

    let base_dir = resolve_submission_base_dir(&context, &task_id);
    let workflow_dir = if is_update_workflow {
        let update_stamp = sanitize_filename(&format!("update_{}", now_rfc3339()));
        base_dir.join("updates").join(update_stamp)
    } else {
        // 每个工作流实例独立目录，避免旧流程未退出时与新流程写入同名文件。
        base_dir
            .join("runs")
            .join(sanitize_filename(&workflow_instance_id))
    };
    let clip_dir = workflow_dir.join("cut");
    let use_copy = if use_transcode_media {
        false
    } else {
        let copy_decision = match decide_clip_copy(&sources) {
            Ok(decision) => decision,
            Err(err) => {
                append_log(
                    &context.app_log_path,
                    &format!(
                        "submission_clip_copy_check_err task_id={} err={}",
                        task_id, err
                    ),
                );
                crate::processing::ClipCopyDecision {
                    use_copy: true,
                    reason: Some(format!("forced_copy_fallback err={}", err)),
                }
            }
        };
        if let Some(reason) = copy_decision.reason.as_deref() {
            append_log(
                &context.app_log_path,
                &format!(
                    "submission_clip_copy_decision task_id={} use_copy={} reason={}",
                    task_id, copy_decision.use_copy, reason
                ),
            );
        }
        copy_decision.use_copy
    };
    append_log(
        &context.app_log_path,
        &format!(
            "submission_clip_start task_id={} sources={} use_copy={} output_dir={} mode={}",
            task_id,
            sources.len(),
            use_copy,
            clip_dir.to_string_lossy(),
            if use_transcode_media {
                "clip_transcode"
            } else {
                "clip_copy"
            }
        ),
    );
    for source in &sources {
        append_log(
            &context.app_log_path,
            &format!(
                "submission_clip_source task_id={} order={} input={} start={} end={}",
                task_id,
                source.order,
                source.input_path,
                source.start_time.as_deref().unwrap_or(""),
                source.end_time.as_deref().unwrap_or("")
            ),
        );
    }
    let sources_clone = sources.clone();
    let clip_dir_clone = clip_dir.clone();
    let clip_outputs = if use_transcode_media {
        match tauri::async_runtime::spawn_blocking(move || {
            clip_sources_transcode(
                &sources_clone,
                &clip_dir_clone,
                TranscodeProfile::ClipAndMergeClean,
            )
        })
        .await
        {
            Ok(Ok(outputs)) => outputs,
            Ok(Err(err)) => {
                append_log(
                    &context.app_log_path,
                    &format!("submission_clip_fail task_id={} err={}", task_id, err),
                );
                return Err(err);
            }
            Err(_) => {
                append_log(
                    &context.app_log_path,
                    &format!(
                        "submission_clip_fail task_id={} err=spawn_blocking_failed",
                        task_id
                    ),
                );
                return Err("Failed to clip videos".to_string());
            }
        }
    } else {
        match tauri::async_runtime::spawn_blocking(move || {
            clip_sources(&sources_clone, &clip_dir_clone, use_copy)
        })
        .await
        {
            Ok(Ok(outputs)) => outputs,
            Ok(Err(err)) => {
                append_log(
                    &context.app_log_path,
                    &format!("submission_clip_fail task_id={} err={}", task_id, err),
                );
                return Err(err);
            }
            Err(_) => {
                append_log(
                    &context.app_log_path,
                    &format!(
                        "submission_clip_fail task_id={} err=spawn_blocking_failed",
                        task_id
                    ),
                );
                return Err("Failed to clip videos".to_string());
            }
        }
    };
    append_log(
        &context.app_log_path,
        &format!(
            "submission_clip_done task_id={} outputs={} output_dir={}",
            task_id,
            clip_outputs.len(),
            clip_dir.to_string_lossy()
        ),
    );

    if !ensure_workflow_instance_latest(&context, &task_id, &workflow_instance_id, "POST_CLIP")? {
        return Ok(());
    }

    let _ = wait_for_workflow_ready(&context, &task_id).await?;
    save_video_clips(
        &context,
        &task_id,
        &sources,
        &clip_outputs,
        !is_update_workflow,
    )?;

    update_submission_status(&context, &task_id, "MERGING")?;
    let _ = update_workflow_status(&context, &task_id, "RUNNING", Some("MERGING"), 40.0);
    let merge_output = build_merge_output_path_for_task_id(&context, &workflow_dir, &task_id)?;
    let merge_list_path = merge_output.with_extension("txt");
    append_log(
        &context.app_log_path,
        &format!(
            "submission_merge_start task_id={} inputs={} output={} list={} mode={}",
            task_id,
            clip_outputs.len(),
            merge_output.to_string_lossy(),
            merge_list_path.to_string_lossy(),
            if use_transcode_media {
                "merge_transcode"
            } else {
                "concat_copy"
            }
        ),
    );
    for path in &clip_outputs {
        append_log(
            &context.app_log_path,
            &format!(
                "submission_merge_input task_id={} path={}",
                task_id,
                path.to_string_lossy()
            ),
        );
    }
    let merge_output_clone = merge_output.clone();
    if use_transcode_media {
        tauri::async_runtime::spawn_blocking(move || {
            merge_files_transcode(
                &clip_outputs,
                &merge_output_clone,
                TranscodeProfile::ClipAndMergeClean,
            )
        })
        .await
        .map_err(|_| "Failed to merge videos".to_string())??;
    } else {
        tauri::async_runtime::spawn_blocking(move || {
            merge_files(&clip_outputs, &merge_output_clone)
        })
        .await
        .map_err(|_| "Failed to merge videos".to_string())??;
    }
    append_log(
        &context.app_log_path,
        &format!(
            "submission_merge_done task_id={} output={}",
            task_id,
            merge_output.to_string_lossy()
        ),
    );
    verify_generated_media_output(
        &context,
        &task_id,
        &merge_output,
        "submission_merge_verify",
        None,
    )?;

    if !ensure_workflow_instance_latest(&context, &task_id, &workflow_instance_id, "POST_MERGE")? {
        return Ok(());
    }

    let _ = wait_for_workflow_ready(&context, &task_id).await?;
    let merged_id = save_merged_video(&context, &task_id, &merge_output)?;
    append_log(
        &context.app_log_path,
        &format!(
            "submission_merge_saved task_id={} merged_id={} path={}",
            task_id,
            merged_id,
            merge_output.to_string_lossy()
        ),
    );
    if let Err(err) = save_merged_source_bindings(&context, &task_id, merged_id, &sources) {
        append_log(
            &context.app_log_path,
            &format!(
                "submission_merge_bind_sources_fail task_id={} merged_id={} err={}",
                task_id, merged_id, err
            ),
        );
    }
    let workflow_settings = load_workflow_settings(&context, &task_id);
    if workflow_settings.enable_segmentation {
        let _ = wait_for_workflow_ready(&context, &task_id).await?;
        update_submission_status(&context, &task_id, "SEGMENTING")?;
        let _ = update_workflow_status(&context, &task_id, "RUNNING", Some("SEGMENTING"), 70.0);
        let segment_dir = workflow_dir.join("output");
        let merge_output_segment = merge_output.clone();
        append_log(
            &context.app_log_path,
            &format!(
        "submission_segment_start task_id={} input={} output_dir={} segment_seconds={} mode={}",
        task_id,
        merge_output_segment.to_string_lossy(),
        segment_dir.to_string_lossy(),
        workflow_settings.segment_duration_seconds,
        if use_transcode_media {
          "segment_transcode"
        } else {
          "segment_copy"
        }
      ),
        );
        let segment_dir_clone = segment_dir.clone();
        let segment_outputs = tauri::async_runtime::spawn_blocking(move || {
            segment_file_with_options(
                &merge_output_segment,
                &segment_dir_clone,
                workflow_settings.segment_duration_seconds,
                false,
                use_transcode_media,
            )
        })
        .await
        .map_err(|_| "Failed to segment video".to_string())??;
        append_log(
            &context.app_log_path,
            &format!(
                "submission_segment_done task_id={} outputs={} output_dir={}",
                task_id,
                segment_outputs.len(),
                segment_dir.to_string_lossy()
            ),
        );

        if !ensure_workflow_instance_latest(
            &context,
            &task_id,
            &workflow_instance_id,
            "POST_SEGMENT",
        )? {
            return Ok(());
        }

        if is_update_workflow {
            let (existing_count, max_order) = load_output_segment_stats(&context, &task_id)?;
            let name_start_index = resolve_update_name_start_index(
                &context,
                &task_id,
                existing_count,
                workflow_settings.segment_prefix.as_deref(),
            )?;
            append_output_segments(
                &context,
                &task_id,
                &segment_outputs,
                Some(merged_id),
                workflow_settings.segment_prefix.as_deref(),
                max_order + 1,
                name_start_index,
            )?;
        } else {
            save_output_segments(
                &context,
                &task_id,
                &segment_outputs,
                Some(merged_id),
                workflow_settings.segment_prefix.as_deref(),
            )?;
        }
    } else if !is_update_workflow {
        let task = load_task_record(&context, &task_id)?;
        if is_non_segmented_task(&task) {
            save_output_segments(
                &context,
                &task_id,
                &[merge_output.clone()],
                Some(merged_id),
                workflow_settings.segment_prefix.as_deref(),
            )?;
            // 单合并单元（无合并组配置）：若对应源视频设置了自定义上传分P标题则覆盖。
            let single_title =
                resolve_merge_groups(&context, &task_id, workflow_config.as_ref(), &sources)
                    .into_iter()
                    .next()
                    .and_then(|group| group.upload_part_title);
            if let Some(title) = single_title {
                apply_non_segmented_part_title(&context, &task_id, merged_id, &title)?;
            }
        }
    }
    if is_update_workflow && !workflow_settings.enable_segmentation {
        let (existing_count, max_order) = load_output_segment_stats(&context, &task_id)?;
        let name_start_index = resolve_update_name_start_index(
            &context,
            &task_id,
            existing_count,
            workflow_settings.segment_prefix.as_deref(),
        )?;
        append_output_segments(
            &context,
            &task_id,
            &[merge_output.clone()],
            Some(merged_id),
            workflow_settings.segment_prefix.as_deref(),
            max_order + 1,
            name_start_index,
        )?;
        // 编辑/更新工作流分段关闭：单源追加时若该源配置了自定义上传分P标题则覆盖。
        // 更新路径会把所有 sources 合并为一个分P，故仅在单源场景按源标题精确赋值，
        // 多源合并保持默认 P{index} 以免错配。
        if sources.len() == 1 {
            let source_titles = load_source_upload_titles(&context, &task_id);
            if let Some(title) = sources
                .first()
                .and_then(|source| source_titles.get(&source.input_path))
            {
                apply_non_segmented_part_title(&context, &task_id, merged_id, title)?;
            }
        }
    }
    if let Err(err) = baidu_sync::enqueue_submission_sync(
        context.db.as_ref(),
        context.app_log_path.as_ref(),
        &task_id,
    ) {
        append_log(
            &context.app_log_path,
            &format!("baidu_sync_enqueue_fail task_id={} err={}", task_id, err),
        );
    }

    if !ensure_workflow_instance_latest(
        &context,
        &task_id,
        &workflow_instance_id,
        "PRE_WAITING_UPLOAD",
    )? {
        return Ok(());
    }

    update_submission_status(&context, &task_id, "WAITING_UPLOAD")?;
    let workflow_status = match load_integrated_download_stats(&context, &task_id)? {
        Some(stats) if stats.completed < stats.total => "VIDEO_DOWNLOADING",
        _ => "COMPLETED",
    };
    let _ = update_workflow_status(&context, &task_id, workflow_status, None, 100.0);
    Ok(())
}

/// 读取库里最新的字幕样式（全局一份）。读不到时退回调用方给的快照。
fn load_subtitle_style(context: &SubmissionContext, fallback: Value) -> Value {
    context
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT value FROM app_settings WHERE key = ?1",
                [crate::commands::anchor::SUBTITLE_STYLE_KEY],
                |row| row.get::<_, String>(0),
            )
        })
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .unwrap_or(fallback)
}

/// 生成参数指纹：边界、样式或字幕任一项变了都要重新生成成片
fn burn_fingerprint(
    style: &Value,
    cues: &[crate::commands::anchor::SubtitleCueInput],
    start: f64,
    end: f64,
) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(start.to_bits().to_le_bytes());
    hasher.update(end.to_bits().to_le_bytes());
    hasher.update(serde_json::to_vec(style).unwrap_or_default());
    for cue in cues {
        hasher.update(cue.start.to_bits().to_le_bytes());
        hasher.update(cue.end.to_bits().to_le_bytes());
        hasher.update(cue.text.as_bytes());
        hasher.update([0u8]);
    }
    hasher.finalize().iter().map(|b| format!("{:02x}", b)).collect()
}

/// 评阅页投稿的切片：先按切片当前起止从录播重新剪辑并压入字幕，再把源视频换成生成的文件。
/// 已烧录且参数未变时跳过；参数变了（改了字号/颜色/位置或字幕内容）会覆盖重新生成。
async fn burn_clip_subtitles(context: &SubmissionContext, task_id: &str, config: Option<&Value>) -> Result<(), String> {
    let Some(burn) = config.and_then(|c| c.get("subtitleBurn")) else {
        return Ok(());
    };
    let clip_id = burn.get("clipId").and_then(|v| v.as_i64()).ok_or("字幕烧录缺少切片编号")?;
    let cues: Vec<crate::commands::anchor::SubtitleCueInput> =
        serde_json::from_value(burn.get("cues").cloned().unwrap_or_default()).map_err(|err| format!("字幕数据无效: {}", err))?;
    // 以库里最新的样式为准，避免前端快照滞后导致烧录用旧字号
    let style = load_subtitle_style(context, burn.get("style").cloned().unwrap_or_default());
    // 边界一并纳入指纹：评阅页改过起止也要重新生成，否则会复用旧时长的成片
    let (bound_start, bound_end) = context
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT start_offset, end_offset FROM live_clip_item WHERE id = ?1",
                [clip_id],
                |row| Ok((row.get::<_, f64>(0)?, row.get::<_, f64>(1)?)),
            )
        })
        .map_err(|err| format!("读取切片边界失败: {}", err))?;
    let fingerprint = burn_fingerprint(&style, &cues, bound_start, bound_end);

    let current: Option<String> = context
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT source_file_path FROM task_source_video WHERE task_id = ?1 ORDER BY sort_order LIMIT 1",
                [task_id],
                |row| row.get(0),
            )
            .optional()
        })
        .map_err(|err| err.to_string())?;
    // 数据库可能存相对路径（配置了 local_path_prefix），读回来要还原成运行时可用的绝对路径
    let current_abs = current
        .as_deref()
        .map(|value| to_runtime_submission_path(context, value));
    let current_path = current_abs.as_deref().map(Path::new);
    let already_burned = current_path
        .and_then(|p| p.parent())
        .and_then(|p| p.file_name())
        .map(|name| name == "subtitled")
        .unwrap_or(false);
    if already_burned {
        // 指纹文件与成片同名，只是扩展名不同
        let stamp = current_path.map(|p| p.with_extension("burn.json"));
        let same = stamp
            .as_deref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|saved| saved.trim() == fingerprint)
            .unwrap_or(false);
        if same && current_path.map(|p| p.is_file()).unwrap_or(false) {
            append_log(&context.app_log_path, &format!("submission_subtitle_burn_skip task_id={} reason=unchanged", task_id));
            return Ok(());
        }
    }

    update_submission_status(context, task_id, "CLIPPING")?;
    let _ = update_workflow_status(context, task_id, "RUNNING", Some("CLIPPING"), 0.0);
    append_log(&context.app_log_path, &format!("submission_subtitle_burn_start task_id={} clip_id={} cues={}", task_id, clip_id, cues.len()));
    let db = context.db.clone();
    let (output, seconds) = tauri::async_runtime::spawn_blocking(move || crate::commands::anchor::render_subtitled(&db, clip_id, cues, &style))
        .await
        .map_err(|_| "字幕烧录任务异常".to_string())??;
    // 时长按秒向下取整，多加 1 秒避免截掉末尾（ffmpeg 读到文件尾即止）
    let end = seconds + 1;
    let end_time = format!("{:02}:{:02}:{:02}", end / 3600, end / 60 % 60, end % 60);
    context
        .db
        .with_conn(|conn| {
            conn.execute(
                "UPDATE task_source_video SET source_file_path = ?1, end_time = ?2 WHERE task_id = ?3",
                (to_stored_submission_path(context, &output), &end_time, task_id),
            )
        })
        .map_err(|err| err.to_string())?;
    // 记录本次烧录的参数指纹，供下次投稿判断是否需要重烧
    let stamp = Path::new(&output).with_extension("burn.json");
    if let Err(err) = std::fs::write(&stamp, &fingerprint) {
        append_log(&context.app_log_path, &format!("submission_subtitle_burn_stamp_fail task_id={} err={}", task_id, err));
    }
    append_log(&context.app_log_path, &format!("submission_subtitle_burn_done task_id={} output={} end={}", task_id, output, end_time));
    Ok(())
}

pub(super) async fn run_submission_workflow_guarded(context: SubmissionContext, task_id: String) {
    if let Err(err) = run_submission_workflow(context.clone(), task_id.clone()).await {
        if err == "Workflow cancelled" {
            if let Err(cleanup_err) = terminate_submission_ffmpeg_processes_async(&context, &task_id).await {
                append_log(
                    &context.app_log_path,
                    &format!(
                        "submission_workflow_cancel_cleanup_fail task_id={} err={}",
                        task_id, cleanup_err
                    ),
                );
            }
            append_log(
                &context.app_log_path,
                &format!(
                    "submission_workflow_cancelled task_id={} err={}",
                    task_id, err
                ),
            );
            return;
        }
        if let Err(cleanup_err) = terminate_submission_ffmpeg_processes_async(&context, &task_id).await {
            append_log(
                &context.app_log_path,
                &format!(
                    "submission_workflow_fail_cleanup_fail task_id={} err={}",
                    task_id, cleanup_err
                ),
            );
        }
        let _ = mark_submission_failure(&context, &task_id, None, &err);
        append_log(
            &context.app_log_path,
            &format!("submission_workflow_failed task_id={} err={}", task_id, err),
        );
    }
}

pub fn start_submission_workflow(
    db: Arc<Db>,
    app_log_path: Arc<PathBuf>,
    edit_upload_state: Arc<Mutex<EditUploadState>>,
    task_id: String,
) {
    let local_path_prefix = load_local_path_prefix(db.as_ref());
    let context = SubmissionContext {
        db,
        app_log_path,
        edit_upload_state,
        local_path_prefix,
    };
    tauri::async_runtime::spawn(async move {
        run_submission_workflow_guarded(context, task_id).await;
    });
}

pub(super) fn is_workflow_superseded_error(err: &str) -> bool {
    err == "WORKFLOW_SUPERSEDED"
}

pub(super) fn load_latest_workflow_config(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<Option<Value>, String> {
    load_effective_workflow_config_record(context, task_id)
        .map(|value| value.map(|(_, config)| config))
}

pub(super) fn update_integrated_relation_workflow_status(
    context: &SubmissionContext,
    task_id: &str,
    status: &str,
) -> Result<(), String> {
    let now = now_rfc3339();
    context
        .db
        .with_conn(|conn| {
            conn.execute(
                "UPDATE task_relations SET workflow_status = ?1, updated_at = ?2 \
                 WHERE submission_task_id = ?3 AND relation_type = 'INTEGRATED'",
                (status, &now, task_id),
            )?;
            Ok(())
        })
        .map_err(|err| err.to_string())
}

pub(super) fn load_workflow_status(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<Option<WorkflowStatusRecord>, String> {
    context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT status, current_step, progress, error_message FROM workflow_instances WHERE task_id = ?1 ORDER BY created_at DESC LIMIT 1",
      )?;
      let result = stmt
        .query_row([task_id], |row| {
          let progress: Option<f64> = row.get(2)?;
          Ok(WorkflowStatusRecord {
            status: row.get(0)?,
            current_step: row.get(1)?,
            progress: progress.unwrap_or(0.0),
            error_message: row.get(3)?,
          })
        })
        .ok();
      Ok(result)
    })
    .map_err(|err| err.to_string())
}

pub(super) fn load_latest_workflow_type(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<Option<String>, String> {
    context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT workflow_type FROM workflow_instances WHERE task_id = ?1 ORDER BY created_at DESC LIMIT 1",
      )?;
      let result = stmt.query_row([task_id], |row| row.get(0)).optional()?;
      Ok(result)
    })
    .map_err(|err| err.to_string())
}

pub(super) fn load_latest_workflow_runtime(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<Option<(String, String, Option<Value>)>, String> {
    context
        .db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT wi.instance_id, wi.workflow_type, wc.configuration_data \
         FROM workflow_instances wi \
         LEFT JOIN workflow_configurations wc ON wi.configuration_id = wc.config_id \
         WHERE wi.task_id = ?1 ORDER BY wi.created_at DESC LIMIT 1",
            )?;
            let row: Option<(String, String, Option<String>)> = stmt
                .query_row([task_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .optional()?;
            Ok(row)
        })
        .map_err(|err| err.to_string())
        .map(|row| {
            row.map(|(instance_id, workflow_type, config_raw)| {
                let config = config_raw.and_then(|raw| serde_json::from_str::<Value>(&raw).ok());
                (instance_id, workflow_type, config)
            })
        })
}

pub(super) fn is_workflow_instance_latest(
    context: &SubmissionContext,
    task_id: &str,
    workflow_instance_id: &str,
) -> Result<bool, String> {
    context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT instance_id FROM workflow_instances WHERE task_id = ?1 ORDER BY created_at DESC LIMIT 1",
      )?;
      let latest: Option<String> = stmt.query_row([task_id], |row| row.get(0)).optional()?;
      Ok(latest)
    })
    .map_err(|err| err.to_string())
    .map(|latest| {
      latest
        .as_deref()
        .map(|instance_id| instance_id == workflow_instance_id)
        .unwrap_or(false)
    })
}

pub(super) fn ensure_workflow_instance_latest(
    context: &SubmissionContext,
    task_id: &str,
    workflow_instance_id: &str,
    stage: &str,
) -> Result<bool, String> {
    if is_workflow_instance_latest(context, task_id, workflow_instance_id)? {
        return Ok(true);
    }
    append_log(
        &context.app_log_path,
        &format!(
            "submission_workflow_superseded task_id={} instance_id={} stage={}",
            task_id, workflow_instance_id, stage
        ),
    );
    Ok(false)
}

pub(super) fn reset_workflow_instances(context: &SubmissionContext, task_id: &str) -> Result<(), String> {
    terminate_submission_ffmpeg_processes(context, task_id)?;
    context
    .db
    .with_conn(|conn| {
      conn.execute(
        "DELETE FROM workflow_execution_logs WHERE instance_id IN (SELECT instance_id FROM workflow_instances WHERE task_id = ?1)",
        [task_id],
      )?;
      conn.execute(
        "DELETE FROM workflow_performance_metrics WHERE instance_id IN (SELECT instance_id FROM workflow_instances WHERE task_id = ?1)",
        [task_id],
      )?;
      conn.execute(
        "DELETE FROM workflow_steps WHERE instance_id IN (SELECT instance_id FROM workflow_instances WHERE task_id = ?1)",
        [task_id],
      )?;
      conn.execute("DELETE FROM workflow_instances WHERE task_id = ?1", [task_id])?;
      Ok(())
    })
    .map_err(|err| err.to_string())
}

pub(super) fn set_workflow_instance_status(
    context: &SubmissionContext,
    task_id: &str,
    status: &str,
) -> Result<(), String> {
    let now = now_rfc3339();
    let updated = context
        .db
        .with_conn(|conn| {
            let updated = conn.execute(
                "UPDATE workflow_instances SET status = ?1, updated_at = ?2 WHERE task_id = ?3",
                (status, &now, task_id),
            )?;
            Ok(updated)
        })
        .map_err(|err| err.to_string())?;

    if updated == 0 {
        return Err("Workflow instance not found".to_string());
    }

    Ok(())
}

pub(super) async fn wait_for_workflow_ready(context: &SubmissionContext, task_id: &str) -> Result<(), String> {
    loop {
        let status = load_workflow_status(context, task_id)?;
        if let Some(status) = status {
            if status.status == "CANCELLED" {
                update_submission_status(context, task_id, "CANCELLED")?;
                return Err("Workflow cancelled".to_string());
            }
            if status.status == "PAUSED" {
                sleep(Duration::from_secs(1)).await;
                continue;
            }
        }
        return Ok(());
    }
}

pub(super) fn load_workflow_settings(context: &SubmissionContext, task_id: &str) -> WorkflowSettings {
    let config_value = load_latest_workflow_config(context, task_id).ok().flatten();

    parse_workflow_settings(config_value)
}

pub(super) fn parse_workflow_settings(config: Option<Value>) -> WorkflowSettings {
    if let Some(config) = config {
        let segmentation = config.get("segmentationConfig");
        let enable_segmentation = segmentation
            .and_then(|value| value.get("enabled"))
            .and_then(|value| value.as_bool())
            .unwrap_or_else(|| {
                config
                    .get("enableSegmentation")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false)
            });

        let segment_duration_seconds = segmentation
            .and_then(|value| value.get("segmentDurationSeconds"))
            .and_then(|value| value.as_i64())
            .unwrap_or(133);
        let segment_prefix = config
            .get("segmentPrefix")
            .and_then(|value| value.as_str())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());

        return WorkflowSettings {
            enable_segmentation,
            segment_duration_seconds,
            segment_prefix,
        };
    }

    WorkflowSettings {
        enable_segmentation: false,
        segment_duration_seconds: 133,
        segment_prefix: None,
    }
}

pub(super) fn update_workflow_status(
    context: &SubmissionContext,
    task_id: &str,
    status: &str,
    current_step: Option<&str>,
    progress: f64,
) -> Result<(), String> {
    let now = now_rfc3339();
    context
    .db
    .with_conn(|conn| {
      conn.execute(
        "UPDATE workflow_instances SET status = ?1, current_step = ?2, progress = ?3, updated_at = ?4 WHERE task_id = ?5",
        (status, current_step, progress, &now, task_id),
      )?;
      Ok(())
    })
    .map_err(|err| err.to_string())
}

#[cfg(test)]
mod subtitle_burn_tests {
    use super::*;
    use crate::commands::anchor::SubtitleCueInput;

    fn cue(start: f64, end: f64, text: &str) -> SubtitleCueInput {
        SubtitleCueInput { start, end, text: text.to_string() }
    }

    #[test]
    fn fingerprint_changes_when_font_size_changes() {
        let cues = vec![cue(1.0, 3.0, "你好")];
        let small = serde_json::json!({ "fontSize": 54, "color": "#FFFFFF" });
        let large = serde_json::json!({ "fontSize": 78, "color": "#FFFFFF" });
        assert_ne!(
            burn_fingerprint(&small, &cues, 10.0, 40.0),
            burn_fingerprint(&large, &cues, 10.0, 40.0),
            "改字号必须改变指纹，否则不会重烧"
        );
    }

    #[test]
    fn fingerprint_changes_when_cues_change() {
        let style = serde_json::json!({ "fontSize": 54 });
        assert_ne!(
            burn_fingerprint(&style, &[cue(1.0, 3.0, "你好")], 10.0, 40.0),
            burn_fingerprint(&style, &[cue(1.0, 3.0, "再见")], 10.0, 40.0),
            "改字幕文字必须改变指纹"
        );
    }

    #[test]
    fn fingerprint_changes_when_bounds_change() {
        let style = serde_json::json!({ "fontSize": 54 });
        let cues = vec![cue(1.0, 3.0, "你好")];
        assert_ne!(
            burn_fingerprint(&style, &cues, 10.0, 40.0),
            burn_fingerprint(&style, &cues, 8.0, 44.0),
            "改边界必须改变指纹，否则会复用旧时长的成片"
        );
    }

    #[test]
    fn fingerprint_stable_for_identical_input() {
        let style = serde_json::json!({ "fontSize": 54, "x": 50.0, "y": 82.0 });
        let first = burn_fingerprint(&style, &[cue(1.0, 3.0, "你好"), cue(3.5, 5.0, "世界")], 10.0, 40.0);
        let second = burn_fingerprint(&style, &[cue(1.0, 3.0, "你好"), cue(3.5, 5.0, "世界")], 10.0, 40.0);
        assert_eq!(first, second, "参数不变时指纹必须一致，才能跳过重烧");
    }
}
