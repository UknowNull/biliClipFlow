import { useEffect, useRef, useState } from "react";
import { confirm as dialogConfirm } from "@tauri-apps/plugin-dialog";
import { invokeCommand } from "../lib/tauri";
import { formatDateTime } from "../lib/format";

export default function AnchorSection() {
  const [newAnchorUid, setNewAnchorUid] = useState("");
  const [anchors, setAnchors] = useState([]);
  const [anchorAvatars, setAnchorAvatars] = useState({});
  const [message, setMessage] = useState("");
  const [loading, setLoading] = useState(false);
  const [syncAnchor, setSyncAnchor] = useState(null);
  const [syncPath, setSyncPath] = useState("");
  const [syncLoading, setSyncLoading] = useState(false);
  const [syncMessage, setSyncMessage] = useState("");
  const [syncPickerOpen, setSyncPickerOpen] = useState(false);
  const [syncBrowserPath, setSyncBrowserPath] = useState("/");
  const [syncFolders, setSyncFolders] = useState([]);
  const [syncBrowseLoading, setSyncBrowseLoading] = useState(false);
  const [syncBrowseError, setSyncBrowseError] = useState("");
  const [recordView, setRecordView] = useState(null);
  const [recordList, setRecordList] = useState([]);
  const [recordListLoading, setRecordListLoading] = useState(false);
  const [recordListError, setRecordListError] = useState("");
  const [detailView, setDetailView] = useState(null);
  const [analyzingTasks, setAnalyzingTasks] = useState({});
  const [clipView, setClipView] = useState(null);
  const [clipListLoading, setClipListLoading] = useState(false);
  const [clipListError, setClipListError] = useState("");
  const [clipListMessage, setClipListMessage] = useState("");
  const [clipEdit, setClipEdit] = useState(null);
  const [reclipLoading, setReclipLoading] = useState({});
  const [clipSubmitView, setClipSubmitView] = useState(null);
  const [clipSubmitMessage, setClipSubmitMessage] = useState("");
  const [clipSubmitSubmitting, setClipSubmitSubmitting] = useState(false);
  const [submissionConfigView, setSubmissionConfigView] = useState(null);
  const [submissionConfigForm, setSubmissionConfigForm] = useState({
    title: "",
    description: "",
    partitionId: "",
    collectionId: "",
    activityTopicId: "",
    activityMissionId: "",
    activityTitle: "",
    videoType: "ORIGINAL",
  });
  const [submissionConfigTags, setSubmissionConfigTags] = useState([]);
  const [submissionConfigTagInput, setSubmissionConfigTagInput] = useState("");
  const [submissionConfigMessage, setSubmissionConfigMessage] = useState("");
  const [submissionConfigLoading, setSubmissionConfigLoading] = useState(false);
  const [submissionConfigSubmitting, setSubmissionConfigSubmitting] = useState(false);
  const [configPartitions, setConfigPartitions] = useState([]);
  const [configCollections, setConfigCollections] = useState([]);
  const [configActivityOptions, setConfigActivityOptions] = useState([]);
  const [configActivityLoading, setConfigActivityLoading] = useState(false);
  const [configActivityMessage, setConfigActivityMessage] = useState("");
  const [configActivityKeyword, setConfigActivityKeyword] = useState("");
  const [configActivityDropdownOpen, setConfigActivityDropdownOpen] = useState(false);
  const [configUpProfile, setConfigUpProfile] = useState({ uid: 0, name: "" });
  const configActivityRequestSeqRef = useRef(0);

  const logClient = async (text) => {
    try {
      await invokeCommand("auth_client_log", { message: text });
    } catch (error) {
      // ignore log errors
    }
  };

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
  const resolvePartitionSelectValue = (partitionId, options = configPartitions) => {
    const normalizedId = String(partitionId || "").trim();
    if (!normalizedId) {
      return "";
    }
    return options.some((item) => String(item.tid) === normalizedId) ? normalizedId : normalizedId;
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
        showActivityIcon: Boolean(item?.showActivityIcon ?? item?.show_activity_icon ?? false),
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

  const loadConfigPartitions = async () => {
    try {
      const data = await invokeCommand("bilibili_partitions");
      setConfigPartitions(data || []);
    } catch (error) {
      setSubmissionConfigMessage(error.message);
    }
  };

  const loadConfigCollections = async () => {
    try {
      const auth = await invokeCommand("auth_status");
      const profile = extractCurrentAuthProfile(auth);
      setConfigUpProfile(profile);
      if (!profile?.uid) {
        setConfigCollections([]);
        return;
      }
      const data = await invokeCommand("bilibili_collections", { mid: profile.uid });
      const mapped = (data || []).map((item) => ({
        ...item,
        seasonId: item.season_id ?? item.seasonId,
      }));
      setConfigCollections(mapped);
    } catch (error) {
      setSubmissionConfigMessage(error.message);
    }
  };

  const loadConfigActivities = async (partitionId, keyword = "") => {
    const requestSeq = configActivityRequestSeqRef.current + 1;
    configActivityRequestSeqRef.current = requestSeq;
    setConfigActivityLoading(true);
    setConfigActivityMessage("");
    try {
      const normalizedKeyword = String(keyword || "").trim();
      const data = await invokeCommand("bilibili_topics", {
        partitionId: partitionId ? Number(partitionId) : null,
        title: normalizedKeyword || null,
      });
      if (requestSeq !== configActivityRequestSeqRef.current) {
        return;
      }
      const mapped = normalizeActivityOptions(data);
      setConfigActivityOptions(mapped);
      const currentId = Number(submissionConfigForm.activityTopicId || 0);
      if (
        currentId > 0 &&
        mapped.length > 0 &&
        !mapped.some((item) => item.topicId === currentId)
      ) {
        const previousTitle = submissionConfigForm.activityTitle || "";
        if (previousTitle) {
          setSubmissionConfigTags((prev) => prev.filter((tag) => tag !== previousTitle));
        }
        setSubmissionConfigForm((prev) => ({
          ...prev,
          activityTopicId: "",
          activityMissionId: "",
          activityTitle: "",
        }));
      }
    } catch (error) {
      if (requestSeq !== configActivityRequestSeqRef.current) {
        return;
      }
      setConfigActivityOptions([]);
      setConfigActivityMessage(error.message);
    } finally {
      if (requestSeq === configActivityRequestSeqRef.current) {
        setConfigActivityLoading(false);
      }
    }
  };

  const resetSubmissionConfigForm = () => {
    setSubmissionConfigForm({
      title: "",
      description: "",
      partitionId: "",
      collectionId: "",
      activityTopicId: "",
      activityMissionId: "",
      activityTitle: "",
      videoType: "ORIGINAL",
    });
    setSubmissionConfigTags([]);
    setSubmissionConfigTagInput("");
    setConfigActivityKeyword("");
    setConfigActivityDropdownOpen(false);
  };

  const applySubmissionConfigData = (data) => {
    if (!data) {
      resetSubmissionConfigForm();
      return;
    }
    const partitionId = data.partitionId ? String(data.partitionId) : "";
    const collectionId = data.collectionId ? String(data.collectionId) : "";
    const topicId = data.topicId ? String(data.topicId) : "";
    const missionId = data.missionId ? String(data.missionId) : "";
    const activityTitle = data.activityTitle || "";
    const tagList = String(data.tags || "")
      .split(",")
      .map((item) => item.trim())
      .filter((item) => item);
    const uniqueTags = [...new Set(tagList)];
    if (activityTitle && !uniqueTags.includes(activityTitle)) {
      uniqueTags.push(activityTitle);
    }
    setSubmissionConfigForm({
      title: data.title || "",
      description: data.description || "",
      partitionId,
      collectionId,
      activityTopicId: topicId,
      activityMissionId: missionId,
      activityTitle,
      videoType: data.videoType || "ORIGINAL",
    });
    setSubmissionConfigTags(uniqueTags);
    setSubmissionConfigTagInput("");
    setConfigActivityKeyword(activityTitle);
    setConfigActivityDropdownOpen(false);
  };

  const loadAnchorSubmissionConfig = async (anchor, onError) => {
    await Promise.all([loadConfigPartitions(), loadConfigCollections()]);
    const data = await invokeCommand("anchor_submission_config_get", { roomId: anchor.uid });
    applySubmissionConfigData(data);
  };

  const loadAnchors = async () => {
    setMessage("");
    setLoading(true);
    try {
      await logClient("anchor_list:load_start");
      const data = await invokeCommand("anchor_check");
      setAnchors(data || []);
      await logClient(`anchor_list:load_ok:${Array.isArray(data) ? data.length : 0}`);
    } catch (error) {
      await logClient(`anchor_list:load_error:${error?.message || "unknown"}`);
      setMessage(error.message);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    loadAnchors();
    const timer = setInterval(loadAnchors, 30000);
    return () => clearInterval(timer);
  }, []);

  useEffect(() => {
    let active = true;
    const loadAvatars = async () => {
      const updates = {};
      for (const anchor of anchors) {
        if (!anchor?.avatarUrl) {
          continue;
        }
        if (anchorAvatars[anchor.uid]) {
          continue;
        }
        try {
          const data = await invokeCommand("video_proxy_image", { url: anchor.avatarUrl });
          if (data) {
            updates[anchor.uid] = data;
          }
        } catch (error) {
          // ignore avatar proxy errors
        }
      }
      if (active && Object.keys(updates).length > 0) {
        setAnchorAvatars((prev) => ({ ...prev, ...updates }));
      }
    };
    loadAvatars();
    return () => {
      active = false;
    };
  }, [anchors]);

  const handleSubscribe = async () => {
    const uid = extractUidFromInput(newAnchorUid);
    await logClient(`anchor_subscribe:click raw=${newAnchorUid || ""}`);
    if (!uid) {
      await logClient("anchor_subscribe:invalid_input");
      setMessage("请输入房间号或链接");
      return;
    }
    setLoading(true);
    setMessage("");
    try {
      await logClient(`anchor_subscribe:invoke_start uid=${uid}`);
      await invokeCommand("anchor_subscribe", { payload: { uids: [uid] } });
      await logClient("anchor_subscribe:invoke_ok");
      await loadAnchors();
      setNewAnchorUid("");
    } catch (error) {
      await logClient(`anchor_subscribe:invoke_error:${error?.message || "unknown"}`);
      setMessage(error.message);
    } finally {
      setLoading(false);
    }
  };

  const handleOpenRecordView = async (anchor) => {
    setSubmissionConfigView(null);
    setClipSubmitView(null);
    setRecordView(anchor);
    setRecordList([]);
    setRecordListError("");
    setRecordListLoading(true);
    setAnalyzingTasks({});
    try {
      const data = await invokeCommand("anchor_record_list", { roomId: anchor.uid });
      setRecordList(data || []);
    } catch (error) {
      setRecordListError(error.message);
    } finally {
      setRecordListLoading(false);
    }
  };

  const handleUnsubscribe = async (anchor) => {
    setMessage("");
    try {
      await invokeCommand("anchor_unsubscribe", { uid: anchor.uid });
      await loadAnchors();
    } catch (error) {
      setMessage(error.message);
    }
  };

  const handleOpenSubmissionConfig = async (anchor) => {
    setSubmissionConfigView(anchor);
    setRecordView(null);
    setClipView(null);
    setClipEdit(null);
    setSubmissionConfigMessage("");
    setSubmissionConfigLoading(true);
    setSubmissionConfigSubmitting(false);
    resetSubmissionConfigForm();
    try {
      await loadAnchorSubmissionConfig(anchor);
    } catch (error) {
      setSubmissionConfigMessage(error?.message || "加载投稿配置失败");
    } finally {
      setSubmissionConfigLoading(false);
    }
  };

  const closeSubmissionConfigView = () => {
    setSubmissionConfigView(null);
    setSubmissionConfigMessage("");
    setSubmissionConfigSubmitting(false);
    setSubmissionConfigTags([]);
    setSubmissionConfigTagInput("");
    setConfigActivityKeyword("");
    setConfigActivityDropdownOpen(false);
  };

  const handleOpenClipSubmit = async (record) => {
    if (!recordView || !record?.filePath) {
      return;
    }
    setClipSubmitView({ anchor: recordView, record });
    setClipSubmitMessage("");
    setClipSubmitSubmitting(false);
    setSubmissionConfigMessage("");
    setSubmissionConfigLoading(true);
    resetSubmissionConfigForm();
    try {
      await loadAnchorSubmissionConfig(recordView);
    } catch (error) {
      setClipSubmitMessage(error?.message || "加载投稿配置失败");
    } finally {
      setSubmissionConfigLoading(false);
    }
  };

  const closeClipSubmitView = () => {
    setClipSubmitView(null);
    setClipSubmitMessage("");
    setClipSubmitSubmitting(false);
  };

  const handleStartRecord = async (anchor) => {
    setMessage("");
    try {
      await invokeCommand("live_record_start", { roomId: anchor.uid });
      await loadAnchors();
    } catch (error) {
      setMessage(error.message);
    }
  };

  const handleStopRecord = async (anchor) => {
    setMessage("");
    try {
      await invokeCommand("live_record_stop", { roomId: anchor.uid });
      await loadAnchors();
    } catch (error) {
      setMessage(error.message);
    }
  };

  const handleAutoRecordToggle = async (anchor) => {
    setMessage("");
    try {
      await invokeCommand("live_room_auto_record_update", {
        roomId: anchor.uid,
        autoRecord: !anchor.autoRecord,
      });
      await loadAnchors();
    } catch (error) {
      setMessage(error.message);
    }
  };

  const handleSyncToggle = async (anchor) => {
    setMessage("");
    if (!anchor.baiduSyncEnabled && !anchor.baiduSyncPath) {
      setSyncAnchor(anchor);
      setSyncPath("");
      setSyncBrowserPath("/");
      setSyncMessage("请先选择同步路径");
      setSyncPickerOpen(true);
      loadSyncFolders("/");
      return;
    }
    try {
      await invokeCommand("live_room_baidu_sync_toggle", {
        roomId: anchor.uid,
        enabled: !anchor.baiduSyncEnabled,
      });
      await loadAnchors();
    } catch (error) {
      setMessage(error.message || "同步设置失败");
    }
  };

  const loadSyncFolders = async (path) => {
    setSyncBrowseError("");
    setSyncBrowseLoading(true);
    try {
      const data = await invokeCommand("baidu_sync_remote_dirs", {
        request: { path },
      });
      setSyncFolders(Array.isArray(data) ? data : []);
    } catch (error) {
      setSyncBrowseError(error?.message || "读取目录失败");
      setSyncFolders([]);
    } finally {
      setSyncBrowseLoading(false);
    }
  };

  const handleOpenSyncConfig = (anchor) => {
    const initialPath = anchor?.baiduSyncPath || "";
    const normalizedPath = initialPath.trim() || "/";
    setSyncAnchor(anchor);
    setSyncPath(initialPath.trim());
    setSyncBrowserPath(normalizedPath);
    setSyncMessage("");
  };

  const handleCloseSyncConfig = () => {
    if (syncLoading) {
      return;
    }
    setSyncAnchor(null);
    setSyncPath("");
    setSyncMessage("");
    setSyncFolders([]);
    setSyncBrowseError("");
    setSyncBrowserPath("/");
    setSyncPickerOpen(false);
  };

  const handleSaveSyncConfig = async () => {
    if (!syncAnchor) {
      return;
    }
    setSyncLoading(true);
    setSyncMessage("");
    try {
      await invokeCommand("live_room_baidu_sync_update", {
        roomId: syncAnchor.uid,
        baiduSyncPath: syncPath,
      });
      await loadAnchors();
      setMessage("同步配置已保存");
      setSyncAnchor(null);
      setSyncPath("");
      setSyncMessage("");
      setSyncFolders([]);
      setSyncBrowseError("");
      setSyncBrowserPath("/");
      setSyncPickerOpen(false);
    } catch (error) {
      setSyncMessage(error?.message || "保存失败");
    } finally {
      setSyncLoading(false);
    }
  };

  const handleSyncSelectCurrent = () => {
    setSyncPath(syncBrowserPath);
  };

  const handleOpenSyncPicker = () => {
    if (!syncAnchor) {
      return;
    }
    setSyncPickerOpen(true);
    loadSyncFolders(syncBrowserPath);
  };

  const handleCloseSyncPicker = () => {
    if (syncBrowseLoading) {
      return;
    }
    setSyncPickerOpen(false);
  };

  const handleConfirmSyncPicker = () => {
    setSyncPath(syncBrowserPath);
    setSyncPickerOpen(false);
  };

  const handleSyncEnterFolder = (folder) => {
    if (!folder?.path) {
      return;
    }
    setSyncBrowserPath(folder.path);
    loadSyncFolders(folder.path);
  };

  const handleSyncGoParent = () => {
    if (syncBrowserPath === "/") {
      return;
    }
    const trimmed = syncBrowserPath.replace(/\/+$/, "");
    const index = trimmed.lastIndexOf("/");
    const parent = index <= 0 ? "/" : trimmed.slice(0, index);
    setSyncBrowserPath(parent);
    loadSyncFolders(parent);
  };

  const extractUidFromInput = (input) => {
    if (!input) {
      return "";
    }
    const trimmed = input.trim();
    if (/^\d+$/.test(trimmed)) {
      return trimmed;
    }
    const match = trimmed.match(/live\.bilibili\.com\/(\d+)/);
    if (match && match[1]) {
      return match[1];
    }
    return trimmed;
  };

  const formatSeconds = (seconds) => {
    const total = Math.max(0, Number(seconds) || 0);
    const hours = Math.floor(total / 3600);
    const minutes = Math.floor((total % 3600) / 60);
    const secs = Math.floor(total % 60);
    return [hours, minutes, secs].map((value) => String(value).padStart(2, "0")).join(":");
  };

  const parseTimeInput = (value) => {
    if (value === null || value === undefined) {
      return null;
    }
    const raw = String(value).trim();
    if (!raw) {
      return null;
    }
    if (/^\d+$/.test(raw)) {
      return Number(raw);
    }
    const parts = raw.split(":").map((item) => item.trim());
    if (parts.length === 2 || parts.length === 3) {
      const nums = parts.map((item) => Number(item));
      if (nums.some((num) => Number.isNaN(num))) {
        return null;
      }
      const [h, m, s] = parts.length === 3 ? nums : [0, nums[0], nums[1]];
      return h * 3600 + m * 60 + s;
    }
    return null;
  };

  const startClipEdit = (record, field) => {
    if (!record?.id) {
      return;
    }
    const current = field === "start" ? record.startOffset : record.endOffset;
    setClipEdit({ id: record.id, field, value: formatSeconds(current) });
  };

  const submitClipEdit = async (record) => {
    if (!clipEdit || !record) {
      return;
    }
    const value = parseTimeInput(clipEdit.value);
    if (value === null) {
      setClipListError("时间格式无效，支持 HH:MM:SS 或秒数");
      return;
    }
    const startOffset = clipEdit.field === "start" ? value : record.startOffset;
    const endOffset = clipEdit.field === "end" ? value : record.endOffset;
    if (endOffset <= startOffset || startOffset < 0) {
      setClipListError("时间段无效，请确保结束时间大于开始时间");
      return;
    }
    try {
      const updated = await invokeCommand("anchor_clip_update_time", {
        clipId: record.id,
        startOffset,
        endOffset,
      });
      setClipView((prev) => {
        if (!prev) return prev;
        const nextRecords = prev.records.map((item) =>
          item.id === record.id ? { ...item, ...updated } : item
        );
        return { ...prev, records: nextRecords };
      });
      setClipEdit(null);
    } catch (error) {
      setClipListError(error?.message || "更新时间失败");
    }
  };

  const handleAnalyzeClips = async (day, records) => {
    if (!recordView) {
      return;
    }
    const recordIds = records.map((record) => record.id).filter((id) => Number.isFinite(id));
    if (recordIds.length === 0) {
      setRecordListError("录播记录为空，无法分析");
      return;
    }
    setRecordListError("");
    setAnalyzingTasks((prev) => ({
      ...prev,
      [day]: { taskId: prev?.[day]?.taskId || 0, status: "RUNNING", errorMessage: "" },
    }));
    try {
      const taskId = await invokeCommand("anchor_analyze_clips", {
        roomId: recordView.uid,
        recordIds,
      });
      setAnalyzingTasks((prev) => ({
        ...prev,
        [day]: { taskId, status: "RUNNING", errorMessage: "" },
      }));
    } catch (error) {
      setAnalyzingTasks((prev) => ({
        ...prev,
        [day]: { taskId: 0, status: "FAILED", errorMessage: error?.message || "分析失败" },
      }));
      setRecordListError(error?.message || "分析失败");
    }
  };

  const handleOpenClipList = async (day, records) => {
    if (!recordView) {
      return;
    }
    const recordIds = records.map((record) => record.id).filter((id) => Number.isFinite(id));
    setClipListLoading(true);
    setClipListError("");
    setClipListMessage("");
    setClipEdit(null);
    setClipView({ dateLabel: day, records: [], recordIds });
    try {
      const data = await invokeCommand("anchor_clip_list", {
        roomId: recordView.uid,
        recordIds,
      });
      const items = data?.items ?? data ?? [];
      setClipView({ dateLabel: day, records: items, recordIds });
    } catch (error) {
      setClipListError(error?.message || "加载切片记录失败");
    } finally {
      setClipListLoading(false);
    }
  };

  const refreshClipList = async (source = "refresh") => {
    if (!recordView || !clipView?.recordIds?.length || clipListLoading) {
      return;
    }
    setClipListLoading(true);
    setClipListError("");
    if (source === "reclip") {
      setClipListMessage("");
    }
    try {
      const data = await invokeCommand("anchor_clip_list", {
        roomId: recordView.uid,
        recordIds: clipView.recordIds,
      });
      const items = data?.items ?? data ?? [];
      setClipView((prev) => (prev ? { ...prev, records: items } : prev));
    } catch (error) {
      setClipListError(error?.message || "刷新切片记录失败");
    } finally {
      setClipListLoading(false);
    }
  };

  useEffect(() => {
    const handler = () => {
      if (!clipView?.recordIds?.length || !recordView) {
        return;
      }
      refreshClipList("repost");
    };
    window.addEventListener("anchor-clip-list-refresh", handler);
    return () => window.removeEventListener("anchor-clip-list-refresh", handler);
  }, [clipView, recordView]);

  useEffect(() => {
    if (!clipView?.records?.length || !recordView) {
      return undefined;
    }
    const hasRunning = clipView.records.some(
      (record) => String(record?.status || "").toUpperCase() === "RUNNING"
    );
    if (!hasRunning) {
      return undefined;
    }
    const timer = setInterval(() => {
      refreshClipList("poll");
    }, 2000);
    return () => clearInterval(timer);
  }, [clipView, recordView, clipListLoading]);

  useEffect(() => {
    if (!recordView || Object.keys(analyzingTasks).length === 0) {
      return undefined;
    }
    const polling = setInterval(async () => {
      const entries = Object.entries(analyzingTasks);
      for (const [day, task] of entries) {
        if (!task?.taskId || task.status !== "RUNNING") {
          continue;
        }
        try {
          const result = await invokeCommand("anchor_clip_task_status", {
            taskId: task.taskId,
          });
          if (result?.status && result.status !== "RUNNING") {
            setAnalyzingTasks((prev) => ({
              ...prev,
              [day]: {
                ...task,
                status: result.status,
                errorMessage: result.errorMessage || "",
              },
            }));
          }
        } catch (error) {
          setAnalyzingTasks((prev) => ({
            ...prev,
            [day]: {
              ...task,
              status: "FAILED",
              errorMessage: error?.message || "状态查询失败",
            },
          }));
        }
      }
    }, 2000);
    return () => clearInterval(polling);
  }, [analyzingTasks, recordView]);

  useEffect(() => {
    if (!submissionConfigView && !clipSubmitView) {
      return;
    }
    const partitionId = Number(submissionConfigForm.partitionId || 0);
    if (!partitionId) {
      configActivityRequestSeqRef.current += 1;
      setConfigActivityLoading(false);
      setConfigActivityOptions([]);
      setSubmissionConfigForm((prev) => ({
        ...prev,
        activityTopicId: "",
        activityMissionId: "",
        activityTitle: "",
      }));
      return;
    }
    loadConfigActivities(partitionId, configActivityKeyword);
  }, [submissionConfigView, clipSubmitView, submissionConfigForm.partitionId, configActivityKeyword]);

  const statusLabel = (status) => {
    if (status === 1) {
      return "直播中";
    }
    if (status === 2) {
      return "轮播中";
    }
    return "未直播";
  };

  const configActivitySelectOptions = (() => {
    const ordered = [...configActivityOptions];
    const currentId = Number(submissionConfigForm.activityTopicId || 0);
    if (!currentId) {
      return ordered;
    }
    const exists = ordered.some((item) => item.topicId === currentId);
    if (exists || !submissionConfigForm.activityTitle) {
      return ordered;
    }
    return [
      {
        topicId: currentId,
        missionId: Number(submissionConfigForm.activityMissionId || 0),
        name: submissionConfigForm.activityTitle,
        description: "",
        activityText: "",
        activityDescription: "",
        readCount: 0,
        showActivityIcon: false,
      },
      ...ordered,
    ];
  })();

  const configActivityFilteredOptions = (() => {
    const keyword = String(configActivityKeyword || "").trim().toLowerCase();
    if (!keyword) {
      return configActivitySelectOptions;
    }
    const filtered = configActivitySelectOptions.filter((activity) => {
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
    const currentId = Number(submissionConfigForm.activityTopicId || 0);
    if (!currentId || filtered.some((item) => item.topicId === currentId)) {
      return filtered;
    }
    const selected = configActivitySelectOptions.find((item) => item.topicId === currentId);
    return selected ? [selected, ...filtered] : filtered;
  })();

  const applyConfigActivitySelection = (activity) => {
    const previousTitle = submissionConfigForm.activityTitle || "";
    const nextTitle = activity?.name || "";
    setSubmissionConfigForm((prev) => ({
      ...prev,
      activityTopicId: activity ? String(activity.topicId) : "",
      activityMissionId: activity ? String(activity.missionId || "") : "",
      activityTitle: nextTitle,
    }));
    setSubmissionConfigTags((prev) => {
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
    setConfigActivityKeyword(nextTitle);
    setConfigActivityDropdownOpen(false);
  };

  const handleConfigActivitySelect = (value) => {
    if (!value) {
      applyConfigActivitySelection(null);
      return;
    }
    const target = configActivitySelectOptions.find((item) => String(item.topicId) === value);
    if (!target) {
      applyConfigActivitySelection(null);
      return;
    }
    applyConfigActivitySelection(target);
  };

  const isConfigActivityTag = (tag) =>
    Boolean(submissionConfigForm.activityTitle) && tag === submissionConfigForm.activityTitle;
  const resolveConfigTagClassName = (tag) =>
    isConfigActivityTag(tag)
      ? "inline-flex items-center gap-1 rounded-full border border-amber-200 bg-amber-50 px-2 py-1 text-xs text-amber-700"
      : "inline-flex items-center gap-1 rounded-full bg-[var(--accent)]/10 px-2 py-1 text-xs text-[var(--accent)]";
  const resolveConfigTagRemoveClassName = (tag) =>
    isConfigActivityTag(tag)
      ? "text-[10px] font-semibold text-amber-700 hover:opacity-70"
      : "text-[10px] font-semibold text-[var(--accent)] hover:opacity-70";

  const renderConfigActivityTopicSelector = () => (
    <div className="mt-2 space-y-1">
      <div className="text-xs text-[var(--muted)]">活动话题（可选）</div>
      <div className="relative">
        <input
          value={configActivityKeyword}
          onChange={(event) => {
            setConfigActivityKeyword(event.target.value);
            setConfigActivityDropdownOpen(true);
          }}
          onFocus={() => setConfigActivityDropdownOpen(true)}
          onBlur={() => {
            window.setTimeout(() => {
              setConfigActivityDropdownOpen(false);
            }, 120);
          }}
          placeholder="输入活动话题关键字并下拉选择"
          disabled={!submissionConfigForm.partitionId}
          className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm text-[var(--ink)] focus:border-[var(--accent)] focus:outline-none disabled:cursor-not-allowed disabled:bg-black/5"
        />
        {configActivityDropdownOpen && submissionConfigForm.partitionId ? (
          <div className="absolute z-20 mt-1 max-h-64 w-full overflow-auto rounded-lg border border-black/10 bg-white/95 p-1 shadow-lg">
            <button
              type="button"
              className={`w-full rounded-md px-3 py-2 text-left text-sm ${
                submissionConfigForm.activityTopicId
                  ? "text-[var(--ink)] hover:bg-black/5"
                  : "bg-[var(--accent)]/10 text-[var(--accent)]"
              }`}
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => handleConfigActivitySelect("")}
            >
              不参与活动
            </button>
            {configActivityLoading ? (
              <div className="px-3 py-2 text-xs text-[var(--muted)]">活动加载中...</div>
            ) : null}
            {configActivityFilteredOptions.map((activity) => {
              const active = String(activity.topicId) === String(submissionConfigForm.activityTopicId);
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
                  onClick={() => handleConfigActivitySelect(String(activity.topicId))}
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
                    播放 {activity.readCount}
                    {activity.activityText ? ` · ${activity.activityText}` : ""}
                  </div>
                </button>
              );
            })}
            {!configActivityLoading && !configActivityFilteredOptions.length ? (
              <div className="px-3 py-2 text-xs text-[var(--muted)]">没有匹配的话题</div>
            ) : null}
          </div>
        ) : null}
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <button
          type="button"
          onClick={() => loadConfigActivities(submissionConfigForm.partitionId, configActivityKeyword)}
          disabled={configActivityLoading || !submissionConfigForm.partitionId}
          className="rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-xs text-[var(--muted)] hover:text-[var(--accent)] disabled:opacity-60"
        >
          刷新活动
        </button>
        {submissionConfigForm.activityTopicId ? (
          <button
            type="button"
            onClick={() => handleConfigActivitySelect("")}
            className="rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-xs text-[var(--muted)] hover:text-[var(--accent)]"
          >
            清空选择
          </button>
        ) : null}
      </div>
      {configActivityLoading ? <div className="text-xs text-[var(--muted)]">活动加载中...</div> : null}
      {configActivityMessage ? <div className="text-xs text-rose-500">{configActivityMessage}</div> : null}
    </div>
  );

  const addConfigTag = (value) => {
    const nextTag = value.trim();
    if (!nextTag) {
      return;
    }
    if (submissionConfigTags.includes(nextTag)) {
      return;
    }
    setSubmissionConfigTags((prev) => [...prev, nextTag]);
  };

  const removeConfigTag = (target) => {
    setSubmissionConfigTags((prev) => prev.filter((tag) => tag !== target));
    if (target === submissionConfigForm.activityTitle) {
      setSubmissionConfigForm((prev) => ({
        ...prev,
        activityTopicId: "",
        activityMissionId: "",
        activityTitle: "",
      }));
      setConfigActivityKeyword("");
    }
  };

  const handleConfigTagKeyDown = (event) => {
    if (event.key !== "Enter") {
      return;
    }
    event.preventDefault();
    addConfigTag(submissionConfigTagInput);
    setSubmissionConfigTagInput("");
  };

  const handleSaveSubmissionConfig = async () => {
    if (!submissionConfigView || submissionConfigSubmitting) {
      return;
    }
    setSubmissionConfigMessage("");
    if (!submissionConfigForm.title.trim()) {
      setSubmissionConfigMessage("请输入投稿标题");
      return;
    }
    if (submissionConfigForm.title.length > 80) {
      setSubmissionConfigMessage("投稿标题不能超过 80 个字符");
      return;
    }
    if (!submissionConfigForm.partitionId) {
      setSubmissionConfigMessage("请选择B站分区");
      return;
    }
    if (!submissionConfigForm.videoType) {
      setSubmissionConfigMessage("请选择视频类型");
      return;
    }
    if (submissionConfigForm.description && submissionConfigForm.description.length > 2000) {
      setSubmissionConfigMessage("视频描述不能超过 2000 个字符");
      return;
    }
    const normalizedTags = [...submissionConfigTags]
      .map((tag) => tag.trim())
      .filter((tag) => tag);
    if (!normalizedTags.length) {
      setSubmissionConfigMessage("请填写至少一个投稿标签");
      return;
    }
    const uniqueTags = [...new Set(normalizedTags)];
    setSubmissionConfigSubmitting(true);
    try {
      await invokeCommand("anchor_submission_config_save", {
        config: {
          roomId: submissionConfigView.uid,
          title: submissionConfigForm.title.trim(),
          description: submissionConfigForm.description?.trim() || "",
          partitionId: Number(submissionConfigForm.partitionId),
          collectionId: submissionConfigForm.collectionId
            ? Number(submissionConfigForm.collectionId)
            : null,
          tags: uniqueTags.join(","),
          topicId: submissionConfigForm.activityTopicId
            ? Number(submissionConfigForm.activityTopicId)
            : null,
          missionId: submissionConfigForm.activityMissionId
            ? Number(submissionConfigForm.activityMissionId)
            : null,
          activityTitle: submissionConfigForm.activityTitle || null,
          videoType: submissionConfigForm.videoType,
        },
      });
      setSubmissionConfigMessage("保存成功");
    } catch (error) {
      setSubmissionConfigMessage(error?.message || "保存失败");
    } finally {
      setSubmissionConfigSubmitting(false);
    }
  };

  const handleClipSubmit = async () => {
    if (!clipSubmitView || clipSubmitSubmitting) {
      return;
    }
    setClipSubmitMessage("");
    if (!submissionConfigForm.title.trim()) {
      setClipSubmitMessage("请输入投稿标题");
      return;
    }
    if (submissionConfigForm.title.length > 80) {
      setClipSubmitMessage("投稿标题不能超过 80 个字符");
      return;
    }
    if (!submissionConfigForm.partitionId) {
      setClipSubmitMessage("请选择B站分区");
      return;
    }
    if (!submissionConfigForm.videoType) {
      setClipSubmitMessage("请选择视频类型");
      return;
    }
    if (submissionConfigForm.description && submissionConfigForm.description.length > 2000) {
      setClipSubmitMessage("视频描述不能超过 2000 个字符");
      return;
    }
    const normalizedTags = [...submissionConfigTags]
      .map((tag) => tag.trim())
      .filter((tag) => tag);
    if (!normalizedTags.length) {
      setClipSubmitMessage("请填写至少一个投稿标签");
      return;
    }
    const uniqueTags = [...new Set(normalizedTags)];
    const clipPath = clipSubmitView.record?.filePath;
    if (!clipPath) {
      setClipSubmitMessage("切片文件路径为空，无法投稿");
      return;
    }
    setClipSubmitSubmitting(true);
    try {
      try {
        const auth = await invokeCommand("auth_status");
        if (!auth?.loggedIn) {
          setClipSubmitMessage("请先登录B站账号");
          return;
        }
      } catch (error) {
        setClipSubmitMessage(error?.message || "登录状态校验失败");
        return;
      }
      const result = await invokeCommand("submission_create", {
        request: {
          task: {
            title: submissionConfigForm.title.trim(),
            description: submissionConfigForm.description?.trim() || "",
            partitionId: Number(submissionConfigForm.partitionId),
            collectionId: submissionConfigForm.collectionId
              ? Number(submissionConfigForm.collectionId)
              : null,
            tags: uniqueTags.join(","),
            topicId: submissionConfigForm.activityTopicId
              ? Number(submissionConfigForm.activityTopicId)
              : null,
            missionId: submissionConfigForm.activityMissionId
              ? Number(submissionConfigForm.activityMissionId)
              : null,
            activityTitle: submissionConfigForm.activityTitle || null,
            videoType: submissionConfigForm.videoType,
            segmentPrefix: null,
            priority: false,
            baiduSyncEnabled: false,
            baiduSyncPath: null,
            baiduSyncFilename: null,
            sourceType: "CLIP",
          },
          sourceVideos: [
            {
              sourceFilePath: clipPath,
              sortOrder: 1,
            },
          ],
          workflowConfig: {
            enableSegmentation: false,
            segmentationConfig: {
              enabled: false,
              segmentDurationSeconds: 133,
              preserveOriginal: true,
            },
          },
        },
      });
      const taskId = result?.taskId || result?.task_id;
      if (taskId) {
        await invokeCommand("submission_execute", { taskId });
      }
      setClipSubmitMessage("已创建并提交投稿任务");
    } catch (error) {
      setClipSubmitMessage(error?.message || String(error) || "提交投稿失败");
    } finally {
      setClipSubmitSubmitting(false);
    }
  };

  const clipStatusLabel = (record) => {
    if (!record) {
      return "成功";
    }
    const status = String(record.status || "").toUpperCase();
    if (status === "RUNNING") {
      return "剪辑中";
    }
    if (status === "FAILED") {
      return "失败";
    }
    return "成功";
  };


  if (clipSubmitView) {
    const hasPartitionOption = configPartitions.some(
      (item) => String(item.tid) === String(submissionConfigForm.partitionId),
    );
    const partitionOptions =
      submissionConfigForm.partitionId && !hasPartitionOption
        ? [
            ...configPartitions,
            { tid: submissionConfigForm.partitionId, name: `当前分区(${submissionConfigForm.partitionId})` },
          ]
        : configPartitions;
    const partitionSelectValue = resolvePartitionSelectValue(
      submissionConfigForm.partitionId,
      partitionOptions,
    );
    const hasCollectionOption = configCollections.some(
      (item) => String(item.seasonId) === String(submissionConfigForm.collectionId),
    );
    const collectionOptions =
      submissionConfigForm.collectionId && !hasCollectionOption
        ? [
            ...configCollections,
            { seasonId: submissionConfigForm.collectionId, name: `当前合集(${submissionConfigForm.collectionId})` },
          ]
        : configCollections;
    const clipFileName = clipSubmitView.record?.filePath
      ? clipSubmitView.record.filePath.split(/[\\/]/).pop()
      : "-";

    return (
      <div className="space-y-6">
        <div className="rounded-2xl bg-[var(--surface)]/90 p-6 shadow-sm ring-1 ring-black/5">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div>
              <p className="text-sm uppercase tracking-[0.2em] text-[var(--muted)]">切片投稿</p>
              <h2 className="text-2xl font-semibold text-[var(--ink)]">切片投稿配置</h2>
            </div>
            <button
              className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
              onClick={closeClipSubmitView}
            >
              返回列表
            </button>
          </div>
          <div className="mt-3 text-sm text-[var(--muted)]">
            主播：{clipSubmitView.anchor?.nickname || "未知主播"}（房间号 {clipSubmitView.anchor?.uid}）
          </div>
          <div className="mt-1 text-xs text-[var(--muted)]">切片文件：{clipFileName}</div>
          {clipSubmitMessage ? (
            <div className="mt-4 rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-700">
              {clipSubmitMessage}
            </div>
          ) : null}
        </div>
        <div className="rounded-2xl bg-white/80 p-6 shadow-sm ring-1 ring-black/5">
          {submissionConfigLoading ? (
            <div className="text-sm text-[var(--muted)]">加载中...</div>
          ) : (
            <div className="space-y-4">
              <div className="space-y-1">
                <div className="text-xs text-[var(--muted)]">
                  视频标题<span className="ml-1 text-rose-500">必填</span>
                </div>
                <input
                  value={submissionConfigForm.title}
                  onChange={(event) =>
                    setSubmissionConfigForm((prev) => ({ ...prev, title: event.target.value }))
                  }
                  placeholder="请输入投稿标题"
                  className="w-full rounded-xl border border-black/10 bg-white/80 px-3 py-2 text-sm text-[var(--ink)] focus:border-[var(--accent)] focus:outline-none"
                />
              </div>
              <div className="space-y-1">
                <div className="text-xs text-[var(--muted)]">视频描述（可选）</div>
                <textarea
                  value={submissionConfigForm.description}
                  onChange={(event) =>
                    setSubmissionConfigForm((prev) => ({ ...prev, description: event.target.value }))
                  }
                  placeholder="视频描述"
                  rows={2}
                  className="w-full rounded-xl border border-black/10 bg-white/80 px-3 py-2 text-sm text-[var(--ink)] focus:border-[var(--accent)] focus:outline-none"
                />
              </div>
              <div className="grid gap-2 lg:grid-cols-3">
                <div className="space-y-1">
                  <div className="text-xs text-[var(--muted)]">
                    B站分区<span className="ml-1 text-rose-500">必填</span>
                  </div>
                  <select
                    value={partitionSelectValue}
                    onChange={(event) =>
                      setSubmissionConfigForm((prev) => ({
                        ...prev,
                        partitionId: parsePartitionOptionValue(event.target.value),
                      }))
                    }
                    className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
                  >
                    <option value="">请选择分区</option>
                    {partitionOptions.map((partition) => (
                      <option key={partition.tid} value={buildPartitionOptionValue(partition)}>
                        {partition.name}
                      </option>
                    ))}
                  </select>
                </div>
                <div className="space-y-1">
                  <div className="text-xs text-[var(--muted)]">合集（可选）</div>
                  <select
                    value={submissionConfigForm.collectionId}
                    onChange={(event) =>
                      setSubmissionConfigForm((prev) => ({ ...prev, collectionId: event.target.value }))
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
                <div className="space-y-1">
                  <div className="text-xs text-[var(--muted)]">
                    视频类型<span className="ml-1 text-rose-500">必填</span>
                  </div>
                  <select
                    value={submissionConfigForm.videoType}
                    onChange={(event) =>
                      setSubmissionConfigForm((prev) => ({ ...prev, videoType: event.target.value }))
                    }
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
                      {submissionConfigTags.map((tag) => (
                        <span key={tag} className={resolveConfigTagClassName(tag)}>
                          {isConfigActivityTag(tag) ? `#话题 ${tag}` : tag}
                          <button
                            className={resolveConfigTagRemoveClassName(tag)}
                            onClick={() => removeConfigTag(tag)}
                            title="删除标签"
                          >
                            ×
                          </button>
                        </span>
                      ))}
                      <input
                        value={submissionConfigTagInput}
                        onChange={(event) => setSubmissionConfigTagInput(event.target.value)}
                        onKeyDown={handleConfigTagKeyDown}
                        placeholder="回车添加标签"
                        className="min-w-[120px] flex-1 bg-transparent text-sm text-[var(--ink)] focus:outline-none"
                      />
                    </div>
                  </div>
                </div>
                <div>{renderConfigActivityTopicSelector()}</div>
              </div>
              <div className="flex justify-end">
                <button
                  className="rounded-full bg-[var(--accent)] px-4 py-2 text-sm font-semibold text-white disabled:cursor-not-allowed disabled:opacity-60"
                  onClick={handleClipSubmit}
                  disabled={clipSubmitSubmitting}
                >
                  {clipSubmitSubmitting ? "提交中..." : "提交投稿"}
                </button>
              </div>
            </div>
          )}
        </div>
      </div>
    );
  }

  if (submissionConfigView) {
    const hasPartitionOption = configPartitions.some(
      (item) => String(item.tid) === String(submissionConfigForm.partitionId),
    );
    const partitionOptions =
      submissionConfigForm.partitionId && !hasPartitionOption
        ? [
            ...configPartitions,
            { tid: submissionConfigForm.partitionId, name: `当前分区(${submissionConfigForm.partitionId})` },
          ]
        : configPartitions;
    const partitionSelectValue = resolvePartitionSelectValue(
      submissionConfigForm.partitionId,
      partitionOptions,
    );
    const hasCollectionOption = configCollections.some(
      (item) => String(item.seasonId) === String(submissionConfigForm.collectionId),
    );
    const collectionOptions =
      submissionConfigForm.collectionId && !hasCollectionOption
        ? [
            ...configCollections,
            { seasonId: submissionConfigForm.collectionId, name: `当前合集(${submissionConfigForm.collectionId})` },
          ]
        : configCollections;

    return (
      <div className="space-y-6">
        <div className="rounded-2xl bg-[var(--surface)]/90 p-6 shadow-sm ring-1 ring-black/5">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div>
              <p className="text-sm uppercase tracking-[0.2em] text-[var(--muted)]">投稿配置</p>
              <h2 className="text-2xl font-semibold text-[var(--ink)]">主播投稿配置</h2>
            </div>
            <button
              className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
              onClick={closeSubmissionConfigView}
            >
              返回列表
            </button>
          </div>
          <div className="mt-3 text-sm text-[var(--muted)]">
            主播：{submissionConfigView.nickname || "未知主播"}（房间号 {submissionConfigView.uid}）
          </div>
          {submissionConfigMessage ? (
            <div className="mt-4 rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-700">
              {submissionConfigMessage}
            </div>
          ) : null}
        </div>
        <div className="rounded-2xl bg-white/80 p-6 shadow-sm ring-1 ring-black/5">
          {submissionConfigLoading ? (
            <div className="text-sm text-[var(--muted)]">加载中...</div>
          ) : (
            <div className="space-y-4">
              <div className="space-y-1">
                <div className="text-xs text-[var(--muted)]">
                  视频标题<span className="ml-1 text-rose-500">必填</span>
                </div>
                <input
                  value={submissionConfigForm.title}
                  onChange={(event) =>
                    setSubmissionConfigForm((prev) => ({ ...prev, title: event.target.value }))
                  }
                  placeholder="请输入投稿标题"
                  className="w-full rounded-xl border border-black/10 bg-white/80 px-3 py-2 text-sm text-[var(--ink)] focus:border-[var(--accent)] focus:outline-none"
                />
              </div>
              <div className="space-y-1">
                <div className="text-xs text-[var(--muted)]">视频描述（可选）</div>
                <textarea
                  value={submissionConfigForm.description}
                  onChange={(event) =>
                    setSubmissionConfigForm((prev) => ({ ...prev, description: event.target.value }))
                  }
                  placeholder="视频描述"
                  rows={2}
                  className="w-full rounded-xl border border-black/10 bg-white/80 px-3 py-2 text-sm text-[var(--ink)] focus:border-[var(--accent)] focus:outline-none"
                />
              </div>
              <div className="grid gap-2 lg:grid-cols-3">
                <div className="space-y-1">
                  <div className="text-xs text-[var(--muted)]">
                    B站分区<span className="ml-1 text-rose-500">必填</span>
                  </div>
                  <select
                    value={partitionSelectValue}
                    onChange={(event) =>
                      setSubmissionConfigForm((prev) => ({
                        ...prev,
                        partitionId: parsePartitionOptionValue(event.target.value),
                      }))
                    }
                    className="w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none"
                  >
                    <option value="">请选择分区</option>
                    {partitionOptions.map((partition) => (
                      <option key={partition.tid} value={buildPartitionOptionValue(partition)}>
                        {partition.name}
                      </option>
                    ))}
                  </select>
                </div>
                <div className="space-y-1">
                  <div className="text-xs text-[var(--muted)]">合集（可选）</div>
                  <select
                    value={submissionConfigForm.collectionId}
                    onChange={(event) =>
                      setSubmissionConfigForm((prev) => ({ ...prev, collectionId: event.target.value }))
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
                <div className="space-y-1">
                  <div className="text-xs text-[var(--muted)]">
                    视频类型<span className="ml-1 text-rose-500">必填</span>
                  </div>
                  <select
                    value={submissionConfigForm.videoType}
                    onChange={(event) =>
                      setSubmissionConfigForm((prev) => ({ ...prev, videoType: event.target.value }))
                    }
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
                      {submissionConfigTags.map((tag) => (
                        <span key={tag} className={resolveConfigTagClassName(tag)}>
                          {isConfigActivityTag(tag) ? `#话题 ${tag}` : tag}
                          <button
                            className={resolveConfigTagRemoveClassName(tag)}
                            onClick={() => removeConfigTag(tag)}
                            title="删除标签"
                          >
                            ×
                          </button>
                        </span>
                      ))}
                      <input
                        value={submissionConfigTagInput}
                        onChange={(event) => setSubmissionConfigTagInput(event.target.value)}
                        onKeyDown={handleConfigTagKeyDown}
                        placeholder="回车添加标签"
                        className="min-w-[120px] flex-1 bg-transparent text-sm text-[var(--ink)] focus:outline-none"
                      />
                    </div>
                  </div>
                </div>
                <div>{renderConfigActivityTopicSelector()}</div>
              </div>
              <div className="flex justify-end">
                <button
                  className="rounded-full bg-[var(--accent)] px-4 py-2 text-sm font-semibold text-white disabled:cursor-not-allowed disabled:opacity-60"
                  onClick={handleSaveSubmissionConfig}
                  disabled={submissionConfigSubmitting}
                >
                  {submissionConfigSubmitting ? "保存中..." : "保存"}
                </button>
              </div>
            </div>
          )}
        </div>
      </div>
    );
  }

  if (recordView) {
    const formatSize = (bytes) => {
      if (!bytes) return "-";
      if (bytes >= 1024 * 1024 * 1024) return (bytes / (1024 * 1024 * 1024)).toFixed(2) + " GB";
      if (bytes >= 1024 * 1024) return (bytes / (1024 * 1024)).toFixed(1) + " MB";
      return (bytes / 1024).toFixed(0) + " KB";
    };
    const toLocalDateStr = (isoStr) => {
      if (!isoStr) return "未知日期";
      try {
        return new Date(isoStr).toLocaleDateString("zh-CN", { year: "numeric", month: "2-digit", day: "2-digit" });
      } catch {
        return isoStr.slice(0, 10);
      }
    };
    const toLocalTimeStr = (isoStr) => {
      if (!isoStr) return "-";
      try {
        return new Date(isoStr).toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit", second: "2-digit" });
      } catch {
        return isoStr.slice(11, 19);
      }
    };
    const getDirPath = (filePath) => {
      if (!filePath) return "";
      const sep = filePath.includes("/") ? "/" : "\\";
      const parts = filePath.split(sep);
      parts.pop();
      return parts.join(sep);
    };
    // 按天分组
    const grouped = recordList.reduce((acc, record) => {
      const day = toLocalDateStr(record.startTime);
      if (!acc[day]) acc[day] = { day, records: [] };
      acc[day].records.push(record);
      return acc;
    }, {});
    const days = Object.values(grouped);
    return (
      <div className="space-y-6">
        <div className="rounded-2xl bg-[var(--surface)]/90 p-6 shadow-sm ring-1 ring-black/5">
          <div className="flex flex-wrap items-center justify-between gap-4">
            <div>
              <p className="text-sm uppercase tracking-[0.2em] text-[var(--muted)]">录播管理</p>
              <h2 className="text-2xl font-semibold text-[var(--ink)]">{recordView.nickname || recordView.uid} 的录播记录</h2>
            </div>
            <button
              className="rounded-full border border-black/10 bg-white px-4 py-2 text-sm font-semibold text-[var(--ink)] transition hover:border-black/20"
              onClick={() => {
                setRecordView(null);
                setDetailView(null);
                setClipView(null);
                setClipListError("");
              }}
            >
              返回
            </button>
          </div>
        </div>
        <div className="rounded-2xl bg-[var(--surface)]/90 p-6 shadow-sm ring-1 ring-black/5">
          {recordListLoading ? (
            <div className="text-sm text-[var(--muted)]">加载中...</div>
          ) : recordListError ? (
            <div className="rounded-lg border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700">{recordListError}</div>
          ) : days.length === 0 ? (
            <div className="text-sm text-[var(--muted)]">暂无录播记录</div>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b border-black/5 text-left text-xs text-[var(--muted)]">
                    <th className="pb-2 pr-6 font-medium">直播日期</th>
                    <th className="pb-2 pr-6 font-medium">直播标题</th>
                    <th className="pb-2 pr-6 font-medium">录播文件目录</th>
                    <th className="pb-2 font-medium">操作</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-black/5">
                  {days.map(({ day, records }) => {
                    const dirPath = getDirPath(records[0]?.filePath);
                    const dayTitle = records[records.length - 1]?.title || records[0]?.title || "";
                    const analyzing = analyzingTasks[day];
                    const isRunning = analyzing?.status === "RUNNING";
                    const analyzeLabel = isRunning
                      ? "分析中..."
                      : analyzing?.status === "FAILED"
                        ? "分析失败"
                        : "分析";
                    return (
                      <tr key={day} className="text-[var(--ink)]">
                        <td className="py-3 pr-6 tabular-nums whitespace-nowrap">{day}</td>
                        <td className="py-3 pr-6 text-[var(--muted)]">{dayTitle || "-"}</td>
                        <td className="py-3 pr-6">
                          {dirPath ? (
                            <button
                              className="text-left text-xs text-blue-600 hover:underline break-all"
                              onClick={() => invokeCommand("anchor_open_record_dir", { filePath: records[0].filePath })}
                              title="点击打开目录"
                            >
                              {dirPath}
                            </button>
                          ) : "-"}
                        </td>
                        <td className="py-3">
                          <div className="flex flex-wrap gap-2">
                            <button
                              className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)] transition hover:border-black/20"
                              onClick={() => setDetailView({ day, records })}
                            >
                              查看
                            </button>
                            <button
                              className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)] transition hover:border-black/20 disabled:cursor-not-allowed disabled:opacity-60"
                              onClick={() => handleAnalyzeClips(day, records)}
                              disabled={isRunning}
                            >
                              {analyzeLabel}
                            </button>
                            <button
                              className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)] transition hover:border-black/20"
                              onClick={() => handleOpenClipList(day, records)}
                            >
                              切片记录
                            </button>
                          </div>
                          {analyzing?.errorMessage ? (
                            <div className="mt-1 text-[10px] text-amber-600">{analyzing.errorMessage}</div>
                          ) : null}
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )}
        </div>

        {detailView ? (
          <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4" onClick={() => setDetailView(null)}>
            <div className="w-full max-w-5xl rounded-2xl bg-white shadow-xl" onClick={(e) => e.stopPropagation()}>
              <div className="flex items-center justify-between border-b border-black/5 px-6 py-4">
                <div>
                  <div className="text-xs uppercase tracking-widest text-[var(--muted)]">录播文件详情</div>
                  <div className="mt-0.5 text-base font-semibold text-[var(--ink)]">{detailView.day}　{detailView.records[detailView.records.length - 1]?.title || ""}</div>
                </div>
                <button
                  className="rounded-full border border-black/10 bg-white px-3 py-1 text-sm font-semibold text-[var(--ink)] hover:border-black/20"
                  onClick={() => setDetailView(null)}
                >
                  关闭
                </button>
              </div>
              <div className="max-h-[60vh] overflow-y-auto px-6 py-4">
                <table className="w-full text-xs">
                  <thead>
                    <tr className="border-b border-black/5 text-left text-[var(--muted)]">
                      <th className="pb-2 pr-4 font-medium">文件名</th>
                      <th className="pb-2 pr-4 font-medium">开始时间</th>
                      <th className="pb-2 pr-4 font-medium">结束时间</th>
                      <th className="pb-2 pr-4 font-medium">大小</th>
                      <th className="pb-2 font-medium">状态</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-black/5">
                    {detailView.records.map((record) => (
                      <tr key={record.id}>
                        <td className="py-2 pr-4 max-w-[200px] truncate" title={record.filePath}>
                          <button
                            className="text-blue-600 hover:underline text-left truncate max-w-full"
                            onClick={() => invokeCommand("anchor_open_record_file", { filePath: record.filePath }).catch((err) => alert(err?.message || "打开失败"))}
                          >
                            {record.filePath.split(/[\/\\]/).pop()}
                          </button>
                        </td>
                        <td className="py-2 pr-4 tabular-nums text-[var(--muted)] whitespace-nowrap">{toLocalTimeStr(record.startTime)}</td>
                        <td className="py-2 pr-4 tabular-nums text-[var(--muted)] whitespace-nowrap">{record.endTime ? toLocalTimeStr(record.endTime) : "-"}</td>
                        <td className="py-2 pr-4 tabular-nums text-[var(--muted)] whitespace-nowrap">{formatSize(record.fileSize)}</td>
                        <td className="py-2">
                          {record.status === "RECORDING" ? (
                            <span className="rounded-full bg-green-100 px-2 py-0.5 text-green-700">录制中</span>
                          ) : record.status === "ERROR" ? (
                            <span className="rounded-full bg-red-100 px-2 py-0.5 text-red-700" title={record.errorMessage || ""}>出错</span>
                          ) : (
                            <span className="rounded-full bg-black/5 px-2 py-0.5 text-[var(--muted)]">已完成</span>
                          )}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </div>
          </div>
        ) : null}
        {clipView ? (
          <div className="fixed inset-0 z-[55] flex items-center justify-center bg-black/40 p-4" onClick={() => setClipView(null)}>
            <div className="w-full max-w-5xl rounded-2xl bg-white shadow-xl" onClick={(e) => e.stopPropagation()}>
              <div className="flex items-center justify-between border-b border-black/5 px-6 py-4">
                <div>
                  <div className="text-xs uppercase tracking-widest text-[var(--muted)]">切片记录</div>
                  <div className="mt-0.5 text-base font-semibold text-[var(--ink)]">
                    {clipView.dateLabel} 切片记录
                  </div>
                </div>
            <button
              className="rounded-full border border-black/10 bg-white px-3 py-1 text-sm font-semibold text-[var(--ink)] hover:border-black/20"
              onClick={() => {
                setClipView(null);
                setClipEdit(null);
              }}
            >
              关闭
            </button>
              </div>
              <div className="max-h-[60vh] overflow-y-auto px-6 py-4">
                {clipListLoading ? (
                  <div className="text-sm text-[var(--muted)]">加载中...</div>
                ) : clipListError ? (
                  <div className="rounded-lg border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700">
                    {clipListError}
                  </div>
                ) : clipListMessage ? (
                  <div className="rounded-lg border border-emerald-200 bg-emerald-50 px-3 py-2 text-sm text-emerald-700">
                    {clipListMessage}
                  </div>
                ) : clipView.records.length === 0 ? (
                  <div className="text-sm text-[var(--muted)]">暂无切片记录</div>
                ) : (
                  <table className="w-full text-xs">
                    <thead>
                      <tr className="border-b border-black/5 text-left text-[var(--muted)]">
                        <th className="pb-2 pr-4 font-medium">序号</th>
                        <th className="pb-2 pr-4 font-medium">源视频</th>
                        <th className="pb-2 pr-4 font-medium">文件名</th>
                        <th className="pb-2 pr-4 font-medium">起始时间</th>
                        <th className="pb-2 pr-4 font-medium">结束时间</th>
                        <th className="pb-2 pr-4 font-medium">时长</th>
                        <th className="pb-2 pr-4 font-medium">弹幕峰值</th>
                        <th className="pb-2 pr-4 font-medium">状态</th>
                        <th className="pb-2 font-medium">操作</th>
                      </tr>
                    </thead>
                    <tbody className="divide-y divide-black/5">
                      {clipView.records.map((record, index) => (
                        <tr key={record.id || record.filePath || index}>
                          <td className="py-2 pr-4 tabular-nums text-[var(--muted)]">{index + 1}</td>
                          <td className="py-2 pr-4 max-w-[240px] truncate" title={record.sourceFilePath}>
                            {record.sourceFilePath ? (
                              <button
                                className="text-blue-600 hover:underline text-left truncate max-w-full"
                                onClick={() =>
                                  invokeCommand("anchor_open_record_file", {
                                    filePath: record.sourceFilePath,
                                  }).catch((err) => alert(err?.message || "打开失败"))
                                }
                              >
                                {record.sourceFilePath.split(/[\/\\]/).pop()}
                              </button>
                            ) : (
                              "-"
                            )}
                          </td>
                          <td className="py-2 pr-4 max-w-[240px] truncate" title={record.filePath}>
                            {record.filePath?.split(/[\/\\]/).pop() || "-"}
                          </td>
                          <td
                            className="py-2 pr-4 tabular-nums text-[var(--muted)]"
                            onDoubleClick={() => startClipEdit(record, "start")}
                          >
                            {clipEdit && clipEdit.id === record.id && clipEdit.field === "start" ? (
                              <input
                                className="w-20 rounded border border-black/10 px-1 py-0.5 text-xs text-[var(--ink)]"
                                value={clipEdit.value}
                                autoFocus
                                onChange={(event) =>
                                  setClipEdit((prev) => (prev ? { ...prev, value: event.target.value } : prev))
                                }
                                onBlur={() => submitClipEdit(record)}
                                onKeyDown={(event) => {
                                  if (event.key === "Enter") {
                                    submitClipEdit(record);
                                  } else if (event.key === "Escape") {
                                    setClipEdit(null);
                                  }
                                }}
                              />
                            ) : (
                              formatSeconds(record.startOffset)
                            )}
                          </td>
                          <td
                            className="py-2 pr-4 tabular-nums text-[var(--muted)]"
                            onDoubleClick={() => startClipEdit(record, "end")}
                          >
                            {clipEdit && clipEdit.id === record.id && clipEdit.field === "end" ? (
                              <input
                                className="w-20 rounded border border-black/10 px-1 py-0.5 text-xs text-[var(--ink)]"
                                value={clipEdit.value}
                                autoFocus
                                onChange={(event) =>
                                  setClipEdit((prev) => (prev ? { ...prev, value: event.target.value } : prev))
                                }
                                onBlur={() => submitClipEdit(record)}
                                onKeyDown={(event) => {
                                  if (event.key === "Enter") {
                                    submitClipEdit(record);
                                  } else if (event.key === "Escape") {
                                    setClipEdit(null);
                                  }
                                }}
                              />
                            ) : (
                              formatSeconds(record.endOffset)
                            )}
                          </td>
                          <td className="py-2 pr-4 tabular-nums text-[var(--muted)]">
                            {formatSeconds(record.duration)}
                          </td>
                          <td className="py-2 pr-4 tabular-nums text-[var(--muted)]">
                            {record.peakCount ?? "-"}
                          </td>
                          <td className="py-2 pr-4 text-[var(--muted)]">{clipStatusLabel(record)}</td>
                          <td className="py-2">
                            <div className="flex flex-wrap gap-2">
                              <button
                                className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                                onClick={() =>
                                  invokeCommand("anchor_open_record_file", {
                                    filePath: record.filePath,
                                  }).catch((err) => alert(err?.message || "打开失败"))
                                }
                              >
                                打开文件
                              </button>
                              <button
                                className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                                onClick={() => handleOpenClipSubmit(record)}
                              >
                                投稿
                              </button>
                              <button
                                className="rounded-full border border-red-200 bg-white px-3 py-1 text-xs font-semibold text-red-600 hover:border-red-300"
                                onClick={async () => {
                                  if (!record?.id) return;
                                  const confirmed = await dialogConfirm("确认删除该切片文件和记录？", {
                                    title: "删除切片",
                                  });
                                  if (!confirmed) return;
                                  setClipListMessage("");
                                  setClipListError("");
                                  try {
                                    await invokeCommand("anchor_clip_delete", { clipId: record.id });
                                    await refreshClipList("delete");
                                    setClipListMessage("切片已删除");
                                  } catch (error) {
                                    setClipListError(error?.message || "删除失败");
                                  }
                                }}
                              >
                                删除
                              </button>
                              <button
                                className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)] disabled:cursor-not-allowed disabled:opacity-60"
                                onClick={async () => {
                                  if (!record?.id) return;
                                  const isRunning = String(record.status || "").toUpperCase() === "RUNNING";
                                  if (isRunning) return;
                                  setReclipLoading((prev) => ({ ...prev, [record.id]: true }));
                                  setClipView((prev) => {
                                    if (!prev) return prev;
                                    const nextRecords = prev.records.map((item) =>
                                      item.id === record.id ? { ...item, status: "RUNNING" } : item
                                    );
                                    return { ...prev, records: nextRecords };
                                  });
                                  setClipListMessage("");
                                  setClipListError("");
                                  try {
                                    await invokeCommand("anchor_clip_reclip", { clipId: record.id });
                                    setClipListMessage("已开始重新剪辑");
                                  } catch (error) {
                                    const message =
                                      error?.message ||
                                      (typeof error === "string" ? error : error?.toString?.()) ||
                                      "重新剪辑失败";
                                    setClipListError(message);
                                  } finally {
                                    setReclipLoading((prev) => ({ ...prev, [record.id]: false }));
                                    await refreshClipList("reclip");
                                  }
                                }}
                                disabled={
                                  reclipLoading[record.id] ||
                                  String(record.status || "").toUpperCase() === "RUNNING"
                                }
                              >
                                {reclipLoading[record.id] || String(record.status || "").toUpperCase() === "RUNNING"
                                  ? "剪辑中..."
                                  : "重新剪辑"}
                              </button>
                            </div>
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                )}
              </div>
            </div>
          </div>
        ) : null}
      </div>
    );
  }

  return (
    <div className="space-y-6">
      <div className="rounded-2xl bg-[var(--surface)]/90 p-6 shadow-sm ring-1 ring-black/5">
        <div className="flex flex-wrap items-center justify-between gap-4">
          <div>
            <p className="text-sm uppercase tracking-[0.2em] text-[var(--muted)]">主播订阅</p>
            <h2 className="text-2xl font-semibold text-[var(--ink)]">主播订阅管理</h2>
          </div>
          <div className="flex gap-2">
            <button
              className="rounded-full border border-black/10 bg-white px-4 py-2 text-sm font-semibold text-[var(--ink)] transition hover:border-black/20"
              onClick={loadAnchors}
              disabled={loading}
            >
              刷新
            </button>
          </div>
        </div>
        <div className="mt-4 text-sm text-[var(--muted)]">
          从这里开始订阅直播间，状态会在卡片上实时更新。
        </div>
        {message ? (
          <div className="mt-4 rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-700">
            {message}
          </div>
        ) : null}
      </div>

      <div className="rounded-2xl bg-white/80 p-6 shadow-sm ring-1 ring-black/5">
        <div className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">直播间列表</div>
        <div className="mt-4 grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
          <div className="rounded-2xl border border-dashed border-black/15 bg-white/70 p-4">
            <div className="flex items-center gap-2 text-sm font-semibold text-[var(--ink)]">
              <span className="text-lg">＋</span>
              新增直播间
            </div>
            <input
              value={newAnchorUid}
              onChange={(event) => setNewAnchorUid(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  handleSubscribe();
                }
              }}
              placeholder="房间号或链接"
              className="mt-3 w-full rounded-xl border border-black/10 bg-white/80 px-3 py-2 text-sm text-[var(--ink)] focus:border-[var(--accent)] focus:outline-none"
            />
            <button
              className="mt-3 w-full rounded-xl bg-[var(--accent)] px-4 py-2 text-sm font-semibold text-white shadow-sm transition hover:brightness-110"
              onClick={handleSubscribe}
              disabled={loading}
            >
              订阅
            </button>
            <div className="mt-2 text-xs text-[var(--muted)]">支持房间号或直播间链接</div>
          </div>
          {anchors.map((anchor) => (
            <div key={anchor.id} className="rounded-2xl border border-black/5 bg-white/90 p-4">
              <div className="flex items-start justify-between gap-3">
                <div className="flex items-center gap-3">
                  <div className="h-12 w-12 overflow-hidden rounded-full bg-black/5">
                    {anchorAvatars[anchor.uid] ? (
                      <img
                        src={anchorAvatars[anchor.uid]}
                        alt={anchor.nickname || "主播"}
                        className="h-full w-full object-cover"
                      />
                    ) : (
                      <div className="flex h-full w-full items-center justify-center text-xs text-[var(--muted)]">
                        头像
                      </div>
                    )}
                  </div>
                  <div>
                    <div className="text-sm font-semibold text-[var(--ink)]">
                      {anchor.nickname || "未知主播"}
                    </div>
                    <div className="text-xs text-[var(--muted)]">房间号：{anchor.uid}</div>
                  </div>
                </div>
                <div className="flex flex-col items-end gap-2">
                  <span
                    className={`rounded-full px-2 py-0.5 text-xs font-semibold ${
                      anchor.liveStatus === 1
                        ? "bg-emerald-500/10 text-emerald-600"
                        : "bg-slate-500/10 text-slate-600"
                    }`}
                  >
                    {statusLabel(anchor.liveStatus)}
                  </span>
                  {anchor.recordingStatus ? (
                    <span className="rounded-full bg-amber-500/10 px-2 py-0.5 text-xs font-semibold text-amber-600">
                      录制中
                    </span>
                  ) : anchor.autoRecord ? (
                    <span className="rounded-full bg-orange-500/10 px-2 py-0.5 text-xs font-semibold text-orange-600">
                      监控中
                    </span>
                  ) : null}
                </div>
              </div>
              {anchor.liveStatus === 1 ? (
                <div className="mt-3 text-sm text-[var(--ink)]">
                  <div className="font-semibold">{anchor.liveTitle || "直播标题"}</div>
                  <div className="text-xs text-[var(--muted)]">
                    {anchor.category || "未知分区"}
                  </div>
                </div>
              ) : (
                <div className="mt-3 text-xs text-[var(--muted)]">当前未开播</div>
              )}
              <div className="mt-3 flex flex-wrap items-center gap-2 text-xs text-[var(--muted)]">
                <span>自动录制：{anchor.autoRecord ? "已开启" : "已关闭"}</span>
                <button
                  className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                  onClick={() => handleAutoRecordToggle(anchor)}
                >
                  {anchor.autoRecord ? "关闭" : "开启"}
                </button>
                <span>同步上传：{anchor.baiduSyncEnabled ? "已开启" : "未开启"}</span>
                <button
                  className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                  onClick={() => handleSyncToggle(anchor)}
                >
                  {anchor.baiduSyncEnabled ? "关闭" : "开启"}
                </button>
                {anchor.baiduSyncEnabled && anchor.baiduSyncPath ? (
                  <span>同步路径：{anchor.baiduSyncPath}</span>
                ) : null}
                <span>上次检查：{formatDateTime(anchor.lastCheckTime)}</span>
              </div>
              <div className="mt-3 flex flex-wrap gap-2">
                {anchor.recordingStatus ? (
                  <button
                    className="rounded-full border border-black/10 bg-white px-3 py-1.5 text-xs font-semibold text-[var(--ink)]"
                    onClick={() => handleStopRecord(anchor)}
                  >
                    停止录制
                  </button>
                ) : (
                  <button
                    className="rounded-full border border-black/10 bg-white px-3 py-1.5 text-xs font-semibold text-[var(--ink)]"
                    onClick={() => handleStartRecord(anchor)}
                  >
                    开始录制
                  </button>
                )}
                <button
                  className="rounded-full border border-black/10 bg-white px-3 py-1.5 text-xs font-semibold text-[var(--ink)]"
                  onClick={() => handleOpenRecordView(anchor)}
                >
                  录播管理
                </button>
                <button
                  className="rounded-full border border-black/10 bg-white px-3 py-1.5 text-xs font-semibold text-[var(--ink)]"
                  onClick={() => handleOpenSubmissionConfig(anchor)}
                >
                  投稿配置
                </button>
                <button
                  className="rounded-full border border-black/10 bg-white px-3 py-1.5 text-xs font-semibold text-[var(--ink)]"
                  onClick={() => handleUnsubscribe(anchor)}
                >
                  取消订阅
                </button>
                {anchor.baiduSyncEnabled ? (
                  <button
                    className="rounded-full border border-black/10 bg-white px-3 py-1.5 text-xs font-semibold text-[var(--ink)]"
                    onClick={() => handleOpenSyncConfig(anchor)}
                  >
                    同步配置
                  </button>
                ) : null}
              </div>
              {anchor.recordingFile ? (
                <div className="mt-2 text-xs text-[var(--muted)]">
                  当前文件：{anchor.recordingFile}
                </div>
              ) : null}
              {anchor.recordingStartTime ? (
                <div className="mt-1 text-xs text-[var(--muted)]">
                  开始时间：{formatDateTime(anchor.recordingStartTime)}
                </div>
              ) : null}
            </div>
          ))}
        </div>
        {anchors.length === 0 ? (
          <div className="mt-4 text-sm text-[var(--muted)]">暂无订阅记录。</div>
        ) : null}
      </div>
      {syncAnchor ? (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
          <div className="w-[420px] rounded-2xl bg-[var(--block-color)] p-5 text-sm text-[var(--content-color)] shadow-xl">
            <div className="text-base font-semibold">同步配置</div>
            <div className="mt-2 text-xs text-[var(--desc-color)]">
              主播：{syncAnchor.nickname || syncAnchor.uid}
            </div>
            <div className="mt-3 text-xs text-[var(--desc-color)]">
              录播分段上传到百度网盘的目录路径
            </div>
            <div className="mt-2 flex flex-wrap items-center gap-2 text-xs">
              <div className="rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-[var(--ink)]">
                {syncPath || "未配置"}
              </div>
              <button
                className="rounded-full border border-black/10 bg-white px-3 py-1 font-semibold text-[var(--ink)]"
                onClick={handleOpenSyncPicker}
              >
                选择目录
              </button>
            </div>
            {syncMessage ? (
              <div className="mt-3 text-xs text-amber-600">{syncMessage}</div>
            ) : null}
            <div className="mt-4 flex justify-end gap-2">
              <button className="h-9 rounded-lg px-4" onClick={handleCloseSyncConfig}>
                取消
              </button>
              <button
                className="h-9 rounded-lg px-4"
                onClick={handleSaveSyncConfig}
                disabled={syncLoading}
              >
                保存
              </button>
            </div>
          </div>
        </div>
      ) : null}
      {syncPickerOpen ? (
        <div className="fixed inset-0 z-[60] flex items-center justify-center bg-black/50">
          <div className="w-[520px] rounded-2xl bg-[var(--block-color)] p-5 text-sm text-[var(--content-color)] shadow-xl">
            <div className="text-base font-semibold">选择百度网盘目录</div>
            <div className="mt-2 flex items-center gap-2">
              <div className="flex-1 rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-xs text-[var(--ink)]">
                {syncBrowserPath}
              </div>
              <button
                className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                onClick={handleSyncGoParent}
                disabled={syncBrowserPath === "/"}
              >
                上级
              </button>
              <button
                className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                onClick={() => loadSyncFolders(syncBrowserPath)}
                disabled={syncBrowseLoading}
              >
                刷新
              </button>
            </div>
            <div className="mt-3 max-h-64 overflow-auto rounded-xl border border-black/10 bg-white/80 p-2 text-xs text-[var(--ink)]">
              {syncBrowseLoading ? (
                <div className="py-6 text-center text-[var(--desc-color)]">加载中...</div>
              ) : syncBrowseError ? (
                <div className="py-6 text-center text-amber-600">{syncBrowseError}</div>
              ) : syncFolders.length === 0 ? (
                <div className="py-6 text-center text-[var(--desc-color)]">暂无目录</div>
              ) : (
                syncFolders.map((folder) => (
                  <button
                    key={folder.path}
                    className="flex w-full items-center gap-2 rounded-lg px-2 py-2 text-left hover:bg-black/5"
                    onClick={() => handleSyncEnterFolder(folder)}
                  >
                    <span className="text-[10px] font-semibold text-[var(--muted)]">DIR</span>
                    <span className="text-sm">{folder.name}</span>
                  </button>
                ))
              )}
            </div>
            <div className="mt-4 flex justify-end gap-2">
              <button className="h-9 rounded-lg px-4" onClick={handleCloseSyncPicker}>
                取消
              </button>
              <button
                className="h-9 rounded-lg px-4"
                onClick={handleConfirmSyncPicker}
                disabled={syncBrowseLoading}
              >
                选择当前目录
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  );
}
