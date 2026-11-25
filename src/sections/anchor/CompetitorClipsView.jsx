import { useEffect, useMemo, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";

import { invokeCommand } from "../../lib/tauri";

/** 竞品切片状态 → 展示文案与配色 */
const STATUS_META = {
  PENDING: { label: "待下载", cls: "bg-black/5 text-[var(--muted)]" },
  DOWNLOADING: { label: "下载中", cls: "bg-[#FFF3E6] text-[#9A4A0C]" },
  TRANSCRIBING: { label: "转写中", cls: "bg-[#FFF3E6] text-[#9A4A0C]" },
  DONE: { label: "已就绪", cls: "bg-[#E8F5EE] text-[#1F7A45]" },
  FAILED: { label: "失败", cls: "bg-[#FDECEC] text-[#B42318]" },
  SKIPPED: { label: "跳过", cls: "bg-black/5 text-[var(--muted)]" },
};

function fmtSec(secs) {
  if (!secs || secs <= 0) return "—";
  const total = Math.round(secs);
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
}

/**
 * 竞品切片列表：查看本场下载了哪些竞品视频。
 *
 * 这个页面不依赖本场切片是否分析成功——只要有竞品下载下来就能看，
 * 用于判断竞品 UP 主切了哪些内容、我们的过滤规则是否命中。
 */
export default function CompetitorClipsView({ anchor, task, onBack, onOpenCompare }) {
  const [clips, setClips] = useState([]);
  const [loading, setLoading] = useState(true);
  const [message, setMessage] = useState("");
  const [selected, setSelected] = useState(null);
  // 封面需要走本地代理：B站图片有防盗链，直接 <img src> 会被拦
  const [covers, setCovers] = useState({});

  useEffect(() => {
    const missing = clips.filter((c) => c.coverUrl && !covers[c.id]);
    if (!missing.length) return;
    let cancelled = false;
    // 并发加载：逐张串行时几张 260KB 的图会明显卡顿
    void Promise.all(
      missing.map(async (clip) => {
        try {
          const data = await invokeCommand("video_proxy_image", { url: clip.coverUrl });
          if (!cancelled && data) {
            setCovers((prev) => ({ ...prev, [clip.id]: data }));
          }
        } catch (err) {
          // 单张失败不影响其他封面，控制台留痕便于排查
          console.warn("cover load failed", clip.bvid, err);
        }
      }),
    );
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [clips]);

  const load = async () => {
    setLoading(true);
    try {
      const list = await invokeCommand("competitor_clip_list", {
        roomId: anchor.uid,
        taskId: task.id,
      });
      setClips(list || []);
    } catch (error) {
      setMessage(error?.message || "读取竞品切片失败");
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [task.id]);

  const stats = useMemo(() => {
    const byStatus = {};
    for (const clip of clips) {
      byStatus[clip.status] = (byStatus[clip.status] || 0) + 1;
    }
    return { total: clips.length, byStatus };
  }, [clips]);

  /** 有转写结果才可能参与对比 */
  const readyCount = stats.byStatus.DONE || 0;

  return (
    <div className="space-y-3">
      <section className="flex flex-wrap items-center gap-3 rounded-2xl bg-[var(--surface)]/90 p-4 shadow-sm ring-1 ring-black/5">
        <button
          className="h-9 rounded-lg border border-black/10 bg-white px-3 text-sm text-[var(--ink)] hover:border-black/20"
          onClick={onBack}
        >
          ‹ 返回录播管理
        </button>
        <div className="min-w-0">
          <p className="text-[11px] uppercase tracking-[0.2em] text-[var(--muted)]">竞品切片</p>
          <h2 className="text-lg font-semibold text-[var(--ink)]">{task.dateLabel || "本场"}</h2>
          <p className="text-xs text-[var(--muted)]">
            {loading
              ? "正在读取…"
              : `共 ${stats.total} 条 · 已就绪 ${readyCount} 条${
                  stats.byStatus.FAILED ? ` · 失败 ${stats.byStatus.FAILED}` : ""
                }`}
          </p>
        </div>
        <div className="flex-1" />
        {message ? <span className="text-xs text-[#9A4A0C]">{message}</span> : null}
        <button
          className="h-9 rounded-lg border border-[rgba(31,79,184,0.32)] bg-[#EEF3FF] px-3 text-sm font-semibold text-[#1F4FB8] hover:bg-[#E3ECFF] disabled:cursor-not-allowed disabled:opacity-50"
          disabled={readyCount === 0}
          title={readyCount === 0 ? "竞品还没转写完成，无法对比" : "进入对比报告"}
          onClick={onOpenCompare}
        >
          查看对比报告
        </button>
      </section>

      {loading ? (
        <div className="rounded-2xl bg-white p-10 text-center text-sm text-[var(--muted)]">
          正在读取竞品切片…
        </div>
      ) : clips.length === 0 ? (
        <div className="rounded-2xl bg-white p-10 text-center">
          <div className="mx-auto mb-2.5 flex h-10 w-10 items-center justify-center rounded-xl bg-black/5 text-lg text-[var(--muted)]">
            ◌
          </div>
          <div className="mb-1 text-sm font-semibold text-[var(--ink)]">本场还没有竞品切片</div>
          <div className="mx-auto max-w-sm text-xs leading-relaxed text-[var(--muted)]">
            竞品 UP 主发布符合过滤条件的切片后，会自动下载并归属到本场。
            <br />
            可先检查「竞品分析」的配置是否命中。
          </div>
        </div>
      ) : (
        <div className="rounded-2xl bg-white p-3 shadow-sm ring-1 ring-black/5">
          <div className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5">
            {clips.map((clip) => (
              <ClipCard
                key={clip.id}
                clip={clip}
                cover={covers[clip.id]}
                selected={selected?.id === clip.id}
                onSelect={() => setSelected(clip)}
              />
            ))}
          </div>
        </div>
      )}

      {selected ? (
        <DetailPanel clip={selected} onClose={() => setSelected(null)} />
      ) : null}
    </div>
  );
}

function ClipCard({ clip, cover, selected, onSelect }) {
  const meta = STATUS_META[clip.status] || STATUS_META.PENDING;
  return (
    <button
      className={`overflow-hidden rounded-xl border bg-white text-left transition ${
        selected
          ? "border-[#00A1D6] ring-2 ring-[#00A1D6]/20"
          : "border-black/10 hover:border-[#00A1D6]/50 hover:shadow-sm"
      }`}
      onClick={onSelect}
    >
      <div className="relative flex aspect-video items-center justify-center overflow-hidden bg-[linear-gradient(135deg,#dfe8ee,#c8d8e3)]">
        {cover ? (
          <img src={cover} alt="" className="h-full w-full object-cover" />
        ) : (
          <span className="text-[10px] font-semibold uppercase tracking-wider text-[#7d95a5]">
            {clip.coverUrl ? "封面加载中…" : "无封面"}
          </span>
        )}
        <span className={`absolute left-1.5 top-1.5 rounded px-1.5 py-0.5 text-[10px] font-bold ${meta.cls}`}>
          {meta.label}
        </span>
        <span className="absolute bottom-1.5 right-1.5 rounded bg-black/70 px-1.5 py-0.5 text-[10px] font-semibold text-white">
          {fmtSec(clip.durationSecs)}
        </span>
      </div>
      <div className="px-2.5 py-2">
        <div
          className="line-clamp-2 min-h-[32px] text-xs font-semibold leading-snug text-[var(--ink)]"
          title={clip.title}
        >
          {clip.title || "未命名"}
        </div>
        <div className="mt-1 flex items-center gap-1 text-[11px] text-[var(--muted)]">
          <span className="min-w-0 flex-1 truncate">
            {clip.upName || `UID ${clip.upUid}`}
          </span>
        </div>
      </div>
    </button>
  );
}

function DetailPanel({ clip, onClose }) {
  const meta = STATUS_META[clip.status] || STATUS_META.PENDING;
  const videoSrc = clip.filePath ? convertFileSrc(clip.filePath) : "";
  return (
    <div className="rounded-2xl bg-white p-3 shadow-sm ring-1 ring-black/5">
      <div className="mb-2 flex items-center gap-2">
        <span className={`rounded px-2 py-0.5 text-[11px] font-bold ${meta.cls}`}>{meta.label}</span>
        <span className="min-w-0 flex-1 truncate text-xs font-semibold text-[var(--ink)]">
          {clip.title}
        </span>
        <button
          className="h-7 rounded-lg border border-black/10 px-2 text-[11px] text-[var(--muted)] hover:border-black/25"
          onClick={onClose}
        >
          关闭
        </button>
      </div>

      {videoSrc ? (
        <video
          key={clip.id}
          src={videoSrc}
          controls
          preload="metadata"
          className="max-h-[360px] w-full rounded-lg bg-black object-contain"
        />
      ) : (
        <div className="flex h-32 items-center justify-center rounded-lg bg-[#1c1f22] text-xs text-[#8f9aa6]">
          {clip.status === "FAILED" || clip.status === "SKIPPED"
            ? clip.statusNote || "该切片未下载成功"
            : "视频还在处理中，稍后再看"}
        </div>
      )}

      <div className="mt-2 grid grid-cols-2 gap-x-4 gap-y-1 text-[11.5px] text-[var(--muted)] sm:grid-cols-4">
        <span>UP 主：{clip.upName || clip.upUid}</span>
        <span>BV 号：{clip.bvid}</span>
        <span>投稿时间：{(clip.pubTime || "").slice(0, 16).replace("T", " ")}</span>
        <span>时长：{fmtSec(clip.durationSecs)}</span>
      </div>
      {clip.statusNote ? (
        <div className="mt-1.5 text-[11.5px] text-[#9A4A0C]">说明：{clip.statusNote}</div>
      ) : null}
    </div>
  );
}
