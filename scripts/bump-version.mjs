import fs from "node:fs";
import path from "node:path";

const root = process.cwd();
const fallbackVersion = "1.0.0";
const rawInput = resolveRawInput(process.argv.slice(2));

if (!rawInput) {
  console.warn("未提供版本号，默认使用 1.0.0");
}

const targetVersion = normalizeSemverVersion(rawInput || fallbackVersion);

function resolveRawInput(argv) {
  for (const value of argv) {
    const trimmed = String(value || "").trim();
    if (!trimmed || trimmed === "--") {
      continue;
    }
    return trimmed;
  }
  return "";
}

const packageJsonPath = path.join(root, "package.json");
const tauriConfPath = path.join(root, "src-tauri", "tauri.conf.json");
const cargoTomlPath = path.join(root, "src-tauri", "Cargo.toml");

function normalizeSemverVersion(input) {
  let value = String(input || "").trim();
  if (!value) {
    throw new Error("版本号不能为空");
  }
  value = value.replace(/^refs\/tags\//i, "");
  value = value.replace(/^[vV]/, "");
  const semverPattern =
    /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/;
  if (!semverPattern.test(value)) {
    throw new Error(`版本号不合法（需要 semver）：${input}`);
  }
  return value;
}

function readJson(filePath) {
  return JSON.parse(fs.readFileSync(filePath, "utf8"));
}

function writeJson(filePath, data) {
  fs.writeFileSync(filePath, `${JSON.stringify(data, null, 2)}\n`);
}

function updateCargoTomlVersion(tomlText, version) {
  const packageSectionPattern = /(\[package\][\s\S]*?)(?=\n\[|$)/;
  const sectionMatch = tomlText.match(packageSectionPattern);
  if (!sectionMatch) {
    throw new Error("Cargo.toml 缺少 [package] 段落");
  }
  const section = sectionMatch[1];
  const versionPattern = /^\s*version\s*=\s*\"[^\"]+\"\s*$/m;
  if (!versionPattern.test(section)) {
    throw new Error("Cargo.toml 未找到 version 字段");
  }
  const updatedSection = section.replace(versionPattern, `version = \"${version}\"`);
  return tomlText.replace(packageSectionPattern, updatedSection);
}

const packageJson = readJson(packageJsonPath);
packageJson.version = targetVersion;
writeJson(packageJsonPath, packageJson);

const tauriConf = readJson(tauriConfPath);
tauriConf.version = targetVersion;
writeJson(tauriConfPath, tauriConf);

const cargoToml = fs.readFileSync(cargoTomlPath, "utf8");
const updatedCargoToml = updateCargoTomlVersion(cargoToml, targetVersion);
fs.writeFileSync(cargoTomlPath, updatedCargoToml);

console.log(`版本已同步为: ${targetVersion}`);
