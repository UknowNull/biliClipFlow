//! 信号层与候选召回：弹幕、付费互动、响度、语音事件按 5 秒分桶，归一化加权后取峰值。

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use super::asr::{AsrSegment, FullPass};
use super::danmaku::{keyword_score, normalize_text, DanmakuEvent, DanmakuKind};
use super::settings::ClipSettings;

pub const BIN_SECS: f64 = 5.0;
const PEAK_MIN_GAP_SECS: f64 = 180.0;
const WINDOW_BEFORE_SECS: f64 = 90.0;
const WINDOW_AFTER_SECS: f64 = 60.0;
const MIN_PEAK_SCORE: f64 = 0.12;
/// 短场次也至少召回这么多候选，给大模型淘汰留余量
const MIN_CANDIDATES: usize = 5;

/// 主播内容类型，决定信号权重与提示词
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    Reaction,
    Game,
    Chat,
}

impl Profile {
    pub fn parse(value: &str) -> Self {
        match value {
            "game" => Profile::Game,
            "chat" => Profile::Chat,
            _ => Profile::Reaction,
        }
    }

    /// 权重顺序：弹幕热度、刷屏与关键词、付费、响度、笑声与情绪
    fn weights(self) -> [f64; 5] {
        match self {
            Profile::Reaction => [0.25, 0.20, 0.10, 0.10, 0.35],
            Profile::Game => [0.30, 0.20, 0.10, 0.20, 0.20],
            Profile::Chat => [0.30, 0.20, 0.15, 0.10, 0.25],
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignalSnapshot {
    /// 弹幕热度相对基线的倍数
    pub heat_ratio: f64,
    pub max_repeat: usize,
    pub repeat_text: String,
    pub paid_yuan: f64,
    /// 响度高于基线的分贝数
    pub loud_db: f64,
    pub laughs: usize,
    pub emotions: usize,
    pub streamer_speech_secs: f64,
    pub has_danmaku: bool,
}

impl SignalSnapshot {
    /// 给大模型看的一行信号摘要
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.has_danmaku {
            parts.push(format!("弹幕热度为基线的 {:.1} 倍", self.heat_ratio));
            if self.max_repeat >= 3 {
                parts.push(format!("「{}」刷屏 {} 次", self.repeat_text, self.max_repeat));
            }
            if self.paid_yuan > 0.0 {
                parts.push(format!("付费互动 {:.0} 元", self.paid_yuan));
            }
        } else {
            parts.push("无弹幕数据".to_string());
        }
        if self.laughs > 0 {
            parts.push(format!("笑声 {} 次", self.laughs));
        }
        if self.emotions > 0 {
            parts.push(format!("主播情绪波动 {} 处", self.emotions));
        }
        if self.loud_db > 3.0 {
            parts.push(format!("响度高于基线 {:.0} dB", self.loud_db));
        }
        parts.join("；")
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub peak: f64,
    pub start: f64,
    pub end: f64,
    pub score: f64,
    pub signals: SignalSnapshot,
}

/// 场次中一个录播文件在时间轴上的范围（候选不跨越文件）
#[derive(Clone, Copy, Debug)]
pub struct Span {
    pub start: f64,
    pub end: f64,
}

struct Features {
    heat: Vec<f64>,
    heat_ratio: Vec<f64>,
    spam: Vec<f64>,
    max_repeat: Vec<(usize, String)>,
    paid: Vec<f64>,
    paid_raw: Vec<f64>,
    loud: Vec<f64>,
    laugh: Vec<f64>,
    emo: Vec<f64>,
    streamer: Vec<f64>,
    any_speech: Vec<f64>,
    dm_count: Vec<f64>,
}

fn bin_of(t: f64) -> usize {
    (t.max(0.0) / BIN_SECS) as usize
}

fn rolling_median(values: &[f64], radius: usize) -> Vec<f64> {
    let mut out = Vec::with_capacity(values.len());
    let mut window: Vec<f64> = Vec::new();
    for i in 0..values.len() {
        let lo = i.saturating_sub(radius);
        let hi = (i + radius + 1).min(values.len());
        window.clear();
        window.extend_from_slice(&values[lo..hi]);
        window.sort_by(|a, b| a.total_cmp(b));
        out.push(window[window.len() / 2]);
    }
    out
}

/// 按本场 95 分位数缩放到 [0, 1]
fn scale(values: &[f64]) -> Vec<f64> {
    let mut sorted: Vec<f64> = values.iter().copied().filter(|v| *v > 0.0).collect();
    if sorted.is_empty() {
        return vec![0.0; values.len()];
    }
    sorted.sort_by(|a, b| a.total_cmp(b));
    let q = sorted[((sorted.len() - 1) as f64 * 0.95) as usize].max(1e-6);
    values.iter().map(|v| (v / q).clamp(0.0, 1.0)).collect()
}

fn add_overlap(series: &mut [f64], start: f64, end: f64, weight: f64) {
    let mut t = start;
    while t < end {
        let bin = bin_of(t);
        let bin_end = (bin as f64 + 1.0) * BIN_SECS;
        let take = end.min(bin_end) - t;
        if let Some(slot) = series.get_mut(bin) {
            *slot += take * weight;
        }
        t = bin_end;
    }
}

fn build_features(n: usize, events: &[DanmakuEvent], asr: Option<&FullPass>) -> Features {
    let mut users: Vec<HashSet<&str>> = vec![HashSet::new(); n];
    let mut texts: Vec<HashMap<String, usize>> = vec![HashMap::new(); n];
    let mut keyword = vec![0.0; n];
    let mut paid_raw = vec![0.0; n];
    let mut dm_count = vec![0.0; n];
    for e in events {
        let b = bin_of(e.t);
        if b >= n {
            continue;
        }
        if e.money > 0.0 {
            paid_raw[b] += e.money;
        }
        if matches!(e.kind, DanmakuKind::Chat | DanmakuKind::SuperChat) {
            dm_count[b] += 1.0;
            users[b].insert(e.user.as_str());
            let norm = normalize_text(&e.text);
            keyword[b] += keyword_score(&norm);
            if !norm.is_empty() {
                *texts[b].entry(norm).or_insert(0) += 1;
            }
        }
    }
    let heat: Vec<f64> = users.iter().map(|u| u.len() as f64).collect();
    let baseline = rolling_median(&heat, (600.0 / BIN_SECS) as usize);
    let heat_ratio: Vec<f64> = heat.iter().zip(&baseline).map(|(h, b)| h / b.max(1.0)).collect();
    // 刷屏：20 秒窗口内同一句的最大次数
    let mut max_repeat = vec![(0usize, String::new()); n];
    let span = (20.0 / BIN_SECS) as usize;
    for b in 0..n {
        let mut merged: HashMap<&str, usize> = HashMap::new();
        for bb in b.saturating_sub(span - 1)..=b {
            for (k, v) in &texts[bb] {
                *merged.entry(k.as_str()).or_insert(0) += v;
            }
        }
        if let Some((k, v)) = merged.into_iter().max_by_key(|(_, v)| *v) {
            max_repeat[b] = (v, k.to_string());
        }
    }
    let spam: Vec<f64> = (0..n)
        .map(|b| (max_repeat[b].0 as f64 - 2.0).max(0.0) + keyword[b])
        .collect();
    let paid: Vec<f64> = paid_raw.iter().map(|v| (1.0 + v).ln()).collect();

    let mut loud = vec![0.0; n];
    let mut laugh = vec![0.0; n];
    let mut emo = vec![0.0; n];
    let mut streamer = vec![0.0; n];
    let mut any_speech = vec![0.0; n];
    if let Some(pass) = asr {
        let mut bin_loud = vec![-90.0f64; n];
        for (sec, db) in pass.loudness.iter().enumerate() {
            let b = bin_of(sec as f64);
            if b < n {
                bin_loud[b] = bin_loud[b].max(*db as f64);
            }
        }
        let base = rolling_median(&bin_loud, (600.0 / BIN_SECS) as usize);
        for b in 0..n {
            if bin_loud[b] > -80.0 {
                loud[b] = (bin_loud[b] - base[b]).max(0.0);
            }
        }
        for seg in &pass.segments {
            add_overlap(&mut any_speech, seg.start, seg.end, 1.0);
            if seg.event == "Laughter" {
                add_overlap(&mut laugh, seg.start, seg.end, 1.0);
            }
            if seg.is_streamer() {
                add_overlap(&mut streamer, seg.start, seg.end, 1.0);
                if matches!(seg.emotion.as_str(), "HAPPY" | "SURPRISED" | "ANGRY") {
                    add_overlap(&mut emo, seg.start, seg.end, 1.0);
                }
            }
        }
    }
    Features { heat, heat_ratio, spam, max_repeat, paid, paid_raw, loud, laugh, emo, streamer, any_speech, dm_count }
}

fn moving_average(values: &[f64], radius: usize) -> Vec<f64> {
    (0..values.len())
        .map(|i| {
            let lo = i.saturating_sub(radius);
            let hi = (i + radius + 1).min(values.len());
            values[lo..hi].iter().sum::<f64>() / (hi - lo) as f64
        })
        .collect()
}

fn sum_range(series: &[f64], start: f64, end: f64) -> f64 {
    let (a, b) = (bin_of(start), bin_of(end).min(series.len().saturating_sub(1)));
    if a > b || series.is_empty() {
        return 0.0;
    }
    series[a..=b].iter().sum()
}

/// 计算整场得分曲线并选出候选窗口。
pub fn pick_candidates(
    session_secs: f64,
    spans: &[Span],
    events: &[DanmakuEvent],
    has_danmaku: bool,
    asr: Option<&FullPass>,
    profile: Profile,
    settings: &ClipSettings,
) -> Vec<Candidate> {
    let n = (session_secs / BIN_SECS).ceil().max(1.0) as usize;
    let f = build_features(n, events, asr);
    let mut w = profile.weights();
    if !has_danmaku {
        let moved = w[0] + w[1] + w[2];
        let rest = w[3] + w[4];
        w = [0.0, 0.0, 0.0, w[3] + moved * w[3] / rest, w[4] + moved * w[4] / rest];
    }
    let heat_excess: Vec<f64> = f.heat_ratio.iter().zip(&f.heat).map(|(r, h)| if *h >= 2.0 { (r - 1.0).max(0.0) } else { 0.0 }).collect();
    let laugh_emo: Vec<f64> = f.laugh.iter().zip(&f.emo).map(|(l, e)| l * 2.0 + e).collect();
    let scaled = [scale(&heat_excess), scale(&f.spam), scale(&f.paid), scale(&f.loud), scale(&laugh_emo)];
    let raw: Vec<f64> = (0..n).map(|b| (0..5).map(|k| w[k] * scaled[k][b]).sum()).collect();
    let score = moving_average(&raw, 1);

    let hours = session_secs / 3600.0;
    let k = ((hours * settings.candidates_per_hour).ceil() as usize).max(MIN_CANDIDATES).min(settings.max_candidates);
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|a, b| score[*b].total_cmp(&score[*a]));
    let gap_bins = (PEAK_MIN_GAP_SECS / BIN_SECS) as usize;
    let mut picked: Vec<usize> = Vec::new();
    for b in order {
        if picked.len() >= k || score[b] < MIN_PEAK_SCORE {
            break;
        }
        if picked.iter().any(|p| p.abs_diff(b) < gap_bins) {
            continue;
        }
        let t = (b as f64 + 0.5) * BIN_SECS;
        // 主播离开或长时间静音：前后一分钟既没人说话也没有弹幕
        let activity = sum_range(&f.any_speech, t - 60.0, t + 60.0) + sum_range(&f.dm_count, t - 60.0, t + 60.0);
        if asr.is_some() && activity < 5.0 {
            continue;
        }
        picked.push(b);
    }

    let mut out = Vec::new();
    for b in picked {
        let peak = (b as f64 + 0.5) * BIN_SECS;
        let Some(span) = spans.iter().find(|s| peak >= s.start && peak < s.end) else { continue };
        let start = (peak - WINDOW_BEFORE_SECS - settings.danmaku_lag_secs as f64).max(span.start);
        let end = (peak + WINDOW_AFTER_SECS).min(span.end);
        let lo = bin_of(start).min(n - 1);
        let hi = bin_of(end).min(n - 1);
        let (rep, rep_text) = (lo..=hi).map(|i| f.max_repeat[i].clone()).max_by_key(|r| r.0).unwrap_or_default();
        out.push(Candidate {
            peak,
            start,
            end,
            score: score[b],
            signals: SignalSnapshot {
                heat_ratio: (lo..=hi).map(|i| f.heat_ratio[i]).fold(0.0, f64::max),
                max_repeat: rep,
                repeat_text: rep_text,
                paid_yuan: f.paid_raw[lo..=hi].iter().sum(),
                loud_db: f.loud[lo..=hi].iter().copied().fold(0.0, f64::max),
                laughs: count_segments(asr, start, end, |s| s.event == "Laughter"),
                emotions: count_segments(asr, start, end, |s| {
                    s.is_streamer() && matches!(s.emotion.as_str(), "HAPPY" | "SURPRISED" | "ANGRY")
                }),
                streamer_speech_secs: sum_range(&f.streamer, start, end),
                has_danmaku,
            },
        });
    }
    out.sort_by(|a, b| a.peak.total_cmp(&b.peak));
    out
}

fn count_segments(asr: Option<&FullPass>, start: f64, end: f64, pred: impl Fn(&AsrSegment) -> bool) -> usize {
    asr.map(|p| p.segments.iter().filter(|s| s.end > start && s.start < end && pred(s)).count())
        .unwrap_or(0)
}

/// 窗口内出现最多的弹幕（去重后带次数），给大模型参考
pub fn top_danmaku(events: &[DanmakuEvent], start: f64, end: f64, limit: usize) -> Vec<(String, usize)> {
    let mut counts: HashMap<String, (String, usize)> = HashMap::new();
    for e in events.iter().filter(|e| e.t >= start && e.t < end && e.kind != DanmakuKind::Gift) {
        let key = normalize_text(&e.text);
        if key.is_empty() {
            continue;
        }
        let entry = counts.entry(key).or_insert_with(|| (e.text.clone(), 0));
        entry.1 += 1;
    }
    let mut list: Vec<(String, usize)> = counts.into_values().collect();
    list.sort_by(|a, b| b.1.cmp(&a.1));
    list.truncate(limit);
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(t: f64, user: &str, text: &str) -> DanmakuEvent {
        DanmakuEvent { t, user: user.to_string(), text: text.to_string(), money: 0.0, kind: DanmakuKind::Chat }
    }

    #[test]
    fn picks_danmaku_burst_with_lag_compensated_window() {
        // 1 小时场次：平时每 10 秒 1 条弹幕，第 30 分钟出现 40 人刷「哈哈哈」
        let mut events: Vec<DanmakuEvent> = (0..360).map(|i| chat(i as f64 * 10.0, &format!("u{}", i % 7), "嗯")).collect();
        for i in 0..40 {
            events.push(chat(1800.0 + i as f64 * 0.3, &format!("fan{}", i), "哈哈哈哈"));
        }
        events.sort_by(|a, b| a.t.total_cmp(&b.t));
        let settings = ClipSettings::default();
        let spans = [Span { start: 0.0, end: 3600.0 }];
        let cands = pick_candidates(3600.0, &spans, &events, true, None, Profile::Reaction, &settings);
        assert!(!cands.is_empty());
        let top = &cands[0];
        assert!((top.peak - 1800.0).abs() <= 15.0, "peak at {}", top.peak);
        assert!(top.start <= top.peak - 90.0 - settings.danmaku_lag_secs as f64 + BIN_SECS);
        assert!(top.signals.max_repeat >= 30);
    }
}
