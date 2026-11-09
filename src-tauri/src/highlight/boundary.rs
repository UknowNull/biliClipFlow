//! 边界层：起止吸附到语音分段边界，强制时长范围，去重，主播解说占比计算，念 SC 标注。

use std::collections::{HashMap, HashSet};

use super::asr::{AsrSegment, ScRead};
use super::signals::Span;

const SNAP_LOOKBACK_SECS: f64 = 15.0;
const TAIL_SECS: f64 = 2.0;
const LAUGH_EXTEND_SECS: f64 = 10.0;
/// 句间停顿不超过该秒数视为同一段话，终点不在其中断开
const CONTINUE_GAP_SECS: f64 = 1.0;

#[derive(Clone, Copy, Debug)]
pub struct Range {
    pub start: f64,
    pub end: f64,
}

impl Range {
    pub fn len(&self) -> f64 {
        self.end - self.start
    }

    fn overlap(&self, other: &Range) -> f64 {
        (self.end.min(other.end) - self.start.max(other.start)).max(0.0)
    }
}

/// 把 [start, end] 调整为不在句中断开、时长在范围内、不越出所在文件的区间。
/// `segments` 按开始时间排序；`peak` 为信号峰值，截断时以它为中心。
pub fn finalize(
    proposed: Range,
    peak: f64,
    span: Span,
    segments: &[AsrSegment],
    min_secs: f64,
    max_secs: f64,
) -> Range {
    let inside: Vec<&AsrSegment> = segments.iter().filter(|s| s.start >= span.start && s.end <= span.end).collect();
    let mut start = proposed.start.clamp(span.start, span.end);
    let mut end = proposed.end.clamp(span.start, span.end);

    // 起点：若落在句中，退到该句开头；否则吸附到前面不远处的句首
    if let Some(seg) = inside.iter().find(|s| s.start <= start && s.end > start) {
        start = seg.start;
    } else if let Some(seg) = inside.iter().rev().find(|s| s.start <= start && start - s.start <= SNAP_LOOKBACK_SECS) {
        start = seg.start;
    }
    // 终点：若落在句中，延到句末；之后紧跟的笑声也收进来
    if let Some(seg) = inside.iter().find(|s| s.start < end && s.end >= end) {
        end = seg.end;
    }
    while let Some(seg) = inside.iter().find(|s| s.start >= end && s.start - end <= CONTINUE_GAP_SECS && s.end - start <= max_secs) {
        end = seg.end;
    }
    if let Some(seg) = inside.iter().find(|s| s.event == "Laughter" && s.start >= end && s.start - end <= LAUGH_EXTEND_SECS) {
        end = seg.end;
    }
    end = (end + TAIL_SECS).min(span.end);

    // 不足下限：交替向两侧扩到相邻句子边界，没有句子时按秒扩
    let mut guard = 0;
    while end - start < min_secs && guard < 200 {
        guard += 1;
        let before = inside.iter().rev().find(|s| s.end <= start).map(|s| s.start);
        let after = inside.iter().find(|s| s.start >= end).map(|s| s.end);
        let need = min_secs - (end - start);
        let grow_back = start > span.start && (guard % 2 == 0 || end >= span.end);
        if grow_back {
            start = before.filter(|b| start - b <= need + 20.0).unwrap_or(start - need.min(10.0)).max(span.start);
        } else {
            end = after.filter(|a| a - end <= need + 20.0).unwrap_or(end + need.min(10.0)).min(span.end);
        }
        if start <= span.start && end >= span.end {
            break;
        }
    }

    // 超过上限：以峰值为中心截断，再吸附到句子边界
    if end - start > max_secs {
        let half = max_secs / 2.0;
        let mut s = (peak - half).max(start);
        let mut e = (s + max_secs).min(end);
        s = (e - max_secs).max(start);
        if let Some(seg) = inside.iter().find(|x| x.start >= s && x.start - s <= SNAP_LOOKBACK_SECS) {
            s = seg.start;
        }
        if let Some(seg) = inside.iter().rev().find(|x| x.end <= e && e - x.end <= SNAP_LOOKBACK_SECS) {
            e = seg.end;
        }
        start = s;
        end = e.max(s + min_secs.min(max_secs)).min(span.end);
    }
    Range { start, end }
}

/// 只把起止对齐到句子边界：起点在句中退到句首，终点在句中延到句末，不改变话题范围
pub fn snap(proposed: Range, span: Span, segments: &[AsrSegment]) -> Range {
    let mut start = proposed.start.clamp(span.start, span.end);
    let mut end = proposed.end.clamp(span.start, span.end);
    if let Some(seg) = segments.iter().find(|s| s.start <= start && s.end > start) {
        start = seg.start;
    }
    if let Some(seg) = segments.iter().find(|s| s.start < end && s.end >= end) {
        end = seg.end;
    }
    Range { start, end: (end + TAIL_SECS).min(span.end) }
}

/// 主播中文说话时长占区间的比例；念 SC 是在读观众的话，不算主播自己的内容（夹着自己的话的算一半）
pub fn streamer_ratio(range: Range, segments: &[AsrSegment]) -> f64 {
    if range.len() <= 0.0 {
        return 0.0;
    }
    // 分段内主播说话时长按与区间的重叠比例折算
    let speech: f64 = segments
        .iter()
        .filter(|s| s.end > s.start)
        .map(|s| {
            let own = match &s.sc {
                Some(sc) if sc.pure => 0.0,
                Some(_) => 0.5,
                None => 1.0,
            };
            own * s.streamer_seconds() * (s.end.min(range.end) - s.start.max(range.start)).max(0.0) / (s.end - s.start)
        })
        .sum();
    speech / range.len()
}

/// SC 被念出来的时间可能比发送晚很久，在发送后这段时间内找
const SC_READ_WINDOW_SECS: f64 = 3600.0;
/// SC 原文至少有这么多被几句连续转写覆盖，才算找到了念它的位置
const SC_MATCH_MIN: f64 = 0.5;
/// 一句里 SC 原文的成分达到这个比例算在念 SC，达到 PURE 算整句都在念
const SC_LINE_MIN: f64 = 0.25;
const SC_LINE_PURE: f64 = 0.5;

fn norm_chars(text: &str) -> Vec<char> {
    text.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

fn bigrams(chars: &[char]) -> HashSet<(char, char)> {
    chars.windows(2).map(|w| (w[0], w[1])).collect()
}

/// “谢谢某某的 SC”这类念 SC 的开场白（识别结果里 SC 常被拆成字母或认成相近的写法）
fn is_sc_thanks(chars: &[char]) -> bool {
    const CODES: [&str; 10] = ["sc", "az", "sz", "nc", "mc", "ez", "as", "sd", "ac", "easy"];
    let text: String = chars.iter().collect();
    let Some(i) = text.find("谢谢") else { return false };
    let rest: Vec<char> = text[i + "谢谢".len()..].chars().take(26).collect();
    rest.iter().enumerate().any(|(k, c)| {
        *c == '的' && k > 0 && {
            let after: String = rest[k + 1..].iter().collect();
            CODES.iter().any(|code| after.starts_with(code))
        }
    })
}

/// 按弹幕里的 SC 原文和发送时间，标出转写中主播在念 SC 的句子。`scs` 为（发送时间, 原文）。
pub fn mark_sc_readings(segments: &mut [AsrSegment], scs: &[(f64, String)]) {
    let texts: Vec<Vec<char>> = segments.iter().map(|s| norm_chars(s.best_text())).collect();
    let line_grams: Vec<HashSet<(char, char)>> = texts.iter().map(|t| bigrams(t)).collect();
    // 句子下标 -> (SC 编号, 该句中 SC 成分比例)
    let mut marks: HashMap<usize, (usize, f64)> = HashMap::new();
    for (id, (sent, message)) in scs.iter().enumerate() {
        let sc = bigrams(&norm_chars(message));
        if sc.len() < 3 {
            continue;
        }
        // 找连续 1~4 句覆盖 SC 原文最多的位置
        let from = segments.partition_point(|s| s.start < sent - 10.0);
        let to = segments.partition_point(|s| s.start <= sent + SC_READ_WINDOW_SECS);
        let mut best = (0.0, 0, 0);
        for i in from..to {
            let mut window: Vec<char> = Vec::new();
            for j in i..(i + 4).min(segments.len()) {
                window.extend(&texts[j]);
                let covered = sc.intersection(&bigrams(&window)).count() as f64 / sc.len() as f64;
                if covered > best.0 + 0.05 {
                    best = (covered, i, j);
                }
            }
        }
        if best.0 < SC_MATCH_MIN {
            continue;
        }
        let mut first = None;
        for k in best.1.saturating_sub(1)..(best.2 + 2).min(segments.len()) {
            let own = &line_grams[k];
            let share = own.intersection(&sc).count() as f64 / own.len().max(1) as f64;
            if share >= SC_LINE_MIN && share > marks.get(&k).map(|m| m.1).unwrap_or(0.0) {
                marks.insert(k, (id, share));
                first.get_or_insert(k);
            }
        }
        // 紧挨在前面的“谢谢某某的 SC”也是在念这条
        if let Some(k) = first.filter(|k| *k > 0).map(|k| k - 1) {
            if !marks.contains_key(&k) && is_sc_thanks(&texts[k]) {
                marks.insert(k, (id, 1.0));
            }
        }
    }
    for (k, seg) in segments.iter_mut().enumerate() {
        seg.sc = match marks.get(&k) {
            Some(&(id, share)) => Some(ScRead { id, pure: share >= SC_LINE_PURE, text: scs[id].1.clone() }),
            // 没对上原文的 SC，开场白也标出来，方便判断下一条 SC 从哪开始
            None if is_sc_thanks(&texts[k]) => Some(ScRead { id: usize::MAX - k, pure: true, text: String::new() }),
            None => None,
        };
    }
}

/// 按念 SC 的标注修正切片边界：起点落在一条 SC 中间时退到这条 SC 开头；
/// 结尾是在念下一条 SC 的句子时去掉（切片应结束在主播的回应上）
pub fn fit_sc(range: Range, segments: &[AsrSegment]) -> Range {
    let inside: Vec<usize> = (0..segments.len()).filter(|&i| segments[i].start >= range.start - 0.01 && segments[i].start < range.end).collect();
    let (Some(&first), Some(&last)) = (inside.first(), inside.last()) else { return range };
    let mut start_idx = first;
    if let Some(id) = segments[first].sc.as_ref().map(|s| s.id) {
        while start_idx > 0 && segments[start_idx - 1].sc.as_ref().map(|s| s.id) == Some(id) {
            start_idx -= 1;
        }
    }
    let first_id = segments[start_idx].sc.as_ref().map(|s| s.id);
    let mut end_idx = last;
    while end_idx > first {
        match &segments[end_idx].sc {
            Some(sc) if sc.pure && Some(sc.id) != first_id => end_idx -= 1,
            _ => break,
        }
    }
    let end = if end_idx == last { range.end } else { (segments[end_idx].end + TAIL_SECS).min(range.end) };
    Range { start: segments[start_idx].start.min(range.start), end }
}

/// 按分数从高到低保留，与已保留区间重叠超过较短者 30% 的丢弃。返回保留项的下标。
pub fn dedupe(ranges: &[(Range, f64)]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..ranges.len()).collect();
    order.sort_by(|a, b| ranges[*b].1.total_cmp(&ranges[*a].1));
    let mut kept: Vec<usize> = Vec::new();
    for i in order {
        let r = ranges[i].0;
        let clash = kept.iter().any(|&k| {
            let o = ranges[k].0;
            r.overlap(&o) > 0.3 * r.len().min(o.len())
        });
        if !clash {
            kept.push(i);
        }
    }
    kept.sort_by(|a, b| ranges[*a].0.start.total_cmp(&ranges[*b].0.start));
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start: f64, end: f64, event: &str) -> AsrSegment {
        AsrSegment {
            start,
            end,
            file_idx: 0,
            file_start: start,
            lang: "zh".into(),
            emotion: String::new(),
            event: event.into(),
            text: "x".into(),
            zh_text: None,
            streamer_secs: None,
            sc: None,
        }
    }

    #[test]
    fn marks_sc_readings_and_fits_clip_to_them() {
        let line = |start: f64, text: &str| AsrSegment { text: text.into(), ..seg(start, start + 4.0, "Speech") };
        let mut segments = vec![
            line(100.0, "谢谢价值四十的 s c"),
            line(104.0, "李二杀兄弟杀侄子抢嫂子但是是顺位继承的"),
            line(108.0, "这点朱四羡慕不来"),
            line(112.0, "抢嫂子还有这啊哈哈太好笑了吧"),
            line(116.0, "谢谢小王子的 a z 奶姐晚上好"),
        ];
        let scs = vec![(50.0, "李二杀兄弟，杀侄子，抢嫂子，但是是顺位继承的，这点朱四羡慕不来".to_string())];
        mark_sc_readings(&mut segments, &scs);
        let pure: Vec<bool> = segments.iter().map(|s| s.sc.as_ref().is_some_and(|sc| sc.pure)).collect();
        assert_eq!(pure, [true, true, true, false, true]);
        // 从 SC 中间开始、带进下一条 SC 开场白的切片：起点退到“谢谢”，结尾去掉下一条
        let r = fit_sc(Range { start: 104.0, end: 122.0 }, &segments);
        assert_eq!((r.start, r.end), (100.0, 118.0));
    }

    #[test]
    fn snaps_to_sentences_and_enforces_duration() {
        let segments = vec![seg(95.0, 104.0, "Speech"), seg(110.0, 118.0, "Speech"), seg(120.0, 126.0, "Laughter"), seg(140.0, 160.0, "Speech")];
        let span = Span { start: 0.0, end: 1000.0 };
        // 起点落在第一句中间、终点落在第二句中间
        let r = finalize(Range { start: 100.0, end: 115.0 }, 110.0, span, &segments, 45.0, 300.0);
        assert_eq!(r.start, 95.0);
        assert!(r.end >= 128.0, "should include laughter tail, got {}", r.end);
        assert!(r.len() >= 45.0);
        // 超长时以峰值为中心截断
        let r = finalize(Range { start: 0.0, end: 900.0 }, 500.0, span, &segments, 45.0, 300.0);
        assert!(r.len() <= 300.0 && r.start <= 500.0 && r.end >= 500.0);
    }
}
