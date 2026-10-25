//! 文本大模型：OpenAI 兼容接口，结构化 JSON 输出，按话题切分与证据校验。

use std::time::{Duration, Instant};

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::settings::LlmSettings;
use super::signals::Profile;

/// 大模型从一段转写里切出的一个话题切片
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicClip {
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub start: f64,
    #[serde(default)]
    pub end: f64,
    #[serde(default)]
    pub titles: Vec<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, alias = "cover_text")]
    pub cover_text: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub scores: TopicScores,
}

/// 大模型给的分项评分（1~10）
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicScores {
    #[serde(default)]
    pub funny: f64,
    #[serde(default)]
    pub complete: f64,
    #[serde(default)]
    pub streamer: f64,
    #[serde(default)]
    pub standalone: f64,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    pub ok: bool,
    pub mode: String,
    pub latency_ms: u64,
    pub message: String,
}

fn is_local(base_url: &str) -> bool {
    url::Url::parse(base_url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .map(|h| h == "localhost" || h == "127.0.0.1" || h == "::1" || h == "[::1]")
        .unwrap_or(false)
}

fn client(settings: &LlmSettings) -> Result<Client, String> {
    let mut builder = Client::builder().timeout(Duration::from_secs(settings.timeout_secs));
    if is_local(&settings.base_url) {
        builder = builder.no_proxy();
    }
    builder.build().map_err(|e| e.to_string())
}

fn request(client: &Client, settings: &LlmSettings, path: &str) -> reqwest::RequestBuilder {
    let url = format!("{}/{}", settings.base_url.trim_end_matches('/'), path);
    let mut req = if path == "models" { client.get(url) } else { client.post(url) };
    if !settings.api_key.is_empty() {
        req = req.bearer_auth(&settings.api_key);
    }
    req
}

const NOT_JSON: &str = "模型输出不是合法 JSON: ";

/// 找出文本中所有顶层的完整 JSON 对象（跳过字符串内的括号）
fn json_objects(text: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let (mut depth, mut start, mut in_str, mut escaped) = (0usize, None, false, false);
    for (i, ch) in text.char_indices() {
        if in_str {
            match (escaped, ch) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => in_str = false,
                _ => {}
            }
            continue;
        }
        match ch {
            '"' if depth > 0 => in_str = true,
            '{' => {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            }
            '}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    if let Some(v) = start.and_then(|st| serde_json::from_str::<Value>(&text[st..=i]).ok()) {
                        out.push(v);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// 解析模型输出：优先整体解析，否则取第一个不是 JSON Schema 本身的对象（模型有时会先复述 schema）
fn parse_content(content: &str) -> Option<Value> {
    let trimmed = content.trim().trim_start_matches("```json").trim_start_matches("```").trim_end_matches("```").trim();
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        if !looks_like_schema(&v) {
            return Some(v);
        }
    }
    json_objects(trimmed).into_iter().find(|v| !looks_like_schema(v))
}

fn looks_like_schema(v: &Value) -> bool {
    v.get("properties").is_some() && v.get("type").and_then(|t| t.as_str()) == Some("object")
}

/// 调用一次 chat completions，按 `mode` 约束输出，返回解析后的 JSON。限流与服务端错误退避重试两次。
/// 所有模式都把 schema 写进系统提示词：部分网关会忽略 response_format，只靠参数约束不可靠。
pub async fn chat_json(settings: &LlmSettings, mode: &str, system: &str, user: &str, schema: &Value) -> Result<Value, String> {
    chat_json_inner(settings, mode, system, user, schema, true).await
}

async fn chat_json_inner(settings: &LlmSettings, mode: &str, system: &str, user: &str, schema: &Value, schema_hint: bool) -> Result<Value, String> {
    let client = client(settings)?;
    let mut system = system.to_string();
    let mut body = json!({
        "model": settings.model,
        "temperature": settings.temperature,
        // 推理类模型会把思考过程计入输出长度，上限过小会截断 JSON
        "max_tokens": 16384,
    });
    if !settings.reasoning_effort.is_empty() {
        body["reasoning_effort"] = json!(settings.reasoning_effort);
    }
    match mode {
        "json_schema" => {
            body["response_format"] = json!({"type": "json_schema", "json_schema": {"name": "result", "schema": schema}});
        }
        "json_object" => {
            body["response_format"] = json!({"type": "json_object"});
        }
        _ => {}
    }
    if schema_hint {
        system.push_str(&format!(
            "\n只输出一个 JSON 对象，不要输出其他文字。按下面的 JSON Schema 填入实际内容，不要输出 Schema 本身：\n{}",
            schema
        ));
    }
    body["messages"] = json!([{"role": "system", "content": system}, {"role": "user", "content": user}]);
    let mut last_err = String::new();
    for attempt in 0..3 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_secs(2 * attempt as u64 * attempt as u64)).await;
        }
        let resp = match request(&client, settings, "chat/completions").json(&body).send().await {
            Ok(r) => r,
            Err(e) => {
                last_err = format!("请求失败: {}", e);
                continue;
            }
        };
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if status.as_u16() == 429 || status.is_server_error() {
            last_err = format!("HTTP {}: {}", status, truncate(&text, 200));
            continue;
        }
        if !status.is_success() {
            return Err(format!("HTTP {}: {}", status, truncate(&text, 300)));
        }
        let data: Value = serde_json::from_str(&text).map_err(|_| format!("响应不是 JSON: {}", truncate(&text, 200)))?;
        let content = data["choices"][0]["message"]["content"].as_str().unwrap_or("");
        if let Some(v) = parse_content(content) {
            return Ok(v);
        }
        if data["choices"][0]["finish_reason"].as_str() == Some("length") {
            return Err("模型输出被截断（达到输出长度上限）".to_string());
        }
        return Err(format!("{}{}", NOT_JSON, truncate(content, 200)));
    }
    Err(last_err)
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// 测试连接。自动检测时，用不提及 JSON 的请求探测服务端是否真正执行结构化约束：
/// 只有约束生效时才会返回 JSON；都不生效则退回「仅提示词」（schema 写在提示词里，解析时容错）。
/// 手动指定档位时只检查连通性与 JSON 解析。
pub async fn test_connection(settings: &LlmSettings) -> TestResult {
    if !settings.is_configured() {
        return TestResult { message: "请先填写接口地址和模型名".into(), ..Default::default() };
    }
    let schema = json!({"type": "object", "properties": {"greeting": {"type": "string"}}, "required": ["greeting"]});
    let probe = |mode: &'static str, hint: bool| {
        let schema = schema.clone();
        async move {
            let started = Instant::now();
            let r = chat_json_inner(settings, mode, "你是一个友好的助手。", "用一句话向我问好。", &schema, hint).await;
            (r, started.elapsed().as_millis() as u64)
        }
    };
    let ok_value = |v: &Value| v.get("greeting").and_then(|x| x.as_str()).is_some();
    if settings.structured_output != "auto" {
        let mode: &'static str = match settings.structured_output.as_str() {
            "json_schema" => "json_schema",
            "json_object" => "json_object",
            _ => "prompt",
        };
        return match probe(mode, true).await {
            (Ok(v), ms) if ok_value(&v) => TestResult { ok: true, mode: mode.into(), latency_ms: ms, message: "连接成功".into() },
            (Ok(v), _) => TestResult { message: format!("返回内容不符合预期 {}", truncate(&v.to_string(), 80)), ..Default::default() },
            (Err(e), _) => TestResult { message: e, ..Default::default() },
        };
    }
    let mut errors = Vec::new();
    for mode in ["json_schema", "json_object"] {
        match probe(mode, false).await {
            (Ok(v), ms) if ok_value(&v) => {
                return TestResult { ok: true, mode: mode.into(), latency_ms: ms, message: "连接成功".into() };
            }
            (Err(e), _) if e.starts_with(NOT_JSON) => errors.push(format!("{} 未生效", mode)),
            (Ok(_), _) => errors.push(format!("{} 未生效", mode)),
            (Err(e), _) => errors.push(format!("{} 不可用（{}）", mode, truncate(&e, 60))),
        }
    }
    match probe("prompt", true).await {
        (Ok(v), ms) if ok_value(&v) => TestResult {
            ok: true,
            mode: "prompt".into(),
            latency_ms: ms,
            message: format!("连接成功；服务端不执行结构化约束（{}），将依靠提示词输出 JSON", errors.join("，")),
        },
        (Ok(v), _) => TestResult { message: format!("返回内容不符合预期 {}", truncate(&v.to_string(), 80)), ..Default::default() },
        (Err(e), _) => TestResult { message: e, ..Default::default() },
    }
}

pub async fn list_models(settings: &LlmSettings) -> Result<Vec<String>, String> {
    let client = client(settings)?;
    let resp = request(&client, settings, "models").send().await.map_err(|e| format!("请求失败: {}", e))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("HTTP {}: {}", status, truncate(&text, 200)));
    }
    let data: Value = serde_json::from_str(&text).map_err(|_| "响应不是 JSON".to_string())?;
    let mut ids: Vec<String> = data["data"]
        .as_array()
        .map(|list| list.iter().filter_map(|m| m["id"].as_str().map(String::from)).collect())
        .unwrap_or_default();
    ids.sort();
    Ok(ids)
}

fn topics_schema(titles: usize) -> Value {
    let score = json!({"type": "integer", "minimum": 1, "maximum": 10});
    json!({
        "type": "object",
        "properties": {
            "clips": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "category": {"type": "string", "enum": ["搞笑", "高燃", "名场面", "翻车", "感人", "互动", "干货", "其他"]},
                        "start": {"type": "number"},
                        "end": {"type": "number"},
                        "titles": {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": titles},
                        "description": {"type": "string"},
                        "tags": {"type": "array", "items": {"type": "string"}, "maxItems": 8},
                        "coverText": {"type": "string"},
                        "reason": {"type": "string"},
                        "evidence": {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": 3},
                        "scores": {
                            "type": "object",
                            "properties": {"funny": score, "complete": score, "streamer": score, "standalone": score},
                            "required": ["funny", "complete", "streamer", "standalone"]
                        }
                    },
                    "required": ["category", "start", "end", "titles", "description", "tags", "coverText", "reason", "evidence", "scores"]
                }
            }
        },
        "required": ["clips"]
    })
}

fn profile_note(profile: Profile) -> &'static str {
    match profile {
        Profile::Reaction => "这是一场 reaction 直播：主播边看作品边解说。“主播”行是主播的中文解说，“原片”行是正在观看的作品原声（可能是外语）。高光以主播的反应、吐槽和互动为核心，不要把原片情节当成主播的表现。",
        Profile::Game => "这是一场游戏直播。高光通常是精彩操作、意外翻车、搞笑互动或情绪爆发，要结合主播的解说判断。",
        Profile::Chat => "这是一场杂谈直播。高光通常是有趣的观点、完整的小故事、和观众的精彩互动。",
    }
}

const BASE_RULES: &str = "你是B站直播切片编辑。给你一段直播录像的逐句转写和每分钟的弹幕情况，请把其中值得单独投稿的内容切成一条条切片。
规则：
1. 只能依据给出的转写与弹幕，不得编造其中没有的情节、人物、台词或观众反应；转写有识别错误时按上下文理解，不要放大。
2. 每条切片是一个完整的话题或事件：从主播开始聊这件事的那句开始（由观众提问、SC、弹幕引出的，从主播念这条提问开始），到主播把这件事聊完为止。话题一转就结束，不要带上后面无关的内容，也不要从前面无关的闲聊开始。
3. start 取切片第一行的起始秒，end 取切片最后一行的结束秒，都必须是转写里出现的秒数。
4. 相邻几段属于同一类话题、弹幕也在讨论同一件事时，合成一条；话题短、没有内容的单独一句不要切。
5. 弹幕只用来帮你发现观众反应强烈的地方，最终按转写内容判断；寒暄、催下播、念礼物感谢、纯原片播放没有主播内容的段落不要切。
6. 宁多勿漏：凡是能独立成片的话题都输出，平淡的给低分即可。
7. 分项评分 1~10：funny 有趣或精彩程度；complete 话题完整度（开头或结尾被本段转写截断的打低分）；streamer 主播的表现（观点、反应、情绪）；standalone 不看前后文能否看懂。
8. 标题不超过 30 个汉字，口语化、有点击欲但不夸大；coverText 不超过 12 个汉字，用于压在封面上；description 50~120 字；tags 3~8 个。
9. evidence 逐字摘抄 1~3 句该切片内的转写原文（不要改写）；reason 用一两句话说明推荐理由：亮点是什么、起止为什么选在这里。";

pub struct ChunkInput<'a> {
    pub room_title: &'a str,
    pub transcript: &'a str,
    pub danmaku: &'a str,
    pub max_secs: i64,
}

/// 把一段转写切成若干话题切片
pub async fn segment_topics(settings: &LlmSettings, mode: &str, profile: Profile, titles: usize, input: &ChunkInput<'_>) -> Result<Vec<TopicClip>, String> {
    let system = format!(
        "{}\n{}\n单条切片一般在 30 秒到 {} 秒之间。每条给出 {} 个不同角度的标题。没有值得切的内容时 clips 为空数组。",
        BASE_RULES,
        profile_note(profile),
        input.max_secs,
        titles
    );
    let user = format!(
        "直播间标题：{}\n\n转写（每行：[起止秒][说话方|情绪/事件] 文本）：\n{}\n\n每分钟弹幕：\n{}",
        input.room_title, input.transcript, input.danmaku
    );
    let value = chat_json(settings, mode, &system, &user, &topics_schema(titles)).await?;
    serde_json::from_value(value["clips"].clone()).map_err(|e| format!("模型输出字段不完整: {}", e))
}

fn normalize(s: &str) -> Vec<char> {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

fn longest_common_substring(a: &[char], b: &[char]) -> usize {
    let mut best = 0;
    let mut prev = vec![0usize; b.len() + 1];
    for &ca in a {
        let mut cur = vec![0usize; b.len() + 1];
        for (j, &cb) in b.iter().enumerate() {
            if ca == cb {
                cur[j + 1] = prev[j] + 1;
                best = best.max(cur[j + 1]);
            }
        }
        prev = cur;
    }
    best
}

/// 在转写中逐位对齐，取相同字符数最多的位置（容忍语音识别的单字替换）
fn best_aligned_matches(e: &[char], hay: &[char]) -> usize {
    if hay.len() < e.len() {
        return 0;
    }
    (0..=hay.len() - e.len())
        .map(|off| e.iter().zip(&hay[off..]).filter(|(a, b)| a == b).count())
        .max()
        .unwrap_or(0)
}

/// 每条证据去掉标点后，须与转写有不少于 80% 的重合：连续重合或逐位对齐重合满足其一
pub fn evidence_valid(evidence: &[String], transcript_text: &str) -> bool {
    let hay = normalize(transcript_text);
    let items: Vec<Vec<char>> = evidence.iter().map(|e| normalize(e)).filter(|e| !e.is_empty()).collect();
    !items.is_empty()
        && items.iter().all(|e| {
            let need = ((e.len() as f64) * 0.8).ceil() as usize;
            longest_common_substring(e, &hay) >= need.max(1) || best_aligned_matches(e, &hay) >= need.max(1)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_must_come_from_transcript() {
        let transcript = "我觉得这个人设丑到我想报警。尤其是这个查理亚，看着像女主！";
        assert!(evidence_valid(&["人设丑到我想报警".into()], transcript));
        // 少量识别差异可以接受
        assert!(evidence_valid(&["尤其是这个查理啊看着像女主".into()], transcript));
        // 原文没有的情节
        assert!(!evidence_valid(&["原片突然喊出塔利亚".into()], transcript));
        assert!(!evidence_valid(&[], transcript));
    }

    #[test]
    fn parses_fenced_json() {
        assert_eq!(parse_content("```json\n{\"ok\": true}\n```").unwrap()["ok"], true);
        assert_eq!(parse_content("好的：{\"ok\": true} 完成").unwrap()["ok"], true);
        // 模型先复述 schema 再给结果
        let echoed = r#"{"type":"object","properties":{"ok":{"type":"boolean"}}} {"ok": true, "note": "a}b"}"#;
        assert_eq!(parse_content(echoed).unwrap()["ok"], true);
    }
}
