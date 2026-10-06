import { type Annot, type FormWidget, type PageInfo, type Quad, type PageText, type Tile, TILE_SIZE } from "./contract";

/** Placeholder page drawing shared by MockHost and the standalone MockTileSource. */

const WORDS = (
  "papyrine document viewer renders pages quickly and keeps memory small while the engine " +
  "edits objects safely signatures forms annotations outline thumbnails search index scroll zoom " +
  "tile cache journal recovery lightweight portable format reader writer layout column heading"
).split(" ");

function mulberry(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const LINES = 34;
export const LINE_X = 72;
export const LINE_Y0 = 150;
export const LINE_STEP = 17;
export const FONT_PT = 10;

/** Deterministic text for a source page. */
export function mockLines(src: number): string[] {
  if (src < 0) return [];
  const rnd = mulberry(src * 7919 + 13);
  const lines: string[] = [];
  for (let i = 0; i < LINES; i++) {
    const n = 6 + Math.floor(rnd() * 6);
    const words: string[] = [];
    for (let k = 0; k < n; k++) words.push(WORDS[Math.floor(rnd() * WORDS.length)] ?? "x");
    lines.push(words.join(" "));
  }
  // Fixed text with diacritics and a ligature word so accent-insensitive search is testable.
  if (src === 2) lines[5] = "Crème brûlée café résumé naïve office";
  return lines;
}

export function lineWidth(text: string): number {
  return text.length * FONT_PT * 0.5;
}

export function mockPageText(src: number, page: number): PageText {
  return {
    page,
    runs: mockLines(src).map((text, i) => ({
      text,
      x: LINE_X,
      y: LINE_Y0 + i * LINE_STEP,
      width: lineWidth(text),
      height: FONT_PT + 2,
    })),
  };
}

type Ctx = CanvasRenderingContext2D | OffscreenCanvasRenderingContext2D;

function makeCanvas(w: number, h: number): OffscreenCanvas | HTMLCanvasElement {
  if (typeof OffscreenCanvas !== "undefined") return new OffscreenCanvas(w, h);
  const c = document.createElement("canvas");
  c.width = w;
  c.height = h;
  return c;
}

/** Draw one tile of a placeholder page. `src` picks the text; `position` is the page's index in the doc. */
export interface PaintExtras {
  /** Annotations on this page (the host would render them into tiles). */
  annots: Annot[];
  /** Form widgets on this page, drawn with their displayed values. */
  widgets: FormWidget[];
}

export async function renderMockTile(
  info: PageInfo,
  src: number,
  position: number,
  scale: number,
  tx: number,
  ty: number,
  extras?: PaintExtras,
): Promise<Tile> {
  const fullW = Math.ceil(info.width * scale);
  const fullH = Math.ceil(info.height * scale);
  const x0 = tx * TILE_SIZE;
  const y0 = ty * TILE_SIZE;
  const w = Math.max(1, Math.min(TILE_SIZE, fullW - x0));
  const h = Math.max(1, Math.min(TILE_SIZE, fullH - y0));
  const canvas = makeCanvas(w, h);
  const ctx = canvas.getContext("2d") as Ctx;
  paint(ctx, info, src, position, scale, x0, y0, w, h, extras);
  const bitmap = await createImageBitmap(canvas);
  return { bitmap, width: w, height: h };
}

function paint(ctx: Ctx, info: PageInfo, src: number, position: number, scale: number, x0: number, y0: number, w: number, h: number, extras?: PaintExtras): void {
  // Page content is never mirrored, whatever the UI direction is.
  ctx.direction = "ltr";
  ctx.textAlign = "left";
  ctx.fillStyle = "#ffffff";
  ctx.fillRect(0, 0, w, h);
  ctx.save();
  ctx.translate(-x0, -y0);
  ctx.scale(scale, scale);
  if (src >= 0) {
    // Faint grid so zoom and scroll artefacts are visible.
    ctx.strokeStyle = "#d8dce6";
    ctx.lineWidth = 1 / scale;
    for (let gx = 0; gx <= info.width; gx += 72) {
      ctx.beginPath();
      ctx.moveTo(gx, 0);
      ctx.lineTo(gx, info.height);
      ctx.stroke();
    }
    for (let gy = 0; gy <= info.height; gy += 72) {
      ctx.beginPath();
      ctx.moveTo(0, gy);
      ctx.lineTo(info.width, gy);
      ctx.stroke();
    }
    ctx.fillStyle = "#1b2a4a";
    ctx.font = "bold 40px sans-serif";
    ctx.textBaseline = "alphabetic";
    ctx.fillText(`Page ${src + 1}`, 72, 96);
    ctx.fillStyle = "#6a7388";
    ctx.font = "10px sans-serif";
    ctx.fillText(`position ${position + 1} - ${Math.round(info.width)} x ${Math.round(info.height)} pt`, 72, 114);
    ctx.fillStyle = "#222222";
    ctx.font = `${FONT_PT}px sans-serif`;
    mockLines(src).forEach((line, i) => {
      ctx.fillText(line, LINE_X, LINE_Y0 + i * LINE_STEP + FONT_PT, lineWidth(line));
    });
  }
  ctx.strokeStyle = "#5b6b8c";
  ctx.lineWidth = 2 / scale;
  ctx.strokeRect(1, 1, info.width - 2, info.height - 2);
  if (extras) {
    for (const wd of extras.widgets) paintWidget(ctx, wd);
    for (const a of extras.annots) paintAnnot(ctx, a);
  }
  ctx.restore();
}

function quadBox(q: Quad): { x: number; y: number; w: number; h: number } {
  const xs = [q[0], q[2], q[4], q[6]];
  const ys = [q[1], q[3], q[5], q[7]];
  const x = Math.min(...xs);
  const y = Math.min(...ys);
  return { x, y, w: Math.max(...xs) - x, h: Math.max(...ys) - y };
}

function paintWidget(ctx: Ctx, wd: FormWidget): void {
  const { x, y, width, height } = wd.rect;
  ctx.save();
  ctx.lineWidth = 1;
  ctx.strokeStyle = "#7a8aa8";
  ctx.fillStyle = wd.readOnly ? "#eceff5" : "#eef3ff";
  if (wd.kind === "checkbox" || wd.kind === "radio") {
    if (wd.kind === "radio") {
      ctx.beginPath();
      ctx.arc(x + width / 2, y + height / 2, width / 2, 0, Math.PI * 2);
      ctx.fill();
      ctx.stroke();
      if (wd.value === wd.exportValue) {
        ctx.fillStyle = "#1b2a4a";
        ctx.beginPath();
        ctx.arc(x + width / 2, y + height / 2, width / 4, 0, Math.PI * 2);
        ctx.fill();
      }
    } else {
      ctx.fillRect(x, y, width, height);
      ctx.strokeRect(x, y, width, height);
      if (wd.value === "true") {
        ctx.strokeStyle = "#1b2a4a";
        ctx.lineWidth = 2;
        ctx.beginPath();
        ctx.moveTo(x + 3, y + height / 2);
        ctx.lineTo(x + width / 2 - 1, y + height - 4);
        ctx.lineTo(x + width - 3, y + 3);
        ctx.stroke();
      }
    }
    ctx.restore();
    return;
  }
  ctx.fillRect(x, y, width, height);
  ctx.strokeRect(x, y, width, height);
  ctx.fillStyle = "#1b2a4a";
  ctx.font = "11px sans-serif";
  ctx.textBaseline = "middle";
  const shown = wd.password ? "\u2022".repeat(wd.display.length) : wd.kind === "list" ? "" : wd.display;
  if (wd.comb > 0) {
    const cell = width / wd.comb;
    ctx.strokeStyle = "#b5bfd3";
    for (let i = 1; i < wd.comb; i++) {
      ctx.beginPath();
      ctx.moveTo(x + i * cell, y);
      ctx.lineTo(x + i * cell, y + height);
      ctx.stroke();
    }
    ctx.textAlign = "center";
    [...shown].slice(0, wd.comb).forEach((ch, i) => ctx.fillText(ch, x + (i + 0.5) * cell, y + height / 2));
  } else if (wd.kind === "list") {
    (wd.options ?? []).forEach((o, i) => {
      if (wd.value.split("\n").includes(o.value)) {
        ctx.fillStyle = "#b9d0ff";
        ctx.fillRect(x + 1, y + 1 + i * 14, width - 2, 14);
        ctx.fillStyle = "#1b2a4a";
      }
      ctx.textAlign = "left";
      ctx.fillText(o.label, x + 4, y + 8 + i * 14);
    });
  } else {
    ctx.textAlign = wd.align;
    const tx = wd.align === "left" ? x + 3 : wd.align === "right" ? x + width - 3 : x + width / 2;
    const label = wd.kind === "combo" ? (wd.options?.find((o) => o.value === wd.value)?.label ?? wd.display) : shown;
    ctx.fillText(wd.multiline ? label.split("\n")[0] ?? "" : label, tx, wd.multiline ? y + 9 : y + height / 2, width - 6);
  }
  ctx.restore();
}

function paintAnnot(ctx: Ctx, a: Annot): void {
  const p = a.props;
  ctx.save();
  ctx.globalAlpha = p.opacity;
  ctx.strokeStyle = p.color;
  ctx.fillStyle = p.color;
  ctx.lineWidth = p.width;
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  const r = a.rect;
  switch (a.type) {
    case "highlight":
      for (const q of a.quads ?? []) {
        const b = quadBox(q);
        ctx.fillRect(b.x, b.y, b.w, b.h);
      }
      break;
    case "underline":
    case "squiggly":
    case "strikeout":
      for (const q of a.quads ?? []) {
        const b = quadBox(q);
        const y = a.type === "strikeout" ? b.y + b.h / 2 : b.y + b.h - 1;
        ctx.beginPath();
        ctx.moveTo(b.x, y);
        if (a.type === "squiggly") for (let x = b.x; x < b.x + b.w; x += 3) ctx.lineTo(x + 1.5, y + (((x - b.x) / 3) % 2 === 0 ? -1.5 : 1.5));
        else ctx.lineTo(b.x + b.w, y);
        ctx.stroke();
      }
      break;
    case "note":
      ctx.fillStyle = p.color;
      ctx.fillRect(r.x, r.y, r.width, r.height);
      ctx.strokeStyle = "#5a4a00";
      ctx.lineWidth = 1;
      ctx.strokeRect(r.x, r.y, r.width, r.height);
      break;
    case "textbox":
    case "fill-text":
      if (p.fill) {
        ctx.fillStyle = p.fill;
        ctx.fillRect(r.x, r.y, r.width, r.height);
      }
      if (a.type === "textbox") ctx.strokeRect(r.x, r.y, r.width, r.height);
      ctx.fillStyle = a.type === "fill-text" ? p.color : "#111111";
      ctx.font = "11px sans-serif";
      ctx.textBaseline = "top";
      ctx.textAlign = "left";
      p.contents.split("\n").forEach((line, i) => ctx.fillText(line, r.x + 3, r.y + 3 + i * 13, Math.max(10, r.width - 6)));
      break;
    case "fill-mark":
      ctx.font = `${Math.max(8, r.height)}px sans-serif`;
      ctx.textBaseline = "middle";
      ctx.textAlign = "center";
      ctx.fillText(p.contents === "check" ? "\u2713" : p.contents === "cross" ? "\u2717" : "\u2022", r.x + r.width / 2, r.y + r.height / 2);
      break;
    case "ink":
      for (const stroke of a.ink ?? []) {
        ctx.beginPath();
        stroke.forEach((pt, i) => (i === 0 ? ctx.moveTo(pt.x, pt.y) : ctx.lineTo(pt.x, pt.y)));
        ctx.stroke();
      }
      break;
    case "rectangle":
    case "oval":
      if (a.type === "rectangle") {
        if (p.fill) {
          ctx.fillStyle = p.fill;
          ctx.fillRect(r.x, r.y, r.width, r.height);
        }
        ctx.strokeRect(r.x, r.y, r.width, r.height);
      } else {
        ctx.beginPath();
        ctx.ellipse(r.x + r.width / 2, r.y + r.height / 2, Math.max(0.5, r.width / 2), Math.max(0.5, r.height / 2), 0, 0, Math.PI * 2);
        if (p.fill) {
          ctx.fillStyle = p.fill;
          ctx.fill();
        }
        ctx.stroke();
      }
      break;
    case "line":
    case "arrow":
      if (a.line) {
        const [s, e] = a.line;
        ctx.beginPath();
        ctx.moveTo(s.x, s.y);
        ctx.lineTo(e.x, e.y);
        ctx.stroke();
        if (a.type === "arrow") {
          const ang = Math.atan2(e.y - s.y, e.x - s.x);
          const len = 8 + p.width * 2;
          ctx.beginPath();
          ctx.moveTo(e.x - len * Math.cos(ang - 0.4), e.y - len * Math.sin(ang - 0.4));
          ctx.lineTo(e.x, e.y);
          ctx.lineTo(e.x - len * Math.cos(ang + 0.4), e.y - len * Math.sin(ang + 0.4));
          ctx.stroke();
        }
      }
      break;
    default:
      break;
  }
  ctx.restore();
}
