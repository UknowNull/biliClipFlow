import { useEffect, useState } from "react";
import { invokeCommand } from "../lib/tauri";
import { formatDateTime } from "../lib/format";

export default function AnchorSection() {
  const [newAnchorUid, setNewAnchorUid] = useState("");
  const [anchors, setAnchors] = useState([]);
  const [anchorAvatars, setAnchorAvatars] = useState({});
  const [message, setMessage] = useState("");
  const [loading, setLoading] = useState(false);
  const [syncAnchor, setSyncAnchor] = useState(null);
  const [syncPath, setSyncPath] = useState("");
  const [syncLoading, setSyncLoading] = useState(false);
  const [syncMessage, setSyncMessage] = useState("");
  const [syncPickerOpen, setSyncPickerOpen] = useState(false);
  const [syncBrowserPath, setSyncBrowserPath] = useState("/");
  const [syncFolders, setSyncFolders] = useState([]);
  const [syncBrowseLoading, setSyncBrowseLoading] = useState(false);
  const [syncBrowseError, setSyncBrowseError] = useState("");

  const logClient = async (text) => {
    try {
      await invokeCommand("auth_client_log", { message: text });
    } catch (error) {
      // ignore log errors
    }
  };

  const loadAnchors = async () => {
    setMessage("");
    setLoading(true);
    try {
      await logClient("anchor_list:load_start");
      const data = await invokeCommand("anchor_check");
      setAnchors(data || []);
      await logClient(`anchor_list:load_ok:${Array.isArray(data) ? data.length : 0}`);
    } catch (error) {
      await logClient(`anchor_list:load_error:${error?.message || "unknown"}`);
      setMessage(error.message);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    loadAnchors();
    const timer = setInterval(loadAnchors, 30000);
    return () => clearInterval(timer);
  }, []);

  useEffect(() => {
    let active = true;
    const loadAvatars = async () => {
      const updates = {};
      for (const anchor of anchors) {
        if (!anchor?.avatarUrl) {
          continue;
        }
        if (anchorAvatars[anchor.uid]) {
          continue;
        }
        try {
          const data = await invokeCommand("video_proxy_image", { url: anchor.avatarUrl });
          if (data) {
            updates[anchor.uid] = data;
          }
        } catch (error) {
          // ignore avatar proxy errors
        }
      }
      if (active && Object.keys(updates).length > 0) {
        setAnchorAvatars((prev) => ({ ...prev, ...updates }));
      }
    };
    loadAvatars();
    return () => {
      active = false;
    };
  }, [anchors]);

  const handleSubscribe = async () => {
    const uid = extractUidFromInput(newAnchorUid);
    await logClient(`anchor_subscribe:click raw=${newAnchorUid || ""}`);
    if (!uid) {
      await logClient("anchor_subscribe:invalid_input");
      setMessage("请输入房间号或链接");
      return;
    }
    setLoading(true);
    setMessage("");
    try {
      await logClient(`anchor_subscribe:invoke_start uid=${uid}`);
      await invokeCommand("anchor_subscribe", { payload: { uids: [uid] } });
      await logClient("anchor_subscribe:invoke_ok");
      await loadAnchors();
      setNewAnchorUid("");
    } catch (error) {
      await logClient(`anchor_subscribe:invoke_error:${error?.message || "unknown"}`);
      setMessage(error.message);
    } finally {
      setLoading(false);
    }
  };

  const handleUnsubscribe = async (anchor) => {
    setMessage("");
    try {
      await invokeCommand("anchor_unsubscribe", { uid: anchor.uid });
      await loadAnchors();
    } catch (error) {
      setMessage(error.message);
    }
  };

  const handleStartRecord = async (anchor) => {
    setMessage("");
    try {
      await invokeCommand("live_record_start", { roomId: anchor.uid });
      await loadAnchors();
    } catch (error) {
      setMessage(error.message);
    }
  };

  const handleStopRecord = async (anchor) => {
    setMessage("");
    try {
      await invokeCommand("live_record_stop", { roomId: anchor.uid });
      await loadAnchors();
    } catch (error) {
      setMessage(error.message);
    }
  };

  const handleAutoRecordToggle = async (anchor) => {
    setMessage("");
    try {
      await invokeCommand("live_room_auto_record_update", {
        roomId: anchor.uid,
        autoRecord: !anchor.autoRecord,
      });
      await loadAnchors();
    } catch (error) {
      setMessage(error.message);
    }
  };

  const handleSyncToggle = async (anchor) => {
    setMessage("");
    if (!anchor.baiduSyncEnabled && !anchor.baiduSyncPath) {
      setSyncAnchor(anchor);
      setSyncPath("");
      setSyncBrowserPath("/");
      setSyncMessage("请先选择同步路径");
      setSyncPickerOpen(true);
      loadSyncFolders("/");
      return;
    }
    try {
      await invokeCommand("live_room_baidu_sync_toggle", {
        roomId: anchor.uid,
        enabled: !anchor.baiduSyncEnabled,
      });
      await loadAnchors();
    } catch (error) {
      setMessage(error.message || "同步设置失败");
    }
  };

  const loadSyncFolders = async (path) => {
    setSyncBrowseError("");
    setSyncBrowseLoading(true);
    try {
      const data = await invokeCommand("baidu_sync_remote_dirs", {
        request: { path },
      });
      setSyncFolders(Array.isArray(data) ? data : []);
    } catch (error) {
      setSyncBrowseError(error?.message || "读取目录失败");
      setSyncFolders([]);
    } finally {
      setSyncBrowseLoading(false);
    }
  };

  const handleOpenSyncConfig = (anchor) => {
    const initialPath = anchor?.baiduSyncPath || "";
    const normalizedPath = initialPath.trim() || "/";
    setSyncAnchor(anchor);
    setSyncPath(initialPath.trim());
    setSyncBrowserPath(normalizedPath);
    setSyncMessage("");
  };

  const handleCloseSyncConfig = () => {
    if (syncLoading) {
      return;
    }
    setSyncAnchor(null);
    setSyncPath("");
    setSyncMessage("");
    setSyncFolders([]);
    setSyncBrowseError("");
    setSyncBrowserPath("/");
    setSyncPickerOpen(false);
  };

  const handleSaveSyncConfig = async () => {
    if (!syncAnchor) {
      return;
    }
    setSyncLoading(true);
    setSyncMessage("");
    try {
      await invokeCommand("live_room_baidu_sync_update", {
        roomId: syncAnchor.uid,
        baiduSyncPath: syncPath,
      });
      await loadAnchors();
      setMessage("同步配置已保存");
      setSyncAnchor(null);
      setSyncPath("");
      setSyncMessage("");
      setSyncFolders([]);
      setSyncBrowseError("");
      setSyncBrowserPath("/");
      setSyncPickerOpen(false);
    } catch (error) {
      setSyncMessage(error?.message || "保存失败");
    } finally {
      setSyncLoading(false);
    }
  };

  const handleSyncSelectCurrent = () => {
    setSyncPath(syncBrowserPath);
  };

  const handleOpenSyncPicker = () => {
    if (!syncAnchor) {
      return;
    }
    setSyncPickerOpen(true);
    loadSyncFolders(syncBrowserPath);
  };

  const handleCloseSyncPicker = () => {
    if (syncBrowseLoading) {
      return;
    }
    setSyncPickerOpen(false);
  };

  const handleConfirmSyncPicker = () => {
    setSyncPath(syncBrowserPath);
    setSyncPickerOpen(false);
  };

  const handleSyncEnterFolder = (folder) => {
    if (!folder?.path) {
      return;
    }
    setSyncBrowserPath(folder.path);
    loadSyncFolders(folder.path);
  };

  const handleSyncGoParent = () => {
    if (syncBrowserPath === "/") {
      return;
    }
    const trimmed = syncBrowserPath.replace(/\/+$/, "");
    const index = trimmed.lastIndexOf("/");
    const parent = index <= 0 ? "/" : trimmed.slice(0, index);
    setSyncBrowserPath(parent);
    loadSyncFolders(parent);
  };

  const extractUidFromInput = (input) => {
    if (!input) {
      return "";
    }
    const trimmed = input.trim();
    if (/^\d+$/.test(trimmed)) {
      return trimmed;
    }
    const match = trimmed.match(/live\.bilibili\.com\/(\d+)/);
    if (match && match[1]) {
      return match[1];
    }
    return trimmed;
  };

  const statusLabel = (status) => {
    if (status === 1) {
      return "直播中";
    }
    if (status === 2) {
      return "轮播中";
    }
    return "未直播";
  };


  return (
    <div className="space-y-6">
      <div className="rounded-2xl bg-[var(--surface)]/90 p-6 shadow-sm ring-1 ring-black/5">
        <div className="flex flex-wrap items-center justify-between gap-4">
          <div>
            <p className="text-sm uppercase tracking-[0.2em] text-[var(--muted)]">主播订阅</p>
            <h2 className="text-2xl font-semibold text-[var(--ink)]">主播订阅管理</h2>
          </div>
          <div className="flex gap-2">
            <button
              className="rounded-full border border-black/10 bg-white px-4 py-2 text-sm font-semibold text-[var(--ink)] transition hover:border-black/20"
              onClick={loadAnchors}
              disabled={loading}
            >
              刷新
            </button>
          </div>
        </div>
        <div className="mt-4 text-sm text-[var(--muted)]">
          从这里开始订阅直播间，状态会在卡片上实时更新。
        </div>
        {message ? (
          <div className="mt-4 rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-700">
            {message}
          </div>
        ) : null}
      </div>

      <div className="rounded-2xl bg-white/80 p-6 shadow-sm ring-1 ring-black/5">
        <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">直播间列表</div>
        <div className="mt-4 grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
          <div className="rounded-2xl border border-dashed border-black/15 bg-white/70 p-4">
            <div className="flex items-center gap-2 text-sm font-semibold text-[var(--ink)]">
              <span className="text-lg">＋</span>
              新增直播间
            </div>
            <input
              value={newAnchorUid}
              onChange={(event) => setNewAnchorUid(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  handleSubscribe();
                }
              }}
              placeholder="房间号或链接"
              className="mt-3 w-full rounded-xl border border-black/10 bg-white/80 px-3 py-2 text-sm text-[var(--ink)] focus:border-[var(--accent)] focus:outline-none"
            />
            <button
              className="mt-3 w-full rounded-xl bg-[var(--accent)] px-4 py-2 text-sm font-semibold text-white shadow-sm transition hover:brightness-110"
              onClick={handleSubscribe}
              disabled={loading}
            >
              订阅
            </button>
            <div className="mt-2 text-xs text-[var(--muted)]">支持房间号或直播间链接</div>
          </div>
          {anchors.map((anchor) => (
            <div key={anchor.id} className="rounded-2xl border border-black/5 bg-white/90 p-4">
              <div className="flex items-start justify-between gap-3">
                <div className="flex items-center gap-3">
                  <div className="h-12 w-12 overflow-hidden rounded-full bg-black/5">
                    {anchorAvatars[anchor.uid] ? (
                      <img
                        src={anchorAvatars[anchor.uid]}
                        alt={anchor.nickname || "主播"}
                        className="h-full w-full object-cover"
                      />
                    ) : (
                      <div className="flex h-full w-full items-center justify-center text-xs text-[var(--muted)]">
                        头像
                      </div>
                    )}
                  </div>
                  <div>
                    <div className="text-sm font-semibold text-[var(--ink)]">
                      {anchor.nickname || "未知主播"}
                    </div>
                    <div className="text-xs text-[var(--muted)]">房间号：{anchor.uid}</div>
                  </div>
                </div>
                <div className="flex flex-col items-end gap-2">
                  <span
                    className={`rounded-full px-2 py-0.5 text-xs font-semibold ${
                      anchor.liveStatus === 1
                        ? "bg-emerald-500/10 text-emerald-600"
                        : "bg-slate-500/10 text-slate-600"
                    }`}
                  >
                    {statusLabel(anchor.liveStatus)}
                  </span>
                  {anchor.recordingStatus ? (
                    <span className="rounded-full bg-amber-500/10 px-2 py-0.5 text-xs font-semibold text-amber-600">
                      录制中
                    </span>
                  ) : anchor.autoRecord ? (
                    <span className="rounded-full bg-orange-500/10 px-2 py-0.5 text-xs font-semibold text-orange-600">
                      监控中
                    </span>
                  ) : null}
                </div>
              </div>
              {anchor.liveStatus === 1 ? (
                <div className="mt-3 text-sm text-[var(--ink)]">
                  <div className="font-semibold">{anchor.liveTitle || "直播标题"}</div>
                  <div className="text-xs text-[var(--muted)]">
                    {anchor.category || "未知分区"}
                  </div>
                </div>
              ) : (
                <div className="mt-3 text-xs text-[var(--muted)]">当前未开播</div>
              )}
              <div className="mt-3 flex flex-wrap items-center gap-2 text-xs text-[var(--muted)]">
                <span>自动录制：{anchor.autoRecord ? "已开启" : "已关闭"}</span>
                <button
                  className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                  onClick={() => handleAutoRecordToggle(anchor)}
                >
                  {anchor.autoRecord ? "关闭" : "开启"}
                </button>
                <span>同步上传：{anchor.baiduSyncEnabled ? "已开启" : "未开启"}</span>
                <button
                  className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                  onClick={() => handleSyncToggle(anchor)}
                >
                  {anchor.baiduSyncEnabled ? "关闭" : "开启"}
                </button>
                {anchor.baiduSyncEnabled && anchor.baiduSyncPath ? (
                  <span>同步路径：{anchor.baiduSyncPath}</span>
                ) : null}
                <span>上次检查：{formatDateTime(anchor.lastCheckTime)}</span>
              </div>
              <div className="mt-3 flex flex-wrap gap-2">
                {anchor.recordingStatus ? (
                  <button
                    className="rounded-full border border-black/10 bg-white px-3 py-1.5 text-xs font-semibold text-[var(--ink)]"
                    onClick={() => handleStopRecord(anchor)}
                  >
                    停止录制
                  </button>
                ) : (
                  <button
                    className="rounded-full border border-black/10 bg-white px-3 py-1.5 text-xs font-semibold text-[var(--ink)]"
                    onClick={() => handleStartRecord(anchor)}
                  >
                    开始录制
                  </button>
                )}
                <button
                  className="rounded-full border border-black/10 bg-white px-3 py-1.5 text-xs font-semibold text-[var(--ink)]"
                  onClick={() => handleUnsubscribe(anchor)}
                >
                  取消订阅
                </button>
                {anchor.baiduSyncEnabled ? (
                  <button
                    className="rounded-full border border-black/10 bg-white px-3 py-1.5 text-xs font-semibold text-[var(--ink)]"
                    onClick={() => handleOpenSyncConfig(anchor)}
                  >
                    同步配置
                  </button>
                ) : null}
              </div>
              {anchor.recordingFile ? (
                <div className="mt-2 text-xs text-[var(--muted)]">
                  当前文件：{anchor.recordingFile}
                </div>
              ) : null}
              {anchor.recordingStartTime ? (
                <div className="mt-1 text-xs text-[var(--muted)]">
                  开始时间：{formatDateTime(anchor.recordingStartTime)}
                </div>
              ) : null}
            </div>
          ))}
        </div>
        {anchors.length === 0 ? (
          <div className="mt-4 text-sm text-[var(--muted)]">暂无订阅记录。</div>
        ) : null}
      </div>
      {syncAnchor ? (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
          <div className="w-[420px] rounded-2xl bg-[var(--block-color)] p-5 text-sm text-[var(--content-color)] shadow-xl">
            <div className="text-base font-semibold">同步配置</div>
            <div className="mt-2 text-xs text-[var(--desc-color)]">
              主播：{syncAnchor.nickname || syncAnchor.uid}
            </div>
            <div className="mt-3 text-xs text-[var(--desc-color)]">
              录播分段上传到百度网盘的目录路径
            </div>
            <div className="mt-2 flex flex-wrap items-center gap-2 text-xs">
              <div className="rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-[var(--ink)]">
                {syncPath || "未配置"}
              </div>
              <button
                className="rounded-full border border-black/10 bg-white px-3 py-1 font-semibold text-[var(--ink)]"
                onClick={handleOpenSyncPicker}
              >
                选择目录
              </button>
            </div>
            {syncMessage ? (
              <div className="mt-3 text-xs text-amber-600">{syncMessage}</div>
            ) : null}
            <div className="mt-4 flex justify-end gap-2">
              <button className="h-9 rounded-lg px-4" onClick={handleCloseSyncConfig}>
                取消
              </button>
              <button
                className="h-9 rounded-lg px-4"
                onClick={handleSaveSyncConfig}
                disabled={syncLoading}
              >
                保存
              </button>
            </div>
          </div>
        </div>
      ) : null}
      {syncPickerOpen ? (
        <div className="fixed inset-0 z-[60] flex items-center justify-center bg-black/50">
          <div className="w-[520px] rounded-2xl bg-[var(--block-color)] p-5 text-sm text-[var(--content-color)] shadow-xl">
            <div className="text-base font-semibold">选择百度网盘目录</div>
            <div className="mt-2 flex items-center gap-2">
              <div className="flex-1 rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-xs text-[var(--ink)]">
                {syncBrowserPath}
              </div>
              <button
                className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                onClick={handleSyncGoParent}
                disabled={syncBrowserPath === "/"}
              >
                上级
              </button>
              <button
                className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                onClick={() => loadSyncFolders(syncBrowserPath)}
                disabled={syncBrowseLoading}
              >
                刷新
              </button>
            </div>
            <div className="mt-3 max-h-64 overflow-auto rounded-xl border border-black/10 bg-white/80 p-2 text-xs text-[var(--ink)]">
              {syncBrowseLoading ? (
                <div className="py-6 text-center text-[var(--desc-color)]">加载中...</div>
              ) : syncBrowseError ? (
                <div className="py-6 text-center text-amber-600">{syncBrowseError}</div>
              ) : syncFolders.length === 0 ? (
                <div className="py-6 text-center text-[var(--desc-color)]">暂无目录</div>
              ) : (
                syncFolders.map((folder) => (
                  <button
                    key={folder.path}
                    className="flex w-full items-center gap-2 rounded-lg px-2 py-2 text-left hover:bg-black/5"
                    onClick={() => handleSyncEnterFolder(folder)}
                  >
                    <span className="text-[10px] font-semibold text-[var(--muted)]">DIR</span>
                    <span className="text-sm">{folder.name}</span>
                  </button>
                ))
              )}
            </div>
            <div className="mt-4 flex justify-end gap-2">
              <button className="h-9 rounded-lg px-4" onClick={handleCloseSyncPicker}>
                取消
              </button>
              <button
                className="h-9 rounded-lg px-4"
                onClick={handleConfirmSyncPicker}
                disabled={syncBrowseLoading}
              >
                选择当前目录
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
}
