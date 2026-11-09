import { useEffect, useMemo, useState } from "react";
import { invokeCommand } from "../../lib/tauri";

const PAGE = 200;
const TABS = [
  ["chat", "弹幕"],
  ["gift", "礼物"],
  ["sc", "SC"],
  ["guard", "上舰"],
];
const pad = (n) => String(n).padStart(2, "0");
const hms = (sec) => {
  const s = Math.max(0, Math.floor(sec || 0));
  return `${pad(Math.floor(s / 3600))}:${pad(Math.floor((s % 3600) / 60))}:${pad(s % 60)}`;
};
const yuan = (v) => (v >= 100 ? v.toFixed(0) : v.toFixed(1));

// 一场直播的弹幕、礼物、SC、上舰记录。每条记录为 [场次秒数, 类型, uid, 昵称, 内容, 金额(元)]
export default function SessionEventsModal({ roomId, session, onClose }) {
  const [rows, setRows] = useState(null);
  const [error, setError] = useState("");
  const [tab, setTab] = useState("chat");
  const [query, setQuery] = useState("");
  const [page, setPage] = useState(0);

  const idsKey = session.recordIds.join(",");
  useEffect(() => {
    invokeCommand("anchor_session_events", { roomId, recordIds: idsKey.split(",").map(Number) })
      .then((data) => setRows(Array.isArray(data) ? data : []))
      .catch((err) => setError(err?.message || "读取弹幕失败"));
  }, [roomId, idsKey]);

  const stats = useMemo(() => {
    const out = { chat: 0, sc: 0, gift: 0, guard: 0, chatUsers: new Set(), giftMoney: 0, scMoney: 0, guardMoney: 0, payers: {} };
    for (const [, kind, uid, name, , money] of rows || []) {
      out[kind] += 1;
      if (kind === "chat") out.chatUsers.add(uid);
      if (kind === "gift") out.giftMoney += money;
      if (kind === "sc") out.scMoney += money;
      if (kind === "guard") out.guardMoney += money;
      if (money > 0) {
        const p = (out.payers[uid] ||= { name: name || uid, money: 0 });
        p.money += money;
      }
    }
    out.top = Object.values(out.payers).sort((a, b) => b.money - a.money).slice(0, 10);
    return out;
  }, [rows]);

  const list = useMemo(() => {
    const q = query.trim().toLowerCase();
    return (rows || []).filter((r) => r[1] === tab && (!q || r[3].toLowerCase().includes(q) || r[4].toLowerCase().includes(q)));
  }, [rows, tab, query]);
  const pages = Math.max(1, Math.ceil(list.length / PAGE));
  const current = Math.min(page, pages - 1);

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50" onClick={onClose}>
      <div className="flex h-[86vh] w-[min(980px,94vw)] flex-col rounded-2xl bg-white shadow-xl" onClick={(e) => e.stopPropagation()}>
        <div className="flex items-center justify-between border-b border-black/5 px-5 py-3">
          <div className="min-w-0">
            <div className="truncate font-semibold text-[var(--ink)]">弹幕与礼物 · {session.title || "-"}</div>
            <div className="text-xs text-[var(--muted)]">{session.day}</div>
          </div>
          <button className="h-8 rounded-lg border border-black/10 px-3 text-sm hover:border-black/20" onClick={onClose}>
            关闭
          </button>
        </div>

        {error ? <div className="px-5 py-4 text-sm text-red-600">{error}</div> : null}
        {!rows && !error ? <div className="px-5 py-4 text-sm text-[var(--muted)]">读取中…</div> : null}
        {rows ? (
          <>
            <div className="grid grid-cols-[repeat(5,minmax(0,1fr))_minmax(0,2fr)] gap-3 border-b border-black/5 px-5 py-3 text-sm">
              {[
                ["弹幕", stats.chat],
                ["发弹幕人数", stats.chatUsers.size],
                ["礼物", `${stats.gift} 次 · ¥${yuan(stats.giftMoney)}`],
                ["SC", `${stats.sc} 条 · ¥${yuan(stats.scMoney)}`],
                ["上舰", `${stats.guard} 次 · ¥${yuan(stats.guardMoney)}`],
              ].map(([label, value]) => (
                <div key={label}>
                  <div className="text-xs text-[var(--muted)]">{label}</div>
                  <div className="font-semibold tabular-nums text-[var(--ink)]">{value}</div>
                </div>
              ))}
              <div className="min-w-0">
                <div className="text-xs text-[var(--muted)]">付费前 10</div>
                <div className="line-clamp-2 text-xs leading-relaxed text-[var(--ink)]">
                  {stats.top.length ? stats.top.map((p) => `${p.name} ¥${yuan(p.money)}`).join("、") : "无"}
                </div>
              </div>
            </div>

            <div className="flex items-center gap-2 px-5 py-2.5">
              {TABS.map(([key, label]) => (
                <button
                  key={key}
                  className={`h-8 rounded-lg px-3 text-sm ${tab === key ? "bg-[#1F5BD8] font-semibold text-white" : "border border-black/10 hover:border-black/20"}`}
                  onClick={() => {
                    setTab(key);
                    setPage(0);
                  }}
                >
                  {label} {stats[key]}
                </button>
              ))}
              <input
                className="ml-auto h-8 w-56 rounded-lg border border-black/10 px-2.5 text-sm outline-none focus:border-[#1F5BD8]"
                placeholder="搜索昵称或内容"
                value={query}
                onChange={(e) => {
                  setQuery(e.target.value);
                  setPage(0);
                }}
              />
            </div>

            <div className="min-h-0 flex-1 overflow-y-auto border-y border-black/5">
              {list.slice(current * PAGE, (current + 1) * PAGE).map(([t, , uid, name, text, money], i) => (
                <div key={i} className="grid grid-cols-[72px_160px_minmax(0,1fr)_72px] gap-3 border-b border-black/[0.04] px-5 py-1.5 text-[13px]">
                  <span className="tabular-nums text-[var(--muted)]">{hms(t)}</span>
                  <span className="truncate text-[var(--muted)]" title={uid}>
                    {name || uid}
                  </span>
                  <span className="break-all text-[var(--ink)]">{text}</span>
                  <span className="text-right tabular-nums text-[var(--ink)]">{money > 0 ? `¥${yuan(money)}` : ""}</span>
                </div>
              ))}
              {!list.length ? <div className="px-5 py-6 text-center text-sm text-[var(--muted)]">没有记录</div> : null}
            </div>

            <div className="flex items-center justify-between px-5 py-2.5 text-sm text-[var(--muted)]">
              <span>共 {list.length} 条</span>
              <span className="flex items-center gap-2">
                <button className="h-7 rounded-md border border-black/10 px-2.5 disabled:opacity-40" disabled={current === 0} onClick={() => setPage(current - 1)}>
                  上一页
                </button>
                <span className="tabular-nums">
                  {current + 1} / {pages}
                </span>
                <button className="h-7 rounded-md border border-black/10 px-2.5 disabled:opacity-40" disabled={current >= pages - 1} onClick={() => setPage(current + 1)}>
                  下一页
                </button>
              </span>
            </div>
          </>
        ) : null}
      </div>
    </div>
  );
}
