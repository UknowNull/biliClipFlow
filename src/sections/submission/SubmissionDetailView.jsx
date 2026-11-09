import { useState } from "react";
import { confirm as dialogConfirm, open as openDialog } from "@tauri-apps/plugin-dialog";
import { invokeCommand } from "../../lib/tauri";
import { collectCurrentTags, resolveEditChangedModules, validateSourceVideos } from "../../lib/submissionEditDiff";
import {
  buildCollectionTreeValue,
  buildSourceVideoPayload,
  formatSourceBindingSummary,
  isDirectSourceInput,
  resolveSourceRemoteInput,
} from "./helpers";
import { formatDateTime } from "../../lib/format";

export default function SubmissionDetailView({
  addSource,
  applySourceDownloadUrlBinding,
  backToList,
  bindingMergedVideo,
  bindingRemoteFile,
  buildPartitionOptionValue,
  canModifySelectedTask,
  canSyncRemoteEdit,
  collectionOptions,
  collectionSectionOptions,
  collectionTreeSections,
  collectionTreeValue,
  coverPreviewSrc,
  coverUploading,
  defaultBaiduSyncPath,
  deleteMergedSubmitting,
  deleteMergedTarget,
  detailLoading,
  detailSourceOptionList,
  detailTab,
  dragStateRef,
  draggingMergedId,
  draggingSegmentId,
  draggingSourceRowId,
  duplicateSource,
  editBaseline,
  editSegments,
  editingSegmentId,
  editingSegmentName,
  estimatedSegments,
  fetchTaskDetail,
  formatTaskStatus,
  handleChooseSourceRemotePart,
  handleClearCover,
  handleCollectionTreeChange,
  handlePartitionChange,
  handleSelectCoverFile,
  handleTagKeyDown,
  isActivityTag,
  isDetailView,
  isEditView,
  isVideoFilePath,
  mergedBindSubmitting,
  mergedDragStateRef,
  mergedOrderDirty,
  mergedOrderDraftIds,
  message,
  normalizeCoverPreviewSrc,
  normalizeSourceTime,
  openFileDialog,
  parseHmsToSeconds,
  partitionOptions,
  partitionSelectValue,
  partitions,
  removeSource,
  removeTag,
  renderActivityTopicSelector,
  resetMergedOrderDraft,
  resolveTagClassName,
  resolveTagRemoveClassName,
  segmentBindingMergedId,
  segmentBindingSelection,
  segmentBindingSubmitting,
  segmentationEnabled,
  selectedCollectionOption,
  selectedTask,
  setBindingMergedVideo,
  setCoverPreviewModalOpen,
  setCoverPreviewModalSrc,
  setDeleteMergedLocalFile,
  setDeleteMergedMessage,
  setDeleteMergedOpen,
  setDeleteMergedTarget,
  setDetailTab,
  setDraggingMergedId,
  setDraggingSegmentId,
  setDraggingSourceRowId,
  setEditSegments,
  setEditSubmitConfirmOpen,
  setEditSubmitConfirmSyncRemote,
  setEditingSegmentId,
  setEditingSegmentName,
  setMergedBindDialogOpen,
  setMergedBindMessage,
  setMergedBindSelection,
  setMergedBindTarget,
  setMergedOrderDirty,
  setMessage,
  setPendingEditChangedLabel,
  setPendingEditNeedRemote,
  setPendingEditSubmitPayload,
  setRemoteFilePickerOpen,
  setRemoteFilePickerPath,
  setSegmentBindingMergedId,
  setSegmentBindingSelection,
  setSegmentBindingSubmitting,
  setSelectedTask,
  setTagInput,
  setTaskForm,
  sourceDragStateRef,
  sourcePartPickerLoadingId,
  sourceVideos,
  submittingEdit,
  tagInput,
  tags,
  taskForm,
  updateSource,
  updateSourceTime,
  updateSourceUploadTitle,
  uploadTitleHints,
  workflowConfig,
}) {
  const [clearingMergedRemoteId, setClearingMergedRemoteId] = useState(0);

  const [retryingSegmentIds, setRetryingSegmentIds] = useState(() => new Set());

  const [sourceReorderSubmitting, setSourceReorderSubmitting] = useState(false);

  const [mergedReorderSubmitting, setMergedReorderSubmitting] = useState(false);

  const toggleSegmentBindingSelection = (segmentId, checked) => {
    const normalized = String(segmentId || "").trim();
    if (!normalized) {
      return;
    }
    setSegmentBindingSelection((prev) => {
      const next = new Set(prev);
      if (checked) {
        next.add(normalized);
      } else {
        next.delete(normalized);
      }
      return next;
    });
  };

  const toggleAllSegmentBindingSelection = (checked) => {
    const segmentIds = (selectedTask?.outputSegments || [])
      .map((item) => String(item?.segmentId || "").trim())
      .filter((item) => item);
    setSegmentBindingSelection((prev) => {
      const next = new Set(prev);
      segmentIds.forEach((segmentId) => {
        if (checked) {
          next.add(segmentId);
        } else {
          next.delete(segmentId);
        }
      });
      return next;
    });
  };

  const openRemoteFilePickerForMerged = (mergedVideo) => {
    const taskId = selectedTask?.task?.taskId || "";
    const mergedId = Number(mergedVideo?.id || 0);
    if (!taskId || !mergedId) {
      setMessage("合并视频信息不完整，无法绑定");
      return;
    }
    const currentPath = resolveMergedRemotePath(mergedVideo);
    let initialPath = selectedTask?.task?.baiduSyncPath || defaultBaiduSyncPath || "/";
    if (currentPath && currentPath !== "-" && currentPath.includes("/")) {
      const lastIndex = currentPath.lastIndexOf("/");
      initialPath = lastIndex > 0 ? currentPath.slice(0, lastIndex) : "/";
    }
    setBindingMergedVideo({
      taskId,
      mergedId,
      fileName: mergedVideo?.fileName || "",
    });
    setRemoteFilePickerPath(initialPath);
    setRemoteFilePickerOpen(true);
  };

  const handleClearMergedRemoteBinding = async (mergedVideo) => {
    const taskId = String(selectedTask?.task?.taskId || "").trim();
    const mergedId = Number(mergedVideo?.id || 0);
    if (!taskId || !mergedId) {
      setMessage("合并视频信息不完整，无法清空绑定");
      return;
    }
    const remotePath = resolveMergedRemotePath(mergedVideo);
    if (!remotePath || remotePath === "-") {
      setMessage("当前合并视频未绑定网盘文件");
      return;
    }
    const confirmed = await dialogConfirm(
      `确认清空该合并视频的网盘绑定吗？\n当前绑定：${remotePath}`,
      {
        title: "清空网盘绑定",
        kind: "warning",
      },
    );
    if (!confirmed) {
      return;
    }
    setClearingMergedRemoteId(mergedId);
    try {
      await invokeCommand("submission_clear_merged_remote_file", {
        request: {
          taskId,
          mergedId,
        },
      });
      const detail = await fetchTaskDetail(taskId, { log: false });
      setSelectedTask(detail);
      setMessage("已清空网盘绑定");
    } catch (error) {
      setMessage(error?.message || "清空网盘绑定失败");
    } finally {
      setClearingMergedRemoteId(0);
    }
  };

  const reorderSourceVideos = async (fromIndex, toIndex) => {
    const taskId = String(selectedTask?.task?.taskId || "").trim();
    const sources = Array.isArray(selectedTask?.sourceVideos) ? selectedTask.sourceVideos : [];
    if (!taskId || fromIndex === toIndex || fromIndex < 0 || toIndex < 0) {
      return;
    }
    if (fromIndex >= sources.length || toIndex >= sources.length) {
      return;
    }
    const next = [...sources];
    const [moved] = next.splice(fromIndex, 1);
    next.splice(toIndex, 0, moved);
    const sourceIds = next.map((item) => String(item?.id || "").trim()).filter((item) => item);
    if (!sourceIds.length) {
      return;
    }
    setSourceReorderSubmitting(true);
    try {
      const result = await invokeCommand("submission_reorder_source_videos", {
        request: {
          taskId,
          sourceIds,
        },
      });
      const detail = await fetchTaskDetail(taskId, { log: false });
      setSelectedTask(detail);
      setMessage(result || "源视频排序已更新");
    } catch (error) {
      setMessage(error?.message || "更新源视频排序失败");
    } finally {
      setSourceReorderSubmitting(false);
    }
  };

  const handleMergedPointerDown = (event, mergedId) => {
    if (
      event.button !== 0 ||
      mergedReorderSubmitting ||
      bindingRemoteFile ||
      deleteMergedSubmitting ||
      mergedBindSubmitting
    ) {
      return;
    }
    const normalizedId = String(mergedId || "").trim();
    if (!normalizedId) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    if (event.currentTarget?.setPointerCapture) {
      event.currentTarget.setPointerCapture(event.pointerId);
    }
    mergedDragStateRef.current = { activeId: normalizedId, overId: normalizedId };
    setDraggingMergedId(normalizedId);
  };

  const submitMergedOrderDraft = async () => {
    const taskId = String(selectedTask?.task?.taskId || "").trim();
    if (!taskId || !mergedOrderDirty) {
      return;
    }
    const mergedIds = mergedOrderDraftIds
      .map((item) => Number(item))
      .filter((item) => Number.isFinite(item) && item > 0);
    if (!mergedIds.length) {
      setMessage("合并视频排序数据为空，无法提交");
      return;
    }
    setMergedReorderSubmitting(true);
    try {
      const result = await invokeCommand("submission_reorder_merged_videos", {
        request: {
          taskId,
          mergedIds,
        },
      });
      const detail = await fetchTaskDetail(taskId, { log: false });
      setSelectedTask(detail);
      setMergedOrderDirty(false);
      setMessage(result || "合并视频排序已更新");
    } catch (error) {
      setMessage(error?.message || "更新合并视频排序失败");
    } finally {
      setMergedReorderSubmitting(false);
    }
  };

  const openMergedBindDialog = (mergedVideo) => {
    const taskId = String(selectedTask?.task?.taskId || "").trim();
    const mergedId = Number(mergedVideo?.id || 0);
    if (!taskId || !Number.isFinite(mergedId) || mergedId <= 0) {
      setMessage("合并视频信息不完整，无法修改绑定");
      return;
    }
    const boundIds = (Array.isArray(mergedVideo?.sourceVideoIds) ? mergedVideo.sourceVideoIds : [])
      .map((sourceId) => String(sourceId || "").trim())
      .filter((sourceId) => sourceId);
    setMergedBindTarget({
      taskId,
      mergedId,
      fileName: mergedVideo?.fileName || "",
    });
    setMergedBindSelection(new Set(boundIds));
    setMergedBindMessage("");
    setMergedBindDialogOpen(true);
  };

  const openDeleteMergedModal = (mergedVideo) => {
    const taskId = selectedTask?.task?.taskId || "";
    const mergedId = Number(mergedVideo?.id || 0);
    if (!taskId || !mergedId) {
      setMessage("合并视频信息不完整，无法删除");
      return;
    }
    setDeleteMergedTarget({
      taskId,
      mergedId,
      fileName: mergedVideo?.fileName || "",
      videoPath: mergedVideo?.videoPath || "",
    });
    setDeleteMergedLocalFile(false);
    setDeleteMergedMessage("");
    setDeleteMergedOpen(true);
  };

  const handleBatchBindSegmentsMerged = async () => {
    const taskId = String(selectedTask?.task?.taskId || "").trim();
    if (!taskId) {
      setMessage("任务信息不存在，请重新打开详情");
      return;
    }
    const mergedId = Number(segmentBindingMergedId || 0);
    if (!Number.isFinite(mergedId) || mergedId <= 0) {
      setMessage("请选择要绑定的合并视频");
      return;
    }
    const segmentIds = (selectedTask?.outputSegments || [])
      .map((item) => String(item?.segmentId || "").trim())
      .filter((item) => item && segmentBindingSelection.has(item));
    if (!segmentIds.length) {
      setMessage("请先勾选要修改绑定的分段");
      return;
    }
    const targetMerged = (selectedTask?.mergedVideos || []).find(
      (item) => String(item?.id || "") === String(mergedId),
    );
    const mergedDisplay = targetMerged
      ? resolveSegmentMergedDisplay(targetMerged.id)
      : `合并视频 ${mergedId}`;
    const confirmed = await dialogConfirm(
      `确认将选中的 ${segmentIds.length} 个分段绑定到「${mergedDisplay}」吗？`,
      {
        title: "批量更换绑定",
        kind: "warning",
      },
    );
    if (!confirmed) {
      return;
    }
    setSegmentBindingSubmitting(true);
    try {
      const result = await invokeCommand("submission_batch_bind_segments_merged", {
        request: {
          taskId,
          mergedId,
          segmentIds,
        },
      });
      const detail = await fetchTaskDetail(taskId, { log: false });
      setSelectedTask(detail);
      setSegmentBindingSelection(new Set());
      const hasMerged = (detail?.mergedVideos || []).some(
        (item) => String(item?.id || "") === String(mergedId),
      );
      if (!hasMerged) {
        const fallbackMergedId =
          detail?.mergedVideos?.[0]?.id !== undefined && detail?.mergedVideos?.[0]?.id !== null
            ? String(detail.mergedVideos[0].id)
            : "";
        setSegmentBindingMergedId(fallbackMergedId);
      }
      setMessage(result || `已更新 ${segmentIds.length} 个分段绑定`);
    } catch (error) {
      setMessage(error?.message || "批量绑定合并视频失败");
    } finally {
      setSegmentBindingSubmitting(false);
    }
  };

  const extractPartNameFromPath = (filePath) => {
    if (!filePath) {
      return "";
    }
    const rawName = filePath.split(/[\\/]/).pop() || "";
    const dotIndex = rawName.lastIndexOf(".");
    if (dotIndex > 0) {
      return rawName.slice(0, dotIndex);
    }
    return rawName;
  };

  const updateEditSegment = (segmentId, patch) => {
    setEditSegments((prev) =>
      prev.map((segment) =>
        segment.segmentId === segmentId ? { ...segment, ...patch } : segment,
      ),
    );
  };

  const handleEditSubmit = async () => {
    if (!selectedTask?.task?.taskId || submittingEdit) {
      return;
    }
    setMessage("");
    const taskId = selectedTask.task.taskId;
    let segmentsForSubmit = editSegments;
    if (editingSegmentId) {
      const nextName = editingSegmentName.trim();
      if (nextName) {
        segmentsForSubmit = editSegments.map((segment) =>
          segment.segmentId === editingSegmentId
            ? { ...segment, partName: nextName }
            : segment,
        );
        updateEditSegment(editingSegmentId, { partName: nextName });
      }
      setEditingSegmentId("");
      setEditingSegmentName("");
    }
    const changedModules = resolveEditChangedModules({
      baseline: editBaseline,
      taskForm,
      tags,
      tagInput,
      sourceVideos,
      editSegments: segmentsForSubmit,
    });
    if (changedModules.length === 0) {
      setMessage("未检测到改动，无需提交");
      return;
    }
    const needUpdateSource = changedModules.includes("source");
    const needEditSubmit = changedModules.includes("basic") || changedModules.includes("segments");
    const moduleLabelMap = {
      basic: "基本信息",
      source: "源视频",
      segments: "分段与上传",
    };
    const changedLabel = changedModules.map((key) => moduleLabelMap[key] || key).join("、");
    let sourcePayload = [];
    if (needUpdateSource) {
      const sourceValidation = validateSourceVideos({
        items: sourceVideos,
        parseHmsToSeconds,
        isVideoFilePath,
      });
      if (!sourceValidation.valid) {
        setMessage(sourceValidation.message);
        return;
      }
      sourcePayload = sourceValidation.validSources.map((item, index) => ({
        ...buildSourceVideoPayload(item, index),
      }));
    }
    const uniqueTags = collectCurrentTags(tags, tagInput);
    if (needEditSubmit) {
      if (!taskForm.title.trim()) {
        setMessage("请输入投稿标题");
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
      if (uniqueTags.length === 0) {
        setMessage("请填写至少一个投稿标签");
        return;
      }
      if (!segmentsForSubmit.length) {
        setMessage("至少需要保留一个分P");
        return;
      }
      const emptyNameSegment = segmentsForSubmit.find(
        (segment) => !segment.partName || !segment.partName.trim(),
      );
      if (emptyNameSegment) {
        setMessage("分P名称不能为空");
        return;
      }
      const emptyPathSegment = segmentsForSubmit.find(
        (segment) => !segment.segmentFilePath || !segment.segmentFilePath.trim(),
      );
      if (emptyPathSegment) {
        setMessage("分P文件路径不能为空");
        return;
      }
    }
    const segmentPayload = segmentsForSubmit.map((segment, index) => ({
      segmentId: segment.segmentId,
      partName: segment.partName,
      partOrder: index + 1,
      segmentFilePath: segment.segmentFilePath,
      cid: segment.cid ?? null,
      fileName: segment.fileName ?? null,
      uploadStatus: segment.uploadStatus ?? null,
      uploadProgress:
        typeof segment.uploadProgress === "number" ? segment.uploadProgress : null,
      uploadUploadedBytes:
        typeof segment.uploadUploadedBytes === "number" ? segment.uploadUploadedBytes : null,
      uploadTotalBytes:
        typeof segment.uploadTotalBytes === "number" ? segment.uploadTotalBytes : null,
    }));
    const selectedCollectionId = (() => {
      const raw = String(taskForm.collectionId ?? "").trim();
      if (!raw) {
        return null;
      }
      const parsed = Number(raw);
      return Number.isFinite(parsed) && parsed > 0 ? parsed : null;
    })();
    const selectedCollectionSectionId = (() => {
      if (!selectedCollectionId) {
        return null;
      }
      const raw = String(taskForm.collectionSectionId ?? "").trim();
      if (!raw) {
        return null;
      }
      const parsed = Number(raw);
      return Number.isFinite(parsed) && parsed > 0 ? parsed : null;
    })();
    const taskPayload = needEditSubmit
      ? {
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
          segmentPrefix: taskForm.segmentPrefix || null,
        }
      : null;
    setPendingEditSubmitPayload({
      taskId,
      needUpdateSource,
      sourcePayload,
      needEditSubmit,
      task: taskPayload,
      segments: segmentPayload,
    });
    setPendingEditChangedLabel(changedLabel);
    setPendingEditNeedRemote(needEditSubmit);
    setEditSubmitConfirmSyncRemote(needEditSubmit && canSyncRemoteEdit);
    setEditSubmitConfirmOpen(true);
  };

  const handleEditSegmentAdd = async () => {
    if (!selectedTask?.task?.taskId) {
      return;
    }
    setMessage("");
    try {
      await invokeCommand("auth_client_log", {
        message: `submission_edit_add_segment_click taskId=${selectedTask.task.taskId}`,
      });
    } catch (_) {}
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
      try {
        await invokeCommand("auth_client_log", {
          message: `submission_edit_add_segment_selected taskId=${selectedTask.task.taskId} path=${selected}`,
        });
      } catch (_) {}
      const defaultName = extractPartNameFromPath(selected);
      const segment = await invokeCommand("submission_edit_add_segment", {
        request: {
          taskId: selectedTask.task.taskId,
          filePath: selected,
          partName: defaultName || null,
        },
      });
      try {
        await invokeCommand("auth_client_log", {
          message: `submission_edit_add_segment_response taskId=${selectedTask.task.taskId} segmentId=${segment?.segmentId || "unknown"}`,
        });
      } catch (_) {}
      setEditSegments((prev) => [...prev, segment]);
    } catch (error) {
      try {
        await invokeCommand("auth_client_log", {
          message: `submission_edit_add_segment_error taskId=${selectedTask?.task?.taskId || ""} err=${error.message || String(error)}`,
        });
      } catch (_) {}
      setMessage(error.message);
    }
  };

  const handleEditSegmentReupload = async (segmentId) => {
    setMessage("");
    const targetId = String(segmentId || "").trim();
    if (!targetId) {
      setMessage("分P ID无效");
      return;
    }
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
      const segment = await invokeCommand("submission_edit_reupload_segment", {
        request: {
          taskId: selectedTask.task.taskId,
          segmentId: targetId,
          filePath: selected,
        },
      });
      updateEditSegment(targetId, {
        partName: segment.partName,
        segmentFilePath: segment.segmentFilePath,
        uploadStatus: segment.uploadStatus,
        uploadProgress: segment.uploadProgress,
        cid: segment.cid,
        fileName: segment.fileName,
        uploadUploadedBytes: segment.uploadUploadedBytes,
        uploadTotalBytes: segment.uploadTotalBytes,
      });
    } catch (error) {
      setMessage(error.message);
    }
  };

  const handleEditSegmentDelete = (segmentId) => {
    const targetId = String(segmentId || "").trim();
    if (!targetId) {
      return;
    }
    setEditSegments((prev) => prev.filter((segment) => segment.segmentId !== targetId));
    if (editingSegmentId === targetId) {
      setEditingSegmentId("");
      setEditingSegmentName("");
    }
  };

  const handleSegmentNameStartEdit = (segment) => {
    setEditingSegmentId(segment.segmentId);
    setEditingSegmentName(segment.partName || "");
  };

  const commitSegmentNameEdit = () => {
    const targetId = editingSegmentId;
    if (!targetId) {
      return;
    }
    const nextName = editingSegmentName.trim();
    if (nextName) {
      updateEditSegment(targetId, { partName: nextName });
    }
    setEditingSegmentId("");
    setEditingSegmentName("");
  };

  const cancelSegmentNameEdit = () => {
    setEditingSegmentId("");
    setEditingSegmentName("");
  };

  const handleEditSourcePointerDown = (event, sourceId) => {
    if (event.button !== 0) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    if (event.currentTarget?.setPointerCapture) {
      event.currentTarget.setPointerCapture(event.pointerId);
    }
    sourceDragStateRef.current = { activeId: sourceId, overId: sourceId };
    setDraggingSourceRowId(sourceId);
  };

  const handleSegmentPointerDown = (event, segmentId) => {
    if (event.button !== 0) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    if (event.currentTarget?.setPointerCapture) {
      event.currentTarget.setPointerCapture(event.pointerId);
    }
    dragStateRef.current = { activeId: segmentId, overId: segmentId };
    setDraggingSegmentId(segmentId);
    invokeCommand("auth_client_log", {
      message: `submission_edit_drag_pointer_start segmentId=${segmentId}`,
    }).catch(() => {});
  };

  const handleRetrySegmentUpload = async (segmentId) => {
    setMessage("");
    const targetId = String(segmentId || "").trim();
    if (!targetId) {
      setMessage("分段ID无效，无法重试");
      return;
    }
    if (retryingSegmentIds.has(targetId)) {
      return;
    }
    setRetryingSegmentIds((prev) => {
      const next = new Set(prev);
      next.add(targetId);
      return next;
    });
    try {
      const result = await invokeCommand("submission_retry_segment_upload", { segmentId: targetId });
      setMessage(result || "任务已加入投稿队列");
      if (selectedTask?.task?.taskId) {
        const detail = await fetchTaskDetail(selectedTask.task.taskId, { log: false });
        setSelectedTask(detail);
      }
    } catch (error) {
      setMessage(error.message);
    } finally {
      setRetryingSegmentIds((prev) => {
        const next = new Set(prev);
        next.delete(targetId);
        return next;
      });
    }
  };

  const formatUploadProgress = (value) => {
    const numeric = Number(value);
    if (!Number.isFinite(numeric)) {
      return 0;
    }
    return Math.min(100, Math.max(0, Math.round(numeric)));
  };

  const formatSegmentUploadStatus = (status) => {
    if (!status) {
      return "-";
    }
    switch (status) {
      case "PENDING":
        return "待上传";
      case "UPLOADING":
        return "上传中";
      case "SUCCESS":
        return "已上传";
      case "FAILED":
        return "上传失败";
      case "RATE_LIMITED":
        return "等待中";
      case "PAUSED":
        return "已暂停";
      case "CANCELLED":
        return "已取消";
      default:
        return "未知";
    }
  };

  const formatMergedVideoStatus = (status) => {
    const value = Number(status);
    if (!Number.isFinite(value)) {
      return "未知";
    }
    if (value === 0) {
      return "待处理";
    }
    if (value === 1) {
      return "处理中";
    }
    if (value === 2) {
      return "已完成";
    }
    if (value === 3) {
      return "失败";
    }
    return "未知";
  };

  const resolveMergedRemotePath = (merged) => {
    if (!merged) {
      return "-";
    }
    const remoteDir = String(merged.remoteDir || "").trim();
    const remoteName = String(merged.remoteName || "").trim();
    if (!remoteDir || !remoteName) {
      return "-";
    }
    const normalizedDir = remoteDir.endsWith("/") ? remoteDir.slice(0, -1) : remoteDir;
    return `${normalizedDir}/${remoteName}`;
  };

  const detailSourceLabelMap = new Map(
    detailSourceOptionList.map((item) => [item.id, item.label]),
  );

  const resolveMergedBoundSourceDisplay = (merged) => {
    if (!merged) {
      return "-";
    }
    const sourceIds = Array.isArray(merged.sourceVideoIds) ? merged.sourceVideoIds : [];
    const sourcePaths = Array.isArray(merged.sourceVideoPaths) ? merged.sourceVideoPaths : [];
    const labels = [];
    sourceIds.forEach((sourceId) => {
      const normalized = String(sourceId || "").trim();
      if (!normalized) {
        return;
      }
      labels.push(detailSourceLabelMap.get(normalized) || normalized);
    });
    if (labels.length > 0) {
      return labels.join("，");
    }
    const pathLabels = sourcePaths
      .map((value) => String(value || "").trim())
      .filter((value) => value)
      .map((value) => value.split(/[\\/]/).pop() || value);
    if (pathLabels.length > 0) {
      return pathLabels.join("，");
    }
    return "未绑定";
  };

  const detailMergedNameMap = (() => {
    const map = new Map();
    (selectedTask?.mergedVideos || []).forEach((item) => {
      const mergedId = String(item?.id ?? "").trim();
      if (!mergedId) {
        return;
      }
      const fileName = String(item?.fileName || "").trim();
      const pathName = String(item?.videoPath || "")
        .split("/")
        .pop()
        ?.trim();
      map.set(mergedId, fileName || pathName || `合并视频 ${mergedId}`);
    });
    return map;
  })();

  const detailMergedVideos = (() => {
    const mergedVideos = Array.isArray(selectedTask?.mergedVideos) ? selectedTask.mergedVideos : [];
    if (!mergedVideos.length || mergedOrderDraftIds.length !== mergedVideos.length) {
      return mergedVideos;
    }
    const mergedMap = new Map();
    mergedVideos.forEach((item) => {
      const mergedId = String(item?.id || "").trim();
      if (mergedId) {
        mergedMap.set(mergedId, item);
      }
    });
    const ordered = mergedOrderDraftIds.map((mergedId) => mergedMap.get(mergedId)).filter(Boolean);
    return ordered.length === mergedVideos.length ? ordered : mergedVideos;
  })();

  const resolveSegmentMergedDisplay = (mergedId) => {
    const normalized = String(mergedId || "").trim();
    if (!normalized) {
      return "未绑定";
    }
    return detailMergedNameMap.get(normalized) || `未找到(ID:${normalized})`;
  };

  const detailHasOutputSegments = (selectedTask?.outputSegments?.length || 0) > 0;

  const detailSegmentIds = (selectedTask?.outputSegments || [])
    .map((item) => String(item?.segmentId || "").trim())
    .filter((item) => item);

  const detailSelectedSegmentIds = detailSegmentIds.filter((item) =>
    segmentBindingSelection.has(item),
  );

  const detailAllSegmentsSelected =
    detailSegmentIds.length > 0 && detailSelectedSegmentIds.length === detailSegmentIds.length;

  const detailHasMergedVideos = (selectedTask?.mergedVideos?.length || 0) > 0;

  const canBatchBindSegments =
    !canModifySelectedTask &&
    detailHasOutputSegments &&
    detailHasMergedVideos &&
    detailSelectedSegmentIds.length > 0 &&
    Number(segmentBindingMergedId || 0) > 0 &&
    !segmentBindingSubmitting;

  const partitionLabel = (() => {
    const exact = partitions.find((item) => String(item.tid) === String(taskForm.partitionId));
    if (exact?.name) {
      return exact.name;
    }
    const fallback = partitions.find((item) => String(item.tid) === String(taskForm.partitionId));
    return fallback?.name || taskForm.partitionId || "-";
  })();

  const videoTypeLabel =
    taskForm.videoType === "REPOST" ? "转载" : taskForm.videoType ? "原创" : "-";

  const detailEstimatedSegments = segmentationEnabled
    ? Math.max(selectedTask?.outputSegments?.length || 0, estimatedSegments)
    : 0;

  const selectedCollectionSectionOption = collectionSectionOptions.find(
    (section) => String(section.sectionId) === String(taskForm.collectionSectionId),
  );

  const collectionLabel = selectedCollectionOption?.name
    ? selectedCollectionSectionOption?.title
      ? `${selectedCollectionOption.name} / ${selectedCollectionSectionOption.title}`
      : selectedCollectionOption.name
    : taskForm.collectionId || "-";

  const editChangedModules = isEditView
    ? resolveEditChangedModules({
        baseline: editBaseline,
        taskForm,
        tags,
        tagInput,
        sourceVideos,
        editSegments,
      })
    : [];

  const editModuleLabelMap = {
    basic: "基本信息",
    source: "源视频",
    segments: "分段与上传",
  };

  const editChangedLabelText = editChangedModules
    .map((key) => editModuleLabelMap[key] || key)
    .join("、");

  const openCoverPreviewModal = (src) => {
    const normalized = normalizeCoverPreviewSrc(src);
    if (!normalized) {
      return;
    }
    setCoverPreviewModalSrc(normalized);
    setCoverPreviewModalOpen(true);
  };

  if (!(isDetailView || isEditView)) {
    return null;
  }

  return (
    <div className="rounded-2xl bg-[var(--surface)]/90 p-6 shadow-sm ring-1 ring-black/5">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div>
              <p className="text-sm uppercase tracking-[0.2em] text-[var(--muted)]">
                视频投稿
              </p>
              <h2 className="text-2xl font-semibold text-[var(--ink)]">
                {isEditView ? "投稿任务编辑" : "投稿任务详情"}
              </h2>
            </div>
            <button
              className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
              onClick={backToList}
            >
              返回列表
            </button>
          </div>
          {detailLoading ? (
            <div className="mt-4 rounded-lg border border-black/10 bg-white/70 px-4 py-8 text-center text-sm text-[var(--muted)]">
              正在加载投稿任务详情...
            </div>
          ) : !selectedTask ? (
            <div className="mt-4 rounded-lg border border-amber-200 bg-amber-50 px-4 py-8 text-center text-sm text-amber-700">
              {message || "未读取到投稿任务详情"}
            </div>
          ) : (
            <>
              {message ? (
                <div className="mt-4 rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-700">
                  {message}
                </div>
              ) : null}
              <div className="sticky top-0 z-10 -mx-6 mt-4 flex flex-wrap gap-2 border-y border-black/5 bg-[var(--surface)]/95 px-6 py-3 backdrop-blur">
                {[
                  { key: "basic", label: "基本信息" },
                  { key: "source", label: "源视频" },
                  { key: "merged", label: "合并视频" },
                  { key: "segmentUpload", label: "分段与上传" },
                ].map((tab) => (
                  <button
                    key={tab.key}
                    className={`rounded-full px-4 py-2 text-sm font-semibold transition ${
                      detailTab === tab.key
                        ? "bg-[var(--accent)] text-white"
                        : "border border-black/10 bg-white text-[var(--ink)]"
                    }`}
                    onClick={() => setDetailTab(tab.key)}
                  >
                    {tab.label}
                  </button>
                ))}
              </div>
              {detailTab === "basic" ? (
            <div className="mt-4 space-y-4 text-sm text-[var(--ink)]">
              <div className="grid gap-2">
                <div>任务ID：{selectedTask.task.taskId}</div>
                <div>状态：{formatTaskStatus(selectedTask.task.status)}</div>
                <div>BVID：{selectedTask.task.bvid || "-"}</div>
                <div>创建时间：{formatDateTime(selectedTask.task.createdAt)}</div>
                <div>更新时间：{formatDateTime(selectedTask.task.updatedAt)}</div>
              </div>
              {canModifySelectedTask ? (
                <div className="rounded-xl border border-black/5 bg-white/80 p-3">
                  <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
                    投稿信息
                  </div>
                  <div className="mt-3 space-y-3">
                    <div className="space-y-1">
                      <div className="text-xs text-[var(--muted)]">投稿标题</div>
                      <input
                        value={taskForm.title}
                        onChange={(event) =>
                          setTaskForm((prev) => ({ ...prev, title: event.target.value }))
                        }
                        placeholder="请输入投稿标题"
                        className="w-full rounded-xl border border-black/10 bg-white/80 px-3 py-2 text-sm text-[var(--ink)] focus:border-[var(--accent)] focus:outline-none"
                      />
                    </div>
                    <div className="space-y-1">
                      <div className="text-xs text-[var(--muted)]">视频描述</div>
                      <textarea
                        value={taskForm.description}
                        onChange={(event) =>
                          setTaskForm((prev) => ({ ...prev, description: event.target.value }))
                        }
                        placeholder="视频描述"
                        rows={2}
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
                    <div className="grid gap-3 lg:grid-cols-2">
                      <div className="space-y-1">
                        <div className="text-xs text-[var(--muted)]">B站分区</div>
                        <select
                          value={partitionSelectValue}
                          onChange={(event) => handlePartitionChange(event.target.value)}
                          disabled={isEditView}
                          className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none disabled:cursor-not-allowed disabled:bg-black/5"
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
                        <div className="text-xs text-[var(--muted)]">合集</div>
                        <select
                          value={collectionTreeValue}
                          onChange={(event) => handleCollectionTreeChange(event.target.value)}
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
                                value={buildCollectionTreeValue(
                                  collection.seasonId,
                                  section.sectionId,
                                )}
                              >
                                {`　└ ${section.title || `子合集${section.sectionId}`}`}
                              </option>
                            )),
                          ])}
                        </select>
                      </div>
                    </div>
                    <div className="grid gap-3 lg:grid-cols-2">
                      <div className="space-y-1">
                        <div className="text-xs text-[var(--muted)]">视频类型</div>
                        <select
                          value={taskForm.videoType}
                          onChange={(event) =>
                            setTaskForm((prev) => ({
                              ...prev,
                              videoType: event.target.value,
                            }))
                          }
                          className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
                        >
                          <option value="ORIGINAL">原创</option>
                          <option value="REPOST">转载</option>
                        </select>
                      </div>
                      <div className="space-y-1">
                        <div className="text-xs text-[var(--muted)]">分段前缀</div>
                        <input
                          value={taskForm.segmentPrefix}
                          onChange={(event) =>
                            setTaskForm((prev) => ({
                              ...prev,
                              segmentPrefix: event.target.value,
                            }))
                          }
                          placeholder="分段前缀"
                          className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
                        />
                      </div>
                    </div>
                    <div className="space-y-1">
                      <div className="text-xs text-[var(--muted)]">投稿标签</div>
                      <div className="rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus-within:border-[var(--accent)]">
                        <div className="flex flex-wrap gap-2">
                          {tags.map((tag) => (
                            <span
                              key={tag}
                              className={resolveTagClassName(tag)}
                            >
                              {isActivityTag(tag) ? `#话题 ${tag}` : tag}
                              <button
                                className={resolveTagRemoveClassName(tag)}
                                onClick={() => removeTag(tag)}
                                title="删除标签"
                              >
                                ×
                              </button>
                            </span>
                          ))}
                          <input
                            value={tagInput}
                            onChange={(event) => setTagInput(event.target.value)}
                            onKeyDown={handleTagKeyDown}
                            placeholder="回车添加标签"
                            className="min-w-[120px] flex-1 bg-transparent text-sm text-[var(--ink)] focus:outline-none"
                          />
                        </div>
                      </div>
                    </div>
                    {renderActivityTopicSelector()}
                  </div>
                </div>
              ) : (
                <div className="rounded-xl border border-black/5 bg-white/80 p-3">
                  <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
                    投稿信息
                  </div>
                  <div className="mt-3 space-y-3">
                    <div className="space-y-1">
                      <div className="text-xs text-[var(--muted)]">投稿标题</div>
                      <div className="rounded-lg border border-black/10 bg-white/80 px-3 py-2">
                        {taskForm.title || "-"}
                      </div>
                    </div>
                    <div className="space-y-1">
                      <div className="text-xs text-[var(--muted)]">视频描述</div>
                      <div className="rounded-lg border border-black/10 bg-white/80 px-3 py-2">
                        {taskForm.description || "-"}
                      </div>
                    </div>
                    <div className="space-y-1">
                      <div className="text-xs text-[var(--muted)]">视频封面</div>
                      <div className="rounded-lg border border-black/10 bg-white/80 p-2">
                        {coverPreviewSrc ? (
                          <button
                            type="button"
                            className="h-[120px] w-[192px] overflow-hidden rounded-md border border-black/10 bg-black/5"
                            onClick={() => openCoverPreviewModal(coverPreviewSrc)}
                            title="点击查看大图"
                          >
                            <img
                              src={coverPreviewSrc}
                              alt="封面预览"
                              className="h-full w-full object-cover"
                            />
                          </button>
                        ) : (
                          <span className="text-[var(--muted)]">-</span>
                        )}
                      </div>
                    </div>
                    <div className="grid gap-3 lg:grid-cols-2">
                      <div className="space-y-1">
                        <div className="text-xs text-[var(--muted)]">B站分区</div>
                        <div className="rounded-lg border border-black/10 bg-white/80 px-3 py-2">
                          {partitionLabel}
                        </div>
                      </div>
                      <div className="space-y-1">
                        <div className="text-xs text-[var(--muted)]">合集</div>
                        <div className="rounded-lg border border-black/10 bg-white/80 px-3 py-2">
                          {collectionLabel}
                        </div>
                      </div>
                    </div>
                    <div className="grid gap-3 lg:grid-cols-2">
                      <div className="space-y-1">
                        <div className="text-xs text-[var(--muted)]">视频类型</div>
                        <div className="rounded-lg border border-black/10 bg-white/80 px-3 py-2">
                          {videoTypeLabel}
                        </div>
                      </div>
                      <div className="space-y-1">
                        <div className="text-xs text-[var(--muted)]">分段前缀</div>
                        <div className="rounded-lg border border-black/10 bg-white/80 px-3 py-2">
                          {taskForm.segmentPrefix || "-"}
                        </div>
                      </div>
                    </div>
                    <div className="space-y-1">
                      <div className="text-xs text-[var(--muted)]">投稿标签</div>
                      <div className="rounded-lg border border-black/10 bg-white/80 px-3 py-2">
                        {tags.length ? (
                          <div className="flex flex-wrap gap-2">
                            {tags.map((tag) => (
                              <span
                                key={tag}
                                className={resolveTagClassName(tag)}
                              >
                                {isActivityTag(tag) ? `#话题 ${tag}` : tag}
                              </span>
                            ))}
                          </div>
                        ) : (
                          <span className="text-[var(--muted)]">-</span>
                        )}
                      </div>
                    </div>
                  </div>
                </div>
              )}
              <div className="rounded-xl border border-black/5 bg-white/80 p-3">
                <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
                  工作流配置
                </div>
                <div className="mt-2 grid gap-2">
                  <div>是否分段：{segmentationEnabled ? "需要分段" : "不需要分段"}</div>
                  {segmentationEnabled ? (
                    <>
                      <div>
                        分段时长：
                        {workflowConfig.segmentationConfig.segmentDurationSeconds || "-"} 秒
                      </div>
                      <div>
                        保留合并视频：
                        {workflowConfig.segmentationConfig.preserveOriginal ? "是" : "否"}
                      </div>
                      <div>
                        预计分段数：
                        {detailEstimatedSegments ? detailEstimatedSegments : "-"}
                      </div>
                    </>
                  ) : (
                    <div>预计分段数：不分段</div>
                  )}
                </div>
              </div>
            </div>
              ) : null}
              {detailTab === "source" ? (
            <div className="mt-4 overflow-hidden rounded-xl border border-black/5">
              {canModifySelectedTask ? (
                <div className="w-full">
                  <div className="flex flex-wrap items-center justify-between gap-3 border-b border-black/5 px-4 py-3">
                    <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
                      源视频配置
                    </div>
                    <button
                      className="rounded-full bg-[var(--accent)] px-3 py-1 text-xs font-semibold text-white"
                      onClick={addSource}
                    >
                      添加视频
                    </button>
                  </div>
                  <table className="w-full text-left text-sm">
                    <thead className="bg-black/5 text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
                      <tr>
                        <th className="px-4 py-2">序号</th>
                        <th className="px-4 py-2">视频文件路径</th>
                        <th className="px-4 py-2">BV号或下载链接</th>
                        <th className="px-4 py-2">分P标题</th>
                        <th className="px-4 py-2">上传分P标题</th>
                        <th className="px-4 py-2">开始时间</th>
                        <th className="px-4 py-2">结束时间</th>
                        <th className="px-4 py-2">操作</th>
                      </tr>
                    </thead>
                    <tbody>
                      {sourceVideos.length === 0 ? (
                        <tr>
                          <td className="px-4 py-3 text-[var(--muted)]" colSpan={8}>
                            暂无源视频
                          </td>
                        </tr>
                      ) : (
                        sourceVideos.map((item, index) => (
                          <tr
                            key={item.localId || `edit-source-${index}`}
                            data-edit-source-id={item.localId || ""}
                            className={`border-t border-black/5 ${
                              draggingSourceRowId === item.localId ? "bg-black/5" : ""
                            }`}
                          >
                            <td className="px-4 py-2 text-[var(--muted)]">
                              <div className="flex items-center gap-2">
                                <span
                                  className="cursor-grab select-none text-[var(--muted)]"
                                  onPointerDown={(event) =>
                                    handleEditSourcePointerDown(event, item.localId)
                                  }
                                  style={{ touchAction: "none" }}
                                >
                                  ≡
                                </span>
                                <span>{index + 1}</span>
                              </div>
                            </td>
                            <td className="px-4 py-2">
                              <div className="flex flex-wrap gap-2">
                                <input
                                  value={item.sourceFilePath}
                                  onChange={(event) =>
                                    updateSource(index, "sourceFilePath", event.target.value)
                                  }
                                  placeholder="请输入视频文件路径（必填）"
                                  className="w-full flex-1 rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
                                />
                                <button
                                  className="rounded-lg border border-black/10 bg-white px-3 py-2 text-xs font-semibold text-[var(--ink)]"
                                  onClick={() => openFileDialog(index)}
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
                                    updateSource(index, "remoteBvid", event.target.value)
                                  }
                                  placeholder="可选，填写源视频 BV 号或下载链接"
                                    className="w-full flex-1 rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
                                  />
                                  <button
                                    type="button"
                                    className="rounded-lg border border-black/10 bg-white px-3 py-2 text-xs font-semibold text-[var(--ink)] disabled:cursor-not-allowed disabled:opacity-60"
                                    onClick={() =>
                                      isDirectSourceInput(resolveSourceRemoteInput(item))
                                        ? applySourceDownloadUrlBinding("create", index)
                                        : handleChooseSourceRemotePart("create", index)
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
                                  updateSourceUploadTitle(index, event.target.value)
                                }
                                maxLength={80}
                                placeholder={
                                  segmentationEnabled
                                    ? "分段开启时自动分P"
                                    : uploadTitleHints.sourceTitleMap.get(item.localId) ||
                                      `P${index + 1}`
                                }
                                title={
                                  segmentationEnabled ? "分段开启时使用自动分P标题" : ""
                                }
                                disabled={segmentationEnabled}
                                className="w-32 rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none disabled:cursor-not-allowed disabled:bg-black/5"
                              />
                            </td>
                            <td className="px-4 py-2">
                              <input
                                value={item.startTime}
                                onChange={(event) =>
                                  updateSourceTime(index, "startTime", event.target.value)
                                }
                                onBlur={() => normalizeSourceTime(index, "startTime")}
                                placeholder="00:00:00"
                                className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
                              />
                            </td>
                            <td className="px-4 py-2">
                              <input
                                value={item.endTime}
                                onChange={(event) =>
                                  updateSourceTime(index, "endTime", event.target.value)
                                }
                                onBlur={() => normalizeSourceTime(index, "endTime")}
                                placeholder="00:00:00"
                                className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
                              />
                            </td>
                            <td className="px-4 py-2">
                              <div className="flex flex-wrap gap-2">
                                <button
                                  className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                                  onClick={() => duplicateSource(index)}
                                >
                                  复制
                                </button>
                                <button
                                  className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                                  onClick={() => removeSource(index)}
                                >
                                  删除
                                </button>
                              </div>
                            </td>
                          </tr>
                        ))
                      )}
                    </tbody>
                  </table>
                </div>
              ) : (
                <table className="w-full text-left text-sm">
                  <thead className="bg-black/5 text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
                    <tr>
                        <th className="px-4 py-2">序号</th>
                        <th className="px-4 py-2">视频文件路径</th>
                        <th className="px-4 py-2">BV号或下载链接</th>
                        <th className="px-4 py-2">分P标题</th>
                        <th className="px-4 py-2">上传分P标题</th>
                        <th className="px-4 py-2">开始时间</th>
                        <th className="px-4 py-2">结束时间</th>
                        <th className="px-4 py-2">操作</th>
                    </tr>
                  </thead>
                  <tbody>
                    {selectedTask.sourceVideos.length === 0 ? (
                      <tr>
                        <td className="px-4 py-3 text-[var(--muted)]" colSpan={8}>
                          暂无源视频
                        </td>
                      </tr>
                    ) : (
                      selectedTask.sourceVideos.map((item, index) => (
                        <tr key={item.id} className="border-t border-black/5">
                          <td className="px-4 py-2 text-[var(--muted)]">{index + 1}</td>
                          <td className="px-4 py-2 text-[var(--ink)]">{item.sourceFilePath}</td>
                          <td className="px-4 py-2 text-[var(--muted)]">
                            <div className="max-w-[28rem] break-all">{resolveSourceRemoteInput(item)}</div>
                            {formatSourceBindingSummary(item) ? (
                              <div className="text-[10px] text-[var(--muted)]">
                                {formatSourceBindingSummary(item)}
                              </div>
                            ) : null}
                          </td>
                          <td className="px-4 py-2 text-[var(--muted)]">
                            {item.remotePartTitle ||
                              (item.remoteBvid
                                ? "未指定分P"
                                : item.remoteVideoUrl
                                  ? "下载链接"
                                  : "-")}
                          </td>
                          <td className="px-4 py-2 text-[var(--muted)]">
                            {String(item.uploadPartTitleMode || "").toUpperCase() === "CUSTOM" &&
                            item.uploadPartTitle
                              ? item.uploadPartTitle
                              : "自动"}
                          </td>
                          <td className="px-4 py-2 text-[var(--muted)]">{item.startTime || "-"}</td>
                          <td className="px-4 py-2 text-[var(--muted)]">{item.endTime || "-"}</td>
                          <td className="px-4 py-2">
                            <div className="flex gap-2">
                              <button
                                className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)] disabled:cursor-not-allowed disabled:opacity-60"
                                onClick={() => reorderSourceVideos(index, index - 1)}
                                disabled={
                                  sourceReorderSubmitting ||
                                  index === 0
                                }
                              >
                                上移
                              </button>
                              <button
                                className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)] disabled:cursor-not-allowed disabled:opacity-60"
                                onClick={() => reorderSourceVideos(index, index + 1)}
                                disabled={
                                  sourceReorderSubmitting ||
                                  index === selectedTask.sourceVideos.length - 1
                                }
                              >
                                下移
                              </button>
                            </div>
                          </td>
                        </tr>
                      ))
                    )}
                  </tbody>
                </table>
              )}
            </div>
              ) : null}
              {detailTab === "merged" ? (
            <div className="mt-4 overflow-hidden rounded-xl border border-black/5">
              <div className="flex flex-wrap items-center justify-between gap-3 border-b border-black/5 px-4 py-3">
                <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">合并视频</div>
                <div className="flex flex-wrap items-center gap-2">
                  <span className="text-xs text-[var(--muted)]">
                    {mergedOrderDirty ? "排序已调整，点击提交后生效" : "拖动排序手柄可调整顺序"}
                  </span>
                  <button
                    className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)] disabled:cursor-not-allowed disabled:opacity-60"
                    onClick={resetMergedOrderDraft}
                    disabled={
                      !mergedOrderDirty ||
                      mergedReorderSubmitting ||
                      bindingRemoteFile ||
                      deleteMergedSubmitting ||
                      mergedBindSubmitting
                    }
                  >
                    重置
                  </button>
                  <button
                    className="rounded-full bg-[var(--accent)] px-3 py-1 text-xs font-semibold text-white disabled:cursor-not-allowed disabled:opacity-60"
                    onClick={submitMergedOrderDraft}
                    disabled={
                      !mergedOrderDirty ||
                      mergedReorderSubmitting ||
                      bindingRemoteFile ||
                      deleteMergedSubmitting ||
                      mergedBindSubmitting
                    }
                  >
                    {mergedReorderSubmitting ? "提交中" : "提交排序"}
                  </button>
                </div>
              </div>
              <table className="w-full text-left text-sm">
                <thead className="bg-black/5 text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
                  <tr>
                    <th className="px-4 py-2">序号</th>
                    <th className="px-4 py-2">排序</th>
                    <th className="px-4 py-2">文件名</th>
                    <th className="px-4 py-2">文件路径</th>
                    <th className="px-4 py-2">绑定源视频</th>
                    <th className="px-4 py-2">网盘链接</th>
                    <th className="px-4 py-2">状态</th>
                    <th className="px-4 py-2">创建时间</th>
                    <th className="px-4 py-2">操作</th>
                  </tr>
                </thead>
                <tbody>
                  {detailMergedVideos.length === 0 ? (
                    <tr>
                      <td className="px-4 py-3 text-[var(--muted)]" colSpan={9}>
                        暂无合并视频
                      </td>
                    </tr>
                  ) : (
                    detailMergedVideos.map((item, index) => {
                      const mergedId = String(item?.id || "").trim();
                      const remotePath = resolveMergedRemotePath(item);
                      const bindingThisItem =
                        bindingRemoteFile && Number(bindingMergedVideo?.mergedId || 0) === Number(item.id);
                      const deletingThisItem =
                        deleteMergedSubmitting &&
                        Number(deleteMergedTarget?.mergedId || 0) === Number(item.id);
                      const isDragging = draggingMergedId === mergedId;
                      return (
                        <tr
                          key={item.id}
                          data-merged-id={mergedId}
                          className={`border-t border-black/5 ${isDragging ? "bg-black/5" : ""}`}
                        >
                          <td className="px-4 py-2 text-[var(--muted)]">{index + 1}</td>
                          <td
                            className={`px-4 py-2 select-none text-[var(--muted)] ${
                              "cursor-grab"
                            }`}
                            onPointerDown={(event) => handleMergedPointerDown(event, mergedId)}
                            style={{ touchAction: "none" }}
                          >
                            ≡
                          </td>
                          <td className="px-4 py-2 text-[var(--ink)]">{item.fileName}</td>
                          <td className="px-4 py-2 text-[var(--muted)]">{item.videoPath}</td>
                          <td className="px-4 py-2 text-[var(--muted)]">
                            {resolveMergedBoundSourceDisplay(item)}
                          </td>
                          <td className="px-4 py-2 text-[var(--muted)] break-all">{remotePath}</td>
                          <td className="px-4 py-2 text-[var(--muted)]">
                            {formatMergedVideoStatus(item.status)}
                          </td>
                          <td className="px-4 py-2 text-[var(--muted)]">
                            {formatDateTime(item.createTime)}
                          </td>
                          <td className="px-4 py-2">
                            <div className="flex flex-wrap gap-2">
                              <button
                                className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)] disabled:cursor-not-allowed disabled:opacity-60"
                                onClick={() =>
                                  remotePath === "-"
                                    ? openRemoteFilePickerForMerged(item)
                                    : handleClearMergedRemoteBinding(item)
                                }
                                disabled={
                                  clearingMergedRemoteId === Number(item.id) ||
                                  bindingRemoteFile ||
                                  deleteMergedSubmitting ||
                                  mergedReorderSubmitting ||
                                  mergedBindSubmitting
                                }
                              >
                                {bindingThisItem
                                  ? "绑定中"
                                  : clearingMergedRemoteId === Number(item.id)
                                    ? "清空中"
                                    : remotePath === "-"
                                      ? "绑定网盘文件"
                                      : "清空绑定"}
                              </button>
                              <button
                                className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)] disabled:cursor-not-allowed disabled:opacity-60"
                                onClick={() => openMergedBindDialog(item)}
                                disabled={
                                  detailSourceOptionList.length === 0 ||
                                  bindingRemoteFile ||
                                  deleteMergedSubmitting ||
                                  mergedReorderSubmitting ||
                                  mergedBindSubmitting
                                }
                              >
                                重新绑定
                              </button>
                              <button
                                className="rounded-full border border-red-200 bg-red-50 px-2 py-1 text-xs font-semibold text-red-600 disabled:cursor-not-allowed disabled:opacity-60"
                                onClick={() => openDeleteMergedModal(item)}
                                disabled={
                                  bindingRemoteFile ||
                                  deleteMergedSubmitting ||
                                  mergedReorderSubmitting ||
                                  mergedBindSubmitting
                                }
                              >
                                {deletingThisItem ? "删除中" : "删除"}
                              </button>
                            </div>
                          </td>
                        </tr>
                      );
                    })
                  )}
                </tbody>
              </table>
            </div>
              ) : null}
              {detailTab === "segmentUpload" ? (
            <div className="mt-4 overflow-hidden rounded-xl border border-black/5">
              {canModifySelectedTask ? (
                <div className="w-full">
                  <div className="flex flex-wrap items-center justify-between gap-3 border-b border-black/5 px-4 py-3">
                    <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
                      上传进度
                    </div>
                      <button
                      className="rounded-full bg-[var(--accent)] px-3 py-1 text-xs font-semibold text-white"
                      onClick={handleEditSegmentAdd}
                    >
                      新增分P
                    </button>
                  </div>
                  <table className="w-full text-left text-sm">
                    <thead className="bg-black/5 text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
                      <tr>
                        <th className="px-4 py-2">序号</th>
                        <th className="px-4 py-2">排序</th>
                        <th className="px-4 py-2">P名称</th>
                        <th className="px-4 py-2">文件路径</th>
                        <th className="px-4 py-2">绑定合并视频</th>
                        <th className="px-4 py-2">上传状态</th>
                        <th className="px-4 py-2">上传进度</th>
                        <th className="px-4 py-2">操作</th>
                      </tr>
                    </thead>
                    <tbody>
                      {editSegments.length === 0 ? (
                        <tr>
                          <td className="px-4 py-3 text-[var(--muted)]" colSpan={8}>
                            暂无分P
                          </td>
                        </tr>
                      ) : (
                        editSegments.map((item, index) => {
                          const progress = formatUploadProgress(item.uploadProgress);
                          const isEditing = editingSegmentId === item.segmentId;
                          const isDragging = draggingSegmentId === item.segmentId;
                          return (
                            <tr
                              key={item.segmentId}
                              data-segment-id={item.segmentId}
                              className={`border-t border-black/5 ${isDragging ? "bg-black/5" : ""}`}
                            >
                              <td className="px-4 py-2 text-[var(--muted)]">{index + 1}</td>
                              <td
                                className="px-4 py-2 text-[var(--muted)] cursor-grab select-none"
                                onPointerDown={(event) =>
                                  handleSegmentPointerDown(event, item.segmentId)
                                }
                                style={{ touchAction: "none" }}
                              >
                                ≡
                              </td>
                              <td className="px-4 py-2 text-[var(--ink)]">
                                {isEditing ? (
                                  <input
                                    value={editingSegmentName}
                                    onChange={(event) =>
                                      setEditingSegmentName(event.target.value)
                                    }
                                    onBlur={commitSegmentNameEdit}
                                    onKeyDown={(event) => {
                                      if (event.key === "Enter") {
                                        event.preventDefault();
                                        commitSegmentNameEdit();
                                      }
                                      if (event.key === "Escape") {
                                        event.preventDefault();
                                        cancelSegmentNameEdit();
                                      }
                                    }}
                                    autoFocus
                                    className="w-full rounded border border-black/10 bg-white/90 px-2 py-1 text-sm text-[var(--ink)] focus:border-[var(--accent)] focus:outline-none"
                                  />
                                ) : (
                                  <div
                                    className="cursor-text"
                                    onDoubleClick={() => handleSegmentNameStartEdit(item)}
                                  >
                                    {item.partName}
                                  </div>
                                )}
                              </td>
                              <td className="px-4 py-2 text-[var(--muted)]">
                                {item.segmentFilePath}
                              </td>
                              <td className="px-4 py-2 text-[var(--muted)]">
                                {resolveSegmentMergedDisplay(item.mergedId)}
                              </td>
                              <td className="px-4 py-2 text-[var(--muted)]">
                                {formatSegmentUploadStatus(item.uploadStatus)}
                              </td>
                              <td className="px-4 py-2">
                                <div className="flex items-center gap-2">
                                  <div className="h-1.5 w-24 rounded-full bg-black/5">
                                    <div
                                      className="h-1.5 rounded-full bg-[var(--accent)]"
                                      style={{ width: `${progress}%` }}
                                    />
                                  </div>
                                  <span className="text-xs text-[var(--muted)]">
                                    {progress}%
                                  </span>
                                </div>
                              </td>
                              <td className="px-4 py-2">
                                <div className="flex flex-wrap gap-2">
                                  <button
                                    className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)] disabled:cursor-not-allowed disabled:opacity-60"
                                    onClick={() => handleEditSegmentReupload(item.segmentId)}
                                    disabled={item.uploadStatus === "UPLOADING"}
                                  >
                                    重新上传
                                  </button>
                                  <button
                                    className="rounded-full border border-red-200 bg-white px-2 py-1 text-xs font-semibold text-red-600 hover:border-red-300"
                                    onClick={() => handleEditSegmentDelete(item.segmentId)}
                                  >
                                    删除
                                  </button>
                                </div>
                              </td>
                            </tr>
                          );
                        })
                      )}
                    </tbody>
                  </table>
                </div>
              ) : detailHasOutputSegments ? (
                <div className="w-full">
                  <div className="flex flex-wrap items-center justify-between gap-3 border-b border-black/5 px-4 py-3">
                    <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
                      上传进度
                    </div>
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="text-xs text-[var(--muted)]">
                        已选 {detailSelectedSegmentIds.length} / {detailSegmentIds.length}
                      </span>
                      <select
                        value={segmentBindingMergedId}
                        onChange={(event) => setSegmentBindingMergedId(event.target.value)}
                        disabled={
                          segmentBindingSubmitting ||
                          !detailHasMergedVideos
                        }
                        className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)] disabled:cursor-not-allowed disabled:opacity-60"
                      >
                        <option value="">选择合并视频</option>
                        {(selectedTask.mergedVideos || []).map((item) => (
                          <option key={item.id} value={String(item.id)}>
                            {resolveSegmentMergedDisplay(item.id)}
                          </option>
                        ))}
                      </select>
                      <button
                        className="rounded-full bg-[var(--accent)] px-3 py-1 text-xs font-semibold text-white disabled:cursor-not-allowed disabled:opacity-60"
                        onClick={handleBatchBindSegmentsMerged}
                        disabled={!canBatchBindSegments}
                      >
                        {segmentBindingSubmitting ? "绑定中" : "批量更换绑定"}
                      </button>
                    </div>
                  </div>
                  <table className="w-full text-left text-sm">
                    <thead className="bg-black/5 text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
                      <tr>
                        <th className="px-4 py-2">
                          <input
                            type="checkbox"
                            checked={detailAllSegmentsSelected}
                            onChange={(event) =>
                              toggleAllSegmentBindingSelection(event.target.checked)
                            }
                            aria-label="全选分段"
                          />
                        </th>
                        <th className="px-4 py-2">序号</th>
                        <th className="px-4 py-2">P名称</th>
                        <th className="px-4 py-2">文件路径</th>
                        <th className="px-4 py-2">绑定合并视频</th>
                        <th className="px-4 py-2">上传状态</th>
                        <th className="px-4 py-2">上传进度</th>
                        <th className="px-4 py-2">操作</th>
                      </tr>
                    </thead>
                    <tbody>
                      {selectedTask.outputSegments.length === 0 ? (
                        <tr>
                          <td className="px-4 py-3 text-[var(--muted)]" colSpan={8}>
                            暂无输出分段
                          </td>
                        </tr>
                      ) : (
                        selectedTask.outputSegments.map((item, index) => {
                          const progress = formatUploadProgress(item.uploadProgress);
                          const isRetrying = retryingSegmentIds.has(item.segmentId);
                          const segmentId = String(item.segmentId || "").trim();
                          const isChecked = segmentBindingSelection.has(segmentId);
                          return (
                            <tr key={item.segmentId} className="border-t border-black/5">
                              <td className="px-4 py-2">
                                <input
                                  type="checkbox"
                                  checked={isChecked}
                                  onChange={(event) =>
                                    toggleSegmentBindingSelection(segmentId, event.target.checked)
                                  }
                                  aria-label={`选择分段 ${item.partName || item.segmentId || ""}`}
                                />
                              </td>
                              <td className="px-4 py-2 text-[var(--muted)]">{index + 1}</td>
                              <td className="px-4 py-2 text-[var(--ink)]">{item.partName}</td>
                              <td className="px-4 py-2 text-[var(--muted)]">
                                {item.segmentFilePath}
                              </td>
                              <td className="px-4 py-2 text-[var(--muted)]">
                                {resolveSegmentMergedDisplay(item.mergedId)}
                              </td>
                              <td className="px-4 py-2 text-[var(--muted)]">
                                {formatSegmentUploadStatus(item.uploadStatus)}
                              </td>
                              <td className="px-4 py-2">
                                <div className="flex items-center gap-2">
                                  <div className="h-1.5 w-24 rounded-full bg-black/5">
                                    <div
                                      className="h-1.5 rounded-full bg-[var(--accent)]"
                                      style={{ width: `${progress}%` }}
                                    />
                                  </div>
                                  <span className="text-xs text-[var(--muted)]">
                                    {progress}%
                                  </span>
                                </div>
                              </td>
                              <td className="px-4 py-2">
                                {item.uploadStatus === "FAILED" ? (
                                  <button
                                    className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)] disabled:cursor-not-allowed disabled:opacity-60"
                                    onClick={() => handleRetrySegmentUpload(item.segmentId)}
                                    disabled={isRetrying}
                                  >
                                    {isRetrying ? "重试中" : "重试"}
                                  </button>
                                ) : (
                                  <span className="text-xs text-[var(--muted)]">-</span>
                                )}
                              </td>
                            </tr>
                          );
                        })
                      )}
                    </tbody>
                  </table>
                </div>
              ) : (
                <table className="w-full text-left text-sm">
                  <thead className="bg-black/5 text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
                    <tr>
                      <th className="px-4 py-2">序号</th>
                      <th className="px-4 py-2">文件名</th>
                      <th className="px-4 py-2">文件路径</th>
                      <th className="px-4 py-2">上传进度</th>
                      <th className="px-4 py-2">创建时间</th>
                      <th className="px-4 py-2">操作</th>
                    </tr>
                  </thead>
                  <tbody>
                    {selectedTask.mergedVideos.length === 0 ? (
                      <tr>
                        <td className="px-4 py-3 text-[var(--muted)]" colSpan={6}>
                          暂无合并视频
                        </td>
                      </tr>
                    ) : (
                      selectedTask.mergedVideos.map((item, index) => {
                        const progress = formatUploadProgress(item.uploadProgress);
                        const remotePath = resolveMergedRemotePath(item);
                        const clearingThisItem = clearingMergedRemoteId === Number(item.id);
                        return (
                          <tr key={item.id} className="border-t border-black/5">
                            <td className="px-4 py-2 text-[var(--muted)]">{index + 1}</td>
                            <td className="px-4 py-2 text-[var(--ink)]">
                              {item.fileName || "-"}
                            </td>
                            <td className="px-4 py-2 text-[var(--muted)]">
                              {item.videoPath || "-"}
                            </td>
                            <td className="px-4 py-2">
                              <div className="flex items-center gap-2">
                                <div className="h-1.5 w-24 rounded-full bg-black/5">
                                  <div
                                    className="h-1.5 rounded-full bg-[var(--accent)]"
                                    style={{ width: `${progress}%` }}
                                  />
                                </div>
                                <span className="text-xs text-[var(--muted)]">
                                  {progress}%
                                </span>
                              </div>
                            </td>
                            <td className="px-4 py-2 text-[var(--muted)]">
                              {formatDateTime(item.createTime)}
                            </td>
                            <td className="px-4 py-2">
                              {remotePath === "-" ? (
                                <span className="text-xs text-[var(--muted)]">未绑定</span>
                              ) : (
                                <button
                                  className="rounded-full border border-red-200 bg-red-50 px-2 py-1 text-xs font-semibold text-red-600 disabled:cursor-not-allowed disabled:opacity-60"
                                  onClick={() => handleClearMergedRemoteBinding(item)}
                                  disabled={
                                    clearingThisItem ||
                                    bindingRemoteFile ||
                                    deleteMergedSubmitting ||
                                    mergedReorderSubmitting ||
                                    mergedBindSubmitting
                                  }
                                >
                                  {clearingThisItem ? "清空中" : "清空绑定"}
                                </button>
                              )}
                            </td>
                          </tr>
                        );
                      })
                    )}
                  </tbody>
                </table>
              )}
            </div>
              ) : null}
              {isEditView ? (
            <div className="mt-4 flex flex-wrap gap-2">
              <div className="w-full text-xs text-[var(--muted)]">
                本次修改：{editChangedLabelText || "无"}
              </div>
              <button
                className="rounded-full bg-[var(--accent)] px-4 py-2 text-sm font-semibold text-white shadow-sm transition hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-60"
                onClick={handleEditSubmit}
                disabled={submittingEdit || coverUploading}
              >
                {submittingEdit
                  ? "提交中"
                  : coverUploading
                    ? "封面上传中"
                    : "提交修改"}
              </button>
            </div>
              ) : null}
            </>
          )}
        </div>
  );
}
