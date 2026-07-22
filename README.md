<div align="center">
  <img src="src-tauri/icons/icon.png" width="240" alt="biliClipFlow logo" />

  <h1>biliClipFlow</h1>
  <p>面向 Bilibili 录播的下载、剪辑、分段、投稿一体化桌面工具</p>

  <!-- 徽章占位，可按需替换为真实仓库链接 -->
  <!--
  <div>
    <img src="https://img.shields.io/github/stars/your-org/your-repo" />
    <img src="https://img.shields.io/github/v/release/your-org/your-repo" />
    <img src="https://img.shields.io/github/license/your-org/your-repo" />
  </div>
  -->
</div>

<hr />

## 项目描述

biliClipFlow 是一个基于 Tauri 的跨平台桌面应用，聚焦 Bilibili 录播场景：

- 录播订阅与自动录制
- 多分 P 下载
- 剪辑、合并、分段工作流
- 投稿、更新与失败重试
- 可选百度网盘同步

## 技术栈

- **前端**：React 18 + Vite + Tailwind CSS
- **桌面端**：Tauri 2
- **后端**：Rust
- **数据**：SQLite
- **多媒体/下载**：FFmpeg / FFprobe / aria2c / BaiduPCS-Go（内置二进制）

## 实现功能

| 模块 | 状态 | 说明 |
| ---- | ---- | ---- |
| 直播录制 | ✅ 已完成 | 支持订阅、自动录制与分段策略 |
| 视频下载 | ✅ 已完成 | 分 P 选择、支持多分辨率/编码/格式 |
| 剪辑 | ✅ 已完成 | 支持起止时间裁剪与校验 |
| 合并 | ✅ 已完成 | 多段视频合并成投稿素材 |
| 分段 | ✅ 已完成 | 按配置时长自动切段 |
| 投稿 | ✅ 已完成 | 提交、更新、失败重试与状态追踪 |
| 同步 | ✅ 已完成 | 可选百度网盘同步 |
| 工具 | ✅ 已完成 | 内置转封装与辅助工具 |

## 使用需求

- Node.js 18+
- pnpm 8+
- Rust stable
- Tauri 依赖（macOS 需安装 Xcode Command Line Tools）
- 需要 Bilibili 登录态（扫码/密码/短信等）
- 如启用百度网盘同步，需要百度网盘登录

> [!IMPORTANT]
> 资源解析与投稿依赖账号权限，请确保账号具备相应内容访问权限。

## 快速开始

开发模式：

```bash
pnpm install
pnpm tauri dev
```

Windows IDEA 本地调试（直接启动 WebView2 窗口，无需生成安装包）：

```bash
pnpm run tauri:dev:windows
```

该命令必须在 Windows 环境执行；macOS 无法模拟 Windows WebView2 的渲染行为。

仅启动前端：

```bash
pnpm dev
```

构建与打包：

```bash
pnpm run tauri:build
```

> macOS DMG 生成依赖 `hdiutil`，需在非沙箱环境下执行。

如需指定 bundles，可先执行 `pnpm run install-bins`，再运行 `pnpm exec tauri build --bundles <bundles>`。

Windows 打包（自动拉取二进制，需在 Windows x64 环境执行）：

```bash
BIN_DOWNLOAD=1 pnpm exec tauri build --bundles nsis,msi
```

如果已手动准备二进制文件，可用 `BIN_SOURCE_DIR` 指向包含 `ffmpeg`/`ffprobe`/`aria2c`/`BaiduPCS-Go` 的目录。自动拉取默认仅支持 Windows x64。

## 运行数据位置（macOS）

- 数据目录：`~/Library/Application Support/com.tbw.biliclipflow/`
- 数据库：`bili-clip-flow.sqlite3`
- 日志：`app_debug.log` / `auth_debug.log` / `panic_debug.log`

## 界面预览

### 主播订阅

![image-20260203150555830](.github/live.png)

### 视频下载

![image-20260203150717083](.github/download.png)

### 视频投稿

![image-20260203150730791](.github/publish.png)

### 百度网盘登录

![image-20260203150804766](.github/pan_login.png)

## 贡献

请参考 `CONTRIBUTING.md`。

## 许可与声明

- 协议：见 `LICENSE`
- 第三方组件：见 `THIRD_PARTY_NOTICES.md`

> 本项目仅用于学习与技术研究，请遵守平台服务协议及相关法律法规。
