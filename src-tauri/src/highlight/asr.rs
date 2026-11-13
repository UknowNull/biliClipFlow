//! 本地语音识别：sherpa-onnx 运行 SenseVoice（语种/情绪/事件）与 Paraformer（中文文本），silero VAD 分段。
//!
//! 识别器走 sherpa-onnx-sys 底层接口：官方 Rust 封装的结果结构丢弃了 lang/emotion/event 字段，
//! 这里直接读取 C 接口返回的完整 JSON。

use std::ffi::{CStr, CString};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sherpa_onnx::{OfflinePunctuation, OfflinePunctuationConfig, OfflinePunctuationModelConfig, SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};
use sherpa_onnx_sys::offline_asr as sys;

use crate::config::resolve_ffmpeg_path;
use crate::utils::apply_no_window;

pub const SAMPLE_RATE: i32 = 16000;

// ---------------------------------------------------------------------------
// 模型文件
// ---------------------------------------------------------------------------

struct ModelFile {
    rel_path: &'static str,
    /// 相对下载源根地址的路径
    remote: &'static str,
    size: u64,
    /// 接受的 SHA256（同一模型可能有多种导出格式）
    sha256: &'static [&'static str],
}

const MODEL_FILES: &[ModelFile] = &[
    ModelFile {
        rel_path: "sensevoice/model.int8.onnx",
        remote: "csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/main/model.int8.onnx",
        size: 239_233_841,
        sha256: &["c71f0ce00bec95b07744e116345e33d8cbbe08cef896382cf907bf4b51a2cd51"],
    },
    ModelFile {
        rel_path: "sensevoice/tokens.txt",
        remote: "csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/main/tokens.txt",
        size: 315_894,
        sha256: &["f449eb28dc567533d7fa59be34e2abca8784f771850c78a47fb731a31429a1dc"],
    },
    ModelFile {
        rel_path: "paraformer/model.int8.onnx",
        remote: "csukuangfj/sherpa-onnx-paraformer-zh-2023-09-14/resolve/main/model.int8.onnx",
        size: 243_371_218,
        sha256: &["f36a0433bcf096bd6d6f11b80a3ac8bed110bdca632fe0d731df8d1a84475945"],
    },
    ModelFile {
        rel_path: "paraformer/tokens.txt",
        remote: "csukuangfj/sherpa-onnx-paraformer-zh-2023-09-14/resolve/main/tokens.txt",
        size: 75_756,
        sha256: &["59aba8873a2ed1e122c25fee421e25f283b63290efbde85c1f01a853d83cb6e6"],
    },
    ModelFile {
        rel_path: "vad/silero_vad.onnx",
        remote: "csukuangfj/vad/resolve/main/silero_vad.onnx",
        size: 1_807_522,
        sha256: &[
            "a35ebf52fd3ce5f1469b2a36158dba761bc47b973ea3382b3186ca15b1f5af28",
            "9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6",
        ],
    },
    // 标点模型（可选）：给 Paraformer 的无标点结果加标点，缺失时跳过
    ModelFile {
        rel_path: PUNCT_MODEL,
        remote: "csukuangfj/sherpa-onnx-punct-ct-transformer-zh-en-vocab272727-2024-04-12/resolve/main/model.onnx",
        size: 294_372_519,
        sha256: &["e93593a6dbd69a07f8734ef269dbe861a379755f8d1c8354719432116f2c44bd"],
    },
    // 声纹模型（可选）：缺失时按语种区分主播与原片
    ModelFile {
        rel_path: super::speaker::MODEL_REL,
        remote: "csukuangfj/speaker-embedding-models/resolve/main/3dspeaker_speech_campplus_sv_zh-cn_16k-common.onnx",
        size: 28_281_138,
        sha256: &["f682b514c05d947ee3fa91cd6ec6c5c7543479a128373fa29b1faedccd21fd11"],
    },
];

fn source_base(source: &str) -> &'static str {
    match source {
        "huggingface" => "https://huggingface.co",
        _ => "https://hf-mirror.com",
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelFileStatus {
    pub path: String,
    pub ready: bool,
    pub size: u64,
    pub expected_size: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    pub running: bool,
    pub current_file: String,
    pub downloaded: u64,
    pub total: u64,
    pub error: Option<String>,
    /// 当前阶段：download / verify / extract
    pub stage: String,
}

fn progress_cell() -> &'static Mutex<DownloadProgress> {
    static CELL: OnceLock<Mutex<DownloadProgress>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(DownloadProgress::default()))
}

pub fn download_progress() -> DownloadProgress {
    progress_cell().lock().map(|p| p.clone()).unwrap_or_default()
}

/// 只按文件大小判断是否就绪（完整校验在下载完成时做过）
pub fn model_status(dir: &Path) -> Vec<ModelFileStatus> {
    MODEL_FILES
        .iter()
        .map(|f| {
            let size = std::fs::metadata(dir.join(f.rel_path)).map(|m| m.len()).unwrap_or(0);
            ModelFileStatus {
                path: f.rel_path.to_string(),
                ready: size > 0 && (size == f.size || f.rel_path.starts_with("vad/")),
                size,
                expected_size: f.size,
            }
        })
        .collect()
}

/// 语音识别必需的模型是否就绪（声纹、标点模型可选，不计入）
pub fn models_ready(dir: &Path) -> bool {
    model_status(dir).iter().filter(|s| s.path != super::speaker::MODEL_REL && s.path != PUNCT_MODEL).all(|s| s.ready)
}

/// 下载缺失的模型文件（支持断点续传），完成后校验 SHA256。阻塞执行。
pub fn download_models(dir: &Path, source: &str) -> Result<(), String> {
    {
        let mut p = progress_cell().lock().map_err(|_| "下载状态不可用")?;
        if p.running {
            return Err("模型正在下载中".to_string());
        }
        *p = DownloadProgress { running: true, ..Default::default() };
    }
    let result = download_all(dir, source);
    if let Ok(mut p) = progress_cell().lock() {
        p.running = false;
        p.error = result.as_ref().err().cloned();
    }
    result
}

fn download_all(dir: &Path, source: &str) -> Result<(), String> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(20))
        .timeout(None)
        .build()
        .map_err(|e| e.to_string())?;
    for (i, f) in MODEL_FILES.iter().enumerate() {
        let target = dir.join(f.rel_path);
        if model_status(dir)[i].ready {
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建模型目录失败: {}", e))?;
        }
        let part = target.with_extension("part");
        let mut done = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        let url = format!("{}/{}", source_base(source), f.remote);
        let mut req = client.get(url);
        if done > 0 {
            req = req.header(reqwest::header::RANGE, format!("bytes={}-", done));
        }
        let mut resp = req.send().map_err(|e| format!("下载失败 {}: {}", f.rel_path, e))?;
        let status = resp.status();
        if status.as_u16() == 200 {
            done = 0;
        } else if status.as_u16() != 206 {
            return Err(format!("下载失败 {}: HTTP {}", f.rel_path, status));
        }
        let mut out = std::fs::OpenOptions::new()
            .create(true)
            .append(done > 0)
            .write(true)
            .truncate(done == 0)
            .open(&part)
            .map_err(|e| format!("写入模型文件失败: {}", e))?;
        let total = done + resp.content_length().unwrap_or(f.size.saturating_sub(done));
        if let Ok(mut p) = progress_cell().lock() {
            p.current_file = f.rel_path.to_string();
            p.downloaded = done;
            p.total = total;
        }
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let n = resp.read(&mut buf).map_err(|e| format!("下载中断 {}: {}", f.rel_path, e))?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n]).map_err(|e| format!("写入模型文件失败: {}", e))?;
            done += n as u64;
            if let Ok(mut p) = progress_cell().lock() {
                p.downloaded = done;
            }
        }
        drop(out);
        let digest = sha256_file(&part)?;
        if !f.sha256.contains(&digest.as_str()) {
            let _ = std::fs::remove_file(&part);
            return Err(format!("模型校验失败 {}，已删除，请重试", f.rel_path));
        }
        std::fs::rename(&part, &target).map_err(|e| format!("保存模型文件失败: {}", e))?;
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{:02x}", b)).collect())
}

// ---------------------------------------------------------------------------
// 模型包导入（本地 zip）
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ModelPackageManifest {
    #[serde(default)]
    schema: String,
    version: String,
    files: Vec<ModelPackageFile>,
}

#[derive(Debug, Deserialize)]
struct ModelPackageFile {
    path: String,
    size: u64,
    sha256: String,
}

const MODEL_PACKAGE_SCHEMA: &str = "biliclipflow-model-package";

/// 包内路径安全校验：拒绝绝对路径、盘符、反斜杠和 .. 等
fn validate_package_path(path: &str) -> Result<(), String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("模型包内路径不能为空".to_string());
    }
    if trimmed.contains('\\') {
        return Err(format!("模型包内路径必须使用 / 分隔: {}", trimmed));
    }
    if trimmed.contains(':') {
        return Err(format!("模型包内路径不能包含盘符或协议: {}", trimmed));
    }
    let candidate = Path::new(trimmed);
    if candidate.is_absolute() {
        return Err(format!("模型包内路径不能是绝对路径: {}", trimmed));
    }
    for component in candidate.components() {
        match component {
            std::path::Component::Normal(part) if !part.to_string_lossy().trim().is_empty() => {}
            _ => return Err(format!("模型包内路径不安全: {}", trimmed)),
        }
    }
    Ok(())
}

/// 导入模型包：校验 → 解压到临时目录 → 逐文件 SHA256 → 原子替换。阻塞执行。
/// 同步占用进度锁。命令层在 spawn 后台线程前调用，确保前端轮询能看到 running。
pub fn begin_progress(stage: &str) -> Result<(), String> {
    let mut p = progress_cell().lock().map_err(|_| "下载状态不可用")?;
    if p.running {
        return Err("模型正在下载或导入中".to_string());
    }
    *p = DownloadProgress { running: true, stage: stage.to_string(), ..Default::default() };
    Ok(())
}

fn finish_progress(result: &Result<String, String>) {
    if let Ok(mut p) = progress_cell().lock() {
        p.running = false;
        p.stage = String::new();
        p.error = result.as_ref().err().cloned();
    }
}

pub fn import_model_package(dir: &Path, zip_path: &Path) -> Result<String, String> {
    begin_progress("verify")?;
    let result = import_model_package_inner(dir, zip_path);
    finish_progress(&result);
    result
}

/// 已由调用方 begin_progress 占位后执行导入。
pub fn import_model_package_locked(dir: &Path, zip_path: &Path) -> Result<String, String> {
    let result = import_model_package_inner(dir, zip_path);
    finish_progress(&result);
    result
}

fn import_model_package_inner(dir: &Path, zip_path: &Path) -> Result<String, String> {
    if !zip_path.is_file() {
        return Err(format!("模型包不存在: {}", zip_path.to_string_lossy()));
    }
    let file = std::fs::File::open(zip_path).map_err(|e| format!("读取模型包失败: {}", e))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("模型包不是有效 ZIP: {}", e))?;

    // 1) 读 manifest
    let manifest: ModelPackageManifest = {
        let mut entry = archive
            .by_name("manifest.json")
            .map_err(|_| "模型包缺少 manifest.json".to_string())?;
        let mut buf = String::new();
        entry
            .read_to_string(&mut buf)
            .map_err(|e| format!("读取 manifest 失败: {}", e))?;
        serde_json::from_str(&buf).map_err(|e| format!("解析 manifest 失败: {}", e))?
    };
    if manifest.schema != MODEL_PACKAGE_SCHEMA {
        return Err(format!("模型包类型不匹配: {}", manifest.schema));
    }
    if manifest.files.is_empty() {
        return Err("模型包内没有模型文件".to_string());
    }

    // 2) 磁盘空间预检（解压后约等于包内文件总大小）
    let need: u64 = manifest.files.iter().map(|f| f.size).sum();
    ensure_free_space(dir, need)?;

    // 3) 解压到临时目录并逐文件校验
    let staging = dir.join(format!(".importing-{}", uuid::Uuid::new_v4()));
    let result = (|| -> Result<(), String> {
        std::fs::create_dir_all(&staging)
            .map_err(|e| format!("创建临时目录失败: {}", e))?;
        let total_files = manifest.files.len() as u64;
        for (index, item) in manifest.files.iter().enumerate() {
            validate_package_path(&item.path)?;
            let target = staging.join(&item.path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("创建目录失败: {}", e))?;
            }
            let mut entry = archive
                .by_name(&item.path)
                .map_err(|_| format!("模型包缺少文件: {}", item.path))?;
            if entry.is_dir() {
                return Err(format!("模型包文件不能是目录: {}", item.path));
            }
            let mut out = std::fs::File::create(&target)
                .map_err(|e| format!("写入失败 {}: {}", item.path, e))?;
            std::io::copy(&mut entry, &mut out)
                .map_err(|e| format!("解压失败 {}: {}", item.path, e))?;
            drop(out);

            if let Ok(mut p) = progress_cell().lock() {
                p.stage = "extract".to_string();
                p.current_file = item.path.clone();
                p.downloaded = index as u64 + 1;
                p.total = total_files;
            }

            let size = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
            if size != item.size {
                return Err(format!("文件大小不符 {}: {} != {}", item.path, size, item.size));
            }
            let digest = sha256_file(&target)?;
            if digest != item.sha256 {
                return Err(format!("文件校验失败: {}", item.path));
            }
        }
        Ok(())
    })();

    if let Err(err) = result {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(err);
    }

    // 4) 原子替换：先备份旧目录，再改名，失败则回滚
    if let Ok(mut p) = progress_cell().lock() {
        p.stage = "install".to_string();
        p.current_file = "应用模型文件".to_string();
    }
    install_staged_models(dir, &staging)?;
    Ok(format!("模型包 {} 已安装", manifest.version))
}

/// 把 staging/asr/* 安装进目标目录。
///
/// 先复制到同级的 .incoming 目录，全部成功后整体替换；中途失败则丢弃副本，
/// 保证目标目录不会出现新旧模型混用。
fn install_staged_models(dir: &Path, staging: &Path) -> Result<(), String> {
    let staged_asr = staging.join("asr");
    if !staged_asr.is_dir() {
        let _ = std::fs::remove_dir_all(staging);
        return Err("模型包缺少 asr 目录".to_string());
    }
    let parent = dir.parent().unwrap_or(dir);
    let incoming = parent.join(format!(".asr-incoming-{}", uuid::Uuid::new_v4()));
    let backup = parent.join(format!(".asr-backup-{}", uuid::Uuid::new_v4()));

    let result = (|| -> Result<(), String> {
        copy_tree_into(&staged_asr, &incoming)?;
        // 现有目录先改名留底，再整体换上副本
        if dir.exists() {
            std::fs::rename(dir, &backup)
                .map_err(|e| format!("备份旧模型失败: {}", e))?;
        }
        if let Err(err) = std::fs::rename(&incoming, dir) {
            if backup.exists() {
                let _ = std::fs::rename(&backup, dir);
            }
            return Err(format!("安装模型失败: {}", err));
        }
        Ok(())
    })();

    match &result {
        Ok(()) => {
            let _ = std::fs::remove_dir_all(&backup);
        }
        Err(_) => {
            let _ = std::fs::remove_dir_all(&incoming);
            let _ = std::fs::remove_dir_all(&backup);
        }
    }
    let _ = std::fs::remove_dir_all(staging);
    result
}

fn copy_tree_into(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("创建模型目录失败: {}", e))?;
    for entry in std::fs::read_dir(from).map_err(|e| format!("读取模型目录失败: {}", e))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let source = entry.path();
        let target = to.join(entry.file_name());
        if source.is_dir() {
            copy_tree_into(&source, &target)?;
        } else {
            std::fs::copy(&source, &target).map_err(|e| {
                format!("复制模型文件失败 {}: {}", target.to_string_lossy(), e)
            })?;
        }
    }
    Ok(())
}

fn ensure_free_space(dir: &Path, need: u64) -> Result<(), String> {
    let probe = if dir.exists() { dir } else { Path::new(".") };
    // 预留 20% 余量，避免解压到一半空间不足
    let required = need.saturating_add(need / 5);
    match free_space_bytes(probe) {
        Some(free) if free < required => Err(format!(
            "磁盘空间不足：需要约 {} MB，可用 {} MB",
            required / 1024 / 1024,
            free / 1024 / 1024
        )),
        _ => Ok(()),
    }
}

/// 用 df 查询可用空间，避免为此引入 libc 依赖。查询失败时跳过预检。
fn free_space_bytes(path: &Path) -> Option<u64> {
    let output = std::process::Command::new("df")
        .arg("-k")
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().nth(1)?;
    // df -k 输出：Filesystem 1024-blocks Used Available Capacity Mounted-on
    let available_kb: u64 = line.split_whitespace().nth(3)?.parse().ok()?;
    Some(available_kb.saturating_mul(1024))
}

// ---------------------------------------------------------------------------
// 模型包下载（整包）
// ---------------------------------------------------------------------------

/// 模型包固定下载地址。模型更新时只替换该 Release 下的同名资产。
pub const MODEL_PACKAGE_URL: &str =
    "https://github.com/UknowNull/biliClipFlow/releases/download/models-latest/biliClipFlow-models.zip";

/// 下载模型包到 cache_dir，返回本地路径。支持断点续传。
pub fn download_model_package(cache_dir: &Path) -> Result<PathBuf, String> {
    begin_progress("download")?;
    download_model_package_locked(cache_dir)
}

/// 已由调用方 begin_progress 占位后执行下载。后续解压校验不释放锁。
pub fn download_model_package_locked(cache_dir: &Path) -> Result<PathBuf, String> {
    let result = download_package_inner(cache_dir);
    if let Err(err) = &result {
        if let Ok(mut p) = progress_cell().lock() {
            p.running = false;
            p.stage = String::new();
            p.error = Some(err.clone());
        }
    }
    result
}

fn download_package_inner(cache_dir: &Path) -> Result<PathBuf, String> {
    let target = cache_dir.join("biliClipFlow-models.zip");
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建缓存目录失败: {}", e))?;
    }
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(20))
        .timeout(None)
        .build()
        .map_err(|e| e.to_string())?;
    let url = MODEL_PACKAGE_URL;
    let part = target.with_extension("part");
    let mut done = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    let mut req = client.get(url);
    if done > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={}-", done));
    }
    let mut resp = req.send().map_err(|e| format!("下载模型包失败: {}", e))?;
    let status = resp.status();
    if status.as_u16() == 200 {
        done = 0;
    } else if status.as_u16() != 206 {
        return Err(format!("下载模型包失败: HTTP {}", status));
    }
    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .append(done > 0)
        .write(true)
        .truncate(done == 0)
        .open(&part)
        .map_err(|e| format!("写入模型包失败: {}", e))?;
    let total = done + resp.content_length().unwrap_or(0);
    if let Ok(mut p) = progress_cell().lock() {
        p.current_file = "biliClipFlow-models.zip".to_string();
        p.downloaded = done;
        p.total = total;
    }
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = resp.read(&mut buf).map_err(|e| format!("下载中断: {}", e))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n]).map_err(|e| format!("写入模型包失败: {}", e))?;
        done += n as u64;
        if let Ok(mut p) = progress_cell().lock() {
            p.downloaded = done;
        }
    }
    drop(out);
    std::fs::rename(&part, &target).map_err(|e| format!("保存模型包失败: {}", e))?;
    Ok(target)
}

// ---------------------------------------------------------------------------
// 识别器
// ---------------------------------------------------------------------------

pub struct Recognizer {
    ptr: *const sys::OfflineRecognizer,
}

// SAFETY: sherpa-onnx 识别器对象可在线程间移动，本模块只在单线程内顺序调用。
unsafe impl Send for Recognizer {}

impl Recognizer {
    pub fn sense_voice(dir: &Path, threads: i32, language: &str) -> Result<Self, String> {
        Self::create(threads, |cfg, keep| {
            let model = keep(dir.join("sensevoice/model.int8.onnx"));
            let tokens = keep(dir.join("sensevoice/tokens.txt"));
            let lang = keep(PathBuf::from(language));
            cfg.model_config.sense_voice.model = model;
            cfg.model_config.sense_voice.language = lang;
            cfg.model_config.sense_voice.use_itn = 1;
            cfg.model_config.tokens = tokens;
        })
    }

    pub fn paraformer(dir: &Path, threads: i32) -> Result<Self, String> {
        Self::create(threads, |cfg, keep| {
            cfg.model_config.paraformer.model = keep(dir.join("paraformer/model.int8.onnx"));
            cfg.model_config.tokens = keep(dir.join("paraformer/tokens.txt"));
        })
    }

    fn create(
        threads: i32,
        fill: impl FnOnce(&mut sys::OfflineRecognizerConfig, &mut dyn FnMut(PathBuf) -> *const std::os::raw::c_char),
    ) -> Result<Self, String> {
        let mut strings: Vec<CString> = Vec::new();
        let mut keep = |p: PathBuf| -> *const std::os::raw::c_char {
            let c = CString::new(p.to_string_lossy().as_bytes()).unwrap_or_default();
            let ptr = c.as_ptr();
            strings.push(c);
            ptr
        };
        // SAFETY: 配置结构体为 repr(C) 的纯数据，零值即「未设置」，C 接口对空指针字段使用默认值。
        let mut cfg: sys::OfflineRecognizerConfig = unsafe { std::mem::zeroed() };
        cfg.feat_config.sample_rate = SAMPLE_RATE;
        cfg.feat_config.feature_dim = 80;
        cfg.model_config.num_threads = threads;
        cfg.model_config.provider = keep(PathBuf::from("cpu"));
        cfg.decoding_method = keep(PathBuf::from("greedy_search"));
        fill(&mut cfg, &mut keep);
        let ptr = unsafe { sys::SherpaOnnxCreateOfflineRecognizer(&cfg) };
        drop(strings);
        if ptr.is_null() {
            return Err("加载语音识别模型失败，请检查模型文件".to_string());
        }
        Ok(Self { ptr })
    }

    /// 识别一段 16k 单声道音频，返回 C 接口的完整结果 JSON
    pub fn decode(&self, samples: &[f32]) -> Option<Value> {
        if samples.is_empty() {
            return None;
        }
        // SAFETY: ptr 在 self 生命周期内有效；stream 与结果字符串在本函数内创建并释放。
        unsafe {
            let stream = sys::SherpaOnnxCreateOfflineStream(self.ptr);
            sys::SherpaOnnxAcceptWaveformOffline(stream, SAMPLE_RATE, samples.as_ptr(), samples.len() as i32);
            sys::SherpaOnnxDecodeOfflineStream(self.ptr, stream);
            let p = sys::SherpaOnnxGetOfflineStreamResultAsJson(stream);
            let json = if p.is_null() {
                None
            } else {
                let s = CStr::from_ptr(p).to_string_lossy().into_owned();
                sys::SherpaOnnxDestroyOfflineStreamResultJson(p);
                serde_json::from_str(&s).ok()
            };
            sys::SherpaOnnxDestroyOfflineStream(stream);
            json
        }
    }
}

impl Drop for Recognizer {
    fn drop(&mut self) {
        unsafe { sys::SherpaOnnxDestroyOfflineRecognizer(self.ptr) };
    }
}

fn tag(json: &Value, key: &str) -> String {
    json.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim_matches(|c| c == '<' || c == '>' || c == '|')
        .to_string()
}

// ---------------------------------------------------------------------------
// 整场转写
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AsrSegment {
    /// 场次时间轴上的起止秒数
    pub start: f64,
    pub end: f64,
    /// 所在录播文件序号及文件内起点，用于二次识别时重新取音频
    pub file_idx: usize,
    pub file_start: f64,
    pub lang: String,
    pub emotion: String,
    pub event: String,
    pub text: String,
    /// Paraformer 二次识别的中文文本
    #[serde(default)]
    pub zh_text: Option<String>,
    /// 声纹判定的主播说话秒数；为空时按语种判断（中文视为主播）
    #[serde(default)]
    pub streamer_secs: Option<f64>,
    /// 这句是主播在念观众的 SC（分析时按弹幕里的 SC 原文标注，不缓存）
    #[serde(skip)]
    pub sc: Option<ScRead>,
}

impl AsrSegment {
    pub fn is_streamer(&self) -> bool {
        self.streamer_seconds() * 2.0 >= self.end - self.start
    }

    pub fn streamer_seconds(&self) -> f64 {
        match self.streamer_secs {
            Some(secs) => secs,
            None if matches!(self.lang.as_str(), "zh" | "yue") => self.end - self.start,
            None => 0.0,
        }
    }

    pub fn best_text(&self) -> &str {
        self.zh_text.as_deref().filter(|t| !t.trim().is_empty()).unwrap_or(&self.text)
    }
}

/// 念 SC 的标注
#[derive(Clone, Debug)]
pub struct ScRead {
    /// 同一条 SC 的几句编号相同
    pub id: usize,
    /// 整句都在念 SC；否则是念 SC 夹着主播自己的话
    pub pure: bool,
    /// SC 原文；只认出“谢谢某某的 SC”、没对上原文时为空
    pub text: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FullPass {
    pub segments: Vec<AsrSegment>,
    /// 场次每秒的响度（dBFS），无音频处为 -90
    pub loudness: Vec<f32>,
}

pub struct AudioFile<'a> {
    pub path: &'a str,
    pub offset_start: f64,
}

fn spawn_pcm(path: &str, start: Option<f64>, duration: Option<f64>) -> Result<std::process::Child, String> {
    let mut cmd = Command::new(resolve_ffmpeg_path());
    apply_no_window(&mut cmd);
    cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin"]);
    if let Some(s) = start {
        cmd.args(["-ss", &format!("{:.3}", s.max(0.0))]);
    }
    cmd.args(["-i", path]);
    if let Some(d) = duration {
        cmd.args(["-t", &format!("{:.3}", d)]);
    }
    cmd.args(["-vn", "-ac", "1", "-ar", "16000", "-f", "s16le", "-"]);
    cmd.stdout(Stdio::piped()).stderr(Stdio::null());
    cmd.spawn().map_err(|e| format!("启动 ffmpeg 解码音频失败: {}", e))
}

fn pcm_to_f32(bytes: &[u8], out: &mut Vec<f32>) {
    out.extend(bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0));
}

fn new_vad(dir: &Path) -> Result<VoiceActivityDetector, String> {
    let cfg = VadModelConfig {
        silero_vad: SileroVadModelConfig {
            model: Some(dir.join("vad/silero_vad.onnx").to_string_lossy().to_string()),
            threshold: 0.5,
            min_silence_duration: 0.3,
            min_speech_duration: 0.25,
            window_size: 512,
            max_speech_duration: 20.0,
        },
        sample_rate: SAMPLE_RATE,
        num_threads: 1,
        ..Default::default()
    };
    VoiceActivityDetector::create(&cfg, 120.0).ok_or_else(|| "加载 VAD 模型失败".to_string())
}

/// 句末标点处断句；逗号类标点在句子够长时也断开；一直没有标点的超长句强制断开
const SENTENCE_END: &[&str] = &["。", "？", "！", "?", "!"];
const CLAUSE_END: &[&str] = &["，", ",", "；", ";", "：", ":"];
const CLAUSE_MIN_SECS: f64 = 6.0;
const SENTENCE_MAX_SECS: f64 = 15.0;

/// 按识别结果的逐字时间把一段语音拆成单句，返回（段内起点秒, 段内终点秒, 文本）。
/// 结果里没有逐字时间时整段作为一句。
pub fn split_sentences(json: &Value, dur: f64) -> Vec<(f64, f64, String)> {
    let text = json.get("text").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let tokens: Vec<&str> = json.get("tokens").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|t| t.as_str()).collect()).unwrap_or_default();
    let stamps: Vec<f64> = json.get("timestamps").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|t| t.as_f64()).collect()).unwrap_or_default();
    if tokens.is_empty() || tokens.len() != stamps.len() {
        return if text.is_empty() { Vec::new() } else { vec![(0.0, dur, text)] };
    }
    let mut out: Vec<(f64, f64, String)> = Vec::new();
    let mut buf = String::new();
    let mut start: Option<f64> = None;
    for (i, tok) in tokens.iter().enumerate() {
        let at = stamps[i].clamp(0.0, dur);
        let begin = *start.get_or_insert(at);
        // 英文子词以 ▁ 标记词首
        buf.push_str(&tok.replace('\u{2581}', " "));
        let next = stamps.get(i + 1).copied().unwrap_or(dur).clamp(0.0, dur);
        let long = next - begin;
        let cut = SENTENCE_END.contains(tok) || (CLAUSE_END.contains(tok) && long >= CLAUSE_MIN_SECS) || long >= SENTENCE_MAX_SECS;
        if cut || i + 1 == tokens.len() {
            let t = buf.trim().to_string();
            if !t.is_empty() {
                out.push((begin, if i + 1 == tokens.len() { dur } else { next }, t));
            }
            buf.clear();
            start = None;
        }
    }
    out
}

/// 对整场每个文件解码音频：VAD 分段后 SenseVoice 识别并按逐字时间拆成单句，同时计算每秒响度。
/// `session_secs` 为场次总时长；`on_progress` 回报已处理的场次秒数。
pub fn full_pass(
    model_dir: &Path,
    threads: i32,
    files: &[AudioFile],
    session_secs: f64,
    cancel: &dyn Fn() -> bool,
    on_progress: &mut dyn FnMut(f64),
) -> Result<FullPass, String> {
    let recognizer = Recognizer::sense_voice(model_dir, threads, "auto")?;
    let mut pass = FullPass {
        segments: Vec::new(),
        loudness: vec![-90.0; session_secs.ceil().max(1.0) as usize + 1],
    };
    let sr = SAMPLE_RATE as usize;
    for (file_idx, file) in files.iter().enumerate() {
        let vad = new_vad(model_dir)?;
        let mut child = spawn_pcm(file.path, None, None)?;
        let mut stdout = child.stdout.take().ok_or("无法读取 ffmpeg 输出")?;
        let mut raw = vec![0u8; sr * 2 * 10];
        let mut carry: Vec<u8> = Vec::new();
        let mut samples: Vec<f32> = Vec::new();
        let mut pending: Vec<f32> = Vec::new();
        let mut consumed: usize = 0; // 本文件已送入 VAD 的采样数
        let mut sec_acc = (0usize, 0f64); // (当前秒内采样数, 平方和)
        let drain = |vad: &VoiceActivityDetector, pass: &mut FullPass| {
            while !vad.is_empty() {
                if let Some(seg) = vad.front() {
                    let file_start = seg.start() as f64 / SAMPLE_RATE as f64;
                    let audio = seg.samples();
                    let dur = audio.len() as f64 / SAMPLE_RATE as f64;
                    if let Some(json) = recognizer.decode(audio) {
                        // 一段语音可能长达几十秒、混着主播与原片：按逐字时间拆成单句，每句单独判定说话人
                        for (from, to, text) in split_sentences(&json, dur) {
                            pass.segments.push(AsrSegment {
                                start: file.offset_start + file_start + from,
                                end: file.offset_start + file_start + to,
                                file_idx,
                                file_start: file_start + from,
                                lang: tag(&json, "lang"),
                                emotion: tag(&json, "emotion"),
                                event: tag(&json, "event"),
                                text,
                                zh_text: None,
                                streamer_secs: None,
                                sc: None,
                            });
                        }
                    }
                }
                vad.pop();
            }
        };
        loop {
            if cancel() {
                let _ = child.kill();
                return Err("任务已取消".to_string());
            }
            let n = stdout.read(&mut raw).map_err(|e| format!("读取音频失败: {}", e))?;
            if n == 0 {
                break;
            }
            carry.extend_from_slice(&raw[..n]);
            let usable = carry.len() / 2 * 2;
            samples.clear();
            pcm_to_f32(&carry[..usable], &mut samples);
            carry.drain(..usable);
            for &x in &samples {
                sec_acc.0 += 1;
                sec_acc.1 += (x as f64) * (x as f64);
                if sec_acc.0 == sr {
                    let sec = file.offset_start as usize + (consumed + pending.len() + 1) / sr;
                    if let Some(slot) = pass.loudness.get_mut(sec) {
                        *slot = (10.0 * (sec_acc.1 / sr as f64 + 1e-10).log10()) as f32;
                    }
                    sec_acc = (0, 0.0);
                }
                pending.push(x);
            }
            let whole = pending.len() / 512 * 512;
            for chunk in pending[..whole].chunks(512) {
                vad.accept_waveform(chunk);
            }
            consumed += whole;
            pending.drain(..whole);
            drain(&vad, &mut pass);
            on_progress(file.offset_start + consumed as f64 / SAMPLE_RATE as f64);
        }
        if !pending.is_empty() {
            vad.accept_waveform(&pending);
        }
        vad.flush();
        drain(&vad, &mut pass);
        let _ = child.wait();
    }
    pass.segments.sort_by(|a, b| a.start.total_cmp(&b.start));
    Ok(pass)
}

/// 对指定的中文分段用 Paraformer 重新识别，结果写入 `zh_text`。
/// 同一文件内相距不超过 30 秒的分段合并为一次解码，避免频繁启动 ffmpeg。
pub fn refine_chinese(
    model_dir: &Path,
    threads: i32,
    files: &[AudioFile],
    segments: &mut [AsrSegment],
    targets: &[usize],
) -> Result<(), String> {
    if targets.is_empty() {
        return Ok(());
    }
    let recognizer = Recognizer::paraformer(model_dir, threads)?;
    let mut texts = Vec::new();
    for_each_segment_audio(files, segments, targets, |i, audio| {
        if let Some(json) = recognizer.decode(audio) {
            let text = json.get("text").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            if !text.is_empty() {
                texts.push((i, text));
            }
        }
    })?;
    for (i, text) in texts {
        segments[i].zh_text = Some(text);
    }
    Ok(())
}

/// 录播旁的同名 .srt 当作转写：每段录播都有 SRT 时返回整场转写，否则返回 None（照常语音识别）。
/// SRT 只有文字和时间：语种按中文处理（未登记声纹时全部视为主播），没有情绪、笑声和响度；
/// 外部字幕常按半秒到一秒断句，相邻间隔很短的句子合并成整句，便于声纹判定和大模型阅读
pub fn srt_pass(files: &[AudioFile]) -> Option<FullPass> {
    let mut segments: Vec<AsrSegment> = Vec::new();
    for (file_idx, file) in files.iter().enumerate() {
        let text = std::fs::read_to_string(Path::new(file.path).with_extension("srt")).ok()?;
        // SRT 每块时间戳本来就是精确到句的，原样直通：评阅页直接用真实时间，
        // 不做合并——合并后前端只能按字数比例估时，语速不均时字幕严重错位
        for (start, end, line) in parse_srt(&text) {
            segments.push(AsrSegment {
                start: file.offset_start + start,
                end: file.offset_start + end,
                file_idx,
                file_start: start,
                lang: "zh".into(),
                emotion: String::new(),
                event: "Speech".into(),
                text: line.clone(),
                zh_text: Some(line),
                streamer_secs: None,
                sc: None,
            });
        }
    }
    if segments.is_empty() {
        return None;
    }
    segments.sort_by(|a, b| a.start.total_cmp(&b.start));
    Some(FullPass { segments, loudness: Vec::new() })
}

/// 解析 SRT：返回 (起点秒, 终点秒, 文本)，跳过格式不对或没有文字的块
fn parse_srt(text: &str) -> Vec<(f64, f64, String)> {
    let secs = |t: &str| {
        let t = t.trim().replace(',', ".");
        let parts: Vec<&str> = t.split(':').collect();
        if parts.len() != 3 {
            return None;
        }
        Some(parts[0].parse::<f64>().ok()? * 3600.0 + parts[1].parse::<f64>().ok()? * 60.0 + parts[2].parse::<f64>().ok()?)
    };
    let mut out = Vec::new();
    for block in text.trim_start_matches('\u{feff}').replace("\r\n", "\n").split("\n\n") {
        let mut lines = block.lines().filter(|l| !l.trim().is_empty());
        let Some(time_line) = lines.find(|l| l.contains("-->")) else { continue };
        let Some((a, b)) = time_line.split_once("-->") else { continue };
        let (Some(start), Some(end)) = (secs(a), secs(b)) else { continue };
        let body: Vec<&str> = lines.map(str::trim).collect();
        let body = body.join(" ");
        if end > start && !body.is_empty() {
            out.push((start, end, body));
        }
    }
    out
}

const PUNCT_MODEL: &str = "punct/model.onnx";

/// 给 Paraformer 结果加标点（模型缺失时不处理）。返回是否加过
pub fn punctuate(model_dir: &Path, texts: &mut [Option<String>]) -> bool {
    let path = model_dir.join(PUNCT_MODEL);
    if !path.is_file() {
        return false;
    }
    let config = OfflinePunctuationConfig {
        model: OfflinePunctuationModelConfig { ct_transformer: Some(path.to_string_lossy().to_string()), num_threads: 2, ..Default::default() },
    };
    let Some(punct) = OfflinePunctuation::create(&config) else { return false };
    for t in texts.iter_mut().flatten() {
        if let Some(p) = punct.add_punctuation(t) {
            *t = p;
        }
    }
    true
}

/// 解码文件中一段音频为 16kHz 单声道
pub fn decode_range(path: &str, start: f64, duration: f64) -> Result<Vec<f32>, String> {
    let mut child = spawn_pcm(path, Some(start), Some(duration))?;
    let mut bytes = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        out.read_to_end(&mut bytes).map_err(|e| format!("读取音频失败: {}", e))?;
    }
    let _ = child.wait();
    let mut audio = Vec::new();
    pcm_to_f32(&bytes, &mut audio);
    Ok(audio)
}

/// 按文件位置把相邻分段合并解码，逐个回调分段下标与其音频（16kHz 单声道）
pub fn for_each_segment_audio(
    files: &[AudioFile],
    segments: &[AsrSegment],
    targets: &[usize],
    mut f: impl FnMut(usize, &[f32]),
) -> Result<(), String> {
    let mut order = targets.to_vec();
    order.sort_by(|&a, &b| {
        (segments[a].file_idx, segments[a].file_start)
            .partial_cmp(&(segments[b].file_idx, segments[b].file_start))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for i in order {
        let joinable = groups.last().and_then(|g| g.last()).map(|&last| {
            segments[last].file_idx == segments[i].file_idx
                && segments[i].file_start - (segments[last].file_start + segments[last].end - segments[last].start) < 30.0
        });
        match joinable {
            Some(true) => groups.last_mut().unwrap().push(i),
            _ => groups.push(vec![i]),
        }
    }
    for group in groups {
        let first = &segments[group[0]];
        let last = &segments[*group.last().unwrap()];
        let Some(file) = files.get(first.file_idx) else { continue };
        let span_start = first.file_start;
        let span_end = last.file_start + (last.end - last.start);
        let audio = decode_range(file.path, span_start, span_end - span_start + 0.5)?;
        for &i in &group {
            let seg = &segments[i];
            let from = ((seg.file_start - span_start) * SAMPLE_RATE as f64) as usize;
            let to = (from + ((seg.end - seg.start) * SAMPLE_RATE as f64) as usize).min(audio.len());
            if from < to {
                f(i, &audio[from..to]);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srt_pass_keeps_each_cue_with_its_own_timestamps() {
        let dir = std::env::temp_dir().join(format!("srt_pass_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let video = dir.join("a.mp4");
        std::fs::write(
            video.with_extension("srt"),
            "\u{feff}1\r\n00:00:12,220 --> 00:00:12,820\r\n中午好\r\n\r\n2\r\n00:00:12,820 --> 00:00:13,300\r\n你们吃了吗\r\n\r\n3\r\n00:00:16,540 --> 00:00:18,530\r\n主包还没吃\r\n",
        )
        .unwrap();
        let path = video.to_string_lossy().to_string();
        let pass = srt_pass(&[AudioFile { path: &path, offset_start: 100.0 }]).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        // 每条 SRT 字幕单独成段，时间用原始值，序号不混进文本
        assert_eq!(pass.segments.len(), 3);
        assert_eq!(pass.segments[0].text, "中午好");
        assert!((pass.segments[0].start - 112.22).abs() < 1e-6 && (pass.segments[0].end - 112.82).abs() < 1e-6);
        assert_eq!(pass.segments[1].text, "你们吃了吗");
        assert!((pass.segments[1].start - 112.82).abs() < 1e-6 && (pass.segments[1].end - 113.3).abs() < 1e-6);
        assert_eq!(pass.segments[2].zh_text.as_deref(), Some("主包还没吃"));
        assert!(!pass.segments.iter().any(|s| s.text.chars().next().is_some_and(|c| c.is_ascii_digit())));
    }

    #[test]
    fn splits_long_segment_into_sentences_by_token_time() {
        let json = serde_json::json!({
            "text": "大家好。今天看大唐，我们开始吧",
            "tokens": ["大", "家", "好", "。", "今", "天", "看", "大", "唐", "，", "我", "们", "开", "始", "吧"],
            "timestamps": [0.1, 0.3, 0.5, 0.7, 2.0, 2.2, 2.4, 2.6, 2.8, 3.0, 3.2, 3.4, 3.6, 3.8, 4.0]
        });
        let s = split_sentences(&json, 10.5);
        assert_eq!(s.len(), 2, "{:?}", s);
        assert_eq!(s[0], (0.1, 2.0, "大家好。".to_string()));
        // 逗号处句子不足 6 秒不断开，最后一句延到段尾
        assert_eq!(s[1].0, 2.0);
        assert_eq!(s[1].1, 10.5);
        assert_eq!(s[1].2, "今天看大唐，我们开始吧");
    }

    /// 构造一个最小模型包，用于验证导入解压与校验流程
    fn write_test_package(dir: &Path, files: &[(&str, &[u8])]) -> PathBuf {
        use std::io::Write as _;
        let manifest_files: Vec<serde_json::Value> = files
            .iter()
            .map(|(name, data)| {
                let mut hasher = Sha256::new();
                hasher.update(data);
                let digest: String = hasher.finalize().iter().map(|b| format!("{:02x}", b)).collect();
                serde_json::json!({
                    "path": format!("asr/{}", name),
                    "size": data.len(),
                    "sha256": digest,
                })
            })
            .collect();
        let manifest = serde_json::json!({
            "schema": MODEL_PACKAGE_SCHEMA,
            "version": "test",
            "files": manifest_files,
        });
        let zip_path = dir.join("models.zip");
        let file = std::fs::File::create(&zip_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::FileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("manifest.json", options).unwrap();
        zip.write_all(manifest.to_string().as_bytes()).unwrap();
        for (name, data) in files {
            zip.start_file(format!("asr/{}", name), options).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
        zip_path
    }

    // 三个导入测试共用一个全局进度单例，用串行锁避免并行互相干扰
    fn import_test_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn imports_model_package_into_target_dir() {
        let _guard = import_test_guard();
        let root = std::env::temp_dir().join(format!("asr_import_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let target = root.join("models/asr");
        std::fs::create_dir_all(&target).unwrap();
        let zip_path = write_test_package(&root, &[("vad/silero_vad.onnx", b"fake-vad")]);
        let message = import_model_package(&target, &zip_path).unwrap();
        assert!(message.contains("test"), "{}", message);
        assert_eq!(std::fs::read(target.join("vad/silero_vad.onnx")).unwrap(), b"fake-vad");
        // 临时目录应被清理
        let leftovers: Vec<_> = std::fs::read_dir(&target)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".importing-"))
            .collect();
        assert!(leftovers.is_empty(), "残留临时目录");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rejects_package_with_mismatched_checksum() {
        let _guard = import_test_guard();
        let root = std::env::temp_dir().join(format!("asr_import_bad_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let target = root.join("models/asr");
        std::fs::create_dir_all(&target).unwrap();
        let zip_path = write_test_package(&root, &[("vad/silero_vad.onnx", b"fake-vad")]);
        // 篡改包内文件，破坏 sha256
        {
            use std::io::Write as _;
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            let manifest = serde_json::json!({
                "schema": MODEL_PACKAGE_SCHEMA,
                "version": "test",
                "files": [{ "path": "asr/vad/silero_vad.onnx", "size": 8, "sha256": "deadbeef" }],
            });
            zip.start_file("manifest.json", options).unwrap();
            zip.write_all(manifest.to_string().as_bytes()).unwrap();
            zip.start_file("asr/vad/silero_vad.onnx", options).unwrap();
            zip.write_all(b"fake-vad").unwrap();
            zip.finish().unwrap();
        }
        let err = import_model_package(&target, &zip_path).unwrap_err();
        assert!(err.contains("校验失败"), "{}", err);
        assert!(!target.join("vad/silero_vad.onnx").exists(), "不应留下文件");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn install_replaces_whole_directory_without_mixing() {
        let _guard = import_test_guard();
        let root = std::env::temp_dir().join(format!("asr_replace_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let target = root.join("models/asr");
        std::fs::create_dir_all(target.join("vad")).unwrap();
        // 旧模型：内容为 old
        std::fs::write(target.join("vad/silero_vad.onnx"), b"old").unwrap();
        std::fs::write(target.join("stale.onnx"), b"stale").unwrap();

        let zip_path = write_test_package(&root, &[("vad/silero_vad.onnx", b"new")]);
        import_model_package(&target, &zip_path).unwrap();

        // 新文件已就位，旧的多余文件随整目录替换被清掉
        assert_eq!(std::fs::read(target.join("vad/silero_vad.onnx")).unwrap(), b"new");
        assert!(!target.join("stale.onnx").exists(), "旧文件应被整体替换掉");
        // 不留备份或临时目录
        let leftovers: Vec<String> = std::fs::read_dir(&root.join("models"))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with('.'))
            .collect();
        assert!(leftovers.is_empty(), "残留临时目录: {:?}", leftovers);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rejects_unsafe_package_path() {
        assert!(validate_package_path("../../etc/passwd").is_err());
        assert!(validate_package_path("/abs/path").is_err());
        assert!(validate_package_path("C:/win").is_err());
        assert!(validate_package_path("a\\b").is_err());
        assert!(validate_package_path("asr/vad/x.onnx").is_ok());
    }
}
