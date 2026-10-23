//! 重新投稿、重新分段、按分P重传以及从网盘恢复后的续跑。

use super::*;

pub(super) fn parse_reprocess_mode(mode: Option<&str>) -> ReprocessMode {
    let normalized = mode.unwrap_or("").trim().to_ascii_uppercase();
    match normalized.as_str() {
        "SPECIFIED" => ReprocessMode::Specified,
        "MERGE_ALL" => ReprocessMode::MergeAll,
        "FULL_REPROCESS" => ReprocessMode::FullReprocess,
        _ => ReprocessMode::Legacy,
    }
}

pub(super) fn reprocess_mode_to_str(mode: ReprocessMode) -> &'static str {
    match mode {
        ReprocessMode::Specified => "SPECIFIED",
        ReprocessMode::MergeAll => "MERGE_ALL",
        ReprocessMode::FullReprocess => "FULL_REPROCESS",
        ReprocessMode::Legacy => "LEGACY",
    }
}

pub(super) fn apply_reprocess_metadata(config: &mut Value, mode: ReprocessMode, merged_video_id: Option<i64>) {
    if !config.is_object() {
        *config = Value::Object(Map::new());
    }
    let Some(map) = config.as_object_mut() else {
        return;
    };
    map.insert(
        "reprocessMode".to_string(),
        Value::String(reprocess_mode_to_str(mode).to_string()),
    );
    if let Some(merged_id) = merged_video_id {
        map.insert(
            "reprocessMergedId".to_string(),
            Value::Number(Number::from(merged_id)),
        );
    } else {
        map.remove("reprocessMergedId");
    }
}

pub(super) fn load_reprocess_metadata(config: Option<&Value>) -> (ReprocessMode, Option<i64>) {
    let Some(config) = config else {
        return (ReprocessMode::Legacy, None);
    };
    let mode = config
        .get("reprocessMode")
        .and_then(|value| value.as_str())
        .map(|value| parse_reprocess_mode(Some(value)))
        .unwrap_or(ReprocessMode::Legacy);
    let merged_id = config
        .get("reprocessMergedId")
        .and_then(|value| value.as_i64());
    (mode, merged_id)
}

pub(super) fn apply_resegment_terminal_metadata(config: &mut Value, started_from_terminal: bool) {
    if !config.is_object() {
        *config = Value::Object(Map::new());
    }
    let Some(map) = config.as_object_mut() else {
        return;
    };
    map.insert(
        "resegmentStartedFromTerminal".to_string(),
        Value::Bool(started_from_terminal),
    );
}

pub(super) fn load_resegment_terminal_metadata(config: Option<&Value>) -> bool {
    let Some(config) = config else {
        return true;
    };
    config
        .get("resegmentStartedFromTerminal")
        .and_then(|value| value.as_bool())
        .unwrap_or(true)
}

#[tauri::command]
pub async fn submission_repost(
    state: State<'_, AppState>,
    request: SubmissionRepostRequest,
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
    if should_block_submission_retry(&detail.task) {
        return Ok(ApiResponse::error(submission_retry_block_message()));
    }
    let mode = parse_reprocess_mode(request.mode.as_deref());
    if mode == ReprocessMode::FullReprocess {
        let mut workflow_config = match detail.workflow_config {
            Some(ref config) => config.clone(),
            None => return Ok(ApiResponse::error("未找到工作流配置")),
        };
        let integrate_current_bvid = request.integrate_current_bvid;
        apply_reprocess_metadata(&mut workflow_config, mode, request.merged_video_id);
        apply_integrate_current_bvid(&mut workflow_config, integrate_current_bvid);
        apply_media_process_mode(&mut workflow_config, MediaProcessMode::CopyOnly);
        let workflow_config = strip_update_sources(&workflow_config);
        if integrate_current_bvid {
            let has_bvid = detail
                .task
                .bvid
                .as_deref()
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false);
            if !has_bvid {
                return Ok(ApiResponse::error("当前任务没有BV号，无法集成投稿"));
            }
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
        append_log(
            &state.app_log_path,
            &format!(
                "submission_repost_start task_id={} mode=FULL_REPROCESS type={}",
                task_id,
                if integrate_current_bvid {
                    "UPDATE"
                } else {
                    "NEW"
                }
            ),
        );
        if detail.source_videos.is_empty() {
            return handle_full_reprocess_missing_sources(
                &state,
                &context,
                &task_id,
                &workflow_config,
                integrate_current_bvid,
                &[],
            )
            .await;
        }
        let missing_sources = collect_missing_source_files(&detail.source_videos);
        if !missing_sources.is_empty() {
            return handle_full_reprocess_missing_sources(
                &state,
                &context,
                &task_id,
                &workflow_config,
                integrate_current_bvid,
                &missing_sources,
            )
            .await;
        }
        let _ = reset_submission_for_repost(
            &context,
            &state.app_log_path,
            &task_id,
            &workflow_config,
            if integrate_current_bvid {
                "VIDEO_UPDATE"
            } else {
                "VIDEO_SUBMISSION"
            },
            !integrate_current_bvid,
            true,
        )?;
        start_submission_workflow(
            context.db.clone(),
            context.app_log_path.clone(),
            context.edit_upload_state.clone(),
            task_id,
        );
        return Ok(ApiResponse::success("全部重新投稿已启动".to_string()));
    }
    if mode != ReprocessMode::Legacy {
        let mut workflow_config = match detail.workflow_config {
            Some(ref config) => config.clone(),
            None => return Ok(ApiResponse::error("未找到工作流配置")),
        };
        apply_reprocess_metadata(&mut workflow_config, mode, request.merged_video_id);
        let integrate_current_bvid = request.integrate_current_bvid;
        apply_integrate_current_bvid(&mut workflow_config, integrate_current_bvid);
        apply_media_process_mode(&mut workflow_config, MediaProcessMode::CopyOnly);
        if integrate_current_bvid {
            let has_bvid = detail
                .task
                .bvid
                .as_deref()
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false);
            if !has_bvid {
                return Ok(ApiResponse::error("当前任务没有BV号，无法集成投稿"));
            }
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
        append_log(
            &state.app_log_path,
            &format!(
                "submission_repost_start task_id={} mode={} type={}",
                task_id,
                if mode == ReprocessMode::MergeAll {
                    "MERGE_ALL"
                } else {
                    "SPECIFIED"
                },
                if integrate_current_bvid {
                    "UPDATE"
                } else {
                    "NEW"
                }
            ),
        );
        let mut specified_merged = None;
        let mut merge_all_list = Vec::new();
        let workflow_settings = load_workflow_settings(&context, &task_id);
        let enable_segmentation = workflow_settings.enable_segmentation;
        let segment_seconds = workflow_settings.segment_duration_seconds.max(1);
        let should_segment = integrate_current_bvid || enable_segmentation;
        let reprocess_source_paths = resolve_reprocess_source_paths(
            &context,
            &detail,
            &workflow_config,
            mode,
            request.merged_video_id,
        );
        let sources_ready = !reprocess_source_paths.is_empty()
            && collect_missing_source_paths(&reprocess_source_paths).is_empty();
        let source_override = if mode == ReprocessMode::Specified {
            Some(reprocess_source_paths.clone())
        } else {
            None
        };
        let base_dir = resolve_submission_base_dir(&context, &task_id);
        let output_dir = base_dir
            .join("repost")
            .join(sanitize_filename(&format!("repost_{}", now_rfc3339())))
            .join("output");
        if mode == ReprocessMode::Specified {
            let mut merged = match resolve_target_merged_video(
                &context,
                &task_id,
                request.merged_video_id,
                &state.app_log_path,
            ) {
                Ok(merged) => merged,
                Err(_) => {
                    return handle_repost_missing_assets(
                        &state,
                        &context,
                        &detail,
                        &task_id,
                        &workflow_config,
                        integrate_current_bvid,
                        FALLBACK_REASON_REPOST_MISSING_MERGED,
                        source_override.clone(),
                    )
                    .await;
                }
            };
            let mut merged_path = merged.video_path.clone().unwrap_or_default();
            if merged_path.trim().is_empty() || !PathBuf::from(&merged_path).exists() {
                if sources_ready {
                    return handle_repost_missing_assets(
                        &state,
                        &context,
                        &detail,
                        &task_id,
                        &workflow_config,
                        integrate_current_bvid,
                        FALLBACK_REASON_REPOST_MISSING_MERGED,
                        source_override.clone(),
                    )
                    .await;
                }
                match try_restore_merged_from_baidu(
                    &context,
                    &state.app_log_path,
                    &merged,
                    &base_dir,
                )
                .await?
                {
                    BaiduRestoreResult::Ready(restored) => {
                        merged_path = restored.to_string_lossy().to_string();
                        merged.video_path = Some(merged_path.clone());
                    }
                    BaiduRestoreResult::Queued => {
                        if let Err(err) = prepare_workflow_for_baidu_restore(
                            &context,
                            &state.app_log_path,
                            &task_id,
                            &workflow_config,
                            if integrate_current_bvid {
                                "VIDEO_UPDATE"
                            } else {
                                "VIDEO_SUBMISSION"
                            },
                            !integrate_current_bvid,
                        ) {
                            return Ok(ApiResponse::error(format!("准备网盘恢复失败: {}", err)));
                        }
                        return Ok(ApiResponse::success(
                            "合并视频缺失，已创建网盘下载任务，下载完成后自动重新投稿".to_string(),
                        ));
                    }
                    BaiduRestoreResult::NotBound => {}
                }
            }
            if merged_path.trim().is_empty() {
                return handle_repost_missing_assets(
                    &state,
                    &context,
                    &detail,
                    &task_id,
                    &workflow_config,
                    integrate_current_bvid,
                    FALLBACK_REASON_REPOST_MISSING_MERGED,
                    source_override.clone(),
                )
                .await;
            }
            let merged_path_buf = PathBuf::from(merged_path.clone());
            if !merged_path_buf.exists() {
                return handle_repost_missing_assets(
                    &state,
                    &context,
                    &detail,
                    &task_id,
                    &workflow_config,
                    integrate_current_bvid,
                    FALLBACK_REASON_REPOST_MISSING_MERGED,
                    source_override.clone(),
                )
                .await;
            }
            specified_merged = Some(merged);
        } else {
            let mut merged_videos = match load_merged_videos_by_task(&context, &task_id) {
                Ok(list) => list,
                Err(err) => return Ok(ApiResponse::error(err)),
            };
            if merged_videos.is_empty() {
                return handle_repost_missing_assets(
                    &state,
                    &context,
                    &detail,
                    &task_id,
                    &workflow_config,
                    integrate_current_bvid,
                    FALLBACK_REASON_REPOST_MISSING_MERGED,
                    source_override.clone(),
                )
                .await;
            }
            let mut fallback_to_source_download = false;
            for merged in &mut merged_videos {
                let mut path = merged.video_path.clone().unwrap_or_default();
                if path.trim().is_empty() || !PathBuf::from(&path).exists() {
                    match try_restore_merged_from_baidu(
                        &context,
                        &state.app_log_path,
                        merged,
                        &base_dir,
                    )
                    .await?
                    {
                        BaiduRestoreResult::Ready(restored) => {
                            path = restored.to_string_lossy().to_string();
                            merged.video_path = Some(path.clone());
                        }
                        BaiduRestoreResult::Queued => {
                            if let Err(err) = prepare_workflow_for_baidu_restore(
                                &context,
                                &state.app_log_path,
                                &task_id,
                                &workflow_config,
                                if integrate_current_bvid {
                                    "VIDEO_UPDATE"
                                } else {
                                    "VIDEO_SUBMISSION"
                                },
                                !integrate_current_bvid,
                            ) {
                                return Ok(ApiResponse::error(format!(
                                    "准备网盘恢复失败: {}",
                                    err
                                )));
                            }
                            let display_name = merged
                                .file_name
                                .as_deref()
                                .filter(|value| !value.trim().is_empty())
                                .or_else(|| merged.video_path.as_deref())
                                .unwrap_or("未知合并视频");
                            return Ok(ApiResponse::success(format!(
                                "{} 缺失，已创建网盘下载任务，下载完成后自动重新投稿",
                                display_name
                            )));
                        }
                        BaiduRestoreResult::NotBound => {
                            fallback_to_source_download = true;
                            break;
                        }
                    }
                }
                if path.trim().is_empty() {
                    fallback_to_source_download = true;
                    break;
                }
                let path_buf = PathBuf::from(&path);
                if !path_buf.exists() {
                    fallback_to_source_download = true;
                    break;
                }
            }
            if fallback_to_source_download {
                return handle_repost_missing_assets(
                    &state,
                    &context,
                    &detail,
                    &task_id,
                    &workflow_config,
                    integrate_current_bvid,
                    FALLBACK_REASON_REPOST_MISSING_MERGED,
                    source_override.clone(),
                )
                .await;
            }
            merge_all_list = merged_videos;
        }

        if let Err(err) = clear_edit_upload_segments_by_task(&context, &task_id) {
            append_log(
                &state.app_log_path,
                &format!(
                    "submission_repost_clear_cache_fail task_id={} err={}",
                    task_id, err
                ),
            );
        }
        if let Err(err) = reset_workflow_instances(&context, &task_id) {
            return Ok(ApiResponse::error(format!("重置工作流失败: {}", err)));
        }
        let workflow_type = if integrate_current_bvid {
            "VIDEO_UPDATE"
        } else {
            "VIDEO_SUBMISSION"
        };
        let (instance_id, _) = match create_workflow_instance_for_task_with_type(
            context.db.as_ref(),
            &task_id,
            &workflow_config,
            workflow_type,
        ) {
            Ok(value) => value,
            Err(err) => return Ok(ApiResponse::error(format!("创建工作流失败: {}", err))),
        };
        let now = now_rfc3339();
        let update_result = context.db.with_conn(|conn| {
      if integrate_current_bvid {
        conn.execute(
          "UPDATE submission_task SET status = 'SEGMENTING', remote_state = NULL, remote_state_desc = NULL, reject_reason = NULL, updated_at = ?1 WHERE task_id = ?2",
          (&now, &task_id),
        )?;
      } else {
        conn.execute(
          "UPDATE submission_task SET status = 'SEGMENTING', bvid = NULL, aid = NULL, remote_state = NULL, remote_state_desc = NULL, reject_reason = NULL, updated_at = ?1 WHERE task_id = ?2",
          (&now, &task_id),
        )?;
      }
      Ok(())
    });
        if let Err(err) = update_result {
            return Ok(ApiResponse::error(format!("重置任务状态失败: {}", err)));
        }
        let _ = context.db.with_conn(|conn| {
      conn.execute(
        "UPDATE task_relations SET workflow_instance_id = ?1, updated_at = ?2 WHERE submission_task_id = ?3 AND relation_type = 'INTEGRATED'",
        (&instance_id, &now, &task_id),
      )?;
      Ok(())
    });

        if mode == ReprocessMode::Specified {
            let merged = match specified_merged {
                Some(value) => value,
                None => return Ok(ApiResponse::error("未找到合并视频")),
            };
            let merged_path = merged.video_path.clone().unwrap_or_default();
            let merged_path_buf = PathBuf::from(merged_path.clone());
            let segment_prefix = detail.task.segment_prefix.clone();
            let context_clone = context.clone();
            let task_id_clone = task_id.clone();
            let merged_path_clone = merged_path_buf.clone();
            let output_dir_clone = output_dir.clone();
            let app_log_path = state.app_log_path.clone();
            let merged_id = merged.id;
            tauri::async_runtime::spawn(async move {
                let _ = update_workflow_status(
                    &context_clone,
                    &task_id_clone,
                    "RUNNING",
                    Some("SEGMENTING"),
                    70.0,
                );
                if !should_segment {
                    let (replaced_merged_id, replaced_merged_path) =
                        match recreate_selected_merged_for_repost(
                            &context_clone,
                            &task_id_clone,
                            merged_id,
                            merged_path_clone.as_path(),
                            &output_dir_clone,
                        ) {
                            Ok(value) => value,
                            Err(err) => {
                                let _ = mark_submission_failure(
                                    &context_clone,
                                    &task_id_clone,
                                    Some("SEGMENTING"),
                                    &format!("重新投稿替换合并视频失败: {}", err),
                                );
                                append_log(
                                    app_log_path.as_ref(),
                                    &format!(
                  "submission_repost_replace_merged_fail task_id={} merged_id={} err={}",
                  task_id_clone, merged_id, err
                ),
                                );
                                return;
                            }
                        };
                    if let Err(err) = replace_segments_for_merged(
                        &context_clone,
                        &task_id_clone,
                        replaced_merged_id,
                        &[replaced_merged_path],
                        segment_prefix.as_deref(),
                    ) {
                        let _ = mark_submission_failure(
                            &context_clone,
                            &task_id_clone,
                            Some("SEGMENTING"),
                            &format!("重新投稿保存分段失败: {}", err),
                        );
                        append_log(
                            app_log_path.as_ref(),
                            &format!(
                                "submission_repost_save_fail task_id={} err={}",
                                task_id_clone, err
                            ),
                        );
                        return;
                    }
                    if !integrate_current_bvid {
                        if let Err(err) =
                            reset_segments_for_new_bvid(&context_clone, &task_id_clone)
                        {
                            let _ = mark_submission_failure(
                                &context_clone,
                                &task_id_clone,
                                Some("SEGMENTING"),
                                &format!("重新投稿重置分段元数据失败: {}", err),
                            );
                            append_log(
                                app_log_path.as_ref(),
                                &format!(
                                    "submission_repost_reset_segments_fail task_id={} err={}",
                                    task_id_clone, err
                                ),
                            );
                            return;
                        }
                    }
                    finalize_reprocess_success(
                        &context_clone,
                        &task_id_clone,
                        "submission_repost_ok",
                    );
                    return;
                }

                let (new_merged_id, new_merged_path) = match recreate_selected_merged_for_repost(
                    &context_clone,
                    &task_id_clone,
                    merged_id,
                    merged_path_clone.as_path(),
                    &output_dir_clone,
                ) {
                    Ok(value) => value,
                    Err(err) => {
                        let _ = mark_submission_failure(
                            &context_clone,
                            &task_id_clone,
                            Some("SEGMENTING"),
                            &format!("重新投稿替换合并视频失败: {}", err),
                        );
                        append_log(
                            app_log_path.as_ref(),
                            &format!(
                "submission_repost_replace_merged_fail task_id={} merged_id={} err={}",
                task_id_clone, merged_id, err
              ),
                        );
                        return;
                    }
                };

                let outputs = if enable_segmentation {
                    match tauri::async_runtime::spawn_blocking(move || {
                        segment_file(&new_merged_path, &output_dir_clone, segment_seconds)
                    })
                    .await
                    {
                        Ok(result) => result,
                        Err(_) => Err("Failed to segment video".to_string()),
                    }
                } else {
                    Ok(vec![new_merged_path.clone()])
                };
                let outputs = match outputs {
                    Ok(list) => list,
                    Err(err) => {
                        let _ = mark_submission_failure(
                            &context_clone,
                            &task_id_clone,
                            Some("SEGMENTING"),
                            &format!("重新投稿分段失败: {}", err),
                        );
                        append_log(
                            app_log_path.as_ref(),
                            &format!(
                                "submission_repost_segment_fail task_id={} err={}",
                                task_id_clone, err
                            ),
                        );
                        return;
                    }
                };
                if outputs.is_empty() {
                    let _ = mark_submission_failure(
                        &context_clone,
                        &task_id_clone,
                        Some("SEGMENTING"),
                        "重新投稿未生成任何分段文件",
                    );
                    append_log(
                        app_log_path.as_ref(),
                        &format!("submission_repost_empty_outputs task_id={}", task_id_clone),
                    );
                    return;
                }
                if let Err(err) = replace_segments_for_merged(
                    &context_clone,
                    &task_id_clone,
                    new_merged_id,
                    &outputs,
                    segment_prefix.as_deref(),
                ) {
                    let _ = mark_submission_failure(
                        &context_clone,
                        &task_id_clone,
                        Some("SEGMENTING"),
                        &format!("重新投稿保存分段失败: {}", err),
                    );
                    append_log(
                        app_log_path.as_ref(),
                        &format!(
                            "submission_repost_save_fail task_id={} err={}",
                            task_id_clone, err
                        ),
                    );
                    return;
                }
                if !integrate_current_bvid {
                    if let Err(err) = reset_segments_for_new_bvid(&context_clone, &task_id_clone) {
                        let _ = mark_submission_failure(
                            &context_clone,
                            &task_id_clone,
                            Some("SEGMENTING"),
                            &format!("重新投稿重置分段元数据失败: {}", err),
                        );
                        append_log(
                            app_log_path.as_ref(),
                            &format!(
                                "submission_repost_reset_segments_fail task_id={} err={}",
                                task_id_clone, err
                            ),
                        );
                        return;
                    }
                }
                finalize_reprocess_success(&context_clone, &task_id_clone, "submission_repost_ok");
            });
            return Ok(ApiResponse::success("重新投稿已启动".to_string()));
        }
        let merge_all_sources = collect_sources_for_merge_all(&context, &task_id, &merge_all_list);
        let mut merge_inputs = Vec::with_capacity(merge_all_list.len());
        for merged in &merge_all_list {
            let path = merged.video_path.clone().unwrap_or_default();
            if path.trim().is_empty() {
                return Ok(ApiResponse::error("合并视频路径为空"));
            }
            let path_buf = PathBuf::from(path);
            if !path_buf.exists() {
                return Ok(ApiResponse::error("合并视频文件不存在"));
            }
            merge_inputs.push(path_buf);
        }
        let merge_output = build_merge_output_path_for_task(&base_dir, &detail.task);
        let context_clone = context.clone();
        let task_id_clone = task_id.clone();
        let output_dir_clone = output_dir.clone();
        let merge_output_for_merge = merge_output.clone();
        let merge_output_for_segment = merge_output.clone();
        let merge_output_for_save = merge_output.clone();
        let segment_prefix = detail.task.segment_prefix.clone();
        let app_log_path = state.app_log_path.clone();
        tauri::async_runtime::spawn(async move {
            let _ = update_workflow_status(
                &context_clone,
                &task_id_clone,
                "RUNNING",
                Some("SEGMENTING"),
                70.0,
            );
            let merge_inputs_clone = merge_inputs.clone();
            let merge_result = tauri::async_runtime::spawn_blocking(move || {
                merge_files(&merge_inputs_clone, &merge_output_for_merge)
            })
            .await;
            match merge_result {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    let _ = mark_submission_failure(
                        &context_clone,
                        &task_id_clone,
                        Some("SEGMENTING"),
                        &format!("重新投稿合并失败: {}", err),
                    );
                    append_log(
                        app_log_path.as_ref(),
                        &format!(
                            "submission_repost_merge_fail task_id={} err={}",
                            task_id_clone, err
                        ),
                    );
                    return;
                }
                Err(err) => {
                    let _ = mark_submission_failure(
                        &context_clone,
                        &task_id_clone,
                        Some("SEGMENTING"),
                        &format!("重新投稿合并失败: {}", err),
                    );
                    append_log(
                        app_log_path.as_ref(),
                        &format!(
                            "submission_repost_merge_fail task_id={} err={}",
                            task_id_clone, err
                        ),
                    );
                    return;
                }
            };
            let segment_outputs = if should_segment {
                if enable_segmentation {
                    let merge_output_for_segment_clone = merge_output_for_segment.clone();
                    match tauri::async_runtime::spawn_blocking(move || {
                        segment_file_with_options(
                            &merge_output_for_segment_clone,
                            &output_dir_clone,
                            segment_seconds,
                            false,
                            false,
                        )
                    })
                    .await
                    {
                        Ok(result) => result,
                        Err(_) => Err("Failed to segment video".to_string()),
                    }
                } else {
                    Ok(vec![merge_output_for_segment.clone()])
                }
            } else {
                Ok(Vec::new())
            };
            match segment_outputs {
                Ok(outputs) => {
                    if should_segment && outputs.is_empty() {
                        let _ = mark_submission_failure(
                            &context_clone,
                            &task_id_clone,
                            Some("SEGMENTING"),
                            "重新投稿未生成任何分段文件",
                        );
                        append_log(
                            app_log_path.as_ref(),
                            &format!("submission_repost_empty_outputs task_id={}", task_id_clone),
                        );
                        return;
                    }
                    if let Err(err) = context_clone.db.with_conn(|conn| {
                        conn.execute(
                            "DELETE FROM merged_video WHERE task_id = ?1",
                            [&task_id_clone],
                        )?;
                        conn.execute(
                            "DELETE FROM task_output_segment WHERE task_id = ?1",
                            [&task_id_clone],
                        )?;
                        Ok(())
                    }) {
                        let _ = mark_submission_failure(
                            &context_clone,
                            &task_id_clone,
                            Some("SEGMENTING"),
                            &format!("重新投稿清理旧合并/分段记录失败: {}", err),
                        );
                        append_log(
                            app_log_path.as_ref(),
                            &format!(
                                "submission_repost_cleanup_fail task_id={} err={}",
                                task_id_clone, err
                            ),
                        );
                        return;
                    }
                    let merged_id = match save_merged_video(
                        &context_clone,
                        &task_id_clone,
                        &merge_output_for_save,
                    ) {
                        Ok(merged_id) => merged_id,
                        Err(err) => {
                            let _ = mark_submission_failure(
                                &context_clone,
                                &task_id_clone,
                                Some("SEGMENTING"),
                                &format!("重新投稿保存合并视频失败: {}", err),
                            );
                            append_log(
                                app_log_path.as_ref(),
                                &format!(
                                    "submission_repost_save_fail task_id={} err={}",
                                    task_id_clone, err
                                ),
                            );
                            return;
                        }
                    };
                    if let Err(err) = save_merged_source_bindings(
                        &context_clone,
                        &task_id_clone,
                        merged_id,
                        &merge_all_sources,
                    ) {
                        append_log(
                            app_log_path.as_ref(),
                            &format!(
                "submission_repost_bind_sources_fail task_id={} merged_id={} err={}",
                task_id_clone, merged_id, err
              ),
                        );
                    }
                    if should_segment {
                        if let Err(err) = save_output_segments(
                            &context_clone,
                            &task_id_clone,
                            &outputs,
                            Some(merged_id),
                            segment_prefix.as_deref(),
                        ) {
                            let _ = mark_submission_failure(
                                &context_clone,
                                &task_id_clone,
                                Some("SEGMENTING"),
                                &format!("重新投稿保存分段失败: {}", err),
                            );
                            append_log(
                                app_log_path.as_ref(),
                                &format!(
                                    "submission_repost_save_fail task_id={} err={}",
                                    task_id_clone, err
                                ),
                            );
                            return;
                        }
                    }
                    finalize_reprocess_success(
                        &context_clone,
                        &task_id_clone,
                        "submission_repost_ok",
                    );
                }
                Err(err) => {
                    let _ = mark_submission_failure(
                        &context_clone,
                        &task_id_clone,
                        Some("SEGMENTING"),
                        &format!("重新投稿分段失败: {}", err),
                    );
                    append_log(
                        app_log_path.as_ref(),
                        &format!(
                            "submission_repost_segment_fail task_id={} err={}",
                            task_id_clone, err
                        ),
                    );
                }
            }
        });
        return Ok(ApiResponse::success("重新投稿已启动".to_string()));
    }
    let workflow_config = match detail.workflow_config {
        Some(config) => config,
        None => return Ok(ApiResponse::error("未找到工作流配置")),
    };
    let integrate_current_bvid = request.integrate_current_bvid;
    if integrate_current_bvid {
        let has_bvid = detail
            .task
            .bvid
            .as_deref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false);
        if !has_bvid {
            return Ok(ApiResponse::error("当前任务没有BV号，无法集成投稿"));
        }
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

    if detail.source_videos.is_empty() {
        if let Some(response) = try_repost_with_existing_merged_assets(
            &state,
            &context,
            &task_id,
            &workflow_config,
            integrate_current_bvid,
        )
        .await?
        {
            return Ok(response);
        }
        return Ok(ApiResponse::error("请至少添加一个源视频"));
    }

    let missing_sources = collect_missing_source_files(&detail.source_videos);
    if !missing_sources.is_empty() {
        if let Some(response) = try_repost_with_existing_merged_assets(
            &state,
            &context,
            &task_id,
            &workflow_config,
            integrate_current_bvid,
        )
        .await?
        {
            return Ok(response);
        }
        return enqueue_missing_source_downloads_for_repost(
            &state,
            &context,
            &task_id,
            &workflow_config,
            integrate_current_bvid,
            &missing_sources,
            "源视频缺失",
            false,
        )
        .await;
    }

    let _ = reset_submission_for_repost(
        &context,
        &state.app_log_path,
        &task_id,
        &workflow_config,
        if integrate_current_bvid {
            "VIDEO_UPDATE"
        } else {
            "VIDEO_SUBMISSION"
        },
        !integrate_current_bvid,
        false,
    )?;
    start_submission_workflow(
        context.db.clone(),
        context.app_log_path.clone(),
        context.edit_upload_state.clone(),
        task_id,
    );
    Ok(ApiResponse::success("重新投稿已启动".to_string()))
}

pub(super) fn collect_missing_source_files(sources: &[TaskSourceVideoRecord]) -> Vec<String> {
    let mut missing = Vec::new();
    for source in sources {
        if source.source_file_path.trim().is_empty() {
            continue;
        }
        let path = Path::new(&source.source_file_path);
        if !path.exists() {
            missing.push(source.source_file_path.clone());
        }
    }
    missing
}

pub(super) fn collect_missing_source_paths(paths: &[String]) -> Vec<String> {
    let mut missing = Vec::new();
    for path in paths {
        if path.trim().is_empty() {
            continue;
        }
        let path_buf = Path::new(path);
        if !path_buf.exists() {
            missing.push(path.clone());
        }
    }
    missing
}

pub(super) fn resolve_reprocess_source_paths(
    context: &SubmissionContext,
    detail: &SubmissionTaskDetail,
    config: &Value,
    mode: ReprocessMode,
    merged_id: Option<i64>,
) -> Vec<String> {
    if mode == ReprocessMode::Specified {
        if let Some(merged_id) = merged_id {
            if let Ok(paths) = load_merged_source_paths(context, &detail.task.task_id, merged_id) {
                if !paths.is_empty() {
                    return paths;
                }
            }
        }
        let update_sources = extract_update_source_paths_from_config(config);
        if !update_sources.is_empty() {
            return update_sources;
        }
    }
    extract_source_paths_from_task(detail)
}

pub(super) async fn handle_repost_missing_assets(
    state: &State<'_, AppState>,
    context: &SubmissionContext,
    detail: &SubmissionTaskDetail,
    task_id: &str,
    workflow_config: &Value,
    integrate_current_bvid: bool,
    reason: &str,
    source_paths_override: Option<Vec<String>>,
) -> Result<ApiResponse<String>, String> {
    let use_override = source_paths_override
        .as_ref()
        .map(|list| !list.is_empty())
        .unwrap_or(false);
    let workflow_config = if use_override {
        workflow_config.clone()
    } else {
        strip_update_sources(workflow_config)
    };
    let source_paths =
        source_paths_override.unwrap_or_else(|| extract_source_paths_from_task(detail));
    let missing_sources = collect_missing_source_paths(&source_paths);
    append_log(
    &state.app_log_path,
    &format!(
      "submission_repost_fallback task_id={} reason={} workflow_type={} use_override={} source_count={} missing_count={}",
      task_id,
      reason,
      if integrate_current_bvid { "VIDEO_UPDATE" } else { "VIDEO_SUBMISSION" },
      use_override,
      source_paths.len(),
      missing_sources.len()
    ),
  );
    if !missing_sources.is_empty() {
        return enqueue_missing_source_downloads_for_repost(
            state,
            context,
            task_id,
            &workflow_config,
            integrate_current_bvid,
            &missing_sources,
            reason,
            true,
        )
        .await;
    }
    append_log(
        &state.app_log_path,
        &format!(
            "submission_repost_fallback_no_download_needed task_id={} reason={}",
            task_id, reason
        ),
    );
    let _ = reset_submission_for_repost(
        context,
        &state.app_log_path,
        task_id,
        &workflow_config,
        if integrate_current_bvid {
            "VIDEO_UPDATE"
        } else {
            "VIDEO_SUBMISSION"
        },
        !integrate_current_bvid,
        true,
    )?;
    start_submission_workflow(
        context.db.clone(),
        context.app_log_path.clone(),
        context.edit_upload_state.clone(),
        task_id.to_string(),
    );
    Ok(ApiResponse::success(format!(
        "{}，已自动重新剪辑并重新投稿",
        reason
    )))
}

#[tauri::command]
pub async fn submission_repost_transcode(
    state: State<'_, AppState>,
    request: SubmissionTranscodeRepostRequest,
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
    if should_block_submission_retry(&detail.task) {
        return Ok(ApiResponse::error(submission_retry_block_message()));
    }
    let mut workflow_config = match detail.workflow_config {
        Some(ref config) => config.clone(),
        None => return Ok(ApiResponse::error("未找到工作流配置")),
    };
    let integrate_current_bvid = request.integrate_current_bvid;
    if let Err(err) = update_baidu_sync_config(
        &context,
        &task_id,
        request.baidu_sync_enabled,
        normalize_optional_text(request.baidu_sync_path),
        normalize_optional_text(request.baidu_sync_filename),
    ) {
        append_log(
            &state.app_log_path,
            &format!(
                "submission_repost_transcode_baidu_sync_update_fail task_id={} err={}",
                task_id, err
            ),
        );
    }
    apply_reprocess_metadata(&mut workflow_config, ReprocessMode::FullReprocess, None);
    apply_integrate_current_bvid(&mut workflow_config, integrate_current_bvid);
    apply_media_process_mode(
        &mut workflow_config,
        MediaProcessMode::ClipAndMergeTranscode,
    );
    let workflow_config = strip_update_sources(&workflow_config);
    if integrate_current_bvid {
        let has_bvid = detail
            .task
            .bvid
            .as_deref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false);
        if !has_bvid {
            return Ok(ApiResponse::error("当前任务没有BV号，无法集成投稿"));
        }
    }
    if let Some(response) = try_repost_transcode_with_existing_clean_merged(
        &state,
        &context,
        &task_id,
        &workflow_config,
        integrate_current_bvid,
    )
    .await?
    {
        append_log(
            &state.app_log_path,
            &format!(
                "submission_repost_transcode_reuse_merged task_id={}",
                task_id
            ),
        );
        return Ok(response);
    }
    append_log(
        &state.app_log_path,
        &format!(
            "submission_repost_transcode_start task_id={} type={}",
            task_id,
            if integrate_current_bvid {
                "UPDATE"
            } else {
                "NEW"
            }
        ),
    );
    if detail.source_videos.is_empty() {
        return handle_full_reprocess_missing_sources(
            &state,
            &context,
            &task_id,
            &workflow_config,
            integrate_current_bvid,
            &[],
        )
        .await;
    }
    let missing_sources = collect_missing_source_files(&detail.source_videos);
    if !missing_sources.is_empty() {
        return handle_full_reprocess_missing_sources(
            &state,
            &context,
            &task_id,
            &workflow_config,
            integrate_current_bvid,
            &missing_sources,
        )
        .await;
    }
    let _ = reset_submission_for_repost(
        &context,
        &state.app_log_path,
        &task_id,
        &workflow_config,
        if integrate_current_bvid {
            "VIDEO_UPDATE"
        } else {
            "VIDEO_SUBMISSION"
        },
        !integrate_current_bvid,
        true,
    )?;
    start_submission_workflow(
        context.db.clone(),
        context.app_log_path.clone(),
        context.edit_upload_state.clone(),
        task_id,
    );
    Ok(ApiResponse::success("转码投稿已启动".to_string()))
}

pub(super) async fn handle_full_reprocess_missing_sources(
    state: &State<'_, AppState>,
    context: &SubmissionContext,
    task_id: &str,
    workflow_config: &Value,
    integrate_current_bvid: bool,
    missing_sources: &[String],
) -> Result<ApiResponse<String>, String> {
    append_log(
        &state.app_log_path,
        &format!(
            "submission_full_reprocess_missing_sources task_id={} missing_count={}",
            task_id,
            missing_sources.len()
        ),
    );
    if missing_sources.is_empty() {
        return try_prepare_full_reprocess_from_baidu_merged(
            state,
            context,
            task_id,
            workflow_config,
            integrate_current_bvid,
        )
        .await;
    }
    let (_, unresolved_sources) =
        collect_retry_download_records_for_missing_sources(context, task_id, missing_sources)?;
    if unresolved_sources.is_empty() {
        return enqueue_missing_source_downloads_for_repost(
            state,
            context,
            task_id,
            workflow_config,
            integrate_current_bvid,
            missing_sources,
            "源视频缺失，已按源视频绑定重新下载，下载完成后自动重新剪辑、合并、分段并投稿",
            true,
        )
        .await;
    }

    append_log(
        &state.app_log_path,
        &format!(
            "submission_full_reprocess_fallback_to_merged task_id={} unresolved_count={}",
            task_id,
            unresolved_sources.len()
        ),
    );
    for path in &unresolved_sources {
        append_log(
            &state.app_log_path,
            &format!(
                "submission_full_reprocess_unresolved_source task_id={} path={}",
                task_id, path
            ),
        );
    }
    try_prepare_full_reprocess_from_baidu_merged(
        state,
        context,
        task_id,
        workflow_config,
        integrate_current_bvid,
    )
    .await
}

pub(super) fn prepare_workflow_for_existing_merged_repost(
    context: &SubmissionContext,
    app_log_path: &PathBuf,
    task_id: &str,
    workflow_config: &Value,
    workflow_type: &str,
    clear_bvid: bool,
) -> Result<(), String> {
    if let Err(err) = clear_edit_upload_segments_by_task(context, task_id) {
        append_log(
            app_log_path,
            &format!(
                "submission_existing_merged_clear_cache_fail task_id={} err={}",
                task_id, err
            ),
        );
    }
    reset_workflow_instances(context, task_id).map_err(|err| format!("重置工作流失败: {}", err))?;
    let (instance_id, _) = create_workflow_instance_for_task_with_type(
        context.db.as_ref(),
        task_id,
        workflow_config,
        workflow_type,
    )
    .map_err(|err| format!("创建工作流失败: {}", err))?;
    let now = now_rfc3339();
    context
        .db
        .with_conn(|conn| {
            if clear_bvid {
                conn.execute(
                    "UPDATE submission_task SET status = 'PENDING', bvid = NULL, aid = NULL, remote_state = NULL, remote_state_desc = NULL, reject_reason = NULL, updated_at = ?1 WHERE task_id = ?2",
                    (&now, task_id),
                )?;
            } else {
                conn.execute(
                    "UPDATE submission_task SET status = 'PENDING', remote_state = NULL, remote_state_desc = NULL, reject_reason = NULL, updated_at = ?1 WHERE task_id = ?2",
                    (&now, task_id),
                )?;
            }
            conn.execute(
                "UPDATE task_relations SET workflow_instance_id = ?1, updated_at = ?2 WHERE submission_task_id = ?3 AND relation_type = 'INTEGRATED'",
                (&instance_id, &now, task_id),
            )?;
            Ok(())
        })
        .map_err(|err| err.to_string())?;
    Ok(())
}

pub(super) async fn try_repost_with_existing_merged_assets(
    state: &State<'_, AppState>,
    context: &SubmissionContext,
    task_id: &str,
    workflow_config: &Value,
    integrate_current_bvid: bool,
) -> Result<Option<ApiResponse<String>>, String> {
    let mut merged_videos = load_merged_videos_by_task(context, task_id).unwrap_or_default();
    if merged_videos.is_empty() {
        return Ok(None);
    }

    let base_dir = resolve_submission_base_dir(context, task_id);
    let workflow_type = if integrate_current_bvid {
        "VIDEO_UPDATE"
    } else {
        "VIDEO_SUBMISSION"
    };
    let mut merged_workflow_config = workflow_config.clone();
    apply_reprocess_metadata(&mut merged_workflow_config, ReprocessMode::MergeAll, None);

    for merged in &mut merged_videos {
        let mut path = merged.video_path.clone().unwrap_or_default();
        if path.trim().is_empty() || !PathBuf::from(&path).exists() {
            match try_restore_merged_from_baidu(context, &state.app_log_path, merged, &base_dir)
                .await?
            {
                BaiduRestoreResult::Ready(restored) => {
                    path = restored.to_string_lossy().to_string();
                    merged.video_path = Some(path.clone());
                }
                BaiduRestoreResult::Queued => {
                    prepare_workflow_for_baidu_restore(
                        context,
                        &state.app_log_path,
                        task_id,
                        &merged_workflow_config,
                        workflow_type,
                        !integrate_current_bvid,
                    )?;
                    return Ok(Some(ApiResponse::success(
                        "源视频缺失，但已切换为优先恢复合并视频；网盘下载完成后自动重新投稿"
                            .to_string(),
                    )));
                }
                BaiduRestoreResult::NotBound => {
                    append_log(
                        &state.app_log_path,
                        &format!(
                            "submission_existing_merged_unavailable task_id={} merged_id={} reason=not_bound",
                            task_id, merged.id
                        ),
                    );
                    return Ok(None);
                }
            }
        }
        if path.trim().is_empty() || !PathBuf::from(&path).exists() {
            append_log(
                &state.app_log_path,
                &format!(
                    "submission_existing_merged_unavailable task_id={} merged_id={} reason=local_missing",
                    task_id, merged.id
                ),
            );
            return Ok(None);
        }
    }

    prepare_workflow_for_existing_merged_repost(
        context,
        &state.app_log_path,
        task_id,
        &merged_workflow_config,
        workflow_type,
        !integrate_current_bvid,
    )?;
    let context_clone = context.clone();
    let task_id_owned = task_id.to_string();
    let workflow_type_owned = workflow_type.to_string();
    let app_log_path = state.app_log_path.clone();
    tauri::async_runtime::spawn(async move {
        let _ = resume_repost_after_restore(
            &context_clone,
            &app_log_path,
            &task_id_owned,
            ReprocessMode::MergeAll,
            None,
            &workflow_type_owned,
        )
        .await;
    });
    Ok(Some(ApiResponse::success(
        "源视频缺失，但本地合并视频可用，已直接基于合并视频重新投稿".to_string(),
    )))
}

pub(super) async fn try_prepare_full_reprocess_from_baidu_merged(
    state: &State<'_, AppState>,
    context: &SubmissionContext,
    task_id: &str,
    workflow_config: &Value,
    integrate_current_bvid: bool,
) -> Result<ApiResponse<String>, String> {
    let mut merged_videos = load_merged_videos_by_task(context, task_id)?;
    if merged_videos.is_empty() {
        return Ok(ApiResponse::error(
            "任务异常：源视频缺失且无法按B站重新下载，同时未找到可恢复的合并视频".to_string(),
        ));
    }

    let mut merged_workflow_config = workflow_config.clone();
    apply_reprocess_metadata(&mut merged_workflow_config, ReprocessMode::MergeAll, None);
    apply_integrate_current_bvid(&mut merged_workflow_config, integrate_current_bvid);
    let workflow_type = if integrate_current_bvid {
        "VIDEO_UPDATE"
    } else {
        "VIDEO_SUBMISSION"
    };
    let base_dir = resolve_submission_base_dir(context, task_id);
    let mut queued_restore = false;

    for merged in &mut merged_videos {
        match try_restore_merged_from_baidu(context, &state.app_log_path, merged, &base_dir).await?
        {
            BaiduRestoreResult::Ready(restored) => {
                merged.video_path = Some(restored.to_string_lossy().to_string());
            }
            BaiduRestoreResult::Queued => {
                queued_restore = true;
            }
            BaiduRestoreResult::NotBound => {
                let merged_name = merged
                    .file_name
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or("未知合并视频");
                append_log(
                    &state.app_log_path,
                    &format!(
                        "submission_full_reprocess_merged_not_bound task_id={} merged_id={} file={}",
                        task_id, merged.id, merged_name
                    ),
                );
                return Ok(ApiResponse::error(format!(
                    "任务异常：存在缺失源视频且未绑定可重下的BVID/CID，合并视频“{}”也未绑定网盘地址，无法执行全部重新投稿",
                    merged_name
                )));
            }
        }
    }

    if queued_restore {
        prepare_workflow_for_baidu_restore(
            context,
            &state.app_log_path,
            task_id,
            &merged_workflow_config,
            workflow_type,
            !integrate_current_bvid,
        )?;
        return Ok(ApiResponse::success(
            "部分源视频缺失且无法按B站重新下载，已转为下载合并视频后重新分段、投稿".to_string(),
        ));
    }

    prepare_workflow_for_existing_merged_repost(
        context,
        &state.app_log_path,
        task_id,
        &merged_workflow_config,
        workflow_type,
        !integrate_current_bvid,
    )?;
    let context_clone = context.clone();
    let task_id_owned = task_id.to_string();
    let workflow_type_owned = workflow_type.to_string();
    let app_log_path = state.app_log_path.clone();
    tauri::async_runtime::spawn(async move {
        let _ = resume_repost_after_restore(
            &context_clone,
            &app_log_path,
            &task_id_owned,
            ReprocessMode::MergeAll,
            None,
            &workflow_type_owned,
        )
        .await;
    });
    Ok(ApiResponse::success(
        "部分源视频缺失且无法按B站重新下载，已基于已恢复的合并视频重新分段、投稿".to_string(),
    ))
}

pub(super) fn is_merged_video_reusable_for_transcode_repost(
    context: &SubmissionContext,
    task_id: &str,
    merged: &MergedVideoRecord,
    app_log_path: &PathBuf,
) -> Result<bool, String> {
    let Some(path_str) = merged.video_path.as_deref() else {
        append_log(
            app_log_path,
            &format!(
                "submission_transcode_reuse_merged_skip task_id={} merged_id={} reason=path_empty",
                task_id, merged.id
            ),
        );
        return Ok(false);
    };
    let path_str = path_str.trim();
    if path_str.is_empty() {
        append_log(
            app_log_path,
            &format!(
                "submission_transcode_reuse_merged_skip task_id={} merged_id={} reason=path_blank",
                task_id, merged.id
            ),
        );
        return Ok(false);
    }
    let path = PathBuf::from(path_str);
    if !path.exists() {
        append_log(
            app_log_path,
            &format!(
                "submission_transcode_reuse_merged_skip task_id={} merged_id={} reason=missing path={}",
                task_id,
                merged.id,
                path.to_string_lossy()
            ),
        );
        return Ok(false);
    }

    let duration = match verify_generated_media_output(
        context,
        task_id,
        &path,
        "submission_transcode_reuse_merged_verify",
        None,
    ) {
        Ok(value) => value,
        Err(err) => {
            append_log(
                app_log_path,
                &format!(
                    "submission_transcode_reuse_merged_skip task_id={} merged_id={} reason=verify_fail err={}",
                    task_id, merged.id, err
                ),
            );
            return Ok(false);
        }
    };

    let probe_args = vec![
        "-v".to_string(),
        "error".to_string(),
        "-show_entries".to_string(),
        "stream=codec_type,start_time,duration:format=start_time,duration".to_string(),
        "-of".to_string(),
        "json".to_string(),
        path.to_string_lossy().to_string(),
    ];
    let probe = run_ffprobe_json(&probe_args)?;
    let streams = probe
        .get("streams")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    let video_stream = streams
        .iter()
        .find(|item| item.get("codec_type").and_then(|value| value.as_str()) == Some("video"));
    let audio_stream = streams
        .iter()
        .find(|item| item.get("codec_type").and_then(|value| value.as_str()) == Some("audio"));
    let Some(video_stream) = video_stream else {
        append_log(
            app_log_path,
            &format!(
                "submission_transcode_reuse_merged_skip task_id={} merged_id={} reason=missing_video_stream",
                task_id, merged.id
            ),
        );
        return Ok(false);
    };
    let Some(audio_stream) = audio_stream else {
        append_log(
            app_log_path,
            &format!(
                "submission_transcode_reuse_merged_skip task_id={} merged_id={} reason=missing_audio_stream",
                task_id, merged.id
            ),
        );
        return Ok(false);
    };
    let video_start = parse_ffprobe_time(video_stream.get("start_time")).unwrap_or(0.0);
    let audio_start = parse_ffprobe_time(audio_stream.get("start_time")).unwrap_or(0.0);
    let video_duration = parse_ffprobe_time(video_stream.get("duration")).unwrap_or(duration);
    let audio_duration = parse_ffprobe_time(audio_stream.get("duration")).unwrap_or(duration);
    let av_delta = (video_start - audio_start).abs();
    if av_delta > 0.2 {
        append_log(
            app_log_path,
            &format!(
                "submission_transcode_reuse_merged_skip task_id={} merged_id={} reason=av_start_delta delta={:.3}",
                task_id, merged.id, av_delta
            ),
        );
        return Ok(false);
    }
    let av_duration_delta = (video_duration - audio_duration).abs();
    if av_duration_delta > 1.0 {
        append_log(
            app_log_path,
            &format!(
                "submission_transcode_reuse_merged_skip task_id={} merged_id={} reason=av_duration_delta delta={:.3}",
                task_id, merged.id, av_duration_delta
            ),
        );
        return Ok(false);
    }

    let mut sample_points = vec![0.0];
    if duration > 2.0 {
        sample_points.push(duration * 0.25);
        sample_points.push(duration * 0.5);
        sample_points.push(duration * 0.75);
        sample_points.push((duration - 5.0).max(0.0));
    }
    sample_points
        .sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    sample_points.dedup_by(|left, right| (*left - *right).abs() < 0.5);

    for sample in &sample_points {
        let has_video = merged_probe_has_packets(&path, "v:0", *sample)?;
        let has_audio = merged_probe_has_packets(&path, "a:0", *sample)?;
        if !has_video || !has_audio {
            append_log(
                app_log_path,
                &format!(
                    "submission_transcode_reuse_merged_skip task_id={} merged_id={} reason=packet_probe_missing sample={:.3} has_video={} has_audio={}",
                    task_id, merged.id, sample, has_video, has_audio
                ),
            );
            return Ok(false);
        }
    }

    let decode_probe_span = 20.0f64;
    for sample in &sample_points {
        let span = decode_probe_span.min((duration - *sample).max(1.0));
        if let Some(reason) = merged_video_decode_probe_issue(&path, *sample, span)? {
            append_log(
                app_log_path,
                &format!(
                    "submission_transcode_reuse_merged_skip task_id={} merged_id={} reason=video_decode_probe_issue detail={} sample={:.3} span={:.3}",
                    task_id, merged.id, reason, sample, span
                ),
            );
            return Ok(false);
        }
    }

    let continuity_window = 660.0f64;
    let continuity_step = 600.0f64;
    let continuity_threshold = 0.5f64;
    let mut continuity_points = vec![0.0];
    let mut cursor = 0.0f64;
    while cursor < duration {
        continuity_points.push(cursor);
        cursor += continuity_step;
    }
    if duration > continuity_window {
        continuity_points.push((duration - continuity_window).max(0.0));
    }
    continuity_points
        .sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    continuity_points.dedup_by(|left, right| (*left - *right).abs() < 1.0);

    for sample in continuity_points {
        let span = continuity_window.min((duration - sample).max(1.0));
        let max_gap_delta = match merged_audio_packet_gap_delta(&path, sample, span) {
            Ok(value) => value,
            Err(err) => {
                append_log(
                    app_log_path,
                    &format!(
                        "submission_transcode_reuse_merged_skip task_id={} merged_id={} reason=audio_packet_probe_fail sample={:.3} span={:.3} err={}",
                        task_id, merged.id, sample, span, err
                    ),
                );
                return Ok(false);
            }
        };
        if max_gap_delta > continuity_threshold {
            append_log(
                app_log_path,
                &format!(
                    "submission_transcode_reuse_merged_skip task_id={} merged_id={} reason=audio_packet_gap delta={:.3} sample={:.3} span={:.3}",
                    task_id, merged.id, max_gap_delta, sample, span
                ),
            );
            return Ok(false);
        }
    }

    append_log(
        app_log_path,
        &format!(
            "submission_transcode_reuse_merged_ok task_id={} merged_id={} path={} duration={:.3}",
            task_id,
            merged.id,
            path.to_string_lossy(),
            duration
        ),
    );
    Ok(true)
}

pub(super) async fn try_repost_transcode_with_existing_clean_merged(
    state: &State<'_, AppState>,
    context: &SubmissionContext,
    task_id: &str,
    workflow_config: &Value,
    integrate_current_bvid: bool,
) -> Result<Option<ApiResponse<String>>, String> {
    let task = load_task_record(context, task_id)?;
    if is_remote_transcode_failed_state(task.remote_state) {
        append_log(
            &state.app_log_path,
            &format!(
                "submission_transcode_reuse_merged_skip task_id={} reason=remote_transcode_failed",
                task_id
            ),
        );
        return Ok(None);
    }
    let merged = match load_latest_merged_video(context, task_id)? {
        Some(value) => value,
        None => return Ok(None),
    };
    if !is_merged_video_reusable_for_transcode_repost(
        context,
        task_id,
        &merged,
        &state.app_log_path,
    )? {
        return Ok(None);
    }

    let workflow_type = if integrate_current_bvid {
        "VIDEO_UPDATE"
    } else {
        "VIDEO_SUBMISSION"
    };
    let mut merged_workflow_config = workflow_config.clone();
    apply_reprocess_metadata(
        &mut merged_workflow_config,
        ReprocessMode::Specified,
        Some(merged.id),
    );
    apply_integrate_current_bvid(&mut merged_workflow_config, integrate_current_bvid);
    apply_media_process_mode(
        &mut merged_workflow_config,
        MediaProcessMode::ClipAndMergeTranscode,
    );

    prepare_workflow_for_existing_merged_repost(
        context,
        &state.app_log_path,
        task_id,
        &merged_workflow_config,
        workflow_type,
        !integrate_current_bvid,
    )?;
    let context_clone = context.clone();
    let task_id_owned = task_id.to_string();
    let workflow_type_owned = workflow_type.to_string();
    let app_log_path = state.app_log_path.clone();
    let merged_id = merged.id;
    tauri::async_runtime::spawn(async move {
        let _ = resume_repost_after_restore(
            &context_clone,
            &app_log_path,
            &task_id_owned,
            ReprocessMode::Specified,
            Some(merged_id),
            &workflow_type_owned,
        )
        .await;
    });
    Ok(Some(ApiResponse::success(
        "检测到当前本地合并视频可直接复用，已跳过剪辑/合并，直接进入分段转码投稿".to_string(),
    )))
}

pub(super) fn resolve_baidu_restore_path(
    merged: &MergedVideoRecord,
    base_dir: &Path,
    remote_name: &str,
) -> PathBuf {
    if let Some(path) = merged
        .video_path
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        return PathBuf::from(path);
    }
    let file_name = merged
        .file_name
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(remote_name);
    base_dir.join("merge").join(file_name)
}

pub(super) async fn check_baidu_remote_file_exists_for_restore(
    context: &SubmissionContext,
    remote_path: &str,
) -> Result<bool, String> {
    let db = context.db.clone();
    let remote_path_owned = remote_path.to_string();
    let remote_path_for_task = remote_path_owned.clone();
    let check_future = tauri::async_runtime::spawn_blocking(move || {
        baidu_sync::check_baidu_remote_file_exists(db.as_ref(), &remote_path_for_task)
    });
    match timeout(
        Duration::from_secs(BAIDU_RESTORE_CHECK_TIMEOUT_SECS),
        check_future,
    )
    .await
    {
        Ok(Ok(result)) => result,
        Ok(Err(err)) => Err(format!("网盘存在性检查线程失败: {}", err)),
        Err(_) => Err(format!(
            "网盘存在性检查超时({}s): {}",
            BAIDU_RESTORE_CHECK_TIMEOUT_SECS, remote_path_owned
        )),
    }
}

pub(super) fn ensure_remote_restore_relation(
    context: &SubmissionContext,
    task_id: &str,
    record_id: i64,
) -> Result<(), String> {
    let now = now_rfc3339();
    let exists = context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT 1 FROM task_relations WHERE submission_task_id = ?1 AND download_task_id = ?2 AND relation_type = 'REMOTE_RESTORE' LIMIT 1",
      )?;
      let mut rows = stmt.query((task_id, record_id))?;
      Ok(rows.next()?.is_some())
    })
    .map_err(|err| err.to_string())?;
    if exists {
        let _ = context.db.with_conn(|conn| {
      conn.execute(
        "UPDATE task_relations SET workflow_status = 'PENDING_DOWNLOAD', updated_at = ?1 WHERE submission_task_id = ?2 AND download_task_id = ?3 AND relation_type = 'REMOTE_RESTORE'",
        (&now, task_id, record_id),
      )?;
      Ok(())
    });
        return Ok(());
    }
    context
    .db
    .with_conn(|conn| {
      conn.execute(
        "INSERT INTO task_relations (download_task_id, submission_task_id, relation_type, status, created_at, updated_at, workflow_status, retry_count) \
         VALUES (?1, ?2, 'REMOTE_RESTORE', 'ACTIVE', ?3, ?4, 'PENDING_DOWNLOAD', 0)",
        (record_id, task_id, &now, &now),
      )?;
      Ok(())
    })
    .map_err(|err| err.to_string())
}

pub(super) async fn try_restore_merged_from_baidu(
    context: &SubmissionContext,
    app_log_path: &PathBuf,
    merged: &MergedVideoRecord,
    base_dir: &Path,
) -> Result<BaiduRestoreResult, String> {
    let current_baidu_uid = require_logged_baidu_uid(context.db.as_ref())?;
    if let Some(merged_baidu_uid) = load_merged_baidu_uid(context, merged.id)? {
        let trimmed = merged_baidu_uid.trim();
        if !trimmed.is_empty() && trimmed != current_baidu_uid {
            return Err("未对应当前网盘账号".to_string());
        }
    }
    let Some((remote_dir, remote_name)) = extract_baidu_binding(merged) else {
        return Ok(BaiduRestoreResult::NotBound);
    };
    let remote_path = baidu_sync::join_baidu_path(&remote_dir, &remote_name);
    append_log(
        app_log_path,
        &format!(
            "submission_baidu_restore_check merged_id={} remote={}",
            merged.id, remote_path
        ),
    );
    let exists = match check_baidu_remote_file_exists_for_restore(context, &remote_path).await {
        Ok(value) => value,
        Err(err) => {
            append_log(
                app_log_path,
                &format!(
                    "submission_baidu_restore_check_fail merged_id={} err={}",
                    merged.id, err
                ),
            );
            return Ok(BaiduRestoreResult::NotBound);
        }
    };
    if !exists {
        append_log(
            app_log_path,
            &format!(
                "submission_baidu_restore_missing merged_id={} remote={}",
                merged.id, remote_path
            ),
        );
        return Ok(BaiduRestoreResult::NotBound);
    }
    let expected_path = resolve_baidu_restore_path(merged, base_dir, remote_name.trim());
    let expected_path_str = expected_path.to_string_lossy().to_string();
    let expected_path_stored = to_stored_submission_path(context, expected_path_str.as_str());
    let record = context
        .db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT vd.id, vd.local_path, vd.status, tr.id \
         FROM video_download vd \
         LEFT JOIN task_relations tr \
           ON tr.download_task_id = vd.id \
          AND tr.submission_task_id = ?1 \
          AND tr.relation_type = 'REMOTE_RESTORE' \
         WHERE vd.download_url = ?2 AND vd.source_type = ?3 \
         ORDER BY vd.id DESC LIMIT 1",
            )?;
            let row = stmt
                .query_row(
                    (
                        &merged.task_id,
                        &remote_path,
                        crate::commands::download::DOWNLOAD_SOURCE_BAIDU,
                    ),
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, Option<i64>>(3)?,
                        ))
                    },
                )
                .optional()?;
            Ok(row)
        })
        .map_err(|err| err.to_string())?;
    if let Some((record_id, local_path, status, relation_id)) = record {
        let mut local_path_value = local_path.unwrap_or_default();
        if local_path_value.trim().is_empty() {
            local_path_value = expected_path_stored.clone();
            let now = now_rfc3339();
            let _ = context.db.with_conn(|conn| {
                conn.execute(
                    "UPDATE video_download SET local_path = ?1, update_time = ?2 WHERE id = ?3",
                    (&local_path_value, &now, record_id),
                )?;
                Ok(())
            });
        }
        if status == 2 && !local_path_value.trim().is_empty() {
            let local_path_buf = to_absolute_local_path_with_prefix(
                context.local_path_prefix.as_path(),
                &local_path_value,
            );
            if local_path_buf.exists() {
                update_merged_video_after_baidu_download(
                    context,
                    merged.id,
                    &local_path_buf,
                    &remote_dir,
                    &remote_name,
                    Some(current_baidu_uid.as_str()),
                )?;
                append_log(
                    app_log_path,
                    &format!(
                        "submission_baidu_restore_cached merged_id={} local={}",
                        merged.id, local_path_value
                    ),
                );
                return Ok(BaiduRestoreResult::Ready(local_path_buf));
            }
        }
        if status == 2 {
            let _ = reset_baidu_download_record(context, record_id);
            let _ = ensure_remote_restore_relation(context, &merged.task_id, record_id);
            append_log(
                app_log_path,
                &format!(
          "submission_baidu_restore_requeue merged_id={} record_id={} reason=local_missing",
          merged.id, record_id
        ),
            );
            return Ok(BaiduRestoreResult::Queued);
        }
        if status == 3 {
            let _ = reset_baidu_download_record(context, record_id);
        }
        if relation_id.is_none() {
            let _ = ensure_remote_restore_relation(context, &merged.task_id, record_id);
        }
        return Ok(BaiduRestoreResult::Queued);
    }

    let now = now_rfc3339();
    let title = if !remote_name.trim().is_empty() {
        remote_name.trim()
    } else {
        merged
            .file_name
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("网盘文件")
    };
    let record_id = context
    .db
    .with_conn(|conn| {
      conn.execute(
        "INSERT INTO video_download (bvid, aid, title, part_title, part_count, current_part, download_url, local_path, status, progress, progress_total, progress_done, create_time, update_time, resolution, codec, format, cid, content, source_type) \
         VALUES (NULL, NULL, ?1, ?2, 1, 1, ?3, ?4, 0, 0, 0, 0, ?5, ?6, NULL, NULL, NULL, NULL, NULL, ?7)",
        (
          title,
          remote_name.trim(),
          remote_path.as_str(),
          expected_path_stored.as_str(),
          &now,
          &now,
          crate::commands::download::DOWNLOAD_SOURCE_BAIDU,
        ),
      )?;
      Ok(conn.last_insert_rowid())
    })
    .map_err(|err| err.to_string())?;
    ensure_remote_restore_relation(context, &merged.task_id, record_id)?;
    Ok(BaiduRestoreResult::Queued)
}

pub(super) fn prepare_workflow_for_baidu_restore(
    context: &SubmissionContext,
    app_log_path: &PathBuf,
    task_id: &str,
    workflow_config: &Value,
    workflow_type: &str,
    clear_bvid: bool,
) -> Result<(), String> {
    if let Err(err) = clear_edit_upload_segments_by_task(context, task_id) {
        append_log(
            app_log_path,
            &format!(
                "submission_baidu_restore_clear_cache_fail task_id={} err={}",
                task_id, err
            ),
        );
    }
    reset_workflow_instances(context, task_id).map_err(|err| format!("重置工作流失败: {}", err))?;
    let (instance_id, _) = create_workflow_instance_for_task_with_type(
        context.db.as_ref(),
        task_id,
        workflow_config,
        workflow_type,
    )
    .map_err(|err| format!("创建工作流失败: {}", err))?;
    let now = now_rfc3339();
    context
    .db
    .with_conn(|conn| {
      if clear_bvid {
        conn.execute(
          "UPDATE submission_task SET status = 'PENDING', bvid = NULL, aid = NULL, remote_state = NULL, remote_state_desc = NULL, reject_reason = NULL, updated_at = ?1 WHERE task_id = ?2",
          (&now, task_id),
        )?;
      } else {
        conn.execute(
          "UPDATE submission_task SET status = 'PENDING', remote_state = NULL, remote_state_desc = NULL, reject_reason = NULL, updated_at = ?1 WHERE task_id = ?2",
          (&now, task_id),
        )?;
      }
      conn.execute(
        "UPDATE task_relations SET workflow_instance_id = ?1, updated_at = ?2 WHERE submission_task_id = ?3 AND relation_type = 'INTEGRATED'",
        (&instance_id, &now, task_id),
      )?;
      Ok(())
    })
    .map_err(|err| err.to_string())?;
    let _ = update_workflow_status(context, task_id, "VIDEO_DOWNLOADING", None, 0.0);
    Ok(())
}

pub(super) fn load_remote_restore_stats(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<(i64, i64, i64), String> {
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
         WHERE tr.submission_task_id = ?1 AND tr.relation_type = 'REMOTE_RESTORE'",
            )?;
            let row = stmt.query_row([task_id], |row| {
                let total: i64 = row.get(0)?;
                let completed: Option<i64> = row.get(1)?;
                let failed: Option<i64> = row.get(2)?;
                Ok((total, completed.unwrap_or(0), failed.unwrap_or(0)))
            })?;
            Ok(row)
        })
        .map_err(|err| err.to_string())
}

pub(super) fn update_remote_restore_status(
    context: &SubmissionContext,
    task_id: &str,
    workflow_status: &str,
) -> Result<(), String> {
    let now = now_rfc3339();
    context
        .db
        .with_conn(|conn| {
            conn.execute(
                "UPDATE task_relations SET workflow_status = ?1, updated_at = ?2 \
         WHERE submission_task_id = ?3 AND relation_type = 'REMOTE_RESTORE'",
                (workflow_status, &now, task_id),
            )?;
            Ok(())
        })
        .map_err(|err| err.to_string())
}

pub(super) fn should_defer_recovery_for_remote_restore(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<bool, String> {
    let has_remote_restore = context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT 1 FROM task_relations WHERE submission_task_id = ?1 AND relation_type = 'REMOTE_RESTORE' LIMIT 1",
      )?;
      let mut rows = stmt.query([task_id])?;
      Ok(rows.next()?.is_some())
    })
    .map_err(|err| err.to_string())?;
    if !has_remote_restore {
        return Ok(false);
    }

    let has_pending_restore = context
        .db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT 1 FROM task_relations \
         WHERE submission_task_id = ?1 \
           AND relation_type = 'REMOTE_RESTORE' \
           AND COALESCE(workflow_status, '') != 'READY' \
         LIMIT 1",
            )?;
            let mut rows = stmt.query([task_id])?;
            Ok(rows.next()?.is_some())
        })
        .map_err(|err| err.to_string())?;
    if has_pending_restore {
        return Ok(true);
    }
    Ok(false)
}

pub(super) async fn resume_resegment_after_restore(
    context: &SubmissionContext,
    app_log_path: &PathBuf,
    task_id: &str,
    mode: ReprocessMode,
    merged_video_id: Option<i64>,
) -> Result<(), String> {
    let workflow_settings = load_workflow_settings(context, task_id);
    let segment_seconds = workflow_settings.segment_duration_seconds.max(1);
    let segment_prefix = workflow_settings.segment_prefix.clone();
    let config = load_latest_workflow_config(context, task_id).ok().flatten();
    let integrate_current_bvid = load_integrate_current_bvid(config.as_ref());
    let started_from_terminal = load_resegment_terminal_metadata(config.as_ref());
    let base_dir = resolve_submission_base_dir(context, task_id);
    let output_dir = base_dir
        .join("resegment")
        .join(sanitize_filename(&format!("resegment_{}", now_rfc3339())))
        .join("output");

    let update_segmenting = |clear_segments: bool| -> Result<(), String> {
        let now = now_rfc3339();
        context
      .db
      .with_conn(|conn| {
        if clear_segments {
          conn.execute("DELETE FROM task_output_segment WHERE task_id = ?1", [task_id])?;
        }
        if integrate_current_bvid {
          conn.execute(
            "UPDATE submission_task SET status = 'SEGMENTING', remote_state = NULL, remote_state_desc = NULL, reject_reason = NULL, updated_at = ?1 WHERE task_id = ?2",
            (&now, task_id),
          )?;
        } else {
          conn.execute(
            "UPDATE submission_task SET status = 'SEGMENTING', bvid = NULL, aid = NULL, remote_state = NULL, remote_state_desc = NULL, reject_reason = NULL, updated_at = ?1 WHERE task_id = ?2",
            (&now, task_id),
          )?;
        }
        Ok(())
      })
      .map_err(|err| err.to_string())
    };

    if let Err(err) = clear_edit_upload_segments_by_task(context, task_id) {
        append_log(
            app_log_path,
            &format!(
                "submission_resegment_clear_cache_fail task_id={} err={}",
                task_id, err
            ),
        );
    }

    match mode {
        ReprocessMode::Specified => {
            let mut merged =
                resolve_target_merged_video(context, task_id, merged_video_id, app_log_path)?;
            let mut merged_path = merged.video_path.clone().unwrap_or_default();
            if merged_path.trim().is_empty() || !PathBuf::from(&merged_path).exists() {
                match try_restore_merged_from_baidu(context, app_log_path, &merged, &base_dir)
                    .await?
                {
                    BaiduRestoreResult::Ready(restored) => {
                        merged_path = restored.to_string_lossy().to_string();
                        merged.video_path = Some(merged_path.clone());
                    }
                    BaiduRestoreResult::Queued => {
                        return Err(format!("{} 仍在网盘下载中", merged_display_name(&merged)));
                    }
                    BaiduRestoreResult::NotBound => {
                        return Err(format!("{} 合并视频不存在", merged_display_name(&merged)));
                    }
                }
            }
            let mut active_merged_path = PathBuf::from(merged_path);
            if !active_merged_path.exists() {
                return Err(format!("{} 合并视频不存在", merged_display_name(&merged)));
            }
            let mut merged_id = merged.id;
            let use_replace_segments = started_from_terminal;
            if !started_from_terminal {
                let (new_merged_id, new_merged_path) = recreate_selected_merged_for_resegment(
                    context,
                    task_id,
                    merged.id,
                    active_merged_path.as_path(),
                    &output_dir,
                )?;
                merged_id = new_merged_id;
                active_merged_path = new_merged_path;
            }
            update_segmenting(false)?;
            let context_clone = context.clone();
            let task_id_clone = task_id.to_string();
            let merged_path_clone = active_merged_path.clone();
            let output_dir_clone = output_dir.clone();
            let app_log_path = app_log_path.to_path_buf();
            tauri::async_runtime::spawn(async move {
                let _ = update_workflow_status(
                    &context_clone,
                    &task_id_clone,
                    "RUNNING",
                    Some("SEGMENTING"),
                    70.0,
                );
                let segment_outputs = match tauri::async_runtime::spawn_blocking(move || {
                    segment_file(&merged_path_clone, &output_dir_clone, segment_seconds)
                })
                .await
                {
                    Ok(result) => result,
                    Err(_) => Err("Failed to segment video".to_string()),
                };
                match segment_outputs {
                    Ok(outputs) => {
                        if outputs.is_empty() {
                            let _ = mark_submission_failure(
                                &context_clone,
                                &task_id_clone,
                                Some("SEGMENTING"),
                                "重新分段未生成任何分段文件",
                            );
                            append_log(
                                app_log_path.as_ref(),
                                &format!(
                                    "submission_resegment_empty_outputs task_id={}",
                                    task_id_clone
                                ),
                            );
                            return;
                        }
                        let save_result = if use_replace_segments {
                            replace_segments_for_merged(
                                &context_clone,
                                &task_id_clone,
                                merged_id,
                                &outputs,
                                segment_prefix.as_deref(),
                            )
                        } else {
                            save_output_segments(
                                &context_clone,
                                &task_id_clone,
                                &outputs,
                                Some(merged_id),
                                segment_prefix.as_deref(),
                            )
                        };
                        if let Err(err) = save_result {
                            let _ = mark_submission_failure(
                                &context_clone,
                                &task_id_clone,
                                Some("SEGMENTING"),
                                &format!("重新分段保存分段失败: {}", err),
                            );
                            append_log(
                                app_log_path.as_ref(),
                                &format!(
                                    "submission_resegment_save_fail task_id={} err={}",
                                    task_id_clone, err
                                ),
                            );
                            return;
                        }
                        if !integrate_current_bvid {
                            if let Err(err) =
                                reset_segments_for_new_bvid(&context_clone, &task_id_clone)
                            {
                                let _ = mark_submission_failure(
                                    &context_clone,
                                    &task_id_clone,
                                    Some("SEGMENTING"),
                                    &format!("重新分段重置分段元数据失败: {}", err),
                                );
                                append_log(
                                    app_log_path.as_ref(),
                                    &format!(
                    "submission_resegment_reset_segments_fail task_id={} err={}",
                    task_id_clone, err
                  ),
                                );
                                return;
                            }
                        }
                        finalize_reprocess_success(
                            &context_clone,
                            &task_id_clone,
                            "submission_resegment_ok",
                        );
                    }
                    Err(err) => {
                        let _ = mark_submission_failure(
                            &context_clone,
                            &task_id_clone,
                            Some("SEGMENTING"),
                            &format!("重新分段失败: {}", err),
                        );
                        append_log(
                            app_log_path.as_ref(),
                            &format!(
                                "submission_resegment_segment_fail task_id={} err={}",
                                task_id_clone, err
                            ),
                        );
                    }
                }
            });
        }
        ReprocessMode::MergeAll => {
            let mut merged_videos = load_merged_videos_by_task(context, task_id)?;
            if merged_videos.is_empty() {
                return Err("未找到合并视频".to_string());
            }
            let old_merged_ids = merged_videos
                .iter()
                .map(|record| record.id)
                .collect::<Vec<_>>();
            let merge_all_sources = collect_sources_for_merge_all(context, task_id, &merged_videos);
            let mut merge_inputs = Vec::with_capacity(merged_videos.len());
            for merged in &mut merged_videos {
                let mut path = merged.video_path.clone().unwrap_or_default();
                if path.trim().is_empty() || !PathBuf::from(&path).exists() {
                    match try_restore_merged_from_baidu(context, app_log_path, merged, &base_dir)
                        .await?
                    {
                        BaiduRestoreResult::Ready(restored) => {
                            path = restored.to_string_lossy().to_string();
                            merged.video_path = Some(path.clone());
                        }
                        BaiduRestoreResult::Queued => {
                            return Err(format!("{} 仍在网盘下载中", merged_display_name(merged)));
                        }
                        BaiduRestoreResult::NotBound => {
                            return Err(format!("{} 合并视频不存在", merged_display_name(merged)));
                        }
                    }
                }
                if path.trim().is_empty() {
                    return Err(format!("{} 合并视频不存在", merged_display_name(merged)));
                }
                let path_buf = PathBuf::from(&path);
                if !path_buf.exists() {
                    return Err(format!("{} 合并视频不存在", merged_display_name(merged)));
                }
                merge_inputs.push(path_buf);
            }
            update_segmenting(false)?;
            let merge_workflow_dir = if started_from_terminal {
                base_dir.clone()
            } else {
                output_dir
                    .parent()
                    .map(|path| path.to_path_buf())
                    .unwrap_or_else(|| base_dir.clone())
            };
            let merge_output =
                build_merge_output_path_for_task_id(context, &merge_workflow_dir, task_id)?;
            let context_clone = context.clone();
            let task_id_clone = task_id.to_string();
            let output_dir_clone = output_dir.clone();
            let merge_output_for_merge = merge_output.clone();
            let merge_output_for_segment = merge_output.clone();
            let merge_output_for_save = merge_output.clone();
            let app_log_path = app_log_path.to_path_buf();
            let old_merged_ids_clone = old_merged_ids.clone();
            tauri::async_runtime::spawn(async move {
                let _ = update_workflow_status(
                    &context_clone,
                    &task_id_clone,
                    "RUNNING",
                    Some("SEGMENTING"),
                    70.0,
                );
                let merge_inputs_clone = merge_inputs.clone();
                let merge_result = tauri::async_runtime::spawn_blocking(move || {
                    merge_files(&merge_inputs_clone, &merge_output_for_merge)
                })
                .await;
                match merge_result {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => {
                        let _ = mark_submission_failure(
                            &context_clone,
                            &task_id_clone,
                            Some("SEGMENTING"),
                            &format!("重新分段合并失败: {}", err),
                        );
                        append_log(
                            app_log_path.as_ref(),
                            &format!(
                                "submission_resegment_merge_fail task_id={} err={}",
                                task_id_clone, err
                            ),
                        );
                        return;
                    }
                    Err(err) => {
                        let _ = mark_submission_failure(
                            &context_clone,
                            &task_id_clone,
                            Some("SEGMENTING"),
                            &format!("重新分段合并失败: {}", err),
                        );
                        append_log(
                            app_log_path.as_ref(),
                            &format!(
                                "submission_resegment_merge_fail task_id={} err={}",
                                task_id_clone, err
                            ),
                        );
                        return;
                    }
                };
                let merge_output_for_segment_clone = merge_output_for_segment.clone();
                let segment_outputs = match tauri::async_runtime::spawn_blocking(move || {
                    segment_file_with_options(
                        &merge_output_for_segment_clone,
                        &output_dir_clone,
                        segment_seconds,
                        false,
                        false,
                    )
                })
                .await
                {
                    Ok(result) => result,
                    Err(_) => Err("Failed to segment video".to_string()),
                };
                match segment_outputs {
                    Ok(outputs) => {
                        if outputs.is_empty() {
                            let _ = mark_submission_failure(
                                &context_clone,
                                &task_id_clone,
                                Some("SEGMENTING"),
                                "重新分段未生成任何分段文件",
                            );
                            append_log(
                                app_log_path.as_ref(),
                                &format!(
                                    "submission_resegment_empty_outputs task_id={}",
                                    task_id_clone
                                ),
                            );
                            return;
                        }
                        let merged_id = match save_merged_video(
                            &context_clone,
                            &task_id_clone,
                            &merge_output_for_save,
                        ) {
                            Ok(merged_id) => merged_id,
                            Err(err) => {
                                let _ = mark_submission_failure(
                                    &context_clone,
                                    &task_id_clone,
                                    Some("SEGMENTING"),
                                    &format!("重新分段保存合并视频失败: {}", err),
                                );
                                append_log(
                                    app_log_path.as_ref(),
                                    &format!(
                                        "submission_resegment_save_fail task_id={} err={}",
                                        task_id_clone, err
                                    ),
                                );
                                return;
                            }
                        };
                        if let Err(err) = save_merged_source_bindings(
                            &context_clone,
                            &task_id_clone,
                            merged_id,
                            &merge_all_sources,
                        ) {
                            append_log(
                                app_log_path.as_ref(),
                                &format!(
                  "submission_resegment_bind_sources_fail task_id={} merged_id={} err={}",
                  task_id_clone, merged_id, err
                ),
                            );
                        }
                        if let Err(err) = save_output_segments(
                            &context_clone,
                            &task_id_clone,
                            &outputs,
                            Some(merged_id),
                            segment_prefix.as_deref(),
                        ) {
                            let _ = mark_submission_failure(
                                &context_clone,
                                &task_id_clone,
                                Some("SEGMENTING"),
                                &format!("重新分段保存分段失败: {}", err),
                            );
                            append_log(
                                app_log_path.as_ref(),
                                &format!(
                                    "submission_resegment_save_fail task_id={} err={}",
                                    task_id_clone, err
                                ),
                            );
                            return;
                        }
                        let stale_merged_ids = old_merged_ids_clone
                            .iter()
                            .copied()
                            .filter(|id| *id != merged_id)
                            .collect::<Vec<_>>();
                        if let Err(err) = delete_merged_records_by_ids(
                            &context_clone,
                            &task_id_clone,
                            &stale_merged_ids,
                        ) {
                            let _ = mark_submission_failure(
                                &context_clone,
                                &task_id_clone,
                                Some("SEGMENTING"),
                                &format!("重新分段清理旧合并记录失败: {}", err),
                            );
                            append_log(
                                app_log_path.as_ref(),
                                &format!(
                                    "submission_resegment_cleanup_fail task_id={} err={}",
                                    task_id_clone, err
                                ),
                            );
                            return;
                        }
                        if !integrate_current_bvid {
                            if let Err(err) =
                                reset_segments_for_new_bvid(&context_clone, &task_id_clone)
                            {
                                let _ = mark_submission_failure(
                                    &context_clone,
                                    &task_id_clone,
                                    Some("SEGMENTING"),
                                    &format!("重新分段重置分段元数据失败: {}", err),
                                );
                                append_log(
                                    app_log_path.as_ref(),
                                    &format!(
                    "submission_resegment_reset_segments_fail task_id={} err={}",
                    task_id_clone, err
                  ),
                                );
                                return;
                            }
                        }
                        finalize_reprocess_success(
                            &context_clone,
                            &task_id_clone,
                            "submission_resegment_ok",
                        );
                    }
                    Err(err) => {
                        let _ = mark_submission_failure(
                            &context_clone,
                            &task_id_clone,
                            Some("SEGMENTING"),
                            &format!("重新分段失败: {}", err),
                        );
                        append_log(
                            app_log_path.as_ref(),
                            &format!(
                                "submission_resegment_segment_fail task_id={} err={}",
                                task_id_clone, err
                            ),
                        );
                    }
                }
            });
        }
        ReprocessMode::Legacy | ReprocessMode::FullReprocess => {
            let merged = load_latest_merged_video(context, task_id)?
                .ok_or_else(|| "未找到合并视频".to_string())?;
            let merged_path = merged.video_path.clone().unwrap_or_default();
            if merged_path.trim().is_empty() || !PathBuf::from(&merged_path).exists() {
                return Err("合并视频缺失".to_string());
            }
            update_segmenting(true)?;
            if let Err(err) = remove_path_if_exists(app_log_path, "output", &output_dir) {
                append_log(
                    app_log_path,
                    &format!(
                        "submission_resegment_cleanup_fail task_id={} err={}",
                        task_id, err
                    ),
                );
            }
            let merged_path_buf = PathBuf::from(merged_path);
            let merged_id = merged.id;
            let context_clone = context.clone();
            let task_id_clone = task_id.to_string();
            let merged_path_clone = merged_path_buf.clone();
            let output_dir_clone = output_dir.clone();
            let app_log_path = app_log_path.to_path_buf();
            tauri::async_runtime::spawn(async move {
                let _ = update_workflow_status(
                    &context_clone,
                    &task_id_clone,
                    "RUNNING",
                    Some("SEGMENTING"),
                    70.0,
                );
                let segment_outputs = match tauri::async_runtime::spawn_blocking(move || {
                    segment_file(&merged_path_clone, &output_dir_clone, segment_seconds)
                })
                .await
                {
                    Ok(result) => result,
                    Err(_) => Err("Failed to segment video".to_string()),
                };
                match segment_outputs {
                    Ok(outputs) => {
                        if outputs.is_empty() {
                            let _ = mark_submission_failure(
                                &context_clone,
                                &task_id_clone,
                                Some("SEGMENTING"),
                                "重新分段未生成任何分段文件",
                            );
                            append_log(
                                app_log_path.as_ref(),
                                &format!(
                                    "submission_resegment_empty_outputs task_id={}",
                                    task_id_clone
                                ),
                            );
                            return;
                        }
                        if let Err(err) = save_output_segments(
                            &context_clone,
                            &task_id_clone,
                            &outputs,
                            Some(merged_id),
                            segment_prefix.as_deref(),
                        ) {
                            let _ = mark_submission_failure(
                                &context_clone,
                                &task_id_clone,
                                Some("SEGMENTING"),
                                &format!("重新分段保存分段失败: {}", err),
                            );
                            append_log(
                                app_log_path.as_ref(),
                                &format!(
                                    "submission_resegment_save_fail task_id={} err={}",
                                    task_id_clone, err
                                ),
                            );
                            return;
                        }
                        if !integrate_current_bvid {
                            if let Err(err) =
                                reset_segments_for_new_bvid(&context_clone, &task_id_clone)
                            {
                                let _ = mark_submission_failure(
                                    &context_clone,
                                    &task_id_clone,
                                    Some("SEGMENTING"),
                                    &format!("重新分段重置分段元数据失败: {}", err),
                                );
                                append_log(
                                    app_log_path.as_ref(),
                                    &format!(
                    "submission_resegment_reset_segments_fail task_id={} err={}",
                    task_id_clone, err
                  ),
                                );
                                return;
                            }
                        }
                        finalize_reprocess_success(
                            &context_clone,
                            &task_id_clone,
                            "submission_resegment_ok",
                        );
                    }
                    Err(err) => {
                        let _ = mark_submission_failure(
                            &context_clone,
                            &task_id_clone,
                            Some("SEGMENTING"),
                            &format!("重新分段失败: {}", err),
                        );
                        append_log(
                            app_log_path.as_ref(),
                            &format!(
                                "submission_resegment_segment_fail task_id={} err={}",
                                task_id_clone, err
                            ),
                        );
                    }
                }
            });
        }
    }
    Ok(())
}

pub(super) async fn resume_repost_after_restore(
    context: &SubmissionContext,
    app_log_path: &PathBuf,
    task_id: &str,
    mode: ReprocessMode,
    merged_video_id: Option<i64>,
    workflow_type: &str,
) -> Result<(), String> {
    let integrate_current_bvid = workflow_type == "VIDEO_UPDATE";
    let workflow_settings = load_workflow_settings(context, task_id);
    let enable_segmentation = workflow_settings.enable_segmentation;
    let segment_seconds = workflow_settings.segment_duration_seconds.max(1);
    let segment_prefix = workflow_settings.segment_prefix.clone();
    let config = load_latest_workflow_config(context, task_id).ok().flatten();
    let media_process_mode = load_media_process_mode(config.as_ref());
    let use_transcode_media = media_process_mode == MediaProcessMode::ClipAndMergeTranscode;
    let should_segment = integrate_current_bvid || enable_segmentation;
    let base_dir = resolve_submission_base_dir(context, task_id);
    let output_dir = base_dir
        .join("repost")
        .join(sanitize_filename(&format!("repost_{}", now_rfc3339())))
        .join("output");

    let now = now_rfc3339();
    context
    .db
    .with_conn(|conn| {
      if integrate_current_bvid {
        conn.execute(
          "UPDATE submission_task SET status = 'SEGMENTING', remote_state = NULL, remote_state_desc = NULL, reject_reason = NULL, updated_at = ?1 WHERE task_id = ?2",
          (&now, task_id),
        )?;
      } else {
        conn.execute(
          "UPDATE submission_task SET status = 'SEGMENTING', bvid = NULL, aid = NULL, remote_state = NULL, remote_state_desc = NULL, reject_reason = NULL, updated_at = ?1 WHERE task_id = ?2",
          (&now, task_id),
        )?;
      }
      Ok(())
    })
    .map_err(|err| err.to_string())?;

    match mode {
        ReprocessMode::Specified => {
            let merged =
                resolve_target_merged_video(context, task_id, merged_video_id, app_log_path)?;
            let merged_path = merged.video_path.clone().unwrap_or_default();
            if merged_path.trim().is_empty() || !PathBuf::from(&merged_path).exists() {
                return Err("合并视频缺失".to_string());
            }
            let merged_path_buf = PathBuf::from(merged_path);
            let context_clone = context.clone();
            let task_id_clone = task_id.to_string();
            let merged_path_clone = merged_path_buf.clone();
            let output_dir_clone = output_dir.clone();
            let app_log_path = app_log_path.to_path_buf();
            let merged_id = merged.id;
            tauri::async_runtime::spawn(async move {
                let _ = update_workflow_status(
                    &context_clone,
                    &task_id_clone,
                    "RUNNING",
                    Some("SEGMENTING"),
                    70.0,
                );
                if !should_segment {
                    let (replaced_merged_id, replaced_merged_path) =
                        match recreate_selected_merged_for_repost(
                            &context_clone,
                            &task_id_clone,
                            merged_id,
                            merged_path_clone.as_path(),
                            &output_dir_clone,
                        ) {
                            Ok(value) => value,
                            Err(err) => {
                                let _ = update_submission_status(
                                    &context_clone,
                                    &task_id_clone,
                                    "FAILED",
                                );
                                let _ = update_workflow_status(
                                    &context_clone,
                                    &task_id_clone,
                                    "FAILED",
                                    Some("SEGMENTING"),
                                    0.0,
                                );
                                append_log(
                                    app_log_path.as_ref(),
                                    &format!(
                  "submission_repost_replace_merged_fail task_id={} merged_id={} err={}",
                  task_id_clone, merged_id, err
                ),
                                );
                                return;
                            }
                        };
                    if let Err(err) = replace_segments_for_merged(
                        &context_clone,
                        &task_id_clone,
                        replaced_merged_id,
                        &[replaced_merged_path],
                        segment_prefix.as_deref(),
                    ) {
                        let _ = mark_submission_failure(
                            &context_clone,
                            &task_id_clone,
                            Some("SEGMENTING"),
                            &format!("重新投稿保存分段失败: {}", err),
                        );
                        append_log(
                            app_log_path.as_ref(),
                            &format!(
                                "submission_repost_save_fail task_id={} err={}",
                                task_id_clone, err
                            ),
                        );
                        return;
                    }
                    if !integrate_current_bvid {
                        if let Err(err) =
                            reset_segments_for_new_bvid(&context_clone, &task_id_clone)
                        {
                            let _ = mark_submission_failure(
                                &context_clone,
                                &task_id_clone,
                                Some("SEGMENTING"),
                                &format!("重新投稿重置分段元数据失败: {}", err),
                            );
                            append_log(
                                app_log_path.as_ref(),
                                &format!(
                                    "submission_repost_reset_segments_fail task_id={} err={}",
                                    task_id_clone, err
                                ),
                            );
                            return;
                        }
                    }
                    finalize_reprocess_success(
                        &context_clone,
                        &task_id_clone,
                        "submission_repost_ok",
                    );
                    return;
                }

                let (new_merged_id, new_merged_path) = match recreate_selected_merged_for_repost(
                    &context_clone,
                    &task_id_clone,
                    merged_id,
                    merged_path_clone.as_path(),
                    &output_dir_clone,
                ) {
                    Ok(value) => value,
                    Err(err) => {
                        let _ = mark_submission_failure(
                            &context_clone,
                            &task_id_clone,
                            Some("SEGMENTING"),
                            &format!("重新投稿替换合并视频失败: {}", err),
                        );
                        append_log(
                            app_log_path.as_ref(),
                            &format!(
                "submission_repost_replace_merged_fail task_id={} merged_id={} err={}",
                task_id_clone, merged_id, err
              ),
                        );
                        return;
                    }
                };

                let outputs = if enable_segmentation {
                    append_log(
                        app_log_path.as_ref(),
                        &format!(
                            "submission_repost_segment_mode task_id={} mode={}",
                            task_id_clone,
                            if use_transcode_media {
                                "segment_transcode"
                            } else {
                                "segment_copy"
                            }
                        ),
                    );
                    match tauri::async_runtime::spawn_blocking(move || {
                        segment_file_with_options(
                            &new_merged_path,
                            &output_dir_clone,
                            segment_seconds,
                            false,
                            use_transcode_media,
                        )
                    })
                    .await
                    {
                        Ok(result) => result,
                        Err(_) => Err("Failed to segment video".to_string()),
                    }
                } else {
                    Ok(vec![new_merged_path.clone()])
                };
                let outputs = match outputs {
                    Ok(list) => list,
                    Err(err) => {
                        let _ = mark_submission_failure(
                            &context_clone,
                            &task_id_clone,
                            Some("SEGMENTING"),
                            &format!("重新投稿分段失败: {}", err),
                        );
                        append_log(
                            app_log_path.as_ref(),
                            &format!(
                                "submission_repost_segment_fail task_id={} err={}",
                                task_id_clone, err
                            ),
                        );
                        return;
                    }
                };
                if outputs.is_empty() {
                    let _ = mark_submission_failure(
                        &context_clone,
                        &task_id_clone,
                        Some("SEGMENTING"),
                        "重新投稿未生成任何分段文件",
                    );
                    append_log(
                        app_log_path.as_ref(),
                        &format!("submission_repost_empty_outputs task_id={}", task_id_clone),
                    );
                    return;
                }
                if let Err(err) = replace_segments_for_merged(
                    &context_clone,
                    &task_id_clone,
                    new_merged_id,
                    &outputs,
                    segment_prefix.as_deref(),
                ) {
                    let _ = mark_submission_failure(
                        &context_clone,
                        &task_id_clone,
                        Some("SEGMENTING"),
                        &format!("重新投稿保存分段失败: {}", err),
                    );
                    append_log(
                        app_log_path.as_ref(),
                        &format!(
                            "submission_repost_save_fail task_id={} err={}",
                            task_id_clone, err
                        ),
                    );
                    return;
                }
                if !integrate_current_bvid {
                    if let Err(err) = reset_segments_for_new_bvid(&context_clone, &task_id_clone) {
                        let _ = mark_submission_failure(
                            &context_clone,
                            &task_id_clone,
                            Some("SEGMENTING"),
                            &format!("重新投稿重置分段元数据失败: {}", err),
                        );
                        append_log(
                            app_log_path.as_ref(),
                            &format!(
                                "submission_repost_reset_segments_fail task_id={} err={}",
                                task_id_clone, err
                            ),
                        );
                        return;
                    }
                }
                finalize_reprocess_success(&context_clone, &task_id_clone, "submission_repost_ok");
            });
        }
        ReprocessMode::MergeAll => {
            let merged_videos = load_merged_videos_by_task(context, task_id)?;
            if merged_videos.is_empty() {
                return Err("未找到合并视频".to_string());
            }
            let merge_all_sources = collect_sources_for_merge_all(context, task_id, &merged_videos);
            let mut merge_inputs = Vec::with_capacity(merged_videos.len());
            for merged in &merged_videos {
                let path = merged.video_path.clone().unwrap_or_default();
                if path.trim().is_empty() || !PathBuf::from(&path).exists() {
                    return Err("合并视频缺失".to_string());
                }
                merge_inputs.push(PathBuf::from(path));
            }
            let merge_output = build_merge_output_path_for_task_id(context, &base_dir, task_id)?;
            let context_clone = context.clone();
            let task_id_clone = task_id.to_string();
            let output_dir_clone = output_dir.clone();
            let merge_output_for_merge = merge_output.clone();
            let merge_output_for_segment = merge_output.clone();
            let merge_output_for_save = merge_output.clone();
            let app_log_path = app_log_path.to_path_buf();
            tauri::async_runtime::spawn(async move {
                let _ = update_workflow_status(
                    &context_clone,
                    &task_id_clone,
                    "RUNNING",
                    Some("SEGMENTING"),
                    70.0,
                );
                let merge_inputs_clone = merge_inputs.clone();
                let merge_result = tauri::async_runtime::spawn_blocking(move || {
                    merge_files(&merge_inputs_clone, &merge_output_for_merge)
                })
                .await;
                match merge_result {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => {
                        let _ = mark_submission_failure(
                            &context_clone,
                            &task_id_clone,
                            Some("SEGMENTING"),
                            &format!("重新投稿合并失败: {}", err),
                        );
                        append_log(
                            app_log_path.as_ref(),
                            &format!(
                                "submission_repost_merge_fail task_id={} err={}",
                                task_id_clone, err
                            ),
                        );
                        return;
                    }
                    Err(err) => {
                        let _ = mark_submission_failure(
                            &context_clone,
                            &task_id_clone,
                            Some("SEGMENTING"),
                            &format!("重新投稿合并失败: {}", err),
                        );
                        append_log(
                            app_log_path.as_ref(),
                            &format!(
                                "submission_repost_merge_fail task_id={} err={}",
                                task_id_clone, err
                            ),
                        );
                        return;
                    }
                };
                let segment_outputs = if should_segment {
                    if enable_segmentation {
                        let merge_output_for_segment_clone = merge_output_for_segment.clone();
                        append_log(
                            app_log_path.as_ref(),
                            &format!(
                                "submission_repost_segment_mode task_id={} mode={}",
                                task_id_clone,
                                if use_transcode_media {
                                    "segment_transcode"
                                } else {
                                    "segment_copy"
                                }
                            ),
                        );
                        match tauri::async_runtime::spawn_blocking(move || {
                            segment_file_with_options(
                                &merge_output_for_segment_clone,
                                &output_dir_clone,
                                segment_seconds,
                                false,
                                use_transcode_media,
                            )
                        })
                        .await
                        {
                            Ok(result) => result,
                            Err(_) => Err("Failed to segment video".to_string()),
                        }
                    } else {
                        Ok(vec![merge_output_for_segment.clone()])
                    }
                } else {
                    Ok(Vec::new())
                };
                match segment_outputs {
                    Ok(outputs) => {
                        if should_segment && outputs.is_empty() {
                            let _ = mark_submission_failure(
                                &context_clone,
                                &task_id_clone,
                                Some("SEGMENTING"),
                                "重新投稿未生成任何分段文件",
                            );
                            append_log(
                                app_log_path.as_ref(),
                                &format!(
                                    "submission_repost_empty_outputs task_id={}",
                                    task_id_clone
                                ),
                            );
                            return;
                        }
                        if let Err(err) = context_clone.db.with_conn(|conn| {
                            conn.execute(
                                "DELETE FROM merged_video WHERE task_id = ?1",
                                [&task_id_clone],
                            )?;
                            conn.execute(
                                "DELETE FROM task_output_segment WHERE task_id = ?1",
                                [&task_id_clone],
                            )?;
                            Ok(())
                        }) {
                            let _ = mark_submission_failure(
                                &context_clone,
                                &task_id_clone,
                                Some("SEGMENTING"),
                                &format!("重新投稿清理旧合并/分段记录失败: {}", err),
                            );
                            append_log(
                                app_log_path.as_ref(),
                                &format!(
                                    "submission_repost_cleanup_fail task_id={} err={}",
                                    task_id_clone, err
                                ),
                            );
                            return;
                        }
                        let merged_id = match save_merged_video(
                            &context_clone,
                            &task_id_clone,
                            &merge_output_for_save,
                        ) {
                            Ok(merged_id) => merged_id,
                            Err(err) => {
                                let _ = mark_submission_failure(
                                    &context_clone,
                                    &task_id_clone,
                                    Some("SEGMENTING"),
                                    &format!("重新投稿保存合并视频失败: {}", err),
                                );
                                append_log(
                                    app_log_path.as_ref(),
                                    &format!(
                                        "submission_repost_save_fail task_id={} err={}",
                                        task_id_clone, err
                                    ),
                                );
                                return;
                            }
                        };
                        if let Err(err) = save_merged_source_bindings(
                            &context_clone,
                            &task_id_clone,
                            merged_id,
                            &merge_all_sources,
                        ) {
                            append_log(
                                app_log_path.as_ref(),
                                &format!(
                  "submission_repost_bind_sources_fail task_id={} merged_id={} err={}",
                  task_id_clone, merged_id, err
                ),
                            );
                        }
                        if should_segment {
                            if let Err(err) = save_output_segments(
                                &context_clone,
                                &task_id_clone,
                                &outputs,
                                Some(merged_id),
                                segment_prefix.as_deref(),
                            ) {
                                let _ = mark_submission_failure(
                                    &context_clone,
                                    &task_id_clone,
                                    Some("SEGMENTING"),
                                    &format!("重新投稿保存分段失败: {}", err),
                                );
                                append_log(
                                    app_log_path.as_ref(),
                                    &format!(
                                        "submission_repost_save_fail task_id={} err={}",
                                        task_id_clone, err
                                    ),
                                );
                                return;
                            }
                        }
                        finalize_reprocess_success(
                            &context_clone,
                            &task_id_clone,
                            "submission_repost_ok",
                        );
                    }
                    Err(err) => {
                        let _ = mark_submission_failure(
                            &context_clone,
                            &task_id_clone,
                            Some("SEGMENTING"),
                            &format!("重新投稿分段失败: {}", err),
                        );
                        append_log(
                            app_log_path.as_ref(),
                            &format!(
                                "submission_repost_segment_fail task_id={} err={}",
                                task_id_clone, err
                            ),
                        );
                    }
                }
            });
        }
        ReprocessMode::Legacy | ReprocessMode::FullReprocess => {
            append_log(
                app_log_path,
                &format!(
                    "submission_repost_resume_skip task_id={} mode=LEGACY",
                    task_id
                ),
            );
        }
    }
    Ok(())
}

pub async fn resume_reprocess_after_baidu_restore(
    db: Arc<Db>,
    app_log_path: Arc<PathBuf>,
    edit_upload_state: Arc<Mutex<EditUploadState>>,
    task_id: String,
    download_record_id: i64,
) {
    let local_path_prefix = load_local_path_prefix(db.as_ref());
    let context = SubmissionContext {
        db,
        app_log_path: app_log_path.clone(),
        edit_upload_state,
        local_path_prefix,
    };
    let record = context.db.with_conn(|conn| {
        conn.query_row(
            "SELECT download_url, local_path, status FROM video_download WHERE id = ?1",
            [download_record_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
    });
    let record = match record {
        Ok(Some(value)) => value,
        _ => return,
    };
    let (download_url, local_path, status) = record;
    if status != 2 {
        let _ = update_submission_status(&context, &task_id, "PENDING");
        let _ = update_workflow_status(&context, &task_id, "VIDEO_DOWNLOADING", None, 0.0);
        return;
    }
    let local_path = match local_path {
        Some(value) if !value.trim().is_empty() => value,
        _ => return,
    };
    let local_path_buf =
        to_absolute_local_path_with_prefix(context.local_path_prefix.as_path(), &local_path);
    if !local_path_buf.exists() {
        let _ = update_submission_status(&context, &task_id, "PENDING");
        let _ = update_workflow_status(&context, &task_id, "VIDEO_DOWNLOADING", None, 0.0);
        return;
    }
    if let Some(remote_path) = download_url {
        if let Some((remote_dir, remote_name)) = split_baidu_path(&remote_path) {
            let current_baidu_uid = load_logged_baidu_uid(context.db.as_ref()).ok().flatten();
            let _ = update_merged_video_by_remote_binding(
                &context,
                &task_id,
                &remote_dir,
                &remote_name,
                local_path_buf.to_string_lossy().as_ref(),
                current_baidu_uid.as_deref(),
            );
        }
    }
    let (total, completed, failed) =
        load_remote_restore_stats(&context, &task_id).unwrap_or((0, 0, 0));
    if total == 0 {
        return;
    }
    if failed > 0 || completed < total {
        let _ = update_submission_status(&context, &task_id, "PENDING");
        let _ = update_workflow_status(&context, &task_id, "VIDEO_DOWNLOADING", None, 0.0);
        let _ = update_remote_restore_status(&context, &task_id, "PENDING_DOWNLOAD");
        return;
    }
    let _ = update_remote_restore_status(&context, &task_id, "READY");
    let workflow_type = load_latest_workflow_type(&context, &task_id)
        .ok()
        .flatten()
        .unwrap_or_default();
    let config = load_latest_workflow_config(&context, &task_id)
        .ok()
        .flatten();
    let (mode, merged_id) = load_reprocess_metadata(config.as_ref());
    let app_log_path = app_log_path.as_ref();
    if workflow_type == "VIDEO_RESEGMENT" {
        let _ =
            resume_resegment_after_restore(&context, app_log_path, &task_id, mode, merged_id).await;
    } else if workflow_type == "VIDEO_UPDATE" || workflow_type == "VIDEO_SUBMISSION" {
        let _ = resume_repost_after_restore(
            &context,
            app_log_path,
            &task_id,
            mode,
            merged_id,
            &workflow_type,
        )
        .await;
    } else {
        append_log(
            app_log_path,
            &format!(
                "submission_baidu_restore_skip task_id={} workflow_type={}",
                task_id, workflow_type
            ),
        );
    }
}

pub(super) fn collect_retry_download_records_for_missing_sources(
    context: &SubmissionContext,
    task_id: &str,
    missing_sources: &[String],
) -> Result<(Vec<IntegratedDownloadRecord>, Vec<String>), String> {
    let mut integrated_by_path: HashMap<String, IntegratedDownloadRecord> = HashMap::new();
    for record in load_integrated_download_records(context, task_id)? {
        integrated_by_path.insert(record.local_path.clone(), record);
    }
    let source_bindings = load_source_retry_bindings(context, task_id)?;

    let mut retry_records = Vec::new();
    let mut unresolved = Vec::new();
    let mut handled_paths = HashSet::new();

    for missing in missing_sources {
        let runtime_path = normalize_runtime_source_path(context, missing);
        if runtime_path.trim().is_empty() {
            unresolved.push(missing.clone());
            continue;
        }
        if !handled_paths.insert(runtime_path.clone()) {
            continue;
        }
        if let Some(record) = integrated_by_path.remove(&runtime_path) {
            retry_records.push(record);
            continue;
        }
        if let Some(binding) = source_bindings.get(&runtime_path) {
            let binding_type = binding.source_type.trim().to_ascii_uppercase();
            let binding_url = binding.remote_video_url.as_deref().unwrap_or("").trim();
            let binding_remote_path = binding.remote_path.as_deref().unwrap_or("").trim();
            if binding_type == "BAIDU" || !binding_remote_path.is_empty() {
                retry_records.push(IntegratedDownloadRecord {
                    id: 0,
                    download_url: if !binding_remote_path.is_empty() {
                        binding_remote_path.to_string()
                    } else {
                        binding_url.to_string()
                    },
                    bvid: None,
                    aid: None,
                    title: binding.remote_part_title.clone(),
                    part_title: binding.remote_part_title.clone(),
                    part_count: Some(1),
                    current_part: Some(1),
                    local_path: runtime_path.clone(),
                    resolution: None,
                    codec: None,
                    format: None,
                    cid: None,
                    content: None,
                    source_type: crate::commands::download::DOWNLOAD_SOURCE_BAIDU.to_string(),
                });
                continue;
            }
            let is_direct_url = !binding_url.is_empty()
                && binding
                    .remote_bvid
                    .as_deref()
                    .map(|value| value.trim().is_empty())
                    .unwrap_or(true);
            if is_direct_url {
                retry_records.push(IntegratedDownloadRecord {
                    id: 0,
                    download_url: binding_url.to_string(),
                    bvid: None,
                    aid: None,
                    title: binding.remote_part_title.clone(),
                    part_title: binding.remote_part_title.clone(),
                    part_count: Some(1),
                    current_part: Some(1),
                    local_path: runtime_path.clone(),
                    resolution: None,
                    codec: None,
                    format: None,
                    cid: None,
                    content: None,
                    source_type: crate::commands::download::DOWNLOAD_SOURCE_DIRECT.to_string(),
                });
                continue;
            }
            if binding.remote_cid.is_some()
                && (binding
                    .remote_bvid
                    .as_deref()
                    .map(|value| !value.trim().is_empty())
                    .unwrap_or(false)
                    || binding
                        .remote_aid
                        .as_deref()
                        .map(|value| !value.trim().is_empty())
                        .unwrap_or(false))
            {
                retry_records.push(IntegratedDownloadRecord {
                    id: 0,
                    download_url: binding.remote_video_url.clone().unwrap_or_default(),
                    bvid: binding.remote_bvid.clone(),
                    aid: binding.remote_aid.clone(),
                    title: None,
                    part_title: binding.remote_part_title.clone(),
                    part_count: None,
                    current_part: None,
                    local_path: runtime_path.clone(),
                    resolution: None,
                    codec: None,
                    format: None,
                    cid: binding.remote_cid,
                    content: None,
                    source_type: "BILIBILI".to_string(),
                });
                continue;
            }
        }
        unresolved.push(runtime_path);
    }

    Ok((retry_records, unresolved))
}

pub(super) async fn enqueue_missing_source_downloads_for_repost(
    state: &State<'_, AppState>,
    context: &SubmissionContext,
    task_id: &str,
    workflow_config: &Value,
    integrate_current_bvid: bool,
    missing_sources: &[String],
    reason: &str,
    clean_updates: bool,
) -> Result<ApiResponse<String>, String> {
    append_log(
        &state.app_log_path,
        &format!(
            "submission_repost_missing_sources task_id={} count={}",
            task_id,
            missing_sources.len()
        ),
    );
    for path in missing_sources {
        append_log(
            &state.app_log_path,
            &format!(
                "submission_repost_missing_source task_id={} path={}",
                task_id, path
            ),
        );
    }

    let (retry_records, unresolved_sources) =
        collect_retry_download_records_for_missing_sources(context, task_id, missing_sources)?;
    if !unresolved_sources.is_empty() {
        append_log(
            &state.app_log_path,
            &format!(
                "submission_repost_missing_unresolved task_id={} count={}",
                task_id,
                unresolved_sources.len()
            ),
        );
        for path in &unresolved_sources {
            append_log(
                &state.app_log_path,
                &format!(
                    "submission_repost_missing_unresolved_path task_id={} path={}",
                    task_id, path
                ),
            );
        }
        return Ok(ApiResponse::error(
            "源视频不存在，且未绑定可重下信息，请先绑定下载记录或远程视频信息".to_string(),
        ));
    }

    let workflow_instance_id = reset_submission_for_repost(
        context,
        &state.app_log_path,
        task_id,
        workflow_config,
        if integrate_current_bvid {
            "VIDEO_UPDATE"
        } else {
            "VIDEO_SUBMISSION"
        },
        !integrate_current_bvid,
        clean_updates,
    )?;
    let new_download_ids =
        create_retry_download_records(context, task_id, &workflow_instance_id, &retry_records)?;
    crate::commands::download::requeue_integrated_downloads(state, &new_download_ids).await?;
    Ok(ApiResponse::success(format!(
        "{}，已创建下载任务，下载完成后自动重新投稿",
        reason
    )))
}

pub(super) fn reset_submission_for_repost(
    context: &SubmissionContext,
    app_log_path: &PathBuf,
    task_id: &str,
    workflow_config: &Value,
    workflow_type: &str,
    clear_bvid: bool,
    clean_updates: bool,
) -> Result<String, String> {
    append_log(
        app_log_path,
        &format!(
            "submission_repost_start task_id={} type={}",
            task_id, workflow_type
        ),
    );
    if let Err(err) = clear_edit_upload_segments_by_task(context, task_id) {
        append_log(
            app_log_path,
            &format!(
                "submission_repost_clear_cache_fail task_id={} err={}",
                task_id, err
            ),
        );
    }
    reset_workflow_instances(context, task_id).map_err(|err| format!("重置工作流失败: {}", err))?;
    let cleanup_result = context.db.with_conn(|conn| {
        conn.execute(
            "DELETE FROM task_output_segment WHERE task_id = ?1",
            [task_id],
        )?;
        conn.execute(
            "DELETE FROM merged_source_video WHERE task_id = ?1",
            [task_id],
        )?;
        conn.execute("DELETE FROM merged_video WHERE task_id = ?1", [task_id])?;
        conn.execute("DELETE FROM video_clip WHERE task_id = ?1", [task_id])?;
        Ok(())
    });
    if let Err(err) = cleanup_result {
        return Err(format!("清理任务数据失败: {}", err));
    }
    let base_dir = resolve_submission_base_dir(context, task_id);
    cleanup_submission_derived_files_with_retry(app_log_path, &base_dir).map_err(|err| {
        format!(
            "清理旧流程文件失败: {}. 请稍后重试，避免重复投稿任务并发执行",
            err
        )
    })?;
    if clean_updates {
        let updates_dir = base_dir.join("updates");
        remove_path_if_exists_with_retry(
            app_log_path,
            "updates",
            &updates_dir,
            5,
            Duration::from_millis(300),
        )
        .map_err(|err| format!("清理更新目录失败: {}", err))?;
    }
    let now = now_rfc3339();
    let update_result = context.db.with_conn(|conn| {
    if clear_bvid {
      conn.execute(
        "UPDATE submission_task SET status = 'PENDING', bvid = NULL, aid = NULL, remote_state = NULL, remote_state_desc = NULL, reject_reason = NULL, updated_at = ?1 WHERE task_id = ?2",
        (&now, task_id),
      )?;
    } else {
      conn.execute(
        "UPDATE submission_task SET status = 'PENDING', remote_state = NULL, remote_state_desc = NULL, reject_reason = NULL, updated_at = ?1 WHERE task_id = ?2",
        (&now, task_id),
      )?;
    }
    Ok(())
  });
    if let Err(err) = update_result {
        return Err(format!("重置任务状态失败: {}", err));
    }
    let (instance_id, _) = create_workflow_instance_for_task_with_type(
        context.db.as_ref(),
        task_id,
        workflow_config,
        workflow_type,
    )
    .map_err(|err| format!("创建工作流失败: {}", err))?;
    let _ = context.db.with_conn(|conn| {
    conn.execute(
      "UPDATE task_relations SET workflow_instance_id = ?1, updated_at = ?2 WHERE submission_task_id = ?3 AND relation_type = 'INTEGRATED'",
      (&instance_id, &now, task_id),
    )?;
    Ok(())
  });
    Ok(instance_id)
}

#[tauri::command]
pub async fn submission_resegment(
    state: State<'_, AppState>,
    request: SubmissionResegmentRequest,
) -> Result<ApiResponse<String>, String> {
    let context = SubmissionContext::new(&state);
    let task_id = request.task_id.trim().to_string();
    if task_id.is_empty() {
        return Ok(ApiResponse::error("任务ID不能为空"));
    }
    if request.segment_duration_seconds <= 0 {
        return Ok(ApiResponse::error("分段时长必须大于0"));
    }
    let detail = match load_task_detail(&context, &task_id) {
        Ok(detail) => detail,
        Err(err) => return Ok(ApiResponse::error(err)),
    };
    let base_dir = resolve_submission_base_dir(&context, &task_id);
    let mode = parse_reprocess_mode(request.mode.as_deref());
    let integrate_current_bvid = request.integrate_current_bvid.unwrap_or(false);
    if integrate_current_bvid && detail.task.bvid.as_deref().unwrap_or("").trim().is_empty() {
        return Ok(ApiResponse::error(
            "当前任务暂无BVID，无法集成投稿，请选择新建BV".to_string(),
        ));
    }
    if mode != ReprocessMode::Legacy {
        append_log(
            &state.app_log_path,
            &format!(
                "submission_resegment_start task_id={} mode={}",
                task_id,
                if mode == ReprocessMode::MergeAll {
                    "MERGE_ALL"
                } else {
                    "SPECIFIED"
                }
            ),
        );
        let mut updated_config = build_resegment_workflow_config(
            detail.workflow_config.clone(),
            request.segment_duration_seconds,
        );
        let started_from_terminal = is_submission_terminal_status(detail.task.status.as_str());
        apply_reprocess_metadata(&mut updated_config, mode, request.merged_video_id);
        apply_integrate_current_bvid(&mut updated_config, integrate_current_bvid);
        apply_resegment_terminal_metadata(&mut updated_config, started_from_terminal);
        let reprocess_source_paths = resolve_reprocess_source_paths(
            &context,
            &detail,
            &updated_config,
            mode,
            request.merged_video_id,
        );
        let source_override = if mode == ReprocessMode::Specified {
            Some(reprocess_source_paths.clone())
        } else {
            None
        };
        if let Err(err) = clear_edit_upload_segments_by_task(&context, &task_id) {
            append_log(
                &state.app_log_path,
                &format!(
                    "submission_resegment_clear_cache_fail task_id={} err={}",
                    task_id, err
                ),
            );
        }
        if let Err(err) = reset_workflow_instances(&context, &task_id) {
            return Ok(ApiResponse::error(format!("重置工作流失败: {}", err)));
        }
        let (workflow_instance_id, _) = match create_workflow_instance_for_task_with_type(
            context.db.as_ref(),
            &task_id,
            &updated_config,
            "VIDEO_RESEGMENT",
        ) {
            Ok(result) => result,
            Err(err) => return Ok(ApiResponse::error(format!("创建工作流失败: {}", err))),
        };
        let now = now_rfc3339();
        let update_result = context.db.with_conn(|conn| {
      if integrate_current_bvid {
        conn.execute(
          "UPDATE submission_task SET status = 'SEGMENTING', remote_state = NULL, remote_state_desc = NULL, reject_reason = NULL, updated_at = ?1 WHERE task_id = ?2",
          (&now, &task_id),
        )?;
      } else {
        conn.execute(
          "UPDATE submission_task SET status = 'SEGMENTING', bvid = NULL, aid = NULL, remote_state = NULL, remote_state_desc = NULL, reject_reason = NULL, updated_at = ?1 WHERE task_id = ?2",
          (&now, &task_id),
        )?;
      }
      Ok(())
    });
        if let Err(err) = update_result {
            return Ok(ApiResponse::error(format!("重置任务数据失败: {}", err)));
        }
        let output_dir = base_dir
            .join("resegment")
            .join(sanitize_filename(&format!("resegment_{}", now_rfc3339())))
            .join("output");
        let segment_seconds = request.segment_duration_seconds;
        if mode == ReprocessMode::Specified {
            let mut merged = match resolve_target_merged_video(
                &context,
                &task_id,
                request.merged_video_id,
                &state.app_log_path,
            ) {
                Ok(merged) => merged,
                Err(_) => {
                    return handle_repost_missing_assets(
                        &state,
                        &context,
                        &detail,
                        &task_id,
                        &updated_config,
                        integrate_current_bvid,
                        FALLBACK_REASON_RESEGMENT_MISSING_MERGED,
                        source_override.clone(),
                    )
                    .await;
                }
            };
            let mut merged_path = merged.video_path.clone().unwrap_or_default();
            let mut fallback_to_source_download = false;
            if merged_path.trim().is_empty() || !PathBuf::from(&merged_path).exists() {
                match try_restore_merged_from_baidu(
                    &context,
                    &state.app_log_path,
                    &merged,
                    &base_dir,
                )
                .await?
                {
                    BaiduRestoreResult::Ready(restored) => {
                        merged_path = restored.to_string_lossy().to_string();
                        merged.video_path = Some(merged_path.clone());
                    }
                    BaiduRestoreResult::Queued => {
                        let _ = update_submission_status(&context, &task_id, "PENDING");
                        let _ = update_workflow_status(
                            &context,
                            &task_id,
                            "VIDEO_DOWNLOADING",
                            None,
                            0.0,
                        );
                        return Ok(ApiResponse::success(
                            "合并视频缺失，已创建网盘下载任务，下载完成后自动重新分段".to_string(),
                        ));
                    }
                    BaiduRestoreResult::NotBound => {
                        fallback_to_source_download = true;
                    }
                }
            }
            if fallback_to_source_download || merged_path.trim().is_empty() {
                return handle_repost_missing_assets(
                    &state,
                    &context,
                    &detail,
                    &task_id,
                    &updated_config,
                    integrate_current_bvid,
                    FALLBACK_REASON_RESEGMENT_MISSING_MERGED,
                    source_override.clone(),
                )
                .await;
            }
            let mut active_merged_path = PathBuf::from(merged_path.clone());
            if !active_merged_path.exists() {
                return handle_repost_missing_assets(
                    &state,
                    &context,
                    &detail,
                    &task_id,
                    &updated_config,
                    integrate_current_bvid,
                    FALLBACK_REASON_RESEGMENT_MISSING_MERGED,
                    source_override.clone(),
                )
                .await;
            }
            let mut active_merged_id = merged.id;
            if !started_from_terminal {
                let (new_merged_id, new_merged_path) = match recreate_selected_merged_for_resegment(
                    &context,
                    &task_id,
                    merged.id,
                    active_merged_path.as_path(),
                    &output_dir,
                ) {
                    Ok(value) => value,
                    Err(err) => return Ok(ApiResponse::error(err)),
                };
                active_merged_id = new_merged_id;
                active_merged_path = new_merged_path;
            }
            let segment_prefix = detail.task.segment_prefix.clone();
            let context_clone = context.clone();
            let task_id_clone = task_id.clone();
            let merged_path_clone = active_merged_path.clone();
            let output_dir_clone = output_dir.clone();
            let app_log_path = state.app_log_path.clone();
            let merged_id = active_merged_id;
            let use_replace_segments = started_from_terminal;
            let workflow_instance_id_clone = workflow_instance_id.clone();
            tauri::async_runtime::spawn(async move {
                let _ = update_workflow_status(
                    &context_clone,
                    &task_id_clone,
                    "RUNNING",
                    Some("SEGMENTING"),
                    70.0,
                );
                let merged_path_for_segment = merged_path_clone.clone();
                let segment_outputs = match tauri::async_runtime::spawn_blocking(move || {
                    segment_file(&merged_path_for_segment, &output_dir_clone, segment_seconds)
                })
                .await
                {
                    Ok(result) => result,
                    Err(_) => Err("Failed to segment video".to_string()),
                };
                let still_latest = ensure_workflow_instance_latest(
                    &context_clone,
                    &task_id_clone,
                    &workflow_instance_id_clone,
                    "RESEGMENT_SPECIFIED_POST_SEGMENT",
                )
                .unwrap_or(false);
                if !still_latest {
                    return;
                }
                match segment_outputs {
                    Ok(outputs) => {
                        if outputs.is_empty() {
                            let _ = mark_submission_failure(
                                &context_clone,
                                &task_id_clone,
                                Some("SEGMENTING"),
                                "重新分段未生成任何分段文件",
                            );
                            append_log(
                                app_log_path.as_ref(),
                                &format!(
                                    "submission_resegment_empty_outputs task_id={}",
                                    task_id_clone
                                ),
                            );
                            return;
                        }
                        let save_result = if use_replace_segments {
                            replace_segments_for_merged(
                                &context_clone,
                                &task_id_clone,
                                merged_id,
                                &outputs,
                                segment_prefix.as_deref(),
                            )
                        } else {
                            save_output_segments(
                                &context_clone,
                                &task_id_clone,
                                &outputs,
                                Some(merged_id),
                                segment_prefix.as_deref(),
                            )
                        };
                        if let Err(err) = save_result {
                            let _ = mark_submission_failure(
                                &context_clone,
                                &task_id_clone,
                                Some("SEGMENTING"),
                                &format!("重新分段保存分段失败: {}", err),
                            );
                            append_log(
                                app_log_path.as_ref(),
                                &format!(
                                    "submission_resegment_save_fail task_id={} err={}",
                                    task_id_clone, err
                                ),
                            );
                            return;
                        }
                        if !integrate_current_bvid {
                            if let Err(err) =
                                reset_segments_for_new_bvid(&context_clone, &task_id_clone)
                            {
                                let _ = mark_submission_failure(
                                    &context_clone,
                                    &task_id_clone,
                                    Some("SEGMENTING"),
                                    &format!("重新分段重置分段元数据失败: {}", err),
                                );
                                append_log(
                                    app_log_path.as_ref(),
                                    &format!(
                    "submission_resegment_reset_segments_fail task_id={} err={}",
                    task_id_clone, err
                  ),
                                );
                                return;
                            }
                        }
                        finalize_reprocess_success(
                            &context_clone,
                            &task_id_clone,
                            "submission_resegment_ok",
                        );
                    }
                    Err(err) => {
                        let _ = mark_submission_failure(
                            &context_clone,
                            &task_id_clone,
                            Some("SEGMENTING"),
                            &format!("重新分段失败: {}", err),
                        );
                        append_log(
                            app_log_path.as_ref(),
                            &format!(
                                "submission_resegment_segment_fail task_id={} err={}",
                                task_id_clone, err
                            ),
                        );
                    }
                }
            });
            return Ok(ApiResponse::success("重新分段已启动".to_string()));
        }
        let mut merged_videos = match load_merged_videos_by_task(&context, &task_id) {
            Ok(list) => list,
            Err(err) => return Ok(ApiResponse::error(err)),
        };
        if merged_videos.is_empty() {
            return handle_repost_missing_assets(
                &state,
                &context,
                &detail,
                &task_id,
                &updated_config,
                integrate_current_bvid,
                FALLBACK_REASON_RESEGMENT_MISSING_MERGED,
                source_override.clone(),
            )
            .await;
        }
        let old_merged_ids = merged_videos
            .iter()
            .map(|record| record.id)
            .collect::<Vec<_>>();
        let merge_all_sources = collect_sources_for_merge_all(&context, &task_id, &merged_videos);
        let mut merge_inputs = Vec::with_capacity(merged_videos.len());
        let mut fallback_to_source_download = false;
        for merged in &mut merged_videos {
            let mut path = merged.video_path.clone().unwrap_or_default();
            if path.trim().is_empty() || !PathBuf::from(&path).exists() {
                match try_restore_merged_from_baidu(
                    &context,
                    &state.app_log_path,
                    merged,
                    &base_dir,
                )
                .await?
                {
                    BaiduRestoreResult::Ready(restored) => {
                        path = restored.to_string_lossy().to_string();
                        merged.video_path = Some(path.clone());
                    }
                    BaiduRestoreResult::Queued => {
                        let _ = update_submission_status(&context, &task_id, "PENDING");
                        let _ = update_workflow_status(
                            &context,
                            &task_id,
                            "VIDEO_DOWNLOADING",
                            None,
                            0.0,
                        );
                        return Ok(ApiResponse::success(format!(
                            "{} 缺失，已创建网盘下载任务，下载完成后自动重新分段",
                            merged_display_name(merged)
                        )));
                    }
                    BaiduRestoreResult::NotBound => {
                        fallback_to_source_download = true;
                        break;
                    }
                }
            }
            if path.trim().is_empty() {
                fallback_to_source_download = true;
                break;
            }
            let path_buf = PathBuf::from(&path);
            if !path_buf.exists() {
                fallback_to_source_download = true;
                break;
            }
            merge_inputs.push(path_buf);
        }
        if fallback_to_source_download {
            return handle_repost_missing_assets(
                &state,
                &context,
                &detail,
                &task_id,
                &updated_config,
                integrate_current_bvid,
                FALLBACK_REASON_RESEGMENT_MISSING_MERGED,
                source_override.clone(),
            )
            .await;
        }
        let merge_workflow_dir = if started_from_terminal {
            base_dir.clone()
        } else {
            output_dir
                .parent()
                .map(|path| path.to_path_buf())
                .unwrap_or_else(|| base_dir.clone())
        };
        let merge_output = build_merge_output_path_for_task(&merge_workflow_dir, &detail.task);
        let context_clone = context.clone();
        let task_id_clone = task_id.clone();
        let output_dir_clone = output_dir.clone();
        let merge_output_for_merge = merge_output.clone();
        let merge_output_for_segment = merge_output.clone();
        let merge_output_for_save = merge_output.clone();
        let segment_prefix = detail.task.segment_prefix.clone();
        let app_log_path = state.app_log_path.clone();
        let workflow_instance_id_clone = workflow_instance_id.clone();
        let old_merged_ids_clone = old_merged_ids.clone();
        tauri::async_runtime::spawn(async move {
            let _ = update_workflow_status(
                &context_clone,
                &task_id_clone,
                "RUNNING",
                Some("SEGMENTING"),
                70.0,
            );
            let merge_inputs_clone = merge_inputs.clone();
            let merge_result = tauri::async_runtime::spawn_blocking(move || {
                merge_files(&merge_inputs_clone, &merge_output_for_merge)
            })
            .await;
            match merge_result {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    let _ = mark_submission_failure(
                        &context_clone,
                        &task_id_clone,
                        Some("SEGMENTING"),
                        &format!("重新分段合并失败: {}", err),
                    );
                    append_log(
                        app_log_path.as_ref(),
                        &format!(
                            "submission_resegment_merge_fail task_id={} err={}",
                            task_id_clone, err
                        ),
                    );
                    return;
                }
                Err(err) => {
                    let _ = mark_submission_failure(
                        &context_clone,
                        &task_id_clone,
                        Some("SEGMENTING"),
                        &format!("重新分段合并失败: {}", err),
                    );
                    append_log(
                        app_log_path.as_ref(),
                        &format!(
                            "submission_resegment_merge_fail task_id={} err={}",
                            task_id_clone, err
                        ),
                    );
                    return;
                }
            };
            let still_latest_after_merge = ensure_workflow_instance_latest(
                &context_clone,
                &task_id_clone,
                &workflow_instance_id_clone,
                "RESEGMENT_MERGE_ALL_POST_MERGE",
            )
            .unwrap_or(false);
            if !still_latest_after_merge {
                return;
            }
            let merge_output_for_segment_clone = merge_output_for_segment.clone();
            let segment_outputs = match tauri::async_runtime::spawn_blocking(move || {
                segment_file_with_options(
                    &merge_output_for_segment_clone,
                    &output_dir_clone,
                    segment_seconds,
                    false,
                    false,
                )
            })
            .await
            {
                Ok(result) => result,
                Err(_) => Err("Failed to segment video".to_string()),
            };
            let still_latest_after_segment = ensure_workflow_instance_latest(
                &context_clone,
                &task_id_clone,
                &workflow_instance_id_clone,
                "RESEGMENT_MERGE_ALL_POST_SEGMENT",
            )
            .unwrap_or(false);
            if !still_latest_after_segment {
                return;
            }
            match segment_outputs {
                Ok(outputs) => {
                    if outputs.is_empty() {
                        let _ = mark_submission_failure(
                            &context_clone,
                            &task_id_clone,
                            Some("SEGMENTING"),
                            "重新分段未生成任何分段文件",
                        );
                        append_log(
                            app_log_path.as_ref(),
                            &format!(
                                "submission_resegment_empty_outputs task_id={}",
                                task_id_clone
                            ),
                        );
                        return;
                    }
                    let merged_id = match save_merged_video(
                        &context_clone,
                        &task_id_clone,
                        &merge_output_for_save,
                    ) {
                        Ok(merged_id) => merged_id,
                        Err(err) => {
                            let _ = mark_submission_failure(
                                &context_clone,
                                &task_id_clone,
                                Some("SEGMENTING"),
                                &format!("重新分段保存合并视频失败: {}", err),
                            );
                            append_log(
                                app_log_path.as_ref(),
                                &format!(
                                    "submission_resegment_save_fail task_id={} err={}",
                                    task_id_clone, err
                                ),
                            );
                            return;
                        }
                    };
                    if let Err(err) = save_merged_source_bindings(
                        &context_clone,
                        &task_id_clone,
                        merged_id,
                        &merge_all_sources,
                    ) {
                        append_log(
                            app_log_path.as_ref(),
                            &format!(
                "submission_resegment_bind_sources_fail task_id={} merged_id={} err={}",
                task_id_clone, merged_id, err
              ),
                        );
                    }
                    if let Err(err) = save_output_segments(
                        &context_clone,
                        &task_id_clone,
                        &outputs,
                        Some(merged_id),
                        segment_prefix.as_deref(),
                    ) {
                        let _ = mark_submission_failure(
                            &context_clone,
                            &task_id_clone,
                            Some("SEGMENTING"),
                            &format!("重新分段保存分段失败: {}", err),
                        );
                        append_log(
                            app_log_path.as_ref(),
                            &format!(
                                "submission_resegment_save_fail task_id={} err={}",
                                task_id_clone, err
                            ),
                        );
                        return;
                    }
                    let stale_merged_ids = old_merged_ids_clone
                        .iter()
                        .copied()
                        .filter(|id| *id != merged_id)
                        .collect::<Vec<_>>();
                    if let Err(err) = delete_merged_records_by_ids(
                        &context_clone,
                        &task_id_clone,
                        &stale_merged_ids,
                    ) {
                        let _ = mark_submission_failure(
                            &context_clone,
                            &task_id_clone,
                            Some("SEGMENTING"),
                            &format!("重新分段清理旧合并记录失败: {}", err),
                        );
                        append_log(
                            app_log_path.as_ref(),
                            &format!(
                                "submission_resegment_cleanup_fail task_id={} err={}",
                                task_id_clone, err
                            ),
                        );
                        return;
                    }
                    if !integrate_current_bvid {
                        if let Err(err) =
                            reset_segments_for_new_bvid(&context_clone, &task_id_clone)
                        {
                            let _ = mark_submission_failure(
                                &context_clone,
                                &task_id_clone,
                                Some("SEGMENTING"),
                                &format!("重新分段重置分段元数据失败: {}", err),
                            );
                            append_log(
                                app_log_path.as_ref(),
                                &format!(
                                    "submission_resegment_reset_segments_fail task_id={} err={}",
                                    task_id_clone, err
                                ),
                            );
                            return;
                        }
                    }
                    finalize_reprocess_success(
                        &context_clone,
                        &task_id_clone,
                        "submission_resegment_ok",
                    );
                }
                Err(err) => {
                    let _ = mark_submission_failure(
                        &context_clone,
                        &task_id_clone,
                        Some("SEGMENTING"),
                        &format!("重新分段失败: {}", err),
                    );
                    append_log(
                        app_log_path.as_ref(),
                        &format!(
                            "submission_resegment_segment_fail task_id={} err={}",
                            task_id_clone, err
                        ),
                    );
                }
            }
        });
        return Ok(ApiResponse::success("重新分段已启动".to_string()));
    }
    let mut updated_config = build_resegment_workflow_config(
        detail.workflow_config.clone(),
        request.segment_duration_seconds,
    );
    apply_reprocess_metadata(&mut updated_config, ReprocessMode::Legacy, None);
    apply_integrate_current_bvid(&mut updated_config, integrate_current_bvid);
    let mut merged = match load_latest_merged_video(&context, &task_id) {
        Ok(Some(merged)) => merged,
        Ok(None) => return Ok(ApiResponse::error("未找到合并视频")),
        Err(err) => return Ok(ApiResponse::error(err)),
    };
    let mut merged_path = merged.video_path.clone().unwrap_or_default();
    if merged_path.trim().is_empty() || !PathBuf::from(&merged_path).exists() {
        match try_restore_merged_from_baidu(&context, &state.app_log_path, &merged, &base_dir)
            .await?
        {
            BaiduRestoreResult::Ready(restored) => {
                merged_path = restored.to_string_lossy().to_string();
                merged.video_path = Some(merged_path.clone());
            }
            BaiduRestoreResult::Queued => {
                if let Err(err) = prepare_workflow_for_baidu_restore(
                    &context,
                    &state.app_log_path,
                    &task_id,
                    &updated_config,
                    "VIDEO_RESEGMENT",
                    true,
                ) {
                    return Ok(ApiResponse::error(format!("准备网盘恢复失败: {}", err)));
                }
                return Ok(ApiResponse::success(
                    "合并视频缺失，已创建网盘下载任务，下载完成后自动重新分段".to_string(),
                ));
            }
            BaiduRestoreResult::NotBound => {}
        }
    }
    if merged_path.trim().is_empty() {
        return handle_repost_missing_assets(
            &state,
            &context,
            &detail,
            &task_id,
            &updated_config,
            false,
            FALLBACK_REASON_RESEGMENT_MISSING_MERGED,
            None,
        )
        .await;
    }
    let merged_path_buf = PathBuf::from(merged_path.clone());
    let merged_id = merged.id;
    if !merged_path_buf.exists() {
        return handle_repost_missing_assets(
            &state,
            &context,
            &detail,
            &task_id,
            &updated_config,
            false,
            FALLBACK_REASON_RESEGMENT_MISSING_MERGED,
            None,
        )
        .await;
    }
    append_log(
        &state.app_log_path,
        &format!("submission_resegment_start task_id={}", task_id),
    );
    if let Err(err) = clear_edit_upload_segments_by_task(&context, &task_id) {
        append_log(
            &state.app_log_path,
            &format!(
                "submission_resegment_clear_cache_fail task_id={} err={}",
                task_id, err
            ),
        );
    }
    if let Err(err) = reset_workflow_instances(&context, &task_id) {
        return Ok(ApiResponse::error(format!("重置工作流失败: {}", err)));
    }
    let (workflow_instance_id, _) = match create_workflow_instance_for_task_with_type(
        context.db.as_ref(),
        &task_id,
        &updated_config,
        "VIDEO_RESEGMENT",
    ) {
        Ok(result) => result,
        Err(err) => return Ok(ApiResponse::error(format!("创建工作流失败: {}", err))),
    };
    let now = now_rfc3339();
    let cleanup_result = context.db.with_conn(|conn| {
    conn.execute("DELETE FROM task_output_segment WHERE task_id = ?1", [&task_id])?;
    if integrate_current_bvid {
      conn.execute(
        "UPDATE submission_task SET status = 'SEGMENTING', remote_state = NULL, remote_state_desc = NULL, reject_reason = NULL, updated_at = ?1 WHERE task_id = ?2",
        (&now, &task_id),
      )?;
    } else {
      conn.execute(
        "UPDATE submission_task SET status = 'SEGMENTING', bvid = NULL, aid = NULL, remote_state = NULL, remote_state_desc = NULL, reject_reason = NULL, updated_at = ?1 WHERE task_id = ?2",
        (&now, &task_id),
      )?;
    }
    Ok(())
  });
    if let Err(err) = cleanup_result {
        return Ok(ApiResponse::error(format!("重置任务数据失败: {}", err)));
    }
    let base_dir = resolve_submission_base_dir(&context, &task_id);
    let output_dir = base_dir
        .join("resegment")
        .join(sanitize_filename(&format!(
            "resegment_{}",
            workflow_instance_id
        )))
        .join("output");
    if let Err(err) = remove_path_if_exists_with_retry(
        state.app_log_path.as_ref(),
        "output",
        &output_dir,
        5,
        Duration::from_millis(300),
    ) {
        return Ok(ApiResponse::error(format!(
            "清理旧分段目录失败: {}. 请稍后重试，避免重复流程并发执行",
            err
        )));
    }
    let context_clone = context.clone();
    let task_id_clone = task_id.clone();
    let merged_path_clone = merged_path_buf.clone();
    let output_dir_clone = output_dir.clone();
    let integrate_current_bvid = integrate_current_bvid;
    let app_log_path = state.app_log_path.clone();
    let segment_seconds = request.segment_duration_seconds;
    let segment_prefix = detail.task.segment_prefix.clone();
    let workflow_instance_id_clone = workflow_instance_id.clone();
    tauri::async_runtime::spawn(async move {
        let _ = update_workflow_status(
            &context_clone,
            &task_id_clone,
            "RUNNING",
            Some("SEGMENTING"),
            70.0,
        );
        let segment_outputs = match tauri::async_runtime::spawn_blocking(move || {
            segment_file(&merged_path_clone, &output_dir_clone, segment_seconds)
        })
        .await
        {
            Ok(result) => result,
            Err(_) => Err("Failed to segment video".to_string()),
        };
        let still_latest = ensure_workflow_instance_latest(
            &context_clone,
            &task_id_clone,
            &workflow_instance_id_clone,
            "RESEGMENT_LEGACY_POST_SEGMENT",
        )
        .unwrap_or(false);
        if !still_latest {
            return;
        }
        match segment_outputs {
            Ok(outputs) => {
                if outputs.is_empty() {
                    let _ = mark_submission_failure(
                        &context_clone,
                        &task_id_clone,
                        Some("SEGMENTING"),
                        "重新分段未生成任何分段文件",
                    );
                    append_log(
                        app_log_path.as_ref(),
                        &format!(
                            "submission_resegment_empty_outputs task_id={}",
                            task_id_clone
                        ),
                    );
                    return;
                }
                if let Err(err) = save_output_segments(
                    &context_clone,
                    &task_id_clone,
                    &outputs,
                    Some(merged_id),
                    segment_prefix.as_deref(),
                ) {
                    let _ = mark_submission_failure(
                        &context_clone,
                        &task_id_clone,
                        Some("SEGMENTING"),
                        &format!("重新分段保存分段失败: {}", err),
                    );
                    append_log(
                        app_log_path.as_ref(),
                        &format!(
                            "submission_resegment_save_fail task_id={} err={}",
                            task_id_clone, err
                        ),
                    );
                    return;
                }
                if !integrate_current_bvid {
                    if let Err(err) = reset_segments_for_new_bvid(&context_clone, &task_id_clone) {
                        let _ = mark_submission_failure(
                            &context_clone,
                            &task_id_clone,
                            Some("SEGMENTING"),
                            &format!("重新分段重置分段元数据失败: {}", err),
                        );
                        append_log(
                            app_log_path.as_ref(),
                            &format!(
                                "submission_resegment_reset_segments_fail task_id={} err={}",
                                task_id_clone, err
                            ),
                        );
                        return;
                    }
                }
                finalize_reprocess_success(
                    &context_clone,
                    &task_id_clone,
                    "submission_resegment_ok",
                );
            }
            Err(err) => {
                let _ = mark_submission_failure(
                    &context_clone,
                    &task_id_clone,
                    Some("SEGMENTING"),
                    &format!("重新分段失败: {}", err),
                );
                append_log(
                    app_log_path.as_ref(),
                    &format!(
                        "submission_resegment_segment_fail task_id={} err={}",
                        task_id_clone, err
                    ),
                );
            }
        }
    });
    Ok(ApiResponse::success("重新分段已启动".to_string()))
}

#[tauri::command]
pub async fn submission_edit_reupload_segment(
    state: State<'_, AppState>,
    request: SubmissionEditReuploadSegmentRequest,
) -> Result<ApiResponse<TaskOutputSegmentRecord>, String> {
    let context = SubmissionContext::new(&state);
    let task_id = request.task_id.trim().to_string();
    if task_id.is_empty() {
        return Ok(ApiResponse::error("任务ID不能为空"));
    }
    let segment_id = request.segment_id.trim().to_string();
    if segment_id.is_empty() {
        return Ok(ApiResponse::error("分P ID不能为空"));
    }
    let file_path = request.file_path.trim().to_string();
    if file_path.is_empty() {
        return Ok(ApiResponse::error("分P文件路径不能为空"));
    }
    let path = Path::new(&file_path);
    if !path.exists() {
        return Ok(ApiResponse::error("分P文件不存在"));
    }
    if let Err(err) = ensure_editable_status(&context, &task_id) {
        return Ok(ApiResponse::error(err));
    }
    let existing = match load_edit_upload_segment(&context, &segment_id) {
        Ok(Some(segment)) => Some(segment),
        Ok(None) => {
            load_output_segment_by_id(&context, &segment_id).map_err(|err| err.to_string())?
        }
        Err(err) => return Ok(ApiResponse::error(err)),
    };
    if let Some(segment) = &existing {
        if segment.task_id != task_id {
            return Ok(ApiResponse::error("分P不属于当前任务"));
        }
    }
    let part_name = default_part_name_from_path(&file_path);
    let total_bytes = fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    let mut segment = existing.unwrap_or(TaskOutputSegmentRecord {
        segment_id: segment_id.clone(),
        task_id: task_id.clone(),
        merged_id: None,
        part_name: part_name.clone(),
        segment_file_path: file_path.clone(),
        part_order: 0,
        upload_status: "UPLOADING".to_string(),
        cid: None,
        file_name: None,
        upload_progress: 0.0,
        upload_uploaded_bytes: 0,
        upload_total_bytes: total_bytes as i64,
        upload_session_id: None,
        upload_biz_id: 0,
        upload_endpoint: None,
        upload_auth: None,
        upload_uri: None,
        upload_chunk_size: 0,
        upload_last_part_index: 0,
    });
    segment.part_name = part_name;
    segment.segment_file_path = file_path;
    segment.upload_status = "UPLOADING".to_string();
    segment.cid = None;
    segment.file_name = None;
    segment.upload_progress = 0.0;
    segment.upload_uploaded_bytes = 0;
    segment.upload_total_bytes = total_bytes as i64;
    segment.upload_session_id = None;
    segment.upload_biz_id = 0;
    segment.upload_endpoint = None;
    segment.upload_auth = None;
    segment.upload_uri = None;
    segment.upload_chunk_size = 0;
    segment.upload_last_part_index = 0;
    let segment = match upsert_edit_upload_segment(&context, segment) {
        Ok(segment) => segment,
        Err(err) => return Ok(ApiResponse::error(err)),
    };
    let upload_context = UploadContext::new(&state);
    let auth =
        match load_auth_or_refresh_for_task(&upload_context, &task_id, "submission_edit_reupload")
            .await
        {
            Ok(auth) => auth,
            Err(err) => return Ok(ApiResponse::error(err)),
        };
    let context_clone = context.clone();
    let upload_context_clone = upload_context.clone();
    let segment_id_clone = segment.segment_id.clone();
    let task_id_for_upload = task_id.clone();
    tauri::async_runtime::spawn(async move {
        let client = build_submission_upload_client(upload_context_clone.app_log_path.as_ref());
        let result = upload_edit_segment_with_retry(
            &context_clone,
            &upload_context_clone,
            &client,
            &auth,
            &task_id_for_upload,
            None,
            &segment_id_clone,
            upload_context_clone.app_log_path.as_ref(),
            UPLOAD_SEGMENT_RETRY_LIMIT,
        )
        .await;
        match result {
            Ok(upload_result) => {
                let _ = update_edit_upload_segment(&context_clone, &segment_id_clone, |segment| {
                    segment.upload_status = "SUCCESS".to_string();
                    segment.cid = Some(upload_result.cid);
                    segment.file_name = Some(upload_result.filename);
                });
            }
            Err(err) => {
                let _ = update_edit_upload_segment(&context_clone, &segment_id_clone, |segment| {
                    segment.upload_status = "FAILED".to_string();
                });
                append_log(
                    upload_context_clone.app_log_path.as_ref(),
                    &format!(
                        "submission_edit_reupload_fail segment_id={} err={}",
                        segment_id_clone, err
                    ),
                );
            }
        }
    });
    Ok(ApiResponse::success(segment))
}

pub(super) fn resolve_repost_dir_name_for_path(repost_root: &Path, runtime_path: &str) -> Option<String> {
    let trimmed = runtime_path.trim();
    if trimmed.is_empty() {
        return None;
    }
    let relative = Path::new(trimmed).strip_prefix(repost_root).ok()?;
    let dir_name = relative.components().next()?.as_os_str().to_str()?.trim();
    if dir_name.starts_with("repost_") {
        Some(dir_name.to_string())
    } else {
        None
    }
}

pub(super) fn cleanup_stale_repost_dirs_for_task(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<(), String> {
    let repost_root = resolve_submission_base_dir(context, task_id).join("repost");
    let entries = match fs::read_dir(&repost_root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(format!(
                "读取重投稿目录失败: path={} err={}",
                repost_root.to_string_lossy(),
                err
            ))
        }
    };

    let repost_dirs = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_dir() {
                return None;
            }
            let dir_name = entry.file_name().to_string_lossy().to_string();
            if !dir_name.starts_with("repost_") {
                return None;
            }
            Some((dir_name, path))
        })
        .collect::<Vec<_>>();
    if repost_dirs.is_empty() {
        return Ok(());
    }

    let referenced_dirs: HashSet<String> = context
        .db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT video_path AS file_path FROM merged_video \
                 WHERE task_id = ?1 AND video_path IS NOT NULL AND TRIM(video_path) != '' \
                 UNION ALL \
                 SELECT segment_file_path AS file_path FROM task_output_segment \
                 WHERE task_id = ?2 AND segment_file_path IS NOT NULL AND TRIM(segment_file_path) != '' \
                 UNION ALL \
                 SELECT clip_path AS file_path FROM video_clip \
                 WHERE task_id = ?3 AND clip_path IS NOT NULL AND TRIM(clip_path) != ''",
            )?;
            let rows = stmt.query_map(params![task_id, task_id, task_id], |row| {
                row.get::<_, String>(0)
            })?;
            let mut dirs = HashSet::new();
            for row in rows {
                let stored_path = row?;
                let runtime_path = to_runtime_submission_path(context, &stored_path);
                if let Some(dir_name) =
                    resolve_repost_dir_name_for_path(&repost_root, runtime_path.as_str())
                {
                    dirs.insert(dir_name);
                }
            }
            Ok(dirs)
        })
        .map_err(|err| err.to_string())?;

    append_log(
        context.app_log_path.as_ref(),
        &format!(
            "submission_repost_cleanup_stale_start task_id={} total={} referenced={}",
            task_id,
            repost_dirs.len(),
            referenced_dirs.len()
        ),
    );

    let mut removed_count = 0usize;
    let mut kept_count = 0usize;
    for (dir_name, path) in repost_dirs {
        if referenced_dirs.contains(&dir_name) {
            kept_count = kept_count.saturating_add(1);
            append_log(
                context.app_log_path.as_ref(),
                &format!(
                    "submission_repost_cleanup_stale_skip task_id={} dir={}",
                    task_id,
                    path.to_string_lossy()
                ),
            );
            continue;
        }
        remove_path_if_exists_with_retry(
            &context.app_log_path,
            "repost_stale",
            &path,
            5,
            Duration::from_millis(300),
        )?;
        removed_count = removed_count.saturating_add(1);
        append_log(
            context.app_log_path.as_ref(),
            &format!(
                "submission_repost_cleanup_stale_remove task_id={} dir={}",
                task_id,
                path.to_string_lossy()
            ),
        );
    }

    append_log(
        context.app_log_path.as_ref(),
        &format!(
            "submission_repost_cleanup_stale_done task_id={} removed={} kept={}",
            task_id, removed_count, kept_count
        ),
    );
    Ok(())
}

pub(super) fn finalize_reprocess_success(context: &SubmissionContext, task_id: &str, success_event: &str) {
    let _ = update_submission_status(context, task_id, "WAITING_UPLOAD");
    let _ = update_workflow_status(context, task_id, "COMPLETED", None, 100.0);
    if let Err(err) = cleanup_stale_repost_dirs_for_task(context, task_id) {
        append_log(
            context.app_log_path.as_ref(),
            &format!(
                "submission_repost_cleanup_stale_fail task_id={} err={}",
                task_id, err
            ),
        );
    }
    append_log(
        context.app_log_path.as_ref(),
        &format!("{} task_id={}", success_event, task_id),
    );
}

#[tauri::command(async)]
pub fn submission_reupload_all_segments(
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
    if detail.task.status != "FAILED" {
        return Ok(ApiResponse::error("仅失败任务可以重新上传全部分P"));
    }
    let error_message = detail
        .task
        .workflow_status
        .as_ref()
        .and_then(|status| status.error_message.as_deref())
        .unwrap_or("");
    if !is_reupload_required_submission_error(error_message) {
        return Ok(ApiResponse::error("当前失败原因不需要重新上传全部分P"));
    }
    if detail
        .task
        .bvid
        .as_deref()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
        || detail.task.aid.unwrap_or(0) > 0
    {
        return Ok(ApiResponse::error(
            "任务已有BVID或AID，请先校验远端稿件，不能直接重新上传",
        ));
    }
    if detail.output_segments.is_empty() {
        return Ok(ApiResponse::error("当前任务没有本地分P，无法重新上传"));
    }

    let missing_files = detail
        .output_segments
        .iter()
        .filter(|segment| {
            let path = segment.segment_file_path.trim();
            path.is_empty() || !Path::new(path).is_file()
        })
        .map(|segment| segment.part_name.as_str())
        .collect::<Vec<_>>();
    if !missing_files.is_empty() {
        let examples = missing_files
            .iter()
            .take(3)
            .copied()
            .collect::<Vec<_>>()
            .join("、");
        return Ok(ApiResponse::error(format!(
            "有{}个本地分P文件不存在，无法重新上传：{}",
            missing_files.len(),
            examples
        )));
    }

    let segment_count = detail.output_segments.len();
    let now = now_rfc3339();
    context
        .db
        .with_conn_mut(|conn| {
            let tx = conn.transaction()?;
            tx.execute(RESET_ALL_SEGMENT_UPLOAD_SQL, [task_id.as_str()])?;
            tx.execute(
                "UPDATE submission_task SET status = 'WAITING_UPLOAD', bvid = NULL, aid = NULL, remote_state = NULL, remote_state_desc = NULL, reject_reason = NULL, updated_at = ?1 WHERE task_id = ?2",
                (now.as_str(), task_id.as_str()),
            )?;
            tx.execute(
                "UPDATE workflow_instances SET status = ?1, current_step = NULL, progress = 0, error_message = NULL, completed_at = NULL, updated_at = ?2 WHERE task_id = ?3",
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
            "submission_reupload_all_segments task_id={} segments={} queued=true",
            task_id, segment_count
        ),
    );
    Ok(ApiResponse::success(format!(
        "已清除旧CID，{}个本地分P已重新加入上传队列",
        segment_count
    )))
}

pub(super) fn preupload_parse_retry_delay_secs(attempt: u32) -> u64 {
    let exponent = attempt.saturating_sub(1);
    let multiplier = 1u64 << exponent.min(10);
    let wait = PREUPLOAD_PARSE_RETRY_BASE_SECS.saturating_mul(multiplier);
    wait.min(PREUPLOAD_PARSE_RETRY_MAX_SECS)
}

pub(super) fn is_preupload_parse_error(err: &str) -> bool {
    err.contains("预上传解析失败") || err.contains("error decoding response body")
}

pub(super) fn is_reupload_required_submission_error(err: &str) -> bool {
    err.contains("REUPLOAD_REQUIRED:")
}

pub(super) fn submission_retry_block_message() -> &'static str {
    "该任务的远端提交结果尚未确认。请先校验远端状态，避免重复投稿"
}

pub(super) fn build_reupload_required_error(err: &str) -> String {
    format!(
        "REUPLOAD_REQUIRED: 当前批次已被远端判定为重复投稿，不能继续复用本次已上传分P直接重试。请重新上传后投稿。原始错误: {}",
        err
    )
}

pub(super) async fn wait_preupload_throttle(
    log_path: &PathBuf,
    target: &UploadTarget,
    file_name: &str,
    owner_uid: Option<i64>,
) {
    let throttle = PREUPLOAD_THROTTLE.get_or_init(|| AsyncMutex::new(HashMap::new()));
    let owner_key = upload_owner_key(owner_uid);
    let mut last_at = throttle.lock().await;
    let now = Instant::now();
    let wait = last_at
        .get(&owner_key)
        .and_then(|prev| prev.checked_add(Duration::from_millis(PREUPLOAD_MIN_INTERVAL_MS)))
        .and_then(|next| next.checked_duration_since(now))
        .unwrap_or_default();
    if wait > Duration::ZERO {
        append_log(
            log_path,
            &format!(
                "preupload_throttle uid={} target={} wait_ms={} file={}",
                upload_owner_label(owner_uid),
                upload_target_label(target),
                wait.as_millis(),
                file_name
            ),
        );
        sleep(wait).await;
    }
    last_at.insert(owner_key, Instant::now());
}

pub(super) async fn preupload_video(
    context: &SubmissionContext,
    task_id: &str,
    target: &UploadTarget,
    client: &Client,
    auth: &AuthInfo,
    file_name: &str,
    file_size: u64,
    log_path: &PathBuf,
) -> Result<PreuploadInfo, String> {
    let url = "https://member.bilibili.com/preupload";
    let params = vec![
        ("name", file_name.to_string()),
        ("r", "upos".to_string()),
        ("profile", "ugcfx/bup".to_string()),
        ("version", "2.14.0.0".to_string()),
        ("size", file_size.to_string()),
    ];

    loop {
        wait_for_task_upload_rate_limit(log_path, task_id, target, "preupload").await;
        wait_preupload_throttle(log_path, target, file_name, auth.user_id).await;
        let headers = build_headers(Some(&auth.cookie))?;
        let response = client
            .get(url)
            .headers(headers)
            .query(&params)
            .send()
            .await
            .map_err(|err| {
                let detail = format_reqwest_error(&err);
                append_log(
                    log_path,
                    &format!(
                        "preupload_request_error target={} file={} err={}",
                        upload_target_label(target),
                        file_name,
                        detail
                    ),
                );
                format!("预上传请求失败: {}", detail)
            })?;
        let status = response.status();
        if status == StatusCode::NOT_ACCEPTABLE {
            let retry_after = retry_after_seconds(response.headers());
            wait_on_rate_limit(context, task_id, target, log_path, retry_after, "preupload").await;
            continue;
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|val| val.to_str().ok())
            .unwrap_or("-")
            .to_string();
        let body = response
            .text()
            .await
            .map_err(|err| format!("预上传读取失败: {}", err))?;
        if is_rate_limit_error(&body) || status == StatusCode::TOO_MANY_REQUESTS {
            append_log(
                log_path,
                &format!(
                    "preupload_rate_limited target={} status={} content_type={} body={}",
                    upload_target_label(target),
                    status.as_u16(),
                    content_type,
                    truncate_log_text(&body)
                ),
            );
            wait_on_rate_limit(context, task_id, target, log_path, None, "preupload").await;
            continue;
        }
        let value: Value = serde_json::from_str(&body).map_err(|err| {
            append_log(
                log_path,
                &format!(
                    "preupload_parse_fail target={} status={} content_type={} body={}",
                    upload_target_label(target),
                    status.as_u16(),
                    content_type,
                    truncate_log_text(&body)
                ),
            );
            format!("预上传解析失败: {}", err)
        })?;
        if let Some(code) = value.get("code").and_then(|val| val.as_i64()) {
            if code != 0 {
                let message = value
                    .get("message")
                    .and_then(|val| val.as_str())
                    .unwrap_or("预上传失败");
                return Err(format!("{} (code: {})", message, code));
            }
        }
        if let Some(ok) = value.get("OK").and_then(|val| val.as_i64()) {
            if ok != 1 {
                return Err("预上传失败".to_string());
            }
        }
        let remote_chunk_size = value
            .get("chunk_size")
            .and_then(|val| val.as_u64())
            .ok_or_else(|| "预上传缺少chunk_size".to_string())?;
        let chunk_size = effective_upload_chunk_size(remote_chunk_size)
            .ok_or_else(|| "预上传chunk_size无效".to_string())?;
        if chunk_size != remote_chunk_size {
            append_log(
                log_path,
                &format!(
                    "preupload_chunk_size_capped target={} remote={} effective={}",
                    upload_target_label(target),
                    remote_chunk_size,
                    chunk_size
                ),
            );
        }
        reset_task_upload_rate_limit(task_id).await;
        return Ok(PreuploadInfo {
            auth: value
                .get("auth")
                .and_then(|val| val.as_str())
                .ok_or_else(|| "预上传缺少auth".to_string())?
                .to_string(),
            biz_id: value
                .get("biz_id")
                .and_then(|val| val.as_i64())
                .ok_or_else(|| "预上传缺少biz_id".to_string())?,
            chunk_size,
            endpoint: value
                .get("endpoint")
                .and_then(|val| val.as_str())
                .ok_or_else(|| "预上传缺少endpoint".to_string())?
                .to_string(),
            upos_uri: value
                .get("upos_uri")
                .and_then(|val| val.as_str())
                .ok_or_else(|| "预上传缺少upos_uri".to_string())?
                .to_string(),
        });
    }
}

pub(super) fn recreate_selected_merged_for_repost(
    context: &SubmissionContext,
    task_id: &str,
    selected_merged_id: i64,
    source_merged_path: &Path,
    output_dir: &Path,
) -> Result<(i64, PathBuf), String> {
    let workflow_dir = output_dir
        .parent()
        .ok_or_else(|| "重新投稿输出目录无效".to_string())?;
    let target_merge_path = build_merge_output_path_for_task_id(context, workflow_dir, task_id)?;
    if let Some(parent) = target_merge_path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("创建合并目录失败: {}", err))?;
    }
    if source_merged_path != target_merge_path.as_path() {
        fs::copy(source_merged_path, &target_merge_path)
            .map_err(|err| format!("复制合并视频失败: {}", err))?;
    }
    update_merged_video_path(context, selected_merged_id, &target_merge_path)?;
    Ok((selected_merged_id, target_merge_path))
}

pub(super) fn recreate_selected_merged_for_resegment(
    context: &SubmissionContext,
    task_id: &str,
    selected_merged_id: i64,
    source_merged_path: &Path,
    output_dir: &Path,
) -> Result<(i64, PathBuf), String> {
    let workflow_dir = output_dir
        .parent()
        .ok_or_else(|| "重新分段输出目录无效".to_string())?;
    let target_merge_path = build_merge_output_path_for_task_id(context, workflow_dir, task_id)?;
    if let Some(parent) = target_merge_path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("创建合并目录失败: {}", err))?;
    }
    if source_merged_path != target_merge_path.as_path() {
        fs::copy(source_merged_path, &target_merge_path)
            .map_err(|err| format!("复制合并视频失败: {}", err))?;
    }
    let new_merged_id = recreate_selected_merged_video_record(
        context,
        task_id,
        selected_merged_id,
        &target_merge_path,
    )?;
    Ok((new_merged_id, target_merge_path))
}

pub(super) fn restore_upload_status_after_rate_limit(
    context: &SubmissionContext,
    target: &UploadTarget,
) -> Result<(), String> {
    match target {
        UploadTarget::Segment(segment_id) => {
            let segment = load_output_segment_by_id(context, segment_id)?;
            if let Some(segment) = segment {
                if segment.upload_status == "RATE_LIMITED" {
                    return update_segment_upload_status(context, segment_id, "UPLOADING");
                }
            }
            Ok(())
        }
        UploadTarget::Merged(_) => Ok(()),
        UploadTarget::EditSegment(segment_id) => {
            let segment = load_edit_upload_segment(context, segment_id)?;
            if let Some(segment) = segment {
                if segment.upload_status == "RATE_LIMITED" {
                    return update_edit_upload_segment(context, segment_id, |segment| {
                        segment.upload_status = "UPLOADING".to_string();
                    });
                }
            }
            Ok(())
        }
    }
}

pub(super) fn build_resegment_workflow_config(config: Option<Value>, segment_duration_seconds: i64) -> Value {
    let mut config = match config {
        Some(Value::Object(map)) => Value::Object(map),
        Some(_) => Value::Object(Map::new()),
        None => Value::Object(Map::new()),
    };
    if !config.is_object() {
        config = Value::Object(Map::new());
    }
    if let Some(config_map) = config.as_object_mut() {
        config_map.insert("enableSegmentation".to_string(), Value::Bool(true));
        let segmentation = config_map
            .entry("segmentationConfig".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !segmentation.is_object() {
            *segmentation = Value::Object(Map::new());
        }
        if let Some(seg_map) = segmentation.as_object_mut() {
            seg_map.insert("enabled".to_string(), Value::Bool(true));
            seg_map.insert(
                "segmentDurationSeconds".to_string(),
                Value::Number(Number::from(segment_duration_seconds.max(1))),
            );
        }
    }
    config
}
