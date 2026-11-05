#!/usr/bin/env node
// 生成语音模型压缩包：把 asr 模型目录打成带 manifest 的 zip，供设置页导入。
//
// 用法：
//   node scripts/build-model-package.mjs                    # 从默认模型目录打包
//   node scripts/build-model-package.mjs --src <dir>        # 指定模型目录
//   node scripts/build-model-package.mjs --out <file.zip>   # 指定输出文件
//   node scripts/build-model-package.mjs --version 1.0.0    # 指定模型包版本

import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { execFile } from "node:child_process";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);

// 切片分析用到的全部模型（与 src-tauri/src/highlight/asr.rs 的 MODEL_FILES 保持一致）
const MODEL_FILES = [
  "sensevoice/model.int8.onnx",
  "sensevoice/tokens.txt",
  "paraformer/model.int8.onnx",
  "paraformer/tokens.txt",
  "vad/silero_vad.onnx",
  "punct/model.onnx",
  "speaker/3dspeaker_speech_campplus_sv_zh-cn_16k-common.onnx",
];

const DEFAULT_SRC = path.join(
  os.homedir(),
  "Library",
  "Application Support",
  "com.tbw.biliclipflow",
  "models",
  "asr",
);

function parseArgs(argv) {
  const options = { src: DEFAULT_SRC, out: null, version: "1.0.0" };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--src") options.src = argv[++i];
    else if (arg === "--out") options.out = argv[++i];
    else if (arg === "--version") options.version = argv[++i];
    else if (arg === "--help" || arg === "-h") options.help = true;
  }
  return options;
}

async function sha256File(filePath) {
  const hash = createHash("sha256");
  await new Promise((resolve, reject) => {
    createReadStream(filePath)
      .on("data", (chunk) => hash.update(chunk))
      .on("end", resolve)
      .on("error", reject);
  });
  return hash.digest("hex");
}

async function main() {
  const options = parseArgs(process.argv.slice(2));
  if (options.help) {
    console.log(
      [
        "生成语音模型压缩包",
        "",
        "  node scripts/build-model-package.mjs [选项]",
        "",
        "选项：",
        `  --src <dir>        模型目录（默认 ${DEFAULT_SRC}）`,
        "  --out <file.zip>   输出文件（默认 dist/biliClipFlow-models-<version>.zip）",
        "  --version <ver>    模型包版本（默认 1.0.0）",
      ].join("\n"),
    );
    return;
  }

  const srcDir = path.resolve(options.src);
  const outFile = path.resolve(
    options.out || path.join("dist", `biliClipFlow-models-${options.version}.zip`),
  );

  const missing = [];
  const entries = [];
  for (const rel of MODEL_FILES) {
    const abs = path.join(srcDir, rel);
    let stat;
    try {
      stat = await fs.stat(abs);
    } catch {
      missing.push(rel);
      continue;
    }
    if (!stat.isFile() || stat.size === 0) {
      missing.push(rel);
      continue;
    }
    process.stdout.write(`校验 ${rel} ... `);
    const sha256 = await sha256File(abs);
    console.log(`${(stat.size / 1024 / 1024).toFixed(1)} MB`);
    entries.push({ path: `asr/${rel}`, size: stat.size, sha256 });
  }

  if (missing.length) {
    console.error("\n缺少以下模型文件，无法打包：");
    for (const rel of missing) console.error(`  ${rel}`);
    console.error(`\n模型目录：${srcDir}`);
    process.exitCode = 1;
    return;
  }

  const totalBytes = entries.reduce((sum, item) => sum + item.size, 0);
  const manifest = {
    schema: "biliclipflow-model-package",
    version: options.version,
    createdAt: new Date().toISOString(),
    totalBytes,
    files: entries,
  };

  const stagingDir = await fs.mkdtemp(path.join(os.tmpdir(), "bcf-models-"));
  try {
    await fs.mkdir(path.join(stagingDir, "asr"), { recursive: true });
    for (const item of entries) {
      const from = path.join(srcDir, item.path.replace(/^asr\//, ""));
      const to = path.join(stagingDir, item.path);
      await fs.mkdir(path.dirname(to), { recursive: true });
      await fs.copyFile(from, to);
    }
    await fs.writeFile(
      path.join(stagingDir, "manifest.json"),
      JSON.stringify(manifest, null, 2),
      "utf8",
    );

    await fs.mkdir(path.dirname(outFile), { recursive: true });
    await fs.rm(outFile, { force: true });
    // 用系统 zip，保证与运行时 zip 0.6 解压兼容（store 模式，主程序体积已由安装包压缩）
    await execFileAsync("zip", ["-r", "-0", "-q", outFile, "."], { cwd: stagingDir });
  } finally {
    await fs.rm(stagingDir, { recursive: true, force: true });
  }

  const packStat = await fs.stat(outFile);
  console.log("");
  console.log(`已生成：${outFile}`);
  console.log(`  版本：${manifest.version}`);
  console.log(`  文件：${entries.length} 个`);
  console.log(`  原始：${(totalBytes / 1024 / 1024).toFixed(1)} MB`);
  console.log(`  压缩包：${(packStat.size / 1024 / 1024).toFixed(1)} MB`);
}

main().catch((err) => {
  console.error(err);
  process.exitCode = 1;
});
