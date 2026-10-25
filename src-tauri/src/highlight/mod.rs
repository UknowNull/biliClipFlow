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
use danmaku::{DanmakuEvent, DanmakuSource};
use llm::{ChunkInput, TopicClip};
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
    // 已登记主播声纹：整场逐句判定主播说话时长（带缓存），供候选召回、主播占比与转写标注使用
    let voiceprint = room_voiceprint(&db, &room_id).filter(|_| speaker::model_ready(&mdir) && pass.is_some());
    let speaker_key = voiceprint.as_deref().map(speaker::voiceprint_key).unwrap_or_else(|| "lang".into());
    if let Some(vp) = voiceprint {
        let p = pass.take().unwrap();
        set_stage(&db, task_id, "SPEAKER", 60.0);
        let ids_key = record_ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join("-");
        let cache = clips_dir.join(".cache").join(format!("speaker_{}_{}.json", speaker::voiceprint_key(&vp), ids_key));
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
                match cached.filter(|c| c.len() == p.segments.len()) {
                    Some(texts) => p.segments.iter_mut().zip(texts).for_each(|(s, t)| s.zh_text = t),
                    None => {
                        let targets: Vec<usize> = (0..p.segments.len()).filter(|&i| p.segments[i].is_streamer()).collect();
                        let audio: Vec<AudioFile> = files.iter().map(|(path, o)| AudioFile { path, offset_start: *o }).collect();
                        asr::refine_chinese(&mdir2, threads, &audio, &mut p.segments, &targets)?;
                        let texts: Vec<Option<String>> = p.segments.iter().map(|s| s.zh_text.clone()).collect();
                        let _ = std::fs::write(&cache, serde_json::to_vec(&texts).unwrap_or_default());
                    }
                }
                Ok(p)
            })
            .await?;
            topic_clips(ctx, task_id, &cfg, &mode, profile, &room_title, &p, &events, has_danmaku, session_secs, &spans, &mut warnings).await
        }
        (_, p) => {
            set_stage(&db, task_id, "CANDIDATES", 60.0);
            let candidates = signals::pick_candidates(session_secs, &spans, &events, has_danmaku, p.as_ref(), profile, &cfg.clip);
            append_log(&ctx.app_log_path, &format!("auto_clip_candidates task_id={} count={} danmaku_events={} asr_segments={}", task_id, candidates.len(), events.len(), p.as_ref().map(|p| p.segments.len()).unwrap_or(0)));
            signal_clips(&candidates, &spans, p.as_ref(), &cfg.clip)
        }
    };
    let keep_idx = boundary::dedupe(&picked.iter().map(|p| (p.range, p.dedupe_key)).collect::<Vec<_>>());
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
        insert_item(&db, task_id, &room_id, seg, &output, &p, &title, cut_result.is_ok());
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

/// 整场转写分块交给大模型按话题切分，弹幕作为标注；起止只对齐到句子边界
#[allow(clippy::too_many_arguments)]
async fn topic_clips(
    ctx: &Arc<ClipContext>,
    task_id: i64,
    cfg: &settings::AutoClipSettings,
    mode: &str,
    profile: Profile,
    room_title: &str,
    p: &FullPass,
    events: &[DanmakuEvent],
    has_danmaku: bool,
    session_secs: f64,
    spans: &[Span],
    warnings: &mut Vec<String>,
) -> Vec<Picked> {
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
    let (llm_cfg, shared_mode, shared_title) = (Arc::new(cfg.llm.clone()), Arc::new(mode.to_string()), Arc::new(room_title.to_string()));
    let (titles, max_secs, total) = (cfg.clip.title_candidates, cfg.clip.max_duration_secs, jobs.len());
    let done = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let results: Vec<(f64, String, Result<Vec<TopicClip>, String>)> = stream::iter(jobs)
        .map(|(a, lines, plain, dm)| {
            let (llm_cfg, mode, title, db, done) = (Arc::clone(&llm_cfg), Arc::clone(&shared_mode), Arc::clone(&shared_title), Arc::clone(&ctx.db), Arc::clone(&done));
            async move {
                let input = ChunkInput { room_title: &title, transcript: &lines, danmaku: &dm, max_secs };
                let r = llm::segment_topics(&llm_cfg, &mode, profile, titles, &input).await;
                let n = done.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                set_stage(&db, task_id, "AI", 70.0 + 15.0 * n as f64 / total.max(1) as f64);
                (a, plain, r)
            }
        })
        .buffer_unordered(cfg.llm.max_concurrency)
        .collect()
        .await;

    let mut failed = 0;
    let mut out = Vec::new();
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
            if range.len() < MIN_TOPIC_SECS {
                continue;
            }
            let verified = llm::evidence_valid(&clip.evidence, &plain);
            if !verified {
                append_log(&ctx.app_log_path, &format!("auto_clip_evidence_invalid task_id={} start={:.0} evidence={:?}", task_id, range.start, clip.evidence));
            }
            let s = &clip.scores;
            let ai = 0.3 * s.funny + 0.2 * s.complete + 0.15 * s.streamer + 0.15 * s.standalone;
            let dm = has_danmaku.then(|| signals::danmaku_score(&per_min, range.start, range.end));
            let total = dm.map(|d| ai + 0.2 * d).unwrap_or(ai / 0.8);
            let signals = json!({
                "scores": {
                    "funny": s.funny, "complete": s.complete, "streamer": s.streamer, "standalone": s.standalone,
                    "danmaku": dm.map(|d| (d * 10.0).round() / 10.0),
                    "total": (total * 10.0).round() / 10.0,
                },
                "streamerRatio": boundary::streamer_ratio(range, &p.segments),
            });
            out.push(Picked {
                range,
                ai_status: if verified { "KEPT" } else { "UNVERIFIED" },
                score: total / 10.0 * if verified { 1.0 } else { 0.6 },
                dedupe_key: s.complete * 10.0 + total,
                signals,
                peak_count: 0,
                clip: Some(clip),
            });
        }
    }
    if failed > 0 {
        warnings.push(format!("{} 段转写大模型切分失败，这部分没有切片", failed));
    }
    append_log(&ctx.app_log_path, &format!("auto_clip_topics task_id={} topics={}", task_id, out.len()));
    out
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

/// 房间登记的主播声纹
pub fn room_voiceprint(db: &Db, room_id: &str) -> Option<Vec<f32>> {
    let raw = db
        .with_conn(|c| c.query_row("SELECT voiceprint FROM live_room_settings WHERE room_id = ?1", [room_id], |r| r.get::<_, Option<String>>(0)))
        .ok()
        .flatten()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    serde_json::from_value(value.get("embedding")?.clone()).ok()
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
    let j = p.clip.as_ref();
    let titles = j.map(|j| j.titles.clone()).unwrap_or_else(|| vec![title.to_string()]);
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
                p.peak_count,
                now,
                (p.score * 1000.0).round() / 1000.0,
                p.signals.to_string(),
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
