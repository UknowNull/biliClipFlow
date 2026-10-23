import {
  buildMergeGroupId,
  buildSourceVideoPayload,
  emptySource,
  formatSourceBindingSummary,
  insertMergeItemClone,
  insertSourceBelow,
  isDirectSourceInput,
  isDownloadUrlInput,
  resolveSourceRemoteInput,
} from "./helpers";
import { validateSourceVideos } from "../../lib/submissionEditDiff";
import { invokeCommand } from "../../lib/tauri";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

export default function SubmissionUpdateView({
  applySourceDownloadUrlBinding,
  applyUploadTitle,
  backToList,
  buildGroupedSourceIdSet,
  buildMergeGroupsPayload,
  clampTimeSeconds,
  currentPage,
  defaultBaiduSyncPath,
  formatDurationHms,
  handleChooseSourceRemotePart,
  handleOpenSyncPicker,
  isVideoFilePath,
  loadTasks,
  message,
  normalizeTimeValue,
  pageSize,
  parseHmsToSeconds,
  resetUpdateState,
  setMessage,
  setSubmissionView,
  setUpdateBaiduSync,
  setUpdateImportInput,
  setUpdateImportLoading,
  setUpdateImportOpen,
  setUpdateImportResults,
  setUpdateImportSelection,
  setUpdateMergeItems,
  setUpdateMergeSelection,
  setUpdateSegmentPrefix,
  setUpdateSegmentationEnabled,
  setUpdateSourceVideos,
  setUpdateSubmitting,
  setUpdateWorkflowConfig,
  sourcePartPickerLoadingId,
  statusFilter,
  submissionView,
  syncMergeItemsWithSources,
  updateBaiduSync,
  updateMergeItems,
  updateMergeSelection,
  updateSegmentPrefix,
  updateSegmentationEnabled,
  updateSourceVideos,
  updateSubmitting,
  updateTaskDir,
  updateTaskId,
  updateWorkflowConfig,
}) {
  const isUpdateView = submissionView === "update";

  const closeUpdateModal = () => {
    setSubmissionView("list");
    resetUpdateState();
    setMessage("");
  };

  const toggleUpdateMergeSelection = (sourceId, checked) => {
    const grouped = buildGroupedSourceIdSet(updateMergeItems);
    if (grouped.has(sourceId)) {
      return;
    }
    const item = updateMergeItems.find(
      (candidate) => candidate.type === "SOURCE" && candidate.sourceId === sourceId,
    );
    if (item?.standalone) {
      return;
    }
    setUpdateMergeSelection((prev) => {
      const next = new Set(prev);
      if (checked) {
        next.add(sourceId);
      } else {
        next.delete(sourceId);
      }
      return next;
    });
  };

  const toggleUpdateMergeStandalone = (sourceId) => {
    setUpdateMergeItems((prev) =>
      prev.map((item) => {
        if (item.type !== "SOURCE" || item.sourceId !== sourceId) {
          return item;
        }
        const next = { ...item, standalone: !item.standalone };
        if (next.standalone) {
          setUpdateMergeSelection((selection) => {
            const nextSelection = new Set(selection);
            nextSelection.delete(sourceId);
            return nextSelection;
          });
        }
        return next;
      }),
    );
  };

  const createUpdateMergeGroup = () => {
    const selected = Array.from(updateMergeSelection);
    if (selected.length < 2) {
      setMessage("请至少选择两个源视频进行合并");
      return;
    }
    const selectedSet = new Set(selected);
    const orderedSelected = updateSourceVideos
      .map((item) => item.localId)
      .filter((id) => selectedSet.has(id));
    if (orderedSelected.length < 2) {
      setMessage("请选择有效的源视频进行合并");
      return;
    }
    const groupId = buildMergeGroupId();
    setUpdateMergeItems((prev) => {
      const next = [];
      let inserted = false;
      for (const item of prev) {
        if (item.type === "SOURCE" && selectedSet.has(item.sourceId)) {
          if (!inserted) {
            next.push({ id: groupId, type: "GROUP", sourceIds: orderedSelected });
            inserted = true;
          }
          continue;
        }
        next.push(item);
      }
      if (!inserted) {
        next.push({ id: groupId, type: "GROUP", sourceIds: orderedSelected });
      }
      return next;
    });
    setUpdateMergeSelection(new Set());
  };

  const releaseUpdateMergeGroup = (groupId) => {
    const normalizedGroupId = String(groupId || "").trim();
    if (!normalizedGroupId) {
      return;
    }
    setUpdateMergeItems((prev) => {
      const next = [];
      const releasedIds = [];
      for (const item of prev) {
        if (item.id !== normalizedGroupId || item.type !== "GROUP") {
          next.push(item);
          continue;
        }
        (item.sourceIds || []).forEach((sourceId) => {
          releasedIds.push(sourceId);
          next.push({ id: sourceId, type: "SOURCE", sourceId, standalone: false });
        });
      }
      if (releasedIds.length > 0) {
        setUpdateMergeSelection((selection) => {
          const filtered = new Set(selection);
          releasedIds.forEach((id) => filtered.delete(id));
          return filtered;
        });
      }
      return syncMergeItemsWithSources(updateSourceVideos, next);
    });
  };

  const addUpdateSource = () => {
    setUpdateSourceVideos((prev) => {
      const next = [...prev, emptySource(prev.length)];
      setUpdateMergeItems((items) => syncMergeItemsWithSources(next, items));
      return next;
    });
  };

  const duplicateUpdateSource = (index) => {
    setUpdateSourceVideos((prev) => {
      const target = prev[index];
      if (!target) {
        return prev;
      }
      const next = insertSourceBelow(prev, index, target);
      const clonedSource = next[index + 1];
      setUpdateMergeItems((items) => {
        let merged = syncMergeItemsWithSources(next, items);
        if (clonedSource?.localId && target.localId) {
          merged = insertMergeItemClone(merged, target.localId, clonedSource.localId);
        }
        return merged;
      });
      setUpdateMergeSelection((selection) => {
        const nextSelection = new Set(selection);
        if (clonedSource?.localId) {
          nextSelection.delete(clonedSource.localId);
        }
        return nextSelection;
      });
      return next;
    });
  };

  const updateUpdateSource = (index, field, value) => {
    setUpdateSourceVideos((prev) =>
      prev.map((item, idx) => {
        if (idx !== index) {
          return item;
        }
        if (field === "remoteBvid") {
          if (isDownloadUrlInput(value)) {
            return {
              ...item,
              remoteVideoUrl: value,
              remoteBvid: "",
              remoteAid: 0,
              remoteCid: 0,
              remotePartTitle: "",
            };
          }
          return {
            ...item,
            remoteVideoUrl: "",
            remoteBvid: value,
            remoteAid: 0,
            remoteCid: 0,
            remotePartTitle: "",
          };
        }
        return { ...item, [field]: value };
      }),
    );
  };

  const updateUpdateSourceUploadTitle = (index, value) => {
    setUpdateSourceVideos((prev) =>
      prev.map((item, idx) => (idx === index ? applyUploadTitle(item, value) : item)),
    );
  };

  const updateUpdateSourceTime = (index, field, value) => {
    setUpdateSourceVideos((prev) =>
      prev.map((item, idx) => {
        if (idx !== index) {
          return item;
        }
        return { ...item, [field]: value };
      }),
    );
  };

  const normalizeUpdateSourceTime = (index, field) => {
    setUpdateSourceVideos((prev) =>
      prev.map((item, idx) => {
        if (idx !== index) {
          return item;
        }
        const normalized = normalizeTimeValue(item[field], item.durationSeconds);
        if (!normalized) {
          return { ...item, [field]: formatDurationHms(0) };
        }
        return { ...item, [field]: normalized };
      }),
    );
  };

  const removeUpdateSource = (index) => {
    setUpdateSourceVideos((prev) => {
      const next = prev
        .filter((_, idx) => idx !== index)
        .map((item, idx) => ({ ...item, sortOrder: idx + 1 }));
      const removed = prev[index];
      setUpdateMergeItems((items) => syncMergeItemsWithSources(next, items));
      if (removed?.localId) {
        setUpdateMergeSelection((selection) => {
          const nextSelection = new Set(selection);
          nextSelection.delete(removed.localId);
          return nextSelection;
        });
      }
      return next;
    });
  };

  const buildUpdateWorkflowConfig = (mergeGroups = []) => {
    const prefix = updateSegmentPrefix.trim();
    const config = {
      enableSegmentation: updateSegmentationEnabled,
      segmentationConfig: {
        enabled: updateSegmentationEnabled,
        segmentDurationSeconds: updateWorkflowConfig.segmentationConfig.segmentDurationSeconds,
        preserveOriginal: updateWorkflowConfig.segmentationConfig.preserveOriginal,
      },
      segmentPrefix: prefix ? prefix : null,
    };
    if (Array.isArray(mergeGroups) && mergeGroups.length > 0) {
      config.mergeGroups = mergeGroups;
    }
    return config;
  };

  const handleUpdateSubmit = async () => {
    if (updateSubmitting) {
      return;
    }
    setMessage("");
    const taskId = updateTaskId.trim();
    if (!taskId) {
      setMessage("任务ID无效");
      return;
    }
    if (updateSegmentationEnabled) {
      const segmentDuration = updateWorkflowConfig.segmentationConfig.segmentDurationSeconds;
      if (segmentDuration < 30 || segmentDuration > 600) {
        setMessage("分段时长必须在 30-600 秒之间");
        return;
      }
    }
    const sourceValidation = validateSourceVideos({
      items: updateSourceVideos,
      parseHmsToSeconds,
      isVideoFilePath,
    });
    if (!sourceValidation.valid) {
      setMessage(sourceValidation.message);
      return;
    }
    const validSources = sourceValidation.validSources;
    const mergeGroups = buildMergeGroupsPayload(validSources, updateMergeItems);
    setUpdateSubmitting(true);
    try {
      const payload = {
        request: {
          taskId,
          baiduSyncEnabled: Boolean(updateBaiduSync.enabled),
          baiduSyncPath: updateBaiduSync.path || null,
          baiduSyncFilename: updateBaiduSync.filename || null,
          sourceVideos: validSources.map(buildSourceVideoPayload),
          workflowConfig: buildUpdateWorkflowConfig(mergeGroups),
        },
      };
      await invokeCommand("submission_update", payload);
      closeUpdateModal();
      await loadTasks(statusFilter, currentPage, pageSize);
    } catch (error) {
      setMessage(error.message);
    } finally {
      setUpdateSubmitting(false);
    }
  };

  const handleUpdateSelectSourceFile = async (index) => {
    setMessage("");
    try {
      const selected = await openDialog({
        multiple: false,
        directory: false,
        filters: [
          {
            name: "视频文件",
            extensions: ["mp4", "mkv", "mov", "flv", "avi", "webm"],
          },
        ],
      });
      if (typeof selected !== "string") {
        return;
      }
      const duration = await invokeCommand("video_duration", { path: selected });
      const durationSeconds = Number(duration) || 0;
      setUpdateSourceVideos((prev) =>
        prev.map((item, idx) => {
          if (idx !== index) {
            return item;
          }
          const endTime = durationSeconds
            ? formatDurationHms(durationSeconds)
            : item.endTime;
          const startTime = normalizeTimeValue(item.startTime, durationSeconds) || "00:00:00";
          return {
            ...item,
            sourceType: "LOCAL",
            sourceFilePath: selected,
            remotePath: "",
            durationSeconds,
            startTime,
            endTime,
          };
        }),
      );
    } catch (error) {
      setMessage(error.message);
    }
  };

  const openUpdateImportModal = () => {
    if (!String(updateTaskId || "").trim()) {
      setMessage("任务ID无效");
      return;
    }
    if (!String(updateTaskDir || "").trim()) {
      setMessage("任务目录未就绪，请重新进入视频更新页面后重试");
      return;
    }
    setUpdateImportOpen(true);
    setUpdateImportInput("");
    setUpdateImportLoading(false);
    setUpdateImportResults([]);
    setUpdateImportSelection(new Set());
    setMessage("");
  };

  const updateTotalClipSeconds = updateSourceVideos.reduce((acc, item) => {
    const start = parseHmsToSeconds(item.startTime) ?? 0;
    const endRaw = parseHmsToSeconds(item.endTime) ?? 0;
    const end = clampTimeSeconds(endRaw, item.durationSeconds);
    const clipped = Math.max(0, end - clampTimeSeconds(start, item.durationSeconds));
    return acc + clipped;
  }, 0);

  const updateSegmentDurationSeconds =
    Number(updateWorkflowConfig.segmentationConfig.segmentDurationSeconds) || 0;

  const updateEstimatedSegments =
    updateSegmentationEnabled && updateSegmentDurationSeconds > 0
      ? Math.ceil(updateTotalClipSeconds / updateSegmentDurationSeconds)
      : 0;

  if (!(isUpdateView)) {
    return null;
  }

  return (
    <div className="rounded-2xl bg-white/80 p-6 shadow-sm ring-1 ring-black/5">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div>
              <div className="text-sm font-semibold uppercase tracking-[0.2em] text-[var(--muted)]">
                视频更新
              </div>
              <div className="mt-1 text-xs text-[var(--muted)]">
                支持本地文件与远程分P混合配置，远程源视频会在下载完成后自动继续更新流程。
              </div>
            </div>
            <button
              className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
              onClick={backToList}
            >
              返回列表
            </button>
          </div>
          <div className="mt-4">
            {message ? (
              <div className="rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-700">
                {message}
              </div>
            ) : null}
            <div className="mt-4 grid gap-4">
              <div className="rounded-xl border border-black/5 bg-white/80 p-3">
                <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
                  更新配置
                </div>
                <div className="mt-3 space-y-3 text-sm text-[var(--ink)]">
                  <div className="grid gap-2 lg:grid-cols-2">
                    <div className="space-y-1">
                      <div className="text-xs text-[var(--muted)]">分段前缀（可选）</div>
                      <input
                        value={updateSegmentPrefix}
                        onChange={(event) => setUpdateSegmentPrefix(event.target.value)}
                        placeholder="分段前缀"
                        className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
                      />
                    </div>
                  </div>
                  <div className="rounded-lg border border-black/5 bg-white/80 p-3">
                    <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
                      百度网盘同步
                    </div>
                    <label className="mt-2 flex items-center gap-2 text-sm text-[var(--muted)]">
                      <input
                        type="checkbox"
                        checked={updateBaiduSync.enabled}
                        onChange={(event) =>
                          setUpdateBaiduSync((prev) => ({
                            ...prev,
                            enabled: event.target.checked,
                          }))
                        }
                      />
                      同步上传到百度网盘
                    </label>
                    {updateBaiduSync.enabled ? (
                      <div className="mt-3 grid gap-2 lg:grid-cols-2">
                        <div>
                          <div className="text-xs text-[var(--muted)]">远端路径</div>
                          <div className="mt-2 flex flex-wrap items-center gap-2 text-xs">
                            <div className="flex-1 rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-[var(--ink)]">
                              {updateBaiduSync.path || defaultBaiduSyncPath || "/录播"}
                            </div>
                            <button
                              className="rounded-full border border-black/10 bg-white px-3 py-1 font-semibold text-[var(--ink)]"
                              onClick={() => handleOpenSyncPicker("update")}
                            >
                              选择目录
                            </button>
                          </div>
                        </div>
                        <div>
                          <div className="text-xs text-[var(--muted)]">上传文件名</div>
                          <input
                            value={updateBaiduSync.filename}
                            onChange={(event) =>
                              setUpdateBaiduSync((prev) => ({
                                ...prev,
                                filename: event.target.value,
                              }))
                            }
                            placeholder="文件名"
                            className="mt-2 w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
                          />
                        </div>
                      </div>
                    ) : null}
                  </div>
                  <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
                    是否分段
                  </div>
                  <div className="flex flex-wrap gap-4 text-xs text-[var(--muted)]">
                    <label className="flex items-center gap-2">
                      <input
                        type="radio"
                        checked={updateSegmentationEnabled}
                        onChange={() => setUpdateSegmentationEnabled(true)}
                      />
                      需要分段
                    </label>
                    <label className="flex items-center gap-2">
                      <input
                        type="radio"
                        checked={!updateSegmentationEnabled}
                        onChange={() => setUpdateSegmentationEnabled(false)}
                      />
                      不需要分段
                    </label>
                  </div>
                  {updateSegmentationEnabled ? (
                    <div className="grid gap-2 lg:grid-cols-2">
                      <input
                        type="number"
                        value={updateWorkflowConfig.segmentationConfig.segmentDurationSeconds}
                        onChange={(event) =>
                          setUpdateWorkflowConfig((prev) => ({
                            ...prev,
                            segmentationConfig: {
                              ...prev.segmentationConfig,
                              segmentDurationSeconds: Number(event.target.value),
                            },
                          }))
                        }
                        placeholder="分段时长（秒）"
                        className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
                      />
                      <label className="flex items-center gap-2 text-xs text-[var(--muted)]">
                        <input
                          type="checkbox"
                          checked={updateWorkflowConfig.segmentationConfig.preserveOriginal}
                          onChange={(event) =>
                            setUpdateWorkflowConfig((prev) => ({
                              ...prev,
                              segmentationConfig: {
                                ...prev.segmentationConfig,
                                preserveOriginal: event.target.checked,
                              },
                            }))
                          }
                        />
                        保留合并视频
                      </label>
                    </div>
                  ) : null}
                  <div className="text-xs text-[var(--muted)]">
                    预计分段数：
                    {updateSegmentationEnabled ? updateEstimatedSegments : "不分段"}
                  </div>
                </div>
              </div>
              <div className="rounded-xl border border-black/5 bg-white/80 p-3">
                <div className="flex flex-wrap items-center justify-between gap-3">
                  <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
                    源视频配置
                  </div>
                  <div className="flex flex-wrap gap-2">
                    <button
                      className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                      onClick={openUpdateImportModal}
                    >
                      导入视频
                    </button>
                    <button
                      className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                      onClick={createUpdateMergeGroup}
                    >
                      创建合并组
                    </button>
                    <button
                      className="rounded-full bg-[var(--accent)] px-3 py-1 text-xs font-semibold text-white"
                      onClick={addUpdateSource}
                    >
                      添加视频
                    </button>
                  </div>
                </div>
                <div className="mt-3 overflow-hidden rounded-xl border border-black/5">
                  <table className="w-full text-left text-sm">
                    <thead className="bg-black/5 text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
                      <tr>
                        <th className="px-4 py-2">序号</th>
                        <th className="px-4 py-2">视频文件（必填）</th>
                        <th className="px-4 py-2">BV号或下载链接</th>
                        <th className="px-4 py-2">分P标题</th>
                        <th className="px-4 py-2">上传分P标题</th>
                        <th className="px-4 py-2">开始时间</th>
                        <th className="px-4 py-2">结束时间</th>
                        <th className="px-4 py-2">合并</th>
                        <th className="px-4 py-2">独立分组</th>
                        <th className="px-4 py-2">合并状态</th>
                        <th className="px-4 py-2">操作</th>
                      </tr>
                    </thead>
                    <tbody>
                      {updateSourceVideos.map((item, index) => (
                        <tr
                          key={item.localId || `update-source-${index}`}
                          className="border-t border-black/5"
                        >
                          <td className="px-4 py-2 text-[var(--muted)]">{index + 1}</td>
                          <td className="px-4 py-2">
                            <div className="flex flex-wrap gap-2">
                              <input
                                value={item.sourceFilePath}
                                onChange={(event) =>
                                  updateUpdateSource(index, "sourceFilePath", event.target.value)
                                }
                                placeholder="请输入视频文件路径（必填）"
                                className="w-full flex-1 rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
                              />
                              <button
                                className="rounded-lg border border-black/10 bg-white px-3 py-2 text-xs font-semibold text-[var(--ink)]"
                                onClick={() => handleUpdateSelectSourceFile(index)}
                              >
                                选择
                              </button>
                            </div>
                          </td>
                          <td className="px-4 py-2">
                            <div className="space-y-2">
                              <div className="flex flex-wrap gap-2">
                                <input
                                  value={resolveSourceRemoteInput(item)}
                                  onChange={(event) =>
                                    updateUpdateSource(index, "remoteBvid", event.target.value)
                                  }
                                  placeholder="可选，填写源视频 BV 号或下载链接"
                                  className="w-full flex-1 rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
                                />
                                <button
                                  type="button"
                                  className="rounded-lg border border-black/10 bg-white px-3 py-2 text-xs font-semibold text-[var(--ink)] disabled:cursor-not-allowed disabled:opacity-60"
                                  onClick={() =>
                                    isDirectSourceInput(resolveSourceRemoteInput(item))
                                      ? applySourceDownloadUrlBinding("update", index)
                                      : handleChooseSourceRemotePart("update", index)
                                  }
                                  disabled={sourcePartPickerLoadingId === item.localId}
                                >
                                  {sourcePartPickerLoadingId === item.localId
                                    ? "查询中"
                                    : isDirectSourceInput(resolveSourceRemoteInput(item))
                                      ? "保存"
                                      : "选择分P"}
                                </button>
                              </div>
                              {formatSourceBindingSummary(item) ? (
                                <div className="text-xs text-[var(--muted)]">
                                  {formatSourceBindingSummary(item)}
                                </div>
                              ) : null}
                            </div>
                          </td>
                          <td className="px-4 py-2 text-[var(--muted)]">
                            {item.remotePartTitle ||
                              (item.remoteBvid
                                ? "未指定分P"
                                : item.remoteVideoUrl
                                  ? "下载链接"
                                  : "-")}
                          </td>
                          <td className="px-4 py-2">
                            <input
                              value={
                                item.uploadPartTitleMode === "CUSTOM"
                                  ? item.uploadPartTitle || ""
                                  : ""
                              }
                              onChange={(event) =>
                                updateUpdateSourceUploadTitle(index, event.target.value)
                              }
                              maxLength={80}
                              placeholder={
                                updateSegmentationEnabled ? "分段开启时自动分P" : `P${index + 1}`
                              }
                              title={
                                updateSegmentationEnabled ? "分段开启时使用自动分P标题" : ""
                              }
                              disabled={updateSegmentationEnabled}
                              className="w-32 rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none disabled:cursor-not-allowed disabled:bg-black/5"
                            />
                          </td>
                          <td className="px-4 py-2">
                            <input
                              value={item.startTime}
                              onChange={(event) =>
                                updateUpdateSourceTime(index, "startTime", event.target.value)
                              }
                              onBlur={() => normalizeUpdateSourceTime(index, "startTime")}
                              placeholder="00:00:00"
                              className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
                            />
                          </td>
                          <td className="px-4 py-2">
                            <input
                              value={item.endTime}
                              onChange={(event) =>
                                updateUpdateSourceTime(index, "endTime", event.target.value)
                              }
                              onBlur={() => normalizeUpdateSourceTime(index, "endTime")}
                              placeholder="00:00:00"
                              className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
                            />
                          </td>
                          <td className="px-4 py-2">
                            <input
                              type="checkbox"
                              checked={updateMergeSelection.has(item.localId)}
                              onChange={(event) =>
                                toggleUpdateMergeSelection(item.localId, event.target.checked)
                              }
                              disabled={
                                buildGroupedSourceIdSet(updateMergeItems).has(item.localId) ||
                                updateMergeItems.some(
                                  (candidate) =>
                                    candidate.type === "SOURCE" &&
                                    candidate.sourceId === item.localId &&
                                    candidate.standalone,
                                )
                              }
                            />
                          </td>
                          <td className="px-4 py-2">
                            <input
                              type="checkbox"
                              title="独立分组"
                              checked={
                                updateMergeItems.find(
                                  (candidate) =>
                                    candidate.type === "SOURCE" &&
                                    candidate.sourceId === item.localId,
                                )?.standalone || false
                              }
                              onChange={() => toggleUpdateMergeStandalone(item.localId)}
                              disabled={buildGroupedSourceIdSet(updateMergeItems).has(item.localId)}
                            />
                          </td>
                          <td className="px-4 py-2 text-[var(--muted)]">
                            {buildGroupedSourceIdSet(updateMergeItems).has(item.localId)
                              ? "已加入合并组"
                              : updateMergeItems.find(
                                    (candidate) =>
                                      candidate.type === "SOURCE" &&
                                      candidate.sourceId === item.localId &&
                                      candidate.standalone,
                                  )?.standalone
                                ? "独立分组"
                                : "未配置"}
                          </td>
                          <td className="px-4 py-2">
                            <div className="flex flex-wrap gap-2">
                              <button
                                className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                                onClick={() => duplicateUpdateSource(index)}
                              >
                                复制
                              </button>
                              <button
                                className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                                onClick={() => removeUpdateSource(index)}
                              >
                                删除
                              </button>
                            </div>
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
                <div className="mt-3 rounded-xl border border-black/5 bg-white/80 p-3">
                  <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
                    合并配置
                  </div>
                  <div className="mt-2 space-y-2 text-xs text-[var(--ink)]">
                    {updateMergeItems.filter((item) => item.type === "GROUP").length === 0 ? (
                      <div className="text-[var(--muted)]">当前未创建合并组</div>
                    ) : (
                      updateMergeItems
                        .filter((item) => item.type === "GROUP")
                        .map((group, groupIndex) => {
                          const sourceNames = (group.sourceIds || [])
                            .map((sourceId) => {
                              const source = updateSourceVideos.find((item) => item.localId === sourceId);
                              return (
                                String(source?.remotePartTitle || "").trim() ||
                                String(source?.sourceFilePath || "").split(/[\\/]/).pop() ||
                                sourceId
                              );
                            })
                            .join("，");
                          return (
                            <div
                              key={group.id}
                              className="flex flex-wrap items-center justify-between gap-2 rounded-lg border border-black/10 bg-white px-3 py-2"
                            >
                              <div>
                                <div className="font-semibold text-[var(--ink)]">
                                  合并组 {groupIndex + 1}
                                </div>
                                <div className="text-[var(--muted)]">{sourceNames || "-"}</div>
                              </div>
                              <button
                                className="rounded-full border border-black/10 bg-white px-3 py-1 font-semibold text-[var(--ink)]"
                                onClick={() => releaseUpdateMergeGroup(group.id)}
                              >
                                解除合并
                              </button>
                            </div>
                          );
                        })
                    )}
                  </div>
                </div>
              </div>
            </div>
            <div className="mt-4 flex flex-wrap gap-2">
              <button
                className="rounded-full bg-[var(--accent)] px-4 py-2 text-sm font-semibold text-white shadow-sm transition hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-60"
                onClick={handleUpdateSubmit}
                disabled={updateSubmitting}
              >
                {updateSubmitting ? "提交中" : "提交更新"}
              </button>
            </div>
          </div>
        </div>
  );
}
