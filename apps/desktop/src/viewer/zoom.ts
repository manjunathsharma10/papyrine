import type { PageInfo } from "../ipc/contract";

/** 100% means 96/72 CSS px per point, i.e. a page at its physical size on a 96 dpi screen. */
export const CSS_PX_PER_PT = 96 / 72;
export const MIN_ZOOM = 0.1;
export const MAX_ZOOM = 64;
export const PRESETS = [0.25, 0.5, 0.75, 1, 1.25, 1.5, 2, 3, 4, 8, 16, 32, 64] as const;
/** Gap between pages and viewer padding in CSS px. */
export const PAGE_GAP = 12;
export const VIEW_PAD = 16;

export type ZoomMode = "fit-page" | "fit-width" | "actual" | "custom";
export type ViewMode = "continuous" | "single";

export function clampZoom(z: number): number {
  if (!Number.isFinite(z)) return 1;
  return Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, z));
}

/** Next preset above/below the current zoom (strictly), clamped to the range. */
export function stepZoom(z: number, dir: 1 | -1): number {
  const eps = 1e-6;
  if (dir === 1) {
    const next = PRESETS.find((p) => p > z + eps);
    return clampZoom(next ?? MAX_ZOOM);
  }
  const prev = [...PRESETS].reverse().find((p) => p < z - eps);
  return clampZoom(prev ?? MIN_ZOOM);
}

/** Ctrl-wheel / pinch: exponential so equal gestures give equal ratios. */
export function wheelZoom(z: number, deltaY: number, deltaMode = 0): number {
  const px = deltaMode === 1 ? deltaY * 16 : deltaMode === 2 ? deltaY * 400 : deltaY;
  return clampZoom(z * Math.exp(-px * 0.0125));
}

export function fitWidthZoom(containerWidth: number, pageWidthPt: number): number {
  return clampZoom(Math.max(1, containerWidth - 2 * VIEW_PAD) / (pageWidthPt * CSS_PX_PER_PT));
}

export function fitPageZoom(containerWidth: number, containerHeight: number, page: PageInfo): number {
  const w = Math.max(1, containerWidth - 2 * VIEW_PAD) / (page.width * CSS_PX_PER_PT);
  const h = Math.max(1, containerHeight - 2 * VIEW_PAD) / (page.height * CSS_PX_PER_PT);
  return clampZoom(Math.min(w, h));
}

export function formatZoom(z: number): string {
  const pct = z * 100;
  return `${pct >= 100 ? Math.round(pct) : Math.round(pct * 10) / 10}%`;
}

/** Parse "150", "150%", " 12.5 %" into a zoom factor; null when invalid. */
export function parseZoom(input: string): number | null {
  const m = /^\s*(\d+(?:[.,]\d+)?)\s*%?\s*$/.exec(input);
  if (!m) return null;
  const v = Number((m[1] as string).replace(",", ".")) / 100;
  return v > 0 ? clampZoom(v) : null;
}

export interface PageLayout {
  /** Top offset of each item in the scroll content, CSS px. */
  tops: number[];
  widths: number[];
  heights: number[];
  totalHeight: number;
  totalWidth: number;
}

/** Vertical stack of the given pages at `zoom`, with viewer padding. */
export function layoutPages(pages: PageInfo[], zoom: number, containerWidth: number): PageLayout {
  const s = zoom * CSS_PX_PER_PT;
  const tops: number[] = [];
  const widths: number[] = [];
  const heights: number[] = [];
  let y = VIEW_PAD;
  let maxW = 0;
  for (const p of pages) {
    const w = p.width * s;
    const h = p.height * s;
    tops.push(y);
    widths.push(w);
    heights.push(h);
    y += h + PAGE_GAP;
    maxW = Math.max(maxW, w);
  }
  return {
    tops,
    widths,
    heights,
    totalHeight: Math.max(0, y - PAGE_GAP) + VIEW_PAD,
    totalWidth: Math.max(containerWidth, maxW + 2 * VIEW_PAD),
  };
}

/** Device-pixel tile scale for a zoom: quantised so nearby zooms share a tile set. */
export function tileScale(zoom: number, dpr: number): number {
  const raw = zoom * CSS_PX_PER_PT * dpr;
  // Quantise to 1/8 octave steps; the CSS size stays exact so tiles are stretched slightly at most.
  return Math.pow(2, Math.round(Math.log2(raw) * 8) / 8);
}
