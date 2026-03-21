import { useEffect, useMemo, useRef, useState } from "react";
import LoadingButton from "../components/LoadingButton";
import { showErrorDialog } from "../lib/dialog";
import { invokeCommand } from "../lib/tauri";
import { formatDateTime, formatDuration, formatNumber, parseVideoInput } from "../lib/format";
import BaiduSyncPathPicker from "../components/BaiduSyncPathPicker";

const recordTabs = [
  { key: "pending", label: "待下载", status: 0 },
  { key: "downloading", label: "下载中", status: 1 },
  { key: "completed", label: "已下载", status: 2 },
  { key: "failed", label: "失败", status: 3 },
];

const defaultDownloadConfig = {
  downloadName: "",
  downloadPath: "",
  resolution: "",
  codec: "",
  format: "dash",
  content: "audio_video",
};

const defaultWorkflowConfig = {
  segmentationConfig: {
    segmentDurationSeconds: 133,
    preserveOriginal: true,
  },
};

const buildMergeGroupId = () =>
  `group_${Math.random().toString(36).slice(2, 10)}_${Date.now().toString(36)}`;

const buildMergeItemsFromParts = (parts) =>
  (parts || [])
    .map((item) => item?.key)
    .filter((id) => id)
    .map((id) => ({ id, type: "SOURCE", sourceId: id, standalone: false }));

const fallbackResolutions = [
  { value: "64", label: "720P 高清" },
  { value: "80", label: "1080P 高清" },
  { value: "112", label: "1080P 高码率" },
];

const fallbackCodecs = [
  { value: "avc1.640032", label: "H.264" },
  { value: "hev1.1.6.L150.90", label: "H.265" },
  { value: "vp09.00.41.08.01.01.01.01", label: "VP9" },
];

const fallbackFormats = [
  { value: "dash", label: "DASH" },
  { value: "mp4", label: "MP4" },
];

const formatDurationHms = (seconds) => {
  const totalSeconds = Math.max(0, Math.floor(seconds || 0));
  const hrs = Math.floor(totalSeconds / 3600);
  const mins = Math.floor((totalSeconds % 3600) / 60);
  const secs = totalSeconds % 60;
  return `${String(hrs).padStart(2, "0")}:${String(mins).padStart(2, "0")}:${String(secs).padStart(2, "0")}`;
};

const timeToSeconds = (value) => {
  if (!value) {
    return 0;
  }
  const parts = value.split(":").map((part) => Number(part));
  if (parts.length !== 3 || parts.some((item) => Number.isNaN(item))) {
    return 0;
  }
  return parts[0] * 3600 + parts[1] * 60 + parts[2];
};

const sanitizeFilename = (name) => {
  if (!name) {
    return "";
  }
  return Array.from(name)
    .map((ch) => (/[/\\:*?"<>|]/.test(ch) ? "_" : ch))
    .join("");
};

const normalizePath = (path) => {
  return String(path || "")
    .replace(/\\/g, "/")
    .replace(/\/+$/, "");
};

const toFolderPath = (path) => {
  const normalized = normalizePath(path);
  if (!normalized) {
    return "";
  }
  const parts = normalized.split("/");
  if (parts.length <= 1) {
    return normalized;
  }
  parts.pop();
  return parts.join("/") || "/";
};

const buildVideoKey = (bvid, aid, index) => {
  if (bvid) {
    return bvid;
  }
  if (aid) {
    return `av${aid}`;
  }
  return `input-${index}`;
};

const buildPartKey = (videoKey, cid) => {
  return `${videoKey}:${cid}`;
};

const extractVideoInputs = (input) => {
  if (!input) {
    return [];
  }
  const matches = Array.from(input.matchAll(/BV[0-9A-Za-z]+|av\d+/gi), (item) => item[0]);
  if (matches.length > 0) {
    return matches;
  }
  return input
    .split(/[\s,，;；]+/)
    .map((item) => item.trim())
    .filter(Boolean);
};

export default function DownloadSection({
  activeBilibiliUid = "",
  onAuthChange,
  onRefreshBaiduStatus,
}) {
  const [mainTab, setMainTab] = useState("download");
  const [downloadStep, setDownloadStep] = useState("select");
  const [recordTab, setRecordTab] = useState("pending");
  const [searchInput, setSearchInput] = useState("");
  const [searching, setSearching] = useState(false);
  const [videoItems, setVideoItems] = useState([]);
  const [availableResolutions, setAvailableResolutions] = useState([]);
  const [availableCodecs, setAvailableCodecs] = useState([]);
  const [availableFormats, setAvailableFormats] = useState([]);
  const [downloadConfig, setDownloadConfig] = useState(defaultDownloadConfig);
  const [defaultDownloadPath, setDefaultDownloadPath] = useState("");
  const [message, setMessage] = useState("");
  const [downloadList, setDownloadList] = useState([]);
  const [loadingDownloads, setLoadingDownloads] = useState(false);
  const [submitSubmitting, setSubmitSubmitting] = useState(false);
  const [defaultBaiduSyncPath, setDefaultBaiduSyncPath] = useState("/录播");
  const [currentUpProfile, setCurrentUpProfile] = useState({ uid: 0, name: "" });
  const [bilibiliAccounts, setBilibiliAccounts] = useState([]);
  const [selectedBilibiliUid, setSelectedBilibiliUid] = useState("");

  const [integrationEnabled, setIntegrationEnabled] = useState(false);
  const [segmentationEnabled, setSegmentationEnabled] = useState(true);
  const [workflowConfig, setWorkflowConfig] = useState(defaultWorkflowConfig);
  const [tagInput, setTagInput] = useState("");
  const [tags, setTags] = useState([]);
  const [submissionConfig, setSubmissionConfig] = useState({
    title: "",
    description: "",
    partitionId: "",
    videoType: "ORIGINAL",
    collectionId: "",
    activityTopicId: "",
    activityMissionId: "",
    activityTitle: "",
    segmentPrefix: "",
    priority: false,
    baiduSyncEnabled: false,
    baiduSyncPath: "",
    baiduSyncFilename: "",
  });
  const [submitMergeItems, setSubmitMergeItems] = useState(() =>
    buildMergeItemsFromParts([]),
  );
  const [submitMergeSelection, setSubmitMergeSelection] = useState(() => new Set());
  const [draggingSubmitMergeItemId, setDraggingSubmitMergeItemId] = useState("");
  const [draggingSubmitMergeGroupSource, setDraggingSubmitMergeGroupSource] = useState({
    groupId: "",
    sourceId: "",
  });
  const submitMergeDragStateRef = useRef({ activeId: "", overId: "" });
  const submitMergeGroupDragStateRef = useRef({ groupId: "", activeId: "", overId: "" });
  const [partitions, setPartitions] = useState([]);
  const [collections, setCollections] = useState([]);
  const [activityOptions, setActivityOptions] = useState([]);
  const [activityLoading, setActivityLoading] = useState(false);
  const [activityMessage, setActivityMessage] = useState("");
  const [quickFillOpen, setQuickFillOpen] = useState(false);
  const [quickFillTasks, setQuickFillTasks] = useState([]);
  const [quickFillPage, setQuickFillPage] = useState(1);
  const [quickFillTotal, setQuickFillTotal] = useState(0);
  const [quickFillSearch, setQuickFillSearch] = useState("");
  const quickFillPageSize = 10;
  const [syncPickerOpen, setSyncPickerOpen] = useState(false);
  const [deleteConfirmRecord, setDeleteConfirmRecord] = useState(null);
  const [deleteConfirmDeleteFile, setDeleteConfirmDeleteFile] = useState(false);

  const logClient = async (text) => {
    try {
      await invokeCommand("auth_client_log", { message: text });
    } catch (error) {
      // ignore log errors
    }
  };

  const playOptionsEmpty =
    availableResolutions.length === 0 ||
    availableCodecs.length === 0 ||
    availableFormats.length === 0;

  const selectedParts = useMemo(() => {
    return videoItems.flatMap((item) =>
      item.selectedParts.map((part) => ({
        ...part,
        videoKey: item.key,
        videoTitle: item.info?.title || "未知视频",
      })),
    );
  }, [videoItems]);

  const selectedPartsConfig = useMemo(() => {
    return videoItems.flatMap((item) =>
      item.selectedPartsConfig.map((part) => ({
        ...part,
        videoKey: item.key,
        videoTitle: item.info?.title || "未知视频",
      })),
    );
  }, [videoItems]);

  const selectedVideoItems = useMemo(() => {
    return videoItems.filter((item) => item.selectedParts.length > 0);
  }, [videoItems]);

  const selectedCount = selectedParts.length;
  const hasVideo = videoItems.length > 0;
  const hasSelection = selectedCount > 0;
  const isMultiVideo = videoItems.length > 1;
  const allVideosSelected =
    hasVideo &&
    videoItems.every(
      (item) => item.parts.length > 0 && item.selectedParts.length === item.parts.length,
    );
  const submitGroupedSourceIds = buildGroupedSourceIdSet(submitMergeItems);
  const normalizedActiveBilibiliUid = String(activeBilibiliUid || "").trim();

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
      return { auth, profile };
    } catch (_) {
      return null;
    }
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
      if (integrationEnabled) {
        await Promise.all([loadPartitions(nextSelectedUid), loadCollections(nextSelectedUid)]);
      }
    } catch (error) {
      setMessage(error?.message || "切换投稿账号失败");
    }
  };

  useEffect(() => {
    setSubmitMergeItems((prev) => {
      const next = syncMergeItemsWithParts(selectedPartsConfig, prev);
      const grouped = buildGroupedSourceIdSet(next);
      setSubmitMergeSelection((selection) => {
        const filtered = new Set();
        for (const id of selection) {
          if (!grouped.has(id) && selectedPartsConfig.some((item) => item.key === id)) {
            filtered.add(id);
          }
        }
        return filtered;
      });
      return next;
    });
  }, [selectedPartsConfig]);

  useEffect(() => {
    if (!draggingSubmitMergeItemId) {
      return undefined;
    }
    const handleMove = (event) => {
      trackPointerOverSubmitMergeItem(event);
    };
    const handleUp = () => {
      endSubmitMergeItemDrag();
    };
    window.addEventListener("pointermove", handleMove);
    window.addEventListener("pointerup", handleUp);
    window.addEventListener("pointercancel", handleUp);
    return () => {
      window.removeEventListener("pointermove", handleMove);
      window.removeEventListener("pointerup", handleUp);
      window.removeEventListener("pointercancel", handleUp);
    };
  }, [draggingSubmitMergeItemId]);

  useEffect(() => {
    if (!draggingSubmitMergeGroupSource.sourceId) {
      return undefined;
    }
    const handleMove = (event) => {
      trackPointerOverSubmitMergeGroupSource(event);
    };
    const handleUp = () => {
      endSubmitMergeGroupSourceDrag();
    };
    window.addEventListener("pointermove", handleMove);
    window.addEventListener("pointerup", handleUp);
    window.addEventListener("pointercancel", handleUp);
    return () => {
      window.removeEventListener("pointermove", handleMove);
      window.removeEventListener("pointerup", handleUp);
      window.removeEventListener("pointercancel", handleUp);
    };
  }, [draggingSubmitMergeGroupSource]);
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
    setSubmissionConfig((prev) => ({
      ...prev,
      partitionId,
    }));
  };
  const partitionSelectValue = resolvePartitionSelectValue(
    submissionConfig.partitionId,
  );
  const activitySelectOptions = (() => {
    const currentId = Number(submissionConfig.activityTopicId || 0);
    if (!currentId) {
      return activityOptions;
    }
    const exists = activityOptions.some((item) => item.topicId === currentId);
    if (exists || !submissionConfig.activityTitle) {
      return activityOptions;
    }
    return [
      {
        topicId: currentId,
        missionId: Number(submissionConfig.activityMissionId || 0),
        name: submissionConfig.activityTitle,
        description: "",
        activityText: "",
        activityDescription: "",
        showActivityIcon: false,
      },
      ...activityOptions,
    ];
  })();

  const activeRecordStatus = useMemo(() => {
    const tab = recordTabs.find((item) => item.key === recordTab);
    return tab ? tab.status : 0;
  }, [recordTab]);

  useEffect(() => {
    const loadDefaultPath = async () => {
      try {
        const data = await invokeCommand("get_download_settings");
        if (data?.downloadPath) {
          setDefaultDownloadPath(data.downloadPath);
        }
      } catch (error) {
        setMessage(error.message);
      }
    };
    loadDefaultPath();
  }, []);

  useEffect(() => {
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
    const loadBaiduSyncSettings = async () => {
      try {
        const data = await invokeCommand("baidu_sync_settings");
        setDefaultBaiduSyncPath(data?.targetPath || "/录播");
      } catch (_) {}
    };
    loadBaiduSyncSettings();
  }, []);

  const loadDownloadList = async (status = activeRecordStatus) => {
    setLoadingDownloads(true);
    setMessage("");
    try {
      if (status === 1) {
        const [downloading, paused] = await Promise.all([
          invokeCommand("download_list_by_status", { status: 1 }),
          invokeCommand("download_list_by_status", { status: 4 }),
        ]);
        const merged = [...(downloading || []), ...(paused || [])].sort(
          (a, b) => (b.id || 0) - (a.id || 0),
        );
        setDownloadList(merged);
      } else {
        const data = await invokeCommand("download_list_by_status", { status });
        setDownloadList(data || []);
      }
    } catch (error) {
      setMessage(error.message);
    } finally {
      setLoadingDownloads(false);
    }
  };

  const handleOpenRecordFolder = async (record) => {
    const folderPath = toFolderPath(record?.localPath);
    if (!folderPath) {
      setMessage("无法打开：缺少本地路径");
      return;
    }
    try {
      const { openPath } = await import("@tauri-apps/plugin-opener");
      await openPath(folderPath);
    } catch (error) {
      setMessage(error?.message || "打开文件夹失败");
    }
  };

  useEffect(() => {
    if (mainTab === "records") {
      loadDownloadList();
    }
  }, [mainTab, activeRecordStatus]);

  useEffect(() => {
    if (mainTab !== "records") {
      return undefined;
    }
    const timer = setInterval(() => {
      loadDownloadList();
    }, 3000);
    return () => clearInterval(timer);
  }, [mainTab, activeRecordStatus]);

  useEffect(() => {
    if (mainTab !== "download") {
      setDownloadStep("select");
    }
  }, [mainTab]);

  useEffect(() => {
    if (!integrationEnabled) {
      return;
    }
    loadPartitions();
    loadCollections();
  }, [integrationEnabled, selectedBilibiliUid]);

  useEffect(() => {
    if (!integrationEnabled) {
      setActivityOptions([]);
      setActivityMessage("");
      return;
    }
    const partitionId = Number(submissionConfig.partitionId || 0);
    if (!partitionId) {
      setActivityOptions([]);
      clearActivitySelection();
      return;
    }
    loadActivities(partitionId);
  }, [
    integrationEnabled,
    submissionConfig.partitionId,
    partitions,
    selectedBilibiliUid,
  ]);

  useEffect(() => {
    if (!quickFillOpen) {
      return undefined;
    }
    loadQuickFillTasks(quickFillPage);
    return undefined;
  }, [quickFillOpen, quickFillPage, quickFillSearch, selectedBilibiliUid]);

  useEffect(() => {
    setVideoItems((prev) => {
      const isMulti = prev.length > 1;
      let changed = false;
      const next = prev.map((item) => {
        if (item.selectedPartsConfig.length === 0) {
          return item;
        }
        let itemChanged = false;
        const nextConfigs = item.selectedPartsConfig.map((partConfig) => {
          const targetPart = item.parts.find((part) => part.cid === partConfig.cid);
          if (!targetPart) {
            return partConfig;
          }
          const nextPath = buildExpectedFilePath(targetPart, item, isMulti);
          if (nextPath === partConfig.filePath) {
            return partConfig;
          }
          changed = true;
          itemChanged = true;
          return {
            ...partConfig,
            filePath: nextPath,
          };
        });
        if (!itemChanged) {
          return item;
        }
        return {
          ...item,
          selectedPartsConfig: nextConfigs,
        };
      });
      return changed ? next : prev;
    });
  }, [downloadConfig.downloadPath, downloadConfig.downloadName, defaultDownloadPath]);

  const loadPartitions = async (bilibiliUidOverride = "") => {
    try {
      const currentBilibiliUid = Number(
        bilibiliUidOverride || selectedBilibiliUid || currentUpProfile?.uid || 0,
      );
      const data = await invokeCommand("bilibili_partitions", {
        bilibiliUid:
          Number.isFinite(currentBilibiliUid) && currentBilibiliUid > 0
            ? currentBilibiliUid
            : null,
      });
      setPartitions(data || []);
      if ((data || []).length) {
        setSubmissionConfig((prev) => {
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
      const currentBilibiliUid = Number(
        bilibiliUidOverride || selectedBilibiliUid || currentUpProfile?.uid || 0,
      );
      const data = await invokeCommand("bilibili_collections", {
        mid: currentBilibiliUid || 0,
        bilibiliUid:
          Number.isFinite(currentBilibiliUid) && currentBilibiliUid > 0
            ? currentBilibiliUid
            : null,
      });
      const mapped = (data || []).map((item) => ({
        ...item,
        seasonId: item.season_id ?? item.seasonId,
      }));
      setCollections(mapped);
    } catch (error) {
      setMessage(error.message);
    }
  };

  const loadPlayOptions = async (info, part) => {
    if (!info || !part) {
      return;
    }
    try {
      const data = await invokeCommand("video_playurl", {
        bvid: info.bvid,
        cid: String(part.cid),
      });
      const videos = data?.dash?.video || [];
      const resolutionMap = new Map();
      const codecMap = new Map();
      videos.forEach((item) => {
        const id = item?.id;
        if (id && !resolutionMap.has(id)) {
          const height = item?.height;
          let label = height ? `${height}P` : `Q${id}`;
          if (id === 120) label = "4K 超清";
          if (id === 116) label = "1080P 高码率";
          if (id === 112) label = "1080P 高码率";
          if (id === 80) label = "1080P 高清";
          if (id === 64) label = "720P 高清";
          if (id === 32) label = "480P 清晰";
          if (id === 16) label = "360P 流畅";
          resolutionMap.set(id, { value: String(id), label });
        }
        const codec = item?.codecs;
        if (codec) {
          let label = codec;
          if (codec.includes("avc1")) label = "H.264";
          if (codec.includes("hev1") || codec.includes("hvc1")) label = "H.265";
          if (codec.includes("vp09") || codec.includes("vp9")) label = "VP9";
          if (codec.includes("av01")) label = "AV1";
          if (!codecMap.has(label)) {
            codecMap.set(label, { value: codec, label });
          }
        }
      });
      const resolutions = Array.from(resolutionMap.values());
      const codecs = Array.from(codecMap.values());
      if (resolutions.length === 0) {
        resolutions.push(...fallbackResolutions);
      }
      if (codecs.length === 0) {
        codecs.push(...fallbackCodecs);
      }
      const formats = [];
      if (data?.dash) {
        formats.push({ value: "dash", label: "DASH" });
      }
      if (data?.durl) {
        formats.push({ value: "mp4", label: "MP4" });
      }
      if (formats.length === 0) {
        formats.push(...fallbackFormats);
      }
      setAvailableResolutions(resolutions);
      setAvailableCodecs(codecs);
      setAvailableFormats(formats);
      setDownloadConfig((prev) => ({
        ...prev,
        resolution: prev.resolution || resolutions[0]?.value || "",
        codec: prev.codec || codecs[0]?.value || "",
        format: prev.format || formats[0]?.value || "",
      }));
    } catch (error) {
      setMessage(error.message);
      setAvailableResolutions(fallbackResolutions);
      setAvailableCodecs(fallbackCodecs);
      setAvailableFormats(fallbackFormats);
      setDownloadConfig((prev) => ({
        ...prev,
        resolution: prev.resolution || fallbackResolutions[0]?.value || "",
        codec: prev.codec || fallbackCodecs[0]?.value || "",
        format: prev.format || fallbackFormats[0]?.value || "",
      }));
    }
  };

  const fetchProxyImage = async (url) => {
    if (!url) {
      return "";
    }
    try {
      const data = await invokeCommand("video_proxy_image", { url });
      return data || "";
    } catch (error) {
      return "";
    }
  };

  const handleSearch = async () => {
    const rawInputs = extractVideoInputs(searchInput);
    const parsedInputs = [];
    const seen = new Set();
    rawInputs.forEach((raw, index) => {
      const { bvid, aid } = parseVideoInput(raw);
      if (!bvid && !aid) {
        return;
      }
      const key = buildVideoKey(bvid, aid, index);
      if (seen.has(key)) {
        return;
      }
      seen.add(key);
      parsedInputs.push({ bvid, aid, key, raw });
    });
    if (parsedInputs.length === 0) {
      setMessage("请输入正确的 BV 号或 AV 号/链接");
      return;
    }
    setSearching(true);
    setMessage("");
    setAvailableResolutions([]);
    setAvailableCodecs([]);
    setAvailableFormats([]);
    try {
      const nextItems = [];
      const errorMessages = [];
      for (const input of parsedInputs) {
        try {
          const data = await invokeCommand("video_detail", { bvid: input.bvid, aid: input.aid });
          const pages = Array.isArray(data?.pages) ? data.pages : [];
          const coverUrl = await fetchProxyImage(data?.pic);
          const avatarUrl = await fetchProxyImage(data?.owner?.face);
          nextItems.push({
            key: input.key,
            input: input.raw,
            bvid: input.bvid,
            aid: input.aid,
            info: data,
            parts: pages,
            selectedParts: [],
            selectedPartsConfig: [],
            coverUrl,
            avatarUrl,
          });
        } catch (error) {
          const errorMessage = error?.message || "未知错误";
          errorMessages.push(`${input.raw}: ${errorMessage}`);
        }
      }
      setVideoItems(nextItems);
      setDownloadStep("select");
      if (nextItems.length === 1 && nextItems[0]?.info?.title) {
        setDownloadConfig((prev) => ({
          ...prev,
          downloadName: nextItems[0].info.title,
        }));
      }
      if (nextItems.length !== 1) {
        setDownloadConfig((prev) => ({
          ...prev,
          downloadName: "",
        }));
      }
      if (nextItems.length > 0 && nextItems[0].parts.length > 0) {
        await loadPlayOptions(nextItems[0].info, nextItems[0].parts[0]);
      }
      if (errorMessages.length > 0) {
        setMessage(`部分视频获取失败：${errorMessages[0]}`);
      }
    } catch (error) {
      setMessage(error.message);
    } finally {
      setSearching(false);
    }
  };

  const buildExpectedFilePath = (part, item, isMulti) => {
    const basePath = normalizePath(downloadConfig.downloadPath || defaultDownloadPath);
    const folderName = sanitizeFilename(
      isMulti
        ? item?.info?.title || "未知"
        : downloadConfig.downloadName || item?.info?.title || "未知",
    );
    const fileName = `${sanitizeFilename(part.part)}.mp4`;
    if (!basePath) {
      return `${folderName}/${fileName}`;
    }
    return `${basePath}/${folderName}/${fileName}`;
  };

  const buildSelectedPartsConfig = (item, selectedParts, isMulti) => {
    return selectedParts.map((part) => {
      const partKey = buildPartKey(item.key, part.cid);
      const existing = item.selectedPartsConfig.find((config) => config.key === partKey);
      const defaultPath = buildExpectedFilePath(part, item, isMulti);
      return {
        key: partKey,
        cid: part.cid,
        title: part.part,
        filePath: defaultPath,
        startTime: existing?.startTime || "00:00:00",
        endTime: existing?.endTime || formatDurationHms(part.duration),
      };
    });
  };

  const togglePart = (videoKey, part) => {
    setVideoItems((prev) => {
      const isMulti = prev.length > 1;
      return prev.map((item) => {
        if (item.key !== videoKey) {
          return item;
        }
        const exists = item.selectedParts.some((selected) => selected.cid === part.cid);
        let nextSelected;
        if (exists) {
          nextSelected = item.selectedParts.filter((selected) => selected.cid !== part.cid);
        } else {
          nextSelected = [...item.selectedParts, part];
          nextSelected.sort(
            (left, right) =>
              item.parts.findIndex((target) => target.cid === left.cid) -
              item.parts.findIndex((target) => target.cid === right.cid),
          );
        }
        const nextConfigs = buildSelectedPartsConfig(item, nextSelected, isMulti);
        return {
          ...item,
          selectedParts: nextSelected,
          selectedPartsConfig: nextConfigs,
        };
      });
    });
  };

  const toggleSelectAll = (videoKey) => {
    setVideoItems((prev) => {
      const isMulti = prev.length > 1;
      return prev.map((item) => {
        if (item.key !== videoKey) {
          return item;
        }
        const allSelected = item.parts.length > 0 && item.selectedParts.length === item.parts.length;
        const nextSelected = allSelected ? [] : [...item.parts];
        const nextConfigs = buildSelectedPartsConfig(item, nextSelected, isMulti);
        return {
          ...item,
          selectedParts: nextSelected,
          selectedPartsConfig: nextConfigs,
        };
      });
    });
  };

  const toggleSelectAllVideos = () => {
    setVideoItems((prev) => {
      const isMulti = prev.length > 1;
      const nextAllSelected =
        prev.length > 0 &&
        prev.every((item) => item.parts.length > 0 && item.selectedParts.length === item.parts.length);
      return prev.map((item) => {
        const nextSelected = nextAllSelected ? [] : [...item.parts];
        const nextConfigs = buildSelectedPartsConfig(item, nextSelected, isMulti);
        return {
          ...item,
          selectedParts: nextSelected,
          selectedPartsConfig: nextConfigs,
        };
      });
    });
  };

  const updatePartConfig = (partKey, field, value) => {
    setVideoItems((prev) =>
      prev.map((item) => {
        const exists = item.selectedPartsConfig.some((part) => part.key === partKey);
        if (!exists) {
          return item;
        }
        const nextConfigs = item.selectedPartsConfig.map((part) =>
          part.key === partKey ? { ...part, [field]: value } : part,
        );
        return {
          ...item,
          selectedPartsConfig: nextConfigs,
        };
      }),
    );
  };

  const addTag = (value) => {
    const nextTag = value.trim();
    if (!nextTag || tags.includes(nextTag)) {
      return;
    }
    setTags((prev) => [...prev, nextTag]);
  };

  const removeTag = (target) => {
    setTags((prev) => prev.filter((tag) => tag !== target));
    if (target === submissionConfig.activityTitle) {
      setSubmissionConfig((prev) => ({
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

  const normalizeActivityOptions = (items) => {
    return (items || [])
      .map((item) => ({
        topicId: Number(item?.topicId ?? item?.topic_id ?? 0),
        missionId: Number(item?.missionId ?? item?.mission_id ?? 0),
        name: item?.name || item?.topicName || item?.topic_name || "",
        description: item?.description || "",
        activityText: item?.activityText || item?.activity_text || "",
        activityDescription: item?.activityDescription || item?.activity_description || "",
        showActivityIcon: Boolean(
          item?.showActivityIcon ?? item?.show_activity_icon ?? false,
        ),
      }))
      .filter((item) => item.topicId > 0 && item.name);
  };

  const applyActivitySelection = (activity) => {
    const previousTitle = submissionConfig.activityTitle || "";
    const nextTitle = activity?.name || "";
    setSubmissionConfig((prev) => ({
      ...prev,
      activityTopicId: activity ? String(activity.topicId) : "",
      activityMissionId: activity ? String(activity.missionId || "") : "",
      activityTitle: nextTitle,
    }));
    setTags((prev) => {
      let next = prev.filter((tag) => tag !== previousTitle);
      if (nextTitle && !next.includes(nextTitle)) {
        next = [...next, nextTitle];
      }
      return next;
    });
  };

  const clearActivitySelection = () => {
    const previousTitle = submissionConfig.activityTitle || "";
    setSubmissionConfig((prev) => ({
      ...prev,
      activityTopicId: "",
      activityMissionId: "",
      activityTitle: "",
    }));
    if (previousTitle) {
      setTags((prev) => prev.filter((tag) => tag !== previousTitle));
    }
  };

  const loadActivities = async (partitionId, bilibiliUidOverride = "") => {
    setActivityLoading(true);
    setActivityMessage("");
    try {
      const currentBilibiliUid = Number(
        bilibiliUidOverride || selectedBilibiliUid || currentUpProfile?.uid || 0,
      );
      const data = await invokeCommand("bilibili_topics", {
        partitionId: partitionId ? Number(partitionId) : null,
        bilibiliUid:
          Number.isFinite(currentBilibiliUid) && currentBilibiliUid > 0
            ? currentBilibiliUid
            : null,
      });
      const mapped = normalizeActivityOptions(data);
      setActivityOptions(mapped);
      const currentId = Number(submissionConfig.activityTopicId || 0);
      if (currentId > 0 && mapped.length > 0 && !mapped.some((item) => item.topicId === currentId)) {
        clearActivitySelection();
      }
    } catch (error) {
      setActivityOptions([]);
      setActivityMessage(error.message);
    } finally {
      setActivityLoading(false);
    }
  };

  const handleActivityChange = (event) => {
    const value = event.target.value;
    if (!value) {
      applyActivitySelection(null);
      return;
    }
    const target = activityOptions.find((item) => String(item.topicId) === value);
    if (!target) {
      applyActivitySelection(null);
      return;
    }
    applyActivitySelection(target);
  };

  const loadQuickFillTasks = async (page = quickFillPage, keyword = quickFillSearch) => {
    try {
      try {
        await invokeCommand("auth_client_log", {
          message: `quick_fill_request page=${page} size=${quickFillPageSize}`,
        });
      } catch (_) {}
      const payload = { page, page_size: quickFillPageSize, pageSize: quickFillPageSize };
      const currentBilibiliUid = Number(selectedBilibiliUid || currentUpProfile?.uid || 0);
      if (Number.isFinite(currentBilibiliUid) && currentBilibiliUid > 0) {
        payload.bilibiliUid = currentBilibiliUid;
      }
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

  const openQuickFill = () => {
    setQuickFillOpen(true);
    setQuickFillPage(1);
  };

  const closeQuickFill = () => {
    setQuickFillOpen(false);
  };

  const handleQuickFillSelect = (task) => {
    const tagList = String(task.tags || "")
      .split(",")
      .map((tag) => tag.trim())
      .filter(Boolean);
    setSubmissionConfig((prev) => ({
      ...prev,
      title: task.title || "",
      description: task.description || "",
      partitionId: task.partitionId ? String(task.partitionId) : prev.partitionId,
      collectionId: task.collectionId ? String(task.collectionId) : "",
      activityTopicId: task.topicId ? String(task.topicId) : "",
      activityMissionId: task.missionId ? String(task.missionId) : "",
      activityTitle: task.activityTitle || "",
      videoType: task.videoType || "ORIGINAL",
      segmentPrefix: task.segmentPrefix || "",
      priority: Boolean(task.priority),
      baiduSyncEnabled: Boolean(task.baiduSyncEnabled),
      baiduSyncPath: task.baiduSyncPath || "",
      baiduSyncFilename: task.baiduSyncFilename || "",
    }));
    setTags(tagList);
    setTagInput("");
    closeQuickFill();
  };

  const handleOpenSyncPicker = () => {
    setSyncPickerOpen(true);
  };

  const handleCloseSyncPicker = () => {
    setSyncPickerOpen(false);
  };

  const handleConfirmSyncPicker = (path) => {
    setSubmissionConfig((prev) => ({ ...prev, baiduSyncPath: path }));
    setSyncPickerOpen(false);
  };

  const handleSyncPathChange = (path) => {
    setSubmissionConfig((prev) => ({ ...prev, baiduSyncPath: path }));
  };

  const ensureDownloadPathReady = async () => {
    const effectivePath = downloadConfig.downloadPath || defaultDownloadPath;
    await logClient(`download_path_check:start:${effectivePath || "empty"}`);
    if (!effectivePath) {
      setMessage("需要先配置下载路径才可进行下载");
      await logClient("download_path_check:empty");
      return false;
    }
    try {
      await invokeCommand("validate_directory", { path: effectivePath });
      await logClient("download_path_check:ok");
      return true;
    } catch (error) {
      setMessage("需要先配置下载路径才可进行下载");
      await logClient(`download_path_check:error:${error?.message || "unknown"}`);
      return false;
    }
  };

  const handleDownload = async () => {
    await logClient(`download_submit:normal:start parts=${selectedCount}`);
    if (selectedCount === 0) {
      setMessage("请至少选择一个分P");
      await logClient("download_submit:normal:empty_parts");
      return false;
    }
    setMessage("");
    if (!(await ensureDownloadPathReady())) {
      await logClient("download_submit:normal:invalid_path");
      return false;
    }
    try {
      const baseConfig = {
        downloadPath: downloadConfig.downloadPath || null,
        resolution: downloadConfig.resolution || null,
        codec: downloadConfig.codec || null,
        format: downloadConfig.format || null,
        content: downloadConfig.content || null,
      };
      const downloadName = isMultiVideo ? null : downloadConfig.downloadName || null;
      const requests = selectedVideoItems.map((item) => ({
        videoUrl: item.input,
        parts: item.selectedParts.map((part) => ({
          cid: part.cid,
          title: part.part,
          duration: part.duration,
        })),
        config: {
          ...baseConfig,
          downloadName,
        },
      }));
      for (const request of requests) {
        await invokeCommand("download_video", { payload: request });
      }
      await logClient("download_submit:normal:ok");
      if (mainTab === "records") {
        await loadDownloadList();
      }
      return true;
    } catch (error) {
      const errorMessage = error?.message || String(error) || "请求失败";
      await logClient(`download_submit:normal:error:${errorMessage}`);
      setMessage(errorMessage);
      return false;
    }
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

  const isValidTimeFormat = (value) => {
    if (!value) {
      return true;
    }
    return /^([0-1]?[0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9]$/.test(value);
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

  const syncMergeItemsWithParts = (parts, prevItems) => {
    const sourceIds = (parts || [])
      .map((item) => item?.key)
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

  const toggleSubmitMergeSelection = (sourceId, checked, items = submitMergeItems) => {
    const grouped = buildGroupedSourceIdSet(items);
    if (grouped.has(sourceId)) {
      return;
    }
    const item = items.find((i) => i.type === "SOURCE" && i.sourceId === sourceId);
    if (item?.standalone) {
      return;
    }
    setSubmitMergeSelection((prev) => {
      const next = new Set(prev);
      if (checked) {
        next.add(sourceId);
      } else {
        next.delete(sourceId);
      }
      return next;
    });
  };

  const toggleSubmitMergeStandalone = (sourceId) => {
    setSubmitMergeItems((prev) =>
      prev.map((item) => {
        if (item.type !== "SOURCE" || item.sourceId !== sourceId) {
          return item;
        }
        const next = { ...item, standalone: !item.standalone };
        if (next.standalone) {
          setSubmitMergeSelection((sel) => {
            const s = new Set(sel);
            s.delete(sourceId);
            return s;
          });
        }
        return next;
      }),
    );
  };

  const createSubmitMergeGroup = () => {
    const selected = Array.from(submitMergeSelection);
    if (selected.length < 2) {
      setMessage("请至少选择两个源视频进行合并");
      return;
    }
    const selectedSet = new Set(selected);
    const orderedSelected = selectedPartsConfig
      .map((item) => item.key)
      .filter((id) => selectedSet.has(id));
    if (orderedSelected.length < 2) {
      setMessage("请选择有效的源视频进行合并");
      return;
    }
    const groupId = buildMergeGroupId();
    setSubmitMergeItems((prev) => {
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
    setSubmitMergeSelection(new Set());
  };

  const releaseSubmitMergeGroup = (groupId) => {
    const normalizedGroupId = String(groupId || "").trim();
    if (!normalizedGroupId) {
      return;
    }
    setSubmitMergeItems((prev) => {
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
        setSubmitMergeSelection((selection) => {
          const filtered = new Set(selection);
          releasedIds.forEach((id) => filtered.delete(id));
          return filtered;
        });
      }
      return syncMergeItemsWithParts(selectedPartsConfig, next);
    });
  };

  const reorderSubmitMergeItems = (sourceId, targetId) => {
    const normalizedSourceId = String(sourceId || "").trim();
    const normalizedTargetId = String(targetId || "").trim();
    if (!normalizedSourceId || !normalizedTargetId || normalizedSourceId === normalizedTargetId) {
      return;
    }
    setSubmitMergeItems((prev) => {
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

  const handleSubmitMergeItemPointerDown = (event, itemId) => {
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
    submitMergeDragStateRef.current = { activeId: normalizedId, overId: normalizedId };
    setDraggingSubmitMergeItemId(normalizedId);
  };

  const trackPointerOverSubmitMergeItem = (event) => {
    const { activeId } = submitMergeDragStateRef.current;
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
    if (!overId || overId === submitMergeDragStateRef.current.overId) {
      return;
    }
    submitMergeDragStateRef.current.overId = overId;
    reorderSubmitMergeItems(activeId, overId);
  };

  const endSubmitMergeItemDrag = () => {
    const { activeId } = submitMergeDragStateRef.current;
    if (!activeId) {
      return;
    }
    submitMergeDragStateRef.current = { activeId: "", overId: "" };
    setDraggingSubmitMergeItemId("");
  };

  const reorderSubmitMergeGroupSources = (groupId, sourceId, targetId) => {
    const normalizedGroupId = String(groupId || "").trim();
    const normalizedSourceId = String(sourceId || "").trim();
    const normalizedTargetId = String(targetId || "").trim();
    if (!normalizedGroupId || !normalizedSourceId || !normalizedTargetId) {
      return;
    }
    if (normalizedSourceId === normalizedTargetId) {
      return;
    }
    setSubmitMergeItems((prev) =>
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

  const handleSubmitMergeGroupSourcePointerDown = (event, groupId, sourceId) => {
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
    submitMergeGroupDragStateRef.current = {
      groupId: normalizedGroupId,
      activeId: normalizedSourceId,
      overId: normalizedSourceId,
    };
    setDraggingSubmitMergeGroupSource({
      groupId: normalizedGroupId,
      sourceId: normalizedSourceId,
    });
  };

  const trackPointerOverSubmitMergeGroupSource = (event) => {
    const { groupId, activeId } = submitMergeGroupDragStateRef.current;
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
    if (overId === submitMergeGroupDragStateRef.current.overId) {
      return;
    }
    submitMergeGroupDragStateRef.current.overId = overId;
    reorderSubmitMergeGroupSources(groupId, activeId, overId);
  };

  const endSubmitMergeGroupSourceDrag = () => {
    const { activeId } = submitMergeGroupDragStateRef.current;
    if (!activeId) {
      return;
    }
    submitMergeGroupDragStateRef.current = { groupId: "", activeId: "", overId: "" };
    setDraggingSubmitMergeGroupSource({ groupId: "", sourceId: "" });
  };

  const buildSubmitMergeGroupsPayload = () => {
    const hasGroup = submitMergeItems.some((item) => item.type === "GROUP");
    const hasStandalone = submitMergeItems.some((item) => item.type === "SOURCE" && item.standalone);
    if (!hasGroup && !hasStandalone) {
      return [];
    }
    const sourceMap = new Map(
      (selectedPartsConfig || [])
        .filter((item) => item?.key)
        .map((item) => [item.key, item]),
    );
    const groups = [];
    const remainingSources = [];
    for (const item of submitMergeItems) {
      if (item.type === "GROUP") {
        const groupSources = (item.sourceIds || [])
          .map((sourceId) => sourceMap.get(sourceId))
          .filter(Boolean)
          .map((source, index) => ({
            sourceFilePath: source.filePath,
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
              sourceFilePath: source.filePath,
              startTime: source.startTime || null,
              endTime: source.endTime || null,
              sortOrder: 1,
            },
          ],
        });
      } else {
        remainingSources.push({
          sourceFilePath: source.filePath,
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

  const resolvePartIndexById = (sourceId) =>
    selectedPartsConfig.findIndex((item) => item.key === sourceId);

  const renderSubmitMergeRow = ({
    sourceId,
    displayIndex,
    groupId = "",
    mergeItemId = "",
  }) => {
    const partIndex = resolvePartIndexById(sourceId);
    if (partIndex < 0) {
      return null;
    }
    const part = selectedPartsConfig[partIndex];
    const isGroupedRow = Boolean(groupId);
    const activeMergeId = mergeItemId || sourceId;
    const submitMergeItem = !isGroupedRow ? submitMergeItems.find((i) => i.type === "SOURCE" && i.sourceId === sourceId) : null;
    const isStandalone = submitMergeItem?.standalone ?? false;
    const isDragging =
      isGroupedRow
        ? draggingSubmitMergeGroupSource.groupId === groupId &&
          draggingSubmitMergeGroupSource.sourceId === sourceId
        : draggingSubmitMergeItemId === activeMergeId;
    const dragHandle = (
      <span
        className="cursor-grab select-none text-[var(--desc-color)]"
        onPointerDown={(event) => {
          if (isGroupedRow) {
            handleSubmitMergeGroupSourcePointerDown(event, groupId, sourceId);
            return;
          }
          handleSubmitMergeItemPointerDown(event, activeMergeId);
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
        className={`border-t border-[var(--split-color)] ${isDragging ? "bg-[var(--split-color)]" : ""}`}
      >
        <td className="px-3 py-2 text-[var(--desc-color)]">
          <div className="flex items-center gap-2">
            {dragHandle}
            <span>{displayIndex}</span>
          </div>
        </td>
        <td className="px-3 py-2">
          <input
            type="checkbox"
            checked={submitMergeSelection.has(sourceId)}
            onChange={(event) => toggleSubmitMergeSelection(sourceId, event.target.checked)}
            disabled={submitGroupedSourceIds.has(sourceId) || isStandalone}
          />
        </td>
        <td className="px-3 py-2">
          {!isGroupedRow ? (
            <input
              type="checkbox"
              title="独立分组"
              checked={isStandalone}
              onChange={() => toggleSubmitMergeStandalone(sourceId)}
            />
          ) : (
            <input type="checkbox" disabled checked={false} title="合并组内不可独立分组" />
          )}
        </td>
        <td className="px-3 py-2 text-[var(--content-color)]">{part.videoTitle}</td>
        <td className="px-3 py-2">
          <input value={part.filePath} readOnly className="w-full" />
        </td>
        <td className="px-3 py-2">
          <input
            value={part.startTime}
            onChange={(event) => updatePartConfig(sourceId, "startTime", event.target.value)}
            placeholder="00:00:00"
            className="w-full"
          />
        </td>
        <td className="px-3 py-2">
          <input
            value={part.endTime}
            onChange={(event) => updatePartConfig(sourceId, "endTime", event.target.value)}
            placeholder="00:00:00"
            className="w-full"
          />
        </td>
        <td className="px-3 py-2 text-[var(--desc-color)]">
          {submitGroupedSourceIds.has(sourceId) ? "已合并" : "未合并"}
        </td>
      </tr>
    );
  };

  const validateIntegrationForm = () => {
    if (selectedCount === 0) {
      return { valid: false, message: "请至少选择一个分P" };
    }
    if (!downloadConfig.resolution) {
      return { valid: false, message: "请选择分辨率" };
    }
    if (!downloadConfig.codec) {
      return { valid: false, message: "请选择编码格式" };
    }
    if (!downloadConfig.format) {
      return { valid: false, message: "请选择流媒体格式" };
    }
    if (!submissionConfig.title.trim()) {
      return { valid: false, message: "请输入视频标题" };
    }
    if (submissionConfig.title.length > 80) {
      return { valid: false, message: "视频标题不能超过 80 个字符" };
    }
    if (!submissionConfig.partitionId) {
      return { valid: false, message: "请选择视频分区" };
    }
    if (!submissionConfig.videoType) {
      return { valid: false, message: "请选择视频类型" };
    }
    if (submissionConfig.description && submissionConfig.description.length > 2000) {
      return { valid: false, message: "视频描述不能超过 2000 个字符" };
    }
    const normalizedTags = [...tags];
    if (tagInput.trim()) {
      normalizedTags.push(tagInput.trim());
    }
    const uniqueTags = Array.from(new Set(normalizedTags));
    if (uniqueTags.length === 0) {
      return { valid: false, message: "请填写至少一个投稿标签" };
    }
    if (segmentationEnabled) {
      const segmentDuration = workflowConfig.segmentationConfig.segmentDurationSeconds;
      if (segmentDuration < 30 || segmentDuration > 600) {
        return { valid: false, message: "分段时长必须在 30-600 秒之间" };
      }
    }
    for (let index = 0; index < selectedPartsConfig.length; index += 1) {
      const part = selectedPartsConfig[index];
      if (!isValidTimeFormat(part.startTime)) {
        return { valid: false, message: `第${index + 1}个分P的开始时间格式不正确，请使用 HH:MM:SS` };
      }
      if (!isValidTimeFormat(part.endTime)) {
        return { valid: false, message: `第${index + 1}个分P的结束时间格式不正确，请使用 HH:MM:SS` };
      }
      if (part.startTime && part.endTime) {
        const startSeconds = timeToSeconds(part.startTime);
        const endSeconds = timeToSeconds(part.endTime);
        if (startSeconds >= endSeconds) {
          return { valid: false, message: `第${index + 1}个分P的开始时间必须小于结束时间` };
        }
      }
    }
    return { valid: true };
  };

  const validateDownloadConfig = async () => {
    if (selectedCount === 0) {
      setMessage("请至少选择一个分P");
      return false;
    }
    if (!downloadConfig.resolution) {
      setMessage("请选择分辨率");
      return false;
    }
    if (!downloadConfig.codec) {
      setMessage("请选择编码格式");
      return false;
    }
    if (!downloadConfig.format) {
      setMessage("请选择流媒体格式");
      return false;
    }
    if (!(await ensureDownloadPathReady())) {
      return false;
    }
    return true;
  };

  const handleIntegrationDownload = async () => {
    await logClient(`download_submit:integration:start parts=${selectedCount}`);
    if (!integrationEnabled) {
      const ok = await handleDownload();
      return { ok, errorMessage: ok ? "" : "提交失败" };
    }
    const validation = validateIntegrationForm();
    if (!validation.valid) {
      setMessage(validation.message);
      await logClient(`download_submit:integration:invalid:${validation.message}`);
      return { ok: false, errorMessage: validation.message };
    }
    if (!(await ensureDownloadPathReady())) {
      await logClient("download_submit:integration:invalid_path");
      return { ok: false, errorMessage: "下载目录不可用" };
    }
    setMessage("");
    try {
      const normalizedTags = [...tags];
      if (tagInput.trim()) {
        normalizedTags.push(tagInput.trim());
      }
      const uniqueTags = Array.from(new Set(normalizedTags));
      const baseConfig = {
        downloadPath: downloadConfig.downloadPath || null,
        resolution: downloadConfig.resolution || null,
        codec: downloadConfig.codec || null,
        format: downloadConfig.format || null,
        content: downloadConfig.content || null,
      };
      const downloadName = isMultiVideo ? null : downloadConfig.downloadName || null;
      const downloadRequests = selectedVideoItems.map((item) => ({
        videoUrl: item.input,
        parts: item.selectedParts.map((part) => ({
          cid: part.cid,
          title: part.part,
          duration: part.duration,
        })),
        config: {
          ...baseConfig,
          downloadName,
        },
      }));
      const mergeGroups = buildSubmitMergeGroupsPayload();
      const payload = {
        enableSubmission: true,
        workflowConfig: buildWorkflowConfig(mergeGroups),
        downloadRequests,
        submissionRequest: {
          title: submissionConfig.title,
          description: submissionConfig.description || null,
          partitionId: Number(submissionConfig.partitionId),
          tags: uniqueTags.join(","),
          topicId: submissionConfig.activityTopicId
            ? Number(submissionConfig.activityTopicId)
            : null,
          missionId: submissionConfig.activityMissionId
            ? Number(submissionConfig.activityMissionId)
            : null,
          activityTitle: submissionConfig.activityTitle || null,
          videoType: submissionConfig.videoType,
          collectionId: submissionConfig.collectionId
            ? Number(submissionConfig.collectionId)
            : null,
          segmentPrefix: submissionConfig.segmentPrefix || null,
          priority: Boolean(submissionConfig.priority),
          baiduSyncEnabled: Boolean(submissionConfig.baiduSyncEnabled),
          baiduSyncPath: submissionConfig.baiduSyncPath || null,
          baiduSyncFilename: submissionConfig.baiduSyncFilename || null,
          videoParts: selectedPartsConfig.map((part) => ({
            originalTitle: part.title,
            filePath: part.filePath,
            startTime: part.startTime || null,
            endTime: part.endTime || null,
            cid: part.cid,
          })),
        },
      };
      await invokeCommand("download_video", { payload });
      await logClient("download_submit:integration:ok");
      if (mainTab === "records") {
        await loadDownloadList();
      }
      return { ok: true, errorMessage: "" };
    } catch (error) {
      const errorMessage = error?.message || String(error) || "请求失败";
      await logClient(`download_submit:integration:error:${errorMessage}`);
      setMessage(errorMessage);
      return { ok: false, errorMessage };
    }
  };

  const handleStartConfig = (enableIntegration) => {
    if (!hasSelection) {
      setMessage("请至少选择一个分P");
      return;
    }
    setIntegrationEnabled(enableIntegration);
    setDownloadStep("download");
  };

  const handleNextFromDownloadConfig = async () => {
    await logClient(
      `download_submit:next_click mode=${integrationEnabled ? "integration" : "normal"}`,
    );
    if (integrationEnabled) {
      if (!(await validateDownloadConfig())) {
        await logClient("download_submit:next_invalid");
        return;
      }
      setDownloadStep("submission");
      return;
    }
    const success = await handleDownload();
    if (success) {
      await logClient("download_submit:next_ok");
      setDownloadStep("select");
      setRecordTab("pending");
      setMainTab("records");
      await loadDownloadList(0);
    } else {
      await logClient("download_submit:next_failed");
    }
  };

  const handleSubmitDownload = async () => {
    if (submitSubmitting) {
      return;
    }
    setSubmitSubmitting(true);
    await logClient("download_submit:integration_confirm");
    const result = await handleIntegrationDownload();
    if (result.ok) {
      await logClient("download_submit:integration_ok");
      setDownloadStep("select");
      setRecordTab("pending");
      setMainTab("records");
      await loadDownloadList(0);
      setSubmitSubmitting(false);
    } else {
      await logClient("download_submit:integration_failed");
      await showErrorDialog(result.errorMessage || "请求失败");
      setSubmitSubmitting(false);
    }
  };

  const handleDeleteRecord = async (record, deleteFile = false) => {
    setMessage("");
    try {
      if (!record?.id) {
        setMessage("缺少任务ID，无法删除");
        return;
      }
      await invokeCommand("download_delete", { taskId: record.id, deleteFile });
      await loadDownloadList();
    } catch (error) {
      setMessage(error.message);
    }
  };

  const handleRequestDeleteRecord = (record) => {
    if (!record?.id) {
      setMessage("缺少任务ID，无法删除");
      return;
    }
    setDeleteConfirmRecord(record);
    setDeleteConfirmDeleteFile(false);
  };

  const handleCancelDeleteRecord = () => {
    setDeleteConfirmRecord(null);
    setDeleteConfirmDeleteFile(false);
  };

  const handleConfirmDeleteRecord = () => {
    if (!deleteConfirmRecord) {
      return;
    }
    const record = deleteConfirmRecord;
    const shouldDeleteFile = deleteConfirmDeleteFile;
    setDeleteConfirmRecord(null);
    setDeleteConfirmDeleteFile(false);
    handleDeleteRecord(record, shouldDeleteFile);
  };

  const handleRetryRecord = async (taskId) => {
    setMessage("");
    try {
      await invokeCommand("download_retry", { taskId });
      await loadDownloadList();
    } catch (error) {
      setMessage(error.message);
    }
  };

  const handleResumeRecord = async (taskId) => {
    setMessage("");
    try {
      await invokeCommand("download_resume", { taskId });
      await loadDownloadList();
    } catch (error) {
      setMessage(error.message);
    }
  };

  const buildStatItems = (info) => {
    if (!info?.stat) {
      return [];
    }
    return [
      { label: "播放", value: formatNumber(info.stat.view) },
      { label: "弹幕", value: formatNumber(info.stat.danmaku) },
      { label: "评论", value: formatNumber(info.stat.reply) },
      { label: "点赞", value: formatNumber(info.stat.like) },
      { label: "投币", value: formatNumber(info.stat.coin) },
      { label: "收藏", value: formatNumber(info.stat.favorite) },
      { label: "分享", value: formatNumber(info.stat.share) },
    ];
  };

  const totalClipSeconds = selectedPartsConfig.reduce((acc, part) => {
    const start = timeToSeconds(part.startTime);
    const end = timeToSeconds(part.endTime);
    const clipped = Math.max(0, end - start);
    return acc + clipped;
  }, 0);
  const segmentDurationSeconds =
    Number(workflowConfig.segmentationConfig.segmentDurationSeconds) || 0;
  const estimatedSegments =
    segmentationEnabled && segmentDurationSeconds > 0
      ? Math.ceil(totalClipSeconds / segmentDurationSeconds)
      : 0;

  const emptyRecordText = (() => {
    const tab = recordTabs.find((item) => item.key === recordTab);
    return tab ? `暂无${tab.label}任务。` : "暂无下载任务。";
  })();

  const quickFillTotalPages = Math.max(1, Math.ceil(quickFillTotal / quickFillPageSize));
  const quickFillVisibleTasks = quickFillTasks.slice(0, quickFillPageSize);

  const activeRecordLabel = recordTabs.find((item) => item.key === recordTab)?.label || "下载任务";

  const getRecordBorderColor = (status) => {
    switch (status) {
      case 1:
        return "var(--primary-color)";
      case 2:
        return "#4caf50";
      case 3:
        return "#ff5252";
      case 4:
        return "var(--desc-color)";
      default:
        return "var(--split-color)";
    }
  };

  const getRecordProgressColor = (status) => {
    switch (status) {
      case 2:
        return "#4caf50";
      case 3:
        return "#ff5252";
      case 4:
        return "var(--desc-color)";
      default:
        return "var(--primary-color)";
    }
  };

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h1 className="text-lg font-semibold text-[var(--content-color)]">视频下载</h1>
        <div className="flex gap-2">
          <button
            className={`h-8 px-3 rounded-lg text-sm ${
              mainTab === "download"
                ? "bg-[var(--button-color)] text-[var(--primary-color)]"
                : "text-[var(--desc-color)]"
            }`}
            onClick={() => setMainTab("download")}
          >
            视频下载
          </button>
          <button
            className={`h-8 px-3 rounded-lg text-sm ${
              mainTab === "records"
                ? "bg-[var(--button-color)] text-[var(--primary-color)]"
                : "text-[var(--desc-color)]"
            }`}
            onClick={() => setMainTab("records")}
          >
            下载队列
          </button>
        </div>
      </div>

      {mainTab === "download" ? (
        <div className="space-y-4">
          {downloadStep === "select" ? (
            <>
              <div className="panel p-4">
                <div className="flex flex-wrap items-center justify-between gap-4">
                  <div>
                    <p className="text-xs uppercase tracking-[0.2em] text-[var(--desc-color)]">
                      视频下载
                    </p>
                    <h2 className="text-lg font-semibold text-[var(--content-color)]">视频下载</h2>
                  </div>
                </div>
                <div className="mt-3 grid gap-3 lg:grid-cols-[1fr_auto]">
                  <input
                    value={searchInput}
                    onChange={(event) => setSearchInput(event.target.value)}
                    onKeyDown={(event) => {
                      if (event.key === "Enter") {
                        handleSearch();
                      }
                    }}
                    placeholder="请输入 BV 号或视频链接，可用空格分隔多个"
                    className="w-full"
                  />
                  <button
                    className="h-8 px-4 rounded-lg bg-[var(--primary-color)] text-[var(--primary-text)]"
                    onClick={handleSearch}
                    disabled={searching}
                  >
                    {searching ? "搜索中..." : "搜索"}
                  </button>
                </div>
                {message ? (
                  <div className="mt-3 rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-700">
                    {message}
                  </div>
                ) : null}
              </div>

              {hasVideo ? (
                <>
                  <div className="panel p-4">
                    <div className="flex flex-wrap items-center justify-between gap-3">
                      <div className="text-sm font-semibold text-[var(--content-color)]">
                        已选 {selectedCount} 个分P / {videoItems.length} 个视频
                      </div>
                      <div className="flex flex-wrap gap-2">
                        {isMultiVideo ? (
                          <button className="h-8 px-3 rounded-lg" onClick={toggleSelectAllVideos}>
                            {allVideosSelected ? "取消全选全部视频" : "全选全部视频"}
                          </button>
                        ) : null}
                        <button
                          className="h-8 px-3 rounded-lg bg-[var(--primary-color)] text-[var(--primary-text)]"
                          onClick={() => handleStartConfig(false)}
                          disabled={!hasSelection}
                        >
                          常规下载
                        </button>
                        <button
                          className="h-8 px-3 rounded-lg"
                          onClick={() => handleStartConfig(true)}
                          disabled={!hasSelection}
                        >
                          下载+投稿
                        </button>
                      </div>
                    </div>
                    {isMultiVideo ? (
                      <div className="mt-2 text-xs text-[var(--desc-color)]">
                        多视频模式下默认按各自视频标题创建下载目录。
                      </div>
                    ) : null}
                  </div>

                  {videoItems.map((item) => {
                    const stats = buildStatItems(item.info);
                    const coverSrc = item.coverUrl || item.info?.pic;
                    const avatarSrc = item.avatarUrl || item.info?.owner?.face;
                    const allSelected =
                      item.selectedParts.length > 0 && item.selectedParts.length === item.parts.length;
                    return (
                      <div key={item.key} className="space-y-3">
                        <div className="panel p-4">
                          <div className="flex flex-wrap gap-4">
                            <div className="h-28 w-44 overflow-hidden rounded-lg bg-[var(--solid-button-color)]">
                              {coverSrc ? (
                                <img src={coverSrc} alt="封面" className="h-full w-full object-cover" />
                              ) : (
                                <div className="flex h-full w-full items-center justify-center text-xs text-[var(--desc-color)]">
                                  无封面
                                </div>
                              )}
                            </div>
                            <div className="flex-1">
                              <h3 className="text-base font-semibold text-[var(--content-color)]">
                                {item.info?.title || "未知标题"}
                              </h3>
                              <p className="mt-2 text-sm text-[var(--desc-color)]">
                                {item.info?.desc}
                              </p>
                              <div className="mt-3 flex flex-wrap gap-3 text-xs text-[var(--desc-color)]">
                                {stats.map((stat) => (
                                  <span key={stat.label}>
                                    {stat.label}: {stat.value}
                                  </span>
                                ))}
                              </div>
                            </div>
                            <div className="flex flex-col items-center gap-2">
                              <div className="h-12 w-12 overflow-hidden rounded-full bg-[var(--solid-button-color)]">
                                {avatarSrc ? (
                                  <img src={avatarSrc} alt="UP主" className="h-full w-full object-cover" />
                                ) : (
                                  <div className="flex h-full w-full items-center justify-center text-xs text-[var(--desc-color)]">
                                    UP
                                  </div>
                                )}
                              </div>
                              <div className="text-xs text-[var(--content-color)]">
                                {item.info?.owner?.name || "未知UP主"}
                              </div>
                            </div>
                          </div>
                        </div>

                        <div className="mt-4 flex flex-wrap items-center justify-between gap-3">
                          <div className="text-sm font-semibold text-[var(--content-color)]">
                            分P列表（共 {item.parts.length} 个）
                          </div>
                          <button className="h-8 px-3 rounded-lg" onClick={() => toggleSelectAll(item.key)}>
                            {allSelected ? "取消全选" : "全选"}
                          </button>
                        </div>

                        <div className="mt-3 overflow-hidden rounded-lg border border-[var(--split-color)]">
                          <table className="w-full text-left text-sm">
                            <thead className="bg-[var(--solid-button-color)] text-xs uppercase tracking-[0.2em] text-[var(--desc-color)]">
                              <tr>
                                <th className="px-3 py-2"></th>
                                <th className="px-3 py-2">分P标题</th>
                                <th className="px-3 py-2">时长</th>
                              </tr>
                            </thead>
                            <tbody>
                              {item.parts.map((part) => {
                                const checked = item.selectedParts.some((selected) => selected.cid === part.cid);
                                return (
                                  <tr key={part.cid} className="border-t border-[var(--split-color)]">
                                    <td className="px-3 py-2">
                                      <input
                                        type="checkbox"
                                        checked={checked}
                                        onChange={() => togglePart(item.key, part)}
                                      />
                                    </td>
                                    <td className="px-3 py-2 text-[var(--content-color)]">{part.part}</td>
                                    <td className="px-3 py-2 text-[var(--desc-color)]">
                                      {formatDuration(part.duration)}
                                    </td>
                                  </tr>
                                );
                              })}
                            </tbody>
                          </table>
                        </div>
                      </div>
                    );
                  })}

                  <div className="mt-3 text-xs text-[var(--desc-color)]">
                    勾选分P后点击下载进入配置
                  </div>
                </>
              ) : null}
            </>
          ) : downloadStep === "download" ? (
            <>
              <div className="panel p-4">
                <div className="flex flex-wrap items-center justify-between gap-3">
                  <div>
                    <p className="text-xs uppercase tracking-[0.2em] text-[var(--desc-color)]">
                      视频下载
                    </p>
                    <h2 className="text-lg font-semibold text-[var(--content-color)]">下载配置</h2>
                  </div>
                  <button className="h-8 px-3 rounded-lg" onClick={() => setDownloadStep("select")}>
                    返回选择分P
                  </button>
                </div>
                {message ? (
                  <div className="mt-3 rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-700">
                    {message}
                  </div>
                ) : null}
              </div>

              {hasVideo ? (
                <div className="grid gap-4 lg:grid-cols-[1.2fr_0.8fr]">
                  <div className="panel p-4">
                    <div className="text-sm font-semibold text-[var(--content-color)]">
                      已选分P（{selectedCount}）
                    </div>
                    <div className="mt-3 overflow-hidden rounded-lg border border-[var(--split-color)]">
                      <table className="w-full text-left text-sm">
                        <thead className="bg-[var(--solid-button-color)] text-xs uppercase tracking-[0.2em] text-[var(--desc-color)]">
                          <tr>
                            <th className="px-3 py-2">视频标题</th>
                            <th className="px-3 py-2">分P标题</th>
                            <th className="px-3 py-2">时长</th>
                          </tr>
                        </thead>
                        <tbody>
                          {selectedParts.map((part) => (
                            <tr key={buildPartKey(part.videoKey, part.cid)} className="border-t border-[var(--split-color)]">
                              <td className="px-3 py-2 text-[var(--content-color)]">
                                {part.videoTitle}
                              </td>
                              <td className="px-3 py-2 text-[var(--content-color)]">{part.part}</td>
                              <td className="px-3 py-2 text-[var(--desc-color)]">
                                {formatDuration(part.duration)}
                              </td>
                            </tr>
                          ))}
                        </tbody>
                      </table>
                    </div>
                  </div>

                  <div className="space-y-4">
                    <div className="panel p-4">
                      <div className="text-xs uppercase tracking-[0.2em] text-[var(--desc-color)]">
                        下载配置
                      </div>
                      <div className="mt-3 space-y-3 text-sm text-[var(--content-color)]">
                        <input
                          value={downloadConfig.downloadName}
                          onChange={(event) =>
                            setDownloadConfig((prev) => ({
                              ...prev,
                              downloadName: event.target.value,
                            }))
                          }
                          placeholder={isMultiVideo ? "多视频模式自动使用视频标题" : "下载名称"}
                          disabled={isMultiVideo}
                          className="w-full"
                        />
                        <input
                          value={downloadConfig.downloadPath}
                          onChange={(event) =>
                            setDownloadConfig((prev) => ({
                              ...prev,
                              downloadPath: event.target.value,
                            }))
                          }
                          placeholder={
                            defaultDownloadPath
                              ? `下载路径（默认 ${defaultDownloadPath}）`
                              : "下载路径（默认 系统下载目录）"
                          }
                          className="w-full"
                        />
                        <div className="grid gap-2">
                          <select
                            value={downloadConfig.resolution}
                            onChange={(event) =>
                              setDownloadConfig((prev) => ({
                                ...prev,
                                resolution: event.target.value,
                              }))
                            }
                            className="w-full"
                          >
                            <option value="">分辨率</option>
                            {availableResolutions.map((item) => (
                              <option key={item.value} value={item.value}>
                                {item.label}
                              </option>
                            ))}
                          </select>
                          <select
                            value={downloadConfig.codec}
                            onChange={(event) =>
                              setDownloadConfig((prev) => ({
                                ...prev,
                                codec: event.target.value,
                              }))
                            }
                            className="w-full"
                          >
                            <option value="">编码格式</option>
                            {availableCodecs.map((item) => (
                              <option key={item.value} value={item.value}>
                                {item.label}
                              </option>
                            ))}
                          </select>
                          <select
                            value={downloadConfig.format}
                            onChange={(event) =>
                              setDownloadConfig((prev) => ({
                                ...prev,
                                format: event.target.value,
                              }))
                            }
                            className="w-full"
                          >
                            <option value="">流媒体格式</option>
                            {availableFormats.map((item) => (
                              <option key={item.value} value={item.value}>
                                {item.label}
                              </option>
                            ))}
                          </select>
                          <select
                            value={downloadConfig.content}
                            onChange={(event) =>
                              setDownloadConfig((prev) => ({
                                ...prev,
                                content: event.target.value,
                              }))
                            }
                            className="w-full"
                          >
                            <option value="audio_video">音视频</option>
                            <option value="video_only">仅视频</option>
                            <option value="audio_only">仅音频</option>
                          </select>
                        </div>
                        {playOptionsEmpty ? (
                          <div className="rounded-lg border border-dashed border-[var(--split-color)] px-3 py-2 text-xs text-[var(--desc-color)]">
                            搜索视频后加载可选分辨率与编码。
                          </div>
                        ) : null}
                      </div>
                    </div>

                    <div className="flex justify-end">
                      <button
                        className="h-9 px-5 rounded-lg bg-[var(--primary-color)] text-[var(--primary-text)]"
                        onClick={handleNextFromDownloadConfig}
                      >
                        {integrationEnabled ? "下一步" : "开始下载"}
                      </button>
                    </div>
                  </div>
                </div>
              ) : null}
            </>
          ) : (
            <>
              <div className="panel p-4">
                <div className="flex flex-wrap items-center justify-between gap-3">
                  <div>
                    <p className="text-xs uppercase tracking-[0.2em] text-[var(--desc-color)]">
                      下载+投稿
                    </p>
                    <h2 className="text-lg font-semibold text-[var(--content-color)]">投稿配置</h2>
                  </div>
                  <button
                    className="h-8 px-3 rounded-lg"
                    onClick={() => setDownloadStep("download")}
                  >
                    返回下载配置
                  </button>
                </div>
                {message ? (
                  <div className="mt-3 rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-700">
                    {message}
                  </div>
                ) : null}
              </div>

              <div className="panel p-4 space-y-4">
                <div className="flex items-center justify-between gap-3">
                  <div className="text-sm font-semibold text-[var(--content-color)]">投稿信息</div>
                  <div className="flex flex-wrap items-center gap-2">
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
                    <button className="h-7 px-3 rounded-lg text-xs" onClick={openQuickFill}>
                      一键填写
                    </button>
                  </div>
                </div>
                <div className="grid gap-3 lg:grid-cols-[1.2fr_0.8fr]">
                  <div className="space-y-3">
                    <div className="space-y-1">
                      <div className="text-xs text-[var(--desc-color)]">
                        投稿标题<span className="ml-1 text-rose-500">必填</span>
                      </div>
                      <input
                        value={submissionConfig.title}
                        onChange={(event) =>
                          setSubmissionConfig((prev) => ({ ...prev, title: event.target.value }))
                        }
                        placeholder="请输入投稿标题"
                        className="w-full"
                      />
                    </div>
                    <div className="space-y-1">
                      <div className="text-xs text-[var(--desc-color)]">视频描述（可选）</div>
                      <textarea
                        value={submissionConfig.description}
                        onChange={(event) =>
                          setSubmissionConfig((prev) => ({
                            ...prev,
                            description: event.target.value,
                          }))
                        }
                        placeholder="视频描述"
                        rows={2}
                        className="w-full"
                      />
                    </div>
                    <div className="grid gap-2 lg:grid-cols-3">
                      <select
                        value={partitionSelectValue}
                        onChange={(event) => handlePartitionChange(event.target.value)}
                        className="w-full"
                      >
                        <option value="">B站分区</option>
                        {partitions.map((partition) => (
                          <option
                            key={partition.tid}
                            value={buildPartitionOptionValue(partition)}
                          >
                            {partition.name}
                          </option>
                        ))}
                      </select>
                      <select
                        value={submissionConfig.collectionId}
                        onChange={(event) =>
                          setSubmissionConfig((prev) => ({
                            ...prev,
                            collectionId: event.target.value,
                          }))
                        }
                        className="w-full"
                      >
                        <option value="">合集（可选）</option>
                        {collections.map((collection) => (
                          <option key={collection.seasonId} value={collection.seasonId}>
                            {collection.name}
                          </option>
                        ))}
                      </select>
                      <select
                        value={submissionConfig.videoType}
                        onChange={(event) =>
                          setSubmissionConfig((prev) => ({
                            ...prev,
                            videoType: event.target.value,
                          }))
                        }
                        className="w-full"
                      >
                        <option value="ORIGINAL">原创</option>
                        <option value="REPOST">转载</option>
                      </select>
                    </div>
                    <div className="space-y-1">
                      <div className="text-xs text-[var(--desc-color)]">
                        投稿标签<span className="ml-1 text-rose-500">必填</span>
                      </div>
                      <div className="rounded-lg border border-[var(--split-color)] bg-white/70 px-3 py-2 text-sm focus-within:border-[var(--primary-color)]">
                        <div className="flex flex-wrap gap-2">
                          {tags.map((tag) => (
                            <span
                              key={tag}
                              className="inline-flex items-center gap-1 rounded-full bg-[var(--primary-color)]/10 px-2 py-1 text-xs text-[var(--primary-color)]"
                            >
                              {tag}
                              <button
                                className="text-[10px] font-semibold text-[var(--primary-color)] hover:opacity-70"
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
                            className="min-w-[120px] flex-1 bg-transparent text-sm text-[var(--content-color)] focus:outline-none"
                          />
                        </div>
                      </div>
                      <div className="mt-2 space-y-1">
                        <div className="text-xs text-[var(--desc-color)]">活动话题（可选）</div>
                        <div className="flex flex-wrap items-center gap-2">
                          <select
                            value={submissionConfig.activityTopicId}
                            onChange={handleActivityChange}
                            disabled={activityLoading || !submissionConfig.partitionId}
                            className="min-w-[200px] flex-1 rounded-lg border border-[var(--split-color)] bg-white/70 px-3 py-2 text-sm"
                          >
                            <option value="">不参与活动</option>
                            {activitySelectOptions.map((activity) => (
                              <option key={activity.topicId} value={activity.topicId}>
                                {activity.showActivityIcon ? "【活动】" : ""}
                                {activity.name}
                                {activity.activityText ? ` · ${activity.activityText}` : ""}
                              </option>
                            ))}
                          </select>
                          <button
                            type="button"
                            onClick={() =>
                              loadActivities(
                                submissionConfig.partitionId,
                                selectedBilibiliUid,
                              )
                            }
                            disabled={activityLoading || !submissionConfig.partitionId}
                            className="rounded-lg border border-[var(--split-color)] bg-white/70 px-3 py-2 text-xs text-[var(--desc-color)] hover:text-[var(--primary-color)] disabled:opacity-60"
                          >
                            刷新活动
                          </button>
                        </div>
                        {activityLoading ? (
                          <div className="text-xs text-[var(--desc-color)]">活动加载中...</div>
                        ) : null}
                        {activityMessage ? (
                          <div className="text-xs text-rose-500">{activityMessage}</div>
                        ) : null}
                      </div>
                    </div>
                    <div className="space-y-1">
                      <div className="text-xs text-[var(--desc-color)]">分段前缀（可选）</div>
                      <input
                        value={submissionConfig.segmentPrefix}
                        onChange={(event) =>
                          setSubmissionConfig((prev) => ({
                            ...prev,
                            segmentPrefix: event.target.value,
                          }))
                        }
                        placeholder="分段前缀"
                        className="w-full"
                      />
                    </div>
                    <label className="flex items-center gap-2 text-xs text-[var(--desc-color)]">
                      <input
                        type="checkbox"
                        checked={submissionConfig.priority}
                        onChange={(event) =>
                          setSubmissionConfig((prev) => ({
                            ...prev,
                            priority: event.target.checked,
                          }))
                        }
                      />
                      优先投稿（进入投稿队列时置顶）
                    </label>
                    <div className="text-xs text-[var(--desc-color)]">
                      分段前缀会作为分段文件名的前缀（可选）
                    </div>
                    <div className="rounded-lg border border-[var(--split-color)] bg-white/70 p-3">
                      <div className="text-xs uppercase tracking-[0.2em] text-[var(--desc-color)]">
                        百度网盘同步
                      </div>
                      <label className="mt-2 flex items-center gap-2 text-xs text-[var(--desc-color)]">
                        <input
                          type="checkbox"
                          checked={submissionConfig.baiduSyncEnabled}
                          onChange={(event) =>
                            setSubmissionConfig((prev) => ({
                              ...prev,
                              baiduSyncEnabled: event.target.checked,
                            }))
                          }
                        />
                        投稿完成后同步上传
                      </label>
                      {submissionConfig.baiduSyncEnabled ? (
                        <div className="mt-3 grid gap-2">
                          <div>
                            <div className="text-xs text-[var(--desc-color)]">远端路径</div>
                            <div className="mt-2 flex flex-wrap items-center gap-2 text-xs">
                              <div className="flex-1 rounded-lg border border-[var(--split-color)] bg-white/70 px-3 py-2 text-[var(--content-color)]">
                                {submissionConfig.baiduSyncPath || defaultBaiduSyncPath || "/录播"}
                              </div>
                              <button
                                className="rounded-lg border border-[var(--split-color)] bg-white/70 px-3 py-1 font-semibold text-[var(--content-color)]"
                                onClick={handleOpenSyncPicker}
                              >
                                选择目录
                              </button>
                            </div>
                          </div>
                          <div>
                            <div className="text-xs text-[var(--desc-color)]">上传文件名</div>
                            <input
                              value={submissionConfig.baiduSyncFilename}
                              onChange={(event) =>
                                setSubmissionConfig((prev) => ({
                                  ...prev,
                                  baiduSyncFilename: event.target.value,
                                }))
                              }
                              placeholder="文件名"
                              className="mt-2 w-full rounded-lg border border-[var(--split-color)] bg-white/70 px-3 py-2 text-sm focus:border-[var(--primary-color)]"
                            />
                          </div>
                        </div>
                      ) : null}
                    </div>
                  </div>

                  <div className="space-y-3">
                    <div className="text-xs uppercase tracking-[0.2em] text-[var(--desc-color)]">
                      工作流配置
                    </div>
                    <div className="space-y-2 text-sm text-[var(--content-color)]">
                      <div className="text-xs uppercase tracking-[0.2em] text-[var(--desc-color)]">
                        是否分段
                      </div>
                      <div className="flex flex-wrap gap-4 text-xs text-[var(--desc-color)]">
                        <label className="flex items-center gap-2">
                          <input
                            type="radio"
                            checked={segmentationEnabled}
                            onChange={() => setSegmentationEnabled(true)}
                          />
                          需要分段
                        </label>
                        <label className="flex items-center gap-2">
                          <input
                            type="radio"
                            checked={!segmentationEnabled}
                            onChange={() => setSegmentationEnabled(false)}
                          />
                          不需要分段
                        </label>
                      </div>
                      <div className="flex flex-wrap gap-3 text-xs text-[var(--desc-color)]">
                        <label className="flex items-center gap-2">
                          <input type="checkbox" checked disabled />
                          启用剪辑
                        </label>
                        <label className="flex items-center gap-2">
                          <input type="checkbox" checked disabled />
                          启用合并
                        </label>
                        <label className="flex items-center gap-2">
                          <input type="checkbox" checked={segmentationEnabled} disabled />
                          启用分段
                        </label>
                      </div>
                      {segmentationEnabled ? (
                        <div className="mt-2 grid gap-2">
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
                            className="w-full"
                          />
                          <label className="flex items-center gap-2 text-xs text-[var(--desc-color)]">
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
                            />
                            保留合并视频
                          </label>
                        </div>
                      ) : null}
                      <div className="text-xs text-[var(--desc-color)]">
                        预计分段数：{segmentationEnabled ? estimatedSegments : "不分段"}
                      </div>
                    </div>
                  </div>
                </div>
              </div>

              {selectedPartsConfig.length > 0 ? (
                <div className="panel p-4">
                  <div className="flex items-center justify-between gap-3">
                    <div className="text-sm font-semibold text-[var(--content-color)]">
                      源视频配置
                    </div>
                    <div className="flex items-center gap-2">
                      <button
                        className="h-8 px-3 rounded-lg border border-[var(--split-color)] text-xs text-[var(--content-color)] disabled:cursor-not-allowed disabled:opacity-60"
                        onClick={createSubmitMergeGroup}
                        disabled={submitMergeSelection.size < 2}
                      >
                        创建合并组
                      </button>
                    </div>
                  </div>
                  <div className="mt-3 overflow-hidden rounded-lg border border-[var(--split-color)]">
                    <table className="w-full text-left text-sm">
                      <thead className="bg-[var(--solid-button-color)] text-xs uppercase tracking-[0.2em] text-[var(--desc-color)]">
                        <tr>
                          <th className="px-3 py-2">序号</th>
                          <th className="px-3 py-2">选择</th>
                          <th className="px-3 py-2">独立分组</th>
                          <th className="px-3 py-2">来源视频</th>
                          <th className="px-3 py-2">视频文件（必填）</th>
                          <th className="px-3 py-2">开始时间</th>
                          <th className="px-3 py-2">结束时间</th>
                          <th className="px-3 py-2">合并状态</th>
                        </tr>
                      </thead>
                      <tbody>
                        {submitMergeItems.length === 0 ? (
                          <tr>
                            <td className="px-3 py-3 text-[var(--desc-color)]" colSpan={8}>
                              暂无配置
                            </td>
                          </tr>
                        ) : (
                          submitMergeItems.map((item, index) => {
                            if (item.type === "GROUP") {
                              const groupSources = (item.sourceIds || []).filter(
                                (sourceId) => resolvePartIndexById(sourceId) >= 0,
                              );
                              if (groupSources.length === 0) {
                                return null;
                              }
                              return (
                                <tr
                                  key={`merge-group-${item.id}`}
                                  data-merge-item-id={item.id}
                                  className="border-t border-[var(--split-color)]"
                                >
                                  <td className="p-0" colSpan={8}>
                                    <div className="m-2 overflow-hidden rounded-lg border-2 border-[var(--primary-color)]/40 bg-white/70">
                                      <div
                                        className="flex items-center justify-between border-b border-[var(--primary-color)]/30 bg-[var(--primary-color)]/10 px-3 py-2 text-xs text-[var(--desc-color)] cursor-grab"
                                        onPointerDown={(event) =>
                                          handleSubmitMergeItemPointerDown(event, item.id)
                                        }
                                        style={{ touchAction: "none" }}
                                      >
                                        <div className="flex items-center gap-2">
                                          <span className="select-none text-[var(--content-color)]">≡</span>
                                          <span className="uppercase tracking-[0.2em]">
                                            合并组 {index + 1}
                                          </span>
                                          <span>{groupSources.length} 个视频</span>
                                        </div>
                                        <button
                                          type="button"
                                          className="h-7 px-2 rounded border border-[var(--split-color)] text-xs text-[var(--content-color)]"
                                          onPointerDown={(event) => event.stopPropagation()}
                                          onClick={(event) => {
                                            event.stopPropagation();
                                            releaseSubmitMergeGroup(item.id);
                                          }}
                                        >
                                          解绑
                                        </button>
                                      </div>
                                      <table className="w-full text-left text-sm">
                                        <tbody>
                                          {groupSources.map((sourceId, innerIndex) =>
                                            renderSubmitMergeRow({
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
                            return renderSubmitMergeRow({
                              sourceId: item.sourceId,
                              displayIndex: index + 1,
                              mergeItemId: item.id,
                            });
                          })
                        )}
                      </tbody>
                    </table>
                  </div>
                </div>
              ) : null}

              <div className="flex justify-end">
                <LoadingButton
                  className="h-9 px-5 rounded-lg bg-[var(--primary-color)] text-[var(--primary-text)]"
                  onClick={handleSubmitDownload}
                  loading={submitSubmitting}
                  loadingLabel="处理中"
                >
                  创建任务
                </LoadingButton>
              </div>
            </>
          )}

          {quickFillOpen && downloadStep === "submission" ? (
            <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/30 px-4">
              <div className="w-full max-w-2xl rounded-2xl bg-white p-5 shadow-lg">
                <div className="flex items-center justify-between gap-3">
                  <div className="text-sm font-semibold text-[var(--content-color)]">一键填写</div>
                  <button className="h-7 px-3 rounded-lg text-xs" onClick={closeQuickFill}>
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
                    className="w-full rounded-lg border border-[var(--split-color)] bg-white px-3 py-2 text-sm text-[var(--content-color)]"
                  />
                </div>
                <div className="mt-3 h-[420px] overflow-y-auto rounded-xl border border-[var(--split-color)]">
                  <table className="w-full text-left text-sm">
                    <thead className="bg-[var(--solid-button-color)] text-xs uppercase tracking-[0.2em] text-[var(--desc-color)]">
                      <tr>
                        <th className="px-4 py-2">投稿标题</th>
                        <th className="px-4 py-2">创建时间</th>
                      </tr>
                    </thead>
                    <tbody>
                      {quickFillVisibleTasks.length === 0 ? (
                        <tr>
                          <td className="px-4 py-3 text-[var(--desc-color)]" colSpan={2}>
                            暂无任务
                          </td>
                        </tr>
                      ) : (
                        quickFillVisibleTasks.map((task) => (
                          <tr
                            key={task.taskId}
                            className="cursor-pointer border-t border-[var(--split-color)] hover:bg-[var(--solid-button-color)]"
                            onClick={() => handleQuickFillSelect(task)}
                          >
                            <td className="px-4 py-2 text-[var(--content-color)]">{task.title}</td>
                            <td className="px-4 py-2 text-[var(--desc-color)]">
                              {formatDateTime(task.createdAt)}
                            </td>
                          </tr>
                        ))
                      )}
                    </tbody>
                  </table>
                </div>
                <div className="mt-4 flex flex-wrap items-center justify-between gap-3 text-xs text-[var(--desc-color)]">
                  <div>
                    共 {quickFillTotal} 条，当前第 {quickFillPage}/{quickFillTotalPages} 页
                  </div>
                  <div className="flex items-center gap-2">
                    <button
                      className="h-7 px-3 rounded-lg text-xs"
                      onClick={() => setQuickFillPage((prev) => Math.max(1, prev - 1))}
                      disabled={quickFillPage <= 1}
                    >
                      上一页
                    </button>
                    <button
                      className="h-7 px-3 rounded-lg text-xs"
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
        </div>
      ) : (
        <div className="flex w-full h-full gap-3 min-h-0">
          <div className="flex-1 min-h-0">
            <div className="panel flex flex-col gap-2 p-3 min-h-0">
              <div className="flex items-center gap-2 px-1">
                <span className="text-sm font-semibold text-[var(--content-color)]">
                  {activeRecordLabel}（{downloadList.length}）
                </span>
                <button
                  className="ml-auto h-8 px-3 rounded-lg"
                  onClick={() => loadDownloadList()}
                  disabled={loadingDownloads}
                >
                  {loadingDownloads ? "刷新中..." : "刷新"}
                </button>
              </div>
              <div className="flex flex-col gap-2 overflow-y-auto pr-1 min-h-0">
                {downloadList.length === 0 ? (
                  <div className="desc px-2 py-6 text-center">{emptyRecordText}</div>
                ) : (
                  downloadList.map((record) => {
                    const progressTotal = Number(record.progressTotal || 0);
                    const progressDone = Number(record.progressDone || 0);
                    const progressValue =
                      progressTotal > 0
                        ? Math.min(100, (progressDone / progressTotal) * 100)
                        : Math.min(100, record.progress || 0);
                    const progressLabel = Number(progressValue.toFixed(1));
                    const sourceLabel =
                      (record.sourceType || "").toUpperCase() === "BAIDU" ? "网盘" : "B站";
                    return (
                      <div
                        key={record.id}
                        className="flex flex-col gap-2 rounded-lg border-2 bg-[var(--block-color)] p-3 text-sm"
                        style={{ borderColor: getRecordBorderColor(record.status) }}
                      >
                        <div className="flex items-center gap-2 text-[var(--content-color)]">
                          <span className="truncate">{record.title || record.bvid || "-"}</span>
                          <span className="ml-auto text-xs text-[var(--desc-color)]">
                            {formatDateTime(record.createTime)}
                          </span>
                        </div>
                        <div className="flex flex-wrap gap-3 text-xs text-[var(--desc-color)]">
                          <span>分P：{record.partTitle || "-"}</span>
                          <span>分辨率：{record.resolution || "-"}</span>
                          <span>编码：{record.codec || "-"}</span>
                          <span>格式：{record.format || "-"}</span>
                          <span>来源：{sourceLabel}</span>
                        </div>
                        <div className="flex items-center gap-3">
                          <div className="flex-1">
                            <div className="h-1.5 w-full rounded-full bg-[var(--solid-button-color)]">
                              <div
                                className="h-1.5 rounded-full"
                                style={{
                                  width: `${progressValue}%`,
                                  backgroundColor: getRecordProgressColor(record.status),
                                }}
                              />
                            </div>
                          </div>
                          <span className="w-12 text-xs text-[var(--desc-color)]">
                            {progressLabel.toFixed(1)}%
                          </span>
                          {record.status === 4 ? (
                            <button
                              className="h-8 px-3 rounded-lg"
                              onClick={() => handleResumeRecord(record.id)}
                            >
                              继续下载
                            </button>
                          ) : null}
                          {record.status === 3 ? (
                            <button
                              className="h-8 px-3 rounded-lg"
                              onClick={() => handleRetryRecord(record.id)}
                            >
                              重新下载
                            </button>
                          ) : null}
                          {record.status === 2 ? (
                            <button
                              className="h-8 px-3 rounded-lg"
                              onClick={() => handleOpenRecordFolder(record)}
                            >
                              查看
                            </button>
                          ) : null}
                          <button
                            className="h-8 px-3 rounded-lg"
                            onClick={() => handleRequestDeleteRecord(record)}
                          >
                            删除
                          </button>
                        </div>
                      </div>
                    );
                  })
                )}
              </div>
            </div>
          </div>
          <div className="tab">
            {recordTabs.map((tab) => (
              <button
                key={tab.key}
                className={recordTab === tab.key ? "active" : ""}
                onClick={() => setRecordTab(tab.key)}
              >
                <span>{tab.label}</span>
                <label></label>
              </button>
            ))}
          </div>
        </div>
      )}
      <BaiduSyncPathPicker
        open={syncPickerOpen}
        value={submissionConfig.baiduSyncPath || defaultBaiduSyncPath || "/录播"}
        onConfirm={handleConfirmSyncPicker}
        onClose={handleCloseSyncPicker}
        onChange={handleSyncPathChange}
      />
      {deleteConfirmRecord ? (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
          <div className="w-[360px] rounded-2xl bg-[var(--block-color)] p-5 text-sm text-[var(--content-color)] shadow-xl">
            <div className="text-base font-semibold">删除确认</div>
            <div className="mt-2 text-[var(--desc-color)]">确定删除该下载记录？</div>
            <label className="mt-3 flex items-center gap-2 text-[var(--desc-color)]">
              <input
                type="checkbox"
                checked={deleteConfirmDeleteFile}
                onChange={(event) => setDeleteConfirmDeleteFile(event.target.checked)}
              />
              同步删除视频文件
            </label>
            <div className="mt-4 flex justify-end gap-2">
              <button className="h-9 rounded-lg px-4" onClick={handleCancelDeleteRecord}>
                取消
              </button>
              <button className="h-9 rounded-lg px-4" onClick={handleConfirmDeleteRecord}>
                确认
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
}
