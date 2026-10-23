//! 文本大模型：OpenAI 兼容接口，结构化 JSON 输出，切片判断、相对排序与证据校验。

use std::time::{Duration, Instant};

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::settings::LlmSettings;
use super::signals::Profile;

pub const MODE_ORDER: [&str; 3] = ["json_schema", "json_object", "prompt"];

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Judgement {
    #[serde(default)]
    pub keep: bool,
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

/// 去掉 ```json 包裹后解析 JSON；兜底截取第一个 { 到最后一个 }
fn parse_content(content: &str) -> Option<Value> {
    let trimmed = content.trim().trim_start_matches("```json").trim_start_matches("```").trim_end_matches("```").trim();
    serde_json::from_str(trimmed).ok().or_else(|| {
        let (a, b) = (trimmed.find('{')?, trimmed.rfind('}')?);
        serde_json::from_str(&trimmed[a..=b]).ok()
    })
}

/// 调用一次 chat completions，按 `mode` 约束输出，返回解析后的 JSON。限流与服务端错误退避重试两次。
pub async fn chat_json(settings: &LlmSettings, mode: &str, system: &str, user: &str, schema: &Value) -> Result<Value, String> {
    let client = client(settings)?;
    let mut system = system.to_string();
    let mut body = json!({
        "model": settings.model,
        "temperature": settings.temperature,
        "max_tokens": 1500,
    });
    match mode {
        "json_schema" => {
            body["response_format"] = json!({"type": "json_schema", "json_schema": {"name": "result", "schema": schema}});
        }
        "json_object" => {
            body["response_format"] = json!({"type": "json_object"});
            system.push_str(&format!("\n只输出一个 JSON 对象，结构遵循：{}", schema));
        }
        _ => system.push_str(&format!("\n只输出一个 JSON 对象，不要输出其他文字，结构遵循：{}", schema)),
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
        return parse_content(content).ok_or_else(|| format!("模型输出不是合法 JSON: {}", truncate(content, 200)));
    }
    Err(last_err)
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// 依次尝试三种结构化输出方式，返回第一种可用的
pub async fn test_connection(settings: &LlmSettings) -> TestResult {
    if !settings.is_configured() {
        return TestResult { message: "请先填写接口地址和模型名".into(), ..Default::default() };
    }
    let schema = json!({"type": "object", "properties": {"ok": {"type": "boolean"}}, "required": ["ok"]});
    let modes: Vec<&str> = if settings.structured_output == "auto" {
        MODE_ORDER.to_vec()
    } else {
        vec![settings.structured_output.as_str()]
    };
    let mut errors = Vec::new();
    for mode in modes {
        let started = Instant::now();
        match chat_json(settings, mode, "你是接口连通性测试助手。", "返回 {\"ok\": true}", &schema).await {
            Ok(v) if v.get("ok").and_then(|x| x.as_bool()) == Some(true) => {
                return TestResult { ok: true, mode: mode.into(), latency_ms: started.elapsed().as_millis() as u64, message: "连接成功".into() };
            }
            Ok(v) => errors.push(format!("{}: 返回内容不符合预期 {}", mode, truncate(&v.to_string(), 80))),
            Err(e) => errors.push(format!("{}: {}", mode, e)),
        }
    }
    TestResult { ok: false, message: errors.join("；"), ..Default::default() }
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

fn judgement_schema(titles: usize) -> Value {
    json!({
        "type": "object",
        "properties": {
            "keep": {"type": "boolean"},
            "category": {"type": "string", "enum": ["搞笑", "高燃", "名场面", "翻车", "感人", "互动", "干货", "其他"]},
            "start": {"type": "number"},
            "end": {"type": "number"},
            "titles": {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": titles},
            "description": {"type": "string"},
            "tags": {"type": "array", "items": {"type": "string"}, "maxItems": 8},
            "coverText": {"type": "string"},
            "reason": {"type": "string"},
            "evidence": {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": 3}
        },
        "required": ["keep", "category", "start", "end", "titles", "description", "tags", "coverText", "reason", "evidence"]
    })
}

fn profile_note(profile: Profile) -> &'static str {
    match profile {
        Profile::Reaction => "这是一场 reaction 直播：主播边看作品边解说。“主播”行是主播的中文解说，“原片”行是正在观看的作品原声（可能是外语）。高光以主播的反应、吐槽和互动为核心，不要把原片情节当成主播的表现。",
        Profile::Game => "这是一场游戏直播。高光通常是精彩操作、意外翻车、搞笑互动或情绪爆发，要结合主播的解说判断。",
        Profile::Chat => "这是一场杂谈直播。高光通常是有趣的观点、完整的小故事、和观众的精彩互动。",
    }
}

const BASE_RULES: &str = "你是B站直播切片编辑，判断一段直播录像中是否有值得单独投稿的高光片段。
规则：
1. 只能依据给出的转写与弹幕，不得编造其中没有的情节、人物、台词或观众反应；转写有识别错误时按上下文理解，不要放大。
2. start/end 必须取自转写行首的秒数；起点放在铺垫开始处，终点放在笑点或高潮之后的反应结束处。
3. 内容不够精彩时 keep=false，但其余字段仍按最佳候选填写。
4. 标题不超过 30 个汉字，口语化、有点击欲但不夸大；coverText 不超过 12 个汉字，用于压在封面上；description 50~120 字；tags 3~8 个。
5. evidence 逐字摘抄 1~3 句转写原文（不要改写），reason 用一两句话说明依据。";

pub struct WindowInput<'a> {
    pub room_title: &'a str,
    pub transcript: &'a str,
    pub danmaku: &'a [(String, usize)],
    pub signal_summary: &'a str,
}

pub async fn judge(settings: &LlmSettings, mode: &str, profile: Profile, titles: usize, input: &WindowInput<'_>) -> Result<Judgement, String> {
    let system = format!("{}\n{}\n每次给出 {} 个不同角度的标题。", BASE_RULES, profile_note(profile), titles);
    let danmaku = if input.danmaku.is_empty() {
        "（无弹幕）".to_string()
    } else {
        input.danmaku.iter().map(|(t, n)| if *n > 1 { format!("{} ×{}", t, n) } else { t.clone() }).collect::<Vec<_>>().join("\n")
    };
    let user = format!(
        "直播间标题：{}\n信号摘要：{}\n\n转写（每行：[起止秒][说话方|情绪/事件] 文本）：\n{}\n\n窗口内高频弹幕：\n{}",
        input.room_title, input.signal_summary, input.transcript, danmaku
    );
    let value = chat_json(settings, mode, &system, &user, &judgement_schema(titles)).await?;
    serde_json::from_value(value).map_err(|e| format!("模型输出字段不完整: {}", e))
}

/// 对已保留的候选做相对排序，返回按推荐程度从高到低的 id
pub async fn rank(settings: &LlmSettings, mode: &str, items: &[(usize, String, String)]) -> Result<Vec<usize>, String> {
    if items.len() < 2 {
        return Ok(items.iter().map(|i| i.0).collect());
    }
    let list = items
        .iter()
        .map(|(id, title, reason)| format!("{}. {} —— {}", id, title, reason))
        .collect::<Vec<_>>()
        .join("\n");
    let schema = json!({"type": "object", "properties": {"order": {"type": "array", "items": {"type": "integer"}}}, "required": ["order"]});
    let value = chat_json(
        settings,
        mode,
        "你是B站直播切片编辑。按照作为独立投稿的吸引力，从高到低给下列候选排序，只输出编号。",
        &list,
        &schema,
    )
    .await?;
    let order: Vec<usize> = value["order"].as_array().map(|a| a.iter().filter_map(|v| v.as_u64().map(|x| x as usize)).collect()).unwrap_or_default();
    Ok(order)
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
    }
}
