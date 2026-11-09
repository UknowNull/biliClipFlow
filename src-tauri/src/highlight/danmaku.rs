//! 弹幕解析：录制产生的 `*.danmaku.jsonl`（兼容 XML）转为带场次时间偏移的事件。
//!
//! jsonl 每行为 `{"cmd": ..., "data": <B站原始消息>, "timestamp": <接收时间 RFC3339>}`。

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use base64::Engine;
use chrono::{DateTime, Utc};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DanmakuKind {
    Chat,
    SuperChat,
    Gift,
    Guard,
}

#[derive(Clone, Debug)]
pub struct DanmakuEvent {
    /// 场次时间轴上的秒数
    pub t: f64,
    /// 用户标识：uid，取不到时用昵称
    pub user: String,
    /// 昵称，取不到时为空
    pub name: String,
    pub text: String,
    /// 人民币金额
    pub money: f64,
    pub kind: DanmakuKind,
}

/// 一个录播分段在场次时间轴上的位置
pub struct DanmakuSource<'a> {
    pub video_path: &'a str,
    pub offset_start: f64,
}

pub fn danmaku_paths(video_path: &str) -> (PathBuf, PathBuf) {
    let path = Path::new(video_path);
    (path.with_extension("danmaku.jsonl"), path.with_extension("xml"))
}

/// 读取所有分段的弹幕，返回按时间排序的事件，以及是否找到了非空弹幕文件。
pub fn load_events(sources: &[DanmakuSource], base_time: DateTime<Utc>) -> (Vec<DanmakuEvent>, bool) {
    let mut events = Vec::new();
    let mut found = false;
    for source in sources {
        let (jsonl, xml) = danmaku_paths(source.video_path);
        if file_has_content(&jsonl) {
            found = true;
            if let Ok(file) = File::open(&jsonl) {
                for line in BufReader::new(file).lines().map_while(Result::ok) {
                    if let Some(event) = parse_jsonl_line(&line, base_time) {
                        events.push(event);
                    }
                }
            }
        } else if file_has_content(&xml) {
            found = true;
            if let Ok(file) = File::open(&xml) {
                for line in BufReader::new(file).lines().map_while(Result::ok) {
                    events.extend(parse_xml_line(&line, source.offset_start));
                }
            }
        }
    }
    events.retain(|e| e.t >= 0.0);
    events.sort_by(|a, b| a.t.total_cmp(&b.t));
    (events, found)
}

fn file_has_content(path: &Path) -> bool {
    std::fs::metadata(path).map(|m| m.len() > 0).unwrap_or(false)
}

pub fn parse_jsonl_line(line: &str, base_time: DateTime<Utc>) -> Option<DanmakuEvent> {
    let value: Value = serde_json::from_str(line).ok()?;
    let ts = value.get("timestamp")?.as_str()?;
    let at = DateTime::parse_from_rfc3339(ts).ok()?.with_timezone(&Utc);
    let t = (at - base_time).num_milliseconds() as f64 / 1000.0;
    let cmd = value.get("cmd").and_then(|v| v.as_str()).unwrap_or("");
    let raw = value.get("data")?;
    let body = raw.get("data");
    let str_of = |v: Option<&Value>| v.and_then(|v| v.as_str()).unwrap_or("").to_string();
    let uid_or = |uid: Option<&Value>, name: Option<&Value>| -> String {
        let uid = uid.and_then(|v| v.as_i64()).filter(|v| *v > 0);
        match uid {
            Some(id) => id.to_string(),
            None => name.and_then(|v| v.as_str()).unwrap_or("").to_string(),
        }
    };
    if cmd.starts_with("DANMU_MSG") {
        let info = raw.get("info")?.as_array()?;
        let text = info.get(1)?.as_str()?.to_string();
        let user_info = info.get(2).and_then(|v| v.as_array());
        let user = uid_or(
            user_info.and_then(|u| u.first()),
            user_info.and_then(|u| u.get(1)),
        );
        let name = str_of(user_info.and_then(|u| u.get(1)));
        return Some(DanmakuEvent { t, user, name, text, money: 0.0, kind: DanmakuKind::Chat });
    }
    let body = body?;
    match cmd {
        "SUPER_CHAT_MESSAGE" | "SUPER_CHAT_MESSAGE_JPN" => Some(DanmakuEvent {
            t,
            user: uid_or(body.get("uid"), body.get("user_info").and_then(|u| u.get("uname"))),
            name: str_of(body.get("user_info").and_then(|u| u.get("uname"))),
            text: body.get("message").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            money: body.get("price").and_then(as_f64).unwrap_or(0.0),
            kind: DanmakuKind::SuperChat,
        }),
        "SEND_GIFT" => {
            let gold = body.get("coin_type").and_then(|v| v.as_str()) == Some("gold");
            let total = body.get("total_coin").and_then(as_f64).unwrap_or_else(|| {
                body.get("price").and_then(as_f64).unwrap_or(0.0)
                    * body.get("num").and_then(as_f64).unwrap_or(1.0)
            });
            Some(DanmakuEvent {
                t,
                user: uid_or(body.get("uid"), body.get("uname")),
                name: str_of(body.get("uname")),
                text: gift_text(&str_of(body.get("giftName")), body.get("num").and_then(as_f64).unwrap_or(1.0) as u64),
                money: if gold { total / 1000.0 } else { 0.0 },
                kind: DanmakuKind::Gift,
            })
        }
        // 新版送礼消息：礼物信息是 base64 编码的 protobuf
        "SEND_GIFT_V2" => {
            let pb = base64::engine::general_purpose::STANDARD.decode(body.get("pb")?.as_str()?).ok()?;
            let top = pb_fields(&pb)?;
            let gift = pb_fields(pb_bytes(&top, 10)?)?;
            let num = pb_int(&gift, 3).unwrap_or(1);
            let total = pb_int(&gift, 7).unwrap_or_else(|| pb_int(&gift, 5).unwrap_or(0) * num);
            let gold = pb_bytes(&gift, 8) == Some(b"gold".as_slice());
            let uid = pb_int(&top, 1).unwrap_or(0);
            let name = String::from_utf8_lossy(pb_bytes(&top, 2).unwrap_or_default()).to_string();
            Some(DanmakuEvent {
                t,
                user: if uid > 0 { uid.to_string() } else { name.clone() },
                name,
                text: gift_text(&String::from_utf8_lossy(pb_bytes(&gift, 2)?), num),
                money: if gold { total as f64 / 1000.0 } else { 0.0 },
                kind: DanmakuKind::Gift,
            })
        }
        "GUARD_BUY" => Some(DanmakuEvent {
            t,
            user: uid_or(body.get("uid"), body.get("username")),
            name: str_of(body.get("username")),
            text: body.get("gift_name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            money: body.get("price").and_then(as_f64).unwrap_or(0.0)
                * body.get("num").and_then(as_f64).unwrap_or(1.0)
                / 1000.0,
            kind: DanmakuKind::Guard,
        }),
        _ => None,
    }
}

fn gift_text(gift: &str, num: u64) -> String {
    if num > 1 { format!("{gift} × {num}") } else { gift.to_string() }
}

/// protobuf 字段值：整数或字节串（嵌套消息、字符串）
enum PbValue<'a> {
    Int(u64),
    Bytes(&'a [u8]),
}

/// 解析一层 protobuf，同号字段只保留第一个
fn pb_fields(mut buf: &[u8]) -> Option<Vec<(u64, PbValue<'_>)>> {
    fn varint(buf: &mut &[u8]) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let (&b, rest) = buf.split_first()?;
            *buf = rest;
            v |= u64::from(b & 0x7f) << shift;
            if b < 0x80 {
                return Some(v);
            }
        }
        None
    }
    let mut out = Vec::new();
    while !buf.is_empty() {
        let key = varint(&mut buf)?;
        let value = match key & 7 {
            0 => PbValue::Int(varint(&mut buf)?),
            2 => {
                let len = varint(&mut buf)? as usize;
                let (data, rest) = (buf.get(..len)?, buf.get(len..)?);
                buf = rest;
                PbValue::Bytes(data)
            }
            1 => {
                buf = buf.get(8..)?;
                continue;
            }
            5 => {
                buf = buf.get(4..)?;
                continue;
            }
            _ => return None,
        };
        if !out.iter().any(|(f, _)| *f == key >> 3) {
            out.push((key >> 3, value));
        }
    }
    Some(out)
}

fn pb_int(fields: &[(u64, PbValue)], field: u64) -> Option<u64> {
    fields.iter().find_map(|(f, v)| match v {
        PbValue::Int(n) if *f == field => Some(*n),
        _ => None,
    })
}

fn pb_bytes<'a>(fields: &[(u64, PbValue<'a>)], field: u64) -> Option<&'a [u8]> {
    fields.iter().find_map(|(f, v)| match v {
        PbValue::Bytes(b) if *f == field => Some(*b),
        _ => None,
    })
}

fn as_f64(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

/// XML 弹幕：`<d p="秒数,...">文本</d>`，秒数相对该分段开头。
fn parse_xml_line(line: &str, offset_start: f64) -> Vec<DanmakuEvent> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(i) = rest.find("<d p=\"") {
        let after = &rest[i + 6..];
        let Some(q) = after.find('"') else { break };
        let attrs: Vec<&str> = after[..q].split(',').collect();
        let body = &after[q + 1..];
        let text_start = body.find('>').map(|p| p + 1).unwrap_or(0);
        let text_end = body.find("</d>").unwrap_or(body.len());
        let text = if text_start <= text_end { &body[text_start..text_end] } else { "" };
        if let Some(sec) = attrs.first().and_then(|s| s.parse::<f64>().ok()) {
            out.push(DanmakuEvent {
                t: offset_start + sec,
                user: attrs.get(6).copied().unwrap_or("").to_string(),
                name: String::new(),
                text: text.to_string(),
                money: 0.0,
                kind: DanmakuKind::Chat,
            });
        }
        rest = &body[text_end..];
    }
    out
}

/// 归一化弹幕文本用于识别刷屏：去空白标点、小写、连续重复字符压到两个、截断。
pub fn normalize_text(text: &str) -> String {
    let mut out = String::new();
    let mut last: Option<char> = None;
    let mut run = 0;
    for ch in text.chars() {
        if ch.is_whitespace() || (ch.is_ascii_punctuation() && ch != '?') || "，。！、；：“”‘’（）【】《》…—~～".contains(ch) {
            continue;
        }
        let ch = if ch == '？' { '?' } else { ch.to_ascii_lowercase() };
        if Some(ch) == last {
            run += 1;
            if run >= 2 {
                continue;
            }
        } else {
            run = 0;
        }
        last = Some(ch);
        out.push(ch);
        if out.chars().count() >= 20 {
            break;
        }
    }
    out
}

/// 关键词类别及权重：笑、惊、燃、求切片
const KEYWORDS: &[(&[&str], f64)] = &[
    (&["哈哈", "草", "笑死", "233", "hh", "绷不住", "蚌埠住", "xswl", "笑不活", "乐"], 1.0),
    (&["??", "卧槽", "我草", "woc", "啊这", "离谱", "逆天", "震惊", "什么"], 1.0),
    (&["666", "牛", "nb", "神", "帅", "燃", "绝了", "yyds", "好听", "强"], 0.8),
    (&["切片", "录屏", "名场面", "高能", "经典", "截图", "前方高能", "剪了", "切了"], 3.0),
];

/// 一条弹幕的关键词得分（取命中类别中最高的权重）
pub fn keyword_score(normalized: &str) -> f64 {
    KEYWORDS
        .iter()
        .filter(|(words, _)| words.iter().any(|w| normalized.contains(w)))
        .map(|(_, weight)| *weight)
        .fold(0.0, f64::max)
}

/// 问候、下播、催下播之类的寒暄弹幕：刷得再多也不代表内容精彩
const FILLER: &[&str] = &[
    "晚安", "晚上好", "早上好", "中午好", "下午好", "早安", "拜拜", "再见", "88", "下播", "才八点", "才九点", "才十点", "主播好", "来了", "打卡", "签到",
];

/// 是否为寒暄类无效弹幕（只看短句，长句里顺带一句晚安不算）
pub fn is_filler(normalized: &str) -> bool {
    normalized.chars().count() <= 8 && FILLER.iter().any(|w| normalized.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-22T08:50:49+08:00").unwrap().with_timezone(&Utc)
    }

    #[test]
    fn parses_chat_superchat_and_gift_lines() {
        let chat = r#"{"cmd":"DANMU_MSG","data":{"cmd":"DANMU_MSG","info":[[0,1,25,16777215,1761094309000],"哈哈哈哈笑死",[123456,"观众A",0]]},"timestamp":"2026-10-22T08:51:49+08:00"}"#;
        let e = parse_jsonl_line(chat, base()).unwrap();
        assert_eq!((e.kind, e.t, e.user.as_str(), e.text.as_str()), (DanmakuKind::Chat, 60.0, "123456", "哈哈哈哈笑死"));

        let sc = r#"{"cmd":"SUPER_CHAT_MESSAGE","data":{"cmd":"SUPER_CHAT_MESSAGE","data":{"uid":42,"price":30,"message":"冲"}},"timestamp":"2026-10-22T08:52:49+08:00"}"#;
        let e = parse_jsonl_line(sc, base()).unwrap();
        assert_eq!((e.kind, e.money), (DanmakuKind::SuperChat, 30.0));

        let free_gift = r#"{"cmd":"SEND_GIFT","data":{"cmd":"SEND_GIFT","data":{"uid":7,"giftName":"辣条","coin_type":"silver","total_coin":1000}},"timestamp":"2026-10-22T08:52:49+08:00"}"#;
        assert_eq!(parse_jsonl_line(free_gift, base()).unwrap().money, 0.0);
        let paid_gift = r#"{"cmd":"SEND_GIFT","data":{"cmd":"SEND_GIFT","data":{"uid":7,"giftName":"小花花","coin_type":"gold","total_coin":5000}},"timestamp":"2026-10-22T08:52:49+08:00"}"#;
        assert_eq!(parse_jsonl_line(paid_gift, base()).unwrap().money, 5.0);
    }

    #[test]
    fn parses_protobuf_gift_v2() {
        let line = r#"{"cmd":"SEND_GIFT_V2","data":{"cmd":"SEND_GIFT_V2","data":{"pb":"CMS98KABEhLlh7rpm7LltI7jga3jga3lrZBSGBIJ5Lq65rCU56WoGAooZDjoB0IEZ29sZA=="}},"timestamp":"2026-10-22T08:52:49+08:00"}"#;
        let e = parse_jsonl_line(line, base()).unwrap();
        assert_eq!(
            (e.kind, e.user.as_str(), e.name.as_str(), e.text.as_str(), e.money),
            (DanmakuKind::Gift, "337387204", "出雲崎ねね子", "人气票 × 10", 1.0)
        );
    }

    #[test]
    fn normalizes_spam_variants_to_same_key() {
        assert_eq!(normalize_text("哈哈哈哈哈！"), normalize_text("哈哈 哈"));
        assert_eq!(normalize_text("？？？？"), "??");
        assert!(keyword_score(&normalize_text("前方高能！！")) >= 3.0);
        assert!(is_filler(&normalize_text("晚安晚安~")));
        assert!(!is_filler(&normalize_text("主播晚安前能不能讲讲李世民")));
    }

    #[test]
    fn parses_xml_offsets_relative_to_segment() {
        let line = r#"<d p="12.5,1,25,16777215,1700000000,0,abc123,0">好家伙</d><d p="13,1,25,16777215,1700000000,0,def456,0">草</d>"#;
        let events = parse_xml_line(line, 100.0);
        assert_eq!(events.len(), 2);
        assert_eq!((events[0].t, events[0].text.as_str()), (112.5, "好家伙"));
        assert_eq!(events[1].user, "def456");
    }
}
