import { useEffect, useMemo, useState } from "react";
import { createPortal } from "react-dom";
import { confirm as dialogConfirm, open, save } from "@tauri-apps/plugin-dialog";
import { invokeCommand } from "../lib/tauri";
import VideoMaskWorkspace from "./toolbox/VideoMaskProjectTool";

const toolboxTabs = [
  { key: "remux", label: "格式转码" },
  { key: "video_mask", label: "视频遮罩" },
  { key: "bilibili_season_backup", label: "合集备份" },
];

const normalizePath = (path) => String(path || "").replace(/\\/g, "/");

const buildDefaultTarget = (sourcePath, suffix = "", extension = "mp4") => {
  const normalized = normalizePath(sourcePath);
  if (!normalized) {
    return "";
  }
  const lastSlash = normalized.lastIndexOf("/");
  const dir = lastSlash >= 0 ? normalized.slice(0, lastSlash + 1) : "";
  const base = lastSlash >= 0 ? normalized.slice(lastSlash + 1) : normalized;
  const baseName = base.replace(/\.[^.]+$/, "");
  return `${dir}${baseName}${suffix}.${extension}`;
};

const ensureExtension = (path, extension) => {
  if (!path) {
    return "";
  }
  return path.toLowerCase().endsWith(`.${extension}`) ? path : `${path}.${extension}`;
};

const safeJsonFileName = (title) => {
  const safeTitle = String(title || "bilibili-season-backup")
    .replace(/[\\/:*?"<>|]/g, "_")
    .trim()
    .slice(0, 80) || "bilibili-season-backup";
  return `${safeTitle}.json`;
};

const formatDateTime = (value) => {
  if (!value) {
    return "-";
  }
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return "-";
  }
  return date.toLocaleString();
};

const backupSectionCount = (backup) => {
  const sections = Array.isArray(backup?.sections) ? backup.sections : [];
  if (sections.length > 0) {
    return sections.length;
  }
  const episodes = Array.isArray(backup?.episodes) ? backup.episodes : [];
  return episodes.length > 0 ? 1 : 0;
};

const backupEpisodeCount = (backup) => {
  if (Number.isFinite(Number(backup?.capturedEpisodeCount))) {
    return Number(backup.capturedEpisodeCount);
  }
  const sections = Array.isArray(backup?.sections) ? backup.sections : [];
  if (sections.length > 0) {
    return sections.reduce((total, section) => total + (Array.isArray(section?.episodes) ? section.episodes.length : 0), 0);
  }
  return Array.isArray(backup?.episodes) ? backup.episodes.length : 0;
};

const backupPublishedAtCount = (backup) => {
  const sections = Array.isArray(backup?.sections) ? backup.sections : [];
  if (sections.length > 0) {
    return sections.reduce((total, section) => {
      const episodes = Array.isArray(section?.episodes) ? section.episodes : [];
      return total + episodes.filter((episode) => Number.isFinite(Number(episode?.publishedAt))).length;
    }, 0);
  }
  const episodes = Array.isArray(backup?.episodes) ? backup.episodes : [];
  return episodes.filter((episode) => Number.isFinite(Number(episode?.publishedAt))).length;
};

const seasonStructureText = (sectionCount, episodeCount) =>
  `${Number(sectionCount) || 0} 个子合集 / ${Number(episodeCount) || 0} 个视频`;

const episodeSortOptions = [
  { value: "backup", label: "按备份顺序" },
  { value: "publish_asc", label: "投稿时间正序" },
  { value: "publish_desc", label: "投稿时间倒序" },
];

const episodeSortLabel = (value) =>
  episodeSortOptions.find((item) => item.value === value)?.label || episodeSortOptions[0].label;

const formatRestoreMessage = (result) => {
  const warnings = Array.isArray(result?.warnings) ? result.warnings : [];
  const verification = Array.isArray(result?.verification) ? result.verification : [];
  const failedItems = verification.filter((item) => !item?.matched);
  const visibleCount = verification.filter((item) => Number(item?.actualShow) === 1).length;
  const visibleText = verification.length > 0 ? `，子合集展开：${visibleCount}/${verification.length}` : "";
  const verifyText = verification.length > 0
    ? `，验收${result?.verified ? "通过" : "未通过"}：实际 ${result?.restoredSectionCount || 0} 个子合集 / ${result?.restoredEpisodeCount || 0} 个视频${visibleText}${failedItems.length > 0 ? `，异常：${failedItems.map((item) => `${item.title} ${item.actualEpisodes}/${item.expectedEpisodes} 展开${item.actualShow}`).join("；")}` : ""}`
    : "";
  const warningText = warnings.length > 0
    ? `；提示 ${warnings.length} 条：${warnings.slice(0, 3).join("；")}${warnings.length > 3 ? "；其余请查看日志或重新恢复结果" : ""}`
    : "";
  const sortText = result?.episodeSortMode ? `，排序：${episodeSortLabel(result.episodeSortMode)}` : "";
  return `恢复完成，新合集ID：${result?.newSeasonId || "-"}，新增子合集：${result?.createdSectionCount || 0}，绑定视频：${result?.addedEpisodeCount || 0}${sortText}${verifyText}${warningText}`;
};

function RemuxTool() {
  const [sourcePath, setSourcePath] = useState("");
  const [targetPath, setTargetPath] = useState("");
  const [message, setMessage] = useState("");
  const [running, setRunning] = useState(false);

  const defaultTarget = useMemo(() => buildDefaultTarget(sourcePath), [sourcePath]);

  const handlePickSource = async () => {
    setMessage("");
    const selected = await open({
      multiple: false,
      directory: false,
      title: "选择 FLV 文件",
      filters: [{ name: "FLV", extensions: ["flv"] }],
    });
    if (typeof selected === "string") {
      const nextDefault = buildDefaultTarget(selected);
      setSourcePath(selected);
      setTargetPath((prev) => (!prev || prev === defaultTarget ? nextDefault : prev));
    }
  };

  const handlePickTarget = async () => {
    setMessage("");
    const selected = await save({
      title: "保存 MP4 文件",
      filters: [{ name: "MP4", extensions: ["mp4"] }],
      defaultPath: defaultTarget || undefined,
    });
    if (typeof selected === "string") {
      setTargetPath(ensureExtension(selected, "mp4"));
    }
  };

  const handleRemux = async () => {
    setMessage("");
    if (!sourcePath.trim()) {
      setMessage("请选择 FLV 文件");
      return;
    }
    if (!targetPath.trim()) {
      setMessage("请选择输出路径");
      return;
    }
    setRunning(true);
    try {
      await invokeCommand("toolbox_remux", {
        payload: {
          sourcePath,
          targetPath,
        },
      });
      setMessage("转封装完成");
    } catch (error) {
      setMessage(error?.message || "转封装失败");
    } finally {
      setRunning(false);
    }
  };

  return (
    <>
      <div className="panel p-4 space-y-3">
        <div className="space-y-1">
          <div className="text-lg font-semibold">格式转码</div>
          <div className="desc">基于 FFmpeg 转封装，仅支持 FLV 转 MP4，不进行重新编码。</div>
        </div>
        <div className="space-y-2">
          <PathPicker value={sourcePath} placeholder="请选择 FLV 文件" buttonText="选择文件" onPick={handlePickSource} />
          <PathPicker value={targetPath} placeholder="请选择输出 MP4 路径" buttonText="保存到" onPick={handlePickTarget} />
          <ActionLine message={message}>
            <button className="h-8 px-3 rounded-lg" onClick={handleRemux} disabled={running}>
              {running ? "转封装中..." : "开始转封装"}
            </button>
          </ActionLine>
        </div>
      </div>

      <div className="panel p-4 space-y-1 text-xs text-[var(--desc-color)]">
        <div>1. 选择需要转封装的 FLV 文件。</div>
        <div>2. 选择 MP4 保存位置。</div>
        <div>3. 转封装会占用磁盘 IO，可能影响正在进行的录制。</div>
        <div>4. 如果录制文件存在问题，请先修复后再转封装。</div>
        <div>5. 转封装后无法再进行修复，请确认文件正常。</div>
      </div>
    </>
  );
}

function BilibiliSeasonBackupTool() {
  const [seasons, setSeasons] = useState([]);
  const [backups, setBackups] = useState([]);
  const [schedules, setSchedules] = useState([]);
  const [batchExportMode, setBatchExportMode] = useState(false);
  const [selectedBackupIds, setSelectedBackupIds] = useState([]);
  const [message, setMessage] = useState("");
  const [loading, setLoading] = useState(false);
  const [operatingId, setOperatingId] = useState("");
  const [restoreDialog, setRestoreDialog] = useState(null);
  const [restoreSortMode, setRestoreSortMode] = useState("backup");

  const loadSeasons = async () => {
    setMessage("");
    setLoading(true);
    try {
      const data = await invokeCommand("toolbox_bilibili_season_list");
      setSeasons(Array.isArray(data) ? data : []);
    } catch (error) {
      setMessage(error?.message || "查询合集失败");
    } finally {
      setLoading(false);
    }
  };

  const loadBackups = async () => {
    try {
      const data = await invokeCommand("toolbox_bilibili_season_backups");
      const nextBackups = Array.isArray(data) ? data : [];
      setBackups(nextBackups);
      setSelectedBackupIds((prev) => prev.filter((id) => nextBackups.some((backup) => backup.backupId === id)));
    } catch (error) {
      setMessage(error?.message || "读取本地备份失败");
    }
  };

  const loadSchedules = async () => {
    try {
      const data = await invokeCommand("toolbox_bilibili_season_backup_schedules");
      setSchedules(Array.isArray(data) ? data : []);
    } catch (error) {
      setMessage(error?.message || "读取定时备份配置失败");
    }
  };

  useEffect(() => {
    loadSeasons();
    loadBackups();
    loadSchedules();
  }, []);

  const handleBackup = async (seasonId) => {
    setMessage("");
    setOperatingId(`backup-${seasonId}`);
    try {
      const backup = await invokeCommand("toolbox_bilibili_season_backup", {
        payload: { seasonId },
      });
      await loadBackups();
      const sections = backupSectionCount(backup);
      const episodes = backupEpisodeCount(backup);
      const publishedAtCount = backupPublishedAtCount(backup);
      const publishText = episodes > 0 ? `，投稿时间 ${publishedAtCount}/${episodes}` : "";
      setMessage(backup?.complete ? `备份完成：${sections} 个子合集，${episodes} 个视频${publishText}` : `备份已保存，但有子合集视频未完整返回${publishText}`);
    } catch (error) {
      setMessage(error?.message || "备份失败");
    } finally {
      setOperatingId("");
    }
  };

  const openRestoreDialog = (backup) => {
    if (!backup?.complete) {
      setMessage("该备份不完整，暂不能恢复");
      return;
    }
    setRestoreSortMode("backup");
    setRestoreDialog(backup);
  };

  const handleRestore = async () => {
    const backup = restoreDialog;
    if (!backup?.complete) {
      setRestoreDialog(null);
      setMessage("该备份不完整，暂不能恢复");
      return;
    }
    const sections = backupSectionCount(backup);
    const episodes = backupEpisodeCount(backup);
    const ok = await dialogConfirm(`将重建合集「${backup.title}」，恢复 ${sections} 个子合集并绑定 ${episodes} 个视频，视频排序：${episodeSortLabel(restoreSortMode)}，确认继续？`, {
      title: "确认恢复合集",
      kind: "warning",
    });
    if (!ok) {
      return;
    }
    setMessage("");
    setOperatingId(`restore-${backup.backupId}`);
    try {
      const result = await invokeCommand("toolbox_bilibili_season_restore", {
        payload: { backupId: backup.backupId, episodeSortMode: restoreSortMode },
      });
      setMessage(formatRestoreMessage(result));
      setRestoreDialog(null);
      await loadSeasons();
    } catch (error) {
      setMessage(error?.message || "恢复失败");
    } finally {
      setOperatingId("");
    }
  };

  const handleDeleteBackup = async (backup) => {
    const ok = await dialogConfirm(`确定删除本地备份「${backup.title}」？该操作只删除本地备份记录，不会影响 B站合集。`, {
      title: "确认删除备份",
      kind: "warning",
    });
    if (!ok) {
      return;
    }
    setMessage("");
    setOperatingId(`delete-${backup.backupId}`);
    try {
      await invokeCommand("toolbox_bilibili_season_backup_delete", {
        payload: { backupId: backup.backupId },
      });
      await loadBackups();
      setMessage("本地备份已删除");
    } catch (error) {
      setMessage(error?.message || "删除备份失败");
    } finally {
      setOperatingId("");
    }
  };

  const handleSchedule = async (season, enabled) => {
    const seasonId = Number(season?.seasonId) || 0;
    if (seasonId <= 0) {
      return;
    }
    setMessage("");
    setOperatingId(`schedule-${seasonId}`);
    try {
      if (enabled) {
        await invokeCommand("toolbox_bilibili_season_backup_schedule_set", {
          payload: {
            seasonId,
            title: season.title || `合集 ${seasonId}`,
            enabled: true,
          },
        });
        setMessage(`已设置「${season.title || seasonId}」每天 00:00 自动备份`);
      } else {
        await invokeCommand("toolbox_bilibili_season_backup_schedule_delete", {
          payload: { seasonId },
        });
        setMessage(`已取消「${season.title || seasonId}」的定时备份`);
      }
      await loadSchedules();
    } catch (error) {
      setMessage(error?.message || (enabled ? "设置定时备份失败" : "取消定时备份失败"));
    } finally {
      setOperatingId("");
    }
  };

  const handleExportBackups = async (backupIds, defaultPath, clearSelection = false) => {
    setMessage("");
    setOperatingId("export");
    let selected;
    try {
      selected = await save({
        title: "导出合集备份 JSON",
        defaultPath: defaultPath || "bilibili-season-backups.json",
        filters: [{ name: "JSON", extensions: ["json"] }],
      });
    } catch (error) {
      setOperatingId("");
      setMessage(error?.message || "打开导出文件对话框失败");
      return;
    }
    if (typeof selected !== "string" || !selected.trim()) {
      setOperatingId("");
      return;
    }
    try {
      const result = await invokeCommand("toolbox_bilibili_season_backup_export", {
        payload: { path: ensureExtension(selected, "json"), backupIds },
      });
      setMessage(`已导出 ${result?.backupCount || 0} 个合集备份：${result?.path || selected}`);
      if (clearSelection) {
        setSelectedBackupIds([]);
        setBatchExportMode(false);
      }
    } catch (error) {
      setMessage(error?.message || "导出合集备份失败");
    } finally {
      setOperatingId("");
    }
  };

  const handleBatchExport = async () => {
    if (!batchExportMode) {
      setBatchExportMode(true);
      setMessage("请选择要导出的本地合集备份");
      return;
    }
    if (selectedBackupIds.length === 0) {
      setMessage("请至少选择一个本地合集备份");
      return;
    }
    await handleExportBackups(selectedBackupIds, "bilibili-season-backups-selected.json", true);
  };

  const toggleBackupSelection = (backupId) => {
    setSelectedBackupIds((prev) => (
      prev.includes(backupId) ? prev.filter((id) => id !== backupId) : [...prev, backupId]
    ));
  };

  const toggleAllBackupSelection = () => {
    setSelectedBackupIds((prev) => (
      prev.length === backups.length ? [] : backups.map((backup) => backup.backupId)
    ));
  };

  const handleImportBackups = async () => {
    setMessage("");
    const selected = await open({
      multiple: false,
      directory: false,
      title: "导入合集备份 JSON",
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    if (typeof selected !== "string" || !selected.trim()) {
      return;
    }
    try {
      const result = await invokeCommand("toolbox_bilibili_season_backup_import", {
        payload: { path: selected },
      });
      await loadBackups();
      setMessage(`已导入 ${result?.importedCount || 0} 个合集备份，本地共 ${result?.totalCount || 0} 个`);
    } catch (error) {
      setMessage(error?.message || "导入合集备份失败");
    }
  };

  const backupMap = useMemo(() => {
    const map = new Map();
    backups.forEach((item) => map.set(item.sourceSeasonId, item));
    return map;
  }, [backups]);

  const scheduleMap = useMemo(() => {
    const map = new Map();
    schedules.forEach((item) => map.set(item.seasonId, item));
    return map;
  }, [schedules]);

  const restoreModal = restoreDialog ? createPortal(
    <div className="fixed inset-0 z-[220] flex items-center justify-center bg-black/35 p-4">
      <div className="w-full max-w-md rounded-lg border border-[var(--split-color)] bg-[var(--solid-block-color)] p-4 text-[var(--content-color)] shadow-xl">
        <div className="text-base font-semibold">恢复合集</div>
        <div className="mt-2 text-sm text-[var(--desc-color)]">
          将按备份重建「{restoreDialog.title}」，恢复 {backupSectionCount(restoreDialog)} 个子合集并绑定 {backupEpisodeCount(restoreDialog)} 个视频。
        </div>
        <label className="mt-4 block space-y-1 text-xs text-[var(--desc-color)]">
          <span>视频排序</span>
          <select className="w-full border border-[var(--split-color)] bg-[var(--solid-block-color)] text-[var(--content-color)]" value={restoreSortMode} onChange={(event) => setRestoreSortMode(event.target.value)}>
            {episodeSortOptions.map((option) => (
              <option key={option.value} value={option.value}>
                {option.label}
              </option>
            ))}
          </select>
        </label>
        <div className="mt-2 text-xs text-[var(--desc-color)]">
          投稿时间排序只调整每个子合集内部的视频顺序，不改变子合集顺序；排序使用备份时保存的投稿时间，旧备份缺少该字段时建议重新备份。
        </div>
        <div className="mt-4 flex justify-end gap-2">
          <button className="h-8 px-3 rounded-lg border border-[var(--split-color)] bg-[var(--solid-button-color)]" onClick={() => setRestoreDialog(null)} disabled={Boolean(operatingId)}>
            取消
          </button>
          <button className="h-8 px-3 rounded-lg bg-[var(--primary-color)] text-white" onClick={handleRestore} disabled={Boolean(operatingId)}>
            {operatingId === `restore-${restoreDialog.backupId}` ? "恢复中..." : "确认恢复"}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  ) : null;

  return (
    <div className="space-y-4">
      <div className="panel p-4 space-y-3">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="space-y-1">
            <div className="text-lg font-semibold">合集备份</div>
            <div className="desc">查询当前登录账号合集，保存合集信息和接口返回的绑定视频，本地备份不保存 Cookie。</div>
          </div>
          <button className="h-8 px-3 rounded-lg" onClick={loadSeasons} disabled={loading}>
            {loading ? "查询中..." : "刷新"}
          </button>
        </div>
        {message ? <div className="text-xs text-[var(--desc-color)]">{message}</div> : null}
        <div className="overflow-auto rounded-lg border border-[var(--split-color)]">
          <table className="w-full min-w-[760px] text-left text-sm">
            <thead className="bg-[var(--solid-button-color)] text-xs text-[var(--desc-color)]">
              <tr>
                <th className="px-3 py-2">合集</th>
                <th className="px-3 py-2">合集ID</th>
                <th className="px-3 py-2">结构</th>
                <th className="px-3 py-2">可备份</th>
                <th className="px-3 py-2">状态</th>
                <th className="px-3 py-2">操作</th>
              </tr>
            </thead>
            <tbody>
              {seasons.length > 0 ? (
                seasons.map((season) => {
                  const backup = backupMap.get(season.seasonId);
                  const schedule = scheduleMap.get(season.seasonId);
                  return (
                    <tr key={season.seasonId} className="border-t border-[var(--split-color)]">
                      <td className="px-3 py-2">
                        <div className="font-medium">{season.title || "-"}</div>
                        <div className="max-w-[360px] truncate text-xs text-[var(--desc-color)]">{season.description || "无简介"}</div>
                      </td>
                      <td className="px-3 py-2">{season.seasonId}</td>
                      <td className="px-3 py-2">{seasonStructureText(season.sectionCount, season.episodeCount)}</td>
                      <td className="px-3 py-2">{season.complete ? "是" : "否"}</td>
                      <td className="px-3 py-2">
                        <div>{backup ? "已备份" : "未备份"}</div>
                        {schedule?.enabled ? <div className="text-xs text-[var(--primary-color)]">每日 00:00 自动备份</div> : null}
                        {schedule?.lastError ? <div className="max-w-[220px] truncate text-xs text-red-500" title={schedule.lastError}>上次失败</div> : null}
                      </td>
                      <td className="px-3 py-2">
                        <div className="flex flex-wrap gap-2">
                          <button className="h-8 px-3 rounded-lg" onClick={() => handleBackup(season.seasonId)} disabled={Boolean(operatingId)}>
                            {operatingId === `backup-${season.seasonId}` ? "备份中..." : "备份"}
                          </button>
                          <button
                            className={`h-8 rounded-lg px-3 ${schedule?.enabled ? "border border-[var(--split-color)] bg-[var(--solid-button-color)]" : "bg-[var(--primary-color)] text-white"}`}
                            onClick={() => handleSchedule(season, !schedule?.enabled)}
                            disabled={Boolean(operatingId)}
                          >
                            {operatingId === `schedule-${season.seasonId}` ? "处理中..." : schedule?.enabled ? "取消定时" : "定时备份"}
                          </button>
                        </div>
                      </td>
                    </tr>
                  );
                })
              ) : (
                <tr>
                  <td className="px-3 py-6 text-center text-[var(--desc-color)]" colSpan={6}>
                    点击“刷新”加载列表
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      </div>

      <div className="panel p-4 space-y-3">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="text-sm font-semibold">本地备份</div>
          <div className="flex flex-wrap gap-2">
            <button className="h-8 px-3 rounded-lg border border-[var(--split-color)] bg-[var(--solid-button-color)]" onClick={handleImportBackups} disabled={Boolean(operatingId)}>
              导入
            </button>
            <button className="h-8 px-3 rounded-lg" onClick={handleBatchExport} disabled={Boolean(operatingId) || backups.length === 0}>
              {batchExportMode ? `导出已选 (${selectedBackupIds.length})` : "批量导出"}
            </button>
            {batchExportMode ? (
              <button className="h-8 px-3 rounded-lg border border-[var(--split-color)] bg-[var(--solid-button-color)]" onClick={() => { setBatchExportMode(false); setSelectedBackupIds([]); }} disabled={Boolean(operatingId)}>
                取消
              </button>
            ) : null}
          </div>
        </div>
        {batchExportMode ? (
          <div className="flex items-center gap-3 text-xs text-[var(--desc-color)]">
            <button className="underline" onClick={toggleAllBackupSelection} disabled={Boolean(operatingId)}>
              {selectedBackupIds.length === backups.length ? "取消全选" : "全选"}
            </button>
            <span>已选择 {selectedBackupIds.length} / {backups.length}</span>
          </div>
        ) : null}
        <div className="space-y-2">
          {backups.length > 0 ? (
            backups.map((backup) => (
              <div key={backup.backupId} className="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-[var(--split-color)] p-3">
                <div className="min-w-0">
                  <div className="flex items-center gap-2">
                    {batchExportMode ? (
                      <input
                        type="checkbox"
                        checked={selectedBackupIds.includes(backup.backupId)}
                        onChange={() => toggleBackupSelection(backup.backupId)}
                        disabled={Boolean(operatingId)}
                        aria-label={`选择 ${backup.title} 导出`}
                      />
                    ) : null}
                    <div className="font-medium">{backup.title}</div>
                  </div>
                  <div className="text-xs text-[var(--desc-color)]">
                    原合集ID {backup.sourceSeasonId} · {seasonStructureText(backupSectionCount(backup), backupEpisodeCount(backup))} · {backup.complete ? "完整" : "不完整"} · {formatDateTime(backup.createdAt)}
                  </div>
                </div>
                <div className="flex items-center gap-2">
                  <button className="h-8 px-3 rounded-lg" onClick={() => openRestoreDialog(backup)} disabled={Boolean(operatingId) || !backup.complete}>
                    {operatingId === `restore-${backup.backupId}` ? "恢复中..." : "恢复"}
                  </button>
                  <button className="h-8 rounded-lg border border-[var(--split-color)] bg-[var(--solid-button-color)] px-3" onClick={() => handleExportBackups([backup.backupId], safeJsonFileName(backup.title))} disabled={Boolean(operatingId)}>
                    {operatingId === "export" ? "导出中..." : "导出"}
                  </button>
                  <button className="h-8 rounded-lg bg-red-500 px-3 text-white hover:bg-red-600 disabled:opacity-60" onClick={() => handleDeleteBackup(backup)} disabled={Boolean(operatingId)}>
                    {operatingId === `delete-${backup.backupId}` ? "删除中..." : "删除"}
                  </button>
                </div>
              </div>
            ))
          ) : (
            <div className="text-sm text-[var(--desc-color)]">暂无本地备份</div>
          )}
        </div>
      </div>
      {restoreModal}
    </div>
  );
}

function PathPicker({ value, placeholder, buttonText, onPick }) {
  return (
    <div className="flex items-center gap-2">
      <input className="flex-1 min-w-0" value={value} readOnly placeholder={placeholder} />
      <button className="h-8 px-3 rounded-lg shrink-0" onClick={onPick}>
        {buttonText}
      </button>
    </div>
  );
}

function ActionLine({ children, message }) {
  return (
    <div className="flex flex-wrap items-center gap-3">
      {children}
      {message ? <span className="text-xs text-[var(--desc-color)]">{message}</span> : null}
    </div>
  );
}

export default function ToolboxSection({ activeTool = "remux" }) {
  const activeTab = toolboxTabs.some((tab) => tab.key === activeTool) ? activeTool : "remux";

  return (
    <div className="min-w-0 space-y-4 overflow-y-auto pr-1">
      {activeTab === "remux" ? <RemuxTool /> : null}
      {activeTab === "video_mask" ? <VideoMaskWorkspace /> : null}
      {activeTab === "bilibili_season_backup" ? <BilibiliSeasonBackupTool /> : null}
    </div>
  );
}
