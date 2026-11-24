//! 竞品对比的执行入口：把「读取本场数据 → 逐条比对 → 出报告」串起来，
//! 并提供前端调用的 Tauri 命令。

use serde::{Deserialize, Serialize};
use tauri::State;

use super::competitor_compare::{
    ClipComparison, Coverage, ReasonLayer, attribute_missing, best_local_clip,
    coverage_of, load_local_clips, locate_in_session, session_segments, suggestions_for,
};
use crate::api::ApiResponse;
use crate::db::Db;
use crate::utils::{append_log, now_rfc3339};
use crate::AppState;

/// 对比报告（整体）
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareReport {
    pub task_id: i64,
    pub room_id: String,
    pub session_label: String,
    pub total_clips: usize,
    pub full_count: usize,
    pub part_count: usize,
    pub zero_count: usize,
    pub skip_count: usize,
    pub coverage_rate: f64,
    pub items: Vec<ClipComparison>,
    pub create_time: String,
}

/// 竞品切片在库里的完整记录
struct CompetitorRow {
    id: i64,
    bvid: String,
    title: String,
    up_uid: String,
    up_name: String,
    cover_url: String,
    duration_secs: f64,
    status: String,
    status_note: String,
    asr_path: String,
}

fn load_competitor_rows(db: &Db, task_id: i64) -> Result<Vec<CompetitorRow>, String> {
    db.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT c.id, c.bvid, COALESCE(c.title,''), c.up_uid, COALESCE(u.up_name,''), \
                    COALESCE(c.cover_url,''), COALESCE(c.duration_secs, 0), c.status, \
                    COALESCE(c.status_note,''), COALESCE(c.asr_path,'') \
             FROM competitor_clip c \
             LEFT JOIN competitor_up u ON u.room_id = c.room_id AND u.up_uid = c.up_uid \
             WHERE c.task_id = ?1 ORDER BY c.pub_time ASC, c.id ASC",
        )?;
        let rows = stmt.query_map([task_id], |row| {
            Ok(CompetitorRow {
                id: row.get(0)?,
                bvid: row.get(1)?,
                title: row.get(2)?,
                up_uid: row.get(3)?,
                up_name: row.get(4)?,
                cover_url: row.get(5)?,
                duration_secs: row.get(6)?,
                status: row.get(7)?,
                status_note: row.get(8)?,
                asr_path: row.get(9)?,
            })
        })?;
        rows.collect()
    })
    .map_err(|err| format!("读取竞品切片失败: {}", err))
}

/// 读本场切片的候选与拒绝记录，用于分层归因。
/// 候选信息记在 signals 字段里，没有记录时返回空（归因会落到第 1 层）。
fn load_candidate_info(db: &Db, task_id: i64) -> (Vec<(f64, f64)>, Vec<(f64, f64)>) {
    let candidates = db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT start_offset, end_offset FROM live_clip_item \
                 WHERE task_id = ?1 AND status = 'SUCCESS'",
            )?;
            let rows = stmt.query_map([task_id], |row| {
                Ok((row.get::<_, f64>(0)?, row.get::<_, f64>(1)?))
            })?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .unwrap_or_default();
    // 被大模型判为不切的记录：ai_status 标记为 REJECTED 的条目
    let rejected = db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT start_offset, end_offset FROM live_clip_item \
                 WHERE task_id = ?1 AND ai_status = 'REJECTED'",
            )?;
            let rows = stmt.query_map([task_id], |row| {
                Ok((row.get::<_, f64>(0)?, row.get::<_, f64>(1)?))
            })?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .unwrap_or_default();
    (candidates, rejected)
}

/// 读取本场的 ASR 段落。缓存不存在时返回 None（此时所有竞品记为 SKIPPED）。
fn load_session_segments(db: &Db, room_id: &str, task_id: i64) -> Option<Vec<(f64, f64, String)>> {
    let record_ids: Vec<i64> = db
        .with_conn(|conn| {
            let raw: String = conn.query_row(
                "SELECT record_ids FROM live_clip_task WHERE id = ?1",
                [task_id],
                |r| r.get(0),
            )?;
            Ok(serde_json::from_str::<Vec<i64>>(&raw).unwrap_or_default())
        })
        .ok()?;
    if record_ids.is_empty() {
        return None;
    }
    let pass = crate::highlight::load_cached_pass(db, room_id, &record_ids)?;
    let json = serde_json::to_value(&pass).ok()?;
    let segs = session_segments(&json);
    if segs.is_empty() {
        None
    } else {
        Some(segs)
    }
}

/// 执行一场完整的竞品对比
pub fn run_compare(db: &Db, room_id: &str, task_id: i64, max_candidates: usize) -> Result<CompareReport, String> {
    let room_id = room_id.trim();
    let rows = load_competitor_rows(db, task_id)?;
    if rows.is_empty() {
        return Err("本场没有竞品切片".into());
    }
    let local_clips = load_local_clips(db, task_id)?;
    let session_segs = load_session_segments(db, room_id, task_id);
    let (candidate_ranges, rejected_ranges) = load_candidate_info(db, task_id);

    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        // 竞品本身没处理完，直接记跳过
        if row.status != "DONE" {
            items.push(ClipComparison {
                competitor_clip_id: row.id,
                bvid: row.bvid.clone(),
                title: row.title.clone(),
                up_uid: row.up_uid.clone(),
                up_name: row.up_name.clone(),
                cover_url: row.cover_url.clone(),
                duration_secs: row.duration_secs,
                located: None,
                mine_clip_id: None,
                mine_title: String::new(),
                mine_start: None,
                mine_end: None,
                iou: 0.0,
                coverage: Coverage::Skip,
                reason_layer: None,
                reason: format!("竞品未处理完成（{}）", status_label(&row.status)),
                suggestions: Vec::new(),
                status_note: row.status_note.clone(),
            });
            continue;
        }

        // 本场数据缺失 → 跳过
        let Some(segs) = session_segs.as_ref() else {
            items.push(ClipComparison {
                competitor_clip_id: row.id,
                bvid: row.bvid.clone(),
                title: row.title.clone(),
                up_uid: row.up_uid.clone(),
                up_name: row.up_name.clone(),
                cover_url: row.cover_url.clone(),
                duration_secs: row.duration_secs,
                located: None,
                mine_clip_id: None,
                mine_title: String::new(),
                mine_start: None,
                mine_end: None,
                iou: 0.0,
                coverage: Coverage::Skip,
                reason_layer: None,
                reason: "本场识别数据已清理，无法定位竞品内容".into(),
                suggestions: Vec::new(),
                status_note: String::new(),
            });
            continue;
        };

        // 读竞品转写，定位到本场时间轴
        let transcript = std::path::Path::new(&row.asr_path);
        let competitor_text = super::competitor_media::load_transcript(transcript)
            .map(|value| {
                session_segments(&value)
                    .into_iter()
                    .map(|(_, _, t)| t)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();

        let located = locate_in_session(&competitor_text, segs);
        let Some(range) = located else {
            items.push(ClipComparison {
                competitor_clip_id: row.id,
                bvid: row.bvid.clone(),
                title: row.title.clone(),
                up_uid: row.up_uid.clone(),
                up_name: row.up_name.clone(),
                cover_url: row.cover_url.clone(),
                duration_secs: row.duration_secs,
                located: None,
                mine_clip_id: None,
                mine_title: String::new(),
                mine_start: None,
                mine_end: None,
                iou: 0.0,
                coverage: Coverage::Skip,
                reason_layer: None,
                reason: "本场转写里找不到竞品内容，可能归属错了场次".into(),
                suggestions: Vec::new(),
                status_note: String::new(),
            });
            continue;
        };

        // 与本地切片比较
        let matched = best_local_clip((range.start, range.end), &local_clips);
        let (mine_clip, iou_value) = matched
            .map(|(clip, score)| (Some(clip), score))
            .unwrap_or((None, 0.0));
        let coverage = coverage_of(iou_value);

        let (reason_layer, reason, suggestions) = match coverage {
            Coverage::Zero => {
                let (layer, msg) = attribute_missing(
                    (range.start, range.end),
                    segs,
                    &candidate_ranges,
                    &rejected_ranges,
                    max_candidates,
                );
                let tips = suggestions_for(&layer, row.duration_secs, 0.0);
                (Some(layer), msg, tips)
            }
            Coverage::Part => {
                let mine_dur = mine_clip
                    .as_ref()
                    .map(|clip| clip.end - clip.start)
                    .unwrap_or(0.0);
                let tips = suggestions_for(&ReasonLayer::Rule, row.duration_secs, mine_dur);
                (
                    None,
                    format!(
                        "切到了，但边界与竞品不一致（交并比 {:.2}）",
                        iou_value
                    ),
                    tips,
                )
            }
            _ => (None, String::new(), Vec::new()),
        };

        items.push(ClipComparison {
            competitor_clip_id: row.id,
            bvid: row.bvid.clone(),
            title: row.title.clone(),
            up_uid: row.up_uid.clone(),
            up_name: row.up_name.clone(),
            cover_url: row.cover_url.clone(),
            duration_secs: row.duration_secs,
            located: Some(range),
            mine_clip_id: mine_clip.as_ref().map(|clip| clip.id),
            mine_title: mine_clip
                .as_ref()
                .map(|clip| clip.title.clone())
                .unwrap_or_default(),
            mine_start: mine_clip.as_ref().map(|clip| clip.start),
            mine_end: mine_clip.as_ref().map(|clip| clip.end),
            iou: iou_value,
            coverage,
            reason_layer,
            reason,
            suggestions,
            status_note: String::new(),
        });
    }

    // 未覆盖优先，便于直接分析漏切原因
    items.sort_by_key(|item| match item.coverage {
        Coverage::Zero => 0,
        Coverage::Part => 1,
        Coverage::Full => 2,
        Coverage::Skip => 3,
    });

    let full_count = items.iter().filter(|i| i.coverage == Coverage::Full).count();
    let part_count = items.iter().filter(|i| i.coverage == Coverage::Part).count();
    let zero_count = items.iter().filter(|i| i.coverage == Coverage::Zero).count();
    let skip_count = items.iter().filter(|i| i.coverage == Coverage::Skip).count();
    let judged = full_count + part_count + zero_count;
    let coverage_rate = if judged == 0 {
        0.0
    } else {
        (full_count as f64 + part_count as f64 * 0.5) / judged as f64 * 100.0
    };

    let label: String = db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT COALESCE(date_label,'') FROM live_clip_task WHERE id = ?1",
                [task_id],
                |r| r.get(0),
            )
        })
        .unwrap_or_default();

    let now = now_rfc3339();
    let report = CompareReport {
        task_id,
        room_id: room_id.to_string(),
        session_label: label,
        total_clips: items.len(),
        full_count,
        part_count,
        zero_count,
        skip_count,
        coverage_rate,
        items,
        create_time: now.clone(),
    };

    // 落库：报告明细写文件，数据库只存索引与计数
    persist_report(db, &report)?;
    Ok(report)
}

fn status_label(status: &str) -> &'static str {
    match status {
        "PENDING" => "待下载",
        "DOWNLOADING" => "下载中",
        "TRANSCRIBING" => "转写中",
        "FAILED" => "失败",
        "SKIPPED" => "已跳过",
        _ => "未知状态",
    }
}

/// 把报告写盘并更新索引。同一场次只保留一份有效报告，旧的标记作废。
fn persist_report(db: &Db, report: &CompareReport) -> Result<(), String> {
    // 报告放在切片目录下的 competitor 子目录
    let base_dir = db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT COALESCE(i.source_file_path, '') \
                 FROM live_clip_item i WHERE i.task_id = ?1 LIMIT 1",
                [report.task_id],
                |r| r.get::<_, String>(0),
            )
        })
        .ok()
        .filter(|p| !p.trim().is_empty())
        .and_then(|p| super::competitor_media::competitor_dir_of(&p));

    let report_path = if let Some(dir) = base_dir {
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join(format!("report_{}.json", report.task_id));
        if let Ok(bytes) = serde_json::to_vec_pretty(report) {
            let _ = std::fs::write(&path, bytes);
        }
        path.to_string_lossy().to_string()
    } else {
        String::new()
    };

    let now = now_rfc3339();
    db.with_conn(|conn| {
        conn.execute(
            "UPDATE competitor_compare SET status = 'VOID', update_time = ?1 WHERE task_id = ?2",
            (&now, report.task_id),
        )?;
        conn.execute(
            "INSERT INTO competitor_compare \
                (task_id, room_id, status, total_clips, full_count, part_count, zero_count, \
                 skip_count, report_path, create_time, update_time) \
             VALUES (?1, ?2, 'DONE', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
            rusqlite::params![
                report.task_id,
                &report.room_id,
                report.total_clips as i64,
                report.full_count as i64,
                report.part_count as i64,
                report.zero_count as i64,
                report.skip_count as i64,
                &report_path,
                &now,
            ],
        )?;
        Ok(())
    })
    .map_err(|err| format!("保存对比报告失败: {}", err))?;
    Ok(())
}

/// 读取最近一次有效报告
pub fn load_report(db: &Db, task_id: i64) -> Result<Option<CompareReport>, String> {
    let path: Option<String> = db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT report_path FROM competitor_compare \
                 WHERE task_id = ?1 AND status = 'DONE' ORDER BY id DESC LIMIT 1",
                [task_id],
                |r| r.get::<_, Option<String>>(0),
            )
        })
        .ok()
        .flatten();
    let Some(path) = path.filter(|p| !p.is_empty()) else {
        return Ok(None);
    };
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(None),
    };
    let report: CompareReport =
        serde_json::from_slice(&bytes).map_err(|err| format!("解析对比报告失败: {}", err))?;
    Ok(Some(report))
}

// ============ Tauri 命令 ============

#[tauri::command(async)]
pub fn competitor_compare_run(
    state: State<'_, AppState>,
    room_id: String,
    task_id: i64,
) -> ApiResponse<CompareReport> {
    let max_candidates = crate::highlight::settings::load(&state.db).clip.max_candidates;
    let log_path = state.log_path.clone();
    match run_compare(&state.db, &room_id, task_id, max_candidates) {
        Ok(report) => {
            append_log(
                log_path.as_path(),
                &format!(
                    "competitor_compare_done task_id={} full={} part={} zero={} skip={}",
                    task_id, report.full_count, report.part_count, report.zero_count, report.skip_count
                ),
            );
            ApiResponse::success(report)
        }
        Err(err) => ApiResponse::error(err),
    }
}

#[tauri::command(async)]
pub fn competitor_compare_get(state: State<'_, AppState>, task_id: i64) -> ApiResponse<Option<CompareReport>> {
    match load_report(&state.db, task_id) {
        Ok(report) => ApiResponse::success(report),
        Err(err) => ApiResponse::error(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn mem_db() -> Db {
        Db::new(PathBuf::from(":memory:")).expect("init db")
    }

    fn seed_task(db: &Db) {
        let now = now_rfc3339();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO live_clip_task (id, room_id, record_ids, date_label, status, clip_count, create_time, update_time, session_key) \
                 VALUES (1, '25034104', '[101]', '2026-11-22', 'DONE', 1, ?1, ?1, 's1')",
                [&now],
            )?;
            conn.execute(
                "INSERT INTO live_record_task (id, room_id, status, file_path, segment_index, start_time, end_time, create_time, update_time) \
                 VALUES (101, '25034104', 'STOPPED', '/tmp/a.mp4', 1, '2026-11-22T12:00:00+00:00', '2026-11-22T20:00:00+00:00', ?1, ?1)",
                [&now],
            )?;
            Ok(())
        })
        .unwrap();
    }

    fn seed_clip(db: &Db, status: &str) -> i64 {
        let now = now_rfc3339();
        db.with_conn(|conn| {
            // bvid 有唯一索引，用自增计数避免冲突
            let seq: i64 = conn.query_row(
                "SELECT COUNT(*) + 1 FROM competitor_clip", [], |r| r.get(0))?;
            conn.execute(
                "INSERT INTO competitor_clip (room_id, up_uid, bvid, title, duration_secs, \
                    task_id, session_key, status, create_time, update_time) \
                 VALUES ('25034104', '82679456', ?1, '测试竞品', 60.0, 1, 's1', ?2, ?3, ?3)",
                (format!("BV1x{}", seq), status, &now),
            )?;
            Ok(conn.last_insert_rowid())
        })
        .unwrap()
    }

    #[test]
    fn reports_error_when_no_competitor_clips() {
        let db = mem_db();
        seed_task(&db);
        assert!(run_compare(&db, "25034104", 1, 30).is_err());
    }

    #[test]
    fn unfinished_competitor_is_marked_skip() {
        let db = mem_db();
        seed_task(&db);
        seed_clip(&db, "DOWNLOADING");
        let report = run_compare(&db, "25034104", 1, 30).unwrap();
        assert_eq!(report.items.len(), 1);
        assert_eq!(report.items[0].coverage, Coverage::Skip);
        assert_eq!(report.skip_count, 1);
        assert!(report.items[0].reason.contains("未处理完成"));
    }

    #[test]
    fn missing_session_data_marks_all_skip() {
        let db = mem_db();
        seed_task(&db);
        seed_clip(&db, "DONE");
        // 没有 ASR 缓存 → 本场数据缺失
        let report = run_compare(&db, "25034104", 1, 30).unwrap();
        assert_eq!(report.skip_count, 1);
        assert!(report.items[0].reason.contains("识别数据已清理"));
        // 跳过项不计入覆盖率分母
        assert_eq!(report.coverage_rate, 0.0);
    }

    #[test]
    fn coverage_rate_ignores_skipped_items() {
        // 全跳过时覆盖率必须是 0（分母为 0 不能 panic，也不能算成 100%）
        let db = mem_db();
        seed_task(&db);
        seed_clip(&db, "DOWNLOADING");
        seed_clip(&db, "DOWNLOADING");
        let report = run_compare(&db, "25034104", 1, 30).unwrap();
        assert_eq!(report.skip_count, 2);
        assert_eq!(report.coverage_rate, 0.0);
    }

    #[test]
    fn report_is_persisted_and_reloadable() {
        let db = mem_db();
        seed_task(&db);
        seed_clip(&db, "DOWNLOADING");
        run_compare(&db, "25034104", 1, 30).unwrap();
        let loaded = load_report(&db, 1).unwrap();
        // 没有 source_file_path 时报告落盘路径为空，但索引仍应写入
        assert!(loaded.is_none(), "路径为空时报告明细读不回来，属预期");
        let count: i64 = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM competitor_compare WHERE task_id = 1 AND status = 'DONE'",
                    [],
                    |r| r.get(0),
                )
            })
            .unwrap();
        assert_eq!(count, 1, "索引应写入");
    }

    #[test]
    fn rerun_voids_previous_report() {
        let db = mem_db();
        seed_task(&db);
        seed_clip(&db, "DOWNLOADING");
        run_compare(&db, "25034104", 1, 30).unwrap();
        run_compare(&db, "25034104", 1, 30).unwrap();
        let voided: i64 = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM competitor_compare WHERE task_id = 1 AND status = 'VOID'",
                    [],
                    |r| r.get(0),
                )
            })
            .unwrap();
        assert_eq!(voided, 1, "旧报告应标记作废");
    }
}
