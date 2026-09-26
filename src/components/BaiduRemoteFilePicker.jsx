import { useEffect, useMemo, useState } from "react";
import { invokeCommand } from "../lib/tauri";

const VIDEO_FILE_PATTERN = /\.(mp4|mkv|flv|ts|mov|m4v|webm|avi)$/i;

const normalizePath = (value) => {
  const path = String(value || "").trim();
  if (!path) {
    return "/";
  }
  if (path === "/") {
    return path;
  }
  const normalized = path.replace(/\/+/g, "/").replace(/\/$/, "");
  return normalized.startsWith("/") ? normalized : `/${normalized}`;
};

const resolveParentPath = (path) => {
  const normalized = normalizePath(path);
  if (normalized === "/") {
    return "/";
  }
  const parts = normalized.split("/").filter(Boolean);
  parts.pop();
  if (!parts.length) {
    return "/";
  }
  return `/${parts.join("/")}`;
};

const isVideoFile = (entry) => {
  if (!entry || entry.isDir) {
    return false;
  }
  const name = String(entry.name || "");
  return VIDEO_FILE_PATTERN.test(name);
};

export default function BaiduRemoteFilePicker({
  open,
  initialPath,
  onClose,
  onConfirm,
}) {
  const [browserPath, setBrowserPath] = useState("/");
  const [entries, setEntries] = useState([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [selectedFilePath, setSelectedFilePath] = useState("");

  const loadEntries = async (path) => {
    const targetPath = normalizePath(path);
    setLoading(true);
    setError("");
    try {
      const data = await invokeCommand("baidu_sync_remote_entries", {
        request: { path: targetPath },
      });
      const normalized = (Array.isArray(data) ? data : []).map((item) => ({
        name: String(item?.name || "").trim(),
        path: normalizePath(item?.path || ""),
        isDir: Boolean(item?.isDir),
      }));
      setEntries(normalized);
      setBrowserPath(targetPath);
    } catch (loadError) {
      setEntries([]);
      setError(loadError?.message || "读取网盘目录失败");
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    if (!open) {
      return;
    }
    setSelectedFilePath("");
    loadEntries(initialPath || "/");
  }, [open, initialPath]);

  const orderedEntries = useMemo(() => {
    const sorted = [...entries].sort((left, right) => {
      if (left.isDir && !right.isDir) {
        return -1;
      }
      if (!left.isDir && right.isDir) {
        return 1;
      }
      return String(left.name || "").localeCompare(String(right.name || ""), "zh-CN");
    });
    return sorted.filter((item) => item.isDir || isVideoFile(item));
  }, [entries]);

  const handleEnterDir = (entry) => {
    if (!entry?.isDir || !entry?.path) {
      return;
    }
    setSelectedFilePath("");
    loadEntries(entry.path);
  };

  const handleConfirm = () => {
    if (!selectedFilePath) {
      return;
    }
    const selectedEntry = entries.find((entry) => entry.path === selectedFilePath) || null;
    onConfirm?.(selectedFilePath, selectedEntry);
  };

  if (!open) {
    return null;
  }

  return (
    <div className="fixed inset-0 z-[70] flex items-center justify-center bg-black/50">
      <div className="w-[560px] rounded-2xl bg-[var(--block-color)] p-5 text-sm text-[var(--content-color)] shadow-xl">
        <div className="text-base font-semibold">选择网盘视频文件</div>
        <div className="mt-2 flex items-center gap-2">
          <div className="flex-1 rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-xs text-[var(--content-color)]">
            {browserPath}
          </div>
          <button
            className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--content-color)]"
            onClick={() => loadEntries(resolveParentPath(browserPath))}
            disabled={loading || browserPath === "/"}
          >
            上级
          </button>
          <button
            className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--content-color)]"
            onClick={() => loadEntries(browserPath)}
            disabled={loading}
          >
            刷新
          </button>
        </div>
        <div className="mt-3 max-h-72 overflow-auto rounded-xl border border-black/10 bg-white/80 p-2 text-xs text-[var(--content-color)]">
          {loading ? (
            <div className="py-8 text-center text-[var(--desc-color)]">加载中...</div>
          ) : error ? (
            <div className="py-8 text-center text-amber-700">{error}</div>
          ) : orderedEntries.length === 0 ? (
            <div className="py-8 text-center text-[var(--desc-color)]">暂无可选视频文件</div>
          ) : (
            orderedEntries.map((entry) => {
              const active = !entry.isDir && selectedFilePath === entry.path;
              return (
                <button
                  key={`${entry.path}:${entry.isDir ? "dir" : "file"}`}
                  className={`flex w-full items-center gap-2 rounded-lg px-2 py-2 text-left transition ${
                    active
                      ? "bg-[var(--accent)]/12 text-[var(--accent)]"
                      : "hover:bg-black/5 text-[var(--content-color)]"
                  }`}
                  onClick={() => {
                    if (entry.isDir) {
                      handleEnterDir(entry);
                      return;
                    }
                    setSelectedFilePath(entry.path);
                  }}
                >
                  <span className="w-8 text-[10px] font-semibold text-[var(--muted)]">
                    {entry.isDir ? "DIR" : "FILE"}
                  </span>
                  <span className="flex-1 truncate">{entry.name || "-"}</span>
                  <span className="text-[10px] text-[var(--muted)]">
                    {entry.isDir ? "进入" : "选择"}
                  </span>
                </button>
              );
            })
          )}
        </div>
        <div className="mt-2 rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-xs text-[var(--muted)] break-all">
          已选文件：{selectedFilePath || "-"}
        </div>
        <div className="mt-4 flex justify-end gap-2">
          <button
            className="rounded-full border border-black/10 bg-white px-4 py-1 text-xs font-semibold text-[var(--content-color)]"
            onClick={onClose}
            disabled={loading}
          >
            取消
          </button>
          <button
            className="rounded-full bg-[var(--accent)] px-4 py-1 text-xs font-semibold text-white disabled:cursor-not-allowed disabled:opacity-60"
            onClick={handleConfirm}
            disabled={loading || !selectedFilePath}
          >
            确认绑定
          </button>
        </div>
      </div>
    </div>
  );
}
