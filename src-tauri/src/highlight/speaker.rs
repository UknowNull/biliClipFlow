//! 声纹区分主播解说与原片人声：原片也是中文时，按语种无法区分两者。
//!
//! 主播声纹由用户在一段只有主播说话的录播上登记（按房间保存），切片时按相似度判定每句中主播说话的时长。
//! 整场追剧时原片主角出场比主播说话还多，无法自动认出主播，所以必须登记。

use std::path::Path;

use sherpa_onnx::{SpeakerEmbeddingExtractor, SpeakerEmbeddingExtractorConfig};

use super::asr::{for_each_segment_audio, AsrSegment, AudioFile, SAMPLE_RATE};

pub const MODEL_REL: &str = "speaker/3dspeaker_speech_campplus_sv_zh-cn_16k-common.onnx";

const CHUNK_SECS: f64 = 2.0;
const MIN_CHUNK_SECS: f64 = 1.0;
/// 与主播声纹的余弦相似度不低于该值视为主播。主播叠在原片声音上说话时约 0.45~0.5，原片对白多在 0.3 以下
const STREAMER_SIM: f32 = 0.4;
/// 登记时低于该均方根音量的块视为静音丢弃
const SILENCE_RMS: f32 = 0.01;
const MIN_ENROLL_CHUNKS: usize = 8;

pub fn model_ready(model_dir: &Path) -> bool {
    std::fs::metadata(model_dir.join(MODEL_REL)).map(|m| m.len() > 0).unwrap_or(false)
}

fn extractor(model_dir: &Path, threads: i32) -> Result<SpeakerEmbeddingExtractor, String> {
    SpeakerEmbeddingExtractor::create(&SpeakerEmbeddingExtractorConfig {
        model: Some(model_dir.join(MODEL_REL).to_string_lossy().to_string()),
        num_threads: threads,
        ..Default::default()
    })
    .ok_or_else(|| "加载声纹模型失败".to_string())
}

/// 音频按 2 秒切块，返回每块的单位化声纹与时长
fn embed_chunks(ex: &SpeakerEmbeddingExtractor, audio: &[f32]) -> Vec<(Vec<f32>, f64, f32)> {
    let chunk_len = (CHUNK_SECS * SAMPLE_RATE as f64) as usize;
    let min_len = (MIN_CHUNK_SECS * SAMPLE_RATE as f64) as usize;
    audio
        .chunks(chunk_len)
        .filter(|piece| piece.len() >= min_len)
        .filter_map(|piece| {
            let stream = ex.create_stream()?;
            stream.accept_waveform(SAMPLE_RATE, piece);
            stream.input_finished();
            if !ex.is_ready(&stream) {
                return None;
            }
            let rms = (piece.iter().map(|x| x * x).sum::<f32>() / piece.len() as f32).sqrt();
            Some((normalize(ex.compute(&stream)?), piece.len() as f64 / SAMPLE_RATE as f64, rms))
        })
        .collect()
}

fn normalize(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
    v.iter_mut().for_each(|x| *x /= n);
    v
}

fn mean(vs: &[&Vec<f32>]) -> Vec<f32> {
    let mut m = vec![0.0f32; vs.first().map(|v| v.len()).unwrap_or(0)];
    for v in vs {
        m.iter_mut().zip(v.iter()).for_each(|(a, b)| *a += b);
    }
    normalize(m)
}

fn similarity(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// 由一段只有主播说话的音频生成主播声纹：去掉静音块，再剔除与均值明显不像的块（零星杂音）后取平均
pub fn enroll(model_dir: &Path, threads: i32, audio: &[f32]) -> Result<Vec<f32>, String> {
    let ex = extractor(model_dir, threads)?;
    let chunks = embed_chunks(&ex, audio);
    let voiced: Vec<&Vec<f32>> = chunks.iter().filter(|c| c.2 >= SILENCE_RMS).map(|c| &c.0).collect();
    let first = mean(&voiced);
    let kept: Vec<&Vec<f32>> = voiced.into_iter().filter(|e| similarity(e, &first) >= STREAMER_SIM).collect();
    if kept.len() < MIN_ENROLL_CHUNKS {
        return Err(format!("有效人声只有 {} 秒，请选择主播连续说话的片段", kept.len() * CHUNK_SECS as usize));
    }
    Ok(mean(&kept))
}

/// 给目标分段写入主播说话秒数
pub fn label_streamer(model_dir: &Path, threads: i32, files: &[AudioFile], segments: &mut [AsrSegment], targets: &[usize], voiceprint: &[f32]) -> Result<(), String> {
    let ex = extractor(model_dir, threads)?;
    let mut secs = Vec::new();
    for_each_segment_audio(files, segments, targets, |i, audio| {
        let s: f64 = embed_chunks(&ex, audio).iter().filter(|c| similarity(&c.0, voiceprint) >= STREAMER_SIM).map(|c| c.1).sum();
        secs.push((i, s));
    })?;
    for (i, s) in secs {
        segments[i].streamer_secs = Some(s);
    }
    Ok(())
}
