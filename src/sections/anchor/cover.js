import { invokeCommand } from "../../lib/tauri";

// 切片封面：只保存画面帧和各文字块的参数，投稿时按参数渲染成图片。
// 坐标是文字块中心占画面宽高的百分比；字号、描边按 1080p 画面的像素记，渲染时按实际高度缩放。
export const COVER_FONTS = [
  { value: "sourcehan", label: "思源黑体 Heavy", family: "CoverSourceHan" },
  { value: "puhuiti", label: "阿里巴巴普惠体 Heavy", family: "CoverPuHuiTi" },
];
const STYLES = [
  { fill: "#3BB54A", stroke: "#FFFFFF" },
  { fill: "#FFFFFF", stroke: "#1F6B3A" },
];
// 竖排：逐字从上往下排，字距为字号的 1.05 倍；句读符号挪到字格右上角
const V_LINE = 1.05;
const V_PUNCT = "，。、．,.";
const fontOf = (spec) => (COVER_FONTS.find((f) => f.value === spec.font) || COVER_FONTS[0]).family;

export async function ensureCoverFonts() {
  await Promise.all(COVER_FONTS.map((f) => document.fonts.load(`900 100px "${f.family}"`).catch(() => null)));
}

/** 封面文字按空格分段，每段一个文字块 */
export const splitCoverText = (text) => String(text || "").split(/\s+/).map((t) => t.trim()).filter(Boolean);

/**
 * 默认排版：文字块依次竖排在人物区域另一侧（没标人物区域时放在左侧），两种配色交替
 * personBox 为 {x, y, w, h}（百分比）
 */
export function layoutBlocks(texts, personBox, prev = []) {
  const box = personBox || { x: 55, y: 0, w: 45, h: 100 };
  const leftFree = box.x;
  const rightFree = 100 - box.x - box.w;
  const [from, to] = leftFree >= rightFree ? [2, Math.max(2, box.x - 2)] : [box.x + box.w + 2, 98];
  const n = Math.max(1, texts.length);
  const gap = 84 / n;
  return texts.map((text, i) => ({
    text,
    x: (from + to) / 2,
    y: 10 + gap * (i + 0.5),
    size: 96,
    rotate: 0,
    strokeWidth: 10,
    ...STYLES[i % 2],
    ...(prev[i] ? { fill: prev[i].fill, stroke: prev[i].stroke, strokeWidth: prev[i].strokeWidth } : {}),
  }));
}

/** 在画布上按参数画出封面（图片已加载） */
export function drawCover(ctx, width, height, img, spec) {
  ctx.clearRect(0, 0, width, height);
  if (img) {
    // 按画面比例铺满，多出的部分居中裁掉
    const scale = Math.max(width / img.width, height / img.height);
    const w = img.width * scale;
    const h = img.height * scale;
    ctx.drawImage(img, (width - w) / 2, (height - h) / 2, w, h);
  }
  const k = height / 1080;
  const family = fontOf(spec);
  for (const b of spec.blocks || []) {
    ctx.save();
    ctx.translate((b.x / 100) * width, (b.y / 100) * height);
    ctx.rotate(((b.rotate || 0) * Math.PI) / 180);
    ctx.font = `900 ${b.size * k}px "${family}"`;
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    ctx.lineJoin = "round";
    const size = b.size * k;
    const chars = b.vertical ? Array.from(b.text) : [b.text];
    const step = size * V_LINE;
    chars.forEach((ch, i) => {
      const punct = b.vertical && V_PUNCT.includes(ch);
      const x = punct ? size * 0.3 : 0;
      const y = b.vertical ? (i - (chars.length - 1) / 2) * step - (punct ? size * 0.3 : 0) : 0;
      if (b.strokeWidth > 0) {
        ctx.strokeStyle = b.stroke;
        ctx.lineWidth = b.strokeWidth * 2 * k;
        ctx.strokeText(ch, x, y);
      }
      ctx.fillStyle = b.fill;
      ctx.fillText(ch, x, y);
    });
    ctx.restore();
  }
}

/** 文字块在画面上占的宽高（百分比，未旋转），用于编辑器的选中框 */
export function measureBlock(spec, block, width, height) {
  const ctx = measureBlock.ctx || (measureBlock.ctx = document.createElement("canvas").getContext("2d"));
  const k = height / 1080;
  ctx.font = `900 ${block.size * k}px "${fontOf(spec)}"`;
  const pad = block.strokeWidth * 2 * k;
  const size = block.size * k;
  const n = Array.from(block.text).length;
  const w = (block.vertical ? size * 1.1 : ctx.measureText(block.text).width) + pad;
  const h = (block.vertical ? n * size * V_LINE : size * 1.2) + pad;
  return { w: (w / width) * 100, h: (h / height) * 100 };
}

export const loadImage = (src) =>
  new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => resolve(img);
    img.onerror = () => reject(new Error("加载封面画面失败"));
    img.src = src;
  });

/** 投稿用：按参数渲染 1920×1080 的 JPG（data URL） */
export async function renderCoverDataUrl(spec) {
  await ensureCoverFonts();
  const img = await loadImage(await invokeCommand("anchor_clip_cover_image", { path: spec.frame.path }));
  const canvas = document.createElement("canvas");
  canvas.width = 1920;
  canvas.height = 1080;
  drawCover(canvas.getContext("2d"), canvas.width, canvas.height, img, spec);
  return canvas.toDataURL("image/jpeg", 0.92);
}
