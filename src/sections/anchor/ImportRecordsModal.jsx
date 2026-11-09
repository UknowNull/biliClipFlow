import { useEffect, useState } from "react";
import { confirm as dialogConfirm, open as openDialog } from "@tauri-apps/plugin-dialog";
import { invokeCommand } from "../../lib/tauri";

const STATUS = {
  new: ["新录播", "bg-[#E8F5EE] text-[#1F7A45]"],
  overlap: ["时间重叠", "bg-[#FFF3E6] text-[#9A4A0C]"],
  imported: ["已导入过", "bg-black/5 text-[var(--muted)]"],
  invalid: ["跳过", "bg-rose-50 text-rose-600"],
};
const POLICIES = [
  ["skip", "跳过重叠的视频", "保留库里已有的录播，重叠的这几段不登记"],
  ["replace", "替换旧记录", "删除重叠的旧录播记录（只删记录，不删文件，不可恢复），登记导入的视频"],
  ["keep", "都保留", "两份都登记，同一场里会有重复内容"],
];
const fmt = (iso) => (iso ? new Date(iso).toLocaleString("zh-CN", { hour12: false }) : "-");
const dur = (s) => `${Math.floor(s / 3600)}:${String(Math.floor((s % 3600) / 60)).padStart(2, "0")}:${String(s % 60).padStart(2, "0")}`;
const baseName = (p) => String(p || "").split(/[\\/]/).pop();

/** 录播导入：选目录 → 扫描预览（视频、字幕、弹幕） → 有时间重叠时选择处理方式 → 登记入库 */
export default function ImportRecordsModal({ anchor, onClose, onImported }) {
  const [dir, setDir] = useState("");
  const [items, setItems] = useState(null);
  const [policy, setPolicy] = useState("skip");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");

  const pickAndScan = async () => {
    const selected = await openDialog({ directory: true, multiple: false, title: "选择录播目录" });
    if (typeof selected !== "string") {
      if (!items) onClose();
      return;
    }
    setDir(selected);
    setItems(null);
    setMessage("");
    setBusy(true);
    try {
      setItems(await invokeCommand("anchor_record_import_scan", { roomId: anchor.uid, dir: selected }));
    } catch (err) {
      setMessage(err?.message || "扫描目录失败");
      setItems([]);
    } finally {
      setBusy(false);
    }
  };
  useEffect(() => {
    pickAndScan();
  }, []);

  const list = items || [];
  const videos = list.filter((i) => i.status !== "invalid");
  const overlaps = list.filter((i) => i.status === "overlap");
  const toImport = list.filter((i) => i.status === "new" || (i.status === "overlap" && policy !== "skip"));
  const replaceIds = policy === "replace" ? [...new Set(overlaps.flatMap((i) => i.overlaps.map((o) => o.id)))] : [];

  const commit = async () => {
    if (!toImport.length || busy) return;
    if (replaceIds.length && !(await dialogConfirm(`将删除 ${replaceIds.length} 条重叠的旧录播记录（只删记录，不删文件），删除后不可恢复。确定继续吗？`, { title: "替换旧记录" }))) return;
    setBusy(true);
    try {
      const entries = toImport.map((i) => ({ path: i.path, title: i.title, startTime: i.startTime, endTime: i.endTime, size: i.size }));
      const n = await invokeCommand("anchor_record_import_commit", { roomId: anchor.uid, entries, replaceIds });
      setMessage(`已导入 ${n} 段录播${replaceIds.length ? `，替换旧记录 ${replaceIds.length} 条` : ""}`);
      setItems(null);
      onImported?.();
    } catch (err) {
      setMessage(err?.message || "导入失败");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50" onClick={busy ? undefined : onClose}>
      <div className="flex max-h-[86vh] w-[min(980px,94vw)] flex-col rounded-2xl bg-white shadow-xl" onClick={(e) => e.stopPropagation()}>
        <div className="flex items-center justify-between border-b border-black/5 px-5 py-3">
          <div className="min-w-0">
            <div className="font-semibold text-[var(--ink)]">导入录播 · {anchor.nickname || anchor.uid}</div>
            <div className="truncate text-xs text-[var(--muted)]">{dir || "请选择录播所在目录"}</div>
          </div>
          <div className="flex gap-2">
            <button className="h-8 rounded-lg border border-black/10 px-3 text-xs" disabled={busy} onClick={pickAndScan}>
              重新选择目录
            </button>
            <button className="h-8 rounded-lg border border-black/10 px-3 text-xs" disabled={busy} onClick={onClose}>
              关闭
            </button>
          </div>
        </div>
        <div className="flex-1 space-y-3 overflow-auto p-5 text-sm">
          {busy && !items ? <div className="text-[var(--muted)]">正在扫描目录…</div> : null}
          {items ? (
            <>
              <div className="text-xs text-[var(--muted)]">
                视频 {videos.length} 段（新 {list.filter((i) => i.status === "new").length}，重叠 {overlaps.length}，已导入过 {list.filter((i) => i.status === "imported").length}）· 字幕 {videos.filter((i) => i.hasSrt).length} 份 · 弹幕 {videos.filter((i) => i.hasDanmaku).length} 份
                {list.some((i) => i.status === "invalid") ? ` · 跳过 ${list.filter((i) => i.status === "invalid").length} 个文件` : ""}
              </div>
              <table className="w-full text-xs">
                <thead className="text-left text-[var(--muted)]">
                  <tr>
                    <th className="py-1.5 pr-2 font-normal">文件</th>
                    <th className="pr-2 font-normal">开始时间</th>
                    <th className="pr-2 font-normal">时长</th>
                    <th className="pr-2 font-normal">字幕</th>
                    <th className="pr-2 font-normal">弹幕</th>
                    <th className="font-normal">状态</th>
                  </tr>
                </thead>
                <tbody>
                  {list.map((i) => (
                    <tr key={i.path} className="border-t border-black/5 align-top">
                      <td className="max-w-[360px] py-1.5 pr-2">
                        <div className="truncate text-[var(--ink)]" title={i.fileName}>{i.fileName}</div>
                        {i.reason ? <div className="text-rose-600">{i.reason}</div> : null}
                        {i.overlaps.map((o) => (
                          <div key={o.id} className="truncate text-[#9A4A0C]" title={o.filePath}>
                            重叠：#{o.id} {baseName(o.filePath)}（{o.fileExists ? "文件存在" : "文件已缺失"}）
                          </div>
                        ))}
                      </td>
                      <td className="whitespace-nowrap pr-2 tabular-nums">{fmt(i.startTime)}</td>
                      <td className="pr-2 tabular-nums">{i.duration ? dur(i.duration) : "-"}</td>
                      <td className="pr-2">{i.hasSrt ? "✓" : "—"}</td>
                      <td className="pr-2">{i.hasDanmaku ? "✓" : "—"}</td>
                      <td>
                        <span className={`rounded px-1.5 py-0.5 ${STATUS[i.status]?.[1]}`}>{STATUS[i.status]?.[0] || i.status}</span>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
              {overlaps.length ? (
                <div className="space-y-2 rounded-xl bg-[#FFF8EF] p-3">
                  <div className="font-semibold text-[#9A4A0C]">{overlaps.length} 段视频与库里已有录播时间重叠，请选择处理方式：</div>
                  {POLICIES.map(([id, label, hint]) => (
                    <label key={id} className="flex items-start gap-2">
                      <input type="radio" name="overlap-policy" checked={policy === id} onChange={() => setPolicy(id)} className="mt-0.5" />
                      <span>
                        <span className="font-semibold text-[var(--ink)]">{label}</span>
                        <span className="ml-2 text-xs text-[var(--muted)]">{hint}</span>
                      </span>
                    </label>
                  ))}
                </div>
              ) : null}
            </>
          ) : null}
          {message ? <div className="text-[var(--ink)]">{message}</div> : null}
        </div>
        {items ? (
          <div className="flex items-center justify-end gap-3 border-t border-black/5 px-5 py-3">
            <span className="mr-auto text-xs text-[var(--muted)]">时间相接的录播会自动并入同一场；只登记路径，不移动、不复制文件</span>
            <button className="h-9 rounded-lg bg-[#1F5BD8] px-5 text-sm font-semibold text-white disabled:opacity-50" disabled={busy || !toImport.length} onClick={commit}>
              {busy ? "正在导入…" : `导入 ${toImport.length} 段`}
            </button>
          </div>
        ) : null}
      </div>
    </div>
  );
}
