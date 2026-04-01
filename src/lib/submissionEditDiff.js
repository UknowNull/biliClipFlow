function normalizeText(value) {
  return String(value ?? "").trim();
}

export function normalizeTagCollection(items) {
  return Array.from(
    new Set(
      (items || [])
        .map((item) => String(item || "").trim())
        .filter((item) => item),
    ),
  ).sort();
}

export function collectCurrentTags(tags, tagInput) {
  const current = [...(tags || [])];
  const pending = String(tagInput || "").trim();
  if (pending) {
    current.push(pending);
  }
  return normalizeTagCollection(current);
}

export function buildBasicSnapshot(form, tagList) {
  return {
    title: normalizeText(form.title),
    description: normalizeText(form.description),
    coverUrl: normalizeText(form.coverUrl),
    coverDataReady: normalizeText(form.coverDataUrl) ? "1" : "0",
    partitionId: normalizeText(form.partitionId),
    collectionId: normalizeText(form.collectionId),
    activityTopicId: normalizeText(form.activityTopicId),
    activityMissionId: normalizeText(form.activityMissionId),
    activityTitle: normalizeText(form.activityTitle),
    videoType: normalizeText(form.videoType || "ORIGINAL"),
    segmentPrefix: normalizeText(form.segmentPrefix),
    tags: normalizeTagCollection(tagList).join(","),
  };
}

export function buildSourceSnapshot(items) {
  return (items || [])
    .filter((item) => normalizeText(item.sourceFilePath))
    .map((item, index) => ({
      sourceFilePath: normalizeText(item.sourceFilePath),
      remoteBvid: normalizeText(item.remoteBvid),
      sortOrder: index + 1,
      startTime: normalizeText(item.startTime || "00:00:00"),
      endTime: normalizeText(item.endTime || "00:00:00"),
    }));
}

export function buildSegmentSnapshot(items) {
  return (items || []).map((item, index) => ({
    segmentId: normalizeText(item.segmentId),
    partName: normalizeText(item.partName),
    segmentFilePath: normalizeText(item.segmentFilePath),
    partOrder: Number(item.partOrder || index + 1),
    cid: Number(item.cid || 0),
    fileName: normalizeText(item.fileName),
    uploadStatus: normalizeText(item.uploadStatus),
  }));
}

export function buildEditBaselineFromDetail(detail) {
  const task = detail?.task || {};
  const baselineTags = String(task.tags || "")
    .split(",")
    .map((item) => item.trim())
    .filter((item) => item);
  return {
    basic: buildBasicSnapshot(
      {
        title: task.title || "",
        description: task.description || "",
        coverUrl: task.coverUrl ?? task.cover_url ?? "",
        partitionId: task.partitionId ?? task.partition_id ?? "",
        collectionId: task.collectionId ?? task.collection_id ?? "",
        activityTopicId: task.topicId ?? task.topic_id ?? "",
        activityMissionId: task.missionId ?? task.mission_id ?? "",
        activityTitle: task.activityTitle ?? task.activity_title ?? "",
        videoType: task.videoType ?? task.video_type ?? "ORIGINAL",
        segmentPrefix: task.segmentPrefix ?? task.segment_prefix ?? "",
      },
      baselineTags,
    ),
    source: buildSourceSnapshot(detail?.sourceVideos || []),
    segments: buildSegmentSnapshot(detail?.outputSegments || []),
  };
}

export function resolveEditChangedModules({
  baseline,
  taskForm,
  tags,
  tagInput,
  sourceVideos,
  editSegments,
}) {
  if (!baseline) {
    return [];
  }
  const current = {
    basic: buildBasicSnapshot(taskForm, collectCurrentTags(tags, tagInput)),
    source: buildSourceSnapshot(sourceVideos),
    segments: buildSegmentSnapshot(editSegments),
  };
  const changed = [];
  if (JSON.stringify(current.basic) !== JSON.stringify(baseline.basic)) {
    changed.push("basic");
  }
  if (JSON.stringify(current.source) !== JSON.stringify(baseline.source)) {
    changed.push("source");
  }
  if (JSON.stringify(current.segments) !== JSON.stringify(baseline.segments)) {
    changed.push("segments");
  }
  return changed;
}

export function validateSourceVideos({ items, parseHmsToSeconds, isVideoFilePath }) {
  const validSources = (items || [])
    .filter((item) => normalizeText(item.sourceFilePath))
    .map((item) => ({
      ...item,
      sourceFilePath: normalizeText(item.sourceFilePath),
      remoteBvid: normalizeText(item.remoteBvid),
      startTime: normalizeText(item.startTime || "00:00:00"),
      endTime: normalizeText(item.endTime || "00:00:00"),
    }));
  if (validSources.length === 0) {
    return { valid: false, message: "请至少添加一个源视频", validSources: [] };
  }
  const invalidSource = validSources.find((item) => !isVideoFilePath(item.sourceFilePath));
  if (invalidSource) {
    return { valid: false, message: "源视频仅支持常见视频格式", validSources: [] };
  }
  const invalidTime = validSources.find((item) => {
    const start = parseHmsToSeconds(item.startTime);
    const end = parseHmsToSeconds(item.endTime);
    if (start === null || end === null) {
      return true;
    }
    if (start < 0 || end <= 0 || start >= end) {
      return true;
    }
    if (item.durationSeconds > 0 && end > item.durationSeconds) {
      return true;
    }
    return false;
  });
  if (invalidTime) {
    return { valid: false, message: "时间范围不合法，请检查开始与结束时间", validSources: [] };
  }
  return { valid: true, message: "", validSources };
}
