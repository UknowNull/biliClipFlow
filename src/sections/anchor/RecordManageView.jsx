import { Fragment, useEffect, useMemo, useState } from "react";
import { confirm as dialogConfirm } from "@tauri-apps/plugin-dialog";
import { invokeCommand } from "../../lib/tauri";
import SessionEventsModal from "./SessionEventsModal";
import ImportRecordsModal from "./ImportRecordsModal";

const SESSION_GAP_MS = 30 * 60 * 1000;
const ACTIVE = ["RUNNING", "QUEUED"];
const STAGE_LABELS = {
  PREPARE: "准备",
  ASR: "语音识别",
  SPEAKER: "声纹判定",
  CANDIDATES: "挑选候选",
  TRANSCRIBE: "转写",
  AI: "AI 切分",
  CUT: "剪辑",
};
export const CLIP_PROFILES = [
  { value: "reaction", label: "reaction（看作品解说）" },
  { value: "game", label: "游戏" },
  { value: "chat", label: "杂谈" },
];

const pad = (n) => String(n).padStart(2, "0");
const localDate = (iso) => {
  const d = new Date(iso);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
};
const localTime = (iso) => {
  const d = new Date(iso);
  return `${pad(d.getHours())}:${pad(d.getMinutes())}`;
};
const formatSize = (bytes) => {
  if (!bytes) return "-";
  const gb = bytes / 1024 ** 3;
  return gb >= 1 ? `${gb.toFixed(1)} GB` : `${(bytes / 1024 ** 2).toFixed(0)} MB`;
};

// 按场次分组：相邻分段间隔不超过 30 分钟视为同一场（跨午夜不拆开），与自动切片的场次规则一致
export function groupSessions(records) {
  const sessions = [];
  [...records]
    .sort((a, b) => new Date(a.startTime) - new Date(b.startTime))
    .forEach((record) => {
      const start = new Date(record.startTime).getTime();
      const end = new Date(record.endTime || record.startTime).getTime();
      const last = sessions[sessions.length - 1];
      if (last && start - last.endMs <= SESSION_GAP_MS) {
        last.records.push(record);
        last.endMs = Math.max(last.endMs, end);
      } else {
        sessions.push({ records: [record], startMs: start, endMs: end });
      }
    });
  const dayCount = {};
  sessions.forEach((s) => {
    s.date = localDate(s.records[0].startTime);
    dayCount[s.date] = (dayCount[s.date] || 0) + 1;
  });
  return sessions.reverse().map((s) => ({
    key: s.records.map((r) => r.id).join("-"),
    // 同一天有多场时附上开播时间区分
    day: dayCount[s.date] > 1 ? `${s.date} ${localTime(s.records[0].startTime)}` : s.date,
    title: s.records[s.records.length - 1]?.title || s.records[0]?.title || "",
    records: s.records,
    recordIds: s.records.map((r) => r.id),
    hours: Math.max(0, (s.endMs - s.startMs) / 3600000),
  }));
}

// 切片任务按场次划分，可能与按日期分组的录播不完全一致：优先完全匹配，否则取有交集的最近任务
export function findTask(summaries, recordIds) {
  const key = [...recordIds].sort((a, b) => a - b).join(",");
  const exact = summaries.find((t) => [...t.recordIds].sort((a, b) => a - b).join(",") === key);
  return exact || summaries.find((t) => t.recordIds.some((id) => recordIds.includes(id))) || null;
}

function Spark({ heat }) {
  if (!heat?.length) {
    return <span className="block h-px w-full bg-black/10" />;
  }
  const step = Math.max(1, Math.ceil(heat.length / 60));
  const bars = [];
  for (let i = 0; i < heat.length; i += step) {
    bars.push(Math.max(...heat.slice(i, i + step)));
  }
  const max = Math.max(1, ...bars);
  const sorted = [...heat].sort((a, b) => a - b);
  const hot = Math.max(1, sorted[Math.floor(sorted.length / 2)]) * 2;
  return (
    <span className="flex h-7 items-end gap-px">
      {bars.map((v, i) => (
        <span
          key={i}
          className="min-w-px flex-1 rounded-[1px]"
          style={{ height: `${Math.max(2, Math.round((28 * v) / max))}px`, background: v >= hot ? "#E8833A" : "#9DB5E8" }}
        />
      ))}
    </span>
  );
}

export default function RecordManageView({ anchor, records, loading, error, onBack, onOpenReview, onReload }) {
  const [importOpen, setImportOpen] = useState(false);
  const [summaries, setSummaries] = useState([]);
  const [heat, setHeat] = useState({});
  const [filter, setFilter] = useState("all");
  const [openFiles, setOpenFiles] = useState({});
  const [eventsSession, setEventsSession] = useState(null);
  // 删除确认弹窗：{ session, deleteClips }
  const [deleteTarget, setDeleteTarget] = useState(null);
  const [deleteClips, setDeleteClips] = useState(false);
  const [message, setMessage] = useState("");
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [profile, setProfile] = useState("reaction");
  const [voiceprint, setVoiceprint] = useState(null);
  const [voiceForm, setVoiceForm] = useState(null);
  const [voiceBusy, setVoiceBusy] = useState(false);

  const sessions = useMemo(() => groupSessions(records || []), [records]);

  const loadSummaries = () =>
    invokeCommand("anchor_clip_sessions", { roomId: anchor.uid })
      .then((list) => setSummaries(list || []))
      .catch((err) => setMessage(err?.message || "读取切片任务失败"));

  useEffect(() => {
    loadSummaries();
    invokeCommand("anchor_clip_profile_get", { roomId: anchor.uid })
      .then((value) => setProfile(value || "reaction"))
      .catch(() => {});
    invokeCommand("anchor_voiceprint_get", { roomId: anchor.uid })
      .then((value) => setVoiceprint(value || null))
      .catch(() => {});
  }, [anchor.uid]);

  // 弹幕热度按场次逐个加载，不阻塞列表显示
  useEffect(() => {
    let cancelled = false;
    (async () => {
      for (const s of sessions) {
        if (cancelled) return;
        try {
          const ctx = await invokeCommand("anchor_clip_session_context", { roomId: anchor.uid, recordIds: s.recordIds, includeTranscript: false });
          if (!cancelled) setHeat((prev) => ({ ...prev, [s.key]: ctx?.heat || [] }));
        } catch {
          // 文件缺失时没有热度，忽略
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [sessions, anchor.uid]);

  const running = summaries.some((t) => ACTIVE.includes(t.status));
  useEffect(() => {
    if (!running) return undefined;
    const timer = setInterval(loadSummaries, 2000);
    return () => clearInterval(timer);
  }, [running, anchor.uid]);

  const rows = sessions.map((s) => ({ ...s, task: findTask(summaries, s.recordIds) }));
  const stateOf = (task) => (!task ? "none" : ACTIVE.includes(task.status) ? "running" : task.status === "FAILED" ? "failed" : "done");
  const visible = rows.filter((s) => {
    const st = stateOf(s.task);
    if (filter === "todo") return st === "done" && s.task.reviewed < s.task.clipCount;
    return filter === "all" || filter === st;
  });
  const recent = rows.filter((s) => Date.now() - new Date(s.records[0].startTime).getTime() <= 30 * 86400000);
  const pending = rows.reduce((n, s) => n + (stateOf(s.task) === "done" ? s.task.clipCount - s.task.reviewed : 0), 0);
  const passed = rows.reduce((n, s) => n + (stateOf(s.task) === "done" ? s.task.passed : 0), 0);
  const failed = rows.filter((s) => stateOf(s.task) === "failed").length;
  const queueText = (() => {
    const t = summaries.find((x) => x.status === "RUNNING");
    if (t) return `分析中：${STAGE_LABELS[t.stage] || "处理中"} ${Math.round(t.progress || 0)}%`;
    return summaries.some((x) => x.status === "QUEUED") ? "排队中" : "空闲";
  })();

  const analyze = async (s) => {
    // 分析一律二次确认；已有切片时重新分析会在剪辑阶段删除现有切片与评阅结果，无法恢复
    const text =
      s.task?.clipCount > 0
        ? `重新分析「${s.day} ${s.title}」会删除这一场现有的 ${s.task.clipCount} 条切片文件，以及已评阅、已改的标题等结果。\n\n此操作不可挽回，确定要重新分析吗？`
        : `开始分析「${s.day} ${s.title}」？分析会占用较多 CPU 并调用大模型，可能持续数十分钟。`;
    const ok = await dialogConfirm(text, { title: s.task?.clipCount > 0 ? "重新分析（不可挽回）" : "分析场次", kind: s.task?.clipCount > 0 ? "warning" : "info" });
    if (!ok) return;
    setMessage("");
    try {
      await invokeCommand("anchor_analyze_clips", { roomId: anchor.uid, recordIds: s.recordIds });
      await loadSummaries();
    } catch (err) {
      setMessage(err?.message || "创建分析任务失败");
    }
  };

  // 删除场次：只删数据库记录和可选切片产物，不动原始录播文件
  const handleDelete = async () => {
    if (!deleteTarget) return;
    const ids = deleteTarget.records.map((r) => r.id);
    try {
      await invokeCommand("anchor_record_delete", {
        roomId: anchor.uid,
        recordIds: ids,
        deleteClips: deleteClips && deleteTarget.task?.clipCount > 0,
        clipTaskId: deleteTarget.task?.taskId ?? null,
      });
      setMessage("已删除");
      onReload?.();
    } catch (err) {
      setMessage(err?.message || "删除失败");
    } finally {
      setDeleteTarget(null);
      setDeleteClips(false);
    }
  };

  const enroll = async () => {
    const startSecs = voiceForm.time
      .split(":")
      .map(Number)
      .reduce((acc, part) => acc * 60 + (Number.isFinite(part) ? part : 0), 0);
    setVoiceBusy(true);
    try {
      const source = await invokeCommand("anchor_voiceprint_enroll", { roomId: anchor.uid, recordId: voiceForm.recordId, startSecs });
      setVoiceprint(source);
      setVoiceForm(null);
      setMessage("主播声纹已登记，下次分析生效");
    } catch (err) {
      setMessage(err?.message || "登记主播声纹失败");
    } finally {
      setVoiceBusy(false);
    }
  };

  const chip = (s) => {
    const st = stateOf(s.task);
    if (st === "done") return ["已出 " + s.task.clipCount + " 条切片", "bg-[#E8F5EE] text-[#1F7A45]"];
    if (st === "failed") return ["分析失败：" + (s.task.errorMessage || "未知原因"), "bg-[#FDECEC] text-[#B42318]"];
    if (st === "running")
      return [s.task.status === "QUEUED" ? "排队中" : `${STAGE_LABELS[s.task.stage] || "分析中"} ${Math.round(s.task.progress || 0)}%`, "bg-[#FFF3E6] text-[#9A4A0C]"];
    return ["未分析", "bg-black/5 text-[var(--muted)]"];
  };

  return (
    <div className="space-y-4">
      <section className="flex flex-wrap items-center gap-4 rounded-2xl bg-[var(--surface)]/90 p-5 shadow-sm ring-1 ring-black/5">
        <button className="h-9 rounded-lg border border-black/10 bg-white px-3 text-sm text-[var(--ink)] hover:border-black/20" onClick={onBack}>
          ‹ 主播订阅
        </button>
        <div className="min-w-0">
          <p className="text-xs uppercase tracking-[0.2em] text-[var(--muted)]">录播管理</p>
          <h2 className="text-xl font-semibold text-[var(--ink)]">{anchor.nickname || anchor.uid} 的录播记录</h2>
          <p className="mt-0.5 text-xs text-[var(--muted)]">
            房间号 {anchor.uid} · 内容类型 {CLIP_PROFILES.find((p) => p.value === profile)?.label || profile} · 主播声纹{voiceprint ? "已登记" : "未登记"}
          </p>
        </div>
        <div className="flex-1" />
        <span className="flex items-center gap-2 rounded-lg bg-black/[0.04] px-3 py-1.5 text-xs text-[var(--ink)]">
          <span className={`h-2 w-2 rounded-full ${running ? "bg-amber-500" : "bg-emerald-600"}`} />
          分析队列：{queueText}
        </span>
        <button className="h-9 rounded-lg border border-black/10 bg-white px-3 text-sm text-[var(--ink)] hover:border-black/20" onClick={() => setImportOpen(true)}>
          导入录播
        </button>
        <button
          className={`h-9 rounded-lg border px-3 text-sm ${settingsOpen ? "border-blue-300 bg-blue-50 text-blue-700" : "border-black/10 bg-white text-[var(--ink)] hover:border-black/20"}`}
          onClick={() => setSettingsOpen((v) => !v)}
        >
          切片设置（内容类型 / 声纹）
        </button>
      </section>

      {settingsOpen ? (
        <section className="flex flex-wrap items-center gap-6 rounded-2xl bg-white p-5 text-sm shadow-sm ring-1 ring-black/5">
          <label className="flex items-center gap-2 text-[var(--muted)]" title="影响 AI 判断方式，下次分析生效">
            内容类型
            <select
              className="h-9 rounded-lg border border-black/10 bg-white px-2 text-sm text-[var(--ink)]"
              value={profile}
              onChange={async (event) => {
                const value = event.target.value;
                setProfile(value);
                try {
                  await invokeCommand("anchor_clip_profile_set", { roomId: anchor.uid, profile: value });
                } catch (err) {
                  setMessage(err?.message || "保存内容类型失败");
                }
              }}
            >
              {CLIP_PROFILES.map((item) => (
                <option key={item.value} value={item.value}>
                  {item.label}
                </option>
              ))}
            </select>
          </label>
          <div
            className="flex flex-wrap items-center gap-2 text-[var(--muted)]"
            title="原片也是中文时，靠主播声纹区分主播解说与原片对白；选一段只有主播说话的位置，取其后 120 秒"
          >
            主播声纹
            {voiceForm ? (
              <>
                <select
                  className="h-9 max-w-[320px] rounded-lg border border-black/10 bg-white px-2 text-sm text-[var(--ink)]"
                  value={voiceForm.recordId}
                  onChange={(event) => setVoiceForm((prev) => ({ ...prev, recordId: Number(event.target.value) }))}
                >
                  {sessions.flatMap((s) =>
                    s.records.map((record, index) => (
                      <option key={record.id} value={record.id}>
                        {`${s.day} ${s.title}${s.records.length > 1 ? ` 分段 ${index + 1}` : ""}`}
                      </option>
                    ))
                  )}
                </select>
                <input
                  className="h-9 w-24 rounded-lg border border-black/10 bg-white px-2 text-sm text-[var(--ink)]"
                  placeholder="00:04:03"
                  value={voiceForm.time}
                  onChange={(event) => setVoiceForm((prev) => ({ ...prev, time: event.target.value }))}
                />
                <button className="h-9 rounded-lg bg-[#1F5BD8] px-3 text-sm font-semibold text-white disabled:opacity-50" disabled={voiceBusy} onClick={enroll}>
                  {voiceBusy ? "登记中..." : "确定"}
                </button>
                <button className="text-xs hover:underline" onClick={() => setVoiceForm(null)}>
                  取消
                </button>
              </>
            ) : (
              <>
                <span className={voiceprint ? "text-[var(--ink)]" : "text-amber-700"} title={voiceprint || ""}>
                  {voiceprint ? `已登记（${voiceprint}）` : "未登记"}
                </span>
                <button
                  className="text-xs text-blue-600 hover:underline"
                  onClick={() => setVoiceForm({ recordId: sessions[0]?.records[0]?.id, time: "" })}
                  disabled={!sessions.length}
                >
                  {voiceprint ? "重新登记" : "登记"}
                </button>
              </>
            )}
          </div>
        </section>
      ) : null}

      <section className="grid grid-cols-2 gap-3 md:grid-cols-4">
        {[
          ["近 30 天场次", recent.length, "text-[var(--ink)]"],
          ["待评阅切片", pending, "text-[#B4540F]"],
          ["已通过待投稿", passed, "text-[#1F7A45]"],
          ["分析失败", failed, "text-[#B42318]"],
        ].map(([label, value, color]) => (
          <div key={label} className="rounded-2xl bg-white px-4 py-3 shadow-sm ring-1 ring-black/5">
            <div className="text-xs text-[var(--muted)]">{label}</div>
            <div className={`mt-1 text-2xl font-bold ${color}`}>{value}</div>
          </div>
        ))}
      </section>

      {message ? <div className="rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-800">{message}</div> : null}

      <section className="overflow-hidden rounded-2xl bg-white shadow-sm ring-1 ring-black/5">
        <div className="flex flex-wrap items-center gap-3 border-b border-black/5 px-5 py-3">
          <span className="text-[15px] font-semibold text-[var(--ink)]">场次</span>
          <span className="text-xs text-[var(--muted)]">同一天间隔 30 分钟内的分段合并为一场</span>
          <div className="flex-1" />
          <div className="flex gap-1 rounded-lg bg-black/[0.04] p-[3px]">
            {[
              ["all", "全部"],
              ["todo", "待评阅"],
              ["none", "未分析"],
              ["failed", "失败"],
            ].map(([id, label]) => (
              <button
                key={id}
                className={`h-[30px] rounded-md px-3 text-sm ${filter === id ? "bg-white font-semibold text-[var(--ink)] shadow-sm" : "text-[var(--muted)]"}`}
                onClick={() => setFilter(id)}
              >
                {label}
              </button>
            ))}
          </div>
        </div>
        {loading ? (
          <div className="px-5 py-8 text-sm text-[var(--muted)]">加载中...</div>
        ) : error ? (
          <div className="m-5 rounded-lg border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700">{error}</div>
        ) : visible.length === 0 ? (
          <div className="px-5 py-10 text-center text-sm text-[var(--muted)]">{sessions.length ? "这一组没有场次" : "暂无录播记录"}</div>
        ) : (
          <div className="overflow-x-auto">
            <div className="min-w-[940px]">
              <div className="grid grid-cols-[120px_minmax(180px,1.6fr)_64px_150px_200px_330px] gap-3 border-b border-black/5 px-5 py-2.5 text-xs text-[var(--muted)]">
                <span>日期</span>
                <span>直播标题</span>
                <span>时长</span>
                <span>弹幕热度</span>
                <span>分析 / 评阅</span>
                <span className="text-right">操作</span>
              </div>
              {visible.map((s) => {
                const st = stateOf(s.task);
                const [chipText, chipClass] = chip(s);
                const open = !!openFiles[s.key];
                return (
                  <Fragment key={s.key}>
                    <div className="grid grid-cols-[120px_minmax(180px,1.6fr)_64px_150px_200px_330px] items-center gap-3 border-b border-black/5 px-5 py-3 text-sm text-[var(--ink)]">
                      <span className="tabular-nums">{s.day}</span>
                      <span className="min-w-0">
                        <span className="block truncate font-semibold" title={s.title}>
                          {s.title || "-"}
                        </span>
                        <span className="text-xs text-[var(--muted)]">{s.records.length} 个录播分段</span>
                      </span>
                      <span className="tabular-nums">{s.hours.toFixed(1)}h</span>
                      <Spark heat={heat[s.key]} />
                      <span className="flex min-w-0 flex-col items-start gap-1.5">
                        <span className={`max-w-full truncate rounded-md px-2 py-0.5 text-xs ${chipClass}`} title={chipText}>
                          {chipText}
                        </span>
                        {st === "done" && s.task.clipCount > 0 ? (
                          <span className="flex w-full items-center gap-1.5 text-xs text-[var(--muted)]">
                            <span className="h-1.5 flex-1 overflow-hidden rounded-full bg-black/5">
                              <span className="block h-full bg-[#1F5BD8]" style={{ width: `${Math.round((100 * s.task.reviewed) / s.task.clipCount)}%` }} />
                            </span>
                            已评阅 {s.task.reviewed}/{s.task.clipCount}
                          </span>
                        ) : null}
                      </span>
                      <span className="flex justify-end gap-1.5">
                        {st === "done" && s.task.clipCount > 0 ? (
                          <button className="h-8 rounded-lg bg-[#1F5BD8] px-3 text-sm font-semibold text-white" onClick={() => onOpenReview({ ...s, task: s.task })}>
                            评阅切片
                          </button>
                        ) : null}
                        <button
                          className="h-8 rounded-lg border border-black/10 bg-white px-2.5 text-sm hover:border-black/20 disabled:opacity-50"
                          disabled={st === "running"}
                          onClick={() => analyze(s)}
                        >
                          {st === "done" || st === "failed" ? "重新分析" : st === "running" ? "分析中" : "分析"}
                        </button>
                        <button
                          className="h-8 rounded-lg border border-black/10 bg-white px-2.5 text-sm hover:border-black/20"
                          onClick={() => setOpenFiles((prev) => ({ ...prev, [s.key]: !open }))}
                        >
                          {open ? "收起文件" : "录播文件"}
                        </button>
                        <button className="h-8 rounded-lg border border-black/10 bg-white px-2.5 text-sm hover:border-black/20" onClick={() => setEventsSession(s)}>
                          弹幕礼物
                        </button>
                        <button
                          className="h-8 rounded-lg border border-[#B42318]/25 bg-white px-2.5 text-sm text-[#B42318] hover:border-[#B42318]/45 disabled:opacity-50"
                          disabled={st === "running"}
                          onClick={() => {
                            setDeleteTarget(s);
                            setDeleteClips(false);
                          }}
                        >
                          删除
                        </button>
                      </span>
                    </div>
                    {open ? (
                      <div className="mx-5 mb-3 rounded-lg bg-black/[0.03] py-1">
                        {s.records.map((record) => (
                          <div key={record.id} className="grid grid-cols-[64px_minmax(0,1fr)_100px_80px_190px] items-center gap-3 px-3.5 py-2 text-xs">
                            <span className="tabular-nums text-[var(--muted)]">{localTime(record.startTime)}</span>
                            <span className="truncate" title={record.filePath}>
                              {record.filePath.split(/[\\/]/).pop()}
                            </span>
                            <span className="tabular-nums text-[var(--muted)]">{formatSize(record.fileSize)}</span>
                            <span className={record.status === "ERROR" ? "text-red-600" : record.status === "RECORDING" ? "text-emerald-700" : "text-[var(--muted)]"} title={record.errorMessage || ""}>
                              {record.status === "RECORDING" ? "录制中" : record.status === "ERROR" ? "出错" : "已完成"}
                            </span>
                            <span className="flex justify-end gap-1.5">
                              <button
                                className="h-7 rounded-md border border-black/10 bg-white px-2.5 hover:border-black/20"
                                onClick={() => invokeCommand("anchor_open_record_file", { filePath: record.filePath }).catch((err) => setMessage(err?.message || "打开失败"))}
                              >
                                播放
                              </button>
                              <button
                                className="h-7 rounded-md border border-black/10 bg-white px-2.5 hover:border-black/20"
                                onClick={() => invokeCommand("anchor_open_record_dir", { filePath: record.filePath }).catch((err) => setMessage(err?.message || "打开失败"))}
                              >
                                打开目录
                              </button>
                            </span>
                          </div>
                        ))}
                      </div>
                    ) : null}
                  </Fragment>
                );
              })}
            </div>
          </div>
        )}
      </section>
      {deleteTarget ? (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50" onClick={() => setDeleteTarget(null)}>
          <div className="w-[440px] rounded-2xl bg-white p-5 text-sm shadow-xl" onClick={(e) => e.stopPropagation()}>
            <div className="text-base font-semibold text-[var(--ink)]">删除这一场录播</div>
            <div className="mt-2 text-xs text-[var(--muted)]">
              {deleteTarget.day} · {deleteTarget.title || "未命名"}
            </div>
            <div className="mt-3 rounded-lg bg-[#FFF8EF] px-3 py-2 text-xs text-[#9A4A0C]">
              只删除软件内的记录，不会删除磁盘上的原始录播文件。
            </div>
            {deleteTarget.task?.clipCount > 0 ? (
              <label className="mt-3 flex items-start gap-2 text-xs text-[var(--ink)]">
                <input
                  type="checkbox"
                  className="mt-0.5"
                  checked={deleteClips}
                  onChange={(e) => setDeleteClips(e.target.checked)}
                />
                <span>
                  同时删除这一场的 {deleteTarget.task.clipCount} 条切片文件
                  <span className="block text-[var(--muted)]">不勾选则只删录播记录，切片保留在磁盘上</span>
                </span>
              </label>
            ) : null}
            {!(deleteTarget.task?.clipCount > 0) ? (
              <div className="mt-3 text-xs text-[var(--muted)]">
                {deleteTarget.task ? "这一场还没有分析出切片" : "这一场还没有进行切片分析"}
              </div>
            ) : null}
            <div className="mt-4 flex justify-end gap-2">
              <button className="h-9 rounded-lg border border-black/10 px-4" onClick={() => setDeleteTarget(null)}>
                取消
              </button>
              <button className="h-9 rounded-lg bg-[#B42318] px-4 font-semibold text-white" onClick={handleDelete}>
                确认删除
              </button>
            </div>
          </div>
        </div>
      ) : null}
      {importOpen ? <ImportRecordsModal anchor={anchor} onClose={() => setImportOpen(false)} onImported={onReload} /> : null}
      {eventsSession ? <SessionEventsModal roomId={anchor.uid} session={eventsSession} onClose={() => setEventsSession(null)} /> : null}
    </div>
  );
}
