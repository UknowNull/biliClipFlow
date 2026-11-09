#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"
PORT=5176

if ! command -v pnpm >/dev/null 2>&1; then
  echo "错误: 未找到 pnpm，请先安装: npm install -g pnpm" >&2
  exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "错误: 未找到 cargo，请先安装 Rust: https://rustup.rs" >&2
  exit 1
fi

# ---------- 清理上一次残留进程 ----------
kill_pids() {
  label="$1"
  shift
  [ "$#" -eq 0 ] && return 0
  echo "停止${label}: $*"
  kill "$@" 2>/dev/null || true
  i=0
  while [ "$i" -lt 20 ]; do
    alive=""
    for pid in "$@"; do
      if kill -0 "$pid" 2>/dev/null; then
        alive="$alive $pid"
      fi
    done
    [ -z "$alive" ] && return 0
    sleep 0.5
    i=$((i + 1))
  done
  echo "强制终止${label}:${alive}"
  # shellcheck disable=SC2086
  kill -9 ${alive} 2>/dev/null || true
}

# 1) 本项目的 tauri dev / vite / app 进程（只杀属于当前项目目录的）
OLD_PIDS=""
for pid in $(pgrep -f "tauri dev" 2>/dev/null || true) \
           $(pgrep -f "biliClipFlow/node_modules/\.bin/\.\./vite" 2>/dev/null || true) \
           $(pgrep -f "target/debug/bili-clip-flow" 2>/dev/null || true) \
           $(pgrep -f "target/release/bili-clip-flow" 2>/dev/null || true); do
  [ -z "$pid" ] && continue
  if ps -o command= -p "$pid" 2>/dev/null | grep -q "biliClipFlow"; then
    case " $OLD_PIDS " in
      *" $pid "*) ;;
      *) OLD_PIDS="$OLD_PIDS $pid" ;;
    esac
  fi
done
# shellcheck disable=SC2086
kill_pids "旧的 dev 进程" $OLD_PIDS

# 2) 仍占用端口的进程
PORT_PIDS=""
if command -v lsof >/dev/null 2>&1; then
  PORT_PIDS=$(lsof -nP -iTCP:"$PORT" -sTCP:LISTEN -t 2>/dev/null || true)
fi
# shellcheck disable=SC2086
kill_pids "占用端口 $PORT 的进程" $PORT_PIDS

if lsof -nP -iTCP:"$PORT" -sTCP:LISTEN >/dev/null 2>&1; then
  echo "错误: 端口 $PORT 仍被占用，请手动检查" >&2
  lsof -nP -iTCP:"$PORT" -sTCP:LISTEN >&2
  exit 1
fi

# ---------- 依赖与启动 ----------
if [ ! -d node_modules ]; then
  echo "首次运行，安装依赖..."
  pnpm install
fi

echo "启动开发模式 (pnpm tauri dev)..."
exec pnpm tauri dev
