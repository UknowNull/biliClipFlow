import { useEffect, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { invokeCommand } from "../../lib/tauri";
import { clipFormError, createClipSubmission, joinClipTitle } from "./clipSubmit";
import { renderCoverDataUrl } from "./cover";
import CoverEditor from "./CoverEditor";

/** 封面缩略图：按封面参数渲染 */
function CoverThumb({ spec }) {
  const [url, setUrl] = useState("");
  useEffect(() => {
    let alive = true;
    setUrl("");
    if (spec?.frame?.path) renderCoverDataUrl(spec).then((u) => alive && setUrl(u)).catch(() => {});
    return () => {
      alive = false;
    };
  }, [spec]);
  if (!spec?.frame?.path) return <div className="flex aspect-video w-40 items-center justify-center rounded-md bg-black/5 text-[11px] text-[var(--muted)]">未生成封面</div>;
  return url ? <img src={url} alt="封面" className="aspect-video w-40 rounded-md object-cover" /> : <div className="aspect-video w-40 rounded-md bg-black/5" />;
}

const splitTags = (text) => String(text || "").split(/[,，]/).map((t) => t.trim()).filter(Boolean);
const pad = (n) => String(n).padStart(2, "0");
const hms = (sec) => {
  const s = Math.max(0, Math.floor(sec || 0));
  return `${pad(Math.floor(s / 3600))}:${pad(Math.floor((s % 3600) / 60))}:${pad(s % 60)}`;
};
const input = "w-full rounded-lg border border-black/10 bg-white/80 px-3 py-2 text-sm focus:border-[var(--accent)] focus:outline-none disabled:opacity-60";

/**
 * 批量投稿：勾选已通过的切片，逐条修改标题、简介、标签、分区后一起创建投稿任务。
 * 分区、合集、视频类型为公共设置，默认取主播的投稿设置；行内分区留空表示跟随公共设置。
 * 页面里的修改只用于本次投稿，不回写评阅页。
 */
export default function BatchSubmitView({ anchor, records, config, partitions, collections, onBack }) {
  const [common, setCommon] = useState({
    partitionId: config.partitionId ? String(config.partitionId) : "",
    collectionId: config.collectionId ? String(config.collectionId) : "",
    videoType: config.videoType || "ORIGINAL",
  });
  const baseTags = [...splitTags(config.tags), ...(config.activityTitle ? [config.activityTitle] : [])];
  const [rows, setRows] = useState(() =>
    records.map((r) => ({
      record: r,
      checked: true,
      title: joinClipTitle(config.title || "", r.titles?.[0]?.trim() || ""),
      description: r.description?.trim() || config.description || "",
      tags: [...new Set([...baseTags, ...splitTags(r.tags)])].join(","),
      partitionId: "",
      done: false,
      error: "",
      preview: false,
    })),
  );
  const [submitting, setSubmitting] = useState(false);
  const [coverIndex, setCoverIndex] = useState(-1);
  const [message, setMessage] = useState("");

  const update = (i, patch) => setRows((prev) => prev.map((row, k) => (k === i ? { ...row, ...patch } : row)));
  const formOf = (row) => ({
    title: row.title,
    description: row.description,
    partitionId: row.partitionId || common.partitionId,
    collectionId: common.collectionId,
    activityTopicId: config.topicId ? String(config.topicId) : "",
    activityMissionId: config.missionId ? String(config.missionId) : "",
    activityTitle: config.activityTitle || "",
    videoType: common.videoType,
  });
  const pending = rows.filter((row) => row.checked && !row.done);
  const allChecked = rows.every((row) => row.checked || row.done);

  const submit = async () => {
    if (submitting || !pending.length) return;
    // 先整体校验，有问题的行标红，全部通过才提交
    const errors = rows.map((row) => (row.checked && !row.done ? clipFormError(formOf(row), splitTags(row.tags)) : ""));
    setRows((prev) => prev.map((row, i) => ({ ...row, error: errors[i] })));
    if (errors.some(Boolean)) {
      setMessage("有切片的投稿信息不完整，请修改标红的行");
      return;
    }
    setSubmitting(true);
    setMessage("");
    let ok = 0;
    for (let i = 0; i < rows.length; i += 1) {
      const row = rows[i];
      if (!row.checked || row.done) continue;
      try {
        await createClipSubmission(row.record, formOf(row), [...new Set(splitTags(row.tags))]);
        // 任务创建成功后标记为已投稿
        if (row.record?.id != null) {
          await invokeCommand("anchor_clip_mark", { clipId: row.record.id, mark: "SUBMITTED" }).catch(() => {});
        }
        update(i, { done: true, error: "" });
        ok += 1;
      } catch (err) {
        update(i, { error: err?.message || String(err) });
      }
    }
    setSubmitting(false);
    setMessage(`已创建 ${ok} 条投稿任务，字幕烧录和上传在后台进行${ok < pending.length ? `；${pending.length - ok} 条失败，见标红的行` : ""}`);
  };

  const partitionSelect = (value, onChange, followLabel, disabled) => (
    <select className={input} value={value} onChange={(e) => onChange(e.target.value)} disabled={disabled}>
      <option value="">{followLabel}</option>
      {partitions.map((p) => (
        <option key={p.tid} value={String(p.tid)}>
          {p.name}
        </option>
      ))}
    </select>
  );

  return (
    <div className="space-y-4">
      <div className="rounded-2xl bg-[var(--surface)]/90 p-6 shadow-sm ring-1 ring-black/5">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div>
            <p className="text-sm uppercase tracking-[0.2em] text-[var(--muted)]">切片投稿</p>
            <h2 className="text-2xl font-semibold text-[var(--ink)]">批量投稿</h2>
            <div className="mt-1 text-xs text-[var(--muted)]">
              主播：{anchor?.nickname || "未知主播"}（房间号 {anchor?.uid}）· 共 {rows.length} 条已通过的切片
            </div>
          </div>
          <button className="rounded-full border border-black/10 bg-white px-3 py-1 text-xs font-semibold text-[var(--ink)]" onClick={onBack} disabled={submitting}>
            返回评阅
          </button>
        </div>
        <div className="mt-4 grid gap-3 lg:grid-cols-4">
          <label className="space-y-1">
            <div className="text-xs text-[var(--muted)]">B站分区（公共）</div>
            {partitionSelect(common.partitionId, (v) => setCommon((c) => ({ ...c, partitionId: v })), "请选择分区", submitting)}
          </label>
          <label className="space-y-1">
            <div className="text-xs text-[var(--muted)]">合集（公共，可选）</div>
            <select className={input} value={common.collectionId} onChange={(e) => setCommon((c) => ({ ...c, collectionId: e.target.value }))} disabled={submitting}>
              <option value="">不加入合集</option>
              {collections.map((c) => (
                <option key={c.seasonId} value={String(c.seasonId)}>
                  {c.name}
                </option>
              ))}
            </select>
          </label>
          <label className="space-y-1">
            <div className="text-xs text-[var(--muted)]">视频类型（公共）</div>
            <select className={input} value={common.videoType} onChange={(e) => setCommon((c) => ({ ...c, videoType: e.target.value }))} disabled={submitting}>
              <option value="ORIGINAL">原创</option>
              <option value="REPOST">转载</option>
            </select>
          </label>
          <div className="space-y-1">
            <div className="text-xs text-[var(--muted)]">活动话题（来自投稿设置）</div>
            <div className="rounded-lg bg-black/[0.03] px-3 py-2 text-sm text-[var(--ink)]">{config.activityTitle || "无"}</div>
          </div>
        </div>
      </div>

      <div className="flex items-center justify-between px-1 text-sm">
        <label className="flex items-center gap-2 text-[var(--ink)]">
          <input
            type="checkbox"
            checked={allChecked}
            disabled={submitting}
            onChange={(e) => setRows((prev) => prev.map((row) => (row.done ? row : { ...row, checked: e.target.checked })))}
          />
          全选
        </label>
        <span className="text-xs text-[var(--muted)]">已勾选 {pending.length} 条待投稿</span>
      </div>

      {rows.map((row, i) => {
        const r = row.record;
        const locked = row.done || submitting;
        return (
          <div key={r.id} className={`rounded-2xl bg-white p-4 shadow-sm ring-1 ${row.error ? "ring-rose-300" : "ring-black/5"} ${row.checked || row.done ? "" : "opacity-60"}`}>
            <div className="flex flex-wrap items-center gap-3 text-xs text-[var(--muted)]">
              <input type="checkbox" checked={row.checked} disabled={locked} onChange={(e) => update(i, { checked: e.target.checked })} />
              {r.signals?.grade ? <span className="rounded bg-[#EEF3FF] px-1.5 py-0.5 font-semibold text-[#1F4FB8]">{r.signals.grade} 档</span> : null}
              <span className="tabular-nums">
                {hms(r.startOffset)} – {hms(r.endOffset)} · {Math.round(r.endOffset - r.startOffset)} 秒
              </span>
              <span>字幕 {r.subtitle?.cues?.length || 0} 句</span>
              <button className="text-[#1F5BD8] hover:underline" onClick={() => update(i, { preview: !row.preview })}>
                {row.preview ? "收起预览" : "预览"}
              </button>
              {row.done ? <span className="ml-auto font-semibold text-[#1F7A45]">已创建投稿任务</span> : null}
              {row.error ? <span className="ml-auto text-rose-600">{row.error}</span> : null}
            </div>
            <div className="mt-3 flex items-center gap-3">
              <CoverThumb spec={r.coverSpec} />
              <button className="h-8 rounded-md border border-black/10 bg-white px-2.5 text-xs disabled:opacity-40" disabled={locked} onClick={() => setCoverIndex(i)}>
                编辑封面
              </button>
            </div>
            {row.preview ? <video className="mt-3 max-h-72 rounded-lg bg-black" src={convertFileSrc(r.filePath)} controls /> : null}
            <div className="mt-3 grid gap-3 lg:grid-cols-[1fr_220px]">
              <label className="space-y-1">
                <div className="flex justify-between text-xs text-[var(--muted)]">
                  标题<span className={row.title.length > 80 ? "text-rose-600" : ""}>{row.title.length}/80</span>
                </div>
                <input className={input} value={row.title} disabled={locked} onChange={(e) => update(i, { title: e.target.value })} />
              </label>
              <label className="space-y-1">
                <div className="text-xs text-[var(--muted)]">分区</div>
                {partitionSelect(row.partitionId, (v) => update(i, { partitionId: v }), "跟随公共设置", locked)}
              </label>
            </div>
            <div className="mt-3 grid gap-3 lg:grid-cols-2">
              <label className="space-y-1">
                <div className="text-xs text-[var(--muted)]">简介</div>
                <textarea className={input} rows={2} value={row.description} disabled={locked} onChange={(e) => update(i, { description: e.target.value })} />
              </label>
              <label className="space-y-1">
                <div className="text-xs text-[var(--muted)]">标签（逗号分隔）</div>
                <textarea className={input} rows={2} value={row.tags} disabled={locked} onChange={(e) => update(i, { tags: e.target.value })} />
              </label>
            </div>
          </div>
        );
      })}

      {coverIndex >= 0 ? (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50" onClick={() => setCoverIndex(-1)}>
          <div className="max-h-[90vh] w-[min(900px,94vw)] overflow-auto rounded-2xl bg-white p-4 shadow-xl" onClick={(e) => e.stopPropagation()}>
            <div className="mb-3 flex items-center justify-between">
              <span className="font-semibold text-[var(--ink)]">编辑封面 · {rows[coverIndex].title}</span>
              <button className="h-8 rounded-md border border-black/10 px-3 text-xs" onClick={() => setCoverIndex(-1)}>
                完成
              </button>
            </div>
            <CoverEditor
              anchor={anchor}
              clip={rows[coverIndex].record}
              onSaved={(spec) => update(coverIndex, { record: { ...rows[coverIndex].record, coverSpec: spec } })}
            />
          </div>
        </div>
      ) : null}

      <div className="sticky bottom-0 flex items-center justify-end gap-3 rounded-2xl bg-white/95 p-4 shadow-sm ring-1 ring-black/5">
        {message ? <span className="mr-auto text-sm text-[var(--ink)]">{message}</span> : null}
        <button
          className="h-11 rounded-lg bg-[#1F7A45] px-6 text-sm font-semibold text-white disabled:opacity-50"
          disabled={submitting || !pending.length}
          onClick={submit}
        >
          {submitting ? "正在创建投稿任务…" : `投稿已勾选的 ${pending.length} 条`}
        </button>
      </div>
    </div>
  );
}
