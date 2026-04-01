use std::collections::HashMap;
use std::process::Child;
use std::sync::{Arc, Mutex};

use tokio::time::{sleep, Duration};

use tauri::Manager;

mod account_store;
mod api;
mod app_log;
mod baidu_sync;
mod bilibili;
mod commands;
mod config;
mod db;
mod ffmpeg;
mod live_recorder;
mod login_refresh;
mod login_store;
mod path_store;
mod processing;
mod utils;

struct AppState {
    db: Arc<db::Db>,
    bilibili: Arc<bilibili::client::BilibiliClient>,
    login_store: Arc<login_store::LoginStore>,
    log_path: Arc<std::path::PathBuf>,
    app_log_path: Arc<std::path::PathBuf>,
    download_runtime: Arc<DownloadRuntime>,
    live_runtime: Arc<live_recorder::LiveRuntime>,
    edit_upload_state: Arc<Mutex<commands::submission::EditUploadState>>,
    baidu_sync_runtime: Arc<baidu_sync::BaiduSyncRuntime>,
    baidu_login_runtime: Arc<Mutex<commands::baidu_sync::BaiduLoginRuntime>>,
}

struct DownloadRuntime {
    active_count: Mutex<i64>,
    progress_state: Mutex<HashMap<i64, HashMap<String, (u64, u64)>>>,
    baidu_children: Mutex<HashMap<i64, Arc<Mutex<Child>>>>,
}

impl DownloadRuntime {
    fn new() -> Self {
        Self {
            active_count: Mutex::new(0),
            progress_state: Mutex::new(HashMap::new()),
            baidu_children: Mutex::new(HashMap::new()),
        }
    }
}

fn init_panic_log(path: Arc<std::path::PathBuf>) {
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|loc| format!("{}:{}", loc.file(), loc.line()))
            .unwrap_or_else(|| "unknown".to_string());
        app_log::append_log(
            &path,
            &format!(
                "panic ts={} location={} info={}",
                app_log::now_millis(),
                location,
                info
            ),
        );
    }));
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .on_window_event(|window, event| {
            let state = window.app_handle().state::<AppState>();
            match event {
                tauri::WindowEvent::CloseRequested { .. } => {
                    utils::append_log(&state.app_log_path, "window_close_requested");
                }
                tauri::WindowEvent::Destroyed => {
                    utils::append_log(&state.app_log_path, "window_destroyed");
                }
                _ => {}
            }
        })
        .setup(|app| {
            let app_dir = app.path().app_data_dir()?;
            config::init_resource_bins(&app.handle());
            std::fs::create_dir_all(&app_dir)?;
            let db_path = config::database_path(&app_dir);
            let db = Arc::new(db::Db::new(db_path)?);
            let login_path = app_dir.join("bilibili_login_info.json");
            let download_dir = commands::settings::load_download_settings_from_db(&db)
                .map(|settings| settings.download_path)
                .unwrap_or_else(|_| config::default_download_dir().to_string_lossy().to_string());
            let log_dir =
                commands::settings::ensure_log_dir(&db, std::path::Path::new(&download_dir));
            let log_dir = std::path::PathBuf::from(log_dir);
            let log_path = log_dir.join("auth_debug.log");
            let app_log_path = log_dir.join("app_debug.log");
            let panic_log_path = log_dir.join("panic_debug.log");
            utils::append_log(&app_log_path, "app_start");
            if let Some(resource_dir) = config::resolve_resource_bin_dir(&app.handle()) {
                utils::append_log(
                    &app_log_path,
                    &format!("resource_bin_dir={}", resource_dir.to_string_lossy()),
                );
            } else {
                utils::append_log(&app_log_path, "resource_bin_dir_missing");
            }
            let ffmpeg_path = config::resolve_ffmpeg_path();
            let ffprobe_path = config::resolve_ffprobe_path();
            let aria2c_candidates = config::resolve_aria2c_candidates();
            let baidu_pcs_candidates = config::resolve_baidu_pcs_candidates();
            utils::append_log(
                &app_log_path,
                &format!(
                    "bin_paths ffmpeg={} ffprobe={} aria2c={} baidu_pcs={}",
                    ffmpeg_path.to_string_lossy(),
                    ffprobe_path.to_string_lossy(),
                    aria2c_candidates.join(","),
                    baidu_pcs_candidates.join(",")
                ),
            );
            init_panic_log(Arc::new(panic_log_path));
            let heartbeat_path = app_log_path.clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    utils::append_log(&heartbeat_path, "heartbeat");
                    sleep(Duration::from_secs(30)).await;
                }
            });
            let state = AppState {
                db,
                bilibili: Arc::new(bilibili::client::BilibiliClient::new()),
                login_store: Arc::new(login_store::LoginStore::new(login_path)),
                log_path: Arc::new(log_path),
                app_log_path: Arc::new(app_log_path),
                download_runtime: Arc::new(DownloadRuntime::new()),
                live_runtime: Arc::new(live_recorder::new_live_runtime()),
                edit_upload_state: Arc::new(Mutex::new(
                    commands::submission::EditUploadState::default(),
                )),
                baidu_sync_runtime: Arc::new(baidu_sync::BaiduSyncRuntime::new()),
                baidu_login_runtime: Arc::new(Mutex::new(
                    commands::baidu_sync::BaiduLoginRuntime::default(),
                )),
            };
            commands::download::recover_stale_downloads(&state);
            commands::download::start_download_queue_loop(&state);
            let live_context = live_recorder::LiveContext {
                db: Arc::clone(&state.db),
                bilibili: Arc::clone(&state.bilibili),
                login_store: Arc::clone(&state.login_store),
                app_log_path: Arc::clone(&state.app_log_path),
                live_runtime: Arc::clone(&state.live_runtime),
            };
            live_recorder::recover_stale_recordings(live_context.clone());
            live_recorder::start_record_recovery_loop(live_context.clone());
            live_recorder::start_auto_record_loop(live_context);
            login_refresh::start_cookie_refresh_loop(
                Arc::clone(&state.db),
                Arc::clone(&state.bilibili),
                Arc::clone(&state.login_store),
                Arc::clone(&state.app_log_path),
            );
            commands::submission::start_submission_background_tasks(
                Arc::clone(&state.db),
                Arc::clone(&state.bilibili),
                Arc::clone(&state.login_store),
                Arc::clone(&state.app_log_path),
                Arc::clone(&state.edit_upload_state),
            );
            let baidu_context = baidu_sync::BaiduSyncContext {
                db: Arc::clone(&state.db),
                app_log_path: Arc::clone(&state.app_log_path),
                runtime: Arc::clone(&state.baidu_sync_runtime),
            };
            baidu_sync::start_baidu_sync_loop(baidu_context);
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::file_scanner::scan_path,
            commands::file_scanner::validate_directory,
            commands::file_scanner::video_duration,
            commands::auth::auth_qrcode_generate,
            commands::auth::auth_qrcode_poll,
            commands::auth::auth_sms_login,
            commands::auth::auth_pwd_login,
            commands::auth::auth_status,
            commands::auth::auth_refresh,
            commands::auth::auth_accounts_list,
            commands::auth::auth_account_switch,
            commands::auth::auth_account_set_primary,
            commands::auth::auth_account_logout,
            commands::auth::account_binding_list,
            commands::auth::account_binding_get,
            commands::auth::account_binding_set,
            commands::auth::auth_client_log,
            commands::auth::auth_logout,
            commands::auth::auth_perform_qrcode_login,
            commands::settings::get_download_settings,
            commands::settings::update_download_settings,
            commands::settings::get_live_settings,
            commands::settings::update_live_settings,
            commands::settings::settings_cache_overview,
            commands::settings::settings_cache_clear,
            commands::settings::settings_cache_open,
            commands::app_update::app_version,
            commands::app_update::app_update_check,
            commands::anchor::anchor_subscribe,
            commands::anchor::anchor_list,
            commands::anchor::anchor_unsubscribe,
            commands::anchor::anchor_check,
            commands::anchor::anchor_record_list,
            commands::anchor::anchor_open_record_dir,
            commands::anchor::anchor_open_record_file,
            commands::anchor::anchor_submission_config_get,
            commands::anchor::anchor_submission_config_save,
            commands::anchor::anchor_analyze_clips,
            commands::anchor::anchor_clip_task_status,
            commands::anchor::anchor_clip_update_time,
            commands::anchor::anchor_clip_reclip,
            commands::anchor::anchor_clip_delete,
            commands::anchor::anchor_clip_list,
            commands::live::live_record_start,
            commands::live::live_record_stop,
            commands::live::live_room_auto_record_update,
            commands::live::live_room_baidu_sync_update,
            commands::live::live_room_baidu_sync_toggle,
            commands::video::video_detail,
            commands::video::video_playurl,
            commands::video::video_playurl_by_aid,
            commands::video::video_proxy_image,
            commands::video::bilibili_collections,
            commands::video::bilibili_partitions,
            commands::video::bilibili_topics,
            commands::video::bilibili_topics_field_schema,
            commands::download::download_video,
            commands::download::download_get,
            commands::download::download_list_by_status,
            commands::download::download_delete,
            commands::download::download_retry,
            commands::download::download_resume,
            commands::process::process_create,
            commands::process::process_status,
            commands::toolbox::toolbox_remux,
            commands::submission::submission_queue_prioritize,
            commands::baidu_sync::baidu_sync_settings,
            commands::baidu_sync::baidu_sync_status,
            commands::baidu_sync::baidu_accounts_list,
            commands::baidu_sync::baidu_account_switch,
            commands::baidu_sync::baidu_account_logout,
            commands::baidu_sync::baidu_sync_login,
            commands::baidu_sync::baidu_sync_logout,
            commands::baidu_sync::baidu_sync_web_login,
            commands::baidu_sync::baidu_sync_account_login_start,
            commands::baidu_sync::baidu_sync_account_login_status,
            commands::baidu_sync::baidu_sync_account_login_input,
            commands::baidu_sync::baidu_sync_account_login_cancel,
            commands::baidu_sync::baidu_sync_list,
            commands::baidu_sync::baidu_sync_remote_dirs,
            commands::baidu_sync::baidu_sync_remote_entries,
            commands::baidu_sync::baidu_sync_create_dir,
            commands::baidu_sync::baidu_sync_rename_dir,
            commands::baidu_sync::baidu_sync_retry,
            commands::baidu_sync::baidu_sync_cancel,
            commands::baidu_sync::baidu_sync_pause,
            commands::baidu_sync::baidu_sync_delete,
            commands::baidu_sync::baidu_sync_update_settings,
            commands::submission::submission_create,
            commands::submission::submission_update,
            commands::submission::submission_repost,
            commands::submission::submission_resegment,
            commands::submission::submission_list,
            commands::submission::submission_list_by_status,
            commands::submission::submission_remote_video_preview,
            commands::submission::submission_remote_import_save,
            commands::submission::submission_export,
            commands::submission::submission_import,
            commands::submission::submission_task_dir,
            commands::submission::submission_open_task_dir,
            commands::submission::submission_delete_preview,
            commands::submission::submission_detail,
            commands::submission::submission_bind_merged_remote_file,
            commands::submission::submission_clear_merged_remote_file,
            commands::submission::submission_reorder_source_videos,
            commands::submission::submission_reorder_merged_videos,
            commands::submission::submission_bind_merged_sources,
            commands::submission::submission_batch_bind_segments_merged,
            commands::submission::submission_delete_merged_video,
            commands::submission::submission_upload_cover,
            commands::submission::submission_cover_local_preview,
            commands::submission::submission_edit_prepare,
            commands::submission::submission_edit_update_sources,
            commands::submission::submission_edit_add_segment,
            commands::submission::submission_edit_reupload_segment,
            commands::submission::submission_edit_upload_status,
            commands::submission::submission_edit_upload_clear,
            commands::submission::submission_edit_submit,
            commands::submission::submission_delete,
            commands::submission::submission_execute,
            commands::submission::submission_integrated_execute,
            commands::submission::submission_upload_execute,
            commands::submission::submission_retry_segment_upload,
            commands::submission::workflow_status,
            commands::submission::workflow_pause,
            commands::submission::workflow_resume,
            commands::submission::workflow_cancel,
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application");

    app.run(|app_handle, event| {
        if let Some(state) = app_handle.try_state::<AppState>() {
            match event {
                tauri::RunEvent::Ready => {
                    utils::append_log(&state.app_log_path, "run_ready");
                }
                tauri::RunEvent::ExitRequested { code, .. } => {
                    utils::append_log(
                        &state.app_log_path,
                        &format!("run_exit_requested code={:?}", code),
                    );
                }
                tauri::RunEvent::Exit => {
                    utils::append_log(&state.app_log_path, "run_exit");
                }
                _ => {}
            }
        }
    });
}
