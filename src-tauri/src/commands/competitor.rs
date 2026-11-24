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
    fn rejects_empty_uid() {
        let db = mem_db();
        let mut bad = input("25034104", "82679456");
        bad.up_uid = "   ".into();
        assert!(save(&db, &bad).is_err());
    }
}
