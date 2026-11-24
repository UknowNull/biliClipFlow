import { useEffect, useRef, useState } from "react";

/**
 * 竞品与本地切片的双屏对比详情。
 *
 * 两个播放器可独立控制，也可开启同步：任一边播放/暂停/拖动，另一边跟随。
 * 视频源由调用方传入（本地预览文件路径），这里只负责播放与对照展示。
 */
export default function CompetitorClipDetail({
  mine,
  theirs,
  onBack,
  onOpenClip,
}) {
  const mineRef = useRef(null);
  const theirsRef = useRef(null);
  const [sync, setSync] = useState(true);
  const [speed, setSpeed] = useState(1);
  const [playing, setPlaying] = useState({ mine: false, theirs: false });
  const [times, setTimes] = useState({ mine: 0, theirs: 0 });

  const videos = { mine: mineRef, theirs: theirsRef };

  const readTime = (key) => {
    const el = videos[key].current;
    return el ? el.currentTime : 0;
  };

  /** 播放/暂停：同步开启时两边一起 */
  const toggle = (key) => {
    const el = videos[key].current;
    if (!el) return;
    const next = !playing[key];
    if (next) {
      void el.play().catch(() => {});
    } else {
      el.pause();
    }
    setPlaying((prev) => {
      const updated = { ...prev, [key]: next };
      if (sync) {
        const other = key === "mine" ? "theirs" : "mine";
        const otherEl = videos[other].current;
        if (otherEl) {
          if (next) void otherEl.play().catch(() => {});
          else otherEl.pause();
        }
        updated[other] = next;
      }
      return updated;
    });
  };

  /** 拖动进度条：同步开启时把另一边也拉到相同百分比位置 */
  const seek = (key, ratio) => {
    const apply = (target, r) => {
      const el = videos[target].current;
      if (!el || !isFinite(el.duration)) return;
      el.currentTime = el.duration * r;
      setTimes((prev) => ({ ...prev, [target]: el.currentTime }));
    };
    apply(key, ratio);
    if (sync) apply(key === "mine" ? "theirs" : "mine", ratio);
  };

  const changeSpeed = () => {
    const speeds = [1, 1.25, 1.5, 2, 0.5];
    const next = speeds[(speeds.indexOf(speed) + 1) % speeds.length];
    setSpeed(next);
    Object.values(videos).forEach((ref) => {
      if (ref.current) ref.current.playbackRate = next;
    });
  };

  const restart = () => {
    Object.entries(videos).forEach(([key, ref]) => {
      if (!ref.current) return;
      ref.current.currentTime = 0;
      void ref.current.play().catch(() => {});
    });
    setPlaying({ mine: true, theirs: true });
  };

  useEffect(() => {
    // 组件卸载时确保暂停，避免声音残留在后台
    return () => {
      Object.values(videos).forEach((ref) => ref.current?.pause());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const mineDur = mine?.end != null && mine?.start != null ? mine.end - mine.start : 0;
  const theirsDur =
    theirs?.end != null && theirs?.start != null ? theirs.end - theirs.start : 0;
  const headGap = (mine?.start ?? 0) - (theirs?.start ?? 0);
  const tailGap = (theirs?.end ?? 0) - (mine?.end ?? 0);

  return (
    <div className="space-y-3">
      <section className="flex flex-wrap items-center gap-3 rounded-2xl bg-[var(--surface)]/90 p-4 shadow-sm ring-1 ring-black/5">
        <button
          className="h-9 rounded-lg border border-black/10 bg-white px-3 text-sm text-[var(--ink)] hover:border-black/20"
          onClick={onBack}
        >
          ‹ 返回对比报告
        </button>
        <div className="min-w-0">
          <p className="text-[11px] uppercase tracking-[0.2em] text-[var(--muted)]">切片对比详情</p>
          <h2 className="truncate text-base font-semibold text-[var(--ink)]">{theirs?.title}</h2>
        </div>
      </section>

      <div className="grid grid-cols-2 gap-3">
        <PlayerPanel
          tone="mine"
          label="你的切片"
          name={mine?.title || "（无对应切片）"}
          range={mine}
          videoRef={mineRef}
          playing={playing.mine}
          time={times.mine}
          onToggle={() => toggle("mine")}
          onSeek={(ratio) => seek("mine", ratio)}
          emptyText="该时段你没有切片"
          onOpenClip={mine?.id ? () => onOpenClip?.(mine.id) : null}
        />
        <PlayerPanel
          tone="theirs"
          label="竞品切片"
          name={theirs?.title || "—"}
          range={theirs}
          videoRef={theirsRef}
          playing={playing.theirs}
          time={times.theirs}
          onToggle={() => toggle("theirs")}
          onSeek={(ratio) => seek("theirs", ratio)}
          emptyText="竞品视频不可用"
        />
      </div>

      <div className="flex flex-wrap items-center gap-3 rounded-xl border border-black/10 bg-white px-3 py-2.5 text-xs">
        <label className="flex cursor-pointer items-center gap-1.5 font-semibold">
          <input type="checkbox" checked={sync} onChange={(e) => setSync(e.target.checked)} />
          同步播放
        </label>
        <span className="text-[var(--muted)]">
          {sync ? "两边进度一起走，拖任意一个进度条另一边跟随" : "两边独立控制"}
        </span>
        <div className="flex-1" />
        <button
          className="h-7 rounded-lg border border-[rgba(154,74,12,.3)] bg-[#FFF3E6] px-2.5 text-[11.5px] font-semibold text-[#9A4A0C] hover:bg-[#FFE9D5]"
          onClick={restart}
        >
          从头播放
        </button>
        <button
          className="h-7 rounded-lg border border-black/10 bg-white px-2.5 text-[11.5px] font-semibold text-[var(--muted)] hover:border-black/25"
          onClick={changeSpeed}
        >
          {speed.toFixed(2).replace(/0$/, "")}×
        </button>
      </div>

      <div className="rounded-xl border border-black/5 bg-white p-3">
        <div className="mb-2 text-xs font-semibold text-[var(--ink)]">差异分析</div>
        <div className="grid grid-cols-4 gap-2.5">
          <DiffStat label="你的时长" value={`${mineDur.toFixed(1)}s`} />
          <DiffStat label="竞品时长" value={`${theirsDur.toFixed(1)}s`} />
          <DiffStat
            label="起点偏差"
            value={`${headGap > 0 ? "+" : ""}${headGap.toFixed(1)}s`}
            warn={Math.abs(headGap) > 3}
          />
          <DiffStat
            label="终点偏差"
            value={`${tailGap > 0 ? "+" : ""}${tailGap.toFixed(1)}s`}
            warn={Math.abs(tailGap) > 3}
          />
        </div>
        <div className="mt-2 text-[11.5px] leading-relaxed text-[var(--muted)]">
          {tailGap > 3
            ? `竞品比你多切结尾 ${tailGap.toFixed(1)} 秒，可能是终点判定过早。`
            : tailGap < -3
              ? `你比竞品多切结尾 ${Math.abs(tailGap).toFixed(1)} 秒，可能是终点收得太晚。`
              : "两地终点基本一致。"}
          {headGap > 3
            ? ` 竞品比你早开始 ${headGap.toFixed(1)} 秒。`
            : headGap < -3
              ? ` 你比竞品早开始 ${Math.abs(headGap).toFixed(1)} 秒。`
              : ""}
        </div>
      </div>
    </div>
  );
}

function PlayerPanel({
  tone,
  label,
  name,
  range,
  videoRef,
  playing,
  time,
  onToggle,
  onSeek,
  emptyText,
  onOpenClip,
}) {
  const hasVideo = Boolean(range?.videoPath);
  const duration = range?.end != null && range?.start != null ? range.end - range.start : 0;
  const ratio = duration > 0 ? Math.min(time / duration, 1) : 0;
  const toneCls =
    tone === "mine" ? "bg-[#E8F5EE] text-[#1F7A45]" : "bg-[#E6F2F8] text-[#0B6D90]";

  return (
    <div className="overflow-hidden rounded-xl border border-black/[0.07] bg-white">
      <div className="flex items-center gap-2 border-b border-black/5 bg-[#fafbfc] px-3 py-2">
        <span className={`rounded px-1.5 py-0.5 text-[10.5px] font-bold ${toneCls}`}>{label}</span>
        <span className="min-w-0 flex-1 truncate text-xs font-semibold text-[var(--ink)]">{name}</span>
        {onOpenClip ? (
          <button
            className="h-6 flex-none rounded border border-black/10 bg-white px-2 text-[10.5px] font-semibold text-[var(--muted)] hover:border-black/25"
            onClick={onOpenClip}
          >
            在评阅中打开
          </button>
        ) : null}
      </div>

      <div className="relative flex aspect-video items-center justify-center overflow-hidden bg-[#1c1f22]">
        {hasVideo ? (
          <video
            ref={videoRef}
            src={range.videoPath}
            className="h-full w-full object-contain"
            preload="metadata"
            playsInline
          />
        ) : (
          <div className="text-center">
            <div className="mx-auto mb-2 flex h-10 w-10 items-center justify-center rounded-full bg-white/[0.08] text-sm text-[#8F9AA6]">
              ▶
            </div>
            <div className="text-xs font-semibold text-[#8F9AA6]">{emptyText}</div>
            {range?.start != null ? (
              <div className="mt-0.5 text-[10.5px] tabular-nums text-[#5E6873]">
                {range.start.toFixed(1)}s – {range.end.toFixed(1)}s
              </div>
            ) : null}
          </div>
        )}
      </div>

      <div className="flex items-center gap-2 border-t border-black/5 px-2.5 py-2">
        <button
          className="flex h-7 w-7 flex-none items-center justify-center rounded-lg border border-black/10 bg-white text-[11px] text-[var(--ink)] hover:border-[#00A1D6] hover:text-[#00A1D6] disabled:opacity-40"
          onClick={onToggle}
          disabled={!hasVideo}
        >
          {playing ? "❚❚" : "▶"}
        </button>
        <span className="min-w-[64px] text-[10.5px] tabular-nums text-[var(--muted)]">
          {formatSec(time)} / {formatSec(duration)}
        </span>
        <input
          type="range"
          min="0"
          max="1000"
          value={Math.round(ratio * 1000)}
          className="h-1 min-w-[40px] flex-1 cursor-pointer appearance-none rounded bg-black/10 accent-[#00A1D6] disabled:opacity-40"
          onChange={(e) => onSeek(Number(e.target.value) / 1000)}
          disabled={!hasVideo}
        />
      </div>
    </div>
  );
}

function DiffStat({ label, value, warn }) {
  return (
    <div className="rounded-lg border border-black/5 bg-[#f7f9fa] px-2.5 py-2">
      <div className="text-[10.5px] font-semibold text-[var(--muted)]">{label}</div>
      <div
        className="text-base font-bold tabular-nums"
        style={{ color: warn ? "#9A4A0C" : "var(--ink)" }}
      >
        {value}
      </div>
    </div>
  );
}

function formatSec(value) {
  if (!isFinite(value) || value < 0) return "00:00";
  return `${String(Math.floor(value / 60)).padStart(2, "0")}:${String(Math.floor(value % 60)).padStart(2, "0")}`;
}
