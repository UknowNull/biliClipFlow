//! 录播自动精品切片流水线（方案见 docs/录播自动精品切片-需求与开发方案.md）。
//!
//! 召回靠信号（弹幕、付费、响度、语音事件），精排靠理解（文本大模型），边界靠代码，结果靠人工标记。
//! 任务单线程串行执行，避免与录制争抢 CPU。

pub mod asr;
pub mod boundary;
pub mod cutter;
pub mod danmaku;
pub mod llm;
pub mod settings;
pub mod signals;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use futures_util::stream::{self, StreamExt};
use serde_json::json;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

use crate::commands::anchor::{
    load_record_segments, normalize_segments_for_analysis, resolve_clips_dir, LiveRecordSegment,
};
use crate::db::Db;
use crate::utils::{append_log, now_rfc3339, sanitize_filename};

use asr::{AudioFile, FullPass};
use boundary::Range;
use danmaku::DanmakuSource;
use llm::{Judgement, WindowInput};
use signals::{Candidate, Profile, Span};

const ASR_CACHE_VERSION: &str = "sv20240717-v1";
const AUTO_SCAN_SECS: u64 = 300;
const AUTO_LOOKBACK_DAYS: i64 = 3;

pub struct ClipContext {
    pub db: Arc<Db>,
    pub app_log_path: Arc<PathBuf>,
    pub app_data_dir: PathBuf,
}

static QUEUE: OnceLock<UnboundedSender<i64>> = OnceLock::new();

/// 语音模型目录：配置为空时用应用数据目录下 models/asr
pub fn model_dir(app_data_dir: &Path, asr: &settings::AsrSettings) -> PathBuf {
    let custom = asr.local_model_dir.trim();
    if custom.is_empty() {
        app_data_dir.join("models").join("asr")
    } else {
        PathBuf::from(custom)
    }
}

/// 启动任务队列与自动触发；把上次未完成的任务重新入队
pub fn start(ctx: ClipContext) {
    let ctx = Arc::new(ctx);
    let (tx, mut rx) = unbounded_channel::<i64>();
    let _ = QUEUE.set(tx);
    let worker = Arc::clone(&ctx);
    tauri::async_runtime::spawn(async move {
        while let Some(task_id) = rx.recv().await {
            run_task_guarded(&worker, task_id).await;
        }
    });
    let pending: Vec<i64> = ctx
        .db
        .with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT id FROM live_clip_task WHERE status IN ('QUEUED', 'RUNNING') ORDER BY id")?;
            let rows = stmt.query_map([], |row| row.get(0))?;
            rows.collect()
        })
        .unwrap_or_default();
    for id in pending {
        enqueue(id);
    }
    let scanner = Arc::clone(&ctx);
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(AUTO_SCAN_SECS)).await;
            if let Err(err) = auto_trigger(&scanner) {
                append_log(&scanner.app_log_path, &format!("auto_clip_scan_failed err={}", err));
            }
        }
    });
}

pub fn enqueue(task_id: i64) {
    if let Some(tx) = QUEUE.get() {
        let _ = tx.send(task_id);
    }
}

/// 新建切片任务并入队。同一批录播的旧切片会在任务运行时清理。
pub fn create_task(db: &Db, room_id: &str, record_ids: &[i64], trigger: &str, session_key: Option<&str>) -> Result<i64, String> {
    let ids_json = serde_json::to_string(record_ids).unwrap_or_else(|_| "[]".into());
    let first_start: String = db
        .with_conn(|conn| {
            conn.query_row(
                &format!(
                    "SELECT MIN(start_time) FROM live_record_task WHERE id IN ({})",
                    record_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",")
                ),
                [],
                |row| row.get::<_, Option<String>>(0),
            )
        })
        .ok()
        .flatten()
        .unwrap_or_default();
    let date_label = DateTime::parse_from_rfc3339(&first_start)
        .map(|d| d.with_timezone(&Utc).format("%Y-%m-%d").to_string())
        .unwrap_or_default();
    let now = now_rfc3339();
    let id = db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO live_clip_task (room_id, record_ids, date_label, status, clip_count, error_message, create_time, update_time, trigger_type, session_key, stage, progress) \
                 VALUES (?1, ?2, ?3, 'QUEUED', 0, NULL, ?4, ?4, ?5, ?6, 'QUEUED', 0)",
                (room_id, &ids_json, &date_label, &now, trigger, session_key),
            )?;
            Ok(conn.last_insert_rowid())
        })
        .map_err(|e| format!("创建切片任务失败: {}", e))?;
    enqueue(id);
    Ok(id)
}

fn set_stage(db: &Db, task_id: i64, stage: &str, progress: f64) {
    let now = now_rfc3339();
    let _ = db.with_conn(|conn| {
        conn.execute(
            "UPDATE live_clip_task SET status = 'RUNNING', stage = ?1, progress = ?2, update_time = ?3 WHERE id = ?4",
            (stage, (progress * 10.0).round() / 10.0, &now, task_id),
        )
    });
}

async fn run_task_guarded(ctx: &Arc<ClipContext>, task_id: i64) {
    append_log(&ctx.app_log_path, &format!("auto_clip_task_start task_id={}", task_id));
    let result = run_task(ctx, task_id).await;
    let now = now_rfc3339();
    let (status, count, message) = match &result {
        Ok((count, warnings)) => ("COMPLETED", *count as i64, (!warnings.is_empty()).then(|| format!("提示：{}", warnings.join("；")))),
        Err(err) => ("FAILED", 0, Some(err.clone())),
    };
    let _ = ctx.db.with_conn(|conn| {
        conn.execute(
            "UPDATE live_clip_task SET status = ?1, clip_count = ?2, error_message = ?3, stage = ?4, progress = ?5, update_time = ?6 WHERE id = ?7",
            (status, count, message.as_deref(), if status == "COMPLETED" { "DONE" } else { "FAILED" }, if status == "COMPLETED" { 100.0 } else { 0.0 }, &now, task_id),
        )
    });
    append_log(&ctx.app_log_path, &format!("auto_clip_task_end task_id={} status={} clips={} msg={}", task_id, status, count, message.unwrap_or_default()));
}

#[derive(Clone)]
struct Picked {
    cand: Candidate,
    judgement: Option<Judgement>,
    ai_status: &'static str,
    range: Range,
    score: f64,
    streamer_ratio: Option<f64>,
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| format!("后台任务异常: {}", e))?
}

async fn run_task(ctx: &Arc<ClipContext>, task_id: i64) -> Result<(usize, Vec<String>), String> {
    let db = Arc::clone(&ctx.db);
    let (room_id, ids_json): (String, String) = db
        .with_conn(|conn| conn.query_row("SELECT room_id, record_ids FROM live_clip_task WHERE id = ?1", [task_id], |r| Ok((r.get(0)?, r.get(1)?))))
        .map_err(|e| format!("读取切片任务失败: {}", e))?;
    let record_ids: Vec<i64> = serde_json::from_str(&ids_json).unwrap_or_default();
    set_stage(&db, task_id, "PREPARE", 0.0);
    let cfg = settings::load(&db);
    let mut warnings: Vec<String> = Vec::new();

    let segs: Vec<LiveRecordSegment> = {
        let (db, room, ids, log) = (Arc::clone(&db), room_id.clone(), record_ids.clone(), Arc::clone(&ctx.app_log_path));
        blocking(move || Ok(normalize_segments_for_analysis(load_record_segments(&db, &room, &ids)?, Some(log.as_path())))).await?
    };
    if segs.is_empty() {
        return Err("未找到录播文件".into());
    }
    let base_time = segs.iter().map(|s| s.start_time).min().unwrap();
    let session_secs = segs.iter().map(|s| s.offset_end).max().unwrap_or(0) as f64;
    let spans: Vec<Span> = segs.iter().map(|s| Span { start: s.offset_start as f64, end: s.offset_end as f64 }).collect();
    let (room_title, profile) = room_meta(&db, &room_id, record_ids.first().copied());

    // 弹幕
    let sources: Vec<DanmakuSource> = segs.iter().map(|s| DanmakuSource { video_path: &s.file_path, offset_start: s.offset_start as f64 }).collect();
    let (events, has_danmaku) = danmaku::load_events(&sources, base_time);
    if !has_danmaku {
        warnings.push("未找到弹幕文件，已只按语音信号挑选".into());
    }

    // 整场语音识别（带缓存）
    let clips_dir = resolve_clips_dir(&segs[0].file_path)?;
    let mdir = model_dir(&ctx.app_data_dir, &cfg.asr);
    let mut pass: Option<FullPass> = None;
    if cfg.asr.mode == "local" && asr::models_ready(&mdir) {
        set_stage(&db, task_id, "ASR", 1.0);
        let cache = clips_dir.join(".cache").join(format!("asr_{}_{}.json", ASR_CACHE_VERSION, record_ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join("-")));
        let files: Vec<(String, f64)> = segs.iter().map(|s| (s.file_path.clone(), s.offset_start as f64)).collect();
        let (db2, mdir2, threads) = (Arc::clone(&db), mdir.clone(), cfg.asr.threads);
        pass = Some(
            blocking(move || {
                if let Some(cached) = std::fs::read(&cache).ok().and_then(|b| serde_json::from_slice::<FullPass>(&b).ok()) {
                    return Ok(cached);
                }
                let audio: Vec<AudioFile> = files.iter().map(|(p, o)| AudioFile { path: p, offset_start: *o }).collect();
                let mut last = 0.0;
                let result = asr::full_pass(&mdir2, threads, &audio, session_secs, &|| false, &mut |done| {
                    let pct = 1.0 + 59.0 * (done / session_secs.max(1.0)).min(1.0);
                    if pct - last >= 1.0 {
                        last = pct;
                        set_stage(&db2, task_id, "ASR", pct);
                    }
                })?;
                if let Some(parent) = cache.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(&cache, serde_json::to_vec(&result).unwrap_or_default());
                Ok(result)
            })
            .await?,
        );
    } else if cfg.asr.mode == "local" {
        warnings.push("未下载语音识别模型，已跳过语音信号与 AI 精排，可在设置 > 自动切片中下载".into());
    }
    if pass.is_none() && !has_danmaku {
        return Err("既没有弹幕也没有可用的语音识别模型，无法分析".into());
    }

    // 候选召回
    set_stage(&db, task_id, "CANDIDATES", 60.0);
    let candidates = signals::pick_candidates(session_secs, &spans, &events, has_danmaku, pass.as_ref(), profile, &cfg.clip);
    append_log(&ctx.app_log_path, &format!("auto_clip_candidates task_id={} count={} danmaku_events={} asr_segments={}", task_id, candidates.len(), events.len(), pass.as_ref().map(|p| p.segments.len()).unwrap_or(0)));
    if candidates.is_empty() {
        remove_previous_clips(&db, task_id, &room_id, &ids_json);
        return Ok((0, warnings));
    }

    // 候选窗口内的主播中文段用 Paraformer 重新识别
    if let Some(p) = pass.take() {
        set_stage(&db, task_id, "TRANSCRIBE", 62.0);
        let windows: Vec<(f64, f64)> = candidates.iter().map(|c| (c.start, c.end)).collect();
        let files: Vec<(String, f64)> = segs.iter().map(|s| (s.file_path.clone(), s.offset_start as f64)).collect();
        let (mdir2, threads) = (mdir.clone(), cfg.asr.threads);
        pass = Some(
            blocking(move || {
                let mut p = p;
                let targets: Vec<usize> = (0..p.segments.len())
                    .filter(|&i| {
                        let s = &p.segments[i];
                        s.is_streamer() && windows.iter().any(|(a, b)| s.end > *a && s.start < *b)
                    })
                    .collect();
                let audio: Vec<AudioFile> = files.iter().map(|(path, o)| AudioFile { path, offset_start: *o }).collect();
                asr::refine_chinese(&mdir2, threads, &audio, &mut p.segments, &targets)?;
                Ok(p)
            })
            .await?,
        );
    }

    // 大模型精排
    let mut judgements: Vec<Option<Judgement>> = vec![None; candidates.len()];
    let mut statuses: Vec<&'static str> = vec!["SIGNAL_ONLY"; candidates.len()];
    let mut rank_bonus: Vec<f64> = vec![0.0; candidates.len()];
    let llm_wanted = cfg.clip.use_llm;
    if llm_wanted && !cfg.llm.is_configured() {
        warnings.push("未配置大模型，标题使用模板".into());
    }
    if let (true, true, Some(p)) = (llm_wanted, cfg.llm.is_configured(), pass.as_ref()) {
        set_stage(&db, task_id, "AI", 70.0);
        let mode = if cfg.llm.structured_output == "auto" {
            let t = llm::test_connection(&cfg.llm).await;
            if t.ok { Some(t.mode) } else {
                warnings.push(format!("大模型不可用（{}），已按信号排序", t.message.chars().take(80).collect::<String>()));
                None
            }
        } else {
            Some(cfg.llm.structured_output.clone())
        };
        if let Some(mode) = mode {
            // 每个请求持有自己的数据，避免并发闭包借用局部变量
            let mut plains: Vec<String> = Vec::with_capacity(candidates.len());
            let mut jobs: Vec<(usize, String, Vec<(String, usize)>, String)> = Vec::with_capacity(candidates.len());
            for (i, c) in candidates.iter().enumerate() {
                let (lines, plain) = window_transcript(&p.segments, c.start, c.end);
                plains.push(plain);
                jobs.push((i, lines, signals::top_danmaku(&events, c.start, c.end, 30), c.signals.summary()));
            }
            let llm_cfg = Arc::new(cfg.llm.clone());
            let shared_mode = Arc::new(mode.clone());
            let shared_title = Arc::new(room_title.clone());
            let done = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let total = candidates.len();
            let titles = cfg.clip.title_candidates;
            let concurrency = cfg.llm.max_concurrency;
            let results: Vec<(usize, Result<Judgement, String>)> = stream::iter(jobs)
                .map(|(i, lines, dm, summary)| {
                    let (llm_cfg, mode, title, db, done) = (Arc::clone(&llm_cfg), Arc::clone(&shared_mode), Arc::clone(&shared_title), Arc::clone(&db), Arc::clone(&done));
                    async move {
                        let input = WindowInput { room_title: &title, transcript: &lines, danmaku: &dm, signal_summary: &summary };
                        let r = llm::judge(&llm_cfg, &mode, profile, titles, &input).await;
                        let n = done.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                        set_stage(&db, task_id, "AI", 70.0 + 15.0 * n as f64 / total as f64);
                        (i, r)
                    }
                })
                .buffer_unordered(concurrency)
                .collect()
                .await;
            for (i, r) in results {
                match r {
                    Ok(j) if !llm::evidence_valid(&j.evidence, &plains[i]) => {
                        statuses[i] = "UNVERIFIED";
                        append_log(&ctx.app_log_path, &format!("auto_clip_evidence_invalid task_id={} cand={} evidence={:?}", task_id, i, j.evidence));
                    }
                    Ok(j) => {
                        statuses[i] = if j.keep { "KEPT" } else { "REJECTED" };
                        judgements[i] = Some(j);
                    }
                    Err(e) => {
                        statuses[i] = "ERROR";
                        append_log(&ctx.app_log_path, &format!("auto_clip_llm_error task_id={} cand={} err={}", task_id, i, e));
                    }
                }
            }
            let kept: Vec<(usize, String, String)> = (0..candidates.len())
                .filter(|&i| statuses[i] == "KEPT")
                .map(|i| {
                    let j = judgements[i].as_ref().unwrap();
                    (i, j.titles.first().cloned().unwrap_or_default(), j.reason.clone())
                })
                .collect();
            if let Ok(order) = llm::rank(&cfg.llm, &mode, &kept).await {
                let n = order.len().max(1) as f64;
                for (pos, id) in order.iter().enumerate() {
                    if let Some(b) = rank_bonus.get_mut(*id) {
                        *b = 0.05 * (n - pos as f64) / n;
                    }
                }
            }
        }
    }

    // 边界、门槛、去重
    let empty = Vec::new();
    let asr_segs = pass.as_ref().map(|p| &p.segments).unwrap_or(&empty);
    let mut picked: Vec<Picked> = Vec::new();
    let mut dropped_ratio = 0;
    let llm_failed = statuses.iter().filter(|s| **s == "ERROR").count();
    if llm_failed > 0 {
        warnings.push(format!("{} 个候选大模型精排失败，未生成切片", llm_failed));
    }
    for (i, cand) in candidates.iter().enumerate() {
        // 精排失败的候选质量不可控，不出片
        let factor = match statuses[i] {
            "KEPT" | "SIGNAL_ONLY" => 1.0,
            "REJECTED" | "ERROR" => 0.0,
            _ => 0.6,
        };
        if factor == 0.0 {
            continue;
        }
        let Some(span) = spans.iter().copied().find(|s| cand.peak >= s.start && cand.peak < s.end) else { continue };
        let proposed = match judgements[i].as_ref() {
            Some(j) if statuses[i] == "KEPT" && j.end > j.start => Range { start: j.start.max(cand.start - 60.0), end: j.end.min(cand.end + 60.0) },
            _ => Range { start: cand.start, end: cand.end },
        };
        let mut range = boundary::finalize(proposed, cand.peak, span, asr_segs, cfg.clip.min_duration_secs as f64, cfg.clip.max_duration_secs as f64);
        if let Some(j) = judgements[i].as_ref().filter(|_| statuses[i] == "KEPT") {
            range = boundary::cover(range, &locate_evidence(&j.evidence, asr_segs, cand.start, cand.end), span, cfg.clip.max_duration_secs as f64);
        }
        let ratio = pass.as_ref().map(|p| boundary::streamer_ratio(range, &p.segments));
        if profile == Profile::Reaction {
            if let Some(r) = ratio {
                if r * 100.0 < cfg.clip.min_streamer_speech_percent {
                    dropped_ratio += 1;
                    continue;
                }
            }
        }
        picked.push(Picked {
            cand: cand.clone(),
            judgement: if statuses[i] == "KEPT" { judgements[i].clone() } else { None },
            ai_status: statuses[i],
            range,
            score: cand.score * factor + rank_bonus[i],
            streamer_ratio: ratio,
        });
    }
    if dropped_ratio > 0 {
        warnings.push(format!("{} 个候选主播解说占比不足被淘汰", dropped_ratio));
    }
    let keep_idx = boundary::dedupe(&picked.iter().map(|p| (p.range, p.score)).collect::<Vec<_>>());
    let picked: Vec<Picked> = keep_idx.into_iter().map(|i| picked[i].clone()).collect();

    // 剪辑
    set_stage(&db, task_id, "CUT", 85.0);
    remove_previous_clips(&db, task_id, &room_id, &ids_json);
    std::fs::create_dir_all(&clips_dir).map_err(|e| format!("创建切片目录失败: {}", e))?;
    let total = picked.len();
    let mut made = 0;
    for (idx, p) in picked.into_iter().enumerate() {
        let Some(seg) = segs.iter().find(|s| p.range.start >= s.offset_start as f64 && p.range.start < s.offset_end as f64) else { continue };
        let title = p
            .judgement
            .as_ref()
            .and_then(|j| j.titles.first().cloned())
            .unwrap_or_else(|| format!("{} 高光 {}", room_title, hms(p.range.start, ":")));
        let name = format!("clip_{:03}_{}_{}.mp4", idx + 1, hms(p.range.start, "-"), sanitize_filename(&title.chars().take(24).collect::<String>()));
        let output = clips_dir.join(name);
        let (source, start_in_file, duration, out2) = (seg.file_path.clone(), p.range.start - seg.offset_start as f64, p.range.len(), output.clone());
        let cut_result = blocking(move || cutter::cut(&source, start_in_file, duration, &out2)).await;
        if let Err(e) = &cut_result {
            append_log(&ctx.app_log_path, &format!("auto_clip_cut_failed task_id={} err={}", task_id, e));
        }
        insert_item(&db, task_id, &room_id, seg, &output, &p, &title, cut_result.is_ok());
        made += cut_result.is_ok() as usize;
        set_stage(&db, task_id, "CUT", 85.0 + 15.0 * (idx + 1) as f64 / total.max(1) as f64);
    }
    Ok((made, warnings))
}

fn hms(secs: f64, sep: &str) -> String {
    let s = secs.max(0.0) as i64;
    format!("{:02}{sep}{:02}{sep}{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

/// 在候选窗口内找出每条证据所在的句子，返回其起止秒；找不到的证据忽略
fn locate_evidence(evidence: &[String], segments: &[asr::AsrSegment], start: f64, end: f64) -> Vec<(f64, f64)> {
    let window: Vec<&asr::AsrSegment> = segments.iter().filter(|s| s.end > start && s.start < end).collect();
    evidence
        .iter()
        .filter_map(|e| {
            window
                .iter()
                .find(|s| llm::evidence_valid(std::slice::from_ref(e), &format!("{}{}", s.best_text(), s.text)))
                .map(|s| (s.start, s.end))
        })
        .collect()
}

/// 每行：[起止秒][说话方|情绪/事件] 文本；同时返回纯文本用于证据校验
fn window_transcript(segments: &[asr::AsrSegment], start: f64, end: f64) -> (String, String) {
    let mut lines = Vec::new();
    let mut plain = String::new();
    for s in segments.iter().filter(|s| s.end > start && s.start < end) {
        let speaker = if s.is_streamer() { "主播" } else { "原片" };
        let extra: Vec<&str> = [s.emotion.as_str(), s.event.as_str()]
            .into_iter()
            .filter(|x| !x.is_empty() && *x != "EMO_UNKNOWN" && *x != "NEUTRAL" && *x != "Speech")
            .collect();
        let tag = if extra.is_empty() { speaker.to_string() } else { format!("{}|{}", speaker, extra.join("/")) };
        let text = s.best_text();
        lines.push(format!("[{:.1}-{:.1}s][{}] {}", s.start, s.end, tag, text));
        plain.push_str(text);
        plain.push_str(&s.text);
    }
    (lines.join("\n"), plain)
}

fn room_meta(db: &Db, room_id: &str, first_record: Option<i64>) -> (String, Profile) {
    let title = first_record
        .and_then(|id| db.with_conn(|c| c.query_row("SELECT title FROM live_record_task WHERE id = ?1", [id], |r| r.get::<_, Option<String>>(0))).ok().flatten())
        .unwrap_or_default();
    let profile = db
        .with_conn(|c| c.query_row("SELECT clip_profile FROM live_room_settings WHERE room_id = ?1", [room_id], |r| r.get::<_, Option<String>>(0)))
        .ok()
        .flatten()
        .unwrap_or_default();
    (title, Profile::parse(&profile))
}

/// 删除同一批录播此前任务产生的切片文件与记录（重跑时替换旧结果）
fn remove_previous_clips(db: &Db, task_id: i64, room_id: &str, ids_json: &str) {
    let old: Vec<(i64, String)> = db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT i.id, i.file_path FROM live_clip_item i JOIN live_clip_task t ON t.id = i.task_id \
                 WHERE t.room_id = ?1 AND t.record_ids = ?2 AND t.id <> ?3",
            )?;
            let rows = stmt.query_map((room_id, ids_json, task_id), |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect()
        })
        .unwrap_or_default();
    for (id, path) in old {
        let _ = std::fs::remove_file(&path);
        let _ = db.with_conn(|c| c.execute("DELETE FROM live_clip_item WHERE id = ?1", [id]));
    }
}

#[allow(clippy::too_many_arguments)]
fn insert_item(db: &Db, task_id: i64, room_id: &str, seg: &LiveRecordSegment, output: &Path, p: &Picked, title: &str, ok: bool) {
    let j = p.judgement.as_ref();
    let titles = j.map(|j| j.titles.clone()).unwrap_or_else(|| vec![title.to_string()]);
    let signals = json!({
        "snapshot": p.cand.signals,
        "summary": p.cand.signals.summary(),
        "signalScore": p.cand.score,
        "peak": p.cand.peak,
        "streamerRatio": p.streamer_ratio,
    });
    let now = now_rfc3339();
    let _ = db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO live_clip_item (task_id, room_id, source_file_path, status, file_path, start_offset, end_offset, duration, peak_count, create_time, \
             score, signals, category, titles, description, tags, cover_text, reason, evidence, ai_status, mark) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, 'PENDING')",
            rusqlite::params![
                task_id,
                room_id,
                seg.file_path,
                if ok { "SUCCESS" } else { "FAILED" },
                output.to_string_lossy().to_string(),
                p.range.start.round() as i64,
                p.range.end.round() as i64,
                p.range.len().round() as i64,
                p.cand.signals.max_repeat as i64,
                now,
                (p.score * 1000.0).round() / 1000.0,
                signals.to_string(),
                j.map(|j| j.category.clone()),
                serde_json::to_string(&titles).unwrap_or_default(),
                j.map(|j| j.description.clone()),
                j.map(|j| j.tags.join(",")),
                j.map(|j| j.cover_text.clone()),
                j.map(|j| j.reason.clone()),
                j.map(|j| serde_json::to_string(&j.evidence).unwrap_or_default()),
                p.ai_status,
            ],
        )
    });
}

/// 扫描最近几天已结束的场次，为尚未处理的场次创建自动切片任务
fn auto_trigger(ctx: &ClipContext) -> Result<(), String> {
    let cfg = settings::load(&ctx.db);
    if !cfg.clip.enabled {
        return Ok(());
    }
    let now = Utc::now();
    let since = (now - ChronoDuration::days(AUTO_LOOKBACK_DAYS)).to_rfc3339();
    let rows: Vec<(i64, String, String, String, Option<String>, Option<String>)> = ctx
        .db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, room_id, status, start_time, end_time, remux_status FROM live_record_task \
                 WHERE start_time >= ?1 ORDER BY room_id, start_time",
            )?;
            let rows = stmt.query_map([&since], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?;
            rows.collect()
        })
        .map_err(|e| e.to_string())?;
    let gap = ChronoDuration::minutes(cfg.clip.session_gap_minutes);
    let parse = |s: &str| DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc));
    let mut sessions: Vec<(String, Vec<i64>, bool, DateTime<Utc>)> = Vec::new();
    for (id, room, status, start, end, remux) in rows {
        let Some(start) = parse(&start) else { continue };
        let end = end.as_deref().and_then(parse).unwrap_or(start);
        let ready = status != "RECORDING" && matches!(remux.as_deref(), Some("DONE") | Some("FAILED"));
        match sessions.last_mut() {
            Some((r, ids, all_ready, last_end)) if *r == room && start - *last_end <= gap => {
                ids.push(id);
                *all_ready &= ready;
                *last_end = (*last_end).max(end);
            }
            _ => sessions.push((room, vec![id], ready, end)),
        }
    }
    let existing: HashSet<String> = ctx
        .db
        .with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT session_key FROM live_clip_task WHERE session_key IS NOT NULL")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect()
        })
        .unwrap_or_default();
    let enabled_since = parse(&cfg.clip.enabled_since).unwrap_or(now);
    for (room, ids, ready, last_end) in sessions {
        let key = format!("{}:{}", room, ids[0]);
        if !ready || now - last_end <= gap || last_end < enabled_since || existing.contains(&key) {
            continue;
        }
        let id = create_task(&ctx.db, &room, &ids, "AUTO", Some(&key))?;
        append_log(&ctx.app_log_path, &format!("auto_clip_task_created task_id={} session={} records={}", id, key, ids.len()));
    }
    Ok(())
}
