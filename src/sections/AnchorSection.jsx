import { useEffect, useRef, useState } from "react";
import { invokeCommand } from "../lib/tauri";
import { formatDateTime } from "../lib/format";
import RecordManageView from "./anchor/RecordManageView";
import ClipReviewView from "./anchor/ClipReviewView";
import BatchSubmitView from "./anchor/BatchSubmitView";
import { clipFormError, createClipSubmission, joinClipTitle } from "./anchor/clipSubmit";

export default function AnchorSection({ onFullPageChange }) {
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
  const [recordConfigAnchor, setRecordConfigAnchor] = useState(null);
  const [recordConfigMode, setRecordConfigMode] = useState("DAILY");
  const [recordConfigDates, setRecordConfigDates] = useState([]);
  const [recordConfigKeepAfter, setRecordConfigKeepAfter] = useState(false);
  const [recordConfigCalMonth, setRecordConfigCalMonth] = useState(() => {
    const now = new Date();
    return { year: now.getFullYear(), month: now.getMonth() };
  });
  const [recordConfigLoading, setRecordConfigLoading] = useState(false);
  const [recordConfigMessage, setRecordConfigMessage] = useState("");
  const [recordConfigHelpOpen, setRecordConfigHelpOpen] = useState(false);
  const [syncBrowserPath, setSyncBrowserPath] = useState("/");
  const [syncFolders, setSyncFolders] = useState([]);
  const [syncBrowseLoading, setSyncBrowseLoading] = useState(false);
  const [syncBrowseError, setSyncBrowseError] = useState("");
  const [recordView, setRecordView] = useState(null);
  const [recordList, setRecordList] = useState([]);
  const [recordListLoading, setRecordListLoading] = useState(false);
  const [recordListError, setRecordListError] = useState("");
  // 正在评阅的场次；非空时录播管理页显示评阅工作台
  const [reviewSession, setReviewSession] = useState(null);
  useEffect(() => {
    onFullPageChange?.(Boolean(recordView));
    return () => onFullPageChange?.(false);
  }, [recordView]);
  const [clipSubmitView, setClipSubmitView] = useState(null);
  const [batchSubmitView, setBatchSubmitView] = useState(null);
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
      const auth = await invokeCommand("auth_status");
      const profile = extractCurrentAuthProfile(auth);
      setConfigUpProfile(profile);
      const data = await invokeCommand("bilibili_partitions", {
        bilibiliUid: profile?.uid ? Number(profile.uid) : null,
      });
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
      const data = await invokeCommand("bilibili_collections", {
        mid: profile.uid,
        bilibiliUid: Number(profile.uid),
      });
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
      const auth = await invokeCommand("auth_status");
      const profile = extractCurrentAuthProfile(auth);
      setConfigUpProfile(profile);
      const data = await invokeCommand("bilibili_topics", {
        partitionId: partitionId ? Number(partitionId) : null,
        title: normalizedKeyword || null,
        bilibiliUid: profile?.uid ? Number(profile.uid) : null,
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
    setReviewSession(null);
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
      // 自动切片：标题为投稿设置里的标题（如【主播名】）加切片标题，简介和标签用切片上的
      const clipTitle = record.titles?.[0]?.trim();
      if (clipTitle) {
        setSubmissionConfigForm((prev) => ({
          ...prev,
          title: joinClipTitle(prev.title, clipTitle),
          description: record.description?.trim() || prev.description,
        }));
        const clipTags = String(record.tags || "").split(",").map((t) => t.trim()).filter(Boolean);
        setSubmissionConfigTags((prev) => [...new Set([...prev, ...clipTags])]);
      }
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

  // ── 录制日期配置 ──
  const formatRecordDate = (date) =>
    `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(
      date.getDate(),
    ).padStart(2, "0")}`;

  const handleOpenRecordConfig = (anchor) => {
    setRecordConfigAnchor(anchor);
    setRecordConfigMode(String(anchor?.recordMode || "DAILY").toUpperCase());
    setRecordConfigDates(Array.isArray(anchor?.recordDates) ? [...anchor.recordDates].sort() : []);
    setRecordConfigKeepAfter(Boolean(anchor?.keepRecordAfter));
    const now = new Date();
    setRecordConfigCalMonth({ year: now.getFullYear(), month: now.getMonth() });
    setRecordConfigMessage("");
    setRecordConfigHelpOpen(false);
  };

  const handleCloseRecordConfig = () => {
    if (recordConfigLoading) {
      return;
    }
    setRecordConfigAnchor(null);
    setRecordConfigMessage("");
    setRecordConfigHelpOpen(false);
  };

  const handleToggleRecordDate = (key) => {
    setRecordConfigDates((prev) =>
      prev.includes(key) ? prev.filter((item) => item !== key) : [...prev, key].sort(),
    );
    setRecordConfigMessage("");
  };

  const handleAddRecordDate = (offsetDays) => {
    const target = new Date();
    target.setDate(target.getDate() + offsetDays);
    const key = formatRecordDate(target);
    setRecordConfigDates((prev) => (prev.includes(key) ? prev : [...prev, key].sort()));
    setRecordConfigMessage("");
  };

  const handleSaveRecordConfig = async () => {
    if (!recordConfigAnchor) {
      return;
    }
    if (recordConfigMode === "SELECTED" && recordConfigDates.length === 0) {
      setRecordConfigMessage("「仅指定日期」至少需要选择 1 天；或切换到「每天录制」");
      return;
    }
    setRecordConfigLoading(true);
    setRecordConfigMessage("");
    try {
      await invokeCommand("live_room_record_config_update", {
        roomId: recordConfigAnchor.uid,
        mode: recordConfigMode,
        dates: recordConfigDates,
        keepRecordAfter: recordConfigMode === "SELECTED" && recordConfigKeepAfter,
      });
      await loadAnchors();
      setMessage("录制日期配置已保存");
      setRecordConfigAnchor(null);
      setRecordConfigMessage("");
      setRecordConfigHelpOpen(false);
    } catch (error) {
      setRecordConfigMessage(error?.message || "保存失败");
    } finally {
      setRecordConfigLoading(false);
    }
  };

  const renderRecordDateBadge = (anchor) => {
    const mode = String(anchor?.recordMode || "DAILY").toUpperCase();
    if (mode !== "SELECTED") {
      return (
        <span className="rounded-full bg-black/5 px-2 py-1 text-xs font-semibold text-[var(--muted)]">
          每天录制
        </span>
      );
    }
    const count = Array.isArray(anchor?.recordDates) ? anchor.recordDates.length : 0;
    const suffix = anchor?.keepRecordAfter ? " · 后续转每天" : "";
    return (
      <span className="rounded-full bg-[var(--accent)]/10 px-2 py-1 text-xs font-semibold text-[var(--accent)]">
        仅指定日期（{count} 天）{suffix}
      </span>
    );
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
    const tags = [...new Set(submissionConfigTags.map((tag) => tag.trim()).filter(Boolean))];
    const formError = clipFormError(submissionConfigForm, tags);
    setClipSubmitMessage(formError);
    if (formError) {
      return;
    }
    setClipSubmitSubmitting(true);
    try {
      const auth = await invokeCommand("auth_status");
      if (!auth?.loggedIn) {
        setClipSubmitMessage("请先登录B站账号");
        return;
      }
      await createClipSubmission(clipSubmitView.record, submissionConfigForm, tags);
      // 任务创建成功后把切片标记为已投稿，避免重复投稿
      const clipId = clipSubmitView.record?.id;
      if (clipId != null) {
        await invokeCommand("anchor_clip_mark", { clipId, mark: "SUBMITTED" }).catch(() => {});
      }
      setClipSubmitMessage("已创建投稿任务，字幕烧录和上传在后台进行");
    } catch (error) {
      setClipSubmitMessage(error?.message || String(error) || "提交投稿失败");
    } finally {
      setClipSubmitSubmitting(false);
    }
  };

  // 评阅页批量投稿：加载投稿设置和分区、合集列表后进入批量投稿页
  const openBatchSubmit = async (records) => {
    await Promise.all([loadConfigPartitions(), loadConfigCollections()]);
    const config = (await invokeCommand("anchor_submission_config_get", { roomId: recordView.uid })) || {};
    setBatchSubmitView({ records, config });
  };

  if (batchSubmitView) {
    return (
      <BatchSubmitView
        anchor={recordView}
        records={batchSubmitView.records}
        config={batchSubmitView.config}
        partitions={configPartitions}
        collections={configCollections}
        onBack={() => setBatchSubmitView(null)}
      />
    );
  }

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
    if (reviewSession) {
      return (
        <ClipReviewView
          anchor={recordView}
          session={reviewSession}
          onBack={() => setReviewSession(null)}
          onSubmitClip={(clip) => handleOpenClipSubmit(clip)}
          onBatchSubmit={openBatchSubmit}
        />
      );
    }
    return (
      <RecordManageView
        anchor={recordView}
        records={recordList}
        loading={recordListLoading}
        error={recordListError}
        onBack={() => {
          setRecordView(null);
          setReviewSession(null);
        }}
        onOpenReview={(session) => setReviewSession(session)}
        onReload={() => invokeCommand("anchor_record_list", { roomId: recordView.uid }).then((data) => setRecordList(data || [])).catch(() => {})}
      />
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
                <button
                  className="cursor-pointer"
                  onClick={() => handleOpenRecordConfig(anchor)}
                >
                  {renderRecordDateBadge(anchor)}
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
                  className="rounded-full bg-[var(--accent)] px-3 py-1.5 text-xs font-semibold text-white shadow-sm transition hover:brightness-110"
                  onClick={() => handleOpenRecordConfig(anchor)}
                >
                  录制日期
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
      {recordConfigAnchor ? (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
          <div className="w-[460px] max-h-[88vh] overflow-y-auto rounded-2xl bg-[var(--block-color)] p-5 text-sm text-[var(--content-color)] shadow-xl">
            <div className="flex items-start justify-between">
              <div>
                <div className="text-base font-semibold">录制日期</div>
                <div className="mt-1 text-xs text-[var(--desc-color)]">
                  主播：{recordConfigAnchor.nickname || recordConfigAnchor.uid}
                </div>
              </div>
              <button
                className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                onClick={handleCloseRecordConfig}
              >
                关闭
              </button>
            </div>

            <div className="mt-4 space-y-2">
              <div className="text-xs text-[var(--desc-color)]">录制模式</div>
              <div className="flex gap-1 rounded-xl bg-black/5 p-1">
                <button
                  className={`flex-1 rounded-lg px-3 py-2 text-xs font-semibold transition ${
                    recordConfigMode === "DAILY"
                      ? "bg-white text-[var(--ink)] shadow-sm"
                      : "text-[var(--muted)]"
                  }`}
                  onClick={() => setRecordConfigMode("DAILY")}
                >
                  每天录制
                </button>
                <button
                  className={`flex-1 rounded-lg px-3 py-2 text-xs font-semibold transition ${
                    recordConfigMode === "SELECTED"
                      ? "bg-white text-[var(--ink)] shadow-sm"
                      : "text-[var(--muted)]"
                  }`}
                  onClick={() => setRecordConfigMode("SELECTED")}
                >
                  仅指定日期
                </button>
              </div>
              <div className="rounded-lg border border-black/10 bg-white/60 px-3 py-2 text-xs text-[var(--muted)]">
                {recordConfigMode === "DAILY"
                  ? "每天开播都会自动录制（与现有行为一致）"
                  : "只在下方的日期开播时自动录制"}
              </div>
            </div>

            <div
              className={
                recordConfigMode === "DAILY"
                  ? "pointer-events-none mt-4 space-y-3 opacity-40"
                  : "mt-4 space-y-3"
              }
            >
              <div className="flex items-center justify-between">
                <button
                  className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                  onClick={() =>
                    setRecordConfigCalMonth((prev) => {
                      const month = prev.month - 1;
                      return month < 0
                        ? { year: prev.year - 1, month: 11 }
                        : { year: prev.year, month };
                    })
                  }
                >
                  ‹
                </button>
                <div className="text-sm font-semibold text-[var(--ink)]">
                  {recordConfigCalMonth.year} 年 {recordConfigCalMonth.month + 1} 月
                </div>
                <button
                  className="rounded-full border border-black/10 bg-white px-2 py-1 text-xs font-semibold text-[var(--ink)]"
                  onClick={() =>
                    setRecordConfigCalMonth((prev) => {
                      const month = prev.month + 1;
                      return month > 11
                        ? { year: prev.year + 1, month: 0 }
                        : { year: prev.year, month };
                    })
                  }
                >
                  ›
                </button>
              </div>
              <div className="grid grid-cols-7 gap-1 text-center text-xs text-[var(--muted)]">
                {["日", "一", "二", "三", "四", "五", "六"].map((label) => (
                  <div key={label}>{label}</div>
                ))}
              </div>
              <div className="grid grid-cols-7 gap-1">
                {(() => {
                  const { year, month } = recordConfigCalMonth;
                  const firstWeekday = new Date(year, month, 1).getDay();
                  const daysInMonth = new Date(year, month + 1, 0).getDate();
                  const todayKey = formatRecordDate(new Date());
                  const cells = [];
                  for (let i = 0; i < firstWeekday; i += 1) {
                    cells.push(<div key={`pad-${i}`} />);
                  }
                  for (let day = 1; day <= daysInMonth; day += 1) {
                    const date = new Date(year, month, day);
                    const key = formatRecordDate(date);
                    const isPast = key < todayKey;
                    const isSelected = recordConfigDates.includes(key);
                    const isToday = key === todayKey;
                    cells.push(
                      <button
                        key={key}
                        disabled={isPast}
                        onClick={() => handleToggleRecordDate(key)}
                        className={`rounded-lg py-2 text-sm transition ${
                          isSelected
                            ? "bg-[var(--accent)] font-semibold text-white"
                            : "text-[var(--ink)] hover:bg-[var(--accent)] hover:text-white"
                        } ${isPast ? "cursor-not-allowed opacity-30 hover:bg-transparent hover:text-[var(--ink)]" : ""} ${
                          isToday && !isSelected ? "ring-2 ring-inset ring-[var(--accent)]" : ""
                        }`}
                      >
                        {day}
                      </button>,
                    );
                  }
                  return cells;
                })()}
              </div>

              <div className="flex gap-2">
                <button
                  className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                  onClick={() => handleAddRecordDate(0)}
                >
                  今天
                </button>
                <button
                  className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]"
                  onClick={() => handleAddRecordDate(1)}
                >
                  明天
                </button>
                <button
                  className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-rose-600"
                  onClick={() => {
                    setRecordConfigDates([]);
                    setRecordConfigMessage("");
                  }}
                >
                  清空日期
                </button>
              </div>

              <div>
                <div className="text-xs text-[var(--desc-color)]">
                  已选日期（{recordConfigDates.length} 天）
                </div>
                <div className="mt-2 flex min-h-[32px] flex-wrap gap-2">
                  {recordConfigDates.length === 0 ? (
                    <span className="text-xs text-rose-500">
                      尚未选择日期（保存前需至少选 1 天）
                    </span>
                  ) : (
                    recordConfigDates.map((key) => {
                      const [, month, day] = key.split("-");
                      return (
                        <span
                          key={key}
                          className="inline-flex items-center gap-1.5 rounded-full bg-[var(--accent)]/10 px-2.5 py-1 text-xs font-semibold text-[var(--accent)]"
                        >
                          {month}/{day}
                          <button
                            className="opacity-60 hover:opacity-100"
                            onClick={() => handleToggleRecordDate(key)}
                          >
                            ×
                          </button>
                        </span>
                      );
                    })
                  )}
                </div>
              </div>

              <div className="flex items-start justify-between gap-3 rounded-lg border border-black/10 bg-white/70 px-3 py-3">
                <div className="flex items-center gap-1.5">
                  <span className="text-xs font-semibold text-[var(--ink)]">是否继续录制</span>
                  <span
                    className="relative inline-flex"
                    onMouseEnter={() => setRecordConfigHelpOpen(true)}
                    onMouseLeave={() => setRecordConfigHelpOpen(false)}
                  >
                    <span className="flex h-4 w-4 cursor-help items-center justify-center rounded-full bg-black/10 text-[10px] font-bold text-[var(--muted)]">
                      ?
                    </span>
                    {recordConfigHelpOpen ? (
                      <span className="absolute bottom-full left-1/2 z-10 mb-2 w-[260px] -translate-x-1/2 rounded-lg bg-[var(--ink)] px-3 py-2 text-xs leading-relaxed text-white shadow-lg">
                        开启后当最新指定日期录制结束后，开启自动录制功能
                      </span>
                    ) : null}
                  </span>
                </div>
                <button
                  role="switch"
                  aria-checked={recordConfigKeepAfter}
                  className={`relative h-6 w-11 shrink-0 rounded-full transition-colors ${
                    recordConfigKeepAfter ? "bg-[var(--accent)]" : "bg-black/15"
                  }`}
                  onClick={() => setRecordConfigKeepAfter((prev) => !prev)}
                >
                  <span
                    className={`absolute left-0.5 top-0.5 h-5 w-5 rounded-full bg-white shadow transition-transform ${
                      recordConfigKeepAfter ? "translate-x-5" : "translate-x-0"
                    }`}
                  />
                </button>
              </div>
              <div className="text-xs text-[var(--muted)]">
                {recordConfigKeepAfter
                  ? "指定日期结束后自动转为每天录制，无需手动切换模式"
                  : "仅指定日期录制；所有指定日期结束后停止自动录制"}
              </div>
            </div>

            {recordConfigMode === "DAILY" ? (
              <div className="mt-4 rounded-lg border border-black/10 bg-white/60 px-3 py-3 text-xs text-[var(--muted)]">
                每天开播都会自动录制。
                <span className="text-[var(--accent)]">已选的日期列表会保留</span>
                ，切回「仅指定日期」时可直接复用。
              </div>
            ) : null}

            {recordConfigMessage ? (
              <div className="mt-3 text-xs text-rose-600">{recordConfigMessage}</div>
            ) : null}

            <div className="mt-4 flex justify-end gap-2">
              <button className="h-9 rounded-lg px-4" onClick={handleCloseRecordConfig}>
                取消
              </button>
              <button
                className="h-9 rounded-lg bg-[var(--accent)] px-4 font-semibold text-white disabled:opacity-60"
                onClick={handleSaveRecordConfig}
                disabled={recordConfigLoading}
              >
                {recordConfigLoading ? "保存中" : "保存"}
              </button>
            </div>
          </div>
        </div>
      ) : null}
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
