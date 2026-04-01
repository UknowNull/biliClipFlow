use std::cmp::Ordering;
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, USER_AGENT};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::api::ApiResponse;

const UPDATE_SOURCE: &str = "github_releases_latest";
const DEFAULT_UPDATE_REPO: &str = "UknowNull/biliClipFlow";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppVersionInfo {
    pub version: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUpdateCheckResult {
    pub current_version: String,
    pub latest_version: String,
    pub has_update: bool,
    pub release_url: String,
    pub download_url: Option<String>,
    pub release_notes: Option<String>,
    pub published_at: Option<String>,
    pub source: String,
}

#[derive(Deserialize)]
struct GithubReleaseAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: Option<String>,
    body: Option<String>,
    published_at: Option<String>,
    assets: Option<Vec<GithubReleaseAsset>>,
}

#[derive(Deserialize)]
struct GithubApiError {
    message: Option<String>,
}

fn update_repo() -> &'static str {
    option_env!("REACTION_CUT_UPDATE_REPO").unwrap_or(DEFAULT_UPDATE_REPO)
}

#[tauri::command]
pub fn app_version() -> ApiResponse<AppVersionInfo> {
    ApiResponse::success(AppVersionInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

#[tauri::command]
pub async fn app_update_check() -> ApiResponse<AppUpdateCheckResult> {
    let current_version = env!("CARGO_PKG_VERSION").to_string();
    match fetch_latest_release().await {
        Ok(release) => {
            let latest_version = normalize_version_tag(&release.tag_name);
            let has_update = compare_versions(&current_version, &latest_version) == Ordering::Less;
            let repo = update_repo();
            let release_url = release
                .html_url
                .unwrap_or_else(|| format!("https://github.com/{}/releases", repo));
            let download_url = release
                .assets
                .as_deref()
                .and_then(select_download_asset)
                .map(|asset| asset.browser_download_url.clone());
            let release_notes = release.body.and_then(|body| {
                let notes = body.trim().to_string();
                if notes.is_empty() {
                    None
                } else {
                    Some(notes)
                }
            });
            ApiResponse::success(AppUpdateCheckResult {
                current_version,
                latest_version,
                has_update,
                release_url,
                download_url,
                release_notes,
                published_at: release.published_at,
                source: UPDATE_SOURCE.to_string(),
            })
        }
        Err(err) => ApiResponse::error(format!("检查更新失败: {}", err)),
    }
}

async fn fetch_latest_release() -> Result<GithubRelease, String> {
    let repo = update_repo();
    let mut headers = HeaderMap::new();
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static("biliClipFlow-update-check"),
    );
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/vnd.github+json"),
    );
    let client = reqwest::Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|err| err.to_string())?;
    let url = format!("https://api.github.com/repos/{}/releases/latest", repo);
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|err| err.to_string())?;
    let status = response.status();
    if status.is_success() {
        return response
            .json::<GithubRelease>()
            .await
            .map_err(|err| err.to_string());
    }

    if status == StatusCode::NOT_FOUND {
        return fetch_latest_release_from_list(&client, repo).await;
    }

    let body = response.text().await.unwrap_or_default();
    Err(format_http_error(status, &body))
}

async fn fetch_latest_release_from_list(
    client: &reqwest::Client,
    repo: &str,
) -> Result<GithubRelease, String> {
    let url = format!("https://api.github.com/repos/{}/releases?per_page=1", repo);
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|err| err.to_string())?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format_http_error(status, &body));
    }
    let releases = response
        .json::<Vec<GithubRelease>>()
        .await
        .map_err(|err| err.to_string())?;
    releases
        .into_iter()
        .next()
        .ok_or_else(|| format!("仓库 {} 暂无可用发布版本（Release）", repo))
}

fn format_http_error(status: StatusCode, body: &str) -> String {
    let api_error = serde_json::from_str::<GithubApiError>(body)
        .ok()
        .and_then(|item| item.message)
        .unwrap_or_default();
    if api_error.is_empty() {
        format!("HTTP {}", status)
    } else {
        format!("HTTP {}: {}", status, api_error)
    }
}

fn normalize_version_tag(tag: &str) -> String {
    let value = tag.trim();
    value
        .strip_prefix('v')
        .or_else(|| value.strip_prefix('V'))
        .unwrap_or(value)
        .trim()
        .to_string()
}

fn compare_versions(current: &str, latest: &str) -> Ordering {
    let mut current_parts = parse_version_parts(current);
    let mut latest_parts = parse_version_parts(latest);
    let max_len = current_parts.len().max(latest_parts.len());
    current_parts.resize(max_len, 0);
    latest_parts.resize(max_len, 0);
    for (left, right) in current_parts.iter().zip(latest_parts.iter()) {
        match left.cmp(right) {
            Ordering::Equal => {}
            order => return order,
        }
    }
    Ordering::Equal
}

fn parse_version_parts(value: &str) -> Vec<u64> {
    let core = value.split(['-', '+']).next().unwrap_or(value).trim();
    core.split('.')
        .map(|part| part.trim().parse::<u64>().unwrap_or(0))
        .collect()
}

fn select_download_asset(assets: &[GithubReleaseAsset]) -> Option<&GithubReleaseAsset> {
    if assets.is_empty() {
        return None;
    }
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let candidates: &[&str] = match (os, arch) {
        ("windows", "x86_64") => &["x64-setup.exe", "_x64-setup.exe", ".exe"],
        ("windows", "aarch64") => &["arm64-setup.exe", "aarch64-setup.exe", ".exe"],
        ("macos", "aarch64") => &[
            "aarch64.app.tar.gz",
            "arm64.app.tar.gz",
            "aarch64.dmg",
            "arm64.dmg",
            ".dmg",
            ".app.tar.gz",
            ".pkg",
        ],
        ("macos", "x86_64") => &[
            "x64.app.tar.gz",
            "x86_64.app.tar.gz",
            ".dmg",
            ".app.tar.gz",
            ".pkg",
        ],
        _ => &[],
    };
    for candidate in candidates {
        if let Some(asset) = assets
            .iter()
            .find(|item| item.name.to_lowercase().contains(&candidate.to_lowercase()))
        {
            return Some(asset);
        }
    }
    assets.first()
}
