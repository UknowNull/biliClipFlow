import { useEffect, useMemo, useRef, useState } from "react";
import { invokeCommand } from "../lib/tauri";
import BaiduSyncPathPicker from "./BaiduSyncPathPicker";

function pad2(value) {
  return String(value).padStart(2, "0");
}

function formatSecondsToHms(totalSeconds) {
  const safe = Math.max(0, Number(totalSeconds) || 0);
  const hours = Math.floor(safe / 3600);
  const minutes = Math.floor((safe % 3600) / 60);
  const seconds = Math.floor(safe % 60);
  return `${pad2(hours)}:${pad2(minutes)}:${pad2(seconds)}`;
}

function sanitizeName(raw) {
  const normalized = String(raw || "")
    .trim()
    .replace(/[^a-zA-Z0-9\u4e00-\u9fa5._-]+/g, "_")
    .replace(/_+/g, "_")
    .replace(/^_+|_+$/g, "");
  return normalized || "remote_import";
}

function normalizeTimeText(value, fallback) {
  const raw = String(value || "").trim();
  if (!raw) {
    return fallback;
  }
  const match = raw.match(/^(\d{1,2}):(\d{1,2}):(\d{1,2})(?:\.\d+)?$/);
  if (!match) {
    return fallback;
  }
  const hours = Number(match[1]);
  const minutes = Number(match[2]);
  const seconds = Number(match[3]);
  if (
    !Number.isFinite(hours) ||
    !Number.isFinite(minutes) ||
    !Number.isFinite(seconds) ||
    minutes < 0 ||
    minutes > 59 ||
    seconds < 0 ||
    seconds > 59 ||
    hours < 0
  ) {
    return fallback;
  }
  return `${pad2(hours)}:${pad2(minutes)}:${pad2(seconds)}`;
}

function parseHmsToSeconds(value) {
  const raw = String(value || "").trim();
  const match = raw.match(/^(\d{1,2}):(\d{1,2}):(\d{1,2})(?:\.\d+)?$/);
  if (!match) {
    return null;
  }
  const hours = Number(match[1]);
  const minutes = Number(match[2]);
  const seconds = Number(match[3]);
  if (
    !Number.isFinite(hours) ||
    !Number.isFinite(minutes) ||
    !Number.isFinite(seconds) ||
    hours < 0 ||
    minutes < 0 ||
    minutes > 59 ||
    seconds < 0 ||
    seconds > 59
  ) {
    return null;
  }
  return hours * 3600 + minutes * 60 + seconds;
}

function resolveSourceDurationSeconds(source) {
  const duration = Number(source?.durationSeconds || 0);
  if (duration > 0) {
    return duration;
  }
  const start = parseHmsToSeconds(source?.startTime);
  const end = parseHmsToSeconds(source?.endTime);
  if (start === null || end === null || end <= start) {
    return 0;
  }
  return end - start;
}

function buildSourceFromPage(item) {
  return {
    sourceKey: String(item?.sourceKey || "").trim(),
    bvid: String(item?.bvid || "").trim(),
    cid: Number(item?.cid || 0) || 0,
    partName: String(item?.partName || "").trim() || `P${Number(item?.page || 1) || 1}`,
    durationSeconds: Number(item?.durationSeconds || 0) || 0,
    startTime: normalizeTimeText(item?.startTime, "00:00:00"),
    endTime: normalizeTimeText(
      item?.endTime,
      formatSecondsToHms(Number(item?.durationSeconds || 0)),
    ),
    sourceFilePath:
      item?.sourceFilePath ||
      `remote/${item?.bvid || "unknown"}/${Number(item?.cid || 0)}.mp4`,
    remoteFileName: String(item?.remoteFileName || "").trim(),
  };
}

function buildDefaultSources(preview) {
  const pages = Array.isArray(preview?.pages) ? preview.pages : [];
  return pages
    .map((item) => buildSourceFromPage(item))
    .filter((item) => item.sourceKey && item.cid > 0)
    .map((item, index) => ({
      ...item,
      sortOrder: index + 1,
    }));
}

function parseTagItems(raw) {
  return String(raw || "")
    .split(/[,\s，]+/)
    .map((item) => item.trim())
    .filter((item) => item.length > 0);
}

function buildMergedListFromSources(sources, previousMerged) {
  const mergedBySourceKey = new Map();
  (previousMerged || []).forEach((item) => {
    const key = Array.isArray(item?.sourceKeys) ? String(item.sourceKeys[0] || "").trim() : "";
    if (!key) {
      return;
    }
    mergedBySourceKey.set(key, item);
  });
  const baseNameCount = new Map();
  return (sources || []).map((source, index) => {
    const sourceKey = String(source?.sourceKey || "").trim();
    const previous = mergedBySourceKey.get(sourceKey);
    const baseName = sanitizeName(source?.partName || `P${index + 1}`);
    const nextCount = (baseNameCount.get(baseName) || 0) + 1;
    baseNameCount.set(baseName, nextCount);
    const suffix = nextCount > 1 ? `_${pad2(nextCount)}` : "";
    const defaultFileName = `${baseName}${suffix}.mp4`;
    const fileName = String(previous?.fileName || "").trim() || defaultFileName;
    return {
      id: previous?.id || `merged-${sourceKey}`,
      sortOrder: index + 1,
      fileName,
      remotePath: String(previous?.remotePath || "").trim(),
      sourceKeys: [sourceKey],
    };
  });
}

function isMergedListEqual(current, next) {
  if (!Array.isArray(current) || !Array.isArray(next) || current.length !== next.length) {
    return false;
  }
  for (let index = 0; index < current.length; index += 1) {
    const a = current[index];
    const b = next[index];
    if (
      String(a?.id || "") !== String(b?.id || "") ||
      Number(a?.sortOrder || 0) !== Number(b?.sortOrder || 0) ||
      String(a?.fileName || "") !== String(b?.fileName || "") ||
      String(a?.remotePath || "") !== String(b?.remotePath || "")
    ) {
      return false;
    }
    const aKeys = Array.isArray(a?.sourceKeys) ? a.sourceKeys : [];
    const bKeys = Array.isArray(b?.sourceKeys) ? b.sourceKeys : [];
    if (aKeys.length !== bKeys.length) {
      return false;
    }
    for (let keyIndex = 0; keyIndex < aKeys.length; keyIndex += 1) {
      if (String(aKeys[keyIndex] || "") !== String(bKeys[keyIndex] || "")) {
        return false;
      }
    }
  }
  return true;
}

function extractRemoteDir(remotePath) {
  const raw = String(remotePath || "").trim();
  if (!raw) {
    return "/";
  }
  const normalized = raw.replace(/\\/g, "/");
  const index = normalized.lastIndexOf("/");
  if (index <= 0) {
    return "/";
  }
  return normalized.slice(0, index);
}

function joinRemotePath(dir, fileName) {
  const normalizedDir = String(dir || "/").trim().replace(/\\/g, "/");
  const safeDir = normalizedDir === "/" ? "/" : normalizedDir.replace(/\/+$/, "");
  const safeFileName = String(fileName || "").trim();
  if (!safeFileName) {
    return safeDir;
  }
  return safeDir === "/" ? `/${safeFileName}` : `${safeDir}/${safeFileName}`;
}

function normalizeFileName(value, fallback) {
  const raw = String(value || "").trim();
  if (!raw) {
    return fallback;
  }
  return raw.toLowerCase().endsWith(".mp4") ? raw : `${raw}.mp4`;
}

export default function RemoteImportTaskView({
  initialPreview,
  partitions,
  collections,
  onBack,
  onSaved,
}) {
  const [detailTab, setDetailTab] = useState("basic");
  const [message, setMessage] = useState("");
  const [submitting, setSubmitting] = useState(false);

  const [basic, setBasic] = useState({
    originBvid: initialPreview?.bvid || "",
    originAid: Number(initialPreview?.aid || 0) || 0,
    title: initialPreview?.title || "",
    description: initialPreview?.description || "",
    coverUrl: initialPreview?.coverUrl || "",
    coverPreview: "",
    partitionId: (() => {
      const value = Number(initialPreview?.partitionId || 0);
      return value > 0 ? String(Math.trunc(value)) : "";
    })(),
    tags: initialPreview?.tags || "",
    videoType: "ORIGINAL",
    collectionId: initialPreview?.collectionId ? String(initialPreview.collectionId) : "",
    importMode: "NON_SEGMENTED",
    segmentPrefix: "",
    enableSegmentation: Boolean(initialPreview?.enableSegmentation),
    segmentDurationSeconds: Number(initialPreview?.segmentDurationSeconds || 133) || 133,
  });
  const [tagInput, setTagInput] = useState("");
  const [tagList, setTagList] = useState(() => parseTagItems(initialPreview?.tags || ""));

  const [sources, setSources] = useState(() => buildDefaultSources(initialPreview));
  const [mergedList, setMergedList] = useState(() =>
    buildMergedListFromSources(buildDefaultSources(initialPreview), []),
  );
  const [segmentMergedBindingMap, setSegmentMergedBindingMap] = useState({});

  const [sourcePickerOpen, setSourcePickerOpen] = useState(false);
  const [sourceQueryInput, setSourceQueryInput] = useState(initialPreview?.bvid || "");
  const [sourceQueryLoading, setSourceQueryLoading] = useState(false);
  const [sourceQueryResult, setSourceQueryResult] = useState(initialPreview || null);
  const [sourceChecked, setSourceChecked] = useState(() => new Set());
  const [syncPickerOpen, setSyncPickerOpen] = useState(false);
  const [syncTargetMergedId, setSyncTargetMergedId] = useState("");
  const [syncPickerPath, setSyncPickerPath] = useState("/");
  const partitionLogRef = useRef("");

  const sourceMap = useMemo(() => {
    const map = new Map();
    sources.forEach((item) => map.set(item.sourceKey, item));
    return map;
  }, [sources]);

  const partitionOptions = useMemo(
    () => (partitions || []).filter((item) => Number(item?.tid || 0) > 0),
    [partitions],
  );

  const collectionOptions = useMemo(
    () => (collections || []).filter((item) => Number(item?.seasonId || 0) > 0),
    [collections],
  );
  const partitionSelectOptions = partitionOptions;

  const collectionSelectOptions = useMemo(() => {
    const currentCollectionId = Number(basic.collectionId || 0);
    if (!currentCollectionId) {
      return collectionOptions;
    }
    const exists = collectionOptions.some(
      (item) => Number(item.seasonId) === currentCollectionId,
    );
    if (exists) {
      return collectionOptions;
    }
    return [
      {
        seasonId: currentCollectionId,
        name: initialPreview?.collectionName || `合集${currentCollectionId}`,
      },
      ...collectionOptions,
    ];
  }, [collectionOptions, basic.collectionId, initialPreview]);

  const originPages = useMemo(
    () => (Array.isArray(initialPreview?.pages) ? initialPreview.pages : []),
    [initialPreview],
  );
  const originPageCount = originPages.length;
  const originTotalSeconds = useMemo(
    () =>
      originPages.reduce(
        (sum, item) => sum + Math.max(0, Number(item?.durationSeconds || 0) || 0),
        0,
      ),
    [originPages],
  );
  const currentVideoTotalSeconds = useMemo(() => {
    const fromSources = sources.reduce(
      (sum, source) => sum + Math.max(0, resolveSourceDurationSeconds(source)),
      0,
    );
    if (fromSources > 0) {
      return fromSources;
    }
    return originTotalSeconds;
  }, [sources, originTotalSeconds]);
  const reversedSegmentSeconds = useMemo(() => {
    if (originPageCount > 0 && currentVideoTotalSeconds > 0) {
      return Math.max(1, Math.ceil(currentVideoTotalSeconds / originPageCount));
    }
    return Math.max(1, Number(basic.segmentDurationSeconds || 133) || 133);
  }, [originPageCount, currentVideoTotalSeconds, basic.segmentDurationSeconds]);

  useEffect(() => {
    if (!basic.enableSegmentation || originPageCount <= 0) {
      return;
    }
    setBasic((prev) => {
      if (!prev.enableSegmentation) {
        return prev;
      }
      const currentValue = Math.max(1, Number(prev.segmentDurationSeconds || 133) || 133);
      if (currentValue === reversedSegmentSeconds) {
        return prev;
      }
      return {
        ...prev,
        segmentDurationSeconds: reversedSegmentSeconds,
      };
    });
  }, [basic.enableSegmentation, originPageCount, reversedSegmentSeconds]);

  const segmentPreviewRows = useMemo(() => {
    const segmentRows = [];
    const prefix = String(basic.segmentPrefix || "").trim();
    const buildPartName = (order) =>
      prefix ? `${prefix}-${pad2(order)}` : `Part ${order}`;
    let partOrder = 1;
    if (basic.enableSegmentation) {
      const pageCount = originPageCount > 0 ? originPageCount : mergedList.length;
      if (pageCount <= 0) {
        return segmentRows;
      }
      let remaining = Math.max(0, currentVideoTotalSeconds);
      for (let index = 0; index < pageCount; index += 1) {
        const remotePage = originPages[index];
        const remotePageDuration = Math.max(0, Number(remotePage?.durationSeconds || 0) || 0);
        const fallbackDuration =
          index === pageCount - 1
            ? remaining
            : Math.min(remaining, Math.max(1, reversedSegmentSeconds));
        const currentDuration = remotePageDuration > 0 ? remotePageDuration : fallbackDuration;
        const defaultMergedId = (() => {
          if (mergedList.length === 0) {
            return "";
          }
          if (mergedList.length === 1) {
            return String(mergedList[0]?.id || "");
          }
          if (mergedList[index]?.id !== undefined && mergedList[index]?.id !== null) {
            return String(mergedList[index].id);
          }
          return String(mergedList[mergedList.length - 1]?.id || "");
        })();
        const mergedFileName = (() => {
          if (mergedList.length === 0) {
            return "-";
          }
          if (mergedList.length === 1) {
            return mergedList[0]?.fileName || "-";
          }
          if (mergedList[index]?.fileName) {
            return mergedList[index].fileName;
          }
          return mergedList[mergedList.length - 1]?.fileName || "-";
        })();
        segmentRows.push({
          rowKey: `segment-${partOrder}`,
          partOrder,
          partName:
            String(remotePage?.partName || "").trim() || buildPartName(partOrder),
          defaultMergedId,
          mergedFileName,
          durationSeconds: currentDuration,
        });
        remaining = Math.max(0, remaining - currentDuration);
        partOrder += 1;
      }
      return segmentRows;
    }
    mergedList.forEach((merged, index) => {
      const sourceKeys = Array.isArray(merged?.sourceKeys) ? merged.sourceKeys : [];
      const boundSources = sourceKeys
        .map((key) => sourceMap.get(key))
        .filter((item) => Boolean(item));
      const totalDuration = boundSources.reduce(
        (sum, item) => sum + resolveSourceDurationSeconds(item),
        0,
      );
      segmentRows.push({
        rowKey: `segment-${partOrder}`,
        partOrder,
        partName: boundSources[0]?.partName || buildPartName(partOrder),
        defaultMergedId: String(merged?.id || ""),
        mergedFileName: merged?.fileName || `merged-${index + 1}.mp4`,
        durationSeconds: totalDuration,
      });
      partOrder += 1;
    });
    return segmentRows;
  }, [
    basic.enableSegmentation,
    basic.segmentPrefix,
    currentVideoTotalSeconds,
    mergedList,
    originPageCount,
    originPages,
    reversedSegmentSeconds,
    sourceMap,
  ]);

  useEffect(() => {
    let active = true;
    const raw = String(basic.coverUrl || "").trim();
    if (!raw) {
      setBasic((prev) => ({ ...prev, coverPreview: "" }));
      return () => {
        active = false;
      };
    }
    if (!/^https?:\/\//i.test(raw)) {
      setBasic((prev) => ({ ...prev, coverPreview: raw }));
      return () => {
        active = false;
      };
    }
    const load = async () => {
      try {
        const data = await invokeCommand("video_proxy_image", { url: raw });
        if (!active) {
          return;
        }
        setBasic((prev) => ({ ...prev, coverPreview: String(data || "").trim() || raw }));
      } catch (_) {
        if (!active) {
          return;
        }
        setBasic((prev) => ({ ...prev, coverPreview: raw }));
      }
    };
    load();
    return () => {
      active = false;
    };
  }, [basic.coverUrl]);

  useEffect(() => {
    const nextMerged = buildMergedListFromSources(sources, mergedList);
    if (isMergedListEqual(mergedList, nextMerged)) {
      return;
    }
    setMergedList(nextMerged);
  }, [sources, mergedList]);

  useEffect(() => {
    const validMergedIds = new Set(
      mergedList
        .map((item) => String(item?.id || "").trim())
        .filter((item) => item.length > 0),
    );
    setSegmentMergedBindingMap((prev) => {
      const next = {};
      segmentPreviewRows.forEach((row) => {
        const key = String(row?.rowKey || "").trim();
        if (!key) {
          return;
        }
        const previousValue = String(prev?.[key] || "").trim();
        if (previousValue && validMergedIds.has(previousValue)) {
          next[key] = previousValue;
          return;
        }
        const defaultMergedId = String(row?.defaultMergedId || "").trim();
        next[key] = validMergedIds.has(defaultMergedId) ? defaultMergedId : "";
      });
      return next;
    });
  }, [segmentPreviewRows, mergedList]);

  useEffect(() => {
    const nextTags = tagList.join(",");
    setBasic((prev) => (prev.tags === nextTags ? prev : { ...prev, tags: nextTags }));
  }, [tagList]);

  useEffect(() => {
    const currentPartitionId = String(basic.partitionId || "").trim();
    if (!currentPartitionId) {
      return;
    }
    const hasExactId = partitionOptions.some((item) => {
      const rawTid = String(item?.tid || "").trim();
      if (rawTid && rawTid === currentPartitionId) {
        return true;
      }
      return Number(item?.tid || 0) === Number(currentPartitionId);
    });
    const logPartition = async (message) => {
      if (partitionLogRef.current === message) {
        return;
      }
      partitionLogRef.current = message;
      try {
        await invokeCommand("auth_client_log", { message });
      } catch (_) {
      }
    };
    const partitionName = String(initialPreview?.partitionName || "").trim();
    const sample = partitionOptions
      .slice(0, 10)
      .map((item) => `${item?.tid}:${item?.name}`)
      .join("|");
    logPartition(
      `remote_import_partition_check id=${currentPartitionId} name=${partitionName || "-"} options=${partitionOptions.length} sample=${sample}`,
    );
    if (partitionOptions.length === 0) {
      return;
    }
    if (hasExactId) {
      logPartition(`remote_import_partition_matched_by_id id=${currentPartitionId}`);
      return;
    }
    if (!partitionName) {
      logPartition(`remote_import_partition_missing_name id=${currentPartitionId}`);
      setBasic((prev) => ({ ...prev, partitionId: "" }));
      return;
    }
    const normalizeName = (value) =>
      String(value || "")
        .trim()
        .toLowerCase()
        .replace(/[·\s\-_./\\|:：]/g, "");
    const normalizedTarget = normalizeName(partitionName);
    const matched = partitionOptions.find((item) => {
      const name = String(item?.name || "").trim();
      const normalizedName = normalizeName(name);
      if (!normalizedName || !normalizedTarget) {
        return false;
      }
      return (
        normalizedName === normalizedTarget ||
        normalizedName.includes(normalizedTarget) ||
        normalizedTarget.includes(normalizedName)
      );
    });
    if (!matched?.tid) {
      logPartition(
        `remote_import_partition_unmatched id=${currentPartitionId} name=${partitionName} sample=${sample}`,
      );
      setBasic((prev) => ({ ...prev, partitionId: "" }));
      return;
    }
    const matchedTid = String(matched.tid).trim();
    if (!matchedTid || matchedTid === currentPartitionId) {
      return;
    }
    logPartition(
      `remote_import_partition_remap from=${currentPartitionId} name=${partitionName} to=${matchedTid}`,
    );
    setBasic((prev) => ({ ...prev, partitionId: matchedTid }));
  }, [basic.partitionId, partitionOptions, initialPreview]);

  const resetSourcePicker = () => {
    setSourcePickerOpen(false);
    setSourceQueryInput(initialPreview?.bvid || "");
    setSourceQueryLoading(false);
    setSourceQueryResult(initialPreview || null);
    setSourceChecked(new Set());
  };

  const handleSourceQuery = async () => {
    const input = sourceQueryInput.trim();
    if (!input) {
      setMessage("请输入视频链接或BVID");
      return;
    }
    setSourceQueryLoading(true);
    setMessage("");
    try {
      const result = await invokeCommand("submission_remote_video_preview", {
        request: {
          input,
          enforceOwnerMatch: false,
        },
      });
      setSourceQueryResult(result);
      setSourceChecked(new Set());
    } catch (error) {
      setMessage(error?.message || "查询视频失败");
      setSourceQueryResult(null);
      setSourceChecked(new Set());
    } finally {
      setSourceQueryLoading(false);
    }
  };

  const toggleSourceChecked = (key, checked) => {
    setSourceChecked((prev) => {
      const next = new Set(prev);
      if (checked) {
        next.add(key);
      } else {
        next.delete(key);
      }
      return next;
    });
  };

  const handleConfirmSourcePick = () => {
    if (!sourceQueryResult?.pages?.length) {
      setMessage("请先查询可选分P");
      return;
    }
    const selectedPages = sourceQueryResult.pages.filter((item) => sourceChecked.has(item.sourceKey));
    if (selectedPages.length === 0) {
      setMessage("请至少选择一个分P");
      return;
    }
    setSources((prev) => {
      const existing = new Set(prev.map((item) => item.sourceKey));
      const appendItems = selectedPages
        .filter((item) => !existing.has(item.sourceKey))
        .map((item) => ({
          sourceKey: item.sourceKey,
          bvid: item.bvid,
          cid: Number(item.cid || 0) || 0,
          partName: item.partName || `P${item.page || 1}`,
          durationSeconds: Number(item.durationSeconds || 0) || 0,
          startTime: "00:00:00",
          endTime: normalizeTimeText(item.endTime, formatSecondsToHms(Number(item.durationSeconds || 0))),
          sourceFilePath:
            item.sourceFilePath ||
            `remote/${item.bvid || "unknown"}/${Number(item.cid || 0)}.mp4`,
          remoteFileName: String(item.remoteFileName || "").trim(),
        }));
      const combined = [...prev, ...appendItems].map((item, index) => ({
        ...item,
        sortOrder: index + 1,
      }));
      return combined;
    });
    setMessage("已加入源视频列表");
    resetSourcePicker();
  };

  const updateSourceField = (sourceKey, key, value) => {
    setSources((prev) =>
      prev.map((item) => (item.sourceKey === sourceKey ? { ...item, [key]: value } : item)),
    );
  };

  const normalizeSourceTime = (sourceKey, key) => {
    setSources((prev) =>
      prev.map((item) => {
        if (item.sourceKey !== sourceKey) {
          return item;
        }
        const fallback = key === "startTime" ? "00:00:00" : formatSecondsToHms(item.durationSeconds);
        return {
          ...item,
          [key]: normalizeTimeText(item[key], fallback),
        };
      }),
    );
  };

  const addTag = (value) => {
    const normalized = String(value || "").trim();
    if (!normalized) {
      return;
    }
    setTagList((prev) => {
      if (prev.includes(normalized)) {
        return prev;
      }
      return [...prev, normalized];
    });
  };

  const removeTag = (value) => {
    const normalized = String(value || "").trim();
    if (!normalized) {
      return;
    }
    setTagList((prev) => prev.filter((item) => item !== normalized));
  };

  const handleTagKeyDown = (event) => {
    if (event.key !== "Enter") {
      return;
    }
    event.preventDefault();
    addTag(tagInput);
    setTagInput("");
  };

  const removeSource = (sourceKey) => {
    setSources((prev) =>
      prev
        .filter((item) => item.sourceKey !== sourceKey)
        .map((item, index) => ({
          ...item,
          sortOrder: index + 1,
        })),
    );
  };

  const updateMergedField = (id, key, value) => {
    setMergedList((prev) =>
      prev.map((item) => {
        if (item.id !== id) {
          return item;
        }
        if (key === "fileName") {
          const nextFileName = normalizeFileName(value, item.fileName);
          const nextRemotePath = item.remotePath
            ? joinRemotePath(extractRemoteDir(item.remotePath), nextFileName)
            : item.remotePath;
          return {
            ...item,
            fileName: nextFileName,
            remotePath: nextRemotePath,
          };
        }
        return { ...item, [key]: value };
      }),
    );
  };

  const handleRebindMergedSourceKey = (mergedId, nextSourceKey) => {
    const targetMergedId = String(mergedId || "").trim();
    const targetSourceKey = String(nextSourceKey || "").trim();
    if (!targetMergedId || !targetSourceKey) {
      return;
    }
    setMergedList((prev) => {
      const next = prev.map((item) => ({
        ...item,
        sourceKeys: Array.isArray(item?.sourceKeys) ? [...item.sourceKeys] : [],
      }));
      const targetIndex = next.findIndex(
        (item) => String(item?.id || "").trim() === targetMergedId,
      );
      if (targetIndex < 0) {
        return prev;
      }
      const currentTargetKey = String(next[targetIndex]?.sourceKeys?.[0] || "").trim();
      if (currentTargetKey === targetSourceKey) {
        return prev;
      }
      const conflictIndex = next.findIndex((item, index) => {
        if (index === targetIndex) {
          return false;
        }
        return String(item?.sourceKeys?.[0] || "").trim() === targetSourceKey;
      });
      next[targetIndex].sourceKeys = [targetSourceKey];
      if (conflictIndex >= 0 && currentTargetKey) {
        next[conflictIndex].sourceKeys = [currentTargetKey];
      }
      return next;
    });
  };

  const updateSegmentMergedBinding = (rowKey, mergedId) => {
    const normalizedRowKey = String(rowKey || "").trim();
    if (!normalizedRowKey) {
      return;
    }
    setSegmentMergedBindingMap((prev) => ({
      ...prev,
      [normalizedRowKey]: String(mergedId || "").trim(),
    }));
  };

  const openMergedSyncPicker = (mergedId) => {
    const target = mergedList.find((item) => String(item.id) === String(mergedId));
    if (!target) {
      return;
    }
    setSyncTargetMergedId(String(target.id));
    setSyncPickerPath(extractRemoteDir(target.remotePath) || "/");
    setSyncPickerOpen(true);
  };

  const closeMergedSyncPicker = () => {
    setSyncPickerOpen(false);
    setSyncTargetMergedId("");
    setSyncPickerPath("/");
  };

  const handleConfirmMergedSyncPicker = (path) => {
    const normalizedDir = String(path || "").trim() || "/";
    setMergedList((prev) =>
      prev.map((item) => {
        if (String(item.id) !== String(syncTargetMergedId)) {
          return item;
        }
        return {
          ...item,
          remotePath: joinRemotePath(normalizedDir, item.fileName),
        };
      }),
    );
    closeMergedSyncPicker();
  };

  const handleSubmit = async () => {
    const pendingTag = tagInput.trim();
    const finalTags = [...tagList];
    if (pendingTag && !finalTags.includes(pendingTag)) {
      finalTags.push(pendingTag);
    }
    if (!basic.title.trim()) {
      setMessage("请填写投稿标题");
      setDetailTab("basic");
      return;
    }
    if (!(Number(basic.partitionId) > 0)) {
      setMessage("请先选择分区");
      setDetailTab("basic");
      return;
    }
    if (sources.length === 0) {
      setMessage("请至少配置一条源视频");
      setDetailTab("source");
      return;
    }

    setSubmitting(true);
    setMessage("");
    try {
      const payload = {
        basic: {
          originBvid: String(basic.originBvid || "").trim(),
          originAid: Number(basic.originAid || 0) || null,
          title: basic.title,
          description: basic.description,
          coverUrl: basic.coverUrl,
          partitionId: Number(basic.partitionId),
          tags: finalTags.join(","),
          videoType: basic.videoType,
          collectionId: basic.collectionId ? Number(basic.collectionId) : null,
          importMode: basic.importMode,
          segmentPrefix: basic.segmentPrefix,
          enableSegmentation: Boolean(basic.enableSegmentation),
          segmentDurationSeconds: Math.max(
            1,
            Number(basic.segmentDurationSeconds || 133) || 133,
          ),
        },
        sources: sources.map((item, index) => ({
          sourceKey: item.sourceKey,
          bvid: item.bvid,
          cid: Number(item.cid),
          partName: item.partName,
          durationSeconds: Number(item.durationSeconds || 0),
          sortOrder: index + 1,
          startTime: normalizeTimeText(item.startTime, "00:00:00"),
          endTime: normalizeTimeText(
            item.endTime,
            formatSecondsToHms(Number(item.durationSeconds || 0)),
          ),
          sourceFilePath: item.sourceFilePath,
          remoteFileName: String(item.remoteFileName || "").trim(),
        })),
        mergedVideos: mergedList.map((item, index) => ({
          sortOrder: index + 1,
          fileName: item.fileName,
          remotePath: item.remotePath,
          sourceKeys: item.sourceKeys,
        })),
      };

      const result = await invokeCommand("submission_remote_import_save", {
        request: payload,
      });
      setMessage(`提交成功，任务ID：${result.taskId}`);
      onSaved?.(result);
    } catch (error) {
      setMessage(error?.message || "提交失败");
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className="rounded-2xl bg-[var(--surface)]/90 p-6 shadow-sm ring-1 ring-black/5">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <p className="text-sm uppercase tracking-[0.2em] text-[var(--muted)]">视频投稿</p>
          <h2 className="text-2xl font-semibold text-[var(--ink)]">远程导入任务</h2>
        </div>
        <button
          className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
          onClick={onBack}
        >
          返回列表
        </button>
      </div>

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
          { key: "segment", label: "分段视频" },
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
        <div className="ml-auto">
          <button
            className="rounded-full bg-[var(--accent)] px-4 py-2 text-sm font-semibold text-white disabled:cursor-not-allowed disabled:opacity-60"
            onClick={handleSubmit}
            disabled={submitting}
          >
            {submitting ? "提交中" : "提交"}
          </button>
        </div>
      </div>

      {detailTab === "basic" ? (
        <div className="mt-4 space-y-4 text-sm text-[var(--ink)]">
          <div>
            <div className="text-xs text-[var(--muted)]">视频封面</div>
            <div className="mt-1 flex h-44 w-80 items-center justify-center overflow-hidden rounded-lg border border-black/10 bg-white/80">
              {basic.coverPreview ? (
                <img
                  src={basic.coverPreview}
                  alt="封面预览"
                  className="h-full w-full object-cover"
                />
              ) : (
                <span className="text-xs text-[var(--muted)]">暂无封面</span>
              )}
            </div>
          </div>
          <div>
            <div className="text-xs text-[var(--muted)]">投稿标题</div>
            <input
              value={basic.title}
              onChange={(event) => setBasic((prev) => ({ ...prev, title: event.target.value }))}
              placeholder="请输入投稿标题"
              className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2"
            />
          </div>
          <div>
            <div className="text-xs text-[var(--muted)]">视频描述</div>
            <textarea
              value={basic.description}
              onChange={(event) =>
                setBasic((prev) => ({
                  ...prev,
                  description: event.target.value,
                }))
              }
              rows={4}
              className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2"
            />
          </div>
          <div className="grid gap-3 lg:grid-cols-4">
            <div>
              <div className="text-xs text-[var(--muted)]">分区</div>
              <select
                value={basic.partitionId}
                onChange={(event) =>
                  setBasic((prev) => ({
                    ...prev,
                    partitionId: event.target.value,
                  }))
                }
                className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2"
              >
                <option value="">请选择分区</option>
                {partitionSelectOptions.map((item) => (
                  <option key={item.tid} value={String(item.tid)}>
                    {item.name}
                  </option>
                ))}
              </select>
            </div>
            <div>
              <div className="text-xs text-[var(--muted)]">合集</div>
              <select
                value={basic.collectionId}
                onChange={(event) =>
                  setBasic((prev) => ({
                    ...prev,
                    collectionId: event.target.value,
                  }))
                }
                className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2"
              >
                <option value="">不设置</option>
                {collectionSelectOptions.map((item) => (
                  <option key={item.seasonId} value={String(item.seasonId)}>
                    {item.name}
                  </option>
                ))}
              </select>
            </div>
            <div>
              <div className="text-xs text-[var(--muted)]">视频类型</div>
              <select
                value={basic.videoType}
                onChange={(event) =>
                  setBasic((prev) => ({
                    ...prev,
                    videoType: event.target.value,
                  }))
                }
                className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2"
              >
                <option value="ORIGINAL">原创</option>
                <option value="REPOST">转载</option>
              </select>
            </div>
            <div>
              <div className="text-xs text-[var(--muted)]">导入模式</div>
              <select
                value={basic.importMode}
                onChange={(event) =>
                  setBasic((prev) => ({
                    ...prev,
                    importMode: event.target.value === "SEGMENTED" ? "SEGMENTED" : "NON_SEGMENTED",
                  }))
                }
                className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2"
              >
                <option value="NON_SEGMENTED">不分段（一对一）</option>
                <option value="SEGMENTED">分段（一对多）</option>
              </select>
            </div>
          </div>
          <div className="grid gap-3 lg:grid-cols-2">
            <div>
              <div className="text-xs text-[var(--muted)]">投稿标签</div>
              <div className="rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus-within:border-[var(--accent)]">
                <div className="flex flex-wrap gap-2">
                  {tagList.map((tag) => (
                    <span
                      key={tag}
                      className="inline-flex items-center gap-1 rounded-full bg-[var(--accent)]/10 px-2 py-1 text-xs text-[var(--accent)]"
                    >
                      {tag}
                      <button
                        className="text-[10px] font-semibold text-[var(--accent)] hover:opacity-70"
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
                    onBlur={() => {
                      addTag(tagInput);
                      setTagInput("");
                    }}
                    placeholder="回车添加标签"
                    className="min-w-[120px] flex-1 bg-transparent text-sm text-[var(--ink)] focus:outline-none"
                  />
                </div>
              </div>
            </div>
            <div>
              <div className="text-xs text-[var(--muted)]">是否分段</div>
              <select
                value={basic.enableSegmentation ? "1" : "0"}
                onChange={(event) =>
                  setBasic((prev) => ({
                    ...prev,
                    enableSegmentation: event.target.value === "1",
                  }))
                }
                className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2"
              >
                <option value="0">否</option>
                <option value="1">是</option>
              </select>
            </div>
            <div>
              <div className="text-xs text-[var(--muted)]">分段时长（秒）</div>
              <input
                type="number"
                min={1}
                value={basic.segmentDurationSeconds}
                onChange={(event) =>
                  setBasic((prev) => ({
                    ...prev,
                    segmentDurationSeconds: Math.max(1, Number(event.target.value || 1)),
                  }))
                }
                disabled={!basic.enableSegmentation || originPageCount > 0}
                className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 disabled:cursor-not-allowed disabled:opacity-50"
              />
              {basic.enableSegmentation && originPageCount > 0 ? (
                <div className="mt-1 text-[11px] text-[var(--muted)]">
                  已按远程分P自动反算分段时长。
                </div>
              ) : null}
            </div>
            <div>
              <div className="text-xs text-[var(--muted)]">分段前缀（可选）</div>
              <input
                value={basic.segmentPrefix}
                onChange={(event) =>
                  setBasic((prev) => ({
                    ...prev,
                    segmentPrefix: event.target.value,
                  }))
                }
                placeholder="分段前缀"
                className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2"
              />
            </div>
          </div>
        </div>
      ) : null}

      {detailTab === "source" ? (
        <div className="mt-4 space-y-3 text-sm text-[var(--ink)]">
          <div className="flex items-center justify-between gap-2">
            <div className="text-xs text-[var(--muted)]">源视频列表</div>
            <button
              className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
              onClick={() => setSourcePickerOpen(true)}
            >
              选择源视频
            </button>
          </div>
          <div className="overflow-hidden rounded-xl border border-black/5">
            <table className="w-full text-left text-sm">
              <thead className="bg-black/5 text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
                <tr>
                  <th className="px-4 py-2">序号</th>
                  <th className="px-4 py-2">来源视频</th>
                  <th className="px-4 py-2">分P</th>
                  <th className="px-4 py-2">开始时间</th>
                  <th className="px-4 py-2">结束时间</th>
                  <th className="px-4 py-2">操作</th>
                </tr>
              </thead>
              <tbody>
                {sources.length === 0 ? (
                  <tr>
                    <td className="px-4 py-3 text-[var(--muted)]" colSpan={6}>
                      暂无源视频
                    </td>
                  </tr>
                ) : (
                  sources.map((item, index) => (
                    <tr key={item.sourceKey} className="border-t border-black/5">
                      <td className="px-4 py-2 text-[var(--muted)]">{index + 1}</td>
                      <td className="px-4 py-2 text-[var(--ink)]">{item.bvid}</td>
                      <td className="px-4 py-2 text-[var(--ink)]">{item.partName}</td>
                      <td className="px-4 py-2">
                        <input
                          value={item.startTime}
                          onChange={(event) =>
                            updateSourceField(item.sourceKey, "startTime", event.target.value)
                          }
                          onBlur={() => normalizeSourceTime(item.sourceKey, "startTime")}
                          className="w-full rounded-lg border border-black/10 bg-white/80 px-2 py-1"
                        />
                      </td>
                      <td className="px-4 py-2">
                        <input
                          value={item.endTime}
                          onChange={(event) =>
                            updateSourceField(item.sourceKey, "endTime", event.target.value)
                          }
                          onBlur={() => normalizeSourceTime(item.sourceKey, "endTime")}
                          className="w-full rounded-lg border border-black/10 bg-white/80 px-2 py-1"
                        />
                      </td>
                      <td className="px-4 py-2">
                        <button
                          className="rounded-full border border-red-200 bg-red-50 px-2 py-1 text-xs font-semibold text-red-600"
                          onClick={() => removeSource(item.sourceKey)}
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
        </div>
      ) : null}

      {detailTab === "merged" ? (
        <div className="mt-4 space-y-3 text-sm text-[var(--ink)]">
          <div className="flex items-center justify-between gap-2">
            <div className="text-xs text-[var(--muted)]">
              {basic.importMode === "NON_SEGMENTED"
                ? "合并视频列表（与源视频一对一自动绑定）"
                : "合并视频列表（分段模式：后续支持一对多绑定）"}
            </div>
          </div>
          <div className="overflow-hidden rounded-xl border border-black/5">
            <table className="w-full text-left text-sm">
              <thead className="bg-black/5 text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
                <tr>
                  <th className="px-4 py-2">序号</th>
                  <th className="px-4 py-2">文件名</th>
                  <th className="px-4 py-2">网盘链接</th>
                  <th className="px-4 py-2">绑定源视频</th>
                </tr>
              </thead>
              <tbody>
                {mergedList.length === 0 ? (
                  <tr>
                    <td className="px-4 py-3 text-[var(--muted)]" colSpan={4}>
                      暂无合并视频
                    </td>
                  </tr>
                ) : (
                  mergedList.map((item, index) => (
                    <tr key={item.id} className="border-t border-black/5">
                      <td className="px-4 py-2 text-[var(--muted)]">{index + 1}</td>
                      <td className="px-4 py-2">
                        <input
                          value={item.fileName}
                          onChange={(event) =>
                            updateMergedField(item.id, "fileName", event.target.value)
                          }
                          className="w-full rounded-lg border border-black/10 bg-white/80 px-2 py-1"
                        />
                      </td>
                      <td className="px-4 py-2">
                        <div className="flex items-center gap-2">
                          <div className="flex-1 rounded-lg border border-black/10 bg-white/80 px-2 py-1 text-xs text-[var(--ink)]">
                            {item.remotePath || "-"}
                          </div>
                          <button
                            className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                            onClick={() => openMergedSyncPicker(item.id)}
                          >
                            选择目录
                          </button>
                        </div>
                      </td>
                      <td className="px-4 py-2">
                        <select
                          value={String(item?.sourceKeys?.[0] || "")}
                          onChange={(event) =>
                            handleRebindMergedSourceKey(item.id, event.target.value)
                          }
                          className="w-full rounded-lg border border-black/10 bg-white/80 px-2 py-1 text-xs text-[var(--ink)]"
                        >
                          <option value="">请选择源视频</option>
                          {sources.map((source, sourceIndex) => (
                            <option key={source.sourceKey} value={source.sourceKey}>
                              {`${sourceIndex + 1}. ${source.partName} (${source.bvid})`}
                            </option>
                          ))}
                        </select>
                      </td>
                    </tr>
                  ))
                )}
              </tbody>
            </table>
          </div>
        </div>
      ) : null}

      {detailTab === "segment" ? (
        <div className="mt-4 space-y-3 text-sm text-[var(--ink)]">
          <div className="flex items-center justify-between gap-2">
            <div className="text-xs text-[var(--muted)]">
              {basic.enableSegmentation
                ? "分段视频列表（以远程分P为准）"
                : "分段视频列表（未分段时与合并视频一对一）"}
            </div>
            {basic.enableSegmentation ? (
              <div className="text-xs text-[var(--muted)]">
                总时长：{formatSecondsToHms(currentVideoTotalSeconds)} / 远程分P：{originPageCount || 0} / 反算分段时长：{reversedSegmentSeconds}s
              </div>
            ) : null}
          </div>
          <div className="overflow-hidden rounded-xl border border-black/5">
            <table className="w-full text-left text-sm">
              <thead className="bg-black/5 text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
                <tr>
                  <th className="px-4 py-2">序号</th>
                  <th className="px-4 py-2">分段名称</th>
                  <th className="px-4 py-2">绑定合并视频</th>
                  <th className="px-4 py-2">预估时长</th>
                </tr>
              </thead>
              <tbody>
                {segmentPreviewRows.length === 0 ? (
                  <tr>
                    <td className="px-4 py-3 text-[var(--muted)]" colSpan={4}>
                      暂无分段数据
                    </td>
                  </tr>
                ) : (
                  segmentPreviewRows.map((item) => {
                    const bindingKey = String(item?.rowKey || "");
                    const selectedMergedId =
                      segmentMergedBindingMap[bindingKey] || String(item?.defaultMergedId || "");
                    return (
                    <tr key={bindingKey || `${item.partOrder}-${item.partName}`} className="border-t border-black/5">
                      <td className="px-4 py-2 text-[var(--muted)]">{item.partOrder}</td>
                      <td className="px-4 py-2 text-[var(--ink)]">{item.partName}</td>
                      <td className="px-4 py-2">
                        <select
                          value={selectedMergedId}
                          onChange={(event) =>
                            updateSegmentMergedBinding(bindingKey, event.target.value)
                          }
                          className="w-full rounded-lg border border-black/10 bg-white/80 px-2 py-1 text-xs text-[var(--ink)]"
                        >
                          <option value="">请选择合并视频</option>
                          {mergedList.map((merged, mergedIndex) => (
                            <option key={merged.id} value={String(merged.id)}>
                              {merged.fileName || `合并视频 ${mergedIndex + 1}`}
                            </option>
                          ))}
                        </select>
                      </td>
                      <td className="px-4 py-2 text-[var(--muted)]">
                        {formatSecondsToHms(item.durationSeconds)}
                      </td>
                    </tr>
                    );
                  })
                )}
              </tbody>
            </table>
          </div>
        </div>
      ) : null}

      {sourcePickerOpen ? (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/30 px-4">
          <div className="w-full max-w-4xl rounded-2xl bg-white p-5 shadow-lg">
            <div className="flex items-center justify-between gap-3">
              <div className="text-sm font-semibold text-[var(--ink)]">选择源视频</div>
              <button
                className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                onClick={resetSourcePicker}
              >
                关闭
              </button>
            </div>
            <div className="mt-3 flex gap-2">
              <input
                value={sourceQueryInput}
                onChange={(event) => setSourceQueryInput(event.target.value)}
                placeholder="输入视频链接或BVID"
                className="flex-1 rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm"
              />
              <button
                className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                onClick={handleSourceQuery}
                disabled={sourceQueryLoading}
              >
                {sourceQueryLoading ? "查询中" : "查询"}
              </button>
            </div>
            <div className="mt-3 text-xs text-[var(--muted)]">
              {sourceQueryResult
                ? `视频：${sourceQueryResult.title}（${sourceQueryResult.bvid}）`
                : "请先查询视频分P后再选择"}
            </div>
            <div className="mt-3 max-h-[320px] overflow-auto rounded-xl border border-black/5">
              <table className="w-full text-left text-sm">
                <thead className="bg-black/5 text-xs uppercase tracking-[0.2em] text-[var(--muted)]">
                  <tr>
                    <th className="px-4 py-2">选择</th>
                    <th className="px-4 py-2">BVID</th>
                    <th className="px-4 py-2">分P</th>
                    <th className="px-4 py-2">时长</th>
                  </tr>
                </thead>
                <tbody>
                  {!sourceQueryResult?.pages?.length ? (
                    <tr>
                      <td className="px-4 py-3 text-[var(--muted)]" colSpan={4}>
                        暂无可选分P
                      </td>
                    </tr>
                  ) : (
                    sourceQueryResult.pages.map((item) => {
                      const disabled = sources.some((source) => source.sourceKey === item.sourceKey);
                      const checked = sourceChecked.has(item.sourceKey);
                      return (
                        <tr key={item.sourceKey} className="border-t border-black/5">
                          <td className="px-4 py-2">
                            <input
                              type="checkbox"
                              checked={checked}
                              onChange={(event) =>
                                toggleSourceChecked(item.sourceKey, event.target.checked)
                              }
                              disabled={disabled}
                            />
                          </td>
                          <td className="px-4 py-2 text-[var(--ink)]">{item.bvid}</td>
                          <td className="px-4 py-2 text-[var(--ink)]">{item.partName}</td>
                          <td className="px-4 py-2 text-[var(--muted)]">{item.endTime}</td>
                        </tr>
                      );
                    })
                  )}
                </tbody>
              </table>
            </div>
            <div className="mt-4 flex justify-end gap-2">
              <button
                className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                onClick={resetSourcePicker}
              >
                取消
              </button>
              <button
                className="rounded-full bg-[var(--accent)] px-3 py-1 text-xs font-semibold text-white"
                onClick={handleConfirmSourcePick}
              >
                确认加入
              </button>
            </div>
          </div>
        </div>
      ) : null}

      <BaiduSyncPathPicker
        open={syncPickerOpen}
        value={syncPickerPath}
        onConfirm={handleConfirmMergedSyncPicker}
        onClose={closeMergedSyncPicker}
        onChange={setSyncPickerPath}
      />

    </div>
  );
}
