import { useEffect, useState } from "react";
import { confirm as dialogConfirm, message as dialogMessage, open } from "@tauri-apps/plugin-dialog";
import { invokeCommand } from "../lib/tauri";
import BaiduSyncPathPicker from "../components/BaiduSyncPathPicker";

const SETTINGS_TABS = [
  { key: "download", title: "下载设置", group: "设置" },
  { key: "sync", title: "同步配置", group: "同步" },
  { key: "live", title: "直播录制", group: "直播录制" },
  { key: "cache", title: "缓存", group: "存储" },
];

const CACHE_ITEMS = [
  { key: "log", title: "日志", description: "包含 auth_debug.log、app_debug.log 等运行日志。" },
  { key: "temp", title: "临时文件", description: "下载与处理过程中产生的临时文件目录。" },
  { key: "database", title: "数据库文件", description: "应用数据库文件体积（清理会执行压缩 VACUUM）。" },
];

export default function SettingsSection() {
  const [threads, setThreads] = useState(3);
  const [queueSize, setQueueSize] = useState(10);
  const [downloadPath, setDownloadPath] = useState("");
  const [logDir, setLogDir] = useState("");
  const [uploadConcurrency, setUploadConcurrency] = useState(3);
  const [submissionRemoteRefreshMinutes, setSubmissionRemoteRefreshMinutes] = useState(10);
  const [blockPcdn, setBlockPcdn] = useState(true);
  const [aria2cConnections, setAria2cConnections] = useState(4);
  const [aria2cSplit, setAria2cSplit] = useState(4);
  const [baiduMaxParallel, setBaiduMaxParallel] = useState(3);
  const [message, setMessage] = useState("");
  const [syncConcurrency, setSyncConcurrency] = useState(3);
  const [syncTargetPath, setSyncTargetPath] = useState("/录播");
  const [syncConfigMessage, setSyncConfigMessage] = useState("");
  const [syncPickerOpen, setSyncPickerOpen] = useState(false);
  const [liveMessage, setLiveMessage] = useState("");
  const [activeTab, setActiveTab] = useState("download");
  const [cacheItems, setCacheItems] = useState([]);
  const [cacheMessage, setCacheMessage] = useState("");
  const [cacheLoading, setCacheLoading] = useState(false);
  const [cacheClearingKey, setCacheClearingKey] = useState("");
  const [liveSettings, setLiveSettings] = useState({
    fileNameTemplate: "live/{{ roomId }}/{{ liveDate }}/录制-{{ roomId }}-{{ now }}-{{ title }}.flv",
    recordPath: "",
    writeMetadata: true,
    saveCover: false,
    recordingQuality: "avc10000,hevc10000",
    recordMode: 0,
    cuttingMode: 0,
    cuttingNumber: 100,
    cuttingByTitle: false,
    titleSplitMinSeconds: 1800,
    danmakuTransport: 0,
    recordDanmaku: false,
    recordDanmakuRaw: false,
    recordDanmakuSuperchat: true,
    recordDanmakuGift: false,
    recordDanmakuGuard: true,
    streamRetryMs: 6000,
    streamRetryNoQnSec: 90,
    streamConnectTimeoutMs: 5000,
    streamReadTimeoutMs: 15000,
    checkIntervalSec: 180,
    flvFixSplitOnMissing: false,
    flvFixAdjustTimestampJump: false,
    flvFixSplitOnTimestampJump: true,
    flvFixDisableOnAnnexb: false,
    baiduSyncEnabled: false,
    baiduSyncPath: "/录播",
  });

  const logClient = async (text) => {
    try {
      await invokeCommand("auth_client_log", { message: text });
    } catch (error) {
      // ignore log errors
    }
  };

  const loadSettings = async () => {
    setMessage("");
    try {
      await logClient("settings_load:start");
      const data = await invokeCommand("get_download_settings");
      if (data) {
        setThreads(data.threads);
        setQueueSize(data.queueSize);
        setDownloadPath(data.downloadPath || "");
        setLogDir(data.logDir || "");
        const concurrency = Math.min(
          5,
          Math.max(1, Number(data.uploadConcurrency || 3)),
        );
        setUploadConcurrency(concurrency);
        const refreshMinutes = Math.max(
          1,
          Number(data.submissionRemoteRefreshMinutes || 10),
        );
        setSubmissionRemoteRefreshMinutes(refreshMinutes);
        setBlockPcdn(Boolean(data.blockPcdn));
        const connections = Math.min(32, Math.max(1, Number(data.aria2cConnections || 4)));
        const split = Math.min(32, Math.max(1, Number(data.aria2cSplit || 4)));
        setAria2cConnections(connections);
        setAria2cSplit(split);
        const maxParallel = Math.min(100, Math.max(1, Number(data.baiduMaxParallel || 3)));
        setBaiduMaxParallel(maxParallel);
        await logClient(`settings_load:ok:${data.downloadPath || ""}`);
        await loadCacheOverview();
      }
    } catch (error) {
      await logClient(`settings_load:error:${error?.message || "unknown"}`);
      setMessage(error.message);
    }
  };

  const loadBaiduSyncSettings = async () => {
    setSyncConfigMessage("");
    try {
      const data = await invokeCommand("baidu_sync_settings");
      const concurrency = Math.max(1, Number(data?.concurrency || 3));
      setSyncConcurrency(concurrency);
      setSyncTargetPath(data?.targetPath || "/录播");
    } catch (error) {
      setSyncConfigMessage(error?.message || "加载同步配置失败");
    }
  };

  const handleSaveBaiduSyncSettings = async () => {
    setSyncConfigMessage("");
    try {
      await invokeCommand("baidu_sync_update_settings", {
        request: {
          concurrency: Number(syncConcurrency || 1),
          targetPath: syncTargetPath,
        },
      });
      setSyncConfigMessage("同步配置已保存");
      await loadBaiduSyncSettings();
    } catch (error) {
      setSyncConfigMessage(error?.message || "保存同步配置失败");
    }
  };

  const handleOpenSyncPicker = () => {
    setSyncPickerOpen(true);
  };

  const handleCloseSyncPicker = () => {
    setSyncPickerOpen(false);
  };

  const handleConfirmSyncPicker = (path) => {
    setSyncTargetPath(path);
    setSyncPickerOpen(false);
  };

  const handleSyncPathChange = (path) => {
    setSyncTargetPath(path);
  };

  const formatBytes = (value) => {
    const size = Number(value);
    if (!Number.isFinite(size) || size <= 0) {
      return "0 B";
    }
    const units = ["B", "KB", "MB", "GB", "TB"];
    const exponent = Math.min(units.length - 1, Math.floor(Math.log(size) / Math.log(1024)));
    const normalized = size / 1024 ** exponent;
    return `${normalized.toFixed(normalized >= 10 || exponent === 0 ? 0 : 1)} ${units[exponent]}`;
  };

  const loadCacheOverview = async () => {
    setCacheMessage("");
    setCacheLoading(true);
    try {
      const data = await invokeCommand("settings_cache_overview");
      setCacheItems(Array.isArray(data) ? data : []);
    } catch (error) {
      setCacheMessage(error?.message || "加载缓存信息失败");
    } finally {
      setCacheLoading(false);
    }
  };

  const handleClearCache = async (key) => {
    const target = CACHE_ITEMS.find((item) => item.key === key);
    const title = target?.title || key;
    const confirmed = await dialogConfirm(`确认清理「${title}」吗？`, {
      title: "清理缓存",
      kind: key === "database" ? "warning" : "info",
      okLabel: "清理",
      cancelLabel: "取消",
    });
    if (!confirmed) {
      return;
    }
    setCacheMessage("");
    setCacheClearingKey(key);
    try {
      await invokeCommand("settings_cache_clear", { key });
      await loadCacheOverview();
      await dialogMessage(`「${title}」已清理完成。`, { title: "清理缓存" });
    } catch (error) {
      setCacheMessage(error?.message || "清理缓存失败");
    } finally {
      setCacheClearingKey("");
    }
  };

  const handleOpenCache = async (key) => {
    setCacheMessage("");
    try {
      await invokeCommand("settings_cache_open", { key });
    } catch (error) {
      setCacheMessage(error?.message || "打开缓存目录失败");
    }
  };

  useEffect(() => {
    loadSettings();
    loadLiveSettings();
    loadBaiduSyncSettings();
  }, []);

  const loadLiveSettings = async () => {
    setLiveMessage("");
    try {
      const data = await invokeCommand("get_live_settings");
      if (data) {
        setLiveSettings({
          fileNameTemplate: data.fileNameTemplate || "",
          recordPath: data.recordPath || "",
          writeMetadata: Boolean(data.writeMetadata),
          saveCover: Boolean(data.saveCover),
          recordingQuality: data.recordingQuality || "",
          recordMode: Number(data.recordMode || 0),
          cuttingMode: Number(data.cuttingMode || 0),
          cuttingNumber: Number(data.cuttingNumber || 0),
          cuttingByTitle: Boolean(data.cuttingByTitle),
          titleSplitMinSeconds: Number(data.titleSplitMinSeconds || 0),
          danmakuTransport: Number(data.danmakuTransport || 0),
          recordDanmaku: Boolean(data.recordDanmaku),
          recordDanmakuRaw: Boolean(data.recordDanmakuRaw),
          recordDanmakuSuperchat: Boolean(data.recordDanmakuSuperchat),
          recordDanmakuGift: Boolean(data.recordDanmakuGift),
          recordDanmakuGuard: Boolean(data.recordDanmakuGuard),
          streamRetryMs: Number(data.streamRetryMs || 0),
          streamRetryNoQnSec: Number(data.streamRetryNoQnSec || 0),
          streamConnectTimeoutMs: Number(data.streamConnectTimeoutMs || 0),
          streamReadTimeoutMs: Number(data.streamReadTimeoutMs || 0),
          checkIntervalSec: Number(data.checkIntervalSec || 0),
          flvFixSplitOnMissing: Boolean(data.flvFixSplitOnMissing),
          flvFixAdjustTimestampJump: Boolean(data.flvFixAdjustTimestampJump),
          flvFixSplitOnTimestampJump: Boolean(data.flvFixSplitOnTimestampJump),
          flvFixDisableOnAnnexb: Boolean(data.flvFixDisableOnAnnexb),
          baiduSyncEnabled: Boolean(data.baiduSyncEnabled),
          baiduSyncPath: data.baiduSyncPath || "/录播",
        });
      }
    } catch (error) {
      setLiveMessage(error.message);
    }
  };

  const handlePickDownloadPath = async () => {
    setMessage("");
    try {
      await logClient("settings_pick_download_path:start");
      const selected = await open({
        directory: true,
        multiple: false,
      });
      await logClient(`settings_pick_download_path:result:${typeof selected}`);
      if (typeof selected === "string") {
        setDownloadPath(selected);
      }
    } catch (error) {
      await logClient(`settings_pick_download_path:error:${error?.message || "unknown"}`);
      setMessage(error.message);
    }
  };

  const handlePickLogDir = async () => {
    setMessage("");
    try {
      const selected = await open({
        directory: true,
        multiple: false,
      });
      if (typeof selected === "string") {
        setLogDir(selected);
      }
    } catch (error) {
      setMessage(error.message);
    }
  };

  const handleSave = async () => {
    setMessage("");
    try {
      const normalizedUploadConcurrency = Math.min(
        5,
        Math.max(1, Number(uploadConcurrency) || 1),
      );
      const normalizedRefreshMinutes = Math.max(
        1,
        Number(submissionRemoteRefreshMinutes) || 1,
      );
      const normalizedAria2cConnections = Math.min(
        32,
        Math.max(1, Number(aria2cConnections) || 1),
      );
      const normalizedAria2cSplit = Math.min(32, Math.max(1, Number(aria2cSplit) || 1));
      const normalizedBaiduMaxParallel = Math.min(
        100,
        Math.max(1, Number(baiduMaxParallel) || 3),
      );
      await logClient(
        `settings_save:start path=${downloadPath} logDir=${logDir} threads=${String(threads)} queue=${String(queueSize)} uploadConcurrency=${String(normalizedUploadConcurrency)} remoteRefreshMinutes=${String(normalizedRefreshMinutes)} blockPcdn=${String(blockPcdn)} aria2cConnections=${String(normalizedAria2cConnections)} aria2cSplit=${String(normalizedAria2cSplit)} baiduMaxParallel=${String(normalizedBaiduMaxParallel)}`,
      );
      await logClient("settings_save:invoke_start");
      const data = await invokeCommand("update_download_settings", {
        threads: Number(threads),
        queueSize: Number(queueSize),
        downloadPath: downloadPath,
        logDir: logDir,
        uploadConcurrency: normalizedUploadConcurrency,
        submissionRemoteRefreshMinutes: normalizedRefreshMinutes,
        blockPcdn: Boolean(blockPcdn),
        aria2cConnections: normalizedAria2cConnections,
        aria2cSplit: normalizedAria2cSplit,
        baiduMaxParallel: normalizedBaiduMaxParallel,
        enableAria2c: true,
      });
      await logClient("settings_save:invoke_end");
      if (data) {
        setThreads(data.threads);
        setQueueSize(data.queueSize);
        setDownloadPath(data.downloadPath || "");
        setLogDir(data.logDir || "");
        setUploadConcurrency(Number(data.uploadConcurrency || 3));
        setSubmissionRemoteRefreshMinutes(
          Math.max(1, Number(data.submissionRemoteRefreshMinutes || 10)),
        );
        setBlockPcdn(Boolean(data.blockPcdn));
        setAria2cConnections(
          Math.min(32, Math.max(1, Number(data.aria2cConnections || 4))),
        );
        setAria2cSplit(Math.min(32, Math.max(1, Number(data.aria2cSplit || 4))));
        setBaiduMaxParallel(
          Math.min(100, Math.max(1, Number(data.baiduMaxParallel || 3))),
        );
        await logClient(`settings_save:ok:${data.downloadPath || ""}`);
      }
      setMessage("设置已保存，日志目录需重启生效");
      await loadCacheOverview();
    } catch (error) {
      await logClient(`settings_save:error:${error?.message || "unknown"}`);
      setMessage(error.message);
    }
  };

  const handleSaveLiveSettings = async () => {
    setLiveMessage("");
    try {
      await invokeCommand("update_live_settings", {
        payload: {
          fileNameTemplate: liveSettings.fileNameTemplate,
          recordPath: liveSettings.recordPath,
          writeMetadata: liveSettings.writeMetadata,
          saveCover: liveSettings.saveCover,
          recordingQuality: liveSettings.recordingQuality,
          recordMode: Number(liveSettings.recordMode || 0),
          cuttingMode: Number(liveSettings.cuttingMode || 0),
          cuttingNumber: Number(liveSettings.cuttingNumber || 0),
          cuttingByTitle: liveSettings.cuttingByTitle,
          titleSplitMinSeconds: Number(liveSettings.titleSplitMinSeconds || 0),
          danmakuTransport: Number(liveSettings.danmakuTransport || 0),
          recordDanmaku: liveSettings.recordDanmaku,
          recordDanmakuRaw: liveSettings.recordDanmakuRaw,
          recordDanmakuSuperchat: liveSettings.recordDanmakuSuperchat,
          recordDanmakuGift: liveSettings.recordDanmakuGift,
          recordDanmakuGuard: liveSettings.recordDanmakuGuard,
          streamRetryMs: Number(liveSettings.streamRetryMs || 0),
          streamRetryNoQnSec: Number(liveSettings.streamRetryNoQnSec || 0),
          streamConnectTimeoutMs: Number(liveSettings.streamConnectTimeoutMs || 0),
          streamReadTimeoutMs: Number(liveSettings.streamReadTimeoutMs || 0),
          checkIntervalSec: Number(liveSettings.checkIntervalSec || 0),
          flvFixSplitOnMissing: liveSettings.flvFixSplitOnMissing,
          flvFixAdjustTimestampJump: liveSettings.flvFixAdjustTimestampJump,
          flvFixSplitOnTimestampJump: liveSettings.flvFixSplitOnTimestampJump,
          flvFixDisableOnAnnexb: liveSettings.flvFixDisableOnAnnexb,
          baiduSyncEnabled: liveSettings.baiduSyncEnabled,
          baiduSyncPath: liveSettings.baiduSyncPath,
        },
      });
      setLiveMessage("直播录制设置已保存");
    } catch (error) {
      setLiveMessage(error.message);
    }
  };

  const handlePickLiveRecordPath = async () => {
    setLiveMessage("");
    try {
      const selected = await open({
        directory: true,
        multiple: false,
      });
      if (typeof selected === "string") {
        setLiveSettings((prev) => ({
          ...prev,
          recordPath: selected,
        }));
      }
    } catch (error) {
      setLiveMessage(error.message);
    }
  };


  return (
    <div className="space-y-4">
      <div className="flex flex-wrap gap-2 xl:hidden">
        {SETTINGS_TABS.map((tab) => (
          <button
            key={tab.key}
            className={`tab-btn ${activeTab === tab.key ? "active" : ""}`}
            onClick={() => setActiveTab(tab.key)}
            type="button"
          >
            {tab.title}
          </button>
        ))}
      </div>
      <div className="flex min-h-0 gap-4">
        <div className="flex-1 min-w-0 space-y-6">
      <div
        className={`${activeTab === "download" ? "block" : "hidden"} rounded-2xl bg-[var(--surface)]/90 p-6 shadow-sm ring-1 ring-black/5`}
      >
        <div>
          <p className="text-sm uppercase tracking-[0.2em] text-[var(--muted)]">设置</p>
          <h2 className="text-2xl font-semibold text-[var(--ink)]">下载设置</h2>
        </div>
        <div className="mt-4 grid gap-3 lg:grid-cols-4">
          <div>
            <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
              下载并发数
            </div>
            <input
              type="number"
              value={threads}
              onChange={(event) => setThreads(event.target.value)}
              className="mt-2 w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
            />
          </div>
          <div>
            <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
              aria2c 连接数
            </div>
            <input
              type="number"
              value={aria2cConnections}
              onChange={(event) => setAria2cConnections(event.target.value)}
              min={1}
              max={32}
              className="mt-2 w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
            />
          </div>
          <div>
            <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
              aria2c 分片数
            </div>
            <input
              type="number"
              value={aria2cSplit}
              onChange={(event) => setAria2cSplit(event.target.value)}
              min={1}
              max={32}
              className="mt-2 w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
            />
          </div>
          <div>
            <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
              网盘 max_parallel
              <span className="group relative ml-2 inline-flex h-4 w-4 items-center justify-center rounded-full border border-black/20 text-[10px] text-[var(--muted)]">
                ?
                <span className="pointer-events-none absolute left-1/2 top-full z-10 mt-2 w-64 -translate-x-1/2 rounded-md bg-black/80 px-2 py-1 text-[10px] text-white opacity-0 shadow transition group-hover:opacity-100">
                  单个网盘下载任务的并发连接数。数值越大速度可能更快，但更易触发限速。非SVIP用户修改该配置可能不生效。
                </span>
              </span>
            </div>
            <input
              type="number"
              value={baiduMaxParallel}
              onChange={(event) => setBaiduMaxParallel(event.target.value)}
              min={1}
              max={100}
              className="mt-2 w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
            />
          </div>
          <div>
            <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
              投稿并发上传数
            </div>
            <input
              type="number"
              value={uploadConcurrency}
              onChange={(event) => setUploadConcurrency(event.target.value)}
              min={1}
              max={5}
              className="mt-2 w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
            />
          </div>
          <div>
            <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
              投稿状态刷新间隔(分钟)
            </div>
            <input
              type="number"
              value={submissionRemoteRefreshMinutes}
              onChange={(event) =>
                setSubmissionRemoteRefreshMinutes(event.target.value)
              }
              min={1}
              className="mt-2 w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
            />
          </div>
          <div className="lg:col-span-2">
            <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
              下载路径
            </div>
            <input
              type="text"
              value={downloadPath}
              onChange={(event) => setDownloadPath(event.target.value)}
              className="mt-2 w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
              placeholder="默认 系统下载目录"
            />
            <button
              className="mt-2 rounded-full border border-black/10 bg-white px-3 py-1.5 text-xs font-semibold text-[var(--ink)] transition hover:border-black/20"
              onClick={handlePickDownloadPath}
              type="button"
            >
              选择路径
            </button>
          </div>
          <div className="lg:col-span-2">
            <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
              日志目录
            </div>
            <input
              type="text"
              value={logDir}
              onChange={(event) => setLogDir(event.target.value)}
              className="mt-2 w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
              placeholder="默认 下载路径/log"
            />
            <div className="mt-2 text-xs text-[var(--muted)]">
              修改后需重启生效
            </div>
            <button
              className="mt-2 rounded-full border border-black/10 bg-white px-3 py-1.5 text-xs font-semibold text-[var(--ink)] transition hover:border-black/20"
              onClick={handlePickLogDir}
              type="button"
            >
              选择路径
            </button>
          </div>
          <div>
            <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
              队列长度
            </div>
            <input
              type="number"
              value={queueSize}
              onChange={(event) => setQueueSize(event.target.value)}
              className="mt-2 w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
            />
          </div>
          <label className="flex items-center gap-2 text-sm text-[var(--muted)] lg:col-span-2">
            <input
              type="checkbox"
              checked={blockPcdn}
              onChange={(event) => setBlockPcdn(event.target.checked)}
            />
            过滤 PCDN（优先镜像与 upos）
          </label>
        </div>
        <div className="mt-4 flex flex-wrap gap-2">
          <button
            className="rounded-full bg-[var(--accent)] px-4 py-2 text-sm font-semibold text-white shadow-sm transition hover:brightness-110"
            onClick={handleSave}
          >
            保存
          </button>
          <button
            className="rounded-full border border-black/10 bg-white px-4 py-2 text-sm font-semibold text-[var(--ink)] transition hover:border-black/20"
            onClick={loadSettings}
          >
            刷新
          </button>
        </div>
        {message ? (
          <div className="mt-3 rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-700">
            {message}
          </div>
        ) : null}
      </div>

      <div
        className={`${activeTab === "sync" ? "block" : "hidden"} rounded-2xl bg-[var(--surface)]/90 p-6 shadow-sm ring-1 ring-black/5`}
      >
        <div>
          <p className="text-sm uppercase tracking-[0.2em] text-[var(--muted)]">同步</p>
          <h2 className="text-2xl font-semibold text-[var(--ink)]">同步配置</h2>
        </div>
        <div className="mt-4 grid gap-3 lg:grid-cols-2">
          <div>
            <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
              最大同时同步数
            </div>
            <input
              type="number"
              value={syncConcurrency}
              onChange={(event) => setSyncConcurrency(event.target.value)}
              min={1}
              className="mt-2 w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
            />
          </div>
          <div>
            <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
              默认网盘上传目录
            </div>
            <div className="mt-2 flex flex-wrap items-center gap-2 text-xs">
              <div className="flex-1 rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-[var(--content-color)]">
                {syncTargetPath || "/录播"}
              </div>
              <button
                className="rounded-full border border-black/10 bg-white px-3 py-1 font-semibold text-[var(--content-color)]"
                onClick={handleOpenSyncPicker}
              >
                选择目录
              </button>
            </div>
          </div>
        </div>
        <div className="mt-4 flex flex-wrap gap-2">
          <button
            className="rounded-full bg-[var(--accent)] px-4 py-2 text-sm font-semibold text-white shadow-sm transition hover:brightness-110"
            onClick={handleSaveBaiduSyncSettings}
          >
            保存
          </button>
          <button
            className="rounded-full border border-black/10 bg-white px-4 py-2 text-sm font-semibold text-[var(--ink)] transition hover:border-black/20"
            onClick={loadBaiduSyncSettings}
          >
            刷新
          </button>
        </div>
        {syncConfigMessage ? (
          <div className="mt-3 rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-700">
            {syncConfigMessage}
          </div>
        ) : null}
      </div>

      <div
        className={`${activeTab === "live" ? "block" : "hidden"} rounded-2xl bg-[var(--surface)]/90 p-6 shadow-sm ring-1 ring-black/5`}
      >
        <div>
          <p className="text-sm uppercase tracking-[0.2em] text-[var(--muted)]">直播录制</p>
          <h2 className="text-2xl font-semibold text-[var(--ink)]">基础设置</h2>
        </div>
        <div className="mt-4 grid gap-3 lg:grid-cols-2">
          <div>
            <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
              录制模式
            </div>
            <select
              value={liveSettings.recordMode}
              onChange={(event) =>
                setLiveSettings((prev) => ({
                  ...prev,
                  recordMode: Number(event.target.value),
                }))
              }
              className="mt-2 w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
            >
              <option value={0}>Standard</option>
              <option value={1}>RawData</option>
            </select>
          </div>
          <div>
            <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
              录制画质
            </div>
            <input
              type="text"
              value={liveSettings.recordingQuality}
              onChange={(event) =>
                setLiveSettings((prev) => ({
                  ...prev,
                  recordingQuality: event.target.value,
                }))
              }
              className="mt-2 w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
              placeholder="avc10000,hevc10000"
            />
          </div>
          <div className="lg:col-span-2">
            <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
              录制文件名模板
            </div>
            <input
              type="text"
              value={liveSettings.fileNameTemplate}
              onChange={(event) =>
                setLiveSettings((prev) => ({
                  ...prev,
                  fileNameTemplate: event.target.value,
                }))
              }
              className="mt-2 w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
            />
          </div>
          <div className="lg:col-span-2">
            <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
              直播文件路径
            </div>
            <input
              type="text"
              value={liveSettings.recordPath}
              onChange={(event) =>
                setLiveSettings((prev) => ({
                  ...prev,
                  recordPath: event.target.value,
                }))
              }
              className="mt-2 w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
              placeholder="默认 下载设置路径/live_recordings"
            />
            <button
              className="mt-2 rounded-full border border-black/10 bg-white px-3 py-1.5 text-xs font-semibold text-[var(--ink)] transition hover:border-black/20"
              onClick={handlePickLiveRecordPath}
              type="button"
            >
              选择路径
            </button>
          </div>
          <div className="lg:col-span-2">
            <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
              百度网盘同步
            </div>
            <label className="mt-2 flex items-center gap-2 text-sm text-[var(--muted)]">
              <input
                type="checkbox"
                checked={liveSettings.baiduSyncEnabled}
                onChange={(event) =>
                  setLiveSettings((prev) => ({
                    ...prev,
                    baiduSyncEnabled: event.target.checked,
                  }))
                }
              />
              启用录播同步上传
            </label>
            <input
              type="text"
              value={liveSettings.baiduSyncPath}
              onChange={(event) =>
                setLiveSettings((prev) => ({
                  ...prev,
                  baiduSyncPath: event.target.value,
                }))
              }
              className="mt-2 w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
              placeholder="/录播/直播间"
            />
          </div>
          <label className="flex items-center gap-2 text-sm text-[var(--muted)]">
            <input
              type="checkbox"
              checked={liveSettings.writeMetadata}
              onChange={(event) =>
                setLiveSettings((prev) => ({
                  ...prev,
                  writeMetadata: event.target.checked,
                }))
              }
            />
            写入直播 metadata
          </label>
          <label className="flex items-center gap-2 text-sm text-[var(--muted)]">
            <input
              type="checkbox"
              checked={liveSettings.saveCover}
              onChange={(event) =>
                setLiveSettings((prev) => ({
                  ...prev,
                  saveCover: event.target.checked,
                }))
              }
            />
            保存直播封面
          </label>
        </div>
      </div>

      <div
        className={`${activeTab === "live" ? "block" : "hidden"} rounded-2xl bg-[var(--surface)]/90 p-6 shadow-sm ring-1 ring-black/5`}
      >
        <div>
          <p className="text-sm uppercase tracking-[0.2em] text-[var(--muted)]">直播录制</p>
          <h2 className="text-2xl font-semibold text-[var(--ink)]">分段与修复</h2>
        </div>
        <div className="mt-4 grid gap-3 lg:grid-cols-2">
          <div>
            <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
              自动分段模式
            </div>
            <select
              value={liveSettings.cuttingMode}
              onChange={(event) =>
                setLiveSettings((prev) => ({
                  ...prev,
                  cuttingMode: Number(event.target.value),
                }))
              }
              className="mt-2 w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
            >
              <option value={0}>禁用</option>
              <option value={1}>按时间分段</option>
              <option value={2}>按大小分段</option>
            </select>
          </div>
          <div>
            <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
              自动分段数值（秒/MB）
            </div>
            <input
              type="number"
              value={liveSettings.cuttingNumber}
              onChange={(event) =>
                setLiveSettings((prev) => ({
                  ...prev,
                  cuttingNumber: event.target.value,
                }))
              }
              className="mt-2 w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
            />
          </div>
          <label className="flex items-center gap-2 text-sm text-[var(--muted)]">
            <input
              type="checkbox"
              checked={liveSettings.cuttingByTitle}
              onChange={(event) =>
                setLiveSettings((prev) => ({
                  ...prev,
                  cuttingByTitle: event.target.checked,
                }))
              }
            />
            改标题后自动分段
          </label>
          <div>
            <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
              标题分段最小时长（秒）
            </div>
            <input
              type="number"
              value={liveSettings.titleSplitMinSeconds}
              onChange={(event) =>
                setLiveSettings((prev) => ({
                  ...prev,
                  titleSplitMinSeconds: event.target.value,
                }))
              }
              className="mt-2 w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
            />
          </div>
          <label className="flex items-center gap-2 text-sm text-[var(--muted)]">
            <input
              type="checkbox"
              checked={liveSettings.flvFixSplitOnMissing}
              onChange={(event) =>
                setLiveSettings((prev) => ({
                  ...prev,
                  flvFixSplitOnMissing: event.target.checked,
                }))
              }
            />
            FLV 修复-检测到可能缺少数据时分段
          </label>
          <label className="flex items-center gap-2 text-sm text-[var(--muted)]">
            <input
              type="checkbox"
              checked={liveSettings.flvFixAdjustTimestampJump}
              onChange={(event) =>
                setLiveSettings((prev) => ({
                  ...prev,
                  flvFixAdjustTimestampJump: event.target.checked,
                }))
              }
            />
            FLV 修复-时间戳跳变自动校准（保守模式）
          </label>
          <label className="flex items-center gap-2 text-sm text-[var(--muted)]">
            <input
              type="checkbox"
              checked={liveSettings.flvFixSplitOnTimestampJump}
              onChange={(event) =>
                setLiveSettings((prev) => ({
                  ...prev,
                  flvFixSplitOnTimestampJump: event.target.checked,
                }))
              }
            />
            FLV 修复-时间戳跳变时优先分段（推荐）
          </label>
          <label className="flex items-center gap-2 text-sm text-[var(--muted)]">
            <input
              type="checkbox"
              checked={liveSettings.flvFixDisableOnAnnexb}
              onChange={(event) =>
                setLiveSettings((prev) => ({
                  ...prev,
                  flvFixDisableOnAnnexb: event.target.checked,
                }))
              }
            />
            FLV 修复-检测到 H264 Annex-B 时禁用修复分段
          </label>
        </div>
      </div>

      <div
        className={`${activeTab === "live" ? "block" : "hidden"} rounded-2xl bg-[var(--surface)]/90 p-6 shadow-sm ring-1 ring-black/5`}
      >
        <div>
          <p className="text-sm uppercase tracking-[0.2em] text-[var(--muted)]">直播录制</p>
          <h2 className="text-2xl font-semibold text-[var(--ink)]">弹幕录制</h2>
        </div>
        <div className="mt-4 grid gap-3 lg:grid-cols-2">
          <div>
            <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
              弹幕连接协议
            </div>
            <select
              value={liveSettings.danmakuTransport}
              onChange={(event) =>
                setLiveSettings((prev) => ({
                  ...prev,
                  danmakuTransport: Number(event.target.value),
                }))
              }
              className="mt-2 w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
            >
              <option value={0}>随机</option>
              <option value={1}>TCP</option>
              <option value={2}>WS</option>
              <option value={3}>WSS</option>
            </select>
          </div>
          <label className="flex items-center gap-2 text-sm text-[var(--muted)]">
            <input
              type="checkbox"
              checked={liveSettings.recordDanmaku}
              onChange={(event) =>
                setLiveSettings((prev) => ({
                  ...prev,
                  recordDanmaku: event.target.checked,
                }))
              }
            />
            弹幕录制
          </label>
          <label className="flex items-center gap-2 text-sm text-[var(--muted)]">
            <input
              type="checkbox"
              checked={liveSettings.recordDanmakuRaw}
              onChange={(event) =>
                setLiveSettings((prev) => ({
                  ...prev,
                  recordDanmakuRaw: event.target.checked,
                }))
              }
            />
            弹幕录制-原始数据
          </label>
          <label className="flex items-center gap-2 text-sm text-[var(--muted)]">
            <input
              type="checkbox"
              checked={liveSettings.recordDanmakuSuperchat}
              onChange={(event) =>
                setLiveSettings((prev) => ({
                  ...prev,
                  recordDanmakuSuperchat: event.target.checked,
                }))
              }
            />
            弹幕录制-SuperChat
          </label>
          <label className="flex items-center gap-2 text-sm text-[var(--muted)]">
            <input
              type="checkbox"
              checked={liveSettings.recordDanmakuGift}
              onChange={(event) =>
                setLiveSettings((prev) => ({
                  ...prev,
                  recordDanmakuGift: event.target.checked,
                }))
              }
            />
            弹幕录制-礼物
          </label>
          <label className="flex items-center gap-2 text-sm text-[var(--muted)]">
            <input
              type="checkbox"
              checked={liveSettings.recordDanmakuGuard}
              onChange={(event) =>
                setLiveSettings((prev) => ({
                  ...prev,
                  recordDanmakuGuard: event.target.checked,
                }))
              }
            />
            弹幕录制-上船
          </label>
        </div>
      </div>

      <div
        className={`${activeTab === "live" ? "block" : "hidden"} rounded-2xl bg-[var(--surface)]/90 p-6 shadow-sm ring-1 ring-black/5`}
      >
        <div>
          <p className="text-sm uppercase tracking-[0.2em] text-[var(--muted)]">直播录制</p>
          <h2 className="text-2xl font-semibold text-[var(--ink)]">重连与检测</h2>
        </div>
        <div className="mt-4 grid gap-3 lg:grid-cols-2">
          <div>
            <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
              开播检测间隔（秒）
            </div>
            <input
              type="number"
              value={liveSettings.checkIntervalSec}
              onChange={(event) =>
                setLiveSettings((prev) => ({
                  ...prev,
                  checkIntervalSec: event.target.value,
                }))
              }
              className="mt-2 w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
            />
          </div>
          <div>
            <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
              断流重连间隔（毫秒）
            </div>
            <input
              type="number"
              value={liveSettings.streamRetryMs}
              onChange={(event) =>
                setLiveSettings((prev) => ({
                  ...prev,
                  streamRetryMs: event.target.value,
                }))
              }
              className="mt-2 w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
            />
          </div>
          <div>
            <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
              无指定画质重连间隔（秒）
            </div>
            <input
              type="number"
              value={liveSettings.streamRetryNoQnSec}
              onChange={(event) =>
                setLiveSettings((prev) => ({
                  ...prev,
                  streamRetryNoQnSec: event.target.value,
                }))
              }
              className="mt-2 w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
            />
          </div>
          <div>
            <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
              连接直播服务器超时（毫秒）
            </div>
            <input
              type="number"
              value={liveSettings.streamConnectTimeoutMs}
              onChange={(event) =>
                setLiveSettings((prev) => ({
                  ...prev,
                  streamConnectTimeoutMs: event.target.value,
                }))
              }
              className="mt-2 w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
            />
          </div>
          <div>
            <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
              录制读取超时（毫秒）
            </div>
            <input
              type="number"
              value={liveSettings.streamReadTimeoutMs}
              onChange={(event) =>
                setLiveSettings((prev) => ({
                  ...prev,
                  streamReadTimeoutMs: event.target.value,
                }))
              }
              className="mt-2 w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
            />
          </div>
        </div>
        <div className="mt-4 flex flex-wrap gap-2">
          <button
            className="rounded-full bg-[var(--accent)] px-4 py-2 text-sm font-semibold text-white shadow-sm transition hover:brightness-110"
            onClick={handleSaveLiveSettings}
          >
            保存
          </button>
          <button
            className="rounded-full border border-black/10 bg-white px-4 py-2 text-sm font-semibold text-[var(--ink)] transition hover:border-black/20"
            onClick={loadLiveSettings}
          >
            刷新
          </button>
        </div>
        {liveMessage ? (
          <div className="mt-3 rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-700">
            {liveMessage}
          </div>
        ) : null}
      </div>

      <div
        className={`${activeTab === "cache" ? "block" : "hidden"} rounded-2xl bg-[var(--surface)]/90 p-6 shadow-sm ring-1 ring-black/5`}
      >
        <div>
          <p className="text-sm uppercase tracking-[0.2em] text-[var(--muted)]">存储</p>
          <h2 className="text-2xl font-semibold text-[var(--ink)]">缓存</h2>
          <p className="mt-2 text-sm text-[var(--muted)]">
            展示日志、临时文件和数据库文件体积，可按项清理。
          </p>
        </div>
        <div className="mt-4 space-y-3">
          {CACHE_ITEMS.map((item) => {
            const cache = cacheItems.find((entry) => entry?.key === item.key);
            const path = String(cache?.path || "").trim() || "-";
            return (
              <div
                key={item.key}
                className="rounded-xl border border-black/10 bg-white/70 p-3"
              >
                <div className="flex flex-wrap items-start justify-between gap-2">
                  <div>
                    <div className="text-sm font-semibold text-[var(--ink)]">{item.title}</div>
                    <div className="mt-1 text-xs text-[var(--muted)]">{item.description}</div>
                    <div className="mt-2 break-all text-xs text-[var(--muted)]">{path}</div>
                  </div>
                  <div className="flex flex-col items-end gap-2">
                    <div className="text-sm font-semibold text-[var(--ink)]">
                      {formatBytes(cache?.sizeBytes || 0)}
                    </div>
                    <div className="flex items-center gap-2">
                      <button
                        className="rounded-full border border-black/10 bg-white px-3 py-1.5 text-xs font-semibold text-[var(--ink)] transition hover:border-black/20"
                        onClick={() => handleOpenCache(item.key)}
                        type="button"
                      >
                        打开
                      </button>
                      <button
                        className="rounded-full border border-black/10 bg-white px-3 py-1.5 text-xs font-semibold text-[var(--ink)] transition hover:border-black/20 disabled:opacity-60"
                        onClick={() => handleClearCache(item.key)}
                        disabled={cacheLoading || cacheClearingKey === item.key}
                        type="button"
                      >
                        {cacheClearingKey === item.key ? "清理中..." : "清理"}
                      </button>
                    </div>
                  </div>
                </div>
              </div>
            );
          })}
        </div>
        <div className="mt-4 flex flex-wrap gap-2">
          <button
            className="rounded-full border border-black/10 bg-white px-4 py-2 text-sm font-semibold text-[var(--ink)] transition hover:border-black/20 disabled:opacity-60"
            onClick={loadCacheOverview}
            disabled={cacheLoading}
            type="button"
          >
            {cacheLoading ? "刷新中..." : "刷新"}
          </button>
        </div>
        {cacheMessage ? (
          <div className="mt-3 rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-700">
            {cacheMessage}
          </div>
        ) : null}
      </div>
        </div>
        <div className="tab hidden xl:flex">
          {SETTINGS_TABS.map((tab) => (
            <button
              key={tab.key}
              className={activeTab === tab.key ? "active" : ""}
              onClick={() => setActiveTab(tab.key)}
              type="button"
            >
              <span>{tab.title}</span>
              <small className="text-[10px] text-[var(--muted)]">{tab.group}</small>
              <label />
            </button>
          ))}
        </div>
      </div>

      <BaiduSyncPathPicker
        open={syncPickerOpen}
        value={syncTargetPath}
        onConfirm={handleConfirmSyncPicker}
        onClose={handleCloseSyncPicker}
        onChange={handleSyncPathChange}
      />
    </div>
  );
}
