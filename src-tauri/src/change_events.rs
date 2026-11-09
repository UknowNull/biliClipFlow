//! 数据变更通知：写入点只置脏标记，后台循环每秒合并为一次事件推给前端，前端据此刷新，
//! 替代高频轮询。数据库写入由 SQLite update_hook 统一标记，内存状态由各自写入口标记。
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};

pub const APP_DATA_CHANGED_EVENT: &str = "app-data-changed";

static SUBMISSION: AtomicBool = AtomicBool::new(false);
static DOWNLOAD: AtomicBool = AtomicBool::new(false);
static EDIT_UPLOAD: AtomicBool = AtomicBool::new(false);
static TOOLBOX: AtomicBool = AtomicBool::new(false);

pub fn mark_edit_upload() {
    EDIT_UPLOAD.store(true, Ordering::Relaxed);
}

pub fn mark_toolbox() {
    TOOLBOX.store(true, Ordering::Relaxed);
}

/// 由 SQLite update_hook 调用，运行在持有数据库锁的线程上，只能做轻量操作。
pub fn mark_table(table: &str) {
    match table {
        "submission_task" | "merged_video" | "task_output_segment" | "task_source_video" => {
            SUBMISSION.store(true, Ordering::Relaxed);
        }
        "workflow_instances" | "task_relations" => {
            SUBMISSION.store(true, Ordering::Relaxed);
            DOWNLOAD.store(true, Ordering::Relaxed);
        }
        "video_download" => DOWNLOAD.store(true, Ordering::Relaxed),
        _ => {}
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChangedPayload {
    submission: bool,
    download: bool,
    edit_upload: bool,
    toolbox: bool,
}

pub fn start_emit_loop(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(1));
        loop {
            ticker.tick().await;
            let payload = ChangedPayload {
                submission: SUBMISSION.swap(false, Ordering::Relaxed),
                download: DOWNLOAD.swap(false, Ordering::Relaxed),
                edit_upload: EDIT_UPLOAD.swap(false, Ordering::Relaxed),
                toolbox: TOOLBOX.swap(false, Ordering::Relaxed),
            };
            if payload.submission || payload.download || payload.edit_upload || payload.toolbox {
                let _ = app.emit(APP_DATA_CHANGED_EVENT, payload);
            }
        }
    });
}
