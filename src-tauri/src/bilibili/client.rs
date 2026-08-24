use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, REFERER, USER_AGENT};
use reqwest::{Client, StatusCode};
use serde_json::Value;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::sleep;

use crate::bilibili::signer::WbiSigner;
use crate::login_store::AuthInfo;

const ARCHIVE_REQUEST_MIN_INTERVAL: Duration = Duration::from_millis(1500);
const ARCHIVE_RATE_LIMIT_BASE_COOLDOWN_SECS: u64 = 60;
const ARCHIVE_RATE_LIMIT_MAX_COOLDOWN_SECS: u64 = 30 * 60;
const ARCHIVE_BAN_COOLDOWN_SECS: u64 = 30 * 60;

#[derive(Default)]
struct ArchiveRequestState {
    last_request_at: Option<Instant>,
    cooldown_until: Option<Instant>,
    consecutive_rate_limits: u32,
    last_rate_limit_error: Option<String>,
}

impl ArchiveRequestState {
    fn request_wait(&self, now: Instant) -> Option<Duration> {
        self.last_request_at.and_then(|last_request_at| {
            ARCHIVE_REQUEST_MIN_INTERVAL.checked_sub(now.saturating_duration_since(last_request_at))
        })
    }

    fn active_cooldown_error(&mut self, now: Instant) -> Option<String> {
        let Some(cooldown_until) = self.cooldown_until else {
            return None;
        };
        let Some(remaining) = cooldown_until.checked_duration_since(now) else {
            self.cooldown_until = None;
            self.last_rate_limit_error = None;
            return None;
        };
        let remaining_secs = remaining.as_secs().saturating_add(1);
        Some(format!(
            "B站稿件接口处于风控冷却中，约 {} 秒后可重试。上次错误: {}",
            remaining_secs,
            self.last_rate_limit_error
                .as_deref()
                .unwrap_or("请求过于频繁")
        ))
    }

    fn record_success(&mut self) {
        self.consecutive_rate_limits = 0;
        self.cooldown_until = None;
        self.last_rate_limit_error = None;
    }

    fn record_error(&mut self, error: &str, now: Instant) -> Option<Duration> {
        if !is_bilibili_risk_control_error(error) {
            return None;
        }

        self.consecutive_rate_limits = self.consecutive_rate_limits.saturating_add(1);
        let cooldown = if is_bilibili_ban_error(error) {
            Duration::from_secs(ARCHIVE_BAN_COOLDOWN_SECS)
        } else {
            let exponent = self.consecutive_rate_limits.saturating_sub(1).min(10);
            let multiplier = 1_u64 << exponent;
            Duration::from_secs(
                ARCHIVE_RATE_LIMIT_BASE_COOLDOWN_SECS
                    .saturating_mul(multiplier)
                    .min(ARCHIVE_RATE_LIMIT_MAX_COOLDOWN_SECS),
            )
        };
        self.cooldown_until = now.checked_add(cooldown);
        self.last_rate_limit_error = Some(error.to_string());
        Some(cooldown)
    }
}

pub struct BilibiliClient {
    client: Client,
    base_url: String,
    passport_base_url: String,
    signer: WbiSigner,
    buvid3: Mutex<Option<String>>,
    archive_request_state: AsyncMutex<ArchiveRequestState>,
}

impl BilibiliClient {
    pub fn new() -> Self {
        Self {
            // B站为国内 CDN，强制直连、忽略系统/环境变量代理：绕道代理只会更慢且多一层单点。
            // TUN 模式在 IP 层接管则无法在此绕过，需在 Clash 侧为 B站域名配 DIRECT 规则。
            client: Client::builder()
                .no_proxy()
                .build()
                .unwrap_or_else(|_| Client::new()),
            base_url: "https://api.bilibili.com".to_string(),
            passport_base_url: "https://passport.bilibili.com".to_string(),
            signer: WbiSigner::new(),
            buvid3: Mutex::new(None),
            archive_request_state: AsyncMutex::new(ArchiveRequestState::default()),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn passport_base_url(&self) -> &str {
        &self.passport_base_url
    }

    pub async fn get_json(
        &self,
        url: &str,
        params: &[(String, String)],
        auth: Option<&AuthInfo>,
        use_wbi: bool,
    ) -> Result<Value, String> {
        let full_url = if use_wbi {
            let query = self.signer.sign_params(&self.client, params).await?;
            format!("{}?{}", url, query)
        } else if params.is_empty() {
            url.to_string()
        } else {
            format!("{}?{}", url, build_query(params))
        };

        let mut headers = default_headers();
        let mut cookie_value = auth.map(|info| info.cookie.clone()).unwrap_or_default();
        if use_wbi {
            cookie_value = self.ensure_buvid3_cookie(&cookie_value).await?;
        }
        if !cookie_value.is_empty() {
            headers.insert(
                "Cookie",
                HeaderValue::from_str(&cookie_value)
                    .map_err(|_| "Invalid cookie header".to_string())?,
            );
        }
        if url.contains("live.bilibili.com") {
            headers.insert(
                REFERER,
                HeaderValue::from_static("https://live.bilibili.com/"),
            );
            headers.insert(
                "Origin",
                HeaderValue::from_static("https://live.bilibili.com"),
            );
        }
        if url.contains("member.bilibili.com") {
            headers.insert(
                REFERER,
                HeaderValue::from_static("https://member.bilibili.com/"),
            );
            headers.insert(
                "Origin",
                HeaderValue::from_static("https://member.bilibili.com"),
            );
        }
        if is_season_archive_request_url(url) {
            headers.insert(
                REFERER,
                HeaderValue::from_static("https://space.bilibili.com/"),
            );
        }

        let is_archive_request = is_archive_request_url(url);
        let mut archive_state = if is_archive_request {
            let mut state = self.archive_request_state.lock().await;
            let now = Instant::now();
            if let Some(error) = state.active_cooldown_error(now) {
                return Err(error);
            }
            if let Some(wait) = state.request_wait(now) {
                sleep(wait).await;
            }
            state.last_request_at = Some(Instant::now());
            Some(state)
        } else {
            None
        };

        let result = match self.client.get(full_url).headers(headers).send().await {
            Ok(response) => parse_http_response(response).await,
            Err(err) => Err(format!("Request failed: {}", err)),
        };

        if let Some(state) = archive_state.as_mut() {
            match result.as_ref() {
                Ok(_) => state.record_success(),
                Err(error) => {
                    state.record_error(error, Instant::now());
                }
            }
        }
        result
    }

    #[allow(dead_code)]
    pub async fn post_json(
        &self,
        url: &str,
        params: &[(String, String)],
        body: &Value,
        auth: Option<&AuthInfo>,
    ) -> Result<Value, String> {
        let full_url = if params.is_empty() {
            url.to_string()
        } else {
            format!("{}?{}", url, build_query(params))
        };

        let mut headers = default_headers();
        if let Some(auth) = auth {
            headers.insert(
                "Cookie",
                HeaderValue::from_str(&auth.cookie)
                    .map_err(|_| "Invalid cookie header".to_string())?,
            );
        }
        if url.contains("live.bilibili.com") {
            headers.insert(
                REFERER,
                HeaderValue::from_static("https://live.bilibili.com/"),
            );
            headers.insert(
                "Origin",
                HeaderValue::from_static("https://live.bilibili.com"),
            );
        }
        if url.contains("member.bilibili.com") {
            headers.insert(
                REFERER,
                HeaderValue::from_static("https://member.bilibili.com/"),
            );
            headers.insert(
                "Origin",
                HeaderValue::from_static("https://member.bilibili.com"),
            );
        }

        let response = self
            .client
            .post(full_url)
            .headers(headers)
            .json(body)
            .send()
            .await
            .map_err(|err| format!("Request failed: {}", err))?;

        parse_http_response(response).await
    }

    pub async fn post_form(
        &self,
        url: &str,
        params: &[(String, String)],
        form: &[(String, String)],
        auth: Option<&AuthInfo>,
    ) -> Result<Value, String> {
        let full_url = if params.is_empty() {
            url.to_string()
        } else {
            format!("{}?{}", url, build_query(params))
        };

        let mut headers = default_headers();
        if let Some(auth) = auth {
            headers.insert(
                "Cookie",
                HeaderValue::from_str(&auth.cookie)
                    .map_err(|_| "Invalid cookie header".to_string())?,
            );
        }
        if url.contains("live.bilibili.com") {
            headers.insert(
                REFERER,
                HeaderValue::from_static("https://live.bilibili.com/"),
            );
            headers.insert(
                "Origin",
                HeaderValue::from_static("https://live.bilibili.com"),
            );
        }

        let response = self
            .client
            .post(full_url)
            .headers(headers)
            .form(form)
            .send()
            .await
            .map_err(|err| format!("Request failed: {}", err))?;

        parse_http_response(response).await
    }

    pub fn cached_buvid3(&self) -> Option<String> {
        self.buvid3.lock().ok().and_then(|guard| guard.clone())
    }

    async fn ensure_buvid3_cookie(&self, cookie: &str) -> Result<String, String> {
        if cookie_has_key(cookie, "buvid3") {
            return Ok(cookie.to_string());
        }

        let buvid3 = self.fetch_buvid3().await?;
        Ok(append_cookie(cookie, "buvid3", &buvid3))
    }

    async fn fetch_buvid3(&self) -> Result<String, String> {
        let cached = self.buvid3.lock().ok().and_then(|guard| guard.clone());
        if let Some(value) = cached {
            return Ok(value);
        }

        let response = self
            .client
            .get("https://api.bilibili.com/x/web-frontend/getbuvid")
            .headers(default_headers())
            .send()
            .await
            .map_err(|err| format!("Request failed: {}", err))?;

        let data = parse_http_response(response).await?;
        let buvid3 = data
            .get("buvid")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "Failed to parse buvid3".to_string())?
            .to_string();

        if let Ok(mut guard) = self.buvid3.lock() {
            *guard = Some(buvid3.clone());
        }

        Ok(buvid3)
    }
}

async fn parse_http_response(response: reqwest::Response) -> Result<Value, String> {
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|err| format!("Failed to read response: {}", err))?;

    if status == StatusCode::PRECONDITION_FAILED {
        return Err("request was banned (code: -412, HTTP 412)".to_string());
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        return Err("请求频率过高，请稍后再试 (code: -509, HTTP 429)".to_string());
    }
    if !status.is_success() {
        if let Err(error) = parse_response(&body) {
            return Err(error);
        }
        return Err(format!("Bilibili returned HTTP {}", status.as_u16()));
    }

    parse_response(&body)
}

fn is_archive_request_url(url: &str) -> bool {
    url.contains("member.bilibili.com/x/web/archives") || is_season_archive_request_url(url)
}

fn is_season_archive_request_url(url: &str) -> bool {
    url.contains("api.bilibili.com/x/polymer/web-space/seasons_archives_list")
}

pub fn is_bilibili_ban_error(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    error.contains("code: -412")
        || error.contains("HTTP 412")
        || lower.contains("request was banned")
}

pub fn is_bilibili_risk_control_error(error: &str) -> bool {
    is_bilibili_ban_error(error)
        || error.contains("code: -509")
        || error.contains("code: -702")
        || error.contains("HTTP 429")
        || error.contains("请求过于频繁")
        || error.contains("请求频率过高")
}

fn parse_response(response: &str) -> Result<Value, String> {
    let value: Value = serde_json::from_str(response)
        .map_err(|err| format!("Failed to parse response: {}", err))?;
    if let Some(code) = value.get("code").and_then(|value| value.as_i64()) {
        if code != 0 {
            let message = value
                .get("message")
                .and_then(|value| value.as_str())
                .unwrap_or("Bilibili returned an error");
            return Err(format!("{} (code: {})", message, code));
        }
    }

    if let Some(data) = value.get("data") {
        return Ok(data.clone());
    }

    Ok(value)
}

fn default_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
    USER_AGENT,
    HeaderValue::from_static(
      "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/132.0.0.0 Safari/537.36 Edg/132.0.0.0",
    ),
  );
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/json, text/javascript, */*; q=0.01"),
    );
    headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("zh-CN"));
    headers
}

fn build_query(params: &[(String, String)]) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in params {
        serializer.append_pair(key, value);
    }
    serializer.finish()
}

fn cookie_has_key(cookie: &str, key: &str) -> bool {
    let needle = format!("{}=", key);
    cookie
        .split(';')
        .any(|part| part.trim_start().starts_with(&needle))
}

fn append_cookie(cookie: &str, key: &str, value: &str) -> String {
    let trimmed = cookie.trim();
    if trimmed.is_empty() {
        return format!("{}={}", key, value);
    }
    if trimmed.ends_with(';') {
        format!("{} {}={}", trimmed, key, value)
    } else {
        format!("{}; {}={}", trimmed, key, value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risk_control_detection_covers_known_bilibili_codes() {
        assert!(is_bilibili_risk_control_error(
            "request was banned (code: -412)"
        ));
        assert!(is_bilibili_risk_control_error("系统限流 (code: -509)"));
        assert!(is_bilibili_risk_control_error(
            "请求频率过高，请稍后再试 (code: -702)"
        ));
        assert!(!is_bilibili_risk_control_error(
            "网络繁忙 请稍后再试 (code: 69800)"
        ));
    }

    #[test]
    fn season_archive_requests_share_archive_rate_limit() {
        assert!(is_archive_request_url(
            "https://api.bilibili.com/x/polymer/web-space/seasons_archives_list"
        ));
        assert!(is_season_archive_request_url(
            "https://api.bilibili.com/x/polymer/web-space/seasons_archives_list"
        ));
    }

    #[test]
    fn archive_rate_limit_uses_bounded_exponential_cooldown() {
        let mut state = ArchiveRequestState::default();
        let now = Instant::now();
        assert_eq!(
            state.record_error("系统限流 (code: -509)", now),
            Some(Duration::from_secs(60))
        );
        assert_eq!(
            state.record_error("系统限流 (code: -509)", now),
            Some(Duration::from_secs(120))
        );
        state.consecutive_rate_limits = 20;
        assert_eq!(
            state.record_error("请求频率过高 (code: -702)", now),
            Some(Duration::from_secs(30 * 60))
        );
    }

    #[test]
    fn archive_ban_starts_long_cooldown_and_success_resets_it() {
        let mut state = ArchiveRequestState::default();
        let now = Instant::now();
        assert_eq!(
            state.record_error("request was banned (code: -412)", now),
            Some(Duration::from_secs(30 * 60))
        );
        assert!(state.active_cooldown_error(now).is_some());
        state.record_success();
        assert!(state.active_cooldown_error(now).is_none());
        assert_eq!(state.consecutive_rate_limits, 0);
    }

    #[test]
    fn archive_request_state_enforces_minimum_interval() {
        let now = Instant::now();
        let mut state = ArchiveRequestState {
            last_request_at: now.checked_sub(Duration::from_millis(500)),
            ..ArchiveRequestState::default()
        };
        assert_eq!(state.request_wait(now), Some(Duration::from_millis(1000)));

        state.last_request_at = now.checked_sub(Duration::from_secs(2));
        assert_eq!(state.request_wait(now), None);
    }
}
