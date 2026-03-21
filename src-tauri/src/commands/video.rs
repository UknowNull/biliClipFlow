use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use chrono::Utc;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE, USER_AGENT};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use tauri::State;

use crate::api::ApiResponse;
use crate::login_store::AuthInfo;
use crate::utils::append_log;
use crate::AppState;

#[derive(Serialize)]
pub struct Partition {
  pub tid: i64,
  pub name: String,
  pub type_pid: Option<i64>,
}

#[derive(Serialize)]
pub struct Collection {
  pub season_id: i64,
  pub name: String,
  pub cover: Option<String>,
  pub description: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityTopic {
  pub topic_id: i64,
  pub mission_id: i64,
  pub name: String,
  pub description: Option<String>,
  pub activity_text: Option<String>,
  pub activity_description: Option<String>,
  pub read_count: i64,
  pub show_activity_icon: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityTopicFieldSchema {
  pub field: String,
  pub value_type: String,
  pub sample: String,
  pub meaning: String,
}

fn parse_topic_counter(item: &Value, keys: &[&str]) -> i64 {
  for key in keys {
    if let Some(value) = item.get(*key) {
      if let Some(count) = value.as_i64() {
        if count > 0 {
          return count;
        }
      }
      if let Some(count) = value.as_u64() {
        if count > 0 {
          return count as i64;
        }
      }
      if let Some(raw) = value.as_str() {
        let digits: String = raw.chars().filter(|ch| ch.is_ascii_digit()).collect();
        if let Ok(parsed) = digits.parse::<i64>() {
          if parsed > 0 {
            return parsed;
          }
        }
      }
    }
  }
  0
}

fn parse_int_field(item: &Value, keys: &[&str]) -> Option<i64> {
  for key in keys {
    let Some(value) = item.get(*key) else {
      continue;
    };
    if let Some(parsed) = value.as_i64() {
      return Some(parsed);
    }
    if let Some(parsed) = value.as_u64() {
      if parsed <= i64::MAX as u64 {
        return Some(parsed as i64);
      }
    }
    if let Some(raw) = value.as_str() {
      if let Ok(parsed) = raw.trim().parse::<i64>() {
        return Some(parsed);
      }
    }
  }
  None
}

fn parse_bool_field(item: &Value, keys: &[&str]) -> bool {
  for key in keys {
    let Some(value) = item.get(*key) else {
      continue;
    };
    if let Some(parsed) = value.as_bool() {
      return parsed;
    }
    if let Some(parsed) = value.as_i64() {
      return parsed > 0;
    }
    if let Some(parsed) = value.as_u64() {
      return parsed > 0;
    }
    if let Some(raw) = value.as_str() {
      let normalized = raw.trim().to_ascii_lowercase();
      if normalized == "true" || normalized == "1" || normalized == "yes" {
        return true;
      }
      if normalized == "false" || normalized == "0" || normalized == "no" {
        return false;
      }
    }
  }
  false
}

fn value_type_name(value: &Value) -> &'static str {
  if value.is_null() {
    "null"
  } else if value.is_boolean() {
    "boolean"
  } else if value.is_number() {
    "number"
  } else if value.is_string() {
    "string"
  } else if value.is_array() {
    "array"
  } else if value.is_object() {
    "object"
  } else {
    "unknown"
  }
}

fn value_sample(value: &Value) -> String {
  let raw = if let Some(text) = value.as_str() {
    text.to_string()
  } else {
    value.to_string()
  };
  let trimmed = raw.trim();
  if trimmed.chars().count() <= 80 {
    return trimmed.to_string();
  }
  let prefix: String = trimmed.chars().take(77).collect();
  format!("{}...", prefix)
}

fn topic_field_meaning(field: &str) -> String {
  if field == "topic_id" {
    return "活动话题ID".to_string();
  }
  if field == "mission_id" {
    return "活动任务ID".to_string();
  }
  if field == "topic_name" {
    return "活动话题名称".to_string();
  }
  if field == "description" {
    return "活动描述".to_string();
  }
  if field == "activity_text" {
    return "活动文案（展示文本）".to_string();
  }
  if field == "activity_description" {
    return "活动详细说明".to_string();
  }
  if field.contains("read")
    || field.contains("view")
    || field.contains("pv")
    || field.contains("click")
    || field.contains("hot")
  {
    return "疑似阅读/浏览/热度指标字段".to_string();
  }
  if field.contains("cover") || field.contains("img") || field.contains("image") {
    return "疑似图片或封面字段".to_string();
  }
  if field.contains("start") || field.contains("end") || field.contains("time") {
    return "疑似时间相关字段".to_string();
  }
  if field.contains("status") || field.contains("state") {
    return "疑似状态字段".to_string();
  }
  "接口原始字段（需结合B站接口定义确认）".to_string()
}

async fn fetch_bilibili_topic_items(
  state: &State<'_, AppState>,
  type_id: Option<i64>,
  type_pid: Option<i64>,
  title: Option<&str>,
  auth: &AuthInfo,
) -> Result<Vec<Value>, String> {
  if let Some(title) = title {
    let trimmed = title.trim();
    if !trimmed.is_empty() {
      return fetch_bilibili_topic_search_items(state, trimmed, auth).await;
    }
  }
  let url = "https://member.bilibili.com/x/vupre/web/topic/type/v2";
  let timestamp = Utc::now().timestamp_millis();
  let mut page = 0;
  let page_size = 200;
  let mut max_page = 1;
  let mut topics = Vec::new();
  let mut seen = HashSet::new();

  loop {
    let mut params = vec![
      ("pn".to_string(), page.to_string()),
      ("ps".to_string(), page_size.to_string()),
      ("platform".to_string(), "pc".to_string()),
      ("t".to_string(), timestamp.to_string()),
    ];
    if let Some(type_id) = type_id {
      if type_id > 0 {
        params.push(("type_id".to_string(), type_id.to_string()));
      }
    }
    if let Some(type_pid) = type_pid {
      if type_pid > 0 {
        params.push(("type_pid".to_string(), type_pid.to_string()));
      }
    }
    if let Some(title) = title {
      let trimmed = title.trim();
      if !trimmed.is_empty() {
        params.push(("title".to_string(), trimmed.to_string()));
      }
    }
    let type_id_log = type_id.unwrap_or_default();
    let type_pid_log = type_pid.unwrap_or_default();
    let title_log = title.unwrap_or("").trim();
    append_log(
      &state.app_log_path,
      &format!(
        "topics_request pn={} ps={} type_id={} type_pid={} title={}",
        page, page_size, type_id_log, type_pid_log, title_log
      ),
    );

    let data = state
      .bilibili
      .get_json(url, &params, Some(auth), false)
      .await
      .map_err(|err| format!("Failed to load topics: {}", err))?;

    let next_max_page = data
      .get("maxpage")
      .and_then(|value| value.as_i64())
      .or_else(|| data.get("max_page").and_then(|value| value.as_i64()))
      .unwrap_or(page + 1);
    max_page = max_page.max(next_max_page);

    if let Some(list) = data.get("topics").and_then(|value| value.as_array()) {
      for item in list {
        let topic_id = item
          .get("topic_id")
          .and_then(|value| value.as_i64())
          .unwrap_or(0);
        if topic_id <= 0 || !seen.insert(topic_id) {
          continue;
        }
        topics.push(item.clone());
      }
    }
    append_log(
      &state.app_log_path,
      &format!(
        "topics_response pn={} fetched={} dedup_total={} max_page={}",
        page,
        data
          .get("topics")
          .and_then(|value| value.as_array())
          .map(|list| list.len())
          .unwrap_or(0),
        topics.len(),
        max_page
      ),
    );

    if page + 1 >= max_page {
      break;
    }
    page += 1;
  }

  Ok(topics)
}

async fn fetch_bilibili_topic_search_items(
  state: &State<'_, AppState>,
  keywords: &str,
  auth: &AuthInfo,
) -> Result<Vec<Value>, String> {
  let url = "https://member.bilibili.com/x/vupre/web/topic/search";
  let timestamp = Utc::now().timestamp_millis();
  let page_size = 50;
  let mut offset = 0_i64;
  let mut topics = Vec::new();
  let mut seen = HashSet::new();

  loop {
    let params = vec![
      ("keywords".to_string(), keywords.to_string()),
      ("page_size".to_string(), page_size.to_string()),
      ("offset".to_string(), offset.to_string()),
      ("t".to_string(), timestamp.to_string()),
    ];
    append_log(
      &state.app_log_path,
      &format!(
        "topics_search_request offset={} page_size={} keywords={}",
        offset, page_size, keywords
      ),
    );
    let data = state
      .bilibili
      .get_json(url, &params, Some(auth), false)
      .await
      .map_err(|err| format!("Failed to search topics: {}", err))?;
    let result = data.get("result");
    let topics_data = result
      .and_then(|value| value.get("topics"))
      .and_then(|value| value.as_array())
      .cloned()
      .unwrap_or_default();
    for item in &topics_data {
      let topic_id = item
        .get("id")
        .or_else(|| item.get("topic_id"))
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
      if topic_id <= 0 || !seen.insert(topic_id) {
        continue;
      }
      topics.push(item.clone());
    }
    let page_info = result.and_then(|value| value.get("page_info"));
    let has_more = page_info
      .and_then(|value| value.get("has_more"))
      .and_then(|value| value.as_bool())
      .unwrap_or(false);
    let next_offset = page_info
      .and_then(|value| value.get("offset"))
      .and_then(|value| value.as_i64())
      .unwrap_or(offset + page_size as i64);
    append_log(
      &state.app_log_path,
      &format!(
        "topics_search_response offset={} fetched={} dedup_total={} next_offset={} has_more={}",
        offset,
        topics_data.len(),
        topics.len(),
        next_offset,
        has_more
      ),
    );
    if !has_more || topics_data.is_empty() {
      break;
    }
    offset = next_offset;
  }

  Ok(topics)
}

#[tauri::command]
pub async fn video_detail(
  state: State<'_, AppState>,
  bvid: Option<String>,
  aid: Option<i64>,
) -> Result<ApiResponse<Value>, String> {
  if bvid.is_none() && aid.is_none() {
    return Ok(ApiResponse::error("Missing bvid or aid"));
  }

  let mut params = Vec::new();
  if let Some(bvid) = bvid {
    params.push(("bvid".to_string(), bvid));
  }
  if let Some(aid) = aid {
    params.push(("aid".to_string(), aid.to_string()));
  }

  let auth = load_auth(&state);
  let url = format!("{}/x/web-interface/view", state.bilibili.base_url());
  match state.bilibili.get_json(&url, &params, auth.as_ref(), false).await {
    Ok(data) => Ok(ApiResponse::success(data)),
    Err(err) => Ok(ApiResponse::error(format!("Failed to load video detail: {}", err))),
  }
}

#[tauri::command]
pub async fn video_playurl(
  state: State<'_, AppState>,
  bvid: String,
  cid: String,
  qn: Option<String>,
  fnval: Option<String>,
  fnver: Option<String>,
  fourk: Option<String>,
) -> Result<ApiResponse<Value>, String> {
  let params = vec![
    ("bvid".to_string(), bvid),
    ("cid".to_string(), cid),
    ("qn".to_string(), qn.unwrap_or_else(|| "112".to_string())),
    ("fnval".to_string(), fnval.unwrap_or_else(|| "4048".to_string())),
    ("fnver".to_string(), fnver.unwrap_or_else(|| "0".to_string())),
    ("fourk".to_string(), fourk.unwrap_or_else(|| "1".to_string())),
  ];

  let auth = load_auth(&state);
  let url = format!("{}/x/player/wbi/playurl", state.bilibili.base_url());
  match state.bilibili.get_json(&url, &params, auth.as_ref(), true).await {
    Ok(data) => Ok(ApiResponse::success(data)),
    Err(err) => Ok(ApiResponse::error(format!("Failed to load playurl: {}", err))),
  }
}

#[tauri::command]
pub async fn video_playurl_by_aid(
  state: State<'_, AppState>,
  aid: String,
  cid: String,
  qn: Option<String>,
  fnval: Option<String>,
  fnver: Option<String>,
  fourk: Option<String>,
) -> Result<ApiResponse<Value>, String> {
  let params = vec![
    ("avid".to_string(), aid),
    ("cid".to_string(), cid),
    ("qn".to_string(), qn.unwrap_or_else(|| "112".to_string())),
    ("fnval".to_string(), fnval.unwrap_or_else(|| "4048".to_string())),
    ("fnver".to_string(), fnver.unwrap_or_else(|| "0".to_string())),
    ("fourk".to_string(), fourk.unwrap_or_else(|| "1".to_string())),
  ];

  let auth = load_auth(&state);
  let url = format!("{}/x/player/wbi/playurl", state.bilibili.base_url());
  match state.bilibili.get_json(&url, &params, auth.as_ref(), true).await {
    Ok(data) => Ok(ApiResponse::success(data)),
    Err(err) => Ok(ApiResponse::error(format!("Failed to load playurl: {}", err))),
  }
}

#[tauri::command]
pub async fn video_proxy_image(url: String) -> Result<ApiResponse<String>, String> {
  let trimmed = url.trim();
  if trimmed.is_empty() {
    return Ok(ApiResponse::error("图片地址不能为空"));
  }

  let mut headers = HeaderMap::new();
  headers.insert(
    USER_AGENT,
    HeaderValue::from_static("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36"),
  );
  headers.insert(
    "Referer",
    HeaderValue::from_static("https://www.bilibili.com"),
  );

  let client = reqwest::Client::new();
  let response = match client.get(trimmed).headers(headers).send().await {
    Ok(response) => response,
    Err(err) => {
      return Ok(ApiResponse::error(format!("获取图片失败: {}", err)));
    }
  };

  if !response.status().is_success() {
    return Ok(ApiResponse::error(format!(
      "获取图片失败: {}",
      response.status()
    )));
  }

  let content_type = response
    .headers()
    .get(CONTENT_TYPE)
    .and_then(|value| value.to_str().ok())
    .unwrap_or("image/jpeg")
    .to_string();

  let bytes = match response.bytes().await {
    Ok(bytes) => bytes,
    Err(err) => {
      return Ok(ApiResponse::error(format!("读取图片失败: {}", err)));
    }
  };

  let encoded = STANDARD.encode(bytes);
  let data_url = format!("data:{};base64,{}", content_type, encoded);
  Ok(ApiResponse::success(data_url))
}

#[tauri::command]
pub async fn bilibili_collections(
  state: State<'_, AppState>,
  mid: i64,
  bilibili_uid: Option<i64>,
) -> Result<ApiResponse<Vec<Collection>>, String> {
  let auth = load_auth_for_uid(&state, bilibili_uid);
  append_log(
    &state.app_log_path,
    &format!("collections_start mid={} has_auth={}", mid, auth.is_some()),
  );
  if auth.is_none() {
    append_log(&state.app_log_path, &format!("collections_no_auth mid={}", mid));
    return Ok(ApiResponse::error("Login required"));
  }

  let params = vec![
    ("pn".to_string(), "1".to_string()),
    ("ps".to_string(), "100".to_string()),
    ("order".to_string(), "desc".to_string()),
    ("sort".to_string(), "mtime".to_string()),
    ("filter".to_string(), "1".to_string()),
  ];

  let url = "https://member.bilibili.com/x2/creative/web/seasons";
  let data = match state
    .bilibili
    .get_json(url, &params, auth.as_ref(), false)
    .await
  {
    Ok(data) => data,
    Err(err) => {
      append_log(
        &state.app_log_path,
        &format!("collections_api_error mid={} err={}", mid, err),
      );
      return Ok(ApiResponse::error(format!("Failed to load collections: {}", err)));
    }
  };

  let seasons = data.get("seasons").and_then(|value| value.as_array());
  let mut collections = Vec::new();
  if let Some(seasons) = seasons {
    for item in seasons {
      if let Some(season) = item.get("season") {
        if let Some(id) = season.get("id").and_then(|value| value.as_i64()) {
          collections.push(Collection {
            season_id: id,
            name: season
              .get("title")
              .and_then(|value| value.as_str())
              .unwrap_or_default()
              .to_string(),
            cover: season.get("cover").and_then(|value| value.as_str()).map(|value| value.to_string()),
            description: season
              .get("desc")
              .and_then(|value| value.as_str())
              .map(|value| value.to_string()),
          });
        }
      }
    }
  }
  append_log(
    &state.app_log_path,
    &format!("collections_ok mid={} count={}", mid, collections.len()),
  );

  Ok(ApiResponse::success(collections))
}

#[tauri::command]
pub async fn bilibili_partitions(
  state: State<'_, AppState>,
  bilibili_uid: Option<i64>,
) -> Result<ApiResponse<Vec<Partition>>, String> {
  let auth = load_auth_for_uid(&state, bilibili_uid);
  append_log(
    &state.app_log_path,
    &format!("partitions_fetch_start has_auth={}", auth.is_some()),
  );
  let params = vec![("t".to_string(), format!("{}", Utc::now().timestamp_millis()))];
  let url = "https://member.bilibili.com/x/vupre/web/archive/human/type2/list";

  let data = match state
    .bilibili
    .get_json(url, &params, auth.as_ref(), false)
    .await
  {
    Ok(data) => data,
    Err(err) => {
      let defaults = default_partitions();
      append_log(
        &state.app_log_path,
        &format!(
          "partitions_fetch_fail err={} fallback=default count={}",
          err,
          defaults.len()
        ),
      );
      return Ok(ApiResponse::success(defaults));
    }
  };
  append_log(&state.app_log_path, "partitions_fetch_ok");

  let list = data.get("type_list").and_then(|value| value.as_array());
  if list.is_none() {
    let data_kind = if data.is_object() {
      "object"
    } else if data.is_array() {
      "array"
    } else if data.is_string() {
      "string"
    } else if data.is_number() {
      "number"
    } else if data.is_boolean() {
      "boolean"
    } else if data.is_null() {
      "null"
    } else {
      "unknown"
    };
    let key_sample = data
      .as_object()
      .map(|obj| obj.keys().take(12).cloned().collect::<Vec<_>>().join(","))
      .unwrap_or_default();
    append_log(
      &state.app_log_path,
      &format!(
        "partitions_type_list_missing kind={} key_sample={}",
        data_kind, key_sample
      ),
    );
  }

  let mut partitions = Vec::new();
  let mut total_items = 0usize;
  let mut skipped_no_name = 0usize;
  let mut skipped_no_id = 0usize;
  let mut skipped_invalid_id = 0usize;
  if let Some(list) = list {
    for item in list {
      total_items += 1;
      let Some(name) = item.get("name").and_then(|value| value.as_str()) else {
        skipped_no_name += 1;
        continue;
      };
      let raw_id = parse_int_field(item, &["id", "ID"]);
      let raw_pid = parse_int_field(item, &["pid", "parent_id", "parentId", "parentID"]);
      let explicit_type_id = parse_int_field(item, &["type_id", "typeId", "tid"]);
      let explicit_type_pid = parse_int_field(item, &["type_pid", "typePid"]);

      let type_id = explicit_type_id.or(raw_id);
      let Some(type_id) = type_id else {
        skipped_no_id += 1;
        continue;
      };
      let type_pid = explicit_type_pid.or(raw_pid);

      if type_id <= 0 {
        skipped_invalid_id += 1;
        continue;
      }

      partitions.push(Partition {
        tid: type_id,
        name: name.to_string(),
        type_pid: type_pid.filter(|value| *value > 0),
      });
    }
  }

  if partitions.is_empty() {
    let defaults = default_partitions();
    append_log(
      &state.app_log_path,
      &format!(
        "partitions_parse_empty total={} no_name={} no_id={} invalid_id={} fallback=default count={}",
        total_items,
        skipped_no_name,
        skipped_no_id,
        skipped_invalid_id,
        defaults.len()
      ),
    );
    Ok(ApiResponse::success(defaults))
  } else {
    let sample = partitions
      .iter()
      .take(8)
      .map(|item| format!("{}:{}", item.tid, item.name))
      .collect::<Vec<_>>()
      .join("|");
    append_log(
      &state.app_log_path,
      &format!(
        "partitions_parse_ok total={} parsed={} no_name={} no_id={} invalid_id={} sample={}",
        total_items,
        partitions.len(),
        skipped_no_name,
        skipped_no_id,
        skipped_invalid_id,
        sample
      ),
    );
    Ok(ApiResponse::success(partitions))
  }
}

#[tauri::command]
pub async fn bilibili_topics(
  state: State<'_, AppState>,
  partition_id: Option<i64>,
  title: Option<String>,
  bilibili_uid: Option<i64>,
) -> Result<ApiResponse<Vec<ActivityTopic>>, String> {
  let auth = load_auth_for_uid(&state, bilibili_uid);
  let Some(auth_info) = auth else {
    return Ok(ApiResponse::error("Login required"));
  };
  let selected_partition_id = partition_id.filter(|value| *value > 0);
  let query_type_pid = selected_partition_id;
  let query_type_id = if selected_partition_id.is_some() {
    Some(21)
  } else {
    None
  };

  let raw_items = match fetch_bilibili_topic_items(
    &state,
    query_type_id,
    query_type_pid,
    title.as_deref(),
    &auth_info,
  )
  .await
  {
    Ok(items) => items,
    Err(err) => return Ok(ApiResponse::error(err)),
  };
  let mut topics = Vec::new();
  for item in &raw_items {
    let topic_id = item
      .get("topic_id")
      .or_else(|| item.get("id"))
      .and_then(|value| value.as_i64())
      .unwrap_or(0);
    let mission_id = item
      .get("mission_id")
      .and_then(|value| value.as_i64())
      .unwrap_or(0);
    let name = item
      .get("topic_name")
      .or_else(|| item.get("name"))
      .and_then(|value| value.as_str())
      .unwrap_or_default()
      .to_string();
    let description = item
      .get("description")
      .or_else(|| item.get("topic_description"))
      .and_then(|value| value.as_str())
      .map(|value| value.to_string());
    let activity_text = item
      .get("activity_text")
      .or_else(|| item.get("activity_sign"))
      .and_then(|value| value.as_str())
      .map(|value| value.to_string());
    let activity_description = item
      .get("activity_description")
      .or_else(|| item.get("act_protocol"))
      .and_then(|value| value.as_str())
      .map(|value| value.to_string());
    let read_count = parse_topic_counter(
      item,
      &[
        "read_count",
        "readCount",
        "arc_play_vv",
        "arcPlayVv",
        "read",
        "view_count",
        "viewCount",
        "view",
        "pv",
        "click",
        "hot",
      ],
    );
    let show_activity_icon = parse_bool_field(item, &["show_activity_icon", "showActivityIcon"]);

    topics.push(ActivityTopic {
      topic_id,
      mission_id,
      name,
      description,
      activity_text,
      activity_description,
      read_count,
      show_activity_icon,
    });
  }

  Ok(ApiResponse::success(topics))
}

#[tauri::command]
pub async fn bilibili_topics_field_schema(
  state: State<'_, AppState>,
  partition_id: Option<i64>,
  title: Option<String>,
  bilibili_uid: Option<i64>,
) -> Result<ApiResponse<Vec<ActivityTopicFieldSchema>>, String> {
  let auth = load_auth_for_uid(&state, bilibili_uid);
  let Some(auth_info) = auth else {
    return Ok(ApiResponse::error("Login required"));
  };
  let selected_partition_id = partition_id.filter(|value| *value > 0);
  let query_type_pid = selected_partition_id;
  let query_type_id = if selected_partition_id.is_some() {
    Some(21)
  } else {
    None
  };

  let raw_items = match fetch_bilibili_topic_items(
    &state,
    query_type_id,
    query_type_pid,
    title.as_deref(),
    &auth_info,
  )
  .await
  {
    Ok(items) => items,
    Err(err) => return Ok(ApiResponse::error(err)),
  };
  let mut schema: BTreeMap<String, (String, Vec<String>)> = BTreeMap::new();
  for item in &raw_items {
    let Some(map) = item.as_object() else {
      continue;
    };
    for (field, value) in map {
      let entry = schema
        .entry(field.clone())
        .or_insert_with(|| (value_type_name(value).to_string(), Vec::new()));
      if entry.1.len() >= 3 {
        continue;
      }
      let sample = value_sample(value);
      if sample.is_empty() || entry.1.iter().any(|existing| existing == &sample) {
        continue;
      }
      entry.1.push(sample);
    }
  }
  let result = schema
    .into_iter()
    .map(|(field, (value_type, samples))| ActivityTopicFieldSchema {
      meaning: topic_field_meaning(&field),
      sample: samples.join(" | "),
      field,
      value_type,
    })
    .collect::<Vec<_>>();
  Ok(ApiResponse::success(result))
}

fn default_partitions() -> Vec<Partition> {
  vec![
    Partition {
      tid: 1,
      name: "Animation".to_string(),
      type_pid: None,
    },
    Partition {
      tid: 4,
      name: "Game".to_string(),
      type_pid: None,
    },
    Partition {
      tid: 36,
      name: "Knowledge".to_string(),
      type_pid: None,
    },
    Partition {
      tid: 188,
      name: "Technology".to_string(),
      type_pid: None,
    },
  ]
}

fn load_auth(state: &State<'_, AppState>) -> Option<AuthInfo> {
  state.login_store.load_primary_auth_info(&state.db).ok().flatten()
}

fn load_auth_for_uid(state: &State<'_, AppState>, bilibili_uid: Option<i64>) -> Option<AuthInfo> {
  if let Some(user_id) = bilibili_uid.filter(|value| *value > 0) {
    return state
      .login_store
      .load_auth_info_by_uid(&state.db, user_id)
      .ok()
      .flatten();
  }
  load_auth(state)
}
