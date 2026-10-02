import { type PageInfo, type PageText, type Tile, TILE_SIZE } from "./contract";

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
  const rnd = mulberry(src * 7919 + 13);
  const lines: string[] = [];
  for (let i = 0; i < LINES; i++) {
    const n = 6 + Math.floor(rnd() * 6);
    const words: string[] = [];
    for (let k = 0; k < n; k++) words.push(WORDS[Math.floor(rnd() * WORDS.length)] ?? "x");
    lines.push(words.join(" "));
  }
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
export async function renderMockTile(
  info: PageInfo,
  src: number,
  position: number,
  scale: number,
  tx: number,
  ty: number,
): Promise<Tile> {
  const fullW = Math.ceil(info.width * scale);
  const fullH = Math.ceil(info.height * scale);
  const x0 = tx * TILE_SIZE;
  const y0 = ty * TILE_SIZE;
  const w = Math.max(1, Math.min(TILE_SIZE, fullW - x0));
  const h = Math.max(1, Math.min(TILE_SIZE, fullH - y0));
  const canvas = makeCanvas(w, h);
  const ctx = canvas.getContext("2d") as Ctx;
  paint(ctx, info, src, position, scale, x0, y0, w, h);
  const bitmap = await createImageBitmap(canvas);
  return { bitmap, width: w, height: h };
}

function paint(ctx: Ctx, info: PageInfo, src: number, position: number, scale: number, x0: number, y0: number, w: number, h: number): void {
  // Page content is never mirrored, whatever the UI direction is.
  ctx.direction = "ltr";
  ctx.textAlign = "left";
  ctx.fillStyle = "#ffffff";
  ctx.fillRect(0, 0, w, h);
  ctx.save();
  ctx.translate(-x0, -y0);
  ctx.scale(scale, scale);
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
  ctx.strokeStyle = "#5b6b8c";
  ctx.lineWidth = 2 / scale;
  ctx.strokeRect(1, 1, info.width - 2, info.height - 2);
  ctx.restore();
}
