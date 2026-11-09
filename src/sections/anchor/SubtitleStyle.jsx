import { useEffect, useRef, useState } from "react";
import { invokeCommand } from "../../lib/tauri";

// 字幕样式：尺寸类数值都按 1080p 画面的像素记，显示时按实际画面高度等比缩放；
// 位置是字幕框中心点占画面宽高的百分比，boxWidth 是字幕框宽度占画面宽度的百分比（超出自动换行）
export const DEFAULT_SUBTITLE_STYLE = {
  name: "默认",
  fontFamily: "PingFang SC, Microsoft YaHei, sans-serif",
  fontSize: 54,
  color: "#FFFFFF",
  outlineColor: "#000000",
  outlineWidth: 3,
  shadow: 2,
  bold: true,
  italic: false,
  bgColor: "#000000",
  bgOpacity: 0,
  x: 50,
  y: 82,
  boxWidth: 80,
};

/** ASS 颜色 &HAABBGGRR（A 为透明度，00 不透明）转 {hex, alpha} */
function assColor(value) {
  const hex = String(value || "").replace(/^&H/i, "").replace(/&$/, "").padStart(8, "0").slice(-8);
  const a = parseInt(hex.slice(0, 2), 16);
  const [b, g, r] = [hex.slice(2, 4), hex.slice(4, 6), hex.slice(6, 8)];
  return { hex: `#${r}${g}${b}`.toUpperCase(), alpha: Number.isFinite(a) ? 1 - a / 255 : 1 };
}

/** 解析 .ass 文件里的 [V4+ Styles]，换算成本页使用的样式 */
export function parseAssStyles(text) {
  const lines = String(text || "").split(/\r?\n/);
  let playResY = 288;
  let section = "";
  let format = [];
  const styles = [];
  for (const raw of lines) {
    const line = raw.trim();
    if (/^\[.*\]$/.test(line)) {
      section = line.toLowerCase();
      continue;
    }
    if (section === "[script info]" && /^PlayResY\s*:/i.test(line)) {
      playResY = Number(line.split(":")[1]) || playResY;
    }
    if (!section.includes("styles")) continue;
    if (/^Format\s*:/i.test(line)) {
      format = line.slice(line.indexOf(":") + 1).split(",").map((f) => f.trim().toLowerCase());
    } else if (/^Style\s*:/i.test(line) && format.length) {
      const values = line.slice(line.indexOf(":") + 1).split(",");
      // 最后一列之后的逗号属于该列内容，这里各列都是数值或名称，按格式列数对齐即可
      const get = (key) => (values[format.indexOf(key)] ?? "").trim();
      styles.push({ get, playResY });
    }
  }
  return styles.map(({ get, playResY: resY }) => {
    const k = 1080 / resY;
    const fontSize = (Number(get("fontsize")) || 20) * k;
    const primary = assColor(get("primarycolour"));
    const outline = assColor(get("outlinecolour"));
    const back = assColor(get("backcolour"));
    const boxed = get("borderstyle") === "3";
    const align = Number(get("alignment")) || 2;
    const marginV = (Number(get("marginv")) || 10) * k;
    const marginL = (Number(get("marginl")) || 10) * k;
    const marginR = (Number(get("marginr")) || 10) * k;
    // 小键盘方位：1-3 底部、4-6 中间、7-9 顶部；余数 1 靠左、2 居中、0 靠右
    const col = align % 3;
    const x = col === 1 ? ((marginL + 400) / 1920) * 100 : col === 0 ? 100 - ((marginR + 400) / 1920) * 100 : 50;
    const y = align <= 3 ? 100 - ((marginV + fontSize / 2) / 1080) * 100 : align >= 7 ? ((marginV + fontSize / 2) / 1080) * 100 : 50;
    return {
      name: get("name") || "导入样式",
      fontFamily: `${get("fontname") || "sans-serif"}, PingFang SC, Microsoft YaHei, sans-serif`,
      fontSize: Math.round(fontSize),
      color: primary.hex,
      outlineColor: outline.hex,
      outlineWidth: boxed ? 0 : Math.round((Number(get("outline")) || 0) * k * 10) / 10,
      shadow: Math.round((Number(get("shadow")) || 0) * k * 10) / 10,
      bold: get("bold") === "-1" || get("bold") === "1",
      italic: get("italic") === "-1" || get("italic") === "1",
      bgColor: boxed ? back.hex : "#000000",
      bgOpacity: boxed ? Math.round(back.alpha * 100) / 100 : 0,
      x: Math.round(x * 10) / 10,
      y: Math.round(y * 10) / 10,
      boxWidth: 80,
    };
  });
}

const rgba = (hex, alpha) => {
  const h = String(hex || "#000000").replace("#", "");
  const n = parseInt(h.length === 3 ? h.split("").map((c) => c + c).join("") : h, 16);
  return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${alpha})`;
};

/** 读取、实时保存字幕样式（改动约半秒后写入） */
export function useSubtitleStyle() {
  const [style, setStyle] = useState(DEFAULT_SUBTITLE_STYLE);
  const timer = useRef(null);
  useEffect(() => {
    invokeCommand("clip_subtitle_style_get")
      .then((saved) => saved && setStyle({ ...DEFAULT_SUBTITLE_STYLE, ...saved }))
      .catch(() => {});
    return () => clearTimeout(timer.current);
  }, []);
  const update = (patch) => {
    setStyle((prev) => {
      const next = { ...prev, ...patch };
      clearTimeout(timer.current);
      timer.current = setTimeout(() => invokeCommand("clip_subtitle_style_set", { style: next }).catch(() => {}), 500);
      return next;
    });
  };
  return [style, update];
}

/** 画面上的字幕条：按样式渲染，可拖动位置；拖右下角只改字幕框宽度，不改字号 */
export function SubtitleOverlay({ text, style, onChange, frameRef }) {
  const [frameH, setFrameH] = useState(0);
  const drag = useRef(null);
  useEffect(() => {
    const el = frameRef.current;
    if (!el) return undefined;
    const ro = new ResizeObserver(() => setFrameH(el.clientHeight));
    ro.observe(el);
    setFrameH(el.clientHeight);
    return () => ro.disconnect();
  }, [frameRef]);
  if (!text || !frameH) return null;
  const k = frameH / 1080;
  const ow = (style.outlineWidth || 0) * k;
  const sh = (style.shadow || 0) * k;
  const shadows = [];
  if (ow > 0) {
    // 用多方向文字阴影模拟描边，比 text-stroke 更接近 ASS 的外描边效果
    for (let a = 0; a < 360; a += 30) {
      const r = (a * Math.PI) / 180;
      shadows.push(`${(Math.cos(r) * ow).toFixed(2)}px ${(Math.sin(r) * ow).toFixed(2)}px 0 ${style.outlineColor}`);
    }
  }
  if (sh > 0) shadows.push(`${sh}px ${sh}px ${Math.max(1, sh)}px rgba(0,0,0,0.6)`);

  const onPointerDown = (ev, mode) => {
    ev.preventDefault();
    ev.stopPropagation();
    const frame = frameRef.current.getBoundingClientRect();
    drag.current = { mode, startX: ev.clientX, startY: ev.clientY, x: style.x, y: style.y, boxWidth: style.boxWidth || 80, frame };
    ev.currentTarget.setPointerCapture(ev.pointerId);
  };
  const onPointerMove = (ev) => {
    const d = drag.current;
    if (!d) return;
    const dx = ev.clientX - d.startX;
    const dy = ev.clientY - d.startY;
    if (d.mode === "move") {
      onChange({
        x: Math.min(98, Math.max(2, d.x + (dx / d.frame.width) * 100)),
        y: Math.min(98, Math.max(2, d.y + (dy / d.frame.height) * 100)),
      });
    } else {
      // 字幕框以中心定位，右边拖出 dx，宽度增加 2 倍 dx
      onChange({ boxWidth: Math.min(100, Math.max(15, d.boxWidth + ((2 * dx) / d.frame.width) * 100)) });
    }
  };
  const onPointerUp = () => {
    drag.current = null;
  };

  return (
    <div
      role="presentation"
      onPointerDown={(ev) => onPointerDown(ev, "move")}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onClick={(ev) => ev.stopPropagation()}
      className="group absolute z-10 cursor-move select-none rounded text-center outline-1 outline-dashed outline-transparent hover:outline-white/60"
      style={{ left: `${style.x}%`, top: `${style.y}%`, width: `${style.boxWidth || 80}%`, transform: "translate(-50%, -50%)" }}
      title="拖动调整位置，拖右下角调整字幕框宽度（不改字号）"
    >
      <span
        className="inline whitespace-pre-wrap leading-snug [box-decoration-break:clone]"
        style={{
          fontFamily: style.fontFamily,
          fontSize: `${style.fontSize * k}px`,
          color: style.color,
          fontWeight: style.bold ? 700 : 400,
          fontStyle: style.italic ? "italic" : "normal",
          textShadow: shadows.join(", "),
          background: style.bgOpacity > 0 ? rgba(style.bgColor, style.bgOpacity) : "transparent",
          padding: style.bgOpacity > 0 ? `${0.15 * style.fontSize * k}px ${0.4 * style.fontSize * k}px` : 0,
          borderRadius: style.bgOpacity > 0 ? 4 : 0,
        }}
      >
        {text}
      </span>
      <span
        role="presentation"
        onPointerDown={(ev) => onPointerDown(ev, "resize")}
        className="absolute -bottom-1.5 -right-1.5 hidden h-3 w-3 cursor-nwse-resize rounded-sm border border-white bg-[#1F5BD8] group-hover:block"
      />
    </div>
  );
}

/** 字幕样式设置面板：手动调整，或导入 .ass 文件里的样式 */
export function SubtitleStylePanel({ style, onChange, onClose }) {
  const [imported, setImported] = useState([]);
  const [error, setError] = useState("");
  const fileRef = useRef(null);
  const row = "flex items-center justify-between gap-3 text-xs text-[var(--muted)]";
  const input = "h-8 rounded-md border border-black/10 bg-white px-2 text-xs text-[var(--ink)]";
  const importFile = (file) => {
    if (!file) return;
    const reader = new FileReader();
    reader.onload = () => {
      const styles = parseAssStyles(reader.result);
      if (!styles.length) {
        setError("文件里没有找到 [V4+ Styles] 样式定义");
        return;
      }
      setError("");
      setImported(styles);
      onChange(styles[0]);
    };
    reader.readAsText(file);
  };
  return (
    <div className="absolute right-3 top-3 z-20 w-72 space-y-2.5 rounded-xl bg-white p-3.5 shadow-xl ring-1 ring-black/10" onClick={(ev) => ev.stopPropagation()}>
      <div className="flex items-center justify-between">
        <span className="text-sm font-semibold text-[var(--ink)]">字幕样式</span>
        <button className="text-xs text-[var(--muted)] hover:underline" onClick={onClose}>
          收起
        </button>
      </div>
      <div className="flex gap-2">
        <button className="h-8 flex-1 rounded-md border border-black/10 bg-white text-xs hover:border-black/20" onClick={() => fileRef.current?.click()}>
          导入 .ass 样式
        </button>
        <button
          className="h-8 rounded-md border border-black/10 bg-white px-2.5 text-xs hover:border-black/20"
          onClick={() => onChange({ ...DEFAULT_SUBTITLE_STYLE })}
        >
          恢复默认
        </button>
        <input
          ref={fileRef}
          type="file"
          accept=".ass,.ssa"
          className="hidden"
          onChange={(ev) => {
            importFile(ev.target.files?.[0]);
            ev.target.value = "";
          }}
        />
      </div>
      {error ? <div className="text-xs text-red-600">{error}</div> : null}
      {imported.length > 1 ? (
        <label className={row}>
          样式
          <select className={`${input} flex-1`} onChange={(ev) => onChange(imported[Number(ev.target.value)])}>
            {imported.map((s, i) => (
              <option key={i} value={i}>
                {s.name}
              </option>
            ))}
          </select>
        </label>
      ) : null}
      <label className={row}>
        字体
        <input className={`${input} w-40`} value={style.fontFamily} onChange={(ev) => onChange({ fontFamily: ev.target.value })} />
      </label>
      <div className="space-y-1 rounded-lg bg-black/[0.03] p-2">
        <label className={row}>
          字号（1080p 像素）
          <input type="number" min={12} max={200} className={`${input} w-20`} value={style.fontSize} onChange={(ev) => onChange({ fontSize: Math.min(200, Math.max(12, Number(ev.target.value) || 12)) })} />
        </label>
        <input type="range" aria-label="字号" min={20} max={160} step={1} className="w-full" value={style.fontSize} onChange={(ev) => onChange({ fontSize: Number(ev.target.value) })} />
      </div>
      <label className={row}>
        字幕框宽度（占画面）
        <span className="flex items-center gap-2">
          <input type="range" min={15} max={100} step={1} className="w-24" value={Math.round(style.boxWidth || 80)} onChange={(ev) => onChange({ boxWidth: Number(ev.target.value) })} />
          <span className="w-9 text-right tabular-nums text-[var(--ink)]">{Math.round(style.boxWidth || 80)}%</span>
        </span>
      </label>
      <label className={row}>
        文字颜色
        <input type="color" className="h-8 w-12" value={style.color} onChange={(ev) => onChange({ color: ev.target.value.toUpperCase() })} />
      </label>
      <label className={row}>
        描边
        <span className="flex items-center gap-2">
          <input type="color" className="h-8 w-12" value={style.outlineColor} onChange={(ev) => onChange({ outlineColor: ev.target.value.toUpperCase() })} />
          <input type="number" min={0} max={20} step={0.5} className={`${input} w-16`} value={style.outlineWidth} onChange={(ev) => onChange({ outlineWidth: Number(ev.target.value) || 0 })} />
        </span>
      </label>
      <label className={row}>
        阴影
        <input type="number" min={0} max={20} step={0.5} className={`${input} w-16`} value={style.shadow} onChange={(ev) => onChange({ shadow: Number(ev.target.value) || 0 })} />
      </label>
      <label className={row}>
        背景条
        <span className="flex items-center gap-2">
          <input type="color" className="h-8 w-12" value={style.bgColor} onChange={(ev) => onChange({ bgColor: ev.target.value.toUpperCase() })} />
          <input type="range" min={0} max={1} step={0.05} className="w-20" value={style.bgOpacity} onChange={(ev) => onChange({ bgOpacity: Number(ev.target.value) })} />
        </span>
      </label>
      <div className={row}>
        字形
        <span className="flex gap-3 text-[var(--ink)]">
          <label className="flex items-center gap-1">
            <input type="checkbox" checked={!!style.bold} onChange={(ev) => onChange({ bold: ev.target.checked })} />
            粗体
          </label>
          <label className="flex items-center gap-1">
            <input type="checkbox" checked={!!style.italic} onChange={(ev) => onChange({ italic: ev.target.checked })} />
            斜体
          </label>
        </span>
      </div>
      <div className="text-[11px] leading-relaxed text-[var(--muted)]">在画面上拖动字幕调整位置，拖右下角调整字幕框宽度（字号不变，过长自动换行）；所有改动自动保存。当前为 {style.name || "自定义"}。</div>
    </div>
  );
}
