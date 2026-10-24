//! 自动切片配置：切片参数、文本大模型、语音识别，整体以 JSON 存在 app_settings 的一个键里。

use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::utils::now_rfc3339;

const SETTINGS_KEY: &str = "auto_clip_settings";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ClipSettings {
    /// 直播结束后自动创建切片任务
    pub enabled: bool,
    /// 开启自动切片的时间（RFC3339），只处理此后结束的场次，避免开启时积压历史任务
    pub enabled_since: String,
    pub candidates_per_hour: f64,
    pub max_candidates: usize,
    pub min_duration_secs: i64,
    pub max_duration_secs: i64,
    /// 候选窗口起点前移秒数，补偿弹幕滞后
    pub danmaku_lag_secs: i64,
    /// reaction 主播：切片内主播中文说话时长占比下限（百分比）
    pub min_streamer_speech_percent: f64,
    /// 相邻分段间隔不超过该分钟数视为同一场次
    pub session_gap_minutes: i64,
    pub use_llm: bool,
    pub title_candidates: usize,
}

impl Default for ClipSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            enabled_since: String::new(),
            candidates_per_hour: 2.0,
            max_candidates: 15,
            min_duration_secs: 45,
            max_duration_secs: 300,
            danmaku_lag_secs: 30,
            min_streamer_speech_percent: 30.0,
            session_gap_minutes: 30,
            use_llm: true,
            title_candidates: 3,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LlmSettings {
    pub preset: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    /// auto | json_schema | json_object | prompt
    pub structured_output: String,
    /// 思考档位，作为 reasoning_effort 发送；为空不发送，由服务端决定
    pub reasoning_effort: String,
    pub temperature: f64,
    pub timeout_secs: u64,
    pub max_concurrency: usize,
    pub context_tokens: usize,
}

impl Default for LlmSettings {
    fn default() -> Self {
        Self {
            preset: "custom".to_string(),
            base_url: String::new(),
            api_key: String::new(),
            model: String::new(),
            structured_output: "auto".to_string(),
            reasoning_effort: String::new(),
            temperature: 0.3,
            timeout_secs: 120,
            max_concurrency: 1,
            context_tokens: 32768,
        }
    }
}

impl LlmSettings {
    pub fn is_configured(&self) -> bool {
        !self.base_url.trim().is_empty() && !self.model.trim().is_empty()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AsrSettings {
    /// local | api（api 为后续阶段）
    pub mode: String,
    /// 为空时使用应用数据目录下 models/asr
    pub local_model_dir: String,
    /// hf_mirror | huggingface
    pub download_source: String,
    pub threads: i32,
}

impl Default for AsrSettings {
    fn default() -> Self {
        Self {
            mode: "local".to_string(),
            local_model_dir: String::new(),
            download_source: "hf_mirror".to_string(),
            threads: 4,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AutoClipSettings {
    pub clip: ClipSettings,
    pub llm: LlmSettings,
    pub asr: AsrSettings,
}

pub fn load(db: &Db) -> AutoClipSettings {
    db.with_conn(|conn| {
        conn.query_row(
            "SELECT value FROM app_settings WHERE key = ?1",
            [SETTINGS_KEY],
            |row| row.get::<_, String>(0),
        )
    })
    .ok()
    .and_then(|raw| serde_json::from_str(&raw).ok())
    .unwrap_or_default()
}

/// 保存配置；incoming 中的 API Key 若仍是掩码则保留原值。
pub fn save(db: &Db, mut incoming: AutoClipSettings) -> Result<AutoClipSettings, String> {
    let current = load(db);
    incoming.llm.api_key = resolve_api_key(&incoming.llm.api_key, &current.llm.api_key);
    incoming.clip.enabled_since = match (incoming.clip.enabled, current.clip.enabled) {
        (true, true) if !current.clip.enabled_since.is_empty() => current.clip.enabled_since.clone(),
        (true, _) => now_rfc3339(),
        (false, _) => String::new(),
    };
    sanitize(&mut incoming);
    let raw = serde_json::to_string(&incoming).map_err(|err| err.to_string())?;
    let now = now_rfc3339();
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO app_settings (key, value, updated_at) VALUES (?1, ?2, ?3) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            (SETTINGS_KEY, &raw, &now),
        )
    })
    .map_err(|err| format!("保存自动切片配置失败: {}", err))?;
    Ok(incoming)
}

/// 前端回传的 Key 若等于掩码，说明用户没改，沿用已保存的值。
pub fn resolve_api_key(incoming: &str, saved: &str) -> String {
    if !saved.is_empty() && incoming == mask_key(saved) {
        saved.to_string()
    } else {
        incoming.trim().to_string()
    }
}

pub fn mask_key(key: &str) -> String {
    if key.is_empty() {
        return String::new();
    }
    let tail: String = key.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    format!("****{}", tail)
}

/// 返回给前端的副本：Key 只保留掩码。
pub fn masked(settings: &AutoClipSettings) -> AutoClipSettings {
    let mut copy = settings.clone();
    copy.llm.api_key = mask_key(&settings.llm.api_key);
    copy
}

fn sanitize(s: &mut AutoClipSettings) {
    let c = &mut s.clip;
    c.candidates_per_hour = c.candidates_per_hour.clamp(0.5, 10.0);
    c.max_candidates = c.max_candidates.clamp(1, 50);
    c.min_duration_secs = c.min_duration_secs.clamp(15, 600);
    c.max_duration_secs = c.max_duration_secs.clamp(c.min_duration_secs + 15, 1200);
    c.danmaku_lag_secs = c.danmaku_lag_secs.clamp(0, 120);
    c.min_streamer_speech_percent = c.min_streamer_speech_percent.clamp(0.0, 100.0);
    c.session_gap_minutes = c.session_gap_minutes.clamp(5, 240);
    c.title_candidates = c.title_candidates.clamp(1, 5);
    let l = &mut s.llm;
    l.base_url = l.base_url.trim().trim_end_matches('/').to_string();
    l.model = l.model.trim().to_string();
    if !["", "off", "low", "medium", "high"].contains(&l.reasoning_effort.as_str()) {
        l.reasoning_effort = String::new();
    }
    l.temperature = l.temperature.clamp(0.0, 1.5);
    l.timeout_secs = l.timeout_secs.clamp(10, 900);
    l.max_concurrency = l.max_concurrency.clamp(1, 16);
    l.context_tokens = l.context_tokens.clamp(4096, 1_000_000);
    if !["auto", "json_schema", "json_object", "prompt"].contains(&l.structured_output.as_str()) {
        l.structured_output = "auto".to_string();
    }
    let a = &mut s.asr;
    a.threads = a.threads.clamp(1, 16);
    if !["hf_mirror", "huggingface"].contains(&a.download_source.as_str()) {
        a.download_source = "hf_mirror".to_string();
    }
    if a.mode != "local" && a.mode != "api" {
        a.mode = "local".to_string();
    }
}
