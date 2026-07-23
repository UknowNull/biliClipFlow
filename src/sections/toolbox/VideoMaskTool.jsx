import { useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";
import { invokeCommand } from "../../lib/tauri";

const DEFAULT_SEGMENT_LENGTH = 1;
const TIMELINE_HEADER_HEIGHT = 28;
const TIMELINE_LANE_HEIGHT = 56;
const TIMELINE_LANE_GAP = 8;
const TIMELINE_PADDING_Y = 12;
const TIMELINE_SNAP_PIXELS = 12;
const VIDEO_MASK_DRAFT_STORAGE_KEY = "biliClipFlow.videoMask.lastDraft";
const VIDEO_MASK_RENDER_PROGRESS_EVENT = "toolbox://video-mask-render-progress";

const clamp = (value, min, max) => Math.min(max, Math.max(min, value));

const normalizePath = (path) => String(path || "").replace(/\\/g, "/");

const fileNameFromPath = (path) => {
  const normalized = normalizePath(path);
  return normalized.split("/").filter(Boolean).pop() || "未命名资源";
};

const fileSrcFromPath = (path) => {
  const raw = String(path || "").trim();
  return raw ? convertFileSrc(raw) : "";
};

const resourceImageSrc = (resource) => resource?.imageSrc || fileSrcFromPath(resource?.path);

const segmentImageSrc = (segment) => segment?.imageSrc || fileSrcFromPath(segment?.imagePath);

const logVideoMaskClient = (message) => {
  invokeCommand("auth_client_log", { message: `video_mask:${message}` }).catch(() => {});
};

const buildDefaultTarget = (sourcePath, suffix = "_masked", extension = "mp4") => {
  const normalized = normalizePath(sourcePath);
  if (!normalized) {
    return "";
  }
  const lastSlash = normalized.lastIndexOf("/");
  const dir = lastSlash >= 0 ? normalized.slice(0, lastSlash + 1) : "";
  const base = lastSlash >= 0 ? normalized.slice(lastSlash + 1) : normalized;
  const baseName = base.replace(/\.[^.]+$/, "");
  return `${dir}${baseName}${suffix}.${extension}`;
};

const formatTime = (seconds) => {
  const value = Math.max(0, Number(seconds) || 0);
  const hour = Math.floor(value / 3600);
  const minute = Math.floor((value % 3600) / 60);
  const second = Math.floor(value % 60);
  const fraction = Math.floor((value - Math.floor(value)) * 10);
  return [hour, minute, second]
    .map((item) => String(item).padStart(2, "0"))
    .join(":") + `.${fraction}`;
};

const formatSize = (bytes) => {
  const value = Number(bytes) || 0;
  if (value <= 0) {
    return "-";
  }
  if (value < 1024 * 1024) {
    return `${(value / 1024).toFixed(1)} KB`;
  }
  return `${(value / (1024 * 1024)).toFixed(2)} MB`;
};

const createSegment = (index, startTime, duration, resource = null) => {
  const safeStart = Math.max(0, Number(startTime) || 0);
  const safeDuration = Math.max(0.001, Number(duration) || DEFAULT_SEGMENT_LENGTH);
  const imageName = resource?.name || fileNameFromPath(resource?.path || "");
  return {
    id: `segment-${Date.now()}-${index}`,
    label: imageName || `遮罩 ${index + 1}`,
    enabled: true,
    imagePath: resource?.path || "",
    imageSrc: resourceImageSrc(resource),
    imageName,
    startTime: safeStart,
    endTime: safeStart + safeDuration,
    x: 48,
    y: 48,
    width: 260,
    height: 160,
    cropLeft: 0,
    cropTop: 0,
    cropRight: 0,
    cropBottom: 0,
    opacity: 1,
    trackIndex: 0,
  };
};

const segmentColor = (index, selected) => {
  if (selected) {
    return "var(--primary-color)";
  }
  const palette = ["#0ea5e9", "#22c55e", "#f59e0b", "#a855f7", "#ef4444", "#14b8a6"];
  return palette[index % palette.length];
};

const rangesOverlap = (startA, endA, startB, endB) =>
  Math.max(startA, startB) < Math.min(endA, endB);

const findAvailableTrackIndex = (segments, startTime, endTime) => {
  let trackIndex = 0;
  while (
    segments.some(
      (segment) =>
        segment.enabled !== false &&
        Number(segment.trackIndex || 0) === trackIndex &&
        rangesOverlap(startTime, endTime, Number(segment.startTime) || 0, Number(segment.endTime) || 0),
    )
  ) {
    trackIndex += 1;
  }
  return trackIndex;
};

const percentToTimelineSpan = (duration, minSpan, percent) => {
  if (duration <= 0) {
    return 60;
  }
  if (duration <= minSpan) {
    return duration;
  }
  const ratio = clamp((Number(percent) - 1) / 99, 0, 1);
  return clamp(minSpan * Math.pow(duration / minSpan, ratio), minSpan, duration);
};

export default function VideoMaskTool() {
  const videoRef = useRef(null);
  const previewRef = useRef(null);
  const timelineRef = useRef(null);
  const dragRef = useRef(null);
  const maskDragRef = useRef(null);
  const previewCanvasRefs = useRef(new Map());
  const resourceDragRef = useRef(null);
  const renderIdRef = useRef("");
  const importSeqRef = useRef(0);
  const previewFrameSeqRef = useRef(0);
  const previewFrameTimerRef = useRef(null);
  const [sourcePath, setSourcePath] = useState("");
  const [targetPath, setTargetPath] = useState("");
  const [message, setMessage] = useState("");
  const [sourceInfo, setSourceInfo] = useState({
    duration: 0,
    width: 0,
    height: 0,
    fps: 30,
    videoCodec: "",
    audioStreams: 0,
    subtitleStreams: 0,
    chapterCount: 0,
    colorSpace: "",
    colorTransfer: "",
    colorPrimaries: "",
    keyframes: [],
  });
  const [segments, setSegments] = useState([]);
  const [resources, setResources] = useState([]);
  const [selectedId, setSelectedId] = useState("");
  const [selectedResourceId, setSelectedResourceId] = useState("");
  const [currentTime, setCurrentTime] = useState(0);
  const [timelineStart, setTimelineStart] = useState(0);
  const [timelineZoomPercent, setTimelineZoomPercent] = useState(100);
  const [qualityPercent, setQualityPercent] = useState(90);
  const [playing, setPlaying] = useState(false);
  const [plan, setPlan] = useState(null);
  const [renderResult, setRenderResult] = useState(null);
  const [previewFrameSrc, setPreviewFrameSrc] = useState("");
  const [previewBox, setPreviewBox] = useState({ width: 0, height: 0 });
  const [draggingResource, setDraggingResource] = useState(null);
  const [segmentMenu, setSegmentMenu] = useState(null);
  const [imageLoadErrors, setImageLoadErrors] = useState({});
  const [renderProgress, setRenderProgress] = useState(null);
  const [loadingPreviewFrame, setLoadingPreviewFrame] = useState(false);
  const [videoPreviewReady, setVideoPreviewReady] = useState(false);
  const [videoPreviewFailed, setVideoPreviewFailed] = useState(false);
  const [loadingProbe, setLoadingProbe] = useState(false);
  const [loadingKeyframes, setLoadingKeyframes] = useState(false);
  const [loadingPlan, setLoadingPlan] = useState(false);
  const [rendering, setRendering] = useState(false);

  const duration = Number(sourceInfo.duration) || 0;
  const fps = Number(sourceInfo.fps) > 0 ? Number(sourceInfo.fps) : 30;
  const minSegmentLength = 1 / Math.max(1, fps);
  const width = Number(sourceInfo.width) || 1920;
  const height = Number(sourceInfo.height) || 1080;
  const minTimelinePercent = 1;
  const timelinePercent = clamp(Number(timelineZoomPercent) || 100, minTimelinePercent, 100);
  const minTimelineSpan = duration > 0 ? Math.min(duration, minSegmentLength) : 1;
  const timelineSpan = percentToTimelineSpan(duration, minTimelineSpan, timelinePercent);
  const timelineEnd = timelineStart + timelineSpan;
  const qualityValue = clamp(Number(qualityPercent) || 90, 1, 100);
  const outputCrf = Math.round(30 - ((qualityValue - 1) / 99) * 14);
  const selectedSegment = useMemo(
    () => segments.find((segment) => segment.id === selectedId) || null,
    [segments, selectedId],
  );
  const qualityControlValue = selectedSegment ? Math.round(clamp(Number(selectedSegment.opacity) || 1, 0.01, 1) * 100) : qualityValue;
  const trackCount = useMemo(
    () => Math.max(1, ...segments.map((segment) => Number(segment.trackIndex || 0) + 1)),
    [segments],
  );
  const timelineHeight = TIMELINE_HEADER_HEIGHT + TIMELINE_PADDING_Y * 2 + trackCount * TIMELINE_LANE_HEIGHT + Math.max(0, trackCount - 1) * TIMELINE_LANE_GAP;
  const videoSrc = useMemo(() => (sourcePath ? convertFileSrc(sourcePath) : ""), [sourcePath]);
  const previewSegments = useMemo(
    () =>
      segments.filter((segment) => {
        if (!segment.enabled || !segment.imagePath) {
          return false;
        }
        return currentTime >= segment.startTime && currentTime <= segment.endTime;
      }),
    [segments, currentTime],
  );
  const useDomPreview = Boolean(
    videoPreviewFailed ||
      !videoPreviewReady ||
      (previewSegments.length > 0 && !playing),
  );
  const markDirty = () => {
    setPlan(null);
    setRenderResult(null);
  };

  const focusTimeline = (time) => {
    if (duration <= 0) {
      setTimelineStart(0);
      return;
    }
    const maxStart = Math.max(0, duration - timelineSpan);
    const nextTime = clamp(Number(time) || 0, 0, duration);
    setTimelineStart((prev) => {
      if (nextTime < prev) {
        return clamp(nextTime, 0, maxStart);
      }
      if (nextTime > prev + timelineSpan) {
        return clamp(nextTime - timelineSpan, 0, maxStart);
      }
      return prev;
    });
  };

  const scrollTimeline = (deltaTime) => {
    if (duration <= 0) {
      setTimelineStart(0);
      return;
    }
    setTimelineStart((prev) => clamp(prev + deltaTime, 0, Math.max(0, duration - timelineSpan)));
  };

  const snapToFrame = (time) => {
    if (duration <= 0) {
      return 0;
    }
    const frameTime = Math.round((Number(time) || 0) / minSegmentLength) * minSegmentLength;
    return clamp(frameTime, 0, duration);
  };

  const snapTimelineTime = (time, timelineWidth = 0) => {
    const value = snapToFrame(time);
    if (duration <= 0 || segments.length === 0 || timelineWidth <= 0) {
      return value;
    }
    const threshold = Math.max(minSegmentLength, (timelineSpan / timelineWidth) * TIMELINE_SNAP_PIXELS);
    let snapped = value;
    let minDistance = threshold;
    segments.forEach((segment) => {
      [Number(segment.startTime) || 0, Number(segment.endTime) || 0].forEach((edgeTime) => {
        const distance = Math.abs(value - edgeTime);
        if (distance <= minDistance) {
          minDistance = distance;
          snapped = edgeTime;
        }
      });
    });
    return snapToFrame(snapped);
  };

  const getTimelineSnapThreshold = (timelineWidth = 0) => {
    if (duration <= 0 || timelineWidth <= 0) {
      return minSegmentLength;
    }
    return Math.max(minSegmentLength, (timelineSpan / timelineWidth) * TIMELINE_SNAP_PIXELS);
  };

  const snapSegmentTimeToPlayhead = (time, timelineWidth = 0) => {
    const snapped = snapToFrame(time);
    const threshold = getTimelineSnapThreshold(timelineWidth);
    return Math.abs(snapped - currentTime) <= threshold ? snapToFrame(currentTime) : snapped;
  };

  const snapSegmentMoveToPlayhead = (startTime, length, timelineWidth = 0) => {
    const snappedStart = snapToFrame(startTime);
    const snappedEnd = snapToFrame(snappedStart + length);
    const threshold = getTimelineSnapThreshold(timelineWidth);
    const startDistance = Math.abs(snappedStart - currentTime);
    const endDistance = Math.abs(snappedEnd - currentTime);
    if (startDistance > threshold && endDistance > threshold) {
      return snappedStart;
    }
    if (startDistance <= endDistance) {
      return clamp(snapToFrame(currentTime), 0, Math.max(0, duration - length));
    }
    return clamp(snapToFrame(currentTime - length), 0, Math.max(0, duration - length));
  };

  const updateTimelineZoomPercent = (value) => {
    if (duration <= 0) {
      setTimelineZoomPercent(100);
      setTimelineStart(0);
      return;
    }
    const nextPercent = clamp(Number(value) || 100, minTimelinePercent, 100);
    const nextSpan = percentToTimelineSpan(duration, minTimelineSpan, nextPercent);
    setTimelineZoomPercent(nextPercent);
    setTimelineStart(clamp(currentTime - nextSpan / 2, 0, Math.max(0, duration - nextSpan)));
  };

  const updateQualityControl = (value) => {
    const nextValue = clamp(Number(value) || 1, 1, 100);
    if (selectedSegment) {
      updateSegment(selectedSegment.id, { opacity: nextValue / 100 });
      return;
    }
    setQualityPercent(nextValue);
  };

  useEffect(() => () => {
    if (previewFrameTimerRef.current) {
      clearTimeout(previewFrameTimerRef.current);
    }
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten = null;
    listen(VIDEO_MASK_RENDER_PROGRESS_EVENT, (event) => {
      const progress = event?.payload;
      if (!progress?.renderId || progress.renderId !== renderIdRef.current) {
        return;
      }
      setRenderProgress({
        percent: clamp(Number(progress.percent) || 0, 0, 100),
        stage: String(progress.stage || ""),
        partIndex: Number(progress.partIndex) || 0,
        partCount: Number(progress.partCount) || 0,
        stagePercent: clamp(Number(progress.stagePercent) || 0, 0, 100),
      });
    }).then((dispose) => {
      if (disposed) {
        dispose();
        return;
      }
      unlisten = dispose;
    });
    return () => {
      disposed = true;
      if (unlisten) {
        unlisten();
      }
    };
  }, []);

  useEffect(() => {
    if (duration > 0) {
      setSegments((prev) => {
        if (prev.length > 0) {
          return prev.map((segment) => ({
            ...segment,
            endTime: clamp(segment.endTime, segment.startTime + minSegmentLength, duration),
            startTime: clamp(segment.startTime, 0, Math.max(0, duration - minSegmentLength)),
          }));
        }
        return prev;
      });
    }
  }, [duration, minSegmentLength]);

  useEffect(() => {
    setTimelineStart((prev) => clamp(prev, 0, Math.max(0, duration - timelineSpan)));
  }, [duration, timelineSpan]);

  useEffect(() => {
    if (duration <= 0) {
      setTimelineZoomPercent(100);
      return;
    }
    setTimelineZoomPercent((prev) => clamp(Number(prev) || 100, minTimelinePercent, 100));
  }, [duration]);

  useEffect(() => {
    if (!selectedId && segments.length > 0) {
      setSelectedId(segments[0].id);
    }
  }, [segments, selectedId]);

  useEffect(() => {
    const onMove = (event) => {
      const maskDrag = maskDragRef.current;
      if (maskDrag) {
        const metrics = getPreviewMetrics();
        if (!metrics?.scale) {
          return;
        }
        const deltaX = (event.clientX - maskDrag.startClientX) / metrics.scale;
        const deltaY = (event.clientY - maskDrag.startClientY) / metrics.scale;
        setSegments((prev) =>
          prev.map((segment) => {
            if (segment.id !== maskDrag.id) {
              return segment;
            }
            if (maskDrag.mode === "resize") {
              const nextWidth = clamp(maskDrag.width + deltaX, 1, metrics.sourceWidth - maskDrag.x);
              const nextHeight = clamp(maskDrag.height + deltaY, 1, metrics.sourceHeight - maskDrag.y);
              return {
                ...segment,
                width: nextWidth,
                height: nextHeight,
              };
            }
            return {
              ...segment,
              x: clamp(maskDrag.x + deltaX, 0, Math.max(0, metrics.sourceWidth - maskDrag.width)),
              y: clamp(maskDrag.y + deltaY, 0, Math.max(0, metrics.sourceHeight - maskDrag.height)),
            };
          }),
        );
        markDirty();
        return;
      }

      const drag = dragRef.current;
      if (!drag || !timelineRef.current) {
        return;
      }
      const rect = timelineRef.current.getBoundingClientRect();
      if (!rect.width || duration <= 0) {
        return;
      }
      if (drag.mode === "scrub") {
        const rawTime = timelineStart + ((event.clientX - rect.left) / rect.width) * timelineSpan;
        const nextTime = snapTimelineTime(rawTime, rect.width);
        setCurrentTime(nextTime);
        focusTimeline(nextTime);
        if (videoRef.current && Number.isFinite(nextTime)) {
          videoRef.current.currentTime = nextTime;
        }
        schedulePreviewFrame(nextTime);
        return;
      }
      if (drag.mode === "pan") {
        const delta = ((event.clientX - drag.startClientX) / rect.width) * timelineSpan;
        setTimelineStart(clamp(drag.startTimelineStart - delta, 0, Math.max(0, duration - timelineSpan)));
        return;
      }
      const delta = ((event.clientX - drag.startClientX) / rect.width) * timelineSpan;
      setSegments((prev) =>
        prev.map((segment) => {
          if (segment.id !== drag.id) {
            return segment;
          }
          if (drag.mode === "move") {
            const length = Math.max(minSegmentLength, drag.endTime - drag.startTime);
            const startTime = snapSegmentMoveToPlayhead(
              clamp(drag.startTime + delta, 0, Math.max(0, duration - length)),
              length,
              rect.width,
            );
            return {
              ...segment,
              startTime,
              endTime: startTime + length,
            };
          }
          if (drag.mode === "start") {
            const endTime = Math.max(drag.endTime, drag.startTime + minSegmentLength);
            const startTime = clamp(
              snapSegmentTimeToPlayhead(clamp(drag.startTime + delta, 0, endTime - minSegmentLength), rect.width),
              0,
              endTime - minSegmentLength,
            );
            return {
              ...segment,
              startTime,
            };
          }
          if (drag.mode === "end") {
            const endTime = clamp(
              snapSegmentTimeToPlayhead(clamp(drag.endTime + delta, drag.startTime + minSegmentLength, duration), rect.width),
              drag.startTime + minSegmentLength,
              duration,
            );
            return {
              ...segment,
              endTime,
            };
          }
          return segment;
        }),
      );
      markDirty();
    };
    const onUp = () => {
      dragRef.current = null;
      maskDragRef.current = null;
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onUp);
    return () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onUp);
    };
  }, [duration, height, minSegmentLength, segments, timelineSpan, timelineStart, width]);

  const updateSegment = (id, patch) => {
    setSegments((prev) =>
      prev.map((segment) => {
        if (segment.id !== id) {
          return segment;
        }
        return {
          ...segment,
          ...patch,
        };
      }),
    );
    markDirty();
  };

  const deleteSegment = (id) => {
    setSegments((prev) => prev.filter((segment) => segment.id !== id));
    setImageLoadErrors((prev) => {
      const next = { ...prev };
      delete next[id];
      return next;
    });
    if (selectedId === id) {
      setSelectedId("");
    }
    setSegmentMenu((prev) => (prev?.id === id ? null : prev));
    markDirty();
  };

  const pausePreview = () => {
    const video = videoRef.current;
    if (video && !video.paused) {
      video.pause();
    }
    setPlaying(false);
  };

  const deleteResource = (id) => {
    setResources((prev) => prev.filter((resource) => resource.id !== id));
    if (selectedResourceId === id) {
      setSelectedResourceId("");
    }
  };

  useEffect(() => {
    const onKeyDown = (event) => {
      if (event.key === "Escape") {
        setSegmentMenu(null);
        return;
      }
      if (event.key !== "Delete" && event.key !== "Backspace") {
        return;
      }
      const tagName = String(document.activeElement?.tagName || "").toLowerCase();
      if (tagName === "input" || tagName === "textarea" || tagName === "select" || document.activeElement?.isContentEditable) {
        return;
      }
      if (selectedId) {
        event.preventDefault();
        deleteSegment(selectedId);
        return;
      }
      if (selectedResourceId) {
        event.preventDefault();
        deleteResource(selectedResourceId);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [selectedId, selectedResourceId]);

  const openSegmentMenu = (segment, event) => {
    event.preventDefault();
    event.stopPropagation();
    setSelectedId(segment.id);
    setSelectedResourceId("");
    setSegmentMenu({
      id: segment.id,
      x: event.clientX,
      y: event.clientY,
      label: segment.imageName || segment.label || "遮罩片段",
    });
  };

  const loadPreviewFrame = async (path, time) => {
    const normalizedPath = String(path || "").trim();
    if (!normalizedPath) {
      return;
    }
    const seq = previewFrameSeqRef.current + 1;
    previewFrameSeqRef.current = seq;
    setLoadingPreviewFrame(true);
    logVideoMaskClient(`preview_frame_start time=${(Number(time) || 0).toFixed(3)}`);
    try {
      const previewWidth = clamp(Math.round(Number(sourceInfo.width) || width || 1920), 720, 3840);
      const result = await invokeCommand("toolbox_video_mask_preview_frame", {
        payload: {
          sourcePath: normalizedPath,
          time: Number(time) || 0,
          width: previewWidth,
        },
      });
      if (previewFrameSeqRef.current !== seq) {
        return;
      }
      const nextSrc = result?.dataUrl || fileSrcFromPath(result?.path);
      if (!nextSrc) {
        throw new Error("预览帧返回为空");
      }
      logVideoMaskClient(
        `preview_frame_done time=${(Number(result?.time) || Number(time) || 0).toFixed(3)} dataUrlLen=${String(result?.dataUrl || "").length} path=${result?.path ? 1 : 0} srcLen=${nextSrc.length}`,
      );
      setPreviewFrameSrc(nextSrc);
    } catch (error) {
      if (previewFrameSeqRef.current === seq) {
        logVideoMaskClient(`preview_frame_error time=${(Number(time) || 0).toFixed(3)} message=${error?.message || error}`);
        setMessage(error?.message || "预览帧生成失败");
      }
    } finally {
      if (previewFrameSeqRef.current === seq) {
        setLoadingPreviewFrame(false);
      }
    }
  };

  const schedulePreviewFrame = (time, path = sourcePath, delay = 180) => {
    if (previewFrameTimerRef.current) {
      clearTimeout(previewFrameTimerRef.current);
    }
    const normalizedPath = String(path || "").trim();
    if (!normalizedPath) {
      return;
    }
    if (delay <= 0) {
      loadPreviewFrame(normalizedPath, time);
      return;
    }
    previewFrameTimerRef.current = window.setTimeout(() => {
      loadPreviewFrame(normalizedPath, time);
    }, delay);
  };

  const handlePickVideo = async () => {
    setMessage("");
    const selected = await open({
      multiple: false,
      directory: false,
      title: "选择视频文件",
      filters: [{ name: "视频", extensions: ["mp4", "mov", "mkv", "flv", "webm", "avi"] }],
    });
    if (typeof selected !== "string") {
      return;
    }
    const importSeq = importSeqRef.current + 1;
    importSeqRef.current = importSeq;
    setLoadingProbe(true);
    setLoadingKeyframes(false);
    setLoadingPreviewFrame(false);
    setSourcePath(selected);
    setTargetPath(buildDefaultTarget(selected));
    setRenderResult(null);
    setPlan(null);
    setPreviewFrameSrc("");
    setRenderProgress(null);
    setVideoPreviewReady(false);
    setVideoPreviewFailed(false);
    setCurrentTime(0);
    setTimelineStart(0);
    setTimelineZoomPercent(100);
    setPlaying(false);
    setSegments([]);
    setImageLoadErrors({});
    setSelectedId("");
    setSourceInfo({
      duration: 0,
      width: 0,
      height: 0,
      fps: 30,
      videoCodec: "",
      audioStreams: 0,
      subtitleStreams: 0,
      chapterCount: 0,
      colorSpace: "",
      colorTransfer: "",
      colorPrimaries: "",
      keyframes: [],
    });
    setMessage("视频已导入，正在读取基础信息");
    schedulePreviewFrame(0, selected, 0);
    try {
      const info = await invokeCommand("toolbox_video_mask_probe", {
        payload: { sourcePath: selected },
      });
      if (importSeqRef.current !== importSeq) {
        return;
      }
      const nextInfo = {
        duration: Number(info?.duration) || 0,
        width: Number(info?.width) || 0,
        height: Number(info?.height) || 0,
        fps: Number(info?.fps) || 30,
        videoCodec: String(info?.videoCodec || ""),
        audioStreams: Number(info?.audioStreams) || 0,
        subtitleStreams: Number(info?.subtitleStreams) || 0,
        chapterCount: Number(info?.chapterCount) || 0,
        colorSpace: String(info?.colorSpace || ""),
        colorTransfer: String(info?.colorTransfer || ""),
        colorPrimaries: String(info?.colorPrimaries || ""),
        keyframes: [],
      };
      setSourceInfo((prev) => ({
        ...nextInfo,
        duration: nextInfo.duration || prev.duration,
        width: nextInfo.width || prev.width,
        height: nextInfo.height || prev.height,
      }));
      setLoadingProbe(false);
      setLoadingKeyframes(true);
      setMessage("基础信息已读取，可先拖动预览；后台正在分析关键帧");

      let nextKeyframes = [];
      try {
        const keyframeResult = await invokeCommand("toolbox_video_mask_keyframes", {
          payload: { sourcePath: selected },
        });
        if (importSeqRef.current !== importSeq) {
          return;
        }
        nextKeyframes = Array.isArray(keyframeResult?.keyframes) ? keyframeResult.keyframes : [];
        setSourceInfo((prev) => ({
          ...prev,
          keyframes: nextKeyframes,
        }));
      } catch (error) {
        if (importSeqRef.current !== importSeq) {
          return;
        }
        setMessage(error?.message || "关键帧分析失败，播放预览仍可使用");
        return;
      } finally {
        if (importSeqRef.current === importSeq) {
          setLoadingKeyframes(false);
        }
      }

      if (nextKeyframes.length === 0) {
        setMessage("关键帧分析完成，但未读取到关键帧时间点");
        return;
      }
      setMessage("视频分析完成，可以导入图片资源并拖到时间轴");
    } catch (error) {
      if (importSeqRef.current !== importSeq) {
        return;
      }
      setMessage(error?.message || "读取视频信息失败");
      setSourceInfo((prev) => ({ ...prev, keyframes: [] }));
    } finally {
      if (importSeqRef.current === importSeq) {
        setLoadingProbe(false);
        setLoadingKeyframes(false);
      }
    }
  };

  const handlePickTarget = async () => {
    setMessage("");
    const selected = await save({
      title: "保存输出视频",
      filters: [{ name: "MP4", extensions: ["mp4"] }],
      defaultPath: targetPath || buildDefaultTarget(sourcePath) || undefined,
    });
    if (typeof selected === "string") {
      setTargetPath(normalizePath(selected).toLowerCase().endsWith(".mp4") ? normalizePath(selected) : `${normalizePath(selected)}.mp4`);
    }
  };

  const buildImageResource = async (path) => {
    try {
      const result = await invokeCommand("toolbox_video_mask_image_preview", {
        payload: {
          imagePath: path,
        },
      });
      const previewPath = result?.previewPath || "";
      const imageSrc = fileSrcFromPath(previewPath || path);
      logVideoMaskClient(
        `image_preview_ok name=${fileNameFromPath(path)} previewPath=${previewPath || "-"} imageSrcLen=${imageSrc.length}`,
      );
      return {
        id: `resource-${Date.now()}-${Math.random().toString(16).slice(2)}`,
        path,
        previewPath,
        imageSrc,
        name: fileNameFromPath(path),
        previewError: "",
      };
    } catch (error) {
      const fallbackSrc = fileSrcFromPath(path);
      logVideoMaskClient(
        `image_preview_err name=${fileNameFromPath(path)} err=${error?.message || "unknown"} fallbackSrcLen=${fallbackSrc.length}`,
      );
      return {
        id: `resource-${Date.now()}-${Math.random().toString(16).slice(2)}`,
        path,
        previewPath: "",
        imageSrc: fallbackSrc,
        name: fileNameFromPath(path),
        previewError: error?.message || "遮罩图片预览生成失败",
      };
    }
  };

  const handleImportResources = async () => {
    setMessage("");
    const selected = await open({
      multiple: true,
      directory: false,
      title: "导入遮罩图片资源",
      filters: [{ name: "图片", extensions: ["png", "jpg", "jpeg", "webp"] }],
    });
    const paths = Array.isArray(selected) ? selected : typeof selected === "string" ? [selected] : [];
    if (paths.length === 0) {
      return;
    }
    const imported = await Promise.all(paths.map((path) => buildImageResource(path)));
    setResources((prev) => {
      const exists = new Set(prev.map((item) => item.path));
      const next = [...prev];
      for (const resource of imported) {
        if (!exists.has(resource.path)) {
          next.push(resource);
          exists.add(resource.path);
        }
      }
      return next;
    });
    const failedCount = imported.filter((item) => item.previewError).length;
    setMessage(
      failedCount > 0
        ? `已导入 ${paths.length} 个图片资源，其中 ${failedCount} 个预览地址生成失败，已尝试使用本地路径兜底`
        : `已导入 ${paths.length} 个图片资源，可拖到预览区或时间轴使用`,
    );
  };

  const resourceFromDragEvent = (event) => {
    const raw = event.dataTransfer.getData("application/json") || event.dataTransfer.getData("text/plain");
    if (!raw) {
      return null;
    }
    try {
      const parsed = JSON.parse(raw);
      if (parsed?.path) {
        return parsed;
      }
    } catch {
      // text/plain 兜底走资源库匹配。
    }
    return resources.find((item) => item.id === raw || item.path === raw) || null;
  };

  const addResourceToTimeline = (resource, startValue, patch = {}) => {
    if (!sourcePath || duration <= 0) {
      setMessage("请先导入视频");
      return;
    }
    pausePreview();
    const startTime = snapToFrame(clamp(Number(startValue) || 0, 0, Math.max(0, duration - minSegmentLength)));
    const length = Math.min(DEFAULT_SEGMENT_LENGTH, Math.max(minSegmentLength, duration - startTime));
    const segmentId = `segment-${Date.now()}-${Math.random().toString(16).slice(2)}`;
    setSegments((prev) => {
      const trackIndex = findAvailableTrackIndex(prev, startTime, startTime + length);
      const next = {
        ...createSegment(prev.length, startTime, length, resource),
        ...patch,
        id: segmentId,
        trackIndex,
      };
      return [...prev, next];
    });
    setImageLoadErrors((prev) => {
      const next = { ...prev };
      delete next[segmentId];
      return next;
    });
    logVideoMaskClient(
      `segment_add id=${segmentId} name=${resource?.name || "-"} start=${startTime.toFixed(3)} end=${(startTime + length).toFixed(3)} srcLen=${resourceImageSrc(resource).length}`,
    );
    setSelectedId(segmentId);
    setSelectedResourceId("");
    seekTo(startTime);
    markDirty();
  };

  const addResourceToPreview = (resource, event) => {
    const metrics = getPreviewMetrics();
    const baseWidth = Math.min(260, Math.max(16, width));
    const baseHeight = Math.min(160, Math.max(16, height));
    if (!metrics?.scale) {
      addResourceToTimeline(resource, currentTime);
      return;
    }
    const x = clamp(
      (event.clientX - metrics.rect.left - metrics.offsetX) / metrics.scale - baseWidth / 2,
      0,
      Math.max(0, metrics.sourceWidth - baseWidth),
    );
    const y = clamp(
      (event.clientY - metrics.rect.top - metrics.offsetY) / metrics.scale - baseHeight / 2,
      0,
      Math.max(0, metrics.sourceHeight - baseHeight),
    );
    addResourceToTimeline(resource, currentTime, {
      x,
      y,
      width: baseWidth,
      height: baseHeight,
    });
  };

  const startResourceDrag = (resource, event) => {
    if (event.button !== 0) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    setSelectedResourceId(resource.id);
    setSelectedId("");
    resourceDragRef.current = {
      resource,
      startClientX: event.clientX,
      startClientY: event.clientY,
      clientX: event.clientX,
      clientY: event.clientY,
      active: false,
    };
    setDraggingResource({
      resource,
      x: event.clientX,
      y: event.clientY,
      active: false,
    });
    event.currentTarget.setPointerCapture?.(event.pointerId);
  };

  useEffect(() => {
    const isPointInRect = (event, rect) =>
      Boolean(rect) &&
      event.clientX >= rect.left &&
      event.clientX <= rect.right &&
      event.clientY >= rect.top &&
      event.clientY <= rect.bottom;

    const onMove = (event) => {
      const drag = resourceDragRef.current;
      if (!drag) {
        return;
      }
      const distance = Math.hypot(event.clientX - drag.startClientX, event.clientY - drag.startClientY);
      drag.clientX = event.clientX;
      drag.clientY = event.clientY;
      drag.active = drag.active || distance > 4;
      setDraggingResource({
        resource: drag.resource,
        x: event.clientX,
        y: event.clientY,
        active: drag.active,
      });
    };

    const onUp = (event) => {
      const drag = resourceDragRef.current;
      if (!drag) {
        return;
      }
      resourceDragRef.current = null;
      setDraggingResource(null);
      const moved = drag.active || Math.hypot(event.clientX - drag.startClientX, event.clientY - drag.startClientY) > 4;
      if (!moved) {
        return;
      }
      if (!sourcePath || duration <= 0) {
        setMessage("请先导入视频");
        return;
      }

      const previewRect = previewRef.current?.getBoundingClientRect();
      if (isPointInRect(event, previewRect)) {
        addResourceToPreview(drag.resource, event);
        setMessage(`已添加遮罩：${drag.resource.name}`);
        return;
      }

      const timelineRect = timelineRef.current?.getBoundingClientRect();
      if (isPointInRect(event, timelineRect)) {
        const startTime = timelineStart + ((event.clientX - timelineRect.left) / timelineRect.width) * timelineSpan;
        addResourceToTimeline(drag.resource, startTime);
        setMessage(`已添加遮罩：${drag.resource.name}`);
        return;
      }

      setMessage("未放到预览区或时间轴，未添加遮罩");
    };

    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onUp);
    return () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onUp);
    };
  }, [currentTime, duration, height, minSegmentLength, sourcePath, timelineSpan, timelineStart, width]);

  const seekTo = (time) => {
    const videoDuration = Number(videoRef.current?.duration) || duration || 0;
    const nextTime = clamp(Number(time) || 0, 0, videoDuration || Number.MAX_SAFE_INTEGER);
    setCurrentTime(nextTime);
    focusTimeline(nextTime);
    if (videoRef.current && Number.isFinite(nextTime)) {
      videoRef.current.currentTime = nextTime;
    }
    schedulePreviewFrame(nextTime);
  };

  const togglePlayback = async () => {
    const video = videoRef.current;
    if (!video) {
      return;
    }
    if (video.paused) {
      try {
        await video.play();
      } catch (error) {
        setMessage(error?.message || "视频播放失败");
      }
    } else {
      video.pause();
    }
  };

  const buildPayload = () => ({
    sourcePath,
    targetPath,
    duration: sourceInfo.duration,
    width: sourceInfo.width,
    height: sourceInfo.height,
    fps: sourceInfo.fps,
    videoCodec: sourceInfo.videoCodec,
    colorSpace: sourceInfo.colorSpace,
    colorTransfer: sourceInfo.colorTransfer,
    colorPrimaries: sourceInfo.colorPrimaries,
    keyframes: sourceInfo.keyframes,
    crf: outputCrf,
    preset: "veryfast",
    segments: segments.map((segment) => ({
      id: segment.id,
      imagePath: segment.imagePath,
      startTime: Number(segment.startTime) || 0,
      endTime: Number(segment.endTime) || 0,
      x: Number(segment.x) || 0,
      y: Number(segment.y) || 0,
      width: Number(segment.width) || 0,
      height: Number(segment.height) || 0,
      cropLeft: Number(segment.cropLeft) || 0,
      cropTop: Number(segment.cropTop) || 0,
      cropRight: Number(segment.cropRight) || 0,
      cropBottom: Number(segment.cropBottom) || 0,
      opacity: Number(segment.opacity) || 1,
      enabled: Boolean(segment.enabled),
    })),
    options: {
      preserveMetadata: true,
      preserveSubtitle: true,
      preserveAudioTracks: true,
      preserveChapters: true,
      codecStrategy: "source",
    },
  });

  const serializeResources = () =>
    resources.map((resource) => ({
      id: resource.id,
      path: resource.path,
      name: resource.name,
      previewError: resource.previewError || "",
    }));

  useEffect(() => {
    if (!sourcePath && segments.length === 0 && resources.length === 0) {
      return;
    }
    try {
      localStorage.setItem(
        VIDEO_MASK_DRAFT_STORAGE_KEY,
        JSON.stringify({
          savedAt: new Date().toISOString(),
          payload: buildPayload(),
          resources: serializeResources(),
          qualityPercent,
        }),
      );
    } catch {
      // localStorage 不可用时忽略，避免影响遮罩编辑。
    }
  }, [sourcePath, targetPath, sourceInfo, segments, resources, qualityPercent, outputCrf]);

  const handleBuildPlan = async () => {
    setMessage("");
    if (!sourcePath) {
      setMessage("请先导入视频");
      return;
    }
    setLoadingPlan(true);
    try {
      const nextPlan = await invokeCommand("toolbox_video_mask_build_plan", {
        payload: buildPayload(),
      });
      setPlan(nextPlan);
      const parts = Array.isArray(nextPlan?.parts) ? nextPlan.parts : [];
      const copyParts = parts.filter((part) => part.kind === "copy").length;
      const encodeParts = parts.filter((part) => part.kind === "encode").length;
      setMessage(`计划已生成：${copyParts} 段直拷贝，${encodeParts} 段重编码`);
    } catch (error) {
      setMessage(error?.message || "生成计划失败");
    } finally {
      setLoadingPlan(false);
    }
  };

  const handleRender = async () => {
    setMessage("");
    if (!sourcePath || !targetPath) {
      setMessage("请先选择视频和输出路径");
      return;
    }
    if (segments.some((segment) => segment.enabled && !segment.imagePath)) {
      setMessage("存在未绑定遮罩图片的片段");
      return;
    }
    const renderId = `video-mask-${Date.now()}-${Math.random().toString(16).slice(2)}`;
    renderIdRef.current = renderId;
    setRenderProgress({
      percent: 0,
      stage: "准备导出",
      partIndex: 0,
      partCount: 0,
      stagePercent: 0,
    });
    setRendering(true);
    try {
      const result = await invokeCommand("toolbox_video_mask_render", {
        payload: {
          ...buildPayload(),
          renderId,
        },
      });
      setRenderResult(result);
      setRenderProgress((prev) => ({
        ...(prev || {}),
        percent: 100,
        stage: "完成",
        stagePercent: 100,
      }));
      const warnings = Array.isArray(result?.warnings) && result.warnings.length > 0 ? `；提示：${result.warnings.slice(0, 2).join("；")}` : "";
      setMessage(`导出完成：${result?.outputPath || targetPath}${warnings}`);
    } catch (error) {
      setRenderProgress((prev) => prev ? { ...prev, stage: "导出失败" } : null);
      setMessage(error?.message || "导出失败");
    } finally {
      setRendering(false);
    }
  };

  const previewRatio = width > 0 && height > 0 ? `${width} / ${height}` : "16 / 9";
  const previewMetrics = useMemo(() => {
    if (!previewBox.width || !previewBox.height) {
      return null;
    }
    const sourceWidth = Math.max(1, width);
    const sourceHeight = Math.max(1, height);
    const scale = Math.min(previewBox.width / sourceWidth, previewBox.height / sourceHeight);
    const displayWidth = sourceWidth * scale;
    const displayHeight = sourceHeight * scale;
    return {
      scale,
      sourceWidth,
      sourceHeight,
      left: (previewBox.width - displayWidth) / 2,
      top: (previewBox.height - displayHeight) / 2,
      width: displayWidth,
      height: displayHeight,
    };
  }, [height, previewBox.height, previewBox.width, width]);

  const getPreviewMetrics = () => {
    const rect = previewRef.current?.getBoundingClientRect();
    if (!rect?.width || !rect?.height) {
      return null;
    }
    const sourceWidth = Math.max(1, width);
    const sourceHeight = Math.max(1, height);
    const scale = Math.min(rect.width / sourceWidth, rect.height / sourceHeight);
    const displayWidth = sourceWidth * scale;
    const displayHeight = sourceHeight * scale;
    return {
      rect,
      scale,
      offsetX: (rect.width - displayWidth) / 2,
      offsetY: (rect.height - displayHeight) / 2,
      displayWidth,
      displayHeight,
      sourceWidth,
      sourceHeight,
    };
  };

  useEffect(() => {
    const element = previewRef.current;
    if (!element) {
      return undefined;
    }
    const update = () => {
      const rect = element.getBoundingClientRect();
      setPreviewBox({ width: rect.width, height: rect.height });
    };
    update();
    if (typeof ResizeObserver === "undefined") {
      window.addEventListener("resize", update);
      return () => window.removeEventListener("resize", update);
    }
    const observer = new ResizeObserver(update);
    observer.observe(element);
    window.addEventListener("resize", update);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", update);
    };
  }, [sourcePath]);

  return (
    <div className="space-y-4" onPointerDown={() => setSegmentMenu(null)}>
      {draggingResource ? (
        <div
          className={`pointer-events-none fixed z-[9999] w-32 rounded-lg border border-white/40 bg-black/70 p-2 text-white shadow-2xl transition-opacity ${
            draggingResource.active ? "opacity-95" : "opacity-0"
          }`}
          style={{
            left: draggingResource.x,
            top: draggingResource.y,
            transform: "translate(-50%, -50%)",
          }}
        >
          <div className="aspect-video overflow-hidden rounded bg-black">
            <img
              src={resourceImageSrc(draggingResource.resource)}
              alt=""
              className="h-full w-full object-cover"
              draggable={false}
            />
          </div>
          <div className="mt-1 truncate text-[10px]">{draggingResource.resource.name}</div>
        </div>
      ) : null}

      {segmentMenu ? (
        <div
          className="fixed z-[10000] min-w-32 rounded-lg border border-[var(--split-color)] bg-[var(--card-bg)] p-1 text-sm shadow-2xl"
          style={{ left: segmentMenu.x, top: segmentMenu.y }}
          onPointerDown={(event) => event.stopPropagation()}
        >
          <button
            className="block w-full rounded-md bg-red-500 px-3 py-2 text-left text-white hover:bg-red-600"
            onClick={() => deleteSegment(segmentMenu.id)}
            title={segmentMenu.label}
          >
            删除
          </button>
        </div>
      ) : null}

      <div className="panel p-4 space-y-3">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="space-y-1">
            <div className="text-lg font-semibold">视频遮罩</div>
            <div className="desc">导入长视频和图片资源后，将图片拖到时间轴生成遮罩片段，导出时仅对命中的片段重编码。</div>
          </div>
          <div className="flex flex-wrap gap-2">
            <label className="flex h-8 items-center gap-2 rounded-lg border border-[var(--split-color)] px-3 text-xs text-[var(--desc-color)]">
              <span className="whitespace-nowrap">{selectedSegment ? "遮罩清晰度" : "导出质量"} {Math.round(qualityControlValue)}%</span>
              <input
                className="w-24"
                type="range"
                min="1"
                max="100"
                step="1"
                value={qualityControlValue}
                onInput={(event) => updateQualityControl(event.target.value)}
                onChange={(event) => updateQualityControl(event.target.value)}
              />
            </label>
            <button className="h-8 px-3 rounded-lg" onClick={handlePickVideo} disabled={loadingProbe}>
              {loadingProbe ? "读取中..." : "导入视频"}
            </button>
            <button className="h-8 px-3 rounded-lg" onClick={handleRender} disabled={!sourcePath || !targetPath || rendering}>
              {rendering ? "导出中..." : "合并导出"}
            </button>
          </div>
        </div>
        {message ? <div className="text-xs text-[var(--desc-color)]">{message}</div> : null}
        {renderProgress ? (
          <div className="rounded-lg border border-[var(--split-color)] bg-[var(--solid-button-color)] p-3">
            <div className="mb-2 flex items-center justify-between gap-3 text-xs text-[var(--desc-color)]">
              <span>
                {renderProgress.stage || "导出中"}
                {renderProgress.partCount > 0 ? ` · ${renderProgress.partIndex}/${renderProgress.partCount}` : ""}
              </span>
              <span className="font-semibold text-[var(--ink)]">{Math.round(renderProgress.percent)}%</span>
            </div>
            <div className="h-2 overflow-hidden rounded-full bg-black/15">
              <div
                className="h-full rounded-full bg-[var(--primary-color)] transition-[width] duration-200"
                style={{ width: `${clamp(renderProgress.percent, 0, 100)}%` }}
              />
            </div>
          </div>
        ) : null}
      </div>

      <div className="grid grid-cols-1 xl:grid-cols-[minmax(0,1fr)_320px] gap-4">
        <div className="space-y-4 min-w-0">
          <div className="panel p-4 space-y-3">
            <div className="space-y-3">
              <div className="flex items-center justify-between text-xs text-[var(--desc-color)]">
                <span>播放预览</span>
                <span>{formatTime(currentTime)} / {formatTime(duration)}</span>
              </div>
              <div
                ref={previewRef}
                className="relative isolate overflow-hidden rounded-lg bg-black"
                style={{ aspectRatio: previewRatio, minHeight: "min(64vh, 680px)" }}
                onDragOver={(event) => {
                  if (sourcePath && duration > 0) {
                    event.preventDefault();
                    event.dataTransfer.dropEffect = "copy";
                  }
                }}
                onDrop={(event) => {
                  event.preventDefault();
                  const resource = resourceFromDragEvent(event);
                  if (!resource?.path) {
                    return;
                  }
                  addResourceToPreview(resource, event);
                }}
              >
                {videoSrc ? (
                  <>
                    <video
                      key={videoSrc}
                      ref={videoRef}
                      src={videoSrc}
                      className="absolute inset-0 z-0 h-full w-full bg-black object-contain"
                      style={{ display: useDomPreview ? "none" : "block" }}
                      preload="auto"
                      muted
                      playsInline
                      onLoadedMetadata={(event) => {
                        const video = event.currentTarget;
                        const nextDuration = Number(video.duration) || 0;
                        const nextWidth = Number(video.videoWidth) || 0;
                        const nextHeight = Number(video.videoHeight) || 0;
                        logVideoMaskClient(
                          `video_metadata duration=${nextDuration.toFixed(3)} size=${nextWidth}x${nextHeight}`,
                        );
                        setSourceInfo((prev) => ({
                          ...prev,
                          duration: prev.duration || nextDuration,
                          width: prev.width || nextWidth,
                          height: prev.height || nextHeight,
                        }));
                      }}
                      onLoadedData={(event) => {
                        setVideoPreviewReady(true);
                        setVideoPreviewFailed(false);
                        const nextTime = Number(event.currentTarget.currentTime) || 0;
                        logVideoMaskClient(`video_loaded_data current=${nextTime.toFixed(3)}`);
                        setCurrentTime(nextTime);
                        focusTimeline(nextTime);
                      }}
                      onTimeUpdate={(event) => {
                        const nextTime = Number(event.currentTarget.currentTime) || 0;
                        setCurrentTime(nextTime);
                        focusTimeline(nextTime);
                      }}
                      onSeeked={(event) => {
                        const nextTime = Number(event.currentTarget.currentTime) || 0;
                        setCurrentTime(nextTime);
                        focusTimeline(nextTime);
                      }}
                      onPlay={() => setPlaying(true)}
                      onPause={() => setPlaying(false)}
                      onEnded={() => setPlaying(false)}
                      onError={(event) => {
                        const error = event.currentTarget.error;
                        logVideoMaskClient(
                          `video_error code=${error?.code || 0} message=${error?.message || "unknown"}`,
                        );
                        setVideoPreviewFailed(true);
                        setMessage("当前视频格式无法直接播放，已切换为抽帧预览");
                      }}
                    />
                    {useDomPreview && previewFrameSrc ? (
                      <img
                        src={previewFrameSrc}
                        alt=""
                        className="absolute inset-0 z-10 h-full w-full bg-black object-contain"
                        onLoad={(event) => {
                          logVideoMaskClient(
                            `preview_frame_load srcLen=${previewFrameSrc.length} natural=${event.currentTarget.naturalWidth}x${event.currentTarget.naturalHeight}`,
                          );
                        }}
                        onError={() => {
                          logVideoMaskClient(`preview_frame_img_error srcLen=${previewFrameSrc.length}`);
                          setMessage("视频预览帧加载失败");
                        }}
                      />
                    ) : null}
                    {previewMetrics ? (
                      <div
                        className="pointer-events-none absolute z-20"
                        style={{
                          left: previewMetrics.left,
                          top: previewMetrics.top,
                          width: previewMetrics.width,
                          height: previewMetrics.height,
                          zIndex: 20,
                        }}
                      >
                        {previewSegments.map((segment, index) => {
                          const left = clamp(segment.x, 0, width);
                          const top = clamp(segment.y, 0, height);
                          const boxWidth = clamp(segment.width, 16, width - left);
                          const boxHeight = clamp(segment.height, 16, height - top);
                          const cropLeft = clamp(segment.cropLeft || 0, 0, 0.45);
                          const cropTop = clamp(segment.cropTop || 0, 0, 0.45);
                          const cropRight = clamp(segment.cropRight || 0, 0, 0.45);
                          const cropBottom = clamp(segment.cropBottom || 0, 0, 0.45);
                          const cropWidth = Math.max(0.05, 1 - cropLeft - cropRight);
                          const cropHeight = Math.max(0.05, 1 - cropTop - cropBottom);
                          const selected = segment.id === selectedId;
                          const imageFailed = Boolean(imageLoadErrors[segment.id]);
                          const imageSrc = segmentImageSrc(segment);
                          const imageLayerStyle = {
                            left: `${(-cropLeft / cropWidth) * 100}%`,
                            top: `${(-cropTop / cropHeight) * 100}%`,
                            width: `${100 / cropWidth}%`,
                            height: `${100 / cropHeight}%`,
                            opacity: segment.opacity,
                          };
                          return (
                            <div
                              key={segment.id}
                              className={`absolute overflow-hidden ${selected ? "ring-2 ring-yellow-300" : "ring-1 ring-white/50"} pointer-events-auto`}
                              style={{
                                left: left * previewMetrics.scale,
                                top: top * previewMetrics.scale,
                                width: boxWidth * previewMetrics.scale,
                                height: boxHeight * previewMetrics.scale,
                                zIndex: 30 + index,
                                boxShadow: "0 0 0 1px rgba(0,0,0,.35) inset",
                                transform: "translateZ(0)",
                                willChange: "transform",
                                cursor: selected ? "move" : "pointer",
                              }}
                              onPointerDown={(event) => {
                                event.preventDefault();
                                event.stopPropagation();
                                setSegmentMenu(null);
                                setSelectedId(segment.id);
                                setSelectedResourceId("");
                                if (event.button !== 0) {
                                  return;
                                }
                                maskDragRef.current = {
                                  id: segment.id,
                                  mode: "move",
                                  startClientX: event.clientX,
                                  startClientY: event.clientY,
                                  x: segment.x,
                                  y: segment.y,
                                  width: boxWidth,
                                  height: boxHeight,
                                };
                                event.currentTarget.setPointerCapture(event.pointerId);
                              }}
                              onContextMenu={(event) => openSegmentMenu(segment, event)}
                            >
                              <canvas
                                ref={(node) => {
                                  if (node) {
                                    previewCanvasRefs.current.set(segment.id, node);
                                  } else {
                                    previewCanvasRefs.current.delete(segment.id);
                                  }
                                }}
                                className="absolute inset-0 h-full w-full object-cover select-none"
                                style={imageLayerStyle}
                              />
                              <img
                                src={imageSrc}
                                alt=""
                                className="absolute inset-0 h-full w-full object-cover select-none"
                                draggable={false}
                                onLoad={(event) => {
                                  const imageRect = event.currentTarget.getBoundingClientRect();
                                  const boxRect = event.currentTarget.parentElement?.getBoundingClientRect();
                                  const canvas = previewCanvasRefs.current.get(segment.id);
                                  let canvasPainted = false;
                                  if (canvas) {
                                    try {
                                      canvas.width = Math.max(1, event.currentTarget.naturalWidth);
                                      canvas.height = Math.max(1, event.currentTarget.naturalHeight);
                                      const context = canvas.getContext("2d");
                                      context?.clearRect(0, 0, canvas.width, canvas.height);
                                      context?.drawImage(event.currentTarget, 0, 0);
                                      canvasPainted = Boolean(context);
                                    } catch (error) {
                                      logVideoMaskClient(
                                        `preview_canvas_error id=${segment.id} message=${error?.message || error}`,
                                      );
                                    }
                                  }
                                  event.currentTarget.style.visibility = canvasPainted ? "hidden" : "visible";
                                  logVideoMaskClient(
                                    `preview_img_load id=${segment.id} name=${segment.imageName || segment.label || "-"} srcLen=${imageSrc.length} current=${currentTime.toFixed(3)} range=${Number(segment.startTime || 0).toFixed(3)}-${Number(segment.endTime || 0).toFixed(3)} box=${Math.round(boxRect?.width || 0)}x${Math.round(boxRect?.height || 0)}@${Math.round(boxRect?.left || 0)},${Math.round(boxRect?.top || 0)} img=${Math.round(imageRect.width)}x${Math.round(imageRect.height)} natural=${event.currentTarget.naturalWidth}x${event.currentTarget.naturalHeight} scale=${previewMetrics.scale.toFixed(4)} videoHidden=${useDomPreview ? 1 : 0} canvas=${canvasPainted ? 1 : 0}`,
                                  );
                                  setImageLoadErrors((prev) => {
                                    if (!prev[segment.id]) {
                                      return prev;
                                    }
                                    const next = { ...prev };
                                    delete next[segment.id];
                                    return next;
                                  });
                                }}
                                onError={() => {
                                  logVideoMaskClient(
                                    `preview_img_error id=${segment.id} name=${segment.imageName || segment.label || "-"} srcLen=${imageSrc.length}`,
                                  );
                                  setImageLoadErrors((prev) => ({ ...prev, [segment.id]: true }));
                                  setMessage(`遮罩图片加载失败：${segment.imageName || segment.label || segment.imagePath}`);
                                }}
                                style={imageLayerStyle}
                              />
                              {imageFailed ? (
                                <div className="absolute inset-0 flex items-center justify-center bg-red-600 px-2 text-center text-[10px] font-semibold text-white">
                                  图片加载失败
                                </div>
                              ) : null}
                              {selected ? (
                                <span
                                  className="absolute bottom-0 right-0 h-4 w-4 cursor-nwse-resize border-l border-t border-black/40 bg-yellow-300"
                                  onPointerDown={(event) => {
                                    event.preventDefault();
                                    event.stopPropagation();
                                    maskDragRef.current = {
                                      id: segment.id,
                                      mode: "resize",
                                      startClientX: event.clientX,
                                      startClientY: event.clientY,
                                      x: segment.x,
                                      y: segment.y,
                                      width: boxWidth,
                                      height: boxHeight,
                                    };
                                    event.currentTarget.setPointerCapture(event.pointerId);
                                  }}
                                />
                              ) : null}
                            </div>
                          );
                        })}
                      </div>
                    ) : null}
                    {loadingPreviewFrame ? (
                      <div className="pointer-events-none absolute right-2 top-2 rounded bg-black/65 px-2 py-1 text-[10px] text-white">
                        预览帧生成中
                      </div>
                    ) : null}
                  </>
                ) : (
                  <div className="flex h-full items-center justify-center text-sm text-white/60">导入视频后显示预览</div>
                )}
              </div>
            </div>
          </div>

          <div className="panel p-4 space-y-3">
            <div className="flex flex-wrap items-center justify-between gap-3">
              <div>
                <div className="text-sm font-semibold">时间轴</div>
                <div className="desc">拖动图片片段可移动，左右边缘可改起止时间。</div>
              </div>
              <div className="flex min-w-[260px] flex-wrap items-center justify-end gap-3 text-xs text-[var(--desc-color)]">
                <span>{trackCount} 轨 · {segments.length} 段 · 资源 {resources.length} 个</span>
                <label className="flex items-center gap-2">
                  <span className="whitespace-nowrap">显示范围 {Math.round(timelinePercent)}%</span>
                  <input
                    className="w-28"
                    type="range"
                    min={minTimelinePercent}
                    max="100"
                    step="1"
                    value={timelinePercent}
                    onInput={(event) => updateTimelineZoomPercent(event.target.value)}
                    onChange={(event) => updateTimelineZoomPercent(event.target.value)}
                    disabled={duration <= 0}
                  />
                </label>
              </div>
            </div>
            <div
              ref={timelineRef}
              className="relative rounded-lg border border-[var(--split-color)] bg-[var(--solid-button-color)]"
              style={{ height: timelineHeight }}
              onDragOver={(event) => {
                if (sourcePath && duration > 0) {
                  event.preventDefault();
                  event.dataTransfer.dropEffect = "copy";
                }
              }}
              onDrop={(event) => {
                event.preventDefault();
                if (!timelineRef.current || duration <= 0) {
                  return;
                }
                const resource = resourceFromDragEvent(event);
                if (!resource?.path) {
                  return;
                }
                const rect = timelineRef.current.getBoundingClientRect();
                const startTime = timelineStart + ((event.clientX - rect.left) / rect.width) * timelineSpan;
                addResourceToTimeline(resource, startTime);
              }}
              onWheel={(event) => {
                if (!timelineRef.current || duration <= 0) {
                  return;
                }
                event.preventDefault();
                const rect = timelineRef.current.getBoundingClientRect();
                const wheelDelta = Math.abs(event.deltaX) > Math.abs(event.deltaY) ? event.deltaX : event.deltaY;
                if (event.ctrlKey) {
                  const zoomStep = Math.max(1, Math.min(10, Math.round(Math.abs(wheelDelta) / 20)));
                  updateTimelineZoomPercent(timelinePercent + Math.sign(wheelDelta || 1) * zoomStep);
                  return;
                }
                if (timelineSpan >= duration) {
                  return;
                }
                scrollTimeline((wheelDelta / Math.max(1, rect.width)) * timelineSpan);
              }}
              onPointerDown={(event) => {
                if (!timelineRef.current || duration <= 0) {
                  return;
                }
                if (event.target.closest?.("button")) {
                  return;
                }
                const rect = timelineRef.current.getBoundingClientRect();
                if (event.button === 1) {
                  event.preventDefault();
                  dragRef.current = {
                    mode: "pan",
                    startClientX: event.clientX,
                    startTimelineStart: timelineStart,
                  };
                  event.currentTarget.setPointerCapture(event.pointerId);
                  return;
                }
                if (event.button !== 0) {
                  return;
                }
                const rawTime = timelineStart + ((event.clientX - rect.left) / rect.width) * timelineSpan;
                const nextTime = snapTimelineTime(rawTime, rect.width);
                seekTo(nextTime);
                dragRef.current = {
                  mode: "scrub",
                };
              }}
            >
              <div className="absolute inset-x-3 top-3 flex items-center justify-between text-[10px] text-[var(--desc-color)]">
                <span>{formatTime(timelineStart)}</span>
                <span>{formatTime(Math.min(duration, timelineEnd))}</span>
              </div>
              <div
                className="absolute inset-x-3 rounded-md bg-black/20"
                style={{
                  top: TIMELINE_HEADER_HEIGHT,
                  height: timelineHeight - TIMELINE_HEADER_HEIGHT - TIMELINE_PADDING_Y,
                }}
              >
                {Array.from({ length: trackCount }).map((_, trackIndex) => (
                  <div
                    key={`track-${trackIndex}`}
                    className="absolute inset-x-0 rounded-md border border-white/10 bg-white/5"
                    style={{
                      top: TIMELINE_PADDING_Y + trackIndex * (TIMELINE_LANE_HEIGHT + TIMELINE_LANE_GAP),
                      height: TIMELINE_LANE_HEIGHT,
                    }}
                  />
                ))}
                {duration > 0 ? (
                  <div
                    className="pointer-events-none absolute -top-2 bottom-0 z-30 w-0.5 bg-red-500"
                    style={{ left: `${((clamp(currentTime, timelineStart, timelineEnd) - timelineStart) / Math.max(0.0001, timelineSpan)) * 100}%` }}
                  >
                    <span className="absolute -left-1.5 -top-1 h-3 w-3 rotate-45 bg-red-500" />
                    <span className="absolute left-2 top-0 whitespace-nowrap rounded bg-red-500 px-1.5 py-0.5 text-[10px] leading-none text-white shadow">
                      {formatTime(currentTime)}
                    </span>
                  </div>
                ) : null}
                {segments.map((segment, index) => {
                  if (segment.endTime < timelineStart || segment.startTime > timelineEnd) {
                    return null;
                  }
                  const visibleStart = Math.max(segment.startTime, timelineStart);
                  const visibleEnd = Math.min(segment.endTime, timelineEnd);
                  const left = ((visibleStart - timelineStart) / Math.max(0.0001, timelineSpan)) * 100;
                  const widthPercent = ((visibleEnd - visibleStart) / Math.max(0.0001, timelineSpan)) * 100;
                  const selected = segment.id === selectedId;
                  return (
                    <div
                      key={segment.id}
                      className="absolute h-12 rounded-md text-[11px] text-white shadow-sm"
                      style={{
                        left: `${left}%`,
                        top: TIMELINE_PADDING_Y + Number(segment.trackIndex || 0) * (TIMELINE_LANE_HEIGHT + TIMELINE_LANE_GAP) + 4,
                        width: `${Math.max(widthPercent, 0.3)}%`,
                        backgroundColor: segmentColor(index, selected),
                        opacity: segment.enabled ? 0.92 : 0.35,
                      }}
                      onPointerDown={(event) => {
                        setSelectedId(segment.id);
                        setSelectedResourceId("");
                        setSegmentMenu(null);
                        if (event.button !== 0) {
                          return;
                        }
                        event.stopPropagation();
                        dragRef.current = {
                          id: segment.id,
                          mode: "move",
                          startClientX: event.clientX,
                          startTime: segment.startTime,
                          endTime: segment.endTime,
                        };
                        event.currentTarget.setPointerCapture(event.pointerId);
                      }}
                      onContextMenu={(event) => {
                        openSegmentMenu(segment, event);
                      }}
                    >
                      <div className="relative h-full w-full overflow-hidden rounded-md">
                        <div className="absolute inset-y-0 left-0 w-2 cursor-ew-resize bg-black/20"
                          onPointerDown={(event) => {
                            setSelectedId(segment.id);
                            setSelectedResourceId("");
                            dragRef.current = {
                              id: segment.id,
                              mode: "start",
                              startClientX: event.clientX,
                              startTime: segment.startTime,
                              endTime: segment.endTime,
                            };
                            event.stopPropagation();
                            event.currentTarget.setPointerCapture(event.pointerId);
                          }}
                        />
                        <div className="absolute inset-y-0 right-0 w-2 cursor-ew-resize bg-black/20"
                          onPointerDown={(event) => {
                            setSelectedId(segment.id);
                            setSelectedResourceId("");
                            dragRef.current = {
                              id: segment.id,
                              mode: "end",
                              startClientX: event.clientX,
                              startTime: segment.startTime,
                              endTime: segment.endTime,
                            };
                            event.stopPropagation();
                            event.currentTarget.setPointerCapture(event.pointerId);
                          }}
                        />
                        <button
                          className="flex h-full w-full items-center justify-start gap-2 px-3 text-left"
                          onClick={(event) => {
                            event.stopPropagation();
                            setSelectedId(segment.id);
                            setSelectedResourceId("");
                            seekTo(segment.startTime);
                          }}
                        >
                          {segment.imagePath ? (
                            <img
                              src={segmentImageSrc(segment)}
                              alt=""
                              className="h-8 w-10 shrink-0 rounded-sm object-cover"
                              draggable={false}
                            />
                          ) : null}
                          <span className="min-w-0 truncate">{segment.imageName || segment.label}</span>
                        </button>
                        {selected ? (
                          <button
                            className="absolute right-1 top-1 h-5 w-5 rounded bg-black/55 text-[12px] leading-5 text-white"
                            onClick={(event) => {
                              event.stopPropagation();
                              deleteSegment(segment.id);
                            }}
                            title="删除片段"
                          >
                            ×
                          </button>
                        ) : null}
                      </div>
                    </div>
                  );
                })}
              </div>
            </div>
          </div>
        </div>

        <div className="space-y-4 min-w-0">
          <div className="panel p-4 space-y-3">
            <div className="flex items-center justify-between gap-2">
              <div>
                <div className="text-sm font-semibold">资源</div>
                <div className="desc">拖到预览区定位，拖到时间轴定时，双击加入当前时间。</div>
              </div>
              <button className="h-8 px-3 rounded-lg" onClick={handleImportResources}>
                导入
              </button>
            </div>
            {resources.length > 0 ? (
              <div className="grid grid-cols-2 gap-2">
                {resources.map((resource) => {
                  const selected = resource.id === selectedResourceId;
                  return (
                    <div
                      key={resource.id}
                      className={`cursor-grab select-none rounded-lg border bg-[var(--solid-button-color)] p-2 active:cursor-grabbing ${
                        selected ? "border-[var(--primary-color)] ring-2 ring-[var(--primary-color)]/30" : "border-[var(--split-color)]"
                      }`}
                      draggable={false}
                      onPointerDown={(event) => startResourceDrag(resource, event)}
                      onContextMenu={(event) => {
                        event.preventDefault();
                        event.stopPropagation();
                        deleteResource(resource.id);
                      }}
                      onDoubleClick={() => addResourceToTimeline(resource, currentTime)}
                      title={resource.name}
                      style={{ touchAction: "none" }}
                    >
                      <div className="aspect-video overflow-hidden rounded-md bg-black">
                        <img
                          src={resourceImageSrc(resource)}
                          alt=""
                          className="h-full w-full object-cover"
                          draggable={false}
                        />
                      </div>
                      <div className="mt-2 truncate text-xs text-[var(--desc-color)]">{resource.name}</div>
                    </div>
                  );
                })}
              </div>
            ) : (
              <div className="rounded-lg border border-dashed border-[var(--split-color)] p-4 text-sm text-[var(--desc-color)]">
                暂无图片资源。
              </div>
            )}
          </div>

          <div className="panel p-4 space-y-2">
            <div className="text-sm font-semibold">导出结果</div>
            {renderResult ? (
              <div className="space-y-1 text-xs text-[var(--desc-color)]">
                <div>输出：{renderResult.outputPath}</div>
                <div>大小：{formatSize(renderResult.outputSize)}</div>
                <div>分段：{renderResult.partCount}</div>
                {Array.isArray(renderResult.warnings) && renderResult.warnings.length > 0 ? (
                  <div>提示：{renderResult.warnings.join("；")}</div>
                ) : null}
              </div>
            ) : (
              <div className="text-sm text-[var(--desc-color)]">导出完成后会在这里显示结果。</div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
