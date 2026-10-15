#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

if ! command -v pnpm >/dev/null 2>&1; then
  echo "错误: 未找到 pnpm，请先安装: npm install -g pnpm" >&2
  exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "错误: 未找到 cargo，请先安装 Rust: https://rustup.rs" >&2
  exit 1
fi

if [ ! -d node_modules ]; then
  echo "首次运行，安装依赖..."
  pnpm install
fi

echo "启动开发模式 (pnpm tauri dev)..."
exec pnpm tauri dev
