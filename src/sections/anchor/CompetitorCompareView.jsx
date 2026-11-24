import { useEffect, useMemo, useState } from "react";

import { invokeCommand } from "../../lib/tauri";

/** 覆盖档位 → 展示样式 */
const COVERAGE_META = {
  FULL: { label: "完全覆盖", chip: "bg-[#E8F5EE] text-[#1F7A45]", border: "#2E9E63", tone: "#1F7A45" },
  PART: { label: "部分覆盖", chip: "bg-[#FFF3E6] text-[#9A4A0C]", border: "#E0A04A", tone: "#9A4A0C" },
  ZERO: { label: "未覆盖", chip: "bg-[#FDECEC] text-[#B42318]", border: "#D46A63", tone: "#B42318" },
  SKIP: { label: "跳过", chip: "bg-black/5 text-[var(--muted)]", border: "#A7ADB4", tone: "#5B6170" },
};

/** 归因层级 → 徽章 */
const LAYER_META = {
  CANDIDATE: { no: 1, label: "候选阶段", cls: "bg-[#6B7F8D]" },
  RULE: { no: 2, label: "提示词规则", cls: "bg-[#C9822E]" },
  ASR_MISSING: { no: 3, label: "ASR 缺失", cls: "bg-[#8C8F96]" },
  SPEAKER: { no: 4, label: "声纹误判", cls: "bg-[#7D6CA8]" },
};

const FILTERS = [
  ["all", "全部"],
  ["ZERO", "未覆盖"],
  ["PART", "部分覆盖"],
  ["FULL", "完全覆盖"],
];

const COVERAGE_ORDER = { ZERO: 0, PART: 1, FULL: 2, SKIP: 3 };

function fmtSec(value) {
  if (value == null) return "—";
  const total = Math.max(0, value);
  return `${Math.floor(total / 60)}:${String(Math.floor(total % 60)).padStart(2, "0")}`;
}

/**
 * 竞品对比报告页：逐条展示「结果 → 原因 → 建议」。
 * 未覆盖项排在最前，因为这才是本功能要回答的问题。
 */
export default function CompetitorCompareView({ anchor, task, onBack, onOpenClip }) {
  const [report, setReport] = useState(null);
  const [loading, setLoading] = useState(true);
  const [running, setRunning] = useState(false);
  const [filter, setFilter] = useState("all");
  const [message, setMessage] = useState("");
  const [advice, setAdvice] = useState({});

  const load = async () => {
    setLoading(true);
    try {
      const data = await invokeCommand("competitor_compare_get", { taskId: task.id });
      setReport(data || null);
    } catch (error) {
      setMessage(error?.message || "读取对比报告失败");
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [task.id]);

  const runCompare = async () => {
    if (running) return;
    setRunning(true);
    setMessage("");
    try {
      const data = await invokeCommand("competitor_compare_run", {
        roomId: anchor.uid,
        taskId: task.id,
      });
      setReport(data);
      setMessage("对比完成");
    } catch (error) {
      setMessage(error?.message || "对比失败");
    } finally {
      setRunning(false);
    }
  };

  const generateOne = (item) => {
    const key = item.competitorClipId;
    setAdvice((prev) => ({ ...prev, [key]: { loading: true, tips: [] } }));
    // 建议随报告一并算出，这里只做展开动作，避免多余请求
    setTimeout(() => {
      setAdvice((prev) => ({
        ...prev,
        [key]: { loading: false, tips: item.suggestions || [] },
      }));
    }, 300);
  };

  const items = useMemo(() => {
    const list = report?.items || [];
    const filtered = filter === "all" ? list : list.filter((item) => item.coverage === filter);
    return [...filtered].sort(
      (a, b) => (COVERAGE_ORDER[a.coverage] ?? 9) - (COVERAGE_ORDER[b.coverage] ?? 9),
    );
  }, [report, filter]);

  const counts = useMemo(() => {
    const list = report?.items || [];
    return {
      all: list.length,
      ZERO: list.filter((i) => i.coverage === "ZERO").length,
      PART: list.filter((i) => i.coverage === "PART").length,
      FULL: list.filter((i) => i.coverage === "FULL").length,
    };
  }, [report]);

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
          <p className="text-[11px] uppercase tracking-[0.2em] text-[var(--muted)]">竞品对比报告</p>
          <h2 className="text-lg font-semibold text-[var(--ink)]">
            {report?.sessionLabel || task.dateLabel || "本场"}
          </h2>
          <p className="text-xs text-[var(--muted)]">
            {report
              ? `本场 ${task.clipCount ?? 0} 条切片 vs 竞品 ${report.totalClips} 条切片`
              : "尚未生成对比报告"}
          </p>
        </div>
        <div className="flex-1" />
        {message ? <span className="text-xs text-[#9A4A0C]">{message}</span> : null}
        <button
          className="h-9 rounded-lg bg-[#1F5BD8] px-4 text-sm font-semibold text-white hover:brightness-110 disabled:opacity-50"
          disabled={running}
          onClick={runCompare}
        >
          {running ? "对比中…" : report ? "重新对比" : "开始对比"}
        </button>
      </section>

      {loading ? (
        <div className="rounded-2xl bg-white p-10 text-center text-sm text-[var(--muted)]">
          正在读取对比报告…
        </div>
      ) : !report ? (
        <EmptyState
          title="还没有对比结果"
          desc="竞品切片下载并转写完成后，点「开始对比」生成报告。若本场识别数据已清理，则无法对比。"
        />
      ) : (
        <>
          <div className="grid grid-cols-4 gap-2.5">
            <SummaryCard
              label="覆盖率"
              value={`${report.coverageRate.toFixed(0)}%`}
              tone="#1F7A45"
              sub={`${report.fullCount} 完全 + ${report.partCount} 部分×0.5`}
            />
            <SummaryCard label="完全覆盖" value={report.fullCount} tone="#1F7A45" sub="切点基本一致" />
            <SummaryCard label="部分覆盖" value={report.partCount} tone="#9A4A0C" sub="边界有偏差" />
            <SummaryCard label="未覆盖" value={report.zeroCount} tone="#B42318" sub="完全没切出来" />
          </div>

          <div className="flex flex-wrap items-center gap-2">
            <span className="text-xs font-semibold text-[var(--muted)]">筛选</span>
            {FILTERS.map(([value, label]) => (
              <button
                key={value}
                className={`h-7 rounded-lg border px-2.5 text-[11.5px] font-semibold transition-colors ${
                  filter === value
                    ? "border-[rgba(31,79,184,.4)] bg-[#EEF3FF] text-[#1F4FB8]"
                    : "border-black/10 bg-white text-[var(--ink)] hover:border-black/25"
                }`}
                onClick={() => setFilter(value)}
              >
                {label} {counts[value] ?? 0}
              </button>
            ))}
          </div>

          <div className="space-y-3">
            {items.map((item) => (
              <CompareCard
                key={item.competitorClipId}
                item={item}
                advice={advice[item.competitorClipId]}
                onGenerate={() => generateOne(item)}
                onOpenClip={onOpenClip}
              />
            ))}
            {items.length === 0 ? (
              <div className="rounded-2xl bg-white p-8 text-center text-sm text-[var(--muted)]">
                该筛选下没有条目
              </div>
            ) : null}
          </div>
        </>
      )}
    </div>
  );
}

function SummaryCard({ label, value, sub, tone }) {
  return (
    <div className="rounded-xl border border-black/5 bg-white px-3 py-2.5">
      <div className="text-[10.5px] font-semibold uppercase tracking-[0.1em] text-[var(--muted)]">
        {label}
      </div>
      <div className="text-xl font-bold leading-tight" style={{ color: tone }}>
        {value}
      </div>
      <div className="text-[11px] text-[var(--muted)]">{sub}</div>
    </div>
  );
}

function CompareCard({ item, advice, onGenerate, onOpenClip }) {
  const meta = COVERAGE_META[item.coverage] || COVERAGE_META.SKIP;
  const layer = item.reasonLayer ? LAYER_META[item.reasonLayer] : null;
  const theirsDur = item.located ? item.located.end - item.located.start : item.durationSecs;

  // 两条时间条共用一个刻度：原点取两者较早的起点，跨度取较晚的终点
  const starts = [item.located?.start, item.mineStart].filter((v) => v != null);
  const ends = [item.located?.end, item.mineEnd].filter((v) => v != null);
  const origin = starts.length ? Math.min(...starts) : 0;
  const span = ends.length ? Math.max(...ends) - origin : 1;
  const base = Math.max(span, 1);
  const segOf = (start, end) => ({
    left: `${((start - origin) / base) * 100}%`,
    width: `${Math.max(((end - start) / base) * 100, 1.2)}%`,
  });

  return (
    <div
      className="overflow-hidden rounded-xl border border-black/[0.08] bg-white"
      style={{ borderLeft: `3px solid ${meta.border}` }}
    >
      <div className="flex gap-3 border-b border-black/5 bg-gradient-to-b from-[#fbfcfd] to-[#f8fafb] p-3">
        <div className="relative flex aspect-video w-[132px] flex-none items-center justify-center overflow-hidden rounded-lg bg-[linear-gradient(135deg,#dfe8ee,#c8d8e3)]">
          {item.coverUrl ? (
            <img src={item.coverUrl} alt="" className="h-full w-full object-cover" />
          ) : (
            <span className="text-[9px] font-semibold uppercase tracking-wider text-[#7d95a5]">
              封面
            </span>
          )}
          <span className="absolute bottom-1 right-1 rounded bg-black/70 px-1 text-[9.5px] font-semibold text-white">
            {fmtSec(theirsDur)}
          </span>
        </div>
        <div className="min-w-0 flex-1">
          <div className="text-[13px] font-semibold leading-snug text-[var(--ink)]">{item.title}</div>
          <div className="mt-1 flex items-center gap-1.5 text-[11px] text-[var(--muted)]">
            <span className="flex h-3.5 w-3.5 items-center justify-center rounded-full bg-[#d8e6ee] text-[7.5px] font-bold text-[#4a7c96]">
              {(item.upName || item.upUid).slice(0, 1)}
            </span>
            <span>{item.upName || `UID ${item.upUid}`}</span>
          </div>
          <div className="mt-1.5 flex items-center gap-2">
            <span className={`rounded px-2 py-0.5 text-[11px] font-bold ${meta.chip}`}>
              {meta.label}
            </span>
            <span className="text-[11px] text-[var(--muted)]">交并比 {item.iou.toFixed(2)}</span>
          </div>
        </div>
      </div>

      <div className="space-y-1.5 border-b border-black/5 px-3 py-2.5">
        <BarRow
          label="你的切片"
          start={item.mineStart}
          end={item.mineEnd}
          color="bg-[#7FB069]"
          seg={segOf}
        />
        <BarRow
          label="竞品切片"
          start={item.located?.start}
          end={item.located?.end}
          color="bg-[#3D8FB5]"
          seg={segOf}
          highlight
        />
        {item.mineClipId ? (
          <div className="flex items-center gap-1.5 border-t border-dashed border-black/10 pt-1.5 text-[10.5px] text-[var(--muted)]">
            <span>对应你的切片</span>
            <button
              className="font-semibold text-[#1F7A45] underline underline-offset-2"
              onClick={() => onOpenClip?.(item.mineClipId)}
            >
              {item.mineTitle || `#${item.mineClipId}`}
            </button>
            <div className="flex-1" />
            <button
              className="h-6 rounded border border-black/10 bg-[#F6F8FA] px-2 text-[11px] font-semibold text-[#5B6170] hover:bg-[#EEF1F4]"
              onClick={() => onOpenClip?.(item.mineClipId, true)}
            >
              查看详情
            </button>
          </div>
        ) : item.coverage === "ZERO" ? (
          <div className="flex items-center gap-1.5 border-t border-dashed border-black/10 pt-1.5 text-[10.5px] text-[#B42318]">
            <span>你在该时段没有切片</span>
          </div>
        ) : null}
      </div>

      {item.reason ? (
        <div className="border-b border-black/5 px-3 py-2.5">
          <div className="mb-1.5 text-[10.5px] font-bold uppercase tracking-[0.1em] text-[var(--muted)]">
            原因
          </div>
          <div className="border-l-2 border-[rgba(0,161,214,.3)] pl-2.5 text-xs leading-relaxed text-[var(--ink)]">
            {layer ? (
              <span
                className={`mr-1.5 inline-block rounded px-1.5 py-0.5 align-[1px] text-[10px] font-bold text-white ${layer.cls}`}
              >
                第 {layer.no} 层 · {layer.label}
              </span>
            ) : null}
            {item.reason}
          </div>
        </div>
      ) : null}

      <div className="flex flex-wrap items-center gap-2 bg-[#fcfdfd] px-3 py-2.5">
        {item.coverage === "SKIP" ? (
          <span className="text-[11.5px] text-[var(--muted)]">数据缺失，无法分析</span>
        ) : item.reasonLayer === "ASR_MISSING" ? (
          <span className="text-[11.5px] text-[var(--muted)]">属识别质量问题，提示词无法解决</span>
        ) : advice ? (
          <>
            {advice.loading ? (
              <span className="text-[11.5px] text-[var(--muted)]">正在生成建议…</span>
            ) : advice.tips.length ? (
              <div className="min-w-[260px] flex-1 rounded-lg border border-[rgba(0,161,214,.22)] bg-[#F3F9FC] px-2.5 py-2">
                <div className="mb-1 text-[10px] font-bold uppercase tracking-[0.1em] text-[#0B6D90]">
                  优化建议
                </div>
                <ul className="ml-3.5 list-disc space-y-0.5">
                  {advice.tips.map((tip, index) => (
                    <li key={index} className="text-[11.5px] leading-relaxed text-[#24576B]">
                      {tip}
                    </li>
                  ))}
                </ul>
              </div>
            ) : (
              <span className="text-[11.5px] text-[var(--muted)]">这一项没有可执行的提示词建议</span>
            )}
            <button
              className="h-6 rounded border border-[rgba(31,122,69,.32)] bg-[#E8F5EE] px-2 text-[11px] font-semibold text-[#1F7A45] hover:bg-[#DCF0E5]"
              onClick={onGenerate}
            >
              重新生成
            </button>
          </>
        ) : (
          <>
            <span className="text-[11.5px] text-[var(--muted)]">尚未生成建议</span>
            <div className="flex-1" />
            <button
              className="h-6 rounded border border-[rgba(31,122,69,.32)] bg-[#E8F5EE] px-2 text-[11px] font-semibold text-[#1F7A45] hover:bg-[#DCF0E5]"
              onClick={onGenerate}
            >
              生成建议
            </button>
          </>
        )}
      </div>
    </div>
  );
}

function BarRow({ label, start, end, color, seg, highlight }) {
  const hasRange = start != null && end != null;
  return (
    <div className="grid grid-cols-[74px_1fr_86px] items-center gap-2">
      <span
        className={`text-right text-[10.5px] font-semibold ${
          highlight ? "text-[#0B6D90]" : "text-[var(--muted)]"
        }`}
      >
        {label}
      </span>
      <div className="relative h-5 overflow-hidden rounded bg-black/[0.045]">
        {hasRange ? (
          <div
            className={`absolute bottom-0 top-0 flex items-center rounded px-1.5 text-[9.5px] font-bold text-white ${color}`}
            style={seg(start, end)}
          >
            {(end - start).toFixed(1)}s
          </div>
        ) : (
          <div
            className="absolute inset-0 flex items-center rounded px-1.5 text-[9.5px] font-bold text-[#9AA3AB]"
            style={{
              background:
                "repeating-linear-gradient(45deg,#e8eaec,#e8eaec 5px,#f2f4f5 5px,#f2f4f5 10px)",
            }}
          >
            未切出
          </div>
        )}
      </div>
      <span className="text-[10.5px] tabular-nums text-[var(--muted)]">
        {hasRange ? `${start.toFixed(1)}–${end.toFixed(1)}s` : "—"}
      </span>
    </div>
  );
}

function EmptyState({ title, desc }) {
  return (
    <div className="rounded-2xl bg-white p-10 text-center">
      <div className="mx-auto mb-2.5 flex h-10 w-10 items-center justify-center rounded-xl bg-black/5 text-lg text-[var(--muted)]">
        ◌
      </div>
      <div className="mb-1 text-sm font-semibold text-[var(--ink)]">{title}</div>
      <div className="mx-auto max-w-sm text-xs leading-relaxed text-[var(--muted)]">{desc}</div>
    </div>
  );
}
