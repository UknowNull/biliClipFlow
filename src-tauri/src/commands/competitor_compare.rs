//! 竞品切片与本地切片的对比分析。
//!
//! 流程：
//! 1. 用竞品转写文本在本场 ASR 里做模糊匹配，定位到本场时间轴
//! 2. 与本地切片算交并比，判定完全覆盖 / 部分覆盖 / 未覆盖
//! 3. 未覆盖的再分四层归因：候选阶段 / 提示词规则 / ASR 缺失 / 声纹误判

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::Db;

/// 一段文本在本场时间轴上的位置
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocatedRange {
    pub start: f64,
    pub end: f64,
    /// 匹配到的本场文本（用于展示）
    pub text: String,
}

/// 覆盖判定
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Coverage {
    /// 交并比 ≥ 0.5，切点基本一致
    Full,
    /// 0 < 交并比 < 0.5，切到但边界偏差大
    Part,
    /// 完全没切
    Zero,
    /// 数据缺失，无法判定
    Skip,
}

/// 未覆盖的归因层级
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReasonLayer {
    /// 第 1 层：候选阶段就没进前 N 名
    Candidate,
    /// 第 2 层：进了候选但大模型判 keep=false
    Rule,
    /// 第 3 层：该时段没有 ASR 文本
    AsrMissing,
    /// 第 4 层：有文本但被判成原片
    Speaker,
}

/// 单条竞品的对比结果
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipComparison {
    pub competitor_clip_id: i64,
    pub bvid: String,
    pub title: String,
    pub up_uid: String,
    pub up_name: String,
    pub cover_url: String,
    pub duration_secs: f64,
    /// 竞品在本场对应的时间段；None 表示本场找不到对应内容
    pub located: Option<LocatedRange>,
    /// 与之重合度最高的本地切片
    pub mine_clip_id: Option<i64>,
    pub mine_title: String,
    pub mine_start: Option<f64>,
    pub mine_end: Option<f64>,
    pub iou: f64,
    pub coverage: Coverage,
    /// 未覆盖时的归因
    pub reason_layer: Option<ReasonLayer>,
    pub reason: String,
    /// 用户可采纳的优化建议
    pub suggestions: Vec<String>,
    pub status_note: String,
}

/// 交并比：(交集 / 并集)。两段完全不重叠时为 0。
pub fn iou(a_start: f64, a_end: f64, b_start: f64, b_end: f64) -> f64 {
    let inter = (a_end.min(b_end) - a_start.max(b_start)).max(0.0);
    if inter <= 0.0 {
        return 0.0;
    }
    let union = (a_end.max(b_end) - a_start.min(b_start)).max(0.0);
    if union <= 0.0 {
        0.0
    } else {
        inter / union
    }
}

/// 把文本切成用于匹配的字符集合（去掉标点、空白，统一小写）
fn normalize_for_match(text: &str) -> Vec<char> {
    text.chars()
        .filter(|c| !c.is_whitespace() && !is_punct(*c))
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn is_punct(c: char) -> bool {
    matches!(
        c,
        '，' | '。' | '！' | '？' | '；' | '：' | '、' | '（' | '）' | '《' | '》'
            | '“' | '”' | '‘' | '’' | ',' | '.' | '!' | '?' | ';' | ':' | '(' | ')'
            | '"' | '\'' | '-' | '—' | '…'
    )
}

/// 保序字符覆盖率：needle 的字符按顺序出现在 haystack 中的比例。
///
/// 用保序覆盖而不是整句相似度，是因为主播每次说话措辞都会有出入，
/// 但词序基本稳定；整句相似度会被语序微调大幅拉低分数。
pub fn ordered_coverage(needle: &[char], haystack: &[char]) -> f64 {
    if needle.is_empty() || haystack.is_empty() {
        return 0.0;
    }
    let mut cursor = 0usize;
    let mut hit = 0usize;
    for &c in needle {
        while cursor < haystack.len() {
            if haystack[cursor] == c {
                cursor += 1;
                hit += 1;
                break;
            }
            cursor += 1;
        }
        if cursor >= haystack.len() {
            break;
        }
    }
    hit as f64 / needle.len() as f64
}

/// 匹配阈值：低于此值认为本场没有对应内容
pub const LOCATE_THRESHOLD: f64 = 0.60;
/// 判定「完全覆盖」的交并比阈值
pub const FULL_IOU: f64 = 0.5;

/// 在本场 ASR 段落里定位竞品文本。
///
/// 做法：把竞品全文拆成若干窗口，逐个窗口在本场段落上滑动匹配，
/// 取覆盖率最高的连续段落区间作为定位结果。
pub fn locate_in_session(
    competitor_text: &str,
    segments: &[(f64, f64, String)],
) -> Option<LocatedRange> {
    let needle = normalize_for_match(competitor_text);
    if needle.len() < 8 || segments.is_empty() {
        return None;
    }
    // 用竞品文本的前 40 个字符做锚点，锚点命中再扩展整段
    let anchor: Vec<char> = needle.iter().take(40).copied().collect();

    let mut best: Option<(f64, usize)> = None;
    for (idx, (_, _, text)) in segments.iter().enumerate() {
        let hay = normalize_for_match(text);
        let score = ordered_coverage(&anchor, &hay);
        if best.map(|(s, _)| score > s).unwrap_or(true) {
            best = Some((score, idx));
        }
    }
    let (score, idx) = best?;
    if score < LOCATE_THRESHOLD {
        return None;
    }

    // 从命中的段落向前后扩展，直到覆盖率不再提升
    let mut lo = idx;
    let mut hi = idx;
    let mut current = score;
    loop {
        let mut improved = false;
        if hi + 1 < segments.len() {
            let joined: String = segments[lo..=hi + 1]
                .iter()
                .map(|(_, _, t)| t.as_str())
                .collect::<Vec<_>>()
                .join("");
            let s = ordered_coverage(&anchor, &normalize_for_match(&joined));
            if s > current {
                current = s;
                hi += 1;
                improved = true;
            }
        }
        if !improved && lo > 0 {
            let joined: String = segments[lo - 1..=hi]
                .iter()
                .map(|(_, _, t)| t.as_str())
                .collect::<Vec<_>>()
                .join("");
            let s = ordered_coverage(&anchor, &normalize_for_match(&joined));
            if s > current {
                current = s;
                lo -= 1;
            }
        }
        if !improved {
            break;
        }
    }

    Some(LocatedRange {
        start: segments[lo].0,
        end: segments[hi].1,
        text: segments[lo..=hi]
            .iter()
            .map(|(_, _, t)| t.as_str())
            .collect::<Vec<_>>()
            .join(" "),
    })
}

/// 某条本地切片（用于对比）
#[derive(Clone, Debug)]
pub struct LocalClip {
    pub id: i64,
    pub title: String,
    pub start: f64,
    pub end: f64,
}

/// 找与给定区间交并比最高的本地切片
pub fn best_local_clip(range: (f64, f64), clips: &[LocalClip]) -> Option<(LocalClip, f64)> {
    clips
        .iter()
        .map(|clip| (clip.clone(), iou(range.0, range.1, clip.start, clip.end)))
        .filter(|(_, score)| *score > 0.0)
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
}

/// 判定覆盖档位
pub fn coverage_of(iou_value: f64) -> Coverage {
    if iou_value >= FULL_IOU {
        Coverage::Full
    } else if iou_value > 0.0 {
        Coverage::Part
    } else {
        Coverage::Zero
    }
}

/// 分层归因：判断未覆盖时问题出在哪一层。
///
/// 顺序很重要——先看有没有进候选，再看规则，再看 ASR，最后看声纹，
/// 因为前一层不满足时后一层的判断没有意义。
pub fn attribute_missing(
    range: (f64, f64),
    session_segments: &[(f64, f64, String)],
    candidate_ranges: &[(f64, f64)],
    llm_rejected_ranges: &[(f64, f64)],
    max_candidates: usize,
) -> (ReasonLayer, String) {
    // 第 3 层优先：该时段根本没有文本，后面都无从谈起
    let has_text = session_segments
        .iter()
        .any(|(s, e, t)| *e > range.0 && *s < range.1 && !t.trim().is_empty());
    if !has_text {
        return (
            ReasonLayer::AsrMissing,
            "该时段转写为空（可能被背景音盖过人声），没有文本可送大模型，提示词无法解决".into(),
        );
    }

    // 第 1 层：候选阶段是否有覆盖该时段的候选
    let in_candidate = candidate_ranges
        .iter()
        .any(|(s, e)| *e > range.0 && *s < range.1);
    if !in_candidate {
        return (
            ReasonLayer::Candidate,
            format!(
                "该时段未进入候选前 {} 名，大模型根本没看到这段内容。可调整 max_candidates 或 signals 打分权重",
                max_candidates
            ),
        );
    }

    // 第 2 层：进了候选但大模型判不切
    let rejected = llm_rejected_ranges
        .iter()
        .any(|(s, e)| *e > range.0 && *s < range.1);
    if rejected {
        return (
            ReasonLayer::Rule,
            "该时段进了候选，但大模型判定不值得切，说明提示词规则与竞品判断不一致".into(),
        );
    }

    (
        ReasonLayer::Speaker,
        "该时段有文本也在候选内，但可能被声纹判成原片音轨，导致主播的话没有参与评分".into(),
    )
}

/// 按归因层级生成优化建议。第 3 层无提示词可改，返回空。
pub fn suggestions_for(
    layer: &ReasonLayer,
    competitor_dur: f64,
    full_mine: f64,
) -> Vec<String> {
    match layer {
        ReasonLayer::Candidate => vec![
            "把 max_candidates 从当前值适当提高，让靠后的候选也能进大模型".into(),
            "或加大 signals 打分里「弹幕突增」「笑声」的权重，提高这类段落的评分".into(),
        ],
        ReasonLayer::Rule => vec![
            "对照 BASE_RULES / REFINE_RULES，找出与竞品判断冲突的条目".into(),
            "常见冲突：对「主播只回应一两句」判死，而竞品认为这种短互动也值得切".into(),
        ],
        ReasonLayer::AsrMissing => Vec::new(),
        ReasonLayer::Speaker => vec![
            "检查声纹阈值是否过严，主播本人声音被判成原片会导致该段不参与评分".into(),
        ],
    }
    .into_iter()
    .chain(if (competitor_dur - full_mine).abs() > 3.0 && full_mine > 0.0 {
        vec![format!(
            "时长差异 {:.1} 秒：竞品 {:.1}s vs 你的 {:.1}s，可在 REFINE_RULES 里校准起止判断",
            (competitor_dur - full_mine).abs(),
            competitor_dur,
            full_mine
        )]
    } else {
        Vec::new()
    })
    .collect()
}

/// 从数据库读某场次的本地切片
pub fn load_local_clips(db: &Db, task_id: i64) -> Result<Vec<LocalClip>, String> {
    db.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, COALESCE(titles, ''), start_offset, end_offset \
             FROM live_clip_item WHERE task_id = ?1 AND status = 'SUCCESS' ORDER BY start_offset",
        )?;
        let rows = stmt.query_map([task_id], |row| {
            let titles: String = row.get(1)?;
            let title = serde_json::from_str::<Vec<String>>(&titles)
                .ok()
                .and_then(|items| items.first().cloned())
                .unwrap_or_default();
            Ok(LocalClip {
                id: row.get(0)?,
                title,
                start: row.get(2)?,
                end: row.get(3)?,
            })
        })?;
        rows.collect()
    })
    .map_err(|err| format!("读取本地切片失败: {}", err))
}

/// 从本场 ASR 缓存里读出段落 (start, end, text)
pub fn session_segments(pass: &Value) -> Vec<(f64, f64, String)> {
    pass.get("segments")
        .and_then(|s| s.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let start = item.get("start")?.as_f64()?;
                    let end = item.get("end")?.as_f64()?;
                    let text = item
                        .get("zh_text")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .or_else(|| item.get("text")?.as_str())
                        .unwrap_or("")
                        .to_string();
                    Some((start, end, text))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iou_basics() {
        assert!((iou(0.0, 10.0, 0.0, 10.0) - 1.0).abs() < 1e-9);
        assert!((iou(0.0, 10.0, 5.0, 15.0) - 1.0 / 3.0).abs() < 1e-6);
        assert_eq!(iou(0.0, 10.0, 20.0, 30.0), 0.0);
        assert_eq!(iou(10.0, 20.0, 0.0, 10.0), 0.0, "端点相接不算重叠");
    }

    #[test]
    fn coverage_levels() {
        assert_eq!(coverage_of(0.92), Coverage::Full);
        assert_eq!(coverage_of(0.50), Coverage::Full);
        assert_eq!(coverage_of(0.41), Coverage::Part);
        assert_eq!(coverage_of(0.0), Coverage::Zero);
    }

    #[test]
    fn ordered_coverage_tolerates_extra_words() {
        let needle = normalize_for_match("奶姐毒奶中科大考研");
        let hay = normalize_for_match("对，奶姐这次又毒奶了中科大考研的事");
        assert!(ordered_coverage(&needle, &hay) > 0.9, "插入词不应大幅降低覆盖率");

        let unrelated = normalize_for_match("今天天气不错适合出门");
        assert!(ordered_coverage(&needle, &unrelated) < 0.5);
    }

    #[test]
    fn locates_competitor_text_in_session() {
        let segments = vec![
            (10.0, 15.0, "今天我们来聊点别的".to_string()),
            (100.0, 110.0, "奶姐毒奶中科大考研你包考不上的别考了".to_string()),
            (110.0, 120.0, "这话说出去就收不回来了".to_string()),
            (200.0, 210.0, "完全无关的内容".to_string()),
        ];
        let located = locate_in_session("奶姐毒奶中科大考研，你包考不上的别考了", &segments)
            .expect("应能定位");
        assert!((located.start - 100.0).abs() < 1e-6);
        assert!(located.end >= 110.0);
    }

    #[test]
    fn returns_none_when_content_absent() {
        let segments = vec![(10.0, 15.0, "今天我们来聊点别的".to_string())];
        assert!(locate_in_session("奶姐毒奶中科大考研", &segments).is_none());
    }

    #[test]
    fn best_local_clip_picks_highest_overlap() {
        let clips = vec![
            LocalClip { id: 1, title: "A".into(), start: 0.0, end: 30.0 },
            LocalClip { id: 2, title: "B".into(), start: 100.0, end: 160.0 },
            LocalClip { id: 3, title: "C".into(), start: 500.0, end: 510.0 },
        ];
        let (clip, score) = best_local_clip((105.0, 155.0), &clips).unwrap();
        assert_eq!(clip.id, 2);
        assert!(score > 0.8);
    }

    #[test]
    fn attribution_prefers_asr_missing_first() {
        let segments = vec![(100.0, 110.0, "".to_string())];
        let (layer, msg) = attribute_missing((100.0, 110.0), &segments, &[], &[], 30);
        assert_eq!(layer, ReasonLayer::AsrMissing);
        assert!(msg.contains("转写为空"));
    }

    #[test]
    fn attribution_detects_candidate_miss() {
        let segments = vec![(100.0, 110.0, "有文本".to_string())];
        let (layer, msg) = attribute_missing((100.0, 110.0), &segments, &[], &[], 30);
        assert_eq!(layer, ReasonLayer::Candidate);
        assert!(msg.contains("30"));
    }

    #[test]
    fn attribution_detects_rule_rejection() {
        let segments = vec![(100.0, 110.0, "有文本".to_string())];
        let candidates = vec![(95.0, 115.0)];
        let rejected = vec![(95.0, 115.0)];
        let (layer, _) = attribute_missing((100.0, 110.0), &segments, &candidates, &rejected, 30);
        assert_eq!(layer, ReasonLayer::Rule);
    }

    #[test]
    fn no_suggestions_for_asr_missing() {
        let tips = suggestions_for(&ReasonLayer::AsrMissing, 60.0, 0.0);
        assert!(tips.is_empty(), "ASR 缺失属于识别问题，不该给提示词建议");
    }

    #[test]
    fn duration_gap_generates_tip() {
        let tips = suggestions_for(&ReasonLayer::Rule, 55.4, 28.1);
        assert!(tips.iter().any(|t| t.contains("时长差异")));
    }
}
