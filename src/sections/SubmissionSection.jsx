import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import LoadingButton from "../components/LoadingButton";
import { showErrorDialog } from "../lib/dialog";
import { invokeCommand } from "../lib/tauri";
import { formatDateTime, parseVideoInput } from "../lib/format";
import BaiduSyncPathPicker from "../components/BaiduSyncPathPicker";
import BaiduRemoteFilePicker from "../components/BaiduRemoteFilePicker";
import CoverCropModal from "../components/CoverCropModal";
import RemoteImportTaskView from "../components/RemoteImportTaskView";
import useSubmissionCover from "../hooks/useSubmissionCover";
import useAppDataChanged from "../lib/useAppDataChanged";
import { validateSourceVideos } from "../lib/submissionEditDiff";
import {
  buildSourceId,
  buildMergeGroupId,
  buildCollectionTreeValue,
  parseCollectionTreeValue,
  emptySource,
  insertSourceBelow,
  insertMergeItemClone,
  defaultWorkflowConfig,
  buildMergeItemsFromSources,
  formatSourceBindingSummary,
  isDownloadUrlInput,
  resolveSourceRemoteInput,
  normalizeUploadPartTitle,
  resolveUploadPartTitleMode,
  buildSourceVideoPayload,
  resolveUploadPartTitles,
  sanitizeLocalFileName,
  parseDirectVideoUrl,
  isDirectSourceInput,
} from "./submission/helpers";
import SubmissionListView from "./submission/SubmissionListView";
import SubmissionDetailView from "./submission/SubmissionDetailView";

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
  
  
  
  
  
  
  const [submissionView, setSubmissionView] = useState("list");
  const [remoteImportDialogOpen, setRemoteImportDialogOpen] = useState(false);
  const [remoteImportInput, setRemoteImportInput] = useState("");
  const [remoteImportLoading, setRemoteImportLoading] = useState(false);
  const [remoteImportPreview, setRemoteImportPreview] = useState(null);
  const [sourcePartPickerOpen, setSourcePartPickerOpen] = useState(false);
  const [sourcePartPickerLoadingId, setSourcePartPickerLoadingId] = useState("");
  const [sourcePartPickerTarget, setSourcePartPickerTarget] = useState("create");
  const [sourcePartPickerSourceId, setSourcePartPickerSourceId] = useState("");
  const [sourcePartPickerTitle, setSourcePartPickerTitle] = useState("");
  const [sourcePartPickerBvid, setSourcePartPickerBvid] = useState("");
  const [sourcePartPickerAid, setSourcePartPickerAid] = useState(0);
  const [sourcePartPickerPages, setSourcePartPickerPages] = useState([]);
  const [sourcePartPickerError, setSourcePartPickerError] = useState("");
  const [sourceRemotePickerOpen, setSourceRemotePickerOpen] = useState(false);
  const [sourceRemotePickerTarget, setSourceRemotePickerTarget] = useState("create");
  const [sourceRemotePickerIndex, setSourceRemotePickerIndex] = useState(-1);
  
  
  
  
  
  
  
  
  
  
  
  
  
  
  
  
  const [quickFillOpen, setQuickFillOpen] = useState(false);
  const [quickFillTasks, setQuickFillTasks] = useState([]);
  const [quickFillPage, setQuickFillPage] = useState(1);
  const [quickFillTotal, setQuickFillTotal] = useState(0);
  const [quickFillSearch, setQuickFillSearch] = useState("");
  
  
  
  
  
  
  
  
  
  
  
  
  
  
  
  
  
  
  
  
  
  const [transcodeRepostBaiduSync, setTranscodeRepostBaiduSync] = useState({
    enabled: false,
    path: "",
    filename: "",
  });
  const [createSubmitting, setCreateSubmitting] = useState(false);
  
  
  const normalizedActiveBilibiliUid = String(activeBilibiliUid || "").trim();

  
  
  
  const [repostBaiduSync, setRepostBaiduSync] = useState({
    enabled: false,
    path: "",
    filename: "",
  });

  const [defaultBaiduSyncPath, setDefaultBaiduSyncPath] = useState("/录播");
  const [updateTaskId, setUpdateTaskId] = useState("");
  const [updateTaskDir, setUpdateTaskDir] = useState("");
  const [updateSourceVideos, setUpdateSourceVideos] = useState([emptySource(0)]);
  const [updateMergeItems, setUpdateMergeItems] = useState(() =>
    buildMergeItemsFromSources([emptySource(0)]),
  );
  const [updateMergeSelection, setUpdateMergeSelection] = useState(() => new Set());
  const [updateSegmentationEnabled, setUpdateSegmentationEnabled] = useState(true);
  const [updateWorkflowConfig, setUpdateWorkflowConfig] = useState(defaultWorkflowConfig);
  const [updateSegmentPrefix, setUpdateSegmentPrefix] = useState("");
  const [updateBaiduSync, setUpdateBaiduSync] = useState({
    enabled: false,
    path: "",
    filename: "",
  });
  const [updateImportOpen, setUpdateImportOpen] = useState(false);
  const [updateImportInput, setUpdateImportInput] = useState("");
  const [updateImportLoading, setUpdateImportLoading] = useState(false);
  const [updateImportResults, setUpdateImportResults] = useState([]);
  const [updateImportSelection, setUpdateImportSelection] = useState(() => new Set());
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
  
  const [segmentBindingSelection, setSegmentBindingSelection] = useState(() => new Set());
  const [segmentBindingMergedId, setSegmentBindingMergedId] = useState("");
  const [segmentBindingSubmitting, setSegmentBindingSubmitting] = useState(false);
  
  
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
  const [draggingSourceRowId, setDraggingSourceRowId] = useState("");
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
  const sourceDragStateRef = useRef({ activeId: "", overId: "" });
  const mergedDragStateRef = useRef({ activeId: "", overId: "" });
  const mergeDragStateRef = useRef({ activeId: "", overId: "" });
  const mergeGroupDragStateRef = useRef({ groupId: "", activeId: "", overId: "" });
  const coverProxyCacheRef = useRef(new Map());
  const activityRequestSeqRef = useRef(0);
  const isCreateView = submissionView === "create";
  const isDetailView = submissionView === "detail";
  const isEditView = submissionView === "edit";
  const isUpdateView = submissionView === "update";
  const isRemoteImportView = submissionView === "remoteImport";
  const canModifySelectedTask = isEditView;
  const isReadOnly = isDetailView;
  const selectedTaskHasBvid = Boolean(String(selectedTask?.task?.bvid || "").trim());
  const canSyncRemoteEdit = selectedTaskHasBvid;
  const groupedSourceIds = buildGroupedSourceIdSet(mergeItems);
  const uploadTitleHints = resolveUploadPartTitles(mergeItems, sourceVideos);
  const quickFillPageSize = 10;
  
  
  
  
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

  
  
  const resetUpdateState = () => {
    const initialSources = [emptySource(0)];
    setUpdateTaskId("");
    setUpdateTaskDir("");
    setUpdateSourceVideos(initialSources);
    setUpdateMergeItems(buildMergeItemsFromSources(initialSources));
    setUpdateMergeSelection(new Set());
    setUpdateSegmentationEnabled(true);
    setUpdateWorkflowConfig(defaultWorkflowConfig);
    setUpdateSegmentPrefix("");
    setUpdateBaiduSync({ enabled: false, path: "", filename: "" });
    setUpdateImportOpen(false);
    setUpdateImportInput("");
    setUpdateImportLoading(false);
    setUpdateImportResults([]);
    setUpdateImportSelection(new Set());
    setUpdateSubmitting(false);
  };

  
  
  
  
  const handleRemoteImportSaved = async (result) => {
    setRemoteImportPreview(null);
    setSubmissionView("list");
    setMessage(`远程导入成功，任务ID：${result?.taskId || "-"}`);
    await loadTasks(statusFilter, currentPage, pageSize);
  };

  const closeSourcePartPicker = () => {
    if (sourcePartPickerLoadingId) {
      return;
    }
    setSourcePartPickerOpen(false);
    setSourcePartPickerTarget("create");
    setSourcePartPickerSourceId("");
    setSourcePartPickerTitle("");
    setSourcePartPickerBvid("");
    setSourcePartPickerAid(0);
    setSourcePartPickerPages([]);
    setSourcePartPickerError("");
  };

  const buildIntegratedSourceFilePath = (taskDir, preview, page) => {
    const bvid = String(preview?.bvid || "").trim();
    const pageNo = Number(page?.page || 0) || 1;
    const cid = Number(page?.cid || 0) || 0;
    const rawTitle =
      String(page?.partName || page?.part_title || preview?.title || "").trim() ||
      `${bvid || "video"}_P${pageNo}`;
    const safeTitle = sanitizeLocalFileName(rawTitle) || `${bvid || "video"}_P${pageNo}`;
    const fileName = `${bvid || "video"}_P${pageNo}_${cid}_${safeTitle}.mp4`;
    const normalizedTaskDir = String(taskDir || "").trim();
    if (!normalizedTaskDir) {
      return fileName;
    }
    return `${normalizedTaskDir}/integrated_sources/${fileName}`;
  };

  const resolveSourceBindingFilePath = async (preview, page) => {
    if (sourcePartPickerTarget === "update") {
      return buildIntegratedSourceFilePath(updateTaskDir, preview, page);
    }
    if (isEditView) {
      const taskId = String(selectedTask?.task?.taskId || "").trim();
      if (taskId) {
        try {
          const taskDir = await invokeCommand("submission_task_dir", { taskId });
          return buildIntegratedSourceFilePath(taskDir, preview, page);
        } catch (_) {
          return buildIntegratedSourceFilePath("", preview, page);
        }
      }
    }
    return buildIntegratedSourceFilePath("", preview, page);
  };

  const applySourceRemoteBinding = (sourceId, preview, page = null, sourceFilePath = "") => {
    const normalizedBvid = String(preview?.bvid || "").trim();
    const normalizedAid = Number(preview?.aid || 0) || 0;
    const normalizedCid = Number(page?.cid || 0) || 0;
    const normalizedPartTitle = String(
      page?.partName || page?.part_title || "",
    ).trim();
    const normalizedDurationSeconds = Number(page?.durationSeconds || page?.duration || 0) || 0;
    const applyBinding = (items) =>
      items.map((item) => {
        if (item.localId !== sourceId) {
          return item;
        }
        const nextPath = String(sourceFilePath || "").trim() || item.sourceFilePath || "";
        const nextStartTime = item.startTime || "00:00:00";
        const shouldFillEndTime =
          normalizedDurationSeconds > 0 &&
          (!String(item.endTime || "").trim() || item.endTime === "00:00:00");
        return {
          ...item,
          sourceFilePath: nextPath,
          remoteVideoUrl: normalizedBvid ? item.remoteVideoUrl || "" : item.remoteVideoUrl || "",
          remoteBvid: normalizedBvid || item.remoteBvid || "",
          remoteAid: normalizedAid,
          remoteCid: normalizedCid,
          remotePartTitle: normalizedPartTitle,
          durationSeconds: normalizedDurationSeconds || item.durationSeconds || 0,
          startTime: nextStartTime,
          endTime: shouldFillEndTime ? formatDurationHms(normalizedDurationSeconds) : item.endTime,
        };
      });
    if (sourcePartPickerTarget === "update") {
      setUpdateSourceVideos((prev) => applyBinding(prev));
      return;
    }
    setSourceVideos((prev) => applyBinding(prev));
  };

  const handleChooseSourceRemotePart = async (target, sourceIndex) => {
    const isUpdateTarget = target === "update";
    const sourceList = isUpdateTarget ? updateSourceVideos : sourceVideos;
    const source = sourceList[sourceIndex];
    if (!source) {
      return;
    }
    const input = resolveSourceRemoteInput(source);
    if (!input) {
      setMessage("请先输入源视频BV号或下载链接");
      return;
    }
    if (isDirectSourceInput(input)) {
      setMessage("当前输入为第三方直链，请点击保存（将自动探测时长）");
      return;
    }
    setSourcePartPickerLoadingId(source.localId);
    setSourcePartPickerError("");
    try {
      const preview = await invokeCommand("submission_remote_video_preview", {
        request: {
          input,
          enforceOwnerMatch: false,
        },
      });
      const pages = Array.isArray(preview?.pages)
        ? preview.pages.filter((item) => Number(item?.cid || 0) > 0)
        : [];
      setSourcePartPickerSourceId(source.localId);
      setSourcePartPickerTarget(isUpdateTarget ? "update" : "create");
      setSourcePartPickerTitle(String(preview?.title || "").trim());
      setSourcePartPickerBvid(String(preview?.bvid || input).trim());
      setSourcePartPickerAid(Number(preview?.aid || 0) || 0);
      setSourcePartPickerPages(pages);
      setSourcePartPickerOpen(true);
    } catch (error) {
      setMessage(error?.message || "查询分P失败");
    } finally {
      setSourcePartPickerLoadingId("");
    }
  };

  const handleConfirmSourcePartBinding = async (page) => {
    const targetId = String(sourcePartPickerSourceId || "").trim();
    const targetBvid = String(sourcePartPickerBvid || "").trim();
    if (!targetId || !targetBvid) {
      setSourcePartPickerError("绑定目标无效");
      return;
    }
    const preview = {
      bvid: targetBvid,
      aid: sourcePartPickerAid,
      title: sourcePartPickerTitle,
    };
    const sourceFilePath = await resolveSourceBindingFilePath(preview, page);
    applySourceRemoteBinding(
      targetId,
      preview,
      page,
      sourceFilePath,
    );
    setMessage(
      `已绑定分P：${page?.partName || page?.part_title || `CID ${page?.cid || 0}`}`,
    );
    closeSourcePartPicker();
  };

  const handleConfirmSourceBvidBinding = () => {
    const targetId = String(sourcePartPickerSourceId || "").trim();
    const targetBvid = String(sourcePartPickerBvid || "").trim();
    if (!targetId || !targetBvid) {
      setSourcePartPickerError("绑定目标无效");
      return;
    }
    applySourceRemoteBinding(targetId, {
      bvid: targetBvid,
      aid: sourcePartPickerAid,
    });
    setMessage(`已绑定BV：${targetBvid}`);
    closeSourcePartPicker();
  };

  const applySourceDownloadUrlBinding = async (target, sourceIndex) => {
    const isUpdateTarget = target === "update";
    const sourceList = isUpdateTarget ? updateSourceVideos : sourceVideos;
    const updateList = isUpdateTarget ? setUpdateSourceVideos : setSourceVideos;
    const current = sourceList[sourceIndex];
    if (!current) {
      return;
    }
    const direct = parseDirectVideoUrl(resolveSourceRemoteInput(current));
    if (!direct) {
      setMessage("请输入有效的第三方下载链接（http/https 直链）");
      return;
    }
    // 与批量导入(handleUpdateImportQuery)、B站绑定(applySourceRemoteBinding)保持一致：
    // 探测时长并在结束时间为空时自动填充，让第三方源与 B站源在剪辑/合并/投稿中一视同仁。
    setSourcePartPickerLoadingId(current.localId);
    let durationSeconds = 0;
    try {
      const duration = await invokeCommand("download_direct_duration", { url: direct.url });
      durationSeconds = Number(duration) || 0;
    } catch (_) {
      durationSeconds = 0;
    } finally {
      setSourcePartPickerLoadingId("");
    }
    updateList((prev) =>
      prev.map((item, index) => {
        if (index !== sourceIndex) {
          return item;
        }
        const shouldFillEndTime =
          durationSeconds > 0 &&
          (!String(item.endTime || "").trim() || item.endTime === "00:00:00");
        return {
          ...item,
          remoteVideoUrl: direct.url,
          remoteBvid: "",
          remoteAid: 0,
          remoteCid: 0,
          remotePartTitle: String(item.remotePartTitle || "").trim() || direct.title,
          durationSeconds: durationSeconds || item.durationSeconds || 0,
          startTime: item.startTime || "00:00:00",
          endTime: shouldFillEndTime ? formatDurationHms(durationSeconds) : item.endTime,
        };
      }),
    );
    setMessage(
      durationSeconds > 0
        ? `下载链接已保存，时长 ${formatDurationHms(durationSeconds)}`
        : "下载链接已保存（未能探测时长，请手动填写结束时间）",
    );
  };

  
  const closeUpdateModal = () => {
    setSubmissionView("list");
    resetUpdateState();
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
    if (submissionView === "update") {
      resetUpdateState();
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

  const loadPartitions = async (bilibiliUidOverride = "") => {
    try {
      const currentBilibiliUid = Number(
        bilibiliUidOverride || selectedBilibiliUid || currentUpProfile?.uid || 0,
      );
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

  const loadCollections = async (bilibiliUidOverride = "") => {
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
      const effectiveUid = String(
        bilibiliUidOverride || selectedBilibiliUid || auth?.activeAccount?.userId || profile.uid || "",
      );
      setSelectedBilibiliUid((prev) => {
        if (prev === effectiveUid) {
          return prev;
        }
        return prev || effectiveUid;
      });
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
        sections: (Array.isArray(item.sections) ? item.sections : []).map((section) => ({
          ...section,
          sectionId: section.section_id ?? section.sectionId,
        })),
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
      const effectiveUid = String(auth?.activeAccount?.userId || profile.uid || "");
      setCurrentUpProfile(profile);
      setBilibiliAccounts(Array.isArray(auth?.accounts) ? auth.accounts : []);
      setSelectedBilibiliUid((prev) => {
        return prev || effectiveUid;
      });
      return {
        auth,
        profile,
        effectiveUid,
      };
    } catch (_) {}
    return null;
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
      if (taskForm.collectionSectionId) {
        setTaskForm((prev) => ({ ...prev, collectionSectionId: "" }));
      }
      return;
    }
    const exists = collections.some(
      (collection) => String(collection.seasonId) === currentCollectionId,
    );
    if (exists) {
      const currentSectionId = String(taskForm.collectionSectionId || "").trim();
      if (!currentSectionId) {
        return;
      }
      const collection = collections.find(
        (item) => String(item.seasonId) === currentCollectionId,
      );
      const sectionExists = (collection?.sections || []).some(
        (section) => String(section.sectionId) === currentSectionId,
      );
      if (sectionExists) {
        return;
      }
    }
    setTaskForm((prev) => ({
      ...prev,
      collectionId: exists ? prev.collectionId : "",
      collectionSectionId: "",
    }));
  }, [isCreateView, collections, taskForm.collectionId, taskForm.collectionSectionId]);

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

  
  
  
  
  const resetSegmentBindingState = () => {
    setSegmentBindingSelection(new Set());
    setSegmentBindingMergedId("");
    setSegmentBindingSubmitting(false);
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
    if (target === "transcodeRepost") {
      return transcodeRepostBaiduSync.path || fallbackPath;
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
    if (target === "transcodeRepost") {
      setTranscodeRepostBaiduSync((prev) => ({ ...prev, path }));
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

  
  
  
  
  useEffect(() => {
    let disposed = false;

    const initializeSubmissionView = async () => {
      const currentProfile = await loadCurrentUpProfile();
      if (disposed) {
        return;
      }
      const effectiveUid = String(
        currentProfile?.effectiveUid || normalizedActiveBilibiliUid || selectedBilibiliUid || "",
      );
      await Promise.all([loadPartitions(effectiveUid), loadCollections(effectiveUid)]);
      if (disposed) {
        return;
      }
      await loadBaiduSyncSettings();
    };

    initializeSubmissionView();

    return () => {
      disposed = true;
    };
  }, []);

  useEffect(() => {
    if (!normalizedActiveBilibiliUid) {
      return;
    }
    setSelectedBilibiliUid((prev) =>
      prev === normalizedActiveBilibiliUid ? prev : normalizedActiveBilibiliUid,
    );
    loadPartitions(normalizedActiveBilibiliUid);
    loadCollections(normalizedActiveBilibiliUid);
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

  useAppDataChanged(
    "submission",
    () => loadTasks(statusFilter, currentPage, pageSize, false, taskSearch, sourceFilter),
    { enabled: submissionView === "list", minGapMs: 2000 },
  );

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
    return () => {
      active = false;
    };
  }, [submissionView, selectedTask?.task?.taskId, isDetailView]);

  useAppDataChanged(
    "submission",
    async () => {
      const taskId = selectedTask?.task?.taskId;
      if (!taskId) {
        return;
      }
      try {
        const detail = await fetchTaskDetail(taskId, { log: false });
        setSelectedTask((prev) => (prev?.task?.taskId === taskId ? detail : prev));
      } catch (error) {
        setMessage(error.message);
      }
    },
    { enabled: isDetailView && Boolean(selectedTask?.task?.taskId), minGapMs: 1000 },
  );

  const refreshEditUploadStatus = async () => {
    const taskId = selectedTask?.task?.taskId;
    if (!taskId) {
      return;
    }
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
      setEditSegments((prev) => mergeEditUploadStatus(prev, updates || []));
    } catch (error) {
      setMessage(error.message);
    }
  };

  const editUploadPolling = isEditView && Boolean(selectedTask?.task?.taskId) && pendingEditUploads > 0;

  useEffect(() => {
    if (editUploadPolling) {
      refreshEditUploadStatus();
    }
  }, [editUploadPolling, selectedTask?.task?.taskId]);

  useAppDataChanged("editUpload", refreshEditUploadStatus, { enabled: editUploadPolling });

  const openFileDialog = async (index) => {
    await handleSelectSourceFile(index);
  };

  const addSource = () => {
    setSourceVideos((prev) => [...prev, emptySource(prev.length)]);
  };

  const openSourceRemotePicker = (target, index) => {
    setSourceRemotePickerTarget(target);
    setSourceRemotePickerIndex(index);
    setSourceRemotePickerOpen(true);
  };

  const closeSourceRemotePicker = () => {
    setSourceRemotePickerOpen(false);
    setSourceRemotePickerIndex(-1);
  };

  const handleConfirmSourceRemoteFile = (remotePath, entry) => {
    const index = sourceRemotePickerIndex;
    const setter = sourceRemotePickerTarget === "update" ? setUpdateSourceVideos : setSourceVideos;
    if (index < 0 || !remotePath) {
      return;
    }
    setter((prev) =>
      prev.map((item, itemIndex) =>
        itemIndex === index
          ? {
              ...item,
              sourceType: "BAIDU",
              sourceFilePath: "",
              remotePath,
              remoteVideoUrl: "",
              remoteBvid: "",
              remoteAid: 0,
              remoteCid: 0,
              remotePartTitle: entry?.name || item.remotePartTitle || "",
              title: entry?.name ? String(entry.name).replace(/\.[^.]+$/, "") : item.title,
              endTime: "",
            }
          : item,
      ),
    );
    setMessage(`已选择网盘视频：${remotePath}`);
    closeSourceRemotePicker();
  };

  const duplicateSource = (index) => {
    const target = sourceVideos[index];
    if (!target) {
      return;
    }
    const nextSources = insertSourceBelow(sourceVideos, index, target);
    const clonedSource = nextSources[index + 1];
    setSourceVideos(nextSources);
    if (clonedSource?.localId && target.localId) {
      setMergeItems((prev) => insertMergeItemClone(prev, target.localId, clonedSource.localId));
    }
    setMergeSelection((prev) => {
      const next = new Set(prev);
      if (clonedSource?.localId) {
        next.delete(clonedSource.localId);
      }
      return next;
    });
  };

  const updateSource = (index, field, value) => {
    setSourceVideos((prev) =>
      prev.map((item, idx) => {
        if (idx !== index) {
          return item;
        }
        if (field === "remoteBvid") {
          if (isDownloadUrlInput(value)) {
            return {
              ...item,
              sourceType: "DIRECT",
              remoteVideoUrl: value,
              remotePath: "",
              remoteBvid: "",
              remoteAid: 0,
              remoteCid: 0,
              remotePartTitle: "",
            };
          }
          return {
            ...item,
            sourceType: "BILIBILI",
            remoteVideoUrl: "",
            remotePath: "",
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

  const applyUploadTitle = (item, value) => {
    const next = String(value || "").slice(0, 80);
    return {
      ...item,
      uploadPartTitle: next,
      uploadPartTitleMode: next.trim() ? "CUSTOM" : "AUTO",
    };
  };

  const updateSourceUploadTitle = (index, value) => {
    setSourceVideos((prev) =>
      prev.map((item, idx) => (idx === index ? applyUploadTitle(item, value) : item)),
    );
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

  const buildMergeGroupsPayload = (validSources, itemsOverride = mergeItems) => {
    const mergeItemsInput = Array.isArray(itemsOverride) ? itemsOverride : [];
    const hasGroup = mergeItemsInput.some((item) => item.type === "GROUP");
    const hasStandalone = mergeItemsInput.some(
      (item) => item.type === "SOURCE" && item.standalone,
    );
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
    for (const item of mergeItemsInput) {
      if (item.type === "GROUP") {
        const groupSources = (item.sourceIds || [])
          .map((sourceId) => sourceMap.get(sourceId))
          .filter(Boolean)
          .map((source, index) => ({
            sourceFilePath: source.sourceFilePath,
            sourceType: source.sourceType || "LOCAL",
            remotePath: source.remotePath || null,
            remoteVideoUrl: source.remoteVideoUrl || null,
            remoteAid: source.remoteAid || null,
            startTime: source.startTime || null,
            endTime: source.endTime || null,
            remoteBvid: source.remoteBvid || null,
            remoteCid: source.remoteCid || null,
            remotePartTitle: source.remotePartTitle || null,
            sortOrder: index + 1,
          }));
        if (groupSources.length > 0) {
          const groupMode =
            String(item.uploadPartTitleMode || "").trim().toUpperCase() === "CUSTOM" &&
            normalizeUploadPartTitle(item.uploadPartTitle)
              ? "CUSTOM"
              : "AUTO";
          groups.push({
            order: groups.length + 1,
            sources: groupSources,
            uploadPartTitle:
              groupMode === "CUSTOM" ? normalizeUploadPartTitle(item.uploadPartTitle) : null,
            uploadPartTitleMode: groupMode,
          });
        }
        continue;
      }
      const source = sourceMap.get(item.sourceId);
      if (!source) {
        continue;
      }
      if (item.standalone) {
        const sourceMode = resolveUploadPartTitleMode(source);
        groups.push({
          order: groups.length + 1,
          uploadPartTitle:
            sourceMode === "CUSTOM" ? normalizeUploadPartTitle(source.uploadPartTitle) : null,
          uploadPartTitleMode: sourceMode,
          sources: [
            {
              sourceFilePath: source.sourceFilePath,
              sourceType: source.sourceType || "LOCAL",
              remotePath: source.remotePath || null,
              remoteVideoUrl: source.remoteVideoUrl || null,
              remoteBvid: source.remoteBvid || null,
              remoteAid: source.remoteAid || null,
              remoteCid: source.remoteCid || null,
              remotePartTitle: source.remotePartTitle || null,
              startTime: source.startTime || null,
              endTime: source.endTime || null,
              sortOrder: 1,
            },
          ],
        });
      } else {
        remainingSources.push({
          sourceFilePath: source.sourceFilePath,
          sourceType: source.sourceType || "LOCAL",
          remotePath: source.remotePath || null,
          remoteVideoUrl: source.remoteVideoUrl || null,
          remoteBvid: source.remoteBvid || null,
          remoteAid: source.remoteAid || null,
          remoteCid: source.remoteCid || null,
          remotePartTitle: source.remotePartTitle || null,
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

  const buildUpdateImportedSourcePath = (preview, page) =>
    preview?.sourceType === "DIRECT"
      ? (() => {
          const title = sanitizeLocalFileName(
            String(page?.partName || preview?.title || "第三方视频").trim(),
          ) || "第三方视频";
          const extension = String(page?.extension || preview?.extension || "mp4")
            .trim()
            .replace(/^\./, "") || "mp4";
          const normalizedTaskDir = String(updateTaskDir || "").trim();
          const fileName = `${title}.${extension}`;
          return normalizedTaskDir
            ? `${normalizedTaskDir}/integrated_sources/${fileName}`
            : `integrated_sources/${fileName}`;
        })()
      : buildIntegratedSourceFilePath(updateTaskDir, preview, page);

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

  const closeUpdateImportModal = () => {
    if (updateImportLoading) {
      return;
    }
    setUpdateImportOpen(false);
    setUpdateImportInput("");
    setUpdateImportResults([]);
    setUpdateImportSelection(new Set());
  };

  const handleUpdateImportQuery = async () => {
    const rawInput = String(updateImportInput || "").trim();
    if (!rawInput) {
      setMessage("请输入至少一个视频链接或BVID");
      return;
    }
    const inputs = Array.from(
      new Set(
        rawInput
          .split(/[\n\r,\s]+/)
          .map((item) => item.trim())
          .filter((item) => item),
      ),
    );
    if (inputs.length === 0) {
      setMessage("请输入至少一个视频链接或BVID");
      return;
    }
    setUpdateImportLoading(true);
    setMessage("");
    try {
      const previews = [];
      for (const input of inputs) {
        const direct = parseDirectVideoUrl(input);
        if (direct) {
          let durationSeconds = 0;
          try {
            const duration = await invokeCommand("download_direct_duration", { url: direct.url });
            durationSeconds = Number(duration) || 0;
          } catch (_) {
            durationSeconds = 0;
          }
          previews.push({
            input,
            sourceType: "DIRECT",
            title: direct.title,
            bvid: "",
            aid: 0,
            url: direct.url,
            extension: direct.extension,
            pages: [
              {
                cid: 0,
                page: 1,
                partName: direct.title,
                durationSeconds,
                extension: direct.extension,
              },
            ],
          });
          continue;
        }
        const { bvid, aid } = parseVideoInput(input);
        const preview = await invokeCommand("submission_remote_video_preview", {
          request: {
            input: bvid || aid ? input : input,
            enforceOwnerMatch: false,
          },
        });
        const pages = Array.isArray(preview?.pages)
          ? preview.pages.filter((item) => Number(item?.cid || 0) > 0)
          : [];
        previews.push({
          input,
          sourceType: "BILIBILI",
          title: String(preview?.title || "").trim(),
          bvid: String(preview?.bvid || input).trim(),
          aid: Number(preview?.aid || 0) || 0,
          pages,
        });
      }
      setUpdateImportResults(previews);
      setUpdateImportSelection(
        new Set(
          previews.flatMap((preview) =>
            (preview.pages || []).map((page) =>
              preview.sourceType === "DIRECT"
                ? `DIRECT:${preview.url}`
                : `${preview.bvid}:${page.cid}`,
            ),
          ),
        ),
      );
    } catch (error) {
      setMessage(error?.message || "查询视频失败");
    } finally {
      setUpdateImportLoading(false);
    }
  };

  const toggleUpdateImportSelection = (key, checked) => {
    setUpdateImportSelection((prev) => {
      const next = new Set(prev);
      if (checked) {
        next.add(key);
      } else {
        next.delete(key);
      }
      return next;
    });
  };

  const updateImportSelectableKeys = updateImportResults.flatMap((preview) =>
    (preview.pages || [])
      .filter((page) => preview.sourceType === "DIRECT" || Number(page?.cid || 0) > 0)
      .map((page) =>
        preview.sourceType === "DIRECT" ? `DIRECT:${preview.url}` : `${preview.bvid}:${page.cid}`,
      ),
  );
  const updateImportAllSelected =
    updateImportSelectableKeys.length > 0 &&
    updateImportSelectableKeys.every((key) => updateImportSelection.has(key));
  const toggleUpdateImportSelectAll = (checked) => {
    setUpdateImportSelection(
      checked ? new Set(updateImportSelectableKeys) : new Set(),
    );
  };

  const handleApplyUpdateImportedSources = () => {
    const rows = [];
    for (const preview of updateImportResults) {
      for (const page of preview.pages || []) {
        const isDirect = preview.sourceType === "DIRECT";
        const key = isDirect ? `DIRECT:${preview.url}` : `${preview.bvid}:${page.cid}`;
        if (!updateImportSelection.has(key)) {
          continue;
        }
        const durationSeconds = Number(page?.durationSeconds || page?.duration || 0) || 0;
        rows.push({
          localId: buildSourceId(),
          sourceFilePath: buildUpdateImportedSourcePath(preview, page),
          remoteVideoUrl: isDirect ? preview.url : "",
          remoteBvid: isDirect ? "" : preview.bvid,
          remoteAid: isDirect ? null : preview.aid,
          remoteCid: isDirect ? null : Number(page?.cid || 0) || 0,
          remotePartTitle: String(page?.partName || page?.part_title || "").trim(),
          sortOrder: rows.length + 1,
          startTime: "00:00:00",
          endTime: formatDurationHms(durationSeconds > 0 ? durationSeconds : 1),
          durationSeconds,
        });
      }
    }
    if (rows.length === 0) {
      setMessage("请至少选择一个分P");
      return;
    }
    setUpdateSourceVideos((prev) => {
      const next = [
        ...prev.filter((item) => String(item.sourceFilePath || "").trim()),
        ...rows,
      ].map((item, index) => ({
        ...item,
        sortOrder: index + 1,
      }));
      const normalized = next.length > 0 ? next : [emptySource(0)];
      setUpdateMergeItems((items) => syncMergeItemsWithSources(normalized, items));
      return normalized;
    });
    setUpdateImportOpen(false);
    setUpdateImportInput("");
    setUpdateImportResults([]);
    setUpdateImportSelection(new Set());
    setMessage(`已导入 ${rows.length} 个远程分P`);
  };

  
  
  
  const applyTaskSummaryToForm = (task) => {
    if (!task) {
      return;
    }
    const partitionId = task.partitionId ?? task.partition_id;
    const collectionId = task.collectionId ?? task.collection_id;
    const collectionSectionId = task.collectionSectionId ?? task.collection_section_id;
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
      collectionSectionId: collectionSectionId ? String(collectionSectionId) : "",
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
    const collectionSectionId = task.collectionSectionId ?? task.collection_section_id;
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
      collectionSectionId: collectionSectionId ? String(collectionSectionId) : "",
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
      sourceType: item.sourceType || "LOCAL",
      sourceFilePath: item.sourceFilePath || "",
      remotePath: item.remotePath || "",
      remoteVideoUrl: item.remoteVideoUrl || "",
      remoteBvid: item.remoteBvid || "",
      remoteAid: Number(item.remoteAid || 0) || 0,
      remoteCid: Number(item.remoteCid || 0) || 0,
      remotePartTitle: item.remotePartTitle || "",
      uploadPartTitle: item.uploadPartTitle || "",
      uploadPartTitleMode:
        String(item.uploadPartTitleMode || "").trim().toUpperCase() === "CUSTOM"
          ? "CUSTOM"
          : "AUTO",
      sortOrder: index + 1,
      startTime: item.startTime || "00:00:00",
      endTime: item.endTime || "00:00:00",
      durationSeconds: 0,
    }));
    const normalizedSources = sources.length ? sources : [emptySource(0)];
    setSourceVideos(normalizedSources);
    setMergeItems(buildMergeItemsFromDetail(normalizedSources, config));
    setMergeSelection(new Set());
  };

  // 从持久化的 workflowConfig.mergeGroups 重建合并组 mergeItems（按路径+起止时间匹配回 localId）。
  // 无 mergeGroups 时退回到全部独立 SOURCE。
  const buildMergeItemsFromDetail = (normalizedSources, config) => {
    const groups = Array.isArray(config?.mergeGroups)
      ? config.mergeGroups
      : Array.isArray(config?.merge_groups)
        ? config.merge_groups
        : [];
    if (!groups.length) {
      return buildMergeItemsFromSources(normalizedSources);
    }
    const norm = (value) => String(value || "").trim();
    const used = new Set();
    const matchSource = (gs) => {
      const path = norm(gs?.sourceFilePath || gs?.source_file_path);
      const start = norm(gs?.startTime || gs?.start_time);
      const end = norm(gs?.endTime || gs?.end_time);
      const candidates = normalizedSources.filter(
        (s) => !used.has(s.localId) && norm(s.sourceFilePath) === path,
      );
      if (candidates.length === 0) {
        return null;
      }
      const exact = candidates.find(
        (s) =>
          (norm(s.startTime) === start || (!start && norm(s.startTime) === "00:00:00")) &&
          (norm(s.endTime) === end || (!end && norm(s.endTime) === "00:00:00")),
      );
      const picked = exact || candidates[0];
      used.add(picked.localId);
      return picked.localId;
    };
    const sortedGroups = [...groups].sort(
      (a, b) => Number(a?.order || 0) - Number(b?.order || 0),
    );
    const items = [];
    for (const group of sortedGroups) {
      const groupSources = Array.isArray(group?.sources)
        ? group.sources
        : Array.isArray(group?.sourceVideos)
          ? group.sourceVideos
          : [];
      const matchedIds = groupSources.map(matchSource).filter(Boolean);
      if (matchedIds.length === 0) {
        continue;
      }
      const mode =
        String(group?.uploadPartTitleMode || "").trim().toUpperCase() === "CUSTOM"
          ? "CUSTOM"
          : "AUTO";
      const title = mode === "CUSTOM" ? normalizeUploadPartTitle(group?.uploadPartTitle) : "";
      if (matchedIds.length === 1) {
        // 单源组等价于独立分组：标题以源视频自身的 uploadPartTitle 回显。
        items.push({
          id: matchedIds[0],
          type: "SOURCE",
          sourceId: matchedIds[0],
          standalone: true,
        });
      } else {
        items.push({
          id: buildMergeGroupId(),
          type: "GROUP",
          sourceIds: matchedIds,
          uploadPartTitle: title,
          uploadPartTitleMode: title ? "CUSTOM" : "AUTO",
        });
      }
    }
    for (const source of normalizedSources) {
      if (!used.has(source.localId)) {
        items.push({
          id: source.localId,
          type: "SOURCE",
          sourceId: source.localId,
          standalone: false,
        });
      }
    }
    return items;
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

  const reorderEditableSources = (sourceId, targetId) => {
    setSourceVideos((prev) => {
      const fromIndex = prev.findIndex((item) => item.localId === sourceId);
      const toIndex = prev.findIndex((item) => item.localId === targetId);
      if (fromIndex < 0 || toIndex < 0 || fromIndex === toIndex) {
        return prev;
      }
      const next = [...prev];
      const [moved] = next.splice(fromIndex, 1);
      next.splice(toIndex, 0, moved);
      return next.map((item, index) => ({ ...item, sortOrder: index + 1 }));
    });
  };

  
  const trackPointerOverEditSource = (event) => {
    const { activeId } = sourceDragStateRef.current;
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
    const row = target.closest("tr[data-edit-source-id]");
    const overId = String(row?.dataset?.editSourceId || "").trim();
    if (!overId || overId === sourceDragStateRef.current.overId) {
      return;
    }
    sourceDragStateRef.current.overId = overId;
    reorderEditableSources(activeId, overId);
  };

  const endEditSourcePointerDrag = () => {
    if (!sourceDragStateRef.current.activeId) {
      return;
    }
    sourceDragStateRef.current = { activeId: "", overId: "" };
    setDraggingSourceRowId("");
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
    if (!draggingSourceRowId) {
      return undefined;
    }
    const handleMove = (event) => {
      trackPointerOverEditSource(event);
    };
    const handleUp = () => {
      endEditSourcePointerDrag();
    };
    window.addEventListener("pointermove", handleMove);
    window.addEventListener("pointerup", handleUp);
    window.addEventListener("pointercancel", handleUp);
    return () => {
      window.removeEventListener("pointermove", handleMove);
      window.removeEventListener("pointerup", handleUp);
      window.removeEventListener("pointercancel", handleUp);
    };
  }, [draggingSourceRowId]);

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

  
  
  
  
  
  
  
  
  
  
  
  
  
  
  
  
  
  
  
  
  
  
  const formatTaskStatus = (status) => {
    switch (status) {
      case "PENDING_SUBMIT":
        return "待投稿";
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
      case "VERIFY_PENDING":
        return "待远端校验";
      case "SUBMIT_UNKNOWN":
        return "提交结果待确认";
      case "REUPLOAD_PENDING":
        return "待重新上传";
      case "FAILED":
        return "失败";
      case "CANCELLED":
        return "已取消";
      default:
        return status || "-";
    }
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
  
  
  
  const detailMergedSignature = (selectedTask?.mergedVideos || [])
    .map((item) => String(item?.id || "").trim())
    .filter((item) => item)
    .join(",");

  useEffect(() => {
    resetMergedOrderDraft();
  }, [selectedTask?.task?.taskId, detailMergedSignature]);

  
  
  
  
  
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
  const selectedCollectionOption = collectionOptions.find(
    (item) => String(item.seasonId) === String(taskForm.collectionId),
  );
  const baseCollectionSectionOptions = selectedCollectionOption?.sections || [];
  const hasCollectionSectionOption = baseCollectionSectionOptions.some(
    (section) => String(section.sectionId) === String(taskForm.collectionSectionId),
  );
  const collectionSectionOptions =
    canModifySelectedTask &&
    taskForm.collectionSectionId &&
    !hasCollectionSectionOption &&
    taskForm.collectionId
      ? [
          ...baseCollectionSectionOptions,
          {
            sectionId: taskForm.collectionSectionId,
            title: `当前子合集(${taskForm.collectionSectionId})`,
          },
        ]
      : baseCollectionSectionOptions;
  
  
  const collectionTreeValue = buildCollectionTreeValue(
    taskForm.collectionId,
    taskForm.collectionSectionId,
  );
  const handleCollectionTreeChange = (rawValue) => {
    const selection = parseCollectionTreeValue(rawValue);
    setTaskForm((prev) => ({
      ...prev,
      ...selection,
    }));
  };
  const collectionTreeSections = (collection) =>
    String(collection.seasonId) === String(taskForm.collectionId)
      ? collectionSectionOptions
      : Array.isArray(collection.sections)
        ? collection.sections
        : [];
  
  
  
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
  
  const closeCoverPreviewModal = () => {
    setCoverPreviewModalOpen(false);
  };

  
  const quickFillTotalPages = Math.max(1, Math.ceil(quickFillTotal / quickFillPageSize));
  const quickFillVisibleTasks = quickFillTasks.slice(0, quickFillPageSize);
  
  
  
  
  
  
  const sourcePartPickerModal =
    sourcePartPickerOpen && typeof document !== "undefined"
      ? createPortal(
          <div className="fixed inset-0 z-[200] flex items-center justify-center bg-black/40 px-4">
            <div className="w-full max-w-2xl rounded-2xl bg-white p-5 shadow-lg">
              <div className="flex items-center justify-between gap-3">
                <div>
                  <div className="text-sm font-semibold text-[var(--ink)]">选择源视频分P</div>
                  <div className="mt-1 text-xs text-[var(--muted)]">
                    {sourcePartPickerTitle || sourcePartPickerBvid || "-"}
                  </div>
                </div>
                <button
                  type="button"
                  className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                  onClick={closeSourcePartPicker}
                  disabled={Boolean(sourcePartPickerLoadingId)}
                >
                  关闭
                </button>
              </div>
              {sourcePartPickerError ? (
                <div className="mt-3 rounded-lg border border-red-200 bg-red-50 px-3 py-2 text-xs text-red-700">
                  {sourcePartPickerError}
                </div>
              ) : null}
              <div className="mt-4 max-h-[60vh] overflow-auto rounded-xl border border-black/10">
                <table className="w-full text-left text-sm">
                  <thead className="bg-black/5 text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
                    <tr>
                      <th className="px-4 py-2">分P</th>
                      <th className="px-4 py-2">CID</th>
                      <th className="px-4 py-2">标题</th>
                      <th className="px-4 py-2">时长</th>
                      <th className="px-4 py-2">操作</th>
                    </tr>
                  </thead>
                  <tbody>
                    {sourcePartPickerPages.length === 0 ? (
                      <tr>
                        <td className="px-4 py-3 text-[var(--muted)]" colSpan={5}>
                          未查询到分P，可直接绑定当前 BV
                        </td>
                      </tr>
                    ) : (
                      sourcePartPickerPages.map((page) => (
                        <tr
                          key={`${page.cid}-${page.page || 0}`}
                          className="border-t border-black/5"
                        >
                          <td className="px-4 py-2 text-[var(--ink)]">P{page.page || "-"}</td>
                          <td className="px-4 py-2 text-[var(--muted)]">{page.cid || "-"}</td>
                          <td className="px-4 py-2 text-[var(--ink)]">
                            {page.partName || page.part_title || "-"}
                          </td>
                          <td className="px-4 py-2 text-[var(--muted)]">
                            {formatDurationHms(Number(page.durationSeconds || page.duration || 0))}
                          </td>
                          <td className="px-4 py-2">
                            <button
                              type="button"
                              className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                              onClick={() => handleConfirmSourcePartBinding(page)}
                            >
                              绑定
                            </button>
                          </td>
                        </tr>
                      ))
                    )}
                  </tbody>
                </table>
              </div>
              <div className="mt-4 flex justify-end gap-2">
                <button
                  type="button"
                  className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                  onClick={closeSourcePartPicker}
                >
                  取消
                </button>
                <button
                  type="button"
                  className="rounded-full bg-[var(--accent)] px-3 py-1 text-xs font-semibold text-white"
                  onClick={handleConfirmSourceBvidBinding}
                >
                  直接绑定BV
                </button>
              </div>
            </div>
          </div>,
          document.body,
        )
      : null;

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
      ) : null}

      {quickFillOpen && isCreateView ? (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/30 px-4">
          <div className="w-full max-w-2xl rounded-2xl bg-white p-5 shadow-lg">
            <div className="flex items-center justify-between gap-3">
              <div className="text-sm font-semibold text-[var(--ink)]">一键填写</div>
                  <button
                    type="button"
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

      {isUpdateView ? (
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
      ) : null}
      {updateImportOpen ? (
        <div className="fixed inset-0 z-[55] flex items-center justify-center bg-black/40 px-4">
          <div className="w-full max-w-5xl rounded-2xl bg-white p-5 shadow-lg">
            <div className="flex items-center justify-between gap-3">
              <div>
                <div className="text-sm font-semibold text-[var(--ink)]">导入远程视频</div>
                <div className="mt-1 text-xs text-[var(--muted)]">
                  支持一次输入多个 BVID、B站链接或第三方视频直链，按换行、空格或逗号分隔。
                </div>
              </div>
              <button
                className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                onClick={closeUpdateImportModal}
                disabled={updateImportLoading}
              >
                关闭
              </button>
            </div>
            <div className="mt-4 grid gap-3">
              <textarea
                value={updateImportInput}
                onChange={(event) => setUpdateImportInput(event.target.value)}
                placeholder={"例如：\nBV1xx...\nhttps://www.bilibili.com/video/BV1zz...\nhttps://example.com/video.flv"}
                className="min-h-28 w-full rounded-xl border border-black/10 bg-white/80 px-3 py-3 text-sm text-[var(--ink)] focus:border-[var(--accent)] focus:outline-none"
              />
              <div className="flex justify-end gap-2">
                <button
                  className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                  onClick={closeUpdateImportModal}
                  disabled={updateImportLoading}
                >
                  取消
                </button>
                <button
                  className="rounded-full bg-[var(--accent)] px-3 py-1 text-xs font-semibold text-white disabled:cursor-not-allowed disabled:opacity-60"
                  onClick={handleUpdateImportQuery}
                  disabled={updateImportLoading}
                >
                  {updateImportLoading ? "查询中" : "查询视频"}
                </button>
              </div>
            </div>
            <div className="mt-4 max-h-[55vh] overflow-auto rounded-xl border border-black/10">
              <table className="w-full text-left text-sm">
                <thead className="bg-black/5 text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
                  <tr>
	                    <th className="px-4 py-2">
	                      <label className="flex items-center gap-2">
	                        <input
	                          type="checkbox"
	                          checked={updateImportAllSelected}
	                          onChange={(event) =>
	                            toggleUpdateImportSelectAll(event.target.checked)
	                          }
	                          disabled={updateImportSelectableKeys.length === 0}
	                          aria-label="全选导入分P"
	                        />
	                        <span>选择</span>
	                      </label>
	                    </th>
                    <th className="px-4 py-2">来源</th>
                    <th className="px-4 py-2">分P</th>
                    <th className="px-4 py-2">标题</th>
                    <th className="px-4 py-2">时长</th>
                    <th className="px-4 py-2">导入后路径</th>
                  </tr>
                </thead>
                <tbody>
                  {updateImportResults.length === 0 ? (
                    <tr>
                      <td className="px-4 py-3 text-[var(--muted)]" colSpan={6}>
                        暂无查询结果
                      </td>
                    </tr>
                  ) : (
                    updateImportResults.flatMap((preview) =>
                      (preview.pages || []).map((page) => {
	                        const isDirect = preview.sourceType === "DIRECT";
	                        const key = isDirect ? `DIRECT:${preview.url}` : `${preview.bvid}:${page.cid}`;
	                        return (
	                          <tr key={key} className="border-t border-black/5">
                            <td className="px-4 py-2">
                              <input
                                type="checkbox"
                                checked={updateImportSelection.has(key)}
                                onChange={(event) =>
                                  toggleUpdateImportSelection(key, event.target.checked)
                                }
                              />
                            </td>
	                            <td className="px-4 py-2 text-[var(--ink)]">
	                              {isDirect ? "第三方直链" : preview.bvid}
	                            </td>
	                            <td className="px-4 py-2 text-[var(--muted)]">
	                              {isDirect ? "原文件" : `P${page.page || "-"} / CID ${page.cid || "-"}`}
	                            </td>
                            <td className="px-4 py-2 text-[var(--ink)]">
                              {page.partName || page.part_title || preview.title || "-"}
                            </td>
                            <td className="px-4 py-2 text-[var(--muted)]">
                              {formatDurationHms(
                                Number(page.durationSeconds || page.duration || 0),
                              )}
                            </td>
                            <td className="px-4 py-2 text-[var(--muted)]">
                              {buildUpdateImportedSourcePath(preview, page) || "-"}
                            </td>
                          </tr>
                        );
                      }),
                    )
                  )}
                </tbody>
              </table>
            </div>
            <div className="mt-4 flex justify-end gap-2">
              <button
                className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                onClick={closeUpdateImportModal}
                disabled={updateImportLoading}
              >
                取消
              </button>
              <button
                className="rounded-full bg-[var(--accent)] px-3 py-1 text-xs font-semibold text-white disabled:cursor-not-allowed disabled:opacity-60"
                onClick={handleApplyUpdateImportedSources}
                disabled={updateImportSelection.size === 0}
              >
                导入已选分P
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

      <SubmissionListView
        applyDetailToForm={applyDetailToForm}
        bilibiliAccounts={bilibiliAccounts}
        clearEditUploadCache={clearEditUploadCache}
        currentPage={currentPage}
        currentUpProfile={currentUpProfile}
        defaultBaiduSyncPath={defaultBaiduSyncPath}
        deleteMergedLocalFile={deleteMergedLocalFile}
        deleteMergedMessage={deleteMergedMessage}
        deleteMergedOpen={deleteMergedOpen}
        deleteMergedSubmitting={deleteMergedSubmitting}
        deleteMergedTarget={deleteMergedTarget}
        extractCurrentAuthProfile={extractCurrentAuthProfile}
        fetchTaskDetail={fetchTaskDetail}
        formatTaskStatus={formatTaskStatus}
        handleOpenSyncPicker={handleOpenSyncPicker}
        initEditSegments={initEditSegments}
        isVideoFilePath={isVideoFilePath}
        lastEditTaskIdRef={lastEditTaskIdRef}
        loadBaiduSyncSettings={loadBaiduSyncSettings}
        loadCollections={loadCollections}
        loadCurrentUpProfile={loadCurrentUpProfile}
        loadPartitions={loadPartitions}
        loadTasks={loadTasks}
        message={message}
        onAuthChange={onAuthChange}
        onRefreshBaiduStatus={onRefreshBaiduStatus}
        pageSize={pageSize}
        remoteImportDialogOpen={remoteImportDialogOpen}
        remoteImportInput={remoteImportInput}
        remoteImportLoading={remoteImportLoading}
        repostBaiduSync={repostBaiduSync}
        resetCoverState={resetCoverState}
        resetSegmentBindingState={resetSegmentBindingState}
        selectedBilibiliUid={selectedBilibiliUid}
        selectedTask={selectedTask}
        selectedTaskIds={selectedTaskIds}
        setActivityDropdownOpen={setActivityDropdownOpen}
        setActivityKeyword={setActivityKeyword}
        setActivityMessage={setActivityMessage}
        setActivityOptions={setActivityOptions}
        setBilibiliAccounts={setBilibiliAccounts}
        setCurrentPage={setCurrentPage}
        setCurrentUpProfile={setCurrentUpProfile}
        setDeleteMergedLocalFile={setDeleteMergedLocalFile}
        setDeleteMergedMessage={setDeleteMergedMessage}
        setDeleteMergedOpen={setDeleteMergedOpen}
        setDeleteMergedSubmitting={setDeleteMergedSubmitting}
        setDeleteMergedTarget={setDeleteMergedTarget}
        setDetailLoading={setDetailLoading}
        setDetailTab={setDetailTab}
        setDraggingMergeGroupSource={setDraggingMergeGroupSource}
        setDraggingMergeItemId={setDraggingMergeItemId}
        setEditBaseline={setEditBaseline}
        setEditSegments={setEditSegments}
        setEditSubmitConfirmOpen={setEditSubmitConfirmOpen}
        setEditSubmitConfirmSyncRemote={setEditSubmitConfirmSyncRemote}
        setMergeItems={setMergeItems}
        setMergeSelection={setMergeSelection}
        setMessage={setMessage}
        setPageSize={setPageSize}
        setPendingEditChangedLabel={setPendingEditChangedLabel}
        setPendingEditNeedRemote={setPendingEditNeedRemote}
        setPendingEditSubmitPayload={setPendingEditSubmitPayload}
        setQuickFillOpen={setQuickFillOpen}
        setRemoteImportDialogOpen={setRemoteImportDialogOpen}
        setRemoteImportInput={setRemoteImportInput}
        setRemoteImportLoading={setRemoteImportLoading}
        setRemoteImportPreview={setRemoteImportPreview}
        setRepostBaiduSync={setRepostBaiduSync}
        setSegmentBindingMergedId={setSegmentBindingMergedId}
        setSegmentBindingSelection={setSegmentBindingSelection}
        setSegmentBindingSubmitting={setSegmentBindingSubmitting}
        setSegmentationEnabled={setSegmentationEnabled}
        setSelectedBilibiliUid={setSelectedBilibiliUid}
        setSelectedTask={setSelectedTask}
        setSelectedTaskIds={setSelectedTaskIds}
        setSourceFilter={setSourceFilter}
        setSourceVideos={setSourceVideos}
        setStatusFilter={setStatusFilter}
        setSubmissionView={setSubmissionView}
        setTagInput={setTagInput}
        setTags={setTags}
        setTaskForm={setTaskForm}
        setTaskSearch={setTaskSearch}
        setTasks={setTasks}
        setTranscodeRepostBaiduSync={setTranscodeRepostBaiduSync}
        setUpdateBaiduSync={setUpdateBaiduSync}
        setUpdateImportInput={setUpdateImportInput}
        setUpdateImportLoading={setUpdateImportLoading}
        setUpdateImportOpen={setUpdateImportOpen}
        setUpdateImportResults={setUpdateImportResults}
        setUpdateImportSelection={setUpdateImportSelection}
        setUpdateMergeItems={setUpdateMergeItems}
        setUpdateMergeSelection={setUpdateMergeSelection}
        setUpdateSegmentPrefix={setUpdateSegmentPrefix}
        setUpdateSegmentationEnabled={setUpdateSegmentationEnabled}
        setUpdateSourceVideos={setUpdateSourceVideos}
        setUpdateSubmitting={setUpdateSubmitting}
        setUpdateTaskDir={setUpdateTaskDir}
        setUpdateTaskId={setUpdateTaskId}
        setUpdateWorkflowConfig={setUpdateWorkflowConfig}
        setWorkflowConfig={setWorkflowConfig}
        sourceFilter={sourceFilter}
        statusFilter={statusFilter}
        submissionView={submissionView}
        taskSearch={taskSearch}
        tasks={tasks}
        totalTasks={totalTasks}
        transcodeRepostBaiduSync={transcodeRepostBaiduSync}
      />

      <SubmissionDetailView
        addSource={addSource}
        applySourceDownloadUrlBinding={applySourceDownloadUrlBinding}
        backToList={backToList}
        bindingMergedVideo={bindingMergedVideo}
        bindingRemoteFile={bindingRemoteFile}
        buildPartitionOptionValue={buildPartitionOptionValue}
        canModifySelectedTask={canModifySelectedTask}
        canSyncRemoteEdit={canSyncRemoteEdit}
        collectionOptions={collectionOptions}
        collectionSectionOptions={collectionSectionOptions}
        collectionTreeSections={collectionTreeSections}
        collectionTreeValue={collectionTreeValue}
        coverPreviewSrc={coverPreviewSrc}
        coverUploading={coverUploading}
        defaultBaiduSyncPath={defaultBaiduSyncPath}
        deleteMergedSubmitting={deleteMergedSubmitting}
        deleteMergedTarget={deleteMergedTarget}
        detailLoading={detailLoading}
        detailSourceOptionList={detailSourceOptionList}
        detailTab={detailTab}
        dragStateRef={dragStateRef}
        draggingMergedId={draggingMergedId}
        draggingSegmentId={draggingSegmentId}
        draggingSourceRowId={draggingSourceRowId}
        duplicateSource={duplicateSource}
        editBaseline={editBaseline}
        editSegments={editSegments}
        editingSegmentId={editingSegmentId}
        editingSegmentName={editingSegmentName}
        estimatedSegments={estimatedSegments}
        fetchTaskDetail={fetchTaskDetail}
        formatTaskStatus={formatTaskStatus}
        handleChooseSourceRemotePart={handleChooseSourceRemotePart}
        handleClearCover={handleClearCover}
        handleCollectionTreeChange={handleCollectionTreeChange}
        handlePartitionChange={handlePartitionChange}
        handleSelectCoverFile={handleSelectCoverFile}
        handleTagKeyDown={handleTagKeyDown}
        isActivityTag={isActivityTag}
        isDetailView={isDetailView}
        isEditView={isEditView}
        isVideoFilePath={isVideoFilePath}
        mergedBindSubmitting={mergedBindSubmitting}
        mergedDragStateRef={mergedDragStateRef}
        mergedOrderDirty={mergedOrderDirty}
        mergedOrderDraftIds={mergedOrderDraftIds}
        message={message}
        normalizeCoverPreviewSrc={normalizeCoverPreviewSrc}
        normalizeSourceTime={normalizeSourceTime}
        openFileDialog={openFileDialog}
        parseHmsToSeconds={parseHmsToSeconds}
        partitionOptions={partitionOptions}
        partitionSelectValue={partitionSelectValue}
        partitions={partitions}
        removeSource={removeSource}
        removeTag={removeTag}
        renderActivityTopicSelector={renderActivityTopicSelector}
        resetMergedOrderDraft={resetMergedOrderDraft}
        resolveTagClassName={resolveTagClassName}
        resolveTagRemoveClassName={resolveTagRemoveClassName}
        segmentBindingMergedId={segmentBindingMergedId}
        segmentBindingSelection={segmentBindingSelection}
        segmentBindingSubmitting={segmentBindingSubmitting}
        segmentationEnabled={segmentationEnabled}
        selectedCollectionOption={selectedCollectionOption}
        selectedTask={selectedTask}
        setBindingMergedVideo={setBindingMergedVideo}
        setCoverPreviewModalOpen={setCoverPreviewModalOpen}
        setCoverPreviewModalSrc={setCoverPreviewModalSrc}
        setDeleteMergedLocalFile={setDeleteMergedLocalFile}
        setDeleteMergedMessage={setDeleteMergedMessage}
        setDeleteMergedOpen={setDeleteMergedOpen}
        setDeleteMergedTarget={setDeleteMergedTarget}
        setDetailTab={setDetailTab}
        setDraggingMergedId={setDraggingMergedId}
        setDraggingSegmentId={setDraggingSegmentId}
        setDraggingSourceRowId={setDraggingSourceRowId}
        setEditSegments={setEditSegments}
        setEditSubmitConfirmOpen={setEditSubmitConfirmOpen}
        setEditSubmitConfirmSyncRemote={setEditSubmitConfirmSyncRemote}
        setEditingSegmentId={setEditingSegmentId}
        setEditingSegmentName={setEditingSegmentName}
        setMergedBindDialogOpen={setMergedBindDialogOpen}
        setMergedBindMessage={setMergedBindMessage}
        setMergedBindSelection={setMergedBindSelection}
        setMergedBindTarget={setMergedBindTarget}
        setMergedOrderDirty={setMergedOrderDirty}
        setMessage={setMessage}
        setPendingEditChangedLabel={setPendingEditChangedLabel}
        setPendingEditNeedRemote={setPendingEditNeedRemote}
        setPendingEditSubmitPayload={setPendingEditSubmitPayload}
        setRemoteFilePickerOpen={setRemoteFilePickerOpen}
        setRemoteFilePickerPath={setRemoteFilePickerPath}
        setSegmentBindingMergedId={setSegmentBindingMergedId}
        setSegmentBindingSelection={setSegmentBindingSelection}
        setSegmentBindingSubmitting={setSegmentBindingSubmitting}
        setSelectedTask={setSelectedTask}
        setTagInput={setTagInput}
        setTaskForm={setTaskForm}
        sourceDragStateRef={sourceDragStateRef}
        sourcePartPickerLoadingId={sourcePartPickerLoadingId}
        sourceVideos={sourceVideos}
        submittingEdit={submittingEdit}
        tagInput={tagInput}
        tags={tags}
        taskForm={taskForm}
        updateSource={updateSource}
        updateSourceTime={updateSourceTime}
        updateSourceUploadTitle={updateSourceUploadTitle}
        uploadTitleHints={uploadTitleHints}
        workflowConfig={workflowConfig}
      />
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
      {sourcePartPickerModal}
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
      <BaiduRemoteFilePicker
        open={sourceRemotePickerOpen}
        initialPath="/"
        onClose={closeSourceRemotePicker}
        onConfirm={handleConfirmSourceRemoteFile}
      />
    </div>
  );
}
