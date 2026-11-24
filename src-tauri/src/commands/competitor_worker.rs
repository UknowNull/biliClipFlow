//! 竞品分析的自动执行循环。
//!
//! 挂到现有 300 秒轮询里：扫描竞品 UP 新投稿 → 登记 → 下载 → 转写。
//! 任何一步失败都记 `FAILED` 并继续下一条，不阻塞其他竞品，也不影响录播切片。

use std::path::Path;
use std::sync::Arc;

use super::competitor::{list_for_room, save_scan_time};
use super::competitor_discover::scan_up;
use super::competitor_media;
use crate::bilibili::client::BilibiliClient;
use crate::db::Db;
use crate::login_store::{AuthInfo, LoginStore};
use crate::utils::append_log;

/// 单个竞品 UP 每次扫描最多取多少条投稿。
/// 首次配置只回溯当天，所以不需要取太多；提高会增加风控风险。
const SCAN_PAGE_SIZE: u32 = 20;

/// 扫描所有主播的竞品配置，登记新投稿。
pub async fn scan_all(
    db: &Arc<Db>,
    client: &Arc<BilibiliClient>,
    login_store: &Arc<LoginStore>,
    log_path: &Path,
) -> Result<usize, String> {
    let auth = current_auth(login_store, db);
    // 取出所有已启用配置的 room_id
    let rooms: Vec<String> = db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT room_id FROM competitor_up WHERE enabled = 1",
            )?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .map_err(|err| format!("读取竞品房间失败: {}", err))?;

    let mut total = 0usize;
    for room_id in rooms {
        let ups = match list_for_room(db, &room_id) {
            Ok(list) => list,
            Err(err) => {
                append_log(log_path, &format!("competitor_scan_list_failed room={} err={}", room_id, err));
                continue;
            }
        };
        for up in ups.iter().filter(|u| u.enabled) {
            match scan_up(db, client, auth.as_ref(), up, SCAN_PAGE_SIZE).await {
                Ok(added) => {
                    total += added;
                    if added > 0 {
                        append_log(
                            log_path,
                            &format!(
                                "competitor_scan_found room={} up={} added={}",
                                room_id, up.up_uid, added
                            ),
                        );
                    }
                    // 记录扫描时间，便于排查「为什么没抓到」
                    let _ = save_scan_time(db, up.id);
                }
                Err(err) => {
                    append_log(
                        log_path,
                        &format!(
                            "competitor_scan_failed room={} up={} err={}",
                            room_id, up.up_uid, err
                        ),
                    );
                }
            }
        }
    }
    Ok(total)
}

/// 处理处于 PENDING 的竞品切片：下载 → 转写。
///
/// 下载复用现有下载队列：登记到 `video_download` 后由队列执行；
/// 这里只负责把队列里已完成的文件接过来转写。
pub fn process_pending(db: &Arc<Db>, model_dir: &Path, threads: i32, log_path: &Path) -> usize {
    // 取出待处理的竞品切片，连同归属场次的目录一起查出来
    let pending: Vec<(i64, String, String, String)> = db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT c.id, c.bvid, COALESCE(c.title,''), \
                        COALESCE((SELECT i.source_file_path FROM live_clip_item i \
                                  WHERE i.task_id = c.task_id LIMIT 1), '') \
                 FROM competitor_clip c \
                 WHERE c.status = 'PENDING' ORDER BY c.id ASC LIMIT 10",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .unwrap_or_default();

    let mut done = 0usize;
    for (id, bvid, title, source_file) in pending {
        // 没有归属场次 → 无法确定存放位置，记跳过
        if source_file.trim().is_empty() {
            let _ = competitor_media::set_status(
                db,
                id,
                "SKIPPED",
                "没有归属场次，无法确定竞品文件位置",
            );
            continue;
        }
        let Some(dir) = competitor_media::competitor_dir_of(&source_file) else {
            let _ = competitor_media::set_status(db, id, "SKIPPED", "无法解析场次目录");
            continue;
        };
        // 已下载过就直接进转写
        let video_path = dir.join(format!("{}.mp4", bvid));
        let asr_path = dir.join(format!("{}.json", bvid));
        if !video_path.exists() {
            // 真正的入队需要 AppState，放到 Tauri 命令里做；
            // 这里只标记，命令执行时会把它们送进下载队列。
            let _ = competitor_media::set_status(db, id, "DOWNLOADING", "等待下载队列");
            append_log(
                log_path,
                &format!(
                    "competitor_wait_download id={} bvid={} title={}",
                    id, bvid, title
                ),
            );
            continue;
        }

        let _ = competitor_media::set_status(db, id, "TRANSCRIBING", "");
        match competitor_media::transcribe_clip(
            model_dir,
            threads,
            &video_path.to_string_lossy(),
            &asr_path,
        ) {
            Ok(_) => {
                let _ = competitor_media::attach_asr(db, id, &asr_path.to_string_lossy());
                done += 1;
                append_log(
                    log_path,
                    &format!("competitor_transcribed id={} bvid={}", id, bvid),
                );
            }
            Err(err) => {
                let _ = competitor_media::set_status(db, id, "FAILED", &err);
                append_log(
                    log_path,
                    &format!("competitor_transcribe_failed id={} err={}", id, err),
                );
            }
        }
    }
    done
}

/// 取当前激活账号的登录态
fn current_auth(login_store: &Arc<LoginStore>, db: &Arc<Db>) -> Option<AuthInfo> {
    login_store.load_primary_auth_info(db).ok().flatten()
}

/// 把已在下载队列中完成、文件已落盘的竞品切片接去转写。
/// 由前端轮询或定时任务调用。
pub fn resume_downloading(
    db: &Arc<Db>,
    model_dir: &Path,
    threads: i32,
    log_path: &Path,
) -> usize {
    let rows: Vec<(i64, String, String)> = db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT c.id, c.bvid, COALESCE((SELECT i.source_file_path FROM live_clip_item i \
                        WHERE i.task_id = c.task_id LIMIT 1), '') \
                 FROM competitor_clip c WHERE c.status = 'DOWNLOADING' LIMIT 10",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .unwrap_or_default();

    let mut done = 0usize;
    for (id, bvid, source_file) in rows {
        if source_file.trim().is_empty() {
            let _ = competitor_media::set_status(db, id, "SKIPPED", "没有归属场次");
            continue;
        }
        let Some(dir) = competitor_media::competitor_dir_of(&source_file) else {
            continue;
        };
        let video_path = dir.join(format!("{}.mp4", bvid));
        let asr_path = dir.join(format!("{}.json", bvid));
        if !video_path.exists() {
            continue; // 还在下载
        }
        let _ = competitor_media::set_status(db, id, "TRANSCRIBING", "");
        match competitor_media::transcribe_clip(
            model_dir,
            threads,
            &video_path.to_string_lossy(),
            &asr_path,
        ) {
            Ok(_) => {
                let _ = competitor_media::attach_asr(db, id, &asr_path.to_string_lossy());
                done += 1;
                append_log(log_path, &format!("competitor_resume_transcribed id={}", id));
            }
            Err(err) => {
                let _ = competitor_media::set_status(db, id, "FAILED", &err);
                append_log(
                    log_path,
                    &format!("competitor_resume_failed id={} err={}", id, err),
                );
            }
        }
    }
    done
}

/// 竞品分析的轮询周期：与录播自动切片保持一致（5 分钟）。
const SCAN_INTERVAL_SECS: u64 = 300;

/// 启动竞品轮询：扫描新投稿 → 入队下载 → 转写。
/// 任何一步失败只记日志，不影响录播切片主流程。
pub fn start(app: tauri::AppHandle, app_data_dir: std::path::PathBuf) {
    tauri::async_runtime::spawn(async move {
        // 首轮延后一点，避免和启动时的其他扫描抢网络
        tokio::time::sleep(std::time::Duration::from_secs(90)).await;
        loop {
            run_once(&app, &app_data_dir).await;
            tokio::time::sleep(std::time::Duration::from_secs(SCAN_INTERVAL_SECS)).await;
        }
    });
}

/// 执行一轮：扫描 → 入队下载 → 转写已完成下载的
async fn run_once(app: &tauri::AppHandle, app_data_dir: &std::path::Path) {
    use tauri::Manager;
    let state = app.state::<crate::AppState>();
    let db = Arc::clone(&state.db);
    let client = Arc::clone(&state.bilibili);
    let login_store = Arc::clone(&state.login_store);
    let log_path = Arc::clone(&state.app_log_path);

    // 1) 扫描竞品新投稿并登记
    if let Err(err) = scan_all(&db, &client, &login_store, log_path.as_path()).await {
        append_log(log_path.as_path(), &format!("competitor_scan_loop_failed err={}", err));
    }

    // 2) 把待下载的竞品送进下载队列
    let pending: Vec<(i64, String, String)> = db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT c.id, c.bvid, COALESCE(c.title,'') FROM competitor_clip c \
                 WHERE c.status = 'PENDING' LIMIT 5",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
            })?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .unwrap_or_default();

    for (id, bvid, title) in pending {
        // 找归属场次的目录
        let source_file: String = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT COALESCE(i.source_file_path,'') FROM live_clip_item i \
                     JOIN competitor_clip c ON c.task_id = i.task_id \
                     WHERE c.id = ?1 LIMIT 1",
                    [id],
                    |r| r.get(0),
                )
            })
            .unwrap_or_default();
        let Some(dir) = competitor_media::competitor_dir_of(&source_file) else {
            let _ = competitor_media::set_status(&db, id, "SKIPPED", "没有归属场次");
            continue;
        };
        std::fs::create_dir_all(&dir).ok();
        match super::download::enqueue_competitor_download(
            &state,
            &bvid,
            &title,
            &dir.to_string_lossy(),
        )
        .await
        {
            Ok(_) => {
                let _ = competitor_media::set_status(&db, id, "DOWNLOADING", "");
            }
            Err(err) => {
                let _ = competitor_media::set_status(&db, id, "FAILED", &err);
                append_log(
                    log_path.as_path(),
                    &format!("competitor_enqueue_failed id={} err={}", id, err),
                );
            }
        }
    }

    // 3) 已下完的接去转写
    let cfg = crate::highlight::settings::load(&db);
    let mdir = crate::highlight::model_dir(app_data_dir, &cfg.asr);
    if crate::highlight::asr::models_ready(&mdir) {
        resume_downloading(&db, &mdir, cfg.asr.threads, log_path.as_path());
    }
}
