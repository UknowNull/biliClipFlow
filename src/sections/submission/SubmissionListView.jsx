import { useState } from "react";
import {
  buildMergeItemsFromSources,
  canConfirmRemoteMissing,
  canReuploadAllSegments,
  defaultWorkflowConfig,
  emptySource,
  sourceFilters,
  statusFilters,
  submissionNeedsRemoteVerification,
} from "./helpers";
import { invokeCommand } from "../../lib/tauri";
import {
  confirm as dialogConfirm,
  message as dialogMessage,
  open as openDialog,
  save as saveDialog,
} from "@tauri-apps/plugin-dialog";
import { buildEditBaselineFromDetail } from "../../lib/submissionEditDiff";
import { showErrorDialog } from "../../lib/dialog";
import { formatDateTime } from "../../lib/format";
import LoadingButton from "../../components/LoadingButton";

export default function SubmissionListView({
  applyDetailToForm,
  bilibiliAccounts,
  clearEditUploadCache,
  currentPage,
  currentUpProfile,
  defaultBaiduSyncPath,
  deleteMergedLocalFile,
  deleteMergedMessage,
  deleteMergedOpen,
  deleteMergedSubmitting,
  deleteMergedTarget,
  extractCurrentAuthProfile,
  fetchTaskDetail,
  formatTaskStatus,
  handleOpenSyncPicker,
  initEditSegments,
  isVideoFilePath,
  lastEditTaskIdRef,
  loadBaiduSyncSettings,
  loadCollections,
  loadCurrentUpProfile,
  loadPartitions,
  loadTasks,
  message,
  onAuthChange,
  onRefreshBaiduStatus,
  pageSize,
  remoteImportDialogOpen,
  remoteImportInput,
  remoteImportLoading,
  repostBaiduSync,
  resetCoverState,
  resetSegmentBindingState,
  selectedBilibiliUid,
  selectedTask,
  selectedTaskIds,
  setActivityDropdownOpen,
  setActivityKeyword,
  setActivityMessage,
  setActivityOptions,
  setBilibiliAccounts,
  setCurrentPage,
  setCurrentUpProfile,
  setDeleteMergedLocalFile,
  setDeleteMergedMessage,
  setDeleteMergedOpen,
  setDeleteMergedSubmitting,
  setDeleteMergedTarget,
  setDetailLoading,
  setDetailTab,
  setDraggingMergeGroupSource,
  setDraggingMergeItemId,
  setEditBaseline,
  setEditSegments,
  setEditSubmitConfirmOpen,
  setEditSubmitConfirmSyncRemote,
  setMergeItems,
  setMergeSelection,
  setMessage,
  setPageSize,
  setPendingEditChangedLabel,
  setPendingEditNeedRemote,
  setPendingEditSubmitPayload,
  setQuickFillOpen,
  setRemoteImportDialogOpen,
  setRemoteImportInput,
  setRemoteImportLoading,
  setRemoteImportPreview,
  setRepostBaiduSync,
  setSegmentBindingMergedId,
  setSegmentBindingSelection,
  setSegmentBindingSubmitting,
  setSegmentationEnabled,
  setSelectedBilibiliUid,
  setSelectedTask,
  setSelectedTaskIds,
  setSourceFilter,
  setSourceVideos,
  setStatusFilter,
  setSubmissionView,
  setTagInput,
  setTags,
  setTaskForm,
  setTaskSearch,
  setTasks,
  setTranscodeRepostBaiduSync,
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
  setUpdateTaskDir,
  setUpdateTaskId,
  setUpdateWorkflowConfig,
  setWorkflowConfig,
  sourceFilter,
  statusFilter,
  submissionView,
  taskSearch,
  tasks,
  totalTasks,
  transcodeRepostBaiduSync,
}) {
  const [refreshingRemote, setRefreshingRemote] = useState(false);

  const [verifyingRemoteTaskId, setVerifyingRemoteTaskId] = useState("");

  const [remoteMissingConfirmTask, setRemoteMissingConfirmTask] = useState(null);

  const [remoteMissingConfirming, setRemoteMissingConfirming] = useState(false);

  const [reuploadConfirmTask, setReuploadConfirmTask] = useState(null);

  const [reuploadConfirming, setReuploadConfirming] = useState(false);

  const [deleteTargetId, setDeleteTargetId] = useState("");

  const [deleteTargetIds, setDeleteTargetIds] = useState([]);

  const [deleteConfirmOpen, setDeleteConfirmOpen] = useState(false);

  const [deletePreview, setDeletePreview] = useState(null);

  const [deleteBatchPreviews, setDeleteBatchPreviews] = useState([]);

  const [deleteTaskChecked, setDeleteTaskChecked] = useState(false);

  const [deleteFilesChecked, setDeleteFilesChecked] = useState(false);

  const [deleteFileSelections, setDeleteFileSelections] = useState(() => new Set());

  const [deletePreviewLoading, setDeletePreviewLoading] = useState(false);

  const [deleteConflictOpen, setDeleteConflictOpen] = useState(false);

  const [deleteConflictFiles, setDeleteConflictFiles] = useState([]);

  const [deletePendingPayload, setDeletePendingPayload] = useState(null);

  const [deleteSubmitting, setDeleteSubmitting] = useState(false);

  const [deleteMessage, setDeleteMessage] = useState("");

  const [deleteMessageTone, setDeleteMessageTone] = useState("info");

  const deleteMessageClass =
    deleteMessageTone === "error"
      ? "border-red-200 bg-red-50 text-red-700"
      : "border-amber-200 bg-amber-50 text-amber-700";

  const [resegmentOpen, setResegmentOpen] = useState(false);

  const [resegmentTaskId, setResegmentTaskId] = useState("");

  const [resegmentDefaultSeconds, setResegmentDefaultSeconds] = useState(0);

  const [resegmentSeconds, setResegmentSeconds] = useState("");

  const [resegmentSubmitting, setResegmentSubmitting] = useState(false);

  const [resegmentVideoSeconds, setResegmentVideoSeconds] = useState(0);

  const [resegmentMode, setResegmentMode] = useState("SPECIFIED");

  const [resegmentMergedVideos, setResegmentMergedVideos] = useState([]);

  const [resegmentMergedId, setResegmentMergedId] = useState("");

  const [resegmentIntegrateCurrent, setResegmentIntegrateCurrent] = useState(false);

  const [resegmentHasBvid, setResegmentHasBvid] = useState(false);

  const [repostOpen, setRepostOpen] = useState(false);

  const [repostTaskId, setRepostTaskId] = useState("");

  const [repostHasBvid, setRepostHasBvid] = useState(false);

  const [repostUseCurrentBvid, setRepostUseCurrentBvid] = useState(false);

  const [repostSubmitting, setRepostSubmitting] = useState(false);

  const [transcodeRepostOpen, setTranscodeRepostOpen] = useState(false);

  const [transcodeRepostTaskId, setTranscodeRepostTaskId] = useState("");

  const [transcodeRepostHasBvid, setTranscodeRepostHasBvid] = useState(false);

  const [transcodeRepostUseCurrentBvid, setTranscodeRepostUseCurrentBvid] =
    useState(false);

  const [transcodeRepostSubmitting, setTranscodeRepostSubmitting] =
    useState(false);

  const [exportingTasks, setExportingTasks] = useState(false);

  const [importingTasks, setImportingTasks] = useState(false);

  const [repostMode, setRepostMode] = useState("SPECIFIED");

  const [repostMergedVideos, setRepostMergedVideos] = useState([]);

  const [repostMergedId, setRepostMergedId] = useState("");

  const deleteFiles = deletePreview?.files || [];

  const deleteHasFiles = deleteFiles.length > 0;

  const isBatchDelete = deleteTargetIds.length > 0;

  const deleteTargetCount = isBatchDelete ? deleteTargetIds.length : deleteTargetId ? 1 : 0;

  const resetDeleteState = () => {
    setDeleteTargetId("");
    setDeleteTargetIds([]);
    setDeleteConfirmOpen(false);
    setDeletePreview(null);
    setDeleteBatchPreviews([]);
    setDeleteTaskChecked(false);
    setDeleteFilesChecked(true);
    setDeleteFileSelections(new Set());
    setDeletePreviewLoading(false);
    setDeleteConflictOpen(false);
    setDeleteConflictFiles([]);
    setDeletePendingPayload(null);
    setDeleteSubmitting(false);
    setDeleteMessage("");
    setDeleteMessageTone("info");
  };

  const resetFormState = () => {
    const initialSources = [emptySource(0)];
    setTaskForm({
      title: "",
      description: "",
      coverUrl: "",
      coverLocalPath: "",
      coverDataUrl: "",
      partitionId: "",
      collectionId: "",
      collectionSectionId: "",
      activityTopicId: "",
      activityMissionId: "",
      activityTitle: "",
      videoType: "ORIGINAL",
      segmentPrefix: "",
      immediateSubmit: true,
      priority: false,
      baiduSyncEnabled: false,
      baiduSyncPath: "",
      baiduSyncFilename: "",
    });
    setTagInput("");
    setTags([]);
    setSegmentationEnabled(true);
    setSourceVideos(initialSources);
    setMergeItems(buildMergeItemsFromSources(initialSources));
    setMergeSelection(new Set());
    setDraggingMergeItemId("");
    setDraggingMergeGroupSource({ groupId: "", sourceId: "" });
    setWorkflowConfig(defaultWorkflowConfig);
  };

  const openCreateView = async () => {
    setSubmissionView("create");
    setSelectedTask(null);
    setEditBaseline(null);
    resetCoverState();
    setMessage("");
    setQuickFillOpen(false);
    setActivityOptions([]);
    setActivityKeyword("");
    setActivityDropdownOpen(false);
    setActivityMessage("");
    resetFormState();
    const currentProfile = await loadCurrentUpProfile();
    const effectiveUid = String(
      currentProfile?.effectiveUid || selectedBilibiliUid || currentUpProfile?.uid || "",
    );
    await Promise.all([loadPartitions(effectiveUid), loadCollections(effectiveUid)]);
    await loadBaiduSyncSettings();
  };

  const openRemoteImportDialog = () => {
    setRemoteImportDialogOpen(true);
    setRemoteImportInput("");
    setRemoteImportLoading(false);
    setMessage("");
  };

  const closeRemoteImportDialog = () => {
    if (remoteImportLoading) {
      return;
    }
    setRemoteImportDialogOpen(false);
    setRemoteImportInput("");
  };

  const handleRemoteImportConfirm = async () => {
    const input = remoteImportInput.trim();
    if (!input) {
      setMessage("请输入视频链接或BVID");
      return;
    }
    setRemoteImportLoading(true);
    setMessage("");
    try {
      const preview = await invokeCommand("submission_remote_video_preview", {
        request: {
          input,
          enforceOwnerMatch: true,
        },
      });
      setRemoteImportPreview(preview);
      setSubmissionView("remoteImport");
      setRemoteImportDialogOpen(false);
      setRemoteImportInput("");
    } catch (error) {
      setMessage(error?.message || "拉取视频信息失败");
    } finally {
      setRemoteImportLoading(false);
    }
  };

  const openUpdateModal = async (task) => {
    const taskId = String(task?.taskId || "").trim();
    setMessage("");
    setSubmissionView("update");
    setUpdateTaskId(taskId);
    setUpdateTaskDir("");
    setUpdateSegmentPrefix(task?.segmentPrefix || "");
    setUpdateBaiduSync({
      enabled: Boolean(task?.baiduSyncEnabled),
      path: task?.baiduSyncPath || "",
      filename: task?.baiduSyncFilename || "",
    });
    const initialSources = [emptySource(0)];
    setUpdateSourceVideos(initialSources);
    setUpdateMergeItems(buildMergeItemsFromSources(initialSources));
    setUpdateMergeSelection(new Set());
    setUpdateSegmentationEnabled(true);
    setUpdateWorkflowConfig(defaultWorkflowConfig);
    setUpdateImportOpen(false);
    setUpdateImportInput("");
    setUpdateImportLoading(false);
    setUpdateImportResults([]);
    setUpdateImportSelection(new Set());
    setUpdateSubmitting(false);
    if (!taskId) {
      return;
    }
    try {
      const dir = await invokeCommand("submission_task_dir", { taskId });
      setUpdateTaskDir(String(dir || "").trim());
    } catch (error) {
      setMessage(error.message);
    }
  };

  const updateResegmentVideoSecondsByMerged = async (mergedList, mergedId) => {
    const target = mergedList.find(
      (item) => String(item?.id) === String(mergedId),
    );
    const mergedPath = target?.videoPath || "";
    if (!mergedPath) {
      setResegmentVideoSeconds(0);
      return;
    }
    try {
      const duration = await invokeCommand("video_duration", { path: mergedPath });
      const durationSeconds = Number(duration);
      setResegmentVideoSeconds(Number.isFinite(durationSeconds) ? durationSeconds : 0);
    } catch (error) {
      setResegmentVideoSeconds(0);
    }
  };

  const openResegmentModal = async (taskId) => {
    const targetId = String(taskId || "").trim();
    if (!targetId) {
      return;
    }
    setMessage("");
    setResegmentOpen(true);
    setResegmentTaskId(targetId);
    setResegmentDefaultSeconds(0);
    setResegmentSeconds("");
    setResegmentSubmitting(false);
    setResegmentVideoSeconds(0);
    setResegmentMode("SPECIFIED");
    setResegmentMergedVideos([]);
    setResegmentMergedId("");
    setResegmentIntegrateCurrent(false);
    setResegmentHasBvid(false);
    try {
      const detail = await invokeCommand("submission_detail", { taskId: targetId });
      const hasBvid = Boolean(detail?.task?.bvid);
      setResegmentHasBvid(hasBvid);
      setResegmentIntegrateCurrent(hasBvid);
      const seconds = Number(
        detail?.workflowConfig?.segmentationConfig?.segmentDurationSeconds,
      );
      const resolvedSeconds = Number.isFinite(seconds) ? seconds : 0;
      setResegmentDefaultSeconds(resolvedSeconds);
      setResegmentSeconds(resolvedSeconds ? String(resolvedSeconds) : "");
      const mergedVideos = Array.isArray(detail?.mergedVideos)
        ? detail.mergedVideos
        : [];
      setResegmentMergedVideos(mergedVideos);
      const defaultMergedId = mergedVideos[0]?.id ? String(mergedVideos[0].id) : "";
      setResegmentMergedId(defaultMergedId);
      if (defaultMergedId) {
        await updateResegmentVideoSecondsByMerged(mergedVideos, defaultMergedId);
      }
    } catch (error) {
      setMessage(error.message);
    }
  };

  const closeResegmentModal = () => {
    setResegmentOpen(false);
    setResegmentTaskId("");
    setResegmentDefaultSeconds(0);
    setResegmentSeconds("");
    setResegmentSubmitting(false);
    setResegmentVideoSeconds(0);
    setResegmentMode("SPECIFIED");
    setResegmentMergedVideos([]);
    setResegmentMergedId("");
    setResegmentIntegrateCurrent(false);
    setResegmentHasBvid(false);
    setMessage("");
  };

  const openRepostModal = async (task) => {
    const targetId = String(task?.taskId || "").trim();
    if (!targetId) {
      return;
    }
    if (submissionNeedsRemoteVerification(task)) {
      setMessage("远端提交结果尚未确认，请先校验远端状态，避免重复投稿");
      return;
    }
    const hasBvid = Boolean(String(task?.bvid || "").trim());
    setMessage("");
    setRepostOpen(true);
    setRepostTaskId(targetId);
    setRepostHasBvid(hasBvid);
    setRepostUseCurrentBvid(hasBvid);
    setRepostSubmitting(false);
    setRepostMode("SPECIFIED");
    setRepostMergedVideos([]);
    setRepostMergedId("");
    setRepostBaiduSync({
      enabled: Boolean(task?.baiduSyncEnabled),
      path: task?.baiduSyncPath || "",
      filename: task?.baiduSyncFilename || "",
    });
    try {
      const detail = await invokeCommand("submission_detail", { taskId: targetId });
      const mergedVideos = Array.isArray(detail?.mergedVideos)
        ? detail.mergedVideos
        : [];
      setRepostMergedVideos(mergedVideos);
      const defaultMergedId = mergedVideos[0]?.id ? String(mergedVideos[0].id) : "";
      setRepostMergedId(defaultMergedId);
      setRepostMode(mergedVideos.length ? "SPECIFIED" : "FULL_REPROCESS");
    } catch (error) {
      setMessage(error.message);
    }
  };

  const closeRepostModal = () => {
    setRepostOpen(false);
    setRepostTaskId("");
    setRepostHasBvid(false);
    setRepostUseCurrentBvid(false);
    setRepostSubmitting(false);
    setRepostMode("SPECIFIED");
    setRepostMergedVideos([]);
    setRepostMergedId("");
    setRepostBaiduSync({ enabled: false, path: "", filename: "" });
    setMessage("");
  };

  const openTranscodeRepostModal = (task) => {
    const targetId = String(task?.taskId || "").trim();
    if (!targetId) {
      return;
    }
    if (submissionNeedsRemoteVerification(task)) {
      setMessage("远端提交结果尚未确认，请先校验远端状态，避免重复投稿");
      return;
    }
    const hasBvid = Boolean(String(task?.bvid || "").trim());
    setMessage("");
    setTranscodeRepostOpen(true);
    setTranscodeRepostTaskId(targetId);
    setTranscodeRepostHasBvid(hasBvid);
    setTranscodeRepostUseCurrentBvid(hasBvid);
    setTranscodeRepostSubmitting(false);
    setTranscodeRepostBaiduSync({
      enabled: Boolean(task?.baiduSyncEnabled),
      path: task?.baiduSyncPath || "",
      filename: task?.baiduSyncFilename || "",
    });
  };

  const closeTranscodeRepostModal = () => {
    setTranscodeRepostOpen(false);
    setTranscodeRepostTaskId("");
    setTranscodeRepostHasBvid(false);
    setTranscodeRepostUseCurrentBvid(false);
    setTranscodeRepostSubmitting(false);
    setTranscodeRepostBaiduSync({ enabled: false, path: "", filename: "" });
    setMessage("");
  };

  const handleBilibiliAccountChange = async (nextUid) => {
    const normalized = String(nextUid || "").trim();
    if (!normalized || normalized === selectedBilibiliUid) {
      return;
    }
    setMessage("");
    try {
      const auth = await invokeCommand("auth_account_switch", {
        userId: Number(normalized),
      });
      onAuthChange?.(auth || { loggedIn: false });
      await onRefreshBaiduStatus?.();
      const profile = extractCurrentAuthProfile(auth);
      setCurrentUpProfile(profile);
      setBilibiliAccounts(Array.isArray(auth?.accounts) ? auth.accounts : []);
      const nextSelectedUid = String(auth?.activeAccount?.userId || profile.uid || normalized);
      setSelectedBilibiliUid(nextSelectedUid);
      await Promise.all([loadPartitions(nextSelectedUid), loadCollections(nextSelectedUid)]);
      await loadTasks(
        statusFilter,
        1,
        pageSize,
        false,
        taskSearch,
        sourceFilter,
        "account_switch",
        nextSelectedUid,
      );
      setCurrentPage(1);
    } catch (error) {
      setMessage(error.message || "切换投稿账号失败");
    }
  };

  const toggleTaskSelection = (taskId, checked) => {
    const normalized = String(taskId || "").trim();
    if (!normalized) {
      return;
    }
    setSelectedTaskIds((prev) => {
      const next = new Set(prev);
      if (checked) {
        next.add(normalized);
      } else {
        next.delete(normalized);
      }
      return next;
    });
  };

  const toggleCurrentPageSelection = (checked) => {
    setSelectedTaskIds((prev) => {
      const next = new Set(prev);
      tasks.forEach((task) => {
        const taskId = String(task?.taskId || "").trim();
        if (!taskId) {
          return;
        }
        if (checked) {
          next.add(taskId);
        } else {
          next.delete(taskId);
        }
      });
      return next;
    });
  };

  const handleStartSubmissionTasks = async (taskIds) => {
    const ids = (taskIds || [])
      .map((item) => String(item || "").trim())
      .filter(Boolean);
    if (ids.length === 0) {
      setMessage("请先选择待投稿任务");
      return;
    }
    try {
      await invokeCommand("submission_start", { request: { taskIds: ids } });
      setSelectedTaskIds(new Set());
      await loadTasks(statusFilter, currentPage, pageSize, false, taskSearch, sourceFilter, "start_submission");
      setMessage("已加入投稿流程");
    } catch (error) {
      setMessage(error.message);
    }
  };

  const handleIgnoreRemoteStatus = async (taskIds) => {
    const ids = (taskIds || [])
      .map((item) => String(item || "").trim())
      .filter(Boolean);
    if (ids.length === 0) {
      setMessage("请先选择未通过或已锁定的任务");
      return;
    }
    try {
      await invokeCommand("submission_ignore_remote_status", { request: { taskIds: ids } });
      setSelectedTaskIds(new Set());
      await loadTasks(statusFilter, currentPage, pageSize, false, taskSearch, sourceFilter, "ignore_remote");
      setMessage("已忽略远程投稿状态");
    } catch (error) {
      setMessage(error.message);
    }
  };

  const resetSegmentBindingStateByDetail = (detail) => {
    const defaultMergedId =
      detail?.mergedVideos?.[0]?.id !== undefined && detail?.mergedVideos?.[0]?.id !== null
        ? String(detail.mergedVideos[0].id)
        : "";
    setSegmentBindingSelection(new Set());
    setSegmentBindingMergedId(defaultMergedId);
    setSegmentBindingSubmitting(false);
  };

  const handleExportTasks = async (exportAll = false) => {
    if (!exportAll && selectedTaskIds.size === 0) {
      setMessage("请先勾选要导出的投稿任务");
      return;
    }
    const now = new Date();
    const stamp = `${now.getFullYear()}${String(now.getMonth() + 1).padStart(2, "0")}${String(
      now.getDate(),
    ).padStart(2, "0")}_${String(now.getHours()).padStart(2, "0")}${String(
      now.getMinutes(),
    ).padStart(2, "0")}${String(now.getSeconds()).padStart(2, "0")}`;
    const selected = await saveDialog({
      title: exportAll ? "导出全部投稿任务" : "导出选中投稿任务",
      defaultPath: `submission_export_${stamp}.json`,
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    if (typeof selected !== "string" || !selected.trim()) {
      return;
    }
    setExportingTasks(true);
    try {
      const result = await invokeCommand("submission_export", {
        request: {
          exportAll: exportAll,
          taskIds: exportAll ? [] : Array.from(selectedTaskIds),
          savePath: selected,
        },
      });
      await dialogMessage(
        `导出完成，共 ${result?.taskCount ?? 0} 条任务\n文件：${result?.filePath || selected}`,
        {
          title: "导出成功",
          kind: "info",
        },
      );
      setMessage(`导出成功：${result?.filePath || selected}`);
    } catch (error) {
      setMessage(error.message);
    } finally {
      setExportingTasks(false);
    }
  };

  const handleImportTasks = async () => {
    const selected = await openDialog({
      title: "选择投稿任务导入文件",
      multiple: false,
      directory: false,
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    if (typeof selected !== "string" || !selected.trim()) {
      return;
    }
    setImportingTasks(true);
    try {
      const result = await invokeCommand("submission_import", {
        request: {
          filePath: selected,
        },
      });
      await loadTasks(statusFilter, currentPage, pageSize, false, taskSearch, sourceFilter, "import");
      const summary = `总数 ${result?.totalTasks ?? 0}，导入 ${result?.importedTasks ?? 0}，跳过 ${result?.skippedTasks ?? 0}，失败 ${result?.failedTasks ?? 0}`;
      await dialogMessage(summary, {
        title: "导入完成",
        kind: "info",
      });
      setMessage(summary);
      setSelectedTaskIds(new Set());
    } catch (error) {
      setMessage(error.message);
    } finally {
      setImportingTasks(false);
    }
  };

  const closeDeleteMergedModal = (force = false) => {
    if (!force && deleteMergedSubmitting) {
      return;
    }
    setDeleteMergedOpen(false);
    setDeleteMergedTarget(null);
    setDeleteMergedLocalFile(false);
    setDeleteMergedMessage("");
  };

  const handleConfirmDeleteMerged = async () => {
    if (!deleteMergedTarget?.taskId || !deleteMergedTarget?.mergedId) {
      setDeleteMergedMessage("删除目标不存在，请重试");
      return;
    }
    setDeleteMergedSubmitting(true);
    setDeleteMergedMessage("");
    try {
      const result = await invokeCommand("submission_delete_merged_video", {
        request: {
          taskId: deleteMergedTarget.taskId,
          mergedId: deleteMergedTarget.mergedId,
          deleteLocalFile: deleteMergedLocalFile,
        },
      });
      const detail = await fetchTaskDetail(deleteMergedTarget.taskId, { log: false });
      setSelectedTask(detail);
      if (deleteMergedLocalFile) {
        setMessage("合并视频已删除，本地文件已同步删除");
      } else if (result?.archivedLocalPath) {
        setMessage(`合并视频已删除，本地文件已归档到：${result.archivedLocalPath}`);
      } else {
        setMessage("合并视频已删除");
      }
      closeDeleteMergedModal(true);
    } catch (error) {
      setDeleteMergedMessage(error?.message || "删除合并视频失败");
    } finally {
      setDeleteMergedSubmitting(false);
    }
  };

  const handleOpenBvid = async (bvid) => {
    if (!bvid) {
      return;
    }
    try {
      const { openUrl } = await import("@tauri-apps/plugin-opener");
      await openUrl(`https://www.bilibili.com/video/${bvid}`);
    } catch (error) {
      setMessage(error?.message || "打开视频链接失败");
    }
  };

  const handleOpenUpSpace = async () => {
    const uid = Number(currentUpProfile?.uid || 0);
    if (!uid) {
      setMessage("当前账号UID无效，无法打开主页");
      return;
    }
    try {
      const { openUrl } = await import("@tauri-apps/plugin-opener");
      await openUrl(`https://space.bilibili.com/${uid}`);
    } catch (error) {
      setMessage(error?.message || "打开UP主页失败");
    }
  };

  const handleOpenTaskFolder = async (taskId) => {
    if (!taskId) {
      setMessage("任务ID为空，无法打开目录");
      return;
    }
    try {
      await invokeCommand("submission_open_task_dir", { taskId });
    } catch (error) {
      try {
        const folderPath = await invokeCommand("submission_task_dir", { taskId });
        const { openPath } = await import("@tauri-apps/plugin-opener");
        await openPath(folderPath);
      } catch (fallbackError) {
        setMessage(fallbackError?.message || error?.message || "打开任务目录失败");
      }
    }
  };

  const handleDetail = async (taskId) => {
    setMessage("");
    setEditBaseline(null);
    resetSegmentBindingState();
    setDetailLoading(true);
    try {
      setSubmissionView("detail");
      setSelectedTask(null);
      const loadPromises = Promise.all([loadPartitions(), loadCollections()]);
      const detail = await fetchTaskDetail(taskId, { log: true });
      setSelectedTask(detail);
      resetSegmentBindingStateByDetail(detail);
      setDetailTab("basic");
      try {
        const task = detail?.task || {};
        const tagCount = String(task.tags || "")
          .split(",")
          .map((item) => item.trim())
          .filter((item) => item).length;
        await invokeCommand("auth_client_log", {
          message: `submission_detail_task taskId=${task.taskId || ""} titleLen=${(task.title || "").length} tags=${tagCount} sources=${detail?.sourceVideos?.length || 0}`,
        });
      } catch (_) {}
      applyDetailToForm(detail);
      try {
        await invokeCommand("auth_client_log", {
          message: `submission_detail_apply_ok taskId=${detail?.task?.taskId || ""}`,
        });
      } catch (_) {}
      await loadPromises;
    } catch (error) {
      setMessage(error.message);
      try {
        await invokeCommand("auth_client_log", {
          message: `submission_detail_fail err=${error?.message || String(error || "")}`,
        });
      } catch (_) {}
    } finally {
      setDetailLoading(false);
    }
  };

  const handleEdit = async (taskId) => {
    setMessage("");
    resetSegmentBindingState();
    setDetailLoading(true);
    try {
      const previousTaskId = lastEditTaskIdRef.current;
      if (previousTaskId && String(previousTaskId) !== String(taskId)) {
        await clearEditUploadCache(previousTaskId);
      }
      setSubmissionView("edit");
      setSelectedTask(null);
      setEditSegments([]);
      setEditSubmitConfirmOpen(false);
      setEditSubmitConfirmSyncRemote(true);
      setPendingEditSubmitPayload(null);
      setPendingEditChangedLabel("");
      setPendingEditNeedRemote(false);
      setActivityKeyword("");
      setActivityDropdownOpen(false);
      const loadPromises = Promise.all([loadPartitions(), loadCollections()]);
      const detail = await invokeCommand("submission_edit_prepare", { taskId });
      if (!detail?.task) {
        throw new Error("未读取到投稿任务详情");
      }
      setSelectedTask(detail);
      setDetailTab("basic");
      applyDetailToForm(detail);
      initEditSegments(detail);
      setEditBaseline(buildEditBaselineFromDetail(detail));
      await loadPromises;
    } catch (error) {
      setMessage(error.message);
    } finally {
      setDetailLoading(false);
    }
  };

  const handleIntegratedExecute = async (taskId) => {
    setMessage("");
    try {
      await invokeCommand("submission_integrated_execute", { taskId });
      await loadTasks(statusFilter, currentPage, pageSize);
    } catch (error) {
      setMessage(error.message);
    }
  };

  const handleResegmentMergedChange = async (event) => {
    const nextId = event.target.value;
    setResegmentMergedId(nextId);
    if (nextId) {
      await updateResegmentVideoSecondsByMerged(resegmentMergedVideos, nextId);
    } else {
      setResegmentVideoSeconds(0);
    }
  };

  const handleResegmentSubmit = async () => {
    if (!resegmentTaskId || resegmentSubmitting) {
      return;
    }
    const nextSeconds = Number(resegmentSeconds);
    if (!Number.isFinite(nextSeconds) || nextSeconds <= 0) {
      setMessage("分段时长必须大于0");
      return;
    }
    if (resegmentIntegrateCurrent && !resegmentHasBvid) {
      setMessage("当前任务暂无BVID，无法集成投稿，请选择新建BV");
      return;
    }
    if (resegmentMode === "SPECIFIED" && !resegmentMergedId) {
      setMessage("请选择合并视频");
      return;
    }
    setMessage("");
    setResegmentSubmitting(true);
    try {
      await invokeCommand("submission_resegment", {
        request: {
          taskId: resegmentTaskId,
          segmentDurationSeconds: Math.floor(nextSeconds),
          mode: resegmentMode,
          mergedVideoId:
            resegmentMode === "SPECIFIED" && resegmentMergedId
              ? Number(resegmentMergedId)
              : null,
          integrateCurrentBvid: resegmentIntegrateCurrent,
        },
      });
      closeResegmentModal();
      await loadTasks(statusFilter, currentPage, pageSize);
    } catch (error) {
      setMessage(error.message);
      await showErrorDialog(error);
      setResegmentSubmitting(false);
    }
  };

  const handleRepostSubmit = async () => {
    if (!repostTaskId || repostSubmitting) {
      return;
    }
    if (repostMode === "SPECIFIED" && !repostMergedId) {
      setMessage("请选择合并视频");
      return;
    }
    if (repostMode === "FULL_REPROCESS" && repostUseCurrentBvid && !repostHasBvid) {
      setMessage("当前任务没有BV号，无法集成投稿");
      return;
    }
    setMessage("");
    setRepostSubmitting(true);
    try {
      const result = await invokeCommand("submission_repost", {
        request: {
          taskId: repostTaskId,
          integrateCurrentBvid: repostUseCurrentBvid,
          mode: repostMode,
          mergedVideoId:
            repostMode === "SPECIFIED" && repostMergedId
              ? Number(repostMergedId)
              : null,
          baiduSyncEnabled: Boolean(repostBaiduSync.enabled),
          baiduSyncPath: repostBaiduSync.path || null,
          baiduSyncFilename: repostBaiduSync.filename || null,
        },
      });
      setMessage(result || "已提交重新投稿");
      closeRepostModal();
      await loadTasks(statusFilter, currentPage, pageSize);
      if (typeof window !== "undefined") {
        window.dispatchEvent(new CustomEvent("anchor-clip-list-refresh"));
      }
    } catch (error) {
      setMessage(error.message);
      await showErrorDialog(error);
      setRepostSubmitting(false);
    }
  };

  const handleTranscodeRepostSubmit = async () => {
    if (!transcodeRepostTaskId || transcodeRepostSubmitting) {
      return;
    }
    if (transcodeRepostUseCurrentBvid && !transcodeRepostHasBvid) {
      setMessage("当前任务没有BV号，无法集成投稿");
      return;
    }
    setMessage("");
    setTranscodeRepostSubmitting(true);
    try {
      const result = await invokeCommand("submission_repost_transcode", {
        request: {
          taskId: transcodeRepostTaskId,
          integrateCurrentBvid: transcodeRepostUseCurrentBvid,
          baiduSyncEnabled: Boolean(transcodeRepostBaiduSync.enabled),
          baiduSyncPath: transcodeRepostBaiduSync.path || null,
          baiduSyncFilename: transcodeRepostBaiduSync.filename || null,
        },
      });
      setMessage(result || "转码投稿已启动");
      closeTranscodeRepostModal();
      await loadTasks(statusFilter, currentPage, pageSize);
      if (typeof window !== "undefined") {
        window.dispatchEvent(new CustomEvent("anchor-clip-list-refresh"));
      }
    } catch (error) {
      setMessage(error.message);
      await showErrorDialog(error);
      setTranscodeRepostSubmitting(false);
    }
  };

  const handleDeleteTask = async (taskId) => {
    setMessage("");
    setDeleteMessage("");
    setDeleteMessageTone("info");
    const targetId = String(taskId || "").trim();
    if (!targetId) {
      setMessage("任务ID无效，无法删除");
      return;
    }
    setDeleteTargetId(targetId);
    setDeleteTaskChecked(false);
    setDeleteFilesChecked(true);
    setDeleteFileSelections(new Set());
    setDeletePreview(null);
    setDeleteConfirmOpen(true);
    setDeleteConflictOpen(false);
    setDeleteConflictFiles([]);
    setDeletePendingPayload(null);
    setDeletePreviewLoading(true);
    try {
      const preview = await invokeCommand("submission_delete_preview", { taskId: targetId });
      setDeletePreview(preview);
      const defaultSelections = new Set(
        (preview?.files || []).map((item) => item.path),
      );
      setDeleteFileSelections(defaultSelections);
    } catch (error) {
      setMessage(error.message);
    }
    try {
      await invokeCommand("auth_client_log", {
        message: `submission_delete_prompt taskId=${targetId}`,
      });
    } catch (_) {}
    setDeletePreviewLoading(false);
  };

  const handleBatchDeleteTasks = async () => {
    setMessage("");
    setDeleteMessage("");
    setDeleteMessageTone("info");
    const targetIds = Array.from(selectedTaskIds)
      .map((value) => String(value || "").trim())
      .filter((value) => value);
    if (targetIds.length === 0) {
      setMessage("请选择要删除的投稿任务");
      return;
    }
    setDeleteTargetId("");
    setDeleteTargetIds(targetIds);
    setDeleteTaskChecked(false);
    setDeleteFilesChecked(true);
    setDeleteFileSelections(new Set());
    setDeletePreview(null);
    setDeleteBatchPreviews([]);
    setDeleteConfirmOpen(true);
    setDeleteConflictOpen(false);
    setDeleteConflictFiles([]);
    setDeletePendingPayload(null);
    setDeletePreviewLoading(true);
    try {
      const previews = [];
      const mergedFiles = [];
      const seenPaths = new Set();
      for (const taskId of targetIds) {
        const preview = await invokeCommand("submission_delete_preview", { taskId });
        previews.push(preview);
        for (const file of preview?.files || []) {
          if (seenPaths.has(file.path)) {
            continue;
          }
          seenPaths.add(file.path);
          mergedFiles.push(file);
        }
      }
      setDeleteBatchPreviews(previews);
      setDeletePreview({ taskId: "", files: mergedFiles });
      setDeleteFileSelections(new Set(mergedFiles.map((item) => item.path)));
    } catch (error) {
      setDeleteMessage(`读取批量删除预览失败：${error?.message || "未知错误"}`);
      setDeleteMessageTone("error");
    } finally {
      setDeletePreviewLoading(false);
    }
  };

  const handleDeleteCancel = async () => {
    const targetId = deleteTargetId;
    resetDeleteState();
    try {
      await invokeCommand("auth_client_log", {
        message: `submission_delete_cancel taskId=${targetId}`,
      });
    } catch (_) {}
  };

  const handleDeleteConfirm = async () => {
    const targetId = deleteTargetId;
    setDeleteMessage("");
    setDeleteMessageTone("info");
    try {
      await invokeCommand("auth_client_log", {
        message: `submission_delete_confirm_click taskId=${targetId} deleteTask=${deleteTaskChecked ? "1" : "0"} deleteFiles=${deleteFilesChecked ? "1" : "0"}`,
      });
    } catch (_) {}
    if (isBatchDelete) {
      await handleBatchDeleteConfirm(false);
      return;
    }
    if (!targetId) {
      resetDeleteState();
      return;
    }
    if (!deleteTaskChecked && !deleteFilesChecked) {
      setDeleteMessage("请选择删除投稿任务或删除视频文件");
      setDeleteMessageTone("error");
      try {
        await invokeCommand("auth_client_log", {
          message: `submission_delete_validation_fail taskId=${targetId} reason=no_option`,
        });
      } catch (_) {}
      return;
    }
    const deletePaths = deleteFilesChecked ? Array.from(deleteFileSelections) : [];
    if (deleteFilesChecked && deleteHasFiles && deletePaths.length === 0) {
      setDeleteMessage("请选择要删除的视频文件");
      setDeleteMessageTone("error");
      try {
        await invokeCommand("auth_client_log", {
          message: `submission_delete_validation_fail taskId=${targetId} reason=no_paths`,
        });
      } catch (_) {}
      return;
    }
    const payload = {
      taskId: targetId,
      deleteTask: deleteTaskChecked,
      deleteFiles: deleteFilesChecked,
      deletePaths,
      forceDelete: false,
    };
    setDeleteSubmitting(true);
    setDeleteMessage("正在删除...");
    setDeleteMessageTone("info");
    try {
      try {
        await invokeCommand("auth_client_log", {
          message: `submission_delete_invoke_start taskId=${targetId} deleteTask=${deleteTaskChecked ? "1" : "0"} deleteFiles=${deleteFilesChecked ? "1" : "0"} paths=${deletePaths.length}`,
        });
      } catch (_) {}
      const result = await invokeCommand("submission_delete", { request: payload });
      try {
        await invokeCommand("auth_client_log", {
          message: `submission_delete_invoke_ok taskId=${targetId} deleted=${result?.deletedPaths?.length || 0} missing=${result?.missingPaths?.length || 0} blocked=${result?.blocked ? "1" : "0"}`,
        });
      } catch (_) {}
      if (result?.blocked) {
        setDeletePendingPayload(payload);
        setDeleteConflictFiles(result.conflicts || []);
        setDeleteConflictOpen(true);
        return;
      }
      const deletedPaths = result?.deletedPaths || [];
      const missingPaths = result?.missingPaths || [];
      try {
        await invokeCommand("auth_client_log", {
          message: `submission_delete_ok taskId=${targetId}`,
        });
      } catch (_) {}
      if (deleteTaskChecked) {
        if (selectedTask?.task?.taskId === targetId) {
          setSelectedTask(null);
        }
        setTasks((prev) => prev.filter((item) => item.taskId !== targetId));
        await loadTasks(statusFilter);
        const summary = buildDeleteSummary(deletedPaths, missingPaths);
        resetDeleteState();
        await notifyDeleteSuccess(summary);
        return;
      }
      if (deleteFilesChecked) {
        const summary = buildDeleteSummary(deletedPaths, missingPaths);
        resetDeleteState();
        await notifyDeleteSuccess(summary);
      }
    } catch (error) {
      try {
        await invokeCommand("auth_client_log", {
          message: `submission_delete_fail taskId=${targetId} err=${error?.message || String(error || "")}`,
        });
      } catch (_) {}
      try {
        await invokeCommand("auth_client_log", {
          message: `submission_delete_invoke_fail taskId=${targetId} err=${error?.message || String(error || "")}`,
        });
      } catch (_) {}
      setDeleteMessage(`删除失败：${error?.message || "未知错误"}`);
      setDeleteMessageTone("error");
    } finally {
      setDeleteSubmitting(false);
    }
  };

  const handleBatchDeleteConfirm = async (forceDelete) => {
    const targetIds = deleteTargetIds;
    if (targetIds.length === 0) {
      resetDeleteState();
      return;
    }
    if (!deleteTaskChecked && !deleteFilesChecked) {
      setDeleteMessage("请选择删除投稿任务或删除视频文件");
      setDeleteMessageTone("error");
      return;
    }
    const selectedPaths = deleteFilesChecked ? new Set(deleteFileSelections) : new Set();
    if (deleteFilesChecked && deleteHasFiles && selectedPaths.size === 0) {
      setDeleteMessage("请选择要删除的视频文件");
      setDeleteMessageTone("error");
      return;
    }
    if (!forceDelete && deleteFilesChecked) {
      const conflicts = [];
      for (const preview of deleteBatchPreviews) {
        for (const file of preview?.files || []) {
          if (selectedPaths.has(file.path) && (file.conflicts || []).length > 0) {
            conflicts.push(file);
          }
        }
      }
      if (conflicts.length > 0) {
        setDeletePendingPayload({ batch: true });
        setDeleteConflictFiles(conflicts);
        setDeleteConflictOpen(true);
        return;
      }
    }
    setDeleteSubmitting(true);
    setDeleteMessage(`正在删除 ${targetIds.length} 个任务...`);
    setDeleteMessageTone("info");
    try {
      const blockedFiles = [];
      const deletedPaths = [];
      const missingPaths = [];
      for (const taskId of targetIds) {
        const preview = deleteBatchPreviews.find((item) => item.taskId === taskId);
        const deletePaths = deleteFilesChecked
          ? (preview?.files || [])
              .map((item) => item.path)
              .filter((path) => selectedPaths.has(path))
          : [];
        const result = await invokeCommand("submission_delete", {
          request: {
            taskId,
            deleteTask: deleteTaskChecked,
            deleteFiles: deleteFilesChecked,
            deletePaths,
            forceDelete,
          },
        });
        if (result?.blocked) {
          blockedFiles.push(...(result.conflicts || []));
          continue;
        }
        deletedPaths.push(...(result?.deletedPaths || []));
        missingPaths.push(...(result?.missingPaths || []));
      }
      if (blockedFiles.length > 0) {
        setDeletePendingPayload({ batch: true, forceDelete: true });
        setDeleteConflictFiles(blockedFiles);
        setDeleteConflictOpen(true);
        return;
      }
      if (deleteTaskChecked) {
        if (selectedTask && targetIds.includes(selectedTask?.task?.taskId)) {
          setSelectedTask(null);
        }
        setSelectedTaskIds((prev) => {
          const next = new Set(prev);
          targetIds.forEach((taskId) => next.delete(taskId));
          return next;
        });
        setTasks((prev) => prev.filter((item) => !targetIds.includes(item.taskId)));
        await loadTasks(statusFilter, currentPage, pageSize, false, taskSearch, sourceFilter, "batch_delete");
      }
      const summary = buildDeleteSummary(deletedPaths, missingPaths);
      resetDeleteState();
      await notifyDeleteSuccess(summary ? `${targetIds.length} 个任务处理完成，${summary}` : `${targetIds.length} 个任务处理完成`);
    } catch (error) {
      setDeleteMessage(`批量删除失败：${error?.message || "未知错误"}`);
      setDeleteMessageTone("error");
    } finally {
      setDeleteSubmitting(false);
    }
  };

  const handleDeleteFilesToggle = (checked) => {
    setDeleteFilesChecked(checked);
    if (checked && deleteFileSelections.size === 0) {
      const defaultSelections = new Set(deleteFiles.map((item) => item.path));
      setDeleteFileSelections(defaultSelections);
    }
  };

  const toggleDeleteFileSelection = (path) => {
    setDeleteFileSelections((prev) => {
      const next = new Set(prev);
      if (next.has(path)) {
        next.delete(path);
      } else {
        next.add(path);
      }
      return next;
    });
  };

  const buildDeleteSummary = (deletedPaths, missingPaths) => {
    const summaryParts = [];
    if (deletedPaths.length > 0) {
      summaryParts.push(`已删除 ${deletedPaths.length} 个`);
    }
    if (missingPaths.length > 0) {
      summaryParts.push(`未找到 ${missingPaths.length} 个`);
    }
    return summaryParts.join("，");
  };

  const notifyDeleteSuccess = async (summary) => {
    const text = summary ? `删除成功，${summary}` : "删除成功";
    try {
      await dialogMessage(text, {
        title: "删除成功",
        kind: "info",
      });
    } catch (_) {}
  };

  const handleDeleteConflictCancel = () => {
    setDeleteConflictOpen(false);
    setDeleteConflictFiles([]);
    setDeletePendingPayload(null);
    setDeleteMessage("");
    setDeleteMessageTone("info");
  };

  const handleDeleteConflictConfirm = async () => {
    const payload = deletePendingPayload;
    if (!payload) {
      handleDeleteConflictCancel();
      return;
    }
    if (payload.batch) {
      setDeleteConflictOpen(false);
      setDeleteConflictFiles([]);
      await handleBatchDeleteConfirm(true);
      return;
    }
    setDeleteSubmitting(true);
    setDeleteMessage("正在删除...");
    setDeleteMessageTone("info");
    try {
      const result = await invokeCommand("submission_delete", {
        request: {
          ...payload,
          forceDelete: true,
        },
      });
      try {
        await invokeCommand("auth_client_log", {
          message: `submission_delete_invoke_ok taskId=${payload.taskId} deleted=${result?.deletedPaths?.length || 0} missing=${result?.missingPaths?.length || 0} blocked=${result?.blocked ? "1" : "0"}`,
        });
      } catch (_) {}
      if (result?.blocked) {
        setDeleteConflictFiles(result.conflicts || []);
        return;
      }
      const deletedPaths = result?.deletedPaths || [];
      const missingPaths = result?.missingPaths || [];
      if (payload.deleteTask) {
        if (selectedTask?.task?.taskId === payload.taskId) {
          setSelectedTask(null);
        }
        setTasks((prev) => prev.filter((item) => item.taskId !== payload.taskId));
        await loadTasks(statusFilter);
        const summary = buildDeleteSummary(deletedPaths, missingPaths);
        resetDeleteState();
        await notifyDeleteSuccess(summary);
        return;
      }
      if (payload.deleteFiles) {
        const summary = buildDeleteSummary(deletedPaths, missingPaths);
        resetDeleteState();
        await notifyDeleteSuccess(summary);
      }
    } catch (error) {
      setDeleteMessage(`删除失败：${error?.message || "未知错误"}`);
      setDeleteMessageTone("error");
    } finally {
      setDeleteSubmitting(false);
    }
  };

  const handleWorkflowPause = async (taskId) => {
    setMessage("");
    try {
      await invokeCommand("workflow_pause", { task_id: taskId });
      await loadTasks(statusFilter);
    } catch (error) {
      setMessage(error.message);
    }
  };

  const handleWorkflowResume = async (taskId) => {
    setMessage("");
    try {
      await invokeCommand("workflow_resume", { task_id: taskId });
      await loadTasks(statusFilter);
    } catch (error) {
      setMessage(error.message);
    }
  };

  const handleWorkflowCancel = async (taskId) => {
    setMessage("");
    try {
      await invokeCommand("workflow_cancel", { task_id: taskId });
      await loadTasks(statusFilter);
    } catch (error) {
      setMessage(error.message);
    }
  };

  const handleQueuePrioritize = async (taskId) => {
    setMessage("");
    const confirmed = await dialogConfirm("确认设置该任务为优先投稿？如果已设置，将刷新优先时间。", {
      title: "优先投稿",
      kind: "warning",
    });
    if (!confirmed) {
      return;
    }
    try {
      await invokeCommand("submission_queue_prioritize", { taskId });
      await loadTasks(statusFilter);
      await dialogMessage("已设置为优先投稿", {
        title: "操作成功",
        kind: "info",
      });
    } catch (error) {
      setMessage(error.message);
    }
  };

  const handleWorkflowRefresh = async () => {
    setRefreshingRemote(true);
    try {
      await loadTasks(statusFilter, currentPage, pageSize, true);
    } finally {
      setRefreshingRemote(false);
    }
  };

  const handleTaskRemoteVerification = async (task) => {
    const taskId = String(task?.taskId || "").trim();
    if (!taskId || verifyingRemoteTaskId) {
      return;
    }
    setVerifyingRemoteTaskId(taskId);
    setMessage("正在校验当前任务的远端稿件...");
    try {
      const result = await invokeCommand("submission_verify_remote", { taskId });
      setMessage(result || "远端校验完成");
    } catch (error) {
      setMessage(`远端校验失败：${error.message}`);
    } finally {
      await loadTasks(statusFilter, currentPage, pageSize);
      setVerifyingRemoteTaskId("");
    }
  };

  const handleConfirmRemoteMissing = async (task) => {
    const taskId = String(task?.taskId || "").trim();
    if (!taskId || !canConfirmRemoteMissing(task)) {
      return;
    }
    setRemoteMissingConfirmTask(task);
  };

  const confirmRemoteMissing = async () => {
    const task = remoteMissingConfirmTask;
    const taskId = String(task?.taskId || "").trim();
    if (!taskId || !canConfirmRemoteMissing(task) || remoteMissingConfirming) {
      return;
    }
    setRemoteMissingConfirming(true);
    setMessage("");
    try {
      const result = await invokeCommand("submission_confirm_remote_missing", { taskId });
      setMessage(result || "已重新加入投稿队列");
      setRemoteMissingConfirmTask(null);
      await loadTasks(statusFilter, currentPage, pageSize);
    } catch (error) {
      setMessage(error.message);
    } finally {
      setRemoteMissingConfirming(false);
    }
  };

  const handleReuploadAllSegments = (task) => {
    if (!canReuploadAllSegments(task)) {
      return;
    }
    setReuploadConfirmTask(task);
  };

  const confirmReuploadAllSegments = async () => {
    const task = reuploadConfirmTask;
    const taskId = String(task?.taskId || "").trim();
    if (!taskId || !canReuploadAllSegments(task) || reuploadConfirming) {
      return;
    }
    setReuploadConfirming(true);
    setMessage("");
    try {
      const result = await invokeCommand("submission_reupload_all_segments", { taskId });
      setMessage(result || "已重新加入分P上传队列");
      setReuploadConfirmTask(null);
      await loadTasks(statusFilter, currentPage, pageSize);
    } catch (error) {
      setMessage(error.message);
    } finally {
      setReuploadConfirming(false);
    }
  };

  const formatWorkflowStatus = (status) => {
    switch (status) {
      case "PENDING":
        return "待处理";
      case "RUNNING":
        return "运行中";
      case "VIDEO_DOWNLOADING":
        return "视频下载中";
      case "VERIFY_PENDING":
        return "待远端校验";
      case "SUBMIT_UNKNOWN":
        return "提交结果待确认";
      case "REUPLOAD_PENDING":
        return "待重新上传";
      case "PAUSED":
        return "已暂停";
      case "COMPLETED":
        return "已完成";
      case "FAILED":
        return "失败";
      case "CANCELLED":
        return "已取消";
      default:
        return status || "-";
    }
  };

  const taskStatusTone = (status) => {
    if (status === "COMPLETED") return "bg-emerald-500/10 text-emerald-600";
    if (status === "FAILED" || status === "CANCELLED")
      return "bg-rose-500/10 text-rose-600";
    if (
      [
        "UPLOADING",
        "WAITING_UPLOAD",
        "RUNNING",
        "VERIFY_PENDING",
        "SUBMIT_UNKNOWN",
        "REUPLOAD_PENDING",
      ].includes(status)
    ) {
      return "bg-amber-500/10 text-amber-600";
    }
    if (["CLIPPING", "MERGING", "SEGMENTING", "PENDING", "PENDING_SUBMIT"].includes(status)) {
      return "bg-amber-500/10 text-amber-600";
    }
    return "bg-slate-500/10 text-slate-600";
  };

  const remoteStatusTone = (status) => {
    if (status === "已通过") return "bg-emerald-500/10 text-emerald-600";
    if (status === "未通过" || status === "已锁定")
      return "bg-rose-500/10 text-rose-600";
    if (status === "审核中" || status === "未同步")
      return "bg-amber-500/10 text-amber-600";
    return "bg-slate-500/10 text-slate-600";
  };

  const workflowStatusTone = (status) => {
    if (status === "COMPLETED") return "bg-emerald-500/10 text-emerald-600";
    if (status === "FAILED" || status === "CANCELLED")
      return "bg-rose-500/10 text-rose-600";
    if (
      [
        "RUNNING",
        "VIDEO_DOWNLOADING",
        "VERIFY_PENDING",
        "SUBMIT_UNKNOWN",
        "REUPLOAD_PENDING",
      ].includes(status)
    )
      return "bg-amber-500/10 text-amber-600";
    if (status === "PAUSED") return "bg-slate-500/10 text-slate-600";
    return "bg-slate-500/10 text-slate-600";
  };

  const formatWorkflowStep = (step) => {
    switch (step) {
      case "CLIPPING":
        return "剪辑";
      case "MERGING":
        return "合并";
      case "SEGMENTING":
        return "分段";
      default:
        return step || "-";
    }
  };

  const parseRemoteState = (task) => {
    const raw = task?.remoteState;
    if (raw === null || raw === undefined) {
      return null;
    }
    const text = String(raw).trim();
    if (!text) {
      return null;
    }
    const state = Number(text);
    if (!Number.isFinite(state)) {
      return null;
    }
    return state;
  };

  const resolveRemoteStatus = (task) => {
    if (task?.remoteStatusIgnored) {
      return "已通过";
    }
    if (!task?.bvid) {
      return "未生成";
    }
    const stateDesc = String(task?.remoteStateDesc || "").trim();
    if (stateDesc) {
      return stateDesc;
    }
    const state = parseRemoteState(task);
    if (state === null) {
      return "未同步";
    }
    if (state === -2) {
      return "未通过";
    }
    if (state === -4) {
      return "已锁定";
    }
    if (state === 0) {
      return "已通过";
    }
    if (state === -30) {
      return "审核中";
    }
    return `未知（${state}）`;
  };

  const isRemoteRejected = (task) => {
    if (task?.remoteStatusIgnored) {
      return false;
    }
    const state = parseRemoteState(task);
    if (state === null) {
      return false;
    }
    return state === -2 || state === -4;
  };

  const isRemoteFailed = (task) => {
    if (task?.remoteStatusIgnored) {
      return false;
    }
    const state = parseRemoteState(task);
    if (state === null) {
      return false;
    }
    return state === -2 || state === -4;
  };

  const resolveRejectReason = (task) => {
    if (!isRemoteRejected(task)) {
      return "-";
    }
    return task?.rejectReason || "-";
  };

  const resolveTaskFailureReason = (task) => {
    if (!["FAILED", "VERIFY_PENDING", "SUBMIT_UNKNOWN"].includes(task?.status)) {
      return "-";
    }
    const message = String(task?.workflowStatus?.errorMessage || "").trim();
    if (message) {
      return message;
    }
    const currentStep = String(task?.workflowStatus?.currentStep || "").trim();
    if (currentStep) {
      return `${formatWorkflowStep(currentStep)}失败`;
    }
    return "-";
  };

  const resolveMergedVideoLabel = (merged) => {
    if (!merged) {
      return "未命名合并视频";
    }
    const fileName = merged.fileName || "";
    const pathName = merged.videoPath ? merged.videoPath.split("/").pop() : "";
    const displayName = fileName || pathName || `合并视频 ${merged.id ?? ""}`.trim();
    const createdAt = merged.createTime ? ` ${merged.createTime}` : "";
    return `${displayName}${createdAt}`.trim();
  };

  const currentUpName = String(currentUpProfile?.name || "").trim() || "-";

  const currentUpUid = Number(currentUpProfile?.uid || 0);

  const resolveResegmentCount = (durationSeconds, segmentSecondsValue) => {
    const duration = Number(durationSeconds);
    const segmentSeconds = Math.floor(Number(segmentSecondsValue));
    if (!Number.isFinite(duration) || duration <= 0) {
      return null;
    }
    if (!Number.isFinite(segmentSeconds) || segmentSeconds <= 0) {
      return null;
    }
    return Math.ceil(duration / segmentSeconds);
  };

  const totalPages = Math.max(1, Math.ceil(totalTasks / pageSize));

  const currentPageTaskIds = tasks
    .map((task) => String(task?.taskId || "").trim())
    .filter((taskId) => taskId);

  const allCurrentPageSelected =
    currentPageTaskIds.length > 0 &&
    currentPageTaskIds.every((taskId) => selectedTaskIds.has(taskId));

  const selectedTaskCount = selectedTaskIds.size;

  const selectedTasks = tasks.filter((task) =>
    selectedTaskIds.has(String(task?.taskId || "").trim()),
  );

  const selectedPendingSubmitIds = selectedTasks
    .filter((task) => task.status === "PENDING_SUBMIT")
    .map((task) => task.taskId);

  const selectedRemoteRejectedIds = selectedTasks
    .filter((task) => isRemoteRejected(task))
    .map((task) => task.taskId);

  if (!(submissionView === "list")) {
    return null;
  }

  return (
    <>
          <div className="rounded-2xl bg-white/80 shadow-sm ring-1 ring-black/5">
        <div className="flex flex-wrap items-center justify-between gap-3 border-b border-black/5 px-6 py-4">
          <div className="text-sm font-semibold uppercase tracking-[0.2em] text-[var(--muted)]">
            投稿任务列表
          </div>
          <div className="flex flex-wrap gap-2">
            <select
              value={selectedBilibiliUid}
              onChange={(event) => handleBilibiliAccountChange(event.target.value)}
              className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
            >
              <option value="">选择投稿账号</option>
              {bilibiliAccounts.map((account) => (
                <option key={account.userId} value={account.userId}>
                  {(account.nickname || account.username || `UID ${account.userId}`) +
                    (account.isActive ? "（当前）" : "")}
                </option>
              ))}
            </select>
            <input
              value={taskSearch}
              onChange={(event) => {
                setTaskSearch(event.target.value);
                setCurrentPage(1);
              }}
              placeholder="标题或BV号搜索"
              className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
            />
            <button
              className="rounded-full bg-[var(--accent)] px-3 py-1 text-xs font-semibold text-white"
              onClick={openCreateView}
            >
              新增投稿任务
            </button>
            <button
              className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
              onClick={openRemoteImportDialog}
            >
              视频导入
            </button>
            <button
              className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)] transition hover:border-black/20 disabled:cursor-not-allowed disabled:opacity-60"
              onClick={() => handleExportTasks(false)}
              disabled={exportingTasks || importingTasks || selectedTaskCount === 0}
            >
              {exportingTasks ? "导出中" : `导出选中(${selectedTaskCount})`}
            </button>
            <button
              className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)] transition hover:border-black/20 disabled:cursor-not-allowed disabled:opacity-60"
              onClick={() => handleExportTasks(true)}
              disabled={exportingTasks || importingTasks}
            >
              导出全部
            </button>
            <button
              className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)] transition hover:border-black/20 disabled:cursor-not-allowed disabled:opacity-60"
              onClick={handleImportTasks}
              disabled={importingTasks || exportingTasks}
            >
              {importingTasks ? "导入中" : "导入"}
            </button>
            <button
              className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)] transition hover:border-black/20 disabled:cursor-not-allowed disabled:opacity-60"
              onClick={handleWorkflowRefresh}
              disabled={refreshingRemote}
            >
              {refreshingRemote ? "刷新中" : "刷新"}
            </button>
            <button
              className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)] transition hover:border-black/20 disabled:cursor-not-allowed disabled:opacity-60"
              onClick={() => handleStartSubmissionTasks(selectedPendingSubmitIds)}
              disabled={selectedPendingSubmitIds.length === 0}
            >
              开始投稿({selectedPendingSubmitIds.length})
            </button>
            <button
              className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)] transition hover:border-black/20 disabled:cursor-not-allowed disabled:opacity-60"
              onClick={() => handleIgnoreRemoteStatus(selectedRemoteRejectedIds)}
              disabled={selectedRemoteRejectedIds.length === 0}
            >
              忽略投稿({selectedRemoteRejectedIds.length})
            </button>
            <button
              className="rounded-full border border-red-200 bg-white px-3 py-1 text-xs font-semibold text-red-600 transition hover:border-red-300 disabled:cursor-not-allowed disabled:opacity-60"
              onClick={handleBatchDeleteTasks}
              disabled={selectedTaskCount === 0}
            >
              批量删除({selectedTaskCount})
            </button>
            <select
              value={statusFilter}
              onChange={(event) => {
                setStatusFilter(event.target.value);
                setCurrentPage(1);
              }}
              className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
            >
              {statusFilters.map((item) => (
                <option key={item.value} value={item.value}>
                  {item.label}
                </option>
              ))}
            </select>
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-2 border-b border-black/5 px-6 py-2">
          {sourceFilters.map((item) => {
            const active = sourceFilter === item.value;
            return (
              <button
                key={item.value}
                className={`tab-btn ${active ? "active" : ""}`}
                onClick={() => {
                  if (sourceFilter === item.value) {
                    return;
                  }
                  setSourceFilter(item.value);
                  setCurrentPage(1);
                  loadTasks(statusFilter, 1, pageSize, false, taskSearch, item.value, "tab");
                }}
              >
                {item.label}
              </button>
            );
          })}
        </div>
        <div className="overflow-x-auto">
          <table className="w-full min-w-max table-auto text-left text-sm whitespace-nowrap">
            <thead className="bg-black/5 text-xs uppercase tracking-[0.2em] text-[var(--muted)] whitespace-nowrap">
              <tr>
                <th className="px-3 py-3">
                  <input
                    type="checkbox"
                    checked={allCurrentPageSelected}
                    onChange={(event) => toggleCurrentPageSelection(event.target.checked)}
                    aria-label="全选当前页"
                  />
                </th>
                <th className="px-6 py-3">标题</th>
                <th className="px-6 py-3">任务状态</th>
                <th className="px-6 py-3">工作流状态</th>
                <th className="px-6 py-3">投稿状态</th>
                <th className="px-6 py-3">BVID</th>
                <th className="px-6 py-3">UP主</th>
                <th className="px-6 py-3">优先时间</th>
                <th className="px-6 py-3">创建时间</th>
                <th className="px-6 py-3">更新时间</th>
                <th className="sticky right-0 z-20 bg-[var(--surface)] px-6 py-3 shadow-[-6px_0_10px_-6px_rgba(0,0,0,0.2)]">
                  操作
                </th>
              </tr>
            </thead>
            <tbody className="whitespace-nowrap">
              {tasks.length === 0 ? (
                <tr>
                  <td className="px-6 py-4 text-[var(--muted)]" colSpan={11}>
                    暂无任务。
                  </td>
                </tr>
              ) : (
                tasks.map((task) => (
                  <tr key={task.taskId} className="border-t border-black/5">
                    <td className="px-3 py-3">
                      <input
                        type="checkbox"
                        checked={selectedTaskIds.has(String(task.taskId || "").trim())}
                        onChange={(event) =>
                          toggleTaskSelection(task.taskId, event.target.checked)
                        }
                        aria-label={`选择任务 ${task.title || task.taskId || ""}`}
                      />
                    </td>
                    <td className="px-6 py-3 text-[var(--ink)] whitespace-normal">
                      <button
                        className="text-left font-semibold text-[var(--ink)] transition hover:text-[var(--accent)] hover:underline hover:underline-offset-4 break-words"
                        title="打开任务目录"
                        onClick={() => handleOpenTaskFolder(task.taskId)}
                      >
                        {task.title || "-"}
                        {task.priority ? (
                          <span className="ml-2 rounded-full bg-amber-500/10 px-2 py-0.5 text-[10px] font-semibold text-amber-600">
                            优先
                          </span>
                        ) : null}
                      </button>
                    </td>
                    <td className="px-6 py-3 text-[var(--muted)] whitespace-nowrap">
                      {(() => {
                        const reason = resolveTaskFailureReason(task);
                        const showTooltip =
                          ["FAILED", "VERIFY_PENDING", "SUBMIT_UNKNOWN"].includes(task?.status) &&
                          reason !== "-";
                        return (
                          <div className="group relative inline-block">
                            <span
                              className={`rounded-full px-2 py-0.5 text-xs font-semibold whitespace-nowrap ${taskStatusTone(
                                task.status,
                              )}`}
                            >
                              {formatTaskStatus(task.status)}
                            </span>
                            {showTooltip ? (
                              <div className="pointer-events-none absolute left-0 top-full z-50 mt-1 w-[320px] max-w-[360px] rounded-lg border border-black/10 bg-white px-3 py-2 text-xs text-[var(--ink)] shadow-lg opacity-0 transition-opacity duration-150 group-hover:opacity-100">
                                <div className="whitespace-normal break-words">{reason}</div>
                              </div>
                            ) : null}
                          </div>
                        );
                      })()}
                    </td>
                    <td className="px-6 py-3 whitespace-nowrap">
                      {task.workflowStatus ? (
                        <div className="space-y-1">
                          <span
                            className={`rounded-full px-2 py-0.5 text-xs font-semibold whitespace-nowrap ${workflowStatusTone(
                              task.workflowStatus.status,
                            )}`}
                          >
                            {formatWorkflowStatus(task.workflowStatus.status)}
                          </span>
                          {task.workflowStatus.status === "RUNNING" ? (
                            <div className="h-1.5 w-24 rounded-full bg-black/5">
                              <div
                                className="h-1.5 rounded-full bg-[var(--accent)]"
                                style={{
                                  width: `${Math.min(
                                    100,
                                    task.workflowStatus.progress || 0,
                                  )}%`,
                                }}
                              />
                            </div>
                          ) : null}
                          {task.workflowStatus.currentStep ? (
                            <div className="text-xs text-[var(--muted)]">
                              当前步骤：{formatWorkflowStep(task.workflowStatus.currentStep)}
                            </div>
                          ) : null}
                        </div>
                      ) : (
                        <span className="text-xs text-[var(--muted)]">无工作流</span>
                      )}
                    </td>
                    <td className="px-6 py-3 text-[var(--muted)] whitespace-nowrap">
                      {(() => {
                        const reason = resolveRejectReason(task);
                        const showTooltip = isRemoteFailed(task) && reason !== "-";
                        const remoteStatus = resolveRemoteStatus(task);
                        return (
                          <div className="group relative inline-block">
                            <span
                              className={`rounded-full px-2 py-0.5 text-xs font-semibold whitespace-nowrap ${remoteStatusTone(
                                remoteStatus,
                              )}`}
                            >
                              {remoteStatus}
                            </span>
                            {showTooltip ? (
                              <div className="pointer-events-none absolute left-0 top-full z-50 mt-1 w-[320px] max-w-[360px] rounded-lg border border-black/10 bg-white px-3 py-2 text-xs text-[var(--ink)] shadow-lg opacity-0 transition-opacity duration-150 group-hover:opacity-100">
                                <div className="whitespace-normal break-words">{reason}</div>
                              </div>
                            ) : null}
                          </div>
                        );
                      })()}
                    </td>
                    <td className="px-6 py-3 whitespace-nowrap">
                      {task.bvid ? (
                        <button
                          className="text-xs font-semibold text-[var(--accent)] underline underline-offset-2"
                          onClick={() => handleOpenBvid(task.bvid)}
                        >
                          {task.bvid}
                        </button>
                      ) : (
                        <span className="text-[var(--muted)]">-</span>
                      )}
                    </td>
                    <td className="px-6 py-3 whitespace-nowrap">
                      {currentUpUid > 0 ? (
                        <button
                          className="text-xs font-semibold text-[var(--accent)] underline underline-offset-2"
                          onClick={handleOpenUpSpace}
                        >
                          {currentUpName}
                        </button>
                      ) : (
                        <span className="text-[var(--muted)]">-</span>
                      )}
                    </td>
                    <td className="px-6 py-3 text-[var(--muted)] whitespace-nowrap">
                      {task.priorityAt ? formatDateTime(task.priorityAt) : "-"}
                    </td>
                    <td className="px-6 py-3 text-[var(--muted)] whitespace-nowrap">
                      {formatDateTime(task.createdAt)}
                    </td>
                    <td className="px-6 py-3 text-[var(--muted)] whitespace-nowrap">
                      {formatDateTime(task.updatedAt)}
                    </td>
                    <td className="sticky right-0 z-10 bg-[var(--surface)] px-6 py-3 whitespace-nowrap shadow-[-6px_0_10px_-6px_rgba(0,0,0,0.12)]">
                      <div className="flex flex-nowrap gap-2">
                        {task.workflowStatus ? (
                          <>
                            {task.workflowStatus.status === "RUNNING" ? (
                              <button
                                className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                                onClick={() => handleWorkflowPause(task.taskId)}
                              >
                                暂停
                              </button>
                            ) : null}
                            {task.workflowStatus.status === "PAUSED" ? (
                              <button
                                className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                                onClick={() => handleWorkflowResume(task.taskId)}
                              >
                                恢复
                              </button>
                            ) : null}
                            {["RUNNING", "PAUSED"].includes(task.workflowStatus.status) ? (
                              <button
                                className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                                onClick={() => handleWorkflowCancel(task.taskId)}
                              >
                                取消
                              </button>
                            ) : null}
                          </>
                        ) : null}
                        <button
                          className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                          onClick={() => handleEdit(task.taskId)}
                        >
                          编辑
                        </button>
                        <button
                          className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                          onClick={() => openUpdateModal(task)}
                        >
                          视频更新
                        </button>
                        {task.enableSegmentation ? (
                          <button
                            className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                            onClick={() => openResegmentModal(task.taskId)}
                          >
                            重新分段
                          </button>
                        ) : null}
                        <button
                          className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                          onClick={() => handleQueuePrioritize(task.taskId)}
                        >
                          {task.priority ? "刷新优先" : "优先投稿"}
                        </button>
                        {task.status === "FAILED" && task.hasIntegratedDownloads ? (
                          <button
                            className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                            onClick={() => handleIntegratedExecute(task.taskId)}
                          >
                            一键投稿
                          </button>
                        ) : null}
                        {task.status === "PENDING_SUBMIT" ? (
                          <button
                            className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                            onClick={() => handleStartSubmissionTasks([task.taskId])}
                          >
                            开始投稿
                          </button>
                        ) : null}
                        {isRemoteRejected(task) ? (
                          <button
                            className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                            onClick={() => handleIgnoreRemoteStatus([task.taskId])}
                          >
                            忽略投稿
                          </button>
                        ) : null}
                        {submissionNeedsRemoteVerification(task) ? (
                          <button
                            className="rounded-full border border-amber-200 bg-white px-2 py-1 text-xs font-semibold text-amber-700 disabled:cursor-not-allowed disabled:opacity-60"
                            onClick={() => handleTaskRemoteVerification(task)}
                            disabled={Boolean(verifyingRemoteTaskId) || refreshingRemote}
                          >
                            {verifyingRemoteTaskId === task.taskId ? "校验中" : "校验远端"}
                          </button>
                        ) : null}
                        {canConfirmRemoteMissing(task) ? (
                          <button
                            className="rounded-full border border-rose-200 bg-white px-2 py-1 text-xs font-semibold text-rose-600"
                            onClick={() => handleConfirmRemoteMissing(task)}
                          >
                            确认未生成
                          </button>
                        ) : null}
                        {canReuploadAllSegments(task) ? (
                          <button
                            className="rounded-full border border-rose-200 bg-white px-2 py-1 text-xs font-semibold text-rose-600"
                            onClick={() => handleReuploadAllSegments(task)}
                          >
                            重新上传分P
                          </button>
                        ) : null}
                        <button
                          className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)] disabled:cursor-not-allowed disabled:opacity-50"
                          onClick={() => openRepostModal(task)}
                          disabled={submissionNeedsRemoteVerification(task)}
                          title={
                            submissionNeedsRemoteVerification(task)
                              ? "请先校验远端状态，避免重复投稿"
                              : undefined
                          }
                        >
                          重新投稿
                        </button>
                        <button
                          className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)] disabled:cursor-not-allowed disabled:opacity-50"
                          onClick={() => openTranscodeRepostModal(task)}
                          disabled={submissionNeedsRemoteVerification(task)}
                          title={
                            submissionNeedsRemoteVerification(task)
                              ? "请先校验远端状态，避免重复投稿"
                              : undefined
                          }
                        >
                          转码投稿
                        </button>
                        <button
                          className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                          onClick={() => handleDetail(task.taskId)}
                        >
                          查看详情
                        </button>
                        <button
                          className="rounded-full border border-red-200 bg-white px-2 py-1 text-xs font-semibold text-red-600 hover:border-red-300"
                          onClick={() => handleDeleteTask(task.taskId)}
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
        <div className="flex flex-wrap items-center justify-between gap-3 border-t border-black/5 px-6 py-4 text-sm text-[var(--muted)]">
          <div>
            共 {totalTasks} 条，当前第 {currentPage}/{totalPages} 页，已选择 {selectedTaskCount} 条
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <select
              value={pageSize}
              onChange={(event) => {
                const nextSize = Number(event.target.value);
                if (nextSize === pageSize) {
                  return;
                }
                setPageSize(nextSize);
                loadTasks(statusFilter, currentPage, nextSize, false, taskSearch, sourceFilter, "page_size_change");
              }}
              className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
            >
              {[10, 20, 50].map((size) => (
                <option key={size} value={size}>
                  {size} 条/页
                </option>
              ))}
            </select>
            <button
              className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
              onClick={() => setCurrentPage((prev) => Math.max(1, prev - 1))}
              disabled={currentPage <= 1}
            >
              上一页
            </button>
            <button
              className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
              onClick={() => setCurrentPage((prev) => Math.min(totalPages, prev + 1))}
              disabled={currentPage >= totalPages}
            >
              下一页
            </button>
          </div>
        </div>
      </div>
          {message ? (
            <div className="mt-4 rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-700">
              {message}
            </div>
          ) : null}
          {remoteImportDialogOpen ? (
            <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/30 px-4">
              <div className="w-full max-w-lg rounded-2xl bg-white p-5 shadow-lg">
                <div className="flex items-center justify-between gap-3">
                  <div className="text-sm font-semibold text-[var(--ink)]">导入已投稿视频</div>
                  <button
                    className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                    onClick={closeRemoteImportDialog}
                    disabled={remoteImportLoading}
                  >
                    关闭
                  </button>
                </div>
                <div className="mt-3 text-xs text-[var(--muted)]">
                  输入视频链接或BVID，系统将校验该视频是否属于当前登录账号。
                </div>
                <div className="mt-3">
                  <input
                    value={remoteImportInput}
                    onChange={(event) => setRemoteImportInput(event.target.value)}
                    placeholder="例如：https://www.bilibili.com/video/BVxxxx 或 BVxxxx"
                    className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm text-[var(--ink)] focus:border-[var(--accent)] focus:outline-none"
                  />
                </div>
                <div className="mt-4 flex justify-end gap-2">
                  <button
                    className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                    onClick={closeRemoteImportDialog}
                    disabled={remoteImportLoading}
                  >
                    取消
                  </button>
                  <button
                    className="rounded-full bg-[var(--accent)] px-3 py-1 text-xs font-semibold text-white disabled:cursor-not-allowed disabled:opacity-60"
                    onClick={handleRemoteImportConfirm}
                    disabled={remoteImportLoading}
                  >
                    {remoteImportLoading ? "校验中" : "确认"}
                  </button>
                </div>
              </div>
            </div>
          ) : null}
          {remoteMissingConfirmTask ? (
            <div className="fixed inset-0 z-[70] flex items-center justify-center bg-black/35 px-4" onMouseDown={(event) => {
              if (event.target === event.currentTarget && !remoteMissingConfirming) {
                setRemoteMissingConfirmTask(null);
              }
            }}>
              <div className="w-full max-w-lg rounded-2xl bg-white p-5 shadow-lg" role="dialog" aria-modal="true" aria-labelledby="remote-missing-confirm-title">
                <div className="flex items-center justify-between gap-3">
                  <div id="remote-missing-confirm-title" className="text-sm font-semibold text-[var(--ink)]">确认远端未生成</div>
                  <button
                    className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)] disabled:cursor-not-allowed disabled:opacity-50"
                    onClick={() => setRemoteMissingConfirmTask(null)}
                    disabled={remoteMissingConfirming}
                    aria-label="关闭确认窗口"
                  >
                    关闭
                  </button>
                </div>
                <div className="mt-3 text-sm leading-6 text-[var(--muted)]">
                  请先在 B 站创作中心确认稿件确实不存在。继续后会清空本次分P上传结果，并立即重新加入投稿队列上传全部分P。
                </div>
                <div className="mt-4 flex justify-end gap-2">
                  <button
                    className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)] disabled:cursor-not-allowed disabled:opacity-50"
                    onClick={() => setRemoteMissingConfirmTask(null)}
                    disabled={remoteMissingConfirming}
                  >
                    取消
                  </button>
                  <button
                    className="rounded-full bg-rose-600 px-3 py-1 text-xs font-semibold text-white disabled:cursor-not-allowed disabled:opacity-60"
                    onClick={confirmRemoteMissing}
                    disabled={remoteMissingConfirming}
                  >
                    {remoteMissingConfirming ? "处理中" : "确认并重新投稿"}
                  </button>
                </div>
              </div>
            </div>
          ) : null}
          {reuploadConfirmTask ? (
            <div
              className="fixed inset-0 z-[70] flex items-center justify-center bg-black/35 px-4"
              onMouseDown={(event) => {
                if (event.target === event.currentTarget && !reuploadConfirming) {
                  setReuploadConfirmTask(null);
                }
              }}
            >
              <div
                className="w-full max-w-lg rounded-lg bg-white p-5 shadow-lg"
                role="dialog"
                aria-modal="true"
                aria-labelledby="reupload-all-segments-title"
              >
                <div className="flex items-center justify-between gap-3">
                  <div id="reupload-all-segments-title" className="text-sm font-semibold text-[var(--ink)]">
                    重新上传全部分P
                  </div>
                  <button
                    className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)] disabled:cursor-not-allowed disabled:opacity-50"
                    onClick={() => setReuploadConfirmTask(null)}
                    disabled={reuploadConfirming}
                    aria-label="关闭确认窗口"
                  >
                    关闭
                  </button>
                </div>
                <div className="mt-3 text-sm leading-6 text-[var(--muted)]">
                  将清除已被B站判定为重复投稿的旧CID，并重新上传全部本地分P。不会重新剪辑、合并或分段。
                </div>
                <div className="mt-4 flex justify-end gap-2">
                  <button
                    className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)] disabled:cursor-not-allowed disabled:opacity-50"
                    onClick={() => setReuploadConfirmTask(null)}
                    disabled={reuploadConfirming}
                  >
                    取消
                  </button>
                  <button
                    className="rounded-full bg-rose-600 px-3 py-1 text-xs font-semibold text-white disabled:cursor-not-allowed disabled:opacity-60"
                    onClick={confirmReuploadAllSegments}
                    disabled={reuploadConfirming}
                  >
                    {reuploadConfirming ? "处理中" : "确认重新上传"}
                  </button>
                </div>
              </div>
            </div>
          ) : null}
          {resegmentOpen ? (
            <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/30 px-4">
              <div className="w-full max-w-md rounded-2xl bg-white p-5 shadow-lg">
                <div className="flex items-center justify-between gap-3">
                  <div className="text-sm font-semibold text-[var(--ink)]">重新分段</div>
                  <button
                    className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                    onClick={closeResegmentModal}
                  >
                    关闭
                  </button>
                </div>
                {message ? (
                  <div className="mt-3 rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-700">
                    {message}
                  </div>
                ) : null}
                <div className="mt-4 space-y-3 text-sm text-[var(--ink)]">
                  <div className="space-y-2">
                    <div className="text-xs text-[var(--muted)]">重新分段模式</div>
                    <label className="flex items-center gap-2">
                      <input
                        type="radio"
                        checked={resegmentMode === "SPECIFIED"}
                        onChange={() => setResegmentMode("SPECIFIED")}
                      />
                      <span>指定合并视频</span>
                    </label>
                    <label className="flex items-center gap-2">
                      <input
                        type="radio"
                        checked={resegmentMode === "MERGE_ALL"}
                        onChange={() => setResegmentMode("MERGE_ALL")}
                      />
                      <span>合并全部合并视频</span>
                    </label>
                    {resegmentMode === "SPECIFIED" ? (
                      <div className="space-y-1">
                        <div className="text-xs text-[var(--muted)]">选择合并视频</div>
                        <select
                          value={resegmentMergedId}
                          onChange={handleResegmentMergedChange}
                          className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
                        >
                          <option value="">请选择合并视频</option>
                          {resegmentMergedVideos.map((merged) => (
                            <option key={merged.id} value={String(merged.id)}>
                              {resolveMergedVideoLabel(merged)}
                            </option>
                          ))}
                        </select>
                        {resegmentMergedVideos.length === 0 ? (
                          <div className="text-xs text-amber-700">未找到合并视频</div>
                        ) : null}
                      </div>
                    ) : null}
                  </div>
                  <div className="space-y-2">
                    <div className="text-xs text-[var(--muted)]">投稿方式</div>
                    <label className="flex items-center gap-2">
                      <input
                        type="radio"
                        checked={resegmentIntegrateCurrent}
                        onChange={() => setResegmentIntegrateCurrent(true)}
                        disabled={!resegmentHasBvid}
                      />
                      <span className={resegmentHasBvid ? "" : "text-[var(--muted)]"}>
                        集成当前BV
                      </span>
                    </label>
                    <label className="flex items-center gap-2">
                      <input
                        type="radio"
                        checked={!resegmentIntegrateCurrent}
                        onChange={() => setResegmentIntegrateCurrent(false)}
                      />
                      <span>新建BV</span>
                    </label>
                    {!resegmentHasBvid ? (
                      <div className="text-xs text-amber-700">
                        当前任务暂无BVID，只能新建BV
                      </div>
                    ) : null}
                  </div>
                  <div className="space-y-1">
                    <div className="text-xs text-[var(--muted)]">
                      当前分段时长（秒）
                    </div>
                    <div className="rounded-lg border border-black/10 bg-black/5 px-3 py-2">
                      {resegmentDefaultSeconds || "-"}
                    </div>
                  </div>
                  <div className="space-y-1">
                    <div className="text-xs text-[var(--muted)]">
                      新分段时长（秒）
                    </div>
                    <input
                      type="number"
                      min={1}
                      value={resegmentSeconds}
                      onChange={(event) => setResegmentSeconds(event.target.value)}
                      placeholder="例如 120"
                      className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
                    />
                  </div>
                  <div className="space-y-1">
                    <div className="text-xs text-[var(--muted)]">预计分段数</div>
                    <div className="rounded-lg border border-black/10 bg-black/5 px-3 py-2">
                      {resolveResegmentCount(
                        resegmentVideoSeconds,
                        resegmentSeconds,
                      ) ?? "-"}
                    </div>
                  </div>
                </div>
                <div className="mt-4 flex justify-end gap-2">
                  <button
                    className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                    onClick={closeResegmentModal}
                  >
                    取消
                  </button>
                  <LoadingButton
                    className="rounded-full bg-[var(--accent)] px-3 py-1 text-xs font-semibold text-white disabled:cursor-not-allowed disabled:opacity-60"
                    onClick={handleResegmentSubmit}
                    loading={resegmentSubmitting}
                    loadingLabel="处理中"
                    spinnerClassName="h-3 w-3"
                  >
                    开始重新分段
                  </LoadingButton>
                </div>
              </div>
            </div>
          ) : null}
          {repostOpen ? (
            <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/30 px-4">
              <div className="w-full max-w-md rounded-2xl bg-white p-5 shadow-lg">
                <div className="flex items-center justify-between gap-3">
                  <div className="text-sm font-semibold text-[var(--ink)]">重新投稿</div>
                  <button
                    className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                    onClick={closeRepostModal}
                  >
                    关闭
                  </button>
                </div>
                {message ? (
                  <div className="mt-3 rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-700">
                    {message}
                  </div>
                ) : null}
                <div className="mt-4 space-y-3 text-sm text-[var(--ink)]">
                  <div className="space-y-2">
                    <div className="text-xs text-[var(--muted)]">重新投稿模式</div>
                    <label className="flex items-center gap-2">
                      <input
                        type="radio"
                        checked={repostMode === "SPECIFIED"}
                        onChange={() => setRepostMode("SPECIFIED")}
                      />
                      <span>指定合并视频</span>
                    </label>
                    <label className="flex items-center gap-2">
                      <input
                        type="radio"
                        checked={repostMode === "MERGE_ALL"}
                        onChange={() => setRepostMode("MERGE_ALL")}
                      />
                      <span>合并全部合并视频</span>
                    </label>
                    <label className="flex items-center gap-2">
                      <input
                        type="radio"
                        checked={repostMode === "FULL_REPROCESS"}
                        onChange={() => setRepostMode("FULL_REPROCESS")}
                      />
                      <span>全部重新投稿（重新剪辑+合并+分段）</span>
                    </label>
                    {repostMode === "SPECIFIED" ? (
                      <div className="space-y-1">
                        <div className="text-xs text-[var(--muted)]">选择合并视频</div>
                        <select
                          value={repostMergedId}
                          onChange={(event) => setRepostMergedId(event.target.value)}
                          className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
                        >
                          <option value="">请选择合并视频</option>
                          {repostMergedVideos.map((merged) => (
                            <option key={merged.id} value={String(merged.id)}>
                              {resolveMergedVideoLabel(merged)}
                            </option>
                          ))}
                        </select>
                        {repostMergedVideos.length === 0 ? (
                          <div className="text-xs text-amber-700">未找到合并视频</div>
                        ) : null}
                      </div>
                    ) : null}
                  </div>
                  {repostMode === "FULL_REPROCESS" ? (
                    <div className="space-y-2">
                      <div className="text-xs text-[var(--muted)]">投稿类型</div>
                      <label className="flex items-center gap-2">
                        <input
                          type="radio"
                          checked={repostUseCurrentBvid}
                          onChange={() => setRepostUseCurrentBvid(true)}
                          disabled={!repostHasBvid}
                        />
                        <span>集成当前BV视频（编辑投稿，沿用BV号）</span>
                      </label>
                      <label className="flex items-center gap-2">
                        <input
                          type="radio"
                          checked={!repostUseCurrentBvid}
                          onChange={() => setRepostUseCurrentBvid(false)}
                        />
                        <span>重新生成投稿（创建新的BV号）</span>
                      </label>
                      {!repostHasBvid ? (
                        <div className="text-xs text-amber-700">
                          当前任务没有BV号，将创建新投稿。
                        </div>
                      ) : null}
                    </div>
                  ) : (
                    <div className="space-y-2">
                      <div className="text-xs text-[var(--muted)]">是否集成当前BV视频</div>
                      <label className="flex items-center gap-2">
                        <input
                          type="radio"
                          checked={repostUseCurrentBvid}
                          onChange={() => setRepostUseCurrentBvid(true)}
                          disabled={!repostHasBvid}
                        />
                        <span>集成当前BV视频（编辑投稿，沿用BV号）</span>
                      </label>
                      <label className="flex items-center gap-2">
                        <input
                          type="radio"
                          checked={!repostUseCurrentBvid}
                          onChange={() => setRepostUseCurrentBvid(false)}
                        />
                        <span>重新生成投稿（创建新的BV号）</span>
                      </label>
                      {!repostHasBvid ? (
                        <div className="text-xs text-amber-700">
                          当前任务没有BV号，将创建新投稿。
                        </div>
                      ) : null}
                    </div>
                  )}
                </div>
                <div className="mt-4 rounded-lg border border-black/5 bg-white/80 p-3 text-sm text-[var(--ink)]">
                  <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
                    百度网盘同步
                  </div>
                  <label className="mt-2 flex items-center gap-2 text-sm text-[var(--muted)]">
                    <input
                      type="checkbox"
                      checked={repostBaiduSync.enabled}
                      onChange={(event) =>
                        setRepostBaiduSync((prev) => ({
                          ...prev,
                          enabled: event.target.checked,
                        }))
                      }
                    />
                    同步上传到百度网盘
                  </label>
                  {repostBaiduSync.enabled ? (
                    <div className="mt-3 grid gap-2">
                      <div>
                        <div className="text-xs text-[var(--muted)]">远端路径</div>
                        <div className="mt-2 flex flex-wrap items-center gap-2 text-xs">
                          <div className="flex-1 rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-[var(--ink)]">
                            {repostBaiduSync.path || defaultBaiduSyncPath || "/录播"}
                          </div>
                          <button
                            className="rounded-full border border-black/10 bg-white px-3 py-1 font-semibold text-[var(--ink)]"
                            onClick={() => handleOpenSyncPicker("repost")}
                          >
                            选择目录
                          </button>
                        </div>
                      </div>
                      <div>
                        <div className="text-xs text-[var(--muted)]">上传文件名</div>
                        <input
                          value={repostBaiduSync.filename}
                          onChange={(event) =>
                            setRepostBaiduSync((prev) => ({
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
                <div className="mt-4 flex justify-end gap-2">
                  <button
                    className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                    onClick={closeRepostModal}
                  >
                    取消
                  </button>
                  <LoadingButton
                    className="rounded-full bg-[var(--accent)] px-3 py-1 text-xs font-semibold text-white disabled:cursor-not-allowed disabled:opacity-60"
                    onClick={handleRepostSubmit}
                    loading={repostSubmitting}
                    loadingLabel="处理中"
                    spinnerClassName="h-3 w-3"
                  >
                    开始重新投稿
                  </LoadingButton>
                </div>
              </div>
            </div>
          ) : null}
          {transcodeRepostOpen ? (
            <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/30 px-4">
              <div className="w-full max-w-md rounded-2xl bg-white p-5 shadow-lg">
                <div className="flex items-center justify-between gap-3">
                  <div className="text-sm font-semibold text-[var(--ink)]">转码投稿</div>
                  <button
                    className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                    onClick={closeTranscodeRepostModal}
                  >
                    关闭
                  </button>
                </div>
                {message ? (
                  <div className="mt-3 rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-700">
                    {message}
                  </div>
                ) : null}
                <div className="mt-4 space-y-3 text-sm text-[var(--ink)]">
                  <div className="rounded-lg border border-amber-200 bg-amber-50 px-3 py-3 text-xs leading-6 text-amber-800">
                    该操作会从当前任务的源视频重新开始处理，不复用旧的剪辑、合并、分段产物。
                    处理链路固定为“剪辑转码 + 合并转码 + 分段 copy”，用于兜底修复时间戳异常、音画不同步等问题。
                  </div>
                  <div className="space-y-2">
                    <div className="text-xs text-[var(--muted)]">投稿方式</div>
                    <label className="flex items-center gap-2">
                      <input
                        type="radio"
                        checked={transcodeRepostUseCurrentBvid}
                        onChange={() => setTranscodeRepostUseCurrentBvid(true)}
                        disabled={!transcodeRepostHasBvid}
                      />
                      <span className={transcodeRepostHasBvid ? "" : "text-[var(--muted)]"}>
                        集成当前BV视频（编辑投稿，沿用BV号）
                      </span>
                    </label>
                    <label className="flex items-center gap-2">
                      <input
                        type="radio"
                        checked={!transcodeRepostUseCurrentBvid}
                        onChange={() => setTranscodeRepostUseCurrentBvid(false)}
                      />
                      <span>重新生成投稿（创建新的BV号）</span>
                    </label>
                    {!transcodeRepostHasBvid ? (
                      <div className="text-xs text-amber-700">
                        当前任务没有BV号，将创建新投稿。
                      </div>
	                    ) : null}
	                  </div>
	                  <div className="rounded-lg border border-black/5 bg-white/80 p-3">
	                    <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
	                      百度网盘同步
	                    </div>
	                    <label className="mt-2 flex items-center gap-2 text-sm text-[var(--muted)]">
	                      <input
	                        type="checkbox"
	                        checked={transcodeRepostBaiduSync.enabled}
	                        onChange={(event) =>
	                          setTranscodeRepostBaiduSync((prev) => ({
	                            ...prev,
	                            enabled: event.target.checked,
	                          }))
	                        }
	                      />
	                      同步上传到百度网盘
	                    </label>
	                    {transcodeRepostBaiduSync.enabled ? (
	                      <div className="mt-3 grid gap-2">
	                        <div>
	                          <div className="text-xs text-[var(--muted)]">远端路径</div>
	                          <div className="mt-2 flex flex-wrap items-center gap-2 text-xs">
	                            <div className="flex-1 rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-[var(--ink)]">
	                              {transcodeRepostBaiduSync.path || defaultBaiduSyncPath || "/录播"}
	                            </div>
	                            <button
	                              className="rounded-full border border-black/10 bg-white px-3 py-1 font-semibold text-[var(--ink)]"
	                              onClick={() => handleOpenSyncPicker("transcodeRepost")}
	                            >
	                              选择目录
	                            </button>
	                          </div>
	                        </div>
	                        <div>
	                          <div className="text-xs text-[var(--muted)]">上传文件名</div>
	                          <input
	                            value={transcodeRepostBaiduSync.filename}
	                            onChange={(event) =>
	                              setTranscodeRepostBaiduSync((prev) => ({
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
	                </div>
                <div className="mt-4 flex justify-end gap-2">
                  <button
                    className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                    onClick={closeTranscodeRepostModal}
                  >
                    取消
                  </button>
                  <LoadingButton
                    className="rounded-full bg-[var(--accent)] px-3 py-1 text-xs font-semibold text-white disabled:cursor-not-allowed disabled:opacity-60"
                    onClick={handleTranscodeRepostSubmit}
                    loading={transcodeRepostSubmitting}
                    loadingLabel="处理中"
                    spinnerClassName="h-3 w-3"
                  >
                    开始转码投稿
                  </LoadingButton>
                </div>
              </div>
            </div>
          ) : null}
          {deleteConfirmOpen ? (
            <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/30 px-4">
              <div className="w-full max-w-lg rounded-2xl bg-white p-5 shadow-lg">
              <div className="text-sm font-semibold text-[var(--ink)]">
                {isBatchDelete ? `批量删除投稿任务（${deleteTargetCount} 个）` : "删除投稿任务"}
              </div>
              <div className="mt-2 text-xs text-[var(--muted)]">
                请选择删除范围，删除后不可恢复。
              </div>
              {deleteMessage ? (
                <div className={`mt-3 rounded-lg border px-3 py-2 text-xs ${deleteMessageClass}`}>
                  {deleteMessage}
                </div>
              ) : null}
              <div className="mt-4 space-y-3">
                  <label className="flex items-start gap-2 text-sm">
                    <input
                      type="checkbox"
                      className="mt-0.5"
                      checked={deleteTaskChecked}
                      onChange={(event) => setDeleteTaskChecked(event.target.checked)}
                    />
                    <div>
	                      <div className="font-semibold text-[var(--ink)]">删除投稿任务</div>
	                      <div className="text-xs text-[var(--muted)]">
	                        同步删除任务记录与任务目录。
	                      </div>
                    </div>
                  </label>
                  <label className="flex items-start gap-2 text-sm">
                    <input
                      type="checkbox"
                      className="mt-0.5"
                      checked={deleteFilesChecked}
                      onChange={(event) => handleDeleteFilesToggle(event.target.checked)}
                    />
                    <div>
	                      <div className="font-semibold text-[var(--ink)]">删除视频文件</div>
	                      <div className="text-xs text-[var(--muted)]">
	                        包含源视频与任务目录，默认全选。
	                      </div>
                    </div>
                  </label>
                  {deleteFilesChecked ? (
                    <div className="rounded-xl border border-black/10 bg-white/80 p-3">
                      <div className="text-xs font-semibold text-[var(--ink)]">
                        可删除文件
                      </div>
                      {deletePreviewLoading ? (
                        <div className="mt-2 text-xs text-[var(--muted)]">
                          正在读取文件列表...
                        </div>
                      ) : null}
                      {!deletePreviewLoading && !deleteHasFiles ? (
                        <div className="mt-2 text-xs text-[var(--muted)]">
                          未发现可删除文件。
                        </div>
                      ) : null}
                      {!deletePreviewLoading && deleteHasFiles ? (
                        <div className="mt-3 max-h-52 space-y-2 overflow-auto">
                          {deleteFiles.map((item) => {
                            const checked = deleteFileSelections.has(item.path);
                            const isVideo = isVideoFilePath(item.path);
                            const conflicts = item.conflicts || [];
                            return (
                              <label
                                key={item.path}
                                className="flex items-start gap-2 text-xs"
                              >
                                <input
                                  type="checkbox"
                                  className="mt-0.5"
                                  checked={checked}
                                  onChange={() => toggleDeleteFileSelection(item.path)}
                                />
                                <div className="min-w-0">
                                  <div className="truncate text-[var(--ink)]">
                                    {item.path}
                                  </div>
                                  <div className="mt-0.5 text-[10px] text-[var(--muted)]">
                                    {isVideo ? "源视频" : "任务目录"}
                                  </div>
                                  {conflicts.length > 0 ? (
                                    <div className="mt-1 space-y-1 text-[10px] text-red-500">
                                      {conflicts.map((conflict) => (
                                        <div key={conflict.taskId}>
                                          被任务《{conflict.title}》({conflict.status}) 使用中
                                        </div>
                                      ))}
                                    </div>
                                  ) : null}
                                </div>
                              </label>
                            );
                          })}
                        </div>
                      ) : null}
                    </div>
                  ) : null}
                </div>
                <div className="mt-4 flex justify-end gap-2">
                  <button
                    className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                    type="button"
                    onClick={handleDeleteCancel}
                    disabled={deleteSubmitting}
                  >
                    取消
                  </button>
                  <button
                    className="rounded-full border border-red-200 bg-red-500 px-3 py-1 text-xs font-semibold text-white disabled:cursor-not-allowed disabled:opacity-60"
                    type="button"
                    onClick={handleDeleteConfirm}
                    disabled={deleteSubmitting}
                  >
                    {deleteSubmitting ? "处理中" : "确认删除"}
                  </button>
                </div>
              </div>
            </div>
          ) : null}
          {deleteConflictOpen ? (
            <div className="fixed inset-0 z-[60] flex items-center justify-center bg-black/40 px-4">
              <div className="w-full max-w-md rounded-2xl bg-white p-5 shadow-lg">
                <div className="text-sm font-semibold text-[var(--ink)]">
                  文件正被其他任务使用
                </div>
                <div className="mt-2 text-xs text-[var(--muted)]">
                  仍然删除将影响其他投稿任务，确认继续吗？
                </div>
                {deleteMessage ? (
                  <div className={`mt-3 rounded-lg border px-3 py-2 text-xs ${deleteMessageClass}`}>
                    {deleteMessage}
                  </div>
                ) : null}
                <div className="mt-3 max-h-48 space-y-2 overflow-auto">
                  {deleteConflictFiles.map((item) => (
                    <div
                      key={item.path}
                      className="rounded-lg border border-red-100 bg-red-50/60 p-2 text-xs"
                    >
                      <div className="truncate text-[var(--ink)]">{item.path}</div>
                      {item.conflicts && item.conflicts.length > 0 ? (
                        <div className="mt-1 space-y-1 text-[10px] text-red-600">
                          {item.conflicts.map((conflict) => (
                            <div key={conflict.taskId}>
                              任务：{conflict.title}（{conflict.status}）
                            </div>
                          ))}
                        </div>
                      ) : null}
                    </div>
                  ))}
                </div>
                <div className="mt-4 flex justify-end gap-2">
                  <button
                    className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                    type="button"
                    onClick={handleDeleteConflictCancel}
                    disabled={deleteSubmitting}
                  >
                    取消
                  </button>
                  <button
                    className="rounded-full border border-red-200 bg-red-500 px-3 py-1 text-xs font-semibold text-white disabled:cursor-not-allowed disabled:opacity-60"
                    type="button"
                    onClick={handleDeleteConflictConfirm}
                    disabled={deleteSubmitting}
                  >
                    {deleteSubmitting ? "处理中" : "仍然删除"}
                  </button>
                </div>
              </div>
            </div>
          ) : null}
          {deleteMergedOpen ? (
            <div className="fixed inset-0 z-[65] flex items-center justify-center bg-black/40 px-4">
              <div className="w-full max-w-lg rounded-2xl bg-white p-5 shadow-lg">
                <div className="text-sm font-semibold text-[var(--ink)]">删除合并视频</div>
                <div className="mt-2 text-xs text-[var(--muted)]">
                  删除后不可恢复，请确认是否继续。
                </div>
                <div className="mt-3 rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-xs text-[var(--ink)]">
                  <div className="truncate">文件名：{deleteMergedTarget?.fileName || "-"}</div>
                  <div className="mt-1 truncate text-[var(--muted)]">
                    路径：{deleteMergedTarget?.videoPath || "-"}
                  </div>
                </div>
                <label className="mt-4 flex items-start gap-2 text-sm">
                  <input
                    type="checkbox"
                    className="mt-0.5"
                    checked={deleteMergedLocalFile}
                    onChange={(event) => setDeleteMergedLocalFile(event.target.checked)}
                    disabled={deleteMergedSubmitting}
                  />
                  <div>
                    <div className="font-semibold text-[var(--ink)]">同时删除本地文件</div>
                    <div className="text-xs text-[var(--muted)]">
                      不勾选时将保留本地文件，仅移除合并视频记录。
                    </div>
                  </div>
                </label>
                {deleteMergedMessage ? (
                  <div className="mt-3 rounded-lg border border-red-200 bg-red-50 px-3 py-2 text-xs text-red-700">
                    {deleteMergedMessage}
                  </div>
                ) : null}
                <div className="mt-4 flex justify-end gap-2">
                  <button
                    className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                    onClick={() => closeDeleteMergedModal(false)}
                    disabled={deleteMergedSubmitting}
                  >
                    取消
                  </button>
                  <button
                    className="rounded-full border border-red-200 bg-red-500 px-3 py-1 text-xs font-semibold text-white disabled:cursor-not-allowed disabled:opacity-60"
                    onClick={handleConfirmDeleteMerged}
                    disabled={deleteMergedSubmitting}
                  >
                    {deleteMergedSubmitting ? "处理中" : "确认删除"}
                  </button>
                </div>
              </div>
            </div>
          ) : null}
        </>
  );
}
