//! 竞品切片发现：轮询竞品 UP 主的新投稿 → 关键字过滤 → 归属场次。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::competitor::CompetitorUp;
use crate::bilibili::client::BilibiliClient;
use crate::db::Db;
use crate::utils::now_rfc3339;

/// 竞品 UP 主的一条投稿（从 B站空间接口取回）
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpVideo {
    pub bvid: String,
    pub aid: i64,
    pub title: String,
    pub cover_url: String,
    pub pub_time: i64,
    pub duration: i64,
}

/// 关键字是否命中：忽略大小写的子串匹配。
/// 中文不需要分词，直接包含判断即可；英文统一小写避免大小写差异。
fn hit_any(haystack: &str, keywords: &[String]) -> bool {
    if keywords.is_empty() {
        return false;
    }
    let lower = haystack.to_lowercase();
    keywords
        .iter()
        .any(|kw| !kw.trim().is_empty() && lower.contains(&kw.trim().to_lowercase()))
}

/// 判定一条投稿是否属于该 UP 的竞品切片。
///
/// `relation = OR`：标题命中任一 或 标签命中任一
/// `relation = AND`：标题命中任一 且 标签命中任一
///
/// 标题和标签都为空时返回 false——没配规则就不该自动抓取任何东西。
pub fn matches_filter(up: &CompetitorUp, title: &str, tags: &[String]) -> bool {
    let title_hit = hit_any(title, &up.title_keywords);
    let tag_hit = tags.iter().any(|tag| hit_any(tag, &up.tag_keywords));
    let has_title_rule = !up.title_keywords.is_empty();
    let has_tag_rule = !up.tag_keywords.is_empty();

    match up.relation.as_str() {
        "AND" => {
            // 只配了其中一边时，另一边视为「不约束」，否则永远不可能命中
            match (has_title_rule, has_tag_rule) {
                (true, true) => title_hit && tag_hit,
                (true, false) => title_hit,
                (false, true) => tag_hit,
                (false, false) => false,
            }
        }
        _ => title_hit || tag_hit,
    }
}

/// 从 B站视频详情里抽标签名列表
fn extract_tags(value: &Value) -> Vec<String> {
    value
        .get("data")
        .and_then(|d| d.get("tag"))
        .and_then(|t| t.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().or_else(|| item.get("tag_name")?.as_str()))
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// 拉取某 UP 主最近的投稿列表（按发布时间倒序）。
pub async fn fetch_up_videos(
    client: &BilibiliClient,
    auth: Option<&crate::login_store::AuthInfo>,
    up_uid: &str,
    page_size: u32,
) -> Result<Vec<UpVideo>, String> {
    let params = vec![
        ("mid".to_string(), up_uid.to_string()),
        ("ps".to_string(), page_size.to_string()),
        ("pn".to_string(), "1".to_string()),
        ("order".to_string(), "pubdate".to_string()),
        ("platform".to_string(), "web".to_string()),
    ];
    let value = client
        .get_json(
            "https://api.bilibili.com/x/space/wbi/arc/search",
            &params,
            auth,
            true,
        )
        .await?;

    let code = value.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code != 0 {
        return Err(format!(
            "获取 UP 投稿列表失败: code={} {}",
            code,
            value.get("message").and_then(|v| v.as_str()).unwrap_or("")
        ));
    }
    let list = value
        .get("data")
        .and_then(|d| d.get("list"))
        .and_then(|l| l.get("vlist"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    Ok(list
        .iter()
        .filter_map(|item| {
            let bvid = item.get("bvid")?.as_str()?.to_string();
            Some(UpVideo {
                bvid,
                aid: item.get("aid").and_then(|v| v.as_i64()).unwrap_or(0),
                title: item
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                cover_url: item
                    .get("pic")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                pub_time: item.get("created").and_then(|v| v.as_i64()).unwrap_or(0),
                duration: item
                    .get("length")
                    .and_then(|v| v.as_str())
                    .and_then(|s| {
                        let parts: Vec<i64> =
                            s.split(':').filter_map(|p| p.parse().ok()).collect();
                        match parts.len() {
                            2 => Some(parts[0] * 60 + parts[1]),
                            3 => Some(parts[0] * 3600 + parts[1] * 60 + parts[2]),
                            _ => None,
                        }
                    })
                    .unwrap_or(0),
            })
        })
        .collect())
}

/// 拉取某条视频的标签
pub async fn fetch_video_tags(
    client: &BilibiliClient,
    auth: Option<&crate::login_store::AuthInfo>,
    bvid: &str,
) -> Vec<String> {
    let params = vec![("bvid".to_string(), bvid.to_string())];
    match client
        .get_json(
            "https://api.bilibili.com/x/tag/archive/tags",
            &params,
            auth,
            false,
        )
        .await
    {
        Ok(value) => extract_tags(&value),
        Err(_) => Vec::new(),
    }
}

/// 找到竞品投稿应归属的场次：投稿时间 **之前、最近结束** 的那一场。
///
/// 用「之前」而不是「时间差最小」，因为竞品只能切已经发生过的直播。
/// 返回 `(task_id, session_key)`；找不到返回 None。
pub fn resolve_session_for_pub_time(
    db: &Db,
    room_id: &str,
    pub_time: &str,
) -> Result<Option<(i64, String)>, String> {
    use rusqlite::OptionalExtension;
    db.with_conn(|conn| {
        // record_ids 是 JSON 数组（如 [4493,4494]），用 json_each 精确展开，
        // 再取这些记录里最晚的 end_time 作为该场次的结束时间。
        conn.query_row(
            "SELECT t.id, COALESCE(t.session_key, '') \
             FROM live_clip_task t \
             JOIN json_each(t.record_ids) AS j \
             JOIN live_record_task r ON r.id = j.value AND r.room_id = t.room_id \
             WHERE t.room_id = ?1 AND r.end_time IS NOT NULL AND r.end_time <= ?2 \
             GROUP BY t.id \
             ORDER BY MAX(r.end_time) DESC LIMIT 1",
            (room_id, pub_time),
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
    })
    .map_err(|err| format!("查询归属场次失败: {}", err))
}

/// 把一条通过过滤的竞品视频登记入库（已存在则跳过）。
pub fn upsert_clip(
    db: &Db,
    room_id: &str,
    up_uid: &str,
    video: &UpVideo,
    tags: &[String],
    task_id: Option<i64>,
    session_key: &str,
) -> Result<bool, String> {
    let now = now_rfc3339();
    let pub_time = chrono::DateTime::from_timestamp(video.pub_time, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default();
    let tags_json = serde_json::to_string(tags).unwrap_or_else(|_| "[]".into());
    let inserted = db
        .with_conn(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO competitor_clip \
                    (room_id, up_uid, bvid, aid, title, tags, cover_url, duration_secs, \
                     pub_time, task_id, session_key, status, create_time, update_time) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'PENDING', ?12, ?12)",
                rusqlite::params![
                    room_id,
                    up_uid,
                    &video.bvid,
                    video.aid,
                    &video.title,
                    &tags_json,
                    &video.cover_url,
                    video.duration as f64,
                    &pub_time,
                    task_id,
                    session_key,
                    &now,
                ],
            )
        })
        .map_err(|err| format!("登记竞品切片失败: {}", err))?;
    Ok(inserted > 0)
}

/// 扫描一个竞品 UP 的新投稿：抓列表 → 过滤 → 归属 → 入库。
/// 返回本次新登记的条数。
pub async fn scan_up(
    db: &Db,
    client: &BilibiliClient,
    auth: Option<&crate::login_store::AuthInfo>,
    up: &CompetitorUp,
    page_size: u32,
) -> Result<usize, String> {
    if !up.enabled {
        return Ok(0);
    }
    let videos = fetch_up_videos(client, auth, &up.up_uid, page_size).await?;
    let mut added = 0;
    for video in &videos {
        // 只处理配置时间之后的投稿，避免首次配置把历史稿件全抓一遍
        if !up.enabled_since.trim().is_empty() {
            let since = chrono::DateTime::parse_from_rfc3339(&up.enabled_since)
                .map(|dt| dt.timestamp())
                .unwrap_or(0);
            if video.pub_time < since {
                continue;
            }
        }
        let tags = fetch_video_tags(client, auth, &video.bvid).await;
        if !matches_filter(up, &video.title, &tags) {
            continue;
        }
        let pub_time = chrono::DateTime::from_timestamp(video.pub_time, 0)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default();
        let session = resolve_session_for_pub_time(db, &up.room_id, &pub_time)?;
        let (task_id, session_key) = match session {
            Some((id, key)) => (Some(id), key),
            None => (None, String::new()),
        };
        if upsert_clip(db, &up.room_id, &up.up_uid, video, &tags, task_id, &session_key)? {
            added += 1;
        }
    }
    Ok(added)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn up(relation: &str, titles: &[&str], tags: &[&str]) -> CompetitorUp {
        CompetitorUp {
            id: 1,
            room_id: "25034104".into(),
            up_uid: "82679456".into(),
            up_name: "竞品".into(),
            relation: relation.into(),
            title_keywords: titles.iter().map(|s| s.to_string()).collect(),
            tag_keywords: tags.iter().map(|s| s.to_string()).collect(),
            enabled: true,
            enabled_since: String::new(),
            last_scan_at: String::new(),
        }
    }

    #[test]
    fn or_relation_hits_either_side() {
        let u = up("OR", &["奶绿"], &["Reaction"]);
        assert!(matches_filter(&u, "奶绿的日常", &[]), "标题命中");
        assert!(matches_filter(&u, "随便什么", &["Reaction".into()]), "标签命中");
        assert!(!matches_filter(&u, "随便什么", &["其他".into()]), "都不命中");
    }

    #[test]
    fn and_relation_requires_both() {
        let u = up("AND", &["奶绿"], &["Reaction"]);
        assert!(matches_filter(&u, "奶绿的日常", &["Reaction".into()]));
        assert!(!matches_filter(&u, "奶绿的日常", &["其他".into()]), "缺标签不命中");
        assert!(!matches_filter(&u, "别人的视频", &["Reaction".into()]), "缺标题不命中");
    }

    #[test]
    fn and_relation_ignores_missing_side() {
        // 只配标题时，标签不参与约束
        let u = up("AND", &["奶绿"], &[]);
        assert!(matches_filter(&u, "奶绿的日常", &[]));
        // 只配标签时，标题不参与约束
        let u2 = up("AND", &[], &["Reaction"]);
        assert!(matches_filter(&u2, "随便", &["Reaction".into()]));
    }

    #[test]
    fn empty_rules_never_match() {
        let u = up("OR", &[], &[]);
        assert!(!matches_filter(&u, "任何标题", &["任何标签".into()]));
    }

    #[test]
    fn keywords_are_case_insensitive() {
        let u = up("OR", &["reaction"], &[]);
        assert!(matches_filter(&u, "Reaction 合集", &[]));
    }

    #[test]
    fn resolves_session_before_pub_time() {
        let db = crate::db::Db::new(std::path::PathBuf::from(":memory:")).unwrap();
        let now = "2026-11-22T12:00:00+00:00";
        db.with_conn(|conn| {
            // 真实格式：record_ids 是 JSON 数组，tasks 通过它关联到具体录播记录
            conn.execute(
                "INSERT INTO live_record_task (id, room_id, status, file_path, segment_index, start_time, end_time, create_time, update_time) \
                 VALUES (101, '25034104', 'STOPPED', '/a.mp4', 1, '2026-11-20T12:00:00+00:00', '2026-11-20T18:00:00+00:00', ?1, ?1)",
                [now],
            )?;
            conn.execute(
                "INSERT INTO live_record_task (id, room_id, status, file_path, segment_index, start_time, end_time, create_time, update_time) \
                 VALUES (102, '25034104', 'STOPPED', '/b.mp4', 1, '2026-11-22T12:00:00+00:00', '2026-11-22T20:00:00+00:00', ?1, ?1)",
                [now],
            )?;
            conn.execute(
                "INSERT INTO live_clip_task (id, room_id, record_ids, date_label, status, clip_count, create_time, update_time, session_key) \
                 VALUES (1, '25034104', '[101]', 'd1', 'DONE', 0, ?1, ?1, 's1')",
                [now],
            )?;
            conn.execute(
                "INSERT INTO live_clip_task (id, room_id, record_ids, date_label, status, clip_count, create_time, update_time, session_key) \
                 VALUES (2, '25034104', '[102]', 'd2', 'DONE', 0, ?1, ?1, 's2')",
                [now],
            )?;
            Ok(())
        })
        .unwrap();

        // 投稿时间在第二场之前 → 归第一场
        let r = resolve_session_for_pub_time(&db, "25034104", "2026-11-22T10:00:00+00:00")
            .unwrap()
            .unwrap();
        assert_eq!(r.1, "s1");

        // 投稿时间在第二场之后 → 归第二场
        let r = resolve_session_for_pub_time(&db, "25034104", "2026-11-23T01:00:00+00:00")
            .unwrap()
            .unwrap();
        assert_eq!(r.1, "s2");

        // 比所有场次都早 → 找不到
        assert!(resolve_session_for_pub_time(&db, "25034104", "2026-01-01T00:00:00+00:00")
            .unwrap()
            .is_none());
    }
}
