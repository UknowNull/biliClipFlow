export const statusFilters = [
  { value: "ALL", label: "全部" },
  { value: "PENDING_SUBMIT", label: "待投稿" },
  { value: "PENDING", label: "待处理" },
  { value: "CLIPPING", label: "剪辑中" },
  { value: "MERGING", label: "合并中" },
  { value: "SEGMENTING", label: "分段中" },
  { value: "RUNNING", label: "处理中" },
  { value: "WAITING_UPLOAD", label: "投稿队列中" },
  { value: "UPLOADING", label: "投稿中" },
  { value: "VERIFY_PENDING", label: "待远端校验" },
  { value: "SUBMIT_UNKNOWN", label: "提交结果待确认" },
  { value: "COMPLETED", label: "已完成" },
  { value: "FAILED", label: "失败" },
  { value: "CANCELLED", label: "已取消" },
];

export const REMOTE_STATE_TRANSCODE_FAILED = -16;

export const submissionNeedsRemoteVerification = (task) => {
  const status = String(task?.status || "").trim();
  if (["VERIFY_PENDING", "SUBMIT_UNKNOWN"].includes(status)) {
    return true;
  }
  if (status !== "FAILED") {
    return false;
  }
  if (Number(task?.remoteState) === REMOTE_STATE_TRANSCODE_FAILED) {
    return false;
  }
  const error = String(task?.workflowStatus?.errorMessage || "");
  if (error.includes("SUBMIT_RESULT_UNKNOWN:") || error.includes("SUBMISSION_CREATED:")) {
    return true;
  }
  return (
    Boolean(String(task?.bvid || "").trim()) &&
    (error.includes("code: -702") ||
      error.includes("请求频率过高") ||
      error.includes("请求过于频繁") ||
      error.includes("远程分P数量与本地不一致"))
  );
};

export const canConfirmRemoteMissing = (task) => {
  if (!submissionNeedsRemoteVerification(task) || String(task?.bvid || "").trim()) {
    return false;
  }
  const status = String(task?.status || "").trim();
  const error = String(task?.workflowStatus?.errorMessage || "");
  return status === "SUBMIT_UNKNOWN" || error.includes("SUBMIT_RESULT_UNKNOWN:");
};

export const canReuploadAllSegments = (task) => {
  const status = String(task?.status || "").trim();
  const error = String(task?.workflowStatus?.errorMessage || "");
  return (
    status === "FAILED" &&
    !String(task?.bvid || "").trim() &&
    !Number(task?.aid || 0) &&
    error.includes("REUPLOAD_REQUIRED:")
  );
};

export const sourceFilters = [
  { value: "NORMAL", label: "其他投稿" },
  { value: "CLIP", label: "切片投稿" },
];

export const buildSourceId = () =>
  `source_${Math.random().toString(36).slice(2, 10)}_${Date.now().toString(36)}`;
export const buildMergeGroupId = () =>
  `group_${Math.random().toString(36).slice(2, 10)}_${Date.now().toString(36)}`;

export const buildCollectionTreeValue = (collectionId, sectionId) => {
  const normalizedCollectionId = String(collectionId || "").trim();
  if (!normalizedCollectionId) {
    return "";
  }
  const normalizedSectionId = String(sectionId || "").trim();
  return normalizedSectionId
    ? `section:${normalizedCollectionId}:${normalizedSectionId}`
    : `collection:${normalizedCollectionId}`;
};

export const parseCollectionTreeValue = (rawValue) => {
  const [type, collectionId = "", sectionId = ""] = String(rawValue || "").split(":");
  if (!collectionId || !["collection", "section"].includes(type)) {
    return { collectionId: "", collectionSectionId: "" };
  }
  return {
    collectionId,
    collectionSectionId: type === "section" ? sectionId : "",
  };
};

export const emptySource = (index) => ({
  localId: buildSourceId(),
  sourceType: "LOCAL",
  sourceFilePath: "",
  remotePath: "",
  remoteVideoUrl: "",
  remoteBvid: "",
  remoteAid: 0,
  remoteCid: 0,
  remotePartTitle: "",
  uploadPartTitle: "",
  uploadPartTitleMode: "AUTO",
  sortOrder: index + 1,
  startTime: "00:00:00",
  endTime: "00:00:00",
  durationSeconds: 0,
});

export const cloneSource = (source, index) => ({
  ...source,
  localId: buildSourceId(),
  sortOrder: index + 1,
});

export const insertSourceBelow = (items, index, source) => {
  const next = [...(items || [])];
  next.splice(index + 1, 0, cloneSource(source, index + 1));
  return next.map((item, idx) => ({ ...item, sortOrder: idx + 1 }));
};

export const insertMergeItemClone = (items, sourceId, clonedId) => {
  const next = [];
  let inserted = false;
  for (const item of items || []) {
    if (item.type === "GROUP" && Array.isArray(item.sourceIds) && item.sourceIds.includes(sourceId)) {
      next.push({
        ...item,
        sourceIds: item.sourceIds.flatMap((id) =>
          id === sourceId ? [id, clonedId] : [id],
        ),
      });
      inserted = true;
      continue;
    }
    next.push(item);
    if (item.type === "SOURCE" && item.sourceId === sourceId) {
      next.push({
        id: clonedId,
        type: "SOURCE",
        sourceId: clonedId,
        standalone: Boolean(item.standalone),
      });
      inserted = true;
    }
  }
  if (!inserted) {
    next.push({ id: clonedId, type: "SOURCE", sourceId: clonedId, standalone: false });
  }
  return next;
};

export const defaultWorkflowConfig = {
  segmentationConfig: {
    segmentDurationSeconds: 133,
    preserveOriginal: true,
  },
};

export const buildMergeItemsFromSources = (sources) =>
  (sources || [])
    .map((item) => item?.localId)
    .filter((id) => id)
    .map((id) => ({ id, type: "SOURCE", sourceId: id, standalone: false }));

export const formatSourceBindingSummary = (source) => {
  const remoteVideoUrl = String(source?.remoteVideoUrl || "").trim();
  if (remoteVideoUrl && !String(source?.remoteBvid || "").trim()) {
    return "已绑定下载链接";
  }
  const remoteBvid = String(source?.remoteBvid || "").trim();
  if (!remoteBvid) {
    return "";
  }
  const remoteCid = Number(source?.remoteCid || 0);
  const remotePartTitle = String(source?.remotePartTitle || "").trim();
  if (remoteCid > 0 && remotePartTitle) {
    return `已绑定 ${remotePartTitle} (CID ${remoteCid})`;
  }
  if (remoteCid > 0) {
    return `已绑定 CID ${remoteCid}`;
  }
  return "已绑定BV，未指定分P";
};

export const isDownloadUrlInput = (value) => /^https?:\/\//i.test(String(value || "").trim());
export const isBilibiliUrlInput = (value) => {
  if (!isDownloadUrlInput(value)) {
    return false;
  }
  try {
    const host = new URL(String(value || "").trim()).hostname.toLowerCase();
    return host === "bilibili.com" || host.endsWith(".bilibili.com") || host === "b23.tv";
  } catch (_) {
    return false;
  }
};
export const resolveSourceRemoteInput = (source) =>
  String(source?.remoteVideoUrl || source?.remoteBvid || "").trim();

export const normalizeUploadPartTitle = (value) =>
  String(value || "").trim().slice(0, 80);

export const resolveUploadPartTitleMode = (source) =>
  String(source?.uploadPartTitleMode || "").trim().toUpperCase() === "CUSTOM" &&
  normalizeUploadPartTitle(source?.uploadPartTitle)
    ? "CUSTOM"
    : "AUTO";

export const buildSourceVideoPayload = (source, index) => {
  const mode = resolveUploadPartTitleMode(source);
  const title = mode === "CUSTOM" ? normalizeUploadPartTitle(source.uploadPartTitle) : "";
  return {
    sourceFilePath: source.sourceFilePath,
    sourceType: source.sourceType || "LOCAL",
    remotePath: source.remotePath || null,
    remoteVideoUrl: source.remoteVideoUrl || null,
    remoteBvid: source.remoteBvid || null,
    remoteAid: source.remoteAid || null,
    remoteCid: source.remoteCid || null,
    remotePartTitle: source.remotePartTitle || null,
    uploadPartTitle: title || null,
    uploadPartTitleMode: mode,
    sortOrder: index + 1,
    startTime: source.startTime || null,
    endTime: source.endTime || null,
  };
};

// 计算各最终上传单元（合并组算一个、独立/未合并源各算一个，未合并未独立源合并为尾部一个单元）
// 的 AUTO 占位标题 P{n}，顺序与 buildMergeGroupsPayload 的单元顺序一致。
export const resolveUploadPartTitles = (items, sources) => {
  const sourceMap = new Map(
    (sources || []).filter((s) => s?.localId).map((s) => [s.localId, s]),
  );
  const groupTitleMap = new Map();
  const sourceTitleMap = new Map();
  const list = Array.isArray(items) ? items : [];
  const hasGroup = list.some((i) => i.type === "GROUP");
  const hasStandalone = list.some((i) => i.type === "SOURCE" && i.standalone);
  if (!hasGroup && !hasStandalone) {
    // 全部合并为一个单元 P1
    list.forEach((i) => {
      if (i.type === "SOURCE" && sourceMap.has(i.sourceId)) {
        sourceTitleMap.set(i.sourceId, "P1");
      }
    });
    return { groupTitleMap, sourceTitleMap };
  }
  let order = 0;
  const remaining = [];
  for (const item of list) {
    if (item.type === "GROUP") {
      const valid = (item.sourceIds || []).filter((id) => sourceMap.has(id));
      if (valid.length > 0) {
        order += 1;
        groupTitleMap.set(item.id, `P${order}`);
      }
    } else if (sourceMap.has(item.sourceId)) {
      if (item.standalone) {
        order += 1;
        sourceTitleMap.set(item.sourceId, `P${order}`);
      } else {
        remaining.push(item.sourceId);
      }
    }
  }
  if (remaining.length > 0) {
    order += 1;
    remaining.forEach((id) => sourceTitleMap.set(id, `P${order}`));
  }
  return { groupTitleMap, sourceTitleMap };
};

export const sanitizeLocalFileName = (value) =>
  String(value || "")
    .replace(/[<>:"/\\|?*\u0000-\u001F]/g, "_")
    .replace(/\s+/g, " ")
    .trim();

export const parseDirectVideoUrl = (raw) => {
  const value = String(raw || "").trim();
  if (!isDownloadUrlInput(value) || isBilibiliUrlInput(value)) {
    return null;
  }
  try {
    const parsed = new URL(value);
    const fileName = decodeURIComponent(parsed.pathname || "")
      .split("/")
      .filter(Boolean)
      .pop() || "download.mp4";
    const dotIndex = fileName.lastIndexOf(".");
    const title = dotIndex > 0 ? fileName.slice(0, dotIndex) : fileName;
    const extension = dotIndex > 0 ? fileName.slice(dotIndex + 1).toLowerCase() : "mp4";
    return {
      url: value,
      title: title || "第三方视频",
      fileName,
      extension,
    };
  } catch (_) {
    return null;
  }
};

// 是否为"第三方直链"(http/https 直链且非 B站域名)。用于把 B站网页链接正确路由到分P选择，
// 真直链路由到"保存+探测时长"。isDownloadUrlInput 对 bilibili.com/b23.tv 也为真，不能用于此判定。
export const isDirectSourceInput = (value) => Boolean(parseDirectVideoUrl(value));
