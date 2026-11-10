import { useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { confirm as dialogConfirm } from "@tauri-apps/plugin-dialog";
import { invokeCommand } from "../../lib/tauri";
import { SubtitleOverlay, SubtitleStylePanel, useSubtitleStyle } from "./SubtitleStyle";
import { brief, clientLog } from "../../lib/clientLog";
import CoverEditor from "./CoverEditor";

// 字幕上下文：片段前后各显示多少秒
const PRE = 75;
const POST = 60;
const MARK_STATE = { PENDING: "todo", KEEP: "pass", DISCARD: "reject", SUBMITTED: "submitted" };
const STATE_LABEL = { todo: "待评阅", pass: "已通过", reject: "已淘汰", submitted: "已投稿" };
const STATE_CLASS = {
  todo: "bg-[#FFF3E6] text-[#9A4A0C]",
  pass: "bg-[#E8F5EE] text-[#1F7A45]",
  reject: "bg-black/5 text-[var(--muted)]",
  submitted: "bg-[#EAF1FF] text-[#1F4FB8]",
};
// 字幕栏分段配色：上段灰、当前段蓝、下段绿，左侧色条区分
const ZONE_STYLE = {
  before: { label: "上段（片段之前）", divider: "bg-[#EEF0F3] text-[#4A505E]", row: "bg-[#F7F8FA] text-[#5B6170] shadow-[inset_4px_0_0_#B9BEC8]" },
  in: { label: "当前段（切片内容）", divider: "bg-[#DCE6FB] text-[#1F4FB8]", row: "bg-[#EEF3FF] text-[var(--ink)] shadow-[inset_4px_0_0_#1F5BD8]" },
  after: { label: "下段（片段之后）", divider: "bg-[#E1F2E8] text-[#1F7A45]", row: "bg-[#F3FAF6] text-[#5B6170] shadow-[inset_4px_0_0_#5BB585]" },
};
const DIMS = [
  ["funny", "有趣"],
  ["complete", "完整"],
  ["streamer", "主播"],
  ["standalone", "独立可看"],
  ["danmaku", "弹幕"],
];

const pad = (n) => String(n).padStart(2, "0");
const hms = (sec) => {
  const s = Math.max(0, Math.floor(sec || 0));
  return `${pad(Math.floor(s / 3600))}:${pad(Math.floor((s % 3600) / 60))}:${pad(s % 60)}`;
};
const dur = (sec) => {
  const s = Math.round(sec);
  return s >= 60 ? `${Math.floor(s / 60)} 分 ${s % 60} 秒` : `${s} 秒`;
};
const shade = (v) => (v >= 0.75 ? "#1F4FB8" : v >= 0.65 ? "#6F8FD6" : "#B9C6E4");
const norm = (s) => String(s || "").replace(/[^一-龥a-zA-Z0-9]/g, "");
const stateOf = (clip) => MARK_STATE[String(clip.mark || "PENDING").toUpperCase()] || "todo";

// 识别分段可能长达几十秒：按标点和长度切成 SRT 式短句，按字数比例估算每句起止，画面字幕和字幕栏都用它
const CUE_MAX_CHARS = 18;
const PUNCT_TAIL = /[。！？!?；;，,、：:“”"‘’'（）()…—]+$/;
// 按标点切句并保留标点。不用后行断言 (?<=...)：旧版 WebKit 不支持该语法，
// 会让整个模块加载失败、评阅页白屏（打包后在低版本 macOS 上出现过）。
const CUE_SPLIT_PUNCT = "。！？!?；;，,、";
const splitByPunct = (text) => {
  const parts = [];
  let current = "";
  for (const ch of String(text || "")) {
    current += ch;
    if (CUE_SPLIT_PUNCT.includes(ch)) {
      parts.push(current);
      current = "";
    }
  }
  if (current) parts.push(current);
  return parts;
};

const splitParts = (text) =>
  splitByPunct(text)
    .flatMap((p) => {
      const out = [];
      for (let i = 0; i < p.length; i += CUE_MAX_CHARS) out.push(p.slice(i, i + CUE_MAX_CHARS));
      return out;
    })
    .filter((p) => p.trim());
// 人工修订按识别分段保存（seg.key），seg.text 已是修订后的文字，seg.orig 为识别原文（未修订时为空）
function toCues(segments) {
  const cues = [];
  for (const seg of segments) {
    // 改过时间或插入过句子的分段：直接用保存的逐句字幕
    if (seg.cues) {
      seg.cues.forEach((c, part) => {
        cues.push({ key: String(Math.round(c.start * 1000)), start: c.start, end: c.end, text: c.text, raw: c.text, part, seg, orig: seg.orig, edited: true, streamer: seg.streamer });
      });
      continue;
    }
    const parts = splitParts(seg.text);
    const total = parts.reduce((n, p) => n + p.length, 0) || 1;
    let acc = 0;
    parts.forEach((raw, part) => {
      const start = seg.start + ((seg.end - seg.start) * acc) / total;
      acc += raw.length;
      // 字幕习惯不带标点：句末标点去掉，句中残留的标点换成空格
      const text = raw.replace(PUNCT_TAIL, "").replace(/[。！？!?；;，,、：:“”"‘’'（）()…—]+/g, " ").trim();
      if (!text) return;
      const key = String(Math.round(start * 1000));
      const edited = seg.orig != null;
      cues.push({ key, start, end: seg.start + ((seg.end - seg.start) * acc) / total, text, raw, part, seg, orig: seg.orig, edited, streamer: seg.streamer });
    });
  }
  return cues;
}
// 边界输入框：显示到 0.1 秒；可输入 时:分:秒、分:秒 或秒数，回车或失焦生效
const hmsTenth = (sec) => `${hms(sec)}.${Math.floor(((sec || 0) % 1) * 10)}`;
const parseTime = (text) => {
  const parts = String(text).trim().split(":").map(Number);
  if (!parts.length || parts.length > 3 || parts.some((n) => !Number.isFinite(n) || n < 0)) return null;
  return parts.reduce((acc, n) => acc * 60 + n, 0);
};
function TimeInput({ value, onCommit }) {
  const [text, setText] = useState(hmsTenth(value));
  useEffect(() => setText(hmsTenth(value)), [value]);
  const commit = () => {
    const v = parseTime(text);
    if (v == null || Math.abs(v - value) < 0.05) setText(hmsTenth(value));
    else onCommit(Math.round(v * 10) / 10);
  };
  return (
    <input
      className="h-8 w-[92px] rounded-md border border-black/10 bg-white px-2 text-xs tabular-nums"
      value={text}
      onChange={(e) => setText(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
      title="输入时间，如 00:36:12.5、36:12 或 2172"
    />
  );
}
const srtTime = (sec) => `${hms(sec)},${String(Math.floor(((sec || 0) % 1) * 1000)).padStart(3, "0")}`;

export default function ClipReviewView({ anchor, session, onBack, onSubmitClip, onBatchSubmit }) {
  const recordIds = session.task?.recordIds?.length ? session.task.recordIds : session.recordIds;
  const [clips, setClips] = useState([]);
  const [ctx, setCtx] = useState({ heat: [], files: [], segments: [] });
  // 正在修改的字幕 {key, text}
  const [editing, setEditing] = useState(null);
  const [loading, setLoading] = useState(true);
  const [message, setMessage] = useState("");
  const [batching, setBatching] = useState(false);
  const [selId, setSelId] = useState(null);
  const [filter, setFilter] = useState("todo");
  const [sort, setSort] = useState("score");
  const [t, setT] = useState(0);
  const [playing, setPlaying] = useState(false);
  // 预览片段 {path, start, end}：start/end 是它在场次时间轴上的秒数
  const [preview, setPreview] = useState(null);
  const [previewLoading, setPreviewLoading] = useState(false);
  const [videoError, setVideoError] = useState("");
  const [bounds, setBounds] = useState({});
  const [drafts, setDrafts] = useState({});
  const videoRef = useRef(null);
  const pendingSeek = useRef(null);
  const pendingPlay = useRef(false);
  const previewReq = useRef(0);
  const linesRef = useRef(null);
  const [scrubbing, setScrubbing] = useState(false);
  const [queueOpen, setQueueOpen] = useState(false);
  const [sideTab, setSideTab] = useState("subs");
  const [styleOpen, setStyleOpen] = useState(false);
  const [subStyle, setSubStyle] = useSubtitleStyle();
  const frameRef = useRef(null);
  const queueRef = useRef(null);

  const loadClips = async () => {
    const data = await invokeCommand("anchor_clip_list", { roomId: anchor.uid, recordIds });
    const items = data?.items ?? [];
    // 记录返回结构，便于排查打包环境下的数据异常
    void clientLog(`clips_loaded count=${items.length} first=${brief(items[0])}`);
    setClips(items);
    return items;
  };

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    void clientLog(`load_start room=${anchor.uid} recordIds=${brief(recordIds)}`);
    Promise.all([
      loadClips().catch((err) => {
        void clientLog(`clips_load_fail err=${brief(err?.message || err)} stack=${brief(err?.stack || "", 1000)}`);
        throw err;
      }),
      invokeCommand("anchor_clip_session_context", { roomId: anchor.uid, recordIds, includeTranscript: true })
        .then((context) => {
          void clientLog(
            `context_loaded files=${context?.files?.length ?? "N/A"} segments=${context?.segments?.length ?? "N/A"} heat=${context?.heat?.length ?? "N/A"} sample=${brief(context?.segments?.[0])}`,
          );
          return context;
        })
        .catch((err) => {
          void clientLog(`context_load_fail err=${brief(err?.message || err)} stack=${brief(err?.stack || "", 1000)}`);
          setMessage(err?.message || "读取字幕失败");
          return null;
        }),
    ])
      .then(([items, context]) => {
        if (cancelled) return;
        if (context) setCtx(context);
        const first = [...items].sort((a, b) => (b.score || 0) - (a.score || 0)).find((c) => stateOf(c) === "todo") || items[0];
        if (first) {
          void clientLog(`selected first=${brief(first)}`);
          setSelId(first.id);
          setT(first.startOffset);
        } else {
          void clientLog("no_clip_available");
        }
      })
      .catch((err) => {
        void clientLog(`load_fail err=${brief(err?.message || err)} stack=${brief(err?.stack || "", 1200)}`);
        setMessage(err?.message || "读取切片失败");
      })
      .finally(() => {
        if (!cancelled) {
          void clientLog("load_done");
          setLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [anchor.uid, recordIds.join(",")]);

  // 进入渲染前记录一次数据形态：崩溃时能对照日志确认哪项异常
  useEffect(() => {
    if (loading) return;
    void clientLog(
      `render_prepare clips=${clips.length} files=${ctx.files?.length ?? "N/A"} segments=${ctx.segments?.length ?? "N/A"} heat=${ctx.heat?.length ?? "N/A"} selId=${selId ?? "N/A"} ctxKeys=${brief(Object.keys(ctx || {}))} clip=${brief(clips.find((c) => c.id === selId))}`,
    );
  }, [loading, selId]);

  // 有切片在重新剪辑时轮询状态
  const reclipping = clips.some((c) => String(c.status).toUpperCase() === "RUNNING");
  useEffect(() => {
    if (!reclipping) return undefined;
    const timer = setInterval(() => loadClips().catch(() => {}), 2000);
    return () => clearInterval(timer);
  }, [reclipping]);

  const clip = clips.find((c) => c.id === selId) || null;
  const boundsOf = (c) => bounds[c.id] || { s: c.startOffset, e: c.endOffset };
  const draftOf = (c) =>
    drafts[c.id] || { title: c.titles?.[0] || "", cover: c.coverText || "", description: c.description || "", tags: c.tags || "" };
  const cues = useMemo(() => toCues(ctx.segments), [ctx.segments]);
  // 一个识别分段的逐句字幕（含时间）：保存过逐句的直接用，否则按当前切分算出
  const segCuesOf = (seg) => (seg.cues ? seg.cues.map((c) => ({ ...c })) : toCues([seg]).map((c) => ({ start: c.start, end: c.end, text: c.text })));
  const indexIn = (list, cue) => list.findIndex((x) => Math.abs(x.start - cue.start) < 0.001);
  const round2 = (v) => Math.round(v * 100) / 100;
  // 按逐句保存一个分段（列表为空表示这段字幕全删了）。
  // 只发请求，返回更新后的 segment，由调用方决定何时套用到 state
  const persistSegCues = async (seg, list) => {
    const clean = list.filter((c) => c.text.trim() && c.end > c.start).sort((a, b) => a.start - b.start);
    await invokeCommand("anchor_clip_subtitle_edit", { roomId: anchor.uid, recordIds, key: seg.key, text: "", cues: clean });
    const orig = seg.orig ?? seg.text;
    return { key: seg.key, cues: clean, text: clean.map((c) => c.text.trim()).join("，"), orig };
  };
  const applySegCues = (updates) => {
    if (!updates.length) return;
    const byKey = new Map(updates.map((u) => [u.key, u]));
    setCtx((prev) => ({
      ...prev,
      segments: prev.segments.map((s) => {
        const u = byKey.get(s.key);
        return u ? { ...s, cues: u.cues, text: u.text, orig: u.orig } : s;
      }),
    }));
  };
  const saveSegCues = async (seg, list) => {
    applySegCues([await persistSegCues(seg, list)]);
  };
  const deleteCue = async (cue) => {
    if (!(await dialogConfirm(`删除这句字幕？\n${cue.text}`, { title: "删除字幕" }))) return;
    const list = segCuesOf(cue.seg);
    list.splice(indexIn(list, cue), 1);
    try {
      await saveSegCues(cue.seg, list);
      setEditing(null);
    } catch (err) {
      setMessage(err?.message || "删除字幕失败");
    }
  };
  // 改一句的起止：与前后句重叠时把相邻句的边界挪开（相邻句可能在别的分段，一并保存）
  const setCueTime = async (cue, patch) => {
    const start = round2(patch.start ?? cue.start);
    const end = round2(patch.end ?? cue.end);
    if (end - start < 0.2) {
      setMessage("字幕结束时间要晚于开始时间");
      return;
    }
    const i = cues.indexOf(cue);
    const prev = cues[i - 1];
    const next = cues[i + 1];
    if ((prev && start < prev.end && start - prev.start < 0.2) || (next && end > next.start && next.end - end < 0.2)) {
      setMessage("新时间会盖住相邻的整句字幕，请先调整或删除相邻那句");
      return;
    }
    const changed = new Map();
    const listOf = (c) => {
      if (!changed.has(c.seg.key)) changed.set(c.seg.key, { seg: c.seg, list: segCuesOf(c.seg) });
      return changed.get(c.seg.key).list;
    };
    const edit = (c, p) => {
      const list = listOf(c);
      const k = indexIn(list, c);
      list[k] = { ...list[k], ...p };
    };
    if (prev && start < prev.end) edit(prev, { end: start });
    if (next && end > next.start) edit(next, { start: end });
    edit(cue, { start, end });
    try {
      // 先把所有分段存完再一次性更新 state：逐段更新会让面板在中间态闪掉，
      // 用户得点两次才看到效果
      const updates = [];
      for (const { seg, list } of changed.values()) {
        updates.push(await persistSegCues(seg, list));
      }
      applySegCues(updates);
      setMessage("");
      // 起点变了这句的编号也跟着变，保持时间编辑面板开着，方便连续微调
      setEditing({ key: String(Math.round(start * 1000)), mode: "time" });
    } catch (err) {
      setMessage(err?.message || "保存字幕时间失败");
    }
  };
  // 在一句的上方/下方插入：有空档就占空档（最长 2 秒），首尾相接就把这句一分为二
  const startInsert = (cue, where) => {
    const i = cues.indexOf(cue);
    const mid = round2((cue.start + cue.end) / 2);
    let start;
    let end;
    let split = null;
    if (where === "above") {
      const gapStart = cues[i - 1] ? cues[i - 1].end : cue.start - 2;
      if (cue.start - gapStart >= 0.3) [start, end] = [Math.max(gapStart, cue.start - 2), cue.start];
      else [start, end, split] = [cue.start, mid, { start: mid }];
    } else {
      const gapEnd = cues[i + 1] ? cues[i + 1].start : cue.end + 2;
      if (gapEnd - cue.end >= 0.3) [start, end] = [cue.end, Math.min(gapEnd, cue.end + 2)];
      else [start, end, split] = [mid, cue.end, { end: mid }];
    }
    setEditing({ key: cue.key, mode: "insert", where, cue, text: "", start: round2(Math.max(0, start)), end: round2(end), split });
  };
  const commitInsert = async () => {
    const ed = editing;
    if (!ed.text.trim()) {
      setEditing(null);
      return;
    }
    if (ed.end - ed.start < 0.2) {
      setMessage("字幕结束时间要晚于开始时间");
      return;
    }
    const list = segCuesOf(ed.cue.seg);
    if (ed.split) {
      const k = indexIn(list, ed.cue);
      list[k] = { ...list[k], ...ed.split };
    }
    list.push({ start: ed.start, end: ed.end, text: ed.text.trim() });
    try {
      await saveSegCues(ed.cue.seg, list);
      setEditing(null);
    } catch (err) {
      setMessage(err?.message || "保存字幕失败");
    }
  };

  // 投稿带上的字幕：画面上显示的短句和当前样式，投稿工作流第一步压进视频
  const subtitleOf = (c) => {
    const b = boundsOf(c);
    return { cues: cues.filter((x) => x.end > b.s && x.start < b.e).map((x) => ({ start: x.start, end: x.end, text: x.text })), style: subStyle };
  };
  // 已通过的切片进入批量投稿页：带上当前填写的投稿信息和字幕，先保存当前切片
  const batchSubmit = async () => {
    const passed = clips.filter((c) => stateOf(c) === "pass" && c.status === "SUCCESS");
    if (!passed.length || batching) return;
    const records = passed.map((c) => {
      const d = draftOf(c);
      return { ...c, titles: [d.title, ...(c.titles || []).filter((t) => t !== d.title)], description: d.description, tags: d.tags, subtitle: subtitleOf(c) };
    });
    if (!(await saveAll())) return;
    setBatching(true);
    try {
      await onBatchSubmit(records);
    } catch (err) {
      setMessage(err?.message || "打开批量投稿失败");
      setBatching(false);
    }
  };

  // 保存一句字幕的修订：把这句替换进所在识别分段，整段保存（分析时交给大模型的就是这段）；
  // text 为 null 时恢复整段识别原文
  const saveCueEdit = async (cue, text) => {
    if (text != null) {
      if (text.trim() === cue.text) {
        setEditing(null);
        return;
      }
      // 改文字时保持每句的时间不变：整段转成逐句保存，只换这一句的文字（清空即删除这句）。
      // 不再写回整段文字，否则会按字数比例重新分配时间
      const list = segCuesOf(cue.seg);
      const k = indexIn(list, cue);
      list[k] = { ...list[k], text: text.trim() };
      try {
        await saveSegCues(cue.seg, list);
        setEditing(null);
      } catch (err) {
        setMessage(err?.message || "保存字幕失败");
      }
      return;
    }
    // text 为 null：恢复整段识别原文（逐句保存的时间修改也一并清掉）
    const orig = cue.seg.orig ?? cue.seg.text;
    try {
      await invokeCommand("anchor_clip_subtitle_edit", { roomId: anchor.uid, recordIds, key: cue.seg.key, text: "" });
      setCtx((prev) => ({
        ...prev,
        segments: prev.segments.map((s) => (s.key === cue.seg.key ? { ...s, text: orig, orig: null, cues: null } : s)),
      }));
      setEditing(null);
    } catch (err) {
      setMessage(err?.message || "保存字幕失败");
    }
  };
  const visible = useMemo(() => {
    const list = clips.filter((c) => filter === "all" || stateOf(c) === filter);
    return list.sort(sort === "score" ? (a, b) => (b.score || 0) - (a.score || 0) : (a, b) => a.startOffset - b.startOffset);
  }, [clips, filter, sort]);

  // 录播源文件往往很大且索引在末尾，软件内播放器加载不了；按需生成几分钟的预览片段来播放
  const loadPreview = async (from, to, target, play = false) => {
    const req = ++previewReq.current;
    videoRef.current?.pause();
    setPreviewLoading(true);
    setVideoError("");
    try {
      const p = await invokeCommand("anchor_clip_preview", { roomId: anchor.uid, recordIds, start: Math.max(0, from), end: to });
      if (req !== previewReq.current) return;
      pendingSeek.current = Math.max(0, target - p.start);
      pendingPlay.current = play;
      setPreview(p);
    } catch (err) {
      if (req === previewReq.current) setVideoError(err?.message || "生成预览失败");
    } finally {
      if (req === previewReq.current) setPreviewLoading(false);
    }
  };

  const seek = (time, play = false) => {
    setT(time);
    const v = videoRef.current;
    if (preview && time >= preview.start && time < preview.end - 0.5 && v && v.readyState > 0) {
      v.currentTime = time - preview.start;
      if (play) v.play().catch(() => {});
      return;
    }
    // 超出当前预览范围：以目标位置为起点重新生成一段
    loadPreview(time - 30, time + 240, time, play);
  };

  const contextOf = (c) => {
    const b = boundsOf(c);
    return [Math.min(b.s, c.startOffset) - PRE, Math.max(b.e, c.endOffset) + POST];
  };

  const select = (id) => {
    const c = clips.find((x) => x.id === id);
    if (!c) return;
    setSelId(id);
    setT(boundsOf(c).s);
    const [from, to] = contextOf(c);
    loadPreview(from, to, boundsOf(c).s);
  };

  useEffect(() => {
    if (!loading && clip && !preview) select(clip.id);
  }, [loading]);

  // 当前切片的起止，供播放循环判断是否播完（循环里拿不到每次渲染的最新值）
  const clipRange = useRef(null);
  // 播放中用动画帧刷新进度，比 timeupdate 更跟手
  useEffect(() => {
    if (!playing) return undefined;
    let raf;
    let last = null;
    const loop = () => {
      const v = videoRef.current;
      if (v && preview && !scrubbing) {
        const now = preview.start + v.currentTime;
        const r = clipRange.current;
        // 从切片内播到切片终点：回到切片开头并暂停，不继续往后播，也不切到下一条
        if (r && last != null && last < r.e && now >= r.e) {
          v.pause();
          v.currentTime = Math.max(0, r.s - preview.start);
          setT(r.s);
          return;
        }
        last = now;
        // 约每 0.1 秒刷新一次，避免整页以 60 帧重绘
        setT((prev) => (Math.abs(prev - now) >= 0.1 ? now : prev));
      }
      raf = requestAnimationFrame(loop);
    };
    raf = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(raf);
  }, [playing, preview, scrubbing]);

  const togglePlay = () => {
    const v = videoRef.current;
    if (!v || videoError || previewLoading) return;
    if (v.paused) v.play().catch((err) => setVideoError(`无法播放：${err?.message || err}`));
    else v.pause();
  };
  const playFrom = (time) => seek(time, true);

  const setClipBounds = (patch) => {
    if (!clip) return;
    const cur = boundsOf(clip);
    const next = { s: patch.s ?? cur.s, e: patch.e ?? cur.e };
    if (next.e - next.s < 3) {
      setMessage("片段太短，起点要早于终点至少 3 秒");
      return;
    }
    setMessage("");
    setBounds((prev) => ({ ...prev, [clip.id]: next }));
  };

  // 已保存新边界、但切片文件还没按新边界重新剪辑的切片
  const [stale, setStale] = useState({});

  // 保存当前界面的全部改动（边界与投稿信息），只写记录不剪辑
  const saveAll = async (c = clip) => {
    if (!c) return true;
    const b = bounds[c.id];
    if (b) {
      try {
        await invokeCommand("anchor_clip_update_time", { clipId: c.id, startOffset: b.s, endOffset: b.e });
      } catch (err) {
        setMessage(err?.message || "保存边界失败");
        return false;
      }
      setBounds((prev) => {
        const next = { ...prev };
        delete next[c.id];
        return next;
      });
      setStale((prev) => ({ ...prev, [c.id]: true }));
    }
    if (!(await saveMeta(c))) return false;
    if (b) await loadClips().catch(() => {});
    return true;
  };

  const saveMeta = async (c = clip) => {
    if (!c || !drafts[c.id]) return true;
    const d = drafts[c.id];
    try {
      await invokeCommand("anchor_clip_update_meta", { clipId: c.id, title: d.title, coverText: d.cover, description: d.description, tags: d.tags });
      setDrafts((prev) => {
        const next = { ...prev };
        delete next[c.id];
        return next;
      });
      await loadClips();
      return true;
    } catch (err) {
      setMessage(err?.message || "保存投稿信息失败");
      return false;
    }
  };

  const mark = async (value) => {
    if (!clip) return;
    if (!(await saveAll())) return;
    try {
      await invokeCommand("anchor_clip_mark", { clipId: clip.id, mark: value });
    } catch (err) {
      setMessage(err?.message || "标记失败");
      return;
    }
    const updated = clips.map((c) => (c.id === clip.id ? { ...c, mark: value } : c));
    setClips(updated);
    // 标记后跳到下一条待评阅（按综合分）
    const order = [...updated].sort((a, b) => (b.score || 0) - (a.score || 0));
    const idx = order.findIndex((c) => c.id === clip.id);
    const next = order.slice(idx + 1).concat(order.slice(0, idx)).find((c) => stateOf(c) === "todo");
    if (next) select(next.id);
  };

  const step = (dir) => {
    const order = visible.length ? visible : clips;
    const idx = order.findIndex((c) => c.id === selId);
    const next = order[(idx + dir + order.length) % order.length];
    if (next) select(next.id);
  };

  const removeClip = async () => {
    if (!clip) return;
    const confirmed = await dialogConfirm("确认删除该切片文件和记录？", { title: "删除切片" });
    if (!confirmed) return;
    try {
      await invokeCommand("anchor_clip_delete", { clipId: clip.id });
      const items = await loadClips();
      const next = items.find((c) => stateOf(c) === "todo") || items[0];
      if (next) select(next.id);
      else setSelId(null);
    } catch (err) {
      setMessage(err?.message || "删除失败");
    }
  };

  // 快捷键：空格 播放，I / O 设起止，A 通过，X 淘汰，J / K 上下条（输入框内不触发）
  const keyActions = useRef({});
  keyActions.current = {
    " ": togglePlay,
    i: () => setClipBounds({ s: Math.round(t * 10) / 10 }),
    o: () => setClipBounds({ e: Math.round(t * 10) / 10 }),
    a: () => mark("KEEP"),
    x: () => mark("DISCARD"),
    j: () => step(1),
    k: () => step(-1),
    l: () => setQueueOpen((v) => !v),
  };
  useEffect(() => {
    const handler = (ev) => {
      if (ev.target.closest?.("input, textarea, select") || ev.metaKey || ev.ctrlKey || ev.altKey) return;
      const fn = keyActions.current[ev.key.toLowerCase()];
      if (!fn) return;
      ev.preventDefault();
      fn();
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  // 只滚动列表自身，不带动整个页面
  const keepVisible = (box, el, center) => {
    if (!box || !el) return;
    // 列表容器是定位元素，offsetTop 已是相对容器的位置
    const top = el.offsetTop;
    if (top < box.scrollTop || top + el.offsetHeight > box.scrollTop + box.clientHeight) {
      box.scrollTop = center ? top - box.clientHeight / 2 : top - 8;
    }
  };
  // 当前播放的句子保持在字幕栏可见范围内
  useEffect(() => {
    if (playing) keepVisible(linesRef.current, linesRef.current?.querySelector('[data-now="1"]'), true);
  }, [t]);
  // 进入或切换切片时，把切片内第一条字幕（连同"当前段"分隔条）滚到字幕栏最上方
  useEffect(() => {
    const box = linesRef.current;
    const first = box?.querySelector('[data-first="1"]');
    if (box && first) box.scrollTop = first.offsetTop;
  }, [selId, loading, ctx.segments, sideTab]);
  useEffect(() => {
    keepVisible(queueRef.current, queueRef.current?.querySelector(`[data-id="${selId}"]`), false);
  }, [selId, filter, sort, queueOpen]);

  if (loading) {
    return <div className="rounded-2xl bg-white p-8 text-sm text-[var(--muted)] shadow-sm ring-1 ring-black/5">加载切片与字幕中...</div>;
  }

  const sessionSecs = Math.max(1, ...ctx.files.map((f) => f.offsetEnd), ...clips.map((c) => c.endOffset));
  const pct = (s) => `${((100 * s) / sessionSecs).toFixed(3)}%`;
  const heatMax = Math.max(1, ...ctx.heat);
  const sortedHeat = [...ctx.heat].sort((a, b) => a - b);
  const hot = Math.max(1, sortedHeat[Math.floor(sortedHeat.length / 2)] || 1) * 2;
  const counts = {
    todo: clips.filter((c) => stateOf(c) === "todo").length,
    pass: clips.filter((c) => stateOf(c) === "pass").length,
    reject: clips.filter((c) => stateOf(c) === "reject").length,
    submitted: clips.filter((c) => stateOf(c) === "submitted").length,
  };

  if (!clip) {
    return (
      <div className="space-y-4">
        <div className="rounded-2xl bg-white p-8 text-sm text-[var(--muted)] shadow-sm ring-1 ring-black/5">{message || "这一场没有切片"}</div>
        <button className="h-10 rounded-lg border border-black/10 bg-white px-4 text-sm" onClick={onBack}>
          返回录播管理
        </button>
      </div>
    );
  }

  const b = boundsOf(clip);
  const changed = b.s !== clip.startOffset || b.e !== clip.endOffset;
  const cs = Math.max(0, Math.min(b.s, clip.startOffset) - PRE);
  const ce = Math.max(b.e, clip.endOffset) + POST;
  const span = ce - cs;
  const at = (s) => (100 * (Math.min(ce, Math.max(cs, s)) - cs)) / span;
  const lines = cues.filter((s) => s.end > cs && s.start < ce);
  // 正在播放的句子；起点落在无人说话的间隙时，选中切片内的第一句（刚进入、切换切片时即定位到这里）
  const nowLine = lines.find((l) => t >= l.start && t < l.end) || (!playing ? lines.find((l) => l.end > t && l.start < b.e) : null);
  const starts = lines.map((l) => l.start);
  const ends = lines.map((l) => l.end);
  const evidence = (clip.evidence || []).map((e) => norm(e).slice(0, 8)).filter((e) => e.length >= 4);
  const scores = clip.signals?.scores;
  const draft = draftOf(clip);
  const editDraft = (patch) => setDrafts((prev) => ({ ...prev, [clip.id]: { ...draft, ...patch } }));
  const status = String(clip.status || "").toUpperCase();
  const dirty = changed || Boolean(drafts[clip.id]);
  clipRange.current = { s: b.s, e: b.e };
  const inClip = lines.filter((l) => l.end > b.s && l.start < b.e);
  // 勾选字幕：切片始终是一段连续范围，勾选片段外的句子会连同中间的句子一起选入；
  // 取消勾选时从离它更近的一端收缩
  const toggleCue = (l) => {
    if (!(l.end > b.s && l.start < b.e)) {
      setClipBounds(l.start < b.s ? { s: l.start } : { e: l.end });
      return;
    }
    const idx = inClip.indexOf(l);
    if (inClip.length <= 1) {
      setMessage("切片至少要保留一句字幕");
      return;
    }
    if (idx <= inClip.length - 1 - idx) setClipBounds({ s: inClip[idx + 1].start });
    else setClipBounds({ e: inClip[idx - 1].end });
  };
  let zone0 = null;

  return (
    <div className="space-y-3">

      <section className="rounded-2xl bg-white px-5 pb-3 pt-2.5 shadow-sm ring-1 ring-black/5">
        <div className="flex flex-wrap items-center gap-3 text-xs text-[var(--muted)]">
          <span className="font-semibold text-[var(--ink)]">整场时间轴</span>
          <span className="flex items-center gap-1"><i className="inline-block h-2.5 w-2.5 rounded-sm bg-[#9DB5E8]" />每分钟弹幕人数</span>
          <span className="flex items-center gap-1"><i className="inline-block h-2.5 w-2.5 rounded-sm bg-[#E8833A]" />高于中位数 2 倍</span>
          <span className="flex items-center gap-1"><i className="inline-block h-2.5 w-2.5 rounded-sm bg-[#1F4FB8]" />切片（颜色越深分越高）</span>
          <span className="flex items-center gap-1"><i className="inline-block h-2.5 w-2.5 rounded-sm bg-[#F2A900]" />当前切片</span>
        </div>
        <div className="relative mt-1.5 h-[66px]">
          <div className="absolute inset-x-0 top-0 flex h-10 items-end gap-px">
            {ctx.heat.map((v, i) => (
              <span key={i} className="min-w-px flex-1" style={{ height: `${Math.max(2, Math.round((40 * v) / heatMax))}px`, background: v >= hot ? "#E8833A" : "#9DB5E8" }} />
            ))}
          </div>
          <div className="absolute inset-x-0 top-11 h-3 rounded bg-black/5">
            {clips.map((c) => (
              <button
                key={c.id}
                title={`${hms(c.startOffset)} ${c.titles?.[0] || ""}`}
                aria-label={`跳到切片 ${c.titles?.[0] || c.id}`}
                onClick={() => select(c.id)}
                className="absolute rounded-sm"
                style={{
                  left: pct(c.startOffset),
                  width: `max(3px, ${pct(c.endOffset - c.startOffset)})`,
                  top: c.id === clip.id ? -3 : 2,
                  height: c.id === clip.id ? 18 : 8,
                  background: c.id === clip.id ? "#F2A900" : stateOf(c) === "reject" ? "#D5D8DF" : shade(c.score || 0),
                  border: c.id === clip.id ? "2px solid #16181D" : "none",
                }}
              />
            ))}
          </div>
          <div className="absolute inset-x-0 top-[60px] flex justify-between text-[10px] tabular-nums text-[var(--muted)]">
            {[0, 0.2, 0.4, 0.6, 0.8, 1].map((r) => (
              <span key={r}>{hms(sessionSecs * r).slice(0, 5)}</span>
            ))}
          </div>
        </div>
      </section>

      {message ? <div className="rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-800">{message}</div> : null}

      <div className="relative flex items-start gap-3">
        <aside className="flex w-14 flex-none flex-col items-stretch gap-1 overflow-hidden rounded-2xl bg-white py-2 shadow-sm ring-1 ring-black/5">
          <button
            title="展开切片列表（L）"
            aria-label="展开切片列表"
            onClick={() => setQueueOpen((v) => !v)}
            className={`mx-1.5 flex h-12 flex-col items-center justify-center rounded-lg text-[10px] ${queueOpen ? "bg-[#EEF3FF] text-[#1F4FB8]" : "text-[var(--muted)] hover:bg-black/[0.04]"}`}
          >
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true"><path d="M4 6h16M4 12h16M4 18h16" /></svg>
            列表
          </button>
          <div className="flex max-h-[calc(100vh-240px)] flex-col gap-1 overflow-y-auto px-1.5">
            {visible.map((c, i) => {
              const st = stateOf(c);
              return (
                <button
                  key={c.id}
                  title={`${c.titles?.[0] || ""}（${hms(c.startOffset)}）`}
                  onClick={() => select(c.id)}
                  className={`relative flex h-9 flex-none items-center justify-center rounded-md text-[11px] font-semibold tabular-nums ${c.id === clip.id ? "bg-[#F2A900] text-[#16181D]" : "text-white"}`}
                  style={c.id === clip.id ? undefined : { background: st === "reject" ? "#C9CDD6" : shade(c.score || 0) }}
                >
                  {i + 1}
                  {st !== "todo" ? <span className={`absolute right-0.5 top-0.5 h-1.5 w-1.5 rounded-full ${st === "pass" ? "bg-[#2E9D5B]" : "bg-[#B42318]"}`} /> : null}
                </button>
              );
            })}
          </div>
        </aside>
        {queueOpen ? (
          <div className="absolute left-[68px] top-0 z-30 flex max-h-[calc(100vh-200px)] w-[340px] flex-col overflow-hidden rounded-2xl bg-white shadow-2xl ring-1 ring-black/10">
            <div className="flex items-center justify-between border-b border-black/5 px-3 py-2">
              <span className="text-sm font-semibold text-[var(--ink)]">切片列表</span>
              <button className="text-xs text-[var(--muted)] hover:underline" onClick={() => setQueueOpen(false)}>
                收起（L）
              </button>
            </div>
          <div className="space-y-2.5 border-b border-black/5 p-3">
            <div className="flex gap-1 rounded-lg bg-black/[0.04] p-[3px]">
              {[
                ["todo", "待评阅"],
                ["pass", "已通过"],
                ["submitted", "已投稿"],
                ["reject", "已淘汰"],
                ["all", "全部"],
              ].map(([id, label]) => (
                <button
                  key={id}
                  className={`h-[30px] flex-1 whitespace-nowrap rounded-md px-1 text-xs ${filter === id ? "bg-white font-semibold text-[var(--ink)] shadow-sm" : "text-[var(--muted)]"}`}
                  onClick={() => setFilter(id)}
                >
                  {label} {id === "all" ? clips.length : counts[id]}
                </button>
              ))}
            </div>
            {filter === "pass" && counts.pass > 0 && onBatchSubmit ? (
              <button
                className="h-8 w-full rounded-md bg-[#1F7A45] text-xs font-semibold text-white disabled:opacity-50"
                disabled={batching}
                onClick={batchSubmit}
              >
                {batching ? "正在加载投稿设置…" : `批量投稿已通过的 ${counts.pass} 条…`}
              </button>
            ) : null}
            <div className="flex items-center gap-2 text-xs text-[var(--muted)]">
              排序
              {[
                ["score", "综合分"],
                ["time", "时间"],
              ].map(([id, label]) => (
                <button
                  key={id}
                  className={`h-[26px] rounded-md border px-2.5 ${sort === id ? "border-[#1F5BD8] bg-[#EEF3FF] text-[#1F4FB8]" : "border-black/10 bg-white"}`}
                  onClick={() => setSort(id)}
                >
                  {label}
                </button>
              ))}
            </div>
          </div>
          <div ref={queueRef} className="relative max-h-[calc(100vh-260px)] overflow-y-auto">
            {visible.length === 0 ? <div className="p-6 text-center text-xs text-[var(--muted)]">这一组没有切片</div> : null}
            {visible.map((c) => {
              const st = stateOf(c);
              const sc = c.signals?.scores;
              return (
                <button
                  key={c.id}
                  data-id={c.id}
                  onClick={() => select(c.id)}
                  className={`flex w-full flex-col gap-2 border-b border-black/5 p-3 text-left ${c.id === clip.id ? "bg-[#EEF3FF] shadow-[inset_3px_0_0_#1F5BD8]" : "bg-white hover:bg-black/[0.02]"}`}
                >
                  <span className="flex w-full gap-2.5">
                    <span className="flex min-w-0 flex-1 flex-col gap-1">
                      <span className="text-sm font-semibold leading-snug text-[var(--ink)]">{c.titles?.[0] || c.filePath.split(/[\\/]/).pop()}</span>
                      <span className="text-xs tabular-nums text-[var(--muted)]">
                        {hms(c.startOffset)} – {hms(c.endOffset)} · {dur(c.endOffset - c.startOffset)}
                      </span>
                    </span>
                    <span className="flex flex-none flex-col items-end gap-1">
                      <span className="text-xl font-bold tabular-nums text-[#1F4FB8]">{c.signals?.grade ? `${c.signals.grade} ` : ""}{sc ? Number(sc.total).toFixed(1) : (c.score || 0).toFixed(2)}</span>
                      <span className={`rounded px-1.5 py-px text-[11px] ${STATE_CLASS[st]}`}>{STATE_LABEL[st]}</span>
                    </span>
                  </span>
                  {sc ? (
                    <span className="grid w-full grid-cols-5 gap-1">
                      {DIMS.map(([k, label]) => (
                        <span key={k} className="flex flex-col gap-0.5 text-[10px] text-[var(--muted)]">
                          {label.slice(0, 2)} {sc[k] ?? "-"}
                          <span className="h-1 overflow-hidden rounded-full bg-black/5">
                            <span className="block h-full" style={{ width: `${(sc[k] || 0) * 10}%`, background: k === "danmaku" ? "#E8833A" : "#1F5BD8" }} />
                          </span>
                        </span>
                      ))}
                    </span>
                  ) : null}
                </button>
              );
            })}
          </div>
          </div>
        ) : null}

        <main className="flex min-w-0 flex-[999_1_520px] flex-col gap-3">
          <div ref={frameRef} className="relative flex aspect-video w-full flex-col justify-between overflow-hidden rounded-2xl bg-[#111318] text-[#C9CDD6]">
            {preview && !videoError ? (
              <video
                ref={videoRef}
                src={convertFileSrc(preview.path)}
                className="absolute inset-0 h-full w-full object-contain"
                preload="auto"
                onLoadedMetadata={() => {
                  if (pendingSeek.current != null && videoRef.current) {
                    videoRef.current.currentTime = pendingSeek.current;
                    pendingSeek.current = null;
                  }
                  if (pendingPlay.current) {
                    pendingPlay.current = false;
                    videoRef.current?.play().catch(() => {});
                  }
                }}
                onPlay={() => setPlaying(true)}
                onEnded={() => {
                  // 预览片段播完（从片段之后开始播的情况）：同样回到切片开头暂停
                  const r = clipRange.current;
                  if (r && videoRef.current) {
                    videoRef.current.currentTime = Math.max(0, r.s - preview.start);
                    setT(r.s);
                  }
                }}
                onPause={() => setPlaying(false)}
                onError={async () => {
                  // 记下浏览器给出的错误码和资源请求结果，便于定位是文件、协议还是格式问题
                  const err = videoRef.current?.error;
                  const src = convertFileSrc(preview.path);
                  let probe = "";
                  try {
                    const r = await fetch(src, { headers: { Range: "bytes=0-1" } });
                    probe = `status=${r.status} type=${r.headers.get("content-type")} range=${r.headers.get("content-range")}`;
                  } catch (e) {
                    probe = `fetch_error=${e?.message || e}`;
                  }
                  const detail = `code=${err?.code} msg=${err?.message || ""} ${probe}`;
                  invokeCommand("auth_client_log", { message: `clip_review_video_error path=${preview.path} src=${src} ${detail}` }).catch(() => {});
                  setVideoError(`录播文件无法播放（${detail}）`);
                }}
                onClick={togglePlay}
              />
            ) : null}
            <div className="relative flex items-start justify-between p-3 text-xs">
              <span className="pointer-events-none">{videoError || (previewLoading ? "正在生成预览..." : "")}</span>
              <span className="flex items-center gap-1">
                <button
                  aria-label="字号减小"
                  title="字号减小"
                  className="h-7 rounded-md bg-black/50 px-2 text-xs text-white hover:bg-black/70"
                  onClick={() => setSubStyle({ fontSize: Math.max(12, subStyle.fontSize - 4) })}
                >
                  A−
                </button>
                <span className="w-9 text-center tabular-nums text-white/80">{subStyle.fontSize}</span>
                <button
                  aria-label="字号增大"
                  title="字号增大"
                  className="h-7 rounded-md bg-black/50 px-2 text-sm text-white hover:bg-black/70"
                  onClick={() => setSubStyle({ fontSize: Math.min(200, subStyle.fontSize + 4) })}
                >
                  A+
                </button>
                <button
                  className={`ml-1 h-7 rounded-md px-2.5 text-xs ${styleOpen ? "bg-white text-[var(--ink)]" : "bg-black/50 text-white hover:bg-black/70"}`}
                  onClick={() => setStyleOpen((v) => !v)}
                >
                  字幕样式
                </button>
              </span>
            </div>
            {styleOpen ? <SubtitleStylePanel style={subStyle} onChange={setSubStyle} onClose={() => setStyleOpen(false)} /> : null}
            <SubtitleOverlay text={nowLine?.text} style={subStyle} onChange={setSubStyle} frameRef={frameRef} />
            {preview && !videoError && !previewLoading && !playing ? (
              <button
                aria-label="播放"
                onClick={togglePlay}
                className="absolute left-1/2 top-1/2 flex h-16 w-16 -translate-x-1/2 -translate-y-1/2 items-center justify-center rounded-full bg-black/55 text-white hover:bg-black/70"
              >
                <svg width="28" height="28" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M8 5v14l11-7z" /></svg>
              </button>
            ) : null}
            <div className="relative">
              <div className="flex items-center gap-3 bg-gradient-to-t from-black/80 to-black/0 px-3 pb-2.5 pt-4 text-xs text-white">
                <button
                  aria-label={playing ? "暂停" : "播放"}
                  disabled={!preview || !!videoError || previewLoading}
                  onClick={togglePlay}
                  className="flex h-8 w-8 flex-none items-center justify-center rounded-full hover:bg-white/15 disabled:opacity-40"
                >
                  {playing ? (
                    <svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><rect x="6" y="5" width="4" height="14" rx="1" /><rect x="14" y="5" width="4" height="14" rx="1" /></svg>
                  ) : (
                    <svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M8 5v14l11-7z" /></svg>
                  )}
                </button>
                <span className="flex-none tabular-nums">{hms(t)}</span>
                <span className="relative flex h-5 flex-1 items-center">
                  {preview ? (
                    <>
                      <span className="absolute inset-x-0 h-1 rounded-full bg-white/25" />
                      <span
                        className="absolute h-1 rounded-full bg-[#1F5BD8]/70"
                        style={{
                          left: `${Math.max(0, ((b.s - preview.start) / (preview.end - preview.start)) * 100)}%`,
                          width: `${Math.max(0, ((Math.min(b.e, preview.end) - Math.max(b.s, preview.start)) / (preview.end - preview.start)) * 100)}%`,
                        }}
                      />
                      <input
                        type="range"
                        aria-label="播放进度"
                        min={preview.start}
                        max={preview.end}
                        step={0.1}
                        value={Math.min(preview.end, Math.max(preview.start, t))}
                        onPointerDown={() => setScrubbing(true)}
                        onPointerUp={() => setScrubbing(false)}
                        onChange={(e) => {
                          const v = Number(e.target.value);
                          setT(v);
                          if (videoRef.current) videoRef.current.currentTime = v - preview.start;
                        }}
                        className="relative h-5 w-full cursor-pointer appearance-none bg-transparent [&::-webkit-slider-thumb]:h-3.5 [&::-webkit-slider-thumb]:w-3.5 [&::-webkit-slider-thumb]:appearance-none [&::-webkit-slider-thumb]:rounded-full [&::-webkit-slider-thumb]:bg-[#F2A900] [&::-webkit-slider-thumb]:shadow"
                      />
                    </>
                  ) : null}
                </span>
                <span className="flex-none tabular-nums text-white/70">{preview ? hms(preview.end) : ""}</span>
              </div>
            </div>
          </div>

          <div className="flex flex-col gap-2.5 rounded-2xl bg-white px-4 py-3 shadow-sm ring-1 ring-black/5">
            <div className="flex flex-wrap items-center gap-2">
              <button className="h-9 rounded-lg bg-[#16181D] px-3.5 text-sm text-white disabled:opacity-40" disabled={!!videoError} onClick={togglePlay}>
                {playing ? "暂停" : "播放"}
              </button>
              <button className="h-9 rounded-lg border border-black/10 bg-white px-2.5 text-sm" onClick={() => seek(Math.max(cs, t - 5))}>−5 秒</button>
              <button className="h-9 rounded-lg border border-black/10 bg-white px-2.5 text-sm" onClick={() => seek(Math.min(ce, t + 5))}>+5 秒</button>
              <button className="h-9 rounded-lg border border-black/10 bg-white px-2.5 text-sm" onClick={() => playFrom(b.s)}>从起点播放</button>
              <button className="h-9 rounded-lg border border-black/10 bg-white px-2.5 text-sm" onClick={() => playFrom(Math.max(b.s, b.e - 8))}>听结尾 8 秒</button>
              <span className="flex-1" />
              <span className="text-xs text-[var(--muted)]">
                片段 <b className="tabular-nums text-[var(--ink)]">{hms(b.s)} – {hms(b.e)}</b>（{dur(b.e - b.s)}）
              </span>
              {changed ? <span className="rounded-md bg-[#FFF3E6] px-2 py-0.5 text-xs text-[#9A4A0C]">已调整，原始 {hms(clip.startOffset)} – {hms(clip.endOffset)}</span> : null}
            </div>
            <button
              aria-label="点击定位播放位置"
              className="relative block h-[58px] w-full rounded-lg bg-black/[0.04]"
              onClick={(ev) => {
                const r = ev.currentTarget.getBoundingClientRect();
                seek(cs + Math.min(1, Math.max(0, (ev.clientX - r.left) / r.width)) * span);
              }}
            >
              {lines.map((l, i) => (
                <span key={i} className="absolute top-2 h-2.5 w-px" style={{ left: `${at(l.start)}%`, background: l.streamer ? "#5B6170" : "#B9BEC8" }} />
              ))}
              <span className="absolute top-5 h-[26px] rounded border border-dashed border-[#8A93A6]" style={{ left: `${at(clip.startOffset)}%`, width: `${at(clip.endOffset) - at(clip.startOffset)}%` }} />
              <span className="absolute top-[22px] h-[22px] rounded-sm border-x-[3px] border-[#1F5BD8] bg-[#1F5BD8]/20" style={{ left: `${at(b.s)}%`, width: `${at(b.e) - at(b.s)}%` }} />
              <span className="absolute bottom-1 top-1 w-0.5 bg-[#F2A900]" style={{ left: `${at(t)}%` }} />
              <span className="absolute bottom-0.5 left-1.5 text-[10px] tabular-nums text-[var(--muted)]">{hms(cs)}</span>
              <span className="absolute bottom-0.5 right-1.5 text-[10px] tabular-nums text-[var(--muted)]">{hms(ce)}</span>
            </button>
            <div className="grid grid-cols-1 gap-2.5 md:grid-cols-2">
              {[
                ["起点", b.s, "s", starts],
                ["终点", b.e, "e", ends],
              ].map(([label, value, key, list]) => (
                <div key={key} className="flex flex-wrap items-center gap-1.5 rounded-lg bg-black/[0.04] px-2.5 py-2">
                  <span className="w-full text-xs text-[var(--muted)]">
                    {label} <b className="tabular-nums text-[var(--ink)]">{hms(value)}</b>
                  </span>
                  <TimeInput value={value} onCommit={(v) => setClipBounds({ [key]: v })} />
                  {[-1, 1].map((d) => (
                    <button key={d} className="h-8 rounded-md border border-black/10 bg-white px-2 text-xs tabular-nums" onClick={() => setClipBounds({ [key]: Math.max(0, Math.round((value + d) * 10) / 10) })}>
                      {d > 0 ? "+1 秒" : "−1 秒"}
                    </button>
                  ))}
                  <button
                    className="h-8 rounded-md border border-black/10 bg-white px-2.5 text-xs"
                    onClick={() => {
                      const v = list.filter((x) => x < value - 0.05).pop();
                      if (v != null) setClipBounds({ [key]: v });
                    }}
                  >
                    ← 前一句
                  </button>
                  <button
                    className="h-8 rounded-md border border-black/10 bg-white px-2.5 text-xs"
                    onClick={() => {
                      const v = list.find((x) => x > value + 0.05);
                      if (v != null) setClipBounds({ [key]: v });
                    }}
                  >
                    后一句 →
                  </button>
                  <button className="h-8 rounded-md border border-black/10 bg-white px-2.5 text-xs" onClick={() => setClipBounds({ [key]: Math.round(t * 10) / 10 })}>
                    设为播放位置（{key === "s" ? "I" : "O"}）
                  </button>
                </div>
              ))}
            </div>
            <div className="flex flex-wrap items-center gap-2">
              <button
                className="h-[34px] rounded-lg border border-black/10 bg-white px-3 text-sm disabled:opacity-40"
                disabled={!changed}
                onClick={() =>
                  setBounds((prev) => {
                    const next = { ...prev };
                    delete next[clip.id];
                    return next;
                  })
                }
              >
                还原边界
              </button>
              <button className="h-[34px] rounded-lg bg-[#1F5BD8] px-4 text-sm font-semibold text-white disabled:opacity-40" disabled={!dirty} onClick={() => saveAll()}>
                保存
              </button>
              <span className={`text-xs ${dirty || stale[clip.id] ? "text-[#9A4A0C]" : "text-[var(--muted)]"}`}>
                {dirty
                  ? "有未保存的改动"
                  : stale[clip.id]
                    ? "边界已保存，投稿时会按新边界重新生成"
                    : status === "FAILED"
                      ? "上次剪辑失败"
                      : "投稿时按当前边界重新生成"}
              </span>
              <span className="flex-1" />
              <button
                className="h-[34px] rounded-lg border border-black/10 bg-white px-3 text-sm"
                onClick={() => invokeCommand("anchor_open_record_file", { filePath: clip.filePath }).catch((err) => setMessage(err?.message || "打开失败"))}
              >
                打开切片
              </button>
              <button className="h-[34px] rounded-lg border border-red-200 bg-white px-3 text-sm text-red-600" onClick={removeClip}>
                删除
              </button>
            </div>
          </div>

        </main>

        <aside className="flex min-w-0 max-w-[400px] flex-[1_1_320px] flex-col overflow-hidden rounded-2xl bg-white shadow-sm ring-1 ring-black/5">
          <div className="flex gap-1 border-b border-black/5 p-2">
            {[
              ["subs", "字幕"],
              ["meta", "投稿信息"],
              ["cover", "封面"],
              ["score", "评分"],
            ].map(([id, label]) => (
              <button
                key={id}
                onClick={() => setSideTab(id)}
                className={`h-8 flex-1 rounded-lg text-sm ${sideTab === id ? "bg-[#EEF3FF] font-semibold text-[#1F4FB8]" : "text-[var(--muted)] hover:bg-black/[0.03]"}`}
              >
                {label}
                {id === "meta" && drafts[clip.id] ? " •" : ""}
              </button>
            ))}
          </div>
          {sideTab === "subs" ? (
          <>
          <div className="flex flex-col gap-1 border-b border-black/5 px-3.5 py-3">
            <div className="flex items-center gap-2">
              <span className="text-[15px] font-semibold text-[var(--ink)]">字幕上下文</span>
              <span className="text-xs text-[var(--muted)]">
                片段前 {PRE} 秒 · 片段 · 片段后 {POST} 秒
              </span>
            </div>
            <span className="text-xs text-[var(--muted)]">勾选的字幕就是切片内容：勾选片段外的句子会连同中间的句子一起加入；点文字视频跳到该句</span>
          </div>
          <div ref={linesRef} className="relative max-h-[calc(100vh-260px)] overflow-y-auto">
            {lines.length === 0 ? <div className="p-6 text-center text-xs text-[var(--muted)]">没有字幕（这一场可能是按信号切的，或识别缓存已被清理）</div> : null}
            {lines.map((l, i) => {
              const zone = l.end <= b.s ? "before" : l.start >= b.e ? "after" : "in";
              const divider = zone !== zone0;
              zone0 = zone;
              const now = nowLine === l;
              const isEvidence = zone === "in" && evidence.some((e) => norm(l.text).includes(e));
              return (
                <div key={i} data-first={divider && zone === "in" ? "1" : "0"}>
                  {divider ? (
                    <div className={`px-3.5 py-1.5 text-[11px] font-semibold ${ZONE_STYLE[zone].divider}`}>
                      {ZONE_STYLE[zone].label}
                    </div>
                  ) : null}
                  <div
                    data-now={now ? "1" : "0"}
                    className={`group flex flex-col gap-1 border-b border-black/[0.04] px-3.5 py-2 ${now ? "bg-[#FFF6DB] text-[var(--ink)] shadow-[inset_4px_0_0_#F2A900]" : ZONE_STYLE[zone].row}`}
                  >
                    <span className="flex w-full items-start gap-2">
                    <input
                      type="checkbox"
                      aria-label={zone === "in" ? "从切片中移除这句" : "把这句加入切片"}
                      title={zone === "in" ? "取消勾选：从切片中移除这句（从较近的一端收缩）" : "勾选：把这句加入切片，中间的句子一并加入"}
                      checked={zone === "in"}
                      onChange={() => toggleCue(l)}
                      className="mt-1 h-4 w-4 flex-none cursor-pointer accent-[#1F5BD8]"
                    />
                    <button
                      className="flex min-w-0 flex-1 items-start gap-2 text-left text-[13px] leading-relaxed"
                      title="单击跳到这句，双击修改字幕"
                      onClick={() => seek(l.start)}
                      onDoubleClick={() => setEditing({ key: l.key, text: l.text })}
                    >
                      <span className="w-[78px] flex-none pt-0.5 text-[10px] leading-tight tabular-nums text-[var(--muted)]">
                        {srtTime(l.start)}
                        <br />
                        {srtTime(l.end)}
                      </span>
                      <span className={`mt-0.5 flex-none rounded px-1.5 py-px text-[11px] ${l.streamer ? "bg-[#EAF1FF] text-[#1F4FB8]" : "bg-black/5 text-[var(--muted)]"}`}>
                        {l.streamer ? "主播" : "原片"}
                      </span>
                      <span className="min-w-0 flex-1">
                        {editing?.key === l.key && !editing.mode ? <span className="text-[var(--muted)]">（修改中）</span> : l.text}
                        {l.edited ? <span className="ml-1.5 rounded bg-[#EAF1FF] px-1.5 py-px text-[11px] text-[#1F4FB8]" title={`识别原文：${l.orig}`}>已改</span> : null}
                        {isEvidence ? <span className="ml-1.5 rounded bg-[#FFF3E6] px-1.5 py-px text-[11px] text-[#9A4A0C]">证据</span> : null}
                      </span>
                    </button>
                    </span>
                    {editing?.key === l.key && editing.mode === "time" ? (
                      <div className="flex flex-wrap items-center gap-1.5 pl-6 text-[11px] text-[var(--muted)]">
                        {[
                          ["开始", "start"],
                          ["结束", "end"],
                        ].map(([label, k]) => (
                          <span key={k} className="flex items-center gap-1">
                            {label}
                            <TimeInput value={l[k]} onCommit={(v) => setCueTime(l, { [k]: v })} />
                            {[-0.1, 0.1].map((d) => (
                              <button key={d} className="h-[26px] rounded-md border border-black/10 bg-white px-1.5 text-[var(--ink)]" onClick={() => setCueTime(l, { [k]: l[k] + d })}>
                                {d > 0 ? "+0.1" : "−0.1"}
                              </button>
                            ))}
                          </span>
                        ))}
                        <button className="h-[26px] rounded-md border border-black/10 bg-white px-2.5 text-[var(--ink)]" onClick={() => setEditing(null)}>
                          完成
                        </button>
                      </div>
                    ) : null}
                    {editing?.key === l.key && editing.mode === "insert" ? (
                      <div className="flex flex-col gap-1.5 pl-6">
                        <span className="text-[11px] text-[var(--muted)]">在这句{editing.where === "above" ? "上方" : "下方"}插入{editing.split ? "（与这句首尾相接，会把这句一分为二）" : ""}</span>
                        <textarea
                          autoFocus
                          value={editing.text}
                          placeholder="输入字幕，回车保存"
                          onChange={(ev) => setEditing({ ...editing, text: ev.target.value })}
                          onKeyDown={(ev) => {
                            if (ev.key === "Escape") setEditing(null);
                            if (ev.key === "Enter" && !ev.shiftKey) {
                              ev.preventDefault();
                              commitInsert();
                            }
                          }}
                          className="min-h-[40px] w-full resize-y rounded-md border border-[#1F5BD8] bg-white px-2 py-1.5 text-[13px] leading-relaxed text-[var(--ink)]"
                        />
                        <span className="flex flex-wrap items-center gap-1.5 text-[11px] text-[var(--muted)]">
                          开始
                          <TimeInput value={editing.start} onCommit={(v) => setEditing({ ...editing, start: v })} />
                          结束
                          <TimeInput value={editing.end} onCommit={(v) => setEditing({ ...editing, end: v })} />
                          <button className="h-[26px] whitespace-nowrap rounded-md bg-[#1F5BD8] px-2.5 font-semibold text-white" onClick={commitInsert}>
                            保存
                          </button>
                          <button className="h-[26px] whitespace-nowrap rounded-md border border-black/10 bg-white px-2.5 text-[var(--ink)]" onClick={() => setEditing(null)}>
                            取消
                          </button>
                        </span>
                      </div>
                    ) : null}
                    {editing?.key === l.key && !editing.mode ? (
                      <div className="flex flex-col gap-1.5 pl-6">
                        <textarea
                          autoFocus
                          value={editing.text}
                          onChange={(ev) => setEditing({ key: l.key, text: ev.target.value })}
                          onKeyDown={(ev) => {
                            if (ev.key === "Escape") setEditing(null);
                          }}
                          className="min-h-[52px] w-full resize-y rounded-md border border-[#1F5BD8] bg-white px-2 py-1.5 text-[13px] leading-relaxed text-[var(--ink)]"
                        />
                        <span className="flex flex-wrap items-center gap-1.5 text-[11px] text-[var(--muted)]">
                          <button className="h-[26px] whitespace-nowrap rounded-md bg-[#1F5BD8] px-2.5 font-semibold text-white" onClick={() => saveCueEdit(l, editing.text)}>
                            保存
                          </button>
                          <button className="h-[26px] whitespace-nowrap rounded-md border border-black/10 bg-white px-2.5 text-[var(--ink)]" onClick={() => setEditing(null)}>
                            取消
                          </button>
                          {l.edited ? (
                            <button className="h-[26px] whitespace-nowrap rounded-md border border-black/10 bg-white px-2.5 text-[var(--ink)]" onClick={() => saveCueEdit(l, null)}>
                              恢复整句识别原文
                            </button>
                          ) : null}
                          <span>Esc 取消</span>
                        </span>
                      </div>
                    ) : null}
                    <span className={`flex-wrap gap-1.5 pl-6 ${now && editing?.key !== l.key ? "flex" : "hidden group-hover:flex"}`}>
                      <button className="h-[26px] whitespace-nowrap rounded-md border border-black/10 bg-white px-2.5 text-[11px] leading-none text-[var(--ink)]" onClick={() => startInsert(l, "above")}>
                        ↑ 上方插入
                      </button>
                      <button className="h-[26px] whitespace-nowrap rounded-md border border-black/10 bg-white px-2.5 text-[11px] leading-none text-[var(--ink)]" onClick={() => startInsert(l, "below")}>
                        ↓ 下方插入
                      </button>
                      <button className="h-[26px] whitespace-nowrap rounded-md border border-black/10 bg-white px-2.5 text-[11px] leading-none text-[var(--ink)]" onClick={() => setEditing({ key: l.key, mode: "time" })}>
                        改时间
                      </button>
                      <button className="h-[26px] whitespace-nowrap rounded-md border border-[#E5B4AE] bg-white px-2.5 text-[11px] leading-none text-[#B42318]" onClick={() => deleteCue(l)}>
                        删除
                      </button>
                      <button className="h-[26px] whitespace-nowrap rounded-md border border-black/10 bg-white px-2.5 text-[11px] leading-none text-[var(--ink)]" onClick={() => setClipBounds({ s: l.start })}>
                        从这句开始
                      </button>
                      <button className="h-[26px] whitespace-nowrap rounded-md border border-black/10 bg-white px-2.5 text-[11px] leading-none text-[var(--ink)]" onClick={() => setClipBounds({ e: l.end })}>
                        到这句结束
                      </button>
                    </span>
                  </div>
                </div>
              );
            })}
          </div>
          </>
          ) : sideTab === "cover" ? (
          <div className="max-h-[calc(100vh-240px)] overflow-y-auto px-3 py-3">
            <CoverEditor
              anchor={anchor}
              clip={clip}
              currentTime={t}
              onSaved={(spec) => setClips((prev) => prev.map((c) => (c.id === clip.id ? { ...c, coverSpec: spec } : c)))}
            />
          </div>
          ) : sideTab === "meta" ? (
          <div className="flex max-h-[calc(100vh-240px)] flex-col gap-3 overflow-y-auto px-4 py-3.5">

            <div className="flex flex-wrap items-center gap-2">
              <span className="text-[15px] font-semibold text-[var(--ink)]">投稿信息</span>
              {clip.category ? <span className="rounded-md bg-[#EAF1FF] px-2 py-0.5 text-xs text-[#1F4FB8]">{clip.category}</span> : null}
              <span className="text-xs text-[var(--muted)]">AI 生成，可直接修改；用视频下方的「保存」统一保存，通过或淘汰时也会自动保存</span>
            </div>
            {clip.titles?.length > 1 ? (
              <div className="flex flex-wrap gap-1.5">
                {clip.titles.map((title) => (
                  <button
                    key={title}
                    className={`min-h-8 rounded-lg border px-2.5 py-1 text-left text-sm ${draft.title === title ? "border-[#1F5BD8] bg-[#EEF3FF] text-[#1F4FB8]" : "border-black/10 bg-white text-[var(--ink)]"}`}
                    onClick={() => editDraft({ title })}
                  >
                    {title}
                  </button>
                ))}
              </div>
            ) : null}
            <label className="flex flex-col gap-1 text-xs text-[var(--muted)]">
              标题
              <input className="h-9 rounded-lg border border-black/10 px-2.5 text-sm text-[var(--ink)]" value={draft.title} onChange={(e) => editDraft({ title: e.target.value })} />
            </label>
            <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
              <label className="flex flex-col gap-1 text-xs text-[var(--muted)]">
                封面文字
                <input className="h-9 rounded-lg border border-black/10 px-2.5 text-sm text-[var(--ink)]" value={draft.cover} onChange={(e) => editDraft({ cover: e.target.value })} />
              </label>
              <label className="flex flex-col gap-1 text-xs text-[var(--muted)]">
                标签（逗号分隔）
                <input className="h-9 rounded-lg border border-black/10 px-2.5 text-sm text-[var(--ink)]" value={draft.tags} onChange={(e) => editDraft({ tags: e.target.value })} />
              </label>
            </div>
            <label className="flex flex-col gap-1 text-xs text-[var(--muted)]">
              简介
              <textarea className="min-h-16 resize-y rounded-lg border border-black/10 px-2.5 py-2 text-sm leading-relaxed text-[var(--ink)]" value={draft.description} onChange={(e) => editDraft({ description: e.target.value })} />
            </label>
          </div>
          ) : (
          <div className="flex max-h-[calc(100vh-240px)] flex-col gap-4 overflow-y-auto px-4 py-3.5">
            <div className="flex flex-col gap-4">
              <div className="flex flex-col gap-1.5">
                <span className="text-xs text-[var(--muted)]">
                  评分（{clip.signals?.grade ? `${clip.signals.grade} 档 · ` : ""}综合 <b className="text-[#1F4FB8]">{scores ? Number(scores.total).toFixed(1) : (clip.score || 0).toFixed(2)}</b>）
                </span>
                {scores
                  ? DIMS.map(([k, label]) => (
                      <span key={k} className="grid grid-cols-[64px_minmax(0,1fr)_32px] items-center gap-2 text-xs text-[var(--ink)]">
                        {label}
                        <span className="h-1.5 overflow-hidden rounded-full bg-black/5">
                          <span className="block h-full" style={{ width: `${(scores[k] || 0) * 10}%`, background: k === "danmaku" ? "#E8833A" : "#1F5BD8" }} />
                        </span>
                        <span className="text-right tabular-nums">{scores[k] ?? "-"}</span>
                      </span>
                    ))
                  : <span className="text-xs text-[var(--muted)]">{clip.signals?.summary || "按信号切出，没有分项评分"}</span>}
                {clip.signals?.streamerRatio != null ? (
                  <span className="text-xs text-[var(--muted)]">主播说话占比 {Math.round(clip.signals.streamerRatio * 100)}%</span>
                ) : null}
              </div>
              <div className="flex flex-col gap-1.5 text-sm leading-relaxed text-[var(--ink)]">
                <span className="text-xs text-[var(--muted)]">推荐理由</span>
                <span>{clip.reason || "-"}</span>
                {clip.signals?.refineReason ? <span className="text-xs text-[var(--muted)]">精修：{clip.signals.refineReason}</span> : null}
              </div>
            </div>
          </div>
          )}
        </aside>
      </div>

      <footer className="sticky bottom-0 z-40 flex flex-wrap items-center gap-2.5 rounded-2xl border border-black/5 bg-white/95 px-5 py-3 shadow-[0_-4px_12px_rgba(0,0,0,0.06)] backdrop-blur">
        <button className="h-11 rounded-lg border border-black/10 bg-white px-3.5 text-sm" onClick={() => step(-1)}>上一条</button>
        <button className="h-11 rounded-lg border border-black/10 bg-white px-3.5 text-sm" onClick={() => step(1)}>下一条</button>
        <span className="text-xs text-[var(--muted)]">
          第 {Math.max(0, visible.findIndex((c) => c.id === clip.id)) + 1} / {visible.length || clips.length} 条 · 当前：{STATE_LABEL[stateOf(clip)]}
        </span>
        <span className="rounded-full bg-[#FFF3E6] px-2.5 py-1 text-xs text-[#9A4A0C]">待评阅 {counts.todo}</span>
        <span className="rounded-full bg-[#E8F5EE] px-2.5 py-1 text-xs text-[#1F7A45]">已通过 {counts.pass}</span>
        <span className="rounded-full bg-black/5 px-2.5 py-1 text-xs text-[var(--muted)]">已淘汰 {counts.reject}</span>
        <span className="flex-1" />
        <span className="hidden text-xs text-[var(--muted)] lg:inline">空格 播放 · I / O 设起止 · A 通过 · X 淘汰 · J / K 下一条 / 上一条 · L 切片列表</span>
        <button className="h-11 rounded-lg border border-[#E5B4AE] bg-white px-5 text-sm font-semibold text-[#B42318]" onClick={() => mark("DISCARD")}>淘汰</button>
        <button className="h-11 rounded-lg bg-[#1F7A45] px-6 text-sm font-semibold text-white" onClick={() => mark("KEEP")}>通过</button>
        <button
          className="h-11 rounded-lg border border-black/10 bg-white px-4 text-sm font-semibold text-[var(--ink)] disabled:opacity-40"
          disabled={status !== "SUCCESS"}
          title={status !== "SUCCESS" ? "切片文件还没剪好" : "用现有投稿表单投稿这条切片"}
          onClick={async () => {
            // 带上当前填写的投稿信息，投稿表单据此预填；字幕在投稿工作流里压进视频
            const d = draft;
            const subtitle = subtitleOf(clip);
            if (await saveAll()) onSubmitClip({ ...clip, titles: [d.title, ...(clip.titles || []).filter((t) => t !== d.title)], description: d.description, tags: d.tags, subtitle });
          }}
        >
          投稿这条
        </button>
        <button
          className="h-11 rounded-lg border border-black/10 bg-white px-4 text-sm text-[var(--ink)] hover:border-black/20"
          title={dirty ? "有未保存的改动，返回后会丢失" : "返回录播管理"}
          onClick={async () => {
            if (dirty && !(await dialogConfirm("当前切片有未保存的改动，返回后会丢失。确定返回吗？", { title: "返回录播管理" }))) return;
            onBack();
          }}
        >
          ‹ 返回录播管理
        </button>
      </footer>
    </div>
  );
}
