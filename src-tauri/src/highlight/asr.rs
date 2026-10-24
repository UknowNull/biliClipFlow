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
use sherpa_onnx::{SileroVadModelConfig, VadModelConfig, VoiceActivityDetector};
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

/// 语音识别必需的模型是否就绪（声纹模型可选，不计入）
pub fn models_ready(dir: &Path) -> bool {
    model_status(dir).iter().filter(|s| s.path != super::speaker::MODEL_REL).all(|s| s.ready)
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
        let mut req = client.get(&url);
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

/// 对整场每个文件解码音频：VAD 分段后 SenseVoice 识别，同时计算每秒响度。
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
                        let text = json.get("text").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                        if !text.is_empty() {
                            pass.segments.push(AsrSegment {
                                start: file.offset_start + file_start,
                                end: file.offset_start + file_start + dur,
                                file_idx,
                                file_start,
                                lang: tag(&json, "lang"),
                                emotion: tag(&json, "emotion"),
                                event: tag(&json, "event"),
                                text,
                                zh_text: None,
                                streamer_secs: None,
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
