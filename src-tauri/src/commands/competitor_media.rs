//! 竞品切片的下载与转写。
//!
//! 下载复用现有下载队列（写入 `video_download` 表后由队列执行），
//! 转写复用录播分析同一套本地模型（SenseVoice + Paraformer），
//! 保证与本地切片的文本质量一致，避免转写差异干扰对比结论。

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::db::Db;
use crate::highlight::asr;
use crate::utils::now_rfc3339;

/// 竞品文件与录播文件放在一起：`<场次目录>/competitor/`。
/// 这样删除场次时可以一并清理，也便于用户找到。
pub fn competitor_dir_of(source_file: &str) -> Option<PathBuf> {
    let path = Path::new(source_file);
    let parent = path.parent()?;
    Some(parent.join("competitor"))
}

/// 更新竞品切片的处理状态
pub fn set_status(db: &Db, id: i64, status: &str, note: &str) -> Result<(), String> {
    let now = now_rfc3339();
    db.with_conn(|conn| {
        conn.execute(
            "UPDATE competitor_clip SET status = ?1, status_note = ?2, update_time = ?3 WHERE id = ?4",
            (status, note, &now, id),
        )?;
        Ok(())
    })
    .map_err(|err| format!("更新竞品状态失败: {}", err))?;
    Ok(())
}

/// 把下载完成的本地路径写回竞品记录
pub fn attach_file(db: &Db, id: i64, file_path: &str) -> Result<(), String> {
    let now = now_rfc3339();
    db.with_conn(|conn| {
        conn.execute(
            "UPDATE competitor_clip SET file_path = ?1, status = 'TRANSCRIBING', update_time = ?2 WHERE id = ?3",
            (file_path, &now, id),
        )?;
        Ok(())
    })
    .map_err(|err| format!("回写竞品文件路径失败: {}", err))?;
    Ok(())
}

/// 把转写结果写回竞品记录
pub fn attach_asr(db: &Db, id: i64, asr_path: &str) -> Result<(), String> {
    let now = now_rfc3339();
    db.with_conn(|conn| {
        conn.execute(
            "UPDATE competitor_clip SET asr_path = ?1, status = 'DONE', status_note = '', update_time = ?2 WHERE id = ?3",
            (asr_path, &now, id),
        )?;
        Ok(())
    })
    .map_err(|err| format!("回写竞品转写结果失败: {}", err))?;
    Ok(())
}

/// 转写一条竞品切片。
///
/// 用与录播完全相同的模型（SenseVoice 识别 + Paraformer 中文精修），
/// 结果存成 JSON 放在同一个 competitor 目录，跳过重复转写。
pub fn transcribe_clip(
    model_dir: &Path,
    threads: i32,
    video_path: &str,
    output_path: &Path,
) -> Result<(), String> {
    let duration = probe_duration(video_path).unwrap_or(0.0);
    if duration <= 0.0 {
        return Err("无法读取竞品视频时长".into());
    }
    let files = [asr::AudioFile {
        path: video_path,
        offset_start: 0.0,
    }];
    let mut pass = asr::full_pass(
        model_dir,
        threads,
        &files,
        duration,
        &|| false,
        &mut |_| {},
    )?;
    // 与录播一致：中文段用 Paraformer 再精修一遍
    let targets: Vec<usize> = (0..pass.segments.len())
        .filter(|&i| {
            let seg = &pass.segments[i];
            seg.is_streamer() || seg.zh_text.is_none()
        })
        .collect();
    if !targets.is_empty() {
        let _ = asr::refine_chinese(model_dir, threads, &files, &mut pass.segments, &targets);
    }
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("创建竞品目录失败: {}", err))?;
    }
    let payload = serde_json::json!({
        "duration": duration,
        "segments": pass.segments,
    });
    std::fs::write(output_path, serde_json::to_vec(&payload).unwrap_or_default())
        .map_err(|err| format!("写入竞品转写结果失败: {}", err))?;
    Ok(())
}

/// 用 ffprobe 探测时长（秒）
fn probe_duration(path: &str) -> Option<f64> {
    let output = std::process::Command::new(crate::config::resolve_ffprobe_path())
        .args([
            "-v", "error",
            "-show_entries", "format=duration",
            "-of", "default=nw=1:nk=1",
            path,
        ])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

/// 读取已转写的竞品内容（供对比使用）
pub fn load_transcript(path: &Path) -> Option<Value> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn competitor_dir_sits_next_to_source() {
        let dir = competitor_dir_of("/data/25034104/20261122/录制-a.mp4").unwrap();
        assert_eq!(
            dir.to_string_lossy(),
            "/data/25034104/20261122/competitor"
        );
    }

    #[test]
    fn status_transitions_are_persisted() {
        let db = Db::new(std::path::PathBuf::from(":memory:")).unwrap();
        let now = now_rfc3339();
        let id: i64 = db
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO competitor_clip (room_id, up_uid, bvid, title, create_time, update_time) \
                     VALUES ('25034104', '82679456', 'BV1x', 't', ?1, ?1)",
                    [&now],
                )?;
                Ok(conn.last_insert_rowid())
            })
            .unwrap();

        set_status(&db, id, "DOWNLOADING", "").unwrap();
        let (status, _): (String, String) = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT status, COALESCE(status_note,'') FROM competitor_clip WHERE id = ?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
            })
            .unwrap();
        assert_eq!(status, "DOWNLOADING");

        attach_file(&db, id, "/tmp/a.mp4").unwrap();
        attach_asr(&db, id, "/tmp/a.json").unwrap();
        let (status, file, asr): (String, String, String) = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT status, file_path, asr_path FROM competitor_clip WHERE id = ?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
            })
            .unwrap();
        assert_eq!(status, "DONE");
        assert_eq!(file, "/tmp/a.mp4");
        assert_eq!(asr, "/tmp/a.json");
    }

    #[test]
    fn skipped_status_keeps_reason() {
        let db = Db::new(std::path::PathBuf::from(":memory:")).unwrap();
        let now = now_rfc3339();
        let id: i64 = db
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO competitor_clip (room_id, up_uid, bvid, title, create_time, update_time) \
                     VALUES ('25034104', '82679456', 'BV1y', 't', ?1, ?1)",
                    [&now],
                )?;
                Ok(conn.last_insert_rowid())
            })
            .unwrap();
        set_status(&db, id, "SKIPPED", "本场数据已清理").unwrap();
        let (status, note): (String, String) = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT status, status_note FROM competitor_clip WHERE id = ?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
            })
            .unwrap();
        assert_eq!(status, "SKIPPED");
        assert_eq!(note, "本场数据已清理");
    }
}
