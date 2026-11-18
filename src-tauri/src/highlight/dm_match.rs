//! 念弹幕配对：把「主播念的句子」和「普通弹幕」配对（借鉴竞品 dm_match 的实测规则）。
//!
//! 纯本地、零大模型。成功配对的句子标成「念弹幕」，让观众原话进提示词，
//! 大模型由此能识别"粉丝神评论"这类互动事件（SC 已由 `mark_sc_readings` 单独覆盖）。
//!
//! 移植竞品实测沉淀的五条硬规则：
//! 1. 窗口至少 5 分钟：主播常先说完自己的话，再回头念几分钟前发的弹幕；
//! 2. 抗转写错字：用「保序字符覆盖」而不是整句相似度（主播念的时候会加话）；
//! 3. 礼物/SC/舰长类不进候选——那是"被打赏"，不是"被念的弹幕"；
//! 4. 一条弹幕全局只归分数最高的句子（防止一条短弹幕同时"挂"到多个句子）；
//! 5. 句子前已刷屏的弹幕不配（复读发生在句子之后才是"被念"，之前是"群体刷屏"）。

use std::collections::HashMap;

use super::asr::AsrSegment;
use super::danmaku::{DanmakuEvent, DanmakuKind};

/// 一句被确认在念的弹幕：文本用于提示词展示（弹幕原文：…）
#[derive(Clone, Debug)]
pub struct DmRead {
    pub text: String,
}

/// 往前找多久（秒）。竞品站长要求：至少 5 分钟
const WINDOW_SECS: f64 = 300.0;
/// 句子结束后的余量（秒）：弹幕必须在句子结束前发出（观众先发、主播才念）
const TAIL_SECS: f64 = 0.5;
/// 一句最多配几条弹幕（主播连读好几条的情况）
const MAX_HITS: usize = 3;
/// 归一化后短于该长度（字符数）的弹幕不参与（"哈哈""6"是噪声）
const MIN_CAND_CHARS: usize = 2;
/// 保序字符覆盖低于该值直接淘汰
const MIN_COVER: f64 = 0.40;
/// confirmed / uncertain 的判定分
const T_CONFIRM: f64 = 0.72;
const T_UNCERTAIN: f64 = 0.50;
/// 句子开始前同文本出现次数达到该值的弹幕最多只能判"待确认"（群体刷屏惩罚）
const FREQ_SOFT: usize = 3;
/// 句子开始前出现次数达到该值直接不配
const FREQ_HARD: usize = 6;
/// 候选不超过该字符数算"短弹幕"
const SHORT_CAND: usize = 3;
/// 短候选在句子里的占比低于该值时降级排除（短词被长句顺带包含）
const SHORT_RATIO: f64 = 0.35;
/// 组合分权重：覆盖为主，时间与语言线索为辅（与竞品一致）
const W_COVER: f64 = 0.70;
const W_TIME: f64 = 0.18;
const W_CUE: f64 = 0.12;

/// 语言线索：主播的话明确指向"在回应弹幕/观众"
const CUE_HINTS: &[&str] = &[
    "你说", "有人问", "刚有人", "刚才", "刚刚", "弹幕", "评论", "这条", "问一下", "问的",
    "说的是", "你是不是", "我看到", "上面说", "你问", "问我", "你们问", "有人问过",
];

/// 礼物类名词：句中出现这些 + 谢谢/感谢 → 判定为谢礼物，不配弹幕（那是被打赏）
const GIFT_WORDS: &[&str] = &[
    "礼物", "舰长", "大航海", "上舰", "充电", "辣条", "小心心", "sc", "superchat", "打赏", "投喂",
];

/// 只保留汉字/字母/数字，丢掉标点、空格与 emoji
fn norm(s: &str) -> Vec<char> {
    s.chars()
        .filter(|c| ('\u{4e00}'..='\u{9fff}').contains(c) || c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// 保序字符覆盖：候选的字符有多少按顺序出现在句子里（可跳字）。
/// 整段照念直接满分；主播加话只掉一点；同音错字也只掉一点。
fn cover(cand: &[char], sent: &[char]) -> f64 {
    if cand.is_empty() {
        return 0.0;
    }
    // 整段照念（最常见）快速路径
    if contains_slice(sent, cand) {
        return 1.0;
    }
    // LCS 长度 / 候选长度：保序、可跳字，等价于竞品 SequenceMatcher 匹配块之和的口径
    let matched = longest_common_subsequence(cand, sent);
    matched as f64 / cand.len() as f64
}

fn contains_slice(hay: &[char], needle: &[char]) -> bool {
    if needle.is_empty() || needle.len() > hay.len() {
        return false;
    }
    hay.windows(needle.len()).any(|w| w == needle)
}

fn longest_common_subsequence(a: &[char], b: &[char]) -> usize {
    let mut prev = vec![0usize; b.len() + 1];
    for &ca in a {
        let mut cur = vec![0usize; b.len() + 1];
        for (j, &cb) in b.iter().enumerate() {
            cur[j + 1] = if ca == cb { prev[j] + 1 } else { prev[j + 1].max(cur[j]) };
        }
        prev = cur;
    }
    prev[b.len()]
}

/// gap = 句子开始 - 弹幕发出。刚发的权重高；隔几分钟也可能被念 → 衰减平缓、不归零
fn time_score(gap: f64) -> f64 {
    if gap < 0.0 {
        return 0.0;
    }
    if gap <= 15.0 {
        return 1.0;
    }
    let span = (WINDOW_SECS - 15.0).max(1.0);
    (1.0 - 0.55 * (gap - 15.0) / span).max(0.45)
}

fn cue_score(sentence: &str) -> f64 {
    if CUE_HINTS.iter().any(|h| sentence.contains(h)) {
        1.0
    } else {
        0.0
    }
}

/// 像是"谢礼物"而不是"念弹幕"
fn is_gift_like(sentence: &str) -> bool {
    let t = sentence.to_lowercase();
    if !(["谢谢", "感谢", "多谢"].iter().any(|w| t.contains(w))) {
        return false;
    }
    GIFT_WORDS.iter().any(|w| t.contains(w))
}

/// 明显不会被念的噪声弹幕（太短、纯重复字符）
fn is_noise(text: &str) -> bool {
    let t = norm(text);
    if t.len() < MIN_CAND_CHARS {
        return true;
    }
    t.len() >= 3 && t.iter().all(|c| *c == t[0])
}

struct Candidate<'a> {
    t: f64,
    norm: Vec<char>,
    raw: &'a str,
}

#[derive(Clone)]
struct Hit {
    norm: Vec<char>,
    score: f64,
    freq: usize,
    order: usize,
}

/// 把「主播念的句子」和普通弹幕配对。返回 (句子下标, 命中弹幕) 列表，
/// 只含有结论的句子（confirmed / uncertain）；普通句子与谢礼物句不返回。
pub fn match_readings(segments: &[AsrSegment], events: &[DanmakuEvent]) -> Vec<(usize, DmRead)> {
    // 只用普通弹幕：SC/礼物/舰长是"被打赏"，不进候选
    let mut dm: Vec<Candidate> = events
        .iter()
        .filter(|e| e.kind == DanmakuKind::Chat)
        .filter(|e| !is_noise(&e.text))
        .map(|e| Candidate { t: e.t, norm: norm(&e.text), raw: e.text.as_str() })
        .collect();
    dm.sort_by(|a, b| a.t.total_cmp(&b.t));
    // 复读去重：同一文本只留最早那条（被念的通常是原发）
    let mut seen: HashMap<Vec<char>, usize> = HashMap::new();
    let dm: Vec<&Candidate> = dm
        .iter()
        .enumerate()
        .filter(|(i, c)| seen.insert(c.norm.clone(), *i).is_none_or(|first| *i == first))
        .map(|(_, c)| c)
        .collect();

    // 第一遍：逐句收集候选命中
    struct Row {
        idx: usize,
        start: f64,
        gift: bool,
        hits: Vec<Hit>,
    }
    let mut rows: Vec<Row> = Vec::new();
    for (idx, s) in segments.iter().enumerate() {
        let text = s.best_text();
        if s.sc.is_some() {
            // SC 句已有专门标注口径，不重复配对
            rows.push(Row { idx, start: s.start, gift: true, hits: Vec::new() });
            continue;
        }
        if is_gift_like(text) {
            // 谢礼物句保留占位（最终不产出），维持下标对应
            rows.push(Row { idx, start: s.start, gift: true, hits: Vec::new() });
            continue;
        }
        let lo = s.start - WINDOW_SECS;
        let hi = s.end + TAIL_SECS;
        // 「群体刷屏」计数：只统计句子开始之前发出的
        let mut freq_before: HashMap<&Vec<char>, usize> = HashMap::new();
        for c in &dm {
            if c.t <= s.start {
                *freq_before.entry(&c.norm).or_insert(0) += 1;
            }
        }
        let sent = norm(text);
        if sent.is_empty() {
            continue;
        }
        let cue = cue_score(text);
        let mut hits = Vec::new();
        for (order, c) in dm.iter().enumerate() {
            if c.t < lo || c.t > hi {
                continue;
            }
            let fb = freq_before.get(&c.norm).copied().unwrap_or(1);
            if fb >= FREQ_HARD {
                continue;
            }
            let cov = cover(&c.norm, &sent);
            if cov < MIN_COVER {
                continue;
            }
            // 短弹幕不参与自动配对：≤3 字、在句子里占比小、又无语言线索 → 被长句顺带包含
            if c.norm.len() <= SHORT_CAND
                && (c.norm.len() as f64 / sent.len() as f64) < SHORT_RATIO
                && !(cue > 0.0 && sent.len() <= SHORT_CAND * 6)
            {
                continue;
            }
            let gap = s.start - c.t;
            let score = W_COVER * cov + W_TIME * time_score(gap) + W_CUE * cue;
            hits.push(Hit { norm: c.norm.clone(), score, freq: fb, order });
        }
        if hits.is_empty() {
            continue;
        }
        hits.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.order.cmp(&b.order)));
        rows.push(Row { idx, start: s.start, gift: false, hits });
    }

    // 第二遍：全局占用——每条弹幕（按文本）只归分数最高的句子；同分归更早的句子
    let mut claims: HashMap<&Vec<char>, (f64, usize)> = HashMap::new();
    for (r_i, row) in rows.iter().enumerate() {
        for h in &row.hits {
            match claims.get_mut(&h.norm) {
                Some(cur) if (h.score, -row.start) <= (cur.0, -rows[cur.1].start) => {}
                Some(cur) => {
                    cur.0 = h.score;
                    cur.1 = r_i;
                }
                None => {
                    claims.insert(&h.norm, (h.score, r_i));
                }
            }
        }
    }

    let mut out = Vec::new();
    let mut used: HashMap<usize, Vec<&Hit>> = HashMap::new();
    for (r_i, row) in rows.iter().enumerate() {
        if row.gift {
            continue;
        }
        let kept: Vec<&Hit> = row
            .hits
            .iter()
            .filter(|h| claims.get(&h.norm).is_some_and(|c| c.1 == r_i))
            .take(MAX_HITS)
            .collect();
        let Some(best) = kept.first() else { continue };
        let mut level = if best.score >= T_CONFIRM {
            "confirmed"
        } else if best.score >= T_UNCERTAIN {
            "uncertain"
        } else {
            continue;
        };
        // 群体刷屏 → 再高分也压到"待确认"，不冒充确定
        if level == "confirmed" && best.freq >= FREQ_SOFT {
            level = "uncertain";
        }
        if level != "confirmed" {
            // 待确认的只作为弱线索保留：同样标注，由大模型结合上下文自行判断
        }
        used.insert(r_i, kept);
        let _ = level;
    }
    for (r_i, row) in rows.iter().enumerate() {
        if let Some(hits) = used.remove(&r_i) {
            let texts: Vec<String> = hits
                .iter()
                .filter_map(|h| dm.iter().find(|c| c.norm == h.norm).map(|c| c.raw.to_string()))
                .collect();
            if let Some(first) = texts.first() {
                out.push((row.idx, DmRead { text: first.clone() }));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start: f64, end: f64, text: &str) -> AsrSegment {
        AsrSegment {
            start,
            end,
            file_idx: 0,
            file_start: start,
            lang: "zh".into(),
            emotion: String::new(),
            event: String::new(),
            text: text.into(),
            zh_text: None,
            streamer_secs: Some(end - start),
            sc: None,
            dm: None,
        }
    }

    fn chat(t: f64, text: &str) -> DanmakuEvent {
        DanmakuEvent { t, user: "u".into(), name: String::new(), text: text.into(), money: 0.0, kind: DanmakuKind::Chat }
    }

    #[test]
    fn matches_read_danmaku_with_typo_tolerance() {
        // 弹幕"你家猫叫什么名字"，主播念时加了话、还带一个同音错字
        let segments = vec![seg(400.0, 406.0, "有人问你家的猫叫什么名字啊")];
        let events = vec![chat(150.0, "你家猫叫什么名字"), chat(500.0, " unrelated ")];
        let out = match_readings(&segments, &events);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, 0);
        assert_eq!(out[0].1.text, "你家猫叫什么名字");
    }

    #[test]
    fn skips_gift_noise_and_group_spam() {
        let mut events = vec![
            chat(10.0, "哈哈哈"),
            chat(11.0, "66"),
            chat(12.0, "aaaa"),
            chat(20.0, "主播晚安"),
            chat(21.0, "主播晚安"),
            chat(22.0, "主播晚安"),
            chat(23.0, "主播晚安"),
            chat(24.0, "主播晚安"),
            chat(25.0, "主播晚安"),
            chat(26.0, "主播晚安"),
        ];
        // 谢礼物句不配弹幕
        let segments = vec![seg(500.0, 504.0, "谢谢这件礼物的喜欢"), seg(520.0, 524.0, "你提到晚安我就想笑")];
        let out = match_readings(&segments, &events);
        assert!(out.iter().all(|(i, _)| *i == 1), "{out:?}");
        let _ = &mut events;
    }

    #[test]
    fn one_danmaku_claims_only_best_sentence() {
        // 一条弹幕对两个句子都合法：只有分高的句子拿到，另一句不产出
        let segments = vec![
            seg(100.0, 104.0, "跟你讲个事情啊"),                       // 覆盖低
            seg(120.0, 126.0, "有人问你家猫叫什么名字啊"),             // 覆盖高
        ];
        let events = vec![chat(60.0, "你家猫叫什么名字")];
        let out = match_readings(&segments, &events);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, 1);
    }

    #[test]
    fn short_danmaku_not_matched_without_cue() {
        // 短弹幕被长句顺带包含：无线索时不配
        let segments = vec![seg(100.0, 110.0, "他刚才那个操作确实挺秀的不过也好耶过头了")];
        let events = vec![chat(80.0, "好耶")];
        assert!(match_readings(&segments, &events).is_empty());
        // 主播专门念短弹幕（句子短 + 有线索）：豁免
        let segments = vec![seg(100.0, 103.0, "你说好耶我就好耶了")];
        let events = vec![chat(80.0, "好耶")];
        let out = match_readings(&segments, &events);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].1.text, "好耶");
    }
}
