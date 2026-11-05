//! 边界层：起止吸附到语音分段边界，强制时长范围，去重，主播解说占比计算。

use super::asr::AsrSegment;
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

/// 主播中文说话时长占区间的比例
pub fn streamer_ratio(range: Range, segments: &[AsrSegment]) -> f64 {
    if range.len() <= 0.0 {
        return 0.0;
    }
    // 分段内主播说话时长按与区间的重叠比例折算
    let speech: f64 = segments
        .iter()
        .filter(|s| s.end > s.start)
        .map(|s| s.streamer_seconds() * (s.end.min(range.end) - s.start.max(range.start)).max(0.0) / (s.end - s.start))
        .sum();
    speech / range.len()
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
