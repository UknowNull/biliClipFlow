use std::sync::Mutex;

use chrono::{Duration, Utc};
use md5::compute;
use reqwest::Client;
use serde_json::Value;

const MIXIN_KEY_ENC_TAB: [usize; 64] = [
    46, 47, 18, 2, 53, 8, 23, 32, 15, 50, 10, 31, 58, 3, 45, 35, 27, 43, 5, 49, 33, 9, 42, 19, 29,
    28, 14, 39, 12, 38, 41, 13, 37, 48, 7, 16, 24, 55, 40, 61, 26, 17, 0, 1, 60, 51, 30, 4, 22, 25,
    54, 21, 56, 59, 6, 63, 57, 62, 11, 36, 20, 34, 44, 52,
];

pub struct WbiSigner {
    img_key: Mutex<String>,
    sub_key: Mutex<String>,
    last_update: Mutex<i64>,
}

impl WbiSigner {
    pub fn new() -> Self {
        Self {
            img_key: Mutex::new(String::new()),
            sub_key: Mutex::new(String::new()),
            last_update: Mutex::new(0),
        }
    }

    pub async fn sign_params(
        &self,
        client: &Client,
        params: &[(String, String)],
    ) -> Result<String, String> {
        self.ensure_keys(client).await?;

        let mut signed_params: Vec<(String, String)> = params.to_vec();
        signed_params.push(("wts".to_string(), (Utc::now().timestamp()).to_string()));
        signed_params.push(("dm_img_list".to_string(), "[]".to_string()));
        signed_params.push(("dm_img_str".to_string(), "".to_string()));
        signed_params.push(("dm_cover_img_str".to_string(), "".to_string()));

        for item in &mut signed_params {
            let value = item.1.replace(['!', '\'', '(', ')', '*'], "");
            item.1 = value;
        }

        signed_params.sort_by(|a, b| a.0.cmp(&b.0));

        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for (key, value) in &signed_params {
            serializer.append_pair(key, value);
        }
        let query = serializer.finish();

        let mixin_key = self.mixin_key();
        let sign = md5_hex(&format!("{}{}", query, mixin_key));

        Ok(format!("{}&w_rid={}", query, sign))
    }

    async fn ensure_keys(&self, client: &Client) -> Result<(), String> {
        let now = Utc::now().timestamp();
        let (should_update, has_keys) = {
            let last_update = self
                .last_update
                .lock()
                .map_err(|_| "Failed to lock WBI keys".to_string())?;
            let img_key = self
                .img_key
                .lock()
                .map_err(|_| "Failed to lock WBI keys".to_string())?;
            let expired = now - *last_update >= Duration::hours(4).num_seconds();
            let has_keys = !img_key.is_empty();
            (expired, has_keys)
        };

        if !should_update {
            return Ok(());
        }

        let fetch_result: Result<(String, String), String> = async {
            let response = client
                .get("https://api.bilibili.com/x/web-interface/nav")
                .send()
                .await
                .map_err(|err| format!("Failed to fetch WBI keys: {}", err))?
                .text()
                .await
                .map_err(|err| format!("Failed to read WBI response: {}", err))?;
            let value: Value = serde_json::from_str(&response)
                .map_err(|err| format!("Failed to parse WBI response: {}", err))?;
            let img_url = value
                .get("data")
                .and_then(|data| data.get("wbi_img"))
                .and_then(|data| data.get("img_url"))
                .and_then(|data| data.as_str())
                .ok_or_else(|| "WBI response missing img_url".to_string())?;
            let sub_url = value
                .get("data")
                .and_then(|data| data.get("wbi_img"))
                .and_then(|data| data.get("sub_url"))
                .and_then(|data| data.as_str())
                .ok_or_else(|| "WBI response missing sub_url".to_string())?;
            Ok((extract_key_from_url(img_url), extract_key_from_url(sub_url)))
        }
        .await;

        match fetch_result {
            Ok((img_key, sub_key)) => {
                let mut img_key_lock = self
                    .img_key
                    .lock()
                    .map_err(|_| "Failed to lock WBI keys".to_string())?;
                let mut sub_key_lock = self
                    .sub_key
                    .lock()
                    .map_err(|_| "Failed to lock WBI keys".to_string())?;
                let mut last_update = self
                    .last_update
                    .lock()
                    .map_err(|_| "Failed to lock WBI keys".to_string())?;
                *img_key_lock = img_key;
                *sub_key_lock = sub_key;
                *last_update = now;
                Ok(())
            }
            Err(err) => {
                if has_keys {
                    // 已有缓存 key，沿用旧 key 继续，不中断录制
                    Ok(())
                } else {
                    Err(err)
                }
            }
        }
    }

    fn mixin_key(&self) -> String {
        let img_key = self
            .img_key
            .lock()
            .ok()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        let sub_key = self
            .sub_key
            .lock()
            .ok()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        let mixin_key = format!("{}{}", img_key, sub_key);

        let mut result = String::new();
        for index in MIXIN_KEY_ENC_TAB {
            if let Some(ch) = mixin_key.chars().nth(index) {
                result.push(ch);
            }
        }

        result.chars().take(32).collect()
    }
}

fn extract_key_from_url(url: &str) -> String {
    url.split('/')
        .last()
        .and_then(|segment| segment.split('.').next())
        .unwrap_or_default()
        .to_string()
}

fn md5_hex(input: &str) -> String {
    format!("{:x}", compute(input))
}
