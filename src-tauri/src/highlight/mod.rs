//! 录播自动精品切片流水线（方案见 docs/录播自动精品切片-需求与开发方案.md）。
//!
//! 有大模型时整场转写按话题切分、弹幕作标注、多维打分；没有时按信号峰值切。边界只对齐句子，结果靠人工挑选。
//! 任务单线程串行执行，避免与录制争抢 CPU。

pub mod asr;
pub mod boundary;
pub mod cutter;
pub mod danmaku;
pub mod llm;
pub mod settings;
pub mod signals;
pub mod speaker;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use rusqlite::OptionalExtension;
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
use danmaku::{DanmakuEvent, DanmakuSource};
use llm::{ChunkInput, TopicClip};
use signals::{Candidate, Profile, Span};

// v2：按逐字时间拆成单句
// v3：srt_pass 不再合并短句，段数与旧缓存（合并版）对不上，整体换版本作废旧缓存
const ASR_CACHE_VERSION: &str = "sv20240717-v3";
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
    // 同一批录播已有排队或运行中的任务时直接复用，避免重复点击产生重复任务
    let active: Option<i64> = db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT id FROM live_clip_task WHERE room_id = ?1 AND record_ids = ?2 AND status IN ('QUEUED', 'RUNNING') ORDER BY id LIMIT 1",
                (room_id, &ids_json),
                |row| row.get(0),
            )
        })
        .ok();
    if let Some(id) = active {
        return Ok(id);
    }
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
    range: Range,
    clip: Option<TopicClip>,
    ai_status: &'static str,
    score: f64,
    /// 重叠切片去重时保留该值大的
    dedupe_key: f64,
    signals: serde_json::Value,
    peak_count: i64,
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
    let (room_title, profile, streamer) = room_meta(&db, &room_id, record_ids.first().copied());

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
    let srt = {
        let files: Vec<AudioFile> = segs.iter().map(|s| AudioFile { path: &s.file_path, offset_start: s.offset_start as f64 }).collect();
        asr::srt_pass(&files)
    };
    if let Some(p) = srt {
        // 每段录播都有 SRT：直接用作转写，写进识别缓存（评阅页读同一份）
        let cache = clips_dir.join(".cache").join(format!("asr_{}_{}.json", ASR_CACHE_VERSION, record_ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join("-")));
        if let Some(parent) = cache.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&cache, serde_json::to_vec(&p).unwrap_or_default());
        append_log(&ctx.app_log_path, &format!("auto_clip_srt task_id={} segments={}", task_id, p.segments.len()));
        pass = Some(p);
    } else if cfg.asr.mode == "local" && asr::models_ready(&mdir) {
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
    // 已登记主播声纹：整场逐句判定主播说话时长（带缓存），供候选召回、主播占比与转写标注使用
    let voiceprint = room_voiceprint(&db, &room_id).filter(|_| speaker::model_ready(&mdir) && pass.is_some());
    let speaker_key = voiceprint.as_deref().map(speaker::voiceprint_key).unwrap_or_else(|| "lang".into());
    if let Some(vp) = voiceprint {
        let p = pass.take().unwrap();
        set_stage(&db, task_id, "SPEAKER", 60.0);
        let ids_key = record_ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join("-");
        let cache = clips_dir.join(".cache").join(format!("speaker_{}_{}_{}.json", speaker::voiceprint_key(&vp), ASR_CACHE_VERSION, ids_key));
        let files: Vec<(String, f64)> = segs.iter().map(|s| (s.file_path.clone(), s.offset_start as f64)).collect();
        let (mdir2, threads) = (mdir.clone(), cfg.asr.threads);
        pass = Some(
            blocking(move || {
                let mut p = p;
                let cached = std::fs::read(&cache).ok().and_then(|b| serde_json::from_slice::<Vec<Option<f64>>>(&b).ok());
                match cached.filter(|c| c.len() == p.segments.len()) {
                    Some(secs) => p.segments.iter_mut().zip(secs).for_each(|(s, v)| s.streamer_secs = v),
                    None => {
                        let audio: Vec<AudioFile> = files.iter().map(|(path, o)| AudioFile { path, offset_start: *o }).collect();
                        let all: Vec<usize> = (0..p.segments.len()).collect();
                        speaker::label_streamer(&mdir2, threads, &audio, &mut p.segments, &all, &vp)?;
                        let secs: Vec<Option<f64>> = p.segments.iter().map(|s| s.streamer_secs).collect();
                        let _ = std::fs::write(&cache, serde_json::to_vec(&secs).unwrap_or_default());
                    }
                }
                Ok(p)
            })
            .await?,
        );
    }
    if pass.is_none() && !has_danmaku {
        return Err("既没有弹幕也没有可用的语音识别模型，无法分析".into());
    }

    if cfg.clip.use_llm && !cfg.llm.is_configured() {
        warnings.push("未配置大模型，已按信号切片，标题使用模板".into());
    }
    let mut mode: Option<String> = None;
    if cfg.clip.use_llm && cfg.llm.is_configured() && pass.is_some() {
        mode = if cfg.llm.structured_output == "auto" {
            let t = llm::test_connection(&cfg.llm).await;
            if !t.ok {
                warnings.push(format!("大模型不可用（{}），已按信号切片", t.message.chars().take(80).collect::<String>()));
            }
            t.ok.then_some(t.mode)
        } else {
            Some(cfg.llm.structured_output.clone())
        };
    }

    let picked: Vec<Picked> = match (mode, pass.take()) {
        // 有大模型：整场按话题切分，弹幕只作标注
        (Some(mode), Some(p)) => {
            set_stage(&db, task_id, "TRANSCRIBE", 62.0);
            let ids_key = record_ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join("-");
            let cache = clips_dir.join(".cache").join(format!("refine_{}_{}_{}.json", ASR_CACHE_VERSION, ids_key, speaker_key));
            let files: Vec<(String, f64)> = segs.iter().map(|s| (s.file_path.clone(), s.offset_start as f64)).collect();
            let (mdir2, threads) = (mdir.clone(), cfg.asr.threads);
            let p = blocking(move || {
                let mut p = p;
                let cached = std::fs::read(&cache).ok().and_then(|b| serde_json::from_slice::<Vec<Option<String>>>(&b).ok());
                let cached = cached.filter(|c| c.len() == p.segments.len());
                let mut changed = cached.is_none();
                let mut texts = match cached {
                    Some(texts) => texts,
                    None => {
                        // 已有中文文本（来自 SRT）的不再二次识别
                        let targets: Vec<usize> = (0..p.segments.len()).filter(|&i| p.segments[i].is_streamer() && p.segments[i].zh_text.is_none()).collect();
                        let audio: Vec<AudioFile> = files.iter().map(|(path, o)| AudioFile { path, offset_start: *o }).collect();
                        asr::refine_chinese(&mdir2, threads, &audio, &mut p.segments, &targets)?;
                        p.segments.iter().map(|s| s.zh_text.clone()).collect()
                    }
                };
                // 新结果与没有标点的旧缓存补标点后写回，评阅页读同一份缓存
                let has_punct = texts.iter().flatten().any(|t| t.contains(['，', '。', '？', '！']));
                if !has_punct && asr::punctuate(&mdir2, &mut texts) {
                    changed = true;
                }
                if changed {
                    let _ = std::fs::write(&cache, serde_json::to_vec(&texts).unwrap_or_default());
                }
                p.segments.iter_mut().zip(texts).for_each(|(s, t)| s.zh_text = t);
                Ok(p)
            })
            .await?;
            let mut p = p;
            apply_subtitle_edits(&mut p.segments, &load_subtitle_edits(&clips_dir.join(".cache"), &ids_key));
            let scs: Vec<(f64, String)> = events.iter().filter(|e| e.kind == danmaku::DanmakuKind::SuperChat).map(|e| (e.t, e.text.clone())).collect();
            boundary::mark_sc_readings(&mut p.segments, &scs);
            topic_clips(ctx, task_id, &cfg, &mode, profile, &room_title, &streamer, &p, &events, has_danmaku, session_secs, &spans, &mut warnings).await?
        }
        (_, p) => {
            set_stage(&db, task_id, "CANDIDATES", 60.0);
            let candidates = signals::pick_candidates(session_secs, &spans, &events, has_danmaku, p.as_ref(), profile, &cfg.clip);
            append_log(&ctx.app_log_path, &format!("auto_clip_candidates task_id={} count={} danmaku_events={} asr_segments={}", task_id, candidates.len(), events.len(), p.as_ref().map(|p| p.segments.len()).unwrap_or(0)));
            signal_clips(&candidates, &spans, p.as_ref(), &cfg.clip)
        }
    };
    let keep_idx = boundary::dedupe(&picked.iter().map(|p| (p.range, p.dedupe_key)).collect::<Vec<_>>());
    let mut picked: Vec<Picked> = keep_idx.into_iter().map(|i| picked[i].clone()).collect();
    grade_by_rank(&mut picked);

    // 剪辑
    set_stage(&db, task_id, "CUT", 85.0);
    remove_previous_clips(&db, task_id, &room_id, &ids_json);
    std::fs::create_dir_all(&clips_dir).map_err(|e| format!("创建切片目录失败: {}", e))?;
    let total = picked.len();
    let mut made = 0;
    let meta = submission_meta(&db, &room_id, &room_title, base_time);
    for (idx, p) in picked.into_iter().enumerate() {
        let Some(seg) = segs.iter().find(|s| p.range.start >= s.offset_start as f64 && p.range.start < s.offset_end as f64) else { continue };
        let title = p
            .clip
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
        insert_item(&db, task_id, &room_id, seg, &output, &p, &title, &meta, cut_result.is_ok());
        made += cut_result.is_ok() as usize;
        set_stage(&db, task_id, "CUT", 85.0 + 15.0 * (idx + 1) as f64 / total.max(1) as f64);
    }
    Ok((made, warnings))
}

/// 每块转写的时长与步长：块间重叠 5 分钟，跨块的话题至少在一块里是完整的
const CHUNK_SECS: f64 = 900.0;
const CHUNK_STEP_SECS: f64 = 600.0;
/// 短于该秒数的话题不出片
const MIN_TOPIC_SECS: f64 = 15.0;
/// 精修后允许更短：一句金句几秒就说完，不必凑时长
/// 短于此时长的切片同档内排在后面（多是只回一两句的短回应）
const SHORT_CLIP_SECS: f64 = 30.0;
const REFINED_MIN_SECS: f64 = 8.0;
/// 主播说话占比低于该值的话题不出片：切片以主播发言为主，主播几乎没说话的原片段落再精彩也不要
const MIN_STREAMER_RATIO: f64 = 0.2;

/// 整场转写分块交给大模型按话题切分，弹幕作为标注；起止只对齐到句子边界
#[allow(clippy::too_many_arguments)]
async fn topic_clips(
    ctx: &Arc<ClipContext>,
    task_id: i64,
    cfg: &settings::AutoClipSettings,
    mode: &str,
    profile: Profile,
    room_title: &str,
    streamer: &str,
    p: &FullPass,
    events: &[DanmakuEvent],
    has_danmaku: bool,
    session_secs: f64,
    spans: &[Span],
    warnings: &mut Vec<String>,
) -> Result<Vec<Picked>, String> {
    let per_min = signals::users_per_minute(events, session_secs);
    let mut jobs: Vec<(f64, String, String, String)> = Vec::new();
    let mut a = 0.0;
    while a < session_secs {
        let b = (a + CHUNK_SECS).min(session_secs);
        let (lines, plain) = window_transcript(&p.segments, a, b);
        if !lines.is_empty() {
            let dm = if has_danmaku { signals::danmaku_lines(events, &per_min, a, b) } else { "（无弹幕数据）".to_string() };
            jobs.push((a, lines, plain, dm));
        }
        if b >= session_secs {
            break;
        }
        a += CHUNK_STEP_SECS;
    }
    append_log(&ctx.app_log_path, &format!("auto_clip_chunks task_id={} chunks={} danmaku_events={} asr_segments={}", task_id, jobs.len(), events.len(), p.segments.len()));
    set_stage(&ctx.db, task_id, "AI", 70.0);
    let (llm_cfg, shared_mode, shared_title, shared_streamer) = (Arc::new(cfg.llm.clone()), Arc::new(mode.to_string()), Arc::new(room_title.to_string()), Arc::new(streamer.to_string()));
    let (titles, max_secs, total) = (cfg.clip.title_candidates, cfg.clip.max_duration_secs, jobs.len());
    let done = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let results: Vec<(f64, String, Result<Vec<TopicClip>, String>)> = stream::iter(jobs)
        .map(|(a, lines, plain, dm)| {
            let (llm_cfg, mode, title, streamer, db, done, log) =
                (Arc::clone(&llm_cfg), Arc::clone(&shared_mode), Arc::clone(&shared_title), Arc::clone(&shared_streamer), Arc::clone(&ctx.db), Arc::clone(&done), Arc::clone(&ctx.app_log_path));
            async move {
                let input = ChunkInput { room_title: &title, streamer: &streamer, transcript: &lines, danmaku: &dm, max_secs };
                let mut r = llm::segment_topics(&llm_cfg, &mode, profile, titles, &input).await;
                // 偶发的鉴权失败、输出截断不是 JSON，重试一次
                if let Err(e) = &r {
                    append_log(&log, &format!("auto_clip_llm_retry task_id={} chunk_start={:.0} err={}", task_id, a, e));
                    r = llm::segment_topics(&llm_cfg, &mode, profile, titles, &input).await;
                }
                let n = done.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                set_stage(&db, task_id, "AI", 70.0 + 8.0 * n as f64 / total.max(1) as f64);
                (a, plain, r)
            }
        })
        .buffer_unordered(cfg.llm.max_concurrency)
        .collect()
        .await;

    let chunk_count = results.len();
    let mut failed = 0;
    let mut low_streamer = 0;
    let mut cands = Vec::new();
    for (a, plain, r) in results {
        let clips = match r {
            Ok(c) => c,
            Err(e) => {
                failed += 1;
                append_log(&ctx.app_log_path, &format!("auto_clip_llm_error task_id={} chunk_start={:.0} err={}", task_id, a, e));
                continue;
            }
        };
        for clip in clips {
            let Some(span) = spans.iter().copied().find(|s| clip.start >= s.start && clip.start < s.end) else { continue };
            if clip.end <= clip.start {
                continue;
            }
            let range = boundary::snap(Range { start: clip.start, end: clip.end }, span, &p.segments);
            let range = boundary::fit_sc(range, &p.segments);
            let range = Range { start: range.start.max(span.start), end: range.end };
            if range.len() < MIN_TOPIC_SECS {
                continue;
            }
            // 主播占比按秒计算，夹在原片里的主播插话也计入；大模型常忽略「原片」标注，这里用数据兜底
            let ratio = boundary::streamer_ratio(range, &p.segments);
            if ratio < MIN_STREAMER_RATIO {
                low_streamer += 1;
                append_log(&ctx.app_log_path, &format!("auto_clip_low_streamer task_id={} start={:.0} ratio={:.2} title={:?}", task_id, range.start, ratio, clip.titles.first()));
                continue;
            }
            let verified = llm::evidence_valid(&clip.evidence, &plain);
            if !verified {
                append_log(&ctx.app_log_path, &format!("auto_clip_evidence_invalid task_id={} start={:.0} evidence={:?}", task_id, range.start, clip.evidence));
            }
            cands.push((clip, range, span, verified));
        }
    }

    // 精修：每条候选单独交给大模型，看前后文重新定起止、判断值不值得切并分档
    let chat_times: Vec<f64> = events.iter().filter(|e| matches!(e.kind, danmaku::DanmakuKind::Chat | danmaku::DanmakuKind::SuperChat)).map(|e| e.t).collect();
    let total = cands.len();
    let done = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let refined: Vec<(TopicClip, Range, Span, bool, Option<llm::Refined>)> = stream::iter(cands)
        .map(|(clip, range, span, verified)| {
            let (llm_cfg, mode, streamer, db, done, log) = (Arc::clone(&llm_cfg), Arc::clone(&shared_mode), Arc::clone(&shared_streamer), Arc::clone(&ctx.db), Arc::clone(&done), Arc::clone(&ctx.app_log_path));
            let (idx, lines) = numbered_window(&p.segments, &chat_times, range.start - REFINE_PAD_SECS, range.end + REFINE_PAD_SECS);
            async move {
                let mut r = Err("没有转写".to_string());
                if !idx.is_empty() {
                    let first = idx.iter().position(|&i| p.segments[i].end > range.start).unwrap_or(0);
                    let last = idx.iter().rposition(|&i| p.segments[i].start < range.end).unwrap_or(idx.len() - 1).max(first);
                    let input = llm::RefineInput { streamer: &streamer, topic: clip.titles.first().map(String::as_str).unwrap_or(""), reason: &clip.reason, lines: &lines, proposed: (first, last), last_line: idx.len() - 1 };
                    r = llm::refine_clip(&llm_cfg, &mode, profile, &input).await;
                    if r.is_err() {
                        r = llm::refine_clip(&llm_cfg, &mode, profile, &input).await;
                    }
                }
                let n = done.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                set_stage(&db, task_id, "AI", 78.0 + 7.0 * n as f64 / total.max(1) as f64);
                let refined = match r {
                    Ok(x) if x.start_line <= x.end_line && x.end_line < idx.len() => Some((x, idx)),
                    Ok(x) => {
                        append_log(&log, &format!("auto_clip_refine_bad task_id={} start={:.0} lines={}-{}", task_id, range.start, x.start_line, x.end_line));
                        None
                    }
                    Err(e) => {
                        append_log(&log, &format!("auto_clip_refine_error task_id={} start={:.0} err={}", task_id, range.start, e));
                        None
                    }
                };
                (clip, range, span, verified, refined)
            }
        })
        .buffer_unordered(cfg.llm.max_concurrency)
        .map(|(clip, range, span, verified, r)| {
            let r = r.map(|(x, idx)| {
                let new = boundary::snap(Range { start: p.segments[idx[x.start_line]].start, end: p.segments[idx[x.end_line]].end }, span, &p.segments);
                (x, Range { start: new.start.max(span.start), ..boundary::fit_sc(new, &p.segments) })
            });
            (clip, r.as_ref().map(|(_, rg)| *rg).unwrap_or(range), span, verified, r.map(|(x, _)| x))
        })
        .collect()
        .await;

    let mut dropped = 0;
    let mut out = Vec::new();
    for (clip, range, _span, verified, r) in refined {
        // 精修失败的保留初选范围，按 B 档处理
        let grade = r.as_ref().map(|x| x.grade.clone()).unwrap_or_else(|| "B".into());
        if r.as_ref().is_some_and(|x| !x.keep) {
            dropped += 1;
            append_log(&ctx.app_log_path, &format!("auto_clip_refine_drop task_id={} start={:.0} title={:?} reason={:?}", task_id, range.start, clip.titles.first(), r.as_ref().map(|x| &x.reason)));
            continue;
        }
        let ratio = boundary::streamer_ratio(range, &p.segments);
        if range.len() < REFINED_MIN_SECS {
            continue;
        }
        if ratio < MIN_STREAMER_RATIO {
            low_streamer += 1;
            continue;
        }
        // 先按档位排序，同档内按弹幕热度
        let rank = match grade.as_str() { "A" => 2.0, "B" => 1.0, _ => 0.0 };
        let dm = has_danmaku.then(|| signals::danmaku_score(&per_min, range.start, range.end));
        // 同档同热度时（无弹幕时热度都按 5 算）用独立可看、主播占比、时长区分，权重小到不会越过档位：
        // 热度项最大差 0.225，此项在 -0.01～0.05 之间，合计小于档位间隔 0.3
        let tie = 0.004 * clip.scores.standalone + 0.01 * ratio - if range.len() < SHORT_CLIP_SECS { 0.01 } else { 0.0 };
        let total = (0.1 + 0.3 * rank) + 0.025 * dm.unwrap_or(5.0) + tie;
        let s = &clip.scores;
        let signals = json!({
            "grade": grade,
            "refineReason": r.as_ref().map(|x| x.reason.clone()),
            "scores": {
                "funny": s.funny, "complete": s.complete, "standalone": s.standalone, "streamer": s.streamer,
                "danmaku": dm.map(|d| (d * 10.0).round() / 10.0),
                "total": (total * 100.0).round() / 10.0,
            },
            "streamerRatio": ratio,
        });
        out.push(Picked {
            range,
            ai_status: if verified { "KEPT" } else { "UNVERIFIED" },
            score: total * if verified { 1.0 } else { 0.6 },
            dedupe_key: rank * 1000.0 + range.len(),
            signals,
            peak_count: 0,
            clip: Some(clip),
        });
    }
    if dropped > 0 {
        warnings.push(format!("{} 条候选精修后判定不值得切", dropped));
    }
    // 全部失败多半是配置或网关问题，直接报错，避免后面删掉上次的切片
    if failed > 0 && failed == chunk_count {
        return Err(format!("大模型切分全部失败（{} 段），已保留原有切片，请检查大模型配置", failed));
    }
    if failed > 0 {
        warnings.push(format!("{} 段转写大模型切分失败，这部分没有切片", failed));
    }
    if low_streamer > 0 {
        warnings.push(format!("{} 个话题主播说话占比不足 {:.0}%，未出片", low_streamer, MIN_STREAMER_RATIO * 100.0));
    }
    append_log(&ctx.app_log_path, &format!("auto_clip_topics task_id={} topics={}", task_id, out.len()));
    Ok(out)
}

/// 没有大模型时按信号峰值窗口切片
fn signal_clips(candidates: &[Candidate], spans: &[Span], pass: Option<&FullPass>, clip: &settings::ClipSettings) -> Vec<Picked> {
    let empty = Vec::new();
    let segs = pass.map(|p| &p.segments).unwrap_or(&empty);
    candidates
        .iter()
        .filter_map(|cand| {
            let span = spans.iter().copied().find(|s| cand.peak >= s.start && cand.peak < s.end)?;
            let range = boundary::finalize(Range { start: cand.start, end: cand.end }, cand.peak, span, segs, clip.min_duration_secs as f64, clip.max_duration_secs as f64);
            let signals = json!({
                "snapshot": cand.signals,
                "summary": cand.signals.summary(),
                "signalScore": cand.score,
                "peak": cand.peak,
                "streamerRatio": pass.map(|p| boundary::streamer_ratio(range, &p.segments)),
            });
            Some(Picked { range, clip: None, ai_status: "SIGNAL_ONLY", score: cand.score, dedupe_key: cand.score, signals, peak_count: cand.signals.max_repeat as i64 })
        })
        .collect()
}

fn hms(secs: f64, sep: &str) -> String {
    let s = secs.max(0.0) as i64;
    format!("{:02}{sep}{:02}{sep}{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

/// 给大模型的纯原片句子最多保留的字数
const ORIGINAL_LINE_CHARS: usize = 30;

/// 每行：[起止秒][说话方|情绪/事件] 文本；同时返回纯文本用于证据校验
fn window_transcript(segments: &[asr::AsrSegment], start: f64, end: f64) -> (String, String) {
    let mut lines = Vec::new();
    let mut plain = String::new();
    let mut prev_sc: Option<usize> = None;
    for s in segments.iter().filter(|s| s.end > start && s.start < end) {
        // 原片句子里主播插了话（不到一半）时单独标出，免得大模型把主播的吐槽当成原片
        let speaker = if let Some(sc) = &s.sc {
            if sc.pure { "念SC" } else { "念SC+主播" }
        } else if s.is_streamer() {
            "主播"
        } else if s.streamer_seconds() >= 1.0 {
            "原片+主播插话"
        } else {
            "原片"
        };
        let extra: Vec<&str> = [s.emotion.as_str(), s.event.as_str()]
            .into_iter()
            .filter(|x| !x.is_empty() && *x != "EMO_UNKNOWN" && *x != "NEUTRAL" && *x != "Speech")
            .collect();
        let tag = if extra.is_empty() { speaker.to_string() } else { format!("{}|{}", speaker, extra.join("/")) };
        let text = s.best_text();
        // 纯原片句子只留开头，提示大模型这是铺垫；证据校验仍用全文
        let shown = if speaker == "原片" && text.chars().count() > ORIGINAL_LINE_CHARS {
            format!("{}……（原片约 {:.0} 秒）", text.chars().take(ORIGINAL_LINE_CHARS).collect::<String>(), s.end - s.start)
        } else {
            text.to_string()
        };
        // 一条 SC 的第一句附上原文，识别有误时以原文为准
        let sc_text = s
            .sc
            .as_ref()
            .filter(|sc| !sc.text.is_empty() && !matches!(prev_sc, Some(id) if id == sc.id))
            .map(|sc| format!("（SC原文：{}）", sc.text))
            .unwrap_or_default();
        prev_sc = s.sc.as_ref().map(|sc| sc.id);
        lines.push(format!("[{:.1}-{:.1}s][{}] {}{}", s.start, s.end, tag, shown, sc_text));
        plain.push_str(text);
        plain.push_str(&s.text);
    }
    (lines.join("\n"), plain)
}

/// 精修时给大模型看的候选前后转写长度
const REFINE_PAD_SECS: f64 = 90.0;
/// 观众对一句话的弹幕反应滞后，统计这句话结束后这么多秒内的弹幕
const DANMAKU_REACT_SECS: f64 = 6.0;

/// 精修用的编号转写：每行前加行号，行尾附上这句话期间及之后几秒的弹幕条数。返回每行对应的句子下标
fn numbered_window(segments: &[asr::AsrSegment], chat_times: &[f64], start: f64, end: f64) -> (Vec<usize>, String) {
    let idx: Vec<usize> = (0..segments.len()).filter(|&i| segments[i].end > start && segments[i].start < end).collect();
    let (lines, _) = window_transcript(segments, start, end);
    let text = lines
        .split('\n')
        .zip(&idx)
        .enumerate()
        .map(|(n, (line, &i))| {
            let (a, b) = (segments[i].start, segments[i].end + DANMAKU_REACT_SECS);
            let count = chat_times.partition_point(|&t| t < b) - chat_times.partition_point(|&t| t < a);
            format!("{} {}【弹幕{}】", n, line, count)
        })
        .collect::<Vec<_>>()
        .join("\n");
    (idx, text)
}

/// 评阅页用的场次上下文：每分钟弹幕人数、各录播文件在时间轴上的位置，以及（可选）整场字幕。
/// 字幕取自分析时的识别缓存，没分析过的场次为空。
pub fn session_context(db: &Db, room_id: &str, record_ids: &[i64], include_transcript: bool) -> Result<serde_json::Value, String> {
    let segs = normalize_segments_for_analysis(load_record_segments(db, room_id, record_ids)?, None);
    let Some(first) = segs.first() else {
        return Ok(json!({ "heat": [], "files": [], "segments": [] }));
    };
    let cache = resolve_clips_dir(&first.file_path)?.join(".cache");
    let ids_key = record_ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join("-");
    let read = |name: String| std::fs::read(cache.join(name)).ok();
    let heat = session_heat(&segs, &cache, &ids_key);
    let files: Vec<serde_json::Value> = segs
        .iter()
        .map(|s| json!({ "path": s.file_path, "offsetStart": s.offset_start, "offsetEnd": s.offset_end }))
        .collect();
    let mut segments = Vec::new();
    if include_transcript {
        if let Some(mut pass) = read(format!("asr_{}_{}.json", ASR_CACHE_VERSION, ids_key)).and_then(|b| serde_json::from_slice::<FullPass>(&b).ok()) {
            let speaker_key = room_voiceprint(db, room_id).map(|v| speaker::voiceprint_key(&v));
            if let Some(key) = &speaker_key {
                if let Some(secs) = read(format!("speaker_{}_{}_{}.json", key, ASR_CACHE_VERSION, ids_key)).and_then(|b| serde_json::from_slice::<Vec<Option<f64>>>(&b).ok()) {
                    pass.segments.iter_mut().zip(secs).for_each(|(s, v)| s.streamer_secs = v);
                }
            }
            let key = speaker_key.unwrap_or_else(|| "lang".into());
            if let Some(texts) = read(format!("refine_{}_{}_{}.json", ASR_CACHE_VERSION, ids_key, key)).and_then(|b| serde_json::from_slice::<Vec<Option<String>>>(&b).ok()) {
                pass.segments.iter_mut().zip(texts).for_each(|(s, t)| s.zh_text = t);
            }
            let edits = load_subtitle_edits(&cache, &ids_key);
            segments = pass
                .segments
                .iter()
                .map(|s| {
                    let key = subtitle_key(s);
                    let edited = edits.get(&key);
                    // orig 只在改过时返回，用于显示识别原文和恢复；cues 是改过时间或插入过句子后的逐句字幕
                    let cues = match edited {
                        Some(SubtitleEdit::Cues(cues)) => json!(cues),
                        _ => serde_json::Value::Null,
                    };
                    json!({ "key": key, "start": s.start, "end": s.end, "streamer": s.is_streamer(), "text": edited.map(|e| e.text()).unwrap_or_else(|| s.best_text().to_string()), "orig": edited.map(|_| s.best_text()), "cues": cues })
                })
                .collect();
        }
    }
    Ok(json!({ "heat": heat, "files": files, "segments": segments }))
}

/// 字幕修订对应识别分段的编号：分段起点的毫秒数
fn subtitle_key(s: &asr::AsrSegment) -> String {
    ((s.start * 1000.0).round() as i64).to_string()
}

/// 一个识别分段的人工修订：整段文字，或带时间的逐句字幕（改过时间、插入过句子的分段）
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum SubtitleEdit {
    Text(String),
    Cues(Vec<EditedCue>),
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct EditedCue {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

impl SubtitleEdit {
    /// 交给大模型、显示整段时用的文字：逐句字幕按顺序用逗号连起来
    fn text(&self) -> String {
        match self {
            SubtitleEdit::Text(t) => t.clone(),
            SubtitleEdit::Cues(cues) => cues.iter().map(|c| c.text.trim()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("，"),
        }
    }
}

/// 评阅页人工修订的字幕：分段编号 → 修订
fn load_subtitle_edits(cache: &Path, ids_key: &str) -> HashMap<String, SubtitleEdit> {
    std::fs::read(cache.join(format!("subtitle_edits_{}.json", ids_key))).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// 用人工修订替换识别文字，分析时交给大模型的是修订后的字幕
fn apply_subtitle_edits(segments: &mut [asr::AsrSegment], edits: &HashMap<String, SubtitleEdit>) {
    for s in segments.iter_mut() {
        if let Some(edit) = edits.get(&subtitle_key(s)) {
            s.zh_text = Some(edit.text());
        }
    }
}

/// 保存一个识别分段的人工修订（识别不准时在评阅页修改）。`key` 是识别分段的编号（见 `subtitle_key`）；
/// 给了 cues（逐句字幕，含时间）就按逐句保存（空列表表示整段字幕都删了），否则 text 为修订后的整段，text 为空表示恢复识别原文。
pub fn save_subtitle_edit(db: &Db, room_id: &str, record_ids: &[i64], key: &str, text: &str, cues: Option<Vec<EditedCue>>) -> Result<(), String> {
    let segs = normalize_segments_for_analysis(load_record_segments(db, room_id, record_ids)?, None);
    let first = segs.first().ok_or("未找到录播文件")?;
    let cache = resolve_clips_dir(&first.file_path)?.join(".cache");
    std::fs::create_dir_all(&cache).map_err(|e| format!("创建缓存目录失败: {}", e))?;
    let ids_key = record_ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join("-");
    let path = cache.join(format!("subtitle_edits_{}.json", ids_key));
    let mut edits: serde_json::Map<String, serde_json::Value> = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    if let Some(cues) = cues {
        // 逐句保存；空列表表示这一段的字幕全部删掉了（不是恢复原文）
        let cues: Vec<EditedCue> = cues.into_iter().filter(|c| c.end > c.start && !c.text.trim().is_empty()).collect();
        edits.insert(key.to_string(), json!(cues));
    } else if text.trim().is_empty() {
        edits.remove(key);
    } else {
        edits.insert(key.to_string(), json!(text.trim()));
    }
    std::fs::write(&path, serde_json::to_vec(&edits).unwrap_or_default()).map_err(|e| format!("保存字幕修订失败: {}", e))
}

/// 每分钟弹幕人数；解析整场弹幕较慢，结果缓存到切片目录，弹幕文件更新后重新计算
fn session_heat(segs: &[LiveRecordSegment], cache: &Path, ids_key: &str) -> Vec<usize> {
    let path = cache.join(format!("heat_{}.json", ids_key));
    let mtime = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let newest = segs.iter().filter_map(|s| mtime(&danmaku::danmaku_paths(&s.file_path).0)).max();
    if let (Some(cached_at), Some(newest)) = (mtime(&path), newest) {
        if cached_at >= newest {
            if let Some(heat) = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()) {
                return heat;
            }
        }
    }
    let base_time = segs.iter().map(|s| s.start_time).min().unwrap();
    let session_secs = segs.iter().map(|s| s.offset_end).max().unwrap_or(0) as f64;
    let sources: Vec<DanmakuSource> = segs.iter().map(|s| DanmakuSource { video_path: &s.file_path, offset_start: s.offset_start as f64 }).collect();
    let (events, has_danmaku) = danmaku::load_events(&sources, base_time);
    let heat = if has_danmaku { signals::users_per_minute(&events, session_secs) } else { Vec::new() };
    if has_danmaku && std::fs::create_dir_all(cache).is_ok() {
        let _ = std::fs::write(&path, serde_json::to_vec(&heat).unwrap_or_default());
    }
    heat
}

/// 整场弹幕、SC、礼物、上舰记录，供查看。每条为 [场次秒数, 类型, uid, 昵称, 内容, 金额(元)]，
/// 类型：chat 弹幕、sc、gift 礼物、guard 上舰。
pub fn session_events(db: &Db, room_id: &str, record_ids: &[i64]) -> Result<serde_json::Value, String> {
    let segs = normalize_segments_for_analysis(load_record_segments(db, room_id, record_ids)?, None);
    let Some(base_time) = segs.iter().map(|s| s.start_time).min() else {
        return Ok(json!([]));
    };
    let sources: Vec<DanmakuSource> = segs.iter().map(|s| DanmakuSource { video_path: &s.file_path, offset_start: s.offset_start as f64 }).collect();
    let (events, _) = danmaku::load_events(&sources, base_time);
    let rows: Vec<serde_json::Value> = events
        .iter()
        .map(|e| {
            let kind = match e.kind {
                danmaku::DanmakuKind::Chat => "chat",
                danmaku::DanmakuKind::SuperChat => "sc",
                danmaku::DanmakuKind::Gift => "gift",
                danmaku::DanmakuKind::Guard => "guard",
            };
            json!([(e.t * 10.0).round() / 10.0, kind, e.user, e.name, e.text, e.money])
        })
        .collect();
    Ok(json!(rows))
}

/// 评阅预览最多保留的文件数，超出时删除最旧的
const PREVIEW_KEEP: usize = 6;

/// 生成评阅用的预览片段：从场次时间轴 [start, end) 所在录播文件中按关键帧直接复制，索引放在文件开头。
/// 录播源文件太大且索引在末尾时，软件内播放器无法直接加载，预览文件只有几分钟，生成不到一秒。
/// 返回预览文件路径及它在场次时间轴上的起止秒数。
/// 预览放在应用缓存目录：录播目录下的 .cache 是隐藏目录，软件内播放器的文件访问范围不含隐藏目录。
pub fn preview_clip(db: &Db, room_id: &str, record_ids: &[i64], start: f64, end: f64, dir: &Path) -> Result<serde_json::Value, String> {
    let segs = normalize_segments_for_analysis(load_record_segments(db, room_id, record_ids)?, None);
    let seg = segs
        .iter()
        .find(|s| start >= s.offset_start as f64 && start < s.offset_end as f64)
        .ok_or("该位置没有可用的录播文件（可能已被删除）")?;
    let in_file = (start - seg.offset_start as f64).max(0.0);
    let until = end.min(seg.offset_end as f64) - seg.offset_start as f64;
    // 从不晚于起点的最近关键帧开始复制，预览的 0 秒就对应这个关键帧
    let probe = crate::ffmpeg::run_ffprobe_capture(
        &[
            "-v", "error", "-select_streams", "v:0", "-skip_frame", "nokey",
            "-read_intervals", &format!("{:.3}%{:.3}", (in_file - 15.0).max(0.0), in_file + 0.5),
            "-show_entries", "frame=pts_time", "-of", "csv=p=0", &seg.file_path,
        ]
        .map(String::from),
    )
    .map(|(out, _)| out)
    .unwrap_or_default();
    let key = probe
        .lines()
        .filter_map(|l| l.trim().trim_end_matches(',').parse::<f64>().ok())
        .filter(|t| *t <= in_file + 0.01)
        .fold(None, |acc: Option<f64>, t| Some(acc.map_or(t, |a| a.max(t))))
        .unwrap_or(in_file);
    let duration = (until - key).max(1.0);
    std::fs::create_dir_all(dir).map_err(|e| format!("创建预览目录失败: {}", e))?;
    let output = dir.join(format!("preview_{}_{}_{}.mp4", seg.record_id, (key * 1000.0) as i64, (duration * 1000.0) as i64));
    if !output.exists() {
        let tmp = output.with_extension("part.mp4");
        crate::ffmpeg::run_ffmpeg(
            &[
                "-hide_banner", "-loglevel", "error", "-y", "-nostdin",
                "-ss", &format!("{:.3}", key), "-i", &seg.file_path, "-t", &format!("{:.3}", duration),
                "-map", "0:v:0", "-map", "0:a:0?", "-c", "copy",
                "-movflags", "+faststart", "-avoid_negative_ts", "make_zero",
                &tmp.to_string_lossy(),
            ]
            .map(String::from),
        )
        .map_err(|e| format!("生成预览失败: {}", e))?;
        std::fs::rename(&tmp, &output).map_err(|e| format!("生成预览失败: {}", e))?;
        prune_previews(dir);
    }
    Ok(json!({
        "path": output.to_string_lossy(),
        "start": seg.offset_start as f64 + key,
        "end": seg.offset_start as f64 + key + duration,
    }))
}

fn prune_previews(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in files.into_iter().skip(PREVIEW_KEEP) {
        let _ = std::fs::remove_file(path);
    }
}

/// 房间登记的主播声纹
pub fn room_voiceprint(db: &Db, room_id: &str) -> Option<Vec<f32>> {
    let raw = db
        .with_conn(|c| c.query_row("SELECT voiceprint FROM live_room_settings WHERE room_id = ?1", [room_id], |r| r.get::<_, Option<String>>(0)))
        .ok()
        .flatten()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    serde_json::from_value(value.get("embedding")?.clone()).ok()
}

/// 直播标题、内容类型、主播昵称
fn room_meta(db: &Db, room_id: &str, first_record: Option<i64>) -> (String, Profile, String) {
    let title = first_record
        .and_then(|id| db.with_conn(|c| c.query_row("SELECT title FROM live_record_task WHERE id = ?1", [id], |r| r.get::<_, Option<String>>(0))).ok().flatten())
        .unwrap_or_default();
    let profile = db
        .with_conn(|c| c.query_row("SELECT clip_profile FROM live_room_settings WHERE room_id = ?1", [room_id], |r| r.get::<_, Option<String>>(0)))
        .ok()
        .flatten()
        .unwrap_or_default();
    let nickname = db
        .with_conn(|c| c.query_row("SELECT nickname FROM anchor WHERE uid = ?1", [room_id], |r| r.get::<_, Option<String>>(0)))
        .ok()
        .flatten()
        .unwrap_or_default();
    (title, Profile::parse(&profile), nickname)
}

/// 切片的投稿简介和标签：套用主播投稿设置里的简介、固定标签，简介补上直播日期、标题和直播间
fn submission_meta(db: &Db, room_id: &str, room_title: &str, base_time: DateTime<Utc>) -> (String, Vec<String>) {
    let (desc, tags) = db
        .with_conn(|c| {
            c.query_row("SELECT description, tags FROM anchor_submission_config WHERE room_id = ?1", [room_id], |r| {
                Ok((r.get::<_, Option<String>>(0)?.unwrap_or_default(), r.get::<_, Option<String>>(1)?.unwrap_or_default()))
            })
        })
        .unwrap_or_default();
    // B站直播按北京时间记日期
    let day = (base_time + ChronoDuration::hours(8)).format("%Y-%m-%d");
    let live = format!("直播：{} {}\n直播间：https://live.bilibili.com/{}", day, room_title, room_id);
    let description = if desc.trim().is_empty() { live } else { format!("{}\n{}", desc.trim(), live) };
    let tags = tags.split(',').map(str::trim).filter(|t| !t.is_empty()).map(String::from).collect();
    (description, tags)
}

/// 大模型逐条分档看不到整场，比例失控：按综合分名次重新定档，前 10% 为 A、其后 30% 为 B、其余为 C，原档位保留在 aiGrade
fn grade_by_rank(picked: &mut [Picked]) {
    let mut order: Vec<usize> = (0..picked.len()).filter(|&i| picked[i].signals.get("grade").is_some()).collect();
    order.sort_by(|&x, &y| picked[y].score.total_cmp(&picked[x].score));
    let n = order.len() as f64;
    let (a_end, b_end) = ((n * 0.1).ceil() as usize, (n * 0.4).ceil() as usize);
    for (k, i) in order.into_iter().enumerate() {
        let grade = if k < a_end { "A" } else if k < b_end { "B" } else { "C" };
        let s = &mut picked[i].signals;
        s["aiGrade"] = s["grade"].clone();
        s["grade"] = json!(grade);
    }
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
fn insert_item(db: &Db, task_id: i64, room_id: &str, seg: &LiveRecordSegment, output: &Path, p: &Picked, title: &str, meta: &(String, Vec<String>), ok: bool) {
    let j = p.clip.as_ref();
    let titles = j.map(|j| j.titles.clone()).unwrap_or_else(|| vec![title.to_string()]);
    // 固定标签在前，再补大模型给的具体标签
    let mut tags = meta.1.clone();
    for t in j.map(|j| j.tags.as_slice()).unwrap_or_default() {
        if !tags.iter().any(|x| x.eq_ignore_ascii_case(t)) {
            tags.push(t.clone());
        }
    }
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
                p.range.start,
                p.range.end,
                p.range.len(),
                p.peak_count,
                now,
                (p.score * 1000.0).round() / 1000.0,
                p.signals.to_string(),
                j.map(|j| j.category.clone()),
                serde_json::to_string(&titles).unwrap_or_default(),
                meta.0,
                tags.join(","),
                j.map(|j| j.cover_text.clone()),
                j.map(|j| j.reason.clone()),
                j.map(|j| serde_json::to_string(&j.evidence).unwrap_or_default()),
                p.ai_status,
            ],
        )
    });
}

/// 主播自动切片的生效状态：Some(since) 表示按主播设置启用，零点表示沿用全局，
/// None 表示未启用。`since` 用于跳过开启之前结束的场次。
fn room_auto_clip_state(
    db: &crate::db::Db,
    room_id: &str,
    global_enabled: bool,
    global_since: Option<DateTime<Utc>>,
) -> Option<Option<DateTime<Utc>>> {
    let row: Option<(Option<i64>, Option<String>)> = db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT auto_clip, auto_clip_since FROM live_room_settings WHERE room_id = ?1",
                [room_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
        })
        .ok()
        .flatten();
    match row {
        // 主播单独设置过：开就用自己的起始时间（可能为空，表示从最早算）
        Some((Some(flag), since)) => {
            if flag == 0 {
                return None;
            }
            let parsed = since
                .as_deref()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.with_timezone(&Utc));
            Some(parsed)
        }
        // 没设置过：跟随全局
        _ => {
            if global_enabled {
                Some(global_since)
            } else {
                None
            }
        }
    }
}

/// 扫描最近几天已结束的场次，为尚未处理的场次创建自动切片任务
fn auto_trigger(ctx: &ClipContext) -> Result<(), String> {
    let cfg = settings::load(&ctx.db);
    // 全局开关关闭时仍需扫描：主播可能单独开启了自动切片
    let global_enabled = cfg.clip.enabled;
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
        if !ready || now - last_end <= gap || existing.contains(&key) {
            continue;
        }
        let Some(room_since) = room_auto_clip_state(
            &ctx.db,
            &room,
            global_enabled,
            Some(enabled_since),
        ) else {
            continue;
        };
        // 只处理开启之后结束的场次，避免开启瞬间回溯分析历史录播
        if let Some(since) = room_since {
            if last_end < since {
                continue;
            }
        }
        let id = create_task(&ctx.db, &room, &ids, "AUTO", Some(&key))?;
        append_log(&ctx.app_log_path, &format!("auto_clip_task_created task_id={} session={} records={}", id, key, ids.len()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 主播级开关：未设置时跟随全局，设置后以主播为准
    #[test]
    fn room_auto_clip_follows_global_when_absent() {
        let db = crate::db::Db::new(std::path::PathBuf::from(":memory:")).expect("init db");
        let global_since = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        // 全局关、主播未设置 → 不启用
        assert!(room_auto_clip_state(&db, "room1", false, None).is_none());
        // 全局开、主播未设置 → 跟随全局并沿用全局起始时间
        let state = room_auto_clip_state(&db, "room1", true, Some(global_since)).unwrap();
        assert_eq!(state, Some(global_since));
    }

    #[test]
    fn room_auto_clip_overrides_global() {
        let db = crate::db::Db::new(std::path::PathBuf::from(":memory:")).expect("init db");
        let global_since = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        // 全局关但主播单独开启：按主播设置启用，起始时间取主播自己的
        db.with_conn(|c| {
            c.execute(
                "INSERT INTO live_room_settings (room_id, auto_clip, auto_clip_since, update_time) \
                 VALUES ('room1', 1, '2026-02-01T00:00:00Z', '2026-02-01T00:00:00Z')",
                [],
            )
        })
        .unwrap();
        let state = room_auto_clip_state(&db, "room1", false, None).unwrap();
        assert_eq!(
            state,
            Some(
                DateTime::parse_from_rfc3339("2026-02-01T00:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc)
            )
        );

        // 全局开但主播单独关闭：不启用
        db.with_conn(|c| {
            c.execute(
                "UPDATE live_room_settings SET auto_clip = 0 WHERE room_id = 'room1'",
                [],
            )
        })
        .unwrap();
        assert!(room_auto_clip_state(&db, "room1", true, Some(global_since)).is_none());
    }

    #[test]
    fn transcript_for_llm_uses_subtitle_edits() {
        let seg = |start: f64, text: &str| asr::AsrSegment {
            start,
            end: start + 3.0,
            file_idx: 0,
            file_start: start,
            lang: "zh".into(),
            emotion: String::new(),
            event: String::new(),
            text: text.into(),
            zh_text: None,
            streamer_secs: None,
            sc: None,
        };
        let mut segments = vec![seg(3193.182, "到了"), seg(3196.67, "谢谢")];
        let edits = HashMap::from([("3193182".to_string(), SubtitleEdit::Text("爆了".to_string()))]);
        apply_subtitle_edits(&mut segments, &edits);
        let (lines, _) = window_transcript(&segments, 3190.0, 3200.0);
        assert!(lines.contains("] 爆了") && lines.contains("] 谢谢"), "{lines}");
    }
}
