//! 竞品分析：配置竞品 UP 主、发现并下载竞品切片、与本地切片对比。
//!
//! 数据只属于单个主播（`room_id`），不同主播的竞品配置互不影响。
//! 竞品切片的归属依据是「投稿时间之前、最近结束的那一场」，不依赖历史 ASR 数据。

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::api::ApiResponse;
use crate::db::Db;
use crate::AppState;
use crate::utils::now_rfc3339;

/// 竞品 UP 主配置。标题与标签的关系由 `relation` 决定。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompetitorUp {
    pub id: i64,
    pub room_id: String,
    pub up_uid: String,
    pub up_name: String,
    /// OR：标题或标签任一命中；AND：两者都要命中
    pub relation: String,
    pub title_keywords: Vec<String>,
    pub tag_keywords: Vec<String>,
    pub enabled: bool,
    /// 只处理此时间之后的投稿，避免首次配置时把历史稿件全抓一遍
    pub enabled_since: String,
    pub last_scan_at: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompetitorUpInput {
    pub room_id: String,
    pub up_uid: String,
    #[serde(default)]
    pub up_name: String,
    #[serde(default)]
    pub relation: String,
    #[serde(default)]
    pub title_keywords: Vec<String>,
    #[serde(default)]
    pub tag_keywords: Vec<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

fn parse_keywords(raw: Option<String>) -> Vec<String> {
    raw.and_then(|value| serde_json::from_str::<Vec<String>>(&value).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

fn dump_keywords(items: &[String]) -> String {
    let cleaned: Vec<String> = items
        .iter()
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect();
    serde_json::to_string(&cleaned).unwrap_or_else(|_| "[]".to_string())
}

/// 读取某主播名下全部竞品 UP 配置，按创建顺序返回。
pub fn list_for_room(db: &Db, room_id: &str) -> Result<Vec<CompetitorUp>, String> {
    db.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, room_id, up_uid, up_name, relation, title_keywords, tag_keywords, \
                    enabled, enabled_since, last_scan_at \
             FROM competitor_up WHERE room_id = ?1 ORDER BY id ASC",
        )?;
        let rows = stmt.query_map([room_id], |row| {
            Ok(CompetitorUp {
                id: row.get(0)?,
                room_id: row.get(1)?,
                up_uid: row.get(2)?,
                up_name: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                relation: row.get::<_, Option<String>>(4)?.unwrap_or_else(|| "OR".into()),
                title_keywords: parse_keywords(row.get(5)?),
                tag_keywords: parse_keywords(row.get(6)?),
                enabled: row.get::<_, Option<i64>>(7)?.unwrap_or(1) != 0,
                enabled_since: row.get::<_, Option<String>>(8)?.unwrap_or_default(),
                last_scan_at: row.get::<_, Option<String>>(9)?.unwrap_or_default(),
            })
        })?;
        rows.collect()
    })
    .map_err(|err| format!("读取竞品配置失败: {}", err))
}

/// 保存（新增或更新）一条竞品 UP 配置。
/// `enabled_since` 只在首次创建时写入，后续编辑保持不变，避免重复回填历史。
pub fn save(db: &Db, input: &CompetitorUpInput) -> Result<i64, String> {
    let room_id = input.room_id.trim();
    let up_uid = input.up_uid.trim();
    if room_id.is_empty() {
        return Err("房间号不能为空".into());
    }
    if up_uid.is_empty() {
        return Err("竞品 UP 的 UID 不能为空".into());
    }
    let relation = match input.relation.trim().to_ascii_uppercase().as_str() {
        "AND" => "AND",
        _ => "OR",
    };
    let now = now_rfc3339();
    let titles = dump_keywords(&input.title_keywords);
    let tags = dump_keywords(&input.tag_keywords);
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO competitor_up \
                (room_id, up_uid, up_name, relation, title_keywords, tag_keywords, enabled, enabled_since, create_time, update_time) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9) \
             ON CONFLICT(room_id, up_uid) DO UPDATE SET \
                up_name = excluded.up_name, \
                relation = excluded.relation, \
                title_keywords = excluded.title_keywords, \
                tag_keywords = excluded.tag_keywords, \
                enabled = excluded.enabled, \
                update_time = excluded.update_time",
            (
                room_id,
                up_uid,
                input.up_name.trim(),
                relation,
                &titles,
                &tags,
                if input.enabled { 1 } else { 0 },
                &now,
                &now,
            ),
        )?;
        let id: i64 = conn.query_row(
            "SELECT id FROM competitor_up WHERE room_id = ?1 AND up_uid = ?2",
            (room_id, up_uid),
            |r| r.get(0),
        )?;
        Ok(id)
    })
    .map_err(|err| format!("保存竞品配置失败: {}", err))
}

/// 删除一条竞品配置。连同该 UP 已抓取的竞品切片一起清掉，
/// 避免配置删了但数据还留着的孤儿记录。
pub fn delete(db: &Db, room_id: &str, id: i64) -> Result<(), String> {
    let room_id = room_id.trim();
    db.with_conn_mut(|conn| {
        let tx = conn.transaction()?;
        let up_uid: Option<String> = tx
            .query_row(
                "SELECT up_uid FROM competitor_up WHERE id = ?1 AND room_id = ?2",
                (id, room_id),
                |r| r.get(0),
            )
            .ok();
        if let Some(uid) = up_uid {
            tx.execute(
                "DELETE FROM competitor_clip WHERE room_id = ?1 AND up_uid = ?2",
                (room_id, &uid),
            )?;
        }
        tx.execute(
            "DELETE FROM competitor_up WHERE id = ?1 AND room_id = ?2",
            (id, room_id),
        )?;
        tx.commit()?;
        Ok(())
    })
    .map_err(|err| format!("删除竞品配置失败: {}", err))
}

/// 记录本次扫描时间，用于排查「为什么没抓到新投稿」。
pub fn save_scan_time(db: &Db, id: i64) -> Result<(), String> {
    let now = now_rfc3339();
    db.with_conn(|conn| {
        conn.execute(
            "UPDATE competitor_up SET last_scan_at = ?1, update_time = ?1 WHERE id = ?2",
            (&now, id),
        )?;
        Ok(())
    })
    .map_err(|err| format!("记录扫描时间失败: {}", err))?;
    Ok(())
}

// ============ Tauri 命令 ============

#[tauri::command(async)]
pub fn competitor_up_list(state: State<'_, AppState>, room_id: String) -> ApiResponse<Vec<CompetitorUp>> {
    match list_for_room(&state.db, room_id.trim()) {
        Ok(items) => ApiResponse::success(items),
        Err(err) => ApiResponse::error(err),
    }
}

#[tauri::command(async)]
pub fn competitor_up_save(state: State<'_, AppState>, input: CompetitorUpInput) -> ApiResponse<i64> {
    match save(&state.db, &input) {
        Ok(id) => ApiResponse::success(id),
        Err(err) => ApiResponse::error(err),
    }
}

#[tauri::command(async)]
pub fn competitor_up_delete(state: State<'_, AppState>, room_id: String, id: i64) -> ApiResponse<bool> {
    match delete(&state.db, &room_id, id) {
        Ok(_) => ApiResponse::success(true),
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

    fn input(room: &str, uid: &str) -> CompetitorUpInput {
        CompetitorUpInput {
            room_id: room.into(),
            up_uid: uid.into(),
            up_name: "竞品A".into(),
            relation: "or".into(),
            title_keywords: vec!["奶绿".into(), "  ".into(), "奶姐".into()],
            tag_keywords: vec!["Reaction".into()],
            enabled: true,
        }
    }

    #[test]
    fn saves_and_lists_competitor_ups() {
        let db = mem_db();
        save(&db, &input("25034104", "3546735668366092")).unwrap();
        let list = list_for_room(&db, "25034104").unwrap();
        assert_eq!(list.len(), 1);
        let item = &list[0];
        // 空白关键字被丢弃
        assert_eq!(item.title_keywords, vec!["奶绿", "奶姐"]);
        assert_eq!(item.tag_keywords, vec!["Reaction"]);
        assert_eq!(item.relation, "OR");
        assert!(item.enabled);
        // 首次创建写入 enabled_since
        assert!(!item.enabled_since.is_empty());
    }

    #[test]
    fn same_uid_updates_instead_of_duplicating() {
        let db = mem_db();
        let id1 = save(&db, &input("25034104", "82679456")).unwrap();
        let mut second = input("25034104", "82679456");
        second.relation = "and".into();
        second.title_keywords = vec!["七海".into()];
        let id2 = save(&db, &second).unwrap();
        assert_eq!(id1, id2, "同一 UP 应更新而非新增");
        let list = list_for_room(&db, "25034104").unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].relation, "AND");
        assert_eq!(list[0].title_keywords, vec!["七海"]);
    }

    #[test]
    fn rooms_are_isolated() {
        let db = mem_db();
        save(&db, &input("roomA", "up1")).unwrap();
        save(&db, &input("roomB", "up2")).unwrap();
        assert_eq!(list_for_room(&db, "roomA").unwrap().len(), 1);
        assert_eq!(list_for_room(&db, "roomB").unwrap().len(), 1);
        assert_eq!(list_for_room(&db, "roomC").unwrap().len(), 0);
    }

    #[test]
    fn delete_removes_config_and_its_clips() {
        let db = mem_db();
        let id = save(&db, &input("25034104", "82679456")).unwrap();
        let now = now_rfc3339();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO competitor_clip (room_id, up_uid, bvid, title, create_time, update_time) \
                 VALUES ('25034104', '82679456', 'BV1test', 't', ?1, ?1)",
                [&now],
            )?;
            Ok(())
        })
        .unwrap();
        delete(&db, "25034104", id).unwrap();
        assert!(list_for_room(&db, "25034104").unwrap().is_empty());
        let clips: i64 = db
            .with_conn(|conn| {
                conn.query_row("SELECT COUNT(*) FROM competitor_clip", [], |r| r.get(0))
            })
            .unwrap();
        assert_eq!(clips, 0, "删除配置时应一并清掉该 UP 的竞品切片");
    }

    #[test]
    fn reassign_voids_old_report() {
        let db = mem_db();
        let now = now_rfc3339();
        // 两条任务 + 一条竞品切片 + 一条旧报告
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO live_clip_task (id, room_id, record_ids, date_label, status, clip_count, create_time, update_time, session_key) \
                 VALUES (1, 'r', '[]', 'd1', 'DONE', 0, ?1, ?1, 's1')",
                [&now],
            )?;
            conn.execute(
                "INSERT INTO live_clip_task (id, room_id, record_ids, date_label, status, clip_count, create_time, update_time, session_key) \
                 VALUES (2, 'r', '[]', 'd2', 'DONE', 0, ?1, ?1, 's2')",
                [&now],
            )?;
            conn.execute(
                "INSERT INTO competitor_clip (room_id, up_uid, bvid, title, task_id, session_key, create_time, update_time) \
                 VALUES ('r', 'u', 'BV1a', 't', 1, 's1', ?1, ?1)",
                [&now],
            )?;
            conn.execute(
                "INSERT INTO competitor_compare (task_id, room_id, status, create_time, update_time) \
                 VALUES (1, 'r', 'DONE', ?1, ?1)",
                [&now],
            )?;
            Ok(())
        })
        .unwrap();

        let clip_id: i64 = db
            .with_conn(|conn| conn.query_row("SELECT id FROM competitor_clip", [], |r| r.get(0)))
            .unwrap();

        reassign(&db, "r", clip_id, 2, "s2").unwrap();

        let (task, status): (i64, String) = db
            .with_conn(|conn| {
                let t = conn.query_row(
                    "SELECT task_id FROM competitor_clip WHERE id = ?1", [clip_id], |r| r.get(0))?;
                let s = conn.query_row(
                    "SELECT status FROM competitor_compare WHERE task_id = 1", [], |r| r.get(0))?;
                Ok((t, s))
            })
            .unwrap();
        assert_eq!(task, 2, "归属应改到新场次");
        assert_eq!(status, "VOID", "旧场次报告应作废");
    }

    #[test]
    fn reassign_to_same_task_keeps_report() {
        let db = mem_db();
        let now = now_rfc3339();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO competitor_clip (room_id, up_uid, bvid, title, task_id, session_key, create_time, update_time) \
                 VALUES ('r', 'u', 'BV1b', 't', 1, 's1', ?1, ?1)",
                [&now],
            )?;
            conn.execute(
                "INSERT INTO competitor_compare (task_id, room_id, status, create_time, update_time) \
                 VALUES (1, 'r', 'DONE', ?1, ?1)",
                [&now],
            )?;
            Ok(())
        })
        .unwrap();
        let clip_id: i64 = db
            .with_conn(|conn| conn.query_row("SELECT id FROM competitor_clip", [], |r| r.get(0)))
            .unwrap();
        reassign(&db, "r", clip_id, 1, "s1").unwrap();
        let status: String = db
            .with_conn(|conn| {
                conn.query_row("SELECT status FROM competitor_compare WHERE task_id = 1", [], |r| r.get(0))
            })
            .unwrap();
        assert_eq!(status, "DONE", "归属没变时报告不该作废");
    }

    /// 竞品挂在较早的渐进 task 上时，按最新 task 的场次也要能查到。
    #[test]
    fn clip_list_follows_record_overlap_not_exact_task() {
        let db = mem_db();
        let now = now_rfc3339();
        db.with_conn(|conn| {
            for rid in [1, 2, 3] {
                conn.execute(
                    "INSERT INTO live_record_task (id, room_id, status, file_path, segment_index, \
                        start_time, end_time, create_time, update_time) \
                     VALUES (?1, 'r', 'STOPPED', ?2, 1, '2026-11-24T10:00:00+00:00', \
                        '2026-11-24T12:00:00+00:00', ?3, ?3)",
                    (rid, format!("/rec/{}.mp4", rid), &now),
                )?;
            }
            // 早期任务：只含分段 1
            conn.execute(
                "INSERT INTO live_clip_task (id, room_id, record_ids, date_label, status, clip_count, \
                    create_time, update_time, session_key) VALUES (1, 'r', '[1]', 'd', 'FAILED', 0, ?1, ?1, 's')",
                [&now],
            )?;
            // 最新任务：含全部 3 个分段
            conn.execute(
                "INSERT INTO live_clip_task (id, room_id, record_ids, date_label, status, clip_count, \
                    create_time, update_time, session_key) VALUES (2, 'r', '[1,2,3]', 'd', 'RUNNING', 0, ?1, ?1, 's')",
                [&now],
            )?;
            // 竞品挂在早期任务上
            conn.execute(
                "INSERT INTO competitor_clip (room_id, up_uid, bvid, title, task_id, create_time, update_time) \
                 VALUES ('r', 'u', 'BV1a', 't', 1, ?1, ?1)",
                [&now],
            )?;
            Ok(())
        })
        .unwrap();

        // 用最新 task(2) 查，应能查到挂在 task(1) 上的竞品
        let count: i64 = db
            .with_conn(|conn| {
                conn.query_row(
                    "WITH session_records AS ( \
                       SELECT CAST(j.value AS INTEGER) AS rid \
                       FROM live_clip_task t JOIN json_each(t.record_ids) j WHERE t.id = 2 \
                     ) \
                     SELECT COUNT(*) FROM competitor_clip c \
                     JOIN live_clip_task t2 ON t2.id = c.task_id \
                     JOIN json_each(t2.record_ids) j2 ON j2.value IN (SELECT rid FROM session_records) \
                     WHERE c.room_id = 'r'",
                    [],
                    |r| r.get(0),
                )
            })
            .unwrap();
        assert_eq!(count, 1, "按场次分段应能关联到早期任务的竞品");
    }

    #[test]
    fn rejects_empty_uid() {
        let db = mem_db();
        let mut bad = input("25034104", "82679456");
        bad.up_uid = "   ".into();
        assert!(save(&db, &bad).is_err());
    }
}


/// 修改竞品切片的归属场次。
///
/// 归属变了，旧报告就不能再用——标记 VOID，由调用方触发新场次重新对比。
pub fn reassign(
    db: &Db,
    room_id: &str,
    clip_id: i64,
    task_id: i64,
    session_key: &str,
) -> Result<(), String> {
    let now = now_rfc3339();
    db.with_conn_mut(|conn| {
        let tx = conn.transaction()?;
        let old_task: Option<i64> = tx
            .query_row(
                "SELECT task_id FROM competitor_clip WHERE id = ?1 AND room_id = ?2",
                (clip_id, room_id),
                |r| r.get::<_, Option<i64>>(0),
            )
            .ok()
            .flatten();
        tx.execute(
            "UPDATE competitor_clip SET task_id = ?1, session_key = ?2, update_time = ?3 \
             WHERE id = ?4 AND room_id = ?5",
            (task_id, session_key, &now, clip_id, room_id),
        )?;
        // 旧场次的报告作废
        if let Some(old) = old_task {
            if old != task_id {
                tx.execute(
                    "UPDATE competitor_compare SET status = 'VOID', update_time = ?1 \
                     WHERE task_id = ?2 AND status = 'DONE'",
                    (&now, old),
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    })
    .map_err(|err| format!("修改归属场次失败: {}", err))?;
    Ok(())
}

#[tauri::command(async)]
pub fn competitor_clip_reassign(
    state: State<'_, AppState>,
    room_id: String,
    clip_id: i64,
    task_id: i64,
    session_key: String,
) -> ApiResponse<bool> {
    match reassign(&state.db, &room_id, clip_id, task_id, &session_key) {
        Ok(_) => ApiResponse::success(true),
        Err(err) => ApiResponse::error(err),
    }
}


/// 竞品 UP 主的基本信息，用于配置弹窗回显头像和昵称。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompetitorUpInfo {
    pub up_uid: String,
    pub up_name: String,
    pub avatar_url: String,
}

/// 查 B站用户信息（昵称 + 头像）。
/// 用主播信息同一套接口，避免为竞品单独引一套调用。
#[tauri::command(async)]
pub async fn competitor_up_info(
    state: State<'_, AppState>,
    up_uid: String,
) -> Result<ApiResponse<CompetitorUpInfo>, String> {
    let uid = up_uid.trim().to_string();
    if uid.is_empty() {
        return Ok(ApiResponse::error("UID 不能为空"));
    }
    let params = vec![("uid".to_string(), uid.clone())];
    let data = match state
        .bilibili
        .get_json(
            "https://api.live.bilibili.com/live_user/v1/Master/info",
            &params,
            None,
            false,
        )
        .await
    {
        Ok(value) => value,
        Err(err) => return Ok(ApiResponse::error(format!("查询 UP 信息失败: {}", err))),
    };
    let info = data.get("info").cloned().unwrap_or(serde_json::Value::Null);
    let name = info
        .get("uname")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let avatar = info
        .get("face")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if name.is_empty() {
        return Ok(ApiResponse::error(format!("UID {} 查不到用户信息", uid)));
    }
    Ok(ApiResponse::success(CompetitorUpInfo {
        up_uid: uid,
        up_name: name,
        avatar_url: avatar,
    }))
}


/// 竞品切片按「录播分段」统计，返回 record_id → 数量。
///
/// 不能按 task_id 统计：同一场直播在录制过程中会产生多条渐进式切片任务
/// （分段数 4 → 9 → 13 → 21 → 28 递增），而录播管理按场次显示、只绑定其中
/// 一条，按 task 统计会出现「竞品存在但按钮不显示」。用分段 ID 做键，
/// 前端可以按场次包含的分段集合汇总。
#[tauri::command(async)]
pub fn competitor_clip_counts(
    state: State<'_, AppState>,
    room_id: String,
) -> ApiResponse<std::collections::HashMap<String, i64>> {
    let result = state.db.with_conn(|conn| {
        // 把竞品的 task 展开成它包含的每个 record_id，再按 record_id 计数。
        // 同一竞品可能命中多个分段，用 DISTINCT 防止重复计数。
        let mut stmt = conn.prepare(
            "SELECT CAST(j.value AS TEXT) AS rid, COUNT(DISTINCT c.id) \
             FROM competitor_clip c \
             JOIN live_clip_task t ON t.id = c.task_id \
             JOIN json_each(t.record_ids) j \
             WHERE c.room_id = ?1 AND c.task_id IS NOT NULL \
             GROUP BY j.value",
        )?;
        let rows = stmt.query_map([room_id.trim()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        rows.collect::<Result<std::collections::HashMap<String, i64>, _>>()
    });
    match result {
        Ok(map) => ApiResponse::success(map),
        Err(err) => ApiResponse::error(format!("统计竞品切片失败: {}", err)),
    }
}


/// 竞品切片列表项，供「竞品切片」页面展示。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompetitorClipItem {
    pub id: i64,
    pub bvid: String,
    pub title: String,
    pub up_uid: String,
    pub up_name: String,
    pub cover_url: String,
    pub duration_secs: f64,
    pub pub_time: String,
    pub status: String,
    pub status_note: String,
    pub file_path: String,
}

/// 本场归属的竞品切片列表（按投稿时间倒序）。
///
/// 按「场次的分段集合」查，而不是按 task_id 精确匹配：
/// 同一场直播录制中会产生多条渐进式切片任务（分段 4→9→13→21→28 递增），
/// 竞品可能归属到较早的某条，而录播管理只绑定最新那条，精确匹配会一条都查不到。
#[tauri::command(async)]
pub fn competitor_clip_list(
    state: State<'_, AppState>,
    room_id: String,
    task_id: i64,
) -> ApiResponse<Vec<CompetitorClipItem>> {
    let result = state.db.with_conn(|conn| {
        // 先取本场次的分段集合，再找「分段与之有交集」的所有切片任务
        let mut stmt = conn.prepare(
            "WITH session_records AS ( \
               SELECT CAST(j.value AS INTEGER) AS rid \
               FROM live_clip_task t JOIN json_each(t.record_ids) j WHERE t.id = ?2 \
             ), \
             related_tasks AS ( \
               SELECT DISTINCT c.task_id \
               FROM competitor_clip c \
               JOIN live_clip_task t2 ON t2.id = c.task_id \
               JOIN json_each(t2.record_ids) j2 ON j2.value IN (SELECT rid FROM session_records) \
               WHERE c.room_id = ?1 AND c.task_id IS NOT NULL \
             ) \
             SELECT c.id, c.bvid, COALESCE(c.title,''), c.up_uid, COALESCE(u.up_name,''), \
                    COALESCE(c.cover_url,''), COALESCE(c.duration_secs,0), COALESCE(c.pub_time,''), \
                    c.status, COALESCE(c.status_note,''), COALESCE(c.file_path,'') \
             FROM competitor_clip c \
             LEFT JOIN competitor_up u ON u.room_id = c.room_id AND u.up_uid = c.up_uid \
             WHERE c.room_id = ?1 AND c.task_id IN (SELECT task_id FROM related_tasks) \
             ORDER BY c.pub_time DESC, c.id DESC",
        )?;
        let rows = stmt.query_map((room_id.trim(), task_id), |r| {
            Ok(CompetitorClipItem {
                id: r.get(0)?,
                bvid: r.get(1)?,
                title: r.get(2)?,
                up_uid: r.get(3)?,
                up_name: r.get(4)?,
                cover_url: r.get(5)?,
                duration_secs: r.get(6)?,
                pub_time: r.get(7)?,
                status: r.get(8)?,
                status_note: r.get(9)?,
                file_path: r.get(10)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
    });
    match result {
        Ok(items) => ApiResponse::success(items),
        Err(err) => ApiResponse::error(format!("读取竞品切片失败: {}", err)),
    }
}
