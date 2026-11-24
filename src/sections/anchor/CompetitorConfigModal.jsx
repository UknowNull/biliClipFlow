import { useEffect, useRef, useState } from "react";

import { invokeCommand } from "../../lib/tauri";

/**
 * 竞品分析配置弹窗：配置竞品 UP 主，以及每个 UP 独立的标题/标签过滤规则。
 * 配置只属于当前主播（roomId），不同主播互不影响。
 */
export default function CompetitorConfigModal({ roomId, roomName, onClose }) {
  const [ups, setUps] = useState([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState("");
  const [newUid, setNewUid] = useState("");
  // 记录打开时的配置，用于判断哪些 UP 被移除了（需要调删除接口）
  const originalRef = useRef([]);

  const load = async () => {
    setLoading(true);
    try {
      const list = await invokeCommand("competitor_up_list", { roomId });
      originalRef.current = (list || []).map((item) => ({
        id: item.id,
        upUid: item.upUid,
      }));
      setUps(
        (list || []).map((item) => ({
          ...item,
          collapsed: true,
          // 编辑态：关键字先转成数组副本，避免直接改动后端返回值
          titleKeywords: [...(item.titleKeywords || [])],
          tagKeywords: [...(item.tagKeywords || [])],
        })),
      );
    } catch (error) {
      setMessage(error?.message || "读取竞品配置失败");
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [roomId]);

  const patchUp = (index, patch) => {
    setUps((prev) => prev.map((item, i) => (i === index ? { ...item, ...patch } : item)));
  };

  const addUp = () => {
    const uid = newUid.trim();
    if (!uid) {
      setMessage("请输入竞品 UP 主的 UID 或空间链接");
      return;
    }
    // 支持直接粘贴空间链接，从中抽出数字 UID
    const matched = uid.match(/(\d{5,})/);
    const normalized = matched ? matched[1] : uid;
    if (ups.some((item) => item.upUid === normalized)) {
      setMessage("该 UP 主已添加");
      return;
    }
    setUps((prev) => [
      ...prev.map((item) => ({ ...item, collapsed: true })),
      {
        id: 0,
        roomId,
        upUid: normalized,
        upName: "",
        relation: "OR",
        titleKeywords: [],
        tagKeywords: [],
        enabled: true,
        enabledSince: "",
        lastScanAt: "",
        collapsed: false,
      },
    ]);
    setNewUid("");
    setMessage("");
  };

  const removeUp = (index) => {
    setUps((prev) => prev.filter((_, i) => i !== index));
  };

  const addKeyword = (index, field, value) => {
    const text = String(value || "").trim();
    if (!text) return;
    const up = ups[index];
    if (up[field].includes(text)) return;
    patchUp(index, { [field]: [...up[field], text] });
  };

  const removeKeyword = (index, field, keyword) => {
    const up = ups[index];
    patchUp(index, { [field]: up[field].filter((item) => item !== keyword) });
  };

  const saveAll = async () => {
    if (saving) return;
    // 已存在的 UP 走更新，新加的走新增
    setSaving(true);
    setMessage("");
    try {
      for (const up of ups) {
        await invokeCommand("competitor_up_save", {
          input: {
            roomId,
            upUid: up.upUid,
            upName: up.upName || "",
            relation: up.relation,
            titleKeywords: up.titleKeywords,
            tagKeywords: up.tagKeywords,
            enabled: up.enabled,
          },
        });
      }
      // 删除本次编辑中移除的旧配置
      const keptUids = new Set(ups.map((item) => item.upUid));
      for (const original of originalRef.current) {
        if (original.id > 0 && !keptUids.has(original.upUid)) {
          await invokeCommand("competitor_up_delete", { roomId, id: original.id });
        }
      }
      setMessage(`已保存 ${ups.length} 个竞品 UP 主`);
      await load();
    } catch (error) {
      setMessage(error?.message || "保存失败");
    } finally {
      setSaving(false);
    }
  };


  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-6"
      onClick={onClose}
    >
      <div
        className="flex max-h-full w-[620px] flex-col rounded-2xl bg-white shadow-xl"
        onClick={(event) => event.stopPropagation()}
      >
        <div className="flex items-start gap-3 border-b border-black/5 px-5 py-4">
          <div className="min-w-0 flex-1">
            <div className="text-base font-semibold text-[var(--ink)]">竞品分析配置</div>
            <div className="mt-0.5 text-xs text-[var(--muted)]">
              针对「{roomName || roomId}」· 每个竞品 UP 独立配置过滤规则
            </div>
          </div>
          <button
            className="h-8 w-8 rounded-lg text-[var(--muted)] hover:bg-black/5"
            onClick={onClose}
            aria-label="关闭"
          >
            ×
          </button>
        </div>

        <div className="flex-1 overflow-y-auto px-5 py-4">
          <div className="mb-2 flex items-center gap-2">
            <span className="text-xs font-semibold text-[var(--ink)]">竞品 UP 主</span>
            <span className="text-[11px] text-[var(--muted)]">点击卡片展开各自的过滤规则</span>
          </div>

          {loading ? (
            <div className="py-6 text-center text-xs text-[var(--muted)]">正在读取配置…</div>
          ) : ups.length === 0 ? (
            <div className="rounded-xl border border-dashed border-black/15 py-6 text-center text-xs text-[var(--muted)]">
              还没有配置竞品 UP 主
            </div>
          ) : (
            <div className="space-y-2.5">
              {ups.map((up, index) => (
                <div
                  key={`${up.upUid}-${index}`}
                  className="overflow-hidden rounded-xl border border-black/10 bg-white"
                >
                  <div
                    className="flex cursor-pointer items-center gap-2.5 bg-[#fafcfd] px-3 py-2.5"
                    onClick={() => patchUp(index, { collapsed: !up.collapsed })}
                  >
                    <span className="flex h-7 w-7 flex-none items-center justify-center rounded-full bg-[#d8e6ee] text-[11px] font-bold text-[#4a7c96]">
                      {(up.upName || up.upUid).slice(0, 1)}
                    </span>
                    <div className="min-w-0 flex-1">
                      <div className="truncate text-xs font-semibold text-[var(--ink)]">
                        {up.upName || "待识别昵称"}
                      </div>
                      <div className="text-[11px] text-[var(--muted)]">UID {up.upUid}</div>
                    </div>
                    <span className="rounded bg-[rgba(0,161,214,.1)] px-1.5 py-0.5 text-[10.5px] font-semibold text-[#0b6d90]">
                      标题 {up.titleKeywords.length}
                    </span>
                    <span className="rounded bg-[rgba(0,161,214,.1)] px-1.5 py-0.5 text-[10.5px] font-semibold text-[#0b6d90]">
                      标签 {up.tagKeywords.length}
                    </span>
                    <span
                      className={`rounded px-1.5 py-0.5 text-[10.5px] font-semibold ${
                        up.relation === "AND"
                          ? "bg-[#eef3ff] text-[#1f4fb8]"
                          : "bg-[#fff3e6] text-[#9a4a0c]"
                      }`}
                    >
                      {up.relation === "AND" ? "与" : "或"}
                    </span>
                    <button
                      className="h-6 w-6 rounded text-[var(--muted)] hover:bg-[rgba(180,35,24,.1)] hover:text-[#b42318]"
                      onClick={(event) => {
                        event.stopPropagation();
                        removeUp(index);
                      }}
                      aria-label="移除"
                    >
                      ×
                    </button>
                    <span className="text-[11px] text-[var(--muted)]">
                      {up.collapsed ? "▸" : "▾"}
                    </span>
                  </div>

                  {up.collapsed ? null : (
                    <div className="space-y-3 border-t border-black/5 p-3">
                      <div className="flex flex-wrap items-center gap-2 rounded-lg bg-[#fafbfc] px-2.5 py-2">
                        <span className="text-[11.5px] font-semibold text-[var(--ink)]">匹配关系</span>
                        <span className="text-[11px] text-[var(--muted)]">标题与标签之间</span>
                        <div className="flex-1" />
                        <div className="inline-flex gap-0.5 rounded-md bg-black/[0.055] p-0.5">
                          {[
                            ["OR", "或（任一命中）"],
                            ["AND", "与（两者都命中）"],
                          ].map(([value, label]) => (
                            <button
                              key={value}
                              className={`rounded px-2.5 py-1 text-[11.5px] font-semibold ${
                                up.relation === value
                                  ? "bg-white shadow-sm"
                                  : "text-[var(--muted)]"
                              } ${up.relation === value ? (value === "AND" ? "text-[#1f4fb8]" : "text-[#9a4a0c]") : ""}`}
                              onClick={() => patchUp(index, { relation: value })}
                            >
                              {label}
                            </button>
                          ))}
                        </div>
                      </div>

                      <KeywordEditor
                        label="标题关键字"
                        hint="命中任一即满足"
                        keywords={up.titleKeywords}
                        onAdd={(value) => addKeyword(index, "titleKeywords", value)}
                        onRemove={(keyword) => removeKeyword(index, "titleKeywords", keyword)}
                      />
                      <KeywordEditor
                        label="标签关键字"
                        hint="命中任一即满足"
                        keywords={up.tagKeywords}
                        onAdd={(value) => addKeyword(index, "tagKeywords", value)}
                        onRemove={(keyword) => removeKeyword(index, "tagKeywords", keyword)}
                      />
                    </div>
                  )}
                </div>
              ))}
            </div>
          )}

          <div className="mt-2.5 flex items-center gap-2">
            <input
              className="h-9 flex-1 rounded-lg border border-black/10 bg-white px-3 text-xs text-[var(--ink)] focus:border-[#00A1D6] focus:outline-none"
              placeholder="输入竞品 UP 主的 UID 或空间链接"
              value={newUid}
              onChange={(event) => setNewUid(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") addUp();
              }}
            />
            <button
              className="h-9 rounded-lg bg-[#1F5BD8] px-3 text-xs font-semibold text-white hover:brightness-110"
              onClick={addUp}
            >
              添加
            </button>
          </div>
          <div className="mt-1.5 text-[11.5px] text-[var(--muted)]">
            添加后自动抓取该 UP 的新投稿，首次配置只回溯当天的稿件。
          </div>

          <div className="mt-4">
            <div className="mb-1.5 text-xs font-semibold text-[var(--ink)]">归属规则</div>
            <div className="rounded-lg border border-black/10 bg-[#fafbfc] px-3 py-2.5 text-xs leading-relaxed text-[var(--muted)]">
              竞品投稿时间
              <span className="mx-1 font-semibold text-[var(--ink)]">之前、最近结束</span>
              的那一场 → 自动归属。
              <br />
              归属可在对比页手动修改，修改后原报告作废并重新对比。
            </div>
          </div>
        </div>

        <div className="flex items-center gap-2 border-t border-black/5 px-5 py-3">
          {message ? (
            <span className="text-[11.5px] text-[#9a4a0c]">{message}</span>
          ) : null}
          <div className="flex-1" />
          <button
            className="h-9 rounded-lg border border-black/10 bg-white px-4 text-sm text-[var(--ink)] hover:border-black/20"
            onClick={onClose}
          >
            取消
          </button>
          <button
            className="h-9 rounded-lg bg-[#1F5BD8] px-4 text-sm font-semibold text-white disabled:opacity-50"
            disabled={saving}
            onClick={saveAll}
          >
            {saving ? "保存中…" : "保存配置"}
          </button>
        </div>
      </div>
    </div>
  );
}

/** 关键字编辑：回车添加，点击 × 删除 */
function KeywordEditor({ label, hint, keywords, onAdd, onRemove }) {
  const [draft, setDraft] = useState("");
  return (
    <div>
      <div className="mb-1.5 flex items-center gap-1.5">
        <span className="text-[11.5px] font-semibold text-[var(--ink)]">{label}</span>
        <span className="text-[11px] text-[var(--muted)]">{hint}</span>
      </div>
      <div className="flex min-h-[36px] flex-wrap items-center gap-1.5 rounded-lg border border-black/10 bg-white p-1.5 focus-within:border-[#00A1D6]">
        {keywords.map((keyword) => (
          <span
            key={keyword}
            className="inline-flex items-center gap-1 rounded bg-[rgba(0,161,214,.1)] py-0.5 pl-2 pr-1 text-[11.5px] font-medium text-[#0b6d90]"
          >
            {keyword}
            <button
              className="h-4 w-4 rounded text-[10px] opacity-55 hover:bg-black/10 hover:opacity-100"
              onClick={() => onRemove(keyword)}
              aria-label={`删除 ${keyword}`}
            >
              ×
            </button>
          </span>
        ))}
        <input
          className="min-w-[100px] flex-1 bg-transparent text-xs text-[var(--ink)] focus:outline-none"
          placeholder="输入后回车添加"
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={(event) => {
            if (event.key !== "Enter") return;
            event.preventDefault();
            onAdd(draft);
            setDraft("");
          }}
        />
      </div>
    </div>
  );
}
