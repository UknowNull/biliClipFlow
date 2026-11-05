#!/usr/bin/env node
// CI 用：从 hf-mirror 下载切片分析所需的全部模型到指定目录。
// 与 src-tauri/src/highlight/asr.rs 的 MODEL_FILES 保持同步。

import { createHash } from "node:crypto";
import { createWriteStream } from "node:fs";
import fs from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";

const BASE = process.env.MODEL_SOURCE_BASE || "https://hf-mirror.com";

const FILES = [
  {
    path: "sensevoice/model.int8.onnx",
    remote: "csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/main/model.int8.onnx",
    sha256: "c71f0ce00bec95b07744e116345e33d8cbbe08cef896382cf907bf4b51a2cd51",
  },
  {
    path: "sensevoice/tokens.txt",
    remote: "csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/main/tokens.txt",
    sha256: "f449eb28dc567533d7fa59be34e2abca8784f771850c78a47fb731a31429a1dc",
  },
  {
    path: "paraformer/model.int8.onnx",
    remote: "csukuangfj/sherpa-onnx-paraformer-zh-2023-09-14/resolve/main/model.int8.onnx",
    sha256: "f36a0433bcf096bd6d6f11b80a3ac8bed110bdca632fe0d731df8d1a84475945",
  },
  {
    path: "paraformer/tokens.txt",
    remote: "csukuangfj/sherpa-onnx-paraformer-zh-2023-09-14/resolve/main/tokens.txt",
    sha256: "59aba8873a2ed1e122c25fee421e25f283b63290efbde85c1f01a853d83cb6e6",
  },
  {
    path: "vad/silero_vad.onnx",
    remote: "csukuangfj/vad/resolve/main/silero_vad.onnx",
    sha256: null, // 该文件历史上有多份导出，跳过固定校验
  },
  {
    path: "punct/model.onnx",
    remote:
      "csukuangfj/sherpa-onnx-punct-ct-transformer-zh-en-vocab272727-2024-04-12/resolve/main/model.onnx",
    sha256: "e93593a6dbd69a07f8734ef269dbe861a379755f8d1c8354719432116f2c44bd",
  },
  {
    path: "speaker/3dspeaker_speech_campplus_sv_zh-cn_16k-common.onnx",
    remote:
      "csukuangfj/speaker-embedding-models/resolve/main/3dspeaker_speech_campplus_sv_zh-cn_16k-common.onnx",
    sha256: "f682b514c05d947ee3fa91cd6ec6c5c7543479a128373fa29b1faedccd21fd11",
  },
];

async function download(url, target) {
  const response = await fetch(url, { redirect: "follow" });
  if (!response.ok || !response.body) {
    throw new Error(`下载失败 ${url}: HTTP ${response.status}`);
  }
  await fs.mkdir(path.dirname(target), { recursive: true });
  await pipeline(Readable.fromWeb(response.body), createWriteStream(target));
}

async function sha256File(filePath) {
  const hash = createHash("sha256");
  const handle = await fs.open(filePath, "r");
  try {
    const stream = handle.createReadStream();
    for await (const chunk of stream) hash.update(chunk);
  } finally {
    await handle.close();
  }
  return hash.digest("hex");
}

async function main() {
  const outDir = path.resolve(process.argv[2] || "src-tauri/models/asr");
  console.log(`模型输出目录：${outDir}`);
  for (const file of FILES) {
    const target = path.join(outDir, file.path);
    try {
      const stat = await fs.stat(target);
      if (stat.size > 0) {
        console.log(`已存在，跳过：${file.path}`);
        continue;
      }
    } catch {
      // 不存在，继续下载
    }
    const url = `${BASE}/${file.remote}`;
    console.log(`下载 ${file.path} ...`);
    await download(url, target);
    if (file.sha256) {
      const digest = await sha256File(target);
      if (digest !== file.sha256) {
        await fs.rm(target, { force: true });
        throw new Error(`校验失败 ${file.path}\n  期望 ${file.sha256}\n  实际 ${digest}`);
      }
      console.log(`  校验通过`);
    }
  }
  console.log("全部模型准备完成");
}

main().catch((err) => {
  console.error(err.message || err);
  process.exitCode = 1;
});
