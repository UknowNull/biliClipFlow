use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{Manager, State};
use uuid::Uuid;

use crate::api::ApiResponse;
use crate::AppState;

const PROJECTS_DIR: &str = "video-mask/projects";
const SUPPORTED_IMAGE_EXTENSIONS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];

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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMaskProjectSaveResult {
    revision: i64,
    updated_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoMaskProjectAsset {
    id: String,
    original_name: String,
    managed_path: String,
    size_bytes: u64,
}

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

fn project_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(
    String,
    String,
    Option<String>,
    f64,
    String,
    i64,
    String,
    String,
    Option<String>,
)> {
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

fn parse_project(
    raw: (
        String,
        String,
        Option<String>,
        f64,
        String,
        i64,
        String,
        String,
        Option<String>,
    ),
) -> VideoMaskProject {
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
