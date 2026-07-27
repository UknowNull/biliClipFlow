use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{Manager, State};
use uuid::Uuid;
use zip::write::FileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::api::ApiResponse;
use crate::utils::sanitize_filename;
use crate::AppState;

const PROJECTS_DIR: &str = "video-mask/projects";
const SUPPORTED_IMAGE_EXTENSIONS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];
const PACKAGE_FORMAT: &str = "bili-clip-flow-video-mask-project";
const PACKAGE_VERSION: u32 = 1;
const PACKAGE_MANIFEST_JSON: &str = "manifest.json";
const PACKAGE_PROJECT_JSON: &str = "project.json";
const MAX_PACKAGE_JSON_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMaskProject {
    id: String,
    name: String,
    source_path: Option<String>,
    duration: f64,
    editor_state: Value,
    revision: i64,
    created_at: String,
    updated_at: String,
    last_opened_at: Option<String>,
    source_exists: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMaskProjectSummary {
    id: String,
    name: String,
    source_path: Option<String>,
    duration: f64,
    revision: i64,
    created_at: String,
    updated_at: String,
    last_opened_at: Option<String>,
    source_exists: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMaskProjectCreatePayload {
    name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMaskProjectIdPayload {
    project_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMaskProjectSavePayload {
    project_id: String,
    revision: i64,
    editor_state: Value,
    source_path: Option<String>,
    duration: Option<f64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMaskProjectRenamePayload {
    project_id: String,
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMaskProjectAssetImportPayload {
    project_id: String,
    paths: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMaskProjectExportPayload {
    project_id: String,
    target_path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMaskProjectImportPayload {
    archive_path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMaskProjectSaveResult {
    revision: i64,
    updated_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMaskProjectExportResult {
    path: String,
    source_included: bool,
    image_count: usize,
    output_included: bool,
    exported_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMaskProjectAsset {
    id: String,
    original_name: String,
    managed_path: String,
    size_bytes: u64,
}

#[derive(Clone)]
struct ImagePackageEntry {
    original_path: String,
    package_path: String,
    original_name: String,
    resource_id: Option<String>,
    segment_ids: Vec<String>,
    size_bytes: u64,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct VideoMaskPackageProjectMeta {
    original_project_id: String,
    name: String,
    created_at: String,
    updated_at: String,
    revision: i64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VideoMaskPackageAsset {
    role: String,
    path: String,
    original_name: String,
    size_bytes: u64,
    resource_id: Option<String>,
    segment_ids: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VideoMaskPackageManifest {
    format: String,
    version: u32,
    exported_at: String,
    project: VideoMaskPackageProjectMeta,
    assets: Vec<VideoMaskPackageAsset>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VideoMaskPackageProjectData {
    format: String,
    version: u32,
    project: VideoMaskPackageProjectMeta,
    editor_state: Value,
}

type VideoMaskProjectRow = (
    String,
    String,
    Option<String>,
    f64,
    String,
    i64,
    String,
    String,
    Option<String>,
);

fn validated_project_id(raw: &str) -> Result<String, String> {
    Uuid::parse_str(raw.trim())
        .map(|id| id.to_string())
        .map_err(|_| "项目ID无效".to_string())
}

fn normalized_project_name(raw: Option<&str>) -> String {
    let name = raw.unwrap_or_default().trim();
    if name.is_empty() {
        format!("未命名项目 {}", Utc::now().format("%Y-%m-%d %H:%M"))
    } else {
        name.chars().take(80).collect()
    }
}

fn project_root(app: &tauri::AppHandle, project_id: &str) -> Result<PathBuf, String> {
    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|err| format!("解析应用数据目录失败: {}", err))?;
    Ok(app_data.join(PROJECTS_DIR).join(project_id))
}

fn project_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<VideoMaskProjectRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
    ))
}

fn parse_project(raw: VideoMaskProjectRow) -> VideoMaskProject {
    let source_exists = raw
        .2
        .as_deref()
        .map(Path::new)
        .map(Path::is_file)
        .unwrap_or(false);
    VideoMaskProject {
        id: raw.0,
        name: raw.1,
        source_path: raw.2,
        duration: raw.3,
        editor_state: serde_json::from_str(&raw.4).unwrap_or_else(|_| json!({})),
        revision: raw.5,
        created_at: raw.6,
        updated_at: raw.7,
        last_opened_at: raw.8,
        source_exists,
    }
}

fn load_project_row(state: &AppState, id: &str) -> Result<Option<VideoMaskProjectRow>, String> {
    state
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT id, name, source_path, duration, editor_state_json, revision, created_at, updated_at, last_opened_at \
                 FROM video_mask_project WHERE id = ?1",
                [&id],
                project_from_row,
            )
            .optional()
        })
        .map_err(|err| format!("读取视频遮罩项目失败: {}", err))
}

fn value_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
}

fn set_value_string(value: &mut Value, key: &str, next: String) {
    if let Some(object) = value.as_object_mut() {
        object.insert(key.to_string(), Value::String(next));
    }
}

fn remove_value_field(value: &mut Value, key: &str) {
    if let Some(object) = value.as_object_mut() {
        object.remove(key);
    }
}

fn normalized_path_key(raw: &str) -> String {
    raw.trim().replace('\\', "/")
}

fn file_name_from_any_path(raw: &str, fallback: &str) -> String {
    let normalized = normalized_path_key(raw);
    let name = normalized
        .rsplit('/')
        .find(|item| !item.trim().is_empty())
        .unwrap_or(fallback);
    let sanitized = sanitize_filename(name.trim());
    if sanitized.trim().is_empty() {
        fallback.to_string()
    } else {
        sanitized
    }
}

fn extension_from_path(path: &Path, fallback: &str) -> String {
    path.extension()
        .and_then(|value| value.to_str())
        .map(|value| value.trim().trim_start_matches('.').to_ascii_lowercase())
        .filter(|value| !value.is_empty() && value.chars().all(|ch| ch.is_ascii_alphanumeric()))
        .unwrap_or_else(|| fallback.to_string())
}

fn resource_metadata_by_path(editor_state: &Value) -> BTreeMap<String, (Option<String>, String)> {
    let mut resources = BTreeMap::new();
    if let Some(items) = editor_state.get("resources").and_then(Value::as_array) {
        for item in items {
            let Some(path) =
                value_string(item, "path").or_else(|| value_string(item, "previewPath"))
            else {
                continue;
            };
            let id = value_string(item, "id");
            let name = value_string(item, "name")
                .unwrap_or_else(|| file_name_from_any_path(&path, "遮罩图片"));
            resources.insert(normalized_path_key(&path), (id, name));
        }
    }
    resources
}

fn collect_used_image_entries(editor_state: &Value) -> Result<Vec<ImagePackageEntry>, String> {
    let resource_meta = resource_metadata_by_path(editor_state);
    let mut entries = Vec::<ImagePackageEntry>::new();
    let mut index_by_key = BTreeMap::<String, usize>::new();
    let Some(segments) = editor_state.get("segments").and_then(Value::as_array) else {
        return Ok(entries);
    };

    for segment in segments {
        let Some(path) = value_string(segment, "imagePath")
            .or_else(|| value_string(segment, "imagePreviewPath"))
        else {
            continue;
        };
        let key = normalized_path_key(&path);
        let segment_id = value_string(segment, "id").unwrap_or_default();
        if let Some(index) = index_by_key.get(&key).copied() {
            if !segment_id.is_empty() {
                entries[index].segment_ids.push(segment_id);
            }
            continue;
        }

        let image_path = PathBuf::from(path.trim());
        if !image_path.is_file() {
            return Err(format!("时间轴遮罩图片不存在: {}", path));
        }
        if !supported_image(&image_path) {
            return Err(format!("时间轴遮罩图片格式不支持: {}", path));
        }
        let extension = extension_from_path(&image_path, "png");
        let package_path = format!("assets/masks/mask-{:03}.{}", entries.len() + 1, extension);
        let size_bytes = fs::metadata(&image_path)
            .map(|meta| meta.len())
            .map_err(|err| format!("读取遮罩图片信息失败 ({}): {}", path, err))?;
        let (resource_id, original_name) = resource_meta
            .get(&key)
            .cloned()
            .unwrap_or_else(|| (None, file_name_from_any_path(&path, "遮罩图片")));
        index_by_key.insert(key, entries.len());
        entries.push(ImagePackageEntry {
            original_path: path,
            package_path,
            original_name,
            resource_id,
            segment_ids: if segment_id.is_empty() {
                Vec::new()
            } else {
                vec![segment_id]
            },
            size_bytes,
        });
    }
    Ok(entries)
}

fn rewrite_editor_state_for_export(
    mut editor_state: Value,
    source_path: &str,
    target_path: &str,
    image_entries: &[ImagePackageEntry],
) -> Value {
    if !editor_state.is_object() {
        editor_state = json!({});
    }
    set_value_string(&mut editor_state, "sourcePath", source_path.to_string());
    set_value_string(&mut editor_state, "targetPath", target_path.to_string());

    let image_map = image_entries
        .iter()
        .map(|entry| {
            (
                normalized_path_key(&entry.original_path),
                entry.package_path.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();

    if let Some(resources) = editor_state
        .get_mut("resources")
        .and_then(Value::as_array_mut)
    {
        let mut represented = BTreeSet::<String>::new();
        let mut filtered = Vec::new();
        for item in resources.drain(..) {
            let mut resource = item;
            let path =
                value_string(&resource, "path").or_else(|| value_string(&resource, "previewPath"));
            let Some(path) = path else {
                continue;
            };
            let Some(package_path) = image_map.get(&normalized_path_key(&path)).cloned() else {
                continue;
            };
            set_value_string(&mut resource, "path", package_path.clone());
            set_value_string(&mut resource, "previewPath", package_path.clone());
            remove_value_field(&mut resource, "imageSrc");
            represented.insert(package_path);
            filtered.push(resource);
        }
        for (index, entry) in image_entries.iter().enumerate() {
            if represented.contains(&entry.package_path) {
                continue;
            }
            filtered.push(json!({
                "id": entry.resource_id.clone().unwrap_or_else(|| format!("resource-packaged-{}", index + 1)),
                "path": entry.package_path.clone(),
                "previewPath": entry.package_path.clone(),
                "name": entry.original_name.clone(),
                "previewError": ""
            }));
        }
        *resources = filtered;
    }
    if editor_state
        .get("resources")
        .and_then(Value::as_array)
        .is_none()
        && !image_entries.is_empty()
    {
        if let Some(object) = editor_state.as_object_mut() {
            object.insert(
                "resources".to_string(),
                Value::Array(
                    image_entries
                        .iter()
                        .enumerate()
                        .map(|(index, entry)| {
                            json!({
                                "id": entry.resource_id.clone().unwrap_or_else(|| format!("resource-packaged-{}", index + 1)),
                                "path": entry.package_path.clone(),
                                "previewPath": entry.package_path.clone(),
                                "name": entry.original_name.clone(),
                                "previewError": ""
                            })
                        })
                        .collect(),
                ),
            );
        }
    }

    if let Some(segments) = editor_state
        .get_mut("segments")
        .and_then(Value::as_array_mut)
    {
        for segment in segments {
            let path = value_string(segment, "imagePath")
                .or_else(|| value_string(segment, "imagePreviewPath"));
            if let Some(package_path) =
                path.and_then(|item| image_map.get(&normalized_path_key(&item)).cloned())
            {
                set_value_string(segment, "imagePath", package_path.clone());
                set_value_string(segment, "imagePreviewPath", package_path);
            }
            remove_value_field(segment, "imageSrc");
        }
    }

    editor_state
}

fn validate_package_path(path: &str) -> Result<(), String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("项目包内路径不能为空".to_string());
    }
    if trimmed.contains('\\') {
        return Err(format!("项目包内路径必须使用 / 分隔: {}", trimmed));
    }
    if trimmed.contains(':') {
        return Err(format!("项目包内路径不能包含盘符或协议: {}", trimmed));
    }
    let candidate = Path::new(trimmed);
    if candidate.is_absolute() {
        return Err(format!("项目包内路径不能是绝对路径: {}", trimmed));
    }
    for component in candidate.components() {
        match component {
            Component::Normal(part) if !part.to_string_lossy().trim().is_empty() => {}
            _ => return Err(format!("项目包内路径不安全: {}", trimmed)),
        }
    }
    Ok(())
}

fn project_path_from_package(root: &Path, package_path: &str) -> Result<PathBuf, String> {
    validate_package_path(package_path)?;
    Ok(root.join(package_path))
}

fn absolutize_package_path(
    root: &Path,
    package_path: &str,
    must_exist: bool,
) -> Result<String, String> {
    let path = project_path_from_package(root, package_path)?;
    if must_exist && !path.is_file() {
        return Err(format!("项目包缺少文件: {}", package_path));
    }
    Ok(path.to_string_lossy().to_string())
}

fn rewrite_package_path_field(
    value: &mut Value,
    key: &str,
    root: &Path,
    must_exist: bool,
) -> Result<(), String> {
    let Some(package_path) = value_string(value, key) else {
        return Ok(());
    };
    let absolute = absolutize_package_path(root, &package_path, must_exist)?;
    set_value_string(value, key, absolute);
    Ok(())
}

fn rewrite_editor_state_for_import(mut editor_state: Value, root: &Path) -> Result<Value, String> {
    if !editor_state.is_object() {
        return Err("项目配置格式无效".to_string());
    }
    rewrite_package_path_field(&mut editor_state, "sourcePath", root, true)?;
    rewrite_package_path_field(&mut editor_state, "targetPath", root, false)?;

    if let Some(resources) = editor_state
        .get_mut("resources")
        .and_then(Value::as_array_mut)
    {
        for resource in resources {
            rewrite_package_path_field(resource, "path", root, true)?;
            if value_string(resource, "previewPath").is_some() {
                rewrite_package_path_field(resource, "previewPath", root, true)?;
            } else if let Some(path) = value_string(resource, "path") {
                set_value_string(resource, "previewPath", path);
            }
            remove_value_field(resource, "imageSrc");
        }
    }

    if let Some(segments) = editor_state
        .get_mut("segments")
        .and_then(Value::as_array_mut)
    {
        for segment in segments {
            if value_string(segment, "imagePath").is_some() {
                rewrite_package_path_field(segment, "imagePath", root, true)?;
            }
            if value_string(segment, "imagePreviewPath").is_some() {
                rewrite_package_path_field(segment, "imagePreviewPath", root, true)?;
            } else if let Some(path) = value_string(segment, "imagePath") {
                set_value_string(segment, "imagePreviewPath", path);
            }
            remove_value_field(segment, "imageSrc");
        }
    }
    Ok(editor_state)
}

fn editor_state_duration(editor_state: &Value) -> f64 {
    editor_state
        .get("sourceInfo")
        .and_then(|value| value.get("duration"))
        .and_then(Value::as_f64)
        .unwrap_or_default()
        .max(0.0)
}

fn same_existing_path(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

fn ensure_package_target_not_source(target: &Path, sources: &[PathBuf]) -> Result<(), String> {
    for source in sources {
        if source.is_file() && same_existing_path(target, source) {
            return Err("导出包路径不能覆盖项目内的视频或图片文件".to_string());
        }
    }
    Ok(())
}

fn write_zip_json<T: Serialize>(
    zip: &mut ZipWriter<File>,
    name: &str,
    value: &T,
) -> Result<(), String> {
    let data = serde_json::to_vec_pretty(value)
        .map_err(|err| format!("序列化项目包 JSON 失败: {}", err))?;
    let options = FileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o644);
    zip.start_file(name, options)
        .map_err(|err| format!("写入项目包失败: {}", err))?;
    zip.write_all(&data)
        .map_err(|err| format!("写入项目包 JSON 失败: {}", err))
}

fn write_zip_file(
    zip: &mut ZipWriter<File>,
    package_path: &str,
    source_path: &Path,
) -> Result<u64, String> {
    validate_package_path(package_path)?;
    let mut source = File::open(source_path).map_err(|err| {
        format!(
            "读取项目文件失败 ({}): {}",
            source_path.to_string_lossy(),
            err
        )
    })?;
    let options = FileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .unix_permissions(0o644);
    zip.start_file(package_path, options)
        .map_err(|err| format!("写入项目包文件失败 ({}): {}", package_path, err))?;
    std::io::copy(&mut source, zip)
        .map_err(|err| format!("复制项目文件到项目包失败 ({}): {}", package_path, err))
}

fn read_zip_json<T: DeserializeOwned>(
    archive: &mut ZipArchive<File>,
    name: &str,
) -> Result<T, String> {
    let mut file = archive
        .by_name(name)
        .map_err(|_| format!("项目包缺少 {}", name))?;
    if file.size() > MAX_PACKAGE_JSON_BYTES {
        return Err(format!("{} 过大，可能不是有效项目包", name));
    }
    let mut content = String::new();
    file.read_to_string(&mut content)
        .map_err(|err| format!("读取项目包 JSON 失败 ({}): {}", name, err))?;
    serde_json::from_str(&content)
        .map_err(|err| format!("项目包 JSON 格式错误 ({}): {}", name, err))
}

fn extract_zip_file(
    archive: &mut ZipArchive<File>,
    package_path: &str,
    target: &Path,
) -> Result<u64, String> {
    validate_package_path(package_path)?;
    let mut file = archive
        .by_name(package_path)
        .map_err(|_| format!("项目包缺少文件: {}", package_path))?;
    if file.is_dir() {
        return Err(format!("项目包文件不能是目录: {}", package_path));
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("创建导入目录失败: {}", err))?;
    }
    let mut output = File::create(target)
        .map_err(|err| format!("创建导入文件失败 ({}): {}", target.to_string_lossy(), err))?;
    std::io::copy(&mut file, &mut output)
        .map_err(|err| format!("解压项目包文件失败 ({}): {}", package_path, err))
}

#[tauri::command]
pub async fn toolbox_video_mask_project_list(
    state: State<'_, AppState>,
) -> Result<ApiResponse<Vec<VideoMaskProjectSummary>>, String> {
    let projects = state
        .db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, name, source_path, duration, revision, created_at, updated_at, last_opened_at \
                 FROM video_mask_project \
                 ORDER BY COALESCE(last_opened_at, updated_at) DESC, updated_at DESC",
            )?;
            let rows = stmt.query_map([], |row| {
                let source_path: Option<String> = row.get(2)?;
                let source_exists = source_path
                    .as_deref()
                    .map(Path::new)
                    .map(Path::is_file)
                    .unwrap_or(false);
                Ok(VideoMaskProjectSummary {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    source_path,
                    duration: row.get(3)?,
                    revision: row.get(4)?,
                    created_at: row.get(5)?,
                    updated_at: row.get(6)?,
                    last_opened_at: row.get(7)?,
                    source_exists,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .map_err(|err| format!("读取视频遮罩项目失败: {}", err))?;
    Ok(ApiResponse::success(projects))
}

#[tauri::command]
pub async fn toolbox_video_mask_project_create(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    payload: VideoMaskProjectCreatePayload,
) -> Result<ApiResponse<VideoMaskProject>, String> {
    let id = Uuid::new_v4().to_string();
    let name = normalized_project_name(payload.name.as_deref());
    let now = Utc::now().to_rfc3339();
    let root = project_root(&app, &id)?;
    fs::create_dir_all(root.join("assets")).map_err(|err| format!("创建项目目录失败: {}", err))?;
    let editor_state = json!({ "version": 1 });
    let editor_state_json = serde_json::to_string(&editor_state)
        .map_err(|err| format!("序列化项目状态失败: {}", err))?;
    if let Err(err) = state.db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO video_mask_project \
             (id, name, source_path, duration, editor_state_json, revision, created_at, updated_at, last_opened_at) \
             VALUES (?1, ?2, NULL, 0, ?3, 0, ?4, ?4, ?4)",
            params![id, name, editor_state_json, now],
        )?;
        Ok(())
    }) {
        let _ = fs::remove_dir_all(root);
        return Err(format!("创建视频遮罩项目失败: {}", err));
    }
    Ok(ApiResponse::success(VideoMaskProject {
        id,
        name,
        source_path: None,
        duration: 0.0,
        editor_state,
        revision: 0,
        created_at: now.clone(),
        updated_at: now.clone(),
        last_opened_at: Some(now),
        source_exists: false,
    }))
}

#[tauri::command]
pub async fn toolbox_video_mask_project_detail(
    state: State<'_, AppState>,
    payload: VideoMaskProjectIdPayload,
) -> Result<ApiResponse<VideoMaskProject>, String> {
    let id = validated_project_id(&payload.project_id)?;
    let now = Utc::now().to_rfc3339();
    let raw = state
        .db
        .with_conn_mut(|conn| {
            let transaction = conn.transaction()?;
            let raw = transaction
                .query_row(
                    "SELECT id, name, source_path, duration, editor_state_json, revision, created_at, updated_at, last_opened_at \
                     FROM video_mask_project WHERE id = ?1",
                    [&id],
                    project_from_row,
                )
                .optional()?;
            if raw.is_some() {
                transaction.execute(
                    "UPDATE video_mask_project SET last_opened_at = ?2 WHERE id = ?1",
                    params![id, now],
                )?;
            }
            transaction.commit()?;
            Ok(raw)
        })
        .map_err(|err| format!("读取视频遮罩项目失败: {}", err))?;
    let Some(raw) = raw else {
        return Ok(ApiResponse::error("项目不存在或已删除"));
    };
    let mut project = parse_project(raw);
    project.last_opened_at = Some(now);
    Ok(ApiResponse::success(project))
}

#[tauri::command]
pub async fn toolbox_video_mask_project_save(
    state: State<'_, AppState>,
    payload: VideoMaskProjectSavePayload,
) -> Result<ApiResponse<VideoMaskProjectSaveResult>, String> {
    let id = validated_project_id(&payload.project_id)?;
    let editor_state_json = serde_json::to_string(&payload.editor_state)
        .map_err(|err| format!("序列化项目状态失败: {}", err))?;
    let source_path = payload
        .source_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let duration = payload.duration.unwrap_or_default().max(0.0);
    let next_revision = payload.revision.saturating_add(1);
    let now = Utc::now().to_rfc3339();
    let changed = state
        .db
        .with_conn(|conn| {
            conn.execute(
                "UPDATE video_mask_project \
                 SET source_path = ?3, duration = ?4, editor_state_json = ?5, revision = ?6, updated_at = ?7 \
                 WHERE id = ?1 AND revision = ?2",
                params![id, payload.revision, source_path, duration, editor_state_json, next_revision, now],
            )
        })
        .map_err(|err| format!("保存视频遮罩项目失败: {}", err))?;
    if changed == 0 {
        return Ok(ApiResponse::error(
            "项目状态已被更新，请重新打开项目后继续编辑",
        ));
    }
    Ok(ApiResponse::success(VideoMaskProjectSaveResult {
        revision: next_revision,
        updated_at: now,
    }))
}

#[tauri::command]
pub async fn toolbox_video_mask_project_rename(
    state: State<'_, AppState>,
    payload: VideoMaskProjectRenamePayload,
) -> Result<ApiResponse<bool>, String> {
    let id = validated_project_id(&payload.project_id)?;
    let name = normalized_project_name(Some(&payload.name));
    let now = Utc::now().to_rfc3339();
    let changed = state
        .db
        .with_conn(|conn| {
            conn.execute(
                "UPDATE video_mask_project SET name = ?2, updated_at = ?3 WHERE id = ?1",
                params![id, name, now],
            )
        })
        .map_err(|err| format!("重命名视频遮罩项目失败: {}", err))?;
    if changed == 0 {
        return Ok(ApiResponse::error("项目不存在或已删除"));
    }
    Ok(ApiResponse::success(true))
}

#[tauri::command]
pub async fn toolbox_video_mask_project_delete(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    payload: VideoMaskProjectIdPayload,
) -> Result<ApiResponse<bool>, String> {
    let id = validated_project_id(&payload.project_id)?;
    let root = project_root(&app, &id)?;
    if root.exists() {
        fs::remove_dir_all(&root).map_err(|err| format!("删除项目资源失败: {}", err))?;
    }
    let changed = state
        .db
        .with_conn(|conn| conn.execute("DELETE FROM video_mask_project WHERE id = ?1", [&id]))
        .map_err(|err| format!("删除视频遮罩项目失败: {}", err))?;
    if changed == 0 {
        return Ok(ApiResponse::error("项目不存在或已删除"));
    }
    Ok(ApiResponse::success(true))
}

fn supported_image(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .map(|value| SUPPORTED_IMAGE_EXTENSIONS.contains(&value.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn copy_project_assets(
    assets_dir: &Path,
    sources: &[PathBuf],
) -> Result<Vec<VideoMaskProjectAsset>, String> {
    if sources.is_empty() {
        return Err("请选择要导入的图片".to_string());
    }
    for source in sources {
        if !source.is_file() {
            return Err(format!("图片文件不存在: {}", source.to_string_lossy()));
        }
        if !supported_image(source) {
            return Err(format!("不支持的图片格式: {}", source.to_string_lossy()));
        }
    }
    fs::create_dir_all(assets_dir).map_err(|err| format!("创建项目资源目录失败: {}", err))?;

    let mut imported = Vec::new();
    for source in sources {
        let extension = source
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("png")
            .to_ascii_lowercase();
        let asset_id = Uuid::new_v4().to_string();
        let target = assets_dir.join(format!("{}.{}", asset_id, extension));
        fs::copy(source, &target)
            .map_err(|err| format!("复制图片到项目失败 ({}): {}", source.to_string_lossy(), err))?;
        let size_bytes = fs::metadata(&target).map(|meta| meta.len()).unwrap_or(0);
        imported.push(VideoMaskProjectAsset {
            id: asset_id,
            original_name: source
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("未命名图片")
                .to_string(),
            managed_path: target.to_string_lossy().to_string(),
            size_bytes,
        });
    }
    Ok(imported)
}

#[tauri::command]
pub async fn toolbox_video_mask_project_asset_import(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    payload: VideoMaskProjectAssetImportPayload,
) -> Result<ApiResponse<Vec<VideoMaskProjectAsset>>, String> {
    let id = validated_project_id(&payload.project_id)?;
    let exists = state
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM video_mask_project WHERE id = ?1)",
                [&id],
                |row| row.get::<_, bool>(0),
            )
        })
        .map_err(|err| format!("校验视频遮罩项目失败: {}", err))?;
    if !exists {
        return Ok(ApiResponse::error("项目不存在或已删除"));
    }

    let sources = payload
        .paths
        .iter()
        .map(|raw_path| PathBuf::from(raw_path.trim()))
        .collect::<Vec<_>>();
    let assets_dir = project_root(&app, &id)?.join("assets");
    match copy_project_assets(&assets_dir, &sources) {
        Ok(imported) => Ok(ApiResponse::success(imported)),
        Err(err) => Ok(ApiResponse::error(err)),
    }
}

fn export_project_package(
    _app: &tauri::AppHandle,
    state: &AppState,
    project_id: &str,
    target_path: &Path,
) -> Result<VideoMaskProjectExportResult, String> {
    if target_path.as_os_str().is_empty() {
        return Err("请选择项目包导出路径".to_string());
    }
    if target_path.is_dir() {
        return Err("项目包导出路径不能是目录".to_string());
    }

    let Some(raw) = load_project_row(state, project_id)? else {
        return Err("项目不存在或已删除".to_string());
    };
    let editor_state = serde_json::from_str::<Value>(&raw.4).unwrap_or_else(|_| json!({}));
    let source_path_value = value_string(&editor_state, "sourcePath")
        .or_else(|| raw.2.clone())
        .ok_or_else(|| "项目缺少源视频，无法导出".to_string())?;
    let source_path = PathBuf::from(source_path_value.trim());
    if !source_path.is_file() {
        return Err(format!("源视频不存在，无法导出: {}", source_path_value));
    }

    let source_extension = extension_from_path(&source_path, "mp4");
    let source_package_path = format!("assets/source/source.{}", source_extension);
    let image_entries = collect_used_image_entries(&editor_state)?;
    let target_state_path = value_string(&editor_state, "targetPath").unwrap_or_else(|| {
        let source_name = file_name_from_any_path(&source_path_value, "source.mp4");
        let base_name = source_name
            .rsplit_once('.')
            .map(|(base, _)| base)
            .unwrap_or(&source_name);
        format!("{}_masked.mp4", base_name)
    });
    let output_name = file_name_from_any_path(&target_state_path, "masked-output.mp4");
    let output_package_path = format!("assets/output/{}", output_name);
    let output_path = PathBuf::from(target_state_path.trim());
    let output_included = output_path.is_file() && !same_existing_path(&output_path, &source_path);

    let mut guarded_sources = Vec::with_capacity(image_entries.len() + 2);
    guarded_sources.push(source_path.clone());
    guarded_sources.extend(
        image_entries
            .iter()
            .map(|entry| PathBuf::from(&entry.original_path)),
    );
    if output_included {
        guarded_sources.push(output_path.clone());
    }
    ensure_package_target_not_source(target_path, &guarded_sources)?;

    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("创建导出目录失败: {}", err))?;
    }

    let export_editor_state = rewrite_editor_state_for_export(
        editor_state,
        &source_package_path,
        &output_package_path,
        &image_entries,
    );
    let exported_at = Utc::now().to_rfc3339();
    let meta = VideoMaskPackageProjectMeta {
        original_project_id: raw.0.clone(),
        name: raw.1.clone(),
        created_at: raw.6.clone(),
        updated_at: raw.7.clone(),
        revision: raw.5,
    };
    let source_size = fs::metadata(&source_path)
        .map(|meta| meta.len())
        .map_err(|err| format!("读取源视频信息失败: {}", err))?;
    let mut assets = vec![VideoMaskPackageAsset {
        role: "source".to_string(),
        path: source_package_path.clone(),
        original_name: file_name_from_any_path(&source_path_value, "source.mp4"),
        size_bytes: source_size,
        resource_id: None,
        segment_ids: Vec::new(),
    }];
    assets.extend(image_entries.iter().map(|entry| VideoMaskPackageAsset {
        role: "image".to_string(),
        path: entry.package_path.clone(),
        original_name: entry.original_name.clone(),
        size_bytes: entry.size_bytes,
        resource_id: entry.resource_id.clone(),
        segment_ids: entry.segment_ids.clone(),
    }));
    if output_included {
        let output_size = fs::metadata(&output_path)
            .map(|meta| meta.len())
            .map_err(|err| format!("读取导出视频信息失败: {}", err))?;
        assets.push(VideoMaskPackageAsset {
            role: "output".to_string(),
            path: output_package_path.clone(),
            original_name: output_name,
            size_bytes: output_size,
            resource_id: None,
            segment_ids: Vec::new(),
        });
    }

    let manifest = VideoMaskPackageManifest {
        format: PACKAGE_FORMAT.to_string(),
        version: PACKAGE_VERSION,
        exported_at: exported_at.clone(),
        project: meta.clone(),
        assets,
    };
    let project_data = VideoMaskPackageProjectData {
        format: PACKAGE_FORMAT.to_string(),
        version: PACKAGE_VERSION,
        project: meta,
        editor_state: export_editor_state,
    };

    let result = (|| -> Result<(), String> {
        let file = File::create(target_path).map_err(|err| format!("创建项目包失败: {}", err))?;
        let mut zip = ZipWriter::new(file);
        write_zip_json(&mut zip, PACKAGE_MANIFEST_JSON, &manifest)?;
        write_zip_json(&mut zip, PACKAGE_PROJECT_JSON, &project_data)?;
        write_zip_file(&mut zip, &source_package_path, &source_path)?;
        for entry in &image_entries {
            write_zip_file(
                &mut zip,
                &entry.package_path,
                Path::new(&entry.original_path),
            )?;
        }
        if output_included {
            write_zip_file(&mut zip, &output_package_path, &output_path)?;
        }
        zip.finish()
            .map_err(|err| format!("完成项目包写入失败: {}", err))?;
        Ok(())
    })();
    if let Err(err) = result {
        let _ = fs::remove_file(target_path);
        return Err(err);
    }

    Ok(VideoMaskProjectExportResult {
        path: target_path.to_string_lossy().to_string(),
        source_included: true,
        image_count: image_entries.len(),
        output_included,
        exported_at,
    })
}

#[tauri::command]
pub async fn toolbox_video_mask_project_export(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    payload: VideoMaskProjectExportPayload,
) -> Result<ApiResponse<VideoMaskProjectExportResult>, String> {
    let id = validated_project_id(&payload.project_id)?;
    let target_path = payload.target_path.trim();
    if target_path.is_empty() {
        return Ok(ApiResponse::error("请选择项目包导出路径"));
    }
    match export_project_package(&app, &state, &id, Path::new(target_path)) {
        Ok(result) => Ok(ApiResponse::success(result)),
        Err(err) => Ok(ApiResponse::error(err)),
    }
}

fn validate_package_header(
    manifest: &VideoMaskPackageManifest,
    project_data: &VideoMaskPackageProjectData,
) -> Result<(), String> {
    if manifest.format != PACKAGE_FORMAT || project_data.format != PACKAGE_FORMAT {
        return Err("项目包格式不匹配".to_string());
    }
    if manifest.version != PACKAGE_VERSION || project_data.version != PACKAGE_VERSION {
        return Err("项目包版本不兼容".to_string());
    }
    if manifest.assets.iter().all(|asset| asset.role != "source") {
        return Err("项目包缺少源视频".to_string());
    }
    let mut paths = BTreeSet::<String>::new();
    for asset in &manifest.assets {
        validate_package_path(&asset.path)?;
        if !matches!(asset.role.as_str(), "source" | "image" | "output") {
            return Err(format!("项目包包含未知资源类型: {}", asset.role));
        }
        if !paths.insert(asset.path.clone()) {
            return Err(format!("项目包资源路径重复: {}", asset.path));
        }
    }
    Ok(())
}

fn import_project_package(
    app: &tauri::AppHandle,
    state: &AppState,
    archive_path: &Path,
) -> Result<VideoMaskProject, String> {
    if !archive_path.is_file() {
        return Err(format!("项目包不存在: {}", archive_path.to_string_lossy()));
    }
    let file = File::open(archive_path).map_err(|err| format!("读取项目包失败: {}", err))?;
    let mut archive =
        ZipArchive::new(file).map_err(|err| format!("项目包不是有效 ZIP 文件: {}", err))?;
    let manifest: VideoMaskPackageManifest = read_zip_json(&mut archive, PACKAGE_MANIFEST_JSON)?;
    let project_data: VideoMaskPackageProjectData =
        read_zip_json(&mut archive, PACKAGE_PROJECT_JSON)?;
    validate_package_header(&manifest, &project_data)?;

    let id = Uuid::new_v4().to_string();
    let name = normalized_project_name(Some(&project_data.project.name));
    let now = Utc::now().to_rfc3339();
    let root = project_root(app, &id)?;
    let import_result = (|| -> Result<Value, String> {
        fs::create_dir_all(&root).map_err(|err| format!("创建新项目目录失败: {}", err))?;
        for asset in &manifest.assets {
            let target = project_path_from_package(&root, &asset.path)?;
            extract_zip_file(&mut archive, &asset.path, &target)?;
        }
        let mut editor_state = project_data.editor_state;
        if value_string(&editor_state, "sourcePath").is_none() {
            if let Some(source) = manifest.assets.iter().find(|asset| asset.role == "source") {
                set_value_string(&mut editor_state, "sourcePath", source.path.clone());
            }
        }
        if value_string(&editor_state, "targetPath").is_none() {
            if let Some(output) = manifest.assets.iter().find(|asset| asset.role == "output") {
                set_value_string(&mut editor_state, "targetPath", output.path.clone());
            }
        }
        rewrite_editor_state_for_import(editor_state, &root)
    })();

    let editor_state = match import_result {
        Ok(value) => value,
        Err(err) => {
            let _ = fs::remove_dir_all(&root);
            return Err(err);
        }
    };
    let source_path = value_string(&editor_state, "sourcePath")
        .ok_or_else(|| "导入项目缺少源视频路径".to_string())?;
    if !Path::new(&source_path).is_file() {
        let _ = fs::remove_dir_all(&root);
        return Err("导入项目源视频解压失败".to_string());
    }
    let duration = editor_state_duration(&editor_state);
    let editor_state_json = serde_json::to_string(&editor_state)
        .map_err(|err| format!("序列化导入项目配置失败: {}", err))?;
    if let Err(err) = state.db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO video_mask_project \
             (id, name, source_path, duration, editor_state_json, revision, created_at, updated_at, last_opened_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?6, ?6)",
            params![id, name, source_path, duration, editor_state_json, now],
        )?;
        Ok(())
    }) {
        let _ = fs::remove_dir_all(&root);
        return Err(format!("写入导入项目失败: {}", err));
    }

    Ok(VideoMaskProject {
        id,
        name,
        source_path: Some(source_path),
        duration,
        editor_state,
        revision: 0,
        created_at: now.clone(),
        updated_at: now.clone(),
        last_opened_at: Some(now),
        source_exists: true,
    })
}

#[tauri::command]
pub async fn toolbox_video_mask_project_import(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    payload: VideoMaskProjectImportPayload,
) -> Result<ApiResponse<VideoMaskProject>, String> {
    let archive_path = payload.archive_path.trim();
    if archive_path.is_empty() {
        return Ok(ApiResponse::error("请选择要导入的项目包"));
    }
    match import_project_package(&app, &state, Path::new(archive_path)) {
        Ok(project) => Ok(ApiResponse::success(project)),
        Err(err) => Ok(ApiResponse::error(err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_database_path() -> PathBuf {
        std::env::temp_dir().join(format!("bili-clip-flow-video-mask-{}.db", Uuid::new_v4()))
    }

    #[test]
    fn validates_project_ids_and_image_extensions() {
        let id = Uuid::new_v4().to_string();
        assert_eq!(validated_project_id(&id).expect("valid UUID"), id);
        assert!(validated_project_id("../invalid").is_err());
        assert!(supported_image(Path::new("mask.PNG")));
        assert!(!supported_image(Path::new("mask.svg")));
    }

    #[test]
    fn package_paths_reject_absolute_and_traversal_entries() {
        assert!(validate_package_path("assets/source/source.mp4").is_ok());
        assert!(validate_package_path("../source.mp4").is_err());
        assert!(validate_package_path("assets/../source.mp4").is_err());
        assert!(validate_package_path("/tmp/source.mp4").is_err());
        assert!(validate_package_path("C:/tmp/source.mp4").is_err());
        assert!(validate_package_path("assets\\source\\source.mp4").is_err());
    }

    #[test]
    fn export_state_keeps_only_timeline_referenced_resources() {
        let state = json!({
            "sourcePath": "/old/source.mp4",
            "targetPath": "/old/output.mp4",
            "resources": [
                { "id": "r-used", "path": "/old/mask-used.png", "previewPath": "/old/cache-used.png", "name": "使用中.png", "imageSrc": "volatile" },
                { "id": "r-unused", "path": "/old/mask-unused.png", "previewPath": "/old/cache-unused.png", "name": "未使用.png" }
            ],
            "segments": [
                { "id": "s1", "imagePath": "/old/mask-used.png", "imagePreviewPath": "/old/cache-used.png", "startTime": 1.0, "endTime": 2.0, "imageSrc": "volatile" }
            ]
        });
        let images = vec![ImagePackageEntry {
            original_path: "/old/mask-used.png".to_string(),
            package_path: "assets/masks/mask-001.png".to_string(),
            original_name: "使用中.png".to_string(),
            resource_id: Some("r-used".to_string()),
            segment_ids: vec!["s1".to_string()],
            size_bytes: 10,
        }];
        let exported = rewrite_editor_state_for_export(
            state,
            "assets/source/source.mp4",
            "assets/output/output.mp4",
            &images,
        );
        assert_eq!(
            value_string(&exported, "sourcePath").as_deref(),
            Some("assets/source/source.mp4")
        );
        assert_eq!(
            value_string(&exported, "targetPath").as_deref(),
            Some("assets/output/output.mp4")
        );
        let resources = exported
            .get("resources")
            .and_then(Value::as_array)
            .expect("resources");
        assert_eq!(resources.len(), 1);
        assert_eq!(
            value_string(&resources[0], "path").as_deref(),
            Some("assets/masks/mask-001.png")
        );
        assert!(resources[0].get("imageSrc").is_none());
        let segments = exported
            .get("segments")
            .and_then(Value::as_array)
            .expect("segments");
        assert_eq!(
            value_string(&segments[0], "imagePath").as_deref(),
            Some("assets/masks/mask-001.png")
        );
        assert!(segments[0].get("imageSrc").is_none());
    }

    #[test]
    fn import_state_resolves_package_paths_under_project_root() {
        let root =
            std::env::temp_dir().join(format!("bili-clip-flow-import-state-{}", Uuid::new_v4()));
        let source = root.join("assets/source/source.mp4");
        let mask = root.join("assets/masks/mask-001.png");
        fs::create_dir_all(source.parent().unwrap()).expect("create source dir");
        fs::create_dir_all(mask.parent().unwrap()).expect("create mask dir");
        fs::write(&source, b"video").expect("write source");
        fs::write(&mask, b"image").expect("write mask");
        let imported = rewrite_editor_state_for_import(
            json!({
                "sourcePath": "assets/source/source.mp4",
                "targetPath": "assets/output/output.mp4",
                "resources": [
                    { "id": "r1", "path": "assets/masks/mask-001.png", "previewPath": "assets/masks/mask-001.png" }
                ],
                "segments": [
                    { "id": "s1", "imagePath": "assets/masks/mask-001.png", "imagePreviewPath": "assets/masks/mask-001.png" }
                ],
                "sourceInfo": { "duration": 3.5 }
            }),
            &root,
        )
        .expect("rewrite import state");
        let source_string = source.to_string_lossy().to_string();
        let mask_string = mask.to_string_lossy().to_string();
        assert_eq!(
            value_string(&imported, "sourcePath").as_deref(),
            Some(source_string.as_str())
        );
        assert_eq!(editor_state_duration(&imported), 3.5);
        let resources = imported
            .get("resources")
            .and_then(Value::as_array)
            .expect("resources");
        assert_eq!(
            value_string(&resources[0], "path").as_deref(),
            Some(mask_string.as_str())
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn project_package_zip_json_and_file_round_trip() {
        let root = std::env::temp_dir().join(format!("bili-clip-flow-package-{}", Uuid::new_v4()));
        let source_dir = root.join("source");
        let extract_dir = root.join("extract");
        fs::create_dir_all(&source_dir).expect("create source dir");
        let source = source_dir.join("source.mp4");
        fs::write(&source, b"video-bytes").expect("write source file");
        let archive_path = root.join("project.zip");

        let manifest = VideoMaskPackageManifest {
            format: PACKAGE_FORMAT.to_string(),
            version: PACKAGE_VERSION,
            exported_at: "now".to_string(),
            project: VideoMaskPackageProjectMeta {
                original_project_id: Uuid::new_v4().to_string(),
                name: "测试项目".to_string(),
                created_at: "now".to_string(),
                updated_at: "now".to_string(),
                revision: 1,
            },
            assets: vec![VideoMaskPackageAsset {
                role: "source".to_string(),
                path: "assets/source/source.mp4".to_string(),
                original_name: "source.mp4".to_string(),
                size_bytes: 11,
                resource_id: None,
                segment_ids: Vec::new(),
            }],
        };
        let project_data = VideoMaskPackageProjectData {
            format: PACKAGE_FORMAT.to_string(),
            version: PACKAGE_VERSION,
            project: manifest.project.clone(),
            editor_state: json!({ "sourcePath": "assets/source/source.mp4" }),
        };

        {
            let file = File::create(&archive_path).expect("create archive");
            let mut zip = ZipWriter::new(file);
            write_zip_json(&mut zip, PACKAGE_MANIFEST_JSON, &manifest).expect("write manifest");
            write_zip_json(&mut zip, PACKAGE_PROJECT_JSON, &project_data).expect("write project");
            write_zip_file(&mut zip, "assets/source/source.mp4", &source).expect("write source");
            zip.finish().expect("finish archive");
        }

        let file = File::open(&archive_path).expect("open archive");
        let mut archive = ZipArchive::new(file).expect("read archive");
        let decoded_manifest: VideoMaskPackageManifest =
            read_zip_json(&mut archive, PACKAGE_MANIFEST_JSON).expect("read manifest");
        assert_eq!(decoded_manifest.format, PACKAGE_FORMAT);
        let target = extract_dir.join("assets/source/source.mp4");
        extract_zip_file(&mut archive, "assets/source/source.mp4", &target)
            .expect("extract source");
        assert_eq!(fs::read(&target).expect("read extracted"), b"video-bytes");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn project_schema_and_revision_guard_are_available() {
        let path = test_database_path();
        let db = crate::db::Db::new(path.clone()).expect("create test database");
        let id = Uuid::new_v4().to_string();
        db.with_conn(|conn| {
            let table_exists = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'video_mask_project')",
                [],
                |row| row.get::<_, bool>(0),
            )?;
            assert!(table_exists);
            conn.execute(
                "INSERT INTO video_mask_project \
                 (id, name, duration, editor_state_json, revision, created_at, updated_at) \
                 VALUES (?1, '测试项目', 0, '{}', 0, 'now', 'now')",
                [&id],
            )?;
            let first = conn.execute(
                "UPDATE video_mask_project SET editor_state_json = '{\"version\":1}', revision = 1 \
                 WHERE id = ?1 AND revision = 0",
                [&id],
            )?;
            let stale = conn.execute(
                "UPDATE video_mask_project SET editor_state_json = '{\"stale\":true}', revision = 1 \
                 WHERE id = ?1 AND revision = 0",
                [&id],
            )?;
            assert_eq!(first, 1);
            assert_eq!(stale, 0);
            Ok(())
        })
        .expect("verify project revision guard");
        drop(db);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn imported_asset_is_independent_from_original_file() {
        let root = std::env::temp_dir().join(format!("bili-clip-flow-assets-{}", Uuid::new_v4()));
        let source_dir = root.join("source");
        let assets_dir = root.join("project/assets");
        fs::create_dir_all(&source_dir).expect("create source directory");
        let source = source_dir.join("遮罩.png");
        fs::write(&source, b"test-image-bytes").expect("write source image");

        let imported =
            copy_project_assets(&assets_dir, &[source.clone()]).expect("copy project asset");
        assert_eq!(imported.len(), 1);
        assert_eq!(imported[0].original_name, "遮罩.png");
        fs::remove_file(source).expect("remove original image");
        assert!(Path::new(&imported[0].managed_path).is_file());

        let _ = fs::remove_dir_all(root);
    }
}
