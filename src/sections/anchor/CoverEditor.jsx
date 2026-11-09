import { useEffect, useRef, useState } from "react";
import { invokeCommand } from "../../lib/tauri";
import { COVER_FONTS, drawCover, ensureCoverFonts, layoutBlocks, loadImage, measureBlock, splitCoverText } from "./cover";

const btn = "h-8 rounded-md border border-black/10 bg-white px-2.5 text-xs text-[var(--ink)] hover:border-black/20 disabled:opacity-40";
const row = "flex items-center justify-between gap-2 text-xs text-[var(--muted)]";

/**
 * 切片封面编辑器：画面帧 + 按空格分段的文字块。选中文字块后可拖动、拖右下角缩放、拖顶部圆点旋转，
 * 面板里改文字、颜色、描边、字体；可框选主播的人物区域（按主播保存，排版时避开）。改动自动保存。
 */
export default function CoverEditor({ anchor, clip, currentTime, onSaved }) {
  const [spec, setSpec] = useState(null);
  const [img, setImg] = useState(null);
  const [personBox, setPersonBox] = useState(null);
  const [sel, setSel] = useState(0);
  const [drawing, setDrawing] = useState(false);
  const [draft, setDraft] = useState(null);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [size, setSize] = useState({ w: 0, h: 0 });
  const frameRef = useRef(null);
  const canvasRef = useRef(null);
  const drag = useRef(null);
  const saveTimer = useRef(null);

  const save = (next) => {
    setSpec(next);
    clearTimeout(saveTimer.current);
    saveTimer.current = setTimeout(() => {
      invokeCommand("anchor_clip_cover_save", { clipId: clip.id, spec: next })
        .then(() => onSaved?.(next))
        .catch((err) => setMessage(err?.message || "保存封面失败"));
    }, 500);
  };
  const grabFrame = async (time) => {
    const frame = await invokeCommand("anchor_clip_cover_frame", { clipId: clip.id, time: time ?? null });
    setImg(await loadImage(frame.dataUrl));
    return { time: frame.time, path: frame.path };
  };

  useEffect(() => {
    let alive = true;
    setSpec(null);
    setImg(null);
    setSel(0);
    setMessage("");
    (async () => {
      try {
        await ensureCoverFonts();
        const box = await invokeCommand("anchor_cover_person_box_get", { roomId: anchor.uid }).catch(() => null);
        if (!alive) return;
        setPersonBox(box);
        if (clip.coverSpec?.frame?.path) {
          const url = await invokeCommand("anchor_clip_cover_image", { path: clip.coverSpec.frame.path });
          if (!alive) return;
          setImg(await loadImage(url));
          setSpec(clip.coverSpec);
          return;
        }
        // 第一次打开：取切片中点的画面，封面文字按空格分段排版
        const frame = await grabFrame(null);
        if (!alive) return;
        const texts = splitCoverText(clip.coverText || clip.titles?.[0] || "");
        save({ version: 1, frame, font: COVER_FONTS[0].value, blocks: layoutBlocks(texts, box) });
      } catch (err) {
        if (alive) setMessage(err?.message || "生成封面失败");
      }
    })();
    return () => {
      alive = false;
    };
  }, [clip.id]);

  useEffect(() => {
    const el = frameRef.current;
    if (!el) return undefined;
    const ro = new ResizeObserver(() => setSize({ w: el.clientWidth, h: el.clientHeight }));
    ro.observe(el);
    return () => ro.disconnect();
  }, [spec != null]);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || !spec || !size.w) return;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.round(size.w * dpr);
    canvas.height = Math.round(size.h * dpr);
    drawCover(canvas.getContext("2d"), canvas.width, canvas.height, img, spec);
  }, [spec, img, size]);

  if (!spec) {
    return <div className="py-8 text-center text-sm text-[var(--muted)]">{message || "正在生成封面…"}</div>;
  }
  const blocks = spec.blocks || [];
  const block = blocks[sel];
  const setBlock = (i, patch) => save({ ...spec, blocks: blocks.map((b, k) => (k === i ? { ...b, ...patch } : b)) });
  const pct = (ev) => {
    const r = frameRef.current.getBoundingClientRect();
    return { x: ((ev.clientX - r.left) / r.width) * 100, y: ((ev.clientY - r.top) / r.height) * 100, r };
  };

  const onPointerDown = (ev, i, mode, below = false) => {
    ev.stopPropagation();
    ev.preventDefault();
    setSel(i);
    const p = pct(ev);
    const b = blocks[i];
    const cx = p.r.left + (b.x / 100) * p.r.width;
    const cy = p.r.top + (b.y / 100) * p.r.height;
    drag.current = { mode, i, p, b, cx, cy, below, dist: Math.hypot(ev.clientX - cx, ev.clientY - cy) || 1 };
    frameRef.current.setPointerCapture(ev.pointerId);
  };
  const onFramePointerDown = (ev) => {
    if (!drawing) {
      setSel(-1);
      return;
    }
    const p = pct(ev);
    drag.current = { mode: "person", start: p };
    setDraft({ x: p.x, y: p.y, w: 0, h: 0 });
    frameRef.current.setPointerCapture(ev.pointerId);
  };
  const onPointerMove = (ev) => {
    const d = drag.current;
    if (!d) return;
    const p = pct(ev);
    if (d.mode === "move") {
      setBlock(d.i, { x: Math.min(100, Math.max(0, d.b.x + p.x - d.p.x)), y: Math.min(100, Math.max(0, d.b.y + p.y - d.p.y)) });
    } else if (d.mode === "scale") {
      const dist = Math.hypot(ev.clientX - d.cx, ev.clientY - d.cy);
      setBlock(d.i, { size: Math.round(Math.min(320, Math.max(20, (d.b.size * dist) / d.dist))) });
    } else if (d.mode === "rotate") {
      // 旋转圆点在框上方时指针正上方为 0°，在框下方时正下方为 0°
      const deg = (Math.atan2(ev.clientY - d.cy, ev.clientX - d.cx) * 180) / Math.PI + (d.below ? -90 : 90);
      setBlock(d.i, { rotate: Math.round(((deg + 540) % 360) - 180) });
    } else if (d.mode === "person") {
      const x = Math.max(0, Math.min(d.start.x, p.x));
      const y = Math.max(0, Math.min(d.start.y, p.y));
      setDraft({ x, y, w: Math.min(100, Math.max(d.start.x, p.x)) - x, h: Math.min(100, Math.max(d.start.y, p.y)) - y });
    }
  };
  const onPointerUp = async () => {
    const d = drag.current;
    drag.current = null;
    if (d?.mode !== "person" || !draft) return;
    setDrawing(false);
    setDraft(null);
    if (draft.w < 3 || draft.h < 3) return;
    const rect = { x: Math.round(draft.x * 10) / 10, y: Math.round(draft.y * 10) / 10, w: Math.round(draft.w * 10) / 10, h: Math.round(draft.h * 10) / 10 };
    try {
      await invokeCommand("anchor_cover_person_box_set", { roomId: anchor.uid, rect });
      setPersonBox(rect);
      setMessage("已保存这个主播的人物区域，点「重新排版」让文字避开它");
    } catch (err) {
      setMessage(err?.message || "保存人物区域失败");
    }
  };

  const useCurrentFrame = async () => {
    setBusy(true);
    try {
      save({ ...spec, frame: await grabFrame(currentTime) });
    } catch (err) {
      setMessage(err?.message || "截取画面失败");
    } finally {
      setBusy(false);
    }
  };
  const box = draft || personBox;

  return (
    <div className="space-y-3">
      <div
        ref={frameRef}
        className={`relative aspect-video w-full select-none overflow-hidden rounded-lg bg-black ${drawing ? "cursor-crosshair" : ""}`}
        onPointerDown={onFramePointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
      >
        <canvas ref={canvasRef} className="absolute inset-0 h-full w-full" />
        {box && (drawing || draft || personBox) ? (
          <div
            className="pointer-events-none absolute border-2 border-dashed border-amber-400 bg-amber-300/10"
            style={{ left: `${box.x}%`, top: `${box.y}%`, width: `${box.w}%`, height: `${box.h}%` }}
          >
            <span className="absolute left-1 top-1 rounded bg-amber-400 px-1 text-[10px] text-black">人物区域</span>
          </div>
        ) : null}
        {!drawing && size.w
          ? blocks.map((b, i) => {
              const m = measureBlock(spec, b, size.w, size.h);
              const active = i === sel;
              // 文字块贴近画面顶部时旋转圆点放到框下方，免得被裁掉点不到
              const below = b.y - m.h / 2 < 8;
              return (
                <div
                  key={i}
                  className={`absolute cursor-move ${active ? "outline outline-2 outline-[#1F5BD8]" : "hover:outline hover:outline-1 hover:outline-white/70"}`}
                  style={{ left: `${b.x}%`, top: `${b.y}%`, width: `${m.w}%`, height: `${m.h}%`, transform: `translate(-50%, -50%) rotate(${b.rotate || 0}deg)` }}
                  onPointerDown={(ev) => onPointerDown(ev, i, "move")}
                >
                  {active ? (
                    <>
                      <span
                        className="absolute -right-1.5 -bottom-1.5 h-3 w-3 cursor-nwse-resize rounded-sm border border-white bg-[#1F5BD8]"
                        onPointerDown={(ev) => onPointerDown(ev, i, "scale")}
                      />
                      <span
                        className={`absolute ${below ? "-bottom-6" : "-top-6"} left-1/2 h-3 w-3 -translate-x-1/2 cursor-grab rounded-full border border-white bg-[#1F5BD8]`}
                        title="拖动旋转"
                        onPointerDown={(ev) => onPointerDown(ev, i, "rotate", below)}
                      />
                    </>
                  ) : null}
                </div>
              );
            })
          : null}
      </div>

      <div className="flex flex-wrap gap-2">
        <select className="h-8 rounded-md border border-black/10 bg-white px-2 text-xs" value={spec.font} onChange={(e) => save({ ...spec, font: e.target.value })}>
          {COVER_FONTS.map((f) => (
            <option key={f.value} value={f.value}>
              {f.label}
            </option>
          ))}
        </select>
        <button
          className={btn}
          onClick={() => {
            save({ ...spec, blocks: [...blocks, { ...layoutBlocks(["新文字"], personBox)[0], y: 50 }] });
            setSel(blocks.length);
          }}
        >
          新增文字块
        </button>
        <button className={btn} onClick={() => save({ ...spec, blocks: layoutBlocks(blocks.map((b) => b.text), personBox, blocks) })}>
          重新排版
        </button>
        {currentTime != null ? (
          <button className={btn} disabled={busy} onClick={useCurrentFrame}>
            {busy ? "截取中…" : "用当前播放画面"}
          </button>
        ) : null}
        <button className={btn} onClick={() => setDrawing((v) => !v)}>
          {drawing ? "取消框选" : "框选人物区域"}
        </button>
      </div>
      {drawing ? <div className="text-xs text-amber-700">在画面上拖一个框，框住主播的立绘，按主播保存，之后排版都会避开</div> : null}

      {block ? (
        <div className="space-y-2 rounded-lg bg-black/[0.03] p-3">
          <label className={row}>
            文字
            <input className="h-8 flex-1 rounded-md border border-black/10 bg-white px-2 text-sm text-[var(--ink)]" value={block.text} onChange={(e) => setBlock(sel, { text: e.target.value })} />
          </label>
          <div className="grid grid-cols-2 gap-2">
            <label className={row}>
              文字颜色
              <input type="color" className="h-8 w-12" value={block.fill} onChange={(e) => setBlock(sel, { fill: e.target.value.toUpperCase() })} />
            </label>
            <label className={row}>
              描边颜色
              <input type="color" className="h-8 w-12" value={block.stroke} onChange={(e) => setBlock(sel, { stroke: e.target.value.toUpperCase() })} />
            </label>
            <label className={row}>
              字号 {block.size}
              <input type="range" min={20} max={320} className="w-28" value={block.size} onChange={(e) => setBlock(sel, { size: Number(e.target.value) })} />
            </label>
            <label className={row}>
              描边 {block.strokeWidth}
              <input type="range" min={0} max={30} className="w-28" value={block.strokeWidth} onChange={(e) => setBlock(sel, { strokeWidth: Number(e.target.value) })} />
            </label>
            <label className={row}>
              旋转 {block.rotate || 0}°
              <input type="range" min={-180} max={180} className="w-28" value={block.rotate || 0} onChange={(e) => setBlock(sel, { rotate: Number(e.target.value) })} />
            </label>
            <button className={btn} onClick={() => setBlock(sel, { vertical: !block.vertical })}>
              {block.vertical ? "改为横排" : "改为竖排"}
            </button>
            <button
              className="h-8 rounded-md border border-[#E5B4AE] bg-white px-2.5 text-xs text-[#B42318]"
              onClick={() => {
                save({ ...spec, blocks: blocks.filter((_, k) => k !== sel) });
                setSel(-1);
              }}
            >
              删除这个文字块
            </button>
          </div>
        </div>
      ) : (
        <div className="text-xs text-[var(--muted)]">点画面上的文字块选中后修改：拖动移动，拖右下角方块缩放，拖蓝色圆点旋转，面板里可改竖排</div>
      )}
      {message ? <div className="text-xs text-[var(--ink)]">{message}</div> : null}
    </div>
  );
}
