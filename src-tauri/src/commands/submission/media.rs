//! 剪辑、合并、分段与转码等媒体处理。

use super::*;

pub(super) fn media_process_mode_to_str(mode: MediaProcessMode) -> &'static str {
    match mode {
        MediaProcessMode::CopyOnly => "COPY_ONLY",
        MediaProcessMode::ClipAndMergeTranscode => "CLIP_AND_MERGE_TRANSCODE",
    }
}

pub(super) fn parse_media_process_mode(mode: Option<&str>) -> MediaProcessMode {
    let normalized = mode.unwrap_or("").trim().to_ascii_uppercase();
    match normalized.as_str() {
        "CLIP_AND_MERGE_TRANSCODE" => MediaProcessMode::ClipAndMergeTranscode,
        _ => MediaProcessMode::CopyOnly,
    }
}

pub(super) fn apply_media_process_mode(config: &mut Value, mode: MediaProcessMode) {
    if !config.is_object() {
        *config = Value::Object(Map::new());
    }
    let Some(map) = config.as_object_mut() else {
        return;
    };
    map.insert(
        "mediaProcessMode".to_string(),
        Value::String(media_process_mode_to_str(mode).to_string()),
    );
}

pub(super) fn load_media_process_mode(config: Option<&Value>) -> MediaProcessMode {
    let Some(config) = config else {
        return MediaProcessMode::CopyOnly;
    };
    config
        .get("mediaProcessMode")
        .and_then(|value| value.as_str())
        .map(|value| parse_media_process_mode(Some(value)))
        .unwrap_or(MediaProcessMode::CopyOnly)
}

pub(super) fn clip_source_match_key(source: &ClipSource) -> (i64, String, String) {
    (
        source.order,
        source
            .start_time
            .as_deref()
            .unwrap_or("")
            .trim()
            .to_string(),
        source.end_time.as_deref().unwrap_or("").trim().to_string(),
    )
}

pub(super) fn extract_merge_groups_from_config(
    context: &SubmissionContext,
    task_id: &str,
    config: Option<&Value>,
) -> Vec<MergeGroupPlan> {
    let Some(config) = config else {
        return Vec::new();
    };
    let Some(list) = config
        .get("mergeGroups")
        .or_else(|| config.get("merge_groups"))
        .and_then(|value| value.as_array())
    else {
        return Vec::new();
    };
    let mut groups = Vec::new();
    for (group_index, group) in list.iter().enumerate() {
        let Some(source_list) = group
            .get("sources")
            .or_else(|| group.get("sourceVideos"))
            .or_else(|| group.get("source_videos"))
            .and_then(|value| value.as_array())
        else {
            continue;
        };
        let mut sources = Vec::new();
        for (index, item) in source_list.iter().enumerate() {
            let raw_path = item
                .get("sourceFilePath")
                .or_else(|| item.get("source_file_path"))
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let remote_path = item
                .get("remotePath")
                .or_else(|| item.get("remote_path"))
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let raw_path = if raw_path.is_empty() && !remote_path.is_empty() {
                resolve_baidu_source_path_for_task(context, task_id, &remote_path)
                    .to_string_lossy()
                    .to_string()
            } else {
                raw_path
            };
            if raw_path.is_empty() {
                continue;
            }
            let input_path = to_runtime_submission_path(context, &raw_path);
            if input_path.trim().is_empty() {
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
            continue;
        }
        sources.sort_by_key(|item| item.order);
        let sources = normalize_binding_sources(sources);
        let order = group
            .get("order")
            .or_else(|| group.get("sortOrder"))
            .or_else(|| group.get("sort_order"))
            .and_then(|value| value.as_i64())
            .unwrap_or((group_index + 1) as i64);
        let upload_part_title = extract_upload_part_title_from_json(group);
        groups.push(MergeGroupPlan {
            order,
            sources,
            upload_part_title,
        });
    }
    groups.sort_by_key(|item| item.order);
    for (index, group) in groups.iter_mut().enumerate() {
        group.order = (index + 1) as i64;
    }
    groups
}

pub(super) fn build_merge_groups_config_value(groups: &[MergeGroupPlan]) -> Value {
    Value::Array(
        groups
            .iter()
            .enumerate()
            .map(|(group_index, group)| {
                let sources = group
                    .sources
                    .iter()
                    .enumerate()
                    .map(|(source_index, source)| {
                        let mut source_map = Map::new();
                        source_map.insert(
                            "sourceFilePath".to_string(),
                            Value::String(source.input_path.trim().to_string()),
                        );
                        if let Some(start) = source
                            .start_time
                            .as_deref()
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                        {
                            source_map
                                .insert("startTime".to_string(), Value::String(start.to_string()));
                        }
                        if let Some(end) = source
                            .end_time
                            .as_deref()
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                        {
                            source_map
                                .insert("endTime".to_string(), Value::String(end.to_string()));
                        }
                        source_map.insert(
                            "sortOrder".to_string(),
                            Value::Number(Number::from(
                                source.order.max(1).max(source_index as i64 + 1),
                            )),
                        );
                        Value::Object(source_map)
                    })
                    .collect::<Vec<_>>();
                let mut group_map = Map::new();
                group_map.insert(
                    "order".to_string(),
                    Value::Number(Number::from(group.order.max(1).max(group_index as i64 + 1))),
                );
                group_map.insert("sources".to_string(), Value::Array(sources));
                Value::Object(group_map)
            })
            .collect(),
    )
}

pub(super) fn save_latest_workflow_merge_groups(
    context: &SubmissionContext,
    task_id: &str,
    groups: &[MergeGroupPlan],
) -> Result<bool, String> {
    let Some((config_id, mut config)) = load_latest_workflow_config_record(context, task_id)?
    else {
        return Ok(false);
    };
    let merge_groups_value = build_merge_groups_config_value(groups);
    let changed = match &mut config {
        Value::Object(map) => {
            if map.get("mergeGroups") == Some(&merge_groups_value) {
                false
            } else {
                map.insert("mergeGroups".to_string(), merge_groups_value);
                true
            }
        }
        _ => {
            let mut map = Map::new();
            map.insert("mergeGroups".to_string(), merge_groups_value);
            config = Value::Object(map);
            true
        }
    };
    if !changed {
        return Ok(false);
    }
    let config_raw =
        serde_json::to_string(&config).map_err(|err| format!("序列化工作流配置失败: {}", err))?;
    let now = now_rfc3339();
    context
    .db
    .with_conn(|conn| {
      conn.execute(
        "UPDATE workflow_configurations SET configuration_data = ?1, updated_at = ?2 WHERE config_id = ?3",
        (&config_raw, &now, config_id),
      )?;
      Ok(())
    })
    .map_err(|err| err.to_string())?;
    append_log(
        &context.app_log_path,
        &format!(
            "submission_merge_groups_config_synced task_id={} config_id={} groups={}",
            task_id,
            config_id,
            groups.len()
        ),
    );
    Ok(true)
}

pub(super) fn repair_merge_groups_from_fallback(
    context: &SubmissionContext,
    task_id: &str,
    groups: &mut [MergeGroupPlan],
    fallback_sources: &[ClipSource],
) {
    if groups.is_empty() || fallback_sources.is_empty() {
        return;
    }
    let fallback_by_key = fallback_sources
        .iter()
        .filter(|source| !source.input_path.trim().is_empty())
        .map(|source| (clip_source_match_key(source), source.clone()))
        .collect::<HashMap<_, _>>();
    let mut fallback_by_order = HashMap::new();
    for source in fallback_sources
        .iter()
        .filter(|source| !source.input_path.trim().is_empty())
    {
        fallback_by_order
            .entry(source.order)
            .or_insert_with(Vec::new)
            .push(source.clone());
    }

    let mut changed = false;
    for group in groups.iter_mut() {
        for source in &mut group.sources {
            if path_exists(Path::new(&source.input_path)) {
                continue;
            }
            let key = clip_source_match_key(source);
            let replacement = fallback_by_key.get(&key).cloned().or_else(|| {
                fallback_by_order
                    .get(&source.order)
                    .filter(|items| items.len() == 1)
                    .and_then(|items| items.first().cloned())
            });
            let Some(replacement) = replacement else {
                continue;
            };
            if replacement.input_path != source.input_path {
                append_log(
                    &context.app_log_path,
                    &format!(
                        "submission_merge_group_source_repaired task_id={} order={} from={} to={}",
                        task_id, source.order, source.input_path, replacement.input_path
                    ),
                );
                source.input_path = replacement.input_path;
                if source.start_time.as_deref().unwrap_or("").trim().is_empty() {
                    source.start_time = replacement.start_time;
                }
                if source.end_time.as_deref().unwrap_or("").trim().is_empty() {
                    source.end_time = replacement.end_time;
                }
                changed = true;
            }
        }
        group.sources = normalize_binding_sources(group.sources.clone());
    }

    if changed {
        if let Err(err) = save_latest_workflow_merge_groups(context, task_id, groups) {
            append_log(
                &context.app_log_path,
                &format!(
                    "submission_merge_groups_config_sync_fail task_id={} err={}",
                    task_id, err
                ),
            );
        }
    }
}

pub(super) fn resolve_merge_groups(
    context: &SubmissionContext,
    task_id: &str,
    config: Option<&Value>,
    fallback_sources: &[ClipSource],
) -> Vec<MergeGroupPlan> {
    let mut groups = extract_merge_groups_from_config(context, task_id, config);
    repair_merge_groups_from_fallback(context, task_id, &mut groups, fallback_sources);
    let has_config_groups = !groups.is_empty();
    if has_config_groups {
        // 有合并组配置：未被任何组覆盖的源视频各自独立成一组
        let mut used = HashSet::new();
        for group in &groups {
            for source in &group.sources {
                let key = format!(
                    "{}|{}|{}",
                    source.input_path,
                    source.start_time.as_deref().unwrap_or(""),
                    source.end_time.as_deref().unwrap_or("")
                );
                used.insert(key);
            }
        }
        for source in fallback_sources {
            let key = format!(
                "{}|{}|{}",
                source.input_path,
                source.start_time.as_deref().unwrap_or(""),
                source.end_time.as_deref().unwrap_or("")
            );
            if used.contains(&key) {
                continue;
            }
            groups.push(MergeGroupPlan {
                order: groups.len() as i64 + 1,
                sources: normalize_binding_sources(vec![source.clone()]),
                upload_part_title: None,
            });
        }
    } else if !fallback_sources.is_empty() {
        // 无合并组配置：所有源视频合并为一个合并视频
        groups.push(MergeGroupPlan {
            order: 1,
            sources: normalize_binding_sources(fallback_sources.to_vec()),
            upload_part_title: None,
        });
    }
    // 独立分组（单源组）的自定义上传分P标题从源视频读取（合并组标题已在配置中携带）。
    let source_titles = load_source_upload_titles(context, task_id);
    if !source_titles.is_empty() {
        for group in &mut groups {
            if group.upload_part_title.is_some() || group.sources.len() != 1 {
                continue;
            }
            if let Some(title) = group
                .sources
                .first()
                .and_then(|source| source_titles.get(&source.input_path))
            {
                group.upload_part_title = Some(title.clone());
            }
        }
    }
    groups
}

pub(super) fn parse_ffprobe_time(value: Option<&Value>) -> Option<f64> {
    value
        .and_then(|item| item.as_str())
        .and_then(|raw| raw.parse::<f64>().ok())
}

pub(super) fn detect_merged_video_decode_issue(stderr: &str) -> Option<&'static str> {
    let normalized = stderr.to_ascii_lowercase();
    let patterns = [
        (
            "missing picture in access unit",
            "missing_picture_in_access_unit",
        ),
        ("non-existing pps", "non_existing_pps"),
        ("non-existing sps", "non_existing_sps"),
        ("sps_id", "invalid_sps_id"),
        ("pps_id", "invalid_pps_id"),
        ("illegal poc type", "illegal_poc_type"),
        (
            "illegal memory management control operation",
            "illegal_memory_management_control",
        ),
        (
            "a non-intra slice in an idr nal unit",
            "non_intra_slice_in_idr",
        ),
        ("reference count overflow", "reference_count_overflow"),
        ("reference count 1 overflow", "reference_count_overflow"),
        ("cabac_init_idc", "invalid_cabac_init_idc"),
        ("slice type", "invalid_slice_type"),
        ("truncated at", "truncated_nal_or_sei"),
        ("invalid nal unit size", "invalid_nal_unit_size"),
        ("error splitting the input into nal units", "nal_unit_split_fail"),
    ];
    patterns
        .into_iter()
        .find_map(|(needle, reason)| normalized.contains(needle).then_some(reason))
}

pub(super) fn merged_video_decode_probe_issue(
    path: &Path,
    start_seconds: f64,
    span_seconds: f64,
) -> Result<Option<String>, String> {
    let interval = format!(
        "{:.3}%+{:.3}",
        start_seconds.max(0.0),
        span_seconds.max(0.5)
    );
    let args = vec![
        "-v".to_string(),
        "warning".to_string(),
        "-read_intervals".to_string(),
        interval,
        "-select_streams".to_string(),
        "v:0".to_string(),
        "-show_frames".to_string(),
        "-show_entries".to_string(),
        "frame=best_effort_timestamp_time,key_frame,pict_type".to_string(),
        "-of".to_string(),
        "json".to_string(),
        path.to_string_lossy().to_string(),
    ];
    let (_, stderr) = run_ffprobe_capture(&args)?;
    Ok(detect_merged_video_decode_issue(&stderr).map(|reason| reason.to_string()))
}

pub(super) fn merged_probe_has_packets(
    path: &Path,
    stream_selector: &str,
    start_seconds: f64,
) -> Result<bool, String> {
    let interval = format!("{:.3}%+0.8", start_seconds.max(0.0));
    let args = vec![
        "-v".to_string(),
        "error".to_string(),
        "-read_intervals".to_string(),
        interval,
        "-select_streams".to_string(),
        stream_selector.to_string(),
        "-show_packets".to_string(),
        "-show_entries".to_string(),
        "packet=pts_time".to_string(),
        "-of".to_string(),
        "json".to_string(),
        path.to_string_lossy().to_string(),
    ];
    let data = run_ffprobe_json(&args)?;
    Ok(data
        .get("packets")
        .and_then(|value| value.as_array())
        .map(|items| !items.is_empty())
        .unwrap_or(false))
}

pub(super) fn merged_audio_packet_gap_delta(
    path: &Path,
    start_seconds: f64,
    span_seconds: f64,
) -> Result<f64, String> {
    let interval = format!(
        "{:.3}%+{:.3}",
        start_seconds.max(0.0),
        span_seconds.max(0.1)
    );
    let args = vec![
        "-v".to_string(),
        "error".to_string(),
        "-read_intervals".to_string(),
        interval,
        "-select_streams".to_string(),
        "a:0".to_string(),
        "-show_packets".to_string(),
        "-show_entries".to_string(),
        "packet=pts_time,duration_time".to_string(),
        "-of".to_string(),
        "csv=p=0".to_string(),
        path.to_string_lossy().to_string(),
    ];
    let (stdout, _) = run_ffprobe_capture(&args)?;
    let mut prev_pts: Option<f64> = None;
    let mut max_delta = 0.0f64;
    let mut packet_count = 0usize;

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut parts = trimmed
            .split(',')
            .map(|item| item.trim())
            .filter(|item| !item.is_empty());
        let Some(pts_raw) = parts.next() else {
            continue;
        };
        let Some(duration_raw) = parts.next() else {
            continue;
        };
        let Some(pts_time) = pts_raw.parse::<f64>().ok() else {
            continue;
        };
        let Some(duration_time) = duration_raw.parse::<f64>().ok() else {
            continue;
        };
        if let Some(previous) = prev_pts {
            let gap_delta = ((pts_time - previous) - duration_time).abs();
            if gap_delta > max_delta {
                max_delta = gap_delta;
            }
        }
        prev_pts = Some(pts_time);
        packet_count += 1;
    }

    if packet_count < 2 {
        return Err(format!(
            "audio_packet_probe_insufficient start={:.3} span={:.3} packets={}",
            start_seconds, span_seconds, packet_count
        ));
    }

    Ok(max_delta)
}

pub(super) fn load_merged_baidu_uid(
    context: &SubmissionContext,
    merged_id: i64,
) -> Result<Option<String>, String> {
    context
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT baidu_uid FROM merged_video WHERE id = ?1",
                [merged_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map(|value| value.flatten())
        })
        .map_err(|err| err.to_string())
}

pub(super) fn update_merged_video_after_baidu_download(
    context: &SubmissionContext,
    merged_id: i64,
    local_path: &Path,
    remote_dir: &str,
    remote_name: &str,
    baidu_uid: Option<&str>,
) -> Result<(), String> {
    let now = now_rfc3339();
    let stored_path = to_stored_submission_path(context, local_path.to_string_lossy().as_ref());
    let file_name = local_path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.to_string())
        .unwrap_or_else(|| remote_name.trim().to_string());
    context
    .db
    .with_conn(|conn| {
      conn.execute(
        "UPDATE merged_video SET file_name = ?1, video_path = ?2, remote_dir = ?3, remote_name = ?4, baidu_uid = ?5, update_time = ?6 WHERE id = ?7",
        (
          file_name,
          stored_path,
          remote_dir,
          remote_name,
          baidu_uid,
          &now,
          merged_id,
        ),
      )?;
      Ok(())
    })
    .map_err(|err| err.to_string())
}

#[tauri::command(async)]
pub fn submission_batch_bind_segments_merged(
    state: State<'_, AppState>,
    request: SubmissionBatchBindSegmentsMergedRequest,
) -> ApiResponse<String> {
    let context = SubmissionContext::new(&state);
    let task_id = request.task_id.trim().to_string();
    if task_id.is_empty() {
        return ApiResponse::error("任务ID不能为空");
    }
    if request.merged_id <= 0 {
        return ApiResponse::error("合并视频ID无效");
    }
    let mut seen_segment_ids = HashSet::new();
    let segment_ids = request
        .segment_ids
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .filter(|value| seen_segment_ids.insert((*value).to_string()))
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    if segment_ids.is_empty() {
        return ApiResponse::error("请至少选择一个分段");
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

    let segment_count_check = context.db.with_conn(|conn| {
        let placeholders = (0..segment_ids.len())
            .map(|index| format!("?{}", index + 2))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT COUNT(*) FROM task_output_segment WHERE task_id = ?1 AND segment_id IN ({})",
            placeholders
        );
        let mut values = Vec::with_capacity(segment_ids.len() + 1);
        values.push(SqlValue::Text(task_id.clone()));
        for segment_id in &segment_ids {
            values.push(SqlValue::Text(segment_id.to_string()));
        }
        let count: i64 = conn.query_row(sql.as_str(), params_from_iter(values.iter()), |row| {
            row.get(0)
        })?;
        Ok(count)
    });
    let segment_count = match segment_count_check {
        Ok(value) => value,
        Err(err) => return ApiResponse::error(format!("校验分段失败: {}", err)),
    };
    if segment_count != segment_ids.len() as i64 {
        return ApiResponse::error("存在不属于当前任务的分段，无法绑定");
    }

    let now = now_rfc3339();
    let update_result = context.db.with_conn_mut(|conn| {
        let tx = conn.transaction()?;
        let placeholders = (0..segment_ids.len())
            .map(|index| format!("?{}", index + 3))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
      "UPDATE task_output_segment SET merged_id = ?1 WHERE task_id = ?2 AND segment_id IN ({})",
      placeholders
    );
        let mut values = Vec::with_capacity(segment_ids.len() + 2);
        values.push(SqlValue::Integer(request.merged_id));
        values.push(SqlValue::Text(task_id.clone()));
        for segment_id in &segment_ids {
            values.push(SqlValue::Text(segment_id.to_string()));
        }
        let updated = tx.execute(sql.as_str(), params_from_iter(values.iter()))?;
        tx.execute(
            "UPDATE submission_task SET updated_at = ?1 WHERE task_id = ?2",
            (&now, &task_id),
        )?;
        tx.commit()?;
        Ok(updated)
    });
    let updated = match update_result {
        Ok(value) => value,
        Err(err) => return ApiResponse::error(format!("批量绑定分段失败: {}", err)),
    };

    ApiResponse::success(format!("已更新{}个分段绑定", updated))
}

#[tauri::command(async)]
pub fn submission_reorder_merged_videos(
    state: State<'_, AppState>,
    request: SubmissionReorderMergedVideosRequest,
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
    let merged_ids = request
        .merged_ids
        .iter()
        .copied()
        .filter(|value| *value > 0)
        .filter(|value| seen_ids.insert(*value))
        .collect::<Vec<_>>();
    if merged_ids.is_empty() {
        return ApiResponse::error("请至少保留一个合并视频");
    }
    let existing_ids = context.db.with_conn(|conn| {
        let mut stmt = conn.prepare("SELECT id FROM merged_video WHERE task_id = ?1")?;
        let rows = stmt.query_map([&task_id], |row| row.get::<_, i64>(0))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    });
    let existing_ids = match existing_ids {
        Ok(value) => value,
        Err(err) => return ApiResponse::error(format!("读取合并视频失败: {}", err)),
    };
    if existing_ids.len() != merged_ids.len() {
        return ApiResponse::error("排序列表与合并视频数量不一致");
    }
    let existing_set = existing_ids.into_iter().collect::<HashSet<_>>();
    let incoming_set = merged_ids.iter().copied().collect::<HashSet<_>>();
    if existing_set != incoming_set {
        return ApiResponse::error("排序列表存在不属于当前任务的合并视频");
    }

    let now = now_rfc3339();
    let result = context.db.with_conn_mut(|conn| {
        let tx = conn.transaction()?;
        for (index, merged_id) in merged_ids.iter().enumerate() {
            tx.execute(
        "UPDATE merged_video SET sort_order = ?1, update_time = ?2 WHERE task_id = ?3 AND id = ?4",
        ((index + 1) as i64, &now, &task_id, merged_id),
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
        return ApiResponse::error(format!("更新合并视频排序失败: {}", err));
    }
    ApiResponse::success("合并视频排序已更新".to_string())
}

#[tauri::command(async)]
pub fn submission_bind_merged_sources(
    state: State<'_, AppState>,
    request: SubmissionBindMergedSourcesRequest,
) -> ApiResponse<String> {
    let context = SubmissionContext::new(&state);
    let task_id = request.task_id.trim().to_string();
    if task_id.is_empty() {
        return ApiResponse::error("任务ID不能为空");
    }
    if request.merged_id <= 0 {
        return ApiResponse::error("合并视频ID无效");
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

    let mut seen_source_ids = HashSet::new();
    let source_ids = request
        .source_ids
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .filter(|value| seen_source_ids.insert((*value).to_string()))
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    if source_ids.is_empty() {
        return ApiResponse::error("请至少选择一个源视频");
    }

    let source_records = match load_task_source_video_records(&context, &task_id) {
        Ok(value) => value,
        Err(err) => return ApiResponse::error(format!("读取源视频失败: {}", err)),
    };
    let source_record_map = source_records
        .into_iter()
        .map(|item| (item.id.clone(), item))
        .collect::<HashMap<_, _>>();
    let mut bound_sources = Vec::new();
    for (index, source_id) in source_ids.iter().enumerate() {
        let Some(source_record) = source_record_map.get(source_id) else {
            return ApiResponse::error(format!("源视频不存在: {}", source_id));
        };
        bound_sources.push(ClipSource {
            input_path: source_record.source_file_path.clone(),
            start_time: source_record.start_time.clone(),
            end_time: source_record.end_time.clone(),
            order: (index + 1) as i64,
        });
    }
    if let Err(err) =
        save_merged_source_bindings(&context, &task_id, request.merged_id, &bound_sources)
    {
        return ApiResponse::error(format!("绑定源视频失败: {}", err));
    }

    let now = now_rfc3339();
    let _ = context.db.with_conn(|conn| {
        conn.execute(
            "UPDATE merged_video SET update_time = ?1 WHERE id = ?2 AND task_id = ?3",
            (&now, request.merged_id, &task_id),
        )?;
        conn.execute(
            "UPDATE submission_task SET updated_at = ?1 WHERE task_id = ?2",
            (&now, &task_id),
        )?;
        Ok(())
    });
    ApiResponse::success(format!("已更新{}条源视频绑定", bound_sources.len()))
}

#[tauri::command(async)]
pub fn submission_delete_merged_video(
    state: State<'_, AppState>,
    request: SubmissionDeleteMergedRequest,
) -> ApiResponse<SubmissionDeleteMergedResult> {
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
        return ApiResponse::error("当前账号无权删除该合并视频");
    }

    let merged_record = context.db.with_conn(|conn| {
    conn
      .query_row(
        "SELECT video_path, remote_dir, remote_name FROM merged_video WHERE id = ?1 AND task_id = ?2",
        (request.merged_id, &task_id),
        |row| {
          Ok((
            row.get::<_, Option<String>>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
          ))
        },
      )
      .optional()
  });
    let (stored_video_path, remote_dir, remote_name) = match merged_record {
        Ok(Some(value)) => value,
        Ok(None) => return ApiResponse::error("合并视频不存在"),
        Err(err) => return ApiResponse::error(format!("读取合并视频失败: {}", err)),
    };

    let base_dir = resolve_submission_base_dir(&context, &task_id);
    let runtime_video_path = stored_video_path
        .as_deref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| to_runtime_submission_path(&context, value));
    let mut archived_local_path = None;
    if let Some(video_path) = runtime_video_path.as_deref() {
        let local_path = Path::new(video_path);
        if path_exists(local_path) {
            if request.delete_local_file {
                if let Err(err) = remove_path_if_exists_with_retry(
                    &state.app_log_path,
                    "merged_local",
                    local_path,
                    5,
                    Duration::from_millis(300),
                ) {
                    return ApiResponse::error(err);
                }
            } else if should_archive_deleted_merged_path(&base_dir, local_path) {
                match archive_merged_local_file(&state.app_log_path, &base_dir, local_path) {
                    Ok(path) => archived_local_path = Some(path),
                    Err(err) => return ApiResponse::error(err),
                }
            }
        }
    }

    let now = now_rfc3339();
    let delete_result = context.db.with_conn_mut(|conn| {
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM task_output_segment WHERE task_id = ?1 AND merged_id = ?2",
            (&task_id, request.merged_id),
        )?;
        tx.execute(
            "DELETE FROM merged_source_video WHERE task_id = ?1 AND merged_id = ?2",
            (&task_id, request.merged_id),
        )?;

        let remote_dir = remote_dir
            .as_deref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty());
        let remote_name = remote_name
            .as_deref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty());
        if let (Some(remote_dir), Some(remote_name)) = (remote_dir, remote_name) {
            tx.execute(
                "DELETE FROM baidu_sync_task \
         WHERE source_type = 'submission_merged' AND source_id = ?1 \
           AND remote_dir = ?2 AND remote_name = ?3 \
           AND status IN ('PENDING', 'FAILED', 'PAUSED', 'CANCELLED')",
                (&task_id, remote_dir, remote_name),
            )?;
        }
        if let Some(stored_path) = stored_video_path
            .as_deref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        {
            tx.execute(
                "DELETE FROM baidu_sync_task \
         WHERE source_type = 'submission_merged' AND source_id = ?1 \
           AND local_path = ?2 \
           AND status IN ('PENDING', 'FAILED', 'PAUSED', 'CANCELLED')",
                (&task_id, stored_path),
            )?;
        }

        let deleted = tx.execute(
            "DELETE FROM merged_video WHERE id = ?1 AND task_id = ?2",
            (request.merged_id, &task_id),
        )?;
        if deleted == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        tx.execute(
            "UPDATE submission_task SET updated_at = ?1 WHERE task_id = ?2",
            (&now, &task_id),
        )?;
        tx.commit()?;
        Ok(())
    });
    if let Err(err) = delete_result {
        return ApiResponse::error(format!("删除合并视频失败: {}", err));
    }

    ApiResponse::success(SubmissionDeleteMergedResult {
        deleted: true,
        deleted_local_file: request.delete_local_file,
        archived_local_path,
    })
}

#[tauri::command]
pub async fn submission_edit_add_segment(
    state: State<'_, AppState>,
    request: SubmissionEditAddSegmentRequest,
) -> Result<ApiResponse<TaskOutputSegmentRecord>, String> {
    let context = SubmissionContext::new(&state);
    let task_id = request.task_id.trim().to_string();
    if task_id.is_empty() {
        return Ok(ApiResponse::error("任务ID不能为空"));
    }
    let file_path = request.file_path.trim().to_string();
    if file_path.is_empty() {
        return Ok(ApiResponse::error("分P文件路径不能为空"));
    }
    append_log(
        &state.app_log_path,
        &format!(
            "submission_edit_add_segment_start task_id={} file_path={}",
            task_id, file_path
        ),
    );
    let path = Path::new(&file_path);
    if !path.exists() {
        append_log(
            &state.app_log_path,
            &format!(
                "submission_edit_add_segment_fail task_id={} reason=file_missing",
                task_id
            ),
        );
        return Ok(ApiResponse::error("分P文件不存在"));
    }
    if let Err(err) = ensure_editable_status(&context, &task_id) {
        append_log(
            &state.app_log_path,
            &format!(
                "submission_edit_add_segment_fail task_id={} reason={}",
                task_id, err
            ),
        );
        return Ok(ApiResponse::error(err));
    }
    let part_name = request
        .part_name
        .as_deref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
        .unwrap_or_else(|| default_part_name_from_path(&file_path));
    let total_bytes = fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    let segment_id = uuid::Uuid::new_v4().to_string();
    let segment = TaskOutputSegmentRecord {
        segment_id: segment_id.clone(),
        task_id: task_id.clone(),
        merged_id: None,
        part_name,
        segment_file_path: file_path,
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
    };
    let segment = match upsert_edit_upload_segment(&context, segment) {
        Ok(segment) => segment,
        Err(err) => {
            append_log(
                &state.app_log_path,
                &format!(
                    "submission_edit_add_segment_fail task_id={} reason=state err={}",
                    task_id, err
                ),
            );
            return Ok(ApiResponse::error(err));
        }
    };
    append_log(
        &state.app_log_path,
        &format!(
            "submission_edit_add_segment_cached task_id={} segment_id={}",
            task_id, segment.segment_id
        ),
    );
    let upload_context = UploadContext::new(&state);
    let auth = match load_auth_or_refresh_for_task(
        &upload_context,
        &task_id,
        "submission_edit_add_segment",
    )
    .await
    {
        Ok(auth) => auth,
        Err(err) => return Ok(ApiResponse::error(err)),
    };
    let context_clone = context.clone();
    let upload_context_clone = upload_context.clone();
    let segment_id_clone = segment.segment_id.clone();
    let task_id_for_upload = task_id.clone();
    append_log(
        &state.app_log_path,
        &format!(
            "submission_edit_add_segment_upload_queued task_id={} segment_id={}",
            task_id, segment_id
        ),
    );
    tauri::async_runtime::spawn(async move {
        append_log(
            upload_context_clone.app_log_path.as_ref(),
            &format!(
                "submission_edit_add_segment_upload_start segment_id={}",
                segment_id_clone
            ),
        );
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
                append_log(
                    upload_context_clone.app_log_path.as_ref(),
                    &format!(
                        "submission_edit_add_segment_upload_ok segment_id={}",
                        segment_id_clone
                    ),
                );
            }
            Err(err) => {
                let _ = update_edit_upload_segment(&context_clone, &segment_id_clone, |segment| {
                    segment.upload_status = "FAILED".to_string();
                });
                append_log(
                    upload_context_clone.app_log_path.as_ref(),
                    &format!(
                        "submission_edit_add_segment_fail segment_id={} err={}",
                        segment_id_clone, err
                    ),
                );
            }
        }
    });
    append_log(
        &state.app_log_path,
        &format!(
            "submission_edit_add_segment_return task_id={} segment_id={} status={}",
            task_id, segment.segment_id, segment.upload_status
        ),
    );
    Ok(ApiResponse::success(segment))
}

pub(super) fn duration_to_wait_secs(wait: Duration) -> u64 {
    let secs = wait.as_secs();
    if wait.subsec_nanos() > 0 {
        secs.saturating_add(1)
    } else {
        secs
    }
}

pub(super) fn is_non_segmented_task(task: &SubmissionTaskRecord) -> bool {
    task.import_mode == IMPORT_MODE_NON_SEGMENTED && !task.enable_segmentation
}

pub(super) fn should_validate_non_segmented_consistency(
    detail: &SubmissionTaskDetail,
    is_update_workflow: bool,
) -> bool {
    is_non_segmented_task(&detail.task)
        && (is_update_workflow || !detail.output_segments.is_empty())
}

pub(super) fn validate_non_segmented_submission_consistency(
    detail: &SubmissionTaskDetail,
    parts: &[UploadedVideoPart],
) -> Result<(), String> {
    let merged_count = detail.merged_videos.len();
    let segment_count = detail.output_segments.len();
    let part_count = parts.len();
    if segment_count == 0 {
        return Err("非分段任务缺少本地分P记录，请重新执行视频处理".to_string());
    }
    if merged_count != segment_count {
        return Err(format!(
            "非分段任务本地合并视频数与分P记录数不一致（合并={}，分P={}），请先修复任务数据",
            merged_count, segment_count
        ));
    }
    if part_count != segment_count {
        return Err(format!(
            "非分段任务待提交分P数与本地分P记录数不一致（提交={}，分P={}），请重新上传分P",
            part_count, segment_count
        ));
    }
    Ok(())
}

pub(super) fn build_segment_file_name(prefix: &str, index: usize) -> String {
    format!("{}_part_{}.mp4", prefix, index)
}

pub(super) fn rename_segment_outputs_with_prefix(
    segments: &[PathBuf],
    prefix: Option<&str>,
    start_index: usize,
) -> Result<Vec<PathBuf>, String> {
    let prefix = prefix.unwrap_or("").trim();
    if prefix.is_empty() {
        return Ok(segments.to_vec());
    }
    let sanitized_prefix = sanitize_filename(prefix);
    let base_index = if start_index == 0 { 1 } else { start_index };
    let mut renamed = Vec::with_capacity(segments.len());
    for (index, segment) in segments.iter().enumerate() {
        let file_name = segment
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if !file_name.starts_with("part_") {
            renamed.push(segment.clone());
            continue;
        }
        let dir = segment
            .parent()
            .ok_or_else(|| "无法读取分段目录".to_string())?;
        let part_index = base_index + index;
        let new_name = build_segment_file_name(&sanitized_prefix, part_index);
        let target = dir.join(new_name);
        if target != *segment {
            if target.exists() {
                let _ = fs::remove_file(&target);
            }
            fs::rename(segment, &target).map_err(|err| {
                format!(
                    "重命名分段文件失败: {} -> {} err={}",
                    segment.to_string_lossy(),
                    target.to_string_lossy(),
                    err
                )
            })?;
        }
        renamed.push(target);
    }
    Ok(renamed)
}

pub(super) fn save_merged_source_bindings(
    context: &SubmissionContext,
    task_id: &str,
    merged_id: i64,
    sources: &[ClipSource],
) -> Result<(), String> {
    if sources.is_empty() {
        return Ok(());
    }
    let source_records = load_task_source_video_records(context, task_id)?;
    let mut records_by_key: HashMap<(String, i64), String> = HashMap::new();
    let mut records_by_path: HashMap<String, String> = HashMap::new();
    for record in source_records {
        records_by_key.insert(
            (record.source_file_path.clone(), record.sort_order),
            record.id.clone(),
        );
        records_by_path
            .entry(record.source_file_path)
            .or_insert(record.id);
    }
    let now = now_rfc3339();
    let mut seen = HashSet::new();
    let mut normalized_sources = Vec::new();
    for source in sources {
        let key = (source.input_path.clone(), source.order);
        if seen.insert(key) {
            normalized_sources.push(source.clone());
        }
    }
    context
    .db
    .with_conn(|conn| {
      conn.execute(
        "DELETE FROM merged_source_video WHERE merged_id = ?1",
        [merged_id],
      )?;
      for source in &normalized_sources {
        if source.input_path.trim().is_empty() {
          continue;
        }
        let source_id = records_by_key
          .get(&(source.input_path.clone(), source.order))
          .or_else(|| records_by_path.get(&source.input_path))
          .cloned();
        let stored_source_path = to_stored_submission_path(context, &source.input_path);
        conn.execute(
          "INSERT INTO merged_source_video (task_id, merged_id, source_id, source_file_path, sort_order, start_time, end_time, create_time, update_time) \
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
          (
            task_id,
            merged_id,
            source_id,
            stored_source_path.as_str(),
            source.order,
            source.start_time.as_deref(),
            source.end_time.as_deref(),
            &now,
            &now,
          ),
        )?;
      }
      Ok(())
    })
    .map_err(|err| err.to_string())
}

pub(super) fn load_merged_source_clips(
    context: &SubmissionContext,
    task_id: &str,
    merged_id: i64,
) -> Result<Vec<ClipSource>, String> {
    context
        .db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT source_file_path, start_time, end_time, sort_order \
         FROM merged_source_video WHERE task_id = ?1 AND merged_id = ?2 \
         ORDER BY sort_order ASC",
            )?;
            let rows = stmt.query_map((task_id, merged_id), |row| {
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

pub(super) fn load_merged_source_paths(
    context: &SubmissionContext,
    task_id: &str,
    merged_id: i64,
) -> Result<Vec<String>, String> {
    let sources = load_merged_source_clips(context, task_id, merged_id)?;
    Ok(sources
        .into_iter()
        .map(|source| source.input_path)
        .filter(|value| !value.trim().is_empty())
        .collect())
}

pub(super) fn collect_sources_for_merge_all(
    context: &SubmissionContext,
    task_id: &str,
    merged_videos: &[MergedVideoRecord],
) -> Vec<ClipSource> {
    let mut sources = Vec::new();
    for merged in merged_videos {
        if let Ok(mut merged_sources) = load_merged_source_clips(context, task_id, merged.id) {
            sources.append(&mut merged_sources);
        }
    }
    if sources.is_empty() {
        if let Ok(fallback) = load_source_videos(context, task_id) {
            return normalize_binding_sources(fallback);
        }
    }
    normalize_binding_sources(sources)
}

pub(super) fn merged_display_name(merged: &MergedVideoRecord) -> String {
    if let Some(name) = merged
        .file_name
        .as_deref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        return name.to_string();
    }
    if let Some(path) = merged
        .video_path
        .as_deref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        if let Some(file_name) = Path::new(path).file_name().and_then(|value| value.to_str()) {
            let file_name = file_name.trim();
            if !file_name.is_empty() {
                return file_name.to_string();
            }
        }
        return path.to_string();
    }
    format!("合并视频#{}", merged.id)
}

pub(super) fn update_merged_video_path(
    context: &SubmissionContext,
    merged_id: i64,
    target_path: &Path,
) -> Result<(), String> {
    let now = now_rfc3339();
    let file_name = target_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("merged.mp4")
        .to_string();
    let path_str = to_stored_submission_path(context, target_path.to_string_lossy().as_ref());
    context
        .db
        .with_conn(|conn| {
            conn.execute(
        "UPDATE merged_video SET file_name = ?1, video_path = ?2, update_time = ?3 WHERE id = ?4",
        (&file_name, &path_str, &now, merged_id),
      )?;
            Ok(())
        })
        .map_err(|err| err.to_string())
}

pub(super) fn recreate_selected_merged_video_record(
    context: &SubmissionContext,
    task_id: &str,
    selected_merged_id: i64,
    merged_path: &Path,
) -> Result<i64, String> {
    let sources =
        load_merged_source_clips(context, task_id, selected_merged_id).unwrap_or_default();
    context
        .db
        .with_conn(|conn| {
            conn.execute(
                "DELETE FROM task_output_segment WHERE task_id = ?1 AND merged_id = ?2",
                (task_id, selected_merged_id),
            )?;
            conn.execute(
                "DELETE FROM merged_source_video WHERE task_id = ?1 AND merged_id = ?2",
                (task_id, selected_merged_id),
            )?;
            conn.execute(
                "DELETE FROM merged_video WHERE task_id = ?1 AND id = ?2",
                (task_id, selected_merged_id),
            )?;
            Ok(())
        })
        .map_err(|err| err.to_string())?;
    let new_merged_id = save_merged_video(context, task_id, merged_path)?;
    if !sources.is_empty() {
        save_merged_source_bindings(context, task_id, new_merged_id, &sources)?;
    }
    Ok(new_merged_id)
}

pub(super) fn delete_merged_records_by_ids(
    context: &SubmissionContext,
    task_id: &str,
    merged_ids: &[i64],
) -> Result<(), String> {
    if merged_ids.is_empty() {
        return Ok(());
    }
    context
        .db
        .with_conn(|conn| {
            for merged_id in merged_ids {
                conn.execute(
                    "DELETE FROM task_output_segment WHERE task_id = ?1 AND merged_id = ?2",
                    (task_id, merged_id),
                )?;
                conn.execute(
                    "DELETE FROM merged_source_video WHERE task_id = ?1 AND merged_id = ?2",
                    (task_id, merged_id),
                )?;
                conn.execute(
                    "DELETE FROM merged_video WHERE task_id = ?1 AND id = ?2",
                    (task_id, merged_id),
                )?;
            }
            Ok(())
        })
        .map_err(|err| err.to_string())
}

pub(super) fn save_video_clips(
    context: &SubmissionContext,
    task_id: &str,
    sources: &[ClipSource],
    outputs: &[PathBuf],
    replace_existing: bool,
) -> Result<(), String> {
    let now = now_rfc3339();
    context
    .db
    .with_conn(|conn| {
      if replace_existing {
        conn.execute("DELETE FROM video_clip WHERE task_id = ?1", [task_id])?;
      }
      for (index, output) in outputs.iter().enumerate() {
        let source = sources.get(index).cloned();
        let stored_clip_path =
          to_stored_submission_path(context, output.to_string_lossy().as_ref());
        conn.execute(
          "INSERT INTO video_clip (task_id, file_name, start_time, end_time, clip_path, sequence, status, create_time, update_time) \
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, 2, ?7, ?8)",
          (
            task_id,
            output.file_name().and_then(|name| name.to_str()).unwrap_or("clip.mp4"),
            source.as_ref().and_then(|s| s.start_time.as_deref()),
            source.as_ref().and_then(|s| s.end_time.as_deref()),
            stored_clip_path,
            (index + 1) as i64,
            &now,
            &now,
          ),
        )?;
      }
      Ok(())
    })
    .map_err(|err| err.to_string())
}

pub(super) fn save_merged_video(
    context: &SubmissionContext,
    task_id: &str,
    merged_path: &Path,
) -> Result<i64, String> {
    let now = now_rfc3339();
    let file_name = merged_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("merged.mp4");
    let total_bytes = fs::metadata(merged_path)
        .map(|meta| meta.len())
        .unwrap_or(0);
    let stored_path = to_stored_submission_path(context, merged_path.to_string_lossy().as_ref());

    context
    .db
    .with_conn(|conn| {
      conn.execute(
        "INSERT INTO merged_video (task_id, file_name, video_path, duration, status, upload_progress, upload_uploaded_bytes, upload_total_bytes, upload_cid, upload_file_name, upload_session_id, upload_biz_id, upload_endpoint, upload_auth, upload_uri, upload_chunk_size, upload_last_part_index, create_time, update_time) \
         VALUES (?1, ?2, ?3, NULL, 2, 0, 0, ?4, NULL, NULL, NULL, 0, NULL, NULL, NULL, 0, 0, ?5, ?6)",
        (
          task_id,
          file_name,
          stored_path,
          total_bytes as i64,
          &now,
          &now,
        ),
      )?;
      Ok(conn.last_insert_rowid())
    })
    .map_err(|err| err.to_string())
}

pub(super) fn save_merged_video_with_sort_order(
    context: &SubmissionContext,
    task_id: &str,
    merged_path: &Path,
    sort_order: Option<i64>,
) -> Result<i64, String> {
    let merged_id = save_merged_video(context, task_id, merged_path)?;
    let Some(sort_order) = sort_order else {
        return Ok(merged_id);
    };
    context
        .db
        .with_conn(|conn| {
            conn.execute(
                "UPDATE merged_video SET sort_order = ?1 WHERE id = ?2",
                (sort_order, merged_id),
            )?;
            Ok(())
        })
        .map_err(|err| err.to_string())?;
    Ok(merged_id)
}

pub(super) fn load_output_segment_stats(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<(usize, i64), String> {
    context
        .db
        .with_conn(|conn| {
            conn.query_row(
        "SELECT COUNT(*), COALESCE(MAX(part_order), 0) FROM task_output_segment WHERE task_id = ?1",
        [task_id],
        |row| {
          let count: i64 = row.get(0)?;
          let max_order: i64 = row.get(1)?;
          Ok((count.max(0) as usize, max_order.max(0)))
        },
      )
        })
        .map_err(|err| err.to_string())
}

/// 分段关闭时，用自定义「上传分P标题」覆盖某合并视频对应分P的 part_name。
/// 该 part_name 最终会被 build_uploaded_parts 用作 B 站分P标题。
pub(super) fn apply_non_segmented_part_title(
    context: &SubmissionContext,
    task_id: &str,
    merged_id: i64,
    title: &str,
) -> Result<(), String> {
    let title = title.trim();
    if title.is_empty() {
        return Ok(());
    }
    context
        .db
        .with_conn(|conn| {
            conn.execute(
                "UPDATE task_output_segment SET part_name = ?1 WHERE task_id = ?2 AND merged_id = ?3",
                (title, task_id, merged_id),
            )?;
            Ok(())
        })
        .map_err(|err| err.to_string())
}

pub(super) fn append_output_segments(
    context: &SubmissionContext,
    task_id: &str,
    segments: &[PathBuf],
    merged_id: Option<i64>,
    prefix: Option<&str>,
    part_order_start: i64,
    name_start_index: usize,
) -> Result<(), String> {
    let segments = rename_segment_outputs_with_prefix(segments, prefix, name_start_index)?;
    context
    .db
    .with_conn(|conn| {
      for (index, segment) in segments.iter().enumerate() {
        let segment_id = uuid::Uuid::new_v4().to_string();
        let file_name = segment.file_name().and_then(|name| name.to_str()).unwrap_or("segment.mp4");
        let total_bytes = fs::metadata(segment).map(|meta| meta.len()).unwrap_or(0);
        let part_order = part_order_start + index as i64;
        let part_name = build_part_title(prefix, name_start_index + index);
        let stored_segment_path =
          to_stored_submission_path(context, segment.to_string_lossy().as_ref());
        conn.execute(
          "INSERT INTO task_output_segment (segment_id, task_id, merged_id, part_name, segment_file_path, part_order, upload_status, cid, file_name, upload_progress, upload_uploaded_bytes, upload_total_bytes, upload_session_id, upload_biz_id, upload_endpoint, upload_auth, upload_uri, upload_chunk_size, upload_last_part_index) \
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'PENDING', NULL, ?7, 0, 0, ?8, NULL, 0, NULL, NULL, NULL, 0, 0)",
          (
            segment_id,
            task_id,
            merged_id,
            part_name,
            stored_segment_path,
            part_order,
            file_name,
            total_bytes as i64,
          ),
        )?;
      }
      Ok(())
    })
    .map_err(|err| err.to_string())
}

pub(super) fn save_output_segments(
    context: &SubmissionContext,
    task_id: &str,
    segments: &[PathBuf],
    merged_id: Option<i64>,
    prefix: Option<&str>,
) -> Result<(), String> {
    let segments = rename_segment_outputs_with_prefix(segments, prefix, 1)?;
    let has_prefix = prefix
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    context
    .db
    .with_conn(|conn| {
      conn.execute("DELETE FROM task_output_segment WHERE task_id = ?1", [task_id])?;
      for (index, segment) in segments.iter().enumerate() {
        let segment_id = uuid::Uuid::new_v4().to_string();
        let file_name = segment.file_name().and_then(|name| name.to_str()).unwrap_or("segment.mp4");
        let total_bytes = fs::metadata(segment).map(|meta| meta.len()).unwrap_or(0);
        let part_name = if has_prefix {
          build_part_title(prefix, index + 1)
        } else {
          format!("Part {}", index + 1)
        };
        let stored_segment_path =
          to_stored_submission_path(context, segment.to_string_lossy().as_ref());
        conn.execute(
          "INSERT INTO task_output_segment (segment_id, task_id, merged_id, part_name, segment_file_path, part_order, upload_status, cid, file_name, upload_progress, upload_uploaded_bytes, upload_total_bytes, upload_session_id, upload_biz_id, upload_endpoint, upload_auth, upload_uri, upload_chunk_size, upload_last_part_index) \
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'PENDING', NULL, ?7, 0, 0, ?8, NULL, 0, NULL, NULL, NULL, 0, 0)",
          (
            segment_id,
            task_id,
            merged_id,
            part_name,
            stored_segment_path,
            (index + 1) as i64,
            file_name,
            total_bytes as i64,
          ),
        )?;
      }
      Ok(())
    })
    .map_err(|err| err.to_string())
}

pub(super) fn reset_segments_for_new_bvid(context: &SubmissionContext, task_id: &str) -> Result<(), String> {
    context
        .db
        .with_conn(|conn| {
            conn.execute(RESET_ALL_SEGMENT_UPLOAD_SQL, [task_id])?;
            Ok(())
        })
        .map_err(|err| err.to_string())
}

pub(super) fn count_incomplete_segments(context: &SubmissionContext, task_id: &str) -> Result<i64, String> {
    context
    .db
    .with_conn(|conn| {
      let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM task_output_segment WHERE task_id = ?1 AND upload_status != 'SUCCESS'",
        [task_id],
        |row| row.get(0),
      )?;
      Ok(count)
    })
    .map_err(|err| err.to_string())
}

pub(super) fn load_output_segment_by_id(
    context: &SubmissionContext,
    segment_id: &str,
) -> Result<Option<TaskOutputSegmentRecord>, String> {
    context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT segment_id, task_id, merged_id, part_name, segment_file_path, part_order, upload_status, cid, file_name, \
                upload_progress, upload_uploaded_bytes, upload_total_bytes, upload_session_id, upload_biz_id, \
                upload_endpoint, upload_auth, upload_uri, upload_chunk_size, upload_last_part_index \
         FROM task_output_segment WHERE segment_id = ?1",
      )?;
      let result = stmt
        .query_row([segment_id], |row| {
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
        })
        .ok();
      Ok(result)
    })
    .map_err(|err| err.to_string())
    .map(|record| {
      record.map(|mut item| {
        item.segment_file_path = to_runtime_submission_path(context, &item.segment_file_path);
        item
      })
    })
}

pub(super) fn update_output_segments_for_edit(
    context: &SubmissionContext,
    task_id: &str,
    segments: &[SubmissionEditSegmentInput],
    sync_remote_update: bool,
) -> Result<(), String> {
    let mut ordered = segments.to_vec();
    ordered.sort_by_key(|segment| segment.part_order);
    context
    .db
    .with_conn_mut(|conn| {
      let tx = conn.transaction()?;
      let mut keep_ids = HashSet::new();
      let existing_ids = {
        let mut stmt =
          tx.prepare("SELECT segment_id FROM task_output_segment WHERE task_id = ?1")?;
        let rows = stmt.query_map([task_id], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<HashSet<_>, _>>()?
      };
      for (index, segment) in ordered.iter().enumerate() {
        let part_order = (index + 1) as i64;
        let segment_id = segment.segment_id.trim();
        let part_name = segment.part_name.trim();
        let file_path = segment.segment_file_path.trim();
        let stored_file_path = to_stored_submission_path(context, file_path);
        let cid = segment.cid.unwrap_or(0);
        let file_name = segment
          .file_name
          .as_deref()
          .map(|value| value.trim())
          .unwrap_or("");
        let actual_total_bytes = if file_path.is_empty() {
          0
        } else {
          fs::metadata(file_path)
            .map(|meta| meta.len() as i64)
            .unwrap_or(0)
        };
        let upload_status = segment
          .upload_status
          .as_deref()
          .map(|value| value.trim())
          .filter(|value| !value.is_empty())
          .unwrap_or(if cid > 0 && !file_name.is_empty() {
            "SUCCESS"
          } else {
            "PENDING"
          });
        let upload_progress = segment.upload_progress.unwrap_or(if cid > 0 && !file_name.is_empty() {
          100.0
        } else {
          0.0
        });
        let upload_uploaded_bytes = segment.upload_uploaded_bytes.unwrap_or(if cid > 0 && !file_name.is_empty() {
          actual_total_bytes
        } else {
          0
        });
        let upload_total_bytes = segment.upload_total_bytes.unwrap_or(actual_total_bytes);
        if existing_ids.contains(segment_id) {
          if sync_remote_update {
            tx.execute(
              "UPDATE task_output_segment SET part_name = ?1, part_order = ?2, segment_file_path = ?3, upload_status = 'SUCCESS', cid = ?4, file_name = ?5, upload_progress = 100, upload_uploaded_bytes = ?6, upload_total_bytes = ?7, upload_session_id = NULL, upload_biz_id = 0, upload_endpoint = NULL, upload_auth = NULL, upload_uri = NULL, upload_chunk_size = 0, upload_last_part_index = 0 WHERE segment_id = ?8 AND task_id = ?9",
              (
                part_name,
                part_order,
                stored_file_path.as_str(),
                cid,
                file_name,
                actual_total_bytes,
                actual_total_bytes,
                segment_id,
                task_id,
              ),
            )?;
          } else {
            tx.execute(
              "UPDATE task_output_segment SET part_name = ?1, part_order = ?2, segment_file_path = ?3, cid = ?4, file_name = ?5, upload_status = ?6, upload_progress = ?7, upload_uploaded_bytes = ?8, upload_total_bytes = ?9 WHERE segment_id = ?10 AND task_id = ?11",
              (
                part_name,
                part_order,
                stored_file_path.as_str(),
                cid,
                file_name,
                upload_status,
                upload_progress,
                upload_uploaded_bytes,
                upload_total_bytes,
                segment_id,
                task_id,
              ),
            )?;
          }
        } else {
          tx.execute(
            "INSERT INTO task_output_segment (segment_id, task_id, merged_id, part_name, segment_file_path, part_order, upload_status, cid, file_name, upload_progress, upload_uploaded_bytes, upload_total_bytes, upload_session_id, upload_biz_id, upload_endpoint, upload_auth, upload_uri, upload_chunk_size, upload_last_part_index) \
             VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, NULL, 0, NULL, NULL, NULL, 0, 0)",
            (
              segment_id,
              task_id,
              part_name,
              stored_file_path.as_str(),
              part_order,
              if sync_remote_update { "SUCCESS" } else { upload_status },
              cid,
              file_name,
              if sync_remote_update { 100.0 } else { upload_progress },
              if sync_remote_update {
                actual_total_bytes
              } else {
                upload_uploaded_bytes
              },
              if sync_remote_update {
                actual_total_bytes
              } else {
                upload_total_bytes
              },
            ),
          )?;
        }
        keep_ids.insert(segment_id.to_string());
      }
      for segment_id in existing_ids {
        if !keep_ids.contains(&segment_id) {
          tx.execute(
            "DELETE FROM task_output_segment WHERE segment_id = ?1",
            [segment_id],
          )?;
        }
      }
      tx.commit()?;
      Ok(())
    })
    .map_err(|err| err.to_string())
}

pub(super) fn load_latest_merged_video(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<Option<MergedVideoRecord>, String> {
    context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT id, task_id, file_name, video_path, remote_dir, remote_name, duration, status, \
                upload_progress, upload_uploaded_bytes, upload_total_bytes, upload_cid, upload_file_name, \
                upload_session_id, upload_biz_id, upload_endpoint, upload_auth, upload_uri, upload_chunk_size, \
                upload_last_part_index, create_time, update_time \
         FROM merged_video WHERE task_id = ?1 ORDER BY id DESC LIMIT 1",
      )?;
      let result = stmt
        .query_row([task_id], |row| {
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
        })
        .ok();
      Ok(result)
    })
    .map_err(|err| err.to_string())
    .map(|record| {
      record.map(|mut item| {
        item.video_path = to_runtime_submission_path_opt(context, item.video_path.clone());
        item
      })
    })
}

pub(super) fn load_merged_video_by_id(
    context: &SubmissionContext,
    task_id: &str,
    merged_id: i64,
) -> Result<Option<MergedVideoRecord>, String> {
    context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT id, task_id, file_name, video_path, remote_dir, remote_name, duration, status, \
                upload_progress, upload_uploaded_bytes, upload_total_bytes, upload_cid, upload_file_name, \
                upload_session_id, upload_biz_id, upload_endpoint, upload_auth, upload_uri, upload_chunk_size, \
                upload_last_part_index, create_time, update_time \
         FROM merged_video WHERE task_id = ?1 AND id = ?2",
      )?;
      let result = stmt
        .query_row((task_id, merged_id), |row| {
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
        })
        .ok();
      Ok(result)
    })
    .map_err(|err| err.to_string())
    .map(|record| {
      record.map(|mut item| {
        item.video_path = to_runtime_submission_path_opt(context, item.video_path.clone());
        item
      })
    })
}

pub(super) fn load_merged_videos_by_task(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<Vec<MergedVideoRecord>, String> {
    context
    .db
    .with_conn(|conn| {
      let mut stmt = conn.prepare(
        "SELECT id, task_id, file_name, video_path, remote_dir, remote_name, duration, status, \
                upload_progress, upload_uploaded_bytes, upload_total_bytes, upload_cid, upload_file_name, \
                upload_session_id, upload_biz_id, upload_endpoint, upload_auth, upload_uri, upload_chunk_size, \
                upload_last_part_index, create_time, update_time \
         FROM merged_video WHERE task_id = ?1 ORDER BY CASE WHEN COALESCE(sort_order, 0) > 0 THEN 0 ELSE 1 END ASC, COALESCE(sort_order, 0) ASC, create_time ASC, id ASC",
      )?;
      let rows = stmt.query_map([task_id], |row| {
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
      })?;
      Ok(rows.collect::<Result<Vec<_>, _>>()?)
    })
    .map_err(|err| err.to_string())
    .map(|mut list| {
      for item in &mut list {
        item.video_path = to_runtime_submission_path_opt(context, item.video_path.clone());
      }
      list
    })
}

pub(super) fn load_output_segment_range_by_merged_id(
    context: &SubmissionContext,
    task_id: &str,
    merged_id: i64,
) -> Result<Option<(i64, i64, i64)>, String> {
    context
    .db
    .with_conn(|conn| {
      let (min_order, max_order, count): (Option<i64>, Option<i64>, i64) = conn.query_row(
        "SELECT MIN(part_order), MAX(part_order), COUNT(*) FROM task_output_segment WHERE task_id = ?1 AND merged_id = ?2",
        (task_id, merged_id),
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
      )?;
      if count <= 0 {
        return Ok(None);
      }
      let min_order = min_order.unwrap_or(1);
      let max_order = max_order.unwrap_or(min_order);
      Ok(Some((min_order, max_order, count)))
    })
    .map_err(|err| err.to_string())
}

pub(super) fn ensure_segments_bound_to_merged(
    context: &SubmissionContext,
    task_id: &str,
    merged_id: i64,
) -> Result<(), String> {
    let (total_segments, bound_segments, null_segments, merged_count) = context
    .db
    .with_conn(|conn| {
      let total: i64 = conn.query_row(
        "SELECT COUNT(*) FROM task_output_segment WHERE task_id = ?1",
        [task_id],
        |row| row.get(0),
      )?;
      let bound: i64 = conn.query_row(
        "SELECT COUNT(*) FROM task_output_segment WHERE task_id = ?1 AND merged_id = ?2",
        (task_id, merged_id),
        |row| row.get(0),
      )?;
      let null_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM task_output_segment WHERE task_id = ?1 AND (merged_id IS NULL OR merged_id = 0)",
        [task_id],
        |row| row.get(0),
      )?;
      let merged_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM merged_video WHERE task_id = ?1",
        [task_id],
        |row| row.get(0),
      )?;
      Ok((total, bound, null_count, merged_count))
    })
    .map_err(|err| err.to_string())?;
    if total_segments == 0 || bound_segments > 0 {
        return Ok(());
    }
    if merged_count == 1 || null_segments == total_segments {
        context
      .db
      .with_conn(|conn| {
        conn.execute(
          "UPDATE task_output_segment SET merged_id = ?1 WHERE task_id = ?2 AND (merged_id IS NULL OR merged_id = 0)",
          (merged_id, task_id),
        )?;
        Ok(())
      })
      .map_err(|err| err.to_string())?;
    }
    Ok(())
}

pub(super) fn replace_segments_for_merged(
    context: &SubmissionContext,
    task_id: &str,
    merged_id: i64,
    outputs: &[PathBuf],
    segment_prefix: Option<&str>,
) -> Result<(), String> {
    if outputs.is_empty() {
        return Err("重新分段输出为空".to_string());
    }
    let range = load_output_segment_range_by_merged_id(context, task_id, merged_id)?;
    let (start_order, old_count, end_order) = match range {
        Some((min_order, max_order, count)) => (min_order, count, max_order),
        None => {
            let (existing_count, _max_order) = load_output_segment_stats(context, task_id)?;
            if existing_count == 0 {
                return save_output_segments(
                    context,
                    task_id,
                    outputs,
                    Some(merged_id),
                    segment_prefix,
                );
            }
            return Err("未找到合并视频对应的分段范围".to_string());
        }
    };
    let name_start_index = if start_order > 0 {
        start_order as usize
    } else {
        1
    };
    let outputs = rename_segment_outputs_with_prefix(outputs, segment_prefix, name_start_index)?;
    let delta = outputs.len() as i64 - old_count;
    context
    .db
    .with_conn_mut(|conn| {
      let tx = conn.transaction()?;
      tx.execute(
        "DELETE FROM task_output_segment WHERE task_id = ?1 AND merged_id = ?2 AND part_order >= ?3 AND part_order <= ?4",
        (task_id, merged_id, start_order, end_order),
      )?;
      let mut rename_targets: Vec<(String, i64)> = Vec::new();
      if delta != 0 {
        let mut stmt = tx.prepare(
          "SELECT segment_id, part_name, part_order FROM task_output_segment WHERE task_id = ?1 AND part_order > ?2",
        )?;
        let rows = stmt.query_map((task_id, end_order), |row| {
          Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
          ))
        })?;
        for row in rows {
          let (segment_id, part_name, part_order) = row?;
          if is_default_part_name(&part_name, part_order, segment_prefix) {
            rename_targets.push((segment_id, part_order));
          }
        }
        tx.execute(
          "UPDATE task_output_segment SET part_order = part_order + ?1 WHERE task_id = ?2 AND part_order > ?3",
          (delta, task_id, end_order),
        )?;
      }
      for (index, segment) in outputs.iter().enumerate() {
        let segment_id = uuid::Uuid::new_v4().to_string();
        let file_name = segment.file_name().and_then(|name| name.to_str()).unwrap_or("segment.mp4");
        let total_bytes = fs::metadata(segment).map(|meta| meta.len()).unwrap_or(0);
        let part_order = start_order + index as i64;
        let part_index = if part_order > 0 { part_order as usize } else { 1 };
        let part_name = build_part_title(segment_prefix, part_index);
        let stored_segment_path =
          to_stored_submission_path(context, segment.to_string_lossy().as_ref());
        tx.execute(
          "INSERT INTO task_output_segment (segment_id, task_id, merged_id, part_name, segment_file_path, part_order, upload_status, cid, file_name, upload_progress, upload_uploaded_bytes, upload_total_bytes, upload_session_id, upload_biz_id, upload_endpoint, upload_auth, upload_uri, upload_chunk_size, upload_last_part_index) \
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'PENDING', NULL, ?7, 0, 0, ?8, NULL, 0, NULL, NULL, NULL, 0, 0)",
          (
            segment_id,
            task_id,
            merged_id,
            part_name,
            stored_segment_path,
            part_order,
            file_name,
            total_bytes as i64,
          ),
        )?;
      }
      for (segment_id, part_order) in rename_targets {
        let new_order = part_order + delta;
        if new_order > 0 {
          let new_name = build_part_title(segment_prefix, new_order as usize);
          tx.execute(
            "UPDATE task_output_segment SET part_name = ?1 WHERE segment_id = ?2",
            (new_name, segment_id),
          )?;
        }
      }
      tx.commit()?;
      Ok(())
    })
    .map_err(|err| err.to_string())
}

pub(super) fn build_merge_output_path(workflow_dir: &Path, task_id: &str) -> PathBuf {
    let stamp = sanitize_filename(&now_rfc3339());
    let task = sanitize_filename(task_id);
    workflow_dir
        .join("merge")
        .join(format!("{}_merged_{}.mp4", task, stamp))
}

pub(super) fn build_merge_output_path_for_task(workflow_dir: &Path, task: &SubmissionTaskRecord) -> PathBuf {
    if task.baidu_sync_enabled {
        return build_merge_output_path_with_remote_name(
            workflow_dir,
            task.baidu_sync_filename.as_deref(),
            &task.task_id,
        );
    }
    build_merge_output_path(workflow_dir, &task.task_id)
}

pub(super) fn build_merge_output_path_for_task_id(
    context: &SubmissionContext,
    workflow_dir: &Path,
    task_id: &str,
) -> Result<PathBuf, String> {
    let (enabled, filename): (i64, Option<String>) = context
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT IFNULL(baidu_sync_enabled, 0), baidu_sync_filename FROM submission_task WHERE task_id = ?1",
                [task_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?)),
            )
        })
        .map_err(|err| err.to_string())?;
    if enabled != 0 {
        return Ok(build_merge_output_path_with_remote_name(
            workflow_dir,
            filename.as_deref(),
            task_id,
        ));
    }
    Ok(build_merge_output_path(workflow_dir, task_id))
}

pub(super) fn build_merge_output_path_for_group(
    workflow_dir: &Path,
    task_id: &str,
    stamp: &str,
    group_index: usize,
) -> PathBuf {
    let task = sanitize_filename(task_id);
    let safe_stamp = sanitize_filename(stamp);
    workflow_dir.join("merge").join(format!(
        "{}_merged_{}_g{:02}.mp4",
        task, safe_stamp, group_index
    ))
}

pub(super) fn resolve_target_merged_video(
    context: &SubmissionContext,
    task_id: &str,
    requested_id: Option<i64>,
    app_log_path: &PathBuf,
) -> Result<MergedVideoRecord, String> {
    if let Some(merged_id) = requested_id {
        if let Ok(Some(merged)) = load_merged_video_by_id(context, task_id, merged_id) {
            ensure_segments_bound_to_merged(context, task_id, merged.id)?;
            return Ok(merged);
        }
        append_log(
            app_log_path,
            &format!(
                "submission_reprocess_merge_fallback task_id={} merged_id={} reason=missing",
                task_id, merged_id
            ),
        );
    }
    let latest =
        load_latest_merged_video(context, task_id)?.ok_or_else(|| "未找到合并视频".to_string())?;
    ensure_segments_bound_to_merged(context, task_id, latest.id)?;
    Ok(latest)
}

pub(super) fn reset_failed_segments_to_pending(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<(), String> {
    context
    .db
    .with_conn(|conn| {
      conn.execute(
        "UPDATE task_output_segment SET upload_status = 'PENDING' WHERE task_id = ?1 AND upload_status = 'FAILED'",
        [task_id],
      )?;
      Ok(())
    })
    .map_err(|err| err.to_string())
}

pub(super) fn list_submission_ffmpeg_pids(task_id: &str) -> Result<Vec<(u32, String)>, String> {
    let ffmpeg_path = crate::config::resolve_ffmpeg_path();
    let ffmpeg_path = ffmpeg_path.to_string_lossy().to_string();
    let task_marker = format!("/{}/", task_id);
    let output = Command::new("ps")
        .args(["-axo", "pid=,command="])
        .output()
        .map_err(|err| format!("读取 ffmpeg 进程失败: {}", err))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("读取 ffmpeg 进程失败: {}", stderr.trim()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut result = Vec::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let pid_text = parts.next().unwrap_or("").trim();
        let command = parts.next().unwrap_or("").trim();
        if pid_text.is_empty() || command.is_empty() {
            continue;
        }
        if !command.contains(&ffmpeg_path) || !command.contains(&task_marker) {
            continue;
        }
        let pid = match pid_text.parse::<u32>() {
            Ok(value) => value,
            Err(_) => continue,
        };
        result.push((pid, command.to_string()));
    }
    Ok(result)
}

// 清理过程含轮询等待进程退出的阻塞 sleep，异步上下文中必须放到阻塞线程池执行。
pub(super) async fn terminate_submission_ffmpeg_processes_async(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<(), String> {
    let context = context.clone();
    let task_id = task_id.to_string();
    tauri::async_runtime::spawn_blocking(move || {
        terminate_submission_ffmpeg_processes(&context, &task_id)
    })
    .await
    .map_err(|err| format!("ffmpeg cleanup task failed: {}", err))?
}

pub(super) fn terminate_submission_ffmpeg_processes(
    context: &SubmissionContext,
    task_id: &str,
) -> Result<(), String> {
    let processes = list_submission_ffmpeg_pids(task_id)?;
    if processes.is_empty() {
        return Ok(());
    }

    append_log(
        &context.app_log_path,
        &format!(
            "submission_ffmpeg_cleanup_start task_id={} count={}",
            task_id,
            processes.len()
        ),
    );

    let mut failed = Vec::new();
    for (pid, command) in processes {
        append_log(
            &context.app_log_path,
            &format!(
                "submission_ffmpeg_cleanup_term task_id={} pid={} command={}",
                task_id, pid, command
            ),
        );
        if let Err(err) = send_signal_to_process(pid, "-TERM") {
            append_log(
                &context.app_log_path,
                &format!(
                    "submission_ffmpeg_cleanup_term_fail task_id={} pid={} err={}",
                    task_id, pid, err
                ),
            );
        }

        let mut stopped = false;
        for _ in 0..10 {
            std::thread::sleep(Duration::from_millis(200));
            if !is_process_alive(pid) {
                stopped = true;
                break;
            }
        }

        if !stopped && is_process_alive(pid) {
            append_log(
                &context.app_log_path,
                &format!(
                    "submission_ffmpeg_cleanup_kill task_id={} pid={}",
                    task_id, pid
                ),
            );
            if let Err(err) = send_signal_to_process(pid, "-KILL") {
                failed.push(format!("pid={} err={}", pid, err));
                continue;
            }
            for _ in 0..10 {
                std::thread::sleep(Duration::from_millis(200));
                if !is_process_alive(pid) {
                    stopped = true;
                    break;
                }
            }
        }

        if !stopped && is_process_alive(pid) {
            failed.push(format!("pid={} err=still_alive", pid));
        } else {
            append_log(
                &context.app_log_path,
                &format!(
                    "submission_ffmpeg_cleanup_done task_id={} pid={}",
                    task_id, pid
                ),
            );
        }
    }

    if failed.is_empty() {
        return Ok(());
    }

    Err(format!(
        "终止旧剪辑进程失败 task_id={} details={}",
        task_id,
        failed.join("; ")
    ))
}
