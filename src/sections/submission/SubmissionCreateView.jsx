import { useState } from "react";
import {
  buildCollectionTreeValue,
  buildMergeGroupId,
  buildSourceVideoPayload,
  formatSourceBindingSummary,
  isDirectSourceInput,
  resolveSourceRemoteInput,
} from "./helpers";
import { validateSourceVideos } from "../../lib/submissionEditDiff";
import { invokeCommand } from "../../lib/tauri";
import { showErrorDialog } from "../../lib/dialog";
import LoadingButton from "../../components/LoadingButton";

export default function SubmissionCreateView({
  addSource,
  applySourceDownloadUrlBinding,
  applyUploadTitle,
  backToList,
  buildGroupedSourceIdSet,
  buildMergeGroupsPayload,
  buildPartitionOptionValue,
  collectionOptions,
  collectionTreeSections,
  collectionTreeValue,
  collections,
  coverPreviewSrc,
  currentUpProfile,
  defaultBaiduSyncPath,
  draggingMergeGroupSource,
  draggingMergeItemId,
  duplicateSource,
  estimatedSegments,
  handleChooseSourceRemotePart,
  handleClearCover,
  handleCollectionTreeChange,
  handleOpenSyncPicker,
  handlePartitionChange,
  handleSelectCoverFile,
  handleTagKeyDown,
  isActivityTag,
  isCreateView,
  isDetailView,
  isVideoFilePath,
  loadTasks,
  mergeDragStateRef,
  mergeGroupDragStateRef,
  mergeItems,
  mergeSelection,
  message,
  normalizeSourceTime,
  openFileDialog,
  parseHmsToSeconds,
  partitionOptions,
  partitionSelectValue,
  removeSource,
  removeTag,
  renderActivityTopicSelector,
  resolveTagClassName,
  resolveTagRemoveClassName,
  segmentationEnabled,
  selectedBilibiliUid,
  setDraggingMergeGroupSource,
  setDraggingMergeItemId,
  setMergeItems,
  setMergeSelection,
  setMessage,
  setQuickFillOpen,
  setQuickFillPage,
  setQuickFillTasks,
  setQuickFillTotal,
  setSegmentationEnabled,
  setSourceRemotePickerIndex,
  setSourceRemotePickerOpen,
  setSourceRemotePickerTarget,
  setSubmissionView,
  setTagInput,
  setTaskForm,
  setWorkflowConfig,
  sourcePartPickerLoadingId,
  sourceVideos,
  syncMergeItemsWithSources,
  tagInput,
  tags,
  taskForm,
  updateSource,
  updateSourceTime,
  updateSourceUploadTitle,
  uploadTitleHints,
  workflowConfig,
}) {
  const [createSubmitting, setCreateSubmitting] = useState(false);

  const isReadOnly = isDetailView;

  const groupedSourceIds = buildGroupedSourceIdSet(mergeItems);

  const openSourceRemotePicker = (target, index) => {
    setSourceRemotePickerTarget(target);
    setSourceRemotePickerIndex(index);
    setSourceRemotePickerOpen(true);
  };

  const updateMergeGroupTitle = (groupId, value) => {
    setMergeItems((prev) =>
      prev.map((item) =>
        item.type === "GROUP" && item.id === groupId
          ? applyUploadTitle(item, value)
          : item,
      ),
    );
  };

  const toggleMergeSelection = (sourceId, checked, items = mergeItems) => {
    const grouped = buildGroupedSourceIdSet(items);
    if (grouped.has(sourceId)) {
      return;
    }
    const item = items.find((i) => i.type === "SOURCE" && i.sourceId === sourceId);
    if (item?.standalone) {
      return;
    }
    setMergeSelection((prev) => {
      const next = new Set(prev);
      if (checked) {
        next.add(sourceId);
      } else {
        next.delete(sourceId);
      }
      return next;
    });
  };

  const toggleMergeStandalone = (sourceId) => {
    setMergeItems((prev) =>
      prev.map((item) => {
        if (item.type !== "SOURCE" || item.sourceId !== sourceId) {
          return item;
        }
        const next = { ...item, standalone: !item.standalone };
        if (next.standalone) {
          setMergeSelection((sel) => {
            const s = new Set(sel);
            s.delete(sourceId);
            return s;
          });
        }
        return next;
      }),
    );
  };

  const createMergeGroup = () => {
    const selected = Array.from(mergeSelection);
    if (selected.length < 2) {
      setMessage("请至少选择两个源视频进行合并");
      return;
    }
    const selectedSet = new Set(selected);
    const orderedSelected = sourceVideos
      .map((item) => item.localId)
      .filter((id) => selectedSet.has(id));
    if (orderedSelected.length < 2) {
      setMessage("请选择有效的源视频进行合并");
      return;
    }
    const groupId = buildMergeGroupId();
    setMergeItems((prev) => {
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
    setMergeSelection(new Set());
  };

  const releaseMergeGroup = (groupId) => {
    const normalizedGroupId = String(groupId || "").trim();
    if (!normalizedGroupId) {
      return;
    }
    setMergeItems((prev) => {
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
        setMergeSelection((selection) => {
          const filtered = new Set(selection);
          releasedIds.forEach((id) => filtered.delete(id));
          return filtered;
        });
      }
      return syncMergeItemsWithSources(sourceVideos, next);
    });
  };

  const handleMergeItemPointerDown = (event, itemId) => {
    if (event.button !== 0) {
      return;
    }
    const normalizedId = String(itemId || "").trim();
    if (!normalizedId) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    if (event.currentTarget?.setPointerCapture) {
      event.currentTarget.setPointerCapture(event.pointerId);
    }
    mergeDragStateRef.current = { activeId: normalizedId, overId: normalizedId };
    setDraggingMergeItemId(normalizedId);
  };

  const handleMergeGroupSourcePointerDown = (event, groupId, sourceId) => {
    if (event.button !== 0) {
      return;
    }
    const normalizedGroupId = String(groupId || "").trim();
    const normalizedSourceId = String(sourceId || "").trim();
    if (!normalizedGroupId || !normalizedSourceId) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    if (event.currentTarget?.setPointerCapture) {
      event.currentTarget.setPointerCapture(event.pointerId);
    }
    mergeGroupDragStateRef.current = {
      groupId: normalizedGroupId,
      activeId: normalizedSourceId,
      overId: normalizedSourceId,
    };
    setDraggingMergeGroupSource({ groupId: normalizedGroupId, sourceId: normalizedSourceId });
  };

  const resolveSourceIndexById = (sourceId) =>
    sourceVideos.findIndex((item) => item.localId === sourceId);

  const renderMergeSourceRow = ({
    sourceId,
    displayIndex,
    groupId = "",
    mergeItemId = "",
  }) => {
    const sourceIndex = resolveSourceIndexById(sourceId);
    if (sourceIndex < 0) {
      return null;
    }
    const source = sourceVideos[sourceIndex];
    const isGroupedRow = Boolean(groupId);
    const activeMergeId = mergeItemId || sourceId;
    const mergeItem = !isGroupedRow ? mergeItems.find((i) => i.type === "SOURCE" && i.sourceId === sourceId) : null;
    const isStandalone = mergeItem?.standalone ?? false;
    const isDragging =
      isGroupedRow
        ? draggingMergeGroupSource.groupId === groupId &&
          draggingMergeGroupSource.sourceId === sourceId
        : draggingMergeItemId === activeMergeId;
    const dragHandle = isReadOnly ? null : (
      <span
        className="cursor-grab select-none text-[var(--muted)]"
        onPointerDown={(event) => {
          if (isGroupedRow) {
            handleMergeGroupSourcePointerDown(event, groupId, sourceId);
            return;
          }
          handleMergeItemPointerDown(event, activeMergeId);
        }}
        style={{ touchAction: "none" }}
      >
        ≡
      </span>
    );

    return (
      <tr
        key={`${groupId || "single"}-${sourceId}`}
        data-merge-item-id={!isGroupedRow ? activeMergeId : undefined}
        data-merge-group-id={isGroupedRow ? groupId : undefined}
        data-merge-source-id={isGroupedRow ? sourceId : undefined}
        className={`border-t border-black/5 ${isDragging ? "bg-black/5" : ""}`}
      >
        <td className="px-4 py-2 text-[var(--muted)]">
          <div className="flex items-center gap-2">
            {dragHandle}
            <span>{displayIndex}</span>
          </div>
        </td>
        <td className="px-4 py-2">
          <input
            type="checkbox"
            checked={mergeSelection.has(sourceId)}
            onChange={(event) => toggleMergeSelection(sourceId, event.target.checked)}
            disabled={isReadOnly || groupedSourceIds.has(sourceId) || isStandalone}
          />
        </td>
        <td className="px-4 py-2">
          {!isGroupedRow ? (
            <input
              type="checkbox"
              title="独立分组"
              checked={isStandalone}
              onChange={() => toggleMergeStandalone(sourceId)}
              disabled={isReadOnly}
            />
          ) : (
            <input type="checkbox" disabled checked={false} title="合并组内不可独立分组" />
          )}
        </td>
        <td className="px-4 py-2">
          <div className="flex flex-wrap gap-2">
            <input
              value={source.sourceFilePath}
              onChange={(event) => updateSource(sourceIndex, "sourceFilePath", event.target.value)}
              placeholder="请输入视频文件路径（必填）"
              readOnly={isReadOnly}
              className="w-full flex-1 rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
            />
            {!isReadOnly ? (
              <>
                <button
                  className="rounded-lg border border-black/10 bg-white px-3 py-2 text-xs font-semibold text-[var(--ink)]"
                  onClick={() => openFileDialog(sourceIndex)}
                >
                  本地
                </button>
                <button
                  type="button"
                  className="rounded-lg border border-black/10 bg-white px-3 py-2 text-xs font-semibold text-[var(--ink)]"
                  onClick={() => openSourceRemotePicker("create", sourceIndex)}
                >
                  网盘
                </button>
              </>
            ) : null}
          </div>
        </td>
        <td className="px-4 py-2">
          <div className="space-y-2">
            <div className="flex flex-wrap gap-2">
              <input
                value={resolveSourceRemoteInput(source)}
                onChange={(event) => updateSource(sourceIndex, "remoteBvid", event.target.value)}
                placeholder="可选，填写源视频 BV 号或下载链接"
                readOnly={isReadOnly}
                className="w-full flex-1 rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
              />
              {!isReadOnly ? (
                <button
                  type="button"
                  className="rounded-lg border border-black/10 bg-white px-3 py-2 text-xs font-semibold text-[var(--ink)] disabled:cursor-not-allowed disabled:opacity-60"
                  onClick={() =>
                    isDirectSourceInput(resolveSourceRemoteInput(source))
                      ? applySourceDownloadUrlBinding("create", sourceIndex)
                      : handleChooseSourceRemotePart("create", sourceIndex)
                  }
                  disabled={sourcePartPickerLoadingId === source.localId}
                >
                  {sourcePartPickerLoadingId === source.localId
                    ? "查询中"
                    : isDirectSourceInput(resolveSourceRemoteInput(source))
                      ? "保存"
                      : "选择分P"}
                </button>
              ) : null}
            </div>
            {formatSourceBindingSummary(source) ? (
              <div className="text-xs text-[var(--muted)]">
                {formatSourceBindingSummary(source)}
              </div>
            ) : null}
          </div>
        </td>
        <td className="px-4 py-2 text-[var(--muted)]">
          {source.sourceType === "BAIDU"
            ? source.remotePath || "网盘视频"
            : source.remotePartTitle ||
            (source.remoteBvid
              ? "未指定分P"
              : source.remoteVideoUrl
                ? "下载链接"
                : "-")}
        </td>
        <td className="px-4 py-2">
          {isGroupedRow ? (
            <span className="text-xs text-[var(--muted)]">见合并组标题</span>
          ) : (
            <input
              value={source.uploadPartTitleMode === "CUSTOM" ? source.uploadPartTitle || "" : ""}
              onChange={(event) => updateSourceUploadTitle(sourceIndex, event.target.value)}
              maxLength={80}
              placeholder={
                segmentationEnabled
                  ? "分段开启时自动分P"
                  : uploadTitleHints.sourceTitleMap.get(sourceId) || "P1"
              }
              title={segmentationEnabled ? "分段开启时使用自动分P标题" : ""}
              readOnly={isReadOnly}
              disabled={segmentationEnabled}
              className="w-32 rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none disabled:cursor-not-allowed disabled:bg-black/5"
            />
          )}
        </td>
        <td className="px-4 py-2">
          <input
            value={source.startTime}
            onChange={(event) => updateSourceTime(sourceIndex, "startTime", event.target.value)}
            onBlur={() => normalizeSourceTime(sourceIndex, "startTime")}
            placeholder="00:00:00"
            readOnly={isReadOnly}
            className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
          />
        </td>
        <td className="px-4 py-2">
          <input
            value={source.endTime}
            onChange={(event) => updateSourceTime(sourceIndex, "endTime", event.target.value)}
            onBlur={() => normalizeSourceTime(sourceIndex, "endTime")}
            placeholder="00:00:00"
            readOnly={isReadOnly}
            className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
          />
        </td>
        <td className="px-4 py-2 text-[var(--muted)]">
          {groupedSourceIds.has(sourceId) ? "已合并" : "未合并"}
        </td>
        <td className="px-4 py-2">
          {!isReadOnly ? (
            <div className="flex flex-wrap gap-2">
              <button
                className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                onClick={() => duplicateSource(sourceIndex)}
              >
                复制
              </button>
              <button
                className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                onClick={() => removeSource(sourceIndex)}
              >
                删除
              </button>
            </div>
          ) : null}
        </td>
      </tr>
    );
  };

  const buildWorkflowConfig = (mergeGroups) => {
    const config = {
      enableSegmentation: segmentationEnabled,
      segmentationConfig: {
        enabled: segmentationEnabled,
        segmentDurationSeconds: workflowConfig.segmentationConfig.segmentDurationSeconds,
        preserveOriginal: workflowConfig.segmentationConfig.preserveOriginal,
      },
    };
    if (Array.isArray(mergeGroups) && mergeGroups.length > 0) {
      config.mergeGroups = mergeGroups;
    }
    return config;
  };

  const handleCreate = async () => {
    if (createSubmitting) {
      return;
    }
    setMessage("");
    if (!taskForm.title.trim()) {
      setMessage("请输入投稿标题");
      return;
    }
    if (taskForm.title.length > 80) {
      setMessage("投稿标题不能超过 80 个字符");
      return;
    }
    if (!taskForm.partitionId) {
      setMessage("请选择B站分区");
      return;
    }
    if (!taskForm.videoType) {
      setMessage("请选择视频类型");
      return;
    }
    if (taskForm.description && taskForm.description.length > 2000) {
      setMessage("视频描述不能超过 2000 个字符");
      return;
    }
    const normalizedTags = [...tags];
    if (tagInput.trim()) {
      normalizedTags.push(tagInput.trim());
    }
    const uniqueTags = Array.from(new Set(normalizedTags));
    if (uniqueTags.length === 0) {
      setMessage("请填写至少一个投稿标签");
      return;
    }
    if (segmentationEnabled) {
      const segmentDuration = workflowConfig.segmentationConfig.segmentDurationSeconds;
      if (segmentDuration < 30 || segmentDuration > 600) {
        setMessage("分段时长必须在 30-600 秒之间");
        return;
      }
    }
    const sourceValidation = validateSourceVideos({
      items: sourceVideos,
      parseHmsToSeconds,
      isVideoFilePath,
    });
    if (!sourceValidation.valid) {
      setMessage(sourceValidation.message);
      return;
    }
    const validSources = sourceValidation.validSources;
    try {
      const auth = await invokeCommand("auth_status");
      if (!auth?.loggedIn) {
        setMessage("请先登录B站账号");
        return;
      }
      const targetBilibiliUid = Number(selectedBilibiliUid || auth?.activeAccount?.userId || 0);
      if (!Number.isFinite(targetBilibiliUid) || targetBilibiliUid <= 0) {
        setMessage("请选择投稿账号");
        return;
      }
      if (taskForm.baiduSyncEnabled) {
        const binding = await invokeCommand("account_binding_get", {
          bilibiliUid: targetBilibiliUid,
        });
        if (!binding?.baiduUid) {
          setMessage("当前投稿账号未绑定网盘账号");
          return;
        }
      }
    } catch (error) {
      setMessage(error.message || "登录状态校验失败");
      return;
    }
    setCreateSubmitting(true);
    try {
      const selectedCollectionId = (() => {
        const raw = String(taskForm.collectionId || "").trim();
        if (!raw) {
          return null;
        }
        const exists = collections.some(
          (collection) => String(collection.seasonId) === raw,
        );
        if (!exists) {
          return null;
        }
        const parsed = Number(raw);
        return Number.isFinite(parsed) && parsed > 0 ? parsed : null;
      })();
      const selectedCollectionSectionId = (() => {
        if (!selectedCollectionId) {
          return null;
        }
        const raw = String(taskForm.collectionSectionId || "").trim();
        if (!raw) {
          return null;
        }
        const collection = collections.find(
          (item) => Number(item.seasonId) === selectedCollectionId,
        );
        const exists = (collection?.sections || []).some(
          (section) => String(section.sectionId) === raw,
        );
        if (!exists) {
          return null;
        }
        const parsed = Number(raw);
        return Number.isFinite(parsed) && parsed > 0 ? parsed : null;
      })();
      const mergeGroups = buildMergeGroupsPayload(validSources);
      const payload = {
        request: {
            task: {
              title: taskForm.title,
              description: taskForm.description || null,
            coverUrl: taskForm.coverUrl || null,
            coverDataUrl: taskForm.coverDataUrl || null,
            partitionId: Number(taskForm.partitionId),
            collectionId: selectedCollectionId,
            collectionSectionId: selectedCollectionSectionId,
            tags: uniqueTags.join(","),
            topicId: taskForm.activityTopicId ? Number(taskForm.activityTopicId) : null,
            missionId: taskForm.activityMissionId ? Number(taskForm.activityMissionId) : null,
            activityTitle: taskForm.activityTitle || null,
            videoType: taskForm.videoType,
            bilibiliUid: selectedBilibiliUid ? Number(selectedBilibiliUid) : null,
            segmentPrefix: taskForm.segmentPrefix || null,
            immediateSubmit: Boolean(taskForm.immediateSubmit),
            priority: Boolean(taskForm.priority),
            baiduSyncEnabled: Boolean(taskForm.baiduSyncEnabled),
            baiduSyncPath: taskForm.baiduSyncPath || null,
            baiduSyncFilename: taskForm.baiduSyncFilename || null,
          },
          sourceVideos: validSources.map(buildSourceVideoPayload),
          workflowConfig: buildWorkflowConfig(mergeGroups),
        },
      };
      await invokeCommand("submission_create", payload);
      await loadTasks();
      setSubmissionView("list");
      setMessage("");
    } catch (error) {
      setMessage(error.message);
      await showErrorDialog(error);
    } finally {
      setCreateSubmitting(false);
    }
  };

  const openQuickFill = () => {
    if (isReadOnly) {
      return;
    }
    setMessage("");
    setQuickFillTasks([]);
    setQuickFillTotal(0);
    setQuickFillPage(1);
    setQuickFillOpen(true);
  };

  if (!(isCreateView)) {
    return null;
  }

  return (
    <>
          <div className="rounded-2xl bg-[var(--surface)]/90 p-6 shadow-sm ring-1 ring-black/5">
            <div className="flex flex-wrap items-center justify-between gap-3">
              <div>
                <p className="text-sm uppercase tracking-[0.2em] text-[var(--muted)]">
                  视频投稿
                </p>
                <h2 className="text-2xl font-semibold text-[var(--ink)]">新增投稿任务</h2>
              </div>
              <button
                className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                onClick={backToList}
              >
                返回列表
              </button>
            </div>
            <div className="mt-4 space-y-3">
              <div className="rounded-xl bg-white/70 px-3 py-2 text-sm text-[var(--content-color)]">
                当前投稿账号：
                <span className="ml-2 font-semibold">
                  {currentUpProfile?.name || "未选择"}
                </span>
                <span className="ml-2 text-xs text-[var(--muted)]">
                  UID {currentUpProfile?.uid || "—"}
                </span>
              </div>
              <div className="space-y-1">
                <div className="flex items-center justify-between text-xs text-[var(--muted)]">
                  <div>
                    投稿标题<span className="ml-1 text-rose-500">必填</span>
                  </div>
                  {!isReadOnly ? (
                    <button
                      className="rounded-full border border-black/10 bg-white px-2 py-1 text-[10px] font-semibold text-[var(--ink)]"
                      onClick={openQuickFill}
                    >
                      一键填写
                    </button>
                  ) : null}
                </div>
                <input
                  value={taskForm.title}
                  onChange={(event) =>
                    setTaskForm((prev) => ({ ...prev, title: event.target.value }))
                  }
                  placeholder="请输入投稿标题"
                  readOnly={isReadOnly}
                  className="w-full rounded-xl border border-black/10 bg-white/80 px-3 py-2 text-sm text-[var(--ink)] focus:border-[var(--accent)] focus:outline-none"
                />
              </div>
              <div className="space-y-1">
                <div className="text-xs text-[var(--muted)]">视频描述（可选）</div>
                <textarea
                  value={taskForm.description}
                  onChange={(event) =>
                    setTaskForm((prev) => ({ ...prev, description: event.target.value }))
                  }
                  placeholder="视频描述"
                  rows={2}
                  readOnly={isReadOnly}
                  className="w-full rounded-xl border border-black/10 bg-white/80 px-3 py-2 text-sm text-[var(--ink)] focus:border-[var(--accent)] focus:outline-none"
	                />
	              </div>
	              <div className="space-y-2">
	                <div className="text-xs text-[var(--muted)]">视频封面（本地裁剪上传）</div>
	                <div className="flex flex-wrap items-center gap-3">
	                  <div className="h-[120px] w-[192px] overflow-hidden rounded-lg border border-black/10 bg-black/5">
	                    {coverPreviewSrc ? (
	                      <img
	                        src={coverPreviewSrc}
	                        alt="封面预览"
	                        className="h-full w-full object-cover"
	                      />
	                    ) : (
	                      <div className="flex h-full items-center justify-center text-xs text-[var(--muted)]">
	                        暂无封面
	                      </div>
	                    )}
	                  </div>
	                  <div className="flex flex-wrap gap-2">
	                    <button
	                      type="button"
	                      className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
	                      onClick={handleSelectCoverFile}
	                    >
	                      选择封面
	                    </button>
	                    <button
	                      type="button"
	                      className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)] disabled:opacity-60"
	                      onClick={handleClearCover}
	                      disabled={!coverPreviewSrc}
	                    >
	                      清空封面
	                    </button>
	                  </div>
	                </div>
	                <div className="text-xs text-[var(--muted)]">
	                  裁剪比例固定 16:10，建议最小尺寸 960x600
	                </div>
	              </div>
			          <div className="grid gap-2 lg:grid-cols-3">
            <div className="space-y-1">
              <div className="text-xs text-[var(--muted)]">
                B站分区<span className="ml-1 text-rose-500">必填</span>
              </div>
              <select
                value={partitionSelectValue}
                onChange={(event) => handlePartitionChange(event.target.value)}
                disabled={isReadOnly}
                className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
              >
                <option value="">请选择分区</option>
                {partitionOptions.map((partition) => (
                  <option
                    key={partition.tid}
                    value={buildPartitionOptionValue(partition)}
                  >
                    {partition.name}
                  </option>
                ))}
              </select>
            </div>
            <div className="space-y-1">
              <div className="text-xs text-[var(--muted)]">合集（可选）</div>
              <select
                value={collectionTreeValue}
                onChange={(event) => handleCollectionTreeChange(event.target.value)}
                disabled={isReadOnly}
                className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
              >
                <option value="">请选择合集</option>
                {collectionOptions.flatMap((collection) => [
                  <option
                    key={`collection-${collection.seasonId}`}
                    value={buildCollectionTreeValue(collection.seasonId)}
                  >
                    {collection.name}
                  </option>,
                  ...collectionTreeSections(collection).map((section) => (
                    <option
                      key={`section-${collection.seasonId}-${section.sectionId}`}
                      value={buildCollectionTreeValue(collection.seasonId, section.sectionId)}
                    >
                      {`　└ ${section.title || `子合集${section.sectionId}`}`}
                    </option>
                  )),
                ])}
              </select>
            </div>
            <div className="space-y-1">
              <div className="text-xs text-[var(--muted)]">
                视频类型<span className="ml-1 text-rose-500">必填</span>
              </div>
              <select
                value={taskForm.videoType}
                onChange={(event) =>
                  setTaskForm((prev) => ({ ...prev, videoType: event.target.value }))
                }
                disabled={isReadOnly}
                className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
              >
                <option value="ORIGINAL">原创</option>
                <option value="REPOST">转载</option>
              </select>
            </div>
          </div>
          <div className="grid gap-2 lg:grid-cols-2">
            <div className="space-y-1">
              <div className="text-xs text-[var(--muted)]">
                投稿标签<span className="ml-1 text-rose-500">必填</span>
              </div>
              <div className="rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus-within:border-[var(--accent)]">
                <div className="flex flex-wrap gap-2">
                  {tags.map((tag) => (
                    <span
                      key={tag}
                      className={resolveTagClassName(tag)}
                    >
                      {isActivityTag(tag) ? `#话题 ${tag}` : tag}
                      {!isReadOnly ? (
                        <button
                          className={resolveTagRemoveClassName(tag)}
                          onClick={() => removeTag(tag)}
                          title="删除标签"
                        >
                          ×
                        </button>
                      ) : null}
                    </span>
                  ))}
                  {isReadOnly ? null : (
                    <input
                      value={tagInput}
                      onChange={(event) => setTagInput(event.target.value)}
                      onKeyDown={handleTagKeyDown}
                      placeholder="回车添加标签"
                      className="min-w-[120px] flex-1 bg-transparent text-sm text-[var(--ink)] focus:outline-none"
                    />
                  )}
                </div>
              </div>
              {isReadOnly ? null : renderActivityTopicSelector()}
            </div>
            <div className="space-y-1">
              <div className="text-xs text-[var(--muted)]">分段前缀（可选）</div>
              <input
                value={taskForm.segmentPrefix}
                onChange={(event) =>
                  setTaskForm((prev) => ({ ...prev, segmentPrefix: event.target.value }))
                }
                placeholder="分段前缀"
                readOnly={isReadOnly}
                className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
              />
            </div>
          </div>
          <label className="flex items-center gap-2 text-sm text-[var(--muted)]">
            <input
              type="checkbox"
              checked={taskForm.priority}
              onChange={(event) =>
                setTaskForm((prev) => ({
                  ...prev,
                  priority: event.target.checked,
                }))
              }
              disabled={isReadOnly}
            />
            优先投稿（按最近设置时间参与队列优先级）
          </label>
          {!isReadOnly ? (
            <div className="space-y-2 rounded-xl border border-black/5 bg-white/80 p-3">
              <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
                是否立即投稿
              </div>
              <div className="flex flex-wrap gap-4 text-sm text-[var(--muted)]">
                <label className="flex items-center gap-2">
                  <input
                    type="radio"
                    checked={taskForm.immediateSubmit}
                    onChange={() =>
                      setTaskForm((prev) => ({ ...prev, immediateSubmit: true }))
                    }
                  />
                  立即投稿
                </label>
                <label className="flex items-center gap-2">
                  <input
                    type="radio"
                    checked={!taskForm.immediateSubmit}
                    onChange={() =>
                      setTaskForm((prev) => ({ ...prev, immediateSubmit: false }))
                    }
                  />
                  暂不投稿
                </label>
              </div>
            </div>
          ) : null}
          <div className="text-xs text-[var(--muted)]">
            分段前缀会作为分段文件名的前缀（可选）
          </div>
          <div className="rounded-xl border border-black/5 bg-white/80 p-3">
            <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
              百度网盘同步
            </div>
            <label className="mt-2 flex items-center gap-2 text-sm text-[var(--muted)]">
              <input
                type="checkbox"
                checked={taskForm.baiduSyncEnabled}
                onChange={(event) =>
                  setTaskForm((prev) => ({
                    ...prev,
                    baiduSyncEnabled: event.target.checked,
                  }))
                }
                disabled={isReadOnly}
              />
              投稿完成后同步上传到百度网盘
            </label>
            {taskForm.baiduSyncEnabled ? (
              <div className="mt-3 grid gap-2 lg:grid-cols-2">
                <div>
                  <div className="text-xs text-[var(--muted)]">远端路径</div>
                  <div className="mt-2 flex flex-wrap items-center gap-2 text-xs">
                    <div className="flex-1 rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-[var(--ink)]">
                      {taskForm.baiduSyncPath || defaultBaiduSyncPath || "/录播"}
                    </div>
                    <button
                      className="rounded-full border border-black/10 bg-white px-3 py-1 font-semibold text-[var(--ink)]"
                      onClick={() => handleOpenSyncPicker("create")}
                      disabled={isReadOnly}
                    >
                      选择目录
                    </button>
                  </div>
                </div>
                <div>
                  <div className="text-xs text-[var(--muted)]">上传文件名</div>
                  <input
                    value={taskForm.baiduSyncFilename}
                    onChange={(event) =>
                      setTaskForm((prev) => ({
                        ...prev,
                        baiduSyncFilename: event.target.value,
                      }))
                    }
                    placeholder="文件名"
                    readOnly={isReadOnly}
                    className="mt-2 w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
                  />
                </div>
              </div>
            ) : null}
          </div>
          <div className="rounded-xl border border-black/5 bg-white/80 p-3">
            <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
              工作流配置
            </div>
            <div className="mt-2 space-y-3 text-sm text-[var(--ink)]">
              <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
                是否分段
              </div>
              <div className="flex flex-wrap gap-4 text-xs text-[var(--muted)]">
                <label className="flex items-center gap-2">
                  <input
                    type="radio"
                    checked={segmentationEnabled}
                    onChange={() => setSegmentationEnabled(true)}
                    disabled={isReadOnly}
                  />
                  需要分段
                </label>
                <label className="flex items-center gap-2">
                  <input
                    type="radio"
                    checked={!segmentationEnabled}
                    onChange={() => setSegmentationEnabled(false)}
                    disabled={isReadOnly}
                  />
                  不需要分段
                </label>
              </div>

              {segmentationEnabled ? (
                <div className="grid gap-2 lg:grid-cols-2">
                  <input
                    type="number"
                    value={workflowConfig.segmentationConfig.segmentDurationSeconds}
                    onChange={(event) =>
                      setWorkflowConfig((prev) => ({
                        ...prev,
                        segmentationConfig: {
                          ...prev.segmentationConfig,
                          segmentDurationSeconds: Number(event.target.value),
                        },
                      }))
                    }
                    placeholder="分段时长（秒）"
                    readOnly={isReadOnly}
                    className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
                  />
                  <label className="flex items-center gap-2 text-xs text-[var(--muted)]">
                    <input
                      type="checkbox"
                      checked={workflowConfig.segmentationConfig.preserveOriginal}
                      onChange={(event) =>
                        setWorkflowConfig((prev) => ({
                          ...prev,
                          segmentationConfig: {
                            ...prev.segmentationConfig,
                            preserveOriginal: event.target.checked,
                          },
                        }))
                      }
                      disabled={isReadOnly}
                    />
                    保留合并视频
                  </label>
                </div>
              ) : null}
              <div className="text-xs text-[var(--muted)]">
                预计分段数：{segmentationEnabled ? estimatedSegments : "不分段"}
              </div>
            </div>
          </div>
        </div>
      </div>

      <div className="rounded-2xl bg-white/80 p-6 shadow-sm ring-1 ring-black/5">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">源视频配置</div>
          {!isReadOnly ? (
            <div className="flex flex-wrap gap-2">
              <button
                className="rounded-full bg-[var(--accent)] px-3 py-1 text-xs font-semibold text-white"
                onClick={addSource}
              >
                添加视频
              </button>
              <button
                className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)] disabled:cursor-not-allowed disabled:opacity-60"
                onClick={createMergeGroup}
                disabled={mergeSelection.size < 2}
              >
                创建合并组
              </button>
            </div>
          ) : null}
        </div>
        <div className="mt-3 overflow-hidden rounded-xl border border-black/5">
          <table className="w-full text-left text-sm whitespace-nowrap">
            <thead className="bg-black/5 text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
              <tr>
                <th className="px-4 py-2">序号</th>
                <th className="px-4 py-2">选择</th>
                <th className="px-4 py-2">独立分组</th>
                          <th className="px-4 py-2">视频文件/网盘路径</th>
                <th className="px-4 py-2">BV号或下载链接</th>
                <th className="px-4 py-2">分P标题</th>
                <th className="px-4 py-2">上传分P标题</th>
                <th className="px-4 py-2">开始时间</th>
                <th className="px-4 py-2">结束时间</th>
                <th className="px-4 py-2">合并状态</th>
                <th className="px-4 py-2">操作</th>
              </tr>
            </thead>
            <tbody>
              {mergeItems.length === 0 ? (
                <tr>
                  <td className="px-4 py-3 text-[var(--muted)]" colSpan={11}>
                    暂无配置
                  </td>
                </tr>
              ) : (
                mergeItems.map((item, index) => {
                  if (item.type === "GROUP") {
                    const groupSources = (item.sourceIds || []).filter(
                      (sourceId) => resolveSourceIndexById(sourceId) >= 0,
                    );
                    if (groupSources.length === 0) {
                      return null;
                    }
                    return (
                      <tr
                        key={`merge-group-${item.id}`}
                        data-merge-item-id={item.id}
                        className="border-t border-black/5"
                      >
                        <td className="p-0" colSpan={11}>
                          <div className="m-2 overflow-hidden rounded-xl border-2 border-[var(--accent)]/40 bg-white/70">
                            <div
                              className={`flex items-center justify-between border-b border-[var(--accent)]/30 bg-[var(--accent)]/10 px-3 py-2 text-xs text-[var(--muted)] ${
                                isReadOnly ? "" : "cursor-grab"
                              }`}
                              onPointerDown={
                                isReadOnly
                                  ? undefined
                                  : (event) => handleMergeItemPointerDown(event, item.id)
                              }
                              style={{ touchAction: "none" }}
                            >
                              <div className="flex items-center gap-2">
                                {!isReadOnly ? (
                                  <span className="select-none text-[var(--ink)]">≡</span>
                                ) : null}
                                <span className="uppercase tracking-[0.2em]">
                                  合并组 {index + 1}
                                </span>
                                <span>{groupSources.length} 个视频</span>
                                <span className="text-[var(--muted)]">上传分P标题</span>
                                <input
                                  value={
                                    item.uploadPartTitleMode === "CUSTOM"
                                      ? item.uploadPartTitle || ""
                                      : ""
                                  }
                                  onChange={(event) =>
                                    updateMergeGroupTitle(item.id, event.target.value)
                                  }
                                  onPointerDown={(event) => event.stopPropagation()}
                                  maxLength={80}
                                  placeholder={
                                    segmentationEnabled
                                      ? "分段开启时自动分P"
                                      : uploadTitleHints.groupTitleMap.get(item.id) || "P1"
                                  }
                                  title={
                                    segmentationEnabled ? "分段开启时使用自动分P标题" : ""
                                  }
                                  readOnly={isReadOnly}
                                  disabled={segmentationEnabled}
                                  className="w-32 rounded-lg border border-black/10 bg-white/80 px-2 py-1 text-xs text-[var(--ink)] focus:border-[var(--accent)] focus:outline-none disabled:cursor-not-allowed disabled:bg-black/5"
                                />
                              </div>
                              {!isReadOnly ? (
                                <button
                                  type="button"
                                  className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                                  onPointerDown={(event) => event.stopPropagation()}
                                  onClick={(event) => {
                                    event.stopPropagation();
                                    releaseMergeGroup(item.id);
                                  }}
                                >
                                  解绑
                                </button>
                              ) : null}
                            </div>
                            <table className="w-full text-left text-sm whitespace-nowrap">
                              <tbody>
                                {groupSources.map((sourceId, innerIndex) =>
                                  renderMergeSourceRow({
                                    sourceId,
                                    displayIndex: `${index + 1}.${innerIndex + 1}`,
                                    groupId: item.id,
                                  }),
                                )}
                              </tbody>
                            </table>
                          </div>
                        </td>
                      </tr>
                    );
                  }
                  return renderMergeSourceRow({
                    sourceId: item.sourceId,
                    displayIndex: index + 1,
                    mergeItemId: item.id,
                  });
                })
              )}
            </tbody>
          </table>
        </div>
        {!isReadOnly ? (
          <div className="mt-4 flex flex-wrap gap-2">
            <LoadingButton
              className="rounded-full bg-[var(--accent)] px-4 py-2 text-sm font-semibold text-white shadow-sm transition hover:brightness-110"
              onClick={handleCreate}
              loading={createSubmitting}
              loadingLabel="处理中"
            >
              创建任务
            </LoadingButton>
          </div>
        ) : null}
        {message ? (
          <div className="mt-3 rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-700">
            {message}
          </div>
        ) : null}
      </div>
        </>
  );
}
