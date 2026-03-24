import { useEffect, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import {
  confirm as dialogConfirm,
  message as dialogMessage,
  open as openDialog,
  save as saveDialog,
} from "@tauri-apps/plugin-dialog";
import LoadingButton from "../components/LoadingButton";
import { showErrorDialog } from "../lib/dialog";
import { invokeCommand } from "../lib/tauri";
import { formatDateTime } from "../lib/format";
import BaiduSyncPathPicker from "../components/BaiduSyncPathPicker";
import BaiduRemoteFilePicker from "../components/BaiduRemoteFilePicker";
import CoverCropModal from "../components/CoverCropModal";
import RemoteImportTaskView from "../components/RemoteImportTaskView";
import useSubmissionCover from "../hooks/useSubmissionCover";
import {
  buildEditBaselineFromDetail,
  collectCurrentTags,
  resolveEditChangedModules,
  validateSourceVideos,
} from "../lib/submissionEditDiff";

const statusFilters = [
  { value: "ALL", label: "全部" },
  { value: "PENDING", label: "待处理" },
  { value: "CLIPPING", label: "剪辑中" },
  { value: "MERGING", label: "合并中" },
  { value: "SEGMENTING", label: "分段中" },
  { value: "RUNNING", label: "处理中" },
  { value: "WAITING_UPLOAD", label: "投稿队列中" },
  { value: "UPLOADING", label: "投稿中" },
  { value: "COMPLETED", label: "已完成" },
  { value: "FAILED", label: "失败" },
  { value: "CANCELLED", label: "已取消" },
];

const sourceFilters = [
  { value: "NORMAL", label: "其他投稿" },
  { value: "CLIP", label: "切片投稿" },
];

const buildSourceId = () =>
  `source_${Math.random().toString(36).slice(2, 10)}_${Date.now().toString(36)}`;
const buildMergeGroupId = () =>
  `group_${Math.random().toString(36).slice(2, 10)}_${Date.now().toString(36)}`;

const emptySource = (index) => ({
  localId: buildSourceId(),
  sourceFilePath: "",
  sortOrder: index + 1,
  startTime: "00:00:00",
  endTime: "00:00:00",
  durationSeconds: 0,
});

const defaultWorkflowConfig = {
  segmentationConfig: {
    segmentDurationSeconds: 133,
    preserveOriginal: true,
  },
};

const buildMergeItemsFromSources = (sources) =>
  (sources || [])
    .map((item) => item?.localId)
    .filter((id) => id)
    .map((id) => ({ id, type: "SOURCE", sourceId: id, standalone: false }));

export default function SubmissionSection({
  activeBilibiliUid = "",
  onAuthChange,
  onRefreshBaiduStatus,
}) {
  const [taskForm, setTaskForm] = useState({
    title: "",
    description: "",
    coverUrl: "",
    coverLocalPath: "",
    coverDataUrl: "",
    partitionId: "",
    collectionId: "",
    activityTopicId: "",
    activityMissionId: "",
    activityTitle: "",
    videoType: "ORIGINAL",
    segmentPrefix: "",
    priority: false,
    baiduSyncEnabled: false,
    baiduSyncPath: "",
    baiduSyncFilename: "",
  });
  const [tagInput, setTagInput] = useState("");
  const [tags, setTags] = useState([]);
  const [segmentationEnabled, setSegmentationEnabled] = useState(true);
  const initialSourcesRef = useRef(null);
  if (!initialSourcesRef.current) {
    initialSourcesRef.current = [emptySource(0)];
  }
  const [sourceVideos, setSourceVideos] = useState(initialSourcesRef.current);
  const [mergeItems, setMergeItems] = useState(() =>
    buildMergeItemsFromSources(initialSourcesRef.current),
  );
  const [mergeSelection, setMergeSelection] = useState(() => new Set());
  const [draggingMergeItemId, setDraggingMergeItemId] = useState("");
  const [draggingMergeGroupSource, setDraggingMergeGroupSource] = useState({
    groupId: "",
    sourceId: "",
  });
  const [workflowConfig, setWorkflowConfig] = useState(defaultWorkflowConfig);
  const [partitions, setPartitions] = useState([]);
  const [collections, setCollections] = useState([]);
  const [activityOptions, setActivityOptions] = useState([]);
  const [activityLoading, setActivityLoading] = useState(false);
  const [activityMessage, setActivityMessage] = useState("");
  const [activityKeyword, setActivityKeyword] = useState("");
  const [activityDropdownOpen, setActivityDropdownOpen] = useState(false);
  const [tasks, setTasks] = useState([]);
  const [currentUpProfile, setCurrentUpProfile] = useState({ uid: 0, name: "" });
  const [bilibiliAccounts, setBilibiliAccounts] = useState([]);
  const [selectedBilibiliUid, setSelectedBilibiliUid] = useState("");
  const [selectedTaskIds, setSelectedTaskIds] = useState(() => new Set());
  const [totalTasks, setTotalTasks] = useState(0);
  const [taskSearch, setTaskSearch] = useState("");
  const [selectedTask, setSelectedTask] = useState(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [detailTab, setDetailTab] = useState("basic");
  const [currentPage, setCurrentPage] = useState(1);
  const [pageSize, setPageSize] = useState(20);
  const [statusFilter, setStatusFilter] = useState("ALL");
  const [sourceFilter, setSourceFilter] = useState("NORMAL");
  const [message, setMessage] = useState("");
  const [refreshingRemote, setRefreshingRemote] = useState(false);
  const [submissionView, setSubmissionView] = useState("list");
  const [remoteImportDialogOpen, setRemoteImportDialogOpen] = useState(false);
  const [remoteImportInput, setRemoteImportInput] = useState("");
  const [remoteImportLoading, setRemoteImportLoading] = useState(false);
  const [remoteImportPreview, setRemoteImportPreview] = useState(null);
  const [deleteTargetId, setDeleteTargetId] = useState("");
  const [deleteConfirmOpen, setDeleteConfirmOpen] = useState(false);
  const [deletePreview, setDeletePreview] = useState(null);
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
  const [quickFillOpen, setQuickFillOpen] = useState(false);
  const [quickFillTasks, setQuickFillTasks] = useState([]);
  const [quickFillPage, setQuickFillPage] = useState(1);
  const [quickFillTotal, setQuickFillTotal] = useState(0);
  const [quickFillSearch, setQuickFillSearch] = useState("");
  const [updateOpen, setUpdateOpen] = useState(false);
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
  const [createSubmitting, setCreateSubmitting] = useState(false);
  const [exportingTasks, setExportingTasks] = useState(false);
  const [importingTasks, setImportingTasks] = useState(false);
  const normalizedActiveBilibiliUid = String(activeBilibiliUid || "").trim();

  const [repostMode, setRepostMode] = useState("SPECIFIED");
  const [repostMergedVideos, setRepostMergedVideos] = useState([]);
  const [repostMergedId, setRepostMergedId] = useState("");
  const [repostBaiduSync, setRepostBaiduSync] = useState({
    enabled: false,
    path: "",
    filename: "",
  });

  const [defaultBaiduSyncPath, setDefaultBaiduSyncPath] = useState("/录播");
  const [updateTaskId, setUpdateTaskId] = useState("");
  const [updateSourceVideos, setUpdateSourceVideos] = useState([emptySource(0)]);
  const [updateSegmentationEnabled, setUpdateSegmentationEnabled] = useState(true);
  const [updateWorkflowConfig, setUpdateWorkflowConfig] = useState(defaultWorkflowConfig);
  const [updateSegmentPrefix, setUpdateSegmentPrefix] = useState("");
  const [updateBaiduSync, setUpdateBaiduSync] = useState({
    enabled: false,
    path: "",
    filename: "",
  });
  const [syncPickerOpen, setSyncPickerOpen] = useState(false);
  const [syncTarget, setSyncTarget] = useState("");
  const [remoteFilePickerOpen, setRemoteFilePickerOpen] = useState(false);
  const [remoteFilePickerPath, setRemoteFilePickerPath] = useState("/");
  const [bindingMergedVideo, setBindingMergedVideo] = useState(null);
  const [bindingRemoteFile, setBindingRemoteFile] = useState(false);
  const [deleteMergedOpen, setDeleteMergedOpen] = useState(false);
  const [deleteMergedTarget, setDeleteMergedTarget] = useState(null);
  const [deleteMergedLocalFile, setDeleteMergedLocalFile] = useState(false);
  const [deleteMergedSubmitting, setDeleteMergedSubmitting] = useState(false);
  const [deleteMergedMessage, setDeleteMergedMessage] = useState("");
  const [updateSubmitting, setUpdateSubmitting] = useState(false);
  const [retryingSegmentIds, setRetryingSegmentIds] = useState(() => new Set());
  const [segmentBindingSelection, setSegmentBindingSelection] = useState(() => new Set());
  const [segmentBindingMergedId, setSegmentBindingMergedId] = useState("");
  const [segmentBindingSubmitting, setSegmentBindingSubmitting] = useState(false);
  const [sourceReorderSubmitting, setSourceReorderSubmitting] = useState(false);
  const [mergedReorderSubmitting, setMergedReorderSubmitting] = useState(false);
  const [mergedOrderDraftIds, setMergedOrderDraftIds] = useState([]);
  const [mergedOrderDirty, setMergedOrderDirty] = useState(false);
  const [draggingMergedId, setDraggingMergedId] = useState("");
  const [mergedBindDialogOpen, setMergedBindDialogOpen] = useState(false);
  const [mergedBindTarget, setMergedBindTarget] = useState(null);
  const [mergedBindSelection, setMergedBindSelection] = useState(() => new Set());
  const [mergedBindSubmitting, setMergedBindSubmitting] = useState(false);
  const [mergedBindMessage, setMergedBindMessage] = useState("");
  const [editSegments, setEditSegments] = useState([]);
  const editSegmentsRef = useRef([]);
  const [pendingEditUploads, setPendingEditUploads] = useState(0);
  const [editingSegmentId, setEditingSegmentId] = useState("");
  const [editingSegmentName, setEditingSegmentName] = useState("");
  const [draggingSegmentId, setDraggingSegmentId] = useState("");
  const [editSubmitConfirmOpen, setEditSubmitConfirmOpen] = useState(false);
  const [editSubmitConfirmSyncRemote, setEditSubmitConfirmSyncRemote] = useState(true);
  const [pendingEditSubmitPayload, setPendingEditSubmitPayload] = useState(null);
  const [pendingEditChangedLabel, setPendingEditChangedLabel] = useState("");
  const [pendingEditNeedRemote, setPendingEditNeedRemote] = useState(false);
  const [coverPreviewModalOpen, setCoverPreviewModalOpen] = useState(false);
  const [coverPreviewModalSrc, setCoverPreviewModalSrc] = useState("");
  const [localCoverDataPreviewSrc, setLocalCoverDataPreviewSrc] = useState("");
  const [coverProxyPreviewSrc, setCoverProxyPreviewSrc] = useState("");
  const [submittingEdit, setSubmittingEdit] = useState(false);
  const [editBaseline, setEditBaseline] = useState(null);
  const lastDetailTaskIdRef = useRef(null);
  const lastEditTaskIdRef = useRef(null);
  const dragStateRef = useRef({ activeId: "", overId: "" });
  const mergedDragStateRef = useRef({ activeId: "", overId: "" });
  const mergeDragStateRef = useRef({ activeId: "", overId: "" });
  const mergeGroupDragStateRef = useRef({ groupId: "", activeId: "", overId: "" });
  const coverProxyCacheRef = useRef(new Map());
  const activityRequestSeqRef = useRef(0);
  const isCreateView = submissionView === "create";
  const isDetailView = submissionView === "detail";
  const isEditView = submissionView === "edit";
  const isRemoteImportView = submissionView === "remoteImport";
  const canModifySelectedTask = isEditView;
  const isReadOnly = isDetailView;
  const selectedTaskHasBvid = Boolean(String(selectedTask?.task?.bvid || "").trim());
  const canSyncRemoteEdit = selectedTaskHasBvid;
  const groupedSourceIds = buildGroupedSourceIdSet(mergeItems);
  const quickFillPageSize = 10;
  const deleteFiles = deletePreview?.files || [];
  const deleteHasFiles = deleteFiles.length > 0;
  const buildPartitionOptionValue = (partition) => {
    const tid = String(partition?.tid ?? "").trim();
    if (!tid) {
      return "";
    }
    return tid;
  };
  const parsePartitionOptionValue = (value) => {
    return String(value || "").trim();
  };
  const resolvePartitionSelectValue = (partitionId, options = partitions) => {
    const normalizedId = String(partitionId || "").trim();
    if (!normalizedId) {
      return "";
    }
    return options.some((item) => String(item.tid) === normalizedId) ? normalizedId : normalizedId;
  };
  const handlePartitionChange = (rawValue) => {
    const partitionId = parsePartitionOptionValue(rawValue);
    setTaskForm((prev) => ({
      ...prev,
      partitionId,
    }));
  };
  const activitySelectOptions = (() => {
    const ordered = [...activityOptions];
    const currentId = Number(taskForm.activityTopicId || 0);
    if (!currentId) {
      return ordered;
    }
    const exists = ordered.some((item) => item.topicId === currentId);
    if (exists || !taskForm.activityTitle) {
      return ordered;
    }
    return [
      {
        topicId: currentId,
        missionId: Number(taskForm.activityMissionId || 0),
        name: taskForm.activityTitle,
        description: "",
        activityText: "",
        activityDescription: "",
        readCount: 0,
        showActivityIcon: false,
      },
      ...ordered,
    ];
  })();
  const activityFilteredOptions = (() => {
    const keyword = String(activityKeyword || "").trim().toLowerCase();
    if (!keyword) {
      return activitySelectOptions;
    }
    const filtered = activitySelectOptions.filter((activity) => {
      const text = [
        activity?.name,
        activity?.activityText,
        activity?.description,
        activity?.activityDescription,
      ]
        .map((item) => String(item || "").toLowerCase())
        .join(" ");
      return text.includes(keyword);
    });
    const currentId = Number(taskForm.activityTopicId || 0);
    if (!currentId || filtered.some((item) => item.topicId === currentId)) {
      return filtered;
    }
    const selected = activitySelectOptions.find((item) => item.topicId === currentId);
    return selected ? [selected, ...filtered] : filtered;
  })();
  const {
    coverAspectRatio,
    coverMinWidth,
    coverMinHeight,
    coverCropOpen,
    coverCropSourceUrl,
    coverUploading,
    coverPreviewUrl,
    resetCoverState,
    handleSelectCoverFile,
    handleCloseCoverCrop,
    handleCoverCropImageError,
    handleConfirmCoverCrop,
    handleClearCover,
  } = useSubmissionCover({
    openDialog,
    convertFileSrc,
    invokeCommand,
    setTaskForm,
    setMessage,
  });

  useEffect(() => {
    editSegmentsRef.current = editSegments;
  }, [editSegments]);

  useEffect(() => {
    const nextCount = editSegments.filter(
      (segment) =>
        segment.uploadStatus === "UPLOADING" ||
        segment.uploadStatus === "RATE_LIMITED",
    ).length;
    setPendingEditUploads((prev) => (prev === nextCount ? prev : nextCount));
  }, [editSegments]);

  const formatDurationHms = (seconds) => {
    const totalSeconds = Math.max(0, Math.floor(seconds || 0));
    const hrs = Math.floor(totalSeconds / 3600);
    const mins = Math.floor((totalSeconds % 3600) / 60);
    const secs = totalSeconds % 60;
    return `${String(hrs).padStart(2, "0")}:${String(mins).padStart(2, "0")}:${String(secs).padStart(2, "0")}`;
  };

  const parseHmsToSeconds = (value) => {
    if (!value) {
      return null;
    }
    const parts = value.split(":").map((part) => Number(part));
    if (parts.length !== 3 || parts.some((item) => Number.isNaN(item))) {
      return null;
    }
    return parts[0] * 3600 + parts[1] * 60 + parts[2];
  };

  const clampTimeSeconds = (seconds, maxSeconds) => {
    const clamped = Math.max(0, seconds);
    if (Number.isFinite(maxSeconds) && maxSeconds > 0) {
      return Math.min(clamped, maxSeconds);
    }
    return clamped;
  };

  const normalizeTimeValue = (value, maxSeconds) => {
    const parsed = parseHmsToSeconds(value);
    if (parsed === null) {
      return null;
    }
    const clamped = clampTimeSeconds(parsed, maxSeconds);
    return formatDurationHms(clamped);
  };

  const isVideoFilePath = (path) => {
    if (!path) {
      return false;
    }
    return /\.(mp4|mkv|mov|flv|avi|webm)$/i.test(path);
  };

  const resetDeleteState = () => {
    setDeleteTargetId("");
    setDeleteConfirmOpen(false);
    setDeletePreview(null);
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
      activityTopicId: "",
      activityMissionId: "",
      activityTitle: "",
      videoType: "ORIGINAL",
      segmentPrefix: "",
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

  const resetUpdateState = () => {
    setUpdateTaskId("");
    setUpdateSourceVideos([emptySource(0)]);
    setUpdateSegmentationEnabled(true);
    setUpdateWorkflowConfig(defaultWorkflowConfig);
    setUpdateSegmentPrefix("");
    setUpdateBaiduSync({ enabled: false, path: "", filename: "" });
    setUpdateSubmitting(false);
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
    await loadPartitions();
    await loadCollections();
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

  const handleRemoteImportSaved = async (result) => {
    setRemoteImportPreview(null);
    setSubmissionView("list");
    setMessage(`远程导入成功，任务ID：${result?.taskId || "-"}`);
    await loadTasks(statusFilter, currentPage, pageSize);
  };

  const openUpdateModal = (task) => {
    setMessage("");
    setUpdateOpen(true);
    setUpdateTaskId(task?.taskId || "");
    setUpdateSegmentPrefix(task?.segmentPrefix || "");
    setUpdateBaiduSync({
      enabled: Boolean(task?.baiduSyncEnabled),
      path: task?.baiduSyncPath || "",
      filename: task?.baiduSyncFilename || "",
    });
    setUpdateSourceVideos([emptySource(0)]);
    setUpdateSegmentationEnabled(true);
    setUpdateWorkflowConfig(defaultWorkflowConfig);
    setUpdateSubmitting(false);
  };

  const closeUpdateModal = () => {
    setUpdateOpen(false);
    resetUpdateState();
    setMessage("");
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

  const clearEditUploadCache = async (taskId) => {
    const targetId = String(taskId || "").trim();
    if (!targetId) {
      return;
    }
    try {
      await invokeCommand("submission_edit_upload_clear", {
        request: { taskId: targetId },
      });
    } catch (error) {
      setMessage(error.message);
    }
  };

  const backToList = () => {
    if (submissionView === "edit") {
      const taskId = selectedTask?.task?.taskId || lastEditTaskIdRef.current;
      if (taskId) {
        clearEditUploadCache(taskId);
      }
    }
    setSubmissionView("list");
    setMessage("");
    setSelectedTask(null);
    setEditSegments([]);
    setEditingSegmentId("");
    setEditingSegmentName("");
    setDraggingSegmentId("");
    setEditSubmitConfirmOpen(false);
    setEditSubmitConfirmSyncRemote(true);
    setPendingEditSubmitPayload(null);
    setPendingEditChangedLabel("");
    setPendingEditNeedRemote(false);
    setSubmittingEdit(false);
    setEditBaseline(null);
    resetCoverState();
    setQuickFillOpen(false);
    setActivityKeyword("");
    setActivityDropdownOpen(false);
    setRemoteImportDialogOpen(false);
    setRemoteImportInput("");
    setRemoteImportLoading(false);
    setRemoteImportPreview(null);
    resetSegmentBindingState();
  };

  const loadPartitions = async () => {
    try {
      const currentBilibiliUid = Number(selectedBilibiliUid || currentUpProfile?.uid || 0);
      const data = await invokeCommand("bilibili_partitions", {
        bilibiliUid: Number.isFinite(currentBilibiliUid) && currentBilibiliUid > 0
          ? currentBilibiliUid
          : null,
      });
      setPartitions(data || []);
      if ((data || []).length) {
        setTaskForm((prev) => {
          if (prev.partitionId) {
            return prev;
          }
          return {
            ...prev,
            partitionId: String(data[0].tid),
          };
        });
      }
    } catch (error) {
      setMessage(error.message);
    }
  };

  const loadCollections = async () => {
    try {
      await invokeCommand("auth_client_log", {
        message: "collections_load_start",
      });
      const auth = await invokeCommand("auth_status");
      await invokeCommand("auth_client_log", {
        message: `collections_auth_status loggedIn=${auth?.loggedIn ? "1" : "0"}`,
      });
      if (!auth?.loggedIn) {
        setCollections([]);
        setCurrentUpProfile({ uid: 0, name: "" });
        setBilibiliAccounts(Array.isArray(auth?.accounts) ? auth.accounts : []);
        setSelectedBilibiliUid("");
        return;
      }
      const profile = extractCurrentAuthProfile(auth);
      setCurrentUpProfile(profile);
      setBilibiliAccounts(Array.isArray(auth?.accounts) ? auth.accounts : []);
      const effectiveUid = String(selectedBilibiliUid || auth?.activeAccount?.userId || profile.uid || "");
      setSelectedBilibiliUid((prev) => prev || effectiveUid);
      const mid = Number(effectiveUid || profile.uid || 0);
      await invokeCommand("auth_client_log", {
        message: `collections_mid=${mid || 0}`,
      });
      const data = await invokeCommand("bilibili_collections", {
        mid: mid || 0,
        bilibiliUid: mid || null,
      });
      const mapped = (data || []).map((item) => ({
        ...item,
        seasonId: item.season_id ?? item.seasonId,
      }));
      setCollections(mapped);
      await invokeCommand("auth_client_log", {
        message: `collections_load_ok count=${mapped.length}`,
      });
    } catch (error) {
      try {
        await invokeCommand("auth_client_log", {
          message: `collections_load_fail err=${error?.message || String(error || "")}`,
        });
      } catch (_) {
      }
      setMessage(error.message);
    }
  };

  const extractCurrentAuthProfile = (auth) => {
    if (!auth?.loggedIn) {
      return { uid: 0, name: "" };
    }
    const userInfo = auth?.userInfo || {};
    const level1 = userInfo?.data || userInfo;
    const level2 = level1?.data || level1;
    const uid = Number(
      level2?.mid ||
        level1?.mid ||
        userInfo?.mid ||
        level2?.user_id ||
        level1?.user_id ||
        userInfo?.user_id ||
        0,
    );
    const name = String(
      level2?.name ||
        level1?.name ||
        userInfo?.name ||
        level2?.uname ||
        level1?.uname ||
        userInfo?.uname ||
        level2?.username ||
        level1?.username ||
        userInfo?.username ||
        level2?.nickname ||
        level1?.nickname ||
        userInfo?.nickname ||
        "",
    ).trim();
    return {
      uid: Number.isFinite(uid) ? uid : 0,
      name,
    };
  };

  const loadCurrentUpProfile = async () => {
    try {
      const auth = await invokeCommand("auth_status");
      const profile = extractCurrentAuthProfile(auth);
      setCurrentUpProfile(profile);
      setBilibiliAccounts(Array.isArray(auth?.accounts) ? auth.accounts : []);
      setSelectedBilibiliUid((prev) => {
        const nextUid = String(auth?.activeAccount?.userId || profile.uid || "");
        return prev || nextUid;
      });
    } catch (_) {}
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
      await loadCollections();
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

  const loadBaiduSyncSettings = async () => {
    try {
      const data = await invokeCommand("baidu_sync_settings");
      setDefaultBaiduSyncPath(data?.targetPath || "/录播");
    } catch (_) {}
  };

  const normalizeActivityOptions = (items) => {
    const parseReadCount = (value) => {
      const raw = String(value ?? "").trim();
      if (!raw) {
        return 0;
      }
      const numeric = Number(raw);
      if (Number.isFinite(numeric)) {
        return Math.max(0, Math.floor(numeric));
      }
      const digits = raw.replace(/[^\d]/g, "");
      if (!digits) {
        return 0;
      }
      const parsed = Number(digits);
      return Number.isFinite(parsed) ? Math.max(0, Math.floor(parsed)) : 0;
    };
    return (items || [])
      .map((item) => ({
        topicId: Number(item?.topicId ?? item?.topic_id ?? 0),
        missionId: Number(item?.missionId ?? item?.mission_id ?? 0),
        name: item?.name || item?.topicName || item?.topic_name || "",
        description: item?.description || item?.topicDescription || item?.topic_description || "",
        activityText: item?.activityText || item?.activity_text || "",
        activityDescription: item?.activityDescription || item?.activity_description || "",
        showActivityIcon: Boolean(
          item?.showActivityIcon ?? item?.show_activity_icon ?? false,
        ),
        readCount: parseReadCount(
          item?.readCount ??
            item?.read_count ??
            item?.arcPlayVv ??
            item?.arc_play_vv ??
            item?.read ??
            item?.viewCount ??
            item?.view_count ??
            item?.view ??
            item?.pv ??
            item?.click ??
            item?.hot,
        ),
      }))
      .filter((item) => item.topicId > 0 && item.name);
  };

  const applyActivitySelection = (activity) => {
    const previousTitle = taskForm.activityTitle || "";
    const nextTitle = activity?.name || "";
    setTaskForm((prev) => ({
      ...prev,
      activityTopicId: activity ? String(activity.topicId) : "",
      activityMissionId: activity ? String(activity.missionId || "") : "",
      activityTitle: nextTitle,
    }));
    setTags((prev) => {
      const previousIndex = previousTitle ? prev.indexOf(previousTitle) : -1;
      let next = prev.filter((tag) => tag !== previousTitle);
      if (!nextTitle) {
        return next;
      }
      const existingIndex = next.indexOf(nextTitle);
      if (existingIndex >= 0) {
        if (previousIndex >= 0) {
          const [tagValue] = next.splice(existingIndex, 1);
          const insertAt = Math.min(previousIndex, next.length);
          next.splice(insertAt, 0, tagValue);
        }
        return next;
      }
      if (previousIndex >= 0) {
        const insertAt = Math.min(previousIndex, next.length);
        next.splice(insertAt, 0, nextTitle);
        return next;
      }
      next = [...next, nextTitle];
      return next;
    });
    setActivityKeyword(nextTitle);
    setActivityDropdownOpen(false);
  };

  const clearActivitySelection = ({ clearKeyword = true, closeDropdown = true } = {}) => {
    const previousTitle = taskForm.activityTitle || "";
    setTaskForm((prev) => ({
      ...prev,
      activityTopicId: "",
      activityMissionId: "",
      activityTitle: "",
    }));
    if (clearKeyword) {
      setActivityKeyword("");
    }
    if (closeDropdown) {
      setActivityDropdownOpen(false);
    }
    if (previousTitle) {
      setTags((prev) => prev.filter((tag) => tag !== previousTitle));
    }
  };

  const loadActivities = async (partitionId, keyword = "") => {
    const requestSeq = activityRequestSeqRef.current + 1;
    activityRequestSeqRef.current = requestSeq;
    setActivityLoading(true);
    setActivityMessage("");
    try {
      const normalizedKeyword = String(keyword || "").trim();
      const currentBilibiliUid = Number(selectedBilibiliUid || currentUpProfile?.uid || 0);
      const data = await invokeCommand("bilibili_topics", {
        partitionId: partitionId ? Number(partitionId) : null,
        title: normalizedKeyword || null,
        bilibiliUid:
          Number.isFinite(currentBilibiliUid) && currentBilibiliUid > 0
            ? currentBilibiliUid
            : null,
      });
      if (requestSeq !== activityRequestSeqRef.current) {
        return;
      }
      const mapped = normalizeActivityOptions(data);
      setActivityOptions(mapped);
      const currentId = Number(taskForm.activityTopicId || 0);
      if (currentId > 0 && mapped.length > 0 && !mapped.some((item) => item.topicId === currentId)) {
        clearActivitySelection({ clearKeyword: false, closeDropdown: false });
      }
    } catch (error) {
      if (requestSeq !== activityRequestSeqRef.current) {
        return;
      }
      setActivityOptions([]);
      setActivityMessage(error.message);
    } finally {
      if (requestSeq === activityRequestSeqRef.current) {
        setActivityLoading(false);
      }
    }
  };

  const handleActivitySelect = (value) => {
    if (!value) {
      applyActivitySelection(null);
      return;
    }
    const target = activitySelectOptions.find((item) => String(item.topicId) === value);
    if (!target) {
      applyActivitySelection(null);
      return;
    }
    applyActivitySelection(target);
  };

  useEffect(() => {
    if (!isCreateView && !isEditView) {
      return;
    }
    const partitionId = Number(taskForm.partitionId || 0);
    if (!partitionId) {
      activityRequestSeqRef.current += 1;
      setActivityLoading(false);
      setActivityOptions([]);
      clearActivitySelection({ clearKeyword: true, closeDropdown: true });
      return;
    }
    loadActivities(partitionId, activityKeyword);
  }, [isCreateView, isEditView, taskForm.partitionId, partitions]);

  useEffect(() => {
    if (!isCreateView && !isEditView) {
      return undefined;
    }
    const partitionId = Number(taskForm.partitionId || 0);
    if (!partitionId) {
      return undefined;
    }
    const timer = window.setTimeout(() => {
      loadActivities(partitionId, activityKeyword);
    }, 320);
    return () => {
      window.clearTimeout(timer);
    };
  }, [
    isCreateView,
    isEditView,
    activityKeyword,
  ]);

  useEffect(() => {
    if (!isCreateView) {
      return;
    }
    const currentCollectionId = String(taskForm.collectionId || "").trim();
    if (!currentCollectionId) {
      return;
    }
    const exists = collections.some(
      (collection) => String(collection.seasonId) === currentCollectionId,
    );
    if (exists) {
      return;
    }
    setTaskForm((prev) => ({ ...prev, collectionId: "" }));
  }, [isCreateView, collections, taskForm.collectionId]);

  const loadTasks = async (
    filter = statusFilter,
    page = currentPage,
    size = pageSize,
    refreshRemote = false,
    keyword = taskSearch,
    sourceType = sourceFilter,
    source = "auto",
    bilibiliUidOverride = "",
  ) => {
    try {
      const payload = { page, page_size: size, pageSize: size };
      const currentBilibiliUid = Number(
        bilibiliUidOverride || selectedBilibiliUid || currentUpProfile?.uid || 0,
      );
      if (Number.isFinite(currentBilibiliUid) && currentBilibiliUid > 0) {
        payload.bilibiliUid = currentBilibiliUid;
      }
      const trimmedKeyword = keyword?.trim();
      if (trimmedKeyword) {
        payload.query = trimmedKeyword;
      }
      if (sourceType && sourceType !== "ALL") {
        payload.sourceType = sourceType;
      }
      if (source === "page_size_change") {
        try {
          await invokeCommand("auth_client_log", {
            message: `submission_list_request source=${source} page=${page} size=${size} status=${filter} query=${trimmedKeyword || "-"}`,
          });
        } catch (_) {}
      }
      if (refreshRemote) {
        payload.refresh_remote = true;
      }
      const data =
        filter === "ALL"
          ? await invokeCommand("submission_list", payload)
          : await invokeCommand("submission_list_by_status", {
              status: filter,
              ...payload,
            });
      const items = data?.items || [];
      const total = Number(data?.total) || 0;
      if (source === "page_size_change") {
        try {
          await invokeCommand("auth_client_log", {
            message: `submission_list_response source=${source} page=${page} size=${size} items=${items.length} total=${total}`,
          });
        } catch (_) {}
      }
      setTasks(items);
      setTotalTasks(total);
      const maxPage = Math.max(1, Math.ceil(total / size));
      if (page > maxPage) {
        setCurrentPage(maxPage);
      }
    } catch (error) {
      setMessage(error.message);
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

  const resetSegmentBindingState = () => {
    setSegmentBindingSelection(new Set());
    setSegmentBindingMergedId("");
    setSegmentBindingSubmitting(false);
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

  const loadQuickFillTasks = async (page = quickFillPage, keyword = quickFillSearch) => {
    try {
      try {
        await invokeCommand("auth_client_log", {
          message: `quick_fill_request page=${page} size=${quickFillPageSize}`,
        });
      } catch (_) {}
      const payload = { page, page_size: quickFillPageSize, pageSize: quickFillPageSize };
      const trimmedKeyword = keyword?.trim();
      if (trimmedKeyword) {
        payload.query = trimmedKeyword;
      }
      const data = await invokeCommand("submission_list", payload);
      const items = data?.items || [];
      const total = Number(data?.total) || 0;
      try {
        await invokeCommand("auth_client_log", {
          message: `quick_fill_response page=${page} items=${items.length} total=${total}`,
        });
      } catch (_) {}
      setQuickFillTasks(items);
      setQuickFillTotal(total);
      const maxPage = Math.max(1, Math.ceil(total / quickFillPageSize));
      if (page > maxPage) {
        setQuickFillPage(maxPage);
      }
    } catch (error) {
      setMessage(error.message);
    }
  };

  const resolveSyncPath = (target) => {
    const fallbackPath = defaultBaiduSyncPath || "/录播";
    if (target === "update") {
      return updateBaiduSync.path || fallbackPath;
    }
    if (target === "repost") {
      return repostBaiduSync.path || fallbackPath;
    }
    return taskForm.baiduSyncPath || fallbackPath;
  };

  const applySyncPath = (target, path) => {
    if (target === "update") {
      setUpdateBaiduSync((prev) => ({ ...prev, path }));
      return;
    }
    if (target === "repost") {
      setRepostBaiduSync((prev) => ({ ...prev, path }));
      return;
    }
    setTaskForm((prev) => ({ ...prev, baiduSyncPath: path }));
  };

  const handleOpenSyncPicker = (target) => {
    setSyncTarget(target);
    setSyncPickerOpen(true);
  };

  const handleCloseSyncPicker = () => {
    setSyncPickerOpen(false);
    setSyncTarget("");
  };

  const handleConfirmSyncPicker = (path) => {
    if (syncTarget) {
      applySyncPath(syncTarget, path);
    }
    setSyncPickerOpen(false);
    setSyncTarget("");
  };

  const handleSyncPathChange = (path) => {
    if (!syncTarget) {
      return;
    }
    applySyncPath(syncTarget, path);
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

  const closeRemoteFilePicker = () => {
    if (bindingRemoteFile) {
      return;
    }
    setRemoteFilePickerOpen(false);
    setBindingMergedVideo(null);
  };

  const handleConfirmRemoteFileBinding = async (remotePath) => {
    if (!bindingMergedVideo?.taskId || !bindingMergedVideo?.mergedId) {
      setMessage("绑定目标不存在，请重试");
      return;
    }
    setBindingRemoteFile(true);
    try {
      await invokeCommand("submission_bind_merged_remote_file", {
        request: {
          taskId: bindingMergedVideo.taskId,
          mergedId: bindingMergedVideo.mergedId,
          remotePath,
        },
      });
      const detail = await fetchTaskDetail(bindingMergedVideo.taskId, { log: false });
      setSelectedTask(detail);
      setMessage("网盘文件绑定成功");
      setRemoteFilePickerOpen(false);
      setBindingMergedVideo(null);
    } catch (error) {
      setMessage(error?.message || "绑定网盘文件失败");
    } finally {
      setBindingRemoteFile(false);
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

  const resetMergedOrderDraft = () => {
    const mergedIds = (Array.isArray(selectedTask?.mergedVideos) ? selectedTask.mergedVideos : [])
      .map((item) => String(item?.id || "").trim())
      .filter((item) => item);
    setMergedOrderDraftIds(mergedIds);
    setMergedOrderDirty(false);
    mergedDragStateRef.current = { activeId: "", overId: "" };
    setDraggingMergedId("");
  };

  const reorderMergedDraft = (sourceId, targetId) => {
    const normalizedSourceId = String(sourceId || "").trim();
    const normalizedTargetId = String(targetId || "").trim();
    if (!normalizedSourceId || !normalizedTargetId || normalizedSourceId === normalizedTargetId) {
      return;
    }
    setMergedOrderDraftIds((prev) => {
      const fromIndex = prev.findIndex((item) => item === normalizedSourceId);
      const toIndex = prev.findIndex((item) => item === normalizedTargetId);
      if (fromIndex < 0 || toIndex < 0 || fromIndex === toIndex) {
        return prev;
      }
      const next = [...prev];
      const [moved] = next.splice(fromIndex, 1);
      next.splice(toIndex, 0, moved);
      setMergedOrderDirty(true);
      return next;
    });
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

  const trackPointerOverMerged = (event) => {
    const { activeId } = mergedDragStateRef.current;
    if (!activeId) {
      return;
    }
    const { clientX, clientY } = event;
    if (!Number.isFinite(clientX) || !Number.isFinite(clientY)) {
      return;
    }
    const target = document.elementFromPoint(clientX, clientY);
    if (!target || typeof target.closest !== "function") {
      return;
    }
    const row = target.closest("tr[data-merged-id]");
    const overId = String(row?.dataset?.mergedId || "").trim();
    if (!overId || overId === mergedDragStateRef.current.overId) {
      return;
    }
    mergedDragStateRef.current.overId = overId;
    reorderMergedDraft(activeId, overId);
  };

  const endMergedPointerDrag = () => {
    const { activeId } = mergedDragStateRef.current;
    if (!activeId) {
      return;
    }
    mergedDragStateRef.current = { activeId: "", overId: "" };
    setDraggingMergedId("");
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

  const closeMergedBindDialog = (force = false) => {
    if (!force && mergedBindSubmitting) {
      return;
    }
    setMergedBindDialogOpen(false);
    setMergedBindTarget(null);
    setMergedBindSelection(new Set());
    setMergedBindMessage("");
  };

  const toggleMergedBindSelection = (sourceId, checked) => {
    const normalized = String(sourceId || "").trim();
    if (!normalized) {
      return;
    }
    setMergedBindSelection((prev) => {
      const next = new Set(prev);
      if (checked) {
        next.add(normalized);
      } else {
        next.delete(normalized);
      }
      return next;
    });
  };

  const handleConfirmMergedBindDialog = async () => {
    if (!mergedBindTarget?.taskId || !mergedBindTarget?.mergedId) {
      setMergedBindMessage("绑定目标不存在，请重试");
      return;
    }
    const sourceIds = Array.from(mergedBindSelection);
    if (!sourceIds.length) {
      setMergedBindMessage("至少保留一个绑定源视频");
      return;
    }
    setMergedBindSubmitting(true);
    setMergedBindMessage("");
    try {
      const result = await invokeCommand("submission_bind_merged_sources", {
        request: {
          taskId: mergedBindTarget.taskId,
          mergedId: mergedBindTarget.mergedId,
          sourceIds,
        },
      });
      const detail = await fetchTaskDetail(mergedBindTarget.taskId, { log: false });
      setSelectedTask(detail);
      setMessage(result || "绑定源视频成功");
      closeMergedBindDialog(true);
    } catch (error) {
      setMergedBindMessage(error?.message || "绑定源视频失败");
    } finally {
      setMergedBindSubmitting(false);
    }
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
      if (deleteLocalFile) {
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

  useEffect(() => {
    loadPartitions();
    loadCollections();
    loadBaiduSyncSettings();
    loadCurrentUpProfile();
  }, []);

  useEffect(() => {
    if (!normalizedActiveBilibiliUid) {
      return;
    }
    setSelectedBilibiliUid((prev) =>
      prev === normalizedActiveBilibiliUid ? prev : normalizedActiveBilibiliUid,
    );
  }, [normalizedActiveBilibiliUid]);

  useEffect(() => {
    if (!isRemoteImportView) {
      return;
    }
    if (remoteImportPreview) {
      return;
    }
    setSubmissionView("list");
    setMessage("导入上下文已失效，请重新发起视频导入");
  }, [isRemoteImportView, remoteImportPreview]);

  useEffect(() => {
    if (!isDetailView && !isEditView) {
      lastDetailTaskIdRef.current = null;
      lastEditTaskIdRef.current = null;
      setEditBaseline(null);
      return;
    }
    const taskId = selectedTask?.task?.taskId;
    if (!taskId) {
      return;
    }
    if (isDetailView) {
      if (lastDetailTaskIdRef.current === taskId) {
        return;
      }
      lastDetailTaskIdRef.current = taskId;
      setEditBaseline(null);
      applyDetailToForm(selectedTask);
      return;
    }
    if (isEditView) {
      if (lastEditTaskIdRef.current === taskId) {
        return;
      }
      lastEditTaskIdRef.current = taskId;
      applyDetailToForm(selectedTask);
      initEditSegments(selectedTask);
    }
  }, [submissionView, selectedTask, isDetailView, isEditView]);

  useEffect(() => {
    if (submissionView !== "list") {
      return undefined;
    }
    loadTasks(statusFilter, currentPage, pageSize, false, taskSearch, sourceFilter);
    return undefined;
    }, [submissionView, statusFilter, sourceFilter, currentPage, pageSize, taskSearch, selectedBilibiliUid]);

  useEffect(() => {
    if (submissionView !== "list") {
      return;
    }
    setSelectedTaskIds(new Set());
  }, [submissionView, statusFilter, sourceFilter, taskSearch, selectedBilibiliUid]);

  useEffect(() => {
    if (submissionView !== "list") {
      return undefined;
    }
    const timer = setInterval(() => {
      loadTasks(statusFilter, currentPage, pageSize, false, taskSearch, sourceFilter);
    }, 3000);
    return () => clearInterval(timer);
  }, [submissionView, statusFilter, sourceFilter, currentPage, pageSize, taskSearch, selectedBilibiliUid]);

  useEffect(() => {
    if (!quickFillOpen) {
      return undefined;
    }
    loadQuickFillTasks(quickFillPage);
    return undefined;
  }, [quickFillOpen, quickFillPage, quickFillSearch]);

  useEffect(() => {
    if (!isDetailView || !selectedTask?.task?.taskId) {
      return undefined;
    }
    let active = true;
    const taskId = selectedTask.task.taskId;
    const refreshDetail = async () => {
      try {
        const detail = await fetchTaskDetail(taskId, { log: false });
        if (!active) {
          return;
        }
        setSelectedTask(detail);
      } catch (error) {
        if (active) {
          setMessage(error.message);
        }
      }
    };
    refreshDetail();
    const timer = setInterval(refreshDetail, 3000);
    return () => {
      active = false;
      clearInterval(timer);
    };
  }, [submissionView, selectedTask?.task?.taskId, isDetailView]);

  useEffect(() => {
    if (!isEditView || !selectedTask?.task?.taskId || pendingEditUploads === 0) {
      return undefined;
    }
    let active = true;
    const taskId = selectedTask.task.taskId;
    const refreshStatus = async () => {
      const pendingIds = (editSegmentsRef.current || [])
        .filter(
          (segment) =>
            segment.uploadStatus === "UPLOADING" ||
            segment.uploadStatus === "RATE_LIMITED",
        )
        .map((segment) => segment.segmentId);
      if (!pendingIds.length) {
        return;
      }
      try {
        const updates = await invokeCommand("submission_edit_upload_status", {
          request: {
            taskId,
            segmentIds: pendingIds,
          },
        });
        if (!active) {
          return;
        }
        setEditSegments((prev) => mergeEditUploadStatus(prev, updates || []));
      } catch (error) {
        if (active) {
          setMessage(error.message);
        }
      }
    };
    refreshStatus();
    const timer = setInterval(refreshStatus, 2000);
    return () => {
      active = false;
      clearInterval(timer);
    };
  }, [isEditView, pendingEditUploads, selectedTask?.task?.taskId]);

  const openFileDialog = async (index) => {
    await handleSelectSourceFile(index);
  };

  const addSource = () => {
    setSourceVideos((prev) => [...prev, emptySource(prev.length)]);
  };

  const updateSource = (index, field, value) => {
    setSourceVideos((prev) =>
      prev.map((item, idx) => (idx === index ? { ...item, [field]: value } : item)),
    );
  };

  const updateSourceTime = (index, field, value) => {
    setSourceVideos((prev) =>
      prev.map((item, idx) => {
        if (idx !== index) {
          return item;
        }
        return { ...item, [field]: value };
      }),
    );
  };

  const normalizeSourceTime = (index, field) => {
    setSourceVideos((prev) =>
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

  const removeSource = (index) => {
    setSourceVideos((prev) =>
      prev
        .filter((_, idx) => idx !== index)
        .map((item, idx) => ({ ...item, sortOrder: idx + 1 })),
    );
  };

  function buildGroupedSourceIdSet(items) {
    const grouped = new Set();
    items
      .filter((item) => item.type === "GROUP")
      .forEach((item) => {
        (item.sourceIds || []).forEach((sourceId) => grouped.add(sourceId));
      });
    return grouped;
  }

  const syncMergeItemsWithSources = (sources, prevItems) => {
    const sourceIds = (sources || [])
      .map((item) => item?.localId)
      .filter((id) => id);
    const sourceIdSet = new Set(sourceIds);
    const next = [];
    const used = new Set();
    for (const item of prevItems || []) {
      if (item.type === "GROUP") {
        const nextSourceIds = (item.sourceIds || []).filter((id) => sourceIdSet.has(id));
        if (nextSourceIds.length > 0) {
          next.push({ ...item, sourceIds: nextSourceIds });
          nextSourceIds.forEach((id) => used.add(id));
        }
        continue;
      }
      if (sourceIdSet.has(item.sourceId)) {
        next.push(item);
        used.add(item.sourceId);
      }
    }
    for (const id of sourceIds) {
      if (!used.has(id)) {
        next.push({ id, type: "SOURCE", sourceId: id, standalone: false });
      }
    }
    return next;
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

  const reorderMergeItems = (sourceId, targetId) => {
    const normalizedSourceId = String(sourceId || "").trim();
    const normalizedTargetId = String(targetId || "").trim();
    if (!normalizedSourceId || !normalizedTargetId || normalizedSourceId === normalizedTargetId) {
      return;
    }
    setMergeItems((prev) => {
      const fromIndex = prev.findIndex((item) => item.id === normalizedSourceId);
      const toIndex = prev.findIndex((item) => item.id === normalizedTargetId);
      if (fromIndex < 0 || toIndex < 0 || fromIndex === toIndex) {
        return prev;
      }
      const next = [...prev];
      const [moved] = next.splice(fromIndex, 1);
      next.splice(toIndex, 0, moved);
      return next;
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

  const trackPointerOverMergeItem = (event) => {
    const { activeId } = mergeDragStateRef.current;
    if (!activeId) {
      return;
    }
    const { clientX, clientY } = event;
    if (!Number.isFinite(clientX) || !Number.isFinite(clientY)) {
      return;
    }
    const target = document.elementFromPoint(clientX, clientY);
    if (!target || typeof target.closest !== "function") {
      return;
    }
    const row = target.closest("[data-merge-item-id]");
    const overId = String(row?.dataset?.mergeItemId || "").trim();
    if (!overId || overId === mergeDragStateRef.current.overId) {
      return;
    }
    mergeDragStateRef.current.overId = overId;
    reorderMergeItems(activeId, overId);
  };

  const endMergeItemDrag = () => {
    const { activeId } = mergeDragStateRef.current;
    if (!activeId) {
      return;
    }
    mergeDragStateRef.current = { activeId: "", overId: "" };
    setDraggingMergeItemId("");
  };

  const reorderMergeGroupSources = (groupId, sourceId, targetId) => {
    const normalizedGroupId = String(groupId || "").trim();
    const normalizedSourceId = String(sourceId || "").trim();
    const normalizedTargetId = String(targetId || "").trim();
    if (!normalizedGroupId || !normalizedSourceId || !normalizedTargetId) {
      return;
    }
    if (normalizedSourceId === normalizedTargetId) {
      return;
    }
    setMergeItems((prev) =>
      prev.map((item) => {
        if (item.type !== "GROUP" || item.id !== normalizedGroupId) {
          return item;
        }
        const sourceIds = [...(item.sourceIds || [])];
        const fromIndex = sourceIds.findIndex((id) => id === normalizedSourceId);
        const toIndex = sourceIds.findIndex((id) => id === normalizedTargetId);
        if (fromIndex < 0 || toIndex < 0 || fromIndex === toIndex) {
          return item;
        }
        const [moved] = sourceIds.splice(fromIndex, 1);
        sourceIds.splice(toIndex, 0, moved);
        return { ...item, sourceIds };
      }),
    );
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

  const trackPointerOverMergeGroupSource = (event) => {
    const { groupId, activeId } = mergeGroupDragStateRef.current;
    if (!groupId || !activeId) {
      return;
    }
    const { clientX, clientY } = event;
    if (!Number.isFinite(clientX) || !Number.isFinite(clientY)) {
      return;
    }
    const target = document.elementFromPoint(clientX, clientY);
    if (!target || typeof target.closest !== "function") {
      return;
    }
    const row = target.closest("tr[data-merge-group-id][data-merge-source-id]");
    const overGroupId = String(row?.dataset?.mergeGroupId || "").trim();
    const overId = String(row?.dataset?.mergeSourceId || "").trim();
    if (!overGroupId || !overId || overGroupId !== groupId) {
      return;
    }
    if (overId === mergeGroupDragStateRef.current.overId) {
      return;
    }
    mergeGroupDragStateRef.current.overId = overId;
    reorderMergeGroupSources(groupId, activeId, overId);
  };

  const endMergeGroupSourceDrag = () => {
    const { activeId } = mergeGroupDragStateRef.current;
    if (!activeId) {
      return;
    }
    mergeGroupDragStateRef.current = { groupId: "", activeId: "", overId: "" };
    setDraggingMergeGroupSource({ groupId: "", sourceId: "" });
  };

  const buildMergeGroupsPayload = (validSources) => {
    const hasGroup = mergeItems.some((item) => item.type === "GROUP");
    const hasStandalone = mergeItems.some((item) => item.type === "SOURCE" && item.standalone);
    if (!hasGroup && !hasStandalone) {
      return [];
    }
    const sourceMap = new Map(
      (validSources || [])
        .filter((item) => item?.localId)
        .map((item) => [item.localId, item]),
    );
    const groups = [];
    const remainingSources = [];
    for (const item of mergeItems) {
      if (item.type === "GROUP") {
        const groupSources = (item.sourceIds || [])
          .map((sourceId) => sourceMap.get(sourceId))
          .filter(Boolean)
          .map((source, index) => ({
            sourceFilePath: source.sourceFilePath,
            startTime: source.startTime || null,
            endTime: source.endTime || null,
            sortOrder: index + 1,
          }));
        if (groupSources.length > 0) {
          groups.push({
            order: groups.length + 1,
            sources: groupSources,
          });
        }
        continue;
      }
      const source = sourceMap.get(item.sourceId);
      if (!source) {
        continue;
      }
      if (item.standalone) {
        groups.push({
          order: groups.length + 1,
          sources: [
            {
              sourceFilePath: source.sourceFilePath,
              startTime: source.startTime || null,
              endTime: source.endTime || null,
              sortOrder: 1,
            },
          ],
        });
      } else {
        remainingSources.push({
          sourceFilePath: source.sourceFilePath,
          startTime: source.startTime || null,
          endTime: source.endTime || null,
        });
      }
    }
    if (remainingSources.length > 0) {
      groups.push({
        order: groups.length + 1,
        sources: remainingSources.map((s, index) => ({ ...s, sortOrder: index + 1 })),
      });
    }
    return groups;
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
              <button
                className="rounded-lg border border-black/10 bg-white px-3 py-2 text-xs font-semibold text-[var(--ink)]"
                onClick={() => openFileDialog(sourceIndex)}
              >
                选择
              </button>
            ) : null}
          </div>
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
            <button
              className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
              onClick={() => removeSource(sourceIndex)}
            >
              删除
            </button>
          ) : null}
        </td>
      </tr>
    );
  };

  const addUpdateSource = () => {
    setUpdateSourceVideos((prev) => [...prev, emptySource(prev.length)]);
  };

  const updateUpdateSource = (index, field, value) => {
    setUpdateSourceVideos((prev) =>
      prev.map((item, idx) => (idx === index ? { ...item, [field]: value } : item)),
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
    setUpdateSourceVideos((prev) =>
      prev
        .filter((_, idx) => idx !== index)
        .map((item, idx) => ({ ...item, sortOrder: idx + 1 })),
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

  const buildUpdateWorkflowConfig = () => {
    const prefix = updateSegmentPrefix.trim();
    return {
      enableSegmentation: updateSegmentationEnabled,
      segmentationConfig: {
        enabled: updateSegmentationEnabled,
        segmentDurationSeconds: updateWorkflowConfig.segmentationConfig.segmentDurationSeconds,
        preserveOriginal: updateWorkflowConfig.segmentationConfig.preserveOriginal,
      },
      segmentPrefix: prefix ? prefix : null,
    };
  };

  const addTag = (value) => {
    const nextTag = value.trim();
    if (!nextTag) {
      return;
    }
    if (tags.includes(nextTag)) {
      return;
    }
    setTags((prev) => [...prev, nextTag]);
  };

  const removeTag = (target) => {
    setTags((prev) => prev.filter((tag) => tag !== target));
    if (target === taskForm.activityTitle) {
      setTaskForm((prev) => ({
        ...prev,
        activityTopicId: "",
        activityMissionId: "",
        activityTitle: "",
      }));
    }
  };

  const handleTagKeyDown = (event) => {
    if (event.key !== "Enter") {
      return;
    }
    event.preventDefault();
    addTag(tagInput);
    setTagInput("");
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
            tags: uniqueTags.join(","),
            topicId: taskForm.activityTopicId ? Number(taskForm.activityTopicId) : null,
            missionId: taskForm.activityMissionId ? Number(taskForm.activityMissionId) : null,
            activityTitle: taskForm.activityTitle || null,
            videoType: taskForm.videoType,
            bilibiliUid: selectedBilibiliUid ? Number(selectedBilibiliUid) : null,
            segmentPrefix: taskForm.segmentPrefix || null,
            priority: Boolean(taskForm.priority),
            baiduSyncEnabled: Boolean(taskForm.baiduSyncEnabled),
            baiduSyncPath: taskForm.baiduSyncPath || null,
            baiduSyncFilename: taskForm.baiduSyncFilename || null,
          },
          sourceVideos: validSources.map((item, index) => ({
            sourceFilePath: item.sourceFilePath,
            sortOrder: index + 1,
            startTime: item.startTime || null,
            endTime: item.endTime || null,
          })),
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
    setUpdateSubmitting(true);
    try {
      const payload = {
        request: {
          taskId,
          baiduSyncEnabled: Boolean(updateBaiduSync.enabled),
          baiduSyncPath: updateBaiduSync.path || null,
          baiduSyncFilename: updateBaiduSync.filename || null,
          sourceVideos: validSources.map((item, index) => ({
            sourceFilePath: item.sourceFilePath,
            sortOrder: index + 1,
            startTime: item.startTime || null,
            endTime: item.endTime || null,
          })),
          workflowConfig: buildUpdateWorkflowConfig(),
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

  const handleSelectSourceFile = async (index) => {
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
      setSourceVideos((prev) =>
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
            sourceFilePath: selected,
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
            sourceFilePath: selected,
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

  const applyTaskSummaryToForm = (task) => {
    if (!task) {
      return;
    }
    const partitionId = task.partitionId ?? task.partition_id;
    const collectionId = task.collectionId ?? task.collection_id;
    const topicId = task.topicId ?? task.topic_id;
    const missionId = task.missionId ?? task.mission_id;
    const activityTitle = task.activityTitle ?? task.activity_title;
    const coverUrl = task.coverUrl ?? task.cover_url;
    const coverLocalPath = task.coverLocalPath ?? task.cover_local_path;
    const baiduSyncEnabled = task.baiduSyncEnabled ?? task.baidu_sync_enabled;
    const baiduSyncPath = task.baiduSyncPath ?? task.baidu_sync_path;
    const baiduSyncFilename = task.baiduSyncFilename ?? task.baidu_sync_filename;
    const tagList = String(task.tags || "")
      .split(",")
      .map((item) => item.trim())
      .filter((item) => item);
    setTaskForm((prev) => ({
      ...prev,
      title: task.title || "",
      description: task.description || "",
      coverUrl: coverUrl || "",
      coverLocalPath: coverLocalPath || "",
      coverDataUrl: "",
      partitionId: partitionId ? String(partitionId) : "",
      collectionId: collectionId ? String(collectionId) : "",
      activityTopicId: topicId ? String(topicId) : "",
      activityMissionId: missionId ? String(missionId) : "",
      activityTitle: activityTitle || "",
      videoType: task.videoType || "ORIGINAL",
      priority: Boolean(task.priority),
      baiduSyncEnabled: Boolean(baiduSyncEnabled),
      baiduSyncPath: baiduSyncPath || "",
      baiduSyncFilename: baiduSyncFilename || "",
    }));
    setTags(tagList);
    setTagInput("");
    setActivityKeyword(activityTitle || "");
    setActivityDropdownOpen(false);
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

  const closeQuickFill = () => {
    setQuickFillOpen(false);
  };

  const handleQuickFillSelect = (task) => {
    applyTaskSummaryToForm(task);
    setQuickFillOpen(false);
  };

  const applyDetailToForm = (detail) => {
    const task = detail?.task || {};
    const partitionId = task.partitionId ?? task.partition_id;
    const collectionId = task.collectionId ?? task.collection_id;
    const topicId = task.topicId ?? task.topic_id;
    const missionId = task.missionId ?? task.mission_id;
    const activityTitle = task.activityTitle ?? task.activity_title;
    const coverUrl = task.coverUrl ?? task.cover_url;
    const coverLocalPath = task.coverLocalPath ?? task.cover_local_path;
    const videoType = task.videoType ?? task.video_type;
    const segmentPrefix = task.segmentPrefix ?? task.segment_prefix;
    const baiduSyncEnabled = task.baiduSyncEnabled ?? task.baidu_sync_enabled;
    const baiduSyncPath = task.baiduSyncPath ?? task.baidu_sync_path;
    const baiduSyncFilename = task.baiduSyncFilename ?? task.baidu_sync_filename;
    const tagList = String(task.tags || "")
      .split(",")
      .map((item) => item.trim())
      .filter((item) => item);
    setTaskForm({
      title: task.title || "",
      description: task.description || "",
      coverUrl: coverUrl || "",
      coverLocalPath: coverLocalPath || "",
      coverDataUrl: "",
      partitionId: partitionId ? String(partitionId) : "",
      collectionId: collectionId ? String(collectionId) : "",
      activityTopicId: topicId ? String(topicId) : "",
      activityMissionId: missionId ? String(missionId) : "",
      activityTitle: activityTitle || "",
      videoType: videoType || "ORIGINAL",
      segmentPrefix: segmentPrefix || "",
      priority: Boolean(task.priority),
      baiduSyncEnabled: Boolean(baiduSyncEnabled),
      baiduSyncPath: baiduSyncPath || "",
      baiduSyncFilename: baiduSyncFilename || "",
    });
    setTags(tagList);
    setTagInput("");
    setActivityKeyword(activityTitle || "");
    setActivityDropdownOpen(false);
    const config = detail?.workflowConfig || {};
    const segmentation = config?.segmentationConfig || {};
    const enableSegmentation =
      typeof segmentation.enabled === "boolean"
        ? segmentation.enabled
        : Boolean(config?.enableSegmentation);
    setSegmentationEnabled(enableSegmentation);
    setWorkflowConfig({
      segmentationConfig: {
        segmentDurationSeconds: Number(segmentation.segmentDurationSeconds || 133),
        preserveOriginal:
          typeof segmentation.preserveOriginal === "boolean"
            ? segmentation.preserveOriginal
            : true,
      },
    });
    const sources = (detail?.sourceVideos || []).map((item, index) => ({
      localId: buildSourceId(),
      sourceFilePath: item.sourceFilePath || "",
      sortOrder: index + 1,
      startTime: item.startTime || "00:00:00",
      endTime: item.endTime || "00:00:00",
      durationSeconds: 0,
    }));
    const normalizedSources = sources.length ? sources : [emptySource(0)];
    setSourceVideos(normalizedSources);
    setMergeItems(buildMergeItemsFromSources(normalizedSources));
    setMergeSelection(new Set());
  };

  const initEditSegments = (detail) => {
    const segments = (detail?.outputSegments || []).map((segment) => ({ ...segment }));
    setEditSegments(segments);
    setEditingSegmentId("");
    setEditingSegmentName("");
    setDraggingSegmentId("");
  };

  const mergeEditUploadStatus = (current, updates) => {
    if (!updates || updates.length === 0) {
      return current;
    }
    const updateMap = new Map(
      updates.map((segment) => [segment.segmentId, segment]),
    );
    return current.map((segment) => {
      const update = updateMap.get(segment.segmentId);
      if (!update) {
        return segment;
      }
      return {
        ...segment,
        partName: update.partName || segment.partName,
        segmentFilePath: update.segmentFilePath || segment.segmentFilePath,
        uploadStatus: update.uploadStatus || segment.uploadStatus,
        uploadProgress:
          typeof update.uploadProgress === "number"
            ? update.uploadProgress
            : segment.uploadProgress,
        cid: update.cid ?? segment.cid,
        fileName: update.fileName ?? segment.fileName,
        uploadUploadedBytes:
          typeof update.uploadUploadedBytes === "number"
            ? update.uploadUploadedBytes
            : segment.uploadUploadedBytes,
        uploadTotalBytes:
          typeof update.uploadTotalBytes === "number"
            ? update.uploadTotalBytes
            : segment.uploadTotalBytes,
      };
    });
  };

  const fetchTaskDetail = async (taskId, options = {}) => {
    const { log = true } = options;
    if (log) {
      try {
        await invokeCommand("auth_client_log", {
          message: `submission_detail_click taskId=${taskId}`,
        });
      } catch (_) {}
    }
    let detail = await invokeCommand("submission_detail", { taskId });
    if (log) {
      try {
        const hasTask = detail && typeof detail === "object" && "task" in detail;
        await invokeCommand("auth_client_log", {
          message: `submission_detail_result hasTask=${hasTask ? "1" : "0"} keys=${Object.keys(detail || {}).join(",")}`,
        });
      } catch (_) {}
    }
    if (!detail?.task) {
      const raw = await invoke("submission_detail", { taskId });
      if (log) {
        try {
          await invokeCommand("auth_client_log", {
            message: `submission_detail_raw type=${typeof raw} hasCode=${raw && typeof raw.code === "number" ? "1" : "0"}`,
          });
        } catch (_) {}
      }
      if (raw && typeof raw.code === "number") {
        if (raw.code !== 0) {
          throw new Error(raw.message || "读取任务详情失败");
        }
        detail = raw.data || null;
      } else {
        detail = raw;
      }
    }
    if (!detail?.task) {
      throw new Error("未读取到投稿任务详情");
    }
    return detail;
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
        sourceFilePath: item.sourceFilePath,
        sortOrder: index + 1,
        startTime: item.startTime || null,
        endTime: item.endTime || null,
      }));
    }
    const uniqueTags = collectCurrentTags(tags, tagInput);
    if (needEditSubmit) {
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
    const taskPayload = needEditSubmit
      ? {
          title: taskForm.title,
          description: taskForm.description || null,
          coverUrl: taskForm.coverUrl || null,
          coverDataUrl: taskForm.coverDataUrl || null,
          partitionId: Number(taskForm.partitionId),
          collectionId: selectedCollectionId,
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

  const handleCloseEditSubmitConfirm = () => {
    if (submittingEdit) {
      return;
    }
    setEditSubmitConfirmOpen(false);
    setEditSubmitConfirmSyncRemote(true);
    setPendingEditSubmitPayload(null);
    setPendingEditChangedLabel("");
    setPendingEditNeedRemote(false);
  };

  const handleConfirmEditSubmit = async () => {
    if (!pendingEditSubmitPayload || submittingEdit) {
      return;
    }
    const syncRemoteUpdate = pendingEditNeedRemote ? editSubmitConfirmSyncRemote : false;
    if (syncRemoteUpdate && !canSyncRemoteEdit) {
      setMessage("当前任务暂无BVID，无法同步远程，请取消勾选后仅保存本地数据");
      return;
    }
    if (syncRemoteUpdate && pendingEditSubmitPayload.needEditSubmit) {
      const incompleteSegment = pendingEditSubmitPayload.segments.find(
        (segment) => segment.uploadStatus !== "SUCCESS",
      );
      if (incompleteSegment) {
        setMessage("存在未上传成功的分P，请处理后再提交");
        return;
      }
      const missingUploadInfo = pendingEditSubmitPayload.segments.find(
        (segment) => !segment.cid || !segment.fileName,
      );
      if (missingUploadInfo) {
        setMessage("分P上传信息缺失，请重新上传");
        return;
      }
    }
    setSubmittingEdit(true);
    try {
      if (pendingEditSubmitPayload.needUpdateSource) {
        await invokeCommand("submission_edit_update_sources", {
          request: {
            taskId: pendingEditSubmitPayload.taskId,
            sourceVideos: pendingEditSubmitPayload.sourcePayload,
          },
        });
      }
      if (pendingEditSubmitPayload.needEditSubmit) {
        await invokeCommand("submission_edit_submit", {
          request: {
            taskId: pendingEditSubmitPayload.taskId,
            syncRemoteUpdate,
            task: pendingEditSubmitPayload.task,
            segments: pendingEditSubmitPayload.segments,
          },
        });
      }
      setEditSubmitConfirmOpen(false);
      setEditSubmitConfirmSyncRemote(true);
      setPendingEditSubmitPayload(null);
      setPendingEditChangedLabel("");
      setPendingEditNeedRemote(false);
      backToList();
      await loadTasks(statusFilter, currentPage, pageSize);
    } catch (error) {
      setMessage(error.message);
    } finally {
      setSubmittingEdit(false);
    }
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

  const reorderEditSegments = (sourceId, targetId) => {
    setEditSegments((prev) => {
      const fromIndex = prev.findIndex((segment) => segment.segmentId === sourceId);
      const toIndex = prev.findIndex((segment) => segment.segmentId === targetId);
      invokeCommand("auth_client_log", {
        message: `submission_edit_drag_indices sourceId=${sourceId} targetId=${targetId} from=${fromIndex} to=${toIndex}`,
      }).catch(() => {});
      if (fromIndex < 0 || toIndex < 0 || fromIndex === toIndex) {
        return prev;
      }
      const next = [...prev];
      const [moved] = next.splice(fromIndex, 1);
      next.splice(toIndex, 0, moved);
      return next;
    });
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

  const trackPointerOverSegment = (event) => {
    const { activeId } = dragStateRef.current;
    if (!activeId) {
      return;
    }
    const { clientX, clientY } = event;
    if (!Number.isFinite(clientX) || !Number.isFinite(clientY)) {
      return;
    }
    const target = document.elementFromPoint(clientX, clientY);
    if (!target || typeof target.closest !== "function") {
      return;
    }
    const row = target.closest("tr[data-segment-id]");
    const overId = row?.dataset?.segmentId || "";
    if (!overId || overId === dragStateRef.current.overId) {
      return;
    }
    dragStateRef.current.overId = overId;
    reorderEditSegments(activeId, overId);
    invokeCommand("auth_client_log", {
      message: `submission_edit_drag_pointer_over activeId=${activeId} overId=${overId}`,
    }).catch(() => {});
  };

  const endPointerDrag = () => {
    const { activeId, overId } = dragStateRef.current;
    if (!activeId) {
      return;
    }
    dragStateRef.current = { activeId: "", overId: "" };
    setDraggingSegmentId("");
    invokeCommand("auth_client_log", {
      message: `submission_edit_drag_pointer_end activeId=${activeId} overId=${overId}`,
    }).catch(() => {});
  };

  useEffect(() => {
    if (!draggingSegmentId) {
      return undefined;
    }
    const handleMove = (event) => {
      trackPointerOverSegment(event);
    };
    const handleUp = () => {
      endPointerDrag();
    };
    window.addEventListener("pointermove", handleMove);
    window.addEventListener("pointerup", handleUp);
    window.addEventListener("pointercancel", handleUp);
    return () => {
      window.removeEventListener("pointermove", handleMove);
      window.removeEventListener("pointerup", handleUp);
      window.removeEventListener("pointercancel", handleUp);
    };
  }, [draggingSegmentId]);

  useEffect(() => {
    if (!draggingMergedId) {
      return undefined;
    }
    const handleMove = (event) => {
      trackPointerOverMerged(event);
    };
    const handleUp = () => {
      endMergedPointerDrag();
    };
    window.addEventListener("pointermove", handleMove);
    window.addEventListener("pointerup", handleUp);
    window.addEventListener("pointercancel", handleUp);
    return () => {
      window.removeEventListener("pointermove", handleMove);
      window.removeEventListener("pointerup", handleUp);
      window.removeEventListener("pointercancel", handleUp);
    };
  }, [draggingMergedId]);

  useEffect(() => {
    setMergeItems((prev) => {
      const next = syncMergeItemsWithSources(sourceVideos, prev);
      const grouped = buildGroupedSourceIdSet(next);
      setMergeSelection((selection) => {
        const filtered = new Set();
        for (const id of selection) {
          if (!grouped.has(id) && sourceVideos.some((item) => item.localId === id)) {
            filtered.add(id);
          }
        }
        return filtered;
      });
      return next;
    });
  }, [sourceVideos]);

  useEffect(() => {
    if (!draggingMergeItemId) {
      return undefined;
    }
    const handleMove = (event) => {
      trackPointerOverMergeItem(event);
    };
    const handleUp = () => {
      endMergeItemDrag();
    };
    window.addEventListener("pointermove", handleMove);
    window.addEventListener("pointerup", handleUp);
    window.addEventListener("pointercancel", handleUp);
    return () => {
      window.removeEventListener("pointermove", handleMove);
      window.removeEventListener("pointerup", handleUp);
      window.removeEventListener("pointercancel", handleUp);
    };
  }, [draggingMergeItemId]);

  useEffect(() => {
    if (!draggingMergeGroupSource.sourceId) {
      return undefined;
    }
    const handleMove = (event) => {
      trackPointerOverMergeGroupSource(event);
    };
    const handleUp = () => {
      endMergeGroupSourceDrag();
    };
    window.addEventListener("pointermove", handleMove);
    window.addEventListener("pointerup", handleUp);
    window.addEventListener("pointercancel", handleUp);
    return () => {
      window.removeEventListener("pointermove", handleMove);
      window.removeEventListener("pointerup", handleUp);
      window.removeEventListener("pointercancel", handleUp);
    };
  }, [draggingMergeGroupSource]);

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
      await invokeCommand("submission_retry_segment_upload", { segmentId: targetId });
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
    const confirmed = await dialogConfirm("确认将该任务置顶并优先投稿？", {
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

  const formatTaskStatus = (status) => {
    switch (status) {
      case "PENDING":
        return "待处理";
      case "CLIPPING":
        return "剪辑中";
      case "MERGING":
        return "合并中";
      case "SEGMENTING":
        return "分段中";
      case "RUNNING":
        return "处理中";
      case "COMPLETED":
        return "已完成";
      case "WAITING_UPLOAD":
        return "投稿队列中";
      case "UPLOADING":
        return "投稿中";
      case "FAILED":
        return "失败";
      case "CANCELLED":
        return "已取消";
      default:
        return status || "-";
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
    if (["UPLOADING", "WAITING_UPLOAD", "RUNNING"].includes(status)) {
      return "bg-amber-500/10 text-amber-600";
    }
    if (["CLIPPING", "MERGING", "SEGMENTING", "PENDING"].includes(status)) {
      return "bg-amber-500/10 text-amber-600";
    }
    return "bg-slate-500/10 text-slate-600";
  };

  const remoteStatusTone = (status) => {
    if (status === "已通过") return "bg-emerald-500/10 text-emerald-600";
    if (status === "未通过" || status === "已锁定")
      return "bg-rose-500/10 text-rose-600";
    return "bg-amber-500/10 text-amber-600";
  };

  const workflowStatusTone = (status) => {
    if (status === "COMPLETED") return "bg-emerald-500/10 text-emerald-600";
    if (status === "FAILED" || status === "CANCELLED")
      return "bg-rose-500/10 text-rose-600";
    if (status === "RUNNING" || status === "VIDEO_DOWNLOADING")
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
    if (!task?.bvid) {
      return "进行中";
    }
    const state = parseRemoteState(task);
    if (state === null) {
      return "进行中";
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
      return "进行中";
    }
    return "进行中";
  };

  const isRemoteRejected = (task) => {
    const state = parseRemoteState(task);
    if (state === null) {
      return false;
    }
    return state === -2 || state === -4;
  };

  const isRemoteFailed = (task) => {
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
    if (task?.status !== "FAILED") {
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

  const detailSourceOptionList = (selectedTask?.sourceVideos || []).map((item, index) => {
    const sourceId = String(item?.id || "").trim();
    const path = String(item?.sourceFilePath || "").trim();
    const pathName = path.split(/[\\/]/).pop() || path;
    return {
      id: sourceId,
      label: `${index + 1}. ${pathName || sourceId || "-"}`,
      path,
    };
  });
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

  const detailMergedSignature = (selectedTask?.mergedVideos || [])
    .map((item) => String(item?.id || "").trim())
    .filter((item) => item)
    .join(",");

  useEffect(() => {
    resetMergedOrderDraft();
  }, [selectedTask?.task?.taskId, detailMergedSignature]);

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

  const totalClipSeconds = sourceVideos.reduce((acc, item) => {
    const start = parseHmsToSeconds(item.startTime) ?? 0;
    const endRaw = parseHmsToSeconds(item.endTime) ?? 0;
    const end = clampTimeSeconds(endRaw, item.durationSeconds);
    const clipped = Math.max(0, end - clampTimeSeconds(start, item.durationSeconds));
    return acc + clipped;
  }, 0);
  const segmentDurationSeconds = Number(workflowConfig.segmentationConfig.segmentDurationSeconds) || 0;
  const estimatedSegments =
    segmentationEnabled && segmentDurationSeconds > 0
      ? Math.ceil(totalClipSeconds / segmentDurationSeconds)
      : 0;
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
  const collectionLabel =
    collections.find((item) => String(item.seasonId) === String(taskForm.collectionId))
      ?.name ||
    (taskForm.collectionId ? taskForm.collectionId : "-");
  const videoTypeLabel =
    taskForm.videoType === "REPOST" ? "转载" : taskForm.videoType ? "原创" : "-";
  const detailEstimatedSegments = segmentationEnabled
    ? Math.max(selectedTask?.outputSegments?.length || 0, estimatedSegments)
    : 0;
  const hasPartitionOption = partitions.some((item) => {
    return String(item.tid) === String(taskForm.partitionId);
  });
  const hasCollectionOption = collections.some(
    (item) => String(item.seasonId) === String(taskForm.collectionId),
  );
  const partitionOptions =
    canModifySelectedTask && taskForm.partitionId && !hasPartitionOption
      ? [
          ...partitions,
          {
            tid: taskForm.partitionId,
            name: `当前分区(${taskForm.partitionId})`,
          },
        ]
      : partitions;
  const partitionSelectValue = resolvePartitionSelectValue(
    taskForm.partitionId,
    partitionOptions,
  );
  const collectionOptions =
    canModifySelectedTask && taskForm.collectionId && !hasCollectionOption
      ? [
          ...collections,
          {
            seasonId: taskForm.collectionId,
            name: `当前合集(${taskForm.collectionId})`,
          },
        ]
      : collections;
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
  const formatActivityPlayCount = (value) => {
    const count = Number(value || 0);
    if (!Number.isFinite(count) || count <= 0) {
      return "0";
    }
    if (count >= 100000000) {
      return `${(count / 100000000).toFixed(1)}亿`;
    }
    if (count >= 10000) {
      return `${(count / 10000).toFixed(1)}万`;
    }
    return String(Math.floor(count));
  };
  const selectedActivityOption = activitySelectOptions.find(
    (item) => String(item.topicId) === String(taskForm.activityTopicId),
  );
  const activitySummaryText = selectedActivityOption
    ? `播放次数：${formatActivityPlayCount(selectedActivityOption.readCount)}`
    : "未参与活动";
  const isActivityTag = (tag) => Boolean(taskForm.activityTitle) && tag === taskForm.activityTitle;
  const resolveTagClassName = (tag) =>
    isActivityTag(tag)
      ? "inline-flex items-center gap-1 rounded-full border border-amber-200 bg-amber-50 px-2 py-1 text-xs text-amber-700"
      : "inline-flex items-center gap-1 rounded-full bg-[var(--accent)]/10 px-2 py-1 text-xs text-[var(--accent)]";
  const resolveTagRemoveClassName = (tag) =>
    isActivityTag(tag)
      ? "text-[10px] font-semibold text-amber-700 hover:opacity-70"
      : "text-[10px] font-semibold text-[var(--accent)] hover:opacity-70";
  const renderActivityTopicSelector = () => (
    <div className="mt-2 space-y-1">
      <div className="text-xs text-[var(--muted)]">活动话题（可选）</div>
      <div className="relative">
        <input
          value={activityKeyword}
          onChange={(event) => {
            setActivityKeyword(event.target.value);
            setActivityDropdownOpen(true);
          }}
          onFocus={() => setActivityDropdownOpen(true)}
          onBlur={() => {
            window.setTimeout(() => {
              setActivityDropdownOpen(false);
            }, 120);
          }}
          placeholder="输入活动话题关键字并下拉选择"
          disabled={!taskForm.partitionId}
          className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm text-[var(--ink)] focus:border-[var(--accent)] focus:outline-none disabled:cursor-not-allowed disabled:bg-black/5"
        />
        {activityDropdownOpen && taskForm.partitionId ? (
          <div className="absolute z-20 mt-1 max-h-64 w-full overflow-auto rounded-lg border border-black/10 bg-white/95 p-1 shadow-lg">
            <button
              type="button"
              className={`w-full rounded-md px-3 py-2 text-left text-sm ${
                taskForm.activityTopicId
                  ? "text-[var(--ink)] hover:bg-black/5"
                  : "bg-[var(--accent)]/10 text-[var(--accent)]"
              }`}
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => handleActivitySelect("")}
            >
              不参与活动
            </button>
            {activityLoading ? (
              <div className="px-3 py-2 text-xs text-[var(--muted)]">活动加载中...</div>
            ) : null}
            {activityFilteredOptions.map((activity) => {
              const active = String(activity.topicId) === String(taskForm.activityTopicId);
              return (
                <button
                  key={activity.topicId}
                  type="button"
                  className={`mt-1 w-full rounded-md px-3 py-2 text-left ${
                    active
                      ? "bg-[var(--accent)]/10 text-[var(--accent)]"
                      : "text-[var(--ink)] hover:bg-black/5"
                  }`}
                  onMouseDown={(event) => event.preventDefault()}
                  onClick={() => handleActivitySelect(String(activity.topicId))}
                >
                  <div className="flex items-center gap-2">
                    <div className="text-sm font-medium">{activity.name}</div>
                    {activity.showActivityIcon ? (
                      <span className="rounded-full border border-emerald-200 bg-emerald-50 px-2 py-0.5 text-[10px] font-semibold text-emerald-700">
                        活动
                      </span>
                    ) : null}
                  </div>
                  <div className="text-[11px] text-[var(--muted)]">
                    播放 {formatActivityPlayCount(activity.readCount)}
                    {activity.activityText ? ` · ${activity.activityText}` : ""}
                  </div>
                </button>
              );
            })}
            {!activityLoading && !activityFilteredOptions.length ? (
              <div className="px-3 py-2 text-xs text-[var(--muted)]">没有匹配的话题</div>
            ) : null}
          </div>
        ) : null}
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <button
          type="button"
          onClick={() =>
            loadActivities(
              taskForm.partitionId,
              activityKeyword,
            )
          }
          disabled={activityLoading || !taskForm.partitionId}
          className="rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-xs text-[var(--muted)] hover:text-[var(--accent)] disabled:opacity-60"
        >
          刷新活动
        </button>
        {taskForm.activityTopicId ? (
          <button
            type="button"
            onClick={() => handleActivitySelect("")}
            className="rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-xs text-[var(--muted)] hover:text-[var(--accent)]"
          >
            清空选择
          </button>
        ) : null}
      </div>
      <div className="text-[11px] text-[var(--muted)]">{activitySummaryText}</div>
      {activityLoading ? <div className="text-xs text-[var(--muted)]">活动加载中...</div> : null}
      {activityMessage ? <div className="text-xs text-rose-500">{activityMessage}</div> : null}
      {!activityLoading && !activityDropdownOpen && activityFilteredOptions.length === 0 && taskForm.partitionId ? (
        <div className="text-xs text-[var(--muted)]">没有匹配的话题，请尝试更换关键字。</div>
      ) : null}
    </div>
  );
  const normalizeCoverPreviewSrc = (value) => {
    const raw = String(value ?? "").trim();
    if (!raw) {
      return "";
    }
    if (raw.startsWith("//")) {
      return `https:${raw}`;
    }
    if (raw.startsWith("http://")) {
      return `https://${raw.slice("http://".length)}`;
    }
    return raw;
  };
  useEffect(() => {
    let active = true;
    const rawPath = String(taskForm.coverLocalPath || "").trim();
    if (!rawPath) {
      setLocalCoverDataPreviewSrc("");
      return () => {
        active = false;
      };
    }
    const loadLocalCoverPreview = async () => {
      try {
        const data = await invokeCommand("submission_cover_local_preview", {
          request: { filePath: rawPath },
        });
        if (!active) {
          return;
        }
        setLocalCoverDataPreviewSrc(normalizeCoverPreviewSrc(data));
      } catch (_) {
        if (active) {
          setLocalCoverDataPreviewSrc("");
        }
      }
    };
    loadLocalCoverPreview();
    return () => {
      active = false;
    };
  }, [taskForm.coverLocalPath]);
  const localCoverFilePreviewSrc = taskForm.coverLocalPath
    ? normalizeCoverPreviewSrc(convertFileSrc(taskForm.coverLocalPath))
    : "";
  const localCoverPreviewSrc = localCoverDataPreviewSrc || localCoverFilePreviewSrc;
  const sessionCoverPreviewSrc = taskForm.coverDataUrl ? coverPreviewUrl : "";
  const rawCoverPreviewSrc = normalizeCoverPreviewSrc(
    sessionCoverPreviewSrc || localCoverPreviewSrc || taskForm.coverUrl,
  );
  useEffect(() => {
    if (!coverPreviewModalOpen) {
      return undefined;
    }
    const onKeyDown = (event) => {
      if (event.key === "Escape") {
        setCoverPreviewModalOpen(false);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [coverPreviewModalOpen]);
  useEffect(() => {
    let active = true;
    const raw = String(rawCoverPreviewSrc || "").trim();
    if (!raw) {
      setCoverProxyPreviewSrc("");
      return () => {
        active = false;
      };
    }
    if (!/^https?:\/\//i.test(raw)) {
      setCoverProxyPreviewSrc("");
      return () => {
        active = false;
      };
    }
    const cached = coverProxyCacheRef.current.get(raw);
    if (cached) {
      setCoverProxyPreviewSrc(cached);
      return () => {
        active = false;
      };
    }
    setCoverProxyPreviewSrc("");
    const loadCoverProxy = async () => {
      try {
        const data = await invokeCommand("video_proxy_image", { url: raw });
        if (!active) {
          return;
        }
        const normalized = normalizeCoverPreviewSrc(data);
        if (!normalized) {
          return;
        }
        coverProxyCacheRef.current.set(raw, normalized);
        setCoverProxyPreviewSrc(normalized);
      } catch (_) {
        if (active) {
          setCoverProxyPreviewSrc("");
        }
      }
    };
    loadCoverProxy();
    return () => {
      active = false;
    };
  }, [rawCoverPreviewSrc]);
  const coverPreviewSrc = coverProxyPreviewSrc || rawCoverPreviewSrc;
  const openCoverPreviewModal = (src) => {
    const normalized = normalizeCoverPreviewSrc(src);
    if (!normalized) {
      return;
    }
    setCoverPreviewModalSrc(normalized);
    setCoverPreviewModalOpen(true);
  };
  const closeCoverPreviewModal = () => {
    setCoverPreviewModalOpen(false);
  };

  const totalPages = Math.max(1, Math.ceil(totalTasks / pageSize));
  const quickFillTotalPages = Math.max(1, Math.ceil(quickFillTotal / quickFillPageSize));
  const quickFillVisibleTasks = quickFillTasks.slice(0, quickFillPageSize);
  const currentPageTaskIds = tasks
    .map((task) => String(task?.taskId || "").trim())
    .filter((taskId) => taskId);
  const allCurrentPageSelected =
    currentPageTaskIds.length > 0 &&
    currentPageTaskIds.every((taskId) => selectedTaskIds.has(taskId));
  const selectedTaskCount = selectedTaskIds.size;

  return (
    <div className="space-y-6">
      {isCreateView ? (
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
                value={taskForm.collectionId}
                onChange={(event) =>
                  setTaskForm((prev) => ({ ...prev, collectionId: event.target.value }))
                }
                disabled={isReadOnly}
                className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
              >
                <option value="">请选择合集</option>
                {collectionOptions.map((collection) => (
                  <option key={collection.seasonId} value={collection.seasonId}>
                    {collection.name}
                  </option>
                ))}
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
            优先投稿（进入投稿队列时置顶）
          </label>
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
                <th className="px-4 py-2">视频文件（必填）</th>
                <th className="px-4 py-2">开始时间</th>
                <th className="px-4 py-2">结束时间</th>
                <th className="px-4 py-2">合并状态</th>
                <th className="px-4 py-2">操作</th>
              </tr>
            </thead>
            <tbody>
              {mergeItems.length === 0 ? (
                <tr>
                  <td className="px-4 py-3 text-[var(--muted)]" colSpan={8}>
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
                        <td className="p-0" colSpan={8}>
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
      ) : null}

      {quickFillOpen && isCreateView ? (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/30 px-4">
          <div className="w-full max-w-2xl rounded-2xl bg-white p-5 shadow-lg">
            <div className="flex items-center justify-between gap-3">
              <div className="text-sm font-semibold text-[var(--ink)]">一键填写</div>
              <button
                className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                onClick={closeQuickFill}
              >
                关闭
              </button>
            </div>
            <div className="mt-3">
              <input
                value={quickFillSearch}
                onChange={(event) => {
                  setQuickFillSearch(event.target.value);
                  setQuickFillPage(1);
                }}
                placeholder="标题或BV号搜索"
                className="w-full rounded-xl border border-black/10 bg-white px-3 py-2 text-sm text-[var(--ink)]"
              />
            </div>
            <div className="mt-3 h-[420px] overflow-y-auto rounded-xl border border-black/5">
              <table className="w-full text-left text-sm">
                <thead className="bg-black/5 text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
                  <tr>
                    <th className="px-4 py-2">投稿标题</th>
                    <th className="px-4 py-2">创建时间</th>
                  </tr>
                </thead>
                <tbody>
                  {quickFillVisibleTasks.length === 0 ? (
                    <tr>
                      <td className="px-4 py-3 text-[var(--muted)]" colSpan={2}>
                        暂无任务
                      </td>
                    </tr>
                  ) : (
                    quickFillVisibleTasks.map((task) => (
                      <tr
                        key={task.taskId}
                        className="cursor-pointer border-t border-black/5 hover:bg-black/5"
                        onClick={() => handleQuickFillSelect(task)}
                      >
                        <td className="px-4 py-2 text-[var(--ink)]">{task.title}</td>
                        <td className="px-4 py-2 text-[var(--muted)]">
                          {formatDateTime(task.createdAt)}
                        </td>
                      </tr>
                    ))
                  )}
                </tbody>
              </table>
            </div>
            <div className="mt-4 flex flex-wrap items-center justify-between gap-3 text-xs text-[var(--muted)]">
              <div>
                共 {quickFillTotal} 条，当前第 {quickFillPage}/{quickFillTotalPages} 页
              </div>
              <div className="flex items-center gap-2">
                <button
                  className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                  onClick={() => setQuickFillPage((prev) => Math.max(1, prev - 1))}
                  disabled={quickFillPage <= 1}
                >
                  上一页
                </button>
                <button
                  className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                  onClick={() =>
                    setQuickFillPage((prev) => Math.min(quickFillTotalPages, prev + 1))
                  }
                  disabled={quickFillPage >= quickFillTotalPages}
                >
                  下一页
                </button>
              </div>
            </div>
          </div>
        </div>
      ) : null}

      {updateOpen ? (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/30 px-4">
          <div className="w-full max-w-5xl rounded-2xl bg-white p-5 shadow-lg">
            <div className="flex items-center justify-between gap-3">
              <div className="text-sm font-semibold text-[var(--ink)]">视频更新</div>
              <button
                className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                onClick={closeUpdateModal}
              >
                关闭
              </button>
            </div>
            {message ? (
              <div className="mt-3 rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-700">
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
                  <button
                    className="rounded-full bg-[var(--accent)] px-3 py-1 text-xs font-semibold text-white"
                    onClick={addUpdateSource}
                  >
                    添加视频
                  </button>
                </div>
                <div className="mt-3 overflow-hidden rounded-xl border border-black/5">
                  <table className="w-full text-left text-sm">
                    <thead className="bg-black/5 text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
                      <tr>
                        <th className="px-4 py-2">序号</th>
                        <th className="px-4 py-2">视频文件（必填）</th>
                        <th className="px-4 py-2">开始时间</th>
                        <th className="px-4 py-2">结束时间</th>
                        <th className="px-4 py-2">操作</th>
                      </tr>
                    </thead>
                    <tbody>
                      {updateSourceVideos.map((item, index) => (
                        <tr key={`update-source-${index}`} className="border-t border-black/5">
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
                            <button
                              className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                              onClick={() => removeUpdateSource(index)}
                            >
                              删除
                            </button>
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
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
      ) : null}

      {isRemoteImportView && remoteImportPreview ? (
        <RemoteImportTaskView
          initialPreview={remoteImportPreview}
          partitions={partitions}
          collections={collections}
          onBack={backToList}
          onSaved={handleRemoteImportSaved}
        />
      ) : null}

      {submissionView === "list" ? (
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
                <th className="px-6 py-3">BVID</th>
                <th className="px-6 py-3">UP主</th>
                <th className="px-6 py-3">投稿状态</th>
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
                  <td className="px-6 py-4 text-[var(--muted)]" colSpan={10}>
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
                        const showTooltip = task?.status === "FAILED" && reason !== "-";
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
                        {task.status === "WAITING_UPLOAD" ? (
                          <button
                            className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)] disabled:opacity-60"
                            onClick={() => handleQueuePrioritize(task.taskId)}
                            disabled={task.priority}
                          >
                            {task.priority ? "已优先" : "优先投稿"}
                          </button>
                        ) : null}
                        {task.status === "FAILED" && task.hasIntegratedDownloads ? (
                          <button
                            className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                            onClick={() => handleIntegratedExecute(task.taskId)}
                          >
                            一键投稿
                          </button>
                        ) : null}
                        <button
                          className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                          onClick={() => openRepostModal(task)}
                        >
                          重新投稿
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
          {deleteConfirmOpen ? (
            <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/30 px-4">
              <div className="w-full max-w-lg rounded-2xl bg-white p-5 shadow-lg">
              <div className="text-sm font-semibold text-[var(--ink)]">删除投稿任务</div>
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
      ) : null}

      {(isDetailView || isEditView) ? (
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
                          value={taskForm.collectionId}
                          onChange={(event) =>
                            setTaskForm((prev) => ({
                              ...prev,
                              collectionId: event.target.value,
                            }))
                          }
                          className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
                        >
                          <option value="">请选择合集</option>
                          {collectionOptions.map((collection) => (
                            <option key={collection.seasonId} value={collection.seasonId}>
                              {collection.name}
                            </option>
                          ))}
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
                        <th className="px-4 py-2">开始时间</th>
                        <th className="px-4 py-2">结束时间</th>
                        <th className="px-4 py-2">操作</th>
                      </tr>
                    </thead>
                    <tbody>
                      {sourceVideos.length === 0 ? (
                        <tr>
                          <td className="px-4 py-3 text-[var(--muted)]" colSpan={5}>
                            暂无源视频
                          </td>
                        </tr>
                      ) : (
                        sourceVideos.map((item, index) => (
                          <tr key={`edit-source-${index}`} className="border-t border-black/5">
                            <td className="px-4 py-2 text-[var(--muted)]">{index + 1}</td>
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
                              <button
                                className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                                onClick={() => removeSource(index)}
                              >
                                删除
                              </button>
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
                      <th className="px-4 py-2">开始时间</th>
                      <th className="px-4 py-2">结束时间</th>
                      <th className="px-4 py-2">操作</th>
                    </tr>
                  </thead>
                  <tbody>
                    {selectedTask.sourceVideos.length === 0 ? (
                      <tr>
                        <td className="px-4 py-3 text-[var(--muted)]" colSpan={5}>
                          暂无源视频
                        </td>
                      </tr>
                    ) : (
                      selectedTask.sourceVideos.map((item, index) => (
                        <tr key={item.id} className="border-t border-black/5">
                          <td className="px-4 py-2 text-[var(--muted)]">{index + 1}</td>
                          <td className="px-4 py-2 text-[var(--ink)]">{item.sourceFilePath}</td>
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
                                onClick={() => openRemoteFilePickerForMerged(item)}
                                disabled={
                                  bindingRemoteFile ||
                                  deleteMergedSubmitting ||
                                  mergedReorderSubmitting ||
                                  mergedBindSubmitting
                                }
                              >
                                {bindingThisItem
                                  ? "绑定中"
                                  : remotePath === "-"
                                    ? "绑定网盘文件"
                                    : "修改绑定"}
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
                    </tr>
                  </thead>
                  <tbody>
                    {selectedTask.mergedVideos.length === 0 ? (
                      <tr>
                        <td className="px-4 py-3 text-[var(--muted)]" colSpan={5}>
                          暂无合并视频
                        </td>
                      </tr>
                    ) : (
                      selectedTask.mergedVideos.map((item, index) => {
                        const progress = formatUploadProgress(item.uploadProgress);
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
      ) : null}
      {mergedBindDialogOpen ? (
        <div className="fixed inset-0 z-[62] flex items-center justify-center bg-black/40 px-4">
          <div className="w-full max-w-lg rounded-2xl bg-white p-5 shadow-lg">
            <div className="text-sm font-semibold text-[var(--ink)]">重新绑定源视频</div>
            <div className="mt-2 text-xs text-[var(--muted)]">
              勾选后点击提交完成重绑定，至少保留一个源视频绑定。
            </div>
            <div className="mt-3 rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-xs text-[var(--ink)]">
              合并视频：{mergedBindTarget?.fileName || `ID:${mergedBindTarget?.mergedId || "-"}`}
            </div>
            <div className="mt-3 max-h-64 space-y-2 overflow-auto rounded-lg border border-black/10 bg-white/80 p-3">
              {detailSourceOptionList.length === 0 ? (
                <div className="text-xs text-[var(--muted)]">暂无可选源视频</div>
              ) : (
                detailSourceOptionList.map((source) => {
                  const checked = mergedBindSelection.has(source.id);
                  return (
                    <label key={source.id} className="flex items-start gap-2 text-xs">
                      <input
                        type="checkbox"
                        className="mt-0.5"
                        checked={checked}
                        onChange={(event) => toggleMergedBindSelection(source.id, event.target.checked)}
                        disabled={mergedBindSubmitting}
                      />
                      <div className="min-w-0">
                        <div className="truncate text-[var(--ink)]">{source.label}</div>
                        <div className="truncate text-[var(--muted)]">{source.path || "-"}</div>
                      </div>
                    </label>
                  );
                })
              )}
            </div>
            {mergedBindMessage ? (
              <div className="mt-3 rounded-lg border border-red-200 bg-red-50 px-3 py-2 text-xs text-red-700">
                {mergedBindMessage}
              </div>
            ) : null}
            <div className="mt-4 flex justify-end gap-2">
              <button
                className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                onClick={closeMergedBindDialog}
                disabled={mergedBindSubmitting}
              >
                取消
              </button>
              <button
                className="rounded-full bg-[var(--accent)] px-3 py-1 text-xs font-semibold text-white disabled:cursor-not-allowed disabled:opacity-60"
                onClick={handleConfirmMergedBindDialog}
                disabled={mergedBindSubmitting || detailSourceOptionList.length === 0}
              >
                {mergedBindSubmitting ? "提交中" : "提交"}
              </button>
            </div>
          </div>
        </div>
      ) : null}
      {coverPreviewModalOpen ? (
        <div
          className="fixed inset-0 z-[70] flex items-center justify-center bg-black/70 px-4 py-6"
          onClick={closeCoverPreviewModal}
        >
          <div
            className="relative max-h-full max-w-[96vw]"
            onClick={(event) => event.stopPropagation()}
          >
            <button
              type="button"
              className="absolute right-2 top-2 rounded-full border border-white/20 bg-black/40 px-3 py-1 text-xs font-semibold text-white"
              onClick={closeCoverPreviewModal}
            >
              关闭
            </button>
            <img
              src={coverPreviewModalSrc}
              alt="封面大图"
              className="max-h-[90vh] max-w-[96vw] rounded-lg object-contain shadow-2xl"
            />
          </div>
        </div>
      ) : null}
      {editSubmitConfirmOpen ? (
        <div className="fixed inset-0 z-[65] flex items-center justify-center bg-black/30 px-4">
          <div className="w-full max-w-md rounded-2xl bg-white p-5 shadow-lg">
            <div className="text-sm font-semibold text-[var(--ink)]">确认提交修改</div>
            <div className="mt-2 text-xs text-[var(--muted)]">
              本次将提交：{pendingEditChangedLabel || "无"}。
            </div>
            {pendingEditNeedRemote ? (
              <label className="mt-4 flex items-start gap-2 text-sm">
                <input
                  type="checkbox"
                  className="mt-0.5"
                  checked={editSubmitConfirmSyncRemote}
                  onChange={(event) => setEditSubmitConfirmSyncRemote(event.target.checked)}
                  disabled={submittingEdit}
                />
                <div>
                  <div className="font-semibold text-[var(--ink)]">是否同步更新远程</div>
                  <div className="text-xs text-[var(--muted)]">
                    {!canSyncRemoteEdit
                      ? editSubmitConfirmSyncRemote
                        ? "当前任务暂无BVID，若继续提交将被拦截；取消勾选后可仅保存本地数据。"
                        : "当前任务暂无BVID，取消勾选后将仅保存本地数据，不会调用B站远程编辑接口。"
                      : editSubmitConfirmSyncRemote
                      ? "勾选后将同步调用B站远程编辑接口。"
                      : "不勾选时仅更新本地数据，后续重投稿/重分段时再同步。"}
                  </div>
                </div>
              </label>
            ) : (
              <div className="mt-4 rounded-lg border border-black/10 bg-black/5 px-3 py-2 text-xs text-[var(--muted)]">
                本次仅更新源视频配置，只会保存本地数据。
              </div>
            )}
            <div className="mt-4 flex justify-end gap-2">
              <button
                className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)] disabled:cursor-not-allowed disabled:opacity-60"
                type="button"
                onClick={handleCloseEditSubmitConfirm}
                disabled={submittingEdit}
              >
                取消
              </button>
              <button
                className="rounded-full bg-[var(--accent)] px-3 py-1 text-xs font-semibold text-white disabled:cursor-not-allowed disabled:opacity-60"
                type="button"
                onClick={handleConfirmEditSubmit}
                disabled={submittingEdit}
              >
                {submittingEdit ? "提交中" : "确认提交"}
              </button>
            </div>
          </div>
        </div>
      ) : null}
      <CoverCropModal
        open={coverCropOpen}
        imageSrc={coverCropSourceUrl}
        aspectRatio={coverAspectRatio}
        minWidth={coverMinWidth}
        minHeight={coverMinHeight}
        submitting={coverUploading}
        onClose={handleCloseCoverCrop}
        onImageLoadError={handleCoverCropImageError}
        onConfirm={handleConfirmCoverCrop}
      />
      <BaiduSyncPathPicker
        open={syncPickerOpen}
        value={resolveSyncPath(syncTarget)}
        onConfirm={handleConfirmSyncPicker}
        onClose={handleCloseSyncPicker}
        onChange={handleSyncPathChange}
      />
      <BaiduRemoteFilePicker
        open={remoteFilePickerOpen}
        initialPath={remoteFilePickerPath}
        onClose={closeRemoteFilePicker}
        onConfirm={handleConfirmRemoteFileBinding}
      />
    </div>
  );
}
