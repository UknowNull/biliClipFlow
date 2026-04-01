use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, REFERER, USER_AGENT};
use reqwest::Client;
use serde_json::Value;
use std::sync::Mutex;

use crate::bilibili::signer::WbiSigner;
use crate::login_store::AuthInfo;

pub struct BilibiliClient {
    client: Client,
    base_url: String,
    passport_base_url: String,
    signer: WbiSigner,
    buvid3: Mutex<Option<String>>,
}

impl BilibiliClient {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            base_url: "https://api.bilibili.com".to_string(),
            passport_base_url: "https://passport.bilibili.com".to_string(),
            signer: WbiSigner::new(),
            buvid3: Mutex::new(None),
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

        let response = self
            .client
            .get(full_url)
            .headers(headers)
            .send()
            .await
            .map_err(|err| format!("Request failed: {}", err))?
            .text()
            .await
            .map_err(|err| format!("Failed to read response: {}", err))?;

        parse_response(&response)
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

        let response = self
            .client
            .post(full_url)
            .headers(headers)
            .json(body)
            .send()
            .await
            .map_err(|err| format!("Request failed: {}", err))?
            .text()
            .await
            .map_err(|err| format!("Failed to read response: {}", err))?;

        parse_response(&response)
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
            .map_err(|err| format!("Request failed: {}", err))?
            .text()
            .await
            .map_err(|err| format!("Failed to read response: {}", err))?;

        parse_response(&response)
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
            .map_err(|err| format!("Request failed: {}", err))?
            .text()
            .await
            .map_err(|err| format!("Failed to read response: {}", err))?;

        let data = parse_response(&response)?;
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
